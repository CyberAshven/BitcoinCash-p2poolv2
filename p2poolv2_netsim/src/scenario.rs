// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A scenario: every input of one run, and how a run is put together.
//!
//! Defaults are assumptions, not measurements: a hundred nodes on mixed
//! home and data-centre links, a thousand miners, a pool with 1% of BCH's
//! hashrate, 32 MB blocks (mainnet's ABLA floor), 10-second shares, the
//! hybrid payout proposal for a share without merge-mined tokens, whole
//! shares on the wire as today, and a day of mining (8,640 shares).

use crate::constants::{
    BCH_BLOCK_INTERVAL_SECONDS, BCH_MAINNET_ABLA_FLOOR_BYTES, BCHN_OUTBOUND_PEERS,
    SHARE_HEADER_BYTES_PER_UNCLE, SHARE_INTERVAL_SECONDS, TYPICAL_TRANSACTION_BYTES,
};
use crate::distribution::Distribution;
use crate::engine::{self, BchBlockLoad, Network, NodeLink, Parameters, PeerLink};
use crate::payees::{HashrateSpread, expected_payees, hashrate_fractions, pplns_window_shares};
use crate::random::SimulationRandom;
use crate::report::Report;
use crate::sizes::{ACK_PAYLOAD_BYTES, CoinbaseLayout, ShareSizes, wire_bytes};
use crate::topology::{self, Adjacency};
use crate::traffic::{BchnLoad, StratumProtocol, bchn_load};
use serde::Serialize;

/// Bytes per second in a megabit per second.
const BYTES_PER_MEGABIT: f64 = 125_000.0;

/// Share of its link a node keeps for P2Pool when BCHN's relay alone needs
/// all of it, so the run can still go on and show the damage.
const MINIMUM_LINK_SHARE: f64 = 0.05;

/// Every input of one run.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Scenario {
    /// Name in tables.
    pub label: String,
    /// Seed of every random draw: the same scenario and seed give the same
    /// result on any platform.
    pub seed: u64,
    /// P2Pool nodes.
    pub node_count: usize,
    /// Peers each node starts with in the small-world graph.
    pub peers_per_node: usize,
    /// Chance each ring link is rewired to a random node.
    pub rewire_probability: f64,
    /// How the pool's hashrate is spread over nodes.
    pub node_hashrate: HashrateSpread,
    /// Miners mining on the pool.
    pub miner_count: u64,
    /// How the pool's hashrate is spread over miners.
    pub miner_hashrate: HashrateSpread,
    /// Payees per coinbase, instead of the count expected from the miners
    /// and the window: to look at counts the window cannot reach.
    pub payees_override: Option<u64>,
    /// The pool's fraction of BCH's hashrate, which sizes the PPLNS window.
    pub pool_fraction_of_bch: f64,
    /// Mean seconds between shares.
    pub share_interval_seconds: f64,
    /// Bytes of the BCH blocks being mined.
    pub block_bytes: u64,
    /// Bytes of an average BCH transaction.
    pub average_transaction_bytes: u64,
    /// How the coinbase pays the payees.
    pub layout: CoinbaseLayout,
    /// How shares travel between nodes.
    pub relay: ShareRelay,
    /// Share transaction bytes beyond the share's own coinbase.
    pub extra_share_bytes: u64,
    /// Each node's upload, megabits per second.
    pub upload_megabits: Distribution,
    /// Each node's download, megabits per second.
    pub download_megabits: Distribution,
    /// Each link's one-way latency, milliseconds.
    pub latency_milliseconds: Distribution,
    /// Seconds to validate any share (an estimate).
    pub validation_seconds: f64,
    /// Further validation seconds per megabyte of share (an estimate).
    pub validation_seconds_per_megabyte: f64,
    /// Seconds from a new best share to the node's miners hashing on it.
    pub work_switch_seconds: f64,
    /// Whether BCHN uses the same link, so its relay and its compact blocks
    /// take bandwidth from the share chain.
    pub bchn_shares_link: bool,
    /// BCHN's peer links.
    pub bch_peers: u64,
    /// How the miners get their work.
    pub stratum: StratumProtocol,
    /// Seconds of mining.
    pub duration_seconds: f64,
}

