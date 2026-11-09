// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;
use crate::constants::{BCH_NETWORK_HASHRATE, MINI_CEILING_DIFFICULTY};
use crate::miners::{MinerKind, MinerMix, TrackedMiner};

#[test]
fn the_node_default_aims_at_300_shares_a_month_in_a_small_pool() {
    // 1% of BCH finds 43.2 blocks a month: 4 shares a block is 172.8.
    let target = node_default_shares_per_month(0.01 * BCH_NETWORK_HASHRATE, BCH_NETWORK_HASHRATE);
    assert!((target - 300.0).abs() < 1e-9);
}

#[test]
fn the_node_default_aims_at_four_shares_a_pool_block_in_a_large_pool() {
    // 30% of BCH finds 1,296 blocks a month.
    let target = node_default_shares_per_month(0.3 * BCH_NETWORK_HASHRATE, BCH_NETWORK_HASHRATE);
    assert!((target - 5_184.0).abs() < 1e-6);
}

#[test]
fn the_uniform_rule_makes_one_chain_share_per_period() {
    let pool = 0.01 * BCH_NETWORK_HASHRATE;
    let classes = MinerMix::HalfIndustrial.classes(pool, 5);
    let parameters = ChainParameters {
        period_seconds: 10.0,
        stale_rate: 0.021,
        ceiling_difficulty: MINI_CEILING_DIFFICULTY,
        network_hashrate: BCH_NETWORK_HASHRATE,
    };
    let solution = solve_chain(&classes, &parameters, DifficultyRule::Uniform);
    let expected = pool * 10.0 * (1.0 - 0.021) / HASHES_PER_DIFFICULTY;
    assert!((solution.asert_difficulty / expected - 1.0).abs() < 1e-12);
    assert!((solution.chain_share_rate - 0.1).abs() < 1e-12);
}

#[test]
fn the_uniform_rule_stops_at_the_ceiling_in_a_tiny_pool() {
    // 1 GH/s would need difficulty 2.3 for 10-second shares; the ceiling is
    // 4.55, so shares come slower.
    let classes = MinerMix::Home.classes(1.0e9, 5);
    let parameters = ChainParameters {
        period_seconds: 10.0,
        stale_rate: 0.021,
        ceiling_difficulty: MINI_CEILING_DIFFICULTY,
        network_hashrate: BCH_NETWORK_HASHRATE,
    };
    let solution = solve_chain(&classes, &parameters, DifficultyRule::Uniform);
    assert_eq!(solution.asert_difficulty, MINI_CEILING_DIFFICULTY);
    assert!(solution.chain_share_rate < 0.1);
}

#[test]
fn a_band_capped_at_one_is_the_uniform_rule() {
    let pool = 0.01 * BCH_NETWORK_HASHRATE;
    let classes = MinerMix::MostlyIndustrial.classes(pool, 5);
    let parameters = ChainParameters {
        period_seconds: 10.0,
        stale_rate: 0.021,
        ceiling_difficulty: MINI_CEILING_DIFFICULTY,
        network_hashrate: BCH_NETWORK_HASHRATE,
    };
    let band = solve_chain(&classes, &parameters, DifficultyRule::Band { cap: 1.0 });
    let uniform = solve_chain(&classes, &parameters, DifficultyRule::Uniform);
    assert!((band.asert_difficulty / uniform.asert_difficulty - 1.0).abs() < 1e-9);
}

#[test]
fn the_band_minimum_makes_one_chain_share_per_period() {
    let pool = 0.01 * BCH_NETWORK_HASHRATE;
    let classes = MinerMix::MostlyIndustrial.classes(pool, 5);
    let parameters = ChainParameters {
        period_seconds: 10.0,
        stale_rate: 0.021,
        ceiling_difficulty: MINI_CEILING_DIFFICULTY,
        network_hashrate: BCH_NETWORK_HASHRATE,
    };
    let solution = solve_chain(&classes, &parameters, DifficultyRule::Band { cap: 64.0 });
    assert!((solution.chain_share_rate / 0.1 - 1.0).abs() < 1e-9);
}

