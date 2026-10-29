// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use crate::shares::bch_htlc::p2sh32_script;
use crate::test_utils::bch_vm::run_p2sh32_spend_with_both_vms;
use bitcoin::hashes::Hash;
use bitcoin::{PubkeyHash, Txid};

/// P2PKH script of a payee numbered `number`.
fn p2pkh(number: u32) -> ScriptBuf {
    let mut hash = [0x11u8; 20];
    hash[..4].copy_from_slice(&number.to_le_bytes());
    ScriptBuf::new_p2pkh(&PubkeyHash::from_byte_array(hash))
}

/// `count` P2PKH payees owed `amount` satoshis each, the last owed `last`.
fn payees(count: u32, amount: u64, last: u64) -> Vec<TreePayee> {
    (0..count)
        .map(|number| TreePayee {
            script_pubkey: p2pkh(number),
            amount: if number + 1 == count { last } else { amount },
        })
        .collect()
}

fn root_outpoint() -> OutPoint {
    OutPoint::new(Txid::from_byte_array([0x44; 32]), 3)
}

/// Satoshis paid to payee scripts by the variant's transactions.
fn paid_to_payees(tree: &PayoutTree, transactions: &[Transaction]) -> u64 {
    let payee_scripts: std::collections::HashSet<&ScriptBuf> = tree
        .payees()
        .iter()
        .map(|payee| &payee.script_pubkey)
        .collect();
    transactions
        .iter()
        .flat_map(|transaction| &transaction.output)
        .filter(|output| payee_scripts.contains(&output.script_pubkey))
        .map(|output| output.value.to_sat())
        .sum()
}

/// Each transaction's fee, given the outputs it spends, in order.
fn fees(tree: &PayoutTree, transactions: &[Transaction]) -> Vec<u64> {
    let spent = spent_outputs(tree, transactions);
    transactions
        .iter()
        .zip(&spent)
        .map(|(transaction, spent)| {
            let paid: u64 = transaction
                .output
                .iter()
                .map(|output| output.value.to_sat())
                .sum();
            spent.value.to_sat() - paid
        })
        .collect()
}

/// The output each node transaction spends: the root output for the root,
/// else its parent's output.
fn spent_outputs(tree: &PayoutTree, transactions: &[Transaction]) -> Vec<TxOut> {
    let by_txid: std::collections::HashMap<Txid, &Transaction> = transactions
        .iter()
        .map(|transaction| (transaction.compute_txid(), transaction))
        .collect();
    transactions
        .iter()
        .map(|transaction| {
            let previous = transaction.input[0].previous_output;
            if previous == root_outpoint() {
                tree.root_output()
            } else {
                by_txid[&previous.txid].output[previous.vout as usize].clone()
            }
        })
        .collect()
}

/// Run every node transaction in both script machines against the output it
/// spends, checking that their acceptance decisions agree.
fn vm_verdicts(tree: &PayoutTree, transactions: &[Transaction]) -> Vec<Result<(), String>> {
    let spent = spent_outputs(tree, transactions);
    transactions
        .iter()
        .zip(spent)
        .map(|(transaction, spent)| {
            run_p2sh32_spend_with_both_vms(transaction, 0, &[spent]).map(|_| ())
        })
        .collect()
}

// Shapes.

#[test]
fn one_payee_takes_one_transaction() {
    let tree = PayoutTree::build(payees(1, 0, 1_000_000)).unwrap();
    assert_eq!(tree.node_count(), 1);
}

#[test]
fn five_hundred_payees_take_one_transaction() {
    let tree = PayoutTree::build(payees(500, 100_000, 100_000)).unwrap();
    assert_eq!(tree.node_count(), 1);
}

#[test]
fn five_hundred_and_one_payees_take_a_root_and_two_nodes() {
    let tree = PayoutTree::build(payees(501, 100_000, 100_000)).unwrap();
    assert_eq!(tree.node_count(), 3);
    let transactions = tree.node_transactions(1, root_outpoint()).unwrap();
    assert_eq!(transactions[0].output.len(), 2);
    assert_eq!(transactions[1].output.len(), 500);
    assert_eq!(transactions[2].output.len(), 1);
}

