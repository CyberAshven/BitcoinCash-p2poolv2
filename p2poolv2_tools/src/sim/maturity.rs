// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Check from the outside that sim block-finds pay a late miner only once
//! its shares are mature. Replaces `load-tests/sim/maturity_check.py`.
//!
//! The late-miner scenario starts one node with its emitter off and turns it
//! on once the chain is past the maturity depth. This check reads only the
//! node logs, the node API and bitcoind, never the payout code, and checks
//! the payout maturity rule against the real regtest coinbases:
//!
//! * the late miner's bitcoin address is never paid by a block whose paying
//!   share is less than the coinbase maturity above the late miner's first
//!   share (confirmed or uncle), and
//! * it is paid by the first block whose paying share is at least the
//!   coinbase maturity above the late miner's first confirmed share.
//!
//! A share is paid through a nephew too, but an uncle is never above its
//! nephew, so the first bound uses the lowest of the late miner's confirmed
//! and uncle heights. The second uses confirmed shares only, since an uncle
//! is paid only once its nephew matures.

use crate::error::{ToolError, ToolResult};
use crate::log_file::for_each_line;
use crate::node_api::{ApiResponse, NodeApi};
use crate::sim::RunDir;
use crate::sim::bitcoind::{Bitcoind, CallOutcome};
use regex::Regex;
use serde_json::Value;
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::sync::LazyLock;
use std::time::Duration;

/// The verdict, which is also the process exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Pass = 0,
    Violation = 1,
    /// No block has reached the maturity depth for the late miner yet.
    Inconclusive = 2,
    SetupError = 3,
}

impl Verdict {
    pub fn label(self) -> &'static str {
        match self {
            Verdict::Pass => "PASS",
            Verdict::Violation => "FAIL",
            Verdict::Inconclusive => "INCONCLUSIVE",
            Verdict::SetupError => "SETUP ERROR",
        }
    }
}

/// Shares per `/shares` request; the API caps it at 100.
const SHARES_PAGE_SIZE: u64 = 100;

static BLOCK_FIND: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"sim block-find: .* submitting block (?P<bitcoin_hash>[0-9a-f]{64}) at height \d+ prev_share (?P<prev_share>[0-9a-f]{64})",
    )
    .expect("valid regex")
});
static SIM_DEPTHS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"Sim depths: coinbase maturity (\d+) shares").expect("valid regex")
});

/// One block-find a node logged, with what the check learned about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockRow {
    pub bitcoin_hash: String,
    pub paying_height: u64,
    pub late_paid: bool,
}

/// The settings the check runs with.
#[derive(Debug, Clone)]
pub struct MaturityCheck {
    pub run_dir: RunDir,
    pub late_miner: Option<usize>,
    pub base_api: u32,
}

/// The coinbase maturity of the sim build, from the `Sim depths` line a node
/// logs at startup. It is a compile-time constant, so every node has the
/// same value.
pub fn read_sim_maturity(run_dir: &RunDir, node: usize) -> ToolResult<u64> {
    let path = run_dir.node_log(node);
    let mut maturity: Option<u64> = None;
    for_each_line(&path, |line| {
        if maturity.is_none()
            && let Some(found) = SIM_DEPTHS.captures(line)
        {
            maturity = found[1].parse().ok();
        }
    })?;
    maturity
        .ok_or_else(|| ToolError::message(format!("{} has no 'Sim depths' line", path.display())))
}

/// `sim.miner_address` from a node config.
fn config_miner_address(path: &std::path::Path) -> ToolResult<String> {
    let text = std::fs::read_to_string(path).map_err(|error| ToolError::io("read", path, error))?;
    let config: toml::Table = text
        .parse()
        .map_err(|error| ToolError::message(format!("cannot parse {}: {error}", path.display())))?;
    config
        .get("sim")
        .and_then(|sim| sim.get("miner_address"))
        .and_then(toml::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| ToolError::message(format!("{} has no sim.miner_address", path.display())))
}

/// The check is meaningless if another node pays to the same address.
fn require_distinct_address(run_dir: &RunDir, late_address: &str) -> ToolResult<()> {
    let mut configs: Vec<std::path::PathBuf> = (0..run_dir.node_count())
        .map(|index| run_dir.node_config(index))
        .collect();
    configs.sort();
    let mut sharing = Vec::with_capacity(2);
    for path in &configs {
        if config_miner_address(path)? == late_address {
            sharing.push(
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            );
        }
    }
    if sharing.len() > 1 {
        return Err(ToolError::message(format!(
            "late miner address {late_address} is shared by {}; run the swarm with DISTINCT_ADDR=1 and a loaded wallet",
            sharing.join(", ")
        )));
    }
    Ok(())
}

