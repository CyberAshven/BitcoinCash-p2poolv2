use super::*;
use crate::accounting::payout::sharechain_pplns::pplns_window::PRUNE_DEPTH;
use crate::node::request_response_handler::block_fetcher;
use crate::node::validation_worker;
use crate::pool_difficulty::AsertParent;
#[mockall_double::double]
use crate::pool_difficulty::PoolDifficulty;
use crate::shares::validation::MockDefaultShareValidator;
use crate::store::block_tx_metadata::{BlockMetadata, ChainMembership};
use crate::store::writer::StoreError;
use crate::test_utils::TestShareBlockBuilder;
use bitcoin::CompactTarget;
use bitcoin::hashes::Hash;
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn test_add_to_pending_inserts_block() {
    let (_, event_rx) = create_block_receiver_channel();
    let (block_fetcher_handle, _) = block_fetcher::create_block_fetcher_channel();
    let (validation_tx, _) = validation_worker::create_validation_channel();
    let mut receiver = BlockReceiver::new(
        event_rx,
        Arc::new(MockDefaultShareValidator::default()),
        ChainStoreHandle::default(),
        block_fetcher_handle,
        validation_tx,
    );
    let share_block = TestShareBlockBuilder::new().build();
    let block_hash = share_block.block_hash();

    receiver.add_to_pending(block_hash, share_block);

    assert_eq!(receiver.pending_count(), 1);
    assert!(receiver.pending.contains_key(&block_hash));
}

#[test]
fn test_add_to_pending_updates_descendants_index() {
    let (_, event_rx) = create_block_receiver_channel();
    let (block_fetcher_handle, _) = block_fetcher::create_block_fetcher_channel();
    let (validation_tx, _) = validation_worker::create_validation_channel();
    let mut receiver = BlockReceiver::new(
        event_rx,
        Arc::new(MockDefaultShareValidator::default()),
        ChainStoreHandle::default(),
        block_fetcher_handle,
        validation_tx,
    );

    let parent_block = TestShareBlockBuilder::new().nonce(0xe9695790).build();
    let parent_hash = parent_block.block_hash();

    let child_block = TestShareBlockBuilder::new()
        .prev_share_blockhash(parent_hash.to_string())
        .nonce(0xe9695791)
        .build();
    let child_hash = child_block.block_hash();

    receiver.add_to_pending(child_hash, child_block);

    assert_eq!(
        receiver.descendants.get(&parent_hash).unwrap(),
        &[child_hash]
    );
}

#[test]
fn test_add_to_pending_uncle_descendants() {
    let (_, event_rx) = create_block_receiver_channel();
    let (block_fetcher_handle, _) = block_fetcher::create_block_fetcher_channel();
    let (validation_tx, _) = validation_worker::create_validation_channel();
    let mut receiver = BlockReceiver::new(
        event_rx,
        Arc::new(MockDefaultShareValidator::default()),
        ChainStoreHandle::default(),
        block_fetcher_handle,
        validation_tx,
    );

    let uncle_hash = BlockHash::from_byte_array([0xaa; 32]);
    let share_block = TestShareBlockBuilder::new()
        .uncles(vec![uncle_hash])
        .build();
    let block_hash = share_block.block_hash();

    receiver.add_to_pending(block_hash, share_block);

    assert_eq!(
        receiver.descendants.get(&uncle_hash).unwrap(),
        &[block_hash]
    );
}

#[test]
fn test_remove_from_pending_cleans_descendants() {
    let (_, event_rx) = create_block_receiver_channel();
    let (block_fetcher_handle, _) = block_fetcher::create_block_fetcher_channel();
    let (validation_tx, _) = validation_worker::create_validation_channel();
    let mut receiver = BlockReceiver::new(
        event_rx,
        Arc::new(MockDefaultShareValidator::default()),
        ChainStoreHandle::default(),
        block_fetcher_handle,
        validation_tx,
    );

    let parent_block = TestShareBlockBuilder::new().nonce(0xe9695790).build();
    let parent_hash = parent_block.block_hash();

    let child_block = TestShareBlockBuilder::new()
        .prev_share_blockhash(parent_hash.to_string())
        .nonce(0xe9695791)
        .build();
    let child_hash = child_block.block_hash();

    receiver.add_to_pending(child_hash, child_block);
    assert!(receiver.descendants.contains_key(&parent_hash));

    let removed = receiver.remove_from_pending(&child_hash);
    assert!(removed.is_some());
    assert!(!receiver.descendants.contains_key(&parent_hash));
    assert_eq!(receiver.pending_count(), 0);
}

#[test]
fn test_genesis_parent_not_tracked_in_descendants_key() {
    let (_, event_rx) = create_block_receiver_channel();
    let (block_fetcher_handle, _) = block_fetcher::create_block_fetcher_channel();
    let (validation_tx, _) = validation_worker::create_validation_channel();
    let mut receiver = BlockReceiver::new(
        event_rx,
        Arc::new(MockDefaultShareValidator::default()),
        ChainStoreHandle::default(),
        block_fetcher_handle,
        validation_tx,
    );

    // Default TestShareBlockBuilder has all-zeros parent (genesis)
    let share_block = TestShareBlockBuilder::new().build();
    let block_hash = share_block.block_hash();

    receiver.add_to_pending(block_hash, share_block);

    assert!(!receiver.descendants.contains_key(&BlockHash::all_zeros()));
}

