// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;

const CHIPNET_BOOTSTRAP: &str = "bchtest:pr6m7j9njldwwzlg9v7v53unlr4jkmx6eyvwc0uz5t";
const MAINNET_BOOTSTRAP: &str = "bitcoincash:qr6m7j9njldwwzlg9v7v53unlr4jkmx6eylep8ekg2";

#[test]
fn mainnet_donation_string_decodes_to_the_built_in_key() {
    let decoded = cashaddr::decode(BITCOIN_CASH_DONATION_ADDRESS, BitcoinCashNetwork::Mainnet)
        .unwrap()
        .to_payout_address();
    assert_eq!(
        decoded,
        bitcoin_cash_donation_address(BitcoinCashNetwork::Mainnet)
    );
}

#[test]
fn chipnet_donation_string_decodes_to_the_built_in_key() {
    let decoded = cashaddr::decode(
        bitcoin_cash_donation_cashaddr(BitcoinCashNetwork::Chipnet),
        BitcoinCashNetwork::Chipnet,
    )
    .unwrap()
    .to_payout_address();
    assert_eq!(
        decoded,
        bitcoin_cash_donation_address(BitcoinCashNetwork::Chipnet)
    );
}

#[test]
fn regtest_donation_string_decodes_to_the_built_in_key() {
    let decoded = cashaddr::decode(
        bitcoin_cash_donation_cashaddr(BitcoinCashNetwork::Regtest),
        BitcoinCashNetwork::Regtest,
    )
    .unwrap()
    .to_payout_address();
    assert_eq!(
        decoded,
        bitcoin_cash_donation_address(BitcoinCashNetwork::Regtest)
    );
}

#[test]
fn bitcoin_cash_donates_one_percent_by_default() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.chain = Chain::BitcoinCash;
    config.bitcoin_cash_network = Some(BitcoinCashNetwork::Chipnet);
    config.bootstrap_address = CHIPNET_BOOTSTRAP.to_string();
    let parsed = config.parse().unwrap();
    assert_eq!(parsed.donation, Some(100));
    assert_eq!(
        parsed.donation_address(),
        Some(&bitcoin_cash_donation_address(BitcoinCashNetwork::Chipnet))
    );
}

#[test]
fn bitcoin_cash_mainnet_default_pays_the_published_address() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.chain = Chain::BitcoinCash;
    config.bitcoin_cash_network = Some(BitcoinCashNetwork::Mainnet);
    config.bootstrap_address = MAINNET_BOOTSTRAP.to_string();
    let parsed = config.parse().unwrap();
    let published = cashaddr::decode(BITCOIN_CASH_DONATION_ADDRESS, BitcoinCashNetwork::Mainnet)
        .unwrap()
        .to_payout_address();
    assert_eq!(parsed.donation, Some(DEFAULT_BITCOIN_CASH_DONATION));
    assert_eq!(parsed.donation_address(), Some(&published));
}

#[test]
fn bitcoin_cash_donation_can_be_raised() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.chain = Chain::BitcoinCash;
    config.bitcoin_cash_network = Some(BitcoinCashNetwork::Chipnet);
    config.bootstrap_address = CHIPNET_BOOTSTRAP.to_string();
    config.donation = Some(300);
    let parsed = config.parse().unwrap();
    assert_eq!(parsed.donation, Some(300));
    assert!(parsed.donation_address().is_some());
}

#[test]
fn bitcoin_cash_donation_can_be_lowered() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.chain = Chain::BitcoinCash;
    config.bitcoin_cash_network = Some(BitcoinCashNetwork::Chipnet);
    config.bootstrap_address = CHIPNET_BOOTSTRAP.to_string();
    config.donation = Some(50);
    let parsed = config.parse().unwrap();
    assert_eq!(parsed.donation, Some(50));
    assert!(parsed.donation_address().is_some());
}

#[test]
fn bitcoin_cash_donation_of_zero_opts_out() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.chain = Chain::BitcoinCash;
    config.bitcoin_cash_network = Some(BitcoinCashNetwork::Chipnet);
    config.bootstrap_address = CHIPNET_BOOTSTRAP.to_string();
    config.donation = Some(0);
    let parsed = config.parse().unwrap();
    assert_eq!(parsed.donation, None);
    assert!(parsed.donation_address().is_none());
}

