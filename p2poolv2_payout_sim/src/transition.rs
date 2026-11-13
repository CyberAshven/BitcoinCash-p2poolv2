// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A big miner joining or leaving at once (study #85), share by share.
//!
//! The steady-state model fixes the minimum, the band cap `C` and the weak
//! ratio `R`. Here they move as the design page's rules move them, one
//! share at a time: ASERT anchored at the parent with its 600-second
//! half-life, `band_cap` from the parent's `band_cap` and `wanted_bits`
//! (section 2, "The cap `C`"), and `weak_ratio` from the parent's
//! `weak_ratio` and carried count (section 3, "The weak target"). Every node
//! carries every weak share and every proof on a job at the minimum is
//! credited at the weak work, as the page now has it.
//!
//! Each node's estimates of the pool's and its miners' hashrates, which the
//! node default reads over ten minutes, are taken as exact. Stale shares
//! only lower the chain's share rate by the stale fraction; they are not
//! raced. Home miners' pay is their expected share of the window of each
//! moment's blocks, with blocks at the pool's hashrate, so the only noise is
//! the chain's own.

use crate::constants::{
    ASERT_HALF_LIFE_SECONDS, BCH_BLOCK_INTERVAL_SECONDS, BCH_PPLNS_WINDOW_BLOCKS,
    BLOCK_REWARD_SATOSHIS, DONATION_FRACTION, FIXED_POINT_ONE, HASHES_PER_DIFFICULTY,
    MATURITY_SKIP_SHARES, WINDOW_CAP_SECONDS,
};
use crate::difficulty::{found_share_rate, node_default_shares_per_month, wanted_ratio};
use crate::miners::{MinerClass, MinerKind};
use crate::sampling::poisson;
use crate::weak::next_weak_ratio;
use p2poolv2_netsim::random::SimulationRandom;

/// Seconds in an hour, the length of each sample.
const HOUR_SECONDS: f64 = 3_600.0;

/// Hours sampled before the event.
pub const HOURS_BEFORE_EVENT: usize = 24;

/// The next share's band cap, in units of 1/1024, from its parent's
/// (design page, section 2, "The cap `C`"). It rises by `C * u / divisor` when
/// the parent's node wanted heavier shares than its cap allowed,
/// `parent_wanted` (the wanted work over the parent's minimum) above
/// `C`, with `u` that multiple as a whole number from 1 to `cap_max`; it
/// falls by `C / divisor` otherwise; and it stays between 1 and `cap_max`. The
/// page's `divisor` is 512.
pub fn next_band_cap(parent_cap: u64, parent_wanted: f64, cap_max: u64, divisor: u64) -> u64 {
    let wanted_more = parent_wanted * FIXED_POINT_ONE as f64 > parent_cap as f64;
    let moved = if wanted_more {
        let multiple = (parent_wanted.floor() as u64).clamp(1, cap_max);
        parent_cap + parent_cap * multiple / divisor
    } else {
        parent_cap - parent_cap / divisor
    };
    moved.clamp(FIXED_POINT_ONE, cap_max * FIXED_POINT_ONE)
}

/// The next share's minimum difficulty, ASERT anchored at the parent:
/// the parent's minimum times `2^(-(interval - period) / half_life)`, with
/// `interval` the time from the parent's own parent to the parent, never
/// easier than `ceiling`. In floating point; the node uses Bitcoin Cash's
/// fixed-point exponent.
pub fn next_minimum(parent_minimum: f64, parent_interval: f64, period: f64, ceiling: f64) -> f64 {
    let exponent = -(parent_interval - period) / ASERT_HALF_LIFE_SECONDS;
    (parent_minimum * exponent.exp2()).max(ceiling)
}

