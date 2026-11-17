// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The weak ratio's path from genesis and after a big miner, with the asks.
//!
//! Section 6's rule as PR #118 writes it lets a share carry 640 weak shares
//! while the ratio still moves by 1/256 a share. A 12-node swarm found that
//! from genesis, at `R_MAX`, every share then carries hundreds of weak
//! shares for hundreds of shares; today's `WEAK_CAP` of 64 bounds that start.
//! The steady-state tables of `crate::ask_study` never saw it. These tables
//! run the rule and the variants that might fix it share by share in the
//! transition model (`crate::transition`): from genesis, and a big miner
//! joining or leaving at the minimum. They also run each variant in steady
//! state and under the attacks of `crate::ask_study`.

use crate::ask_study::{ASK_MIXES, WeakRule, outcome_cells, outcome_headers, ruled_scenario};
use crate::asks::{AskRule, CapRule, RatioDescent, WeakAsks};
use crate::constants::{
    BAND_CAP_STEP_DIVISOR, BCH_NETWORK_HASHRATE, DEFAULT_BAND_CAP, DEFAULT_LEVEL,
    MINI_CEILING_DIFFICULTY, PAGE_INCLUDER_CUT, WEAK_FILL,
};
use crate::miners::{MinerClass, MinerKind, MinerMix};
use crate::report::{markdown_table, percent, pool_percent, rounded, two_decimals};
use crate::scenario::Scenario;
use crate::study::hours_sum;
use crate::tables::{parallel_map, run_scenarios};
use crate::transition::{
    HOURS_BEFORE_EVENT, HourSample, TransitionInputs, simulate_transition, stay_hour,
};
use p2poolv2_netsim::random::SimulationRandom;

/// Runs per case, averaged.
const PATH_SEEDS: usize = 8;

/// Seconds simulated from genesis: two days.
const GENESIS_RUN_SECONDS: f64 = 2.0 * 86_400.0;

/// Seconds from genesis to a big miner's arrival or departure: a day, so the
/// ratio has settled from any genesis value.
const EVENT_SECONDS: f64 = 86_400.0;

/// Seconds simulated after the arrival or departure: two days.
const AFTER_EVENT_SECONDS: f64 = 2.0 * 86_400.0;

/// Hours after genesis or the event over which the start is measured.
const START_HOURS: usize = 6;

/// Hours at the end of a run that set where the count settles.
const SETTLED_HOURS: usize = 24;

/// How close to its settled value the hourly count must stay.
const STAY_TOLERANCE: f64 = 0.25;

/// The pools of every table: Bitcoin Cash's hashrate as a multiple of
/// today's, and the pool's part of it. Nobody needs to ask in the first;
/// the asks open the second to 189 weak shares a share and the third to the
/// ceiling (`crate::ask_study`).
const PATH_POOLS: [(f64, f64); 3] = [(1.0, 0.10), (10.0, 0.10), (10.0, 0.30)];

/// Bitcoin Cash's hashrate on the test network, as a multiple of mainnet's:
/// 3.7 TH/s.
const TEST_NETWORK_MULTIPLE: f64 = 1.0e-6;

/// The test network's pool: CPU-sized miners of 10 MH/s, 10 GH/s in all,
/// small enough that the chain's ceiling holds the weak ratio in force near
/// 5.
const TEST_MINER_HASHRATE: f64 = 10.0e6;
const TEST_POOL_HASHRATE: f64 = 10.0e9;

/// The miner that joins or leaves the test network's pool: 1 TH/s.
const TEST_BIG_MINER_HASHRATE: f64 = 1.0e12;

/// Part of the hashrate at the minimum that asks whatever it finds on the
/// test network's attacked rows: enough for about two asking weak shares a
/// share while the weak ratio is at `R_MAX`.
const TEST_ALWAYS_ASKING: f64 = 0.01;

/// Parts of the hashrate at the minimum that ask whatever their miners
/// find, in the attack table (as `crate::ask_study`).
const ALWAYS_ASKING: [f64; 5] = [0.0, 0.01, 0.02, 0.05, 0.25];

