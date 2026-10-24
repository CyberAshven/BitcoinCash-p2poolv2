// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;

fn write_node(run_dir: &RunDir, index: usize, config: &str, log: &str) {
    std::fs::write(run_dir.node_config(index), config).expect("write config");
    std::fs::write(run_dir.node_log(index), log).expect("write log");
}

#[test]
fn config_value_reads_the_last_field() {
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("node-0.toml");
    std::fs::write(&path, "network_hashrate = 9\nhashrate = 1500\n").expect("write config");
    assert_eq!(config_value(&path, "hashrate"), Some("1500".to_string()));
}

#[test]
fn empty_run_is_an_error() {
    let directory = tempfile::tempdir().expect("temp dir");
    assert!(collect(&RunDir::new(directory.path())).is_err());
}

#[test]
fn report_of_a_small_run() {
    let directory = tempfile::tempdir().expect("temp dir");
    let run_dir = RunDir::new(directory.path());
    write_node(
        &run_dir,
        0,
        "hashrate = 3000\nideal_block_time_secs = 5\n",
        concat!(
            "2026-10-19T10:00:00.000000Z DEBUG Promoted block aaaaaaaaaaaaaaaa01 to confirmed height Some(1)\n",
            "2026-10-19T10:00:04.000000Z DEBUG Promoted block aaaaaaaaaaaaaaaa02 to confirmed height Some(2)\n",
            "2026-10-19T10:00:05.000000Z DEBUG Promoted block bbbbbbbbbbbbbbbb03 to confirmed height None\n",
            "2026-10-19T10:00:10.000000Z DEBUG Promoted block aaaaaaaaaaaaaaaa03 to confirmed height Some(3)\n",
            "2026-10-19T10:00:11.000000Z INFO sim stats: emitted=30 pool_difficulty=100.4\n",
            "2026-10-19T10:00:12.000000Z INFO sim stats: emitted=40 pool_difficulty=99.6\n",
            "2026-10-19T10:00:13.000000Z INFO sim block-find: submitting block\n",
        ),
    );
    write_node(
        &run_dir,
        1,
        "hashrate = 1000\n",
        concat!(
            "2026-10-19T10:00:01.000000Z DEBUG Promoted block aaaaaaaaaaaaaaaa01 to confirmed height Some(1)\n",
            "2026-10-19T10:00:12.000000Z INFO sim stats: emitted=10 pool_difficulty=99\n",
            "2026-10-19T10:00:13.000000Z WARN rejected: AsertMismatch\n",
        ),
    );
    let report = collect(&run_dir).expect("report");
    assert_eq!(
        report.text,
        concat!(
            "=== sim swarm summary (2 nodes, ideal_block_time=5s -> target 0.200/s) ===\n",
            "--- 1. convergence ---\n",
            "max confirmed height: 3   nodes within 2 of tip: 2/2   (lowest node: 1)\n",
            "distinct block hash at height 1 across nodes: 1  (1 = single converged chain)\n",
            "--- 2. main-chain share rate (node 0) ---\n",
            "promotions=3  span=10s  rate=0.300/s  (target 0.200/s)\n",
            "--- 3. ASERT pool-difficulty (node 0) ---\n",
            "first=100  last=100  min=100  max=100  samples=2\n",
            "--- 4. emission proportional to hashrate (per node) ---\n",
            "node  hashrate         hr_share%  emitted   em_share% \n",
            "0     3000             75.0       40        80.0      \n",
            "1     1000             25.0       10        20.0      \n",
            "(hr_share% and em_share% track each other once enough shares accumulate)\n",
            "--- 5. uncles / block-finds / health ---\n",
            "uncle rate (node 0): 25.0%   (1 uncles / 4 produced; main-chain length 3)\n",
            "block-finds (all nodes): 1\n",
            "rejections: AsertMismatch=1  merkle/payout=0   panicked nodes: 0\n",
        )
    );
    assert_eq!(
        report.summary,
        MetricsSummary {
            promotions: 3,
            panicked_nodes: 0,
            asert_mismatches: 1,
            merkle_payout_mismatches: 0,
            uncle_rate: Some("25.0".to_string()),
        }
    );
}
