// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use crate::difficulty::compact_for_difficulty;
use crate::rules::{SimChain, SimShare};
use crate::scenario::{Genesis, Length, MinerGroup, Schedule};
use crate::simulate::ShareRecord;

const GENESIS_TIME: u32 = 1_776_855_600;

/// A run whose shares were found at `found` (seconds after genesis), at
/// `difficulties`, all by group 0 against a steady difficulty of `steady`.
fn synthetic_run(found: &[f64], difficulties: &[f64], steady: f64) -> Run {
    let genesis = SimShare::new(0, GENESIS_TIME, compact_for_difficulty(steady));
    let mut chain = SimChain::new(genesis, found.len());
    let mut records = Vec::with_capacity(found.len());
    let mut started = 0.0;
    for (index, (time, difficulty)) in found.iter().zip(difficulties).enumerate() {
        chain.push(SimShare::new(
            index as u32 + 1,
            GENESIS_TIME + *time as u32,
            compact_for_difficulty(*difficulty),
        ));
        records.push(ShareRecord {
            started,
            found: *time,
            group: 0,
            steady_difficulty: steady,
            work: time - started,
        });
        started = *time;
    }
    Run {
        chain,
        records,
        group_work: vec![started],
        group_credit: vec![started],
        end: started,
        ceiling_difficulty: 1_000.0,
    }
}

fn scenario_with_events(events: Vec<f64>, warmup_shares: usize) -> Scenario {
    Scenario {
        id: "test",
        title: "test",
        description: "test",
        genesis: Genesis::Main,
        groups: vec![MinerGroup {
            name: "miners",
            device_hashrate: None,
            schedule: Schedule::Constant(1.0),
        }],
        length: Length::Seconds(1_000.0),
        warmup_shares,
        events,
        attack: false,
    }
}

#[test]
fn percentiles_index_as_upstream_did() {
    let sorted: Vec<f64> = (1..=100).map(f64::from).collect();
    assert_eq!(upstream_percentile(&sorted, 0.95), 96.0);
    assert_eq!(upstream_percentile(&sorted, 0.99), 100.0);
    assert_eq!(upstream_percentile(&sorted[..3], 0.99), 3.0);
    assert_eq!(upstream_percentile(&[], 0.95), 0.0);
}

#[test]
fn median_takes_the_middle_or_the_mean_of_the_middle_two() {
    assert_eq!(median(&[1.0, 2.0, 9.0]), 2.0);
    assert_eq!(median(&[1.0, 2.0, 4.0, 9.0]), 3.0);
    assert_eq!(median(&[]), 0.0);
}

#[test]
fn peak_count_finds_the_busiest_window() {
    let times = [0.0, 10.0, 20.0, 61.0, 62.0, 63.0, 64.0, 200.0];
    assert_eq!(peak_count(&times, 60.0), 6);
    assert_eq!(peak_count(&times, 1.0), 1);
    assert_eq!(peak_count(&[], 60.0), 0);
}

#[test]
fn doublings_off_is_symmetric() {
    assert_eq!(doublings_off(200.0, 100.0), 1.0);
    assert_eq!(doublings_off(50.0, 100.0), 1.0);
    assert_eq!(doublings_off(100.0, 100.0), 0.0);
}

#[test]
fn settling_waits_for_a_share_within_ten_percent() {
    // Shares at 10, 50, 95 and 100: the third is within 10% of 100.
    let run = synthetic_run(
        &[100.0, 110.0, 130.0, 140.0],
        &[10.0, 50.0, 95.0, 100.0],
        100.0,
    );
    let settled = settling_after(&run, 105.0, 1_000.0).expect("settles");
    assert_eq!(settled.seconds, 110.0 - 105.0);
    // Share 2, found at 110, is the only one in [105, 110].
    assert_eq!(settled.shares, 1);
    assert_eq!(settling_after(&run, 105.0, 120.0), None);
    assert_eq!(settling_after(&run, 200.0, 1_000.0), None);
}

#[test]
fn measure_leaves_out_the_warmup() {
    let found = [1.0, 2.0, 40.0, 41.0, 51.0];
    let run = synthetic_run(
        &found,
        &[1_000.0, 2_000.0, 2_000.0, 2_000.0, 2_000.0],
        2_000.0,
    );
    let metrics = measure(&run, &scenario_with_events(Vec::new(), 1), 0);
    assert_eq!(metrics.shares, 5);
    assert_eq!(metrics.intervals, [1.0, 38.0, 1.0, 10.0]);
    assert_eq!(metrics.mean_interval, 12.5);
    assert_eq!(metrics.fast_percent, 50.0);
    assert_eq!(metrics.slow_percent, 25.0);
    assert_eq!(metrics.longest_interval, 38.0);
    assert_eq!(metrics.ceiling_percent, 0.0);
    assert_eq!(metrics.peak_shares_per_minute, 5);
    assert!(metrics.tracking_error < 1e-3);
    assert!(metrics.settling.is_empty());
    assert_eq!(metrics.credit_over_work, [Some(1.0)]);
    assert_eq!(metrics.uncredited_percent, 0.0);
}

