use super::*;
use crate::accounting::OutputPair;
use crate::accounting::stats::metrics;
use crate::shares::coinbase_proof::CoinbaseProof;
use crate::stratum::sv2::session::encode_message;
use crate::stratum::work::block_template::BlockTemplate;
use crate::stratum::work::prepared_notify::PreparedNotifyParamsBuilder;
use crate::stratum::work::tracker::start_tracker_actor;
use crate::test_utils::{make_test_share_address, setup_test_chain_store_handle};
use crate::test_utils::{
    test_bch_custom_job, test_bch_prepared, test_bch_prepared_for_template,
    test_bch_solve_custom_job, test_bch_stratum_context,
};
use bitcoin::consensus::deserialize;
use bitcoin::hashes::sha256d;
use bitcoin::{Address, Amount, CompressedPublicKey, Network, Transaction};
use bitcoindrpc::BitcoindRpcClient;
use bitcoindrpc::test_utils::{mock_submit_block_with_any_body, setup_mock_bitcoin_rpc};
use p2poolv2_config::DEFAULT_VERSION_MASK;
use p2poolv2_wallet::bch_share::encode_payout_address;
use p2poolv2_wallet::bch_share::{BchShareAddress, BchShareNetwork};
use tokio::sync::mpsc;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};

const USERNAME: &str = "tb1q3udk7r26qs32ltf9nmqrjaaa7tr55qmkk30q5d.rig1";
/// Easier than almost every hash, so the test share meets the pool target.
const EASY_POOL_BITS: u32 = 0x2100ffff;

#[tokio::test]
async fn zero_maximum_target_is_refused_at_channel_opening() {
    let (ctx, _emissions, _dirs) = test_bch_stratum_context("http://127.0.0.1:1").await;
    let mut channels = MiningChannels::new(ctx, DEFAULT_VERSION_MASK);
    let mut request = binary_sv2::to_bytes(OpenExtendedMiningChannel {
        request_id: 1,
        user_identity: "unused".try_into().unwrap(),
        nominal_hash_rate: 1.0,
        max_target: U256::new(&[0; 32]).unwrap(),
        min_extranonce_size: 8,
    })
    .unwrap();
    let rejected = channels
        .handle(
            MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL,
            &mut request,
            None,
        )
        .await
        .unwrap();
    let mut rejected_payload = payload(&rejected[0]);
    let error = OpenMiningChannelError::from_bytes(&mut rejected_payload).unwrap();
    assert_eq!(
        error.error_code.as_utf8_or_hex(),
        ERROR_CODE_OPEN_MINING_CHANNEL_MAX_TARGET_OUT_OF_RANGE
    );
    assert!(channels.channels.is_empty());
}

#[tokio::test]
async fn active_custom_job_keeps_its_share_credit_after_difficulty_changes() {
    let (mut ctx, mut emissions, _dirs) = test_bch_stratum_context("http://127.0.0.1:1").await;
    ctx.start_difficulty = 5;
    let mut channels = MiningChannels::new(ctx, DEFAULT_VERSION_MASK);
    let payout = Address::p2pkh(
        bitcoin::PubkeyHash::from_byte_array([0x22; 20]),
        Network::Regtest,
    );
    let identity = encode_payout_address(&payout, BchShareNetwork::Regtest).unwrap();
    let mut open = binary_sv2::to_bytes(OpenExtendedMiningChannel {
        request_id: 1,
        user_identity: identity.as_str().try_into().unwrap(),
        // No declared rate, so the channel starts at the node's
        // start difficulty.
        nominal_hash_rate: 0.0,
        max_target: U256::new(&[0xff; 32]).unwrap(),
        min_extranonce_size: 8,
    })
    .unwrap();
    let opened = channels
        .handle(MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL, &mut open, None)
        .await
        .unwrap();
    let mut open_payload = payload(&opened[0]);
    let opened = OpenExtendedMiningChannelSuccess::from_bytes(&mut open_payload).unwrap();
    let prepared = test_bch_prepared();
    let token = prepared
        .mining_job_token([1; 32], prepared.template().curtime)
        .unwrap();
    let (mut bytes, declared) = test_bch_custom_job(
        Arc::clone(&prepared),
        token,
        vec![],
        false,
        prepared.template().coinbasevalue,
    );
    let request = SetCustomMiningJob::from_bytes(&mut bytes).unwrap();
    let installed = channels
        .install_custom_job(&request, declared, Some(prepared))
        .await
        .unwrap();
    let mut install_payload = payload(&installed[0]);
    let installed = SetCustomMiningJobSuccess::from_bytes(&mut install_payload).unwrap();
    channels
        .channels
        .get_mut(&1)
        .unwrap()
        .session
        .difficulty_adjuster
        .current_difficulty = 99;
    let next_target = target_for_difficulty(1000);
    channels.update(1, &next_target.to_le_bytes()).unwrap();
    assert_eq!(
        channels.channels[&1].jobs[&installed.job_id].target,
        target_for_difficulty(5)
    );
    let header = test_bch_solve_custom_job(&request, opened.extranonce_prefix.as_bytes());
    let mut submit = binary_sv2::to_bytes(SubmitSharesExtended {
        channel_id: 1,
        sequence_number: 1,
        job_id: installed.job_id,
        nonce: header.nonce,
        ntime: header.time,
        version: request.version,
        extranonce: B032::new(&[0; 8]).unwrap(),
    })
    .unwrap();
    let accepted = channels
        .handle(MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED, &mut submit, None)
        .await
        .unwrap();
    let mut accepted_payload = payload(&accepted[0]);
    let accepted = SubmitSharesSuccess::from_bytes(&mut accepted_payload).unwrap();
    assert_eq!(accepted.new_shares_sum, 5);
    // The share chain share carries no PPLNS credit; the reply above is
    // where the job's difficulty shows.
    assert_eq!(emissions.try_recv().unwrap().header, header);
    channels.on_template(&test_bch_prepared());
    let next_job = channels.channels[&1].jobs.values().next_back().unwrap();
    assert_eq!(next_job.target, next_target);
    assert_eq!(next_job.difficulty, 1000);
}

#[tokio::test]
async fn node_rejection_does_not_install_a_custom_job() {
    let rpc = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"result": "bad-cb-amount", "error": null, "id": 0}),
            ),
        )
        .expect(1)
        .mount(&rpc)
        .await;
    let (ctx, _emissions, _dirs) = test_bch_stratum_context(&rpc.uri()).await;
    let mut channels = MiningChannels::new(ctx, DEFAULT_VERSION_MASK);
    let payout = Address::p2pkh(
        bitcoin::PubkeyHash::from_byte_array([0x22; 20]),
        Network::Regtest,
    );
    let identity = encode_payout_address(&payout, BchShareNetwork::Regtest).unwrap();
    let mut open = binary_sv2::to_bytes(OpenExtendedMiningChannel {
        request_id: 1,
        user_identity: identity.as_str().try_into().unwrap(),
        nominal_hash_rate: 1.0,
        max_target: U256::new(&[0xff; 32]).unwrap(),
        min_extranonce_size: 8,
    })
    .unwrap();
    channels
        .handle(MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL, &mut open, None)
        .await
        .unwrap();
    let prepared = test_bch_prepared();
    let token = prepared
        .mining_job_token([1; 32], prepared.template().curtime)
        .unwrap();
    let (mut bytes, declared) = test_bch_custom_job(
        Arc::clone(&prepared),
        token,
        vec![],
        true,
        prepared.template().coinbasevalue,
    );
    let request = SetCustomMiningJob::from_bytes(&mut bytes).unwrap();
    let rejected = channels
        .install_custom_job(&request, declared, Some(prepared))
        .await
        .unwrap();
    let mut error_payload = payload(&rejected[0]);
    let error = SetCustomMiningJobError::from_bytes(&mut error_payload).unwrap();
    assert_eq!(error.error_code.as_utf8_or_hex(), "invalid-template");
    assert!(channels.channels[&1].jobs.is_empty());
}

#[tokio::test]
async fn new_bits_on_the_same_parent_send_a_new_prev_hash() {
    let (ctx, _emissions, _dirs) = test_bch_stratum_context("http://127.0.0.1:1").await;
    let mut channels = MiningChannels::new(ctx, DEFAULT_VERSION_MASK);
    let payout = Address::p2pkh(
        bitcoin::PubkeyHash::from_byte_array([0x22; 20]),
        Network::Regtest,
    );
    let identity = encode_payout_address(&payout, BchShareNetwork::Regtest).unwrap();
    let mut open = binary_sv2::to_bytes(OpenExtendedMiningChannel {
        request_id: 1,
        user_identity: identity.as_str().try_into().unwrap(),
        nominal_hash_rate: 1.0,
        max_target: U256::new(&[0xff; 32]).unwrap(),
        min_extranonce_size: 8,
    })
    .unwrap();
    channels
        .handle(
            MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL,
            &mut open,
            Some(test_bch_prepared()),
        )
        .await
        .unwrap();
    // Chipnet switches to the minimum difficulty 20 minutes after the
    // parent: same parent, other bits.
    let mut easier: BlockTemplate = serde_json::from_str(include_str!(
        "../../../../../bitcoindrpc/test_data/bchn-regtest-template.json"
    ))
    .unwrap();
    easier.bits = "1e0ffff0".to_string();
    let frames = channels.on_template(&test_bch_prepared_for_template(easier));
    assert_eq!(frames.len(), 2);
    assert_eq!(frame_type(&frames[0]), MESSAGE_TYPE_NEW_EXTENDED_MINING_JOB);
    let mut job_payload = payload(&frames[0]);
    let job = NewExtendedMiningJob::from_bytes(&mut job_payload).unwrap();
    assert!(job.min_ntime.into_inner().is_none());
    assert_eq!(
        frame_type(&frames[1]),
        MESSAGE_TYPE_MINING_SET_NEW_PREV_HASH
    );
    let mut prev_hash_payload = payload(&frames[1]);
    let prev_hash = SetNewPrevHash::from_bytes(&mut prev_hash_payload).unwrap();
    assert_eq!(prev_hash.nbits, 0x1e0f_fff0);
}