/// Every block-find any node logged: bitcoin block hash and the share the
/// block's coinbase was built on.
fn collect_block_finds(run_dir: &RunDir) -> ToolResult<Vec<(String, String)>> {
    let mut finds = Vec::with_capacity(64);
    for log in run_dir.node_logs() {
        for_each_line(&log, |line| {
            if line.contains("sim block-find:")
                && let Some(found) = BLOCK_FIND.captures(line)
            {
                finds.push((
                    found["bitcoin_hash"].to_string(),
                    found["prev_share"].to_string(),
                ));
            }
        })?;
    }
    Ok(finds)
}

/// Addresses the coinbase of a verbose `getblock` result pays. Both the
/// `address` field (Bitcoin Core, BCHN 27 and later) and the older
/// `addresses` list are read.
pub fn coinbase_payees(block: &Value) -> BTreeSet<String> {
    let outputs = block
        .get("tx")
        .and_then(|transactions| transactions.get(0))
        .and_then(|coinbase| coinbase.get("vout"))
        .and_then(Value::as_array);
    let mut payees = BTreeSet::new();
    for output in outputs.into_iter().flatten() {
        let script = output.get("scriptPubKey");
        if let Some(address) = script
            .and_then(|script| script.get("address"))
            .and_then(Value::as_str)
        {
            payees.insert(address.to_string());
        }
        let listed = script
            .and_then(|script| script.get("addresses"))
            .and_then(Value::as_array);
        payees.extend(
            listed
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string),
        );
    }
    payees
}

/// Height of the share whose parent is `prev_share`, or `None` when the
/// queried node does not know `prev_share` or has no height for it.
async fn paying_height(api: &NodeApi, port: u32, prev_share: &str) -> ToolResult<Option<u64>> {
    // The endpoint answers with a list of shares; a hash lookup has one.
    match api.get(port, &format!("/share?hash={prev_share}")).await? {
        ApiResponse::NotFound => Ok(None),
        ApiResponse::Found(shares) => Ok(shares
            .get(0)
            .and_then(|share| share.get("height"))
            .and_then(Value::as_u64)
            .map(|height| height + 1)),
    }
}

/// Lowest heights the late miner has mined on the confirmed chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FirstHeights {
    /// Its lowest confirmed share.
    pub confirmed: Option<u64>,
    /// Its lowest share, confirmed or uncle.
    pub any: Option<u64>,
}

fn required_u64(value: &Value, field: &str) -> ToolResult<u64> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .ok_or_else(|| ToolError::message(format!("API response has no {field}")))
}

async fn late_miner_first_heights(
    api: &NodeApi,
    port: u32,
    late_address: &str,
) -> ToolResult<FirstHeights> {
    let tip = required_u64(&api.get_found(port, "/shares?num=1").await?, "to_height")?;
    let mut first = FirstHeights {
        confirmed: None,
        any: None,
    };
    let mut page_top = Some(tip);
    while let Some(top) = page_top {
        let page = api
            .get_found(port, &format!("/shares?to={top}&num={SHARES_PAGE_SIZE}"))
            .await?;
        let shares = page
            .get("shares")
            .and_then(Value::as_array)
            .ok_or_else(|| ToolError::message("API response has no shares"))?;
        for share in shares {
            if share.get("miner_bitcoin_address").and_then(Value::as_str) == Some(late_address) {
                let height = required_u64(share, "height")?;
                first.confirmed = Some(first.confirmed.map_or(height, |lowest| lowest.min(height)));
                first.any = Some(first.any.map_or(height, |lowest| lowest.min(height)));
            }
            let uncles = share.get("uncles").and_then(Value::as_array);
            for uncle in uncles.into_iter().flatten() {
                let height = uncle.get("height").and_then(Value::as_u64);
                if uncle.get("miner_bitcoin_address").and_then(Value::as_str) == Some(late_address)
                    && let Some(height) = height
                {
                    first.any = Some(first.any.map_or(height, |lowest| lowest.min(height)));
                }
            }
        }
        // Walk down a page at a time; a page that does not move down ends
        // the walk rather than looping forever.
        page_top = required_u64(&page, "from_height")?
            .checked_sub(1)
            .filter(|next| *next < top);
    }
    Ok(first)
}

