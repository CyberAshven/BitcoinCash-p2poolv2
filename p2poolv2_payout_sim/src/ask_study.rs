// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The tables that test section 6 of the design page: weak shares whose
//! number follows what the miners at the minimum ask for (`crate::asks`).
//!
//! Every table runs the adopted design as the scale table does: the band,
//! weak shares and payout levels, every proof on a job at the minimum
//! credited at the weak work, and every class at the payout level a node
//! would leave its machine at.

use crate::asks::{AskRule, WeakAsks};
use crate::constants::{MINI_CEILING_DIFFICULTY, WEAK_FILL_MAX};
use crate::miners::MinerMix;
use crate::report::{
    income_ratio, markdown_table, optional, percent, pool_percent, rounded, two_decimals,
};
use crate::scenario::{Report, Scenario};
use crate::study::{ScaleLever, lever_scenario, scale_scenario, smallest_regular_miner};
use crate::tables::{run_scenarios, weak_ratio};

/// Pool sizes of the ask tables, as parts of Bitcoin Cash's hashrate.
const ASK_POOL_FRACTIONS: [f64; 2] = [0.10, 0.30];

/// Bitcoin Cash hashrate multiples of the settling table: today's 3.7 EH/s
/// up to Bitcoin's, about 270 times it.
const ASK_HASHRATE_MULTIPLES: [f64; 4] = [1.0, 10.0, 100.0, 270.0];

/// The hashrate multiple the other ask tables run at: ten times today's,
/// where a pool at 10% of Bitcoin Cash is as large as all of it is today.
const ASK_STUDY_MULTIPLE: f64 = 10.0;

/// The miner mixes of the mixes table, in table order.
const ASK_MIXES: [MinerMix; 7] = [
    MinerMix::Home,
    MinerMix::HalfIndustrial,
    MinerMix::MostlyIndustrial,
    MinerMix::AlmostAllIndustrial,
    MinerMix::Zipf,
    MinerMix::Medium,
    MinerMix::MediumFewBig,
];

/// `WEAK_ASK_DIVISOR` values of the constants table.
const ASK_DIVISORS: [u64; 3] = [16, 64, 256];

/// Node defaults of the constants table: weak shares a month below which a
/// miner's node asks.
const ASK_NODE_DEFAULTS: [f64; 3] = [4.0, 16.0, 64.0];

/// `WEAK_FILL_MAX` values of the constants table.
const ASK_FILL_MAXIMA: [u64; 3] = [64, 160, 320];

/// Parts of the hashrate at the minimum that ask whatever their miners
/// find, in the attack table.
const ALWAYS_ASKING: [f64; 5] = [0.0, 0.01, 0.02, 0.05, 0.25];

/// Parts of the hashrate at the minimum that never ask, in the attack table.
const NEVER_ASKING: [f64; 4] = [0.5, 0.9, 0.97, 0.99];

/// Fractions of chain shares from nodes that carry no asking weak share, in
/// the attack table.
const CENSORING_SHARES: [f64; 4] = [0.25, 0.5, 0.75, 0.9];

/// How a row's weak shares are steered.
#[derive(Debug, Clone, Copy, PartialEq)]
enum WeakRule {
    /// The rules as they are: `WEAK_FILL` weak shares a share.
    Today,
    /// The proposal.
    Asks(WeakAsks),
    /// The rules as they are with `WEAK_FILL` raised to the proposal's
    /// `WEAK_FILL_MAX`, the other way to give small miners more credit.
    AtTheCeiling,
}

impl WeakRule {
    fn name(&self) -> &'static str {
        match self {
            WeakRule::Today => "today",
            WeakRule::Asks(_) => "asks",
            WeakRule::AtTheCeiling => "fill 320",
        }
    }
}

/// The scale table's scenario at `multiple` and `fraction`, its weak shares
/// steered by `rule`.
fn ruled_scenario(base: &Scenario, multiple: f64, fraction: f64, rule: WeakRule) -> Scenario {
    match rule {
        WeakRule::Today => scale_scenario(base, multiple, fraction),
        WeakRule::Asks(asks) => Scenario {
            weak_asks: Some(asks),
            ..scale_scenario(base, multiple, fraction)
        },
        WeakRule::AtTheCeiling => {
            let today = scale_scenario(base, multiple, fraction);
            let times = WEAK_FILL_MAX / today.weak_fill;
            lever_scenario(base, multiple, fraction, ScaleLever::WeakShares(times))
        }
    }
}

