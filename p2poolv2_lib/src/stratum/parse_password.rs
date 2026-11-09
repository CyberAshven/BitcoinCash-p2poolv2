// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Parse a miner's login options: difficulty hints, the share address and
//! the P2Pool donation.
//!
//! Miners give options in the SV1 password, after a comma in the SV1
//! username, or after a comma in the SV2 identity:
//! - `d=<integer>` sets the difficulty directly
//! - `th=<integer>` specifies terahash/s, converted to difficulty
//! - `p2p=<miner share address>` names the owner of the miner's share coinbase outputs
//! - `donation=<percent>` chooses the miner's own P2Pool donation on Bitcoin
//!   Cash, 0 to 10 in steps of 0.5 (see [`parse_miner_donation`])
//! - `min-payout=<satoshis>` and `fee-tolerance=<percent>` choose the miner's
//!   payout level on Bitcoin Cash (see [`parse_miner_payout_level`])
//!
//! If both `d=` and `th=` are present, `d=` takes priority, and `min-payout=`
//! takes priority over `fee-tolerance=`. Keys P2Pool does not read, such as
//! Pickaxe's own `pickaxe-donation=`, are ignored, so one login works at both
//! kinds of pool.
//!
//! This module only extracts values. Policy lives with the caller: difficulty
//! clamping and the share address network check both happen at authorize.
//!
//! We manually parse the key value pairs. If we add more params we can switch
//! to regex when needed.

use crate::address::{Address, AddressError};
use crate::config::{Chain, PoolMode};
use crate::stratum::difficulty_adjuster::TARGET_DRR;
use crate::stratum::work::level_policy::PayoutLevelChoice;
use p2poolv2_config::MAX_BITCOIN_CASH_CUT;
use std::str::FromStr;
use tracing::debug;

/// What separates login options from each other.
const OPTION_SEPARATORS: [char; 4] = [',', ' ', '\t', ';'];

/// The option keys P2Pool reads. Any other key is ignored.
const KNOWN_OPTION_KEYS: [&str; 6] = ["d", "th", "p2p", "donation", "min-payout", "fee-tolerance"];

/// The largest `fee-tolerance=`: 100%, in basis points.
const MAX_FEE_TOLERANCE: u32 = 10_000;

/// A miner's donation moves in steps of 0.5%, 50 basis points, from 0 up to
/// the donation's own cap, [`MAX_BITCOIN_CASH_CUT`].
pub(crate) const MINER_DONATION_STEP: u16 = 50;

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

/// Split an SV1 username or SV2 identity into the account and its options.
///
/// The options start at the first token, after a comma, space, tab or
/// semicolon (the separators of the SV1 password options), that holds a
/// `key=value` pair, as in `bitcoincash:q...rig1,donation=0.5`. Everything
/// before that separator is the account, so a worker name containing a space
/// keeps it, as before options existed. An identity without such a token is
/// all account, with no options.
pub(crate) fn split_user_identity(user_identity: &str) -> (&str, Option<&str>) {
    for (separator, _) in user_identity.match_indices(OPTION_SEPARATORS) {
        let rest = &user_identity[separator + 1..];
        let token_end = rest.find(OPTION_SEPARATORS).unwrap_or(rest.len());
        if rest[..token_end].contains('=') {
            return (&user_identity[..separator], Some(rest));
        }
    }
    (user_identity, None)
}

/// The options of an SV1 login: those after the username, then the password.
pub(crate) fn join_login_options(
    username_options: Option<&str>,
    password: Option<&str>,
) -> Option<String> {
    match (username_options, password) {
        (None, None) => None,
        (Some(options), None) | (None, Some(options)) => Some(options.to_string()),
        (Some(username_options), Some(password)) => Some(format!("{username_options},{password}")),
    }
}

/// Log, at debug level only, the `key=value` options P2Pool does not read.
///
/// They are not errors: a login shared with another pool, such as Pickaxe,
/// may carry that pool's own keys.
pub(crate) fn log_unknown_options(options: &str) {
    for option in options.split(OPTION_SEPARATORS) {
        if let Some((key, _)) = option.split_once('=')
            && !KNOWN_OPTION_KEYS
                .iter()
                .any(|known| key.eq_ignore_ascii_case(known))
        {
            debug!("Ignoring login option {key}=, which P2Pool does not read");
        }
    }
}

