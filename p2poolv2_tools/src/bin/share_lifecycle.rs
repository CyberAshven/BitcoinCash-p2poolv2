// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Show the lifecycle of each share in P2Poolv2 node logs.

use clap::Parser;
use p2poolv2_tools::share_lifecycle::{
    Event, parse_log, summary, write_sequential, write_two_columns,
};
use p2poolv2_tools::terminal;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::process::ExitCode;

/// Parse P2Poolv2 logs to show the lifecycle of each share: received
/// headers, block fetch, validation, candidate chain, confirmed chain.
/// Locally mined shares are told apart from shares received from peers.
///
/// Pass one file to see its lifecycle events, or two files to compare nodes
/// in two columns. Pipe through `less -R` for colour.
#[derive(Parser)]
#[command(name = "share_lifecycle")]
struct Arguments {
    /// Log files to parse.
    #[arg(required = true)]
    files: Vec<PathBuf>,
    /// Total width of the two-column layout. Defaults to COLUMNS, then the
    /// terminal width, then 160.
    #[arg(long)]
    width: Option<usize>,
}

fn main() -> ExitCode {
    let arguments = Arguments::parse();
    if let Some(missing) = arguments.files.iter().find(|path| !path.exists()) {
        eprintln!("File not found: {}", missing.display());
        return ExitCode::FAILURE;
    }

    let mut logs = Vec::with_capacity(arguments.files.len());
    for path in &arguments.files {
        let label = path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        match parse_log(path) {
            Ok(events) => logs.push((label, events)),
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::FAILURE;
            }
        }
    }

    let stdout = std::io::stdout();
    let mut out = BufWriter::new(stdout.lock());
    // A reader that quits early (`less`, `head`) closes the pipe; that is
    // not a failure of this tool.
    match write_report(&mut out, &logs, arguments.width).and_then(|()| out.flush()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("cannot write output: {error}");
            ExitCode::FAILURE
        }
    }
}

fn write_report(
    out: &mut impl Write,
    logs: &[(String, Vec<Event>)],
    width: Option<usize>,
) -> std::io::Result<()> {
    for (label, events) in logs {
        write!(out, "{}", summary(events, label))?;
    }
    if let [(left_label, left), (right_label, right)] = logs {
        write_two_columns(
            out,
            left,
            right,
            (left_label, right_label),
            terminal::width(width),
        )
    } else {
        write_sequential(out, logs)
    }
}
