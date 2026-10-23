use super::*;
use crate::test_utils::test_bch_prepared;
use binary_sv2::{B016M, U256};
use bitcoin::absolute::LockTime;
use bitcoin::consensus::serialize;
use bitcoin::script::Builder;
use bitcoin::transaction::{Sequence, TxIn, TxOut, Version};
use bitcoin::{Amount, OutPoint, ScriptBuf, Witness};

fn registry() -> Arc<Mutex<JobRegistry>> {
    let cache = PoolTransactionCache::new();
    cache.set_prepared(test_bch_prepared());
    Arc::new(Mutex::new(JobRegistry::new(
        cache,
        Some(MergeMiningCommitment::empty()),
    )))
}

fn transaction_with_lock(lock_time: u32) -> Transaction {
    Transaction {
        version: Version::TWO,
        lock_time: LockTime::from_consensus(lock_time),
        input: vec![TxIn {
            previous_output: OutPoint::null(),
            script_sig: Builder::new().push_int(i64::from(lock_time)).into_script(),
            sequence: Sequence::MAX,
            witness: Witness::new(),
        }],
        output: vec![TxOut {
            value: Amount::from_sat(1),
            script_pubkey: ScriptBuf::new(),
        }],
    }
}

fn setup(flags: u32, protocol: Protocol) -> SetupConnection<'static> {
    SetupConnection {
        protocol,
        min_version: PROTOCOL_VERSION,
        max_version: PROTOCOL_VERSION,
        flags,
        endpoint_host: "127.0.0.1".try_into().unwrap(),
        endpoint_port: 0,
        vendor: "p2poolv2".try_into().unwrap(),
        hardware_version: "test".try_into().unwrap(),
        firmware: "test".try_into().unwrap(),
        device_id: "".try_into().unwrap(),
    }
}

fn accept_message<T>(connection: &mut Sv2Connection, message: T, message_type: u8) -> Vec<Vec<u8>>
where
    T: binary_sv2::Serialize + binary_sv2::GetSize,
{
    let frame = encode_message(message, message_type).unwrap();
    connection.accept(&frame).unwrap()
}

fn decode_success_flags(frame: &[u8]) -> u32 {
    let mut connection = Sv2Connection::new(registry());
    connection.inbox.extend_from_slice(frame);
    let room = connection.decoder.writable_len();
    connection
        .decoder
        .writable()
        .copy_from_slice(&connection.inbox[..room]);
    connection.inbox.drain(..room);
    let mut payload_frame = match connection.decoder.next_frame().unwrap() {
        Decoded::Frame(frame) => frame,
        Decoded::Incomplete(_) => {
            let room = connection.decoder.writable_len();
            connection
                .decoder
                .writable()
                .copy_from_slice(&connection.inbox[..room]);
            connection.inbox.drain(..room);
            match connection.decoder.next_frame().unwrap() {
                Decoded::Frame(frame) => frame,
                Decoded::Incomplete(_) => panic!("response frame did not complete"),
            }
        }
    };
    let mut payload = payload_frame.payload().to_vec();
    let success = SetupConnectionSuccess::from_bytes(&mut payload).unwrap();
    success.flags
}

