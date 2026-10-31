// SPDX-License-Identifier: AGPL-3.0-or-later

#![cfg(feature = "test-utils")]

use libauth::runtime::bigint::BigInt;
use libauth::{Nft, NonFungibleTokenCapability, Token, sha256};
use p2poolv2_photon_payout::reference::{Beneficiary, Manifest, maximum_window_weight};
use p2poolv2_photon_payout::state::{RootLock, State, script_hash_lock};
use p2poolv2_photon_payout::test_utils::{
    beneficiary_script, equal_tree, funding_input, internal_fixture, mutate_state_byte,
    rebind_raw_state, reject_both, replace_redeem, replace_witness, root_spend,
    sign_synthetic_funding, spend_options, terminal_fixture, terminal_reveal, verify_both,
};
use p2poolv2_photon_payout::transaction::build_spend;
use p2poolv2_photon_payout::vm::{evaluate_input, measure_input, verify_transaction};

#[test]
fn a1_p2sh20_root_internal_four_funding_inputs_and_change() {
    let spend = root_spend(&equal_tree(9).unwrap(), i64::MAX as u64, RootLock::P2sh20).unwrap();
    assert_eq!(spend.transaction.inputs.len(), 5);
    assert_eq!(spend.transaction.outputs.len(), 3);
    assert!(spend.transaction.outputs[2].token.is_none());
    verify_both(&spend).unwrap();
}

#[test]
fn a1_p2sh32_root_internal_four_funding_inputs_and_change() {
    let spend = internal_fixture().unwrap();
    assert_eq!(spend.transaction.inputs.len(), 5);
    assert_eq!(spend.transaction.outputs.len(), 3);
    verify_both(&spend).unwrap();
}

#[test]
fn a3_eight_maximum_scripts_wide_weights_maximum_reward() {
    let per_leaf = maximum_window_weight().shr(3).sub(&BigInt::from_u64(1));
    let tree = Manifest::canonicalize(vec![
        Beneficiary {
            script: beneficiary_script(0),
            weight: per_leaf.clone(),
        },
        Beneficiary {
            script: beneficiary_script(1),
            weight: per_leaf.clone(),
        },
        Beneficiary {
            script: beneficiary_script(2),
            weight: per_leaf.clone(),
        },
        Beneficiary {
            script: beneficiary_script(3),
            weight: per_leaf.clone(),
        },
        Beneficiary {
            script: beneficiary_script(4),
            weight: per_leaf.clone(),
        },
        Beneficiary {
            script: beneficiary_script(5),
            weight: per_leaf.clone(),
        },
        Beneficiary {
            script: beneficiary_script(6),
            weight: per_leaf.clone(),
        },
        Beneficiary {
            script: beneficiary_script(7),
            weight: per_leaf,
        },
    ])
    .unwrap()
    .tree()
    .unwrap();
    let spend = root_spend(&tree, i64::MAX as u64, RootLock::P2sh32).unwrap();
    assert_eq!(spend.transaction.inputs.len(), 5);
    assert_eq!(spend.transaction.outputs.len(), 9);
    assert!(spend.transaction.outputs[0].token.as_ref().unwrap().amount > u32::MAX as u64);
    verify_both(&spend).unwrap();
}

#[test]
fn a4_single_leaf_root_terminal_p2sh20() {
    let spend = root_spend(&equal_tree(1).unwrap(), 1, RootLock::P2sh20).unwrap();
    assert_eq!(spend.state.depth, 0);
    assert!(spend.children.is_empty());
    verify_both(&spend).unwrap();
}

#[test]
fn a4_single_leaf_root_terminal_p2sh32() {
    let spend = root_spend(&equal_tree(1).unwrap(), i64::MAX as u64, RootLock::P2sh32).unwrap();
    verify_both(&spend).unwrap();
}

#[test]
fn a4_root_terminal_eight_leaves() {
    let spend = terminal_fixture().unwrap();
    assert_eq!(spend.state.depth, 0);
    verify_both(&spend).unwrap();
}

