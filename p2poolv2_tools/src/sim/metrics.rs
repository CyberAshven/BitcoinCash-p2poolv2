// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The authoritative, log-based summary of a sim swarm run. Replaces
//! `load-tests/sim/metrics.sh`.
//!
//! It reads only the per-node logs and configs in `RUN_DIR` and never
//! queries the running nodes, so it is load-immune and safe to run during or
//! after a run. The nightly verdict reads its numbers.

use crate::error::{ToolError, ToolResult};
use crate::log_file::for_each_line_bytes;
use crate::log_time::find_micros;
use crate::sim::RunDir;
use crate::sim::node_log::{HeightRange, confirmed_heights, first_hash_at_height, promoted_hashes};
use regex::bytes::Regex;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

static POOL_DIFFICULTY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"pool_difficulty=([0-9.]+)").expect("valid regex"));
static EMITTED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"emitted=([0-9]+)").expect("valid regex"));
static ASERT_MISMATCH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)AsertMismatch").expect("valid regex"));
static MERKLE_MISMATCH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)don't match merkle|does not match calculated").expect("valid regex")
});
static PANICKED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)panicked").expect("valid regex"));

/// The numbers the nightly verdict reads.
#[derive(Debug, Clone, PartialEq)]
pub struct MetricsSummary {
    /// Node 0's confirmed promotions, or 0 when there were fewer than two
    /// (the report then says "not enough promotions yet").
    pub promotions: u64,
    pub panicked_nodes: u64,
    pub asert_mismatches: u64,
    pub merkle_payout_mismatches: u64,
    /// Node 0's uncle rate as printed (`%.1f`), or `None` before any share
    /// was confirmed.
    pub uncle_rate: Option<String>,
}

/// The printed report and its numbers.
#[derive(Debug, Clone)]
pub struct MetricsReport {
    pub text: String,
    pub summary: MetricsSummary,
}

/// What one node log shows. The node 0 fields are filled for every log but
/// only read for node 0.
#[derive(Debug, Default)]
struct LogScan {
    height_range: Option<HeightRange>,
    /// `emitted=` on the last `sim stats` line; `None` when that line has
    /// none or there is no such line.
    last_emitted: Option<u64>,
    promotion_lines: u64,
    first_promotion_second: Option<i64>,
    last_promotion_second: Option<i64>,
    difficulty: DifficultyTrack,
    produced: HashSet<Vec<u8>>,
    block_finds: u64,
    asert_mismatches: u64,
    merkle_mismatches: u64,
    panicked: bool,
}

/// First, last, lowest and highest `pool_difficulty` sample.
#[derive(Debug, Default, Clone, Copy)]
struct DifficultyTrack {
    first: f64,
    last: f64,
    lowest: f64,
    highest: f64,
    samples: u64,
}

impl DifficultyTrack {
    fn add(&mut self, value: f64) {
        if self.samples == 0 {
            self.first = value;
            self.lowest = value;
            // awk starts the maximum at 0 (an unset variable).
            self.highest = value.max(0.0);
        } else {
            self.lowest = self.lowest.min(value);
            self.highest = self.highest.max(value);
        }
        self.last = value;
        self.samples += 1;
    }
}

fn parse_number<T: std::str::FromStr>(bytes: &[u8]) -> Option<T> {
    std::str::from_utf8(bytes).ok()?.parse().ok()
}

fn scan_log(path: &Path) -> ToolResult<LogScan> {
    let mut scan = LogScan::default();
    for_each_line_bytes(path, |line| {
        if memchr::memmem::find(line, b"to confirmed height Some").is_some() {
            scan.promotion_lines += 1;
            let second = std::str::from_utf8(line)
                .ok()
                .and_then(find_micros)
                .map(|micros| micros.div_euclid(1_000_000));
            if second.is_some() {
                scan.first_promotion_second = scan.first_promotion_second.or(second);
                scan.last_promotion_second = second;
            }
        }
        for height in confirmed_heights(line) {
            scan.height_range = Some(match scan.height_range {
                None => HeightRange {
                    lowest: height,
                    highest: height,
                },
                Some(range) => HeightRange {
                    lowest: range.lowest.min(height),
                    highest: range.highest.max(height),
                },
            });
        }
        for hash in promoted_hashes(line) {
            if !scan.produced.contains(hash) {
                scan.produced.insert(hash.to_vec());
            }
        }
        if memchr::memmem::find(line, b"sim stats").is_some() {
            scan.last_emitted = EMITTED
                .captures(line)
                .and_then(|found| parse_number(&found[1]));
            for found in POOL_DIFFICULTY.captures_iter(line) {
                if let Some(value) = parse_number(&found[1]) {
                    scan.difficulty.add(value);
                }
            }
        }
        if memchr::memmem::find(line, b"sim block-find").is_some() {
            scan.block_finds += 1;
        }
        if ASERT_MISMATCH.is_match(line) {
            scan.asert_mismatches += 1;
        }
        if MERKLE_MISMATCH.is_match(line) {
            scan.merkle_mismatches += 1;
        }
        if !scan.panicked && PANICKED.is_match(line) {
            scan.panicked = true;
        }
    })?;
    Ok(scan)
}

