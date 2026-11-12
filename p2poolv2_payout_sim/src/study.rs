// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The tables of study #85, "Study: weak work in fork choice, with a
//! smaller includer cut" (`docs/sharechain/small-miners-design.md` on
//! `docs/weak-work-fork-choice`).
//!
//! Every study table runs the design as the page now has it unless it says
//! otherwise: the band, weak shares and payout levels, every proof on a job
//! at the minimum credited at the weak work, and `INCLUDER_CUT` at 5%.

use crate::constants::{
    ASIC_HASHRATE, BCH_BLOCKS_PER_MONTH, BCH_NETWORK_HASHRATE, BLOCK_REWARD_SATOSHIS,
    DEFAULT_BAND_CAP, DEFAULT_LEVEL, HOME_MINER_HASHRATE, MINI_CEILING_DIFFICULTY,
    PAGE_INCLUDER_CUT, WEAK_SHARE_MESSAGE_BYTES,
};
use crate::levels::level_amount;
use crate::miners::{MinerClass, MinerKind, MinerMix, TrackedMiner};
use crate::races::ForkChoice;
use crate::report::{
    both_miner_cells, both_miner_headers, expected_ratio, income_ratio, markdown_table,
    miner_cells, miner_headers, optional, percent, pool_percent, rounded, satoshis, two_decimals,
};
use crate::scenario::{
    Design, DrawSteering, NON_CARRYING_SUFFIX, Report, Scenario, ShareRaces, SteeringMode,
    WeakCredit, stale_rate_for_period,
};
use crate::tables::{
    parallel_map, run_scenarios, table_from, tracked_with_non_carrying_asic, uniform_difficulty,
    variant_name, weak_ratio,
};
use crate::transition::{
    HOURS_BEFORE_EVENT, HourSample, TransitionInputs, reach_hour, simulate_transition,
};
use p2poolv2_netsim::random::SimulationRandom;

/// `INCLUDER_CUT` values of the refusing-coalition sweep.
const STUDY_INCLUDER_CUTS: [f64; 4] = [0.0, 0.01, 0.02, 0.05];

/// Share of the hashrate refusing to carry others' weak shares.
const REFUSING_FRACTIONS: [f64; 3] = [0.0, 0.5, 0.9];

/// Fork choice rules of the refusing-coalition sweep.
const FORK_CHOICES: [ForkChoice; 3] = [
    ForkChoice::ShareWork,
    ForkChoice::WeakWork,
    ForkChoice::WeakWorkSiblings,
];

/// Pool sizes of the miner-mix table.
const MIX_POOL_FRACTIONS: [f64; 3] = [0.01, 0.10, 0.30];

/// Runs per transition case, averaged.
const TRANSITION_SEEDS: usize = 8;

/// Seconds from genesis to the arrival or departure: a week, so `R`, `C`
/// and the window have settled.
const TRANSITION_EVENT_SECONDS: f64 = 7.0 * 86_400.0;

/// Seconds simulated after the arrival or departure: two weeks.
const TRANSITION_AFTER_SECONDS: f64 = 14.0 * 86_400.0;

/// Hours at the end of a transition run that set the settled values.
const SETTLED_HOURS: usize = 72;

/// How close to its settled value a measure must stay to count as settled.
const SETTLE_TOLERANCE: f64 = 0.10;

/// Hour ranges after the event over which home miners' pay is reported.
const PAY_PERIODS: [(usize, usize, &str); 7] = [
    (0, 1, "0-1 h"),
    (1, 6, "1-6 h"),
    (6, 24, "6-24 h"),
    (24, 48, "1-2 d"),
    (48, 96, "2-4 d"),
    (96, 192, "4-8 d"),
    (192, 336, "8-14 d"),
];

/// Bitcoin Cash's hashrate in the tiny-pool table, as a multiple of
/// mainnet's: 37 GH/s, so a 1 GH/s pool finds blocks at all.
const TINY_NETWORK_MULTIPLE: f64 = 1.0e-8;

/// The tiny pool's two miners, hashes a second.
const TINY_SMALL_HASHRATE: f64 = 0.25e9;
const TINY_LARGE_HASHRATE: f64 = 0.75e9;

/// Home miners in the home pool of the flood table.
const FLOOD_HOME_MINERS: f64 = 1_000.0;

/// Flood multiples of the honest home hashrate.
const FLOOD_MULTIPLES: [f64; 4] = [0.0, 10.0, 100.0, 1_000.0];

/// `INCLUDER_CUT` values of the flood table.
const FLOOD_INCLUDER_CUTS: [f64; 2] = [0.01, 0.05];

/// Pool sizes of the steering and loss tables.
const STEERING_POOL_FRACTIONS: [f64; 3] = [0.01, 0.10, 0.30];
const LOSS_POOL_FRACTIONS: [f64; 2] = [0.01, 0.10];

/// Share periods of the loss table, seconds.
const LOSS_PERIODS: [f64; 2] = [10.0, 2.0];

/// The design as the page now has it, from `base`'s months and seed.
pub fn study_scenario(base: &Scenario) -> Scenario {
    Scenario {
        design: Design::BandWeakLevels,
        weak_credit: WeakCredit::WeakWork,
        includer_cut: PAGE_INCLUDER_CUT,
        ..base.clone()
    }
}

/// A difference as a signed percentage: `+1.20%`, `-0.35%`.
fn signed_percent(fraction: f64) -> String {
    format!("{:+.2}%", fraction * 100.0)
}

/// A number of satoshis with its sign, grouped in thousands: `+1,308`,
/// `-5,306`.
fn signed_satoshis(value: f64) -> String {
    let sign = if value < 0.0 { "-" } else { "+" };
    format!("{sign}{}", rounded(value.abs()))
}

