// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! One scenario: a design, a pool, a miner mix and the rule constants, and
//! the report its run gives.
//!
//! A run builds the pool's miner classes, splits them into chains (two for
//! `mini`, one otherwise), solves each chain's steady-state difficulties,
//! runs the weak share queue where the design has weak shares, works out
//! every class's expected credit and the window, and then simulates the
//! tracked miners' payments month by month.

use crate::constants::{
    BCH_BLOCK_INTERVAL_SECONDS, BCH_BLOCKS_PER_MONTH, BCH_NETWORK_HASHRATE, BLOCK_REWARD_SATOSHIS,
    DEFAULT_BAND_CAP, DEFAULT_LEVEL, DONATION_FRACTION, HASHES_PER_DIFFICULTY, INCLUDER_CUT,
    LEVEL_COUNT, MAIN_CEILING_DIFFICULTY, MINI_CEILING_DIFFICULTY, MINI_THRESHOLD_HASHRATE,
    MONTH_SECONDS, SHARE_INTERVAL_SECONDS, SHARE_MESSAGE_BYTES, SPEND_COST_SATOSHIS,
    STALE_RATE_BY_PERIOD, UNCLE_PAID_FRACTION, WEAK_CAP, WEAK_DEPTH, WEAK_FILL, WEAK_RATIO_MAX,
    WEAK_SHARE_MESSAGE_BYTES,
};
use crate::difficulty::{
    ChainDifficulty, ChainParameters, DifficultyRule, chain_hashrate, found_share_rate,
    settled_band_cap, solve_chain,
};
use crate::levels::{level_amount, node_level};
use crate::miners::{MinerClass, MinerKind, MinerMix, TrackedMiner};
use crate::payouts::{
    ChainPayoutInputs, Steering, TrackedInputs, TrackedOutcome, simulate_chain_payouts,
};
use crate::races::{
    ForkChoice, RaceOutcome, RaceQueueInputs, RaceSettings, Racer, simulate_race_queue,
};
use crate::sampling::poisson;
use crate::weak::{WeakQueueInputs, WeakQueueOutcome, simulate_weak_queue};
use crate::window::{WindowGeometry, size_window};
use p2poolv2_netsim::random::SimulationRandom;

/// Samples per class when estimating the other payees' pot in a level group.
const LEVEL_POT_SAMPLES: usize = 2_000;

/// What the name of a class's part on non-carrying nodes adds to the
/// class's name.
pub const NON_CARRYING_SUFFIX: &str = " (non-carrying)";

/// The share chain designs compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Design {
    /// Today's rule: uniform difficulty, miners under the threshold on a
    /// separate mini chain with its own blocks.
    Mini,
    /// One chain, uniform difficulty.
    OneChain,
    /// One chain with the difficulty band.
    Band,
    /// Cash Dragon's vardiff chain.
    Vardiff,
    /// The band and weak shares, every payment direct.
    BandWeak,
    /// The band, weak shares and payout levels.
    BandWeakLevels,
}

impl Design {
    /// Every design, in table order.
    pub const ALL: [Design; 6] = [
        Design::Mini,
        Design::OneChain,
        Design::Band,
        Design::Vardiff,
        Design::BandWeak,
        Design::BandWeakLevels,
    ];

    /// Name in tables and on the command line.
    pub fn name(&self) -> &'static str {
        match self {
            Design::Mini => "mini",
            Design::OneChain => "one-chain",
            Design::Band => "band",
            Design::Vardiff => "vardiff-cd",
            Design::BandWeak => "band-weak",
            Design::BandWeakLevels => "band-weak-levels",
        }
    }

    /// Whether the design has weak shares.
    pub fn has_weak_shares(&self) -> bool {
        matches!(self, Design::BandWeak | Design::BandWeakLevels)
    }

    /// Whether the design pays small slices by the level draw.
    pub fn has_levels(&self) -> bool {
        matches!(self, Design::BandWeakLevels)
    }

    /// Whether the design has the difficulty band.
    pub fn has_band(&self) -> bool {
        matches!(
            self,
            Design::Band | Design::BandWeak | Design::BandWeakLevels
        )
    }

    fn rule(&self, band_cap: f64) -> DifficultyRule {
        match self {
            Design::Mini | Design::OneChain => DifficultyRule::Uniform,
            Design::Band | Design::BandWeak | Design::BandWeakLevels => {
                DifficultyRule::Band { cap: band_cap }
            }
            Design::Vardiff => DifficultyRule::Vardiff,
        }
    }
}

/// How a share mined on a job at the minimum is credited when the design
/// has weak shares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeakCredit {
    /// As the design page writes it: a full share its full work, and each
    /// weak share its weak work besides.
    Page,
    /// A share on a minimum job credited the weak work of its parent's weak
    /// target, like a weak share, so each hash is credited once on average.
    /// Not on the design page; a correction this simulator tests.
    WeakWork,
}

/// Share races simulated in the weak share queue (study #85) instead of a
/// flat stale rate, and the weak shares they lose.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShareRaces {
    /// What decides a race.
    pub fork_choice: ForkChoice,
    /// Fraction of weak shares mined on a parent that leaves the chain.
    pub stale_parent_loss: f64,
    /// Whether a race's loser drops the weak shares it carried.
    pub uncle_carrier_drops: bool,
}

impl ShareRaces {
    /// Races with `fork_choice` and no weak shares lost.
    pub fn without_losses(fork_choice: ForkChoice) -> Self {
        Self {
            fork_choice,
            stale_parent_loss: 0.0,
            uncle_carrier_drops: false,
        }
    }
}

