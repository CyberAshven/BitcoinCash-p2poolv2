// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! SAFA command line; only the operator's claimer can broadcast claims.

use std::env;
use std::process::ExitCode;

use p2poolv2_safa::claimer::{Options, run_claimer};
use p2poolv2_safa::cli::{USAGE, read_json, run};

fn main() -> ExitCode {
    match execute() {
        Ok(output) => {
            if !output.is_empty() {
                println!("{output}");
            }
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
    if command == "claimer" {
        let options = Options::parse(&arguments[1..])?;
        tokio::runtime::Runtime::new()
            .map_err(|_| "could not start the claimer runtime")?
            .block_on(run_claimer(options))?;
        return Ok(String::new());
    }
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
