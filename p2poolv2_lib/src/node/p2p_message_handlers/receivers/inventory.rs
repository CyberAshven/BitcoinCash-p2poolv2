// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::node::SwarmSend;
use crate::node::messages::{InventoryMessage, Message};
use crate::node::p2p_message_handlers::receivers::request_missing_blocks::request_headers_for_missing_blocks;
use crate::node::weak_share_fetch::WeakFetchEvent;
#[cfg(test)]
#[mockall_double::double]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
#[cfg(not(test))]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
use crate::shares::weak::WEAK_CAP;
use bitcoin::BlockHash;
use std::error::Error;
use tokio::sync::mpsc;
use tracing::{debug, error};

/// Handle an Inventory message received from a peer.
///
/// Sends an Ack response on the request-response channel, then
/// when the candidate chain is current, responds to block
/// announcements by sending a getheaders request to sync any missing
/// headers from the announcing peer. The headers-first pipeline then
/// fetches the actual block data.
///
/// When the candidate chain is not current (initial sync in
/// progress), inv messages are ignored because header sync will
/// catch up independently.
pub async fn handle_inventory<C: Send + Sync>(
    inventory: InventoryMessage,
    peer: libp2p::PeerId,
    chain_store_handle: ChainStoreHandle,
    response_channel: C,
    swarm_tx: mpsc::Sender<SwarmSend<C>>,
    weak_fetch_tx: Option<mpsc::Sender<WeakFetchEvent>>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    if let Err(err) = swarm_tx
        .send(SwarmSend::Response(response_channel, Message::Ack))
        .await
    {
        error!("Failed to send inventory ack: {}", err);
        return Err(format!("Failed to send inventory ack: {err}").into());
    }

    debug!("Received Inv: {:?}", inventory);

    match inventory {
        InventoryMessage::BlockHashes(blockhashes) => {
            debug!("Received block hashes locator: {:?}", blockhashes);
            request_headers_for_missing_blocks(&blockhashes, peer, chain_store_handle, swarm_tx)
                .await?;
        }
        InventoryMessage::TransactionHashes(transaction_hashes) => {
            debug!(
                "Received transaction hashes inventory: {:?}",
                transaction_hashes
            );
        }
        InventoryMessage::WeakShareHashes(hashes) => {
            forward_weak_share_hashes(peer, hashes, weak_fetch_tx.as_ref());
        }
    }

    Ok(())
}

/// Keep the first [`WEAK_CAP`] hashes and hand them to the weak-share worker.
///
/// One inventory can name more hashes than a share may carry. The rest are
/// dropped here, and a later inventory can offer them. A node with no weak
/// pool has no fetch channel; the inventory was already acknowledged.
fn forward_weak_share_hashes(
    peer: libp2p::PeerId,
    hashes: Vec<BlockHash>,
    weak_fetch_tx: Option<&mpsc::Sender<WeakFetchEvent>>,
) {
    let hashes: Vec<BlockHash> = hashes.into_iter().take(WEAK_CAP).collect();
    if hashes.is_empty() {
        return;
    }
    let Some(weak_fetch_tx) = weak_fetch_tx else {
        debug!("Dropping weak share inventory from {peer}: this node keeps no weak pool");
        return;
    };
    if weak_fetch_tx
        .try_send(WeakFetchEvent::Announced { peer, hashes })
        .is_err()
    {
        debug!("Weak share fetch queue full, dropping inventory from {peer}");
    }
}

