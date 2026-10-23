// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Run every rule over a scenario for a set of seeds.

use crate::difficulty::steady_difficulty;
use crate::metrics::{RunMetrics, measure};
use crate::rules::Rule;
use crate::scenario::Scenario;
use crate::simulate::{Run, Settings, simulate};
use std::thread;

/// Seeds are `base + run * 1000`, as upstream seeded run `run` with
/// `42 + run * 1000`.
pub const SEED_STRIDE: u64 = 1_000;

/// Points in a chart trace.
pub const TRACE_POINTS: usize = 600;

/// Difficulty over time in one run, for charts: `(hours, difficulty)`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Trace {
    pub difficulty: Vec<(f64, f64)>,
    /// Steady difficulty of the hashrate mining at each point, at least the
    /// ceiling's; zero where nobody mines.
    pub steady: Vec<(f64, f64)>,
}

/// One rule's measurements over every seed.
#[derive(Clone, Debug)]
pub struct RuleResults {
    pub rule: Rule,
    pub runs: Vec<RunMetrics>,
    /// The first seed's run, sampled.
    pub trace: Trace,
}

/// Every rule's results for one scenario.
#[derive(Clone, Debug)]
pub struct ScenarioResults {
    pub scenario: Scenario,
    pub rules: Vec<RuleResults>,
}

/// Seed of run `run`.
pub fn run_seed(base_seed: u64, run: usize) -> u64 {
    base_seed.wrapping_add(run as u64 * SEED_STRIDE)
}

/// Where a scenario's chart starts: an hour before its first event, or at
/// genesis.
pub fn chart_start(scenario: &Scenario) -> f64 {
    scenario
        .events
        .first()
        .map(|event| (event - 3_600.0).max(0.0))
        .unwrap_or(0.0)
}

/// Sample `run` of `scenario` at `points` evenly spaced times from
/// `start` to the run's end.
pub fn trace(run: &Run, scenario: &Scenario, seed: u64, start: f64, points: usize) -> Trace {
    if run.records.is_empty() || run.end <= start || points < 2 {
        return Trace::default();
    }
    let step = (run.end - start) / (points - 1) as f64;
    let mut difficulty = Vec::with_capacity(points);
    let mut steady = Vec::with_capacity(points);
    for point in 0..points {
        let time = start + step * point as f64;
        // The share being mined at `time` is the last one started by then.
        let index = run
            .records
            .partition_point(|record| record.started <= time)
            .saturating_sub(1);
        let share = run.share(index);
        let hours = (time - start) / 3_600.0;
        difficulty.push((hours, share.difficulty));
        let hashrate = scenario.total_hashrate(share.height, time, seed);
        let steady_point = if hashrate > 0.0 {
            steady_difficulty(hashrate).max(run.ceiling_difficulty)
        } else {
            0.0
        };
        steady.push((hours, steady_point));
    }
    Trace { difficulty, steady }
}

/// Run `rule` over `scenario` for `runs` seeds from `base_seed`.
pub fn evaluate_rule(
    scenario: &Scenario,
    rule: Rule,
    runs: usize,
    base_seed: u64,
    settings: &Settings,
) -> RuleResults {
    let mut measured = Vec::with_capacity(runs);
    let mut first_trace = Trace::default();
    for run_index in 0..runs {
        let seed = run_seed(base_seed, run_index);
        let run = simulate(scenario, rule, seed, settings);
        if run_index == 0 {
            first_trace = trace(&run, scenario, seed, chart_start(scenario), TRACE_POINTS);
        }
        measured.push(measure(&run, scenario, seed));
    }
    RuleResults {
        rule,
        runs: measured,
        trace: first_trace,
    }
}

/// Run every rule in `rules` over `scenario`, one thread per rule.
pub fn evaluate(
    scenario: &Scenario,
    rules: &[Rule],
    runs: usize,
    base_seed: u64,
    settings: &Settings,
) -> ScenarioResults {
    let results = thread::scope(|scope| {
        let handles: Vec<_> = rules
            .iter()
            .map(|rule| {
                scope.spawn(move || evaluate_rule(scenario, *rule, runs, base_seed, settings))
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("a rule's simulation panicked"))
            .collect()
    });
    ScenarioResults {
        scenario: scenario.clone(),
        rules: results,
    }
}

#[cfg(test)]
mod tests;
