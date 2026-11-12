// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The weak share queue with share races simulated (study #85).
//!
//! `weak.rs` takes stale shares as a flat rate. Here a share is a race with
//! probability `stale / (1 - stale)`: a second share is found on the same
//! parent, both carry what their nodes carry from the same queue, and fork
//! choice picks the winner. The loser is a stale share (an uncle); the weak
//! shares it carried stay waiting, as the design page has nodes keep a weak
//! share until the best chain carries it, unless the settings drop them.
//!
//! Fork choice counts a share's own work, and under the study's rule also
//! the work of the weak shares it carries, each once. With
//! [`ForkChoice::WeakWorkSiblings`] carrying nodes also act on it: when the
//! tip comes from a non-carrying node and carried less weak work than they
//! would have carried on the same parent, they keep mining on that parent.
//! A share they find there is a sibling that outweighs the tip and replaces
//! it, and the weak shares the tip's own miners found on it are lost with
//! it. A share a non-carrying node finds first is the tip's child and ends
//! the chance.
//!
//! Each step is one found share at an exponential time with a mean of one
//! share period. Replacements are not fed back into ASERT: they take steps
//! without growing the chain, and the weak ratio absorbs the change in weak
//! shares per chain share.

use crate::constants::FIXED_POINT_ONE;
use crate::sampling::poisson;
use crate::weak::{Carried, WaitingWeakShares, WeakQueueInputs, WeakQueueOutcome, next_weak_ratio};
use p2poolv2_netsim::random::SimulationRandom;

/// What decides a race between two shares on the same parent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForkChoice {
    /// Today's rule: a share adds its own work to its chain. The heavier
    /// share wins a race; equal ones win half each.
    ShareWork,
    /// The study's rule: a share also adds the work of the weak shares it
    /// carries, each once.
    WeakWork,
    /// The study's rule, and carrying nodes mine a sibling of a tip that
    /// carried less weak work than they would have on the same parent.
    WeakWorkSiblings,
}

impl ForkChoice {
    /// Name in tables.
    pub fn name(&self) -> &'static str {
        match self {
            ForkChoice::ShareWork => "share work",
            ForkChoice::WeakWork => "weak work",
            ForkChoice::WeakWorkSiblings => "weak work, siblings",
        }
    }

    /// Whether carried weak shares count in a chain's work.
    pub fn counts_weak_work(&self) -> bool {
        !matches!(self, ForkChoice::ShareWork)
    }
}

/// The races the queue simulates and what they lose.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RaceSettings {
    /// The fork choice rule.
    pub fork_choice: ForkChoice,
    /// Races per share step, `stale / (1 - stale)`: each race makes one
    /// stale share.
    pub races_per_share: f64,
    /// Fraction of the weak shares found that were mined on a parent that
    /// then left the chain, lost uncredited.
    pub stale_parent_loss: f64,
    /// Whether the weak shares a race's loser carried are dropped instead
    /// of staying in the queue.
    pub uncle_carrier_drops: bool,
}

/// One kind of share a race can draw: a class of miners on the chain.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Racer {
    /// Shares the class finds a second, stale ones included; racers are
    /// drawn in proportion to it.
    pub found_rate: f64,
    /// Whether its node carries other nodes' weak shares.
    pub on_carrying_node: bool,
    /// Its share's own work over the minimum's.
    pub work_multiple: f64,
}

/// What the race queue depends on.
#[derive(Debug, Clone)]
pub struct RaceQueueInputs {
    /// The weak share queue's inputs; its `carrying_share_fraction` is not
    /// used, since the racers say who makes each share.
    pub queue: WeakQueueInputs,
    /// The classes that make shares, in chain class order.
    pub racers: Vec<Racer>,
    /// The races.
    pub settings: RaceSettings,
}

