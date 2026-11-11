// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The pool's weak shares, share by share: the weak ratio `R` as section 3
//! of the design page moves it, the weak shares found between two chain
//! shares, and which of them a share carries before they expire.
//!
//! Only counts are simulated, not miners: weak shares are found at
//! `(R - 1)` times the rate the miners at the minimum find full shares
//! (a hash that meets the weak target but not the share target), during
//! the exponential time between two chain shares, on the current tip. A new
//! share carries up to `WEAK_CAP` of them, oldest parent first, and a weak
//! share whose parent falls more than `WEAK_DEPTH` below the tip expires
//! uncredited. A share from a node that never carries others' weak shares
//! carries only its own node's.

use crate::constants::{FIXED_POINT_ONE, WEAK_RATIO_STEP_DIVISOR};
use crate::sampling::poisson;
use p2poolv2_netsim::random::SimulationRandom;
use std::cmp::Ordering;

/// What the weak share queue depends on.
#[derive(Debug, Clone, Copy)]
pub struct WeakQueueInputs {
    /// Full shares a share period that miners at the minimum on carrying
    /// nodes find.
    pub carrying_minimum_shares_per_period: f64,
    /// The same for miners on nodes that never carry others' weak shares.
    pub non_carrying_minimum_shares_per_period: f64,
    /// Fraction of chain shares that carrying nodes make.
    pub carrying_share_fraction: f64,
    /// `WEAK_CAP`: most weak shares one share carries.
    pub weak_cap: u64,
    /// `WEAK_FILL`: the carried count the weak ratio steers towards.
    pub weak_fill: u64,
    /// `WEAK_DEPTH`: deepest parent a carried weak share may have.
    pub weak_depth: usize,
    /// `R_MAX`.
    pub ratio_max: u64,
    /// The weak ratio at which the weak target reaches the chain's ceiling:
    /// the minimum difficulty over the ceiling difficulty. The weak target
    /// is never easier than the ceiling.
    pub ceiling_ratio: f64,
    /// Chain shares to measure over.
    pub shares: usize,
    /// Chain shares to run first and not measure, while `R` settles from
    /// its genesis value `R_MAX`.
    pub warmup_shares: usize,
}

/// What the queue settles at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WeakQueueOutcome {
    /// Time-weighted mean of the weak ratio in force, ceiling applied.
    pub mean_ratio: f64,
    /// Weak shares found per chain share.
    pub arrivals_per_share: f64,
    /// Weak shares carried per chain share.
    pub carried_per_share: f64,
    /// Weak shares carried per chain share from a carrying node.
    pub carried_per_carrying_share: f64,
    /// Fraction of carrying nodes' weak shares that expire uncarried.
    pub carrying_expired_fraction: f64,
    /// Fraction of non-carrying nodes' weak shares that expire uncarried.
    pub non_carrying_expired_fraction: f64,
    /// Fraction of chain shares that carry `WEAK_CAP` weak shares.
    pub full_share_fraction: f64,
}

impl Default for WeakQueueOutcome {
    /// No weak shares: a ratio of 1 and nothing found, carried or expired.
    fn default() -> Self {
        Self {
            mean_ratio: 1.0,
            arrivals_per_share: 0.0,
            carried_per_share: 0.0,
            carried_per_carrying_share: 0.0,
            carrying_expired_fraction: 0.0,
            non_carrying_expired_fraction: 0.0,
            full_share_fraction: 0.0,
        }
    }
}

/// The weak ratio of a share, in units of 1/1024, from its parent's ratio
/// and the number of weak shares its parent carried (design page,
/// section 3, "The weak target").
pub fn next_weak_ratio(parent_ratio: u64, parent_carried: u64, fill: u64, ratio_max: u64) -> u64 {
    let step = parent_ratio / WEAK_RATIO_STEP_DIVISOR;
    let moved = match parent_carried.cmp(&fill) {
        Ordering::Greater => parent_ratio - step,
        Ordering::Less => parent_ratio + step,
        Ordering::Equal => parent_ratio,
    };
    moved.clamp(FIXED_POINT_ONE, ratio_max * FIXED_POINT_ONE)
}

