use super::*;
use crate::accounting::OutputPair;
use crate::shares::validation::MAX_UNCLES;
use crate::stratum::work::block_template::BlockTemplate;
use crate::stratum::work::tracker::{JobId, start_tracker_actor};
use crate::test_utils::make_test_share_address;
use crate::test_utils::make_test_share_program;
use crate::test_utils::test_bch_prepared;
use crate::test_utils::test_coinbase_transaction;
use bitcoin::{
    Amount, CompressedPublicKey, Network, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut,
    Txid, Witness, absolute,
};
use p2poolv2_wallet::share_owner::ShareOwner;

#[test]
fn bch_job_token_roundtrips_all_terms_within_the_wire_budget() {
    let mut prepared = Arc::try_unwrap(test_bch_prepared()).ok().unwrap();
    prepared.uncles = vec![BlockHash::from_byte_array([0x44; 32]); MAX_UNCLES];
    prepared.commitment_prefix = build_commitment_prefix(
        prepared.prev_share_blockhash,
        &prepared.uncles,
        prepared.bits,
    );
    let payout = Address::p2pkh(
        bitcoin::PubkeyHash::from_byte_array([0x22; 20]),
        Network::Regtest,
    );
    prepared.donation_address = Some(payout.clone());
    prepared.donation = Some(150);
    prepared.fee_address = Some(payout.clone());
    prepared.fee = Some(50);
    let token = prepared.mining_job_token([9; 32], 1234).unwrap();
    assert!(token.len() <= 255);
    let commitment = ShareCommitment::from_mining_job_token(
        &token,
        payout.clone(),
        ShareOwner::CashP2pkh([0x22; 20]),
    )
    .unwrap();
    assert_eq!(
        commitment.prev_share_blockhash,
        prepared.prev_share_blockhash
    );
    assert_eq!(commitment.uncles, prepared.uncles);
    assert_eq!(commitment.time, 1234);
    assert_eq!(commitment.bits, prepared.bits);
    assert_eq!(commitment.donation_address, Some(payout.clone()));
    assert_eq!(commitment.fee_address, Some(payout.clone()));
    assert_eq!(commitment.donation, Some(150));
    assert_eq!(commitment.fee, Some(50));
    let mut trailing = token.clone();
    trailing.push(0);
    assert!(
        ShareCommitment::from_mining_job_token(
            &trailing,
            payout.clone(),
            ShareOwner::CashP2pkh([0x22; 20])
        )
        .is_err()
    );
    let mut excess_uncles = token;
    excess_uncles[65] = (MAX_UNCLES + 1) as u8;
    assert!(
        ShareCommitment::from_mining_job_token(
            &excess_uncles,
            payout,
            ShareOwner::CashP2pkh([0x22; 20])
        )
        .is_err()
    );
}

fn test_template() -> BlockTemplate {
    let data =
        include_str!("../../../../../p2poolv2_tests/test_data/gbt/regtest/ckpool/one-txn/gbt.json");
    serde_json::from_str(data).expect("Failed to parse BlockTemplate")
}

fn test_address() -> Address {
    let miner_pubkey: CompressedPublicKey =
        "020202020202020202020202020202020202020202020202020202020202020202"
            .parse()
            .unwrap();
    Address::p2wpkh(&miner_pubkey, Network::Signet)
}

fn test_output_distribution(template: &BlockTemplate) -> Vec<OutputPair> {
    vec![OutputPair {
        address: test_address(),
        amount: bitcoin::Amount::from_sat(template.coinbasevalue),
    }]
}

fn test_notify_params_builder(
    template: Arc<BlockTemplate>,
    clean_jobs: bool,
) -> PreparedNotifyParamsBuilder {
    let output_distribution = test_output_distribution(&template);
    PreparedNotifyParamsBuilder::new(template, output_distribution, b"test_pool", clean_jobs)
        .bits(CompactTarget::from_consensus(0x1d00ffff))
}

