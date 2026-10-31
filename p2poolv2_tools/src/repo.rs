// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Where the repository is.
//!
//! The scripts these tools replace found the repository from their own
//! location. The tools are built from the repository with cargo, so the
//! crate's manifest directory at build time gives the same answer.

use std::path::PathBuf;

/// The repository root: the parent of this crate's directory.
pub fn root() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest.parent().map(PathBuf::from).unwrap_or(manifest)
}

/// `load-tests/jmeter-testing`, where the JMeter plan and configs live.
pub fn jmeter_dir() -> PathBuf {
    root().join("load-tests").join("jmeter-testing")
}
