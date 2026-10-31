// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Terminal width for side-by-side layouts.

use std::io::IsTerminal;
use std::process::{Command, Stdio};

/// Width used when stdout is not a terminal, as the Python tools did.
pub const DEFAULT_WIDTH: usize = 160;

/// Columns to lay out for: `requested` when given, else `COLUMNS`, else the
/// terminal's width when stdout is a terminal (asked of `stty`), else
/// [`DEFAULT_WIDTH`].
pub fn width(requested: Option<usize>) -> usize {
    if let Some(width) = requested {
        return width;
    }
    if let Some(columns) =
        crate::env_vars::optional_string("COLUMNS").and_then(|columns| columns.trim().parse().ok())
    {
        return columns;
    }
    if std::io::stdout().is_terminal()
        && let Some(columns) = stty_columns()
    {
        return columns;
    }
    DEFAULT_WIDTH
}

/// Columns of the controlling terminal from `stty size`, which prints
/// `rows columns` for the terminal on its stdin.
fn stty_columns() -> Option<usize> {
    let output = Command::new("stty")
        .arg("size")
        .stdin(Stdio::inherit())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}