/// Parts of the hashrate at the minimum that never ask, in the attack table.
const NEVER_ASKING: [f64; 4] = [0.5, 0.9, 0.97, 0.99];

/// Fractions of chain shares from nodes that carry no asking weak share, in
/// the attack table.
const CENSORING_SHARES: [f64; 4] = [0.25, 0.5, 0.75, 0.9];

/// The rules the tables compare.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Variant {
    /// Today's rule: `WEAK_CAP` 64, no asks.
    Today,
    /// v0: section 6 as PR #118 writes it.
    AsWritten,
    /// v1: v0, but a share that was not asked and carried more than
    /// `WEAK_FILL` takes the ratio to `R * 32 / count` at once, never below
    /// `R / bound`.
    FasterDown(u64),
    /// v2: v0, but a share may carry more than 64 weak shares only when
    /// more than one in 64 of those it carries ask.
    CapFollowsAsks,
    /// v3: v0 with a genesis ratio of 32.
    LowGenesis,
    /// v4: v2 and v3 together.
    CapFollowsAsksLowGenesis,
    /// v5: v4, but one asking weak share opens the cap.
    CapOpensAtOneAsk,
}

impl Variant {
    /// Every rule, in table order.
    pub(crate) const ALL: [Variant; 8] = [
        Variant::Today,
        Variant::AsWritten,
        Variant::FasterDown(2),
        Variant::FasterDown(4),
        Variant::CapFollowsAsks,
        Variant::LowGenesis,
        Variant::CapFollowsAsksLowGenesis,
        Variant::CapOpensAtOneAsk,
    ];

    /// The rules of the attack table: the rule as written and those that
    /// change how the asks steer the count once the chain has started. v3
    /// and v4 are v0 and v2 after genesis.
    const ATTACKED: [Variant; 4] = [
        Variant::AsWritten,
        Variant::FasterDown(2),
        Variant::CapFollowsAsks,
        Variant::CapOpensAtOneAsk,
    ];

    /// The rules of the miner-mix table.
    const MIXED: [Variant; 4] = [
        Variant::Today,
        Variant::AsWritten,
        Variant::CapFollowsAsksLowGenesis,
        Variant::CapOpensAtOneAsk,
    ];

    /// Name in tables.
    pub(crate) fn name(&self) -> String {
        match self {
            Variant::Today => "today".to_string(),
            Variant::AsWritten => "v0 as written".to_string(),
            Variant::FasterDown(bound) => format!("v1 down to R/{bound}"),
            Variant::CapFollowsAsks => "v2 cap follows asks".to_string(),
            Variant::LowGenesis => "v3 genesis 32".to_string(),
            Variant::CapFollowsAsksLowGenesis => "v4 = v2 + v3".to_string(),
            Variant::CapOpensAtOneAsk => "v5 = v4, one ask opens".to_string(),
        }
    }

    /// The asks the rule runs with, or `None` for today's rule.
    pub(crate) fn asks(&self) -> Option<WeakAsks> {
        let written = AskRule::default();
        let rule = match self {
            Variant::Today => return None,
            Variant::AsWritten => written,
            Variant::FasterDown(bound) => AskRule {
                descent: RatioDescent::Proportional { bound: *bound },
                ..written
            },
            Variant::CapFollowsAsks => AskRule {
                cap: CapRule::FollowsAsks {
                    divisor: written.ask_divisor,
                },
                ..written
            },
            Variant::LowGenesis => AskRule {
                genesis_ratio: WEAK_FILL,
                ..written
            },
            Variant::CapFollowsAsksLowGenesis => AskRule {
                cap: CapRule::FollowsAsks {
                    divisor: written.ask_divisor,
                },
                genesis_ratio: WEAK_FILL,
                ..written
            },
            Variant::CapOpensAtOneAsk => AskRule {
                cap: CapRule::FollowsAsks {
                    divisor: written.weak_cap(),
                },
                genesis_ratio: WEAK_FILL,
                ..written
            },
        };
        Some(WeakAsks {
            rule,
            ..WeakAsks::default()
        })
    }

