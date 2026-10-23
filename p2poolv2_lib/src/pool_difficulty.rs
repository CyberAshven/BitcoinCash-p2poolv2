// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::config::Chain;
#[cfg(test)]
#[mockall_double::double]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
#[cfg(not(test))]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
use crate::shares::share_block::ShareHeader;
use bitcoin::{CompactTarget, Target};
use std::error::Error;
use std::fmt;

// The generated arithmetic trips these lints inside the external macro body,
// where they cannot be fixed. An outer #[allow] on the macro invocation is
// ignored, so the allow has to be an inner attribute on a wrapping module.
mod u512 {
    #![allow(clippy::manual_div_ceil, clippy::assign_op_pattern)]

    uint::construct_uint! {
        /// 512-bit unsigned integer used as an intermediate type for ASERT target
        /// arithmetic. Using 512 bits avoids overflow when multiplying a 256-bit
        /// anchor target by the polynomial factor and applying bit shifts.
        pub struct U512(8);
    }
}
use u512::U512;

use crate::sim_overrides;

/// Maximum (easiest) target as a consensus u32. This is the regtest maximum and
/// serves as the ceiling for the share chain target. Stored as u32 because
/// `CompactTarget::from_consensus` is not a const fn.
const MAX_TARGET_CONSENSUS: u32 = 0x1b4188f5;

/// Fixed-point radix for the ASERT exponent representation (2^16)
const RADIX: i64 = 65536;

/// Name of the Bitcoin Cash ASERT rule in protocol strings: every share
/// after the first is anchored at its parent (see [`PoolDifficulty`]).
pub const BCH_ASERT_ANCHOR_TAG: &str = "asert-parent";

/// Convert a `Target` to a `U512` via its little-endian byte representation.
fn target_to_u512(target: Target) -> U512 {
    let bytes = target.to_le_bytes();
    let mut padded = [0u8; 64];
    padded[..32].copy_from_slice(&bytes);
    U512::from_little_endian(&padded)
}

/// Convert a `U512` value back to a `Target`, assuming it fits in 256 bits.
/// The caller must ensure the value has been clamped to the 256-bit range.
fn u512_to_target(value: U512) -> Target {
    let bytes = value.to_little_endian();
    Target::from_le_bytes(bytes[..32].try_into().expect("slice is 32 bytes"))
}

// ---------------------------------------------------------------------------
// ASERT core algorithm
// ---------------------------------------------------------------------------

/// Easiest target ASERT may return for this anchor.
///
/// The mini Bitcoin Cash chain clamps to its own anchor. Every other anchor,
/// including Bitcoin, keeps [`MAX_TARGET_CONSENSUS`].
fn asert_easy_clamp(anchor_target: CompactTarget) -> u32 {
    if anchor_target.to_consensus() == crate::shares::bch_genesis::MINI_ANCHOR_TARGET {
        crate::shares::bch_genesis::MINI_ANCHOR_TARGET
    } else {
        MAX_TARGET_CONSENSUS
    }
}

/// Calculate the next target using the ASERT (aserti3-2d, as
/// aserti3-10m) algorithm. We use 10m halflife instead of 2 day.
///
/// This is a pure function implementing the integer-only BCH ASERT specification.
/// All arithmetic uses fixed-point integers with no floating point.
///
/// The formula computes:
///   next_target = anchor_target * 2^((time_delta - ideal_block_time * (height_delta + 1)) / halflife)
///
/// using a cubic polynomial approximation for the fractional exponent part.
///
/// Parameters:
///   - `anchor_target`: the compact target of the anchor block
///   - `time_delta`: seconds elapsed from anchor parent time to current block parent time
///   - `height_delta`: block height difference from anchor to current block
///   - `halflife`: ASERT halflife parameter in seconds
///   - `ideal_block_time`: target time between blocks in seconds
///
/// Returns the computed compact target for the next block, never easier than
/// [`asert_easy_clamp`] for this anchor.
pub(crate) fn asert_calculate_target(
    anchor_target: CompactTarget,
    time_delta: i64,
    height_delta: i64,
    halflife: u32,
    ideal_block_time: u32,
) -> CompactTarget {
    asert_target_with_easy_clamp(
        anchor_target,
        time_delta,
        height_delta,
        halflife,
        ideal_block_time,
        CompactTarget::from_consensus(asert_easy_clamp(anchor_target)),
    )
}

/// The ASERT computation of [`asert_calculate_target`] with the easiest
/// allowed target passed in.
///
/// The genesis anchor derives its clamp from the anchor target, which is a
/// chain constant. The Bitcoin Cash parent anchor cannot: its anchor target
/// is the parent's bits, so it passes the chain's ceiling instead.
///
/// Public so the difficulty simulator (`p2poolv2_daa`) can run the share
/// chain's own integer arithmetic with other half-lives. Consensus code goes
/// through [`PoolDifficulty`], which passes the production constants.
pub fn asert_target_with_easy_clamp(
    anchor_target: CompactTarget,
    time_delta: i64,
    height_delta: i64,
    halflife: u32,
    ideal_block_time: u32,
    easy_clamp: CompactTarget,
) -> CompactTarget {
    let max_target = target_to_u512(Target::from_compact(easy_clamp));

    // Step 1: Compute exponent in 16.16 fixed-point format.
    // Use i128 to prevent overflow on the numerator multiplication.
    let ideal_spacing = ideal_block_time as i128;
    let exponent_numerator: i128 =
        (time_delta as i128 - ideal_spacing * (height_delta as i128 + 1)) * RADIX as i128;
    let exponent: i64 = (exponent_numerator / halflife as i128) as i64;

    // Step 2: Decompose into integer shifts and fractional remainder.
    // Rust's >> on i64 is arithmetic (sign-extending), matching the spec requirement.
    let num_shifts: i32 = (exponent >> 16) as i32;
    let remainder: i64 = exponent - (num_shifts as i64) * RADIX;
    // remainder is guaranteed to be in [0, 65535] by the arithmetic right shift property.

    // Step 3: Cubic polynomial approximation of 2^(remainder/65536) in fixed-point.
    // Coefficients from the BCH specification.
    let remainder_wide = remainder as i128;
    let remainder_squared = remainder_wide * remainder_wide;
    let remainder_cubed = remainder_squared * remainder_wide;

    let polynomial: i128 = 195_766_423_245_049_i128 * remainder_wide
        + 971_821_376_i128 * remainder_squared
        + 5_127_i128 * remainder_cubed
        + (1_i128 << 47);

    let factor: u64 = ((polynomial >> 48) + RADIX as i128) as u64;

    // Step 4: Convert anchor target to U512 and multiply by factor.
    // Using U512 avoids overflow even for the maximum 256-bit target.
    let anchor_wide = target_to_u512(Target::from_compact(anchor_target));
    let mut next_target = anchor_wide * U512::from(factor);

    // Step 5: Apply integer shifts.
    // Guard against U512 overflow: left-shifting next_target beyond its
    // available headroom wraps to zero, bypassing the max_target clamp.
    // Compute the safe limit dynamically from the actual bit width so
    // non-max anchors are not clamped unnecessarily.
    if num_shifts >= 0 {
        let available_bits = 512 - next_target.bits();
        if num_shifts as usize > available_bits {
            return easy_clamp;
        }
        next_target <<= num_shifts as usize;
    } else {
        next_target >>= (-num_shifts) as usize;
    }

    // Step 6: Remove fixed-point scaling (divide by 2^16).
    next_target >>= 16;

    // Step 7: Clamp to maximum target.
    if next_target > max_target {
        next_target = max_target;
    }

    // Step 7b: Ensure target is not zero (minimum target of 1).
    if next_target.is_zero() {
        next_target = U512::one();
    }

    // Step 8: Convert back to CompactTarget via Target round-trip.
    let result_target = u512_to_target(next_target);
    result_target.to_compact_lossy()
}

