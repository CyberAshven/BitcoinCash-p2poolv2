use super::*;
#[mockall_double::double]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
use crate::test_utils::TestShareBlockBuilder;
use bitcoin::hashes::Hash;
use tokio::sync::mpsc;

#[tokio::test]
async fn test_handle_getheaders() {
    let mut chain_store_handle = ChainStoreHandle::default();
    chain_store_handle
        .expect_carried_bch_headers()
        .returning(|_| Vec::new());
    let (swarm_tx, mut swarm_rx) = mpsc::channel::<SwarmSend<u32>>(1);
    let response_channel = 1u32;

    let block1 = TestShareBlockBuilder::new().build();

    let block2 = TestShareBlockBuilder::new().build();

    let block_hashes = vec![block1.block_hash(), block2.block_hash()];

    let response_headers = vec![block1.header.clone(), block2.header.clone()];

    let stop_block_hash = block2.block_hash();

    // Set up mock expectations
    chain_store_handle
        .expect_get_headers_for_locator()
        .returning(move |_, _, _| Ok(response_headers.clone()));
    chain_store_handle
        .expect_get_template_merkle_branches()
        .returning(|_| Ok(Vec::new()));

    let _result = handle_getheaders(
        block_hashes,
        stop_block_hash,
        chain_store_handle,
        response_channel,
        swarm_tx,
    )
    .await;

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

/// Each served header carries its stored coinbase branch, and headers
/// sharing a branch share one table entry.
#[tokio::test]
async fn test_handle_getheaders_serves_stored_branches() {
    let mut chain_store_handle = ChainStoreHandle::default();
    chain_store_handle
        .expect_carried_bch_headers()
        .returning(|_| Vec::new());
    let (swarm_tx, mut swarm_rx) = mpsc::channel::<SwarmSend<u32>>(1);

    let block1 = TestShareBlockBuilder::new().nonce(1).build();
    let block2 = TestShareBlockBuilder::new().nonce(2).build();
    let response_headers = vec![block1.header.clone(), block2.header.clone()];
    let branch = vec![bitcoin::TxMerkleNode::from_byte_array([0x42; 32])];
    let stored_branch = branch.clone();

    chain_store_handle
        .expect_get_headers_for_locator()
        .returning(move |_, _, _| Ok(response_headers.clone()));
    chain_store_handle
        .expect_get_template_merkle_branches()
        .times(2)
        .returning(move |_| Ok(stored_branch.clone()));

    handle_getheaders(
        vec![block1.block_hash()],
        block2.block_hash(),
        chain_store_handle,
        1u32,
        swarm_tx,
    )
    .await
    .unwrap();

    match swarm_rx.recv().await {
        Some(SwarmSend::Response(_, Message::ShareHeaders(batch))) => {
            assert_eq!(batch.len(), 2);
            assert_eq!(batch.branch_count(), 1);
            assert_eq!(batch.branch(0), branch.as_slice());
            assert_eq!(batch.branch(1), branch.as_slice());
        }
        _ => panic!("Expected SwarmSend::Response with ShareHeaders message"),
    }
}

#[tokio::test]
async fn test_handle_getheaders_send_failure() {
    let mut chain_store_handle = ChainStoreHandle::default();
    chain_store_handle
        .expect_carried_bch_headers()
        .returning(|_| Vec::new());
    let (swarm_tx, swarm_rx) = mpsc::channel::<SwarmSend<u32>>(1);
    let response_channel = 1u32;

    let block1 = TestShareBlockBuilder::new().build();

    let block2 = TestShareBlockBuilder::new().build();

    let block_hashes = vec![block1.block_hash(), block2.block_hash()];

    let stop_block_hash = block2.block_hash();

    // Set up mock expectations
    chain_store_handle
        .expect_get_headers_for_locator()
        .returning(move |_, _, _| Ok(Vec::new()));
    chain_store_handle
        .expect_get_template_merkle_branches()
        .returning(|_| Ok(Vec::new()));

    // Drop the receiver to simulate send failure
    drop(swarm_rx);

    let result = handle_getheaders(
        block_hashes,
        stop_block_hash,
        chain_store_handle,
        response_channel,
        swarm_tx,
    )
    .await;

    assert!(result.is_err());
    if let Err(e) = result {
        assert!(e.to_string().contains("Failed to send getheaders response"));
    } else {
        panic!("Expected an error due to send failure");
    }
}