impl Default for Scenario {
    fn default() -> Self {
        Self {
            label: "default".to_string(),
            seed: 1,
            node_count: 100,
            peers_per_node: 8,
            rewire_probability: 0.5,
            node_hashrate: HashrateSpread::Equal,
            miner_count: 1_000,
            miner_hashrate: HashrateSpread::Equal,
            payees_override: None,
            pool_fraction_of_bch: 0.01,
            share_interval_seconds: SHARE_INTERVAL_SECONDS,
            block_bytes: BCH_MAINNET_ABLA_FLOOR_BYTES,
            average_transaction_bytes: TYPICAL_TRANSACTION_BYTES,
            layout: CoinbaseLayout::no_tokens(),
            relay: ShareRelay::Full,
            extra_share_bytes: 0,
            upload_megabits: Distribution::LogNormal {
                median: 50.0,
                sigma: 0.8,
            },
            download_megabits: Distribution::LogNormal {
                median: 200.0,
                sigma: 0.8,
            },
            latency_milliseconds: Distribution::LogNormal {
                median: 50.0,
                sigma: 0.5,
            },
            validation_seconds: 0.005,
            validation_seconds_per_megabyte: 0.02,
            work_switch_seconds: 0.05,
            bchn_shares_link: true,
            bch_peers: BCHN_OUTBOUND_PEERS,
            stratum: StratumProtocol::V1,
            duration_seconds: 86_400.0,
        }
    }
}

/// How a share travels between nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ShareRelay {
    /// The whole `ShareBlock`, coinbase included, as nodes send it today.
    Full,
    /// Without the payout outputs, which every peer rebuilds from its own
    /// window to validate the share anyway: the compact share the hybrid
    /// payout proposal suggests (`bch-hybrid-payouts.md`, section 8.1).
    /// Not built; its size here is an estimate.
    WithoutPayouts,
}

impl ShareRelay {
    /// Parse `full` or `without-payouts`.
    pub fn parse(text: &str) -> Result<Self, String> {
        match text.trim() {
            "full" => Ok(ShareRelay::Full),
            "without-payouts" => Ok(ShareRelay::WithoutPayouts),
            other => Err(format!("expected full or without-payouts, got `{other}`")),
        }
    }

    /// The command-line name.
    pub fn name(&self) -> &'static str {
        match self {
            ShareRelay::Full => "full",
            ShareRelay::WithoutPayouts => "without-payouts",
        }
    }
}

/// What the network turned out to be.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NetworkSummary {
    /// Undirected peer links.
    pub links: usize,
    /// Whether every node can reach every other.
    pub connected: bool,
    /// Mean peers per node.
    pub mean_peers: f64,
    /// Nodes whose upload or download is too small for BCHN's relay alone.
    /// They cannot keep up with BCH, so they relay shares but mine none.
    pub nodes_short_for_bchn: usize,
    /// The pool's hashrate on those nodes, which the model moves to the
    /// other nodes.
    pub hashrate_short_for_bchn: f64,
    /// Median upload left for P2Pool, megabits per second.
    pub median_p2pool_upload_megabits: f64,
}

impl Scenario {
    /// Check the inputs make sense.
    pub fn validate(&self) -> Result<(), String> {
        if self.node_count == 0 {
            return Err("a scenario needs at least one node".to_string());
        }
        if !(self.share_interval_seconds > 0.0 && self.share_interval_seconds.is_finite()) {
            return Err("the share interval must be positive".to_string());
        }
        if !(self.duration_seconds > 0.0 && self.duration_seconds.is_finite()) {
            return Err("the duration must be positive".to_string());
        }
        if !(0.0..=1.0).contains(&self.rewire_probability) {
            return Err("the rewire probability must be between 0 and 1".to_string());
        }
        if !(0.0..=1.0).contains(&self.pool_fraction_of_bch) {
            return Err("the pool's fraction of BCH must be between 0 and 1".to_string());
        }
        if self.average_transaction_bytes == 0 {
            return Err("the average transaction must have bytes".to_string());
        }
        if self.validation_seconds < 0.0
            || self.validation_seconds_per_megabyte < 0.0
            || self.work_switch_seconds < 0.0
        {
            return Err("delays must not be negative".to_string());
        }
        Ok(())
    }

