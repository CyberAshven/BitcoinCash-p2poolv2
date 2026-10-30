// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The discrete-event simulation of share propagation.
//!
//! Shares are found pool-wide as a Poisson process, each by a node picked
//! by hashrate. A node pushes every share it validates, the whole
//! `ShareBlock`, to every peer not known to have it, as
//! `send_share_block_broadcast` in `p2poolv2_lib` does, and each peer
//! answers with an `Ack`. A node knows a peer has a share once it sent the
//! share to that peer or received it from that peer, so a node often
//! receives the same share from several peers. A share waits at a node until
//! its parent and uncles are accepted there, is then validated, and is
//! accepted and relayed. A node mines on its best share, the highest, the
//! lowest hash breaking a tie as the mining base search does, from the
//! moment its miners have switched to the new work. When it finds a share it
//! names up to three uncles as `Store::find_uncles` picks them.
//!
//! Links: each node has an upload and a download rate. A broadcast to `k`
//! peers shares the upload equally (TCP connections in parallel), so every
//! copy needs `k * bytes / upload`, after whatever the node was already
//! uploading; a copy also needs at least `bytes / download` and the link's
//! one-way latency. Shares do not queue on the receiver's download: with
//! pushes from several peers that would need a fair-sharing model, and
//! uploads, not downloads, are the narrow side of the links modelled. This
//! is a fluid approximation, not a packet model: there is no TCP slow start
//! and no loss.
//!
//! When `bch_block` is set, a Bitcoin Cash block arrives on average every
//! ten minutes and its compact block takes each node's link for its bytes,
//! once down and once up, as BCHN relays it: shares wait behind it.

use crate::constants::{MAX_UNCLES, MAX_UNCLES_DEPTH};
use crate::random::SimulationRandom;
use serde::Serialize;
use std::cmp::Ordering;
use std::collections::BinaryHeap;

/// A node's view of its link.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct NodeLink {
    /// Bytes per second the node can upload for P2Pool.
    pub upload_bytes_per_second: f64,
    /// Bytes per second the node can download for P2Pool.
    pub download_bytes_per_second: f64,
    /// Fraction of the pool's hashrate mining through the node.
    pub hashrate_fraction: f64,
}

/// One end of a peer link as a node sees it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct PeerLink {
    /// The peer's node index.
    pub peer: usize,
    /// One-way latency, seconds.
    pub latency_seconds: f64,
}

/// The network a simulation runs on.
#[derive(Debug, Clone, PartialEq)]
pub struct Network {
    /// Each node's link.
    pub nodes: Vec<NodeLink>,
    /// Each node's peers. Links are undirected and both ends carry the same
    /// latency.
    pub peers: Vec<Vec<PeerLink>>,
}

impl Network {
    /// A network on `adjacency` where every node has the same link and the
    /// same hashrate and every link the same latency.
    pub fn uniform(
        adjacency: &[Vec<usize>],
        upload_bytes_per_second: f64,
        download_bytes_per_second: f64,
        latency_seconds: f64,
    ) -> Self {
        let hashrate_fraction = 1.0 / adjacency.len().max(1) as f64;
        Self {
            nodes: vec![
                NodeLink {
                    upload_bytes_per_second,
                    download_bytes_per_second,
                    hashrate_fraction,
                };
                adjacency.len()
            ],
            peers: adjacency
                .iter()
                .map(|neighbours| {
                    neighbours
                        .iter()
                        .map(|&peer| PeerLink {
                            peer,
                            latency_seconds,
                        })
                        .collect()
                })
                .collect(),
        }
    }
}

/// Bitcoin Cash blocks taking the links.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct BchBlockLoad {
    /// Mean seconds between BCH blocks.
    pub interval_seconds: f64,
    /// Bytes of one compact block, sent once down and once up per node.
    pub compact_block_bytes: u64,
}

/// What the simulation needs besides the network.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Parameters {
    /// Mean seconds between shares, pool-wide.
    pub share_interval_seconds: f64,
    /// Wire bytes of a `ShareBlock` message without uncles.
    pub share_wire_bytes: u64,
    /// Wire bytes each uncle adds.
    pub wire_bytes_per_uncle: u64,
    /// Wire bytes of the `Ack` a peer answers a pushed share with.
    pub ack_wire_bytes: u64,
    /// Seconds a node takes to validate a share.
    pub validation_seconds: f64,
    /// Seconds from a node's new best share to its miners hashing on it.
    pub work_switch_seconds: f64,
    /// Seconds of mining. Propagation then runs until every share has
    /// reached every node.
    pub duration_seconds: f64,
    /// Bitcoin Cash blocks on the same links, if any.
    pub bch_block: Option<BchBlockLoad>,
}

