use super::*;
use axum::extract::State;

#[test]
fn test_work_to_f64() {
    // Little-endian bytes for 256 (0x0100).
    let mut le = [0u8; 32];
    le[1] = 1;
    assert_eq!(work_to_f64(bitcoin::Work::from_le_bytes(le)), 256.0);

    // 65536 (0x010000) is larger.
    let mut le_larger = [0u8; 32];
    le_larger[2] = 1;
    let larger = bitcoin::Work::from_le_bytes(le_larger);
    assert_eq!(work_to_f64(larger), 65536.0);
    assert!(work_to_f64(larger) > work_to_f64(bitcoin::Work::from_le_bytes(le)));
}

use base64::Engine;
use bitcoin::blockdata::constants::genesis_block;
use bitcoin::{Amount, Network, TxOut};
use p2poolv2_lib::accounting::stats::metrics;
use p2poolv2_lib::monitoring_events::create_monitoring_event_channel;
use p2poolv2_lib::shares::share_block::ShareBlock;
use p2poolv2_lib::stratum::work::block_template::BlockTemplate;
use p2poolv2_lib::stratum::work::coinbase::parse_address;
use p2poolv2_lib::stratum::work::tracker::start_tracker_actor;
use p2poolv2_lib::test_utils::{TestShareBlockBuilder, setup_test_chain_store_handle};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

/// Helper to build an AppState with the given NodeHandle for endpoint tests.
async fn build_test_state(node_handle: NodeHandle) -> (Arc<AppState>, tempfile::TempDir) {
    let (chain_store_handle, temp_dir) = setup_test_chain_store_handle(true).await;
    let metrics_temp = tempfile::tempdir().unwrap();
    let metrics_handle = metrics::start_metrics(metrics_temp.path().to_str().unwrap().to_string())
        .await
        .unwrap();
    let tracker_handle = start_tracker_actor();
    let state = Arc::new(AppState {
        app_config: AppConfig {
            pool_signature_length: 0,
            network: bitcoin::Network::Signet,
            cash_network: None,
            cors_allowed: false,
        },
        chain_store_handle,
        metrics_handle,
        tracker_handle,
        node_handle,
        monitoring_event_sender: create_monitoring_event_channel().0,
        auth_user: None,
        auth_token: None,
        pplns_window: None,
    });
    (state, temp_dir)
}

#[tokio::test]
async fn test_metrics_endpoint_includes_p2p_health() {
    let node_handle = NodeHandle::new_for_test();
    let (state, _temp_dir) = build_test_state(node_handle).await;

    let exposition = metrics(State(state)).await;

    assert!(exposition.contains("# TYPE p2p_connected_peers gauge\np2p_connected_peers 0\n"));
    assert!(exposition.contains("p2p_connections_closed_unresponsive_total 0\n"));
}

#[test]
fn test_found_blocks_exposition_lists_each_block() {
    let found_blocks = vec![FoundBlock {
        bitcoin_height: 840000,
        bitcoin_blockhash: bitcoin::BlockHash::from_str(
            "0000000000000000000320283a032748cef8227873ff4872689bf23f1cda83a5",
        )
        .unwrap(),
        share_blockhash: bitcoin::BlockHash::from_str(
            "00000000000000000001b4e3a8b6c1d2e3f405162738495a6b7c8d9e0f1a2b3c",
        )
        .unwrap(),
        bitcoin_time: 1713571767,
        miner_bitcoin_address: "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh".to_string(),
    }];

    let exposition = found_blocks_exposition(&found_blocks, Network::Bitcoin);

    assert!(exposition.contains("# TYPE found_blocks_total counter\nfound_blocks_total 1\n"));
    assert!(exposition.contains("# TYPE found_block_time_seconds gauge\n"));
    assert!(exposition.contains(
            "found_block_time_seconds{blockhash=\"0000000000000000000320283a032748cef8227873ff4872689bf23f1cda83a5\",height=\"840000\",miner=\"bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh\",network=\"main\"} 1713571767\n"
        ));
}

#[test]
fn test_found_blocks_exposition_with_no_blocks() {
    let exposition = found_blocks_exposition(&[], Network::Testnet4);

    assert!(exposition.contains("found_blocks_total 0\n"));
    assert!(!exposition.contains("found_block_time_seconds{"));
}