#[test]
fn two_hundred_fifty_thousand_payees_take_two_levels() {
    let tree = PayoutTree::build(payees(250_000, 100_000, 100_000)).unwrap();
    assert_eq!(tree.node_count(), 1 + 500);
}

#[test]
fn two_hundred_fifty_thousand_and_one_payees_take_three_levels() {
    let tree = PayoutTree::build(payees(250_001, 100_000, 100_000)).unwrap();
    // 501 nodes pay the payees, two nodes pay those, and the root pays two.
    assert_eq!(tree.node_count(), 1 + 2 + 501);
}

#[test]
fn an_empty_tree_is_an_error() {
    assert_eq!(
        PayoutTree::build(Vec::new()),
        Err(PayoutTreeError::NoPayees)
    );
}

// Value and fees.

/// At every rate, the payee outputs and the fees add up to the tree's value
/// to the satoshi.
#[test]
fn outputs_and_fees_equal_the_tree_value_at_every_rate() {
    let tree = PayoutTree::build(payees(1_201, 90_000, 2_000_000)).unwrap();
    let at_one = tree.node_transactions(1, root_outpoint()).unwrap();
    let at_two = tree.node_transactions(2, root_outpoint()).unwrap();
    let at_three = tree.node_transactions(3, root_outpoint()).unwrap();

    assert_eq!(
        paid_to_payees(&tree, &at_one) + fees(&tree, &at_one).iter().sum::<u64>(),
        tree.value()
    );
    assert_eq!(
        paid_to_payees(&tree, &at_two) + fees(&tree, &at_two).iter().sum::<u64>(),
        tree.value()
    );
    assert_eq!(
        paid_to_payees(&tree, &at_three) + fees(&tree, &at_three).iter().sum::<u64>(),
        tree.value()
    );
    assert_eq!(tree.total_fee(1), Some(fees(&tree, &at_one).iter().sum()));
}

/// Every node but the root pays exactly the rate times its size; the root
/// pays at least that and less than `n` satoshis more.
#[test]
fn nodes_pay_the_rate_times_their_size_and_the_root_the_rounding() {
    let tree = PayoutTree::build(payees(1_201, 90_000, 2_000_000)).unwrap();
    let transactions = tree.node_transactions(2, root_outpoint()).unwrap();
    let fees = fees(&tree, &transactions);
    let sizes: Vec<u64> = transactions
        .iter()
        .map(|transaction| transaction.total_size() as u64)
        .collect();

    assert!(fees[0] >= 2 * sizes[0]);
    assert!(fees[0] < 2 * sizes[0] + 1_201);
    assert_eq!(
        fees[1..],
        sizes[1..].iter().map(|size| 2 * size).collect::<Vec<_>>()[..]
    );
}

/// The whole tree pays at most `R * B + n - 1` satoshis.
#[test]
fn the_tree_fee_is_at_most_the_rate_times_its_bytes_plus_rounding() {
    let tree = PayoutTree::build(payees(777, 90_000, 2_000_000)).unwrap();
    let transactions = tree.node_transactions(3, root_outpoint()).unwrap();
    let bytes: u64 = transactions
        .iter()
        .map(|transaction| transaction.total_size() as u64)
        .sum();

    assert!(tree.total_fee(3).unwrap() < 3 * bytes + 777);
    assert!(tree.total_fee(3).unwrap() >= 3 * bytes);
}

/// At 1 sat/byte, the fee a P2PKH payee pays is under a hundred satoshis.
#[test]
fn a_payee_pays_under_a_hundred_satoshis_at_one_sat_per_byte() {
    let tree = PayoutTree::build(payees(1_000, 90_000, 90_000)).unwrap();
    assert!(tree.total_fee(1).unwrap() < 1_000 * 100);
}

#[test]
fn a_rate_the_root_does_not_accept_has_no_transactions() {
    let tree = PayoutTree::build(payees(3, 90_000, 90_000)).unwrap();
    assert!(tree.node_transactions(4, root_outpoint()).is_none());
    assert!(tree.node_transactions(0, root_outpoint()).is_none());
}

// Dust.

