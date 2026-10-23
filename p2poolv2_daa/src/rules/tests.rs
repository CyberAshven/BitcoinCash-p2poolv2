// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use crate::scenario::Genesis;
use bitcoin::Target;

const GENESIS_TIME: u32 = 1_776_855_600;

/// Genesis at difficulty one million, far above the main ceiling.
fn regulated_genesis() -> SimShare {
    SimShare::new(0, GENESIS_TIME, compact_for_difficulty(1e6))
}

/// A chain from `genesis` whose shares follow at `intervals`, each mined at
/// the target `rule` gives it.
fn mine_chain(rule: Rule, genesis: SimShare, intervals: &[u32]) -> SimChain {
    let context = RuleContext::new(&genesis);
    let mut chain = SimChain::new(genesis, intervals.len());
    for interval in intervals {
        let bits = rule.next_target(&context, &chain);
        let tip = chain.tip();
        chain.push(SimShare::new(tip.height + 1, tip.time + interval, bits));
    }
    chain
}

/// A chain of `count` shares at constant `bits`, `interval` seconds apart.
fn constant_chain(bits: CompactTarget, interval: u32, count: u32) -> SimChain {
    let mut chain = SimChain::new(SimShare::new(0, GENESIS_TIME, bits), count as usize);
    for height in 1..=count {
        chain.push(SimShare::new(
            height,
            GENESIS_TIME + height * interval,
            bits,
        ));
    }
    chain
}

fn ratio(first: CompactTarget, second: CompactTarget) -> f64 {
    difficulty_of(first) / difficulty_of(second)
}

/// Irregular whole-second intervals averaging about ten seconds.
fn irregular_intervals(count: usize) -> Vec<u32> {
    (0..count)
        .map(|index| [3, 17, 0, 9, 26, 1, 12, 7][index % 8])
        .collect()
}

#[test]
fn every_catalogued_rule_parses_from_its_id() {
    for rule in CATALOG {
        assert_eq!(Rule::from_id(&rule.id()), Some(rule), "{}", rule.id());
    }
    assert_eq!(
        Rule::from_id("asert-parent-h450"),
        Some(Rule::ParentAnchorHalfLife(450))
    );
    assert_eq!(
        Rule::from_id("asert-parent-h450-cap900"),
        Some(Rule::ParentAnchorCapped {
            half_life: 450,
            max_interval: 900
        })
    );
    assert_eq!(Rule::from_id("asert-parent-h0"), None);
    assert_eq!(Rule::from_id("asert-parent-hx"), None);
    assert_eq!(Rule::from_id("asert-parent-h600-cap0"), None);
    assert_eq!(Rule::from_id("asert-parent-h600-cap"), None);
    assert_eq!(Rule::from_id("lwma-99"), None);
}

/// The cap limits how far one long interval lowers the difficulty; shorter
/// intervals are the uncapped rule's.
#[test]
fn an_interval_cap_limits_the_drop_after_a_quiet_stretch() {
    let capped = Rule::ParentAnchorCapped {
        half_life: 600,
        max_interval: 1_210,
    };
    let uncapped = Rule::ParentAnchorHalfLife(600);
    let mut intervals = vec![10; 20];
    intervals.push(3_600);
    let capped_chain = mine_chain(capped, regulated_genesis(), &intervals);
    let uncapped_chain = mine_chain(uncapped, regulated_genesis(), &intervals);
    let context = RuleContext::new(&regulated_genesis());
    let before = capped_chain.tip().bits;
    // Two half-lives over the ideal interval: a quarter of the difficulty.
    let capped_next = capped.next_target(&context, &capped_chain);
    assert!((ratio(capped_next, before) - 0.25).abs() < 1e-3);
    // Uncapped, the hour takes six doublings off.
    let uncapped_next = uncapped.next_target(&context, &uncapped_chain);
    assert!((ratio(uncapped_next, before) - 2f64.powf(-3_590.0 / 600.0)).abs() < 1e-4);
    // On schedule the two agree.
    let steady = constant_chain(compact_for_difficulty(1e6), 7, 30);
    assert_eq!(
        capped.next_target(&context, &steady),
        uncapped.next_target(&context, &steady)
    );
}

#[test]
fn rule_ids_and_labels_are_unique() {
    let mut ids: Vec<String> = CATALOG.iter().map(Rule::id).collect();
    let mut labels: Vec<String> = CATALOG.iter().map(Rule::label).collect();
    ids.sort();
    ids.dedup();
    labels.sort();
    labels.dedup();
    assert_eq!(ids.len(), CATALOG.len());
    assert_eq!(labels.len(), CATALOG.len());
}

