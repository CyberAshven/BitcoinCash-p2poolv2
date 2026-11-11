// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;

#[test]
fn whole_pool_percentages_print_without_decimals() {
    assert_eq!(pool_percent(0.01), "1%");
    assert_eq!(pool_percent(0.3), "30%");
}

#[test]
fn fractional_pool_percentages_keep_two_decimals() {
    assert_eq!(pool_percent(0.015), "1.50%");
}

#[test]
fn a_missing_value_prints_a_dash() {
    assert_eq!(optional(None, two_decimals), "-");
    assert_eq!(optional(Some(0.456), two_decimals), "0.46");
}

#[test]
fn seconds_print_as_hours() {
    assert_eq!(hours(5_400.0), "1.5 h");
}

#[test]
fn a_markdown_table_has_a_header_a_rule_and_its_rows() {
    let rows = vec![
        vec!["1".to_string(), "2".to_string()],
        vec!["3".to_string(), "4".to_string()],
    ];
    assert_eq!(
        markdown_table(&["a", "b"], &rows),
        "| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n"
    );
}

#[test]
fn both_miners_headers_name_each_miner() {
    let headers = both_miner_headers();
    assert_eq!(headers.len(), 12);
    assert_eq!(headers[0], "Bitaxe pay/mo");
    assert_eq!(headers[6], "ASIC pay/mo");
}