/// What refusing gains the coalition as a whole: its classes' expected
/// income over what the same classes earn, per unit of fair income, when
/// everyone carries (`all_carry`), less one; `None` without a coalition.
fn coalition_gain(report: &Report, all_carry: &Report) -> Option<f64> {
    let mut expected = 0.0;
    let mut if_carrying = 0.0;
    report
        .classes
        .iter()
        .filter(|class| class.kind != MinerKind::Tracked && !class.on_carrying_node)
        .for_each(|class| {
            let carrying_name = class.name.trim_end_matches(NON_CARRYING_SUFFIX);
            let carrying_ratio = all_carry
                .classes
                .iter()
                .find(|other| other.name == carrying_name)
                .map_or(1.0, |other| {
                    other.expected_income_per_month / other.fair_income_per_month
                });
            expected += class.expected_income_per_month;
            if_carrying += class.fair_income_per_month * carrying_ratio;
        });
    (if_carrying > 0.0).then(|| expected / if_carrying - 1.0)
}

/// What refusing gains the tracked refusing ASIC: its expected income over
/// the carrying ASIC's in the all-carrying run. In the all-carrying run
/// itself it is a lone refuser.
fn refusing_asic_gain(report: &Report, all_carry: &Report) -> f64 {
    vs_fair(report, 2) / vs_fair(all_carry, 1) - 1.0
}

/// Honest (carrying nodes') weak shares that expire or are lost.
fn honest_weak_lost(report: &Report) -> f64 {
    let races = report
        .home_chain()
        .races
        .as_ref()
        .expect("the refusing tables run share races");
    races.queue.carrying_expired_fraction + races.carrying_lost_fraction
}

/// The scenarios of a refusing sweep, one group of `REFUSING_FRACTIONS`
/// per scenario of `groups`, all-carrying first; and their reports.
fn run_refusing_groups(groups: &[Scenario]) -> Vec<Report> {
    let scenarios: Vec<Scenario> = groups
        .iter()
        .flat_map(|group| {
            REFUSING_FRACTIONS
                .into_iter()
                .map(move |refusing| Scenario {
                    non_carrying_fraction: refusing,
                    tracked: tracked_with_non_carrying_asic(),
                    ..group.clone()
                })
        })
        .collect();
    run_scenarios(&scenarios)
}

/// The stale fraction of the found shares of a report's untracked classes
/// on carrying (or non-carrying) nodes; `None` when there are none.
fn stale_of(report: &Report, on_carrying_node: bool) -> Option<f64> {
    let (stale, found) = report
        .classes
        .iter()
        .filter(|class| {
            class.kind != MinerKind::Tracked && class.on_carrying_node == on_carrying_node
        })
        .fold((0.0, 0.0), |(stale, found), class| {
            (
                stale + class.found_share_rate * class.stale_rate,
                found + class.found_share_rate,
            )
        });
    (found > 0.0).then(|| stale / found)
}

/// Expected income over fair income of a tracked miner, unrounded.
fn vs_fair(report: &Report, miner: usize) -> f64 {
    report.miners[miner].expected_income_per_month / report.miners[miner].fair_income_per_month
}

/// 1. Refusing coalition: for each fork choice rule, cut and refusing
///    share, honest weak shares lost, stale rates, each tracked miner's
///    expected income over its work, and what refusing gains the coalition.
///
/// Each group of three rows shares a pool, rule and cut, and starts with
/// the run where everyone carries. "Coalition gains" compares the refusing
/// classes with the same classes in that run; "refusing ASIC gains"
/// compares the tracked refusing ASIC with the carrying ASIC there.
pub fn refusing_races(base: &Scenario) -> String {
    let study = study_scenario(base);
    let groups: Vec<Scenario> = [0.01, 0.10]
        .into_iter()
        .flat_map(|fraction| {
            let study = study.clone();
            FORK_CHOICES.into_iter().flat_map(move |fork_choice| {
                let study = study.clone();
                STUDY_INCLUDER_CUTS.into_iter().map(move |cut| Scenario {
                    pool_fraction: fraction,
                    includer_cut: cut,
                    share_races: Some(ShareRaces::without_losses(fork_choice)),
                    ..study.clone()
                })
            })
        })
        .collect();
    let reports = run_refusing_groups(&groups);
    let rows: Vec<Vec<String>> = reports
        .chunks(REFUSING_FRACTIONS.len())
        .flat_map(|group| {
            let all_carry = &group[0];
            group
                .iter()
                .map(move |report| refusing_row(report, all_carry))
        })
        .collect();
    let headers = [
        "Pool",
        "Fork choice",
        "INCLUDER_CUT",
        "Refusing",
        "Honest weak lost",
        "Honest stale",
        "Refusing stale",
        "Tips replaced per share",
        "Bitaxe vs fair",
        "Carrying ASIC vs fair",
        "Refusing ASIC vs fair",
        "Coalition gains",
        "Refusing ASIC gains",
    ];
    markdown_table(&headers, &rows)
}

/// The fork choice rule a report's races ran under.
fn fork_choice_name(report: &Report) -> String {
    report
        .scenario
        .share_races
        .map_or("-", |share_races| share_races.fork_choice.name())
        .to_string()
}

fn refusing_row(report: &Report, all_carry: &Report) -> Vec<String> {
    let scenario = &report.scenario;
    let races = report
        .home_chain()
        .races
        .as_ref()
        .expect("the refusing table runs share races");
    vec![
        pool_percent(scenario.pool_fraction),
        fork_choice_name(report),
        percent(scenario.includer_cut),
        percent(scenario.non_carrying_fraction),
        percent(honest_weak_lost(report)),
        optional(stale_of(report, true), percent),
        optional(stale_of(report, false), percent),
        rounded(races.replaced_per_share),
        expected_ratio(&report.miners[0]),
        expected_ratio(&report.miners[1]),
        expected_ratio(&report.miners[2]),
        optional(coalition_gain(report, all_carry), signed_percent),
        signed_percent(refusing_asic_gain(report, all_carry)),
    ]
}

/// `WEAK_DEPTH` values of its sweep; 48, the rule's, is in the refusing
/// table.
const WEAK_DEPTHS: [usize; 3] = [6, 12, 24];