/// What became of the shares and what the links carried.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Outcome {
    /// Shares found during the run.
    pub shares_found: u64,
    /// Shares the rates count: those found early enough for later shares to
    /// settle whether they are on the chain, named as uncles or orphaned.
    pub shares_counted: u64,
    /// Counted shares on the final chain.
    pub chain_shares: u64,
    /// Counted shares off the final chain but named as uncles by it: paid
    /// 9/10 of their weight.
    pub uncle_shares: u64,
    /// Counted shares neither on the final chain nor named by it: paid
    /// nothing.
    pub orphan_shares: u64,
    /// Fraction of counted shares off the final chain (uncles and orphans).
    pub stale_rate: f64,
    /// Fraction of counted shares that became uncles.
    pub uncle_rate: f64,
    /// Fraction of counted shares orphaned.
    pub orphan_rate: f64,
    /// Mean over counted shares of the seconds until half the nodes had
    /// accepted the share.
    pub mean_seconds_to_half: f64,
    /// Same, until nine nodes in ten had.
    pub mean_seconds_to_ninety_percent: f64,
    /// Same, until every node had (infinite if some node never did).
    pub mean_seconds_to_all: f64,
    /// `ShareBlock` messages sent.
    pub share_messages: u64,
    /// `ShareBlock` messages that reached a node already holding the share.
    pub duplicate_share_messages: u64,
    /// P2Pool bytes each node sent, per second of mining.
    pub node_upload_bytes_per_second: Vec<f64>,
    /// P2Pool bytes each node received, per second of mining.
    pub node_download_bytes_per_second: Vec<f64>,
    /// Nodes whose share uploads alone needed more than their upload rate.
    pub saturated_nodes: u64,
}

/// Where a share stands at one node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// The node has not received the share.
    Unknown,
    /// Received, waiting for its parent or an uncle.
    Waiting,
    /// Being validated.
    Validating,
    /// Accepted and relayed.
    Accepted,
}

/// One node's state for one share.
#[derive(Debug, Clone, Copy)]
struct ShareAtNode {
    stage: Stage,
    accepted_at: f64,
    /// Bit `i` is set once the node knows its `i`th peer has the share.
    peers_known: u128,
}

impl Default for ShareAtNode {
    fn default() -> Self {
        Self {
            stage: Stage::Unknown,
            accepted_at: f64::INFINITY,
            peers_known: 0,
        }
    }
}

/// A share in the global DAG. Share 0 is the genesis share every node holds.
#[derive(Debug, Clone)]
struct Share {
    parent: Option<usize>,
    height: u64,
    uncles: Vec<usize>,
    found_at: f64,
    /// Stands in for the block hash when equal heights tie.
    hash: u64,
    wire_bytes: u64,
    children: Vec<usize>,
    nephews: Vec<usize>,
}

