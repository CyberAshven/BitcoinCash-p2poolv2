// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The limits study: upstream's five practical objections to the scaling
//! study's ideas, each run against the costs a node really pays.
//!
//! 1. A share must carry enough to check how it paid out: `limits-coinbase`
//!    sends shares whole or without the payouts every peer rebuilds, and
//!    charges both the same validation.
//! 2. Relaying before validation may not move the needle:
//!    `limits-relay-early` forwards after the header check or after full
//!    validation at each measured validation cost.
//! 3. More peers are bounded by upload bandwidth: `limits-fanout` runs 8 to
//!    64 peers on 20 and 100 Mbit/s uploads with whole and compact shares.
//! 4. A larger uncle cap may add latency: `limits-uncles` runs caps of 3, 6
//!    and 12 at 10, 2 and 1-second shares.
//! 5. Faster shares build a fatter DAG and load each node:
//!    `limits-period` runs 10 to 1-second shares with and without weak
//!    shares, at each measured validation cost and on a node five times
//!    slower.
//!
//! Every limits scenario validates one share at a time
//! (`ValidationQueue::Serial`), as a node's validation worker does, at the
//! costs `validation_cost_at_bitcoin_cash_window_sizes` in `p2poolv2_lib`
//! measured: the window walk and coinbase build for a 120,960 share window.

use crate::distribution::Distribution;
use crate::engine::{Forwarding, ValidationQueue};
use crate::scaling::{LARGE_NETWORK_SHARE_INTERVALS, at_size};
use crate::scenario::{HEADER_CHECK_SECONDS, Scenario, ShareRelay};
use crate::sizes::CoinbaseLayout;
use crate::sweep::with_payees;

/// Nodes in every limits scenario: upstream's stated limit of about 1,000
/// miners, one node each.
pub const LIMITS_NODE_COUNT: usize = 1_000;

/// What checking one share costs a node, measured on the release build of
/// `p2poolv2_lib` (one core of a 2026 desktop): payees in the window, the
/// share's validation seconds (window walk plus coinbase build) and a weak
/// share's (coinbase build alone; the walk is cached by parent).
pub const MEASURED_VALIDATION: [(u64, f64, f64); 3] = [
    (1_000, 0.0066, 0.0003),
    (10_000, 0.0383, 0.0040),
    (100_000, 0.4713, 0.1195),
];

/// How much slower than the measuring desktop a small node (a single-board
/// computer) is taken to be.
pub const SLOW_NODE_FACTOR: f64 = 5.0;

/// Weak shares per share in the weak-share rows: `WEAK_FILL`.
const WEAK_SHARES_PER_SHARE: u64 = 32;

/// Share intervals each share-period run mines for: three hours at 10-second
/// shares. A thousand nodes tracking every weak share of a day at 1-second
/// shares needs more memory than a desktop has.
const PERIOD_RUN_INTERVALS: f64 = 1_080.0;

/// One of the limits study's sweeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    /// Objection 1: whole shares against shares without payouts.
    Coinbase,
    /// Objection 2: forwarding after the header check.
    RelayEarly,
    /// Objection 3: peers against upload bandwidth.
    Fanout,
    /// Objection 4: the uncle cap.
    Uncles,
    /// Objection 5: the share period and each node's validation load.
    Period,
}

impl Limit {
    /// Every limits sweep, in the order of upstream's objections.
    pub const ALL: [Limit; 5] = [
        Limit::Coinbase,
        Limit::RelayEarly,
        Limit::Fanout,
        Limit::Uncles,
        Limit::Period,
    ];

    /// The command-line name of the sweep.
    pub fn name(&self) -> &'static str {
        match self {
            Limit::Coinbase => "limits-coinbase",
            Limit::RelayEarly => "limits-relay-early",
            Limit::Fanout => "limits-fanout",
            Limit::Uncles => "limits-uncles",
            Limit::Period => "limits-period",
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
        }
    }
}