#[test]
fn the_band_lowers_the_minimum_when_big_miners_mine_heavier_shares() {
    let pool = 0.01 * BCH_NETWORK_HASHRATE;
    let classes = MinerMix::MostlyIndustrial.classes(pool, 5);
    let parameters = ChainParameters {
        period_seconds: 10.0,
        stale_rate: 0.021,
        ceiling_difficulty: MINI_CEILING_DIFFICULTY,
        network_hashrate: BCH_NETWORK_HASHRATE,
    };
    let band = solve_chain(&classes, &parameters, DifficultyRule::Band { cap: 64.0 });
    let uniform = solve_chain(&classes, &parameters, DifficultyRule::Uniform);
    assert!(band.asert_difficulty < 0.2 * uniform.asert_difficulty);
    // The home miners stay at the minimum; both farms mine heavier.
    assert_eq!(band.class_at_minimum, vec![true, false, false]);
}

#[test]
fn a_miner_inside_the_band_makes_the_node_default_target() {
    // At 1% of BCH with 90% industrial the ASIC wants about 21 times the
    // minimum, inside a cap of 64, so it makes the default's 300 a month.
    let pool = 0.01 * BCH_NETWORK_HASHRATE;
    let asic = TrackedMiner::asic().class();
    let mut classes = MinerMix::MostlyIndustrial.classes(pool - asic.hashrate, 5);
    classes.push(asic);
    let parameters = ChainParameters {
        period_seconds: 10.0,
        stale_rate: 0.021,
        ceiling_difficulty: MINI_CEILING_DIFFICULTY,
        network_hashrate: BCH_NETWORK_HASHRATE,
    };
    let solution = solve_chain(&classes, &parameters, DifficultyRule::Band { cap: 64.0 });
    let asic_index = classes.len() - 1;
    let multiple = solution.class_difficulty[asic_index] / solution.asert_difficulty;
    assert!(multiple > 1.0 && multiple < 64.0, "multiple {multiple}");
    let shares_per_month =
        found_share_rate(&classes[asic_index], solution.class_difficulty[asic_index])
            * MONTH_SECONDS;
    assert!((shares_per_month - 300.0).abs() < 1e-6);
}

#[test]
fn a_miner_that_ignores_the_default_mines_at_the_minimum() {
    let pool = 0.01 * BCH_NETWORK_HASHRATE;
    let classes: Vec<MinerClass> = MinerMix::MostlyIndustrial
        .classes(pool, 5)
        .into_iter()
        .map(|class| MinerClass {
            follows_node_default: class.kind != MinerKind::Industrial,
            ..class
        })
        .collect();
    let parameters = ChainParameters {
        period_seconds: 10.0,
        stale_rate: 0.021,
        ceiling_difficulty: MINI_CEILING_DIFFICULTY,
        network_hashrate: BCH_NETWORK_HASHRATE,
    };
    let band = solve_chain(&classes, &parameters, DifficultyRule::Band { cap: 64.0 });
    let uniform = solve_chain(&classes, &parameters, DifficultyRule::Uniform);
    assert_eq!(band.class_at_minimum, vec![true, true, true]);
    assert!((band.asert_difficulty / uniform.asert_difficulty - 1.0).abs() < 1e-9);
}

#[test]
fn the_band_cap_rises_at_one_when_most_shares_want_more() {
    // 90% of the hashrate wants heavier shares: at C = 1 about 90% of the
    // shares push the cap up and 10% pull it down.
    let pool = 0.01 * BCH_NETWORK_HASHRATE;
    let classes = MinerMix::MostlyIndustrial.classes(pool, 5);
    let parameters = ChainParameters {
        period_seconds: 10.0,
        stale_rate: 0.021,
        ceiling_difficulty: MINI_CEILING_DIFFICULTY,
        network_hashrate: BCH_NETWORK_HASHRATE,
    };
    let drift = band_cap_drift(&classes, &parameters, 1.0);
    assert!((drift - 0.8).abs() < 1e-6, "drift {drift}");
}

