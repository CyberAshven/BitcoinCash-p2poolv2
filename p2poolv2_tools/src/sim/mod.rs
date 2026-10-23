// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The sim swarm harness: launch a swarm of no-PoW sim nodes against one
//! regtest bitcoind, watch it, measure it, inject failures into it, stop it,
//! and run the whole thing end to end as the nightly test.
//!
//! Replaces the scripts in `load-tests/sim/`. Each submodule names the
//! script it replaces. The harness keeps their contract: configuration
//! through the same environment variables, and the same files in `RUN_DIR`
//! (`node-<i>.toml`, `node-<i>.log`, `store-<i>.db`, positional `pids.txt`,
//! `failure-results.txt`, `maturity-check.txt`).

pub mod bitcoind;
pub mod failures;
pub mod maturity;
pub mod metrics;
pub mod nightly;
pub mod node_log;
pub mod observe;
pub mod plot;
pub mod stop;
pub mod swarm;

use crate::env_vars;
use crate::error::{ToolError, ToolResult};
use std::path::{Path, PathBuf};

/// Default `RUN_DIR`.
pub const DEFAULT_RUN_DIR: &str = "/tmp/p2pool-sim";

/// The swarm's working directory and the files in it.
#[derive(Debug, Clone)]
pub struct RunDir {
    path: PathBuf,
}

impl RunDir {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// `RUN_DIR`, default `/tmp/p2pool-sim`.
    pub fn from_env() -> Self {
        Self::new(env_vars::string("RUN_DIR", DEFAULT_RUN_DIR))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn node_config(&self, index: usize) -> PathBuf {
        self.path.join(format!("node-{index}.toml"))
    }

    pub fn node_log(&self, index: usize) -> PathBuf {
        self.path.join(format!("node-{index}.log"))
    }

    pub fn node_store(&self, index: usize) -> PathBuf {
        self.path.join(format!("store-{index}.db"))
    }

    pub fn pids_file(&self) -> PathBuf {
        self.path.join("pids.txt")
    }

    /// The `pgrep -f` pattern that matches this swarm's node processes: their
    /// command line names a config in this directory. Matching on the path
    /// leaves an unrelated single-node run alone.
    pub fn process_pattern(&self) -> String {
        format!("{}/node-", self.path.display())
    }

    /// Number of node configs (`node-*.toml`) in the directory: the swarm
    /// size. Zero when the directory is missing.
    pub fn node_count(&self) -> usize {
        std::fs::read_dir(&self.path)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|entry| {
                        let name = entry.file_name();
                        let name = name.to_string_lossy();
                        name.starts_with("node-") && name.ends_with(".toml")
                    })
                    .count()
            })
            .unwrap_or(0)
    }

    /// Every `node-*.log` in the directory, sorted.
    pub fn node_logs(&self) -> Vec<PathBuf> {
        let mut logs: Vec<PathBuf> = std::fs::read_dir(&self.path)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .filter(|path| {
                        path.file_name().is_some_and(|name| {
                            let name = name.to_string_lossy();
                            name.starts_with("node-") && name.ends_with(".log")
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        logs.sort();
        logs
    }

    /// PIDs in `pids.txt`, one line per node index; an empty line is `None`.
    pub fn read_pids(&self) -> Vec<Option<u32>> {
        std::fs::read_to_string(self.pids_file())
            .map(|text| {
                text.lines()
                    .map(|line| line.trim().parse().ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The PID on line `index + 1` of `pids.txt`.
    pub fn node_pid(&self, index: usize) -> Option<u32> {
        self.read_pids().get(index).copied().flatten()
    }

    /// Replace line `index + 1` of `pids.txt` with `pid`, so later liveness
    /// checks read the restarted node's PID. Written to a temporary file and
    /// renamed, so a reader never sees a half-written file.
    pub fn set_node_pid(&self, index: usize, pid: u32) -> ToolResult<()> {
        let path = self.pids_file();
        let text = std::fs::read_to_string(&path).map_err(|error| ToolError::io("read", &path, error))?;
        let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
        if let Some(line) = lines.get_mut(index) {
            *line = pid.to_string();
        }
        let mut rewritten = lines.join("\n");
        rewritten.push('\n');
        let temporary = self.path.join(format!("pids.{}.tmp", std::process::id()));
        std::fs::write(&temporary, rewritten)
            .map_err(|error| ToolError::io("write", &temporary, error))?;
        std::fs::rename(&temporary, &path).map_err(|error| ToolError::io("replace", &path, error))
    }
}

/// Why a harness command stopped, with the exit code its script used: 2
/// for a usage or setup mistake, 1 for anything else.
#[derive(Debug)]
pub struct Exit {
    pub code: u8,
    pub message: String,
}

impl Exit {
    /// A usage or setup mistake (exit code 2).
    pub fn usage(message: impl Into<String>) -> Self {
        Self {
            code: 2,
            message: message.into(),
        }
    }

    /// A failure (exit code 1).
    pub fn failure(message: impl Into<String>) -> Self {
        Self {
            code: 1,
            message: message.into(),
        }
    }
}

impl From<ToolError> for Exit {
    fn from(error: ToolError) -> Self {
        Self::failure(error.to_string())
    }
}

/// `[HH:MM:SS] message`, the progress line format of the harness.
pub fn log_message(message: &str) {
    println!("[{}] {message}", chrono::Local::now().format("%H:%M:%S"));
}

/// The scenario names `failures` and `nightly` accept.
pub const SCENARIOS: [&str; 9] = [
    "clean",
    "restart-fast",
    "restart-delayed",
    "partition",
    "all",
    "late-miner",
    "pruned-join",
    "deep-fork-refused",
    "shallow-fork-reorged",
];

/// An error for a scenario name that is not one of [`SCENARIOS`].
pub fn check_scenario(scenario: &str) -> ToolResult<()> {
    if SCENARIOS.contains(&scenario) {
        return Ok(());
    }
    Err(ToolError::message(format!(
        "unknown scenario '{scenario}' (expected clean, restart-fast, restart-delayed, partition, all, late-miner, pruned-join, deep-fork-refused or shallow-fork-reorged)"
    )))
}

#[cfg(test)]
mod tests {
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
}
