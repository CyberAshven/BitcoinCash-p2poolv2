// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The difficulty rules compared: the share chain's ASERT on both anchors
//! (the node's own code), the parent anchor at other half-lives (the same
//! integer code), and the window algorithms upstream's analysis considered.
//!
//! Every rule reads only the header fields of earlier shares (height,
//! whole-second time, compact target) and returns the compact target of the
//! next share, never easier than the chain's ceiling.

use crate::difficulty::{
    compact_for_difficulty, difficulty_of, ideal_share_interval, no_easier_than,
    production_half_life,
};
use bitcoin::CompactTarget;
use p2poolv2_lib::config::Chain;
use p2poolv2_lib::pool_difficulty::{AsertParent, PoolDifficulty, asert_target_with_easy_clamp};

/// The header fields of a simulated share that a difficulty rule reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SimShare {
    pub height: u32,
    /// Header time, whole seconds since the Unix epoch.
    pub time: u32,
    pub bits: CompactTarget,
    /// Share difficulty of `bits`, kept so window rules need not recompute it.
    pub difficulty: f64,
}

impl SimShare {
    pub fn new(height: u32, time: u32, bits: CompactTarget) -> Self {
        Self {
            height,
            time,
            bits,
            difficulty: difficulty_of(bits),
        }
    }
}

/// A simulated share chain: genesis followed by every share mined on it.
/// It is never empty, so it always has a tip.
#[derive(Clone, Debug)]
pub struct SimChain {
    shares: Vec<SimShare>,
}

impl SimChain {
    /// A chain holding only `genesis`, with room for `capacity` more shares.
    pub fn new(genesis: SimShare, capacity: usize) -> Self {
        let mut shares = Vec::with_capacity(capacity + 1);
        shares.push(genesis);
        Self { shares }
    }

    pub fn push(&mut self, share: SimShare) {
        self.shares.push(share);
    }

    /// Genesis first.
    pub fn shares(&self) -> &[SimShare] {
        &self.shares
    }

    pub fn genesis(&self) -> &SimShare {
        &self.shares[0]
    }

    pub fn tip(&self) -> &SimShare {
        &self.shares[self.shares.len() - 1]
    }

    /// Time of the tip's parent; genesis, which has none, uses its own, as
    /// `AsertParent` does.
    pub fn previous_time(&self) -> u32 {
        match self.shares.len().checked_sub(2) {
            Some(index) => self.shares[index].time,
            None => self.tip().time,
        }
    }

    /// The tip as the parent of the next share.
    pub fn asert_parent(&self) -> AsertParent {
        let tip = self.tip();
        AsertParent {
            height: tip.height,
            time: tip.time,
            bits: tip.bits,
            previous_time: self.previous_time(),
        }
    }
}

/// A difficulty adjustment rule.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Rule {
    /// The production Bitcoin Cash rule (decision 12): ASERT anchored at
    /// each share's parent, computed by `PoolDifficulty` on
    /// `Chain::BitcoinCash` with the production half-life.
    ParentAnchor,
    /// Upstream's choice and Bitcoin's rule: ASERT anchored at genesis,
    /// computed by `PoolDifficulty` on `Chain::Bitcoin`.
    GenesisAnchor,
    /// The parent anchor's integer arithmetic
    /// (`asert_target_with_easy_clamp`) with another half-life in seconds.
    ParentAnchorHalfLife(u32),
    /// Not a share-chain rule: an option this analysis measures. The parent
    /// anchor with `half_life`, counting the parent's interval as at most
    /// `max_interval` seconds, so one quiet stretch lowers the difficulty by
    /// at most `(max_interval - 10) / half_life` doublings. Same integer code.
    ParentAnchorCapped { half_life: u32, max_interval: u32 },
    /// Upstream's "SMA-72": the tip's difficulty times expected over actual
    /// window time, at every share. It compounds the window's error once per
    /// share, unlike a moving average.
    SmaUpstream { window: usize },
    /// A simple moving average: the window's mean difficulty times expected
    /// over actual window time.
    SmaWindowMean { window: usize },
    /// Upstream's DigiShield v3: a 17-share window, its deviation damped by
    /// four and limited to -25%/+50%, applied to the tip's difficulty.
    DigiShieldUpstream,
    /// Upstream's LWMA: linearly weighted solve times (each limited to six
    /// ideal intervals), the window's mean difficulty, and at most
    /// `max_change` times the tip's difficulty either way.
    Lwma { window: usize, max_change: f64 },
    /// Upstream's "DGW v3": the window's mean difficulty times expected over
    /// actual window time, limited to a factor of three either way.
    DarkGravityWave { window: usize },
}

