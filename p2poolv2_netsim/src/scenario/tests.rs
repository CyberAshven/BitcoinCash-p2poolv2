// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;
use crate::constants::scaled_to_block_interval;

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

/// The miners with a share in the window, and the donation key every
/// share's default donation pays.
#[test]
fn the_payees_are_the_miners_expected_in_the_window_and_the_donation_key() {
    let scenario = Scenario::default();
    let miners = expected_payees(1_000, HashrateSpread::Equal, scenario.window_shares());
    assert_eq!(scenario.payees(), miners.round() as u64 + 1);
}

/// With one-minute blocks the default window of 1% of BCH holds 1,800
/// shares, so a thousand equal miners are not all in it:
/// 1,000 * (1 - exp(1,800 * ln(1 - 1/1,000))) = 834.8 of them are, with
/// the donation key 836 payees. (Ten-minute blocks gave an 18,000 share
/// window that paid all thousand.)
#[test]
fn a_thousand_miners_in_an_1800_share_window_make_836_payees() {
    let scenario = Scenario::default();
    assert_eq!(scenario.window_shares(), 1_800);
    assert_eq!(scenario.payees(), 836);
}

#[test]
fn a_million_miners_at_a_tenth_of_bch_fill_at_most_the_window() {
    let scenario = Scenario {
        miner_count: 1_000_000,
        pool_fraction_of_bch: 0.1,
        ..Scenario::default()
    };
    // 1e6 * (1 - exp(180 * ln(1 - 1e-6))) = 180.0 miners in a 180 share
    // window, and the donation key.
    assert_eq!(scenario.window_shares(), 180);
    assert_eq!(scenario.payees(), 181);
}

