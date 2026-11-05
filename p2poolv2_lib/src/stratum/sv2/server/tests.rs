use super::*;
use crate::accounting::stats::metrics;
use crate::config::{Chain, DEFAULT_VERSION_MASK, PoolMode};
use crate::shares::merge_mining::MergeMiningCommitment;
use crate::shares::share_block::ShareHeader;
use crate::shares::share_commitment::ShareCommitment;
use crate::stratum::block_submit::FoundBlockSubmitter;
use crate::stratum::sv2::PoolTransactionCache;
use crate::stratum::sv2::payout_outputs::{
    EXACT_OUTPUTS, INVALID_REQUEST, PAYOUT_OUTPUTS_STALE, REQUEST_PAYOUT_OUTPUTS,
    REQUEST_PAYOUT_OUTPUTS_ERROR, REQUEST_PAYOUT_OUTPUTS_SUCCESS, RequestPayoutOutputsError,
    RequestPayoutOutputsSuccess, STALE_PAYOUT_OUTPUTS, VERIFY_PAYOUT_OUTPUTS,
    VERIFY_PAYOUT_OUTPUTS_ERROR,
};
use crate::stratum::sv2::session::encode_message;
use crate::stratum::work::merge_tokens::{MergeMinedToken, TokenSet};
use crate::stratum::work::prepared_notify::token_split;
use crate::stratum::work::tracker::start_tracker_actor;
use crate::test_utils::{
    TestSv2Client, bch_template_transaction, test_bch_custom_job, test_bch_custom_job_with_leaves,
    test_bch_prepared, test_bch_prepared_on_anchor, test_bch_prepared_with_cuts,
    test_bch_solve_custom_job, test_bch_stratum_context, test_custom_job_from_payout_outputs,
    test_declared_coinbase_parts, test_payout_outputs_request, test_sv2_authority, test_sv2_setup,
    test_verify_payout_outputs_request,
};
use crate::test_utils::{make_test_share_address, setup_test_chain_store_handle};
use binary_sv2::U256;
use binary_sv2::{B016M, B032, B064K, B0255, Seq064K};
use bitcoin::consensus::{deserialize, serialize};
use bitcoin::hashes::Hash;
use bitcoin::script::Builder;
use bitcoin::secp256k1::{Keypair, Secp256k1};
use bitcoin::{BlockHash, Network};
use bitcoindrpc::BitcoindRpcClient;
use codec_sv2::{Handshake, NoiseEncoder};
use common_messages_sv2::{
    MESSAGE_TYPE_SETUP_CONNECTION, MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS, Protocol, SetupConnection,
};
use framing_sv2::framing::SerializedFrame;
use job_declaration_sv2::{
    AllocateMiningJobToken, AllocateMiningJobTokenSuccess, DeclareMiningJob,
    DeclareMiningJobSuccess, MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN,
    MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN_SUCCESS, MESSAGE_TYPE_DECLARE_MINING_JOB,
    MESSAGE_TYPE_DECLARE_MINING_JOB_SUCCESS, MESSAGE_TYPE_PROVIDE_MISSING_TRANSACTIONS,
    MESSAGE_TYPE_PROVIDE_MISSING_TRANSACTIONS_SUCCESS, ProvideMissingTransactions,
    ProvideMissingTransactionsSuccess,
};
use mining_sv2::{
    MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL, MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL_SUCCESS,
    OpenExtendedMiningChannel,
};
use mining_sv2::{
    MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_ERROR, MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_SUCCESS,
    MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED, MESSAGE_TYPE_SUBMIT_SHARES_SUCCESS,
    OpenExtendedMiningChannelSuccess, SetCustomMiningJobError, SetCustomMiningJobSuccess,
    SubmitSharesExtended, SubmitSharesSuccess,
};
use noise_sv2::{INITIATOR_EXPECTED_HANDSHAKE_MESSAGE_SIZE, Initiator};
use p2poolv2_wallet::bch_share::{BchShareNetwork, encode_payout_address};
use tokio::sync::mpsc;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_partial_json, method},
};

#[tokio::test]
async fn withdrawing_work_closes_the_sv2_connection() {
    let (context, _emissions, _directories) = test_bch_stratum_context("http://127.0.0.1:1").await;
    let listener = bind("127.0.0.1", 0).await.unwrap();
    let _client = TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let (stream, _) = listener.accept().await.unwrap();
    let permits = Arc::new(Semaphore::new(1));
    let permit = Arc::clone(&permits).acquire_owned().await.unwrap();
    let registry = Arc::new(Mutex::new(JobRegistry::new(
        PoolTransactionCache::new(),
        Some(MergeMiningCommitment::empty()),
    )));
    let (templates, receiver) = watch::channel(Some(test_bch_prepared()));
    let (_shutdown_sender, shutdown) = watch::channel(());
    let connection = tokio::spawn(handle_connection(
        stream,
        permit,
        registry,
        Some(Sv2Mining {
            ctx: context,
            version_mask: DEFAULT_VERSION_MASK,
            template_rx: receiver,
        }),
        None,
        None,
        shutdown,
    ));
    templates.send(None).unwrap();
    timeout(Duration::from_secs(5), connection)
        .await
        .expect("withdrawal must close a connection holding previous work")
        .unwrap();
    assert_eq!(permits.available_permits(), 1);
}

