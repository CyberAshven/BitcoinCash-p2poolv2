// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

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
        body_of(reply_to(
            br#"{"method":"getdifficulty","params":[],"id":0}"#,
            &template
        )),
        "{\"result\":1,\"error\":null,\"id\":0}\n"
    );
}

#[test]
fn submitblock_answers_null() {
    let template = load_template(None).expect("built-in template");
    assert_eq!(
        body_of(reply_to(
            br#"{"method":"submitblock","params":["00"],"id":"s"}"#,
            &template
        )),
        "{\"result\":null,\"error\":null,\"id\":\"s\"}\n"
    );
}

#[test]
fn getbestblockhash_answers_the_template_parent() {
    let template = load_template(None).expect("built-in template");
    assert_eq!(
        body_of(reply_to(
            br#"{"method":"getbestblockhash","id":2}"#,
            &template
        )),
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
        body_of(reply_to(
            br#"{"method":"validateaddress","id":4}"#,
            &template
        )),
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
