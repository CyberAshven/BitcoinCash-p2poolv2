// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use crate::accounting::stats::metrics::spawn_test_metrics_handle;
use crate::shares::bch_headers::KnownBchHeader;
use crate::test_utils::TestShareBlockBuilder;
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
        effort: None,
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
        effort: None,
    };
    let mut store = ChainStoreHandle::default();
    store
        .expect_get_candidate_found_blocks()
        .times(2)
        .returning(move || Ok(vec![candidate.clone()]));
    store.expect_is_current().return_const(true);
    store.expect_record_accepted_found_block().never();
    // No known headers: ten blocks by height are 6,000 ticks.
    store.expect_get_bch_best_hash().returning(|_| Ok(None));
    store.expect_get_bch_header().returning(|_| Ok(None));
    let metrics = spawn_test_metrics_handle();
    let client = BitcoindRpcClient::new(&server.uri(), "user", "pass").unwrap();
    let mut verifier = FoundBlockVerifier::new(client, store, metrics);

    verifier.verify_round().await;
    verifier.verify_round().await;
}

/// BCHN's finalization depth counts ticks (CHIP-2025-03): on one-minute
/// blocks, read from the known headers of BCHN's tip and of the block that
/// took the candidate's height, ten blocks are 600 ticks and not final, so
/// the candidate is asked about again; at a tip 6,000 ticks past that block
/// it is settled.
#[tokio::test]
async fn bchn_finalization_counts_ticks_on_one_minute_blocks() {
    for (tip_height, tip_ticks, asked) in [(332_010, 600, 2), (332_100, 6_000, 1)] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(body_partial_json(
                serde_json::json!({ "method": "getblockchaininfo" }),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "result": { "initialblockdownload": false, "chain": "chip", "blocks": tip_height },
                "error": null, "id": 0
            })))
            .mount(&server)
            .await;
        let at_height = BlockHash::from_byte_array([0x33; 32]);
        Mock::given(method("POST"))
            .and(body_partial_json(serde_json::json!({
                "method": "getblockhash", "params": [332_000]
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "result": at_height.to_string(),
                "error": null, "id": 0
            })))
            .expect(asked)
            .mount(&server)
            .await;
        let candidate = FoundBlock {
            bitcoin_height: 332_000,
            bitcoin_blockhash: BlockHash::from_byte_array([0x11; 32]),
            share_blockhash: BlockHash::from_byte_array([0x22; 32]),
            bitcoin_time: 1_700_000_000,
            miner_bitcoin_address: "miner".to_string(),
            effort: None,
        };
        let tip_hash = BlockHash::from_byte_array([0x44; 32]);
        let block = |height: u32, ticks: i64| KnownBchHeader {
            height,
            header: TestShareBlockBuilder::new().build().header.bitcoin_header,
            ticks: 1_000_000 + ticks,
            next_interval: 60,
        };
        let mut store = ChainStoreHandle::default();
        store
            .expect_get_candidate_found_blocks()
            .times(2)
            .returning(move || Ok(vec![candidate.clone()]));
        store.expect_is_current().return_const(true);
        store.expect_record_accepted_found_block().never();
        store
            .expect_get_bch_best_hash()
            .returning(move |height| Ok((height == tip_height).then_some(tip_hash)));
        store.expect_get_bch_header().returning(move |hash| {
            Ok(if *hash == tip_hash {
                Some(block(tip_height, tip_ticks))
            } else if *hash == at_height {
                Some(block(332_000, 0))
            } else {
                None
            })
        });
        let metrics = spawn_test_metrics_handle();
        let client = BitcoindRpcClient::new(&server.uri(), "user", "pass").unwrap();
        // The pins would count every one of these blocks as ten minutes.
        let mut verifier = FoundBlockVerifier::new(client, store, metrics)
            .with_tick_schedule(TickSchedule::TEN_MINUTES);

        verifier.verify_round().await;
        verifier.verify_round().await;
    }
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
        effort: None,
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
    let share_header = TestShareBlockBuilder::new()
        .nonce(0xe9695792)
        .build()
        .header;
    let expected_difficulty = share_header.bitcoin_network_difficulty();
    store
        .expect_get_share_header()
        .times(1)
        .return_once(move |_| Ok(share_header));
    store
        .expect_set_accepted_found_block_effort()
        .times(1)
        .withf(move |hash, snapshot| {
            *hash == bitcoin_blockhash
                && snapshot.work == 77.0
                && snapshot.network_difficulty == Some(expected_difficulty)
        })
        .returning(|_, _| Ok(()));
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
        effort: None,
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
    let share_header = TestShareBlockBuilder::new()
        .nonce(0xe9695792)
        .build()
        .header;
    let expected_difficulty = share_header.bitcoin_network_difficulty();
    store
        .expect_get_share_header()
        .times(1)
        .return_once(move |_| Ok(share_header));
    store
        .expect_set_accepted_found_block_effort()
        .times(1)
        .withf(move |hash, snapshot| {
            *hash == bitcoin_blockhash
                && snapshot.work == 77.0
                && snapshot.network_difficulty == Some(expected_difficulty)
        })
        .returning(|_, _| Ok(()));
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
        effort: None,
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
