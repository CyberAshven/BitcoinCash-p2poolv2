// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use crate::accounting::stats::metrics::spawn_test_metrics_handle;
use bitcoin::hashes::Hash;
use mockall::Sequence;
use wiremock::matchers::{body_partial_json, method};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn a_claimed_target_candidate_absent_from_bchn_does_not_reset_effort() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_partial_json(serde_json::json!({
            "method": "getblockhash", "params": [900000]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": BlockHash::from_byte_array([0x33; 32]).to_string(),
            "error": null, "id": 0
        })))
        .expect(1)
        .mount(&server)
        .await;
    let candidate = FoundBlock {
        bitcoin_height: 900_000,
        bitcoin_blockhash: BlockHash::from_byte_array([0x11; 32]),
        share_blockhash: BlockHash::from_byte_array([0x22; 32]),
        bitcoin_time: 1_700_000_000,
        miner_bitcoin_address: "miner".to_string(),
    };
    let mut store = ChainStoreHandle::default();
    store
        .expect_get_candidate_found_blocks()
        .times(1)
        .return_once(move || Ok(vec![candidate]));
    store.expect_is_current().return_const(true);
    store.expect_record_accepted_found_block().never();
    let metrics = spawn_test_metrics_handle();
    metrics.record_confirmed_share(77.0).await.unwrap();
    let client = BitcoindRpcClient::new(&server.uri(), "user", "pass").unwrap();
    let mut verifier = FoundBlockVerifier::new(client, store, metrics.clone());

    verifier.verify_round().await;

    assert_eq!(metrics.get_metrics().await.work_since_last_block, 77.0);
}

/// A candidate that lost its height to a block BCHN has finalized can never
/// be accepted, so the verifier stops asking BCHN about it.
#[tokio::test]
async fn a_candidate_below_bchn_finalization_is_no_longer_checked() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_partial_json(
            serde_json::json!({ "method": "getblockchaininfo" }),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": { "initialblockdownload": false, "chain": "main", "blocks": 900_010 },
            "error": null, "id": 0
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(body_partial_json(serde_json::json!({
            "method": "getblockhash", "params": [900000]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": BlockHash::from_byte_array([0x33; 32]).to_string(),
            "error": null, "id": 0
        })))
        .expect(1)
        .mount(&server)
        .await;
    let candidate = FoundBlock {
        bitcoin_height: 900_000,
        bitcoin_blockhash: BlockHash::from_byte_array([0x11; 32]),
        share_blockhash: BlockHash::from_byte_array([0x22; 32]),
        bitcoin_time: 1_700_000_000,
        miner_bitcoin_address: "miner".to_string(),
    };
    let mut store = ChainStoreHandle::default();
    store
        .expect_get_candidate_found_blocks()
        .times(2)
        .returning(move || Ok(vec![candidate.clone()]));
    store.expect_is_current().return_const(true);
    store.expect_record_accepted_found_block().never();
    let metrics = spawn_test_metrics_handle();
    let client = BitcoindRpcClient::new(&server.uri(), "user", "pass").unwrap();
    let mut verifier = FoundBlockVerifier::new(client, store, metrics);

    verifier.verify_round().await;
    verifier.verify_round().await;
}

#[tokio::test]
async fn bchn_acceptance_resets_effort_only_for_the_first_persisted_find() {
    let server = MockServer::start().await;
    let bitcoin_blockhash = BlockHash::from_byte_array([0x11; 32]);
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": bitcoin_blockhash.to_string(), "error": null, "id": 0
        })))
        .expect(2)
        .mount(&server)
        .await;
    let candidate = FoundBlock {
        bitcoin_height: 900_000,
        bitcoin_blockhash,
        share_blockhash: BlockHash::from_byte_array([0x22; 32]),
        bitcoin_time: 1_700_000_000,
        miner_bitcoin_address: "miner".to_string(),
    };
    let mut store = ChainStoreHandle::default();
    let mut reads = Sequence::new();
    store
        .expect_get_candidate_found_blocks()
        .times(1)
        .in_sequence(&mut reads)
        .return_once(|| Ok(Vec::new()));
    store
        .expect_get_candidate_found_blocks()
        .times(2)
        .in_sequence(&mut reads)
        .returning(move || Ok(vec![candidate.clone()]));
    store.expect_is_current().return_const(true);
    let mut writes = Sequence::new();
    store
        .expect_record_accepted_found_block()
        .withf(move |found| found.bitcoin_blockhash == bitcoin_blockhash)
        .times(1)
        .in_sequence(&mut writes)
        .returning(|_| Ok(true));
    store
        .expect_record_accepted_found_block()
        .withf(move |found| found.bitcoin_blockhash == bitcoin_blockhash)
        .times(1)
        .in_sequence(&mut writes)
        .returning(|_| Ok(false));
    let metrics = spawn_test_metrics_handle();
    let client = BitcoindRpcClient::new(&server.uri(), "user", "pass").unwrap();
    let mut verifier = FoundBlockVerifier::new(client, store, metrics.clone());
    verifier.verify_round().await;
    metrics.record_confirmed_share(77.0).await.unwrap();

    verifier.verify_round().await;
    assert_eq!(metrics.get_metrics().await.work_since_last_block, 0.0);
    metrics.record_confirmed_share(9.0).await.unwrap();
    verifier.verify_round().await;

    assert_eq!(metrics.get_metrics().await.work_since_last_block, 9.0);
}

