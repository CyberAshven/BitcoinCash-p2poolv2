// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Compact targets and BCHN's integer ASERT calculation.

use libauth::big_int_to_vm_number;

use crate::{BigInt, EASE_CAP, FALLBACK_DELTA, MAX_THREADS, network_parameters};

pub const POW_LIMIT_BITS: u32 = 0x1d00ffff;

/// BCH's proof-of-work limit, 2^224 - 1.
pub fn pow_limit() -> BigInt {
    BigInt::from_u64(1).shl(224).sub(&BigInt::from_u64(1))
}

/// The easiest canonical token target (compact bits 0x207fffff).
pub fn max_token_target() -> BigInt {
    BigInt::from_u64(0x7fffff).shl(232)
}

/// Expand a token target with exponent 3..=32 and positive 23-bit mantissa.
pub fn expand_token_target(bits: u32) -> Result<BigInt, String> {
    let exponent = bits >> 24;
    let mantissa = bits & 0xffffff;
    if !(3..=32).contains(&exponent) || mantissa == 0 || mantissa >= 0x800000 {
        return Err("target bits are not a canonical token target".into());
    }
    Ok(BigInt::from_u64(u64::from(mantissa)).shl(u64::from(8 * (exponent - 3))))
}

/// Round a positive target down to the covenant's compact precision.
pub fn encode_token_target(target: &BigInt) -> Result<u32, String> {
    if target.is_negative() || target.is_zero() || target.cmp(&max_token_target()).is_gt() {
        return Err("token target must be from 1 to 0x7fffff << 232".into());
    }
    let size = big_int_to_vm_number(target).len().max(3);
    let mantissa = target
        .shr((8 * (size - 3)) as u64)
        .to_u64()
        .ok_or("target mantissa overflow")?;
    Ok(((size as u32) << 24) | mantissa as u32)
}

/// BCHN's SetCompact, rejecting negative, zero, and overflowing values.
pub fn set_compact(bits: u32) -> Option<BigInt> {
    let size = bits >> 24;
    let word = bits & 0x7fffff;
    let value = if size <= 3 {
        BigInt::from_u64(u64::from(word >> (8 * (3 - size))))
    } else {
        if word != 0 && (size > 34 || (word > 0xff && size > 33) || (word > 0xffff && size > 32)) {
            return None;
        }
        BigInt::from_u64(u64::from(word)).shl(u64::from(8 * (size - 3)))
    };
    if word != 0 && bits & 0x800000 != 0 || value.is_zero() {
        None
    } else {
        Some(value)
    }
}

/// BCHN's GetCompact for an unsigned target.
pub fn get_compact(value: &BigInt) -> u32 {
    let mut size = value.bit_length().div_ceil(8);
    let mut compact = if size <= 3 {
        value.shl((8 * (3 - size)) as u64)
    } else {
        value.shr((8 * (size - 3)) as u64)
    }
    .to_u64()
    .expect("compact mantissa fits u64") as u32;
    if compact & 0x800000 != 0 {
        compact >>= 8;
        size += 1;
    }
    compact | ((size as u32) << 24)
}

/// Expand header nBits and enforce BCH's proof-of-work limit.
pub fn decode_header_bits(bits: u32) -> Option<BigInt> {
    set_compact(bits).filter(|target| !target.cmp(&pow_limit()).is_gt())
}

/// BCHN CalculateASERT, including truncating division and flooring shifts.
///
/// `half_life` must be positive. Deployment parameters enforce this invariant.
pub fn calculate_asert(
    reference: &BigInt,
    time_diff: i64,
    height_diff: i64,
    half_life: i64,
) -> BigInt {
    assert!(half_life > 0, "ASERT half-life must be positive");
    let span = i128::from(time_diff) - 600 * i128::from(height_diff);
    let exponent = ((span - 600) * 65_536) / i128::from(half_life);
    let shifts = exponent >> 16;
    let fraction = (exponent - shifts * 65_536) as u128;
    let factor = 65_536
        + ((195_766_423_245_049 * fraction
            + 971_821_376 * fraction * fraction
            + 5_127 * fraction * fraction * fraction
            + (1u128 << 47))
            >> 48);
    let next = reference.mul(&BigInt::from_u64(factor as u64));
    let shift = shifts - 16;
    let next = if shift <= 0 {
        next.shr((-shift).min(256) as u64)
    } else {
        next.shl(shift.min(256) as u64)
    };
    if next.is_zero() {
        BigInt::from_u64(1)
    } else if next.cmp(&pow_limit()).is_gt() {
        pow_limit()
    } else {
        next
    }
}

/// Compute the deployment's ASERT target for the claimed height and time.
pub fn asert_target(network: &str, height: u32, time: u32) -> Result<Option<BigInt>, String> {
    let parameters = network_parameters(network)?;
    let height_diff = i64::from(height) - 1 - i64::from(parameters.anchor_height);
    if height_diff < 0 {
        return Ok(None);
    }
    let reference = set_compact(parameters.anchor_bits).ok_or("invalid ASERT anchor")?;
    Ok(Some(calculate_asert(
        &reference,
        i64::from(time) - i64::from(parameters.anchor_parent_time),
        height_diff,
        i64::from(parameters.half_life),
    )))
}

/// Check the agreed mainnet /2..x1.05 or chipnet /8..x8 ASERT band.
pub fn asert_band_ok(network: &str, bits: u32, time: u32, height: u32) -> Result<bool, String> {
    let parameters = network_parameters(network)?;
    let Some(target) = decode_header_bits(bits) else {
        return Ok(false);
    };
    if parameters.allow_pow_limit && bits == POW_LIMIT_BITS {
        return Ok(true);
    }
    let Some(computed) = asert_target(network, height, time)? else {
        return Ok(false);
    };
    Ok(!target
        .cmp(
            &computed
                .div_mod(&BigInt::from_u64(u64::from(parameters.hard_divisor)))
                .0,
        )
        .is_lt()
        && !target
            .mul(&BigInt::from_u64(u64::from(parameters.easy_denominator)))
            .cmp(&computed.mul(&BigInt::from_u64(u64::from(parameters.easy_numerator))))
            .is_gt())
}

/// Check the fallback band after at least 4,032 blocks without a claim.
pub fn fallback_band_ok(bits: u32, reference_bits: u32, height_delta: u32) -> bool {
    match (decode_header_bits(bits), decode_header_bits(reference_bits)) {
        (Some(target), Some(reference)) if height_delta >= FALLBACK_DELTA => {
            !target.cmp(&reference.shr(4)).is_lt()
                && !target
                    .mul(&BigInt::from_u64(10))
                    .cmp(&reference.mul(&BigInt::from_u64(11)))
                    .is_gt()
        }
        _ => false,
    }
}

/// Ease a thread's target by min(height_delta, 288) / threads.
pub fn claim_target_bits(state_bits: u32, height_delta: u32, threads: u32) -> Result<u32, String> {
    if !(1..=MAX_THREADS).contains(&threads) {
        return Err(format!(
            "threads must be an integer from 1 to {MAX_THREADS}"
        ));
    }
    let factor = u64::from(height_delta.min(EASE_CAP)) * 5_000 / u64::from(threads) + 5_000;
    let target = expand_token_target(state_bits)?
        .mul(&BigInt::from_u64(factor))
        .div_mod(&BigInt::from_u64(10_000))
        .0;
    let target = if target.is_zero() {
        BigInt::from_u64(1)
    } else if target.cmp(&max_token_target()).is_gt() {
        max_token_target()
    } else {
        target
    };
    encode_token_target(&target)
}
