// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::node::Message;
use crate::node::SwarmSend;
use crate::node::messages::GetData;
#[cfg(test)]
#[mockall_double::double]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
#[cfg(not(test))]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
use crate::shares::weak_pool::WeakPool;
use bitcoin::BlockHash;
use std::error::Error;
use tokio::sync::mpsc;
use tracing::debug;

/// Handle a GetData::Block request from a peer.
///
/// Looks up the requested block and responds with the full ShareBlock if
/// found. The block may not yet be confirmed or even on the candidate
/// chain, but the peer needs it to build its own chain view.
pub async fn handle_getdata_block<C: Send + Sync>(
    block_hash: BlockHash,
    chain_store_handle: ChainStoreHandle,
    response_channel: C,
    swarm_tx: mpsc::Sender<SwarmSend<C>>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    debug!("Received GetData::Block request for {}", block_hash);

    let response_message = match chain_store_handle.get_share(&block_hash) {
        Some(share_block) => {
            debug!("Serving block {} to peer", block_hash);
            let bch_headers =
                chain_store_handle.carried_bch_headers(std::slice::from_ref(&share_block.header));
            Message::ShareBlock(share_block, bch_headers)
        }
        None => {
            debug!("Block {} not found, sending notfound", block_hash);
            Message::NotFound(GetData::Block(block_hash))
        }
    };

    if let Err(err) = swarm_tx
        .send(SwarmSend::Response(response_channel, response_message))
        .await
    {
        tracing::error!("Failed to send getdata block response: {}", err);
        return Err(format!("Failed to send getdata block response: {err}").into());
    }
    Ok(())
}