/// `INCLUDER_CUT` values of the `WEAK_DEPTH` sweep.
const DEPTH_SWEEP_INCLUDER_CUTS: [f64; 3] = [0.0, 0.01, 0.05];

/// Constants: `WEAK_DEPTH` and `INCLUDER_CUT` together against a refusing
/// coalition, with weak work in fork choice, with and without carriers
/// mining siblings of light tips. Only the refusing rows are printed; each
/// group's all-carrying run is their baseline, and its lone refusing ASIC
/// gives the last column.
pub fn weak_depth(base: &Scenario) -> String {
    let study = study_scenario(base);
    let groups: Vec<Scenario> = [0.01, 0.10]
        .into_iter()
        .flat_map(|fraction| {
            let study = study.clone();
            [ForkChoice::WeakWork, ForkChoice::WeakWorkSiblings]
                .into_iter()
                .flat_map(move |fork_choice| {
                    let study = study.clone();
                    WEAK_DEPTHS.into_iter().flat_map(move |depth| {
                        let study = study.clone();
                        DEPTH_SWEEP_INCLUDER_CUTS
                            .into_iter()
                            .map(move |cut| Scenario {
                                pool_fraction: fraction,
                                includer_cut: cut,
                                weak_depth: depth,
                                share_races: Some(ShareRaces::without_losses(fork_choice)),
                                ..study.clone()
                            })
                    })
                })
        })
        .collect();
    let reports = run_refusing_groups(&groups);
    let rows: Vec<Vec<String>> = reports
        .chunks(REFUSING_FRACTIONS.len())
        .flat_map(|group| {
            let all_carry = &group[0];
            group[1..].iter().map(move |report| {
                let scenario = &report.scenario;
                vec![
                    pool_percent(scenario.pool_fraction),
                    fork_choice_name(report),
                    scenario.weak_depth.to_string(),
                    percent(scenario.includer_cut),
                    percent(scenario.non_carrying_fraction),
                    percent(honest_weak_lost(report)),
                    expected_ratio(&report.miners[0]),
                    optional(report.miners[0].swing, two_decimals),
                    optional(coalition_gain(report, all_carry), signed_percent),
                    signed_percent(refusing_asic_gain(report, all_carry)),
                    signed_percent(refusing_asic_gain(all_carry, all_carry)),
                ]
            })
        })
        .collect();
    let headers = [
        "Pool",
        "Fork choice",
        "WEAK_DEPTH",
        "INCLUDER_CUT",
        "Refusing",
        "Honest weak lost",
        "Bitaxe vs fair",
        "Bitaxe swing",
        "Coalition gains",
        "Refusing ASIC gains",
        "Lone refuser gains",
    ];
    markdown_table(&headers, &rows)
}

/// 2. Miner mixes: a Zipf spread and mostly medium miners, one chain
///    against the page's design.
pub fn mixes(base: &Scenario) -> String {
    let study = study_scenario(base);
    let designs = [
        (Design::OneChain, WeakCredit::Page),
        (Design::BandWeakLevels, WeakCredit::WeakWork),
    ];
    let scenarios: Vec<Scenario> = MinerMix::STUDY
        .into_iter()
        .flat_map(|mix| {
            let study = study.clone();
            MIX_POOL_FRACTIONS.into_iter().flat_map(move |fraction| {
                let study = study.clone();
                designs.into_iter().map(move |(design, credit)| Scenario {
                    mix,
                    pool_fraction: fraction,
                    design,
                    weak_credit: credit,
                    ..study.clone()
                })
            })
        })
        .collect();
    let reports = run_scenarios(&scenarios);
    let rows: Vec<Vec<String>> = reports
        .iter()
        .map(|report| {
            let chain = report.home_chain();
            let pool_hashrate = report.pool_fraction * report.scenario.network_hashrate();
            let miners: f64 = report.classes.iter().map(|class| class.count).sum();
            let largest = report
                .classes
                .iter()
                .map(|class| class.miner_hashrate)
                .fold(0.0, f64::max);
            let mut row = vec![
                report.scenario.mix.name().to_string(),
                pool_percent(report.scenario.pool_fraction),
                variant_name(&report.scenario),
                rounded(miners),
                percent(largest / pool_hashrate),
                percent(chain.minimum_hashrate_fraction),
                two_decimals(chain.asert_difficulty / uniform_difficulty(report)),
                weak_ratio(report),
                optional(
                    chain.weak.map(|outcome| outcome.carrying_expired_fraction),
                    percent,
                ),
            ];
            row.extend(both_miner_cells(report));
            row
        })
        .collect();
    let mut headers: Vec<String> = [
        "Mix",
        "Pool",
        "Design",
        "Miners",
        "Largest miner",
        "Hashrate at the minimum",
        "Minimum / uniform",
        "R",
        "Weak expired",
    ]
    .iter()
    .map(|cell| cell.to_string())
    .collect();
    headers.extend(both_miner_headers());
    table_from(&headers, &rows)
}

/// One transition case of the arrivals table.
#[derive(Debug, Clone, Copy)]
struct TransitionCase {
    pool_fraction: f64,
    leaves: bool,
    big_follows_node_default: bool,
}

impl TransitionCase {
    fn event(&self) -> &'static str {
        if self.leaves { "leaves" } else { "joins" }
    }

    fn jobs(&self) -> &'static str {
        if self.big_follows_node_default {
            "node default"
        } else {
            "at the minimum"
        }
    }

    /// The case's inputs: the 50% industrial mix at the case's pool size,
    /// and a big miner with as much hashrate again.
    fn inputs(&self, base: &Scenario) -> TransitionInputs {
        let pool_hashrate = base.network_hashrate() * self.pool_fraction;
        let mut classes = MinerMix::HalfIndustrial.classes(pool_hashrate, DEFAULT_LEVEL);
        let big_miner = classes.len();
        classes.push(MinerClass {
            name: "big miner".to_string(),
            kind: MinerKind::Industrial,
            hashrate: pool_hashrate,
            count: 1.0,
            follows_node_default: self.big_follows_node_default,
            on_carrying_node: true,
            level: DEFAULT_LEVEL,
        });
        TransitionInputs {
            classes,
            big_miner,
            big_miner_leaves: self.leaves,
            network_hashrate: base.network_hashrate(),
            period_seconds: base.period_seconds,
            stale_rate: base.stale_rate,
            ceiling_difficulty: MINI_CEILING_DIFFICULTY,
            cap_max: DEFAULT_BAND_CAP as u64,
            weak_cap: base.weak_cap,
            weak_fill: base.weak_fill,
            weak_depth: base.weak_depth,
            ratio_max: base.weak_ratio_max,
            includer_cut: PAGE_INCLUDER_CUT,
            event_seconds: TRANSITION_EVENT_SECONDS,
            after_seconds: TRANSITION_AFTER_SECONDS,
        }
    }
}