/// When a steering miner has a second parent to pick from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SteeringMode {
    /// Only while two shares race: it builds on the one with the better
    /// seed. A race goes either way, so this adds no orphan risk.
    Races,
    /// Always: it builds on the tip or on the tip's parent, whichever seed
    /// is better. A share it finds on the parent is a sibling of the tip
    /// and goes stale.
    TipOrParent,
}

impl SteeringMode {
    /// Name in tables.
    pub fn name(&self) -> &'static str {
        match self {
            SteeringMode::Races => "races only",
            SteeringMode::TipOrParent => "tip or its parent",
        }
    }
}

/// The draw-steering attack: one tracked miner picks parents for a better
/// level draw in the blocks it finds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrawSteering {
    /// Index of the steering miner in the scenario's tracked miners.
    pub tracked_index: usize,
    /// When it can pick.
    pub mode: SteeringMode,
}

/// The stale share rate the network simulation measured at `period`
/// seconds, interpolated linearly between its rows and held at its ends.
pub fn stale_rate_for_period(period: f64) -> f64 {
    let rows = STALE_RATE_BY_PERIOD;
    let last = rows.len() - 1;
    if period <= rows[0].0 {
        rows[0].1
    } else if period >= rows[last].0 {
        rows[last].1
    } else {
        let upper = rows
            .iter()
            .position(|(seconds, _)| *seconds >= period)
            .unwrap_or(last);
        let (low_period, low_rate) = rows[upper - 1];
        let (high_period, high_rate) = rows[upper];
        low_rate + (high_rate - low_rate) * (period - low_period) / (high_period - low_period)
    }
}

/// Every input of one run.
#[derive(Debug, Clone)]
pub struct Scenario {
    /// Label in tables.
    pub label: String,
    /// The design.
    pub design: Design,
    /// Bitcoin Cash's hashrate as a multiple of today's 3.7 EH/s.
    pub hashrate_multiple: f64,
    /// The pool's part of Bitcoin Cash's hashrate, before any spam.
    pub pool_fraction: f64,
    /// How the pool's hashrate is split.
    pub mix: MinerMix,
    /// Share period, seconds.
    pub period_seconds: f64,
    /// Share of found shares that go stale.
    pub stale_rate: f64,
    /// The band cap `C`, fixed for the run.
    pub band_cap: f64,
    /// `WEAK_CAP`.
    pub weak_cap: u64,
    /// `WEAK_FILL`.
    pub weak_fill: u64,
    /// `WEAK_DEPTH`.
    pub weak_depth: usize,
    /// `R_MAX`.
    pub weak_ratio_max: u64,
    /// `INCLUDER_CUT`, as a fraction.
    pub includer_cut: f64,
    /// The payout level of every miner but the tracked ones.
    pub default_level: u32,
    /// Every class of miners is at the payout level a node would leave it at
    /// for its machine's hashrate, instead of all at `default_level`.
    pub node_levels: bool,
    /// The miners whose payments are simulated.
    pub tracked: Vec<TrackedMiner>,
    /// Hashrate below which a miner is on the mini chain (`mini` only).
    pub mini_threshold: f64,
    /// Attack (a): the industrial miners' nodes ignore the node default and
    /// give them the minimum.
    pub big_node_at_minimum: bool,
    /// Attack (b): this fraction of every class's hashrate (the tracked
    /// miners excepted) is on nodes that never carry others' weak shares.
    pub non_carrying_fraction: f64,
    /// Attack (c): one more miner at the minimum with this multiple of the
    /// honest home miners' hashrate, added to the pool.
    pub spam_multiple: f64,
    /// How shares on minimum jobs are credited with weak shares.
    pub weak_credit: WeakCredit,
    /// Share races in the weak share queue, or `None` for the flat stale
    /// rate.
    pub share_races: Option<ShareRaces>,
    /// The draw-steering attack, if run.
    pub draw_steering: Option<DrawSteering>,
    /// Every miner, the tracked ones too, mines at the minimum whatever the
    /// node default would give it.
    pub every_miner_at_minimum: bool,
    /// Months simulated.
    pub months: usize,
    /// Seed of every random draw.
    pub seed: u64,
    /// Chain shares the weak queue is measured over.
    pub weak_queue_shares: usize,
}

impl Default for Scenario {
    fn default() -> Self {
        Self {
            label: String::new(),
            design: Design::BandWeakLevels,
            hashrate_multiple: 1.0,
            pool_fraction: 0.01,
            mix: MinerMix::HalfIndustrial,
            period_seconds: SHARE_INTERVAL_SECONDS,
            stale_rate: stale_rate_for_period(SHARE_INTERVAL_SECONDS),
            band_cap: DEFAULT_BAND_CAP,
            weak_cap: WEAK_CAP,
            weak_fill: WEAK_FILL,
            weak_depth: WEAK_DEPTH,
            weak_ratio_max: WEAK_RATIO_MAX,
            includer_cut: INCLUDER_CUT,
            default_level: DEFAULT_LEVEL,
            node_levels: false,
            tracked: vec![TrackedMiner::bitaxe(), TrackedMiner::asic()],
            mini_threshold: MINI_THRESHOLD_HASHRATE,
            big_node_at_minimum: false,
            non_carrying_fraction: 0.0,
            spam_multiple: 0.0,
            weak_credit: WeakCredit::Page,
            share_races: None,
            draw_steering: None,
            every_miner_at_minimum: false,
            months: 1_000,
            seed: 1,
            weak_queue_shares: 400_000,
        }
    }
}

