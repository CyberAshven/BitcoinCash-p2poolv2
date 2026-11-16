// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Invariants of the hybrid payout split at many sizes.

use super::*;
use crate::accounting::payout::payout_levels::MIN_PAYOUT_LEVEL;
use bitcoin::hashes::Hash;
use bitcoin::{Address, BlockHash, Network, PubkeyHash};
use std::collections::HashSet;

/// A coinbase subsidy of 3.125 BCH, in satoshis.
const REWARD: u64 = 312_500_000;

/// A share with no merge-mining commitment: the whole BCH transaction limit.
const NO_TOKEN: PayoutSplit = PayoutSplit::BchHybrid {
    coinbase_limit: BCH_MAX_TX_SIZE,
};

/// A token share: the coinbase stays within the covenant's hashing limit.
const TOKEN: PayoutSplit = PayoutSplit::BchHybrid {
    coinbase_limit: BCH_TOKEN_SHARE_COINBASE_LIMIT,
};

/// Window sizes around the token-share boundary and up to five thousand.
const SIZES: [u32; 9] = [1, 2, 79, 80, 81, 500, 501, 1_000, 5_000];

/// No draw, then the lowest level, level 5 and the highest level.
const LEVELS: [Option<u8>; 4] = [None, Some(MIN_PAYOUT_LEVEL), Some(5), Some(7)];

/// P2PKH on regtest. The first four bytes are `number`, so each payee is a
/// different address.
fn payee(number: u32) -> PayoutAddress {
    let mut hash = [0x33u8; 20];
    hash[..4].copy_from_slice(&number.to_le_bytes());
    Address::p2pkh(PubkeyHash::from_byte_array(hash), Network::Regtest).into()
}

/// Payees `1..=count`. Rising weights put them in number order; flat weights
/// tie, and the locking scripts break the ties. `level` draws every payee at
/// that level from the all-zero parent share.
fn window(count: u32, rising: bool, level: Option<u8>) -> PplnsWeights {
    let weights: HashMap<PayoutAddress, u128> = (1..=count)
        .map(|number| {
            let weight = if rising {
                u128::from(number) * 1_000
            } else {
                1_000
            };
            (payee(number), weight)
        })
        .collect();
    if let Some(level) = level {
        let levels: HashMap<PayoutAddress, u8> =
            (1..=count).map(|number| (payee(number), level)).collect();
        PplnsWeights {
            weights,
            levels: Some(PayoutLevels::new(BlockHash::all_zeros(), levels)),
        }
    } else {
        PplnsWeights::without_levels(weights)
    }
}

/// The direct outputs and the tree `split` makes of `amount` for `window`.
fn paid(
    window: &PplnsWeights,
    amount: u64,
    split: PayoutSplit,
) -> (Vec<OutputPair>, Option<PayoutTree>) {
    let mut outputs = Vec::new();
    let tree = append_payout_split(window, Amount::from_sat(amount), split, &mut outputs).unwrap();
    (outputs, tree)
}

/// Every size, weight shape, level and split the scale checks share.
fn every_case() -> Vec<(u32, bool, Option<u8>, PayoutSplit, &'static str)> {
    SIZES
        .into_iter()
        .flat_map(|count| {
            [false, true].into_iter().flat_map(move |rising| {
                LEVELS.into_iter().flat_map(move |level| {
                    [(NO_TOKEN, "no tokens"), (TOKEN, "tokens")]
                        .into_iter()
                        .map(move |(split, name)| (count, rising, level, split, name))
                })
            })
        })
        .collect()
}

/// Bytes of `outputs` as a coinbase serializes them.
fn output_bytes(outputs: &[OutputPair]) -> usize {
    outputs
        .iter()
        .map(|output| {
            bitcoin::TxOut {
                value: output.amount,
                script_pubkey: output.address.script_pubkey(),
            }
            .size()
        })
        .sum()
}

