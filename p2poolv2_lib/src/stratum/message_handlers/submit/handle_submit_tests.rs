use super::*;
use crate::accounting::stats::metrics;
use crate::shares::share_commitment::ShareCommitment;
use crate::stratum::block_submit::FoundBlockSubmitter;
use crate::stratum::difficulty_adjuster::{DifficultyAdjuster, MockDifficultyAdjusterTrait};
use crate::stratum::merge_disclosure::{LineLog, MergeMiningLogs};
use crate::stratum::messages::Id;
use crate::stratum::messages::SetDifficultyNotification;
use crate::stratum::session::Session;
use crate::stratum::work::gbt::build_merkle_branches_for_template;
use crate::stratum::work::prepared_notify::insert_miner_job;
use crate::stratum::work::tracker::start_tracker_actor;
use crate::test_utils::{
    TEST_COINBASE_NSECS, build_pool_job_submit, build_token_win_submit, create_test_commitment,
    load_valid_stratum_work_components, setup_test_chain_store_handle, solve_job_submit,
    test_bch_prepared_with_tree_payees, test_bch_stratum_context,
};
use bitcoin::{Address, BlockHash, Network, PubkeyHash};
use bitcoindrpc::BitcoindRpcClient;
use bitcoindrpc::test_utils::{mock_submit_block_with_any_body, setup_mock_bitcoin_rpc};
use p2poolv2_config::DEFAULT_VERSION_MASK;
use p2poolv2_wallet::share_owner::ShareOwner;
use std::sync::Arc;
use tokio::sync::mpsc;

#[tokio::test]
async fn a_found_tree_is_stored_with_the_submitted_coinbase_and_share() {
    let (ctx, _emissions, _directories) = test_bch_stratum_context("http://127.0.0.1:1").await;
    let prepared = test_bch_prepared_with_tree_payees(100);
    let payout = Address::p2pkh(PubkeyHash::from_byte_array([0x22; 20]), Network::Regtest);
    let (job_id, _) = insert_miner_job(
        &prepared,
        Some(&payout),
        Some(ShareOwner::CashP2pkh([0x22; 20])),
        &ctx.tracker_handle,
    );
    let job = ctx.tracker_handle.get_job(job_id).unwrap();
    let submit = solve_job_submit(job_id, &job, "01010101", "0000000000000001");
    let found =
        validate_bitcoin_difficulty(&job, &submit, "01010101", DEFAULT_VERSION_MASK).unwrap();
    record_found_payout_tree(&ctx, found.header, &found.coinbase, &job).await;
    let stored = ctx.chain_store_handle.get_payout_trees().unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].coinbase_txid, found.coinbase.compute_txid());
    assert_eq!(stored[0].bitcoin_blockhash, found.header.block_hash());
    assert_eq!(
        stored[0].bitcoin_height,
        u64::from(job.blocktemplate.height)
    );
    assert_eq!(
        found.coinbase.output[stored[0].root_vout as usize],
        job.payout_tree.as_ref().unwrap().root_output()
    );
    let proof = CoinbaseProof::from_coinbase(
        &found.coinbase,
        job.share_commitment.as_ref().unwrap().share_witness_root,
    )
    .unwrap();
    let expected_share = ShareHeader::from_commitment_and_header(
        job.share_commitment.unwrap(),
        found.header,
        u64::from(job.blocktemplate.height),
        proof,
    );
    assert_eq!(stored[0].share_blockhash, expected_share.block_hash());
    assert_eq!(stored[0].payees, job.payout_tree.unwrap().payees());
}

/// A job without a share commitment (solo or Hydrapool mode) has no share to
/// name, and its tree is kept all the same.
#[tokio::test]
async fn a_tree_job_without_a_share_commitment_is_kept_without_a_share() {
    let (ctx, _emissions, _directories) = test_bch_stratum_context("http://127.0.0.1:1").await;
    let prepared = test_bch_prepared_with_tree_payees(100);
    let payout = Address::p2pkh(PubkeyHash::from_byte_array([0x22; 20]), Network::Regtest);
    let (job_id, _) = insert_miner_job(
        &prepared,
        Some(&payout),
        Some(ShareOwner::CashP2pkh([0x22; 20])),
        &ctx.tracker_handle,
    );
    let mut job = ctx.tracker_handle.get_job(job_id).unwrap();
    let submit = solve_job_submit(job_id, &job, "01010101", "0000000000000001");
    let found =
        validate_bitcoin_difficulty(&job, &submit, "01010101", DEFAULT_VERSION_MASK).unwrap();
    job.share_commitment = None;
    record_found_payout_tree(&ctx, found.header, &found.coinbase, &job).await;
    let stored = ctx.chain_store_handle.get_payout_trees().unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].coinbase_txid, found.coinbase.compute_txid());
    assert_eq!(stored[0].share_blockhash, BlockHash::all_zeros());
}

/// A store that cannot keep the tree does not hold the block back: the
/// whole reward is at stake, and the tree is fixed in the block, where every
/// peer validating the share keeps its own copy.
#[cfg(not(feature = "sim"))]
#[tokio::test]
async fn a_failed_tree_write_still_hands_the_block_off() {
    let (mock_server, _) = setup_mock_bitcoin_rpc().await;
    mock_submit_block_with_any_body(&mock_server).await;
    let (mut ctx, _emissions, _directories) = test_bch_stratum_context(&mock_server.uri()).await;
    let (closed_store, _closed_directory) = setup_test_chain_store_handle(false).await;
    ctx.chain_store_handle = closed_store;
    let prepared = test_bch_prepared_with_tree_payees(100);
    let payout = Address::p2pkh(PubkeyHash::from_byte_array([0x22; 20]), Network::Regtest);
    let (job_id, _) = insert_miner_job(
        &prepared,
        Some(&payout),
        Some(ShareOwner::CashP2pkh([0x22; 20])),
        &ctx.tracker_handle,
    );
    let job = ctx.tracker_handle.get_job(job_id).unwrap();
    let submit = solve_job_submit(job_id, &job, "01010101", "0000000000000001");
    let found =
        validate_bitcoin_difficulty(&job, &submit, "01010101", DEFAULT_VERSION_MASK).unwrap();

    submit_found_block(&ctx, found.header, found.coinbase.clone(), &job)
        .await
        .unwrap();

    assert!(
        ctx.chain_store_handle
            .get_payout_trees()
            .unwrap()
            .is_empty()
    );
    assert_eq!(mock_server.received_requests().await.unwrap().len(), 1);
}

#[cfg(not(feature = "sim"))]
#[tokio::test]
async fn sv1_keeps_a_found_tree_even_when_the_share_pipeline_rejects_it() {
    let (mock_server, _) = setup_mock_bitcoin_rpc().await;
    mock_submit_block_with_any_body(&mock_server).await;
    let (ctx, emissions, _directories) = test_bch_stratum_context(&mock_server.uri()).await;
    drop(emissions);
    let prepared = test_bch_prepared_with_tree_payees(100);
    let payout = Address::p2pkh(PubkeyHash::from_byte_array([0x22; 20]), Network::Regtest);
    let (job_id, _) = insert_miner_job(
        &prepared,
        Some(&payout),
        Some(ShareOwner::CashP2pkh([0x22; 20])),
        &ctx.tracker_handle,
    );
    let job = ctx.tracker_handle.get_job(job_id).unwrap();
    let mut session = Session::<DifficultyAdjuster>::new(1, 1, None, DEFAULT_VERSION_MASK);
    session.subscribed = true;
    session.user_id = Some(1);
    let submit = solve_job_submit(job_id, &job, &session.enonce1_hex, "0000000000000001");
    let store = ctx.chain_store_handle.clone();
    let error = handle_submit(submit, &mut session, ctx).await.unwrap_err();
    assert!(error.to_string().contains("Failed to send share to store"));
    let stored = store.get_payout_trees().unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].payees, job.payout_tree.unwrap().payees());
    assert_eq!(mock_server.received_requests().await.unwrap().len(), 1);
}

#[cfg(not(feature = "sim"))]
#[tokio::test]
async fn a_private_declarators_tree_is_kept_before_waiting_for_its_broadcast() {
    let (mock_server, _) = setup_mock_bitcoin_rpc().await;
    let (ctx, _emissions, _directories) = test_bch_stratum_context(&mock_server.uri()).await;
    let prepared = test_bch_prepared_with_tree_payees(100);
    let payout = Address::p2pkh(PubkeyHash::from_byte_array([0x22; 20]), Network::Regtest);
    let (job_id, _) = insert_miner_job(
        &prepared,
        Some(&payout),
        Some(ShareOwner::CashP2pkh([0x22; 20])),
        &ctx.tracker_handle,
    );
    let mut job = ctx.tracker_handle.get_job(job_id).unwrap();
    job.block_available = false;
    let submit = solve_job_submit(job_id, &job, "01010101", "0000000000000001");
    let found =
        validate_bitcoin_difficulty(&job, &submit, "01010101", DEFAULT_VERSION_MASK).unwrap();
    submit_found_block(&ctx, found.header, found.coinbase, &job)
        .await
        .unwrap();
    assert_eq!(ctx.chain_store_handle.get_payout_trees().unwrap().len(), 1);
    assert!(mock_server.received_requests().await.unwrap().is_empty());
}

#[test]
fn test_true_difficulty() {
    let hash = "000000000007f7453abd3f11338c165bf4876c086979630ed6f35ddbe59125a9"
        .parse::<BlockHash>()
        .unwrap();
    let difficulty = get_true_difficulty(&hash);
    assert_eq!(difficulty, 8226);
}

