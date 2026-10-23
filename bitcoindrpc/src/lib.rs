// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use base64::{Engine, engine::general_purpose::STANDARD};
use bitcoin::consensus::encode::serialize_hex;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::{debug, error};

#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils;

/// JSON-RPC 1.0 request structure (Bitcoin Core format)
#[derive(Serialize)]
struct JsonRpcRequest {
    method: String,
    params: Vec<serde_json::Value>,
    id: u64,
}

/// JSON-RPC 1.0 response structure (Bitcoin Core format)
/// In JSON-RPC 1.0, both result and error are always present
/// One will be the actual value, the other will be null
#[derive(Deserialize, Debug)]
struct JsonRpcResponse {
    result: serde_json::Value,
    error: Option<JsonRpcError>,
}

/// JSON-RPC 1.0 error structure
#[derive(Deserialize, Debug)]
struct JsonRpcError {
    code: i32,
    message: String,
}

#[derive(Deserialize, Clone)]
#[allow(dead_code)]
pub struct BitcoinRpcConfig {
    pub url: String,
    pub username: String,
    pub password: String,
}

/// Custom Debug to redact passwords
impl std::fmt::Debug for BitcoinRpcConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BitcoinRpcConfig")
            .field("url", &self.url)
            .field("username", &self.username)
            .field("password", &"[redacted]")
            .finish()
    }
}

/// Error type for the BitcoindRpcClient
#[derive(Debug)]
pub enum BitcoindRpcError {
    HttpError { status_code: u16, message: String },
    ParseError { message: String },
    RpcError { code: i32, message: String },
    Other(String),
}

impl Error for BitcoindRpcError {
    fn description(&self) -> &str {
        match self {
            BitcoindRpcError::HttpError { message, .. } => message,
            BitcoindRpcError::ParseError { message } => message,
            BitcoindRpcError::RpcError { message, .. } => message,
            BitcoindRpcError::Other(msg) => msg,
        }
    }
}
impl fmt::Display for BitcoindRpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BitcoindRpcError::HttpError {
                status_code,
                message,
            } => {
                write!(f, "HTTP error {status_code}: {message}")
            }
            BitcoindRpcError::ParseError { message } => {
                write!(f, "Parse error: {message}")
            }
            BitcoindRpcError::RpcError { code, message } => {
                write!(f, "RPC error {code}: {message}")
            }
            BitcoindRpcError::Other(msg) => write!(f, "{msg}"),
        }
    }
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
pub struct GetBlockchainInfo {
    #[serde(rename = "initialblockdownload")]
    pub initial_block_download: bool,
}

/// The fields of a verbose `getblockheader` result that are read.
#[derive(Debug, Deserialize)]
struct VerboseBlockHeader {
    height: u64,
    /// BCHN's adaptive block size limit (ABLA) state of the block. Absent
    /// from Bitcoin Core, and from BCHN for a block it keeps no state for.
    #[serde(default)]
    ablastate: Option<AblaState>,
}

/// The field of BCHN's `ablastate` that is read.
#[derive(Debug, Deserialize)]
struct AblaState {
    nextblocksizelimit: u64,
}

/// What a verbose `getblockheader` says about a block a template builds on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockHeaderSummary {
    /// Height of the block.
    pub height: u64,
    /// Size limit, in bytes, of a block built on this one: BCHN's
    /// `ablastate.nextblocksizelimit`, the larger of the node's floor and its
    /// ABLA limit. `None` when the node reports no ABLA state for the block.
    pub next_block_size_limit: Option<u64>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct BitcoindRpcClient {
    client: reqwest::Client,
    url: String,
    request_id: Arc<AtomicU64>,
}

impl BitcoindRpcClient {
    pub fn new(url: &str, username: &str, password: &str) -> Result<Self, BitcoindRpcError> {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::AUTHORIZATION,
            format!(
                "Basic {}",
                STANDARD.encode(format!("{username}:{password}"))
            )
            .parse()
            .map_err(|e| BitcoindRpcError::Other(format!("Invalid header: {e}")))?,
        );

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .map_err(|e| BitcoindRpcError::Other(format!("Failed to create HTTP client: {e}")))?;

