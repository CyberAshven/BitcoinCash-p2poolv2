// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;

#[test]
fn set_node_pid_rewrites_one_line() {
    let directory = tempfile::tempdir().expect("temp dir");
    let run_dir = RunDir::new(directory.path());
    std::fs::write(run_dir.pids_file(), "100\n101\n102\n").expect("write pids");
    run_dir.set_node_pid(1, 555).expect("set pid");
    assert_eq!(
        std::fs::read_to_string(run_dir.pids_file()).expect("read pids"),
        "100\n555\n102\n"
    );
}

#[test]
fn node_pid_reads_positional_lines() {
    let directory = tempfile::tempdir().expect("temp dir");
    let run_dir = RunDir::new(directory.path());
    std::fs::write(run_dir.pids_file(), "100\n\n102\n").expect("write pids");
    assert_eq!(run_dir.node_pid(0), Some(100));
    assert_eq!(run_dir.node_pid(1), None);
    assert_eq!(run_dir.node_pid(3), None);
}

#[test]
fn node_count_counts_node_configs_only() {
    let directory = tempfile::tempdir().expect("temp dir");
    let run_dir = RunDir::new(directory.path());
    std::fs::write(run_dir.node_config(0), "").expect("write config");
    std::fs::write(run_dir.node_config(1), "").expect("write config");
    std::fs::write(run_dir.node_log(0), "").expect("write log");
    assert_eq!(run_dir.node_count(), 2);
}

#[test]
fn node_count_of_missing_directory_is_zero() {
    assert_eq!(RunDir::new("/nonexistent/p2pool-sim-test").node_count(), 0);
}

#[test]
fn check_scenario_rejects_unknown_names() {
    assert!(check_scenario("restart-fast").is_ok());
    assert!(check_scenario("restart-slow").is_err());
}