    /// The steady-state tables' weak rule for this variant.
    fn weak_rule(&self) -> WeakRule {
        self.asks().map_or(WeakRule::Today, WeakRule::Asks)
    }
}

/// A pool of the transition tables.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PathPool {
    /// Bitcoin Cash's hashrate as a multiple of today's.
    multiple: f64,
    /// The pool's hashrate.
    hashrate: f64,
    /// The test network's pool of small miners, instead of the 50%
    /// industrial mix.
    test_network: bool,
    /// Part of every class's hashrate at the minimum whose weak shares ask
    /// whatever its miners find: an attacker that wants the cap open.
    always_asking: f64,
}

impl PathPool {
    fn mainnet(multiple: f64, fraction: f64) -> Self {
        Self {
            multiple,
            hashrate: BCH_NETWORK_HASHRATE * multiple * fraction,
            test_network: false,
            always_asking: 0.0,
        }
    }

    fn test_network(always_asking: f64) -> Self {
        Self {
            multiple: TEST_NETWORK_MULTIPLE,
            hashrate: TEST_POOL_HASHRATE,
            test_network: true,
            always_asking,
        }
    }

    fn network_hashrate(&self) -> f64 {
        BCH_NETWORK_HASHRATE * self.multiple
    }

    fn label(&self) -> String {
        let pool = if self.test_network {
            "test network, 10 GH/s".to_string()
        } else {
            format!(
                "x{}, {}",
                self.multiple,
                pool_percent(self.hashrate / self.network_hashrate())
            )
        };
        if self.always_asking > 0.0 {
            format!("{pool}, {} always asks", pool_percent(self.always_asking))
        } else {
            pool
        }
    }

    /// The pool's classes before any big miner.
    fn classes(&self) -> Vec<MinerClass> {
        if self.test_network {
            vec![MinerClass {
                name: "test miners".to_string(),
                kind: MinerKind::Home,
                hashrate: TEST_MINER_HASHRATE,
                count: self.hashrate / TEST_MINER_HASHRATE,
                follows_node_default: true,
                on_carrying_node: true,
                level: DEFAULT_LEVEL,
            }]
        } else {
            MinerMix::HalfIndustrial.classes(self.hashrate, DEFAULT_LEVEL)
        }
    }

    /// The big miner's hashrate: as much as the pool's, or 1 TH/s on the
    /// test network.
    fn big_miner_hashrate(&self) -> f64 {
        if self.test_network {
            TEST_BIG_MINER_HASHRATE
        } else {
            self.hashrate
        }
    }
}

/// What happens in a transition run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Event {
    /// Nothing but the start from genesis.
    Genesis,
    /// A big miner at the minimum joins a day after genesis.
    Joins,
    /// A big miner at the minimum, there from genesis, leaves a day after.
    Leaves,
}

impl Event {
    fn name(&self) -> &'static str {
        match self {
            Event::Genesis => "genesis",
            Event::Joins => "joins",
            Event::Leaves => "leaves",
        }
    }
}

/// One transition case.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PathCase {
    pool: PathPool,
    event: Event,
    variant: Variant,
}

impl PathCase {
    /// The case's transition inputs, the design as the page has it with
    /// study #85's constants and the variant's rule.
    fn inputs(&self, base: &Scenario) -> TransitionInputs {
        let mut classes = self.pool.classes();
        let big_miner = classes.len();
        classes.push(MinerClass {
            name: "big miner".to_string(),
            kind: MinerKind::Industrial,
            hashrate: self.pool.big_miner_hashrate(),
            count: if self.event == Event::Genesis {
                0.0
            } else {
                1.0
            },
            follows_node_default: false,
            on_carrying_node: true,
            level: DEFAULT_LEVEL,
        });
        let (event_seconds, after_seconds) = match self.event {
            Event::Genesis => (0.0, GENESIS_RUN_SECONDS),
            Event::Joins | Event::Leaves => (EVENT_SECONDS, AFTER_EVENT_SECONDS),
        };
        TransitionInputs {
            classes,
            big_miner,
            big_miner_leaves: self.event == Event::Leaves,
            network_hashrate: self.pool.network_hashrate(),
            period_seconds: base.period_seconds,
            stale_rate: base.stale_rate,
            ceiling_difficulty: MINI_CEILING_DIFFICULTY,
            cap_max: DEFAULT_BAND_CAP as u64,
            band_step_divisor: BAND_CAP_STEP_DIVISOR as u64,
            overstating: None,
            weak_cap: base.weak_cap,
            weak_fill: base.weak_fill,
            weak_depth: base.weak_depth,
            ratio_max: base.weak_ratio_max,
            includer_cut: PAGE_INCLUDER_CUT,
            event_seconds,
            after_seconds,
            asks: self.variant.asks().map(|asks| WeakAsks {
                always_asking: self.pool.always_asking,
                ..asks
            }),
        }
    }
}