#[test]
fn a6_reward_one_omits_all_but_one_leaf() {
    let spend = root_spend(&equal_tree(8).unwrap(), 1, RootLock::P2sh32).unwrap();
    assert_eq!(spend.transaction.outputs.len(), 2);
    assert_eq!(
        spend.transaction.outputs[0].token.as_ref().unwrap().amount,
        1
    );
    verify_both(&spend).unwrap();
}

#[test]
fn a6_reward_two_to_the_32() {
    let spend = root_spend(&equal_tree(8).unwrap(), 1u64 << 32, RootLock::P2sh32).unwrap();
    verify_both(&spend).unwrap();
}

#[test]
fn a6_reward_two_to_the_63_minus_one() {
    let spend = terminal_fixture().unwrap();
    verify_both(&spend).unwrap();
}

#[test]
fn a7_zero_allocation_child_is_omitted() {
    let spend = root_spend(&equal_tree(9).unwrap(), 1, RootLock::P2sh32).unwrap();
    assert_eq!(spend.children.len(), 1);
    assert_eq!(spend.transaction.outputs.len(), 2);
    verify_both(&spend).unwrap();
}

#[test]
fn a7_zero_allocation_leaf_is_omitted() {
    let spend = root_spend(&equal_tree(8).unwrap(), 7, RootLock::P2sh32).unwrap();
    assert_eq!(spend.transaction.outputs.len(), 8);
    verify_both(&spend).unwrap();
}

#[test]
fn a8_exact_rounding_boundary() {
    let spend = root_spend(&equal_tree(2).unwrap(), 8, RootLock::P2sh32).unwrap();
    assert_eq!(
        spend.transaction.outputs[0].token.as_ref().unwrap().amount,
        4
    );
    assert_eq!(
        spend.transaction.outputs[1].token.as_ref().unwrap().amount,
        4
    );
    verify_both(&spend).unwrap();
}

#[test]
fn a8_one_below_rounding_boundary() {
    let spend = root_spend(&equal_tree(2).unwrap(), 7, RootLock::P2sh32).unwrap();
    assert_eq!(
        spend.transaction.outputs[0].token.as_ref().unwrap().amount,
        3
    );
    assert_eq!(
        spend.transaction.outputs[1].token.as_ref().unwrap().amount,
        4
    );
    verify_both(&spend).unwrap();
}

#[test]
fn a8_one_above_rounding_boundary() {
    let spend = root_spend(&equal_tree(2).unwrap(), 9, RootLock::P2sh32).unwrap();
    assert_eq!(
        spend.transaction.outputs[0].token.as_ref().unwrap().amount,
        4
    );
    assert_eq!(
        spend.transaction.outputs[1].token.as_ref().unwrap().amount,
        5
    );
    verify_both(&spend).unwrap();
}

#[test]
fn positive_aggregate_excess_may_move_between_allocations() {
    let tree = equal_tree(2).unwrap();
    let state = State::root(&tree, 0, [0x22; 32], [0x44; 32]).unwrap();
    let mut spend = build_spend(
        &state,
        &tree,
        8,
        &spend_options(RootLock::P2sh32, 1_000_000),
    )
    .unwrap();
    spend.transaction.outputs[0].value_satoshis = 705;
    spend.transaction.outputs[1].value_satoshis = 999_295;
    sign_synthetic_funding(&mut spend).unwrap();
    verify_both(&spend).unwrap();
}

#[test]
fn positive_terminal_all_three_supported_script_templates() {
    let tree = Manifest::canonicalize(vec![
        Beneficiary {
            script: hex::decode("76a914111111111111111111111111111111111111111188ac").unwrap(),
            weight: BigInt::from_u64(1),
        },
        Beneficiary {
            script: hex::decode("a914222222222222222222222222222222222222222287").unwrap(),
            weight: BigInt::from_u64(1),
        },
        Beneficiary {
            script: beneficiary_script(0),
            weight: BigInt::from_u64(1),
        },
    ])
    .unwrap()
    .tree()
    .unwrap();
    let spend = root_spend(&tree, 3, RootLock::P2sh32).unwrap();
    assert_eq!(spend.transaction.outputs[0].value_satoshis, 675);
    assert_eq!(spend.transaction.outputs[1].value_satoshis, 669);
    assert_eq!(spend.transaction.outputs[2].value_satoshis, 705);
    verify_both(&spend).unwrap();
}

