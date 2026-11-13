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

#[test]
fn a_miner_below_ten_terahashes_is_a_home_miner() {
    assert_eq!(kind_of(9.9e12), MinerKind::Home);
}

#[test]
fn a_miner_from_ten_terahashes_to_a_petahash_is_a_medium_miner() {
    assert_eq!(kind_of(10.0e12), MinerKind::Medium);
    assert_eq!(kind_of(0.99e15), MinerKind::Medium);
}

#[test]
fn a_miner_from_a_petahash_is_industrial() {
    assert_eq!(kind_of(1.0e15), MinerKind::Industrial);
}

#[test]
fn one_home_miner_alone_is_a_zipf_spread_of_one() {
    assert_eq!(zipf_miner_count(1.2e12), (1, 1.0));
}

#[test]
fn two_zipf_miners_hold_three_halves_of_the_largest() {
    // 1.2 TH/s * 2 miners * H_2 (1.5) = 3.6 TH/s.
    assert_eq!(zipf_miner_count(3.6e12), (2, 1.5));
}

#[test]
fn the_zipf_mix_holds_its_hashrate_in_its_miner_count() {
    let hashrate = 37.0e15;
    let (count, harmonic) = zipf_miner_count(hashrate);
    let classes = MinerMix::Zipf.classes(hashrate, 5);
    let total: f64 = classes.iter().map(MinerClass::total_hashrate).sum();
    let miners: f64 = classes.iter().map(|class| class.count).sum();
    assert!((total / hashrate - 1.0).abs() < 1e-9, "{total}");
    assert_eq!(miners, count as f64);
    // The largest miner holds 1 / H_N of the hashrate.
    assert!((classes[0].hashrate - hashrate / harmonic).abs() < 1.0);
    assert_eq!(classes[0].count, 1.0);
}

#[test]
fn the_smallest_zipf_miners_are_home_miners() {
    let classes = MinerMix::Zipf.classes(37.0e15, 5);
    let smallest = &classes[classes.len() - 1];
    assert_eq!(smallest.kind, MinerKind::Home);
    assert!(smallest.hashrate < 1.6e12, "{}", smallest.hashrate);
}

#[test]
fn the_medium_mix_is_a_tenth_home_and_the_rest_medium() {
    let classes = MinerMix::Medium.classes(10.0e15, 5);
    assert_eq!(classes.len(), 6);
    assert_eq!(classes[0].kind, MinerKind::Home);
    assert!((classes[0].total_hashrate() - 1.0e15).abs() < 1.0);
    assert!((classes[1].total_hashrate() - 1.8e15).abs() < 1.0);
    assert_eq!(classes[1].hashrate, 30.0e12);
    assert_eq!(classes[5].hashrate, 300.0e12);
    assert_eq!(classes[5].kind, MinerKind::Medium);
}

#[test]
fn the_medium_few_big_mix_keeps_a_tenth_on_one_petahash_farms() {
    let classes = MinerMix::MediumFewBig.classes(10.0e15, 5);
    let farm = &classes[classes.len() - 1];
    assert_eq!(farm.kind, MinerKind::Industrial);
    assert_eq!(farm.hashrate, 1.0e15);
    assert!((farm.count - 1.0).abs() < 1e-12);
    assert!((classes[1].total_hashrate() - 1.6e15).abs() < 1.0);
}
