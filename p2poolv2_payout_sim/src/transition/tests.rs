// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;
use crate::miners::MinerMix;

#[test]
fn the_band_cap_rises_by_the_whole_wanted_multiple_when_the_parent_wanted_more() {
    // C = 2, the node wanted 10.5 times the minimum: u = 10.
    assert_eq!(
        next_band_cap(2_048, 10.5, 64, 512),
        2_048 + 2_048 * 10 / 512
    );
}

#[test]
fn the_band_cap_step_takes_at_most_c_max_as_the_wanted_multiple() {
    assert_eq!(
        next_band_cap(2_048, 1_000.0, 64, 512),
        2_048 + 2_048 * 64 / 512
    );
}

#[test]
fn a_parent_wanting_exactly_its_cap_pulls_the_band_cap_down() {
    // Wanted more means wanted harder than the cap allowed.
    assert_eq!(next_band_cap(2_048, 2.0, 64, 512), 2_048 - 4);
}

#[test]
fn a_wanted_multiple_between_one_and_two_pushes_by_one() {
    assert_eq!(next_band_cap(1_024, 1.5, 64, 512), 1_024 + 2);
}

#[test]
fn the_band_cap_never_falls_below_one() {
    assert_eq!(next_band_cap(1_024, 1.0, 64, 512), 1_024);
}

#[test]
fn the_band_cap_never_rises_above_c_max() {
    assert_eq!(next_band_cap(64 * 1_024, 1_000.0, 64, 512), 64 * 1_024);
}

#[test]
fn asert_holds_the_minimum_when_the_parent_took_one_period() {
    assert_eq!(next_minimum(100.0, 10.0, 10.0, 4.5), 100.0);
}

#[test]
fn asert_halves_the_minimum_a_half_life_late() {
    assert!((next_minimum(100.0, 610.0, 10.0, 4.5) - 50.0).abs() < 1e-9);
}

#[test]
fn asert_doubles_the_minimum_a_half_life_early_at_most_by_its_step() {
    // A share can come at once, so the step is at most 2^(10 / 600).
    let step = next_minimum(100.0, 0.0, 10.0, 4.5) / 100.0;
    assert!((step - (10.0_f64 / 600.0).exp2()).abs() < 1e-12);
}

#[test]
fn asert_never_goes_below_the_ceiling() {
    assert_eq!(next_minimum(5.0, 10_000.0, 10.0, 4.5), 4.5);
}

#[test]
fn a_measure_already_at_its_level_reaches_it_in_the_first_hour() {
    assert_eq!(reach_hour(&[1.05, 0.5, 1.0], 1.0, 0.1), Some(0));
}

#[test]
fn a_measure_reaches_its_level_in_its_first_hour_within_the_tolerance() {
    assert_eq!(reach_hour(&[2.0, 1.5, 1.09, 0.95], 1.0, 0.1), Some(2));
}

#[test]
fn a_measure_never_within_the_tolerance_does_not_reach_its_level() {
    assert_eq!(reach_hour(&[2.0, 1.5, 1.2], 1.0, 0.1), None);
}

#[test]
fn the_window_takes_shares_back_until_their_work_reaches_the_threshold() {
    // Ten shares of work 1, the newest four the home miners' alone. Skip
    // the newest two, then take back until 3 units: shares 5 to 7, of
    // which 6 and 7 are home miners'.
    let mut ledger = CreditLedger::with_capacity(10);
    ledger.push(1.0, 0.0);
    ledger.push(1.0, 0.0);
    ledger.push(1.0, 0.0);
    ledger.push(1.0, 0.0);
    ledger.push(1.0, 0.0);
    ledger.push(1.0, 0.0);
    ledger.push(1.0, 1.0);
    ledger.push(1.0, 1.0);
    ledger.push(1.0, 1.0);
    ledger.push(1.0, 1.0);
    let fraction = ledger.home_fraction(2, 3.0, 100);
    assert!((fraction - 2.0 / 3.0).abs() < 1e-12, "{fraction}");
}

#[test]
fn the_window_stops_at_its_share_cap() {
    let mut ledger = CreditLedger::with_capacity(4);
    ledger.push(1.0, 0.0);
    ledger.push(1.0, 1.0);
    ledger.push(1.0, 1.0);
    ledger.push(1.0, 1.0);
    // The work would reach back to the first share; the cap of two keeps
    // only the home miners' two before the skipped newest one.
    assert_eq!(ledger.home_fraction(1, 100.0, 2), 1.0);
}

#[test]
fn weak_slots_carry_the_oldest_first_with_their_work() {
    let mut slots = WeakSlots::new(3);
    slots.deepen(WeakBatch {
        count: 4,
        work: 8.0,
        home_work: 2.0,
    });
    slots.deepen(WeakBatch {
        count: 2,
        work: 2.0,
        home_work: 2.0,
    });
    let carried = slots.carry(5);
    // All four older ones, then one of the two newer ones.
    assert_eq!(carried.count, 5);
    assert!((carried.work - 9.0).abs() < 1e-12);
    assert!((carried.home_work - 3.0).abs() < 1e-12);
}

