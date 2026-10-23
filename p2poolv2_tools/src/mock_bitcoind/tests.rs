// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use p2poolv2_wallet::bch_share::{CashAddrType, encode_cashaddr};

/// The parent of the BCHN regtest template.
const BCHN_REGTEST_PARENT: &str =
    "500d7a714666fdf354063124ab09342ae8326f2a2f4b161908749af29fbd0afe";

/// The parent of the Bitcoin signet template.
const BITCOIN_SIGNET_PARENT: &str =
    "00000000863a9563ff5d87f94b9db355a53326456301fcaf8f665af26d600f56";

/// The default mock: BCHN on regtest.
fn bchn() -> MockNode {
    MockNode::load(NodeKind::default(), None).expect("BCHN regtest mock")
}

fn bitcoin_core() -> MockNode {
    MockNode::load(NodeKind::BitcoinCore, None).expect("Bitcoin Core mock")
}

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

fn validate(node: &MockNode, address: &str) -> Value {
    let request = format!(r#"{{"method":"validateaddress","params":["{address}"],"id":7}}"#);
    let body = body_of(reply_to(request.as_bytes(), node));
    let response: Value = serde_json::from_str(&body).expect("JSON body");
    response["result"].clone()
}

#[test]
fn the_default_is_the_bchn_regtest_template() {
    let template = bchn().template().clone();
    assert_eq!(template["bits"], "2100ffff");
    assert_eq!(template["target"], EASY_TARGET);
    assert_eq!(template["previousblockhash"], BCHN_REGTEST_PARENT);
    assert_eq!(template["height"], 102);
}

#[test]
fn bitcoin_core_serves_the_bitcoin_signet_template() {
    let template = bitcoin_core().template().clone();
    assert_eq!(template["bits"], "2100ffff");
    assert_eq!(template["previousblockhash"], BITCOIN_SIGNET_PARENT);
}

#[test]
fn bchn_chipnet_serves_the_chipnet_template() {
    let node = MockNode::load(NodeKind::Bchn(BitcoinCashNetwork::Chipnet), None)
        .expect("BCHN chipnet mock");
    assert_eq!(node.template()["bits"], "2100ffff");
}

#[test]
fn bchn_mainnet_needs_a_template() {
    assert!(MockNode::load(NodeKind::Bchn(BitcoinCashNetwork::Mainnet), None).is_err());
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
    let node = bchn();
    assert_eq!(reply_to(b"{nope", &node), Reply::BadRequest);
}

#[test]
fn getdifficulty_answers_one() {
    let node = bchn();
    assert_eq!(
        body_of(reply_to(
            br#"{"method":"getdifficulty","params":[],"id":0}"#,
            &node
        )),
        "{\"result\":1,\"error\":null,\"id\":0}\n"
    );
}

#[test]
fn submitblock_answers_null() {
    let node = bchn();
    assert_eq!(
        body_of(reply_to(
            br#"{"method":"submitblock","params":["00"],"id":"s"}"#,
            &node
        )),
        "{\"result\":null,\"error\":null,\"id\":\"s\"}\n"
    );
}

#[test]
fn getbestblockhash_answers_the_template_parent() {
    let node = bchn();
    assert_eq!(
        body_of(reply_to(br#"{"method":"getbestblockhash","id":2}"#, &node)),
        format!("{{\"result\":\"{BCHN_REGTEST_PARENT}\",\"error\":null,\"id\":2}}\n")
    );
}

#[test]
fn bchn_validates_a_p2pkh_cashaddr_in_its_field_order() {
    let payout = encode_cashaddr("bchreg", CashAddrType::PayToPublicKeyHash, &[0x11; 20]);
    let request = format!(r#"{{"method":"validateaddress","params":["{payout}"],"id":3}}"#);

    assert_eq!(
        body_of(reply_to(request.as_bytes(), &bchn())),
        format!(
            "{{\"result\":{{\"isvalid\":true,\"address\":\"{payout}\",\"scriptPubKey\":\"76a914{}88ac\",\"isscript\":false,\"istokenaware\":false}},\"error\":null,\"id\":3}}\n",
            "11".repeat(20)
        )
    );
}

#[test]
fn bchn_validates_a_p2sh_cashaddr_as_a_script() {
    let payout = encode_cashaddr("bchreg", CashAddrType::PayToScriptHash, &[0x22; 20]);

    let result = validate(&bchn(), &payout);

    assert_eq!(result["isvalid"], true);
    assert_eq!(result["isscript"], true);
    assert_eq!(result["scriptPubKey"], format!("a914{}87", "22".repeat(20)));
}

#[test]
fn bchn_takes_a_cashaddr_without_its_prefix() {
    let payout = encode_cashaddr("bchreg", CashAddrType::PayToPublicKeyHash, &[0x11; 20]);
    let bare = payout.trim_start_matches("bchreg:");

    let result = validate(&bchn(), bare);

    assert_eq!(result["isvalid"], true);
    assert_eq!(result["address"], payout.as_str());
}

#[test]
fn bchn_answers_an_uppercase_cashaddr_in_lowercase() {
    let payout = encode_cashaddr("bchreg", CashAddrType::PayToPublicKeyHash, &[0x11; 20]);

    let result = validate(&bchn(), &payout.to_ascii_uppercase());

    assert_eq!(result["address"], payout.as_str());
}

#[test]
fn bchn_refuses_a_bitcoin_address_with_isvalid_alone() {
    let result = validate(&bchn(), "bcrt1qqclp5usts33x0cgy2l5839659t7798w7g5a0fu");

    assert_eq!(result, serde_json::json!({"isvalid": false}));
}

#[test]
fn bchn_refuses_another_networks_cashaddr() {
    let chipnet = encode_cashaddr("bchtest", CashAddrType::PayToPublicKeyHash, &[0x11; 20]);

    assert_eq!(
        validate(&bchn(), &chipnet),
        serde_json::json!({"isvalid": false})
    );
}

#[test]
fn bchn_validateaddress_without_params_is_invalid() {
    assert_eq!(
        body_of(reply_to(br#"{"method":"validateaddress","id":4}"#, &bchn())),
        "{\"result\":{\"isvalid\":false},\"error\":null,\"id\":4}\n"
    );
}

#[test]
fn bitcoin_core_echoes_the_address_in_javascript_field_order() {
    assert_eq!(
        body_of(reply_to(
            br#"{"method":"validateaddress","params":["tb1qx"],"id":3}"#,
            &bitcoin_core()
        )),
        "{\"result\":{\"isvalid\":true,\"address\":\"tb1qx\",\"scriptPubKey\":\"\",\"isscript\":false,\"iswitness\":true},\"error\":null,\"id\":3}\n"
    );
}

#[test]
fn bitcoin_core_validateaddress_without_params_has_empty_address() {
    assert_eq!(
        body_of(reply_to(
            br#"{"method":"validateaddress","id":4}"#,
            &bitcoin_core()
        )),
        "{\"result\":{\"isvalid\":true,\"address\":\"\",\"scriptPubKey\":\"\",\"isscript\":false,\"iswitness\":true},\"error\":null,\"id\":4}\n"
    );
}

#[test]
fn unknown_method_is_method_not_found() {
    let node = bchn();
    assert_eq!(
        body_of(reply_to(br#"{"method":"stop","id":5}"#, &node)),
        "{\"result\":null,\"error\":{\"code\":-32601,\"message\":\"Method not found: stop\"},\"id\":5}\n"
    );
}

#[test]
fn request_without_id_gets_response_without_id() {
    let node = bchn();
    assert_eq!(
        body_of(reply_to(br#"{"method":"submitblock"}"#, &node)),
        "{\"result\":null,\"error\":null}\n"
    );
}

#[test]
fn missing_method_reads_undefined() {
    let node = bchn();
    assert_eq!(
        body_of(reply_to(br#"{"id":null}"#, &node)),
        "{\"result\":null,\"error\":{\"code\":-32601,\"message\":\"Method not found: undefined\"},\"id\":null}\n"
    );
}

#[test]
fn content_length_counts_the_trailing_newline() {
    let node = bchn();
    let reply = reply_to(br#"{"method":"getdifficulty","id":0}"#, &node);
    let Reply::Rpc(bytes) = reply else {
        panic!("expected an RPC reply");
    };
    let text = String::from_utf8(bytes).expect("utf-8 response");
    assert!(text.contains("Content-Length: 33\r\n"));
}
