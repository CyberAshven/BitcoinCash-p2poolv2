// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A mock node JSON-RPC 1.0 server for stratum load tests.
//!
//! It answers `getblocktemplate` with a fixed template whose target is so
//! easy that every share a load-test miner submits is a block, so the pool
//! under test calls `submitblock` on every submit. It replaces
//! `load-tests/jmeter-testing/mock-bitcoind/server.js` and serves the same
//! responses on the same port.
//!
//! By default it answers as BCHN, the Bitcoin Cash node this pool mines
//! against: the built-in template is a BCHN regtest template
//! (`bitcoindrpc/test_data/bchn-regtest-template.json`) and
//! `validateaddress` decodes a CashAddr and answers with BCHN's fields.
//! [`NodeKind::BitcoinCore`] keeps the JavaScript server's Bitcoin signet
//! template and answers, for the JMeter comparison with CKPool, which still
//! mines Bitcoin signet (`config-load-test.toml`, the CKPool configs).
//!
//! As BCHN it also serves the header of the block its template builds on:
//! the node follows BCHN's headers (mining on the current chain), and the
//! template's real parent has no header to serve. So the mock makes up a
//! parent, a first block of a regtest chain (previous block all zeros), and
//! points the template at it; `getbestblockhash` names it and
//! `getblockheader` serves it. The template keeps its own height.
//!
//! HTTP is handled by hand on a raw TCP socket, as the JavaScript server did:
//! CKPool reads with LF-only line endings and non-blocking reads, so each
//! response (headers and body) goes out in one write.

use crate::error::{ToolError, ToolResult};
use bitcoin::block::{Header, Version};
use bitcoin::consensus::encode::serialize_hex;
use bitcoin::hashes::Hash;
use bitcoin::{BlockHash, CompactTarget, TxMerkleNode};
use p2poolv2_config::{BitcoinCashNetwork, Chain, parse_payout_address};
use p2poolv2_wallet::bch_share::encode_payout_address;
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

/// The BCHN regtest template, the default.
const BCHN_REGTEST_TEMPLATE: &str =
    include_str!("../../bitcoindrpc/test_data/bchn-regtest-template.json");

/// A BCHN chipnet template.
const BCHN_CHIPNET_TEMPLATE: &str =
    include_str!("../../bitcoindrpc/test_data/bchn-chipnet-template.json");

/// The Bitcoin signet template the JavaScript server served.
const BITCOIN_SIGNET_TEMPLATE: &str =
    include_str!("../../p2poolv2_tests/test_data/validation/stratum/b/template.json");

/// Overrides that make every share a block, whatever extranonce the stratum
/// server hands out.
const EASY_BITS: &str = "2100ffff";
const EASY_TARGET: &str = "ffff000000000000000000000000000000000000000000000000000000000000";

const BAD_REQUEST: &[u8] =
    b"HTTP/1.1 400 Bad Request\r\nConnection: close\r\nContent-Length: 12\r\n\r\nInvalid JSON";

static CONTENT_LENGTH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)content-length:\s*(\d+)").expect("valid regex"));

/// The node the mock answers as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// BCHN on a Bitcoin Cash network, the default on regtest.
    Bchn(BitcoinCashNetwork),
    /// Bitcoin Core on signet, as the JavaScript server answered: for the
    /// JMeter comparison with CKPool, which mines Bitcoin.
    BitcoinCore,
}

impl Default for NodeKind {
    fn default() -> Self {
        NodeKind::Bchn(BitcoinCashNetwork::Regtest)
    }
}

impl NodeKind {
    /// The built-in template for this node, if there is one. BCHN mainnet
    /// and scalenet have none: pass one with `--template`.
    fn built_in_template(self) -> Option<(&'static str, &'static str)> {
        match self {
            NodeKind::Bchn(BitcoinCashNetwork::Regtest) => {
                Some(("built-in BCHN regtest template", BCHN_REGTEST_TEMPLATE))
            }
            NodeKind::Bchn(BitcoinCashNetwork::Chipnet) => {
                Some(("built-in BCHN chipnet template", BCHN_CHIPNET_TEMPLATE))
            }
            NodeKind::Bchn(BitcoinCashNetwork::Mainnet | BitcoinCashNetwork::Scalenet) => None,
            NodeKind::BitcoinCore => {
                Some(("built-in Bitcoin signet template", BITCOIN_SIGNET_TEMPLATE))
            }
        }
    }
}

