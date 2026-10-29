// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A Bitcoin Cash miner's own donation, chosen with `donation=` at login.

use super::*;
use crate::config::{BitcoinCashNetwork, bitcoin_cash_donation_address};
use crate::stratum::work::coinbase::parse_bitcoin_coinbase_fields;
use crate::stratum::work::difficulty::validate::build_coinbase_from_components;
use crate::stratum::work::tracker::start_tracker_actor;
use crate::test_utils::{test_bch_prepared, test_bch_prepared_with_cuts};
use bitcoin::{Amount, Network, PubkeyHash, Transaction};
use p2poolv2_wallet::share_owner::ShareOwner;

fn fee_address() -> Address {
    Address::p2pkh(PubkeyHash::from_byte_array([0x55; 20]), Network::Regtest)
}

fn donation_script() -> bitcoin::ScriptBuf {
    bitcoin_cash_donation_address(BitcoinCashNetwork::Regtest).script_pubkey()
}

/// A node donating 1% and taking a 1% fee, the defaults.
fn node_at_the_defaults() -> Arc<PreparedNotifyParams> {
    test_bch_prepared_with_cuts(Some(100), Some(fee_address()), Some(100))
}

/// Insert one miner's job on `prepared` and return its commitment and the
/// coinbase the miner hashes.
fn mine(prepared: &PreparedNotifyParams) -> (ShareCommitment, Transaction) {
    let tracker = start_tracker_actor();
    let payout = Address::p2pkh(PubkeyHash::from_byte_array([1; 20]), Network::Regtest);
    let (job_id, _) = insert_miner_job(
        prepared,
        Some(&payout),
        Some(ShareOwner::CashP2pkh([0x22; 20])),
        &tracker,
    );
    let job = tracker.get_job(job_id).unwrap();
    let coinbase = build_coinbase_from_components(
        &job.coinbase1,
        "00000000",
        "0000000000000000",
        &job.coinbase2,
    )
    .unwrap();
    (job.share_commitment.unwrap(), coinbase)
}

/// The coinbase a validating peer rebuilds from the share's committed
/// donation and fee and the window's weights, as
/// `DefaultShareValidator::validate_bitcoin_payout` does.
fn rebuilt_by_a_peer(
    prepared: &PreparedNotifyParams,
    commitment: &ShareCommitment,
    coinbase: &Transaction,
) -> Transaction {
    let expected = build_pplns_outputs(
        coinbase
            .output
            .iter()
            .map(|output| output.value)
            .sum::<Amount>()
            .to_sat(),
        prepared.pplns_weights.as_ref().unwrap(),
        &commitment.donation_address,
        commitment.donation,
        &commitment.fee_address,
        commitment.fee,
        prepared.payout_split,
    )
    .unwrap();
    let fields = parse_bitcoin_coinbase_fields(coinbase).unwrap();
    build_coinbase(
        Version::TWO,
        &expected.outputs,
        prepared.template.height as i64,
        fields.aux_flags,
        fields.witness_commitment.as_ref(),
        &fields.pool_signature,
        Some(commitment.hash()),
        fields.nsecs,
        Some(&fields.extranonce),
        Some(MergeMiningCommitment::empty()),
        expected.tree_output().as_ref(),
    )
    .unwrap()
}

#[test]
fn no_choice_and_the_nodes_own_donation_use_the_nodes_params() {
    let prepared = node_at_the_defaults();
    assert!(prepared.for_miner_donation(None).unwrap().is_none());
    assert!(prepared.for_miner_donation(Some(100)).unwrap().is_none());
}

/// A coinbase's outputs but the last, the share commitment, which commits to
/// each miner's own terms.
fn payouts(coinbase: &Transaction) -> &[bitcoin::TxOut] {
    &coinbase.output[..coinbase.output.len() - 1]
}

/// Whether `coinbase` has an output to the donation key or the node's fee
/// address. On Bitcoin Cash it never does for a share's own cuts: they come
/// off that share's own weight in the windows that pay it.
fn pays_a_cut_output(coinbase: &Transaction) -> bool {
    coinbase.output.iter().any(|output| {
        output.script_pubkey == donation_script()
            || output.script_pubkey == fee_address().script_pubkey()
    })
}

/// A miner who opts out commits no donation in its share and keeps the
/// operator's 1% fee. The coinbase it mines is the node's own: the choice is
/// a term of the miner's share, not a cut of this coinbase.
#[tokio::test]
async fn opting_out_commits_no_donation_and_keeps_the_fee() {
    let prepared = node_at_the_defaults();
    let opted_out = prepared.for_miner_donation(Some(0)).unwrap().unwrap();
    let (commitment, coinbase) = mine(&opted_out);
    let (_, node_coinbase) = mine(&prepared);

    assert_eq!(commitment.donation, None);
    assert_eq!(commitment.donation_address, None);
    assert_eq!(commitment.fee, Some(100));
    assert_eq!(commitment.fee_address, Some(fee_address()));
    assert_eq!(payouts(&coinbase), payouts(&node_coinbase));
    assert!(!pays_a_cut_output(&coinbase));
    assert_eq!(
        coinbase
            .output
            .iter()
            .map(|output| output.value)
            .sum::<Amount>(),
        Amount::from_sat(prepared.template.coinbasevalue)
    );
    assert_eq!(
        rebuilt_by_a_peer(&prepared, &commitment, &coinbase),
        coinbase
    );
    assert_eq!(
        prepared.donation,
        Some(100),
        "the node's own params are untouched"
    );
}