/// One tracked miner's results.
#[derive(Debug, Clone)]
pub struct MinerReport {
    /// Its name.
    pub name: String,
    /// Its hashrate.
    pub hashrate: f64,
    /// Its fair income a month: its share of Bitcoin Cash's hashrate times
    /// the month's blocks, less the donation.
    pub fair_income_per_month: f64,
    /// Its expected income a month from its expected credit.
    pub expected_income_per_month: f64,
    /// Its simulated mean income a month.
    pub income_per_month: f64,
    /// Blocks a month that paid it an output.
    pub payments_per_month: f64,
    /// Median payment in satoshis, if it was ever paid.
    pub median_payment: Option<f64>,
    /// Standard deviation of monthly income over its mean.
    pub swing: Option<f64>,
    /// Fraction of months with no payment.
    pub months_without_payment: f64,
    /// Payments times the spend cost over income.
    pub fee_loss: Option<f64>,
    /// Shares a month it found, stale ones included.
    pub shares_per_month: f64,
    /// Credited weak shares a month.
    pub weak_shares_per_month: f64,
    /// Its share difficulty over the chain's minimum.
    pub difficulty_multiple: f64,
    /// Part of its expected credit from nephew and includer bonuses.
    pub bonus_fraction: f64,
    /// For the steering miner: satoshis a month steering is expected to
    /// win it in level draws.
    pub steering_gain_per_month: f64,
    /// For the steering miner: satoshis a month it is expected to lose to
    /// shares that go stale on the parent it picked.
    pub steering_orphan_cost_per_month: f64,
    /// For the steering miner: share of its draws in which the other
    /// parent's seed was the better one.
    pub steering_alternative_better: f64,
}

/// One class's expected income, with the tracked miners' classes among
/// them.
#[derive(Debug, Clone)]
pub struct ClassReport {
    /// Its name.
    pub name: String,
    /// What it is.
    pub kind: MinerKind,
    /// Whether its node carries other nodes' weak shares.
    pub on_carrying_node: bool,
    /// Miners in the class.
    pub count: f64,
    /// Hashes a second of one of its miners.
    pub miner_hashrate: f64,
    /// The whole class's hashrate.
    pub hashrate: f64,
    /// The whole class's fair income a month.
    pub fair_income_per_month: f64,
    /// The whole class's expected income a month.
    pub expected_income_per_month: f64,
    /// Satoshis a month of its weak shares' work that goes to the shares
    /// carrying them, at `INCLUDER_CUT`.
    pub includer_paid_per_month: f64,
    /// Satoshis a month its shares receive as includer cuts.
    pub includer_received_per_month: f64,
    /// Shares the whole class finds a second, stale ones included.
    pub found_share_rate: f64,
    /// Credited weak shares a second, the whole class.
    pub weak_rate: f64,
    /// Its found shares that go stale.
    pub stale_rate: f64,
}

/// One chain's steady state.
#[derive(Debug, Clone)]
pub struct ChainReport {
    /// `mini`, `main` or `one`.
    pub name: &'static str,
    /// The chain's part of Bitcoin Cash's hashrate.
    pub hashrate_fraction: f64,
    /// The ASERT (minimum) difficulty.
    pub asert_difficulty: f64,
    /// The easiest share any miner mines.
    pub easiest_difficulty: f64,
    /// Where the band cap rule would settle, for band designs.
    pub settled_band_cap: Option<f64>,
    /// The weak queue, for designs with weak shares.
    pub weak: Option<WeakQueueOutcome>,
    /// The window.
    pub window: WindowGeometry,
    /// Chain shares a second.
    pub chain_share_rate: f64,
    /// Found shares a second, stale ones included.
    pub found_share_rate: f64,
    /// Weak shares found a second.
    pub weak_share_rate: f64,
    /// Weak shares carried a second.
    pub carried_weak_rate: f64,
    /// The race queue's outcome, when share races are simulated.
    pub races: Option<RaceOutcome>,
    /// Share of the chain's hashrate on jobs at the minimum.
    pub minimum_hashrate_fraction: f64,
}

/// What a run gives.
#[derive(Debug, Clone)]
pub struct Report {
    /// The scenario run.
    pub scenario: Scenario,
    /// The pool's part of Bitcoin Cash's hashrate, spam included.
    pub pool_fraction: f64,
    /// Each tracked miner, in the scenario's order.
    pub miners: Vec<MinerReport>,
    /// Each chain.
    pub chains: Vec<ChainReport>,
    /// Every class on every chain, chain by chain.
    pub classes: Vec<ClassReport>,
}

impl Report {
    /// Found shares a second over all chains, stale ones included.
    pub fn shares_per_second(&self) -> f64 {
        self.chains.iter().map(|chain| chain.found_share_rate).sum()
    }

    /// Stale shares (uncles) a second over all chains.
    pub fn uncles_per_second(&self) -> f64 {
        self.shares_per_second() * self.scenario.stale_rate
    }

    /// Weak shares found a second over all chains.
    pub fn weak_shares_per_second(&self) -> f64 {
        self.chains.iter().map(|chain| chain.weak_share_rate).sum()
    }

    /// Bytes a second of share and weak share messages, each counted once.
    pub fn bytes_per_second(&self) -> f64 {
        self.shares_per_second() * SHARE_MESSAGE_BYTES
            + self.weak_shares_per_second() * WEAK_SHARE_MESSAGE_BYTES
    }

    /// The chain the first tracked miner (the Bitaxe by default) is on.
    pub fn home_chain(&self) -> &ChainReport {
        &self.chains[0]
    }
}

/// One class's expected credit on its chain.
#[derive(Debug, Clone, Copy, Default)]
struct ClassCredit {
    found_rate: f64,
    stale_rate: f64,
    chain_weight: f64,
    uncle_weight: f64,
    weak_rate: f64,
    weak_work: f64,
    weak_weight: f64,
    includer_bonus: f64,
    bonus_rate: f64,
    credited_rate: f64,
}

