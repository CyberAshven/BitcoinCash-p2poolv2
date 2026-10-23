use super::*;
use crate::node::SwarmSend;
use crate::node::messages::Message;
use crate::node::request_response_handler::block_fetcher;
use crate::shares::bch_genesis::MINI_ANCHOR_TARGET;
#[mockall_double::double]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
use crate::shares::share_block::MAX_POOL_TARGET;
use crate::shares::validation::{MockDefaultShareValidator, ValidationError};
use crate::store::block_tx_metadata::{BlockMetadata, ChainMembership, Status};
use crate::test_utils::{TestShareBlockBuilder, share_header_batch_with_empty_branches};
use tokio::sync::{mpsc, oneshot};

/// Build a single test header with bits set to MAX_POOL_TARGET so it
/// passes both minimum difficulty and ASERT validation (since ASERT
/// computes MAX_POOL_TARGET from the genesis anchor).
fn build_valid_test_header() -> ShareHeader {
    let mut header = TestShareBlockBuilder::new().build().header;
    header.bits = CompactTarget::from_consensus(MAX_POOL_TARGET);
    header
}

/// Set up chain_store_handle mocks for validate_header_chain:
/// genesis header, parent header lookup, and parent metadata.
fn setup_chain_validation_mocks(chain_store_handle: &mut ChainStoreHandle) {
    let template_header = build_valid_test_header();
    let genesis_header = template_header.clone();
    chain_store_handle
        .expect_get_genesis_header()
        .returning(move || Ok(genesis_header.clone()));
    let parent_header = template_header.clone();
    chain_store_handle
        .expect_get_share_header()
        .returning(move |_| Ok(parent_header.clone()));
    let asert_parent_header = template_header.clone();
    chain_store_handle
        .expect_get_asert_parent()
        .returning(move |_| {
            Ok(AsertParent::from_header(
                &asert_parent_header,
                0,
                asert_parent_header.time,
            ))
        });
    chain_store_handle
        .expect_get_block_metadata()
        .returning(|_| {
            Ok(BlockMetadata {
                expected_height: Some(0),
                chain_work: Work::from_hex("0x00").unwrap(),
                status: Status::BlockValid,
                chain: ChainMembership::Confirmed,
            })
        });
    chain_store_handle
        .expect_get_block_metadata_batch()
        .returning(|hashes| {
            Ok(hashes
                .iter()
                .map(|hash| {
                    (
                        *hash,
                        BlockMetadata {
                            expected_height: Some(0),
                            chain_work: Work::from_hex("0x00").unwrap(),
                            status: Status::BlockValid,
                            chain: ChainMembership::Confirmed,
                        },
                    )
                })
                .collect())
        });
}

fn setup_minimum_difficulty_mock(mock_validator: &mut MockDefaultShareValidator) {
    mock_validator
        .expect_validate_header_minimum_difficulty()
        .returning(|_| Ok(()));
    mock_validator
        .expect_validate_coinbase_proof()
        .returning(|_, _| Ok(()));
    mock_validator
        .expect_validate_header_timestamp()
        .returning(|_, _| Ok(()));
    let mut pool_difficulty = PoolDifficulty::default();
    pool_difficulty
        .expect_calculate_target_clamped()
        .returning(|_| CompactTarget::from_consensus(MAX_POOL_TARGET));
    pool_difficulty
        .expect_target_ceiling()
        .return_const(CompactTarget::from_consensus(MAX_POOL_TARGET));
    mock_validator
        .expect_pool_difficulty()
        .return_const(pool_difficulty);
}

#[tokio::test]
async fn test_fewer_than_max_headers_does_not_send_getheaders() {
    let peer_id = libp2p::PeerId::random();
    let mut chain_store_handle = ChainStoreHandle::default();
    chain_store_handle
        .expect_organise_header()
        .returning(|_| Ok(None));
    chain_store_handle
        .expect_add_header_template_merkle_branches()
        .returning(|_| Ok(()));
    chain_store_handle
        .expect_find_fork_point_height()
        .returning(|_| Ok(Some(0)));
    chain_store_handle
        .expect_get_candidate_blocks_missing_data()
        .returning(|_| Ok(Vec::new()));
    setup_chain_validation_mocks(&mut chain_store_handle);

    let (swarm_tx, mut swarm_rx) = mpsc::channel::<SwarmSend<oneshot::Sender<Message>>>(32);
    let (block_fetcher_handle, _block_fetcher_rx) = block_fetcher::create_block_fetcher_channel();

    let mut mock_validator = MockDefaultShareValidator::default();
    setup_minimum_difficulty_mock(&mut mock_validator);

    let share_headers = vec![build_valid_test_header()];

    let result = handle_share_headers(
        peer_id,
        share_header_batch_with_empty_branches(share_headers),
        chain_store_handle,
        swarm_tx,
        block_fetcher_handle,
        &mock_validator,
    )
    .await;
    assert!(result.is_ok(), "Expected Ok, got: {}", result.unwrap_err());

    assert!(swarm_rx.try_recv().is_err());
}

/// A header whose coinbase proof fails rejects the batch before anything
/// is stored or organised: a bitcoin header replayed under other share
/// fields never reaches the candidate chain.
#[tokio::test]
async fn test_handle_share_headers_rejects_header_with_invalid_coinbase_proof() {
    let peer_id = libp2p::PeerId::random();
    let mut chain_store_handle = ChainStoreHandle::default();
    setup_chain_validation_mocks(&mut chain_store_handle);

    let (swarm_tx, _swarm_rx) = mpsc::channel::<SwarmSend<oneshot::Sender<Message>>>(32);
    let (block_fetcher_handle, _block_fetcher_rx) = block_fetcher::create_block_fetcher_channel();

    let mut mock_validator = MockDefaultShareValidator::default();
    mock_validator
        .expect_validate_header_minimum_difficulty()
        .returning(|_| Ok(()));
    mock_validator
        .expect_validate_coinbase_proof()
        .returning(|_, _| {
            Err(ValidationError::consensus(
                "Invalid coinbase proof: proof gives merkle root a, bitcoin header has b",
            ))
        });
    let pool_difficulty = PoolDifficulty::default();
    mock_validator
        .expect_pool_difficulty()
        .return_const(pool_difficulty);

    let result = handle_share_headers(
        peer_id,
        share_header_batch_with_empty_branches(vec![build_valid_test_header()]),
        chain_store_handle,
        swarm_tx,
        block_fetcher_handle,
        &mock_validator,
    )
    .await;

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("coinbase proof"));
}

