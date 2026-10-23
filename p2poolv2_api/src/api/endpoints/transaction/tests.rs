use super::*;
use crate::api::server::{AppConfig, AppState};
use axum::extract::{Query, State};
use p2poolv2_lib::accounting::stats::metrics;
use p2poolv2_lib::monitoring_events::create_monitoring_event_channel;
use p2poolv2_lib::node::actor::NodeHandle;
use p2poolv2_lib::stratum::work::tracker::start_tracker_actor;
use p2poolv2_lib::test_utils::{
    TestShareBlockBuilder, genesis_for_tests, setup_test_chain_store_handle,
};

async fn build_test_state() -> (Arc<AppState>, tempfile::TempDir) {
    let (chain_store_handle, temp_dir) = setup_test_chain_store_handle(true).await;
    let genesis = genesis_for_tests();
    chain_store_handle
        .init_or_setup_genesis(genesis)
        .await
        .unwrap();

    let share = TestShareBlockBuilder::new()
        .prev_share_blockhash(chain_store_handle.get_chain_tip().unwrap().to_string())
        .nonce(1)
        .work(2)
        .build();
    chain_store_handle.add_share_block(share).await.unwrap();

    let metrics_temp = tempfile::tempdir().unwrap();
    let metrics_handle = metrics::start_metrics(metrics_temp.path().to_str().unwrap().to_string())
        .await
        .unwrap();
    let tracker_handle = start_tracker_actor();
    let node_handle = NodeHandle::new_for_test();
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
async fn test_transaction_invalid_txid() {
    let (state, _temp_dir) = build_test_state().await;
    let query = Query(TransactionQuery {
        txid: "not_a_valid_txid".to_string(),
        raw: None,
    });
    let result = transaction(State(state), query).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_transaction_not_found() {
    let (state, _temp_dir) = build_test_state().await;
    let query = Query(TransactionQuery {
        txid: "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
        raw: None,
    });
    let result = transaction(State(state), query).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_transaction_json_output() {
    let (state, _temp_dir) = build_test_state().await;
    let store = state.chain_store_handle.store_handle().store();
    let genesis_hash = state.chain_store_handle.get_chain_tip().unwrap();
    let txids = store.get_txids_for_blockhash(&genesis_hash);
    let txid = txids.0[0];

    let query = Query(TransactionQuery {
        txid: txid.to_string(),
        raw: None,
    });
    let result = transaction(State(state), query).await;
    assert!(result.is_ok());
    let json = result.unwrap().0;
    assert_eq!(json["txid"], txid.to_string());
    assert!(json["version"].is_number());
    assert!(json["inputs"].is_array());
    assert!(json["outputs"].is_array());
}

#[tokio::test]
async fn test_transaction_raw_output() {
    let (state, _temp_dir) = build_test_state().await;
    let store = state.chain_store_handle.store_handle().store();
    let genesis_hash = state.chain_store_handle.get_chain_tip().unwrap();
    let txids = store.get_txids_for_blockhash(&genesis_hash);
    let txid = txids.0[0];

    let query = Query(TransactionQuery {
        txid: txid.to_string(),
        raw: Some(true),
    });
    let result = transaction(State(state), query).await;
    assert!(result.is_ok());
    let json = result.unwrap().0;
    assert_eq!(json["txid"], txid.to_string());
    assert!(json["hex"].is_string());
    let hex_str = json["hex"].as_str().unwrap();
    assert!(!hex_str.is_empty());
    assert!(hex::decode(hex_str).is_ok());
}
