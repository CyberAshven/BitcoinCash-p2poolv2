// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The named tables of the default run: which scenarios each runs and how
//! it prints them.

use crate::constants::{HASHES_PER_DIFFICULTY, SHARE_INTERVAL_SECONDS};
use crate::levels::level_amount;
use crate::miners::{MinerMix, TrackedMiner};
use crate::report::{
    both_miner_cells, both_miner_headers, expected_ratio, group_thousands, hours, income_ratio,
    markdown_table, miner_cells, miner_headers, optional, percent, pool_percent, rounded, satoshis,
    two_decimals,
};
use crate::scenario::{Design, Report, Scenario, WeakCredit, stale_rate_for_period};
use crate::study;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;

/// The pool sizes of the design comparison, as fractions of Bitcoin Cash.
pub const POOL_FRACTIONS: [f64; 5] = [0.01, 0.05, 0.10, 0.20, 0.30];

/// The three pool sizes most tables use.
const KEY_POOL_FRACTIONS: [f64; 3] = [0.01, 0.10, 0.30];

/// Bitcoin Cash hashrate multiples of the hashrate table.
const HASHRATE_MULTIPLES: [f64; 4] = [1.0, 3.0, 10.0, 270.0];

/// Band caps of the band cap sweep.
const BAND_CAPS: [f64; 5] = [1.0, 8.0, 30.0, 64.0, 256.0];

/// `(WEAK_CAP, WEAK_FILL)` pairs of the weak constants sweep.
const WEAK_CONSTANTS: [(u64, u64); 6] =
    [(16, 8), (32, 16), (64, 32), (128, 64), (64, 16), (64, 48)];

/// `INCLUDER_CUT` values of its sweep.
const INCLUDER_CUTS: [f64; 4] = [0.0, 0.01, 0.02, 0.05];

/// Payout levels of the level sweep; levels 0 to 2 pay as level 3 does in
/// every pool size tested, so they are left out.
const LEVELS_SWEPT: [u32; 5] = [3, 4, 5, 6, 7];

/// Share periods of the period table, seconds.
const PERIODS: [f64; 4] = [10.0, 5.0, 2.0, 1.0];

/// Non-carrying fractions of attack (b).
const NON_CARRYING_FRACTIONS: [f64; 3] = [0.0, 0.5, 0.9];

/// Spam multiples of attack (c).
const SPAM_MULTIPLES: [f64; 3] = [0.0, 1.0, 10.0];

/// The designs of the comparison tables: every design as the page writes
/// it, and the band with weak shares and levels with shares on minimum jobs
/// credited at the weak work.
const VARIANTS: [(Design, WeakCredit); 7] = [
    (Design::Mini, WeakCredit::Page),
    (Design::OneChain, WeakCredit::Page),
    (Design::Band, WeakCredit::Page),
    (Design::Vardiff, WeakCredit::Page),
    (Design::BandWeak, WeakCredit::Page),
    (Design::BandWeakLevels, WeakCredit::Page),
    (Design::BandWeakLevels, WeakCredit::WeakWork),
];

/// A design's name in tables, marked when shares on minimum jobs are
/// credited at the weak work.
pub(crate) fn variant_name(scenario: &Scenario) -> String {
    if scenario.design.has_weak_shares() && scenario.weak_credit == WeakCredit::WeakWork {
        format!("{}, weak work", scenario.design.name())
    } else {
        scenario.design.name().to_string()
    }
}

/// One table of the default run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Table {
    /// Every design at every pool size, one table per miner mix.
    Designs,
    /// Every design as Bitcoin Cash's hashrate grows.
    Hashrate,
    /// The pool's shares, uncles, weak shares and bytes per design.
    Pool,
    /// The band cap `C`.
    BandCap,
    /// `WEAK_CAP` and `WEAK_FILL`.
    WeakConstants,
    /// `INCLUDER_CUT`.
    IncluderCut,
    /// The default payout level.
    Levels,
    /// The share period.
    Period,
    /// Attack (a): a big node mining at the minimum under the band.
    AttackMinimum,
    /// Attack (b): nodes that never carry others' weak shares.
    AttackCarriers,
    /// Attack (c): weak share spam.
    AttackSpam,
    /// How a share on a minimum job is credited with weak shares.
    WeakCredit,
    /// Study #85: a refusing coalition with share races and weak work in
    /// fork choice.
    RefusingRaces,
    /// Study #85: Zipf and medium miner mixes.
    Mixes,
    /// Study #85: a big miner joining or leaving at once.
    Arrivals,
    /// Study #85: a tiny pool at the ceiling.
    TinyPool,
    /// Study #85: large miners filling the weak slots.
    Floods,
    /// Study #85: a miner steering the level draw.
    DrawSteering,
    /// Study #85: weak shares lost to stale parents and uncle carriers.
    Losses,
    /// Study #85 constants: `WEAK_DEPTH` against a refusing coalition.
    WeakDepth,
    /// Study #85 constants: `R_MAX` and the start from genesis.
    RatioMax,
    /// Study #85 constants: `R_MAX` in every miner mix.
    RatioMaxMixes,
    /// Bigger pools on a bigger Bitcoin Cash, levels as a node sets them.
    Scale,
}

