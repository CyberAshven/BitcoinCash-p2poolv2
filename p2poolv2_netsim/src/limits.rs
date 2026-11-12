// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The limits study: upstream's five practical objections to the scaling
//! study's ideas, and a bigger network, each run against the costs a node
//! really pays under the adopted rules.
//!
//! 1. A share must carry enough to check how it paid out: `limits-coinbase`
//!    sends shares whole, without the payouts every peer rebuilds, or as
//!    token shares (whose coinbase is 3,072 bytes and pays the rest through
//!    the payout tree), and charges each its own validation.
//! 2. Relaying before validation may not move the needle:
//!    `limits-relay-early` forwards after the header check or after full
//!    validation at each measured validation cost.
//! 3. More peers are bounded by upload bandwidth: `limits-fanout` runs 8 to
//!    64 peers on 20 and 100 Mbit/s uploads with whole and compact shares.
//! 4. A larger uncle cap may add latency: `limits-uncles` runs caps of 3, 6
//!    and 12 at 10, 2 and 1-second shares.
//! 5. Faster shares build a fatter DAG and load each node:
//!    `limits-period` runs 10 to 1-second shares with and without weak
//!    shares, at each measured validation cost, on a node five times
//!    slower, at the measured cost of payout level 0 and with token shares.
//!
//! `limits-network` then grows the network from 1,000 to 10,000 nodes.
//!
//! Every limits scenario validates one share at a time
//! (`ValidationQueue::Serial`), as a node's validation worker does, at the
//! costs `validation_cost_at_bitcoin_cash_window_sizes` in `p2poolv2_lib`
//! measured: the window walk and coinbase build for a 120,960 share window.

use crate::distribution::Distribution;
use crate::engine::{Forwarding, ValidationQueue};
use crate::report::group_thousands;
use crate::scaling::{LARGE_NETWORK_SHARE_INTERVALS, at_size};
use crate::scenario::{HEADER_CHECK_SECONDS, Scenario, ShareRelay};
use crate::sizes::CoinbaseLayout;
use crate::sweep::with_payees;

/// Nodes in every limits scenario but `limits-network`'s: upstream's stated
/// limit of about 1,000 miners, one node each.
pub const LIMITS_NODE_COUNT: usize = 1_000;

/// What checking one share costs a node, measured on the release build of
/// `p2poolv2_lib` (one core of a 2026 desktop, the median of seven builds).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeasuredCost {
    /// Miners with credit in the window.
    pub miners: u64,
    /// Outputs the block pays them through, in the coinbase or the payout
    /// tree: every miner, unless the level draw pays fewer.
    pub paid_outputs: u64,
    /// Whether the share carries a token commitment (a 3,072 byte coinbase).
    pub token_share: bool,
    /// The payout level every miner is at.
    pub level: u8,
    /// Seconds to check a share: the window walk and the coinbase build.
    pub share_seconds: f64,
    /// Seconds to check a weak share: the coinbase build alone; the walk is
    /// cached by parent.
    pub weak_share_seconds: f64,
}

/// The adopted rules: every miner at the default payout level 5 (17,472
/// satoshis), shares without tokens. At 100,000 equal miners a 3.125 BCH
/// block pays 17,883 of them by the level draw.
pub const ADOPTED: [MeasuredCost; 3] = [
    MeasuredCost {
        miners: 1_000,
        paid_outputs: 1_000,
        token_share: false,
        level: 5,
        share_seconds: 0.0064,
        weak_share_seconds: 0.00026,
    },
    MeasuredCost {
        miners: 10_000,
        paid_outputs: 10_000,
        token_share: false,
        level: 5,
        share_seconds: 0.0379,
        weak_share_seconds: 0.00375,
    },
    MeasuredCost {
        miners: 100_000,
        paid_outputs: 17_883,
        token_share: false,
        level: 5,
        share_seconds: 0.3615,
        weak_share_seconds: 0.04485,
    },
];

/// The same miners on token shares: 79 payees in the coinbase, the others in
/// the payout tree every validator rebuilds.
pub const TOKEN_SHARES: [MeasuredCost; 3] = [
    MeasuredCost {
        miners: 1_000,
        paid_outputs: 1_000,
        token_share: true,
        level: 5,
        share_seconds: 0.00735,
        weak_share_seconds: 0.00121,
    },
    MeasuredCost {
        miners: 10_000,
        paid_outputs: 10_000,
        token_share: true,
        level: 5,
        share_seconds: 0.0446,
        weak_share_seconds: 0.0105,
    },
    MeasuredCost {
        miners: 100_000,
        paid_outputs: 17_883,
        token_share: true,
        level: 5,
        share_seconds: 0.376,
        weak_share_seconds: 0.0593,
    },
];

/// The worst case measured while level 0 was a legal payout level: 100,000
/// miners all at payout level 0, so every one is paid, 29,400 in the
/// coinbase and 70,600 through the payout tree. Level 0 is no longer a
/// level; these times are that run's, not a level a node now chooses.
pub const LEVEL_ZERO_100K: MeasuredCost = MeasuredCost {
    miners: 100_000,
    paid_outputs: 100_000,
    token_share: false,
    level: 0,
    share_seconds: 0.4376,
    weak_share_seconds: 0.1328,
};