#[test]
fn weak_shares_older_than_the_depth_expire() {
    let mut slots = WeakSlots::new(2);
    slots.deepen(WeakBatch {
        count: 7,
        work: 7.0,
        home_work: 0.0,
    });
    slots.deepen(WeakBatch::default());
    assert_eq!(slots.deepen(WeakBatch::default()), 7);
}

#[test]
fn without_any_change_home_miners_are_paid_fairly() {
    // A "big miner" with no hashrate joins: nothing happens.
    let pool_hashrate = 3.7e17;
    let mut classes = MinerMix::HalfIndustrial.classes(pool_hashrate, 5);
    let big_miner = classes.len();
    classes.push(MinerClass {
        name: "nobody".to_string(),
        kind: MinerKind::Industrial,
        hashrate: pool_hashrate,
        count: 0.0,
        follows_node_default: true,
        on_carrying_node: true,
        level: 5,
    });
    let inputs = TransitionInputs {
        classes,
        big_miner,
        big_miner_leaves: false,
        network_hashrate: 3.7e18,
        period_seconds: 10.0,
        stale_rate: 0.021,
        ceiling_difficulty: 4.55,
        cap_max: 64,
        band_step_divisor: 512,
        overstating: None,
        weak_cap: 64,
        weak_fill: 32,
        weak_depth: 6,
        ratio_max: 4_096,
        includer_cut: 0.05,
        event_seconds: 2.0 * 86_400.0,
        after_seconds: 86_400.0,
        asks: None,
    };
    let hours = simulate_transition(&inputs, &mut SimulationRandom::from_seed(1));
    let mut after = HourSample::default();
    hours[HOURS_BEFORE_EVENT..]
        .iter()
        .for_each(|hour| after.add(hour));
    let ratio = after.home_income / after.home_fair_income;
    assert!((ratio - 1.0).abs() < 0.03, "{ratio}");
}

#[test]
fn a_big_miner_joining_pays_home_miners_more_until_the_window_turns_over() {
    // At 10% of Bitcoin Cash the window is three one-minute blocks of the
    // pool's work, half an hour, behind a skip of 100 shares (about 17
    // minutes). When the pool doubles, blocks come twice as often and for
    // about the next half hour pay from a window of mostly the old pool's
    // work, so the first hour pays home miners about 1.36 times fair, and by
    // the second hour the window has turned over. (Ten-minute blocks gave a
    // five-hour window, and the whole first hour paid about twice fair.) The
    // big miner mines at the minimum, so its credit in a window is not a few
    // heavy shares.
    let window_seconds = BCH_PPLNS_WINDOW_BLOCKS * BCH_BLOCK_INTERVAL_SECONDS / 0.1;
    let skip_seconds = MATURITY_SKIP_SHARES * 10.0;
    assert!(window_seconds + skip_seconds < HOUR_SECONDS);
    let pool_hashrate = 3.7e17;
    let mut classes = MinerMix::HalfIndustrial.classes(pool_hashrate, 5);
    let big_miner = classes.len();
    classes.push(MinerClass {
        name: "big miner".to_string(),
        kind: MinerKind::Industrial,
        hashrate: pool_hashrate,
        count: 1.0,
        follows_node_default: false,
        on_carrying_node: true,
        level: 5,
    });
    let inputs = TransitionInputs {
        classes,
        big_miner,
        big_miner_leaves: false,
        network_hashrate: 3.7e18,
        period_seconds: 10.0,
        stale_rate: 0.021,
        ceiling_difficulty: 4.55,
        cap_max: 64,
        band_step_divisor: 512,
        overstating: None,
        weak_cap: 64,
        weak_fill: 32,
        weak_depth: 6,
        ratio_max: 4_096,
        includer_cut: 0.05,
        event_seconds: 86_400.0,
        after_seconds: 86_400.0,
        asks: None,
    };
    let hours = simulate_transition(&inputs, &mut SimulationRandom::from_seed(2));
    let first_hour = hours[HOURS_BEFORE_EVENT];
    let ratio = first_hour.home_income / first_hour.home_fair_income;
    assert!(ratio > 1.2 && ratio < 1.5, "{ratio}");
    let second_hour = hours[HOURS_BEFORE_EVENT + 1];
    let turned_over = second_hour.home_income / second_hour.home_fair_income;
    assert!((turned_over - 1.0).abs() < 0.05, "{turned_over}");
    let mut last_hours = HourSample::default();
    hours[hours.len() - 6..]
        .iter()
        .for_each(|hour| last_hours.add(hour));
    let settled = last_hours.home_income / last_hours.home_fair_income;
    assert!((settled - 1.0).abs() < 0.05, "{settled}");
}

