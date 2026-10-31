// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;

#[test]
fn one_miner_is_one_payee() {
    let payees = expected_payees(1, HashrateSpread::Equal, 120_960);
    assert!((payees - 1.0).abs() < 1e-9, "payees {payees}");
}

#[test]
fn a_hundred_equal_miners_are_all_paid() {
    let payees = expected_payees(100, HashrateSpread::Equal, 120_960);
    assert!((payees - 100.0).abs() < 1e-6, "payees {payees}");
}

#[test]
fn a_million_equal_miners_fill_about_114_thousand_payee_slots() {
    // 1e6 * (1 - exp(120,960 * ln(1 - 1e-6))) = 113,931.
    let payees = expected_payees(1_000_000, HashrateSpread::Equal, 120_960);
    assert!((payees - 113_931.0).abs() < 10.0, "payees {payees}");
}

#[test]
fn payees_never_exceed_the_window() {
    let payees = expected_payees(10_000_000, HashrateSpread::Equal, 120_960);
    assert!(payees < 120_960.0, "payees {payees}");
}

#[test]
fn no_miners_means_no_payees() {
    assert_eq!(expected_payees(0, HashrateSpread::Equal, 120_960), 0.0);
}

#[test]
fn skewed_hashrate_pays_fewer_miners_than_equal_hashrate() {
    let equal = expected_payees(100_000, HashrateSpread::Equal, 120_960);
    let skewed = expected_payees(100_000, HashrateSpread::Pareto { shape: 1.16 }, 120_960);
    assert!(skewed < equal, "skewed {skewed} equal {equal}");
}

#[test]
fn pareto_fractions_sum_to_one_and_grow_with_the_quantile() {
    let fractions = hashrate_fractions(1_000, HashrateSpread::Pareto { shape: 1.16 });
    let total: f64 = fractions.iter().sum();
    assert!((total - 1.0).abs() < 1e-9, "total {total}");
    assert!(fractions[999] > fractions[0]);
}

#[test]
fn a_millionth_of_the_pool_waits_116_days_for_a_share() {
    let seconds = seconds_between_shares(1e-6, 10.0);
    assert!((seconds - 10_000_000.0).abs() < 1e-3);
    assert!((seconds / 86_400.0 - 115.74).abs() < 0.01);
}

#[test]
fn a_pool_with_a_tenth_of_bch_has_an_1800_share_window() {
    // payout-distribution.md: "10% | about 1,800 shares".
    assert_eq!(pplns_window_shares(0.1, 10.0), 1_800);
}

#[test]
fn a_pool_with_one_percent_of_bch_has_an_18000_share_window() {
    assert_eq!(pplns_window_shares(0.01, 10.0), 18_000);
}

#[test]
fn a_pool_under_about_0_15_percent_of_bch_reaches_the_cap() {
    assert_eq!(pplns_window_shares(0.001, 10.0), 120_960);
    assert_eq!(pplns_window_shares(0.0015, 10.0), 120_000);
}

#[test]
fn a_pool_of_unknown_size_gets_the_full_window() {
    assert_eq!(pplns_window_shares(0.0, 10.0), 120_960);
}

#[test]
fn slower_shares_shorten_the_window_in_shares() {
    assert_eq!(pplns_window_shares(0.1, 30.0), 600);
}

#[test]
fn hashrate_spreads_parse_from_the_command_line_forms() {
    assert_eq!(HashrateSpread::parse("equal"), Ok(HashrateSpread::Equal));
    assert_eq!(
        HashrateSpread::parse("pareto:1.16"),
        Ok(HashrateSpread::Pareto { shape: 1.16 })
    );
}

#[test]
fn a_pareto_shape_of_one_or_less_is_refused() {
    assert!(HashrateSpread::parse("pareto:1").is_err());
    assert!(HashrateSpread::parse("zipf").is_err());
}