/// The compact target whose pool difficulty is the steady state for a given
/// total network hashrate: `D* = hashrate · IDEAL_BLOCK_TIME / 2^32` is the
/// difficulty at which that hashrate produces one share per target interval.
///
/// Sim only: used to anchor the share chain at ~the difficulty ASERT would
/// settle to, so it starts already regulated instead of climbing from the easy
/// clamp for ~15-20 min (the half-life). Computed by scaling the clamp target
/// by `d_clamp / D*` (target ∝ 1/difficulty), which inverts `difficulty_float`
/// exactly via a known reference target rather than guessing its constant.
/// Returns the clamp unchanged when the hashrate is too low to exceed the floor.
pub fn anchor_target_for_network_hashrate(hashrate_hps: f64) -> CompactTarget {
    let clamp = CompactTarget::from_consensus(MAX_TARGET_CONSENSUS);
    let d_star =
        (hashrate_hps * sim_overrides::ideal_block_time() as f64 / 4_294_967_296.0).max(1.0);
    let d_clamp = Target::from_compact(clamp).difficulty_float();
    if d_star <= d_clamp {
        return clamp;
    }
    let clamp_wide = target_to_u512(Target::from_compact(clamp));
    let scaled =
        clamp_wide * U512::from(d_clamp.round() as u128) / U512::from(d_star.round() as u128);
    u512_to_target(scaled).to_compact_lossy()
}

// ---------------------------------------------------------------------------
// PoolDifficulty -- public API
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum PoolDifficultyError {
    /// Failed to update difficulty due to an internal error
    UpdateError(String),
    /// Anchor block data is not available
    AnchorNotFound(String),
}

impl fmt::Display for PoolDifficultyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PoolDifficultyError::UpdateError(message) => {
                write!(formatter, "Pool difficulty update error: {message}")
            }
            PoolDifficultyError::AnchorNotFound(message) => {
                write!(formatter, "Anchor block not found: {message}")
            }
        }
    }
}

impl Error for PoolDifficultyError {}

/// The parent of the share whose target is being computed or checked.
///
/// Bitcoin reads `height` and `time`. Bitcoin Cash reads `bits`, `time` and
/// `previous_time`. Callers fill every field from the same parent, so either
/// rule sees the share it was given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AsertParent {
    /// Height of the parent share.
    pub height: u32,
    /// Timestamp of the parent share.
    pub time: u32,
    /// Target the parent share was mined at.
    pub bits: CompactTarget,
    /// Timestamp of the parent's own parent. Genesis has no parent and uses
    /// its own time, as the genesis anchor does.
    pub previous_time: u32,
}

impl AsertParent {
    /// Descriptor for `header` at `height`, whose own parent has timestamp
    /// `previous_time`.
    pub fn from_header(header: &ShareHeader, height: u32, previous_time: u32) -> Self {
        Self {
            height,
            time: header.time,
            bits: header.bits,
            previous_time,
        }
    }
}

/// Pool difficulty tracker using the ASERT (aserti3-2d renamed as
/// aserti3-10m) algorithm. Our halflife is 10m instead of 2 days.
///
/// Where the algorithm is anchored depends on the chain:
///
/// - Bitcoin anchors at genesis, as upstream does. The algorithm is absolute
///   (not iterative): the target follows from the anchor point and the
///   parent's height and time alone.
/// - Bitcoin Cash anchors at each share's parent: the same integer formula,
///   with the parent's bits as the anchor target and the parent's own
///   interval as the time delta. Without clamping this gives the genesis
///   rule's targets; at the ceiling it stops counting time the chain spent
///   behind schedule, so a chain started long after its genesis time, or
///   restarted after an outage, is not held at the ceiling while it makes up
///   the missing shares. See `docs/difficulty_adjustment/asert.md`.
#[derive(Clone, Copy)]
pub struct PoolDifficulty {
    /// The anchor block's compact target (difficulty at the anchor point)
    anchor_target: CompactTarget,
    /// The timestamp of the anchor block's parent (or the anchor itself for genesis)
    anchor_parent_time: u32,
    /// The height of the anchor block
    anchor_height: u32,
    /// Chain whose anchor rule applies: the genesis anchor on Bitcoin, the
    /// parent anchor on Bitcoin Cash.
    chain: Chain,
}

impl PoolDifficulty {
    /// Create a new PoolDifficulty anchored at the given block, on Bitcoin's
    /// genesis anchor rule. Use [`Self::for_chain`] to select another chain.
    ///
    /// For a new chain, the anchor is the genesis block. Use the genesis block's
    /// own timestamp as `anchor_parent_time` since genesis has no parent.
    pub fn new(anchor_target: CompactTarget, anchor_parent_time: u32, anchor_height: u32) -> Self {
        Self {
            anchor_target,
            anchor_parent_time,
            anchor_height,
            chain: Chain::Bitcoin,
        }
    }

    /// Build a PoolDifficulty for `chain` anchored at the genesis block from
    /// the chain store.
    ///
    /// Reads the genesis header to obtain the anchor target and timestamp.
    /// The anchor height is always 0 since the anchor is genesis. On Bitcoin
    /// Cash the genesis anchor only sets the first share's target and the
    /// chain's ceiling; every later share is anchored at its parent.
    pub fn build(
        chain_store_handle: &ChainStoreHandle,
        chain: Chain,
    ) -> Result<Self, PoolDifficultyError> {
        let genesis_header = chain_store_handle
            .get_genesis_header()
            .map_err(|error| PoolDifficultyError::AnchorNotFound(error.to_string()))?;
        Ok(Self::new(genesis_header.bits, genesis_header.time, 0).for_chain(chain))
    }

    /// Select the chain whose anchor rule applies.
    pub fn for_chain(mut self, chain: Chain) -> Self {
        self.chain = chain;
        self
    }

    /// Calculate the required target for a block given its parent.
    ///
    /// This is a pure computation from the anchor point: genesis on Bitcoin,
    /// the parent itself on Bitcoin Cash. The ASERT formula internally adds 1
    /// to the height delta, so callers pass the parent, not the block being
    /// mined.
    pub fn calculate_target(&self, parent: AsertParent) -> CompactTarget {
        match self.chain {
            Chain::Bitcoin => self.target_from_genesis(parent),
            Chain::BitcoinCash => self.target_from_parent(parent),
        }
    }

