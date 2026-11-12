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
//!
//! Three changes studied for scaling, none of them built, can be turned on:
//! forwarding a share once its header checks out, before its body is
//! validated (`Forwarding::AfterHeaderCheck`); weak shares gossiped beside
//! the shares on the same upload queue (`WeakShareLoad`); and an uncle cap
//! other than three (`Parameters::max_uncles`). With none of them a run is
//! exactly the model above.

use crate::constants::MAX_UNCLES_DEPTH;
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

/// When a node forwards a share it holds.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub enum Forwarding {
    /// Once the share is fully validated, as nodes do today.
    AfterValidation,
    /// Once its proof of work and header check out, its body validated
    /// after, as compact blocks' high-bandwidth mode forwards blocks
    /// (BIP152). The header check needs the parent's and the uncles'
    /// headers checked at the node first. A node still mines on, and names
    /// as an uncle, only a share it has fully validated. Not built.
    AfterHeaderCheck {
        /// Seconds the proof-of-work and header check take (an estimate).
        header_check_seconds: f64,
    },
}

/// Weak shares on the same links as the shares
/// (`docs/sharechain/small-miners-design.md`, section 3). Each is found as a
/// Poisson process by a node picked by hashrate, on that node's mining tip,
/// and travels like a share: held at a node until its parent share is
/// accepted there, validated in a share's validation time, then pushed to
/// every peer not known to have it, behind whatever the node is already
/// uploading, and answered with an `Ack`. Weak shares never race: they
/// extend no chain and are never named as uncles. Not built.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct WeakShareLoad {
    /// Weak shares found per share, on average.
    pub per_share: f64,
    /// Wire bytes of one weak share message.
    pub wire_bytes: u64,
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
    /// Most uncles a share names: `MAX_UNCLES`, three, today.
    pub max_uncles: usize,
    /// When a node forwards a share.
    pub forwarding: Forwarding,
    /// Weak shares on the same links, if any.
    pub weak_shares: Option<WeakShareLoad>,
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
    /// Nodes whose share and weak share uploads alone needed more than their
    /// upload rate.
    pub saturated_nodes: u64,
    /// Weak shares found during the run.
    pub weak_shares_found: u64,
    /// Weak share messages sent.
    pub weak_share_messages: u64,
    /// Shares on the final chain, genesis left out, counted or not.
    pub chain_length: u64,
    /// Uncles named by the final chain's shares: headers a node syncing the
    /// chain fetches and checks beside the chain's own.
    pub uncles_named_by_chain: u64,
    /// Uncle hashes in the headers of those uncles.
    pub uncle_hashes_in_named_uncles: u64,
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

/// Where a share's header check stands at one node. Only shares forwarded
/// after the header check go through it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeaderCheck {
    /// Not started: the node lacks the share, or the checked headers of its
    /// parent and uncles.
    Unchecked,
    /// Being checked.
    Checking,
    /// Checked and forwarded.
    Checked,
}

/// One node's state for one share.
#[derive(Debug, Clone, Copy)]
struct ShareAtNode {
    stage: Stage,
    header: HeaderCheck,
    accepted_at: f64,
    /// Bit `i` is set once the node knows its `i`th peer has the share.
    peers_known: u128,
}

impl Default for ShareAtNode {
    fn default() -> Self {
        Self {
            stage: Stage::Unknown,
            header: HeaderCheck::Unchecked,
            accepted_at: f64::INFINITY,
            peers_known: 0,
        }
    }
}

/// One node's state for one weak share.
#[derive(Debug, Clone, Copy)]
struct WeakShareAtNode {
    stage: Stage,
    /// Bit `i` is set once the node knows its `i`th peer has the weak share.
    peers_known: u128,
}

impl Default for WeakShareAtNode {
    fn default() -> Self {
        Self {
            stage: Stage::Unknown,
            peers_known: 0,
        }
    }
}

