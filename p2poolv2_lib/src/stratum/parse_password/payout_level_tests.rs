// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::{parse_miner_payout_level, resolve_miner_payout_level};
use crate::config::{Chain, PoolMode};
use crate::stratum::work::level_policy::PayoutLevelChoice;

fn payout_level(options: &str) -> Result<Option<PayoutLevelChoice>, String> {
    parse_miner_payout_level(Some(options))
}

#[test]
fn no_options_leave_the_node_level() {
    assert_eq!(parse_miner_payout_level(None), Ok(None));
}

#[test]
fn options_without_a_level_leave_the_node_level() {
    assert_eq!(payout_level("d=1000,p2p=tp2pool:qq,donation=1"), Ok(None));
}

#[test]
fn min_payout_is_read_in_satoshis() {
    assert_eq!(
        payout_level("min-payout=10000"),
        Ok(Some(PayoutLevelChoice::MinPayout(10_000)))
    );
}

#[test]
fn min_payout_of_zero_is_accepted() {
    assert_eq!(
        payout_level("min-payout=0"),
        Ok(Some(PayoutLevelChoice::MinPayout(0)))
    );
}

#[test]
fn a_min_payout_above_the_highest_level_is_accepted() {
    assert_eq!(
        payout_level("min-payout=1000000"),
        Ok(Some(PayoutLevelChoice::MinPayout(1_000_000)))
    );
}

#[test]
fn fee_tolerance_is_read_in_basis_points() {
    assert_eq!(
        payout_level("fee-tolerance=0.85"),
        Ok(Some(PayoutLevelChoice::FeeTolerance(85)))
    );
}

#[test]
fn a_whole_fee_tolerance_is_read_in_basis_points() {
    assert_eq!(
        payout_level("fee-tolerance=1"),
        Ok(Some(PayoutLevelChoice::FeeTolerance(100)))
    );
}

#[test]
fn a_fee_tolerance_of_one_hundred_percent_is_accepted() {
    assert_eq!(
        payout_level("fee-tolerance=100"),
        Ok(Some(PayoutLevelChoice::FeeTolerance(10_000)))
    );
}

#[test]
fn min_payout_wins_over_fee_tolerance() {
    assert_eq!(
        payout_level("fee-tolerance=1,min-payout=2000"),
        Ok(Some(PayoutLevelChoice::MinPayout(2_000)))
    );
}

#[test]
fn the_keys_are_case_insensitive() {
    assert_eq!(
        payout_level("MIN-PAYOUT=5000"),
        Ok(Some(PayoutLevelChoice::MinPayout(5_000)))
    );
}

#[test]
fn the_same_min_payout_twice_is_accepted() {
    assert_eq!(
        payout_level("min-payout=5000,min-payout=5000"),
        Ok(Some(PayoutLevelChoice::MinPayout(5_000)))
    );
}

#[test]
fn two_different_min_payouts_are_refused() {
    let error = payout_level("min-payout=5000,min-payout=6000").unwrap_err();

    assert!(error.contains("given twice"), "{error}");
}

#[test]
fn two_different_fee_tolerances_are_refused() {
    let error = payout_level("fee-tolerance=1,fee-tolerance=2").unwrap_err();

    assert!(error.contains("given twice"), "{error}");
}

#[test]
fn an_empty_min_payout_is_refused() {
    let error = payout_level("min-payout=").unwrap_err();

    assert!(error.contains("min-payout="), "{error}");
}

#[test]
fn a_negative_min_payout_is_refused() {
    let error = payout_level("min-payout=-1").unwrap_err();

    assert!(error.contains("whole number of satoshis"), "{error}");
}

#[test]
fn a_fractional_min_payout_is_refused() {
    let error = payout_level("min-payout=1000.5").unwrap_err();

    assert!(error.contains("whole number of satoshis"), "{error}");
}

#[test]
fn a_min_payout_in_bch_is_refused() {
    let error = payout_level("min-payout=0.0001BCH").unwrap_err();

    assert!(error.contains("min-payout=0.0001BCH"), "{error}");
}

#[test]
fn a_min_payout_past_a_64_bit_amount_is_refused() {
    let error = payout_level("min-payout=99999999999999999999").unwrap_err();

    assert!(error.contains("whole number of satoshis"), "{error}");
}

#[test]
fn a_fee_tolerance_above_one_hundred_percent_is_refused() {
    let error = payout_level("fee-tolerance=100.5").unwrap_err();

    assert!(error.contains("above 100%"), "{error}");
}

#[test]
fn a_fee_tolerance_with_a_percent_sign_is_refused() {
    let error = payout_level("fee-tolerance=1%").unwrap_err();

    assert!(error.contains("without a % sign"), "{error}");
}

#[test]
fn a_fee_tolerance_with_three_decimals_is_refused() {
    let error = payout_level("fee-tolerance=0.125").unwrap_err();

    assert!(error.contains("fee-tolerance=0.125"), "{error}");
}

#[test]
fn an_empty_fee_tolerance_is_refused() {
    let error = payout_level("fee-tolerance=").unwrap_err();

    assert!(error.contains("fee-tolerance="), "{error}");
}

#[test]
fn a_bitcoin_cash_p2pool_node_honours_a_level() {
    assert_eq!(
        resolve_miner_payout_level(
            Some("min-payout=10000"),
            Chain::BitcoinCash,
            PoolMode::P2poolv2
        ),
        Ok(Some(PayoutLevelChoice::MinPayout(10_000)))
    );
}

#[test]
fn a_bitcoin_node_refuses_a_level() {
    let error =
        resolve_miner_payout_level(Some("min-payout=10000"), Chain::Bitcoin, PoolMode::P2poolv2)
            .unwrap_err();

    assert!(error.contains("Bitcoin Cash P2Pool node"), "{error}");
}

#[test]
fn a_hydrapool_node_refuses_a_level() {
    let error = resolve_miner_payout_level(
        Some("fee-tolerance=1"),
        Chain::BitcoinCash,
        PoolMode::Hydrapool,
    )
    .unwrap_err();

    assert!(error.contains("Bitcoin Cash P2Pool node"), "{error}");
}

#[test]
fn a_bitcoin_node_without_a_level_is_fine() {
    assert_eq!(
        resolve_miner_payout_level(Some("d=1000"), Chain::Bitcoin, PoolMode::P2poolv2),
        Ok(None)
    );
}
