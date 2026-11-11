// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The tracked miners' payouts, block by block.
//!
//! Each tracked miner's shares, uncles and credited weak shares are Poisson
//! processes, simulated event by event. The pool's blocks are a Poisson
//! process too. A block pays each tracked miner its credited work in the
//! window, which ends `skip` seconds before the block and spans `span`
//! seconds, over the window's whole work: the rest of the pool's expected
//! work in that span plus the tracked miners' actual work. The rest of the
//! pool is many miners, so its work in a span is taken at its expectation;
//! the noise in a tracked miner's payments is its own luck and the blocks'.

use crate::constants::{BLOCK_REWARD_SATOSHIS, DONATION_FRACTION, MONTH_SECONDS};
use crate::levels::{draw_level_group, level_amount};
use crate::sampling::poisson_times;
use crate::window::WindowGeometry;
use p2poolv2_netsim::random::SimulationRandom;

/// What one tracked miner's entries look like.
#[derive(Debug, Clone, Copy)]
pub struct TrackedInputs {
    /// Shares a second it finds, stale ones included.
    pub found_rate: f64,
    /// Chance a found share goes stale and is paid as an uncle.
    pub stale_rate: f64,
    /// Credited hashes of a chain share, its expected nephew and includer
    /// bonus included.
    pub chain_weight: f64,
    /// Credited hashes of one of its uncles.
    pub uncle_weight: f64,
    /// Credited weak shares a second (those that do not expire).
    pub weak_rate: f64,
    /// Credited hashes of one weak share, after the includer's cut.
    pub weak_weight: f64,
    /// Its payout level `k`.
    pub level: u32,
}

/// What a chain's blocks and window look like to the tracked miners on it.
#[derive(Debug, Clone)]
pub struct ChainPayoutInputs {
    /// The chain's Bitcoin Cash blocks a second.
    pub block_rate: f64,
    /// The window in steady state.
    pub window: WindowGeometry,
    /// Credited hashes a second of everyone on the chain but the tracked
    /// miners.
    pub others_credited_rate: f64,
    /// The tracked miners on this chain.
    pub tracked: Vec<TrackedInputs>,
    /// Satoshis the other payees below each level put into its group in a
    /// block, by level; `None` when the design has no payout levels.
    pub level_pots: Option<Vec<u64>>,
    /// Months to simulate.
    pub months: usize,
}

/// One tracked miner's simulated payouts.
#[derive(Debug, Clone, Default)]
pub struct TrackedOutcome {
    /// Satoshis received in each month.
    pub monthly_income: Vec<f64>,
    /// Payments (blocks paying it an output) in each month.
    pub monthly_payments: Vec<u64>,
    /// Every payment's satoshis.
    pub payments: Vec<u64>,
    /// Shares it found during the simulated months, stale ones included.
    pub found_shares: u64,
    /// Credited weak shares during the simulated months.
    pub weak_shares: u64,
}

/// Event times with the running total of their weights, for summing a
/// window with two binary searches.
struct WeightedEvents {
    times: Vec<f64>,
    /// `cumulative[i]` is the weight of events `0 .. i`.
    cumulative: Vec<f64>,
}

impl WeightedEvents {
    fn new(times: Vec<f64>, weight_of: impl FnMut(usize) -> f64) -> Self {
        let mut cumulative = Vec::with_capacity(times.len() + 1);
        cumulative.push(0.0);
        let mut weight_of = weight_of;
        let mut running = 0.0;
        (0..times.len()).for_each(|index| {
            running += weight_of(index);
            cumulative.push(running);
        });
        Self { times, cumulative }
    }

    /// Weight of the events in `[start, end)`.
    fn weight_between(&self, start: f64, end: f64) -> f64 {
        let first = self.times.partition_point(|time| *time < start);
        let last = self.times.partition_point(|time| *time < end);
        self.cumulative[last] - self.cumulative[first]
    }

    /// Number of events in `[start, end)`.
    fn count_between(&self, start: f64, end: f64) -> u64 {
        let first = self.times.partition_point(|time| *time < start);
        let last = self.times.partition_point(|time| *time < end);
        (last - first) as u64
    }
}

/// Expected entries of one kind in one window above which a tracked miner's
/// entries of that kind are not simulated one by one but credited at their
/// expected work: its luck in a window is then below 1/sqrt(2000), about 2%,
/// and simulating each would take gigabytes for a miner alone on a chain.
const EVENT_LIMIT_PER_WINDOW: f64 = 2_000.0;

/// A tracked miner's entries of one kind: simulated one by one, or credited
/// at their expected rate when a window holds very many.
enum EntryStream {
    Events(WeightedEvents),
    Expected { rate: f64, weight_rate: f64 },
}

impl EntryStream {
    /// Entries at `rate` a second between `start` and `end`, each weighing
    /// what `weight_of` draws, `mean_weight` on average.
    fn simulate(
        rate: f64,
        mean_weight: f64,
        window_seconds: f64,
        span: (f64, f64),
        random: &mut SimulationRandom,
        weight_of: impl FnMut(&mut SimulationRandom) -> f64,
    ) -> Self {
        if rate * window_seconds > EVENT_LIMIT_PER_WINDOW {
            EntryStream::Expected {
                rate,
                weight_rate: rate * mean_weight,
            }
        } else {
            let times = poisson_times(random, rate, span.0, span.1);
            let mut weight_of = weight_of;
            EntryStream::Events(WeightedEvents::new(times, |_| weight_of(random)))
        }
    }

    fn weight_between(&self, start: f64, end: f64) -> f64 {
        match self {
            EntryStream::Events(events) => events.weight_between(start, end),
            EntryStream::Expected { weight_rate, .. } => weight_rate * (end - start),
        }
    }

