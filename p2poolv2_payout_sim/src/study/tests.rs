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

/// The class `name` names, so a missing class fails before the level check.
fn class_named<'a>(classes: &'a [MinerClass], name: &str) -> &'a MinerClass {
    classes.iter().find(|class| class.name == name).expect(name)
}

#[test]
fn with_node_levels_each_class_has_its_own_level() {
    let scenario = scale_scenario(&Scenario::default(), 100.0, 0.9);
    // `pool_classes` asks `node_level` with this scenario's network hashrate
    // and a month of blocks times the pool fraction: the same two figures
    // `scale_scenario` uses for a multiple of 100 and a fraction of 0.9.
    let network = scenario.network_hashrate();
    let pool_blocks = BCH_BLOCKS_PER_MONTH * scenario.pool_fraction;
    let home_level = node_level(HOME_MINER_HASHRATE, network, pool_blocks);
    let classes = scenario.pool_classes();
    let home = class_named(&classes, "home 1.2 TH/s");
    assert_eq!(
        home.level, home_level,
        "class home 1.2 TH/s expected level {home_level}"
    );
    assert!(
        home.level < 5,
        "class home 1.2 TH/s level {} is below 5",
        home.level
    );
    let small_farm = class_named(&classes, "farm 1 PH/s");
    assert_eq!(small_farm.level, 5, "class farm 1 PH/s expected level 5");
    let large_farm = class_named(&classes, "farm 10 PH/s");
    assert_eq!(large_farm.level, 5, "class farm 10 PH/s expected level 5");
}

#[test]
fn without_node_levels_every_class_is_at_the_default() {
    let mut scenario = scale_scenario(&Scenario::default(), 100.0, 0.9);
    scenario.node_levels = false;
    scenario.default_level = 3;
    // A tracked miner's level is the level stored on the miner. `pool_classes`
    // copies it when `node_levels` is off, so the tracked miners are set to
    // the same default as the mix before the classes are built.
    let default_level = scenario.default_level;
    scenario
        .tracked
        .iter_mut()
        .for_each(|miner| miner.level = default_level);
    let classes = scenario.pool_classes();
    [
        "home 1.2 TH/s",
        "farm 1 PH/s",
        "farm 10 PH/s",
        "Bitaxe 1.2 TH/s",
        "ASIC 100 TH/s",
    ]
    .into_iter()
    .for_each(|name| {
        let level = class_named(&classes, name).level;
        assert_eq!(
            level, default_level,
            "class {name} expected level {default_level}"
        );
    });
    assert_eq!(classes.len(), 5, "every class of the half-industrial pool");
}

#[test]
fn a_shorter_period_lever_takes_the_stale_rate_of_its_period() {
    let today = scale_scenario(&Scenario::default(), 10.0, 0.3);
    let scenario = lever_scenario(&Scenario::default(), 10.0, 0.3, ScaleLever::Period(2.0));
    assert_eq!(scenario.period_seconds, 2.0);
    assert_eq!(scenario.stale_rate, stale_rate_for_period(2.0));
    assert!(scenario.stale_rate > today.stale_rate);
    assert_eq!(scenario.weak_cap, today.weak_cap);
    assert_eq!(scenario.weak_fill, today.weak_fill);
    assert_eq!(scenario.weak_ratio_max, today.weak_ratio_max);
}

#[test]
fn a_weak_shares_lever_keeps_the_period_and_raises_cap_fill_and_ratio_maximum_together() {
    let today = scale_scenario(&Scenario::default(), 10.0, 0.3);
    let scenario = lever_scenario(&Scenario::default(), 10.0, 0.3, ScaleLever::WeakShares(5));
    assert_eq!(scenario.period_seconds, today.period_seconds);
    assert_eq!(scenario.stale_rate, today.stale_rate);
    assert_eq!(scenario.weak_cap, 5 * today.weak_cap);
    assert_eq!(scenario.weak_fill, 5 * today.weak_fill);
    assert_eq!(scenario.weak_ratio_max, 5 * today.weak_ratio_max);
}

/// The study's finding at the model's own level: five times the weak shares
/// a share give the Bitaxe the weak shares a share period a fifth as long
/// gives it, without that period's stale shares.
#[test]
fn more_weak_shares_a_share_credit_a_home_miner_as_a_shorter_period_does() {
    let base = Scenario {
        months: 1,
        ..Scenario::default()
    };
    let reports = run_scenarios(&[
        lever_scenario(&base, 10.0, 0.3, ScaleLever::Period(10.0)),
        lever_scenario(&base, 10.0, 0.3, ScaleLever::Period(2.0)),
        lever_scenario(&base, 10.0, 0.3, ScaleLever::WeakShares(5)),
    ]);
    let weak_per_month = |report: &Report| report.miners[0].weak_shares_per_month;
    let today = weak_per_month(&reports[0]);
    let shorter = weak_per_month(&reports[1]);
    let more = weak_per_month(&reports[2]);
    assert!(
        shorter > 4.0 * today,
        "a period a fifth as long gives {shorter} weak shares a month against {today}"
    );
    assert!(
        (more / shorter - 1.0).abs() < 0.1,
        "five times the weak shares a share give {more} a month, the shorter period {shorter}"
    );
    assert!(reports[2].scenario.stale_rate < reports[1].scenario.stale_rate);
}