/// A chain being built.
struct Chain {
    name: &'static str,
    classes: Vec<MinerClass>,
    ceiling: f64,
}

impl Scenario {
    /// Bitcoin Cash's hashrate in this scenario.
    pub fn network_hashrate(&self) -> f64 {
        BCH_NETWORK_HASHRATE * self.hashrate_multiple
    }

    /// The pool's classes: the mix, the attacks applied, then the tracked
    /// miners and any spammer.
    pub fn pool_classes(&self) -> Vec<MinerClass> {
        let pool = self.network_hashrate() * self.pool_fraction;
        let tracked: f64 = self.tracked.iter().map(|miner| miner.hashrate).sum();
        let mix = self
            .mix
            .classes((pool - tracked).max(0.0), self.default_level);
        let mut classes = Vec::with_capacity(2 * mix.len() + self.tracked.len() + 1);
        mix.into_iter().for_each(|class| {
            let class = MinerClass {
                follows_node_default: !(self.big_node_at_minimum
                    && class.kind == MinerKind::Industrial),
                ..class
            };
            if self.non_carrying_fraction > 0.0 {
                classes.push(MinerClass {
                    name: format!("{}{NON_CARRYING_SUFFIX}", class.name),
                    count: class.count * self.non_carrying_fraction,
                    on_carrying_node: false,
                    ..class.clone()
                });
                classes.push(MinerClass {
                    count: class.count * (1.0 - self.non_carrying_fraction),
                    ..class
                });
            } else {
                classes.push(class);
            }
        });
        self.tracked
            .iter()
            .for_each(|miner| classes.push(miner.class()));
        if self.spam_multiple > 0.0 {
            let home: f64 = classes
                .iter()
                .filter(|class| class.hashrate < self.mini_threshold)
                .map(MinerClass::total_hashrate)
                .sum();
            classes.push(MinerClass {
                name: "spammer".to_string(),
                kind: MinerKind::Spammer,
                hashrate: home * self.spam_multiple,
                count: 1.0,
                follows_node_default: false,
                on_carrying_node: true,
                level: self.default_level,
            });
        }
        if self.every_miner_at_minimum {
            classes
                .iter_mut()
                .for_each(|class| class.follows_node_default = false);
        }
        if self.node_levels {
            let network = self.network_hashrate();
            let pool_blocks = BCH_BLOCKS_PER_MONTH * self.pool_fraction;
            classes.iter_mut().for_each(|class| {
                class.level = node_level(class.hashrate, network, pool_blocks);
            });
        }
        classes
    }

    /// The chains, the one holding the first tracked miner first.
    fn chains(&self) -> Vec<Chain> {
        let classes = self.pool_classes();
        if self.design == Design::Mini {
            let (mini, main): (Vec<MinerClass>, Vec<MinerClass>) = classes
                .into_iter()
                .partition(|class| class.hashrate < self.mini_threshold);
            let mini = Chain {
                name: "mini",
                classes: mini,
                ceiling: MINI_CEILING_DIFFICULTY,
            };
            let main = Chain {
                name: "main",
                classes: main,
                ceiling: MAIN_CEILING_DIFFICULTY,
            };
            let first_on_main = self
                .tracked
                .first()
                .is_some_and(|miner| miner.hashrate >= self.mini_threshold);
            let ordered = if first_on_main {
                [main, mini]
            } else {
                [mini, main]
            };
            ordered
                .into_iter()
                .filter(|chain| !chain.classes.is_empty())
                .collect()
        } else {
            vec![Chain {
                name: "one",
                classes,
                ceiling: MINI_CEILING_DIFFICULTY,
            }]
        }
    }

    /// Run the scenario.
    pub fn run(&self) -> Report {
        let mut random = SimulationRandom::from_seed(self.seed);
        let mut miners: Vec<Option<MinerReport>> = vec![None; self.tracked.len()];
        let mut chain_reports = Vec::with_capacity(2);
        let mut class_reports = Vec::with_capacity(16);
        let mut pool_hashrate = 0.0;
        self.chains().into_iter().for_each(|chain| {
            let mut chain_random = random.derive();
            let (report, tracked_reports, classes) = self.run_chain(&chain, &mut chain_random);
            pool_hashrate += chain_hashrate(&chain.classes);
            chain_reports.push(report);
            class_reports.extend(classes);
            tracked_reports
                .into_iter()
                .for_each(|(index, miner)| miners[index] = Some(miner));
        });
        Report {
            scenario: self.clone(),
            pool_fraction: pool_hashrate / self.network_hashrate(),
            miners: miners
                .into_iter()
                .map(|miner| miner.expect("every tracked miner is on a chain"))
                .collect(),
            chains: chain_reports,
            classes: class_reports,
        }
    }

