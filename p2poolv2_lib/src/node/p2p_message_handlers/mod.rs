// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

pub mod receivers;
pub mod senders;

use crate::node::SwarmSend;
use crate::node::messages::{GetData, Message};
use crate::node::p2p_message_handlers::receivers::block_receiver::BlockReceiverHandle;
use crate::node::request_response_handler::block_fetcher::{BlockFetcherEvent, BlockFetcherHandle};
use crate::node::validation_worker::ValidationSender;
use crate::service::p2p_service::RequestContext;
#[cfg(test)]
#[mockall_double::double]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
#[cfg(not(test))]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
use crate::shares::transaction_pool::{ShareTransactionPool, SubmitOutcome, TransactionSource};
use crate::shares::validation::ShareValidator;
use crate::utils::time_provider::TimeProvider;
use receivers::getblocks::handle_getblocks;
use receivers::getdata::handle_getdata_block;
use receivers::getheaders::handle_getheaders;
use receivers::handshake::handle_handshake;
use receivers::inventory::handle_inventory;
use receivers::request_missing_blocks::request_headers_for_missing_blocks;
use receivers::share_blocks::handle_share_block;
use receivers::share_headers::handle_share_headers;
use std::error::Error;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error};

/// Most headers a `ShareHeaders` response carries (before completing the
/// last height).
///
/// Sized so the worst case fits `MAX_P2P_MESSAGE_SIZE`: every header with
/// worst-case fields, its bitcoin merkle root included, and its own coinbase
/// branch of `MAX_COINBASE_MERKLE_BRANCH_LENGTH` entries and a declared BCH
/// coinbase descriptor -- about 1.5 KB a
/// header. Real batches omit the root and share a branch per block template,
/// at well under 1 KB a header.
/// `test_full_share_headers_response_fits_max_message_size` guards it.
pub(crate) const MAX_HEADERS_IN_RESPONSE: usize = 650;

/// The Tower service that processes inbound P2P requests.
pub async fn handle_request<C: Send + Sync, T: TimeProvider + Send + Sync>(
    ctx: RequestContext<C, T>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    debug!("Received request {} from peer: {}", ctx.request, ctx.peer);
    match ctx.request {
        Message::GetShareHeaders(block_hashes, stop_block_hash) => {
            handle_getheaders(
                block_hashes,
                stop_block_hash,
                ctx.chain_store_handle,
                ctx.response_channel,
                ctx.swarm_tx,
            )
            .await
        }
        Message::GetShareBlocks(block_hashes, stop_block_hash) => {
            handle_getblocks(
                block_hashes,
                stop_block_hash,
                ctx.chain_store_handle,
                ctx.response_channel,
                ctx.swarm_tx,
            )
            .await
        }
        Message::Inventory(inventory) => {
            handle_inventory(
                inventory,
                ctx.peer,
                ctx.chain_store_handle,
                ctx.response_channel,
                ctx.swarm_tx,
            )
            .await
        }
        Message::NotFound(_) => {
            debug!("Received not found message");
            Ok(())
        }
        Message::GetData(get_data) => {
            debug!("Received get data: {:?}", get_data);
            match get_data {
                GetData::Block(block_hash) => {
                    handle_getdata_block(
                        block_hash,
                        ctx.chain_store_handle,
                        ctx.response_channel,
                        ctx.swarm_tx,
                    )
                    .await
                }
                GetData::Txid(txid) => {
                    debug!("Received txid: {:?}", txid);
                    Ok(())
                }
            }
        }
        Message::Transaction(transaction) => {
            handle_share_transaction(
                transaction,
                ctx.peer,
                ctx.share_transaction_pool.as_ref(),
                ctx.response_channel,
                ctx.swarm_tx,
            )
            .await
        }
        Message::Handshake(handshake_data) => {
            handle_handshake(
                handshake_data,
                ctx.peer,
                ctx.chain_store_handle,
                ctx.response_channel,
                ctx.swarm_tx,
            )
            .await
        }
        Message::ShareBlock(share_block) => {
            if let Err(err) = ctx
                .swarm_tx
                .send(SwarmSend::Response(ctx.response_channel, Message::Ack))
                .await
            {
                error!("Failed to send ShareBlock ack to peer {}: {err}", ctx.peer);
                return Err(format!("Failed to send ShareBlock ack: {err}").into());
            }
            let parent_hash = share_block.header.prev_share_blockhash;
            handle_share_block(
                share_block,
                &ctx.chain_store_handle,
                ctx.validation_tx,
                &ctx.block_receiver_handle,
                &ctx.block_fetcher_handle,
                ctx.share_validator.as_ref(),
            )
            .await?;
            request_headers_for_missing_blocks(
                &[parent_hash],
                ctx.peer,
                ctx.chain_store_handle,
                ctx.swarm_tx,
            )
            .await
        }
        other => {
            debug!("Unexpected request type {other}");
            Ok(())
        }
    }
}