#[test]
fn rule_sets_draw_from_the_catalog() {
    for rule in UPSTREAM_RULES.iter().chain(BCH_RULES.iter()) {
        assert!(CATALOG.contains(rule), "{}", rule.id());
    }
}

/// The half-life sweep runs the node's integer code: at the production
/// half-life it gives exactly the production rule's targets.
#[test]
fn parent_anchor_at_the_production_half_life_is_the_production_rule() {
    let intervals = irregular_intervals(400);
    let production = mine_chain(Rule::ParentAnchor, regulated_genesis(), &intervals);
    let swept = mine_chain(
        Rule::ParentAnchorHalfLife(production_half_life()),
        regulated_genesis(),
        &intervals,
    );
    assert_eq!(production.shares(), swept.shares());
}

#[test]
fn a_shorter_half_life_moves_further_per_share() {
    // Shares every 5 seconds: every rule raises the difficulty.
    let chain = constant_chain(compact_for_difficulty(1e6), 5, 10);
    let context = RuleContext::new(chain.genesis());
    let fast = Rule::ParentAnchorHalfLife(150).next_target(&context, &chain);
    let slow = Rule::ParentAnchorHalfLife(2400).next_target(&context, &chain);
    assert!(ratio(fast, chain.tip().bits) > ratio(slow, chain.tip().bits));
    assert!(ratio(slow, chain.tip().bits) > 1.0);
}

/// Away from the ceiling the parent and genesis anchors agree up to the
/// rounding the parent anchor repeats at every share: the compact target
/// (at most 2^-15) and the truncated exponent (at most 2^-16 of a
/// doubling). On these fixed intervals nothing feeds the drift back; on a
/// mined chain ASERT's time term absorbs it (`asert.md`, "Rounding").
#[test]
fn parent_and_genesis_anchors_agree_away_from_the_ceiling() {
    let intervals = irregular_intervals(600);
    let parent = mine_chain(Rule::ParentAnchor, regulated_genesis(), &intervals);
    let genesis = mine_chain(Rule::GenesisAnchor, regulated_genesis(), &intervals);
    for (parent_share, genesis_share) in parent.shares().iter().zip(genesis.shares()) {
        let drift = (parent_share.difficulty / genesis_share.difficulty)
            .ln()
            .abs();
        let bound = 1e-4 + parent_share.height as f64 * 5e-5;
        assert!(drift < bound, "height {}: {drift}", parent_share.height);
    }
}

/// After a day without shares the parent anchor restarts from the ceiling
/// and climbs; the genesis anchor stays at the ceiling until the chain has
/// made up the day.
#[test]
fn only_the_parent_anchor_forgets_a_quiet_day() {
    let mut intervals = vec![10; 50];
    intervals.push(86_400);
    intervals.extend(std::iter::repeat_n(3, 300));
    let ceiling = RuleContext::new(&regulated_genesis()).ceiling();
    let parent = mine_chain(Rule::ParentAnchor, regulated_genesis(), &intervals);
    let genesis_anchored = mine_chain(Rule::GenesisAnchor, regulated_genesis(), &intervals);
    // Share 51 came a day late, so share 52 is at the ceiling.
    assert!(parent.shares()[51].difficulty > 0.9e6);
    assert_eq!(parent.shares()[52].bits, ceiling);
    assert!(difficulty_of(parent.tip().bits) > 8.0 * difficulty_of(ceiling));
    assert!(genesis_anchored.shares()[51].difficulty > 0.9e6);
    assert_eq!(genesis_anchored.tip().bits, ceiling);
}

#[test]
fn every_rule_stays_at_or_below_the_ceiling_target() {
    let genesis = SimShare::new(0, GENESIS_TIME, Genesis::Main.bits());
    let ceiling = Target::from_compact(RuleContext::new(&genesis).ceiling());
    // Very slow shares push every rule towards the easiest target.
    let intervals = vec![3_600; 120];
    for rule in CATALOG {
        let chain = mine_chain(rule, genesis, &intervals);
        for share in &chain.shares()[1..] {
            assert!(Target::from_compact(share.bits) <= ceiling, "{}", rule.id());
        }
    }
}

