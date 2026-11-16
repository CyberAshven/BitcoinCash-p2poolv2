use super::*;
use crate::accounting::payout::sharechain_pplns::PplnsWindow;
use crate::accounting::payout::{AnchorWeights, ShareChainWeights};
use crate::config::Chain;
use crate::shares::chain::chain_store_handle::{ConfirmedHeaderResult, MockChainStoreHandle};
use crate::store::block_tx_metadata::{BlockMetadata, ChainMembership, Status};
use crate::store::writer::StoreError;
use crate::stratum::sv2::large_templates::{
    LARGE_TEMPLATES_EXTENSION, PROVIDE_MISSING_TRANSACTIONS_POSITIONS,
    ProvideMissingTransactionsPositions,
};
use crate::stratum::sv2::payout_outputs::{
    INVALID_MINING_JOB_TOKEN, REQUEST_PAYOUT_OUTPUTS_ERROR, REQUEST_PAYOUT_OUTPUTS_SUCCESS,
    RequestPayoutOutputsError, RequestPayoutOutputsSuccess, UNKNOWN_ANCHOR,
    VERIFY_PAYOUT_OUTPUTS_ERROR, VERIFY_PAYOUT_OUTPUTS_SUCCESS, VERIFY_RATE_LIMITED,
};
use crate::test_utils::{
    TestShareBlockBuilder, test_bch_prepared, test_bch_prepared_anchored,
    test_bch_prepared_for_template, test_payout_outputs_request,
    test_verify_payout_outputs_request,
};
use binary_sv2::{B016M, U256};
use bitcoin::absolute::LockTime;
use bitcoin::consensus::serialize;
use bitcoin::script::Builder;
use bitcoin::transaction::{Sequence, TxIn, TxOut, Version};
use bitcoin::{Amount, CompactTarget, Network, OutPoint, ScriptBuf, Witness, Work};
use p2poolv2_wallet::payout_address::PayoutAddress;
use p2poolv2_wallet::share_owner::ShareOwner;
use std::str::FromStr;
use std::sync::RwLock;

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
            owner: 0,
            prepared: Some(prepared),
            identity: None,
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
    let first = connection.on_allocate(1, "miner", None).unwrap();
    let second = connection.on_allocate(2, "miner", None).unwrap();
    let mut first_payload = response_payload(&first[0]);
    let mut second_payload = response_payload(&second[0]);
    let first = AllocateMiningJobTokenSuccess::from_bytes(&mut first_payload).unwrap();
    let second = AllocateMiningJobTokenSuccess::from_bytes(&mut second_payload).unwrap();
    assert_ne!(
        first.mining_job_token.as_bytes(),
        second.mining_job_token.as_bytes()
    );
    for request_id in 2..MAX_SESSION_JOB_TOKENS {
        connection
            .on_allocate(request_id as u32, "miner", None)
            .unwrap();
    }
    // One more is answered like any other, and takes the place of the
    // connection's oldest open token.
    connection.on_allocate(99, "miner", None).unwrap();
    let registry = connection.registry.lock().unwrap();
    assert_eq!(
        registry.session_tokens(connection.session_id),
        MAX_SESSION_JOB_TOKENS
    );
    assert!(
        registry
            .open_token(first.mining_job_token.as_bytes())
            .is_none()
    );
    assert!(
        registry
            .open_token(second.mining_job_token.as_bytes())
            .is_some()
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
    let allocated = connection.on_allocate(1, user_identifier, None)?;
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
            .on_allocate(1, "miner,donation=0", None)
            .unwrap_err()
            .contains("cannot honour")
    );
    assert!(
        connection
            .on_allocate(2, "miner,pickaxe-donation=0", None)
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

/// A connection may hold requested payout outputs for eight open tokens, and
/// may ask again for any of them; asking for a ninth closes it, as a ninth
/// pending declaration does.
#[test]
fn a_ninth_outstanding_payout_set_closes_the_connection() {
    let mut connection = Sv2Connection::new(registry());
    accept_message(
        &mut connection,
        setup(0, Protocol::JobDeclarationProtocol),
        MESSAGE_TYPE_SETUP_CONNECTION,
    );
    assert!(connection.grant_payout_outputs());
    let identity = PayoutIdentity {
        payout: bitcoin::Address::p2pkh(
            bitcoin::PubkeyHash::from_byte_array([0x22; 20]),
            bitcoin::Network::Regtest,
        )
        .into(),
        owner: p2poolv2_wallet::share_owner::ShareOwner::CashP2pkh([0x22; 20]),
    };
    let prepared = test_bch_prepared();
    let template = prepared.template();
    let prefix = Builder::new()
        .push_int(i64::from(template.height))
        .into_bytes();
    let mut tokens = Vec::with_capacity(MAX_PAYOUT_SETS + 1);
    for request_id in 0..=MAX_PAYOUT_SETS as u32 {
        let allocated = connection
            .on_allocate(request_id, "miner", Some(identity.clone()))
            .unwrap();
        let mut payload = response_payload(&allocated[0]);
        let allocated = AllocateMiningJobTokenSuccess::from_bytes(&mut payload).unwrap();
        tokens.push(allocated.mining_job_token.as_bytes().to_vec());
    }
    for token in &tokens[..MAX_PAYOUT_SETS] {
        let mut request =
            test_payout_outputs_request(1, token, template, template.coinbasevalue, &prefix);
        let answered = connection
            .handle_payout_outputs(REQUEST_PAYOUT_OUTPUTS, &mut request)
            .unwrap();
        assert_eq!(answered[0][2], REQUEST_PAYOUT_OUTPUTS_SUCCESS);
    }
    let mut again =
        test_payout_outputs_request(2, &tokens[0], template, template.coinbasevalue, &prefix);
    let answered = connection
        .handle_payout_outputs(REQUEST_PAYOUT_OUTPUTS, &mut again)
        .unwrap();
    assert_eq!(answered[0][2], REQUEST_PAYOUT_OUTPUTS_SUCCESS);

    let mut ninth = test_payout_outputs_request(
        3,
        &tokens[MAX_PAYOUT_SETS],
        template,
        template.coinbasevalue,
        &prefix,
    );

    assert_eq!(
        connection
            .handle_payout_outputs(REQUEST_PAYOUT_OUTPUTS, &mut ninth)
            .unwrap_err(),
        "payout output request limit reached"
    );
}

/// A token is single use: once a mining connection has taken it, a request
/// for its outputs gets `invalid-mining-job-token`.
#[test]
fn a_payout_request_for_a_used_token_is_refused() {
    let mut connection = Sv2Connection::new(registry());
    accept_message(
        &mut connection,
        setup(0, Protocol::JobDeclarationProtocol),
        MESSAGE_TYPE_SETUP_CONNECTION,
    );
    assert!(connection.grant_payout_outputs());
    let identity = PayoutIdentity {
        payout: bitcoin::Address::p2pkh(
            bitcoin::PubkeyHash::from_byte_array([0x22; 20]),
            bitcoin::Network::Regtest,
        )
        .into(),
        owner: p2poolv2_wallet::share_owner::ShareOwner::CashP2pkh([0x22; 20]),
    };
    let allocated = connection.on_allocate(1, "miner", Some(identity)).unwrap();
    let mut payload = response_payload(&allocated[0]);
    let allocated = AllocateMiningJobTokenSuccess::from_bytes(&mut payload).unwrap();
    let token = allocated.mining_job_token.as_bytes().to_vec();
    assert!(connection.take_declared_job(&token).is_ok());
    let prepared = test_bch_prepared();
    let template = prepared.template();
    let prefix = Builder::new()
        .push_int(i64::from(template.height))
        .into_bytes();
    let mut request =
        test_payout_outputs_request(2, &token, template, template.coinbasevalue, &prefix);

    let answered = connection
        .handle_payout_outputs(REQUEST_PAYOUT_OUTPUTS, &mut request)
        .unwrap();

    assert_eq!(answered[0][2], REQUEST_PAYOUT_OUTPUTS_ERROR);
    let mut payload = response_payload(&answered[0]);
    let error = RequestPayoutOutputsError::from_bytes(&mut payload).unwrap();
    assert_eq!(error.error_code.as_utf8_or_hex(), INVALID_MINING_JOB_TOKEN);
}

/// A connection that negotiated the requested payout outputs reserves room
/// for every output its answers carry: output 0, the padding and the
/// commitment beside the payout scripts. Without the extension the
/// allocation lists the payout scripts alone, as before.
#[test]
fn a_granted_allocation_reserves_room_for_the_answered_outputs() {
    let identity = PayoutIdentity {
        payout: bitcoin::Address::p2pkh(
            bitcoin::PubkeyHash::from_byte_array([0x22; 20]),
            bitcoin::Network::Regtest,
        )
        .into(),
        owner: p2poolv2_wallet::share_owner::ShareOwner::CashP2pkh([0x22; 20]),
    };
    let allocate = |connection: &mut Sv2Connection| {
        accept_message(
            connection,
            setup(0, Protocol::JobDeclarationProtocol),
            MESSAGE_TYPE_SETUP_CONNECTION,
        );
        let allocated = connection
            .on_allocate(1, "miner", Some(identity.clone()))
            .unwrap();
        let mut payload = response_payload(&allocated[0]);
        let allocated = AllocateMiningJobTokenSuccess::from_bytes(&mut payload).unwrap();
        let outputs: Vec<TxOut> = deserialize(allocated.coinbase_outputs.as_bytes()).unwrap();
        (allocated.mining_job_token.as_bytes().to_vec(), outputs)
    };
    let (_, plain) = allocate(&mut Sv2Connection::new(registry()));
    let mut connection = Sv2Connection::new(registry());
    accept_message(
        &mut connection,
        setup(0, Protocol::JobDeclarationProtocol),
        MESSAGE_TYPE_SETUP_CONNECTION,
    );
    assert!(connection.grant_payout_outputs());
    let (token, reserved) = allocate(&mut connection);
    assert_eq!(reserved.len(), plain.len() + 3);
    assert_eq!(&reserved[1..=plain.len()], plain.as_slice());

    let prepared = test_bch_prepared();
    let template = prepared.template();
    let prefix = Builder::new()
        .push_int(i64::from(template.height))
        .into_bytes();
    let mut request =
        test_payout_outputs_request(1, &token, template, template.coinbasevalue, &prefix);
    let answered = connection
        .handle_payout_outputs(REQUEST_PAYOUT_OUTPUTS, &mut request)
        .unwrap();
    let mut payload = response_payload(&answered[0]);
    let answered = RequestPayoutOutputsSuccess::from_bytes(&mut payload).unwrap();
    assert!(answered.coinbase_tx_outputs.as_bytes().len() <= serialize(&reserved).len());
}

/// A new BCH block under the same share sends `PayoutOutputsStale` once:
/// tokens allocated on the earlier template would only be answered
/// `stale-payout-outputs`.
#[test]
fn a_new_bch_block_on_the_same_share_pushes_payout_outputs_stale() {
    let mut connection = Sv2Connection::new(registry());
    accept_message(
        &mut connection,
        setup(0, Protocol::JobDeclarationProtocol),
        MESSAGE_TYPE_SETUP_CONNECTION,
    );
    assert!(connection.grant_payout_outputs());
    let current = test_bch_prepared();
    assert!(connection.on_template(&current).unwrap().is_empty());

    let mut template = current.template().as_ref().clone();
    template.previousblockhash = BlockHash::from_byte_array([0x33; 32]).to_string();
    let next = test_bch_prepared_for_template(template);
    assert_eq!(next.prev_share_blockhash(), current.prev_share_blockhash());

    assert_eq!(
        connection.on_template(&next).unwrap(),
        vec![stale_frame().unwrap()]
    );
    assert!(connection.on_template(&next).unwrap().is_empty());
}

fn requesting_identity() -> PayoutIdentity {
    PayoutIdentity {
        payout: bitcoin::Address::p2pkh(
            bitcoin::PubkeyHash::from_byte_array([0x22; 20]),
            Network::Regtest,
        )
        .into(),
        owner: ShareOwner::CashP2pkh([0x22; 20]),
    }
}

/// One confirmed share, paid to a regtest P2PKH, on its own window and store.
fn bch_window_holding(header: &crate::shares::share_block::ShareHeader) -> ShareChainWeights {
    let tip = header.block_hash();
    let stored = vec![ConfirmedHeaderResult {
        height: 0,
        blockhash: tip,
        header: header.clone(),
    }];
    let mut store = MockChainStoreHandle::default();
    store.expect_get_chain_tip().returning(move || Ok(tip));
    store.expect_get_tip_height().returning(|| Ok(Some(0)));
    store.expect_get_block_metadata().returning(|_| {
        Ok(BlockMetadata {
            expected_height: Some(0),
            chain_work: Work::from_le_bytes([0u8; 32]),
            status: Status::BlockValid,
            chain: ChainMembership::Confirmed,
        })
    });
    store
        .expect_get_confirmed_headers_in_range()
        .returning(move |_, _| Ok(stored.clone()));
    let window = Arc::new(RwLock::new(
        PplnsWindow::new(Network::Regtest).for_chain(Chain::BitcoinCash),
    ));
    window
        .write()
        .expect("PPLNS window lock")
        .update(&store)
        .expect("the share loads");
    ShareChainWeights::new(window, store)
}

/// Node A issues a token on a snapshot of its window. Node B, with its own
/// store of the same share, answers `VerifyPayoutOutputs` for that token.
fn issuer_and_verifier_frames() -> (Vec<u8>, Vec<u8>) {
    let payout = PayoutAddress::from(bitcoin::Address::p2pkh(
        bitcoin::PubkeyHash::from_byte_array([0x44; 20]),
        Network::Regtest,
    ));
    let header = TestShareBlockBuilder::new()
        .prev_share_blockhash(BlockHash::all_zeros().to_string())
        .payout_address(payout)
        .share_owner(ShareOwner::CashP2pkh([0x44; 20]))
        .build()
        .header;
    let anchor = header.block_hash();
    let issuer_window = bch_window_holding(&header);
    let verifier_window = bch_window_holding(&header);
    let fixture = test_bch_prepared();
    let template = fixture.template();
    let nbits = CompactTarget::from_unprefixed_hex(&template.bits)
        .unwrap()
        .to_consensus();
    let prev_hash = BlockHash::from_str(&template.previousblockhash).unwrap();
    let weights = issuer_window
        .weights_at(anchor, nbits, prev_hash)
        .expect("the issuer's window pays the share");
    let prepared = test_bch_prepared_anchored(anchor, weights);
    let template = &prepared.template().clone();
    let prefix = Builder::new()
        .push_int(i64::from(template.height))
        .into_bytes();
    let identity = requesting_identity();
    let cache = PoolTransactionCache::new();
    cache.set_prepared(prepared);
    let mut issuer = Sv2Connection::new(Arc::new(Mutex::new(JobRegistry::new(
        cache,
        Some(MergeMiningCommitment::empty()),
    ))));
    accept_message(
        &mut issuer,
        setup(0, Protocol::JobDeclarationProtocol),
        MESSAGE_TYPE_SETUP_CONNECTION,
    );
    assert!(issuer.grant_payout_outputs());
    let allocated = issuer
        .on_allocate(1, "miner", Some(identity.clone()))
        .unwrap();
    let mut payload = response_payload(&allocated[0]);
    let allocated = AllocateMiningJobTokenSuccess::from_bytes(&mut payload).unwrap();
    let token = allocated.mining_job_token.as_bytes().to_vec();
    let mut request =
        test_payout_outputs_request(11, &token, template, template.coinbasevalue, &prefix);
    let issued = issuer
        .handle_payout_outputs(REQUEST_PAYOUT_OUTPUTS, &mut request)
        .unwrap();
    let mut verify = test_verify_payout_outputs_request(
        11,
        &token,
        "miner",
        template,
        template.coinbasevalue,
        &prefix,
    );
    let verified = Sv2Connection::new(registry())
        .handle_verify_payout_outputs(
            &mut verify,
            |_| Some(identity.clone()),
            Some(&verifier_window),
        )
        .unwrap();
    (issued[0].clone(), verified[0].clone())
}

/// Two nodes, each with its own store of the same share chain: the issuer's
/// `RequestPayoutOutputs.Success` payload is the verifier's
/// `VerifyPayoutOutputs.Success` payload.
#[test]
fn a_second_node_recomputes_the_issuing_nodes_outputs() {
    let (issued, verified) = issuer_and_verifier_frames();

    assert_eq!(issued[2], REQUEST_PAYOUT_OUTPUTS_SUCCESS);
    assert_eq!(verified[2], VERIFY_PAYOUT_OUTPUTS_SUCCESS);
    assert_eq!(response_payload(&issued), response_payload(&verified));
}

/// The comparison a client makes: the two success payloads, byte for byte.
/// Changing one output of the issuer's set makes them differ.
#[test]
fn a_changed_output_does_not_match_the_second_node() {
    let (issued, verified) = issuer_and_verifier_frames();
    assert_eq!(issued[2], REQUEST_PAYOUT_OUTPUTS_SUCCESS);
    assert_eq!(verified[2], VERIFY_PAYOUT_OUTPUTS_SUCCESS);
    let issuer_outputs = response_payload(&issued);
    let verifier_outputs = response_payload(&verified);
    assert_eq!(issuer_outputs, verifier_outputs);

    let mut changed = issuer_outputs.clone();
    let last = changed.len() - 1;
    changed[last] ^= 0xff;
    assert_ne!(changed, verifier_outputs);
}

/// A verifier that does not hold the token's anchor share answers
/// `unknown-anchor`, and the connection stays up.
#[test]
fn a_node_without_the_anchor_answers_unknown_anchor() {
    let prepared = test_bch_prepared();
    let template = prepared.template();
    let token = prepared
        .mining_job_token([0x11; 32], template.curtime)
        .unwrap();
    let prefix = Builder::new()
        .push_int(i64::from(template.height))
        .into_bytes();
    let mut request = test_verify_payout_outputs_request(
        3,
        &token,
        "miner",
        template,
        template.coinbasevalue,
        &prefix,
    );
    let mut store = MockChainStoreHandle::default();
    store
        .expect_get_block_metadata()
        .returning(|_| Err(StoreError::NotFound("not found".into())));
    let window = Arc::new(RwLock::new(
        PplnsWindow::new(Network::Regtest).for_chain(Chain::BitcoinCash),
    ));
    let weights = ShareChainWeights::new(window, store);
    let identity = requesting_identity();

    let answered = Sv2Connection::new(registry())
        .handle_verify_payout_outputs(&mut request, |_| Some(identity.clone()), Some(&weights))
        .unwrap();

    assert_eq!(answered[0][2], VERIFY_PAYOUT_OUTPUTS_ERROR);
    let mut payload = response_payload(&answered[0]);
    let error = RequestPayoutOutputsError::from_bytes(&mut payload).unwrap();
    assert_eq!(error.error_code.as_utf8_or_hex(), UNKNOWN_ANCHOR);
}

/// A token that does not decode is `invalid-mining-job-token`.
#[test]
fn a_token_that_does_not_decode_is_invalid_mining_job_token() {
    let prepared = test_bch_prepared();
    let template = prepared.template();
    let mut request = test_verify_payout_outputs_request(
        4,
        &[0x00; 33],
        "miner",
        template,
        template.coinbasevalue,
        &[],
    );
    let identity = requesting_identity();

    let answered = Sv2Connection::new(registry())
        .handle_verify_payout_outputs(&mut request, |_| Some(identity.clone()), None)
        .unwrap();

    assert_eq!(answered[0][2], VERIFY_PAYOUT_OUTPUTS_ERROR);
    let mut payload = response_payload(&answered[0]);
    let error = RequestPayoutOutputsError::from_bytes(&mut payload).unwrap();
    assert_eq!(error.error_code.as_utf8_or_hex(), INVALID_MINING_JOB_TOKEN);
}

/// Four verifies in a second are answered. The next is `verify-rate-limited`,
/// and the connection stays up.
#[test]
fn a_verify_past_the_connection_budget_is_rate_limited() {
    let prepared = test_bch_prepared();
    let template = prepared.template();
    let mut connection = Sv2Connection::new(registry());
    let verify = |connection: &mut Sv2Connection, request_id: u32| {
        let mut request = test_verify_payout_outputs_request(
            request_id,
            &[0x00; 33],
            "miner",
            template,
            template.coinbasevalue,
            &[],
        );
        connection
            .handle_verify_payout_outputs(&mut request, |_| None, None)
            .unwrap()
    };
    for request_id in 1..=MAX_VERIFY_PAYOUTS_PER_SECOND {
        let answered = verify(&mut connection, request_id);
        assert_eq!(answered[0][2], VERIFY_PAYOUT_OUTPUTS_ERROR);
        let mut payload = response_payload(&answered[0]);
        let error = RequestPayoutOutputsError::from_bytes(&mut payload).unwrap();
        assert_eq!(error.error_code.as_utf8_or_hex(), INVALID_MINING_JOB_TOKEN);
    }

    let answered = verify(&mut connection, MAX_VERIFY_PAYOUTS_PER_SECOND + 1);

    assert_eq!(answered[0][2], VERIFY_PAYOUT_OUTPUTS_ERROR);
    let mut payload = response_payload(&answered[0]);
    let error = RequestPayoutOutputsError::from_bytes(&mut payload).unwrap();
    assert_eq!(error.error_code.as_utf8_or_hex(), VERIFY_RATE_LIMITED);
}

/// The extranonce a pushed solution's coinbase carries in these tests: the
/// whole scriptSig of the declared coinbase.
static PUSHED_EXTRANONCE: [u8; 8] = [7; 8];

/// A declared coinbase's bytes before the extranonce: version, one input
/// spending nothing, and the scriptSig's length.
fn pushed_coinbase_prefix() -> Vec<u8> {
    let mut prefix = 2u32.to_le_bytes().to_vec();
    prefix.push(1);
    prefix.extend([0; 32]);
    prefix.extend(u32::MAX.to_le_bytes());
    prefix.push(PUSHED_EXTRANONCE.len() as u8);
    prefix
}

/// A Job Declaration connection on `shared` that declared `transactions`
/// in Full-Template mode, with a coinbase of `pushed_coinbase_prefix`, an
/// extranonce and the empty-commitment suffix.
fn full_template_declarator(
    shared: &Arc<Mutex<JobRegistry>>,
    transactions: &[Transaction],
) -> Sv2Connection {
    let mut connection = Sv2Connection::new(Arc::clone(shared));
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
    let wtxid_bytes = transactions
        .iter()
        .map(|transaction| transaction.compute_wtxid().to_byte_array())
        .collect::<Vec<_>>();
    let wtxids = wtxid_bytes
        .iter()
        .map(|wtxid| U256::new(wtxid.as_slice()).unwrap())
        .collect::<Vec<_>>();
    accept_message(
        &mut connection,
        DeclareMiningJob {
            request_id: 2,
            mining_job_token: B0255::new(&token).unwrap(),
            version: 1,
            coinbase_tx_prefix: B064K::new(&pushed_coinbase_prefix()).unwrap(),
            coinbase_tx_suffix: B064K::new(&empty_commitment_suffix()).unwrap(),
            wtxid_list: Seq064K::new(wtxids).unwrap(),
            excess_data: B064K::new(&[]).unwrap(),
        },
        MESSAGE_TYPE_DECLARE_MINING_JOB,
    );
    let serialized = transactions.iter().map(serialize).collect::<Vec<_>>();
    let revealed = serialized
        .iter()
        .map(|bytes| B016M::new(bytes.as_slice()).unwrap())
        .collect::<Vec<_>>();
    let success = accept_message(
        &mut connection,
        ProvideMissingTransactionsSuccess {
            request_id: 2,
            transaction_list: Seq064K::new(revealed).unwrap(),
        },
        MESSAGE_TYPE_PROVIDE_MISSING_TRANSACTIONS_SUCCESS,
    );
    assert_eq!(
        reply_type(&success[0]),
        MESSAGE_TYPE_DECLARE_MINING_JOB_SUCCESS
    );
    connection
}

/// The block a declarator of `transactions` finds with `nonce`, built here
/// without the session's merkle branch: the coinbase from the declared
/// prefix, the extranonce and the suffix, then the transactions in CTOR.
fn expected_pushed_block(transactions: &[Transaction], nonce: u32) -> Block {
    let mut raw = pushed_coinbase_prefix();
    raw.extend(PUSHED_EXTRANONCE);
    raw.extend(empty_commitment_suffix());
    let coinbase: Transaction = deserialize(&raw).unwrap();
    let mut sorted = transactions.to_vec();
    sort_ctor(&mut sorted);
    let template = test_bch_prepared().template().clone();
    let mut block = Block {
        header: Header {
            version: bitcoin::block::Version::from_consensus(0x2000_0000),
            prev_blockhash: template.previousblockhash.parse().unwrap(),
            merkle_root: TxMerkleNode::all_zeros(),
            time: template.curtime,
            bits: CompactTarget::from_unprefixed_hex(&template.bits).unwrap(),
            nonce,
        },
        txdata: std::iter::once(coinbase).chain(sorted).collect(),
    };
    block.header.merkle_root = block.compute_merkle_root().unwrap();
    block
}

/// The first nonce from 0 whose header meets the template's target when
/// `meets`, or misses it when not.
fn pushed_nonce(transactions: &[Transaction], meets: bool) -> u32 {
    (0..)
        .find(|nonce| {
            let header = expected_pushed_block(transactions, *nonce).header;
            Target::from_compact(header.bits).is_met_by(header.block_hash()) == meets
        })
        .unwrap()
}

/// A `PushSolution` for `header`, whose previous block hash is `prev_hash`,
/// with `PUSHED_EXTRANONCE`.
fn push_solution_message<'a>(header: &Header, prev_hash: &'a [u8; 32]) -> PushSolution<'a> {
    PushSolution {
        extranonce: binary_sv2::B032::new(&PUSHED_EXTRANONCE).unwrap(),
        prev_hash: U256::from(prev_hash),
        nonce: header.nonce,
        ntime: header.time,
        nbits: header.bits.to_consensus(),
        version: header.version.to_consensus() as u32,
    }
}

