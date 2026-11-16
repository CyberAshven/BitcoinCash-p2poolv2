// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;
use crate::constants::WEAK_FILL;

#[test]
fn the_ceiling_rule_runs_today_s_rule_at_the_proposal_s_fill_maximum() {
    let scenario = ruled_scenario(&Scenario::default(), 10.0, 0.3, WeakRule::AtTheCeiling);
    assert_eq!(scenario.weak_fill, WEAK_FILL_MAX);
    assert_eq!(scenario.weak_cap, 2 * WEAK_FILL_MAX);
    assert!(scenario.weak_asks.is_none());
}

#[test]
fn an_asking_scenario_keeps_today_s_fill_and_adds_the_asks() {
    let asks = WeakAsks::default();
    let scenario = ruled_scenario(&Scenario::default(), 10.0, 0.3, WeakRule::Asks(asks));
    assert_eq!(scenario.weak_fill, WEAK_FILL);
    assert_eq!(scenario.weak_asks, Some(asks));
}

/// The proposal's purpose at the model's own level: the asks leave a pool
/// whose small miners have enough weak shares near today's count, and open
/// one whose small miners do not.
#[test]
fn the_asks_open_only_a_pool_whose_small_miners_need_more_weak_shares() {
    let base = Scenario {
        months: 1,
        ..Scenario::default()
    };
    let asking = WeakRule::Asks(WeakAsks::default());
    let reports = run_scenarios(&[
        ruled_scenario(&base, 1.0, 0.1, asking),
        ruled_scenario(&base, 10.0, 0.3, WeakRule::Today),
        ruled_scenario(&base, 10.0, 0.3, asking),
    ]);
    let carried = |report: &Report| {
        report
            .home_chain()
            .weak
            .unwrap_or_default()
            .carried_per_share
    };
    assert!(
        carried(&reports[0]) < 2.0 * WEAK_FILL as f64,
        "a pool that needs no more carries {} weak shares a share",
        carried(&reports[0])
    );
    assert!(
        carried(&reports[2]) > 5.0 * carried(&reports[1]),
        "a pool that needs more carries {} with asks, {} without",
        carried(&reports[2]),
        carried(&reports[1])
    );
    assert!(
        reports[2].miners[0].weak_shares_per_month
            > 5.0 * reports[1].miners[0].weak_shares_per_month
    );
}