/// LWMA-45, as upstream ran it.
pub const LWMA_45: Rule = Rule::Lwma {
    window: 45,
    max_change: 1.5,
};

/// LWMA-20 ("tight"), as upstream ran it.
pub const LWMA_20: Rule = Rule::Lwma {
    window: 20,
    max_change: 2.0,
};

/// Every named rule, in table order. Other parent-anchor half-lives and caps
/// parse from `asert-parent-h<seconds>` and `asert-parent-h<seconds>-cap<seconds>`.
pub const CATALOG: [Rule; 15] = [
    Rule::ParentAnchor,
    Rule::GenesisAnchor,
    Rule::ParentAnchorHalfLife(150),
    Rule::ParentAnchorHalfLife(300),
    Rule::ParentAnchorHalfLife(1200),
    Rule::ParentAnchorHalfLife(2400),
    Rule::ParentAnchorHalfLife(3600),
    Rule::ParentAnchorCapped {
        half_life: 600,
        max_interval: 1200,
    },
    Rule::ParentAnchorCapped {
        half_life: 300,
        max_interval: 600,
    },
    Rule::SmaUpstream { window: 72 },
    Rule::SmaWindowMean { window: 72 },
    Rule::DigiShieldUpstream,
    LWMA_45,
    LWMA_20,
    Rule::DarkGravityWave { window: 24 },
];

/// Upstream's six algorithms (ASERT as the node's genesis anchor), the
/// production rule, and a true moving average beside upstream's SMA.
pub const UPSTREAM_RULES: [Rule; 8] = [
    Rule::ParentAnchor,
    Rule::GenesisAnchor,
    Rule::SmaUpstream { window: 72 },
    Rule::SmaWindowMean { window: 72 },
    Rule::DigiShieldUpstream,
    LWMA_45,
    LWMA_20,
    Rule::DarkGravityWave { window: 24 },
];

/// The rules the Bitcoin Cash scenarios compare: the production rule, the
/// half-life sweep, the genesis anchor, and upstream's three best window
/// algorithms.
pub const BCH_RULES: [Rule; 10] = [
    Rule::ParentAnchor,
    Rule::ParentAnchorHalfLife(150),
    Rule::ParentAnchorHalfLife(300),
    Rule::ParentAnchorHalfLife(1200),
    Rule::ParentAnchorHalfLife(2400),
    Rule::GenesisAnchor,
    Rule::DigiShieldUpstream,
    LWMA_45,
    Rule::DarkGravityWave { window: 24 },
    Rule::SmaWindowMean { window: 72 },
];

/// What every rule needs from the chain's genesis: both `PoolDifficulty`
/// anchors and the ceiling.
#[derive(Clone, Copy)]
pub struct RuleContext {
    parent_anchor: PoolDifficulty,
    genesis_anchor: PoolDifficulty,
    ceiling: CompactTarget,
    ceiling_difficulty: f64,
    ideal_interval: u32,
}

impl RuleContext {
    pub fn new(genesis: &SimShare) -> Self {
        let genesis_anchor =
            PoolDifficulty::new(genesis.bits, genesis.time, 0).for_chain(Chain::Bitcoin);
        let parent_anchor = genesis_anchor.for_chain(Chain::BitcoinCash);
        let ceiling = parent_anchor.target_ceiling();
        Self {
            parent_anchor,
            genesis_anchor,
            ceiling,
            ceiling_difficulty: difficulty_of(ceiling),
            ideal_interval: ideal_share_interval(),
        }
    }

    /// Easiest target the chain allows.
    pub fn ceiling(&self) -> CompactTarget {
        self.ceiling
    }

    pub fn ceiling_difficulty(&self) -> f64 {
        self.ceiling_difficulty
    }

    /// `difficulty` as a header target, never easier than the ceiling.
    fn target_for_difficulty(&self, difficulty: f64) -> CompactTarget {
        let floored = difficulty.max(self.ceiling_difficulty);
        no_easier_than(compact_for_difficulty(floored), self.ceiling)
    }
}