#[tokio::test]
async fn encrypted_bch_job_declaration_modes_install_mine_and_emit() {
    let rpc = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_partial_json(
            serde_json::json!({"method": "getblocktemplate"}),
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"result": null, "error": null, "id": 0})),
        )
        .expect(1)
        .mount(&rpc)
        .await;
    Mock::given(method("POST"))
        .and(body_partial_json(
            serde_json::json!({"method": "submitblock"}),
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"result": null, "error": null, "id": 1})),
        )
        .expect(if cfg!(feature = "sim") { 0 } else { 1 })
        .mount(&rpc)
        .await;
    // A node taking a 1% donation and a 1% fee, so a declarator's token
    // leaves carry both in their split.
    let fee_address = bitcoin::Address::p2pkh(
        bitcoin::PubkeyHash::from_byte_array([0xfe; 20]),
        Network::Regtest,
    );
    let prepared = test_bch_prepared_with_cuts(Some(100), Some(fee_address.into()), Some(100));
    let cache = PoolTransactionCache::new();
    cache.set_prepared(Arc::clone(&prepared));
    let (_template_sender, template_rx) = watch::channel(Some(Arc::clone(&prepared)));
    let (ctx, mut emissions, _dirs) = test_bch_stratum_context(&rpc.uri()).await;
    let authority = test_sv2_authority();
    let listener = bind("127.0.0.1", 0).await.unwrap();
    let address = listener.local_addr().unwrap();
    let connection_limit = Arc::new(Semaphore::new(3));
    let (shutdown_sender, shutdown_receiver) = watch::channel(());
    let server = tokio::spawn(serve(
        listener,
        Arc::clone(&connection_limit),
        cache,
        Some(MergeMiningCommitment::empty()),
        Some(Sv2Mining {
            ctx,
            version_mask: DEFAULT_VERSION_MASK,
            template_rx,
        }),
        Some(authority),
        None,
        shutdown_receiver,
    ));
    let mut mining = TestSv2Client::connect(address, authority.public_key).await;
    mining
        .send(
            encode_message(
                test_sv2_setup(Protocol::MiningProtocol, 2),
                MESSAGE_TYPE_SETUP_CONNECTION,
            )
            .unwrap(),
        )
        .await;
    mining.receive(MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS).await;
    let payout = bitcoin::Address::p2pkh(
        bitcoin::PubkeyHash::from_byte_array([0x22; 20]),
        Network::Regtest,
    );
    let identity = encode_payout_address(&payout, BchShareNetwork::Regtest).unwrap();
    mining
        .send(
            encode_message(
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
        )
        .await;
    let mut open_payload = mining
        .receive(MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL_SUCCESS)
        .await;
    let open = OpenExtendedMiningChannelSuccess::from_bytes(&mut open_payload).unwrap();
    let extranonce_prefix = open.extranonce_prefix.as_bytes().to_vec();

    let mut declarator = TestSv2Client::connect(address, authority.public_key).await;
    declarator
        .send_pair(
            encode_message(
                test_sv2_setup(Protocol::JobDeclarationProtocol, 1),
                MESSAGE_TYPE_SETUP_CONNECTION,
            )
            .unwrap(),
            encode_message(
                AllocateMiningJobToken {
                    request_id: 2,
                    user_identifier: "miner".try_into().unwrap(),
                },
                MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN,
            )
            .unwrap(),
        )
        .await;
    declarator
        .receive(MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS)
        .await;
    let mut allocation_payload = declarator
        .receive(MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN_SUCCESS)
        .await;
    let allocated = AllocateMiningJobTokenSuccess::from_bytes(&mut allocation_payload).unwrap();
    let token = allocated.mining_job_token.as_bytes().to_vec();
    let selected: bitcoin::Transaction =
        deserialize(&hex::decode(bch_template_transaction(1).data).unwrap()).unwrap();
    // Sixteen tokens, the most one tree holds, each leaf paying the miner
    // with the job's donation and fee: their 3,830-byte disclosure makes
    // the declaration longer than the listener's 4,096-byte reads.
    let terms = ShareCommitment::from_mining_job_token(
        &token,
        payout.clone().into(),
        p2poolv2_wallet::share_owner::ShareOwner::CashP2pkh([0x22; 20]),
    )
    .unwrap();
    let split = token_split(
        &terms.donation_address,
        terms.donation,
        &terms.fee_address,
        terms.fee,
    )
    .unwrap();
    let sixteen_tokens = TokenSet::new(
        (1..=16u8)
            .map(|category| MergeMinedToken {
                category: [category; 32],
                lock_hash: [0x51; 32],
                state_hash: [category; 32],
                target_bits: 0x207f_ffff,
                max_height: 16,
                safa: None,
            })
            .collect(),
    )
    .unwrap();
    let leaves = sixteen_tokens.miner_job(payout.script_pubkey().as_bytes(), split);
    let excess_data = leaves.disclosure().encode();
    assert_eq!(excess_data.len(), 3_830);
    let (mut custom_bytes, declared) = test_bch_custom_job_with_leaves(
        Arc::clone(&prepared),
        token.clone(),
        vec![selected.clone()],
        true,
        prepared.template().coinbasevalue,
        leaves.commitment,
        leaves.disclosure(),
    );
    let declaration = declared.declaration.unwrap();
    let wtxid = selected.compute_wtxid().to_byte_array();
    declarator
        .send(
            encode_message(
                DeclareMiningJob {
                    request_id: 3,
                    mining_job_token: B0255::new(&token).unwrap(),
                    version: declaration.version,
                    coinbase_tx_prefix: B064K::new(&declaration.prefix).unwrap(),
                    coinbase_tx_suffix: B064K::new(&declaration.suffix).unwrap(),
                    wtxid_list: Seq064K::new(vec![U256::new(&wtxid).unwrap()]).unwrap(),
                    excess_data: B064K::new(&excess_data).unwrap(),
                },
                MESSAGE_TYPE_DECLARE_MINING_JOB,
            )
            .unwrap(),
        )
        .await;
    let mut missing_payload = declarator
        .receive(MESSAGE_TYPE_PROVIDE_MISSING_TRANSACTIONS)
        .await;
    let missing = ProvideMissingTransactions::from_bytes(&mut missing_payload).unwrap();
    assert_eq!(missing.request_id, 3);
    assert_eq!(missing.unknown_tx_position_list.as_slice(), &[0]);
    let transaction_bytes = serialize(&selected);
    declarator
        .send(
            encode_message(
                ProvideMissingTransactionsSuccess {
                    request_id: 3,
                    transaction_list: Seq064K::new(vec![B016M::new(&transaction_bytes).unwrap()])
                        .unwrap(),
                },
                MESSAGE_TYPE_PROVIDE_MISSING_TRANSACTIONS_SUCCESS,
            )
            .unwrap(),
        )
        .await;
    let mut accepted_payload = declarator
        .receive(MESSAGE_TYPE_DECLARE_MINING_JOB_SUCCESS)
        .await;
    let declared = DeclareMiningJobSuccess::from_bytes(&mut accepted_payload).unwrap();
    let new_token = declared.new_mining_job_token.as_bytes().to_vec();
    let mut custom = SetCustomMiningJob::from_bytes(&mut custom_bytes).unwrap();
    custom.token = B0255::new(&new_token).unwrap();
    let custom_frame = encode_message(custom.clone(), MESSAGE_TYPE_SET_CUSTOM_MINING_JOB).unwrap();
    mining.send(custom_frame.clone()).await;
    let mut installed_payload = mining
        .receive(MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_SUCCESS)
        .await;
    let installed = SetCustomMiningJobSuccess::from_bytes(&mut installed_payload).unwrap();
    assert_eq!(installed.request_id, custom.request_id);
    mining.send(custom_frame).await;
    let mut replay_payload = mining
        .receive(MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_ERROR)
        .await;
    let replay = SetCustomMiningJobError::from_bytes(&mut replay_payload).unwrap();
    assert_eq!(
        replay.error_code.as_utf8_or_hex(),
        "invalid-mining-job-token"
    );
    let header = test_bch_solve_custom_job(&custom, &extranonce_prefix);
    mining
        .send(
            encode_message(
                SubmitSharesExtended {
                    channel_id: 1,
                    sequence_number: 1,
                    job_id: installed.job_id,
                    nonce: header.nonce,
                    ntime: header.time,
                    version: custom.version,
                    extranonce: B032::new(&[0; 8]).unwrap(),
                },
                MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED,
            )
            .unwrap(),
        )
        .await;
    let mut submitted_payload = mining.receive(MESSAGE_TYPE_SUBMIT_SHARES_SUCCESS).await;
    let submitted = SubmitSharesSuccess::from_bytes(&mut submitted_payload).unwrap();
    assert_eq!(submitted.last_sequence_number, 1);
    let emission = emissions.try_recv().unwrap();
    assert_eq!(emission.header, header);
    assert_eq!(
        emission.blocktemplate.transactions[0].txid,
        selected.compute_txid().to_string()
    );
    let share = ShareHeader::from_commitment_and_header(
        emission.share_commitment,
        emission.header,
        u64::from(emission.blocktemplate.height),
        emission.coinbase_proof,
    );
    assert_eq!(
        share
            .coinbase_proof
            .merkle_root(&share, &emission.template_merkle_branches)
            .unwrap(),
        header.merkle_root
    );

    let mut private_declarator = TestSv2Client::connect(address, authority.public_key).await;
    private_declarator
        .send_pair(
            encode_message(
                test_sv2_setup(Protocol::JobDeclarationProtocol, 0),
                MESSAGE_TYPE_SETUP_CONNECTION,
            )
            .unwrap(),
            encode_message(
                AllocateMiningJobToken {
                    request_id: 4,
                    user_identifier: "miner".try_into().unwrap(),
                },
                MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN,
            )
            .unwrap(),
        )
        .await;
    private_declarator
        .receive(MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS)
        .await;
    let mut allocation_payload = private_declarator
        .receive(MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN_SUCCESS)
        .await;
    let allocated = AllocateMiningJobTokenSuccess::from_bytes(&mut allocation_payload).unwrap();
    let private_tx = deserialize(&hex::decode(bch_template_transaction(9).data).unwrap()).unwrap();
    let (mut private_bytes, _) = test_bch_custom_job(
        Arc::clone(&prepared),
        allocated.mining_job_token.as_bytes().to_vec(),
        vec![private_tx],
        false,
        prepared.template().coinbasevalue,
    );
    let private = SetCustomMiningJob::from_bytes(&mut private_bytes).unwrap();
    mining
        .send(encode_message(private.clone(), MESSAGE_TYPE_SET_CUSTOM_MINING_JOB).unwrap())
        .await;
    let mut installed_payload = mining
        .receive(MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_SUCCESS)
        .await;
    let installed = SetCustomMiningJobSuccess::from_bytes(&mut installed_payload).unwrap();
    let private_header = test_bch_solve_custom_job(&private, &extranonce_prefix);
    mining
        .send(
            encode_message(
                SubmitSharesExtended {
                    channel_id: 1,
                    sequence_number: 2,
                    job_id: installed.job_id,
                    nonce: private_header.nonce,
                    ntime: private_header.time,
                    version: private.version,
                    extranonce: B032::new(&[0; 8]).unwrap(),
                },
                MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED,
            )
            .unwrap(),
        )
        .await;
    let mut submitted_payload = mining.receive(MESSAGE_TYPE_SUBMIT_SHARES_SUCCESS).await;
    let submitted = SubmitSharesSuccess::from_bytes(&mut submitted_payload).unwrap();
    assert_eq!(submitted.last_sequence_number, 2);
    let emission = emissions.try_recv().unwrap();
    assert_eq!(emission.header, private_header);
    assert!(emission.blocktemplate.transactions.is_empty());
    assert_ne!(
        emission.template_merkle_branches[0].as_byte_array(),
        selected.compute_txid().as_byte_array()
    );
    let share = ShareHeader::from_commitment_and_header(
        emission.share_commitment,
        emission.header,
        u64::from(emission.blocktemplate.height),
        emission.coinbase_proof,
    );
    assert_eq!(
        share
            .coinbase_proof
            .merkle_root(&share, &emission.template_merkle_branches)
            .unwrap(),
        private_header.merkle_root
    );
    let received = rpc.received_requests().await.unwrap();
    assert_eq!(received.len(), if cfg!(feature = "sim") { 1 } else { 2 });
    let proposal: serde_json::Value = serde_json::from_slice(&received[0].body).unwrap();
    assert_eq!(proposal["params"][0]["mode"], "proposal");
    drop(mining);
    drop(declarator);
    drop(private_declarator);
    shutdown_sender.send(()).unwrap();
    timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    let permits = timeout(
        Duration::from_secs(5),
        connection_limit.acquire_many_owned(3),
    )
    .await
    .unwrap()
    .unwrap();
    drop(permits);
}

