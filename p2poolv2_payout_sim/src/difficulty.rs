// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Steady-state share difficulties.
//!
//! ASERT moves the minimum difficulty until the chain grows by one share per
//! share period. In steady state that is an equation: the chain shares all
//! miners make together at their own difficulties equal one per period. A
//! share that goes stale does not grow the chain, so only `1 - stale` of
//! the shares found count. Each design gives each miner its difficulty as a
//! function of the minimum; this module solves the equation for it.

use crate::constants::{
    BAND_CAP_STEP_DIVISOR, BCH_BLOCKS_PER_MONTH, HASHES_PER_DIFFICULTY, MONTH_SECONDS,
    NODE_DEFAULT_SHARES_PER_MONTH, NODE_DEFAULT_SHARES_PER_POOL_BLOCK, PPLNS_WINDOW_SHARES,
    VARDIFF_PRESENCE_SHARES,
};
use crate::miners::MinerClass;

/// Bisection steps in log space; 200 halvings of a range of 2^100 leave a
/// relative error far below a double's precision.
const BISECTION_STEPS: usize = 200;

/// What every design shares about one chain.
#[derive(Debug, Clone, Copy)]
pub struct ChainParameters {
    /// Target seconds between chain shares (`IDEAL_BLOCK_TIME`).
    pub period_seconds: f64,
    /// Share of found shares that go stale (become uncles).
    pub stale_rate: f64,
    /// The chain's easiest share difficulty.
    pub ceiling_difficulty: f64,
    /// Bitcoin Cash's hashrate, which sets the pool's block rate.
    pub network_hashrate: f64,
}

/// How a design sets each miner's share difficulty.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DifficultyRule {
    /// Every share at the minimum (today's rule, one chain or main and mini).
    Uniform,
    /// The difficulty band with a fixed cap `C`: the node default picks each
    /// miner's difficulty between the minimum and `C` times it.
    Band {
        /// The band cap `C`.
        cap: f64,
    },
    /// Cash Dragon's vardiff chain: each miner at its hashrate share times
    /// the ASERT difficulty, above a floor derived from the window.
    Vardiff,
}

/// The steady state of one chain.
#[derive(Debug, Clone)]
pub struct ChainDifficulty {
    /// The ASERT difficulty: the share difficulty under the uniform rule,
    /// the band's minimum, or Cash Dragon's `D_asert`.
    pub asert_difficulty: f64,
    /// Difficulty each class mines at, in class order.
    pub class_difficulty: Vec<f64>,
    /// Whether each class mines at exactly the ASERT minimum, the only jobs
    /// that make weak shares.
    pub class_at_minimum: Vec<bool>,
    /// Chain shares a second, one per period unless the ceiling binds.
    pub chain_share_rate: f64,
    /// Cash Dragon's floor where it binds above the ceiling, else zero.
    pub vardiff_floor: f64,
}

impl ChainDifficulty {
    /// The easiest share difficulty any class mines at.
    pub fn easiest_difficulty(&self) -> f64 {
        self.class_difficulty
            .iter()
            .copied()
            .fold(f64::INFINITY, f64::min)
    }
}

/// Shares a second one miner of `class` finds at `difficulty`, stale ones
/// included.
pub fn found_share_rate(class: &MinerClass, difficulty: f64) -> f64 {
    class.hashrate / (difficulty * HASHES_PER_DIFFICULTY)
}

/// The whole hashrate of `classes`.
pub fn chain_hashrate(classes: &[MinerClass]) -> f64 {
    classes.iter().map(MinerClass::total_hashrate).sum()
}

/// Chain shares a second when each class mines at its difficulty.
fn chain_share_rate(classes: &[MinerClass], difficulties: &[f64], stale_rate: f64) -> f64 {
    let found: f64 = classes
        .iter()
        .zip(difficulties)
        .map(|(class, difficulty)| class.count * found_share_rate(class, *difficulty))
        .sum();
    found * (1.0 - stale_rate)
}

/// The value at or above `low` where the decreasing `function` falls to
/// `target`, found by bisection in log space; `low` itself when `function`
/// is already at or below `target` there.
fn solve_decreasing(function: impl Fn(f64) -> f64, target: f64, low: f64) -> f64 {
    if function(low) <= target {
        low
    } else {
        let mut lower = low;
        let mut upper = low * 2.0;
        while function(upper) > target {
            lower = upper;
            upper *= 2.0;
        }
        let mut step = 0;
        while step < BISECTION_STEPS {
            let middle = (lower * upper).sqrt();
            if function(middle) > target {
                lower = middle;
            } else {
                upper = middle;
            }
            step += 1;
        }
        (lower * upper).sqrt()
    }
}

