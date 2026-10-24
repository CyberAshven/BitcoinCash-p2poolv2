// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `netsim`: run one scenario, a sweep, or print the payee limits.
//!
//! ```text
//! cargo run --release -p p2poolv2_netsim -- run --miners 100000 --tokens
//! cargo run --release -p p2poolv2_netsim -- sweep payees
//! cargo run --release -p p2poolv2_netsim -- sweep all > results.md
//! cargo run --release -p p2poolv2_netsim -- limits
//! ```

use clap::{Args, Parser, Subcommand, ValueEnum};
use p2poolv2_netsim::constants::{
    BCH_MAX_COINBASE_PAYOUTS, BCH_VM_STACK_ELEMENT_BYTES, DEVICE_COINBASE_EVIDENCE,
    TOKEN_COINBASE_THRESHOLD_BYTES,
};
use p2poolv2_netsim::distribution::Distribution;
use p2poolv2_netsim::payees::HashrateSpread;
use p2poolv2_netsim::report::{Report, group_thousands, markdown_table};
use p2poolv2_netsim::scenario::{Scenario, ShareRelay};
use p2poolv2_netsim::sizes::{
    CoinbaseLayout, ShareSizes, largest_direct_payee_count, largest_payee_count_without_tree,
    split_payees,
};
use p2poolv2_netsim::sweep::{Sweep, run_scenarios};
use p2poolv2_netsim::traffic::StratumProtocol;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "netsim",
    version,
    about = "Simulate share propagation in a Bitcoin Cash P2Poolv2 network"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run one scenario.
    Run {
        #[command(flatten)]
        scenario: ScenarioArgs,
        /// Output format.
        #[arg(long, value_enum, default_value_t = Format::Text)]
        format: Format,
    },
    /// Run a sweep (payees, pool-size, block-size, bandwidth, latency,
    /// interval, nodes, seeds) or `all`, varied from the given scenario.
    Sweep {
        /// The sweep's name, or `all`.
        name: String,
        #[command(flatten)]
        scenario: ScenarioArgs,
        /// Output format.
        #[arg(long, value_enum, default_value_t = Format::Markdown)]
        format: Format,
    },
    /// Print the payee counts at which a coinbase meets each limit.
    Limits,
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Text,
    Json,
    Markdown,
}

#[derive(Clone, Copy, ValueEnum)]
enum PayoutRule {
    /// The hybrid payout rule, on PR #33 and on chipnet: payees direct up to
    /// BCH's 1 MB transaction limit without tokens, up to the token
    /// threshold with them, the rest through a payout tree.
    Hybrid,
    /// The earlier rule it replaced, as a historical comparison: at most 11
    /// payouts and a weighted tail draw.
    Cap11,
    /// Every payee direct with no limit (upstream's Bitcoin rule).
    Uncapped,
}

