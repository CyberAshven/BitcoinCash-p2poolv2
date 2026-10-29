// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use crate::shares::share_block::ShareBlock;
use crate::test_utils::TestShareBlockBuilder;
use bitcoin::BlockHash;
use bitcoin::hashes::Hash;
use tokio_test::{assert_pending, assert_ready, task};

/// A share block on top of a parent named by `parent`.
fn block(parent: u8) -> ShareBlock {
    TestShareBlockBuilder::new()
        .prev_share_blockhash(BlockHash::from_byte_array([parent; 32]).to_string())
        .build()
}

fn block_event(share_block: ShareBlock) -> OrganiseEvent {
    OrganiseEvent::Block {
        share_block,
        content_validated: true,
    }
}

/// The hash of a received event's block, or of an invalid block.
fn hash_of(event: OrganiseEvent) -> BlockHash {
    match event {
        OrganiseEvent::Block { share_block, .. } => share_block.block_hash(),
        OrganiseEvent::InvalidBlock(hash) => hash,
    }
}

#[tokio::test]
async fn a_queued_block_holds_its_bytes_until_received() {
    let first = block(1);
    let size = first.encoded_size();
    let (sender, mut receiver) = organise_channel(8, 10 * size);

    sender.send(block_event(first.clone())).await.unwrap();
    assert_eq!(sender.available_bytes(), 9 * size);

    assert_eq!(hash_of(receiver.recv().await.unwrap()), first.block_hash());
    assert_eq!(sender.available_bytes(), 10 * size);
}

/// A sender whose block would pass the byte budget waits until the worker
/// takes enough off the queue: backpressure, with the event count far from
/// its bound.
#[tokio::test]
async fn a_send_past_the_byte_budget_waits_for_the_receiver() {
    let (first, second) = (block(1), block(2));
    let size = first.encoded_size();
    let (sender, mut receiver) = organise_channel(8, size + size / 2);
    sender.send(block_event(first.clone())).await.unwrap();

    let mut waiting = task::spawn(sender.send(block_event(second.clone())));
    assert_pending!(waiting.poll());
    assert_pending!(waiting.poll());

    assert_eq!(hash_of(receiver.recv().await.unwrap()), first.block_hash());
    assert!(waiting.is_woken());
    assert_ready!(waiting.poll()).unwrap();
    drop(waiting);
    assert_eq!(hash_of(receiver.recv().await.unwrap()), second.block_hash());
}

/// The count bound is kept: a full queue blocks a sender however few bytes
/// it holds.
#[tokio::test]
async fn the_event_count_still_bounds_the_channel() {
    let (sender, mut receiver) = organise_channel(2, usize::MAX);
    sender.send(block_event(block(1))).await.unwrap();
    sender.send(block_event(block(2))).await.unwrap();

    let mut waiting = task::spawn(sender.send(block_event(block(3))));
    assert_pending!(waiting.poll());

    receiver.recv().await.unwrap();
    assert_ready!(waiting.poll()).unwrap();
}

/// A block larger than the whole budget takes the whole budget, so it
/// passes once the queue is empty instead of waiting forever.
#[tokio::test]
async fn a_block_larger_than_the_budget_passes_alone() {
    let small = block(1);
    let large = TestShareBlockBuilder::new()
        .prev_share_blockhash(BlockHash::from_byte_array([2; 32]).to_string())
        .add_transaction(bitcoin::Transaction {
            version: bitcoin::transaction::Version::TWO,
            lock_time: bitcoin::absolute::LockTime::ZERO,
            input: vec![bitcoin::TxIn::default()],
            output: vec![bitcoin::TxOut {
                value: bitcoin::Amount::ZERO,
                script_pubkey: bitcoin::ScriptBuf::from_bytes(vec![0x6a; 10_000]),
            }],
        })
        .build();
    let budget = 2 * small.encoded_size();
    assert!(large.encoded_size() > budget);
    let (sender, mut receiver) = organise_channel(8, budget);
    sender.send(block_event(small.clone())).await.unwrap();

    let mut waiting = task::spawn(sender.send(block_event(large.clone())));
    assert_pending!(waiting.poll());

    receiver.recv().await.unwrap();
    assert_ready!(waiting.poll()).unwrap();
    drop(waiting);
    assert_eq!(sender.available_bytes(), 0);
    assert_eq!(hash_of(receiver.recv().await.unwrap()), large.block_hash());
    assert_eq!(sender.available_bytes(), budget);
}

/// An invalid-block report carries a hash only, so a full byte budget does
/// not hold it back.
#[tokio::test]
async fn an_invalid_block_report_takes_no_bytes() {
    let first = block(1);
    let (sender, mut receiver) = organise_channel(8, first.encoded_size());
    sender.send(block_event(first)).await.unwrap();
    assert_eq!(sender.available_bytes(), 0);

    let invalid = BlockHash::from_byte_array([7; 32]);
    sender
        .send(OrganiseEvent::InvalidBlock(invalid))
        .await
        .unwrap();

    receiver.recv().await.unwrap();
    assert_eq!(hash_of(receiver.recv().await.unwrap()), invalid);
}

/// Dropping the receiver fails a sender waiting for bytes, as a closed
/// channel fails any send, and hands the event back.
#[tokio::test]
async fn a_waiting_send_fails_when_the_receiver_is_gone() {
    let (first, second) = (block(1), block(2));
    let (sender, receiver) = organise_channel(8, first.encoded_size());
    sender.send(block_event(first)).await.unwrap();

    let mut waiting = task::spawn(sender.send(block_event(second.clone())));
    assert_pending!(waiting.poll());

    drop(receiver);
    let returned = assert_ready!(waiting.poll()).unwrap_err().0;
    assert_eq!(hash_of(returned), second.block_hash());
    assert!(sender.send(block_event(block(3))).await.is_err());
}

#[tokio::test]
async fn the_receiver_ends_when_every_sender_is_gone() {
    let first = block(1);
    let (sender, mut receiver) = organise_channel(8, 4 * first.encoded_size());
    sender.send(block_event(first.clone())).await.unwrap();
    drop(sender);

    assert_eq!(hash_of(receiver.recv().await.unwrap()), first.block_hash());
    assert!(receiver.recv().await.is_none());
}

#[test]
fn the_default_budget_holds_the_count_bound_of_honest_shares() {
    use super::super::{ORGANISE_CHANNEL_BYTES, ORGANISE_CHANNEL_CAPACITY};
    use crate::node::messages::MAX_P2P_MESSAGE_SIZE;

    assert_eq!(ORGANISE_CHANNEL_BYTES, 64 * 1024 * 1024);
    // 512 shares of 128 KiB: honest shares meet the count bound first.
    const { assert!(ORGANISE_CHANNEL_BYTES >= ORGANISE_CHANNEL_CAPACITY * 128 * 1024) };
    // At least 51 of the largest shares a peer can send.
    const { assert!(ORGANISE_CHANNEL_BYTES / MAX_P2P_MESSAGE_SIZE >= 51) };
    assert!(u32::try_from(ORGANISE_CHANNEL_BYTES).is_ok());
}
