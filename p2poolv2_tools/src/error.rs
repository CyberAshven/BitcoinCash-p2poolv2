// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The one error type the tools share.

use bitcoindrpc::BitcoindRpcError;
use std::path::Path;
use thiserror::Error;

/// Anything that stops a tool. Each tool decides how an error maps to its
/// exit code, so this carries only the message.
#[derive(Debug, Error)]
pub enum ToolError {
    #[error("{0}")]
    Message(String),
    #[error("{context}: {source}")]
    Io {
        context: String,
        source: std::io::Error,
    },
    #[error("HTTP request to {url} failed: {source}")]
    Http { url: String, source: reqwest::Error },
    #[error("invalid JSON from {origin}: {source}")]
    Json {
        origin: String,
        source: serde_json::Error,
    },
    #[error("bitcoind RPC {method} failed: {source}")]
    Rpc {
        method: String,
        source: BitcoindRpcError,
    },
}

pub type ToolResult<T> = Result<T, ToolError>;

impl ToolError {
    /// An error carrying only a message.
    pub fn message(text: impl Into<String>) -> Self {
        ToolError::Message(text.into())
    }

    /// An I/O error on `path`, naming what was being done.
    pub fn io(action: &str, path: &Path, source: std::io::Error) -> Self {
        ToolError::Io {
            context: format!("cannot {action} {}", path.display()),
            source,
        }
    }
}