#[test]
fn full_template_reorders_revealed_transactions_into_ctor() {
    let shared = registry();
    let mut connection = Sv2Connection::new(Arc::clone(&shared));
    let setup_response = accept_message(
        &mut connection,
        setup(1, Protocol::JobDeclarationProtocol),
        MESSAGE_TYPE_SETUP_CONNECTION,
    );
    assert_eq!(decode_success_flags(&setup_response[0]), 0);

    let allocate = accept_message(
        &mut connection,
        AllocateMiningJobToken {
            user_identifier: "miner".try_into().unwrap(),
            request_id: 7,
        },
        MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN,
    );
    assert_eq!(allocate.len(), 1);
    let mut payload = response_payload(&allocate[0]);
    let allocated = AllocateMiningJobTokenSuccess::from_bytes(&mut payload).unwrap();
    let outputs: Vec<TxOut> = deserialize(allocated.coinbase_outputs.as_bytes()).unwrap();
    // The token lists the scripts the template's own value pays, at zero
    // value: under the coinbase budget which payees get an output depends
    // on the amount, so a value of zero would name other scripts.
    let prepared = test_bch_prepared();
    let expected = prepared
        .declared_outputs(prepared.template().coinbasevalue)
        .unwrap()
        .into_iter()
        .map(|pair| TxOut {
            value: bitcoin::Amount::ZERO,
            script_pubkey: pair.address.script_pubkey(),
        })
        .collect::<Vec<_>>();
    assert_eq!(outputs, expected);
    let token = allocated.mining_job_token.as_bytes().to_vec();

    let first = transaction_with_lock(1);
    let second = transaction_with_lock(2);
    let mut declared = vec![first.clone(), second.clone()];
    let reversed = ctor_is_reversed(&first, &second);
    if !reversed {
        declared = vec![second.clone(), first.clone()];
    }
    let wtxid_bytes = declared
        .iter()
        .map(|transaction| transaction.compute_wtxid().to_byte_array())
        .collect::<Vec<_>>();
    let wtxids = wtxid_bytes
        .iter()
        .map(|wtxid| U256::new(wtxid.as_slice()).unwrap())
        .collect::<Vec<_>>();
    let declared_response = accept_message(
        &mut connection,
        DeclareMiningJob {
            request_id: 8,
            mining_job_token: B0255::new(&token).unwrap(),
            version: 1,
            coinbase_tx_prefix: B064K::new(&[]).unwrap(),
            coinbase_tx_suffix: B064K::new(&[]).unwrap(),
            wtxid_list: Seq064K::new(wtxids).unwrap(),
            excess_data: B064K::new(&[]).unwrap(),
        },
        MESSAGE_TYPE_DECLARE_MINING_JOB,
    );
    let mut missing_payload = response_payload(&declared_response[0]);
    let missing = ProvideMissingTransactions::from_bytes(&mut missing_payload).unwrap();
    assert_eq!(missing.unknown_tx_position_list.as_slice(), &[0, 1]);

    let serialized = declared.iter().map(serialize).collect::<Vec<_>>();
    let revealed = serialized
        .iter()
        .map(|bytes| B016M::new(bytes.as_slice()).unwrap())
        .collect::<Vec<_>>();
    let success = accept_message(
        &mut connection,
        ProvideMissingTransactionsSuccess {
            request_id: 8,
            transaction_list: Seq064K::new(revealed).unwrap(),
        },
        MESSAGE_TYPE_PROVIDE_MISSING_TRANSACTIONS_SUCCESS,
    );
    let mut success_payload = response_payload(&success[0]);
    let accepted = DeclareMiningJobSuccess::from_bytes(&mut success_payload).unwrap();
    let stored = shared
        .lock()
        .unwrap()
        .transactions_for_token(accepted.new_mining_job_token.as_bytes())
        .unwrap();
    let declared_first = declared[0].lock_time;
    let mut expected = declared;
    sort_ctor(&mut expected);
    assert_eq!(stored, expected);
    assert_ne!(stored[0].lock_time, declared_first);

    let token = accepted.new_mining_job_token.as_bytes();
    let mut mining = Sv2Connection::new(shared);
    accept_message(
        &mut mining,
        setup(0, Protocol::MiningProtocol),
        MESSAGE_TYPE_SETUP_CONNECTION,
    );
    let custom = accept_message(
        &mut mining,
        custom_job(token),
        MESSAGE_TYPE_SET_CUSTOM_MINING_JOB,
    );
    let mut custom_payload = response_payload(&custom[0]);
    let rejected = SetCustomMiningJobError::from_bytes(&mut custom_payload).unwrap();
    assert_eq!(
        rejected.error_code.as_utf8_or_hex(),
        "job-not-yet-validated"
    );
}

