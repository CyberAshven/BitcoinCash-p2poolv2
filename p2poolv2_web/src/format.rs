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
    let mut scaled = if value == 0.0 { 0.0 } else { value };
    let mut unit = 0;
    while scaled.abs() >= 1000.0 && unit + 1 < UNITS.len() {
        scaled /= 1000.0;
        unit += 1;
    }
    (scaled, UNITS[unit])
}

/// Three significant digits, never printing a negative zero.
fn three_digits(value: f64) -> String {
    let value = if value < 0.0 && value.abs() < 0.005 {
        0.0
    } else {
        value
    };
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

/// A share's weak ratio `R` as the dashboard spells it: `R` itself, then
/// how many times the share chain's minimum target its weak target is.
/// `R` counts minimum targets in `WEAK_RATIO_UNIT` (1024) parts, whose
/// definition lives in `p2poolv2_lib`, which this crate does not depend
/// on; the style guard compares this with the pages' formatter.
pub fn fmt_weak_ratio(weak_ratio: u32) -> String {
    format!(
        "R = {weak_ratio} ({}x the minimum target)",
        three_digits(f64::from(weak_ratio) / 1024.0)
    )
}

/// The current block effort: the work accumulated since the last accepted
/// find as a percent of the network difficulty (whole percents, over 100%
/// meaning a block is overdue). The formula lives here, as it does in
/// Grafana. No difficulty yet is the dash, not a zero.
pub fn fmt_effort(work_since_last_block: Option<f64>, network_difficulty: Option<f64>) -> String {
    match (work_since_last_block, network_difficulty) {
        (Some(work), Some(difficulty)) if difficulty > 0.0 => {
            format!("{:.0}%", work / difficulty * 100.0)
        }
        _ => UNKNOWN.to_string(),
    }
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
pub(crate) fn civil_time(unix: u64) -> (i64, usize, u32, u64, u64) {
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

/// A peer's uptime, compact: 42 s, 13 min, 4h 12m, 2d 03h.
pub fn fmt_duration(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs} s"),
        60..=3_599 => format!("{} min", secs / 60),
        3_600..=86_399 => format!("{}h {:02}m", secs / 3_600, secs % 3_600 / 60),
        _ => format!("{}d {:02}h", secs / 86_400, secs % 86_400 / 3_600),
    }
}

/// A ping round trip in milliseconds: sub-millisecond values keep their
/// fraction, so a loopback peer reads 0.91 ms instead of a fabricated 0.
pub fn fmt_latency(latency_ms: f64) -> String {
    if latency_ms < 1.0 {
        format!("{latency_ms:.2} ms")
    } else if latency_ms < 10.0 {
        format!("{latency_ms:.1} ms")
    } else {
        format!("{latency_ms:.0} ms")
    }
}

/// A peer's version cell: Identify's agent_version arrives as
/// "p2poolv2/<semver>", so the cell shows just the version, or the dash
/// when the peer sent none (or an empty one).
pub fn agent_version(agent: Option<&str>) -> String {
    let version = agent.map(|agent| agent.split_once('/').map_or(agent, |(_, suffix)| suffix));
    match version {
        Some(version) if !version.is_empty() => version.to_string(),
        _ => UNKNOWN.to_string(),
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

/// A weak share's difficulty from its compact target, hex the way the
/// share lookup spells it (`0x1c0448d7`). Bits that do not parse render
/// as the dash, as a list row with no difficulty does.
pub fn weak_difficulty(bits: &str) -> String {
    let difficulty = u32::from_str_radix(bits.trim_start_matches("0x"), 16)
        .ok()
        .map(difficulty_from_bits);
    optional(difficulty, fmt_difficulty)
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
    fn test_hashrates_print_negative_zero_as_zero() {
        assert_eq!(fmt_hashrate(-0.0), "0.00 H/s");
        assert_eq!(fmt_hashrate(-0.001), "0.00 H/s");
    }

    #[test]
    fn test_difficulties_use_unit_prefixes() {
        assert_eq!(fmt_difficulty(665_151.95), "665k");
        assert_eq!(fmt_difficulty(4.5), "4.50");
    }

    #[test]
    fn test_difficulties_print_negative_zero_as_zero() {
        assert_eq!(fmt_difficulty(-0.0), "0.00");
        assert_eq!(fmt_difficulty(-0.001), "0.00");
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
    fn test_peer_uptimes_pick_a_compact_unit() {
        assert_eq!(fmt_duration(0), "0 s");
        assert_eq!(fmt_duration(42), "42 s");
        assert_eq!(fmt_duration(1_800), "30 min");
        assert_eq!(fmt_duration(8_280), "2h 18m");
        assert_eq!(fmt_duration(90_000), "1d 01h");
    }

    #[test]
    fn test_latencies_keep_sub_millisecond_precision() {
        assert_eq!(fmt_latency(0.0), "0.00 ms");
        assert_eq!(fmt_latency(0.914), "0.91 ms");
        assert_eq!(fmt_latency(5.44), "5.4 ms");
        assert_eq!(fmt_latency(42.6), "43 ms");
    }

    #[test]
    fn test_peer_versions_drop_the_agent_prefix() {
        assert_eq!(agent_version(Some("p2poolv2/0.0.1-alpha")), "0.0.1-alpha");
        assert_eq!(agent_version(Some("custom-agent")), "custom-agent");
        assert_eq!(agent_version(Some("p2poolv2/")), UNKNOWN);
        assert_eq!(agent_version(Some("")), UNKNOWN);
        assert_eq!(agent_version(None), UNKNOWN);
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
    fn test_weak_ratio_spells_out_its_multiple_of_the_minimum() {
        assert_eq!(
            fmt_weak_ratio(262_144),
            "R = 262144 (256x the minimum target)"
        );
        assert_eq!(fmt_weak_ratio(1024), "R = 1024 (1.00x the minimum target)");
    }

    #[test]
    fn test_weak_difficulty_reads_hex_bits_with_or_without_the_prefix() {
        assert_eq!(weak_difficulty("0x1c0448d7"), "59.7");
        assert_eq!(weak_difficulty("1d00ffff"), "1.00");
        assert_eq!(weak_difficulty("not hex"), UNKNOWN);
    }

    #[test]
    fn test_network_difficulty_inverts_the_hashrate_estimate() {
        let hashrate = 2.0 * HASHES_PER_DIFFICULTY / BLOCK_INTERVAL_SECONDS;
        assert!((difficulty_from_network_hashrate(hashrate) - 2.0).abs() < 1e-12);
    }
}