#[tokio::test]
async fn test_metrics_endpoint_exposes_found_blocks() {
    let node_handle = NodeHandle::new_for_test();
    let (state, _temp_dir) = build_test_state(node_handle).await;
    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    state
        .chain_store_handle
        .init_or_setup_genesis(genesis.clone())
        .await
        .unwrap();

    // A header meeting its claimed target is only a candidate until BCHN
    // acceptance has been recorded.
    let mut share = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    share.header.bitcoin_header = genesis_block(Network::Regtest).header;
    state
        .chain_store_handle
        .add_share_block_and_organise_header(share.clone())
        .await
        .unwrap();
    state
        .chain_store_handle
        .mark_block_valid(share.block_hash())
        .await
        .unwrap();
    state.chain_store_handle.organise_block().await.unwrap();

    let unverified = metrics(State(state.clone())).await;
    assert!(unverified.contains("found_blocks_total 0\n"));
    assert!(
        state
            .chain_store_handle
            .get_found_blocks()
            .unwrap()
            .is_empty()
    );
    let candidate = state
        .chain_store_handle
        .get_candidate_found_blocks()
        .unwrap();
    assert_eq!(candidate.len(), 1);
    state
        .chain_store_handle
        .record_accepted_found_block(candidate[0].clone())
        .await
        .unwrap();
    let exposition = metrics(State(state)).await;

    assert!(exposition.contains("found_blocks_total 1\n"));
    assert!(exposition.contains(&format!(
            "found_block_time_seconds{{blockhash=\"{}\",height=\"{}\",miner=\"{}\",network=\"signet\"}} {}\n",
            share.header.bitcoin_header.block_hash(),
            share.header.bitcoin_height,
            share.header.miner_bitcoin_address,
            share.header.bitcoin_header.time
        )));
}

#[tokio::test]
async fn test_peers_endpoint_returns_empty_list() {
    let node_handle = NodeHandle::new_for_test();
    let (state, _temp_dir) = build_test_state(node_handle).await;

    let response = peers(State(state)).await.unwrap();
    assert!(response.0.is_empty());
}

#[tokio::test]
async fn test_peers_endpoint_returns_peer_list() {
    let (node_handle, expected_peer_ids) = NodeHandle::new_for_test_with_peer_count(3);
    let (state, _temp_dir) = build_test_state(node_handle).await;

    let response = peers(State(state)).await.unwrap();
    let peer_responses = &response.0;

    assert_eq!(peer_responses.len(), 3);

    let returned_ids: Vec<&str> = peer_responses
        .iter()
        .map(|peer| peer.peer_id.as_str())
        .collect();
    for expected_id in &expected_peer_ids {
        assert!(
            returned_ids.contains(&expected_id.as_str()),
            "Expected peer ID {expected_id} not found in response"
        );
    }
}

