// SPDX-License-Identifier: AGPL-3.0-or-later

//! Regenerate the checked, annotated assembly and byte-exact program artifacts.

use std::fs;
use std::path::Path;

use p2poolv2_photon_payout::covenant::{generated_source, program, program_hash};

fn main() -> Result<(), String> {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR"));
    fs::write(directory.join("payout.cash"), generated_source())
        .map_err(|error| error.to_string())?;
    fs::write(
        directory.join("program.hex"),
        format!("{}\n", hex::encode(program()?)),
    )
    .map_err(|error| error.to_string())?;
    fs::write(
        directory.join("program.sha256"),
        format!("{}\n", hex::encode(program_hash()?)),
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}