impl Rule {
    /// Identifier for the command line.
    pub fn id(&self) -> String {
        match self {
            Rule::ParentAnchor => "asert-parent".to_string(),
            Rule::GenesisAnchor => "asert-genesis".to_string(),
            Rule::ParentAnchorHalfLife(half_life) => format!("asert-parent-h{half_life}"),
            Rule::ParentAnchorCapped {
                half_life,
                max_interval,
            } => format!("asert-parent-h{half_life}-cap{max_interval}"),
            Rule::SmaUpstream { window } => format!("sma-{window}"),
            Rule::SmaWindowMean { window } => format!("sma-{window}-mean"),
            Rule::DigiShieldUpstream => "digishield".to_string(),
            Rule::Lwma { window, .. } => format!("lwma-{window}"),
            Rule::DarkGravityWave { window } => format!("dgw-{window}"),
        }
    }

    /// Name in result tables.
    pub fn label(&self) -> String {
        match self {
            Rule::ParentAnchor => {
                format!("ASERT parent, {} s (production)", production_half_life())
            }
            Rule::GenesisAnchor => {
                format!("ASERT genesis, {} s (upstream)", production_half_life())
            }
            Rule::ParentAnchorHalfLife(half_life) => format!("ASERT parent, {half_life} s"),
            Rule::ParentAnchorCapped {
                half_life,
                max_interval,
            } => format!("ASERT parent, {half_life} s, interval cap {max_interval} s"),
            Rule::SmaUpstream { window } => format!("SMA-{window} (upstream form)"),
            Rule::SmaWindowMean { window } => format!("SMA-{window} (window mean)"),
            Rule::DigiShieldUpstream => "DigiShield v3 (upstream form)".to_string(),
            Rule::Lwma { window, .. } => format!("LWMA-{window}"),
            Rule::DarkGravityWave { window } => format!("DGW v3 ({window})"),
        }
    }

    /// The rule named `id`: a catalog entry, `asert-parent-h<seconds>` or
    /// `asert-parent-h<seconds>-cap<seconds>`.
    pub fn from_id(id: &str) -> Option<Rule> {
        let catalogued = CATALOG.iter().find(|rule| rule.id() == id).copied();
        catalogued.or_else(|| {
            let parameters = id.strip_prefix("asert-parent-h")?;
            let positive = |text: &str| text.parse::<u32>().ok().filter(|seconds| *seconds > 0);
            match parameters.split_once("-cap") {
                Some((half_life, max_interval)) => Some(Rule::ParentAnchorCapped {
                    half_life: positive(half_life)?,
                    max_interval: positive(max_interval)?,
                }),
                None => positive(parameters).map(Rule::ParentAnchorHalfLife),
            }
        })
    }

    /// The parent anchor's integer code with `half_life`, counting the
    /// parent's interval as at most `max_interval` seconds.
    fn parent_anchor_with(
        context: &RuleContext,
        chain: &SimChain,
        half_life: u32,
        max_interval: i64,
    ) -> CompactTarget {
        let parent = chain.asert_parent();
        let interval = parent.time as i64 - parent.previous_time as i64;
        asert_target_with_easy_clamp(
            parent.bits,
            interval.min(max_interval),
            0,
            half_life,
            context.ideal_interval,
            context.ceiling,
        )
    }

    /// Compact target of the share that extends `chain`.
    pub fn next_target(&self, context: &RuleContext, chain: &SimChain) -> CompactTarget {
        match self {
            Rule::ParentAnchor => context
                .parent_anchor
                .calculate_target_clamped(chain.asert_parent()),
            Rule::GenesisAnchor => context
                .genesis_anchor
                .calculate_target_clamped(chain.asert_parent()),
            Rule::ParentAnchorHalfLife(half_life) => {
                Self::parent_anchor_with(context, chain, *half_life, i64::MAX)
            }
            Rule::ParentAnchorCapped {
                half_life,
                max_interval,
            } => Self::parent_anchor_with(context, chain, *half_life, *max_interval as i64),
            window_rule => {
                let ideal = context.ideal_interval as f64;
                match window_difficulty(window_rule, chain.shares(), ideal) {
                    Some(difficulty) => context.target_for_difficulty(difficulty),
                    // Upstream keeps the tip's difficulty until the window
                    // fills. The main genesis bits are easier than the main
                    // ceiling, so the ceiling still applies.
                    None => no_easier_than(chain.tip().bits, context.ceiling),
                }
            }
        }
    }
}

