// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The PPLNS window sized by work, in steady state.
//!
//! On Bitcoin Cash the window walks back from the paying share, skips the
//! 100 immature shares, then takes shares up to and including the first one
//! that brings its work to `K` = 3 Bitcoin Cash blocks of work, at most
//! 120,960 shares (`docs/architecture/payout-distribution.md`, rule 4). A
//! share's work for this is its own, its uncles' and, with weak shares, its
//! weak shares'. A Bitcoin Cash block's work is the network's hashrate times
//! 600 seconds, so in steady state the window holds a fixed number of chain
//! shares and spans a fixed time.

use crate::constants::{
    BCH_BLOCK_INTERVAL_SECONDS, BCH_PPLNS_WINDOW_BLOCKS, MATURITY_SKIP_SHARES, WINDOW_CAP_SECONDS,
};

/// The window's size in steady state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowGeometry {
    /// Chain shares in the window.
    pub shares: f64,
    /// Seconds the window spans.
    pub span_seconds: f64,
    /// Seconds of immature shares the window skips before it starts.
    pub skip_seconds: f64,
    /// Credited work in the window, in hashes.
    pub work: f64,
    /// Whether the share cap, rather than the work, ends the window.
    pub capped: bool,
}

/// Size the window from the chain's credited work rate (hashes a second
/// credited by shares, uncles and weak shares together), its chain share
/// rate and the share period, against Bitcoin Cash's hashrate.
///
/// The cap is 120,960 shares at the 10-second period and 14 days of shares
/// at any other, as section 5 of the design page proposes.
pub fn size_window(
    credited_work_rate: f64,
    chain_share_rate: f64,
    network_hashrate: f64,
    period_seconds: f64,
) -> WindowGeometry {
    let threshold = BCH_PPLNS_WINDOW_BLOCKS * network_hashrate * BCH_BLOCK_INTERVAL_SECONDS;
    let work_per_share = credited_work_rate / chain_share_rate;
    let shares_by_work = threshold / work_per_share;
    let cap_shares = WINDOW_CAP_SECONDS / period_seconds;
    let capped = shares_by_work > cap_shares;
    let shares = shares_by_work.min(cap_shares);
    WindowGeometry {
        shares,
        span_seconds: shares / chain_share_rate,
        skip_seconds: MATURITY_SKIP_SHARES / chain_share_rate,
        work: shares * work_per_share,
        capped,
    }
}

#[cfg(test)]
mod tests;
