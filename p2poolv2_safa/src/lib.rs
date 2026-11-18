// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! SAFA AuxPoW deployment, proof, and transaction tools for Bitcoin Cash.
//!
//! The covenant is compiled directly from `safa.cash` by libauth-rs and
//! verified by its BCH 2026 virtual machine. No keys are read or generated.
#![forbid(unsafe_code)]

mod codec;
mod covenant;
mod targets;
mod transaction;

pub mod claimer;
pub mod cli;

pub use codec::*;
pub use covenant::*;
pub use libauth::runtime::bigint::BigInt;
pub use libauth::{Input, Nft, NonFungibleTokenCapability, Output, Token, TransactionCommon};
pub use targets::*;
pub use transaction::*;

/// A claim pays this fraction of the vault's tokens, rounded down.
pub const REWARD_DIVISOR: u64 = 420_000;
/// Satoshis on each token payout and the minimum surviving vault.
pub const OUTPUT_SATS: u64 = 800;
/// Split payouts below this AMM value remain with the miner.
pub const MIN_SHARE_SATS: u64 = 1_000;
/// A claim's target eases by at most this many ticks since the thread's
/// state: 288 ten-minute blocks, two days (CHIP-2025-03 counts block time in
/// ticks, 600 a ten-minute block and 60 a one-minute one).
pub const EASE_CAP_TICKS: u64 = 288 * 600;
/// The fewest threads a deployment may have. A thread takes at most one
/// claim per block, so on ten-minute blocks the ticks since its last claim
/// are at least 600, and with one thread the easing factor
/// `min(dt, 172800) * 5000 / (threads * 600) + 5000` is then at least 10000:
/// the target could ease but never harden. (Claims a one-minute block apart
/// harden it even with one thread, ×0.55; the minimum stays 2.)
pub const MIN_THREADS: u32 = 2;
pub const MAX_THREADS: u32 = 63;
/// The fallback band applies from this many ticks after the state's block:
/// 4,032 ten-minute blocks, four weeks.
pub const FALLBACK_TICKS: u64 = 4_032 * 600;
pub const MAX_PAYOUT_SCRIPT_BYTES: usize = 35;
pub const MAX_TREE_HEIGHT: u8 = 16;
pub const MAX_COINBASE_BRANCH: usize = 25;
pub const MAX_SPLIT_BPS: u16 = 10_000;
pub const MAX_UNLOCKING_BYTES: usize = 10_000;
pub const MIN_FEE_RATE: u32 = 1;
pub const MAX_FEE_RATE: u32 = 3;
pub const LOCKTIME_THRESHOLD: u32 = 500_000_000;
/// Maximum transaction bytes besides the proof, redeem script, and its push.
pub const CLAIM_FIXED_BYTES: usize = 577;