#[tokio::test]
async fn a_zero_version_mask_does_not_allow_version_rolling() {
    let (ctx, _emissions, _dirs) = test_bch_stratum_context("http://127.0.0.1:1").await;
    let mut channels = MiningChannels::new(ctx, 0);
    let payout = Address::p2pkh(
        bitcoin::PubkeyHash::from_byte_array([0x22; 20]),
        Network::Regtest,
    );
    let identity = encode_payout_address(&payout, BchShareNetwork::Regtest).unwrap();
    let mut open = binary_sv2::to_bytes(OpenExtendedMiningChannel {
        request_id: 1,
        user_identity: identity.as_str().try_into().unwrap(),
        nominal_hash_rate: 1.0,
        max_target: U256::new(&[0xff; 32]).unwrap(),
        min_extranonce_size: 8,
    })
    .unwrap();
    let opened = channels
        .handle(
            MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL,
            &mut open,
            Some(test_bch_prepared()),
        )
        .await
        .unwrap();
    assert_eq!(frame_type(&opened[1]), MESSAGE_TYPE_NEW_EXTENDED_MINING_JOB);
    let mut job_payload = payload(&opened[1]);
    let job = NewExtendedMiningJob::from_bytes(&mut job_payload).unwrap();
    assert!(!job.version_rolling_allowed);
}

fn frame_type(frame: &[u8]) -> u8 {
    frame[2]
}

fn frame_has_channel_bit(frame: &[u8]) -> bool {
    u16::from_le_bytes([frame[0], frame[1]]) & 0x8000 != 0
}

fn payload(frame: &[u8]) -> Vec<u8> {
    frame[6..].to_vec()
}

#[test]
fn difficulty_one_is_the_mainnet_difficulty_one_target() {
    assert_eq!(target_for_difficulty(1), Target::MAX_ATTAINABLE_MAINNET);
    assert_eq!(target_for_difficulty(0), Target::MAX_ATTAINABLE_MAINNET);
}

#[test]
fn difficulty_two_halves_the_target() {
    let half = target_for_difficulty(2).to_be_bytes();
    let full = Target::MAX_ATTAINABLE_MAINNET.to_be_bytes();
    assert_eq!(half[4], full[4] >> 1);
    assert_eq!(half[5], (full[4] << 7) | (full[5] >> 1));
}

/// The target of the share difficulty ceiling is never harder than the
/// share target, so a channel at the ceiling loses no share-chain share.
#[test]
fn the_mini_chain_ceiling_target_is_no_harder_than_its_share_target() {
    let share_bits = CompactTarget::from_consensus(0x1c38_4bd7);
    let ceiling = share_difficulty_ceiling(share_bits).unwrap();

    assert!(target_for_difficulty(ceiling) >= Target::from_compact(share_bits));
    assert!(target_for_difficulty(ceiling + 1) < Target::from_compact(share_bits));
}

#[test]
fn the_main_chain_ceiling_target_is_no_harder_than_its_share_target() {
    let share_bits = CompactTarget::from_consensus(0x1b38_4bd7);
    let ceiling = share_difficulty_ceiling(share_bits).unwrap();

    assert!(target_for_difficulty(ceiling) >= Target::from_compact(share_bits));
    assert!(target_for_difficulty(ceiling + 1) < Target::from_compact(share_bits));
}

/// A device declaring 1 PH/s would settle near difficulty 776,000, far
/// above the share difficulty of the job it is about to get (1 here), so
/// the channel opens at the share difficulty and needs no `SetTarget`.
#[tokio::test]
async fn a_fast_channel_opens_at_the_share_difficulty_of_its_first_job() {
    let (ctx, _emissions, _dirs) = test_bch_stratum_context("http://127.0.0.1:1").await;
    let mut channels = MiningChannels::new(ctx, DEFAULT_VERSION_MASK);
    let payout = Address::p2pkh(
        bitcoin::PubkeyHash::from_byte_array([0x22; 20]),
        Network::Regtest,
    );
    let identity = encode_payout_address(&payout, BchShareNetwork::Regtest).unwrap();
    let mut open = binary_sv2::to_bytes(OpenExtendedMiningChannel {
        request_id: 1,
        user_identity: identity.as_str().try_into().unwrap(),
        nominal_hash_rate: 1.0e15,
        max_target: U256::new(&[0xff; 32]).unwrap(),
        min_extranonce_size: 8,
    })
    .unwrap();

    let opened = channels
        .handle(
            MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL,
            &mut open,
            Some(test_bch_prepared()),
        )
        .await
        .unwrap();

    let mut open_payload = payload(&opened[0]);
    let success = OpenExtendedMiningChannelSuccess::from_bytes(&mut open_payload).unwrap();
    assert_eq!(
        success.target.as_bytes(),
        &Target::MAX_ATTAINABLE_MAINNET.to_le_bytes()
    );
    assert_eq!(frame_type(&opened[1]), MESSAGE_TYPE_NEW_EXTENDED_MINING_JOB);
    assert_eq!(
        channels.channels[&1]
            .session
            .difficulty_adjuster
            .get_current_difficulty(),
        1
    );
}

/// A channel above the share difficulty of a new job comes down to it, and
/// the `SetTarget` saying so goes before the job.
#[tokio::test]
async fn a_channel_above_the_share_difficulty_gets_set_target_before_the_job() {
    let (ctx, _emissions, _dirs) = test_bch_stratum_context("http://127.0.0.1:1").await;
    let mut channels = MiningChannels::new(ctx, DEFAULT_VERSION_MASK);
    let payout = Address::p2pkh(
        bitcoin::PubkeyHash::from_byte_array([0x22; 20]),
        Network::Regtest,
    );
    let identity = encode_payout_address(&payout, BchShareNetwork::Regtest).unwrap();
    let mut open = binary_sv2::to_bytes(OpenExtendedMiningChannel {
        request_id: 1,
        user_identity: identity.as_str().try_into().unwrap(),
        nominal_hash_rate: 1.0e15,
        max_target: U256::new(&[0xff; 32]).unwrap(),
        min_extranonce_size: 8,
    })
    .unwrap();
    channels
        .handle(MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL, &mut open, None)
        .await
        .unwrap();
    assert!(
        channels.channels[&1]
            .session
            .difficulty_adjuster
            .get_current_difficulty()
            > 1
    );

    let frames = channels.on_template(&test_bch_prepared());

    assert_eq!(frame_type(&frames[0]), MESSAGE_TYPE_SET_TARGET);
    let mut target_payload = payload(&frames[0]);
    let set_target = SetTarget::from_bytes(&mut target_payload).unwrap();
    assert_eq!(
        set_target.maximum_target.as_bytes(),
        &Target::MAX_ATTAINABLE_MAINNET.to_le_bytes()
    );
    assert_eq!(frame_type(&frames[1]), MESSAGE_TYPE_NEW_EXTENDED_MINING_JOB);
    let channel = &channels.channels[&1];
    assert_eq!(
        channel.session.difficulty_adjuster.get_current_difficulty(),
        1
    );
    let job = channel.jobs.values().next().unwrap();
    assert_eq!(job.target, Target::MAX_ATTAINABLE_MAINNET);
}

