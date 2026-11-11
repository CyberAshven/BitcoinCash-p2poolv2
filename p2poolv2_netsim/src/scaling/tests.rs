// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;
use crate::scenario::WeakShareCarriage;

#[test]
fn a_search_below_ten_seconds_finds_the_crossing_within_six_percent() {
    // A stale rate of 0.1 / interval crosses 4% at 2.5 seconds.
    let found = shortest_interval_within(0.04, 10.0, 0.25, 40.0, 6, |interval| 0.1 / interval);
    let interval = found.unwrap_or(0.0);
    assert!((2.5..2.5 * 1.06).contains(&interval), "found {interval}");
}

#[test]
fn a_search_above_ten_seconds_looks_at_longer_intervals() {
    // A stale rate of 0.8 / interval crosses 4% at 20 seconds; the bracket
    // from 10 to 40 seconds narrows to about 2%.
    let found = shortest_interval_within(0.04, 10.0, 0.25, 40.0, 6, |interval| 0.8 / interval);
    let interval = found.unwrap_or(0.0);
    assert!((20.0..20.0 * 1.03).contains(&interval), "found {interval}");
}

#[test]
fn a_search_returns_the_shortest_interval_when_that_is_already_within() {
    let found = shortest_interval_within(0.04, 10.0, 0.25, 40.0, 6, |_| 0.0);
    assert_eq!(found, Some(0.25));
}

#[test]
fn a_search_returns_nothing_when_even_the_longest_interval_is_above() {
    let found = shortest_interval_within(0.04, 10.0, 0.25, 40.0, 6, |_| 1.0);
    assert_eq!(found, None);
}

#[test]
fn a_search_runs_twice_and_then_once_a_step() {
    let mut runs = 0;
    let found = shortest_interval_within(0.04, 10.0, 0.25, 40.0, 6, |interval| {
        runs += 1;
        0.1 / interval
    });
    assert!(found.is_some());
    assert_eq!(runs, 8);
}

#[test]
fn a_search_never_returns_an_interval_above_the_ceiling() {
    // A stale rate that jumps at 3 seconds: everything shorter is far above.
    let found = shortest_interval_within(0.04, 10.0, 0.25, 40.0, 6, |interval| {
        if interval < 3.0 { 0.5 } else { 0.01 }
    });
    let interval = found.unwrap_or(0.0);
    assert!((3.0..3.0 * 1.06).contains(&interval), "found {interval}");
}

#[test]
fn the_grid_runs_each_variant_at_both_node_counts_and_four_intervals() {
    let scenarios = grid(&Idea::Today.variants(&Scenario::default()));
    assert_eq!(scenarios.len(), 8);
    assert_eq!(scenarios[0].label, "today, 100 nodes, 10 s shares");
    assert_eq!(scenarios[3].label, "today, 100 nodes, 1 s shares");
    assert_eq!(scenarios[7].label, "today, 1,000 nodes, 1 s shares");
    assert_eq!(scenarios[7].node_count, 1_000);
    assert_eq!(scenarios[7].share_interval_seconds, 1.0);
}

#[test]
fn a_small_network_mines_for_the_base_duration() {
    let scenario = at_size(&Scenario::default(), 100, 1.0);
    assert_eq!(scenario.duration_seconds, 86_400.0);
}

#[test]
fn a_large_network_mines_for_8640_share_intervals() {
    let scenario = at_size(&Scenario::default(), 1_000, 2.0);
    assert_eq!(scenario.duration_seconds, 17_280.0);
}

#[test]
fn a_large_network_at_ten_second_shares_mines_for_a_day_like_the_nodes_table() {
    let scenario = at_size(&Scenario::default(), 1_000, 10.0);
    assert_eq!(scenario.duration_seconds, 86_400.0);
}

#[test]
fn today_is_the_default_scenario() {
    let variants = Idea::Today.variants(&Scenario::default());
    assert_eq!(variants.len(), 1);
    assert_eq!(
        variants[0],
        Scenario {
            label: "today".to_string(),
            ..Scenario::default()
        }
    );
}

#[test]
fn the_weak_share_idea_tries_three_counts_gossiped_and_carried_whole() {
    let variants = Idea::WeakShares.variants(&Scenario::default());
    assert_eq!(variants.len(), 6);
    assert_eq!(variants[0].label, "16 weak shares a share, gossip only");
    assert_eq!(
        variants[0].weak_share_carriage,
        WeakShareCarriage::GossipOnly
    );
    assert_eq!(variants[5].label, "64 weak shares a share, carried whole");
    assert_eq!(variants[5].weak_shares_per_share, 64);
    assert_eq!(variants[5].weak_share_carriage, WeakShareCarriage::Whole);
}

#[test]
fn the_compact_coinbase_idea_compares_with_3_kb_and_200_kib_coinbases() {
    let variants = Idea::CompactCoinbase.variants(&Scenario::default());
    assert_eq!(variants.len(), 4);
    assert_eq!(variants[0].share_sizes().coinbase_bytes(), 3_051);
    assert_eq!(variants[0].relay, ShareRelay::Full);
    assert_eq!(variants[1].share_sizes().coinbase_bytes(), 204_663);
    assert_eq!(variants[1].relay, ShareRelay::Full);
    assert_eq!(variants[2].share_sizes().coinbase_bytes(), 34_357);
    assert_eq!(variants[2].relay, ShareRelay::WithoutPayouts);
    assert_eq!(variants[3].share_sizes().coinbase_bytes(), 204_663);
    assert_eq!(variants[3].relay, ShareRelay::WithoutPayouts);
}

#[test]
fn the_combined_idea_sends_without_payouts_and_forwards_after_the_header_check() {
    let variants = Idea::Combined.variants(&Scenario::default());
    assert_eq!(variants.len(), 4);
    assert_eq!(variants[3].label, "without payouts, header check, 64 peers");
    assert_eq!(variants[3].relay, ShareRelay::WithoutPayouts);
    assert_eq!(variants[3].peers_per_node, 64);
    assert_eq!(
        variants[3].forwarding,
        Forwarding::AfterHeaderCheck {
            header_check_seconds: HEADER_CHECK_SECONDS,
        }
    );
    assert_eq!(variants[0].peers_per_node, 8);
}

#[test]
fn the_uncle_cap_and_peer_ideas_change_only_their_own_input() {
    let base = Scenario::default();
    let caps = Idea::UncleCap.variants(&base);
    assert_eq!(caps.len(), 2);
    assert_eq!(
        caps[1],
        Scenario {
            label: "uncle cap 12".to_string(),
            max_uncles: 12,
            ..base.clone()
        }
    );
    let peers = Idea::PeerDegree.variants(&base);
    assert_eq!(peers.len(), 3);
    assert_eq!(
        peers[0],
        Scenario {
            label: "16 peers".to_string(),
            peers_per_node: 16,
            ..base
        }
    );
}
