// SPDX-License-Identifier: AGPL-3.0-or-later

#![cfg(feature = "test-utils")]

use libauth::{Nft, NonFungibleTokenCapability, Token, encode_data_push};

use p2poolv2_photon_payout::lean::program::generated_source;
use p2poolv2_photon_payout::lean::test_utils::{
    FIXTURE_COST, build_fixture, claim_amount_fixture, fixture, manifest, mixed_profile,
    profile_fixture, proof_binding, reject_both, replace_redeem, small_cost_fixture, verify_both,
};
use p2poolv2_photon_payout::lean::{ListOrder, SplitOptions, build_split};
use p2poolv2_photon_payout::state::RootLock;
use p2poolv2_photon_payout::test_utils::{
    beneficiary_script, funding_script, sign_funding_transaction, spend_options,
};
use p2poolv2_photon_payout::vm::evaluate_input;

#[test]
fn lean_one_payee_both_modes() {
    verify_both(&fixture(1, RootLock::P2sh20).unwrap()).unwrap();
}

#[test]
fn lean_eight_payees_both_modes() {
    verify_both(&fixture(8, RootLock::P2sh20).unwrap()).unwrap();
}

#[test]
fn lean_four_token_free_funders_pass_both_modes() {
    let mut split = build_split(
        &manifest(8),
        8 * 20 * FIXTURE_COST,
        &SplitOptions {
            spend: spend_options(RootLock::P2sh20, 700),
            submitter_script: funding_script(),
        },
    )
    .unwrap();
    sign_funding_transaction(&mut split.transaction, &split.source_outputs).unwrap();
    assert_eq!(split.transaction.inputs.len(), 5);
    verify_both(&split).unwrap();
}

#[test]
fn lean_token_bearing_funder_is_rejected_before_token_burn() {
    let mut split = fixture(8, RootLock::P2sh20).unwrap();
    split.source_outputs[1].token = Some(Token {
        category: vec![0x55; 32],
        amount: 1,
        nft: None,
    });
    reject_both(&mut split).unwrap();
}

#[test]
fn lean_funder_immutable_nft_with_empty_commitment_is_rejected() {
    let mut split = fixture(8, RootLock::P2sh20).unwrap();
    split.source_outputs[1].token = Some(Token {
        category: vec![0x55; 32],
        amount: 0,
        nft: Some(Nft {
            capability: NonFungibleTokenCapability::None,
            commitment: Vec::with_capacity(0),
        }),
    });
    reject_both(&mut split).unwrap();
}

#[test]
fn lean_32_payees_both_modes() {
    verify_both(&fixture(32, RootLock::P2sh20).unwrap()).unwrap();
}

#[test]
fn lean_64_payees_both_modes() {
    verify_both(&fixture(64, RootLock::P2sh20).unwrap()).unwrap();
}

#[test]
fn lean_100_payees_both_modes() {
    verify_both(&fixture(100, RootLock::P2sh20).unwrap()).unwrap();
}

#[test]
fn lean_p2sh32_root_both_modes() {
    verify_both(&fixture(8, RootLock::P2sh32).unwrap()).unwrap();
}

#[test]
fn lean_wrong_payee_script_fails() {
    let mut split = fixture(8, RootLock::P2sh20).unwrap();
    split.transaction.outputs[0].locking_bytecode = beneficiary_script(999);
    reject_both(&mut split).unwrap();
}

#[test]
fn lean_wrong_amount_plus_one_fails() {
    let mut split = fixture(8, RootLock::P2sh20).unwrap();
    split.transaction.outputs[0].token.as_mut().unwrap().amount += 1;
    reject_both(&mut split).unwrap();
}

#[test]
fn lean_wrong_amount_minus_one_burn_fails() {
    let mut split = fixture(8, RootLock::P2sh20).unwrap();
    split.transaction.outputs[0].token.as_mut().unwrap().amount -= 1;
    reject_both(&mut split).unwrap();
}

#[test]
fn lean_wrong_order_fails() {
    let mut split = fixture(8, RootLock::P2sh20).unwrap();
    split.transaction.outputs.swap(0, 1);
    reject_both(&mut split).unwrap();
}

#[test]
fn lean_extra_output_fails() {
    let mut split = fixture(8, RootLock::P2sh20).unwrap();
    split
        .transaction
        .outputs
        .push(split.transaction.outputs[0].clone());
    reject_both(&mut split).unwrap();
}

#[test]
fn lean_missing_payee_output_fails() {
    let mut split = fixture(8, RootLock::P2sh20).unwrap();
    split.transaction.outputs.remove(0);
    reject_both(&mut split).unwrap();
}

#[test]
fn lean_wrong_category_fails() {
    let mut split = fixture(8, RootLock::P2sh20).unwrap();
    split.transaction.outputs[0]
        .token
        .as_mut()
        .unwrap()
        .category[0] ^= 1;
    reject_both(&mut split).unwrap();
}