#[test]
fn two_connections_reusing_a_request_id_keep_their_own_transactions() {
    let shared = registry();
    let mut first_connection = Sv2Connection::new(Arc::clone(&shared));
    let mut second_connection = Sv2Connection::new(Arc::clone(&shared));
    accept_message(
        &mut first_connection,
        setup(1, Protocol::JobDeclarationProtocol),
        MESSAGE_TYPE_SETUP_CONNECTION,
    );
    accept_message(
        &mut second_connection,
        setup(1, Protocol::JobDeclarationProtocol),
        MESSAGE_TYPE_SETUP_CONNECTION,
    );
    let first_allocate = accept_message(
        &mut first_connection,
        AllocateMiningJobToken {
            user_identifier: "first".try_into().unwrap(),
            request_id: 1,
        },
        MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN,
    );
    let second_allocate = accept_message(
        &mut second_connection,
        AllocateMiningJobToken {
            user_identifier: "second".try_into().unwrap(),
            request_id: 1,
        },
        MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN,
    );
    let mut first_payload = response_payload(&first_allocate[0]);
    let first_token = AllocateMiningJobTokenSuccess::from_bytes(&mut first_payload)
        .unwrap()
        .mining_job_token
        .as_bytes()
        .to_vec();
    let mut second_payload = response_payload(&second_allocate[0]);
    let second_token = AllocateMiningJobTokenSuccess::from_bytes(&mut second_payload)
        .unwrap()
        .mining_job_token
        .as_bytes()
        .to_vec();

    let first_transaction = transaction_with_lock(1);
    let second_transaction = transaction_with_lock(2);
    let first_wtxid = first_transaction.compute_wtxid().to_byte_array();
    let second_wtxid = second_transaction.compute_wtxid().to_byte_array();
    accept_message(
        &mut first_connection,
        DeclareMiningJob {
            request_id: 1,
            mining_job_token: B0255::new(&first_token).unwrap(),
            version: 1,
            coinbase_tx_prefix: B064K::new(&[]).unwrap(),
            coinbase_tx_suffix: B064K::new(&[]).unwrap(),
            wtxid_list: Seq064K::new(vec![U256::new(first_wtxid.as_slice()).unwrap()]).unwrap(),
            excess_data: B064K::new(&[]).unwrap(),
        },
        MESSAGE_TYPE_DECLARE_MINING_JOB,
    );
    accept_message(
        &mut second_connection,
        DeclareMiningJob {
            request_id: 1,
            mining_job_token: B0255::new(&second_token).unwrap(),
            version: 1,
            coinbase_tx_prefix: B064K::new(&[]).unwrap(),
            coinbase_tx_suffix: B064K::new(&[]).unwrap(),
            wtxid_list: Seq064K::new(vec![U256::new(second_wtxid.as_slice()).unwrap()]).unwrap(),
            excess_data: B064K::new(&[]).unwrap(),
        },
        MESSAGE_TYPE_DECLARE_MINING_JOB,
    );

    let second_serialized = serialize(&second_transaction);
    let second_success = accept_message(
        &mut second_connection,
        ProvideMissingTransactionsSuccess {
            request_id: 1,
            transaction_list: Seq064K::new(vec![B016M::new(second_serialized.as_slice()).unwrap()])
                .unwrap(),
        },
        MESSAGE_TYPE_PROVIDE_MISSING_TRANSACTIONS_SUCCESS,
    );
    let first_serialized = serialize(&first_transaction);
    let first_success = accept_message(
        &mut first_connection,
        ProvideMissingTransactionsSuccess {
            request_id: 1,
            transaction_list: Seq064K::new(vec![B016M::new(first_serialized.as_slice()).unwrap()])
                .unwrap(),
        },
        MESSAGE_TYPE_PROVIDE_MISSING_TRANSACTIONS_SUCCESS,
    );

    let mut second_success_payload = response_payload(&second_success[0]);
    let second_ready = DeclareMiningJobSuccess::from_bytes(&mut second_success_payload)
        .unwrap()
        .new_mining_job_token
        .as_bytes()
        .to_vec();
    let mut first_success_payload = response_payload(&first_success[0]);
    let first_ready = DeclareMiningJobSuccess::from_bytes(&mut first_success_payload)
        .unwrap()
        .new_mining_job_token
        .as_bytes()
        .to_vec();
    let registry = shared.lock().unwrap();
    assert_eq!(
        registry.transactions_for_token(&first_ready).unwrap(),
        vec![first_transaction]
    );
    assert_eq!(
        registry.transactions_for_token(&second_ready).unwrap(),
        vec![second_transaction]
    );
}