#[tokio::test]
async fn test_handle_submit_meets_difficulty_should_submit() {
    let mut session = Session::<DifficultyAdjuster>::new(1, 1, None, DEFAULT_VERSION_MASK);
    session.subscribed = true;
    let tracker_handle = start_tracker_actor();

    let (mock_server, bitcoinrpc_config) = setup_mock_bitcoin_rpc().await;
    mock_submit_block_with_any_body(&mock_server).await;

    let (template, notify, submit, authorize_response) =
        load_valid_stratum_work_components("../p2poolv2_tests/test_data/validation/stratum/b/");

    let enonce1 = authorize_response.result.unwrap()[1].clone();
    let enonce1: &str = enonce1.as_str().unwrap();
    session.enonce1 =
        u32::from_le_bytes(hex::decode(enonce1).unwrap().as_slice().try_into().unwrap());
    session.enonce1_hex = enonce1.to_string();
    session.btcaddress = Some("tb1q3udk7r26qs32ltf9nmqrjaaa7tr55qmkk30q5d".to_string());
    session.user_id = Some(1);

    let job_id = JobId(u64::from_str_radix(&notify.params.job_id, 16).unwrap());

    let merkle_branches = build_merkle_branches_for_template(&template)
        .unwrap()
        .into_iter()
        .map(bitcoin::TxMerkleNode::from_raw_hash)
        .collect();
    let _ = tracker_handle.insert_job(
        Arc::new(template),
        notify.params.coinbase1.to_string(),
        notify.params.coinbase2.to_string(),
        Some(create_test_commitment()),
        TEST_COINBASE_NSECS,
        merkle_branches,
        job_id,
    );

    let (emissions_tx, _emissions_rx) = mpsc::channel(10);
    let (pplns_shares_tx, mut pplns_shares_rx) = mpsc::channel(10);
    let stats_dir = tempfile::tempdir().unwrap();
    let metrics_handle = metrics::start_metrics(stats_dir.path().to_str().unwrap().to_string())
        .await
        .unwrap();
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let (template_refresh_tx, mut template_refresh_rx) = mpsc::channel(1);

    let ctx = StratumContext {
        found_block_submitter: FoundBlockSubmitter::single_attempt(),
        tracker_handle: tracker_handle.clone(),
        bitcoindrpc_client: BitcoindRpcClient::new(
            &bitcoinrpc_config.url,
            &bitcoinrpc_config.username,
            &bitcoinrpc_config.password,
        )
        .unwrap(),
        start_difficulty: 10000,
        minimum_difficulty: 1,
        maximum_difficulty: Some(2),
        ignore_difficulty: false,
        validate_addresses: true,
        emissions_tx,
        pplns_shares_tx,
        network: bitcoin::network::Network::Signet,
        metrics: metrics_handle.clone(),
        chain_store_handle,
        mode: PoolMode::P2poolv2,
        miner_address: None,
        share_chain: crate::config::Chain::Bitcoin,
        bch_share_network: None,
        cash_share: None,
        template_refresh_tx: Some(template_refresh_tx),
        merge_mining_logs: None,
    };

    let message = handle_submit(submit, &mut session, ctx).await.unwrap();

    let response = match &message[..] {
        [Message::Response(response)] => response,
        _ => panic!("Expected a Response message"),
    };

    assert_eq!(response.id, Some(Id::Number(4)));

    // The response should indicate that the share met required difficulty
    assert_eq!(response.result, Some(json!(true)));

    // The accepted block asks the template task for a new template once.
    // The sim feature compiles block submission out, so it asks for none.
    #[cfg(not(feature = "sim"))]
    assert_eq!(template_refresh_rx.try_recv(), Ok(()));
    assert!(template_refresh_rx.try_recv().is_err());

    // P2Poolv2 mode never queues a hydrapool PPLNS share
    assert!(pplns_shares_rx.try_recv().is_err());

    // Verify that the block is submitted to the mock server
    mock_server.verify().await;

    assert_eq!(metrics_handle.get_metrics().await.accepted_total, 1);
}

#[tokio::test]
async fn test_handle_submit_a_meets_difficulty_should_submit() {
    let mut session = Session::<DifficultyAdjuster>::new(1, 1, None, DEFAULT_VERSION_MASK);
    session.subscribed = true;
    let tracker_handle = start_tracker_actor();

    let (mock_server, bitcoinrpc_config) = setup_mock_bitcoin_rpc().await;
    mock_submit_block_with_any_body(&mock_server).await;

    let (template, notify, submit, authorize_response) =
        load_valid_stratum_work_components("../p2poolv2_tests/test_data/validation/stratum/a/");

    let enonce1 = authorize_response.result.unwrap()[1].clone();
    let enonce1: &str = enonce1.as_str().unwrap();
    session.enonce1 =
        u32::from_le_bytes(hex::decode(enonce1).unwrap().as_slice().try_into().unwrap());
    session.enonce1_hex = enonce1.to_string();
    session.btcaddress = Some("tb1q3udk7r26qs32ltf9nmqrjaaa7tr55qmkk30q5d".to_string());
    session.user_id = Some(1);

    let job_id = JobId(u64::from_str_radix(&notify.params.job_id, 16).unwrap());

    let _ = tracker_handle.insert_job(
        Arc::new(template),
        notify.params.coinbase1.to_string(),
        notify.params.coinbase2.to_string(),
        Some(create_test_commitment()),
        TEST_COINBASE_NSECS,
        vec![],
        job_id,
    );

    let (emissions_tx, _emissions_rx) = mpsc::channel(10);
    let (pplns_shares_tx, _pplns_shares_rx) = mpsc::channel(10);
    let stats_dir = tempfile::tempdir().unwrap();
    let metrics_handle = metrics::start_metrics(stats_dir.path().to_str().unwrap().to_string())
        .await
        .unwrap();

    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;

    let ctx = StratumContext {
        found_block_submitter: FoundBlockSubmitter::single_attempt(),
        tracker_handle: tracker_handle.clone(),
        bitcoindrpc_client: BitcoindRpcClient::new(
            &bitcoinrpc_config.url,
            &bitcoinrpc_config.username,
            &bitcoinrpc_config.password,
        )
        .unwrap(),
        start_difficulty: 10000,
        minimum_difficulty: 1,
        maximum_difficulty: Some(2),
        ignore_difficulty: false,
        validate_addresses: true,
        emissions_tx,
        pplns_shares_tx,
        network: bitcoin::network::Network::Signet,
        metrics: metrics_handle.clone(),
        chain_store_handle,
        mode: PoolMode::P2poolv2,
        miner_address: None,
        share_chain: crate::config::Chain::Bitcoin,
        bch_share_network: None,
        cash_share: None,
        template_refresh_tx: None,
        merge_mining_logs: None,
    };

    let response = handle_submit(submit, &mut session, ctx).await.unwrap();

    let response = match &response[..] {
        [Message::Response(response)] => response,
        _ => panic!("Expected a Response message"),
    };

    assert_eq!(response.id, Some(Id::Number(4)));

    // The response should indicate that the share met required difficulty
    assert_eq!(response.result, Some(json!(true)));

    // Verify that the block is submitted to the mock server
    mock_server.verify().await;

    assert_eq!(metrics_handle.get_metrics().await.accepted_total, 1);
}

#[tokio::test]
async fn test_handle_submit_with_version_rolling_meets_difficulty_should_submit() {
    let mut session = Session::<DifficultyAdjuster>::new(1, 1, None, DEFAULT_VERSION_MASK);
    session.subscribed = true;
    let tracker_handle = start_tracker_actor();

    let (mock_server, bitcoinrpc_config) = setup_mock_bitcoin_rpc().await;
    mock_submit_block_with_any_body(&mock_server).await;

    let (template, notify, submit, authorize_response) = load_valid_stratum_work_components(
        "../p2poolv2_tests/test_data/validation/stratum/with_version_rolling/",
    );

    let enonce1 = authorize_response.result.unwrap()[1].clone();
    let enonce1: &str = enonce1.as_str().unwrap();
    session.enonce1 =
        u32::from_le_bytes(hex::decode(enonce1).unwrap().as_slice().try_into().unwrap());
    session.enonce1_hex = enonce1.to_string();
    session.btcaddress = Some("tb1q3udk7r26qs32ltf9nmqrjaaa7tr55qmkk30q5d".to_string());
    session.user_id = Some(1);

    let job_id = JobId(u64::from_str_radix(&notify.params.job_id, 16).unwrap());

    let _ = tracker_handle.insert_job(
        Arc::new(template),
        notify.params.coinbase1.to_string(),
        notify.params.coinbase2.to_string(),
        Some(create_test_commitment()),
        TEST_COINBASE_NSECS,
        vec![],
        job_id,
    );

    let (emissions_tx, _emissions_rx) = mpsc::channel(10);
    let (pplns_shares_tx, _pplns_shares_rx) = mpsc::channel(10);
    let stats_dir = tempfile::tempdir().unwrap();
    let metrics_handle = metrics::start_metrics(stats_dir.path().to_str().unwrap().to_string())
        .await
        .unwrap();
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;

    let ctx = StratumContext {
        found_block_submitter: FoundBlockSubmitter::single_attempt(),
        tracker_handle: tracker_handle.clone(),
        bitcoindrpc_client: BitcoindRpcClient::new(
            &bitcoinrpc_config.url,
            &bitcoinrpc_config.username,
            &bitcoinrpc_config.password,
        )
        .unwrap(),
        start_difficulty: 10000,
        minimum_difficulty: 1,
        maximum_difficulty: Some(2),
        ignore_difficulty: false,
        validate_addresses: true,
        emissions_tx,
        pplns_shares_tx,
        network: bitcoin::network::Network::Signet,
        metrics: metrics_handle.clone(),
        chain_store_handle,
        mode: PoolMode::P2poolv2,
        miner_address: None,
        share_chain: crate::config::Chain::Bitcoin,
        bch_share_network: None,
        cash_share: None,
        template_refresh_tx: None,
        merge_mining_logs: None,
    };

    let response = handle_submit(submit, &mut session, ctx).await.unwrap();

    let response = match &response[..] {
        [Message::Response(response)] => response,
        _ => panic!("Expected a Response message"),
    };

    assert_eq!(response.id, Some(Id::Number(5)));

    // The response should indicate that the share met required difficulty
    assert_eq!(response.result, Some(json!(true)));

    // Verify that the block is submitted to the mock server
    mock_server.verify().await;

    assert_eq!(metrics_handle.get_metrics().await.accepted_total, 1);
}