/// Each validated header's branch is stored before the headers are
/// organised, so the node can serve headers it holds without bodies.
#[tokio::test]
async fn test_handle_share_headers_stores_branches_of_validated_headers() {
    let peer_id = libp2p::PeerId::random();
    let header = build_valid_test_header();
    let branch = vec![bitcoin::TxMerkleNode::from_byte_array([0x42; 32])];
    let expected_entries = vec![(header.block_hash(), branch.clone())];

    let mut chain_store_handle = ChainStoreHandle::default();
    chain_store_handle
        .expect_add_header_template_merkle_branches()
        .withf(move |entries| *entries == expected_entries)
        .times(1)
        .returning(|_| Ok(()));
    chain_store_handle
        .expect_organise_header()
        .returning(|_| Ok(None));
    chain_store_handle
        .expect_find_fork_point_height()
        .returning(|_| Ok(Some(0)));
    chain_store_handle
        .expect_get_candidate_blocks_missing_data()
        .returning(|_| Ok(Vec::new()));
    setup_chain_validation_mocks(&mut chain_store_handle);

    let (swarm_tx, _swarm_rx) = mpsc::channel::<SwarmSend<oneshot::Sender<Message>>>(32);
    let (block_fetcher_handle, _block_fetcher_rx) = block_fetcher::create_block_fetcher_channel();

    let mut mock_validator = MockDefaultShareValidator::default();
    setup_minimum_difficulty_mock(&mut mock_validator);

    let result = handle_share_headers(
        peer_id,
        ShareHeaderBatch::from_headers_with_branches(vec![(header, branch)]),
        chain_store_handle,
        swarm_tx,
        block_fetcher_handle,
        &mock_validator,
    )
    .await;

    assert!(result.is_ok(), "Expected Ok, got: {}", result.unwrap_err());
}

#[tokio::test]
async fn test_empty_headers_does_not_send_getheaders() {
    let peer_id = libp2p::PeerId::random();
    let mut chain_store_handle = ChainStoreHandle::default();
    chain_store_handle
        .expect_get_candidate_blocks_missing_data()
        .returning(|_| Ok(Vec::new()));
    let (swarm_tx, mut swarm_rx) = mpsc::channel::<SwarmSend<oneshot::Sender<Message>>>(32);
    let (block_fetcher_handle, _block_fetcher_rx) = block_fetcher::create_block_fetcher_channel();

    let mock_validator = MockDefaultShareValidator::default();
    let share_headers: Vec<ShareHeader> = Vec::new();

    let result = handle_share_headers(
        peer_id,
        share_header_batch_with_empty_branches(share_headers),
        chain_store_handle,
        swarm_tx,
        block_fetcher_handle,
        &mock_validator,
    )
    .await;

    assert!(result.is_ok());
    assert!(swarm_rx.try_recv().is_err());
}

#[tokio::test]
async fn test_share_headers_sends_getheaders_to_same_peer() {
    let peer_id = libp2p::PeerId::random();
    let mut chain_store_handle = ChainStoreHandle::default();
    chain_store_handle
        .expect_organise_header()
        .returning(|_| Ok(None));
    chain_store_handle
        .expect_add_header_template_merkle_branches()
        .returning(|_| Ok(()));
    setup_chain_validation_mocks(&mut chain_store_handle);

    let (swarm_tx, mut swarm_rx) = mpsc::channel::<SwarmSend<oneshot::Sender<Message>>>(32);
    let (block_fetcher_handle, _block_fetcher_rx) = block_fetcher::create_block_fetcher_channel();

    let mut mock_validator = MockDefaultShareValidator::default();
    setup_minimum_difficulty_mock(&mut mock_validator);

    let first = build_valid_test_header();
    let mut parent_hash = first.prev_share_blockhash;
    let mut share_headers = Vec::with_capacity(MAX_HEADERS_IN_RESPONSE);
    for nonce in 0..MAX_HEADERS_IN_RESPONSE as u32 {
        let mut header = TestShareBlockBuilder::new()
            .prev_share_blockhash(parent_hash.to_string())
            .nonce(nonce)
            .build()
            .header;
        header.bits = CompactTarget::from_consensus(MAX_POOL_TARGET);
        parent_hash = header.block_hash();
        share_headers.push(header);
    }
    let expected_last_hash = share_headers.last().unwrap().block_hash();

    let result = handle_share_headers(
        peer_id,
        share_header_batch_with_empty_branches(share_headers),
        chain_store_handle,
        swarm_tx,
        block_fetcher_handle,
        &mock_validator,
    )
    .await;

    assert!(result.is_ok(), "Expected Ok, got: {}", result.unwrap_err());

    let swarm_message = swarm_rx
        .try_recv()
        .expect("expected a follow-up getheaders request");
    match swarm_message {
        SwarmSend::Request(sent_peer_id, Message::GetShareHeaders(locator, stop_hash)) => {
            assert_eq!(sent_peer_id, peer_id);
            assert_eq!(locator, vec![expected_last_hash]);
            assert_eq!(stop_hash, BlockHash::all_zeros());
        }
        other => panic!("expected SwarmSend::Request with GetShareHeaders, got: {other:?}"),
    }
}

