// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Display formatting, the same as the pages the node renders.
//!
//! Every function here prints what its namesake in
//! `p2poolv2_api/src/api/dashboard/pages.rs` prints, so both dashboards show
//! the same text. The API crate's style guard compiles this file and checks
//! that. It uses only `std`, so it builds in both crates.

const UNITS: [&str; 7] = ["", "k", "M", "G", "T", "P", "E"];

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Hashes a difficulty-1 share takes on average.
pub const HASHES_PER_DIFFICULTY: f64 = 4_294_967_296.0;

/// Seconds between Bitcoin Cash blocks the network difficulty targets.
pub const BLOCK_INTERVAL_SECONDS: f64 = 600.0;

/// Shown where a value is not known.
pub const UNKNOWN: &str = "\u{2014}";

/// `value` scaled to a unit prefix.
fn with_prefix(value: f64) -> (f64, &'static str) {
    let mut scaled = value;
    let mut unit = 0;
    while scaled.abs() >= 1000.0 && unit + 1 < UNITS.len() {
        scaled /= 1000.0;
        unit += 1;
    }
    (scaled, UNITS[unit])
}

/// Three significant digits.
fn three_digits(value: f64) -> String {
    if value.abs() >= 100.0 {
        format!("{value:.0}")
    } else if value.abs() >= 10.0 {
        format!("{value:.1}")
    } else {
        format!("{value:.2}")
    }
}

/// A hashrate with a unit prefix, e.g. `2.40 TH/s`.
pub fn fmt_hashrate(hashes_per_second: f64) -> String {
    let (value, unit) = with_prefix(hashes_per_second);
    format!("{} {unit}H/s", three_digits(value))
}

/// A difficulty with a unit prefix, e.g. `665k`.
pub fn fmt_difficulty(difficulty: f64) -> String {
    let (value, unit) = with_prefix(difficulty);
    format!("{}{unit}", three_digits(value))
        .trim_end()
        .to_string()
}

/// Days since 1970-01-01 to a civil (year, month, day), Howard Hinnant's
/// algorithm.
fn civil_from_days(days: i64) -> (i64, usize, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    } as usize;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// (year, month 1-12, day, hour, minute) of a unix time, in UTC.
fn civil_time(unix: u64) -> (i64, usize, u32, u64, u64) {
    let days = i64::try_from(unix / 86_400).unwrap_or(i64::MAX / 2);
    let (year, month, day) = civil_from_days(days);
    let seconds = unix % 86_400;
    (year, month, day, seconds / 3600, seconds % 3600 / 60)
}

/// `YYYY-MM-DD HH:MM UTC`.
pub fn fmt_utc(unix: u64) -> String {
    let (year, month, day, hour, minute) = civil_time(unix);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02} UTC")
}

/// Day and time, short enough for a chart axis that spans a day, e.g.
/// `Oct 14 17:46 UTC`.
pub fn fmt_day_time(unix: u64) -> String {
    let (_, month, day, hour, minute) = civil_time(unix);
    format!("{} {day} {hour:02}:{minute:02} UTC", MONTHS[month - 1])
}

/// How long ago `then` was, e.g. `15 min ago`.
pub fn fmt_ago(now: u64, then: u64) -> String {
    let seconds = now.saturating_sub(then);
    match seconds {
        0..=59 => format!("{seconds} s ago"),
        60..=3_599 => format!("{} min ago", seconds / 60),
        3_600..=86_399 => format!("{} h ago", seconds / 3_600),
        _ => format!("{} d ago", seconds / 86_400),
    }
}

/// A long hash or address cut to its ends.
pub fn short(hash: &str) -> String {
    let length = hash.chars().count();
    if length <= 20 {
        return hash.to_string();
    }
    let head: String = hash.chars().take(10).collect();
    let tail: String = hash.chars().skip(length - 6).collect();
    format!("{head}\u{2026}{tail}")
}

/// `value` formatted, or a dash when it is not known.
pub fn optional(value: Option<f64>, format: fn(f64) -> String) -> String {
    value.map(format).unwrap_or_else(|| UNKNOWN.to_string())
}

/// A fraction as a percentage with two decimals.
pub fn percent(fraction: f64) -> String {
    format!("{:.2}%", fraction * 100.0)
}

