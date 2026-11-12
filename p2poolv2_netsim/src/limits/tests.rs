// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;

#[test]
fn every_limits_scenario_validates_one_share_at_a_time() {
    let base = Scenario::default();
    Limit::ALL.iter().for_each(|limit| {
        let scenarios = limit.scenarios(&base);
        assert!(!scenarios.is_empty(), "{}", limit.name());
        scenarios.iter().for_each(|scenario| {
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
fn only_the_network_sweep_leaves_a_thousand_nodes() {
    let base = Scenario::default();
    Limit::ALL
        .iter()
        .filter(|limit| **limit != Limit::Network)
        .flat_map(|limit| limit.scenarios(&base))
        .for_each(|scenario| {
            assert_eq!(scenario.node_count, LIMITS_NODE_COUNT, "{}", scenario.label)
        });
    let largest = Limit::Network
        .scenarios(&base)
        .iter()
        .map(|scenario| scenario.node_count)
        .max();
    assert_eq!(largest, Some(10_000));
}

#[test]
fn a_scenario_takes_the_measured_cost_of_its_miners() {
    let scenario = measured(&Scenario::default(), ADOPTED[1], 1.0, 10.0);

    assert_eq!(scenario.validation_seconds, 0.0379);
    assert_eq!(
        scenario.validation_queue,
        ValidationQueue::Serial {
            weak_share_seconds: 0.00375
        }
    );
    assert_eq!(scenario.payees_override, Some(10_000));
}

#[test]
fn a_hundred_thousand_miners_at_level_five_are_paid_by_17_883_outputs() {
    let scenario = measured(&Scenario::default(), ADOPTED[2], 1.0, 10.0);

    assert_eq!(scenario.payees_override, Some(17_883));
}

/// A token share's coinbase stops at 3,072 bytes whatever the payees, so it
/// travels whole in a few kilobytes where a share without tokens paying
/// 10,000 miners takes hundreds.
#[test]
fn a_token_share_stays_small_whatever_it_pays() {
    let small_network = |cost| Scenario {
        duration_seconds: 600.0,
        node_count: 20,
        ..measured(&Scenario::default(), cost, 1.0, 10.0)
    };
    let plain = small_network(ADOPTED[1]).run();
    let token = small_network(TOKEN_SHARES[1]).run();

    assert!(
        token.share_wire_bytes < 8_000,
        "{} bytes",
        token.share_wire_bytes
    );
    assert!(
        plain.share_wire_bytes > 300_000,
        "{} bytes",
        plain.share_wire_bytes
    );
}

#[test]
fn a_slow_node_pays_the_measured_cost_times_its_factor() {
    let scenario = measured(&Scenario::default(), ADOPTED[0], SLOW_NODE_FACTOR, 2.0);

    assert!((scenario.validation_seconds - 0.0064 * SLOW_NODE_FACTOR).abs() < 1e-12);
    assert_eq!(scenario.share_interval_seconds, 2.0);
}