/// Run every case `PATH_SEEDS` times and return each case's hours, summed
/// over its runs.
fn run_cases(base: &Scenario, cases: &[PathCase]) -> Vec<Vec<HourSample>> {
    let mut seeder = SimulationRandom::from_seed(base.seed);
    let jobs: Vec<(TransitionInputs, u64)> = cases
        .iter()
        .flat_map(|case| {
            let inputs = case.inputs(base);
            (0..PATH_SEEDS)
                .map(|_| (inputs.clone(), seeder.next_u64()))
                .collect::<Vec<_>>()
        })
        .collect();
    let runs = parallel_map(&jobs, |(inputs, seed)| {
        simulate_transition(inputs, &mut SimulationRandom::from_seed(*seed))
    });
    runs.chunks(PATH_SEEDS)
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
        .collect()
}

/// The first hour from which the hourly count stays within
/// `STAY_TOLERANCE` of `settled`.
fn stay_cell(counts: &[f64], settled: f64) -> String {
    match stay_hour(counts, settled, STAY_TOLERANCE) {
        Some(0) => "< 1 h".to_string(),
        Some(hour) => format!("{hour} h"),
        None => "not reached".to_string(),
    }
}

/// What the count does from the start of `hours` (genesis or the event):
/// the highest hourly count, the largest share in any run, the shares a run
/// above 64 and above 320 in the first hours, when the count stays near
/// where it settles, the weak shares expired in the first hours, where it
/// settles and the weak shares expired once it has.
fn path_cells(hours: &[HourSample]) -> Vec<String> {
    let start = hours_sum(hours, 0, START_HOURS);
    let whole = hours_sum(hours, 0, hours.len());
    let settled = hours_sum(hours, hours.len() - SETTLED_HOURS, hours.len());
    let counts: Vec<f64> = hours.iter().map(HourSample::carried_per_share).collect();
    let peak = counts.iter().copied().fold(0.0, f64::max);
    let runs = PATH_SEEDS as f64;
    vec![
        rounded(peak),
        rounded(whole.largest_carried),
        rounded(start.shares_above_cap / runs),
        rounded(start.shares_above_fill_max / runs),
        stay_cell(&counts, settled.carried_per_share()),
        percent(start.expired_fraction()),
        rounded(settled.carried_per_share()),
        percent(settled.expired_fraction()),
    ]
}

/// The headers of `path_cells`, after the `leading` ones, its first hours
/// called `start`.
fn path_headers(leading: &[&str], start: &str) -> Vec<String> {
    let mut headers: Vec<String> = leading.iter().map(|cell| cell.to_string()).collect();
    headers.extend([
        "Peak weak a share (hourly)".to_string(),
        "Largest share".to_string(),
        format!("Shares above 64, {start}"),
        format!("Shares above 320, {start}"),
        "Count stays within 25%".to_string(),
        format!("Weak expired, {start}"),
        "Weak a share, last day".to_string(),
        "Weak expired, last day".to_string(),
    ]);
    headers
}

