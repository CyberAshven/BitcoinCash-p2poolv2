// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Bitcoin Cash's own block difficulty, aserti3-2d, and the header check
//! P2Pool runs with it on merge-mining proofs and token shares.
//!
//! The arithmetic follows `CalculateASERT` and `GetNextASERTWorkRequired` in
//! Bitcoin Cash Node (`src/pow.cpp`) integer for integer: the exponent is a
//! truncating 64-bit division, its integer part an arithmetic (flooring)
//! shift, the fraction goes through BCHN's cubic, and the result is clamped
//! to the proof-of-work limit and to at least 1. The per-network anchors and
//! half-lives are BCHN's chain parameters (`src/chainparams.cpp`,
//! `asertAnchorParams` and `nASERTHalfLife`). The tests run the aserti3-2d
//! specification's twelve vector runs and real mainnet and chipnet headers.
//!
//! The header check is a format check, not block validation: P2Pool has
//! only the header and its coinbase height, not the parent block, so the
//! header's own time stands in for its parent's and the computed target gets
//! a band (see [`BchAsert::check_header_bits`]). BCHN stays the authority on
//! whether a block is valid.

use crate::config::BitcoinCashNetwork;
use bitcoin::CompactTarget;

// The generated arithmetic trips these lints inside the external macro body,
// where they cannot be fixed, so the allow wraps the invocation in a module.
mod u256 {
    #![allow(clippy::manual_div_ceil, clippy::assign_op_pattern)]

    uint::construct_uint! {
        /// BCHN's `arith_uint256`: the unsigned 256-bit integer its
        /// difficulty arithmetic works in.
        pub struct U256(4);
    }
}
pub use u256::U256;

/// The interval of every Bitcoin Cash block up to and including a network's
/// one-minute activation block, in ticks (seconds of target time), and the
/// interval of the ASERT anchor block on every network: 600.
pub const BCH_BLOCK_SPACING: i64 = 600;

/// The interval of every block after a network's activation block under
/// CHIP-2025-03 "Faster Blocks for Bitcoin Cash": 60 ticks.
pub const BCH_FAST_BLOCK_SPACING: i64 = 60;

/// When a network moves to one-minute blocks (CHIP-2025-03).
///
/// A block's ticks are the sum of the target intervals of the blocks up to
/// it: 600 for every block up to and including the activation block, 60 for
/// every block after it. The activation block is the first block whose
/// median time past is at or after `activation_time`; its child is the first
/// one-minute block. Before activation a block's ticks are exactly `600 *
/// height`, so every rule counted in ticks is the old rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FastBlocks {
    /// Median time past at which the network activates one-minute blocks.
    pub activation_time: i64,
    /// Height of the activation block, the last 600-tick block, once the
    /// network has activated and that block can no longer be reorganized.
    /// The CHIP fixes it in node code the same way. `None` before that:
    /// every block is then a 600-tick block, which holds only while the
    /// network has not activated.
    pub activation_height: Option<u32>,
}

impl FastBlocks {
    /// Mainnet's activation, 2027-05-15 12:00 UTC (also testnet3,
    /// testnet4, scalenet and regtest), not yet reached.
    pub const MAINNET: Self = Self {
        activation_time: 1_810_382_400,
        activation_height: None,
    };

    /// Chipnet's activation, 2026-11-15 12:00 UTC. Block 332,450 is the
    /// last 600-tick block and 332,451 the first one-minute block.
    pub const CHIPNET: Self = Self {
        activation_time: 1_794_744_000,
        activation_height: Some(332_450),
    };

    /// The ticks of the block at `height`: `600 * height` up to the
    /// activation block, then 60 more per block.
    pub fn ticks(&self, height: u32) -> i64 {
        let height = i64::from(height);
        match self.activation_height.map(i64::from) {
            Some(activation) if height > activation => {
                activation * BCH_BLOCK_SPACING + (height - activation) * BCH_FAST_BLOCK_SPACING
            }
            _ => height * BCH_BLOCK_SPACING,
        }
    }

    /// The target interval, in ticks, of the block after the one at
    /// `height`: 60 from the activation block on, 600 before it.
    pub fn interval_after(&self, height: u32) -> i64 {
        match self.activation_height {
            Some(activation) if height >= activation => BCH_FAST_BLOCK_SPACING,
            _ => BCH_BLOCK_SPACING,
        }
    }

    /// The activation of `network`. Regtest and scalenet use mainnet's
    /// time, as in BCHN.
    pub fn for_network(network: BitcoinCashNetwork) -> Self {
        match network {
            BitcoinCashNetwork::Chipnet => Self::CHIPNET,
            BitcoinCashNetwork::Mainnet
            | BitcoinCashNetwork::Regtest
            | BitcoinCashNetwork::Scalenet => Self::MAINNET,
        }
    }

