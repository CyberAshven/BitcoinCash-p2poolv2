// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;

#[test]
fn the_study_runs_the_design_as_the_page_now_has_it() {
    let scenario = study_scenario(&Scenario::default());
    assert_eq!(scenario.design, Design::BandWeakLevels);
    assert_eq!(scenario.weak_credit, WeakCredit::WeakWork);
    assert_eq!(scenario.includer_cut, 0.01);
}

#[test]
fn a_signed_percentage_shows_its_sign() {
    assert_eq!(signed_percent(0.0123), "+1.23%");
    assert_eq!(signed_percent(-0.0035), "-0.35%");
}

#[test]
fn signed_satoshis_are_grouped_with_their_sign() {
    assert_eq!(signed_satoshis(-5_305.9), "-5,306");
    assert_eq!(signed_satoshis(1_308.2), "+1,308");
}

#[test]
fn a_measure_reports_the_first_hour_it_reaches_its_level() {
    assert_eq!(settle_cell(&[1.0, 2.0], 1.0), "< 1 h");
    assert_eq!(settle_cell(&[2.0, 1.0], 1.0), "1 h");
    assert_eq!(settle_cell(&[2.0, 2.0], 1.0), "not reached");
}

#[test]
fn the_coalition_gain_compares_refusing_classes_with_the_same_classes_carrying() {
    let base = Scenario {
        weak_credit: WeakCredit::WeakWork,
        share_races: Some(ShareRaces::without_losses(ForkChoice::WeakWork)),
        tracked: tracked_with_non_carrying_asic(),
        months: 2,
        weak_queue_shares: 50_000,
        weak_depth: 6,
        ..Scenario::default()
    };
    let all_carry = base.run();
    let refusing = Scenario {
        non_carrying_fraction: 0.5,
        ..base.clone()
    }
    .run();
    assert_eq!(coalition_gain(&all_carry, &all_carry), None);
    let gain = coalition_gain(&refusing, &all_carry).expect("half the pool refuses");
    // At the first depth, 6, refusing lets about 5% of honest weak shares
    // expire, and the rest of the pool shares their part of the blocks:
    // about 1% for the coalition.
    assert!(gain > 0.0 && gain < 0.03, "{gain}");

    // At the rule's depth, 48, almost none expire and the coalition loses its
    // includer cuts.
    let deep = Scenario {
        weak_depth: crate::constants::WEAK_DEPTH,
        ..base
    };
    let deep_refusing = Scenario {
        non_carrying_fraction: 0.5,
        ..deep.clone()
    }
    .run();
    let deep_gain = coalition_gain(&deep_refusing, &deep.run()).expect("half the pool refuses");
    assert!(deep_gain < 0.0, "{deep_gain}");
}

#[test]
fn a_tiny_pool_with_every_miner_at_the_minimum_pays_as_its_one_chain_row() {
    let table = tiny_pool(&Scenario {
        months: 20,
        weak_queue_shares: 20_000,
        ..Scenario::default()
    });
    let rows: Vec<&str> = table.lines().collect();
    // Header, rule, then the rows in order, one-chain first.
    assert!(rows[2].starts_with("| one-chain, node default"));
    assert!(rows[6].starts_with("| band, all at the minimum"));
    assert!(rows[6].ends_with("| one-chain, node default |"));
    assert!(rows[7].ends_with("| one-chain, node default |"));
}
