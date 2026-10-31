// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;

#[test]
fn a_bare_number_is_a_fixed_value() {
    assert_eq!(Distribution::parse("50"), Ok(Distribution::fixed(50.0)));
    assert_eq!(
        Distribution::parse("fixed:2.5"),
        Ok(Distribution::fixed(2.5))
    );
}

#[test]
fn uniform_and_lognormal_parse_with_their_parameters() {
    assert_eq!(
        Distribution::parse("uniform:10:100"),
        Ok(Distribution::Uniform {
            low: 10.0,
            high: 100.0
        })
    );
    assert_eq!(
        Distribution::parse("lognormal:50:0.7"),
        Ok(Distribution::LogNormal {
            median: 50.0,
            sigma: 0.7
        })
    );
}

#[test]
fn zero_negative_and_unknown_forms_are_refused() {
    assert!(Distribution::parse("0").is_err());
    assert!(Distribution::parse("-5").is_err());
    assert!(Distribution::parse("uniform:100:10").is_err());
    assert!(Distribution::parse("gamma:1:2").is_err());
    assert!(Distribution::parse("lognormal:50").is_err());
}

#[test]
fn the_command_line_form_round_trips() {
    let distribution = Distribution::LogNormal {
        median: 50.0,
        sigma: 0.7,
    };
    assert_eq!(
        Distribution::parse(&distribution.describe()),
        Ok(distribution)
    );
}

#[test]
fn a_fixed_value_is_always_drawn() {
    let mut random = SimulationRandom::from_seed(1);
    assert_eq!(Distribution::fixed(7.0).sample(&mut random), 7.0);
    assert_eq!(Distribution::fixed(7.0).sample(&mut random), 7.0);
}

#[test]
fn uniform_draws_stay_within_their_bounds() {
    let mut random = SimulationRandom::from_seed(2);
    let distribution = Distribution::Uniform {
        low: 10.0,
        high: 20.0,
    };
    let draws: Vec<f64> = (0..10_000)
        .map(|_| distribution.sample(&mut random))
        .collect();
    let smallest = draws.iter().copied().fold(f64::INFINITY, f64::min);
    let largest = draws.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    assert!(smallest >= 10.0, "smallest {smallest}");
    assert!(largest < 20.0, "largest {largest}");
}

#[test]
fn half_of_lognormal_draws_fall_below_the_median() {
    let mut random = SimulationRandom::from_seed(3);
    let distribution = Distribution::LogNormal {
        median: 50.0,
        sigma: 0.7,
    };
    let draws = 100_000;
    let below = (0..draws)
        .filter(|_| distribution.sample(&mut random) < 50.0)
        .count();
    let fraction = below as f64 / draws as f64;
    assert!((fraction - 0.5).abs() < 0.01, "fraction {fraction}");
    assert_eq!(distribution.median(), 50.0);
}