#[tokio::test]
async fn test_fewer_than_max_headers_sends_fetch_blocks_event() {
    let peer_id = libp2p::PeerId::random();
    let mut chain_store_handle = ChainStoreHandle::default();
    chain_store_handle
        .expect_organise_header()
        .returning(|_| Ok(None));
    chain_store_handle
        .expect_add_header_template_merkle_branches()
        .returning(|_| Ok(()));
    chain_store_handle
        .expect_find_fork_point_height()
        .returning(|_| Ok(Some(0)));

    let missing_hash = bitcoin::BlockHash::all_zeros();
    let expected_hashes = vec![missing_hash];
    let returned_hashes = expected_hashes.clone();
    chain_store_handle
        .expect_get_candidate_blocks_missing_data()
        .returning(move |_| Ok(returned_hashes.clone()));
    chain_store_handle.expect_is_current().returning(|| true);
    setup_chain_validation_mocks(&mut chain_store_handle);

    let (swarm_tx, _swarm_rx) = mpsc::channel::<SwarmSend<oneshot::Sender<Message>>>(32);
    let (block_fetcher_handle, mut block_fetcher_rx) =
        block_fetcher::create_block_fetcher_channel();

    let mut mock_validator = MockDefaultShareValidator::default();
    setup_minimum_difficulty_mock(&mut mock_validator);

    let share_headers = vec![build_valid_test_header()];

    let result = handle_share_headers(
        peer_id,
        share_header_batch_with_empty_branches(share_headers),
        chain_store_handle,
        swarm_tx,
        block_fetcher_handle,
        &mock_validator,
    )
    .await;

    assert!(result.is_ok(), "Expected Ok, got: {}", result.unwrap_err());

    let event = block_fetcher_rx
        .try_recv()
        .expect("expected a FetchBlocks event");
    match event {
        BlockFetcherEvent::FetchBlocks {
            blockhashes,
            peer_id: event_peer_id,
            use_peer,
        } => {
            assert_eq!(blockhashes, expected_hashes);
            assert_eq!(event_peer_id, peer_id);
            assert!(use_peer, "use_peer should be true when chain is current");
        }
        other => panic!("expected FetchBlocks event, got: {other}"),
    }
}

#[tokio::test]
async fn test_asert_mismatch_rejected() {
    let peer_id = libp2p::PeerId::random();
    let mut chain_store_handle = ChainStoreHandle::default();
    setup_chain_validation_mocks(&mut chain_store_handle);

    let (swarm_tx, _swarm_rx) = mpsc::channel::<SwarmSend<oneshot::Sender<Message>>>(32);
    let (block_fetcher_handle, _block_fetcher_rx) = block_fetcher::create_block_fetcher_channel();

    let mut mock_validator = MockDefaultShareValidator::default();
    setup_minimum_difficulty_mock(&mut mock_validator);

    // Header with wrong bits (genesis bits instead of MAX_POOL_TARGET)
    let header = TestShareBlockBuilder::new().build().header;
    // header.bits is 0x1b4188f5 (genesis), but ASERT expects MAX_POOL_TARGET
    let share_headers = vec![header];

    let result = handle_share_headers(
        peer_id,
        share_header_batch_with_empty_branches(share_headers),
        chain_store_handle,
        swarm_tx,
        block_fetcher_handle,
        &mock_validator,
    )
    .await;

    assert!(result.is_err());
    assert!(
        result.unwrap_err().to_string().contains("ASERT mismatch"),
        "Expected ASERT mismatch error"
    );
}

#[test]
fn test_validate_header_chain_accepts_single_header_linked_to_anchor() {
    let anchor = TestShareBlockBuilder::new().nonce(1).build();
    let mut child = TestShareBlockBuilder::new()
        .prev_share_blockhash(anchor.block_hash().to_string())
        .nonce(2)
        .build()
        .header;
    child.bits = CompactTarget::from_consensus(MAX_POOL_TARGET);

    let mut chain_store_handle = ChainStoreHandle::default();
    setup_chain_validation_mocks(&mut chain_store_handle);
    let mut mock_validator = MockDefaultShareValidator::default();
    setup_minimum_difficulty_mock(&mut mock_validator);

    let result = validate_header_chain(
        &share_header_batch_with_empty_branches(vec![child.clone()]),
        &chain_store_handle,
        &mock_validator,
    );
    assert!(result.is_ok(), "Expected Ok, got: {}", result.unwrap_err());
}

/// Header sync checks every header's timestamp against the times of its
/// ancestors, read from the batch for headers earlier in it and from the
/// store for the anchor's own ancestry. Here the store's anchor has time
/// T and no parent, so the first header is measured against [T] and the
/// second against [T + 1, T].
#[test]
fn test_validate_header_chain_checks_timestamps_against_batch_and_store_ancestors() {
    let anchor = TestShareBlockBuilder::new().nonce(1).build();
    let mut share_a = TestShareBlockBuilder::new()
        .prev_share_blockhash(anchor.block_hash().to_string())
        .nonce(2)
        .time(1_700_000_001)
        .build()
        .header;
    share_a.bits = CompactTarget::from_consensus(MAX_POOL_TARGET);
    let mut share_b = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_a.block_hash().to_string())
        .nonce(3)
        .time(1_700_000_002)
        .build()
        .header;
    share_b.bits = CompactTarget::from_consensus(MAX_POOL_TARGET);

    // The store returns the template header, at time 1_700_000_000 with
    // an all-zeros parent, for the anchor.
    let mut chain_store_handle = ChainStoreHandle::default();
    setup_chain_validation_mocks(&mut chain_store_handle);
    let mut mock_validator = MockDefaultShareValidator::default();
    mock_validator
        .expect_validate_header_minimum_difficulty()
        .returning(|_| Ok(()));
    mock_validator
        .expect_validate_coinbase_proof()
        .returning(|_, _| Ok(()));
    mock_validator
        .expect_validate_header_timestamp()
        .withf(|header, ancestor_times| {
            header.time == 1_700_000_001 && ancestor_times == [1_700_000_000]
        })
        .times(1)
        .returning(|_, _| Ok(()));
    mock_validator
        .expect_validate_header_timestamp()
        .withf(|header, ancestor_times| {
            header.time == 1_700_000_002 && ancestor_times == [1_700_000_001, 1_700_000_000]
        })
        .times(1)
        .returning(|_, _| Ok(()));
    let mut pool_difficulty = PoolDifficulty::default();
    pool_difficulty
        .expect_calculate_target_clamped()
        .returning(|_| CompactTarget::from_consensus(MAX_POOL_TARGET));
    pool_difficulty
        .expect_target_ceiling()
        .return_const(CompactTarget::from_consensus(MAX_POOL_TARGET));
    mock_validator
        .expect_pool_difficulty()
        .return_const(pool_difficulty);

    let result = validate_header_chain(
        &share_header_batch_with_empty_branches(vec![share_a, share_b]),
        &chain_store_handle,
        &mock_validator,
    );
    assert!(result.is_ok(), "Expected Ok, got: {}", result.unwrap_err());
}