impl Table {
    /// Every table, in the order of the default run.
    pub const ALL: [Table; 23] = [
        Table::Designs,
        Table::Hashrate,
        Table::Pool,
        Table::WeakCredit,
        Table::BandCap,
        Table::WeakConstants,
        Table::IncluderCut,
        Table::Levels,
        Table::Period,
        Table::AttackMinimum,
        Table::AttackCarriers,
        Table::AttackSpam,
        Table::RefusingRaces,
        Table::Mixes,
        Table::Arrivals,
        Table::TinyPool,
        Table::Floods,
        Table::DrawSteering,
        Table::Losses,
        Table::WeakDepth,
        Table::RatioMax,
        Table::RatioMaxMixes,
        Table::Scale,
    ];

    /// Name on the command line.
    pub fn name(&self) -> &'static str {
        match self {
            Table::Designs => "designs",
            Table::Hashrate => "hashrate",
            Table::Pool => "pool",
            Table::BandCap => "band-cap",
            Table::WeakConstants => "weak-constants",
            Table::IncluderCut => "includer-cut",
            Table::Levels => "levels",
            Table::Period => "period",
            Table::AttackMinimum => "attack-minimum",
            Table::AttackCarriers => "attack-carriers",
            Table::AttackSpam => "attack-spam",
            Table::WeakCredit => "weak-credit",
            Table::RefusingRaces => "refusing-races",
            Table::Mixes => "mixes",
            Table::Arrivals => "arrivals",
            Table::TinyPool => "tiny-pool",
            Table::Floods => "floods",
            Table::DrawSteering => "draw-steering",
            Table::Losses => "losses",
            Table::WeakDepth => "weak-depth",
            Table::RatioMax => "r-max",
            Table::RatioMaxMixes => "r-max-mixes",
            Table::Scale => "scale",
        }
    }

    /// The table with this name.
    pub fn parse(name: &str) -> Result<Self, String> {
        Table::ALL
            .into_iter()
            .find(|table| table.name() == name)
            .ok_or_else(|| {
                let names: Vec<&str> = Table::ALL.iter().map(Table::name).collect();
                format!("unknown table `{name}`; one of: all, {}", names.join(", "))
            })
    }

    /// Heading printed above the table.
    pub fn title(&self) -> &'static str {
        match self {
            Table::Designs => "Designs at each pool size",
            Table::Hashrate => {
                "Designs as Bitcoin Cash's hashrate grows (pool 10%, 50% industrial)"
            }
            Table::Pool => "Pool load (50% industrial)",
            Table::BandCap => "Band cap C (design band)",
            Table::WeakConstants => "WEAK_CAP and WEAK_FILL (band-weak-levels, 50% industrial)",
            Table::IncluderCut => "INCLUDER_CUT (band-weak-levels)",
            Table::Levels => "Default payout level (band-weak-levels, 50% industrial)",
            Table::Period => "Share period (pool 10%, 50% industrial)",
            Table::AttackMinimum => {
                "Attack (a): the industrial node mines at the minimum (90% industrial)"
            }
            Table::AttackCarriers => {
                "Attack (b): nodes that never carry others' weak shares (band-weak-levels)"
            }
            Table::AttackSpam => "Attack (c): weak share spam (band-weak-levels)",
            Table::WeakCredit => {
                "Weak share credit: as the page writes it and credited once (band-weak-levels)"
            }
            Table::RefusingRaces => {
                "Study: a refusing coalition with share races (band-weak-levels, weak work, 50% industrial)"
            }
            Table::Mixes => "Study: miner mixes (INCLUDER_CUT 1%)",
            Table::Arrivals => "Study: a big miner joining or leaving at once (50% industrial)",
            Table::TinyPool => {
                "Study: a tiny pool at the ceiling (1 GH/s on a 37 GH/s test network)"
            }
            Table::Floods => {
                "Study: large miners filling the weak slots (band-weak-levels, weak work)"
            }
            Table::DrawSteering => {
                "Study: steering the level draw (band-weak-levels, weak work, 50% industrial)"
            }
            Table::Losses => {
                "Study: weak shares lost to stale parents and uncle carriers (band-weak-levels, weak work, 50% industrial)"
            }
            Table::WeakDepth => {
                "Study constants: WEAK_DEPTH and INCLUDER_CUT against a refusing coalition (band-weak-levels, weak work, 50% industrial)"
            }
            Table::RatioMax => "Study constants: R_MAX and the start from genesis (50% industrial)",
            Table::RatioMaxMixes => {
                "Study constants: R_MAX 256 and 4,096 in every miner mix (band-weak-levels, weak work)"
            }
            Table::Scale => {
                "Scale: bigger pools on a bigger Bitcoin Cash, levels as a node sets them (band-weak-levels, weak work, 50% industrial)"
            }
        }
    }

    /// Run the table's scenarios from `base` (its months and seed) and
    /// print it in Markdown.
    pub fn render(&self, base: &Scenario) -> String {
        let body = match self {
            Table::Designs => designs(base),
            Table::Hashrate => hashrate(base),
            Table::Pool => pool(base),
            Table::BandCap => band_cap(base),
            Table::WeakConstants => weak_constants(base),
            Table::IncluderCut => includer_cut(base),
            Table::Levels => levels(base),
            Table::Period => period(base),
            Table::AttackMinimum => attack_minimum(base),
            Table::AttackCarriers => attack_carriers(base),
            Table::AttackSpam => attack_spam(base),
            Table::WeakCredit => weak_credit(base),
            Table::RefusingRaces => study::refusing_races(base),
            Table::Mixes => study::mixes(base),
            Table::Arrivals => study::arrivals(base),
            Table::TinyPool => study::tiny_pool(base),
            Table::Floods => study::floods(base),
            Table::DrawSteering => study::draw_steering(base),
            Table::Losses => study::losses(base),
            Table::WeakDepth => study::weak_depth(base),
            Table::RatioMax => study::ratio_max(base),
            Table::RatioMaxMixes => study::ratio_max_mixes(base),
            Table::Scale => study::scale(base),
        };
        format!("## {}\n\n{body}", self.title())
    }
}