/// Solve one chain's steady state under `rule`.
pub fn solve_chain(
    classes: &[MinerClass],
    parameters: &ChainParameters,
    rule: DifficultyRule,
) -> ChainDifficulty {
    match rule {
        DifficultyRule::Uniform => solve_uniform(classes, parameters),
        DifficultyRule::Band { cap } => solve_band(classes, parameters, cap),
        DifficultyRule::Vardiff => solve_vardiff(classes, parameters),
    }
}

/// Every share at one difficulty: the chain's hashrate over one share per
/// period, never easier than the ceiling.
fn solve_uniform(classes: &[MinerClass], parameters: &ChainParameters) -> ChainDifficulty {
    let hashrate = chain_hashrate(classes);
    let difficulty = (hashrate * (1.0 - parameters.stale_rate) * parameters.period_seconds
        / HASHES_PER_DIFFICULTY)
        .max(parameters.ceiling_difficulty);
    let class_difficulty = vec![difficulty; classes.len()];
    let chain_share_rate = chain_share_rate(classes, &class_difficulty, parameters.stale_rate);
    ChainDifficulty {
        asert_difficulty: difficulty,
        class_difficulty,
        class_at_minimum: vec![true; classes.len()],
        chain_share_rate,
        vardiff_floor: 0.0,
    }
}

/// Shares a month the node default aims a heavy miner at:
/// `max(4 shares per pool block, 300 shares a month)`, the pool's block rate
/// from the chain's part of Bitcoin Cash's hashrate.
pub fn node_default_shares_per_month(chain_hashrate: f64, network_hashrate: f64) -> f64 {
    let pool_blocks_per_month = chain_hashrate / network_hashrate * BCH_BLOCKS_PER_MONTH;
    (NODE_DEFAULT_SHARES_PER_POOL_BLOCK * pool_blocks_per_month).max(NODE_DEFAULT_SHARES_PER_MONTH)
}

/// How much harder than the minimum the node default would make a miner's
/// shares without the cap: its shares a month at the minimum over the
/// default's target, or 1 for a miner that ignores the default.
pub fn wanted_ratio(class: &MinerClass, minimum_difficulty: f64, target_per_month: f64) -> f64 {
    if class.follows_node_default {
        let shares_per_month = found_share_rate(class, minimum_difficulty) * MONTH_SECONDS;
        (shares_per_month / target_per_month).max(1.0)
    } else {
        1.0
    }
}

/// The band at a fixed cap: each miner at `min(C, max(1, ratio))` times the
/// minimum, the minimum solved so the chain makes one share per period.
fn solve_band(classes: &[MinerClass], parameters: &ChainParameters, cap: f64) -> ChainDifficulty {
    let target =
        node_default_shares_per_month(chain_hashrate(classes), parameters.network_hashrate);
    let difficulties_at = |minimum: f64| -> Vec<f64> {
        classes
            .iter()
            .map(|class| minimum * wanted_ratio(class, minimum, target).min(cap))
            .collect()
    };
    let rate_at = |minimum: f64| -> f64 {
        chain_share_rate(classes, &difficulties_at(minimum), parameters.stale_rate)
    };
    let minimum = solve_decreasing(
        rate_at,
        1.0 / parameters.period_seconds,
        parameters.ceiling_difficulty,
    );
    let class_difficulty = difficulties_at(minimum);
    let class_at_minimum = class_difficulty
        .iter()
        .map(|difficulty| *difficulty <= minimum)
        .collect();
    let chain_share_rate = chain_share_rate(classes, &class_difficulty, parameters.stale_rate);
    ChainDifficulty {
        asert_difficulty: minimum,
        class_difficulty,
        class_at_minimum,
        chain_share_rate,
        vardiff_floor: 0.0,
    }
}

/// Cash Dragon's tiers, `max(ceiling, floor, w_i * D_asert)` with `w_i` the
/// miner's share of the chain's hashrate.
fn vardiff_tiers(
    classes: &[MinerClass],
    hashrate: f64,
    asert: f64,
    floor: f64,
    ceiling: f64,
) -> Vec<f64> {
    classes
        .iter()
        .map(|class| (class.hashrate / hashrate * asert).max(floor).max(ceiling))
        .collect()
}

