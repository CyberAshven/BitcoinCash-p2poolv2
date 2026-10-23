// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::api::error::ApiError;
use crate::api::server::AppState;
use axum::{
    Json,
    extract::{Query, State},
};
use bitcoin::consensus::encode;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::sync::Arc;

#[derive(Deserialize)]
pub struct TransactionQuery {
    pub txid: String,
    pub raw: Option<bool>,
}

#[derive(Serialize)]
pub struct TransactionOutput {
    pub txid: String,
    pub version: i32,
    pub lock_time: String,
    pub inputs: Vec<InputOutput>,
    pub outputs: Vec<OutputOutput>,
}

#[derive(Serialize)]
pub struct InputOutput {
    pub previous_output: String,
    pub script_sig: String,
    pub sequence: u32,
}

#[derive(Serialize)]
pub struct OutputOutput {
    pub value: u64,
    pub script_pubkey: String,
}

#[derive(Serialize)]
pub struct RawTransactionOutput {
    pub txid: String,
    pub hex: String,
}

pub(crate) async fn transaction(
    State(state): State<Arc<AppState>>,
    Query(query): Query<TransactionQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let txid = bitcoin::Txid::from_str(&query.txid)
        .map_err(|error| ApiError::BadRequest(format!("Invalid txid: {error}")))?;

    let store = state.chain_store_handle.store_handle().store();
    let tx = store
        .get_tx(&txid)
        .map_err(|error| ApiError::NotFound(format!("Transaction not found: {error}")))?;

    if query.raw.unwrap_or(false) {
        let raw_bytes = encode::serialize(&tx);
        let hex = hex::encode(raw_bytes);
        let output = RawTransactionOutput {
            txid: txid.to_string(),
            hex,
        };
        let value = serde_json::to_value(output).map_err(|error| {
            ApiError::ServerError(format!("Failed to serialize response: {error}"))
        })?;
        Ok(Json(value))
    } else {
        let inputs: Vec<InputOutput> = tx
            .input
            .iter()
            .map(|input| InputOutput {
                previous_output: format!(
                    "{}:{}",
                    input.previous_output.txid, input.previous_output.vout
                ),
                script_sig: input.script_sig.to_hex_string(),
                sequence: input.sequence.0,
            })
            .collect();

        let outputs: Vec<OutputOutput> = tx
            .output
            .iter()
            .map(|output| OutputOutput {
                value: output.value.to_sat(),
                script_pubkey: output.script_pubkey.to_hex_string(),
            })
            .collect();

        let output = TransactionOutput {
            txid: txid.to_string(),
            version: tx.version.0,
            lock_time: format!("{}", tx.lock_time),
            inputs,
            outputs,
        };
        let value = serde_json::to_value(output).map_err(|error| {
            ApiError::ServerError(format!("Failed to serialize response: {error}"))
        })?;
        Ok(Json(value))
    }
}

#[cfg(test)]
mod tests;