/// A node's moving parts.
#[derive(Debug, Clone)]
struct NodeState {
    tip: usize,
    mining_tip: usize,
    /// When the node's upload finishes what it has queued.
    upload_free_at: f64,
    /// When the node's download finishes the last BCH compact block.
    download_busy_until: f64,
    waiting: Vec<usize>,
    bytes_sent: u64,
    bytes_received: u64,
    share_bytes_sent: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum EventKind {
    ShareFound,
    ShareArrives {
        node: usize,
        share: usize,
        from_peer_index: usize,
    },
    ShareValidated {
        node: usize,
        share: usize,
    },
    WorkSwitch {
        node: usize,
        share: usize,
    },
    BchBlock,
}

#[derive(Debug, Clone, Copy)]
struct Event {
    time: f64,
    sequence: u64,
    kind: EventKind,
}

impl PartialEq for Event {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Event {}

impl PartialOrd for Event {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Event {
    /// Reversed, so the binary heap pops the earliest event, and the first
    /// scheduled among events at the same time.
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .time
            .total_cmp(&self.time)
            .then_with(|| other.sequence.cmp(&self.sequence))
    }
}

/// Peers a node can track in its `u128` knowledge mask.
pub const MAX_PEERS_PER_NODE: usize = 128;

/// Seconds after the last share is found that the run may still need to
/// deliver it before it is cut off, a guard against a scenario whose links
/// never catch up.
const DRAIN_LIMIT_SECONDS: f64 = 86_400.0;

/// Shares within this many share intervals of the end are not counted: a
/// later share has to be able to name them as uncles.
const SETTLING_INTERVALS: f64 = 10.0;

struct Simulation<'network> {
    network: &'network Network,
    parameters: Parameters,
    random: SimulationRandom,
    cumulative_hashrate: Vec<f64>,
    /// For each node and peer index, the node's index in that peer's list.
    reverse_peer_index: Vec<Vec<usize>>,
    shares: Vec<Share>,
    at_node: Vec<Vec<ShareAtNode>>,
    nodes: Vec<NodeState>,
    queue: BinaryHeap<Event>,
    sequence: u64,
    share_messages: u64,
    duplicate_share_messages: u64,
}

/// Run one simulation. Panics if a node has more than
/// `MAX_PEERS_PER_NODE` peers or the network has no node.
pub fn simulate(network: &Network, parameters: Parameters, random: SimulationRandom) -> Outcome {
    assert!(!network.nodes.is_empty(), "a network needs a node");
    assert!(
        network
            .peers
            .iter()
            .all(|peers| peers.len() <= MAX_PEERS_PER_NODE),
        "a node may have at most {MAX_PEERS_PER_NODE} peers"
    );
    let mut simulation = Simulation::new(network, parameters, random);
    simulation.run();
    simulation.outcome()
}

impl<'network> Simulation<'network> {
    fn new(network: &'network Network, parameters: Parameters, random: SimulationRandom) -> Self {
        let node_count = network.nodes.len();
        let total_hashrate: f64 = network
            .nodes
            .iter()
            .map(|node| node.hashrate_fraction)
            .sum();
        // Without any hashrate every node is taken to mine equally.
        let weight = |node: &NodeLink| {
            if total_hashrate > 0.0 {
                node.hashrate_fraction / total_hashrate
            } else {
                1.0 / node_count as f64
            }
        };
        let mut running = 0.0;
        let cumulative_hashrate = network
            .nodes
            .iter()
            .map(|node| {
                running += weight(node);
                running
            })
            .collect();
        let reverse_peer_index = network
            .peers
            .iter()
            .enumerate()
            .map(|(node, peers)| {
                peers
                    .iter()
                    .map(|link| {
                        network.peers[link.peer]
                            .iter()
                            .position(|back| back.peer == node)
                            .expect("links are undirected")
                    })
                    .collect()
            })
            .collect();
        let genesis = Share {
            parent: None,
            height: 0,
            uncles: Vec::new(),
            found_at: 0.0,
            hash: 0,
            wire_bytes: 0,
            children: Vec::new(),
            nephews: Vec::new(),
        };
        let genesis_at_node = ShareAtNode {
            stage: Stage::Accepted,
            accepted_at: 0.0,
            peers_known: u128::MAX,
        };
        let expected_shares = (parameters.duration_seconds / parameters.share_interval_seconds)
            .ceil()
            .max(0.0) as usize
            + 16;
        let mut shares = Vec::with_capacity(expected_shares);
        shares.push(genesis);
        let mut at_node = Vec::with_capacity(expected_shares);
        at_node.push(vec![genesis_at_node; node_count]);
        let nodes = vec![
            NodeState {
                tip: 0,
                mining_tip: 0,
                upload_free_at: 0.0,
                download_busy_until: 0.0,
                waiting: Vec::new(),
                bytes_sent: 0,
                bytes_received: 0,
                share_bytes_sent: 0,
            };
            node_count
        ];
        Self {
            network,
            parameters,
            random,
            cumulative_hashrate,
            reverse_peer_index,
            shares,
            at_node,
            nodes,
            queue: BinaryHeap::with_capacity(1024),
            sequence: 0,
            share_messages: 0,
            duplicate_share_messages: 0,
        }
    }

    fn schedule(&mut self, time: f64, kind: EventKind) {
        self.sequence += 1;
        self.queue.push(Event {
            time,
            sequence: self.sequence,
            kind,
        });
    }

    fn run(&mut self) {
        let first_share = self
            .random
            .next_exponential(self.parameters.share_interval_seconds);
        self.schedule(first_share, EventKind::ShareFound);
        if let Some(load) = self.parameters.bch_block {
            let first_block = self.random.next_exponential(load.interval_seconds);
            self.schedule(first_block, EventKind::BchBlock);
        }
        let cutoff = self.parameters.duration_seconds + DRAIN_LIMIT_SECONDS;
        while let Some(event) = self.queue.pop().filter(|event| event.time <= cutoff) {
            self.handle(event);
        }
    }

