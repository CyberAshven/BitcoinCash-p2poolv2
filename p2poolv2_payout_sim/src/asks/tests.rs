// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;
use crate::weak::next_weak_ratio;

/// The proposed rule with a divisor of 64, whatever the constants are set to.
fn rule() -> AskRule {
    AskRule {
        weak_fill: 32,
        weak_fill_max: 320,
        ask_divisor: 64,
        ratio_max: 256,
        ratio_ask_max: 4_096,
    }
}

/// One class at the minimum finding `minimum_shares_per_period` of the
/// chain's shares.
fn queue(
    minimum_shares_per_period: f64,
    asks_below_ratio: f64,
    censoring_share_fraction: f64,
) -> AskQueueInputs {
    AskQueueInputs {
        classes: vec![AskClass {
            minimum_shares_per_period,
            asks_below_ratio,
        }],
        rule: rule(),
        weak_depth: 48,
        ceiling_ratio: 1.0e9,
        censoring_share_fraction,
        shares: 40_000,
        warmup_shares: 20_000,
    }
}

#[test]
fn without_asks_the_ratio_moves_as_it_does_today() {
    [1_024, 50_000, 102_400, 256 * 1_024]
        .into_iter()
        .for_each(|ratio| {
            [0, 31, 32, 33, 64, 640].into_iter().for_each(|carried| {
                assert_eq!(
                    next_weak_ratio_with_asks(ratio, carried, 0, &rule()),
                    next_weak_ratio(ratio, carried, 32, 256),
                    "ratio {ratio}, carried {carried}"
                );
            });
        });
}

#[test]
fn asked_the_ratio_rises_past_today_s_maximum() {
    let at_maximum = 256 * 1_024;
    assert_eq!(
        next_weak_ratio_with_asks(at_maximum, 40, 1, &rule()),
        at_maximum + 1_024
    );
}

#[test]
fn asked_the_ratio_stops_at_the_ask_maximum() {
    let at_ask_maximum = 4_096 * 1_024;
    assert_eq!(
        next_weak_ratio_with_asks(at_ask_maximum, 40, 40, &rule()),
        at_ask_maximum
    );
}

#[test]
fn asks_do_not_lift_the_ratio_once_a_share_carries_more_than_the_ceiling() {
    assert_eq!(
        next_weak_ratio_with_asks(102_400, 321, 321, &rule()),
        102_400 - 400
    );
}

#[test]
fn asks_of_one_weak_share_in_the_divisor_or_fewer_do_not_count() {
    assert_eq!(
        next_weak_ratio_with_asks(102_400, 128, 2, &rule()),
        102_400 - 400
    );
    assert_eq!(
        next_weak_ratio_with_asks(102_400, 128, 3, &rule()),
        102_400 + 400
    );
}

#[test]
fn a_ratio_above_today_s_maximum_comes_down_once_nobody_asks() {
    let above = 300 * 1_024;
    assert_eq!(
        next_weak_ratio_with_asks(above, 0, 0, &rule()),
        above - above / 256
    );
}

#[test]
fn nobody_asking_the_queue_settles_at_today_s_fill() {
    let mut random = SimulationRandom::from_seed(1);
    let outcome = simulate_ask_queue(&queue(1.0, 0.0, 0.0), &mut random);
    assert_eq!(outcome.asking_fraction, 0.0);
    assert!(
        (30.0..60.0).contains(&outcome.carried_per_share),
        "{} weak shares a share",
        outcome.carried_per_share
    );
}

#[test]
fn everybody_asking_the_queue_settles_at_the_ceiling() {
    let mut random = SimulationRandom::from_seed(1);
    let outcome = simulate_ask_queue(&queue(1.0, f64::INFINITY, 0.0), &mut random);
    assert_eq!(outcome.asking_fraction, 1.0);
    assert!(
        (300.0..400.0).contains(&outcome.carried_per_share),
        "{} weak shares a share",
        outcome.carried_per_share
    );
}

#[test]
fn a_class_stops_asking_at_the_ratio_that_gives_it_enough() {
    let mut random = SimulationRandom::from_seed(1);
    let outcome = simulate_ask_queue(&queue(0.1, 1_000.0, 0.0), &mut random);
    assert!(
        (950.0..1_050.0).contains(&outcome.mean_ratio),
        "the ratio settled at {}",
        outcome.mean_ratio
    );
}

#[test]
fn nodes_that_carry_no_asking_weak_share_leave_the_ratio_at_today_s_maximum() {
    let mut random = SimulationRandom::from_seed(1);
    let outcome = simulate_ask_queue(&queue(1.0, f64::INFINITY, 1.0), &mut random);
    assert_eq!(outcome.carried_per_share, 0.0);
    assert!(outcome.mean_ratio <= 256.0);
}
