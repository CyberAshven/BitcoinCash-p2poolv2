// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What a run reports, as JSON or as a Markdown table row.

use crate::engine::{Network, Outcome, Parameters};
use crate::scenario::{NetworkSummary, Scenario};
use crate::sizes::{LimitCheck, ShareSizes};
use crate::traffic::{
    BchnLoad, coinbase_relay_seconds, megabits, orphan_risk, stratum_job_bytes,
    stratum_jobs_per_second,
};
use serde::Serialize;

/// Links a found block crosses to reach most BCH miners, for the coinbase's
/// share of block relay time. An estimate.
const BCH_RELAY_HOPS: u64 = 4;

/// Bytes per second in a megabit per second.
const BYTES_PER_MEGABIT: f64 = 125_000.0;

/// What happened to the shares.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PropagationSummary {
    /// Shares found.
    pub shares_found: u64,
    /// Shares the rates count.
    pub shares_counted: u64,
    /// Counted shares on the final chain.
    pub chain_shares: u64,
    /// Counted shares that became uncles (paid 9/10).
    pub uncle_shares: u64,
    /// Counted shares orphaned (paid nothing).
    pub orphan_shares: u64,
    /// Fraction of counted shares off the chain.
    pub stale_rate: f64,
    /// Fraction of counted shares that became uncles.
    pub uncle_rate: f64,
    /// Fraction of counted shares orphaned.
    pub orphan_rate: f64,
    /// Mean seconds until half the nodes accepted a share.
    pub mean_seconds_to_half: f64,
    /// Mean seconds until nine nodes in ten accepted a share.
    pub mean_seconds_to_ninety_percent: f64,
    /// Mean seconds until every node accepted a share.
    pub mean_seconds_to_all: f64,
    /// `ShareBlock` messages per share per node: one would be a perfect
    /// relay, more is the cost of pushing whole shares.
    pub copies_received_per_share: f64,
    /// Nodes whose share uploads needed more than their upload rate.
    pub saturated_nodes: u64,
}

/// Bytes per second per node, by what carries them.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BandwidthSummary {
    /// P2Pool share-chain upload, mean over nodes, megabits per second.
    pub p2pool_upload_megabits_mean: f64,
    /// P2Pool share-chain upload of the busiest node.
    pub p2pool_upload_megabits_max: f64,
    /// P2Pool share-chain download, mean over nodes.
    pub p2pool_download_megabits_mean: f64,
    /// P2Pool share-chain download of the busiest node.
    pub p2pool_download_megabits_max: f64,
    /// BCHN's transaction relay, each way.
    pub bchn_relay_megabits: f64,
    /// Bytes of one compact block.
    pub compact_block_bytes: u64,
    /// Full block templates fetched from BCHN (node to BCHN, often local).
    pub template_megabits: f64,
    /// Bytes of one Stratum job for one miner.
    pub stratum_job_bytes: u64,
    /// Stratum jobs per miner per second.
    pub stratum_jobs_per_second: f64,
    /// Stratum jobs to one miner.
    pub stratum_megabits_per_miner: f64,
    /// Miners on the average node.
    pub miners_per_node: f64,
    /// Stratum jobs from the average node to its miners.
    pub stratum_megabits_per_node: f64,
    /// Seconds the coinbase adds to relaying a found block across four
    /// links at the median upload.
    pub coinbase_relay_seconds: f64,
    /// Extra chance a found block is orphaned on BCH because of that delay.
    pub coinbase_orphan_risk: f64,
}

/// Everything one run reports.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Report {
    /// The inputs.
    pub scenario: Scenario,
    /// Shares in the PPLNS window.
    pub window_shares: u64,
    /// Payees per coinbase.
    pub payees: u64,
    /// Share and coinbase sizes.
    pub sizes: ShareSizes,
    /// Wire bytes of a `ShareBlock` message without uncles.
    pub share_wire_bytes: u64,
    /// The limits the share meets.
    pub limits: LimitCheck,
    /// The limits the share breaks, worst first.
    pub broken_limits: Vec<String>,
    /// The network.
    pub network: NetworkSummary,
    /// Share propagation.
    pub propagation: PropagationSummary,
    /// Bandwidth.
    pub bandwidth: BandwidthSummary,
}

