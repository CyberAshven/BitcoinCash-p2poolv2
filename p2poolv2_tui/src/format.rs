// SPDX-FileCopyrightText: 2024 PDM Authors
// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Formatting for numbers, times and hashes on screen.
//!
//! `short`, `with_commas`, `difficulty_from_bits` and the calendar maths are
//! ported from PDM's P2Pool status view. Hashrates and times read like the
//! node's own dashboard: SI prefixes with three significant digits, and UTC.

/// Shown where a value is not known.
pub const UNKNOWN: &str = "-";

/// SI prefixes, each 1000 times the one before.
const SI_PREFIXES: [&str; 7] = ["", "k", "M", "G", "T", "P", "E"];

/// Difficulty 1 is this many hashes on average.
const HASHES_PER_DIFFICULTY: f64 = 4_294_967_296.0;

/// `value` divided down to the largest SI prefix that keeps it at or above
/// 1, with that prefix.
fn with_prefix(value: f64) -> (f64, &'static str) {
    let mut scaled = value;
    let mut index = 0;
    while scaled.abs() >= 1000.0 && index + 1 < SI_PREFIXES.len() {
        scaled /= 1000.0;
        index += 1;
    }
    (scaled, SI_PREFIXES[index])
}

/// Three significant digits: `1.23`, `12.3`, `123`.
fn three_digits(value: f64) -> String {
    let magnitude = value.abs();
    if magnitude >= 100.0 {
        format!("{value:.0}")
    } else if magnitude >= 10.0 {
        format!("{value:.1}")
    } else {
        format!("{value:.2}")
    }
}

/// A hashrate, e.g. `1.23 TH/s`.
pub fn hashrate(hashes_per_second: f64) -> String {
    if !hashes_per_second.is_finite() || hashes_per_second < 0.0 {
        return UNKNOWN.to_string();
    }
    let (value, prefix) = with_prefix(hashes_per_second);
    format!("{} {prefix}H/s", three_digits(value))
}

/// A difficulty, e.g. `512`, `1.23 M`.
pub fn difficulty(value: f64) -> String {
    if !value.is_finite() || value < 0.0 {
        return UNKNOWN.to_string();
    }
    let (scaled, prefix) = with_prefix(value);
    if prefix.is_empty() {
        three_digits(scaled)
    } else {
        format!("{} {prefix}", three_digits(scaled))
    }
}

/// Difficulty of a compact target, relative to difficulty 1 (`0x1d00ffff`),
/// which is the same on Bitcoin Cash. `None` for a zero mantissa.
pub fn difficulty_from_bits(bits: u32) -> Option<f64> {
    let exponent = i32::try_from(bits >> 24).ok()?;
    let mantissa = bits & 0x00ff_ffff;
    if mantissa == 0 {
        return None;
    }
    let difficulty =
        (f64::from(0x0000_ffff_u32) / f64::from(mantissa)) * 256_f64.powi(0x1d - exponent);
    (difficulty.is_finite() && difficulty > 0.0).then_some(difficulty)
}

/// The average hashes behind one share at difficulty `bits`.
pub fn work_from_bits(bits: u32) -> Option<f64> {
    difficulty_from_bits(bits).map(|difficulty| difficulty * HASHES_PER_DIFFICULTY)
}

/// Unix time as `2026-10-23 12:34:56 UTC`.
pub fn utc(unix_seconds: u64) -> String {
    let days = i64::try_from(unix_seconds / 86_400).unwrap_or(i64::MAX / 2);
    let seconds_of_day = unix_seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC",
        seconds_of_day / 3_600,
        (seconds_of_day % 3_600) / 60,
        seconds_of_day % 60
    )
}

/// How long ago `then` was at `now`, both Unix seconds, e.g. `5 min ago`.
/// Zero means it never happened.
pub fn ago(now: u64, then: u64) -> String {
    if then == 0 {
        return "never".to_string();
    }
    format!("{} ago", duration(now.saturating_sub(then)))
}