/// The sum of `hours[start..end]`.
fn hours_sum(hours: &[HourSample], start: usize, end: usize) -> HourSample {
    let mut sum = HourSample::default();
    hours[start..end.min(hours.len())]
        .iter()
        .for_each(|hour| sum.add(hour));
    sum
}

/// The first hour after the event in which a measure's mean is within
/// `SETTLE_TOLERANCE` of where it settles.
fn settle_cell(values: &[f64], settled: f64) -> String {
    match reach_hour(values, settled, SETTLE_TOLERANCE) {
        Some(0) => "< 1 h".to_string(),
        Some(hour) => format!("{hour} h"),
        None => "not reached".to_string(),
    }
}

/// 3. A big miner (half the pool) joins or leaves at once: how long the
///    minimum, `C` and `R` take to settle, and home miners' pay meanwhile.
pub fn arrivals(base: &Scenario) -> String {
    let cases: Vec<TransitionCase> = [0.01, 0.10]
        .into_iter()
        .flat_map(|pool_fraction| {
            [false, true].into_iter().flat_map(move |leaves| {
                [true, false]
                    .into_iter()
                    .map(move |big_follows_node_default| TransitionCase {
                        pool_fraction,
                        leaves,
                        big_follows_node_default,
                    })
            })
        })
        .collect();
    let mut seeder = SimulationRandom::from_seed(base.seed);
    let jobs: Vec<(TransitionInputs, u64)> = cases
        .iter()
        .flat_map(|case| {
            let inputs = case.inputs(base);
            (0..TRANSITION_SEEDS)
                .map(|_| (inputs.clone(), seeder.next_u64()))
                .collect::<Vec<_>>()
        })
        .collect();
    let runs = parallel_map(&jobs, |(inputs, seed)| {
        simulate_transition(inputs, &mut SimulationRandom::from_seed(*seed))
    });
    let summed: Vec<Vec<HourSample>> = runs
        .chunks(TRANSITION_SEEDS)
        .map(|seeds| {
            let mut hours = vec![HourSample::default(); seeds[0].len()];
            seeds.iter().for_each(|run| {
                hours
                    .iter_mut()
                    .zip(run)
                    .for_each(|(sum, hour)| sum.add(hour));
            });
            hours
        })
        .collect();
    let settling: Vec<Vec<String>> = cases
        .iter()
        .zip(&summed)
        .map(|(case, hours)| settling_row(case, hours))
        .collect();
    let pay: Vec<Vec<String>> = cases
        .iter()
        .zip(&summed)
        .map(|(case, hours)| pay_row(case, hours))
        .collect();
    let settling_headers = [
        "Pool",
        "Big miner",
        "Its jobs",
        "Minimum after / before",
        "C before",
        "C after",
        "R before",
        "R after",
        "Minimum within 10%",
        "C within 10%",
        "R within 10%",
        "Weak expired, first 6 h",
    ];
    let mut pay_headers = vec!["Pool", "Big miner", "Its jobs", "Before"];
    PAY_PERIODS
        .iter()
        .for_each(|(_, _, label)| pay_headers.push(label));
    format!(
        "### Settling\n\n{}\n### Home miners' expected pay over fair, after the event\n\n{}",
        markdown_table(&settling_headers, &settling),
        markdown_table(&pay_headers, &pay)
    )
}

fn settling_row(case: &TransitionCase, hours: &[HourSample]) -> Vec<String> {
    let before = hours_sum(hours, 0, HOURS_BEFORE_EVENT);
    let after = hours_sum(hours, hours.len() - SETTLED_HOURS, hours.len());
    let early = hours_sum(hours, HOURS_BEFORE_EVENT, HOURS_BEFORE_EVENT + 6);
    let following = &hours[HOURS_BEFORE_EVENT..];
    let minimum: Vec<f64> = following.iter().map(HourSample::minimum).collect();
    let band_cap: Vec<f64> = following.iter().map(HourSample::band_cap).collect();
    let weak_ratio: Vec<f64> = following.iter().map(HourSample::weak_ratio).collect();
    let early_expired = if early.weak_found > 0.0 {
        early.weak_expired / early.weak_found
    } else {
        0.0
    };
    vec![
        pool_percent(case.pool_fraction),
        case.event().to_string(),
        case.jobs().to_string(),
        two_decimals(after.minimum() / before.minimum()),
        rounded(before.band_cap()),
        rounded(after.band_cap()),
        rounded(before.weak_ratio()),
        rounded(after.weak_ratio()),
        settle_cell(&minimum, after.minimum()),
        settle_cell(&band_cap, after.band_cap()),
        settle_cell(&weak_ratio, after.weak_ratio()),
        percent(early_expired),
    ]
}

fn pay_row(case: &TransitionCase, hours: &[HourSample]) -> Vec<String> {
    let before = hours_sum(hours, 0, HOURS_BEFORE_EVENT);
    let mut row = vec![
        pool_percent(case.pool_fraction),
        case.event().to_string(),
        case.jobs().to_string(),
        two_decimals(before.home_income / before.home_fair_income),
    ];
    PAY_PERIODS.iter().for_each(|(start, end, _)| {
        let period = hours_sum(hours, HOURS_BEFORE_EVENT + start, HOURS_BEFORE_EVENT + end);
        row.push(two_decimals(period.home_income / period.home_fair_income));
    });
    row
}