#[test]
fn the_band_cap_settles_at_its_limit_with_ninety_percent_industrial() {
    let pool = 0.01 * BCH_NETWORK_HASHRATE;
    let classes = MinerMix::MostlyIndustrial.classes(pool, 5);
    let parameters = ChainParameters {
        period_seconds: 10.0,
        stale_rate: 0.021,
        ceiling_difficulty: MINI_CEILING_DIFFICULTY,
        network_hashrate: BCH_NETWORK_HASHRATE,
    };
    assert_eq!(settled_band_cap(&classes, &parameters, 64.0), 64.0);
}

#[test]
fn the_band_cap_stays_near_one_with_half_the_hashrate_industrial() {
    // Half the shares push the cap up at C = 1 and half pull it down; any
    // rise makes the heavy miners' shares fewer, so it falls back.
    let pool = 0.01 * BCH_NETWORK_HASHRATE;
    let classes = MinerMix::HalfIndustrial.classes(pool, 5);
    let parameters = ChainParameters {
        period_seconds: 10.0,
        stale_rate: 0.021,
        ceiling_difficulty: MINI_CEILING_DIFFICULTY,
        network_hashrate: BCH_NETWORK_HASHRATE,
    };
    let settled = settled_band_cap(&classes, &parameters, 64.0);
    assert!(settled < 1.01, "settled {settled}");
}

#[test]
fn vardiff_gives_every_miner_the_same_share_rate() {
    // 90% industrial at 1% of BCH: about 31,000 miners, each holding more
    // than the presence minimum of the floor window.
    let pool = 0.01 * BCH_NETWORK_HASHRATE;
    let bitaxe = TrackedMiner::bitaxe().class();
    let asic = TrackedMiner::asic().class();
    let mut classes = MinerMix::MostlyIndustrial.classes(pool - bitaxe.hashrate - asic.hashrate, 5);
    classes.push(bitaxe);
    classes.push(asic);
    let parameters = ChainParameters {
        period_seconds: 10.0,
        stale_rate: 0.021,
        ceiling_difficulty: MINI_CEILING_DIFFICULTY,
        network_hashrate: BCH_NETWORK_HASHRATE,
    };
    let solution = solve_chain(&classes, &parameters, DifficultyRule::Vardiff);
    let bitaxe_rate = found_share_rate(&classes[3], solution.class_difficulty[3]);
    let asic_rate = found_share_rate(&classes[4], solution.class_difficulty[4]);
    assert!((bitaxe_rate / asic_rate - 1.0).abs() < 1e-9);
    assert!((solution.chain_share_rate / 0.1 - 1.0).abs() < 1e-9);
    assert_eq!(solution.vardiff_floor, 0.0);
}

#[test]
fn vardiff_with_too_many_miners_for_the_presence_minimum_floors_them() {
    // 308,000 equal home miners hold under one entry each of the 120,960
    // entries: the floor binds and every share is at the uniform
    // difficulty.
    let pool = 0.1 * BCH_NETWORK_HASHRATE;
    let classes = MinerMix::Home.classes(pool, 5);
    let parameters = ChainParameters {
        period_seconds: 10.0,
        stale_rate: 0.021,
        ceiling_difficulty: MINI_CEILING_DIFFICULTY,
        network_hashrate: BCH_NETWORK_HASHRATE,
    };
    let solution = solve_chain(&classes, &parameters, DifficultyRule::Vardiff);
    let uniform = solve_chain(&classes, &parameters, DifficultyRule::Uniform);
    assert!(solution.vardiff_floor > 0.0);
    assert!((solution.easiest_difficulty() / uniform.asert_difficulty - 1.0).abs() < 1e-9);
    assert!((solution.chain_share_rate / 0.1 - 1.0).abs() < 1e-9);
}
