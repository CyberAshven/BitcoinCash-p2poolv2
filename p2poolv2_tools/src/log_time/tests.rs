// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;

#[test]
fn parse_micros_reads_six_digits() {
    assert_eq!(parse_micros("1970-01-01T00:00:01.000002"), Some(1_000_002));
}

#[test]
fn parse_micros_pads_short_fractions() {
    assert_eq!(parse_micros("1970-01-01T00:00:00.5"), Some(500_000));
}

#[test]
fn parse_micros_truncates_long_fractions() {
    assert_eq!(parse_micros("1970-01-01T00:00:00.1234569"), Some(123_456));
}

#[test]
fn parse_micros_rejects_missing_fraction() {
    assert_eq!(parse_micros("1970-01-01T00:00:00"), None);
}

#[test]
fn find_micros_finds_a_timestamp_inside_a_line() {
    assert_eq!(
        find_micros("\x1b[2m1970-01-01T00:01:00.25Z\x1b[0m INFO hello"),
        Some(60_250_000)
    );
}

#[test]
fn find_micros_is_none_without_a_timestamp() {
    assert_eq!(find_micros("no time here"), None);
}
