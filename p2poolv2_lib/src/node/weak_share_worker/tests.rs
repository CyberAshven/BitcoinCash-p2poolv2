// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use crate::shares::chain::chain_store_handle::MockChainStoreHandle;
use crate::shares::validation::{MockDefaultShareValidator, ValidationError};
use crate::store::block_tx_metadata::{BlockMetadata, ChainMembership, Status};
use crate::test_utils::{setup_test_chain_store_handle, test_weak_share_on};
use bitcoin::BlockHash;
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
    let (_events_tx, events_rx) = mpsc::channel(1);
    let mut worker = WeakShareWorker::new(
        events_rx,
        pool.clone(),
        MockChainStoreHandle::new(),
        Arc::new(validator),
        Arc::new(RwLock::new(PplnsWindow::default())),
    );

    worker.handle(WeakShareEvent {
        weak_share: test_weak_share_on(BlockHash::from_byte_array([1; 32]), 1),
        source: WeakShareSource::Peer(PeerId::random()),
    });

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
    let (_events_tx, events_rx) = mpsc::channel(1);
    let mut worker = WeakShareWorker::new(
        events_rx,
        pool.clone(),
        MockChainStoreHandle::new(),
        Arc::new(validator),
        Arc::new(RwLock::new(PplnsWindow::default())),
    );

    worker.handle(WeakShareEvent {
        weak_share: test_weak_share_on(BlockHash::from_byte_array([1; 32]), 1),
        source: WeakShareSource::Peer(PeerId::random()),
    });

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
    let mut worker = WeakShareWorker::new(
        events_rx,
        pool.clone(),
        chain_store_handle,
        Arc::new(validator),
        Arc::new(RwLock::new(PplnsWindow::default())),
    );

    worker.handle(WeakShareEvent {
        weak_share: test_weak_share_on(BlockHash::from_byte_array([1; 32]), 1),
        source: WeakShareSource::Local,
    });

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
    let mut worker = WeakShareWorker::new(
        events_rx,
        pool.clone(),
        chain_store_handle,
        Arc::new(validator),
        Arc::new(RwLock::new(PplnsWindow::default())),
    );

    worker.handle(WeakShareEvent {
        weak_share: test_weak_share_on(BlockHash::from_byte_array([1; 32]), 1),
        source: WeakShareSource::Peer(PeerId::random()),
    });

    let stats = pool.stats();
    assert_eq!(stats.counts.received, 1);
    assert_eq!(stats.counts.found, 0);
    assert_eq!(stats.pooled, 1);
}
