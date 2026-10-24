// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A tiny seeded generator for reproducible sim choices.
//!
//! The sim harness draws per-node latencies and failure targets from a seed so
//! a failing nightly can be rerun exactly. The scripts it replaced used awk's
//! `srand(seed); rand()`, whose sequence differs between awk implementations.
//! SplitMix64 is fixed here, so a seed now picks the same values on every
//! platform. It is not, and must never be used as, a source of secrets.

/// SplitMix64 (Steele, Lea and Flood 2014).
#[derive(Debug, Clone)]
pub struct SeededSequence {
    state: u64,
}

impl SeededSequence {
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// The next 64 pseudo-random bits.
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut mixed = self.state;
        mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        mixed ^ (mixed >> 31)
    }

    /// A value in `[0, 1)` from the top 53 bits, like awk's `rand()`.
    pub fn next_unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// A whole number in `[0, bound)`, as awk's `int(rand() * bound)`.
    /// Zero when `bound` is zero.
    pub fn next_below(&mut self, bound: u64) -> u64 {
        (self.next_unit() * bound as f64) as u64
    }
}

#[cfg(test)]
mod tests;