/// Percent-encode anything outside the unreserved set, for a query or path
/// component.
pub fn url_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~' | b':') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// Undo [`url_component`] and form encoding (`+` for a space). A broken
/// escape is kept as it is.
pub fn url_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let escaped = (bytes[index] == b'%')
            .then(|| value.get(index + 1..index + 3))
            .flatten()
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match (bytes[index], escaped) {
            (_, Some(byte)) => {
                decoded.push(byte);
                index += 3;
            }
            (b'+', None) => {
                decoded.push(b' ');
                index += 1;
            }
            (byte, None) => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

/// Difficulty of a compact target (`bits`), relative to difficulty 1
/// (`0x1d00ffff`), as Bitcoin's `Target::difficulty_float` computes it.
pub fn difficulty_from_bits(bits: u32) -> f64 {
    let exponent = i32::try_from(bits >> 24).unwrap_or(0);
    let mantissa = f64::from(bits & 0x007f_ffff);
    if mantissa == 0.0 {
        return 0.0;
    }
    f64::from(0xffff_u32) / mantissa * 256_f64.powi(0x1d - exponent)
}

/// Hashes a share of compact target `bits` takes on average.
pub fn work_from_bits(bits: u32) -> f64 {
    difficulty_from_bits(bits) * HASHES_PER_DIFFICULTY
}

/// Block difficulty implied by a network hashrate, the inverse of how the
/// node estimates the hashrate from the difficulty.
pub fn difficulty_from_network_hashrate(hashes_per_second: f64) -> f64 {
    hashes_per_second * BLOCK_INTERVAL_SECONDS / HASHES_PER_DIFFICULTY
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hashrates_use_unit_prefixes() {
        assert_eq!(fmt_hashrate(0.0), "0.00 H/s");
        assert_eq!(fmt_hashrate(2_400_000_000_000.0), "2.40 TH/s");
        assert_eq!(fmt_hashrate(38_790_000_000_000.0), "38.8 TH/s");
        assert_eq!(fmt_hashrate(665_000.0), "665 kH/s");
    }

    #[test]
    fn test_difficulties_use_unit_prefixes() {
        assert_eq!(fmt_difficulty(665_151.95), "665k");
        assert_eq!(fmt_difficulty(4.5), "4.50");
    }

    #[test]
    fn test_times_are_utc() {
        assert_eq!(fmt_utc(0), "1970-01-01 00:00 UTC");
        assert_eq!(fmt_utc(1_792_675_942), "2026-10-22 13:32 UTC");
        assert_eq!(fmt_day_time(1_792_000_000), "Oct 14 17:46 UTC");
        assert_eq!(fmt_day_time(1_772_323_200), "Mar 1 00:00 UTC");
    }

    #[test]
    fn test_ages_pick_a_readable_unit() {
        assert_eq!(fmt_ago(100, 58), "42 s ago");
        assert_eq!(fmt_ago(1_000, 100), "15 min ago");
        assert_eq!(fmt_ago(20_000, 0), "5 h ago");
        assert_eq!(fmt_ago(200_000, 0), "2 d ago");
        assert_eq!(fmt_ago(0, 100), "0 s ago");
    }

    #[test]
    fn test_long_hashes_are_shortened_on_characters() {
        assert_eq!(short("abc"), "abc");
        let hash = "00000000000cb752459c2b08e1955e5f33252b0207b1c7b5b2f069d5254b9025";
        assert_eq!(short(hash), "0000000000\u{2026}4b9025");
        assert_eq!(
            short(&"\u{e9}".repeat(30)),
            format!("{}\u{2026}{}", "\u{e9}".repeat(10), "\u{e9}".repeat(6))
        );
    }

    #[test]
    fn test_url_components_round_trip() {
        assert_eq!(url_component("bchtest:qr3rryn6"), "bchtest:qr3rryn6");
        assert_eq!(url_component("a&b=c d"), "a%26b%3Dc%20d");
        assert_eq!(url_decode("a%26b%3Dc%20d"), "a&b=c d");
        assert_eq!(url_decode("a+b%2"), "a b%2");
    }

    #[test]
    fn test_difficulty_one_bits() {
        assert!((difficulty_from_bits(0x1d00_ffff) - 1.0).abs() < 1e-12);
        assert!((difficulty_from_bits(0x1c00_ffff) - 256.0).abs() < 1e-9);
        assert_eq!(difficulty_from_bits(0x1d00_0000), 0.0);
        assert!((work_from_bits(0x1d00_ffff) - HASHES_PER_DIFFICULTY).abs() < 1e-3);
    }

    #[test]
    fn test_network_difficulty_inverts_the_hashrate_estimate() {
        let hashrate = 2.0 * HASHES_PER_DIFFICULTY / BLOCK_INTERVAL_SECONDS;
        assert!((difficulty_from_network_hashrate(hashrate) - 2.0).abs() < 1e-12);
    }
}
