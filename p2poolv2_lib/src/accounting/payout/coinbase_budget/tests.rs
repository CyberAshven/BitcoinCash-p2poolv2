// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use crate::accounting::payout::payout_levels::MIN_PAYOUT_LEVEL;
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
    let tree = append_payout_split(
        &PplnsWeights::without_levels(weights.clone()),
        Amount::from_sat(amount),
        split,
        &mut outputs,
    )
    .unwrap();
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
        &PplnsWeights::without_levels(rising_weights(80)),
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
    assert!(
        append_payout_split(
            &PplnsWeights::without_levels(weights),
            Amount::from_sat(SUBSIDY),
            TOKEN,
            &mut outputs
        )
        .is_err()
    );
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

/// The parent share every payout level test's draw is seeded from.
const PARENT_SHARE: [u8; 32] = [0x5a; 32];

/// The outputs `split` makes of `amount` for `weights` drawn at `levels`.
fn leveled_split(
    weights: HashMap<PayoutAddress, u128>,
    levels: HashMap<PayoutAddress, u8>,
    amount: u64,
    split: PayoutSplit,
) -> Vec<OutputPair> {
    let mut outputs = Vec::new();
    append_payout_split(
        &PplnsWeights {
            weights,
            levels: Some(PayoutLevels::new(
                bitcoin::BlockHash::from_byte_array(PARENT_SHARE),
                levels,
            )),
        },
        Amount::from_sat(amount),
        split,
        &mut outputs,
    )
    .unwrap();
    outputs
}

/// The amount `outputs` pay `address`, if any.
fn paid_to(outputs: &[OutputPair], address: &PayoutAddress) -> Option<u64> {
    outputs
        .iter()
        .find(|output| output.address == *address)
        .map(|output| output.amount.to_sat())
}

/// Two payees of 50,000 satoshis at level 5, 17,472, are paid as without
/// levels.
#[test]
fn payees_at_or_above_their_level_keep_their_amounts() {
    let outputs = leveled_split(
        HashMap::from([(p2pkh(1), 1), (p2pkh(2), 1)]),
        HashMap::from([(p2pkh(1), 5), (p2pkh(2), 5)]),
        100_000,
        NO_TOKEN,
    );

    assert_eq!(outputs.len(), 2);
    assert_eq!(paid_to(&outputs, &p2pkh(1)), Some(50_000));
    assert_eq!(paid_to(&outputs, &p2pkh(2)), Some(50_000));
}

/// Two slices of 1,000 satoshis at level 5 make a pot of 2,000, below the
/// level: one of them wins it all and the other gets no output, while the
/// payee above its level keeps its 98,000.
#[test]
fn payees_below_their_level_share_one_pot_of_the_same_size() {
    let outputs = leveled_split(
        HashMap::from([(p2pkh(1), 98), (p2pkh(2), 1), (p2pkh(3), 1)]),
        HashMap::from([(p2pkh(1), 0), (p2pkh(2), 5), (p2pkh(3), 5)]),
        100_000,
        NO_TOKEN,
    );
    let small = [paid_to(&outputs, &p2pkh(2)), paid_to(&outputs, &p2pkh(3))];

    assert_eq!(outputs.len(), 2);
    assert_eq!(paid_to(&outputs, &p2pkh(1)), Some(98_000));
    assert!(small.contains(&Some(2_000)), "{small:?}");
    assert!(small.contains(&None), "{small:?}");
    assert_eq!(
        outputs
            .iter()
            .map(|output| output.amount.to_sat())
            .sum::<u64>(),
        100_000
    );
}

/// Four slices of 3,000 satoshis at level 3, 4,368, make a pot of 12,000:
/// `n = 2` parts of 6,000, each at least the level, and the pot is paid
/// exactly.
#[test]
fn a_pot_of_several_levels_is_paid_in_parts_of_at_least_the_level() {
    let outputs = leveled_split(
        HashMap::from([(p2pkh(1), 3), (p2pkh(2), 3), (p2pkh(3), 3), (p2pkh(4), 3)]),
        HashMap::from([(p2pkh(1), 3), (p2pkh(2), 3), (p2pkh(3), 3), (p2pkh(4), 3)]),
        12_000,
        NO_TOKEN,
    );

    assert!(outputs.len() <= 2, "{outputs:?}");
    assert!(
        outputs
            .iter()
            .all(|output| output.amount.to_sat().is_multiple_of(6_000)),
        "{outputs:?}"
    );
    assert_eq!(
        outputs
            .iter()
            .map(|output| output.amount.to_sat())
            .sum::<u64>(),
        12_000
    );
}

