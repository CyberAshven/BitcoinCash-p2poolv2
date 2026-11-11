// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use crate::test_utils::{TestShareBlockBuilder, setup_test_chain_store_handle, test_weak_share_on};
use bitcoin::ScriptBuf;
use bitcoin::hashes::Hash;
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
    pool.insert(pool_weak_share(1), 3, WeakShareSource::Local);
    assert!(!pool.admit(pow_hash, WeakShareSource::Peer(PeerId::random())));
}

/// Only a weak share that passed its checks is remembered: admitting one,
/// which may then fail its checks, keeps no other copy of its proof of work
/// out.
#[tokio::test]
async fn a_weak_share_admitted_but_not_pooled_is_admitted_again() {
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let (pool, _relay_rx) = WeakPool::new(chain_store_handle);
    let pow_hash = pool_weak_share(1).pow_hash();

    assert!(pool.admit(pow_hash, WeakShareSource::Peer(PeerId::random())));
    assert!(pool.admit(pow_hash, WeakShareSource::Peer(PeerId::random())));
}

/// A held copy whose proof of work passed while it waited is dropped as a
/// duplicate when its parent arrives, rather than checked again.
#[tokio::test]
async fn a_held_weak_share_already_pooled_is_released_as_a_duplicate() {
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let (pool, _relay_rx) = WeakPool::new(chain_store_handle);
    let parent = BlockHash::from_byte_array([1; 32]);
    pool.hold(test_weak_share_on(parent, 1), WeakShareSource::Local);
    pool.insert(test_weak_share_on(parent, 1), 10, WeakShareSource::Local);

    let released = pool.release_held(&HashMap::from([(parent, 10)]), 10);

    assert!(released.is_empty());
    assert_eq!(pool.held_len(), 0);
    assert_eq!(pool.counts().refused.duplicate, 1);
}

/// One held for a retry at a tip of 10 waits while the tip is 10 and is
/// released once the tip is 11.
#[tokio::test]
async fn a_weak_share_held_for_a_retry_waits_for_the_tip_to_move() {
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let (pool, _relay_rx) = WeakPool::new(chain_store_handle);
    let parent = BlockHash::from_byte_array([1; 32]);
    pool.hold_for_retry(
        test_weak_share_on(parent, 1),
        WeakShareSource::Local,
        now_millis(),
        Some(10),
    );

    let at_the_same_tip = pool.release_held(&HashMap::from([(parent, 10)]), 10);
    let at_the_next_tip = pool.release_held(&HashMap::from([(parent, 10)]), 11);

    assert!(at_the_same_tip.is_empty());
    assert_eq!(at_the_next_tip.len(), 1);
    assert_eq!(pool.held_len(), 0);
}

