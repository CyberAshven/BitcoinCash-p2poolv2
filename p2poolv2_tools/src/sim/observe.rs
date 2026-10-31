// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A live snapshot of a running sim swarm: each node's tip height and hash
//! (from its log, which a busy node cannot time out), peer count (from the
//! API, which is live state) and whether it is alive, then convergence and a
//! scan of the logs for errors. Replaces `load-tests/sim/observe.sh`.
//!
//! This is a best-effort reporter: partial data is fine. For the
//! authoritative post-run summary use `metrics`.

use crate::error::{ToolError, ToolResult};
use crate::log_file::for_each_line_bytes;
use crate::node_api::NodeApi;
use crate::process::{Signal, send_signal};
use crate::sim::RunDir;
use crate::sim::node_log::{HeightRange, confirmed_heights, promoted_hashes};
use regex::bytes::Regex;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

/// Distinct error lines kept for the sample; more are counted but not
/// sampled, which bounds memory on a log full of errors.
const MAX_DISTINCT_ERROR_LINES: usize = 100_000;

static ERROR_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)error|panic|failed to build emission").expect("valid regex"));
static NOT_AN_ERROR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)max_workbase|0 warnings").expect("valid regex"));
static PROMOTED_ANY_CASE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)Promoted block").expect("valid regex"));
static SOME_HEIGHT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"Some\(([0-9]+)\)").expect("valid regex"));
static LAST_PROMOTED_HASH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r".*Promoted block ([0-9a-f]{16,})").expect("valid regex"));

/// What one node log shows.
#[derive(Debug, Default)]
struct LogScan {
    /// Height and hash prefix from the last confirmed promotion line.
    tip: Option<(Option<u64>, Option<String>)>,
    promotion_lines: u64,
    error_lines: u64,
    errors: HashMap<Vec<u8>, u64>,
    produced: HashSet<Vec<u8>>,
    height_range: Option<HeightRange>,
}