/// A client that knows nothing of P2Pool's coinbase builds its jobs from
/// `RequestPayoutOutputs.Success` alone, in both Job Declaration modes: the
/// reply's outputs, version, sequence and lock time, with its own scriptSig
/// prefix and transactions. Each job is installed and mines a share.
#[tokio::test]
async fn a_standard_client_mines_on_requested_payout_outputs_in_both_modes() {
    let rpc = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_partial_json(
            serde_json::json!({"method": "getblocktemplate"}),
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"result": null, "error": null, "id": 0})),
        )
        .expect(1)
        .mount(&rpc)
        .await;
    Mock::given(method("POST"))
        .and(body_partial_json(
            serde_json::json!({"method": "submitblock"}),
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"result": null, "error": null, "id": 1})),
        )
        .expect(if cfg!(feature = "sim") { 0 } else { 1 })
        .mount(&rpc)
        .await;
    // A node taking a 1% donation and a 1% fee, both in the outputs' terms.
    let fee_address = bitcoin::Address::p2pkh(
        bitcoin::PubkeyHash::from_byte_array([0xfe; 20]),
        Network::Regtest,
    );
    let prepared = test_bch_prepared_with_cuts(Some(100), Some(fee_address.into()), Some(100));
    let template = prepared.template();
    let cache = PoolTransactionCache::new();
    cache.set_prepared(Arc::clone(&prepared));
    let (_template_sender, template_rx) = watch::channel(Some(Arc::clone(&prepared)));
    let (ctx, mut emissions, _dirs) = test_bch_stratum_context(&rpc.uri()).await;
    let authority = test_sv2_authority();
    let listener = bind("127.0.0.1", 0).await.unwrap();
    let address = listener.local_addr().unwrap();
    let connection_limit = Arc::new(Semaphore::new(3));
    let (shutdown_sender, shutdown_receiver) = watch::channel(());
    let server = tokio::spawn(serve(
        listener,
        Arc::clone(&connection_limit),
        cache,
        Some(MergeMiningCommitment::empty()),
        Some(Sv2Mining {
            ctx,
            version_mask: DEFAULT_VERSION_MASK,
            template_rx,
        }),
        Some(authority),
        None,
        shutdown_receiver,
    ));
    let payout = bitcoin::Address::p2pkh(
        bitcoin::PubkeyHash::from_byte_array([0x22; 20]),
        Network::Regtest,
    );
    // One identity for the channel and both declarators; the node's
    // configured share address owns the shares.
    let identity = encode_payout_address(&payout, BchShareNetwork::Regtest).unwrap();
    let mut mining = TestSv2Client::connect(address, authority.public_key).await;
    mining
        .send(
            encode_message(
                test_sv2_setup(Protocol::MiningProtocol, 2),
                MESSAGE_TYPE_SETUP_CONNECTION,
            )
            .unwrap(),
        )
        .await;
    mining.receive(MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS).await;
    mining
        .send(
            encode_message(
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
        )
        .await;
    let mut open_payload = mining
        .receive(MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL_SUCCESS)
        .await;
    let open = OpenExtendedMiningChannelSuccess::from_bytes(&mut open_payload).unwrap();
    let extranonce_prefix = open.extranonce_prefix.as_bytes().to_vec();
    let extensions = encode_extension_frame(
        EXTENSIONS_NEGOTIATION,
        REQUEST_EXTENSIONS,
        false,
        &extensions_payload(1, &[PAYOUT_OUTPUTS_EXTENSION]),
    )
    .unwrap();
    // The client's own scriptSig prefix, which P2Pool never chose.
    let prefix = Builder::new()
        .push_int(i64::from(template.height))
        .push_slice(b"kth")
        .into_bytes();

    let mut declarator = TestSv2Client::connect(address, authority.public_key).await;
    declarator
        .send_pair(
            encode_message(
                test_sv2_setup(Protocol::JobDeclarationProtocol, 1),
                MESSAGE_TYPE_SETUP_CONNECTION,
            )
            .unwrap(),
            extensions.clone(),
        )
        .await;
    declarator
        .receive(MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS)
        .await;
    assert_eq!(
        declarator
            .receive_extension(EXTENSIONS_NEGOTIATION, REQUEST_EXTENSIONS_SUCCESS)
            .await,
        extensions_payload(1, &[PAYOUT_OUTPUTS_EXTENSION])
    );
    declarator
        .send(
            encode_message(
                AllocateMiningJobToken {
                    request_id: 2,
                    user_identifier: identity.as_str().try_into().unwrap(),
                },
                MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN,
            )
            .unwrap(),
        )
        .await;
    let mut allocation_payload = declarator
        .receive(MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN_SUCCESS)
        .await;
    let allocated = AllocateMiningJobTokenSuccess::from_bytes(&mut allocation_payload).unwrap();
    let token = allocated.mining_job_token.as_bytes().to_vec();
    declarator
        .send(
            encode_extension_frame(
                PAYOUT_OUTPUTS_EXTENSION,
                REQUEST_PAYOUT_OUTPUTS,
                false,
                &test_payout_outputs_request(3, &token, template, template.coinbasevalue, &prefix),
            )
            .unwrap(),
        )
        .await;
    let mut success_payload = declarator
        .receive_extension(PAYOUT_OUTPUTS_EXTENSION, REQUEST_PAYOUT_OUTPUTS_SUCCESS)
        .await;
    let success = RequestPayoutOutputsSuccess::from_bytes(&mut success_payload).unwrap();
    assert_eq!(success.request_id, 3);
    assert_eq!(success.flags, EXACT_OUTPUTS);
    let selected: bitcoin::Transaction =
        deserialize(&hex::decode(bch_template_transaction(1).data).unwrap()).unwrap();
    let mut custom_bytes = test_custom_job_from_payout_outputs(
        &success,
        &token,
        template,
        &prefix,
        std::slice::from_ref(&selected),
    );
    let mut custom = SetCustomMiningJob::from_bytes(&mut custom_bytes).unwrap();
    let (coinbase_prefix, coinbase_suffix) = test_declared_coinbase_parts(&custom);
    let wtxid = selected.compute_wtxid().to_byte_array();
    declarator
        .send(
            encode_message(
                DeclareMiningJob {
                    request_id: 4,
                    mining_job_token: B0255::new(&token).unwrap(),
                    version: custom.version,
                    coinbase_tx_prefix: B064K::new(&coinbase_prefix).unwrap(),
                    coinbase_tx_suffix: B064K::new(&coinbase_suffix).unwrap(),
                    wtxid_list: Seq064K::new(vec![U256::new(&wtxid).unwrap()]).unwrap(),
                    excess_data: B064K::new(&[]).unwrap(),
                },
                MESSAGE_TYPE_DECLARE_MINING_JOB,
            )
            .unwrap(),
        )
        .await;
    declarator
        .receive(MESSAGE_TYPE_PROVIDE_MISSING_TRANSACTIONS)
        .await;
    let transaction_bytes = serialize(&selected);
    declarator
        .send(
            encode_message(
                ProvideMissingTransactionsSuccess {
                    request_id: 4,
                    transaction_list: Seq064K::new(vec![B016M::new(&transaction_bytes).unwrap()])
                        .unwrap(),
                },
                MESSAGE_TYPE_PROVIDE_MISSING_TRANSACTIONS_SUCCESS,
            )
            .unwrap(),
        )
        .await;
    let mut accepted_payload = declarator
        .receive(MESSAGE_TYPE_DECLARE_MINING_JOB_SUCCESS)
        .await;
    let accepted = DeclareMiningJobSuccess::from_bytes(&mut accepted_payload).unwrap();
    let new_token = accepted.new_mining_job_token.as_bytes().to_vec();
    custom.token = B0255::new(&new_token).unwrap();
    mining
        .send(encode_message(custom.clone(), MESSAGE_TYPE_SET_CUSTOM_MINING_JOB).unwrap())
        .await;
    let mut installed_payload = mining
        .receive(MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_SUCCESS)
        .await;
    let installed = SetCustomMiningJobSuccess::from_bytes(&mut installed_payload).unwrap();
    let header = test_bch_solve_custom_job(&custom, &extranonce_prefix);
    mining
        .send(
            encode_message(
                SubmitSharesExtended {
                    channel_id: 1,
                    sequence_number: 1,
                    job_id: installed.job_id,
                    nonce: header.nonce,
                    ntime: header.time,
                    version: custom.version,
                    extranonce: B032::new(&[0; 8]).unwrap(),
                },
                MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED,
            )
            .unwrap(),
        )
        .await;
    let mut submitted_payload = mining.receive(MESSAGE_TYPE_SUBMIT_SHARES_SUCCESS).await;
    let submitted = SubmitSharesSuccess::from_bytes(&mut submitted_payload).unwrap();
    assert_eq!(submitted.last_sequence_number, 1);
    let emission = emissions.try_recv().unwrap();
    assert_eq!(emission.header, header);
    assert_eq!(
        emission.blocktemplate.transactions[0].txid,
        selected.compute_txid().to_string()
    );
    let share = ShareHeader::from_commitment_and_header(
        emission.share_commitment,
        emission.header,
        u64::from(emission.blocktemplate.height),
        emission.coinbase_proof,
    );
    assert_eq!(
        share
            .coinbase_proof
            .merkle_root(&share, &emission.template_merkle_branches)
            .unwrap(),
        header.merkle_root
    );

    let mut private_declarator = TestSv2Client::connect(address, authority.public_key).await;
    private_declarator
        .send_pair(
            encode_message(
                test_sv2_setup(Protocol::JobDeclarationProtocol, 0),
                MESSAGE_TYPE_SETUP_CONNECTION,
            )
            .unwrap(),
            extensions,
        )
        .await;
    private_declarator
        .receive(MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS)
        .await;
    assert_eq!(
        private_declarator
            .receive_extension(EXTENSIONS_NEGOTIATION, REQUEST_EXTENSIONS_SUCCESS)
            .await,
        extensions_payload(1, &[PAYOUT_OUTPUTS_EXTENSION])
    );
    private_declarator
        .send(
            encode_message(
                AllocateMiningJobToken {
                    request_id: 5,
                    user_identifier: identity.as_str().try_into().unwrap(),
                },
                MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN,
            )
            .unwrap(),
        )
        .await;
    let mut allocation_payload = private_declarator
        .receive(MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN_SUCCESS)
        .await;
    let allocated = AllocateMiningJobTokenSuccess::from_bytes(&mut allocation_payload).unwrap();
    let private_token = allocated.mining_job_token.as_bytes().to_vec();
    private_declarator
        .send(
            encode_extension_frame(
                PAYOUT_OUTPUTS_EXTENSION,
                REQUEST_PAYOUT_OUTPUTS,
                false,
                &test_payout_outputs_request(
                    6,
                    &private_token,
                    template,
                    template.coinbasevalue,
                    &prefix,
                ),
            )
            .unwrap(),
        )
        .await;
    let mut success_payload = private_declarator
        .receive_extension(PAYOUT_OUTPUTS_EXTENSION, REQUEST_PAYOUT_OUTPUTS_SUCCESS)
        .await;
    let success = RequestPayoutOutputsSuccess::from_bytes(&mut success_payload).unwrap();
    assert_eq!(success.request_id, 6);
    let private_tx: bitcoin::Transaction =
        deserialize(&hex::decode(bch_template_transaction(9).data).unwrap()).unwrap();
    let mut private_bytes = test_custom_job_from_payout_outputs(
        &success,
        &private_token,
        template,
        &prefix,
        &[private_tx],
    );
    let private = SetCustomMiningJob::from_bytes(&mut private_bytes).unwrap();
    mining
        .send(encode_message(private.clone(), MESSAGE_TYPE_SET_CUSTOM_MINING_JOB).unwrap())
        .await;
    let mut installed_payload = mining
        .receive(MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_SUCCESS)
        .await;
    let installed = SetCustomMiningJobSuccess::from_bytes(&mut installed_payload).unwrap();
    let private_header = test_bch_solve_custom_job(&private, &extranonce_prefix);
    mining
        .send(
            encode_message(
                SubmitSharesExtended {
                    channel_id: 1,
                    sequence_number: 2,
                    job_id: installed.job_id,
                    nonce: private_header.nonce,
                    ntime: private_header.time,
                    version: private.version,
                    extranonce: B032::new(&[0; 8]).unwrap(),
                },
                MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED,
            )
            .unwrap(),
        )
        .await;
    let mut submitted_payload = mining.receive(MESSAGE_TYPE_SUBMIT_SHARES_SUCCESS).await;
    let submitted = SubmitSharesSuccess::from_bytes(&mut submitted_payload).unwrap();
    assert_eq!(submitted.last_sequence_number, 2);
    let emission = emissions.try_recv().unwrap();
    assert_eq!(emission.header, private_header);
    assert!(emission.blocktemplate.transactions.is_empty());
    let share = ShareHeader::from_commitment_and_header(
        emission.share_commitment,
        emission.header,
        u64::from(emission.blocktemplate.height),
        emission.coinbase_proof,
    );
    assert_eq!(
        share
            .coinbase_proof
            .merkle_root(&share, &emission.template_merkle_branches)
            .unwrap(),
        private_header.merkle_root
    );
    drop(mining);
    drop(declarator);
    drop(private_declarator);
    shutdown_sender.send(()).unwrap();
    timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    let permits = timeout(
        Duration::from_secs(5),
        connection_limit.acquire_many_owned(3),
    )
    .await
    .unwrap()
    .unwrap();
    drop(permits);
}