/// A weak share while it travels. Its slot is reused once every node has
/// accepted it and no copy of it is on the way, so memory follows the weak
/// shares in flight rather than every weak share found.
#[derive(Debug, Clone)]
struct WeakShare {
    parent: usize,
    at_node: Vec<WeakShareAtNode>,
    accepted_nodes: usize,
    messages_in_flight: u64,
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
    /// Slots of weak shares waiting for their parent share.
    waiting_weak_shares: Vec<usize>,
    bytes_sent: u64,
    bytes_received: u64,
    /// Bytes of shares and weak shares the node pushed, acks left out.
    pushed_bytes: u64,
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
    HeaderChecked {
        node: usize,
        share: usize,
    },
    WeakShareFound,
    WeakShareArrives {
        node: usize,
        slot: usize,
        from_peer_index: usize,
    },
    WeakShareValidated {
        node: usize,
        slot: usize,
    },
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

/// Key of the weak shares' own random stream, so that turning weak shares on
/// leaves every share's time, finder and hash as they were.
const WEAK_SHARE_STREAM: u64 = 0x5745_414b_5348_4152;

struct Simulation<'network> {
    network: &'network Network,
    parameters: Parameters,
    random: SimulationRandom,
    weak_share_random: SimulationRandom,
    cumulative_hashrate: Vec<f64>,
    /// For each node and peer index, the node's index in that peer's list.
    reverse_peer_index: Vec<Vec<usize>>,
    shares: Vec<Share>,
    at_node: Vec<Vec<ShareAtNode>>,
    nodes: Vec<NodeState>,
    weak_shares: Vec<WeakShare>,
    free_weak_share_slots: Vec<usize>,
    queue: BinaryHeap<Event>,
    sequence: u64,
    share_messages: u64,
    duplicate_share_messages: u64,
    weak_shares_found: u64,
    weak_share_messages: u64,
}

/// Run one simulation. Panics if a node has more than
/// `MAX_PEERS_PER_NODE` peers or the network has no node.
pub fn simulate(network: &Network, parameters: Parameters, random: SimulationRandom) -> Outcome {
    let mut simulation = Simulation::new(network, parameters, random);
    simulation.run();
    simulation.outcome()
}