/// What the mock serves: a block template, answered as one kind of node.
#[derive(Debug, Clone)]
pub struct MockNode {
    kind: NodeKind,
    template: Value,
    /// The made-up block a BCHN template builds on, served by
    /// `getblockheader`.
    parent: Option<Header>,
}

impl MockNode {
    /// The mock for `kind`, serving the template at `path` when given, else
    /// the node's built-in one, with the easy `bits` and `target` overrides
    /// applied.
    pub fn load(kind: NodeKind, path: Option<&Path>) -> ToolResult<Self> {
        let mut template = load_template(kind, path)?;
        let parent = match kind {
            NodeKind::Bchn(_) => {
                let parent = made_up_parent(&template);
                template["previousblockhash"] = Value::String(parent.block_hash().to_string());
                Some(parent)
            }
            NodeKind::BitcoinCore => None,
        };
        Ok(Self {
            kind,
            template,
            parent,
        })
    }

    /// The made-up block a BCHN template builds on.
    pub fn parent(&self) -> Option<&Header> {
        self.parent.as_ref()
    }

    /// The template `getblocktemplate` answers with.
    pub fn template(&self) -> &Value {
        &self.template
    }
}

/// A regtest chain's first block, dated ten minutes before `template`'s
/// `curtime`, for a BCHN template to build on.
fn made_up_parent(template: &Value) -> Header {
    let curtime = template
        .get("curtime")
        .and_then(Value::as_u64)
        .and_then(|time| u32::try_from(time).ok())
        .unwrap_or(1_700_000_000);
    Header {
        version: Version::from_consensus(0x2000_0000),
        prev_blockhash: BlockHash::all_zeros(),
        merkle_root: TxMerkleNode::all_zeros(),
        time: curtime.saturating_sub(600),
        bits: CompactTarget::from_consensus(0x207f_ffff),
        nonce: 0,
    }
}

/// `getblockheader`: the made-up parent, raw when `params[1]` is false,
/// else its verbose fields with height 0. Any other block is unknown, as
/// BCHN answers (code -5).
fn block_header_reply(params: Option<&Value>, parent: Option<&Header>) -> Result<String, RpcError> {
    let asked = params
        .and_then(|params| params.get(0))
        .and_then(Value::as_str);
    let verbose = params
        .and_then(|params| params.get(1))
        .is_none_or(is_truthy);
    match parent {
        Some(parent) if asked == Some(parent.block_hash().to_string().as_str()) => {
            if verbose {
                Ok(serialize(&serde_json::json!({
                    "hash": parent.block_hash().to_string(),
                    "height": 0,
                })))
            } else {
                Ok(serialize(&serialize_hex(parent)))
            }
        }
        _ => Err(RpcError {
            code: -5,
            message: "Block not found".to_string(),
        }),
    }
}

/// The block template to serve: `path` when given, else `kind`'s built-in
/// one, with the easy `bits` and `target` overrides applied.
pub fn load_template(kind: NodeKind, path: Option<&Path>) -> ToolResult<Value> {
    let (origin, text): (String, Cow<'_, str>) = match path {
        Some(path) => (
            path.display().to_string(),
            Cow::Owned(
                std::fs::read_to_string(path)
                    .map_err(|error| ToolError::io("read", path, error))?,
            ),
        ),
        None => {
            let (origin, text) = kind.built_in_template().ok_or_else(|| {
                ToolError::message("there is no built-in BCHN mainnet template; pass --template")
            })?;
            (origin.to_string(), Cow::Borrowed(text))
        }
    };
    let mut template: Value =
        serde_json::from_str(&text).map_err(|source| ToolError::Json { origin, source })?;
    let fields = template
        .as_object_mut()
        .ok_or_else(|| ToolError::message("the block template is not a JSON object"))?;
    fields.insert("bits".to_string(), Value::from(EASY_BITS));
    fields.insert("target".to_string(), Value::from(EASY_TARGET));
    Ok(template)
}

