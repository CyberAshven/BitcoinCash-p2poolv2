// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use crate::shares::bch_coinbase::BchCoinbase;
use crate::test_utils::TestShareBlockBuilder;
use bitcoin::Address;
use bitcoin::hashes::Hash;
use bitcoin::{Network, PubkeyHash, ScriptHash, TxOut};
use p2poolv2_wallet::payout_address::PayoutAddress;

/// A coinbase subsidy of 3.125 BCH, in satoshis.
const SUBSIDY: u64 = 312_500_000;

const NO_TOKEN: PayoutSplit = PayoutSplit::BchHybrid {
    coinbase_limit: BCH_MAX_TX_SIZE,
};

const TOKEN: PayoutSplit = PayoutSplit::BchHybrid {
    coinbase_limit: BCH_TOKEN_SHARE_COINBASE_LIMIT,
};

/// P2PKH address of payee `number`.
fn p2pkh(number: u32) -> PayoutAddress {
    let mut hash = [0x11u8; 20];
    hash[..4].copy_from_slice(&number.to_le_bytes());
    Address::p2pkh(PubkeyHash::from_byte_array(hash), Network::Regtest).into()
}

/// P2SH20 address of payee `number`.
fn p2sh20(number: u32) -> PayoutAddress {
    let mut hash = [0x22u8; 20];
    hash[..4].copy_from_slice(&number.to_le_bytes());
    Address::p2sh_from_hash(ScriptHash::from_byte_array(hash), Network::Regtest).into()
}

/// `count` P2PKH payees, payee `n` (from 1) weighing `n * 1_000`, so the
/// canonical order is by number.
fn rising_weights(count: u32) -> HashMap<PayoutAddress, u128> {
    (1..=count)
        .map(|number| (p2pkh(number), u128::from(number) * 1_000))
        .collect()
}

fn split(
    weights: &HashMap<PayoutAddress, u128>,
    amount: u64,
    split: PayoutSplit,
) -> (Vec<OutputPair>, Option<PayoutTree>) {
    let mut outputs = Vec::new();
    let tree = append_payout_split(weights, Amount::from_sat(amount), split, &mut outputs).unwrap();
    (outputs, tree)
}

fn output_bytes(outputs: &[OutputPair]) -> usize {
    outputs
        .iter()
        .map(|output| {
            TxOut {
                value: output.amount,
                script_pubkey: output.address.script_pubkey(),
            }
            .size()
        })
        .sum()
}

fn total(outputs: &[OutputPair], tree: &Option<PayoutTree>) -> u64 {
    outputs
        .iter()
        .map(|output| output.amount.to_sat())
        .sum::<u64>()
        + tree.as_ref().map_or(0, PayoutTree::value)
}

// Constants.

#[test]
fn the_payout_budgets_follow_from_the_limits() {
    assert_eq!(BCH_COINBASE_BYTES_BESIDE_PAYOUTS, 367);
    assert_eq!(
        BCH_TOKEN_SHARE_COINBASE_LIMIT - BCH_COINBASE_BYTES_BESIDE_PAYOUTS,
        2_705
    );
    assert_eq!(BCH_MAX_TX_SIZE - BCH_COINBASE_BYTES_BESIDE_PAYOUTS, 999_633);
}

#[test]
fn the_rule_tag_carries_the_limits_and_the_tree() {
    assert_eq!(
        bch_payout_rule_tag(),
        "payout-hybrid-t3072-f500-r1to3-z8-cov1"
    );
}

#[test]
fn bitcoin_pays_every_payee_and_bitcoin_cash_uses_the_hybrid_split() {
    assert_eq!(
        PayoutSplit::for_chain(Chain::Bitcoin, true),
        PayoutSplit::EveryPayee
    );
    assert_eq!(PayoutSplit::for_chain(Chain::BitcoinCash, false), NO_TOKEN);
    assert_eq!(PayoutSplit::for_chain(Chain::BitcoinCash, true), TOKEN);
}

// Token shares, against T = 3,072 bytes.

#[test]
fn one_payee_is_paid_directly() {
    let (outputs, tree) = split(&rising_weights(1), SUBSIDY, TOKEN);
    assert_eq!(outputs.len(), 1);
    assert!(tree.is_none());
    assert_eq!(outputs[0].amount.to_sat(), SUBSIDY);
}

#[test]
fn a_token_share_pays_seventy_nine_payees_directly() {
    let (outputs, tree) = split(&rising_weights(79), SUBSIDY, TOKEN);
    assert_eq!(outputs.len(), 79);
    assert!(tree.is_none());
}

