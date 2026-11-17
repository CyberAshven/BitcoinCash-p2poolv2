// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;
use crate::constants::{BCH_BLOCK_INTERVAL_SECONDS, BCH_NETWORK_HASHRATE};
use crate::window::size_window;

/// Satoshis a block pays its window, the donation taken: 30,937,500 for a
/// one-minute block.
fn paid_block() -> u64 {
    (BLOCK_REWARD_SATOSHIS * (1.0 - DONATION_FRACTION)) as u64
}

/// The window of a pool of 1% of Bitcoin Cash at one chain share every 10
/// seconds: 1,800 shares, 5 hours.
fn one_percent_window() -> WindowGeometry {
    size_window(0.01 * BCH_NETWORK_HASHRATE, 0.1, BCH_NETWORK_HASHRATE, 10.0)
}

/// Blocks a second of a chain with `fraction` of Bitcoin Cash's hashrate.
fn block_rate(fraction: f64) -> f64 {
    fraction / BCH_BLOCK_INTERVAL_SECONDS
}

#[test]
fn a_miner_alone_on_its_chain_gets_every_block_simulated_share_by_share() {
    // 100 entries expected in each window: simulated one by one, and the
    // window is all its own.
    let inputs = ChainPayoutInputs {
        block_rate: block_rate(1.0),
        window: WindowGeometry {
            shares: 1_000.0,
            span_seconds: 10_000.0,
            skip_seconds: 1_000.0,
            work: 1.0e18,
            capped: false,
        },
        others_credited_rate: 0.0,
        tracked: vec![TrackedInputs {
            found_rate: 0.01,
            stale_rate: 0.021,
            chain_weight: 1.0e15,
            uncle_weight: 0.9e15,
            weak_rate: 0.0,
            weak_weight: 0.0,
            level: 5,
        }],
        level_pots: None,
        months: 2,
        steering: None,
    };
    let outcomes = simulate_chain_payouts(&inputs, &mut SimulationRandom::from_seed(1));
    let outcome = &outcomes[0];
    let blocks = 2.0 * MONTH_SECONDS * block_rate(1.0);
    assert!(outcome.payments.len() as f64 > 0.95 * blocks);
    assert!(
        outcome
            .payments
            .iter()
            .all(|payment| *payment == paid_block())
    );
}

#[test]
fn a_miner_alone_on_its_chain_gets_every_block_at_its_expected_work() {
    // 100,000 entries expected in each window: credited at their expected
    // work rather than simulated one by one.
    let inputs = ChainPayoutInputs {
        block_rate: block_rate(1.0),
        window: WindowGeometry {
            shares: 1_000.0,
            span_seconds: 1_000_000.0,
            skip_seconds: 1_000.0,
            work: 1.0e18,
            capped: false,
        },
        others_credited_rate: 0.0,
        tracked: vec![TrackedInputs {
            found_rate: 0.1,
            stale_rate: 0.021,
            chain_weight: 1.0e15,
            uncle_weight: 0.9e15,
            weak_rate: 0.0,
            weak_weight: 0.0,
            level: 5,
        }],
        level_pots: None,
        months: 1,
        steering: None,
    };
    let outcomes = simulate_chain_payouts(&inputs, &mut SimulationRandom::from_seed(2));
    let outcome = &outcomes[0];
    let blocks = MONTH_SECONDS * block_rate(1.0);
    assert!(outcome.payments.len() as f64 > 0.95 * blocks);
    assert!(
        outcome
            .payments
            .iter()
            .all(|payment| *payment == paid_block())
    );
    assert_eq!(outcome.found_shares, 259_200);
}

#[test]
fn a_block_pays_thirty_million_satoshis_after_the_donation() {
    // 0.3125 BCH less 1%.
    assert_eq!(paid_block(), 30_937_500);
}

