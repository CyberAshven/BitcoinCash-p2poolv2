// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A mock bitcoind JSON-RPC 1.0 server for stratum load tests.
//!
//! It answers `getblocktemplate` with a fixed template whose target is so
//! easy that every share a load-test miner submits is a block, so the pool
//! under test calls `submitblock` on every submit. It replaces
//! `load-tests/jmeter-testing/mock-bitcoind/server.js` and serves the same
//! responses on the same port.
//!
//! HTTP is handled by hand on a raw TCP socket, as the JavaScript server did:
//! CKPool reads with LF-only line endings and non-blocking reads, so each
//! response (headers and body) goes out in one write.

use crate::error::{ToolError, ToolResult};
use regex::bytes::Regex;
use serde::Serialize;
use serde_json::Value;
use std::borrow::Cow;
use std::path::Path;
use std::sync::{Arc, LazyLock};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tokio::time::{Instant, timeout_at};

/// The port the JavaScript server listened on.
pub const DEFAULT_PORT: u16 = 48332;

/// Concurrent connections served at once; further connections wait in the
/// listen backlog.
pub const DEFAULT_MAX_CONNECTIONS: usize = 512;

/// Largest request (headers and body) a connection may buffer. A larger one
/// closes the connection.
pub const MAX_REQUEST_BYTES: usize = 8 * 1024 * 1024;

/// How long a connection stays open after a response, for a client that
/// sends another request before reading `Connection: close`.
const LINGER_AFTER_RESPONSE: Duration = Duration::from_millis(50);

/// The template the JavaScript server served, before its overrides.
const DEFAULT_TEMPLATE: &str =
    include_str!("../../p2poolv2_tests/test_data/validation/stratum/b/template.json");

/// Overrides that make every share a block, whatever extranonce the stratum
/// server hands out.
const EASY_BITS: &str = "2100ffff";
const EASY_TARGET: &str = "ffff000000000000000000000000000000000000000000000000000000000000";

const BAD_REQUEST: &[u8] =
    b"HTTP/1.1 400 Bad Request\r\nConnection: close\r\nContent-Length: 12\r\n\r\nInvalid JSON";

static CONTENT_LENGTH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)content-length:\s*(\d+)").expect("valid regex"));

/// The block template to serve: `path` when given, else the built-in one,
/// with the easy `bits` and `target` overrides applied.
pub fn load_template(path: Option<&Path>) -> ToolResult<Value> {
    let text: Cow<'_, str> = match path {
        Some(path) => Cow::Owned(
            std::fs::read_to_string(path).map_err(|error| ToolError::io("read", path, error))?,
        ),
        None => Cow::Borrowed(DEFAULT_TEMPLATE),
    };
    let mut template: Value = serde_json::from_str(&text).map_err(|source| ToolError::Json {
        origin: path.map_or_else(|| "built-in template".to_string(), |path| path.display().to_string()),
        source,
    })?;
    let fields = template
        .as_object_mut()
        .ok_or_else(|| ToolError::message("the block template is not a JSON object"))?;
    fields.insert("bits".to_string(), Value::from(EASY_BITS));
    fields.insert("target".to_string(), Value::from(EASY_TARGET));
    Ok(template)
}

/// Serve JSON-RPC on `listener` until the process ends, at most
/// `max_connections` connections at a time.
pub async fn serve(listener: TcpListener, template: Arc<Value>, max_connections: usize) {
    let slots = Arc::new(Semaphore::new(max_connections.max(1)));
    loop {
        let slot = match slots.clone().acquire_owned().await {
            Ok(slot) => slot,
            Err(_) => return,
        };
        match listener.accept().await {
            Ok((stream, _)) => {
                let template = Arc::clone(&template);
                tokio::spawn(async move {
                    handle_connection(stream, &template).await;
                    drop(slot);
                });
            }
            // Out of file descriptors and the like: back off and keep serving.
            Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
        }
    }
}

/// What to send back for one request.
#[derive(Debug, PartialEq, Eq)]
enum Reply {
    /// A 200 response; the connection stays open briefly.
    Rpc(Vec<u8>),
    /// A 400 response, after which the connection closes.
    BadRequest,
}