/// A duration, e.g. `45 s`, `12 min`, `3 h 20 min`, `2 d 4 h`.
pub fn duration(seconds: u64) -> String {
    match seconds {
        0..=59 => format!("{seconds} s"),
        60..=3_599 => format!("{} min", seconds / 60),
        3_600..=86_399 => format!("{} h {} min", seconds / 3_600, (seconds % 3_600) / 60),
        _ => format!("{} d {} h", seconds / 86_400, (seconds % 86_400) / 3_600),
    }
}

/// A size in bytes with an SI prefix, e.g. `1.23 MB`.
pub fn bytes(count: u64) -> String {
    let (value, prefix) = with_prefix(count as f64);
    if prefix.is_empty() {
        format!("{count} B")
    } else {
        format!("{} {prefix}B", three_digits(value))
    }
}

/// A fraction as a percentage, e.g. `12.34%`.
pub fn percent(fraction: f64) -> String {
    format!("{:.2}%", fraction * 100.0)
}

/// A fee rate in BCH per kilobyte, as BCHN reports it.
pub fn bch_per_kb(rate: f64) -> String {
    format!("{rate:.8} BCH/kB")
}

/// `yes`, `no`, or unknown.
pub fn yes_no(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "yes",
        Some(false) => "no",
        None => UNKNOWN,
    }
}

/// `format(value)`, or unknown when there is none.
pub fn optional<T>(value: Option<T>, format: impl FnOnce(T) -> String) -> String {
    value.map_or_else(|| UNKNOWN.to_string(), format)
}

/// An integer with thousands separators, e.g. `1,234,567`.
pub fn with_commas(value: u64) -> String {
    let digits = value.to_string();
    let mut formatted = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            formatted.push(',');
        }
        formatted.push(digit);
    }
    formatted
}

/// `value` cut to at most `max_len` characters by keeping its head and tail
/// around `...`, as hashes and addresses are shown.
pub fn short(value: &str, max_len: usize) -> String {
    let length = value.chars().count();
    if length <= max_len {
        return value.to_string();
    }
    if max_len <= 3 {
        return value.chars().take(max_len).collect();
    }
    let head_len = (max_len - 3) / 2;
    let tail_len = max_len - 3 - head_len;
    let head: String = value.chars().take(head_len).collect();
    let tail: String = value.chars().skip(length - tail_len).collect();
    format!("{head}...{tail}")
}

/// `text` with every double-quoted string replaced by `"..."`. Parser
/// errors quote the value they could not take, and that value could be a
/// password.
pub fn without_quoted(text: &str) -> String {
    let mut cleaned = String::with_capacity(text.len());
    let mut inside = false;
    for character in text.chars() {
        match (character, inside) {
            ('"', false) => {
                cleaned.push_str("\"...\"");
                inside = true;
            }
            ('"', true) => inside = false,
            (_, false) => cleaned.push(character),
            (_, true) => {}
        }
    }
    cleaned
}