/// `base` with `payees` paid, validating one share at a time at the measured
/// cost for that many payees times `slowdown`, on `LIMITS_NODE_COUNT` nodes
/// at `interval` seconds.
fn measured(base: &Scenario, payees: u64, slowdown: f64, interval: f64) -> Scenario {
    let (_, share_seconds, weak_seconds) = MEASURED_VALIDATION
        .iter()
        .copied()
        .find(|(measured_payees, _, _)| *measured_payees == payees)
        .unwrap_or(MEASURED_VALIDATION[0]);
    let paid = with_payees(base, CoinbaseLayout::no_tokens(), payees);
    let scenario = Scenario {
        validation_seconds: share_seconds * slowdown,
        validation_seconds_per_megabyte: 0.0,
        validation_queue: ValidationQueue::Serial {
            weak_share_seconds: weak_seconds * slowdown,
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

fn payees_label(payees: u64) -> String {
    crate::report::group_thousands(payees)
}

fn coinbase(base: &Scenario) -> Vec<Scenario> {
    [10.0, 2.0]
        .into_iter()
        .flat_map(|interval| {
            MEASURED_VALIDATION
                .into_iter()
                .flat_map(move |(payees, _, _)| {
                    [ShareRelay::Full, ShareRelay::WithoutPayouts].map(|relay| {
                        let name = match relay {
                            ShareRelay::Full => "whole",
                            ShareRelay::WithoutPayouts => "without payouts",
                        };
                        labelled(
                            Scenario {
                                relay,
                                ..measured(base, payees, 1.0, interval)
                            },
                            format!("{name}, {} payees, {interval} s", payees_label(payees)),
                        )
                    })
                })
        })
        .collect()
}

fn relay_early(base: &Scenario) -> Vec<Scenario> {
    [10.0, 2.0]
        .into_iter()
        .flat_map(|interval| {
            MEASURED_VALIDATION
                .into_iter()
                .flat_map(move |(payees, _, _)| {
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
                                ..measured(base, payees, 1.0, interval)
                            },
                            format!("{name}, {} payees, {interval} s", payees_label(payees)),
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
                        let name = match relay {
                            ShareRelay::Full => "whole",
                            ShareRelay::WithoutPayouts => "without payouts",
                        };
                        labelled(
                            Scenario {
                                relay,
                                peers_per_node: peers,
                                upload_megabits: Distribution::Fixed { value: upload },
                                ..measured(base, 10_000, 1.0, 10.0)
                            },
                            format!("{name}, 10,000 payees, {upload} Mbit/s up, {peers} peers"),
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
                        ..measured(base, 10_000, 1.0, interval)
                    },
                    format!("uncle cap {cap}, 10,000 payees, {interval} s"),
                )
            })
        })
        .collect()
}

fn period(base: &Scenario) -> Vec<Scenario> {
    let short = |scenario: Scenario| Scenario {
        duration_seconds: PERIOD_RUN_INTERVALS * scenario.share_interval_seconds,
        ..scenario
    };
    let intervals = [10.0, 5.0, 2.0, 1.0];
    let by_payees = MEASURED_VALIDATION
        .into_iter()
        .flat_map(move |(payees, _, _)| {
            [0, WEAK_SHARES_PER_SHARE]
                .into_iter()
                .flat_map(move |weak| {
                    intervals.map(|interval| {
                        let weak_name = if weak == 0 {
                            "no weak shares".to_string()
                        } else {
                            format!("{weak} weak shares a share")
                        };
                        labelled(
                            Scenario {
                                weak_shares_per_share: weak,
                                ..measured(base, payees, 1.0, interval)
                            },
                            format!("{} payees, {weak_name}, {interval} s", payees_label(payees)),
                        )
                    })
                })
        });
    let slow = intervals.map(|interval| {
        labelled(
            Scenario {
                weak_shares_per_share: WEAK_SHARES_PER_SHARE,
                ..measured(base, 10_000, SLOW_NODE_FACTOR, interval)
            },
            format!(
                "10,000 payees, {WEAK_SHARES_PER_SHARE} weak shares a share, nodes {SLOW_NODE_FACTOR}x slower, {interval} s"
            ),
        )
    });
    by_payees.chain(slow).map(short).collect()
}

#[cfg(test)]
mod tests;