#[test]
fn positive_one_signed_funding_input_without_change() {
    let tree = equal_tree(9).unwrap();
    let state = State::root(&tree, 0, [0x22; 32], [0x44; 32]).unwrap();
    let mut options = spend_options(RootLock::P2sh32, 700);
    options.funding.truncate(1);
    options.change_script = None;
    let preliminary = build_spend(&state, &tree, 9, &options).unwrap();
    options.funding[0].source.value_satoshis = preliminary.size as u64 + 710;
    let mut spend = build_spend(&state, &tree, 9, &options).unwrap();
    sign_synthetic_funding(&mut spend).unwrap();
    assert_eq!(spend.transaction.inputs.len(), 2);
    assert_eq!(spend.transaction.outputs.len(), 2);
    assert_eq!(spend.fee, spend.size as u64);
    verify_both(&spend).unwrap();
}

#[test]
fn positive_terminal_child_may_have_remaining_depth_above_zero() {
    let parent = root_spend(&equal_tree(17).unwrap(), 17, RootLock::P2sh32).unwrap();
    let child = &parent.children[0];
    assert_eq!(child.node.leaf_count, 8);
    assert_eq!(child.state.depth, 1);
    let mut spend = build_spend(
        &child.state,
        &child.node,
        parent.reward,
        &spend_options(
            RootLock::P2sh32,
            parent.transaction.outputs[child.output_index].value_satoshis,
        ),
    )
    .unwrap();
    sign_synthetic_funding(&mut spend).unwrap();
    verify_both(&spend).unwrap();
}

#[test]
fn d_vm_trace_reports_metrics_and_limits_from_actual_unlocking_length() {
    let spend = terminal_fixture().unwrap();
    let standard = measure_input(&spend.transaction, &spend.source_outputs, 0, true).unwrap();
    let consensus = measure_input(&spend.transaction, &spend.source_outputs, 0, false).unwrap();
    assert_eq!(
        standard.operation_cost_limit,
        (800 * (41 + standard.unlocking_length)) as f64
    );
    assert_eq!(
        standard.hash_iteration_limit,
        ((41 + standard.unlocking_length) / 2) as f64
    );
    assert_eq!(
        consensus.hash_iteration_limit,
        (7 * (41 + standard.unlocking_length) / 2) as f64
    );
    assert!(standard.operation_cost > 0.0);
    assert!(standard.hash_iterations > 0.0);
    assert!(standard.peak_memory_slots < 1_000);
    assert!(standard.peak_control_depth < 100);
    assert!(standard.maximum_stack_item <= 10_000);
}

#[test]
fn b_internal_output_amount_increased_one() {
    let mut spend = internal_fixture().unwrap();
    spend.transaction.outputs[0].token.as_mut().unwrap().amount += 1;
    reject_both(&spend).unwrap();
}

#[test]
fn b_internal_output_amount_decreased_one_burn() {
    let mut spend = internal_fixture().unwrap();
    spend.transaction.outputs[0].token.as_mut().unwrap().amount -= 1;
    reject_both(&spend).unwrap();
}

#[test]
fn b_internal_second_output_amount_increased_one() {
    let mut spend = internal_fixture().unwrap();
    spend.transaction.outputs[1].token.as_mut().unwrap().amount += 1;
    reject_both(&spend).unwrap();
}

#[test]
fn b_internal_second_output_amount_decreased_one() {
    let mut spend = internal_fixture().unwrap();
    spend.transaction.outputs[1].token.as_mut().unwrap().amount -= 1;
    reject_both(&spend).unwrap();
}

#[test]
fn b_terminal_output_amount_increased_one() {
    let mut spend = terminal_fixture().unwrap();
    spend.transaction.outputs[0].token.as_mut().unwrap().amount += 1;
    reject_both(&spend).unwrap();
}

#[test]
fn b_terminal_output_amount_decreased_one_burn() {
    let mut spend = terminal_fixture().unwrap();
    spend.transaction.outputs[0].token.as_mut().unwrap().amount -= 1;
    reject_both(&spend).unwrap();
}

#[test]
fn b_terminal_output_one_amount_increased_one() {
    let mut spend = terminal_fixture().unwrap();
    spend.transaction.outputs[1].token.as_mut().unwrap().amount += 1;
    reject_both(&spend).unwrap();
}