    /// Whether the coinbase of the block at `height`, with `confirmations`
    /// on the node's chain, can be spent in the next block: that block's
    /// ticks are at least [`BCH_COINBASE_MATURITY_TICKS`] past the
    /// coinbase's. That is 100 confirmations before activation and 1,000
    /// one-minute blocks after it.
    pub fn coinbase_spendable_in_next_block(&self, height: u32, confirmations: i64) -> bool {
        let Some(next) = i64::from(height)
            .checked_add(confirmations)
            .and_then(|next| u32::try_from(next).ok())
        else {
            return false;
        };
        confirmations > 0 && self.ticks(next) - self.ticks(height) >= BCH_COINBASE_MATURITY_TICKS
    }

    /// The first block height at which a transaction locked to height
    /// `lock_height` (`nLockTime`, or `OP_CHECKLOCKTIMEVERIFY`'s operand) is
    /// final.
    ///
    /// CHIP-2025-03 keeps a lock height in ten-minute units: it is final in
    /// a block whose ticks are past `600 * lock_height`. That is block
    /// `lock_height + 1` while blocks are ten minutes, and ten blocks per
    /// unit after the activation block.
    pub fn first_block_after_lock_height(&self, lock_height: u32) -> u32 {
        match self.activation_height {
            Some(activation) if lock_height >= activation => {
                let units = u64::from(lock_height - activation);
                let height = u64::from(activation) + units * 10 + 1;
                u32::try_from(height).unwrap_or(u32::MAX)
            }
            _ => lock_height.saturating_add(1),
        }
    }

    /// The highest block height whose ticks are at most `ticks`.
    fn height_at_or_below(&self, ticks: i64) -> u32 {
        let ticks = ticks.max(0);
        let height = match self.activation_height.map(i64::from) {
            Some(activation) if ticks > activation * BCH_BLOCK_SPACING => {
                activation + (ticks - activation * BCH_BLOCK_SPACING) / BCH_FAST_BLOCK_SPACING
            }
            _ => ticks / BCH_BLOCK_SPACING,
        };
        u32::try_from(height).unwrap_or(u32::MAX)
    }

    /// How many blocks back from the block at `tip` span at least `span`
    /// ticks: the fewest `n` with `ticks(tip) - ticks(tip - n) >= span`, or
    /// `tip` when the chain is shorter. 8,400 ten-minute blocks hold the
    /// span 84,000 one-minute blocks do.
    pub fn blocks_spanning(&self, tip: u32, span: i64) -> usize {
        let start = self.height_at_or_below(self.ticks(tip) - span).min(tip);
        (tip - start) as usize
    }
}

/// Bitcoin Cash coinbase maturity in ticks, CHIP-2025-03: 60,000, which is
/// 100 ten-minute blocks or 1,000 one-minute blocks.
pub const BCH_COINBASE_MATURITY_TICKS: i64 = 60_000;

/// The proof-of-work limit of mainnet, testnet3, testnet4, chipnet and
/// scalenet: `00000000ffff...ff`, 2^224 - 1. Its compact form is
/// [`BCH_POW_LIMIT_BITS`].
pub fn bch_pow_limit() -> U256 {
    (U256::one() << 224) - U256::one()
}

/// `GetCompact` of [`bch_pow_limit`]: the bits of a minimum-difficulty block.
pub const BCH_POW_LIMIT_BITS: u32 = 0x1d00_ffff;

/// Mainnet's band below the computed target: a header may be up to twice as
/// hard as its own time implies.
const MAINNET_HARD_DIVISOR: u32 = 2;

/// Test networks' band below the computed target: up to 8 times as hard.
const TEST_NETWORK_HARD_DIVISOR: u32 = 8;

/// Mainnet's band above the computed target: at most 5% easier,
/// `105 / 100`. A header's time can fall before its parent's by at most
/// BCHN's two-hour future limit, which with mainnet's two-day half-life
/// is a factor of 2^(7200 / 172800), about 1.029.
const MAINNET_EASY: (u32, u32) = (105, 100);

/// Test networks' band above the computed target: up to 8 times easier,
/// `8 / 1`. With the one-hour half-life a header dated two hours before
/// its parent computes a target 4 times too hard; 8 leaves room.
const TEST_NETWORK_EASY: (u32, u32) = (8, 1);