/// How much slower than the measuring desktop a small node (a single-board
/// computer) is taken to be.
pub const SLOW_NODE_FACTOR: f64 = 5.0;

/// Weak shares per share in the weak-share rows: `WEAK_FILL`.
const WEAK_SHARES_PER_SHARE: u64 = 32;

/// Share intervals each share-period and network run mines for: three hours
/// at 10-second shares. A thousand nodes tracking every weak share of a day
/// at 1-second shares need more memory than a desktop has.
const SHORT_RUN_INTERVALS: f64 = 1_080.0;

/// Node counts of the network sweep.
const NETWORK_NODE_COUNTS: [usize; 3] = [1_000, 3_000, 10_000];

/// Largest network the network sweep runs with weak shares: every weak share
/// is a message to every peer of every node.
const LARGEST_NETWORK_WITH_WEAK_SHARES: usize = 3_000;

/// One of the limits study's sweeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    /// Objection 1: whole shares, shares without payouts and token shares.
    Coinbase,
    /// Objection 2: forwarding after the header check.
    RelayEarly,
    /// Objection 3: peers against upload bandwidth.
    Fanout,
    /// Objection 4: the uncle cap.
    Uncles,
    /// Objection 5: the share period and each node's validation load.
    Period,
    /// A bigger network: 1,000 to 10,000 nodes.
    Network,
}

impl Limit {
    /// Every limits sweep: upstream's objections in order, then the network.
    pub const ALL: [Limit; 6] = [
        Limit::Coinbase,
        Limit::RelayEarly,
        Limit::Fanout,
        Limit::Uncles,
        Limit::Period,
        Limit::Network,
    ];

    /// The command-line name of the sweep.
    pub fn name(&self) -> &'static str {
        match self {
            Limit::Coinbase => "limits-coinbase",
            Limit::RelayEarly => "limits-relay-early",
            Limit::Fanout => "limits-fanout",
            Limit::Uncles => "limits-uncles",
            Limit::Period => "limits-period",
            Limit::Network => "limits-network",
        }
    }

    /// The sweep's scenarios, from `base`.
    pub fn scenarios(&self, base: &Scenario) -> Vec<Scenario> {
        match self {
            Limit::Coinbase => coinbase(base),
            Limit::RelayEarly => relay_early(base),
            Limit::Fanout => fanout(base),
            Limit::Uncles => uncles(base),
            Limit::Period => period(base),
            Limit::Network => network(base),
        }
    }
}

/// `base` paying `cost`'s outputs and validating one share at a time at
/// `cost` times `slowdown`, on `LIMITS_NODE_COUNT` nodes at `interval`
/// seconds.
fn measured(base: &Scenario, cost: MeasuredCost, slowdown: f64, interval: f64) -> Scenario {
    let layout = if cost.token_share {
        CoinbaseLayout::with_tokens()
    } else {
        CoinbaseLayout::no_tokens()
    };
    let paid = with_payees(base, layout, cost.paid_outputs);
    let scenario = Scenario {
        validation_seconds: cost.share_seconds * slowdown,
        validation_seconds_per_megabyte: 0.0,
        validation_queue: ValidationQueue::Serial {
            weak_share_seconds: cost.weak_share_seconds * slowdown,
        },
        ..paid
    };
    let mut sized = at_size(&scenario, LIMITS_NODE_COUNT, interval);
    sized.duration_seconds = LARGE_NETWORK_SHARE_INTERVALS * interval;
    sized
}

fn labelled(scenario: Scenario, label: String) -> Scenario {
    Scenario { label, ..scenario }
}

/// A scenario mining for `SHORT_RUN_INTERVALS` share intervals.
fn short(scenario: Scenario) -> Scenario {
    Scenario {
        duration_seconds: SHORT_RUN_INTERVALS * scenario.share_interval_seconds,
        ..scenario
    }
}

fn miners_label(cost: MeasuredCost) -> String {
    format!("{} miners", group_thousands(cost.miners))
}

fn relay_name(relay: ShareRelay) -> &'static str {
    match relay {
        ShareRelay::Full => "whole",
        ShareRelay::WithoutPayouts => "without payouts",
    }
}

fn weak_name(weak: u64) -> String {
    if weak == 0 {
        "no weak shares".to_string()
    } else {
        format!("{weak} weak shares a share")
    }
}

fn coinbase(base: &Scenario) -> Vec<Scenario> {
    [10.0, 2.0]
        .into_iter()
        .flat_map(|interval| {
            ADOPTED
                .into_iter()
                .zip(TOKEN_SHARES)
                .flat_map(move |(cost, token_cost)| {
                    let plain = [ShareRelay::Full, ShareRelay::WithoutPayouts].map(|relay| {
                        labelled(
                            Scenario {
                                relay,
                                ..measured(base, cost, 1.0, interval)
                            },
                            format!(
                                "{}, {}, {interval} s",
                                relay_name(relay),
                                miners_label(cost)
                            ),
                        )
                    });
                    let token = labelled(
                        measured(base, token_cost, 1.0, interval),
                        format!("token share, {}, {interval} s", miners_label(cost)),
                    );
                    plain.into_iter().chain([token])
                })
        })
        .collect()
}

