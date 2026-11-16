// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What a node stores for the share chain: arithmetic, no simulation.
//!
//! A node keeps every share header for ever, about 1 KB with its template
//! merkle branch (`docs/architecture/pruning.md`, "Header Sync"). It keeps
//! a share's body, the Bitcoin Cash coinbase and the weak shares it carries
//! among them, for two payout depths and `WEAK_DEPTH` and then prunes it
//! (`ChainDepths::prune_depth`): 242,534 shares with the pruner's lag, 28
//! days, with today's 14-day window cap. A found block that pays some
//! payees through a payout tree keeps that tree for ever, about 45 bytes a
//! tree payee (`store/payout_tree.rs`).
//!
//! The coinbase is the large part. With `n` equal payees and a level of `L`
//! satoshis, a payee whose part of the block is at least `L` is paid
//! directly; below `L` the parts are pooled and drawn in whole levels
//! (`coinbase_budget.rs`, payout levels), so a block pays at most the reward
//! over `L` outputs however many payees there are. Outputs past the 1,000,000
//! byte transaction limit go to the payout tree (the straddle rule). Fees are
//! left out: they add outputs at the same rate as the reward.

use crate::constants::{BLOCK_REWARD_SATOSHIS, DEFAULT_LEVEL, MIN_PAYOUT_LEVEL};
use crate::levels::level_amount;
use crate::report::{group_thousands, rounded};
use crate::tables::table_from;
use p2poolv2_netsim::constants::{
    BCH_MAX_TRANSACTION_BYTES, HYBRID_COINBASE_BYTES_BESIDE_PAYOUTS, MAX_UNCLES_DEPTH,
    P2PKH_OUTPUT_BYTES, SHARE_INTERVAL_SECONDS,
};

/// Bytes a node keeps for ever for each share: its header with the template
/// merkle branch, about 1.0 KB (`docs/architecture/pruning.md`).
pub const HEADER_BYTES_KEPT: f64 = 1_000.0;

/// Bytes of a payout tree kept for each tree payee, about 45
/// (`p2poolv2_lib/src/store/payout_tree.rs`).
pub const TREE_PAYEE_BYTES: f64 = 45.0;

/// Bytes of one carried weak share as stored, its message size, 0.8 KB
/// (`WEAK_SHARE_MESSAGE_BYTES`).
pub const WEAK_SHARE_BYTES: f64 = 800.0;

/// Shares a coinbase skips before its window: Bitcoin Cash's 100-share
/// maturity (`BCH_COINBASE_MATURITY`).
const MATURITY_SHARES: f64 = 100.0;

/// Shares the pruner may lag behind its target (`PRUNE_INTERVAL`).
const PRUNE_INTERVAL_SHARES: f64 = 360.0;

/// Shares below its carrier a carried weak share's parent may be
/// (`WEAK_DEPTH`), which the body prune depth adds to two payout depths.
const WEAK_DEPTH_SHARES: f64 = 48.0;

/// Seconds in a day and in a year.
const DAY_SECONDS: f64 = 86_400.0;
const YEAR_SECONDS: f64 = 365.0 * DAY_SECONDS;

/// Bytes in a gigabyte and a megabyte (decimal, as `pruning.md`).
const GB: f64 = 1.0e9;
const MB: f64 = 1.0e6;

/// Payees in the window, the rows of the storage table.
const PAYEES: [u64; 6] = [10, 100, 1_000, 10_000, 100_000, 1_000_000];

/// Window caps the table compares, in days: today's 14 and 30.
const CAP_DAYS: [f64; 2] = [14.0, 30.0];

/// Weak shares a share carries: today's fill and part 6's ceiling.
const WEAK_COUNTS: [f64; 2] = [32.0, 320.0];

/// Shares of one payout depth with the window capped at `cap_days`: the
/// maturity skip, the window and an uncle of its deepest share
/// (`BCH_PAYOUT_DEPTH`, 121,063 shares at 14 days).
pub fn payout_depth_shares(cap_days: f64) -> f64 {
    MATURITY_SHARES + cap_days * DAY_SECONDS / SHARE_INTERVAL_SECONDS + MAX_UNCLES_DEPTH as f64
}

/// Shares whose bodies a node keeps: two payout depths, `WEAK_DEPTH` and the
/// pruner's lag (242,534 at 14 days).
pub fn kept_body_shares(cap_days: f64) -> f64 {
    2.0 * payout_depth_shares(cap_days) + WEAK_DEPTH_SHARES + PRUNE_INTERVAL_SHARES
}

/// One coinbase of a block shared by `payees` equal payees at payout level
/// `level`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coinbase {
    /// Outputs paid, directly or through the tree.
    pub outputs: f64,
    /// Outputs paid directly in the coinbase.
    pub direct: f64,
    /// Payees paid through the payout tree.
    pub tree: f64,
    /// The coinbase's bytes.
    pub bytes: f64,
}