/// From genesis: every rule in the three pools, 50% industrial. Each run
/// is two days; a run makes about 2,100 shares in its first six hours.
pub fn genesis(base: &Scenario) -> String {
    let cases: Vec<PathCase> = PATH_POOLS
        .into_iter()
        .flat_map(|(multiple, fraction)| {
            Variant::ALL.into_iter().map(move |variant| PathCase {
                pool: PathPool::mainnet(multiple, fraction),
                event: Event::Genesis,
                variant,
            })
        })
        .collect();
    let summed = run_cases(base, &cases);
    let rows: Vec<Vec<String>> = cases
        .iter()
        .zip(&summed)
        .map(|(case, hours)| {
            let mut row = vec![case.pool.label(), case.variant.name()];
            row.extend(path_cells(&hours[HOURS_BEFORE_EVENT..]));
            row
        })
        .collect();
    let headers = path_headers(&["BCH hashrate, pool", "Rule"], "first 6 h");
    let headers: Vec<&str> = headers.iter().map(String::as_str).collect();
    markdown_table(&headers, &rows)
}

/// A big miner at the minimum joining or leaving a day after genesis: as
/// much hashrate as the pool in the three pools, 1 TH/s on a 10 GH/s test
/// network pool, and the same with `TEST_ALWAYS_ASKING` of the hashrate at
/// the minimum asking whatever it finds, every rule.
pub fn big_miner(base: &Scenario) -> String {
    let pools: Vec<PathPool> = PATH_POOLS
        .into_iter()
        .map(|(multiple, fraction)| PathPool::mainnet(multiple, fraction))
        .chain([
            PathPool::test_network(0.0),
            PathPool::test_network(TEST_ALWAYS_ASKING),
        ])
        .collect();
    let cases: Vec<PathCase> = pools
        .into_iter()
        .flat_map(|pool| {
            [Event::Joins, Event::Leaves]
                .into_iter()
                .flat_map(move |event| {
                    Variant::ALL.into_iter().map(move |variant| PathCase {
                        pool,
                        event,
                        variant,
                    })
                })
        })
        .collect();
    let summed = run_cases(base, &cases);
    let rows: Vec<Vec<String>> = cases
        .iter()
        .zip(&summed)
        .map(|(case, hours)| {
            let before = hours_sum(hours, HOURS_BEFORE_EVENT - START_HOURS, HOURS_BEFORE_EVENT);
            let after = &hours[HOURS_BEFORE_EVENT..];
            let first_day = hours_sum(after, 0, 24);
            let mut row = vec![
                case.pool.label(),
                case.event.name().to_string(),
                case.variant.name(),
                rounded(before.carried_per_share()),
            ];
            row.extend(path_cells(after));
            row.push(two_decimals(
                first_day.home_income / first_day.home_fair_income,
            ));
            row
        })
        .collect();
    let mut headers = path_headers(
        &[
            "BCH hashrate, pool",
            "Big miner",
            "Rule",
            "Weak a share, 6 h before",
        ],
        "6 h after",
    );
    headers.push("Home pay over fair, first day".to_string());
    let headers: Vec<&str> = headers.iter().map(String::as_str).collect();
    markdown_table(&headers, &rows)
}

/// A steady-state table of `rows`, each a scenario with its leading cells:
/// the part of the weak shares found that expired, then what
/// `crate::ask_study`'s tables show.
fn rule_table(leading: &[&str], rows: Vec<(Vec<String>, Scenario)>) -> String {
    let (labels, scenarios): (Vec<Vec<String>>, Vec<Scenario>) = rows.into_iter().unzip();
    let reports = run_scenarios(&scenarios);
    let rows: Vec<Vec<String>> = labels
        .into_iter()
        .zip(&reports)
        .map(|(mut cells, report)| {
            let weak = report.home_chain().weak.unwrap_or_default();
            cells.push(percent(weak.carrying_expired_fraction));
            cells.extend(outcome_cells(report));
            cells
        })
        .collect();
    let mut leading = leading.to_vec();
    leading.push("Weak expired");
    let headers = outcome_headers(&leading);
    let headers: Vec<&str> = headers.iter().map(String::as_str).collect();
    markdown_table(&headers, &rows)
}

