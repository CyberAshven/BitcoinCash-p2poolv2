// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Measurements of a run, their aggregation over seeds, and upstream's
//! scores.
//!
//! Upstream's interval statistics are ported as upstream computed them:
//! percentiles are `sorted[floor(n * p)]`, a run's figures are averaged over
//! runs, and the median is over every run's intervals pooled.
//!
//! The Bitcoin Cash measurements add:
//!
//! - tracking error: how far the share difficulty is from the steady
//!   difficulty of the hashrate mining it, in doublings
//!   (`|log2(D / D*)|`), averaged over shares weighted by the work spent on
//!   each, so a burst of fast shares counts for the little work it took;
//! - settling: after a hashrate change, the time until the chain first
//!   mines at a difficulty within 10% of the new steady difficulty;
//! - credit over work: each group's share of the credit it earned on
//!   average (see [`crate::simulate`]; what PPLNS pays by) over its share of
//!   the work done. 1.0 is fair; above 1 a group is overpaid;
//! - uncredited work: the part of all work done while the share difficulty
//!   was below a group's stratum difficulty, so that each of its submissions
//!   earned the share difficulty instead of its own.

use crate::scenario::Scenario;
use crate::simulate::Run;
use p2poolv2_tools::stats::summarize;

/// Intervals shorter than this count as fast in upstream's tables.
pub const FAST_INTERVAL: f64 = 2.0;

/// Intervals longer than this (three ideal intervals) count as slow.
pub const SLOW_INTERVAL: f64 = 30.0;

/// A share is settled when its difficulty is within 10% of the steady
/// difficulty: `0.9 <= D / D* <= 1 / 0.9`.
pub const SETTLED_RATIO: f64 = 0.9;

/// How long before an event the hashrate is read to tell a rise from a
/// fall: schedules change exactly at their boundaries.
const CHANGE_EPSILON: f64 = 1e-6;

/// The time a hashrate change took to settle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settling {
    /// Seconds from the change until mining began on the first settled
    /// share.
    pub seconds: f64,
    /// Shares found in that time.
    pub shares: usize,
}

/// How the chain settled after one hashrate change.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EventSettling {
    /// The change raised the total hashrate.
    pub rising: bool,
    /// `None` when the difficulty did not settle before the next change or
    /// the end of the run.
    pub settled: Option<Settling>,
}

/// Measurements of one run.
#[derive(Clone, Debug, PartialEq)]
pub struct RunMetrics {
    pub shares: usize,
    /// Intervals after the warmup, in seconds, in chain order.
    pub intervals: Vec<f64>,
    pub mean_interval: f64,
    pub stddev_interval: f64,
    pub p95_interval: f64,
    pub p99_interval: f64,
    pub fast_percent: f64,
    pub slow_percent: f64,
    pub longest_interval: f64,
    /// Work-weighted mean of `|log2(D / D*)|` after the warmup.
    pub tracking_error: f64,
    /// Shares after the warmup mined at the ceiling, in percent.
    pub ceiling_percent: f64,
    /// Most shares found in any 60 seconds.
    pub peak_shares_per_minute: usize,
    /// One entry per event after which some group mines.
    pub settling: Vec<EventSettling>,
    /// Credit over work for each group; `None` for a group that did no work.
    pub credit_over_work: Vec<Option<f64>>,
    /// Work that earned no credit, in percent of all work.
    pub uncredited_percent: f64,
}

/// `sorted[floor(len * fraction)]`, as upstream indexed percentiles. Zero
/// for an empty slice.
pub fn upstream_percentile(sorted: &[f64], fraction: f64) -> f64 {
    let index = (sorted.len() as f64 * fraction) as usize;
    sorted
        .get(index.min(sorted.len().saturating_sub(1)))
        .copied()
        .unwrap_or(0.0)
}

/// Median of `sorted`, the mean of the middle two for an even count.
pub fn median(sorted: &[f64]) -> f64 {
    let count = sorted.len();
    match count {
        0 => 0.0,
        _ if count % 2 == 1 => sorted[count / 2],
        _ => (sorted[count / 2 - 1] + sorted[count / 2]) / 2.0,
    }
}

fn percent(part: usize, whole: usize) -> f64 {
    if whole == 0 {
        0.0
    } else {
        100.0 * part as f64 / whole as f64
    }
}

/// Most of the ascending `times` that fall in any window of `window`
/// seconds.
pub fn peak_count(times: &[f64], window: f64) -> usize {
    let mut first = 0;
    let mut peak = 0;
    for (last, time) in times.iter().enumerate() {
        while time - times[first] >= window {
            first += 1;
        }
        peak = peak.max(last + 1 - first);
    }
    peak
}

/// `|log2(difficulty / steady)|`.
pub fn doublings_off(difficulty: f64, steady: f64) -> f64 {
    (difficulty / steady).log2().abs()
}

fn is_settled(difficulty: f64, steady: f64) -> bool {
    doublings_off(difficulty, steady) <= (1.0 / SETTLED_RATIO).log2()
}