#[tokio::test]
async fn extended_channel_mines_the_job_it_is_sent_and_its_share_is_emitted() {
    let template: BlockTemplate = serde_json::from_str(include_str!(
        "../../../../../p2poolv2_tests/test_data/gbt/regtest/ckpool/one-txn/gbt.json"
    ))
    .unwrap();
    let payout_key: CompressedPublicKey =
        "020202020202020202020202020202020202020202020202020202020202020202"
            .parse()
            .unwrap();
    let prepared = Arc::new(
        PreparedNotifyParamsBuilder::new(
            Arc::new(template.clone()),
            vec![OutputPair {
                address: Address::p2wpkh(&payout_key, Network::Signet),
                amount: Amount::from_sat(template.coinbasevalue),
            }],
            b"P2Poolv2",
            true,
        )
        .bits(CompactTarget::from_consensus(EASY_POOL_BITS))
        .build()
        .unwrap(),
    );
    let (emissions_tx, mut emissions_rx) = mpsc::channel(10);
    let stats_dir = tempfile::tempdir().unwrap();
    let metrics_handle = metrics::start_metrics(stats_dir.path().to_str().unwrap().to_string())
        .await
        .unwrap();
    let (chain_store_handle, _store_dir) = setup_test_chain_store_handle(true).await;
    let ctx = StratumContext {
        extra_submit_clients: Arc::from([]),
        tracker_handle: start_tracker_actor(),
        bitcoindrpc_client: BitcoindRpcClient::new("http://127.0.0.1:1", "user", "pass").unwrap(),
        start_difficulty: 1,
        minimum_difficulty: 1,
        maximum_difficulty: None,
        ignore_difficulty: true,
        validate_addresses: true,
        emissions_tx,
        // P2Poolv2 mode never queues a Hydrapool PPLNS share.
        pplns_shares_tx: mpsc::channel(10).0,
        network: Network::Signet,
        metrics: metrics_handle,
        chain_store_handle,
        mode: PoolMode::P2poolv2,
        miner_address: Some(make_test_share_address(1, Network::Signet)),
        share_chain: Chain::Bitcoin,
        bch_share_network: None,
        cash_share: None,
        template_refresh_tx: None,
    };
    let mut channels = MiningChannels::new(ctx, DEFAULT_VERSION_MASK);

    let mut open = payload(
        &encode_message(
            OpenExtendedMiningChannel {
                request_id: 5,
                user_identity: USERNAME.try_into().unwrap(),
                nominal_hash_rate: 1.0,
                max_target: U256::new(&[0xff; 32]).unwrap(),
                min_extranonce_size: 8,
            },
            MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL,
        )
        .unwrap(),
    );
    let opened = channels
        .handle(
            MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL,
            &mut open,
            Some(Arc::clone(&prepared)),
        )
        .await
        .unwrap();
    assert_eq!(opened.len(), 3);
    assert_eq!(
        frame_type(&opened[0]),
        MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL_SUCCESS
    );
    assert_eq!(frame_type(&opened[1]), MESSAGE_TYPE_NEW_EXTENDED_MINING_JOB);
    assert_eq!(
        frame_type(&opened[2]),
        MESSAGE_TYPE_MINING_SET_NEW_PREV_HASH
    );
    assert!(!frame_has_channel_bit(&opened[0]));
    assert!(frame_has_channel_bit(&opened[1]));
    assert!(frame_has_channel_bit(&opened[2]));

    let mut success_payload = payload(&opened[0]);
    let success = OpenExtendedMiningChannelSuccess::from_bytes(&mut success_payload).unwrap();
    assert_eq!(success.request_id, 5);
    assert_eq!(success.extranonce_size, EXTRANONCE_SIZE);
    let channel_id = success.channel_id;
    let extranonce_prefix = success.extranonce_prefix.as_bytes().to_vec();
    let mut job_payload = payload(&opened[1]);
    let job = NewExtendedMiningJob::from_bytes(&mut job_payload).unwrap();
    assert!(job.min_ntime.clone().into_inner().is_none());
    let mut prev_hash_payload = payload(&opened[2]);
    let prev_hash = SetNewPrevHash::from_bytes(&mut prev_hash_payload).unwrap();
    assert_eq!(prev_hash.job_id, job.job_id);

    // Rebuild the header the way an SV2 miner does.
    let extranonce = [0x11u8; 8];
    let mut coinbase_bytes = job.coinbase_tx_prefix.as_bytes().to_vec();
    coinbase_bytes.extend_from_slice(&extranonce_prefix);
    coinbase_bytes.extend_from_slice(&extranonce);
    coinbase_bytes.extend_from_slice(job.coinbase_tx_suffix.as_bytes());
    let coinbase: Transaction = deserialize(&coinbase_bytes).unwrap();
    let path = job.merkle_path.clone().into_inner();
    let mut root = coinbase.compute_txid().to_byte_array();
    assert!(path.len() <= 1);
    if let Some(branch) = path.first() {
        let mut combined = Vec::with_capacity(64);
        combined.extend_from_slice(&root);
        combined.extend_from_slice(branch.as_bytes());
        root = sha256d::Hash::hash(&combined).to_byte_array();
    }
    let header = Header {
        version: Version::from_consensus(job.version as i32),
        prev_blockhash: BlockHash::from_byte_array(*prev_hash.prev_hash.as_array()),
        merkle_root: TxMerkleNode::from_byte_array(root),
        time: prev_hash.min_ntime,
        bits: CompactTarget::from_consensus(prev_hash.nbits),
        nonce: 7,
    };

    let mut submit = payload(
        &encode_message(
            SubmitSharesExtended {
                channel_id,
                sequence_number: 1,
                job_id: job.job_id,
                nonce: 7,
                ntime: prev_hash.min_ntime,
                version: job.version,
                extranonce: B032::new(&extranonce).unwrap(),
            },
            MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED,
        )
        .unwrap(),
    );
    let mut duplicate = submit.clone();
    channels
        .channels
        .get_mut(&channel_id)
        .unwrap()
        .session
        .difficulty_adjuster
        .current_difficulty = 77;
    let accepted = channels
        .handle(MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED, &mut submit, None)
        .await
        .unwrap();
    assert_eq!(frame_type(&accepted[0]), MESSAGE_TYPE_SUBMIT_SHARES_SUCCESS);
    let emission = emissions_rx.try_recv().unwrap();
    assert_eq!(emission.header, header);
    let mut accepted_payload = payload(&accepted[0]);
    assert_eq!(
        SubmitSharesSuccess::from_bytes(&mut accepted_payload)
            .unwrap()
            .new_shares_sum,
        1
    );
    assert_eq!(
        emission.share_commitment.miner_bitcoin_address.to_string(),
        "tb1q3udk7r26qs32ltf9nmqrjaaa7tr55qmkk30q5d"
    );
    let expected_proof = CoinbaseProof::from_coinbase(
        &emission.bitcoin_coinbase,
        emission.share_commitment.share_witness_root,
    )
    .unwrap();
    assert_eq!(emission.coinbase_proof, expected_proof);

    let repeated = channels
        .handle(MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED, &mut duplicate, None)
        .await
        .unwrap();
    let mut repeated_payload = payload(&repeated[0]);
    let repeated_error = SubmitSharesError::from_bytes(&mut repeated_payload).unwrap();
    assert_eq!(
        repeated_error.error_code.as_utf8_or_hex(),
        "duplicate-share"
    );

    let mut unknown = payload(
        &encode_message(
            SubmitSharesExtended {
                channel_id,
                sequence_number: 2,
                job_id: 99,
                nonce: 1,
                ntime: prev_hash.min_ntime,
                version: job.version,
                extranonce: B032::new(&extranonce).unwrap(),
            },
            MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED,
        )
        .unwrap(),
    );
    let unknown_response = channels
        .handle(MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED, &mut unknown, None)
        .await
        .unwrap();
    let mut unknown_payload = payload(&unknown_response[0]);
    let unknown_error = SubmitSharesError::from_bytes(&mut unknown_payload).unwrap();
    assert_eq!(unknown_error.error_code.as_utf8_or_hex(), "invalid-job-id");

    let mut rolled_outside_mask = payload(
        &encode_message(
            SubmitSharesExtended {
                channel_id,
                sequence_number: 3,
                job_id: job.job_id,
                nonce: 2,
                ntime: prev_hash.min_ntime,
                version: job.version ^ 0x0000_0001,
                extranonce: B032::new(&extranonce).unwrap(),
            },
            MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED,
        )
        .unwrap(),
    );
    let rolled_response = channels
        .handle(
            MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED,
            &mut rolled_outside_mask,
            None,
        )
        .await
        .unwrap();
    let mut rolled_payload = payload(&rolled_response[0]);
    let rolled_error = SubmitSharesError::from_bytes(&mut rolled_payload).unwrap();
    assert_eq!(
        rolled_error.error_code.as_utf8_or_hex(),
        ERROR_CODE_SUBMIT_SHARES_INVALID_NON_ROLLABLE_VERSION_BIT
    );

    // Same tip: the next template is an active job with no new prev hash.
    // The channel was set to difficulty 77 above, over this job's share
    // difficulty (1), so a `SetTarget` bringing it down comes first.
    let refreshed = channels.on_template(&prepared);
    assert_eq!(refreshed.len(), 2);
    assert_eq!(frame_type(&refreshed[0]), MESSAGE_TYPE_SET_TARGET);
    assert_eq!(
        frame_type(&refreshed[1]),
        MESSAGE_TYPE_NEW_EXTENDED_MINING_JOB
    );
    let mut refreshed_payload = payload(&refreshed[1]);
    let refreshed_job = NewExtendedMiningJob::from_bytes(&mut refreshed_payload).unwrap();
    assert_eq!(refreshed_job.min_ntime.into_inner(), Some(template.curtime));
}

