// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;

#[test]
fn the_home_mix_is_home_miners_only() {
    let classes = MinerMix::Home.classes(1.2e15, 5);
    assert_eq!(classes.len(), 1);
    assert_eq!(classes[0].kind, MinerKind::Home);
    assert!((classes[0].count - 1_000.0).abs() < 1e-9);
}

#[test]
fn the_half_industrial_mix_splits_the_farms_equally() {
    let classes = MinerMix::HalfIndustrial.classes(40.0e15, 5);
    assert_eq!(classes.len(), 3);
    assert!((classes[0].total_hashrate() - 20.0e15).abs() < 1.0);
    assert!((classes[1].total_hashrate() - 10.0e15).abs() < 1.0);
    assert!((classes[2].total_hashrate() - 10.0e15).abs() < 1.0);
    assert!((classes[2].count - 1.0).abs() < 1e-12);
}

#[test]
fn the_mostly_industrial_mix_keeps_a_tenth_home() {
    let classes = MinerMix::MostlyIndustrial.classes(10.0e15, 3);
    assert!((classes[0].total_hashrate() - 1.0e15).abs() < 1.0);
    assert_eq!(classes[1].level, 3);
}

#[test]
fn a_tracked_miner_is_one_miner_following_the_node_default() {
    let class = TrackedMiner::bitaxe().class();
    assert_eq!(class.kind, MinerKind::Tracked);
    assert_eq!(class.count, 1.0);
    assert!(class.follows_node_default);
    assert!(class.on_carrying_node);
}

#[test]
fn the_non_carrying_asic_is_on_a_non_carrying_node() {
    let class = TrackedMiner::non_carrying_asic().class();
    assert!(!class.on_carrying_node);
    assert_eq!(class.hashrate, ASIC_HASHRATE);
}
