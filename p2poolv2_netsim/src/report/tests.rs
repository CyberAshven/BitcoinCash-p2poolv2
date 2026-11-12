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
    assert_eq!(report.payees, 1_001);
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

#[test]
fn header_sync_counts_each_chain_share_and_each_named_uncle_with_their_uncle_hashes() {
    let outcome = Outcome {
        shares_found: 12,
        shares_counted: 12,
        chain_shares: 10,
        uncle_shares: 2,
        orphan_shares: 0,
        stale_rate: 2.0 / 12.0,
        uncle_rate: 2.0 / 12.0,
        orphan_rate: 0.0,
        mean_seconds_to_half: 0.0,
        mean_seconds_to_ninety_percent: 0.0,
        mean_seconds_to_all: 0.0,
        share_messages: 0,
        duplicate_share_messages: 0,
        node_upload_bytes_per_second: Vec::new(),
        node_download_bytes_per_second: Vec::new(),
        node_validation_busy_fraction: Vec::new(),
        mean_validation_wait_seconds: 0.0,
        saturated_nodes: 0,
        weak_shares_found: 0,
        weak_share_messages: 0,
        chain_length: 10,
        uncles_named_by_chain: 2,
        uncle_hashes_in_named_uncles: 1,
    };
    // (10 + 2) headers of 305 bytes and 2 + 1 uncle hashes of 32 bytes,
    // over 10 chain shares.
    let (uncles, bytes) = header_sync_per_chain_share(&outcome, 305);
    assert_eq!(uncles, 0.2);
    assert!((bytes - 375.6).abs() < 1e-9, "bytes {bytes}");
}

#[test]
fn an_empty_chain_costs_no_header_sync() {
    let outcome = Outcome {
        shares_found: 0,
        shares_counted: 0,
        chain_shares: 0,
        uncle_shares: 0,
        orphan_shares: 0,
        stale_rate: 0.0,
        uncle_rate: 0.0,
        orphan_rate: 0.0,
        mean_seconds_to_half: 0.0,
        mean_seconds_to_ninety_percent: 0.0,
        mean_seconds_to_all: 0.0,
        share_messages: 0,
        duplicate_share_messages: 0,
        node_upload_bytes_per_second: Vec::new(),
        node_download_bytes_per_second: Vec::new(),
        node_validation_busy_fraction: Vec::new(),
        mean_validation_wait_seconds: 0.0,
        saturated_nodes: 0,
        weak_shares_found: 0,
        weak_share_messages: 0,
        chain_length: 0,
        uncles_named_by_chain: 0,
        uncle_hashes_in_named_uncles: 0,
    };
    assert_eq!(header_sync_per_chain_share(&outcome, 305), (0.0, 0.0));
}

#[test]
fn a_scaling_row_has_as_many_cells_as_its_header() {
    let scenario = Scenario {
        node_count: 10,
        duration_seconds: 600.0,
        weak_shares_per_share: 4,
        ..Scenario::default()
    };
    let report = scenario.run();
    let header_cells = scaling_markdown_header()
        .lines()
        .next()
        .map(|line| line.matches('|').count());
    let row_cells = scaling_markdown_row(&report).matches('|').count();
    assert_eq!(header_cells, Some(row_cells));
    assert!(report.propagation.weak_shares_found > 0);
}
