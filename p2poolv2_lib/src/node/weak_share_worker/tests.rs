// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use crate::node::messages::Message;
use crate::shares::chain::chain_store_handle::MockChainStoreHandle;
use crate::shares::validation::{MockDefaultShareValidator, ValidationError};
use crate::store::block_tx_metadata::{BlockMetadata, ChainMembership, Status};
use crate::test_utils::{setup_test_chain_store_handle, test_weak_share_on};
use bitcoin::hashes::Hash;
use libp2p::PeerId;

#[tokio::test]
async fn a_weak_share_breaking_a_rule_is_counted_as_invalid() {
    let (pool_store, _temp_dir) = setup_test_chain_store_handle(true).await;
    let (pool, _relay_rx) = WeakPool::new(pool_store);
    let mut validator = MockDefaultShareValidator::new();
    validator
        .expect_validate_weak_share()
        .returning(|_, _, _, _| Err(ValidationError::consensus("misses its weak target")));
    let mut chain_store_handle = MockChainStoreHandle::new();
    chain_store_handle
        .expect_get_block_metadata()
        .returning(|_| {
            Ok(BlockMetadata {
                expected_height: Some(5),
                chain_work: bitcoin::Work::from_be_bytes([0u8; 32]),
                status: Status::BlockValid,
                chain: ChainMembership::Confirmed,
            })
        });
    let (_events_tx, events_rx) = mpsc::channel(1);
    let (swarm_tx, _swarm_rx) = mpsc::channel::<SwarmSend<()>>(1);
    let mut worker = WeakShareWorker::new(
        events_rx,
        pool.clone(),
        chain_store_handle,
        Arc::new(validator),
        Arc::new(RwLock::new(PplnsWindow::default())),
        swarm_tx,
    );

    worker
        .handle(WeakShareEvent {
            weak_share: test_weak_share_on(BlockHash::from_byte_array([1; 32]), 1),
            source: WeakShareSource::Peer(PeerId::random()),
        })
        .await;

    let stats = pool.stats();
    assert_eq!(stats.counts.refused.invalid, 1);
    assert_eq!(stats.counts.refused.unchecked, 0);
    assert_eq!(stats.pooled, 0);
}

#[tokio::test]
async fn a_weak_share_that_cannot_be_checked_is_counted_as_unchecked() {
    let (pool_store, _temp_dir) = setup_test_chain_store_handle(true).await;
    let (pool, _relay_rx) = WeakPool::new(pool_store);
    let mut validator = MockDefaultShareValidator::new();
    validator
        .expect_validate_weak_share()
        .returning(|_, _, _, _| Err(ValidationError::store_access("read failed")));
    let mut chain_store_handle = MockChainStoreHandle::new();
    chain_store_handle
        .expect_get_block_metadata()
        .returning(|_| {
            Ok(BlockMetadata {
                expected_height: Some(5),
                chain_work: bitcoin::Work::from_be_bytes([0u8; 32]),
                status: Status::BlockValid,
                chain: ChainMembership::Confirmed,
            })
        });
    let (_events_tx, events_rx) = mpsc::channel(1);
    let (swarm_tx, _swarm_rx) = mpsc::channel::<SwarmSend<()>>(1);
    let mut worker = WeakShareWorker::new(
        events_rx,
        pool.clone(),
        chain_store_handle,
        Arc::new(validator),
        Arc::new(RwLock::new(PplnsWindow::default())),
        swarm_tx,
    );

    worker
        .handle(WeakShareEvent {
            weak_share: test_weak_share_on(BlockHash::from_byte_array([1; 32]), 1),
            source: WeakShareSource::Peer(PeerId::random()),
        })
        .await;

    let stats = pool.stats();
    assert_eq!(stats.counts.refused.unchecked, 1);
    assert_eq!(stats.counts.refused.invalid, 0);
    assert_eq!(stats.pooled, 0);
}

