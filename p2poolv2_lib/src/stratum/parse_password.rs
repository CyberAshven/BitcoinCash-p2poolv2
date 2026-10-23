// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Parse stratum password field for difficulty hints and the miner share address.
//!
//! Miners can specify options via the password field in mining.authorize:
//! - `d=<integer>` sets the difficulty directly
//! - `th=<integer>` specifies terahash/s, converted to difficulty
//! - `p2p=<miner share address>` names the owner of the miner's share coinbase outputs
//!
//! If both `d=` and `th=` are present, `d=` takes priority.
//!
//! This module only extracts values. Policy lives with the caller: difficulty
//! clamping and the share address network check both happen at authorize.
//!
//! We manually parse the key value pairs. If we add more params we can switch
//! to regex when needed.

use crate::address::{Address, AddressError};
use crate::stratum::difficulty_adjuster::TARGET_DRR;
use std::str::FromStr;

/// Result of parsing the password field.
pub struct ParsedPassword {
    /// The optional difficulty supplied by a miner connecting to node's stratum
    /// server.
    pub difficulty: Option<u64>,

    /// The share chain address from `p2p=`, in three distinguishable states:
    /// `None` when the key is absent, `Some(Err)` when it was supplied but
    /// unparseable, and `Some(Ok)` when it is a well formed address.
    ///
    /// The distinction matters because authorize rejects a P2Poolv2 mode miner
    /// without an address, and telling someone who typoed their address that
    /// they supplied none would send them looking in the wrong place.
    pub miner_address: Option<Result<Address, AddressError>>,
}

/// Parse the password string for `d=`, `th=` and `p2p=` options.
///
/// Scans the password for recognized options anywhere in the string,
/// separated by commas, spaces, or at string boundaries.
/// `d=` (explicit difficulty) takes priority over `th=` (terahash/s).
pub fn parse_password(password: &str) -> ParsedPassword {
    ParsedPassword {
        difficulty: parse_difficulty(password),
        miner_address: extract_string_value(password, "p2p=").map(Address::from_str),
    }
}

/// Resolve the session start difficulty from `d=`, falling back to `th=`.
fn parse_difficulty(password: &str) -> Option<u64> {
    let difficulty_value = extract_value(password, "d=");
    if let Some(value) = difficulty_value
        && value > 0
    {
        return Some(value);
    }

    let terahash_value = extract_value(password, "th=");
    if let Some(value) = terahash_value
        && value > 0
    {
        let difficulty = terahash_to_difficulty(value);
        if difficulty > 0 {
            return Some(difficulty);
        }
    }

    None
}

/// Extract a string value for a given key (e.g. "p2p=") from the password.
///
/// Follows the same boundary rules as [`extract_value`]: the key may appear at
/// the start of the string or after a comma, space, tab or semicolon. The value
/// runs to the next such delimiter or to the end of the string.
///
/// Unlike [`extract_value`] this matches the key case insensitively against the
/// original string rather than scanning a lowercased copy, and returns a slice
/// of the original. Lowercasing first would silently repair a mixed case bech32
/// address, which BIP173 requires be rejected because mixed case defeats the
/// checksum, and `str::to_lowercase` can change byte length on non-ASCII input
/// and desynchronise the offsets.
pub(crate) fn extract_string_value<'a>(password: &'a str, key: &str) -> Option<&'a str> {
    let bytes = password.as_bytes();
    let key_bytes = key.as_bytes();
    if key_bytes.is_empty() || bytes.len() < key_bytes.len() {
        return None;
    }

    for position in 0..=bytes.len() - key_bytes.len() {
        if !bytes[position..position + key_bytes.len()].eq_ignore_ascii_case(key_bytes) {
            continue;
        }

        let at_valid_boundary =
            position == 0 || matches!(bytes[position - 1], b',' | b' ' | b'\t' | b';');
        if !at_valid_boundary {
            continue;
        }

        let value_start = position + key_bytes.len();
        let value_end = bytes[value_start..]
            .iter()
            .position(|byte| matches!(byte, b',' | b' ' | b'\t' | b';'))
            .map_or(bytes.len(), |offset| value_start + offset);

        if value_end > value_start {
            return Some(&password[value_start..value_end]);
        }
    }

    None
}

/// Extract an integer value for a given key (e.g., "d=" or "th=") from the password string.
///
/// The key can appear at the start of the string, or after a comma or space.
/// The value is the sequence of digits immediately following the key.
fn extract_value(password: &str, key: &str) -> Option<u64> {
    let password_lower_case = password.to_lowercase();
    let mut search_from = 0;

    while search_from < password_lower_case.len() {
        let position = {
            let pos = password_lower_case[search_from..].find(key)?;
            search_from + pos
        };

        let at_valid_boundary = position == 0
            || matches!(
                password_lower_case.as_bytes()[position - 1],
                b',' | b' ' | b'\t' | b';'
            );

        if at_valid_boundary {
            let digits_start = position + key.len();
            let digits: String = password_lower_case[digits_start..]
                .chars()
                .take_while(|character| character.is_ascii_digit())
                .collect();

            if !digits.is_empty() {
                return digits.parse::<u64>().ok();
            }
        }

        search_from = position + key.len();
    }

    None
}

/// Convert terahash/s to difficulty.
///
/// Uses the formula: difficulty = hashrate_hps / (TARGET_DRR * 2^32)
/// where hashrate_hps = terahash * 10^12
///
/// Note that 1/TARGET_DRR gives us target share rate.
fn terahash_to_difficulty(terahash: u64) -> u64 {
    let hashrate_hps = terahash as f64 * 1e12;
    let difficulty = hashrate_hps / (TARGET_DRR * 4_294_967_296.0);
    difficulty as u64
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod p2p_address_tests;
