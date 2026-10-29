// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Public-data SAFA command line. It never signs or broadcasts transactions.

use std::env;
use std::process::ExitCode;

use p2poolv2_safa::cli::{USAGE, read_json, run};

fn main() -> ExitCode {
    match execute() {
        Ok(output) => {
            println!("{output}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn execute() -> Result<String, String> {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    let command = arguments.first().ok_or(USAGE)?;
    let expected = match command.as_str() {
        "params" | "genesis" => 1,
        "leaf" | "work" | "unlock" | "claim" | "verify" => 2,
        _ => return Err(USAGE.into()),
    };
    if arguments.len() != expected + 1 {
        return Err(USAGE.into());
    }
    let inputs = arguments[1..]
        .iter()
        .map(read_json)
        .collect::<Result<Vec<_>, _>>()?;
    serde_json::to_string_pretty(&run(command, &inputs)?).map_err(|error| error.to_string())
}
