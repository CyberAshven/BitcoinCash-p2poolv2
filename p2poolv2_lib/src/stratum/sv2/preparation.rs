// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Prepare BCH coinbase bytes for an external Job Declaration client without
//! duplicating P2Pool's payout or share-commitment rules in that client.
//! This calculation grants no mining authorization: the SV2 listener still
//! checks its own token registry, payouts, template and submitted proof.

use crate::accounting::payout::coinbase_budget::bch_split_for_commitment;
use crate::accounting::payout::payout_distribution::build_pplns_outputs;
use crate::accounting::payout::payout_levels::PplnsWeights;
use crate::shares::bch_coinbase::BchCoinbase;
use crate::shares::share_commitment::ShareCommitment;
use crate::stratum::work::coinbase::{
    EXTRANONCE_SEPARATOR, LOCKTIME_LENGTH, SEQUENCE_LENGTH, build_declared_coinbase,
    check_bch_coinbase_limit,
};
use bitcoin::Amount;
use bitcoin::consensus::serialize;
use bitcoin::hashes::Hash;
use std::fmt;

#[derive(Debug)]
pub struct PreparedCoinbase {
    pub prefix: Vec<u8>,
    pub suffix: Vec<u8>,
    pub token_hash: [u8; 32],
}

impl PreparedCoinbase {
    /// The coinbase's outputs as `SetCustomMiningJob.coinbase_tx_outputs`
    /// carries them: a CompactSize count, then the serialized outputs. In
    /// the suffix they sit between the input's sequence and the lock time.
    pub fn outputs(&self) -> &[u8] {
        &self.suffix[SEQUENCE_LENGTH..self.suffix.len() - LOCKTIME_LENGTH]
    }
}

/// Why a declared coinbase could not be prepared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreparationError {
    /// The builders refuse an input: the revenue, a cut in the job token,
    /// the scriptSig prefix or merge-mining commitment, or the payouts.
    Invalid(String),
    /// The coinbase exceeds the share chain's limit for its commitment, or
    /// its prefix or suffix the 65,535 bytes a Job Declaration field carries.
    TooLarge(String),
}

impl fmt::Display for PreparationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PreparationError::Invalid(message) | PreparationError::TooLarge(message) => {
                formatter.write_str(message)
            }
        }
    }
}

impl std::error::Error for PreparationError {}

/// The caller obtains exact difficulty weights from the token's share-chain
/// anchor. Neither an empty window nor unavailable history has a fallback payee.
///
/// The payouts follow the Bitcoin Cash hybrid payout at the limit the
/// descriptor's merge-mining commitment sets (T for a token commitment,
/// BCH's transaction limit for the exact empty one), as the validator
/// rebuilds them, and the coinbase must fit that limit. A Job Declaration
/// message carries the coinbase prefix and suffix in fields of at most
/// 65,535 bytes each, so a larger coinbase is refused here with a clear
/// error rather than sent truncated.
pub fn prepare_coinbase(
    token: &[u8],
    commitment: ShareCommitment,
    weights: &PplnsWeights,
    value: u64,
    height: u32,
    descriptor: BchCoinbase,
) -> Result<PreparedCoinbase, String> {
    prepare_declared_coinbase(token, commitment, weights, value, height, descriptor)
        .map_err(|error| error.to_string())
}

/// [`prepare_coinbase`], telling an input the builders refuse from a
/// coinbase too large to declare, so the requested payout outputs
/// extension can answer each with its own error code.
pub fn prepare_declared_coinbase(
    token: &[u8],
    mut commitment: ShareCommitment,
    weights: &PplnsWeights,
    value: u64,
    height: u32,
    descriptor: BchCoinbase,
) -> Result<PreparedCoinbase, PreparationError> {
    if value > Amount::MAX_MONEY.to_sat() {
        return Err(PreparationError::Invalid(
            "coinbase revenue exceeds BCH monetary range".into(),
        ));
    }
    for (address, rate) in [
        (&commitment.donation_address, commitment.donation),
        (&commitment.fee_address, commitment.fee),
    ] {
        if rate.is_some_and(|rate| rate > 10_000 || (rate > 0 && address.is_none())) {
            return Err(PreparationError::Invalid(
                "invalid payout cut in job token".into(),
            ));
        }
    }
    descriptor
        .validate(u64::from(height))
        .map_err(|error| PreparationError::Invalid(error.to_owned()))?;
    let split = bch_split_for_commitment(&descriptor.merge_mining);
    let payouts = build_pplns_outputs(
        value,
        weights,
        &commitment.donation_address,
        commitment.donation,
        &commitment.fee_address,
        commitment.fee,
        split,
    )
    .map_err(|error| PreparationError::Invalid(error.to_string()))?;
    commitment.bch_coinbase = Some(descriptor.clone());
    let coinbase = build_declared_coinbase(
        &payouts,
        u64::from(height),
        commitment.hash(),
        &EXTRANONCE_SEPARATOR,
        &descriptor,
    )
    .map_err(|error| PreparationError::Invalid(error.to_string()))?;
    check_bch_coinbase_limit(&coinbase, split)
        .map_err(|error| PreparationError::TooLarge(error.to_string()))?;
    let raw = serialize(&coinbase);
    // Version, singleton input count, null outpoint and bounded script length.
    let offset = 4 + 1 + 36 + 1 + descriptor.prefix.len();
    let end = offset + EXTRANONCE_SEPARATOR.len();
    if raw.get(offset..end) != Some(EXTRANONCE_SEPARATOR.as_slice()) {
        return Err(PreparationError::Invalid(
            "unexpected declared coinbase layout".into(),
        ));
    }
    let prefix = raw[..offset].to_vec();
    let suffix = raw[end..].to_vec();
    if prefix.len() > 65535 || suffix.len() > 65535 {
        return Err(PreparationError::TooLarge(format!(
            "declared coinbase prefix of {} bytes or suffix of {} bytes exceeds the 65,535 bytes a Job Declaration field carries",
            prefix.len(),
            suffix.len()
        )));
    }
    Ok(PreparedCoinbase {
        prefix,
        suffix,
        token_hash: bitcoin::hashes::sha256::Hash::hash(token).to_byte_array(),
    })
}

#[cfg(test)]
mod tests;