#[test]
fn a_second_declaration_with_a_pending_request_id_is_rejected() {
    let mut connection = Sv2Connection::new(registry());
    accept_message(
        &mut connection,
        setup(1, Protocol::JobDeclarationProtocol),
        MESSAGE_TYPE_SETUP_CONNECTION,
    );
    let first_allocate = accept_message(
        &mut connection,
        AllocateMiningJobToken {
            user_identifier: "miner".try_into().unwrap(),
            request_id: 1,
        },
        MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN,
    );
    let second_allocate = accept_message(
        &mut connection,
        AllocateMiningJobToken {
            user_identifier: "miner".try_into().unwrap(),
            request_id: 2,
        },
        MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN,
    );
    let mut first_payload = response_payload(&first_allocate[0]);
    let first_token = AllocateMiningJobTokenSuccess::from_bytes(&mut first_payload)
        .unwrap()
        .mining_job_token
        .as_bytes()
        .to_vec();
    let mut second_payload = response_payload(&second_allocate[0]);
    let second_token = AllocateMiningJobTokenSuccess::from_bytes(&mut second_payload)
        .unwrap()
        .mining_job_token
        .as_bytes()
        .to_vec();
    accept_message(
        &mut connection,
        DeclareMiningJob {
            request_id: 5,
            mining_job_token: B0255::new(&first_token).unwrap(),
            version: 1,
            coinbase_tx_prefix: B064K::new(&[]).unwrap(),
            coinbase_tx_suffix: B064K::new(&[]).unwrap(),
            wtxid_list: Seq064K::new(vec![]).unwrap(),
            excess_data: B064K::new(&[]).unwrap(),
        },
        MESSAGE_TYPE_DECLARE_MINING_JOB,
    );
    let rejected = accept_message(
        &mut connection,
        DeclareMiningJob {
            request_id: 5,
            mining_job_token: B0255::new(&second_token).unwrap(),
            version: 1,
            coinbase_tx_prefix: B064K::new(&[]).unwrap(),
            coinbase_tx_suffix: B064K::new(&[]).unwrap(),
            wtxid_list: Seq064K::new(vec![]).unwrap(),
            excess_data: B064K::new(&[]).unwrap(),
        },
        MESSAGE_TYPE_DECLARE_MINING_JOB,
    );
    let mut error_payload = response_payload(&rejected[0]);
    let error = DeclareMiningJobError::from_bytes(&mut error_payload).unwrap();
    assert_eq!(
        error.error_code.as_utf8_or_hex(),
        ERROR_CODE_DECLARE_MINING_JOB_INVALID_JOB
    );
}

#[test]
fn bitcoin_tokens_carry_an_empty_output_list() {
    let shared = Arc::new(Mutex::new(JobRegistry::new(
        PoolTransactionCache::new(),
        None,
    )));
    let mut connection = Sv2Connection::new(shared);
    accept_message(
        &mut connection,
        setup(0, Protocol::JobDeclarationProtocol),
        MESSAGE_TYPE_SETUP_CONNECTION,
    );
    let allocate = accept_message(
        &mut connection,
        AllocateMiningJobToken {
            user_identifier: "miner".try_into().unwrap(),
            request_id: 1,
        },
        MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN,
    );
    let mut payload = response_payload(&allocate[0]);
    let allocated = AllocateMiningJobTokenSuccess::from_bytes(&mut payload).unwrap();
    let outputs: Vec<TxOut> = deserialize(allocated.coinbase_outputs.as_bytes()).unwrap();
    assert!(outputs.is_empty());
}