/// A header whose timestamp breaks the rules rejects the batch before
/// anything is organised, and the error is not retried.
#[test]
fn test_validate_header_chain_rejects_a_header_with_an_invalid_timestamp() {
    let anchor = TestShareBlockBuilder::new().nonce(1).build();
    let mut child = TestShareBlockBuilder::new()
        .prev_share_blockhash(anchor.block_hash().to_string())
        .nonce(2)
        .build()
        .header;
    child.bits = CompactTarget::from_consensus(MAX_POOL_TARGET);

    let mut chain_store_handle = ChainStoreHandle::default();
    setup_chain_validation_mocks(&mut chain_store_handle);
    let mut mock_validator = MockDefaultShareValidator::default();
    mock_validator
        .expect_validate_header_minimum_difficulty()
        .returning(|_| Ok(()));
    mock_validator
        .expect_validate_coinbase_proof()
        .returning(|_, _| Ok(()));
    mock_validator
        .expect_validate_header_timestamp()
        .returning(|_, _| {
            Err(ValidationError::consensus(
                "Share timestamp is not greater than median time past",
            ))
        });
    let mut pool_difficulty = PoolDifficulty::default();
    pool_difficulty
        .expect_calculate_target_clamped()
        .returning(|_| CompactTarget::from_consensus(MAX_POOL_TARGET));
    pool_difficulty
        .expect_target_ceiling()
        .return_const(CompactTarget::from_consensus(MAX_POOL_TARGET));
    mock_validator
        .expect_pool_difficulty()
        .return_const(pool_difficulty);

    let error = validate_header_chain(
        &share_header_batch_with_empty_branches(vec![child]),
        &chain_store_handle,
        &mock_validator,
    )
    .unwrap_err();

    assert!(!error.is_retryable());
    assert!(
        error.to_string().contains("median time past"),
        "unexpected error: {error}"
    );
}

#[test]
fn test_validate_header_chain_accepts_linear_chain() {
    let anchor = TestShareBlockBuilder::new().nonce(1).build();
    let mut share_a = TestShareBlockBuilder::new()
        .prev_share_blockhash(anchor.block_hash().to_string())
        .nonce(2)
        .build()
        .header;
    share_a.bits = CompactTarget::from_consensus(MAX_POOL_TARGET);
    let mut share_b = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_a.block_hash().to_string())
        .nonce(3)
        .build()
        .header;
    share_b.bits = CompactTarget::from_consensus(MAX_POOL_TARGET);

    let mut chain_store_handle = ChainStoreHandle::default();
    setup_chain_validation_mocks(&mut chain_store_handle);
    let mut mock_validator = MockDefaultShareValidator::default();
    setup_minimum_difficulty_mock(&mut mock_validator);

    let headers = vec![share_a, share_b];
    let result = validate_header_chain(
        &share_header_batch_with_empty_branches(headers),
        &chain_store_handle,
        &mock_validator,
    );
    assert!(result.is_ok(), "Expected Ok, got: {}", result.unwrap_err());
}

#[test]
fn test_validate_header_chain_accepts_uncle_before_nephew() {
    let anchor = TestShareBlockBuilder::new().nonce(1).build();
    let mut share_a = TestShareBlockBuilder::new()
        .prev_share_blockhash(anchor.block_hash().to_string())
        .nonce(2)
        .build()
        .header;
    share_a.bits = CompactTarget::from_consensus(MAX_POOL_TARGET);
    // uncle is a sibling of share_a (same parent = anchor)
    let mut uncle = TestShareBlockBuilder::new()
        .prev_share_blockhash(anchor.block_hash().to_string())
        .nonce(3)
        .build()
        .header;
    uncle.bits = CompactTarget::from_consensus(MAX_POOL_TARGET);
    // share_b follows share_a on the confirmed chain
    let mut share_b = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_a.block_hash().to_string())
        .uncles(vec![uncle.block_hash()])
        .nonce(4)
        .build()
        .header;
    share_b.bits = CompactTarget::from_consensus(MAX_POOL_TARGET);

    let mut chain_store_handle = ChainStoreHandle::default();
    setup_chain_validation_mocks(&mut chain_store_handle);
    let mut mock_validator = MockDefaultShareValidator::default();
    setup_minimum_difficulty_mock(&mut mock_validator);

    // Order: share_a, uncle, share_b (as get_descendant_blockhashes produces)
    let headers = vec![share_a, uncle, share_b];
    let result = validate_header_chain(
        &share_header_batch_with_empty_branches(headers),
        &chain_store_handle,
        &mock_validator,
    );
    assert!(result.is_ok(), "Expected Ok, got: {}", result.unwrap_err());
}

#[test]
fn test_validate_header_chain_rejects_first_header_not_linked_to_anchor() {
    let unrelated = TestShareBlockBuilder::new().nonce(99).build();
    let mut child = TestShareBlockBuilder::new()
        .prev_share_blockhash(unrelated.block_hash().to_string())
        .nonce(2)
        .build()
        .header;
    child.bits = CompactTarget::from_consensus(MAX_POOL_TARGET);

    // No header in the batch has a parent that the store recognises.
    let mut chain_store_handle = ChainStoreHandle::default();
    chain_store_handle
        .expect_get_block_metadata()
        .returning(|_| Err(StoreError::NotFound("not found".into())));
    chain_store_handle
        .expect_get_block_metadata_batch()
        .returning(|_| Ok(Vec::new()));
    let mut mock_validator = MockDefaultShareValidator::default();
    setup_minimum_difficulty_mock(&mut mock_validator);

    let result = validate_header_chain(
        &share_header_batch_with_empty_branches(vec![child.clone()]),
        &chain_store_handle,
        &mock_validator,
    );
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("External parent(s) not found in store"),
    );
}