fn relay_early(base: &Scenario) -> Vec<Scenario> {
    [10.0, 2.0]
        .into_iter()
        .flat_map(|interval| {
            ADOPTED.into_iter().flat_map(move |cost| {
                [
                    ("after validation", Forwarding::AfterValidation),
                    (
                        "after the header check",
                        Forwarding::AfterHeaderCheck {
                            header_check_seconds: HEADER_CHECK_SECONDS,
                        },
                    ),
                ]
                .map(|(name, forwarding)| {
                    labelled(
                        Scenario {
                            forwarding,
                            ..measured(base, cost, 1.0, interval)
                        },
                        format!("{name}, {}, {interval} s", miners_label(cost)),
                    )
                })
            })
        })
        .collect()
}

fn fanout(base: &Scenario) -> Vec<Scenario> {
    [20.0, 100.0]
        .into_iter()
        .flat_map(|upload| {
            [ShareRelay::Full, ShareRelay::WithoutPayouts]
                .into_iter()
                .flat_map(move |relay| {
                    [8, 16, 32, 64].map(|peers| {
                        labelled(
                            Scenario {
                                relay,
                                peers_per_node: peers,
                                upload_megabits: Distribution::Fixed { value: upload },
                                ..measured(base, ADOPTED[1], 1.0, 10.0)
                            },
                            format!(
                                "{}, {}, {upload} Mbit/s up, {peers} peers",
                                relay_name(relay),
                                miners_label(ADOPTED[1])
                            ),
                        )
                    })
                })
        })
        .collect()
}

fn uncles(base: &Scenario) -> Vec<Scenario> {
    [10.0, 2.0, 1.0]
        .into_iter()
        .flat_map(|interval| {
            [3, 6, 12].map(|cap| {
                labelled(
                    Scenario {
                        max_uncles: cap,
                        ..measured(base, ADOPTED[1], 1.0, interval)
                    },
                    format!(
                        "uncle cap {cap}, {}, {interval} s",
                        miners_label(ADOPTED[1])
                    ),
                )
            })
        })
        .collect()
}

/// `cost` at every share period, with `weak` weak shares a share, on nodes
/// `slowdown` times slower; `note` says what sets the rows apart.
fn period_rows(
    base: &Scenario,
    cost: MeasuredCost,
    slowdown: f64,
    weak: u64,
    note: &str,
) -> Vec<Scenario> {
    [10.0, 5.0, 2.0, 1.0]
        .map(|interval| {
            labelled(
                short(Scenario {
                    weak_shares_per_share: weak,
                    ..measured(base, cost, slowdown, interval)
                }),
                format!(
                    "{}{note}, {}, {interval} s",
                    miners_label(cost),
                    weak_name(weak)
                ),
            )
        })
        .to_vec()
}

fn period(base: &Scenario) -> Vec<Scenario> {
    ADOPTED
        .into_iter()
        .flat_map(|cost| {
            [0, WEAK_SHARES_PER_SHARE]
                .into_iter()
                .flat_map(move |weak| period_rows(base, cost, 1.0, weak, ""))
        })
        .chain(period_rows(
            base,
            ADOPTED[1],
            SLOW_NODE_FACTOR,
            WEAK_SHARES_PER_SHARE,
            ", nodes 5x slower",
        ))
        .chain(period_rows(
            base,
            TOKEN_SHARES[2],
            1.0,
            WEAK_SHARES_PER_SHARE,
            ", token shares",
        ))
        .chain(period_rows(
            base,
            LEVEL_ZERO_100K,
            1.0,
            WEAK_SHARES_PER_SHARE,
            ", all at level 0",
        ))
        .collect()
}

fn network(base: &Scenario) -> Vec<Scenario> {
    NETWORK_NODE_COUNTS
        .into_iter()
        .flat_map(|nodes| {
            [ShareRelay::Full, ShareRelay::WithoutPayouts]
                .into_iter()
                .flat_map(move |relay| {
                    let weak_counts: &[u64] = if nodes <= LARGEST_NETWORK_WITH_WEAK_SHARES {
                        &[0, WEAK_SHARES_PER_SHARE]
                    } else {
                        &[0]
                    };
                    weak_counts.iter().map(move |&weak| {
                        labelled(
                            short(Scenario {
                                relay,
                                node_count: nodes,
                                weak_shares_per_share: weak,
                                ..measured(base, ADOPTED[1], 1.0, 10.0)
                            }),
                            format!(
                                "{} nodes, {}, {}, {}",
                                group_thousands(nodes as u64),
                                relay_name(relay),
                                miners_label(ADOPTED[1]),
                                weak_name(weak)
                            ),
                        )
                    })
                })
        })
        .collect()
}

#[cfg(test)]
mod tests;
