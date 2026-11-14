// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Weak shares whose number follows what the miners at the minimum ask for:
//! the proposal of section 6 of the design page
//! (`docs/sharechain/small-miners-design.md`), not a rule of the chain.
//!
//! Today the weak ratio `R` steers the weak shares a share carries towards
//! `WEAK_FILL`, whatever the pool's size. Under the proposal a weak share
//! asks for an easier weak target when its node wanted one for its miner,
//! and while more than one in `WEAK_ASK_DIVISOR` of the weak shares a share
//! carried ask, `R` rises instead, up to `WEAK_FILL_MAX` weak shares a
//! share. Without asks `R` moves as it does today.
//!
//! As in [`crate::weak`], only counts are simulated. A class of miners asks
//! while the weak ratio in force is below the ratio at which one of its
//! miners finds the node default's weak shares a month, so the asking part
//! of the weak shares found follows the ratio share by share.

use crate::constants::{
    ASK_WEAK_SHARES_PER_MONTH, FIXED_POINT_ONE, WEAK_ASK_DIVISOR, WEAK_FILL, WEAK_FILL_MAX,
    WEAK_RATIO_ASK_MAX, WEAK_RATIO_MAX, WEAK_RATIO_STEP_DIVISOR,
};
use crate::sampling::poisson;
use crate::weak::{WaitingWeakShares, WeakQueueOutcome};
use p2poolv2_netsim::random::SimulationRandom;

/// The proposed rule's constants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AskRule {
    /// `WEAK_FILL`: the weak shares a share the ratio steers towards without
    /// asks, today's rule.
    pub weak_fill: u64,
    /// `WEAK_FILL_MAX`: the most weak shares a share asks may steer towards.
    pub weak_fill_max: u64,
    /// `WEAK_ASK_DIVISOR`: a share was asked when more than one in this many
    /// of the weak shares it carried ask.
    pub ask_divisor: u64,
    /// `R_MAX`: the largest ratio without asks, today's rule.
    pub ratio_max: u64,
    /// `R_ASK_MAX`: the largest ratio asks may reach.
    pub ratio_ask_max: u64,
}

impl AskRule {
    /// The most weak shares one share may carry: twice `WEAK_FILL_MAX`, as
    /// `WEAK_CAP` is twice `WEAK_FILL` today.
    pub fn weak_cap(&self) -> u64 {
        2 * self.weak_fill_max
    }
}

impl Default for AskRule {
    /// The rule with the constants section 6 of the design page proposes.
    fn default() -> Self {
        Self {
            weak_fill: WEAK_FILL,
            weak_fill_max: WEAK_FILL_MAX,
            ask_divisor: WEAK_ASK_DIVISOR,
            ratio_max: WEAK_RATIO_MAX,
            ratio_ask_max: WEAK_RATIO_ASK_MAX,
        }
    }
}

/// The proposal as a scenario runs it: the rule, the node default that
/// asks, and the ways a pool's hashrate may depart from that default.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WeakAsks {
    /// The rule.
    pub rule: AskRule,
    /// Node default: a miner's node asks while the miner expects fewer than
    /// this many weak shares a month.
    pub weak_shares_per_month: f64,
    /// Part of the hashrate at the minimum whose weak shares ask whatever
    /// their miners find.
    pub always_asking: f64,
    /// Part of the hashrate at the minimum whose weak shares never ask.
    pub never_asking: f64,
    /// Fraction of chain shares from nodes that carry no weak share that
    /// asks.
    pub censoring_shares: f64,
}

impl Default for WeakAsks {
    /// The proposed rule and node default, every node following it.
    fn default() -> Self {
        Self {
            rule: AskRule::default(),
            weak_shares_per_month: ASK_WEAK_SHARES_PER_MONTH,
            always_asking: 0.0,
            never_asking: 0.0,
            censoring_shares: 0.0,
        }
    }
}

/// The weak ratio of a share under the proposal, in units of 1/1024, from
/// its parent's ratio, the weak shares its parent carried and how many of
/// those asked.
///
/// Asked, the ratio rises one step, up to `R_ASK_MAX`. Not asked, it moves
/// as today: down above `WEAK_FILL`, up below it as far as `R_MAX`. A ratio
/// that asks left above `R_MAX` comes back down once nobody asks.
pub fn next_weak_ratio_with_asks(
    parent_ratio: u64,
    parent_carried: u64,
    parent_asking: u64,
    rule: &AskRule,
) -> u64 {
    let step = parent_ratio / WEAK_RATIO_STEP_DIVISOR;
    let idle_max = rule.ratio_max * FIXED_POINT_ONE;
    let asked =
        parent_carried <= rule.weak_fill_max && parent_asking * rule.ask_divisor > parent_carried;
    let moved = if asked {
        (parent_ratio + step).min(rule.ratio_ask_max * FIXED_POINT_ONE)
    } else if parent_carried > rule.weak_fill || parent_ratio > idle_max {
        parent_ratio - step
    } else if parent_carried < rule.weak_fill {
        (parent_ratio + step).min(idle_max)
    } else {
        parent_ratio
    };
    moved.max(FIXED_POINT_ONE)
}