/// The parent and every uncle are handed to the store together, so both
/// are held to the same body-or-below-prune rule.
#[test]
fn test_ancestry_ready_checks_parent_and_uncles() {
    let parent_hash = BlockHash::from_byte_array([0x66; 32]);
    let uncle_hash = BlockHash::from_byte_array([0x67; 32]);

    let mut mock_store = ChainStoreHandle::default();
    mock_store
        .expect_all_block_and_uncle_data_available()
        .withf(move |hashes, prune_height| {
            hashes == [parent_hash, uncle_hash] && *prune_height == 7
        })
        .times(1)
        .returning(|_, _| Ok(true));

    let child_block = TestShareBlockBuilder::new()
        .prev_share_blockhash(parent_hash.to_string())
        .uncles(vec![uncle_hash])
        .nonce(0xe9695791)
        .build();

    let (_, event_rx) = create_block_receiver_channel();
    let (block_fetcher_handle, _) = block_fetcher::create_block_fetcher_channel();
    let (validation_tx, _) = validation_worker::create_validation_channel();
    let receiver = BlockReceiver::new(
        event_rx,
        Arc::new(MockDefaultShareValidator::default()),
        mock_store,
        block_fetcher_handle,
        validation_tx,
    );

    assert!(receiver.ancestry_ready(&child_block, 7));
}

/// A parent that has its header but not its body holds the block back.
/// Header sync marks a whole range HeaderValid before any body is fetched,
/// so header status alone would let a block validate while the Outputs CF
/// is still missing its parent's transactions.
#[test]
fn test_ancestry_not_ready_when_parent_body_missing() {
    let parent_hash = BlockHash::from_byte_array([0x55; 32]);

    let mut mock_store = ChainStoreHandle::default();
    mock_store
        .expect_all_block_and_uncle_data_available()
        .returning(|_, _| Ok(false));

    let child_block = TestShareBlockBuilder::new()
        .prev_share_blockhash(parent_hash.to_string())
        .nonce(0xe9695791)
        .build();

    let (_, event_rx) = create_block_receiver_channel();
    let (block_fetcher_handle, _) = block_fetcher::create_block_fetcher_channel();
    let (validation_tx, _) = validation_worker::create_validation_channel();
    let receiver = BlockReceiver::new(
        event_rx,
        Arc::new(MockDefaultShareValidator::default()),
        mock_store,
        block_fetcher_handle,
        validation_tx,
    );

    assert!(!receiver.ancestry_ready(&child_block, 0));
}

/// An ancestor with no metadata has not arrived yet. The store reports that
/// as an error, and the block must be buffered rather than dropped.
#[test]
fn test_ancestry_not_ready_when_ancestor_metadata_missing() {
    let mut mock_store = ChainStoreHandle::default();
    mock_store
        .expect_all_block_and_uncle_data_available()
        .returning(|_, _| Err(StoreError::NotFound("no metadata".to_string())));

    let child_block = TestShareBlockBuilder::new()
        .prev_share_blockhash(BlockHash::from_byte_array([0x55; 32]).to_string())
        .nonce(0xe9695791)
        .build();

    let (_, event_rx) = create_block_receiver_channel();
    let (block_fetcher_handle, _) = block_fetcher::create_block_fetcher_channel();
    let (validation_tx, _) = validation_worker::create_validation_channel();
    let receiver = BlockReceiver::new(
        event_rx,
        Arc::new(MockDefaultShareValidator::default()),
        mock_store,
        block_fetcher_handle,
        validation_tx,
    );

    assert!(!receiver.ancestry_ready(&child_block, 0));
}

/// Genesis has no parent to wait for, so the terminal hash is not sent to
/// the store, which has no metadata for it.
#[test]
fn test_ancestry_ready_skips_terminal_parent() {
    let mut mock_store = ChainStoreHandle::default();
    mock_store
        .expect_all_block_and_uncle_data_available()
        .withf(|hashes, _| hashes.is_empty())
        .times(1)
        .returning(|_, _| Ok(true));

    let genesis_child = TestShareBlockBuilder::new().nonce(0xe9695791).build();

    let (_, event_rx) = create_block_receiver_channel();
    let (block_fetcher_handle, _) = block_fetcher::create_block_fetcher_channel();
    let (validation_tx, _) = validation_worker::create_validation_channel();
    let receiver = BlockReceiver::new(
        event_rx,
        Arc::new(MockDefaultShareValidator::default()),
        mock_store,
        block_fetcher_handle,
        validation_tx,
    );

    assert!(receiver.ancestry_ready(&genesis_child, 0));
}

/// Bodies are only fetched within PRUNE_DEPTH of the candidate tip, so the
/// boundary the ancestry check exempts is the tip less that depth.
#[test]
fn test_prune_height_is_candidate_tip_less_prune_depth() {
    let mut mock_store = ChainStoreHandle::default();
    mock_store
        .expect_get_candidate_tip_height()
        .returning(|| Ok(Some(PRUNE_DEPTH as u32 + 5)));

    let (_, event_rx) = create_block_receiver_channel();
    let (block_fetcher_handle, _) = block_fetcher::create_block_fetcher_channel();
    let (validation_tx, _) = validation_worker::create_validation_channel();
    let mut receiver = BlockReceiver::new(
        event_rx,
        Arc::new(MockDefaultShareValidator::default()),
        mock_store,
        block_fetcher_handle,
        validation_tx,
    );

    assert_eq!(receiver.current_prune_height(), 5);
}