/// Locking scripts the split paid: each direct output, then each tree payee.
fn paid_scripts(outputs: &[OutputPair], tree: &Option<PayoutTree>) -> Vec<ScriptBuf> {
    let mut scripts: Vec<ScriptBuf> = outputs
        .iter()
        .map(|output| output.address.script_pubkey())
        .collect();
    if let Some(built) = tree {
        scripts.extend(
            built
                .payees()
                .iter()
                .map(|payee| payee.script_pubkey.clone()),
        );
    }
    scripts
}

/// Satoshis the direct outputs pay.
fn output_sats(outputs: &[OutputPair]) -> u64 {
    outputs.iter().map(|output| output.amount.to_sat()).sum()
}

/// The case a failure names: the payee count, the weight shape, the level
/// and which coinbase limit the split uses.
fn case(count: u32, rising: bool, level: Option<u8>, name: &str) -> String {
    format!("count {count}, rising {rising}, level {level:?}, {name}")
}

/// Direct outputs plus the tree's value pay the whole reward. A level draw
/// and the rounding remainder move satoshis between payees and leave the
/// total unchanged.
#[test]
fn the_split_pays_exactly_the_amount_in_every_case() {
    for (count, rising, level, split, name) in every_case() {
        let window = window(count, rising, level);
        let (outputs, tree) = paid(&window, REWARD, split);
        let paid_sats = output_sats(&outputs) + tree.as_ref().map_or(0, PayoutTree::value);
        assert_eq!(paid_sats, REWARD, "{}", case(count, rising, level, name));
    }
}

/// Each locking script is paid once, either as a direct output or through
/// the tree.
#[test]
fn no_payee_is_paid_twice_in_any_case() {
    for (count, rising, level, split, name) in every_case() {
        let window = window(count, rising, level);
        let (outputs, tree) = paid(&window, REWARD, split);
        let scripts = paid_scripts(&outputs, &tree);
        let distinct: HashSet<ScriptBuf> = scripts.iter().cloned().collect();
        assert_eq!(
            distinct.len(),
            scripts.len(),
            "{}",
            case(count, rising, level, name)
        );
    }
}

/// The split pays only addresses from the window it was given.
#[test]
fn every_paid_script_belongs_to_a_payee_of_the_window() {
    for (count, rising, level, split, name) in every_case() {
        let window = window(count, rising, level);
        let (outputs, tree) = paid(&window, REWARD, split);
        let expected: HashSet<ScriptBuf> = window
            .weights
            .keys()
            .map(|address| address.script_pubkey())
            .collect();
        let label = case(count, rising, level, name);
        assert!(
            paid_scripts(&outputs, &tree)
                .iter()
                .all(|script| expected.contains(script)),
            "{label}"
        );
    }
}

/// Five hundred and five thousand rising payees do not fit a token share's
/// coinbase. The tail goes to a tree, and the direct outputs stay within
/// the payout budget: the coinbase limit less the bytes beside the payouts.
#[test]
fn a_token_share_keeps_its_direct_payouts_within_its_coinbase() {
    for count in [500, 5_000] {
        let (outputs, tree) = paid(&window(count, true, None), REWARD, TOKEN);
        let label = case(count, true, None, "tokens");
        assert!(tree.is_some(), "{label}");
        assert!(
            output_bytes(&outputs)
                <= BCH_TOKEN_SHARE_COINBASE_LIMIT - BCH_COINBASE_BYTES_BESIDE_PAYOUTS,
            "{label}"
        );
    }
}

/// Five thousand P2PKH outputs sit far under Bitcoin Cash's transaction
/// limit, so a share without tokens pays every payee directly.
#[test]
fn a_share_without_tokens_pays_five_thousand_payees_without_a_tree() {
    let (outputs, tree) = paid(&window(5_000, true, None), REWARD, NO_TOKEN);
    let label = case(5_000, true, None, "no tokens");
    assert!(tree.is_none(), "{label}");
    assert_eq!(outputs.len(), 5_000, "{label}");
}