impl Report {
    /// Put a report together.
    pub fn new(
        scenario: Scenario,
        sizes: ShareSizes,
        load: BchnLoad,
        network_summary: NetworkSummary,
        parameters: Parameters,
        network: &Network,
        outcome: Outcome,
    ) -> Self {
        let limits = sizes.limit_check(scenario.layout);
        let node_count = network.nodes.len().max(1) as f64;
        let copies_received_per_share = if outcome.shares_found == 0 {
            0.0
        } else {
            outcome.share_messages as f64 / (outcome.shares_found as f64 * node_count)
        };
        let propagation = PropagationSummary {
            shares_found: outcome.shares_found,
            shares_counted: outcome.shares_counted,
            chain_shares: outcome.chain_shares,
            uncle_shares: outcome.uncle_shares,
            orphan_shares: outcome.orphan_shares,
            stale_rate: outcome.stale_rate,
            uncle_rate: outcome.uncle_rate,
            orphan_rate: outcome.orphan_rate,
            mean_seconds_to_half: outcome.mean_seconds_to_half,
            mean_seconds_to_ninety_percent: outcome.mean_seconds_to_ninety_percent,
            mean_seconds_to_all: outcome.mean_seconds_to_all,
            copies_received_per_share,
            saturated_nodes: outcome.saturated_nodes,
        };
        let job_bytes = stratum_job_bytes(
            scenario.stratum,
            sizes.coinbase_bytes(),
            sizes.merkle_branch_hashes,
        );
        let jobs_per_second = stratum_jobs_per_second(scenario.share_interval_seconds);
        let stratum_per_miner = job_bytes as f64 * jobs_per_second;
        let miners_per_node = scenario.miner_count as f64 / node_count;
        let relay_seconds = coinbase_relay_seconds(
            sizes.coinbase_bytes(),
            scenario.upload_megabits.median() * BYTES_PER_MEGABIT,
            BCH_RELAY_HOPS,
        );
        let bandwidth = BandwidthSummary {
            p2pool_upload_megabits_mean: megabits(mean(&outcome.node_upload_bytes_per_second)),
            p2pool_upload_megabits_max: megabits(maximum(&outcome.node_upload_bytes_per_second)),
            p2pool_download_megabits_mean: megabits(mean(&outcome.node_download_bytes_per_second)),
            p2pool_download_megabits_max: megabits(maximum(
                &outcome.node_download_bytes_per_second,
            )),
            bchn_relay_megabits: megabits(load.relay_bytes_per_second),
            compact_block_bytes: load.compact_block_bytes,
            template_megabits: megabits(load.template_bytes_per_second),
            stratum_job_bytes: job_bytes,
            stratum_jobs_per_second: jobs_per_second,
            stratum_megabits_per_miner: megabits(stratum_per_miner),
            miners_per_node,
            stratum_megabits_per_node: megabits(stratum_per_miner * miners_per_node),
            coinbase_relay_seconds: relay_seconds,
            coinbase_orphan_risk: orphan_risk(relay_seconds),
        };
        Self {
            window_shares: scenario.window_shares(),
            payees: scenario.payees(),
            share_wire_bytes: parameters.share_wire_bytes,
            broken_limits: limits.broken().into_iter().map(str::to_string).collect(),
            limits,
            sizes,
            network: network_summary,
            propagation,
            bandwidth,
            scenario,
        }
    }

    /// A few lines a person can read.
    pub fn summary_text(&self) -> String {
        let propagation = &self.propagation;
        let bandwidth = &self.bandwidth;
        let broken = if self.broken_limits.is_empty() {
            "none".to_string()
        } else {
            self.broken_limits.join(", ")
        };
        format!(
            "{label}: {nodes} nodes, {miners} miners, {payees} payees (window {window} shares), {layout} coinbase\n\
             coinbase {coinbase} bytes ({direct} direct outputs, {tree} payees in a tree, {drawn} in the draw); share {share} bytes on the wire ({share_relay} relay)\n\
             limits broken: {broken}\n\
             shares {found} found, {counted} counted: stale {stale}, uncles {uncles}, orphans {orphans}\n\
             propagation: half the nodes in {half} s, 90% in {ninety} s, all in {all} s; {copies} copies per share per node\n\
             P2Pool per node: up {up_mean} Mbit/s mean, {up_max} max; down {down_mean} mean, {down_max} max; {saturated} nodes saturated\n\
             BCHN relay {relay} Mbit/s each way ({short} nodes short of it, mining none of the pool's hashrate); compact block {compact} bytes; full templates {template} Mbit/s from BCHN\n\
             Stratum {protocol}: {job} bytes a job, {per_miner} Mbit/s a miner, {per_node} Mbit/s for {miners_per_node} miners a node\n\
             a found block's coinbase adds {relay_seconds} s over four BCH hops: {orphan_risk} extra BCH orphan risk",
            label = self.scenario.label,
            nodes = self.scenario.node_count,
            miners = group_thousands(self.scenario.miner_count),
            payees = group_thousands(self.payees),
            window = group_thousands(self.window_shares),
            layout = self.scenario.layout.name(),
            share_relay = self.scenario.relay.name(),
            coinbase = group_thousands(self.sizes.coinbase_bytes()),
            direct = group_thousands(self.sizes.split.direct_outputs),
            tree = group_thousands(self.sizes.split.tree_payees),
            drawn = group_thousands(self.sizes.split.drawn_payees),
            share = group_thousands(self.share_wire_bytes),
            found = propagation.shares_found,
            counted = propagation.shares_counted,
            stale = percent(propagation.stale_rate),
            uncles = percent(propagation.uncle_rate),
            orphans = percent(propagation.orphan_rate),
            half = seconds(propagation.mean_seconds_to_half),
            ninety = seconds(propagation.mean_seconds_to_ninety_percent),
            all = seconds(propagation.mean_seconds_to_all),
            copies = rounded(propagation.copies_received_per_share),
            up_mean = rounded(bandwidth.p2pool_upload_megabits_mean),
            up_max = rounded(bandwidth.p2pool_upload_megabits_max),
            down_mean = rounded(bandwidth.p2pool_download_megabits_mean),
            down_max = rounded(bandwidth.p2pool_download_megabits_max),
            saturated = propagation.saturated_nodes,
            relay = rounded(bandwidth.bchn_relay_megabits),
            short = self.network.nodes_short_for_bchn,
            compact = group_thousands(bandwidth.compact_block_bytes),
            template = rounded(bandwidth.template_megabits),
            protocol = self.scenario.stratum.name(),
            job = group_thousands(bandwidth.stratum_job_bytes),
            per_miner = rounded(bandwidth.stratum_megabits_per_miner),
            per_node = rounded(bandwidth.stratum_megabits_per_node),
            miners_per_node = rounded(bandwidth.miners_per_node),
            relay_seconds = rounded(bandwidth.coinbase_relay_seconds),
            orphan_risk = percent(bandwidth.coinbase_orphan_risk),
        )
    }
}