/// With no candidate chain yet, nothing is exempt: every ancestor needs its
/// body. That can only delay a block, never admit an unverified one.
#[test]
fn test_prune_height_is_zero_without_candidate_tip() {
    let mut mock_store = ChainStoreHandle::default();
    mock_store
        .expect_get_candidate_tip_height()
        .returning(|| Ok(None));

    let (_, event_rx) = create_block_receiver_channel();
    let (block_fetcher_handle, _) = block_fetcher::create_block_fetcher_channel();
    let (validation_tx, _) = validation_worker::create_validation_channel();
    let mut receiver = BlockReceiver::new(
        event_rx,
        Arc::new(MockDefaultShareValidator::default()),
        mock_store,
        block_fetcher_handle,
        validation_tx,
    );

    assert_eq!(receiver.current_prune_height(), 0);
}

/// A store failure reading the tip before any successful read falls
/// back to the strict end of the rule rather than exempting everything.
#[test]
fn test_prune_height_is_zero_when_first_tip_read_fails() {
    let mut mock_store = ChainStoreHandle::default();
    mock_store
        .expect_get_candidate_tip_height()
        .returning(|| Err(StoreError::Database("read failed".to_string())));

    let (_, event_rx) = create_block_receiver_channel();
    let (block_fetcher_handle, _) = block_fetcher::create_block_fetcher_channel();
    let (validation_tx, _) = validation_worker::create_validation_channel();
    let mut receiver = BlockReceiver::new(
        event_rx,
        Arc::new(MockDefaultShareValidator::default()),
        mock_store,
        block_fetcher_handle,
        validation_tx,
    );

    assert_eq!(receiver.current_prune_height(), 0);
}

/// Once bodies below the prune height may have been pruned, a failed tip
/// read must not drop the boundary to 0, which would demand bodies that
/// no longer exist. It keeps the last height read successfully.
#[test]
fn test_prune_height_keeps_last_known_when_tip_read_fails() {
    let mut mock_store = ChainStoreHandle::default();
    let reads = Arc::new(AtomicUsize::new(0));
    mock_store
        .expect_get_candidate_tip_height()
        .returning(move || match reads.fetch_add(1, Ordering::SeqCst) {
            0 => Ok(Some(PRUNE_DEPTH as u32 + 5)),
            _ => Err(StoreError::Database("read failed".to_string())),
        });

    let (_, event_rx) = create_block_receiver_channel();
    let (block_fetcher_handle, _) = block_fetcher::create_block_fetcher_channel();
    let (validation_tx, _) = validation_worker::create_validation_channel();
    let mut receiver = BlockReceiver::new(
        event_rx,
        Arc::new(MockDefaultShareValidator::default()),
        mock_store,
        block_fetcher_handle,
        validation_tx,
    );

    assert_eq!(receiver.current_prune_height(), 5);
    assert_eq!(receiver.current_prune_height(), 5);
}

#[tokio::test]
async fn test_process_share_block_with_ready_parent_commits() {
    let parent_hash = BlockHash::from_byte_array([0x88; 32]);
    let parent_header = TestShareBlockBuilder::new()
        .nonce(0xe9695790)
        .build()
        .header;
    let parent_header_clone = parent_header.clone();

    let mut mock_store = ChainStoreHandle::default();
    mock_store
        .expect_get_candidate_tip_height()
        .returning(|| Ok(Some(0)));
    mock_store
        .expect_all_block_and_uncle_data_available()
        .returning(|_, _| Ok(true));
    // The new block's own metadata lookup (fast path) returns NotFound.
    // The parent is at status Confirmed.
    mock_store
        .expect_get_block_metadata()
        .returning(move |hash| {
            if hash == &parent_hash {
                Ok(BlockMetadata {
                    expected_height: Some(0),
                    chain_work: bitcoin::Work::from_be_bytes([0u8; 32]),
                    status: Status::BlockValid,
                    chain: ChainMembership::Confirmed,
                })
            } else {
                Err(StoreError::NotFound("not found".to_string()))
            }
        });
    mock_store
        .expect_get_asert_parent()
        .with(mockall::predicate::eq(parent_hash))
        .returning(move |_| {
            Ok(AsertParent::from_header(
                &parent_header_clone,
                0,
                parent_header_clone.time,
            ))
        });
    mock_store
        .expect_add_share_block_and_organise_header()
        .returning(|_| Ok(None));

    let mut mock_validator = MockDefaultShareValidator::default();
    let mut pool_difficulty = PoolDifficulty::default();
    pool_difficulty
        .expect_calculate_target_clamped()
        .returning(|_| CompactTarget::from_consensus(crate::shares::share_block::MAX_POOL_TARGET));
    mock_validator
        .expect_pool_difficulty()
        .return_const(pool_difficulty);

    let (_, event_rx) = create_block_receiver_channel();
    let (block_fetcher_handle, mut block_fetcher_rx) =
        block_fetcher::create_block_fetcher_channel();
    let (validation_tx, mut validation_rx) = validation_worker::create_validation_channel();
    let mut receiver = BlockReceiver::new(
        event_rx,
        Arc::new(mock_validator),
        mock_store,
        block_fetcher_handle,
        validation_tx,
    );

    let mut child_block = TestShareBlockBuilder::new()
        .prev_share_blockhash(parent_hash.to_string())
        .nonce(0xe9695791)
        .build();
    child_block.header.bits =
        CompactTarget::from_consensus(crate::shares::share_block::MAX_POOL_TARGET);
    let child_hash = child_block.block_hash();

    let result = receiver.process_share_block(child_block).await;
    assert!(result.is_ok(), "Expected Ok, got: {}", result.unwrap_err());

    assert_eq!(receiver.pending_count(), 0);

    let fetcher_event = block_fetcher_rx.try_recv().unwrap();
    match fetcher_event {
        BlockFetcherEvent::BlockRequestCompleted(hash) => assert_eq!(hash, child_hash),
        other => panic!("Expected BlockRequestCompleted, got: {other}"),
    }

    let validation_event = validation_rx.try_recv().unwrap();
    match validation_event {
        ValidationEvent::ValidateBlockHash(hash) => assert_eq!(hash, child_hash),
        ValidationEvent::ValidateShareBlock(_) => {
            panic!("Expected ValidateBlockHash, got ValidateShareBlock")
        }
    }
}