/// The cells every ask table ends with: what the chain's weak shares settle
/// at, what relaying them costs, and what the Bitaxe and the ASIC get.
fn outcome_cells(report: &Report) -> Vec<String> {
    let chain = report.home_chain();
    let weak = chain.weak.unwrap_or_default();
    let weak_difficulty = (chain.asert_difficulty / weak.mean_ratio).max(MINI_CEILING_DIFFICULTY);
    let mut cells = vec![
        rounded(weak.carried_per_share),
        weak_ratio(report),
        percent(weak.asking_fraction),
        rounded(report.weak_shares_per_second()),
        rounded(report.bytes_per_second() / 1_000.0),
        rounded(smallest_regular_miner(weak_difficulty) / 1.0e12),
    ];
    report.miners.iter().for_each(|miner| {
        cells.extend([
            rounded(miner.weak_shares_per_month),
            rounded(miner.payments_per_month),
            percent(miner.months_without_payment),
            optional(miner.swing, two_decimals),
            income_ratio(miner),
        ]);
    });
    cells
}

/// The headers of `outcome_cells`, after the `leading` ones.
fn outcome_headers(leading: &[&str]) -> Vec<String> {
    let mut headers: Vec<String> = leading.iter().map(|cell| cell.to_string()).collect();
    headers.extend(
        [
            "Weak a share",
            "R",
            "Asking",
            "Weak/s",
            "KB/s",
            "Smallest miner with 3 weak shares a month (TH/s)",
        ]
        .iter()
        .map(|cell| cell.to_string()),
    );
    ["Bitaxe", "ASIC"].iter().for_each(|name| {
        headers.extend(
            ["weak/mo", "pay/mo", "empty months", "swing", "vs fair"]
                .iter()
                .map(|column| format!("{name} {column}")),
        );
    });
    headers
}

/// A table of `rows`, each a scenario with its leading cells.
fn ask_table(leading: &[&str], rows: Vec<(Vec<String>, Scenario)>) -> String {
    let (labels, scenarios): (Vec<Vec<String>>, Vec<Scenario>) = rows.into_iter().unzip();
    let reports = run_scenarios(&scenarios);
    let rows: Vec<Vec<String>> = labels
        .into_iter()
        .zip(&reports)
        .map(|(mut cells, report)| {
            cells.extend(outcome_cells(report));
            cells
        })
        .collect();
    let headers = outcome_headers(leading);
    let headers: Vec<&str> = headers.iter().map(String::as_str).collect();
    markdown_table(&headers, &rows)
}

/// Where the asks settle: today's rule, the proposal and `WEAK_FILL` raised
/// to the proposal's ceiling, at 10 and 30% of Bitcoin Cash and 1 to 270
/// times today's hashrate, 50% industrial.
pub fn weak_asks(base: &Scenario) -> String {
    let rules = [
        WeakRule::Today,
        WeakRule::Asks(WeakAsks::default()),
        WeakRule::AtTheCeiling,
    ];
    let rows = ASK_HASHRATE_MULTIPLES
        .into_iter()
        .flat_map(|multiple| {
            ASK_POOL_FRACTIONS.into_iter().flat_map(move |fraction| {
                rules.into_iter().map(move |rule| {
                    (
                        vec![
                            format!("x{multiple}"),
                            pool_percent(fraction),
                            rule.name().to_string(),
                        ],
                        ruled_scenario(base, multiple, fraction, rule),
                    )
                })
            })
        })
        .collect();
    ask_table(&["BCH hashrate", "Pool", "Rule"], rows)
}

/// The proposal in every miner mix, beside today's rule, at ten times
/// today's hashrate.
pub fn weak_asks_mixes(base: &Scenario) -> String {
    let rules = [WeakRule::Today, WeakRule::Asks(WeakAsks::default())];
    let rows = ASK_MIXES
        .into_iter()
        .flat_map(|mix| {
            ASK_POOL_FRACTIONS.into_iter().flat_map(move |fraction| {
                rules.into_iter().map(move |rule| {
                    (
                        vec![
                            mix.name().to_string(),
                            pool_percent(fraction),
                            rule.name().to_string(),
                        ],
                        Scenario {
                            mix,
                            ..ruled_scenario(base, ASK_STUDY_MULTIPLE, fraction, rule)
                        },
                    )
                })
            })
        })
        .collect();
    ask_table(&["Mix", "Pool", "Rule"], rows)
}

