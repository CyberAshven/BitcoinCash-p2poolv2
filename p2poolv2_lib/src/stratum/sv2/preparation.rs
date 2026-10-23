// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Prepare BCH coinbase bytes for an external Job Declaration client without
//! duplicating P2Pool's payout or share-commitment rules in that client.
//! This calculation grants no mining authorization: the SV2 listener still
//! checks its own token registry, payouts, template and submitted proof.

use crate::accounting::payout::coinbase_budget::PayoutSplit;
use crate::accounting::payout::payout_distribution::build_pplns_outputs;
use crate::config::Chain;
use crate::shares::bch_coinbase::BchCoinbase;
use crate::shares::share_commitment::ShareCommitment;
use crate::stratum::work::coinbase::{
    EXTRANONCE_SEPARATOR, build_declared_coinbase, check_bch_coinbase_budget,
};
use bitcoin::consensus::serialize;
use bitcoin::hashes::Hash;
use bitcoin::{Address, Amount};
use std::collections::HashMap;

#[derive(Debug)]
pub struct PreparedCoinbase {
    pub prefix: Vec<u8>,
    pub suffix: Vec<u8>,
    pub token_hash: [u8; 32],
}

/// The caller obtains exact difficulty weights from the token's share-chain
/// anchor. Neither an empty window nor unavailable history has a fallback payee.
///
/// The payouts follow the Bitcoin Cash coinbase budget anchored at the
/// token's prev share, as the validator rebuilds them, and the coinbase must
/// fit `BCH_COINBASE_BYTE_BUDGET`.
pub fn prepare_coinbase(
    token: &[u8],
    mut commitment: ShareCommitment,
    weights: &HashMap<Address, u128>,
    value: u64,
    height: u32,
    descriptor: BchCoinbase,
) -> Result<PreparedCoinbase, String> {
    if value > Amount::MAX_MONEY.to_sat() {
        return Err("coinbase revenue exceeds BCH monetary range".into());
    }
    for (address, rate) in [
        (&commitment.donation_address, commitment.donation),
        (&commitment.fee_address, commitment.fee),
    ] {
        if rate.is_some_and(|rate| rate > 10_000 || (rate > 0 && address.is_none())) {
            return Err("invalid payout cut in job token".into());
        }
    }
    descriptor
        .validate(u64::from(height))
        .map_err(str::to_owned)?;
    let outputs = build_pplns_outputs(
        value,
        weights,
        &commitment.donation_address,
        commitment.donation,
        &commitment.fee_address,
        commitment.fee,
        PayoutSplit::for_chain(Chain::BitcoinCash, commitment.prev_share_blockhash),
    )
    .map_err(|error| error.to_string())?;
    commitment.bch_coinbase = Some(descriptor.clone());
    let coinbase = build_declared_coinbase(
        &outputs,
        u64::from(height),
        commitment.hash(),
        &EXTRANONCE_SEPARATOR,
        &descriptor,
    )
    .map_err(|error| error.to_string())?;
    check_bch_coinbase_budget(&coinbase).map_err(|error| error.to_string())?;
    let raw = serialize(&coinbase);
    // Version, singleton input count, null outpoint and bounded script length.
    let offset = 4 + 1 + 36 + 1 + descriptor.prefix.len();
    let end = offset + EXTRANONCE_SEPARATOR.len();
    if raw.get(offset..end) != Some(EXTRANONCE_SEPARATOR.as_slice()) {
        return Err("unexpected declared coinbase layout".into());
    }
    let prefix = raw[..offset].to_vec();
    let suffix = raw[end..].to_vec();
    if prefix.len() > 65535 || suffix.len() > 65535 {
        return Err("declared coinbase exceeds SV2 field limits".into());
    }
    Ok(PreparedCoinbase {
        prefix,
        suffix,
        token_hash: bitcoin::hashes::sha256::Hash::hash(token).to_byte_array(),
    })
}

#[cfg(test)]
mod tests;
