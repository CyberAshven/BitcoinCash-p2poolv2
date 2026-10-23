use super::*;
use bitcoin::CompactTarget;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, header, method, path},
};

#[tokio::test]
async fn test_bitcoin_client() {
    // Start mock server
    let mock_server = MockServer::start().await;

    let auth_header = format!(
        "Basic {}",
        STANDARD.encode(format!("{}:{}", "testuser", "testpass"))
    );

    // Define expected request and response (JSON-RPC 1.0)
    Mock::given(method("POST"))
        .and(path("/"))
        .and(header("Authorization", auth_header))
        .and(body_json(serde_json::json!({
            "method": "test",
            "params": [],
            "id": 0
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": "test response",
            "error": null,
            "id": 0
        })))
        .mount(&mock_server)
        .await;

    let client = BitcoindRpcClient::new(&mock_server.uri(), "testuser", "testpass").unwrap();

    let params: Vec<serde_json::Value> = vec![];
    let result: String = client.request("test", params).await.unwrap();

    assert_eq!(result, "test response");
}

#[tokio::test]
async fn test_bitcoin_client_with_invalid_credentials() {
    let mock_server = MockServer::start().await;

    let client = BitcoindRpcClient::new(&mock_server.uri(), "invaliduser", "invalidpass").unwrap();
    let params: Vec<serde_json::Value> = vec![];
    let result: Result<String, BitcoindRpcError> = client.request("test", params).await;
    assert!(result.is_err());
}

#[tokio::test]
#[ignore] // Ignore by default since we only use it to test the connection to a locally running bitcoind
async fn test_bitcoin_client_real_connection() {
    let client = BitcoindRpcClient::new("http://localhost:38332", "p2pool", "p2pool").unwrap();

    let params: Vec<serde_json::Value> = vec![];
    let result: serde_json::Value = client.request("getblockchaininfo", params).await.unwrap();

    assert!(result.is_object());
    assert!(result.get("chain").is_some());
}

#[tokio::test]
async fn test_get_difficulty() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/"))
        .and(header("Authorization", "Basic cDJwb29sOnAycG9vbA=="))
        .and(body_json(serde_json::json!({
            "method": "getdifficulty",
            "params": [],
            "id": 0
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": 1234.56,
            "error": null,
            "id": 0
        })))
        .mount(&mock_server)
        .await;

    let client = BitcoindRpcClient::new(&mock_server.uri(), "p2pool", "p2pool").unwrap();
    let difficulty = client.get_difficulty().await.unwrap();

    assert_eq!(difficulty, 1234.56);
}

#[tokio::test]
async fn test_getblockchaininfo() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/"))
        .and(header("Authorization", "Basic cDJwb29sOnAycG9vbA=="))
        .and(body_json(serde_json::json!({
            "method": "getblockchaininfo",
            "params": [],
            "id": 0
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": {
                "initialblockdownload": true,
            },
            "error": null,
            "id": 0
        })))
        .mount(&mock_server)
        .await;

    let client = BitcoindRpcClient::new(&mock_server.uri(), "p2pool", "p2pool").unwrap();
    let info = client.getblockchaininfo().await.unwrap();

    assert_eq!(
        info,
        GetBlockchainInfo {
            initial_block_download: true,
        }
    );
}

/// BCHN's verbose header carries the ABLA state; the next block's size limit
/// is read from it, beyond 32 bits.
#[tokio::test]
async fn next_block_size_limit_comes_from_the_abla_state() {
    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_json(serde_json::json!({
            "method": "getblockheader",
            "params": ["00000000000000000000000000000000000000000000000000000000000000aa", true],
            "id": 0,
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": {
                "hash": "aa",
                "height": 900_000,
                "ablastate": {
                    "epsilon": 2_500_000_000_u64,
                    "beta": 2_500_000_000_u64,
                    "blocksize": 1_000,
                    "blocksizelimit": 5_000_000_000_u64,
                    "nextblocksizelimit": 5_000_000_123_u64,
                },
            },
            "error": null,
            "id": 0,
        })))
        .expect(1)
        .mount(&mock_server)
        .await;
    let client = BitcoindRpcClient::new(&mock_server.uri(), "p2pool", "p2pool").unwrap();
    let summary = client
        .get_block_header_summary(
            "00000000000000000000000000000000000000000000000000000000000000aa",
        )
        .await
        .unwrap();
    assert_eq!(
        summary,
        BlockHeaderSummary {
            height: 900_000,
            next_block_size_limit: Some(5_000_000_123),
        }
    );
}

