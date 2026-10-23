// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;

fn series_with_promotions(times: &[f64]) -> Series {
    Series {
        promotions: times.to_vec(),
        ..Series::default()
    }
}

#[test]
fn time_axis_uses_forty_bins_of_at_least_five_seconds() {
    let axis = TimeAxis::of(&series_with_promotions(&[100.0, 500.0])).expect("axis");
    assert_eq!(axis.start, 100.0);
    assert_eq!(axis.span, 400.0);
    assert_eq!(axis.bin_seconds, 10.0);
    assert_eq!(axis.bins, 41);
}

#[test]
fn short_runs_get_five_second_bins() {
    let axis = TimeAxis::of(&series_with_promotions(&[0.0, 20.0])).expect("axis");
    assert_eq!(axis.bin_seconds, 5.0);
    assert_eq!(axis.bins, 5);
}

#[test]
fn no_events_has_no_axis() {
    assert!(TimeAxis::of(&Series::default()).is_none());
}

#[test]
fn cumulative_uncle_rate_counts_uncles_up_to_each_block() {
    let series = Series {
        promotions: vec![0.0, 10.0, 20.0],
        uncle_blocks: vec![(10.0, 1.0)],
        ..Series::default()
    };
    let axis = TimeAxis::of(&series).expect("axis");
    let rates = rates(&series, &axis);
    assert_eq!(rates.cumulative_uncle_rate, vec![0.0, 100.0 / 3.0, 25.0]);
    assert_eq!(rates.cumulative_share_rate, vec![0.0, 0.2, 0.15]);
}

#[test]
fn format_general_matches_printf_g() {
    assert_eq!(format_general(0.1, 3), "0.1");
    assert_eq!(format_general(1.0, 3), "1");
    assert_eq!(format_general(1.0 / 3.0, 3), "0.333");
}
