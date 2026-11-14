// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `payout-sim`: print the result tables, or run one scenario.
//!
//! ```text
//! cargo payout-sim                         # every table, in Markdown
//! cargo payout-sim --scenario attack-spam  # one table
//! cargo payout-sim run --design band --pool-fraction 0.1 --mix 90-industrial
//! ```

use clap::{Args, Parser, Subcommand, ValueEnum};
use p2poolv2_payout_sim::asks::WeakAsks;
use p2poolv2_payout_sim::levels::require_payout_level;
use p2poolv2_payout_sim::miners::MinerMix;
use p2poolv2_payout_sim::report::{
    hours, optional, percent, pool_percent, rounded, satoshis, two_decimals,
};
use p2poolv2_payout_sim::scenario::{Design, Report, Scenario, WeakCredit, stale_rate_for_period};
use p2poolv2_payout_sim::tables::{Table, base_scenario};
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "payout-sim",
    version,
    about = "Monte Carlo of payout variance and fees under Bitcoin Cash share chain designs"
)]
struct Cli {
    /// The table to print (`all`, or one name: designs, hashrate, pool,
    /// weak-credit, band-cap, weak-constants, includer-cut, levels, period,
    /// attack-minimum, attack-carriers, attack-spam, and study #85's
    /// refusing-races, mixes, arrivals, tiny-pool, floods, draw-steering,
    /// losses, weak-depth, r-max).
    #[arg(long, default_value = "all")]
    scenario: String,
    /// Months each scenario simulates.
    #[arg(long, default_value_t = 1_000)]
    months: usize,
    /// Seed of every random draw.
    #[arg(long, default_value_t = 1)]
    seed: u64,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run one scenario and print its numbers.
    Run(RunArgs),
}

#[derive(Clone, Copy, ValueEnum)]
enum DesignArg {
    Mini,
    OneChain,
    Band,
    VardiffCd,
    BandWeak,
    BandWeakLevels,
}

#[derive(Clone, Copy, ValueEnum)]
enum MixArg {
    Home,
    #[value(name = "50-industrial")]
    HalfIndustrial,
    #[value(name = "90-industrial")]
    MostlyIndustrial,
    #[value(name = "99-industrial")]
    AlmostAllIndustrial,
    Zipf,
    Medium,
    MediumFewBig,
}

#[derive(Clone, Copy, ValueEnum)]
enum CreditArg {
    Page,
    WeakWork,
}

/// One scenario's inputs; anything left out keeps the default.
#[derive(Args)]
struct RunArgs {
    #[arg(long, value_enum, default_value_t = DesignArg::BandWeakLevels)]
    design: DesignArg,
    /// The pool's part of Bitcoin Cash's hashrate (0.01 is 1%).
    #[arg(long, default_value_t = 0.01)]
    pool_fraction: f64,
    /// Bitcoin Cash's hashrate as a multiple of 3.7 EH/s.
    #[arg(long, default_value_t = 1.0)]
    hashrate_multiple: f64,
    #[arg(long, value_enum, default_value_t = MixArg::HalfIndustrial)]
    mix: MixArg,
    /// Share period, seconds.
    #[arg(long, default_value_t = 10.0)]
    period: f64,
    /// Stale share rate; by default the network simulation's at the period.
    #[arg(long)]
    stale_rate: Option<f64>,
    /// The band cap C.
    #[arg(long, default_value_t = 64.0)]
    band_cap: f64,
    #[arg(long, default_value_t = 64)]
    weak_cap: u64,
    #[arg(long, default_value_t = 32)]
    weak_fill: u64,
    #[arg(long, default_value_t = 48)]
    weak_depth: usize,
    /// `R_MAX`, the most times easier than the minimum a weak share may be.
    #[arg(long, default_value_t = 256)]
    weak_ratio_max: u64,
    /// INCLUDER_CUT as a fraction (0.01 is 1%).
    #[arg(long, default_value_t = 0.01)]
    includer_cut: f64,
    /// Payout level of every untracked miner, from the lowest to 7.
    #[arg(long, default_value_t = 5)]
    level: u32,
    /// Payout level of the tracked Bitaxe, from the lowest to 7.
    #[arg(long, default_value_t = 5)]
    bitaxe_level: u32,
    /// Payout level of the tracked 100 TH/s ASIC, from the lowest to 7.
    #[arg(long, default_value_t = 5)]
    asic_level: u32,
    #[arg(long, value_enum, default_value_t = CreditArg::Page)]
    weak_credit: CreditArg,
    /// The industrial miners' nodes give them the minimum (attack a).
    #[arg(long)]
    big_node_at_minimum: bool,
    /// Fraction of hashrate on nodes that never carry others' weak shares.
    #[arg(long, default_value_t = 0.0)]
    non_carrying: f64,
    /// Spam hashrate at the minimum as a multiple of the home hashrate.
    #[arg(long, default_value_t = 0.0)]
    spam: f64,
    /// Weak shares ask for an easier weak target (section 6 of the design
    /// page, a proposal), with the proposed constants and node default.
    #[arg(long)]
    asks: bool,
}

fn design(argument: DesignArg) -> Design {
    match argument {
        DesignArg::Mini => Design::Mini,
        DesignArg::OneChain => Design::OneChain,
        DesignArg::Band => Design::Band,
        DesignArg::VardiffCd => Design::Vardiff,
        DesignArg::BandWeak => Design::BandWeak,
        DesignArg::BandWeakLevels => Design::BandWeakLevels,
    }
}

