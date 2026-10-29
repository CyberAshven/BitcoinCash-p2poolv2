// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The node covenant run in the Bitcoin Cash script machine
//! (`test_utils::bch_vm`) and libauth-rs's BCH 2026 VM: honest spends pass
//! within the 2025 VM limits at relay, and every way to take money from a
//! node fails. The theft cases are built as complete transactions, not only
//! by breaking one byte of an honest spend.

use super::*;
use crate::test_utils::bch_vm::{PREFIX_TOKEN, SpendMetrics, run_p2sh32_spend_with_both_vms};
use bitcoin::absolute::LockTime;
use bitcoin::hashes::Hash;
use bitcoin::transaction::Version;
use bitcoin::{
    Amount, OutPoint, PubkeyHash, ScriptBuf, ScriptHash, Sequence, Transaction, TxIn, Txid, Witness,
};

/// P2PKH script of a payee numbered `number`.
fn p2pkh(number: u16) -> ScriptBuf {
    let mut hash = [0x11u8; 20];
    hash[..2].copy_from_slice(&number.to_le_bytes());
    ScriptBuf::new_p2pkh(&PubkeyHash::from_byte_array(hash))
}

/// P2SH32 script of a payee numbered `number`.
fn p2sh32(number: u16) -> ScriptBuf {
    let mut hash = [0x22u8; 32];
    hash[..2].copy_from_slice(&number.to_le_bytes());
    p2sh32_script(&hash)
}

/// `count` P2PKH outputs of 10,000 satoshis plus their number.
fn p2pkh_outputs(count: u16) -> Vec<TxOut> {
    (0..count)
        .map(|number| TxOut {
            value: Amount::from_sat(10_000 + u64::from(number)),
            script_pubkey: p2pkh(number),
        })
        .collect()
}

/// A node that holds its outputs plus `fee` and accepts exactly them.
fn node_for(outputs: &[TxOut], fee: u64) -> NodeCovenant {
    let paid: u64 = outputs.iter().map(|output| output.value.to_sat()).sum();
    NodeCovenant::new(paid + fee, outputs.len(), vec![outputs_hash(outputs)])
}

/// The canonical transaction spending `node` from `outpoint` into `outputs`.
fn spend(node: &NodeCovenant, outputs: &[TxOut]) -> Transaction {
    Transaction {
        version: Version(NODE_TRANSACTION_VERSION),
        lock_time: LockTime::from_consensus(NODE_LOCK_TIME),
        input: vec![TxIn {
            previous_output: OutPoint::new(Txid::from_byte_array([0x33; 32]), 1),
            script_sig: node.unlocking_script(),
            sequence: Sequence(NODE_SEQUENCE),
            witness: Witness::new(),
        }],
        output: outputs.to_vec(),
    }
}

fn run(transaction: &Transaction, node: &NodeCovenant) -> Result<SpendMetrics, String> {
    run_p2sh32_spend_with_both_vms(transaction, 0, &[node.output()])
}

// Honest spends.

#[test]
fn a_one_output_node_is_spent_by_its_canonical_transaction() {
    let outputs = p2pkh_outputs(1);
    let node = node_for(&outputs, 300);

    let metrics = run(&spend(&node, &outputs), &node).unwrap();

    assert!(metrics.operation_cost * 2 <= metrics.operation_cost_limit);
    assert!(metrics.hash_iterations * 2 <= metrics.hash_iteration_limit);
}

/// The fan-out of 500 P2PKH outputs passes the relay limits using at most
/// half of each: the filler of 8 bytes per output keeps a margin of two.
#[test]
fn a_node_of_five_hundred_outputs_uses_at_most_half_its_vm_budget() {
    let outputs = p2pkh_outputs(500);
    let node = node_for(&outputs, 21_000);

    let metrics = run(&spend(&node, &outputs), &node).unwrap();

    assert!(
        metrics.operation_cost * 2 <= metrics.operation_cost_limit,
        "{metrics:?}"
    );
    assert!(
        metrics.hash_iterations * 2 <= metrics.hash_iteration_limit,
        "{metrics:?}"
    );
}