#[test]
fn lean_submitter_takes_extra_token_fails() {
    let mut split = fixture(8, RootLock::P2sh20).unwrap();
    split.transaction.outputs[8].token.as_mut().unwrap().amount += 1;
    split.transaction.outputs[0].token.as_mut().unwrap().amount -= 1;
    reject_both(&mut split).unwrap();
}

#[test]
fn lean_token_change_fails() {
    let mut split = fixture(8, RootLock::P2sh20).unwrap();
    split.transaction.outputs.last_mut().unwrap().token = Some(Token {
        category: vec![0x22; 32],
        amount: 1,
        nft: None,
    });
    split.transaction.outputs[0].token.as_mut().unwrap().amount -= 1;
    reject_both(&mut split).unwrap();
}

#[test]
fn lean_version_one_fails() {
    let mut split = fixture(8, RootLock::P2sh20).unwrap();
    split.transaction.version = 1;
    reject_both(&mut split).unwrap();
}

#[test]
fn lean_nonzero_locktime_fails() {
    let mut split = fixture(8, RootLock::P2sh20).unwrap();
    split.transaction.locktime = 1;
    reject_both(&mut split).unwrap();
}

#[test]
fn lean_nonfinal_active_sequence_fails() {
    let mut split = fixture(8, RootLock::P2sh20).unwrap();
    split.transaction.inputs[0].sequence_number -= 1;
    reject_both(&mut split).unwrap();
}

#[test]
fn lean_nonfinal_funding_sequence_fails() {
    let mut split = fixture(8, RootLock::P2sh20).unwrap();
    split.transaction.inputs[1].sequence_number -= 1;
    reject_both(&mut split).unwrap();
}

#[test]
fn lean_source_reward_changed_fails() {
    let mut split = fixture(8, RootLock::P2sh20).unwrap();
    split.source_outputs[0].token.as_mut().unwrap().amount -= 1;
    reject_both(&mut split).unwrap();
}

#[test]
fn lean_binding_changed_redeem_fails() {
    let mut split = fixture(8, RootLock::P2sh20).unwrap();
    split.redeem[43] ^= 1;
    let submitter = split.transaction.outputs[8].locking_bytecode.clone();
    let mut unlocking = encode_data_push(&submitter);
    unlocking.extend_from_slice(&encode_data_push(&split.redeem));
    split.transaction.inputs[0].unlocking_bytecode = unlocking;
    reject_both(&mut split).unwrap();
}

#[test]
fn lean_short_list_insufficient_coverage_fails_closed() {
    assert!(manifest(1).allocate(21 * (1 << 32)).is_err());
}

#[test]
fn lean_clipped_last_below_minimum_fails_closed() {
    assert!(manifest(2).allocate(21 * (1 << 32)).is_err());
}

#[test]
fn lean_exact_minimum_gross_not_net() {
    assert_eq!(
        manifest(1).allocate(20 * (1 << 32)).unwrap(),
        vec![20 * (1 << 32)]
    );
}

#[test]
fn lean_annotations_match_checked_source() {
    assert_eq!(
        generated_source(ListOrder::MostOwed),
        include_str!("../lean-payout.cash")
    );
}

#[test]
fn lean_share_order_annotations_match_checked_source() {
    assert_eq!(
        generated_source(ListOrder::ShareChain),
        include_str!("../lean-payout-share-order.cash")
    );
}

/// First-eligible order: smaller debts ahead of larger ones, not most owed.
fn share_order_manifest(count: usize) -> p2poolv2_photon_payout::lean::Manifest {
    let mut listed = manifest(count);
    listed.order = ListOrder::ShareChain;
    for (index, entry) in listed.entries.iter_mut().enumerate() {
        entry.owed = (20 + index as u64) * FIXTURE_COST;
    }
    listed
}

#[test]
fn lean_share_order_ascending_debts_pass_both_modes() {
    let listed = share_order_manifest(8);
    let reward = listed.entries.iter().map(|entry| entry.owed).sum();
    verify_both(&build_fixture(&listed, reward, RootLock::P2sh20).unwrap()).unwrap();
}

#[test]
fn lean_most_owed_rejects_share_order_list() {
    let mut listed = share_order_manifest(8);
    listed.order = ListOrder::MostOwed;
    let reward = listed.entries.iter().map(|entry| entry.owed).sum();
    assert!(build_fixture(&listed, reward, RootLock::P2sh20).is_err());
}

#[test]
fn lean_share_order_clips_last_committed_entry() {
    let listed = share_order_manifest(3);
    let reward = (20 + 21 + 20) * FIXTURE_COST;
    let split = build_fixture(&listed, reward, RootLock::P2sh20).unwrap();
    assert_eq!(
        split.gross_payments,
        vec![20 * FIXTURE_COST, 21 * FIXTURE_COST, 20 * FIXTURE_COST]
    );
    verify_both(&split).unwrap();
}