#[tokio::test]
async fn a_candidate_is_retried_after_an_rpc_failure_and_counted_after_acceptance() {
    let server = MockServer::start().await;
    let bitcoin_blockhash = BlockHash::from_byte_array([0x11; 32]);
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503))
        .expect(1)
        .mount(&server)
        .await;
    let candidate = FoundBlock {
        bitcoin_height: 900_000,
        bitcoin_blockhash,
        share_blockhash: BlockHash::from_byte_array([0x22; 32]),
        bitcoin_time: 1_700_000_000,
        miner_bitcoin_address: "miner".to_string(),
    };
    let mut store = ChainStoreHandle::default();
    let mut reads = Sequence::new();
    store
        .expect_get_candidate_found_blocks()
        .times(1)
        .in_sequence(&mut reads)
        .return_once(|| Ok(Vec::new()));
    store
        .expect_get_candidate_found_blocks()
        .times(2)
        .in_sequence(&mut reads)
        .returning(move || Ok(vec![candidate.clone()]));
    store.expect_is_current().return_const(true);
    store
        .expect_record_accepted_found_block()
        .times(1)
        .returning(|_| Ok(true));
    let metrics = spawn_test_metrics_handle();
    let client = BitcoindRpcClient::new(&server.uri(), "user", "pass").unwrap();
    let mut verifier = FoundBlockVerifier::new(client, store, metrics.clone());
    verifier.verify_round().await;
    metrics.record_confirmed_share(77.0).await.unwrap();

    verifier.verify_round().await;
    assert_eq!(metrics.get_metrics().await.work_since_last_block, 77.0);
    server.verify().await;
    server.reset().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": bitcoin_blockhash.to_string(), "error": null, "id": 0
        })))
        .expect(1)
        .mount(&server)
        .await;
    verifier.verify_round().await;

    assert_eq!(metrics.get_metrics().await.work_since_last_block, 0.0);
}

#[tokio::test]
async fn an_accepted_historical_candidate_is_recorded_without_resetting_runtime_effort() {
    let server = MockServer::start().await;
    let bitcoin_blockhash = BlockHash::from_byte_array([0x11; 32]);
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": bitcoin_blockhash.to_string(), "error": null, "id": 0
        })))
        .expect(1)
        .mount(&server)
        .await;
    let candidate = FoundBlock {
        bitcoin_height: 900_000,
        bitcoin_blockhash,
        share_blockhash: BlockHash::from_byte_array([0x22; 32]),
        bitcoin_time: 1_700_000_000,
        miner_bitcoin_address: "miner".to_string(),
    };
    let mut store = ChainStoreHandle::default();
    store
        .expect_get_candidate_found_blocks()
        .times(1)
        .return_once(move || Ok(vec![candidate]));
    store
        .expect_record_accepted_found_block()
        .times(1)
        .returning(|_| Ok(true));
    let metrics = spawn_test_metrics_handle();
    metrics.record_confirmed_share(77.0).await.unwrap();
    let client = BitcoindRpcClient::new(&server.uri(), "user", "pass").unwrap();
    let mut verifier = FoundBlockVerifier::new(client, store, metrics.clone());

    verifier.verify_round().await;

    assert_eq!(metrics.get_metrics().await.work_since_last_block, 77.0);
}