/// A share transaction of `size` bytes or slightly more, distinct for
/// each `byte`.
fn share_transaction_of_size(byte: u8, size: usize) -> ShareTransaction {
    ShareTransaction(Transaction {
        version: Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::new(Txid::from_byte_array([byte; 32]), 0),
            script_sig: ScriptBuf::new(),
            sequence: Sequence::MAX,
            witness: Witness::new(),
        }],
        output: vec![TxOut {
            value: Amount::from_sat(1_000),
            script_pubkey: ScriptBuf::from_bytes(vec![byte; size]),
        }],
    })
}

#[test]
fn share_transactions_past_the_block_size_limit_are_dropped_from_the_end() {
    let first = share_transaction_of_size(1, 90_000);
    let second = share_transaction_of_size(2, 90_000);
    let third = share_transaction_of_size(3, 90_000);

    let kept = fit_share_transactions(
        vec![first.clone(), second.clone(), third],
        &test_coinbase_transaction(1),
    );

    assert_eq!(kept, vec![first, second]);
}

#[test]
fn share_transactions_within_the_block_size_limit_are_all_kept() {
    let first = share_transaction_of_size(1, 1_000);
    let second = share_transaction_of_size(2, 1_000);

    let kept = fit_share_transactions(
        vec![first.clone(), second.clone()],
        &test_coinbase_transaction(1),
    );

    assert_eq!(kept, vec![first, second]);
}

#[tokio::test]
async fn a_job_without_share_transactions_commits_to_the_empty_root() {
    let prepared = test_bch_prepared();
    let tracker = start_tracker_actor();
    let payout = Address::p2pkh(
        bitcoin::PubkeyHash::from_byte_array([1; 20]),
        Network::Regtest,
    );

    let (job_id, _) = insert_miner_job(
        &prepared,
        Some(&payout),
        Some(ShareOwner::CashP2pkh([9; 20])),
        &tracker,
    );

    let job = tracker.get_job(job_id).unwrap();
    assert!(job.share_transactions.is_empty());
    assert_eq!(
        job.share_commitment.unwrap().share_witness_root,
        compute_witness_root(&[])
    );
}

#[test]
fn test_prepare_notify_params_produces_valid_json() {
    let template = Arc::new(test_template());
    let prepared = test_notify_params_builder(template, false)
        .build()
        .expect("build should succeed");

    // Verify the JSON is parseable
    let parsed: serde_json::Value =
        serde_json::from_str(&prepared.json_template).expect("JSON should be valid");
    assert_eq!(parsed["method"], "mining.notify");
    let params = parsed["params"].as_array().expect("params should be array");
    assert_eq!(params.len(), 9);

    // Verify offsets are within bounds
    assert!(prepared.job_id_offset + 16 <= prepared.json_template.len());
    assert!(
        prepared.coinbase2_offset + prepared.coinbase2_placeholder_len
            <= prepared.json_template.len()
    );
}

#[test]
fn bch_job_rejects_a_coinbase_that_exceeds_the_block_limit() {
    let mut template: BlockTemplate = serde_json::from_str(include_str!(
        "../../../../../bitcoindrpc/test_data/bchn-regtest-template.json"
    ))
    .unwrap();
    template.sizelimit = 81;
    template.validate_bch().unwrap();
    let distribution = vec![OutputPair {
        address: Address::p2pkh(bitcoin::PubkeyHash::all_zeros(), Network::Regtest),
        amount: bitcoin::Amount::from_sat(template.coinbasevalue),
    }];
    let result =
        PreparedNotifyParamsBuilder::new(Arc::new(template), distribution, b"test_pool", true)
            .include_merge_mining(true)
            .build();
    assert!(result.err().unwrap().message.contains("block size limit"));
}

