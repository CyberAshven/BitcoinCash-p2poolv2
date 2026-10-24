// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The mock bitcoind over a real socket, as CKPool and the node talk to it.

use p2poolv2_tools::mock_bitcoind::{MockNode, NodeKind, serve};
use serde_json::Value;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

async fn start_server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("local address");
    let node = Arc::new(MockNode::load(NodeKind::default(), None).expect("BCHN regtest mock"));
    tokio::spawn(serve(listener, node, 8));
    address
}

/// Send `request` and read until the server closes the connection.
async fn exchange(address: SocketAddr, request: &[u8]) -> String {
    let mut stream = TcpStream::connect(address).await.expect("connect");
    stream.write_all(request).await.expect("send");
    let mut response = Vec::with_capacity(4096);
    stream.read_to_end(&mut response).await.expect("receive");
    String::from_utf8(response).expect("utf-8 response")
}

fn body_json(response: &str) -> Value {
    let (_, body) = response.split_once("\r\n\r\n").expect("header end");
    serde_json::from_str(body).expect("JSON body")
}

fn post(body: &str) -> Vec<u8> {
    format!(
        "POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Basic dXNlcjpwYXNz\r\ncontent-type: text/plain;\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

#[tokio::test]
async fn getblocktemplate_serves_the_easy_template() {
    let address = start_server().await;
    let response = exchange(
        address,
        &post(r#"{"method":"getblocktemplate","params":[],"id":2}"#),
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n"));
    let json = body_json(&response);
    assert_eq!(json["id"], 2);
    assert_eq!(json["error"], Value::Null);
    assert_eq!(json["result"]["bits"], "2100ffff");
    assert_eq!(json["result"]["height"], 102);
}

#[tokio::test]
async fn lf_only_request_is_served() {
    let address = start_server().await;
    let body = r#"{"method":"getdifficulty","params":[],"id":0}"#;
    let request = format!("POST / HTTP/1.1\nContent-Length: {}\n\n{body}", body.len());
    let response = exchange(address, request.as_bytes()).await;
    assert!(response.ends_with("\r\n\r\n{\"result\":1,\"error\":null,\"id\":0}\n"));
}

#[tokio::test]
async fn request_split_across_writes_is_served() {
    let address = start_server().await;
    let request = post(r#"{"method":"submitblock","params":["00"],"id":3}"#);
    let mut stream = TcpStream::connect(address).await.expect("connect");
    stream
        .write_all(&request[..20])
        .await
        .expect("send first part");
    stream.flush().await.expect("flush");
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    stream.write_all(&request[20..]).await.expect("send rest");
    let mut response = Vec::with_capacity(512);
    stream.read_to_end(&mut response).await.expect("receive");
    let response = String::from_utf8(response).expect("utf-8 response");
    assert_eq!(
        body_json(&response),
        serde_json::json!({"result": null, "error": null, "id": 3})
    );
}

#[tokio::test]
async fn invalid_json_gets_400() {
    let address = start_server().await;
    let response = exchange(address, &post("{not json")).await;
    assert_eq!(
        response,
        "HTTP/1.1 400 Bad Request\r\nConnection: close\r\nContent-Length: 12\r\n\r\nInvalid JSON"
    );
}

#[tokio::test]
async fn validateaddress_answers_as_bchn() {
    let address = start_server().await;
    let payout = p2poolv2_wallet::bch_share::encode_cashaddr(
        "bchreg",
        p2poolv2_wallet::bch_share::CashAddrType::PayToPublicKeyHash,
        &[0x11; 20],
    );
    let response = exchange(
        address,
        &post(&format!(
            r#"{{"method":"validateaddress","params":["{payout}"],"id":4}}"#
        )),
    )
    .await;
    let json = body_json(&response);
    assert_eq!(json["result"]["isvalid"], true);
    assert_eq!(json["result"]["address"], payout.as_str());
    assert_eq!(json["result"]["isscript"], false);
    assert!(json["result"].get("iswitness").is_none());
}
