use super::*;
use crate::node::SwarmSend;
use crate::node::messages::{GetData, InventoryMessage, Message};
use crate::node::p2p_message_handlers::receivers::block_receiver::create_block_receiver_channel;
use crate::node::request_sender::MockRequestSender;
#[mockall_double::double]
use crate::pool_difficulty::PoolDifficulty;
use crate::service::PeerHandle;
#[mockall_double::double]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
use crate::shares::validation::MockDefaultShareValidator;
use crate::test_utils::{
    TestShareBlockBuilder, share_header_batch_with_empty_branches, valid_share_block_from_fixture,
};
use bitcoin::hashes::Hash as _;
use bitcoin::{BlockHash, CompactTarget};
use tokio::sync::mpsc;
use tokio::sync::oneshot;

type TestChannel = oneshot::Sender<Message>;

const TEST_RATE_LIMIT: u64 = 10;

fn build_test_handler(
    chain_store_handle: ChainStoreHandle,
    swarm_tx: mpsc::Sender<SwarmSend<TestChannel>>,
) -> RequestResponseHandler<TestChannel> {
    build_test_handler_with_validator(
        chain_store_handle,
        swarm_tx,
        Arc::new(MockDefaultShareValidator::default()),
    )
}

fn build_test_handler_with_validator(
    chain_store_handle: ChainStoreHandle,
    swarm_tx: mpsc::Sender<SwarmSend<TestChannel>>,
    share_validator: Arc<dyn ShareValidator + Send + Sync>,
) -> RequestResponseHandler<TestChannel> {
    let (handler, response_worker_rx) =
        build_test_handler_parts(chain_store_handle, swarm_tx, share_validator);
    // Drain the response worker channel so dispatch_response's try_send
    // succeeds in tests that only assert the dispatch result.
    tokio::spawn(async move {
        let mut response_worker_rx = response_worker_rx;
        while response_worker_rx.recv().await.is_some() {}
    });
    handler
}

/// Build a handler and return the response worker receiver so a test can
/// assert what `dispatch_response` enqueues.
fn build_test_handler_parts(
    chain_store_handle: ChainStoreHandle,
    swarm_tx: mpsc::Sender<SwarmSend<TestChannel>>,
    share_validator: Arc<dyn ShareValidator + Send + Sync>,
) -> (
    RequestResponseHandler<TestChannel>,
    crate::node::response_worker::ResponseWorkerReceiver,
) {
    let (block_fetcher_tx, _block_fetcher_rx) = block_fetcher::create_block_fetcher_channel();
    let (validation_tx, _validation_rx) =
        crate::node::validation_worker::create_validation_channel();
    let (block_receiver_handle, _block_receiver_rx) = create_block_receiver_channel();
    let (response_worker_handle, response_worker_rx) =
        crate::node::response_worker::create_response_worker_channel();
    let handler = RequestResponseHandler {
        peer_handles: HashMap::new(),
        max_requests_per_second: TEST_RATE_LIMIT,
        chain_store_handle,
        swarm_tx,
        block_fetcher_handle: block_fetcher_tx,
        validation_tx,
        block_receiver_handle,
        peer_block_knowledge: PeerBlockKnowledge::default(),
        share_validator,
        response_worker_handle,
        outbound_failures_total: 0,
        inbound_failures_total: 0,
        responses_dropped_total: 0,
        share_transaction_pool: None,
    };
    (handler, response_worker_rx)
}