/// What the race queue settles at.
#[derive(Debug, Clone, PartialEq)]
pub struct RaceOutcome {
    /// The weak share queue's measures. Its expired fractions, like the
    /// lost fractions below, are of all weak shares found.
    pub queue: WeakQueueOutcome,
    /// Each racer's stale shares over its found shares.
    pub stale_fraction: Vec<f64>,
    /// Weak shares a chain share from a non-carrying node carries.
    pub carried_per_non_carrying_share: f64,
    /// Fraction of carrying nodes' weak shares lost: mined on a parent that
    /// left the chain, or dropped with a race's loser.
    pub carrying_lost_fraction: f64,
    /// The same for non-carrying nodes' weak shares.
    pub non_carrying_lost_fraction: f64,
    /// Tips replaced by a heavier sibling, per chain share.
    pub replaced_per_share: f64,
}

/// The winner of a race between two shares on the same parent: true when
/// `first` wins. A share's chain work is its own work and, when the rule
/// counts it, its carried weak shares' work at the weak ratio `ratio`
/// (weak work over the minimum's is `1 / ratio`). Equal work is a coin
/// toss.
pub(crate) fn first_wins_race(
    first: (&Racer, Carried),
    second: (&Racer, Carried),
    ratio: f64,
    fork_choice: ForkChoice,
    random: &mut SimulationRandom,
) -> bool {
    let work = |racer: &Racer, carried: Carried| -> f64 {
        let weak = if fork_choice.counts_weak_work() {
            carried.total() as f64 / ratio
        } else {
            0.0
        };
        racer.work_multiple + weak
    };
    let first_work = work(first.0, first.1);
    let second_work = work(second.0, second.1);
    if (first_work - second_work).abs() <= 1e-12 * first_work.max(second_work) {
        random.next_bool(0.5)
    } else {
        first_work > second_work
    }
}

/// Whether a tip from a non-carrying node is light: a carrying node's share
/// at the minimum on the same parent, carrying `honest` weak shares, would
/// outweigh it.
pub(crate) fn is_light_tip(tip: &Racer, carried: Carried, honest: Carried, ratio: f64) -> bool {
    !tip.on_carrying_node
        && tip.work_multiple + carried.total() as f64 / ratio < 1.0 + honest.total() as f64 / ratio
}

/// A tip carrying nodes are mining a sibling of.
#[derive(Debug, Clone)]
struct LightTip {
    racer: usize,
    queue_before_carry: WaitingWeakShares,
    carried: Carried,
    tallied: bool,
}

/// Running sums over the measured steps.
#[derive(Debug, Clone, Default)]
struct RaceTally {
    shares: u64,
    carrying_shares: u64,
    non_carrying_shares: u64,
    full_shares: u64,
    replaced: u64,
    time: f64,
    ratio_time: f64,
    found_carrying: u64,
    found_non_carrying: u64,
    carried_by_carrying: u64,
    carried_by_non_carrying: u64,
    expired_carrying: u64,
    expired_non_carrying: u64,
    lost_carrying: u64,
    lost_non_carrying: u64,
    chain_shares: Vec<u64>,
    stale_shares: Vec<u64>,
}

/// Weak shares found during one step, those lost to a stale parent apart.
#[derive(Debug, Clone, Copy, Default)]
struct FoundWeak {
    carrying: u64,
    non_carrying: u64,
    lost_carrying: u64,
    lost_non_carrying: u64,
}

/// The queue and the chain tip as the race queue runs.
struct RaceQueue<'a> {
    inputs: &'a RaceQueueInputs,
    cumulative_found: Vec<f64>,
    waiting: WaitingWeakShares,
    tip_ratio: u64,
    tip_carried: u64,
    light_tip: Option<LightTip>,
    measuring: bool,
    tally: RaceTally,
}