/// A body for a block already known below the prune height is dropped
/// without being stored: it is never needed and would be pruned again.
#[tokio::test]
async fn test_process_share_block_below_prune_height_is_dropped() {
    let mut mock_store = ChainStoreHandle::default();
    mock_store
        .expect_get_candidate_tip_height()
        .returning(|| Ok(Some(PRUNE_DEPTH as u32 + 10)));
    mock_store.expect_get_block_metadata().returning(|_| {
        Ok(BlockMetadata {
            expected_height: Some(5),
            chain_work: bitcoin::Work::from_be_bytes([0u8; 32]),
            status: Status::HeaderValid,
            chain: ChainMembership::Confirmed,
        })
    });
    mock_store
        .expect_add_share_block_and_organise_header()
        .never();

    let (_, event_rx) = create_block_receiver_channel();
    let (block_fetcher_handle, mut block_fetcher_rx) =
        block_fetcher::create_block_fetcher_channel();
    let (validation_tx, mut validation_rx) = validation_worker::create_validation_channel();
    let mut receiver = BlockReceiver::new(
        event_rx,
        Arc::new(MockDefaultShareValidator::default()),
        mock_store,
        block_fetcher_handle,
        validation_tx,
    );
    let block = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let block_hash = block.block_hash();

    let result = receiver.process_share_block(block).await;

    assert!(result.is_ok(), "Expected Ok, got: {}", result.unwrap_err());
    assert_eq!(receiver.pending_count(), 0);
    match block_fetcher_rx.try_recv().unwrap() {
        BlockFetcherEvent::BlockRequestCompleted(hash) => assert_eq!(hash, block_hash),
        other => panic!("Expected BlockRequestCompleted, got: {other}"),
    }
    assert!(validation_rx.try_recv().is_err());
}

/// The store refusing a body below the prune target, after the
/// receiver's own check passed, is not an error and queues no validation.
#[tokio::test]
async fn test_process_share_block_refused_below_prune_target_is_ok() {
    let parent_hash = BlockHash::from_byte_array([0x88; 32]);
    let parent_header = TestShareBlockBuilder::new()
        .nonce(0xe9695790)
        .build()
        .header;

    let mut mock_store = ChainStoreHandle::default();
    mock_store
        .expect_get_candidate_tip_height()
        .returning(|| Ok(Some(0)));
    mock_store
        .expect_all_block_and_uncle_data_available()
        .returning(|_, _| Ok(true));
    mock_store
        .expect_get_block_metadata()
        .returning(move |hash| {
            if hash == &parent_hash {
                Ok(BlockMetadata {
                    expected_height: Some(0),
                    chain_work: bitcoin::Work::from_be_bytes([0u8; 32]),
                    status: Status::BlockValid,
                    chain: ChainMembership::Confirmed,
                })
            } else {
                Err(StoreError::NotFound("not found".to_string()))
            }
        });
    mock_store
        .expect_get_asert_parent()
        .with(mockall::predicate::eq(parent_hash))
        .returning(move |_| {
            Ok(AsertParent::from_header(
                &parent_header,
                0,
                parent_header.time,
            ))
        });
    mock_store
        .expect_add_share_block_and_organise_header()
        .returning(|share| Err(StoreError::BelowPruneHeight(share.block_hash())));

    let mut mock_validator = MockDefaultShareValidator::default();
    let mut pool_difficulty = PoolDifficulty::default();
    pool_difficulty
        .expect_calculate_target_clamped()
        .returning(|_| CompactTarget::from_consensus(crate::shares::share_block::MAX_POOL_TARGET));
    mock_validator
        .expect_pool_difficulty()
        .return_const(pool_difficulty);

    let (_, event_rx) = create_block_receiver_channel();
    let (block_fetcher_handle, _block_fetcher_rx) = block_fetcher::create_block_fetcher_channel();
    let (validation_tx, mut validation_rx) = validation_worker::create_validation_channel();
    let mut receiver = BlockReceiver::new(
        event_rx,
        Arc::new(mock_validator),
        mock_store,
        block_fetcher_handle,
        validation_tx,
    );
    let mut child_block = TestShareBlockBuilder::new()
        .prev_share_blockhash(parent_hash.to_string())
        .nonce(0xe9695791)
        .build();
    child_block.header.bits =
        CompactTarget::from_consensus(crate::shares::share_block::MAX_POOL_TARGET);

    let result = receiver.process_share_block(child_block).await;

    assert!(result.is_ok(), "Expected Ok, got: {}", result.unwrap_err());
    assert!(validation_rx.try_recv().is_err());
}