#[test]
fn a_miner_with_no_entries_is_never_paid() {
    let inputs = ChainPayoutInputs {
        block_rate: block_rate(0.01),
        window: one_percent_window(),
        others_credited_rate: 3.7e16,
        tracked: vec![TrackedInputs {
            found_rate: 0.0,
            stale_rate: 0.021,
            chain_weight: 3.6e17,
            uncle_weight: 3.24e17,
            weak_rate: 0.0,
            weak_weight: 0.0,
            level: 5,
        }],
        level_pots: None,
        months: 3,
        steering: None,
    };
    let outcomes = simulate_chain_payouts(&inputs, &mut SimulationRandom::from_seed(3));
    assert!(outcomes[0].payments.is_empty());
    assert_eq!(outcomes[0].monthly_payments, vec![0, 0, 0]);
}

#[test]
fn a_small_miner_earns_its_credit_share_of_the_blocks() {
    // A Bitaxe in a pool of 1% of BCH: about 8.6 shares a month, each paid
    // about 16,700 sat by each of the three blocks it is in on average (a
    // tenth of the reward from a window of a tenth of the work, the same as
    // with ten-minute blocks).
    let found_rate = 3.3e-6;
    let chain_weight = 3.6e17;
    let uncle_weight = 0.9 * chain_weight;
    let stale_rate = 0.021;
    let credited_rate =
        found_rate * ((1.0 - stale_rate) * chain_weight + stale_rate * uncle_weight);
    let others = 3.7e16 - credited_rate;
    let inputs = ChainPayoutInputs {
        block_rate: block_rate(0.01),
        window: one_percent_window(),
        others_credited_rate: others,
        tracked: vec![TrackedInputs {
            found_rate,
            stale_rate,
            chain_weight,
            uncle_weight,
            weak_rate: 0.0,
            weak_weight: 0.0,
            level: 5,
        }],
        level_pots: None,
        months: 2_000,
        steering: None,
    };
    let outcomes = simulate_chain_payouts(&inputs, &mut SimulationRandom::from_seed(4));
    let income: f64 = outcomes[0].monthly_income.iter().sum::<f64>() / 2_000.0;
    let expected = block_rate(0.01) * MONTH_SECONDS * paid_block() as f64 * credited_rate / 3.7e16;
    assert!(
        (income / expected - 1.0).abs() < 0.04,
        "{income} {expected}"
    );
}

#[test]
fn a_share_in_the_window_pays_about_16700_satoshis_a_block() {
    let window = one_percent_window();
    let slice = paid_block() as f64 * 3.6e17 / window.work;
    assert!((slice - 16_723.0).abs() < 0.5, "{slice}");
}

#[test]
fn slices_below_the_level_are_paid_at_least_the_level_and_their_amount_on_average() {
    // The same Bitaxe at level 5 (17,472 sat): one share in the window pays
    // about 16,700 sat, below the level, so it goes to a draw with a group
    // holding a sixth of the block; two or more pay directly.
    let found_rate = 3.3e-6;
    let chain_weight = 3.6e17;
    let uncle_weight = 0.9 * chain_weight;
    let stale_rate = 0.021;
    let credited_rate =
        found_rate * ((1.0 - stale_rate) * chain_weight + stale_rate * uncle_weight);
    let mut pots = vec![0; 8];
    pots[5] = paid_block() / 6;
    let inputs = ChainPayoutInputs {
        block_rate: block_rate(0.01),
        window: one_percent_window(),
        others_credited_rate: 3.7e16 - credited_rate,
        tracked: vec![TrackedInputs {
            found_rate,
            stale_rate,
            chain_weight,
            uncle_weight,
            weak_rate: 0.0,
            weak_weight: 0.0,
            level: 5,
        }],
        level_pots: Some(pots),
        months: 2_000,
        steering: None,
    };
    let outcomes = simulate_chain_payouts(&inputs, &mut SimulationRandom::from_seed(5));
    assert!(
        outcomes[0]
            .payments
            .iter()
            .all(|payment| *payment >= 17_472)
    );
    let income: f64 = outcomes[0].monthly_income.iter().sum::<f64>() / 2_000.0;
    let expected = block_rate(0.01) * MONTH_SECONDS * paid_block() as f64 * credited_rate / 3.7e16;
    assert!(
        (income / expected - 1.0).abs() < 0.05,
        "{income} {expected}"
    );
}