/// Next difficulty from a window rule, or `None` while the chain is shorter
/// than the window plus one share. ASERT rules return `None`.
fn window_difficulty(rule: &Rule, shares: &[SimShare], ideal: f64) -> Option<f64> {
    match rule {
        Rule::SmaUpstream { window } => {
            let window_shares = last_window(shares, *window)?;
            let ratio = window_ratio(window_shares, *window, ideal).clamp(0.25, 4.0);
            Some(window_shares[*window].difficulty * ratio)
        }
        Rule::SmaWindowMean { window } => {
            let window_shares = last_window(shares, *window)?;
            let ratio = window_ratio(window_shares, *window, ideal).clamp(0.25, 4.0);
            Some(mean_difficulty(&window_shares[1..]) * ratio)
        }
        Rule::DigiShieldUpstream => digishield(shares, ideal),
        Rule::Lwma { window, max_change } => lwma(shares, *window, *max_change, ideal),
        Rule::DarkGravityWave { window } => {
            let window_shares = last_window(shares, *window)?;
            let ratio = window_ratio(window_shares, *window, ideal).clamp(1.0 / 3.0, 3.0);
            Some(mean_difficulty(&window_shares[1..]) * ratio)
        }
        Rule::ParentAnchor
        | Rule::GenesisAnchor
        | Rule::ParentAnchorHalfLife(_)
        | Rule::ParentAnchorCapped { .. } => None,
    }
}

/// The last `window + 1` shares (the window and the share before it).
fn last_window(shares: &[SimShare], window: usize) -> Option<&[SimShare]> {
    shares
        .len()
        .checked_sub(window + 1)
        .map(|start| &shares[start..])
}

/// Seconds from the first to the last share of `window_shares`; upstream
/// counts a non-positive span as one second.
fn window_span(window_shares: &[SimShare]) -> f64 {
    let first = window_shares[0].time as f64;
    let last = window_shares[window_shares.len() - 1].time as f64;
    let span = last - first;
    if span <= 0.0 { 1.0 } else { span }
}

/// Expected over actual time for a window of `window` intervals.
fn window_ratio(window_shares: &[SimShare], window: usize, ideal: f64) -> f64 {
    window as f64 * ideal / window_span(window_shares)
}

fn mean_difficulty(shares: &[SimShare]) -> f64 {
    shares.iter().map(|share| share.difficulty).sum::<f64>() / shares.len() as f64
}

/// Upstream's DigiShield v3 (N = 17).
fn digishield(shares: &[SimShare], ideal: f64) -> Option<f64> {
    const WINDOW: usize = 17;
    let window_shares = last_window(shares, WINDOW)?;
    let expected = WINDOW as f64 * ideal;
    let damped = expected + (window_span(window_shares) - expected) / 4.0;
    let limited = damped.clamp(expected * 0.75, expected * 1.5);
    Some(window_shares[WINDOW].difficulty * expected / limited)
}

/// Upstream's LWMA.
fn lwma(shares: &[SimShare], window: usize, max_change: f64, ideal: f64) -> Option<f64> {
    let window_shares = last_window(shares, window)?;
    let (weighted_sum, weight_total) = window_shares.windows(2).zip(1..=window).fold(
        (0.0f64, 0.0f64),
        |(weighted_sum, weight_total), (pair, weight)| {
            let solve_time =
                (pair[1].time as f64 - pair[0].time as f64).clamp(-6.0 * ideal, 6.0 * ideal);
            (
                weighted_sum + solve_time * weight as f64,
                weight_total + weight as f64,
            )
        },
    );
    let average = weighted_sum / weight_total;
    let average = if average <= 0.0 { ideal * 0.1 } else { average };
    let next = mean_difficulty(&window_shares[1..]) * ideal / average;
    let previous = window_shares[window].difficulty;
    Some(
        next.clamp(previous / max_change, previous * max_change)
            .max(0.001),
    )
}

#[cfg(test)]
mod tests;