fn scan_log(path: &Path) -> ToolResult<LogScan> {
    let mut scan = LogScan::default();
    for_each_line_bytes(path, |line| {
        if memchr::memmem::find(line, b"to confirmed height Some").is_some() {
            let height = SOME_HEIGHT
                .captures(line)
                .and_then(|found| std::str::from_utf8(&found[1]).ok()?.parse().ok());
            let hash = LAST_PROMOTED_HASH.captures(line).map(|found| {
                let hash = String::from_utf8_lossy(&found[1]);
                hash.chars().take(12).collect()
            });
            scan.tip = Some((height, hash));
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
        if PROMOTED_ANY_CASE.is_match(line) {
            scan.promotion_lines += 1;
        }
        if ERROR_LINE.is_match(line) && !NOT_AN_ERROR.is_match(line) {
            scan.error_lines += 1;
            let room = scan.errors.len() < MAX_DISTINCT_ERROR_LINES;
            match scan.errors.get_mut(line) {
                Some(count) => *count += 1,
                None if room => {
                    scan.errors.insert(line.to_vec(), 1);
                }
                None => {}
            }
        }
    })?;
    Ok(scan)
}

/// Lines of `uniq -c` output for `counts`, sorted as `sort -rn` sorts them
/// (count descending, then the line descending), at most `limit`.
pub fn top_counts(counts: &HashMap<String, u64>, limit: usize) -> Vec<String> {
    let mut sorted: Vec<(&String, &u64)> = counts.iter().collect();
    sorted.sort_by(|first, second| second.1.cmp(first.1).then_with(|| second.0.cmp(first.0)));
    sorted
        .into_iter()
        .take(limit)
        .map(|(line, count)| format!("{count:>7} {line}"))
        .collect()
}

/// `jq length` of a JSON value; `None` where jq fails.
fn json_length(value: &Value) -> Option<String> {
    match value {
        Value::Null => Some("0".to_string()),
        Value::Array(items) => Some(items.len().to_string()),
        Value::Object(fields) => Some(fields.len().to_string()),
        Value::String(text) => Some(text.chars().count().to_string()),
        Value::Number(number) => number.as_f64().map(|float| {
            let length = float.abs();
            if length.fract() == 0.0 {
                format!("{length:.0}")
            } else {
                length.to_string()
            }
        }),
        Value::Bool(_) => None,
    }
}

/// Build the snapshot of the swarm in `run_dir`, whose node APIs start at
/// `base_api`.
pub async fn snapshot(run_dir: &RunDir, base_api: u32) -> ToolResult<String> {
    if !run_dir.path().is_dir() {
        return Err(ToolError::message(format!(
            "No run dir at {}.",
            run_dir.path().display()
        )));
    }
    let node_count = run_dir.node_count();
    if node_count == 0 {
        return Err(ToolError::message(format!(
            "No node configs in {}.",
            run_dir.path().display()
        )));
    }

    let logs = run_dir.node_logs();
    let mut scans: HashMap<PathBuf, LogScan> = HashMap::with_capacity(logs.len());
    for log in logs {
        let scan = scan_log(&log)?;
        scans.insert(log, scan);
    }

    let api = NodeApi::new(Duration::from_secs(2))?;
    let pids = run_dir.read_pids();
    let mut text = String::with_capacity(4096);
    let rule = "-".repeat(61);
    let _ = writeln!(
        text,
        "{:<5} {:<7} {:<12} {:<8} {:<6}",
        "node", "tip_h", "tip_hash", "peers", "alive"
    );
    let _ = writeln!(text, "{rule}");

    let mut tips: Vec<String> = Vec::with_capacity(node_count);
    let mut highest: i64 = -1;
    let mut alive_total = 0usize;
    for index in 0..node_count {
        let tip = scans
            .get(&run_dir.node_log(index))
            .and_then(|scan| scan.tip.clone());
        let tip_height = tip.as_ref().and_then(|(height, _)| *height);
        let tip_hash = tip
            .and_then(|(_, hash)| hash)
            .unwrap_or_else(|| "-".to_string());
        let height_text = tip_height.map_or_else(|| "-1".to_string(), |height| height.to_string());

        let port = base_api + index as u32;
        let peers = api
            .get_any_status(port, "/peers")
            .await
            .and_then(|peers| json_length(&peers))
            .unwrap_or_else(|| "-".to_string());

        let alive = pids
            .get(index)
            .copied()
            .flatten()
            .is_some_and(|pid| send_signal(&[pid], Signal::Probe));
        alive_total += usize::from(alive);
        let _ = writeln!(
            text,
            "{index:<5} {height_text:<7} {tip_hash:<12} {peers:<8} {:<6}",
            if alive { "yes" } else { "NO" }
        );

        if let Some(height) = tip_height {
            tips.push(format!("{tip_hash}@{height}"));
            highest = highest.max(height as i64);
        }
    }
    let _ = writeln!(text, "{rule}");
    let _ = writeln!(
        text,
        "alive: {alive_total}/{node_count}   max tip height: {highest}"
    );

    if !tips.is_empty() {
        let mut tip_counts: HashMap<String, u64> = HashMap::with_capacity(tips.len());
        for tip in tips {
            *tip_counts.entry(tip).or_insert(0) += 1;
        }
        let _ = writeln!(
            text,
            "distinct tips: {} (1 = fully converged)",
            tip_counts.len()
        );
        let _ = writeln!(text, "tip distribution:");
        for line in top_counts(&tip_counts, 8) {
            let _ = writeln!(text, "  {line}");
        }
    }

    let mut error_lines = 0u64;
    let mut promotion_lines = 0u64;
    let mut errors: HashMap<String, u64> = HashMap::with_capacity(64);
    for scan in scans.values() {
        error_lines += scan.error_lines;
        promotion_lines += scan.promotion_lines;
        for (line, count) in &scan.errors {
            let line = String::from_utf8_lossy(line).into_owned();
            if errors.len() < MAX_DISTINCT_ERROR_LINES || errors.contains_key(&line) {
                *errors.entry(line).or_insert(0) += count;
            }
        }
    }
    let _ = writeln!(text);
    let _ = writeln!(
        text,
        "log scan (all nodes):  promotions={promotion_lines}  error-lines={error_lines}"
    );

    // Uncle rate on node 0: distinct produced blocks against the chain's net
    // growth, not log-line counts, which re-promotions inflate under reorgs.
    if let Some(node_zero) = scans.get(&run_dir.node_log(0)) {
        let produced = node_zero.produced.len() as u64;
        let (main_chain, uncles, rate) = match node_zero.height_range {
            Some(range) if produced > 0 => {
                let main_chain = range.highest - range.lowest + 1;
                let uncles = produced.saturating_sub(main_chain);
                let rate = format!("{:.1}%", 100.0 * uncles as f64 / produced as f64);
                (main_chain, uncles, rate)
            }
            _ => (0, 0, "n/a".to_string()),
        };
        let _ = writeln!(
            text,
            "node 0:  produced={produced}  main-chain={main_chain}  uncles={uncles}  uncle-rate={rate}"
        );
    }
    if error_lines > 0 {
        let _ = writeln!(text, "sample errors:");
        for line in top_counts(&errors, 5) {
            let _ = writeln!(text, "  {line}");
        }
    }
    Ok(text)
}

#[cfg(test)]
mod tests;