#[test]
fn b_terminal_output_one_amount_decreased_one() {
    let mut spend = terminal_fixture().unwrap();
    spend.transaction.outputs[1].token.as_mut().unwrap().amount -= 1;
    reject_both(&spend).unwrap();
}

#[test]
fn b_terminal_output_two_amount_increased_one() {
    let mut spend = terminal_fixture().unwrap();
    spend.transaction.outputs[2].token.as_mut().unwrap().amount += 1;
    reject_both(&spend).unwrap();
}

#[test]
fn b_terminal_output_two_amount_decreased_one() {
    let mut spend = terminal_fixture().unwrap();
    spend.transaction.outputs[2].token.as_mut().unwrap().amount -= 1;
    reject_both(&spend).unwrap();
}

#[test]
fn b_terminal_output_three_amount_increased_one() {
    let mut spend = terminal_fixture().unwrap();
    spend.transaction.outputs[3].token.as_mut().unwrap().amount += 1;
    reject_both(&spend).unwrap();
}

#[test]
fn b_terminal_output_three_amount_decreased_one() {
    let mut spend = terminal_fixture().unwrap();
    spend.transaction.outputs[3].token.as_mut().unwrap().amount -= 1;
    reject_both(&spend).unwrap();
}

#[test]
fn b_terminal_output_four_amount_increased_one() {
    let mut spend = terminal_fixture().unwrap();
    spend.transaction.outputs[4].token.as_mut().unwrap().amount += 1;
    reject_both(&spend).unwrap();
}

#[test]
fn b_terminal_output_four_amount_decreased_one() {
    let mut spend = terminal_fixture().unwrap();
    spend.transaction.outputs[4].token.as_mut().unwrap().amount -= 1;
    reject_both(&spend).unwrap();
}

#[test]
fn b_terminal_output_five_amount_increased_one() {
    let mut spend = terminal_fixture().unwrap();
    spend.transaction.outputs[5].token.as_mut().unwrap().amount += 1;
    reject_both(&spend).unwrap();
}

#[test]
fn b_terminal_output_five_amount_decreased_one() {
    let mut spend = terminal_fixture().unwrap();
    spend.transaction.outputs[5].token.as_mut().unwrap().amount -= 1;
    reject_both(&spend).unwrap();
}

#[test]
fn b_terminal_output_six_amount_increased_one() {
    let mut spend = terminal_fixture().unwrap();
    spend.transaction.outputs[6].token.as_mut().unwrap().amount += 1;
    reject_both(&spend).unwrap();
}

#[test]
fn b_terminal_output_six_amount_decreased_one() {
    let mut spend = terminal_fixture().unwrap();
    spend.transaction.outputs[6].token.as_mut().unwrap().amount -= 1;
    reject_both(&spend).unwrap();
}

#[test]
fn b_terminal_output_seven_amount_increased_one() {
    let mut spend = terminal_fixture().unwrap();
    spend.transaction.outputs[7].token.as_mut().unwrap().amount += 1;
    reject_both(&spend).unwrap();
}

#[test]
fn b_terminal_output_seven_amount_decreased_one() {
    let mut spend = terminal_fixture().unwrap();
    spend.transaction.outputs[7].token.as_mut().unwrap().amount -= 1;
    reject_both(&spend).unwrap();
}

#[test]
fn b_token_change_instead_of_complete_allocation() {
    let mut spend = internal_fixture().unwrap();
    spend.transaction.outputs[0].token.as_mut().unwrap().amount -= 1;
    spend.transaction.outputs[2].token = Some(Token {
        category: vec![0x22; 32],
        amount: 1,
        nft: None,
    });
    reject_both(&spend).unwrap();
}

#[test]
fn b_extra_token_output() {
    let mut spend = internal_fixture().unwrap();
    spend
        .transaction
        .outputs
        .push(spend.transaction.outputs[0].clone());
    reject_both(&spend).unwrap();
}

#[test]
fn b_wrong_output_category() {
    let mut spend = internal_fixture().unwrap();
    spend.transaction.outputs[0]
        .token
        .as_mut()
        .unwrap()
        .category[0] ^= 1;
    reject_both(&spend).unwrap();
}

