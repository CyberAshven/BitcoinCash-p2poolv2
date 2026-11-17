// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;
use crate::constants::BCH_NETWORK_HASHRATE;

#[test]
fn the_window_spans_three_block_intervals_over_the_pools_fraction() {
    let window = size_window(0.01 * BCH_NETWORK_HASHRATE, 0.1, BCH_NETWORK_HASHRATE, 10.0);
    let span = BCH_PPLNS_WINDOW_BLOCKS * BCH_BLOCK_INTERVAL_SECONDS / 0.01;
    assert!((window.span_seconds / span - 1.0).abs() < 1e-12);
    assert!((window.shares / (span * 0.1) - 1.0).abs() < 1e-12);
    let work = BCH_PPLNS_WINDOW_BLOCKS * BCH_NETWORK_HASHRATE * BCH_BLOCK_INTERVAL_SECONDS;
    assert!((window.work / work - 1.0).abs() < 1e-12);
}

#[test]
fn the_window_holds_three_blocks_of_work_at_one_percent_of_bch() {
    // A pool of 1% of 3.7 EH/s making one chain share every 10 seconds:
    // three one-minute blocks of six shares over 0.01 = 1,800 shares, 5
    // hours (ten-minute blocks gave 18,000 shares, 50 hours).
    let window = size_window(0.01 * BCH_NETWORK_HASHRATE, 0.1, BCH_NETWORK_HASHRATE, 10.0);
    assert!((window.shares - 1_800.0).abs() < 1e-6);
    assert!((window.span_seconds - 18_000.0).abs() < 1e-3);
    assert!(!window.capped);
}

#[test]
fn weak_work_that_doubles_the_credited_work_halves_the_window() {
    let window = size_window(0.02 * BCH_NETWORK_HASHRATE, 0.1, BCH_NETWORK_HASHRATE, 10.0);
    assert!((window.shares - 900.0).abs() < 1e-6);
    assert!((window.span_seconds - 9_000.0).abs() < 1e-3);
}

#[test]
fn heavier_shares_do_not_change_the_window_span() {
    // The same work in half as many chain shares (band shares twice as
    // heavy): half the shares, the same 5 hours.
    let window = size_window(
        0.01 * BCH_NETWORK_HASHRATE,
        0.05,
        BCH_NETWORK_HASHRATE,
        10.0,
    );
    assert!((window.shares - 900.0).abs() < 1e-6);
    assert!((window.span_seconds - 18_000.0).abs() < 1e-3);
}

#[test]
fn a_small_pool_window_stops_at_the_share_cap() {
    // 0.01% of BCH would need 180,000 shares; the cap is 120,960. With
    // one-minute blocks only pools under about 0.015% of BCH reach it.
    let window = size_window(
        0.0001 * BCH_NETWORK_HASHRATE,
        0.1,
        BCH_NETWORK_HASHRATE,
        10.0,
    );
    assert!(window.capped);
    assert!((window.shares - 120_960.0).abs() < 1e-6);
    assert!((window.span_seconds - 14.0 * 86_400.0).abs() < 1e-3);
}

#[test]
fn a_pool_of_a_tenth_of_a_percent_of_bch_no_longer_reaches_the_cap() {
    // 18,000 shares, under the 120,960 cap that ten-minute blocks reached.
    let window = size_window(
        0.001 * BCH_NETWORK_HASHRATE,
        0.1,
        BCH_NETWORK_HASHRATE,
        10.0,
    );
    assert!(!window.capped);
    assert!((window.shares - 18_000.0).abs() < 1e-6);
}

#[test]
fn the_cap_is_fourteen_days_at_a_five_second_period() {
    let window = size_window(
        0.0001 * BCH_NETWORK_HASHRATE,
        0.2,
        BCH_NETWORK_HASHRATE,
        5.0,
    );
    assert!(window.capped);
    assert!((window.shares - 241_920.0).abs() < 1e-6);
    assert!((window.span_seconds - 14.0 * 86_400.0).abs() < 1e-3);
}

#[test]
fn the_skip_is_one_hundred_chain_shares() {
    let window = size_window(0.01 * BCH_NETWORK_HASHRATE, 0.1, BCH_NETWORK_HASHRATE, 10.0);
    assert!((window.skip_seconds - 1_000.0).abs() < 1e-9);
}
