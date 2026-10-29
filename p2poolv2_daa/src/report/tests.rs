// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use crate::rules::{LWMA_45, Rule};
use crate::scenario::{Length, bch_suite, upstream_suite};
use crate::simulate::Settings;
use crate::suite::evaluate;

#[test]
fn durations_read_in_the_nearest_unit() {
    assert_eq!(human_duration(59.4), "59 s");
    assert_eq!(human_duration(600.0), "10.0 min");
    assert_eq!(human_duration(7_200.0), "2.0 h");
    assert_eq!(human_duration(3.0 * 86_400.0), "3.0 d");
}

#[test]
fn an_upstream_row_has_upstreams_columns() {
    let row = UpstreamRow {
        label: "ASERT".to_string(),
        mean: 10.0,
        median: 6.924,
        stddev: 10.03,
        p95: 29.92,
        p99: 46.53,
        fast_percent: 18.14,
        slow_percent: 4.91,
        deviation_percent: 0.0,
    };
    assert_eq!(
        upstream_table_row(&row),
        "| ASERT | 10.00 | 6.92 | 10.03 | 29.92 | 46.53 | 18.1 | 4.9 | 0.0 |"
    );
}

#[test]
fn a_bch_row_marks_missing_values() {
    let row = BchRow {
        label: "rule".to_string(),
        mean_shares: 1_234.4,
        mean_interval: 9.876,
        p99_interval: 45.67,
        longest_interval: 7_200.0,
        tracking_error: 0.1234,
        ceiling_percent: 0.0,
        peak_shares_per_minute: 31,
        settle_up_mean: None,
        settle_down_mean: None,
        settle_max: None,
        settle_shares: None,
        settled_percent: None,
        first_group_credit: Some(1.0),
        uncredited_percent: 12.34,
    };
    assert_eq!(
        bch_table_row(&row),
        "| rule | 1234 | 9.88 | 45.7 | 2.0 h | 0.123 | 0.0 | 31 | - | - | - | - | - | 12.3 | 1.000 |"
    );
}

#[test]
fn the_upstream_report_ranks_every_rule() {
    let rules = [Rule::ParentAnchor, LWMA_45];
    let results: Vec<_> = upstream_suite()
        .into_iter()
        .take(3)
        .map(|mut scenario| {
            scenario.length = Length::Shares(150);
            scenario.warmup_shares = 10;
            evaluate(&scenario, &rules, 2, 42, &Settings::default())
        })
        .collect();
    let mut out = Vec::new();
    write_upstream_report(&mut out, &results, 10.0).expect("writes to memory");
    let text = String::from_utf8(out).expect("the report is UTF-8");
    assert!(text.contains("#### Steady State"));
    assert!(text.contains("#### Oscillation Attack (100-share cycle)"));
    assert!(text.contains("#### Composite score (lower is better)"));
    assert!(text.contains("#### Attack score"));
    for label in rules.map(|rule| rule.label()) {
        // One row in each of the three scenario tables and both rankings.
        assert_eq!(text.matches(&format!("| {label} |")).count(), 5, "{label}");
    }
    assert!(text.contains("| 1 | "));
    assert!(text.contains("| 2 | "));
}

#[test]
fn the_bch_report_names_the_first_group() {
    let mut scenario = bch_suite()
        .into_iter()
        .find(|scenario| scenario.id == "farm-leaves")
        .expect("farm-leaves is a Bitcoin Cash scenario");
    scenario.length = Length::Seconds(3.0 * 3_600.0);
    let results = [evaluate(
        &scenario,
        &[Rule::ParentAnchor],
        1,
        42,
        &Settings::default(),
    )];
    let mut out = Vec::new();
    write_bch_report(&mut out, &results).expect("writes to memory");
    let text = String::from_utf8(out).expect("the report is UTF-8");
    assert!(text.contains(&format!("#### {}", scenario.title)));
    assert!(text.contains("| BitAxe credit/work |"));
    assert!(text.contains(&format!("| {} |", Rule::ParentAnchor.label())));
}