#[tokio::test]
async fn test_build_notify_from_prepared_produces_valid_notify() {
    let template = Arc::new(test_template());
    let address = test_address();
    let tracker_handle = start_tracker_actor();

    let prepared = test_notify_params_builder(template, false)
        .build()
        .expect("build should succeed");

    let notify_json = build_notify_from_prepared(
        &prepared,
        Some(&address),
        Some(p2poolv2_wallet::share_owner::ShareOwner::Taproot(
            make_test_share_address(1, bitcoin::Network::Signet).witness_program(),
        )),
        &tracker_handle,
    )
    .expect("build_notify_from_prepared should succeed");

    // Verify the result is valid JSON
    let parsed: serde_json::Value =
        serde_json::from_str(&notify_json).expect("notify JSON should be valid");
    assert_eq!(parsed["method"], "mining.notify");

    // Verify job_id was filled in (not zeros)
    let params = parsed["params"].as_array().unwrap();
    let job_id_str = params[0].as_str().unwrap();
    assert_ne!(job_id_str, "0000000000000000");

    // Verify job was inserted in tracker by parsing the job_id from JSON
    let job_id = JobId(u64::from_str_radix(job_id_str, 16).unwrap());
    let job_details = tracker_handle.get_job(job_id);
    assert!(job_details.is_some());

    // Verify commitment was stored with correct miner address
    let details = job_details.unwrap();
    let commitment = details.share_commitment.as_ref().unwrap();
    assert_eq!(commitment.miner_bitcoin_address, address);
    assert_eq!(commitment.prev_share_blockhash, BlockHash::all_zeros());

    // Verify merkle branches were stored in job details (1 branch for 1-txn template)
    assert_eq!(
        details.template_merkle_branches.len(),
        1,
        "Expected 1 merkle branch for 1-txn template"
    );
}

#[tokio::test]
async fn test_commitment_hash_matches_struct_hash() {
    let template = Arc::new(test_template());
    let address = test_address();
    let bits = CompactTarget::from_consensus(0x1d00ffff);
    let tracker_handle = start_tracker_actor();

    let prepared = test_notify_params_builder(template, false)
        .build()
        .expect("build should succeed");

    let notify_json = build_notify_from_prepared(
        &prepared,
        Some(&address),
        Some(p2poolv2_wallet::share_owner::ShareOwner::Taproot(
            make_test_share_address(1, bitcoin::Network::Signet).witness_program(),
        )),
        &tracker_handle,
    )
    .expect("build_notify_from_prepared should succeed");

    // Parse job_id from JSON to look up the tracker entry
    let parsed: serde_json::Value = serde_json::from_str(&notify_json).unwrap();
    let job_id_str = parsed["params"][0].as_str().unwrap();
    let job_id = JobId(u64::from_str_radix(job_id_str, 16).unwrap());
    let details = tracker_handle.get_job(job_id).unwrap();
    let commitment = details.share_commitment.as_ref().unwrap();

    // Build the same commitment directly using the fresh time that
    // build_notify_from_prepared stored in the tracker.
    let direct_commitment = ShareCommitment {
        bch_coinbase: None,
        prev_share_blockhash: BlockHash::all_zeros(),
        uncles: Vec::new(),
        miner_bitcoin_address: address,
        miner_address: make_test_share_program(1).into(),
        share_witness_root: compute_witness_root(&[]),
        bits,
        time: commitment.time,
        donation_address: None,
        donation: None,
        fee_address: None,
        fee: None,
    };

    assert_eq!(commitment.hash(), direct_commitment.hash());
}

#[test]
fn test_different_addresses_produce_different_hashes() {
    let template = Arc::new(test_template());
    let prepared = test_notify_params_builder(template, false)
        .build()
        .expect("build should succeed");

    let address1 = test_address();
    let other_pubkey: CompressedPublicKey =
        "02ac493f2130ca56cb5c3a559860cef9a84f90b5a85dfe4ec6e6067eeee17f4d2d"
            .parse()
            .unwrap();
    let address2 = Address::p2wpkh(&other_pubkey, Network::Signet);
    let share_address = make_test_share_program(1);

    let time = 1_700_000_000;
    let hash1 = get_commitment_hex(
        &prepared.commitment_prefix,
        time,
        &prepared.commitment_suffix,
        Some(&address1),
        Some(share_address.into()),
    );
    let hash2 = get_commitment_hex(
        &prepared.commitment_prefix,
        time,
        &prepared.commitment_suffix,
        Some(&address2),
        Some(share_address.into()),
    );

    assert_ne!(
        hash1, hash2,
        "the bitcoin address must reach the commitment hash"
    );
}