impl<'a> RaceQueue<'a> {
    fn new(inputs: &'a RaceQueueInputs) -> Self {
        let mut running = 0.0;
        let cumulative_found = inputs
            .racers
            .iter()
            .map(|racer| {
                running += racer.found_rate;
                running
            })
            .collect();
        let racer_count = inputs.racers.len();
        Self {
            inputs,
            cumulative_found,
            waiting: WaitingWeakShares::new(inputs.queue.weak_depth),
            tip_ratio: inputs.queue.ratio_max * FIXED_POINT_ONE,
            tip_carried: 0,
            light_tip: None,
            measuring: false,
            tally: RaceTally {
                chain_shares: vec![0; racer_count],
                stale_shares: vec![0; racer_count],
                ..RaceTally::default()
            },
        }
    }

    /// A class drawn in proportion to the shares it finds.
    fn draw_racer(&self, random: &mut SimulationRandom) -> usize {
        let total = self.cumulative_found.last().copied().unwrap_or(0.0);
        let point = random.next_unit() * total;
        self.cumulative_found
            .partition_point(|running| *running <= point)
            .min(self.inputs.racers.len() - 1)
    }

    /// The weak ratio in force on the tip, the ceiling applied.
    fn effective_ratio(&self) -> f64 {
        (self.tip_ratio as f64 / FIXED_POINT_ONE as f64)
            .min(self.inputs.queue.ceiling_ratio)
            .max(1.0)
    }

    /// Weak shares found in `interval` periods at weak ratio `ratio`, each
    /// lost to a stale parent with the settings' chance.
    fn find_weak(&self, interval: f64, ratio: f64, random: &mut SimulationRandom) -> FoundWeak {
        let queue = &self.inputs.queue;
        let loss = self.inputs.settings.stale_parent_loss;
        let per_period = ratio - 1.0;
        let carrying_mean = per_period * queue.carrying_minimum_shares_per_period * interval;
        let non_carrying_mean =
            per_period * queue.non_carrying_minimum_shares_per_period * interval;
        FoundWeak {
            carrying: poisson(random, carrying_mean * (1.0 - loss)),
            non_carrying: poisson(random, non_carrying_mean * (1.0 - loss)),
            lost_carrying: poisson(random, carrying_mean * loss),
            lost_non_carrying: poisson(random, non_carrying_mean * loss),
        }
    }

    /// Count the time and the weak shares found in a step.
    fn tally_step(&mut self, interval: f64, ratio: f64, found: &FoundWeak) {
        if self.measuring {
            self.tally.time += interval;
            self.tally.ratio_time += ratio * interval;
            self.tally.found_carrying += found.carrying + found.lost_carrying;
            self.tally.found_non_carrying += found.non_carrying + found.lost_non_carrying;
            self.tally.lost_carrying += found.lost_carrying;
            self.tally.lost_non_carrying += found.lost_non_carrying;
        }
    }

    /// Count a share that joined the chain, or, with `joined` false, take
    /// it back out as a stale share when a sibling replaced it.
    fn tally_chain_share(&mut self, racer: usize, carried: Carried, joined: bool) {
        if self.measuring {
            let on_carrying_node = self.inputs.racers[racer].on_carrying_node;
            let total = carried.total();
            let full = u64::from(total >= self.inputs.queue.weak_cap);
            if joined {
                self.tally.shares += 1;
                self.tally.chain_shares[racer] += 1;
                self.tally.full_shares += full;
                if on_carrying_node {
                    self.tally.carrying_shares += 1;
                    self.tally.carried_by_carrying += total;
                } else {
                    self.tally.non_carrying_shares += 1;
                    self.tally.carried_by_non_carrying += total;
                }
            } else {
                self.tally.shares -= 1;
                self.tally.chain_shares[racer] -= 1;
                self.tally.full_shares -= full;
                self.tally.stale_shares[racer] += 1;
                self.tally.replaced += 1;
                if on_carrying_node {
                    self.tally.carrying_shares -= 1;
                    self.tally.carried_by_carrying -= total;
                } else {
                    self.tally.non_carrying_shares -= 1;
                    self.tally.carried_by_non_carrying -= total;
                }
            }
        }
    }