/// A `PushSolution` payload for `header` with `PUSHED_EXTRANONCE`.
fn push_solution_payload(header: &Header) -> Vec<u8> {
    let prev_hash = header.prev_blockhash.to_byte_array();
    let frame = encode_message(
        push_solution_message(header, &prev_hash),
        MESSAGE_TYPE_PUSH_SOLUTION,
    )
    .unwrap();
    response_payload(&frame)
}

#[test]
fn a_push_solution_rebuilds_the_declared_block() {
    let shared = registry();
    let transactions = vec![transaction_with_lock(1), transaction_with_lock(2)];
    let mut connection = full_template_declarator(&shared, &transactions);
    let expected = expected_pushed_block(&transactions, pushed_nonce(&transactions, true));

    let mut payload = push_solution_payload(&expected.header);
    let block = connection.push_solution(&mut payload).unwrap().unwrap();

    assert_eq!(block, expected);
    assert!(block.check_merkle_root());
}

#[test]
fn a_push_solution_neither_answers_nor_closes_the_connection() {
    let shared = registry();
    let transactions = vec![transaction_with_lock(1)];
    let mut connection = full_template_declarator(&shared, &transactions);
    let solved = expected_pushed_block(&transactions, pushed_nonce(&transactions, true));

    let prev_hash = solved.header.prev_blockhash.to_byte_array();
    let frames = accept_message(
        &mut connection,
        push_solution_message(&solved.header, &prev_hash),
        MESSAGE_TYPE_PUSH_SOLUTION,
    );

    assert!(frames.is_empty());
    // The connection still takes Job Declaration messages.
    let allocate = accept_message(
        &mut connection,
        AllocateMiningJobToken {
            user_identifier: "miner".try_into().unwrap(),
            request_id: 3,
        },
        MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN,
    );
    assert_eq!(
        reply_type(&allocate[0]),
        MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN_SUCCESS
    );
}

