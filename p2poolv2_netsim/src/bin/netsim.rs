// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `netsim`: run one scenario, a sweep, or print the payee limits.
//!
//! ```text
//! cargo run --release -p p2poolv2_netsim -- run --miners 100000 --tokens
//! cargo run --release -p p2poolv2_netsim -- sweep payees
//! cargo run --release -p p2poolv2_netsim -- sweep all > results.md
//! cargo run --release -p p2poolv2_netsim -- sweep scaling --threads 16
//! cargo run --release -p p2poolv2_netsim -- interval-limit scaling --nodes 1000
//! cargo run --release -p p2poolv2_netsim -- limits
//! ```

use clap::{Args, Parser, Subcommand, ValueEnum};
use p2poolv2_netsim::constants::{
    BCH_MAX_COINBASE_PAYOUTS, BCH_VM_STACK_ELEMENT_BYTES, DEVICE_COINBASE_EVIDENCE,
    TOKEN_COINBASE_THRESHOLD_BYTES,
};
use p2poolv2_netsim::distribution::Distribution;
use p2poolv2_netsim::engine::Forwarding;
use p2poolv2_netsim::payees::HashrateSpread;
use p2poolv2_netsim::report::{
    Report, group_thousands, markdown_table, percent, scaling_markdown_table,
};
use p2poolv2_netsim::scaling::{
    Idea, REFERENCE_SHARE_INTERVAL_SECONDS, SEARCH_LONGEST_INTERVAL_SECONDS,
    SEARCH_SHORTEST_INTERVAL_SECONDS, SEARCH_STEPS, at_size, shortest_interval_within,
};
use p2poolv2_netsim::scenario::{HEADER_CHECK_SECONDS, Scenario, ShareRelay, WeakShareCarriage};
use p2poolv2_netsim::sizes::{
    CoinbaseLayout, ShareSizes, largest_direct_payee_count, largest_payee_count_without_tree,
    split_payees,
};
use p2poolv2_netsim::sweep::{Sweep, available_threads, parallel_map, run_scenarios_on};
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
    /// interval, nodes, seeds), `all` of those, one sweep of the scaling
    /// study (scaling-today, scaling-weak-shares, scaling-compact-coinbase,
    /// scaling-header-check, scaling-peers, scaling-uncle-cap,
    /// scaling-combined) or `scaling` for all of them, varied from the given
    /// scenario.
    Sweep {
        /// The sweep's name, `all` or `scaling`.
        name: String,
        #[command(flatten)]
        scenario: ScenarioArgs,
        /// Output format.
        #[arg(long, value_enum, default_value_t = Format::Markdown)]
        format: Format,
        /// Runs at once; each holds its shares in memory. Default: every
        /// thread.
        #[arg(long)]
        threads: Option<usize>,
    },
    /// For each scenario of a scaling study sweep (or `scaling` for all),
    /// on the given scenario's node count, the shortest share interval whose
    /// stale rate stays within today's at 10-second shares on that many
    /// nodes, by bisection.
    IntervalLimit {
        /// A scaling sweep's name, or `scaling`.
        name: String,
        #[command(flatten)]
        scenario: ScenarioArgs,
        /// The stale rate to stay within (0.0423 is 4.23%). Default: today's
        /// at 10-second shares on the scenario's node count.
        #[arg(long)]
        stale_ceiling: Option<f64>,
        /// Searches at once. Default: every thread.
        #[arg(long)]
        threads: Option<usize>,
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
    /// Most uncles a share names (3 today).
    #[arg(long)]
    max_uncles: Option<usize>,
    /// When a node forwards a share (studied, not built: after-header-check).
    #[arg(long, value_enum)]
    forward: Option<ForwardingRule>,
    /// Seconds the header check takes when forwarding after it.
    #[arg(long, default_value_t = HEADER_CHECK_SECONDS)]
    header_check_seconds: f64,
    /// Weak shares found per share, gossiped beside the shares (studied, not
    /// built).
    #[arg(long)]
    weak_shares: Option<u64>,
    /// Payload bytes of a weak share message.
    #[arg(long)]
    weak_share_bytes: Option<u64>,
    /// What a share carries of its weak shares: `gossip-only` (nothing) or
    /// `whole` (their messages in its body).
    #[arg(long, value_parser = WeakShareCarriage::parse)]
    weak_share_carriage: Option<WeakShareCarriage>,
}

#[derive(Clone, Copy, ValueEnum)]
enum ForwardingRule {
    /// Once the share is fully validated, as nodes do today.
    AfterValidation,
    /// Once its proof of work and header check out, its body validated
    /// after.
    AfterHeaderCheck,
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
            max_uncles: self.max_uncles.unwrap_or(defaults.max_uncles),
            forwarding: match self.forward {
                None => defaults.forwarding,
                Some(ForwardingRule::AfterValidation) => Forwarding::AfterValidation,
                Some(ForwardingRule::AfterHeaderCheck) => Forwarding::AfterHeaderCheck {
                    header_check_seconds: self.header_check_seconds,
                },
            },
            weak_shares_per_share: self.weak_shares.unwrap_or(defaults.weak_shares_per_share),
            weak_share_payload_bytes: self
                .weak_share_bytes
                .unwrap_or(defaults.weak_share_payload_bytes),
            weak_share_carriage: self
                .weak_share_carriage
                .unwrap_or(defaults.weak_share_carriage),
        }
    }
}

