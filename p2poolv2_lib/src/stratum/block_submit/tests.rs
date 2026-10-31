// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use crate::accounting::stats::metrics::spawn_test_metrics_handle;
use crate::test_utils::setup_test_chain_store_handle;
use bitcoin::Network;
use bitcoin::constants::genesis_block;
use wiremock::matchers::{body_partial_json, method};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Retries in milliseconds, so the tests do not wait minutes.
const FAST_POLICY: SubmitRetryPolicy = SubmitRetryPolicy {
    retry_window: Duration::from_millis(600),
    attempt_timeout: Duration::from_millis(200),
    first_backoff: Duration::from_millis(10),
    max_backoff: Duration::from_millis(40),
};

fn submitblock_answer(result: serde_json::Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(serde_json::json!({
        "result": result,
        "error": null,
        "id": 0,
    }))
}

fn client_for(server: &MockServer) -> BitcoindRpcClient {
    BitcoindRpcClient::new(&server.uri(), "user", "pass").unwrap()
}

#[tokio::test]
async fn an_accepted_block_is_sent_once() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(submitblock_answer(serde_json::Value::Null))
        .expect(1)
        .mount(&server)
        .await;
    let block = genesis_block(Network::Regtest);

    let result = submit_with_retry(&block, None, &client_for(&server), &FAST_POLICY).await;

    assert_eq!(result, (BlockSubmission::Accepted, 0));
}

/// `duplicate` means the node already has the valid block, for example from
/// an attempt whose answer was lost: the block is not lost.
#[tokio::test]
async fn a_duplicate_means_the_node_has_the_block() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(submitblock_answer(serde_json::json!("duplicate")))
        .expect(1)
        .mount(&server)
        .await;
    let block = genesis_block(Network::Regtest);

    let (result, _) = submit_with_retry(&block, None, &client_for(&server), &FAST_POLICY).await;

    assert_eq!(result, BlockSubmission::Duplicate);
    assert!(result.node_has_block());
}

/// `inconclusive` is the node's answer (it stored the block without a
/// verdict), so the block is not sent again.
#[tokio::test]
async fn an_inconclusive_block_is_not_sent_again() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(submitblock_answer(serde_json::json!("inconclusive")))
        .expect(1)
        .mount(&server)
        .await;
    let block = genesis_block(Network::Regtest);

    let (result, retries) =
        submit_with_retry(&block, None, &client_for(&server), &FAST_POLICY).await;

    assert_eq!(result, BlockSubmission::Inconclusive);
    assert!(result.node_has_block());
    assert_eq!(retries, 0);
}

#[tokio::test]
async fn a_rejected_block_is_not_sent_again() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(submitblock_answer(serde_json::json!("high-hash")))
        .expect(1)
        .mount(&server)
        .await;
    let block = genesis_block(Network::Regtest);

    let (result, retries) =
        submit_with_retry(&block, None, &client_for(&server), &FAST_POLICY).await;

    assert_eq!(result, BlockSubmission::Rejected("high-hash".to_string()));
    assert!(!result.node_has_block());
    assert_eq!(retries, 0);
}

/// BCHN answers 503 when its RPC work queue is full. The block goes again
/// until the node takes it.
#[tokio::test]
async fn a_busy_node_gets_the_block_again_until_it_answers() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(503).set_body_string("Work queue depth exceeded"))
        .up_to_n_times(2)
        .with_priority(1)
        .expect(2)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(submitblock_answer(serde_json::Value::Null))
        .with_priority(2)
        .expect(1)
        .mount(&server)
        .await;
    let block = genesis_block(Network::Regtest);

    let result = submit_with_retry(&block, None, &client_for(&server), &FAST_POLICY).await;

    assert_eq!(result, (BlockSubmission::Accepted, 2));
}

/// An attempt that gets no answer in time is sent again; the node may have
/// taken the first one, and then answers `duplicate`.
#[tokio::test]
async fn an_attempt_without_an_answer_in_time_is_sent_again() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            submitblock_answer(serde_json::Value::Null).set_delay(Duration::from_millis(500)),
        )
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(submitblock_answer(serde_json::json!("duplicate")))
        .with_priority(2)
        .mount(&server)
        .await;
    let block = genesis_block(Network::Regtest);

    let (result, retries) =
        submit_with_retry(&block, None, &client_for(&server), &FAST_POLICY).await;

    assert_eq!(result, BlockSubmission::Duplicate);
    assert_eq!(retries, 1);
}

#[tokio::test]
async fn an_unreachable_node_fails_after_the_retry_window() {
    let client = BitcoindRpcClient::new("http://127.0.0.1:1", "user", "pass").unwrap();
    let block = genesis_block(Network::Regtest);

    let (result, retries) = submit_with_retry(&block, None, &client, &FAST_POLICY).await;

    assert!(matches!(result, BlockSubmission::Failed(_)));
    assert!(retries >= 1);
}

/// An RPC error is the node's answer and comes back the same, so the block
/// is not sent again.
#[tokio::test]
async fn an_rpc_error_is_not_sent_again() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500).set_body_json(serde_json::json!({
            "result": null,
            "error": {"code": -22, "message": "Block decode failed"},
            "id": 0,
        })))
        .expect(1)
        .mount(&server)
        .await;
    let block = genesis_block(Network::Regtest);

    let (result, retries) =
        submit_with_retry(&block, None, &client_for(&server), &FAST_POLICY).await;

    assert!(matches!(result, BlockSubmission::Failed(ref reason) if reason.contains("-22")));
    assert_eq!(retries, 0);
}

