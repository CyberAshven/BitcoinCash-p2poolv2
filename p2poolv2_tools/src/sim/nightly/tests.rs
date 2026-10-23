// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;

fn passing_evidence() -> Evidence {
    Evidence {
        alive: (3, 3),
        convergence: Some((true, "1".to_string(), "3/3".to_string())),
        metrics: Some(MetricsSummary {
            promotions: 50,
            panicked_nodes: 0,
            asert_mismatches: 0,
            merkle_payout_mismatches: 0,
            uncle_rate: Some("12.5".to_string()),
        }),
        verify: (0, 3),
        failures: Some(FailureSummary {
            text: String::new(),
            targets: "none".to_string(),
            rejoined: "0/0".to_string(),
            unrecovered: 0,
            maturity_verdict: None,
            join_verdict: None,
            fork_verdict: None,
        }),
    }
}

#[test]
fn clean_run_passes() {
    let (text, passed) = evaluate("clean", "25", &passing_evidence());
    assert!(passed);
    assert_eq!(
        text,
        concat!(
            "=== NIGHTLY SIM RESULTS (scenario: clean) ===\n",
            "  nodes alive:        PASS  (3/3)\n",
            "  chain converged:    PASS  (distinct hashes: 1, within 2 of tip: 3/3)\n",
            "  chain grew:         PASS  (promotions: 50)\n",
            "  no panics:          PASS  (panicked nodes: 0)\n",
            "  no rejections:      PASS  (AsertMismatch=0 merkle/payout=0)\n",
            "  uncle rate:         PASS  (12.5% < 25%)\n",
            "  verify_chain:       PASS  (3/3 passed)\n",
            "  failure recovery:   PASS  (targets: none, rejoined 0/0, unrecovered 0)\n",
            "  payout maturity:    SKIP  (late-miner only)\n",
            "  pruned join:        SKIP  (pruned-join only)\n",
            "  fork depth limit:   SKIP  (fork scenarios only)\n",
            "RESULT: PASS\n",
        )
    );
}

#[test]
fn uncle_rate_at_the_threshold_fails() {
    let mut evidence = passing_evidence();
    if let Some(metrics) = evidence.metrics.as_mut() {
        metrics.uncle_rate = Some("25.0".to_string());
    }
    assert!(!evaluate("clean", "25", &evidence).1);
}

#[test]
fn missing_metrics_fail_with_script_fallbacks() {
    let mut evidence = passing_evidence();
    evidence.metrics = None;
    let (text, passed) = evaluate("clean", "25", &evidence);
    assert!(!passed);
    assert!(text.contains("  no panics:          FAIL  (panicked nodes: -1)\n"));
    assert!(text.contains("  uncle rate:         PASS  (0% < 25%)\n"));
}

#[test]
fn missing_failure_summary_fails_recovery() {
    let mut evidence = passing_evidence();
    evidence.failures = None;
    let (text, passed) = evaluate("restart-fast", "25", &evidence);
    assert!(!passed);
    assert!(text.contains(
        "  failure recovery:   FAIL  (targets: unknown, rejoined unknown, unrecovered -1)\n"
    ));
}

#[test]
fn late_miner_needs_a_passing_maturity_verdict() {
    let mut evidence = passing_evidence();
    let (text, passed) = evaluate("late-miner", "25", &evidence);
    assert!(!passed);
    assert!(text.contains("  payout maturity:    FAIL  (missing)\n"));
    if let Some(summary) = evidence.failures.as_mut() {
        summary.maturity_verdict = Some("PASS".to_string());
    }
    assert!(evaluate("late-miner", "25", &evidence).1);
}

#[test]
fn deep_fork_skips_convergence() {
    let mut evidence = passing_evidence();
    evidence.convergence = None;
    let (text, _) = evaluate("deep-fork-refused", "25", &evidence);
    assert!(
        text.contains("  chain converged:    SKIP  (distinct hashes: n/a, within 2 of tip: n/a)\n")
    );
}

#[test]
fn first_share_reads_the_cli_answer() {
    let share =
        first_share(r#"{"from_height":0,"to_height":5,"shares":[{"height":5,"blockhash":"ab"}]}"#);
    assert_eq!(
        share.and_then(|share| share.get("height").and_then(Value::as_u64)),
        Some(5)
    );
}

fn nightly_settings() -> NightlySettings {
    NightlySettings {
        scenario: "clean".to_string(),
        node_count: 3,
        convergence_wait: 60,
        uncle_rate_threshold: "25".to_string(),
        bitcoind_datadir: PathBuf::from("/tmp/bitcoind-p2poolv2"),
        bitcoind_binary: None,
        rpc: RpcSettings {
            url: "http://127.0.0.1:19443".to_string(),
            user: "p2pool".to_string(),
            password: "p2pool".to_string(),
        },
        zmq: "tcp://127.0.0.1:28332".to_string(),
        run_dir: RunDir::new("/tmp/p2pool-sim"),
        profile: "release".to_string(),
    }
}

#[test]
fn bchn_mines_without_peers() {
    let arguments = bitcoind_arguments(&nightly_settings());

    assert!(arguments.contains(&"-regtest".to_string()));
    assert!(arguments.contains(&"-allowunconnectedmining".to_string()));
    assert!(arguments.contains(&"-zmqpubhashblock=tcp://127.0.0.1:28332".to_string()));
}

#[test]
fn bchn_fallback_fee_is_one_sat_per_byte() {
    let arguments = bitcoind_arguments(&nightly_settings());

    assert!(arguments.contains(&"-fallbackfee=0.00001".to_string()));
}
