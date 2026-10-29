// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! TCP connection monitor for a mining pool's stratum port.

use clap::Parser;
use p2poolv2_tools::env_vars;
use p2poolv2_tools::tcp_monitor::{MonitorOptions, run};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

/// TCP connection monitor for a mining pool. Shows connection counts by
/// state for the pool port and the whole host, and kernel limits (file
/// descriptors, SYN backlog, orphans, TIME_WAIT buckets), and appends each
/// sample to a CSV log. Some /proc files need root. Press Ctrl+C to stop.
///
/// Defaults come from POOL_PORT, LOG_FILE, UPDATE_INTERVAL and
/// ALERT_THRESHOLD when set.
#[derive(Parser)]
#[command(name = "tcp_monitor")]
struct Arguments {
    /// Mining pool port to monitor (default 3333).
    #[arg(short, long)]
    port: Option<u16>,
    /// CSV log file (default ./tcp-monitor.log).
    #[arg(short, long = "log")]
    log_file: Option<PathBuf>,
    /// Seconds between updates (default 2).
    #[arg(short, long)]
    interval: Option<f64>,
    /// Alert threshold in percent (default 80).
    #[arg(short, long)]
    threshold: Option<i64>,
    /// Log only, no display.
    #[arg(short, long)]
    quiet: bool,
}

fn options(arguments: Arguments) -> p2poolv2_tools::ToolResult<MonitorOptions> {
    let interval = match arguments.interval {
        Some(interval) => interval,
        None => env_vars::parsed("UPDATE_INTERVAL", 2.0)?,
    };
    Ok(MonitorOptions {
        port: match arguments.port {
            Some(port) => port,
            None => env_vars::parsed("POOL_PORT", 3333)?,
        },
        log_file: arguments
            .log_file
            .unwrap_or_else(|| PathBuf::from(env_vars::string("LOG_FILE", "./tcp-monitor.log"))),
        interval: Duration::from_secs_f64(interval.max(0.0)),
        threshold: match arguments.threshold {
            Some(threshold) => threshold,
            None => env_vars::parsed("ALERT_THRESHOLD", 80)?,
        },
        quiet: arguments.quiet,
    })
}

#[tokio::main]
async fn main() -> ExitCode {
    let options = match options(Arguments::parse()) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("Error: {error}");
            return ExitCode::FAILURE;
        }
    };
    tokio::select! {
        result = run(&options) => match result {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{error}");
                ExitCode::FAILURE
            }
        },
        _ = tokio::signal::ctrl_c() => {
            println!();
            println!("Stopping TCP monitor...");
            ExitCode::SUCCESS
        }
    }
}
