// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

#![allow(dead_code)]

use std::fs;
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::tempdir;

/// Run the CLI against temporary files containing public JSON inputs.
pub fn run_cli_json(command: &str, inputs: &[Value]) -> Output {
    let directory = tempdir().unwrap();
    let mut process = Command::new(env!("CARGO_BIN_EXE_safa"));
    process.arg(command);
    for (index, input) in inputs.iter().enumerate() {
        let path = directory.path().join(format!("input-{index}.json"));
        fs::write(&path, serde_json::to_vec(input).unwrap()).unwrap();
        process.arg(path);
    }
    process.output().unwrap()
}

/// Read a checked public-data fixture relative to the crate.
pub fn fixture(name: &str) -> Value {
    let path = format!("{}/test_data/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

/// Decode successful JSON output and retain stderr on assertion failure.
pub fn successful_json(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

/// Compare one reference CLI case byte for byte, including the final newline.
pub fn assert_cli_case(name: &str) {
    let cases = fixture("parity/cases.json");
    let case = cases
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == name)
        .unwrap();
    let output = run_cli_json(
        case["command"].as_str().unwrap(),
        case["inputs"].as_array().unwrap(),
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        case["stdout"].as_str().unwrap()
    );
}