#[test]
fn test_validate_header_chain_rejects_forest_with_disconnected_subtree() {
    let anchor = TestShareBlockBuilder::new().nonce(1).build();
    let anchor_hash = anchor.block_hash();
    let mut share_a = TestShareBlockBuilder::new()
        .prev_share_blockhash(anchor.block_hash().to_string())
        .nonce(2)
        .build()
        .header;
    share_a.bits = CompactTarget::from_consensus(MAX_POOL_TARGET);
    // share_c has an unrelated parent, forming a disconnected subtree
    let unrelated = TestShareBlockBuilder::new().nonce(99).build();
    let _unrelated_hash = unrelated.block_hash();
    let mut share_c = TestShareBlockBuilder::new()
        .prev_share_blockhash(unrelated.block_hash().to_string())
        .nonce(3)
        .build()
        .header;
    share_c.bits = CompactTarget::from_consensus(MAX_POOL_TARGET);

    let mut chain_store_handle = ChainStoreHandle::default();
    // Anchor parent exists in store, unrelated parent does not
    chain_store_handle
        .expect_get_block_metadata()
        .returning(move |hash| {
            if *hash == anchor_hash {
                Ok(BlockMetadata {
                    expected_height: Some(0),
                    chain_work: Work::from_hex("0x00").unwrap(),
                    status: Status::BlockValid,
                    chain: ChainMembership::Confirmed,
                })
            } else {
                Err(StoreError::NotFound(format!("{hash} not found")))
            }
        });
    chain_store_handle
        .expect_get_block_metadata_batch()
        .returning(move |hashes| {
            Ok(hashes
                .iter()
                .filter(|hash| **hash == anchor_hash)
                .map(|hash| {
                    (
                        *hash,
                        BlockMetadata {
                            expected_height: Some(0),
                            chain_work: Work::from_hex("0x00").unwrap(),
                            status: Status::BlockValid,
                            chain: ChainMembership::Confirmed,
                        },
                    )
                })
                .collect())
        });
    let template_header = build_valid_test_header();
    chain_store_handle
        .expect_get_share_header()
        .returning(move |_| Ok(template_header.clone()));
    let mut mock_validator = MockDefaultShareValidator::default();
    setup_minimum_difficulty_mock(&mut mock_validator);

    let headers = vec![share_a, share_c];
    let result = validate_header_chain(
        &share_header_batch_with_empty_branches(headers),
        &chain_store_handle,
        &mock_validator,
    );
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("External parent(s) not found in store"),
        "Expected external parent not found error for disconnected subtree"
    );
}

/// A fork from the anchor is valid DAG behavior -- it's a sibling
/// branch that will be stored and may become an uncle later.
#[test]
fn test_validate_header_chain_accepts_fork_from_anchor() {
    let anchor = TestShareBlockBuilder::new().nonce(1).build();
    let mut share_a = TestShareBlockBuilder::new()
        .prev_share_blockhash(anchor.block_hash().to_string())
        .nonce(2)
        .build()
        .header;
    share_a.bits = CompactTarget::from_consensus(MAX_POOL_TARGET);
    let mut share_b = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_a.block_hash().to_string())
        .nonce(3)
        .build()
        .header;
    share_b.bits = CompactTarget::from_consensus(MAX_POOL_TARGET);
    let mut share_c = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_b.block_hash().to_string())
        .nonce(4)
        .build()
        .header;
    share_c.bits = CompactTarget::from_consensus(MAX_POOL_TARGET);
    // share_d forks from anchor -- valid DAG member
    let mut share_d = TestShareBlockBuilder::new()
        .prev_share_blockhash(anchor.block_hash().to_string())
        .nonce(5)
        .build()
        .header;
    share_d.bits = CompactTarget::from_consensus(MAX_POOL_TARGET);

    let mut chain_store_handle = ChainStoreHandle::default();
    setup_chain_validation_mocks(&mut chain_store_handle);
    let mut mock_validator = MockDefaultShareValidator::default();
    setup_minimum_difficulty_mock(&mut mock_validator);

    let headers = vec![share_a, share_b, share_c, share_d];
    let result = validate_header_chain(
        &share_header_batch_with_empty_branches(headers),
        &chain_store_handle,
        &mock_validator,
    );
    assert!(result.is_ok(), "Expected Ok, got: {}", result.unwrap_err());
}

#[test]
fn test_validate_header_chain_accepts_uncle_referencing_anchor() {
    let anchor = TestShareBlockBuilder::new().nonce(1).build();
    let mut share_a = TestShareBlockBuilder::new()
        .prev_share_blockhash(anchor.block_hash().to_string())
        .nonce(2)
        .build()
        .header;
    share_a.bits = CompactTarget::from_consensus(MAX_POOL_TARGET);
    let mut share_b = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_a.block_hash().to_string())
        .nonce(3)
        .build()
        .header;
    share_b.bits = CompactTarget::from_consensus(MAX_POOL_TARGET);
    // uncle has parent=anchor, which remains in the recent_confirmed
    // window throughout this batch.
    let mut uncle = TestShareBlockBuilder::new()
        .prev_share_blockhash(anchor.block_hash().to_string())
        .nonce(4)
        .build()
        .header;
    uncle.bits = CompactTarget::from_consensus(MAX_POOL_TARGET);
    let mut share_c = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_b.block_hash().to_string())
        .uncles(vec![uncle.block_hash()])
        .nonce(5)
        .build()
        .header;
    share_c.bits = CompactTarget::from_consensus(MAX_POOL_TARGET);

    let mut chain_store_handle = ChainStoreHandle::default();
    setup_chain_validation_mocks(&mut chain_store_handle);
    let mut mock_validator = MockDefaultShareValidator::default();
    setup_minimum_difficulty_mock(&mut mock_validator);

    let headers = vec![share_a, share_b, uncle, share_c];
    let result = validate_header_chain(
        &share_header_batch_with_empty_branches(headers),
        &chain_store_handle,
        &mock_validator,
    );
    assert!(result.is_ok(), "Expected Ok, got: {}", result.unwrap_err());
}

