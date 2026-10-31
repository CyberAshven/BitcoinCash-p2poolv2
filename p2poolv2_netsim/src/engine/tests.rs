// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;

#[test]
fn the_same_seed_gives_the_same_outcome() {
    let ring: Vec<Vec<usize>> = (0..20)
        .map(|node| vec![(node + 19) % 20, (node + 1) % 20])
        .collect();
    let network = Network::uniform(&ring, 1_000_000.0, 4_000_000.0, 0.05);
    let parameters = Parameters {
        share_interval_seconds: 10.0,
        share_wire_bytes: 2_000,
        wire_bytes_per_uncle: 32,
        ack_wire_bytes: 105,
        validation_seconds: 0.005,
        work_switch_seconds: 0.05,
        duration_seconds: 3_600.0,
        bch_block: None,
    };
    let first = simulate(&network, parameters, SimulationRandom::from_seed(9));
    let second = simulate(&network, parameters, SimulationRandom::from_seed(9));
    assert_eq!(first, second);
    assert!(first.shares_found > 300, "found {}", first.shares_found);
}

#[test]
fn a_different_seed_gives_a_different_outcome() {
    let ring: Vec<Vec<usize>> = (0..20)
        .map(|node| vec![(node + 19) % 20, (node + 1) % 20])
        .collect();
    let network = Network::uniform(&ring, 1_000_000.0, 4_000_000.0, 0.05);
    let parameters = Parameters {
        share_interval_seconds: 10.0,
        share_wire_bytes: 2_000,
        wire_bytes_per_uncle: 32,
        ack_wire_bytes: 105,
        validation_seconds: 0.005,
        work_switch_seconds: 0.05,
        duration_seconds: 3_600.0,
        bch_block: None,
    };
    let first = simulate(&network, parameters, SimulationRandom::from_seed(9));
    let second = simulate(&network, parameters, SimulationRandom::from_seed(10));
    assert_ne!(first, second);
}

#[test]
fn instant_links_make_no_stale_shares() {
    let complete: Vec<Vec<usize>> = (0..10)
        .map(|node| (0..10).filter(|&peer| peer != node).collect())
        .collect();
    let network = Network::uniform(&complete, 1e15, 1e15, 0.0);
    let parameters = Parameters {
        share_interval_seconds: 10.0,
        share_wire_bytes: 2_000,
        wire_bytes_per_uncle: 32,
        ack_wire_bytes: 105,
        validation_seconds: 0.0,
        work_switch_seconds: 0.0,
        duration_seconds: 20_000.0,
        bch_block: None,
    };
    let outcome = simulate(&network, parameters, SimulationRandom::from_seed(1));
    assert!(
        outcome.shares_counted > 1_500,
        "counted {}",
        outcome.shares_counted
    );
    assert_eq!(outcome.stale_rate, 0.0);
    assert_eq!(outcome.chain_shares, outcome.shares_counted);
}

#[test]
fn a_single_node_without_delays_never_goes_stale_and_sends_nothing() {
    let alone: Vec<Vec<usize>> = vec![Vec::new()];
    let network = Network::uniform(&alone, 1_000_000.0, 1_000_000.0, 0.05);
    let parameters = Parameters {
        share_interval_seconds: 10.0,
        share_wire_bytes: 2_000,
        wire_bytes_per_uncle: 32,
        ack_wire_bytes: 105,
        validation_seconds: 0.0,
        work_switch_seconds: 0.0,
        duration_seconds: 3_600.0,
        bch_block: None,
    };
    let outcome = simulate(&network, parameters, SimulationRandom::from_seed(2));
    assert_eq!(outcome.stale_rate, 0.0);
    assert_eq!(outcome.share_messages, 0);
    assert_eq!(outcome.node_upload_bytes_per_second, vec![0.0]);
}

#[test]
fn a_single_node_goes_stale_while_its_miners_switch_work() {
    // A share found in the second after the node's own last share, before
    // its miners have the new job, builds on the old tip: about
    // 1 - exp(-1/10) of shares, 9.5%.
    let alone: Vec<Vec<usize>> = vec![Vec::new()];
    let network = Network::uniform(&alone, 1_000_000.0, 1_000_000.0, 0.05);
    let parameters = Parameters {
        share_interval_seconds: 10.0,
        share_wire_bytes: 2_000,
        wire_bytes_per_uncle: 32,
        ack_wire_bytes: 105,
        validation_seconds: 0.0,
        work_switch_seconds: 1.0,
        duration_seconds: 200_000.0,
        bch_block: None,
    };
    let outcome = simulate(&network, parameters, SimulationRandom::from_seed(2));
    assert!(
        (0.08..0.11).contains(&outcome.stale_rate),
        "stale {}",
        outcome.stale_rate
    );
}