/// One network's ASERT anchor and half-life, from BCHN's chain parameters,
/// and the band P2Pool's header check allows around the computed target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BchAsert {
    /// Height of the anchor block.
    pub anchor_height: u32,
    /// The anchor block's nBits.
    pub anchor_bits: u32,
    /// Time of the anchor block's parent, which the absolute formulation of
    /// ASERT measures from.
    pub anchor_parent_time: i64,
    /// Seconds behind (ahead of) schedule that double (halve) the target.
    pub half_life: i64,
    /// Test networks allow minimum-difficulty blocks
    /// (`fPowAllowMinDifficultyBlocks`).
    pub allow_pow_limit: bool,
    /// A header may be this many times harder than the computed target.
    pub hard_divisor: u32,
    /// A header may be up to `easy_numerator / easy_denominator` times
    /// easier than the computed target.
    pub easy_numerator: u32,
    /// See [`Self::easy_numerator`].
    pub easy_denominator: u32,
    /// The network's move to one-minute blocks.
    pub fast_blocks: FastBlocks,
}

impl BchAsert {
    /// Mainnet (`CMainParams`): anchor block 661,647 on bits `0x1804dafe`,
    /// its parent at 1,605,447,844, and a two-day half-life.
    pub const MAINNET: Self = Self {
        anchor_height: 661_647,
        anchor_bits: 0x1804_dafe,
        anchor_parent_time: 1_605_447_844,
        half_life: 2 * 24 * 60 * 60,
        allow_pow_limit: false,
        hard_divisor: MAINNET_HARD_DIVISOR,
        easy_numerator: MAINNET_EASY.0,
        easy_denominator: MAINNET_EASY.1,
        fast_blocks: FastBlocks::MAINNET,
    };

    /// Chipnet (`CChipNetParams`), which shares testnet4's anchor
    /// (`CTestNet4Params`): block 16,844 on bits `0x1d00ffff`, its parent at
    /// 1,605,451,779, and a one-hour half-life.
    pub const CHIPNET: Self = Self {
        anchor_height: 16_844,
        anchor_bits: 0x1d00_ffff,
        anchor_parent_time: 1_605_451_779,
        half_life: 60 * 60,
        allow_pow_limit: true,
        hard_divisor: TEST_NETWORK_HARD_DIVISOR,
        easy_numerator: TEST_NETWORK_EASY.0,
        easy_denominator: TEST_NETWORK_EASY.1,
        fast_blocks: FastBlocks::CHIPNET,
    };

    /// Testnet4: the same anchor and half-life as chipnet, and mainnet's
    /// activation time.
    pub const TESTNET4: Self = Self {
        fast_blocks: FastBlocks::MAINNET,
        ..Self::CHIPNET
    };

    /// The parameters for a network P2Pool runs on. Regtest has no anchor in
    /// BCHN (nor does scalenet), so it has none here and its headers are not
    /// checked.
    pub fn for_network(network: BitcoinCashNetwork) -> Option<Self> {
        match network {
            BitcoinCashNetwork::Mainnet => Some(Self::MAINNET),
            BitcoinCashNetwork::Chipnet => Some(Self::CHIPNET),
            BitcoinCashNetwork::Regtest | BitcoinCashNetwork::Scalenet => None,
        }
    }

    /// The target ASERT requires of the block after one at `parent_height`
    /// and `parent_time`, as `GetNextASERTWorkRequired` computes it (before
    /// a test network's minimum-difficulty rule). `None` below the anchor,
    /// where ASERT does not govern.
    ///
    /// The parent's ticks and the new block's interval come from the
    /// network's activation ([`FastBlocks`]): after it the target is ten
    /// times easier, for a block ten times as frequent.
    pub fn next_target(&self, parent_height: u32, parent_time: u32) -> Option<U256> {
        if parent_height < self.anchor_height {
            return None;
        }
        let tick_delta = self.fast_blocks.ticks(parent_height)
            - i64::from(self.anchor_height) * BCH_BLOCK_SPACING;
        let time_diff = i64::from(parent_time) - self.anchor_parent_time;
        let reference = set_compact(self.anchor_bits)?;
        calculate_asert(
            reference,
            time_diff,
            tick_delta,
            BCH_BLOCK_SPACING,
            self.fast_blocks.interval_after(parent_height),
            self.half_life,
            bch_pow_limit(),
        )
    }

    /// How long after its parent a test network block may take the
    /// proof-of-work limit: more than twice the new block's interval, 20
    /// minutes before activation and 2 minutes after.
    pub fn min_difficulty_gap(&self, parent_height: u32) -> u32 {
        (2 * self.fast_blocks.interval_after(parent_height)) as u32
    }