#[tokio::test]
async fn test_metrics_endpoint_exposes_coinbase_split() {
    let tracker_handle = start_tracker_actor();

    let temp_dir = tempfile::tempdir().unwrap();
    let metrics_handle = metrics::start_metrics(temp_dir.path().to_str().unwrap().to_string())
        .await
        .unwrap();

    //  Use Signet Addresses
    let address = parse_address(
        "tb1q3udk7r26qs32ltf9nmqrjaaa7tr55qmkk30q5d",
        Network::Signet,
    )
    .unwrap();

    let donation_address = parse_address(
        "tb1q0afww6y0kgl4tyjjyv6xlttvfwdfqxvrfzz35f",
        Network::Signet,
    )
    .unwrap();

    let template = BlockTemplate {
        default_witness_commitment: Some(
            "6a24aa21a9ed0100000000000000000000000000000000000000000000000000000000000000"
                .to_string(),
        ),
        height: 100,
        version: 0x20000000,
        previousblockhash: "0000000000000000000000000000000000000000000000000000000000000000"
            .to_string(),
        bits: "1d00ffff".to_string(),
        curtime: 1234567890,
        transactions: vec![],
        coinbasevalue: 50_0000_0000,
        coinbaseaux: HashMap::new(),
        rules: vec![],
        vbavailable: HashMap::new(),
        vbrequired: 0,
        longpollid: "".to_string(),
        target: "".to_string(),
        mintime: 0,
        mutable: vec![],
        noncerange: "".to_string(),
        sigoplimit: 0,
        sizelimit: 0,
        weightlimit: 0,
        provided: None,
    };

    let pool_signature = b"P2Poolv2";

    // Build outputs
    let outputs = vec![
        TxOut {
            value: Amount::from_str("49 BTC").unwrap(),
            script_pubkey: address.script_pubkey(),
        },
        TxOut {
            value: Amount::from_str("1 BTC").unwrap(),
            script_pubkey: donation_address.script_pubkey(),
        },
    ];

    // Manually construct coinbase2 hex matching the new scriptSig layout:
    // [nsecs_push][pool_sig_push][sequence][outputs][locktime]
    // No commitment hash push since share_commitment is None.
    let mut coinbase2_bytes = Vec::new();
    coinbase2_bytes.push(0x08); // push 8 bytes for nsecs
    coinbase2_bytes.extend_from_slice(&[0u8; 8]); // dummy nsecs
    coinbase2_bytes.push(pool_signature.len() as u8);
    coinbase2_bytes.extend_from_slice(pool_signature);
    coinbase2_bytes.extend_from_slice(&[0xff, 0xff, 0xff, 0xff]); // Sequence
    coinbase2_bytes.extend_from_slice(&bitcoin::consensus::serialize(&outputs)); // Outputs
    coinbase2_bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // LockTime

    let coinbase2_hex = hex::encode(coinbase2_bytes);

    //  Insert Job into Tracker
    let job_id = tracker_handle.get_next_job_id();
    tracker_handle.insert_job(
        Arc::new(template),
        "".to_string(),
        coinbase2_hex,
        None,
        p2poolv2_lib::test_utils::TEST_COINBASE_NSECS,
        vec![],
        job_id,
    );

    //  Mock ChainStore
    let _genesis = ShareBlock::build_genesis_for_network(Network::Signet);
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;

    //  Prepare AppState
    let node_handle = NodeHandle::new_for_test();
    let state = Arc::new(AppState {
        app_config: AppConfig {
            pool_signature_length: 8,
            network: bitcoin::Network::Signet,
            cash_network: None,
            cors_allowed: false,
        },
        chain_store_handle,
        metrics_handle,
        tracker_handle,
        node_handle,
        monitoring_event_sender: create_monitoring_event_channel().0,
        auth_user: None,
        auth_token: None,
        pplns_window: None,
    });

    let response_body = metrics(State(state)).await;

    println!("{response_body}");

    //  Verify Output
    assert!(response_body.contains(
            "coinbase_output{index=\"0\",address=\"tb1q3udk7r26qs32ltf9nmqrjaaa7tr55qmkk30q5d\"} 4900000000"
        ));
    assert!(response_body.contains(
            "coinbase_output{index=\"1\",address=\"tb1q0afww6y0kgl4tyjjyv6xlttvfwdfqxvrfzz35f\"} 100000000"
        ));
}