#[tokio::test]
async fn test_handle_submit_triggers_difficulty_adjustment() {
    let ctx = MockDifficultyAdjusterTrait::new_context();
    ctx.expect().returning(|_, _, _| {
        let mut mock = MockDifficultyAdjusterTrait::default();
        mock.expect_record_share_submission().returning(
            |_difficulty, _job_id, _suggested_difficulty, _current_timestamp| (Some(12345), false),
        );
        mock.expect_get_current_difficulty().returning(|| 1u64);
        mock
    });

    let mut session = Session::<MockDifficultyAdjusterTrait>::new(1, 1, None, DEFAULT_VERSION_MASK);
    session.subscribed = true;
    let tracker_handle = start_tracker_actor();

    let (mock_server, bitcoinrpc_config) = setup_mock_bitcoin_rpc().await;
    mock_submit_block_with_any_body(&mock_server).await;

    let (template, notify, submit, authorize_response) =
        load_valid_stratum_work_components("../p2poolv2_tests/test_data/validation/stratum/a/");

    let enonce1 = authorize_response.result.unwrap()[1].clone();
    let enonce1: &str = enonce1.as_str().unwrap();
    session.enonce1 =
        u32::from_le_bytes(hex::decode(enonce1).unwrap().as_slice().try_into().unwrap());
    session.enonce1_hex = enonce1.to_string();
    session.btcaddress = Some("tb1q3udk7r26qs32ltf9nmqrjaaa7tr55qmkk30q5d".to_string());
    session.user_id = Some(1);

    let job_id = JobId(u64::from_str_radix(&notify.params.job_id, 16).unwrap());

    let _ = tracker_handle.insert_job(
        Arc::new(template),
        notify.params.coinbase1.to_string(),
        notify.params.coinbase2.to_string(),
        Some(create_test_commitment()),
        TEST_COINBASE_NSECS,
        vec![],
        job_id,
    );

    let (emissions_tx, _emissions_rx) = mpsc::channel(10);
    let (pplns_shares_tx, _pplns_shares_rx) = mpsc::channel(10);
    let stats_dir = tempfile::tempdir().unwrap();
    let metrics_handle = metrics::start_metrics(stats_dir.path().to_str().unwrap().to_string())
        .await
        .unwrap();

    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;

    let ctx = StratumContext {
        found_block_submitter: FoundBlockSubmitter::single_attempt(),
        tracker_handle: tracker_handle.clone(),
        bitcoindrpc_client: BitcoindRpcClient::new(
            &bitcoinrpc_config.url,
            &bitcoinrpc_config.username,
            &bitcoinrpc_config.password,
        )
        .unwrap(),
        start_difficulty: 10000,
        minimum_difficulty: 1,
        maximum_difficulty: Some(2),
        ignore_difficulty: false,
        validate_addresses: true,
        emissions_tx,
        pplns_shares_tx,
        network: bitcoin::network::Network::Signet,
        metrics: metrics_handle.clone(),
        chain_store_handle,
        mode: PoolMode::P2poolv2,
        miner_address: None,
        share_chain: crate::config::Chain::Bitcoin,
        bch_share_network: None,
        cash_share: None,
        template_refresh_tx: None,
        merge_mining_logs: None,
    };

    let message = handle_submit(submit, &mut session, ctx).await.unwrap();

    match &message[..] {
        [
            Message::Response(Response {
                id: _,
                result,
                error: _,
            }),
            Message::SetDifficulty(SetDifficultyNotification { method: _, params }),
        ] => {
            assert_eq!(result, &Some(json!(true)));
            assert_eq!(params[0], 12345);
        }
        _ => panic!("Expected SetDifficultyNotification message"),
    }

    mock_server.verify().await;
}

#[tokio::test]
async fn test_handle_submit_with_stale_job_returns_error() {
    let mut session = Session::<DifficultyAdjuster>::new(1, 1, None, DEFAULT_VERSION_MASK);
    session.subscribed = true;
    let tracker_handle = start_tracker_actor();

    let (_mock_server, bitcoinrpc_config) = setup_mock_bitcoin_rpc().await;

    let (_template, _notify, submit, authorize_response) =
        load_valid_stratum_work_components("../p2poolv2_tests/test_data/validation/stratum/a/");

    let enonce1 = authorize_response.result.unwrap()[1].clone();
    let enonce1: &str = enonce1.as_str().unwrap();
    session.enonce1 =
        u32::from_le_bytes(hex::decode(enonce1).unwrap().as_slice().try_into().unwrap());
    session.enonce1_hex = enonce1.to_string();
    session.btcaddress = Some("tb1q3udk7r26qs32ltf9nmqrjaaa7tr55qmkk30q5d".to_string());
    session.user_id = Some(1);

    let (emissions_tx, _emissions_rx) = mpsc::channel(10);
    let (pplns_shares_tx, _pplns_shares_rx) = mpsc::channel(10);
    let stats_dir = tempfile::tempdir().unwrap();
    let metrics_handle = metrics::start_metrics(stats_dir.path().to_str().unwrap().to_string())
        .await
        .unwrap();
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;

    let ctx = StratumContext {
        found_block_submitter: FoundBlockSubmitter::single_attempt(),
        tracker_handle: tracker_handle.clone(),
        bitcoindrpc_client: BitcoindRpcClient::new(
            &bitcoinrpc_config.url,
            &bitcoinrpc_config.username,
            &bitcoinrpc_config.password,
        )
        .unwrap(),
        start_difficulty: 10000,
        minimum_difficulty: 1,
        maximum_difficulty: Some(2),
        ignore_difficulty: false,
        validate_addresses: true,
        emissions_tx,
        pplns_shares_tx,
        network: bitcoin::network::Network::Signet,
        metrics: metrics_handle.clone(),
        chain_store_handle,
        mode: PoolMode::P2poolv2,
        miner_address: None,
        share_chain: crate::config::Chain::Bitcoin,
        bch_share_network: None,
        cash_share: None,
        template_refresh_tx: None,
        merge_mining_logs: None,
    };

    let message = handle_submit(submit, &mut session, ctx).await.unwrap();

    let response = match &message[..] {
        [Message::Response(response)] => response,
        _ => panic!("Expected a Response message"),
    };

    // Should return stale error
    assert_eq!(response.result, None);
    let err = response.error.as_ref().unwrap();
    assert_eq!(err.code, 21, "should be JobNotFound (code 21)");
    assert_eq!(err.message, "Job not found");
}

/// Session difficulty decides the miner response and metrics; the pool
/// target alone decides share chain emission. A share below session
/// difficulty that meets the pool target is rejected to the miner but
/// still emitted.
#[tokio::test]
async fn test_handle_submit_with_less_difficulty_than_session_is_rejected_but_emitted() {
    let mut session =
        Session::<DifficultyAdjuster>::new(10_000, 10_000, None, DEFAULT_VERSION_MASK);
    session.subscribed = true;
    session.enonce1 = u32::from_le_bytes([0xde, 0xad, 0xbe, 0xef]);
    session.enonce1_hex = "deadbeef".to_string();
    session.btcaddress = Some("tb1q3udk7r26qs32ltf9nmqrjaaa7tr55qmkk30q5d".to_string());
    session.user_id = Some(1);
    let tracker_handle = start_tracker_actor();

    let (mock_server, bitcoinrpc_config) = setup_mock_bitcoin_rpc().await;
    mock_submit_block_with_any_body(&mock_server).await;

    let submit = build_pool_job_submit(&tracker_handle, &session.enonce1_hex);

    let (emissions_tx, mut emissions_rx) = mpsc::channel(10);
    let (pplns_shares_tx, _pplns_shares_rx) = mpsc::channel(10);
    let stats_dir = tempfile::tempdir().unwrap();
    let metrics_handle = metrics::start_metrics(stats_dir.path().to_str().unwrap().to_string())
        .await
        .unwrap();
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;

    let ctx = StratumContext {
        found_block_submitter: FoundBlockSubmitter::single_attempt(),
        tracker_handle: tracker_handle.clone(),
        bitcoindrpc_client: BitcoindRpcClient::new(
            &bitcoinrpc_config.url,
            &bitcoinrpc_config.username,
            &bitcoinrpc_config.password,
        )
        .unwrap(),
        start_difficulty: 10_000,
        minimum_difficulty: 1,
        maximum_difficulty: None,
        ignore_difficulty: false,
        validate_addresses: true,
        emissions_tx,
        pplns_shares_tx,
        network: bitcoin::network::Network::Signet,
        metrics: metrics_handle.clone(),
        chain_store_handle,
        mode: PoolMode::P2poolv2,
        miner_address: None,
        share_chain: crate::config::Chain::Bitcoin,
        bch_share_network: None,
        cash_share: None,
        template_refresh_tx: None,
        merge_mining_logs: None,
    };

    let message = handle_submit(submit, &mut session, ctx).await.unwrap();

    let response = match &message[..] {
        [Message::Response(response)] => response,
        _ => panic!("Expected a Response message"),
    };

    assert_eq!(response.id, Some(Id::Number(4)));
    assert_eq!(response.result, Some(json!(false)));

    let share = emissions_rx.try_recv().unwrap();
    assert_eq!(
        share.share_commitment.miner_bitcoin_address,
        create_test_commitment().miner_bitcoin_address
    );

    assert_eq!(metrics_handle.get_metrics().await.accepted_total, 0);
    assert_eq!(metrics_handle.get_metrics().await.rejected_total, 1);
}

