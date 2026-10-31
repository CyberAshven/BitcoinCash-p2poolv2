// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The BCHN regtest node the swarm mines against, over JSON-RPC.
//!
//! The swarm runs Bitcoin Cash, so the node must be Bitcoin Cash Node
//! (BCHN), the reference for BCH consensus and relay policy. Its daemon is
//! called `bitcoind` like Bitcoin Core's, which is why this module and the
//! settings keep that name.

use crate::env_vars;
use crate::error::{ToolError, ToolResult};
use bitcoindrpc::{BitcoindRpcClient, BitcoindRpcError};
use serde_json::Value;

/// Default `RPC_URL`.
pub const DEFAULT_RPC_URL: &str = "http://127.0.0.1:19443";

/// The wallet the nightly creates and the swarm takes payout addresses from.
pub const WALLET: &str = "p2pool";

/// What BCHN puts in its `getnetworkinfo` `subversion`, as in
/// `/Bitcoin Cash Node:29.2.0(EB32.0)/`.
const BCHN_USER_AGENT: &str = "Bitcoin Cash Node";

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

    /// The settings for wallet `name`'s endpoint, `<RPC_URL>/wallet/<name>`.
    ///
    /// Wallet calls go there so they reach that wallet however many the
    /// node has loaded. BCHN creates and loads a default wallet at startup,
    /// so once `p2pool` is loaded as well a wallet call to the bare URL
    /// fails with "Wallet file not specified".
    pub fn for_wallet(&self, name: &str) -> Self {
        Self {
            url: format!("{}/wallet/{name}", self.url.trim_end_matches('/')),
            user: self.user.clone(),
            password: self.password.clone(),
        }
    }
}

/// How a call went, keeping the node's own errors apart from failing to
/// reach it at all.
#[derive(Debug)]
pub enum CallOutcome {
    Result(Value),
    /// The node answered with an RPC error.
    RpcError(String),
}

/// A JSON-RPC client for the regtest node.
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

    /// Call `method`. An RPC error from the node (sent with HTTP 500 and a
    /// JSON body, or in a 200 body) is [`CallOutcome::RpcError`]; failing to
    /// reach the node, a refused login or an unreadable answer is an error.
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

    /// Whether the node answers and is on regtest.
    pub async fn is_regtest(&self) -> bool {
        matches!(
            self.call("getblockchaininfo", Vec::new()).await,
            Ok(CallOutcome::Result(info)) if info.get("chain").and_then(Value::as_str) == Some("regtest")
        )
    }

    /// An error unless the node is BCHN. A Bitcoin Core regtest node
    /// answers the same calls but mines Bitcoin, which the sim nodes would
    /// refuse only once they fetch a template.
    pub async fn require_bchn(&self) -> ToolResult<()> {
        let info = self.result("getnetworkinfo", Vec::new()).await?;
        let user_agent = info
            .get("subversion")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if is_bchn_user_agent(user_agent) {
            return Ok(());
        }
        Err(ToolError::message(format!(
            "ERROR: the regtest node is '{user_agent}', not Bitcoin Cash Node; the sim swarm mines Bitcoin Cash and needs BCHN."
        )))
    }
}

/// Whether a `getnetworkinfo` `subversion` is BCHN's.
fn is_bchn_user_agent(user_agent: &str) -> bool {
    user_agent.contains(BCHN_USER_AGENT)
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
