// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! ANSI colour codes, and stripping them back out of text.

use regex::Regex;
use std::sync::LazyLock;

pub const RESET: &str = "\x1b[0m";
pub const BOLD: &str = "\x1b[1m";
pub const DIM: &str = "\x1b[2m";
pub const RED: &str = "\x1b[31m";
pub const GREEN: &str = "\x1b[32m";
pub const YELLOW: &str = "\x1b[33m";
pub const BLUE: &str = "\x1b[34m";
pub const MAGENTA: &str = "\x1b[35m";
pub const CYAN: &str = "\x1b[36m";
pub const WHITE: &str = "\x1b[37m";

static ESCAPE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("\x1b\\[[0-9;]*m").expect("valid regex"));

/// `text` with every SGR colour sequence removed.
pub fn strip(text: &str) -> std::borrow::Cow<'_, str> {
    ESCAPE.replace_all(text, "")
}

/// Number of characters a terminal shows for `text`, ignoring colour codes.
pub fn visible_length(text: &str) -> usize {
    strip(text).chars().count()
}

/// `text` padded with spaces to `width` visible characters. Longer text is
/// returned unchanged.
pub fn pad_to_width(text: &str, width: usize) -> String {
    let visible = visible_length(text);
    let mut padded = String::with_capacity(text.len() + width.saturating_sub(visible));
    padded.push_str(text);
    padded.extend(std::iter::repeat_n(' ', width.saturating_sub(visible)));
    padded
}

#[cfg(test)]
mod tests;