/// A payee the levels do not list, as the window lists no donation key or
/// fee address, keeps a slice below every level.
#[test]
fn a_payee_without_a_level_is_never_drawn() {
    let outputs = leveled_split(
        HashMap::from([(p2pkh(1), 999), (p2pkh(9), 1)]),
        HashMap::from([(p2pkh(1), 7)]),
        100_000,
        NO_TOKEN,
    );

    assert_eq!(paid_to(&outputs, &p2pkh(9)), Some(100));
    assert_eq!(paid_to(&outputs, &p2pkh(1)), Some(99_900));
}

/// Every node built on the same parent draws the same winners.
#[test]
fn the_same_parent_draws_the_same_outputs() {
    let weights = HashMap::from([(p2pkh(1), 1), (p2pkh(2), 1), (p2pkh(3), 1)]);
    let levels = HashMap::from([(p2pkh(1), 7), (p2pkh(2), 7), (p2pkh(3), 7)]);

    let first = leveled_split(weights.clone(), levels.clone(), 30_000, NO_TOKEN);
    let second = leveled_split(weights, levels, 30_000, NO_TOKEN);

    assert_eq!(first, second);
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].amount.to_sat(), 30_000);
}

/// A Bitcoin split pays every payee its exact slice, levels or not.
#[test]
fn a_bitcoin_split_never_draws() {
    let outputs = leveled_split(
        HashMap::from([(p2pkh(1), 1), (p2pkh(2), 1)]),
        HashMap::from([(p2pkh(1), 7), (p2pkh(2), 7)]),
        1_000,
        PayoutSplit::EveryPayee,
    );

    assert_eq!(paid_to(&outputs, &p2pkh(1)), Some(500));
    assert_eq!(paid_to(&outputs, &p2pkh(2)), Some(500));
}

/// One class of miners in a `payouts_at_scale_by_miner_class` scenario.
struct MinerClass {
    name: &'static str,
    /// Payees of the class with credit in the window.
    payees: u32,
    /// The class's part of the block reward, by its work.
    part_of_reward: f64,
    /// The payout level every payee of the class is drawn at.
    level: u8,
}