    /// Solve, credit and simulate one chain; returns its report, the
    /// reports of the tracked miners on it with their scenario index, and
    /// every class's report.
    fn run_chain(
        &self,
        chain: &Chain,
        random: &mut SimulationRandom,
    ) -> (ChainReport, Vec<(usize, MinerReport)>, Vec<ClassReport>) {
        let network = self.network_hashrate();
        let parameters = ChainParameters {
            period_seconds: self.period_seconds,
            stale_rate: self.stale_rate,
            ceiling_difficulty: chain.ceiling,
            network_hashrate: network,
        };
        let difficulty = solve_chain(&chain.classes, &parameters, self.design.rule(self.band_cap));
        let settled = self
            .design
            .has_band()
            .then(|| settled_band_cap(&chain.classes, &parameters, self.band_cap));
        let mut weak_random = random.derive();
        let (weak, races) = if self.design.has_weak_shares() {
            let (weak, races) = self.weak_queue(chain, &difficulty, &mut weak_random);
            (Some(weak), races)
        } else {
            (None, None)
        };
        let credits = self.credits(chain, &difficulty, weak.as_ref(), races.as_ref());
        let total_credited: f64 = chain
            .classes
            .iter()
            .zip(&credits)
            .map(|(class, credit)| class.count * credit.credited_rate)
            .sum();
        let window = size_window(
            total_credited,
            difficulty.chain_share_rate,
            network,
            self.period_seconds,
        );
        let hashrate = chain_hashrate(&chain.classes);
        let block_rate = hashrate / network / BCH_BLOCK_INTERVAL_SECONDS;
        let income_per_credit =
            block_rate * MONTH_SECONDS * BLOCK_REWARD_SATOSHIS * (1.0 - DONATION_FRACTION)
                / total_credited;
        let tracked_classes: Vec<(usize, usize)> = chain
            .classes
            .iter()
            .enumerate()
            .filter(|(_, class)| class.kind == MinerKind::Tracked)
            .filter_map(|(class_index, class)| {
                self.tracked
                    .iter()
                    .position(|miner| miner.name == class.name)
                    .map(|miner_index| (class_index, miner_index))
            })
            .collect();
        let tracked_credit: f64 = tracked_classes
            .iter()
            .map(|(class_index, _)| credits[*class_index].credited_rate)
            .sum();
        let level_pots = self.design.has_levels().then(|| {
            let mut pot_random = random.derive();
            self.level_pots(chain, &credits, &window, &mut pot_random)
        });
        let payout_inputs = ChainPayoutInputs {
            block_rate,
            window,
            others_credited_rate: total_credited - tracked_credit,
            tracked: tracked_classes
                .iter()
                .map(|(class_index, _)| {
                    let credit = credits[*class_index];
                    TrackedInputs {
                        found_rate: credit.found_rate,
                        stale_rate: credit.stale_rate,
                        chain_weight: credit.chain_weight,
                        uncle_weight: credit.uncle_weight,
                        weak_rate: credit.weak_rate,
                        weak_weight: credit.weak_weight,
                        level: chain.classes[*class_index].level,
                    }
                })
                .collect(),
            level_pots,
            months: self.months,
            steering: self.steering(chain, &tracked_classes),
        };
        let mut payout_random = random.derive();
        let outcomes = simulate_chain_payouts(&payout_inputs, &mut payout_random);
        let tracked_reports = tracked_classes
            .iter()
            .zip(&outcomes)
            .map(|((class_index, miner_index), outcome)| {
                let class = &chain.classes[*class_index];
                let credit = credits[*class_index];
                let expected = block_rate
                    * MONTH_SECONDS
                    * BLOCK_REWARD_SATOSHIS
                    * (1.0 - DONATION_FRACTION)
                    * credit.credited_rate
                    / total_credited;
                let summary = summarise_outcome(class, outcome, self.months);
                let report = MinerReport {
                    name: class.name.clone(),
                    hashrate: class.hashrate,
                    fair_income_per_month: fair_income_per_month(class.hashrate, network),
                    expected_income_per_month: expected,
                    difficulty_multiple: difficulty.class_difficulty[*class_index]
                        / difficulty.asert_difficulty,
                    bonus_fraction: credit.bonus_rate / credit.credited_rate,
                    steering_orphan_cost_per_month: self.steering_orphan_cost(
                        *miner_index,
                        &credit,
                        summary.steering_alternative_better,
                        income_per_credit,
                    ),
                    ..summary
                };
                (*miner_index, report)
            })
            .collect();
        let class_reports = chain
            .classes
            .iter()
            .zip(&credits)
            .map(|(class, credit)| self.class_report(class, credit, income_per_credit))
            .collect();
        let found_share_rate: f64 = chain
            .classes
            .iter()
            .zip(&credits)
            .map(|(class, credit)| class.count * credit.found_rate)
            .sum();
        let weak_share_rate = weak.map_or(0.0, |outcome| {
            outcome.arrivals_per_share * difficulty.chain_share_rate
        });
        let carried_weak_rate = weak.map_or(0.0, |outcome| {
            outcome.carried_per_share * difficulty.chain_share_rate
        });
        let minimum_hashrate: f64 = chain
            .classes
            .iter()
            .zip(&difficulty.class_at_minimum)
            .filter(|(_, at_minimum)| **at_minimum)
            .map(|(class, _)| class.total_hashrate())
            .sum();
        let report = ChainReport {
            name: chain.name,
            hashrate_fraction: hashrate / network,
            asert_difficulty: difficulty.asert_difficulty,
            easiest_difficulty: difficulty.easiest_difficulty(),
            settled_band_cap: settled,
            weak,
            window,
            chain_share_rate: difficulty.chain_share_rate,
            found_share_rate,
            weak_share_rate,
            carried_weak_rate,
            races,
            minimum_hashrate_fraction: minimum_hashrate / hashrate,
        };
        (report, tracked_reports, class_reports)
    }

    /// The steering miner's part in the payout simulation, when it is on
    /// this chain. `tracked_classes` pairs each tracked class's index on
    /// the chain with its index in the scenario's tracked miners.
    fn steering(&self, chain: &Chain, tracked_classes: &[(usize, usize)]) -> Option<Steering> {
        self.draw_steering.and_then(|steering| {
            tracked_classes
                .iter()
                .position(|(_, miner_index)| *miner_index == steering.tracked_index)
                .map(|payout_index| {
                    let class = &chain.classes[tracked_classes[payout_index].0];
                    Steering {
                        tracked_index: payout_index,
                        own_block_chance: class.total_hashrate() / chain_hashrate(&chain.classes),
                        choice_chance: match steering.mode {
                            SteeringMode::Races => races_per_share(self.stale_rate),
                            SteeringMode::TipOrParent => 1.0,
                        },
                    }
                })
        })
    }