#[test]
fn test_validate_header_chain_accepts_uncle_subtree_before_nephew() {
    // Confirmed chain is A -> E -> H. B, C, D are HeaderValid uncles
    // (all parent=A) referenced by E. F, G are HeaderValid uncles (all
    // parent=E) referenced by H. Sync order: B, C, D, E, F, G, H.
    let anchor = TestShareBlockBuilder::new().nonce(1).build();

    let mut share_b = TestShareBlockBuilder::new()
        .prev_share_blockhash(anchor.block_hash().to_string())
        .nonce(2)
        .build()
        .header;
    share_b.bits = CompactTarget::from_consensus(MAX_POOL_TARGET);
    let mut share_c = TestShareBlockBuilder::new()
        .prev_share_blockhash(anchor.block_hash().to_string())
        .nonce(3)
        .build()
        .header;
    share_c.bits = CompactTarget::from_consensus(MAX_POOL_TARGET);
    let mut share_d = TestShareBlockBuilder::new()
        .prev_share_blockhash(anchor.block_hash().to_string())
        .nonce(4)
        .build()
        .header;
    share_d.bits = CompactTarget::from_consensus(MAX_POOL_TARGET);

    let mut share_e = TestShareBlockBuilder::new()
        .prev_share_blockhash(anchor.block_hash().to_string())
        .uncles(vec![
            share_b.block_hash(),
            share_c.block_hash(),
            share_d.block_hash(),
        ])
        .nonce(5)
        .build()
        .header;
    share_e.bits = CompactTarget::from_consensus(MAX_POOL_TARGET);

    let mut share_f = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_e.block_hash().to_string())
        .nonce(6)
        .build()
        .header;
    share_f.bits = CompactTarget::from_consensus(MAX_POOL_TARGET);
    let mut share_g = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_e.block_hash().to_string())
        .nonce(7)
        .build()
        .header;
    share_g.bits = CompactTarget::from_consensus(MAX_POOL_TARGET);

    let mut share_h = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_e.block_hash().to_string())
        .uncles(vec![share_f.block_hash(), share_g.block_hash()])
        .nonce(8)
        .build()
        .header;
    share_h.bits = CompactTarget::from_consensus(MAX_POOL_TARGET);

    let mut chain_store_handle = ChainStoreHandle::default();
    setup_chain_validation_mocks(&mut chain_store_handle);
    let mut mock_validator = MockDefaultShareValidator::default();
    setup_minimum_difficulty_mock(&mut mock_validator);

    let headers = vec![
        share_b, share_c, share_d, share_e, share_f, share_g, share_h,
    ];
    let result = validate_header_chain(
        &share_header_batch_with_empty_branches(headers),
        &chain_store_handle,
        &mock_validator,
    );
    assert!(result.is_ok(), "Expected Ok, got: {}", result.unwrap_err());
}

/// Reproduces a sync issue: uncle header declares a confirmed
/// block as its own uncle. Previously collect_declared_uncles
/// added the confirmed block to the uncle set, causing it to be
/// skipped during chain-linkage validation, breaking sync.
///
/// Scenario (from testnet4 debugging):
///   Confirmed chain: anchor -> A(h:1) -> B(h:2) -> C(h:3)
///   Fork: anchor -> fork_parent(h:1) -> U(h:2)
///   U declares A(h:1) as its own uncle (valid: A is at ancestor height).
///   C declares U as uncle.
///   Batch order: A, fork_parent, B, U, C
///
/// Old behaviour: A is in declared_uncles (via U's .uncles), gets
/// classified as uncle, B fails because parent A was skipped.
/// New behaviour: DAG connectivity passes because all parents are
/// in the batch or store.
#[test]
fn test_validate_header_chain_accepts_uncle_declaring_confirmed_block_as_its_uncle() {
    let anchor = TestShareBlockBuilder::new().nonce(1).build();

    // A: confirmed at h:1
    let mut share_a = TestShareBlockBuilder::new()
        .prev_share_blockhash(anchor.block_hash().to_string())
        .nonce(2)
        .build()
        .header;
    share_a.bits = CompactTarget::from_consensus(MAX_POOL_TARGET);

    // fork_parent: fork block at h:1 (same parent as A)
    let mut fork_parent = TestShareBlockBuilder::new()
        .prev_share_blockhash(anchor.block_hash().to_string())
        .nonce(3)
        .build()
        .header;
    fork_parent.bits = CompactTarget::from_consensus(MAX_POOL_TARGET);

    // B: confirmed at h:2
    let mut share_b = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_a.block_hash().to_string())
        .nonce(4)
        .build()
        .header;
    share_b.bits = CompactTarget::from_consensus(MAX_POOL_TARGET);

    // U: uncle at h:2 (parent=fork_parent), declares A as its own uncle
    let mut uncle_u = TestShareBlockBuilder::new()
        .prev_share_blockhash(fork_parent.block_hash().to_string())
        .uncles(vec![share_a.block_hash()])
        .nonce(5)
        .build()
        .header;
    uncle_u.bits = CompactTarget::from_consensus(MAX_POOL_TARGET);

    // C: confirmed at h:3, declares U as uncle
    let mut share_c = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_b.block_hash().to_string())
        .uncles(vec![uncle_u.block_hash()])
        .nonce(6)
        .build()
        .header;
    share_c.bits = CompactTarget::from_consensus(MAX_POOL_TARGET);

    let mut chain_store_handle = ChainStoreHandle::default();
    setup_chain_validation_mocks(&mut chain_store_handle);
    let mut mock_validator = MockDefaultShareValidator::default();
    setup_minimum_difficulty_mock(&mut mock_validator);

    // Batch order: A, fork_parent, B, U, C
    let headers = vec![share_a, fork_parent, share_b, uncle_u, share_c];
    let result = validate_header_chain(
        &share_header_batch_with_empty_branches(headers),
        &chain_store_handle,
        &mock_validator,
    );
    assert!(result.is_ok(), "Expected Ok, got: {}", result.unwrap_err());
}

