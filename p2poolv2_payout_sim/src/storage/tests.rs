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
        242_534.0,
        "BCH_PRUNE_DEPTH, with WEAK_DEPTH, and the pruner's lag"
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
    assert_eq!(million.outputs, (312_500_000.0_f64 / 17_472.0).floor());
    assert_eq!(million.tree, 0.0, "17,885 outputs fit the 1 MB coinbase");
}

#[test]
fn the_lowest_level_fills_the_coinbase_and_pays_the_rest_through_the_tree() {
    let block = coinbase(1_000_000, MIN_PAYOUT_LEVEL);
    assert_eq!(block.outputs, (312_500_000.0_f64 / 1_092.0).floor());
    assert_eq!(block.direct, 29_400.0, "1,000,000 bytes less 367, over 34");
    assert_eq!(block.tree, block.outputs - 29_400.0);
    assert!(block.bytes <= 1_000_000.0, "{}", block.bytes);
}
