use super::*;
use crate::node::SwarmSend;
use crate::node::messages::InventoryMessage;
use crate::node::p2p_message_handlers::receivers::block_receiver::create_block_receiver_channel;
use crate::node::request_response_handler::block_fetcher::BlockFetcherHandle;
use crate::node::request_response_handler::block_fetcher::create_block_fetcher_channel;
use crate::node::validation_worker::ValidationSender;
use crate::node::validation_worker::create_validation_channel;
#[mockall_double::double]
use crate::pool_difficulty::PoolDifficulty;
#[mockall_double::double]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
use crate::shares::share_block::Txids;
use crate::shares::validation::MockDefaultShareValidator;
use crate::test_utils::setup_header_chain_validation_mocks;
use crate::test_utils::share_header_batch_with_empty_branches;
use crate::test_utils::{
    TestShareBlockBuilder, test_coinbase_transaction, valid_share_block_from_fixture,
};
use crate::utils::time_provider::TestTimeProvider;
use bitcoin::hashes::Hash as _;
use bitcoin::{BlockHash, CompactTarget};
use mockall::predicate::*;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

/// Create test block fetcher, validation, and block receiver handles for handle_response tests.
fn test_handles() -> (BlockFetcherHandle, ValidationSender, BlockReceiverHandle) {
    let (block_fetcher_tx, _) = create_block_fetcher_channel();
    let (validation_tx, _) = create_validation_channel();
    let (block_receiver_handle, _) = create_block_receiver_channel();
    (block_fetcher_tx, validation_tx, block_receiver_handle)
}

#[tokio::test]
async fn test_handle_request_getheaders() {
    let peer_id = libp2p::PeerId::random();
    let (swarm_tx, mut swarm_rx) = mpsc::channel(32);
    let response_channel = 1u32;
    let mut chain_store_handle = ChainStoreHandle::default();
    let time_provider = TestTimeProvider::new(SystemTime::now());
    let (block_fetcher_handle, validation_tx, block_receiver_handle) = test_handles();

    // Mock the response headers
    let block1 = TestShareBlockBuilder::new().build();
    let block2 = TestShareBlockBuilder::new().build();

    let block_hashes = vec![block1.block_hash()];
    let stop_block_hash = block2.block_hash();

    let response_headers = vec![block1.header.clone(), block2.header.clone()];

    chain_store_handle
        .expect_get_headers_for_locator()
        .returning(move |_, _, _| Ok(response_headers.clone()));

    chain_store_handle
        .expect_get_template_merkle_branches()
        .returning(|_| Ok(Vec::new()));

    let ctx = RequestContext {
        peer: peer_id,
        request: Message::GetShareHeaders(block_hashes, stop_block_hash),
        chain_store_handle,
        response_channel,
        swarm_tx,
        time_provider,
        block_fetcher_handle,
        validation_tx,
        block_receiver_handle: block_receiver_handle.clone(),
        share_validator: Arc::new(MockDefaultShareValidator::default()),
        share_transaction_pool: None,
    };

    let result = handle_request(ctx).await;
    assert!(result.is_ok());

    // Verify swarm message
    if let Some(SwarmSend::Response(channel, Message::ShareHeaders(headers))) =
        swarm_rx.recv().await
    {
        assert_eq!(channel, response_channel);
        assert_eq!(
            headers.headers().to_vec(),
            vec![block1.header, block2.header]
        );
    } else {
        panic!("Expected SwarmSend::Response with ShareHeaders message");
    }
}