#[test]
fn two_nodes_a_second_apart_go_stale_about_five_percent_of_the_time() {
    // Each node finds a share every 20 s on average, so the other one finds
    // a conflicting share in the second before it hears of one with
    // probability 1 - exp(-1/20), about 4.9%. Both hear of both shares
    // within a second, so the loser is named as an uncle, not orphaned.
    let pair: Vec<Vec<usize>> = vec![vec![1], vec![0]];
    let network = Network::uniform(&pair, 1e12, 1e12, 1.0);
    let parameters = Parameters {
        share_interval_seconds: 10.0,
        share_wire_bytes: 2_000,
        wire_bytes_per_uncle: 32,
        ack_wire_bytes: 105,
        validation_seconds: 0.0,
        work_switch_seconds: 0.0,
        duration_seconds: 200_000.0,
        bch_block: None,
    };
    let outcome = simulate(&network, parameters, SimulationRandom::from_seed(3));
    assert!(
        (0.035..0.065).contains(&outcome.stale_rate),
        "stale {}",
        outcome.stale_rate
    );
    assert!(
        outcome.orphan_rate < 0.005,
        "orphans {}",
        outcome.orphan_rate
    );
    assert_eq!(
        outcome.chain_shares + outcome.uncle_shares + outcome.orphan_shares,
        outcome.shares_counted
    );
}

#[test]
fn slower_links_make_more_stale_shares() {
    let ring: Vec<Vec<usize>> = (0..10)
        .map(|node| vec![(node + 9) % 10, (node + 1) % 10])
        .collect();
    let fast = Network::uniform(&ring, 1e9, 1e9, 0.1);
    let slow = Network::uniform(&ring, 1e9, 1e9, 2.0);
    let parameters = Parameters {
        share_interval_seconds: 10.0,
        share_wire_bytes: 2_000,
        wire_bytes_per_uncle: 32,
        ack_wire_bytes: 105,
        validation_seconds: 0.005,
        work_switch_seconds: 0.05,
        duration_seconds: 50_000.0,
        bch_block: None,
    };
    let fast_outcome = simulate(&fast, parameters, SimulationRandom::from_seed(4));
    let slow_outcome = simulate(&slow, parameters, SimulationRandom::from_seed(4));
    assert!(
        slow_outcome.stale_rate > 2.0 * fast_outcome.stale_rate,
        "slow {} fast {}",
        slow_outcome.stale_rate,
        fast_outcome.stale_rate
    );
    assert!(slow_outcome.mean_seconds_to_all > fast_outcome.mean_seconds_to_all);
}

#[test]
fn a_two_node_link_carries_each_share_once_and_an_ack_back() {
    let pair: Vec<Vec<usize>> = vec![vec![1], vec![0]];
    let network = Network::uniform(&pair, 1e12, 1e12, 0.0);
    let parameters = Parameters {
        share_interval_seconds: 10.0,
        share_wire_bytes: 2_000,
        wire_bytes_per_uncle: 32,
        ack_wire_bytes: 105,
        validation_seconds: 0.0,
        work_switch_seconds: 0.0,
        duration_seconds: 10_000.0,
        bch_block: None,
    };
    let outcome = simulate(&network, parameters, SimulationRandom::from_seed(5));
    assert_eq!(outcome.share_messages, outcome.shares_found);
    assert_eq!(outcome.duplicate_share_messages, 0);
    let uploaded: f64 = outcome.node_upload_bytes_per_second.iter().sum::<f64>() * 10_000.0;
    let expected = outcome.shares_found as f64 * (2_000.0 + 105.0);
    assert!((uploaded - expected).abs() < 1e-3, "uploaded {uploaded}");
}

#[test]
fn a_triangle_pushes_every_share_twice_more_than_needed() {
    // The finder pushes to both peers; each peer, not knowing the other has
    // it, pushes it on, so the third and fourth copies are duplicates.
    let triangle: Vec<Vec<usize>> = vec![vec![1, 2], vec![0, 2], vec![0, 1]];
    let network = Network::uniform(&triangle, 1e12, 1e12, 0.0);
    let parameters = Parameters {
        share_interval_seconds: 10.0,
        share_wire_bytes: 2_000,
        wire_bytes_per_uncle: 32,
        ack_wire_bytes: 105,
        validation_seconds: 0.0,
        work_switch_seconds: 0.0,
        duration_seconds: 2_000.0,
        bch_block: None,
    };
    let outcome = simulate(&network, parameters, SimulationRandom::from_seed(6));
    assert_eq!(outcome.share_messages, 4 * outcome.shares_found);
    assert_eq!(outcome.duplicate_share_messages, 2 * outcome.shares_found);
}

