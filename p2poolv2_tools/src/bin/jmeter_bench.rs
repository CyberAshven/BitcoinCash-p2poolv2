// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! JMeter stratum load-test drivers.

use clap::{Parser, Subcommand};
use p2poolv2_tools::jmeter::{
    BenchmarkOptions, RemoteOptions, benchmark, flamegraph, gui, remote, summarize_jtl,
};
use std::path::PathBuf;
use std::process::ExitCode;

/// Drive the JMeter stratum load test (load-tests/jmeter-testing/stratum.jmx)
/// against P2Poolv2 and, optionally, CKPool. Needs jmeter on PATH.
#[derive(Parser)]
#[command(name = "jmeter_bench")]
struct Arguments {
    #[command(subcommand)]
    command: BenchCommand,
}

#[derive(Subcommand)]
enum BenchCommand {
    /// A/B benchmark: start the mock bitcoind, then build and load-test the
    /// portable and the native (target-cpu=native) P2Poolv2 builds, and
    /// CKPool if given, and print latency percentiles per sampler. Results
    /// go to load-tests/jmeter-testing/results/.
    Benchmark {
        /// P2Poolv2 config file.
        #[arg(long, default_value = "./config-load-test.toml")]
        config: PathBuf,
        /// Skip the default (portable) build test.
        #[arg(long)]
        skip_default: bool,
        /// Skip the native (CPU-optimized) build test.
        #[arg(long)]
        skip_native: bool,
        /// Path to the ckpool binary (enables the CKPool benchmark).
        #[arg(long)]
        ckpool: Option<PathBuf>,
    },
    /// Profile P2Poolv2 under load with cargo flamegraph (needs perf and
    /// cargo-flamegraph); writes flamegraph_loadtest.svg at the repository
    /// root.
    Flamegraph,
    /// Generate load against a stratum server already running on another
    /// host.
    Remote {
        /// Stratum server IP or hostname.
        #[arg(long)]
        host: Option<String>,
        /// Stratum server port.
        #[arg(long, default_value_t = 3333)]
        port: u16,
        /// Number of simulated miners.
        #[arg(long, default_value_t = 5000)]
        threads: u32,
        /// Ramp-up time in seconds.
        #[arg(long, default_value_t = 60)]
        ramp: u32,
        /// Test duration in seconds.
        #[arg(long, default_value_t = 300)]
        duration: u32,
        /// Delay between submit calls per thread, in milliseconds.
        #[arg(long, default_value_t = 3000)]
        delay: u32,
    },
    /// Open the JMeter GUI, scaled for high-DPI displays, with TCP protocol
    /// logging at DEBUG.
    Gui,
    /// Print the per-sampler summary of a JMeter results (.jtl) file.
    Summarize {
        /// Results file.
        results: PathBuf,
        /// Heading for the summary.
        #[arg(long, default_value = "Results")]
        label: String,
    },
}

fn report(result: p2poolv2_tools::ToolResult<()>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    match Arguments::parse().command {
        BenchCommand::Benchmark {
            config,
            skip_default,
            skip_native,
            ckpool,
        } => report(
            benchmark(&BenchmarkOptions {
                config,
                skip_default,
                skip_native,
                ckpool,
            })
            .await,
        ),
        BenchCommand::Flamegraph => report(flamegraph().await),
        BenchCommand::Remote {
            host,
            port,
            threads,
            ramp,
            duration,
            delay,
        } => {
            let Some(host) = host else {
                eprintln!("ERROR: --host is required");
                return ExitCode::FAILURE;
            };
            report(remote(&RemoteOptions {
                host,
                port,
                threads,
                ramp,
                duration,
                delay,
            }))
        }
        BenchCommand::Gui => match gui() {
            Ok(true) => ExitCode::SUCCESS,
            Ok(false) => ExitCode::FAILURE,
            Err(error) => {
                eprintln!("{error}");
                ExitCode::FAILURE
            }
        },
        BenchCommand::Summarize { results, label } => {
            print!("{}", summarize_jtl(&results, &label));
            ExitCode::SUCCESS
        }
    }
}
