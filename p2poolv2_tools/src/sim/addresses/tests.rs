// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use p2poolv2_wallet::bch_share::{CashAddrType, encode_cashaddr, parse};
use std::collections::HashSet;

/// HASH160 of the compressed public key with secret 1, the key behind
/// Bitcoin's well known `1BgGZ9tcN4rm9KBzDn7KprQz87SZ26SAMH`.
const SECRET_ONE_KEY_HASH: [u8; 20] = [
    0x75, 0x1e, 0x76, 0xe8, 0x19, 0x91, 0x96, 0xd4, 0x54, 0x94, 0x1c, 0x45, 0xd1, 0xb3, 0xa3, 0x23,
    0xf1, 0x43, 0x3b, 0xd6,
];

#[test]
fn key_hash_matches_the_well_known_secret_one_key() {
    assert_eq!(key_hash(1), SECRET_ONE_KEY_HASH);
}

#[test]
fn default_miner_address_is_the_secret_one_key_on_regtest() {
    assert_eq!(
        DEFAULT_MINER_ADDRESS,
        encode_cashaddr("bchreg", CashAddrType::PayToPublicKeyHash, &key_hash(1))
    );
    assert_eq!(check_payout_address(DEFAULT_MINER_ADDRESS), Ok(()));
}

#[test]
fn share_addresses_are_regtest_share_addresses() {
    for index in 0..3 {
        let address = share_address(index);
        let decoded = parse(&address, BchShareNetwork::Regtest).expect("regtest share address");

        assert!(address.starts_with("rp2pool:q"), "{address}");
        assert_eq!(decoded.hash, key_hash(FIRST_SHARE_SECRET + index as u64));
    }
}

#[test]
fn every_node_has_its_own_share_owner() {
    let owners: HashSet<String> = (0..20).map(share_address).collect();

    assert_eq!(owners.len(), 20);
}

#[test]
fn no_share_owner_uses_the_default_payout_key() {
    let payout_hash = key_hash(1);

    for index in 0..20 {
        let owner = parse(&share_address(index), BchShareNetwork::Regtest).expect("share address");
        assert_ne!(owner.hash, payout_hash);
    }
}

#[test]
fn a_bitcoin_address_is_not_a_payout_address() {
    let error = check_payout_address("bcrt1qqclp5usts33x0cgy2l5839659t7798w7g5a0fu").unwrap_err();

    assert!(error.contains("MINER_ADDRESS"), "{error}");
}

#[test]
fn a_chipnet_address_is_not_a_regtest_payout_address() {
    let chipnet = encode_cashaddr("bchtest", CashAddrType::PayToPublicKeyHash, &key_hash(1));

    assert!(check_payout_address(&chipnet).is_err());
}