fn ctor_is_reversed(first: &Transaction, second: &Transaction) -> bool {
    let mut pair = [first.clone(), second.clone()];
    sort_ctor(&mut pair);
    pair[0].lock_time != first.lock_time
}

fn response_payload(frame: &[u8]) -> Vec<u8> {
    let mut owned = frame.to_vec();
    let mut parsed = SerializedFrame::<Vec<u8>>::from_bytes(owned.clone()).unwrap();
    let _ = &mut owned;
    parsed.payload().to_vec()
}

#[test]
fn coinbase_only_keeps_transactions_private_and_rejects_declare() {
    let cache = PoolTransactionCache::new();
    cache.set_prepared(test_bch_prepared());
    let first = transaction_with_lock(1);
    let second = transaction_with_lock(2);
    let mut pool = vec![first.clone(), second.clone()];
    sort_ctor(&mut pool);
    pool.reverse();
    cache.set(pool);
    let shared = Arc::new(Mutex::new(JobRegistry::new(
        cache,
        Some(MergeMiningCommitment::empty()),
    )));

    let mut declaration = Sv2Connection::new(Arc::clone(&shared));
    let setup_response = accept_message(
        &mut declaration,
        setup(0, Protocol::JobDeclarationProtocol),
        MESSAGE_TYPE_SETUP_CONNECTION,
    );
    assert_eq!(decode_success_flags(&setup_response[0]), 0);
    let allocate = accept_message(
        &mut declaration,
        AllocateMiningJobToken {
            user_identifier: "miner".try_into().unwrap(),
            request_id: 3,
        },
        MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN,
    );
    let mut payload = response_payload(&allocate[0]);
    let allocated = AllocateMiningJobTokenSuccess::from_bytes(&mut payload).unwrap();
    let token = allocated.mining_job_token.as_bytes().to_vec();

    let rejected = accept_message(
        &mut declaration,
        DeclareMiningJob {
            request_id: 4,
            mining_job_token: B0255::new(&token).unwrap(),
            version: 1,
            coinbase_tx_prefix: B064K::new(&[]).unwrap(),
            coinbase_tx_suffix: B064K::new(&[]).unwrap(),
            wtxid_list: Seq064K::new(vec![]).unwrap(),
            excess_data: B064K::new(&[]).unwrap(),
        },
        MESSAGE_TYPE_DECLARE_MINING_JOB,
    );
    let mut error_payload = response_payload(&rejected[0]);
    let error = DeclareMiningJobError::from_bytes(&mut error_payload).unwrap();
    assert_eq!(
        error.error_code.as_utf8_or_hex(),
        ERROR_CODE_DECLARE_MINING_JOB_INVALID_JOB
    );

    let mut mining = Sv2Connection::new(shared);
    accept_message(
        &mut mining,
        setup(0, Protocol::MiningProtocol),
        MESSAGE_TYPE_SETUP_CONNECTION,
    );
    let custom = accept_message(
        &mut mining,
        custom_job(&token),
        MESSAGE_TYPE_SET_CUSTOM_MINING_JOB,
    );
    let mut custom_payload = response_payload(&custom[0]);
    let rejected = SetCustomMiningJobError::from_bytes(&mut custom_payload).unwrap();
    assert_eq!(rejected.channel_id, 1);
    assert_eq!(rejected.request_id, 9);
    assert_eq!(
        rejected.error_code.as_utf8_or_hex(),
        "job-not-yet-validated"
    );
    assert!(
        mining
            .registry
            .lock()
            .unwrap()
            .transactions_for_token(&token)
            .is_none()
    );
    let private = mining.take_declared_job(&token).unwrap();
    assert!(private.transactions.is_none());
    assert!(mining.take_declared_job(&token).is_err());
}