/// How the chain settled after the change at `event`, before `until`.
pub fn settling_after(run: &Run, event: f64, until: f64) -> Option<Settling> {
    let settled = run.records.iter().enumerate().find(|(index, record)| {
        record.started >= event
            && record.found < until
            && is_settled(run.share(*index).difficulty, record.steady_difficulty)
    });
    settled.map(|(_, record)| Settling {
        seconds: record.started - event,
        shares: run
            .records
            .iter()
            .filter(|earlier| earlier.found >= event && earlier.found <= record.started)
            .count(),
    })
}

/// Settling after each event of `scenario` after which some group mines.
fn settling_per_event(run: &Run, scenario: &Scenario, seed: u64) -> Vec<EventSettling> {
    scenario
        .events
        .iter()
        .enumerate()
        .filter(|(_, event)| scenario.total_hashrate(0, **event, seed) > 0.0)
        .map(|(index, event)| {
            let until = scenario
                .events
                .get(index + 1)
                .copied()
                .unwrap_or(run.end)
                .min(run.end);
            let before = scenario.total_hashrate(0, event - CHANGE_EPSILON, seed);
            EventSettling {
                rising: scenario.total_hashrate(0, *event, seed) > before,
                settled: settling_after(run, *event, until),
            }
        })
        .collect()
}

/// Credit over work for each group.
pub fn credit_over_work(run: &Run) -> Vec<Option<f64>> {
    let total_credit: f64 = run.group_credit.iter().sum();
    let total_work: f64 = run.group_work.iter().sum();
    run.group_credit
        .iter()
        .zip(&run.group_work)
        .map(|(credit, work)| {
            (*work > 0.0 && total_credit > 0.0)
                .then(|| (credit / total_credit) / (work / total_work))
        })
        .collect()
}

/// Work that earned no credit, in percent of all work; zero without work.
pub fn uncredited_percent(run: &Run) -> f64 {
    let total_work: f64 = run.group_work.iter().sum();
    let total_credit: f64 = run.group_credit.iter().sum();
    if total_work > 0.0 {
        (100.0 * (1.0 - total_credit / total_work)).max(0.0)
    } else {
        0.0
    }
}

/// Measure `run` of `scenario`, which was seeded with `seed`.
pub fn measure(run: &Run, scenario: &Scenario, seed: u64) -> RunMetrics {
    let warmup = scenario.warmup_shares.min(run.records.len());
    let measured = &run.records[warmup..];
    let intervals: Vec<f64> = measured
        .iter()
        .map(|record| record.found - record.started)
        .collect();
    let mut sorted = intervals.clone();
    sorted.sort_by(f64::total_cmp);
    let summary = summarize(&intervals);
    let (weighted_error, total_work) = measured.iter().enumerate().fold(
        (0.0, 0.0),
        |(weighted_error, total_work), (offset, record)| {
            let difficulty = run.share(warmup + offset).difficulty;
            (
                weighted_error + record.work * doublings_off(difficulty, record.steady_difficulty),
                total_work + record.work,
            )
        },
    );
    let at_ceiling = (warmup..run.records.len())
        .filter(|index| run.share(*index).difficulty <= run.ceiling_difficulty)
        .count();
    let found_times: Vec<f64> = run.records.iter().map(|record| record.found).collect();
    RunMetrics {
        shares: run.records.len(),
        mean_interval: summary.mean,
        stddev_interval: summary.stddev,
        p95_interval: upstream_percentile(&sorted, 0.95),
        p99_interval: upstream_percentile(&sorted, 0.99),
        fast_percent: percent(
            intervals.iter().filter(|gap| **gap < FAST_INTERVAL).count(),
            intervals.len(),
        ),
        slow_percent: percent(
            intervals.iter().filter(|gap| **gap > SLOW_INTERVAL).count(),
            intervals.len(),
        ),
        longest_interval: sorted.last().copied().unwrap_or(0.0),
        tracking_error: if total_work > 0.0 {
            weighted_error / total_work
        } else {
            0.0
        },
        ceiling_percent: percent(at_ceiling, measured.len()),
        peak_shares_per_minute: peak_count(&found_times, 60.0),
        settling: settling_per_event(run, scenario, seed),
        credit_over_work: credit_over_work(run),
        uncredited_percent: uncredited_percent(run),
        intervals,
    }
}

fn mean_of(values: impl Iterator<Item = f64>) -> f64 {
    let collected: Vec<f64> = values.collect();
    summarize(&collected).mean
}

/// One rule's row in an upstream scenario table.
#[derive(Clone, Debug, PartialEq)]
pub struct UpstreamRow {
    pub label: String,
    pub mean: f64,
    pub median: f64,
    pub stddev: f64,
    pub p95: f64,
    pub p99: f64,
    pub fast_percent: f64,
    pub slow_percent: f64,
    /// `|mean - ideal| / ideal`, in percent.
    pub deviation_percent: f64,
}