/// Run `scenarios` on every core, returning the reports in order. Each run
/// depends only on its own scenario, so the result does not depend on the
/// number of threads.
pub fn run_scenarios(scenarios: &[Scenario]) -> Vec<Report> {
    parallel_map(scenarios, Scenario::run)
}

/// `work` applied to every item on every core, the results in the items'
/// order. Each result must depend only on its own item, so the outcome does
/// not depend on the number of threads.
pub fn parallel_map<Item, Output>(
    items: &[Item],
    work: impl Fn(&Item) -> Output + Sync,
) -> Vec<Output>
where
    Item: Sync,
    Output: Send,
{
    let threads = thread::available_parallelism()
        .map(NonZeroUsize::get)
        .unwrap_or(1)
        .min(items.len().max(1));
    let next = AtomicUsize::new(0);
    let (sender, receiver) = mpsc::channel();
    thread::scope(|scope| {
        (0..threads).for_each(|_| {
            let sender = sender.clone();
            let next = &next;
            let work = &work;
            scope.spawn(move || {
                let mut index = next.fetch_add(1, Ordering::Relaxed);
                while index < items.len() {
                    // The receiver outlives the scope, so a send cannot fail.
                    let _ = sender.send((index, work(&items[index])));
                    index = next.fetch_add(1, Ordering::Relaxed);
                }
            });
        });
    });
    drop(sender);
    let mut slots: Vec<Option<Output>> = Vec::with_capacity(items.len());
    slots.resize_with(items.len(), || None);
    receiver.into_iter().for_each(|(index, output)| {
        slots[index] = Some(output);
    });
    slots
        .into_iter()
        .map(|slot| slot.expect("every item ran"))
        .collect()
}

/// Header cells: `leading` then both tracked miners' cells.
fn headers_with_miners(leading: &[&str]) -> Vec<String> {
    let mut headers: Vec<String> = leading.iter().map(|cell| cell.to_string()).collect();
    headers.extend(both_miner_headers());
    headers
}

pub(crate) fn table_from(headers: &[String], rows: &[Vec<String>]) -> String {
    let header_cells: Vec<&str> = headers.iter().map(String::as_str).collect();
    markdown_table(&header_cells, rows)
}

/// The weak ratio `R` a report's first chain settled at, or a dash.
pub(crate) fn weak_ratio(report: &Report) -> String {
    optional(
        report.home_chain().weak.map(|outcome| outcome.mean_ratio),
        rounded,
    )
}