/// Scenario inputs; anything left out keeps the default.
#[derive(Args)]
struct ScenarioArgs {
    /// Label in tables.
    #[arg(long)]
    label: Option<String>,
    /// Seed of every random draw.
    #[arg(long)]
    seed: Option<u64>,
    /// P2Pool nodes.
    #[arg(long)]
    nodes: Option<usize>,
    /// Peers each node starts with.
    #[arg(long)]
    peers: Option<usize>,
    /// Chance each ring link is rewired to a random node.
    #[arg(long)]
    rewire: Option<f64>,
    /// How hashrate is spread over nodes: `equal` or `pareto:<shape>`.
    #[arg(long, value_parser = HashrateSpread::parse)]
    node_hashrate: Option<HashrateSpread>,
    /// Miners.
    #[arg(long)]
    miners: Option<u64>,
    /// How hashrate is spread over miners: `equal` or `pareto:<shape>`.
    #[arg(long, value_parser = HashrateSpread::parse)]
    miner_hashrate: Option<HashrateSpread>,
    /// Payees per coinbase, instead of the count the miners and the window
    /// give.
    #[arg(long)]
    payees: Option<u64>,
    /// The pool's fraction of BCH's hashrate (0.01 is 1%), which sizes the
    /// PPLNS window.
    #[arg(long)]
    pool_fraction: Option<f64>,
    /// Mean seconds between shares.
    #[arg(long)]
    share_interval: Option<f64>,
    /// BCH block bytes.
    #[arg(long)]
    block_bytes: Option<u64>,
    /// Average BCH transaction bytes.
    #[arg(long)]
    transaction_bytes: Option<u64>,
    /// Payout rule.
    #[arg(long, value_enum)]
    payout: Option<PayoutRule>,
    /// Shares carry merge-mined token commitments (hybrid rule only).
    #[arg(long)]
    tokens: bool,
    /// Coinbase bytes a share with tokens may use (hybrid rule only).
    #[arg(long, default_value_t = TOKEN_COINBASE_THRESHOLD_BYTES)]
    token_threshold: u64,
    /// Share transaction bytes beyond the share's own coinbase.
    #[arg(long)]
    extra_share_bytes: Option<u64>,
    /// How shares travel: `full` (as nodes send them) or `without-payouts`
    /// (suggested by the hybrid payout design, not built).
    #[arg(long, value_parser = ShareRelay::parse)]
    relay: Option<ShareRelay>,
    /// Node upload in Mbit/s: `50`, `uniform:10:100` or `lognormal:50:0.8`.
    #[arg(long, value_parser = Distribution::parse)]
    upload_mbps: Option<Distribution>,
    /// Node download in Mbit/s, same forms.
    #[arg(long, value_parser = Distribution::parse)]
    download_mbps: Option<Distribution>,
    /// One-way link latency in milliseconds, same forms.
    #[arg(long, value_parser = Distribution::parse)]
    latency_ms: Option<Distribution>,
    /// Seconds to validate any share.
    #[arg(long)]
    validation_seconds: Option<f64>,
    /// Further validation seconds per megabyte of share.
    #[arg(long)]
    validation_seconds_per_megabyte: Option<f64>,
    /// Seconds from a new best share to the miners hashing on it.
    #[arg(long)]
    work_switch_seconds: Option<f64>,
    /// BCHN has a link of its own instead of sharing the node's.
    #[arg(long)]
    separate_bchn_link: bool,
    /// BCHN peer links.
    #[arg(long)]
    bch_peers: Option<u64>,
    /// Stratum protocol: v1, v2-extended or v2-standard.
    #[arg(long, value_parser = StratumProtocol::parse)]
    stratum: Option<StratumProtocol>,
    /// Seconds of mining.
    #[arg(long)]
    duration: Option<f64>,
}

impl ScenarioArgs {
    fn scenario(&self) -> Scenario {
        let defaults = Scenario::default();
        let layout = match self.payout.unwrap_or(PayoutRule::Hybrid) {
            PayoutRule::Hybrid => CoinbaseLayout::hybrid(self.tokens, self.token_threshold),
            PayoutRule::Cap11 => CoinbaseLayout::cap_11(),
            PayoutRule::Uncapped => CoinbaseLayout::Uncapped,
        };
        Scenario {
            label: self.label.clone().unwrap_or(defaults.label),
            seed: self.seed.unwrap_or(defaults.seed),
            node_count: self.nodes.unwrap_or(defaults.node_count),
            peers_per_node: self.peers.unwrap_or(defaults.peers_per_node),
            rewire_probability: self.rewire.unwrap_or(defaults.rewire_probability),
            node_hashrate: self.node_hashrate.unwrap_or(defaults.node_hashrate),
            miner_count: self.miners.unwrap_or(defaults.miner_count),
            miner_hashrate: self.miner_hashrate.unwrap_or(defaults.miner_hashrate),
            payees_override: self.payees.or(defaults.payees_override),
            pool_fraction_of_bch: self.pool_fraction.unwrap_or(defaults.pool_fraction_of_bch),
            share_interval_seconds: self
                .share_interval
                .unwrap_or(defaults.share_interval_seconds),
            block_bytes: self.block_bytes.unwrap_or(defaults.block_bytes),
            average_transaction_bytes: self
                .transaction_bytes
                .unwrap_or(defaults.average_transaction_bytes),
            layout,
            relay: self.relay.unwrap_or(defaults.relay),
            extra_share_bytes: self.extra_share_bytes.unwrap_or(defaults.extra_share_bytes),
            upload_megabits: self.upload_mbps.unwrap_or(defaults.upload_megabits),
            download_megabits: self.download_mbps.unwrap_or(defaults.download_megabits),
            latency_milliseconds: self.latency_ms.unwrap_or(defaults.latency_milliseconds),
            validation_seconds: self
                .validation_seconds
                .unwrap_or(defaults.validation_seconds),
            validation_seconds_per_megabyte: self
                .validation_seconds_per_megabyte
                .unwrap_or(defaults.validation_seconds_per_megabyte),
            work_switch_seconds: self
                .work_switch_seconds
                .unwrap_or(defaults.work_switch_seconds),
            bchn_shares_link: !self.separate_bchn_link,
            bch_peers: self.bch_peers.unwrap_or(defaults.bch_peers),
            stratum: self.stratum.unwrap_or(defaults.stratum),
            duration_seconds: self.duration.unwrap_or(defaults.duration_seconds),
        }
    }
}