/// Multiple anchors: two headers in the batch have different
/// external parents, both present in the store. This models a
/// height-based batch where blocks at the first height have
/// parents on different branches from the previous batch.
///
/// anchor_a(h:0) -> share_a(h:1)
/// anchor_b(h:0) -> share_b(h:1)
///
/// Both anchor_a and anchor_b are external parents in the store.
#[test]
fn test_validate_header_chain_accepts_multiple_anchors() {
    let anchor_a = TestShareBlockBuilder::new().nonce(1).build();
    let anchor_b = TestShareBlockBuilder::new().nonce(2).build();
    let anchor_a_hash = anchor_a.block_hash();
    let anchor_b_hash = anchor_b.block_hash();

    let mut share_a = TestShareBlockBuilder::new()
        .prev_share_blockhash(anchor_a.block_hash().to_string())
        .nonce(10)
        .build()
        .header;
    share_a.bits = CompactTarget::from_consensus(MAX_POOL_TARGET);

    let mut share_b = TestShareBlockBuilder::new()
        .prev_share_blockhash(anchor_b.block_hash().to_string())
        .nonce(20)
        .build()
        .header;
    share_b.bits = CompactTarget::from_consensus(MAX_POOL_TARGET);

    let mut chain_store_handle = ChainStoreHandle::default();
    let template_header = build_valid_test_header();
    // Each anchor's own header, read for the median time past walk.
    let anchor_a_header = anchor_a.header.clone();
    let anchor_b_header = anchor_b.header.clone();
    chain_store_handle
        .expect_get_share_header()
        .withf(move |hash| *hash == anchor_a_hash || *hash == anchor_b_hash)
        .returning(move |hash| {
            if *hash == anchor_a_hash {
                Ok(anchor_a_header.clone())
            } else {
                Ok(anchor_b_header.clone())
            }
        });
    chain_store_handle
        .expect_get_asert_parent()
        .withf(move |hash| *hash == anchor_a_hash || *hash == anchor_b_hash)
        .returning(move |_| {
            Ok(AsertParent::from_header(
                &template_header,
                0,
                template_header.time,
            ))
        });
    chain_store_handle
        .expect_get_block_metadata()
        .returning(move |hash| {
            if *hash == anchor_a_hash || *hash == anchor_b_hash {
                Ok(BlockMetadata {
                    expected_height: Some(0),
                    chain_work: Work::from_hex("0x00").unwrap(),
                    status: Status::BlockValid,
                    chain: ChainMembership::Confirmed,
                })
            } else {
                Err(StoreError::NotFound(format!("{hash} not found")))
            }
        });
    chain_store_handle
        .expect_get_block_metadata_batch()
        .returning(move |hashes| {
            Ok(hashes
                .iter()
                .filter(|hash| **hash == anchor_a_hash || **hash == anchor_b_hash)
                .map(|hash| {
                    (
                        *hash,
                        BlockMetadata {
                            expected_height: Some(0),
                            chain_work: Work::from_hex("0x00").unwrap(),
                            status: Status::BlockValid,
                            chain: ChainMembership::Confirmed,
                        },
                    )
                })
                .collect())
        });
    let mut mock_validator = MockDefaultShareValidator::default();
    setup_minimum_difficulty_mock(&mut mock_validator);

    let headers = vec![share_a, share_b];
    let result = validate_header_chain(
        &share_header_batch_with_empty_branches(headers),
        &chain_store_handle,
        &mock_validator,
    );
    assert!(result.is_ok(), "Expected Ok, got: {}", result.unwrap_err());
}

/// Missing external parent returns a retry depth.
#[test]
fn test_missing_external_parent_has_retry_depth() {
    let error = HeaderSyncError::MissingExternalParents {
        missing: vec![BlockHash::all_zeros()],
        lowest_anchor_height: 500,
    };
    // tip=600, target=500-100=400, depth=600-400=200
    assert_eq!(error.retry_depth(600), Some(200));
}

/// Missing uncle returns a retry depth.
#[test]
fn test_missing_uncle_has_retry_depth() {
    let error = HeaderSyncError::MissingUncle {
        uncle_hash: BlockHash::all_zeros(),
        lowest_anchor_height: 300,
    };
    // tip=400, target=300-100=200, depth=400-200=200
    assert_eq!(error.retry_depth(400), Some(200));
}

/// Missing parent in batch returns a retry depth.
#[test]
fn test_missing_parent_in_batch_has_retry_depth() {
    let error = HeaderSyncError::MissingParentInBatch {
        header: BlockHash::all_zeros(),
        parent: BlockHash::all_zeros(),
        lowest_anchor_height: 50,
    };
    // tip=100, target=50-100=0 (saturating), depth=100-0=100
    assert_eq!(error.retry_depth(100), Some(100));
}

/// ASERT mismatch has no retry depth.
#[test]
fn test_asert_mismatch_has_no_retry_depth() {
    let error = HeaderSyncError::AsertMismatch {
        block_hash: BlockHash::all_zeros(),
        declared: 0,
        expected: 1,
    };
    assert_eq!(error.retry_depth(600), None);
}

/// One mini share of work meets the mini chain's minimum; before, a mini
/// chain needed the work of a whole main share (256 mini shares).
#[test]
fn mini_chain_minimum_work_is_one_mini_share() {
    let mini_ceiling = CompactTarget::from_consensus(MINI_ANCHOR_TARGET);
    let one_mini_share = Target::from_compact(mini_ceiling).to_work();
    let zero_work = Work::from_hex("0x00").unwrap();
    assert!(validate_cumulative_work(zero_work, one_mini_share, mini_ceiling).is_ok());
}

#[test]
fn mini_chain_rejects_a_batch_with_no_work() {
    let mini_ceiling = CompactTarget::from_consensus(MINI_ANCHOR_TARGET);
    let zero_work = Work::from_hex("0x00").unwrap();
    assert!(matches!(
        validate_cumulative_work(zero_work, zero_work, mini_ceiling),
        Err(HeaderSyncError::InsufficientWork)
    ));
}