    /// Shares in the PPLNS window.
    pub fn window_shares(&self) -> u64 {
        pplns_window_shares(self.pool_fraction_of_bch, self.share_interval_seconds)
    }

    /// Payees per coinbase: the override, or the miners expected to hold a
    /// share in the window, rounded.
    pub fn payees(&self) -> u64 {
        self.payees_override.unwrap_or_else(|| {
            expected_payees(self.miner_count, self.miner_hashrate, self.window_shares()).round()
                as u64
        })
    }

    /// Sizes of a share in this scenario.
    pub fn share_sizes(&self) -> ShareSizes {
        ShareSizes::new(
            self.layout,
            self.payees(),
            self.block_bytes,
            self.average_transaction_bytes,
            self.extra_share_bytes,
        )
    }

    /// BCHN's load in this scenario.
    pub fn bchn_load(&self, coinbase_bytes: u64) -> BchnLoad {
        bchn_load(
            self.block_bytes,
            self.average_transaction_bytes,
            self.bch_peers,
            coinbase_bytes,
        )
    }

    /// The simulation parameters for shares of `sizes`.
    pub fn parameters(&self, sizes: &ShareSizes, load: &BchnLoad) -> Parameters {
        let full_bytes = sizes.message_payload_bytes(0);
        let share_wire_bytes = wire_bytes(match self.relay {
            ShareRelay::Full => full_bytes,
            ShareRelay::WithoutPayouts => sizes.message_payload_bytes_without_payouts(0),
        });
        // Validation rebuilds the whole coinbase however the share travelled.
        let validation_seconds = self.validation_seconds
            + self.validation_seconds_per_megabyte * full_bytes as f64 / 1_000_000.0;
        Parameters {
            share_interval_seconds: self.share_interval_seconds,
            share_wire_bytes,
            wire_bytes_per_uncle: SHARE_HEADER_BYTES_PER_UNCLE,
            ack_wire_bytes: wire_bytes(ACK_PAYLOAD_BYTES),
            validation_seconds,
            work_switch_seconds: self.work_switch_seconds,
            duration_seconds: self.duration_seconds,
            bch_block: self.bchn_shares_link.then_some(BchBlockLoad {
                interval_seconds: BCH_BLOCK_INTERVAL_SECONDS,
                compact_block_bytes: load.compact_block_bytes,
            }),
        }
    }

