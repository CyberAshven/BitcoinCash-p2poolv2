// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;

#[test]
fn thousands_are_grouped_with_commas() {
    assert_eq!(group_thousands(0), "0");
    assert_eq!(group_thousands(999), "999");
    assert_eq!(group_thousands(1_000), "1,000");
    assert_eq!(group_thousands(1_234_567), "1,234,567");
}

#[test]
fn fractions_print_as_percentages() {
    assert_eq!(percent(0.0123), "1.23%");
}

#[test]
fn a_share_some_node_never_got_prints_never() {
    assert_eq!(seconds(f64::INFINITY), "never");
    assert_eq!(seconds(1.234), "1.23");
}

#[test]
fn rates_keep_about_three_significant_digits() {
    assert_eq!(rounded(1_234.5), "1,235");
    assert_eq!(rounded(12.34), "12.3");
    assert_eq!(rounded(1.234), "1.23");
    assert_eq!(rounded(0.1234), "0.123");
    assert_eq!(rounded(0.0), "0");
    assert_eq!(rounded(0.00012), "1.2e-4");
}

#[test]
fn a_row_has_as_many_cells_as_the_header() {
    let scenario = Scenario {
        node_count: 10,
        duration_seconds: 600.0,
        ..Scenario::default()
    };
    let report = scenario.run();
    let header_cells = markdown_header()
        .lines()
        .next()
        .map(|line| line.matches('|').count());
    let row_cells = markdown_row(&report).matches('|').count();
    assert_eq!(header_cells, Some(row_cells));
}

#[test]
fn a_report_names_what_it_ran() {
    let scenario = Scenario {
        label: "tiny".to_string(),
        node_count: 10,
        duration_seconds: 600.0,
        ..Scenario::default()
    };
    let report = scenario.run();
    assert!(report.summary_text().starts_with("tiny: 10 nodes"));
    assert_eq!(report.payees, 1_000);
    assert!(report.broken_limits.is_empty());
}

#[test]
fn stratum_load_grows_with_the_miners_on_a_node() {
    let few = Scenario {
        node_count: 10,
        miner_count: 100,
        duration_seconds: 600.0,
        ..Scenario::default()
    }
    .run();
    let many = Scenario {
        node_count: 10,
        miner_count: 1_000,
        duration_seconds: 600.0,
        ..Scenario::default()
    }
    .run();
    assert!(many.bandwidth.stratum_megabits_per_node > few.bandwidth.stratum_megabits_per_node);
}