/// A header without ABLA state still gives its height, and no limit.
#[tokio::test]
async fn block_height_comes_from_the_verbose_block_header() {
    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_json(serde_json::json!({
            "method": "getblockheader",
            "params": ["00000000000000000000000000000000000000000000000000000000000000aa", true],
            "id": 0,
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": {"hash": "aa", "height": 1234, "confirmations": 1},
            "error": null,
            "id": 0,
        })))
        .expect(1)
        .mount(&mock_server)
        .await;
    let client = BitcoindRpcClient::new(&mock_server.uri(), "p2pool", "p2pool").unwrap();
    let summary = client
        .get_block_header_summary(
            "00000000000000000000000000000000000000000000000000000000000000aa",
        )
        .await
        .unwrap();
    assert_eq!(
        summary,
        BlockHeaderSummary {
            height: 1234,
            next_block_size_limit: None,
        }
    );
}

#[tokio::test]
async fn an_unknown_block_header_is_an_error() {
    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": null,
            "error": {"code": -5, "message": "Block not found"},
            "id": 0,
        })))
        .mount(&mock_server)
        .await;
    let client = BitcoindRpcClient::new(&mock_server.uri(), "p2pool", "p2pool").unwrap();
    let result = client
        .get_block_header_summary(
            "00000000000000000000000000000000000000000000000000000000000000aa",
        )
        .await;
    assert!(matches!(
        result,
        Err(BitcoindRpcError::RpcError { code: -5, .. })
    ));
}

#[tokio::test]
async fn bch_template_request_has_no_segwit_rule() {
    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_json(serde_json::json!({
            "method": "getblocktemplate",
            "params": [{"capabilities": ["coinbasetxn", "coinbase/append", "workid"]}],
            "id": 0,
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": {"height": 326900, "transactions": []},
            "error": null,
            "id": 0,
        })))
        .expect(1)
        .mount(&mock_server)
        .await;
    let client = BitcoindRpcClient::new(&mock_server.uri(), "p2pool", "p2pool").unwrap();
    let template: serde_json::Value =
        serde_json::from_str(&client.get_bch_block_template().await.unwrap()).unwrap();
    assert_eq!(template["height"], 326900);
}

#[tokio::test]
async fn rpc_error_is_preserved_when_result_is_null() {
    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": null,
            "error": {"code": -28, "message": "Loading block index"},
            "id": 0,
        })))
        .expect(1)
        .mount(&mock_server)
        .await;
    let client = BitcoindRpcClient::new(&mock_server.uri(), "p2pool", "p2pool").unwrap();
    assert!(matches!(
        client.getblockchaininfo().await,
        Err(BitcoindRpcError::RpcError { code: -28, message })
            if message == "Loading block index"
    ));
}

#[tokio::test]
async fn test_getblocktemplate_mainnet() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
            .and(path("/"))
            .and(header("Authorization", "Basic cDJwb29sOnAycG9vbA=="))
            .and(body_json(serde_json::json!({
                "method": "getblocktemplate",
                "params": [{
                    "capabilities": ["coinbasetxn", "coinbase/append", "workid"],
                    "rules": ["segwit"],
                }],
                "id": 0
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "result": {
                    "version": 536870912,
                    "previousblockhash": "0000000000000000000b4d0b2e8e7e4e6b8e8e8e8e8e8e8e8e8e8e8e8e8e8e",
                    "transactions": [],
                    "coinbaseaux": {},
                    "coinbasevalue": 625000000,
                    "longpollid": "mockid",
                    "target": "0000000000000000000b4d0b2e8e7e4e6b8e8e8e8e8e8e8e8e8e8e8e8e8e8e",
                    "mintime": 1610000000,
                    "mutable": ["time", "transactions", "prevblock"],
                    "noncerange": "00000000ffffffff",
                    "sigoplimit": 80000,
                    "sizelimit": 4000000,
                    "curtime": 1610000000,
                    "bits": "170d6d54",
                    "height": 1000000,
                    "default_witness_commitment": "6a24aa21a9ed"
                },
                "error": null,
                "id": 0
            })))
            .mount(&mock_server)
            .await;

    let client = BitcoindRpcClient::new(&mock_server.uri(), "p2pool", "p2pool").unwrap();
    let result = client.getblocktemplate(bitcoin::Network::Bitcoin).await;
    let result = result.unwrap();

    let result = serde_json::from_str::<serde_json::Value>(&result).unwrap();

    assert!(result.get("version").is_some());
    assert_eq!(result.get("height").unwrap(), 1000000);
}

