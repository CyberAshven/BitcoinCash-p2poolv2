// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use crate::test_utils::TestShareBlockBuilder;
use bitcoin::hashes::Hash;
use bitcoin::{Amount, ScriptBuf, Transaction, TxIn, TxOut, absolute, transaction};

/// A small share block on top of a parent named by `parent`.
fn block(parent: u8) -> ShareBlock {
    TestShareBlockBuilder::new()
        .prev_share_blockhash(BlockHash::from_byte_array([parent; 32]).to_string())
        .build()
}

/// A share block carrying a share transaction of about `bytes` bytes, as
/// large as a Bitcoin Cash share with a full coinbase.
fn large_block(parent: u8, bytes: usize) -> ShareBlock {
    let transaction = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn::default()],
        output: vec![TxOut {
            value: Amount::ZERO,
            script_pubkey: ScriptBuf::from_bytes(vec![0x6a; bytes]),
        }],
    };
    TestShareBlockBuilder::new()
        .prev_share_blockhash(BlockHash::from_byte_array([parent; 32]).to_string())
        .add_transaction(transaction)
        .build()
}

fn limits(max_blocks: usize, max_bytes: usize) -> BufferLimits {
    BufferLimits {
        max_blocks,
        max_bytes,
    }
}

/// The buffer's own bookkeeping agrees with what it holds.
fn assert_consistent(buffer: &PendingBuffer) {
    let held: Vec<&BufferedBlock> = buffer.by_parent_height.values().flatten().collect();
    assert_eq!(held.len(), buffer.len());
    assert_eq!(
        held.iter().map(|entry| entry.size).sum::<usize>(),
        buffer.bytes()
    );
    assert!(buffer.len() <= buffer.limits.max_blocks);
    assert!(buffer.bytes() <= buffer.limits.max_bytes);
    assert!(
        buffer
            .by_parent_height
            .values()
            .all(|blocks| !blocks.is_empty())
    );
    for entry in held {
        assert_eq!(
            buffer.arrival_order.get(&entry.sequence),
            Some(&(
                buffer
                    .by_parent_height
                    .iter()
                    .find(|(_, blocks)| blocks.iter().any(|b| b.block_hash == entry.block_hash))
                    .map(|(height, _)| *height)
                    .unwrap(),
                entry.block_hash
            ))
        );
    }
}

#[test]
fn a_buffered_block_counts_its_encoded_size() {
    let mut buffer = PendingBuffer::new(BufferLimits::default());
    let first = block(1);
    let size = first.encoded_size();

    let outcome = buffer.insert(5, first.clone(), true);

    assert_eq!(outcome, BufferOutcome::Buffered { evicted: vec![] });
    assert_eq!(buffer.len(), 1);
    assert_eq!(buffer.bytes(), size);
    assert_eq!(buffer.get(&5).map(Vec::len), Some(1));
    assert_consistent(&buffer);
}

#[test]
fn a_duplicate_is_not_added_or_counted_again() {
    let mut buffer = PendingBuffer::new(limits(1, usize::MAX));
    let first = block(1);
    buffer.insert(5, first.clone(), true);

    assert_eq!(buffer.insert(5, first, true), BufferOutcome::Duplicate);
    assert_eq!(buffer.len(), 1);
    assert_consistent(&buffer);
}

/// The count bound is kept: past it the oldest block goes, not the new one.
#[test]
fn past_its_block_bound_the_oldest_block_is_evicted() {
    let mut buffer = PendingBuffer::new(limits(2, usize::MAX));
    let (first, second, third) = (block(1), block(2), block(3));
    buffer.insert(7, first.clone(), true);
    buffer.insert(5, second.clone(), true);

    let outcome = buffer.insert(6, third.clone(), false);

    assert_eq!(
        outcome,
        BufferOutcome::Buffered {
            evicted: vec![first.block_hash()]
        }
    );
    assert!(!buffer.contains_key(&7));
    assert!(buffer.contains_key(&5) && buffer.contains_key(&6));
    assert_eq!(buffer.bytes(), second.encoded_size() + third.encoded_size());
    assert_consistent(&buffer);
}

#[test]
fn past_its_byte_bound_the_oldest_blocks_are_evicted() {
    let (first, second, third) = (block(1), block(2), block(3));
    let size = first.encoded_size();
    let mut buffer = PendingBuffer::new(limits(1_024, 2 * size + size / 2));
    buffer.insert(5, first.clone(), true);
    buffer.insert(5, second.clone(), true);

    let outcome = buffer.insert(6, third.clone(), true);

    assert_eq!(
        outcome,
        BufferOutcome::Buffered {
            evicted: vec![first.block_hash()]
        }
    );
    assert_eq!(buffer.len(), 2);
    assert_eq!(
        buffer.get(&5).map(|blocks| blocks[0].block_hash),
        Some(second.block_hash())
    );
    assert_consistent(&buffer);
}