#[tokio::test]
async fn the_primary_node_result_is_counted() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(submitblock_answer(serde_json::Value::Null))
        .mount(&server)
        .await;
    let metrics = spawn_test_metrics_handle();
    let block = genesis_block(Network::Regtest);
    let (store, _directory) = setup_test_chain_store_handle(true).await;
    let acceptance = FoundBlockAcceptance {
        store,
        found: FoundBlock {
            bitcoin_height: 0,
            bitcoin_blockhash: block.block_hash(),
            share_blockhash: block.block_hash(),
            bitcoin_time: block.header.time,
            miner_bitcoin_address: "miner".to_string(),
        },
    };
    metrics.record_confirmed_share(12.0).await.unwrap();

    let result =
        submit_to_primary_node(&block, None, &client_for(&server), &metrics, &FAST_POLICY).await;

    assert_eq!(result, BlockSubmission::Accepted);
    acceptance.record(&result, &metrics).await;
    assert_eq!(
        acceptance.store.get_found_blocks().unwrap(),
        vec![acceptance.found.clone()]
    );
    assert_eq!(metrics.get_metrics().await.work_since_last_block, 0.0);
    metrics.record_confirmed_share(7.0).await.unwrap();
    acceptance
        .record(&BlockSubmission::Duplicate, &metrics)
        .await;
    assert_eq!(metrics.get_metrics().await.work_since_last_block, 7.0);
    let counted = metrics.get_metrics().await;
    assert_eq!(
        counted
            .found_block_submissions
            .get(&("primary".to_string(), "accepted".to_string())),
        Some(&1)
    );
}

#[tokio::test]
async fn inconclusive_and_rejected_submissions_do_not_record_finds_or_reset_effort() {
    let (store, _directory) = setup_test_chain_store_handle(true).await;
    let block = genesis_block(Network::Regtest);
    let acceptance = FoundBlockAcceptance {
        store,
        found: FoundBlock {
            bitcoin_height: 0,
            bitcoin_blockhash: block.block_hash(),
            share_blockhash: block.block_hash(),
            bitcoin_time: block.header.time,
            miner_bitcoin_address: "miner".to_string(),
        },
    };
    let metrics = spawn_test_metrics_handle();
    metrics.record_confirmed_share(9.0).await.unwrap();

    acceptance
        .record(&BlockSubmission::Inconclusive, &metrics)
        .await;
    assert!(acceptance.store.get_found_blocks().unwrap().is_empty());
    assert_eq!(metrics.get_metrics().await.work_since_last_block, 9.0);
    acceptance
        .record(
            &BlockSubmission::Rejected("bad-diffbits".to_string()),
            &metrics,
        )
        .await;
    assert!(acceptance.store.get_found_blocks().unwrap().is_empty());
    assert_eq!(metrics.get_metrics().await.work_since_last_block, 9.0);
    acceptance
        .record(&BlockSubmission::Failed("timeout".to_string()), &metrics)
        .await;
    assert!(acceptance.store.get_found_blocks().unwrap().is_empty());
    assert_eq!(metrics.get_metrics().await.work_since_last_block, 9.0);
}

/// Every extra node gets the block from its own task and is counted under
/// its position in `[[submit_nodes]]`.
#[tokio::test]
async fn an_extra_node_gets_the_block_and_is_counted() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_partial_json(
            serde_json::json!({"method": "submitblock"}),
        ))
        .respond_with(submitblock_answer(serde_json::json!("duplicate")))
        .expect(1)
        .mount(&server)
        .await;
    let metrics = spawn_test_metrics_handle();
    let block = Arc::new(genesis_block(Network::Regtest));
    let extra_nodes: Arc<[BitcoindRpcClient]> = Arc::from([client_for(&server)]);

    spawn_extra_node_submissions(&block, &extra_nodes, &metrics, FAST_POLICY, None);

    let counted = timeout(Duration::from_secs(5), async {
        let key = ("extra1".to_string(), "duplicate".to_string());
        let mut counted = metrics.get_metrics().await;
        while !counted.found_block_submissions.contains_key(&key) {
            sleep(Duration::from_millis(10)).await;
            counted = metrics.get_metrics().await;
        }
        counted
    })
    .await
    .expect("the extra node submission was never counted");
    assert_eq!(counted.found_block_submit_retries.get("extra1"), Some(&0));
}

#[test]
fn extra_nodes_are_labelled_by_position_from_one() {
    assert_eq!(extra_node_label(0), "extra1");
    assert_eq!(extra_node_label(1), "extra2");
}

#[test]
fn result_labels_name_the_outcome() {
    assert_eq!(BlockSubmission::Accepted.label(), "accepted");
    assert_eq!(BlockSubmission::Duplicate.label(), "duplicate");
    assert_eq!(BlockSubmission::Inconclusive.label(), "inconclusive");
    assert_eq!(
        BlockSubmission::Rejected("high-hash".to_string()).label(),
        "rejected"
    );
    assert_eq!(
        BlockSubmission::Failed("timed out".to_string()).label(),
        "failed"
    );
}
