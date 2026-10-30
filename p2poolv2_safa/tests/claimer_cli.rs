// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Real-process claimer argument and input failures, all before RPC setup.

use std::fs;
use std::process::Command;

use serde_json::json;
use tempfile::tempdir;

#[test]
fn claimer_without_arguments_reports_usage_on_stderr() {
    let output = Command::new(env!("CARGO_BIN_EXE_safa"))
        .arg("claimer")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap().trim_end(),
        "usage: safa claimer claimer.json [--once] [--dry-run]"
    );
}

#[test]
fn claimer_missing_config_file_fails_cleanly_without_echoing_its_path() {
    let directory = tempdir().unwrap();
    let path = directory
        .path()
        .join("secret-do-not-log missing config.json");
    let output = Command::new(env!("CARGO_BIN_EXE_safa"))
        .arg("claimer")
        .arg(path)
        .arg("--once")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).unwrap();
    assert_eq!(
        error.trim_end(),
        "cannot read claimer configuration as a regular JSON file"
    );
    assert!(!error.contains("secret-do-not-log"));
}

#[test]
fn claimer_unknown_flag_reports_usage_without_echoing_argument_contents() {
    let output = Command::new(env!("CARGO_BIN_EXE_safa"))
        .args(["claimer", "claimer.json", "--secret-do-not-log"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).unwrap();
    assert_eq!(
        error.trim_end(),
        "usage: safa claimer claimer.json [--once] [--dry-run]"
    );
    assert!(!error.contains("secret-do-not-log"));
}

#[test]
fn claimer_duplicate_once_flag_reports_usage() {
    let output = Command::new(env!("CARGO_BIN_EXE_safa"))
        .args(["claimer", "claimer.json", "--once", "--once"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap().trim_end(),
        "usage: safa claimer claimer.json [--once] [--dry-run]"
    );
}

#[test]
fn claimer_duplicate_dry_run_flag_reports_usage() {
    let output = Command::new(env!("CARGO_BIN_EXE_safa"))
        .args(["claimer", "claimer.json", "--dry-run", "--dry-run"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap().trim_end(),
        "usage: safa claimer claimer.json [--once] [--dry-run]"
    );
}

#[test]
fn claimer_malformed_config_never_echoes_supplied_secret_values() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("claimer.json");
    fs::write(
        &path,
        json!({
            "deployment":"deployment.json", "vault":{"txid":"11".repeat(32),"vout":3},
            "token_file":"tokens.json", "wins_log":"wins.jsonl", "payout_scripts":["51"],
            "p2pool_config":"config.toml", "journal":"journal.json", "fee_rate":"secret-do-not-log"
        })
        .to_string(),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_safa"))
        .arg("claimer")
        .arg(path)
        .args(["--once", "--dry-run"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).unwrap();
    assert_eq!(
        error.trim_end(),
        "invalid claimer configuration fields or types"
    );
    assert!(!error.contains("secret-do-not-log"));
    assert!(!directory.path().join("journal.json").exists());
    assert!(!directory.path().join("journal.json.lock").exists());
}

#[test]
fn claimer_oversized_config_is_rejected_before_json_or_rpc_processing() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("claimer.json");
    let mut bytes = vec![b' '; 1024 * 1024 + 1];
    bytes[..17].copy_from_slice(b"secret-do-not-log");
    fs::write(&path, bytes).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_safa"))
        .arg("claimer")
        .arg(path)
        .arg("--once")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).unwrap();
    assert_eq!(error.trim_end(), "input JSON exceeds 1 MiB");
    assert!(!error.contains("secret-do-not-log"));
    assert!(!directory.path().join("journal.json").exists());
}
