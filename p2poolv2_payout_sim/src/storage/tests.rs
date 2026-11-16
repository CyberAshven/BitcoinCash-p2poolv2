// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;
use p2poolv2_netsim::constants::PPLNS_WINDOW_SHARES;

#[test]
fn a_14_day_cap_gives_the_node_s_depths() {
    assert_eq!(
        payout_depth_shares(14.0),
        PPLNS_WINDOW_SHARES as f64 + 103.0,
        "BCH_PAYOUT_DEPTH, 121,063 shares"
    );
    assert_eq!(
        kept_body_shares(14.0),
        242_486.0,
        "BCH_PRUNE_DEPTH and the pruner's lag"
    );
}

#[test]
fn few_payees_above_their_level_are_all_paid_directly() {
    let block = coinbase(1_000, DEFAULT_LEVEL);
    assert_eq!(block.outputs, 1_000.0);
    assert_eq!(block.tree, 0.0);
    assert_eq!(block.bytes, 367.0 + 34_000.0, "pruning.md's 34,367 bytes");
}

#[test]
fn the_default_level_caps_the_outputs_whatever_the_payees() {
    let hundred_thousand = coinbase(100_000, DEFAULT_LEVEL);
    let million = coinbase(1_000_000, DEFAULT_LEVEL);
    assert_eq!(
        hundred_thousand, million,
        "the draw pays the reward in whole levels"
    );
    assert_eq!(million.outputs, (BLOCK_REWARD_SATOSHIS / 17_472.0).floor());
    assert_eq!(million.tree, 0.0, "the outputs fit the 1 MB coinbase");
}

#[test]
fn a_one_minute_block_pays_1788_default_level_outputs() {
    // 0.3125 BCH over 17,472 sat; a ten-minute block's 3.125 BCH paid 17,885.
    assert_eq!(coinbase(1_000_000, DEFAULT_LEVEL).outputs, 1_788.0);
}

#[test]
fn the_lowest_level_pays_every_output_in_the_coinbase_of_a_one_minute_block() {
    // 0.3125 BCH over 1,092 sat is 28,617 outputs, under the 29,400 that
    // 1,000,000 bytes less 367 hold at 34 bytes each. A ten-minute block's
    // 286,172 outputs filled the coinbase and paid the rest through the tree.
    let block = coinbase(1_000_000, MIN_PAYOUT_LEVEL);
    assert_eq!(
        block.outputs,
        (BLOCK_REWARD_SATOSHIS / level_amount(MIN_PAYOUT_LEVEL) as f64).floor()
    );
    assert_eq!(block.outputs, 28_617.0);
    assert_eq!(block.direct, block.outputs);
    assert_eq!(block.tree, 0.0);
    assert!(block.bytes <= 1_000_000.0, "{}", block.bytes);
}

#[test]
fn levels_never_reach_the_payout_tree_without_fees() {
    // The reward over the lowest level is under the 29,400 outputs the
    // coinbase holds, so only fees (left out here) could fill it.
    let fit = ((BCH_MAX_TRANSACTION_BYTES - HYBRID_COINBASE_BYTES_BESIDE_PAYOUTS)
        / P2PKH_OUTPUT_BYTES) as f64;
    assert_eq!(fit, 29_400.0);
    assert!(BLOCK_REWARD_SATOSHIS / (level_amount(MIN_PAYOUT_LEVEL) as f64) < fit);
}