        Ok(Self {
            client,
            url: url.to_string(),
            request_id: Arc::new(AtomicU64::new(0)),
        })
    }

    pub async fn request<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: Vec<serde_json::Value>,
    ) -> Result<T, BitcoindRpcError> {
        let id = self.request_id.fetch_add(1, Ordering::SeqCst);

        let request = JsonRpcRequest {
            method: method.to_string(),
            params,
            id,
        };

        let response = match self.client.post(&self.url).json(&request).send().await {
            Ok(resp) => resp,
            Err(e) => {
                let status_code = e.status().map(|s| s.as_u16());
                error!(
                    "HTTP request failed to bitcoin node: status={:?}, error={}",
                    status_code, e
                );
                return Err(BitcoindRpcError::Other(format!("HTTP request failed: {e}")));
            }
        };

        let status = response.status();

        // Check for non-success HTTP status codes
        if !status.is_success() {
            let status_code = status.as_u16();
            let error_body = response
                .text()
                .await
                .unwrap_or_else(|_| "Failed to read error body".to_string());
            error!(
                "Error reaching bitcoin node with status={:?}. Message={:?}",
                status_code, error_body
            );
            return Err(BitcoindRpcError::HttpError {
                status_code,
                message: error_body,
            });
        }

        let rpc_response: JsonRpcResponse =
            response
                .json()
                .await
                .map_err(|e| BitcoindRpcError::ParseError {
                    message: format!("Failed to parse response: {e}"),
                })?;

        // JSON-RPC 1.0: check error first, then return result
        if let Some(error) = rpc_response.error {
            return Err(BitcoindRpcError::RpcError {
                code: error.code,
                message: error.message,
            });
        }

        // In JSON-RPC 1.0, result is always present (can be null for void methods like submitblock)
        serde_json::from_value(rpc_response.result).map_err(|error| BitcoindRpcError::ParseError {
            message: format!("Invalid result for {method}: {error}"),
        })
    }

    /// Get current bitcoin difficulty from bitcoind rpc
    pub async fn get_difficulty(&self) -> Result<f64, BitcoindRpcError> {
        let params: Vec<serde_json::Value> = vec![];
        let result: serde_json::Value = self.request("getdifficulty", params).await?;
        Ok(result.as_f64().unwrap())
    }

    pub async fn getblockchaininfo(&self) -> Result<GetBlockchainInfo, BitcoindRpcError> {
        self.request("getblockchaininfo", vec![]).await
    }

    /// Height of the block with `block_hash`, and the size limit BCHN applies
    /// to a block built on it, from a verbose `getblockheader`.
    ///
    /// An error means the node does not know the block, so a template
    /// building on it is not on this node's chain.
    pub async fn get_block_header_summary(
        &self,
        block_hash: &str,
    ) -> Result<BlockHeaderSummary, BitcoindRpcError> {
        let header: VerboseBlockHeader = self
            .request(
                "getblockheader",
                vec![serde_json::json!(block_hash), serde_json::json!(true)],
            )
            .await?;
        Ok(BlockHeaderSummary {
            height: header.height,
            next_block_size_limit: header.ablastate.map(|state| state.nextblocksizelimit),
        })
    }

    /// Fetch a Bitcoin template, negotiating SegWit and Signet when required.
    pub async fn getblocktemplate(
        &self,
        network: bitcoin::Network,
    ) -> Result<String, BitcoindRpcError> {
        let params = match network {
            bitcoin::Network::Signet => {
                vec![serde_json::json!({
                    "capabilities": ["coinbasetxn", "coinbase/append", "workid"],
                    "rules": ["segwit", "signet"],
                })]
            }
            _ => {
                vec![serde_json::json!({
                    "capabilities": ["coinbasetxn", "coinbase/append", "workid"],
                    "rules": ["segwit"],
                })]
            }
        };
        self.request_block_template(params).await
    }

    /// Fetch a BCHN template without requesting Bitcoin-only consensus rules.
    pub async fn get_bch_block_template(&self) -> Result<String, BitcoindRpcError> {
        self.request_block_template(vec![serde_json::json!({
            "capabilities": ["coinbasetxn", "coinbase/append", "workid"],
        })])
        .await
    }

    async fn request_block_template(
        &self,
        params: Vec<serde_json::Value>,
    ) -> Result<String, BitcoindRpcError> {
        debug!("Requesting getblocktemplate with params: {:?}", params);

        // Configure retry parameters
        const MAX_RETRIES: u32 = 5;
        const INITIAL_BACKOFF_MS: u64 = 10;
        const MAX_BACKOFF_MS: u64 = 160;

        let mut attempt = 0;
        let mut backoff_ms = INITIAL_BACKOFF_MS;
        let mut last_error = None;

        while attempt <= MAX_RETRIES {
            match self
                .request::<serde_json::Value>("getblocktemplate", params.clone())
                .await
            {
                Ok(result) => {
                    return Ok(result.to_string());
                }
                Err(e) => {
                    attempt += 1;
                    last_error = Some(e);

                    if attempt > MAX_RETRIES {
                        break;
                    }

                    debug!(
                        "getblocktemplate attempt {} failed, retrying in {}ms",
                        attempt, backoff_ms
                    );

                    // Sleep with exponential backoff
                    tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;

                    // Double the backoff for next attempt (capped at max)
                    backoff_ms = std::cmp::min(backoff_ms * 2, MAX_BACKOFF_MS);
                }
            }
        }

        Err(last_error.unwrap_or(BitcoindRpcError::Other(
            "Failed to get block template after all retries".to_string(),
        )))
    }

    /// Decode a raw transaction using bitcoind RPC
    ///
    /// Sends the transaction serialized as hex to the Bitcoin Core RPC,
    /// then receives and parses the decoded transaction information.
    pub async fn decoderawtransaction(
        &self,
        tx: &bitcoin::Transaction,
    ) -> Result<bitcoin::Transaction, BitcoindRpcError> {
        // Serialize the transaction to hex string
        let tx_hex = serialize_hex(tx);

        // Prepare params for the RPC call
        let params = vec![serde_json::Value::String(tx_hex)];

        // Make the RPC request
        match self
            .request::<serde_json::Value>("decoderawtransaction", params)
            .await
        {
            Ok(result) => {
                // Parse the response into a bitcoin::Transaction
                let tx: bitcoin::Transaction = match serde_json::from_value(result) {
                    Ok(tx) => tx,
                    Err(e) => {
                        return Err(BitcoindRpcError::Other(format!(
                            "Failed to decode raw transaction: {e}"
                        )));
                    }
                };
                Ok(tx)
            }
            Err(e) => Err(BitcoindRpcError::Other(format!(
                "Failed to decode raw transaction: {e}",
            ))),
        }
    }

    /// Validate a miner's complete template against the local node without PoW.
    pub async fn validate_block_proposal(
        &self,
        block: &bitcoin::Block,
    ) -> Result<(), BitcoindRpcError> {
        let result: serde_json::Value = self
            .request(
                "getblocktemplate",
                vec![serde_json::json!({
                    "mode": "proposal", "data": serialize_hex(block),
                })],
            )
            .await?;
        if result.is_null() {
            Ok(())
        } else {
            Err(BitcoindRpcError::Other(format!(
                "Block proposal rejected: {result}"
            )))
        }
    }

    pub async fn submit_block(&self, block: &bitcoin::Block) -> Result<String, BitcoindRpcError> {
        // Serialize the block to hex string
        let block_hex = serialize_hex(block);

        // Prepare params for the RPC call
        let params = vec![serde_json::Value::String(block_hex)];

        // Make the RPC request - submitblock returns null on success, or error string on failure
        let result: serde_json::Value = self.request("submitblock", params).await?;
        Ok(result.to_string())
    }
}

#[cfg(test)]
mod tests;
