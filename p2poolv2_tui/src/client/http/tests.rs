// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[test]
fn test_display_url_drops_a_login() {
    assert_eq!(
        display_url("http://p2pool:secret@127.0.0.1:48332/wallet"),
        "http://127.0.0.1:48332/wallet"
    );
}

#[test]
fn test_display_url_keeps_a_url_without_a_login() {
    assert_eq!(
        display_url("https://node.example:48332"),
        "https://node.example:48332"
    );
}

#[test]
fn test_display_url_ignores_an_at_in_the_path() {
    assert_eq!(
        display_url("http://127.0.0.1:46884/miner?address=a@b"),
        "http://127.0.0.1:46884/miner?address=a@b"
    );
}

#[test]
fn test_url_component_keeps_cashaddr_colons() {
    assert_eq!(url_component("bchtest:qq9 x/y"), "bchtest:qq9%20x%2Fy");
}

#[tokio::test]
async fn test_read_capped_reads_a_short_body() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("hello"))
        .mount(&server)
        .await;
    let response = reqwest::get(server.uri()).await.unwrap();
    assert_eq!(read_capped(response, 16).await.unwrap(), b"hello");
}

#[tokio::test]
async fn test_read_capped_refuses_a_long_body() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("x".repeat(64)))
        .mount(&server)
        .await;
    let response = reqwest::get(server.uri()).await.unwrap();
    assert_eq!(
        read_capped(response, 16).await.unwrap_err(),
        "the answer is longer than 16 bytes"
    );
}

#[tokio::test]
async fn test_describe_leaves_out_the_url() {
    let error = reqwest::get("http://user:secret@127.0.0.1:1/x")
        .await
        .unwrap_err();
    let message = describe(error);
    assert!(!message.contains("secret"), "{message}");
    assert!(!message.contains("/x"), "{message}");
}