#[test]
fn crossing_shares_become_uncles() {
    let pair: Vec<Vec<usize>> = vec![vec![1], vec![0]];
    let network = Network::uniform(&pair, 1e12, 1e12, 3.0);
    let parameters = Parameters {
        share_interval_seconds: 10.0,
        share_wire_bytes: 2_000,
        wire_bytes_per_uncle: 32,
        ack_wire_bytes: 105,
        validation_seconds: 0.0,
        work_switch_seconds: 0.0,
        duration_seconds: 20_000.0,
        bch_block: None,
    };
    let outcome = simulate(&network, parameters, SimulationRandom::from_seed(7));
    assert!(outcome.uncle_shares > 0);
    assert!(outcome.uncle_shares > outcome.orphan_shares);
}

#[test]
fn bch_blocks_on_a_slow_link_delay_shares() {
    let ring: Vec<Vec<usize>> = (0..10)
        .map(|node| vec![(node + 9) % 10, (node + 1) % 10])
        .collect();
    let network = Network::uniform(&ring, 1_000_000.0, 1_000_000.0, 0.05);
    let without = Parameters {
        share_interval_seconds: 10.0,
        share_wire_bytes: 2_000,
        wire_bytes_per_uncle: 32,
        ack_wire_bytes: 105,
        validation_seconds: 0.005,
        work_switch_seconds: 0.05,
        duration_seconds: 36_000.0,
        bch_block: None,
    };
    let with = Parameters {
        bch_block: Some(BchBlockLoad {
            interval_seconds: 600.0,
            compact_block_bytes: 50_000_000,
        }),
        ..without
    };
    let quiet = simulate(&network, without, SimulationRandom::from_seed(8));
    let busy = simulate(&network, with, SimulationRandom::from_seed(8));
    assert!(
        busy.mean_seconds_to_all > quiet.mean_seconds_to_all,
        "busy {} quiet {}",
        busy.mean_seconds_to_all,
        quiet.mean_seconds_to_all
    );
}

#[test]
fn an_upload_too_slow_for_the_shares_is_reported_saturated() {
    let ring: Vec<Vec<usize>> = (0..10)
        .map(|node| vec![(node + 9) % 10, (node + 1) % 10])
        .collect();
    let network = Network::uniform(&ring, 100.0, 100.0, 0.05);
    let parameters = Parameters {
        share_interval_seconds: 10.0,
        share_wire_bytes: 2_000,
        wire_bytes_per_uncle: 32,
        ack_wire_bytes: 105,
        validation_seconds: 0.005,
        work_switch_seconds: 0.05,
        duration_seconds: 3_600.0,
        bch_block: None,
    };
    let outcome = simulate(&network, parameters, SimulationRandom::from_seed(11));
    assert!(outcome.saturated_nodes > 0);
}

#[test]
fn a_node_without_hashrate_finds_no_shares() {
    let pair: Vec<Vec<usize>> = vec![vec![1], vec![0]];
    let mut network = Network::uniform(&pair, 1e12, 1e12, 0.0);
    network.nodes[0].hashrate_fraction = 1.0;
    network.nodes[1].hashrate_fraction = 0.0;
    let parameters = Parameters {
        share_interval_seconds: 10.0,
        share_wire_bytes: 2_000,
        wire_bytes_per_uncle: 32,
        ack_wire_bytes: 105,
        validation_seconds: 0.0,
        work_switch_seconds: 0.0,
        duration_seconds: 10_000.0,
        bch_block: None,
    };
    let outcome = simulate(&network, parameters, SimulationRandom::from_seed(12));
    // Node 1 only answers with acks.
    let node_one_bytes = outcome.node_upload_bytes_per_second[1] * 10_000.0;
    assert!((node_one_bytes - outcome.shares_found as f64 * 105.0).abs() < 1e-3);
}

#[test]
fn quantiles_take_the_value_at_their_rank() {
    let sorted = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
    assert_eq!(quantile(&sorted, 0.5), 5.0);
    assert_eq!(quantile(&sorted, 0.9), 9.0);
    assert_eq!(quantile(&sorted, 1.0), 10.0);
    assert_eq!(quantile(&[4.0], 0.5), 4.0);
}

#[test]
#[should_panic(expected = "a network needs a node")]
fn an_empty_network_is_refused() {
    let network = Network {
        nodes: Vec::new(),
        peers: Vec::new(),
    };
    let parameters = Parameters {
        share_interval_seconds: 10.0,
        share_wire_bytes: 2_000,
        wire_bytes_per_uncle: 32,
        ack_wire_bytes: 105,
        validation_seconds: 0.0,
        work_switch_seconds: 0.0,
        duration_seconds: 100.0,
        bch_block: None,
    };
    simulate(&network, parameters, SimulationRandom::from_seed(13));
}
