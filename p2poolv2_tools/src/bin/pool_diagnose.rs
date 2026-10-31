// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Diagnostics for a pool process under load.

use clap::Parser;
use p2poolv2_tools::diagnose::{DEFAULT_PROCESS, diagnose, find_process};
use std::process::ExitCode;

/// Diagnose a running pool process under load: system load and CPU, vmstat,
/// threads, context switches, I/O, file descriptors, memory, TCP
/// connections, top threads, and a five-second perf stat and syscall
/// summary when perf and strace are installed. Linux only.
#[derive(Parser)]
#[command(name = "pool_diagnose")]
struct Arguments {
    /// Process name (exact) or command-line pattern to diagnose.
    #[arg(long, default_value = DEFAULT_PROCESS)]
    process: String,
}

fn main() -> ExitCode {
    let arguments = Arguments::parse();
    match find_process(&arguments.process) {
        Some(pid) => {
            diagnose(pid);
            ExitCode::SUCCESS
        }
        None => {
            println!("ERROR: Cannot find {} process", arguments.process);
            ExitCode::FAILURE
        }
    }
}