#[test]
fn b_new_genesis_category_on_change() {
    let mut spend = internal_fixture().unwrap();
    spend.transaction.inputs[1].outpoint_index = 0;
    spend.transaction.outputs[2].token = Some(Token {
        category: spend.transaction.inputs[1]
            .outpoint_transaction_hash
            .clone(),
        amount: 1,
        nft: None,
    });
    reject_both(&spend).unwrap();
}

#[test]
fn b_funding_immutable_nft_empty_commitment() {
    let mut spend = internal_fixture().unwrap();
    spend.source_outputs[1].token = Some(Token {
        category: vec![0x55; 32],
        amount: 0,
        nft: Some(Nft {
            capability: NonFungibleTokenCapability::None,
            commitment: vec![],
        }),
    });
    reject_both(&spend).unwrap();
}

#[test]
fn b_funding_mutable_nft() {
    let mut spend = internal_fixture().unwrap();
    spend.source_outputs[1].token = Some(Token {
        category: vec![0x55; 32],
        amount: 0,
        nft: Some(Nft {
            capability: NonFungibleTokenCapability::Mutable,
            commitment: vec![1],
        }),
    });
    reject_both(&spend).unwrap();
}

#[test]
fn b_funding_minting_nft() {
    let mut spend = internal_fixture().unwrap();
    spend.source_outputs[1].token = Some(Token {
        category: vec![0x55; 32],
        amount: 0,
        nft: Some(Nft {
            capability: NonFungibleTokenCapability::Minting,
            commitment: vec![1],
        }),
    });
    reject_both(&spend).unwrap();
}

#[test]
fn b_funding_photon_fungible_tokens() {
    let mut spend = internal_fixture().unwrap();
    spend.source_outputs[1].token = Some(Token {
        category: vec![0x22; 32],
        amount: 1,
        nft: None,
    });
    reject_both(&spend).unwrap();
}

#[test]
fn b_funding_other_fungible_tokens() {
    let mut spend = internal_fixture().unwrap();
    spend.source_outputs[4].token = Some(Token {
        category: vec![0x55; 32],
        amount: 1,
        nft: None,
    });
    reject_both(&spend).unwrap();
}

#[test]
fn b_second_payout_input() {
    let mut spend = internal_fixture().unwrap();
    spend.source_outputs[1] = spend.source_outputs[0].clone();
    spend.transaction.inputs[1].unlocking_bytecode =
        spend.transaction.inputs[0].unlocking_bytecode.clone();
    reject_both(&spend).unwrap();
}

#[test]
fn b_active_input_not_index_zero() {
    let mut spend = internal_fixture().unwrap();
    spend.transaction.inputs.swap(0, 1);
    spend.source_outputs.swap(0, 1);
    assert!(evaluate_input(&spend.transaction, &spend.source_outputs, 1, true).is_err());
    assert!(evaluate_input(&spend.transaction, &spend.source_outputs, 1, false).is_err());
    assert!(verify_transaction(&spend.transaction, &spend.source_outputs, true).is_err());
    assert!(verify_transaction(&spend.transaction, &spend.source_outputs, false).is_err());
}

#[test]
fn b_only_one_total_input() {
    let mut spend = internal_fixture().unwrap();
    spend.transaction.inputs.truncate(1);
    spend.source_outputs.truncate(1);
    reject_both(&spend).unwrap();
}

#[test]
fn b_six_total_inputs() {
    let mut spend = internal_fixture().unwrap();
    let extra = funding_input(5, 10_000_000);
    spend.transaction.inputs.push(extra.input);
    spend.source_outputs.push(extra.source);
    reject_both(&spend).unwrap();
}

#[test]
fn b_unvalidated_extra_output() {
    let mut spend = internal_fixture().unwrap();
    spend
        .transaction
        .outputs
        .push(spend.transaction.outputs[2].clone());
    reject_both(&spend).unwrap();
}

#[test]
fn b_change_not_last() {
    let mut spend = internal_fixture().unwrap();
    spend.transaction.outputs.swap(1, 2);
    reject_both(&spend).unwrap();
}

