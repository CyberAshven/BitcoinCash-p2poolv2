// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;

#[test]
fn compensated_sum_keeps_small_terms() {
    assert_eq!(compensated_sum([1e16, 1.0, -1e16]), 1.0);
}

#[test]
fn compensated_sum_of_nothing_is_zero() {
    assert_eq!(compensated_sum([]), 0.0);
}

#[test]
fn summarize_empty_is_zero() {
    assert_eq!(
        summarize(&[]),
        Summary {
            count: 0,
            mean: 0.0,
            stddev: 0.0
        }
    );
}

#[test]
fn summarize_single_value_has_no_spread() {
    assert_eq!(
        summarize(&[2.5]),
        Summary {
            count: 1,
            mean: 2.5,
            stddev: 0.0
        }
    );
}

#[test]
fn summarize_uses_sample_standard_deviation() {
    let summary = summarize(&[1.0, 2.0, 3.0, 4.0]);
    assert_eq!(summary.count, 4);
    assert_eq!(summary.mean, 2.5);
    assert!((summary.stddev - 1.2909944487358056).abs() < 1e-15);
}

#[test]
fn truncated_percent_truncates() {
    assert_eq!(truncated_percent(2.0, 3.0), 66);
}

#[test]
fn truncated_percent_of_zero_whole_is_zero() {
    assert_eq!(truncated_percent(5.0, 0.0), 0);
}
