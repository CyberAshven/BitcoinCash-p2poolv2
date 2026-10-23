// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Share propagation and confirmation latencies across nodes, from their
//! logs.
//!
//! Propagation latency is the time from a share being mined at one node
//! until another node validates it. Confirmation latency is the time from a
//! share first appearing at a node (mined or received) until that node
//! promotes it to the confirmed chain. By default only confirmed chain
//! shares count; uncles can be included.
//!
//! Replaces `debug_tools/share_latency.py`.

use crate::ansi::{BLUE, BOLD, CYAN, DIM, GREEN, MAGENTA, RED, RESET, YELLOW, strip};
use crate::error::ToolResult;
use crate::log_file::LogLines;
use crate::log_time::parse_micros;
use crate::stats::summarize;
use regex::Regex;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

static TIMESTAMP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d+)Z").expect("valid regex")
});
static MINED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"Adding share to chain: ([0-9a-f]{64})").expect("valid regex"));
static STORE_PEER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"Adding share and organising header atomically: ([0-9a-f]{64})")
        .expect("valid regex")
});
static VALID_OK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"Share block ([0-9a-f]{64}) validated successfully").expect("valid regex")
});
static CONFIRMED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"Promoted block ([0-9a-f]{64}) to confirmed height (Some\(\d+\)|None)")
        .expect("valid regex")
});

/// First time each share hash was seen, in log order.
#[derive(Debug, Default, Clone)]
pub struct FirstSeen {
    order: Vec<String>,
    times: HashMap<String, i64>,
}

impl FirstSeen {
    /// Record `hash` at `micros` unless it was seen before.
    fn record(&mut self, hash: &str, micros: i64) {
        if !self.times.contains_key(hash) {
            self.times.insert(hash.to_string(), micros);
            self.order.push(hash.to_string());
        }
    }

    pub fn get(&self, hash: &str) -> Option<i64> {
        self.times.get(hash).copied()
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// Hashes and times in the order they were first seen.
    pub fn iter(&self) -> impl Iterator<Item = (&str, i64)> {
        self.order
            .iter()
            .map(|hash| (hash.as_str(), self.times[hash.as_str()]))
    }
}

/// What one node's log says about each share.
#[derive(Debug, Default, Clone)]
pub struct NodeEvents {
    pub mined: FirstSeen,
    pub received: FirstSeen,
    pub validated: FirstSeen,
    pub confirmed: FirstSeen,
    pub uncles: FirstSeen,
}

impl NodeEvents {
    /// Record whatever `line` says. Colour codes are ignored.
    pub fn add_line(&mut self, line: &str) {
        // Cheap test first: most lines are about nothing this tool reads.
        let relevant = ["Adding share", "validated successfully", "Promoted block"]
            .iter()
            .any(|marker| line.contains(marker));
        if !relevant {
            return;
        }
        let clean = strip(line);
        let Some(timestamp) = TIMESTAMP
            .captures(&clean)
            .and_then(|found| parse_micros(&found[1]))
        else {
            return;
        };
        if let Some(found) = MINED.captures(&clean) {
            self.mined.record(&found[1], timestamp);
        }
        if let Some(found) = STORE_PEER.captures(&clean) {
            self.received.record(&found[1], timestamp);
        }
        if let Some(found) = VALID_OK.captures(&clean) {
            self.validated.record(&found[1], timestamp);
        }
        if let Some(found) = CONFIRMED.captures(&clean) {
            if &found[2] == "None" {
                self.uncles.record(&found[1], timestamp);
            } else {
                self.confirmed.record(&found[1], timestamp);
            }
        }
    }

    /// Events of the log at `path`.
    pub fn from_log(path: &Path) -> ToolResult<Self> {
        let mut events = NodeEvents::default();
        let mut lines = LogLines::open(path)?;
        for line in lines.by_ref() {
            events.add_line(&line);
        }
        lines.finish()?;
        Ok(events)
    }