#[tokio::test]
async fn test_process_share_block_missing_parent_buffers_without_fetching() {
    let missing_parent_hash = BlockHash::from_byte_array([0x99; 32]);

    let mut mock_store = ChainStoreHandle::default();
    mock_store
        .expect_get_candidate_tip_height()
        .returning(|| Ok(Some(0)));
    mock_store
        .expect_all_block_and_uncle_data_available()
        .returning(|_, _| Err(StoreError::NotFound("no metadata".to_string())));
    mock_store
        .expect_get_block_metadata()
        .returning(|_| Err(StoreError::NotFound("not found".to_string())));

    let (_, event_rx) = create_block_receiver_channel();
    let (block_fetcher_handle, mut block_fetcher_rx) =
        block_fetcher::create_block_fetcher_channel();
    let (validation_tx, _validation_rx) = validation_worker::create_validation_channel();
    let mut receiver = BlockReceiver::new(
        event_rx,
        Arc::new(MockDefaultShareValidator::default()),
        mock_store,
        block_fetcher_handle,
        validation_tx,
    );

    let child_block = TestShareBlockBuilder::new()
        .prev_share_blockhash(missing_parent_hash.to_string())
        .nonce(0xe9695791)
        .build();
    let child_hash = child_block.block_hash();

    let result = receiver.process_share_block(child_block).await;
    assert!(result.is_ok());

    assert_eq!(receiver.pending_count(), 1);
    assert!(receiver.pending.contains_key(&child_hash));

    let fetcher_event = block_fetcher_rx.try_recv().unwrap();
    match fetcher_event {
        BlockFetcherEvent::BlockRequestCompleted(hash) => assert_eq!(hash, child_hash),
        other => panic!("Expected BlockRequestCompleted, got: {other}"),
    }

    assert!(
        block_fetcher_rx.try_recv().is_err(),
        "No FetchBlocks should be sent; headers-first pipeline supplies ancestors"
    );
}

/// The receiver itself does not bound the pending buffer: every block
/// naming a parent that never arrives takes a slot it never releases.
///
/// That is safe only because the admission gate in `handle_share_block`
/// now runs `validate_coinbase_proof`, which binds the share fields --
/// `prev_share_blockhash` included -- to the bitcoin header's proof of
/// work. A bitcoin header replayed under another parent fails there, so
/// each block reaching this buffer costs a share's worth of real work.
/// This test drives the receiver directly, below that gate, to pin the
/// receiver's half: it buffers whatever it is given.
#[tokio::test]
async fn test_replayed_header_under_distinct_parents_fills_pending() {
    const REPLAYS: u8 = 32;

    let mut mock_store = ChainStoreHandle::default();
    mock_store
        .expect_get_candidate_tip_height()
        .returning(|| Ok(Some(0)));
    mock_store
        .expect_all_block_and_uncle_data_available()
        .returning(|_, _| Err(StoreError::NotFound("no metadata".to_string())));
    mock_store
        .expect_get_block_metadata()
        .returning(|_| Err(StoreError::NotFound("not found".to_string())));

    let (_, event_rx) = create_block_receiver_channel();
    let (block_fetcher_handle, _block_fetcher_rx) = block_fetcher::create_block_fetcher_channel();
    let (validation_tx, _validation_rx) = validation_worker::create_validation_channel();
    let mut receiver = BlockReceiver::new(
        event_rx,
        Arc::new(MockDefaultShareValidator::default()),
        mock_store,
        block_fetcher_handle,
        validation_tx,
    );

    // Every block carries the same bitcoin header and the same declared
    // bits; only the parent hash differs. Starts at 1: an all-zeros parent
    // is the genesis sentinel, which add_to_pending does not index.
    let mut hashes = std::collections::HashSet::new();
    for replay in 1..=REPLAYS {
        let block = TestShareBlockBuilder::new()
            .prev_share_blockhash(BlockHash::from_byte_array([replay; 32]).to_string())
            .nonce(0xe9695791)
            .build();
        hashes.insert(block.block_hash());
        receiver.process_share_block(block).await.unwrap();
    }

    assert_eq!(
        hashes.len(),
        REPLAYS as usize,
        "each parent hash yields a distinct block hash, so dedupe never fires"
    );
    assert_eq!(
        receiver.pending_count(),
        REPLAYS as usize,
        "every replay occupies its own pending slot"
    );
    assert_eq!(
        receiver.descendants.len(),
        REPLAYS as usize,
        "and its own descendants entry"
    );
}

#[tokio::test]
async fn test_process_share_block_already_block_valid_is_noop() {
    let mut mock_store = ChainStoreHandle::default();
    mock_store.expect_get_block_metadata().returning(|_| {
        Ok(BlockMetadata {
            expected_height: Some(0),
            chain_work: bitcoin::Work::from_be_bytes([0u8; 32]),
            status: Status::BlockValid,
            chain: ChainMembership::None,
        })
    });

    let (_, event_rx) = create_block_receiver_channel();
    let (block_fetcher_handle, mut block_fetcher_rx) =
        block_fetcher::create_block_fetcher_channel();
    let (validation_tx, mut validation_rx) = validation_worker::create_validation_channel();
    let mut receiver = BlockReceiver::new(
        event_rx,
        Arc::new(MockDefaultShareValidator::default()),
        mock_store,
        block_fetcher_handle,
        validation_tx,
    );

    let block = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let result = receiver.process_share_block(block).await;
    assert!(result.is_ok());

    assert!(block_fetcher_rx.try_recv().is_err());
    assert!(validation_rx.try_recv().is_err());
    assert_eq!(receiver.pending_count(), 0);
}