#[tokio::test]
async fn standard_channel_mines_the_merkle_root_it_is_sent() {
    let template: BlockTemplate = serde_json::from_str(include_str!(
        "../../../../../p2poolv2_tests/test_data/gbt/regtest/ckpool/one-txn/gbt.json"
    ))
    .unwrap();
    let payout_key: CompressedPublicKey =
        "020202020202020202020202020202020202020202020202020202020202020202"
            .parse()
            .unwrap();
    let prepared = Arc::new(
        PreparedNotifyParamsBuilder::new(
            Arc::new(template.clone()),
            vec![OutputPair {
                address: Address::p2wpkh(&payout_key, Network::Signet),
                amount: Amount::from_sat(template.coinbasevalue),
            }],
            b"P2Poolv2",
            true,
        )
        .bits(CompactTarget::from_consensus(EASY_POOL_BITS))
        .build()
        .unwrap(),
    );
    let (emissions_tx, mut emissions_rx) = mpsc::channel(10);
    let stats_dir = tempfile::tempdir().unwrap();
    let metrics_handle = metrics::start_metrics(stats_dir.path().to_str().unwrap().to_string())
        .await
        .unwrap();
    let (chain_store_handle, _store_dir) = setup_test_chain_store_handle(true).await;
    let ctx = StratumContext {
        extra_submit_clients: Arc::from([]),
        tracker_handle: start_tracker_actor(),
        bitcoindrpc_client: BitcoindRpcClient::new("http://127.0.0.1:1", "user", "pass").unwrap(),
        start_difficulty: 1,
        minimum_difficulty: 1,
        maximum_difficulty: None,
        ignore_difficulty: true,
        validate_addresses: true,
        emissions_tx,
        // P2Poolv2 mode never queues a Hydrapool PPLNS share.
        pplns_shares_tx: mpsc::channel(10).0,
        network: Network::Signet,
        metrics: metrics_handle,
        chain_store_handle,
        mode: PoolMode::P2poolv2,
        miner_address: Some(make_test_share_address(1, Network::Signet)),
        share_chain: Chain::Bitcoin,
        bch_share_network: None,
        cash_share: None,
        template_refresh_tx: None,
    };
    let mut channels = MiningChannels::new(ctx, DEFAULT_VERSION_MASK);

    let mut open = payload(
        &encode_message(
            OpenStandardMiningChannel {
                request_id: 1,
                user_identity: USERNAME.try_into().unwrap(),
                nominal_hash_rate: 1.0,
                max_target: U256::new(&[0xff; 32]).unwrap(),
            },
            MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL,
        )
        .unwrap(),
    );
    let opened = channels
        .handle(
            MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL,
            &mut open,
            Some(Arc::clone(&prepared)),
        )
        .await
        .unwrap();
    assert_eq!(
        frame_type(&opened[0]),
        MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL_SUCCESS
    );
    let mut success_payload = payload(&opened[0]);
    let success = OpenStandardMiningChannelSuccess::from_bytes(&mut success_payload).unwrap();
    assert_eq!(success.extranonce_prefix.as_bytes().len(), 12);
    let channel_id = success.channel_id;
    assert_eq!(frame_type(&opened[1]), MESSAGE_TYPE_NEW_MINING_JOB);
    let mut job_payload = payload(&opened[1]);
    let job = NewMiningJob::from_bytes(&mut job_payload).unwrap();
    let mut prev_hash_payload = payload(&opened[2]);
    let prev_hash = SetNewPrevHash::from_bytes(&mut prev_hash_payload).unwrap();
    let header = Header {
        version: Version::from_consensus(job.version as i32),
        prev_blockhash: BlockHash::from_byte_array(*prev_hash.prev_hash.as_array()),
        merkle_root: TxMerkleNode::from_byte_array(*job.merkle_root.as_array()),
        time: prev_hash.min_ntime,
        bits: CompactTarget::from_consensus(prev_hash.nbits),
        nonce: 3,
    };

    let mut submit = payload(
        &encode_message(
            SubmitSharesStandard {
                channel_id,
                sequence_number: 1,
                job_id: job.job_id,
                nonce: 3,
                ntime: prev_hash.min_ntime,
                version: job.version,
            },
            MESSAGE_TYPE_SUBMIT_SHARES_STANDARD,
        )
        .unwrap(),
    );
    let accepted = channels
        .handle(MESSAGE_TYPE_SUBMIT_SHARES_STANDARD, &mut submit, None)
        .await
        .unwrap();
    assert_eq!(frame_type(&accepted[0]), MESSAGE_TYPE_SUBMIT_SHARES_SUCCESS);
    let emission = emissions_rx.try_recv().unwrap();
    assert_eq!(emission.header, header);
}

#[tokio::test]
async fn a_share_below_the_channel_target_keeps_the_block_submission_policy() {
    let mut template: BlockTemplate = serde_json::from_str(include_str!(
        "../../../../../p2poolv2_tests/test_data/gbt/regtest/ckpool/one-txn/gbt.json"
    ))
    .unwrap();
    // A network target easier than almost every hash, so the share is a block.
    template.bits = "2100ffff".to_string();
    let payout_key: CompressedPublicKey =
        "020202020202020202020202020202020202020202020202020202020202020202"
            .parse()
            .unwrap();
    let prepared = Arc::new(
        PreparedNotifyParamsBuilder::new(
            Arc::new(template.clone()),
            vec![OutputPair {
                address: Address::p2wpkh(&payout_key, Network::Signet),
                amount: Amount::from_sat(template.coinbasevalue),
            }],
            b"P2Poolv2",
            true,
        )
        .bits(CompactTarget::from_consensus(EASY_POOL_BITS))
        .build()
        .unwrap(),
    );
    let (mock_server, bitcoinrpc_config) = setup_mock_bitcoin_rpc().await;
    mock_submit_block_with_any_body(&mock_server).await;
    let (emissions_tx, mut emissions_rx) = mpsc::channel(10);
    let stats_dir = tempfile::tempdir().unwrap();
    let metrics_handle = metrics::start_metrics(stats_dir.path().to_str().unwrap().to_string())
        .await
        .unwrap();
    let (chain_store_handle, _store_dir) = setup_test_chain_store_handle(true).await;
    let ctx = StratumContext {
        extra_submit_clients: Arc::from([]),
        tracker_handle: start_tracker_actor(),
        bitcoindrpc_client: BitcoindRpcClient::new(
            &bitcoinrpc_config.url,
            &bitcoinrpc_config.username,
            &bitcoinrpc_config.password,
        )
        .unwrap(),
        // A channel target far harder than the test share.
        start_difficulty: 1_000_000_000_000,
        minimum_difficulty: 1,
        maximum_difficulty: None,
        ignore_difficulty: false,
        validate_addresses: true,
        emissions_tx,
        // P2Poolv2 mode never queues a Hydrapool PPLNS share.
        pplns_shares_tx: mpsc::channel(10).0,
        network: Network::Signet,
        metrics: metrics_handle,
        chain_store_handle,
        mode: PoolMode::P2poolv2,
        miner_address: Some(make_test_share_address(1, Network::Signet)),
        share_chain: Chain::Bitcoin,
        bch_share_network: None,
        cash_share: None,
        template_refresh_tx: None,
    };
    let mut channels = MiningChannels::new(ctx, DEFAULT_VERSION_MASK);
    let mut open = payload(
        &encode_message(
            OpenStandardMiningChannel {
                request_id: 1,
                user_identity: USERNAME.try_into().unwrap(),
                nominal_hash_rate: 1.0,
                max_target: U256::new(&[0xff; 32]).unwrap(),
            },
            MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL,
        )
        .unwrap(),
    );
    let opened = channels
        .handle(
            MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL,
            &mut open,
            Some(Arc::clone(&prepared)),
        )
        .await
        .unwrap();
    let mut success_payload = payload(&opened[0]);
    let channel_id = OpenStandardMiningChannelSuccess::from_bytes(&mut success_payload)
        .unwrap()
        .channel_id;
    let mut job_payload = payload(&opened[1]);
    let job = NewMiningJob::from_bytes(&mut job_payload).unwrap();
    let mut prev_hash_payload = payload(&opened[2]);
    let prev_hash = SetNewPrevHash::from_bytes(&mut prev_hash_payload).unwrap();

    let mut submit = payload(
        &encode_message(
            SubmitSharesStandard {
                channel_id,
                sequence_number: 1,
                job_id: job.job_id,
                nonce: 0,
                ntime: prev_hash.min_ntime,
                version: job.version,
            },
            MESSAGE_TYPE_SUBMIT_SHARES_STANDARD,
        )
        .unwrap(),
    );
    let response = channels
        .handle(MESSAGE_TYPE_SUBMIT_SHARES_STANDARD, &mut submit, None)
        .await
        .unwrap();
    let mut error_payload = payload(&response[0]);
    let error = SubmitSharesError::from_bytes(&mut error_payload).unwrap();
    assert_eq!(error.error_code.as_utf8_or_hex(), "difficulty-too-low");
    let requests = mock_server.received_requests().await.unwrap();
    assert_eq!(requests.len(), usize::from(!cfg!(feature = "sim")));
    if let Some(request) = requests.first() {
        assert!(String::from_utf8_lossy(&request.body).contains("submitblock"));
    }
    assert!(emissions_rx.try_recv().is_ok());
}