/// Where a complete request sits in the connection buffer.
#[derive(Debug, PartialEq, Eq)]
enum Framing {
    Incomplete,
    TooLarge,
    Complete { body_start: usize, body_end: usize },
}

async fn handle_connection(mut stream: TcpStream, template: &Value) {
    // Errors on a socket only end that connection, as in the JavaScript server.
    let _ = stream.set_nodelay(true);
    let mut buffer: Vec<u8> = Vec::with_capacity(4096);
    let mut chunk = vec![0u8; 16 * 1024];
    let mut linger_until: Option<Instant> = None;
    loop {
        loop {
            match frame_request(&buffer) {
                Framing::Incomplete => break,
                Framing::TooLarge => return,
                Framing::Complete {
                    body_start,
                    body_end,
                } => {
                    let reply = reply_to(&buffer[body_start..body_end], template);
                    buffer.drain(..body_end);
                    match reply {
                        Reply::Rpc(response) => {
                            if stream.write_all(&response).await.is_err() {
                                return;
                            }
                            linger_until = Some(Instant::now() + LINGER_AFTER_RESPONSE);
                        }
                        Reply::BadRequest => {
                            let _ = stream.write_all(BAD_REQUEST).await;
                            let _ = stream.shutdown().await;
                            return;
                        }
                    }
                }
            }
        }
        let read = match linger_until {
            Some(deadline) => match timeout_at(deadline, stream.read(&mut chunk)).await {
                Ok(read) => read,
                Err(_) => {
                    let _ = stream.shutdown().await;
                    return;
                }
            },
            None => stream.read(&mut chunk).await,
        };
        match read {
            Ok(0) | Err(_) => return,
            Ok(count) => buffer.extend_from_slice(&chunk[..count]),
        }
        if buffer.len() > MAX_REQUEST_BYTES {
            return;
        }
    }
}

/// Find the first complete HTTP request in `buffer`. Headers end at the
/// first blank line, CRLF or bare LF; the body is `Content-Length` bytes,
/// or empty without that header.
fn frame_request(buffer: &[u8]) -> Framing {
    let header_end = find(buffer, b"\r\n\r\n")
        .map(|position| (position, 4))
        .or_else(|| find(buffer, b"\n\n").map(|position| (position, 2)));
    let Some((header_length, separator_length)) = header_end else {
        return if buffer.len() > MAX_REQUEST_BYTES {
            Framing::TooLarge
        } else {
            Framing::Incomplete
        };
    };
    let content_length = match CONTENT_LENGTH.captures(&buffer[..header_length]) {
        None => 0,
        Some(found) => match std::str::from_utf8(&found[1])
            .ok()
            .and_then(|digits| digits.parse::<usize>().ok())
        {
            Some(length) => length,
            None => return Framing::TooLarge,
        },
    };
    let body_start = header_length + separator_length;
    if content_length > MAX_REQUEST_BYTES.saturating_sub(body_start) {
        return Framing::TooLarge;
    }
    let body_end = body_start + content_length;
    if buffer.len() < body_end {
        return Framing::Incomplete;
    }
    Framing::Complete {
        body_start,
        body_end,
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[derive(Serialize)]
struct RpcError {
    code: i64,
    message: String,
}

/// `validateaddress` result, in the field order the JavaScript server used.
#[derive(Serialize)]
struct ValidateAddress<'a> {
    isvalid: bool,
    address: Cow<'a, Value>,
    #[serde(rename = "scriptPubKey")]
    script_pub_key: &'static str,
    isscript: bool,
    iswitness: bool,
}

/// The HTTP response for one request body.
fn reply_to(body: &[u8], template: &Value) -> Reply {
    let text = String::from_utf8_lossy(body);
    let request: Value = match serde_json::from_str(&text) {
        Ok(request) => request,
        Err(_) => return Reply::BadRequest,
    };
    let method = request.get("method");
    let params = request.get("params");
    let result = match method.and_then(Value::as_str) {
        Some("getblocktemplate") => Ok(serialize(template)),
        Some("submitblock") => Ok("null".to_string()),
        Some("getdifficulty") => Ok("1".to_string()),
        Some("getbestblockhash") => Ok(serialize(
            template.get("previousblockhash").unwrap_or(&Value::Null),
        )),
        Some("validateaddress") => Ok(serialize(&ValidateAddress {
            isvalid: true,
            address: first_param_or_empty(params),
            script_pub_key: "",
            isscript: false,
            iswitness: true,
        })),
        _ => Err(RpcError {
            code: -32601,
            message: format!("Method not found: {}", javascript_display(method)),
        }),
    };
    let mut json = String::with_capacity(256);
    match result {
        Ok(result) => {
            json.push_str("{\"result\":");
            json.push_str(&result);
            json.push_str(",\"error\":null");
        }
        Err(error) => {
            json.push_str("{\"result\":null,\"error\":");
            json.push_str(&serialize(&error));
        }
    }
    // A request without an id gets a response without one, as
    // JSON.stringify drops an undefined field.
    if let Some(id) = request.get("id") {
        json.push_str(",\"id\":");
        json.push_str(&serialize(id));
    }
    json.push_str("}\n");
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        json.len()
    )
    .into_bytes();
    response.extend_from_slice(json.as_bytes());
    Reply::Rpc(response)
}