#[test]
fn a_steering_miner_below_its_level_finds_a_better_draw_about_a_quarter_of_the_time() {
    // The Bitaxe of the level test: one share in the window pays about
    // 16,700 sat against a 17,472-sat level, so about 0.96 of the seeds win
    // it a part of the pot; the other parent's seed is better when the
    // first loses and the second wins, about 0.04 of the draws. The gain is
    // the difference weighted by the chance the block is its own.
    let found_rate = 3.3e-6;
    let chain_weight = 3.6e17;
    let uncle_weight = 0.9 * chain_weight;
    let stale_rate = 0.021;
    let credited_rate =
        found_rate * ((1.0 - stale_rate) * chain_weight + stale_rate * uncle_weight);
    let mut pots = vec![0; 8];
    pots[5] = paid_block() / 6;
    let inputs = ChainPayoutInputs {
        block_rate: block_rate(0.01),
        window: one_percent_window(),
        others_credited_rate: 3.7e16 - credited_rate,
        tracked: vec![TrackedInputs {
            found_rate,
            stale_rate,
            chain_weight,
            uncle_weight,
            weak_rate: 0.0,
            weak_weight: 0.0,
            level: 5,
        }],
        level_pots: Some(pots),
        months: 500,
        steering: Some(Steering {
            tracked_index: 0,
            own_block_chance: 0.5,
            choice_chance: 0.1,
        }),
    };
    let outcomes = simulate_chain_payouts(&inputs, &mut SimulationRandom::from_seed(6));
    let outcome = &outcomes[0];
    assert!(outcome.steering_draws > 1_000, "{}", outcome.steering_draws);
    let better = outcome.steering_alternative_better as f64 / outcome.steering_draws as f64;
    assert!(better > 0.02 && better < 0.5, "{better}");
    // Each better draw gains about one part of the pot, a level, times
    // 0.5 * 0.1; the first point's remainder adds less than a level.
    let per_better_draw = outcome.steering_gain / outcome.steering_alternative_better as f64;
    assert!(
        per_better_draw > 0.0 && per_better_draw <= 0.05 * 3.0 * 17_472.0,
        "{per_better_draw}"
    );
}

#[test]
fn a_steering_miner_paid_directly_is_never_in_a_draw() {
    // Paid about 38,000 sat a block, above its 17,472-sat level: its 1e14
    // credited hashes in a window of the others' and its own.
    let span = 10_000.0;
    let others = (paid_block() as f64 / 38_000.0 - 1.0) * 1.0e14 / span;
    let inputs = ChainPayoutInputs {
        block_rate: block_rate(1.0),
        window: WindowGeometry {
            shares: 1_000.0,
            span_seconds: span,
            skip_seconds: 1_000.0,
            work: 1.0e18,
            capped: false,
        },
        others_credited_rate: others,
        tracked: vec![TrackedInputs {
            found_rate: 0.01,
            stale_rate: 0.021,
            chain_weight: 1.0e12,
            uncle_weight: 0.9e12,
            weak_rate: 0.0,
            weak_weight: 0.0,
            level: 5,
        }],
        level_pots: Some(vec![1_000_000; 8]),
        months: 2,
        steering: Some(Steering {
            tracked_index: 0,
            own_block_chance: 1.0,
            choice_chance: 1.0,
        }),
    };
    let outcomes = simulate_chain_payouts(&inputs, &mut SimulationRandom::from_seed(7));
    assert_eq!(outcomes[0].steering_draws, 0);
    assert_eq!(outcomes[0].steering_gain, 0.0);
}