/// The P2Pool donation a miner chose with `donation=<percent>`, in basis
/// points, or `None` when the options do not choose one and the node's own
/// donation applies.
///
/// The key is agreed with Pickaxe, so one login works at both kinds of pool.
/// The percentage runs from 0 to the donation's own 10% cap in steps of 0.5:
/// `donation=0`, `donation=0.5`, `donation=1` ... `donation=10`, with at most
/// two decimals and no `%` sign. Anything else is an error naming the value,
/// never a silent fallback to the node's donation. The same choice given twice
/// is accepted; two different ones are not.
pub(crate) fn parse_miner_donation(options: Option<&str>) -> Result<Option<u16>, String> {
    let Some(options) = options else {
        return Ok(None);
    };
    let mut chosen = None;
    for option in options.split(OPTION_SEPARATORS) {
        let (key, value) = option.split_once('=').unwrap_or((option, ""));
        if !key.eq_ignore_ascii_case("donation") {
            continue;
        }
        let basis_points = donation_basis_points(value)?;
        match chosen {
            Some(previous) if previous != basis_points => {
                return Err(format!(
                    "donation= is given twice, as {} and {}. Give it once.",
                    donation_percent(previous),
                    donation_percent(basis_points)
                ));
            }
            _ => chosen = Some(basis_points),
        }
    }
    Ok(chosen)
}

/// Basis points of a `donation=` percentage (see [`parse_miner_donation`]).
fn donation_basis_points(value: &str) -> Result<u16, String> {
    let malformed = || {
        format!(
            "donation={value} is not a donation P2Pool reads. Give a percentage from 0 to 10 \
             in steps of 0.5, such as donation=0, donation=0.5 or donation=1, without a % sign."
        )
    };
    let basis_points = percent_basis_points(value).ok_or_else(malformed)?;
    if basis_points > u32::from(MAX_BITCOIN_CASH_CUT) {
        return Err(format!(
            "donation={value} is above the donation's 10% cap. Give a percentage from 0 to 10."
        ));
    }
    if !basis_points.is_multiple_of(u32::from(MINER_DONATION_STEP)) {
        return Err(format!(
            "donation={value} is not one of the steps of 0.5%: give 0, 0.5, 1, 1.5 and so on up to 10."
        ));
    }
    u16::try_from(basis_points).map_err(|_| malformed())
}

/// Basis points of a percentage as a miner writes it: at most six whole
/// digits and at most two decimals, with no sign and no `%`. `None` for
/// anything else.
fn percent_basis_points(value: &str) -> Option<u32> {
    let is_digits = |text: &str, most: usize| {
        !text.is_empty() && text.len() <= most && text.bytes().all(|byte| byte.is_ascii_digit())
    };
    let (whole, fraction) = match value.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (value, None),
    };
    if !is_digits(whole, 6) || fraction.is_some_and(|fraction| !is_digits(fraction, 2)) {
        return None;
    }
    let whole: u32 = whole.parse().ok()?;
    let hundredths: u32 = match fraction {
        None => 0,
        Some(fraction) => {
            let digits: u32 = fraction.parse().ok()?;
            if fraction.len() == 1 {
                digits * 10
            } else {
                digits
            }
        }
    };
    Some(whole * 100 + hundredths)
}

/// The payout level a miner chose: `min-payout=<satoshis>`, the smallest
/// level at or above that amount, or `fee-tolerance=<percent>`, the smallest
/// level whose spend costs at most that share of a payment
/// ([`PayoutLevelChoice`]). `None` when the options choose neither and the
/// node's level applies. `min-payout=` wins when both are given.
///
/// `min-payout=` takes a whole number of satoshis, `fee-tolerance=` a
/// percentage from 0 to 100 with at most two decimals and no `%` sign, as
/// `donation=` does. Anything else is an error naming the value, never a
/// silent fallback to the node's level. The same choice given twice is
/// accepted; two different ones are not.
pub(crate) fn parse_miner_payout_level(
    options: Option<&str>,
) -> Result<Option<PayoutLevelChoice>, String> {
    let Some(options) = options else {
        return Ok(None);
    };
    let mut min_payout = None;
    let mut fee_tolerance = None;
    for option in options.split(OPTION_SEPARATORS) {
        let (key, value) = option.split_once('=').unwrap_or((option, ""));
        if key.eq_ignore_ascii_case("min-payout") {
            let satoshis = min_payout_satoshis(value)?;
            min_payout = once_chosen("min-payout", min_payout, satoshis)?;
        } else if key.eq_ignore_ascii_case("fee-tolerance") {
            let basis_points = fee_tolerance_basis_points(value)?;
            fee_tolerance = once_chosen("fee-tolerance", fee_tolerance, basis_points)?;
        }
    }
    Ok(min_payout
        .map(PayoutLevelChoice::MinPayout)
        .or(fee_tolerance.map(PayoutLevelChoice::FeeTolerance)))
}