/// When the share new tokens are anchored on changes, a connection that
/// negotiated the extension is told, a request on a token from before gets
/// `stale-payout-outputs`, and a token allocated after gets its outputs.
#[tokio::test]
async fn a_moved_anchor_pushes_payout_outputs_stale_and_refuses_earlier_tokens() {
    let prepared = test_bch_prepared();
    let template = prepared.template();
    let cache = PoolTransactionCache::new();
    cache.set_prepared(Arc::clone(&prepared));
    let (template_sender, template_rx) = watch::channel(Some(Arc::clone(&prepared)));
    let (ctx, _emissions, _dirs) = test_bch_stratum_context("http://127.0.0.1:1").await;
    let authority = test_sv2_authority();
    let listener = bind("127.0.0.1", 0).await.unwrap();
    let address = listener.local_addr().unwrap();
    let (shutdown_sender, shutdown_receiver) = watch::channel(());
    let server = tokio::spawn(serve(
        listener,
        Arc::new(Semaphore::new(1)),
        cache.clone(),
        Some(MergeMiningCommitment::empty()),
        Some(Sv2Mining {
            ctx,
            version_mask: DEFAULT_VERSION_MASK,
            template_rx,
        }),
        Some(authority),
        None,
        shutdown_receiver,
    ));
    let payout = bitcoin::Address::p2pkh(
        bitcoin::PubkeyHash::from_byte_array([0x22; 20]),
        Network::Regtest,
    );
    let identity = encode_payout_address(&payout, BchShareNetwork::Regtest).unwrap();
    let prefix = Builder::new()
        .push_int(i64::from(template.height))
        .push_slice(b"kth")
        .into_bytes();
    let mut declarator = TestSv2Client::connect(address, authority.public_key).await;
    declarator
        .send_pair(
            encode_message(
                test_sv2_setup(Protocol::JobDeclarationProtocol, 0),
                MESSAGE_TYPE_SETUP_CONNECTION,
            )
            .unwrap(),
            encode_extension_frame(
                EXTENSIONS_NEGOTIATION,
                REQUEST_EXTENSIONS,
                false,
                &extensions_payload(1, &[PAYOUT_OUTPUTS_EXTENSION]),
            )
            .unwrap(),
        )
        .await;
    declarator
        .receive(MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS)
        .await;
    declarator
        .receive_extension(EXTENSIONS_NEGOTIATION, REQUEST_EXTENSIONS_SUCCESS)
        .await;
    declarator
        .send(
            encode_message(
                AllocateMiningJobToken {
                    request_id: 2,
                    user_identifier: identity.as_str().try_into().unwrap(),
                },
                MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN,
            )
            .unwrap(),
        )
        .await;
    let mut allocation_payload = declarator
        .receive(MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN_SUCCESS)
        .await;
    let allocated = AllocateMiningJobTokenSuccess::from_bytes(&mut allocation_payload).unwrap();
    let earlier_token = allocated.mining_job_token.as_bytes().to_vec();

    // A new share arrives under the same template, as the notifier
    // publishes it: the token cache first, then the template watch.
    let moved = test_bch_prepared_on_anchor(BlockHash::from_byte_array([0x44; 32]));
    cache.set_prepared(Arc::clone(&moved));
    template_sender.send(Some(moved)).unwrap();

    assert!(
        declarator
            .receive_extension(PAYOUT_OUTPUTS_EXTENSION, PAYOUT_OUTPUTS_STALE)
            .await
            .is_empty()
    );
    declarator
        .send(
            encode_extension_frame(
                PAYOUT_OUTPUTS_EXTENSION,
                REQUEST_PAYOUT_OUTPUTS,
                false,
                &test_payout_outputs_request(
                    3,
                    &earlier_token,
                    template,
                    template.coinbasevalue,
                    &prefix,
                ),
            )
            .unwrap(),
        )
        .await;
    let mut error_payload = declarator
        .receive_extension(PAYOUT_OUTPUTS_EXTENSION, REQUEST_PAYOUT_OUTPUTS_ERROR)
        .await;
    let error = RequestPayoutOutputsError::from_bytes(&mut error_payload).unwrap();
    assert_eq!(error.request_id, 3);
    assert_eq!(error.error_code.as_utf8_or_hex(), STALE_PAYOUT_OUTPUTS);
    declarator
        .send(
            encode_message(
                AllocateMiningJobToken {
                    request_id: 4,
                    user_identifier: identity.as_str().try_into().unwrap(),
                },
                MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN,
            )
            .unwrap(),
        )
        .await;
    let mut allocation_payload = declarator
        .receive(MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN_SUCCESS)
        .await;
    let allocated = AllocateMiningJobTokenSuccess::from_bytes(&mut allocation_payload).unwrap();
    let later_token = allocated.mining_job_token.as_bytes().to_vec();
    declarator
        .send(
            encode_extension_frame(
                PAYOUT_OUTPUTS_EXTENSION,
                REQUEST_PAYOUT_OUTPUTS,
                false,
                &test_payout_outputs_request(
                    5,
                    &later_token,
                    template,
                    template.coinbasevalue,
                    &prefix,
                ),
            )
            .unwrap(),
        )
        .await;
    let mut success_payload = declarator
        .receive_extension(PAYOUT_OUTPUTS_EXTENSION, REQUEST_PAYOUT_OUTPUTS_SUCCESS)
        .await;
    let success = RequestPayoutOutputsSuccess::from_bytes(&mut success_payload).unwrap();
    assert_eq!(success.request_id, 5);
    drop(declarator);
    shutdown_sender.send(()).unwrap();
    timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn disconnect_releases_the_authorized_worker_and_connection_permit() {
    let stats_dir = tempfile::tempdir().unwrap();
    let metrics = metrics::start_metrics(stats_dir.path().to_str().unwrap().to_string())
        .await
        .unwrap();
    let (chain_store_handle, _store_dir) = setup_test_chain_store_handle(true).await;
    let (emissions_tx, _emissions_rx) = mpsc::channel(10);
    let ctx = StratumContext {
        found_block_submitter: FoundBlockSubmitter::single_attempt(),
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
        metrics: metrics.clone(),
        chain_store_handle,
        mode: PoolMode::P2poolv2,
        miner_address: Some(make_test_share_address(1, Network::Signet)),
        share_chain: Chain::Bitcoin,
        bch_share_network: None,
        cash_share: None,
        template_refresh_tx: None,
        merge_mining_logs: None,
    };
    let listener = bind("127.0.0.1", 0).await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut client = TcpStream::connect(address).await.unwrap();
    let (stream, _) = listener.accept().await.unwrap();
    let registry = Arc::new(Mutex::new(JobRegistry::new(
        PoolTransactionCache::new(),
        None,
    )));
    let connection_limit = Arc::new(Semaphore::new(1));
    let permit = Arc::clone(&connection_limit).acquire_owned().await.unwrap();
    let (_template_sender, template_rx) = watch::channel(None);
    let (_shutdown_sender, shutdown_rx) = watch::channel(());
    let connection = tokio::spawn(handle_connection(
        stream,
        permit,
        Arc::clone(&registry),
        Some(Sv2Mining {
            ctx,
            version_mask: DEFAULT_VERSION_MASK,
            template_rx,
        }),
        None,
        None,
        shutdown_rx,
    ));
    let setup = encode_message(
        SetupConnection {
            protocol: Protocol::MiningProtocol,
            min_version: 2,
            max_version: 2,
            flags: 0,
            endpoint_host: "localhost".try_into().unwrap(),
            endpoint_port: address.port(),
            vendor: "p2poolv2".try_into().unwrap(),
            hardware_version: "test".try_into().unwrap(),
            firmware: "test".try_into().unwrap(),
            device_id: "".try_into().unwrap(),
        },
        MESSAGE_TYPE_SETUP_CONNECTION,
    )
    .unwrap();
    client.write_all(&setup).await.unwrap();
    let payout = "tb1q3udk7r26qs32ltf9nmqrjaaa7tr55qmkk30q5d";
    let username = format!("{payout}.rig1");
    let open = encode_message(
        OpenExtendedMiningChannel {
            request_id: 1,
            user_identity: username.as_str().try_into().unwrap(),
            nominal_hash_rate: 1.0,
            max_target: U256::new(&[0xff; 32]).unwrap(),
            min_extranonce_size: 8,
        },
        MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL,
    )
    .unwrap();
    client.write_all(&open).await.unwrap();
    let mut decoder = Sv2Connection::new(registry);
    timeout(Duration::from_secs(5), async {
        let mut opened = false;
        let mut buffer = [0u8; 512];
        while !opened {
            let count = client.read(&mut buffer).await.unwrap();
            assert_ne!(count, 0);
            opened = decoder
                .decode(&buffer[..count])
                .unwrap()
                .iter()
                .any(|message| {
                    message.message_type == MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL_SUCCESS
                });
        }
    })
    .await
    .unwrap();
    assert!(metrics.get_metrics().await.users[payout].workers["rig1"].active);
    drop(client);
    timeout(Duration::from_secs(5), connection)
        .await
        .unwrap()
        .unwrap();
    assert!(!metrics.get_metrics().await.users[payout].workers["rig1"].active);
    assert_eq!(connection_limit.available_permits(), 1);
}

#[test]
fn reading_the_current_template_leaves_it_for_next_template() {
    let (sender, receiver): (_, TemplateReceiver) = watch::channel(None);
    sender.send(None).unwrap();
    assert!(current_template(Some(&receiver)).is_none());
    assert!(receiver.has_changed().unwrap());
}

/// A Mining connection, set up, on a registry with no template.
fn mining_connection() -> Sv2Connection {
    let registry = Arc::new(Mutex::new(JobRegistry::new(
        PoolTransactionCache::new(),
        Some(MergeMiningCommitment::empty()),
    )));
    let mut connection = Sv2Connection::new(registry);
    let mut setup = binary_sv2::to_bytes(test_sv2_setup(Protocol::MiningProtocol, 0)).unwrap();
    connection
        .handle_message(
            common_messages_sv2::MESSAGE_TYPE_SETUP_CONNECTION,
            &mut setup,
        )
        .unwrap();
    connection
}

/// `RequestExtensions` asking for `0x4D4D` and an unknown extension on a
/// Mining connection is answered with `0x4D4D` alone, and the connection's
/// channels then get the merge-mining disclosure.
#[tokio::test]
async fn a_mining_connection_is_granted_the_merge_mining_disclosure() {
    let (ctx, _emissions, _dirs) = test_bch_stratum_context("http://127.0.0.1:1").await;
    let mut channels = MiningChannels::new(ctx, DEFAULT_VERSION_MASK);
    let mut connection = mining_connection();
    let request = Sv2Message {
        extension_type: EXTENSIONS_NEGOTIATION,
        message_type: REQUEST_EXTENSIONS,
        payload: extensions_payload(9, &[MERGE_DISCLOSURE_EXTENSION, 0x0002]),
    };

    let frames = handle_messages(
        &mut connection,
        Some(&mut channels),
        None,
        None,
        vec![request],
    )
    .await
    .unwrap();

    assert_eq!(
        frames,
        vec![
            [
                &[0x01, 0x00, REQUEST_EXTENSIONS_SUCCESS, 6, 0, 0][..],
                &extensions_payload(9, &[MERGE_DISCLOSURE_EXTENSION])[..],
            ]
            .concat()
        ]
    );
}

/// A connection that asks for no extension this node knows, or that has
/// no mining channels to disclose to, is answered with none.
#[tokio::test]
async fn an_unknown_extension_request_is_answered_with_none() {
    let mut connection = mining_connection();
    let request = Sv2Message {
        extension_type: EXTENSIONS_NEGOTIATION,
        message_type: REQUEST_EXTENSIONS,
        payload: extensions_payload(9, &[MERGE_DISCLOSURE_EXTENSION]),
    };

    let frames = handle_messages(&mut connection, None, None, None, vec![request])
        .await
        .unwrap();

    assert_eq!(
        frames,
        vec![
            [
                &[0x01, 0x00, REQUEST_EXTENSIONS_SUCCESS, 4, 0, 0][..],
                &extensions_payload(9, &[])[..],
            ]
            .concat()
        ]
    );
}

/// Extensions are negotiated after `SetupConnection`; a request before it,
/// or a message of an extension that was not negotiated, closes the
/// connection.
#[tokio::test]
async fn extension_messages_before_setup_or_unknown_close_the_connection() {
    let registry = Arc::new(Mutex::new(JobRegistry::new(
        PoolTransactionCache::new(),
        Some(MergeMiningCommitment::empty()),
    )));
    let mut fresh = Sv2Connection::new(registry);
    let request = Sv2Message {
        extension_type: EXTENSIONS_NEGOTIATION,
        message_type: REQUEST_EXTENSIONS,
        payload: extensions_payload(9, &[MERGE_DISCLOSURE_EXTENSION]),
    };
    assert!(
        handle_messages(&mut fresh, None, None, None, vec![request])
            .await
            .is_err()
    );

    let mut connection = mining_connection();
    let unknown = Sv2Message {
        extension_type: 0x0003,
        message_type: 0x00,
        payload: Vec::new(),
    };
    assert!(
        handle_messages(&mut connection, None, None, None, vec![unknown])
            .await
            .is_err()
    );
}

/// A Job Declaration connection of a Bitcoin Cash node, on a listener that
/// serves mining, is granted the requested payout outputs extension.
#[tokio::test]
async fn a_bch_job_declaration_connection_is_granted_requested_payout_outputs() {
    let (ctx, _emissions, _dirs) = test_bch_stratum_context("http://127.0.0.1:1").await;
    let mut channels = MiningChannels::new(ctx, DEFAULT_VERSION_MASK);
    let registry = Arc::new(Mutex::new(JobRegistry::new(
        PoolTransactionCache::new(),
        Some(MergeMiningCommitment::empty()),
    )));
    let mut connection = Sv2Connection::new(registry);
    let mut setup =
        binary_sv2::to_bytes(test_sv2_setup(Protocol::JobDeclarationProtocol, 0)).unwrap();
    connection
        .handle_message(MESSAGE_TYPE_SETUP_CONNECTION, &mut setup)
        .unwrap();
    let request = Sv2Message {
        extension_type: EXTENSIONS_NEGOTIATION,
        message_type: REQUEST_EXTENSIONS,
        payload: extensions_payload(9, &[PAYOUT_OUTPUTS_EXTENSION]),
    };

    let frames = handle_messages(
        &mut connection,
        Some(&mut channels),
        None,
        None,
        vec![request],
    )
    .await
    .unwrap();

    assert_eq!(
        frames,
        vec![
            [
                &[0x01, 0x00, REQUEST_EXTENSIONS_SUCCESS, 6, 0, 0][..],
                &extensions_payload(9, &[PAYOUT_OUTPUTS_EXTENSION])[..],
            ]
            .concat()
        ]
    );
    assert!(connection.has_payout_outputs());
}

/// A Mining connection keeps getting `0x4D4D` alone, and a Bitcoin node's
/// Job Declaration connection, whose tokens carry no P2Pool terms, gets
/// neither.
#[tokio::test]
async fn requested_payout_outputs_are_not_granted_to_mining_or_bitcoin_connections() {
    let (ctx, _emissions, _dirs) = test_bch_stratum_context("http://127.0.0.1:1").await;
    let mut channels = MiningChannels::new(ctx.clone(), DEFAULT_VERSION_MASK);
    let mut mining = mining_connection();
    let both = Sv2Message {
        extension_type: EXTENSIONS_NEGOTIATION,
        message_type: REQUEST_EXTENSIONS,
        payload: extensions_payload(9, &[MERGE_DISCLOSURE_EXTENSION, PAYOUT_OUTPUTS_EXTENSION]),
    };

    let mining_frames = handle_messages(&mut mining, Some(&mut channels), None, None, vec![both])
        .await
        .unwrap();

    assert_eq!(
        mining_frames,
        vec![
            [
                &[0x01, 0x00, REQUEST_EXTENSIONS_SUCCESS, 6, 0, 0][..],
                &extensions_payload(9, &[MERGE_DISCLOSURE_EXTENSION])[..],
            ]
            .concat()
        ]
    );
    assert!(!mining.has_payout_outputs());
    let mut bitcoin_channels = MiningChannels::new(ctx, DEFAULT_VERSION_MASK);
    let bitcoin_registry = Arc::new(Mutex::new(JobRegistry::new(
        PoolTransactionCache::new(),
        None,
    )));
    let mut bitcoin = Sv2Connection::new(bitcoin_registry);
    let mut setup =
        binary_sv2::to_bytes(test_sv2_setup(Protocol::JobDeclarationProtocol, 0)).unwrap();
    bitcoin
        .handle_message(MESSAGE_TYPE_SETUP_CONNECTION, &mut setup)
        .unwrap();
    let payout_outputs = Sv2Message {
        extension_type: EXTENSIONS_NEGOTIATION,
        message_type: REQUEST_EXTENSIONS,
        payload: extensions_payload(9, &[PAYOUT_OUTPUTS_EXTENSION]),
    };

    let bitcoin_frames = handle_messages(
        &mut bitcoin,
        Some(&mut bitcoin_channels),
        None,
        None,
        vec![payout_outputs],
    )
    .await
    .unwrap();

    assert_eq!(
        bitcoin_frames,
        vec![
            [
                &[0x01, 0x00, REQUEST_EXTENSIONS_SUCCESS, 4, 0, 0][..],
                &extensions_payload(9, &[])[..],
            ]
            .concat()
        ]
    );
}

/// A requested payout outputs message on a connection that did not
/// negotiate the extension closes it, as any unknown extension's does, and
/// so does a pool-to-client message of the extension sent by a client.
#[tokio::test]
async fn requested_payout_outputs_messages_outside_the_extension_close_the_connection() {
    let registry = Arc::new(Mutex::new(JobRegistry::new(
        PoolTransactionCache::new(),
        Some(MergeMiningCommitment::empty()),
    )));
    let mut connection = Sv2Connection::new(registry);
    let mut setup =
        binary_sv2::to_bytes(test_sv2_setup(Protocol::JobDeclarationProtocol, 0)).unwrap();
    connection
        .handle_message(MESSAGE_TYPE_SETUP_CONNECTION, &mut setup)
        .unwrap();
    let request = Sv2Message {
        extension_type: PAYOUT_OUTPUTS_EXTENSION,
        message_type: REQUEST_PAYOUT_OUTPUTS,
        payload: test_payout_outputs_request(1, &[1; 32], test_bch_prepared().template(), 1, &[]),
    };

    assert!(
        handle_messages(&mut connection, None, None, None, vec![request])
            .await
            .unwrap_err()
            .contains("unsupported Stratum V2 extension 0x5052")
    );

    assert!(connection.grant_payout_outputs());
    let stale = Sv2Message {
        extension_type: PAYOUT_OUTPUTS_EXTENSION,
        message_type: PAYOUT_OUTPUTS_STALE,
        payload: Vec::new(),
    };
    assert!(
        handle_messages(&mut connection, None, None, None, vec![stale])
            .await
            .is_err()
    );
}

/// `user_identifier` is resolved by `MiningChannels::payout_identity`, the
/// same way an allocation is. A string that is not a payout address is
/// `invalid-request`, and the connection stays up.
#[tokio::test]
async fn an_unresolvable_verify_identity_is_invalid_request() {
    let (ctx, _emissions, _dirs) = test_bch_stratum_context("http://127.0.0.1:1").await;
    let mut channels = MiningChannels::new(ctx, DEFAULT_VERSION_MASK);
    let registry = Arc::new(Mutex::new(JobRegistry::new(
        PoolTransactionCache::new(),
        Some(MergeMiningCommitment::empty()),
    )));
    let mut connection = Sv2Connection::new(registry);
    let mut setup =
        binary_sv2::to_bytes(test_sv2_setup(Protocol::JobDeclarationProtocol, 0)).unwrap();
    connection
        .handle_message(MESSAGE_TYPE_SETUP_CONNECTION, &mut setup)
        .unwrap();
    let negotiate = Sv2Message {
        extension_type: EXTENSIONS_NEGOTIATION,
        message_type: REQUEST_EXTENSIONS,
        payload: extensions_payload(9, &[PAYOUT_OUTPUTS_EXTENSION]),
    };
    handle_messages(
        &mut connection,
        Some(&mut channels),
        None,
        None,
        vec![negotiate],
    )
    .await
    .unwrap();
    assert!(connection.has_payout_outputs());
    let prepared = test_bch_prepared();
    let token = prepared
        .mining_job_token([0x11; 32], prepared.template().curtime)
        .unwrap();
    let verify = Sv2Message {
        extension_type: PAYOUT_OUTPUTS_EXTENSION,
        message_type: VERIFY_PAYOUT_OUTPUTS,
        payload: test_verify_payout_outputs_request(
            4,
            &token,
            "not-an-address",
            prepared.template(),
            prepared.template().coinbasevalue,
            &[],
        ),
    };

    let frames = handle_messages(
        &mut connection,
        Some(&mut channels),
        None,
        None,
        vec![verify],
    )
    .await
    .unwrap();

    assert_eq!(frames[0][2], VERIFY_PAYOUT_OUTPUTS_ERROR);
    let mut payload = SerializedFrame::<Vec<u8>>::from_bytes(frames[0].clone())
        .unwrap()
        .payload()
        .to_vec();
    let error = RequestPayoutOutputsError::from_bytes(&mut payload).unwrap();
    assert_eq!(error.error_code.as_utf8_or_hex(), INVALID_REQUEST);
}

#[tokio::test]
async fn bind_resolves_a_host_name() {
    let listener = bind("localhost", 0).await.unwrap();
    assert!(listener.local_addr().unwrap().ip().is_loopback());
}

#[tokio::test]
async fn bind_reports_an_unresolvable_host() {
    assert!(bind("no-such-host.invalid", 0).await.is_err());
}

#[tokio::test]
async fn noise_listener_answers_setup_connection_to_a_client_pinning_its_key() {
    let authority = test_sv2_authority();
    let listener = bind("localhost", 0).await.unwrap();
    let address = listener.local_addr().unwrap();
    let (shutdown_sender, shutdown_receiver) = watch::channel(());
    let server = tokio::spawn(serve(
        listener,
        Arc::new(Semaphore::new(8)),
        PoolTransactionCache::new(),
        None,
        None,
        Some(authority),
        None,
        shutdown_receiver,
    ));

    let mut client = TestSv2Client::connect(address, authority.public_key).await;
    client
        .send_message(
            MESSAGE_TYPE_SETUP_CONNECTION,
            test_sv2_setup(Protocol::MiningProtocol, 0),
        )
        .await;
    client.receive(MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS).await;

    shutdown_sender.send(()).unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn a_client_pinning_another_authority_key_rejects_the_handshake() {
    let authority = test_sv2_authority();
    let listener = bind("localhost", 0).await.unwrap();
    let address = listener.local_addr().unwrap();
    let (shutdown_sender, shutdown_receiver) = watch::channel(());
    let server = tokio::spawn(serve(
        listener,
        Arc::new(Semaphore::new(8)),
        PoolTransactionCache::new(),
        None,
        None,
        Some(authority),
        None,
        shutdown_receiver,
    ));

    let mut stream = TcpStream::connect(address).await.unwrap();
    // The x-only key of secret 0x01..01: a valid key, but not the authority.
    let other_key = Keypair::from_seckey_slice(&Secp256k1::new(), &[1u8; 32])
        .unwrap()
        .x_only_public_key()
        .0
        .serialize();
    let initiator = Initiator::from_raw_k(other_key).unwrap();
    let (hello, waiting) = Handshake::initiator(initiator).step_0().unwrap();
    let mut encoder = NoiseEncoder::new();
    stream
        .write_all(encoder.encode_handshake(hello).as_ref())
        .await
        .unwrap();
    let mut reply = [0u8; INITIATOR_EXPECTED_HANDSHAKE_MESSAGE_SIZE];
    stream.read_exact(&mut reply).await.unwrap();
    assert!(waiting.step_2(reply).is_err());

    shutdown_sender.send(()).unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn served_listener_answers_setup_connection() {
    let listener = bind("localhost", 0).await.unwrap();
    let address = listener.local_addr().unwrap();
    let (shutdown_sender, shutdown_receiver) = watch::channel(());
    let server = tokio::spawn(serve(
        listener,
        Arc::new(Semaphore::new(8)),
        PoolTransactionCache::new(),
        Some(MergeMiningCommitment::empty()),
        None,
        None,
        None,
        shutdown_receiver,
    ));

    let mut stream = TcpStream::connect(address).await.unwrap();
    let setup = encode_message(
        SetupConnection {
            protocol: Protocol::JobDeclarationProtocol,
            min_version: 2,
            max_version: 2,
            flags: 0,
            endpoint_host: "localhost".try_into().unwrap(),
            endpoint_port: address.port(),
            vendor: "p2poolv2".try_into().unwrap(),
            hardware_version: "test".try_into().unwrap(),
            firmware: "test".try_into().unwrap(),
            device_id: "".try_into().unwrap(),
        },
        MESSAGE_TYPE_SETUP_CONNECTION,
    )
    .unwrap();
    stream.write_all(&setup).await.unwrap();
    let mut response = vec![0u8; 64];
    let read = stream.read(&mut response).await.unwrap();
    response.truncate(read);
    let frame = SerializedFrame::<Vec<u8>>::from_bytes(response).unwrap();
    assert_eq!(
        frame.header().msg_type(),
        MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS
    );

    shutdown_sender.send(()).unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn a_connection_over_the_shared_limit_is_closed() {
    let listener = bind("localhost", 0).await.unwrap();
    let address = listener.local_addr().unwrap();
    let (shutdown_sender, shutdown_receiver) = watch::channel(());
    let server = tokio::spawn(serve(
        listener,
        Arc::new(Semaphore::new(0)),
        PoolTransactionCache::new(),
        Some(MergeMiningCommitment::empty()),
        None,
        None,
        None,
        shutdown_receiver,
    ));

    let mut stream = TcpStream::connect(address).await.unwrap();
    let setup = encode_message(
        SetupConnection {
            protocol: Protocol::JobDeclarationProtocol,
            min_version: 2,
            max_version: 2,
            flags: 0,
            endpoint_host: "localhost".try_into().unwrap(),
            endpoint_port: address.port(),
            vendor: "p2poolv2".try_into().unwrap(),
            hardware_version: "test".try_into().unwrap(),
            firmware: "test".try_into().unwrap(),
            device_id: "".try_into().unwrap(),
        },
        MESSAGE_TYPE_SETUP_CONNECTION,
    )
    .unwrap();
    let _ = stream.write_all(&setup).await;
    let mut response = vec![0u8; 64];
    let read = stream.read(&mut response).await.unwrap_or(0);
    assert_eq!(read, 0, "a connection without a permit gets no reply");

    shutdown_sender.send(()).unwrap();
    server.await.unwrap();
}