    /// What the steering miner loses a month to stale shares when it mines
    /// on the tip's parent: in that share of the time its shares would
    /// have joined the chain go stale, paid as uncles. Zero for any other
    /// miner and when it picks only between racing shares.
    fn steering_orphan_cost(
        &self,
        miner_index: usize,
        credit: &ClassCredit,
        alternative_better: f64,
        income_per_credit: f64,
    ) -> f64 {
        match self.draw_steering {
            Some(steering)
                if steering.tracked_index == miner_index
                    && steering.mode == SteeringMode::TipOrParent =>
            {
                alternative_better
                    * credit.found_rate
                    * (1.0 - credit.stale_rate)
                    * (credit.chain_weight - credit.uncle_weight)
                    * income_per_credit
            }
            _ => 0.0,
        }
    }

    /// One class's expected income and includer cuts, all its miners
    /// together.
    fn class_report(
        &self,
        class: &MinerClass,
        credit: &ClassCredit,
        income_per_credit: f64,
    ) -> ClassReport {
        let network = self.network_hashrate();
        ClassReport {
            name: class.name.clone(),
            kind: class.kind,
            on_carrying_node: class.on_carrying_node,
            count: class.count,
            miner_hashrate: class.hashrate,
            hashrate: class.total_hashrate(),
            fair_income_per_month: fair_income_per_month(class.total_hashrate(), network),
            expected_income_per_month: class.count * credit.credited_rate * income_per_credit,
            includer_paid_per_month: class.count
                * credit.weak_rate
                * credit.weak_work
                * self.includer_cut
                * income_per_credit,
            includer_received_per_month: class.count
                * credit.found_rate
                * (1.0 - credit.stale_rate)
                * credit.includer_bonus
                * income_per_credit,
            found_share_rate: class.count * credit.found_rate,
            weak_rate: class.count * credit.weak_rate,
            stale_rate: credit.stale_rate,
        }
    }

    /// The weak share queue's inputs for a chain in its steady state.
    fn weak_queue_inputs(&self, chain: &Chain, difficulty: &ChainDifficulty) -> WeakQueueInputs {
        let minimum_shares_per_period = |on_carrying_node: bool| -> f64 {
            chain
                .classes
                .iter()
                .zip(&difficulty.class_difficulty)
                .zip(&difficulty.class_at_minimum)
                .filter(|((class, _), at_minimum)| {
                    **at_minimum && class.on_carrying_node == on_carrying_node
                })
                .map(|((class, class_difficulty), _)| {
                    class.count * found_share_rate(class, *class_difficulty)
                })
                .sum::<f64>()
                * self.period_seconds
        };
        let carrying_chain_rate: f64 = chain
            .classes
            .iter()
            .zip(&difficulty.class_difficulty)
            .filter(|(class, _)| class.on_carrying_node)
            .map(|(class, class_difficulty)| {
                class.count * found_share_rate(class, *class_difficulty) * (1.0 - self.stale_rate)
            })
            .sum();
        WeakQueueInputs {
            carrying_minimum_shares_per_period: minimum_shares_per_period(true),
            non_carrying_minimum_shares_per_period: minimum_shares_per_period(false),
            carrying_share_fraction: carrying_chain_rate / difficulty.chain_share_rate,
            weak_cap: self.weak_cap,
            weak_fill: self.weak_fill,
            weak_depth: self.weak_depth,
            ratio_max: self.weak_ratio_max,
            ceiling_ratio: difficulty.asert_difficulty / chain.ceiling,
            shares: self.weak_queue_shares,
            warmup_shares: 20_000,
        }
    }

    /// Run the weak share queue of a chain in its steady state, with share
    /// races when the scenario simulates them.
    fn weak_queue(
        &self,
        chain: &Chain,
        difficulty: &ChainDifficulty,
        random: &mut SimulationRandom,
    ) -> (WeakQueueOutcome, Option<RaceOutcome>) {
        let inputs = self.weak_queue_inputs(chain, difficulty);
        match self.share_races {
            None => (simulate_weak_queue(&inputs, random), None),
            Some(share_races) => {
                let race_inputs = RaceQueueInputs {
                    queue: inputs,
                    racers: racers(chain, difficulty),
                    settings: RaceSettings {
                        fork_choice: share_races.fork_choice,
                        races_per_share: races_per_share(self.stale_rate),
                        stale_parent_loss: share_races.stale_parent_loss,
                        uncle_carrier_drops: share_races.uncle_carrier_drops,
                    },
                };
                let outcome = simulate_race_queue(&race_inputs, random);
                (outcome.queue, Some(outcome))
            }
        }
    }