#[tokio::test]
async fn test_dispatch_response_share_headers() {
    let (swarm_tx, _swarm_rx) = mpsc::channel(32);
    let mut chain_store_handle = ChainStoreHandle::default();

    chain_store_handle.expect_clone().returning(|| {
        let mut cloned = ChainStoreHandle::default();
        cloned.expect_organise_header().returning(|_| Ok(None));
        cloned
            .expect_find_fork_point_height()
            .returning(|_| Ok(Some(0)));
        cloned
            .expect_get_candidate_blocks_missing_data()
            .returning(|_| Ok(Vec::new()));
        crate::test_utils::setup_header_chain_validation_mocks(&mut cloned);
        cloned
    });

    let mut mock_validator = MockDefaultShareValidator::default();
    mock_validator
        .expect_validate_header_minimum_difficulty()
        .returning(|_| Ok(()));
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

    let mut handler =
        build_test_handler_with_validator(chain_store_handle, swarm_tx, Arc::new(mock_validator));

    let peer_id = libp2p::PeerId::random();
    let mut header1 = TestShareBlockBuilder::new().build().header;
    header1.bits = CompactTarget::from_consensus(crate::shares::share_block::MAX_POOL_TARGET);
    let mut header2 = TestShareBlockBuilder::new()
        .nonce(0xe9695792) // doesn't matter, as we don't compare block hash to target
        .build()
        .header;
    header2.bits = CompactTarget::from_consensus(crate::shares::share_block::MAX_POOL_TARGET);
    header2.prev_share_blockhash = header1.block_hash();
    let share_headers = vec![header1, header2];

    let result = handler.dispatch_response(
        peer_id,
        ConnectionId::new_unchecked(1),
        Message::ShareHeaders(share_header_batch_with_empty_branches(share_headers)),
    );

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_dispatch_response_not_found() {
    let (swarm_tx, _swarm_rx) = mpsc::channel(32);
    let mut chain_store_handle = ChainStoreHandle::default();
    chain_store_handle
        .expect_clone()
        .returning(ChainStoreHandle::default);

    let mut handler = build_test_handler(chain_store_handle, swarm_tx);

    let peer_id = libp2p::PeerId::random();

    let result = handler.dispatch_response(
        peer_id,
        ConnectionId::new_unchecked(1),
        Message::NotFound(GetData::Block(BlockHash::all_zeros())),
    );

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_dispatch_response_inventory() {
    let (swarm_tx, _swarm_rx) = mpsc::channel(32);
    let mut chain_store_handle = ChainStoreHandle::default();
    chain_store_handle
        .expect_clone()
        .returning(ChainStoreHandle::default);

    let mut handler = build_test_handler(chain_store_handle, swarm_tx);

    let peer_id = libp2p::PeerId::random();
    let block_hashes = vec![
        "0000000000000000000000000000000000000000000000000000000000000001"
            .parse::<BlockHash>()
            .unwrap(),
    ];
    let inventory = InventoryMessage::BlockHashes(block_hashes);

    let result = handler.dispatch_response(
        peer_id,
        ConnectionId::new_unchecked(1),
        Message::Inventory(inventory),
    );

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_dispatch_response_unexpected_message() {
    let (swarm_tx, _swarm_rx) = mpsc::channel(32);
    let mut chain_store_handle = ChainStoreHandle::default();
    chain_store_handle
        .expect_clone()
        .returning(ChainStoreHandle::default);

    let mut handler = build_test_handler(chain_store_handle, swarm_tx);

    let peer_id = libp2p::PeerId::random();
    let result = handler.dispatch_response(
        peer_id,
        ConnectionId::new_unchecked(1),
        Message::GetData(crate::node::messages::GetData::Block(BlockHash::all_zeros())),
    );

    assert!(result.is_ok());
}

#[tokio::test]
async fn test_dispatch_response_enqueues_to_worker() {
    let (swarm_tx, _swarm_rx) = mpsc::channel(32);
    let mut chain_store_handle = ChainStoreHandle::default();
    chain_store_handle
        .expect_clone()
        .returning(ChainStoreHandle::default);
    let (mut handler, mut response_worker_rx) = build_test_handler_parts(
        chain_store_handle,
        swarm_tx,
        Arc::new(MockDefaultShareValidator::default()),
    );

    let peer_id = libp2p::PeerId::random();
    handler
        .dispatch_response(
            peer_id,
            ConnectionId::new_unchecked(1),
            Message::NotFound(GetData::Block(BlockHash::all_zeros())),
        )
        .unwrap();

    let event = response_worker_rx
        .try_recv()
        .expect("response handed to the worker, not processed on the driver");
    assert_eq!(event.peer, peer_id);
    assert!(matches!(event.response, Message::NotFound(_)));
}

#[tokio::test]
async fn test_dispatch_response_drops_when_worker_full() {
    let (swarm_tx, _swarm_rx) = mpsc::channel(32);
    let mut chain_store_handle = ChainStoreHandle::default();
    chain_store_handle
        .expect_clone()
        .returning(ChainStoreHandle::default);
    let (mut handler, mut response_worker_rx) = build_test_handler_parts(
        chain_store_handle,
        swarm_tx,
        Arc::new(MockDefaultShareValidator::default()),
    );

    // Dispatch far more than the worker channel can hold, without draining
    // it. Every dispatch must return Ok without blocking (it is a try_send),
    // and the overflow must be dropped rather than queued.
    let peer_id = libp2p::PeerId::random();
    let mut dispatched = 0;
    while dispatched < 2000 {
        handler
            .dispatch_response(
                peer_id,
                ConnectionId::new_unchecked(1),
                Message::NotFound(GetData::Block(BlockHash::all_zeros())),
            )
            .expect("dispatch never blocks or errors, even when the worker is full");
        dispatched += 1;
    }

    let mut received = 0;
    while response_worker_rx.try_recv().is_ok() {
        received += 1;
    }
    assert!(
        received < dispatched,
        "some responses were dropped when the worker channel was full"
    );
    assert_eq!(
        handler.responses_dropped_total(),
        (dispatched - received) as u64,
        "every dropped response is counted"
    );
}

// A plain `#[test]` (not `#[tokio::test]`): dispatch_response must be callable
// with no runtime, which is only possible because it does not await -- the
// property that keeps the node event loop from blocking on swarm_tx.
#[test]
fn test_dispatch_response_is_synchronous() {
    let (swarm_tx, _swarm_rx) = mpsc::channel(32);
    let mut chain_store_handle = ChainStoreHandle::default();
    chain_store_handle
        .expect_clone()
        .returning(ChainStoreHandle::default);
    let (mut handler, _response_worker_rx) = build_test_handler_parts(
        chain_store_handle,
        swarm_tx,
        Arc::new(MockDefaultShareValidator::default()),
    );

    let result = handler.dispatch_response(
        PeerId::random(),
        ConnectionId::new_unchecked(1),
        Message::NotFound(GetData::Block(BlockHash::all_zeros())),
    );
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_dispatch_request_disconnects_on_full_peer_channel() {
    let (swarm_tx, _swarm_rx) = mpsc::channel(32);
    let mut chain_store_handle = ChainStoreHandle::default();
    chain_store_handle
        .expect_clone()
        .returning(ChainStoreHandle::default);
    let mut handler = build_test_handler(chain_store_handle, swarm_tx);

    // A capacity-1 peer channel whose receiver is kept alive (so sends fail
    // Full, not Closed). The first dispatch fills it; the second must
    // disconnect the peer via the request sender rather than await swarm_tx.
    let peer_id = PeerId::random();
    let (sender, _receiver) = mpsc::channel(1);
    handler
        .peer_handles
        .insert(peer_id, PeerHandle::new_for_test(sender));

    let mut request_sender = MockRequestSender::new();
    request_sender
        .expect_disconnect_peer()
        .times(1)
        .return_const(());

    let (channel_tx_first, _first) = oneshot::channel::<Message>();
    handler
        .dispatch_request(
            peer_id,
            ConnectionId::new_unchecked(1),
            Message::NotFound(GetData::Block(BlockHash::all_zeros())),
            channel_tx_first,
            &mut request_sender,
        )
        .unwrap();

    let (channel_tx_second, _second) = oneshot::channel::<Message>();
    handler
        .dispatch_request(
            peer_id,
            ConnectionId::new_unchecked(1),
            Message::NotFound(GetData::Block(BlockHash::all_zeros())),
            channel_tx_second,
            &mut request_sender,
        )
        .unwrap();
}

#[tokio::test]
async fn test_dispatch_request_calls_service() {
    let (swarm_tx, mut swarm_rx) = mpsc::channel(32);
    let mut chain_store_handle = ChainStoreHandle::default();

    let block1 = TestShareBlockBuilder::new().build();
    let block2 = TestShareBlockBuilder::new().build();

    let block_hashes = vec![block1.block_hash()];
    let stop_block_hash = block2.block_hash();

    chain_store_handle.expect_clone().returning(|| {
        let mut mock = ChainStoreHandle::default();
        let headers = vec![
            TestShareBlockBuilder::new().build().header,
            TestShareBlockBuilder::new().build().header,
        ];
        mock.expect_get_headers_for_locator()
            .returning(move |_, _, _| Ok(headers.clone()));
        mock.expect_get_template_merkle_branches()
            .returning(|_| Ok(Vec::new()));
        mock
    });

    let mut handler = build_test_handler(chain_store_handle, swarm_tx);

    let peer_id = PeerId::random();
    handler.add_peer(peer_id);
    let (response_tx, _response_rx) = oneshot::channel::<Message>();

    let result = handler.dispatch_request(
        peer_id,
        ConnectionId::new_unchecked(1),
        Message::GetShareHeaders(block_hashes, stop_block_hash),
        response_tx,
        &mut MockRequestSender::new(),
    );

    assert!(result.is_ok());

    // The per-peer task processes asynchronously, wait for response
    if let Some(SwarmSend::Response(_, Message::ShareHeaders(headers))) = swarm_rx.recv().await {
        assert_eq!(headers.len(), 2);
    } else {
        panic!("Expected SwarmSend::Response with ShareHeaders message");
    }
}

#[tokio::test]
async fn test_dispatch_request_creates_handle_on_the_fly() {
    let (swarm_tx, _swarm_rx) = mpsc::channel(32);
    let mut chain_store_handle = ChainStoreHandle::default();
    chain_store_handle.expect_clone().returning(|| {
        let mut cloned = ChainStoreHandle::default();
        cloned.expect_is_current().returning(|| true);
        cloned
            .expect_get_missing_blockhashes()
            .returning(|_| Vec::with_capacity(0));
        cloned
    });

    let mut handler = build_test_handler(chain_store_handle, swarm_tx);

    // Do NOT call add_peer -- dispatch_request should create the handle
    let peer_id = PeerId::random();
    let (channel_tx, _channel_rx) = oneshot::channel::<Message>();

    let result = handler.dispatch_request(
        peer_id,
        ConnectionId::new_unchecked(1),
        Message::Inventory(InventoryMessage::BlockHashes(vec![BlockHash::all_zeros()])),
        channel_tx,
        &mut MockRequestSender::new(),
    );
    assert!(result.is_ok());

    // Verify the handle was created
    assert!(handler.peer_handles.contains_key(&peer_id));
}

#[tokio::test]
async fn test_dispatch_request_removes_stale_handle_on_closed() {
    let (swarm_tx, _swarm_rx) = mpsc::channel(32);
    let mut chain_store_handle = ChainStoreHandle::default();
    chain_store_handle
        .expect_clone()
        .returning(ChainStoreHandle::default);

    let mut handler = build_test_handler(chain_store_handle, swarm_tx);

    let peer_id = PeerId::random();

    // Create a channel where the receiver is immediately dropped,
    // simulating a task that has exited.
    let (sender, receiver) = mpsc::channel(16);
    drop(receiver);
    handler
        .peer_handles
        .insert(peer_id, PeerHandle::new_for_test(sender));

    let (channel_tx, _channel_rx) = oneshot::channel::<Message>();
    let result = handler.dispatch_request(
        peer_id,
        ConnectionId::new_unchecked(1),
        Message::NotFound(GetData::Block(BlockHash::all_zeros())),
        channel_tx,
        &mut MockRequestSender::new(),
    );
    assert!(result.is_ok());

    // Stale handle should have been removed on Closed
    assert!(
        !handler.peer_handles.contains_key(&peer_id),
        "Stale handle should be removed after Closed error"
    );
}

#[tokio::test]
async fn test_dispatch_request_records_inventory_knowledge() {
    let (swarm_tx, _swarm_rx) = mpsc::channel(32);
    let mut chain_store_handle = ChainStoreHandle::default();
    chain_store_handle.expect_clone().returning(|| {
        let mut cloned = ChainStoreHandle::default();
        cloned.expect_is_current().returning(|| true);
        cloned
            .expect_get_missing_blockhashes()
            .returning(|_| Vec::with_capacity(0));
        cloned
    });
    let mut handler = build_test_handler(chain_store_handle, swarm_tx);

    let peer_id = PeerId::random();
    handler.add_peer(peer_id);
    let block_hash = BlockHash::all_zeros();
    let inventory = InventoryMessage::BlockHashes(vec![block_hash]);
    let (channel_tx, _channel_rx) = oneshot::channel::<Message>();

    let result = handler.dispatch_request(
        peer_id,
        ConnectionId::new_unchecked(1),
        Message::Inventory(inventory),
        channel_tx,
        &mut MockRequestSender::new(),
    );
    assert!(result.is_ok());

    assert!(
        handler
            .peer_block_knowledge()
            .peer_knows_block(&peer_id, &block_hash)
    );
}

#[tokio::test]
async fn test_dispatch_response_records_share_block_knowledge() {
    let (swarm_tx, _swarm_rx) = mpsc::channel(32);
    let mut chain_store_handle = ChainStoreHandle::default();

    // The cloned handle is used by handle_response -> handle_share_block,
    // which checks for duplicates, validates header, and stores the block.
    chain_store_handle.expect_clone().returning(|| {
        let mut cloned = ChainStoreHandle::default();
        cloned.expect_share_block_exists().returning(|_| false);
        cloned.expect_is_candidate().returning(|_| false);
        cloned.expect_add_share_block().returning(|_| Ok(()));
        cloned
    });

    let mut mock_validator = MockDefaultShareValidator::default();
    mock_validator
        .expect_validate_share_header()
        .returning(|_| Ok(()));
    mock_validator
        .expect_validate_block_size()
        .returning(|_| Ok(()));
    mock_validator
        .expect_validate_body_matches_header()
        .returning(|_| Ok(()));
    mock_validator
        .expect_validate_with_pool_difficulty()
        .returning(|_, _| Ok(()));

    let mut handler =
        build_test_handler_with_validator(chain_store_handle, swarm_tx, Arc::new(mock_validator));

    let peer_id = libp2p::PeerId::random();
    let block = valid_share_block_from_fixture();
    let block_hash = block.block_hash();

    let result = handler.dispatch_response(
        peer_id,
        ConnectionId::new_unchecked(1),
        Message::ShareBlock(block),
    );
    assert!(result.is_ok());

    // Knowledge is recorded before handle_response processes the block
    assert!(
        handler
            .peer_block_knowledge()
            .peer_knows_block(&peer_id, &block_hash)
    );
}

#[tokio::test]
async fn test_dispatch_response_records_inventory_knowledge() {
    let (swarm_tx, _swarm_rx) = mpsc::channel(32);
    let mut chain_store_handle = ChainStoreHandle::default();
    chain_store_handle
        .expect_clone()
        .returning(ChainStoreHandle::default);

    let mut handler = build_test_handler(chain_store_handle, swarm_tx);

    let peer_id = libp2p::PeerId::random();
    let block_hash = BlockHash::all_zeros();
    let inventory = InventoryMessage::BlockHashes(vec![block_hash]);

    let result = handler.dispatch_response(
        peer_id,
        ConnectionId::new_unchecked(1),
        Message::Inventory(inventory),
    );
    assert!(result.is_ok());

    assert!(
        handler
            .peer_block_knowledge()
            .peer_knows_block(&peer_id, &block_hash)
    );
}

#[tokio::test]
async fn test_remove_peer() {
    let (swarm_tx, _swarm_rx) = mpsc::channel(32);
    let mut chain_store_handle = ChainStoreHandle::default();
    chain_store_handle.expect_clone().returning(|| {
        let mut cloned = ChainStoreHandle::default();
        cloned.expect_is_current().returning(|| true);
        cloned
            .expect_get_missing_blockhashes()
            .returning(|_| Vec::with_capacity(0));
        cloned
    });
    let mut handler = build_test_handler(chain_store_handle, swarm_tx);

    let peer_id = PeerId::random();
    handler.add_peer(peer_id);
    let block_hash = BlockHash::all_zeros();
    let inventory = InventoryMessage::BlockHashes(vec![block_hash]);
    let (channel_tx, _channel_rx) = oneshot::channel::<Message>();

    let _ = handler.dispatch_request(
        peer_id,
        ConnectionId::new_unchecked(1),
        Message::Inventory(inventory),
        channel_tx,
        &mut MockRequestSender::new(),
    );
    assert!(
        handler
            .peer_block_knowledge()
            .peer_knows_block(&peer_id, &block_hash)
    );
    assert!(handler.peer_handles.contains_key(&peer_id));

    handler.remove_peer(&peer_id);
    assert!(
        !handler
            .peer_block_knowledge()
            .peer_knows_block(&peer_id, &block_hash)
    );
    assert!(!handler.peer_handles.contains_key(&peer_id));
}