/// Eighty P2PKH outputs take 2,720 bytes: over the 2,705 left beside a tree
/// output, but within the 2,749 left when no tree output is needed, so all
/// are direct.
#[test]
fn a_token_share_pays_eighty_payees_directly_using_the_tree_outputs_bytes() {
    let (outputs, tree) = split(&rising_weights(80), SUBSIDY, TOKEN);
    assert_eq!(outputs.len(), 80);
    assert!(tree.is_none());
    assert_eq!(output_bytes(&outputs), 2_720);
}

/// Eighty-one do not fit even without a tree output: 79 are direct and the
/// two largest go to the tree.
#[test]
fn a_token_share_sends_payees_past_seventy_nine_to_the_tree() {
    let weights = rising_weights(81);
    let (outputs, tree) = split(&weights, SUBSIDY, TOKEN);
    let tree = tree.unwrap();

    assert_eq!(outputs.len(), 79);
    assert_eq!(tree.payees().len(), 2);
    assert_eq!(tree.payees()[0].script_pubkey, p2pkh(80).script_pubkey());
    assert_eq!(tree.payees()[1].script_pubkey, p2pkh(81).script_pubkey());
    assert_eq!(total(&outputs, &Some(tree)), SUBSIDY);
}

/// A tree tail too small to pay its fee and leave its largest payee above
/// dust is merged into that payee and paid directly in the tree output's
/// place, never as a zero-value tree output.
#[test]
fn an_underfunded_tree_tail_is_merged_and_paid_directly() {
    let weights = rising_weights(81);
    let (outputs, tree) = split(&weights, 100, TOKEN);

    assert!(tree.is_none());
    assert_eq!(outputs.len(), 80);
    assert_eq!(outputs[79].address, p2pkh(81));
    assert_eq!(total(&outputs, &tree), 100);
    assert!(
        output_bytes(&outputs) + BCH_COINBASE_BYTES_BESIDE_PAYOUTS - TREE_OUTPUT_BYTES
            <= BCH_TOKEN_SHARE_COINBASE_LIMIT
    );
}

/// Outputs already in the coinbase, as Hydrapool's donation and fee are,
/// take their bytes from the limit, so the coinbase still fits it.
#[test]
fn outputs_already_placed_shrink_the_payout_budget() {
    let mut outputs = vec![
        OutputPair {
            address: p2pkh(1_001),
            amount: Amount::from_sat(1_000),
        },
        OutputPair {
            address: p2pkh(1_002),
            amount: Amount::from_sat(1_000),
        },
    ];
    let tree = append_payout_split(
        &rising_weights(80),
        Amount::from_sat(SUBSIDY),
        TOKEN,
        &mut outputs,
    )
    .unwrap();

    assert_eq!(outputs.len(), 2 + 77);
    assert_eq!(tree.unwrap().payees().len(), 3);
    assert!(
        output_bytes(&outputs) + BCH_COINBASE_BYTES_BESIDE_PAYOUTS
            <= BCH_TOKEN_SHARE_COINBASE_LIMIT
    );
}

/// The coinbase a token share builds stays within T at its worst case: the
/// direct payouts plus every other field at its largest.
#[test]
fn a_token_share_coinbase_stays_within_t() {
    let (outputs, tree) = split(&rising_weights(400), SUBSIDY, TOKEN);
    assert!(tree.is_some());
    assert!(
        output_bytes(&outputs) + BCH_COINBASE_BYTES_BESIDE_PAYOUTS
            <= BCH_TOKEN_SHARE_COINBASE_LIMIT
    );
}

// No-token shares, against BCH's 1,000,000-byte transaction limit.

#[test]
fn a_no_token_share_pays_29_400_payees_directly() {
    let (outputs, tree) = split(&rising_weights(29_400), SUBSIDY, NO_TOKEN);
    assert_eq!(outputs.len(), 29_400);
    assert!(tree.is_none());
}

/// 29,402 P2PKH outputs take 999,668 bytes: over the 999,633 left beside a
/// tree output, within the 999,677 left without one.
#[test]
fn a_no_token_share_pays_29_402_payees_directly_using_the_tree_outputs_bytes() {
    let (outputs, tree) = split(&rising_weights(29_402), SUBSIDY, NO_TOKEN);
    assert_eq!(outputs.len(), 29_402);
    assert!(tree.is_none());
    assert!(output_bytes(&outputs) + BCH_COINBASE_BYTES_BESIDE_PAYOUTS <= BCH_MAX_TX_SIZE + 44);
}

