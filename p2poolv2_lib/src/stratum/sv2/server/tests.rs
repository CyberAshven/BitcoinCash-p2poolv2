use super::*;
use crate::accounting::stats::metrics;
use crate::config::{Chain, DEFAULT_VERSION_MASK, PoolMode};
use crate::shares::merge_mining::MergeMiningCommitment;
use crate::shares::share_block::ShareHeader;
use crate::stratum::sv2::PoolTransactionCache;
use crate::stratum::sv2::session::encode_message;
use crate::stratum::work::tracker::start_tracker_actor;
use crate::test_utils::{
    TestSv2Client, bch_template_transaction, test_bch_custom_job, test_bch_prepared,
    test_bch_solve_custom_job, test_bch_stratum_context, test_sv2_authority, test_sv2_setup,
};
use crate::test_utils::{make_test_share_address, setup_test_chain_store_handle};
use binary_sv2::U256;
use binary_sv2::{B016M, B032, B064K, B0255, Seq064K};
use bitcoin::Network;
use bitcoin::consensus::{deserialize, serialize};
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::{Keypair, Secp256k1};
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
    let prepared = test_bch_prepared();
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
        Some(MergeMiningCommitment::reserved()),
        Some(Sv2Mining {
            ctx,
            version_mask: DEFAULT_VERSION_MASK,
            template_rx,
        }),
        Some(authority),
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
    let (mut custom_bytes, declared) = test_bch_custom_job(
        Arc::clone(&prepared),
        token.clone(),
        vec![selected.clone()],
        true,
        prepared.template().coinbasevalue,
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
                    excess_data: B064K::new(&[0; 5000]).unwrap(),
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

#[tokio::test]
async fn disconnect_releases_the_authorized_worker_and_connection_permit() {
    let stats_dir = tempfile::tempdir().unwrap();
    let metrics = metrics::start_metrics(stats_dir.path().to_str().unwrap().to_string())
        .await
        .unwrap();
    let (chain_store_handle, _store_dir) = setup_test_chain_store_handle(true).await;
    let (emissions_tx, _emissions_rx) = mpsc::channel(10);
    let ctx = StratumContext {
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
                .any(|(message_type, _)| {
                    *message_type == MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL_SUCCESS
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
        Some(MergeMiningCommitment::reserved()),
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
        Some(MergeMiningCommitment::reserved()),
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