fn serialize(value: &impl Serialize) -> String {
    // Serializing a Value or a plain struct cannot fail.
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())
}

/// `params && params[0] ? params[0] : ""` from the JavaScript server.
fn first_param_or_empty(params: Option<&Value>) -> Cow<'_, Value> {
    let first = match params {
        Some(Value::Array(items)) => items.first().map(Cow::Borrowed),
        Some(Value::Object(fields)) => fields.get("0").map(Cow::Borrowed),
        Some(Value::String(text)) => text
            .chars()
            .next()
            .map(|character| Cow::Owned(Value::from(character.to_string()))),
        _ => None,
    };
    match first {
        Some(value) if is_truthy(&value) => value,
        _ => Cow::Owned(Value::from("")),
    }
}

/// JavaScript truthiness of a JSON value.
fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|float| float != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// How a template literal prints a JSON value, for the method name in a
/// "Method not found" message.
fn javascript_display(value: Option<&Value>) -> String {
    match value {
        None => "undefined".to_string(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Object(_)) => "[object Object]".to_string(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                Value::Null => String::new(),
                other => javascript_display(Some(other)),
            })
            .collect::<Vec<_>>()
            .join(","),
        Some(other) => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body_of(reply: Reply) -> String {
        match reply {
            Reply::Rpc(bytes) => {
                let text = String::from_utf8(bytes).expect("utf-8 response");
                let (_, body) = text.split_once("\r\n\r\n").expect("header end");
                body.to_string()
            }
            Reply::BadRequest => panic!("expected an RPC reply"),
        }
    }

    #[test]
    fn template_overrides_bits_and_target() {
        let template = load_template(None).expect("built-in template");
        assert_eq!(template["bits"], "2100ffff");
        assert_eq!(template["target"], EASY_TARGET);
        assert_eq!(
            template["previousblockhash"],
            "00000000863a9563ff5d87f94b9db355a53326456301fcaf8f665af26d600f56"
        );
    }

    #[test]
    fn frame_request_waits_for_the_header_end() {
        assert_eq!(frame_request(b"POST / HTTP/1.1\r\n"), Framing::Incomplete);
    }

    #[test]
    fn frame_request_waits_for_the_whole_body() {
        assert_eq!(
            frame_request(b"POST / HTTP/1.1\r\nContent-Length: 5\r\n\r\nab"),
            Framing::Incomplete
        );
    }

    #[test]
    fn frame_request_finds_a_crlf_request() {
        assert_eq!(
            frame_request(b"POST / HTTP/1.1\r\ncontent-length: 2\r\n\r\n{}extra"),
            Framing::Complete {
                body_start: 38,
                body_end: 40
            }
        );
    }

    #[test]
    fn frame_request_accepts_lf_only_headers() {
        assert_eq!(
            frame_request(b"POST / HTTP/1.1\nContent-Length: 2\n\n{}"),
            Framing::Complete {
                body_start: 35,
                body_end: 37
            }
        );
    }

    #[test]
    fn frame_request_without_length_has_empty_body() {
        assert_eq!(
            frame_request(b"GET / HTTP/1.1\r\n\r\n"),
            Framing::Complete {
                body_start: 18,
                body_end: 18
            }
        );
    }

    #[test]
    fn frame_request_rejects_an_oversized_body() {
        assert_eq!(
            frame_request(b"POST / HTTP/1.1\r\nContent-Length: 99999999999\r\n\r\n"),
            Framing::TooLarge
        );
    }

    #[test]
    fn invalid_json_is_a_bad_request() {
        let template = load_template(None).expect("built-in template");
        assert_eq!(reply_to(b"{nope", &template), Reply::BadRequest);
    }

    #[test]
    fn getdifficulty_answers_one() {
        let template = load_template(None).expect("built-in template");
        assert_eq!(
            body_of(reply_to(br#"{"method":"getdifficulty","params":[],"id":0}"#, &template)),
            "{\"result\":1,\"error\":null,\"id\":0}\n"
        );
    }

    #[test]
    fn submitblock_answers_null() {
        let template = load_template(None).expect("built-in template");
        assert_eq!(
            body_of(reply_to(br#"{"method":"submitblock","params":["00"],"id":"s"}"#, &template)),
            "{\"result\":null,\"error\":null,\"id\":\"s\"}\n"
        );
    }

    #[test]
    fn getbestblockhash_answers_the_template_parent() {
        let template = load_template(None).expect("built-in template");
        assert_eq!(
            body_of(reply_to(br#"{"method":"getbestblockhash","id":2}"#, &template)),
            "{\"result\":\"00000000863a9563ff5d87f94b9db355a53326456301fcaf8f665af26d600f56\",\"error\":null,\"id\":2}\n"
        );
    }

    #[test]
    fn validateaddress_echoes_the_address_in_javascript_field_order() {
        let template = load_template(None).expect("built-in template");
        assert_eq!(
            body_of(reply_to(
                br#"{"method":"validateaddress","params":["tb1qx"],"id":3}"#,
                &template
            )),
            "{\"result\":{\"isvalid\":true,\"address\":\"tb1qx\",\"scriptPubKey\":\"\",\"isscript\":false,\"iswitness\":true},\"error\":null,\"id\":3}\n"
        );
    }

    #[test]
    fn validateaddress_without_params_has_empty_address() {
        let template = load_template(None).expect("built-in template");
        assert_eq!(
            body_of(reply_to(br#"{"method":"validateaddress","id":4}"#, &template)),
            "{\"result\":{\"isvalid\":true,\"address\":\"\",\"scriptPubKey\":\"\",\"isscript\":false,\"iswitness\":true},\"error\":null,\"id\":4}\n"
        );
    }

    #[test]
    fn unknown_method_is_method_not_found() {
        let template = load_template(None).expect("built-in template");
        assert_eq!(
            body_of(reply_to(br#"{"method":"stop","id":5}"#, &template)),
            "{\"result\":null,\"error\":{\"code\":-32601,\"message\":\"Method not found: stop\"},\"id\":5}\n"
        );
    }

    #[test]
    fn request_without_id_gets_response_without_id() {
        let template = load_template(None).expect("built-in template");
        assert_eq!(
            body_of(reply_to(br#"{"method":"submitblock"}"#, &template)),
            "{\"result\":null,\"error\":null}\n"
        );
    }

    #[test]
    fn missing_method_reads_undefined() {
        let template = load_template(None).expect("built-in template");
        assert_eq!(
            body_of(reply_to(br#"{"id":null}"#, &template)),
            "{\"result\":null,\"error\":{\"code\":-32601,\"message\":\"Method not found: undefined\"},\"id\":null}\n"
        );
    }

    #[test]
    fn content_length_counts_the_trailing_newline() {
        let template = load_template(None).expect("built-in template");
        let reply = reply_to(br#"{"method":"getdifficulty","id":0}"#, &template);
        let Reply::Rpc(bytes) = reply else {
            panic!("expected an RPC reply");
        };
        let text = String::from_utf8(bytes).expect("utf-8 response");
        assert!(text.contains("Content-Length: 33\r\n"));
    }
}