#[test]
fn b_two_change_outputs() {
    let mut spend = internal_fixture().unwrap();
    let mut change = spend.transaction.outputs[2].clone();
    change.value_satoshis = 546;
    spend.transaction.outputs.push(change);
    reject_both(&spend).unwrap();
}

#[test]
fn b_version_one() {
    let mut spend = internal_fixture().unwrap();
    spend.transaction.version = 1;
    reject_both(&spend).unwrap();
}

#[test]
fn b_nonzero_locktime() {
    let mut spend = internal_fixture().unwrap();
    spend.transaction.locktime = 1;
    reject_both(&spend).unwrap();
}

#[test]
fn b_nonfinal_distribution_sequence() {
    let mut spend = internal_fixture().unwrap();
    spend.transaction.inputs[0].sequence_number -= 1;
    reject_both(&spend).unwrap();
}

#[test]
fn b_nonfinal_last_funding_sequence() {
    let mut spend = internal_fixture().unwrap();
    spend.transaction.inputs[4].sequence_number -= 1;
    reject_both(&spend).unwrap();
}

#[test]
fn b_allocation_below_committed_bch_minimum() {
    let mut spend = internal_fixture().unwrap();
    spend.transaction.outputs[0].value_satoshis = 704;
    reject_both(&spend).unwrap();
}

#[test]
fn b_inherited_bch_peeled_to_change() {
    let tree = equal_tree(9).unwrap();
    let state = State::root(&tree, 0, [0x22; 32], [0x44; 32]).unwrap();
    let mut spend = build_spend(
        &state,
        &tree,
        9,
        &spend_options(RootLock::P2sh32, 1_000_000),
    )
    .unwrap();
    spend.transaction.outputs[0].value_satoshis -= 1;
    spend.transaction.outputs[2].value_satoshis += 1;
    reject_both(&spend).unwrap();
}

#[test]
fn b_inherited_bch_peeled_to_fee() {
    let tree = equal_tree(9).unwrap();
    let state = State::root(&tree, 0, [0x22; 32], [0x44; 32]).unwrap();
    let mut spend = build_spend(
        &state,
        &tree,
        9,
        &spend_options(RootLock::P2sh32, 1_000_000),
    )
    .unwrap();
    spend.transaction.outputs[0].value_satoshis -= 1;
    reject_both(&spend).unwrap();
}

#[test]
fn b_witness_supplied_child_lock() {
    let mut spend = internal_fixture().unwrap();
    spend.transaction.outputs[0].locking_bytecode = beneficiary_script(999);
    reject_both(&spend).unwrap();
}

// Each field has its own immutable-address binding regression. These mutate
// the redeem preimage while keeping the funded P2SH lock fixed, as an attacker
// replacing a field in the unlocking script must do.
#[test]
fn b_state_version_binding() {
    let mut spend = internal_fixture().unwrap();
    mutate_state_byte(&mut spend, 0).unwrap();
    reject_both(&spend).unwrap();
}

#[test]
fn b_state_mode_binding() {
    let mut spend = internal_fixture().unwrap();
    mutate_state_byte(&mut spend, 2).unwrap();
    reject_both(&spend).unwrap();
}

#[test]
fn b_state_depth_binding() {
    let mut spend = internal_fixture().unwrap();
    mutate_state_byte(&mut spend, 3).unwrap();
    reject_both(&spend).unwrap();
}

#[test]
fn b_state_network_binding() {
    let mut spend = internal_fixture().unwrap();
    mutate_state_byte(&mut spend, 4).unwrap();
    reject_both(&spend).unwrap();
}

#[test]
fn b_state_category_binding() {
    let mut spend = internal_fixture().unwrap();
    mutate_state_byte(&mut spend, 8).unwrap();
    reject_both(&spend).unwrap();
}

#[test]
fn b_state_context_binding() {
    let mut spend = internal_fixture().unwrap();
    mutate_state_byte(&mut spend, 40).unwrap();
    reject_both(&spend).unwrap();
}

#[test]
fn b_state_program_hash_binding() {
    let mut spend = internal_fixture().unwrap();
    mutate_state_byte(&mut spend, 72).unwrap();
    reject_both(&spend).unwrap();
}