#[test]
fn bitcoin_cash_accepts_the_built_in_donation_address_written_out() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.chain = Chain::BitcoinCash;
    config.bitcoin_cash_network = Some(BitcoinCashNetwork::Chipnet);
    config.bootstrap_address = CHIPNET_BOOTSTRAP.to_string();
    config.donation_address =
        Some(bitcoin_cash_donation_cashaddr(BitcoinCashNetwork::Chipnet).to_string());
    let parsed = config.parse().unwrap();
    assert_eq!(parsed.donation, Some(DEFAULT_BITCOIN_CASH_DONATION));
    assert_eq!(
        parsed.donation_address(),
        Some(&bitcoin_cash_donation_address(BitcoinCashNetwork::Chipnet))
    );
}

#[test]
fn bitcoin_cash_rejects_another_donation_address() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.chain = Chain::BitcoinCash;
    config.bitcoin_cash_network = Some(BitcoinCashNetwork::Chipnet);
    config.bootstrap_address = CHIPNET_BOOTSTRAP.to_string();
    config.donation_address = Some(CHIPNET_BOOTSTRAP.to_string());
    config.donation = Some(150);
    let error = config.parse().unwrap_err();
    assert!(
        error
            .message
            .contains("donation_address on bitcoin-cash is fixed"),
        "unexpected message: {}",
        error.message
    );
}

#[test]
fn bitcoin_cash_pays_an_operator_fee_beside_the_donation() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.chain = Chain::BitcoinCash;
    config.bitcoin_cash_network = Some(BitcoinCashNetwork::Chipnet);
    config.bootstrap_address = CHIPNET_BOOTSTRAP.to_string();
    config.fee_address = Some(CHIPNET_BOOTSTRAP.to_string());
    config.fee = Some(100);
    let parsed = config.parse().unwrap();
    assert_eq!(parsed.donation, Some(DEFAULT_BITCOIN_CASH_DONATION));
    assert_eq!(parsed.fee, Some(100));
    assert!(parsed.fee_address().is_some());
}

/// A Bitcoin Cash fee_address without fee takes the default 1% fee.
#[test]
fn bitcoin_cash_fee_address_alone_takes_the_default_fee() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.chain = Chain::BitcoinCash;
    config.bitcoin_cash_network = Some(BitcoinCashNetwork::Chipnet);
    config.bootstrap_address = CHIPNET_BOOTSTRAP.to_string();
    config.fee_address = Some(CHIPNET_BOOTSTRAP.to_string());
    let parsed = config.parse().unwrap();
    assert_eq!(DEFAULT_BITCOIN_CASH_FEE, 100);
    assert_eq!(parsed.fee, Some(DEFAULT_BITCOIN_CASH_FEE));
    assert!(parsed.fee_address().is_some());
}

/// A Bitcoin Cash fee without fee_address has nowhere to go.
#[test]
fn bitcoin_cash_rejects_a_fee_without_an_address() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.chain = Chain::BitcoinCash;
    config.bitcoin_cash_network = Some(BitcoinCashNetwork::Chipnet);
    config.bootstrap_address = CHIPNET_BOOTSTRAP.to_string();
    config.fee = Some(100);
    let error = config.parse().unwrap_err();
    assert!(
        error.message.contains("fee_address and fee"),
        "{}",
        error.message
    );
}

fn bitcoin_cash_config_with_cuts(donation: u16, fee: u16) -> StratumConfig<Raw> {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.chain = Chain::BitcoinCash;
    config.bitcoin_cash_network = Some(BitcoinCashNetwork::Chipnet);
    config.bootstrap_address = CHIPNET_BOOTSTRAP.to_string();
    config.donation = Some(donation);
    config.fee_address = Some(CHIPNET_BOOTSTRAP.to_string());
    config.fee = Some(fee);
    config
}

/// The donation and the fee each have their own 10% cap, with no
/// combined one: 10% and 10% together are accepted.
#[test]
fn bitcoin_cash_accepts_each_cut_at_its_own_cap() {
    let parsed = bitcoin_cash_config_with_cuts(MAX_BITCOIN_CASH_CUT, MAX_BITCOIN_CASH_CUT)
        .parse()
        .unwrap();
    assert_eq!(parsed.donation, Some(MAX_BITCOIN_CASH_CUT));
    assert_eq!(parsed.fee, Some(MAX_BITCOIN_CASH_CUT));
}