#[tokio::test]
async fn test_handle_submit_duplicate_share_is_rejected() {
    let mut session = Session::<DifficultyAdjuster>::new(1, 1, None, DEFAULT_VERSION_MASK);
    session.subscribed = true;
    session.enonce1 = u32::from_le_bytes([0xde, 0xad, 0xbe, 0xef]);
    session.enonce1_hex = "deadbeef".to_string();
    session.btcaddress = Some("tb1q3udk7r26qs32ltf9nmqrjaaa7tr55qmkk30q5d".to_string());
    session.user_id = Some(1);
    let tracker_handle = start_tracker_actor();

    let (mock_server, bitcoinrpc_config) = setup_mock_bitcoin_rpc().await;
    mock_submit_block_with_any_body(&mock_server).await;

    let submit = build_pool_job_submit(&tracker_handle, &session.enonce1_hex);

    let (emissions_tx, mut emissions_rx) = mpsc::channel(10);
    let (pplns_shares_tx, _pplns_shares_rx) = mpsc::channel(10);
    let stats_dir = tempfile::tempdir().unwrap();
    let metrics_handle = metrics::start_metrics(stats_dir.path().to_str().unwrap().to_string())
        .await
        .unwrap();
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;

    // First submission is emitted
    let ctx = StratumContext {
        found_block_submitter: FoundBlockSubmitter::single_attempt(),
        tracker_handle: tracker_handle.clone(),
        bitcoindrpc_client: BitcoindRpcClient::new(
            &bitcoinrpc_config.url,
            &bitcoinrpc_config.username,
            &bitcoinrpc_config.password,
        )
        .unwrap(),
        start_difficulty: 1,
        minimum_difficulty: 1,
        maximum_difficulty: None,
        ignore_difficulty: false,
        validate_addresses: true,
        emissions_tx: emissions_tx.clone(),
        pplns_shares_tx: pplns_shares_tx.clone(),
        network: bitcoin::network::Network::Signet,
        metrics: metrics_handle.clone(),
        chain_store_handle: chain_store_handle.clone(),
        mode: PoolMode::P2poolv2,
        miner_address: None,
        share_chain: crate::config::Chain::Bitcoin,
        bch_share_network: None,
        cash_share: None,
        template_refresh_tx: None,
        merge_mining_logs: None,
    };

    let message = handle_submit(submit.clone(), &mut session, ctx)
        .await
        .unwrap();

    let response = match &message[..] {
        [Message::Response(response)] => response,
        _ => panic!("Expected a Response message"),
    };
    assert!(response.error.is_none());
    let _share = emissions_rx.try_recv().unwrap();

    // Second submission of the same share should be rejected as duplicate
    let ctx2 = StratumContext {
        found_block_submitter: FoundBlockSubmitter::single_attempt(),
        tracker_handle: tracker_handle.clone(),
        bitcoindrpc_client: BitcoindRpcClient::new(
            &bitcoinrpc_config.url,
            &bitcoinrpc_config.username,
            &bitcoinrpc_config.password,
        )
        .unwrap(),
        start_difficulty: 1,
        minimum_difficulty: 1,
        maximum_difficulty: None,
        ignore_difficulty: false,
        validate_addresses: true,
        emissions_tx,
        pplns_shares_tx,
        network: bitcoin::network::Network::Signet,
        metrics: metrics_handle.clone(),
        chain_store_handle,
        mode: PoolMode::P2poolv2,
        miner_address: None,
        share_chain: crate::config::Chain::Bitcoin,
        bch_share_network: None,
        cash_share: None,
        template_refresh_tx: None,
        merge_mining_logs: None,
    };

    let message2 = handle_submit(submit, &mut session, ctx2).await.unwrap();

    let response2 = match &message2[..] {
        [Message::Response(response)] => response,
        _ => panic!("Expected a Response message"),
    };

    // Duplicate submission should return error code 22
    assert_eq!(response2.result, None);
    let err = response2.error.as_ref().unwrap();
    assert_eq!(err.code, 22, "should be DuplicateShare (code 22)");
    assert_eq!(err.message, "Duplicate share");

    // No additional emission should be sent for duplicate
    assert!(emissions_rx.try_recv().is_err());
}

#[tokio::test]
async fn test_handle_submit_accepts_low_difficulty_share_when_ignore_difficulty_is_true() {
    // Session difficulty 10_000 is far above what the share's hash meets
    let mut session =
        Session::<DifficultyAdjuster>::new(10_000, 10_000, None, DEFAULT_VERSION_MASK);
    session.subscribed = true;
    session.enonce1 = u32::from_le_bytes([0xde, 0xad, 0xbe, 0xef]);
    session.enonce1_hex = "deadbeef".to_string();
    session.btcaddress = Some("tb1q3udk7r26qs32ltf9nmqrjaaa7tr55qmkk30q5d".to_string());
    session.user_id = Some(1);
    let tracker_handle = start_tracker_actor();

    let (mock_server, bitcoinrpc_config) = setup_mock_bitcoin_rpc().await;
    mock_submit_block_with_any_body(&mock_server).await;

    let submit = build_pool_job_submit(&tracker_handle, &session.enonce1_hex);

    let (emissions_tx, mut emissions_rx) = mpsc::channel(10);
    let (pplns_shares_tx, _pplns_shares_rx) = mpsc::channel(10);
    let stats_dir = tempfile::tempdir().unwrap();
    let metrics_handle = metrics::start_metrics(stats_dir.path().to_str().unwrap().to_string())
        .await
        .unwrap();
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;

    let ctx = StratumContext {
        found_block_submitter: FoundBlockSubmitter::single_attempt(),
        tracker_handle: tracker_handle.clone(),
        bitcoindrpc_client: BitcoindRpcClient::new(
            &bitcoinrpc_config.url,
            &bitcoinrpc_config.username,
            &bitcoinrpc_config.password,
        )
        .unwrap(),
        start_difficulty: 10_000,
        minimum_difficulty: 1,
        maximum_difficulty: None,
        ignore_difficulty: true,
        validate_addresses: true,
        emissions_tx,
        pplns_shares_tx,
        network: bitcoin::network::Network::Signet,
        metrics: metrics_handle.clone(),
        chain_store_handle,
        mode: PoolMode::P2poolv2,
        miner_address: None,
        share_chain: crate::config::Chain::Bitcoin,
        bch_share_network: None,
        cash_share: None,
        template_refresh_tx: None,
        merge_mining_logs: None,
    };

    let message = handle_submit(submit, &mut session, ctx).await.unwrap();

    let response = match &message[..] {
        [Message::Response(response)] => response,
        _ => panic!("Expected a Response message"),
    };

    assert_eq!(response.id, Some(Id::Number(4)));

    // The response should indicate the share is accepted even though it doesn't meet session difficulty
    assert_eq!(response.result, Some(json!(true)));

    // Emission should be sent
    let share = emissions_rx.try_recv().unwrap();
    assert_eq!(
        share.share_commitment.miner_bitcoin_address,
        create_test_commitment().miner_bitcoin_address
    );

    // Share should be counted as accepted (not rejected)
    // accepted_difficulty_total tracks the session difficulty of accepted shares (10000)
    assert_eq!(metrics_handle.get_metrics().await.accepted_total, 1);
    assert_eq!(
        metrics_handle.get_metrics().await.accepted_difficulty_total,
        10000
    );
    assert_eq!(metrics_handle.get_metrics().await.rejected_total, 0);
}

/// The emitted coinbase proof is the midstate of the coinbase the miner
/// hashed, and the template merkle branches are passed through from the
/// job.
#[tokio::test]
async fn test_handle_submit_emits_coinbase_proof_of_share_coinbase() {
    let mut session = Session::<DifficultyAdjuster>::new(1, 1, None, DEFAULT_VERSION_MASK);
    session.subscribed = true;
    session.enonce1 = u32::from_le_bytes([0xde, 0xad, 0xbe, 0xef]);
    session.enonce1_hex = "deadbeef".to_string();
    session.btcaddress = Some("tb1q3udk7r26qs32ltf9nmqrjaaa7tr55qmkk30q5d".to_string());
    session.user_id = Some(1);
    let tracker_handle = start_tracker_actor();

    let (mock_server, bitcoinrpc_config) = setup_mock_bitcoin_rpc().await;
    mock_submit_block_with_any_body(&mock_server).await;

    let submit = build_pool_job_submit(&tracker_handle, &session.enonce1_hex);

    let (emissions_tx, mut emissions_rx) = mpsc::channel(10);
    let (pplns_shares_tx, _pplns_shares_rx) = mpsc::channel(10);
    let stats_dir = tempfile::tempdir().unwrap();
    let metrics_handle = metrics::start_metrics(stats_dir.path().to_str().unwrap().to_string())
        .await
        .unwrap();
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;

    let ctx = StratumContext {
        found_block_submitter: FoundBlockSubmitter::single_attempt(),
        tracker_handle: tracker_handle.clone(),
        bitcoindrpc_client: BitcoindRpcClient::new(
            &bitcoinrpc_config.url,
            &bitcoinrpc_config.username,
            &bitcoinrpc_config.password,
        )
        .unwrap(),
        start_difficulty: 1,
        minimum_difficulty: 1,
        maximum_difficulty: None,
        ignore_difficulty: false,
        validate_addresses: true,
        emissions_tx,
        pplns_shares_tx,
        network: bitcoin::network::Network::Signet,
        metrics: metrics_handle.clone(),
        chain_store_handle,
        mode: PoolMode::P2poolv2,
        miner_address: None,
        share_chain: crate::config::Chain::Bitcoin,
        bch_share_network: None,
        cash_share: None,
        template_refresh_tx: None,
        merge_mining_logs: None,
    };

    handle_submit(submit, &mut session, ctx).await.unwrap();

    let share = emissions_rx.try_recv().unwrap();
    let expected_proof = CoinbaseProof::from_coinbase(
        &share.bitcoin_coinbase,
        share.share_commitment.share_witness_root,
    )
    .unwrap();
    assert_eq!(share.coinbase_proof, expected_proof);

    let job = tracker_handle
        .get_job(tracker_handle.get_latest_job_id())
        .unwrap();
    assert_eq!(share.template_merkle_branches, job.template_merkle_branches);
}