/// The verdict and its reason for the per-block rows.
pub fn evaluate(
    rows: &[BlockRow],
    maturity: u64,
    first_confirmed: u64,
    first_any: u64,
) -> (Verdict, String) {
    let early: Vec<String> = rows
        .iter()
        .filter(|row| row.late_paid && row.paying_height < first_any + maturity)
        .map(|row| row.paying_height.to_string())
        .collect();
    if !early.is_empty() {
        return (
            Verdict::Violation,
            format!(
                "late miner paid before maturity at paying height(s) {}",
                early.join(", ")
            ),
        );
    }
    let first_mature = rows
        .iter()
        .filter(|row| row.paying_height >= first_confirmed + maturity)
        .min_by_key(|row| row.paying_height);
    match first_mature {
        None => (
            Verdict::Inconclusive,
            "no block-find has reached the maturity depth for the late miner yet".to_string(),
        ),
        Some(row) if !row.late_paid => (
            Verdict::Violation,
            format!(
                "late miner not paid by the first mature block-find at paying height {}",
                row.paying_height
            ),
        ),
        Some(row) => (
            Verdict::Pass,
            format!(
                "late miner first paid at paying height {}",
                row.paying_height
            ),
        ),
    }
}

/// The per-block report table.
pub fn report(rows: &[BlockRow], late: (usize, &str), maturity: u64, first: (u64, u64)) -> String {
    let (late_index, late_address) = late;
    let (first_confirmed, first_any) = first;
    let mut text = String::with_capacity(256 + rows.len() * 64);
    let _ = writeln!(
        text,
        "late miner: node {late_index} ({late_address}), coinbase maturity {maturity}"
    );
    let _ = writeln!(
        text,
        "first late share: confirmed {first_confirmed}, confirmed or uncle {first_any}"
    );
    let _ = writeln!(
        text,
        "{:<20} {:>9} {:>8} {:>7} {:>10}",
        "bitcoin block", "paying_h", "p-first", "mature", "late_paid"
    );
    let mut sorted: Vec<&BlockRow> = rows.iter().collect();
    sorted.sort_by_key(|row| row.paying_height);
    for row in sorted {
        let depth = row.paying_height as i64 - first_confirmed as i64;
        let short = format!(
            "{}..",
            row.bitcoin_hash.get(..16).unwrap_or(&row.bitcoin_hash)
        );
        let yes_no = |flag: bool| if flag { "yes" } else { "no" };
        let _ = writeln!(
            text,
            "{short:<20} {:>9} {depth:>8} {:>7} {:>10}",
            row.paying_height,
            yes_no(depth >= maturity as i64),
            yes_no(row.late_paid)
        );
    }
    text
}

impl MaturityCheck {
    /// Run the check and return the verdict with the full report, which ends
    /// with the `maturity check: <VERDICT> (<reason>)` line.
    pub async fn run(&self, bitcoind: &Bitcoind) -> (Verdict, String) {
        match self.run_checked(bitcoind).await {
            Ok(outcome) => outcome,
            Err(error) => (
                Verdict::SetupError,
                format!("maturity check: SETUP ERROR ({error})\n"),
            ),
        }
    }

    async fn run_checked(&self, bitcoind: &Bitcoind) -> ToolResult<(Verdict, String)> {
        let late_index = self
            .late_miner
            .ok_or_else(|| ToolError::message("LATE_MINER is not set"))?;
        let port = self.base_api;
        let late_address = config_miner_address(&self.run_dir.node_config(late_index))?;
        let maturity = read_sim_maturity(&self.run_dir, late_index)?;
        require_distinct_address(&self.run_dir, &late_address)?;

        let api = NodeApi::new(Duration::from_secs(10))?;
        let first = late_miner_first_heights(&api, port, &late_address).await?;
        let (Some(first_confirmed), Some(first_any)) = (first.confirmed, first.any) else {
            return Ok((
                Verdict::Inconclusive,
                "maturity check: INCONCLUSIVE (late miner has no confirmed share yet)\n"
                    .to_string(),
            ));
        };

        let mut rows = Vec::with_capacity(64);
        for (bitcoin_hash, prev_share) in collect_block_finds(&self.run_dir)? {
            let outcome = bitcoind
                .call(
                    "getblock",
                    vec![Value::from(bitcoin_hash.as_str()), Value::from(2)],
                )
                .await?;
            // A block bitcoind rejected (an RPC error here) pays nobody.
            if let CallOutcome::Result(block) = outcome
                && !block.is_null()
                && let Some(paying_height) = paying_height(&api, port, &prev_share).await?
            {
                rows.push(BlockRow {
                    late_paid: coinbase_payees(&block).contains(&late_address),
                    bitcoin_hash,
                    paying_height,
                });
            }
        }

        let mut text = report(
            &rows,
            (late_index, &late_address),
            maturity,
            (first_confirmed, first_any),
        );
        let (verdict, reason) = evaluate(&rows, maturity, first_confirmed, first_any);
        let _ = writeln!(text, "maturity check: {} ({reason})", verdict.label());
        Ok((verdict, text))
    }
}

#[cfg(test)]
mod tests;