/// A retry keeps the time it was first held, so it expires with it.
#[tokio::test]
async fn a_weak_share_held_again_for_a_retry_keeps_its_first_hold_time() {
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let (pool, _relay_rx) = WeakPool::new(chain_store_handle);
    let parent = BlockHash::from_byte_array([1; 32]);
    pool.lock().hold(
        test_weak_share_on(parent, 2),
        WeakShareSource::Local,
        5_000,
        None,
    );
    pool.lock().hold(
        test_weak_share_on(parent, 1),
        WeakShareSource::Local,
        1_000,
        Some(10),
    );

    pool.lock().expire_held(1_000 + max_held_millis() + 1);

    assert_eq!(pool.held_len(), 1);
    assert_eq!(pool.counts().refused.unknown_parent, 1);
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

#[tokio::test]
async fn a_weak_share_seen_again_is_counted_as_a_duplicate() {
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let (pool, _relay_rx) = WeakPool::new(chain_store_handle);
    let pow_hash = pool_weak_share(1).pow_hash();
    pool.insert(pool_weak_share(1), 3, WeakShareSource::Local);

    pool.admit(pow_hash, WeakShareSource::Peer(PeerId::random()));

    let refused = pool.counts().refused;
    assert_eq!(refused.duplicate, 1);
    assert_eq!(refused.rate_limited, 0);
}

/// A peer's burst is `PEER_WEAK_SHARE_BURST` weak shares; the next one in the
/// same millisecond is over its rate.
#[tokio::test]
async fn a_peer_past_its_burst_is_counted_as_rate_limited() {
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let (pool, _relay_rx) = WeakPool::new(chain_store_handle);
    let peer = PeerId::random();
    for nonce in 0..PEER_WEAK_SHARE_BURST as u32 {
        pool.admit(
            pool_weak_share(nonce).pow_hash(),
            WeakShareSource::Peer(peer),
        );
    }

    let admitted = pool.admit(
        pool_weak_share(PEER_WEAK_SHARE_BURST as u32).pow_hash(),
        WeakShareSource::Peer(peer),
    );

    assert!(!admitted);
    let refused = pool.counts().refused;
    assert_eq!(refused.rate_limited, 1);
    assert_eq!(refused.duplicate, 0);
}

#[tokio::test]
async fn a_weak_share_queued_for_relay_is_counted_as_relayed() {
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let (pool, _relay_rx) = WeakPool::new(chain_store_handle);

    pool.insert(pool_weak_share(1), 3, WeakShareSource::Local);

    let counts = pool.counts();
    assert_eq!(counts.relayed, 1);
    assert_eq!(counts.found, 1);
    assert_eq!(counts.received, 0);
}

/// With no relay receiver the queue is closed: the weak share is pooled but
/// not relayed.
#[tokio::test]
async fn a_weak_share_the_relay_queue_refuses_is_not_counted_as_relayed() {
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let (pool, relay_rx) = WeakPool::new(chain_store_handle);
    drop(relay_rx);

    pool.insert(
        pool_weak_share(1),
        3,
        WeakShareSource::Peer(PeerId::random()),
    );

    let stats = pool.stats();
    assert_eq!(stats.counts.relayed, 0);
    assert_eq!(stats.counts.received, 1);
    assert_eq!(stats.pooled, 1);
}

/// A full pool keeps a weak share only in place of one on an older parent;
/// one on a parent as old as the oldest pooled is refused.
#[tokio::test]
async fn a_full_pool_counts_a_weak_share_on_an_old_parent_as_pool_full() {
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let (pool, _relay_rx) = WeakPool::new(chain_store_handle);
    for nonce in 0..MAX_POOL_WEAK_SHARES as u32 {
        pool.insert(pool_weak_share(nonce), 3, WeakShareSource::Local);
    }

    pool.insert(
        pool_weak_share(MAX_POOL_WEAK_SHARES as u32),
        3,
        WeakShareSource::Local,
    );

    let stats = pool.stats();
    assert_eq!(stats.counts.refused.pool_full, 1);
    assert_eq!(stats.pooled, MAX_POOL_WEAK_SHARES);
}

#[test]
fn the_weak_pool_exposition_lists_each_series_and_every_refusal_reason() {
    let stats = WeakPoolStats {
        counts: WeakPoolCounts {
            found: 2,
            received: 3,
            relayed: 4,
            carried: 5,
            expired: 6,
            refused: WeakShareRefusals {
                duplicate: 7,
                rate_limited: 8,
                invalid: 9,
                unchecked: 10,
                pool_full: 11,
                too_deep: 13,
                unknown_parent: 14,
            },
        },
        pooled: 12,
        held: 15,
    };

    let exposition = stats.exposition();

    assert!(
        exposition.contains("# TYPE weak_shares_found_total counter\nweak_shares_found_total 2\n")
    );
    assert!(exposition.contains("weak_shares_received_total 3\n"));
    assert!(exposition.contains("weak_shares_relayed_total 4\n"));
    assert!(exposition.contains("weak_shares_expired_total 6\n"));
    assert!(exposition.contains("# TYPE weak_pool_size gauge\nweak_pool_size 12\n"));
    assert!(
        exposition.contains(
            "# TYPE weak_pool_waiting_for_parent gauge\nweak_pool_waiting_for_parent 15\n"
        )
    );
    assert!(exposition.contains("# TYPE weak_shares_refused_total counter\n"));
    assert!(exposition.contains("weak_shares_refused_total{reason=\"duplicate\"} 7\n"));
    assert!(exposition.contains("weak_shares_refused_total{reason=\"rate_limited\"} 8\n"));
    assert!(exposition.contains("weak_shares_refused_total{reason=\"invalid\"} 9\n"));
    assert!(exposition.contains("weak_shares_refused_total{reason=\"unchecked\"} 10\n"));
    assert!(exposition.contains("weak_shares_refused_total{reason=\"pool_full\"} 11\n"));
    assert!(exposition.contains("weak_shares_refused_total{reason=\"too_deep\"} 13\n"));
    assert!(exposition.contains("weak_shares_refused_total{reason=\"unknown_parent\"} 14\n"));
}

#[tokio::test]
async fn a_held_weak_share_is_released_once_its_parent_has_a_height() {
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let (pool, _relay_rx) = WeakPool::new(chain_store_handle);
    let parent = BlockHash::from_byte_array([1; 32]);
    let outcome = pool.hold(test_weak_share_on(parent, 1), WeakShareSource::Local);

    let released = pool.release_held(&HashMap::from([(parent, 10)]), 10);

    assert_eq!(outcome, HoldOutcome::NewParent);
    assert_eq!(released.len(), 1);
    assert_eq!(released[0].weak_share, test_weak_share_on(parent, 1));
    assert_eq!(released[0].parent_height, 10);
    assert_eq!(pool.held_len(), 0);
    assert!(pool.held_parents().is_empty());
}

#[tokio::test]
async fn a_held_weak_share_waits_while_its_parent_has_no_height() {
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let (pool, _relay_rx) = WeakPool::new(chain_store_handle);
    let parent = BlockHash::from_byte_array([1; 32]);
    pool.hold(test_weak_share_on(parent, 1), WeakShareSource::Local);

    let released = pool.release_held(&HashMap::new(), 10);

    assert!(released.is_empty());
    assert_eq!(pool.held_len(), 1);
    assert_eq!(pool.held_parents(), vec![parent]);
}

/// A tip at 10 takes a carrier at 11, which carries weak shares on parents
/// from 5: one on a parent at 4 can never be carried.
#[tokio::test]
async fn a_held_weak_share_whose_parent_arrives_too_deep_is_dropped() {
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let (pool, _relay_rx) = WeakPool::new(chain_store_handle);
    let parent = BlockHash::from_byte_array([1; 32]);
    pool.hold(test_weak_share_on(parent, 1), WeakShareSource::Local);

    let released = pool.release_held(&HashMap::from([(parent, 4)]), 10);

    assert!(released.is_empty());
    assert_eq!(pool.held_len(), 0);
    assert_eq!(pool.counts().refused.too_deep, 1);
}

#[tokio::test]
async fn a_held_weak_share_on_a_parent_at_the_depth_limit_is_released() {
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let (pool, _relay_rx) = WeakPool::new(chain_store_handle);
    let parent = BlockHash::from_byte_array([1; 32]);
    pool.hold(test_weak_share_on(parent, 1), WeakShareSource::Local);

    let released = pool.release_held(&HashMap::from([(parent, 5)]), 10);

    assert_eq!(released.len(), 1);
    assert_eq!(pool.counts().refused.too_deep, 0);
}

#[tokio::test]
async fn a_weak_share_held_longer_than_the_depth_in_share_periods_is_dropped() {
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let (pool, _relay_rx) = WeakPool::new(chain_store_handle);
    let parent = BlockHash::from_byte_array([1; 32]);
    pool.lock().hold(
        test_weak_share_on(parent, 1),
        WeakShareSource::Local,
        1_000,
        None,
    );

    pool.lock().expire_held(1_000 + max_held_millis() + 1);

    assert_eq!(pool.held_len(), 0);
    assert!(pool.held_parents().is_empty());
    assert_eq!(pool.counts().refused.unknown_parent, 1);
}

#[tokio::test]
async fn the_same_weak_share_is_held_once() {
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let (pool, _relay_rx) = WeakPool::new(chain_store_handle);
    let parent = BlockHash::from_byte_array([1; 32]);
    pool.hold(test_weak_share_on(parent, 1), WeakShareSource::Local);

    let outcome = pool.hold(
        test_weak_share_on(parent, 1),
        WeakShareSource::Peer(PeerId::random()),
    );

    assert_eq!(outcome, HoldOutcome::NotHeld);
    assert_eq!(pool.held_len(), 1);
    assert_eq!(pool.counts().refused.duplicate, 1);
}

/// A flood of weak shares, each on a parent of its own this node does not
/// have, keeps at most `MAX_HELD_WEAK_SHARES` held within
/// `MAX_HELD_WEAK_SHARE_BYTES`, the oldest dropped first, and as many
/// parents waited for.
#[tokio::test]
async fn a_flood_of_weak_shares_on_unknown_parents_stays_within_the_bounds() {
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let (pool, _relay_rx) = WeakPool::new(chain_store_handle);
    let flood = MAX_HELD_WEAK_SHARES as u32 + 100;
    for nonce in 0..flood {
        let mut parent = [0; 32];
        parent[..4].copy_from_slice(&nonce.to_le_bytes());
        pool.hold(
            test_weak_share_on(BlockHash::from_byte_array(parent), nonce),
            WeakShareSource::Peer(PeerId::random()),
        );
    }

    let state = pool.lock();
    assert_eq!(state.held.len(), MAX_HELD_WEAK_SHARES);
    assert_eq!(state.held_by_parent.len(), MAX_HELD_WEAK_SHARES);
    assert!(state.held_bytes <= MAX_HELD_WEAK_SHARE_BYTES);
    assert_eq!(
        state.held_bytes,
        state.held.values().map(|held| held.bytes).sum::<usize>()
    );
    assert_eq!(state.counts.refused.unknown_parent, 100);
    let oldest_kept = state.held.values().next().unwrap();
    assert_eq!(oldest_kept.weak_share.header.bitcoin_header.nonce, 100);
}
