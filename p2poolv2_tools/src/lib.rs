// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Developer tooling for P2Poolv2.
//!
//! Everything here used to be a Python, JavaScript or shell script under
//! `load-tests/` and `debug_tools/`. Each binary in `src/bin/` is a thin
//! command line front end over a module of this library; `docs/tooling.md`
//! maps every old script to its replacement.

pub mod ansi;
pub mod cargo_build;
pub mod diagnose;
pub mod donate_qr;
pub mod env_vars;
pub mod error;
pub mod jmeter;
pub mod log_file;
pub mod log_time;
pub mod mock_bitcoind;
pub mod node_api;
pub mod process;
pub mod repo;
pub mod seeded;
pub mod share_latency;
pub mod share_lifecycle;
pub mod sim;
pub mod stats;
pub mod tcp_monitor;
pub mod terminal;

pub use error::{ToolError, ToolResult};