#[tokio::test]
async fn a_weak_share_this_nodes_miner_found_is_counted_as_found() {
    let (pool_store, _temp_dir) = setup_test_chain_store_handle(true).await;
    let (pool, _relay_rx) = WeakPool::new(pool_store);
    let mut validator = MockDefaultShareValidator::new();
    validator
        .expect_validate_weak_share()
        .returning(|_, _, _, _| Ok(()));
    let mut chain_store_handle = MockChainStoreHandle::new();
    chain_store_handle
        .expect_get_block_metadata()
        .returning(|_| {
            Ok(BlockMetadata {
                expected_height: Some(5),
                chain_work: bitcoin::Work::from_be_bytes([0u8; 32]),
                status: Status::BlockValid,
                chain: ChainMembership::Confirmed,
            })
        });
    let (_events_tx, events_rx) = mpsc::channel(1);
    let (swarm_tx, _swarm_rx) = mpsc::channel::<SwarmSend<()>>(1);
    let mut worker = WeakShareWorker::new(
        events_rx,
        pool.clone(),
        chain_store_handle,
        Arc::new(validator),
        Arc::new(RwLock::new(PplnsWindow::default())),
        swarm_tx,
    );

    worker
        .handle(WeakShareEvent {
            weak_share: test_weak_share_on(BlockHash::from_byte_array([1; 32]), 1),
            source: WeakShareSource::Local,
        })
        .await;

    let stats = pool.stats();
    assert_eq!(stats.counts.found, 1);
    assert_eq!(stats.counts.received, 0);
    assert_eq!(stats.pooled, 1);
}

#[tokio::test]
async fn a_weak_share_from_a_peer_is_counted_as_received() {
    let (pool_store, _temp_dir) = setup_test_chain_store_handle(true).await;
    let (pool, _relay_rx) = WeakPool::new(pool_store);
    let mut validator = MockDefaultShareValidator::new();
    validator
        .expect_validate_weak_share()
        .returning(|_, _, _, _| Ok(()));
    let mut chain_store_handle = MockChainStoreHandle::new();
    chain_store_handle
        .expect_get_block_metadata()
        .returning(|_| {
            Ok(BlockMetadata {
                expected_height: Some(5),
                chain_work: bitcoin::Work::from_be_bytes([0u8; 32]),
                status: Status::BlockValid,
                chain: ChainMembership::Confirmed,
            })
        });
    let (_events_tx, events_rx) = mpsc::channel(1);
    let (swarm_tx, _swarm_rx) = mpsc::channel::<SwarmSend<()>>(1);
    let mut worker = WeakShareWorker::new(
        events_rx,
        pool.clone(),
        chain_store_handle,
        Arc::new(validator),
        Arc::new(RwLock::new(PplnsWindow::default())),
        swarm_tx,
    );

    worker
        .handle(WeakShareEvent {
            weak_share: test_weak_share_on(BlockHash::from_byte_array([1; 32]), 1),
            source: WeakShareSource::Peer(PeerId::random()),
        })
        .await;

    let stats = pool.stats();
    assert_eq!(stats.counts.received, 1);
    assert_eq!(stats.counts.found, 0);
    assert_eq!(stats.pooled, 1);
}