#[test]
fn a_push_solution_short_of_the_target_or_on_another_block_gives_no_block() {
    let shared = registry();
    let transactions = vec![transaction_with_lock(1)];
    let mut connection = full_template_declarator(&shared, &transactions);
    let short = expected_pushed_block(&transactions, pushed_nonce(&transactions, false));
    let mut elsewhere = expected_pushed_block(&transactions, pushed_nonce(&transactions, true));
    elsewhere.header.prev_blockhash = BlockHash::from_byte_array([9; 32]);

    for header in [short.header, elsewhere.header] {
        let mut payload = push_solution_payload(&header);
        assert_eq!(connection.push_solution(&mut payload).unwrap(), None);
    }
}

#[test]
fn a_coinbase_only_push_solution_gives_no_block() {
    let mut connection = Sv2Connection::new(registry());
    accept_message(
        &mut connection,
        setup(0, Protocol::JobDeclarationProtocol),
        MESSAGE_TYPE_SETUP_CONNECTION,
    );
    let transactions = vec![transaction_with_lock(1)];
    let solved = expected_pushed_block(&transactions, pushed_nonce(&transactions, true));

    let mut payload = push_solution_payload(&solved.header);

    assert_eq!(connection.push_solution(&mut payload).unwrap(), None);
}

#[test]
fn a_push_solution_off_job_declaration_closes_the_connection() {
    let mut connection = Sv2Connection::new(registry());
    accept_message(
        &mut connection,
        setup(0, Protocol::MiningProtocol),
        MESSAGE_TYPE_SETUP_CONNECTION,
    );
    let transactions = vec![transaction_with_lock(1)];
    let solved = expected_pushed_block(&transactions, pushed_nonce(&transactions, true));
    let prev_hash = solved.header.prev_blockhash.to_byte_array();
    let frame = encode_message(
        push_solution_message(&solved.header, &prev_hash),
        MESSAGE_TYPE_PUSH_SOLUTION,
    )
    .unwrap();

    assert!(connection.accept(&frame).is_err());
}