/// Where every rule settles, in steady state, in the three pools, 50%
/// industrial.
pub fn steady(base: &Scenario) -> String {
    let rows: Vec<(Vec<String>, Scenario)> = PATH_POOLS
        .into_iter()
        .flat_map(|(multiple, fraction)| {
            Variant::ALL.into_iter().map(move |variant| {
                (
                    vec![
                        format!("x{multiple}, {}", pool_percent(fraction)),
                        variant.name(),
                    ],
                    ruled_scenario(base, multiple, fraction, variant.weak_rule()),
                )
            })
        })
        .collect();
    rule_table(&["BCH hashrate, pool", "Rule"], rows)
}

/// Where the rules that bound the start settle in every miner mix of
/// `crate::ask_study`, at ten times today's hashrate, beside today's rule
/// and the rule as written.
pub fn mixes(base: &Scenario) -> String {
    let rows: Vec<(Vec<String>, Scenario)> = ASK_MIXES
        .into_iter()
        .flat_map(|mix| {
            [0.10, 0.30].into_iter().flat_map(move |fraction| {
                Variant::MIXED.into_iter().map(move |variant| {
                    (
                        vec![
                            mix.name().to_string(),
                            pool_percent(fraction),
                            variant.name(),
                        ],
                        Scenario {
                            mix,
                            ..ruled_scenario(base, 10.0, fraction, variant.weak_rule())
                        },
                    )
                })
            })
        })
        .collect();
    rule_table(&["Mix", "Pool", "Rule"], rows)
}

/// The attacks of `crate::ask_study` under the rule as written, the faster
/// way down and the two caps that follow the asks.
pub fn attacks(base: &Scenario) -> String {
    let mut rows: Vec<(Vec<String>, Scenario)> = Vec::with_capacity(64);
    let mut push = |attack: &str, size: String, pool: &str, rule: String, scenario: Scenario| {
        rows.push((
            vec![attack.to_string(), size, pool.to_string(), rule],
            scenario,
        ));
    };
    let with = |variant: Variant, change: &dyn Fn(WeakAsks) -> WeakAsks| -> WeakRule {
        variant
            .asks()
            .map_or(WeakRule::Today, |asks| WeakRule::Asks(change(asks)))
    };
    let needy = |rule: WeakRule| ruled_scenario(base, 10.0, 0.30, rule);
    ALWAYS_ASKING.into_iter().for_each(|always_asking| {
        Variant::ATTACKED.into_iter().for_each(|variant| {
            let rule = with(variant, &|asks| WeakAsks {
                always_asking,
                ..asks
            });
            push(
                "asks whatever it finds",
                percent(always_asking),
                "x1, 10%",
                variant.name(),
                ruled_scenario(base, 1.0, 0.10, rule),
            );
        });
    });
    NEVER_ASKING.into_iter().for_each(|never_asking| {
        Variant::ATTACKED.into_iter().for_each(|variant| {
            let rule = with(variant, &|asks| WeakAsks {
                never_asking,
                ..asks
            });
            push(
                "never asks",
                percent(never_asking),
                "x10, 30%",
                variant.name(),
                needy(rule),
            );
        });
    });
    [Variant::Today]
        .into_iter()
        .chain(Variant::ATTACKED)
        .for_each(|variant| {
            push(
                "industrial miners at the minimum",
                "-".to_string(),
                "x10, 30%",
                variant.name(),
                Scenario {
                    big_node_at_minimum: true,
                    ..needy(variant.weak_rule())
                },
            );
        });
    CENSORING_SHARES.into_iter().for_each(|censoring_shares| {
        Variant::ATTACKED.into_iter().for_each(|variant| {
            let rule = with(variant, &|asks| WeakAsks {
                censoring_shares,
                ..asks
            });
            push(
                "carries no asking weak share",
                percent(censoring_shares),
                "x10, 30%",
                variant.name(),
                needy(rule),
            );
        });
    });
    rule_table(&["Attack", "Its part", "BCH hashrate, pool", "Rule"], rows)
}

#[cfg(test)]
mod tests;
