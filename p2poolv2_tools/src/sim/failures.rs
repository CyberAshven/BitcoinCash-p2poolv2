// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Failure-mode injection for a running sim swarm. Replaces
//! `load-tests/sim/failures.sh`.
//!
//! Runs one named scenario against a swarm already launched by `run-swarm`:
//! kills nodes with SIGKILL, brings them back, and verifies each one rejoins
//! and catches up to the rest of the swarm. Writes a plain-text summary to
//! stdout and to `RUN_DIR/failure-results.txt`; the nightly reads its
//! verdict from the same summary.
//!
//! Node 0 is never a target: `metrics` derives the share rate and uncle
//! rate from `node-0.log` alone, so killing it would corrupt the run's own
//! metrics.
//!
//! Observation is from logs and the API only: RocksDB is locked while a
//! node runs, so the store-based checks in the nightly cannot be used
//! mid-run.

use crate::cargo_build::profile_binary;
use crate::env_vars;
use crate::error::{ToolError, ToolResult};
use crate::log_file::for_each_line;
use crate::node_api::NodeApi;
use crate::process::{ProcessTable, Signal, send_signal};
use crate::repo;
use crate::seeded::SeededSequence;
use crate::sim::bitcoind::{Bitcoind, RpcSettings};
use crate::sim::maturity::{MaturityCheck, Verdict};
use crate::sim::node_log::tip_height;
use crate::sim::swarm::start_node;
use crate::sim::{Exit, RunDir, check_scenario, log_message};
use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

static PORT_IN_ADDRESS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/tcp/([0-9]+)").expect("valid regex"));
static LATE_MATURITY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"Sim depths: coinbase maturity ([0-9]+) shares").expect("valid regex")
});
static PRUNE_DEPTH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"Sim depths: .*prune depth ([0-9]+) shares").expect("valid regex")
});
static PRUNE_INTERVAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"Sim depths: .*prune interval ([0-9]+) shares").expect("valid regex")
});

/// The scenario's settings, from the same environment variables
/// `failures.sh` read.
#[derive(Debug, Clone)]
pub struct FailureSettings {
    pub scenario: String,
    pub run_dir: RunDir,
    pub base_p2p: u32,
    pub base_api: u32,
    pub profile: String,
    pub sim_binary: Option<PathBuf>,
    pub dial_fanout: usize,
    pub seed: u64,
    pub warmup: u64,
    pub fast_down: u64,
    pub slow_down: u64,
    pub partition_down: u64,
    pub catchup_timeout: u64,
    pub late_miner: Option<usize>,
    pub late_miner_tip_timeout: u64,
    pub late_miner_check_timeout: u64,
    pub pruned_join_tip_timeout: u64,
    pub fork_group_from: Option<usize>,
    pub fork_tip_timeout: u64,
    pub fork_observe_seconds: u64,
    pub rpc: RpcSettings,
}

impl FailureSettings {
    pub fn from_env(scenario: &str) -> ToolResult<Self> {
        Ok(Self {
            scenario: scenario.to_string(),
            run_dir: RunDir::from_env(),
            base_p2p: env_vars::parsed("BASE_P2P", 7000)?,
            base_api: env_vars::parsed("BASE_API", 7600)?,
            profile: env_vars::string("PROFILE", "release"),
            sim_binary: env_vars::optional_string("SIM_BIN").map(PathBuf::from),
            dial_fanout: env_vars::parsed("DIAL_FANOUT", 3)?,
            seed: env_vars::parsed("FAILURE_SEED", 42)?,
            warmup: env_vars::parsed("FAILURE_WARMUP", 60)?,
            fast_down: env_vars::parsed("FAILURE_FAST_DOWN", 10)?,
            slow_down: env_vars::parsed("FAILURE_SLOW_DOWN", 330)?,
            partition_down: env_vars::parsed("FAILURE_PARTITION_DOWN", 90)?,
            catchup_timeout: env_vars::parsed("FAILURE_CATCHUP_TIMEOUT", 240)?,
            late_miner: env_vars::optional("LATE_MINER")?,
            late_miner_tip_timeout: env_vars::parsed("LATE_MINER_TIP_TIMEOUT", 900)?,
            late_miner_check_timeout: env_vars::parsed("LATE_MINER_CHECK_TIMEOUT", 900)?,
            pruned_join_tip_timeout: env_vars::parsed("PRUNED_JOIN_TIP_TIMEOUT", 900)?,
            fork_group_from: env_vars::optional("FORK_GROUP_FROM")?,
            fork_tip_timeout: env_vars::parsed("FORK_TIP_TIMEOUT", 900)?,
            fork_observe_seconds: env_vars::parsed("FORK_OBSERVE_SECS", 90)?,
            rpc: RpcSettings::from_env(),
        })
    }
}

/// Which nodes the scenarios hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Targets {
    pub fast: usize,
    pub slow: usize,
    pub partition_start: usize,
    pub partition_width: usize,
}

/// Seeded target selection, so a failing nightly is reproducible. Targets
/// are drawn from `[1, N-1]`; the partition cut keeps at least one node on
/// each side of it. Needs at least three nodes.
pub fn select_targets(seed: u64, node_total: usize, dial_fanout: usize) -> Targets {
    let mut sequence = SeededSequence::new(seed);
    let candidates = node_total.saturating_sub(1) as u64;
    let fast = 1 + sequence.next_below(candidates) as usize;
    let mut slow = 1 + sequence.next_below(candidates) as usize;
    // Redraw on a collision; with three or more nodes there is always
    // another candidate, and the attempts are bounded regardless.
    let mut attempts = 0;
    while slow == fast && node_total > 2 && attempts < 1000 {
        slow = 1 + sequence.next_below(candidates) as usize;
        attempts += 1;
    }
    let width = dial_fanout.min(node_total.saturating_sub(2)).max(1);
    let max_start = node_total.saturating_sub(width + 1).max(1);
    let partition_start = 1 + sequence.next_below(max_start as u64) as usize;
    Targets {
        fast,
        slow,
        partition_start,
        partition_width: width,
    }
}