#[tokio::test]
async fn test_handle_submit_not_subscribed_returns_error() {
    let mut session = Session::<DifficultyAdjuster>::new(1, 1, None, DEFAULT_VERSION_MASK);
    // session.subscribed is false by default
    let submit = SimpleRequest::new_submit(
        1,
        "worker".to_string(),
        "1".to_string(),
        "00000000".to_string(),
        "504e86ed".to_string(),
        "e9695791".to_string(),
    );
    let tracker_handle = start_tracker_actor();
    let (emissions_tx, _) = mpsc::channel(10);
    let (pplns_shares_tx, _pplns_shares_rx) = mpsc::channel(10);
    let (chain_store_handle, _) = setup_test_chain_store_handle(true).await;
    let (_mock_server, bitcoinrpc_config) = setup_mock_bitcoin_rpc().await;
    let stats_dir = tempfile::tempdir().unwrap();
    let metrics_handle = metrics::start_metrics(stats_dir.path().to_str().unwrap().to_string())
        .await
        .unwrap();

    let ctx = StratumContext {
        found_block_submitter: FoundBlockSubmitter::single_attempt(),
        tracker_handle: tracker_handle.clone(),
        bitcoindrpc_client: BitcoindRpcClient::new(
            &bitcoinrpc_config.url,
            &bitcoinrpc_config.username,
            &bitcoinrpc_config.password,
        )
        .unwrap(),
        start_difficulty: 1,
        minimum_difficulty: 1,
        maximum_difficulty: None,
        ignore_difficulty: false,
        validate_addresses: true,
        emissions_tx,
        pplns_shares_tx,
        network: bitcoin::network::Network::Regtest,
        metrics: metrics_handle,
        chain_store_handle,
        mode: PoolMode::P2poolv2,
        miner_address: None,
        share_chain: crate::config::Chain::Bitcoin,
        bch_share_network: None,
        cash_share: None,
        template_refresh_tx: None,
        merge_mining_logs: None,
    };

    let messages = handle_submit(submit, &mut session, ctx).await.unwrap();
    let response = match &messages[..] {
        [Message::Response(r)] => r,
        _ => panic!("expected Response"),
    };
    assert_eq!(response.result, None);
    let err = response.error.as_ref().unwrap();
    assert_eq!(err.code, 25, "should be NotSubscribed (code 25)");
    assert_eq!(err.message, "Not subscribed");
}

#[tokio::test]
async fn test_handle_submit_unknown_job_id_returns_invalid_jobid() {
    let mut session = Session::<DifficultyAdjuster>::new(1, 1, None, DEFAULT_VERSION_MASK);
    session.subscribed = true;
    let tracker_handle = start_tracker_actor();

    let (_mock_server, bitcoinrpc_config) = setup_mock_bitcoin_rpc().await;

    let (template, _notify, _submit, authorize_response) =
        load_valid_stratum_work_components("../p2poolv2_tests/test_data/validation/stratum/a/");

    let enonce1 = authorize_response.result.unwrap()[1].clone();
    let enonce1: &str = enonce1.as_str().unwrap();
    session.enonce1 =
        u32::from_le_bytes(hex::decode(enonce1).unwrap().as_slice().try_into().unwrap());
    session.enonce1_hex = enonce1.to_string();
    session.btcaddress = Some("tb1q3udk7r26qs32ltf9nmqrjaaa7tr55qmkk30q5d".to_string());
    session.user_id = Some(1);

    // Insert a job to set latest_job_id, then submit an ID beyond it
    let inserted_id = tracker_handle.insert_job(
        Arc::new(template),
        "cb1".to_string(),
        "cb2".to_string(),
        Some(create_test_commitment()),
        TEST_COINBASE_NSECS,
        vec![],
        tracker_handle.get_next_job_id(),
    );
    let unknown_job_id = tracker_handle.get_latest_job_id().0 + 1;

    let submit = SimpleRequest::new_submit(
        1,
        "worker".to_string(),
        format!("{unknown_job_id:x}"),
        "00000000".to_string(),
        "504e86ed".to_string(),
        "e9695791".to_string(),
    );

    let (emissions_tx, _) = mpsc::channel(10);
    let (pplns_shares_tx, _pplns_shares_rx) = mpsc::channel(10);
    let stats_dir = tempfile::tempdir().unwrap();
    let metrics_handle = metrics::start_metrics(stats_dir.path().to_str().unwrap().to_string())
        .await
        .unwrap();
    let (chain_store_handle, _) = setup_test_chain_store_handle(true).await;

    let ctx = StratumContext {
        found_block_submitter: FoundBlockSubmitter::single_attempt(),
        tracker_handle: tracker_handle.clone(),
        bitcoindrpc_client: BitcoindRpcClient::new(
            &bitcoinrpc_config.url,
            &bitcoinrpc_config.username,
            &bitcoinrpc_config.password,
        )
        .unwrap(),
        start_difficulty: 1,
        minimum_difficulty: 1,
        maximum_difficulty: None,
        ignore_difficulty: false,
        validate_addresses: true,
        emissions_tx,
        pplns_shares_tx,
        network: bitcoin::network::Network::Signet,
        metrics: metrics_handle,
        chain_store_handle,
        mode: PoolMode::P2poolv2,
        miner_address: None,
        share_chain: crate::config::Chain::Bitcoin,
        bch_share_network: None,
        cash_share: None,
        template_refresh_tx: None,
        merge_mining_logs: None,
    };

    // Verify the inserted job is properly registered
    let _ = inserted_id;

    let messages = handle_submit(submit, &mut session, ctx).await.unwrap();
    let response = match &messages[..] {
        [Message::Response(r)] => r,
        _ => panic!("expected Response"),
    };
    assert_eq!(response.result, None);
    let err = response.error.as_ref().unwrap();
    assert_eq!(err.code, 21, "should be JobNotFound (code 21)");
    assert_eq!(err.message, "Job not found");
}

#[tokio::test]
async fn test_handle_submit_validation_failure_returns_other_unknown_with_message() {
    let mut session = Session::<DifficultyAdjuster>::new(1, 1, None, DEFAULT_VERSION_MASK);
    session.subscribed = true;
    session.enonce1_hex = "deadbeef".to_string();
    session.user_id = Some(1);

    let tracker_handle = start_tracker_actor();

    let template: BlockTemplate = serde_json::from_value(json!({
         "version": 536870912,
        "rules": [],
        "vbavailable": {},
        "vbrequired": 0,
        "previousblockhash": "0000000000000000000000000000000000000000000000000000000000000000",
        "transactions": [],
        "coinbaseaux": {},
        "coinbasevalue": 5000000000_u64,
        "longpollid": "0",
        "target": "00000000ffff0000000000000000000000000000000000000000000000000000",
        "mintime": 1,
        "mutable": [],
        "noncerange": "00000000ffffffff",
        "sigoplimit": 80000,
        "sizelimit": 4000000,
        "weightlimit": 4000000,
        "curtime": 1,
        "bits": "1d00ffff",
        "height": 1,
        "default_witness_commitment": ""
    }))
    .unwrap();

    // bad coinbase fields make sure the validation fails
    let job_id = JobId(1);
    let _ = tracker_handle.insert_job(
        Arc::new(template),
        "deadbeef".to_string(),
        "cafebabe".to_string(),
        None,
        0,
        vec![],
        job_id,
    );

    let submit = SimpleRequest::new_submit(
        4,
        "worker".to_string(),
        "1".to_string(),
        "00000000".to_string(),
        "504e86ed".to_string(),
        "e9695791".to_string(),
    );

    let (_mock_server, bitcoinrpc_config) = setup_mock_bitcoin_rpc().await;
    let (emissions_tx, _) = mpsc::channel(10);
    let (pplns_shares_tx, _pplns_shares_rx) = mpsc::channel(10);
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let stats_dir = tempfile::tempdir().unwrap();
    let metrics_handle = metrics::start_metrics(stats_dir.path().to_str().unwrap().to_string())
        .await
        .unwrap();

    let ctx = StratumContext {
        found_block_submitter: FoundBlockSubmitter::single_attempt(),
        tracker_handle: tracker_handle.clone(),
        bitcoindrpc_client: BitcoindRpcClient::new(
            &bitcoinrpc_config.url,
            &bitcoinrpc_config.username,
            &bitcoinrpc_config.password,
        )
        .unwrap(),
        start_difficulty: 1,
        minimum_difficulty: 1,
        maximum_difficulty: None,
        ignore_difficulty: false,
        validate_addresses: true,
        emissions_tx,
        pplns_shares_tx,
        network: bitcoin::network::Network::Regtest,
        metrics: metrics_handle,
        chain_store_handle,
        mode: PoolMode::P2poolv2,
        miner_address: None,
        share_chain: crate::config::Chain::Bitcoin,
        bch_share_network: None,
        cash_share: None,
        template_refresh_tx: None,
        merge_mining_logs: None,
    };

    let messages = handle_submit(submit, &mut session, ctx).await.unwrap();
    let response = match &messages[..] {
        [Message::Response(r)] => r,
        _ => panic!("expected Response"),
    };
    assert_eq!(response.result, None);
    let err = response.error.as_ref().unwrap();
    assert_eq!(err.code, 20, "should be OtherUnknown (code 20)");
    assert!(
        err.message.starts_with("Invalid parameters provided:"),
        "error message should contain the validation failure reason, got: {}",
        err.message
    );
}