/// The uniform share difficulty of a pool of `fraction` of Bitcoin Cash.
pub(crate) fn uniform_difficulty(report: &Report) -> f64 {
    let scenario = &report.scenario;
    scenario.network_hashrate()
        * report.pool_fraction
        * (1.0 - scenario.stale_rate)
        * scenario.period_seconds
        / HASHES_PER_DIFFICULTY
}

fn designs(base: &Scenario) -> String {
    let mut sections = Vec::with_capacity(MinerMix::ALL.len());
    MinerMix::ALL.iter().for_each(|mix| {
        let scenarios: Vec<Scenario> = VARIANTS
            .iter()
            .flat_map(|(design, credit)| {
                POOL_FRACTIONS.iter().map(move |fraction| Scenario {
                    design: *design,
                    weak_credit: *credit,
                    mix: *mix,
                    pool_fraction: *fraction,
                    ..base.clone()
                })
            })
            .collect();
        let reports = run_scenarios(&scenarios);
        let rows: Vec<Vec<String>> = reports
            .iter()
            .map(|report| {
                let mut row = vec![
                    variant_name(&report.scenario),
                    pool_percent(report.scenario.pool_fraction),
                ];
                row.extend(both_miner_cells(report));
                row
            })
            .collect();
        let headers = headers_with_miners(&["Design", "Pool"]);
        sections.push(format!(
            "### Mix: {}\n\n{}",
            mix.name(),
            table_from(&headers, &rows)
        ));
    });
    sections.join("\n")
}

fn hashrate(base: &Scenario) -> String {
    let scenarios: Vec<Scenario> = VARIANTS
        .iter()
        .flat_map(|(design, credit)| {
            HASHRATE_MULTIPLES.iter().map(move |multiple| Scenario {
                design: *design,
                weak_credit: *credit,
                pool_fraction: 0.10,
                hashrate_multiple: *multiple,
                ..base.clone()
            })
        })
        .collect();
    let reports = run_scenarios(&scenarios);
    let rows: Vec<Vec<String>> = reports
        .iter()
        .map(|report| {
            let mut row = vec![
                variant_name(&report.scenario),
                format!("x{}", report.scenario.hashrate_multiple),
                satoshis(report.miners[0].fair_income_per_month),
            ];
            row.extend(both_miner_cells(report));
            row
        })
        .collect();
    table_from(
        &headers_with_miners(&["Design", "BCH hashrate", "Bitaxe fair sat/mo"]),
        &rows,
    )
}

fn pool(base: &Scenario) -> String {
    let scenarios: Vec<Scenario> = VARIANTS
        .iter()
        .flat_map(|(design, credit)| {
            POOL_FRACTIONS.iter().map(move |fraction| Scenario {
                design: *design,
                weak_credit: *credit,
                pool_fraction: *fraction,
                ..base.clone()
            })
        })
        .collect();
    let reports = run_scenarios(&scenarios);
    let rows: Vec<Vec<String>> = reports
        .iter()
        .map(|report| {
            let chain = report.home_chain();
            vec![
                variant_name(&report.scenario),
                pool_percent(report.scenario.pool_fraction),
                rounded(chain.asert_difficulty),
                rounded(chain.easiest_difficulty),
                optional(chain.settled_band_cap, rounded),
                weak_ratio(report),
                optional(
                    chain.weak.map(|outcome| outcome.arrivals_per_share),
                    rounded,
                ),
                optional(
                    chain.weak.map(|outcome| outcome.carrying_expired_fraction),
                    percent,
                ),
                hours(chain.window.span_seconds),
                rounded(report.shares_per_second()),
                rounded(report.uncles_per_second()),
                rounded(report.weak_shares_per_second()),
                rounded(report.bytes_per_second() / 1_000.0),
            ]
        })
        .collect();
    let headers = [
        "Design",
        "Pool",
        "ASERT difficulty",
        "Easiest share",
        "C settles at",
        "R",
        "Weak per share",
        "Weak expired",
        "Window (Bitaxe's chain)",
        "Shares/s",
        "Uncles/s",
        "Weak/s",
        "KB/s",
    ];
    markdown_table(&headers, &rows)
}

