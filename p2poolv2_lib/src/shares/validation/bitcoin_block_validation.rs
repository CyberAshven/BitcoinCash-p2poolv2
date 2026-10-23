// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::ValidationError;
use bitcoin::consensus::encode::serialize;
use bitcoindrpc::BitcoindRpcClient;
use serde_json::json;

/// Validate the bitcoin block.
/// Expect the block to exist in the chain, if it does not, return an error and the client should retry.
#[allow(dead_code)]
pub async fn validate_bitcoin_block(
    block: &bitcoin::Block,
    bitcoindrpc_client: &BitcoindRpcClient,
) -> Result<bool, ValidationError> {
    // Serialize block to hex string for RPC call
    let block_hex = hex::encode(serialize(block));

    // Create parameters for getblocktemplate call in proposal mode
    let params = vec![json!({
        "mode": "proposal",
        "data": block_hex
    })];

    // Call getblocktemplate RPC method
    let result: Result<serde_json::Value, _> =
        bitcoindrpc_client.request("getblocktemplate", params).await;

    match result {
        Ok(response) => Ok(response == "duplicate"),
        Err(e) => Err(ValidationError::consensus(format!(
            "Bitcoin block validation failed: {e}"
        ))),
    }
}

#[cfg(test)]
mod tests;
