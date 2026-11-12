// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The scaling study of issue #85 (`docs/sharechain/small-miners-design.md`,
//! "Sync and propagation cost" and "Scaling propagation instead of capping
//! it"): weak-share traffic and four ways to scale propagation, each run
//! beside today's design on 100 and 1,000 nodes at 10, 5, 2 and 1-second
//! shares; and, for each, the shortest share interval at which a network
//! stays within today's stale rate at 10-second shares, the operating point
//! of upstream's stated limit of about 1,000 miners.
//!
//! None of the ideas is built. Each is the engine's model of it
//! (`engine::Forwarding`, `engine::WeakShareLoad`, `Parameters::max_uncles`)
//! or a scenario input that already existed (`ShareRelay::WithoutPayouts`,
//! `peers_per_node`).

use crate::engine::Forwarding;
use crate::report::group_thousands;
use crate::scenario::{HEADER_CHECK_SECONDS, Scenario, ShareRelay, WeakShareCarriage};
use crate::sizes::CoinbaseLayout;
use crate::sweep::with_payees;

/// Node counts the study runs: the default network and upstream's limit.
pub const NODE_COUNTS: [usize; 2] = [100, 1_000];

/// Share intervals the study runs, seconds.
pub const SHARE_INTERVALS_SECONDS: [f64; 4] = [10.0, 5.0, 2.0, 1.0];

/// Networks up to this many nodes mine for the base scenario's duration (a
/// day by default), as every other table does; larger ones mine for
/// `LARGE_NETWORK_SHARE_INTERVALS` share intervals instead, so that a run at
/// short intervals on 1,000 nodes stays within memory (a day of 1-second
/// shares would hold 86,400 shares on 1,000 nodes, about 2.8 GB) and time.
pub const LARGEST_DAY_LONG_NETWORK: usize = 100;

/// Share intervals a run on a larger network mines for: as many shares as a
/// day at 10-second shares.
pub const LARGE_NETWORK_SHARE_INTERVALS: f64 = 8_640.0;

/// The share interval of the limit the study measures against: upstream's
/// 10-second shares.
pub const REFERENCE_SHARE_INTERVAL_SECONDS: f64 = 10.0;

/// Shortest and longest share intervals the limit search tries, seconds.
pub const SEARCH_SHORTEST_INTERVAL_SECONDS: f64 = 0.25;
pub const SEARCH_LONGEST_INTERVAL_SECONDS: f64 = 40.0;

/// Halvings of the searched range, on a log scale, after its first two runs:
/// six bring the 40-fold range from 0.25 to 10 seconds to within 6%.
pub const SEARCH_STEPS: u32 = 6;

/// Weak shares per share the study tries.
const WEAK_SHARE_COUNTS: [u64; 3] = [16, 32, 64];

/// Peers per node the study tries beyond today's eight.
const MORE_PEERS: [usize; 3] = [16, 32, 64];

/// Peers per node in the combination.
const COMBINED_PEERS: [usize; 4] = [8, 16, 32, 64];

/// Uncle caps the study tries beyond today's three.
const LARGER_UNCLE_CAPS: [usize; 2] = [6, 12];

/// One idea of the study, with the scenarios it is run as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Idea {
    /// Today's design, the before figures: the default scenario.
    Today,
    /// 16, 32 and 64 weak shares per share, gossiped only, and also carried
    /// whole in the share's body.
    WeakShares,
    /// Shares sent without the payout outputs every peer rebuilds, against
    /// whole shares with a 3 KB and a 200 KiB coinbase.
    CompactCoinbase,
    /// Shares forwarded once their header checks out.
    HeaderCheckForwarding,
    /// 16, 32 and 64 peers per node.
    PeerDegree,
    /// An uncle cap of 6 and 12.
    UncleCap,
    /// Shares sent without payouts and forwarded after the header check, on
    /// 8, 16, 32 and 64 peers per node.
    Combined,
}

impl Idea {
    /// Every idea, in the order the documentation shows them.
    pub const ALL: [Idea; 7] = [
        Idea::Today,
        Idea::WeakShares,
        Idea::CompactCoinbase,
        Idea::HeaderCheckForwarding,
        Idea::PeerDegree,
        Idea::UncleCap,
        Idea::Combined,
    ];

    /// The command-line name of the idea's sweep.
    pub fn name(&self) -> &'static str {
        match self {
            Idea::Today => "scaling-today",
            Idea::WeakShares => "scaling-weak-shares",
            Idea::CompactCoinbase => "scaling-compact-coinbase",
            Idea::HeaderCheckForwarding => "scaling-header-check",
            Idea::PeerDegree => "scaling-peers",
            Idea::UncleCap => "scaling-uncle-cap",
            Idea::Combined => "scaling-combined",
        }
    }

    /// The idea's scenarios, labelled, at `base`'s node count and share
    /// interval.
    pub fn variants(&self, base: &Scenario) -> Vec<Scenario> {
        match self {
            Idea::Today => vec![Scenario {
                label: "today".to_string(),
                ..base.clone()
            }],
            Idea::WeakShares => weak_share_variants(base),
            Idea::CompactCoinbase => compact_coinbase_variants(base),
            Idea::HeaderCheckForwarding => vec![Scenario {
                label: "forwarded after the header check".to_string(),
                forwarding: header_check_forwarding(),
                ..base.clone()
            }],
            Idea::PeerDegree => MORE_PEERS
                .map(|peers| Scenario {
                    label: format!("{peers} peers"),
                    peers_per_node: peers,
                    ..base.clone()
                })
                .to_vec(),
            Idea::UncleCap => LARGER_UNCLE_CAPS
                .map(|cap| Scenario {
                    label: format!("uncle cap {cap}"),
                    max_uncles: cap,
                    ..base.clone()
                })
                .to_vec(),
            Idea::Combined => COMBINED_PEERS
                .map(|peers| Scenario {
                    label: format!("without payouts, header check, {peers} peers"),
                    relay: ShareRelay::WithoutPayouts,
                    forwarding: header_check_forwarding(),
                    peers_per_node: peers,
                    ..base.clone()
                })
                .to_vec(),
        }
    }
}

