use super::*;
use crate::accounting::payout::sharechain_pplns::pplns_window::PAYOUT_DEPTH;
use crate::monitoring_events::create_monitoring_event_channel;
use crate::node::validation_worker::create_validation_channel;
use crate::shares::chain::chain_store_handle::MockChainStoreHandle;
use crate::shares::validation::MockDefaultShareValidator;
use crate::shares::validation::ValidationError;
use crate::store::block_tx_metadata::Status;
use crate::store::block_tx_metadata::{BlockMetadata, ChainMembership};
use crate::stratum::work::notify::{NotifyCmd, NotifyReceiver};
use crate::test_utils::TestShareBlockBuilder;
use std::collections::HashMap;

/// Create a notify channel for tests, returning the sender and receiver.
fn create_test_notify_channel() -> (NotifySender, NotifyReceiver) {
    tokio::sync::mpsc::channel::<NotifyCmd>(10)
}

/// Create a mock PplnsWindow wrapped in Arc<RwLock<>> for tests.
/// The mock expects `update` to succeed and return false (no change).
fn create_test_pplns_window() -> Arc<RwLock<PplnsWindow>> {
    let mut mock_window = PplnsWindow::default();
    mock_window.expect_update().returning(|_| Ok(false));
    Arc::new(RwLock::new(mock_window))
}

/// Spawn a throwaway metrics actor and return its handle for tests.
fn create_test_metrics_handle() -> MetricsHandle {
    crate::accounting::stats::metrics::spawn_test_metrics_handle()
}

/// Build a stub share validator that approves every chain-context check.
fn stub_share_validator_with_success() -> Arc<dyn ShareValidator + Send + Sync> {
    let mut mock_validator = MockDefaultShareValidator::new();
    mock_validator
        .expect_validate_with_chain_context()
        .returning(|_, _, _| Ok(()));
    Arc::new(mock_validator)
}

/// Build an OrganiseWorker whose chain store returns `metadata_result`
/// for any get_block_metadata call and `body_exists` for any
/// share_block_exists call. Used to unit-test parent_state.
fn worker_with_parent_metadata(
    metadata_result: Result<BlockMetadata, StoreError>,
    body_exists: bool,
) -> OrganiseWorker {
    let mut mock_chain_handle = MockChainStoreHandle::new();
    mock_chain_handle
        .expect_get_candidate_blocks_needing_validation()
        .returning(|_, _| Ok(Vec::new()));
    // The confirmed-block follow-up reads each newly confirmed block
    // back; this test is not about the share feed, so let it find
    // nothing and skip.
    mock_chain_handle
        .expect_get_confirmed_at_height()
        .returning(|_| Err(StoreError::NotFound("not under test".to_string())));
    mock_chain_handle
        .expect_get_tip_height()
        .returning(|| Ok(None));
    mock_chain_handle
        .expect_get_block_metadata()
        .returning(move |_| metadata_result.clone());
    mock_chain_handle
        .expect_share_block_exists()
        .returning(move |_| body_exists);

    let (_organise_tx, organise_rx) = create_organise_channel();
    let (monitoring_tx, _monitoring_rx) = create_monitoring_event_channel();
    let (notify_tx, _notify_rx) = create_test_notify_channel();
    OrganiseWorker::new(
        organise_rx,
        create_validation_channel().0,
        mock_chain_handle,
        monitoring_tx,
        notify_tx,
        create_test_metrics_handle(),
        create_test_pplns_window(),
        stub_share_validator_with_success(),
    )
}

fn parent_metadata(status: Status, chain: ChainMembership) -> BlockMetadata {
    BlockMetadata {
        expected_height: Some(1),
        chain_work: bitcoin::Work::from_be_bytes([0u8; 32]),
        status,
        chain,
    }
}

#[tokio::test]
async fn test_parent_state() {
    let parent = TestShareBlockBuilder::new()
        .nonce(0xe9695791)
        .build()
        .block_hash();

    // A BlockValid parent (even off-chain) is Valid.
    let worker = worker_with_parent_metadata(
        Ok(parent_metadata(Status::BlockValid, ChainMembership::None)),
        false,
    );
    assert!(matches!(
        worker.parent_state(&parent),
        ParentState::Valid(1)
    ));

    // A confirmed parent that is only HeaderValid (genesis, or a block
    // confirmed below the PPLNS zone) is also Valid.
    let worker = worker_with_parent_metadata(
        Ok(parent_metadata(
            Status::HeaderValid,
            ChainMembership::Confirmed,
        )),
        false,
    );
    assert!(matches!(
        worker.parent_state(&parent),
        ParentState::Valid(1)
    ));

    // A HeaderValid candidate parent without its body is Pending: still
    // waiting on the fetcher, defer the block.
    let worker = worker_with_parent_metadata(
        Ok(parent_metadata(
            Status::HeaderValid,
            ChainMembership::Candidate,
        )),
        false,
    );
    assert!(matches!(
        worker.parent_state(&parent),
        ParentState::Pending(1)
    ));

    // A HeaderValid candidate parent whose body is stored is Stranded: it
    // has everything it needs to be validated but never became BlockValid.
    let worker = worker_with_parent_metadata(
        Ok(parent_metadata(
            Status::HeaderValid,
            ChainMembership::Candidate,
        )),
        true,
    );
    assert!(matches!(
        worker.parent_state(&parent),
        ParentState::Stranded(1)
    ));

    // An Invalid parent is Invalid (block is invalid by descent).
    let worker = worker_with_parent_metadata(
        Ok(parent_metadata(Status::Invalid, ChainMembership::None)),
        false,
    );
    assert!(matches!(worker.parent_state(&parent), ParentState::Invalid));

    // Missing parent metadata is Unknown.
    let worker =
        worker_with_parent_metadata(Err(StoreError::NotFound("missing".to_string())), false);
    assert!(matches!(worker.parent_state(&parent), ParentState::Unknown));
}

