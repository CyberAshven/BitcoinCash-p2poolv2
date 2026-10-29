// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The sweeps behind `docs/simulation/network-sim.md`: one input varied at a
//! time from a base scenario, each run with the base's seed.

use crate::constants::{
    BCH_CHIPNET_ABLA_FLOOR_BYTES, BCH_MAINNET_ABLA_FLOOR_BYTES, BCH_MAX_CONSENSUS_BLOCK_BYTES,
    BCH_SCALENET_LIMIT_BYTES,
};
use crate::distribution::Distribution;
use crate::report::{Report, group_thousands};
use crate::scenario::{Scenario, ShareRelay};
use crate::sizes::CoinbaseLayout;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;

/// A family of runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sweep {
    /// Payees per coinbase under each coinbase layout.
    Payees,
    /// Whole shares against shares sent without their payouts.
    Relay,
    /// The pool's share of BCH, which sizes the window, with a million
    /// miners.
    PoolSize,
    /// BCH block size on mixed and on data-centre links.
    BlockSize,
    /// Node bandwidth for three share sizes.
    Bandwidth,
    /// Link latency.
    Latency,
    /// Share interval for two share sizes.
    Interval,
    /// Number of nodes.
    Nodes,
    /// The base scenario under eight seeds, to show the noise.
    Seeds,
}

impl Sweep {
    /// Every sweep, in the order the documentation shows them.
    pub const ALL: [Sweep; 9] = [
        Sweep::Payees,
        Sweep::Relay,
        Sweep::PoolSize,
        Sweep::BlockSize,
        Sweep::Bandwidth,
        Sweep::Latency,
        Sweep::Interval,
        Sweep::Nodes,
        Sweep::Seeds,
    ];

    /// Parse a sweep's command-line name.
    pub fn parse(text: &str) -> Result<Self, String> {
        Sweep::ALL
            .iter()
            .copied()
            .find(|sweep| sweep.name() == text.trim())
            .ok_or_else(|| {
                let names: Vec<&str> = Sweep::ALL.iter().map(Sweep::name).collect();
                format!(
                    "unknown sweep `{text}`; expected one of {}",
                    names.join(", ")
                )
            })
    }

    /// The command-line name.
    pub fn name(&self) -> &'static str {
        match self {
            Sweep::Payees => "payees",
            Sweep::Relay => "relay",
            Sweep::PoolSize => "pool-size",
            Sweep::BlockSize => "block-size",
            Sweep::Bandwidth => "bandwidth",
            Sweep::Latency => "latency",
            Sweep::Interval => "interval",
            Sweep::Nodes => "nodes",
            Sweep::Seeds => "seeds",
        }
    }

    /// The scenarios of this sweep, varied from `base`.
    pub fn scenarios(&self, base: &Scenario) -> Vec<Scenario> {
        match self {
            Sweep::Payees => payees_scenarios(base),
            Sweep::Relay => relay_scenarios(base),
            Sweep::PoolSize => pool_size_scenarios(base),
            Sweep::BlockSize => block_size_scenarios(base),
            Sweep::Bandwidth => bandwidth_scenarios(base),
            Sweep::Latency => latency_scenarios(base),
            Sweep::Interval => interval_scenarios(base),
            Sweep::Nodes => node_scenarios(base),
            Sweep::Seeds => seed_scenarios(base),
        }
    }
}

/// `base` with a payee count and a layout, and at least as many miners as
/// payees.
fn with_payees(base: &Scenario, layout: CoinbaseLayout, payees: u64) -> Scenario {
    Scenario {
        label: format!("{}, {} payees", layout.name(), group_thousands(payees)),
        layout,
        payees_override: Some(payees),
        miner_count: base.miner_count.max(payees),
        ..base.clone()
    }
}

fn payees_scenarios(base: &Scenario) -> Vec<Scenario> {
    let no_tokens = [10, 100, 1_000, 6_010, 10_000, 29_402, 100_000, 1_000_000]
        .map(|payees| with_payees(base, CoinbaseLayout::no_tokens(), payees));
    let tokens = [100, 10_000, 1_000_000]
        .map(|payees| with_payees(base, CoinbaseLayout::with_tokens(), payees));
    let capped = [1_000_000].map(|payees| with_payees(base, CoinbaseLayout::cap_11(), payees));
    let uncapped = [100_000].map(|payees| with_payees(base, CoinbaseLayout::Uncapped, payees));
    no_tokens
        .into_iter()
        .chain(tokens)
        .chain(capped)
        .chain(uncapped)
        .collect()
}

fn relay_scenarios(base: &Scenario) -> Vec<Scenario> {
    [ShareRelay::Full, ShareRelay::WithoutPayouts]
        .iter()
        .flat_map(|&relay| {
            [1_000, 6_010, 29_402].map(|payees| {
                let shaped = with_payees(base, CoinbaseLayout::no_tokens(), payees);
                Scenario {
                    label: format!("{}, {} relay", shaped.label, relay.name()),
                    relay,
                    ..shaped
                }
            })
        })
        .collect()
}

fn pool_size_scenarios(base: &Scenario) -> Vec<Scenario> {
    let fractions = [
        ("0.1%", 0.001),
        ("0.3%", 0.003),
        ("1%", 0.01),
        ("3%", 0.03),
        ("10%", 0.1),
        ("30%", 0.3),
    ];
    [CoinbaseLayout::no_tokens(), CoinbaseLayout::with_tokens()]
        .iter()
        .flat_map(|&layout| {
            fractions.iter().map(move |&(name, fraction)| Scenario {
                label: format!("{}, pool {name} of BCH", layout.name()),
                layout,
                miner_count: 1_000_000,
                payees_override: None,
                pool_fraction_of_bch: fraction,
                ..base.clone()
            })
        })
        .collect()
}