/// Cash Dragon's vardiff chain in steady state.
///
/// With every miner at its fair tier `w_i * D_asert` every miner makes
/// shares at the same rate, so ASERT settles `D_asert` at the number of
/// miners times the uniform difficulty. The floor is the smallest present
/// miner's own tier and binds for nobody, unless miners are so many that the
/// smallest holds fewer than the presence minimum `N` of the 120,960 entries
/// of the floor window. Then the floor is set by a small miner that happens
/// to hold exactly `N` entries: `floor = N * floor / W_total * D_asert`,
/// which holds only where `D_asert = W_total / N`, the floor window's work
/// over `N`. The floor itself then settles where the chain makes one share
/// per period.
fn solve_vardiff(classes: &[MinerClass], parameters: &ChainParameters) -> ChainDifficulty {
    let hashrate = chain_hashrate(classes);
    let ceiling = parameters.ceiling_difficulty;
    let target_rate = 1.0 / parameters.period_seconds;
    let largest_weight = classes
        .iter()
        .map(|class| class.hashrate / hashrate)
        .fold(0.0, f64::max);
    let rate_at_asert = |asert: f64| -> f64 {
        let tiers = vardiff_tiers(classes, hashrate, asert, 0.0, ceiling);
        chain_share_rate(classes, &tiers, parameters.stale_rate)
    };
    let fair_asert = solve_decreasing(rate_at_asert, target_rate, ceiling / largest_weight);
    let fair_tiers = vardiff_tiers(classes, hashrate, fair_asert, 0.0, ceiling);
    let fair_rate = chain_share_rate(classes, &fair_tiers, parameters.stale_rate);
    let smallest_entries =
        smallest_miner_window_entries(classes, &fair_tiers, parameters, fair_rate);
    let (asert, floor) = if smallest_entries >= VARDIFF_PRESENCE_SHARES {
        (fair_asert, 0.0)
    } else {
        let window_seconds = PPLNS_WINDOW_SHARES as f64 / target_rate;
        let window_work = hashrate * window_seconds / HASHES_PER_DIFFICULTY;
        let presence_asert = window_work / VARDIFF_PRESENCE_SHARES;
        let rate_at_floor = |floor: f64| -> f64 {
            let tiers = vardiff_tiers(classes, hashrate, presence_asert, floor, ceiling);
            chain_share_rate(classes, &tiers, parameters.stale_rate)
        };
        let floor = solve_decreasing(rate_at_floor, target_rate, ceiling);
        (presence_asert, floor)
    };
    let class_difficulty = vardiff_tiers(classes, hashrate, asert, floor, ceiling);
    let chain_share_rate = chain_share_rate(classes, &class_difficulty, parameters.stale_rate);
    ChainDifficulty {
        asert_difficulty: asert,
        class_at_minimum: vec![false; classes.len()],
        class_difficulty,
        chain_share_rate,
        vardiff_floor: floor,
    }
}

/// Expected entries one miner of the smallest class holds in Cash Dragon's
/// fixed-count floor window of 120,960 shares.
fn smallest_miner_window_entries(
    classes: &[MinerClass],
    tiers: &[f64],
    parameters: &ChainParameters,
    chain_rate: f64,
) -> f64 {
    classes
        .iter()
        .zip(tiers)
        .filter(|(class, _)| class.count > 0.0)
        .min_by(|(first, _), (second, _)| first.hashrate.total_cmp(&second.hashrate))
        .map(|(class, tier)| {
            let miner_rate = found_share_rate(class, *tier) * (1.0 - parameters.stale_rate);
            PPLNS_WINDOW_SHARES as f64 * miner_rate / chain_rate
        })
        .unwrap_or(f64::INFINITY)
}

/// Where the ABLA-like band cap rule of the design page settles, scanning
/// `C` upwards from 1 in steps of `1/512` until its expected step per share
/// is no longer upwards, or `cap_limit`.
///
/// A share pushes `C` up by `C * u / 512` when its miner wanted more than
/// the cap allowed (`u` = its share's work over the minimum, an integer,
/// `floor(C)` for a capped share), and every other share pulls it down by
/// `C / 512`. With `p` the fraction of chain shares from capped miners the
/// expected step is `C / 512 * (p * floor(C) - (1 - p))`.
pub fn settled_band_cap(
    classes: &[MinerClass],
    parameters: &ChainParameters,
    cap_limit: f64,
) -> f64 {
    let step = 1.0 + 1.0 / BAND_CAP_STEP_DIVISOR;
    let mut cap = 1.0;
    while cap < cap_limit && band_cap_drift(classes, parameters, cap) > 0.0 {
        cap = (cap * step).min(cap_limit);
    }
    cap
}

/// The sign of the band cap's expected step per share at cap `cap`:
/// `p * floor(C) - (1 - p)`.
pub fn band_cap_drift(classes: &[MinerClass], parameters: &ChainParameters, cap: f64) -> f64 {
    let solution = solve_band(classes, parameters, cap);
    let target =
        node_default_shares_per_month(chain_hashrate(classes), parameters.network_hashrate);
    let rates: Vec<f64> = classes
        .iter()
        .zip(&solution.class_difficulty)
        .map(|(class, difficulty)| class.count * found_share_rate(class, *difficulty))
        .collect();
    let total: f64 = rates.iter().sum();
    let capped: f64 = classes
        .iter()
        .zip(&rates)
        .filter(|(class, _)| wanted_ratio(class, solution.asert_difficulty, target) > cap)
        .map(|(_, rate)| *rate)
        .sum();
    let capped_fraction = if total > 0.0 { capped / total } else { 0.0 };
    capped_fraction * cap.floor().max(1.0) - (1.0 - capped_fraction)
}

#[cfg(test)]
mod tests;
