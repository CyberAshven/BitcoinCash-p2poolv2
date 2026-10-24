// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Writes the landing page's donation QR codes.

use clap::Parser;
use p2poolv2_tools::donate_qr::{website_dir, write_codes};
use std::path::PathBuf;
use std::process::ExitCode;

/// Draw the QR codes of the P2Pool donation address, plain and token-aware,
/// as the SVG files `website/index.html` shows. Run it after the address
/// changes; the workspace tests fail until the files match.
#[derive(Parser)]
#[command(name = "donate_qr")]
struct Arguments {
    /// Directory to write the SVG files to (default: the repository's
    /// website/).
    #[arg(short, long)]
    output: Option<PathBuf>,
}

fn main() -> ExitCode {
    let arguments = Arguments::parse();
    let directory = arguments.output.unwrap_or_else(website_dir);
    match write_codes(&directory) {
        Ok(written) => {
            for path in written {
                println!("{}", path.display());
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("donate_qr: {error}");
            ExitCode::FAILURE
        }
    }
}