/// The parsed summary a scenario run ends with.
#[derive(Debug, Clone, PartialEq)]
pub struct FailureSummary {
    pub text: String,
    /// The `targets:` value, such as ` 3 5` or ` none`.
    pub targets: String,
    pub rejoined: String,
    pub unrecovered: usize,
    pub maturity_verdict: Option<String>,
    pub join_verdict: Option<String>,
    pub fork_verdict: Option<String>,
}

impl FailureSummary {
    /// Every killed node recovered and every scenario verdict passed.
    pub fn passed(&self) -> bool {
        let passed_or_absent = |verdict: &Option<String>| verdict.as_deref().is_none_or(|verdict| verdict == "PASS");
        self.unrecovered == 0
            && passed_or_absent(&self.maturity_verdict)
            && passed_or_absent(&self.join_verdict)
            && passed_or_absent(&self.fork_verdict)
    }
}

/// `" 1 2 3"`: each index with a leading space, as the script built lists.
fn spaced(indexes: &[usize]) -> String {
    indexes.iter().map(|index| format!(" {index}")).collect()
}

fn time_of_day() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
}

/// Append `line` to the log at `path`. The markers written into node logs
/// must not contain "error" or "panic": `observe` greps for those.
fn append_line(path: &Path, line: &str) {
    let appended = std::fs::File::options()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| writeln!(file, "{line}"));
    if let Err(error) = appended {
        log_message(&format!("  cannot append to {}: {error}", path.display()));
    }
}

/// The first number `pattern` captures in the log at `path`.
fn first_capture(path: &Path, pattern: &Regex) -> Option<i64> {
    let mut found: Option<i64> = None;
    let scanned = for_each_line(path, |line| {
        if found.is_none()
            && let Some(captures) = pattern.captures(line)
        {
            found = captures[1].parse().ok();
        }
    });
    scanned.ok().and(found)
}

/// Number of lines in the log at `path` containing `needle`; 0 for a
/// missing log.
fn count_lines(path: &Path, needle: &str) -> usize {
    let mut count = 0;
    let _ = for_each_line(path, |line| count += usize::from(line.contains(needle)));
    count
}

/// Rewrite a config a line at a time, writing to a temporary file and
/// renaming it over the original.
fn rewrite_config(path: &Path, rewrite: impl Fn(&str) -> String) -> ToolResult<()> {
    let text = std::fs::read_to_string(path).map_err(|error| ToolError::io("read", path, error))?;
    let mut rewritten = String::with_capacity(text.len() + 64);
    for line in text.lines() {
        rewritten.push_str(&rewrite(line));
        rewritten.push('\n');
    }
    let temporary = path.with_extension(format!("toml.{}.tmp", std::process::id()));
    std::fs::write(&temporary, rewritten).map_err(|error| ToolError::io("write", &temporary, error))?;
    std::fs::rename(&temporary, path).map_err(|error| ToolError::io("replace", path, error))
}

/// The late node's config with `[sim] enabled = true`, so its next start
/// runs the emitter.
pub fn enable_emitter(line: &str) -> String {
    if line == "enabled = false" {
        "enabled = true".to_string()
    } else {
        line.to_string()
    }
}

/// A config line with node 0's address added to `dial_peers`, so the node's
/// next start connects the two groups.
pub fn dial_node_zero(line: &str, base_p2p: u32) -> String {
    let address = format!("\"/ip4/127.0.0.1/tcp/{base_p2p}\"");
    if line == "dial_peers = []" {
        return format!("dial_peers = [{address}]");
    }
    match line.strip_prefix("dial_peers = [") {
        Some(rest) => format!("dial_peers = [{address}, {rest}"),
        None => line.to_string(),
    }
}

/// A scenario run against a live swarm.
struct Injection<'a> {
    settings: &'a FailureSettings,
    processes: &'a mut ProcessTable,
    binary: PathBuf,
    node_total: usize,
    targets: Targets,
    api: NodeApi,
    summary_lines: Vec<String>,
    observation_lines: Vec<String>,
    target_list: String,
    rejoined_ok: usize,
    rejoined_total: usize,
    tip_at_kill: HashMap<usize, u64>,
    catchup_seconds: u64,
    reconnect_log_line: usize,
    maturity_verdict: Option<String>,
    join_verdict: Option<String>,
    fork_verdict: Option<String>,
}