#[test]
fn b_state_subtree_hash_binding() {
    let mut spend = internal_fixture().unwrap();
    mutate_state_byte(&mut spend, 104).unwrap();
    reject_both(&spend).unwrap();
}

#[test]
fn b_state_total_weight_binding() {
    let mut spend = internal_fixture().unwrap();
    mutate_state_byte(&mut spend, 136).unwrap();
    reject_both(&spend).unwrap();
}

#[test]
fn b_state_lower_binding() {
    let mut spend = internal_fixture().unwrap();
    mutate_state_byte(&mut spend, 200).unwrap();
    reject_both(&spend).unwrap();
}

#[test]
fn b_state_upper_binding() {
    let mut spend = internal_fixture().unwrap();
    mutate_state_byte(&mut spend, 264).unwrap();
    reject_both(&spend).unwrap();
}

#[test]
fn b_state_reward_binding() {
    let mut spend = internal_fixture().unwrap();
    mutate_state_byte(&mut spend, 328).unwrap();
    reject_both(&spend).unwrap();
}

#[test]
fn b_wrong_child_depth_decrement() {
    let mut spend = internal_fixture().unwrap();
    let mut redeem = spend.children[0].state.redeem().unwrap();
    redeem[6] += 1;
    spend.transaction.outputs[0].locking_bytecode = script_hash_lock(&redeem, RootLock::P2sh32);
    reject_both(&spend).unwrap();
}

#[test]
fn b_internal_at_depth_zero_even_with_matching_source_lock() {
    let mut spend = internal_fixture().unwrap();
    let mut redeem = spend.state.redeem().unwrap();
    redeem[6] = 0;
    spend.source_outputs[0].locking_bytecode = script_hash_lock(&redeem, RootLock::P2sh32);
    replace_redeem(
        &mut spend,
        &equal_tree(9).unwrap().witness().unwrap(),
        &redeem,
    )
    .unwrap();
    reject_both(&spend).unwrap();
}

#[test]
fn b_nine_leaf_terminal_witness() {
    let mut spend = internal_fixture().unwrap();
    let witness = terminal_reveal(&spend.node).unwrap();
    replace_witness(&mut spend, &witness).unwrap();
    reject_both(&spend).unwrap();
}

#[test]
fn b_leaf_internal_domain_swap() {
    let mut spend = internal_fixture().unwrap();
    let witness = spend.node.witness().unwrap();
    let mut wrong_domain_preimage = vec![0];
    wrong_domain_preimage.extend_from_slice(&witness[2..]);
    let wrong_hash = sha256.hash(&wrong_domain_preimage);
    let mut raw = spend.state.encode().unwrap();
    raw[104..136].copy_from_slice(&wrong_hash);
    rebind_raw_state(&mut spend, &raw).unwrap();
    reject_both(&spend).unwrap();
}

#[test]
fn b_child_weight_sum_does_not_partition_interval() {
    let mut spend = internal_fixture().unwrap();
    let mut witness = spend.node.witness().unwrap();
    witness[34] += 1;
    replace_witness(&mut spend, &witness).unwrap();
    reject_both(&spend).unwrap();
}

#[test]
fn b_odd_split_larger_half_on_left() {
    let mut spend = internal_fixture().unwrap();
    let mut witness = spend.node.witness().unwrap();
    let left = witness[2..100].to_vec();
    let right = witness[100..198].to_vec();
    witness[2..100].copy_from_slice(&right);
    witness[100..198].copy_from_slice(&left);
    replace_witness(&mut spend, &witness).unwrap();
    reject_both(&spend).unwrap();
}

#[test]
fn b_zero_weight_terminal_leaf() {
    let mut spend = terminal_fixture().unwrap();
    let mut witness = spend.node.witness().unwrap();
    witness[38..102].fill(0);
    replace_witness(&mut spend, &witness).unwrap();
    reject_both(&spend).unwrap();
}

#[test]
fn b_wrong_midpoint_child_leaf_count() {
    let mut spend = internal_fixture().unwrap();
    let mut witness = spend.node.witness().unwrap();
    witness[98] += 1;
    replace_witness(&mut spend, &witness).unwrap();
    reject_both(&spend).unwrap();
}