/// Upstream's aggregation of `runs` for one rule and scenario.
pub fn upstream_row(label: String, runs: &[RunMetrics], ideal: f64) -> UpstreamRow {
    let mut pooled: Vec<f64> = runs
        .iter()
        .flat_map(|run| run.intervals.iter().copied())
        .collect();
    pooled.sort_by(f64::total_cmp);
    let mean = mean_of(runs.iter().map(|run| run.mean_interval));
    UpstreamRow {
        label,
        mean,
        median: median(&pooled),
        stddev: mean_of(runs.iter().map(|run| run.stddev_interval)),
        p95: mean_of(runs.iter().map(|run| run.p95_interval)),
        p99: mean_of(runs.iter().map(|run| run.p99_interval)),
        fast_percent: mean_of(runs.iter().map(|run| run.fast_percent)),
        slow_percent: mean_of(runs.iter().map(|run| run.slow_percent)),
        deviation_percent: (mean - ideal).abs() / ideal * 100.0,
    }
}

/// Upstream's composite score of one row: deviation, spread and tails,
/// doubled for attack scenarios. Lower is better.
pub fn composite_score(row: &UpstreamRow, attack: bool, ideal: f64) -> f64 {
    let weight = if attack { 2.0 } else { 1.0 };
    weight
        * (row.deviation_percent
            + row.stddev / ideal * 10.0
            + row.slow_percent * 5.0
            + row.fast_percent * 2.0)
}

/// Upstream's attack score of one row of an attack scenario. Lower is
/// better.
pub fn attack_score(row: &UpstreamRow, ideal: f64) -> f64 {
    row.deviation_percent + row.slow_percent * 10.0 + row.stddev / ideal * 20.0
}

/// One rule's row in a Bitcoin Cash scenario table.
#[derive(Clone, Debug, PartialEq)]
pub struct BchRow {
    pub label: String,
    pub mean_shares: f64,
    pub mean_interval: f64,
    pub p99_interval: f64,
    /// Longest interval over every run.
    pub longest_interval: f64,
    pub tracking_error: f64,
    pub ceiling_percent: f64,
    /// Most shares in a minute over every run.
    pub peak_shares_per_minute: usize,
    /// Mean settling time after rises and after falls in hashrate, and the
    /// longest after either, over every event of every run, in seconds;
    /// `None` without settled events.
    pub settle_up_mean: Option<f64>,
    pub settle_down_mean: Option<f64>,
    pub settle_max: Option<f64>,
    /// Mean shares found while settling.
    pub settle_shares: Option<f64>,
    /// Settled events over measured events, in percent; `None` without
    /// events.
    pub settled_percent: Option<f64>,
    /// Credit over work of the scenario's first group, averaged over runs.
    pub first_group_credit: Option<f64>,
    /// Work that earned no credit, in percent, averaged over runs.
    pub uncredited_percent: f64,
}

/// Aggregate `runs` of one rule and scenario.
pub fn bch_row(label: String, runs: &[RunMetrics]) -> BchRow {
    let settlings: Vec<&EventSettling> = runs.iter().flat_map(|run| run.settling.iter()).collect();
    let settled: Vec<Settling> = settlings.iter().filter_map(|entry| entry.settled).collect();
    let settle_seconds: Vec<f64> = settled.iter().map(|entry| entry.seconds).collect();
    let mean_settle = |rising: bool| {
        let seconds: Vec<f64> = settlings
            .iter()
            .filter(|entry| entry.rising == rising)
            .filter_map(|entry| entry.settled.map(|settled| settled.seconds))
            .collect();
        (!seconds.is_empty()).then(|| summarize(&seconds).mean)
    };
    let credits: Vec<f64> = runs
        .iter()
        .filter_map(|run| run.credit_over_work.first().copied().flatten())
        .collect();
    BchRow {
        label,
        mean_shares: mean_of(runs.iter().map(|run| run.shares as f64)),
        mean_interval: mean_of(runs.iter().map(|run| run.mean_interval)),
        p99_interval: mean_of(runs.iter().map(|run| run.p99_interval)),
        longest_interval: runs
            .iter()
            .map(|run| run.longest_interval)
            .fold(0.0, f64::max),
        tracking_error: mean_of(runs.iter().map(|run| run.tracking_error)),
        ceiling_percent: mean_of(runs.iter().map(|run| run.ceiling_percent)),
        peak_shares_per_minute: runs
            .iter()
            .map(|run| run.peak_shares_per_minute)
            .max()
            .unwrap_or(0),
        settle_up_mean: mean_settle(true),
        settle_down_mean: mean_settle(false),
        settle_max: settle_seconds.iter().copied().reduce(f64::max),
        settle_shares: (!settled.is_empty())
            .then(|| mean_of(settled.iter().map(|entry| entry.shares as f64))),
        settled_percent: (!settlings.is_empty()).then(|| percent(settled.len(), settlings.len())),
        first_group_credit: (!credits.is_empty()).then(|| summarize(&credits).mean),
        uncredited_percent: mean_of(runs.iter().map(|run| run.uncredited_percent)),
    }
}

#[cfg(test)]
mod tests;