#[tokio::test]
async fn test_organise_worker_stops_on_channel_close() {
    let (_organise_tx, organise_rx) = create_organise_channel();
    let mut mock_chain_handle = MockChainStoreHandle::new();
    mock_chain_handle
        .expect_get_candidate_blocks_needing_validation()
        .returning(|_, _| Ok(Vec::new()));
    // The confirmed-block follow-up reads each newly confirmed block
    // back; this test is not about the share feed, so let it find
    // nothing and skip.
    mock_chain_handle
        .expect_get_confirmed_at_height()
        .returning(|_| Err(StoreError::NotFound("not under test".to_string())));
    mock_chain_handle
        .expect_get_tip_height()
        .returning(|| Ok(None));
    mock_chain_handle
        .expect_clone()
        .return_once(MockChainStoreHandle::new);
    mock_chain_handle
        .expect_mark_block_valid()
        .returning(|_| Ok(()));
    let (monitoring_tx, _monitoring_rx) = create_monitoring_event_channel();
    let (notify_tx, _notify_rx) = create_test_notify_channel();
    let worker = OrganiseWorker::new(
        organise_rx,
        create_validation_channel().0,
        mock_chain_handle,
        monitoring_tx,
        notify_tx,
        create_test_metrics_handle(),
        create_test_pplns_window(),
        stub_share_validator_with_success(),
    );

    // Drop sender so recv() returns None immediately
    drop(_organise_tx);

    let result = worker.run().await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_organise_worker_calls_organise_block() {
    let (organise_tx, organise_rx) = create_organise_channel();
    let mut mock_chain_handle = MockChainStoreHandle::new();
    mock_chain_handle
        .expect_get_candidate_blocks_needing_validation()
        .returning(|_, _| Ok(Vec::new()));
    // The confirmed-block follow-up reads each newly confirmed block
    // back; this test is not about the share feed, so let it find
    // nothing and skip.
    mock_chain_handle
        .expect_get_confirmed_at_height()
        .returning(|_| Err(StoreError::NotFound("not under test".to_string())));
    mock_chain_handle
        .expect_clone()
        .return_once(MockChainStoreHandle::new);
    // Successful chain-context validation must persist BlockValid.
    mock_chain_handle
        .expect_mark_block_valid()
        .times(1)
        .returning(|_| Ok(()));
    // Parent on the confirmed chain, so the transitivity gate lets the
    // block be marked BlockValid.
    mock_chain_handle
        .expect_get_block_metadata()
        .returning(|_| {
            Ok(BlockMetadata {
                expected_height: Some(1),
                chain_work: bitcoin::Work::from_be_bytes([0u8; 32]),
                status: Status::HeaderValid,
                chain: ChainMembership::Confirmed,
            })
        });
    mock_chain_handle
        .expect_get_candidate_tip_height()
        .returning(|| Ok(Some(1)));
    mock_chain_handle
        .expect_get_tip_height()
        .returning(|| Ok(Some(1)));
    mock_chain_handle
        .expect_promote_block()
        .returning(|_| Ok(None));

    let (monitoring_tx, _monitoring_rx) = create_monitoring_event_channel();
    let (notify_tx, _notify_rx) = create_test_notify_channel();
    let worker = OrganiseWorker::new(
        organise_rx,
        create_validation_channel().0,
        mock_chain_handle,
        monitoring_tx,
        notify_tx,
        create_test_metrics_handle(),
        create_test_pplns_window(),
        stub_share_validator_with_success(),
    );

    let share = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    organise_tx
        .send(OrganiseEvent::Block {
            share_block: share,
            content_validated: true,
        })
        .await
        .unwrap();
    drop(organise_tx);

    let result = worker.run().await;
    assert!(result.is_ok());
}

/// A prune-window block (below the PPLNS zone) must be marked BlockValid so
/// the promotion gate -- which requires BlockValid down to the prune depth
/// -- can confirm it. It was ASERT/PoW-validated by the validation worker,
/// so chain-context validation must NOT run again here.
#[tokio::test]
async fn test_organise_worker_marks_prune_window_block_block_valid() {
    let (organise_tx, organise_rx) = create_organise_channel();
    let mut mock_chain_handle = MockChainStoreHandle::new();
    mock_chain_handle
        .expect_get_candidate_blocks_needing_validation()
        .returning(|_, _| Ok(Vec::new()));
    // The confirmed-block follow-up reads each newly confirmed block
    // back; this test is not about the share feed, so let it find
    // nothing and skip.
    mock_chain_handle
        .expect_get_confirmed_at_height()
        .returning(|_| Err(StoreError::NotFound("not under test".to_string())));
    mock_chain_handle
        .expect_get_tip_height()
        .returning(|| Ok(None));
    mock_chain_handle
        .expect_clone()
        .return_once(MockChainStoreHandle::new);
    // Parent is confirmed (parent_state == Valid) and the block sits at
    // height 5, far below the candidate tip, so check_pplns_zone is false.
    mock_chain_handle
        .expect_get_block_metadata()
        .returning(|_| {
            Ok(BlockMetadata {
                expected_height: Some(5),
                chain_work: bitcoin::Work::from_be_bytes([0u8; 32]),
                status: Status::HeaderValid,
                chain: ChainMembership::Confirmed,
            })
        });
    // Candidate tip more than one payout depth above height 5, so
    // is_in_pplns_zone(5, tip) is false (prune window).
    let candidate_tip = PAYOUT_DEPTH as u32 + 100;
    mock_chain_handle
        .expect_get_candidate_tip_height()
        .returning(move || Ok(Some(candidate_tip)));
    // The prune-window block must be marked BlockValid even though it is
    // below the zone.
    mock_chain_handle
        .expect_mark_block_valid()
        .times(1)
        .returning(|_| Ok(()));
    mock_chain_handle
        .expect_promote_block()
        .returning(|_| Ok(None));

    // Below the zone, chain-context validation must NOT run.
    let mut mock_validator = MockDefaultShareValidator::new();
    mock_validator.expect_validate_with_chain_context().never();
    let share_validator: Arc<dyn ShareValidator + Send + Sync> = Arc::new(mock_validator);

    let (monitoring_tx, _monitoring_rx) = create_monitoring_event_channel();
    let (notify_tx, _notify_rx) = create_test_notify_channel();
    let worker = OrganiseWorker::new(
        organise_rx,
        create_validation_channel().0,
        mock_chain_handle,
        monitoring_tx,
        notify_tx,
        create_test_metrics_handle(),
        create_test_pplns_window(),
        share_validator,
    );

    let share = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    organise_tx
        .send(OrganiseEvent::Block {
            share_block: share,
            content_validated: true,
        })
        .await
        .unwrap();
    drop(organise_tx);

    let result = worker.run().await;
    assert!(result.is_ok());
}

/// Transitivity gate: a block whose parent is only HeaderValid on the
/// candidate chain (not BlockValid, not confirmed) must NOT be validated or
/// marked BlockValid. It is buffered until the parent is validated.
#[tokio::test]
async fn test_block_not_marked_valid_when_parent_unvalidated() {
    let (organise_tx, organise_rx) = create_organise_channel();
    let mut mock_chain_handle = MockChainStoreHandle::new();
    mock_chain_handle
        .expect_get_candidate_blocks_needing_validation()
        .returning(|_, _| Ok(Vec::new()));
    // The confirmed-block follow-up reads each newly confirmed block
    // back; this test is not about the share feed, so let it find
    // nothing and skip.
    mock_chain_handle
        .expect_get_confirmed_at_height()
        .returning(|_| Err(StoreError::NotFound("not under test".to_string())));
    mock_chain_handle
        .expect_clone()
        .return_once(MockChainStoreHandle::new);
    // Parent is HeaderValid on the candidate chain -- not yet validated.
    mock_chain_handle
        .expect_share_block_exists()
        .returning(|_| false);
    mock_chain_handle
        .expect_get_block_metadata()
        .returning(|_| {
            Ok(BlockMetadata {
                expected_height: Some(1),
                chain_work: bitcoin::Work::from_be_bytes([0u8; 32]),
                status: Status::HeaderValid,
                chain: ChainMembership::Candidate,
            })
        });
    mock_chain_handle
        .expect_get_candidate_tip_height()
        .returning(|| Ok(Some(1)));
    mock_chain_handle
        .expect_get_tip_height()
        .returning(|| Ok(Some(1)));
    // The block is buffered (parent not validated): never validated, never
    // marked, never promoted. The worker opportunistically tries
    // organise_block from the buffer branch.
    mock_chain_handle.expect_mark_block_valid().never();
    mock_chain_handle.expect_promote_block().never();
    mock_chain_handle
        .expect_organise_block()
        .returning(|| Ok(None));

    let (monitoring_tx, _monitoring_rx) = create_monitoring_event_channel();
    let (notify_tx, _notify_rx) = create_test_notify_channel();
    let worker = OrganiseWorker::new(
        organise_rx,
        create_validation_channel().0,
        mock_chain_handle,
        monitoring_tx,
        notify_tx,
        create_test_metrics_handle(),
        create_test_pplns_window(),
        stub_share_validator_with_success(),
    );

    let share = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    organise_tx
        .send(OrganiseEvent::Block {
            share_block: share,
            content_validated: true,
        })
        .await
        .unwrap();
    drop(organise_tx);

    let result = worker.run().await;
    assert!(result.is_ok());
}

/// A Consensus validation failure marks the block Invalid (rebuilding the
/// candidate chain from the invalid block's parent onto the best surviving
/// branch) and then advances the confirmed chain onto it via organise_block
/// -- it must not wait for the next event. The invalid block itself is never
/// promoted.
#[tokio::test]
async fn test_organise_worker_advances_confirmed_after_consensus_invalidation() {
    let (organise_tx, organise_rx) = create_organise_channel();
    let mut mock_chain_handle = MockChainStoreHandle::new();
    mock_chain_handle
        .expect_get_candidate_blocks_needing_validation()
        .returning(|_, _| Ok(Vec::new()));
    // The confirmed-block follow-up reads the block back; this test is
    // not about the share feed, so let it find nothing and skip.
    mock_chain_handle
        .expect_get_confirmed_at_height()
        .returning(|_| Err(StoreError::NotFound("not under test".to_string())));
    mock_chain_handle
        .expect_clone()
        .return_once(MockChainStoreHandle::new);
    // Validation failed -> the block must NOT be marked BlockValid.
    mock_chain_handle.expect_mark_block_valid().never();
    mock_chain_handle
        .expect_get_block_metadata()
        .returning(|_| {
            Ok(BlockMetadata {
                expected_height: Some(10),
                chain_work: bitcoin::Work::from_hex("0x00").unwrap(),
                status: Status::BlockValid,
                chain: ChainMembership::Confirmed,
            })
        });
    mock_chain_handle
        .expect_get_candidate_tip_height()
        .returning(|| Ok(Some(10)));
    mock_chain_handle
        .expect_get_tip_height()
        .returning(|| Ok(Some(10)));
    // On chain-context validation failure the block must be marked Invalid
    // (so promotion never confirms it) and promote_block must NOT be called
    // for it. mark_invalid rebuilds the candidate chain from the invalid
    // block's parent onto the best surviving branch, and the worker then
    // calls organise_block to advance the confirmed chain onto it -- rather
    // than waiting for the next event.
    mock_chain_handle
        .expect_mark_invalid()
        .times(1)
        .returning(|_| Ok(()));
    mock_chain_handle.expect_promote_block().never();
    mock_chain_handle
        .expect_organise_block()
        .times(1)
        .returning(|| Ok(Some(6)));

    let mut mock_validator = MockDefaultShareValidator::new();
    mock_validator
        .expect_validate_with_chain_context()
        .returning(|_, _, _| Err(ValidationError::consensus("rejected for test")));
    let share_validator: Arc<dyn ShareValidator + Send + Sync> = Arc::new(mock_validator);

    let (monitoring_tx, _monitoring_rx) = create_monitoring_event_channel();
    let (notify_tx, _notify_rx) = create_test_notify_channel();
    let worker = OrganiseWorker::new(
        organise_rx,
        create_validation_channel().0,
        mock_chain_handle,
        monitoring_tx,
        notify_tx,
        create_test_metrics_handle(),
        create_test_pplns_window(),
        share_validator,
    );

    let share = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    organise_tx
        .send(OrganiseEvent::Block {
            share_block: share,
            content_validated: true,
        })
        .await
        .unwrap();
    drop(organise_tx);

    let result = worker.run().await;
    assert!(result.is_ok());
}

/// An InvalidBlock event marks the named block Invalid, then runs
/// organise_block so the confirmed chain can advance onto the surviving
/// branch. The invalid block itself is never validated or promoted, and
/// organise_block only promotes candidates that are already BlockValid, so
/// nothing reaches the confirmed chain that skipped validation.
///
/// The validation worker sends this when a pre-context check fails with all
/// its dependencies present.
#[tokio::test]
async fn test_organise_worker_marks_invalid_block_event() {
    let (organise_tx, organise_rx) = create_organise_channel();
    let mut mock_chain_handle = MockChainStoreHandle::new();
    mock_chain_handle
        .expect_get_candidate_blocks_needing_validation()
        .returning(|_, _| Ok(Vec::new()));
    // The confirmed-block follow-up reads each newly confirmed block
    // back; this test is not about the share feed, so let it find
    // nothing and skip.
    mock_chain_handle
        .expect_get_confirmed_at_height()
        .returning(|_| Err(StoreError::NotFound("not under test".to_string())));
    mock_chain_handle
        .expect_clone()
        .return_once(MockChainStoreHandle::new);
    mock_chain_handle
        .expect_mark_invalid()
        .times(1)
        .returning(|_| Ok(()));
    mock_chain_handle
        .expect_get_tip_height()
        .returning(|| Ok(Some(10)));
    mock_chain_handle
        .expect_organise_block()
        .times(1)
        .returning(|| Ok(None));
    // The block failed validation, so it must never be marked valid or promoted.
    mock_chain_handle.expect_mark_block_valid().never();
    mock_chain_handle.expect_promote_block().never();

    let mut mock_validator = MockDefaultShareValidator::new();
    mock_validator.expect_validate_with_chain_context().never();
    let share_validator: Arc<dyn ShareValidator + Send + Sync> = Arc::new(mock_validator);

    let (monitoring_tx, _monitoring_rx) = create_monitoring_event_channel();
    let (notify_tx, _notify_rx) = create_test_notify_channel();
    let worker = OrganiseWorker::new(
        organise_rx,
        create_validation_channel().0,
        mock_chain_handle,
        monitoring_tx,
        notify_tx,
        create_test_metrics_handle(),
        create_test_pplns_window(),
        share_validator,
    );

    let share = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    organise_tx
        .send(OrganiseEvent::InvalidBlock(share.block_hash()))
        .await
        .unwrap();
    drop(organise_tx);

    assert!(worker.run().await.is_ok());
}

#[tokio::test]
async fn test_organise_worker_fatal_on_channel_closed() {
    let (organise_tx, organise_rx) = create_organise_channel();
    let mut mock_chain_handle = MockChainStoreHandle::new();
    mock_chain_handle
        .expect_get_candidate_blocks_needing_validation()
        .returning(|_, _| Ok(Vec::new()));
    // The confirmed-block follow-up reads each newly confirmed block
    // back; this test is not about the share feed, so let it find
    // nothing and skip.
    mock_chain_handle
        .expect_get_confirmed_at_height()
        .returning(|_| Err(StoreError::NotFound("not under test".to_string())));
    mock_chain_handle
        .expect_clone()
        .return_once(MockChainStoreHandle::new);
    mock_chain_handle
        .expect_mark_block_valid()
        .returning(|_| Ok(()));
    mock_chain_handle
        .expect_get_block_metadata()
        .returning(|_| {
            Ok(BlockMetadata {
                expected_height: Some(1),
                chain_work: bitcoin::Work::from_be_bytes([0u8; 32]),
                status: Status::BlockValid,
                chain: ChainMembership::Confirmed,
            })
        });
    mock_chain_handle
        .expect_get_candidate_tip_height()
        .returning(|| Ok(Some(1)));
    mock_chain_handle
        .expect_get_tip_height()
        .returning(|| Ok(Some(1)));
    mock_chain_handle
        .expect_promote_block()
        .returning(|_| Err(StoreError::ChannelClosed));

    let (monitoring_tx, _monitoring_rx) = create_monitoring_event_channel();
    let (notify_tx, _notify_rx) = create_test_notify_channel();
    let worker = OrganiseWorker::new(
        organise_rx,
        create_validation_channel().0,
        mock_chain_handle,
        monitoring_tx,
        notify_tx,
        create_test_metrics_handle(),
        create_test_pplns_window(),
        stub_share_validator_with_success(),
    );

    let share = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    organise_tx
        .send(OrganiseEvent::Block {
            share_block: share,
            content_validated: true,
        })
        .await
        .unwrap();
    drop(organise_tx);

    let result = worker.run().await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_organise_worker_continues_on_non_fatal_error() {
    let (tx, rx) = create_organise_channel();
    let mut mock_chain_handle = MockChainStoreHandle::new();
    mock_chain_handle
        .expect_get_candidate_blocks_needing_validation()
        .returning(|_, _| Ok(Vec::new()));
    // The confirmed-block follow-up reads each newly confirmed block
    // back; this test is not about the share feed, so let it find
    // nothing and skip.
    mock_chain_handle
        .expect_get_confirmed_at_height()
        .returning(|_| Err(StoreError::NotFound("not under test".to_string())));
    mock_chain_handle
        .expect_clone()
        .return_once(MockChainStoreHandle::new);
    mock_chain_handle
        .expect_mark_block_valid()
        .returning(|_| Ok(()));
    mock_chain_handle
        .expect_get_block_metadata()
        .returning(|_| {
            Ok(BlockMetadata {
                expected_height: Some(1),
                chain_work: bitcoin::Work::from_be_bytes([0u8; 32]),
                status: Status::BlockValid,
                chain: ChainMembership::Confirmed,
            })
        });
    mock_chain_handle
        .expect_get_candidate_tip_height()
        .returning(|| Ok(Some(1)));
    mock_chain_handle
        .expect_get_tip_height()
        .returning(|| Ok(Some(1)));
    mock_chain_handle
        .expect_promote_block()
        .returning(|_| Err(StoreError::Database("test error".to_string())));

    let (monitoring_tx, _monitoring_rx) = create_monitoring_event_channel();
    let (notify_tx, _notify_rx) = create_test_notify_channel();
    let worker = OrganiseWorker::new(
        rx,
        create_validation_channel().0,
        mock_chain_handle,
        monitoring_tx,
        notify_tx,
        create_test_metrics_handle(),
        create_test_pplns_window(),
        stub_share_validator_with_success(),
    );

    let share = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    tx.send(OrganiseEvent::Block {
        share_block: share,
        content_validated: true,
    })
    .await
    .unwrap();
    drop(tx);

    // Worker should continue past the non-fatal error and exit cleanly
    let result = worker.run().await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_organise_worker_sends_new_notify_when_confirmed_catches_up() {
    let (organise_tx, organise_rx) = create_organise_channel();
    let mut mock_chain_handle = MockChainStoreHandle::new();
    mock_chain_handle
        .expect_get_candidate_blocks_needing_validation()
        .returning(|_, _| Ok(Vec::new()));
    // The confirmed-block follow-up reads each newly confirmed block
    // back; this test is not about the share feed, so let it find
    // nothing and skip.
    mock_chain_handle
        .expect_get_confirmed_at_height()
        .returning(|_| Err(StoreError::NotFound("not under test".to_string())));
    mock_chain_handle
        .expect_clone()
        .return_once(MockChainStoreHandle::new);
    mock_chain_handle
        .expect_mark_block_valid()
        .returning(|_| Ok(()));
    mock_chain_handle
        .expect_get_block_metadata()
        .returning(|_| {
            Ok(BlockMetadata {
                expected_height: Some(1),
                chain_work: bitcoin::Work::from_be_bytes([0u8; 32]),
                status: Status::BlockValid,
                chain: ChainMembership::Confirmed,
            })
        });
    mock_chain_handle
        .expect_get_tip_height()
        .returning(|| Ok(Some(1)));
    mock_chain_handle
        .expect_promote_block()
        .returning(|_| Ok(Some(5)));
    mock_chain_handle
        .expect_get_candidate_tip_height()
        .returning(|| Ok(Some(5)));
    mock_chain_handle.expect_is_current().returning(|| true);
    mock_chain_handle
        .expect_get_uncle_infos()
        .returning(|_| Vec::new());

    let (monitoring_tx, _monitoring_rx) = create_monitoring_event_channel();
    let (notify_tx, mut notify_rx) = create_test_notify_channel();
    let worker = OrganiseWorker::new(
        organise_rx,
        create_validation_channel().0,
        mock_chain_handle,
        monitoring_tx,
        notify_tx,
        create_test_metrics_handle(),
        create_test_pplns_window(),
        stub_share_validator_with_success(),
    );

    let share = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    organise_tx
        .send(OrganiseEvent::Block {
            share_block: share,
            content_validated: true,
        })
        .await
        .unwrap();
    drop(organise_tx);

    let result = worker.run().await;
    assert!(result.is_ok());

    // Verify NewNotify was sent on the notify channel
    let cmd = notify_rx.try_recv();
    assert!(cmd.is_ok());
    assert!(matches!(cmd.unwrap(), NotifyCmd::NewNotify));
}

#[tokio::test]
async fn test_organise_worker_no_new_notify_when_confirmed_below_candidate() {
    let (organise_tx, organise_rx) = create_organise_channel();
    let mut mock_chain_handle = MockChainStoreHandle::new();
    mock_chain_handle
        .expect_get_candidate_blocks_needing_validation()
        .returning(|_, _| Ok(Vec::new()));
    // The confirmed-block follow-up reads each newly confirmed block
    // back; this test is not about the share feed, so let it find
    // nothing and skip.
    mock_chain_handle
        .expect_get_confirmed_at_height()
        .returning(|_| Err(StoreError::NotFound("not under test".to_string())));
    mock_chain_handle
        .expect_clone()
        .return_once(MockChainStoreHandle::new);
    mock_chain_handle
        .expect_mark_block_valid()
        .returning(|_| Ok(()));
    mock_chain_handle
        .expect_get_block_metadata()
        .returning(|_| {
            Ok(BlockMetadata {
                expected_height: Some(1),
                chain_work: bitcoin::Work::from_be_bytes([0u8; 32]),
                status: Status::BlockValid,
                chain: ChainMembership::Confirmed,
            })
        });
    mock_chain_handle
        .expect_get_tip_height()
        .returning(|| Ok(Some(1)));
    // Confirmed height 3 is below candidate tip 5
    mock_chain_handle
        .expect_promote_block()
        .returning(|_| Ok(Some(3)));
    mock_chain_handle
        .expect_get_candidate_tip_height()
        .returning(|| Ok(Some(5)));
    mock_chain_handle.expect_is_current().returning(|| true);
    mock_chain_handle
        .expect_get_uncle_infos()
        .returning(|_| Vec::new());

    let (monitoring_tx, _monitoring_rx) = create_monitoring_event_channel();
    let (notify_tx, mut notify_rx) = create_test_notify_channel();
    let worker = OrganiseWorker::new(
        organise_rx,
        create_validation_channel().0,
        mock_chain_handle,
        monitoring_tx,
        notify_tx,
        create_test_metrics_handle(),
        create_test_pplns_window(),
        stub_share_validator_with_success(),
    );

    let share = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    organise_tx
        .send(OrganiseEvent::Block {
            share_block: share,
            content_validated: true,
        })
        .await
        .unwrap();
    drop(organise_tx);

    let result = worker.run().await;
    assert!(result.is_ok());

    // No NewNotify should have been sent
    assert!(notify_rx.try_recv().is_err());
}

#[tokio::test]
async fn test_organise_worker_buffers_block_when_parent_above_confirmed_tip() {
    let (organise_tx, organise_rx) = create_organise_channel();
    let mut mock_chain_handle = MockChainStoreHandle::new();
    mock_chain_handle
        .expect_get_candidate_blocks_needing_validation()
        .returning(|_, _| Ok(Vec::new()));
    // The confirmed-block follow-up reads each newly confirmed block
    // back; this test is not about the share feed, so let it find
    // nothing and skip.
    mock_chain_handle
        .expect_get_confirmed_at_height()
        .returning(|_| Err(StoreError::NotFound("not under test".to_string())));
    mock_chain_handle
        .expect_clone()
        .return_once(MockChainStoreHandle::new);
    mock_chain_handle
        .expect_mark_block_valid()
        .returning(|_| Ok(()));
    mock_chain_handle
        .expect_share_block_exists()
        .returning(|_| false);
    mock_chain_handle
        .expect_get_block_metadata()
        .returning(|_| {
            Ok(BlockMetadata {
                expected_height: Some(10),
                chain_work: bitcoin::Work::from_be_bytes([0u8; 32]),
                status: Status::HeaderValid,
                chain: ChainMembership::Candidate,
            })
        });
    mock_chain_handle
        .expect_get_tip_height()
        .returning(|| Ok(Some(5)));
    mock_chain_handle.expect_promote_block().never();
    mock_chain_handle
        .expect_organise_block()
        .returning(|| Ok(None));

    let (monitoring_tx, _monitoring_rx) = create_monitoring_event_channel();
    let (notify_tx, _notify_rx) = create_test_notify_channel();
    let worker = OrganiseWorker::new(
        organise_rx,
        create_validation_channel().0,
        mock_chain_handle,
        monitoring_tx,
        notify_tx,
        create_test_metrics_handle(),
        create_test_pplns_window(),
        stub_share_validator_with_success(),
    );

    let share = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    organise_tx
        .send(OrganiseEvent::Block {
            share_block: share,
            content_validated: true,
        })
        .await
        .unwrap();
    drop(organise_tx);

    let result = worker.run().await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_organise_worker_does_not_buffer_when_parent_at_confirmed_tip() {
    let (organise_tx, organise_rx) = create_organise_channel();
    let mut mock_chain_handle = MockChainStoreHandle::new();
    mock_chain_handle
        .expect_get_candidate_blocks_needing_validation()
        .returning(|_, _| Ok(Vec::new()));
    // The confirmed-block follow-up reads the block back; this test is
    // not about the share feed, so let it find nothing and skip.
    mock_chain_handle
        .expect_get_confirmed_at_height()
        .returning(|_| Err(StoreError::NotFound("not under test".to_string())));
    mock_chain_handle
        .expect_clone()
        .return_once(MockChainStoreHandle::new);
    mock_chain_handle
        .expect_mark_block_valid()
        .returning(|_| Ok(()));
    mock_chain_handle
        .expect_get_block_metadata()
        .returning(|_| {
            Ok(BlockMetadata {
                expected_height: Some(5),
                chain_work: bitcoin::Work::from_be_bytes([0u8; 32]),
                status: Status::BlockValid,
                chain: ChainMembership::Confirmed,
            })
        });
    mock_chain_handle
        .expect_get_tip_height()
        .returning(|| Ok(Some(5)));
    mock_chain_handle
        .expect_promote_block()
        .returning(|_| Ok(Some(6)));
    mock_chain_handle
        .expect_get_candidate_tip_height()
        .returning(|| Ok(Some(6)));
    mock_chain_handle.expect_is_current().returning(|| true);
    mock_chain_handle
        .expect_get_uncle_infos()
        .returning(|_| Vec::new());

    let (monitoring_tx, _monitoring_rx) = create_monitoring_event_channel();
    let (notify_tx, _notify_rx) = create_test_notify_channel();
    let worker = OrganiseWorker::new(
        organise_rx,
        create_validation_channel().0,
        mock_chain_handle,
        monitoring_tx,
        notify_tx,
        create_test_metrics_handle(),
        create_test_pplns_window(),
        stub_share_validator_with_success(),
    );

    let share = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    organise_tx
        .send(OrganiseEvent::Block {
            share_block: share,
            content_validated: true,
        })
        .await
        .unwrap();
    drop(organise_tx);

    let result = worker.run().await;
    assert!(result.is_ok());
}

/// A promotion that confirms several blocks at once reports every one of
/// them, each at its own height.
///
/// `promote_block` returns the new confirmed *tip*, not the height of the
/// block that triggered it. Reporting once, for the triggering share, at
/// that tip height published the wrong height for it and left the rest of
/// the confirmed prefix out of the share feed and the effort numerator
/// entirely. A prefix confirms at once whenever confirmation was stalled --
/// blocks validated while a lower one still lacked its body -- and then
/// catches up.
#[tokio::test]
async fn test_confirming_a_prefix_reports_every_block_at_its_own_height() {
    let (_organise_tx, organise_rx) = create_organise_channel();
    let mut mock_chain_handle = MockChainStoreHandle::new();
    mock_chain_handle
        .expect_get_candidate_blocks_needing_validation()
        .returning(|_, _| Ok(Vec::new()));

    // Blocks 5, 6 and 7 all confirm in the promotion triggered by block 5.
    let confirmed: Vec<ShareBlock> = (5..=7u32)
        .map(|height| {
            TestShareBlockBuilder::new()
                .nonce(0xe9690000 + height)
                .build()
        })
        .collect();
    let by_height: HashMap<u32, ShareBlock> = (5..=7u32).zip(confirmed.iter().cloned()).collect();
    let by_hash: HashMap<BlockHash, ShareBlock> = confirmed
        .iter()
        .map(|share| (share.block_hash(), share.clone()))
        .collect();

    mock_chain_handle
        .expect_get_block_metadata()
        .returning(|_| {
            Ok(BlockMetadata {
                expected_height: Some(4),
                chain_work: bitcoin::Work::from_be_bytes([0u8; 32]),
                status: Status::BlockValid,
                chain: ChainMembership::Confirmed,
            })
        });
    mock_chain_handle
        .expect_get_candidate_tip_height()
        .returning(|| Ok(Some(7)));
    // Confirmed tip is 4 before the promotion and 7 after it.
    mock_chain_handle
        .expect_get_tip_height()
        .returning(|| Ok(Some(4)));
    mock_chain_handle
        .expect_mark_block_valid()
        .returning(|_| Ok(()));
    mock_chain_handle
        .expect_promote_block()
        .returning(|_| Ok(Some(7)));
    mock_chain_handle
        .expect_get_confirmed_at_height()
        .returning(move |height| Ok(by_height[&height].block_hash()));
    mock_chain_handle
        .expect_get_share_header()
        .returning(move |hash| Ok(by_hash[hash].header.clone()));
    mock_chain_handle.expect_is_current().returning(|| true);
    mock_chain_handle
        .expect_get_uncle_infos()
        .returning(|_| Vec::new());

    let (monitoring_tx, mut monitoring_rx) = create_monitoring_event_channel();
    let (notify_tx, _notify_rx) = create_test_notify_channel();
    let mut worker = OrganiseWorker::new(
        organise_rx,
        create_validation_channel().0,
        mock_chain_handle,
        monitoring_tx,
        notify_tx,
        create_test_metrics_handle(),
        create_test_pplns_window(),
        stub_share_validator_with_success(),
    );

    worker
        .process_share_block(confirmed[0].clone(), true)
        .await
        .unwrap();

    let mut reported = Vec::new();
    while let Ok(MonitoringEvent::Share(share_info)) = monitoring_rx.try_recv() {
        reported.push((share_info.blockhash, share_info.height));
    }

    let expected: Vec<(BlockHash, u32)> = (5..=7u32)
        .map(|height| (by_height_hash(&confirmed, height), height))
        .collect();
    assert_eq!(
        reported, expected,
        "every block the promotion confirmed must be reported at its own height"
    );
}

/// The block confirmed at `height` in the fixture above.
fn by_height_hash(confirmed: &[ShareBlock], height: u32) -> BlockHash {
    confirmed[(height - 5) as usize].block_hash()
}

#[tokio::test]
async fn test_confirmed_block_skips_effort_while_syncing() {
    let (organise_tx, organise_rx) = create_organise_channel();
    let mut mock_chain_handle = MockChainStoreHandle::new();
    mock_chain_handle
        .expect_get_candidate_blocks_needing_validation()
        .returning(|_, _| Ok(Vec::new()));
    let share = TestShareBlockBuilder::new().nonce(0xe9695791).build();

    // This test is about the follow-up, so the newly confirmed block has
    // to be readable back.
    let share_hash = share.block_hash();
    let share_header = share.header.clone();
    mock_chain_handle
        .expect_get_confirmed_at_height()
        .returning(move |_| Ok(share_hash));
    mock_chain_handle
        .expect_get_share_header()
        .returning(move |_| Ok(share_header.clone()));
    mock_chain_handle
        .expect_clone()
        .return_once(MockChainStoreHandle::new);
    mock_chain_handle
        .expect_mark_block_valid()
        .returning(|_| Ok(()));
    mock_chain_handle
        .expect_get_block_metadata()
        .returning(|_| {
            Ok(BlockMetadata {
                expected_height: Some(5),
                chain_work: bitcoin::Work::from_be_bytes([0u8; 32]),
                status: Status::BlockValid,
                chain: ChainMembership::Confirmed,
            })
        });
    mock_chain_handle
        .expect_get_tip_height()
        .returning(|| Ok(Some(5)));
    mock_chain_handle
        .expect_promote_block()
        .returning(|_| Ok(Some(6)));
    mock_chain_handle
        .expect_get_candidate_tip_height()
        .returning(|| Ok(Some(6)));
    // Not current: the node is still syncing, so effort must not be recorded.
    mock_chain_handle.expect_is_current().returning(|| false);
    mock_chain_handle
        .expect_get_uncle_infos()
        .returning(|_| Vec::new());

    let (monitoring_tx, _monitoring_rx) = create_monitoring_event_channel();
    let (notify_tx, _notify_rx) = create_test_notify_channel();
    // Hold a metrics handle so we can inspect it after the worker runs.
    let metrics = create_test_metrics_handle();
    let worker = OrganiseWorker::new(
        organise_rx,
        create_validation_channel().0,
        mock_chain_handle,
        monitoring_tx,
        notify_tx,
        metrics.clone(),
        create_test_pplns_window(),
        stub_share_validator_with_success(),
    );

    organise_tx
        .send(OrganiseEvent::Block {
            share_block: share,
            content_validated: true,
        })
        .await
        .unwrap();
    drop(organise_tx);

    let result = worker.run().await;
    assert!(result.is_ok());

    // The block was promoted, but because the chain was not current no
    // confirmed-share work was accumulated into the effort numerator.
    let pool_metrics = metrics.get_metrics().await;
    assert_eq!(pool_metrics.work_since_last_block, 0.0);
}

#[tokio::test]
async fn test_confirmed_bitcoin_block_resets_block_effort() {
    let (organise_tx, organise_rx) = create_organise_channel();
    let mut mock_chain_handle = MockChainStoreHandle::new();
    mock_chain_handle
        .expect_get_candidate_blocks_needing_validation()
        .returning(|_, _| Ok(Vec::new()));
    let mut share = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    share.header.bitcoin_header =
        bitcoin::blockdata::constants::genesis_block(bitcoin::Network::Regtest).header;

    // This test is about the follow-up, so the newly confirmed block has
    // to be readable back.
    let share_hash = share.block_hash();
    let share_header = share.header.clone();
    mock_chain_handle
        .expect_get_confirmed_at_height()
        .returning(move |_| Ok(share_hash));
    mock_chain_handle
        .expect_get_share_header()
        .returning(move |_| Ok(share_header.clone()));
    mock_chain_handle
        .expect_clone()
        .return_once(MockChainStoreHandle::new);
    mock_chain_handle
        .expect_mark_block_valid()
        .returning(|_| Ok(()));
    mock_chain_handle
        .expect_get_block_metadata()
        .returning(|_| {
            Ok(BlockMetadata {
                expected_height: Some(5),
                chain_work: bitcoin::Work::from_be_bytes([0u8; 32]),
                status: Status::BlockValid,
                chain: ChainMembership::Confirmed,
            })
        });
    mock_chain_handle
        .expect_get_tip_height()
        .returning(|| Ok(Some(5)));
    mock_chain_handle
        .expect_promote_block()
        .returning(|_| Ok(Some(6)));
    mock_chain_handle
        .expect_get_candidate_tip_height()
        .returning(|| Ok(Some(6)));
    mock_chain_handle.expect_is_current().returning(|| true);
    mock_chain_handle
        .expect_get_uncle_infos()
        .returning(|_| Vec::new());

    let (monitoring_tx, _monitoring_rx) = create_monitoring_event_channel();
    let (notify_tx, _notify_rx) = create_test_notify_channel();
    let metrics = create_test_metrics_handle();
    let worker = OrganiseWorker::new(
        organise_rx,
        create_validation_channel().0,
        mock_chain_handle,
        monitoring_tx,
        notify_tx,
        metrics.clone(),
        create_test_pplns_window(),
        stub_share_validator_with_success(),
    );

    // Make the confirmed share a real bitcoin block: the regtest genesis
    // header meets its own target, so is_bitcoin_block() is true.
    organise_tx
        .send(OrganiseEvent::Block {
            share_block: share,
            content_validated: true,
        })
        .await
        .unwrap();
    drop(organise_tx);

    let result = worker.run().await;
    assert!(result.is_ok());

    // A pool-wide block find from the share chain resets the effort
    // accumulator.
    let pool_metrics = metrics.get_metrics().await;
    assert_eq!(pool_metrics.work_since_last_block, 0.0);
}

#[tokio::test]
async fn test_organise_worker_drains_buffered_block_after_promotion() {
    let (organise_tx, organise_rx) = create_organise_channel();
    let mut mock_chain_handle = MockChainStoreHandle::new();
    mock_chain_handle
        .expect_get_candidate_blocks_needing_validation()
        .returning(|_, _| Ok(Vec::new()));
    // The confirmed-block follow-up reads the block back; this test is
    // not about the share feed, so let it find nothing and skip.
    mock_chain_handle
        .expect_get_confirmed_at_height()
        .returning(|_| Err(StoreError::NotFound("not under test".to_string())));
    mock_chain_handle
        .expect_clone()
        .return_once(MockChainStoreHandle::new);
    mock_chain_handle
        .expect_mark_block_valid()
        .returning(|_| Ok(()));

    // First call: block A has parent at height 100, tip is 5 -> buffer.
    // Second call: block B has parent at height 5, tip is 5 -> proceed.
    // Third call (during drain): block A re-checked, parent at 100 still.
    let metadata_call_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let metadata_counter = metadata_call_count.clone();
    mock_chain_handle
        .expect_get_block_metadata()
        .returning(move |_| {
            let count = metadata_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let height = if count == 0 { 100 } else { 5 };
            Ok(BlockMetadata {
                expected_height: Some(height),
                chain_work: bitcoin::Work::from_be_bytes([0u8; 32]),
                status: Status::BlockValid,
                chain: ChainMembership::Confirmed,
            })
        });

    // First call (should_buffer for block A): tip is 5 -> buffer.
    // Second call (should_buffer for block B): tip is 5 -> proceed.
    // Third call (drain after B promotes): tip is 100 -> drain A.
    // Fourth call (drain loop re-check): tip is 100 -> empty, stop.
    let tip_call_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let tip_counter = tip_call_count.clone();
    mock_chain_handle
        .expect_get_tip_height()
        .returning(move || {
            let count = tip_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let tip = if count < 2 { 5 } else { 100 };
            Ok(Some(tip))
        });

    mock_chain_handle
        .expect_organise_block()
        .returning(|| Ok(None));
    mock_chain_handle
        .expect_promote_block()
        .returning(|_| Ok(Some(100)));
    mock_chain_handle
        .expect_get_candidate_tip_height()
        .returning(|| Ok(Some(100)));
    mock_chain_handle.expect_is_current().returning(|| true);
    mock_chain_handle
        .expect_get_uncle_infos()
        .returning(|_| Vec::new());

    let (monitoring_tx, _monitoring_rx) = create_monitoring_event_channel();
    let (notify_tx, _notify_rx) = create_test_notify_channel();
    let worker = OrganiseWorker::new(
        organise_rx,
        create_validation_channel().0,
        mock_chain_handle,
        monitoring_tx,
        notify_tx,
        create_test_metrics_handle(),
        create_test_pplns_window(),
        stub_share_validator_with_success(),
    );

    // Block A: parent height 100, will be buffered
    let share_a = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    organise_tx
        .send(OrganiseEvent::Block {
            share_block: share_a,
            content_validated: true,
        })
        .await
        .unwrap();

    // Block B: parent height 5, will proceed and promote
    let share_b = TestShareBlockBuilder::new().nonce(0xe9695792).build();
    organise_tx
        .send(OrganiseEvent::Block {
            share_block: share_b,
            content_validated: true,
        })
        .await
        .unwrap();
    drop(organise_tx);

    let result = worker.run().await;
    assert!(result.is_ok());

    // promote_block was called at least twice: once for B, once for
    // drained A. The mock allows unlimited calls.
}

/// A transient failure after the parent gate must leave the block buffered
/// for a later drain. Dropping it loses a block that is already stored, so
/// nothing re-fetches it and its children wait on a parent that will never
/// be marked BlockValid.
/// End to end for the wedge: a block fails transiently, is re-buffered
/// under its parent's height, and is retried when a later event finds
/// confirmation stalled. Without the retry the confirmed tip cannot move
/// -- this block is what blocks it -- so no confirm-driven drain ever
/// reaches it and the node stays wedged until a peer happens to resend.
#[tokio::test]
async fn test_stalled_confirmation_retries_the_block_wedging_it() {
    let (_organise_tx, organise_rx) = create_organise_channel();
    let mut mock_chain_handle = MockChainStoreHandle::new();
    mock_chain_handle
        .expect_get_candidate_blocks_needing_validation()
        .returning(|_, _| Ok(Vec::new()));
    // The confirmed-block follow-up reads each newly confirmed block
    // back; this test is not about the share feed, so let it find
    // nothing and skip.
    mock_chain_handle
        .expect_get_confirmed_at_height()
        .returning(|_| Err(StoreError::NotFound("not under test".to_string())));

    // Parent is confirmed at height 5, so the block reaches promotion and
    // is buffered under height 5 when that fails.
    mock_chain_handle
        .expect_get_block_metadata()
        .returning(|_| {
            Ok(BlockMetadata {
                expected_height: Some(5),
                chain_work: bitcoin::Work::from_be_bytes([0u8; 32]),
                status: Status::BlockValid,
                chain: ChainMembership::Confirmed,
            })
        });
    mock_chain_handle
        .expect_get_candidate_tip_height()
        .returning(|| Ok(Some(6)));
    mock_chain_handle
        .expect_mark_block_valid()
        .returning(|_| Ok(()));
    // The confirmed tip stays at 5: this block is what stops it advancing.
    mock_chain_handle
        .expect_get_tip_height()
        .returning(|| Ok(Some(5)));
    mock_chain_handle
        .expect_organise_block()
        .returning(|| Ok(None));
    mock_chain_handle.expect_is_current().returning(|| false);
    mock_chain_handle
        .expect_get_uncle_infos()
        .returning(|_| Vec::new());

    // Promotion fails once, then succeeds -- a transient store failure.
    let promote_attempts = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let attempts = promote_attempts.clone();
    mock_chain_handle
        .expect_promote_block()
        .returning(move |_| {
            if attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                Err(StoreError::Database("transient".to_string()))
            } else {
                Ok(Some(6))
            }
        });

    let (monitoring_tx, _monitoring_rx) = create_monitoring_event_channel();
    let (notify_tx, _notify_rx) = create_test_notify_channel();
    let mut worker = OrganiseWorker::new(
        organise_rx,
        create_validation_channel().0,
        mock_chain_handle,
        monitoring_tx,
        notify_tx,
        create_test_metrics_handle(),
        create_test_pplns_window(),
        stub_share_validator_with_success(),
    );

    let share = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let outcome = worker.process_share_block(share, true).await.unwrap();
    assert!(matches!(outcome, ProcessOutcome::NotAdvanced));
    assert_eq!(worker.pending_blocks.get(&5).map(Vec::len), Some(1));

    // A later event finds confirmation stalled and retries the wedging
    // block, which now promotes.
    worker.advance_confirmed_and_drain().await.unwrap();

    assert_eq!(
        promote_attempts.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "the wedging block must be re-attempted"
    );
    assert!(
        worker.pending_blocks.is_empty(),
        "a block that advances must leave the buffer"
    );
}

/// One block delivered repeatedly takes one buffer slot, not one per
/// delivery.
///
/// `handle_share_block` re-queues an already-stored block for validation
/// each time a peer sends it, and `validate_share_block` only
/// short-circuits on `BlockValid`, so a `HeaderValid` block is re-validated
/// and re-emitted every time. Undeduped, replaying one observed share fills
/// the buffer and `buffer_block` starts dropping live blocks -- which are
/// already stored with their bodies, so no missing-data scan re-requests
/// them and confirmation stops at their height.
#[tokio::test]
async fn test_repeated_delivery_takes_one_buffer_slot() {
    const DELIVERIES: usize = 16;

    let (_organise_tx, organise_rx) = create_organise_channel();
    let mut mock_chain_handle = MockChainStoreHandle::new();
    mock_chain_handle
        .expect_get_candidate_blocks_needing_validation()
        .returning(|_, _| Ok(Vec::new()));
    // The confirmed-block follow-up reads each newly confirmed block
    // back; this test is not about the share feed, so let it find
    // nothing and skip.
    mock_chain_handle
        .expect_get_confirmed_at_height()
        .returning(|_| Err(StoreError::NotFound("not under test".to_string())));
    mock_chain_handle
        .expect_get_tip_height()
        .returning(|| Ok(None));
    // Parent is stored but only HeaderValid, so the block is buffered.
    mock_chain_handle
        .expect_share_block_exists()
        .returning(|_| false);
    mock_chain_handle
        .expect_get_block_metadata()
        .returning(|_| {
            Ok(BlockMetadata {
                expected_height: Some(5),
                chain_work: bitcoin::Work::from_be_bytes([0u8; 32]),
                status: Status::HeaderValid,
                chain: ChainMembership::Candidate,
            })
        });

    let (monitoring_tx, _monitoring_rx) = create_monitoring_event_channel();
    let (notify_tx, _notify_rx) = create_test_notify_channel();
    let mut worker = OrganiseWorker::new(
        organise_rx,
        create_validation_channel().0,
        mock_chain_handle,
        monitoring_tx,
        notify_tx,
        create_test_metrics_handle(),
        create_test_pplns_window(),
        stub_share_validator_with_success(),
    );

    let share = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    for _ in 0..DELIVERIES {
        let outcome = worker
            .process_share_block(share.clone(), true)
            .await
            .unwrap();
        assert!(matches!(outcome, ProcessOutcome::Buffered));
    }

    let buffered = worker
        .pending_blocks
        .get(&5)
        .expect("the block must be buffered under its parent height");
    assert_eq!(
        buffered.len(),
        1,
        "every delivery of the same block must share one slot"
    );
    assert_eq!(buffered[0].block_hash, share.block_hash());
}

/// A worker whose every parent is `HeaderValid` without a body, so every
/// block it processes is buffered under parent height 1, and whose pending
/// buffer has `limits`.
fn worker_buffering_everything(limits: BufferLimits) -> OrganiseWorker {
    let mut worker = worker_with_parent_metadata(
        Ok(parent_metadata(
            Status::HeaderValid,
            ChainMembership::Candidate,
        )),
        false,
    );
    worker.pending_blocks = PendingBuffer::new(limits);
    worker
}

/// Past its block bound the buffer evicts its oldest block rather than
/// turning the new one away, so blocks stuck behind a parent that never
/// validates cannot pin it. The evicted block stays stored, to be
/// re-validated as a stranded parent when a descendant arrives.
#[tokio::test]
async fn test_full_buffer_evicts_its_oldest_block() {
    let mut worker = worker_buffering_everything(BufferLimits {
        max_blocks: 2,
        max_bytes: usize::MAX,
    });
    let shares: Vec<ShareBlock> = (0..3)
        .map(|nonce| TestShareBlockBuilder::new().nonce(nonce).build())
        .collect();

    for share in &shares {
        let outcome = worker
            .process_share_block(share.clone(), true)
            .await
            .unwrap();
        assert!(matches!(outcome, ProcessOutcome::Buffered));
    }

    let buffered: Vec<BlockHash> = worker
        .pending_blocks
        .get(&1)
        .unwrap()
        .iter()
        .map(|entry| entry.block_hash)
        .collect();
    assert_eq!(
        buffered,
        vec![shares[1].block_hash(), shares[2].block_hash()]
    );
}

/// The byte bound evicts the same way, before the block bound is reached.
#[tokio::test]
async fn test_buffer_past_its_byte_bound_evicts_its_oldest_block() {
    let shares: Vec<ShareBlock> = (0..3)
        .map(|nonce| TestShareBlockBuilder::new().nonce(nonce).build())
        .collect();
    let size = shares[0].encoded_size();
    let mut worker = worker_buffering_everything(BufferLimits {
        max_blocks: PENDING_BLOCKS_CAPACITY,
        max_bytes: 2 * size + size / 2,
    });

    for share in &shares {
        worker
            .process_share_block(share.clone(), true)
            .await
            .unwrap();
    }

    assert_eq!(worker.pending_blocks.len(), 2);
    assert!(worker.pending_blocks.bytes() <= 2 * size + size / 2);
    assert!(
        worker
            .pending_blocks
            .get(&1)
            .unwrap()
            .iter()
            .all(|entry| entry.block_hash != shares[0].block_hash())
    );
}

#[tokio::test]
async fn test_transient_promote_failure_rebuffers_the_block() {
    let (_organise_tx, organise_rx) = create_organise_channel();
    let mut mock_chain_handle = MockChainStoreHandle::new();
    mock_chain_handle
        .expect_get_candidate_blocks_needing_validation()
        .returning(|_, _| Ok(Vec::new()));
    // The confirmed-block follow-up reads each newly confirmed block
    // back; this test is not about the share feed, so let it find
    // nothing and skip.
    mock_chain_handle
        .expect_get_confirmed_at_height()
        .returning(|_| Err(StoreError::NotFound("not under test".to_string())));
    mock_chain_handle
        .expect_get_tip_height()
        .returning(|| Ok(None));

    // Parent is confirmed at height 5, so the block passes the parent gate
    // and reaches promotion.
    mock_chain_handle
        .expect_get_block_metadata()
        .returning(|_| {
            Ok(BlockMetadata {
                expected_height: Some(5),
                chain_work: bitcoin::Work::from_be_bytes([0u8; 32]),
                status: Status::BlockValid,
                chain: ChainMembership::Confirmed,
            })
        });
    mock_chain_handle
        .expect_get_candidate_tip_height()
        .returning(|| Ok(Some(6)));
    mock_chain_handle
        .expect_mark_block_valid()
        .returning(|_| Ok(()));
    // A store failure that says nothing about the block itself.
    mock_chain_handle
        .expect_promote_block()
        .returning(|_| Err(StoreError::Database("transient".to_string())));

    let (monitoring_tx, _monitoring_rx) = create_monitoring_event_channel();
    let (notify_tx, _notify_rx) = create_test_notify_channel();
    let mut worker = OrganiseWorker::new(
        organise_rx,
        create_validation_channel().0,
        mock_chain_handle,
        monitoring_tx,
        notify_tx,
        create_test_metrics_handle(),
        create_test_pplns_window(),
        stub_share_validator_with_success(),
    );

    let share = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let blockhash = share.block_hash();
    let outcome = worker.process_share_block(share, true).await.unwrap();

    assert!(matches!(outcome, ProcessOutcome::NotAdvanced));
    let buffered = worker
        .pending_blocks
        .get(&5)
        .expect("block must be buffered under its parent height for retry");
    assert_eq!(buffered.len(), 1);
    assert_eq!(buffered[0].share_block.block_hash(), blockhash);
}

/// Build a worker whose parent gate passes at height 5 and whose candidate
/// tip puts the block inside the PPLNS zone, driving `validate_mark_promote`
/// with the given validator.
fn worker_in_pplns_zone(share_validator: Arc<dyn ShareValidator + Send + Sync>) -> OrganiseWorker {
    let mut mock_chain_handle = MockChainStoreHandle::new();
    mock_chain_handle
        .expect_get_candidate_blocks_needing_validation()
        .returning(|_, _| Ok(Vec::new()));
    mock_chain_handle
        .expect_get_tip_height()
        .returning(|| Ok(None));
    // The confirmed-block follow-up reads the block back; this test is
    // not about the share feed, so let it find nothing and skip.
    mock_chain_handle
        .expect_get_confirmed_at_height()
        .returning(|_| Err(StoreError::NotFound("not under test".to_string())));
    mock_chain_handle
        .expect_get_block_metadata()
        .returning(|_| {
            Ok(BlockMetadata {
                expected_height: Some(5),
                chain_work: bitcoin::Work::from_be_bytes([0u8; 32]),
                status: Status::BlockValid,
                chain: ChainMembership::Confirmed,
            })
        });
    mock_chain_handle
        .expect_get_candidate_tip_height()
        .returning(|| Ok(Some(6)));
    mock_chain_handle
        .expect_mark_block_valid()
        .returning(|_| Ok(()));
    mock_chain_handle
        .expect_promote_block()
        .returning(|_| Ok(None));

    let (_organise_tx, organise_rx) = create_organise_channel();
    let (monitoring_tx, _monitoring_rx) = create_monitoring_event_channel();
    let (notify_tx, _notify_rx) = create_test_notify_channel();
    OrganiseWorker::new(
        organise_rx,
        create_validation_channel().0,
        mock_chain_handle,
        monitoring_tx,
        notify_tx,
        create_test_metrics_handle(),
        create_test_pplns_window(),
        share_validator,
    )
}

/// A block the validation worker tiered below the PPLNS zone, but which is
/// back inside the zone by the time the organise worker sees it, has its
/// skipped content checks run before it can be marked BlockValid.
///
/// The candidate tip can shorten between the two `check_pplns_zone` reads --
/// a reorg onto a shorter branch, or a rebuild after an invalidation -- so
/// re-deriving the tier here would confirm a block whose merkle root,
/// coinbase, witness commitment and scripts were never checked.
#[tokio::test]
async fn test_below_zone_block_back_in_zone_runs_the_skipped_content_checks() {
    let mut mock_validator = MockDefaultShareValidator::new();
    mock_validator
        .expect_validate_share_block()
        .times(1)
        .returning(|_, _| Ok(()));
    mock_validator
        .expect_validate_with_chain_context()
        .times(1)
        .returning(|_, _, _| Ok(()));

    let mut worker = worker_in_pplns_zone(Arc::new(mock_validator));
    let share = TestShareBlockBuilder::new().nonce(0xe9695791).build();

    // content_validated = false: stage 1 ran validate_below_pplns_depth.
    let outcome = worker.process_share_block(share, false).await.unwrap();
    assert!(matches!(outcome, ProcessOutcome::Advanced(_)));
}

/// A block stage 1 already content-validated is not validated twice.
#[tokio::test]
async fn test_content_validated_block_skips_the_full_content_checks() {
    let mut mock_validator = MockDefaultShareValidator::new();
    mock_validator.expect_validate_share_block().never();
    mock_validator
        .expect_validate_with_chain_context()
        .times(1)
        .returning(|_, _, _| Ok(()));

    let mut worker = worker_in_pplns_zone(Arc::new(mock_validator));
    let share = TestShareBlockBuilder::new().nonce(0xe9695791).build();

    let outcome = worker.process_share_block(share, true).await.unwrap();
    assert!(matches!(outcome, ProcessOutcome::Advanced(_)));
}

/// A failed mark_invalid must not be reported as an invalidation.
///
/// The whole batch is dropped uncommitted, so the block keeps its
/// HeaderValid status and Candidate membership and confirmation still stops
/// at its height. Returning Invalidated would send the caller to advance
/// onto a rebuilt candidate chain that was never written, leaving the
/// height wedged with one error line. The block is re-buffered so the
/// verdict is re-attempted.
#[tokio::test]
async fn test_failed_mark_invalid_rebuffers_instead_of_reporting_invalidated() {
    let (_organise_tx, organise_rx) = create_organise_channel();
    let mut mock_chain_handle = MockChainStoreHandle::new();
    mock_chain_handle
        .expect_get_candidate_blocks_needing_validation()
        .returning(|_, _| Ok(Vec::new()));
    // The confirmed-block follow-up reads each newly confirmed block
    // back; this test is not about the share feed, so let it find
    // nothing and skip.
    mock_chain_handle
        .expect_get_confirmed_at_height()
        .returning(|_| Err(StoreError::NotFound("not under test".to_string())));
    mock_chain_handle
        .expect_get_tip_height()
        .returning(|| Ok(None));
    mock_chain_handle
        .expect_get_block_metadata()
        .returning(|_| {
            Ok(BlockMetadata {
                expected_height: Some(5),
                chain_work: bitcoin::Work::from_be_bytes([0u8; 32]),
                status: Status::BlockValid,
                chain: ChainMembership::Confirmed,
            })
        });
    mock_chain_handle
        .expect_get_candidate_tip_height()
        .returning(|| Ok(Some(6)));
    mock_chain_handle
        .expect_mark_invalid()
        .times(1)
        .returning(|_| Err(StoreError::Database("transient".to_string())));
    mock_chain_handle.expect_mark_block_valid().never();
    mock_chain_handle.expect_promote_block().never();

    let mut mock_validator = MockDefaultShareValidator::new();
    mock_validator
        .expect_validate_with_chain_context()
        .returning(|_, _, _| Err(ValidationError::consensus("rejected for test")));

    let (monitoring_tx, _monitoring_rx) = create_monitoring_event_channel();
    let (notify_tx, _notify_rx) = create_test_notify_channel();
    let mut worker = OrganiseWorker::new(
        organise_rx,
        create_validation_channel().0,
        mock_chain_handle,
        monitoring_tx,
        notify_tx,
        create_test_metrics_handle(),
        create_test_pplns_window(),
        Arc::new(mock_validator),
    );

    let share = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let blockhash = share.block_hash();
    let outcome = worker.process_share_block(share, true).await.unwrap();

    assert!(
        matches!(outcome, ProcessOutcome::NotAdvanced),
        "an uncommitted invalidation must not be reported as Invalidated"
    );
    let buffered = worker
        .pending_blocks
        .get(&5)
        .expect("the block must be re-buffered so the verdict is retried");
    assert_eq!(buffered[0].share_block.block_hash(), blockhash);
}

/// A failed mark_invalid on the InvalidBlock event path skips the confirmed
/// chain follow-up: nothing was committed, so there is no rebuilt candidate
/// chain to advance onto.
#[tokio::test]
async fn test_failed_mark_invalid_event_does_not_advance_confirmed() {
    let (_organise_tx, organise_rx) = create_organise_channel();
    let mut mock_chain_handle = MockChainStoreHandle::new();
    mock_chain_handle
        .expect_get_candidate_blocks_needing_validation()
        .returning(|_, _| Ok(Vec::new()));
    // The confirmed-block follow-up reads each newly confirmed block
    // back; this test is not about the share feed, so let it find
    // nothing and skip.
    mock_chain_handle
        .expect_get_confirmed_at_height()
        .returning(|_| Err(StoreError::NotFound("not under test".to_string())));
    mock_chain_handle
        .expect_get_tip_height()
        .returning(|| Ok(None));
    mock_chain_handle
        .expect_mark_invalid()
        .times(1)
        .returning(|_| Err(StoreError::Database("transient".to_string())));
    mock_chain_handle.expect_organise_block().never();

    let (monitoring_tx, _monitoring_rx) = create_monitoring_event_channel();
    let (notify_tx, _notify_rx) = create_test_notify_channel();
    let mut worker = OrganiseWorker::new(
        organise_rx,
        create_validation_channel().0,
        mock_chain_handle,
        monitoring_tx,
        notify_tx,
        create_test_metrics_handle(),
        create_test_pplns_window(),
        stub_share_validator_with_success(),
    );

    let blockhash = TestShareBlockBuilder::new()
        .nonce(0xe9695791)
        .build()
        .block_hash();
    worker.handle_invalid_block_event(blockhash).await.unwrap();
}

/// A closed store writer channel is fatal, not a retry.
#[tokio::test]
async fn test_mark_invalid_channel_closed_is_fatal() {
    let (_organise_tx, organise_rx) = create_organise_channel();
    let mut mock_chain_handle = MockChainStoreHandle::new();
    mock_chain_handle
        .expect_get_candidate_blocks_needing_validation()
        .returning(|_, _| Ok(Vec::new()));
    // The confirmed-block follow-up reads each newly confirmed block
    // back; this test is not about the share feed, so let it find
    // nothing and skip.
    mock_chain_handle
        .expect_get_confirmed_at_height()
        .returning(|_| Err(StoreError::NotFound("not under test".to_string())));
    mock_chain_handle
        .expect_get_tip_height()
        .returning(|| Ok(None));
    mock_chain_handle
        .expect_mark_invalid()
        .returning(|_| Err(StoreError::ChannelClosed));
    mock_chain_handle.expect_organise_block().never();

    let (monitoring_tx, _monitoring_rx) = create_monitoring_event_channel();
    let (notify_tx, _notify_rx) = create_test_notify_channel();
    let mut worker = OrganiseWorker::new(
        organise_rx,
        create_validation_channel().0,
        mock_chain_handle,
        monitoring_tx,
        notify_tx,
        create_test_metrics_handle(),
        create_test_pplns_window(),
        stub_share_validator_with_success(),
    );

    let blockhash = TestShareBlockBuilder::new()
        .nonce(0xe9695791)
        .build()
        .block_hash();
    assert!(worker.handle_invalid_block_event(blockhash).await.is_err());
}

/// An unresolvable chain-context failure drops the block instead of
/// buffering it, and records no verdict.
///
/// The buffer is keyed by parent height and drains only reach heights at or
/// above the confirmed tip, so an entry whose parent is far below is never
/// revisited: it would hold its slot until the buffer starts dropping live
/// blocks. Marking it Invalid is not an option either -- a node retaining
/// more of its window may judge the same block valid, and the verdict would
/// bar its branch forever.
#[tokio::test]
async fn test_unresolvable_validation_error_drops_without_buffering() {
    let (_organise_tx, organise_rx) = create_organise_channel();
    let mut mock_chain_handle = MockChainStoreHandle::new();
    mock_chain_handle
        .expect_get_candidate_blocks_needing_validation()
        .returning(|_, _| Ok(Vec::new()));
    // The confirmed-block follow-up reads each newly confirmed block
    // back; this test is not about the share feed, so let it find
    // nothing and skip.
    mock_chain_handle
        .expect_get_confirmed_at_height()
        .returning(|_| Err(StoreError::NotFound("not under test".to_string())));
    mock_chain_handle
        .expect_get_tip_height()
        .returning(|| Ok(None));

    mock_chain_handle
        .expect_get_block_metadata()
        .returning(|_| {
            Ok(BlockMetadata {
                expected_height: Some(5),
                chain_work: bitcoin::Work::from_be_bytes([0u8; 32]),
                status: Status::BlockValid,
                chain: ChainMembership::Confirmed,
            })
        });
    mock_chain_handle
        .expect_get_candidate_tip_height()
        .returning(|| Ok(Some(6)));
    // Neither a verdict nor a promotion may follow a drop.
    mock_chain_handle.expect_mark_invalid().never();
    mock_chain_handle.expect_mark_block_valid().never();
    mock_chain_handle.expect_promote_block().never();

    let mut mock_validator = MockDefaultShareValidator::new();
    mock_validator
        .expect_validate_with_chain_context()
        .returning(|_, _, _| {
            Err(ValidationError::unresolvable(
                "PPLNS window for anchor is truncated by eviction",
            ))
        });

    let (monitoring_tx, _monitoring_rx) = create_monitoring_event_channel();
    let (notify_tx, _notify_rx) = create_test_notify_channel();
    let mut worker = OrganiseWorker::new(
        organise_rx,
        create_validation_channel().0,
        mock_chain_handle,
        monitoring_tx,
        notify_tx,
        create_test_metrics_handle(),
        create_test_pplns_window(),
        Arc::new(mock_validator),
    );

    let share = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let outcome = worker.process_share_block(share, true).await.unwrap();

    assert!(matches!(outcome, ProcessOutcome::Dropped));
    assert!(
        worker.pending_blocks.is_empty(),
        "an unresolvable block must not occupy a buffer slot it can never leave"
    );
}

/// A rejected status transition means the block is Pending or already
/// Invalid. No retry can change that, so it must not be re-buffered --
/// otherwise it is re-attempted on every drain of its parent's height.
#[tokio::test]
async fn test_invalid_status_transition_does_not_rebuffer() {
    let (_organise_tx, organise_rx) = create_organise_channel();
    let mut mock_chain_handle = MockChainStoreHandle::new();
    mock_chain_handle
        .expect_get_candidate_blocks_needing_validation()
        .returning(|_, _| Ok(Vec::new()));
    // The confirmed-block follow-up reads each newly confirmed block
    // back; this test is not about the share feed, so let it find
    // nothing and skip.
    mock_chain_handle
        .expect_get_confirmed_at_height()
        .returning(|_| Err(StoreError::NotFound("not under test".to_string())));
    mock_chain_handle
        .expect_get_tip_height()
        .returning(|| Ok(None));

    mock_chain_handle
        .expect_get_block_metadata()
        .returning(|_| {
            Ok(BlockMetadata {
                expected_height: Some(5),
                chain_work: bitcoin::Work::from_be_bytes([0u8; 32]),
                status: Status::BlockValid,
                chain: ChainMembership::Confirmed,
            })
        });
    mock_chain_handle
        .expect_get_candidate_tip_height()
        .returning(|| Ok(Some(6)));
    mock_chain_handle.expect_mark_block_valid().returning(|_| {
        Err(StoreError::InvalidStatusTransition(
            "block is Invalid".to_string(),
        ))
    });
    mock_chain_handle.expect_promote_block().never();

    let (monitoring_tx, _monitoring_rx) = create_monitoring_event_channel();
    let (notify_tx, _notify_rx) = create_test_notify_channel();
    let mut worker = OrganiseWorker::new(
        organise_rx,
        create_validation_channel().0,
        mock_chain_handle,
        monitoring_tx,
        notify_tx,
        create_test_metrics_handle(),
        create_test_pplns_window(),
        stub_share_validator_with_success(),
    );

    let share = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let outcome = worker.process_share_block(share, true).await.unwrap();

    assert!(matches!(outcome, ProcessOutcome::NotAdvanced));
    assert!(
        worker.pending_blocks.is_empty(),
        "a permanently failing block must not be queued for retry"
    );
}

#[tokio::test]
async fn test_drain_reattempts_all_blocks_sharing_a_parent_height() {
    // Two distinct blocks buffered under the same parent height must both be
    // re-attempted when that height is drained -- the buffer keeps a Vec per
    // height, not a single block.
    let (_organise_tx, organise_rx) = create_organise_channel();
    let mut mock_chain_handle = MockChainStoreHandle::new();
    mock_chain_handle
        .expect_get_candidate_blocks_needing_validation()
        .returning(|_, _| Ok(Vec::new()));
    mock_chain_handle
        .expect_get_tip_height()
        .returning(|| Ok(None));
    // The confirmed-block follow-up reads the block back; this test is
    // not about the share feed, so let it find nothing and skip.
    mock_chain_handle
        .expect_get_confirmed_at_height()
        .returning(|_| Err(StoreError::NotFound("not under test".to_string())));

    // Parent is confirmed (Valid); the blocks sit below the PPLNS zone, so
    // no chain-context validation runs. They are still marked BlockValid
    // (prune-window blocks) and then promote directly.
    mock_chain_handle
        .expect_get_block_metadata()
        .returning(|_| {
            Ok(BlockMetadata {
                expected_height: Some(5),
                chain_work: bitcoin::Work::from_be_bytes([0u8; 32]),
                status: Status::BlockValid,
                chain: ChainMembership::Confirmed,
            })
        });
    mock_chain_handle
        .expect_get_candidate_tip_height()
        .returning(|| Ok(Some(200_000)));
    mock_chain_handle
        .expect_mark_block_valid()
        .returning(|_| Ok(()));

    let promote_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let promote_counter = promote_count.clone();
    mock_chain_handle
        .expect_promote_block()
        .returning(move |_| {
            promote_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Some(6))
        });
    mock_chain_handle.expect_is_current().returning(|| false);
    mock_chain_handle
        .expect_get_uncle_infos()
        .returning(|_| Vec::new());

    let (monitoring_tx, _monitoring_rx) = create_monitoring_event_channel();
    let (notify_tx, _notify_rx) = create_test_notify_channel();
    let mut worker = OrganiseWorker::new(
        organise_rx,
        create_validation_channel().0,
        mock_chain_handle,
        monitoring_tx,
        notify_tx,
        create_test_metrics_handle(),
        create_test_pplns_window(),
        stub_share_validator_with_success(),
    );

    // Two distinct blocks buffered under the same parent height (5).
    let share_a = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let share_b = TestShareBlockBuilder::new().nonce(0xe9695792).build();
    worker.buffer_block(5, share_a, true);
    worker.buffer_block(5, share_b, true);

    worker.drain_pending_blocks(5).await.unwrap();

    // Both buffered blocks were re-attempted and promoted (neither was
    // overwritten by the other).
    assert_eq!(
        promote_count.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "both blocks sharing a parent height must be drained"
    );
}

/// Fail-first regression for the out-of-order stranding. Validation
/// runs one tokio task per block, so a child's OrganiseEvent can be handled
/// before its parent's. The child must still become BlockValid once the
/// parent does -- otherwise the contiguous promotion prefix wedges at the
/// stranded child, and confirmation (which the child blocks) can never
/// revisit it.
///
/// Layout: GP is confirmed (height 2); P (height 3) and C (height 4) are a
/// candidate fork at/below the confirmed tip. C is delivered before P. On
/// the pre-fix code C is validated immediately, gate-blocked to HeaderValid,
/// and never re-marked; here it is buffered and re-marked via the drain.
#[tokio::test]
async fn test_out_of_order_child_becomes_valid_after_parent() {
    use std::collections::HashSet;
    use std::sync::Mutex;

    let (organise_tx, organise_rx) = create_organise_channel();
    let mut mock_chain_handle = MockChainStoreHandle::new();
    mock_chain_handle
        .expect_get_candidate_blocks_needing_validation()
        .returning(|_, _| Ok(Vec::new()));
    // The confirmed-block follow-up reads each newly confirmed block
    // back; this test is not about the share feed, so let it find
    // nothing and skip.
    mock_chain_handle
        .expect_get_confirmed_at_height()
        .returning(|_| Err(StoreError::NotFound("not under test".to_string())));
    mock_chain_handle
        .expect_clone()
        .return_once(MockChainStoreHandle::new);

    let gp = TestShareBlockBuilder::new().nonce(0xe9695790).build();
    let gp_hash = gp.block_hash();
    let p = TestShareBlockBuilder::new()
        .prev_share_blockhash(gp_hash.to_string())
        .nonce(0xe9695791)
        .build();
    let p_hash = p.block_hash();
    let c = TestShareBlockBuilder::new()
        .prev_share_blockhash(p_hash.to_string())
        .nonce(0xe9695792)
        .build();
    let c_hash = c.block_hash();

    // Records which blocks have been marked BlockValid; the metadata mock
    // reflects the transition so a drained child sees a now-valid parent.
    let marked: Arc<Mutex<HashSet<BlockHash>>> = Arc::new(Mutex::new(HashSet::new()));

    let marked_meta = marked.clone();
    mock_chain_handle
        .expect_share_block_exists()
        .returning(|_| false);
    mock_chain_handle
        .expect_get_block_metadata()
        .returning(move |hash| {
            let hash = *hash;
            let (height, status, chain) = if hash == gp_hash {
                (2, Status::BlockValid, ChainMembership::Confirmed)
            } else if hash == p_hash {
                let status = if marked_meta.lock().unwrap().contains(&p_hash) {
                    Status::BlockValid
                } else {
                    Status::HeaderValid
                };
                (3, status, ChainMembership::Candidate)
            } else if hash == c_hash {
                let status = if marked_meta.lock().unwrap().contains(&c_hash) {
                    Status::BlockValid
                } else {
                    Status::HeaderValid
                };
                (4, status, ChainMembership::Candidate)
            } else {
                (0, Status::BlockValid, ChainMembership::Confirmed)
            };
            Ok(BlockMetadata {
                expected_height: Some(height),
                chain_work: bitcoin::Work::from_be_bytes([0u8; 32]),
                status,
                chain,
            })
        });
    let marked_mark = marked.clone();
    mock_chain_handle
        .expect_mark_block_valid()
        .returning(move |hash| {
            marked_mark.lock().unwrap().insert(hash);
            Ok(())
        });
    // Small candidate tip so heights 3 and 4 are inside the PPLNS zone.
    mock_chain_handle
        .expect_get_candidate_tip_height()
        .returning(|| Ok(Some(5)));
    // The fork is not promotable yet (candidate, off the confirmed chain).
    mock_chain_handle
        .expect_promote_block()
        .returning(|_| Ok(None));
    // Buffering C triggers an opportunistic organise_block.
    mock_chain_handle
        .expect_organise_block()
        .returning(|| Ok(None));
    mock_chain_handle
        .expect_get_tip_height()
        .returning(|| Ok(Some(2)));

    let (monitoring_tx, _monitoring_rx) = create_monitoring_event_channel();
    let (notify_tx, _notify_rx) = create_test_notify_channel();
    let worker = OrganiseWorker::new(
        organise_rx,
        create_validation_channel().0,
        mock_chain_handle,
        monitoring_tx,
        notify_tx,
        create_test_metrics_handle(),
        create_test_pplns_window(),
        stub_share_validator_with_success(),
    );

    // Deliver the child before the parent (out of order).
    organise_tx
        .send(OrganiseEvent::Block {
            share_block: c,
            content_validated: true,
        })
        .await
        .unwrap();
    organise_tx
        .send(OrganiseEvent::Block {
            share_block: p,
            content_validated: true,
        })
        .await
        .unwrap();
    drop(organise_tx);

    worker.run().await.unwrap();

    let marked = marked.lock().unwrap();
    assert!(marked.contains(&p_hash), "parent P must be BlockValid");
    assert!(
        marked.contains(&c_hash),
        "child C must become BlockValid after its parent, not be stranded"
    );
}

/// When organise_block confirms a contiguous range of
/// heights at once (common during sync), the Buffered/Invalidated follow-up
/// must drain dependents buffered under the intermediate heights, not only
/// the new top.
///
/// C is buffered under intermediate height 7; organise_block advances the
/// confirmed tip from 5 to 10. The drain must visit height 7 and process C.
/// On the pre-fix code the drain started at the top (10) and walked only
/// upward, skipping 6..9 and stranding C.
#[tokio::test]
async fn test_buffered_follow_up_drains_intermediate_confirmed_heights() {
    let (_organise_tx, organise_rx) = create_organise_channel();
    let mut mock_chain_handle = MockChainStoreHandle::new();
    mock_chain_handle
        .expect_get_candidate_blocks_needing_validation()
        .returning(|_, _| Ok(Vec::new()));
    // The confirmed-block follow-up reads the block back; this test is
    // not about the share feed, so let it find nothing and skip.
    mock_chain_handle
        .expect_get_confirmed_at_height()
        .returning(|_| Err(StoreError::NotFound("not under test".to_string())));

    // Distinct parent hashes so the metadata mock can give T a Pending parent
    // (T buffers, triggering the follow-up) and C a Valid parent (C promotes).
    let t_parent = TestShareBlockBuilder::new()
        .nonce(0x0000aaaa)
        .build()
        .block_hash();
    let c_parent = TestShareBlockBuilder::new()
        .nonce(0x0000bbbb)
        .build()
        .block_hash();
    let share_t = TestShareBlockBuilder::new()
        .prev_share_blockhash(t_parent.to_string())
        .nonce(0x0000cccc)
        .build();
    let share_c = TestShareBlockBuilder::new()
        .prev_share_blockhash(c_parent.to_string())
        .nonce(0x0000dddd)
        .build();
    let c_hash = share_c.block_hash();

    mock_chain_handle
        .expect_share_block_exists()
        .returning(|_| false);
    mock_chain_handle
        .expect_get_block_metadata()
        .returning(move |hash| {
            let (height, status, chain) = if *hash == t_parent {
                (5, Status::HeaderValid, ChainMembership::Candidate)
            } else if *hash == c_parent {
                (7, Status::BlockValid, ChainMembership::Confirmed)
            } else if *hash == c_hash {
                (7, Status::HeaderValid, ChainMembership::Candidate)
            } else {
                (0, Status::BlockValid, ChainMembership::Confirmed)
            };
            Ok(BlockMetadata {
                expected_height: Some(height),
                chain_work: bitcoin::Work::from_be_bytes([0u8; 32]),
                status,
                chain,
            })
        });
    // Previous confirmed tip 5; organise_block confirms up to 10 in one call.
    mock_chain_handle
        .expect_get_tip_height()
        .returning(|| Ok(Some(5)));
    mock_chain_handle
        .expect_organise_block()
        .returning(|| Ok(Some(10)));
    // Large candidate tip so C (height 7) is below the PPLNS zone and promotes
    // without chain-context validation.
    mock_chain_handle
        .expect_get_candidate_tip_height()
        .returning(|| Ok(Some(200_000)));
    mock_chain_handle
        .expect_mark_block_valid()
        .returning(|_| Ok(()));
    mock_chain_handle
        .expect_promote_block()
        .returning(|_| Ok(None));

    let (monitoring_tx, _monitoring_rx) = create_monitoring_event_channel();
    let (notify_tx, _notify_rx) = create_test_notify_channel();
    let mut worker = OrganiseWorker::new(
        organise_rx,
        create_validation_channel().0,
        mock_chain_handle,
        monitoring_tx,
        notify_tx,
        create_test_metrics_handle(),
        create_test_pplns_window(),
        stub_share_validator_with_success(),
    );

    // C buffered under intermediate height 7 (between old tip 5 and new tip 10).
    worker.buffer_block(7, share_c, true);
    // T buffers (Pending parent), triggering the organise_block + drain follow-up.
    worker
        .handle_organise_block_event(share_t, true)
        .await
        .unwrap();

    assert!(
        !worker.pending_blocks.contains_key(&7),
        "dependent buffered under an intermediate confirmed height must be drained"
    );
}

/// A StoreAccess validation failure with a valid parent is fatal (data that
/// must exist could not be read -- corruption or a bug). The block must NOT
/// be marked Invalid, and the worker must stop with an error rather than
/// silently diverge.
#[tokio::test]
async fn test_organise_worker_fatal_on_store_access_validation_error() {
    let (organise_tx, organise_rx) = create_organise_channel();
    let mut mock_chain_handle = MockChainStoreHandle::new();
    mock_chain_handle
        .expect_get_candidate_blocks_needing_validation()
        .returning(|_, _| Ok(Vec::new()));
    // The confirmed-block follow-up reads each newly confirmed block
    // back; this test is not about the share feed, so let it find
    // nothing and skip.
    mock_chain_handle
        .expect_get_confirmed_at_height()
        .returning(|_| Err(StoreError::NotFound("not under test".to_string())));
    mock_chain_handle
        .expect_get_tip_height()
        .returning(|| Ok(None));
    mock_chain_handle
        .expect_clone()
        .return_once(MockChainStoreHandle::new);
    // Parent is valid and the block is in the PPLNS zone, so validation runs.
    mock_chain_handle
        .expect_get_block_metadata()
        .returning(|_| {
            Ok(BlockMetadata {
                expected_height: Some(10),
                chain_work: bitcoin::Work::from_be_bytes([0u8; 32]),
                status: Status::BlockValid,
                chain: ChainMembership::Confirmed,
            })
        });
    mock_chain_handle
        .expect_get_candidate_tip_height()
        .returning(|| Ok(Some(10)));
    // Must not mark Invalid/BlockValid, must not promote.
    mock_chain_handle.expect_mark_invalid().never();
    mock_chain_handle.expect_mark_block_valid().never();
    mock_chain_handle.expect_promote_block().never();

    let mut mock_validator = MockDefaultShareValidator::new();
    mock_validator
        .expect_validate_with_chain_context()
        .returning(|_, _, _| Err(ValidationError::store_access("store read failed")));
    let share_validator: Arc<dyn ShareValidator + Send + Sync> = Arc::new(mock_validator);

    let (monitoring_tx, _monitoring_rx) = create_monitoring_event_channel();
    let (notify_tx, _notify_rx) = create_test_notify_channel();
    let worker = OrganiseWorker::new(
        organise_rx,
        create_validation_channel().0,
        mock_chain_handle,
        monitoring_tx,
        notify_tx,
        create_test_metrics_handle(),
        create_test_pplns_window(),
        share_validator,
    );

    let share = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    organise_tx
        .send(OrganiseEvent::Block {
            share_block: share,
            content_validated: true,
        })
        .await
        .unwrap();
    drop(organise_tx);

    let result = worker.run().await;
    assert!(
        result.is_err(),
        "a store error with a valid parent must be fatal"
    );
}

/// A block whose parent is HeaderValid with its body stored (stranded) is
/// buffered, and the parent's validation is re-driven so it can be promoted.
#[tokio::test]
async fn test_stranded_parent_is_redriven_and_child_buffered() {
    let parent_hash = TestShareBlockBuilder::new()
        .nonce(0x11111111)
        .build()
        .block_hash();
    let child = TestShareBlockBuilder::new()
        .nonce(0x22222222)
        .prev_share_blockhash(parent_hash.to_string())
        .build();

    let mut mock_chain_handle = MockChainStoreHandle::new();
    mock_chain_handle
        .expect_get_candidate_blocks_needing_validation()
        .returning(|_, _| Ok(Vec::new()));
    mock_chain_handle
        .expect_get_block_metadata()
        .returning(|_| {
            Ok(BlockMetadata {
                expected_height: Some(5),
                chain_work: bitcoin::Work::from_be_bytes([0u8; 32]),
                status: Status::HeaderValid,
                chain: ChainMembership::Candidate,
            })
        });
    mock_chain_handle
        .expect_share_block_exists()
        .returning(|_| true);
    mock_chain_handle.expect_is_current().returning(|| false);

    let (_organise_tx, organise_rx) = create_organise_channel();
    let (monitoring_tx, _monitoring_rx) = create_monitoring_event_channel();
    let (notify_tx, _notify_rx) = create_test_notify_channel();
    let (validation_tx, mut validation_rx) = create_validation_channel();
    let mut worker = OrganiseWorker::new(
        organise_rx,
        validation_tx,
        mock_chain_handle,
        monitoring_tx,
        notify_tx,
        create_test_metrics_handle(),
        create_test_pplns_window(),
        stub_share_validator_with_success(),
    );

    let outcome = worker
        .process_share_block(child, true)
        .await
        .expect("process_share_block");
    assert!(matches!(outcome, ProcessOutcome::Buffered));

    match validation_rx.try_recv() {
        Ok(ValidationEvent::ValidateBlockHash(hash)) => assert_eq!(hash, parent_hash),
        Ok(ValidationEvent::ValidateShareBlock(_)) => {
            panic!("expected ValidateBlockHash, got ValidateShareBlock")
        }
        Err(error) => {
            panic!("expected a re-drive event for the stranded parent, got {error:?}")
        }
    }
    assert_eq!(worker.pending_blocks.get(&5).map(Vec::len), Some(1));
}

/// A block whose parent is HeaderValid but whose body has not arrived is
/// buffered without re-driving: there is nothing yet to re-validate.
#[tokio::test]
async fn test_waiting_parent_is_not_redriven() {
    let parent_hash = TestShareBlockBuilder::new()
        .nonce(0x33333333)
        .build()
        .block_hash();
    let child = TestShareBlockBuilder::new()
        .nonce(0x44444444)
        .prev_share_blockhash(parent_hash.to_string())
        .build();

    let mut mock_chain_handle = MockChainStoreHandle::new();
    mock_chain_handle
        .expect_get_candidate_blocks_needing_validation()
        .returning(|_, _| Ok(Vec::new()));
    mock_chain_handle
        .expect_get_block_metadata()
        .returning(|_| {
            Ok(BlockMetadata {
                expected_height: Some(7),
                chain_work: bitcoin::Work::from_be_bytes([0u8; 32]),
                status: Status::HeaderValid,
                chain: ChainMembership::Candidate,
            })
        });
    mock_chain_handle
        .expect_share_block_exists()
        .returning(|_| false);

    let (_organise_tx, organise_rx) = create_organise_channel();
    let (monitoring_tx, _monitoring_rx) = create_monitoring_event_channel();
    let (notify_tx, _notify_rx) = create_test_notify_channel();
    let (validation_tx, mut validation_rx) = create_validation_channel();
    let mut worker = OrganiseWorker::new(
        organise_rx,
        validation_tx,
        mock_chain_handle,
        monitoring_tx,
        notify_tx,
        create_test_metrics_handle(),
        create_test_pplns_window(),
        stub_share_validator_with_success(),
    );

    let outcome = worker
        .process_share_block(child, true)
        .await
        .expect("process_share_block");
    assert!(matches!(outcome, ProcessOutcome::Buffered));
    assert!(
        validation_rx.try_recv().is_err(),
        "a waiting parent must not be re-driven"
    );
    assert_eq!(worker.pending_blocks.get(&7).map(Vec::len), Some(1));
}

/// At startup the worker re-drives validation for the stranded run above the
/// confirmed tip, so a restart resumes promotion with no peer re-send.
#[tokio::test]
async fn test_startup_seed_redrives_stranded_run() {
    let stranded: Vec<BlockHash> = vec![
        TestShareBlockBuilder::new()
            .nonce(0x55555555)
            .build()
            .block_hash(),
        TestShareBlockBuilder::new()
            .nonce(0x66666666)
            .build()
            .block_hash(),
        TestShareBlockBuilder::new()
            .nonce(0x77777777)
            .build()
            .block_hash(),
    ];

    let mut mock_chain_handle = MockChainStoreHandle::new();
    mock_chain_handle
        .expect_get_tip_height()
        .returning(|| Ok(Some(10)));
    let seed = stranded.clone();
    mock_chain_handle
        .expect_get_candidate_blocks_needing_validation()
        .returning(move |from_height, limit| {
            assert_eq!(from_height, 11, "seed scans from confirmed tip + 1");
            assert_eq!(limit, REVALIDATION_WINDOW);
            Ok(seed.clone())
        });

    let (organise_tx, organise_rx) = create_organise_channel();
    let (monitoring_tx, _monitoring_rx) = create_monitoring_event_channel();
    let (notify_tx, _notify_rx) = create_test_notify_channel();
    let (validation_tx, mut validation_rx) = create_validation_channel();
    let worker = OrganiseWorker::new(
        organise_rx,
        validation_tx,
        mock_chain_handle,
        monitoring_tx,
        notify_tx,
        create_test_metrics_handle(),
        create_test_pplns_window(),
        stub_share_validator_with_success(),
    );

    // Close the organise channel so run() exits once the seed has fired.
    drop(organise_tx);
    worker.run().await.expect("run");

    for expected in stranded {
        match validation_rx.try_recv() {
            Ok(ValidationEvent::ValidateBlockHash(hash)) => assert_eq!(hash, expected),
            Ok(ValidationEvent::ValidateShareBlock(_)) => {
                panic!("expected ValidateBlockHash, got ValidateShareBlock")
            }
            Err(error) => panic!("expected a seeded re-drive event, got {error:?}"),
        }
    }
    assert!(
        validation_rx.try_recv().is_err(),
        "seed must emit exactly the stranded run"
    );
}

/// A store read failure while reading the confirmed tip at startup is fatal:
/// it must not be silently treated as an empty chain, which would run a
/// full-history recovery scan and continue past a real store fault.
#[tokio::test]
async fn test_startup_seed_is_fatal_on_confirmed_tip_read_error() {
    let mut mock_chain_handle = MockChainStoreHandle::new();
    mock_chain_handle
        .expect_get_tip_height()
        .returning(|| Err(StoreError::Database("simulated read failure".to_string())));

    let (_organise_tx, organise_rx) = create_organise_channel();
    let (monitoring_tx, _monitoring_rx) = create_monitoring_event_channel();
    let (notify_tx, _notify_rx) = create_test_notify_channel();
    let worker = OrganiseWorker::new(
        organise_rx,
        create_validation_channel().0,
        mock_chain_handle,
        monitoring_tx,
        notify_tx,
        create_test_metrics_handle(),
        create_test_pplns_window(),
        stub_share_validator_with_success(),
    );

    let result = worker.run().await;
    assert!(
        result.is_err(),
        "a confirmed-tip read error at startup must be fatal, not a full-history scan"
    );
}
