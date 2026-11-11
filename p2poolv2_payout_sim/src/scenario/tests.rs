// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;

#[test]
fn the_stale_rate_is_the_network_simulations_at_ten_seconds() {
    assert_eq!(stale_rate_for_period(10.0), 0.021);
}

#[test]
fn the_stale_rate_is_interpolated_between_measured_periods() {
    assert!((stale_rate_for_period(7.5) - 0.0295).abs() < 1e-12);
}

#[test]
fn the_stale_rate_holds_beyond_the_measured_periods() {
    assert_eq!(stale_rate_for_period(0.5), 0.159);
    assert_eq!(stale_rate_for_period(60.0), 0.005);
}

#[test]
fn a_bitaxe_fair_income_is_its_share_of_the_months_blocks() {
    // 1.2 TH/s of 3.7 EH/s, 4,320 blocks of 3.125 BCH, less 1%.
    let income = fair_income_per_month(1.2e12, 3.7e18);
    assert!((income - 433_459.459).abs() < 0.01, "{income}");
}

#[test]
fn the_pool_classes_hold_the_pools_hashrate_with_the_tracked_miners() {
    let scenario = Scenario::default();
    let classes = scenario.pool_classes();
    let expected = scenario.network_hashrate() * scenario.pool_fraction;
    assert!((chain_hashrate(&classes) / expected - 1.0).abs() < 1e-12);
    assert_eq!(
        classes
            .iter()
            .filter(|class| class.kind == MinerKind::Tracked)
            .count(),
        2
    );
}

#[test]
fn non_carrying_nodes_take_their_fraction_of_every_untracked_class() {
    let scenario = Scenario {
        non_carrying_fraction: 0.5,
        ..Scenario::default()
    };
    let classes = scenario.pool_classes();
    let untracked = scenario.network_hashrate() * scenario.pool_fraction
        - TrackedMiner::bitaxe().hashrate
        - TrackedMiner::asic().hashrate;
    let non_carrying: f64 = classes
        .iter()
        .filter(|class| !class.on_carrying_node)
        .map(MinerClass::total_hashrate)
        .sum();
    assert!((non_carrying / untracked - 0.5).abs() < 1e-12);
}

#[test]
fn the_spammer_adds_its_multiple_of_the_home_hashrate() {
    let scenario = Scenario {
        spam_multiple: 10.0,
        mix: MinerMix::MostlyIndustrial,
        ..Scenario::default()
    };
    let classes = scenario.pool_classes();
    let spammer = classes
        .iter()
        .find(|class| class.kind == MinerKind::Spammer)
        .map(MinerClass::total_hashrate);
    let home: f64 = classes
        .iter()
        .filter(|class| {
            class.kind != MinerKind::Spammer && class.hashrate < MINI_THRESHOLD_HASHRATE
        })
        .map(MinerClass::total_hashrate)
        .sum();
    assert!(spammer.is_some_and(|hashrate| (hashrate / home - 10.0).abs() < 1e-12));
}

#[test]
fn the_big_node_attack_takes_the_farms_off_the_node_default() {
    let scenario = Scenario {
        big_node_at_minimum: true,
        ..Scenario::default()
    };
    let classes = scenario.pool_classes();
    assert!(
        classes
            .iter()
            .filter(|class| class.kind == MinerKind::Industrial)
            .all(|class| !class.follows_node_default)
    );
    assert!(
        classes
            .iter()
            .filter(|class| class.kind != MinerKind::Industrial)
            .all(|class| class.follows_node_default)
    );
}

#[test]
fn the_mini_design_puts_the_bitaxe_and_the_asic_on_separate_chains() {
    let scenario = Scenario {
        design: Design::Mini,
        months: 2,
        ..Scenario::default()
    };
    let report = scenario.run();
    assert_eq!(report.chains.len(), 2);
    assert_eq!(report.chains[0].name, "mini");
    assert_eq!(report.chains[1].name, "main");
    assert!((report.pool_fraction - 0.01).abs() < 1e-12);
}

#[test]
fn at_one_difficulty_expected_income_is_fair() {
    // Every share the same work: an uncle's 9/10 and its nephew's 1/10 add
    // up to each miner's own work.
    let scenario = Scenario {
        design: Design::OneChain,
        mix: MinerMix::MostlyIndustrial,
        months: 2,
        ..Scenario::default()
    };
    let report = scenario.run();
    let bitaxe = &report.miners[0];
    let asic = &report.miners[1];
    assert!((bitaxe.expected_income_per_month / bitaxe.fair_income_per_month - 1.0).abs() < 1e-9);
    assert!((asic.expected_income_per_month / asic.fair_income_per_month - 1.0).abs() < 1e-9);
}

#[test]
fn under_the_band_light_shares_collect_the_nephew_tenth_of_heavy_uncles() {
    // Heavy and light shares go stale alike, but light shares are most of
    // the chain, so they name most uncles and take 1/10 of heavy uncles'
    // work: about 0.1 * 2.1% of the farms' work moves to the home miners.
    let scenario = Scenario {
        design: Design::Band,
        mix: MinerMix::MostlyIndustrial,
        months: 2,
        ..Scenario::default()
    };
    let report = scenario.run();
    let bitaxe = &report.miners[0];
    let asic = &report.miners[1];
    let bitaxe_ratio = bitaxe.expected_income_per_month / bitaxe.fair_income_per_month;
    let asic_ratio = asic.expected_income_per_month / asic.fair_income_per_month;
    assert!(
        bitaxe_ratio > 1.005 && bitaxe_ratio < 1.03,
        "{bitaxe_ratio}"
    );
    assert!(asic_ratio > 0.995 && asic_ratio < 1.0, "{asic_ratio}");
}

#[test]
fn the_pages_weak_credit_pays_miners_at_the_minimum_about_twice() {
    // With 90% of the hashrate on heavier shares, a home miner at the
    // minimum is credited its full shares and, besides, nearly as much again
    // in weak shares: 1.97 / (0.1 * 1.97 + 0.9), about 1.8 times fair.
    let scenario = Scenario {
        design: Design::BandWeak,
        mix: MinerMix::MostlyIndustrial,
        months: 2,
        weak_queue_shares: 20_000,
        ..Scenario::default()
    };
    let report = scenario.run();
    let bitaxe = &report.miners[0];
    let asic = &report.miners[1];
    assert!(bitaxe.expected_income_per_month / bitaxe.fair_income_per_month > 1.7);
    assert!(asic.expected_income_per_month / asic.fair_income_per_month < 0.95);
}

#[test]
fn crediting_shares_on_minimum_jobs_at_the_weak_work_is_fair() {
    let scenario = Scenario {
        design: Design::BandWeak,
        mix: MinerMix::MostlyIndustrial,
        weak_credit: WeakCredit::WeakWork,
        months: 2,
        weak_queue_shares: 20_000,
        ..Scenario::default()
    };
    let report = scenario.run();
    let bitaxe = &report.miners[0];
    let asic = &report.miners[1];
    assert!((bitaxe.expected_income_per_month / bitaxe.fair_income_per_month - 1.0).abs() < 0.02);
    assert!((asic.expected_income_per_month / asic.fair_income_per_month - 1.0).abs() < 0.02);
    // The window is three blocks of real work again: 50 hours at 1%.
    assert!((report.home_chain().window.span_seconds / 180_000.0 - 1.0).abs() < 0.02);
}
