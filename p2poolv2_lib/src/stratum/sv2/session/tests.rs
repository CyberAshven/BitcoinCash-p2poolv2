use super::*;
use crate::test_utils::test_bch_prepared;
use binary_sv2::{B016M, U256};
use bitcoin::absolute::LockTime;
use bitcoin::consensus::serialize;
use bitcoin::script::Builder;
use bitcoin::transaction::{Sequence, TxIn, TxOut, Version};
use bitcoin::{Amount, OutPoint, ScriptBuf, Witness};

#[test]
fn withdrawing_the_template_clears_job_declaration_work() {
    let cache = PoolTransactionCache::new();
    cache.set_prepared(test_bch_prepared());
    assert!(cache.prepared.read().unwrap().is_some());
    cache.clear_prepared();
    assert!(cache.prepared.read().unwrap().is_none());
    cache.set_prepared(test_bch_prepared());
    assert!(cache.prepared.read().unwrap().is_some());
}

fn registry() -> Arc<Mutex<JobRegistry>> {
    let cache = PoolTransactionCache::new();
    cache.set_prepared(test_bch_prepared());
    Arc::new(Mutex::new(JobRegistry::new(
        cache,
        Some(MergeMiningCommitment::empty()),
    )))
}

/// A declared coinbase's bytes after the extranonce whose only output is
/// `commitment`: the sequence, the output list and the locktime.
fn commitment_suffix(commitment: &MergeMiningCommitment) -> Vec<u8> {
    let mut suffix = 0xffff_fffeu32.to_le_bytes().to_vec();
    suffix.extend(serialize(&vec![commitment.output()]));
    suffix.extend([0; 4]);
    suffix
}