/// Scenario report, not a check: what one Bitcoin Cash block pays each class
/// of miner under the adopted rules (payout levels, the hybrid split, the
/// payout tree and its dust rule), for a share without and with a token
/// commitment, as the pool and Bitcoin Cash's hashrate grow.
///
/// Each scenario is a window: big farms at the default level 5, and home
/// miners whose payees are the weak-share credits the window holds
/// (`shares in the window * WEAK_FILL`, at most one per home miner). At
/// today's hashrate the node leaves a home miner at level 5; at 270 times
/// today's it lowers a 1.2 TH/s miner to the lowest payout level to keep
/// three payments a month.
///
/// Run with
/// `cargo test --release -p p2poolv2_lib payouts_at_scale -- --ignored --nocapture`.
#[test]
#[ignore = "scenario report, run by hand"]
fn payouts_at_scale_by_miner_class() {
    use bitcoin::BlockHash;
    use std::time::Instant;

    let class = |name, payees, part_of_reward, level| MinerClass {
        name,
        payees,
        part_of_reward,
        level,
    };
    let scenarios: Vec<(&str, Vec<MinerClass>)> = vec![
        (
            "today; pool 10% of BCH; home at level 5",
            vec![class("farms", 18, 0.5, 5), class("home", 47_700, 0.5, 5)],
        ),
        (
            "today; pool 1% of BCH; home at level 5",
            vec![class("farms", 2, 0.5, 5), class("home", 15_400, 0.5, 5)],
        ),
        (
            "270x hashrate; pool 30% of BCH; home at the lowest level",
            vec![
                class("farms", 600, 0.5, 5),
                class("home", 19_200, 0.5, MIN_PAYOUT_LEVEL),
            ],
        ),
        (
            "270x hashrate; pool 10% of BCH; home at the lowest level",
            vec![
                class("farms", 1_800, 0.5, 5),
                class("home", 57_600, 0.5, MIN_PAYOUT_LEVEL),
            ],
        ),
        (
            "270x hashrate; pool 2% of BCH; home at the lowest level",
            vec![
                class("farms", 400, 0.5, 5),
                class("home", 288_000, 0.5, MIN_PAYOUT_LEVEL),
            ],
        ),
        (
            "270x hashrate; pool 1% of BCH; home at the lowest level",
            vec![
                class("farms", 200, 0.5, 5),
                class("home", 576_000, 0.5, MIN_PAYOUT_LEVEL),
            ],
        ),
        (
            "270x hashrate; pool 1% of BCH; home at level 1",
            vec![
                class("farms", 200, 0.5, 5),
                class("home", 576_000, 0.5, MIN_PAYOUT_LEVEL),
            ],
        ),
        (
            "270x hashrate; pool 1% of BCH; home 90% of the work at the lowest level",
            vec![
                class("farms", 40, 0.1, 5),
                class("home", 576_000, 0.9, MIN_PAYOUT_LEVEL),
            ],
        ),
    ];

    println!(
        "scenario|share|coinbase outputs|coinbase payout bytes|tree payees|tree transactions|tree fee sat at 1 sat/byte|build ms|class|payees|level|slice sat|fair sat|paid sat|paid over fair|paid in coinbase|paid in tree|tree fee each"
    );
    for (scenario, classes) in &scenarios {
        let mut weights = HashMap::new();
        let mut levels = HashMap::new();
        let mut class_of: HashMap<ScriptBuf, usize> = HashMap::new();
        let mut number = 0u32;
        let mut class_weight = vec![0u128; classes.len()];
        for (index, class) in classes.iter().enumerate() {
            //* Weights in parts of 10^15 of the reward, so a payee's slice
            //* keeps its digits even among 576,000 payees.
            let weight = (class.part_of_reward / f64::from(class.payees) * 1e15) as u128;
            for _ in 0..class.payees {
                number += 1;
                let address = p2pkh(number);
                class_of.insert(address.script_pubkey(), index);
                levels.insert(address.clone(), class.level);
                weights.insert(address, weight);
                class_weight[index] += weight;
            }
        }
        let total_weight: u128 = class_weight.iter().sum();
        let window = PplnsWeights {
            weights,
            levels: Some(PayoutLevels::new(BlockHash::all_zeros(), levels)),
        };
        for (share, payout_split) in [("no tokens", NO_TOKEN), ("tokens", TOKEN)] {
            let mut outputs = Vec::new();
            let started = Instant::now();
            let tree = append_payout_split(
                &window,
                Amount::from_sat(SUBSIDY),
                payout_split,
                &mut outputs,
            )
            .unwrap();
            let build_ms = started.elapsed().as_secs_f64() * 1_000.0;
            let mut coinbase_sat = vec![0u64; classes.len()];
            let mut coinbase_payees = vec![0u32; classes.len()];
            let mut tree_sat = vec![0u64; classes.len()];
            let mut tree_payees = vec![0u32; classes.len()];
            for output in &outputs {
                let class = class_of[&output.address.script_pubkey()];
                coinbase_sat[class] += output.amount.to_sat();
                coinbase_payees[class] += 1;
            }
            let (tree_count, tree_transactions, tree_fee) =
                tree.as_ref().map_or((0, 0, 0), |tree| {
                    for payee in tree.payees() {
                        let class = class_of[&payee.script_pubkey];
                        tree_sat[class] += payee.amount;
                        tree_payees[class] += 1;
                    }
                    (
                        tree.payees().len(),
                        tree.node_count(),
                        tree.total_fee(1).unwrap(),
                    )
                });
            let fee_each = if tree_count == 0 {
                0
            } else {
                tree_fee / tree_count as u64
            };
            for (index, class) in classes.iter().enumerate() {
                let fair = SUBSIDY as f64 * class_weight[index] as f64 / total_weight as f64;
                let paid = coinbase_sat[index] + tree_sat[index];
                println!(
                    "{scenario}|{share}|{}|{}|{tree_count}|{tree_transactions}|{tree_fee}|{build_ms:.0}|{}|{}|{}|{:.0}|{fair:.0}|{paid}|{:.4}|{} payees, {} sat|{} payees, {} sat|{fee_each}",
                    outputs.len(),
                    output_bytes(&outputs),
                    class.name,
                    class.payees,
                    class.level,
                    fair / f64::from(class.payees),
                    paid as f64 / fair,
                    coinbase_payees[index],
                    coinbase_sat[index],
                    tree_payees[index],
                    tree_sat[index],
                );
            }
        }
    }
}
