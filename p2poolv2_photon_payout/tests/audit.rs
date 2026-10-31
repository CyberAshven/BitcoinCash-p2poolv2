// SPDX-License-Identifier: AGPL-3.0-or-later

//! Cross-checks added after an independent acceptance-coverage audit.

#![cfg(feature = "test-utils")]

use libauth::{Token, sha256, verify_transaction_tokens};

use p2poolv2_photon_payout::covenant::program;
use p2poolv2_photon_payout::reference::weight_bytes;
use p2poolv2_photon_payout::state::{RootLock, State, script_hash_lock};
use p2poolv2_photon_payout::test_utils::{
    equal_tree, internal_fixture, rebind_raw_state, reject_both, replace_redeem, root_spend,
    sign_synthetic_funding, spend_options, verify_both,
};
use p2poolv2_photon_payout::transaction::build_spend;

#[test]
fn asymmetric_category_uses_raw_wire_state_and_display_order_libauth_tokens() {
    let tree = equal_tree(2).unwrap();
    let category: [u8; 32] =
        hex::decode("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f")
            .unwrap()
            .try_into()
            .unwrap();
    let state = State::root(&tree, 0, category, [0x44; 32]).unwrap();
    let mut spend = build_spend(
        &state,
        &tree,
        1u64 << 32,
        &spend_options(RootLock::P2sh20, 700),
    )
    .unwrap();
    assert_eq!(
        hex::encode(&spend.source_outputs[0].token.as_ref().unwrap().category),
        "1f1e1d1c1b1a191817161514131211100f0e0d0c0b0a09080706050403020100"
    );
    assert_eq!(&state.encode().unwrap()[8..40], &category);
    sign_synthetic_funding(&mut spend).unwrap();
    verify_both(&spend).unwrap();
}

#[test]
fn standalone_child_mode_single_leaf_terminal_pins_original_reward() {
    // Canonical trees stop splitting at eight leaves, so their non-root
    // buckets have at least four. This is a standalone valid child-state VM
    // vector, rather than a claim that such a state arises on a canonical path.
    let tree = equal_tree(1).unwrap();
    let mut state = State::root(&tree, 0, [0x22; 32], [0x44; 32]).unwrap();
    state.mode = 1;
    state.reward = 10;
    let mut spend = build_spend(&state, &tree, 10, &spend_options(RootLock::P2sh20, 705)).unwrap();
    assert_eq!(
        &spend.source_outputs[0].locking_bytecode[..2],
        &[0xaa, 0x20]
    );
    assert_eq!(
        spend.transaction.outputs[0].token.as_ref().unwrap().amount,
        10
    );
    assert_eq!(spend.state.mode, 1);
    sign_synthetic_funding(&mut spend).unwrap();
    verify_both(&spend).unwrap();
}

#[test]
fn code_separator_before_active_bytecode_fails_even_with_rebound_program_hash_and_lock() {
    let mut spend = internal_fixture().unwrap();
    let mut altered_program = vec![0xab];
    altered_program.extend_from_slice(&program().unwrap());
    let mut state = spend.state.encode().unwrap();
    state[72..104].copy_from_slice(&sha256.hash(&altered_program));
    let mut redeem = vec![0x4d, 0x50, 0x01];
    redeem.extend_from_slice(&state);
    redeem.extend_from_slice(&altered_program);
    spend.source_outputs[0].locking_bytecode = script_hash_lock(&redeem, RootLock::P2sh32);
    let witness = spend.node.witness().unwrap();
    replace_redeem(&mut spend, &witness, &redeem).unwrap();
    sign_synthetic_funding(&mut spend).unwrap();
    reject_both(&spend).unwrap();
}

#[test]
fn valid_new_genesis_category_on_change_is_rejected_by_covenant() {
    let mut spend = internal_fixture().unwrap();
    spend.transaction.inputs[0].outpoint_index = 0;
    spend.transaction.outputs[2].token = Some(Token {
        category: spend.transaction.inputs[0]
            .outpoint_transaction_hash
            .clone(),
        amount: 1,
        nft: None,
    });
    // The token validator permits this new category: it is genuinely the
    // transaction hash of an input spending output zero. The covenant's
    // token-free change condition must reject it independently.
    verify_transaction_tokens(
        &spend.transaction,
        &spend.source_outputs,
        128, // Pinned BCH 2026 maximum token commitment length.
    )
    .unwrap();
    sign_synthetic_funding(&mut spend).unwrap();
    reject_both(&spend).unwrap();
}

#[test]
fn terminal_leaf_domain_one_commitment_is_rejected_with_matching_source_lock() {
    let tree = equal_tree(1).unwrap();
    let mut spend = root_spend(&tree, 10, RootLock::P2sh32).unwrap();
    let leaf = tree.leaves()[0];
    let mut wrong_preimage = vec![1, leaf.script.len() as u8];
    wrong_preimage.extend_from_slice(&leaf.script);
    wrong_preimage.extend_from_slice(&weight_bytes(&leaf.weight).unwrap());
    let mut state = spend.state.encode().unwrap();
    state[104..136].copy_from_slice(&sha256.hash(&wrong_preimage));
    rebind_raw_state(&mut spend, &state).unwrap();
    sign_synthetic_funding(&mut spend).unwrap();
    reject_both(&spend).unwrap();
}