/// Mirror of `test_different_addresses_produce_different_hashes`: hold the
/// bitcoin address fixed and vary the share address instead.
///
/// Both assert on the commitment hash rather than the notify JSON, because
/// the JSON carries a fresh job id and timestamp per call and so always
/// differs -- which would make either test pass even if the address were
/// ignored entirely.
/// The two callers of `commitment_digest` must agree: the notify path uses
/// a prefix and suffix pre-built per template, while `ShareCommitment::hash`
/// serializes its own fields per share. A miner mines the first and every
/// validator reconstructs the second, so a divergence means no share is
/// ever accepted.
#[test]
fn notify_hash_matches_share_commitment_hash() {
    let template = Arc::new(test_template());
    let prepared = test_notify_params_builder(template, false)
        .build()
        .expect("build should succeed");

    let bitcoin_address = test_address();
    let share_address = make_test_share_program(1);
    let share_witness_root = compute_witness_root(&[]);
    let time = 1_700_000_000;

    let from_notify = get_commitment_hex(
        &prepared.commitment_prefix,
        time,
        &prepared.commitment_suffix,
        Some(&bitcoin_address),
        Some(share_address.into()),
    );

    let commitment = ShareCommitment {
        bch_coinbase: None,
        prev_share_blockhash: prepared.prev_share_blockhash,
        uncles: prepared.uncles.clone(),
        miner_bitcoin_address: bitcoin_address,
        miner_address: share_address.into(),
        share_witness_root,
        bits: prepared.bits,
        time,
        donation_address: prepared.donation_address.clone(),
        donation: prepared.donation,
        fee_address: prepared.fee_address.clone(),
        fee: prepared.fee,
    };
    let from_struct = hex::encode(commitment.hash().as_byte_array());

    assert_eq!(from_notify, from_struct);
}

#[test]
fn test_different_share_addresses_produce_different_hashes() {
    let template = Arc::new(test_template());
    let prepared = test_notify_params_builder(template, false)
        .build()
        .expect("build should succeed");

    let bitcoin_address = test_address();
    let share_address1 = make_test_share_program(1);
    let share_address2 = make_test_share_program(2);
    assert_ne!(share_address1, share_address2);

    let time = 1_700_000_000;
    let hash1 = get_commitment_hex(
        &prepared.commitment_prefix,
        time,
        &prepared.commitment_suffix,
        Some(&bitcoin_address),
        Some(share_address1.into()),
    );
    let hash2 = get_commitment_hex(
        &prepared.commitment_prefix,
        time,
        &prepared.commitment_suffix,
        Some(&bitcoin_address),
        Some(share_address2.into()),
    );

    assert_ne!(
        hash1, hash2,
        "the share address must reach the commitment hash"
    );
}

#[test]
fn test_placeholder_offsets_produce_correct_overwrite() {
    let template = Arc::new(test_template());
    let prepared = test_notify_params_builder(template, true)
        .build()
        .expect("build should succeed");

    // Manually overwrite placeholders and verify JSON remains valid
    let mut json = prepared.json_template.clone();
    let test_job_id = "abcdef0123456789";
    // Build a test coinbase2 of the correct length
    let test_coinbase2 = "f".repeat(prepared.coinbase2_placeholder_len);
    json.replace_range(
        prepared.job_id_offset..prepared.job_id_offset + 16,
        test_job_id,
    );
    json.replace_range(
        prepared.coinbase2_offset..prepared.coinbase2_offset + prepared.coinbase2_placeholder_len,
        &test_coinbase2,
    );

    let parsed: serde_json::Value =
        serde_json::from_str(&json).expect("Overwritten JSON should still be valid");
    let params = parsed["params"].as_array().unwrap();
    assert_eq!(params[0].as_str().unwrap(), test_job_id);
    // coinbase2 should be the test value
    let coinbase2 = params[3].as_str().unwrap();
    assert_eq!(coinbase2, test_coinbase2);
    // clean_jobs should be true
    assert!(params[8].as_bool().unwrap());
}