    /// Every class's expected credit: what a chain share, an uncle and a
    /// weak share of one of its miners are credited, and the rates.
    ///
    /// With share races simulated (`races`), each class's stale rate, the
    /// weak shares each kind of node's shares carry and the weak shares lost
    /// come from the race queue instead of the flat stale rate.
    fn credits(
        &self,
        chain: &Chain,
        difficulty: &ChainDifficulty,
        weak: Option<&WeakQueueOutcome>,
        races: Option<&RaceOutcome>,
    ) -> Vec<ClassCredit> {
        let stale = self.stale_rate;
        let ratio = weak.map_or(1.0, |outcome| outcome.mean_ratio);
        let weak_work = difficulty.asert_difficulty * HASHES_PER_DIFFICULTY / ratio;
        let base_weights: Vec<f64> = chain
            .classes
            .iter()
            .zip(&difficulty.class_difficulty)
            .zip(&difficulty.class_at_minimum)
            .map(|((_, class_difficulty), at_minimum)| {
                if weak.is_some() && *at_minimum && self.weak_credit == WeakCredit::WeakWork {
                    weak_work
                } else {
                    class_difficulty * HASHES_PER_DIFFICULTY
                }
            })
            .collect();
        let found_rates: Vec<f64> = chain
            .classes
            .iter()
            .zip(&difficulty.class_difficulty)
            .map(|(class, class_difficulty)| found_share_rate(class, *class_difficulty))
            .collect();
        let stale_rates: Vec<f64> = match races {
            None => vec![stale; chain.classes.len()],
            Some(outcome) => outcome.stale_fraction.clone(),
        };
        let nephew_bonus = match races {
            None => {
                let found_total: f64 = chain
                    .classes
                    .iter()
                    .zip(&found_rates)
                    .map(|(class, rate)| class.count * rate)
                    .sum();
                let found_work: f64 = chain
                    .classes
                    .iter()
                    .zip(&found_rates)
                    .zip(&base_weights)
                    .map(|((class, rate), weight)| class.count * rate * weight)
                    .sum();
                let average_uncle_weight = found_work / found_total;
                (1.0 - UNCLE_PAID_FRACTION) * stale / (1.0 - stale) * average_uncle_weight
            }
            Some(_) => race_nephew_bonus(&chain.classes, &found_rates, &base_weights, &stale_rates),
        };
        let (carried_by_carrying, carried_by_non_carrying) = match races {
            None => weak.map_or((0.0, 0.0), |outcome| {
                split_carried(outcome, chain, difficulty, stale)
            }),
            Some(outcome) => (
                outcome.queue.carried_per_carrying_share,
                outcome.carried_per_non_carrying_share,
            ),
        };
        chain
            .classes
            .iter()
            .zip(&difficulty.class_at_minimum)
            .zip(found_rates.iter().zip(&base_weights))
            .zip(&stale_rates)
            .map(
                |(((class, at_minimum), (found_rate, base_weight)), stale)| {
                    let carried = if class.on_carrying_node {
                        carried_by_carrying
                    } else {
                        carried_by_non_carrying
                    };
                    let includer_bonus = self.includer_cut * carried * weak_work;
                    let bonus = nephew_bonus + includer_bonus;
                    let weak_found_rate = match weak {
                        Some(_) if *at_minimum => (ratio - 1.0) * found_rate,
                        _ => 0.0,
                    };
                    let expired = weak.map_or(0.0, |outcome| {
                        if class.on_carrying_node {
                            outcome.carrying_expired_fraction
                        } else {
                            outcome.non_carrying_expired_fraction
                        }
                    });
                    let weak_rate = match races {
                        None => weak_found_rate * (1.0 - expired),
                        Some(outcome) => {
                            let lost = if class.on_carrying_node {
                                outcome.carrying_lost_fraction
                            } else {
                                outcome.non_carrying_lost_fraction
                            };
                            weak_found_rate * (1.0 - expired - lost)
                        }
                    };
                    let weak_weight = weak_work * (1.0 - self.includer_cut);
                    let chain_weight = base_weight + bonus;
                    let uncle_weight = UNCLE_PAID_FRACTION * base_weight;
                    let bonus_rate = found_rate * (1.0 - stale) * bonus;
                    let credited_rate = found_rate
                        * ((1.0 - stale) * chain_weight + stale * uncle_weight)
                        + weak_rate * weak_weight;
                    ClassCredit {
                        found_rate: *found_rate,
                        stale_rate: *stale,
                        chain_weight,
                        uncle_weight,
                        weak_rate,
                        weak_work,
                        weak_weight,
                        includer_bonus,
                        bonus_rate,
                        credited_rate,
                    }
                },
            )
            .collect()
    }

    /// The other payees' satoshis below each level in a block, by level.
    ///
    /// For each class at a level, its miners' amounts in a block are drawn
    /// from their entries in the window (Poisson counts of chain shares,
    /// uncles and weak shares), and the mean of the amounts that are below
    /// the level, times the class's miners, is its part of the group's pot.
    fn level_pots(
        &self,
        chain: &Chain,
        credits: &[ClassCredit],
        window: &WindowGeometry,
        random: &mut SimulationRandom,
    ) -> Vec<u64> {
        let reward = BLOCK_REWARD_SATOSHIS * (1.0 - DONATION_FRACTION);
        let span = window.span_seconds;
        let mut pots = vec![0.0; LEVEL_COUNT as usize];
        chain
            .classes
            .iter()
            .zip(credits)
            .filter(|(class, _)| class.kind != MinerKind::Tracked)
            .for_each(|(class, credit)| {
                let threshold = level_amount(class.level) as f64;
                let chain_mean = credit.found_rate * (1.0 - credit.stale_rate) * span;
                let uncle_mean = credit.found_rate * credit.stale_rate * span;
                let weak_mean = credit.weak_rate * span;
                let below: f64 = (0..LEVEL_POT_SAMPLES)
                    .map(|_| {
                        let weight = poisson(random, chain_mean) as f64 * credit.chain_weight
                            + poisson(random, uncle_mean) as f64 * credit.uncle_weight
                            + poisson(random, weak_mean) as f64 * credit.weak_weight;
                        let amount = (reward * weight / window.work).floor();
                        if amount < threshold { amount } else { 0.0 }
                    })
                    .sum();
                pots[class.level as usize] += class.count * below / LEVEL_POT_SAMPLES as f64;
            });
        pots.into_iter().map(|pot| pot.round() as u64).collect()
    }
}

