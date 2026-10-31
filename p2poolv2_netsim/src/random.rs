// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Deterministic pseudo-random numbers for the simulator.
//!
//! A run is a pure function of its scenario and seed, so a result in the
//! documentation can be reproduced exactly on any platform. The generator is
//! xoshiro256** (Blackman and Vigna, 2018) seeded through SplitMix64, both
//! fixed here rather than taken from a crate whose stream may change between
//! versions. It is a simulation generator and must never be used for keys,
//! nonces or anything else that needs to be unpredictable.

/// Seeded xoshiro256** generator.
#[derive(Debug, Clone)]
pub struct SimulationRandom {
    state: [u64; 4],
}

/// One SplitMix64 step: advances `state` and returns the mixed output.
fn split_mix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut mixed = *state;
    mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    mixed ^ (mixed >> 31)
}

impl SimulationRandom {
    /// A generator whose whole stream is fixed by `seed`.
    pub fn from_seed(seed: u64) -> Self {
        let mut seed_state = seed;
        let state = [
            split_mix(&mut seed_state),
            split_mix(&mut seed_state),
            split_mix(&mut seed_state),
            split_mix(&mut seed_state),
        ];
        Self { state }
    }

    /// An independent generator for one part of the model, so that adding a
    /// draw to one part does not shift the numbers every other part sees.
    pub fn derive(&mut self) -> Self {
        Self::from_seed(self.next_u64())
    }

    /// The next 64 pseudo-random bits.
    pub fn next_u64(&mut self) -> u64 {
        let result = self.state[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let shifted = self.state[1] << 17;
        self.state[2] ^= self.state[0];
        self.state[3] ^= self.state[1];
        self.state[1] ^= self.state[2];
        self.state[0] ^= self.state[3];
        self.state[2] ^= shifted;
        self.state[3] = self.state[3].rotate_left(45);
        result
    }

    /// A value in `[0, 1)` built from the top 53 bits.
    pub fn next_unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// A whole number in `[0, bound)`, zero when `bound` is zero. Uses the
    /// high half of a 128-bit product, whose bias is below `bound / 2^64`.
    pub fn next_below(&mut self, bound: u64) -> u64 {
        ((u128::from(self.next_u64()) * u128::from(bound)) >> 64) as u64
    }

    /// True with probability `probability`.
    pub fn next_bool(&mut self, probability: f64) -> bool {
        self.next_unit() < probability
    }

    /// An exponential variate with the given mean: the waiting time of a
    /// Poisson process, such as the time to the next share.
    pub fn next_exponential(&mut self, mean: f64) -> f64 {
        // 1 - unit is in (0, 1], so the logarithm is finite.
        -mean * (1.0 - self.next_unit()).ln()
    }

    /// A normal variate (Box-Muller).
    pub fn next_normal(&mut self, mean: f64, standard_deviation: f64) -> f64 {
        let radius = (-2.0 * (1.0 - self.next_unit()).ln()).sqrt();
        let angle = std::f64::consts::TAU * self.next_unit();
        mean + standard_deviation * radius * angle.cos()
    }

    /// A uniform value in `[low, high)`.
    pub fn next_between(&mut self, low: f64, high: f64) -> f64 {
        low + (high - low) * self.next_unit()
    }
}

#[cfg(test)]
mod tests;