/// The parent is not stored, so the weak share is held unchecked and its
/// peer is asked for the parent's headers, as for a share block's parent.
#[tokio::test]
async fn a_weak_share_on_an_unknown_parent_is_held_and_its_peer_asked_for_the_parent() {
    let (pool_store, _temp_dir) = setup_test_chain_store_handle(true).await;
    let (pool, _relay_rx) = WeakPool::new(pool_store);
    let mut validator = MockDefaultShareValidator::new();
    validator.expect_validate_weak_share().never();
    let parent = BlockHash::from_byte_array([1; 32]);
    let mut chain_store_handle = MockChainStoreHandle::new();
    chain_store_handle
        .expect_get_block_metadata()
        .returning(|hash| Err(StoreError::NotFound(hash.to_string())));
    chain_store_handle.expect_clone().returning(|| {
        let mut clone = MockChainStoreHandle::new();
        clone.expect_is_current().returning(|| true);
        clone
            .expect_get_missing_blockhashes()
            .returning(|hashes| hashes.to_vec());
        clone
            .expect_build_locator()
            .returning(|_| Ok(vec![BlockHash::all_zeros()]));
        clone
    });
    let (_events_tx, events_rx) = mpsc::channel(1);
    let (swarm_tx, mut swarm_rx) = mpsc::channel::<SwarmSend<()>>(4);
    let mut worker = WeakShareWorker::new(
        events_rx,
        pool.clone(),
        chain_store_handle,
        Arc::new(validator),
        Arc::new(RwLock::new(PplnsWindow::default())),
        swarm_tx,
    );
    let peer = PeerId::random();

    worker
        .handle(WeakShareEvent {
            weak_share: test_weak_share_on(parent, 1),
            source: WeakShareSource::Peer(peer),
        })
        .await;

    assert_eq!(pool.held_len(), 1);
    assert_eq!(pool.len(), 0);
    match swarm_rx.try_recv() {
        Ok(SwarmSend::Request(asked, Message::GetShareHeaders(_, _))) => assert_eq!(asked, peer),
        _ => panic!("expected a GetShareHeaders request to the weak share's peer"),
    }
}

/// A second weak share waiting for the same parent does not ask again.
#[tokio::test]
async fn a_second_weak_share_on_the_same_unknown_parent_does_not_ask_again() {
    let (pool_store, _temp_dir) = setup_test_chain_store_handle(true).await;
    let (pool, _relay_rx) = WeakPool::new(pool_store);
    let mut validator = MockDefaultShareValidator::new();
    validator.expect_validate_weak_share().never();
    let parent = BlockHash::from_byte_array([1; 32]);
    let mut chain_store_handle = MockChainStoreHandle::new();
    chain_store_handle
        .expect_get_block_metadata()
        .returning(|hash| Err(StoreError::NotFound(hash.to_string())));
    chain_store_handle.expect_clone().times(1).returning(|| {
        let mut clone = MockChainStoreHandle::new();
        clone.expect_is_current().returning(|| true);
        clone
            .expect_get_missing_blockhashes()
            .returning(|hashes| hashes.to_vec());
        clone
            .expect_build_locator()
            .returning(|_| Ok(vec![BlockHash::all_zeros()]));
        clone
    });
    let (_events_tx, events_rx) = mpsc::channel(1);
    let (swarm_tx, mut swarm_rx) = mpsc::channel::<SwarmSend<()>>(4);
    let mut worker = WeakShareWorker::new(
        events_rx,
        pool.clone(),
        chain_store_handle,
        Arc::new(validator),
        Arc::new(RwLock::new(PplnsWindow::default())),
        swarm_tx,
    );
    let peer = PeerId::random();
    worker
        .handle(WeakShareEvent {
            weak_share: test_weak_share_on(parent, 1),
            source: WeakShareSource::Peer(peer),
        })
        .await;

    worker
        .handle(WeakShareEvent {
            weak_share: test_weak_share_on(parent, 2),
            source: WeakShareSource::Peer(peer),
        })
        .await;

    assert_eq!(pool.held_len(), 2);
    assert!(swarm_rx.try_recv().is_ok());
    assert!(swarm_rx.try_recv().is_err());
}