fn print_reports(reports: &[Report], format: Format) -> Result<(), String> {
    match format {
        Format::Text => {
            let texts: Vec<String> = reports.iter().map(Report::summary_text).collect();
            println!("{}", texts.join("\n\n"));
        }
        Format::Json => {
            let json = serde_json::to_string_pretty(reports).map_err(|error| error.to_string())?;
            println!("{json}");
        }
        Format::Markdown => println!("{}", markdown_table(reports)),
    }
    Ok(())
}

fn run_sweep(name: &str, base: &Scenario, format: Format) -> Result<(), String> {
    let sweeps: Vec<Sweep> = if name == "all" {
        Sweep::ALL.to_vec()
    } else {
        vec![Sweep::parse(name)?]
    };
    sweeps.iter().try_for_each(|sweep| {
        let reports = run_scenarios(&sweep.scenarios(base));
        if matches!(format, Format::Markdown) {
            println!("### Sweep: {}\n", sweep.name());
        }
        print_reports(&reports, format)?;
        println!();
        Ok(())
    })
}

fn print_limits() {
    let limit = |name: &str, fits: &dyn Fn(&ShareSizes) -> bool| {
        let largest = largest_direct_payee_count(fits);
        println!(
            "{name}: at most {} direct P2PKH payees",
            group_thousands(largest)
        );
    };
    let check = |sizes: &ShareSizes| sizes.limit_check(CoinbaseLayout::Uncapped);
    limit(
        "200 KiB share body before the hybrid rule (P2Pool share-chain rule, historical)",
        &|sizes| check(sizes).share_body_within_pre_hybrid_limit,
    );
    limit(
        "1,204,800-byte share body of the hybrid rule (P2Pool share-chain rule)",
        &|sizes| check(sizes).share_body_within_hybrid_limit,
    );
    limit("1.25 MiB P2P message (node implementation)", &|sizes| {
        check(sizes).message_within_p2p_limit
    });
    limit("1,000,000-byte transaction (BCH consensus)", &|sizes| {
        check(sizes).coinbase_within_bch_transaction_limit
    });
    limit(
        "65,535-byte coinbase field of an SV2 extended job (SV2)",
        &|sizes| check(sizes).coinbase_within_sv2_extended_fields,
    );
    limit(
        "10,000-byte VM element for a token claim (BCH consensus)",
        &|sizes| sizes.coinbase_bytes() <= BCH_VM_STACK_ELEMENT_BYTES,
    );
    limit(
        "784-byte OCEAN block on stock Antminer firmware (evidence)",
        &|sizes| check(sizes).coinbase_within_asic_evidence,
    );
    DEVICE_COINBASE_EVIDENCE
        .iter()
        .for_each(|&(device, bytes)| {
            limit(
                &format!(
                    "{} bytes for {device} in DATUM (evidence)",
                    group_thousands(bytes)
                ),
                &|sizes| sizes.coinbase_bytes() <= bytes,
            );
        });
    let hybrid = |name: &str, layout: CoinbaseLayout| {
        let beside_tree = split_payees(layout, u64::from(u32::MAX));
        println!(
            "{name}: {} payees direct without a tree; past that {} direct beside the tree, a {}-byte coinbase",
            group_thousands(largest_payee_count_without_tree(layout)),
            group_thousands(beside_tree.direct_outputs),
            group_thousands(beside_tree.coinbase_bytes)
        );
    };
    hybrid(
        "Hybrid rule without tokens (C = 1,000,000)",
        CoinbaseLayout::no_tokens(),
    );
    hybrid(
        &format!(
            "Hybrid rule with tokens (T = {})",
            group_thousands(TOKEN_COINBASE_THRESHOLD_BYTES)
        ),
        CoinbaseLayout::with_tokens(),
    );
    let capped = split_payees(CoinbaseLayout::cap_11(), u64::from(u32::MAX));
    println!(
        "The earlier cap of {BCH_MAX_COINBASE_PAYOUTS} payouts (historical, replaced by the hybrid rule): a {}-byte coinbase",
        capped.coinbase_bytes
    );
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Run { scenario, format } => {
            let scenario = scenario.scenario();
            scenario
                .validate()
                .and_then(|()| print_reports(&[scenario.run()], format))
        }
        Command::Sweep {
            name,
            scenario,
            format,
        } => {
            let base = scenario.scenario();
            base.validate()
                .and_then(|()| run_sweep(&name, &base, format))
        }
        Command::Limits => {
            print_limits();
            Ok(())
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("netsim: {error}");
            ExitCode::FAILURE
        }
    }
}