#[tokio::test]
async fn test_getblocktemplate_signet() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
            .and(path("/"))
            .and(header("Authorization", "Basic cDJwb29sOnAycG9vbA=="))
            .and(body_json(serde_json::json!({
                "method": "getblocktemplate",
                "params": [{
                    "capabilities": ["coinbasetxn", "coinbase/append", "workid"],
                    "rules": ["segwit", "signet"],
                }],
                "id": 0
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "result": {
                    "version": 536870912,
                    "previousblockhash": "0000000000000000000b4d0b2e8e7e4e6b8e8e8e8e8e8e8e8e8e8e8e8e8e8e",
                    "transactions": [],
                    "coinbaseaux": {},
                    "coinbasevalue": 625000000,
                    "longpollid": "mockid",
                    "target": "0000000000000000000b4d0b2e8e7e4e6b8e8e8e8e8e8e8e8e8e8e8e8e8e8e",
                    "mintime": 1610000000,
                    "mutable": ["time", "transactions", "prevblock"],
                    "noncerange": "00000000ffffffff",
                    "sigoplimit": 80000,
                    "sizelimit": 4000000,
                    "curtime": 1610000000,
                    "bits": "170d6d54",
                    "height": 2000000,
                    "default_witness_commitment": "6a24aa21a9ed"
                },
                "error": null,
                "id": 0
            })))
            .mount(&mock_server)
            .await;

    let client = BitcoindRpcClient::new(&mock_server.uri(), "p2pool", "p2pool").unwrap();
    let result = client
        .getblocktemplate(bitcoin::Network::Signet)
        .await
        .unwrap();

    let result = serde_json::from_str::<serde_json::Value>(&result).unwrap();

    assert!(result.get("version").is_some());
    assert_eq!(result.get("height").unwrap(), 2000000);
}

#[tokio::test]
async fn test_decoderawtransaction() {
    let mock_server = MockServer::start().await;
    let tx_hex = "0100000001000000000000000000000000000000000000000000000000000000\
                  0000000000ffffffff1c02fa01010004bdaf326804554ce1370c0101010101\
                  01010101010101ffffffff0300e1f50500000000160014fd8b1a0b2a4c387d\
                  0a418969c62f2812c76ee45d0011102401000000160014ca81d03f2707c355\
                  502622c7db77fdf79546926e0000000000000000266a24aa21a9eddd9e37e4\
                  20b1b58781dada016dfa5812f62133a381e1a58e83389735b2330ef700000000";
    let tx = bitcoin::consensus::encode::deserialize::<bitcoin::Transaction>(
        hex::decode(tx_hex).unwrap().as_slice(),
    )
    .unwrap();

    // Setup mock for decoderawtransaction (JSON-RPC 1.0)
    Mock::given(method("POST"))
        .and(path("/"))
        .and(header("Authorization", "Basic cDJwb29sOnAycG9vbA=="))
        .and(body_json(serde_json::json!({
            "method": "decoderawtransaction",
            "params": [tx_hex],
            "id": 0
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": serde_json::to_value(tx.clone()).unwrap(),
            "error": null,
            "id": 0
        })))
        .mount(&mock_server)
        .await;

    let client = BitcoindRpcClient::new(&mock_server.uri(), "p2pool", "p2pool").unwrap();

    let decoded_tx = client.decoderawtransaction(&tx).await.unwrap();

    assert_eq!(decoded_tx, tx);
}

#[tokio::test]
async fn test_submit_block() {
    let mock_server = MockServer::start().await;

    // Create a simple block
    let block = bitcoin::Block {
        header: bitcoin::blockdata::block::Header {
            version: bitcoin::blockdata::block::Version::from_consensus(1),
            prev_blockhash: "5e9a183768460fbf56eab199a66057375b424bdca195e7ecc808374365a7ea67"
                .parse()
                .unwrap(),
            merkle_root: "277c298e9f1254a59411cfc29f1a88ec6ee12cf4c955044d8bb8a7242cfed919"
                .parse()
                .unwrap(),
            time: 1610000000,
            bits: CompactTarget::from_consensus(503543726),
            nonce: 12345,
        },
        txdata: vec![],
    };

    let block_hex = serialize_hex(&block);

    // Setup mock for submitblock (JSON-RPC 1.0)
    Mock::given(method("POST"))
        .and(path("/"))
        .and(header("Authorization", "Basic cDJwb29sOnAycG9vbA=="))
        .and(body_json(serde_json::json!({
            "method": "submitblock",
            "params": [block_hex],
            "id": 0
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": null,  // null indicates success in Bitcoin Core
            "error": null,
            "id": 0
        })))
        .mount(&mock_server)
        .await;

    let client = BitcoindRpcClient::new(&mock_server.uri(), "p2pool", "p2pool").unwrap();

    let result = client.submit_block(&block).await.unwrap();
    assert_eq!(result, "null"); // Successful submission returns null
}