/// Held while its parent is unknown, then checked and pooled as new once the
/// parent is stored at a height a share on the tip can still carry it from.
#[tokio::test]
async fn a_held_weak_share_is_checked_and_pooled_once_its_parent_arrives() {
    let (pool_store, _temp_dir) = setup_test_chain_store_handle(true).await;
    let (pool, mut relay_rx) = WeakPool::new(pool_store);
    let mut validator = MockDefaultShareValidator::new();
    validator
        .expect_validate_weak_share()
        .times(1)
        .returning(|_, _, _, _| Ok(()));
    let parent = BlockHash::from_byte_array([1; 32]);
    let mut chain_store_handle = MockChainStoreHandle::new();
    chain_store_handle
        .expect_get_block_metadata()
        .returning(|hash| Err(StoreError::NotFound(hash.to_string())));
    chain_store_handle
        .expect_get_block_metadata_batch()
        .returning(move |_| {
            Ok(vec![(
                parent,
                BlockMetadata {
                    expected_height: Some(10),
                    chain_work: bitcoin::Work::from_be_bytes([0u8; 32]),
                    status: Status::BlockValid,
                    chain: ChainMembership::Confirmed,
                },
            )])
        });
    chain_store_handle
        .expect_get_tip_height()
        .returning(|| Ok(Some(10)));
    let (_events_tx, events_rx) = mpsc::channel(1);
    let (swarm_tx, _swarm_rx) = mpsc::channel::<SwarmSend<()>>(4);
    let mut worker = WeakShareWorker::new(
        events_rx,
        pool.clone(),
        chain_store_handle,
        Arc::new(validator),
        Arc::new(RwLock::new(PplnsWindow::default())),
        swarm_tx,
    );
    worker
        .handle(WeakShareEvent {
            weak_share: test_weak_share_on(parent, 1),
            source: WeakShareSource::Local,
        })
        .await;
    assert_eq!(pool.held_len(), 1);

    worker.release_held();

    let stats = pool.stats();
    assert_eq!(stats.held, 0);
    assert_eq!(stats.pooled, 1);
    assert_eq!(stats.counts.found, 1);
    assert_eq!(
        relay_rx.try_recv().unwrap().weak_share,
        test_weak_share_on(parent, 1)
    );
}

/// The parent arrives at height 3 while the confirmed tip is at 10: no share
/// on the tip may carry a weak share mined on it, so it is dropped
/// unchecked.
#[tokio::test]
async fn a_held_weak_share_whose_parent_arrives_too_deep_is_dropped_unchecked() {
    let (pool_store, _temp_dir) = setup_test_chain_store_handle(true).await;
    let (pool, _relay_rx) = WeakPool::new(pool_store);
    let mut validator = MockDefaultShareValidator::new();
    validator.expect_validate_weak_share().never();
    let parent = BlockHash::from_byte_array([1; 32]);
    let mut chain_store_handle = MockChainStoreHandle::new();
    chain_store_handle
        .expect_get_block_metadata()
        .returning(|hash| Err(StoreError::NotFound(hash.to_string())));
    chain_store_handle
        .expect_get_block_metadata_batch()
        .returning(move |_| {
            Ok(vec![(
                parent,
                BlockMetadata {
                    expected_height: Some(3),
                    chain_work: bitcoin::Work::from_be_bytes([0u8; 32]),
                    status: Status::BlockValid,
                    chain: ChainMembership::Confirmed,
                },
            )])
        });
    chain_store_handle
        .expect_get_tip_height()
        .returning(|| Ok(Some(10)));
    let (_events_tx, events_rx) = mpsc::channel(1);
    let (swarm_tx, _swarm_rx) = mpsc::channel::<SwarmSend<()>>(4);
    let mut worker = WeakShareWorker::new(
        events_rx,
        pool.clone(),
        chain_store_handle,
        Arc::new(validator),
        Arc::new(RwLock::new(PplnsWindow::default())),
        swarm_tx,
    );
    worker
        .handle(WeakShareEvent {
            weak_share: test_weak_share_on(parent, 1),
            source: WeakShareSource::Local,
        })
        .await;

    worker.release_held();

    let stats = pool.stats();
    assert_eq!(stats.held, 0);
    assert_eq!(stats.pooled, 0);
    assert_eq!(stats.counts.refused.too_deep, 1);
}