#[tokio::test]
async fn test_handle_submit_bad_nonce_returns_error_not_panic() {
    let mut session = Session::<DifficultyAdjuster>::new(1, 1, None, DEFAULT_VERSION_MASK);
    session.subscribed = true;
    let tracker_handle = start_tracker_actor();

    let (template, notify, _submit, authorize_response) =
        load_valid_stratum_work_components("../p2poolv2_tests/test_data/validation/stratum/b/");

    let enonce1 = authorize_response.result.unwrap()[1].clone();
    let enonce1: &str = enonce1.as_str().unwrap();
    session.enonce1 =
        u32::from_le_bytes(hex::decode(enonce1).unwrap().as_slice().try_into().unwrap());
    session.enonce1_hex = enonce1.to_string();
    session.btcaddress = Some("tb1q3udk7r26qs32ltf9nmqrjaaa7tr55qmkk30q5d".to_string());
    session.user_id = Some(1);

    let job_id = JobId(u64::from_str_radix(&notify.params.job_id, 16).unwrap());
    let _ = tracker_handle.insert_job(
        Arc::new(template),
        notify.params.coinbase1.to_string(),
        notify.params.coinbase2.to_string(),
        None,
        TEST_COINBASE_NSECS,
        vec![],
        job_id,
    );

    let submit = SimpleRequest::new_submit(
        4,
        "worker".to_string(),
        notify.params.job_id.clone(),
        "0000000000000000".to_string(),
        "504e86ed".to_string(),
        "ZZZZZZZZ".to_string(),
    );

    let (_mock_server, bitcoinrpc_config) = setup_mock_bitcoin_rpc().await;
    let (emissions_tx, _) = mpsc::channel(10);
    let (pplns_shares_tx, _pplns_shares_rx) = mpsc::channel(10);
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let stats_dir = tempfile::tempdir().unwrap();
    let metrics_handle = metrics::start_metrics(stats_dir.path().to_str().unwrap().to_string())
        .await
        .unwrap();

    let ctx = StratumContext {
        found_block_submitter: FoundBlockSubmitter::single_attempt(),
        tracker_handle: tracker_handle.clone(),
        bitcoindrpc_client: BitcoindRpcClient::new(
            &bitcoinrpc_config.url,
            &bitcoinrpc_config.username,
            &bitcoinrpc_config.password,
        )
        .unwrap(),
        start_difficulty: 1,
        minimum_difficulty: 1,
        maximum_difficulty: None,
        ignore_difficulty: false,
        validate_addresses: true,
        emissions_tx,
        pplns_shares_tx,
        network: bitcoin::network::Network::Regtest,
        metrics: metrics_handle,
        chain_store_handle,
        mode: PoolMode::P2poolv2,
        miner_address: None,
        share_chain: crate::config::Chain::Bitcoin,
        bch_share_network: None,
        cash_share: None,
        template_refresh_tx: None,
        merge_mining_logs: None,
    };

    let messages = handle_submit(submit, &mut session, ctx).await.unwrap();
    let response = match &messages[..] {
        [Message::Response(r)] => r,
        _ => panic!("expected Response"),
    };
    assert_eq!(response.result, None);
    let err = response.error.as_ref().unwrap();
    assert_eq!(err.code, 20, "should be OtherUnknown (code 20)");
    assert_eq!(err.message, "Invalid parameters provided: Bad nonce");
}

#[tokio::test]
async fn test_handle_submit_none_version_bits_returns_error_not_panic() {
    let mut session = Session::<DifficultyAdjuster>::new(1, 1, None, DEFAULT_VERSION_MASK);
    session.subscribed = true;
    let tracker_handle = start_tracker_actor();

    let (template, notify, _submit, authorize_response) =
        load_valid_stratum_work_components("../p2poolv2_tests/test_data/validation/stratum/b/");

    let enonce1 = authorize_response.result.unwrap()[1].clone();
    let enonce1: &str = enonce1.as_str().unwrap();
    session.enonce1 =
        u32::from_le_bytes(hex::decode(enonce1).unwrap().as_slice().try_into().unwrap());
    session.enonce1_hex = enonce1.to_string();
    session.btcaddress = Some("tb1q3udk7r26qs32ltf9nmqrjaaa7tr55qmkk30q5d".to_string());
    session.user_id = Some(1);

    let job_id = JobId(u64::from_str_radix(&notify.params.job_id, 16).unwrap());
    let _ = tracker_handle.insert_job(
        Arc::new(template),
        notify.params.coinbase1.to_string(),
        notify.params.coinbase2.to_string(),
        None,
        TEST_COINBASE_NSECS,
        vec![],
        job_id,
    );

    // Build a submit with 6 params where the 6th (version bits) is None
    let submit = SimpleRequest {
        id: Some(Id::Number(4)),
        method: std::borrow::Cow::Owned("mining.submit".to_string()),
        params: std::borrow::Cow::Owned(vec![
            Some("worker".to_string()),
            Some(notify.params.job_id.clone()),
            Some("0000000000000000".to_string()),
            Some("504e86ed".to_string()),
            Some("e9695791".to_string()),
            None, // version bits is None
        ]),
    };

    let (_mock_server, bitcoinrpc_config) = setup_mock_bitcoin_rpc().await;
    let (emissions_tx, _) = mpsc::channel(10);
    let (pplns_shares_tx, _pplns_shares_rx) = mpsc::channel(10);
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let stats_dir = tempfile::tempdir().unwrap();
    let metrics_handle = metrics::start_metrics(stats_dir.path().to_str().unwrap().to_string())
        .await
        .unwrap();

    let ctx = StratumContext {
        found_block_submitter: FoundBlockSubmitter::single_attempt(),
        tracker_handle: tracker_handle.clone(),
        bitcoindrpc_client: BitcoindRpcClient::new(
            &bitcoinrpc_config.url,
            &bitcoinrpc_config.username,
            &bitcoinrpc_config.password,
        )
        .unwrap(),
        start_difficulty: 1,
        minimum_difficulty: 1,
        maximum_difficulty: None,
        ignore_difficulty: false,
        validate_addresses: true,
        emissions_tx,
        pplns_shares_tx,
        network: bitcoin::network::Network::Regtest,
        metrics: metrics_handle,
        chain_store_handle,
        mode: PoolMode::P2poolv2,
        miner_address: None,
        share_chain: crate::config::Chain::Bitcoin,
        bch_share_network: None,
        cash_share: None,
        template_refresh_tx: None,
        merge_mining_logs: None,
    };

    let messages = handle_submit(submit, &mut session, ctx).await.unwrap();
    let response = match &messages[..] {
        [Message::Response(r)] => r,
        _ => panic!("expected Response"),
    };
    assert_eq!(response.result, None);
    let err = response.error.as_ref().unwrap();
    assert_eq!(err.code, 20, "should be OtherUnknown (code 20)");
    assert_eq!(
        err.message,
        "Invalid parameters provided: Missing version bits"
    );
}

#[tokio::test]
async fn test_handle_submit_unauthorized_returns_error() {
    let mut session = Session::<DifficultyAdjuster>::new(1, 1, None, DEFAULT_VERSION_MASK);
    session.subscribed = true;
    // user_id is None -- simulates a session where authorize partially
    // failed (username set but register_user errored)
    assert!(session.user_id.is_none());

    let submit = SimpleRequest::new_submit(
        1,
        "worker".to_string(),
        "1".to_string(),
        "00000000".to_string(),
        "504e86ed".to_string(),
        "e9695791".to_string(),
    );
    let tracker_handle = start_tracker_actor();
    let (emissions_tx, _emissions_rx) = mpsc::channel(10);
    let (pplns_shares_tx, _pplns_shares_rx) = mpsc::channel(10);
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let (_mock_server, bitcoinrpc_config) = setup_mock_bitcoin_rpc().await;
    let stats_dir = tempfile::tempdir().unwrap();
    let metrics_handle = metrics::start_metrics(stats_dir.path().to_str().unwrap().to_string())
        .await
        .unwrap();

    let ctx = StratumContext {
        found_block_submitter: FoundBlockSubmitter::single_attempt(),
        tracker_handle: tracker_handle.clone(),
        bitcoindrpc_client: BitcoindRpcClient::new(
            &bitcoinrpc_config.url,
            &bitcoinrpc_config.username,
            &bitcoinrpc_config.password,
        )
        .unwrap(),
        start_difficulty: 1,
        minimum_difficulty: 1,
        maximum_difficulty: None,
        ignore_difficulty: false,
        validate_addresses: true,
        emissions_tx,
        pplns_shares_tx,
        network: bitcoin::network::Network::Regtest,
        metrics: metrics_handle,
        chain_store_handle,
        mode: PoolMode::P2poolv2,
        miner_address: None,
        share_chain: crate::config::Chain::Bitcoin,
        bch_share_network: None,
        cash_share: None,
        template_refresh_tx: None,
        merge_mining_logs: None,
    };

    let messages = handle_submit(submit, &mut session, ctx).await.unwrap();
    let response = match &messages[..] {
        [Message::Response(r)] => r,
        _ => panic!("expected Response"),
    };
    assert_eq!(response.result, None);
    let err = response.error.as_ref().unwrap();
    assert_eq!(err.code, 24, "should be UnauthorizedWorker (code 24)");
    assert_eq!(err.message, "Not authorized");
}

/// Build a commitment with an impossibly hard pool target so that
/// any test share will fail the ASERT pool difficulty check.
fn create_hard_pool_difficulty_commitment() -> ShareCommitment {
    let mut commitment = create_test_commitment();
    // 0x01003456 is an extremely hard target that no test share can meet
    commitment.bits = bitcoin::CompactTarget::from_consensus(0x01003456);
    commitment
}