    /// A new share on the tip, drawn as `first`: a race with another one
    /// at the settings' rate, fork choice picking the winner. Returns the
    /// winner and what it carried; the queue is left as the winner left it.
    fn add_share(
        &mut self,
        first: usize,
        ratio: f64,
        random: &mut SimulationRandom,
    ) -> (usize, Carried) {
        let inputs = self.inputs;
        let racers = &inputs.racers;
        let cap = inputs.queue.weak_cap;
        let settings = inputs.settings;
        let queue_before_carry = self.waiting.clone();
        let (winner, carried) = if random.next_bool(settings.races_per_share) {
            let second = self.draw_racer(random);
            let mut first_queue = self.waiting.clone();
            let first_carried = first_queue.carry(racers[first].on_carrying_node, cap);
            let mut second_queue = self.waiting.clone();
            let second_carried = second_queue.carry(racers[second].on_carrying_node, cap);
            let first_won = first_wins_race(
                (&racers[first], first_carried),
                (&racers[second], second_carried),
                ratio,
                settings.fork_choice,
                random,
            );
            let (winner, winner_carried, winner_queue, loser, loser_queue) = if first_won {
                (first, first_carried, first_queue, second, second_queue)
            } else {
                (second, second_carried, second_queue, first, first_queue)
            };
            self.waiting = winner_queue;
            if settings.uncle_carrier_drops {
                let dropped = self.waiting.keep_shared_with(&loser_queue);
                if self.measuring {
                    self.tally.lost_carrying += dropped.carrying;
                    self.tally.lost_non_carrying += dropped.non_carrying;
                }
            }
            if self.measuring {
                self.tally.stale_shares[loser] += 1;
            }
            (winner, winner_carried)
        } else {
            let carried = self.waiting.carry(racers[first].on_carrying_node, cap);
            (first, carried)
        };
        self.tally_chain_share(winner, carried, true);
        if settings.fork_choice == ForkChoice::WeakWorkSiblings {
            let mut honest_queue = queue_before_carry.clone();
            let honest = honest_queue.carry_all(cap);
            if is_light_tip(&racers[winner], carried, honest, ratio) {
                self.light_tip = Some(LightTip {
                    racer: winner,
                    queue_before_carry,
                    carried,
                    tallied: self.measuring,
                });
            }
        }
        (winner, carried)
    }

    /// One step with an ordinary tip: weak shares found on it, then the
    /// next share.
    fn step_on_tip(&mut self, random: &mut SimulationRandom) {
        let interval = random.next_exponential(1.0);
        let ratio = self.effective_ratio();
        let found = self.find_weak(interval, ratio, random);
        let inputs = self.inputs;
        let queue = &inputs.queue;
        let child_ratio = next_weak_ratio(
            self.tip_ratio,
            self.tip_carried,
            queue.weak_fill,
            queue.ratio_max,
        );
        let expired = self.waiting.deepen(found.carrying, found.non_carrying);
        self.tally_step(interval, ratio, &found);
        self.tally_expired(expired.0, expired.1);
        let first = self.draw_racer(random);
        let (_, carried) = self.add_share(first, ratio, random);
        self.tip_ratio = child_ratio;
        self.tip_carried = carried.total();
    }

    /// One step while carrying nodes mine a sibling of the light tip: their
    /// weak shares are found on its parent, the others' on the tip. A
    /// carrying node's share replaces the tip; another's is its child.
    fn step_on_light_tip(&mut self, light_tip: LightTip, random: &mut SimulationRandom) {
        let interval = random.next_exponential(1.0);
        let ratio = self.effective_ratio();
        let found = self.find_weak(interval, ratio, random);
        self.tally_step(interval, ratio, &found);
        let next = self.draw_racer(random);
        let cap = self.inputs.queue.weak_cap;
        if self.inputs.racers[next].on_carrying_node {
            if light_tip.tallied {
                self.tally_chain_share(light_tip.racer, light_tip.carried, false);
            }
            if self.measuring {
                self.tally.lost_non_carrying += found.non_carrying;
            }
            self.waiting = light_tip.queue_before_carry;
            self.waiting.add_at(0, found.carrying, 0);
            let carried = self.waiting.carry_all(cap);
            self.tally_chain_share(next, carried, true);
            self.tip_carried = carried.total();
        } else {
            let inputs = self.inputs;
            let queue = &inputs.queue;
            let child_ratio = next_weak_ratio(
                self.tip_ratio,
                self.tip_carried,
                queue.weak_fill,
                queue.ratio_max,
            );
            let expired = self.waiting.deepen(0, found.non_carrying);
            let parent_expired = if queue.weak_depth > 1 {
                self.waiting.add_at(1, found.carrying, 0);
                0
            } else {
                found.carrying
            };
            self.tally_expired(expired.0 + parent_expired, expired.1);
            let (_, carried) = self.add_share(next, ratio, random);
            self.tip_ratio = child_ratio;
            self.tip_carried = carried.total();
        }
    }