/// Print `reports`; a Markdown table in the scaling study's columns when
/// `scaling` is set.
fn print_reports(reports: &[Report], format: Format, scaling: bool) -> Result<(), String> {
    match format {
        Format::Text => {
            let texts: Vec<String> = reports.iter().map(Report::summary_text).collect();
            println!("{}", texts.join("\n\n"));
        }
        Format::Json => {
            let json = serde_json::to_string_pretty(reports).map_err(|error| error.to_string())?;
            println!("{json}");
        }
        Format::Markdown if scaling => println!("{}", scaling_markdown_table(reports)),
        Format::Markdown => println!("{}", markdown_table(reports)),
    }
    Ok(())
}

/// The sweeps `name` stands for: one, `all` or `scaling`.
fn sweeps_named(name: &str) -> Result<Vec<Sweep>, String> {
    match name {
        "all" => Ok(Sweep::ALL.to_vec()),
        "scaling" => Ok(Sweep::SCALING.to_vec()),
        _ => Sweep::parse(name).map(|sweep| vec![sweep]),
    }
}

fn run_sweep(name: &str, base: &Scenario, format: Format, threads: usize) -> Result<(), String> {
    sweeps_named(name)?.iter().try_for_each(|sweep| {
        let reports = run_scenarios_on(&sweep.scenarios(base), threads);
        if matches!(format, Format::Markdown) {
            println!("### Sweep: {}\n", sweep.name());
        }
        print_reports(&reports, format, sweep.is_scaling())?;
        println!();
        Ok(())
    })
}

/// The scaling ideas `name` stands for: one sweep's, or `scaling` for all.
fn ideas_named(name: &str) -> Result<Vec<Idea>, String> {
    if name == "scaling" {
        return Ok(Idea::ALL.to_vec());
    }
    Idea::ALL
        .iter()
        .copied()
        .find(|idea| idea.name() == name)
        .map(|idea| vec![idea])
        .ok_or_else(|| {
            let names: Vec<&str> = Idea::ALL.iter().map(Idea::name).collect();
            format!(
                "unknown scaling sweep `{name}`; expected scaling or one of {}",
                names.join(", ")
            )
        })
}

/// Where one scenario's limit search ended.
struct LimitFound {
    label: String,
    interval_seconds: Option<f64>,
    report: Option<Report>,
}

/// Search each scaling scenario `name` stands for, on `base`'s node count,
/// for the shortest share interval within the stale ceiling, and print a
/// Markdown table.
fn run_interval_limit(
    name: &str,
    base: &Scenario,
    stale_ceiling: Option<f64>,
    threads: usize,
) -> Result<(), String> {
    let node_count = base.node_count;
    let ceiling = stale_ceiling.unwrap_or_else(|| {
        let today = Idea::Today.variants(base);
        at_size(&today[0], node_count, REFERENCE_SHARE_INTERVAL_SECONDS)
            .run()
            .propagation
            .stale_rate
    });
    let variants: Vec<Scenario> = ideas_named(name)?
        .iter()
        .flat_map(|idea| idea.variants(base))
        .collect();
    let found = parallel_map(&variants, threads, |variant| {
        let mut reports: Vec<(f64, Report)> = Vec::with_capacity(2 + SEARCH_STEPS as usize);
        let interval_seconds = shortest_interval_within(
            ceiling,
            REFERENCE_SHARE_INTERVAL_SECONDS,
            SEARCH_SHORTEST_INTERVAL_SECONDS,
            SEARCH_LONGEST_INTERVAL_SECONDS,
            SEARCH_STEPS,
            |interval| {
                let report = at_size(variant, node_count, interval).run();
                let stale = report.propagation.stale_rate;
                reports.push((interval, report));
                stale
            },
        );
        let report = interval_seconds.and_then(|limit| {
            reports
                .into_iter()
                .find(|(interval, _)| *interval == limit)
                .map(|(_, report)| report)
        });
        LimitFound {
            label: variant.label.clone(),
            interval_seconds,
            report,
        }
    });
    println!(
        "Shortest share interval within {} stale on {} nodes\n",
        percent(ceiling),
        group_thousands(node_count as u64)
    );
    println!(
        "| Scenario | Shortest share interval (s) | Shares per second against 10-second shares | Stale there | Orphaned there |\n\
         |---|---:|---:|---:|---:|"
    );
    found
        .iter()
        .for_each(|limit| println!("{}", limit_row(limit)));
    Ok(())
}

/// One row of the interval limit table.
fn limit_row(limit: &LimitFound) -> String {
    match (limit.interval_seconds, &limit.report) {
        (Some(interval), Some(report)) => format!(
            "| {} | {}{:.2} | {:.2} | {} | {} |",
            limit.label,
            if interval <= SEARCH_SHORTEST_INTERVAL_SECONDS {
                "at most "
            } else {
                ""
            },
            interval,
            REFERENCE_SHARE_INTERVAL_SECONDS / interval,
            percent(report.propagation.stale_rate),
            percent(report.propagation.orphan_rate),
        ),
        _ => format!(
            "| {} | above {SEARCH_LONGEST_INTERVAL_SECONDS} | - | - | - |",
            limit.label
        ),
    }
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
                .and_then(|()| print_reports(&[scenario.run()], format, false))
        }
        Command::Sweep {
            name,
            scenario,
            format,
            threads,
        } => {
            let base = scenario.scenario();
            let threads = threads.unwrap_or_else(available_threads);
            base.validate()
                .and_then(|()| run_sweep(&name, &base, format, threads))
        }
        Command::IntervalLimit {
            name,
            scenario,
            stale_ceiling,
            threads,
        } => {
            let base = scenario.scenario();
            let threads = threads.unwrap_or_else(available_threads);
            base.validate()
                .and_then(|()| run_interval_limit(&name, &base, stale_ceiling, threads))
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