/// Hydrapool stores an SV2 share for PPLNS at the job's channel
/// difficulty when its hash meets the channel target, as SV1 does with
/// the session difficulty, and never emits it to the share chain.
#[tokio::test]
async fn a_hydrapool_share_meeting_the_channel_target_is_queued_at_the_job_difficulty() {
    let template: BlockTemplate = serde_json::from_str(include_str!(
        "../../../../../p2poolv2_tests/test_data/gbt/regtest/ckpool/one-txn/gbt.json"
    ))
    .unwrap();
    let payout_key: CompressedPublicKey =
        "020202020202020202020202020202020202020202020202020202020202020202"
            .parse()
            .unwrap();
    let prepared = Arc::new(
        PreparedNotifyParamsBuilder::new(
            Arc::new(template.clone()),
            vec![OutputPair {
                address: Address::p2wpkh(&payout_key, Network::Signet),
                amount: Amount::from_sat(template.coinbasevalue),
            }],
            b"P2Poolv2",
            true,
        )
        .build()
        .unwrap(),
    );
    let (mock_server, bitcoinrpc_config) = setup_mock_bitcoin_rpc().await;
    mock_submit_block_with_any_body(&mock_server).await;
    let (emissions_tx, mut emissions_rx) = mpsc::channel(10);
    let (pplns_shares_tx, mut pplns_shares_rx) = mpsc::channel(10);
    let stats_dir = tempfile::tempdir().unwrap();
    let metrics_handle = metrics::start_metrics(stats_dir.path().to_str().unwrap().to_string())
        .await
        .unwrap();
    let (chain_store_handle, _store_dir) = setup_test_chain_store_handle(true).await;
    let ctx = StratumContext {
        extra_submit_clients: Arc::from([]),
        tracker_handle: start_tracker_actor(),
        bitcoindrpc_client: BitcoindRpcClient::new(
            &bitcoinrpc_config.url,
            &bitcoinrpc_config.username,
            &bitcoinrpc_config.password,
        )
        .unwrap(),
        start_difficulty: 7,
        minimum_difficulty: 1,
        maximum_difficulty: None,
        ignore_difficulty: false,
        validate_addresses: true,
        emissions_tx,
        pplns_shares_tx,
        network: Network::Signet,
        metrics: metrics_handle,
        chain_store_handle,
        mode: PoolMode::Hydrapool,
        miner_address: None,
        share_chain: Chain::Bitcoin,
        bch_share_network: None,
        cash_share: None,
        template_refresh_tx: None,
    };
    let mut channels = MiningChannels::new(ctx, DEFAULT_VERSION_MASK);
    let mut open = payload(
        &encode_message(
            OpenStandardMiningChannel {
                request_id: 1,
                user_identity: USERNAME.try_into().unwrap(),
                // No declared rate, so the channel starts at the node's
                // start difficulty.
                nominal_hash_rate: 0.0,
                max_target: U256::new(&[0xff; 32]).unwrap(),
            },
            MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL,
        )
        .unwrap(),
    );
    let opened = channels
        .handle(
            MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL,
            &mut open,
            Some(Arc::clone(&prepared)),
        )
        .await
        .unwrap();
    let mut success_payload = payload(&opened[0]);
    let channel_id = OpenStandardMiningChannelSuccess::from_bytes(&mut success_payload)
        .unwrap()
        .channel_id;
    let mut job_payload = payload(&opened[1]);
    let job = NewMiningJob::from_bytes(&mut job_payload).unwrap();
    let mut prev_hash_payload = payload(&opened[2]);
    let prev_hash = SetNewPrevHash::from_bytes(&mut prev_hash_payload).unwrap();
    // Any hash meets the job's target (all ones, easier than
    // `Target::MAX`, which is difficulty 1); its credit stays difficulty 7.
    channels
        .channels
        .get_mut(&channel_id)
        .unwrap()
        .jobs
        .get_mut(&job.job_id)
        .unwrap()
        .target = Target::from_le_bytes([0xff; 32]);

    let mut submit = payload(
        &encode_message(
            SubmitSharesStandard {
                channel_id,
                sequence_number: 1,
                job_id: job.job_id,
                nonce: 0,
                ntime: prev_hash.min_ntime,
                version: job.version,
            },
            MESSAGE_TYPE_SUBMIT_SHARES_STANDARD,
        )
        .unwrap(),
    );
    let response = channels
        .handle(MESSAGE_TYPE_SUBMIT_SHARES_STANDARD, &mut submit, None)
        .await
        .unwrap();
    assert_eq!(frame_type(&response[0]), MESSAGE_TYPE_SUBMIT_SHARES_SUCCESS);
    let share = pplns_shares_rx.try_recv().unwrap();
    assert_eq!(share.difficulty, 7);
    assert_eq!(
        share.btcaddress.as_deref(),
        Some("tb1q3udk7r26qs32ltf9nmqrjaaa7tr55qmkk30q5d")
    );
    assert!(emissions_rx.try_recv().is_err());
}

/// A Hydrapool SV2 share whose hash misses the channel target is not
/// stored, even though it would be credited at the channel difficulty.
#[tokio::test]
async fn a_hydrapool_share_below_the_channel_target_is_not_queued() {
    let template: BlockTemplate = serde_json::from_str(include_str!(
        "../../../../../p2poolv2_tests/test_data/gbt/regtest/ckpool/one-txn/gbt.json"
    ))
    .unwrap();
    let payout_key: CompressedPublicKey =
        "020202020202020202020202020202020202020202020202020202020202020202"
            .parse()
            .unwrap();
    let prepared = Arc::new(
        PreparedNotifyParamsBuilder::new(
            Arc::new(template.clone()),
            vec![OutputPair {
                address: Address::p2wpkh(&payout_key, Network::Signet),
                amount: Amount::from_sat(template.coinbasevalue),
            }],
            b"P2Poolv2",
            true,
        )
        .build()
        .unwrap(),
    );
    let (mock_server, bitcoinrpc_config) = setup_mock_bitcoin_rpc().await;
    mock_submit_block_with_any_body(&mock_server).await;
    let (emissions_tx, mut emissions_rx) = mpsc::channel(10);
    let (pplns_shares_tx, mut pplns_shares_rx) = mpsc::channel(10);
    let stats_dir = tempfile::tempdir().unwrap();
    let metrics_handle = metrics::start_metrics(stats_dir.path().to_str().unwrap().to_string())
        .await
        .unwrap();
    let (chain_store_handle, _store_dir) = setup_test_chain_store_handle(true).await;
    let ctx = StratumContext {
        extra_submit_clients: Arc::from([]),
        tracker_handle: start_tracker_actor(),
        bitcoindrpc_client: BitcoindRpcClient::new(
            &bitcoinrpc_config.url,
            &bitcoinrpc_config.username,
            &bitcoinrpc_config.password,
        )
        .unwrap(),
        // A channel target far harder than the test share.
        start_difficulty: 1_000_000_000_000,
        minimum_difficulty: 1,
        maximum_difficulty: None,
        ignore_difficulty: false,
        validate_addresses: true,
        emissions_tx,
        pplns_shares_tx,
        network: Network::Signet,
        metrics: metrics_handle,
        chain_store_handle,
        mode: PoolMode::Hydrapool,
        miner_address: None,
        share_chain: Chain::Bitcoin,
        bch_share_network: None,
        cash_share: None,
        template_refresh_tx: None,
    };
    let mut channels = MiningChannels::new(ctx, DEFAULT_VERSION_MASK);
    let mut open = payload(
        &encode_message(
            OpenStandardMiningChannel {
                request_id: 1,
                user_identity: USERNAME.try_into().unwrap(),
                nominal_hash_rate: 1.0,
                max_target: U256::new(&[0xff; 32]).unwrap(),
            },
            MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL,
        )
        .unwrap(),
    );
    let opened = channels
        .handle(
            MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL,
            &mut open,
            Some(Arc::clone(&prepared)),
        )
        .await
        .unwrap();
    let mut success_payload = payload(&opened[0]);
    let channel_id = OpenStandardMiningChannelSuccess::from_bytes(&mut success_payload)
        .unwrap()
        .channel_id;
    let mut job_payload = payload(&opened[1]);
    let job = NewMiningJob::from_bytes(&mut job_payload).unwrap();
    let mut prev_hash_payload = payload(&opened[2]);
    let prev_hash = SetNewPrevHash::from_bytes(&mut prev_hash_payload).unwrap();

    let mut submit = payload(
        &encode_message(
            SubmitSharesStandard {
                channel_id,
                sequence_number: 1,
                job_id: job.job_id,
                nonce: 0,
                ntime: prev_hash.min_ntime,
                version: job.version,
            },
            MESSAGE_TYPE_SUBMIT_SHARES_STANDARD,
        )
        .unwrap(),
    );
    let response = channels
        .handle(MESSAGE_TYPE_SUBMIT_SHARES_STANDARD, &mut submit, None)
        .await
        .unwrap();
    let mut error_payload = payload(&response[0]);
    let error = SubmitSharesError::from_bytes(&mut error_payload).unwrap();
    assert_eq!(error.error_code.as_utf8_or_hex(), "difficulty-too-low");
    assert!(pplns_shares_rx.try_recv().is_err());
    assert!(emissions_rx.try_recv().is_err());
}