/// Payees whose net amount at 3 sat/byte would be below dust leave the tree;
/// their amounts go to the last payee, and the tree's value does not change.
#[test]
fn payees_below_dust_are_merged_into_the_last_payee() {
    let mut owed = payees(10, 50_000, 1_000_000);
    owed[0].amount = 300;
    owed[1].amount = 600;
    let value: u64 = owed.iter().map(|payee| payee.amount).sum();

    let tree = PayoutTree::build(owed).unwrap();

    assert_eq!(tree.payees().len(), 8);
    assert_eq!(tree.payees()[7].amount, 1_000_000 + 300 + 600);
    assert_eq!(tree.value(), value);
    assert_eq!(tree.payees()[0].script_pubkey, p2pkh(2));
}

/// After merging, every payee's output meets its dust threshold at the
/// highest rate, so every node transaction can be relayed.
#[test]
fn after_merging_every_payee_output_meets_dust_at_the_highest_rate() {
    let owed: Vec<TreePayee> = (0..30u32)
        .map(|number| TreePayee {
            script_pubkey: p2pkh(number),
            amount: 200 + 100 * u64::from(number),
        })
        .collect();

    let tree = PayoutTree::build(owed).unwrap();
    let transactions = tree.node_transactions(3, root_outpoint()).unwrap();

    assert!(
        transactions
            .iter()
            .flat_map(|transaction| &transaction.output)
            .all(|output| output.value.to_sat() >= dust_threshold_for_script_length(25))
    );
}

/// Merging can take several rounds: removing payees raises the share of the
/// fixed bytes each remaining payee carries.
#[test]
fn merging_repeats_until_no_payee_is_below_dust() {
    let owed: Vec<TreePayee> = (0..6u32)
        .map(|number| TreePayee {
            script_pubkey: p2pkh(number),
            amount: if number == 5 { 1_200 } else { 700 },
        })
        .collect();

    let tree = PayoutTree::build(owed).unwrap();

    assert_eq!(tree.payees().len(), 1);
    assert_eq!(tree.payees()[0].amount, 4_700);
}

/// An underfunded last payee must be handled by the hybrid split, never
/// committed to a zero-valued or sub-dust tree output.
#[test]
fn a_last_payee_below_its_fee_is_refused() {
    assert_eq!(
        PayoutTree::build(payees(1, 0, 100)),
        Err(PayoutTreeError::InsufficientValue)
    );
}

#[test]
fn a_last_payee_below_fee_plus_dust_is_refused() {
    let owed = payees(1, 0, 1_000_000);
    let shape = TreeShape::for_payees(1);
    let fee = MAX_TREE_FEE_RATE * node_sizes(&shape, &owed)[0] as u64;
    let minimum = fee + owed[0].dust();
    assert_eq!(
        PayoutTree::build(payees(1, 0, minimum - 1)),
        Err(PayoutTreeError::InsufficientValue)
    );
}

#[test]
fn a_last_payee_at_fee_plus_dust_is_relayable_at_every_rate() {
    let owed = payees(1, 0, 1_000_000);
    let shape = TreeShape::for_payees(1);
    let minimum = MAX_TREE_FEE_RATE * node_sizes(&shape, &owed)[0] as u64 + owed[0].dust();
    let tree = PayoutTree::build(payees(1, 0, minimum)).unwrap();
    let transactions = tree.node_transactions(3, root_outpoint()).unwrap();
    assert_eq!(transactions[0].output[0].value.to_sat(), owed[0].dust());
    assert_eq!(vm_verdicts(&tree, &transactions), vec![Ok(())]);
}

// Execution.

/// Every node transaction of a two-level tree passes the script machine at
/// every rate, spending the root's coinbase output and then its parents.
#[test]
fn every_node_transaction_passes_the_covenant_at_every_rate() {
    let tree = PayoutTree::build(payees(1_001, 90_000, 2_000_000)).unwrap();
    let at_one = tree.node_transactions(1, root_outpoint()).unwrap();
    let at_two = tree.node_transactions(2, root_outpoint()).unwrap();
    let at_three = tree.node_transactions(3, root_outpoint()).unwrap();

    let verdicts_one = vm_verdicts(&tree, &at_one);
    let verdicts_two = vm_verdicts(&tree, &at_two);
    let verdicts_three = vm_verdicts(&tree, &at_three);

    assert!(verdicts_one.iter().all(Result::is_ok), "{verdicts_one:?}");
    assert!(verdicts_two.iter().all(Result::is_ok), "{verdicts_two:?}");
    assert!(
        verdicts_three.iter().all(Result::is_ok),
        "{verdicts_three:?}"
    );
}