/// Links of a well-connected server: a gigabit each way.
fn data_centre(base: &Scenario) -> Scenario {
    Scenario {
        upload_megabits: Distribution::fixed(1_000.0),
        download_megabits: Distribution::fixed(1_000.0),
        latency_milliseconds: Distribution::LogNormal {
            median: 30.0,
            sigma: 0.5,
        },
        ..base.clone()
    }
}

fn block_size_scenarios(base: &Scenario) -> Vec<Scenario> {
    let sizes = [
        ("2 MB", BCH_CHIPNET_ABLA_FLOOR_BYTES),
        ("32 MB", BCH_MAINNET_ABLA_FLOOR_BYTES),
        ("256 MB", BCH_SCALENET_LIMIT_BYTES),
        ("1 GB", 1_000_000_000),
        ("2 GB", BCH_MAX_CONSENSUS_BLOCK_BYTES),
    ];
    let mixed = sizes.map(|(name, bytes)| Scenario {
        label: format!("{name} blocks, mixed links"),
        block_bytes: bytes,
        ..base.clone()
    });
    let servers = sizes.map(|(name, bytes)| Scenario {
        label: format!("{name} blocks, data-centre links"),
        block_bytes: bytes,
        ..data_centre(base)
    });
    mixed.into_iter().chain(servers).collect()
}

fn bandwidth_scenarios(base: &Scenario) -> Vec<Scenario> {
    let uploads = [5.0, 20.0, 50.0, 200.0, 1_000.0];
    let shares = [
        (CoinbaseLayout::with_tokens(), 1_000),
        (CoinbaseLayout::no_tokens(), 6_010),
        (CoinbaseLayout::no_tokens(), 29_402),
    ];
    shares
        .iter()
        .flat_map(|&(layout, payees)| {
            uploads.iter().map(move |&upload| {
                let shaped = with_payees(base, layout, payees);
                Scenario {
                    label: format!("{}, upload {upload} Mbit/s", shaped.label),
                    upload_megabits: Distribution::LogNormal {
                        median: upload,
                        sigma: 0.8,
                    },
                    download_megabits: Distribution::LogNormal {
                        median: 4.0 * upload,
                        sigma: 0.8,
                    },
                    ..shaped
                }
            })
        })
        .collect()
}

fn latency_scenarios(base: &Scenario) -> Vec<Scenario> {
    [10.0, 50.0, 150.0, 400.0]
        .map(|latency| Scenario {
            label: format!("latency {latency} ms"),
            latency_milliseconds: Distribution::LogNormal {
                median: latency,
                sigma: 0.5,
            },
            ..base.clone()
        })
        .into_iter()
        .collect()
}

fn interval_scenarios(base: &Scenario) -> Vec<Scenario> {
    let intervals = [1.0, 2.0, 5.0, 10.0, 30.0];
    let shares = [
        (CoinbaseLayout::with_tokens(), 1_000),
        (CoinbaseLayout::no_tokens(), 6_010),
    ];
    shares
        .iter()
        .flat_map(|&(layout, payees)| {
            intervals.iter().map(move |&interval| {
                let shaped = with_payees(base, layout, payees);
                Scenario {
                    label: format!("{}, {interval} s shares", shaped.label),
                    share_interval_seconds: interval,
                    ..shaped
                }
            })
        })
        .collect()
}

fn node_scenarios(base: &Scenario) -> Vec<Scenario> {
    [10, 50, 200, 1_000]
        .map(|nodes| Scenario {
            label: format!("{nodes} nodes"),
            node_count: nodes,
            ..base.clone()
        })
        .into_iter()
        .collect()
}

fn seed_scenarios(base: &Scenario) -> Vec<Scenario> {
    (1..=8)
        .map(|seed| Scenario {
            label: format!("seed {seed}"),
            seed,
            ..base.clone()
        })
        .collect()
}

/// Run `scenarios` on every core, returning the reports in order. Each run
/// depends only on its own scenario, so the result does not depend on the
/// number of threads.
pub fn run_scenarios(scenarios: &[Scenario]) -> Vec<Report> {
    let threads = thread::available_parallelism()
        .map(NonZeroUsize::get)
        .unwrap_or(1)
        .min(scenarios.len().max(1));
    let next = AtomicUsize::new(0);
    let (sender, receiver) = mpsc::channel();
    thread::scope(|scope| {
        (0..threads).for_each(|_| {
            let sender = sender.clone();
            let next = &next;
            scope.spawn(move || {
                let mut index = next.fetch_add(1, Ordering::Relaxed);
                while index < scenarios.len() {
                    // The receiver outlives the scope, so a send cannot fail.
                    let _ = sender.send((index, scenarios[index].run()));
                    index = next.fetch_add(1, Ordering::Relaxed);
                }
            });
        });
    });
    drop(sender);
    let mut slots: Vec<Option<Report>> = vec![None; scenarios.len()];
    receiver.into_iter().for_each(|(index, report)| {
        slots[index] = Some(report);
    });
    slots
        .into_iter()
        .map(|slot| slot.expect("every scenario ran"))
        .collect()
}

#[cfg(test)]
mod tests;