#[tokio::test]
async fn a_connection_cannot_open_more_than_the_channel_cap() {
    let (emissions_tx, _emissions_rx) = mpsc::channel(10);
    let stats_dir = tempfile::tempdir().unwrap();
    let metrics_handle = metrics::start_metrics(stats_dir.path().to_str().unwrap().to_string())
        .await
        .unwrap();
    let (chain_store_handle, _store_dir) = setup_test_chain_store_handle(true).await;
    let ctx = StratumContext {
        extra_submit_clients: Arc::from([]),
        tracker_handle: start_tracker_actor(),
        bitcoindrpc_client: BitcoindRpcClient::new("http://127.0.0.1:1", "user", "pass").unwrap(),
        start_difficulty: 1,
        minimum_difficulty: 1,
        maximum_difficulty: None,
        ignore_difficulty: true,
        validate_addresses: true,
        emissions_tx,
        // P2Poolv2 mode never queues a Hydrapool PPLNS share.
        pplns_shares_tx: mpsc::channel(10).0,
        network: Network::Signet,
        metrics: metrics_handle,
        chain_store_handle,
        mode: PoolMode::P2poolv2,
        miner_address: Some(make_test_share_address(1, Network::Signet)),
        share_chain: Chain::Bitcoin,
        bch_share_network: None,
        cash_share: None,
        template_refresh_tx: None,
    };
    let mut channels = MiningChannels::new(ctx, DEFAULT_VERSION_MASK);
    let open = encode_message(
        OpenExtendedMiningChannel {
            request_id: 1,
            user_identity: USERNAME.try_into().unwrap(),
            nominal_hash_rate: 1.0,
            max_target: U256::new(&[0xff; 32]).unwrap(),
            min_extranonce_size: 8,
        },
        MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL,
    )
    .unwrap();
    for _ in 0..MAX_CHANNELS_PER_CONNECTION {
        let mut request = payload(&open);
        let opened = channels
            .handle(
                MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL,
                &mut request,
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            frame_type(&opened[0]),
            MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL_SUCCESS
        );
    }
    let mut one_more = payload(&open);
    let refused = channels
        .handle(
            MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL,
            &mut one_more,
            None,
        )
        .await
        .unwrap();
    let mut refused_payload = payload(&refused[0]);
    let refusal = OpenMiningChannelError::from_bytes(&mut refused_payload).unwrap();
    assert_eq!(
        refusal.error_code.as_utf8_or_hex(),
        ERROR_CODE_OPEN_MINING_CHANNEL_CHANNEL_CAPACITY_EXHAUSTED
    );
}

#[tokio::test]
async fn channels_are_refused_without_a_share_address_or_a_valid_identity() {
    let (emissions_tx, _emissions_rx) = mpsc::channel(10);
    let stats_dir = tempfile::tempdir().unwrap();
    let metrics_handle = metrics::start_metrics(stats_dir.path().to_str().unwrap().to_string())
        .await
        .unwrap();
    let (chain_store_handle, _store_dir) = setup_test_chain_store_handle(true).await;
    let ctx = StratumContext {
        extra_submit_clients: Arc::from([]),
        tracker_handle: start_tracker_actor(),
        bitcoindrpc_client: BitcoindRpcClient::new("http://127.0.0.1:1", "user", "pass").unwrap(),
        start_difficulty: 1,
        minimum_difficulty: 1,
        maximum_difficulty: None,
        ignore_difficulty: true,
        validate_addresses: true,
        emissions_tx,
        // P2Poolv2 mode never queues a Hydrapool PPLNS share.
        pplns_shares_tx: mpsc::channel(10).0,
        network: Network::Signet,
        metrics: metrics_handle,
        chain_store_handle,
        mode: PoolMode::P2poolv2,
        miner_address: None,
        share_chain: Chain::Bitcoin,
        bch_share_network: None,
        cash_share: None,
        template_refresh_tx: None,
    };
    let mut channels = MiningChannels::new(ctx, DEFAULT_VERSION_MASK);

    let mut without_owner = payload(
        &encode_message(
            OpenExtendedMiningChannel {
                request_id: 1,
                user_identity: USERNAME.try_into().unwrap(),
                nominal_hash_rate: 1.0,
                max_target: U256::new(&[0xff; 32]).unwrap(),
                min_extranonce_size: 8,
            },
            MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL,
        )
        .unwrap(),
    );
    let refused = channels
        .handle(
            MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL,
            &mut without_owner,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        frame_type(&refused[0]),
        MESSAGE_TYPE_OPEN_MINING_CHANNEL_ERROR
    );
    let mut refused_payload = payload(&refused[0]);
    let refusal = OpenMiningChannelError::from_bytes(&mut refused_payload).unwrap();
    assert_eq!(
        refusal.error_code.as_utf8_or_hex(),
        "share-address-required"
    );

    let mut bad_identity = payload(
        &encode_message(
            OpenExtendedMiningChannel {
                request_id: 2,
                user_identity: "not-an-address".try_into().unwrap(),
                nominal_hash_rate: 1.0,
                max_target: U256::new(&[0xff; 32]).unwrap(),
                min_extranonce_size: 8,
            },
            MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL,
        )
        .unwrap(),
    );
    let refused_identity = channels
        .handle(
            MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL,
            &mut bad_identity,
            None,
        )
        .await
        .unwrap();
    let mut identity_payload = payload(&refused_identity[0]);
    let identity_refusal = OpenMiningChannelError::from_bytes(&mut identity_payload).unwrap();
    assert_eq!(
        identity_refusal.error_code.as_utf8_or_hex(),
        "invalid-user-identity"
    );

    let mut too_much_extranonce = payload(
        &encode_message(
            OpenExtendedMiningChannel {
                request_id: 3,
                user_identity: USERNAME.try_into().unwrap(),
                nominal_hash_rate: 1.0,
                max_target: U256::new(&[0xff; 32]).unwrap(),
                min_extranonce_size: 16,
            },
            MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL,
        )
        .unwrap(),
    );
    let refused_size = channels
        .handle(
            MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL,
            &mut too_much_extranonce,
            None,
        )
        .await
        .unwrap();
    let mut size_payload = payload(&refused_size[0]);
    let size_refusal = OpenMiningChannelError::from_bytes(&mut size_payload).unwrap();
    assert_eq!(
        size_refusal.error_code.as_utf8_or_hex(),
        "min-extranonce-size-too-large"
    );
}

#[test]
fn user_identity_splits_into_username_and_sv1_options() {
    assert_eq!(split_user_identity(USERNAME), (USERNAME, None));
    assert_eq!(
        split_user_identity("address.rig1,p2p=share"),
        ("address.rig1", Some("p2p=share"))
    );
    assert_eq!(
        split_user_identity("address.rig1 p2p=share;d=5"),
        ("address.rig1", Some("p2p=share;d=5"))
    );
}

#[test]
fn a_channel_starts_at_the_difficulty_its_declared_rate_settles_on() {
    // Nothing usable declared: the node's start difficulty.
    assert_eq!(initial_difficulty(0.0, 1_000, 1, None), 1_000);
    assert_eq!(initial_difficulty(f32::NAN, 1_000, 1, None), 1_000);
    assert_eq!(initial_difficulty(-5.0, 1_000, 1, None), 1_000);
    // A 4 TH/s ASIC settles near 3,104.
    assert_eq!(initial_difficulty(4.0e12, 1_000, 1, None), 3_104);
    // A 20 MH/s CPU miner is held at the node's minimum.
    assert_eq!(initial_difficulty(2.0e7, 1_000, 1, None), 1);
    // And the node's maximum caps a huge declaration.
    assert_eq!(initial_difficulty(1.0e18, 1_000, 1, Some(50_000)), 50_000);
}

#[test]
fn a_worker_name_with_a_space_stays_in_the_username() {
    assert_eq!(
        split_user_identity("address.my rig"),
        ("address.my rig", None)
    );
    assert_eq!(
        split_user_identity("address.my rig,p2p=share"),
        ("address.my rig", Some("p2p=share"))
    );
}

#[tokio::test]
async fn a_bitcoin_channel_takes_its_share_address_from_p2p() {
    let (emissions_tx, _emissions_rx) = mpsc::channel(10);
    let stats_dir = tempfile::tempdir().unwrap();
    let metrics_handle = metrics::start_metrics(stats_dir.path().to_str().unwrap().to_string())
        .await
        .unwrap();
    let (chain_store_handle, _store_dir) = setup_test_chain_store_handle(true).await;
    let ctx = StratumContext {
        extra_submit_clients: Arc::from([]),
        tracker_handle: start_tracker_actor(),
        bitcoindrpc_client: BitcoindRpcClient::new("http://127.0.0.1:1", "user", "pass").unwrap(),
        start_difficulty: 1,
        minimum_difficulty: 1,
        maximum_difficulty: None,
        ignore_difficulty: true,
        validate_addresses: true,
        emissions_tx,
        // P2Poolv2 mode never queues a Hydrapool PPLNS share.
        pplns_shares_tx: mpsc::channel(10).0,
        network: Network::Signet,
        metrics: metrics_handle,
        chain_store_handle,
        mode: PoolMode::P2poolv2,
        miner_address: None,
        share_chain: Chain::Bitcoin,
        bch_share_network: None,
        cash_share: None,
        template_refresh_tx: None,
    };
    let mut channels = MiningChannels::new(ctx, DEFAULT_VERSION_MASK);
    let share_address = make_test_share_address(2, Network::Signet);
    let identity = format!("{USERNAME},p2p={share_address}");

    let mut open = payload(
        &encode_message(
            OpenExtendedMiningChannel {
                request_id: 1,
                user_identity: identity.as_str().try_into().unwrap(),
                nominal_hash_rate: 1.0,
                max_target: U256::new(&[0xff; 32]).unwrap(),
                min_extranonce_size: 8,
            },
            MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL,
        )
        .unwrap(),
    );
    let opened = channels
        .handle(MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL, &mut open, None)
        .await
        .unwrap();

    assert_eq!(
        frame_type(&opened[0]),
        MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL_SUCCESS
    );
    let session = &channels.channels[&1].session;
    assert_eq!(
        session.share_owner,
        Some(ShareOwner::Taproot(share_address.witness_program()))
    );
    assert_eq!(session.username.as_deref(), Some(USERNAME));
}