impl Injection<'_> {
    fn run_dir(&self) -> &RunDir {
        &self.settings.run_dir
    }

    fn log_tip(&self, index: usize) -> u64 {
        tip_height(&self.run_dir().node_log(index))
    }

    fn swarm_max_tip(&self) -> u64 {
        (0..self.node_total)
            .map(|index| self.log_tip(index))
            .max()
            .unwrap_or(0)
    }

    fn node_alive(&mut self, index: usize) -> bool {
        match self.run_dir().node_pid(index) {
            Some(pid) => self.processes.is_alive(pid),
            None => false,
        }
    }

    /// SIGKILL a node and wait for it to go away. True when it was running
    /// and has exited.
    async fn kill_node(&mut self, index: usize) -> bool {
        let tip = self.log_tip(index);
        self.tip_at_kill.insert(index, tip);
        let pid = self.run_dir().node_pid(index);
        let Some(pid) = pid.filter(|pid| self.processes.is_alive(*pid)) else {
            log_message(&format!("  node {index} already down (no live pid)"));
            return false;
        };
        append_line(
            &self.run_dir().node_log(index),
            &format!(
                "=== failure injection: SIGKILL node {index} at {}, tip {tip} ===",
                time_of_day()
            ),
        );
        send_signal(&[pid], Signal::Kill);
        for _ in 0..10 {
            if !self.processes.is_alive(pid) {
                log_message(&format!("  killed node {index} (pid {pid}, tip {tip})"));
                return true;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        log_message(&format!(
            "  node {index} (pid {pid}) did not exit after SIGKILL"
        ));
        false
    }

    async fn kill_nodes(&mut self, indexes: &[usize]) {
        for index in indexes {
            self.kill_node(*index).await;
        }
    }

    /// Relaunch a node against its existing config and store. The log is
    /// appended to: truncating it would destroy the promotion history
    /// `metrics` reads.
    fn restart_node(&mut self, index: usize) {
        let log = self.run_dir().node_log(index);
        append_line(
            &log,
            &format!(
                "=== failure injection: restarting node {index} at {} ===",
                time_of_day()
            ),
        );
        let config = self.run_dir().node_config(index);
        match start_node(self.processes, &self.binary, &config, &log, true) {
            Ok(pid) => {
                if let Err(error) = self.run_dir().set_node_pid(index, pid) {
                    log_message(&format!("  cannot record pid of node {index}: {error}"));
                }
                log_message(&format!("  restarted node {index} (pid {pid})"));
            }
            // The catch-up check then sees a dead node.
            Err(error) => log_message(&format!("  cannot restart node {index}: {error}")),
        }
    }

    /// Poll a restarted node's log until its confirmed tip reaches
    /// `target`. Sets `catchup_seconds`. False on timeout or if the process
    /// died.
    async fn wait_for_catchup(&mut self, index: usize, target: u64, timeout: u64) -> bool {
        let start = Instant::now();
        loop {
            self.catchup_seconds = start.elapsed().as_secs();
            let height = self.log_tip(index);
            if height >= target {
                return true;
            }
            if !self.node_alive(index) {
                log_message(&format!(
                    "  node {index} died while catching up (tip {height}, target {target})"
                ));
                return false;
            }
            if self.catchup_seconds >= timeout {
                log_message(&format!(
                    "  node {index} did not reach height {target} within {timeout}s (tip {height})"
                ));
                return false;
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }

    /// Restart every node in `indexes` and verify each one catches up to
    /// where the surviving swarm has got to.
    async fn restart_and_verify(&mut self, indexes: &[usize]) {
        let target = self.swarm_max_tip();
        let names: Vec<String> = indexes.iter().map(usize::to_string).collect();
        log_message(&format!(
            "Restarting node(s) '{}'; catch-up target is height {target}",
            names.join(" ")
        ));
        for index in indexes {
            self.restart_node(*index);
        }
        for index in indexes {
            let killed_at = self.tip_at_kill.get(index).copied().unwrap_or(0);
            let gap = target as i64 - killed_at as i64;
            let rejoined = self
                .wait_for_catchup(*index, target, self.settings.catchup_timeout)
                .await;
            if rejoined {
                self.rejoined_ok += 1;
                log_message(&format!(
                    "  node {index} caught up to height {target} in {}s (gap {gap} heights)",
                    self.catchup_seconds
                ));
            }
            self.rejoined_total += 1;
            self.summary_lines.push(format!(
                "node {index}: rejoined={} catchup_secs={} gap_heights={gap} target_height={target}",
                if rejoined { "yes" } else { "no" },
                self.catchup_seconds
            ));
        }
    }

    /// Listen ports of the peers a node has dialled out to. Inbound entries
    /// carry an ephemeral port, so only outbound links map back to a node,
    /// which is enough: every link is outbound from one of its two ends.
    async fn outbound_peer_ports(&self, index: usize) -> Vec<u32> {
        let port = self.settings.base_api + index as u32;
        let Some(Value::Array(peers)) = self.api.get_any_status(port, "/peers").await else {
            return Vec::new();
        };
        peers
            .iter()
            .filter(|peer| peer.get("direction").and_then(Value::as_str) == Some("Outbound"))
            .filter_map(|peer| peer.get("address").and_then(Value::as_str))
            .flat_map(|address| {
                PORT_IN_ADDRESS
                    .captures_iter(address)
                    .filter_map(|found| found[1].parse().ok())
                    .collect::<Vec<u32>>()
            })
            .collect()
    }

    /// Links from the nodes in `sources` to the nodes in `targets`.
    async fn count_links_between(&self, sources: &[usize], targets: &[usize]) -> usize {
        let target_ports: Vec<u32> = targets
            .iter()
            .map(|index| self.settings.base_p2p + *index as u32)
            .collect();
        let mut links = 0;
        for source in sources {
            links += self
                .outbound_peer_ports(*source)
                .await
                .iter()
                .filter(|port| target_ports.contains(port))
                .count();
        }
        links
    }

    async fn restart_fast(&mut self) {
        let target = self.targets.fast;
        log_message(&format!(
            "--- restart-fast: node {target} down for {}s ---",
            self.settings.fast_down
        ));
        self.target_list.push_str(&spaced(&[target]));
        self.kill_nodes(&[target]).await;
        tokio::time::sleep(Duration::from_secs(self.settings.fast_down)).await;
        self.restart_and_verify(&[target]).await;
    }

    async fn restart_delayed(&mut self) {
        let target = self.targets.slow;
        log_message(&format!(
            "--- restart-delayed: node {target} down for {}s ---",
            self.settings.slow_down
        ));
        self.target_list.push_str(&spaced(&[target]));
        self.kill_nodes(&[target]).await;
        tokio::time::sleep(Duration::from_secs(self.settings.slow_down)).await;
        self.restart_and_verify(&[target]).await;
    }

    /// Kill the nodes that bridge the dial chain, so nodes below and above
    /// the cut have no configured path to each other, then bring them back.
    /// Whether this is a real partition depends on Kademlia, which discovers
    /// and dials peers that were never in `dial_peers`; the observation
    /// lines record what actually happened.
    async fn partition(&mut self) {
        let start = self.targets.partition_start;
        let end = start + self.targets.partition_width;
        let killed: Vec<usize> = (start..end).collect();
        let lower: Vec<usize> = (0..start).collect();
        let upper: Vec<usize> = (end..self.node_total).collect();
        let down = self.settings.partition_down;
        log_message(&format!(
            "--- partition: nodes{} down for {down}s (lower side{}, upper side{}) ---",
            spaced(&killed),
            spaced(&lower),
            spaced(&upper)
        ));
        self.target_list.push_str(&spaced(&killed));
        self.kill_nodes(&killed).await;

        let mut elapsed = 0;
        for sample in [15u64, 45] {
            if sample <= down {
                tokio::time::sleep(Duration::from_secs(sample - elapsed)).await;
                elapsed = sample;
                let lower_to_upper = self.count_links_between(&lower, &upper).await;
                let upper_to_lower = self.count_links_between(&upper, &lower).await;
                self.observation_lines.push(format!(
                    "cross-side links at +{sample}s: lower_to_upper={lower_to_upper} upper_to_lower={upper_to_lower}"
                ));
                log_message(&format!(
                    "  cross-side links at +{sample}s: lower->upper={lower_to_upper} upper->lower={upper_to_lower}"
                ));
            }
        }
        if down > elapsed {
            tokio::time::sleep(Duration::from_secs(down - elapsed)).await;
        }
        self.restart_and_verify(&killed).await;
    }

    /// Poll until the swarm tip reaches `target`. False on timeout.
    async fn wait_for_swarm_height(&self, target: u64, timeout: u64) -> bool {
        let start = Instant::now();
        while self.swarm_max_tip() < target {
            if start.elapsed().as_secs() >= timeout {
                log_message(&format!(
                    "  swarm did not reach height {target} within {timeout}s (tip {})",
                    self.swarm_max_tip()
                ));
                return false;
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
        true
    }

    fn late_miner_maturity(&self, late: usize) -> Option<i64> {
        first_capture(&self.run_dir().node_log(late), &LATE_MATURITY)
    }

    /// Run the maturity check until it gives a verdict other than
    /// inconclusive, or the timeout passes. Keeps the last report in
    /// `RUN_DIR/maturity-check.txt`.
    async fn wait_for_maturity_verdict(&mut self) -> ToolResult<String> {
        let check = MaturityCheck {
            run_dir: self.run_dir().clone(),
            late_miner: self.settings.late_miner,
            base_api: self.settings.base_api,
        };
        let bitcoind = Bitcoind::new(&self.settings.rpc)?;
        let report_path = self.run_dir().path().join("maturity-check.txt");
        let start = Instant::now();
        loop {
            let (verdict, report) = check.run(&bitcoind).await;
            std::fs::write(&report_path, &report)
                .map_err(|error| ToolError::io("write", &report_path, error))?;
            if verdict != Verdict::Inconclusive {
                return Ok(verdict.label().to_string());
            }
            if start.elapsed().as_secs() >= self.settings.late_miner_check_timeout {
                log_message(&format!(
                    "  no maturity verdict within {}s",
                    self.settings.late_miner_check_timeout
                ));
                return Ok("INCONCLUSIVE".to_string());
            }
            tokio::time::sleep(Duration::from_secs(15)).await;
        }
    }

    /// Payout maturity. The late node has synced but mined nothing. Once the
    /// chain is two maturity depths deep, so the payout skip is at its full
    /// size, restart it with the emitter on and check the regtest coinbases
    /// pay it only once its shares are mature.
    async fn late_miner(&mut self, late: usize) -> ToolResult<()> {
        let maturity = self.late_miner_maturity(late).unwrap_or(0);
        let target = (2 * maturity).max(0) as u64;
        log_message(&format!(
            "--- late-miner: node {late} starts mining once the tip reaches {target} (maturity {maturity}) ---"
        ));
        self.target_list.push_str(&spaced(&[late]));
        if !self
            .wait_for_swarm_height(target, self.settings.late_miner_tip_timeout)
            .await
        {
            self.maturity_verdict = Some("INCONCLUSIVE".to_string());
            return Ok(());
        }
        self.kill_nodes(&[late]).await;
        rewrite_config(&self.run_dir().node_config(late), enable_emitter)?;
        self.restart_and_verify(&[late]).await;
        let verdict = self.wait_for_maturity_verdict().await?;
        let report_path = self.run_dir().path().join("maturity-check.txt");
        log_message(&format!(
            "  maturity check: {verdict} (report in {})",
            report_path.display()
        ));
        self.maturity_verdict = Some(verdict);
        let report = std::fs::read_to_string(&report_path).unwrap_or_default();
        self.observation_lines
            .extend(report.lines().map(str::to_string));
        Ok(())
    }

    fn sim_prune_depths(&self) -> Option<(i64, i64)> {
        let log = self.run_dir().node_log(0);
        Some((
            first_capture(&log, &PRUNE_DEPTH)?,
            first_capture(&log, &PRUNE_INTERVAL)?,
        ))
    }

    /// Number of nodes whose log shows a completed block body pruning run.
    fn count_pruned_nodes(&self) -> usize {
        (0..self.node_total)
            .filter(|index| {
                count_lines(&self.run_dir().node_log(*index), "Pruned block bodies at heights") > 0
            })
            .count()
    }

    /// Poll until every node has logged a pruning run. False on timeout.
    async fn wait_for_all_nodes_pruned(&self, timeout: u64) -> bool {
        let start = Instant::now();
        while self.count_pruned_nodes() < self.node_total {
            if start.elapsed().as_secs() >= timeout {
                log_message(&format!(
                    "  only {}/{} nodes pruned within {timeout}s",
                    self.count_pruned_nodes(),
                    self.node_total
                ));
                return false;
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
        true
    }

    /// The confirmed blockhash node `index`'s API reports at `height`.
    async fn api_confirmed_hash(&self, index: usize, height: i64) -> Option<String> {
        if height < 0 {
            return None;
        }
        let port = self.settings.base_api + index as u32;
        let shares = self
            .api
            .get_any_status(port, &format!("/share?height={height}"))
            .await?;
        shares.as_array()?.iter().find_map(|share| {
            let chain = share.get("chain")?.as_str()?;
            if chain.eq_ignore_ascii_case("confirmed") {
                share.get("blockhash")?.as_str().map(str::to_string)
            } else {
                None
            }
        })
    }

    /// A fresh node joins a swarm that has pruned. Once the swarm is past
    /// the sim prune target and every node has pruned, wipe one node's store
    /// and restart it, so it syncs from genesis off peers that hold headers
    /// for every height but bodies only near the tip. The old log is moved
    /// aside, outside the `node-*.log` names, so the fresh node's catch-up
    /// is read from its own log.
    async fn pruned_join(&mut self) -> ToolResult<()> {
        let Some((depth, interval)) = self.sim_prune_depths() else {
            log_message("  node 0 has not logged its sim depths; is it a sim build?");
            self.join_verdict = Some("INCONCLUSIVE".to_string());
            return Ok(());
        };
        let tip_target = (depth + 3 * interval).max(0) as u64;
        let index = self.targets.slow;
        log_message(&format!(
            "--- pruned-join: node {index} is wiped once the tip reaches {tip_target} and every node has pruned (prune depth {depth}, interval {interval}) ---"
        ));
        self.target_list.push_str(&spaced(&[index]));
        let timeout = self.settings.pruned_join_tip_timeout;
        if !self.wait_for_swarm_height(tip_target, timeout).await
            || !self.wait_for_all_nodes_pruned(timeout).await
        {
            self.join_verdict = Some("INCONCLUSIVE".to_string());
            return Ok(());
        }
        self.observation_lines.push(format!(
            "nodes pruned before the join: {}/{}",
            self.count_pruned_nodes(),
            self.node_total
        ));

        self.kill_nodes(&[index]).await;
        let store = self.run_dir().node_store(index);
        if store.exists() {
            std::fs::remove_dir_all(&store).map_err(|error| ToolError::io("remove", &store, error))?;
        }
        let log = self.run_dir().node_log(index);
        let moved = self.run_dir().path().join(format!("prejoin-node-{index}.log"));
        std::fs::rename(&log, &moved).map_err(|error| ToolError::io("move", &log, error))?;
        log_message(&format!(
            "  wiped node {index} store and moved its log to prejoin-node-{index}.log"
        ));
        self.restart_and_verify(&[index]).await;

        // One height the joiner holds header-only, below its own prune
        // height, and one near the tip it holds with a body.
        let low_height = interval;
        let high_height = self.log_tip(index) as i64 - interval;
        let joiner_low = self.api_confirmed_hash(index, low_height).await;
        let peer_low = self.api_confirmed_hash(0, low_height).await;
        let joiner_high = self.api_confirmed_hash(index, high_height).await;
        let peer_high = self.api_confirmed_hash(0, high_height).await;
        let not_found = count_lines(&log, "Received not found response");
        let failed_stores = count_lines(&log, "Failed to store and organise block");
        let none = || "none".to_string();
        self.observation_lines.push(format!(
            "joiner confirmed hash at {low_height}: {} (node 0: {})",
            joiner_low.clone().unwrap_or_else(none),
            peer_low.clone().unwrap_or_else(none)
        ));
        self.observation_lines.push(format!(
            "joiner confirmed hash at {high_height}: {} (node 0: {})",
            joiner_high.clone().unwrap_or_else(none),
            peer_high.clone().unwrap_or_else(none)
        ));
        self.observation_lines.push(format!(
            "joiner not-found responses: {not_found}, failed stores: {failed_stores}"
        ));

        let mut reasons: Vec<&str> = Vec::with_capacity(5);
        if self.rejoined_ok != self.rejoined_total {
            reasons.push("did-not-catch-up");
        }
        if joiner_low.is_none() || joiner_low != peer_low {
            reasons.push("pruned-height-mismatch");
        }
        if joiner_high.is_none() || joiner_high != peer_high {
            reasons.push("tip-height-mismatch");
        }
        if not_found != 0 {
            reasons.push("requested-pruned-bodies");
        }
        if failed_stores != 0 {
            reasons.push("failed-stores");
        }
        let verdict = verdict_from(&reasons);
        log_message(&format!("  pruned-join: {verdict}"));
        self.join_verdict = Some(verdict);
        Ok(())
    }

    /// Restart a minority node dialing the majority group. Remembers the log
    /// length before the restart, so later checks read only what the node
    /// logged after it met the majority chain.
    async fn reconnect_to_majority(&mut self, index: usize) -> ToolResult<()> {
        self.kill_nodes(&[index]).await;
        let base_p2p = self.settings.base_p2p;
        rewrite_config(&self.run_dir().node_config(index), |line| {
            dial_node_zero(line, base_p2p)
        })?;
        self.reconnect_log_line = line_count(&self.run_dir().node_log(index));
        self.restart_node(index);
        Ok(())
    }

    /// Lines node `index` logged since the reconnect containing `needle`.
    fn count_since_reconnect(&self, index: usize, needle: &str) -> usize {
        let mut line_number = 0usize;
        let mut count = 0usize;
        let _ = for_each_line(&self.run_dir().node_log(index), |line| {
            line_number += 1;
            if line_number > self.reconnect_log_line && line.contains(needle) {
                count += 1;
            }
        });
        count
    }

    /// The groups fork at launch. Once both are past the sim prune target and
    /// every node has pruned, the minority group's fork point (genesis) is
    /// below its prune target. Reconnect one minority node: the majority
    /// chain has more work, but the node must refuse to reorg onto it, keep
    /// its own chain and keep confirming, while the majority keeps advancing.
    async fn deep_fork_refused(&mut self, index: usize) -> ToolResult<()> {
        let Some((depth, interval)) = self.sim_prune_depths() else {
            log_message("  node 0 has not logged its sim depths; is it a sim build?");
            self.fork_verdict = Some("INCONCLUSIVE".to_string());
            return Ok(());
        };
        let tip_target = (depth + 3 * interval).max(0) as u64;
        log_message(&format!(
            "--- deep-fork-refused: node {index} reconnects to the majority once every node has pruned (prune depth {depth}, interval {interval}) ---"
        ));
        self.target_list.push_str(&spaced(&[index]));
        let timeout = self.settings.fork_tip_timeout;
        if !self.wait_for_swarm_height(tip_target, timeout).await
            || !self.wait_for_all_nodes_pruned(timeout).await
        {
            // A minority whose hashrate needs a share target easier than the
            // ASERT clamp mines below the ideal rate and may never reach the
            // prune target.
            log_message("  if the minority lags, raise HASHRATE (e.g. 1e13 at IDEAL_BLOCK_TIME=1)");
            self.fork_verdict = Some("INCONCLUSIVE".to_string());
            return Ok(());
        }

        let minority_tip_before = self.log_tip(index);
        let majority_tip_before = self.log_tip(0);
        let check_height = minority_tip_before as i64 - interval;
        let minority_hash_before = self.api_confirmed_hash(index, check_height).await;
        let majority_hash = self.api_confirmed_hash(0, check_height).await;
        self.observation_lines.push(format!(
            "before reconnect: minority tip {minority_tip_before}, majority tip {majority_tip_before}"
        ));

        self.reconnect_to_majority(index).await?;
        log_message(&format!(
            "  observing for {}s",
            self.settings.fork_observe_seconds
        ));
        tokio::time::sleep(Duration::from_secs(self.settings.fork_observe_seconds)).await;

        let links = self.count_links_between(&[index], &[0]).await;
        let refusals = self.count_since_reconnect(index, "branch forks below the prune target");
        let minority_hash_after = self.api_confirmed_hash(index, check_height).await;
        let minority_tip_after = self.log_tip(index);
        let majority_tip_after = self.log_tip(0);
        let shown = |hash: &Option<String>| hash.clone().unwrap_or_else(|| "none".to_string());
        self.observation_lines.push(format!(
            "after reconnect: minority tip {minority_tip_after}, majority tip {majority_tip_after}, links to node 0: {links}, refusals: {refusals}"
        ));
        self.observation_lines.push(format!(
            "confirmed hash at {check_height}: minority {} -> {}, majority {}",
            shown(&minority_hash_before),
            shown(&minority_hash_after),
            shown(&majority_hash)
        ));

        if links < 1 {
            let verdict = format!("INCONCLUSIVE (node {index} never connected to node 0)");
            log_message(&format!("  deep-fork-refused: {verdict}"));
            self.fork_verdict = Some(verdict);
            return Ok(());
        }
        let mut reasons: Vec<&str> = Vec::with_capacity(5);
        if refusals < 1 {
            reasons.push("no-refusal");
        }
        if minority_hash_before.is_none() || minority_hash_after != minority_hash_before {
            reasons.push("left-own-branch");
        }
        if minority_hash_before == majority_hash {
            reasons.push("groups-did-not-fork");
        }
        if minority_tip_after <= minority_tip_before {
            reasons.push("minority-stalled");
        }
        if majority_tip_after <= majority_tip_before {
            reasons.push("majority-stalled");
        }
        let verdict = verdict_from(&reasons);
        log_message(&format!("  deep-fork-refused: {verdict}"));
        self.fork_verdict = Some(verdict);
        Ok(())
    }

    /// Control for deep-fork-refused: the same split, but the minority node
    /// reconnects while the tips are still below the prune target. The fork
    /// point is then above it, so the node must follow the majority chain
    /// and log no refusal; the nightly convergence check then sees one
    /// chain. The reconnect height also keeps the fork within the PPLNS
    /// window cache, which bounds how deep a fork a node can pay out for.
    async fn shallow_fork_reorged(&mut self, index: usize) -> ToolResult<()> {
        let interval = first_capture(&self.run_dir().node_log(0), &PRUNE_INTERVAL);
        let Some(interval) = interval else {
            log_message("  node 0 has not logged its sim depths; is it a sim build?");
            self.fork_verdict = Some("INCONCLUSIVE".to_string());
            return Ok(());
        };
        let reconnect_height = (3 * interval).max(0) as u64;
        log_message(&format!(
            "--- shallow-fork-reorged: node {index} reconnects to the majority at height {reconnect_height}, above the prune target ---"
        ));
        self.target_list.push_str(&spaced(&[index]));
        if !self
            .wait_for_swarm_height(reconnect_height, self.settings.fork_tip_timeout)
            .await
        {
            self.fork_verdict = Some("INCONCLUSIVE".to_string());
            return Ok(());
        }
        if self.count_pruned_nodes() != 0 {
            let verdict = "INCONCLUSIVE (a node pruned before the reconnect)".to_string();
            log_message(&format!("  shallow-fork-reorged: {verdict}"));
            self.fork_verdict = Some(verdict);
            return Ok(());
        }

        self.reconnect_to_majority(index).await?;
        log_message(&format!(
            "  observing for {}s",
            self.settings.fork_observe_seconds
        ));
        tokio::time::sleep(Duration::from_secs(self.settings.fork_observe_seconds)).await;

        let links = self.count_links_between(&[index], &[0]).await;
        let refusals = self.count_since_reconnect(index, "branch forks below the prune target");
        let check_height = self.log_tip(0) as i64 - interval;
        let minority_hash = self.api_confirmed_hash(index, check_height).await;
        let majority_hash = self.api_confirmed_hash(0, check_height).await;
        let shown = |hash: &Option<String>| hash.clone().unwrap_or_else(|| "none".to_string());
        self.observation_lines.push(format!(
            "after reconnect: links to node 0: {links}, refusals: {refusals}"
        ));
        self.observation_lines.push(format!(
            "confirmed hash at {check_height}: minority {}, majority {}",
            shown(&minority_hash),
            shown(&majority_hash)
        ));

        if links < 1 {
            let verdict = format!("INCONCLUSIVE (node {index} never connected to node 0)");
            log_message(&format!("  shallow-fork-reorged: {verdict}"));
            self.fork_verdict = Some(verdict);
            return Ok(());
        }
        let mut reasons: Vec<&str> = Vec::with_capacity(2);
        if refusals != 0 {
            reasons.push("refused");
        }
        if minority_hash.is_none() || minority_hash != majority_hash {
            reasons.push("did-not-reorg");
        }
        let verdict = verdict_from(&reasons);
        log_message(&format!("  shallow-fork-reorged: {verdict}"));
        self.fork_verdict = Some(verdict);
        Ok(())
    }

    fn summary(self) -> FailureSummary {
        build_summary(
            &self.settings.scenario,
            SummaryParts {
                target_list: self.target_list,
                observation_lines: self.observation_lines,
                summary_lines: self.summary_lines,
                rejoined_ok: self.rejoined_ok,
                rejoined_total: self.rejoined_total,
                maturity_verdict: self.maturity_verdict,
                join_verdict: self.join_verdict,
                fork_verdict: self.fork_verdict,
            },
        )
    }
}

/// `PASS`, or `FAIL (reason reason ...)`.
fn verdict_from(reasons: &[&str]) -> String {
    if reasons.is_empty() {
        "PASS".to_string()
    } else {
        format!("FAIL ({})", reasons.join(" "))
    }
}

/// Lines in a file, as `wc -l` counts them (newline characters), read in
/// chunks so a large log is never held in memory. 0 for a missing file.
fn line_count(path: &Path) -> usize {
    use std::io::Read;
    let Ok(mut file) = std::fs::File::open(path) else {
        return 0;
    };
    let mut chunk = vec![0u8; 256 * 1024];
    let mut count = 0;
    loop {
        match file.read(&mut chunk) {
            Ok(0) | Err(_) => return count,
            Ok(read) => count += memchr::memchr_iter(b'\n', &chunk[..read]).count(),
        }
    }
}

/// What the summary is built from.
#[derive(Debug, Default)]
pub struct SummaryParts {
    pub target_list: String,
    pub observation_lines: Vec<String>,
    pub summary_lines: Vec<String>,
    pub rejoined_ok: usize,
    pub rejoined_total: usize,
    pub maturity_verdict: Option<String>,
    pub join_verdict: Option<String>,
    pub fork_verdict: Option<String>,
}

/// The summary text and its parsed values.
pub fn build_summary(scenario: &str, parts: SummaryParts) -> FailureSummary {
    let unrecovered = parts.rejoined_total - parts.rejoined_ok;
    let targets = if parts.target_list.is_empty() {
        " none".to_string()
    } else {
        parts.target_list
    };
    let rejoined = format!("{}/{}", parts.rejoined_ok, parts.rejoined_total);
    let mut lines: Vec<String> = Vec::with_capacity(
        8 + parts.observation_lines.len() + parts.summary_lines.len(),
    );
    lines.push(format!("=== failure injection summary (scenario: {scenario}) ==="));
    lines.push(format!("targets:{targets}"));
    lines.extend(parts.observation_lines);
    lines.extend(parts.summary_lines);
    lines.push(format!("rejoined: {rejoined}"));
    lines.push(format!("unrecovered nodes: {unrecovered}"));
    if let Some(verdict) = &parts.maturity_verdict {
        lines.push(format!("maturity verdict: {verdict}"));
    }
    if let Some(verdict) = &parts.join_verdict {
        lines.push(format!("pruned-join verdict: {verdict}"));
    }
    if let Some(verdict) = &parts.fork_verdict {
        lines.push(format!("fork verdict: {verdict}"));
    }
    let mut text = lines.join("\n");
    text.push('\n');
    FailureSummary {
        text,
        targets: targets.trim_start().to_string(),
        rejoined,
        unrecovered,
        maturity_verdict: parts.maturity_verdict,
        join_verdict: parts.join_verdict,
        fork_verdict: parts.fork_verdict,
    }
}

/// Print the summary and write it to `RUN_DIR/failure-results.txt`.
fn publish(run_dir: &RunDir, summary: &FailureSummary) -> ToolResult<()> {
    print!("{}", summary.text);
    let path = run_dir.path().join("failure-results.txt");
    std::fs::write(&path, &summary.text).map_err(|error| ToolError::io("write", &path, error))
}

/// Refuse a late-miner run that cannot finish: the swarm must have a late
/// node with its emitter off, and that node must have logged its maturity
/// depth.
fn require_late_miner_setup(settings: &FailureSettings) -> Result<usize, Exit> {
    let Some(late) = settings.late_miner else {
        return Err(Exit::usage(
            "ERROR: late-miner needs LATE_MINER set to the node run-swarm launched with its emitter off.",
        ));
    };
    let config = std::fs::read_to_string(settings.run_dir.node_config(late)).unwrap_or_default();
    if !config.lines().any(|line| line == "enabled = false") {
        return Err(Exit::usage(format!(
            "ERROR: node {late} is not a late miner; launch the swarm with LATE_MINER={late}."
        )));
    }
    if first_capture(&settings.run_dir.node_log(late), &LATE_MATURITY).is_none() {
        return Err(Exit::usage(format!(
            "ERROR: node {late} has not logged its sim depths; is it a sim build?"
        )));
    }
    Ok(late)
}

/// The majority group must outwork the minority, so a reorg onto it is
/// wanted by chain work; only the reorg depth limit may refuse it. The swarm
/// must have been launched split, with node 0 in the majority group.
fn require_fork_setup(settings: &FailureSettings, node_total: usize) -> Result<usize, Exit> {
    let first = settings
        .fork_group_from
        .filter(|first| *first >= 2 && *first < node_total);
    let Some(first) = first else {
        return Err(Exit::usage(format!(
            "ERROR: {} needs a swarm launched with FORK_GROUP_FROM between 2 and {}.",
            settings.scenario,
            node_total.saturating_sub(1)
        )));
    };
    let config = std::fs::read_to_string(settings.run_dir.node_config(first)).unwrap_or_default();
    if config.contains(&format!("tcp/{}\"", settings.base_p2p)) {
        return Err(Exit::usage(format!(
            "ERROR: node {first} already dials node 0; launch the swarm with FORK_GROUP_FROM={first}."
        )));
    }
    Ok(first)
}

/// Run the scenario in `settings` against the swarm in its run directory.
/// Returns the summary, already printed and written to
/// `failure-results.txt`. Nodes this process restarts are recorded in
/// `processes`.
pub async fn inject(settings: &FailureSettings, processes: &mut ProcessTable) -> Result<FailureSummary, Exit> {
    check_scenario(&settings.scenario).map_err(|error| Exit::usage(format!("ERROR: {error}")))?;
    let run_dir = &settings.run_dir;
    if !run_dir.path().is_dir() {
        return Err(Exit::failure(format!(
            "ERROR: no run dir at {}; start a swarm with run-swarm first.",
            run_dir.path().display()
        )));
    }
    let node_total = run_dir.node_count();
    if node_total == 0 {
        return Err(Exit::failure(format!(
            "ERROR: no node configs in {}; start a swarm with run-swarm first.",
            run_dir.path().display()
        )));
    }

    if settings.scenario == "clean" {
        log_message("scenario 'clean': no failures injected");
        let summary = build_summary(&settings.scenario, SummaryParts::default());
        publish(run_dir, &summary)?;
        return Ok(summary);
    }

    let binary = settings
        .sim_binary
        .clone()
        .unwrap_or_else(|| profile_binary(&repo::root(), &settings.profile, "p2poolv2_sim"));
    if !binary.is_file() {
        return Err(Exit::failure(format!(
            "ERROR: sim binary not found at {} (set PROFILE, or build with run-swarm).",
            binary.display()
        )));
    }
    let late = if settings.scenario == "late-miner" {
        Some(require_late_miner_setup(settings)?)
    } else {
        None
    };
    let fork_node = if settings.scenario == "deep-fork-refused"
        || settings.scenario == "shallow-fork-reorged"
    {
        Some(require_fork_setup(settings, node_total)?)
    } else {
        None
    };
    if node_total < 3 {
        return Err(Exit::failure(format!(
            "ERROR: failure injection needs at least 3 nodes (node 0 is never a target); have {node_total}."
        )));
    }

    let targets = select_targets(settings.seed, node_total, settings.dial_fanout);
    let mut injection = Injection {
        settings,
        processes,
        binary,
        node_total,
        targets,
        api: NodeApi::new(Duration::from_secs(5))?,
        summary_lines: Vec::with_capacity(8),
        observation_lines: Vec::with_capacity(16),
        target_list: String::with_capacity(16),
        rejoined_ok: 0,
        rejoined_total: 0,
        tip_at_kill: HashMap::with_capacity(8),
        catchup_seconds: 0,
        reconnect_log_line: 0,
        maturity_verdict: None,
        join_verdict: None,
        fork_verdict: None,
    };

    log_message(&format!(
        "scenario '{}' on {node_total} nodes (seed {})",
        settings.scenario, settings.seed
    ));
    log_message(&format!(
        "Warming up for {}s before injecting failures...",
        settings.warmup
    ));
    tokio::time::sleep(Duration::from_secs(settings.warmup)).await;

    match (settings.scenario.as_str(), late, fork_node) {
        ("restart-fast", _, _) => injection.restart_fast().await,
        ("restart-delayed", _, _) => injection.restart_delayed().await,
        ("partition", _, _) => injection.partition().await,
        ("late-miner", Some(late), _) => injection.late_miner(late).await?,
        ("pruned-join", _, _) => injection.pruned_join().await?,
        ("deep-fork-refused", _, Some(node)) => injection.deep_fork_refused(node).await?,
        ("shallow-fork-reorged", _, Some(node)) => injection.shallow_fork_reorged(node).await?,
        _ => {
            injection.restart_fast().await;
            injection.restart_delayed().await;
            injection.partition().await;
        }
    }

    let summary = injection.summary();
    publish(run_dir, &summary)?;
    Ok(summary)
}

#[cfg(test)]
mod tests {
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
}
