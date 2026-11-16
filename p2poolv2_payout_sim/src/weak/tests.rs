// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;

#[test]
fn the_weak_ratio_falls_when_the_parent_carried_more_than_the_fill() {
    assert_eq!(next_weak_ratio(102_400, 33, 32, 4_096), 102_400 - 400);
}

#[test]
fn the_weak_ratio_rises_when_the_parent_carried_fewer_than_the_fill() {
    assert_eq!(next_weak_ratio(102_400, 31, 32, 4_096), 102_400 + 400);
}

#[test]
fn the_weak_ratio_holds_when_the_parent_carried_exactly_the_fill() {
    assert_eq!(next_weak_ratio(102_400, 32, 32, 4_096), 102_400);
}

#[test]
fn the_weak_ratio_never_falls_below_one() {
    assert_eq!(next_weak_ratio(1_024, 64, 32, 4_096), 1_024);
}

#[test]
fn the_weak_ratio_never_rises_above_its_maximum() {
    assert_eq!(next_weak_ratio(4_096 * 1_024, 0, 32, 4_096), 4_096 * 1_024);
}

#[test]
fn the_queue_settles_with_about_the_fill_carried_per_share() {
    // Every miner at the minimum, every node carrying.
    let inputs = WeakQueueInputs {
        carrying_minimum_shares_per_period: 1.0,
        non_carrying_minimum_shares_per_period: 0.0,
        carrying_share_fraction: 1.0,
        weak_cap: 64,
        weak_fill: 32,
        weak_depth: 6,
        ratio_max: 4_096,
        ceiling_ratio: 1.0e9,
        shares: 50_000,
        warmup_shares: 5_000,
    };
    let outcome = simulate_weak_queue(&inputs, &mut SimulationRandom::from_seed(1));
    assert!(
        outcome.arrivals_per_share > 30.0 && outcome.arrivals_per_share < 40.0,
        "{}",
        outcome.arrivals_per_share
    );
    // R - 1 weak shares per full share at the minimum.
    assert!((outcome.mean_ratio - 1.0 - outcome.arrivals_per_share).abs() < 1.5);
    assert!(outcome.carrying_expired_fraction < 0.01);
    assert!((outcome.carried_per_share - outcome.arrivals_per_share).abs() < 0.5);
}

#[test]
fn weak_shares_expire_when_few_shares_carry_them() {
    // Nine shares in ten come from nodes that carry only their own weak
    // shares, and those nodes' miners make none.
    let inputs = WeakQueueInputs {
        carrying_minimum_shares_per_period: 0.1,
        non_carrying_minimum_shares_per_period: 0.0,
        carrying_share_fraction: 0.1,
        weak_cap: 64,
        weak_fill: 32,
        weak_depth: 6,
        ratio_max: 4_096,
        ceiling_ratio: 1.0e9,
        shares: 50_000,
        warmup_shares: 20_000,
    };
    let outcome = simulate_weak_queue(&inputs, &mut SimulationRandom::from_seed(2));
    // No carrying share in the next six: 0.9^6 = 53%, and the carrying
    // shares fill up besides.
    assert!(
        outcome.carrying_expired_fraction > 0.5,
        "{}",
        outcome.carrying_expired_fraction
    );
}

#[test]
fn the_ceiling_bounds_the_weak_ratio() {
    let inputs = WeakQueueInputs {
        carrying_minimum_shares_per_period: 0.5,
        non_carrying_minimum_shares_per_period: 0.0,
        carrying_share_fraction: 1.0,
        weak_cap: 64,
        weak_fill: 32,
        weak_depth: 6,
        ratio_max: 4_096,
        ceiling_ratio: 5.0,
        shares: 20_000,
        warmup_shares: 5_000,
    };
    let outcome = simulate_weak_queue(&inputs, &mut SimulationRandom::from_seed(3));
    assert!((outcome.mean_ratio - 5.0).abs() < 1e-9);
    assert!((outcome.arrivals_per_share - 2.0).abs() < 0.1);
}

#[test]
fn a_share_carries_no_more_than_the_cap() {
    let inputs = WeakQueueInputs {
        carrying_minimum_shares_per_period: 1.0,
        non_carrying_minimum_shares_per_period: 0.0,
        carrying_share_fraction: 1.0,
        weak_cap: 8,
        weak_fill: 7,
        weak_depth: 6,
        ratio_max: 4_096,
        ceiling_ratio: 1.0e9,
        shares: 20_000,
        warmup_shares: 5_000,
    };
    let outcome = simulate_weak_queue(&inputs, &mut SimulationRandom::from_seed(4));
    assert!(outcome.carried_per_share <= 8.0);
    assert!(outcome.full_share_fraction > 0.0);
}

#[test]
fn a_carrying_node_carries_all_kinds_and_a_refusing_one_its_own() {
    let mut waiting = WaitingWeakShares::new(2);
    waiting.add_at(1, 3, 2);
    let mut refusing = waiting.clone();
    assert_eq!(
        waiting.carry(true, 64),
        Carried {
            carrying: 3,
            non_carrying: 2
        }
    );
    assert_eq!(
        refusing.carry(false, 64),
        Carried {
            carrying: 0,
            non_carrying: 2
        }
    );
}

#[test]
fn weak_shares_added_at_a_depth_join_it() {
    let mut waiting = WaitingWeakShares::new(3);
    waiting.add_at(1, 4, 1);
    waiting.add_at(1, 1, 0);
    assert_eq!(waiting.carrying, vec![0, 5, 0]);
    assert_eq!(waiting.non_carrying, vec![0, 1, 0]);
}

#[test]
fn keeping_what_two_carriers_left_drops_what_either_took() {
    // A refusing share took the non-carrying ones; a carrying share took
    // everything at the deeper depth. What stays is what both left.
    let mut waiting = WaitingWeakShares::new(2);
    waiting.add_at(0, 2, 1);
    waiting.add_at(1, 3, 4);
    let mut after_carrier = waiting.clone();
    after_carrier.carry_all(7);
    let mut after_refuser = waiting.clone();
    after_refuser.carry_own(64);
    let dropped = after_carrier.keep_shared_with(&after_refuser);
    assert_eq!(after_carrier.carrying, vec![2, 0]);
    assert_eq!(after_carrier.non_carrying, vec![0, 0]);
    assert_eq!(
        dropped,
        Carried {
            carrying: 0,
            non_carrying: 1
        }
    );
}
