// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use crate::test_utils::{TestShareBlockBuilder, setup_test_chain_store_handle};
use bitcoin::ScriptBuf;
use p2poolv2_wallet::share_owner::ShareOwner;

/// A weak share known by the hash `nonce` gives it; the pool reads nothing
/// else of it.
fn pool_weak_share(nonce: u32) -> WeakShare {
    WeakShare {
        header: TestShareBlockBuilder::new()
            .share_owner(ShareOwner::CashP2pkh([0x33; 20]))
            .nonce(nonce)
            .build()
            .header,
        coinbase_script_sig: ScriptBuf::new(),
        coinbase_value: 1,
        merkle_branch: Vec::new(),
    }
}

#[tokio::test]
async fn a_weak_share_seen_once_is_not_admitted_again() {
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let (pool, _relay_rx) = WeakPool::new(chain_store_handle);
    let pow_hash = pool_weak_share(1).pow_hash();

    assert!(pool.admit(pow_hash, WeakShareSource::Local));
    assert!(!pool.admit(pow_hash, WeakShareSource::Peer(PeerId::random())));
}

#[tokio::test]
async fn an_accepted_weak_share_is_queued_for_relay_but_not_back_to_its_peer() {
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let (pool, mut relay_rx) = WeakPool::new(chain_store_handle);
    let peer = PeerId::random();

    pool.insert(pool_weak_share(1), 3, WeakShareSource::Peer(peer));

    let relay = relay_rx.try_recv().unwrap();
    assert_eq!(relay.weak_share, pool_weak_share(1));
    assert_eq!(relay.source, Some(peer));
    assert_eq!(pool.counts().received, 1);
}

#[tokio::test]
async fn the_pool_offers_weak_shares_oldest_parent_first() {
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let mining_base = TestShareBlockBuilder::new()
        .share_owner(ShareOwner::CashP2pkh([0x22; 20]))
        .build();
    chain_store_handle
        .add_share_block(mining_base.clone())
        .await
        .unwrap();
    let (pool, _relay_rx) = WeakPool::new(chain_store_handle);
    pool.insert(pool_weak_share(1), 4, WeakShareSource::Local);
    pool.insert(pool_weak_share(2), 3, WeakShareSource::Local);

    let selected = pool.select_for_share(mining_base.block_hash(), 4);

    assert_eq!(selected, vec![pool_weak_share(2), pool_weak_share(1)]);
}

/// A share at height 8 may carry weak shares mined on parents from height 2;
/// one mined at height 1 can never be carried and leaves the pool.
#[tokio::test]
async fn a_weak_share_whose_parent_falls_below_the_depth_expires() {
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let mining_base = TestShareBlockBuilder::new()
        .share_owner(ShareOwner::CashP2pkh([0x22; 20]))
        .build();
    chain_store_handle
        .add_share_block(mining_base.clone())
        .await
        .unwrap();
    let (pool, _relay_rx) = WeakPool::new(chain_store_handle);
    pool.insert(pool_weak_share(1), 1, WeakShareSource::Local);
    pool.insert(pool_weak_share(2), 2, WeakShareSource::Local);

    let selected = pool.select_for_share(mining_base.block_hash(), 7);

    assert_eq!(selected, vec![pool_weak_share(2)]);
    assert_eq!(pool.counts().expired, 1);
    assert_eq!(pool.len(), 1);
}

#[tokio::test]
async fn a_weak_share_the_best_chain_carries_leaves_the_pool() {
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let mining_base = TestShareBlockBuilder::new()
        .share_owner(ShareOwner::CashP2pkh([0x22; 20]))
        .weak_shares(vec![pool_weak_share(1)])
        .build();
    chain_store_handle
        .add_share_block(mining_base.clone())
        .await
        .unwrap();
    let (pool, _relay_rx) = WeakPool::new(chain_store_handle);
    pool.insert(pool_weak_share(1), 3, WeakShareSource::Local);
    pool.insert(pool_weak_share(2), 3, WeakShareSource::Local);

    let selected = pool.select_for_share(mining_base.block_hash(), 4);

    assert_eq!(selected, vec![pool_weak_share(2)]);
    assert_eq!(pool.counts().carried, 1);
}