/// What a transition run depends on.
#[derive(Debug, Clone)]
pub struct TransitionInputs {
    /// Every class that mines at some time, the big miner among them.
    pub classes: Vec<MinerClass>,
    /// Index of the big miner in `classes`.
    pub big_miner: usize,
    /// Whether the big miner leaves at the event; otherwise it joins.
    pub big_miner_leaves: bool,
    /// Bitcoin Cash's hashrate.
    pub network_hashrate: f64,
    /// Target seconds between chain shares.
    pub period_seconds: f64,
    /// Share of found shares that go stale.
    pub stale_rate: f64,
    /// The chain's easiest share difficulty.
    pub ceiling_difficulty: f64,
    /// `C_MAX`.
    pub cap_max: u64,
    /// The band cap's step divisor: 512 on the design page.
    pub band_step_divisor: u64,
    /// A class that mines at the minimum and records the largest wanted
    /// difficulty in every share it finds, to push the cap up.
    pub overstating: Option<usize>,
    /// `WEAK_CAP`.
    pub weak_cap: u64,
    /// `WEAK_FILL`.
    pub weak_fill: u64,
    /// `WEAK_DEPTH`.
    pub weak_depth: usize,
    /// `R_MAX`.
    pub ratio_max: u64,
    /// `INCLUDER_CUT`.
    pub includer_cut: f64,
    /// Seconds from genesis to the event.
    pub event_seconds: f64,
    /// Seconds simulated after the event.
    pub after_seconds: f64,
}

/// One hour's measures.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct HourSample {
    /// Seconds of the hour simulated.
    pub seconds: f64,
    /// Seconds times the minimum difficulty in force.
    pub minimum_seconds: f64,
    /// Seconds times the band cap in force.
    pub band_cap_seconds: f64,
    /// Seconds times the weak ratio in force, the ceiling applied.
    pub weak_ratio_seconds: f64,
    /// Home miners' expected income, satoshis.
    pub home_income: f64,
    /// Home miners' fair income, satoshis.
    pub home_fair_income: f64,
    /// Weak shares found.
    pub weak_found: f64,
    /// Weak shares that expired uncarried.
    pub weak_expired: f64,
}

impl HourSample {
    /// Add another sample's sums to this one's.
    pub fn add(&mut self, other: &HourSample) {
        self.seconds += other.seconds;
        self.minimum_seconds += other.minimum_seconds;
        self.band_cap_seconds += other.band_cap_seconds;
        self.weak_ratio_seconds += other.weak_ratio_seconds;
        self.home_income += other.home_income;
        self.home_fair_income += other.home_fair_income;
        self.weak_found += other.weak_found;
        self.weak_expired += other.weak_expired;
    }

    /// Time-weighted mean minimum difficulty.
    pub fn minimum(&self) -> f64 {
        self.minimum_seconds / self.seconds
    }

    /// Time-weighted mean band cap.
    pub fn band_cap(&self) -> f64 {
        self.band_cap_seconds / self.seconds
    }

    /// Time-weighted mean weak ratio.
    pub fn weak_ratio(&self) -> f64 {
        self.weak_ratio_seconds / self.seconds
    }
}

/// Weak shares waiting to be carried, by the depth of their parent, with
/// their credited work and the home miners' part of it.
#[derive(Debug, Clone)]
struct WeakSlots {
    count: Vec<u64>,
    work: Vec<f64>,
    home_work: Vec<f64>,
}

/// Weak shares taken or added at once: how many, their work, the home
/// miners' part.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct WeakBatch {
    count: u64,
    work: f64,
    home_work: f64,
}

impl WeakSlots {
    fn new(depth: usize) -> Self {
        Self {
            count: vec![0; depth],
            work: vec![0.0; depth],
            home_work: vec![0.0; depth],
        }
    }

    /// A new share: every waiting weak share is one share deeper, those at
    /// the deepest expire (returned), and `found` joins at depth one.
    fn deepen(&mut self, found: WeakBatch) -> u64 {
        self.count.rotate_right(1);
        self.work.rotate_right(1);
        self.home_work.rotate_right(1);
        let expired = self.count[0];
        self.count[0] = found.count;
        self.work[0] = found.work;
        self.home_work[0] = found.home_work;
        expired
    }

    /// Take up to `cap` weak shares, oldest parent first.
    fn carry(&mut self, cap: u64) -> WeakBatch {
        let mut carried = WeakBatch::default();
        (0..self.count.len()).rev().for_each(|depth| {
            let waiting = self.count[depth];
            let taken = (cap - carried.count).min(waiting);
            if taken > 0 {
                let part = taken as f64 / waiting as f64;
                let work = self.work[depth] * part;
                let home_work = self.home_work[depth] * part;
                self.count[depth] -= taken;
                self.work[depth] -= work;
                self.home_work[depth] -= home_work;
                carried.count += taken;
                carried.work += work;
                carried.home_work += home_work;
            }
        });
        carried
    }
}

/// The chain's credited work, share by share, as running totals.
struct CreditLedger {
    total: Vec<f64>,
    home: Vec<f64>,
}