/// The main chain keeps its threshold: one mini share of work is not a
/// main share.
#[test]
fn main_chain_still_rejects_one_mini_share_of_work() {
    let main_ceiling = CompactTarget::from_consensus(MAX_POOL_TARGET);
    let one_mini_share =
        Target::from_compact(CompactTarget::from_consensus(MINI_ANCHOR_TARGET)).to_work();
    let zero_work = Work::from_hex("0x00").unwrap();
    assert!(matches!(
        validate_cumulative_work(zero_work, one_mini_share, main_ceiling),
        Err(HeaderSyncError::InsufficientWork)
    ));
}

/// Insufficient work has no retry depth.
#[test]
fn test_insufficient_work_has_no_retry_depth() {
    let error = HeaderSyncError::InsufficientWork;
    assert_eq!(error.retry_depth(600), None);
}

/// When handle_share_headers gets a missing external parent error,
/// it sends a deeper getheaders instead of propagating the error.
#[tokio::test]
async fn test_handle_share_headers_retries_on_missing_external_parent() {
    let peer_id = libp2p::PeerId::random();
    let unknown_parent = TestShareBlockBuilder::new().nonce(99).build();

    let mut share = TestShareBlockBuilder::new()
        .prev_share_blockhash(unknown_parent.block_hash().to_string())
        .nonce(10)
        .build()
        .header;
    share.bits = CompactTarget::from_consensus(MAX_POOL_TARGET);

    let mut chain_store_handle = ChainStoreHandle::default();
    // External parent not found in store
    chain_store_handle
        .expect_get_block_metadata_batch()
        .returning(|_| Ok(Vec::new()));
    // candidate tip height for computing retry depth, matching the base
    // build_locator subtracts the depth from
    chain_store_handle
        .expect_get_candidate_tip_height()
        .returning(|| Ok(Some(100)));
    // build_locator for the retry getheaders
    chain_store_handle
        .expect_build_locator()
        .return_once(|_| Ok(vec![BlockHash::all_zeros()]));
    let mut mock_validator = MockDefaultShareValidator::default();
    setup_minimum_difficulty_mock(&mut mock_validator);

    let (swarm_tx, mut swarm_rx) = mpsc::channel::<SwarmSend<oneshot::Sender<Message>>>(32);
    let (block_fetcher_handle, _block_fetcher_rx) = block_fetcher::create_block_fetcher_channel();

    let result = handle_share_headers(
        peer_id,
        share_header_batch_with_empty_branches(vec![share]),
        chain_store_handle,
        swarm_tx,
        block_fetcher_handle,
        &mock_validator,
    )
    .await;

    // Should succeed (retry sent, not an error)
    assert!(result.is_ok());

    // Should have sent a getheaders request
    let message = swarm_rx.try_recv().expect("expected a getheaders retry");
    match message {
        SwarmSend::Request(sent_peer, Message::GetShareHeaders(_, _)) => {
            assert_eq!(sent_peer, peer_id);
        }
        other => panic!("expected GetShareHeaders, got: {other:?}"),
    }
}

/// When handle_share_headers gets an ASERT mismatch, it propagates
/// the error instead of retrying.
#[tokio::test]
async fn test_handle_share_headers_propagates_asert_mismatch() {
    let peer_id = libp2p::PeerId::random();
    let mut chain_store_handle = ChainStoreHandle::default();
    setup_chain_validation_mocks(&mut chain_store_handle);

    let (swarm_tx, _swarm_rx) = mpsc::channel::<SwarmSend<oneshot::Sender<Message>>>(32);
    let (block_fetcher_handle, _block_fetcher_rx) = block_fetcher::create_block_fetcher_channel();

    let mut mock_validator = MockDefaultShareValidator::default();
    setup_minimum_difficulty_mock(&mut mock_validator);

    // Header with wrong bits -- will trigger ASERT mismatch
    let header = TestShareBlockBuilder::new().build().header;
    let share_headers = vec![header];

    let result = handle_share_headers(
        peer_id,
        share_header_batch_with_empty_branches(share_headers),
        chain_store_handle,
        swarm_tx,
        block_fetcher_handle,
        &mock_validator,
    )
    .await;

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("ASERT mismatch"));
}

/// Batch has an external parent that is completely unknown to the
/// receiver. This can happen if the sender's DAG changed between
/// batches. The batch should be rejected.
#[test]
fn test_validate_header_chain_rejects_batch_with_unknown_external_parent() {
    let known_anchor = TestShareBlockBuilder::new().nonce(1).build();
    let unknown_anchor = TestShareBlockBuilder::new().nonce(99).build();
    let known_anchor_hash = known_anchor.block_hash();

    let mut share_a = TestShareBlockBuilder::new()
        .prev_share_blockhash(known_anchor.block_hash().to_string())
        .nonce(10)
        .build()
        .header;
    share_a.bits = CompactTarget::from_consensus(MAX_POOL_TARGET);

    let mut share_b = TestShareBlockBuilder::new()
        .prev_share_blockhash(unknown_anchor.block_hash().to_string())
        .nonce(20)
        .build()
        .header;
    share_b.bits = CompactTarget::from_consensus(MAX_POOL_TARGET);

    let mut chain_store_handle = ChainStoreHandle::default();
    chain_store_handle
        .expect_get_block_metadata_batch()
        .returning(move |hashes| {
            Ok(hashes
                .iter()
                .filter(|hash| **hash == known_anchor_hash)
                .map(|hash| {
                    (
                        *hash,
                        BlockMetadata {
                            expected_height: Some(0),
                            chain_work: Work::from_hex("0x00").unwrap(),
                            status: Status::BlockValid,
                            chain: ChainMembership::Confirmed,
                        },
                    )
                })
                .collect())
        });
    let mut mock_validator = MockDefaultShareValidator::default();
    setup_minimum_difficulty_mock(&mut mock_validator);

    let headers = vec![share_a, share_b];
    let result = validate_header_chain(
        &share_header_batch_with_empty_branches(headers),
        &chain_store_handle,
        &mock_validator,
    );
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("External parent(s) not found in store"),
    );
}
