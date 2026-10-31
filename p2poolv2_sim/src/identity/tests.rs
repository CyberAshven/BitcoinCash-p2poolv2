// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use bitcoin::PubkeyHash;
use bitcoin::hashes::Hash;
use p2poolv2_lib::address::bch_share::{
    BchShareAddress, BchShareNetwork, CashAddrType, encode_cashaddr,
};

const BITCOIN_CASH_REGTEST: ChainSelection = ChainSelection {
    chain: Chain::BitcoinCash,
    network: Network::Regtest,
    bitcoin_cash_network: Some(BitcoinCashNetwork::Regtest),
};

const PAYOUT_HASH: [u8; 20] = [0x11; 20];
const OWNER_HASH: [u8; 20] = [0x22; 20];

fn bchreg_payout() -> String {
    encode_cashaddr("bchreg", CashAddrType::PayToPublicKeyHash, &PAYOUT_HASH)
}

fn rp2pool_share() -> String {
    BchShareAddress::new(BchShareNetwork::Regtest, OWNER_HASH).encode()
}

#[test]
fn bitcoin_cash_reads_a_bchreg_payout_and_an_rp2pool_share_owner() {
    let payout = bchreg_payout();
    let share = rp2pool_share();
    assert!(payout.starts_with("bchreg:q"));
    assert!(share.starts_with("rp2pool:q"));

    let identity = resolve(BITCOIN_CASH_REGTEST, &payout, Some(&share), None).unwrap();

    assert_eq!(
        identity.payout.script_pubkey(),
        bitcoin::ScriptBuf::new_p2pkh(&PubkeyHash::from_byte_array(PAYOUT_HASH))
    );
    assert_eq!(identity.share_owner, ShareOwner::CashP2pkh(OWNER_HASH));
}

#[test]
fn bitcoin_cash_falls_back_to_the_stratum_share_address() {
    let identity = resolve(
        BITCOIN_CASH_REGTEST,
        &bchreg_payout(),
        None,
        Some(&rp2pool_share()),
    )
    .unwrap();

    assert_eq!(identity.share_owner, ShareOwner::CashP2pkh(OWNER_HASH));
}

#[test]
fn bitcoin_cash_rejects_a_bitcoin_payout_address() {
    let error = resolve(
        BITCOIN_CASH_REGTEST,
        "bcrt1qqclp5usts33x0cgy2l5839659t7798w7g5a0fu",
        Some(&rp2pool_share()),
        None,
    )
    .unwrap_err();

    assert!(error.contains("sim miner_address"), "{error}");
}

#[test]
fn bitcoin_cash_rejects_a_payout_for_another_network() {
    let chipnet = encode_cashaddr("bchtest", CashAddrType::PayToPublicKeyHash, &PAYOUT_HASH);

    let error = resolve(BITCOIN_CASH_REGTEST, &chipnet, Some(&rp2pool_share()), None).unwrap_err();

    assert!(error.contains("sim miner_address"), "{error}");
}

#[test]
fn bitcoin_cash_rejects_a_chipnet_share_address_on_regtest() {
    let chipnet_share = BchShareAddress::new(BchShareNetwork::Chipnet, OWNER_HASH).encode();

    let error = resolve(
        BITCOIN_CASH_REGTEST,
        &bchreg_payout(),
        Some(&chipnet_share),
        None,
    )
    .unwrap_err();

    assert!(error.contains("sim share_address"), "{error}");
}

#[test]
fn bitcoin_cash_rejects_a_bitcoin_share_address() {
    let error = resolve(
        BITCOIN_CASH_REGTEST,
        &bchreg_payout(),
        None,
        Some("rp2pool1pmfr3p9j00pfxjh0zmgp99y8zftmd3s5pmedqhyptwy6lm87hf5ssmq4eld"),
    )
    .unwrap_err();

    assert!(error.contains("stratum miner_address"), "{error}");
}

#[test]
fn a_missing_share_address_is_an_error() {
    let error = resolve(BITCOIN_CASH_REGTEST, &bchreg_payout(), None, None).unwrap_err();

    assert!(error.contains("[sim] share_address"), "{error}");
}

#[test]
fn bitcoin_keeps_its_own_address_parsers() {
    let selection = ChainSelection {
        chain: Chain::Bitcoin,
        network: Network::Regtest,
        bitcoin_cash_network: None,
    };

    let identity = resolve(
        selection,
        "bcrt1qqclp5usts33x0cgy2l5839659t7798w7g5a0fu",
        Some("rp2pool1pmfr3p9j00pfxjh0zmgp99y8zftmd3s5pmedqhyptwy6lm87hf5ssmq4eld"),
        None,
    )
    .unwrap();

    assert!(matches!(identity.share_owner, ShareOwner::Taproot(_)));
    assert!(
        resolve(selection, &bchreg_payout(), Some(&rp2pool_share()), None).is_err(),
        "a Bitcoin node must not accept a CashAddr payout"
    );
}