/// Under the cap-11 rule the donation was an output of its own, not a payee.
#[test]
fn the_cap_11_rule_counts_only_miners_as_payees() {
    let scenario = Scenario {
        layout: CoinbaseLayout::cap_11(),
        ..Scenario::default()
    };
    assert_eq!(scenario.payees(), 835);
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
fn the_default_share_pays_its_836_payees_directly() {
    let sizes = Scenario::default().share_sizes();
    assert_eq!(sizes.split.direct_outputs, 836);
    // 321 + 836 * 34 + 2 more bytes of output count.
    assert_eq!(sizes.coinbase_bytes(), 28_747);
    // 3.2 MB / 226 = 14,159 transactions: a 14-hash branch.
    assert_eq!(sizes.merkle_branch_hashes, 14);
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

/// Blocks carrying 2 GB every ten minutes: 200 MB one-minute blocks, about
/// 48 Mbit/s of BCHN relay each way.
fn two_gigabytes_every_ten_minutes() -> Scenario {
    Scenario {
        block_bytes: scaled_to_block_interval(2_000_000_000),
        ..Scenario::default()
    }
}

#[test]
fn bchn_relay_of_two_gigabytes_every_ten_minutes_leaves_some_mixed_links_short() {
    let scenario = two_gigabytes_every_ten_minutes();
    let load = scenario.bchn_load(782);
    let mut random = SimulationRandom::from_seed(3);
    let (_, summary) = scenario.build_network(load.relay_bytes_per_second, &mut random);
    assert!(
        summary.nodes_short_for_bchn > 10 && summary.nodes_short_for_bchn < 100,
        "short {}",
        summary.nodes_short_for_bchn
    );
}

#[test]
fn bchn_relay_at_two_gigabyte_one_minute_blocks_leaves_every_mixed_link_short() {
    // About 479 Mbit/s each way, past every node of the default link mix.
    let scenario = Scenario {
        block_bytes: 2_000_000_000,
        ..Scenario::default()
    };
    let load = scenario.bchn_load(782);
    let mut random = SimulationRandom::from_seed(3);
    let (_, summary) = scenario.build_network(load.relay_bytes_per_second, &mut random);
    assert_eq!(summary.nodes_short_for_bchn, 100);
}

#[test]
fn nodes_short_for_bchn_mine_none_of_the_pools_hashrate() {
    let scenario = two_gigabytes_every_ten_minutes();
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

#[test]
fn the_default_scenario_forwards_after_validation_names_three_uncles_and_has_no_weak_shares() {
    let scenario = Scenario::default();
    let sizes = scenario.share_sizes();
    let parameters = scenario.parameters(&sizes, &scenario.bchn_load(sizes.coinbase_bytes()));
    assert_eq!(parameters.max_uncles, 3);
    assert_eq!(parameters.forwarding, Forwarding::AfterValidation);
    assert_eq!(parameters.weak_shares, None);
    assert_eq!(parameters.share_wire_bytes, 30_525);
}

#[test]
fn weak_shares_travel_as_framed_800_byte_messages_at_their_rate_per_share() {
    let scenario = Scenario {
        weak_shares_per_share: 32,
        ..Scenario::default()
    };
    let sizes = scenario.share_sizes();
    let parameters = scenario.parameters(&sizes, &scenario.bchn_load(sizes.coinbase_bytes()));
    // 800 bytes, 64 of framing and one 40-byte TCP/IP header.
    assert_eq!(
        parameters.weak_shares,
        Some(WeakShareLoad {
            per_share: 32.0,
            wire_bytes: 904,
        })
    );
}

#[test]
fn gossiped_weak_shares_leave_the_share_as_it_was() {
    let scenario = Scenario {
        weak_shares_per_share: 64,
        weak_share_carriage: WeakShareCarriage::GossipOnly,
        ..Scenario::default()
    };
    let sizes = scenario.share_sizes();
    let parameters = scenario.parameters(&sizes, &scenario.bchn_load(sizes.coinbase_bytes()));
    assert_eq!(parameters.share_wire_bytes, 30_525);
}

#[test]
fn weak_shares_carried_whole_add_their_payload_to_every_share() {
    let scenario = Scenario {
        weak_shares_per_share: 64,
        weak_share_carriage: WeakShareCarriage::Whole,
        ..Scenario::default()
    };
    let sizes = scenario.share_sizes();
    let parameters = scenario.parameters(&sizes, &scenario.bchn_load(sizes.coinbase_bytes()));
    let payload = sizes.message_payload_bytes(0) + 64 * 800;
    assert_eq!(parameters.share_wire_bytes, wire_bytes(payload));
    // The weak shares are checked when they arrive by gossip, so carrying
    // them adds no validation time.
    let gossiped = Scenario {
        weak_share_carriage: WeakShareCarriage::GossipOnly,
        ..scenario
    };
    let gossiped_parameters =
        gossiped.parameters(&sizes, &gossiped.bchn_load(sizes.coinbase_bytes()));
    assert_eq!(
        parameters.validation_seconds,
        gossiped_parameters.validation_seconds
    );
}

#[test]
fn shares_without_payouts_carry_whole_weak_shares_too() {
    let scenario = Scenario {
        relay: ShareRelay::WithoutPayouts,
        weak_shares_per_share: 16,
        weak_share_carriage: WeakShareCarriage::Whole,
        ..Scenario::default()
    };
    let sizes = scenario.share_sizes();
    let parameters = scenario.parameters(&sizes, &scenario.bchn_load(sizes.coinbase_bytes()));
    let payload = sizes.message_payload_bytes_without_payouts(0) + 16 * 800;
    assert_eq!(parameters.share_wire_bytes, wire_bytes(payload));
}

#[test]
fn the_scenario_passes_its_uncle_cap_and_forwarding_to_the_engine() {
    let scenario = Scenario {
        max_uncles: 12,
        forwarding: Forwarding::AfterHeaderCheck {
            header_check_seconds: 0.002,
        },
        ..Scenario::default()
    };
    let sizes = scenario.share_sizes();
    let parameters = scenario.parameters(&sizes, &scenario.bchn_load(sizes.coinbase_bytes()));
    assert_eq!(parameters.max_uncles, 12);
    assert_eq!(
        parameters.forwarding,
        Forwarding::AfterHeaderCheck {
            header_check_seconds: 0.002,
        }
    );
}

#[test]
fn a_header_check_taking_negative_time_is_refused() {
    let scenario = Scenario {
        forwarding: Forwarding::AfterHeaderCheck {
            header_check_seconds: -0.001,
        },
        ..Scenario::default()
    };
    assert!(scenario.validate().is_err());
}

#[test]
fn weak_share_carriage_parses_its_two_names() {
    assert_eq!(
        WeakShareCarriage::parse("gossip-only"),
        Ok(WeakShareCarriage::GossipOnly)
    );
    assert_eq!(
        WeakShareCarriage::parse("whole"),
        Ok(WeakShareCarriage::Whole)
    );
    assert!(WeakShareCarriage::parse("hashes").is_err());
}
