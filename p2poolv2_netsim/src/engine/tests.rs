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
        validation_queue: ValidationQueue::Parallel,
        work_switch_seconds: 0.05,
        duration_seconds: 3_600.0,
        bch_block: None,
        max_uncles: 3,
        forwarding: Forwarding::AfterValidation,
        weak_shares: None,
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
        validation_queue: ValidationQueue::Parallel,
        work_switch_seconds: 0.05,
        duration_seconds: 3_600.0,
        bch_block: None,
        max_uncles: 3,
        forwarding: Forwarding::AfterValidation,
        weak_shares: None,
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
        validation_queue: ValidationQueue::Parallel,
        work_switch_seconds: 0.0,
        duration_seconds: 20_000.0,
        bch_block: None,
        max_uncles: 3,
        forwarding: Forwarding::AfterValidation,
        weak_shares: None,
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
        validation_queue: ValidationQueue::Parallel,
        work_switch_seconds: 0.0,
        duration_seconds: 3_600.0,
        bch_block: None,
        max_uncles: 3,
        forwarding: Forwarding::AfterValidation,
        weak_shares: None,
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
        validation_queue: ValidationQueue::Parallel,
        work_switch_seconds: 1.0,
        duration_seconds: 200_000.0,
        bch_block: None,
        max_uncles: 3,
        forwarding: Forwarding::AfterValidation,
        weak_shares: None,
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
        validation_queue: ValidationQueue::Parallel,
        work_switch_seconds: 0.0,
        duration_seconds: 200_000.0,
        bch_block: None,
        max_uncles: 3,
        forwarding: Forwarding::AfterValidation,
        weak_shares: None,
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
        validation_queue: ValidationQueue::Parallel,
        work_switch_seconds: 0.05,
        duration_seconds: 50_000.0,
        bch_block: None,
        max_uncles: 3,
        forwarding: Forwarding::AfterValidation,
        weak_shares: None,
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
        validation_queue: ValidationQueue::Parallel,
        work_switch_seconds: 0.0,
        duration_seconds: 10_000.0,
        bch_block: None,
        max_uncles: 3,
        forwarding: Forwarding::AfterValidation,
        weak_shares: None,
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
        validation_queue: ValidationQueue::Parallel,
        work_switch_seconds: 0.0,
        duration_seconds: 2_000.0,
        bch_block: None,
        max_uncles: 3,
        forwarding: Forwarding::AfterValidation,
        weak_shares: None,
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
        validation_queue: ValidationQueue::Parallel,
        work_switch_seconds: 0.0,
        duration_seconds: 20_000.0,
        bch_block: None,
        max_uncles: 3,
        forwarding: Forwarding::AfterValidation,
        weak_shares: None,
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
        validation_queue: ValidationQueue::Parallel,
        work_switch_seconds: 0.05,
        duration_seconds: 36_000.0,
        bch_block: None,
        max_uncles: 3,
        forwarding: Forwarding::AfterValidation,
        weak_shares: None,
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
        validation_queue: ValidationQueue::Parallel,
        work_switch_seconds: 0.05,
        duration_seconds: 3_600.0,
        bch_block: None,
        max_uncles: 3,
        forwarding: Forwarding::AfterValidation,
        weak_shares: None,
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
        validation_queue: ValidationQueue::Parallel,
        work_switch_seconds: 0.0,
        duration_seconds: 10_000.0,
        bch_block: None,
        max_uncles: 3,
        forwarding: Forwarding::AfterValidation,
        weak_shares: None,
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
        validation_queue: ValidationQueue::Parallel,
        work_switch_seconds: 0.0,
        duration_seconds: 100.0,
        bch_block: None,
        max_uncles: 3,
        forwarding: Forwarding::AfterValidation,
        weak_shares: None,
    };
    simulate(&network, parameters, SimulationRandom::from_seed(13));
}