#[tokio::test]
async fn test_handle_get_share_blocks() {
    let peer_id = libp2p::PeerId::random();
    let (swarm_tx, mut swarm_rx) = mpsc::channel(32);
    let (response_channel, _response_channel_rx) = oneshot::channel::<Message>();
    let mut chain_store_handle = ChainStoreHandle::default();
    let time_provider = TestTimeProvider::new(SystemTime::now());
    let (block_fetcher_handle, validation_tx, block_receiver_handle) = test_handles();

    // Create test blocks that will be returned
    let block1 = TestShareBlockBuilder::new().build();
    let block2 = TestShareBlockBuilder::new().build();

    let block_hashes: Vec<BlockHash> = vec![block1.block_hash(), block2.block_hash()];
    let stop_block_hash = block2.block_hash();

    // Set up mock expectations
    chain_store_handle
        .expect_get_blockhashes_for_locator()
        .returning(move |_, _, _| Ok(vec![block1.block_hash(), block2.block_hash()]));

    let ctx = RequestContext {
        peer: peer_id,
        request: Message::GetShareBlocks(block_hashes.clone(), stop_block_hash),
        chain_store_handle,
        response_channel,
        swarm_tx,
        time_provider,
        block_fetcher_handle,
        validation_tx,
        block_receiver_handle: block_receiver_handle.clone(),
        share_validator: Arc::new(MockDefaultShareValidator::default()),
        share_transaction_pool: None,
    };

    let result = handle_request(ctx).await;

    assert!(result.is_ok());

    // Verify swarm message
    if let Some(SwarmSend::Response(_, Message::Inventory(InventoryMessage::BlockHashes(hashes)))) =
        swarm_rx.recv().await
    {
        assert_eq!(hashes, block_hashes);
    } else {
        panic!("Expected SwarmSend::Response with Inventory message");
    }
}

#[tokio::test]
async fn test_handle_request_inventory_for_blocks() {
    let peer_id = libp2p::PeerId::random();
    let (swarm_tx, mut swarm_rx) = mpsc::channel(32);
    let (response_channel_tx, _response_channel_rx) = oneshot::channel::<Message>();
    let mut chain_store_handle = ChainStoreHandle::default();
    let time_provider = TestTimeProvider::new(SystemTime::now());
    let (block_fetcher_handle, validation_tx, block_receiver_handle) = test_handles();

    let block_hash1 = "0000000000000000000000000000000000000000000000000000000000000001"
        .parse::<BlockHash>()
        .unwrap();
    let block_hash2 = "0000000000000000000000000000000000000000000000000000000000000002"
        .parse::<BlockHash>()
        .unwrap();

    let block_hashes = vec![block_hash1, block_hash2];
    let missing = vec![block_hash1];

    chain_store_handle.expect_is_current().returning(|| true);
    chain_store_handle
        .expect_get_missing_blockhashes()
        .returning(move |_| missing.clone());
    chain_store_handle
        .expect_build_locator()
        .return_once(|_| Ok(vec![BlockHash::all_zeros()]));

    let inventory = InventoryMessage::BlockHashes(block_hashes);

    let ctx = RequestContext {
        peer: peer_id,
        request: Message::Inventory(inventory),
        chain_store_handle,
        response_channel: response_channel_tx,
        swarm_tx,
        time_provider,
        block_fetcher_handle,
        validation_tx,
        block_receiver_handle: block_receiver_handle.clone(),
        share_validator: Arc::new(MockDefaultShareValidator::default()),
        share_transaction_pool: None,
    };

    let result = handle_request(ctx).await;
    assert!(result.is_ok());

    if let Some(SwarmSend::Response(_, Message::Ack)) = swarm_rx.recv().await {
    } else {
        panic!("Expected SwarmSend::Response with Ack message");
    }

    if let Some(SwarmSend::Request(sent_peer, Message::GetShareHeaders(_, _))) =
        swarm_rx.recv().await
    {
        assert_eq!(sent_peer, peer_id);
    } else {
        panic!("Expected SwarmSend::Request with GetShareHeaders message");
    }

    assert!(
        swarm_rx.try_recv().is_err(),
        "No additional messages expected"
    );
}

#[tokio::test]
async fn test_handle_request_inventory_for_txns() {
    let peer_id = libp2p::PeerId::random();
    let (swarm_tx, mut swarm_rx) = mpsc::channel(32);
    let (response_channel_tx, _response_channel_rx) = oneshot::channel::<Message>();
    let chain_store_handle = ChainStoreHandle::default();
    let time_provider = TestTimeProvider::new(SystemTime::now());
    let (block_fetcher_handle, validation_tx, block_receiver_handle) = test_handles();

    let tx_hashes: Vec<bitcoin::Txid> = vec![
        "0000000000000000000000000000000000000000000000000000000000000001"
            .parse()
            .unwrap(),
        "0000000000000000000000000000000000000000000000000000000000000002"
            .parse()
            .unwrap(),
    ];
    let inventory = InventoryMessage::TransactionHashes(Txids(tx_hashes));

    let ctx = RequestContext {
        peer: peer_id,
        request: Message::Inventory(inventory),
        chain_store_handle,
        response_channel: response_channel_tx,
        swarm_tx,
        time_provider,
        block_fetcher_handle,
        validation_tx,
        block_receiver_handle: block_receiver_handle.clone(),
        share_validator: Arc::new(MockDefaultShareValidator::default()),
        share_transaction_pool: None,
    };

    let result = handle_request(ctx).await;
    assert!(result.is_ok());

    if let Some(SwarmSend::Response(_, Message::Ack)) = swarm_rx.recv().await {
    } else {
        panic!("Expected SwarmSend::Response with Ack message");
    }

    assert!(
        swarm_rx.try_recv().is_err(),
        "No additional messages expected for TransactionHashes inventory"
    );
}

