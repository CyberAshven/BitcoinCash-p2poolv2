// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;

#[test]
fn split_mix_matches_the_reference_stream_for_seed_zero() {
    // The first outputs of Vigna's splitmix64.c for a zero seed.
    let mut state = 0u64;
    assert_eq!(split_mix(&mut state), 0xe220_a839_7b1d_cdaf);
    assert_eq!(split_mix(&mut state), 0x6e78_9e6a_a1b9_65f4);
    assert_eq!(split_mix(&mut state), 0x06c4_5d18_8009_454f);
    assert_eq!(split_mix(&mut state), 0xf88b_b8a8_724c_81ec);
}

#[test]
fn the_same_seed_gives_the_same_stream() {
    let mut first = SimulationRandom::from_seed(42);
    let mut second = SimulationRandom::from_seed(42);
    assert_eq!(first.next_u64(), second.next_u64());
    assert_eq!(first.next_u64(), second.next_u64());
    assert_eq!(first.next_unit(), second.next_unit());
}

#[test]
fn different_seeds_give_different_streams() {
    let mut first = SimulationRandom::from_seed(1);
    let mut second = SimulationRandom::from_seed(2);
    assert_ne!(first.next_u64(), second.next_u64());
}

#[test]
fn xoshiro_matches_the_reference_stream_for_state_one_to_four() {
    // The first outputs of Blackman and Vigna's xoshiro256starstar.c from
    // the state {1, 2, 3, 4}, as the rand_xoshiro crate tests them.
    let mut random = SimulationRandom {
        state: [1, 2, 3, 4],
    };
    assert_eq!(random.next_u64(), 11_520);
    assert_eq!(random.next_u64(), 0);
    assert_eq!(random.next_u64(), 1_509_978_240);
    assert_eq!(random.next_u64(), 1_215_971_899_390_074_240);
}

#[test]
fn the_stream_for_seed_one_is_pinned() {
    // Pins the generator: a change to it changes every published result,
    // so it has to show up here first.
    let mut random = SimulationRandom::from_seed(1);
    assert_eq!(random.next_u64(), 12_966_619_160_104_079_557);
}

#[test]
fn a_derived_generator_differs_from_its_parent() {
    let mut parent = SimulationRandom::from_seed(7);
    let mut derived = parent.derive();
    assert_ne!(parent.next_u64(), derived.next_u64());
}

#[test]
fn unit_values_stay_in_the_half_open_interval() {
    let mut random = SimulationRandom::from_seed(3);
    let smallest = (0..10_000)
        .map(|_| random.next_unit())
        .fold(f64::INFINITY, f64::min);
    let largest = (0..10_000)
        .map(|_| random.next_unit())
        .fold(f64::NEG_INFINITY, f64::max);
    assert!(smallest >= 0.0);
    assert!(largest < 1.0);
}

#[test]
fn next_below_never_reaches_its_bound() {
    let mut random = SimulationRandom::from_seed(4);
    let largest = (0..10_000).map(|_| random.next_below(7)).max();
    assert_eq!(largest, Some(6));
}

#[test]
fn next_below_zero_is_zero() {
    let mut random = SimulationRandom::from_seed(4);
    assert_eq!(random.next_below(0), 0);
}

#[test]
fn exponential_draws_have_the_requested_mean() {
    let mut random = SimulationRandom::from_seed(5);
    let draws = 200_000;
    let mean = (0..draws)
        .map(|_| random.next_exponential(10.0))
        .sum::<f64>()
        / draws as f64;
    assert!((mean - 10.0).abs() < 0.1, "mean was {mean}");
}

#[test]
fn normal_draws_have_the_requested_mean_and_spread() {
    let mut random = SimulationRandom::from_seed(6);
    let draws = 200_000;
    let samples: Vec<f64> = (0..draws).map(|_| random.next_normal(15.0, 4.0)).collect();
    let mean = samples.iter().sum::<f64>() / draws as f64;
    let variance = samples
        .iter()
        .map(|sample| (sample - mean) * (sample - mean))
        .sum::<f64>()
        / draws as f64;
    assert!((mean - 15.0).abs() < 0.05, "mean was {mean}");
    assert!(
        (variance.sqrt() - 4.0).abs() < 0.05,
        "deviation was {variance}"
    );
}