#[test]
fn an_uncle_cap_of_one_lets_no_share_name_two_uncles() {
    // One-second shares over one-second links race all the time, so many
    // shares could name several uncles.
    let ring: Vec<Vec<usize>> = (0..10)
        .map(|node| vec![(node + 9) % 10, (node + 1) % 10])
        .collect();
    let network = Network::uniform(&ring, 1e12, 1e12, 1.0);
    let parameters = Parameters {
        share_interval_seconds: 1.0,
        share_wire_bytes: 2_000,
        wire_bytes_per_uncle: 32,
        ack_wire_bytes: 105,
        validation_seconds: 0.005,
        validation_queue: ValidationQueue::Parallel,
        work_switch_seconds: 0.05,
        duration_seconds: 2_000.0,
        bch_block: None,
        max_uncles: 1,
        forwarding: Forwarding::AfterValidation,
        weak_shares: None,
    };
    let mut simulation = Simulation::new(&network, parameters, SimulationRandom::from_seed(21));
    simulation.run();
    let most_uncles = simulation
        .shares
        .iter()
        .map(|share| share.uncles.len())
        .max();
    assert_eq!(most_uncles, Some(1));
}

#[test]
fn an_uncle_cap_of_twelve_names_more_than_three_uncles_when_shares_race() {
    let ring: Vec<Vec<usize>> = (0..10)
        .map(|node| vec![(node + 9) % 10, (node + 1) % 10])
        .collect();
    let network = Network::uniform(&ring, 1e12, 1e12, 1.0);
    let parameters = Parameters {
        share_interval_seconds: 1.0,
        share_wire_bytes: 2_000,
        wire_bytes_per_uncle: 32,
        ack_wire_bytes: 105,
        validation_seconds: 0.005,
        validation_queue: ValidationQueue::Parallel,
        work_switch_seconds: 0.05,
        duration_seconds: 2_000.0,
        bch_block: None,
        max_uncles: 12,
        forwarding: Forwarding::AfterValidation,
        weak_shares: None,
    };
    let mut simulation = Simulation::new(&network, parameters, SimulationRandom::from_seed(21));
    simulation.run();
    let most_uncles = simulation
        .shares
        .iter()
        .map(|share| share.uncles.len())
        .max()
        .unwrap_or(0);
    assert!((4..=12).contains(&most_uncles), "most uncles {most_uncles}");
}

#[test]
fn a_chain_without_races_names_no_uncles_and_holds_every_share() {
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
        validation_queue: ValidationQueue::Parallel,
        work_switch_seconds: 0.0,
        duration_seconds: 20_000.0,
        bch_block: None,
        max_uncles: 3,
        forwarding: Forwarding::AfterValidation,
        weak_shares: None,
    };
    let outcome = simulate(&network, parameters, SimulationRandom::from_seed(1));
    assert_eq!(outcome.chain_length, outcome.shares_found);
    assert_eq!(outcome.uncles_named_by_chain, 0);
    assert_eq!(outcome.uncle_hashes_in_named_uncles, 0);
}

#[test]
fn the_chain_names_at_least_the_counted_uncles() {
    let pair: Vec<Vec<usize>> = vec![vec![1], vec![0]];
    let network = Network::uniform(&pair, 1e12, 1e12, 3.0);
    let parameters = Parameters {
        share_interval_seconds: 10.0,
        share_wire_bytes: 2_000,
        wire_bytes_per_uncle: 32,
        ack_wire_bytes: 105,
        validation_seconds: 0.0,
        validation_queue: ValidationQueue::Parallel,
        work_switch_seconds: 0.0,
        duration_seconds: 20_000.0,
        bch_block: None,
        max_uncles: 3,
        forwarding: Forwarding::AfterValidation,
        weak_shares: None,
    };
    let outcome = simulate(&network, parameters, SimulationRandom::from_seed(7));
    assert!(outcome.uncle_shares > 0);
    assert!(outcome.uncles_named_by_chain >= outcome.uncle_shares);
    assert!(outcome.chain_length >= outcome.chain_shares);
    assert!(outcome.chain_length + outcome.uncles_named_by_chain <= outcome.shares_found);
}