/// Handle a share transaction a peer relayed: acknowledge it, then offer it
/// to the share transaction pool, which checks it, rate limits peers and
/// relays it on when it is new. A transaction the pool refuses is dropped
/// without penalising the peer: it may only have lost a race with another
/// spend or a new share. A node without a pool (Bitcoin) only acknowledges.
async fn handle_share_transaction<C: Send + Sync>(
    transaction: bitcoin::Transaction,
    peer: libp2p::PeerId,
    share_transaction_pool: Option<&ShareTransactionPool>,
    response_channel: C,
    swarm_tx: mpsc::Sender<SwarmSend<C>>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    if let Err(error) = swarm_tx
        .send(SwarmSend::Response(response_channel, Message::Ack))
        .await
    {
        error!("Failed to send Transaction ack to peer {peer}: {error}");
        return Err(format!("Failed to send Transaction ack: {error}").into());
    }
    let Some(pool) = share_transaction_pool else {
        debug!("Ignoring share transaction from peer {peer}: this node keeps no pool");
        return Ok(());
    };
    match pool.submit(transaction, TransactionSource::Peer(peer)) {
        Ok(SubmitOutcome::Accepted(txid)) => {
            debug!("Accepted share transaction {txid} from peer {peer}")
        }
        Ok(SubmitOutcome::AlreadyInPool(_)) => {}
        Err(rejection) => debug!("Refused share transaction from peer {peer}: {rejection}"),
    }
    Ok(())
}

/// Handle a response message received from a peer.
///
/// Unlike handle_request, this is called directly without the Tower service
/// layers (rate limiting, inactivity tracking). Responses are solicited by
/// us and libp2p only delivers them for matching outstanding requests, so
/// peer-protection middleware is unnecessary.
///
/// The swarm_tx channel is provided so that individual response handlers can
/// send follow-up messages (e.g. GetShareBlocks after receiving ShareHeaders)
/// back to the peer.
#[allow(clippy::too_many_arguments)] // wiring constructor: each parameter is a distinct collaborator, a params struct would only move the list
pub async fn handle_response<C: Send + Sync>(
    peer: libp2p::PeerId,
    response: Message,
    chain_store_handle: ChainStoreHandle,
    swarm_tx: mpsc::Sender<SwarmSend<C>>,
    block_fetcher_handle: BlockFetcherHandle,
    validation_tx: ValidationSender,
    block_receiver_handle: BlockReceiverHandle,
    share_validator: Arc<dyn ShareValidator + Send + Sync>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    debug!("Received response {} from peer: {}", response, peer);
    match response {
        Message::ShareHeaders(share_headers) => handle_share_headers(
            peer,
            share_headers,
            chain_store_handle,
            swarm_tx,
            block_fetcher_handle,
            share_validator.as_ref(),
        )
        .await
        .map_err(|e| {
            error!("Error handling received share headers: {}", e);
            e
        }),
        Message::ShareBlock(share_block) => handle_share_block(
            share_block,
            &chain_store_handle,
            validation_tx,
            &block_receiver_handle,
            &block_fetcher_handle,
            share_validator.as_ref(),
        )
        .await
        .map_err(|e| {
            error!("Failed to add share from response: {}", e);
            format!("Failed to add share from response: {e}").into()
        }),
        Message::NotFound(get_data) => {
            debug!("Received not found response from peer: {}", peer);
            match get_data {
                GetData::Block(block_hash) => {
                    let _ = block_fetcher_handle
                        .send(BlockFetcherEvent::BlockRequestCompleted(block_hash))
                        .await;
                }
                GetData::Txid(_) => {}
            }
            Ok(())
        }
        Message::Ack => {
            debug!("Received Ack response from peer: {}", peer);
            Ok(())
        }
        other => {
            debug!("Unexpected response type from peer {}: {}", peer, other);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests;