#[tokio::test]
async fn test_process_share_block_already_header_valid_still_stores_body() {
    // When the header was synced ahead of the body (organise_header
    // during share-header sync), the block's status is already
    // HeaderValid but add_share_block was never called. Receiving
    // the body must still persist it via add_share_block and emit
    // a validation event.
    let parent_hash = BlockHash::from_byte_array([0x71; 32]);
    let parent_header = TestShareBlockBuilder::new()
        .nonce(0xe9695790)
        .build()
        .header;
    let parent_header_clone = parent_header.clone();

    let mut mock_store = ChainStoreHandle::default();
    mock_store
        .expect_get_candidate_tip_height()
        .returning(|| Ok(Some(0)));
    mock_store
        .expect_all_block_and_uncle_data_available()
        .returning(|_, _| Ok(true));
    // Both the new block (already HeaderValid) and its parent are
    // HeaderValid in the store.
    mock_store.expect_get_block_metadata().returning(|_| {
        Ok(BlockMetadata {
            expected_height: Some(0),
            chain_work: bitcoin::Work::from_be_bytes([0u8; 32]),
            status: Status::HeaderValid,
            chain: ChainMembership::None,
        })
    });
    mock_store.expect_get_asert_parent().returning(move |_| {
        Ok(AsertParent::from_header(
            &parent_header_clone,
            0,
            parent_header_clone.time,
        ))
    });
    mock_store
        .expect_add_share_block_and_organise_header()
        .with(mockall::predicate::always())
        .times(1)
        .returning(|_| Ok(None));

    let mut mock_validator = MockDefaultShareValidator::default();
    let mut pool_difficulty = PoolDifficulty::default();
    pool_difficulty
        .expect_calculate_target_clamped()
        .returning(|_| CompactTarget::from_consensus(crate::shares::share_block::MAX_POOL_TARGET));
    mock_validator
        .expect_pool_difficulty()
        .return_const(pool_difficulty);

    let (_, event_rx) = create_block_receiver_channel();
    let (block_fetcher_handle, _block_fetcher_rx) = block_fetcher::create_block_fetcher_channel();
    let (validation_tx, mut validation_rx) = validation_worker::create_validation_channel();
    let mut receiver = BlockReceiver::new(
        event_rx,
        Arc::new(mock_validator),
        mock_store,
        block_fetcher_handle,
        validation_tx,
    );

    let mut child_block = TestShareBlockBuilder::new()
        .prev_share_blockhash(parent_hash.to_string())
        .nonce(0xe9695791)
        .build();
    child_block.header.bits =
        CompactTarget::from_consensus(crate::shares::share_block::MAX_POOL_TARGET);
    let child_hash = child_block.block_hash();

    let result = receiver.process_share_block(child_block).await;
    assert!(result.is_ok(), "Expected Ok, got: {}", result.unwrap_err());

    let event = validation_rx.try_recv().unwrap();
    match event {
        ValidationEvent::ValidateBlockHash(hash) => assert_eq!(hash, child_hash),
        ValidationEvent::ValidateShareBlock(_) => {
            panic!("Expected ValidateBlockHash, got ValidateShareBlock")
        }
    }
}

#[tokio::test]
async fn test_process_share_block_asert_mismatch_dropped() {
    let parent_hash = BlockHash::from_byte_array([0xab; 32]);
    let parent_header = TestShareBlockBuilder::new()
        .nonce(0xe9695790)
        .build()
        .header;
    let parent_header_clone = parent_header.clone();

    let mut mock_store = ChainStoreHandle::default();
    mock_store
        .expect_get_candidate_tip_height()
        .returning(|| Ok(Some(0)));
    mock_store
        .expect_all_block_and_uncle_data_available()
        .returning(|_, _| Ok(true));
    mock_store
        .expect_get_block_metadata()
        .returning(move |hash| {
            if hash == &parent_hash {
                Ok(BlockMetadata {
                    expected_height: Some(0),
                    chain_work: bitcoin::Work::from_be_bytes([0u8; 32]),
                    status: Status::BlockValid,
                    chain: ChainMembership::Confirmed,
                })
            } else {
                Err(StoreError::NotFound("not found".to_string()))
            }
        });
    mock_store.expect_get_asert_parent().returning(move |_| {
        Ok(AsertParent::from_header(
            &parent_header_clone,
            0,
            parent_header_clone.time,
        ))
    });

    let mut mock_pool_difficulty = PoolDifficulty::default();
    mock_pool_difficulty
        .expect_calculate_target_clamped()
        .returning(|_| CompactTarget::from_consensus(0x1d00ffff));
    let mut mock_validator = MockDefaultShareValidator::default();
    mock_validator
        .expect_pool_difficulty()
        .return_const(mock_pool_difficulty);

    let (_, event_rx) = create_block_receiver_channel();
    let (block_fetcher_handle, _block_fetcher_rx) = block_fetcher::create_block_fetcher_channel();
    let (validation_tx, mut validation_rx) = validation_worker::create_validation_channel();
    let mut receiver = BlockReceiver::new(
        event_rx,
        Arc::new(mock_validator),
        mock_store,
        block_fetcher_handle,
        validation_tx,
    );

    let mut child_block = TestShareBlockBuilder::new()
        .prev_share_blockhash(parent_hash.to_string())
        .nonce(0xe9695791)
        .build();
    child_block.header.bits =
        CompactTarget::from_consensus(crate::shares::share_block::MAX_POOL_TARGET);

    let result = receiver.process_share_block(child_block).await;
    assert!(result.is_ok());

    // Block must NOT enter the store and validation must NOT be emitted.
    assert!(validation_rx.try_recv().is_err());
    assert_eq!(receiver.pending_count(), 0);
}

