// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::node::Message;
use crate::node::SwarmSend;
use crate::node::behaviour::request_response::ProtocolVersion;
use crate::node::messages::{GetShareHeaders, ShareHeaderBatch};
use crate::node::p2p_message_handlers::MAX_HEADERS_IN_RESPONSE;
#[cfg(test)]
#[mockall_double::double]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
#[cfg(not(test))]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
use std::error::Error;
use tokio::sync::mpsc;
use tracing::debug;

/// Handle a GetHeaders request from a peer
/// - serve in the form the requester's protocol version understands:
///   - 1.0.0: headers after the locator, each height truncated to
///     MAX_BLOCKS_PER_HEIGHT, as v0.18 receivers expect
///   - 1.1.0: every header after `resume_after` (or after the locator), with
///     no per-height cap, paged mid-height when a page fills
/// - stop at the stop block hash and limit to MAX_HEADERS_IN_RESPONSE blocks
/// - respond with all headers found, each with the stored coinbase merkle
///   branch its proof is checked against
pub async fn handle_getheaders<C: Send + Sync>(
    request: GetShareHeaders,
    chain_store_handle: ChainStoreHandle,
    response_channel: C,
    swarm_tx: mpsc::Sender<SwarmSend<C>>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    debug!("Received GetHeaders: {:?}", request);
    let response_headers = match request.protocol_version {
        ProtocolVersion::V1_0 => chain_store_handle.get_headers_for_locator(
            &request.locator,
            &request.stop,
            MAX_HEADERS_IN_RESPONSE,
        )?,
        ProtocolVersion::V1_1 => chain_store_handle.get_headers_after(
            &request.locator,
            request.resume_after,
            &request.stop,
            MAX_HEADERS_IN_RESPONSE,
        )?,
    };
    let bch_headers = chain_store_handle.carried_bch_headers(&response_headers);
    let mut entries = Vec::with_capacity(response_headers.len());
    for header in response_headers {
        let branch = chain_store_handle.get_template_merkle_branches(&header.block_hash())?;
        entries.push((header, branch));
    }
    let headers_message = Message::ShareHeaders(
        ShareHeaderBatch::from_headers_with_branches(entries).with_bch_headers(bch_headers),
    );
    // Send response and handle errors by logging them before returning
    debug!("Sending Headers {headers_message:?}");
    if let Err(err) = swarm_tx
        .send(SwarmSend::Response(response_channel, headers_message))
        .await
    {
        tracing::error!("Failed to send getheaders response: {}", err);
        return Err(format!("Failed to send getheaders response: {err}").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