fn band_cap(base: &Scenario) -> String {
    let mixes = [MinerMix::HalfIndustrial, MinerMix::MostlyIndustrial];
    let scenarios: Vec<Scenario> = mixes
        .iter()
        .flat_map(|mix| {
            KEY_POOL_FRACTIONS.iter().flat_map(move |fraction| {
                BAND_CAPS.iter().map(move |cap| Scenario {
                    design: Design::Band,
                    mix: *mix,
                    pool_fraction: *fraction,
                    band_cap: *cap,
                    ..base.clone()
                })
            })
        })
        .collect();
    let reports = run_scenarios(&scenarios);
    let rows: Vec<Vec<String>> = reports
        .iter()
        .map(|report| {
            let chain = report.home_chain();
            let bitaxe = &report.miners[0];
            let asic = &report.miners[1];
            vec![
                report.scenario.mix.name().to_string(),
                pool_percent(report.scenario.pool_fraction),
                rounded(report.scenario.band_cap),
                optional(chain.settled_band_cap, rounded),
                two_decimals(chain.asert_difficulty / uniform_difficulty(report)),
                rounded(asic.difficulty_multiple),
                rounded(bitaxe.shares_per_month),
                rounded(bitaxe.payments_per_month),
                optional(bitaxe.swing, two_decimals),
                percent(bitaxe.months_without_payment),
                rounded(asic.payments_per_month),
                optional(asic.swing, two_decimals),
            ]
        })
        .collect();
    let headers = [
        "Mix",
        "Pool",
        "C",
        "C settles at",
        "Minimum / uniform",
        "ASIC share multiple",
        "Bitaxe shares/mo",
        "Bitaxe pay/mo",
        "Bitaxe swing",
        "Bitaxe empty months",
        "ASIC pay/mo",
        "ASIC swing",
    ];
    markdown_table(&headers, &rows)
}

fn weak_constants(base: &Scenario) -> String {
    let scenarios: Vec<Scenario> = [WeakCredit::Page, WeakCredit::WeakWork]
        .into_iter()
        .flat_map(|credit| {
            KEY_POOL_FRACTIONS.iter().flat_map(move |fraction| {
                WEAK_CONSTANTS.iter().map(move |(cap, fill)| Scenario {
                    design: Design::BandWeakLevels,
                    weak_credit: credit,
                    pool_fraction: *fraction,
                    weak_cap: *cap,
                    weak_fill: *fill,
                    ..base.clone()
                })
            })
        })
        .collect();
    let reports = run_scenarios(&scenarios);
    let rows: Vec<Vec<String>> = reports
        .iter()
        .map(|report| {
            let weak = report.home_chain().weak.unwrap_or_default();
            let bitaxe = &report.miners[0];
            vec![
                credit_name(report.scenario.weak_credit).to_string(),
                pool_percent(report.scenario.pool_fraction),
                report.scenario.weak_cap.to_string(),
                report.scenario.weak_fill.to_string(),
                rounded(weak.mean_ratio),
                rounded(weak.arrivals_per_share),
                rounded(weak.carried_per_share),
                percent(weak.full_share_fraction),
                percent(weak.carrying_expired_fraction),
                rounded(report.bytes_per_second() / 1_000.0),
                rounded(bitaxe.weak_shares_per_month),
                rounded(bitaxe.payments_per_month),
                optional(bitaxe.swing, two_decimals),
                percent(bitaxe.months_without_payment),
            ]
        })
        .collect();
    let headers = [
        "Credit",
        "Pool",
        "WEAK_CAP",
        "WEAK_FILL",
        "R",
        "Weak found per share",
        "Carried per share",
        "Shares at the cap",
        "Weak expired",
        "KB/s",
        "Bitaxe weak/mo",
        "Bitaxe pay/mo",
        "Bitaxe swing",
        "Bitaxe empty months",
    ];
    markdown_table(&headers, &rows)
}

/// The tracked miners of the carrying-node tables: the Bitaxe, the ASIC on a
/// carrying node and the ASIC on a node that never carries others' weak
/// shares.
pub(crate) fn tracked_with_non_carrying_asic() -> Vec<TrackedMiner> {
    vec![
        TrackedMiner::bitaxe(),
        TrackedMiner::asic(),
        TrackedMiner::non_carrying_asic(),
    ]
}

/// Name of a weak credit rule in tables.
fn credit_name(credit: WeakCredit) -> &'static str {
    match credit {
        WeakCredit::Page => "page",
        WeakCredit::WeakWork => "weak work",
    }
}