/// `chosen` once `value` is given for `key`: the value, unless an earlier
/// one differs.
fn once_chosen<T: PartialEq + std::fmt::Display + Copy>(
    key: &str,
    chosen: Option<T>,
    value: T,
) -> Result<Option<T>, String> {
    match chosen {
        Some(previous) if previous != value => Err(format!(
            "{key}= is given twice, as {previous} and {value}. Give it once."
        )),
        _ => Ok(Some(value)),
    }
}

/// Satoshis of a `min-payout=` value: a whole number, as many digits as a
/// 64-bit amount holds.
fn min_payout_satoshis(value: &str) -> Result<u64, String> {
    let malformed = || {
        format!(
            "min-payout={value} is not a payout P2Pool reads. Give a whole number of satoshis, \
             such as min-payout=10000."
        )
    };
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(malformed());
    }
    value.parse().map_err(|_| malformed())
}

/// Basis points of a `fee-tolerance=` percentage, 0 to 100.
fn fee_tolerance_basis_points(value: &str) -> Result<u16, String> {
    let basis_points = percent_basis_points(value).ok_or_else(|| {
        format!(
            "fee-tolerance={value} is not a fee tolerance P2Pool reads. Give a percentage from \
             0 to 100 with at most two decimals, such as fee-tolerance=1 or fee-tolerance=0.5, \
             without a % sign."
        )
    })?;
    if basis_points > MAX_FEE_TOLERANCE {
        return Err(format!(
            "fee-tolerance={value} is above 100%. Give a percentage from 0 to 100."
        ));
    }
    u16::try_from(basis_points).map_err(|_| format!("fee-tolerance={value} is above 100%."))
}

/// The payout level choice a miner's login options make on this node.
///
/// Only a Bitcoin Cash node in P2Pool mode has payout levels. Any other node
/// refuses `min-payout=` and `fee-tolerance=` rather than mine without the
/// level the miner chose, as it refuses `donation=`.
pub(crate) fn resolve_miner_payout_level(
    options: Option<&str>,
    chain: Chain,
    mode: PoolMode,
) -> Result<Option<PayoutLevelChoice>, String> {
    let choice = parse_miner_payout_level(options)?;
    if choice.is_some() && (chain != Chain::BitcoinCash || mode != PoolMode::P2poolv2) {
        return Err(
            "min-payout= and fee-tolerance= choose a payout level on a Bitcoin Cash P2Pool \
             node. This node does not build a Bitcoin Cash share chain, so it cannot honour \
             them. Remove them to mine here."
                .to_string(),
        );
    }
    Ok(choice)
}

/// Basis points as a miner writes the percentage: 50 is `0.5%`.
pub(crate) fn donation_percent(basis_points: u16) -> String {
    let whole = basis_points / 100;
    match basis_points % 100 {
        0 => format!("{whole}%"),
        hundredths if hundredths.is_multiple_of(10) => format!("{whole}.{}%", hundredths / 10),
        hundredths => format!("{whole}.{hundredths:02}%"),
    }
}

/// The donation a miner's login options choose on this node.
///
/// Only a Bitcoin Cash node in P2Pool mode builds each miner's own jobs and
/// shares with their donation. Any other node refuses a `donation=` option
/// rather than mine with a donation the miner did not choose; without the
/// option every node mines with its own donation.
pub(crate) fn resolve_miner_donation(
    options: Option<&str>,
    chain: Chain,
    mode: PoolMode,
) -> Result<Option<u16>, String> {
    let donation = parse_miner_donation(options)?;
    if donation.is_some() && (chain != Chain::BitcoinCash || mode != PoolMode::P2poolv2) {
        return Err(
            "donation= chooses a P2Pool donation on a Bitcoin Cash P2Pool node. This node \
             does not build a Bitcoin Cash share chain, so it cannot honour it. Remove \
             donation= to mine here."
                .to_string(),
        );
    }
    Ok(donation)
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

#[cfg(test)]
mod donation_tests;

#[cfg(test)]
mod payout_level_tests;