/// Year, month and day of a count of days since 1970-01-01, by Howard
/// Hinnant's `civil_from_days`.
fn civil_from_days(days_since_epoch: i64) -> (i64, u32, u32) {
    let days = days_since_epoch + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_param = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_param + 2) / 5 + 1;
    let month = if month_param < 10 {
        month_param + 3
    } else {
        month_param - 9
    };
    let year = if month <= 2 { year + 1 } else { year };
    (
        year,
        u32::try_from(month).unwrap_or(1),
        u32::try_from(day).unwrap_or(1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hashrate_uses_si_prefixes() {
        assert_eq!(hashrate(1_234_000_000_000.0), "1.23 TH/s");
        assert_eq!(hashrate(56_700.0), "56.7 kH/s");
        assert_eq!(hashrate(999.0), "999 H/s");
    }

    #[test]
    fn test_hashrate_of_nothing_is_unknown() {
        assert_eq!(hashrate(f64::NAN), UNKNOWN);
        assert_eq!(hashrate(-1.0), UNKNOWN);
    }

    #[test]
    fn test_difficulty_without_and_with_prefix() {
        assert_eq!(difficulty(512.0), "512");
        assert_eq!(difficulty(1_230_000.0), "1.23 M");
    }

    #[test]
    fn test_difficulty_from_bits_of_difficulty_one() {
        assert_eq!(difficulty_from_bits(0x1d00_ffff), Some(1.0));
    }

    #[test]
    fn test_difficulty_from_bits_of_a_harder_target() {
        let difficulty = difficulty_from_bits(0x1b38_4bd7).unwrap();
        assert!((difficulty - 1_164.11).abs() < 0.01, "{difficulty}");
    }

    #[test]
    fn test_difficulty_from_bits_of_a_zero_mantissa() {
        assert_eq!(difficulty_from_bits(0x1d00_0000), None);
    }

    #[test]
    fn test_work_from_bits_of_difficulty_one() {
        assert_eq!(work_from_bits(0x1d00_ffff), Some(HASHES_PER_DIFFICULTY));
    }

    #[test]
    fn test_utc_formats_the_epoch() {
        assert_eq!(utc(0), "1970-01-01 00:00:00 UTC");
    }

    #[test]
    fn test_utc_formats_a_leap_day() {
        assert_eq!(utc(1_709_210_096), "2024-02-29 12:34:56 UTC");
    }

    #[test]
    fn test_ago_in_each_unit() {
        assert_eq!(ago(1_000, 970), "30 s ago");
        assert_eq!(ago(10_000, 9_400), "10 min ago");
        assert_eq!(ago(100_000, 100_000 - 7_500), "2 h 5 min ago");
        assert_eq!(ago(1_000_000, 1_000_000 - 180_000), "2 d 2 h ago");
    }

    #[test]
    fn test_ago_of_zero_is_never() {
        assert_eq!(ago(1_000, 0), "never");
    }

    #[test]
    fn test_ago_of_a_future_time_is_zero() {
        assert_eq!(ago(100, 200), "0 s ago");
    }

    #[test]
    fn test_bytes_below_and_above_a_kilobyte() {
        assert_eq!(bytes(999), "999 B");
        assert_eq!(bytes(1_234_567), "1.23 MB");
    }

    #[test]
    fn test_percent_has_two_decimals() {
        assert_eq!(percent(0.123_456), "12.35%");
    }

    #[test]
    fn test_bch_per_kb_has_eight_decimals() {
        assert_eq!(bch_per_kb(0.00001), "0.00001000 BCH/kB");
    }

    #[test]
    fn test_yes_no_covers_unknown() {
        assert_eq!(yes_no(Some(true)), "yes");
        assert_eq!(yes_no(Some(false)), "no");
        assert_eq!(yes_no(None), UNKNOWN);
    }

    #[test]
    fn test_optional_formats_or_shows_unknown() {
        assert_eq!(optional(Some(5_u64), |value| value.to_string()), "5");
        assert_eq!(optional(None::<u64>, |value| value.to_string()), UNKNOWN);
    }

    #[test]
    fn test_with_commas_groups_thousands() {
        assert_eq!(with_commas(0), "0");
        assert_eq!(with_commas(999), "999");
        assert_eq!(with_commas(1_234_567), "1,234,567");
    }

    #[test]
    fn test_short_keeps_short_values() {
        assert_eq!(short("abcdef", 10), "abcdef");
    }

    #[test]
    fn test_short_keeps_head_and_tail() {
        assert_eq!(short("0123456789abcdef", 10), "012...cdef");
    }

    #[test]
    fn test_without_quoted_hides_quoted_values() {
        assert_eq!(
            without_quoted(r#"invalid type: string "hunter2", expected u16"#),
            r#"invalid type: string "...", expected u16"#
        );
    }

    #[test]
    fn test_without_quoted_keeps_backticked_names() {
        assert_eq!(
            without_quoted("missing field `bootstrap_address`"),
            "missing field `bootstrap_address`"
        );
    }

    #[test]
    fn test_short_with_no_room_for_dots_truncates() {
        assert_eq!(short("0123456789", 3), "012");
    }
}