/// Shares as large as a Bitcoin Cash share with a full coinbase are bounded
/// by bytes long before the count bound.
#[test]
fn megabyte_shares_are_bounded_by_bytes() {
    let size = large_block(0, 1_000_000).encoded_size();
    assert!(size > 1_000_000, "{size}");
    let mut buffer = PendingBuffer::new(limits(1_024, 3 * size + size / 2));
    let blocks: Vec<ShareBlock> = (1..=5)
        .map(|parent| large_block(parent, 1_000_000))
        .collect();

    let mut evicted = Vec::new();
    for (height, share_block) in (10..).zip(blocks.iter()) {
        if let BufferOutcome::Buffered { evicted: now } =
            buffer.insert(height, share_block.clone(), true)
        {
            evicted.extend(now);
        }
    }

    assert_eq!(buffer.len(), 3);
    assert_eq!(buffer.bytes(), 3 * size);
    assert_eq!(
        evicted,
        vec![blocks[0].block_hash(), blocks[1].block_hash()]
    );
    assert!(!buffer.contains_key(&10) && !buffer.contains_key(&11));
    assert_consistent(&buffer);
}

/// One block may need several older ones evicted to fit.
#[test]
fn a_large_block_evicts_as_many_old_ones_as_it_needs() {
    let small = [block(1), block(2), block(3)];
    let large = large_block(4, 10_000);
    let max_bytes = large.encoded_size() + small[0].encoded_size() / 2;
    let mut buffer = PendingBuffer::new(limits(1_024, max_bytes));
    for (height, share_block) in (1..).zip(small.iter()) {
        buffer.insert(height, share_block.clone(), true);
    }

    let outcome = buffer.insert(9, large.clone(), true);

    assert_eq!(
        outcome,
        BufferOutcome::Buffered {
            evicted: small.iter().map(ShareBlock::block_hash).collect()
        }
    );
    assert_eq!(buffer.len(), 1);
    assert_eq!(buffer.bytes(), large.encoded_size());
    assert_consistent(&buffer);
}

#[test]
fn a_block_larger_than_the_byte_bound_is_never_buffered() {
    let first = block(1);
    let resident = block(2);
    let mut buffer = PendingBuffer::new(limits(1_024, first.encoded_size() - 1));
    let large = large_block(3, 10_000);
    let mut roomy = PendingBuffer::new(limits(1_024, large.encoded_size() - 1));
    roomy.insert(4, resident.clone(), true);

    assert_eq!(
        buffer.insert(5, first.clone(), true),
        BufferOutcome::TooLarge {
            size: first.encoded_size()
        }
    );
    assert!(buffer.is_empty());
    assert_eq!(buffer.bytes(), 0);
    // Nothing is evicted for a block that could never fit.
    assert!(matches!(
        roomy.insert(5, large, true),
        BufferOutcome::TooLarge { .. }
    ));
    assert!(roomy.contains_key(&4));
    assert_consistent(&roomy);
}

#[test]
fn removing_a_height_returns_its_blocks_and_their_bytes() {
    let (first, second, third) = (block(1), block(2), block(3));
    let size = first.encoded_size();
    let mut buffer = PendingBuffer::new(limits(1_024, 3 * size));
    buffer.insert(5, first.clone(), true);
    buffer.insert(5, second.clone(), false);
    buffer.insert(6, third.clone(), true);

    let removed = buffer.remove(&5).unwrap();

    assert_eq!(
        removed
            .iter()
            .map(|entry| (entry.block_hash, entry.content_validated))
            .collect::<Vec<_>>(),
        vec![(first.block_hash(), true), (second.block_hash(), false)]
    );
    assert_eq!(buffer.len(), 1);
    assert_eq!(buffer.bytes(), third.encoded_size());
    assert!(buffer.remove(&5).is_none());
    assert_consistent(&buffer);

    // The freed room is used without evicting the remaining block.
    assert_eq!(
        buffer.insert(7, first, true),
        BufferOutcome::Buffered { evicted: vec![] }
    );
    assert_consistent(&buffer);
}

#[test]
fn the_default_bounds_are_the_channels_doubled() {
    let limits = BufferLimits::default();
    assert_eq!(limits.max_blocks, 1_024);
    assert_eq!(limits.max_bytes, 128 * 1024 * 1024);
    assert_eq!(limits.max_bytes, 2 * super::super::ORGANISE_CHANNEL_BYTES);
    assert!(limits.max_bytes / crate::node::messages::MAX_P2P_MESSAGE_SIZE >= 102);
}
