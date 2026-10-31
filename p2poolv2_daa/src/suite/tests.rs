// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use crate::rules::LWMA_45;
use crate::scenario::{Length, bch_suite, upstream_suite};

fn short_steady(shares: usize) -> Scenario {
    let mut scenario = upstream_suite().swap_remove(0);
    scenario.length = Length::Shares(shares);
    scenario.warmup_shares = 10;
    scenario
}

#[test]
fn run_seeds_step_by_a_thousand_as_upstream() {
    assert_eq!(run_seed(42, 0), 42);
    assert_eq!(run_seed(42, 3), 3_042);
    assert_eq!(run_seed(u64::MAX, 1), 999);
}

#[test]
fn charts_start_an_hour_before_the_first_event() {
    let mut scenario = short_steady(10);
    assert_eq!(chart_start(&scenario), 0.0);
    scenario.events = vec![7_200.0, 9_000.0];
    assert_eq!(chart_start(&scenario), 3_600.0);
    scenario.events = vec![1_800.0];
    assert_eq!(chart_start(&scenario), 0.0);
}

#[test]
fn evaluate_keeps_rule_order_and_is_reproducible() {
    let scenario = short_steady(200);
    let rules = [Rule::ParentAnchor, LWMA_45, Rule::GenesisAnchor];
    let settings = Settings::default();
    let first = evaluate(&scenario, &rules, 3, 42, &settings);
    let again = evaluate(&scenario, &rules, 3, 42, &settings);
    let order: Vec<Rule> = first.rules.iter().map(|result| result.rule).collect();
    assert_eq!(order, rules);
    for (one, other) in first.rules.iter().zip(&again.rules) {
        assert_eq!(one.runs, other.runs);
        assert_eq!(one.trace, other.trace);
        assert_eq!(one.runs.len(), 3);
    }
    // Each run has its own seed.
    let runs = &first.rules[0].runs;
    assert_ne!(runs[0].intervals, runs[1].intervals);
}

#[test]
fn a_trace_samples_difficulty_and_steady_difficulty_over_the_run() {
    let scenario = bch_suite()
        .into_iter()
        .find(|scenario| scenario.id == "quiet-1h")
        .expect("quiet-1h is a Bitcoin Cash scenario");
    let seed = run_seed(42, 0);
    let run = simulate(&scenario, Rule::ParentAnchor, seed, &Settings::default());
    let start = chart_start(&scenario);
    let sampled = trace(&run, &scenario, seed, start, 100);
    assert_eq!(sampled.difficulty.len(), 100);
    assert_eq!(sampled.steady.len(), 100);
    assert_eq!(sampled.difficulty[0].0, 0.0);
    let span = (run.end - start) / 3_600.0;
    assert!((sampled.difficulty[99].0 - span).abs() < 1e-9);
    // Nobody mines during the quiet hour, which starts an hour into the chart.
    let quiet = sampled
        .steady
        .iter()
        .filter(|(hours, _)| (1.05..1.95).contains(hours));
    assert!(quiet.clone().count() > 0);
    assert!(quiet.into_iter().all(|(_, steady)| *steady == 0.0));
    assert!(
        sampled
            .difficulty
            .iter()
            .all(|(_, difficulty)| *difficulty > 0.0)
    );
}

#[test]
fn an_empty_run_has_no_trace() {
    let scenario = short_steady(0);
    let run = simulate(&scenario, Rule::ParentAnchor, 1, &Settings::default());
    assert!(run.records.is_empty());
    assert_eq!(trace(&run, &scenario, 1, 0.0, 10), Trace::default());
}