#[tokio::test]
async fn test_handle_request_not_found() {
    let peer_id = libp2p::PeerId::random();
    let (swarm_tx, _swarm_rx) = mpsc::channel(32);
    let (response_channel_tx, _response_channel_rx) = oneshot::channel::<Message>();
    let chain_store_handle = ChainStoreHandle::default();
    let time_provider = TestTimeProvider::new(SystemTime::now());
    let (block_fetcher_handle, validation_tx, block_receiver_handle) = test_handles();

    let ctx = RequestContext {
        peer: peer_id,
        request: Message::NotFound(GetData::Block(BlockHash::all_zeros())),
        chain_store_handle,
        response_channel: response_channel_tx,
        swarm_tx,
        time_provider,
        block_fetcher_handle,
        validation_tx,
        block_receiver_handle: block_receiver_handle.clone(),
        share_validator: Arc::new(MockDefaultShareValidator::default()),
        share_transaction_pool: None,
    };

    let result = handle_request(ctx).await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_handle_request_get_data_for_block_confirmed() {
    let peer_id = libp2p::PeerId::random();
    let (swarm_tx, mut swarm_rx) = mpsc::channel(32);
    let response_channel = 1u32;
    let mut chain_store_handle = ChainStoreHandle::default();
    let time_provider = TestTimeProvider::new(SystemTime::now());
    let (block_fetcher_handle, validation_tx, block_receiver_handle) = test_handles();

    let block = TestShareBlockBuilder::new().build();
    let block_hash = block.block_hash();
    let expected_block = block.clone();

    chain_store_handle
        .expect_get_share()
        .returning(move |_| Some(block.clone()));
    chain_store_handle
        .expect_is_confirmed_or_confirmed_uncle()
        .returning(|_| true);

    let get_data = GetData::Block(block_hash);

    let ctx = RequestContext {
        peer: peer_id,
        request: Message::GetData(get_data),
        chain_store_handle,
        response_channel,
        swarm_tx,
        time_provider,
        block_fetcher_handle,
        validation_tx,
        block_receiver_handle: block_receiver_handle.clone(),
        share_validator: Arc::new(MockDefaultShareValidator::default()),
        share_transaction_pool: None,
    };

    let result = handle_request(ctx).await;
    assert!(result.is_ok());

    if let Some(SwarmSend::Response(channel, Message::ShareBlock(share_block))) =
        swarm_rx.recv().await
    {
        assert_eq!(channel, response_channel);
        assert_eq!(share_block, expected_block);
    } else {
        panic!("Expected SwarmSend::Response with ShareBlock message");
    }
}

#[tokio::test]
async fn test_handle_request_get_data_for_block_not_found() {
    let peer_id = libp2p::PeerId::random();
    let (swarm_tx, mut swarm_rx) = mpsc::channel(32);
    let response_channel = 1u32;
    let mut chain_store_handle = ChainStoreHandle::default();
    let time_provider = TestTimeProvider::new(SystemTime::now());
    let (block_fetcher_handle, validation_tx, block_receiver_handle) = test_handles();

    let block_hash = "0000000000000000000000000000000000000000000000000000000000000001"
        .parse::<BlockHash>()
        .unwrap();

    chain_store_handle.expect_get_share().returning(|_| None);

    let get_data = GetData::Block(block_hash);

    let ctx = RequestContext {
        peer: peer_id,
        request: Message::GetData(get_data),
        chain_store_handle,
        response_channel,
        swarm_tx,
        time_provider,
        block_fetcher_handle,
        validation_tx,
        block_receiver_handle: block_receiver_handle.clone(),
        share_validator: Arc::new(MockDefaultShareValidator::default()),
        share_transaction_pool: None,
    };

    let result = handle_request(ctx).await;
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
async fn test_handle_request_get_data_for_txn() {
    let peer_id = libp2p::PeerId::random();
    let (swarm_tx, _swarm_rx) = mpsc::channel(32);
    let (response_channel_tx, _response_channel_rx) = oneshot::channel::<Message>();
    let chain_store_handle = ChainStoreHandle::default();
    let time_provider = TestTimeProvider::new(SystemTime::now());
    let (block_fetcher_handle, validation_tx, block_receiver_handle) = test_handles();

    // Test GetData message with txid
    let txid = "0000000000000000000000000000000000000000000000000000000000000001"
        .parse()
        .unwrap();
    let get_data = GetData::Txid(txid);

    let ctx = RequestContext {
        peer: peer_id,
        request: Message::GetData(get_data),
        chain_store_handle,
        response_channel: response_channel_tx,
        swarm_tx,
        time_provider,
        block_fetcher_handle,
        validation_tx,
        block_receiver_handle: block_receiver_handle.clone(),
        share_validator: Arc::new(MockDefaultShareValidator::default()),
        share_transaction_pool: None,
    };

    let result = handle_request(ctx).await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_handle_request_transaction() {
    let peer_id = libp2p::PeerId::random();
    let (swarm_tx, _swarm_rx) = mpsc::channel(32);
    let (response_channel_tx, _response_channel_rx) = oneshot::channel::<Message>();
    let chain_store_handle = ChainStoreHandle::default();
    let time_provider = TestTimeProvider::new(SystemTime::now());
    let (block_fetcher_handle, validation_tx, block_receiver_handle) = test_handles();

    // Create a test transaction
    let transaction = test_coinbase_transaction(1);

    let ctx = RequestContext {
        peer: peer_id,
        request: Message::Transaction(transaction),
        chain_store_handle,
        response_channel: response_channel_tx,
        swarm_tx,
        time_provider,
        block_fetcher_handle,
        validation_tx,
        block_receiver_handle: block_receiver_handle.clone(),
        share_validator: Arc::new(MockDefaultShareValidator::default()),
        share_transaction_pool: None,
    };

    let result = handle_request(ctx).await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_handle_request_share_headers() {
    let peer_id = libp2p::PeerId::random();
    let (swarm_tx, _swarm_rx) = mpsc::channel(32);
    let (response_channel_tx, _response_channel_rx) = oneshot::channel::<Message>();
    let mut chain_store_handle = ChainStoreHandle::default();
    let time_provider = TestTimeProvider::new(SystemTime::now());
    let (block_fetcher_handle, validation_tx, block_receiver_handle) = test_handles();

    // Create test share headers
    let block1 = TestShareBlockBuilder::new().build();
    let block2 = TestShareBlockBuilder::new().build();

    let share_headers = vec![block1.header.clone(), block2.header.clone()];

    // Set up mock expectations for processing headers
    chain_store_handle
        .expect_get_headers_for_locator()
        .returning(|_, _, _| Ok(vec![]));
    chain_store_handle
        .expect_get_template_merkle_branches()
        .returning(|_| Ok(Vec::new()));

    let ctx = RequestContext {
        peer: peer_id,
        request: Message::ShareHeaders(share_header_batch_with_empty_branches(share_headers)),
        chain_store_handle,
        response_channel: response_channel_tx,
        swarm_tx,
        time_provider,
        block_fetcher_handle,
        validation_tx,
        block_receiver_handle: block_receiver_handle.clone(),
        share_validator: Arc::new(MockDefaultShareValidator::default()),
        share_transaction_pool: None,
    };

    let result = handle_request(ctx).await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_handle_request_share_block_sends_ack_and_processes() {
    let peer_id = libp2p::PeerId::random();
    let (swarm_tx, mut swarm_rx) = mpsc::channel(32);
    let response_channel = 1u32;
    let mut chain_store_handle = ChainStoreHandle::default();
    let time_provider = TestTimeProvider::new(SystemTime::now());
    let (block_fetcher_handle, validation_tx, block_receiver_handle) = test_handles();

    let share_block = valid_share_block_from_fixture();

    // Block already confirmed -- simplest path through handle_share_block
    chain_store_handle
        .expect_share_block_exists()
        .returning(|_| true);
    chain_store_handle
        .expect_is_block_confirmed()
        .returning(|_| true);
    // Chain not current, so request_headers_for_missing_blocks is a no-op
    chain_store_handle.expect_is_current().returning(|| false);

    let ctx = RequestContext {
        peer: peer_id,
        request: Message::ShareBlock(share_block),
        chain_store_handle,
        response_channel,
        swarm_tx,
        time_provider,
        block_fetcher_handle,
        validation_tx,
        block_receiver_handle,
        share_validator: Arc::new(MockDefaultShareValidator::default()),
        share_transaction_pool: None,
    };

    let result = handle_request(ctx).await;
    assert!(result.is_ok());

    let ack_message = swarm_rx.recv().await.unwrap();
    match ack_message {
        SwarmSend::Response(channel, Message::Ack) => {
            assert_eq!(channel, 1u32);
        }
        _ => panic!("Expected SwarmSend::Response with Ack"),
    }

    assert!(
        swarm_rx.try_recv().is_err(),
        "No additional messages expected"
    );
}

#[tokio::test]
async fn test_handle_request_share_block_missing_parent_sends_getheaders() {
    let peer_id = libp2p::PeerId::random();
    let (swarm_tx, mut swarm_rx) = mpsc::channel(32);
    let response_channel = 5u32;
    let mut chain_store_handle = ChainStoreHandle::default();
    let time_provider = TestTimeProvider::new(SystemTime::now());
    let (block_fetcher_handle, validation_tx, block_receiver_handle) = test_handles();

    let share_block = valid_share_block_from_fixture();
    let parent_hash = share_block.header.prev_share_blockhash;

    // Block already confirmed -- simplest path through handle_share_block
    chain_store_handle
        .expect_share_block_exists()
        .returning(|_| true);
    chain_store_handle
        .expect_is_block_confirmed()
        .returning(|_| true);

    // Chain is current and parent is missing -- should trigger getheaders
    chain_store_handle.expect_is_current().returning(|| true);
    let missing = vec![parent_hash];
    chain_store_handle
        .expect_get_missing_blockhashes()
        .with(eq(vec![parent_hash]))
        .returning(move |_| missing.clone());
    chain_store_handle
        .expect_build_locator()
        .return_once(|_| Ok(vec![BlockHash::all_zeros()]));

    let ctx = RequestContext {
        peer: peer_id,
        request: Message::ShareBlock(share_block),
        chain_store_handle,
        response_channel,
        swarm_tx,
        time_provider,
        block_fetcher_handle,
        validation_tx,
        block_receiver_handle,
        share_validator: Arc::new(MockDefaultShareValidator::default()),
        share_transaction_pool: None,
    };

    let result = handle_request(ctx).await;
    assert!(result.is_ok());

    let ack_message = swarm_rx.recv().await.unwrap();
    match ack_message {
        SwarmSend::Response(channel, Message::Ack) => {
            assert_eq!(channel, 5u32);
        }
        _ => panic!("Expected SwarmSend::Response with Ack"),
    }

    let getheaders_message = swarm_rx.recv().await.unwrap();
    match getheaders_message {
        SwarmSend::Request(sent_peer, Message::GetShareHeaders(locator, stop_hash)) => {
            assert_eq!(sent_peer, peer_id);
            assert_eq!(locator, vec![BlockHash::all_zeros()]);
            assert_eq!(stop_hash, BlockHash::all_zeros());
        }
        _ => panic!("Expected SwarmSend::Request with GetShareHeaders"),
    }

    assert!(
        swarm_rx.try_recv().is_err(),
        "No additional messages expected"
    );
}

#[tokio::test]
async fn test_handle_response_share_headers() {
    let peer_id = libp2p::PeerId::random();
    let mut chain_store_handle = ChainStoreHandle::default();

    let mut mock_validator = MockDefaultShareValidator::default();
    mock_validator
        .expect_validate_header_minimum_difficulty()
        .returning(|_| Ok(()));
    mock_validator
        .expect_validate_coinbase_proof()
        .returning(|_, _| Ok(()));
    let mut pool_difficulty = PoolDifficulty::default();
    pool_difficulty
        .expect_calculate_target_clamped()
        .returning(|_| CompactTarget::from_consensus(crate::shares::share_block::MAX_POOL_TARGET));
    pool_difficulty
        .expect_target_ceiling()
        .return_const(CompactTarget::from_consensus(
            crate::shares::share_block::MAX_POOL_TARGET,
        ));
    mock_validator
        .expect_pool_difficulty()
        .return_const(pool_difficulty);

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
    setup_header_chain_validation_mocks(&mut chain_store_handle);

    let (swarm_tx, _swarm_rx) = mpsc::channel::<SwarmSend<oneshot::Sender<Message>>>(32);

    let mut header1 = TestShareBlockBuilder::new().build().header;
    header1.bits = CompactTarget::from_consensus(crate::shares::share_block::MAX_POOL_TARGET);
    let mut header2 = TestShareBlockBuilder::new()
        .nonce(0xe9695792) // doesn't matter, as we don't compare block hash to target
        .build()
        .header;
    header2.bits = CompactTarget::from_consensus(crate::shares::share_block::MAX_POOL_TARGET);
    header2.prev_share_blockhash = header1.block_hash();
    let share_headers = vec![header1, header2];

    let (block_fetcher_handle, validation_tx, block_receiver_handle) = test_handles();
    let result = handle_response(
        peer_id,
        Message::ShareHeaders(share_header_batch_with_empty_branches(share_headers)),
        chain_store_handle,
        swarm_tx,
        block_fetcher_handle,
        validation_tx,
        block_receiver_handle,
        Arc::new(mock_validator),
    )
    .await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_handle_response_not_found_notifies_block_fetcher() {
    let peer_id = libp2p::PeerId::random();
    let chain_store_handle = ChainStoreHandle::default();
    let (swarm_tx, _swarm_rx) = mpsc::channel::<SwarmSend<oneshot::Sender<Message>>>(32);

    let (block_fetcher_handle, mut block_fetcher_rx) = create_block_fetcher_channel();
    let (validation_tx, _) = create_validation_channel();
    let (block_receiver_handle, _) = create_block_receiver_channel();

    let block_hash = BlockHash::all_zeros();
    let result = handle_response(
        peer_id,
        Message::NotFound(GetData::Block(block_hash)),
        chain_store_handle,
        swarm_tx,
        block_fetcher_handle,
        validation_tx,
        block_receiver_handle,
        Arc::new(MockDefaultShareValidator::default()),
    )
    .await;

    assert!(result.is_ok());

    let event = block_fetcher_rx
        .try_recv()
        .expect("Expected BlockRequestCompleted event from NotFound handler");
    match event {
        BlockFetcherEvent::BlockRequestCompleted(hash) => {
            assert_eq!(hash, block_hash);
        }
        other => panic!("Expected BlockRequestCompleted, got: {other}"),
    }
}

#[tokio::test]
async fn test_handle_response_inventory() {
    let peer_id = libp2p::PeerId::random();
    let chain_store_handle = ChainStoreHandle::default();
    let (swarm_tx, _swarm_rx) = mpsc::channel::<SwarmSend<oneshot::Sender<Message>>>(32);

    let block_hashes = vec![
        "0000000000000000000000000000000000000000000000000000000000000001"
            .parse::<BlockHash>()
            .unwrap(),
    ];
    let inventory = InventoryMessage::BlockHashes(block_hashes);

    let (block_fetcher_handle, validation_tx, block_receiver_handle) = test_handles();
    let result = handle_response(
        peer_id,
        Message::Inventory(inventory),
        chain_store_handle,
        swarm_tx,
        block_fetcher_handle,
        validation_tx,
        block_receiver_handle,
        Arc::new(MockDefaultShareValidator::default()),
    )
    .await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_handle_response_unexpected_message() {
    let peer_id = libp2p::PeerId::random();
    let chain_store_handle = ChainStoreHandle::default();
    let (swarm_tx, _swarm_rx) = mpsc::channel::<SwarmSend<oneshot::Sender<Message>>>(32);

    let (block_fetcher_handle, validation_tx, block_receiver_handle) = test_handles();
    let result = handle_response(
        peer_id,
        Message::GetData(GetData::Block(BlockHash::all_zeros())),
        chain_store_handle,
        swarm_tx,
        block_fetcher_handle,
        validation_tx,
        block_receiver_handle,
        Arc::new(MockDefaultShareValidator::default()),
    )
    .await;

    assert!(result.is_ok());
}