    fn handle(&mut self, event: Event) {
        match event.kind {
            EventKind::ShareFound => self.on_share_found(event.time),
            EventKind::ShareArrives {
                node,
                share,
                from_peer_index,
            } => self.on_share_arrives(event.time, node, share, from_peer_index),
            EventKind::ShareValidated { node, share } => {
                self.on_share_validated(event.time, node, share)
            }
            EventKind::WorkSwitch { node, share } => {
                if self.nodes[node].tip == share {
                    self.nodes[node].mining_tip = share;
                }
            }
            EventKind::BchBlock => self.on_bch_block(event.time),
        }
    }

    fn pick_finder(&mut self) -> usize {
        let draw = self.random.next_unit();
        let index = self
            .cumulative_hashrate
            .partition_point(|&cumulative| cumulative <= draw);
        index.min(self.network.nodes.len() - 1)
    }

    fn on_share_found(&mut self, time: f64) {
        if time >= self.parameters.duration_seconds {
            return;
        }
        let finder = self.pick_finder();
        let parent = self.nodes[finder].mining_tip;
        let uncles = self.select_uncles(finder, parent);
        let share = self.shares.len();
        let wire_bytes = self.parameters.share_wire_bytes
            + self.parameters.wire_bytes_per_uncle * uncles.len() as u64;
        let hash = self.random.next_u64();
        self.shares[parent].children.push(share);
        uncles
            .iter()
            .for_each(|&uncle| self.shares[uncle].nephews.push(share));
        self.shares.push(Share {
            parent: Some(parent),
            height: self.shares[parent].height + 1,
            uncles,
            found_at: time,
            hash,
            wire_bytes,
            children: Vec::new(),
            nephews: Vec::new(),
        });
        let mut states = vec![ShareAtNode::default(); self.network.nodes.len()];
        states[finder].stage = Stage::Validating;
        self.at_node.push(states);
        let validated_at = time + self.parameters.validation_seconds;
        self.schedule(
            validated_at,
            EventKind::ShareValidated {
                node: finder,
                share,
            },
        );
        let next = time
            + self
                .random
                .next_exponential(self.parameters.share_interval_seconds);
        self.schedule(next, EventKind::ShareFound);
    }

    /// Uncles for a share `node` builds on `base`, as `Store::find_uncles`
    /// picks them: children of the base's parent, grandparent and
    /// great-grandparent that are not on the base's own chain, accepted by
    /// the node and not already named as an uncle by the base or one of its
    /// ancestors, highest first, the lowest hash breaking a tie, at most
    /// three. An uncle named only on another branch can still be named here.
    fn select_uncles(&self, node: usize, base: usize) -> Vec<usize> {
        let mut ancestry = Vec::with_capacity(MAX_UNCLES_DEPTH + 1);
        ancestry.push(base);
        let mut current = base;
        let mut steps = 0;
        while steps < MAX_UNCLES_DEPTH {
            if let Some(parent) = self.shares[current].parent {
                ancestry.push(parent);
                current = parent;
            }
            steps += 1;
        }
        let mut candidates: Vec<usize> = ancestry[1..]
            .iter()
            .flat_map(|&ancestor| self.shares[ancestor].children.iter().copied())
            .filter(|child| !ancestry.contains(child))
            .filter(|&child| self.at_node[child][node].stage == Stage::Accepted)
            .filter(|&child| !Self::is_uncle_on_ancestry(&self.shares[child], &ancestry))
            .collect();
        candidates.sort_by(|first, second| {
            let first_share = &self.shares[*first];
            let second_share = &self.shares[*second];
            second_share
                .height
                .cmp(&first_share.height)
                .then(first_share.hash.cmp(&second_share.hash))
        });
        candidates.truncate(MAX_UNCLES);
        candidates
    }

    fn is_uncle_on_ancestry(share: &Share, ancestry: &[usize]) -> bool {
        share.nephews.iter().any(|nephew| ancestry.contains(nephew))
    }

    fn on_share_arrives(&mut self, time: f64, node: usize, share: usize, from_peer_index: usize) {
        let wire_bytes = self.shares[share].wire_bytes;
        let sender = self.network.peers[node][from_peer_index].peer;
        self.nodes[node].bytes_received += wire_bytes;
        // The receiver answers with an Ack.
        self.nodes[node].bytes_sent += self.parameters.ack_wire_bytes;
        self.nodes[sender].bytes_received += self.parameters.ack_wire_bytes;
        let state = &mut self.at_node[share][node];
        state.peers_known |= 1u128 << from_peer_index;
        if state.stage == Stage::Unknown {
            if self.dependencies_accepted(node, share) {
                self.at_node[share][node].stage = Stage::Validating;
                let validated_at = time + self.parameters.validation_seconds;
                self.schedule(validated_at, EventKind::ShareValidated { node, share });
            } else {
                self.at_node[share][node].stage = Stage::Waiting;
                self.nodes[node].waiting.push(share);
            }
        } else {
            self.duplicate_share_messages += 1;
        }
    }