#[test]
fn kept_declarations_hold_their_bytes_until_the_connection_closes() {
    let shared = registry();
    let transactions = vec![transaction_with_lock(1), transaction_with_lock(2)];
    let declared_bytes = transactions
        .iter()
        .map(Transaction::total_size)
        .sum::<usize>()
        + pushed_coinbase_prefix().len()
        + empty_commitment_suffix().len();
    let connection = full_template_declarator(&shared, &transactions);
    let budget = Arc::clone(&shared.lock().unwrap().solvable_bytes);

    assert_eq!(budget.load(Ordering::SeqCst), declared_bytes);
    drop(connection);
    assert_eq!(budget.load(Ordering::SeqCst), 0);
}

/// A registry that takes Coinbase-Only declarations from `networks` only.
fn registry_taking_coinbase_only_from(networks: &[&str]) -> Arc<Mutex<JobRegistry>> {
    let cache = PoolTransactionCache::new();
    cache.set_prepared(test_bch_prepared());
    Arc::new(Mutex::new(
        JobRegistry::new(cache, Some(MergeMiningCommitment::empty())).with_coinbase_only_from(
            networks
                .iter()
                .map(|network| network.parse().unwrap())
                .collect(),
        ),
    ))
}

/// Set up Job Declaration from `peer` with `flags`, and return the reply.
fn job_declaration_setup_from(
    registry: Arc<Mutex<JobRegistry>>,
    peer: &str,
    flags: u32,
) -> (Sv2Connection, Vec<u8>) {
    let mut connection = Sv2Connection::new(registry).with_peer(peer.parse().unwrap());
    let reply = accept_message(
        &mut connection,
        setup(flags, Protocol::JobDeclarationProtocol),
        MESSAGE_TYPE_SETUP_CONNECTION,
    );
    (connection, reply[0].clone())
}