#[tokio::test]
async fn test_pplns_shares_rejects_limit_zero() {
    let node_handle = NodeHandle::new_for_test();
    let (state, _temp_dir) = build_test_state(node_handle).await;

    let query = Query(PplnsQuery {
        limit: Some(0),
        start_time: None,
        end_time: None,
    });

    let result = pplns_shares(State(state), query).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_pplns_shares_rejects_limit_above_max() {
    let node_handle = NodeHandle::new_for_test();
    let (state, _temp_dir) = build_test_state(node_handle).await;

    let query = Query(PplnsQuery {
        limit: Some(1001),
        start_time: None,
        end_time: None,
    });

    let result = pplns_shares(State(state), query).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_pplns_shares_rejects_end_before_start() {
    let node_handle = NodeHandle::new_for_test();
    let (state, _temp_dir) = build_test_state(node_handle).await;

    let query = Query(PplnsQuery {
        limit: None,
        start_time: Some("2025-01-02T00:00:00Z".to_string()),
        end_time: Some("2025-01-01T00:00:00Z".to_string()),
    });

    let result = pplns_shares(State(state), query).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_pplns_shares_defaults_limit() {
    let node_handle = NodeHandle::new_for_test();
    let (state, _temp_dir) = build_test_state(node_handle).await;

    let query = Query(PplnsQuery {
        limit: None,
        start_time: None,
        end_time: None,
    });

    let result = pplns_shares(State(state), query).await;
    assert!(result.is_ok());
}

/// Build an AppState with auth credentials configured.
async fn build_auth_test_state() -> (Arc<AppState>, tempfile::TempDir, String) {
    let (chain_store_handle, temp_dir) = setup_test_chain_store_handle(true).await;
    let metrics_temp = tempfile::tempdir().unwrap();
    let metrics_handle = metrics::start_metrics(metrics_temp.path().to_str().unwrap().to_string())
        .await
        .unwrap();
    let tracker_handle = start_tracker_actor();
    let node_handle = NodeHandle::new_for_test();

    let username = "testuser";
    let password = "testpassword";
    let salt = "testsalt";
    let hmac = p2poolv2_lib::auth::password_to_hmac(salt, password).unwrap();
    let token = format!("{salt}${hmac}");

    let base64_credentials =
        base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"));

    let state = Arc::new(AppState {
        app_config: AppConfig {
            pool_signature_length: 0,
            network: bitcoin::Network::Signet,
            cash_network: None,
            cors_allowed: false,
        },
        chain_store_handle,
        metrics_handle,
        tracker_handle,
        node_handle,
        monitoring_event_sender: create_monitoring_event_channel().0,
        auth_user: Some(username.to_string()),
        auth_token: Some(token),
        pplns_window: None,
    });
    (state, temp_dir, base64_credentials)
}

#[tokio::test]
async fn test_rest_with_valid_auth_header_succeeds() {
    use tower::ServiceExt;

    let (state, _temp_dir, base64_credentials) = build_auth_test_state().await;
    let app = build_router(state.clone(), state.app_config.clone());

    let request = http::Request::builder()
        .uri("/health")
        .header("Authorization", format!("Basic {base64_credentials}"))
        .body(http_body_util::Empty::<bytes::Bytes>::new())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), http::StatusCode::OK);
}

#[tokio::test]
async fn test_rest_without_auth_header_returns_401() {
    use tower::ServiceExt;

    let (state, _temp_dir, _base64_credentials) = build_auth_test_state().await;
    let app = build_router(state.clone(), state.app_config.clone());

    let request = http::Request::builder()
        .uri("/health")
        .body(http_body_util::Empty::<bytes::Bytes>::new())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), http::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_rest_with_token_param_returns_401() {
    use tower::ServiceExt;

    let (state, _temp_dir, base64_credentials) = build_auth_test_state().await;
    let app = build_router(state.clone(), state.app_config.clone());

    // Token query param should NOT authenticate REST routes
    let request = http::Request::builder()
        .uri(format!("/health?token={base64_credentials}"))
        .body(http_body_util::Empty::<bytes::Bytes>::new())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), http::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_miner_route_without_auth_header_returns_401() {
    use tower::ServiceExt;

    let (state, _temp_dir, _base64_credentials) = build_auth_test_state().await;
    let app = build_router(state.clone(), state.app_config.clone());

    let request = http::Request::builder()
        .uri("/miner?address=tb1q3udk7r26qs32ltf9nmqrjaaa7tr55qmkk30q5d")
        .body(http_body_util::Empty::<bytes::Bytes>::new())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), http::StatusCode::UNAUTHORIZED);
}

/// A malformed address is a 400 from the handler, which tells a routed
/// request apart from the 404 an unknown path gets.
#[tokio::test]
async fn test_miner_route_with_auth_reaches_the_handler() {
    use tower::ServiceExt;

    let (state, _temp_dir, base64_credentials) = build_auth_test_state().await;
    let app = build_router(state.clone(), state.app_config.clone());

    let request = http::Request::builder()
        .uri("/miner?address=not-an-address")
        .header("Authorization", format!("Basic {base64_credentials}"))
        .body(http_body_util::Empty::<bytes::Bytes>::new())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), http::StatusCode::BAD_REQUEST);
}

/// Start a real HTTP server on a random port and return (base_url, shutdown_tx).
///
/// WebSocket upgrade tests need a real TCP connection because axum's
/// WebSocketUpgrade extractor rejects non-upgradeable requests with 426.
async fn start_test_server(state: Arc<AppState>) -> (String, oneshot::Sender<()>) {
    let app = build_router(state.clone(), state.app_config.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
            .unwrap();
    });

    (format!("http://{addr}"), shutdown_tx)
}

#[tokio::test]
async fn test_ws_without_token_returns_401() {
    let (state, _temp_dir, _base64_credentials) = build_auth_test_state().await;
    let (base_url, _shutdown_tx) = start_test_server(state).await;

    let response = reqwest::Client::new()
        .get(format!("{base_url}/ws"))
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), 401);
}

#[tokio::test]
async fn test_ws_with_valid_token_param_upgrades() {
    let (state, _temp_dir, base64_credentials) = build_auth_test_state().await;
    let (base_url, _shutdown_tx) = start_test_server(state).await;

    let response = reqwest::Client::new()
        .get(format!("{base_url}/ws?token={base64_credentials}"))
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")
        .send()
        .await
        .unwrap();

    // 101 Switching Protocols means the upgrade succeeded
    assert_eq!(response.status(), 101);
}

