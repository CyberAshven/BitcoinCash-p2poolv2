// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;

#[test]
fn every_limits_scenario_validates_one_share_at_a_time_on_a_thousand_nodes() {
    let base = Scenario::default();
    Limit::ALL.iter().for_each(|limit| {
        let scenarios = limit.scenarios(&base);
        assert!(!scenarios.is_empty(), "{}", limit.name());
        scenarios.iter().for_each(|scenario| {
            assert_eq!(scenario.node_count, LIMITS_NODE_COUNT, "{}", scenario.label);
            assert!(
                matches!(scenario.validation_queue, ValidationQueue::Serial { .. }),
                "{}",
                scenario.label
            );
            assert_eq!(scenario.validation_seconds_per_megabyte, 0.0);
            assert!(scenario.validate().is_ok(), "{}", scenario.label);
        });
    });
}

#[test]
fn a_scenario_takes_the_measured_cost_for_its_payees() {
    let scenario = measured(&Scenario::default(), 10_000, 1.0, 10.0);

    assert_eq!(scenario.validation_seconds, 0.0383);
    assert_eq!(
        scenario.validation_queue,
        ValidationQueue::Serial {
            weak_share_seconds: 0.0040
        }
    );
    assert_eq!(scenario.payees_override, Some(10_000));
}

#[test]
fn a_slow_node_pays_the_measured_cost_times_its_factor() {
    let scenario = measured(&Scenario::default(), 1_000, SLOW_NODE_FACTOR, 2.0);

    assert!((scenario.validation_seconds - 0.0066 * SLOW_NODE_FACTOR).abs() < 1e-12);
    assert_eq!(scenario.share_interval_seconds, 2.0);
}
