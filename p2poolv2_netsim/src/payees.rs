// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! How many miners a coinbase pays.
//!
//! A coinbase pays the miners with a share in the PPLNS window. On Bitcoin
//! Cash the window holds three BCH blocks' worth of the pool's work, at most
//! `PPLNS_WINDOW_SHARES` shares (`docs/architecture/payout-distribution.md`,
//! "Bitcoin Cash window"), so it is short for a large pool and long for a
//! small one. However many miners there are, a window can never have more
//! payees than shares, and a miner whose hashrate is a small part of the
//! pool's often has no share in it at all. This module turns a miner count
//! into the expected payee count.

use crate::constants::{BCH_BLOCK_INTERVAL_SECONDS, BCH_PPLNS_WINDOW_BLOCKS, PPLNS_WINDOW_SHARES};
use serde::Serialize;

/// How hashrate is spread over miners or nodes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub enum HashrateSpread {
    /// Every member has the same hashrate.
    Equal,
    /// Hashrate follows a Pareto distribution with this shape: a few large
    /// members and a long tail of small ones. A shape of 1.16 puts 80% of
    /// the hashrate on 20% of the members.
    Pareto {
        /// Pareto shape parameter, above 1.
        shape: f64,
    },
}

impl HashrateSpread {
    /// Parse `equal` or `pareto:<shape>`.
    pub fn parse(text: &str) -> Result<Self, String> {
        let trimmed = text.trim();
        if trimmed.eq_ignore_ascii_case("equal") {
            return Ok(HashrateSpread::Equal);
        }
        let shape_text = trimmed
            .strip_prefix("pareto:")
            .ok_or_else(|| format!("expected `equal` or `pareto:<shape>`, got `{text}`"))?;
        let shape: f64 = shape_text
            .parse()
            .map_err(|_| format!("bad Pareto shape `{shape_text}`"))?;
        if shape > 1.0 && shape.is_finite() {
            Ok(HashrateSpread::Pareto { shape })
        } else {
            Err(format!("a Pareto shape must be above 1, got {shape}"))
        }
    }

    /// Short description for tables.
    pub fn describe(&self) -> String {
        match self {
            HashrateSpread::Equal => "equal".to_string(),
            HashrateSpread::Pareto { shape } => format!("pareto:{shape}"),
        }
    }
}

/// Hashrate fractions of `count` members, summing to one.
///
/// The Pareto case takes each member's hashrate at the midpoint of its
/// quantile, so the result is deterministic and needs no random numbers.
pub fn hashrate_fractions(count: u64, spread: HashrateSpread) -> Vec<f64> {
    let members = count as usize;
    let weights: Vec<f64> = match spread {
        HashrateSpread::Equal => vec![1.0; members],
        HashrateSpread::Pareto { shape } => (0..members)
            .map(|index| {
                let quantile = (index as f64 + 0.5) / members as f64;
                (1.0 - quantile).powf(-1.0 / shape)
            })
            .collect(),
    };
    let total: f64 = weights.iter().sum();
    weights.iter().map(|weight| weight / total).collect()
}

/// Shares in the Bitcoin Cash PPLNS window for a pool holding
/// `pool_fraction_of_bch` of BCH's hashrate: `K` BCH blocks of work at one
/// share every `share_interval_seconds`, about `K * 60 / f` shares at
/// 10-second shares, never more than `PPLNS_WINDOW_SHARES`.
///
/// It leaves out the uncles' work the real window also counts, so it is
/// slightly long when shares go stale. A fraction of zero or less is a pool
/// too small to measure, which gets the full window.
pub fn pplns_window_shares(pool_fraction_of_bch: f64, share_interval_seconds: f64) -> u64 {
    if pool_fraction_of_bch <= 0.0 || share_interval_seconds <= 0.0 {
        return PPLNS_WINDOW_SHARES;
    }
    let shares_per_block = BCH_BLOCK_INTERVAL_SECONDS / share_interval_seconds;
    // The small offset keeps float noise such as 1800.0000000000002 from
    // rounding a whole number of shares up by one.
    let window = (BCH_PPLNS_WINDOW_BLOCKS * shares_per_block / pool_fraction_of_bch - 1e-9).ceil();
    if window >= PPLNS_WINDOW_SHARES as f64 {
        PPLNS_WINDOW_SHARES
    } else {
        (window as u64).max(1)
    }
}

/// Expected number of distinct miners with at least one share among
/// `window_shares` shares, each share going to a miner in proportion to
/// its hashrate.
pub fn expected_payees(miner_count: u64, spread: HashrateSpread, window_shares: u64) -> f64 {
    if miner_count == 0 {
        return 0.0;
    }
    let shares = window_shares as f64;
    match spread {
        HashrateSpread::Equal => {
            let miners = miner_count as f64;
            miners * (1.0 - (shares * (-1.0 / miners).ln_1p()).exp())
        }
        HashrateSpread::Pareto { .. } => hashrate_fractions(miner_count, spread)
            .iter()
            .map(|fraction| 1.0 - (shares * (-fraction).ln_1p()).exp())
            .sum(),
    }
}

/// Expected seconds between shares for a miner holding `hashrate_fraction`
/// of the pool, at one share every `share_interval_seconds` pool-wide.
pub fn seconds_between_shares(hashrate_fraction: f64, share_interval_seconds: f64) -> f64 {
    share_interval_seconds / hashrate_fraction
}

#[cfg(test)]
mod tests;
