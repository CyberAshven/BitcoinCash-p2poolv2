// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;

/// BIP086 output key of the genesis NUMS pubkey, encoded for each network.
/// The test helper config uses Signet.
const SIGNET_ADDRESS: &str = "sp2pool1pvmde7zkgeg9qqcpsy7e6g3w6dm3d7mqwqnudcmuedk6wt8gwgkls4zffd6";
const TESTNET4_ADDRESS: &str =
    "tp2pool1pvmde7zkgeg9qqcpsy7e6g3w6dm3d7mqwqnudcmuedk6wt8gwgkls3qc3th";

/// A p2poolv2 pool with no configured address is a normal setup, not an
/// edge case: each miner supplies its own with `p2p=`. The mode is set
/// explicitly rather than left to `PoolMode::default()`, so this keeps
/// covering p2poolv2 even if that default ever changes.
#[test]
fn absent_miner_address_parses_to_none() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.mode = PoolMode::P2poolv2;

    let parsed = config.parse().unwrap();
    assert!(parsed.miner_address().is_none());
    assert_eq!(parsed.mode, PoolMode::P2poolv2);
}

#[test]
fn valid_miner_address_is_parsed_and_exposed() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.miner_address = Some(SIGNET_ADDRESS.to_string());

    let parsed = config.parse().unwrap();

    assert_eq!(
        parsed.miner_address().map(|address| address.to_string()),
        Some(SIGNET_ADDRESS.to_string())
    );
    assert_eq!(parsed.miner_address().unwrap().network(), Network::Signet);
}

/// A share address for another network must be rejected at config parse,
/// so the node fails at startup rather than assigning every share on the
/// pool to an address the operator cannot use.
#[test]
fn miner_address_for_another_network_is_rejected() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.miner_address = Some(TESTNET4_ADDRESS.to_string());

    let error = config.parse().unwrap_err();

    assert!(
        error.message.contains("share chain address"),
        "unexpected message: {}",
        error.message
    );
}

#[test]
fn malformed_miner_address_is_rejected() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.miner_address = Some("not-a-share-address".to_string());

    let error = config.parse().unwrap_err();

    assert!(
        error.message.contains("share chain address"),
        "unexpected message: {}",
        error.message
    );
}

/// A bitcoin address here is the mistake the whole address type exists to
/// prevent, so it must fail loudly at startup.
#[test]
fn bitcoin_address_as_miner_address_is_rejected() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.miner_address = Some("tb1qyazxde6558qj6z3d9np5e6msmrspwpf6k0qggk".to_string());

    assert!(config.parse().is_err());
}

/// Hydrapool builds no share commitment, so the field could never be used.
#[test]
fn miner_address_with_hydrapool_mode_is_rejected() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.miner_address = Some(SIGNET_ADDRESS.to_string());
    config.mode = PoolMode::Hydrapool;

    let error = config.parse().unwrap_err();

    assert!(
        error.message.contains("hydrapool"),
        "unexpected message: {}",
        error.message
    );
}

#[test]
fn hydrapool_without_a_miner_address_still_parses() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.mode = PoolMode::Hydrapool;

    assert!(config.parse().is_ok());
}

#[test]
fn miner_address_with_p2poolv2_mode_is_accepted() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.miner_address = Some(SIGNET_ADDRESS.to_string());
    config.mode = PoolMode::P2poolv2;

    assert!(config.parse().is_ok());
}

/// A Bitcoin Cash P2Pool node may merge-mine tokens for its miners.
#[test]
fn a_bitcoin_cash_p2pool_node_takes_a_token_file() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.chain = Chain::BitcoinCash;
    config.bitcoin_cash_network = Some(BitcoinCashNetwork::Chipnet);
    config.bootstrap_address = "bchtest:pr6m7j9njldwwzlg9v7v53unlr4jkmx6eyvwc0uz5t".to_string();
    config.merge_mining_tokens = Some("merge-tokens.json".to_string());

    let parsed = config.parse().unwrap();

    assert_eq!(
        parsed.merge_mining_tokens.as_deref(),
        Some("merge-tokens.json")
    );
}

/// Only the Bitcoin Cash coinbase a P2Pool node builds for each miner
/// carries their tokens: Bitcoin and Hydrapool refuse a token file.
#[test]
fn a_token_file_needs_a_bitcoin_cash_p2pool_node() {
    let mut bitcoin = StratumConfig::<Raw>::new_for_test_default();
    bitcoin.merge_mining_tokens = Some("merge-tokens.json".to_string());
    assert!(
        bitcoin
            .parse()
            .unwrap_err()
            .message
            .contains("merge_mining_tokens")
    );

    let mut hydrapool = StratumConfig::<Raw>::new_for_test_default();
    hydrapool.chain = Chain::BitcoinCash;
    hydrapool.bitcoin_cash_network = Some(BitcoinCashNetwork::Chipnet);
    hydrapool.bootstrap_address = "bchtest:pr6m7j9njldwwzlg9v7v53unlr4jkmx6eyvwc0uz5t".to_string();
    hydrapool.mode = PoolMode::Hydrapool;
    hydrapool.merge_mining_tokens = Some("merge-tokens.json".to_string());
    assert!(hydrapool.parse().is_err());
}