/// Equal slices of 1,000 satoshis are below level 5 (17,472), so the draw
/// pays `1_000_000 / 17_472` whole levels. The winners are paid directly,
/// and their amounts are the whole pot.
#[test]
fn a_level_draw_pays_whole_levels_and_keeps_the_pot() {
    let label = case(1_000, false, Some(5), "no tokens");
    let (outputs, tree) = paid(&window(1_000, false, Some(5)), 1_000_000, NO_TOKEN);
    assert_eq!(level_amount(5), 17_472, "{label}");
    assert!(tree.is_none(), "{label}");
    assert_eq!(outputs.len(), 57, "{label}");
    assert!(
        outputs
            .iter()
            .all(|output| output.amount.to_sat() >= 17_472),
        "{label}"
    );
    assert_eq!(output_sats(&outputs), 1_000_000, "{label}");
}

/// The draw is seeded by the parent share, so the same parent, level and
/// weights produce the same outputs in the same order.
#[test]
fn the_same_parent_gives_the_same_draw() {
    let first = paid(&window(1_000, false, Some(5)), 1_000_000, NO_TOKEN).0;
    let second = paid(&window(1_000, false, Some(5)), 1_000_000, NO_TOKEN).0;
    let pairs = |outputs: &[OutputPair]| -> Vec<(ScriptBuf, Amount)> {
        outputs
            .iter()
            .map(|output| (output.address.script_pubkey(), output.amount))
            .collect()
    };
    assert_eq!(
        pairs(&first),
        pairs(&second),
        "{}",
        case(1_000, false, Some(5), "no tokens")
    );
}

/// Another parent share seeds another draw. The winners change, and each
/// draw still pays the whole pot to 57 payees.
#[test]
fn another_parent_gives_another_draw() {
    let first = paid(&window(1_000, false, Some(5)), 1_000_000, NO_TOKEN).0;
    let weights: HashMap<PayoutAddress, u128> =
        (1..=1_000).map(|number| (payee(number), 1_000)).collect();
    let levels: HashMap<PayoutAddress, u8> = (1..=1_000).map(|number| (payee(number), 5)).collect();
    let second_window = PplnsWeights {
        weights,
        levels: Some(PayoutLevels::new(
            BlockHash::from_byte_array([1; 32]),
            levels,
        )),
    };
    let second = paid(&second_window, 1_000_000, NO_TOKEN).0;
    let scripts = |outputs: &[OutputPair]| -> HashSet<ScriptBuf> {
        outputs
            .iter()
            .map(|output| output.address.script_pubkey())
            .collect()
    };
    let label = case(1_000, false, Some(5), "no tokens");
    assert_ne!(scripts(&first), scripts(&second), "{label}");
    assert_eq!(first.len(), 57, "{label}, all-zero parent");
    assert_eq!(second.len(), 57, "{label}, parent of 0x01 bytes");
    assert_eq!(output_sats(&first), 1_000_000, "{label}, all-zero parent");
    assert_eq!(
        output_sats(&second),
        1_000_000,
        "{label}, parent of 0x01 bytes"
    );
}

/// The tree accepts fee rates 1, 2 and 3 satoshis per byte. The fee rises
/// with the rate and stays below the value the coinbase put in the tree.
#[test]
fn tree_fees_rise_with_the_fee_rate_and_stay_below_the_tree_value() {
    let label = case(1_000, true, None, "tokens");
    let (_, tree) = paid(&window(1_000, true, None), REWARD, TOKEN);
    let tree = tree.expect(&label);
    let at_one = tree.total_fee(1);
    let at_two = tree.total_fee(2);
    let at_three = tree.total_fee(3);
    assert!(at_one.is_some(), "{label}, fee rate 1");
    assert!(at_two.is_some(), "{label}, fee rate 2");
    assert!(at_three.is_some(), "{label}, fee rate 3");
    let at_one = at_one.unwrap();
    let at_two = at_two.unwrap();
    let at_three = at_three.unwrap();
    assert!(at_one < at_two, "{label}: fee rises from rate 1 to 2");
    assert!(at_two < at_three, "{label}: fee rises from rate 2 to 3");
    assert!(
        at_three < tree.value(),
        "{label}: fee at rate 3 stays below the tree value"
    );
    assert!(tree.total_fee(4).is_none(), "{label}, fee rate 4");
}