impl CreditLedger {
    fn with_capacity(shares: usize) -> Self {
        let mut total = Vec::with_capacity(shares + 1);
        let mut home = Vec::with_capacity(shares + 1);
        total.push(0.0);
        home.push(0.0);
        Self { total, home }
    }

    fn push(&mut self, total: f64, home: f64) {
        let last_total = self.total[self.total.len() - 1];
        let last_home = self.home[self.home.len() - 1];
        self.total.push(last_total + total);
        self.home.push(last_home + home);
    }

    /// Home miners' part of the window a block found on the current tip
    /// pays: skip the newest `skip` shares, then take shares back until
    /// their credited work reaches `threshold`, at most `cap` shares.
    fn home_fraction(&self, skip: usize, threshold: f64, cap: usize) -> f64 {
        let shares = self.total.len() - 1;
        let end = shares.saturating_sub(skip);
        let end_total = self.total[end];
        let reach = self.total[..=end].partition_point(|running| *running <= end_total - threshold);
        let start = reach.saturating_sub(1).max(end.saturating_sub(cap));
        let work = end_total - self.total[start];
        if work > 0.0 {
            (self.home[end] - self.home[start]) / work
        } else {
            0.0
        }
    }
}

/// The tip the next share builds on.
#[derive(Debug, Clone, Copy)]
struct Tip {
    minimum: f64,
    interval: f64,
    band_cap: u64,
    wanted: f64,
    weak_ratio: u64,
    carried: u64,
}

/// What every present class mines at for the next share.
struct Jobs {
    pool_hashrate: f64,
    home_hashrate: f64,
    minimum: f64,
    band_cap: u64,
    found_rates: Vec<f64>,
    multiples: Vec<f64>,
    wanted: Vec<f64>,
    minimum_rate: f64,
    home_minimum_rate: f64,
}

/// One run's state.
struct Transition<'a> {
    inputs: &'a TransitionInputs,
    time: f64,
    big_present: bool,
    tip: Tip,
    tip_time: f64,
    slots: WeakSlots,
    found_on_tip: WeakBatch,
    ledger: CreditLedger,
    hours: Vec<HourSample>,
}