    /// Build the network: the graph, each link's latency and each node's
    /// bandwidth and hashrate. `relay_bytes_per_second` is BCHN's relay,
    /// taken from each link when BCHN shares it.
    pub fn build_network(
        &self,
        relay_bytes_per_second: f64,
        random: &mut SimulationRandom,
    ) -> (Network, NetworkSummary) {
        let mut topology_random = random.derive();
        let mut link_random = random.derive();
        let adjacency = topology::small_world(
            self.node_count,
            self.peers_per_node,
            self.rewire_probability,
            &mut topology_random,
        );
        let peers = self.peer_links(&adjacency, &mut link_random);
        let mut hashrates = hashrate_fractions(self.node_count as u64, self.node_hashrate);
        shuffle(&mut hashrates, &mut link_random);
        let background = if self.bchn_shares_link {
            relay_bytes_per_second
        } else {
            0.0
        };
        let links: Vec<(f64, f64)> = (0..self.node_count)
            .map(|_| {
                let upload = self.upload_megabits.sample(&mut link_random) * BYTES_PER_MEGABIT;
                let download = self.download_megabits.sample(&mut link_random) * BYTES_PER_MEGABIT;
                (upload, download)
            })
            .collect();
        let short: Vec<bool> = links
            .iter()
            .map(|&(upload, download)| background >= upload || background >= download)
            .collect();
        let nodes_short_for_bchn = short.iter().filter(|&&is_short| is_short).count();
        let hashrate_short_for_bchn: f64 = hashrates
            .iter()
            .zip(short.iter())
            .filter(|(_, is_short)| **is_short)
            .map(|(hashrate, _)| hashrate)
            .sum();
        // A node that cannot keep up with BCH mines on stale templates, so
        // its miners would mine elsewhere: the rest share the pool's
        // hashrate. If no node keeps up, all keep mining so the run still
        // shows what happens.
        let remaining = 1.0 - hashrate_short_for_bchn;
        let mining_share = |hashrate: f64, is_short: bool| {
            if remaining <= 1e-12 {
                hashrate
            } else if is_short {
                0.0
            } else {
                hashrate / remaining
            }
        };
        let nodes: Vec<NodeLink> = links
            .iter()
            .zip(hashrates.iter())
            .zip(short.iter())
            .map(|((&(upload, download), &hashrate), &is_short)| NodeLink {
                upload_bytes_per_second: left_for_p2pool(upload, background),
                download_bytes_per_second: left_for_p2pool(download, background),
                hashrate_fraction: mining_share(hashrate, is_short),
            })
            .collect();
        let mut uploads: Vec<f64> = nodes
            .iter()
            .map(|node| node.upload_bytes_per_second / BYTES_PER_MEGABIT)
            .collect();
        uploads.sort_by(f64::total_cmp);
        let summary = NetworkSummary {
            links: topology::link_count(&adjacency),
            connected: topology::is_connected(&adjacency),
            mean_peers: 2.0 * topology::link_count(&adjacency) as f64 / self.node_count as f64,
            nodes_short_for_bchn,
            hashrate_short_for_bchn,
            median_p2pool_upload_megabits: uploads[uploads.len() / 2],
        };
        (Network { nodes, peers }, summary)
    }

    /// Both ends of every link with one latency drawn per link.
    fn peer_links(
        &self,
        adjacency: &Adjacency,
        random: &mut SimulationRandom,
    ) -> Vec<Vec<PeerLink>> {
        let mut peers: Vec<Vec<PeerLink>> = adjacency
            .iter()
            .map(|neighbours| {
                neighbours
                    .iter()
                    .map(|&peer| PeerLink {
                        peer,
                        latency_seconds: 0.0,
                    })
                    .collect()
            })
            .collect();
        (0..adjacency.len()).for_each(|node| {
            (0..adjacency[node].len()).for_each(|index| {
                let peer = adjacency[node][index];
                if node < peer {
                    let latency = self.latency_milliseconds.sample(random) / 1_000.0;
                    peers[node][index].latency_seconds = latency;
                    let back = adjacency[peer]
                        .iter()
                        .position(|&other| other == node)
                        .expect("links are undirected");
                    peers[peer][back].latency_seconds = latency;
                }
            });
        });
        peers
    }

    /// Run the scenario.
    pub fn run(&self) -> Report {
        let sizes = self.share_sizes();
        let load = self.bchn_load(sizes.coinbase_bytes());
        let mut random = SimulationRandom::from_seed(self.seed);
        let (network, summary) = self.build_network(load.relay_bytes_per_second, &mut random);
        let parameters = self.parameters(&sizes, &load);
        let outcome = engine::simulate(&network, parameters, random.derive());
        Report::new(
            self.clone(),
            sizes,
            load,
            summary,
            parameters,
            &network,
            outcome,
        )
    }
}

/// Bytes per second left for P2Pool on a link of `link` once BCHN's
/// `background` is taken, never below `MINIMUM_LINK_SHARE` of the link.
fn left_for_p2pool(link: f64, background: f64) -> f64 {
    (link - background).max(link * MINIMUM_LINK_SHARE)
}

/// Fisher-Yates shuffle with the simulation generator.
fn shuffle(values: &mut [f64], random: &mut SimulationRandom) {
    (1..values.len()).rev().for_each(|index| {
        let other = random.next_below(index as u64 + 1) as usize;
        values.swap(index, other);
    });
}

#[cfg(test)]
mod tests;