#[cfg(test)]
mod tests {
    use super::ChainStoreHandle;
    use crate::node::messages::InventoryMessage;
    use crate::node::p2p_message_handlers::receivers::inventory::handle_inventory;
    use crate::node::{Message, SwarmSend};
    use crate::test_utils::TestShareBlockBuilder;
    use bitcoin::BlockHash;
    use bitcoin::hashes::Hash;
    use mockall::predicate::*;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn test_handle_inventory_sends_ack_and_getheaders_when_current() {
        let mut chain_store_handle = ChainStoreHandle::default();
        let peer_id = libp2p::PeerId::random();

        let block1 = TestShareBlockBuilder::new().build();
        let block_hash1: BlockHash = block1.block_hash();

        let blockhashes = vec![block_hash1];
        let missing_blocks = vec![block_hash1];

        chain_store_handle.expect_is_current().returning(|| true);
        chain_store_handle
            .expect_get_missing_blockhashes()
            .with(eq(blockhashes.clone()))
            .returning(move |_| missing_blocks.clone());
        chain_store_handle
            .expect_build_locator()
            .return_once(|_| Ok(vec![BlockHash::all_zeros()]));

        let (swarm_tx, mut swarm_rx) = mpsc::channel::<SwarmSend<u32>>(10);
        let response_channel = 1u32;

        let inventory = InventoryMessage::BlockHashes(blockhashes);
        let result = handle_inventory(
            inventory,
            peer_id,
            chain_store_handle,
            response_channel,
            swarm_tx,
            None,
        )
        .await;

        assert!(result.is_ok(), "handle_inventory should return Ok");

        let ack_message = swarm_rx.recv().await.unwrap();
        match ack_message {
            SwarmSend::Response(channel, Message::Ack) => {
                assert_eq!(channel, 1u32);
            }
            _ => panic!("Expected SwarmSend::Response with Ack"),
        }

        let message = swarm_rx.recv().await.unwrap();
        match message {
            SwarmSend::Request(sent_peer, Message::GetShareHeaders(locator, stop_hash)) => {
                assert_eq!(sent_peer, peer_id);
                assert_eq!(locator, vec![BlockHash::all_zeros()]);
                assert_eq!(stop_hash, BlockHash::all_zeros());
            }
            _ => panic!("Expected SwarmSend::Request with GetShareHeaders"),
        }

        assert!(
            swarm_rx.try_recv().is_err(),
            "No more messages should have been sent"
        );
    }

    #[tokio::test]
    async fn test_handle_inventory_sends_ack_when_not_current() {
        let mut chain_store_handle = ChainStoreHandle::default();
        let peer_id = libp2p::PeerId::random();

        let block1 = TestShareBlockBuilder::new().build();
        let block_hash1: BlockHash = block1.block_hash();

        chain_store_handle.expect_is_current().returning(|| false);

        let (swarm_tx, mut swarm_rx) = mpsc::channel::<SwarmSend<u32>>(10);
        let response_channel = 2u32;

        let inventory = InventoryMessage::BlockHashes(vec![block_hash1]);
        let result = handle_inventory(
            inventory,
            peer_id,
            chain_store_handle,
            response_channel,
            swarm_tx,
            None,
        )
        .await;

        assert!(result.is_ok());

        let ack_message = swarm_rx.recv().await.unwrap();
        match ack_message {
            SwarmSend::Response(_, Message::Ack) => {}
            _ => panic!("Expected SwarmSend::Response with Ack"),
        }

        assert!(
            swarm_rx.try_recv().is_err(),
            "No additional messages should be sent when chain is not current"
        );
    }

    #[tokio::test]
    async fn test_handle_inventory_sends_ack_when_no_missing_blocks() {
        let mut chain_store_handle = ChainStoreHandle::default();
        let peer_id = libp2p::PeerId::random();

        let block1 = TestShareBlockBuilder::new().build();
        let block_hash1: BlockHash = block1.block_hash();

        chain_store_handle.expect_is_current().returning(|| true);
        chain_store_handle
            .expect_get_missing_blockhashes()
            .returning(|_| Vec::with_capacity(0));

        let (swarm_tx, mut swarm_rx) = mpsc::channel::<SwarmSend<u32>>(10);
        let response_channel = 3u32;

        let inventory = InventoryMessage::BlockHashes(vec![block_hash1]);
        let result = handle_inventory(
            inventory,
            peer_id,
            chain_store_handle,
            response_channel,
            swarm_tx,
            None,
        )
        .await;

        assert!(result.is_ok());

        let ack_message = swarm_rx.recv().await.unwrap();
        match ack_message {
            SwarmSend::Response(_, Message::Ack) => {}
            _ => panic!("Expected SwarmSend::Response with Ack"),
        }

        assert!(
            swarm_rx.try_recv().is_err(),
            "No additional messages should be sent when no blocks are missing"
        );
    }