/// P2SH32 outputs are the largest a payee can have; 500 of them fit too.
#[test]
fn a_node_of_five_hundred_p2sh32_outputs_uses_at_most_half_its_vm_budget() {
    let outputs: Vec<TxOut> = (0..500u16)
        .map(|number| TxOut {
            value: Amount::from_sat(20_000),
            script_pubkey: p2sh32(number),
        })
        .collect();
    let node = node_for(&outputs, 26_000);

    let metrics = run(&spend(&node, &outputs), &node).unwrap();

    assert!(
        metrics.operation_cost * 2 <= metrics.operation_cost_limit,
        "{metrics:?}"
    );
    assert!(
        metrics.hash_iterations * 2 <= metrics.hash_iteration_limit,
        "{metrics:?}"
    );
}

/// Without the filler the same spend runs out of hash iterations at relay:
/// the filler is what buys the budget. The covenant commits to `Z = 0`, so
/// its filler check passes and the spend fails on the hash limit itself.
#[test]
fn without_the_filler_a_large_node_exceeds_the_relay_limits() {
    let outputs = p2pkh_outputs(500);
    let node = node_for(&outputs, 21_000).without_filler();
    let transaction = spend(&node, &outputs);

    let error = run(&transaction, &node).unwrap_err();

    assert!(error.starts_with("hash iterations "), "{error}");
    assert!(error.contains("exceed the standard limit"), "{error}");
}

#[test]
fn the_root_accepts_each_of_its_three_output_sets() {
    let at_one = p2pkh_outputs(3);
    let mut at_two = at_one.clone();
    at_two[0].value = Amount::from_sat(at_two[0].value.to_sat() - 100);
    let mut at_three = at_one.clone();
    at_three[1].value = Amount::from_sat(at_three[1].value.to_sat() - 200);
    let root = NodeCovenant::new(
        40_000,
        3,
        vec![
            outputs_hash(&at_one),
            outputs_hash(&at_two),
            outputs_hash(&at_three),
        ],
    );

    assert!(run(&spend(&root, &at_one), &root).is_ok());
    assert!(run(&spend(&root, &at_two), &root).is_ok());
    assert!(run(&spend(&root, &at_three), &root).is_ok());
}

#[test]
fn the_root_refuses_an_output_set_it_does_not_name() {
    let at_one = p2pkh_outputs(3);
    let mut other = at_one.clone();
    other[2].value = Amount::from_sat(other[2].value.to_sat() - 1);
    let root = NodeCovenant::new(
        40_000,
        3,
        vec![outputs_hash(&at_one), [0x44; 32], [0x55; 32]],
    );

    assert!(run(&spend(&root, &other), &root).is_err());
}

// Theft and tampering, each a complete transaction an executor could
// broadcast.

#[test]
fn a_payee_replaced_by_a_thief_is_refused() {
    let outputs = p2pkh_outputs(4);
    let node = node_for(&outputs, 400);
    let mut stolen = outputs.clone();
    stolen[2].script_pubkey = p2pkh(9_999);

    assert!(run(&spend(&node, &stolen), &node).is_err());
}

#[test]
fn moving_one_satoshi_between_payees_is_refused() {
    let outputs = p2pkh_outputs(4);
    let node = node_for(&outputs, 400);
    let mut moved = outputs.clone();
    moved[0].value = Amount::from_sat(moved[0].value.to_sat() - 1);
    moved[3].value = Amount::from_sat(moved[3].value.to_sat() + 1);

    assert!(run(&spend(&node, &moved), &node).is_err());
}

/// An executor keeping the fee for itself: the payees as committed, plus an
/// output paying the executor what the fee would have been.
#[test]
fn an_extra_output_for_the_executor_is_refused() {
    let outputs = p2pkh_outputs(4);
    let node = node_for(&outputs, 400);
    let mut skimmed = outputs.clone();
    skimmed.push(TxOut {
        value: Amount::from_sat(300),
        script_pubkey: p2pkh(9_999),
    });

    assert!(run(&spend(&node, &skimmed), &node).is_err());
}

#[test]
fn a_missing_payee_is_refused() {
    let outputs = p2pkh_outputs(4);
    let node = node_for(&outputs, 400);

    assert!(run(&spend(&node, &outputs[..3]), &node).is_err());
}

#[test]
fn reordered_payees_are_refused() {
    let outputs = p2pkh_outputs(4);
    let node = node_for(&outputs, 400);
    let mut reordered = outputs.clone();
    reordered.swap(0, 1);

    assert!(run(&spend(&node, &reordered), &node).is_err());
}