/// The proposal's constants: `WEAK_ASK_DIVISOR`, the node default and
/// `WEAK_FILL_MAX`, each varied alone from the proposed values, in a pool at
/// 30% of a Bitcoin Cash ten times today's, in the 50% industrial and Zipf
/// mixes.
pub fn weak_asks_constants(base: &Scenario) -> String {
    let proposed = WeakAsks::default();
    let divisors = ASK_DIVISORS.into_iter().map(|ask_divisor| WeakAsks {
        rule: AskRule {
            ask_divisor,
            ..proposed.rule
        },
        ..proposed
    });
    let node_defaults = ASK_NODE_DEFAULTS
        .into_iter()
        .map(|weak_shares_per_month| WeakAsks {
            weak_shares_per_month,
            ..proposed
        });
    let fill_maxima = ASK_FILL_MAXIMA.into_iter().map(|weak_fill_max| WeakAsks {
        rule: AskRule {
            weak_fill_max,
            ..proposed.rule
        },
        ..proposed
    });
    let variants: Vec<WeakAsks> = divisors.chain(node_defaults).chain(fill_maxima).collect();
    let rows = [MinerMix::HalfIndustrial, MinerMix::Zipf]
        .into_iter()
        .flat_map(|mix| {
            variants.clone().into_iter().map(move |asks| {
                (
                    vec![
                        mix.name().to_string(),
                        asks.rule.ask_divisor.to_string(),
                        asks.weak_shares_per_month.to_string(),
                        asks.rule.weak_fill_max.to_string(),
                    ],
                    Scenario {
                        mix,
                        ..ruled_scenario(base, ASK_STUDY_MULTIPLE, 0.30, WeakRule::Asks(asks))
                    },
                )
            })
        })
        .collect();
    ask_table(
        &[
            "Mix",
            "WEAK_ASK_DIVISOR",
            "Node default (weak shares a month)",
            "WEAK_FILL_MAX",
        ],
        rows,
    )
}

/// The attacks on the asks. Hashrate that asks whatever its miners find, in
/// a pool that needs no more weak shares (10% of today's Bitcoin Cash).
/// Then, in a pool that does (30% of a Bitcoin Cash ten times today's):
/// hashrate that never asks, the industrial miners mining at the minimum
/// without asking, and nodes that carry no weak share that asks.
pub fn weak_asks_attacks(base: &Scenario) -> String {
    let proposed = WeakAsks::default();
    let asking = WeakRule::Asks(proposed);
    let needy = |rule: WeakRule| ruled_scenario(base, ASK_STUDY_MULTIPLE, 0.30, rule);
    let mut rows: Vec<(Vec<String>, Scenario)> = Vec::with_capacity(24);
    let mut push = |attack: &str, size: String, pool: &str, scenario: Scenario| {
        rows.push((vec![attack.to_string(), size, pool.to_string()], scenario));
    };
    ALWAYS_ASKING.into_iter().for_each(|always_asking| {
        push(
            "asks whatever it finds",
            percent(always_asking),
            "x1, 10%",
            ruled_scenario(
                base,
                1.0,
                0.10,
                WeakRule::Asks(WeakAsks {
                    always_asking,
                    ..proposed
                }),
            ),
        );
    });
    push(
        "none, today's rule",
        "-".to_string(),
        "x10, 30%",
        needy(WeakRule::Today),
    );
    push("none", "-".to_string(), "x10, 30%", needy(asking));
    NEVER_ASKING.into_iter().for_each(|never_asking| {
        push(
            "never asks",
            percent(never_asking),
            "x10, 30%",
            needy(WeakRule::Asks(WeakAsks {
                never_asking,
                ..proposed
            })),
        );
    });
    [WeakRule::Today, asking].into_iter().for_each(|rule| {
        push(
            "industrial miners at the minimum",
            rule.name().to_string(),
            "x10, 30%",
            Scenario {
                big_node_at_minimum: true,
                ..needy(rule)
            },
        );
    });
    CENSORING_SHARES.into_iter().for_each(|censoring_shares| {
        push(
            "carries no asking weak share",
            percent(censoring_shares),
            "x10, 30%",
            needy(WeakRule::Asks(WeakAsks {
                censoring_shares,
                ..proposed
            })),
        );
    });
    ask_table(&["Attack", "Its part", "BCH hashrate, pool"], rows)
}

#[cfg(test)]
mod tests;
