// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The organise worker's buffer of blocks waiting for their parent, bounded
//! by block count and by bytes.
//!
//! When either bound would be passed, the oldest arrivals go first. A block
//! that waited longest is the likeliest never to proceed (its parent stuck
//! or on a branch nobody extends), and evicting it keeps a flood of such
//! blocks, each with valid proof of work but cheap on the mini chain, from
//! pinning the buffer while honest blocks are turned away. An evicted block
//! is stored with its body, so nothing needs fetching again: it is
//! `HeaderValid` with a body, which the organise worker treats as stranded
//! and re-validates when a descendant arrives on top of it
//! (`redrive_stranded_parent`), or at the next start (`seed_revalidation`).

use crate::shares::share_block::ShareBlock;
use bitcoin::BlockHash;
use std::collections::BTreeMap;

/// Most blocks and bytes the buffer holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct BufferLimits {
    /// Most blocks held.
    pub(super) max_blocks: usize,
    /// Most bytes held, by consensus encoding.
    pub(super) max_bytes: usize,
}

/// A block waiting for its parent, with the validation tier stage 1 gave it.
pub(super) struct BufferedBlock {
    /// Carried rather than recomputed: `insert` compares it against every
    /// entry already at the height.
    pub(super) block_hash: BlockHash,
    pub(super) share_block: ShareBlock,
    pub(super) content_validated: bool,
    /// Arrival order, the block's key in `PendingBuffer::arrival_order`.
    sequence: u64,
    /// The block's consensus-encoded size, counted in `PendingBuffer::bytes`.
    size: usize,
}

/// What `PendingBuffer::insert` did with a block.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum BufferOutcome {
    /// Buffered, after evicting these blocks, oldest first, to make room.
    Buffered { evicted: Vec<BlockHash> },
    /// Already buffered at that parent height; not added again.
    Duplicate,
    /// Larger than the whole byte bound, so never buffered.
    TooLarge { size: usize },
}

/// Blocks keyed by their parent's height, with their arrival order.
pub(super) struct PendingBuffer {
    by_parent_height: BTreeMap<u32, Vec<BufferedBlock>>,
    /// Buffered blocks by arrival, oldest first: parent height and hash.
    arrival_order: BTreeMap<u64, (u32, BlockHash)>,
    /// Sequence number the next block gets.
    next_sequence: u64,
    /// Sum of the buffered blocks' sizes.
    bytes: usize,
    limits: BufferLimits,
}

impl PendingBuffer {
    pub(super) fn new(limits: BufferLimits) -> Self {
        Self {
            by_parent_height: BTreeMap::new(),
            arrival_order: BTreeMap::new(),
            next_sequence: 0,
            bytes: 0,
            limits,
        }
    }

    pub(super) fn limits(&self) -> BufferLimits {
        self.limits
    }

    /// Number of blocks buffered.
    pub(super) fn len(&self) -> usize {
        self.arrival_order.len()
    }

    /// Bytes buffered, by consensus encoding.
    pub(super) fn bytes(&self) -> usize {
        self.bytes
    }

    /// Buffer `share_block` under `parent_height`, evicting the oldest
    /// blocks until it fits within both bounds.
    pub(super) fn insert(
        &mut self,
        parent_height: u32,
        share_block: ShareBlock,
        content_validated: bool,
    ) -> BufferOutcome {
        let block_hash = share_block.block_hash();
        if self
            .by_parent_height
            .get(&parent_height)
            .is_some_and(|buffered| buffered.iter().any(|entry| entry.block_hash == block_hash))
        {
            return BufferOutcome::Duplicate;
        }
        let size = share_block.encoded_size();
        if size > self.limits.max_bytes || self.limits.max_blocks == 0 {
            return BufferOutcome::TooLarge { size };
        }
        let mut evicted = Vec::new();
        while self.len() >= self.limits.max_blocks || self.bytes + size > self.limits.max_bytes {
            match self.evict_oldest() {
                Some(hash) => evicted.push(hash),
                None => break,
            }
        }
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        self.arrival_order
            .insert(sequence, (parent_height, block_hash));
        self.bytes += size;
        self.by_parent_height
            .entry(parent_height)
            .or_insert_with(|| Vec::with_capacity(2))
            .push(BufferedBlock {
                block_hash,
                share_block,
                content_validated,
                sequence,
                size,
            });
        BufferOutcome::Buffered { evicted }
    }

    /// Take every block buffered under `parent_height`, in arrival order.
    pub(super) fn remove(&mut self, parent_height: &u32) -> Option<Vec<BufferedBlock>> {
        let blocks = self.by_parent_height.remove(parent_height)?;
        for block in &blocks {
            self.arrival_order.remove(&block.sequence);
            self.bytes -= block.size;
        }
        Some(blocks)
    }

    /// Drop the oldest block, returning its hash.
    fn evict_oldest(&mut self) -> Option<BlockHash> {
        let (_, (parent_height, block_hash)) = self.arrival_order.pop_first()?;
        if let Some(blocks) = self.by_parent_height.get_mut(&parent_height) {
            if let Some(index) = blocks
                .iter()
                .position(|entry| entry.block_hash == block_hash)
            {
                self.bytes -= blocks.remove(index).size;
            }
            if blocks.is_empty() {
                self.by_parent_height.remove(&parent_height);
            }
        }
        Some(block_hash)
    }

    /// The blocks buffered under `parent_height`.
    #[cfg(test)]
    pub(super) fn get(&self, parent_height: &u32) -> Option<&Vec<BufferedBlock>> {
        self.by_parent_height.get(parent_height)
    }

    /// Whether any block is buffered under `parent_height`.
    #[cfg(test)]
    pub(super) fn contains_key(&self, parent_height: &u32) -> bool {
        self.by_parent_height.contains_key(parent_height)
    }

    /// Whether the buffer is empty.
    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.arrival_order.is_empty()
    }
}

#[cfg(test)]
mod tests;