/// Past BCH's transaction limit even a no-token share needs the tree: the
/// 29,401st output would end one byte past the budget.
#[test]
fn a_no_token_share_sends_payees_past_29_400_to_the_tree() {
    let (outputs, tree) = split(&rising_weights(29_403), SUBSIDY, NO_TOKEN);
    let tree = tree.unwrap();
    assert_eq!(outputs.len(), 29_400);
    assert_eq!(tree.payees().len(), 3);
    assert!(output_bytes(&outputs) + BCH_COINBASE_BYTES_BESIDE_PAYOUTS <= BCH_MAX_TX_SIZE);
}

#[test]
fn a_no_token_share_with_thirty_thousand_payees_pays_the_largest_through_the_tree() {
    let weights = rising_weights(30_000);
    let (outputs, tree) = split(&weights, SUBSIDY, NO_TOKEN);
    let tree = tree.unwrap();
    assert_eq!(outputs.len(), 29_400);
    assert_eq!(tree.payees().len(), 600);
    assert_eq!(total(&outputs, &Some(tree)), SUBSIDY);
}

// The straddle rule, on output sizes.

/// A payee whose output ends exactly at the budget is paid directly.
#[test]
fn a_payee_ending_exactly_at_the_budget_is_direct() {
    assert_eq!(
        direct_payee_count(&[2_705, 100], BCH_TOKEN_SHARE_COINBASE_LIMIT),
        1
    );
}

/// One byte over and it goes to the tree with everyone after it.
#[test]
fn a_payee_one_byte_over_the_budget_goes_to_the_tree() {
    assert_eq!(
        direct_payee_count(&[2_706, 100], BCH_TOKEN_SHARE_COINBASE_LIMIT),
        0
    );
}

/// A smaller output after the straddler is never moved back into the
/// direct part, even when it would fit the bytes left over.
#[test]
fn no_payee_after_the_straddler_is_moved_back() {
    assert_eq!(
        direct_payee_count(&[2_680, 34, 20, 40], BCH_TOKEN_SHARE_COINBASE_LIMIT),
        1
    );
}

/// Mixed P2PKH and P2SH20 payees, the 80th straddling the budget by one
/// byte: 73 P2PKH and 6 P2SH20 outputs take 2,674 bytes and a 7th P2SH20
/// would take 2,706. It and every payee after it go to the tree, although a
/// P2SH20 output is smaller than the bytes some payees already took.
#[test]
fn a_mixed_payee_straddling_by_one_byte_goes_to_the_tree() {
    let mut weights: HashMap<PayoutAddress, u128> = (1..=73u32)
        .map(|number| (p2pkh(number), u128::from(number)))
        .collect();
    weights.extend((74..=86u32).map(|number| (p2sh20(number), u128::from(number))));

    let (outputs, tree) = split(&weights, SUBSIDY, TOKEN);

    assert_eq!(outputs.len(), 79);
    assert_eq!(output_bytes(&outputs), 2_674);
    assert_eq!(tree.unwrap().payees().len(), 7);
}

// Order and amounts.

/// The direct payees are in canonical order: ascending weight.
#[test]
fn direct_payees_are_in_ascending_weight_order() {
    let weights = HashMap::from([(p2pkh(1), 30u128), (p2pkh(2), 10), (p2pkh(3), 20)]);
    let (outputs, _) = split(&weights, 600, NO_TOKEN);
    assert_eq!(outputs[0].address, p2pkh(2));
    assert_eq!(outputs[1].address, p2pkh(3));
    assert_eq!(outputs[2].address, p2pkh(1));
}

/// Equal weights are ordered by locking bytecode.
#[test]
fn equal_weights_are_ordered_by_script_bytes() {
    let weights = HashMap::from([(p2pkh(9), 10u128), (p2sh20(1), 10), (p2pkh(3), 10)]);
    let (outputs, _) = split(&weights, 600, NO_TOKEN);
    let scripts: Vec<ScriptBuf> = outputs
        .iter()
        .map(|output| output.address.script_pubkey())
        .collect();
    let mut sorted = scripts.clone();
    sorted.sort_by(|first, second| first.as_bytes().cmp(second.as_bytes()));
    assert_eq!(scripts, sorted);
}

/// The rounding remainder goes to the largest payee, in the tree when there
/// is one.
#[test]
fn the_remainder_goes_to_the_largest_payee_in_the_tree() {
    let weights = rising_weights(81);
    let total_weight: u128 = weights.values().sum();
    let largest_share = (u128::from(SUBSIDY) * 81_000 / total_weight) as u64;
    let paid_shares: u64 = (1..=81u128)
        .map(|number| (u128::from(SUBSIDY) * number * 1_000 / total_weight) as u64)
        .sum();

    let (_, tree) = split(&weights, SUBSIDY, TOKEN);

    let tree = tree.unwrap();
    let largest = &tree.payees()[1];
    assert_eq!(largest.amount, largest_share + (SUBSIDY - paid_shares));
}