/// `R_MAX` values of its sweep.
const RATIO_MAXIMA: [u64; 3] = [256, 1_024, 4_096];

/// Seconds simulated from genesis in the `R_MAX` table: two days.
const GENESIS_SECONDS: f64 = 2.0 * 86_400.0;

/// Hours at the end of a genesis run that set where `R` settles.
const GENESIS_SETTLED_HOURS: usize = 24;

/// A chain from genesis, the 50% industrial mix at `pool_fraction`, with
/// `R_MAX` at `ratio_max`. The transition's big miner has no hashrate, so
/// nothing happens but the start.
fn genesis_inputs(base: &Scenario, pool_fraction: f64, ratio_max: u64) -> TransitionInputs {
    let pool_hashrate = base.network_hashrate() * pool_fraction;
    let mut classes = MinerMix::HalfIndustrial.classes(pool_hashrate, DEFAULT_LEVEL);
    let big_miner = classes.len();
    classes.push(MinerClass {
        name: "nobody".to_string(),
        kind: MinerKind::Industrial,
        hashrate: pool_hashrate,
        count: 0.0,
        follows_node_default: true,
        on_carrying_node: true,
        level: DEFAULT_LEVEL,
    });
    TransitionInputs {
        classes,
        big_miner,
        big_miner_leaves: false,
        network_hashrate: base.network_hashrate(),
        period_seconds: base.period_seconds,
        stale_rate: base.stale_rate,
        ceiling_difficulty: MINI_CEILING_DIFFICULTY,
        cap_max: DEFAULT_BAND_CAP as u64,
        weak_cap: base.weak_cap,
        weak_fill: base.weak_fill,
        weak_depth: base.weak_depth,
        ratio_max,
        includer_cut: PAGE_INCLUDER_CUT,
        event_seconds: 0.0,
        after_seconds: GENESIS_SECONDS,
    }
}

/// Constants: `R_MAX`, which is also the weak ratio at genesis. From
/// genesis: where `R` settles, the first hour it gets within 10% of it, and
/// the weak shares found (all relayed and checked) and lost in the first
/// hour, 50% industrial.
pub fn ratio_max(base: &Scenario) -> String {
    let cases: Vec<(f64, u64)> = [0.01, 0.10]
        .into_iter()
        .flat_map(|fraction| {
            RATIO_MAXIMA
                .into_iter()
                .map(move |maximum| (fraction, maximum))
        })
        .collect();
    let mut seeder = SimulationRandom::from_seed(base.seed);
    let jobs: Vec<(TransitionInputs, u64)> = cases
        .iter()
        .flat_map(|(fraction, maximum)| {
            let inputs = genesis_inputs(base, *fraction, *maximum);
            (0..TRANSITION_SEEDS)
                .map(|_| (inputs.clone(), seeder.next_u64()))
                .collect::<Vec<_>>()
        })
        .collect();
    let runs = parallel_map(&jobs, |(inputs, seed)| {
        simulate_transition(inputs, &mut SimulationRandom::from_seed(*seed))
    });
    let rows: Vec<Vec<String>> = cases
        .iter()
        .zip(runs.chunks(TRANSITION_SEEDS))
        .map(|((fraction, maximum), seeds)| {
            let mut hours = vec![HourSample::default(); seeds[0].len()];
            seeds.iter().for_each(|run| {
                hours
                    .iter_mut()
                    .zip(run)
                    .for_each(|(sum, hour)| sum.add(hour));
            });
            let from_genesis = &hours[HOURS_BEFORE_EVENT..];
            let settled = hours_sum(&hours, hours.len() - GENESIS_SETTLED_HOURS, hours.len());
            let first = from_genesis[0];
            let ratios: Vec<f64> = from_genesis.iter().map(HourSample::weak_ratio).collect();
            let found_per_second = first.weak_found / first.seconds;
            vec![
                pool_percent(*fraction),
                maximum.to_string(),
                rounded(settled.weak_ratio()),
                settle_cell(&ratios, settled.weak_ratio()),
                rounded(found_per_second),
                rounded(found_per_second * WEAK_SHARE_MESSAGE_BYTES / 1_000.0),
                percent(first.weak_expired / first.weak_found),
            ]
        })
        .collect();
    let headers = [
        "Pool",
        "R_MAX",
        "R settles at",
        "R gets within 10%",
        "Weak found/s, first hour",
        "Weak KB/s, first hour",
        "Weak expired, first hour",
    ];
    markdown_table(&headers, &rows)
}

/// `R_MAX` values of the mix sweep: the rule's and the one before it.
const MIX_RATIO_MAXIMA: [u64; 2] = [256, 4_096];

/// Constants: `R_MAX` in every miner mix. `R` settles where the hashrate at
/// the minimum fills `WEAK_FILL` weak slots, so the less of a pool's
/// hashrate mines at the minimum, the higher `R` wants to go: a pool of
/// almost only heavier shares is where 256 can bind and 4,096 not.
pub fn ratio_max_mixes(base: &Scenario) -> String {
    let study = study_scenario(base);
    let scenarios: Vec<Scenario> = MinerMix::ALL
        .into_iter()
        .chain(MinerMix::STUDY)
        .chain([MinerMix::AlmostAllIndustrial])
        .flat_map(|mix| {
            let study = study.clone();
            MIX_POOL_FRACTIONS.into_iter().flat_map(move |fraction| {
                let study = study.clone();
                MIX_RATIO_MAXIMA.into_iter().map(move |ratio_max| Scenario {
                    mix,
                    pool_fraction: fraction,
                    weak_ratio_max: ratio_max,
                    ..study.clone()
                })
            })
        })
        .collect();
    let reports = run_scenarios(&scenarios);
    let rows: Vec<Vec<String>> = reports
        .iter()
        .map(|report| {
            let chain = report.home_chain();
            let mut row = vec![
                report.scenario.mix.name().to_string(),
                pool_percent(report.scenario.pool_fraction),
                report.scenario.weak_ratio_max.to_string(),
                percent(chain.minimum_hashrate_fraction),
                weak_ratio(report),
                optional(
                    chain.weak.map(|outcome| outcome.carrying_expired_fraction),
                    percent,
                ),
            ];
            row.extend(both_miner_cells(report));
            row
        })
        .collect();
    let mut headers: Vec<String> = [
        "Mix",
        "Pool",
        "R_MAX",
        "Hashrate at the minimum",
        "R",
        "Weak expired",
    ]
    .iter()
    .map(|cell| cell.to_string())
    .collect();
    headers.extend(both_miner_headers());
    table_from(&headers, &rows)
}

