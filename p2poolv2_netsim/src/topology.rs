// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Who peers with whom.
//!
//! The graph is the small world upstream's ns-3 simulation builds
//! (`CreateSmallWorldNetworkP2P` in p2poolv2/Ns3_simulation,
//! `scratch/p2pool_v2_with_share.cc`): a ring where each node links to its
//! `peers_per_node / 2` nearest neighbours on each side, after which each
//! ring link is moved, with some probability, to a random node (Watts and
//! Strogatz, 1998). Reimplemented here from that description with this
//! crate's seeded generator; no upstream code is copied.

use crate::random::SimulationRandom;
use std::collections::BTreeSet;

/// Attempts to find a new end for a rewired link before keeping the old
/// one, as upstream does.
const REWIRE_ATTEMPTS: u32 = 100;

/// Each node's peers, sorted. Links are undirected: if `a` lists `b`,
/// `b` lists `a`.
pub type Adjacency = Vec<Vec<usize>>;

/// Build a small-world graph of `node_count` nodes, each first linked to
/// `peers_per_node / 2` neighbours on either side of a ring, then each ring
/// link rewired with `rewire_probability`. A ring cannot have more than
/// `(node_count - 1) / 2` neighbours a side, so small graphs get fewer.
pub fn small_world(
    node_count: usize,
    peers_per_node: usize,
    rewire_probability: f64,
    random: &mut SimulationRandom,
) -> Adjacency {
    let mut neighbours: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); node_count];
    let half = (peers_per_node / 2)
        .max(1)
        .min(node_count.saturating_sub(1) / 2);
    let ring_links = ring_links(node_count, half);
    ring_links.iter().for_each(|&(node, peer)| {
        connect(&mut neighbours, node, peer);
    });
    // Two nodes form a single link the ring above cannot express.
    if node_count == 2 {
        connect(&mut neighbours, 0, 1);
    }
    ring_links.iter().for_each(|&(node, peer)| {
        if random.next_bool(rewire_probability) && neighbours[node].contains(&peer) {
            rewire(&mut neighbours, node, peer, random);
        }
    });
    neighbours
        .into_iter()
        .map(|peers| peers.into_iter().collect())
        .collect()
}

/// The ring's links, each once, in node order.
fn ring_links(node_count: usize, half: usize) -> Vec<(usize, usize)> {
    let mut links = Vec::with_capacity(node_count * half);
    (0..node_count).for_each(|node| {
        (1..=half).for_each(|step| links.push((node, (node + step) % node_count)));
    });
    links
}

/// Add the undirected link `a`-`b`.
fn connect(neighbours: &mut [BTreeSet<usize>], first: usize, second: usize) {
    if first != second {
        neighbours[first].insert(second);
        neighbours[second].insert(first);
    }
}

/// Move the link `node`-`old_peer` to a random node `node` is not linked
/// to, keeping it if none is found.
fn rewire(
    neighbours: &mut [BTreeSet<usize>],
    node: usize,
    old_peer: usize,
    random: &mut SimulationRandom,
) {
    let node_count = neighbours.len() as u64;
    let mut attempts = 0;
    let mut new_peer = None;
    while attempts < REWIRE_ATTEMPTS && new_peer.is_none() {
        let candidate = random.next_below(node_count) as usize;
        if candidate != node && !neighbours[node].contains(&candidate) {
            new_peer = Some(candidate);
        }
        attempts += 1;
    }
    if let Some(peer) = new_peer {
        neighbours[node].remove(&old_peer);
        neighbours[old_peer].remove(&node);
        connect(neighbours, node, peer);
    }
}

/// Whether every node can reach every other.
pub fn is_connected(adjacency: &Adjacency) -> bool {
    if adjacency.is_empty() {
        return true;
    }
    let mut reached = vec![false; adjacency.len()];
    let mut frontier = vec![0usize];
    reached[0] = true;
    while let Some(node) = frontier.pop() {
        adjacency[node].iter().for_each(|&peer| {
            if !reached[peer] {
                reached[peer] = true;
                frontier.push(peer);
            }
        });
    }
    reached.iter().all(|&was_reached| was_reached)
}

/// Undirected links in the graph.
pub fn link_count(adjacency: &Adjacency) -> usize {
    adjacency.iter().map(Vec::len).sum::<usize>() / 2
}

#[cfg(test)]
mod tests;
