// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Share propagation and confirmation latencies across P2Poolv2 node logs.

use clap::Parser;
use p2poolv2_tools::ansi::{DIM, RESET};
use p2poolv2_tools::share_latency::{
    NodeEvents, Nodes, confirmation_latencies, propagation_latencies, resolve_files,
    write_confirmation_report, write_overview, write_propagation_report,
};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::process::ExitCode;

/// Compute share propagation and confirmation latencies across P2Poolv2
/// nodes.
///
/// Accepts glob patterns (quoted to avoid shell expansion, `**` included) or
/// explicit file paths. Pipe through `less -R` for colour.
#[derive(Parser)]
#[command(name = "share_latency")]
struct Arguments {
    /// Include uncle shares in latency statistics (default: confirmed chain
    /// only).
    #[arg(long)]
    include_uncles: bool,
    /// Log files or glob patterns to parse.
    #[arg(required = true)]
    files: Vec<String>,
}

fn main() -> ExitCode {
    let arguments = Arguments::parse();
    let paths = resolve_files(&arguments.files);
    if paths.is_empty() {
        eprintln!("Error: no log files found");
        return ExitCode::FAILURE;
    }

    let stdout = std::io::stdout();
    let mut out = BufWriter::new(stdout.lock());
    let result = run(&mut out, &paths, arguments.include_uncles)
        .and_then(|()| out.flush().map_err(ToolOutcome::from));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(ToolOutcome::Output(error)) if error.kind() == std::io::ErrorKind::BrokenPipe => {
            ExitCode::SUCCESS
        }
        Err(ToolOutcome::Output(error)) => {
            eprintln!("cannot write output: {error}");
            ExitCode::FAILURE
        }
        Err(ToolOutcome::Failed(message)) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

/// Why the report stopped.
enum ToolOutcome {
    Output(std::io::Error),
    Failed(String),
}

impl From<std::io::Error> for ToolOutcome {
    fn from(error: std::io::Error) -> Self {
        ToolOutcome::Output(error)
    }
}

fn run(out: &mut impl Write, paths: &[PathBuf], include_uncles: bool) -> Result<(), ToolOutcome> {
    writeln!(out, "{DIM}Parsing {} log file(s)...{RESET}", paths.len())?;
    for path in paths {
        writeln!(out, "  {DIM}{}{RESET}", path.display())?;
    }
    out.flush()?;

    let mut nodes = Nodes::default();
    for path in paths {
        let label = path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let events =
            NodeEvents::from_log(path).map_err(|error| ToolOutcome::Failed(error.to_string()))?;
        nodes.insert(label, events);
    }

    write_overview(out, &nodes)?;
    let propagation = propagation_latencies(&nodes, include_uncles);
    write_propagation_report(out, &propagation, include_uncles)?;
    let confirmation = confirmation_latencies(&nodes, include_uncles);
    write_confirmation_report(out, &confirmation, include_uncles)?;
    Ok(())
}