/// The tiny pool's two tracked miners.
fn tiny_tracked() -> Vec<TrackedMiner> {
    vec![
        TrackedMiner {
            name: "small 0.25 GH/s".to_string(),
            hashrate: TINY_SMALL_HASHRATE,
            level: DEFAULT_LEVEL,
            on_carrying_node: true,
        },
        TrackedMiner {
            name: "large 0.75 GH/s".to_string(),
            hashrate: TINY_LARGE_HASHRATE,
            level: DEFAULT_LEVEL,
            on_carrying_node: true,
        },
    ]
}

/// Whether two reports paid every tracked miner exactly alike.
fn same_payments(first: &Report, second: &Report) -> bool {
    first.miners.len() == second.miners.len()
        && first.miners.iter().zip(&second.miners).all(|(one, other)| {
            one.income_per_month == other.income_per_month
                && one.payments_per_month == other.payments_per_month
                && one.median_payment == other.median_payment
                && one.swing == other.swing
        })
}

/// 4. A tiny pool whose minimum is the chain's ceiling: weak shares switch
///    off, and the rows say which earlier row paid exactly the same.
pub fn tiny_pool(base: &Scenario) -> String {
    let network = BCH_NETWORK_HASHRATE * TINY_NETWORK_MULTIPLE;
    let tiny = Scenario {
        hashrate_multiple: TINY_NETWORK_MULTIPLE,
        pool_fraction: (TINY_SMALL_HASHRATE + TINY_LARGE_HASHRATE) / network,
        mix: MinerMix::Home,
        tracked: tiny_tracked(),
        ..study_scenario(base)
    };
    let variants = [
        (Design::OneChain, WeakCredit::Page, false),
        (Design::Band, WeakCredit::Page, false),
        (Design::BandWeak, WeakCredit::WeakWork, false),
        (Design::BandWeakLevels, WeakCredit::WeakWork, false),
        (Design::Band, WeakCredit::Page, true),
        (Design::BandWeak, WeakCredit::WeakWork, true),
        (Design::BandWeakLevels, WeakCredit::WeakWork, true),
    ];
    let scenarios: Vec<Scenario> = variants
        .iter()
        .map(|(design, credit, at_minimum)| Scenario {
            design: *design,
            weak_credit: *credit,
            every_miner_at_minimum: *at_minimum,
            ..tiny.clone()
        })
        .collect();
    let reports = run_scenarios(&scenarios);
    let rows: Vec<Vec<String>> = reports
        .iter()
        .enumerate()
        .map(|(index, report)| {
            let chain = report.home_chain();
            let same = reports[..index]
                .iter()
                .position(|earlier| same_payments(earlier, report))
                .map_or("-".to_string(), |earlier| row_label(&reports[earlier]));
            let mut row = vec![
                row_label(report),
                two_decimals(chain.asert_difficulty / MINI_CEILING_DIFFICULTY),
                rounded(report.miners[0].difficulty_multiple),
                rounded(report.miners[1].difficulty_multiple),
                weak_ratio(report),
                rounded(report.weak_shares_per_second()),
                rounded(report.shares_per_second()),
            ];
            row.extend(miner_cells(&report.miners[0]));
            row.extend(miner_cells(&report.miners[1]));
            row.push(same);
            row
        })
        .collect();
    let mut headers: Vec<String> = [
        "Design",
        "Minimum / ceiling",
        "Small share multiple",
        "Large share multiple",
        "R",
        "Weak/s",
        "Shares/s",
    ]
    .iter()
    .map(|cell| cell.to_string())
    .collect();
    headers.extend(miner_headers("Small"));
    headers.extend(miner_headers("Large"));
    headers.push("Pays exactly as".to_string());
    table_from(&headers, &rows)
}

/// A tiny-pool row's name: the design, and how miners' jobs are set.
fn row_label(report: &Report) -> String {
    let jobs = if report.scenario.every_miner_at_minimum {
        "all at the minimum"
    } else {
        "node default"
    };
    format!("{}, {jobs}", variant_name(&report.scenario))
}

/// One base pool of the flood table: its mix, its size and the floods run.
struct FloodBase {
    label: &'static str,
    mix: MinerMix,
    pool_fraction: f64,
    multiples: &'static [f64],
}