/// A declared coinbase suffix with the empty commitment at output 0.
fn empty_commitment_suffix() -> Vec<u8> {
    commitment_suffix(&MergeMiningCommitment::empty())
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
    // value, for a declared coinbase with the empty commitment: the
    // amounts decide which payees fit directly, so a value of zero would
    // name other scripts.
    let prepared = test_bch_prepared();
    let expected = prepared
        .declared_outputs(
            prepared.template().coinbasevalue,
            &MergeMiningCommitment::empty(),
        )
        .unwrap()
        .outputs
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
            coinbase_tx_suffix: B064K::new(&empty_commitment_suffix()).unwrap(),
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
            coinbase_tx_suffix: B064K::new(&empty_commitment_suffix()).unwrap(),
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
            coinbase_tx_suffix: B064K::new(&empty_commitment_suffix()).unwrap(),
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
            coinbase_tx_suffix: B064K::new(&empty_commitment_suffix()).unwrap(),
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
            coinbase_tx_suffix: B064K::new(&empty_commitment_suffix()).unwrap(),
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
            coinbase_tx_suffix: B064K::new(&empty_commitment_suffix()).unwrap(),
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
    let first = connection.on_allocate(1, "miner").unwrap();
    let second = connection.on_allocate(2, "miner").unwrap();
    let mut first_payload = response_payload(&first[0]);
    let mut second_payload = response_payload(&second[0]);
    let first = AllocateMiningJobTokenSuccess::from_bytes(&mut first_payload).unwrap();
    let second = AllocateMiningJobTokenSuccess::from_bytes(&mut second_payload).unwrap();
    assert_ne!(
        first.mining_job_token.as_bytes(),
        second.mining_job_token.as_bytes()
    );
    for request_id in 2..MAX_JOB_TOKENS {
        connection.on_allocate(request_id as u32, "miner").unwrap();
    }
    assert_eq!(
        connection.on_allocate(99, "miner").unwrap_err(),
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

/// The donation the terms of a token allocated for `user_identifier` carry.
fn allocated_donation(user_identifier: &str) -> Result<Option<u16>, String> {
    let mut connection = Sv2Connection::new(registry());
    accept_message(
        &mut connection,
        setup(0, Protocol::JobDeclarationProtocol),
        MESSAGE_TYPE_SETUP_CONNECTION,
    );
    let allocated = connection.on_allocate(1, user_identifier)?;
    let mut payload = response_payload(&allocated[0]);
    let allocated = AllocateMiningJobTokenSuccess::from_bytes(&mut payload).unwrap();
    let payout = bitcoin::Address::p2pkh(
        bitcoin::PubkeyHash::from_byte_array([0x22; 20]),
        bitcoin::Network::Regtest,
    );
    let commitment = crate::shares::share_commitment::ShareCommitment::from_mining_job_token(
        allocated.mining_job_token.as_bytes(),
        payout.into(),
        p2poolv2_wallet::share_owner::ShareOwner::CashP2pkh([0x22; 20]),
    )
    .unwrap();
    Ok(commitment.donation)
}

/// A token's terms carry the `donation=` of the identifier that allocated
/// it, the node's own without one; Pickaxe's own key is ignored.
#[test]
fn a_token_carries_the_identifiers_donation() {
    assert_eq!(allocated_donation("miner"), Ok(None));
    assert_eq!(
        allocated_donation("miner,pickaxe-donation=0,donation=0.5"),
        Ok(Some(50))
    );
    assert_eq!(allocated_donation("miner,donation=10"), Ok(Some(1_000)));
    assert!(
        allocated_donation("miner,donation=0.25")
            .unwrap_err()
            .contains("steps of 0.5")
    );
}

/// A Bitcoin registry builds no Bitcoin Cash share chain, so it cannot
/// honour a miner's donation.
#[test]
fn a_bitcoin_token_refuses_a_donation() {
    let mut connection = Sv2Connection::new(Arc::new(Mutex::new(JobRegistry::new(
        PoolTransactionCache::new(),
        None,
    ))));
    accept_message(
        &mut connection,
        setup(0, Protocol::JobDeclarationProtocol),
        MESSAGE_TYPE_SETUP_CONNECTION,
    );
    assert!(
        connection
            .on_allocate(1, "miner,donation=0")
            .unwrap_err()
            .contains("cannot honour")
    );
    assert!(
        connection
            .on_allocate(2, "miner,pickaxe-donation=0")
            .is_ok()
    );
}

/// Declare a Full-Template job whose coinbase suffix is `suffix` with
/// `excess_data`, on a new connection, and return the reply's frame.
fn declare_with_leaves(suffix: &[u8], excess_data: &[u8]) -> Vec<u8> {
    let mut connection = Sv2Connection::new(registry());
    accept_message(
        &mut connection,
        setup(1, Protocol::JobDeclarationProtocol),
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
    let token = AllocateMiningJobTokenSuccess::from_bytes(&mut payload)
        .unwrap()
        .mining_job_token
        .as_bytes()
        .to_vec();
    let mut reply = accept_message(
        &mut connection,
        DeclareMiningJob {
            request_id: 2,
            mining_job_token: B0255::new(&token).unwrap(),
            version: 1,
            coinbase_tx_prefix: B064K::new(&[]).unwrap(),
            coinbase_tx_suffix: B064K::new(suffix).unwrap(),
            wtxid_list: Seq064K::new(vec![]).unwrap(),
            excess_data: B064K::new(excess_data).unwrap(),
        },
        MESSAGE_TYPE_DECLARE_MINING_JOB,
    );
    reply.remove(0)
}

/// The type of a reply frame's message.
fn reply_type(frame: &[u8]) -> u8 {
    frame[2]
}

/// The code and details of a `DeclareMiningJob.Error`.
fn declare_refusal(frame: &[u8]) -> (String, String) {
    assert_eq!(reply_type(frame), MESSAGE_TYPE_DECLARE_MINING_JOB_ERROR);
    let mut payload = response_payload(frame);
    let error = DeclareMiningJobError::from_bytes(&mut payload).unwrap();
    (
        error.error_code.as_utf8_or_hex(),
        String::from_utf8(error.error_details.as_bytes().to_vec()).unwrap(),
    )
}

/// A Full-Template declaration whose output 0 is a token root and whose
/// `excess_data` discloses the leaves that make it goes on to its
/// transactions.
#[test]
fn a_declared_token_root_with_its_leaves_is_accepted() {
    let leaves = crate::test_utils::test_bch_declared_leaves(
        &test_bch_prepared().mining_job_token([1; 32], 0).unwrap(),
    );
    let reply = declare_with_leaves(
        &commitment_suffix(&leaves.commitment),
        &leaves.disclosure().encode(),
    );
    assert_eq!(
        reply_type(&reply),
        MESSAGE_TYPE_PROVIDE_MISSING_TRANSACTIONS
    );
}

/// A token root with empty `excess_data` is refused with
/// `invalid-coinbase-tx`, saying why.
#[test]
fn a_declared_token_root_without_leaves_is_refused() {
    let leaves = crate::test_utils::test_bch_declared_leaves(
        &test_bch_prepared().mining_job_token([1; 32], 0).unwrap(),
    );
    let (code, details) = declare_refusal(&declare_with_leaves(
        &commitment_suffix(&leaves.commitment),
        &[],
    ));
    assert_eq!(code, "invalid-coinbase-tx");
    assert!(details.contains("excess_data"), "{details}");
}

/// Leaves beside the empty commitment are refused: empty `excess_data`
/// means no root.
#[test]
fn declared_leaves_without_a_token_root_are_refused() {
    let leaves = crate::test_utils::test_bch_declared_leaves(
        &test_bch_prepared().mining_job_token([1; 32], 0).unwrap(),
    );
    let (code, details) = declare_refusal(&declare_with_leaves(
        &empty_commitment_suffix(),
        &leaves.disclosure().encode(),
    ));
    assert_eq!(code, "invalid-coinbase-tx");
    assert!(details.contains("without a token commitment"), "{details}");
}

/// Leaves that do not make output 0's root (one left out) are refused.
#[test]
fn declared_leaves_that_miss_the_root_are_refused() {
    let leaves = crate::test_utils::test_bch_declared_leaves(
        &test_bch_prepared().mining_job_token([1; 32], 0).unwrap(),
    );
    let mut disclosure = leaves.disclosure();
    disclosure.leaves.pop();
    let (code, details) = declare_refusal(&declare_with_leaves(
        &commitment_suffix(&leaves.commitment),
        &disclosure.encode(),
    ));
    assert_eq!(code, "invalid-coinbase-tx");
    assert!(details.contains("root"), "{details}");
}

/// `excess_data` must be exactly one canonical disclosure: a trailing byte
/// is refused.
#[test]
fn a_declared_disclosure_with_a_trailing_byte_is_refused() {
    let leaves = crate::test_utils::test_bch_declared_leaves(
        &test_bch_prepared().mining_job_token([1; 32], 0).unwrap(),
    );
    let mut excess_data = leaves.disclosure().encode();
    excess_data.push(0);
    let (code, _) = declare_refusal(&declare_with_leaves(
        &commitment_suffix(&leaves.commitment),
        &excess_data,
    ));
    assert_eq!(code, "invalid-coinbase-tx");
}
