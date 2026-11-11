// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;
use p2poolv2_netsim::random::SimulationRandom;

#[test]
fn levels_double_from_the_dust_limit() {
    assert_eq!(level_amount(0), 546);
    assert_eq!(level_amount(5), 17_472);
    assert_eq!(level_amount(7), 69_888);
}

#[test]
fn the_draw_splits_the_pot_into_parts_and_gives_the_first_point_the_remainder() {
    // S = 30, L = 7: n = 4 parts of D = 7, remainder 2. With seed 0 the
    // points are 0, 7, 14 and 21: the first payee covers [0, 10) and holds
    // points 0 and 7, the second [10, 20) holds 14, the third [20, 30) 21.
    // The first point's payee also gets the remainder.
    let winnings = draw_level_group(&[10, 10, 10], 7, 0);
    assert_eq!(winnings, vec![16, 7, 7]);
}

#[test]
fn the_draw_offset_moves_the_points() {
    // Offset 5 mod 7: points 5, 12, 19 and 26: the first payee holds 5, the
    // second 12 and 19, the third 26; the first point is the first payee's.
    let winnings = draw_level_group(&[10, 10, 10], 7, 5);
    assert_eq!(winnings, vec![9, 14, 7]);
}

#[test]
fn the_draw_keeps_the_pot_exact() {
    let amounts = [1_000, 5_000, 700, 12_000, 300, 9_999];
    let pot: u64 = amounts.iter().sum();
    assert_eq!(draw_level_group(&amounts, 546, 1).iter().sum::<u64>(), pot);
    assert_eq!(
        draw_level_group(&amounts, 546, 7_919).iter().sum::<u64>(),
        pot
    );
    assert_eq!(
        draw_level_group(&amounts, 17_472, 2).iter().sum::<u64>(),
        pot
    );
    assert_eq!(
        draw_level_group(&amounts, 17_472, u64::MAX)
            .iter()
            .sum::<u64>(),
        pot
    );
}

#[test]
fn every_winning_is_at_least_the_level() {
    let amounts = [1_000, 5_000, 700, 12_000, 300, 9_999];
    let winnings = draw_level_group(&amounts, 4_368, 12_345);
    assert!(
        winnings
            .iter()
            .filter(|won| **won > 0)
            .all(|won| *won >= 4_368)
    );
}

#[test]
fn a_pot_below_the_level_goes_whole_to_one_payee() {
    // Point 250 mod 600 = 250: the running totals 100, 300 and 600 first
    // exceed it at the second payee.
    let winnings = draw_level_group(&[100, 200, 300], 17_472, 250);
    assert_eq!(winnings, vec![0, 600, 0]);
}

#[test]
fn a_payee_with_nothing_never_wins() {
    let winnings = draw_level_group(&[0, 500, 0, 500], 546, 0);
    assert_eq!(winnings[0], 0);
    assert_eq!(winnings[2], 0);
}

#[test]
fn an_empty_pot_pays_nobody() {
    assert_eq!(draw_level_group(&[0, 0], 546, 3), vec![0, 0]);
}

#[test]
fn each_payee_expects_its_amount_when_the_pot_has_many_parts() {
    // A group of five, pot 100,000 at level 17,472: 5 parts of 20,000. Over
    // many seeded draws each payee's mean winnings must be its amount. The
    // largest standard error over 100,000 draws is about 30 sat.
    let amounts = [3_000, 41_000, 6_000, 27_000, 23_000];
    let mut seeds = SimulationRandom::from_seed(11);
    let draws = 100_000;
    let totals = (0..draws).fold([0u64; 5], |mut totals, _| {
        let winnings = draw_level_group(&amounts, 17_472, seeds.next_u64());
        totals
            .iter_mut()
            .zip(&winnings)
            .for_each(|(total, won)| *total += won);
        totals
    });
    let mean = |index: usize| totals[index] as f64 / draws as f64;
    assert!((mean(0) - 3_000.0).abs() < 120.0, "{}", mean(0));
    assert!((mean(1) - 41_000.0).abs() < 250.0, "{}", mean(1));
    assert!((mean(2) - 6_000.0).abs() < 160.0, "{}", mean(2));
    assert!((mean(3) - 27_000.0).abs() < 250.0, "{}", mean(3));
    assert!((mean(4) - 23_000.0).abs() < 250.0, "{}", mean(4));
}

#[test]
fn each_payee_expects_its_amount_when_the_pot_is_below_the_level() {
    // Pot 10,000 below level 17,472: one point picks a payee for it all.
    let amounts = [1_500, 8_000, 500];
    let mut seeds = SimulationRandom::from_seed(12);
    let draws = 100_000;
    let totals = (0..draws).fold([0u64; 3], |mut totals, _| {
        let winnings = draw_level_group(&amounts, 17_472, seeds.next_u64());
        totals
            .iter_mut()
            .zip(&winnings)
            .for_each(|(total, won)| *total += won);
        totals
    });
    let mean = |index: usize| totals[index] as f64 / draws as f64;
    assert!((mean(0) - 1_500.0).abs() < 60.0, "{}", mean(0));
    assert!((mean(1) - 8_000.0).abs() < 60.0, "{}", mean(1));
    assert!((mean(2) - 500.0).abs() < 40.0, "{}", mean(2));
}
