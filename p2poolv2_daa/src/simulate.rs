// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The mining model.
//!
//! Shares arrive as a Poisson process whose rate changes with hashrate and
//! share difficulty. Each miner group `i` contributes
//! `hashrate_i / (max(D, d_i) * 2^32)` shares per second, where `D` is the
//! share difficulty and `d_i` the stratum difficulty of one of its
//! connections: the node's vardiff settles each connection at one
//! submission per 3.33 seconds, and a miner sends only work that meets its
//! stratum difficulty, so shares cannot outpace submissions. Upstream's
//! model had no such bound (`device_hashrate: None`).
//!
//! The waiting time for a share is drawn as one exponential amount of work
//! and spent across hashrate changes exactly, so a schedule that switches
//! hashrate in the middle of a share, or stops it for days, is followed to
//! the second. A share's header time is the whole second it was found in,
//! as a node stamps it.
//!
//! Besides the work each group does, the model adds up the credit it earns
//! on average: `rate_i * D` per second, which is its work times
//! `D / max(D, d_i)`. That is the difficulty PPLNS would pay it for, with
//! the lottery of which group finds the next share left out, so it shows
//! what the difficulty rule does to fairness: below a group's stratum
//! difficulty, the share target, not the work, sets what a submission
//! earns.
//!
//! Not modelled: propagation delay and uncles (one node, one chain), vardiff
//! transients after a miner connects, timestamp manipulation, and the
//! median-time-past and future-time limits on header times.

use crate::difficulty::{
    HASHES_PER_DIFFICULTY, difficulty_of, ideal_share_interval, maximum_stratum_difficulty,
    steady_difficulty, submission_interval,
};
use crate::rules::{Rule, RuleContext, SimChain, SimShare};
use crate::scenario::{Length, Scenario};
use p2poolv2_lib::stratum::difficulty_adjuster::MINIMUM_SESSION_DIFFICULTY;
use p2poolv2_tools::seeded::SeededSequence;

/// Header time of genesis: 1776855600 (2026-04-22 11:00 UTC), the time the
/// Bitcoin Cash share genesis takes from the regtest genesis data.
pub const GENESIS_TIME: u32 = 1_776_855_600;

/// Constants of the mining model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub genesis_time: u32,
    /// Seconds between one connection's submissions (3.33).
    pub submission_interval: f64,
    /// Stratum difficulty cap (2^32).
    pub maximum_stratum_difficulty: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            genesis_time: GENESIS_TIME,
            submission_interval: submission_interval(),
            maximum_stratum_difficulty: maximum_stratum_difficulty(),
        }
    }
}

/// What happened while one share was mined.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShareRecord {
    /// Seconds after genesis at which mining on the parent began.
    pub started: f64,
    /// Seconds after genesis at which the share was found.
    pub found: f64,
    /// Index of the miner group that found it.
    pub group: usize,
    /// Difficulty at which the hashrate mining when the share was found
    /// finds one share per ideal interval, never below the ceiling's.
    pub steady_difficulty: f64,
    /// Work done by every group while the share was mined, in units of
    /// 2^32 hashes.
    pub work: f64,
}

/// One simulated run.
#[derive(Clone, Debug)]
pub struct Run {
    /// Genesis and every share; share `index + 1` belongs to
    /// `records[index]`.
    pub chain: SimChain,
    pub records: Vec<ShareRecord>,
    /// Work done by each group over the run, in units of 2^32 hashes.
    pub group_work: Vec<f64>,
    /// Credit each group earned on average over the run, in difficulty:
    /// its share rate times the share difficulty, integrated over time.
    pub group_credit: Vec<f64>,
    /// Seconds after genesis at which the run stopped.
    pub end: f64,
    pub ceiling_difficulty: f64,
}

impl Run {
    /// Share `index` of the records (genesis excluded).
    pub fn share(&self, index: usize) -> &SimShare {
        &self.chain.shares()[index + 1]
    }
}

/// Work and average credit of each group so far.
struct Tally {
    work: Vec<f64>,
    credit: Vec<f64>,
}

/// A share the miners found.
struct Found {
    time: f64,
    group: usize,
    total_hashrate: f64,
    work: f64,
}

/// The miner groups of a scenario and their stratum difficulty floors.
struct Miners<'a> {
    scenario: &'a Scenario,
    stratum_floors: Vec<Option<f64>>,
    end: Option<f64>,
    seed: u64,
    hashrates: Vec<f64>,
    rates: Vec<f64>,
}

impl<'a> Miners<'a> {
    fn new(scenario: &'a Scenario, settings: &Settings, seed: u64) -> Self {
        let stratum_floors = scenario
            .groups
            .iter()
            .map(|group| {
                group
                    .device_hashrate
                    .map(|device| stratum_difficulty(device, settings))
            })
            .collect();
        let end = match scenario.length {
            Length::Shares(_) => None,
            Length::Seconds(seconds) => Some(seconds),
        };
        Self {
            scenario,
            stratum_floors,
            end,
            seed,
            hashrates: vec![0.0; scenario.groups.len()],
            rates: vec![0.0; scenario.groups.len()],
        }
    }

    /// Fill each group's hashrate and share rate for a share at `height`
    /// and `difficulty` at `time`; return the total share rate.
    fn fill_rates(&mut self, height: u32, time: f64, difficulty: f64) -> f64 {
        let mut total = 0.0;
        for (index, group) in self.scenario.groups.iter().enumerate() {
            let hashrate = group.schedule.hashrate(height, time, self.seed).max(0.0);
            let effective = match self.stratum_floors[index] {
                Some(floor) => difficulty.max(floor),
                None => difficulty,
            };
            let rate = hashrate / (effective * HASHES_PER_DIFFICULTY);
            self.hashrates[index] = hashrate;
            self.rates[index] = rate;
            total += rate;
        }
        total
    }