    /// Bitcoin's rule: ASERT anchored at genesis, from the parent's height
    /// and time.
    fn target_from_genesis(&self, parent: AsertParent) -> CompactTarget {
        let time_delta = parent.time as i64 - self.anchor_parent_time as i64;
        let height_delta = parent.height as i64 - self.anchor_height as i64;

        asert_calculate_target(
            self.anchor_target,
            time_delta,
            height_delta,
            sim_overrides::half_life(),
            sim_overrides::ideal_block_time(),
        )
    }

    /// Bitcoin Cash's rule: ASERT anchored at the parent, BCH's own anchor
    /// convention applied one share back. The anchor target is the parent's
    /// bits, the time delta is the parent's interval from its own parent, and
    /// the height delta is zero.
    ///
    /// When the parent is genesis, its interval is zero and its bits are the
    /// genesis anchor target, so the first share's target is the genesis
    /// rule's. Results never exceed the chain's ceiling, which is what lets
    /// the next share start from the ceiling instead of from a schedule the
    /// chain has fallen behind.
    fn target_from_parent(&self, parent: AsertParent) -> CompactTarget {
        let interval = parent.time as i64 - parent.previous_time as i64;

        asert_target_with_easy_clamp(
            parent.bits,
            interval,
            0,
            sim_overrides::half_life(),
            sim_overrides::ideal_block_time(),
            self.target_ceiling(),
        )
    }

    /// Calculate the pool target clamped to stay within valid bounds.
    ///
    /// The returned target is never easier than the chain ceiling. Bitcoin
    /// and the Bitcoin Cash main chain use MAX_POOL_TARGET. The mini chain
    /// uses its own anchor.
    ///
    /// There is no floor at the block's own target: the share target can be
    /// harder than a block, as on chipnet when BCHN offers a
    /// minimum-difficulty block. Such a block is still submitted and pays
    /// the PPLNS window, but it is not a share. A floor would have to take
    /// the block target from the share's own header, whose nBits, time and
    /// height its miner chooses; the ASERT band cannot pin them, because the
    /// header's time stands in for its parent's and the proof-of-work limit
    /// is always allowed on test networks. Anyone could then claim an easy
    /// block target and mine shares at it.
    pub fn calculate_target_clamped(&self, parent: AsertParent) -> CompactTarget {
        let asert_target = self.calculate_target(parent);

        let max_pool_target_compact = self.target_ceiling();
        let max_pool_target = Target::from_compact(max_pool_target_compact);

        let result_target = Target::from_compact(asert_target);
        if result_target > max_pool_target {
            max_pool_target_compact
        } else {
            asert_target
        }
    }

    /// Easiest share target this chain allows: `MAX_POOL_TARGET` on Bitcoin
    /// and the Bitcoin Cash main chain, the mini chain's own ceiling on mini.
    pub fn target_ceiling(&self) -> CompactTarget {
        crate::shares::bch_genesis::target_ceiling(self.anchor_target)
    }

    /// Calculate the target and return it as a consensus u32.
    ///
    /// Convenience wrapper for compatibility with `ChainStoreHandle::get_current_target()`.
    pub fn calculate_target_consensus(&self, parent: AsertParent) -> u32 {
        self.calculate_target(parent).to_consensus()
    }
}