#[test]
fn coinbase_only_is_refused_from_a_client_off_the_allowlist() {
    let registry = registry_taking_coinbase_only_from(&["127.0.0.0/8", "::1/128"]);

    let (connection, reply) = job_declaration_setup_from(registry, "203.0.113.7", 0);

    assert_eq!(reply_type(&reply), MESSAGE_TYPE_SETUP_CONNECTION_ERROR);
    let mut payload = response_payload(&reply);
    let refused = SetupConnectionError::from_bytes(&mut payload).unwrap();
    assert_eq!(
        refused.error_code.as_utf8_or_hex(),
        "unsupported-feature-flags"
    );
    assert_eq!(refused.flags, DECLARE_TX_DATA_FLAG);
    assert!(!connection.is_set_up());
}

#[test]
fn coinbase_only_is_taken_from_the_allowlist() {
    for peer in ["127.0.0.1", "::1", "::ffff:127.0.0.1", "100.64.0.9"] {
        let registry =
            registry_taking_coinbase_only_from(&["127.0.0.0/8", "::1/128", "100.64.0.9/32"]);

        let (connection, reply) = job_declaration_setup_from(registry, peer, 0);

        assert_eq!(
            reply_type(&reply),
            MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS,
            "{peer}"
        );
        assert!(connection.is_set_up());
    }
}

#[test]
fn full_template_is_taken_from_every_client() {
    let registry = registry_taking_coinbase_only_from(&[]);

    let (connection, reply) = job_declaration_setup_from(registry, "203.0.113.7", 1);

    assert_eq!(reply_type(&reply), MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS);
    assert!(connection.is_set_up());
}

#[test]
fn an_empty_allowlist_refuses_coinbase_only_even_from_loopback() {
    let registry = registry_taking_coinbase_only_from(&[]);

    let (_, reply) = job_declaration_setup_from(registry, "127.0.0.1", 0);

    assert_eq!(reply_type(&reply), MESSAGE_TYPE_SETUP_CONNECTION_ERROR);
}

#[test]
fn a_refused_coinbase_only_client_can_set_up_again_as_full_template() {
    let registry = registry_taking_coinbase_only_from(&["127.0.0.0/8"]);
    let (mut connection, reply) = job_declaration_setup_from(registry, "203.0.113.7", 0);
    assert_eq!(reply_type(&reply), MESSAGE_TYPE_SETUP_CONNECTION_ERROR);

    let again = accept_message(
        &mut connection,
        setup(1, Protocol::JobDeclarationProtocol),
        MESSAGE_TYPE_SETUP_CONNECTION,
    );

    assert_eq!(reply_type(&again[0]), MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS);
}

/// A registry whose published template builds a block of `size_limit` bytes.
fn registry_with_size_limit(size_limit: u64) -> Arc<Mutex<JobRegistry>> {
    let mut template = test_bch_prepared().template().as_ref().clone();
    template.sizelimit = size_limit;
    let cache = PoolTransactionCache::new();
    cache.set_prepared(test_bch_prepared_for_template(template));
    Arc::new(Mutex::new(JobRegistry::new(
        cache,
        Some(MergeMiningCommitment::empty()),
    )))
}