#[test]
fn lean_share_order_swapped_outputs_fail() {
    let listed = share_order_manifest(8);
    let reward = listed.entries.iter().map(|entry| entry.owed).sum();
    let mut split = build_fixture(&listed, reward, RootLock::P2sh20).unwrap();
    split.transaction.outputs.swap(0, 1);
    reject_both(&mut split).unwrap();
}

#[test]
fn lean_share_order_version_one_header_fails() {
    let listed = share_order_manifest(8);
    let reward = listed.entries.iter().map(|entry| entry.owed).sum();
    let mut split = build_fixture(&listed, reward, RootLock::P2sh20).unwrap();
    let mut redeem = split.redeem.clone();
    // OP_PUSHDATA1 and its length precede the little-endian version word.
    assert_eq!(redeem[2], 2);
    redeem[2] = 1;
    replace_redeem(&mut split, redeem, true);
    reject_both(&mut split).unwrap();
}

#[test]
fn lean_share_order_64_payees_both_modes() {
    let listed = share_order_manifest(64);
    let reward = listed.entries.iter().map(|entry| entry.owed).sum();
    verify_both(&build_fixture(&listed, reward, RootLock::P2sh20).unwrap()).unwrap();
}

#[test]
fn lean_maximum_35_byte_profile_187_payees_passes() {
    verify_both(&fixture(187, RootLock::P2sh20).unwrap()).unwrap();
}

#[test]
fn lean_next_35_byte_profile_exceeds_unlock_limit() {
    assert!(
        fixture(188, RootLock::P2sh20)
            .unwrap_err()
            .contains("10000")
    );
}

#[test]
fn lean_short_script_density_limit_fails_in_both_vms() {
    let mut split = profile_fixture(84, 23).unwrap();
    assert!(
        evaluate_input(&split.transaction, &split.source_outputs, 0, true)
            .unwrap_err()
            .contains("operation cost")
    );
    assert!(
        evaluate_input(&split.transaction, &split.source_outputs, 0, false)
            .unwrap_err()
            .contains("operation cost")
    );
    reject_both(&mut split).unwrap();
}

#[test]
fn lean_unlock_above_10000_rejected_by_actual_vms() {
    let mut split = fixture(187, RootLock::P2sh20).unwrap();
    let mut redeem = split.redeem.clone();
    redeem.extend_from_slice(&[0x61; 25]);
    replace_redeem(&mut split, redeem, true);
    assert!(split.transaction.inputs[0].unlocking_bytecode.len() > 10_000);
    reject_both(&mut split).unwrap();
}

#[test]
fn lean_internal_item_above_10000_rejected_by_actual_vms() {
    let mut split = fixture(1, RootLock::P2sh20).unwrap();
    let mut redeem = split.redeem.clone();
    // 1 << 80000 would be a 10001-byte stack item; no oversized unlock.
    redeem.extend_from_slice(&[0x51, 0x03, 0x80, 0x38, 0x01, 0x8d, 0x75]);
    replace_redeem(&mut split, redeem, true);
    assert!(
        evaluate_input(&split.transaction, &split.source_outputs, 0, true)
            .unwrap_err()
            .contains("length")
    );
    reject_both(&mut split).unwrap();
}

#[test]
fn lean_artificial_opcost_exhaustion_rejected_by_actual_vms() {
    let mut split = fixture(1, RootLock::P2sh20).unwrap();
    let mut redeem = split.redeem.clone();
    // Repeated legal 10000-byte temporary items exhaust density, not size.
    redeem.extend_from_slice(&[0x51, 0x03, 0x7e, 0x38, 0x01, 0x8d, 0x75].repeat(1000));
    replace_redeem(&mut split, redeem, true);
    assert!(split.transaction.inputs[0].unlocking_bytecode.len() <= 10_000);
    assert!(
        evaluate_input(&split.transaction, &split.source_outputs, 0, true)
            .unwrap_err()
            .contains("operation cost")
    );
    reject_both(&mut split).unwrap();
}

#[test]
fn lean_unused_suffix_does_not_repay_submitter_for_unpaid_candidates() {
    let split = build_fixture(&manifest(8), 20 * FIXTURE_COST, RootLock::P2sh20).unwrap();
    assert_eq!(split.gross_payments.len(), 1);
    assert_eq!(
        split.transaction.outputs[1].token.as_ref().unwrap().amount,
        FIXTURE_COST
    );
    verify_both(&split).unwrap();
}