#[test]
fn forwarding_after_validation_validates_at_every_hop_of_a_line() {
    // Only node 0 mines, links are instant and validation takes a second,
    // so every share reaches the end of a three-node line after three
    // validations.
    let line: Vec<Vec<usize>> = vec![vec![1], vec![0, 2], vec![1]];
    let mut network = Network::uniform(&line, 1e12, 1e12, 0.0);
    network.nodes[0].hashrate_fraction = 1.0;
    network.nodes[1].hashrate_fraction = 0.0;
    network.nodes[2].hashrate_fraction = 0.0;
    let parameters = Parameters {
        share_interval_seconds: 100.0,
        share_wire_bytes: 2_000,
        wire_bytes_per_uncle: 32,
        ack_wire_bytes: 105,
        validation_seconds: 1.0,
        validation_queue: ValidationQueue::Parallel,
        work_switch_seconds: 0.0,
        duration_seconds: 20_000.0,
        bch_block: None,
        max_uncles: 3,
        forwarding: Forwarding::AfterValidation,
        weak_shares: None,
    };
    let outcome = simulate(&network, parameters, SimulationRandom::from_seed(14));
    assert!(outcome.shares_counted > 100);
    assert!(
        (outcome.mean_seconds_to_all - 3.0).abs() < 1e-6,
        "to all {}",
        outcome.mean_seconds_to_all
    );
}

#[test]
fn forwarding_after_the_header_check_validates_once_along_a_line() {
    // The same line: each hop now waits only for the header check, and the
    // last node validates the body once, so a share reaches it after three
    // header checks and one validation.
    let line: Vec<Vec<usize>> = vec![vec![1], vec![0, 2], vec![1]];
    let mut network = Network::uniform(&line, 1e12, 1e12, 0.0);
    network.nodes[0].hashrate_fraction = 1.0;
    network.nodes[1].hashrate_fraction = 0.0;
    network.nodes[2].hashrate_fraction = 0.0;
    let parameters = Parameters {
        share_interval_seconds: 100.0,
        share_wire_bytes: 2_000,
        wire_bytes_per_uncle: 32,
        ack_wire_bytes: 105,
        validation_seconds: 1.0,
        validation_queue: ValidationQueue::Parallel,
        work_switch_seconds: 0.0,
        duration_seconds: 20_000.0,
        bch_block: None,
        max_uncles: 3,
        forwarding: Forwarding::AfterHeaderCheck {
            header_check_seconds: 0.001,
        },
        weak_shares: None,
    };
    let outcome = simulate(&network, parameters, SimulationRandom::from_seed(14));
    assert!(outcome.shares_counted > 100);
    assert!(
        (outcome.mean_seconds_to_all - 1.003).abs() < 1e-6,
        "to all {}",
        outcome.mean_seconds_to_all
    );
}

#[test]
fn forwarding_after_the_header_check_sends_each_share_to_each_peer_once() {
    // As after validation, the finder pushes to both peers and each peer
    // pushes on to the other: four copies, two of them duplicates, and no
    // second push once the body is validated.
    let triangle: Vec<Vec<usize>> = vec![vec![1, 2], vec![0, 2], vec![0, 1]];
    let network = Network::uniform(&triangle, 1e12, 1e12, 0.0);
    let parameters = Parameters {
        share_interval_seconds: 10.0,
        share_wire_bytes: 2_000,
        wire_bytes_per_uncle: 32,
        ack_wire_bytes: 105,
        validation_seconds: 0.0,
        validation_queue: ValidationQueue::Parallel,
        work_switch_seconds: 0.0,
        duration_seconds: 2_000.0,
        bch_block: None,
        max_uncles: 3,
        forwarding: Forwarding::AfterHeaderCheck {
            header_check_seconds: 0.0,
        },
        weak_shares: None,
    };
    let outcome = simulate(&network, parameters, SimulationRandom::from_seed(6));
    assert_eq!(outcome.share_messages, 4 * outcome.shares_found);
    assert_eq!(outcome.duplicate_share_messages, 2 * outcome.shares_found);
}

