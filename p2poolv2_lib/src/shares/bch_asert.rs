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

/// Every Bitcoin Cash network's block spacing, `nPowTargetSpacing`: 600 s.
pub const BCH_BLOCK_SPACING: i64 = 600;

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
    };

    /// Testnet4: the same anchor and half-life as chipnet.
    pub const TESTNET4: Self = Self::CHIPNET;

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
    pub fn next_target(&self, parent_height: u32, parent_time: u32) -> Option<U256> {
        let height_diff = i64::from(parent_height.checked_sub(self.anchor_height)?);
        let time_diff = i64::from(parent_time) - self.anchor_parent_time;
        let reference = set_compact(self.anchor_bits)?;
        calculate_asert(
            reference,
            time_diff,
            height_diff,
            self.half_life,
            bch_pow_limit(),
        )
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
    /// the SAFA covenant's (`tools/safa-auxpow`), number for number.
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

/// BCHN's `CalculateASERT`: `reference * 2^((time_diff - 600 *
/// (height_diff + 1)) / half_life)` in 16.16 fixed point, clamped to
/// `pow_limit` and to at least 1.
///
/// `None` where BCHN asserts instead: a zero reference or one above the
/// limit, a negative height difference, a non-positive half-life, or a time
/// so far off schedule that the fixed-point exponent would overflow
/// (`|time_diff - 600 * height_diff| >= 2^47`).
pub fn calculate_asert(
    reference: U256,
    time_diff: i64,
    height_diff: i64,
    half_life: i64,
    pow_limit: U256,
) -> Option<U256> {
    if reference.is_zero() || reference > pow_limit || height_diff < 0 || half_life <= 0 {
        return None;
    }
    // BCHN needs leading zero bits in the limit to detect overflow; every
    // network's limit has 32.
    if !(pow_limit >> 224).is_zero() {
        return None;
    }
    let span = time_diff.checked_sub(BCH_BLOCK_SPACING.checked_mul(height_diff)?)?;
    if span.unsigned_abs() >= 1 << (63 - 16) {
        return None;
    }
    // C++ and Rust both truncate toward zero in integer division.
    let exponent = (span - BCH_BLOCK_SPACING) * 65_536 / half_life;
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
mod tests;
