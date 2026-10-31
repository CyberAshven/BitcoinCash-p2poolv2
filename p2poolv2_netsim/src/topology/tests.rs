// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;

#[test]
fn a_ring_without_rewiring_gives_every_node_its_nearest_peers() {
    let mut random = SimulationRandom::from_seed(1);
    let adjacency = small_world(10, 4, 0.0, &mut random);
    assert_eq!(adjacency[0], vec![1, 2, 8, 9]);
    assert_eq!(adjacency[5], vec![3, 4, 6, 7]);
    assert_eq!(link_count(&adjacency), 20);
}

#[test]
fn rewiring_keeps_the_link_count() {
    let mut random = SimulationRandom::from_seed(2);
    let adjacency = small_world(200, 8, 0.5, &mut random);
    assert_eq!(link_count(&adjacency), 800);
}

#[test]
fn rewiring_changes_some_links() {
    let mut random = SimulationRandom::from_seed(3);
    let adjacency = small_world(200, 8, 0.5, &mut random);
    let mut ring_random = SimulationRandom::from_seed(3);
    let ring = small_world(200, 8, 0.0, &mut ring_random);
    assert_ne!(adjacency, ring);
}

#[test]
fn links_are_undirected() {
    let mut random = SimulationRandom::from_seed(4);
    let adjacency = small_world(50, 8, 0.5, &mut random);
    let one_sided = adjacency
        .iter()
        .enumerate()
        .flat_map(|(node, peers)| peers.iter().map(move |&peer| (node, peer)))
        .filter(|&(node, peer)| !adjacency[peer].contains(&node))
        .count();
    assert_eq!(one_sided, 0);
}

#[test]
fn the_default_graph_is_connected() {
    let mut random = SimulationRandom::from_seed(5);
    assert!(is_connected(&small_world(500, 8, 0.5, &mut random)));
}

#[test]
fn the_same_seed_builds_the_same_graph() {
    let mut first = SimulationRandom::from_seed(6);
    let mut second = SimulationRandom::from_seed(6);
    assert_eq!(
        small_world(100, 8, 0.5, &mut first),
        small_world(100, 8, 0.5, &mut second)
    );
}

#[test]
fn two_nodes_share_one_link() {
    let mut random = SimulationRandom::from_seed(7);
    let adjacency = small_world(2, 8, 0.5, &mut random);
    assert_eq!(adjacency, vec![vec![1], vec![0]]);
}

#[test]
fn one_node_has_no_peers() {
    let mut random = SimulationRandom::from_seed(8);
    let adjacency = small_world(1, 8, 0.5, &mut random);
    assert_eq!(adjacency, vec![Vec::<usize>::new()]);
    assert!(is_connected(&adjacency));
}

#[test]
fn a_graph_with_an_isolated_node_is_not_connected() {
    let adjacency: Adjacency = vec![vec![1], vec![0], vec![]];
    assert!(!is_connected(&adjacency));
}