/// The last whitespace-separated field of the first line of `config` that
/// starts with `key`, as `grep -E "^key" | awk '{print $NF}'` reads it.
pub fn config_value(config: &Path, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(config).ok()?;
    text.lines()
        .filter(|line| line.starts_with(key))
        .find_map(|line| line.split_whitespace().last().map(str::to_string))
}

fn format_rate(promotions: u64, span: i64) -> f64 {
    if span > 0 {
        promotions as f64 / span as f64
    } else {
        0.0
    }
}

fn percent(part: f64, whole: f64) -> f64 {
    if whole > 0.0 { 100.0 * part / whole } else { 0.0 }
}

/// Build the report for the run in `run_dir`.
pub fn collect(run_dir: &RunDir) -> ToolResult<MetricsReport> {
    let node_count = run_dir.node_count();
    if node_count == 0 {
        return Err(ToolError::message(format!(
            "No run in {}",
            run_dir.path().display()
        )));
    }

    // Every node-*.log is scanned once. The health counts cover all of them;
    // the per-node numbers come from node-<i>.log for each configured node.
    let logs = run_dir.node_logs();
    let mut totals = LogScan::default();
    let mut panicked_nodes = 0u64;
    let mut scans: HashMap<PathBuf, LogScan> = HashMap::with_capacity(logs.len());
    for log in logs {
        let scan = scan_log(&log)?;
        totals.block_finds += scan.block_finds;
        totals.asert_mismatches += scan.asert_mismatches;
        totals.merkle_mismatches += scan.merkle_mismatches;
        panicked_nodes += u64::from(scan.panicked);
        scans.insert(log, scan);
    }
    let nodes: Vec<LogScan> = (0..node_count)
        .map(|index| scans.remove(&run_dir.node_log(index)).unwrap_or_default())
        .collect();

    let mut text = String::with_capacity(4096);
    let config_zero = run_dir.node_config(0);
    let block_time = config_value(&config_zero, "ideal_block_time_secs")
        .filter(|value| value.parse::<f64>() != Ok(0.0))
        .unwrap_or_else(|| "10".to_string());
    let block_seconds: f64 = block_time.parse().unwrap_or(10.0);
    let target_rate = format!("{:.3}", 1.0 / block_seconds);
    let _ = writeln!(
        text,
        "=== sim swarm summary ({node_count} nodes, ideal_block_time={block_time}s -> target {target_rate}/s) ==="
    );

    write_convergence(&mut text, run_dir, &nodes)?;

    let node_zero = &nodes[0];
    let _ = writeln!(text, "--- 2. main-chain share rate (node 0) ---");
    let promotions = if node_zero.promotion_lines > 1 {
        let span = match (node_zero.first_promotion_second, node_zero.last_promotion_second) {
            (Some(first), Some(last)) => last - first,
            _ => 0,
        };
        let _ = writeln!(
            text,
            "promotions={}  span={span}s  rate={:.3}/s  (target {target_rate}/s)",
            node_zero.promotion_lines,
            format_rate(node_zero.promotion_lines, span)
        );
        node_zero.promotion_lines
    } else {
        let _ = writeln!(text, "not enough promotions yet");
        0
    };

    let _ = writeln!(text, "--- 3. ASERT pool-difficulty (node 0) ---");
    let difficulty = node_zero.difficulty;
    if difficulty.samples > 0 {
        let _ = writeln!(
            text,
            "first={:.0}  last={:.0}  min={:.0}  max={:.0}  samples={}",
            difficulty.first,
            difficulty.last,
            difficulty.lowest,
            difficulty.highest,
            difficulty.samples
        );
    } else {
        let _ = writeln!(text, "no sim-stats samples yet");
    }

    write_emission(&mut text, run_dir, &nodes);

    let _ = writeln!(text, "--- 5. uncles / block-finds / health ---");
    let uncle_rate = match node_zero.height_range {
        Some(range) if !node_zero.produced.is_empty() => {
            let produced = node_zero.produced.len() as u64;
            let main_chain = range.highest - range.lowest + 1;
            let uncles = produced.saturating_sub(main_chain);
            let rate = format!("{:.1}", percent(uncles as f64, produced as f64));
            let _ = writeln!(
                text,
                "uncle rate (node 0): {rate}%   ({uncles} uncles / {produced} produced; main-chain length {main_chain})"
            );
            Some(rate)
        }
        _ => {
            let _ = writeln!(text, "uncle rate (node 0): n/a (no confirmed blocks yet)");
            None
        }
    };
    let _ = writeln!(text, "block-finds (all nodes): {}", totals.block_finds);
    let _ = writeln!(
        text,
        "rejections: AsertMismatch={}  merkle/payout={}   panicked nodes: {panicked_nodes}",
        totals.asert_mismatches, totals.merkle_mismatches
    );

    Ok(MetricsReport {
        text,
        summary: MetricsSummary {
            promotions,
            panicked_nodes,
            asert_mismatches: totals.asert_mismatches,
            merkle_payout_mismatches: totals.merkle_mismatches,
            uncle_rate,
        },
    })
}