impl<'a> Transition<'a> {
    fn new(inputs: &'a TransitionInputs) -> Self {
        let big_present = inputs.big_miner_leaves;
        let present_hashrate: f64 = inputs
            .classes
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != inputs.big_miner || big_present)
            .map(|(_, class)| class.total_hashrate())
            .sum();
        let uniform = present_hashrate * (1.0 - inputs.stale_rate) * inputs.period_seconds
            / HASHES_PER_DIFFICULTY;
        let expected_shares =
            ((inputs.event_seconds + inputs.after_seconds) / inputs.period_seconds) as usize;
        let hour_count = HOURS_BEFORE_EVENT + (inputs.after_seconds / HOUR_SECONDS).ceil() as usize;
        Self {
            inputs,
            time: 0.0,
            big_present,
            tip_time: 0.0,
            tip: Tip {
                minimum: uniform.max(inputs.ceiling_difficulty),
                interval: inputs.period_seconds,
                band_cap: FIXED_POINT_ONE,
                wanted: 1.0,
                weak_ratio: inputs.ratio_max * FIXED_POINT_ONE,
                carried: 0,
            },
            slots: WeakSlots::new(inputs.weak_depth),
            found_on_tip: WeakBatch::default(),
            ledger: CreditLedger::with_capacity(expected_shares + expected_shares / 4),
            hours: vec![HourSample::default(); hour_count],
        }
    }

    fn present(&self, index: usize) -> bool {
        index != self.inputs.big_miner || self.big_present
    }

    /// The jobs for the share after the tip: its minimum and cap from the
    /// tip, each present class's difficulty by the node default inside the
    /// cap, and the rates they give.
    fn jobs(&self) -> Jobs {
        let inputs = self.inputs;
        let present: Vec<&MinerClass> = inputs
            .classes
            .iter()
            .enumerate()
            .filter(|(index, _)| self.present(*index))
            .map(|(_, class)| class)
            .collect();
        let pool_hashrate: f64 = present.iter().map(|class| class.total_hashrate()).sum();
        let home_hashrate: f64 = present
            .iter()
            .filter(|class| class.kind == MinerKind::Home)
            .map(|class| class.total_hashrate())
            .sum();
        let target = node_default_shares_per_month(pool_hashrate, inputs.network_hashrate);
        let minimum = next_minimum(
            self.tip.minimum,
            self.tip.interval,
            inputs.period_seconds,
            inputs.ceiling_difficulty,
        );
        let band_cap = next_band_cap(
            self.tip.band_cap,
            self.tip.wanted,
            inputs.cap_max,
            inputs.band_step_divisor,
        );
        let cap = band_cap as f64 / FIXED_POINT_ONE as f64;
        let mut jobs = Jobs {
            pool_hashrate,
            home_hashrate,
            minimum,
            band_cap,
            found_rates: Vec::with_capacity(inputs.classes.len()),
            multiples: Vec::with_capacity(inputs.classes.len()),
            wanted: Vec::with_capacity(inputs.classes.len()),
            minimum_rate: 0.0,
            home_minimum_rate: 0.0,
        };
        inputs
            .classes
            .iter()
            .enumerate()
            .for_each(|(index, class)| {
                let overstates = inputs.overstating == Some(index);
                let wanted = if overstates {
                    inputs.cap_max as f64
                } else {
                    wanted_ratio(class, minimum, target)
                };
                let multiple = if overstates {
                    1.0
                } else {
                    wanted.min(cap).max(1.0)
                };
                let rate = if self.present(index) {
                    class.count * found_share_rate(class, multiple * minimum)
                } else {
                    0.0
                };
                if multiple <= 1.0 {
                    jobs.minimum_rate += rate;
                    if class.kind == MinerKind::Home {
                        jobs.home_minimum_rate += rate;
                    }
                }
                jobs.found_rates.push(rate);
                jobs.multiples.push(multiple);
                jobs.wanted.push(wanted);
            });
        jobs
    }

    /// The tip's weak ratio in force for the jobs, the ceiling applied.
    fn effective_ratio(&self, minimum: f64) -> f64 {
        (self.tip.weak_ratio as f64 / FIXED_POINT_ONE as f64)
            .min(minimum / self.inputs.ceiling_difficulty)
            .max(1.0)
    }

    /// The hour sample `time` falls in, if it is sampled.
    fn hour_index(&self, time: f64) -> Option<usize> {
        let since = time - self.inputs.event_seconds + HOURS_BEFORE_EVENT as f64 * HOUR_SECONDS;
        if since >= 0.0 {
            let index = (since / HOUR_SECONDS) as usize;
            (index < self.hours.len()).then_some(index)
        } else {
            None
        }
    }

    /// Record `seconds` of mining on the tip: the means, the home miners'
    /// expected and fair pay from the blocks found meanwhile, and the weak
    /// shares found.
    fn record(&mut self, seconds: f64, jobs: &Jobs, ratio: f64, weak_found: u64) {
        if let Some(index) = self.hour_index(self.time) {
            let inputs = self.inputs;
            let block_work = inputs.network_hashrate * BCH_BLOCK_INTERVAL_SECONDS;
            let reward = BLOCK_REWARD_SATOSHIS * (1.0 - DONATION_FRACTION);
            let home_fraction = self.ledger.home_fraction(
                MATURITY_SKIP_SHARES as usize,
                BCH_PPLNS_WINDOW_BLOCKS * block_work,
                (WINDOW_CAP_SECONDS / inputs.period_seconds) as usize,
            );
            let sample = &mut self.hours[index];
            sample.seconds += seconds;
            sample.minimum_seconds += jobs.minimum * seconds;
            sample.band_cap_seconds += jobs.band_cap as f64 / FIXED_POINT_ONE as f64 * seconds;
            sample.weak_ratio_seconds += ratio * seconds;
            sample.home_income +=
                jobs.pool_hashrate * seconds / block_work * reward * home_fraction;
            sample.home_fair_income += jobs.home_hashrate * seconds / block_work * reward;
            sample.weak_found += weak_found as f64;
        }
    }

    /// Weak shares found on the tip in `seconds`, the home miners' apart.
    fn find_weak(
        &self,
        seconds: f64,
        jobs: &Jobs,
        ratio: f64,
        random: &mut SimulationRandom,
    ) -> WeakBatch {
        let weak_work = jobs.minimum * HASHES_PER_DIFFICULTY / ratio;
        let per_share = ratio - 1.0;
        let home = poisson(random, per_share * jobs.home_minimum_rate * seconds);
        let others = poisson(
            random,
            per_share * (jobs.minimum_rate - jobs.home_minimum_rate) * seconds,
        );
        WeakBatch {
            count: home + others,
            work: (home + others) as f64 * weak_work,
            home_work: home as f64 * weak_work,
        }
    }

    /// The next share: who finds it, what it carries, what it is credited,
    /// and the tip it leaves.
    fn add_share(&mut self, jobs: &Jobs, ratio: f64, interval: f64, random: &mut SimulationRandom) {
        let inputs = self.inputs;
        let total_rate: f64 = jobs.found_rates.iter().sum();
        let point = random.next_unit() * total_rate;
        let mut running = 0.0;
        let finder = jobs
            .found_rates
            .iter()
            .position(|rate| {
                running += rate;
                running > point
            })
            .unwrap_or(jobs.found_rates.len() - 1);
        let expired = self.slots.deepen(self.found_on_tip);
        self.found_on_tip = WeakBatch::default();
        if let Some(index) = self.hour_index(self.time) {
            self.hours[index].weak_expired += expired as f64;
        }
        let carried = self.slots.carry(inputs.weak_cap);
        let multiple = jobs.multiples[finder];
        let own = if multiple <= 1.0 {
            jobs.minimum * HASHES_PER_DIFFICULTY / ratio
        } else {
            multiple * jobs.minimum * HASHES_PER_DIFFICULTY
        };
        let finder_is_home = inputs.classes[finder].kind == MinerKind::Home;
        let home = carried.home_work * (1.0 - inputs.includer_cut)
            + if finder_is_home {
                own + inputs.includer_cut * carried.work
            } else {
                0.0
            };
        self.ledger.push(own + carried.work, home);
        self.tip = Tip {
            minimum: jobs.minimum,
            interval,
            band_cap: jobs.band_cap,
            wanted: jobs.wanted[finder],
            weak_ratio: next_weak_ratio(
                self.tip.weak_ratio,
                self.tip.carried,
                inputs.weak_fill,
                inputs.ratio_max,
            ),
            carried: carried.count,
        };
    }

    /// One step: mining on the tip until the next share, or until the event
    /// when it comes first (the work so far is kept and the rates change).
    fn step(&mut self, random: &mut SimulationRandom) {
        let jobs = self.jobs();
        let ratio = self.effective_ratio(jobs.minimum);
        let chain_rate = jobs.found_rates.iter().sum::<f64>() * (1.0 - self.inputs.stale_rate);
        let interval = random.next_exponential(1.0 / chain_rate);
        let event_pending = self.big_present == self.inputs.big_miner_leaves;
        let until_event = self.inputs.event_seconds - self.time;
        let (seconds, reaches_share) = if event_pending && interval >= until_event {
            (until_event, false)
        } else {
            (interval, true)
        };
        let found = self.find_weak(seconds, &jobs, ratio, random);
        self.record(seconds, &jobs, ratio, found.count);
        self.found_on_tip.count += found.count;
        self.found_on_tip.work += found.work;
        self.found_on_tip.home_work += found.home_work;
        if reaches_share {
            let share_time = self.time + seconds;
            self.add_share(&jobs, ratio, share_time - self.tip_time, random);
            self.tip_time = share_time;
        } else {
            self.big_present = !self.big_present;
        }
        self.time += seconds;
    }

    fn run(mut self, random: &mut SimulationRandom) -> Vec<HourSample> {
        let end = self.inputs.event_seconds + self.inputs.after_seconds;
        while self.time < end {
            self.step(random);
        }
        self.hours
    }
}

/// Run one transition and return its hourly samples, from
/// `HOURS_BEFORE_EVENT` hours before the event to the end.
pub fn simulate_transition(
    inputs: &TransitionInputs,
    random: &mut SimulationRandom,
) -> Vec<HourSample> {
    Transition::new(inputs).run(random)
}

/// The first hour of `values` within `tolerance` (a fraction) of `settled`;
/// `None` when none is. The band cap wanders around where it settles, so
/// the first hour it gets there is the measure, not the hour after which it
/// never leaves.
pub fn reach_hour(values: &[f64], settled: f64, tolerance: f64) -> Option<usize> {
    values
        .iter()
        .position(|value| (value - settled).abs() <= tolerance * settled.abs())
}

#[cfg(test)]
mod tests;