fn mix(argument: MixArg) -> MinerMix {
    match argument {
        MixArg::Home => MinerMix::Home,
        MixArg::HalfIndustrial => MinerMix::HalfIndustrial,
        MixArg::MostlyIndustrial => MinerMix::MostlyIndustrial,
        MixArg::AlmostAllIndustrial => MinerMix::AlmostAllIndustrial,
        MixArg::Zipf => MinerMix::Zipf,
        MixArg::Medium => MinerMix::Medium,
        MixArg::MediumFewBig => MinerMix::MediumFewBig,
    }
}

fn scenario_from(arguments: &RunArgs, months: usize, seed: u64) -> Result<Scenario, String> {
    for level in [
        arguments.level,
        arguments.bitaxe_level,
        arguments.asic_level,
    ] {
        require_payout_level(level)?;
    }
    let base = base_scenario(months, seed);
    let mut scenario = Scenario {
        label: "run".to_string(),
        design: design(arguments.design),
        pool_fraction: arguments.pool_fraction,
        hashrate_multiple: arguments.hashrate_multiple,
        mix: mix(arguments.mix),
        period_seconds: arguments.period,
        stale_rate: arguments
            .stale_rate
            .unwrap_or_else(|| stale_rate_for_period(arguments.period)),
        band_cap: arguments.band_cap,
        weak_cap: arguments.weak_cap,
        weak_fill: arguments.weak_fill,
        weak_depth: arguments.weak_depth,
        weak_ratio_max: arguments.weak_ratio_max,
        includer_cut: arguments.includer_cut,
        default_level: arguments.level,
        weak_credit: match arguments.weak_credit {
            CreditArg::Page => WeakCredit::Page,
            CreditArg::WeakWork => WeakCredit::WeakWork,
        },
        big_node_at_minimum: arguments.big_node_at_minimum,
        non_carrying_fraction: arguments.non_carrying,
        spam_multiple: arguments.spam,
        weak_asks: arguments.asks.then(WeakAsks::default),
        ..base
    };
    scenario.tracked[0].level = arguments.bitaxe_level;
    scenario.tracked[1].level = arguments.asic_level;
    Ok(scenario)
}

/// One report as plain text.
fn summary(report: &Report) -> String {
    let mut lines = Vec::with_capacity(32);
    let scenario = &report.scenario;
    lines.push(format!(
        "design {}, pool {} of BCH (x{} hashrate), mix {}, period {} s, stale {}",
        scenario.design.name(),
        pool_percent(report.pool_fraction),
        scenario.hashrate_multiple,
        scenario.mix.name(),
        scenario.period_seconds,
        percent(scenario.stale_rate),
    ));
    report.chains.iter().for_each(|chain| {
        lines.push(format!(
            "chain {}: {} of BCH, ASERT difficulty {}, easiest share {}, window {} ({} shares{}), C settles at {}, R {}",
            chain.name,
            pool_percent(chain.hashrate_fraction),
            rounded(chain.asert_difficulty),
            rounded(chain.easiest_difficulty),
            hours(chain.window.span_seconds),
            rounded(chain.window.shares),
            if chain.window.capped { ", capped" } else { "" },
            optional(chain.settled_band_cap, rounded),
            optional(chain.weak.map(|outcome| outcome.mean_ratio), rounded),
        ));
    });
    lines.push(format!(
        "pool: {} shares/s, {} uncles/s, {} weak shares/s, {} KB/s",
        rounded(report.shares_per_second()),
        rounded(report.uncles_per_second()),
        rounded(report.weak_shares_per_second()),
        rounded(report.bytes_per_second() / 1_000.0),
    ));
    report.miners.iter().for_each(|miner| {
        lines.push(format!(
            "{}: fair {} sat/mo, expected {}, simulated {}; {} shares/mo, {} weak/mo; {} payments/mo, median {} sat, swing {}, empty months {}, fee loss {}",
            miner.name,
            satoshis(miner.fair_income_per_month),
            satoshis(miner.expected_income_per_month),
            satoshis(miner.income_per_month),
            rounded(miner.shares_per_month),
            rounded(miner.weak_shares_per_month),
            rounded(miner.payments_per_month),
            optional(miner.median_payment, satoshis),
            optional(miner.swing, two_decimals),
            percent(miner.months_without_payment),
            optional(miner.fee_loss, percent),
        ));
    });
    lines.join("\n")
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match &cli.command {
        Some(Command::Run(arguments)) => match scenario_from(arguments, cli.months, cli.seed) {
            Ok(scenario) => {
                println!("{}", summary(&scenario.run()));
                ExitCode::SUCCESS
            }
            Err(message) => {
                eprintln!("{message}");
                ExitCode::FAILURE
            }
        },
        None => {
            let tables = if cli.scenario == "all" {
                Ok(Table::ALL.to_vec())
            } else {
                Table::parse(&cli.scenario).map(|table| vec![table])
            };
            match tables {
                Ok(tables) => {
                    let base = base_scenario(cli.months, cli.seed);
                    tables
                        .iter()
                        .for_each(|table| println!("{}\n", table.render(&base)));
                    ExitCode::SUCCESS
                }
                Err(message) => {
                    eprintln!("{message}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}