/// Answer `GetData::WeakShare` from the weak pool.
///
/// A hash still in `pooled` is sent whole, including one the best chain
/// already carries: it stays in the pool until it expires. A missing hash,
/// one that is only held, and one `expire_below` removed are `NotFound`.
/// `weak_pool` is `None` on a node that keeps no weak pool, and that answer
/// is `NotFound` too. The response is always sent, so the stream is not left
/// open until timeout.
pub async fn handle_getdata_weak_share<C: Send + Sync>(
    hash: BlockHash,
    weak_pool: Option<&WeakPool>,
    response_channel: C,
    swarm_tx: mpsc::Sender<SwarmSend<C>>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    debug!("Received GetData::WeakShare request for {hash}");
    let response_message = match weak_pool.and_then(|pool| pool.pooled_weak_share(&hash)) {
        Some(weak_share) => {
            debug!("Serving weak share {hash} to peer");
            Message::WeakShare(weak_share)
        }
        None => {
            debug!("Weak share {hash} not found, sending notfound");
            Message::NotFound(GetData::WeakShare(hash))
        }
    };
    if let Err(err) = swarm_tx
        .send(SwarmSend::Response(response_channel, response_message))
        .await
    {
        debug!("Failed to send weak share getdata response: {err}");
        return Err(format!("Failed to send weak share getdata response: {err}").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[mockall_double::double]
    use crate::shares::chain::chain_store_handle::ChainStoreHandle;
    use crate::shares::weak::WEAK_DEPTH;
    use crate::shares::weak_pool::WeakShareSource;
    use crate::test_utils::{
        TestShareBlockBuilder, setup_test_chain_store_handle, test_weak_share_on,
    };
    use bitcoin::hashes::Hash;
    use p2poolv2_wallet::share_owner::ShareOwner;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn test_handle_getdata_block_found() {
        let (swarm_tx, mut swarm_rx) = mpsc::channel::<SwarmSend<u32>>(1);
        let response_channel = 1u32;
        let mut chain_store_handle = ChainStoreHandle::default();
        chain_store_handle
            .expect_carried_bch_headers()
            .returning(|_| Vec::new());

        let block = TestShareBlockBuilder::new().build();
        let block_hash = block.block_hash();
        let expected_block = block.clone();

        chain_store_handle
            .expect_get_share()
            .returning(move |_| Some(block.clone()));

        let result =
            handle_getdata_block(block_hash, chain_store_handle, response_channel, swarm_tx).await;

        assert!(result.is_ok());

        if let Some(SwarmSend::Response(channel, Message::ShareBlock(share_block, _))) =
            swarm_rx.recv().await
        {
            assert_eq!(channel, response_channel);
            assert_eq!(share_block, expected_block);
        } else {
            panic!("Expected SwarmSend::Response with ShareBlock message");
        }
    }

    #[tokio::test]
    async fn test_handle_getdata_block_not_found() {
        let (swarm_tx, mut swarm_rx) = mpsc::channel::<SwarmSend<u32>>(1);
        let response_channel = 1u32;
        let mut chain_store_handle = ChainStoreHandle::default();
        chain_store_handle
            .expect_carried_bch_headers()
            .returning(|_| Vec::new());

        let block_hash = "0000000000000000000000000000000000000000000000000000000000000001"
            .parse::<BlockHash>()
            .unwrap();

        chain_store_handle.expect_get_share().returning(|_| None);

        let result =
            handle_getdata_block(block_hash, chain_store_handle, response_channel, swarm_tx).await;

        assert!(result.is_ok());

        if let Some(SwarmSend::Response(channel, Message::NotFound(GetData::Block(hash)))) =
            swarm_rx.recv().await
        {
            assert_eq!(channel, response_channel);
            assert_eq!(hash, block_hash);
        } else {
            panic!("Expected SwarmSend::Response with NotFound message");
        }
    }

    #[tokio::test]
    async fn test_handle_getdata_block_send_failure() {
        let (swarm_tx, swarm_rx) = mpsc::channel::<SwarmSend<u32>>(1);
        let response_channel = 1u32;
        let mut chain_store_handle = ChainStoreHandle::default();
        chain_store_handle
            .expect_carried_bch_headers()
            .returning(|_| Vec::new());

        let block_hash = "0000000000000000000000000000000000000000000000000000000000000001"
            .parse::<BlockHash>()
            .unwrap();

        chain_store_handle.expect_get_share().returning(|_| None);

        // Drop receiver to cause send failure
        drop(swarm_rx);

        let result =
            handle_getdata_block(block_hash, chain_store_handle, response_channel, swarm_tx).await;

        assert!(result.is_err());
        if let Err(error) = result {
            assert!(
                error
                    .to_string()
                    .contains("Failed to send getdata block response")
            );
        }
    }

    /// The response `handle_getdata_weak_share` sends for `hash`, and the
    /// channel it names.
    async fn answered(hash: BlockHash, pool: Option<&WeakPool>) -> Message {
        let (swarm_tx, mut swarm_rx) = mpsc::channel::<SwarmSend<u32>>(1);
        let result = handle_getdata_weak_share(hash, pool, 7u32, swarm_tx).await;
        assert!(
            result.is_ok(),
            "GetData::WeakShare for {hash} sends a response"
        );
        match swarm_rx.recv().await {
            Some(SwarmSend::Response(channel, message)) => {
                assert_eq!(channel, 7u32, "response channel for weak share {hash}");
                message
            }
            other => panic!("Expected a response for weak share {hash}, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_pooled_weak_share_is_served_whole() {
        let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
        let (pool, _relay_rx) = WeakPool::new(chain_store_handle);
        let weak_share = test_weak_share_on(BlockHash::from_byte_array([1; 32]), 1);
        let hash = weak_share.pow_hash();
        pool.insert(weak_share.clone(), 3, WeakShareSource::Local);

        let message = answered(hash, Some(&pool)).await;

        assert_eq!(
            message,
            Message::WeakShare(weak_share),
            "a hash in pooled answers Message::WeakShare"
        );
    }

    #[tokio::test]
    async fn a_missing_weak_share_answers_not_found() {
        let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
        let (pool, _relay_rx) = WeakPool::new(chain_store_handle);
        let hash = test_weak_share_on(BlockHash::from_byte_array([1; 32]), 1).pow_hash();

        assert_eq!(
            answered(hash, Some(&pool)).await,
            Message::NotFound(GetData::WeakShare(hash)),
            "a missing hash answers NotFound"
        );
        assert_eq!(
            answered(hash, None).await,
            Message::NotFound(GetData::WeakShare(hash)),
            "a node with no weak pool answers NotFound"
        );
    }

    #[tokio::test]
    async fn a_weak_share_that_was_only_held_answers_not_found() {
        let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
        let (pool, _relay_rx) = WeakPool::new(chain_store_handle);
        let weak_share = test_weak_share_on(BlockHash::from_byte_array([2; 32]), 1);
        let hash = weak_share.pow_hash();
        pool.hold(weak_share, WeakShareSource::Local);

        assert_eq!(
            answered(hash, Some(&pool)).await,
            Message::NotFound(GetData::WeakShare(hash)),
            "a hash that was only held answers NotFound"
        );
    }

    #[tokio::test]
    async fn a_weak_share_removed_by_depth_answers_not_found() {
        let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
        let mining_base = TestShareBlockBuilder::new()
            .share_owner(ShareOwner::CashP2pkh([0x22; 20]))
            .build();
        chain_store_handle
            .add_share_block(mining_base.clone())
            .await
            .unwrap();
        let (pool, _relay_rx) = WeakPool::new(chain_store_handle);
        let expired = test_weak_share_on(BlockHash::from_byte_array([3; 32]), 1);
        let kept = test_weak_share_on(BlockHash::from_byte_array([4; 32]), 2);
        let expired_hash = expired.pow_hash();
        pool.insert(expired, 1, WeakShareSource::Local);
        pool.insert(kept, 2, WeakShareSource::Local);

        let _selected = pool.select_for_share(mining_base.block_hash(), WEAK_DEPTH + 1);

        assert_eq!(
            answered(expired_hash, Some(&pool)).await,
            Message::NotFound(GetData::WeakShare(expired_hash)),
            "a hash removed by expire_below answers NotFound"
        );
    }
}