/// Each row runs two scenarios: every node carrying, and half the hashrate
/// on non-carrying nodes. Dropping others' weak shares pays a coalition when
/// its ASIC earns more in the second than a carrying ASIC in the first.
fn includer_cut(base: &Scenario) -> String {
    let scenarios: Vec<Scenario> = [0.01, 0.10]
        .into_iter()
        .flat_map(|fraction| {
            [WeakCredit::Page, WeakCredit::WeakWork]
                .into_iter()
                .flat_map(move |credit| {
                    INCLUDER_CUTS.iter().flat_map(move |cut| {
                        [0.0, 0.5].into_iter().map(move |non_carrying| Scenario {
                            design: Design::BandWeakLevels,
                            pool_fraction: fraction,
                            weak_credit: credit,
                            includer_cut: *cut,
                            non_carrying_fraction: non_carrying,
                            tracked: tracked_with_non_carrying_asic(),
                            ..base.clone()
                        })
                    })
                })
        })
        .collect();
    let reports = run_scenarios(&scenarios);
    let rows: Vec<Vec<String>> = reports
        .chunks(2)
        .map(|pair| {
            let all_carry = &pair[0];
            let coalition = &pair[1];
            vec![
                pool_percent(all_carry.scenario.pool_fraction),
                credit_name(all_carry.scenario.weak_credit).to_string(),
                percent(all_carry.scenario.includer_cut),
                expected_ratio(&all_carry.miners[0]),
                percent(all_carry.miners[0].bonus_fraction),
                expected_ratio(&all_carry.miners[1]),
                percent(all_carry.miners[1].bonus_fraction),
                expected_ratio(&all_carry.miners[2]),
                percent(
                    coalition
                        .home_chain()
                        .weak
                        .unwrap_or_default()
                        .carrying_expired_fraction,
                ),
                expected_ratio(&coalition.miners[0]),
                expected_ratio(&coalition.miners[2]),
            ]
        })
        .collect();
    let headers = [
        "Pool",
        "Credit",
        "INCLUDER_CUT",
        "Bitaxe vs fair",
        "Bitaxe bonus share",
        "Carrying ASIC vs fair",
        "Carrying ASIC bonus share",
        "Lone non-carrying ASIC vs fair",
        "Honest weak expired, 50% non-carrying",
        "Bitaxe vs fair, 50% non-carrying",
        "Non-carrying ASIC vs fair, 50% non-carrying",
    ];
    markdown_table(&headers, &rows)
}

fn levels(base: &Scenario) -> String {
    let scenarios: Vec<Scenario> = [WeakCredit::Page, WeakCredit::WeakWork]
        .into_iter()
        .flat_map(|credit| {
            KEY_POOL_FRACTIONS.iter().flat_map(move |fraction| {
                let without = Scenario {
                    design: Design::BandWeak,
                    weak_credit: credit,
                    pool_fraction: *fraction,
                    ..base.clone()
                };
                let with_levels = LEVELS_SWEPT.iter().map(move |level| {
                    let mut scenario = Scenario {
                        design: Design::BandWeakLevels,
                        weak_credit: credit,
                        pool_fraction: *fraction,
                        default_level: *level,
                        ..base.clone()
                    };
                    scenario
                        .tracked
                        .iter_mut()
                        .for_each(|miner| miner.level = *level);
                    scenario
                });
                std::iter::once(without).chain(with_levels)
            })
        })
        .collect();
    let reports = run_scenarios(&scenarios);
    let rows: Vec<Vec<String>> = reports
        .iter()
        .map(|report| {
            let level = if report.scenario.design.has_levels() {
                let level = report.scenario.default_level;
                format!("{level} ({} sat)", group_thousands(level_amount(level)))
            } else {
                "none".to_string()
            };
            let bitaxe = &report.miners[0];
            let asic = &report.miners[1];
            vec![
                credit_name(report.scenario.weak_credit).to_string(),
                pool_percent(report.scenario.pool_fraction),
                level,
                rounded(bitaxe.payments_per_month),
                optional(bitaxe.median_payment, satoshis),
                optional(bitaxe.swing, two_decimals),
                percent(bitaxe.months_without_payment),
                optional(bitaxe.fee_loss, percent),
                rounded(asic.payments_per_month),
                optional(asic.median_payment, satoshis),
                optional(asic.fee_loss, percent),
            ]
        })
        .collect();
    let headers = [
        "Credit",
        "Pool",
        "Level",
        "Bitaxe pay/mo",
        "Bitaxe median sat",
        "Bitaxe swing",
        "Bitaxe empty months",
        "Bitaxe fee loss",
        "ASIC pay/mo",
        "ASIC median sat",
        "ASIC fee loss",
    ];
    markdown_table(&headers, &rows)
}