/// Direct payouts below dust are paid as they are: BCH consensus accepts any
/// coinbase output value.
#[test]
fn direct_payouts_below_dust_are_paid_as_they_are() {
    let weights = HashMap::from([(p2pkh(1), 1_000u128), (p2pkh(2), 1_000_000)]);
    let (outputs, tree) = split(&weights, 100_000, NO_TOKEN);
    assert!(tree.is_none());
    assert_eq!(outputs[0].amount.to_sat(), 99);
    assert_eq!(outputs[1].amount.to_sat(), 99_901);
}

/// Tree payees below dust are merged into the largest one; the tree's value,
/// and so the coinbase total, does not change.
#[test]
fn tree_payees_below_dust_are_merged_into_the_largest() {
    // 79 tiny direct payees, then two tree payees: one owed about 400
    // satoshis, below dust after its fee, and the largest.
    let mut weights: HashMap<PayoutAddress, u128> = (1..=79u32)
        .map(|number| (p2pkh(number), u128::from(number)))
        .collect();
    weights.insert(p2pkh(80), 40_000);
    weights.insert(p2pkh(81), 10_000_000);
    let amount = 100_000_000 / 1_000;

    let (outputs, tree) = split(&weights, amount, TOKEN);
    let tree = tree.unwrap();

    assert_eq!(outputs.len(), 79);
    assert_eq!(tree.payees().len(), 1);
    assert_eq!(tree.payees()[0].script_pubkey, p2pkh(81).script_pubkey());
    assert_eq!(total(&outputs, &Some(tree)), amount);
}

/// The outputs are a function of the weights, the amount and the split only.
#[test]
fn the_split_is_deterministic() {
    let weights = rising_weights(300);
    let reordered: HashMap<PayoutAddress, u128> = (1..=300u32)
        .rev()
        .map(|number| (p2pkh(number), u128::from(number) * 1_000))
        .collect();

    assert_eq!(
        split(&weights, SUBSIDY, TOKEN),
        split(&reordered, SUBSIDY, TOKEN)
    );
}

#[test]
fn a_zero_total_weight_is_an_error() {
    let weights = HashMap::from([(p2pkh(1), 0)]);
    let mut outputs = Vec::new();
    assert!(append_payout_split(&weights, Amount::from_sat(SUBSIDY), TOKEN, &mut outputs).is_err());
}

#[test]
fn bitcoin_pays_two_hundred_payees() {
    let (outputs, tree) = split(&rising_weights(200), SUBSIDY, PayoutSplit::EveryPayee);
    assert_eq!(outputs.len(), 200);
    assert!(tree.is_none());
}

// Token shares.

#[test]
fn a_header_without_a_declared_coinbase_takes_the_no_token_limit() {
    let header = TestShareBlockBuilder::new().build().header;
    assert_eq!(split_for_share(Chain::BitcoinCash, &header), NO_TOKEN);
}

#[test]
fn the_empty_commitment_takes_the_no_token_limit() {
    let mut header = TestShareBlockBuilder::new().build().header;
    header.bch_coinbase = Some(BchCoinbase {
        prefix: vec![0x51],
        merge_mining: MergeMiningCommitment::empty(),
    });
    assert_eq!(split_for_share(Chain::BitcoinCash, &header), NO_TOKEN);
    assert_eq!(
        bch_split_for_commitment(&MergeMiningCommitment::empty()),
        NO_TOKEN
    );
}

#[test]
fn a_commitment_naming_a_root_takes_the_token_limit() {
    let commitment = MergeMiningCommitment {
        root: [0x33; 32],
        height: 2,
        salt: 7,
    };
    let mut header = TestShareBlockBuilder::new().build().header;
    header.bch_coinbase = Some(BchCoinbase {
        prefix: vec![0x51],
        merge_mining: commitment,
    });
    assert_eq!(split_for_share(Chain::BitcoinCash, &header), TOKEN);
    assert_eq!(bch_split_for_commitment(&commitment), TOKEN);
}

#[test]
fn a_bitcoin_share_pays_every_payee_whatever_its_header() {
    let header = TestShareBlockBuilder::new().build().header;
    assert_eq!(
        split_for_share(Chain::Bitcoin, &header),
        PayoutSplit::EveryPayee
    );
}
