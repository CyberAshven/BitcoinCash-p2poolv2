// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Timestamps in node log lines.
//!
//! The node logs through `tracing`, which starts each line with an RFC 3339
//! UTC timestamp such as `2026-10-19T12:34:56.123456Z`. The tools only ever
//! compare timestamps with each other, so they are kept as microseconds
//! since the Unix epoch.

use chrono::NaiveDateTime;
use regex::Regex;
use std::sync::LazyLock;

static TIMESTAMP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d+)").expect("valid regex")
});

/// Microseconds since the epoch for `text`, a `YYYY-MM-DDTHH:MM:SS.fraction`
/// timestamp without a zone. Digits past the sixth are dropped, not rounded.
pub fn parse_micros(text: &str) -> Option<i64> {
    let (seconds_part, fraction) = text.split_once('.')?;
    let whole = NaiveDateTime::parse_from_str(seconds_part, "%Y-%m-%dT%H:%M:%S").ok()?;
    if fraction.is_empty() || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let micros = fraction
        .bytes()
        .chain(std::iter::repeat(b'0'))
        .take(6)
        .fold(0i64, |total, digit| total * 10 + i64::from(digit - b'0'));
    whole
        .and_utc()
        .timestamp()
        .checked_mul(1_000_000)?
        .checked_add(micros)
}

/// The first timestamp anywhere in `line`, in microseconds since the epoch.
pub fn find_micros(line: &str) -> Option<i64> {
    let found = TIMESTAMP.captures(line)?;
    parse_micros(&found[1])
}

#[cfg(test)]
mod tests;