    #[tokio::test]
    async fn test_handle_inventory_transaction_hashes_sends_ack() {
        let chain_store_handle = ChainStoreHandle::default();
        let peer_id = libp2p::PeerId::random();

        let (swarm_tx, mut swarm_rx) = mpsc::channel::<SwarmSend<u32>>(10);
        let response_channel = 4u32;

        let tx_hashes = vec![
            "0000000000000000000000000000000000000000000000000000000000000001"
                .parse()
                .unwrap(),
        ];
        let inventory =
            InventoryMessage::TransactionHashes(crate::shares::share_block::Txids(tx_hashes));
        let result = handle_inventory(
            inventory,
            peer_id,
            chain_store_handle,
            response_channel,
            swarm_tx,
            None,
        )
        .await;

        assert!(result.is_ok());

        let ack_message = swarm_rx.recv().await.unwrap();
        match ack_message {
            SwarmSend::Response(_, Message::Ack) => {}
            _ => panic!("Expected SwarmSend::Response with Ack"),
        }

        assert!(
            swarm_rx.try_recv().is_err(),
            "No additional messages should be sent for transaction inventory"
        );
    }

    #[tokio::test]
    async fn test_handle_inventory_weak_share_hashes_forwards_the_first_64() {
        use crate::node::weak_share_fetch::WeakFetchEvent;
        use crate::shares::weak::WEAK_CAP;

        let chain_store_handle = ChainStoreHandle::default();
        let peer_id = libp2p::PeerId::random();
        let (swarm_tx, mut swarm_rx) = mpsc::channel::<SwarmSend<u32>>(1);
        let (fetch_tx, mut fetch_rx) = mpsc::channel(1);
        let hashes: Vec<BlockHash> = (0..WEAK_CAP + 1)
            .map(|index| {
                let mut bytes = [0u8; 32];
                bytes[..4].copy_from_slice(&(index as u32).to_le_bytes());
                BlockHash::from_byte_array(bytes)
            })
            .collect();
        let dropped = hashes[WEAK_CAP];
        let inventory = InventoryMessage::WeakShareHashes(hashes.clone());

        let result = handle_inventory(
            inventory,
            peer_id,
            chain_store_handle,
            5u32,
            swarm_tx,
            Some(fetch_tx),
        )
        .await;

        assert!(result.is_ok(), "65 weak share hashes are acknowledged");
        match swarm_rx.recv().await {
            Some(SwarmSend::Response(channel, Message::Ack)) => {
                assert_eq!(channel, 5u32, "ack channel for 65 weak share hashes");
            }
            other => panic!("Expected Ack for 65 weak share hashes, got {other:?}"),
        }
        match fetch_rx.try_recv() {
            Ok(WeakFetchEvent::Announced {
                peer,
                hashes: forwarded,
            }) => {
                assert_eq!(peer, peer_id, "announcer of 65 weak share hashes");
                assert_eq!(forwarded.len(), WEAK_CAP, "65 weak share hashes forward 64");
                assert_eq!(
                    forwarded.as_slice(),
                    &hashes[..WEAK_CAP],
                    "the first 64 weak share hashes are forwarded"
                );
                assert!(
                    !forwarded.contains(&dropped),
                    "the 65th weak share hash is not forwarded"
                );
            }
            other => panic!("Expected an announcement of 64 hashes, got {other:?}"),
        }
    }
}