/// A Full-Template Job Declaration connection on `shared` with a token
/// allocated, and large templates negotiated when `large`.
fn large_template_declarator(
    shared: &Arc<Mutex<JobRegistry>>,
    large: bool,
) -> (Sv2Connection, Vec<u8>) {
    let mut connection = Sv2Connection::new(Arc::clone(shared));
    accept_message(
        &mut connection,
        setup(1, Protocol::JobDeclarationProtocol),
        MESSAGE_TYPE_SETUP_CONNECTION,
    );
    if large {
        assert!(connection.grant_large_templates());
    }
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
    (connection, token)
}

fn wtxid_bytes(transactions: &[Transaction]) -> Vec<[u8; 32]> {
    transactions
        .iter()
        .map(|transaction| transaction.compute_wtxid().to_byte_array())
        .collect()
}

/// Send `ids` as one `DeclareMiningJob.Transactions` page of declaration 2.
fn send_page(connection: &mut Sv2Connection, ids: &[[u8; 32]]) -> Result<Vec<Vec<u8>>, String> {
    let txids = ids
        .iter()
        .map(|id| U256::new(id.as_slice()).unwrap())
        .collect::<Vec<_>>();
    let mut payload = binary_sv2::to_bytes(DeclareMiningJobTransactions {
        request_id: 2,
        txid_list: Seq064K::new(txids).unwrap(),
    })
    .unwrap();
    connection.handle_large_templates(DECLARE_MINING_JOB_TRANSACTIONS, &mut payload)
}

/// Declare, as request 2 on `token`, a job whose core list is `ids`.
fn declare(
    connection: &mut Sv2Connection,
    token: &[u8],
    ids: &[[u8; 32]],
) -> Result<Vec<Vec<u8>>, String> {
    let wtxids = ids
        .iter()
        .map(|id| U256::new(id.as_slice()).unwrap())
        .collect::<Vec<_>>();
    let frame = encode_message(
        DeclareMiningJob {
            request_id: 2,
            mining_job_token: B0255::new(token).unwrap(),
            version: 1,
            coinbase_tx_prefix: B064K::new(&pushed_coinbase_prefix()).unwrap(),
            coinbase_tx_suffix: B064K::new(&empty_commitment_suffix()).unwrap(),
            wtxid_list: Seq064K::new(wtxids).unwrap(),
            excess_data: B064K::new(&[]).unwrap(),
        },
        MESSAGE_TYPE_DECLARE_MINING_JOB,
    )
    .unwrap();
    connection.accept(&frame)
}

/// Reveal `transactions` for declaration 2.
fn reveal(
    connection: &mut Sv2Connection,
    transactions: &[Transaction],
) -> Result<Vec<Vec<u8>>, String> {
    let serialized = transactions.iter().map(serialize).collect::<Vec<_>>();
    let revealed = serialized
        .iter()
        .map(|bytes| B016M::new(bytes.as_slice()).unwrap())
        .collect::<Vec<_>>();
    let frame = encode_message(
        ProvideMissingTransactionsSuccess {
            request_id: 2,
            transaction_list: Seq064K::new(revealed).unwrap(),
        },
        MESSAGE_TYPE_PROVIDE_MISSING_TRANSACTIONS_SUCCESS,
    )
    .unwrap();
    connection.accept(&frame)
}

/// The positions a missing-transactions request in `frame` asks for, by
/// either form: the core `ProvideMissingTransactions` or `0x4C54`'s
/// `ProvideMissingTransactions.Positions`.
fn requested_positions(frame: &[u8]) -> (bool, Vec<u32>) {
    let extension = u16::from_le_bytes([frame[0], frame[1]]) & 0x7fff;
    let mut payload = frame[6..].to_vec();
    if extension == LARGE_TEMPLATES_EXTENSION {
        assert_eq!(frame[2], PROVIDE_MISSING_TRANSACTIONS_POSITIONS);
        let request = ProvideMissingTransactionsPositions::from_bytes(&mut payload).unwrap();
        assert_eq!(request.request_id, 2);
        (true, request.unknown_tx_position_list.into_inner())
    } else {
        assert_eq!(frame[2], MESSAGE_TYPE_PROVIDE_MISSING_TRANSACTIONS);
        let request = ProvideMissingTransactions::from_bytes(&mut payload).unwrap();
        assert_eq!(request.request_id, 2);
        (
            false,
            request
                .unknown_tx_position_list
                .into_inner()
                .into_iter()
                .map(u32::from)
                .collect(),
        )
    }
}

#[test]
fn large_templates_go_only_to_full_template_declarators() {
    for (flags, protocol, granted) in [
        (1, Protocol::JobDeclarationProtocol, true),
        (0, Protocol::JobDeclarationProtocol, false),
        (0, Protocol::MiningProtocol, false),
    ] {
        let mut connection = Sv2Connection::new(registry());
        accept_message(
            &mut connection,
            setup(flags, protocol),
            MESSAGE_TYPE_SETUP_CONNECTION,
        );
        assert_eq!(connection.grant_large_templates(), granted);
        assert_eq!(connection.has_large_templates(), granted);
    }
}

/// A declaration of more than 65,535 transactions: a page, then the core
/// list. The node asks for the next 65,535 from where the replies stopped,
/// one request at a time: with the core request while every position fits a
/// u16, and with `0x02` past that. It takes partial replies and asks again.
#[test]
fn a_large_template_is_asked_for_in_batches_and_takes_partial_replies() {
    let shared = registry();
    let (mut connection, token) = large_template_declarator(&shared, true);
    let count = MAX_IDS_PER_MESSAGE + 2_000;
    let transactions = (0..count as u32)
        .map(transaction_with_lock)
        .collect::<Vec<_>>();
    let ids = wtxid_bytes(&transactions);

    assert!(
        send_page(&mut connection, &ids[..MAX_IDS_PER_MESSAGE])
            .unwrap()
            .is_empty()
    );
    let asked = declare(&mut connection, &token, &ids[MAX_IDS_PER_MESSAGE..]).unwrap();
    let (extension, positions) = requested_positions(&asked[0]);
    assert!(!extension);
    assert_eq!(
        positions,
        (0..MAX_IDS_PER_MESSAGE as u32).collect::<Vec<_>>()
    );

    // A partial reply: the next batch, from 40,000, crosses u16 positions.
    let asked = reveal(&mut connection, &transactions[..40_000]).unwrap();
    let (extension, positions) = requested_positions(&asked[0]);
    assert!(extension);
    assert_eq!(positions, (40_000..count as u32).collect::<Vec<_>>());

    // Another partial reply: the node asks for what is left.
    let asked = reveal(&mut connection, &transactions[40_000..60_000]).unwrap();
    let (extension, positions) = requested_positions(&asked[0]);
    assert!(extension);
    assert_eq!(positions, (60_000..count as u32).collect::<Vec<_>>());

    let done = reveal(&mut connection, &transactions[60_000..]).unwrap();
    assert_eq!(
        reply_type(&done[0]),
        MESSAGE_TYPE_DECLARE_MINING_JOB_SUCCESS
    );
    let mut payload = response_payload(&done[0]);
    let accepted = DeclareMiningJobSuccess::from_bytes(&mut payload).unwrap();
    let stored = shared
        .lock()
        .unwrap()
        .transactions_for_token(accepted.new_mining_job_token.as_bytes())
        .unwrap();
    let mut expected = transactions;
    sort_ctor(&mut expected);
    assert_eq!(stored, expected);
}

