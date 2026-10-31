// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Environment variable knobs with shell `${NAME:-default}` semantics.
//!
//! The sim and load-test tools are configured through environment variables,
//! as the scripts they replace were. CI sets some of them to an empty string
//! to mean "not set" (a matrix entry with no value), so an empty value falls
//! back to the default here, exactly as `${NAME:-default}` does.

use crate::error::{ToolError, ToolResult};
use std::str::FromStr;

/// The value of `name`, or `None` when it is unset, empty or not UTF-8.
pub fn optional_string(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// The value of `name`, or `default` when it is unset or empty.
pub fn string(name: &str, default: &str) -> String {
    optional_string(name).unwrap_or_else(|| default.to_string())
}

/// `name` parsed as `T`, or `None` when it is unset or empty. A value that
/// does not parse is an error rather than a silent default.
pub fn optional<T: FromStr>(name: &str) -> ToolResult<Option<T>> {
    match optional_string(name) {
        None => Ok(None),
        Some(raw) => parse_value(name, &raw).map(Some),
    }
}

/// `name` parsed as `T`, or `default` when it is unset or empty.
pub fn parsed<T: FromStr>(name: &str, default: T) -> ToolResult<T> {
    Ok(optional(name)?.unwrap_or(default))
}

fn parse_value<T: FromStr>(name: &str, raw: &str) -> ToolResult<T> {
    raw.trim()
        .parse()
        .map_err(|_| ToolError::message(format!("{name}={raw:?} is not a valid value")))
}