#[test]
fn window_rules_keep_the_tip_target_until_the_window_fills() {
    let bits = compact_for_difficulty(1e6);
    let chain = constant_chain(bits, 5, 15);
    let context = RuleContext::new(chain.genesis());
    for rule in [
        Rule::SmaUpstream { window: 72 },
        Rule::SmaWindowMean { window: 72 },
        Rule::DigiShieldUpstream,
        LWMA_45,
        LWMA_20,
        Rule::DarkGravityWave { window: 24 },
    ] {
        assert_eq!(rule.next_target(&context, &chain), bits, "{}", rule.id());
    }
}

#[test]
fn rules_hold_an_on_schedule_chain() {
    let bits = compact_for_difficulty(1e6);
    let chain = constant_chain(bits, 10, 100);
    let context = RuleContext::new(chain.genesis());
    for rule in CATALOG
        .into_iter()
        .filter(|rule| *rule != Rule::GenesisAnchor)
    {
        let next = rule.next_target(&context, &chain);
        assert!((ratio(next, bits) - 1.0).abs() < 1e-4, "{}", rule.id());
    }
    // The node's genesis rule schedules the parent at height h one ideal
    // interval after h intervals, so this chain is one interval early.
    let genesis_anchored = Rule::GenesisAnchor.next_target(&context, &chain);
    assert!((ratio(genesis_anchored, bits) - 2f64.powf(10.0 / 600.0)).abs() < 1e-4);
}

/// Shares at half the ideal interval: each window rule's own response and
/// limits, as upstream wrote them.
#[test]
fn window_rules_respond_to_fast_shares_within_their_limits() {
    let bits = compact_for_difficulty(1e6);
    let chain = constant_chain(bits, 5, 100);
    let context = RuleContext::new(chain.genesis());
    let expected = [
        (Rule::SmaUpstream { window: 72 }, 2.0),
        (Rule::SmaWindowMean { window: 72 }, 2.0),
        // Damped by four: 170 + (85 - 170) / 4 = 148.75 seconds.
        (Rule::DigiShieldUpstream, 170.0 / 148.75),
        // Twice the difficulty, limited to 1.5 times per share.
        (LWMA_45, 1.5),
        (LWMA_20, 2.0),
        (Rule::DarkGravityWave { window: 24 }, 2.0),
    ];
    for (rule, factor) in expected {
        let next = rule.next_target(&context, &chain);
        assert!(
            (ratio(next, bits) / factor - 1.0).abs() < 1e-4,
            "{}: {}",
            rule.id(),
            ratio(next, bits)
        );
    }
}

#[test]
fn digishield_limits_a_slow_window_to_two_thirds() {
    let bits = compact_for_difficulty(1e6);
    // A window five times too slow is damped to twice, then limited to 1.5.
    let chain = constant_chain(bits, 50, 30);
    let context = RuleContext::new(chain.genesis());
    let next = Rule::DigiShieldUpstream.next_target(&context, &chain);
    assert!((ratio(next, bits) - 1.0 / 1.5).abs() < 1e-4);
}

#[test]
fn lwma_limits_each_solve_time_to_six_intervals() {
    let bits = compact_for_difficulty(1e6);
    let mut chain = constant_chain(bits, 10, 60);
    // One share an hour late counts as 60 seconds, not 3,600.
    let tip = *chain.tip();
    chain.push(SimShare::new(tip.height + 1, tip.time + 3_600, bits));
    let context = RuleContext::new(chain.genesis());
    let next = Rule::Lwma {
        window: 45,
        max_change: 10.0,
    }
    .next_target(&context, &chain);
    let weights = (45 * 46 / 2) as f64;
    let average = (10.0 * (weights - 45.0) + 60.0 * 45.0) / weights;
    assert!((ratio(next, bits) - 10.0 / average).abs() < 1e-4);
}

#[test]
fn the_chain_tracks_its_tip_and_the_tips_parent() {
    let bits = compact_for_difficulty(1e6);
    let mut chain = SimChain::new(SimShare::new(0, GENESIS_TIME, bits), 2);
    assert_eq!(chain.previous_time(), GENESIS_TIME);
    assert_eq!(chain.genesis(), chain.tip());
    chain.push(SimShare::new(1, GENESIS_TIME + 7, bits));
    let parent = chain.asert_parent();
    assert_eq!(parent.height, 1);
    assert_eq!(parent.time, GENESIS_TIME + 7);
    assert_eq!(parent.previous_time, GENESIS_TIME);
    assert_eq!(parent.bits, bits);
}