#[tokio::test]
async fn a_channel_naming_another_share_address_than_the_node_is_refused() {
    let (emissions_tx, _emissions_rx) = mpsc::channel(10);
    let stats_dir = tempfile::tempdir().unwrap();
    let metrics_handle = metrics::start_metrics(stats_dir.path().to_str().unwrap().to_string())
        .await
        .unwrap();
    let (chain_store_handle, _store_dir) = setup_test_chain_store_handle(true).await;
    let ctx = StratumContext {
        extra_submit_clients: Arc::from([]),
        tracker_handle: start_tracker_actor(),
        bitcoindrpc_client: BitcoindRpcClient::new("http://127.0.0.1:1", "user", "pass").unwrap(),
        start_difficulty: 1,
        minimum_difficulty: 1,
        maximum_difficulty: None,
        ignore_difficulty: true,
        validate_addresses: true,
        emissions_tx,
        // P2Poolv2 mode never queues a Hydrapool PPLNS share.
        pplns_shares_tx: mpsc::channel(10).0,
        network: Network::Signet,
        metrics: metrics_handle,
        chain_store_handle,
        mode: PoolMode::P2poolv2,
        miner_address: Some(make_test_share_address(1, Network::Signet)),
        share_chain: Chain::Bitcoin,
        bch_share_network: None,
        cash_share: None,
        template_refresh_tx: None,
    };
    let mut channels = MiningChannels::new(ctx, DEFAULT_VERSION_MASK);
    let identity = format!(
        "{USERNAME},p2p={}",
        make_test_share_address(2, Network::Signet)
    );

    let mut open = payload(
        &encode_message(
            OpenExtendedMiningChannel {
                request_id: 1,
                user_identity: identity.as_str().try_into().unwrap(),
                nominal_hash_rate: 1.0,
                max_target: U256::new(&[0xff; 32]).unwrap(),
                min_extranonce_size: 8,
            },
            MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL,
        )
        .unwrap(),
    );
    let refused = channels
        .handle(MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL, &mut open, None)
        .await
        .unwrap();

    let mut refused_payload = payload(&refused[0]);
    let refusal = OpenMiningChannelError::from_bytes(&mut refused_payload).unwrap();
    assert_eq!(refusal.error_code.as_utf8_or_hex(), "invalid-share-address");
    assert!(channels.channels.is_empty());
}

#[tokio::test]
async fn a_bitcoin_cash_channel_takes_its_share_hash_from_p2p() {
    let (emissions_tx, _emissions_rx) = mpsc::channel(10);
    let stats_dir = tempfile::tempdir().unwrap();
    let metrics_handle = metrics::start_metrics(stats_dir.path().to_str().unwrap().to_string())
        .await
        .unwrap();
    let (chain_store_handle, _store_dir) = setup_test_chain_store_handle(true).await;
    let ctx = StratumContext {
        extra_submit_clients: Arc::from([]),
        tracker_handle: start_tracker_actor(),
        bitcoindrpc_client: BitcoindRpcClient::new("http://127.0.0.1:1", "user", "pass").unwrap(),
        start_difficulty: 1,
        minimum_difficulty: 1,
        maximum_difficulty: None,
        ignore_difficulty: true,
        validate_addresses: true,
        emissions_tx,
        // P2Poolv2 mode never queues a Hydrapool PPLNS share.
        pplns_shares_tx: mpsc::channel(10).0,
        network: Network::Regtest,
        metrics: metrics_handle,
        chain_store_handle,
        mode: PoolMode::P2poolv2,
        miner_address: None,
        share_chain: Chain::BitcoinCash,
        bch_share_network: Some(BchShareNetwork::Regtest),
        cash_share: None,
        template_refresh_tx: None,
    };
    let mut channels = MiningChannels::new(ctx, DEFAULT_VERSION_MASK);
    let share_address = BchShareAddress::new(BchShareNetwork::Regtest, [0x22; 20]).encode();
    let identity =
        format!("bchreg:qzh5k63rhy4jg6jtyssmzddju5kkng4jzu8ktcypfm.rig1,p2p={share_address}");

    let mut open = payload(
        &encode_message(
            OpenStandardMiningChannel {
                request_id: 1,
                user_identity: identity.as_str().try_into().unwrap(),
                nominal_hash_rate: 1.0,
                max_target: U256::new(&[0xff; 32]).unwrap(),
            },
            MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL,
        )
        .unwrap(),
    );
    let opened = channels
        .handle(MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL, &mut open, None)
        .await
        .unwrap();

    assert_eq!(
        frame_type(&opened[0]),
        MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL_SUCCESS
    );
    assert_eq!(
        channels.channels[&1].session.share_owner,
        Some(ShareOwner::CashP2pkh([0x22; 20]))
    );
}

#[tokio::test]
async fn a_bitcoin_cash_channel_opens_through_a_translator_worker_suffix() {
    let (emissions_tx, _emissions_rx) = mpsc::channel(10);
    let stats_dir = tempfile::tempdir().unwrap();
    let metrics_handle = metrics::start_metrics(stats_dir.path().to_str().unwrap().to_string())
        .await
        .unwrap();
    let (chain_store_handle, _store_dir) = setup_test_chain_store_handle(true).await;
    let ctx = StratumContext {
        extra_submit_clients: Arc::from([]),
        tracker_handle: start_tracker_actor(),
        bitcoindrpc_client: BitcoindRpcClient::new("http://127.0.0.1:1", "user", "pass").unwrap(),
        start_difficulty: 1,
        minimum_difficulty: 1,
        maximum_difficulty: None,
        ignore_difficulty: true,
        validate_addresses: true,
        emissions_tx,
        // P2Poolv2 mode never queues a Hydrapool PPLNS share.
        pplns_shares_tx: mpsc::channel(10).0,
        network: Network::Regtest,
        metrics: metrics_handle,
        chain_store_handle,
        mode: PoolMode::P2poolv2,
        miner_address: None,
        share_chain: Chain::BitcoinCash,
        bch_share_network: Some(BchShareNetwork::Regtest),
        cash_share: None,
        template_refresh_tx: None,
    };
    let mut channels = MiningChannels::new(ctx, DEFAULT_VERSION_MASK);
    let share_address = BchShareAddress::new(BchShareNetwork::Regtest, [0x22; 20]).encode();
    // What the Stratum V2 reference translator sends for an SV1 device:
    // the configured identity with `.<downstream worker>` appended.
    let identity = format!(
        "bchreg:qzh5k63rhy4jg6jtyssmzddju5kkng4jzu8ktcypfm.rig1,p2p={share_address}.miner1"
    );

    let mut open = payload(
        &encode_message(
            OpenStandardMiningChannel {
                request_id: 1,
                user_identity: identity.as_str().try_into().unwrap(),
                nominal_hash_rate: 1.0,
                max_target: U256::new(&[0xff; 32]).unwrap(),
            },
            MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL,
        )
        .unwrap(),
    );
    let opened = channels
        .handle(MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL, &mut open, None)
        .await
        .unwrap();

    assert_eq!(
        frame_type(&opened[0]),
        MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL_SUCCESS
    );
    assert_eq!(
        channels.channels[&1].session.share_owner,
        Some(ShareOwner::CashP2pkh([0x22; 20]))
    );
}

#[tokio::test]
async fn a_bitcoin_cash_channel_with_a_taproot_share_address_is_refused() {
    let (emissions_tx, _emissions_rx) = mpsc::channel(10);
    let stats_dir = tempfile::tempdir().unwrap();
    let metrics_handle = metrics::start_metrics(stats_dir.path().to_str().unwrap().to_string())
        .await
        .unwrap();
    let (chain_store_handle, _store_dir) = setup_test_chain_store_handle(true).await;
    let ctx = StratumContext {
        extra_submit_clients: Arc::from([]),
        tracker_handle: start_tracker_actor(),
        bitcoindrpc_client: BitcoindRpcClient::new("http://127.0.0.1:1", "user", "pass").unwrap(),
        start_difficulty: 1,
        minimum_difficulty: 1,
        maximum_difficulty: None,
        ignore_difficulty: true,
        validate_addresses: true,
        emissions_tx,
        // P2Poolv2 mode never queues a Hydrapool PPLNS share.
        pplns_shares_tx: mpsc::channel(10).0,
        network: Network::Regtest,
        metrics: metrics_handle,
        chain_store_handle,
        mode: PoolMode::P2poolv2,
        miner_address: None,
        share_chain: Chain::BitcoinCash,
        bch_share_network: Some(BchShareNetwork::Regtest),
        cash_share: None,
        template_refresh_tx: None,
    };
    let mut channels = MiningChannels::new(ctx, DEFAULT_VERSION_MASK);
    let identity = format!(
        "bchreg:qzh5k63rhy4jg6jtyssmzddju5kkng4jzu8ktcypfm.rig1,p2p={}",
        make_test_share_address(2, Network::Regtest)
    );

    let mut open = payload(
        &encode_message(
            OpenStandardMiningChannel {
                request_id: 1,
                user_identity: identity.as_str().try_into().unwrap(),
                nominal_hash_rate: 1.0,
                max_target: U256::new(&[0xff; 32]).unwrap(),
            },
            MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL,
        )
        .unwrap(),
    );
    let refused = channels
        .handle(MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL, &mut open, None)
        .await
        .unwrap();

    let mut refused_payload = payload(&refused[0]);
    let refusal = OpenMiningChannelError::from_bytes(&mut refused_payload).unwrap();
    assert_eq!(refusal.error_code.as_utf8_or_hex(), "invalid-share-address");
}