/// Without `payout_level` and `spend_cost` a node gives miners level 5
/// and prices a payment's spend at 148 satoshis.
#[test]
fn an_unset_payout_level_and_spend_cost_take_their_defaults() {
    let parsed = bitcoin_cash_config_with_cuts(0, 0).parse().unwrap();

    assert_eq!(parsed.payout_level(), DEFAULT_PAYOUT_LEVEL);
    assert_eq!(parsed.payout_level, Some(5));
    assert_eq!(parsed.spend_cost(), 148);
}

#[test]
fn the_highest_payout_level_is_accepted() {
    let mut config = bitcoin_cash_config_with_cuts(0, 0);
    config.payout_level = Some(MAX_PAYOUT_LEVEL);

    assert_eq!(config.parse().unwrap().payout_level(), 7);
}

#[test]
fn a_payout_level_above_seven_is_refused() {
    let mut config = bitcoin_cash_config_with_cuts(0, 0);
    config.payout_level = Some(8);

    let error = config.parse().unwrap_err();
    assert!(error.message.contains("payout_level 8"), "{error:?}");
}

/// `[stratum] payout_level` of 0 is refused, in the config file and as
/// the stratum option, naming the level, the lowest level and its amount.
#[test]
fn a_payout_level_of_zero_is_refused() {
    let mut config = bitcoin_cash_config_with_cuts(0, 0);
    config.payout_level = Some(0);

    let error = config.parse().unwrap_err();
    assert_eq!(
        error.message,
        payout_level_below_minimum(0),
        "config payout_level 0"
    );
}

#[test]
fn bitcoin_cash_rejects_a_donation_above_its_cap() {
    let error = bitcoin_cash_config_with_cuts(1_100, 0).parse().unwrap_err();
    assert!(
        error
            .message
            .contains("must each be at most 1000 basis points"),
        "unexpected message: {}",
        error.message
    );
}

#[test]
fn bitcoin_cash_rejects_a_fee_above_its_cap() {
    let error = bitcoin_cash_config_with_cuts(DEFAULT_BITCOIN_CASH_DONATION, 1_001)
        .parse()
        .unwrap_err();
    assert!(
        error
            .message
            .contains("must each be at most 1000 basis points"),
        "unexpected message: {}",
        error.message
    );
}

#[test]
fn bitcoin_cash_fee_may_use_its_whole_cap_when_the_donation_is_off() {
    let parsed = bitcoin_cash_config_with_cuts(0, MAX_BITCOIN_CASH_CUT)
        .parse()
        .unwrap();
    assert_eq!(parsed.donation, None);
    assert_eq!(parsed.fee, Some(MAX_BITCOIN_CASH_CUT));
}

#[test]
fn bitcoin_has_no_default_donation() {
    let parsed = StratumConfig::<Raw>::new_for_test_default()
        .parse()
        .unwrap();
    assert_eq!(parsed.donation, None);
    assert!(parsed.donation_address().is_none());
}

#[test]
fn bitcoin_rejects_a_donation_above_one_hundred_percent() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.donation_address = Some("tb1qyazxde6558qj6z3d9np5e6msmrspwpf6k0qggk".to_string());
    config.donation = Some(MAX_BASIS_POINTS + 1);
    let error = config.parse().unwrap_err();
    assert!(
        error.message.contains("must each be at most 10000"),
        "unexpected message: {}",
        error.message
    );
}

#[test]
fn bitcoin_rejects_a_fee_above_one_hundred_percent() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.fee_address = Some("tb1qyazxde6558qj6z3d9np5e6msmrspwpf6k0qggk".to_string());
    config.fee = Some(MAX_BASIS_POINTS + 1);
    let error = config.parse().unwrap_err();
    assert!(
        error.message.contains("must each be at most 10000"),
        "unexpected message: {}",
        error.message
    );
}

#[test]
fn bitcoin_still_accepts_a_full_donation() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.donation_address = Some("tb1qyazxde6558qj6z3d9np5e6msmrspwpf6k0qggk".to_string());
    config.donation = Some(MAX_BASIS_POINTS);
    assert_eq!(config.parse().unwrap().donation, Some(MAX_BASIS_POINTS));
}