#[test]
fn bitcoin_work_selection_setup_is_rejected() {
    let shared = Arc::new(Mutex::new(JobRegistry::new(
        PoolTransactionCache::new(),
        None,
    )));
    let mut mining = Sv2Connection::new(shared);
    let responses = accept_message(
        &mut mining,
        setup(WORK_SELECTION_FLAG, Protocol::MiningProtocol),
        MESSAGE_TYPE_SETUP_CONNECTION,
    );
    let mut payload = response_payload(&responses[0]);
    let rejected = SetupConnectionError::from_bytes(&mut payload).unwrap();
    assert_eq!(rejected.flags, WORK_SELECTION_FLAG);
    assert_eq!(
        rejected.error_code.as_utf8_or_hex(),
        "unsupported-feature-flags"
    );
    assert!(!mining.is_mining());
}

#[test]
fn bch_work_selection_setup_is_negotiated() {
    let mut mining = Sv2Connection::new(registry());
    let responses = accept_message(
        &mut mining,
        setup(WORK_SELECTION_FLAG, Protocol::MiningProtocol),
        MESSAGE_TYPE_SETUP_CONNECTION,
    );
    assert_eq!(decode_success_flags(&responses[0]), 0);
    assert!(mining.is_mining());
    assert!(mining.can_select_work());
}

#[test]
fn expired_tokens_cannot_install_a_job() {
    let shared = registry();
    let prepared = test_bch_prepared();
    let token = prepared
        .mining_job_token([1; 32], prepared.template().curtime)
        .unwrap();
    shared.lock().unwrap().tokens.insert(
        token.clone(),
        JobToken {
            allocated: Instant::now() - TOKEN_LIFETIME - Duration::from_secs(1),
            prepared: Some(prepared),
            state: TokenState::Open {
                mode: JobDeclarationMode::CoinbaseOnly,
            },
        },
    );
    assert!(shared.lock().unwrap().take_job(&token).is_err());
    assert!(shared.lock().unwrap().tokens.is_empty());
}

#[test]
fn allocations_are_unpredictable_and_bounded() {
    let mut connection = Sv2Connection::new(registry());
    accept_message(
        &mut connection,
        setup(0, Protocol::JobDeclarationProtocol),
        MESSAGE_TYPE_SETUP_CONNECTION,
    );
    let first = connection.on_allocate(1).unwrap();
    let second = connection.on_allocate(2).unwrap();
    let mut first_payload = response_payload(&first[0]);
    let mut second_payload = response_payload(&second[0]);
    let first = AllocateMiningJobTokenSuccess::from_bytes(&mut first_payload).unwrap();
    let second = AllocateMiningJobTokenSuccess::from_bytes(&mut second_payload).unwrap();
    assert_ne!(
        first.mining_job_token.as_bytes(),
        second.mining_job_token.as_bytes()
    );
    for request_id in 2..MAX_JOB_TOKENS {
        connection.on_allocate(request_id as u32).unwrap();
    }
    assert_eq!(
        connection.on_allocate(99).unwrap_err(),
        "job token limit reached"
    );
}

fn custom_job(token: &[u8]) -> SetCustomMiningJob<'_> {
    SetCustomMiningJob {
        channel_id: 1,
        request_id: 9,
        token: B0255::new(token).unwrap(),
        version: 1,
        prev_hash: U256::new(&[0u8; 32]).unwrap(),
        min_ntime: 0,
        nbits: 0x1d00ffff,
        coinbase_tx_version: 2,
        coinbase_prefix: B0255::new(&[]).unwrap(),
        coinbase_tx_input_n_sequence: 0xffff_fffe,
        coinbase_tx_outputs: B064K::new(&[]).unwrap(),
        coinbase_tx_locktime: 0,
        merkle_path: binary_sv2::Seq0255::new(vec![]).unwrap(),
    }
}