    fn dependencies_accepted(&self, node: usize, share: usize) -> bool {
        let share_record = &self.shares[share];
        share_record
            .parent
            .iter()
            .chain(share_record.uncles.iter())
            .all(|&dependency| self.at_node[dependency][node].stage == Stage::Accepted)
    }

    fn on_share_validated(&mut self, time: f64, node: usize, share: usize) {
        let state = &mut self.at_node[share][node];
        state.stage = Stage::Accepted;
        state.accepted_at = time;
        self.relay(time, node, share);
        if self.is_better(share, self.nodes[node].tip) {
            self.nodes[node].tip = share;
            let switch_at = time + self.parameters.work_switch_seconds;
            self.schedule(switch_at, EventKind::WorkSwitch { node, share });
        }
        self.release_waiting(time, node);
    }

    fn is_better(&self, candidate: usize, current: usize) -> bool {
        let candidate_share = &self.shares[candidate];
        let current_share = &self.shares[current];
        candidate_share.height > current_share.height
            || (candidate_share.height == current_share.height
                && candidate_share.hash < current_share.hash)
    }

    fn release_waiting(&mut self, time: f64, node: usize) {
        let waiting = std::mem::take(&mut self.nodes[node].waiting);
        let (ready, still_waiting): (Vec<usize>, Vec<usize>) = waiting
            .into_iter()
            .partition(|&share| self.dependencies_accepted(node, share));
        self.nodes[node].waiting = still_waiting;
        ready.into_iter().for_each(|share| {
            self.at_node[share][node].stage = Stage::Validating;
            let validated_at = time + self.parameters.validation_seconds;
            self.schedule(validated_at, EventKind::ShareValidated { node, share });
        });
    }

    /// Push `share` from `node` to every peer not known to have it.
    fn relay(&mut self, time: f64, node: usize, share: usize) {
        let known = self.at_node[share][node].peers_known;
        let recipients: Vec<usize> = (0..self.network.peers[node].len())
            .filter(|&peer_index| known & (1u128 << peer_index) == 0)
            .collect();
        if recipients.is_empty() {
            return;
        }
        let all_recipients = recipients
            .iter()
            .fold(known, |mask, &peer_index| mask | (1u128 << peer_index));
        self.at_node[share][node].peers_known = all_recipients;
        let wire_bytes = self.shares[share].wire_bytes;
        let link = self.network.nodes[node];
        let start = time.max(self.nodes[node].upload_free_at);
        let upload_seconds =
            recipients.len() as f64 * wire_bytes as f64 / link.upload_bytes_per_second;
        self.nodes[node].upload_free_at = start + upload_seconds;
        let sent = wire_bytes * recipients.len() as u64;
        self.nodes[node].bytes_sent += sent;
        self.nodes[node].share_bytes_sent += sent;
        self.share_messages += recipients.len() as u64;
        recipients.into_iter().for_each(|peer_index| {
            let peer_link = self.network.peers[node][peer_index];
            let peer = peer_link.peer;
            let download_rate = self.network.nodes[peer].download_bytes_per_second;
            let downlink_start =
                (start + peer_link.latency_seconds).max(self.nodes[peer].download_busy_until);
            let arrival = (downlink_start + wire_bytes as f64 / download_rate)
                .max(start + upload_seconds + peer_link.latency_seconds);
            let from_peer_index = self.reverse_peer_index[node][peer_index];
            self.schedule(
                arrival,
                EventKind::ShareArrives {
                    node: peer,
                    share,
                    from_peer_index,
                },
            );
        });
    }

    fn on_bch_block(&mut self, time: f64) {
        let Some(load) = self.parameters.bch_block else {
            return;
        };
        if time >= self.parameters.duration_seconds {
            return;
        }
        let bytes = load.compact_block_bytes as f64;
        self.nodes
            .iter_mut()
            .zip(self.network.nodes.iter())
            .for_each(|(state, link)| {
                state.download_busy_until =
                    state.download_busy_until.max(time) + bytes / link.download_bytes_per_second;
                state.upload_free_at =
                    state.upload_free_at.max(time) + bytes / link.upload_bytes_per_second;
            });
        let next = time + self.random.next_exponential(load.interval_seconds);
        self.schedule(next, EventKind::BchBlock);
    }