/// One class of miners at the minimum, as the ask queue sees it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AskClass {
    /// Full shares a share period the class finds.
    pub minimum_shares_per_period: f64,
    /// The class's weak shares ask while the weak ratio in force is below
    /// this: infinity for a class that always asks, zero for one that never
    /// does.
    pub asks_below_ratio: f64,
}

/// What the ask queue depends on.
#[derive(Debug, Clone)]
pub struct AskQueueInputs {
    /// The classes at the minimum.
    pub classes: Vec<AskClass>,
    /// The rule.
    pub rule: AskRule,
    /// `WEAK_DEPTH`.
    pub weak_depth: usize,
    /// The weak ratio at which the weak target reaches the chain's ceiling.
    /// No weak share asks at or above it: nothing is easier than the ceiling.
    pub ceiling_ratio: f64,
    /// Fraction of chain shares from nodes that carry no weak share that
    /// asks.
    pub censoring_share_fraction: f64,
    /// Chain shares to measure over.
    pub shares: usize,
    /// Chain shares to run first and not measure, while the ratio settles
    /// from `R_MAX`.
    pub warmup_shares: usize,
}

/// Running sums over the measured shares.
#[derive(Default)]
struct Tally {
    shares: u64,
    full_shares: u64,
    time: f64,
    ratio_time: f64,
    found: u64,
    carried: u64,
    carried_asking: u64,
    expired: u64,
}

/// `numerator / denominator`, zero when the denominator is.
fn ratio_or_zero(numerator: f64, denominator: f64) -> f64 {
    if denominator > 0.0 {
        numerator / denominator
    } else {
        0.0
    }
}

/// Run the weak share queue under the proposed rule and report what it
/// settles at. The queue's two kinds stand for the weak shares that ask and
/// those that do not; every node carries both, except a censoring node,
/// which carries only those that do not ask.
pub fn simulate_ask_queue(
    inputs: &AskQueueInputs,
    random: &mut SimulationRandom,
) -> WeakQueueOutcome {
    let cap = inputs.rule.weak_cap();
    let mut waiting = WaitingWeakShares::new(inputs.weak_depth);
    let mut tip_ratio = inputs.rule.ratio_max * FIXED_POINT_ONE;
    let mut tip_carried = 0;
    let mut tip_asking = 0;
    let mut tally = Tally::default();
    (0..inputs.warmup_shares + inputs.shares).for_each(|share| {
        //* Find weak shares on the tip until the next chain share.
        let interval = random.next_exponential(1.0);
        let ratio = (tip_ratio as f64 / FIXED_POINT_ONE as f64)
            .min(inputs.ceiling_ratio)
            .max(1.0);
        let (found_asking, found_silent) =
            inputs
                .classes
                .iter()
                .fold((0, 0), |(asking, silent), class| {
                    let found = poisson(
                        random,
                        (ratio - 1.0) * class.minimum_shares_per_period * interval,
                    );
                    if ratio < class.asks_below_ratio.min(inputs.ceiling_ratio) {
                        (asking + found, silent)
                    } else {
                        (asking, silent + found)
                    }
                });
        //* The next share takes its ratio from the tip and carries what it may.
        let child_ratio =
            next_weak_ratio_with_asks(tip_ratio, tip_carried, tip_asking, &inputs.rule);
        let (expired_asking, expired_silent) = waiting.deepen(found_asking, found_silent);
        let carried = if random.next_bool(inputs.censoring_share_fraction) {
            waiting.carry_own(cap)
        } else {
            waiting.carry_all(cap)
        };
        tip_ratio = child_ratio;
        tip_carried = carried.total();
        tip_asking = carried.carrying;
        //* Count it once the ratio has settled.
        if share >= inputs.warmup_shares {
            tally.shares += 1;
            tally.time += interval;
            tally.ratio_time += ratio * interval;
            tally.found += found_asking + found_silent;
            tally.carried += carried.total();
            tally.carried_asking += carried.carrying;
            tally.expired += expired_asking + expired_silent;
            if carried.total() >= cap {
                tally.full_shares += 1;
            }
        }
    });
    let shares = tally.shares as f64;
    let carried_per_share = ratio_or_zero(tally.carried as f64, shares);
    WeakQueueOutcome {
        mean_ratio: ratio_or_zero(tally.ratio_time, tally.time).max(1.0),
        arrivals_per_share: ratio_or_zero(tally.found as f64, shares),
        carried_per_share,
        carried_per_carrying_share: carried_per_share,
        carrying_expired_fraction: ratio_or_zero(tally.expired as f64, tally.found as f64),
        non_carrying_expired_fraction: 0.0,
        full_share_fraction: ratio_or_zero(tally.full_shares as f64, shares),
        asking_fraction: ratio_or_zero(tally.carried_asking as f64, tally.carried as f64),
    }
}

#[cfg(test)]
mod tests;
