// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Command line front end of the share difficulty simulator.

use clap::{Parser, Subcommand};
use p2poolv2_daa::chart::write_chart;
use p2poolv2_daa::difficulty::ideal_share_interval;
use p2poolv2_daa::report::{write_bch_report, write_upstream_report};
use p2poolv2_daa::rules::{BCH_RULES, CATALOG, Rule, UPSTREAM_RULES};
use p2poolv2_daa::scenario::{Scenario, bch_suite, upstream_suite};
use p2poolv2_daa::simulate::Settings;
use p2poolv2_daa::suite::{ScenarioResults, evaluate};
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Simulate share difficulty adjustment on the Bitcoin Cash share chain,
/// with the share chain's own ASERT code beside the algorithms upstream's
/// analysis compared. Every run is seeded, so the same arguments print the
/// same tables. See docs/difficulty_adjustment/bch-daa-analysis.md.
#[derive(Parser)]
#[command(name = "daa_sim")]
struct Arguments {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Args)]
struct RunOptions {
    /// Runs per rule and scenario. Run `n` is seeded with `seed + 1000 n`.
    #[arg(long, default_value_t = 20)]
    runs: usize,
    /// Seed of the first run (upstream used 42).
    #[arg(long, default_value_t = 42)]
    seed: u64,
    /// Rule to run (repeatable; `list` names them). Default: the suite's.
    #[arg(long = "rule")]
    rules: Vec<String>,
    /// Scenario to run (repeatable; `list` names them). Default: all.
    #[arg(long = "scenario")]
    scenarios: Vec<String>,
    /// Write the Markdown tables here instead of to standard output.
    #[arg(long)]
    output: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Command {
    /// Upstream's six scenarios (5,000 shares, the first 100 left out) and
    /// its composite and attack rankings.
    Upstream {
        #[command(flatten)]
        options: RunOptions,
    },
    /// The Bitcoin Cash scenarios: BTC/BCH switching, one BitAxe growing to
    /// exahashes, and quiet periods.
    Bch {
        #[command(flatten)]
        options: RunOptions,
        /// Write one SVG chart of share difficulty per scenario to this
        /// directory.
        #[arg(long)]
        charts: Option<PathBuf>,
    },
    /// List the rules and scenarios.
    List,
}

/// The rules named by `ids`, or `default` when there are none.
fn select_rules(ids: &[String], default: &[Rule]) -> Result<Vec<Rule>, String> {
    if ids.is_empty() {
        return Ok(default.to_vec());
    }
    ids.iter()
        .map(|id| Rule::from_id(id).ok_or_else(|| format!("unknown rule {id}")))
        .collect()
}

/// The scenarios of `suite` named by `ids`, or all of them.
fn select_scenarios(ids: &[String], suite: Vec<Scenario>) -> Result<Vec<Scenario>, String> {
    if ids.is_empty() {
        return Ok(suite);
    }
    ids.iter()
        .map(|id| {
            suite
                .iter()
                .find(|scenario| scenario.id == id)
                .cloned()
                .ok_or_else(|| format!("unknown scenario {id}"))
        })
        .collect()
}

/// Run every selected rule over every selected scenario, reporting progress
/// on standard error.
fn run_suite(
    options: &RunOptions,
    suite: Vec<Scenario>,
    default_rules: &[Rule],
) -> Result<Vec<ScenarioResults>, String> {
    let rules = select_rules(&options.rules, default_rules)?;
    let scenarios = select_scenarios(&options.scenarios, suite)?;
    let settings = Settings::default();
    Ok(scenarios
        .iter()
        .map(|scenario| {
            eprintln!(
                "{}: {} rules x {} runs",
                scenario.id,
                rules.len(),
                options.runs
            );
            evaluate(scenario, &rules, options.runs, options.seed, &settings)
        })
        .collect())
}

/// Standard output, or the file at `path`.
fn open_output(path: Option<&Path>) -> Result<Box<dyn Write>, String> {
    match path {
        Some(path) => File::create(path)
            .map(|file| Box::new(BufWriter::new(file)) as Box<dyn Write>)
            .map_err(|error| format!("cannot create {}: {error}", path.display())),
        None => Ok(Box::new(BufWriter::new(io::stdout()))),
    }
}

fn write_header(out: &mut dyn Write, options: &RunOptions) -> io::Result<()> {
    writeln!(
        out,
        "Runs per rule and scenario: {}; seeds {} + 1000 n.\n",
        options.runs, options.seed
    )
}

fn upstream(options: &RunOptions) -> Result<(), String> {
    let results = run_suite(options, upstream_suite(), &UPSTREAM_RULES)?;
    let mut out = open_output(options.output.as_deref())?;
    write_header(&mut out, options)
        .and_then(|()| write_upstream_report(&mut out, &results, ideal_share_interval() as f64))
        .and_then(|()| out.flush())
        .map_err(|error| format!("cannot write the report: {error}"))
}

fn bch(options: &RunOptions, charts: Option<&Path>) -> Result<(), String> {
    let results = run_suite(options, bch_suite(), &BCH_RULES)?;
    let mut out = open_output(options.output.as_deref())?;
    write_header(&mut out, options)
        .and_then(|()| write_bch_report(&mut out, &results))
        .and_then(|()| out.flush())
        .map_err(|error| format!("cannot write the report: {error}"))?;
    if let Some(directory) = charts {
        fs::create_dir_all(directory)
            .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
        for scenario in &results {
            write_chart(
                scenario,
                &directory.join(format!("{}.svg", scenario.scenario.id)),
            )?;
        }
    }
    Ok(())
}

fn list() {
    println!("Rules (also asert-parent-h<seconds> for any half-life):");
    for rule in CATALOG {
        println!("  {:<20} {}", rule.id(), rule.label());
    }
    println!("\nUpstream scenarios:");
    for scenario in upstream_suite() {
        println!("  {:<20} {}", scenario.id, scenario.title);
    }
    println!("\nBitcoin Cash scenarios:");
    for scenario in bch_suite() {
        println!("  {:<20} {}", scenario.id, scenario.title);
    }
}

fn main() -> ExitCode {
    let arguments = Arguments::parse();
    let outcome = match &arguments.command {
        Command::Upstream { options } => upstream(options),
        Command::Bch { options, charts } => bch(options, charts.as_deref()),
        Command::List => {
            list();
            Ok(())
        }
    };
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("daa_sim: {message}");
            ExitCode::FAILURE
        }
    }
}
