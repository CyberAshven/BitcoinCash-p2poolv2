// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Difficulty, compact target and hashrate conversions, and the constants
//! the mining model reads from the node instead of copying them.

use bitcoin::{CompactTarget, Target};
use p2poolv2_lib::config::DEFAULT_MAXIMUM_DIFFICULTY;
use p2poolv2_lib::sim_overrides;
use p2poolv2_lib::stratum::difficulty_adjuster::TARGET_DRR;

/// Hashes a share takes on average per unit of difficulty, 2^32, as
/// `pool_difficulty::anchor_target_for_network_hashrate` counts them.
pub const HASHES_PER_DIFFICULTY: f64 = 4_294_967_296.0;

/// Mantissa of the difficulty-1 target, `0xffff * 2^208` (compact
/// `0x1d00ffff`).
const DIFFICULTY_ONE_MANTISSA: f64 = 65_535.0;

/// Largest compact mantissa: bit 23 is the sign bit.
const COMPACT_MANTISSA_LIMIT: f64 = 8_388_608.0;

/// Smallest normalised compact mantissa (two significant bytes).
const COMPACT_MANTISSA_FLOOR: f64 = 65_536.0;

/// Ideal seconds between shares on the share chain (10 in production).
pub fn ideal_share_interval() -> u32 {
    sim_overrides::ideal_block_time()
}

/// The share chain's ASERT half-life in seconds (600 in production).
pub fn production_half_life() -> u32 {
    sim_overrides::half_life()
}

/// Seconds between one stratum session's submissions once the node's vardiff
/// has settled: `1 / TARGET_DRR`, about 3.33 seconds.
pub fn submission_interval() -> f64 {
    1.0 / TARGET_DRR
}

/// Highest stratum difficulty a session gets when the operator sets no
/// `maximum_difficulty` (2^32).
pub fn maximum_stratum_difficulty() -> f64 {
    DEFAULT_MAXIMUM_DIFFICULTY as f64
}

/// Share difficulty of a compact target, relative to the difficulty-1
/// target.
pub fn difficulty_of(bits: CompactTarget) -> f64 {
    Target::from_compact(bits).difficulty_float()
}

/// Difficulty at which `hashrate` (hashes per second) finds one share per
/// ideal interval: `hashrate * 10 / 2^32` in production.
pub fn steady_difficulty(hashrate: f64) -> f64 {
    hashrate * ideal_share_interval() as f64 / HASHES_PER_DIFFICULTY
}

/// Compact target for a share difficulty, rounded to compact precision the
/// way `Target::to_compact_lossy` rounds: the mantissa is truncated, so the
/// target is at most 2^-15 harder than the exact one.
///
/// `difficulty` must be positive and finite. The window algorithms work in
/// floating-point difficulty; a share header carries this compact form.
pub fn compact_for_difficulty(difficulty: f64) -> CompactTarget {
    let target = DIFFICULTY_ONE_MANTISSA * 2f64.powi(208) / difficulty;
    let mut size = (target.log2() / 8.0).floor() as i32 + 1;
    let mut mantissa = target / 256f64.powi(size - 3);
    // log2 can round across a byte boundary; renormalise in either direction.
    if mantissa < COMPACT_MANTISSA_FLOOR {
        size -= 1;
        mantissa *= 256.0;
    }
    if mantissa >= COMPACT_MANTISSA_LIMIT {
        size += 1;
        mantissa /= 256.0;
    }
    CompactTarget::from_consensus(((size as u32) << 24) | mantissa as u32)
}

/// `bits`, or `ceiling` when `bits` is easier (a larger target).
pub fn no_easier_than(bits: CompactTarget, ceiling: CompactTarget) -> CompactTarget {
    if Target::from_compact(bits) > Target::from_compact(ceiling) {
        ceiling
    } else {
        bits
    }
}

#[cfg(test)]
mod tests;