    /// [`Self::next_target`] in compact form: the nBits BCHN requires.
    pub fn next_bits(&self, parent_height: u32, parent_time: u32) -> Option<CompactTarget> {
        self.next_target(parent_height, parent_time)
            .map(get_compact)
    }

    /// The ASERT format check of a header at `height` (its coinbase's BIP34
    /// height) with nBits `bits` and time `header_time`.
    ///
    /// The header's own time stands in for its parent's, so the target is
    /// computed as ASERT would from a parent at `height - 1` dated
    /// `header_time`, and the header's target must lie in the agreed band:
    ///
    /// - mainnet: `computed / 2 <= header_target <= computed * 1.05`;
    /// - test networks: `computed / 8 <= header_target <= computed * 8`, or
    ///   nBits exactly the proof-of-work limit's (a minimum-difficulty
    ///   block).
    ///
    /// The hard side is wide because a header can be minted hours after its
    /// parent. The easy side covers a header dated up to two hours before
    /// its parent, which BCHN accepts when the parent is dated ahead: a
    /// factor of about 1.029 on mainnet, 4 on the test networks' one-hour
    /// half-life. `computed / d` is floored and the easy side is compared
    /// exactly (`target * 100 <= computed * 105` on mainnet). The bands are
    /// the SAFA covenant's (`p2poolv2_safa`), number for number.
    pub fn check_header_bits(
        &self,
        bits: CompactTarget,
        header_time: u32,
        height: u64,
    ) -> Result<(), String> {
        let bits = bits.to_consensus();
        let target = set_compact(bits).ok_or_else(|| format!("invalid nBits {bits:#010x}"))?;
        if target > bch_pow_limit() {
            return Err(format!(
                "nBits {bits:#010x} is above the proof-of-work limit"
            ));
        }
        if self.allow_pow_limit && bits == BCH_POW_LIMIT_BITS {
            return Ok(());
        }
        let parent_height = height
            .checked_sub(1)
            .and_then(|height| u32::try_from(height).ok())
            .ok_or_else(|| format!("height {height} has no ASERT parent"))?;
        let computed = self
            .next_target(parent_height, header_time)
            .ok_or_else(|| {
                format!(
                    "height {height} is not above the ASERT anchor {}",
                    self.anchor_height
                )
            })?;
        let hard_floor = computed / U256::from(self.hard_divisor);
        if target < hard_floor {
            return Err(format!(
                "nBits {bits:#010x} is more than {} times harder than ASERT's {:#010x} at height {height}",
                self.hard_divisor,
                get_compact(computed).to_consensus()
            ));
        }
        // Both sides stay far below 2^256: the target is at most the limit,
        // 2^224 - 1, and so is the computed target.
        if target * U256::from(self.easy_denominator) > computed * U256::from(self.easy_numerator) {
            return Err(format!(
                "nBits {bits:#010x} is more than {}/{} times easier than ASERT's {:#010x} at height {height}",
                self.easy_numerator,
                self.easy_denominator,
                get_compact(computed).to_consensus()
            ));
        }
        Ok(())
    }
}