#[test]
fn forwarding_after_the_header_check_accepts_no_share_before_its_parent_and_uncles() {
    let ring: Vec<Vec<usize>> = (0..10)
        .map(|node| vec![(node + 9) % 10, (node + 1) % 10])
        .collect();
    let network = Network::uniform(&ring, 200_000.0, 800_000.0, 0.3);
    let parameters = Parameters {
        share_interval_seconds: 1.0,
        share_wire_bytes: 20_000,
        wire_bytes_per_uncle: 32,
        ack_wire_bytes: 105,
        validation_seconds: 0.05,
        validation_queue: ValidationQueue::Parallel,
        work_switch_seconds: 0.05,
        duration_seconds: 2_000.0,
        bch_block: None,
        max_uncles: 3,
        forwarding: Forwarding::AfterHeaderCheck {
            header_check_seconds: 0.001,
        },
        weak_shares: None,
    };
    let mut simulation = Simulation::new(&network, parameters, SimulationRandom::from_seed(15));
    simulation.run();
    let out_of_order = (1..simulation.shares.len())
        .flat_map(|share| (0..10).map(move |node| (share, node)))
        .filter(|&(share, node)| {
            let accepted_at = simulation.at_node[share][node].accepted_at;
            let record = &simulation.shares[share];
            record
                .parent
                .iter()
                .chain(record.uncles.iter())
                .any(|&dependency| simulation.at_node[dependency][node].accepted_at > accepted_at)
        })
        .count();
    let never_accepted = (1..simulation.shares.len())
        .flat_map(|share| (0..10).map(move |node| (share, node)))
        .filter(|&(share, node)| simulation.at_node[share][node].stage != Stage::Accepted)
        .count();
    assert!(simulation.shares.len() > 1_500);
    assert_eq!(out_of_order, 0);
    assert_eq!(never_accepted, 0);
}

#[test]
fn turning_weak_shares_on_moves_no_share_on_instant_links() {
    // Weak shares draw from their own stream, so the shares are found at
    // the same times by the same nodes; on instant links they delay nothing.
    let complete: Vec<Vec<usize>> = (0..10)
        .map(|node| (0..10).filter(|&peer| peer != node).collect())
        .collect();
    let network = Network::uniform(&complete, 1e15, 1e15, 0.0);
    let without = Parameters {
        share_interval_seconds: 10.0,
        share_wire_bytes: 2_000,
        wire_bytes_per_uncle: 32,
        ack_wire_bytes: 105,
        validation_seconds: 0.0,
        validation_queue: ValidationQueue::Parallel,
        work_switch_seconds: 0.0,
        duration_seconds: 5_000.0,
        bch_block: None,
        max_uncles: 3,
        forwarding: Forwarding::AfterValidation,
        weak_shares: None,
    };
    let with = Parameters {
        weak_shares: Some(WeakShareLoad {
            per_share: 16.0,
            wire_bytes: 904,
        }),
        ..without
    };
    let quiet = simulate(&network, without, SimulationRandom::from_seed(16));
    let busy = simulate(&network, with, SimulationRandom::from_seed(16));
    assert_eq!(busy.shares_found, quiet.shares_found);
    assert_eq!(busy.chain_shares, quiet.chain_shares);
    assert_eq!(busy.share_messages, quiet.share_messages);
    assert_eq!(quiet.weak_shares_found, 0);
    let expected_weak_shares = 16.0 * 5_000.0 / 10.0;
    assert!(
        (busy.weak_shares_found as f64 - expected_weak_shares).abs() < 0.1 * expected_weak_shares,
        "weak shares {}",
        busy.weak_shares_found
    );
}

#[test]
fn a_two_node_link_carries_each_weak_share_once_and_an_ack_back() {
    let pair: Vec<Vec<usize>> = vec![vec![1], vec![0]];
    let network = Network::uniform(&pair, 1e12, 1e12, 0.0);
    let parameters = Parameters {
        share_interval_seconds: 10.0,
        share_wire_bytes: 2_000,
        wire_bytes_per_uncle: 32,
        ack_wire_bytes: 105,
        validation_seconds: 0.0,
        validation_queue: ValidationQueue::Parallel,
        work_switch_seconds: 0.0,
        duration_seconds: 10_000.0,
        bch_block: None,
        max_uncles: 3,
        forwarding: Forwarding::AfterValidation,
        weak_shares: Some(WeakShareLoad {
            per_share: 8.0,
            wire_bytes: 904,
        }),
    };
    let outcome = simulate(&network, parameters, SimulationRandom::from_seed(17));
    assert!(outcome.weak_shares_found > 7_000);
    assert_eq!(outcome.weak_share_messages, outcome.weak_shares_found);
    let uploaded: f64 = outcome.node_upload_bytes_per_second.iter().sum::<f64>() * 10_000.0;
    let expected = outcome.shares_found as f64 * (2_000.0 + 105.0)
        + outcome.weak_shares_found as f64 * (904.0 + 105.0);
    assert!((uploaded - expected).abs() < 1e-3, "uploaded {uploaded}");
}