    fn count_between(&self, start: f64, end: f64) -> u64 {
        match self {
            EntryStream::Events(events) => events.count_between(start, end),
            EntryStream::Expected { rate, .. } => (rate * (end - start)).round() as u64,
        }
    }
}

/// One tracked miner's entries over the simulated time and before it.
struct TrackedEntries {
    shares: EntryStream,
    weak: EntryStream,
}

impl TrackedEntries {
    fn simulate(
        inputs: &TrackedInputs,
        window_seconds: f64,
        span: (f64, f64),
        random: &mut SimulationRandom,
    ) -> Self {
        let mean_share_weight = (1.0 - inputs.stale_rate) * inputs.chain_weight
            + inputs.stale_rate * inputs.uncle_weight;
        let shares = EntryStream::simulate(
            inputs.found_rate,
            mean_share_weight,
            window_seconds,
            span,
            random,
            |random| {
                if random.next_bool(inputs.stale_rate) {
                    inputs.uncle_weight
                } else {
                    inputs.chain_weight
                }
            },
        );
        let weak = EntryStream::simulate(
            inputs.weak_rate,
            inputs.weak_weight,
            window_seconds,
            span,
            random,
            |_| inputs.weak_weight,
        );
        Self { shares, weak }
    }

    fn weight_between(&self, start: f64, end: f64) -> f64 {
        self.shares.weight_between(start, end) + self.weak.weight_between(start, end)
    }
}

/// Simulate the tracked miners on one chain for `inputs.months` months.
pub fn simulate_chain_payouts(
    inputs: &ChainPayoutInputs,
    random: &mut SimulationRandom,
) -> Vec<TrackedOutcome> {
    let horizon = inputs.months as f64 * MONTH_SECONDS;
    let warmup = inputs.window.span_seconds + inputs.window.skip_seconds;
    let entries: Vec<TrackedEntries> = inputs
        .tracked
        .iter()
        .map(|tracked| {
            TrackedEntries::simulate(
                tracked,
                inputs.window.span_seconds,
                (-warmup, horizon),
                random,
            )
        })
        .collect();
    let mut outcomes: Vec<TrackedOutcome> = entries
        .iter()
        .map(|entry| TrackedOutcome {
            monthly_income: vec![0.0; inputs.months],
            monthly_payments: vec![0; inputs.months],
            payments: Vec::with_capacity(64),
            found_shares: entry.shares.count_between(0.0, horizon),
            weak_shares: entry.weak.count_between(0.0, horizon),
        })
        .collect();
    let blocks = poisson_times(random, inputs.block_rate, 0.0, horizon);
    let others_work = inputs.others_credited_rate * inputs.window.span_seconds;
    let reward = BLOCK_REWARD_SATOSHIS * (1.0 - DONATION_FRACTION);
    blocks.iter().for_each(|block_time| {
        let end = block_time - inputs.window.skip_seconds;
        let start = end - inputs.window.span_seconds;
        let weights: Vec<f64> = entries
            .iter()
            .map(|entry| entry.weight_between(start, end))
            .collect();
        let window_work = others_work + weights.iter().sum::<f64>();
        let amounts: Vec<u64> = weights
            .iter()
            .map(|weight| {
                if window_work > 0.0 {
                    (reward * (weight / window_work)).floor() as u64
                } else {
                    0
                }
            })
            .collect();
        let paid = match &inputs.level_pots {
            Some(pots) => apply_levels(&amounts, &inputs.tracked, pots, random),
            None => amounts,
        };
        let month = ((block_time / MONTH_SECONDS) as usize).min(inputs.months - 1);
        outcomes
            .iter_mut()
            .zip(&paid)
            .filter(|(_, amount)| **amount > 0)
            .for_each(|(outcome, amount)| {
                outcome.monthly_income[month] += *amount as f64;
                outcome.monthly_payments[month] += 1;
                outcome.payments.push(*amount);
            });
    });
    outcomes
}

/// Run the draw for every level group holding a tracked miner below its
/// level, returning what each tracked miner is paid in this block.
///
/// The other payees of a group are represented by the satoshis before and
/// between the tracked miners in the group's order. The draw only sees
/// running totals, so a tracked miner's outcome is exactly what it would be
/// among the real payees with the same total in front of it. Each tracked
/// miner's place in the order (its locking script) is uniform.
fn apply_levels(
    amounts: &[u64],
    tracked: &[TrackedInputs],
    pots: &[u64],
    random: &mut SimulationRandom,
) -> Vec<u64> {
    let mut paid = amounts.to_vec();
    (0..pots.len()).for_each(|level| {
        let threshold = level_amount(level as u32);
        let mut members: Vec<(u64, usize)> = amounts
            .iter()
            .zip(tracked)
            .enumerate()
            .filter(|(_, (amount, inputs))| {
                inputs.level as usize == level && **amount > 0 && **amount < threshold
            })
            .map(|(index, _)| (random.next_below(pots[level] + 1), index))
            .collect();
        if !members.is_empty() {
            members.sort_unstable();
            let mut group = Vec::with_capacity(2 * members.len() + 1);
            let mut placed = 0;
            members.iter().for_each(|(position, index)| {
                group.push(position - placed);
                group.push(amounts[*index]);
                placed = *position;
            });
            group.push(pots[level] - placed);
            let winnings = draw_level_group(&group, threshold, random.next_u64());
            members.iter().enumerate().for_each(|(member, (_, index))| {
                paid[*index] = winnings[2 * member + 1];
            });
        }
    });
    paid
}

#[cfg(test)]
mod tests;