/// A share that meets the miner's session difficulty but not the pool
/// difficulty target must still be accepted (the miner met its assigned
/// difficulty) and counted toward hashrate metrics, but must NOT be emitted
/// to the share chain. This keeps hashrate reporting accurate for miners
/// whose session difficulty is below the pool target.
#[tokio::test]
async fn test_p2poolv2_mode_accepts_and_accounts_share_below_pool_difficulty_without_emitting() {
    let mut session = Session::<DifficultyAdjuster>::new(1, 1, None, DEFAULT_VERSION_MASK);
    session.subscribed = true;
    let tracker_handle = start_tracker_actor();

    let (mock_server, bitcoinrpc_config) = setup_mock_bitcoin_rpc().await;
    mock_submit_block_with_any_body(&mock_server).await;

    let (template, notify, submit, authorize_response) =
        load_valid_stratum_work_components("../p2poolv2_tests/test_data/validation/stratum/b/");

    let enonce1 = authorize_response.result.unwrap()[1].clone();
    let enonce1: &str = enonce1.as_str().unwrap();
    session.enonce1 =
        u32::from_le_bytes(hex::decode(enonce1).unwrap().as_slice().try_into().unwrap());
    session.enonce1_hex = enonce1.to_string();
    session.btcaddress = Some("tb1q3udk7r26qs32ltf9nmqrjaaa7tr55qmkk30q5d".to_string());
    session.user_id = Some(1);

    let job_id = JobId(u64::from_str_radix(&notify.params.job_id, 16).unwrap());

    let merkle_branches = build_merkle_branches_for_template(&template)
        .unwrap()
        .into_iter()
        .map(bitcoin::TxMerkleNode::from_raw_hash)
        .collect();
    let _ = tracker_handle.insert_job(
        Arc::new(template),
        notify.params.coinbase1.to_string(),
        notify.params.coinbase2.to_string(),
        Some(create_hard_pool_difficulty_commitment()),
        TEST_COINBASE_NSECS,
        merkle_branches,
        job_id,
    );

    let (emissions_tx, mut emissions_rx) = mpsc::channel(10);
    let (pplns_shares_tx, _pplns_shares_rx) = mpsc::channel(10);
    let stats_dir = tempfile::tempdir().unwrap();
    let metrics_handle = metrics::start_metrics(stats_dir.path().to_str().unwrap().to_string())
        .await
        .unwrap();
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;

    let ctx = StratumContext {
        found_block_submitter: FoundBlockSubmitter::single_attempt(),
        tracker_handle: tracker_handle.clone(),
        bitcoindrpc_client: BitcoindRpcClient::new(
            &bitcoinrpc_config.url,
            &bitcoinrpc_config.username,
            &bitcoinrpc_config.password,
        )
        .unwrap(),
        start_difficulty: 1,
        minimum_difficulty: 1,
        maximum_difficulty: None,
        ignore_difficulty: false,
        validate_addresses: true,
        emissions_tx,
        pplns_shares_tx,
        network: bitcoin::network::Network::Signet,
        metrics: metrics_handle.clone(),
        chain_store_handle,
        mode: PoolMode::P2poolv2,
        miner_address: None,
        share_chain: crate::config::Chain::Bitcoin,
        bch_share_network: None,
        cash_share: None,
        template_refresh_tx: None,
        merge_mining_logs: None,
    };

    let messages = handle_submit(submit, &mut session, ctx).await.unwrap();
    let response = match &messages[..] {
        [Message::Response(r)] => r,
        _ => panic!("expected Response"),
    };
    // Share met the miner's session difficulty, so it is accepted.
    assert_eq!(response.result, Some(json!(true)));

    // Share must NOT be emitted to the share chain: it does not meet the
    // pool difficulty target.
    assert!(
        emissions_rx.try_recv().is_err(),
        "share below pool difficulty must not be emitted to the share chain"
    );

    // Share must still be counted toward hashrate metrics.
    assert_eq!(metrics_handle.get_metrics().await.accepted_total, 1);
}

#[tokio::test]
async fn test_hydrapool_mode_accepts_share_despite_hard_pool_difficulty() {
    let mut session = Session::<DifficultyAdjuster>::new(1, 1, None, DEFAULT_VERSION_MASK);
    session.subscribed = true;
    let tracker_handle = start_tracker_actor();

    let (mock_server, bitcoinrpc_config) = setup_mock_bitcoin_rpc().await;
    mock_submit_block_with_any_body(&mock_server).await;

    let (template, notify, submit, authorize_response) =
        load_valid_stratum_work_components("../p2poolv2_tests/test_data/validation/stratum/b/");

    let enonce1 = authorize_response.result.unwrap()[1].clone();
    let enonce1: &str = enonce1.as_str().unwrap();
    session.enonce1 =
        u32::from_le_bytes(hex::decode(enonce1).unwrap().as_slice().try_into().unwrap());
    session.enonce1_hex = enonce1.to_string();
    session.btcaddress = Some("tb1q3udk7r26qs32ltf9nmqrjaaa7tr55qmkk30q5d".to_string());
    session.user_id = Some(1);

    let job_id = JobId(u64::from_str_radix(&notify.params.job_id, 16).unwrap());

    let merkle_branches = build_merkle_branches_for_template(&template)
        .unwrap()
        .into_iter()
        .map(bitcoin::TxMerkleNode::from_raw_hash)
        .collect();
    let _ = tracker_handle.insert_job(
        Arc::new(template),
        notify.params.coinbase1.to_string(),
        notify.params.coinbase2.to_string(),
        Some(create_hard_pool_difficulty_commitment()),
        TEST_COINBASE_NSECS,
        merkle_branches,
        job_id,
    );

    let (emissions_tx, mut emissions_rx) = mpsc::channel(10);
    let (pplns_shares_tx, mut pplns_shares_rx) = mpsc::channel(10);
    let stats_dir = tempfile::tempdir().unwrap();
    let metrics_handle = metrics::start_metrics(stats_dir.path().to_str().unwrap().to_string())
        .await
        .unwrap();
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;

    let ctx = StratumContext {
        found_block_submitter: FoundBlockSubmitter::single_attempt(),
        tracker_handle: tracker_handle.clone(),
        bitcoindrpc_client: BitcoindRpcClient::new(
            &bitcoinrpc_config.url,
            &bitcoinrpc_config.username,
            &bitcoinrpc_config.password,
        )
        .unwrap(),
        start_difficulty: 1,
        minimum_difficulty: 1,
        maximum_difficulty: None,
        ignore_difficulty: false,
        validate_addresses: true,
        emissions_tx,
        pplns_shares_tx,
        network: bitcoin::network::Network::Signet,
        metrics: metrics_handle,
        chain_store_handle,
        mode: PoolMode::Hydrapool,
        miner_address: None,
        share_chain: crate::config::Chain::Bitcoin,
        bch_share_network: None,
        cash_share: None,
        template_refresh_tx: None,
        merge_mining_logs: None,
    };

    let messages = handle_submit(submit, &mut session, ctx).await.unwrap();
    let response = match &messages[..] {
        [Message::Response(response)] => response,
        _ => panic!("expected Response"),
    };
    // Share is accepted despite not meeting pool difficulty
    assert_eq!(response.result, Some(json!(true)));

    // Share is queued for the PPLNS share worker at the session
    // difficulty, and never emitted to the share chain.
    let share = pplns_shares_rx.try_recv().unwrap();
    assert_eq!(
        share.btcaddress,
        Some("tb1q3udk7r26qs32ltf9nmqrjaaa7tr55qmkk30q5d".to_string())
    );
    assert_eq!(share.difficulty, 1);
    assert!(
        emissions_rx.try_recv().is_err(),
        "hydrapool share must not be emitted to the share chain"
    );
}

/// Regression for GHSA-mmw7-mxvq-79wq: a hydrapool share whose hash does
/// not meet the session difficulty must not be stored for PPLNS, because
/// it would be credited at the session difficulty the miner chose.
#[tokio::test]
async fn test_hydrapool_mode_does_not_store_share_below_session_difficulty() {
    let mut session =
        Session::<DifficultyAdjuster>::new(10_000, 10_000, None, DEFAULT_VERSION_MASK);
    session.subscribed = true;
    let tracker_handle = start_tracker_actor();

    let (mock_server, bitcoinrpc_config) = setup_mock_bitcoin_rpc().await;
    mock_submit_block_with_any_body(&mock_server).await;

    let (template, notify, submit, authorize_response) =
        load_valid_stratum_work_components("../p2poolv2_tests/test_data/validation/stratum/b/");

    let enonce1 = authorize_response.result.unwrap()[1].clone();
    let enonce1: &str = enonce1.as_str().unwrap();
    session.enonce1 =
        u32::from_le_bytes(hex::decode(enonce1).unwrap().as_slice().try_into().unwrap());
    session.enonce1_hex = enonce1.to_string();
    session.btcaddress = Some("tb1q3udk7r26qs32ltf9nmqrjaaa7tr55qmkk30q5d".to_string());
    session.user_id = Some(1);

    let job_id = JobId(u64::from_str_radix(&notify.params.job_id, 16).unwrap());

    let _ = tracker_handle.insert_job(
        Arc::new(template),
        notify.params.coinbase1.to_string(),
        notify.params.coinbase2.to_string(),
        None,
        TEST_COINBASE_NSECS,
        vec![],
        job_id,
    );

    let (emissions_tx, mut emissions_rx) = mpsc::channel(10);
    let (pplns_shares_tx, mut pplns_shares_rx) = mpsc::channel(10);
    let stats_dir = tempfile::tempdir().unwrap();
    let metrics_handle = metrics::start_metrics(stats_dir.path().to_str().unwrap().to_string())
        .await
        .unwrap();
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;

    let ctx = StratumContext {
        found_block_submitter: FoundBlockSubmitter::single_attempt(),
        tracker_handle: tracker_handle.clone(),
        bitcoindrpc_client: BitcoindRpcClient::new(
            &bitcoinrpc_config.url,
            &bitcoinrpc_config.username,
            &bitcoinrpc_config.password,
        )
        .unwrap(),
        start_difficulty: 10_000,
        minimum_difficulty: 1,
        maximum_difficulty: None,
        ignore_difficulty: false,
        validate_addresses: true,
        emissions_tx,
        pplns_shares_tx,
        network: bitcoin::network::Network::Signet,
        metrics: metrics_handle.clone(),
        chain_store_handle,
        mode: PoolMode::Hydrapool,
        miner_address: None,
        share_chain: crate::config::Chain::Bitcoin,
        bch_share_network: None,
        cash_share: None,
        template_refresh_tx: None,
        merge_mining_logs: None,
    };

    let messages = handle_submit(submit, &mut session, ctx).await.unwrap();
    let response = match &messages[..] {
        [Message::Response(response)] => response,
        _ => panic!("expected Response"),
    };
    assert_eq!(response.result, Some(json!(false)));

    assert!(
        pplns_shares_rx.try_recv().is_err(),
        "hydrapool share below session difficulty must not be stored"
    );
    assert!(
        emissions_rx.try_recv().is_err(),
        "hydrapool share below session difficulty must not be emitted"
    );
    assert_eq!(metrics_handle.get_metrics().await.rejected_total, 1);
}