#[test]
fn weak_shares_on_a_slow_upload_delay_the_shares() {
    let ring: Vec<Vec<usize>> = (0..10)
        .map(|node| vec![(node + 9) % 10, (node + 1) % 10])
        .collect();
    // Sixty-four weak shares of 904 bytes every 10 seconds, pushed on about
    // 1.1 times each, fill about 64% of a 10,000-byte-a-second upload.
    let network = Network::uniform(&ring, 10_000.0, 40_000.0, 0.05);
    let without = Parameters {
        share_interval_seconds: 10.0,
        share_wire_bytes: 2_000,
        wire_bytes_per_uncle: 32,
        ack_wire_bytes: 105,
        validation_seconds: 0.005,
        validation_queue: ValidationQueue::Parallel,
        work_switch_seconds: 0.05,
        duration_seconds: 20_000.0,
        bch_block: None,
        max_uncles: 3,
        forwarding: Forwarding::AfterValidation,
        weak_shares: None,
    };
    let with = Parameters {
        weak_shares: Some(WeakShareLoad {
            per_share: 64.0,
            wire_bytes: 904,
        }),
        ..without
    };
    let quiet = simulate(&network, without, SimulationRandom::from_seed(18));
    let busy = simulate(&network, with, SimulationRandom::from_seed(18));
    assert!(
        busy.mean_seconds_to_all > 1.1 * quiet.mean_seconds_to_all,
        "busy {} quiet {}",
        busy.mean_seconds_to_all,
        quiet.mean_seconds_to_all
    );
}

#[test]
fn weak_share_slots_are_reused_once_every_node_has_the_weak_share() {
    // Tens of thousands of weak shares, but only those in flight hold a slot.
    let ring: Vec<Vec<usize>> = (0..10)
        .map(|node| vec![(node + 9) % 10, (node + 1) % 10])
        .collect();
    let network = Network::uniform(&ring, 1_000_000.0, 4_000_000.0, 0.05);
    let parameters = Parameters {
        share_interval_seconds: 10.0,
        share_wire_bytes: 2_000,
        wire_bytes_per_uncle: 32,
        ack_wire_bytes: 105,
        validation_seconds: 0.005,
        validation_queue: ValidationQueue::Parallel,
        work_switch_seconds: 0.05,
        duration_seconds: 5_000.0,
        bch_block: None,
        max_uncles: 3,
        forwarding: Forwarding::AfterValidation,
        weak_shares: Some(WeakShareLoad {
            per_share: 64.0,
            wire_bytes: 904,
        }),
    };
    let mut simulation = Simulation::new(&network, parameters, SimulationRandom::from_seed(19));
    simulation.run();
    assert!(simulation.weak_shares_found > 25_000);
    assert!(
        simulation.weak_shares.len() < 100,
        "slots {}",
        simulation.weak_shares.len()
    );
    assert_eq!(
        simulation.free_weak_share_slots.len(),
        simulation.weak_shares.len()
    );
}

