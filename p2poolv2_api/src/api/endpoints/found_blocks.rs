// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::api::error::ApiError;
use crate::api::server::AppState;
use axum::{
    Json,
    extract::{Query, State},
};
use p2poolv2_lib::address::bch_share::BchShareNetwork;
use p2poolv2_lib::address_display::payout_address_string;
use p2poolv2_lib::store::found_block::{EffortSnapshot, FoundBlock};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Maximum number of found blocks that can be requested in a single query.
const MAX_NUM_FOUND_BLOCKS_IN_RESPONSE: u32 = 100;

/// Query parameters for the /found_blocks endpoint.
#[derive(Deserialize)]
pub struct FoundBlocksQuery {
    /// Number of most recent found blocks to return. Without it, every
    /// found block is returned.
    pub num: Option<u32>,
}

/// A bitcoin block found by the pool, as JSON.
#[derive(Serialize, Debug, PartialEq)]
pub struct FoundBlockResponse {
    pub bitcoin_height: u64,
    pub bitcoin_blockhash: String,
    pub share_blockhash: String,
    pub bitcoin_time: u32,
    pub miner_bitcoin_address: String,
    /// The work since the previous find over the network difficulty of the
    /// header that claimed this block, kept when it was accepted. Blocks
    /// accepted before snapshots were kept have no effort.
    pub effort: Option<EffortSnapshot>,
}

impl FoundBlockResponse {
    /// The store keeps the payout address in Bitcoin's base58 spelling; on a
    /// Bitcoin Cash node it is shown as CashAddr, as everywhere else.
    fn new(block: &FoundBlock, cash_network: Option<BchShareNetwork>) -> Self {
        Self {
            bitcoin_height: block.bitcoin_height,
            bitcoin_blockhash: block.bitcoin_blockhash.to_string(),
            share_blockhash: block.share_blockhash.to_string(),
            bitcoin_time: block.bitcoin_time,
            miner_bitcoin_address: match block
                .miner_bitcoin_address
                .parse::<p2poolv2_lib::config::PayoutAddress>()
            {
                Ok(address) => payout_address_string(&address, cash_network),
                Err(_) => block.miner_bitcoin_address.clone(),
            },
            effort: block.effort,
        }
    }
}

/// Returns the bitcoin blocks found by the pool, newest first. The same data
/// `/metrics` exposes as `found_block_time_seconds`, as JSON.
///
/// With `num`, only the `num` most recent blocks are read from the store,
/// so the cost of a request stays bounded as the list grows. Without it,
/// every found block is returned: the dashboards count, page and look up
/// blocks by hash in the whole list.
pub(crate) async fn found_blocks(
    State(state): State<Arc<AppState>>,
    Query(query): Query<FoundBlocksQuery>,
) -> Result<Json<Vec<FoundBlockResponse>>, ApiError> {
    let cash_network = state.app_config.cash_network;
    let Some(num) = query.num else {
        let found_blocks = state
            .chain_store_handle
            .get_found_blocks()
            .map_err(|error| {
                ApiError::ServerError(format!("Failed to read found blocks: {error}"))
            })?;
        let mut response: Vec<FoundBlockResponse> = found_blocks
            .iter()
            .map(|block| FoundBlockResponse::new(block, cash_network))
            .collect();
        response.sort_by_key(|block| std::cmp::Reverse(block.bitcoin_height));
        return Ok(Json(response));
    };

    if !(1..=MAX_NUM_FOUND_BLOCKS_IN_RESPONSE).contains(&num) {
        return Err(ApiError::BadRequest(format!(
            "num must be between 1 and {MAX_NUM_FOUND_BLOCKS_IN_RESPONSE}, got {num}"
        )));
    }

    // The response carries no total, so the walk stops at the last block
    // returned: the scan limit is `num` too.
    let latest_found_blocks = state
        .chain_store_handle
        .get_latest_found_blocks(num as usize, num as usize)
        .map_err(|error| ApiError::ServerError(format!("Failed to read found blocks: {error}")))?;
    let response: Vec<FoundBlockResponse> = latest_found_blocks
        .found_blocks
        .iter()
        .map(|block| FoundBlockResponse::new(block, cash_network))
        .collect();
    Ok(Json(response))
}
