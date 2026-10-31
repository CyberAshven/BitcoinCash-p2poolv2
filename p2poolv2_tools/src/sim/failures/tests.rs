// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;

#[test]
fn targets_avoid_node_zero_and_differ() {
    let targets = select_targets(42, 10, 3);
    assert!((1..10).contains(&targets.fast));
    assert!((1..10).contains(&targets.slow));
    assert_ne!(targets.fast, targets.slow);
}

#[test]
fn partition_keeps_a_node_on_each_side() {
    let targets = select_targets(7, 6, 3);
    assert_eq!(targets.partition_width, 3);
    assert!(targets.partition_start >= 1);
    assert!(targets.partition_start + targets.partition_width < 6);
}

#[test]
fn partition_width_shrinks_for_small_swarms() {
    let targets = select_targets(1, 3, 3);
    assert_eq!(targets.partition_width, 1);
    assert_eq!(targets.partition_start, 1);
}

#[test]
fn same_seed_selects_same_targets() {
    assert_eq!(select_targets(42, 10, 3), select_targets(42, 10, 3));
}

#[test]
fn enable_emitter_flips_only_the_flag() {
    assert_eq!(enable_emitter("enabled = false"), "enabled = true");
    assert_eq!(enable_emitter("console = true"), "console = true");
}

#[test]
fn dial_node_zero_fills_an_empty_list() {
    assert_eq!(
        dial_node_zero("dial_peers = []", 7000),
        "dial_peers = [\"/ip4/127.0.0.1/tcp/7000\"]"
    );
}

#[test]
fn dial_node_zero_prepends_to_a_list() {
    assert_eq!(
        dial_node_zero("dial_peers = [\"/ip4/127.0.0.1/tcp/7006\"]", 7000),
        "dial_peers = [\"/ip4/127.0.0.1/tcp/7000\", \"/ip4/127.0.0.1/tcp/7006\"]"
    );
}

#[test]
fn clean_summary_has_no_targets() {
    let summary = build_summary("clean", SummaryParts::default());
    assert_eq!(
        summary.text,
        "=== failure injection summary (scenario: clean) ===\ntargets: none\nrejoined: 0/0\nunrecovered nodes: 0\n"
    );
    assert!(summary.passed());
}

#[test]
fn summary_lists_observations_then_rows_then_verdicts() {
    let summary = build_summary(
        "late-miner",
        SummaryParts {
            target_list: " 1".to_string(),
            observation_lines: vec!["maturity check: FAIL (x)".to_string()],
            summary_lines: vec!["node 1: rejoined=yes".to_string()],
            rejoined_ok: 1,
            rejoined_total: 1,
            maturity_verdict: Some("FAIL".to_string()),
            join_verdict: None,
            fork_verdict: None,
        },
    );
    assert_eq!(
        summary.text,
        "=== failure injection summary (scenario: late-miner) ===\ntargets: 1\nmaturity check: FAIL (x)\nnode 1: rejoined=yes\nrejoined: 1/1\nunrecovered nodes: 0\nmaturity verdict: FAIL\n"
    );
    assert_eq!(summary.targets, "1");
    assert!(!summary.passed());
}

#[test]
fn verdict_from_joins_reasons() {
    assert_eq!(verdict_from(&[]), "PASS");
    assert_eq!(
        verdict_from(&["no-refusal", "minority-stalled"]),
        "FAIL (no-refusal minority-stalled)"
    );
}