impl<'network> Simulation<'network> {
    fn new(network: &'network Network, parameters: Parameters, random: SimulationRandom) -> Self {
        assert!(!network.nodes.is_empty(), "a network needs a node");
        assert!(
            network
                .peers
                .iter()
                .all(|peers| peers.len() <= MAX_PEERS_PER_NODE),
            "a node may have at most {MAX_PEERS_PER_NODE} peers"
        );
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
            header: HeaderCheck::Checked,
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
                waiting_weak_shares: Vec::new(),
                bytes_sent: 0,
                bytes_received: 0,
                pushed_bytes: 0,
            };
            node_count
        ];
        let weak_share_random = random.branch(WEAK_SHARE_STREAM);
        Self {
            network,
            parameters,
            random,
            weak_share_random,
            cumulative_hashrate,
            reverse_peer_index,
            shares,
            at_node,
            nodes,
            weak_shares: Vec::with_capacity(64),
            free_weak_share_slots: Vec::with_capacity(64),
            queue: BinaryHeap::with_capacity(1024),
            sequence: 0,
            share_messages: 0,
            duplicate_share_messages: 0,
            weak_shares_found: 0,
            weak_share_messages: 0,
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
        if let Some(mean) = self.weak_share_interval_seconds() {
            let first_weak_share = self.weak_share_random.next_exponential(mean);
            self.schedule(first_weak_share, EventKind::WeakShareFound);
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
            EventKind::HeaderChecked { node, share } => {
                self.on_header_checked(event.time, node, share)
            }
            EventKind::WeakShareFound => self.on_weak_share_found(event.time),
            EventKind::WeakShareArrives {
                node,
                slot,
                from_peer_index,
            } => self.on_weak_share_arrives(event.time, node, slot, from_peer_index),
            EventKind::WeakShareValidated { node, slot } => {
                self.on_weak_share_validated(event.time, node, slot)
            }
        }
    }

    fn pick_finder(&mut self) -> usize {
        let draw = self.random.next_unit();
        self.node_at(draw)
    }

    /// The node a uniform `draw` in `[0, 1)` lands on, by hashrate.
    fn node_at(&self, draw: f64) -> usize {
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
        match self.parameters.forwarding {
            Forwarding::AfterValidation => {
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
            }
            Forwarding::AfterHeaderCheck {
                header_check_seconds,
            } => {
                states[finder].stage = Stage::Waiting;
                states[finder].header = HeaderCheck::Checking;
                self.at_node.push(states);
                self.nodes[finder].waiting.push(share);
                self.schedule(
                    time + header_check_seconds,
                    EventKind::HeaderChecked {
                        node: finder,
                        share,
                    },
                );
            }
        }
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
    /// `max_uncles` (three today). An uncle named only on another branch can
    /// still be named here.
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
        candidates.truncate(self.parameters.max_uncles);
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
            self.receive_new_share(time, node, share);
        } else {
            self.duplicate_share_messages += 1;
        }
    }

    /// Validate a share `node` has just received for the first time, or hold
    /// it until what it depends on is accepted (or, when shares are forwarded
    /// after the header check, checked).
    fn receive_new_share(&mut self, time: f64, node: usize, share: usize) {
        match self.parameters.forwarding {
            Forwarding::AfterValidation => {
                if self.dependencies_accepted(node, share) {
                    self.at_node[share][node].stage = Stage::Validating;
                    let validated_at = time + self.parameters.validation_seconds;
                    self.schedule(validated_at, EventKind::ShareValidated { node, share });
                } else {
                    self.at_node[share][node].stage = Stage::Waiting;
                    self.nodes[node].waiting.push(share);
                }
            }
            Forwarding::AfterHeaderCheck { .. } => {
                self.at_node[share][node].stage = Stage::Waiting;
                self.nodes[node].waiting.push(share);
                self.advance(time, node, share);
            }
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

    fn dependency_headers_checked(&self, node: usize, share: usize) -> bool {
        let share_record = &self.shares[share];
        share_record
            .parent
            .iter()
            .chain(share_record.uncles.iter())
            .all(|&dependency| self.at_node[dependency][node].header == HeaderCheck::Checked)
    }

    fn on_share_validated(&mut self, time: f64, node: usize, share: usize) {
        let state = &mut self.at_node[share][node];
        state.stage = Stage::Accepted;
        state.accepted_at = time;
        let forwards_after_validation = self.parameters.forwarding == Forwarding::AfterValidation;
        if forwards_after_validation {
            self.relay(time, node, share);
        }
        if self.is_better(share, self.nodes[node].tip) {
            self.nodes[node].tip = share;
            let switch_at = time + self.parameters.work_switch_seconds;
            self.schedule(switch_at, EventKind::WorkSwitch { node, share });
        }
        if forwards_after_validation {
            self.release_waiting(time, node);
        } else {
            self.advance_waiting(time, node);
        }
        self.release_waiting_weak_shares(time, node);
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

    /// With forwarding after the header check: start the header check of
    /// `share` at `node` once its parent's and uncles' headers are checked
    /// there, and its body validation once its own header is checked and its
    /// parent and uncles are accepted there.
    fn advance(&mut self, time: f64, node: usize, share: usize) {
        let Forwarding::AfterHeaderCheck {
            header_check_seconds,
        } = self.parameters.forwarding
        else {
            return;
        };
        let state = self.at_node[share][node];
        if state.header == HeaderCheck::Unchecked && self.dependency_headers_checked(node, share) {
            self.at_node[share][node].header = HeaderCheck::Checking;
            self.schedule(
                time + header_check_seconds,
                EventKind::HeaderChecked { node, share },
            );
        }
        if state.header == HeaderCheck::Checked
            && state.stage == Stage::Waiting
            && self.dependencies_accepted(node, share)
        {
            self.at_node[share][node].stage = Stage::Validating;
            let validated_at = time + self.parameters.validation_seconds;
            self.schedule(validated_at, EventKind::ShareValidated { node, share });
        }
    }

    /// Advance every share waiting at `node`, keeping those still waiting.
    fn advance_waiting(&mut self, time: f64, node: usize) {
        let waiting = std::mem::take(&mut self.nodes[node].waiting);
        waiting
            .iter()
            .for_each(|&share| self.advance(time, node, share));
        let still_waiting: Vec<usize> = waiting
            .into_iter()
            .filter(|&share| self.at_node[share][node].stage == Stage::Waiting)
            .collect();
        self.nodes[node].waiting = still_waiting;
    }

    fn on_header_checked(&mut self, time: f64, node: usize, share: usize) {
        self.at_node[share][node].header = HeaderCheck::Checked;
        self.relay(time, node, share);
        self.advance_waiting(time, node);
    }

    /// Indices of `node`'s peers not in the `known` mask.
    fn unknown_peers(&self, node: usize, known: u128) -> Vec<usize> {
        (0..self.network.peers[node].len())
            .filter(|&peer_index| known & (1u128 << peer_index) == 0)
            .collect()
    }

    /// Push a message of `wire_bytes` from `node` to each peer index in
    /// `recipients`, after whatever the node is already uploading, and
    /// return when each copy arrives, in the same order.
    fn push(&mut self, time: f64, node: usize, recipients: &[usize], wire_bytes: u64) -> Vec<f64> {
        let link = self.network.nodes[node];
        let start = time.max(self.nodes[node].upload_free_at);
        let upload_seconds =
            recipients.len() as f64 * wire_bytes as f64 / link.upload_bytes_per_second;
        self.nodes[node].upload_free_at = start + upload_seconds;
        let sent = wire_bytes * recipients.len() as u64;
        self.nodes[node].bytes_sent += sent;
        self.nodes[node].pushed_bytes += sent;
        recipients
            .iter()
            .map(|&peer_index| {
                let peer_link = self.network.peers[node][peer_index];
                let peer = peer_link.peer;
                let download_rate = self.network.nodes[peer].download_bytes_per_second;
                let downlink_start =
                    (start + peer_link.latency_seconds).max(self.nodes[peer].download_busy_until);
                (downlink_start + wire_bytes as f64 / download_rate)
                    .max(start + upload_seconds + peer_link.latency_seconds)
            })
            .collect()
    }

    /// Push `share` from `node` to every peer not known to have it.
    fn relay(&mut self, time: f64, node: usize, share: usize) {
        let known = self.at_node[share][node].peers_known;
        let recipients = self.unknown_peers(node, known);
        if recipients.is_empty() {
            return;
        }
        self.at_node[share][node].peers_known = with_peers(known, &recipients);
        let arrivals = self.push(time, node, &recipients, self.shares[share].wire_bytes);
        self.share_messages += recipients.len() as u64;
        recipients
            .iter()
            .zip(arrivals)
            .for_each(|(&peer_index, arrival)| {
                let peer = self.network.peers[node][peer_index].peer;
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

    /// Mean seconds between weak shares, pool-wide, when there are any.
    fn weak_share_interval_seconds(&self) -> Option<f64> {
        self.parameters
            .weak_shares
            .filter(|load| load.per_share > 0.0)
            .map(|load| self.parameters.share_interval_seconds / load.per_share)
    }

    fn weak_share_wire_bytes(&self) -> u64 {
        self.parameters
            .weak_shares
            .map_or(0, |load| load.wire_bytes)
    }

    fn on_weak_share_found(&mut self, time: f64) {
        let Some(mean) = self.weak_share_interval_seconds() else {
            return;
        };
        if time >= self.parameters.duration_seconds {
            return;
        }
        let draw = self.weak_share_random.next_unit();
        let finder = self.node_at(draw);
        let parent = self.nodes[finder].mining_tip;
        let slot = self.allocate_weak_share(parent);
        self.weak_shares[slot].at_node[finder].stage = Stage::Validating;
        self.weak_shares_found += 1;
        self.schedule(
            time + self.parameters.validation_seconds,
            EventKind::WeakShareValidated { node: finder, slot },
        );
        let next = time + self.weak_share_random.next_exponential(mean);
        self.schedule(next, EventKind::WeakShareFound);
    }

    /// A slot for a new weak share on `parent`, reusing a freed one.
    fn allocate_weak_share(&mut self, parent: usize) -> usize {
        match self.free_weak_share_slots.pop() {
            Some(slot) => {
                let weak_share = &mut self.weak_shares[slot];
                weak_share.parent = parent;
                weak_share.at_node.fill(WeakShareAtNode::default());
                weak_share.accepted_nodes = 0;
                weak_share.messages_in_flight = 0;
                slot
            }
            None => {
                self.weak_shares.push(WeakShare {
                    parent,
                    at_node: vec![WeakShareAtNode::default(); self.network.nodes.len()],
                    accepted_nodes: 0,
                    messages_in_flight: 0,
                });
                self.weak_shares.len() - 1
            }
        }
    }

    fn on_weak_share_arrives(
        &mut self,
        time: f64,
        node: usize,
        slot: usize,
        from_peer_index: usize,
    ) {
        let wire_bytes = self.weak_share_wire_bytes();
        let sender = self.network.peers[node][from_peer_index].peer;
        self.nodes[node].bytes_received += wire_bytes;
        self.nodes[node].bytes_sent += self.parameters.ack_wire_bytes;
        self.nodes[sender].bytes_received += self.parameters.ack_wire_bytes;
        let weak_share = &mut self.weak_shares[slot];
        weak_share.messages_in_flight -= 1;
        let parent = weak_share.parent;
        let state = &mut weak_share.at_node[node];
        state.peers_known |= 1u128 << from_peer_index;
        if state.stage == Stage::Unknown {
            if self.at_node[parent][node].stage == Stage::Accepted {
                self.weak_shares[slot].at_node[node].stage = Stage::Validating;
                self.schedule(
                    time + self.parameters.validation_seconds,
                    EventKind::WeakShareValidated { node, slot },
                );
            } else {
                self.weak_shares[slot].at_node[node].stage = Stage::Waiting;
                self.nodes[node].waiting_weak_shares.push(slot);
            }
        }
        self.free_weak_share_if_done(slot);
    }

    fn on_weak_share_validated(&mut self, time: f64, node: usize, slot: usize) {
        let weak_share = &mut self.weak_shares[slot];
        weak_share.at_node[node].stage = Stage::Accepted;
        weak_share.accepted_nodes += 1;
        let known = weak_share.at_node[node].peers_known;
        let recipients = self.unknown_peers(node, known);
        if !recipients.is_empty() {
            self.weak_shares[slot].at_node[node].peers_known = with_peers(known, &recipients);
            let wire_bytes = self.weak_share_wire_bytes();
            let arrivals = self.push(time, node, &recipients, wire_bytes);
            let sent = recipients.len() as u64;
            self.weak_shares[slot].messages_in_flight += sent;
            self.weak_share_messages += sent;
            recipients
                .iter()
                .zip(arrivals)
                .for_each(|(&peer_index, arrival)| {
                    let peer = self.network.peers[node][peer_index].peer;
                    let from_peer_index = self.reverse_peer_index[node][peer_index];
                    self.schedule(
                        arrival,
                        EventKind::WeakShareArrives {
                            node: peer,
                            slot,
                            from_peer_index,
                        },
                    );
                });
        }
        self.free_weak_share_if_done(slot);
    }

    /// Validate the weak shares waiting at `node` whose parent it has now
    /// accepted.
    fn release_waiting_weak_shares(&mut self, time: f64, node: usize) {
        if self.nodes[node].waiting_weak_shares.is_empty() {
            return;
        }
        let waiting = std::mem::take(&mut self.nodes[node].waiting_weak_shares);
        let (ready, still_waiting): (Vec<usize>, Vec<usize>) =
            waiting.into_iter().partition(|&slot| {
                self.at_node[self.weak_shares[slot].parent][node].stage == Stage::Accepted
            });
        self.nodes[node].waiting_weak_shares = still_waiting;
        ready.into_iter().for_each(|slot| {
            self.weak_shares[slot].at_node[node].stage = Stage::Validating;
            self.schedule(
                time + self.parameters.validation_seconds,
                EventKind::WeakShareValidated { node, slot },
            );
        });
    }

    /// Free `slot` once every node accepted its weak share and no copy is on
    /// the way: no event can refer to it any more.
    fn free_weak_share_if_done(&mut self, slot: usize) {
        let weak_share = &self.weak_shares[slot];
        if weak_share.accepted_nodes == self.network.nodes.len()
            && weak_share.messages_in_flight == 0
        {
            self.free_weak_share_slots.push(slot);
        }
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
        let (chain_length, uncles_named_by_chain, uncle_hashes_in_named_uncles) =
            self.header_sync_counts(&on_chain);
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
                state.pushed_bytes as f64 > link.upload_bytes_per_second * duration
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
            weak_shares_found: self.weak_shares_found,
            weak_share_messages: self.weak_share_messages,
            chain_length,
            uncles_named_by_chain,
            uncle_hashes_in_named_uncles,
        }
    }

    /// Shares on the final chain (genesis left out), the uncles they name,
    /// and the uncle hashes in those uncles' own headers.
    fn header_sync_counts(&self, on_chain: &[bool]) -> (u64, u64, u64) {
        (1..self.shares.len())
            .filter(|&share| on_chain[share])
            .fold((0, 0, 0), |(length, named, nested), share| {
                let uncles = &self.shares[share].uncles;
                let uncles_of_uncles: usize = uncles
                    .iter()
                    .map(|&uncle| self.shares[uncle].uncles.len())
                    .sum();
                (
                    length + 1,
                    named + uncles.len() as u64,
                    nested + uncles_of_uncles as u64,
                )
            })
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

/// `known` with the bits of `recipients` set.
fn with_peers(known: u128, recipients: &[usize]) -> u128 {
    recipients
        .iter()
        .fold(known, |mask, &peer_index| mask | (1u128 << peer_index))
}

/// The value at `fraction` of sorted `values`: the smallest value at least
/// that fraction of them do not exceed.
fn quantile(sorted: &[f64], fraction: f64) -> f64 {
    let rank = (fraction * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

#[cfg(test)]
mod tests;