#[tokio::test]
async fn test_ws_with_auth_header_only_does_not_upgrade() {
    let (state, _temp_dir, base64_credentials) = build_auth_test_state().await;
    let (base_url, _shutdown_tx) = start_test_server(state).await;

    // Auth header alone should NOT authenticate the WebSocket route
    let response = reqwest::Client::new()
        .get(format!("{base_url}/ws"))
        .header("Authorization", format!("Basic {base64_credentials}"))
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")
        .send()
        .await
        .unwrap();

    assert_ne!(
        response.status(),
        101,
        "WebSocket upgrade should not succeed with only an Authorization header"
    );
}

#[tokio::test]
async fn test_cors_headers_present_when_enabled() {
    use tower::ServiceExt;

    let node_handle = NodeHandle::new_for_test();
    let (chain_store_handle, _temp_dir) =
        p2poolv2_lib::test_utils::setup_test_chain_store_handle(true).await;
    let metrics_temp = tempfile::tempdir().unwrap();
    let metrics_handle = metrics::start_metrics(metrics_temp.path().to_str().unwrap().to_string())
        .await
        .unwrap();
    let tracker_handle = start_tracker_actor();

    let state = Arc::new(AppState {
        app_config: AppConfig {
            pool_signature_length: 0,
            network: bitcoin::Network::Signet,
            cash_network: None,
            cors_allowed: true,
        },
        chain_store_handle,
        metrics_handle,
        tracker_handle,
        node_handle,
        monitoring_event_sender: create_monitoring_event_channel().0,
        auth_user: None,
        auth_token: None,
        pplns_window: None,
    });

    let app = build_router(state.clone(), state.app_config.clone());

    let request = http::Request::builder()
        .uri("/health")
        .body(http_body_util::Empty::<bytes::Bytes>::new())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), http::StatusCode::OK);

    let headers = response.headers();
    assert_eq!(
        headers.get("access-control-allow-origin"),
        Some(&"*".parse().unwrap()),
        "access-control-allow-origin header should be present"
    );
}

async fn get_json(app: Router, uri: &str) -> (http::StatusCode, serde_json::Value) {
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    let request = http::Request::builder()
        .uri(uri)
        .body(http_body_util::Empty::<bytes::Bytes>::new())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or_default())
}

#[tokio::test]
async fn test_hashrate_route_serves_history() {
    let (state, _temp_dir) = build_test_state(NodeHandle::new_for_test()).await;
    let history = Arc::new(dashboard::history::HashrateHistory::default());
    history.push_sample(dashboard::history::Sample {
        time: 1000,
        pool: Some(5.0),
        local: None,
        network: Some(7.0),
    });
    let app = build_router_with_history(
        state.clone(),
        state.app_config.clone(),
        history,
        WebOptions::default(),
    );

    let (status, body) = get_json(app, "/hashrate?window=600").await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(body["pool_hashrate"], 5.0);
    assert!(body["local_hashrate"].is_null());
    assert_eq!(body["history"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn test_hashrate_route_rejects_out_of_range_window() {
    let (state, _temp_dir) = build_test_state(NodeHandle::new_for_test()).await;
    let app = build_router(state.clone(), state.app_config.clone());

    let (status, _) = get_json(app, "/hashrate?window=10").await;
    assert_eq!(status, http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_found_blocks_route_returns_json_list() {
    let (state, _temp_dir) = build_test_state(NodeHandle::new_for_test()).await;
    let app = build_router(state.clone(), state.app_config.clone());

    let (status, body) = get_json(app, "/found_blocks").await;
    assert_eq!(status, http::StatusCode::OK);
    assert!(body.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_payout_trees_route_returns_json_list() {
    let (state, _temp_dir) = build_test_state(NodeHandle::new_for_test()).await;
    let app = build_router(state.clone(), state.app_config.clone());

    let (status, body) = get_json(app, "/payout_trees").await;
    assert_eq!(status, http::StatusCode::OK);
    assert!(body.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_payout_tree_route_rejects_a_bad_txid_and_misses_an_unknown_one() {
    let (state, _temp_dir) = build_test_state(NodeHandle::new_for_test()).await;

    let (status, _) = get_json(
        build_router(state.clone(), state.app_config.clone()),
        "/payout_tree?coinbase_txid=nothex",
    )
    .await;
    assert_eq!(status, http::StatusCode::BAD_REQUEST);
    let (status, _) = get_json(
        build_router(state.clone(), state.app_config.clone()),
        &format!("/payout_tree?coinbase_txid={}", "11".repeat(32)),
    )
    .await;
    assert_eq!(status, http::StatusCode::NOT_FOUND);
}