/// Races per chain share at stale rate `stale`: each race makes one stale
/// share, and `stale / (1 - stale)` of them are found per chain share. At
/// most one.
pub fn races_per_share(stale: f64) -> f64 {
    (stale / (1.0 - stale)).min(1.0)
}

/// The racers of the race queue: each class's found shares, its node and
/// its share's work over the minimum, in class order.
fn racers(chain: &Chain, difficulty: &ChainDifficulty) -> Vec<Racer> {
    chain
        .classes
        .iter()
        .zip(&difficulty.class_difficulty)
        .map(|(class, class_difficulty)| Racer {
            found_rate: class.count * found_share_rate(class, *class_difficulty),
            on_carrying_node: class.on_carrying_node,
            work_multiple: class_difficulty / difficulty.asert_difficulty,
        })
        .collect()
}

/// The bonus a chain share gets for naming uncles when each class goes
/// stale at its own rate: the nephew's tenth of the uncles' base work,
/// uncles per chain share times their mean base work.
fn race_nephew_bonus(
    classes: &[MinerClass],
    found_rates: &[f64],
    base_weights: &[f64],
    stale_rates: &[f64],
) -> f64 {
    let mut stale_work = 0.0;
    let mut chain_shares = 0.0;
    classes
        .iter()
        .zip(found_rates)
        .zip(base_weights)
        .zip(stale_rates)
        .for_each(|(((class, found_rate), base_weight), stale)| {
            stale_work += class.count * found_rate * stale * base_weight;
            chain_shares += class.count * found_rate * (1.0 - stale);
        });
    if chain_shares > 0.0 {
        (1.0 - UNCLE_PAID_FRACTION) * stale_work / chain_shares
    } else {
        0.0
    }
}

/// Weak shares a carrying node's share and a non-carrying node's share each
/// carry on average.
fn split_carried(
    outcome: &WeakQueueOutcome,
    chain: &Chain,
    difficulty: &ChainDifficulty,
    stale: f64,
) -> (f64, f64) {
    let carrying_rate: f64 = chain
        .classes
        .iter()
        .zip(&difficulty.class_difficulty)
        .filter(|(class, _)| class.on_carrying_node)
        .map(|(class, class_difficulty)| {
            class.count * found_share_rate(class, *class_difficulty) * (1.0 - stale)
        })
        .sum();
    let carrying_fraction = carrying_rate / difficulty.chain_share_rate;
    let by_carrying = outcome.carried_per_carrying_share;
    let by_non_carrying = if carrying_fraction < 1.0 {
        ((outcome.carried_per_share - carrying_fraction * by_carrying) / (1.0 - carrying_fraction))
            .max(0.0)
    } else {
        0.0
    };
    (by_carrying, by_non_carrying)
}

/// A miner's fair income a month: its share of Bitcoin Cash's hashrate of
/// the month's blocks, less the donation.
pub fn fair_income_per_month(hashrate: f64, network_hashrate: f64) -> f64 {
    hashrate / network_hashrate
        * BCH_BLOCKS_PER_MONTH
        * BLOCK_REWARD_SATOSHIS
        * (1.0 - DONATION_FRACTION)
}

/// The monthly statistics of one tracked miner's simulated payments. The
/// fields that need the chain (fair and expected income, difficulty and
/// bonus) are left for the caller.
fn summarise_outcome(class: &MinerClass, outcome: &TrackedOutcome, months: usize) -> MinerReport {
    let month_count = months as f64;
    let total_income: f64 = outcome.monthly_income.iter().sum();
    let income_per_month = total_income / month_count;
    let payment_count = outcome.payments.len() as f64;
    let swing = (income_per_month > 0.0).then(|| {
        let variance = outcome
            .monthly_income
            .iter()
            .map(|income| (income - income_per_month).powi(2))
            .sum::<f64>()
            / month_count;
        variance.sqrt() / income_per_month
    });
    let empty_months = outcome
        .monthly_payments
        .iter()
        .filter(|payments| **payments == 0)
        .count() as f64;
    MinerReport {
        name: class.name.clone(),
        hashrate: class.hashrate,
        fair_income_per_month: 0.0,
        expected_income_per_month: 0.0,
        income_per_month,
        payments_per_month: payment_count / month_count,
        median_payment: median(&outcome.payments),
        swing,
        months_without_payment: empty_months / month_count,
        fee_loss: (total_income > 0.0).then(|| payment_count * SPEND_COST_SATOSHIS / total_income),
        shares_per_month: outcome.found_shares as f64 / month_count,
        weak_shares_per_month: outcome.weak_shares as f64 / month_count,
        difficulty_multiple: 1.0,
        bonus_fraction: 0.0,
        steering_gain_per_month: outcome.steering_gain / month_count,
        steering_orphan_cost_per_month: 0.0,
        steering_alternative_better: if outcome.steering_draws > 0 {
            outcome.steering_alternative_better as f64 / outcome.steering_draws as f64
        } else {
            0.0
        },
    }
}

/// The median of `values`, if there are any.
fn median(values: &[u64]) -> Option<f64> {
    if values.is_empty() {
        None
    } else {
        let mut sorted = values.to_vec();
        sorted.sort_unstable();
        let middle = sorted.len() / 2;
        if sorted.len().is_multiple_of(2) {
            Some((sorted[middle - 1] + sorted[middle]) as f64 / 2.0)
        } else {
            Some(sorted[middle] as f64)
        }
    }
}

#[cfg(test)]
mod tests;