#[tokio::test]
async fn block_proposal_accepts_only_null_and_sends_proposal_mode() {
    let server = MockServer::start().await;
    let block = bitcoin::blockdata::constants::genesis_block(bitcoin::Network::Regtest);
    Mock::given(method("POST"))
            .and(body_json(serde_json::json!({"method": "getblocktemplate", "params": [{"mode": "proposal", "data": serialize_hex(&block)}], "id": 0})))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"result": null, "error": null, "id": 0})))
            .expect(1)
            .mount(&server).await;
    let client = BitcoindRpcClient::new(&server.uri(), "user", "pass").unwrap();
    client.validate_block_proposal(&block).await.unwrap();
}

#[tokio::test]
async fn block_proposal_preserves_node_rejection_reason() {
    let server = MockServer::start().await;
    let block = bitcoin::blockdata::constants::genesis_block(bitcoin::Network::Regtest);
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"result": "bad-cb-amount", "error": null, "id": 0}),
            ),
        )
        .expect(1)
        .mount(&server)
        .await;
    let client = BitcoindRpcClient::new(&server.uri(), "user", "pass").unwrap();
    assert!(
        client
            .validate_block_proposal(&block)
            .await
            .unwrap_err()
            .to_string()
            .contains("bad-cb-amount")
    );
}

#[tokio::test]
async fn block_proposal_does_not_accept_false_as_success() {
    let server = MockServer::start().await;
    let block = bitcoin::blockdata::constants::genesis_block(bitcoin::Network::Regtest);
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"result": false, "error": null, "id": 0})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let client = BitcoindRpcClient::new(&server.uri(), "user", "pass").unwrap();
    assert!(client.validate_block_proposal(&block).await.is_err());
}

#[tokio::test]
async fn test_getblocktemplate_retry_logic() {
    let mock_server = MockServer::start().await;
    let auth_header = "Basic cDJwb29sOnAycG9vbA==";

    // Mock 3 failed responses followed by a successful one (JSON-RPC 1.0)
    // First 3 calls fail
    for i in 0..3 {
        Mock::given(method("POST"))
            .and(path("/"))
            .and(header("Authorization", auth_header))
            .and(body_json(serde_json::json!({
                "method": "getblocktemplate",
                "params": [{
                    "capabilities": ["coinbasetxn", "coinbase/append", "workid"],
                    "rules": ["segwit"],
                }],
                "id": i
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "result": null,
                "error": {
                    "code": -1,
                    "message": format!("Failed attempt {}", i)
                },
                "id": i
            })))
            .expect(1) // Each mock should be called exactly once
            .mount(&mock_server)
            .await;
    }

    // Fourth call succeeds
    Mock::given(method("POST"))
            .and(path("/"))
            .and(header("Authorization", auth_header))
            .and(body_json(serde_json::json!({
                "method": "getblocktemplate",
                "params": [{
                    "capabilities": ["coinbasetxn", "coinbase/append", "workid"],
                    "rules": ["segwit"],
                }],
                "id": 3
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "result": {
                    "version": 536870912,
                    "height": 1000000,
                    "previousblockhash": "0000000000000000000b4d0b2e8e7e4e6b8e8e8e8e8e8e8e8e8e8e8e8e8e8e",
                    "bits": "1a01f56e"
                },
                "error": null,
                "id": 3
            })))
            .expect(1)
            .mount(&mock_server)
            .await;

    let client = BitcoindRpcClient::new(&mock_server.uri(), "p2pool", "p2pool").unwrap();
    let result = client.getblocktemplate(bitcoin::Network::Bitcoin).await;

    assert!(result.is_ok());
    let result_value = serde_json::from_str::<serde_json::Value>(&result.unwrap()).unwrap();
    assert_eq!(result_value.get("height").unwrap(), 1000000);
}

#[tokio::test]
async fn test_request_with_4xx_http_error() {
    let mock_server = MockServer::start().await;

    // Mock a 401 Unauthorized response
    Mock::given(method("POST"))
        .and(path("/"))
        .and(header("Authorization", "Basic cDJwb29sOnAycG9vbA=="))
        .and(body_json(serde_json::json!({
            "method": "getdifficulty",
            "params": [],
            "id": 0
        })))
        .respond_with(ResponseTemplate::new(401).set_body_string("Unauthorized"))
        .mount(&mock_server)
        .await;

    let client = BitcoindRpcClient::new(&mock_server.uri(), "p2pool", "p2pool").unwrap();
    let result: Result<f64, BitcoindRpcError> = client.get_difficulty().await;

    assert!(result.is_err());
    if let Err(BitcoindRpcError::HttpError {
        status_code,
        message,
    }) = result
    {
        assert_eq!(status_code, 401);
        assert_eq!(message, "Unauthorized");
    } else {
        panic!("Expected BitcoindRpcError::HttpError, got {result:?}");
    }
}