/// The header of the Markdown table `markdown_row` fills.
pub fn markdown_header() -> String {
    "| Scenario | Payees | Coinbase bytes | Share bytes | Stale | Orphaned | To 90% (s) | To all (s) | P2Pool up, mean / max (Mbit/s) | BCHN relay (Mbit/s) | Templates (Mbit/s) | Stratum per miner (kbit/s) | Limits broken |\n\
     |---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|"
        .to_string()
}

/// One report as a Markdown table row.
pub fn markdown_row(report: &Report) -> String {
    let mut broken = report.broken_limits.clone();
    if report.network.nodes_short_for_bchn > 0 {
        broken.push(format!(
            "BCHN relay over the link on {} of {} nodes",
            report.network.nodes_short_for_bchn, report.scenario.node_count
        ));
    }
    let broken = if broken.is_empty() {
        "none".to_string()
    } else {
        broken.join(", ")
    };
    format!(
        "| {} | {} | {} | {} | {} | {} | {} | {} | {} / {} | {} | {} | {} | {} |",
        report.scenario.label,
        group_thousands(report.payees),
        group_thousands(report.sizes.coinbase_bytes()),
        group_thousands(report.share_wire_bytes),
        percent(report.propagation.stale_rate),
        percent(report.propagation.orphan_rate),
        seconds(report.propagation.mean_seconds_to_ninety_percent),
        seconds(report.propagation.mean_seconds_to_all),
        rounded(report.bandwidth.p2pool_upload_megabits_mean),
        rounded(report.bandwidth.p2pool_upload_megabits_max),
        rounded(report.bandwidth.bchn_relay_megabits),
        rounded(report.bandwidth.template_megabits),
        rounded(report.bandwidth.stratum_megabits_per_miner * 1_000.0),
        broken,
    )
}

/// A Markdown table of reports.
pub fn markdown_table(reports: &[Report]) -> String {
    let mut table = markdown_header();
    reports.iter().for_each(|report| {
        table.push('\n');
        table.push_str(&markdown_row(report));
    });
    table
}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

fn maximum(values: &[f64]) -> f64 {
    values.iter().copied().fold(0.0, f64::max)
}

/// `1234567` as `1,234,567`.
pub fn group_thousands(value: u64) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    digits.chars().enumerate().for_each(|(index, digit)| {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    });
    grouped
}

/// A fraction as a percentage with two decimals.
pub fn percent(fraction: f64) -> String {
    format!("{:.2}%", fraction * 100.0)
}

/// Seconds with two decimals, or "never" for a share some node never got.
pub fn seconds(value: f64) -> String {
    if value.is_finite() {
        format!("{value:.2}")
    } else {
        "never".to_string()
    }
}

/// A rate with about three significant digits.
pub fn rounded(value: f64) -> String {
    let magnitude = value.abs();
    if !value.is_finite() {
        "never".to_string()
    } else if magnitude >= 100.0 {
        group_thousands(value.round() as u64)
    } else if magnitude >= 10.0 {
        format!("{value:.1}")
    } else if magnitude >= 1.0 {
        format!("{value:.2}")
    } else if magnitude >= 0.01 {
        format!("{value:.3}")
    } else if magnitude == 0.0 {
        "0".to_string()
    } else {
        format!("{value:.1e}")
    }
}

#[cfg(test)]
mod tests;
