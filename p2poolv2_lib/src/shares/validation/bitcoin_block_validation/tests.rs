use super::*;
use base64::Engine;
use bitcoin::consensus::Decodable;
use bitcoindrpc::{BitcoinRpcConfig, BitcoindRpcClient};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, header, method, path},
};

#[tokio::test]
async fn test_validate_bitcoin_block_success() {
    // Start mock server
    let mock_server = MockServer::start().await;
    let block_hex_string =
        include_str!("../../../../../p2poolv2_tests/test_data/seralized/block_1.txt");
    let block_hex = hex::decode(block_hex_string).unwrap();
    let block = bitcoin::Block::consensus_decode(&mut block_hex.as_slice()).unwrap();

    // Set up mock auth
    let auth_header = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", "testuser", "testpass"))
    );

    // Set up expected request/response (JSON-RPC 1.0)
    Mock::given(method("POST"))
        .and(path("/"))
        .and(header("Authorization", auth_header))
        .and(body_json(serde_json::json!({
            "id": 0,
            "method": "getblocktemplate",
            "params": [{
                "mode": "proposal",
                "data": block_hex_string
            }],
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": "duplicate",
            "error": null,
            "id": 0
        })))
        .mount(&mock_server)
        .await;

    // Create test config
    let config = BitcoinRpcConfig {
        url: mock_server.uri(),
        username: "testuser".to_string(),
        password: "testpass".to_string(),
    };

    // Test validation
    let client = BitcoindRpcClient::new(&config.url, &config.username, &config.password).unwrap();
    let result = validate_bitcoin_block(&block, &client).await;
    assert!(result.is_ok());
    assert!(result.unwrap());
}

#[tokio::test]
async fn test_validate_bitcoin_block_reject() {
    // Start mock server
    let mock_server = MockServer::start().await;
    let block_hex_string =
        include_str!("../../../../../p2poolv2_tests/test_data/seralized/block_1.txt");
    let block_hex = hex::decode(block_hex_string).unwrap();
    let block = bitcoin::Block::consensus_decode(&mut block_hex.as_slice()).unwrap();

    // Set up mock auth
    let auth_header = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", "testuser", "testpass"))
    );

    // Set up expected request/response (JSON-RPC 1.0)
    Mock::given(method("POST"))
        .and(path("/"))
        .and(header("Authorization", auth_header))
        .and(body_json(serde_json::json!({
            "id": 0,
            "method": "getblocktemplate",
            "params": [{
                "mode": "proposal",
                "data": block_hex_string
            }],
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": "rejected",
            "error": null,
            "id": 0
        })))
        .mount(&mock_server)
        .await;

    // Create test config
    let config = BitcoinRpcConfig {
        url: mock_server.uri(),
        username: "testuser".to_string(),
        password: "testpass".to_string(),
    };

    // Test validation
    let client = BitcoindRpcClient::new(&config.url, &config.username, &config.password).unwrap();
    let result = validate_bitcoin_block(&block, &client).await;
    assert!(result.is_ok());
    assert!(!result.unwrap());
}

#[tokio::test]
async fn test_validate_bitcoin_block_http_error() {
    // Start mock server
    let mock_server = MockServer::start().await;
    let block_hex_string =
        include_str!("../../../../../p2poolv2_tests/test_data/seralized/block_1.txt");
    let block_hex = hex::decode(block_hex_string).unwrap();
    let block = bitcoin::Block::consensus_decode(&mut block_hex.as_slice()).unwrap();

    // Set up mock auth
    let auth_header = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", "testuser", "testpass"))
    );

    // Set up expected request/response with HTTP 500 error
    Mock::given(method("POST"))
        .and(path("/"))
        .and(header("Authorization", auth_header))
        .and(body_json(serde_json::json!({
            "id": 0,
            "method": "getblocktemplate",
            "params": [{
                "mode": "proposal",
                "data": block_hex_string
            }],
        })))
        .respond_with(ResponseTemplate::new(500))
        .mount(&mock_server)
        .await;

    // Create test config
    let config = BitcoinRpcConfig {
        url: mock_server.uri(),
        username: "testuser".to_string(),
        password: "testpass".to_string(),
    };

    // Test validation
    let client = BitcoindRpcClient::new(&config.url, &config.username, &config.password).unwrap();
    let result = validate_bitcoin_block(&block, &client).await;
    assert!(result.is_err());
}