/// 5. Large miners at the minimum filling the weak slots: floods of 10,
///    100 and 1,000 times the honest home hashrate.
pub fn floods(base: &Scenario) -> String {
    let study = study_scenario(base);
    let bases = [
        FloodBase {
            label: "1,000 home miners",
            mix: MinerMix::Home,
            pool_fraction: FLOOD_HOME_MINERS * HOME_MINER_HASHRATE / base.network_hashrate(),
            multiples: &FLOOD_MULTIPLES,
        },
        FloodBase {
            label: "90% industrial",
            mix: MinerMix::MostlyIndustrial,
            pool_fraction: 0.01,
            multiples: &FLOOD_MULTIPLES[..3],
        },
    ];
    let cases: Vec<(&str, Scenario)> = bases
        .iter()
        .flat_map(|flood_base| {
            let study = study.clone();
            FLOOD_INCLUDER_CUTS.into_iter().flat_map(move |cut| {
                let study = study.clone();
                flood_base.multiples.iter().map(move |multiple| {
                    let scenario = Scenario {
                        mix: flood_base.mix,
                        pool_fraction: flood_base.pool_fraction,
                        includer_cut: cut,
                        spam_multiple: *multiple,
                        ..study.clone()
                    };
                    (flood_base.label, scenario)
                })
            })
        })
        .collect();
    let scenarios: Vec<Scenario> = cases.iter().map(|(_, scenario)| scenario.clone()).collect();
    let reports = run_scenarios(&scenarios);
    let rows: Vec<Vec<String>> = reports
        .iter()
        .zip(&cases)
        .map(|(report, (label, _))| flood_row(report, label))
        .collect();
    let headers = [
        "Pool",
        "Size",
        "Flood",
        "Pool with flood",
        "INCLUDER_CUT",
        "R",
        "Bitaxe weak/mo",
        "Bitaxe pay/mo",
        "Bitaxe median sat",
        "Bitaxe swing",
        "Bitaxe empty months",
        "Bitaxe expected vs fair",
        "Bitaxe simulated vs fair",
        "Flood's part of weak shares",
        "Flood vs fair",
        "Flood's cuts paid less received, of its fair income",
    ];
    markdown_table(&headers, &rows)
}

fn flood_row(report: &Report, label: &str) -> Vec<String> {
    let weak = report.home_chain().weak.unwrap_or_default();
    let bitaxe = &report.miners[0];
    let flood = report
        .classes
        .iter()
        .find(|class| class.kind == MinerKind::Spammer);
    let all_weak: f64 = report.classes.iter().map(|class| class.weak_rate).sum();
    vec![
        label.to_string(),
        pool_percent(report.scenario.pool_fraction),
        format!("{}x", report.scenario.spam_multiple),
        pool_percent(report.pool_fraction),
        percent(report.scenario.includer_cut),
        rounded(weak.mean_ratio),
        rounded(bitaxe.weak_shares_per_month),
        rounded(bitaxe.payments_per_month),
        optional(bitaxe.median_payment, satoshis),
        optional(bitaxe.swing, two_decimals),
        percent(bitaxe.months_without_payment),
        expected_ratio(bitaxe),
        income_ratio(bitaxe),
        optional(flood.map(|class| class.weak_rate / all_weak), percent),
        optional(
            flood.map(|class| class.expected_income_per_month / class.fair_income_per_month),
            two_decimals,
        ),
        optional(
            flood.map(|class| {
                (class.includer_paid_per_month - class.includer_received_per_month)
                    / class.fair_income_per_month
            }),
            percent,
        ),
    ]
}

/// 6. A miner steering the level draw by picking its parent, at its own
///    level with every payee at that level.
pub fn draw_steering(base: &Scenario) -> String {
    let study = study_scenario(base);
    let steerers = [(0, DEFAULT_LEVEL), (1, 7)];
    let modes = [SteeringMode::Races, SteeringMode::TipOrParent];
    let scenarios: Vec<Scenario> = STEERING_POOL_FRACTIONS
        .into_iter()
        .flat_map(|fraction| {
            let study = study.clone();
            steerers
                .into_iter()
                .flat_map(move |(tracked_index, level)| {
                    let study = study.clone();
                    modes.into_iter().map(move |mode| {
                        let mut scenario = Scenario {
                            pool_fraction: fraction,
                            default_level: level,
                            draw_steering: Some(DrawSteering {
                                tracked_index,
                                mode,
                            }),
                            ..study.clone()
                        };
                        scenario
                            .tracked
                            .iter_mut()
                            .for_each(|miner| miner.level = level);
                        scenario
                    })
                })
        })
        .collect();
    let reports = run_scenarios(&scenarios);
    let rows: Vec<Vec<String>> = reports
        .iter()
        .map(|report| {
            let scenario = &report.scenario;
            let steering = scenario
                .draw_steering
                .expect("the steering table runs the steering attack");
            let miner = &report.miners[steering.tracked_index];
            let blocks = miner.hashrate / scenario.network_hashrate() * BCH_BLOCKS_PER_MONTH;
            let net = miner.steering_gain_per_month - miner.steering_orphan_cost_per_month;
            vec![
                pool_percent(scenario.pool_fraction),
                miner.name.clone(),
                scenario.default_level.to_string(),
                steering.mode.name().to_string(),
                rounded(blocks),
                percent(miner.steering_alternative_better),
                rounded(miner.steering_gain_per_month),
                rounded(miner.steering_orphan_cost_per_month),
                signed_satoshis(net),
                signed_percent(net / miner.expected_income_per_month),
            ]
        })
        .collect();
    let headers = [
        "Pool",
        "Steering miner",
        "Level",
        "Picks between",
        "Blocks it finds a month",
        "Draws where the other parent is better",
        "Gain sat/mo",
        "Orphan cost sat/mo",
        "Net sat/mo",
        "Net of its income",
    ];
    markdown_table(&headers, &rows)
}

/// The loss cases of the loss table: name, weak shares lost to a stale
/// parent (as a share of the stale rate), whether uncle carriers drop.
const LOSS_CASES: [(&str, f64, bool); 4] = [
    ("none", 0.0, false),
    ("stale parent", 0.5, false),
    ("uncle carrier drops", 0.0, true),
    ("both", 0.5, true),
];