#[test]
fn lean_positive_last_clipping_at_minimum() {
    let mut listed = manifest(2);
    listed.entries[1].multiple = 10;
    let split = build_fixture(&listed, 30 * FIXTURE_COST, RootLock::P2sh20).unwrap();
    assert_eq!(
        split.gross_payments,
        vec![20 * FIXTURE_COST, 10 * FIXTURE_COST]
    );
    verify_both(&split).unwrap();
}

#[test]
fn lean_relocked_multiplier_below_ten_rejected_by_covenant() {
    let mut split = fixture(1, RootLock::P2sh20).unwrap();
    let mut redeem = split.redeem.clone();
    redeem[131] = 9;
    replace_redeem(&mut split, redeem, true);
    reject_both(&mut split).unwrap();
}

#[test]
fn lean_relocked_insufficient_coverage_rejected_by_covenant() {
    let mut split = fixture(1, RootLock::P2sh20).unwrap();
    split.source_outputs[0].token.as_mut().unwrap().amount += 1;
    reject_both(&mut split).unwrap();
}

#[test]
fn lean_clipped_final_below_minimum_rejected_by_covenant() {
    let mut split = fixture(2, RootLock::P2sh20).unwrap();
    split.source_outputs[0].token.as_mut().unwrap().amount = 21 * FIXTURE_COST;
    reject_both(&mut split).unwrap();
}

#[test]
fn lean_mixed_207_payees_both_modes() {
    verify_both(
        &build_fixture(
            &mixed_profile(207).unwrap(),
            207 * 20 * FIXTURE_COST,
            RootLock::P2sh20,
        )
        .unwrap(),
    )
    .unwrap();
}

#[test]
fn lean_mixed_208_standard_density_limit_nonstandard_passes() {
    let split = build_fixture(
        &mixed_profile(208).unwrap(),
        208 * 20 * FIXTURE_COST,
        RootLock::P2sh20,
    )
    .unwrap();
    assert!(
        evaluate_input(&split.transaction, &split.source_outputs, 0, true)
            .unwrap_err()
            .contains("operation cost")
    );
    evaluate_input(&split.transaction, &split.source_outputs, 0, false).unwrap();
}

#[test]
fn lean_binding_parent_is_committed() {
    let binding = proof_binding();
    let mut changed = binding.clone();
    changed.parent[0] ^= 1;
    assert_ne!(binding.digest().unwrap(), changed.digest().unwrap());
}

#[test]
fn lean_binding_baton_is_committed() {
    let binding = proof_binding();
    let mut changed = binding.clone();
    changed.baton_outpoint[0] ^= 1;
    assert_ne!(binding.digest().unwrap(), changed.digest().unwrap());
}

#[test]
fn lean_binding_metadata_is_committed() {
    let binding = proof_binding();
    let mut changed = binding.clone();
    changed.metadata_hash[0] ^= 1;
    assert_ne!(binding.digest().unwrap(), changed.digest().unwrap());
}

#[test]
fn lean_binding_window_is_committed() {
    let binding = proof_binding();
    let mut changed = binding.clone();
    changed.window_commitment[0] ^= 1;
    assert_ne!(binding.digest().unwrap(), changed.digest().unwrap());
}

#[test]
fn lean_binding_reserved_ledger_is_committed() {
    let binding = proof_binding();
    let mut changed = binding.clone();
    changed.ledger_commitment[0] ^= 1;
    assert_ne!(binding.digest().unwrap(), changed.digest().unwrap());
}

#[test]
fn lean_binding_deployment_is_committed() {
    let binding = proof_binding();
    let mut changed = binding.clone();
    changed.deployment_lock_hash[0] ^= 1;
    assert_ne!(binding.digest().unwrap(), changed.digest().unwrap());
}

#[test]
fn lean_binding_exact_seed_is_committed() {
    let binding = proof_binding();
    let mut changed = binding.clone();
    changed.proof_seed[100] ^= 1;
    assert_ne!(binding.digest().unwrap(), changed.digest().unwrap());
}

#[test]
fn lean_binding_seed_length_is_committed() {
    let binding = proof_binding();
    let mut changed = binding.clone();
    changed.proof_seed.push(0);
    assert_ne!(binding.digest().unwrap(), changed.digest().unwrap());
}

#[test]
fn lean_largest_measured_209_low_cost_unequal_payees_both_modes() {
    verify_both(&small_cost_fixture(209, true).unwrap()).unwrap();
}

#[test]
fn lean_next_210_low_cost_unequal_payees_density_rejected() {
    let mut split = small_cost_fixture(210, true).unwrap();
    reject_both(&mut split).unwrap();
}

#[test]
fn lean_claim_amount_layout_209_payees_pass_both_modes() {
    verify_both(&claim_amount_fixture(209).unwrap()).unwrap();
}

#[test]
fn lean_claim_amount_layout_210_density_rejected_both_modes() {
    let mut split = claim_amount_fixture(210).unwrap();
    reject_both(&mut split).unwrap();
}