/// Weak shares waiting to be carried, by the depth of their parent: for the
/// share about to carry them, index `d` holds those whose parent is `d + 1`
/// shares below it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WaitingWeakShares {
    pub(crate) carrying: Vec<u64>,
    pub(crate) non_carrying: Vec<u64>,
}

/// Weak shares one new share carries, by kind.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Carried {
    pub(crate) carrying: u64,
    pub(crate) non_carrying: u64,
}

impl Carried {
    pub(crate) fn total(&self) -> u64 {
        self.carrying + self.non_carrying
    }
}

impl WaitingWeakShares {
    pub(crate) fn new(depth: usize) -> Self {
        Self {
            carrying: vec![0; depth],
            non_carrying: vec![0; depth],
        }
    }

    /// A new share arrives: every waiting weak share is one share deeper,
    /// those already at the deepest expire (returned), and the weak shares
    /// found on the old tip join at depth one.
    pub(crate) fn deepen(&mut self, found_carrying: u64, found_non_carrying: u64) -> (u64, u64) {
        self.carrying.rotate_right(1);
        self.non_carrying.rotate_right(1);
        let expired = (self.carrying[0], self.non_carrying[0]);
        self.carrying[0] = found_carrying;
        self.non_carrying[0] = found_non_carrying;
        expired
    }

    /// A share from a carrying node takes up to `cap` weak shares of both
    /// kinds, oldest parent first, splitting each depth in proportion.
    pub(crate) fn carry_all(&mut self, cap: u64) -> Carried {
        let mut carried = Carried::default();
        (0..self.carrying.len()).rev().for_each(|depth| {
            let room = cap - carried.total();
            let carrying = self.carrying[depth];
            let non_carrying = self.non_carrying[depth];
            let waiting = carrying + non_carrying;
            let taken = room.min(waiting);
            if taken > 0 {
                let carrying_taken = ((taken * carrying + waiting / 2) / waiting)
                    .min(carrying)
                    .max(taken.saturating_sub(non_carrying));
                let non_carrying_taken = taken - carrying_taken;
                self.carrying[depth] -= carrying_taken;
                self.non_carrying[depth] -= non_carrying_taken;
                carried.carrying += carrying_taken;
                carried.non_carrying += non_carrying_taken;
            }
        });
        carried
    }

    /// A share from a non-carrying node takes up to `cap` of its own nodes'
    /// weak shares only, oldest parent first.
    pub(crate) fn carry_own(&mut self, cap: u64) -> Carried {
        let mut carried = Carried::default();
        (0..self.non_carrying.len()).rev().for_each(|depth| {
            let taken = (cap - carried.non_carrying).min(self.non_carrying[depth]);
            self.non_carrying[depth] -= taken;
            carried.non_carrying += taken;
        });
        carried
    }

    /// A share takes what its node carries: everything waiting from a
    /// carrying node, its own nodes' weak shares only from another.
    pub(crate) fn carry(&mut self, on_carrying_node: bool, cap: u64) -> Carried {
        if on_carrying_node {
            self.carry_all(cap)
        } else {
            self.carry_own(cap)
        }
    }

    /// Weak shares found on a parent `index + 1` shares below the share
    /// about to carry them join the queue there.
    pub(crate) fn add_at(&mut self, index: usize, found_carrying: u64, found_non_carrying: u64) {
        self.carrying[index] += found_carrying;
        self.non_carrying[index] += found_non_carrying;
    }

    /// Keep only the weak shares also left waiting in `other`, a copy of
    /// the same queue that another share carried from: both take the oldest
    /// first, so what either took is gone. Returns how many this queue
    /// loses, by kind.
    pub(crate) fn keep_shared_with(&mut self, other: &WaitingWeakShares) -> Carried {
        let mut dropped = Carried::default();
        self.carrying
            .iter_mut()
            .zip(&other.carrying)
            .for_each(|(mine, theirs)| {
                dropped.carrying += mine.saturating_sub(*theirs);
                *mine = (*mine).min(*theirs);
            });
        self.non_carrying
            .iter_mut()
            .zip(&other.non_carrying)
            .for_each(|(mine, theirs)| {
                dropped.non_carrying += mine.saturating_sub(*theirs);
                *mine = (*mine).min(*theirs);
            });
        dropped
    }
}