/// The coinbase of a block shared by `payees` equal payees at `level`.
pub fn coinbase(payees: u64, level: u32) -> Coinbase {
    let level_satoshis = level_amount(level) as f64;
    let share = BLOCK_REWARD_SATOSHIS / payees as f64;
    let outputs = if share >= level_satoshis {
        payees as f64
    } else {
        (BLOCK_REWARD_SATOSHIS / level_satoshis).floor()
    };
    let fit = ((BCH_MAX_TRANSACTION_BYTES - HYBRID_COINBASE_BYTES_BESIDE_PAYOUTS)
        / P2PKH_OUTPUT_BYTES) as f64;
    let direct = outputs.min(fit);
    Coinbase {
        outputs,
        direct,
        tree: outputs - direct,
        bytes: HYBRID_COINBASE_BYTES_BESIDE_PAYOUTS as f64 + direct * P2PKH_OUTPUT_BYTES as f64,
    }
}

/// The storage tables: coinbases by payees and level, then weak shares and
/// headers.
pub fn storage() -> String {
    let shares_per_year = YEAR_SECONDS / SHARE_INTERVAL_SECONDS;
    let rows: Vec<Vec<String>> = PAYEES
        .into_iter()
        .flat_map(|payees| {
            [DEFAULT_LEVEL, MIN_PAYOUT_LEVEL]
                .into_iter()
                .map(move |level| (payees, level))
        })
        .map(|(payees, level)| {
            let block = coinbase(payees, level);
            let mut row = vec![
                group_thousands(payees),
                format!("{level} ({} sat)", group_thousands(level_amount(level))),
                group_thousands(block.outputs as u64),
                group_thousands(block.tree as u64),
                rounded(block.bytes / 1_000.0),
            ];
            row.extend(
                CAP_DAYS
                    .iter()
                    .map(|cap_days| rounded(kept_body_shares(*cap_days) * block.bytes / GB)),
            );
            row.push(rounded(shares_per_year * block.bytes / GB));
            row.push(rounded(block.tree * TREE_PAYEE_BYTES / MB));
            row
        })
        .collect();
    let mut headers: Vec<String> = [
        "Payees in the window",
        "Payout level",
        "Coinbase outputs",
        "In the payout tree",
        "Coinbase KB",
    ]
    .iter()
    .map(|cell| cell.to_string())
    .collect();
    headers.extend(CAP_DAYS.iter().map(|cap_days| {
        format!(
            "GB kept, {cap_days}-day cap ({:.0} days)",
            kept_body_shares(*cap_days) * SHARE_INTERVAL_SECONDS / DAY_SECONDS
        )
    }));
    headers.push("GB a year if never pruned".to_string());
    headers.push("Payout tree MB a found block, kept for ever".to_string());
    let coinbases = table_from(&headers, &rows);

    let mut other_rows: Vec<Vec<String>> = WEAK_COUNTS
        .iter()
        .map(|count| {
            let per_share = count * WEAK_SHARE_BYTES;
            let mut row = vec![format!("{count} weak shares a share")];
            row.extend(
                CAP_DAYS
                    .iter()
                    .map(|cap_days| rounded(kept_body_shares(*cap_days) * per_share / GB)),
            );
            row.extend(
                CAP_DAYS
                    .iter()
                    .map(|cap_days| rounded(payout_depth_shares(*cap_days) * per_share / GB)),
            );
            row.push(rounded(shares_per_year * per_share / GB));
            row
        })
        .collect();
    let bare = HYBRID_COINBASE_BYTES_BESIDE_PAYOUTS as f64;
    let mut bare_row = vec!["A coinbase stored without its payouts".to_string()];
    bare_row.extend(
        CAP_DAYS
            .iter()
            .map(|cap_days| rounded(kept_body_shares(*cap_days) * bare / GB)),
    );
    bare_row.extend(
        CAP_DAYS
            .iter()
            .map(|cap_days| rounded(payout_depth_shares(*cap_days) * bare / GB)),
    );
    bare_row.push(rounded(shares_per_year * bare / GB));
    other_rows.push(bare_row);
    let mut header_row = vec!["Share headers, kept for ever".to_string()];
    header_row.extend(["-", "-", "-", "-"].map(str::to_string));
    header_row.push(rounded(shares_per_year * HEADER_BYTES_KEPT / GB));
    other_rows.push(header_row);
    let mut other_headers = vec!["Stored".to_string()];
    other_headers.extend(
        CAP_DAYS
            .iter()
            .map(|cap_days| format!("GB for two payout depths, {cap_days}-day cap")),
    );
    other_headers.extend(
        CAP_DAYS
            .iter()
            .map(|cap_days| format!("GB for one payout depth, {cap_days}-day cap")),
    );
    other_headers.push("GB a year".to_string());
    let others = table_from(&other_headers, &other_rows);

    format!("{coinbases}\n{others}")
}

#[cfg(test)]
mod tests;