fn period(base: &Scenario) -> String {
    let designs = [
        (Design::OneChain, WeakCredit::Page),
        (Design::BandWeakLevels, WeakCredit::Page),
        (Design::BandWeakLevels, WeakCredit::WeakWork),
    ];
    let scenarios: Vec<Scenario> = designs
        .iter()
        .flat_map(|(design, credit)| {
            PERIODS.iter().map(move |seconds| Scenario {
                design: *design,
                weak_credit: *credit,
                pool_fraction: 0.10,
                period_seconds: *seconds,
                stale_rate: stale_rate_for_period(*seconds),
                ..base.clone()
            })
        })
        .collect();
    let reports = run_scenarios(&scenarios);
    let rows: Vec<Vec<String>> = reports
        .iter()
        .map(|report| {
            let chain = report.home_chain();
            let mut row = vec![
                variant_name(&report.scenario),
                format!("{} s", report.scenario.period_seconds),
                percent(report.scenario.stale_rate),
                hours(chain.window.span_seconds),
                rounded(report.shares_per_second()),
                rounded(report.uncles_per_second()),
                rounded(report.weak_shares_per_second()),
                rounded(report.bytes_per_second() / 1_000.0),
            ];
            row.extend(miner_cells(&report.miners[0]));
            row
        })
        .collect();
    let mut headers: Vec<String> = [
        "Design", "Period", "Stale", "Window", "Shares/s", "Uncles/s", "Weak/s", "KB/s",
    ]
    .iter()
    .map(|cell| cell.to_string())
    .collect();
    headers.extend(miner_headers("Bitaxe"));
    table_from(&headers, &rows)
}

/// The weak credit rules a design is run under: both with weak shares, the
/// page's alone otherwise (the rule only matters with weak shares).
fn credits_for(design: Design) -> Vec<WeakCredit> {
    if design.has_weak_shares() {
        vec![WeakCredit::Page, WeakCredit::WeakWork]
    } else {
        vec![WeakCredit::Page]
    }
}

fn attack_minimum(base: &Scenario) -> String {
    let designs = [Design::OneChain, Design::Band, Design::BandWeakLevels];
    let scenarios: Vec<Scenario> = designs
        .iter()
        .flat_map(|design| {
            credits_for(*design).into_iter().flat_map(move |credit| {
                KEY_POOL_FRACTIONS.iter().flat_map(move |fraction| {
                    [false, true].into_iter().map(move |attack| Scenario {
                        design: *design,
                        weak_credit: credit,
                        mix: MinerMix::MostlyIndustrial,
                        pool_fraction: *fraction,
                        big_node_at_minimum: attack,
                        ..base.clone()
                    })
                })
            })
        })
        .filter(|scenario| scenario.design.has_band() || !scenario.big_node_at_minimum)
        .collect();
    let reports = run_scenarios(&scenarios);
    let rows: Vec<Vec<String>> = reports
        .iter()
        .map(|report| {
            let behaviour = if report.scenario.big_node_at_minimum {
                "at the minimum"
            } else {
                "node default"
            };
            let mut row = vec![
                report.scenario.design.name().to_string(),
                credit_name(report.scenario.weak_credit).to_string(),
                pool_percent(report.scenario.pool_fraction),
                behaviour.to_string(),
                two_decimals(report.home_chain().asert_difficulty / uniform_difficulty(report)),
                weak_ratio(report),
            ];
            row.extend(both_miner_cells(report));
            row
        })
        .collect();
    table_from(
        &headers_with_miners(&[
            "Design",
            "Credit",
            "Pool",
            "Industrial node",
            "Minimum / uniform",
            "R",
        ]),
        &rows,
    )
}

fn attack_carriers(base: &Scenario) -> String {
    let mixes = [MinerMix::HalfIndustrial, MinerMix::MostlyIndustrial];
    let scenarios: Vec<Scenario> = mixes
        .iter()
        .flat_map(|mix| {
            [0.01, 0.10].into_iter().flat_map(move |fraction| {
                [WeakCredit::Page, WeakCredit::WeakWork]
                    .into_iter()
                    .flat_map(move |credit| {
                        NON_CARRYING_FRACTIONS
                            .iter()
                            .map(move |non_carrying| Scenario {
                                design: Design::BandWeakLevels,
                                mix: *mix,
                                pool_fraction: fraction,
                                weak_credit: credit,
                                non_carrying_fraction: *non_carrying,
                                tracked: tracked_with_non_carrying_asic(),
                                ..base.clone()
                            })
                    })
            })
        })
        .collect();
    let reports = run_scenarios(&scenarios);
    let rows: Vec<Vec<String>> = reports
        .iter()
        .map(|report| {
            let weak = report.home_chain().weak.unwrap_or_default();
            let bitaxe = &report.miners[0];
            vec![
                report.scenario.mix.name().to_string(),
                pool_percent(report.scenario.pool_fraction),
                credit_name(report.scenario.weak_credit).to_string(),
                percent(report.scenario.non_carrying_fraction),
                rounded(weak.mean_ratio),
                percent(weak.carrying_expired_fraction),
                rounded(bitaxe.weak_shares_per_month),
                rounded(bitaxe.payments_per_month),
                optional(bitaxe.swing, two_decimals),
                percent(bitaxe.months_without_payment),
                expected_ratio(bitaxe),
                expected_ratio(&report.miners[1]),
                expected_ratio(&report.miners[2]),
            ]
        })
        .collect();
    let headers = [
        "Mix",
        "Pool",
        "Credit",
        "Non-carrying",
        "R",
        "Honest weak expired",
        "Bitaxe weak/mo",
        "Bitaxe pay/mo",
        "Bitaxe swing",
        "Bitaxe empty months",
        "Bitaxe vs fair",
        "Carrying ASIC vs fair",
        "Non-carrying ASIC vs fair",
    ];
    markdown_table(&headers, &rows)
}