#[test]
fn a_big_miner_at_the_minimum_raises_the_minimum_when_it_joins() {
    let pool_hashrate = 3.7e17;
    let mut classes = MinerMix::Home.classes(pool_hashrate, 5);
    let big_miner = classes.len();
    classes.push(MinerClass {
        name: "big miner".to_string(),
        kind: MinerKind::Industrial,
        hashrate: pool_hashrate,
        count: 1.0,
        follows_node_default: false,
        on_carrying_node: true,
        level: 5,
    });
    let inputs = TransitionInputs {
        classes,
        big_miner,
        big_miner_leaves: false,
        network_hashrate: 3.7e18,
        period_seconds: 10.0,
        stale_rate: 0.021,
        ceiling_difficulty: 4.55,
        cap_max: 64,
        band_step_divisor: 512,
        overstating: None,
        weak_cap: 64,
        weak_fill: 32,
        weak_depth: 6,
        ratio_max: 4_096,
        includer_cut: 0.05,
        event_seconds: 86_400.0,
        after_seconds: 86_400.0,
        asks: None,
    };
    let hours = simulate_transition(&inputs, &mut SimulationRandom::from_seed(3));
    let before = hours[HOURS_BEFORE_EVENT - 1].minimum();
    let after = hours[hours.len() - 1].minimum();
    // Every miner at the minimum and the hashrate doubled.
    assert!((after / before - 2.0).abs() < 0.2, "{}", after / before);
}

#[test]
fn a_measure_that_never_leaves_its_level_stays_from_the_first_hour() {
    assert_eq!(stay_hour(&[1.1, 0.9, 1.0], 1.0, 0.25), Some(0));
}

#[test]
fn a_measure_stays_from_the_hour_after_its_last_one_outside() {
    // Reached in hour 1, left in hour 2, back for good from hour 3.
    assert_eq!(stay_hour(&[3.0, 1.0, 2.0, 1.1, 0.9], 1.0, 0.25), Some(3));
}

#[test]
fn a_measure_outside_its_level_at_the_end_does_not_stay() {
    assert_eq!(stay_hour(&[1.0, 1.0, 2.0], 1.0, 0.25), None);
}

#[test]
fn weak_slots_take_a_part_of_a_depth_with_its_part_of_the_work() {
    let mut slots = WeakSlots::new(2);
    slots.deepen(WeakBatch {
        count: 4,
        work: 8.0,
        home_work: 4.0,
    });
    let taken = slots.take(0, 1);
    assert_eq!(taken.count, 1);
    assert!((taken.work - 2.0).abs() < 1e-12);
    assert!((taken.home_work - 1.0).abs() < 1e-12);
    assert_eq!(slots.count[0], 3);
    assert_eq!(slots.take(0, 0), WeakBatch::default());
}

/// A pool of home miners at 30% of a Bitcoin Cash ten times today's, from
/// genesis for `seconds`, with `asks`.
fn home_pool_from_genesis(asks: Option<WeakAsks>, seconds: f64) -> HourSample {
    let pool_hashrate = 0.3 * 3.7e19;
    let mut classes = MinerMix::Home.classes(pool_hashrate, 5);
    let big_miner = classes.len();
    classes.push(MinerClass {
        name: "nobody".to_string(),
        kind: MinerKind::Industrial,
        hashrate: pool_hashrate,
        count: 0.0,
        follows_node_default: true,
        on_carrying_node: true,
        level: 5,
    });
    let inputs = TransitionInputs {
        classes,
        big_miner,
        big_miner_leaves: false,
        network_hashrate: 3.7e19,
        period_seconds: 10.0,
        stale_rate: 0.021,
        ceiling_difficulty: 4.55,
        cap_max: 64,
        band_step_divisor: 512,
        overstating: None,
        weak_cap: 64,
        weak_fill: 32,
        weak_depth: 48,
        ratio_max: 256,
        includer_cut: 0.01,
        event_seconds: 0.0,
        after_seconds: seconds,
        asks,
    };
    let hours = simulate_transition(&inputs, &mut SimulationRandom::from_seed(5));
    let mut last_hour = HourSample::default();
    hours[hours.len() - 1..]
        .iter()
        .for_each(|hour| last_hour.add(hour));
    last_hour
}

/// The steady-state model puts this pool at the ceiling, 344 weak shares a
/// share, with every home miner asking; share by share the asks open it
/// there too, and today's rule holds it at 32.
#[test]
fn share_by_share_the_asks_open_a_pool_whose_home_miners_need_them() {
    let hours = 3.0 * 3_600.0;
    let asking = home_pool_from_genesis(Some(WeakAsks::default()), hours);
    let today = home_pool_from_genesis(None, hours);
    assert!(
        (280.0..420.0).contains(&asking.carried_per_share()),
        "{} weak shares a share",
        asking.carried_per_share()
    );
    assert!(asking.weak_asking > 0.9 * asking.weak_carried);
    assert!(
        today.carried_per_share() < 64.0,
        "{} weak shares a share today",
        today.carried_per_share()
    );
    assert_eq!(today.weak_asking, 0.0);
    assert!(today.largest_carried <= 64.0);
}