#[test]
fn tracking_error_weights_each_share_by_its_work() {
    // One share at half the steady difficulty over 30 seconds of work,
    // one on it over 10.
    let run = synthetic_run(&[30.0, 40.0], &[500.0, 1_000.0], 1_000.0);
    let metrics = measure(&run, &scenario_with_events(Vec::new(), 0), 0);
    assert!((metrics.tracking_error - 0.75).abs() < 1e-3);
}

#[test]
fn settling_is_measured_after_events_with_miners_and_tells_rises_from_falls() {
    let mut run = synthetic_run(&[100.0, 110.0], &[10.0, 1_000.0], 1_000.0);
    run.end = 200.0;
    let mut scenario = scenario_with_events(vec![50.0, 150.0, 170.0], 0);
    scenario.groups[0].schedule =
        Schedule::TimeSteps(vec![(0.0, 1.0), (50.0, 2.0), (150.0, 1.0), (170.0, 0.0)]);
    let metrics = measure(&run, &scenario, 0);
    // Nobody mines after the third event, so it is not measured.
    assert_eq!(
        metrics.settling,
        [
            EventSettling {
                rising: true,
                settled: Some(Settling {
                    seconds: 50.0,
                    shares: 1
                }),
            },
            EventSettling {
                rising: false,
                settled: None,
            },
        ]
    );
}

#[test]
fn credit_over_work_compares_shares_of_credit_and_work() {
    let mut run = synthetic_run(&[10.0], &[1_000.0], 1_000.0);
    run.group_work = vec![10.0, 30.0, 0.0];
    run.group_credit = vec![10.0, 10.0, 0.0];
    let ratios = credit_over_work(&run);
    assert_eq!(ratios[0], Some(0.5 / 0.25));
    assert_eq!(ratios[1], Some(0.5 / 0.75));
    assert_eq!(ratios[2], None);
    assert_eq!(uncredited_percent(&run), 50.0);
}

fn metrics_with(mean: f64, intervals: Vec<f64>) -> RunMetrics {
    RunMetrics {
        shares: intervals.len(),
        intervals,
        mean_interval: mean,
        stddev_interval: 10.0,
        p95_interval: 30.0,
        p99_interval: 45.0,
        fast_percent: 20.0,
        slow_percent: 5.0,
        longest_interval: 60.0,
        tracking_error: 0.1,
        ceiling_percent: 0.0,
        peak_shares_per_minute: 12,
        settling: vec![
            EventSettling {
                rising: true,
                settled: Some(Settling {
                    seconds: 60.0,
                    shares: 6,
                }),
            },
            EventSettling {
                rising: false,
                settled: Some(Settling {
                    seconds: 120.0,
                    shares: 2,
                }),
            },
            EventSettling {
                rising: false,
                settled: None,
            },
        ],
        credit_over_work: vec![Some(1.2)],
        uncredited_percent: 2.0,
    }
}

#[test]
fn upstream_rows_average_runs_and_pool_the_median() {
    let runs = [
        metrics_with(9.0, vec![1.0, 2.0, 3.0]),
        metrics_with(11.0, vec![4.0, 5.0, 6.0]),
    ];
    let row = upstream_row("rule".to_string(), &runs, 10.0);
    assert_eq!(row.mean, 10.0);
    assert_eq!(row.median, 3.5);
    assert_eq!(row.stddev, 10.0);
    assert_eq!(row.deviation_percent, 0.0);
}

#[test]
fn scores_follow_upstreams_formulas() {
    let row = UpstreamRow {
        label: "rule".to_string(),
        mean: 11.0,
        median: 7.0,
        stddev: 10.0,
        p95: 30.0,
        p99: 45.0,
        fast_percent: 20.0,
        slow_percent: 5.0,
        deviation_percent: 10.0,
    };
    // 10 + 10 + 25 + 40.
    assert_eq!(composite_score(&row, false, 10.0), 85.0);
    assert_eq!(composite_score(&row, true, 10.0), 170.0);
    // 10 + 50 + 20.
    assert_eq!(attack_score(&row, 10.0), 80.0);
}

#[test]
fn bch_rows_aggregate_settling_and_credit() {
    let runs = [metrics_with(9.0, vec![1.0]), metrics_with(11.0, vec![2.0])];
    let row = bch_row("rule".to_string(), &runs);
    assert_eq!(row.mean_interval, 10.0);
    assert_eq!(row.settle_up_mean, Some(60.0));
    assert_eq!(row.settle_down_mean, Some(120.0));
    assert_eq!(row.settle_max, Some(120.0));
    assert_eq!(row.settle_shares, Some(4.0));
    let settled = row.settled_percent.expect("events were measured");
    assert!((settled - 200.0 / 3.0).abs() < 1e-9);
    assert_eq!(row.first_group_credit, Some(1.2));
    assert_eq!(row.uncredited_percent, 2.0);
    assert_eq!(row.peak_shares_per_minute, 12);

    let quiet = RunMetrics {
        settling: Vec::new(),
        credit_over_work: vec![None],
        ..metrics_with(10.0, vec![1.0])
    };
    let row = bch_row("rule".to_string(), &[quiet]);
    assert_eq!(row.settle_up_mean, None);
    assert_eq!(row.settle_down_mean, None);
    assert_eq!(row.settled_percent, None);
    assert_eq!(row.first_group_credit, None);
}
