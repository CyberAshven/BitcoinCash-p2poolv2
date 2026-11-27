// SPDX-FileCopyrightText: 2024 PDM Authors
// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! BCHN's JSON-RPC, ported from PDM's Bitcoin Core client.
//!
//! The node's own `bitcoindrpc` client is not reused: it has no request
//! timeout and reads whole bodies, and a status screen polling a node that
//! hangs must neither block nor grow. The calls are the same JSON-RPC 1.0
//! with HTTP Basic auth.

use crate::client::http::{MAX_BODY_BYTES, build_client, describe, read_capped};
use crate::model::{
    BchnChain, BchnMempool, BchnMining, BchnNetwork, BchnPeer, BchnStatus, MAX_BCHN_PEERS,
};
use reqwest::Client;
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::json;
use std::fmt;

/// Talks to one BCHN.
#[derive(Clone)]
pub struct BchnClient {
    client: Client,
    url: String,
    username: String,
    password: String,
}

impl fmt::Debug for BchnClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BchnClient")
            .field("url", &self.url)
            .field("username", &self.username)
            .field("password", &"[redacted]")
            .finish()
    }
}

/// A JSON-RPC 1.0 answer: a result, or an error.
#[derive(Deserialize)]
struct Reply<T> {
    result: Option<T>,
    error: Option<RpcError>,
}

#[derive(Deserialize)]
struct RpcError {
    code: i64,
    message: String,
}

impl BchnClient {
    pub fn new(url: &str, username: &str, password: &str) -> Result<Self, String> {
        Ok(Self {
            client: build_client()?,
            url: url.to_string(),
            username: username.to_string(),
            password: password.to_string(),
        })
    }

    /// Calls `method` without parameters.
    async fn call<T: DeserializeOwned>(&self, method: &str) -> Result<T, String> {
        let request = json!({
            "jsonrpc": "1.0",
            "id": "p2poolv2_tui",
            "method": method,
            "params": [],
        });
        let response = self
            .client
            .post(&self.url)
            .basic_auth(&self.username, Some(&self.password))
            .json(&request)
            .send()
            .await
            .map_err(describe)?;
        let status = response.status().as_u16();
        if status == 401 || status == 403 {
            return Err(format!(
                "BCHN refused the RPC login (HTTP {status}): check [bitcoinrpc] username and \
                 password in the node config"
            ));
        }
        let body = read_capped(response, MAX_BODY_BYTES).await?;
        let reply: Reply<T> = serde_json::from_slice(&body).map_err(|error| {
            if (200..300).contains(&status) {
                format!("BCHN answered {method} in an unexpected form: {error}")
            } else {
                format!("BCHN answered {method} with HTTP {status}")
            }
        })?;
        if let Some(error) = reply.error {
            return Err(format!(
                "BCHN {method} failed: {} (code {})",
                error.message, error.code
            ));
        }
        reply
            .result
            .ok_or_else(|| format!("BCHN answered {method} without a result"))
    }

    /// One refresh of the chain, network, mempool and peers, asked for at
    /// once. The network hashrate is left out when `getmininginfo` fails.
    pub async fn status(&self) -> Result<BchnStatus, String> {
        let (chain, network, mempool, peers, mining) = tokio::join!(
            self.call::<BchnChain>("getblockchaininfo"),
            self.call::<BchnNetwork>("getnetworkinfo"),
            self.call::<BchnMempool>("getmempoolinfo"),
            self.call::<Vec<BchnPeer>>("getpeerinfo"),
            self.call::<BchnMining>("getmininginfo"),
        );
        let mut peers = peers?;
        peers.truncate(MAX_BCHN_PEERS);
        Ok(BchnStatus {
            chain: chain?,
            network: network?,
            mempool: mempool?,
            peers,
            network_hashrate: mining.ok().and_then(|mining| mining.network_hashrate),
        })
    }
}

#[cfg(test)]
mod tests;