#[test]
fn a_partial_reply_without_large_templates_closes_the_connection() {
    let shared = registry();
    let (mut connection, token) = large_template_declarator(&shared, false);
    let transactions = (0..3).map(transaction_with_lock).collect::<Vec<_>>();
    declare(&mut connection, &token, &wtxid_bytes(&transactions)).unwrap();

    assert!(reveal(&mut connection, &transactions[..2]).is_err());
}

#[test]
fn an_empty_reply_to_a_request_closes_the_connection() {
    let shared = registry();
    let (mut connection, token) = large_template_declarator(&shared, true);
    let transactions = (0..3).map(transaction_with_lock).collect::<Vec<_>>();
    declare(&mut connection, &token, &wtxid_bytes(&transactions)).unwrap();

    assert!(reveal(&mut connection, &[]).is_err());
}

/// A block of 650 bytes holds at most ten 65-byte transactions.
#[test]
fn pages_past_the_size_limit_close_the_connection() {
    let shared = registry_with_size_limit(650);
    let (mut connection, _token) = large_template_declarator(&shared, true);
    let ids = wtxid_bytes(&(0..11).map(transaction_with_lock).collect::<Vec<_>>());

    assert!(send_page(&mut connection, &ids[..10]).is_ok());
    assert!(send_page(&mut connection, &ids[10..]).is_err());
}

#[test]
fn a_declaration_past_the_size_limit_closes_the_connection() {
    let shared = registry_with_size_limit(650);
    let (mut connection, token) = large_template_declarator(&shared, true);
    let ids = wtxid_bytes(&(0..11).map(transaction_with_lock).collect::<Vec<_>>());

    send_page(&mut connection, &ids[..6]).unwrap();

    assert!(declare(&mut connection, &token, &ids[6..]).is_err());
}

#[test]
fn a_page_after_its_declaration_closes_the_connection() {
    let shared = registry();
    let (mut connection, token) = large_template_declarator(&shared, true);
    let ids = wtxid_bytes(&(0..3).map(transaction_with_lock).collect::<Vec<_>>());
    declare(&mut connection, &token, &ids).unwrap();

    assert!(send_page(&mut connection, &ids).is_err());
}

/// The declared transaction memory follows the published template's size
/// limit, from a 64 MiB floor.
#[test]
fn the_declared_memory_bound_follows_the_block_size_limit() {
    let floor = registry_with_size_limit(32_000_000);
    assert_eq!(
        floor.lock().unwrap().declared_byte_limit(),
        DECLARED_TRANSACTION_BYTES_FLOOR
    );
    let raised = registry_with_size_limit(256_000_000);
    assert_eq!(raised.lock().unwrap().declared_byte_limit(), 512_000_000);
}

/// A Job Declaration connection on `shared`, set up with `flags`.
fn job_declaration_connection(shared: &Arc<Mutex<JobRegistry>>, flags: u32) -> Sv2Connection {
    let mut connection = Sv2Connection::new(Arc::clone(shared));
    accept_message(
        &mut connection,
        setup(flags, Protocol::JobDeclarationProtocol),
        MESSAGE_TYPE_SETUP_CONNECTION,
    );
    connection
}

/// Allocate a token on `connection` and return its bytes.
fn allocate_token(connection: &mut Sv2Connection, request_id: u32) -> Vec<u8> {
    let allocated = connection
        .on_allocate(request_id, "miner", Some(requesting_identity()))
        .unwrap();
    let mut payload = response_payload(&allocated[0]);
    AllocateMiningJobTokenSuccess::from_bytes(&mut payload)
        .unwrap()
        .mining_job_token
        .as_bytes()
        .to_vec()
}

/// Declare `transaction` on `token` as request 2, reveal it, and return the
/// ready token.
fn declare_ready(
    connection: &mut Sv2Connection,
    token: &[u8],
    transaction: &Transaction,
) -> Vec<u8> {
    let transactions = std::slice::from_ref(transaction);
    declare(connection, token, &wtxid_bytes(transactions)).unwrap();
    let success = reveal(connection, transactions).unwrap();
    assert_eq!(
        reply_type(&success[0]),
        MESSAGE_TYPE_DECLARE_MINING_JOB_SUCCESS
    );
    let mut payload = response_payload(&success[0]);
    DeclareMiningJobSuccess::from_bytes(&mut payload)
        .unwrap()
        .new_mining_job_token
        .as_bytes()
        .to_vec()
}

/// The token limit is each connection's own: two connections hold
/// `MAX_SESSION_JOB_TOKENS` open tokens each at the same time. One limit
/// for the whole registry closed the second client's connection here.
#[test]
fn two_connections_each_hold_their_own_tokens() {
    let shared = registry();
    let mut first = job_declaration_connection(&shared, 0);
    let mut second = job_declaration_connection(&shared, 0);
    let mut tokens = Vec::with_capacity(2 * MAX_SESSION_JOB_TOKENS);
    for request_id in 0..MAX_SESSION_JOB_TOKENS as u32 {
        tokens.push(allocate_token(&mut first, request_id));
        tokens.push(allocate_token(&mut second, request_id));
    }

    let registry = shared.lock().unwrap();
    assert_eq!(registry.tokens.len(), 2 * MAX_SESSION_JOB_TOKENS);
    assert!(
        tokens
            .iter()
            .all(|token| registry.open_token(token).is_some())
    );
}

/// The registry's own bound counts every connection's tokens, and an
/// allocation past it closes its connection. A connection that ends takes
/// its tokens with it, which makes room again.
#[test]
fn a_closed_connection_frees_its_tokens_for_the_registry_bound() {
    let shared = registry();
    let mut connections = Vec::with_capacity(MAX_JOB_TOKENS / MAX_SESSION_JOB_TOKENS);
    for _ in 0..MAX_JOB_TOKENS / MAX_SESSION_JOB_TOKENS {
        let mut connection = job_declaration_connection(&shared, 0);
        for request_id in 0..MAX_SESSION_JOB_TOKENS as u32 {
            allocate_token(&mut connection, request_id);
        }
        connections.push(connection);
    }
    assert_eq!(shared.lock().unwrap().tokens.len(), MAX_JOB_TOKENS);
    let mut refused = job_declaration_connection(&shared, 0);
    assert_eq!(
        refused.on_allocate(1, "miner", None).unwrap_err(),
        "job token limit reached"
    );

    drop(connections.pop());

    assert_eq!(
        shared.lock().unwrap().tokens.len(),
        MAX_JOB_TOKENS - MAX_SESSION_JOB_TOKENS
    );
    let mut admitted = job_declaration_connection(&shared, 0);
    allocate_token(&mut admitted, 1);
}