/// Serve JSON-RPC on `listener` until the process ends, at most
/// `max_connections` connections at a time.
pub async fn serve(listener: TcpListener, node: Arc<MockNode>, max_connections: usize) {
    let slots = Arc::new(Semaphore::new(max_connections.max(1)));
    loop {
        let slot = match slots.clone().acquire_owned().await {
            Ok(slot) => slot,
            Err(_) => return,
        };
        match listener.accept().await {
            Ok((stream, _)) => {
                let node = Arc::clone(&node);
                tokio::spawn(async move {
                    handle_connection(stream, &node).await;
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

async fn handle_connection(mut stream: TcpStream, node: &MockNode) {
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
                    let reply = reply_to(&buffer[body_start..body_end], node);
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

/// Bitcoin Core's `validateaddress` result, in the field order the
/// JavaScript server used. It calls every address a valid witness address.
#[derive(Serialize)]
struct CoreValidateAddress<'a> {
    isvalid: bool,
    address: Cow<'a, Value>,
    #[serde(rename = "scriptPubKey")]
    script_pub_key: &'static str,
    isscript: bool,
    iswitness: bool,
}

/// BCHN's `validateaddress` result, in BCHN's field order
/// (`src/rpc/misc.cpp`): an invalid address answers `isvalid` alone.
#[derive(Debug, Serialize, PartialEq, Eq)]
struct BchnValidateAddress {
    isvalid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    address: Option<String>,
    #[serde(rename = "scriptPubKey", skip_serializing_if = "Option::is_none")]
    script_pub_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    isscript: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    istokenaware: Option<bool>,
}

impl BchnValidateAddress {
    fn invalid() -> Self {
        Self {
            isvalid: false,
            address: None,
            script_pub_key: None,
            isscript: None,
            istokenaware: None,
        }
    }
}

/// `validateaddress` as BCHN answers it on `network`.
///
/// The address is read with the node's own CashAddr parser, the one it
/// reads payout addresses with. Like BCHN it takes the address with or
/// without the network's prefix and answers with the prefixed, lowercase
/// form, the hex `scriptPubKey`, whether it pays a script and whether it is
/// token-aware. The parser takes the P2PKH, P2SH20 and P2SH32 addresses this
/// node pays, token-aware or not, so a legacy base58 address, which BCHN also
/// accepts, answers `isvalid: false` here.
fn bchn_validate_address(address: &Value, network: BitcoinCashNetwork) -> BchnValidateAddress {
    let Some(text) = address.as_str() else {
        return BchnValidateAddress::invalid();
    };
    let prefixed = if text.contains(':') {
        Cow::Borrowed(text)
    } else {
        Cow::Owned(format!("{}:{text}", network.cashaddr_prefix()))
    };
    let Ok(parsed) = parse_payout_address(
        &prefixed,
        Chain::BitcoinCash,
        network.legacy_network(),
        Some(network),
    ) else {
        return BchnValidateAddress::invalid();
    };
    // The first payload symbol carries the CashAddr type: `z` and `r` are
    // the token-aware P2PKH and P2SH types, whatever the hash size. BCHN answers a
    // token-aware address in its own, token-aware form.
    let token_aware = prefixed
        .split_once(':')
        .and_then(|(_, payload)| payload.chars().next())
        .is_some_and(|symbol| matches!(symbol.to_ascii_lowercase(), 'z' | 'r'));
    let canonical = if token_aware {
        prefixed.to_ascii_lowercase()
    } else {
        match encode_payout_address(&parsed, network.share_network()) {
            Ok(canonical) => canonical,
            Err(_) => return BchnValidateAddress::invalid(),
        }
    };
    let script = parsed.script_pubkey();
    BchnValidateAddress {
        isvalid: true,
        address: Some(canonical),
        script_pub_key: Some(script.to_hex_string()),
        isscript: Some(script.is_p2sh() || parsed.is_p2sh32()),
        istokenaware: Some(token_aware),
    }
}

/// The HTTP response for one request body.
fn reply_to(body: &[u8], node: &MockNode) -> Reply {
    let text = String::from_utf8_lossy(body);
    let request: Value = match serde_json::from_str(&text) {
        Ok(request) => request,
        Err(_) => return Reply::BadRequest,
    };
    let method = request.get("method");
    let params = request.get("params");
    let template = &node.template;
    let result = match method.and_then(Value::as_str) {
        Some("getblocktemplate") => Ok(serialize(template)),
        Some("submitblock") => Ok("null".to_string()),
        Some("getdifficulty") => Ok("1".to_string()),
        Some("getbestblockhash") => Ok(serialize(
            template.get("previousblockhash").unwrap_or(&Value::Null),
        )),
        Some("getblockheader") => block_header_reply(params, node.parent.as_ref()),
        Some("validateaddress") => Ok(match node.kind {
            NodeKind::Bchn(network) => serialize(&bchn_validate_address(
                &first_param_or_empty(params),
                network,
            )),
            NodeKind::BitcoinCore => serialize(&CoreValidateAddress {
                isvalid: true,
                address: first_param_or_empty(params),
                script_pub_key: "",
                isscript: false,
                iswitness: true,
            }),
        }),
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
mod tests;