/// Half a percent is committed to the built-in donation address in the
/// miner's share, and the coinbase stays the node's.
#[tokio::test]
async fn a_half_percent_donation_is_committed_in_the_share() {
    let prepared = node_at_the_defaults();
    let chosen = prepared.for_miner_donation(Some(50)).unwrap().unwrap();
    let (commitment, coinbase) = mine(&chosen);
    let (_, node_coinbase) = mine(&prepared);

    assert_eq!(commitment.donation, Some(50));
    assert_eq!(
        commitment
            .donation_address
            .as_ref()
            .map(Address::script_pubkey),
        Some(donation_script())
    );
    assert_eq!(payouts(&coinbase), payouts(&node_coinbase));
    assert!(!pays_a_cut_output(&coinbase));
    assert_eq!(
        rebuilt_by_a_peer(&prepared, &commitment, &coinbase),
        coinbase
    );
}

/// The node's donation is only a default: a node donating 10% cannot raise
/// a miner who chose 0, and a node donating nothing honours a miner who
/// chose more.
#[tokio::test]
async fn the_node_cannot_override_the_miners_choice() {
    let generous_node = test_bch_prepared_with_cuts(Some(1_000), None, None);
    let opted_out = generous_node.for_miner_donation(Some(0)).unwrap().unwrap();
    let (commitment, coinbase) = mine(&opted_out);
    assert_eq!(commitment.donation, None);
    assert_eq!(
        rebuilt_by_a_peer(&generous_node, &commitment, &coinbase),
        coinbase
    );

    let node_without_donation = test_bch_prepared();
    let chosen = node_without_donation
        .for_miner_donation(Some(200))
        .unwrap()
        .unwrap();
    let (commitment, coinbase) = mine(&chosen);
    assert_eq!(commitment.donation, Some(200));
    assert!(!pays_a_cut_output(&coinbase));
    assert_eq!(
        rebuilt_by_a_peer(&node_without_donation, &commitment, &coinbase),
        coinbase
    );
}

/// The donation and the fee have independent 10% caps: a miner may commit
/// a 10% donation on a node taking a 10% fee.
#[tokio::test]
async fn a_ten_percent_donation_beside_a_ten_percent_fee() {
    let prepared = test_bch_prepared_with_cuts(Some(100), Some(fee_address()), Some(1_000));
    let chosen = prepared.for_miner_donation(Some(1_000)).unwrap().unwrap();
    let (commitment, coinbase) = mine(&chosen);

    assert_eq!(commitment.donation, Some(1_000));
    assert_eq!(commitment.fee, Some(1_000));
    assert!(!pays_a_cut_output(&coinbase));
    assert_eq!(
        rebuilt_by_a_peer(&prepared, &commitment, &coinbase),
        coinbase
    );
}

#[test]
fn choices_off_the_half_percent_steps_or_above_the_cap_are_refused() {
    let prepared = node_at_the_defaults();
    for basis_points in [25, 101, 1_050, u16::MAX] {
        assert!(
            prepared.for_miner_donation(Some(basis_points)).is_err(),
            "{basis_points}"
        );
    }
}

/// Every miner choosing one donation on a template shares one rebuild of it,
/// which keeps the template, anchor, weights and share transactions.
#[test]
fn one_rebuild_per_choice_is_shared() {
    let prepared = node_at_the_defaults();
    let first = prepared.for_miner_donation(Some(0)).unwrap().unwrap();
    let second = prepared.for_miner_donation(Some(0)).unwrap().unwrap();
    let other = prepared.for_miner_donation(Some(50)).unwrap().unwrap();

    assert!(Arc::ptr_eq(&first, &second));
    assert!(!Arc::ptr_eq(&first, &other));
    assert!(Arc::ptr_eq(&first.template, &prepared.template));
    assert!(Arc::ptr_eq(
        first.pplns_weights.as_ref().unwrap(),
        prepared.pplns_weights.as_ref().unwrap()
    ));
    assert_eq!(first.prev_share_blockhash, prepared.prev_share_blockhash);
    assert_eq!(first.bits, prepared.bits);
    assert_eq!(first.share_witness_root, prepared.share_witness_root);
    assert_eq!(first.coinbase1, prepared.coinbase1);
    assert_eq!(first.merkle_branches, prepared.merkle_branches);
}

/// A rebuild answers for its own donation and refuses to be rebuilt again;
/// the miner's choice is always taken from the node's own params.
#[test]
fn a_rebuild_is_not_rebuilt() {
    let prepared = node_at_the_defaults();
    let opted_out = prepared.for_miner_donation(Some(0)).unwrap().unwrap();
    assert!(opted_out.for_miner_donation(Some(0)).unwrap().is_none());
    assert!(opted_out.for_miner_donation(Some(50)).is_err());
}

/// Params built without `miner_donations`, as on Bitcoin or in Hydrapool
/// mode, refuse a miner's own donation rather than ignore it.
#[test]
fn params_without_miner_donations_refuse_a_choice() {
    let template: BlockTemplate = serde_json::from_str(include_str!(
        "../../../../../bitcoindrpc/test_data/bchn-regtest-template.json"
    ))
    .unwrap();
    let payout = Address::p2pkh(PubkeyHash::from_byte_array([1; 20]), Network::Regtest);
    let prepared = PreparedNotifyParamsBuilder::new(
        Arc::new(template.clone()),
        vec![crate::accounting::OutputPair {
            address: payout,
            amount: Amount::from_sat(template.coinbasevalue),
        }],
        b"P2Poolv2",
        true,
    )
    .bits(CompactTarget::from_consensus(0x2100ffff))
    .include_merge_mining(true)
    .build()
    .unwrap();

    assert!(prepared.for_miner_donation(None).unwrap().is_none());
    assert!(prepared.for_miner_donation(Some(0)).unwrap().is_none());
    let Err(error) = prepared.for_miner_donation(Some(50)) else {
        panic!("a choice was accepted without miner donations");
    };
    assert!(error.message.contains("cannot carry"), "{}", error.message);
}