#[test]
fn b_internal_witness_for_eight_leaf_node() {
    let mut spend = terminal_fixture().unwrap();
    let witness = internal_fixture().unwrap().node.witness().unwrap();
    replace_witness(&mut spend, &witness).unwrap();
    reject_both(&spend).unwrap();
}

#[test]
fn b_program_one_bit_flipped() {
    let mut spend = internal_fixture().unwrap();
    let mut redeem = spend.state.redeem().unwrap();
    redeem[339] ^= 1;
    let witness = spend.node.witness().unwrap();
    replace_redeem(&mut spend, &witness, &redeem).unwrap();
    reject_both(&spend).unwrap();
}

#[test]
fn b_program_code_separator_inserted() {
    let mut spend = internal_fixture().unwrap();
    let mut redeem = spend.state.redeem().unwrap();
    redeem.insert(339, 0xab);
    let witness = spend.node.witness().unwrap();
    replace_redeem(&mut spend, &witness, &redeem).unwrap();
    reject_both(&spend).unwrap();
}

#[test]
fn b_root_wrong_category_utxo() {
    let mut spend = internal_fixture().unwrap();
    spend.source_outputs[0].token.as_mut().unwrap().category[0] ^= 1;
    reject_both(&spend).unwrap();
}

#[test]
fn b_root_bch_only_utxo() {
    let mut spend = internal_fixture().unwrap();
    spend.source_outputs[0].token = None;
    reject_both(&spend).unwrap();
}

#[test]
fn b_child_utxo_wrong_prescribed_amount() {
    let parent = internal_fixture().unwrap();
    let child = &parent.children[0];
    let value = parent.transaction.outputs[child.output_index].value_satoshis;
    let mut spend = build_spend(
        &child.state,
        &child.node,
        parent.reward,
        &spend_options(RootLock::P2sh32, value),
    )
    .unwrap();
    spend.source_outputs[0].token.as_mut().unwrap().amount -= 1;
    reject_both(&spend).unwrap();
}

#[test]
fn b_unknown_version_even_with_matching_source_lock() {
    let mut spend = internal_fixture().unwrap();
    let mut raw = spend.state.encode().unwrap();
    raw[0] = 2;
    rebind_raw_state(&mut spend, &raw).unwrap();
    reject_both(&spend).unwrap();
}

#[test]
fn b_unknown_mode_even_with_matching_source_lock() {
    let mut spend = internal_fixture().unwrap();
    let mut raw = spend.state.encode().unwrap();
    raw[2] = 2;
    rebind_raw_state(&mut spend, &raw).unwrap();
    reject_both(&spend).unwrap();
}

#[test]
fn b_unknown_network_even_with_matching_source_lock() {
    let mut spend = internal_fixture().unwrap();
    let mut raw = spend.state.encode().unwrap();
    raw[4] = 5;
    rebind_raw_state(&mut spend, &raw).unwrap();
    reject_both(&spend).unwrap();
}

#[test]
fn b_root_reward_not_zero_sentinel_even_with_matching_source_lock() {
    let mut spend = internal_fixture().unwrap();
    let mut raw = spend.state.encode().unwrap();
    raw[328] = 1;
    rebind_raw_state(&mut spend, &raw).unwrap();
    reject_both(&spend).unwrap();
}

#[test]
fn b_root_interval_not_full_even_with_matching_source_lock() {
    let mut spend = internal_fixture().unwrap();
    let mut raw = spend.state.encode().unwrap();
    raw[200] = 1;
    rebind_raw_state(&mut spend, &raw).unwrap();
    reject_both(&spend).unwrap();
}

#[test]
fn b_nonminimal_root_depth_even_with_matching_source_lock() {
    let mut spend = internal_fixture().unwrap();
    let mut raw = spend.state.encode().unwrap();
    raw[3] = 2;
    rebind_raw_state(&mut spend, &raw).unwrap();
    reject_both(&spend).unwrap();
}

#[test]
fn b_wrong_program_hash_even_with_matching_source_lock() {
    let mut spend = internal_fixture().unwrap();
    let mut raw = spend.state.encode().unwrap();
    raw[72] ^= 1;
    rebind_raw_state(&mut spend, &raw).unwrap();
    reject_both(&spend).unwrap();
}
