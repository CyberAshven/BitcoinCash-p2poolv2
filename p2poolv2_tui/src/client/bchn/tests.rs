// SPDX-FileCopyrightText: 2024 PDM Authors
// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//

use super::*;
use wiremock::matchers::{body_partial_json, header, method};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn test_status_reads_all_five_calls() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_partial_json(json!({"method": "getblockchaininfo"})))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"result":{"chain":"chip","blocks":10,"headers":10,"bestblockhash":"00ab",
                "difficulty":2.0},"error":null,"id":"p2poolv2_tui"}"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(body_partial_json(json!({"method": "getnetworkinfo"})))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"result":{"subversion":"/Bitcoin Cash Node:28.0.1/","protocolversion":70016,
                "connections":3},"error":null}"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(body_partial_json(json!({"method": "getmempoolinfo"})))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"result":{"size":1,"bytes":2,"usage":3,"maxmempool":4,
                "mempoolminfee":0.00001},"error":null}"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(body_partial_json(json!({"method": "getpeerinfo"})))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(
                r#"{"result":[{"id":1,"addr":"203.0.113.5:48333"}],"error":null}"#,
            ),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(body_partial_json(json!({"method": "getmininginfo"})))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"result":{"networkhashps":5.0e12},"error":null}"#),
        )
        .mount(&server)
        .await;
    let client = BchnClient::new(&server.uri(), "p2pool", "pw").unwrap();
    let status = client.status().await.unwrap();
    assert_eq!(status.chain.chain, "chip");
    assert_eq!(status.network.connections, 3);
    assert_eq!(status.mempool.size, 1);
    assert_eq!(status.peers[0].address, "203.0.113.5:48333");
    assert_eq!(status.network_hashrate, Some(5.0e12));
}

#[tokio::test]
async fn test_calls_carry_the_rpc_login() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(header("authorization", "Basic cDJwb29sOnB3"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"result":7,"error":null}"#))
        .mount(&server)
        .await;
    let client = BchnClient::new(&server.uri(), "p2pool", "pw").unwrap();
    assert_eq!(client.call::<u64>("getblockcount").await.unwrap(), 7);
}

#[tokio::test]
async fn test_an_rpc_error_is_reported() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500).set_body_string(
            r#"{"result":null,"error":{"code":-28,"message":"Loading block index..."}}"#,
        ))
        .mount(&server)
        .await;
    let client = BchnClient::new(&server.uri(), "p2pool", "pw").unwrap();
    assert_eq!(
        client.call::<u64>("getblockcount").await.unwrap_err(),
        "BCHN getblockcount failed: Loading block index... (code -28)"
    );
}

#[tokio::test]
async fn test_a_refused_login_does_not_show_the_password() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let client = BchnClient::new(&server.uri(), "p2pool", "hunter2").unwrap();
    let error = client.status().await.unwrap_err();
    assert!(
        error.starts_with("BCHN refused the RPC login (HTTP 401)"),
        "{error}"
    );
    assert!(!error.contains("hunter2"), "{error}");
}

#[tokio::test]
async fn test_status_keeps_at_most_max_peers() {
    let server = MockServer::start().await;
    let peer = r#"{"id":1,"addr":"203.0.113.5:48333"}"#;
    let peers = format!(
        r#"{{"result":[{}],"error":null}}"#,
        vec![peer; MAX_BCHN_PEERS + 10].join(",")
    );
    Mock::given(method("POST"))
        .and(body_partial_json(json!({"method": "getpeerinfo"})))
        .respond_with(ResponseTemplate::new(200).set_body_string(peers))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(body_partial_json(json!({"method": "getblockchaininfo"})))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"result":{"chain":"main","blocks":1,"headers":1,"bestblockhash":"00",
                "difficulty":1.0},"error":null}"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(body_partial_json(json!({"method": "getnetworkinfo"})))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"result":{"subversion":"/BCHN/","protocolversion":1,"connections":0},
                "error":null}"#,
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(body_partial_json(json!({"method": "getmempoolinfo"})))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"result":{"size":0,"bytes":0,"usage":0,"maxmempool":0,"mempoolminfee":0.0},
                "error":null}"#,
        ))
        .mount(&server)
        .await;
    let client = BchnClient::new(&server.uri(), "p2pool", "pw").unwrap();
    let status = client.status().await.unwrap();
    assert_eq!(status.peers.len(), MAX_BCHN_PEERS);
    assert_eq!(status.network_hashrate, None);
}

#[test]
fn test_debug_hides_the_password() {
    let client = BchnClient::new("http://127.0.0.1:48332", "p2pool", "hunter2").unwrap();
    let shown = format!("{client:?}");
    assert!(!shown.contains("hunter2"), "{shown}");
}
