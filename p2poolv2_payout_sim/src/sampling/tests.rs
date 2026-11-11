// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;

#[test]
fn poisson_of_a_zero_mean_is_zero() {
    let mut random = SimulationRandom::from_seed(1);
    assert_eq!(poisson(&mut random, 0.0), 0);
    assert_eq!(poisson(&mut random, -3.0), 0);
}

#[test]
fn poisson_mean_matches_a_small_mean() {
    let mut random = SimulationRandom::from_seed(2);
    let draws = 40_000;
    let total: u64 = (0..draws).map(|_| poisson(&mut random, 3.5)).sum();
    let mean = total as f64 / draws as f64;
    assert!((mean - 3.5).abs() < 0.04, "mean {mean}");
}

#[test]
fn poisson_mean_matches_a_mean_drawn_in_several_chunks() {
    let mut random = SimulationRandom::from_seed(3);
    let draws = 20_000;
    let total: u64 = (0..draws).map(|_| poisson(&mut random, 75.0)).sum();
    let mean = total as f64 / draws as f64;
    assert!((mean - 75.0).abs() < 0.3, "mean {mean}");
}

#[test]
fn poisson_variance_equals_its_mean() {
    let mut random = SimulationRandom::from_seed(4);
    let draws: Vec<f64> = (0..40_000)
        .map(|_| poisson(&mut random, 12.0) as f64)
        .collect();
    let mean = draws.iter().sum::<f64>() / draws.len() as f64;
    let variance = draws.iter().map(|draw| (draw - mean).powi(2)).sum::<f64>() / draws.len() as f64;
    assert!((variance / 12.0 - 1.0).abs() < 0.05, "variance {variance}");
}

#[test]
fn poisson_above_the_exact_range_keeps_its_mean() {
    let mut random = SimulationRandom::from_seed(5);
    let draws = 10_000;
    let total: u64 = (0..draws).map(|_| poisson(&mut random, 5_000.0)).sum();
    let mean = total as f64 / draws as f64;
    assert!((mean - 5_000.0).abs() < 3.0, "mean {mean}");
}

#[test]
fn poisson_times_are_ordered_inside_their_span_at_their_rate() {
    let mut random = SimulationRandom::from_seed(6);
    let times = poisson_times(&mut random, 0.5, -100.0, 19_900.0);
    assert!(times.windows(2).all(|pair| pair[0] <= pair[1]));
    assert!(times.first().is_some_and(|time| *time >= -100.0));
    assert!(times.last().is_some_and(|time| *time < 19_900.0));
    // 10,000 expected, standard deviation 100.
    assert!(
        (times.len() as f64 - 10_000.0).abs() < 400.0,
        "{}",
        times.len()
    );
}

#[test]
fn poisson_times_at_a_zero_rate_are_empty() {
    let mut random = SimulationRandom::from_seed(7);
    assert!(poisson_times(&mut random, 0.0, 0.0, 1_000.0).is_empty());
}