/// A node below the root accepts only its own rate's outputs: mixing a
/// 1 sat/byte root with 2 sat/byte children fails, because the children of
/// one rate are other outputs than the children of another.
#[test]
fn a_child_of_another_rate_cannot_spend_a_node() {
    let tree = PayoutTree::build(payees(501, 90_000, 2_000_000)).unwrap();
    let at_one = tree.node_transactions(1, root_outpoint()).unwrap();
    let at_two = tree.node_transactions(2, root_outpoint()).unwrap();
    let mut mixed = at_two[1].clone();
    mixed.input[0].previous_output = OutPoint::new(at_one[0].compute_txid(), 0);

    assert!(run_p2sh32_spend_with_both_vms(&mixed, 0, &[at_one[0].output[0].clone()]).is_err());
}

/// A thief builds, from scratch and with the same builder, the tree for a
/// payee set in which it replaces one payee, and spends the honest tree's
/// coinbase output with that tree's root transaction. Its own tree is
/// internally valid; the honest root's covenant refuses it.
#[test]
fn a_tree_built_for_other_payees_cannot_spend_the_root() {
    let honest = payees(40, 90_000, 2_000_000);
    let mut forged_payees = honest.clone();
    forged_payees[39].script_pubkey = p2pkh(999_999);
    let honest_tree = PayoutTree::build(honest).unwrap();
    let forged_tree = PayoutTree::build(forged_payees).unwrap();
    let forged = forged_tree.node_transactions(1, root_outpoint()).unwrap();

    assert_eq!(forged_tree.value(), honest_tree.value());
    assert!(run_p2sh32_spend_with_both_vms(&forged[0], 0, &[forged_tree.root_output()]).is_ok());
    assert!(run_p2sh32_spend_with_both_vms(&forged[0], 0, &[honest_tree.root_output()]).is_err());
}

/// The same thief keeps the honest root's unlocking bytecode and only
/// substitutes its forged outputs. The covenant runs and refuses them.
#[test]
fn forged_outputs_under_the_honest_covenant_are_refused() {
    let honest = payees(40, 90_000, 2_000_000);
    let mut forged_payees = honest.clone();
    forged_payees[39].script_pubkey = p2pkh(999_999);
    let honest_tree = PayoutTree::build(honest).unwrap();
    let forged_tree = PayoutTree::build(forged_payees).unwrap();
    let mut transaction = honest_tree
        .node_transactions(1, root_outpoint())
        .unwrap()
        .remove(0);
    transaction.output = forged_tree
        .node_transactions(1, root_outpoint())
        .unwrap()
        .remove(0)
        .output;

    assert!(run_p2sh32_spend_with_both_vms(&transaction, 0, &[honest_tree.root_output()]).is_err());
}

/// Node transactions are a function of the payees and the root outpoint
/// only, so every node computes the same txids.
#[test]
fn node_transactions_are_deterministic() {
    let first = PayoutTree::build(payees(700, 90_000, 2_000_000)).unwrap();
    let second = PayoutTree::build(payees(700, 90_000, 2_000_000)).unwrap();

    assert_eq!(
        first.node_transactions(1, root_outpoint()),
        second.node_transactions(1, root_outpoint())
    );
}

#[test]
fn the_root_output_is_p2sh32_and_holds_the_tree_value() {
    let tree = PayoutTree::build(payees(3, 90_000, 2_000_000)).unwrap();
    let output = tree.root_output();
    assert_eq!(output.value.to_sat(), 2_180_000);
    assert_eq!(output.script_pubkey.len(), 35);
    assert_eq!(output.size(), 44);
    let variant = tree.variant(1).unwrap();
    assert_eq!(
        p2sh32_script(&variant.nodes()[0].covenant().script_hash()),
        output.script_pubkey
    );
}
