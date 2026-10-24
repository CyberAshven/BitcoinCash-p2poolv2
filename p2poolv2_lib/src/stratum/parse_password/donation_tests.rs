// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::{
    donation_percent, join_login_options, parse_miner_donation, parse_password,
    resolve_miner_donation, split_user_identity,
};
use crate::config::{Chain, PoolMode};

fn donation(options: &str) -> Result<Option<u16>, String> {
    parse_miner_donation(Some(options))
}

#[test]
fn no_donation_option_leaves_the_node_default() {
    assert_eq!(parse_miner_donation(None), Ok(None));
    assert_eq!(donation("x"), Ok(None));
    assert_eq!(donation("d=1000,p2p=tp2pool:qq"), Ok(None));
}

#[test]
fn whole_and_half_percent_steps_are_basis_points() {
    assert_eq!(donation("donation=0"), Ok(Some(0)));
    assert_eq!(donation("donation=0.5"), Ok(Some(50)));
    assert_eq!(donation("donation=1"), Ok(Some(100)));
    assert_eq!(donation("donation=1.5"), Ok(Some(150)));
    assert_eq!(donation("donation=2.50"), Ok(Some(250)));
    assert_eq!(donation("donation=9.5"), Ok(Some(950)));
}

/// The donation's own 10% cap is the most a miner may choose, whatever the
/// operator's fee is: the two caps are independent.
#[test]
fn ten_percent_is_the_most_a_miner_may_choose() {
    assert_eq!(donation("donation=10"), Ok(Some(1_000)));
    assert_eq!(donation("donation=10.0"), Ok(Some(1_000)));
    for above in ["10.5", "11", "100", "999999"] {
        let error = donation(&format!("donation={above}")).unwrap_err();
        assert!(error.contains("10% cap"), "{above}: {error}");
    }
}

#[test]
fn values_off_the_half_percent_steps_are_refused() {
    for value in ["0.25", "0.1", "1.05", "1.75"] {
        let error = donation(&format!("donation={value}")).unwrap_err();
        assert!(error.contains("steps of 0.5"), "{value}: {error}");
    }
}

#[test]
fn malformed_values_are_refused_not_defaulted() {
    for options in [
        "donation=",
        "donation",
        "donation=-1",
        "donation=+1",
        "donation=1%",
        "donation=.5",
        "donation=1.",
        "donation=1.5.0",
        "donation=one",
        "donation=0x1",
        "donation=1.500",
    ] {
        let error = donation(options).unwrap_err();
        assert!(error.contains("donation="), "{options}: {error}");
    }
}

#[test]
fn the_key_is_case_insensitive() {
    assert_eq!(donation("DONATION=0.5"), Ok(Some(50)));
    assert_eq!(donation("Donation=2"), Ok(Some(200)));
}

#[test]
fn the_same_choice_twice_is_accepted_and_two_choices_are_refused() {
    assert_eq!(donation("donation=1,donation=1.0"), Ok(Some(100)));
    let error = donation("donation=1,donation=0").unwrap_err();
    assert!(error.contains("twice"), "{error}");
}

/// Another pool's keys, such as Pickaxe's own donation, are ignored, so one
/// login works at both kinds of pool.
#[test]
fn unknown_keys_such_as_pickaxe_donation_are_ignored() {
    assert_eq!(donation("pickaxe-donation=0,donation=0.5"), Ok(Some(50)));
    assert_eq!(donation("donation=0.5,pickaxe-donation=0"), Ok(Some(50)));
    assert_eq!(donation("pickaxe-donation=7"), Ok(None));
    assert_eq!(donation("pooldonation=0,mydonation=bad"), Ok(None));
}

#[test]
fn every_separator_of_the_login_options_works() {
    assert_eq!(donation("d=1000;donation=0.5"), Ok(Some(50)));
    assert_eq!(donation("d=1000 donation=0.5"), Ok(Some(50)));
    assert_eq!(donation("d=1000\tdonation=0.5"), Ok(Some(50)));
    let parsed = parse_password("d=1000,donation=0.5,th=10");
    assert_eq!(parsed.difficulty, Some(1000));
}

#[test]
fn a_bitcoin_cash_p2pool_node_honours_the_choice() {
    assert_eq!(
        resolve_miner_donation(Some("donation=0"), Chain::BitcoinCash, PoolMode::P2poolv2),
        Ok(Some(0))
    );
    assert_eq!(
        resolve_miner_donation(None, Chain::BitcoinCash, PoolMode::P2poolv2),
        Ok(None)
    );
}

/// A node that builds no Bitcoin Cash share chain cannot honour a miner's
/// donation, so it says so instead of mining with its own.
#[test]
fn nodes_without_a_bitcoin_cash_share_chain_refuse_the_option() {
    for (chain, mode) in [
        (Chain::BitcoinCash, PoolMode::Hydrapool),
        (Chain::Bitcoin, PoolMode::P2poolv2),
        (Chain::Bitcoin, PoolMode::Hydrapool),
    ] {
        let error = resolve_miner_donation(Some("donation=0"), chain, mode).unwrap_err();
        assert!(error.contains("cannot honour"), "{error}");
        assert_eq!(
            resolve_miner_donation(Some("pickaxe-donation=0"), chain, mode),
            Ok(None)
        );
    }
}

#[test]
fn an_identity_splits_at_its_first_option() {
    assert_eq!(
        split_user_identity("bitcoincash:qq.rig1,pickaxe-donation=0,donation=0.5"),
        (
            "bitcoincash:qq.rig1",
            Some("pickaxe-donation=0,donation=0.5")
        )
    );
    assert_eq!(
        split_user_identity("bitcoincash:qq.rig1"),
        ("bitcoincash:qq.rig1", None)
    );
}

#[test]
fn username_options_come_before_the_password() {
    assert_eq!(join_login_options(None, None), None);
    assert_eq!(
        join_login_options(Some("donation=0"), None).as_deref(),
        Some("donation=0")
    );
    assert_eq!(join_login_options(None, Some("x")).as_deref(), Some("x"));
    assert_eq!(
        join_login_options(Some("donation=0"), Some("d=500")).as_deref(),
        Some("donation=0,d=500")
    );
}

#[test]
fn percentages_print_as_miners_write_them() {
    assert_eq!(donation_percent(0), "0%");
    assert_eq!(donation_percent(50), "0.5%");
    assert_eq!(donation_percent(100), "1%");
    assert_eq!(donation_percent(1_000), "10%");
    assert_eq!(donation_percent(125), "1.25%");
}