#[tokio::test]
async fn test_cascade_commits_child_when_parent_arrives() {
    let root_hash = BlockHash::from_byte_array([0xcc; 32]);
    let root_header = TestShareBlockBuilder::new()
        .nonce(0xe9695790)
        .build()
        .header;
    let root_header_clone = root_header.clone();

    // Build parent -> child chain
    let mut parent_block = TestShareBlockBuilder::new()
        .prev_share_blockhash(root_hash.to_string())
        .nonce(0xe9695791)
        .build();
    parent_block.header.bits =
        CompactTarget::from_consensus(crate::shares::share_block::MAX_POOL_TARGET);
    let parent_hash = parent_block.block_hash();

    let mut child_block = TestShareBlockBuilder::new()
        .prev_share_blockhash(parent_hash.to_string())
        .nonce(0xe9695792)
        .build();
    child_block.header.bits =
        CompactTarget::from_consensus(crate::shares::share_block::MAX_POOL_TARGET);
    let child_hash = child_block.block_hash();

    let mut mock_store = ChainStoreHandle::default();
    mock_store
        .expect_get_candidate_tip_height()
        .returning(|| Ok(Some(0)));
    // The child's ancestry: the parent has no body on the first check, and
    // has one once the parent itself commits.
    mock_store
        .expect_all_block_and_uncle_data_available()
        .withf(move |hashes, _| hashes == [parent_hash])
        .times(1)
        .returning(|_, _| Ok(false));
    mock_store
        .expect_all_block_and_uncle_data_available()
        .withf(move |hashes, _| hashes == [parent_hash])
        .returning(|_, _| Ok(true));
    // The parent's ancestry: root is already in the store.
    mock_store
        .expect_all_block_and_uncle_data_available()
        .withf(move |hashes, _| hashes == [root_hash])
        .returning(|_, _| Ok(true));
    // root_hash always Confirmed (it is the anchor in the store).
    mock_store
        .expect_get_block_metadata()
        .with(mockall::predicate::eq(root_hash))
        .returning(|_| {
            Ok(BlockMetadata {
                expected_height: Some(0),
                chain_work: bitcoin::Work::from_be_bytes([0u8; 32]),
                status: Status::BlockValid,
                chain: ChainMembership::Confirmed,
            })
        });
    // child_hash: always NotFound (never reaches the store via the
    // fast path; the new design only persists via add_share_block,
    // which the mock here is a no-op for).
    mock_store
        .expect_get_block_metadata()
        .with(mockall::predicate::eq(child_hash))
        .returning(|_| Err(StoreError::NotFound("not found".to_string())));
    // parent_hash: NotFound for the parent's own fast-path lookup, then
    // Confirmed for the cascaded child's validate_asert lookup.
    mock_store
        .expect_get_block_metadata()
        .with(mockall::predicate::eq(parent_hash))
        .times(1)
        .returning(|_| Err(StoreError::NotFound("not found".to_string())));
    mock_store
        .expect_get_block_metadata()
        .with(mockall::predicate::eq(parent_hash))
        .returning(|_| {
            Ok(BlockMetadata {
                expected_height: Some(0),
                chain_work: bitcoin::Work::from_be_bytes([0u8; 32]),
                status: Status::BlockValid,
                chain: ChainMembership::Confirmed,
            })
        });
    mock_store.expect_get_asert_parent().returning(move |_| {
        Ok(AsertParent::from_header(
            &root_header_clone,
            0,
            root_header_clone.time,
        ))
    });
    mock_store
        .expect_add_share_block_and_organise_header()
        .returning(|_| Ok(None));

    let mut mock_validator = MockDefaultShareValidator::default();
    let mut pool_difficulty = PoolDifficulty::default();
    pool_difficulty
        .expect_calculate_target_clamped()
        .returning(|_| CompactTarget::from_consensus(crate::shares::share_block::MAX_POOL_TARGET));
    mock_validator
        .expect_pool_difficulty()
        .return_const(pool_difficulty);

    let (_, event_rx) = create_block_receiver_channel();
    let (block_fetcher_handle, _block_fetcher_rx) = block_fetcher::create_block_fetcher_channel();
    let (validation_tx, mut validation_rx) = validation_worker::create_validation_channel();
    let mut receiver = BlockReceiver::new(
        event_rx,
        Arc::new(mock_validator),
        mock_store,
        block_fetcher_handle,
        validation_tx,
    );

    // Child arrives first -- parent is missing in store, so child
    // is buffered.
    let result = receiver.process_share_block(child_block).await;
    assert!(result.is_ok());
    assert_eq!(receiver.pending_count(), 1);

    // Parent arrives -- commits itself, then cascade commits the
    // buffered child.
    let result = receiver.process_share_block(parent_block).await;
    assert!(result.is_ok(), "Expected Ok, got: {}", result.unwrap_err());

    assert_eq!(receiver.pending_count(), 0);

    let event1 = validation_rx
        .try_recv()
        .expect("expected first validation event");
    match event1 {
        ValidationEvent::ValidateBlockHash(hash) => assert_eq!(hash, parent_hash),
        ValidationEvent::ValidateShareBlock(_) => {
            panic!("Expected ValidateBlockHash, got ValidateShareBlock")
        }
    }
    let event2 = validation_rx
        .try_recv()
        .expect("expected second validation event");
    match event2 {
        ValidationEvent::ValidateBlockHash(hash) => assert_eq!(hash, child_hash),
        ValidationEvent::ValidateShareBlock(_) => {
            panic!("Expected ValidateBlockHash, got ValidateShareBlock")
        }
    }
}