    /// The final chain: the best share overall and its ancestors.
    fn final_chain(&self) -> Vec<bool> {
        let best = (0..self.shares.len())
            .reduce(|best, candidate| {
                if self.is_better(candidate, best) {
                    candidate
                } else {
                    best
                }
            })
            .unwrap_or(0);
        let mut on_chain = vec![false; self.shares.len()];
        let mut current = Some(best);
        while let Some(share) = current {
            on_chain[share] = true;
            current = self.shares[share].parent;
        }
        on_chain
    }

    fn outcome(&self) -> Outcome {
        let on_chain = self.final_chain();
        let mut named_by_chain = vec![false; self.shares.len()];
        (0..self.shares.len())
            .filter(|&share| on_chain[share])
            .for_each(|share| {
                self.shares[share]
                    .uncles
                    .iter()
                    .for_each(|&uncle| named_by_chain[uncle] = true);
            });
        let settled_before = self.parameters.duration_seconds
            - SETTLING_INTERVALS * self.parameters.share_interval_seconds;
        let counted: Vec<usize> = (1..self.shares.len())
            .filter(|&share| settled_before <= 0.0 || self.shares[share].found_at < settled_before)
            .collect();
        let chain_shares = counted.iter().filter(|&&share| on_chain[share]).count() as u64;
        let uncle_shares = counted
            .iter()
            .filter(|&&share| !on_chain[share] && named_by_chain[share])
            .count() as u64;
        let orphan_shares = counted.len() as u64 - chain_shares - uncle_shares;
        let shares_counted = counted.len() as u64;
        let rate = |count: u64| {
            if shares_counted == 0 {
                0.0
            } else {
                count as f64 / shares_counted as f64
            }
        };
        let (half, ninety, all) = self.mean_propagation_seconds(&counted);
        let duration = self.parameters.duration_seconds.max(f64::MIN_POSITIVE);
        let saturated_nodes = self
            .nodes
            .iter()
            .zip(self.network.nodes.iter())
            .filter(|(state, link)| {
                state.share_bytes_sent as f64 > link.upload_bytes_per_second * duration
            })
            .count() as u64;
        Outcome {
            shares_found: (self.shares.len() - 1) as u64,
            shares_counted,
            chain_shares,
            uncle_shares,
            orphan_shares,
            stale_rate: rate(uncle_shares + orphan_shares),
            uncle_rate: rate(uncle_shares),
            orphan_rate: rate(orphan_shares),
            mean_seconds_to_half: half,
            mean_seconds_to_ninety_percent: ninety,
            mean_seconds_to_all: all,
            share_messages: self.share_messages,
            duplicate_share_messages: self.duplicate_share_messages,
            node_upload_bytes_per_second: self
                .nodes
                .iter()
                .map(|state| state.bytes_sent as f64 / duration)
                .collect(),
            node_download_bytes_per_second: self
                .nodes
                .iter()
                .map(|state| state.bytes_received as f64 / duration)
                .collect(),
            saturated_nodes,
        }
    }

    /// Mean seconds from a share being found until half, nine tenths and all
    /// of the nodes accepted it.
    fn mean_propagation_seconds(&self, counted: &[usize]) -> (f64, f64, f64) {
        if counted.is_empty() {
            return (0.0, 0.0, 0.0);
        }
        let node_count = self.network.nodes.len();
        let mut delays = Vec::with_capacity(node_count);
        let mut sums = (0.0, 0.0, 0.0);
        counted.iter().for_each(|&share| {
            let found_at = self.shares[share].found_at;
            delays.clear();
            delays.extend(
                self.at_node[share]
                    .iter()
                    .map(|state| state.accepted_at - found_at),
            );
            delays.sort_by(f64::total_cmp);
            sums.0 += quantile(&delays, 0.5);
            sums.1 += quantile(&delays, 0.9);
            sums.2 += delays[node_count - 1];
        });
        let count = counted.len() as f64;
        (sums.0 / count, sums.1 / count, sums.2 / count)
    }
}

/// The value at `fraction` of sorted `values`: the smallest value at least
/// that fraction of them do not exceed.
fn quantile(sorted: &[f64], fraction: f64) -> f64 {
    let rank = (fraction * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

#[cfg(test)]
mod tests;