/// BCHN's `CalculateASERT` with CHIP-2025-03's ticks: `reference *
/// 2^((time_diff - (tick_delta + anchor_interval)) / half_life) *
/// anchor_interval / next_interval` in 16.16 fixed point, clamped to
/// `pow_limit` and to at least 1.
///
/// `tick_delta` is the parent's ticks less the anchor's, `600 *
/// height_diff` on a chain that has not activated one-minute blocks.
/// `anchor_interval` is the anchor block's interval in ticks, 600 on every
/// Bitcoin Cash network ([`BCH_BLOCK_SPACING`]), and `next_interval` the new
/// block's, 600 or 60. Scaling by the ratio of the two can pass 2^256; the
/// CHIP's reference then returns the limit, not the wrapped value, and so
/// does this.
///
/// `None` where BCHN asserts instead: a zero reference or one above the
/// limit, a negative tick difference, a non-positive half-life, an
/// interval that does not divide 600, or a time so far off schedule that
/// the fixed-point exponent would overflow (`|time_diff - tick_delta| >=
/// 2^47`).
pub fn calculate_asert(
    reference: U256,
    time_diff: i64,
    tick_delta: i64,
    anchor_interval: i64,
    next_interval: i64,
    half_life: i64,
    pow_limit: U256,
) -> Option<U256> {
    if reference.is_zero() || reference > pow_limit || tick_delta < 0 || half_life <= 0 {
        return None;
    }
    let divides_600 = |interval: i64| {
        (1..=BCH_BLOCK_SPACING).contains(&interval) && BCH_BLOCK_SPACING % interval == 0
    };
    if !divides_600(anchor_interval) || !divides_600(next_interval) {
        return None;
    }
    // BCHN needs leading zero bits in the limit to detect overflow; every
    // network's limit has 32.
    if !(pow_limit >> 224).is_zero() {
        return None;
    }
    let span = time_diff.checked_sub(tick_delta)?;
    if span.unsigned_abs() >= 1 << (63 - 16) {
        return None;
    }
    // C++ and Rust both truncate toward zero in integer division.
    let exponent = (span - anchor_interval) * 65_536 / half_life;
    // `>>` on a negative i64 is arithmetic in both: it floors, so the
    // fraction below is always in 0..65536.
    let mut shifts = exponent >> 16;
    let fraction = u128::from(exponent as u16);
    // 65536 * 2^(fraction / 65536), BCHN's cubic; at most 2^64 - 2^47 or so,
    // computed wide to be safe.
    let factor = 65_536
        + ((195_766_423_245_049 * fraction
            + 971_821_376 * fraction * fraction
            + 5_127 * fraction * fraction * fraction
            + (1 << 47))
            >> 48);
    // Below 2^241: the reference is below 2^224 and the factor below 2^17.
    let mut next = reference * U256::from(factor as u64);
    shifts -= 16;
    if shifts <= 0 {
        let right = shifts.unsigned_abs();
        next = if right >= 256 {
            U256::zero()
        } else {
            next >> right as usize
        };
    } else if next.bits() as u64 + shifts.unsigned_abs() > 256 {
        // BCHN's 256-bit shift would discard high bits: the limit.
        next = pow_limit;
    } else {
        next <<= shifts as usize;
    }
    // The interval ratio, a whole number from 1 to 600 either way. A
    // product past 2^256 is the limit (the CHIP's `ratio_guard_wrap_low`
    // vector).
    next = if anchor_interval >= next_interval {
        next.checked_mul(U256::from((anchor_interval / next_interval) as u64))
            .unwrap_or(pow_limit)
    } else {
        next / U256::from((next_interval / anchor_interval) as u64)
    };
    if next.is_zero() {
        next = U256::one();
    } else if next > pow_limit {
        next = pow_limit;
    }
    Some(next)
}

/// BCHN's `arith_uint256::SetCompact`, refusing a negative, zero or
/// overflowing value.
pub fn set_compact(bits: u32) -> Option<U256> {
    let size = bits >> 24;
    let word = bits & 0x007f_ffff;
    let value = if size <= 3 {
        U256::from(word >> (8 * (3 - size)))
    } else {
        let overflow =
            word != 0 && (size > 34 || (word > 0xff && size > 33) || (word > 0xffff && size > 32));
        if overflow {
            return None;
        }
        U256::from(word) << (8 * (size as usize - 3))
    };
    let negative = word != 0 && bits & 0x0080_0000 != 0;
    if negative || value.is_zero() {
        return None;
    }
    Some(value)
}

/// BCHN's `arith_uint256::GetCompact` for a non-negative value.
pub fn get_compact(value: U256) -> CompactTarget {
    let mut size = value.bits().div_ceil(8) as u32;
    let mut compact = if size <= 3 {
        (value.low_u64() << (8 * (3 - size))) as u32
    } else {
        (value >> (8 * (size as usize - 3))).low_u32()
    };
    if compact & 0x0080_0000 != 0 {
        compact >>= 8;
        size += 1;
    }
    CompactTarget::from_consensus(compact | size << 24)
}

/// The height a BCH coinbase scriptSig starts with (BIP34, as BCHN writes
/// it: `CScript() << nHeight`): `OP_1` to `OP_16`, or a minimal push of a
/// positive little-endian number of up to 5 bytes.
pub fn bip34_height(script_sig: &[u8]) -> Option<u64> {
    let (&opcode, rest) = script_sig.split_first()?;
    match opcode {
        0x51..=0x60 => Some(u64::from(opcode - 0x50)),
        1..=5 => {
            let bytes = rest.get(..usize::from(opcode))?;
            let last = *bytes.last()?;
            // Positive and minimal: no sign bit, and no needless zero byte.
            if last & 0x80 != 0
                || (last == 0 && (bytes.len() == 1 || bytes[bytes.len() - 2] & 0x80 == 0))
            {
                return None;
            }
            Some(
                bytes
                    .iter()
                    .rev()
                    .fold(0u64, |value, byte| value << 8 | u64::from(*byte)),
            )
        }
        _ => None,
    }
}

#[cfg(test)]
pub(crate) mod tests;
