// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;

#[test]
fn the_default_scenario_is_valid() {
    assert_eq!(Scenario::default().validate(), Ok(()));
}

#[test]
fn a_scenario_without_nodes_is_refused() {
    let scenario = Scenario {
        node_count: 0,
        ..Scenario::default()
    };
    assert!(scenario.validate().is_err());
}

#[test]
fn a_scenario_with_a_zero_share_interval_is_refused() {
    let scenario = Scenario {
        share_interval_seconds: 0.0,
        ..Scenario::default()
    };
    assert!(scenario.validate().is_err());
}

#[test]
fn a_pool_fraction_above_one_is_refused() {
    let scenario = Scenario {
        pool_fraction_of_bch: 1.5,
        ..Scenario::default()
    };
    assert!(scenario.validate().is_err());
}

/// The thousand miners, and the donation key every share's default donation
/// pays.
#[test]
fn a_thousand_miners_in_an_18000_share_window_are_all_paid() {
    let scenario = Scenario::default();
    assert_eq!(scenario.window_shares(), 18_000);
    assert_eq!(scenario.payees(), 1_001);
}

#[test]
fn a_million_miners_at_a_tenth_of_bch_fill_at_most_the_window() {
    let scenario = Scenario {
        miner_count: 1_000_000,
        pool_fraction_of_bch: 0.1,
        ..Scenario::default()
    };
    // 1e6 * (1 - exp(1,800 * ln(1 - 1e-6))) = 1,798.4 miners, and the
    // donation key.
    assert_eq!(scenario.payees(), 1_799);
}

/// Under the cap-11 rule the donation was an output of its own, not a payee.
#[test]
fn the_cap_11_rule_counts_only_miners_as_payees() {
    let scenario = Scenario {
        layout: CoinbaseLayout::cap_11(),
        ..Scenario::default()
    };
    assert_eq!(scenario.payees(), 1_000);
}

#[test]
fn a_payee_override_wins_over_the_miners() {
    let scenario = Scenario {
        payees_override: Some(42),
        ..Scenario::default()
    };
    assert_eq!(scenario.payees(), 42);
}

#[test]
fn the_default_share_pays_a_thousand_payees_directly() {
    let sizes = Scenario::default().share_sizes();
    assert_eq!(sizes.split.direct_outputs, 1_001);
    // 321 + 1,001 * 34 + 2 more bytes of output count.
    assert_eq!(sizes.coinbase_bytes(), 34_357);
    // 32 MB / 226 = 141,592 transactions: an 18-hash branch.
    assert_eq!(sizes.merkle_branch_hashes, 18);
}

#[test]
fn the_network_has_the_requested_nodes_and_links() {
    let scenario = Scenario::default();
    let mut random = SimulationRandom::from_seed(1);
    let (network, summary) = scenario.build_network(0.0, &mut random);
    assert_eq!(network.nodes.len(), 100);
    assert_eq!(summary.links, 400);
    assert!(summary.connected);
    assert!((summary.mean_peers - 8.0).abs() < 1e-12);
    let total_hashrate: f64 = network
        .nodes
        .iter()
        .map(|node| node.hashrate_fraction)
        .sum();
    assert!((total_hashrate - 1.0).abs() < 1e-9);
}

#[test]
fn both_ends_of_a_link_see_the_same_latency() {
    let scenario = Scenario::default();
    let mut random = SimulationRandom::from_seed(2);
    let (network, _) = scenario.build_network(0.0, &mut random);
    let mismatched = network
        .peers
        .iter()
        .enumerate()
        .flat_map(|(node, links)| links.iter().map(move |link| (node, *link)))
        .filter(|(node, link)| {
            network.peers[link.peer]
                .iter()
                .find(|back| back.peer == *node)
                .map(|back| back.latency_seconds)
                != Some(link.latency_seconds)
        })
        .count();
    assert_eq!(mismatched, 0);
}

#[test]
fn bchn_relay_at_two_gigabytes_leaves_some_mixed_links_short() {
    let scenario = Scenario {
        block_bytes: 2_000_000_000,
        ..Scenario::default()
    };
    let load = scenario.bchn_load(782);
    let mut random = SimulationRandom::from_seed(3);
    let (_, summary) = scenario.build_network(load.relay_bytes_per_second, &mut random);
    assert!(
        summary.nodes_short_for_bchn > 10,
        "short {}",
        summary.nodes_short_for_bchn
    );
}

#[test]
fn nodes_short_for_bchn_mine_none_of_the_pools_hashrate() {
    let scenario = Scenario {
        block_bytes: 2_000_000_000,
        ..Scenario::default()
    };
    let load = scenario.bchn_load(782);
    let mut random = SimulationRandom::from_seed(3);
    let (network, summary) = scenario.build_network(load.relay_bytes_per_second, &mut random);
    let mining_nodes = network
        .nodes
        .iter()
        .filter(|node| node.hashrate_fraction > 0.0)
        .count();
    assert_eq!(mining_nodes, 100 - summary.nodes_short_for_bchn);
    let total: f64 = network
        .nodes
        .iter()
        .map(|node| node.hashrate_fraction)
        .sum();
    assert!((total - 1.0).abs() < 1e-9, "total {total}");
    let expected_short = summary.nodes_short_for_bchn as f64 / 100.0;
    assert!((summary.hashrate_short_for_bchn - expected_short).abs() < 1e-9);
}

#[test]
fn a_separate_bchn_link_leaves_the_whole_link_to_p2pool() {
    let scenario = Scenario {
        upload_megabits: Distribution::fixed(100.0),
        download_megabits: Distribution::fixed(100.0),
        bchn_shares_link: false,
        ..Scenario::default()
    };
    let mut random = SimulationRandom::from_seed(4);
    let (network, summary) = scenario.build_network(5_000_000.0, &mut random);
    assert_eq!(network.nodes[0].upload_bytes_per_second, 12_500_000.0);
    assert_eq!(summary.nodes_short_for_bchn, 0);
}

#[test]
fn a_shared_link_gives_bchn_its_relay_first() {
    let scenario = Scenario {
        upload_megabits: Distribution::fixed(100.0),
        download_megabits: Distribution::fixed(100.0),
        ..Scenario::default()
    };
    let mut random = SimulationRandom::from_seed(5);
    let (network, _) = scenario.build_network(5_000_000.0, &mut random);
    assert_eq!(network.nodes[0].upload_bytes_per_second, 7_500_000.0);
}

#[test]
fn a_link_bchn_fills_keeps_five_percent_for_p2pool() {
    assert_eq!(left_for_p2pool(1_000.0, 5_000.0), 50.0);
}

#[test]
fn a_run_is_reproducible() {
    let scenario = Scenario {
        node_count: 20,
        duration_seconds: 1_800.0,
        ..Scenario::default()
    };
    assert_eq!(scenario.run(), scenario.run());
}

#[test]
fn a_shuffle_keeps_every_value() {
    let mut values = vec![1.0, 2.0, 3.0, 4.0, 5.0];
    let mut random = SimulationRandom::from_seed(6);
    shuffle(&mut values, &mut random);
    values.sort_by(f64::total_cmp);
    assert_eq!(values, vec![1.0, 2.0, 3.0, 4.0, 5.0]);
}
