// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Random draws the model needs beyond `SimulationRandom`'s own: Poisson
//! counts and Poisson event times.

use p2poolv2_netsim::random::SimulationRandom;

/// Means up to this are drawn exactly by multiplying uniforms (Knuth), in
/// chunks small enough that `exp(-chunk)` stays far above the smallest
/// double.
const EXACT_POISSON_CHUNK: f64 = 30.0;

/// Means above this are drawn from the normal approximation, whose error is
/// below `1 / sqrt(mean)`, about 3% of a standard deviation here.
const NORMAL_POISSON_MEAN: f64 = 1_000.0;

/// A Poisson count with the given mean. Exact up to a mean of 1,000, the
/// normal approximation above it; zero for a mean that is not positive.
pub fn poisson(random: &mut SimulationRandom, mean: f64) -> u64 {
    if mean <= 0.0 || !mean.is_finite() {
        0
    } else if mean > NORMAL_POISSON_MEAN {
        let draw = random.next_normal(mean, mean.sqrt()).round();
        draw.max(0.0) as u64
    } else {
        let mut remaining = mean;
        let mut count = 0;
        while remaining > 0.0 {
            let chunk = remaining.min(EXACT_POISSON_CHUNK);
            count += poisson_by_multiplication(random, chunk);
            remaining -= chunk;
        }
        count
    }
}

/// Knuth's method: the number of uniforms whose running product stays above
/// `exp(-mean)`, less one.
fn poisson_by_multiplication(random: &mut SimulationRandom, mean: f64) -> u64 {
    let limit = (-mean).exp();
    let mut product = 1.0 - random.next_unit();
    let mut count = 0;
    while product > limit {
        product *= 1.0 - random.next_unit();
        count += 1;
    }
    count
}

/// The times of a Poisson process with `rate` events a second between
/// `start` and `end`, in order.
pub fn poisson_times(random: &mut SimulationRandom, rate: f64, start: f64, end: f64) -> Vec<f64> {
    let expected = if rate > 0.0 && end > start {
        rate * (end - start)
    } else {
        0.0
    };
    let mut times = Vec::with_capacity((expected * 1.1 + 16.0) as usize);
    if expected > 0.0 {
        let mean_gap = 1.0 / rate;
        let mut time = start + random.next_exponential(mean_gap);
        while time < end {
            times.push(time);
            time += random.next_exponential(mean_gap);
        }
    }
    times
}

#[cfg(test)]
mod tests;
