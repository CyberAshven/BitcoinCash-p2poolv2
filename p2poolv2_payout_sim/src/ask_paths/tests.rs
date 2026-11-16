// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;

#[test]
fn every_variant_but_today_runs_with_asks_and_changes_one_thing() {
    let written = AskRule::default();
    assert!(Variant::Today.asks().is_none());
    let rule = |variant: Variant| variant.asks().expect("a variant with asks").rule;
    assert_eq!(rule(Variant::AsWritten), written);
    assert_eq!(
        rule(Variant::FasterDown(2)),
        AskRule {
            descent: RatioDescent::Proportional { bound: 2 },
            ..written
        }
    );
    assert_eq!(
        rule(Variant::CapFollowsAsks),
        AskRule {
            cap: CapRule::FollowsAsks { divisor: 64 },
            ..written
        }
    );
    assert_eq!(
        rule(Variant::LowGenesis),
        AskRule {
            genesis_ratio: 32,
            ..written
        }
    );
    assert_eq!(
        rule(Variant::CapFollowsAsksLowGenesis),
        AskRule {
            cap: CapRule::FollowsAsks { divisor: 64 },
            genesis_ratio: 32,
            ..written
        }
    );
    assert_eq!(
        rule(Variant::CapOpensAtOneAsk),
        AskRule {
            cap: CapRule::FollowsAsks { divisor: 640 },
            genesis_ratio: 32,
            ..written
        }
    );
}

#[test]
fn the_test_network_pool_is_held_at_the_ceiling_until_the_big_miner_joins() {
    let pool = PathPool::test_network(0.0);
    let minimum_before = pool.hashrate * 10.0 / 4_294_967_296.0;
    let minimum_after = (pool.hashrate + pool.big_miner_hashrate()) * 10.0 / 4_294_967_296.0;
    // The weak ratio in force is at most the minimum over the ceiling.
    assert!(minimum_before / MINI_CEILING_DIFFICULTY < 32.0);
    assert!(minimum_after / MINI_CEILING_DIFFICULTY > 256.0);
}

/// A short run of `variant` from genesis in a pool at 10% of today's
/// Bitcoin Cash, where nobody asks, summed over its hours.
fn genesis_hours(variant: Variant, seconds: f64) -> HourSample {
    let case = PathCase {
        pool: PathPool::mainnet(1.0, 0.10),
        event: Event::Genesis,
        variant,
    };
    let inputs = TransitionInputs {
        after_seconds: seconds,
        ..case.inputs(&Scenario::default())
    };
    let hours = simulate_transition(&inputs, &mut SimulationRandom::from_seed(4));
    hours_sum(&hours, 0, hours.len())
}

/// The flaw PR #118's swarm found, at the model's own level: from genesis
/// the rule as written carries far more than today's 64 weak shares a
/// share; with the cap following the asks no share does, and with a genesis
/// ratio of 32 hardly any weak share expires.
#[test]
fn from_genesis_only_the_rule_as_written_carries_more_than_64() {
    let hour = 3_600.0;
    let written = genesis_hours(Variant::AsWritten, hour);
    let capped = genesis_hours(Variant::CapFollowsAsks, hour);
    let low = genesis_hours(Variant::CapFollowsAsksLowGenesis, hour);
    assert!(
        written.carried_per_share() > 100.0,
        "{} weak shares a share",
        written.carried_per_share()
    );
    assert!(written.shares_above_cap > written.shares / 2.0);
    assert_eq!(capped.shares_above_cap, 0.0);
    assert!(capped.largest_carried <= 64.0);
    assert!(
        capped.expired_fraction() > 0.3,
        "{} expired",
        capped.expired_fraction()
    );
    assert_eq!(low.shares_above_cap, 0.0);
    assert!(
        low.expired_fraction() < 0.01,
        "{} expired",
        low.expired_fraction()
    );
}