#[tokio::test]
async fn test_build_notify_from_prepared_with_none_address() {
    let template = Arc::new(test_template());
    let tracker_handle = start_tracker_actor();

    let prepared = test_notify_params_builder(template, false)
        .build()
        .expect("build should succeed");

    let notify_json = build_notify_from_prepared(&prepared, None, None, &tracker_handle)
        .expect("build_notify_from_prepared with None address should succeed");

    // Verify the result is valid JSON
    let parsed: serde_json::Value =
        serde_json::from_str(&notify_json).expect("notify JSON should be valid");
    assert_eq!(parsed["method"], "mining.notify");

    // Verify job_id was filled in (not zeros)
    let params = parsed["params"].as_array().unwrap();
    let job_id_str = params[0].as_str().unwrap();
    assert_ne!(job_id_str, "0000000000000000");

    // Verify nsecs and the commitment output were placed in coinbase2.
    // Format: "08" + 16 hex nsecs + middle + "6a20" + 64 hex hash + 8 hex locktime
    let coinbase2 = params[3].as_str().unwrap();
    assert!(
        coinbase2.starts_with("08"),
        "coinbase2 should start with the nsecs push opcode"
    );
    let commitment_script_start = coinbase2.len() - 8 - 64 - 4;
    assert_eq!(
        &coinbase2[commitment_script_start..commitment_script_start + 4],
        "6a20",
        "coinbase2 should end with the OP_RETURN commitment output and locktime"
    );

    // Verify job was inserted in tracker with no share_commitment
    let job_id = JobId(u64::from_str_radix(job_id_str, 16).unwrap());
    let details = tracker_handle
        .get_job(job_id)
        .expect("Job should be in tracker");
    assert!(
        details.share_commitment.is_none(),
        "share_commitment should be None for solo mode"
    );
}

#[test]
fn test_split_coinbase2_separates_middle_and_locktime() {
    let dummy_commitment_hash = hashes::sha256::Hash::from_byte_array([0xab_u8; 32]);
    let coinbase = build_bitcoin_coinbase_transaction(
        Version::TWO,
        &[OutputPair {
            address: test_address(),
            amount: bitcoin::Amount::from_sat(5_000_000_000),
        }],
        100,
        bitcoin::script::PushBytesBuf::from(&[0u8]),
        None,
        b"P2Poolv2",
        Some(dummy_commitment_hash),
        0,
        None,
    )
    .unwrap();
    let (_coinbase1, coinbase2) = split_coinbase(&coinbase).unwrap();
    let dummy_commitment_hex = hex::encode(dummy_commitment_hash.as_byte_array());

    let (middle, locktime) = split_coinbase2(&coinbase2, &dummy_commitment_hex).unwrap();

    assert_eq!(
        format!(
            "{}{middle}{dummy_commitment_hex}{locktime}",
            &coinbase2[..18]
        ),
        coinbase2
    );
    assert_eq!(locktime, hex::encode(99u32.to_le_bytes()));
    assert!(middle.ends_with("6a20"));
}

/// A coinbase without the commitment output cannot carry a per-miner
/// commitment, so the split refuses it rather than slicing payout bytes.
#[test]
fn test_split_coinbase2_rejects_coinbase_without_commitment_output() {
    let coinbase = build_bitcoin_coinbase_transaction(
        Version::TWO,
        &[OutputPair {
            address: test_address(),
            amount: bitcoin::Amount::from_sat(5_000_000_000),
        }],
        100,
        bitcoin::script::PushBytesBuf::from(&[0u8]),
        None,
        b"P2Poolv2",
        None,
        0,
        None,
    )
    .unwrap();
    let (_coinbase1, coinbase2) = split_coinbase(&coinbase).unwrap();

    let result = split_coinbase2(&coinbase2, &hex::encode([0xab_u8; 32]));

    assert!(result.is_err());
}

#[test]
fn test_split_coinbase2_rejects_short_coinbase2() {
    let result = split_coinbase2("08", &hex::encode([0xab_u8; 32]));
    assert!(result.unwrap_err().message.contains("too short"));
}
