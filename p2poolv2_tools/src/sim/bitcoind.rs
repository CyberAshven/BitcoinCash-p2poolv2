// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The regtest bitcoind the swarm mines against, over JSON-RPC.

use crate::env_vars;
use crate::error::{ToolError, ToolResult};
use bitcoindrpc::{BitcoindRpcClient, BitcoindRpcError};
use serde_json::Value;

/// Default `RPC_URL`.
pub const DEFAULT_RPC_URL: &str = "http://127.0.0.1:19443";

/// Connection settings: `RPC_URL`, `RPC_USER` and `RPC_PASS`.
#[derive(Debug, Clone)]
pub struct RpcSettings {
    pub url: String,
    pub user: String,
    pub password: String,
}

impl RpcSettings {
    pub fn from_env() -> Self {
        Self {
            url: env_vars::string("RPC_URL", DEFAULT_RPC_URL),
            user: env_vars::string("RPC_USER", "p2pool"),
            password: env_vars::string("RPC_PASS", "p2pool"),
        }
    }
}

/// How a call went, keeping bitcoind's own errors apart from failing to
/// reach it at all.
#[derive(Debug)]
pub enum CallOutcome {
    Result(Value),
    /// bitcoind answered with an RPC error.
    RpcError(String),
}

/// A JSON-RPC client for the regtest bitcoind.
#[derive(Debug, Clone)]
pub struct Bitcoind {
    client: BitcoindRpcClient,
}

impl Bitcoind {
    pub fn new(settings: &RpcSettings) -> ToolResult<Self> {
        let client = BitcoindRpcClient::new(&settings.url, &settings.user, &settings.password)
            .map_err(|source| ToolError::Rpc {
                method: "client setup".to_string(),
                source,
            })?;
        Ok(Self { client })
    }

    /// Call `method`. An RPC error from bitcoind (sent with HTTP 500 and a
    /// JSON body, or in a 200 body) is [`CallOutcome::RpcError`]; failing to
    /// reach bitcoind, a refused login or an unreadable answer is an error.
    pub async fn call(&self, method: &str, params: Vec<Value>) -> ToolResult<CallOutcome> {
        match self.client.request::<Value>(method, params).await {
            Ok(result) => Ok(CallOutcome::Result(result)),
            Err(BitcoindRpcError::RpcError { code, message }) => {
                Ok(CallOutcome::RpcError(format!("{code}: {message}")))
            }
            Err(BitcoindRpcError::HttpError {
                status_code,
                message,
            }) => match rpc_error_in_body(&message) {
                Some(error) => Ok(CallOutcome::RpcError(error)),
                None => Err(ToolError::message(format!(
                    "bitcoind RPC {method} failed with HTTP {status_code}: {message}"
                ))),
            },
            Err(source) => Err(ToolError::Rpc {
                method: method.to_string(),
                source,
            }),
        }
    }

    /// Call `method` and return its result, treating an RPC error as an
    /// error too.
    pub async fn result(&self, method: &str, params: Vec<Value>) -> ToolResult<Value> {
        match self.call(method, params).await? {
            CallOutcome::Result(value) => Ok(value),
            CallOutcome::RpcError(error) => Err(ToolError::message(format!(
                "bitcoind RPC {method} failed: {error}"
            ))),
        }
    }

    /// Whether bitcoind answers and is on regtest.
    pub async fn is_regtest(&self) -> bool {
        matches!(
            self.call("getblockchaininfo", Vec::new()).await,
            Ok(CallOutcome::Result(info)) if info.get("chain").and_then(Value::as_str) == Some("regtest")
        )
    }
}

/// The error in a JSON-RPC response body, when the body is one with a
/// non-null error.
fn rpc_error_in_body(body: &str) -> Option<String> {
    let response: Value = serde_json::from_str(body).ok()?;
    let error = response.get("error").filter(|error| !error.is_null())?;
    Some(error.to_string())
}

#[cfg(test)]
mod tests;