#[tokio::test]
async fn test_pending_block_re_received_with_ready_parent_is_processed() {
    let parent_hash = BlockHash::from_byte_array([0xdd; 32]);
    let parent_header = TestShareBlockBuilder::new()
        .nonce(0xe9695790)
        .build()
        .header;
    let parent_header_clone = parent_header.clone();

    let mut child_block = TestShareBlockBuilder::new()
        .prev_share_blockhash(parent_hash.to_string())
        .nonce(0xe9695791)
        .build();
    child_block.header.bits =
        CompactTarget::from_consensus(crate::shares::share_block::MAX_POOL_TARGET);
    let child_hash = child_block.block_hash();
    let child_block_clone = child_block.clone();

    let mut mock_store = ChainStoreHandle::default();
    mock_store
        .expect_get_candidate_tip_height()
        .returning(|| Ok(Some(0)));
    // First arrival: the parent's body is not stored yet. Second arrival:
    // it is, so the block is committed rather than left in pending.
    mock_store
        .expect_all_block_and_uncle_data_available()
        .times(1)
        .returning(|_, _| Ok(false));
    mock_store
        .expect_all_block_and_uncle_data_available()
        .returning(|_, _| Ok(true));
    // The child's own metadata lookup (fast path) is NotFound on both
    // arrivals; only add_share_block would have stored it.
    mock_store
        .expect_get_block_metadata()
        .with(mockall::predicate::eq(child_hash))
        .returning(|_| Err(StoreError::NotFound("not found".to_string())));
    // The parent's metadata is read by validate_asert on the second
    // arrival. The ancestry gate reads bodies, not metadata, so it does
    // not consume this.
    mock_store
        .expect_get_block_metadata()
        .with(mockall::predicate::eq(parent_hash))
        .returning(|_| {
            Ok(BlockMetadata {
                expected_height: Some(0),
                chain_work: bitcoin::Work::from_be_bytes([0u8; 32]),
                status: Status::HeaderValid,
                chain: ChainMembership::None,
            })
        });
    mock_store
        .expect_get_asert_parent()
        .with(mockall::predicate::eq(parent_hash))
        .returning(move |_| {
            Ok(AsertParent::from_header(
                &parent_header_clone,
                0,
                parent_header_clone.time,
            ))
        });
    mock_store
        .expect_add_share_block_and_organise_header()
        .returning(|_| Ok(None));

    let mut mock_validator = MockDefaultShareValidator::default();
    let mut pool_difficulty = PoolDifficulty::default();
    pool_difficulty
        .expect_calculate_target_clamped()
        .returning(|_| CompactTarget::from_consensus(crate::shares::share_block::MAX_POOL_TARGET));
    mock_validator
        .expect_pool_difficulty()
        .return_const(pool_difficulty);

    let (_, event_rx) = create_block_receiver_channel();
    let (block_fetcher_handle, mut block_fetcher_rx) =
        block_fetcher::create_block_fetcher_channel();
    let (validation_tx, mut validation_rx) = validation_worker::create_validation_channel();
    let mut receiver = BlockReceiver::new(
        event_rx,
        Arc::new(mock_validator),
        mock_store,
        block_fetcher_handle,
        validation_tx,
    );

    // First arrival: parent not ready, block is buffered
    let result = receiver.process_share_block(child_block).await;
    assert!(result.is_ok());
    assert_eq!(receiver.pending_count(), 1);

    // Drain the BlockRequestCompleted from first arrival
    let fetcher_event = block_fetcher_rx.try_recv().unwrap();
    match fetcher_event {
        BlockFetcherEvent::BlockRequestCompleted(hash) => assert_eq!(hash, child_hash),
        other => panic!("Expected BlockRequestCompleted, got: {other}"),
    }

    // Second arrival (via GetData response after headers synced parent).
    // Parent is now HeaderValid. Block should be processed, not dropped.
    let result = receiver.process_share_block(child_block_clone).await;
    assert!(result.is_ok(), "Expected Ok, got: {}", result.unwrap_err());

    // BlockRequestCompleted must be sent to clear the fetcher's in-flight state
    let fetcher_event = block_fetcher_rx.try_recv().unwrap();
    match fetcher_event {
        BlockFetcherEvent::BlockRequestCompleted(hash) => assert_eq!(hash, child_hash),
        other => panic!("Expected BlockRequestCompleted on re-receive, got: {other}"),
    }

    // Block should have been committed and sent to validation
    assert_eq!(receiver.pending_count(), 0);
    let validation_event = validation_rx.try_recv().unwrap();
    match validation_event {
        ValidationEvent::ValidateBlockHash(hash) => assert_eq!(hash, child_hash),
        ValidationEvent::ValidateShareBlock(_) => {
            panic!("Expected ValidateBlockHash, got ValidateShareBlock")
        }
    }
}

#[tokio::test]
async fn test_run_processes_event_and_shuts_down() {
    let missing_parent_hash = BlockHash::from_byte_array([0xbb; 32]);

    let mut mock_store = ChainStoreHandle::default();
    mock_store
        .expect_get_candidate_tip_height()
        .returning(|| Ok(Some(0)));
    mock_store
        .expect_all_block_and_uncle_data_available()
        .returning(|_, _| Err(StoreError::NotFound("no metadata".to_string())));
    mock_store
        .expect_get_block_metadata()
        .returning(|_| Err(StoreError::NotFound("not found".to_string())));

    let (event_tx, event_rx) = create_block_receiver_channel();
    let (block_fetcher_handle, _block_fetcher_rx) = block_fetcher::create_block_fetcher_channel();
    let (validation_tx, _validation_rx) = validation_worker::create_validation_channel();
    let receiver = BlockReceiver::new(
        event_rx,
        Arc::new(MockDefaultShareValidator::default()),
        mock_store,
        block_fetcher_handle,
        validation_tx,
    );

    let child_block = TestShareBlockBuilder::new()
        .prev_share_blockhash(missing_parent_hash.to_string())
        .nonce(0xe9695791)
        .build();

    let (result_tx, result_rx) = oneshot::channel();
    event_tx
        .send(BlockReceiverEvent::ShareBlockReceived {
            share_block: child_block,
            result_tx,
        })
        .await
        .unwrap();

    drop(event_tx);

    receiver.run().await;

    let result = result_rx.await.unwrap();
    assert!(result.is_ok());
}
