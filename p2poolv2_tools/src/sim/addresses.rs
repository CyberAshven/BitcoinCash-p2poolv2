// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The swarm's Bitcoin Cash regtest identities: the default payout address
//! and each node's share address.
//!
//! Both come from throwaway keys whose secrets are small integers, so anyone
//! can spend from them; they exist for regtest only. The payout address is
//! a `bchreg:` CashAddr, the share address an `rp2pool:` one. They are
//! separate settings of a sim node and use different keys: a share address
//! cannot be derived from the payout address.

use bitcoin::CompressedPublicKey;
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::{Secp256k1, SecretKey, SignOnly};
use p2poolv2_config::{BitcoinCashNetwork, Chain, parse_payout_address};
use p2poolv2_wallet::bch_share::{BchShareAddress, BchShareNetwork};
use std::sync::LazyLock;

/// Default `MINER_ADDRESS`: the regtest P2PKH CashAddr of the key with
/// secret 1. Every node falls back to it when the wallet hands out no
/// address of its own.
pub const DEFAULT_MINER_ADDRESS: &str = "bchreg:qp63uahgrxged4z5jswyt5dn5v3lzsem6c6mz8vuwd";

/// Secret of the key behind node 0's share address. Node `i` uses secret
/// `FIRST_SHARE_SECRET + i`, so no node shares its owner with another node
/// or with the default payout key.
const FIRST_SHARE_SECRET: u64 = 2;

static SECP: LazyLock<Secp256k1<SignOnly>> = LazyLock::new(Secp256k1::signing_only);

/// HASH160 of the compressed public key whose secret is `secret`.
fn key_hash(secret: u64) -> [u8; 20] {
    let mut bytes = [0u8; 32];
    bytes[24..].copy_from_slice(&secret.to_be_bytes());
    let secret_key =
        SecretKey::from_slice(&bytes).expect("a small nonzero integer is a valid secret key");
    CompressedPublicKey(secret_key.public_key(&SECP))
        .pubkey_hash()
        .to_byte_array()
}

/// The share address of node `index`: the `rp2pool:` CashAddr of the key
/// with secret `FIRST_SHARE_SECRET + index`. It owns the node's share
/// coinbase outputs. Without one the sim emitter refuses to start and the
/// swarm produces no shares at all.
pub fn share_address(index: usize) -> String {
    BchShareAddress::new(
        BchShareNetwork::Regtest,
        key_hash(FIRST_SHARE_SECRET + index as u64),
    )
    .encode()
}

/// An error unless `address` is a Bitcoin Cash regtest payout address, read
/// with the node's own parser, so a bad `MINER_ADDRESS` stops the launch
/// instead of every node refusing its config.
pub fn check_payout_address(address: &str) -> Result<(), String> {
    parse_payout_address(
        address,
        Chain::BitcoinCash,
        bitcoin::Network::Regtest,
        Some(BitcoinCashNetwork::Regtest),
    )
    .map(drop)
    .map_err(|error| format!("MINER_ADDRESS {address:?} is not a bchreg: CashAddr: {error}"))
}

#[cfg(test)]
mod tests;