/// 7. Weak shares lost to a parent that leaves the chain (about half the
///    stale rate: half the hashrate mines on a race's loser for about a
///    share period) or dropped with a race's loser.
pub fn losses(base: &Scenario) -> String {
    let study = study_scenario(base);
    let scenarios: Vec<Scenario> = LOSS_POOL_FRACTIONS
        .into_iter()
        .flat_map(|fraction| {
            let study = study.clone();
            LOSS_PERIODS.into_iter().flat_map(move |period| {
                let study = study.clone();
                let stale = stale_rate_for_period(period);
                LOSS_CASES
                    .into_iter()
                    .map(move |(_, parent_share, drops)| Scenario {
                        pool_fraction: fraction,
                        period_seconds: period,
                        stale_rate: stale,
                        share_races: Some(ShareRaces {
                            fork_choice: ForkChoice::ShareWork,
                            stale_parent_loss: parent_share * stale,
                            uncle_carrier_drops: drops,
                        }),
                        ..study.clone()
                    })
            })
        })
        .collect();
    let reports = run_scenarios(&scenarios);
    let rows: Vec<Vec<String>> = reports
        .iter()
        .zip(LOSS_CASES.iter().cycle())
        .map(|(report, (name, _, _))| {
            let scenario = &report.scenario;
            let races = report
                .home_chain()
                .races
                .as_ref()
                .expect("the loss table runs share races");
            let bitaxe = &report.miners[0];
            vec![
                pool_percent(scenario.pool_fraction),
                format!("{} s", scenario.period_seconds),
                percent(scenario.stale_rate),
                name.to_string(),
                percent(races.carrying_lost_fraction),
                percent(races.queue.carrying_expired_fraction),
                rounded(bitaxe.weak_shares_per_month),
                rounded(bitaxe.payments_per_month),
                optional(bitaxe.swing, two_decimals),
                percent(bitaxe.months_without_payment),
                expected_ratio(bitaxe),
                income_ratio(bitaxe),
                expected_ratio(&report.miners[1]),
            ]
        })
        .collect();
    let headers = [
        "Pool",
        "Period",
        "Stale",
        "Losses",
        "Weak lost",
        "Weak expired",
        "Bitaxe weak/mo",
        "Bitaxe pay/mo",
        "Bitaxe swing",
        "Bitaxe empty months",
        "Bitaxe expected vs fair",
        "Bitaxe simulated vs fair",
        "ASIC expected vs fair",
    ];
    markdown_table(&headers, &rows)
}

/// Pool sizes of the scale table, as parts of Bitcoin Cash's hashrate.
const SCALE_POOL_FRACTIONS: [f64; 4] = [0.10, 0.30, 0.50, 0.90];

/// Bitcoin Cash hashrate multiples of the scale table: today's 3.7 EH/s, up
/// to Bitcoin's (about 270 times it) and four times that.
const SCALE_HASHRATE_MULTIPLES: [f64; 5] = [1.0, 10.0, 100.0, 270.0, 1_000.0];

/// Fewest payments a month a node leaves a miner expecting:
/// `MIN_PAYMENTS_PER_MONTH` in `p2poolv2_lib`'s `level_policy`.
const NODE_MIN_PAYMENTS_PER_MONTH: f64 = 3.0;

/// The payout level a node leaves a miner of `hashrate` at in a pool finding
/// `pool_blocks_per_month` blocks: the highest at or below the default at
/// which it expects `NODE_MIN_PAYMENTS_PER_MONTH` payments a month, or 0
/// (`affordable_payout_level` in `p2poolv2_lib`).
fn node_level(hashrate: f64, network_hashrate: f64, pool_blocks_per_month: f64) -> u32 {
    let fair_per_month = hashrate / network_hashrate * BCH_BLOCKS_PER_MONTH * BLOCK_REWARD_SATOSHIS;
    let per_block = fair_per_month / pool_blocks_per_month;
    (0..=DEFAULT_LEVEL)
        .rev()
        .find(|level| {
            let won_per_block = (per_block / level_amount(*level) as f64).min(1.0);
            pool_blocks_per_month * won_per_block >= NODE_MIN_PAYMENTS_PER_MONTH
        })
        .unwrap_or(0)
}

/// Bigger pools on a bigger Bitcoin Cash: the adopted design at 10 to 90% of
/// Bitcoin Cash and 1 to 1,000 times today's hashrate, 50% industrial, with
/// every home miner's level, the tracked Bitaxe's and the tracked ASIC's set
/// as a node sets them.
pub fn scale(base: &Scenario) -> String {
    let study = study_scenario(base);
    let scenarios: Vec<Scenario> = SCALE_HASHRATE_MULTIPLES
        .into_iter()
        .flat_map(|multiple| {
            let study = study.clone();
            SCALE_POOL_FRACTIONS.into_iter().map(move |fraction| {
                let network = BCH_NETWORK_HASHRATE * multiple;
                let pool_blocks = BCH_BLOCKS_PER_MONTH * fraction;
                let home_level = node_level(HOME_MINER_HASHRATE, network, pool_blocks);
                Scenario {
                    pool_fraction: fraction,
                    hashrate_multiple: multiple,
                    default_level: home_level,
                    tracked: vec![
                        TrackedMiner {
                            level: home_level,
                            ..TrackedMiner::bitaxe()
                        },
                        TrackedMiner {
                            level: node_level(ASIC_HASHRATE, network, pool_blocks),
                            ..TrackedMiner::asic()
                        },
                    ],
                    ..study.clone()
                }
            })
        })
        .collect();
    let reports = run_scenarios(&scenarios);
    let rows: Vec<Vec<String>> = reports
        .iter()
        .map(|report| {
            let chain = report.home_chain();
            let mut row = vec![
                format!("x{}", report.scenario.hashrate_multiple),
                pool_percent(report.scenario.pool_fraction),
                rounded(chain.asert_difficulty),
                weak_ratio(report),
                report.scenario.tracked[0].level.to_string(),
                rounded(report.miners[0].weak_shares_per_month),
                report.scenario.tracked[1].level.to_string(),
                rounded(report.miners[1].weak_shares_per_month),
            ];
            row.extend(both_miner_cells(report));
            row
        })
        .collect();
    let mut headers: Vec<String> = [
        "BCH hashrate",
        "Pool",
        "Minimum difficulty",
        "R",
        "Bitaxe level",
        "Bitaxe weak/mo",
        "ASIC level",
        "ASIC weak/mo",
    ]
    .iter()
    .map(|cell| cell.to_string())
    .collect();
    headers.extend(both_miner_headers());
    table_from(&headers, &rows)
}

#[cfg(test)]
mod tests;