fn attack_spam(base: &Scenario) -> String {
    let cases = [
        (MinerMix::HalfIndustrial, 0.01),
        (MinerMix::MostlyIndustrial, 0.01),
        (MinerMix::MostlyIndustrial, 0.10),
    ];
    let scenarios: Vec<Scenario> = cases
        .iter()
        .flat_map(|(mix, fraction)| {
            [WeakCredit::Page, WeakCredit::WeakWork]
                .into_iter()
                .flat_map(move |credit| {
                    SPAM_MULTIPLES.iter().map(move |multiple| Scenario {
                        design: Design::BandWeakLevels,
                        mix: *mix,
                        pool_fraction: *fraction,
                        weak_credit: credit,
                        spam_multiple: *multiple,
                        ..base.clone()
                    })
                })
        })
        .collect();
    let reports = run_scenarios(&scenarios);
    let rows: Vec<Vec<String>> = reports
        .iter()
        .map(|report| {
            let weak = report.home_chain().weak.unwrap_or_default();
            let bitaxe = &report.miners[0];
            vec![
                report.scenario.mix.name().to_string(),
                pool_percent(report.scenario.pool_fraction),
                format!("{}x", report.scenario.spam_multiple),
                pool_percent(report.pool_fraction),
                credit_name(report.scenario.weak_credit).to_string(),
                rounded(weak.mean_ratio),
                rounded(bitaxe.weak_shares_per_month),
                rounded(bitaxe.payments_per_month),
                optional(bitaxe.median_payment, satoshis),
                optional(bitaxe.swing, two_decimals),
                percent(bitaxe.months_without_payment),
                expected_ratio(bitaxe),
                income_ratio(bitaxe),
            ]
        })
        .collect();
    let headers = [
        "Mix",
        "Pool before spam",
        "Spam",
        "Pool with spam",
        "Credit",
        "R",
        "Bitaxe weak/mo",
        "Bitaxe pay/mo",
        "Bitaxe median sat",
        "Bitaxe swing",
        "Bitaxe empty months",
        "Bitaxe expected vs fair",
        "Bitaxe simulated vs fair",
    ];
    markdown_table(&headers, &rows)
}

fn weak_credit(base: &Scenario) -> String {
    let scenarios: Vec<Scenario> = MinerMix::ALL
        .iter()
        .flat_map(|mix| {
            KEY_POOL_FRACTIONS.iter().flat_map(move |fraction| {
                [WeakCredit::Page, WeakCredit::WeakWork]
                    .into_iter()
                    .map(move |credit| Scenario {
                        design: Design::BandWeakLevels,
                        mix: *mix,
                        pool_fraction: *fraction,
                        weak_credit: credit,
                        ..base.clone()
                    })
            })
        })
        .collect();
    let reports = run_scenarios(&scenarios);
    let rows: Vec<Vec<String>> = reports
        .iter()
        .map(|report| {
            let bitaxe = &report.miners[0];
            let asic = &report.miners[1];
            vec![
                report.scenario.mix.name().to_string(),
                pool_percent(report.scenario.pool_fraction),
                credit_name(report.scenario.weak_credit).to_string(),
                hours(report.home_chain().window.span_seconds),
                expected_ratio(bitaxe),
                income_ratio(bitaxe),
                optional(bitaxe.swing, two_decimals),
                rounded(asic.difficulty_multiple),
                expected_ratio(asic),
                income_ratio(asic),
            ]
        })
        .collect();
    let headers = [
        "Mix",
        "Pool",
        "Credit",
        "Window",
        "Bitaxe expected vs fair",
        "Bitaxe simulated vs fair",
        "Bitaxe swing",
        "ASIC share multiple",
        "ASIC expected vs fair",
        "ASIC simulated vs fair",
    ];
    markdown_table(&headers, &rows)
}

/// The base scenario of the default run: every table varies it.
pub fn base_scenario(months: usize, seed: u64) -> Scenario {
    Scenario {
        months,
        seed,
        period_seconds: SHARE_INTERVAL_SECONDS,
        ..Scenario::default()
    }
}

#[cfg(test)]
mod tests;