/// Shares a second apart that take most of a second to validate: one at a
/// time, as a node's validation worker takes them, they queue and the
/// stale rate rises; in parallel nothing waits.
#[test]
fn a_serial_validation_worker_makes_shares_wait_their_turn() {
    let ring: Vec<Vec<usize>> = (0..20)
        .map(|node| vec![(node + 19) % 20, (node + 1) % 20])
        .collect();
    let network = Network::uniform(&ring, 1_000_000.0, 4_000_000.0, 0.05);
    let parallel = Parameters {
        share_interval_seconds: 1.0,
        share_wire_bytes: 2_000,
        wire_bytes_per_uncle: 32,
        ack_wire_bytes: 105,
        validation_seconds: 0.8,
        validation_queue: ValidationQueue::Parallel,
        work_switch_seconds: 0.05,
        duration_seconds: 600.0,
        bch_block: None,
        max_uncles: 3,
        forwarding: Forwarding::AfterValidation,
        weak_shares: None,
    };
    let serial = Parameters {
        validation_queue: ValidationQueue::Serial {
            weak_share_seconds: 0.0,
            weak_cached_seconds: 0.0,
            templates: WeakTemplates::Each,
        },
        ..parallel
    };

    let in_parallel = simulate(&network, parallel, SimulationRandom::from_seed(3));
    let one_at_a_time = simulate(&network, serial, SimulationRandom::from_seed(3));

    assert_eq!(in_parallel.mean_validation_wait_seconds, 0.0);
    assert!(
        one_at_a_time.mean_validation_wait_seconds > 0.1,
        "waited {}",
        one_at_a_time.mean_validation_wait_seconds
    );
    assert!(
        one_at_a_time
            .node_validation_busy_fraction
            .iter()
            .all(|busy| *busy <= 1.0 + 1e-9),
        "a worker busier than the run"
    );
    assert!(one_at_a_time.stale_rate > in_parallel.stale_rate);
}

/// Busy seconds after one node validates two weak shares at the build cost
/// `1.0` and the cached cost `0.1`.
fn busy_after_two_weak_shares(
    templates: WeakTemplates,
    parents: [usize; 2],
    finders: [usize; 2],
) -> f64 {
    let network = Network::uniform(&[Vec::new()], 1_000_000.0, 4_000_000.0, 0.05);
    let parameters = Parameters {
        share_interval_seconds: 10.0,
        share_wire_bytes: 2_000,
        wire_bytes_per_uncle: 32,
        ack_wire_bytes: 105,
        validation_seconds: 0.8,
        validation_queue: ValidationQueue::Serial {
            weak_share_seconds: 1.0,
            weak_cached_seconds: 0.1,
            templates,
        },
        work_switch_seconds: 0.05,
        duration_seconds: 600.0,
        bch_block: None,
        max_uncles: 3,
        forwarding: Forwarding::AfterValidation,
        weak_shares: None,
    };
    let mut simulation = Simulation::new(&network, parameters, SimulationRandom::from_seed(1));
    parents
        .into_iter()
        .zip(finders)
        .for_each(|(parent, finder)| {
            simulation.validation_done_at(0, 0.0, Some((parent, finder)));
        });
    simulation.nodes[0].validation_busy_seconds
}

/// One node checks two weak shares on one parent found by one other node.
#[test]
fn a_weak_share_on_a_cached_template_costs_the_cached_time() {
    let parents = [4, 4];
    let finders = [1, 1];
    assert_eq!(
        busy_after_two_weak_shares(WeakTemplates::PerNode, parents, finders),
        1.1,
        "per node, same finder"
    );
    assert_eq!(
        busy_after_two_weak_shares(WeakTemplates::Shared, parents, finders),
        1.1,
        "one template, same finder"
    );
    assert_eq!(
        busy_after_two_weak_shares(WeakTemplates::Each, parents, finders),
        2.0,
        "each template"
    );
}

/// Two finders on one parent are two templates, unless the network shares one.
#[test]
fn per_node_templates_pay_one_build_a_finding_node() {
    let parents = [4, 4];
    let finders = [1, 2];
    assert_eq!(
        busy_after_two_weak_shares(WeakTemplates::PerNode, parents, finders),
        2.0,
        "per node, two finders"
    );
    assert_eq!(
        busy_after_two_weak_shares(WeakTemplates::Shared, parents, finders),
        1.1,
        "one template, two finders"
    );
}

/// A new parent is a new template in every mode.
#[test]
fn a_new_parent_pays_the_build_again() {
    let parents = [4, 5];
    let finders = [1, 1];
    [
        WeakTemplates::Each,
        WeakTemplates::PerNode,
        WeakTemplates::Shared,
    ]
    .into_iter()
    .for_each(|templates| {
        assert_eq!(
            busy_after_two_weak_shares(templates, parents, finders),
            2.0,
            "two parents, {templates:?}"
        );
    });
}
