// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Building workspace binaries and finding where cargo put them.
//!
//! The scripts these tools replace assumed `target/<profile>/<binary>`, which
//! is wrong whenever `CARGO_TARGET_DIR` or a cargo config moves the target
//! directory. Cargo's JSON messages name each built executable, so the tools
//! read the path from there instead.

use crate::error::{ToolError, ToolResult};
use serde::Deserialize;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// One `cargo build` invocation.
#[derive(Debug, Clone, Default)]
pub struct BuildRequest {
    /// `-p` packages; empty means `--workspace`.
    pub packages: Vec<String>,
    /// `--bin` targets; empty builds every target of the packages.
    pub binaries: Vec<String>,
    /// `--features`.
    pub features: Vec<String>,
    pub release: bool,
    /// Extra environment for cargo, such as `RUSTFLAGS`.
    pub environment: Vec<(String, String)>,
}

#[derive(Deserialize)]
struct CargoMessage {
    reason: String,
    #[serde(default)]
    target: Option<CargoTarget>,
    #[serde(default)]
    executable: Option<PathBuf>,
}

#[derive(Deserialize)]
struct CargoTarget {
    name: String,
    #[serde(default)]
    kind: Vec<String>,
}

/// Run `cargo build` in `repo_root` and return the path of every binary it
/// built, by target name. Build output and diagnostics go to stderr as
/// usual.
pub fn build(repo_root: &Path, request: &BuildRequest) -> ToolResult<HashMap<String, PathBuf>> {
    let mut command = Command::new("cargo");
    command.current_dir(repo_root).arg("build");
    if request.packages.is_empty() {
        command.arg("--workspace");
    }
    for package in &request.packages {
        command.arg("-p").arg(package);
    }
    for binary in &request.binaries {
        command.arg("--bin").arg(binary);
    }
    if !request.features.is_empty() {
        command.arg("--features").arg(request.features.join(","));
    }
    if request.release {
        command.arg("--release");
    }
    command
        .arg("--message-format=json-render-diagnostics")
        .envs(request.environment.iter().map(|(name, value)| (name, value)))
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());

    let mut child = command
        .spawn()
        .map_err(|error| ToolError::message(format!("cannot run cargo: {error}")))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ToolError::message("cargo stdout was not captured"))?;
    let mut executables = HashMap::with_capacity(request.binaries.len().max(4));
    for line in BufReader::new(stdout).lines() {
        let line = line.map_err(|source| ToolError::Io {
            context: "cannot read cargo output".to_string(),
            source,
        })?;
        if let Some((name, path)) = executable_from_message(&line) {
            executables.insert(name, path);
        }
    }
    let status = child.wait().map_err(|source| ToolError::Io {
        context: "cannot wait for cargo".to_string(),
        source,
    })?;
    if !status.success() {
        return Err(ToolError::message(format!("cargo build failed ({status})")));
    }
    Ok(executables)
}

/// The binary target name and executable path a cargo JSON message reports,
/// when it reports one.
fn executable_from_message(line: &str) -> Option<(String, PathBuf)> {
    let message: CargoMessage = serde_json::from_str(line).ok()?;
    if message.reason != "compiler-artifact" {
        return None;
    }
    let target = message.target?;
    if !target.kind.iter().any(|kind| kind == "bin") {
        return None;
    }
    Some((target.name, message.executable?))
}

/// Path of `binary` from a [`build`] result, or an error naming it.
pub fn executable(built: &HashMap<String, PathBuf>, binary: &str) -> ToolResult<PathBuf> {
    built
        .get(binary)
        .cloned()
        .ok_or_else(|| ToolError::message(format!("cargo build did not produce {binary}")))
}

/// Where cargo puts build output for this repository: `CARGO_TARGET_DIR`
/// when set, else what `cargo metadata` reports, else `target/`.
pub fn target_directory(repo_root: &Path) -> PathBuf {
    if let Some(directory) = crate::env_vars::optional_string("CARGO_TARGET_DIR") {
        return PathBuf::from(directory);
    }
    let output = Command::new("cargo")
        .current_dir(repo_root)
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .stderr(Stdio::null())
        .output();
    let reported = output
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| serde_json::from_slice::<serde_json::Value>(&output.stdout).ok())
        .and_then(|metadata| {
            metadata
                .get("target_directory")
                .and_then(|directory| directory.as_str())
                .map(PathBuf::from)
        });
    reported.unwrap_or_else(|| repo_root.join("target"))
}

/// Path of `binary` built with `profile` (`release` or `debug`) under the
/// target directory, with the platform's executable suffix.
pub fn profile_binary(repo_root: &Path, profile: &str, binary: &str) -> PathBuf {
    let profile_directory = if profile == "release" { "release" } else { "debug" };
    target_directory(repo_root)
        .join(profile_directory)
        .join(format!("{binary}{}", std::env::consts::EXE_SUFFIX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executable_from_message_reads_a_binary_artifact() {
        let line = r#"{"reason":"compiler-artifact","target":{"name":"verify_chain","kind":["bin"]},"executable":"/t/release/verify_chain"}"#;
        assert_eq!(
            executable_from_message(line),
            Some((
                "verify_chain".to_string(),
                PathBuf::from("/t/release/verify_chain")
            ))
        );
    }

    #[test]
    fn executable_from_message_skips_libraries() {
        let line = r#"{"reason":"compiler-artifact","target":{"name":"p2poolv2_lib","kind":["lib"]},"executable":null}"#;
        assert_eq!(executable_from_message(line), None);
    }

    #[test]
    fn executable_from_message_skips_other_messages() {
        let line = r#"{"reason":"build-finished","success":true}"#;
        assert_eq!(executable_from_message(line), None);
    }

    #[test]
    fn executable_from_message_skips_non_json() {
        assert_eq!(executable_from_message("Compiling foo"), None);
    }
}