    fn promoted_hashes(&self, include_uncles: bool) -> impl Iterator<Item = &str> {
        let uncles = include_uncles.then(|| self.uncles.iter().map(|(hash, _)| hash));
        self.confirmed
            .iter()
            .map(|(hash, _)| hash)
            .chain(uncles.into_iter().flatten())
    }
}

/// Every node's events, keyed by log file name, in the order the logs were
/// given. A later log with the same file name replaces an earlier one's
/// events but keeps its position.
#[derive(Debug, Default)]
pub struct Nodes {
    entries: Vec<(String, NodeEvents)>,
}

impl Nodes {
    pub fn insert(&mut self, label: String, events: NodeEvents) {
        match self
            .entries
            .iter_mut()
            .find(|(existing, _)| *existing == label)
        {
            Some(entry) => entry.1 = events,
            None => self.entries.push((label, events)),
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &NodeEvents)> {
        self.entries
            .iter()
            .map(|(label, events)| (label.as_str(), events))
    }
}

/// One propagation sample.
#[derive(Debug, Clone, PartialEq)]
pub struct Propagation {
    pub hash: String,
    pub miner: String,
    pub receiver: String,
    pub seconds: f64,
}

/// One confirmation sample.
#[derive(Debug, Clone, PartialEq)]
pub struct Confirmation {
    pub hash: String,
    pub node: String,
    pub seconds: f64,
}

fn seconds_between(earlier: i64, later: i64) -> f64 {
    (later - earlier) as f64 / 1_000_000.0
}

/// For each share mined at one node, the time until each other node
/// validated it. Only shares some node confirmed (or took as an uncle, with
/// `include_uncles`) count.
pub fn propagation_latencies(nodes: &Nodes, include_uncles: bool) -> Vec<Propagation> {
    let included: HashSet<&str> = nodes
        .iter()
        .flat_map(|(_, events)| events.promoted_hashes(include_uncles))
        .collect();
    let mut results = Vec::with_capacity(included.len() * 2);
    for (miner, miner_events) in nodes.iter() {
        let mined = miner_events
            .mined
            .iter()
            .filter(|(hash, _)| included.contains(hash));
        for (hash, mined_at) in mined {
            let receivers = nodes.iter().filter(|(receiver, _)| *receiver != miner);
            for (receiver, receiver_events) in receivers {
                if let Some(validated_at) = receiver_events.validated.get(hash) {
                    results.push(Propagation {
                        hash: hash.to_string(),
                        miner: miner.to_string(),
                        receiver: receiver.to_string(),
                        seconds: seconds_between(mined_at, validated_at),
                    });
                }
            }
        }
    }
    results
}

/// For each share a node promoted, the time from when that node first saw it
/// (mined or received) to the promotion.
pub fn confirmation_latencies(nodes: &Nodes, include_uncles: bool) -> Vec<Confirmation> {
    let mut results = Vec::with_capacity(1024);
    for (node, events) in nodes.iter() {
        let mut promoted: Vec<(&str, i64)> = events.confirmed.iter().collect();
        if include_uncles {
            promoted.extend(
                events
                    .uncles
                    .iter()
                    .filter(|(hash, _)| events.confirmed.get(hash).is_none()),
            );
        }
        for (hash, promoted_at) in promoted {
            let first_seen = match (events.mined.get(hash), events.received.get(hash)) {
                (Some(mined), Some(received)) => Some(mined.min(received)),
                (Some(mined), None) => Some(mined),
                (None, Some(received)) => Some(received),
                (None, None) => None,
            };
            if let Some(first_seen) = first_seen {
                results.push(Confirmation {
                    hash: hash.to_string(),
                    node: node.to_string(),
                    seconds: seconds_between(first_seen, promoted_at),
                });
            }
        }
    }
    results
}

/// A duration as milliseconds under a second, seconds under a minute,
/// minutes and seconds above.
pub fn format_duration(seconds: f64) -> String {
    if seconds < 1.0 {
        return format!("{:.1}ms", seconds * 1000.0);
    }
    if seconds < 60.0 {
        return format!("{seconds:.3}s");
    }
    let minutes = (seconds / 60.0).floor();
    let remaining = seconds - minutes * 60.0;
    format!("{minutes:.0}m {remaining:.1}s")
}

/// First eight characters of a hash.
fn short(hash: &str) -> &str {
    if hash.is_empty() {
        return "???";
    }
    hash.get(..8).unwrap_or(hash)
}

fn scope(include_uncles: bool) -> &'static str {
    if include_uncles {
        "confirmed + uncles"
    } else {
        "confirmed chain only"
    }
}

/// Write the five statistics lines for `values`.
fn write_statistics(
    out: &mut impl Write,
    values: &[f64],
    highlight_mean: bool,
) -> std::io::Result<()> {
    let summary = summarize(values);
    let minimum = values.iter().copied().fold(f64::INFINITY, f64::min);
    let maximum = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    writeln!(out, "    samples: {}", summary.count)?;
    if highlight_mean {
        writeln!(
            out,
            "    mean:    {BOLD}{}{RESET}",
            format_duration(summary.mean)
        )?;
    } else {
        writeln!(out, "    mean:    {}", format_duration(summary.mean))?;
    }
    writeln!(out, "    stddev:  {}", format_duration(summary.stddev))?;
    writeln!(out, "    min:     {}", format_duration(minimum))?;
    writeln!(out, "    max:     {}", format_duration(maximum))
}

/// Up to three samples with the highest latency, highest first; samples
/// with equal latency keep their order.
fn top_outliers(entries: &[(&str, f64)]) -> Vec<(String, f64)> {
    let mut sorted: Vec<(String, f64)> = entries
        .iter()
        .map(|(hash, seconds)| (hash.to_string(), *seconds))
        .collect();
    sorted.sort_by(|first, second| second.1.total_cmp(&first.1));
    sorted.truncate(3);
    sorted
}

/// Write the per-pair and aggregate propagation report.
pub fn write_propagation_report(
    out: &mut impl Write,
    results: &[Propagation],
    include_uncles: bool,
) -> std::io::Result<()> {
    writeln!(
        out,
        "\n{BOLD}{GREEN}=== Share Propagation Latency ({}) ==={RESET}",
        scope(include_uncles)
    )?;
    writeln!(
        out,
        "{DIM}(time from mined at source to validated at receiver){RESET}\n"
    )?;
    if results.is_empty() {
        writeln!(
            out,
            "  {YELLOW}No cross-node propagation data found.{RESET}"
        )?;
        writeln!(
            out,
            "  {DIM}(Need 2+ log files with shares mined at one node"
        )?;
        return writeln!(out, "   and validated at another){RESET}");
    }

    let mut pairs: BTreeMap<(&str, &str), Vec<(&str, f64)>> = BTreeMap::new();
    for result in results {
        pairs
            .entry((result.miner.as_str(), result.receiver.as_str()))
            .or_default()
            .push((result.hash.as_str(), result.seconds));
    }
    let mut all_values = Vec::with_capacity(results.len());
    for ((miner, receiver), entries) in &pairs {
        let values: Vec<f64> = entries.iter().map(|(_, seconds)| *seconds).collect();
        all_values.extend_from_slice(&values);
        writeln!(out, "  {CYAN}{miner}{RESET} -> {MAGENTA}{receiver}{RESET}")?;
        write_statistics(out, &values, false)?;
        writeln!(out, "    top outliers:")?;
        for (hash, seconds) in top_outliers(entries) {
            let marker = if seconds < 0.0 {
                format!(" {RED}(negative - clock skew?){RESET}")
            } else {
                String::new()
            };
            writeln!(
                out,
                "      {DIM}{}{RESET}  {}{marker}",
                short(&hash),
                format_duration(seconds)
            )?;
        }
        writeln!(out)?;
    }
    writeln!(out, "  {BOLD}Aggregate (all pairs):{RESET}")?;
    write_statistics(out, &all_values, true)
}

/// Write the per-node and aggregate confirmation report.
pub fn write_confirmation_report(
    out: &mut impl Write,
    results: &[Confirmation],
    include_uncles: bool,
) -> std::io::Result<()> {
    writeln!(
        out,
        "\n{BOLD}{BLUE}=== Share Confirmation Latency ({}) ==={RESET}",
        scope(include_uncles)
    )?;
    writeln!(
        out,
        "{DIM}(time from first seen to confirmed at each node){RESET}\n"
    )?;
    if results.is_empty() {
        return writeln!(out, "  {YELLOW}No confirmation data found.{RESET}");
    }

    let mut per_node: BTreeMap<&str, Vec<(&str, f64)>> = BTreeMap::new();
    for result in results {
        per_node
            .entry(result.node.as_str())
            .or_default()
            .push((result.hash.as_str(), result.seconds));
    }
    let mut all_values = Vec::with_capacity(results.len());
    for (node, entries) in &per_node {
        let values: Vec<f64> = entries.iter().map(|(_, seconds)| *seconds).collect();
        all_values.extend_from_slice(&values);
        writeln!(out, "  {CYAN}{node}{RESET}")?;
        write_statistics(out, &values, false)?;
        writeln!(out, "    top outliers:")?;
        for (hash, seconds) in top_outliers(entries) {
            writeln!(
                out,
                "      {DIM}{}{RESET}  {}",
                short(&hash),
                format_duration(seconds)
            )?;
        }
        writeln!(out)?;
    }
    writeln!(out, "  {BOLD}Aggregate (all nodes):{RESET}")?;
    write_statistics(out, &all_values, true)
}

/// Write how many shares each node mined, received, validated, confirmed
/// and took as uncles.
pub fn write_overview(out: &mut impl Write, nodes: &Nodes) -> std::io::Result<()> {
    writeln!(out, "\n{BOLD}=== Node Overview ==={RESET}\n")?;
    let mut sorted: Vec<(&str, &NodeEvents)> = nodes.iter().collect();
    sorted.sort_by(|first, second| first.0.cmp(second.0));
    for (label, events) in sorted {
        writeln!(
            out,
            "  {CYAN}{label}{RESET}: mined={MAGENTA}{}{RESET}  received={CYAN}{}{RESET}  validated={GREEN}{}{RESET}  confirmed={GREEN}{BOLD}{}{RESET}  uncles={YELLOW}{}{RESET}",
            events.mined.len(),
            events.received.len(),
            events.validated.len(),
            events.confirmed.len(),
            events.uncles.len()
        )?;
    }
    Ok(())
}

/// Resolve arguments as glob patterns (`**` included) or plain paths, into a
/// sorted list of distinct files. An argument that matches nothing is
/// reported on stderr and skipped.
pub fn resolve_files(arguments: &[String]) -> Vec<PathBuf> {
    let mut resolved: Vec<PathBuf> = Vec::with_capacity(arguments.len());
    // Like Python's glob: wildcards do not match a leading dot.
    let options = glob::MatchOptions {
        require_literal_leading_dot: true,
        ..glob::MatchOptions::default()
    };
    for argument in arguments {
        let matches: Vec<PathBuf> = glob::glob_with(argument, options)
            .map(|paths| paths.filter_map(Result::ok).collect())
            .unwrap_or_default();
        if !matches.is_empty() {
            resolved.extend(matches.into_iter().filter(|path| path.is_file()));
        } else if Path::new(argument).is_file() {
            resolved.push(PathBuf::from(argument));
        } else {
            eprintln!("Warning: '{argument}' did not match any files");
        }
    }
    resolved.sort_by(|first, second| first.as_os_str().cmp(second.as_os_str()));
    resolved.dedup();
    resolved
}

#[cfg(test)]
mod tests;