#[tokio::test]
async fn update_channel_answers_with_a_target_capped_by_the_new_maximum() {
    let (emissions_tx, _emissions_rx) = mpsc::channel(10);
    let stats_dir = tempfile::tempdir().unwrap();
    let metrics_handle = metrics::start_metrics(stats_dir.path().to_str().unwrap().to_string())
        .await
        .unwrap();
    let (chain_store_handle, _store_dir) = setup_test_chain_store_handle(true).await;
    let ctx = StratumContext {
        extra_submit_clients: Arc::from([]),
        tracker_handle: start_tracker_actor(),
        bitcoindrpc_client: BitcoindRpcClient::new("http://127.0.0.1:1", "user", "pass").unwrap(),
        start_difficulty: 1,
        minimum_difficulty: 1,
        maximum_difficulty: None,
        ignore_difficulty: true,
        validate_addresses: true,
        emissions_tx,
        // P2Poolv2 mode never queues a Hydrapool PPLNS share.
        pplns_shares_tx: mpsc::channel(10).0,
        network: Network::Signet,
        metrics: metrics_handle,
        chain_store_handle,
        mode: PoolMode::P2poolv2,
        miner_address: Some(make_test_share_address(1, Network::Signet)),
        share_chain: Chain::Bitcoin,
        bch_share_network: None,
        cash_share: None,
        template_refresh_tx: None,
    };
    let mut channels = MiningChannels::new(ctx, DEFAULT_VERSION_MASK);
    let mut open = payload(
        &encode_message(
            OpenExtendedMiningChannel {
                request_id: 1,
                user_identity: USERNAME.try_into().unwrap(),
                nominal_hash_rate: 1.0,
                max_target: U256::new(&[0xff; 32]).unwrap(),
                min_extranonce_size: 8,
            },
            MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL,
        )
        .unwrap(),
    );
    channels
        .handle(MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL, &mut open, None)
        .await
        .unwrap();
    let mut lower_maximum = [0u8; 32];
    lower_maximum[..16].copy_from_slice(&[0xff; 16]);
    let mut update = payload(
        &encode_message(
            UpdateChannel {
                channel_id: 1,
                nominal_hash_rate: 1.0,
                maximum_target: U256::new(&lower_maximum).unwrap(),
            },
            MESSAGE_TYPE_UPDATE_CHANNEL,
        )
        .unwrap(),
    );
    let answered = channels
        .handle(MESSAGE_TYPE_UPDATE_CHANNEL, &mut update, None)
        .await
        .unwrap();

    assert_eq!(answered.len(), 1);
    assert_eq!(frame_type(&answered[0]), MESSAGE_TYPE_SET_TARGET);
    assert!(frame_has_channel_bit(&answered[0]));
    let mut set_target_payload = payload(&answered[0]);
    let set_target = SetTarget::from_bytes(&mut set_target_payload).unwrap();
    assert_eq!(set_target.channel_id, 1);
    assert_eq!(set_target.maximum_target.as_bytes(), &lower_maximum);
    let rejected = channels.update(1, &[0; 32]).unwrap();
    let mut rejected_payload = payload(&rejected[0]);
    let error = UpdateChannelError::from_bytes(&mut rejected_payload).unwrap();
    assert_eq!(
        error.error_code.as_utf8_or_hex(),
        ERROR_CODE_OPEN_MINING_CHANNEL_MAX_TARGET_OUT_OF_RANGE
    );
    assert_eq!(
        channels.channels[&1].max_target.to_le_bytes(),
        lower_maximum
    );
}

#[tokio::test]
async fn update_channel_for_an_unknown_channel_gets_an_error_reply() {
    let (emissions_tx, _emissions_rx) = mpsc::channel(10);
    let stats_dir = tempfile::tempdir().unwrap();
    let metrics_handle = metrics::start_metrics(stats_dir.path().to_str().unwrap().to_string())
        .await
        .unwrap();
    let (chain_store_handle, _store_dir) = setup_test_chain_store_handle(true).await;
    let ctx = StratumContext {
        extra_submit_clients: Arc::from([]),
        tracker_handle: start_tracker_actor(),
        bitcoindrpc_client: BitcoindRpcClient::new("http://127.0.0.1:1", "user", "pass").unwrap(),
        start_difficulty: 1,
        minimum_difficulty: 1,
        maximum_difficulty: None,
        ignore_difficulty: true,
        validate_addresses: true,
        emissions_tx,
        // P2Poolv2 mode never queues a Hydrapool PPLNS share.
        pplns_shares_tx: mpsc::channel(10).0,
        network: Network::Signet,
        metrics: metrics_handle,
        chain_store_handle,
        mode: PoolMode::P2poolv2,
        miner_address: Some(make_test_share_address(1, Network::Signet)),
        share_chain: Chain::Bitcoin,
        bch_share_network: None,
        cash_share: None,
        template_refresh_tx: None,
    };
    let mut channels = MiningChannels::new(ctx, DEFAULT_VERSION_MASK);
    let mut update = payload(
        &encode_message(
            UpdateChannel {
                channel_id: 7,
                nominal_hash_rate: 1.0,
                maximum_target: U256::new(&[0xff; 32]).unwrap(),
            },
            MESSAGE_TYPE_UPDATE_CHANNEL,
        )
        .unwrap(),
    );
    let answered = channels
        .handle(MESSAGE_TYPE_UPDATE_CHANNEL, &mut update, None)
        .await
        .unwrap();

    assert_eq!(frame_type(&answered[0]), MESSAGE_TYPE_UPDATE_CHANNEL_ERROR);
    let mut error_payload = payload(&answered[0]);
    let update_error = UpdateChannelError::from_bytes(&mut error_payload).unwrap();
    assert_eq!(
        update_error.error_code.as_utf8_or_hex(),
        "invalid-channel-id"
    );
}

#[tokio::test]
async fn close_channel_removes_the_channel() {
    let (emissions_tx, _emissions_rx) = mpsc::channel(10);
    let stats_dir = tempfile::tempdir().unwrap();
    let metrics_handle = metrics::start_metrics(stats_dir.path().to_str().unwrap().to_string())
        .await
        .unwrap();
    let (chain_store_handle, _store_dir) = setup_test_chain_store_handle(true).await;
    let ctx = StratumContext {
        extra_submit_clients: Arc::from([]),
        tracker_handle: start_tracker_actor(),
        bitcoindrpc_client: BitcoindRpcClient::new("http://127.0.0.1:1", "user", "pass").unwrap(),
        start_difficulty: 1,
        minimum_difficulty: 1,
        maximum_difficulty: None,
        ignore_difficulty: true,
        validate_addresses: true,
        emissions_tx,
        // P2Poolv2 mode never queues a Hydrapool PPLNS share.
        pplns_shares_tx: mpsc::channel(10).0,
        network: Network::Signet,
        metrics: metrics_handle,
        chain_store_handle,
        mode: PoolMode::P2poolv2,
        miner_address: Some(make_test_share_address(1, Network::Signet)),
        share_chain: Chain::Bitcoin,
        bch_share_network: None,
        cash_share: None,
        template_refresh_tx: None,
    };
    let mut channels = MiningChannels::new(ctx, DEFAULT_VERSION_MASK);
    let mut open = payload(
        &encode_message(
            OpenExtendedMiningChannel {
                request_id: 1,
                user_identity: USERNAME.try_into().unwrap(),
                nominal_hash_rate: 1.0,
                max_target: U256::new(&[0xff; 32]).unwrap(),
                min_extranonce_size: 8,
            },
            MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL,
        )
        .unwrap(),
    );
    channels
        .handle(MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL, &mut open, None)
        .await
        .unwrap();
    assert_eq!(channels.channels.len(), 1);
    channels
        .handle(MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL, &mut open, None)
        .await
        .unwrap();
    assert_eq!(channels.channels.len(), 2);
    let mut close = payload(
        &encode_message(
            CloseChannel {
                channel_id: 1,
                reason_code: "done".try_into().unwrap(),
            },
            MESSAGE_TYPE_CLOSE_CHANNEL,
        )
        .unwrap(),
    );
    let answered = channels
        .handle(MESSAGE_TYPE_CLOSE_CHANNEL, &mut close, None)
        .await
        .unwrap();

    assert!(answered.is_empty());
    assert_eq!(channels.channels.len(), 1);
    let (address, _) = USERNAME.split_once('.').unwrap();
    assert!(channels.ctx.metrics.get_metrics().await.users[address].workers["rig1"].active);
    channels.close_all().await;
    assert!(channels.channels.is_empty());
    assert!(!channels.ctx.metrics.get_metrics().await.users[address].workers["rig1"].active);
    channels.close_all().await;
}

/// An extended job carries at most 65,535 bytes of coinbase on each side of
/// the extranonce.
#[test]
fn an_extended_job_carries_a_coinbase_up_to_its_field_limit() {
    assert!(check_extended_job_fits(100, 65_535).is_ok());
    assert!(check_extended_job_fits(65_535, 100).is_ok());
}

/// A coinbase past the extended job's field limit is refused for the
/// channel with an error that names the limit, instead of being truncated.
#[test]
fn a_coinbase_past_the_extended_job_field_limit_is_refused() {
    let error = check_extended_job_fits(100, 65_536).unwrap_err();
    assert!(error.contains("65535"), "{error}");
    assert!(error.contains("standard channel"), "{error}");
    assert!(check_extended_job_fits(65_536, 100).is_err());
}