/// Forwarding after the header check, which takes `HEADER_CHECK_SECONDS`.
fn header_check_forwarding() -> Forwarding {
    Forwarding::AfterHeaderCheck {
        header_check_seconds: HEADER_CHECK_SECONDS,
    }
}

fn weak_share_variants(base: &Scenario) -> Vec<Scenario> {
    WEAK_SHARE_COUNTS
        .iter()
        .flat_map(|&count| {
            [
                (WeakShareCarriage::GossipOnly, "gossip only"),
                (WeakShareCarriage::Whole, "carried whole"),
            ]
            .map(|(carriage, carriage_label)| Scenario {
                label: format!("{count} weak shares a share, {carriage_label}"),
                weak_shares_per_share: count,
                weak_share_carriage: carriage,
                ..base.clone()
            })
        })
        .collect()
}

fn compact_coinbase_variants(base: &Scenario) -> Vec<Scenario> {
    let small = with_payees(base, CoinbaseLayout::with_tokens(), 1_000);
    let large = with_payees(base, CoinbaseLayout::no_tokens(), 6_010);
    vec![
        Scenario {
            label: "3 KB coinbase, whole shares".to_string(),
            ..small
        },
        Scenario {
            label: "200 KiB coinbase, whole shares".to_string(),
            ..large.clone()
        },
        Scenario {
            label: "34 KB coinbase, without payouts".to_string(),
            relay: ShareRelay::WithoutPayouts,
            ..base.clone()
        },
        Scenario {
            label: "200 KiB coinbase, without payouts".to_string(),
            relay: ShareRelay::WithoutPayouts,
            ..large
        },
    ]
}

/// Seconds a study run on `node_count` nodes at `share_interval_seconds`
/// mines for: `base_duration_seconds` on small networks,
/// `LARGE_NETWORK_SHARE_INTERVALS` intervals on larger ones.
pub fn study_duration_seconds(
    base_duration_seconds: f64,
    node_count: usize,
    share_interval_seconds: f64,
) -> f64 {
    if node_count > LARGEST_DAY_LONG_NETWORK {
        LARGE_NETWORK_SHARE_INTERVALS * share_interval_seconds
    } else {
        base_duration_seconds
    }
}

/// `variant` on `node_count` nodes at `share_interval_seconds`, labelled
/// with both.
pub fn at_size(variant: &Scenario, node_count: usize, share_interval_seconds: f64) -> Scenario {
    Scenario {
        label: format!(
            "{}, {} nodes, {share_interval_seconds} s shares",
            variant.label,
            group_thousands(node_count as u64)
        ),
        node_count,
        share_interval_seconds,
        duration_seconds: study_duration_seconds(
            variant.duration_seconds,
            node_count,
            share_interval_seconds,
        ),
        ..variant.clone()
    }
}

/// Every variant at every study node count and share interval: variant by
/// variant, then by node count, then from the longest interval down.
pub fn grid(variants: &[Scenario]) -> Vec<Scenario> {
    variants
        .iter()
        .flat_map(|variant| {
            NODE_COUNTS.iter().flat_map(move |&node_count| {
                SHARE_INTERVALS_SECONDS
                    .iter()
                    .map(move |&interval| at_size(variant, node_count, interval))
            })
        })
        .collect()
}

/// The shortest share interval between `shortest` and `longest` whose stale
/// rate, as `stale_at` gives it, is at most `ceiling`. It tries `start`
/// first, then whichever end of the range lies past the crossing, then
/// halves the bracket on a log scale `steps` times, and returns the shortest
/// interval tried that was within the ceiling. It assumes the stale rate
/// falls as the interval grows. `None` when even `longest` is above the
/// ceiling; `shortest` when that is already within it.
pub fn shortest_interval_within(
    ceiling: f64,
    start: f64,
    shortest: f64,
    longest: f64,
    steps: u32,
    mut stale_at: impl FnMut(f64) -> f64,
) -> Option<f64> {
    let start_within = stale_at(start) <= ceiling;
    let end = if start_within { shortest } else { longest };
    let end_within = stale_at(end) <= ceiling;
    let mut bisect = |above: f64, within: f64| {
        let (_, within) = (0..steps).fold((above, within), |(above, within), _| {
            let middle = (above * within).sqrt();
            if stale_at(middle) <= ceiling {
                (above, middle)
            } else {
                (middle, within)
            }
        });
        within
    };
    match (start_within, end_within) {
        (true, true) => Some(shortest),
        (true, false) => Some(bisect(end, start)),
        (false, true) => Some(bisect(start, end)),
        (false, false) => None,
    }
}

#[cfg(test)]
mod tests;