/// An executor adding its own input and change output: the payees are paid
/// as committed, but the transaction is not the one the share chain decided.
#[test]
fn a_second_input_is_refused() {
    let outputs = p2pkh_outputs(4);
    let node = node_for(&outputs, 400);
    let mut transaction = spend(&node, &outputs);
    transaction.input.push(TxIn {
        previous_output: OutPoint::new(Txid::from_byte_array([0x66; 32]), 0),
        script_sig: ScriptBuf::new(),
        sequence: Sequence::MAX,
        witness: Witness::new(),
    });
    let own_coin = TxOut {
        value: Amount::from_sat(5_000),
        script_pubkey: node.locking_script(),
    };

    assert!(run_p2sh32_spend_with_both_vms(&transaction, 0, &[node.output(), own_coin]).is_err());
}

/// A node that spends output 0 of its parent is a token genesis input, so an
/// executor could mint a token onto a payee's output. The covenant refuses
/// any token on any output.
#[test]
fn a_token_minted_onto_a_payee_output_is_refused() {
    let outputs = p2pkh_outputs(2);
    let node = node_for(&outputs, 200);
    let mut with_token = outputs.clone();
    let mut field = vec![PREFIX_TOKEN];
    field.extend_from_slice(&[0x33; 32]);
    field.push(0x10);
    field.push(0x01);
    field.extend_from_slice(with_token[1].script_pubkey.as_bytes());
    with_token[1].script_pubkey = ScriptBuf::from_bytes(field);

    assert!(run(&spend(&node, &with_token), &node).is_err());
}

#[test]
fn another_version_is_refused() {
    let outputs = p2pkh_outputs(2);
    let node = node_for(&outputs, 200);
    let mut transaction = spend(&node, &outputs);
    transaction.version = Version::ONE;

    assert!(run(&transaction, &node).is_err());
}

#[test]
fn another_lock_time_is_refused() {
    let outputs = p2pkh_outputs(2);
    let node = node_for(&outputs, 200);
    let mut transaction = spend(&node, &outputs);
    transaction.lock_time = LockTime::from_consensus(1);

    assert!(run(&transaction, &node).is_err());
}

#[test]
fn another_sequence_is_refused() {
    let outputs = p2pkh_outputs(2);
    let node = node_for(&outputs, 200);
    let mut transaction = spend(&node, &outputs);
    transaction.input[0].sequence = Sequence(0xffff_fffe);

    assert!(run(&transaction, &node).is_err());
}

/// The filler must be exactly the zeros the covenant names, so no executor
/// can give a node transaction another txid.
#[test]
fn a_filler_with_a_nonzero_byte_is_refused() {
    let outputs = p2pkh_outputs(2);
    let node = node_for(&outputs, 200);
    let mut filler = vec![0u8; node.filler_length()];
    filler[3] = 1;
    let mut unlocking = Vec::new();
    push_data(&mut unlocking, &filler);
    push_data(&mut unlocking, &node.redeem_script());
    let mut transaction = spend(&node, &outputs);
    transaction.input[0].script_sig = ScriptBuf::from_bytes(unlocking);

    assert!(run(&transaction, &node).is_err());
}

#[test]
fn a_longer_filler_is_refused() {
    let outputs = p2pkh_outputs(2);
    let node = node_for(&outputs, 200);
    let mut unlocking = Vec::new();
    push_data(&mut unlocking, &vec![0u8; node.filler_length() + 1]);
    push_data(&mut unlocking, &node.redeem_script());
    let mut transaction = spend(&node, &outputs);
    transaction.input[0].script_sig = ScriptBuf::from_bytes(unlocking);

    assert!(run(&transaction, &node).is_err());
}

/// Someone sends a coin of another value to a node's address. Spending it
/// with the node's transaction would pay the payees from the wrong coin; the
/// covenant refuses any value but its own.
#[test]
fn a_look_alike_coin_of_another_value_cannot_be_spent() {
    let outputs = p2pkh_outputs(2);
    let node = node_for(&outputs, 200);
    let look_alike = TxOut {
        value: Amount::from_sat(node.value() + 5_000),
        script_pubkey: node.locking_script(),
    };

    assert!(run_p2sh32_spend_with_both_vms(&spend(&node, &outputs), 0, &[look_alike]).is_err());
}

// False statements built from scratch.