    /// Next time after `time` at which a hashrate changes or the run ends.
    fn next_boundary(&self, time: f64) -> Option<f64> {
        let changes = self
            .scenario
            .groups
            .iter()
            .filter_map(|group| group.schedule.next_change(time));
        changes
            .chain(self.end.filter(|end| *end > time))
            .min_by(f64::total_cmp)
    }

    fn has_ended(&self, time: f64) -> bool {
        self.end.is_some_and(|end| time >= end)
    }

    /// Mine the share at `height` and `difficulty` from `start`, adding the
    /// work done and credit earned to `tally`. `None` when the run ends
    /// first, or when no group will ever mine again.
    fn mine(
        &mut self,
        height: u32,
        difficulty: f64,
        start: f64,
        random: &mut SeededSequence,
        tally: &mut Tally,
    ) -> Option<Found> {
        let mut remaining = -(1.0 - random.next_unit()).ln();
        let pick = random.next_unit();
        let mut now = start;
        let mut work = 0.0;
        let mut found = None;
        let mut searching = !self.has_ended(now);
        while searching {
            let total_rate = self.fill_rates(height, now, difficulty);
            let needed = if total_rate > 0.0 {
                remaining / total_rate
            } else {
                f64::INFINITY
            };
            match self.next_boundary(now) {
                Some(boundary) if now + needed > boundary => {
                    let span = boundary - now;
                    work += self.add_work(span, difficulty, tally);
                    remaining -= total_rate * span;
                    now = boundary;
                    searching = !self.has_ended(now);
                }
                _ if needed.is_finite() => {
                    work += self.add_work(needed, difficulty, tally);
                    now += needed;
                    found = Some(Found {
                        time: now,
                        group: self.pick_group(pick * total_rate),
                        total_hashrate: self.hashrates.iter().sum(),
                        work,
                    });
                    searching = false;
                }
                // No hashrate now and no change ahead: the chain has stopped.
                _ => searching = false,
            }
        }
        found
    }

    /// Add `span` seconds of the current hashrates and share rates at
    /// `difficulty` to `tally`; return the work done.
    fn add_work(&self, span: f64, difficulty: f64, tally: &mut Tally) -> f64 {
        let mut total = 0.0;
        for (index, (hashrate, rate)) in self.hashrates.iter().zip(&self.rates).enumerate() {
            let work = hashrate * span / HASHES_PER_DIFFICULTY;
            tally.work[index] += work;
            tally.credit[index] += rate * span * difficulty;
            total += work;
        }
        total
    }

    /// The group whose cumulative share rate first exceeds `point`.
    fn pick_group(&self, point: f64) -> usize {
        let mut cumulative = 0.0;
        let position = self.rates.iter().position(|rate| {
            cumulative += rate;
            point < cumulative
        });
        // Rounding can leave `point` at the very top; the last group with a
        // rate then found it.
        position.unwrap_or_else(|| self.rates.iter().rposition(|rate| *rate > 0.0).unwrap_or(0))
    }
}

/// Stratum difficulty of one connection of `device_hashrate` hashes per
/// second once vardiff has settled, within the session limits.
pub fn stratum_difficulty(device_hashrate: f64, settings: &Settings) -> f64 {
    (device_hashrate * settings.submission_interval / HASHES_PER_DIFFICULTY).clamp(
        MINIMUM_SESSION_DIFFICULTY as f64,
        settings.maximum_stratum_difficulty,
    )
}

/// Run `scenario` under `rule` with the random sequence of `seed`.
pub fn simulate(scenario: &Scenario, rule: Rule, seed: u64, settings: &Settings) -> Run {
    let ideal = ideal_share_interval() as f64;
    let genesis = SimShare::new(0, settings.genesis_time, scenario.genesis.bits());
    let context = RuleContext::new(&genesis);
    let ceiling_difficulty = context.ceiling_difficulty();
    let capacity = scenario.expected_shares(ideal);
    let mut chain = SimChain::new(genesis, capacity);
    let mut records = Vec::with_capacity(capacity);
    let mut tally = Tally {
        work: vec![0.0; scenario.groups.len()],
        credit: vec![0.0; scenario.groups.len()],
    };
    let mut random = SeededSequence::new(seed);
    let mut miners = Miners::new(scenario, settings, seed);
    let share_limit = match scenario.length {
        Length::Shares(count) => count,
        Length::Seconds(_) => usize::MAX,
    };
    let mut now = 0.0;
    let mut running = share_limit > 0;
    while running {
        let bits = rule.next_target(&context, &chain);
        let difficulty = difficulty_of(bits);
        let height = chain.tip().height + 1;
        match miners.mine(height, difficulty, now, &mut random, &mut tally) {
            Some(found) => {
                let time = settings.genesis_time + found.time.floor() as u32;
                chain.push(SimShare::new(height, time, bits));
                records.push(ShareRecord {
                    started: now,
                    found: found.time,
                    group: found.group,
                    steady_difficulty: steady_difficulty(found.total_hashrate)
                        .max(ceiling_difficulty),
                    work: found.work,
                });
                now = found.time;
                running = records.len() < share_limit;
            }
            None => {
                now = miners.end.unwrap_or(now).max(now);
                running = false;
            }
        }
    }
    Run {
        chain,
        records,
        group_work: tally.work,
        group_credit: tally.credit,
        end: now,
        ceiling_difficulty,
    }
}

#[cfg(test)]
mod tests;