/// `ignore_difficulty` only changes the miner response and metrics. It
/// must not let a hydrapool share below session difficulty into PPLNS
/// storage.
#[tokio::test]
async fn test_hydrapool_mode_ignore_difficulty_does_not_store_share_below_session_difficulty() {
    let mut session =
        Session::<DifficultyAdjuster>::new(10_000, 10_000, None, DEFAULT_VERSION_MASK);
    session.subscribed = true;
    let tracker_handle = start_tracker_actor();

    let (mock_server, bitcoinrpc_config) = setup_mock_bitcoin_rpc().await;
    mock_submit_block_with_any_body(&mock_server).await;

    let (template, notify, submit, authorize_response) =
        load_valid_stratum_work_components("../p2poolv2_tests/test_data/validation/stratum/b/");

    let enonce1 = authorize_response.result.unwrap()[1].clone();
    let enonce1: &str = enonce1.as_str().unwrap();
    session.enonce1 =
        u32::from_le_bytes(hex::decode(enonce1).unwrap().as_slice().try_into().unwrap());
    session.enonce1_hex = enonce1.to_string();
    session.btcaddress = Some("tb1q3udk7r26qs32ltf9nmqrjaaa7tr55qmkk30q5d".to_string());
    session.user_id = Some(1);

    let job_id = JobId(u64::from_str_radix(&notify.params.job_id, 16).unwrap());

    let _ = tracker_handle.insert_job(
        Arc::new(template),
        notify.params.coinbase1.to_string(),
        notify.params.coinbase2.to_string(),
        None,
        TEST_COINBASE_NSECS,
        vec![],
        job_id,
    );

    let (emissions_tx, mut emissions_rx) = mpsc::channel(10);
    let (pplns_shares_tx, mut pplns_shares_rx) = mpsc::channel(10);
    let stats_dir = tempfile::tempdir().unwrap();
    let metrics_handle = metrics::start_metrics(stats_dir.path().to_str().unwrap().to_string())
        .await
        .unwrap();
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;

    let ctx = StratumContext {
        found_block_submitter: FoundBlockSubmitter::single_attempt(),
        tracker_handle: tracker_handle.clone(),
        bitcoindrpc_client: BitcoindRpcClient::new(
            &bitcoinrpc_config.url,
            &bitcoinrpc_config.username,
            &bitcoinrpc_config.password,
        )
        .unwrap(),
        start_difficulty: 10_000,
        minimum_difficulty: 1,
        maximum_difficulty: None,
        ignore_difficulty: true,
        validate_addresses: true,
        emissions_tx,
        pplns_shares_tx,
        network: bitcoin::network::Network::Signet,
        metrics: metrics_handle.clone(),
        chain_store_handle,
        mode: PoolMode::Hydrapool,
        miner_address: None,
        share_chain: crate::config::Chain::Bitcoin,
        bch_share_network: None,
        cash_share: None,
        template_refresh_tx: None,
        merge_mining_logs: None,
    };

    let messages = handle_submit(submit, &mut session, ctx).await.unwrap();
    let response = match &messages[..] {
        [Message::Response(response)] => response,
        _ => panic!("expected Response"),
    };
    assert_eq!(response.result, Some(json!(true)));

    assert!(
        pplns_shares_rx.try_recv().is_err(),
        "hydrapool share below session difficulty must not be stored"
    );
    assert!(
        emissions_rx.try_recv().is_err(),
        "hydrapool share below session difficulty must not be emitted"
    );
    assert_eq!(metrics_handle.get_metrics().await.accepted_total, 1);
}

/// A share on a token job that meets token A's target wins it: a miner who
/// asked for `pickaxe-merge` gets the proof after the acknowledgement as a
/// `pickaxe.merge_proof`, which verifies against their leaf as a covenant
/// would, and the node logs the win.
#[tokio::test]
async fn a_winning_share_sends_its_miner_the_token_proof() {
    let mut session = Session::<DifficultyAdjuster>::new(1, 1, None, DEFAULT_VERSION_MASK);
    session.subscribed = true;
    session.user_id = Some(1);
    session.merge_disclosure = true;
    let tracker_handle = start_tracker_actor();
    let (submit, job_id) = build_token_win_submit(&tracker_handle, &session.enonce1_hex);
    let tokens = tracker_handle
        .get_job(job_id)
        .unwrap()
        .merge_tokens
        .unwrap();

    let (mock_server, bitcoinrpc_config) = setup_mock_bitcoin_rpc().await;
    mock_submit_block_with_any_body(&mock_server).await;
    let (emissions_tx, _emissions_rx) = mpsc::channel(10);
    let (pplns_shares_tx, _pplns_shares_rx) = mpsc::channel(10);
    let stats_dir = tempfile::tempdir().unwrap();
    let metrics_handle = metrics::start_metrics(stats_dir.path().to_str().unwrap().to_string())
        .await
        .unwrap();
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let log_directory = tempfile::tempdir().unwrap();
    let (leaves, leaves_writer) = LineLog::start(
        log_directory.path().join("leaves.jsonl"),
        crate::stratum::merge_disclosure::MAX_LOG_BYTES,
    );
    let (wins, wins_writer) = LineLog::start(
        log_directory.path().join("wins.jsonl"),
        crate::stratum::merge_disclosure::MAX_LOG_BYTES,
    );

    let ctx = StratumContext {
        found_block_submitter: FoundBlockSubmitter::single_attempt(),
        tracker_handle: tracker_handle.clone(),
        bitcoindrpc_client: BitcoindRpcClient::new(
            &bitcoinrpc_config.url,
            &bitcoinrpc_config.username,
            &bitcoinrpc_config.password,
        )
        .unwrap(),
        start_difficulty: 1,
        minimum_difficulty: 1,
        maximum_difficulty: None,
        ignore_difficulty: false,
        validate_addresses: true,
        emissions_tx,
        pplns_shares_tx,
        network: bitcoin::network::Network::Regtest,
        metrics: metrics_handle,
        chain_store_handle,
        mode: PoolMode::P2poolv2,
        miner_address: None,
        share_chain: crate::config::Chain::BitcoinCash,
        bch_share_network: Some(p2poolv2_wallet::bch_share::BchShareNetwork::Regtest),
        cash_share: None,
        template_refresh_tx: None,
        merge_mining_logs: Some(MergeMiningLogs::from_logs(leaves, wins)),
    };

    let messages = handle_submit(submit, &mut session, ctx).await.unwrap();

    assert!(matches!(messages.first(), Some(Message::Response(_))));
    let Some(Message::Extension(proof)) = messages.last() else {
        panic!("expected the token proof after the acknowledgement");
    };
    assert_eq!(proof.method, "pickaxe.merge_proof");
    assert_eq!(proof.params[0], format!("{:016x}", job_id.0));
    assert_eq!(proof.params[1], hex::encode([0x11; 32]));
    let aux_proof = crate::shares::auxpow::AuxProof::from_bytes(
        &hex::decode(proof.params[2].as_str().unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(aux_proof.verify(&tokens.entries[0].leaf, 16, None), Ok(()));
    assert_eq!(
        proof.params[3],
        hex::encode(tokens.entries[0].split.preimage())
    );
    leaves_writer.join().unwrap();
    wins_writer.join().unwrap();
    let logged = std::fs::read_to_string(log_directory.path().join("wins.jsonl")).unwrap();
    let line: serde_json::Value = serde_json::from_str(logged.lines().next().unwrap()).unwrap();
    assert_eq!(line["job"], format!("{:016x}", job_id.0));
    assert_eq!(line["proof"], proof.params[2]);
}

/// A miner who did not ask for `pickaxe-merge` gets no proof message: their
/// firmware sees nothing new.
#[tokio::test]
async fn a_winning_share_without_the_disclosure_sends_nothing_new() {
    let mut session = Session::<DifficultyAdjuster>::new(1, 1, None, DEFAULT_VERSION_MASK);
    session.subscribed = true;
    session.user_id = Some(1);
    let tracker_handle = start_tracker_actor();
    let (submit, _job_id) = build_token_win_submit(&tracker_handle, &session.enonce1_hex);

    let (mock_server, bitcoinrpc_config) = setup_mock_bitcoin_rpc().await;
    mock_submit_block_with_any_body(&mock_server).await;
    let (emissions_tx, _emissions_rx) = mpsc::channel(10);
    let (pplns_shares_tx, _pplns_shares_rx) = mpsc::channel(10);
    let stats_dir = tempfile::tempdir().unwrap();
    let metrics_handle = metrics::start_metrics(stats_dir.path().to_str().unwrap().to_string())
        .await
        .unwrap();
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let ctx = StratumContext {
        found_block_submitter: FoundBlockSubmitter::single_attempt(),
        tracker_handle: tracker_handle.clone(),
        bitcoindrpc_client: BitcoindRpcClient::new(
            &bitcoinrpc_config.url,
            &bitcoinrpc_config.username,
            &bitcoinrpc_config.password,
        )
        .unwrap(),
        start_difficulty: 1,
        minimum_difficulty: 1,
        maximum_difficulty: None,
        ignore_difficulty: false,
        validate_addresses: true,
        emissions_tx,
        pplns_shares_tx,
        network: bitcoin::network::Network::Regtest,
        metrics: metrics_handle,
        chain_store_handle,
        mode: PoolMode::P2poolv2,
        miner_address: None,
        share_chain: crate::config::Chain::BitcoinCash,
        bch_share_network: Some(p2poolv2_wallet::bch_share::BchShareNetwork::Regtest),
        cash_share: None,
        template_refresh_tx: None,
        merge_mining_logs: None,
    };

    let messages = handle_submit(submit, &mut session, ctx).await.unwrap();

    assert!(
        !messages
            .iter()
            .any(|message| matches!(message, Message::Extension(_)))
    );
}