/// A thief writes its own covenant from scratch, committing honestly to the
/// outputs it wants with the node's value and output count, and spends the
/// node with that covenant's canonical transaction. Every check in its own
/// redeem script passes; the node's P2SH32 hash does not.
#[test]
fn a_thiefs_own_covenant_cannot_spend_the_node() {
    let outputs = p2pkh_outputs(3);
    let node = node_for(&outputs, 300);
    let thief_outputs = vec![TxOut {
        value: Amount::from_sat(node.value() - 300),
        script_pubkey: p2pkh(9_999),
    }];
    let forged = NodeCovenant::new(node.value(), 1, vec![outputs_hash(&thief_outputs)]);
    let transaction = spend(&forged, &thief_outputs);

    assert!(run(&transaction, &forged).is_ok());
    assert!(run(&transaction, &node).is_err());
}

/// A thief builds, from scratch, an output set that keeps every committed
/// property the covenant checks one by one (the count, the total, the
/// value of each output and the script sizes) but pays itself instead of one
/// payee. The output hash is the only check left, and it refuses.
#[test]
fn an_output_set_matching_every_property_but_the_payee_is_refused() {
    let outputs = p2pkh_outputs(3);
    let node = node_for(&outputs, 300);
    let forged: Vec<TxOut> = outputs
        .iter()
        .enumerate()
        .map(|(index, output)| TxOut {
            value: output.value,
            script_pubkey: if index == 1 {
                p2pkh(9_999)
            } else {
                output.script_pubkey.clone()
            },
        })
        .collect();
    let paid: u64 = forged.iter().map(|output| output.value.to_sat()).sum();

    assert_eq!(paid + 300, node.value());
    assert_eq!(forged.len(), node.output_count());
    assert!(run(&spend(&node, &forged), &node).is_err());
}

// The script itself.

#[test]
fn the_redeem_script_of_a_node_is_about_a_hundred_bytes() {
    let one = NodeCovenant::new(1_000, 1, vec![[0x01; 32]]);
    let five_hundred = NodeCovenant::new(1_000, 500, vec![[0x01; 32]]);
    let root = NodeCovenant::new(1_000, 500, vec![[0x01; 32]; 3]);

    assert_eq!(one.redeem_script().len(), 101);
    assert_eq!(five_hundred.redeem_script().len(), 107);
    assert_eq!(root.redeem_script().len(), 180);
}

/// The value is a fixed 8-byte push, so a node's size does not depend on
/// its value, and tree sizes are known before the values are.
#[test]
fn the_redeem_script_length_does_not_depend_on_the_value() {
    let small = NodeCovenant::new(1, 7, vec![[0x01; 32]]);
    let large = NodeCovenant::new(2_100_000_000_000_000, 7, vec![[0x01; 32]]);

    assert_eq!(small.redeem_script().len(), large.redeem_script().len());
    assert_ne!(small.script_hash(), large.script_hash());
}

#[test]
fn the_filler_is_eight_bytes_per_output() {
    assert_eq!(NodeCovenant::new(1, 1, vec![[0; 32]]).filler_length(), 8);
    assert_eq!(
        NodeCovenant::new(1, 500, vec![[0; 32]]).filler_length(),
        4_000
    );
}

#[test]
fn the_output_hash_chains_value_size_and_script() {
    let output = TxOut {
        value: Amount::from_sat(0x0102),
        script_pubkey: ScriptBuf::from_bytes(vec![0x51]),
    };
    let mut first = Vec::new();
    first.extend_from_slice(&0x0102u64.to_le_bytes());
    first.push(1);
    first.push(0x51);
    let expected_first = sha256::Hash::hash(&first).to_byte_array();
    let mut second = expected_first.to_vec();
    second.extend_from_slice(&first);
    let expected_second = sha256::Hash::hash(&second).to_byte_array();

    assert_eq!(outputs_hash(std::slice::from_ref(&output)), expected_first);
    assert_eq!(outputs_hash(&[output.clone(), output]), expected_second);
}

#[test]
fn p2sh20_payees_are_paid_by_the_covenant_too() {
    let outputs = vec![TxOut {
        value: Amount::from_sat(9_000),
        script_pubkey: ScriptBuf::new_p2sh(&ScriptHash::from_byte_array([0x77; 20])),
    }];
    let node = node_for(&outputs, 200);

    assert!(run(&spend(&node, &outputs), &node).is_ok());
}