/// Section 1: confirmed height and tip-hash agreement across nodes.
fn write_convergence(text: &mut String, run_dir: &RunDir, nodes: &[LogScan]) -> ToolResult<()> {
    let heights: Vec<u64> = nodes
        .iter()
        .map(|scan| scan.height_range.map_or(0, |range| range.highest))
        .collect();
    let highest = heights.iter().copied().max().unwrap_or(0);
    let lowest = heights.iter().copied().min().unwrap_or(0);
    let within_two = heights
        .iter()
        .filter(|height| **height + 2 >= highest)
        .count();
    // Compare tips at a height every node has reached, so nodes straddling
    // the frontier do not look diverged.
    let check_height = if lowest > 2 { lowest - 2 } else { lowest };
    let mut hashes: HashSet<String> = HashSet::with_capacity(nodes.len());
    for index in 0..nodes.len() {
        let log = run_dir.node_log(index);
        if log.exists()
            && let Some(hash) = first_hash_at_height(&log, check_height)?
        {
            hashes.insert(hash);
        }
    }
    let _ = writeln!(text, "--- 1. convergence ---");
    let _ = writeln!(
        text,
        "max confirmed height: {highest}   nodes within 2 of tip: {within_two}/{}   (lowest node: {lowest})",
        nodes.len()
    );
    let _ = writeln!(
        text,
        "distinct block hash at height {check_height} across nodes: {}  (1 = single converged chain)",
        hashes.len()
    );
    Ok(())
}

/// Section 4: each node's share of hashrate against its share of emitted
/// shares.
fn write_emission(text: &mut String, run_dir: &RunDir, nodes: &[LogScan]) {
    let _ = writeln!(text, "--- 4. emission proportional to hashrate (per node) ---");
    let _ = writeln!(
        text,
        "{:<5} {:<16} {:<10} {:<9} {:<10}",
        "node", "hashrate", "hr_share%", "emitted", "em_share%"
    );
    let rows: Vec<(f64, u64)> = nodes
        .iter()
        .enumerate()
        .map(|(index, scan)| {
            let hashrate = config_value(&run_dir.node_config(index), "hashrate")
                .and_then(|value| value.parse().ok())
                .unwrap_or(0.0);
            (hashrate, scan.last_emitted.unwrap_or(0))
        })
        .collect();
    let hashrate_total: f64 = rows.iter().map(|(hashrate, _)| hashrate).sum();
    let emitted_total: u64 = rows.iter().map(|(_, emitted)| emitted).sum();
    for (index, (hashrate, emitted)) in rows.iter().enumerate() {
        let _ = writeln!(
            text,
            "{index:<5} {hashrate:<16.0} {:<10.1} {emitted:<9} {:<10.1}",
            percent(*hashrate, hashrate_total),
            percent(*emitted as f64, emitted_total as f64)
        );
    }
    let _ = writeln!(
        text,
        "(hr_share% and em_share% track each other once enough shares accumulate)"
    );
}

#[cfg(test)]
mod tests {
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
}