    fn tally_expired(&mut self, carrying: u64, non_carrying: u64) {
        if self.measuring {
            self.tally.expired_carrying += carrying;
            self.tally.expired_non_carrying += non_carrying;
        }
    }

    fn run(mut self, random: &mut SimulationRandom) -> RaceOutcome {
        let inputs = self.inputs;
        let queue = &inputs.queue;
        let total_steps = queue.warmup_shares + queue.shares;
        let mut step = 0;
        while step < total_steps {
            self.measuring = step >= queue.warmup_shares;
            match self.light_tip.take() {
                Some(light_tip) => self.step_on_light_tip(light_tip, random),
                None => self.step_on_tip(random),
            }
            step += 1;
        }
        summarise(&self.tally)
    }
}

/// `numerator / denominator`, zero when the denominator is.
fn ratio_or_zero(numerator: f64, denominator: f64) -> f64 {
    if denominator > 0.0 {
        numerator / denominator
    } else {
        0.0
    }
}

fn summarise(tally: &RaceTally) -> RaceOutcome {
    let shares = tally.shares as f64;
    let found_carrying = tally.found_carrying as f64;
    let found_non_carrying = tally.found_non_carrying as f64;
    let queue = WeakQueueOutcome {
        mean_ratio: ratio_or_zero(tally.ratio_time, tally.time).max(1.0),
        arrivals_per_share: ratio_or_zero(found_carrying + found_non_carrying, shares),
        carried_per_share: ratio_or_zero(
            (tally.carried_by_carrying + tally.carried_by_non_carrying) as f64,
            shares,
        ),
        carried_per_carrying_share: ratio_or_zero(
            tally.carried_by_carrying as f64,
            tally.carrying_shares as f64,
        ),
        carrying_expired_fraction: ratio_or_zero(tally.expired_carrying as f64, found_carrying),
        non_carrying_expired_fraction: ratio_or_zero(
            tally.expired_non_carrying as f64,
            found_non_carrying,
        ),
        full_share_fraction: ratio_or_zero(tally.full_shares as f64, shares),
    };
    let stale_fraction = tally
        .chain_shares
        .iter()
        .zip(&tally.stale_shares)
        .map(|(chain, stale)| ratio_or_zero(*stale as f64, (chain + stale) as f64))
        .collect();
    RaceOutcome {
        queue,
        stale_fraction,
        carried_per_non_carrying_share: ratio_or_zero(
            tally.carried_by_non_carrying as f64,
            tally.non_carrying_shares as f64,
        ),
        carrying_lost_fraction: ratio_or_zero(tally.lost_carrying as f64, found_carrying),
        non_carrying_lost_fraction: ratio_or_zero(
            tally.lost_non_carrying as f64,
            found_non_carrying,
        ),
        replaced_per_share: ratio_or_zero(tally.replaced as f64, shares),
    }
}

/// Run the race queue and report what it settles at.
pub fn simulate_race_queue(inputs: &RaceQueueInputs, random: &mut SimulationRandom) -> RaceOutcome {
    RaceQueue::new(inputs).run(random)
}

#[cfg(test)]
mod tests;