// Mock for PoolDifficulty using mockall.
// Use with #[mockall_double::double] to swap the real type for mock in tests.
#[cfg(test)]
mockall::mock! {
    pub PoolDifficulty {
        pub fn new(anchor_target: CompactTarget, anchor_parent_time: u32, anchor_height: u32) -> Self;
        pub fn build(chain_store_handle: &ChainStoreHandle, chain: Chain) -> Result<Self, PoolDifficultyError>;
        pub fn for_chain(self, chain: Chain) -> Self;
        pub fn calculate_target(&self, parent: AsertParent) -> CompactTarget;
        pub fn calculate_target_clamped(&self, parent: AsertParent) -> CompactTarget;
        pub fn calculate_target_consensus(&self, parent: AsertParent) -> u32;
        pub fn target_ceiling(&self) -> CompactTarget;
    }

    impl Clone for PoolDifficulty {
        fn clone(&self) -> Self;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::BchShareChain;
    use crate::shares::bch_genesis;
    use crate::shares::bch_genesis::MINI_ANCHOR_TARGET;
    use crate::shares::share_block::{MAX_POOL_TARGET, ShareBlock};
    use crate::test_utils::mine_at_constant_hashrate;
    use bitcoin::hashes::Hash;
    use p2poolv2_wallet::bch_share::BchShareNetwork;

    // =======================================================================
    // U512 / Target conversion tests
    // =======================================================================

    #[test]
    fn test_target_u512_roundtrip_regtest_max() {
        let original = Target::from_compact(CompactTarget::from_consensus(0x1b4188f5));
        let wide = target_to_u512(original);
        let recovered = u512_to_target(wide);
        assert_eq!(original.to_le_bytes(), recovered.to_le_bytes());
    }

    #[test]
    fn test_target_u512_roundtrip_mainnet_genesis() {
        let original = Target::from_compact(CompactTarget::from_consensus(0x1d00ffff));
        let wide = target_to_u512(original);
        let recovered = u512_to_target(wide);
        assert_eq!(original.to_le_bytes(), recovered.to_le_bytes());
    }

    #[test]
    fn test_u512_multiply_then_shift_recovers_original() {
        // (anchor * 65536) >> 16 should give back anchor
        let target = Target::from_compact(CompactTarget::from_consensus(0x1b4188f5));
        let anchor = target_to_u512(target);
        let product = anchor * U512::from(65536u64);
        let recovered = product >> 16;
        assert_eq!(anchor, recovered);
    }

    #[test]
    fn test_u512_max_target_multiply_does_not_overflow() {
        // Max target (~2^255) * 131072 (~2^17) = ~2^272, fits in U512
        let target = Target::from_compact(CompactTarget::from_consensus(0x1b4188f5));
        let anchor = target_to_u512(target);
        let product = anchor * U512::from(131072u64);
        // Product should be larger than anchor
        assert!(product > anchor);
        // Product should not be zero (no wrapping)
        assert!(!product.is_zero());
    }

    // =======================================================================
    // ASERT algorithm tests
    // =======================================================================

    #[test]
    fn test_asert_on_schedule_returns_anchor_target() {
        // When blocks are exactly on schedule, the exponent is zero and the
        // target should equal the anchor target.
        // For height_delta=0: on schedule means time_delta = ideal_block_time * (0 + 1) = 10
        let anchor = CompactTarget::from_consensus(0x1b4188f5);
        let result = asert_calculate_target(anchor, 10, 0, 600, 10);
        assert_eq!(result.to_consensus(), anchor.to_consensus());
    }

    #[test]
    fn test_asert_on_schedule_height_1() {
        // height_delta=1, on schedule means time_delta = ideal_block_time * (1 + 1) = 20
        let anchor = CompactTarget::from_consensus(0x1b4188f5);
        let result = asert_calculate_target(anchor, 20, 1, 600, 10);
        assert_eq!(result.to_consensus(), anchor.to_consensus());
    }

    #[test]
    fn test_asert_on_schedule_height_60() {
        // height_delta=60, on schedule means time_delta = 10 * 61 = 610
        let anchor = CompactTarget::from_consensus(0x1b4188f5);
        let result = asert_calculate_target(anchor, 610, 60, 600, 10);
        assert_eq!(result.to_consensus(), anchor.to_consensus());
    }

    #[test]
    fn test_asert_on_schedule_below_max_anchor() {
        // On-schedule with an anchor well below max target
        let anchor = CompactTarget::from_consensus(0x1a4188f5);
        let result = asert_calculate_target(anchor, 10, 0, 600, 10);
        assert_eq!(result.to_consensus(), anchor.to_consensus());
    }

    #[test]
    fn test_asert_fast_blocks_decrease_target() {
        // Blocks arriving faster than expected should lower the target (harder mining)
        let anchor = CompactTarget::from_consensus(0x1b4188f5);
        let on_schedule = asert_calculate_target(anchor, 610, 60, 600, 10);
        let too_fast = asert_calculate_target(anchor, 305, 60, 600, 10);

        let on_schedule_wide = target_to_u512(Target::from_compact(on_schedule));
        let too_fast_wide = target_to_u512(Target::from_compact(too_fast));
        assert!(
            too_fast_wide < on_schedule_wide,
            "faster blocks should produce a lower (harder) target"
        );
    }

    #[test]
    fn test_asert_slow_blocks_increase_target() {
        // Blocks arriving slower than expected should raise the target (easier mining).
        // Use anchor below max target so there is room to increase.
        let anchor = CompactTarget::from_consensus(0x1a4188f5);
        let on_schedule = asert_calculate_target(anchor, 610, 60, 600, 10);
        let too_slow = asert_calculate_target(anchor, 1220, 60, 600, 10);

        let on_schedule_wide = target_to_u512(Target::from_compact(on_schedule));
        let too_slow_wide = target_to_u512(Target::from_compact(too_slow));
        assert!(
            too_slow_wide > on_schedule_wide,
            "slower blocks should produce a higher (easier) target"
        );
    }

    #[test]
    fn test_asert_slow_blocks_at_max_target_clamps() {
        // When anchor is already at max, slow blocks should clamp to max
        let anchor = CompactTarget::from_consensus(0x1b4188f5);
        let too_slow = asert_calculate_target(anchor, 1220, 60, 600, 10);
        let max_target_wide = target_to_u512(Target::from_compact(CompactTarget::from_consensus(
            MAX_TARGET_CONSENSUS,
        )));
        let too_slow_wide = target_to_u512(Target::from_compact(too_slow));
        assert!(
            too_slow_wide <= max_target_wide,
            "result should be clamped to max target"
        );
    }

    #[test]
    fn test_asert_halflife_approximately_doubles_target() {
        // When blocks are behind schedule by exactly one halflife, the target
        // should approximately double.
        //
        // For height_delta=0: exponent = (time_delta - 10) * 65536 / 600
        // We want exponent = 65536 (one full unit), so:
        //   (time_delta - 10) * 65536 / 600 = 65536
        //   time_delta - 10 = 600
        //   time_delta = 610
        //
        // Anchor must be well below max target so doubling does not clamp.
        let anchor = CompactTarget::from_consensus(0x1a4188f5);
        let anchor_wide = target_to_u512(Target::from_compact(anchor));
        let result = asert_calculate_target(anchor, 610, 0, 600, 10);
        let result_wide = target_to_u512(Target::from_compact(result));

        assert!(
            result_wide > anchor_wide,
            "target should increase when behind schedule"
        );

        // Verify it is close to 2x (sanity upper bound: less than 3x)
        let anchor_times_3 = anchor_wide * U512::from(3u64);
        assert!(
            result_wide < anchor_times_3,
            "target should not more than triple after one halflife"
        );
    }

    #[test]
    fn test_asert_halflife_approximately_halves_target() {
        // When blocks are ahead of schedule by one halflife, the target should
        // approximately halve.
        //
        // For height_delta=0: exponent = (time_delta - 10) * 65536 / 600
        // We want exponent = -65536, so:
        //   time_delta - 10 = -600
        //   time_delta = -590
        //
        // Anchor must be below max target so halving is not obscured by clamping.
        let anchor = CompactTarget::from_consensus(0x1a4188f5);
        let anchor_wide = target_to_u512(Target::from_compact(anchor));
        let result = asert_calculate_target(anchor, -590, 0, 600, 10);
        let result_wide = target_to_u512(Target::from_compact(result));

        assert!(
            result_wide < anchor_wide,
            "target should decrease when ahead of schedule"
        );

        // Should not drop below anchor/4 (sanity lower bound)
        let anchor_quarter = anchor_wide >> 2;
        assert!(
            result_wide > anchor_quarter,
            "target should not drop below quarter after one halflife"
        );
    }

    #[test]
    fn test_asert_negative_time_delta() {
        let anchor = CompactTarget::from_consensus(0x1b4188f5);
        let normal = asert_calculate_target(anchor, 10, 0, 600, 10);
        let negative = asert_calculate_target(anchor, -10, 0, 600, 10);

        let normal_wide = target_to_u512(Target::from_compact(normal));
        let negative_wide = target_to_u512(Target::from_compact(negative));
        assert!(
            negative_wide < normal_wide,
            "negative time delta should produce harder target"
        );
    }

    #[test]
    fn test_asert_large_height_delta_does_not_panic() {
        let anchor = CompactTarget::from_consensus(0x1b4188f5);
        let result = asert_calculate_target(anchor, 100_000, 100_000, 600, 10);
        let _ = Target::from_compact(result);
    }

    #[test]
    fn test_asert_extreme_fast_mining_target_not_zero() {
        let anchor = CompactTarget::from_consensus(0x1b4188f5);
        let result = asert_calculate_target(anchor, 0, 100_000, 600, 10);
        let result_wide = target_to_u512(Target::from_compact(result));
        assert!(
            !result_wide.is_zero(),
            "target should never be zero even under extreme conditions"
        );
    }

    #[test]
    fn test_asert_extreme_slow_mining_clamps_to_max() {
        let anchor = CompactTarget::from_consensus(0x1d00ffff);
        let result = asert_calculate_target(anchor, 1_000_000, 0, 600, 10);
        let result_wide = target_to_u512(Target::from_compact(result));
        let max_wide = target_to_u512(Target::from_compact(CompactTarget::from_consensus(
            MAX_TARGET_CONSENSUS,
        )));
        assert!(result_wide <= max_wide, "target should be clamped to max");
    }

    #[test]
    fn test_asert_compact_target_stable_under_roundtrip() {
        let anchor = CompactTarget::from_consensus(0x1b4188f5);
        let result = asert_calculate_target(anchor, 50, 3, 600, 10);
        let result_target = Target::from_compact(result);
        let roundtrip = result_target.to_compact_lossy();
        assert_eq!(result.to_consensus(), roundtrip.to_consensus());
    }

    #[test]
    fn test_asert_symmetry_of_adjustments() {
        // Anchor must be below max target so the upward adjustment is not clamped.
        let anchor = CompactTarget::from_consensus(0x1a4188f5);
        let anchor_wide = target_to_u512(Target::from_compact(anchor));

        // 300 seconds behind schedule at height 0
        let behind = asert_calculate_target(anchor, 310, 0, 600, 10);
        let behind_wide = target_to_u512(Target::from_compact(behind));

        // 300 seconds ahead of schedule at height 0
        let ahead = asert_calculate_target(anchor, -290, 0, 600, 10);
        let ahead_wide = target_to_u512(Target::from_compact(ahead));

        assert!(behind_wide > anchor_wide);
        assert!(ahead_wide < anchor_wide);
    }

    #[test]
    fn test_asert_monotonic_with_time() {
        // For a fixed height_delta, increasing time_delta should monotonically
        // increase the target (easier difficulty).
        // Anchor must be below max target so results are not all clamped equally.
        let anchor = CompactTarget::from_consensus(0x1a4188f5);

        let target_at_100 = asert_calculate_target(anchor, 100, 5, 600, 10);
        let target_at_200 = asert_calculate_target(anchor, 200, 5, 600, 10);
        let target_at_300 = asert_calculate_target(anchor, 300, 5, 600, 10);

        let t100 = target_to_u512(Target::from_compact(target_at_100));
        let t200 = target_to_u512(Target::from_compact(target_at_200));
        let t300 = target_to_u512(Target::from_compact(target_at_300));

        assert!(
            t200 >= t100,
            "target should increase with increasing time delta"
        );
        assert!(
            t300 >= t200,
            "target should increase with increasing time delta"
        );
    }

    #[test]
    fn test_asert_with_high_difficulty_anchor() {
        let anchor = CompactTarget::from_consensus(0x01010000);
        let result = asert_calculate_target(anchor, 10, 0, 600, 10);
        assert_ne!(result.to_consensus(), 0);
    }

    // =======================================================================
    // PoolDifficulty struct tests
    // =======================================================================

    #[test]
    fn test_pool_difficulty_new() {
        let pool_difficulty =
            PoolDifficulty::new(CompactTarget::from_consensus(0x1b4188f5), 1_700_000_000, 0);
        assert_eq!(pool_difficulty.anchor_target.to_consensus(), 0x1b4188f5);
        assert_eq!(pool_difficulty.anchor_parent_time, 1_700_000_000);
        assert_eq!(pool_difficulty.anchor_height, 0);
    }

    #[test]
    fn test_pool_difficulty_on_schedule() {
        let pool_difficulty =
            PoolDifficulty::new(CompactTarget::from_consensus(0x1b4188f5), 1_700_000_000, 0);
        // Parent at height 1, on schedule: 10 * (1+1) = 20 seconds after anchor
        let target = pool_difficulty.calculate_target(AsertParent {
            height: 1,
            time: 1_700_000_020,
            bits: CompactTarget::from_consensus(0x1b4188f5),
            previous_time: 1_700_000_010,
        });
        assert_eq!(target.to_consensus(), 0x1b4188f5);
    }

    #[test]
    fn test_pool_difficulty_consensus_matches_compact() {
        let pool_difficulty =
            PoolDifficulty::new(CompactTarget::from_consensus(0x1b4188f5), 1_700_000_000, 0);
        let parent = AsertParent {
            height: 1,
            time: 1_700_000_015,
            bits: CompactTarget::from_consensus(0x1b4188f5),
            previous_time: 1_700_000_005,
        };
        let compact = pool_difficulty.calculate_target(parent);
        let consensus = pool_difficulty.calculate_target_consensus(parent);
        assert_eq!(consensus, compact.to_consensus());
    }

    #[test]
    fn test_pool_difficulty_on_schedule_at_various_heights() {
        let pool_difficulty =
            PoolDifficulty::new(CompactTarget::from_consensus(0x1b4188f5), 1_700_000_000, 0);

        // On schedule: block_parent_time = anchor_parent_time + ideal_block_time * (parent_height + 1)
        let on_schedule_1 = pool_difficulty.calculate_target(AsertParent {
            height: 1,
            time: 1_700_000_020,
            bits: CompactTarget::from_consensus(0x1b4188f5),
            previous_time: 1_700_000_010,
        });
        let on_schedule_5 = pool_difficulty.calculate_target(AsertParent {
            height: 5,
            time: 1_700_000_060,
            bits: CompactTarget::from_consensus(0x1b4188f5),
            previous_time: 1_700_000_050,
        });
        let on_schedule_100 = pool_difficulty.calculate_target(AsertParent {
            height: 100,
            time: 1_700_001_010,
            bits: CompactTarget::from_consensus(0x1b4188f5),
            previous_time: 1_700_001_000,
        });

        assert_eq!(on_schedule_1.to_consensus(), 0x1b4188f5);
        assert_eq!(on_schedule_5.to_consensus(), 0x1b4188f5);
        assert_eq!(on_schedule_100.to_consensus(), 0x1b4188f5);
    }

    #[test]
    fn test_pool_difficulty_slightly_ahead() {
        let pool_difficulty =
            PoolDifficulty::new(CompactTarget::from_consensus(0x1b4188f5), 1_700_000_000, 0);
        // Parent at height 1, only 10 seconds after anchor (ahead of schedule)
        //
        // On-schedule time is ideal_block_time * (parent_height + 1) =
        //  10 * (1 + 1) = 20 seconds after the anchor.
        let target = pool_difficulty.calculate_target(AsertParent {
            height: 1,
            time: 1_700_000_010,
            bits: CompactTarget::from_consensus(0x1b4188f5),
            previous_time: 1_700_000_000,
        });
        let anchor_wide = target_to_u512(Target::from_compact(CompactTarget::from_consensus(
            0x1b4188f5,
        )));
        let target_wide = target_to_u512(Target::from_compact(target));
        // Slightly ahead -> slightly harder -> lower target
        assert!(target_wide <= anchor_wide);
    }

    // =======================================================================
    // Error type tests
    // =======================================================================

    #[test]
    fn test_asert_large_positive_time_delta_clamps_to_max() {
        // Reproduces a live signet bug: chain started Nov 2023, now March 2026.
        // Only 3033 shares mined in ~2.3 years. Blocks are massively behind
        // schedule, so the target should clamp to max (easiest difficulty).
        //
        // Actual debug output from the node:
        //   anchor_target = CompactTarget(545259519)  = 0x1b4188f5
        //   time_delta    = 72393399
        //   height_delta  = 3033
        //
        // Bug: num_shifts ≈ 120,605. Left-shifting U512 by >= 512 wraps to 0,
        // then the "ensure not zero" clamp sets target to 1 (hardest).
        let anchor = CompactTarget::from_consensus(0x1b4188f5);
        let result = asert_calculate_target(anchor, 72_393_399, 3033, 600, 10);
        assert_eq!(
            result.to_consensus(),
            0x1b4188f5,
            "target should clamp to max when blocks are far behind schedule"
        );
    }

    #[test]
    fn test_asert_matches_fixture_share_headers() {
        let fixture_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../p2poolv2_tests/test_data/share_sync/share_headers.json");
        let json_string =
            std::fs::read_to_string(&fixture_path).expect("Failed to read fixture file");
        let headers: Vec<ShareHeader> =
            serde_json::from_str(&json_string).expect("Failed to parse fixture JSON");

        assert!(
            headers.len() >= 2,
            "Fixture must have at least genesis + one share"
        );

        let genesis = &headers[0];
        let pool_difficulty = PoolDifficulty::new(genesis.bits, genesis.time, 0);

        for index in 1..headers.len() {
            let parent_height = (index - 1) as u32;
            let previous_time = headers[index.saturating_sub(2)].time;
            let parent =
                AsertParent::from_header(&headers[index - 1], parent_height, previous_time);
            let calculated_bits = pool_difficulty.calculate_target_clamped(parent);

            assert_eq!(
                calculated_bits.to_consensus(),
                headers[index].bits.to_consensus(),
                "Share {} (parent_height {}): ASERT calculated 0x{:08X} but fixture has 0x{:08X}",
                index,
                parent_height,
                calculated_bits.to_consensus(),
                headers[index].bits.to_consensus(),
            );
        }
    }

    #[test]
    fn test_calculate_target_clamped_clamps_easy_asert_to_max_pool_target() {
        // Pool target is easier (larger) than bitcoin target -- ASERT would
        // return a target easier than MAX_POOL_TARGET, so it gets clamped
        let anchor = CompactTarget::from_consensus(0x1b4188f5);
        let pool_diff = PoolDifficulty::new(anchor, 1700000000, 0);

        let result = pool_diff.calculate_target_clamped(AsertParent {
            height: 0,
            time: 1_700_000_000,
            bits: CompactTarget::from_consensus(0x1b4188f5),
            previous_time: 1_699_999_990,
        });

        assert_eq!(
            result.to_consensus(),
            MAX_POOL_TARGET,
            "Should clamp to MAX_POOL_TARGET when ASERT target is easier"
        );
    }

    #[test]
    fn test_pool_difficulty_error_display() {
        let update_error = PoolDifficultyError::UpdateError("test error".to_string());
        assert_eq!(
            format!("{update_error}"),
            "Pool difficulty update error: test error"
        );

        let anchor_error = PoolDifficultyError::AnchorNotFound("missing".to_string());
        assert_eq!(format!("{anchor_error}"), "Anchor block not found: missing");
    }

    /// Verify the share_sync fixture forms a valid chain:
    /// - Genesis has all-zeros prev_share_blockhash
    /// - Each block's prev_share_blockhash matches the previous block's hash
    /// - Each non-genesis block's bits match the ASERT-calculated target
    /// - Each non-genesis block's bitcoin header hash meets the share target
    #[test]
    fn test_share_sync_fixture_chain_validity() {
        let fixture_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../p2poolv2_tests/test_data/share_sync/share_blocks.json");
        let json_string =
            std::fs::read_to_string(&fixture_path).expect("Failed to read share_blocks fixture");
        let blocks: Vec<ShareBlock> =
            serde_json::from_str(&json_string).expect("Failed to parse share_blocks fixture");

        assert!(
            blocks.len() >= 2,
            "Fixture must have at least genesis + one share"
        );

        let genesis = &blocks[0];
        assert_eq!(
            genesis.header.prev_share_blockhash,
            bitcoin::BlockHash::all_zeros(),
            "Genesis must have all-zeros prev_share_blockhash"
        );

        let pool_difficulty = PoolDifficulty::new(genesis.header.bits, genesis.header.time, 0);

        let mut prev_hash = genesis.block_hash();
        for (index, block) in blocks.iter().enumerate().skip(1) {
            assert_eq!(
                block.header.prev_share_blockhash, prev_hash,
                "Block {index} prev_share_blockhash does not match parent hash"
            );

            let parent_height = (index - 1) as u32;
            let previous_time = blocks[index.saturating_sub(2)].header.time;
            let parent =
                AsertParent::from_header(&blocks[index - 1].header, parent_height, previous_time);
            let expected_target = pool_difficulty.calculate_target_clamped(parent);
            assert_eq!(
                block.header.bits,
                expected_target,
                "Block {index} bits {:#010x} does not match ASERT target {:#010x}",
                block.header.bits.to_consensus(),
                expected_target.to_consensus()
            );

            let target = Target::from_compact(expected_target);
            let bitcoin_block_hash = block.header.bitcoin_header.block_hash();
            assert!(
                target.is_met_by(bitcoin_block_hash),
                "Block {index} bitcoin hash {bitcoin_block_hash} does not meet share target {target}"
            );

            prev_hash = block.block_hash();
        }
    }

    #[test]
    fn mini_anchor_stays_easier_than_the_main_ceiling() {
        let difficulty = PoolDifficulty::new(
            CompactTarget::from_consensus(MINI_ANCHOR_TARGET),
            1_700_000_000,
            0,
        );
        let next = difficulty.calculate_target_clamped(AsertParent {
            height: 0,
            time: 1_700_000_000,
            bits: CompactTarget::from_consensus(MINI_ANCHOR_TARGET),
            previous_time: 1_700_000_000,
        });
        let main_ceiling = Target::from_compact(CompactTarget::from_consensus(MAX_POOL_TARGET));
        assert!(Target::from_compact(next) > main_ceiling);
    }

    // =======================================================================
    // Bitcoin Cash parent anchor tests
    // =======================================================================

    /// Restart time of the live chipnet mini chain, 2026-10-19 13:24 UTC:
    /// 180 days after the genesis time the Bitcoin Cash genesis carries.
    const CHIPNET_RESTART_TIME: u32 = 1_792_416_240;

    /// Hashrate of the Avalon ASIC on the chipnet pool, about 4 TH/s.
    const ASIC_HASHRATE: f64 = 4.0e12;

    /// Seconds between stratum submissions when each one is a share, as the
    /// chipnet pool logged at the ceiling.
    const SUBMISSION_INTERVAL: u32 = 3;

    const TWO_DAYS: u32 = 2 * 24 * 60 * 60;

    #[test]
    fn parent_anchor_keeps_the_parent_target_when_the_parent_is_on_schedule() {
        let pool_difficulty =
            PoolDifficulty::new(CompactTarget::from_consensus(0x1b4188f5), 1_700_000_000, 0)
                .for_chain(Chain::BitcoinCash);

        let next = pool_difficulty.calculate_target_clamped(AsertParent {
            height: 500,
            time: 1_700_005_010,
            bits: CompactTarget::from_consensus(0x1a4188f5),
            previous_time: 1_700_005_000,
        });

        assert_eq!(next, CompactTarget::from_consensus(0x1a4188f5));
    }

    /// One half-life between the parent and its own parent doubles the
    /// target, as the genesis anchor does for a chain one half-life behind.
    #[test]
    fn parent_anchor_doubles_the_target_after_a_half_life_late_parent() {
        let pool_difficulty =
            PoolDifficulty::new(CompactTarget::from_consensus(0x1b4188f5), 1_700_000_000, 0)
                .for_chain(Chain::BitcoinCash);
        let parent_bits = CompactTarget::from_consensus(0x1a4188f5);

        let next = pool_difficulty.calculate_target_clamped(AsertParent {
            height: 500,
            time: 1_700_005_610,
            bits: parent_bits,
            previous_time: 1_700_005_000,
        });

        assert_eq!(
            next,
            asert_calculate_target(parent_bits, 610, 0, 600, 10),
            "the parent anchor is the genesis formula with the parent as anchor"
        );
        let doubled = target_to_u512(Target::from_compact(parent_bits)) * U512::from(2u64);
        let next_wide = target_to_u512(Target::from_compact(next));
        assert!(next_wide <= doubled && next_wide > doubled - (doubled >> 10));
    }

    /// The first share builds on genesis, which has no interval of its own,
    /// so both anchors give it the same target on the main and mini chains.
    #[test]
    fn parent_anchor_gives_the_first_share_the_genesis_anchor_target() {
        let mini = bch_genesis::build(BchShareNetwork::Chipnet, BchShareChain::Mini)
            .unwrap()
            .header;
        let mini_genesis_anchor = PoolDifficulty::new(mini.bits, mini.time, 0);
        let mini_parent_anchor = mini_genesis_anchor.for_chain(Chain::BitcoinCash);
        let mini_parent = AsertParent::from_header(&mini, 0, mini.time);
        assert_eq!(
            mini_parent_anchor.calculate_target_clamped(mini_parent),
            mini_genesis_anchor.calculate_target_clamped(mini_parent)
        );

        let main = bch_genesis::build(BchShareNetwork::Chipnet, BchShareChain::Main)
            .unwrap()
            .header;
        let main_genesis_anchor = PoolDifficulty::new(main.bits, main.time, 0);
        let main_parent_anchor = main_genesis_anchor.for_chain(Chain::BitcoinCash);
        let main_parent = AsertParent::from_header(&main, 0, main.time);
        assert_eq!(
            main_parent_anchor.calculate_target_clamped(main_parent),
            main_genesis_anchor.calculate_target_clamped(main_parent)
        );
    }

    /// The parent's bits are below the mini ceiling and easier than the main
    /// chain's ASERT clamp, so the ceiling has to come from the chain, not
    /// from the anchor target: a slow parent returns the mini ceiling.
    #[test]
    fn parent_anchor_clamps_a_slow_mini_parent_to_the_mini_ceiling() {
        let pool_difficulty = PoolDifficulty::new(
            CompactTarget::from_consensus(MINI_ANCHOR_TARGET),
            1_700_000_000,
            0,
        )
        .for_chain(Chain::BitcoinCash);

        let next = pool_difficulty.calculate_target_clamped(AsertParent {
            height: 500,
            time: 1_700_009_000,
            bits: CompactTarget::from_consensus(0x1c2a0000),
            previous_time: 1_700_005_000,
        });

        assert_eq!(next, CompactTarget::from_consensus(MINI_ANCHOR_TARGET));
    }

    /// `asert_target_with_easy_clamp` is public for the difficulty simulator.
    /// Given the parent's bits and interval, the production half-life and
    /// interval, and the chain's ceiling, it is the Bitcoin Cash rule.
    #[test]
    fn public_asert_with_production_constants_is_the_parent_anchor_rule() {
        for anchor in [bch_genesis::MAIN_ANCHOR_TARGET, MINI_ANCHOR_TARGET] {
            let pool_difficulty =
                PoolDifficulty::new(CompactTarget::from_consensus(anchor), 1_700_000_000, 0)
                    .for_chain(Chain::BitcoinCash);
            for (bits, interval) in [
                (0x1a4188f5, 10),
                (0x1a4188f5, 0),
                (0x1a4188f5, 3_600),
                (0x1b4188f5, 1),
                (0x1c2a0000, 4_000),
                (0x1c2a0000, -120),
            ] {
                let parent = AsertParent {
                    height: 500,
                    time: (1_700_005_000 + interval) as u32,
                    bits: CompactTarget::from_consensus(bits),
                    previous_time: 1_700_005_000,
                };
                let public = asert_target_with_easy_clamp(
                    parent.bits,
                    interval,
                    0,
                    sim_overrides::half_life(),
                    sim_overrides::ideal_block_time(),
                    pool_difficulty.target_ceiling(),
                );
                assert_eq!(public, pool_difficulty.calculate_target(parent));
            }
        }
    }

    /// The live chipnet case: the mini chain restarted 180 days after its
    /// genesis time. Anchored at each parent, a 4 TH/s ASIC submitting every
    /// 3 seconds at the ceiling (difficulty about 4) reaches its steady
    /// difficulty (about 9,300, eleven doublings up) within eight half-lives.
    #[test]
    fn parent_anchor_brings_a_late_mini_chain_to_steady_difficulty() {
        let genesis = bch_genesis::build(BchShareNetwork::Chipnet, BchShareChain::Mini)
            .unwrap()
            .header;
        assert!(CHIPNET_RESTART_TIME - genesis.time > 179 * 24 * 60 * 60);
        let pool_difficulty =
            PoolDifficulty::new(genesis.bits, genesis.time, 0).for_chain(Chain::BitcoinCash);
        let steady_target = anchor_target_for_network_hashrate(ASIC_HASHRATE);

        let tip = mine_at_constant_hashrate(
            &pool_difficulty,
            AsertParent::from_header(&genesis, 0, genesis.time),
            steady_target,
            CHIPNET_RESTART_TIME,
            SUBMISSION_INTERVAL,
            CHIPNET_RESTART_TIME + 8 * sim_overrides::half_life(),
        );

        let next = pool_difficulty.calculate_target_clamped(tip);
        let ratio = Target::from_compact(next).difficulty_float()
            / Target::from_compact(steady_target).difficulty_float();
        assert!(
            (0.85..=1.15).contains(&ratio),
            "difficulty is {ratio} of steady after eight half-lives"
        );
    }

    /// The same chain on the genesis anchor is about 1.5 million shares
    /// behind schedule and stays at the ceiling, where every submission is a
    /// share at the floor difficulty.
    #[test]
    fn genesis_anchor_holds_a_late_mini_chain_at_its_ceiling() {
        let genesis = bch_genesis::build(BchShareNetwork::Chipnet, BchShareChain::Mini)
            .unwrap()
            .header;
        let pool_difficulty = PoolDifficulty::new(genesis.bits, genesis.time, 0);
        let steady_target = anchor_target_for_network_hashrate(ASIC_HASHRATE);

        let tip = mine_at_constant_hashrate(
            &pool_difficulty,
            AsertParent::from_header(&genesis, 0, genesis.time),
            steady_target,
            CHIPNET_RESTART_TIME,
            SUBMISSION_INTERVAL,
            CHIPNET_RESTART_TIME + 8 * sim_overrides::half_life(),
        );

        assert_eq!(
            pool_difficulty.calculate_target_clamped(tip),
            CompactTarget::from_consensus(MINI_ANCHOR_TARGET)
        );
    }

    /// The main chain's ceiling (difficulty about 1,160) is three doublings
    /// below the ASIC's steady difficulty, so a late start reaches it within
    /// four half-lives.
    #[test]
    fn parent_anchor_brings_a_late_main_chain_to_steady_difficulty_in_four_half_lives() {
        let genesis = bch_genesis::build(BchShareNetwork::Chipnet, BchShareChain::Main)
            .unwrap()
            .header;
        let pool_difficulty =
            PoolDifficulty::new(genesis.bits, genesis.time, 0).for_chain(Chain::BitcoinCash);
        let steady_target = anchor_target_for_network_hashrate(ASIC_HASHRATE);

        let tip = mine_at_constant_hashrate(
            &pool_difficulty,
            AsertParent::from_header(&genesis, 0, genesis.time),
            steady_target,
            CHIPNET_RESTART_TIME,
            SUBMISSION_INTERVAL,
            CHIPNET_RESTART_TIME + 4 * sim_overrides::half_life(),
        );

        let next = pool_difficulty.calculate_target_clamped(tip);
        let ratio = Target::from_compact(next).difficulty_float()
            / Target::from_compact(steady_target).difficulty_float();
        assert!(
            (0.85..=1.15).contains(&ratio),
            "difficulty is {ratio} of steady after four half-lives"
        );
    }

    /// Two days without shares drop the next target to the ceiling once.
    /// Anchored at each parent, the chain then climbs back as from a fresh
    /// start instead of first making up two days of missing shares.
    #[test]
    fn parent_anchor_recovers_from_a_two_day_outage() {
        let genesis = bch_genesis::build(BchShareNetwork::Chipnet, BchShareChain::Mini)
            .unwrap()
            .header;
        let pool_difficulty =
            PoolDifficulty::new(genesis.bits, genesis.time, 0).for_chain(Chain::BitcoinCash);
        let steady_target = anchor_target_for_network_hashrate(ASIC_HASHRATE);
        let before_outage = mine_at_constant_hashrate(
            &pool_difficulty,
            AsertParent::from_header(&genesis, 0, genesis.time),
            steady_target,
            genesis.time + 10,
            SUBMISSION_INTERVAL,
            genesis.time + 16 * sim_overrides::half_life(),
        );
        let resume_time = before_outage.time + TWO_DAYS;

        let tip = mine_at_constant_hashrate(
            &pool_difficulty,
            before_outage,
            steady_target,
            resume_time,
            SUBMISSION_INTERVAL,
            resume_time + 8 * sim_overrides::half_life(),
        );

        let next = pool_difficulty.calculate_target_clamped(tip);
        let ratio = Target::from_compact(next).difficulty_float()
            / Target::from_compact(steady_target).difficulty_float();
        assert!(
            (0.85..=1.15).contains(&ratio),
            "difficulty is {ratio} of steady eight half-lives after the outage"
        );
    }

    /// On the genesis anchor the same outage leaves the chain two days
    /// behind schedule: still at the ceiling eight half-lives later.
    #[test]
    fn genesis_anchor_holds_the_ceiling_after_a_two_day_outage() {
        let genesis = bch_genesis::build(BchShareNetwork::Chipnet, BchShareChain::Mini)
            .unwrap()
            .header;
        let pool_difficulty = PoolDifficulty::new(genesis.bits, genesis.time, 0);
        let steady_target = anchor_target_for_network_hashrate(ASIC_HASHRATE);
        let before_outage = mine_at_constant_hashrate(
            &pool_difficulty,
            AsertParent::from_header(&genesis, 0, genesis.time),
            steady_target,
            genesis.time + 10,
            SUBMISSION_INTERVAL,
            genesis.time + 16 * sim_overrides::half_life(),
        );
        let resume_time = before_outage.time + TWO_DAYS;

        let tip = mine_at_constant_hashrate(
            &pool_difficulty,
            before_outage,
            steady_target,
            resume_time,
            SUBMISSION_INTERVAL,
            resume_time + 8 * sim_overrides::half_life(),
        );

        assert_eq!(
            pool_difficulty.calculate_target_clamped(tip),
            CompactTarget::from_consensus(MINI_ANCHOR_TARGET)
        );
    }

    /// The sim anchors genesis at its launch time and at the steady
    /// difficulty of its network hashrate (`sim_overrides::genesis_timestamp`
    /// and `anchor_target`). The parent anchor keeps that regulated start:
    /// the chain stays at its steady difficulty and agrees with the genesis
    /// anchor.
    #[test]
    fn parent_anchor_keeps_a_regulated_start_anchored_at_launch() {
        let steady_target = anchor_target_for_network_hashrate(ASIC_HASHRATE);
        let launch_time = CHIPNET_RESTART_TIME;
        let genesis_anchor = PoolDifficulty::new(steady_target, launch_time, 0);
        let parent_anchor = genesis_anchor.for_chain(Chain::BitcoinCash);
        let genesis = AsertParent {
            height: 0,
            time: launch_time,
            bits: steady_target,
            previous_time: launch_time,
        };

        let parent_anchor_tip = mine_at_constant_hashrate(
            &parent_anchor,
            genesis,
            steady_target,
            launch_time + 10,
            1,
            launch_time + 36 * sim_overrides::half_life(),
        );
        let genesis_anchor_tip = mine_at_constant_hashrate(
            &genesis_anchor,
            genesis,
            steady_target,
            launch_time + 10,
            1,
            launch_time + 36 * sim_overrides::half_life(),
        );

        let steady_difficulty = Target::from_compact(steady_target).difficulty_float();
        let parent_anchor_ratio =
            Target::from_compact(parent_anchor.calculate_target_clamped(parent_anchor_tip))
                .difficulty_float()
                / steady_difficulty;
        let genesis_anchor_ratio =
            Target::from_compact(genesis_anchor.calculate_target_clamped(genesis_anchor_tip))
                .difficulty_float()
                / steady_difficulty;
        assert!(
            (0.95..=1.05).contains(&parent_anchor_ratio),
            "parent anchor difficulty is {parent_anchor_ratio} of steady"
        );
        assert!(
            (0.95..=1.05).contains(&genesis_anchor_ratio),
            "genesis anchor difficulty is {genesis_anchor_ratio} of steady"
        );
    }
}
