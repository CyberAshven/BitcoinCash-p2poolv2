// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;

#[test]
fn sweeps_parse_by_name() {
    assert_eq!(Sweep::parse("block-size"), Ok(Sweep::BlockSize));
    assert!(Sweep::parse("everything").is_err());
}

#[test]
fn the_payee_sweep_reaches_a_million_payees_under_each_layout() {
    let scenarios = Sweep::Payees.scenarios(&Scenario::default());
    assert_eq!(scenarios.len(), 13);
    assert_eq!(scenarios[7].label, "no-tokens, 1,000,000 payees");
    assert_eq!(scenarios[10].label, "tokens-3072, 1,000,000 payees");
    assert_eq!(scenarios[11].label, "cap-11, 1,000,000 payees");
    assert_eq!(scenarios[12].label, "uncapped, 100,000 payees");
}

#[test]
fn the_relay_sweep_compares_whole_shares_with_shares_without_payouts() {
    let scenarios = Sweep::Relay.scenarios(&Scenario::default());
    assert_eq!(scenarios.len(), 6);
    assert_eq!(scenarios[0].relay, ShareRelay::Full);
    assert_eq!(scenarios[5].relay, ShareRelay::WithoutPayouts);
    assert_eq!(
        scenarios[5].label,
        "no-tokens, 29,402 payees, without-payouts relay"
    );
}

#[test]
fn the_pool_size_sweep_uses_a_million_miners() {
    let scenarios = Sweep::PoolSize.scenarios(&Scenario::default());
    assert_eq!(scenarios.len(), 12);
    assert_eq!(scenarios[0].miner_count, 1_000_000);
    assert_eq!(scenarios[0].label, "no-tokens, pool 0.1% of BCH");
}

#[test]
fn the_block_size_sweep_runs_from_chipnet_to_the_bchn_guard() {
    let scenarios = Sweep::BlockSize.scenarios(&Scenario::default());
    assert_eq!(scenarios.len(), 10);
    assert_eq!(scenarios[0].block_bytes, 2_000_000);
    assert_eq!(scenarios[4].block_bytes, 2_000_000_000);
    assert_eq!(scenarios[9].upload_megabits, Distribution::fixed(1_000.0));
}

#[test]
fn the_other_sweeps_have_their_sizes() {
    let base = Scenario::default();
    assert_eq!(Sweep::Bandwidth.scenarios(&base).len(), 15);
    assert_eq!(Sweep::Latency.scenarios(&base).len(), 4);
    assert_eq!(Sweep::Interval.scenarios(&base).len(), 10);
    assert_eq!(Sweep::Nodes.scenarios(&base).len(), 4);
    assert_eq!(Sweep::Seeds.scenarios(&base).len(), 8);
}

#[test]
fn parallel_runs_match_runs_one_by_one() {
    let base = Scenario {
        node_count: 10,
        duration_seconds: 900.0,
        ..Scenario::default()
    };
    let scenarios = Sweep::Latency.scenarios(&base);
    let parallel = run_scenarios(&scenarios);
    let sequential: Vec<Report> = scenarios.iter().map(Scenario::run).collect();
    assert_eq!(parallel, sequential);
}
