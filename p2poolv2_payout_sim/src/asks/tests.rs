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
        genesis_ratio: 256,
        descent: RatioDescent::Step,
        cap: CapRule::Fixed,
    }
}

/// The rule with the faster way down, at most to `R / bound` in a share.
fn faster_down(bound: u64) -> AskRule {
    AskRule {
        descent: RatioDescent::Proportional { bound },
        ..rule()
    }
}

/// The rule with the cap following the asks, opened by more than one
/// asking weak share in `divisor`.
fn cap_follows_asks(divisor: u64) -> AskRule {
    AskRule {
        cap: CapRule::FollowsAsks { divisor },
        ..rule()
    }
}

/// What a share takes from the waiting weak shares under the
/// rules as they are with a cap of `cap`: [`WaitingWeakShares`]'s own carry.
fn carried_today(asking: &[u64], silent: &[u64], cap: u64, censoring: bool) -> Vec<Carried> {
    let mut waiting = WaitingWeakShares::new(asking.len());
    asking
        .iter()
        .zip(silent)
        .enumerate()
        .for_each(|(depth, (asking, silent))| waiting.add_at(depth, *asking, *silent));
    let before = waiting.clone();
    if censoring {
        waiting.carry_own(cap);
    } else {
        waiting.carry_all(cap);
    }
    before
        .carrying
        .iter()
        .zip(&before.non_carrying)
        .zip(waiting.carrying.iter().zip(&waiting.non_carrying))
        .map(|((asking, silent), (asking_left, silent_left))| Carried {
            carrying: asking - asking_left,
            non_carrying: silent - silent_left,
        })
        .collect()
}

/// The total of a plan, by kind.
fn plan_total(plan: &[Carried]) -> Carried {
    plan.iter().fold(Carried::default(), |sum, depth| Carried {
        carrying: sum.carrying + depth.carrying,
        non_carrying: sum.non_carrying + depth.non_carrying,
    })
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

#[test]
fn the_faster_way_down_takes_the_ratio_to_the_fill_at_once() {
    // 40 weak shares at R = 100: to 100 * 32 / 40 = 80.
    assert_eq!(
        next_weak_ratio_with_asks(102_400, 40, 0, &faster_down(2)),
        102_400 * 32 / 40
    );
}

#[test]
fn the_faster_way_down_falls_at_most_to_its_bound() {
    assert_eq!(
        next_weak_ratio_with_asks(102_400, 640, 0, &faster_down(2)),
        51_200
    );
    assert_eq!(
        next_weak_ratio_with_asks(102_400, 640, 0, &faster_down(4)),
        25_600
    );
}

#[test]
fn the_faster_way_down_leaves_every_other_step_as_written() {
    [1_024, 50_000, 102_400, 300 * 1_024]
        .into_iter()
        .for_each(|ratio| {
            [(0, 0), (31, 0), (32, 0), (40, 1), (320, 6), (321, 321)]
                .into_iter()
                .for_each(|(carried, asking)| {
                    let written = next_weak_ratio_with_asks(ratio, carried, asking, &rule());
                    let asked = asking * 64 > carried && carried <= 320;
                    if asked || carried <= 32 {
                        assert_eq!(
                            next_weak_ratio_with_asks(ratio, carried, asking, &faster_down(2)),
                            written,
                            "ratio {ratio}, carried {carried}, asking {asking}"
                        );
                    }
                });
        });
}

#[test]
fn a_fixed_cap_carries_as_the_queue_always_has() {
    let asking = [0, 3, 0, 40, 7, 1];
    let silent = [5, 0, 90, 300, 2, 600];
    [10, 64, 300, 640].into_iter().for_each(|cap| {
        let rule = AskRule {
            weak_fill_max: cap / 2,
            ..rule()
        };
        [false, true].into_iter().for_each(|censoring| {
            assert_eq!(
                carry_plan(&asking, &silent, &rule, censoring),
                carried_today(&asking, &silent, cap, censoring),
                "cap {cap}, censoring {censoring}"
            );
        });
    });
}

#[test]
fn a_cap_that_follows_the_asks_carries_64_when_none_ask() {
    let plan = carry_plan(&[0; 4], &[50, 50, 50, 50], &cap_follows_asks(64), false);
    // The oldest first: all 50 at the deepest, then 14 of the next.
    assert_eq!(plan_total(&plan).total(), 64);
    assert_eq!(plan[3].non_carrying, 50);
    assert_eq!(plan[2].non_carrying, 14);
    assert_eq!(plan[0].non_carrying, 0);
}

#[test]
fn a_cap_that_follows_the_asks_opens_while_enough_of_them_ask() {
    // 4 asking in 200: 4 * 64 > 200, so the share carries all 200.
    let plan = carry_plan(&[0, 0, 4], &[100, 50, 46], &cap_follows_asks(64), false);
    let total = plan_total(&plan);
    assert_eq!(total.carrying, 4);
    assert_eq!(total.non_carrying, 196);
}

#[test]
fn a_cap_that_follows_the_asks_leaves_the_newest_silent_ones_for_too_few_asks() {
    // 2 asking in 400: at most 2 * 64 - 1 = 127, the newest silent ones
    // left waiting.
    let plan = carry_plan(&[0, 0, 2], &[200, 100, 98], &cap_follows_asks(64), false);
    let total = plan_total(&plan);
    assert_eq!(total.carrying, 2);
    assert_eq!(total.total(), 127);
    assert_eq!(plan[0].non_carrying, 0);
    assert_eq!(plan[1].non_carrying, 27);
    assert_eq!(plan[2].non_carrying, 98);
}

#[test]
fn one_asking_weak_share_opens_a_cap_with_the_largest_divisor() {
    let plan = carry_plan(&[0, 1], &[300, 299], &cap_follows_asks(640), false);
    assert_eq!(plan_total(&plan).total(), 600);
    let none = carry_plan(&[0, 0], &[300, 300], &cap_follows_asks(640), false);
    assert_eq!(plan_total(&none).total(), 64);
}

#[test]
fn a_censoring_share_under_a_cap_that_follows_the_asks_carries_64_silent_ones() {
    let plan = carry_plan(&[30, 30], &[300, 300], &cap_follows_asks(64), true);
    let total = plan_total(&plan);
    assert_eq!(total.carrying, 0);
    assert_eq!(total.non_carrying, 64);
}

#[test]
fn the_queue_starts_at_the_rule_s_genesis_ratio() {
    // No weak shares at the minimum: the ratio only rises from genesis,
    // so a start at 32 stays below a start at 256 for the first shares.
    let inputs = |genesis_ratio: u64| AskQueueInputs {
        rule: AskRule {
            genesis_ratio,
            ..rule()
        },
        shares: 50,
        warmup_shares: 0,
        ..queue(0.0, 0.0, 0.0)
    };
    let low = simulate_ask_queue(&inputs(32), &mut SimulationRandom::from_seed(1));
    let high = simulate_ask_queue(&inputs(256), &mut SimulationRandom::from_seed(1));
    assert!(low.mean_ratio < 40.0, "{}", low.mean_ratio);
    assert!(high.mean_ratio > 200.0, "{}", high.mean_ratio);
}