/// Running sums over the measured shares.
#[derive(Default)]
struct Tally {
    shares: u64,
    carrying_shares: u64,
    full_shares: u64,
    time: f64,
    ratio_time: f64,
    found_carrying: u64,
    found_non_carrying: u64,
    carried: u64,
    carried_by_carrying: u64,
    expired_carrying: u64,
    expired_non_carrying: u64,
}

/// Run the weak share queue and report what it settles at.
pub fn simulate_weak_queue(
    inputs: &WeakQueueInputs,
    random: &mut SimulationRandom,
) -> WeakQueueOutcome {
    let mut waiting = WaitingWeakShares::new(inputs.weak_depth);
    let mut tip_ratio = inputs.ratio_max * FIXED_POINT_ONE;
    let mut tip_carried = 0;
    let mut tally = Tally::default();
    let total_shares = inputs.warmup_shares + inputs.shares;
    let mut share = 0;
    while share < total_shares {
        //* Find weak shares on the tip until the next chain share.
        let interval = random.next_exponential(1.0);
        let ratio = (tip_ratio as f64 / FIXED_POINT_ONE as f64)
            .min(inputs.ceiling_ratio)
            .max(1.0);
        let weak_per_period = ratio - 1.0;
        let found_carrying = poisson(
            random,
            weak_per_period * inputs.carrying_minimum_shares_per_period * interval,
        );
        let found_non_carrying = poisson(
            random,
            weak_per_period * inputs.non_carrying_minimum_shares_per_period * interval,
        );
        //* The next share takes its ratio from the tip and carries what it may.
        let child_ratio =
            next_weak_ratio(tip_ratio, tip_carried, inputs.weak_fill, inputs.ratio_max);
        let (expired_carrying, expired_non_carrying) =
            waiting.deepen(found_carrying, found_non_carrying);
        let from_carrying_node = random.next_bool(inputs.carrying_share_fraction);
        let carried = if from_carrying_node {
            waiting.carry_all(inputs.weak_cap)
        } else {
            waiting.carry_own(inputs.weak_cap)
        };
        tip_ratio = child_ratio;
        tip_carried = carried.total();
        //* Count it once the ratio has settled.
        if share >= inputs.warmup_shares {
            tally.shares += 1;
            tally.time += interval;
            tally.ratio_time += ratio * interval;
            tally.found_carrying += found_carrying;
            tally.found_non_carrying += found_non_carrying;
            tally.carried += carried.total();
            tally.expired_carrying += expired_carrying;
            tally.expired_non_carrying += expired_non_carrying;
            if from_carrying_node {
                tally.carrying_shares += 1;
                tally.carried_by_carrying += carried.total();
            }
            if carried.total() >= inputs.weak_cap {
                tally.full_shares += 1;
            }
        }
        share += 1;
    }
    summarise(&tally)
}

/// `numerator / denominator`, zero when the denominator is.
fn ratio_or_zero(numerator: f64, denominator: f64) -> f64 {
    if denominator > 0.0 {
        numerator / denominator
    } else {
        0.0
    }
}

fn summarise(tally: &Tally) -> WeakQueueOutcome {
    let shares = tally.shares as f64;
    WeakQueueOutcome {
        mean_ratio: ratio_or_zero(tally.ratio_time, tally.time).max(1.0),
        arrivals_per_share: ratio_or_zero(
            (tally.found_carrying + tally.found_non_carrying) as f64,
            shares,
        ),
        carried_per_share: ratio_or_zero(tally.carried as f64, shares),
        carried_per_carrying_share: ratio_or_zero(
            tally.carried_by_carrying as f64,
            tally.carrying_shares as f64,
        ),
        carrying_expired_fraction: ratio_or_zero(
            tally.expired_carrying as f64,
            tally.found_carrying as f64,
        ),
        non_carrying_expired_fraction: ratio_or_zero(
            tally.expired_non_carrying as f64,
            tally.found_non_carrying as f64,
        ),
        full_share_fraction: ratio_or_zero(tally.full_shares as f64, shares),
    }
}

#[cfg(test)]
mod tests;