/// A token the limit dropped is answered like an unknown one by every
/// message that names it, and no connection closes: a request for its
/// payout outputs, a declaration, and a Mining connection's custom job.
#[test]
fn a_token_dropped_at_the_limit_gets_invalid_mining_job_token() {
    let shared = registry();
    let mut connection = job_declaration_connection(&shared, 1);
    assert!(connection.grant_payout_outputs());
    let prepared = test_bch_prepared();
    let template = prepared.template();
    let prefix = Builder::new()
        .push_int(i64::from(template.height))
        .into_bytes();
    let dropped = allocate_token(&mut connection, 0);
    let mut request =
        test_payout_outputs_request(1, &dropped, template, template.coinbasevalue, &prefix);
    let answered = connection
        .handle_payout_outputs(REQUEST_PAYOUT_OUTPUTS, &mut request)
        .unwrap();
    assert_eq!(answered[0][2], REQUEST_PAYOUT_OUTPUTS_SUCCESS);

    for request_id in 1..=MAX_SESSION_JOB_TOKENS as u32 {
        allocate_token(&mut connection, request_id);
    }

    assert!(!connection.payout_sets.contains(&dropped));
    let mut request =
        test_payout_outputs_request(2, &dropped, template, template.coinbasevalue, &prefix);
    let answered = connection
        .handle_payout_outputs(REQUEST_PAYOUT_OUTPUTS, &mut request)
        .unwrap();
    assert_eq!(answered[0][2], REQUEST_PAYOUT_OUTPUTS_ERROR);
    let mut payload = response_payload(&answered[0]);
    let error = RequestPayoutOutputsError::from_bytes(&mut payload).unwrap();
    assert_eq!(error.error_code.as_utf8_or_hex(), INVALID_MINING_JOB_TOKEN);

    let declared = declare(&mut connection, &dropped, &[]).unwrap();
    assert_eq!(
        declare_refusal(&declared[0]).0,
        ERROR_CODE_DECLARE_MINING_JOB_INVALID_MINING_JOB_TOKEN
    );

    let mut mining = Sv2Connection::new(Arc::clone(&shared));
    accept_message(
        &mut mining,
        setup(WORK_SELECTION_FLAG, Protocol::MiningProtocol),
        MESSAGE_TYPE_SETUP_CONNECTION,
    );
    assert_eq!(
        mining.take_declared_job(&dropped).err().as_deref(),
        Some("invalid-mining-job-token")
    );
    let custom = accept_message(
        &mut mining,
        custom_job(&dropped),
        MESSAGE_TYPE_SET_CUSTOM_MINING_JOB,
    );
    let mut custom_payload = response_payload(&custom[0]);
    let rejected = SetCustomMiningJobError::from_bytes(&mut custom_payload).unwrap();
    assert_eq!(
        rejected.error_code.as_utf8_or_hex(),
        "invalid-mining-job-token"
    );
}

/// A connection at its limit with no open token loses its oldest token of
/// any state, and stays up. The transactions the token declared leave the
/// registry's count. Transactions that then arrive for a dropped
/// declaration get `DeclareMiningJob.Error`, once, and a custom job on a
/// dropped ready token gets `invalid-mining-job-token`.
#[test]
fn a_connection_with_no_open_token_drops_its_oldest_declaration() {
    let shared = registry();
    let mut connection = job_declaration_connection(&shared, 1);
    let prefix = pushed_coinbase_prefix();
    let suffix = empty_commitment_suffix();
    // The oldest token waits for its transaction, as declaration 9.
    let waiting = allocate_token(&mut connection, 0);
    let unrevealed = transaction_with_lock(1_000);
    let wtxid = unrevealed.compute_wtxid().to_byte_array();
    let asked = accept_message(
        &mut connection,
        DeclareMiningJob {
            request_id: 9,
            mining_job_token: B0255::new(&waiting).unwrap(),
            version: 1,
            coinbase_tx_prefix: B064K::new(&prefix).unwrap(),
            coinbase_tx_suffix: B064K::new(&suffix).unwrap(),
            wtxid_list: Seq064K::new(vec![U256::new(wtxid.as_slice()).unwrap()]).unwrap(),
            excess_data: B064K::new(&[]).unwrap(),
        },
        MESSAGE_TYPE_DECLARE_MINING_JOB,
    );
    assert_eq!(
        reply_type(&asked[0]),
        MESSAGE_TYPE_PROVIDE_MISSING_TRANSACTIONS
    );
    // Every other token is declared and ready.
    let mut ready = Vec::with_capacity(MAX_SESSION_JOB_TOKENS);
    for lock_time in 1..MAX_SESSION_JOB_TOKENS as u32 {
        let token = allocate_token(&mut connection, lock_time);
        ready.push(declare_ready(
            &mut connection,
            &token,
            &transaction_with_lock(lock_time),
        ));
    }
    let declared_bytes = || shared.lock().unwrap().declared_bytes();
    let before = declared_bytes();

    // No token is open, so the next allocation drops the waiting one.
    let newest = allocate_token(&mut connection, 100);

    {
        let registry = shared.lock().unwrap();
        assert_eq!(
            registry.session_tokens(connection.session_id),
            MAX_SESSION_JOB_TOKENS
        );
        assert!(!registry.tokens.contains_key(&waiting));
    }
    assert_eq!(declared_bytes(), before - 32 - prefix.len() - suffix.len());
    let reveal_waiting = |connection: &mut Sv2Connection| {
        let serialized = serialize(&unrevealed);
        let frame = encode_message(
            ProvideMissingTransactionsSuccess {
                request_id: 9,
                transaction_list: Seq064K::new(vec![B016M::new(serialized.as_slice()).unwrap()])
                    .unwrap(),
            },
            MESSAGE_TYPE_PROVIDE_MISSING_TRANSACTIONS_SUCCESS,
        )
        .unwrap();
        connection.accept(&frame)
    };
    let refused = reveal_waiting(&mut connection).unwrap();
    assert_eq!(
        declare_refusal(&refused[0]).0,
        ERROR_CODE_DECLARE_MINING_JOB_INVALID_MINING_JOB_TOKEN
    );
    assert_eq!(
        reveal_waiting(&mut connection).unwrap_err(),
        "transactions arrived with no open declaration"
    );

    // With the newest token ready as well, the oldest ready one goes next.
    declare_ready(&mut connection, &newest, &transaction_with_lock(100));
    let before = declared_bytes();
    allocate_token(&mut connection, 101);

    let oldest_bytes = transaction_with_lock(1).total_size() + prefix.len() + suffix.len();
    assert_eq!(declared_bytes(), before - oldest_bytes);
    let mining = Sv2Connection::new(Arc::clone(&shared));
    assert_eq!(
        mining.take_declared_job(&ready[0]).err().as_deref(),
        Some("invalid-mining-job-token")
    );
    assert!(mining.take_declared_job(&ready[1]).is_ok());
}
