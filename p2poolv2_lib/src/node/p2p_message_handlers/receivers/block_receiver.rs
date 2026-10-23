// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::node::request_response_handler::block_fetcher::{BlockFetcherEvent, BlockFetcherHandle};
use crate::node::validation_worker::{ValidationEvent, ValidationSender};
#[cfg(test)]
#[mockall_double::double]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
#[cfg(not(test))]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
use crate::shares::share_block::{ShareBlock, is_terminal_blockhash};
use crate::shares::validation::{ShareValidator, stored_ancestor_times};
use crate::store::block_tx_metadata::Status;
use crate::store::prune_blocks::prune_height_for_tip;
use crate::store::writer::StoreError;
use bitcoin::BlockHash;
use bitcoin::consensus::Encodable;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::error::Error;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, error, info, warn};

/// Channel capacity for block receiver events.
const BLOCK_RECEIVER_CHANNEL_CAPACITY: usize = 8192;

/// Most blocks the pending buffer holds before it evicts the oldest.
///
/// Honest buffering is bounded by the block fetcher, 16 requests in flight
/// per peer, so the buffer holds a few hundred blocks at most during a sync.
pub const MAX_PENDING_BLOCKS: usize = 1_024;

/// Most bytes, by consensus encoding, the pending buffer holds before it
/// evicts the oldest: 32 MiB, at least 160 blocks of the largest size the
/// admission gate lets through (`BLOCK_TXS_SIZE_LIMIT`, 200 KiB, plus header
/// and merkle branches).
pub const MAX_PENDING_BYTES: usize = 32 * 1024 * 1024;

/// How long a block may wait in the pending buffer for its ancestry. An
/// honest parent arrives within seconds; a block evicted while still on the
/// candidate chain is fetched again by the block fetcher's missing-data scan.
pub const PENDING_BLOCK_TTL: Duration = Duration::from_secs(600);

/// Initial capacity of the pending maps: a block fetcher round from a few
/// peers. They grow towards `MAX_PENDING_BLOCKS` only under load.
const INITIAL_PENDING_CAPACITY: usize = 64;

/// Bounds on the pending buffer: block count, bytes and age.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingLimits {
    /// Most blocks held.
    pub max_blocks: usize,
    /// Most bytes held, by consensus encoding.
    pub max_bytes: usize,
    /// Longest a block is held.
    pub max_age: Duration,
}

impl Default for PendingLimits {
    fn default() -> Self {
        Self {
            max_blocks: MAX_PENDING_BLOCKS,
            max_bytes: MAX_PENDING_BYTES,
            max_age: PENDING_BLOCK_TTL,
        }
    }
}

/// Events sent to the BlockReceiver actor.
pub enum BlockReceiverEvent {
    /// A new share block arrived from a peer, after passing DoS
    /// validation (validate_share_header) in handle_share_block.
    ShareBlockReceived {
        share_block: ShareBlock,
        result_tx: oneshot::Sender<Result<(), Box<dyn Error + Send + Sync>>>,
    },
}

pub type BlockReceiverHandle = mpsc::Sender<BlockReceiverEvent>;
pub type BlockReceiverReceiver = mpsc::Receiver<BlockReceiverEvent>;

/// Create a block receiver channel with bounded capacity.
pub fn create_block_receiver_channel() -> (BlockReceiverHandle, BlockReceiverReceiver) {
    mpsc::channel(BLOCK_RECEIVER_CHANNEL_CAPACITY)
}

/// A share block waiting in the pending set for its ancestors to reach
/// HeaderValid in the store.
struct PendingBlock {
    share_block: ShareBlock,
    /// Arrival order, the block's key in `BlockReceiver::arrival_order`.
    sequence: u64,
    /// When the block entered the buffer.
    received_at: Instant,
    /// The block's consensus-encoded size, counted in `pending_bytes`.
    size: usize,
}

/// Buffers incoming ShareBlocks until their direct parent and uncles are
/// at status HeaderValid (or better) in the store, then validates ASERT
/// difficulty, persists the block, and queues it for full validation.
///
/// Each block is processed in isolation against its parent's metadata
/// from the store. When a block becomes ready and is committed, any
/// previously-buffered descendants whose ancestry is now complete are
/// driven to commit via an iterative descendant worklist.
///
/// The buffer is bounded (`PendingLimits`): at most `max_blocks` blocks and
/// `max_bytes` bytes, the oldest evicted first, and a block older than
/// `max_age` is dropped on the next event. A block naming a parent that never
/// arrives costs a share's proof of work (the admission gate checks the
/// coinbase proof), but the mini chain's share difficulty is low, so without
/// the bounds a peer could grow the buffer without limit. The bound is global
/// rather than per peer: peer identities are free, so a per-peer quota bounds
/// one connection, not an attacker.
pub struct BlockReceiver {
    event_rx: BlockReceiverReceiver,
    /// Pending blocks indexed by their block hash.
    pending: HashMap<BlockHash, PendingBlock>,
    /// Pending blocks by arrival order, oldest first, for eviction and
    /// expiry.
    arrival_order: BTreeMap<u64, BlockHash>,
    /// Sequence number the next pending block gets.
    next_sequence: u64,
    /// Sum of the sizes of the pending blocks.
    pending_bytes: usize,
    /// Bounds on the pending buffer.
    limits: PendingLimits,
    /// Reverse index: ancestor_hash -> pending blocks that declared it
    /// as parent or uncle. Used to drive descendants when an ancestor
    /// reaches HeaderValid.
    descendants: HashMap<BlockHash, Vec<BlockHash>>,
    share_validator: Arc<dyn ShareValidator + Send + Sync>,
    chain_store_handle: ChainStoreHandle,
    block_fetcher_handle: BlockFetcherHandle,
    validation_tx: ValidationSender,
    /// Prune height from the last successful candidate tip read, used when
    /// the tip cannot be read.
    last_prune_height: u32,
}

/// Size of a share block as the pending buffer counts it: its consensus
/// encoding, measured without allocating. Encoding into a sink cannot fail;
/// if it ever did, the block is counted as too large to buffer.
fn buffered_size(share_block: &ShareBlock) -> usize {
    share_block
        .consensus_encode(&mut bitcoin::io::sink())
        .unwrap_or(usize::MAX)
}

impl BlockReceiver {
    /// Create a new BlockReceiver actor.
    pub fn new(
        event_rx: BlockReceiverReceiver,
        share_validator: Arc<dyn ShareValidator + Send + Sync>,
        chain_store_handle: ChainStoreHandle,
        block_fetcher_handle: BlockFetcherHandle,
        validation_tx: ValidationSender,
    ) -> Self {
        Self {
            event_rx,
            pending: HashMap::with_capacity(INITIAL_PENDING_CAPACITY),
            arrival_order: BTreeMap::new(),
            next_sequence: 0,
            pending_bytes: 0,
            limits: PendingLimits::default(),
            descendants: HashMap::with_capacity(INITIAL_PENDING_CAPACITY),
            share_validator,
            chain_store_handle,
            block_fetcher_handle,
            validation_tx,
            last_prune_height: 0,
        }
    }

    /// Use other bounds on the pending buffer than `PendingLimits::default`.
    pub fn with_pending_limits(mut self, limits: PendingLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Number of blocks currently in the pending set.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Bytes the pending set holds, by consensus encoding.
    pub fn pending_bytes(&self) -> usize {
        self.pending_bytes
    }

    /// Add a block to the pending set, received at `received_at`.
    /// Updates the descendants index for parent and uncles, then evicts the
    /// oldest blocks while the set is over its bounds. A block larger than
    /// the whole byte bound is not buffered.
    fn add_to_pending(
        &mut self,
        block_hash: BlockHash,
        share_block: ShareBlock,
        received_at: Instant,
    ) {
        if self.pending.contains_key(&block_hash) {
            return;
        }
        let size = buffered_size(&share_block);
        if size > self.limits.max_bytes {
            debug!("Not buffering block {block_hash}: {size} bytes exceeds the pending buffer");
            return;
        }

        let parent_hash = share_block.header.prev_share_blockhash;
        if !is_terminal_blockhash(&parent_hash) {
            self.descendants
                .entry(parent_hash)
                .or_insert_with(|| Vec::with_capacity(2))
                .push(block_hash);
        }
        for uncle_hash in &share_block.header.uncles {
            self.descendants
                .entry(*uncle_hash)
                .or_insert_with(|| Vec::with_capacity(2))
                .push(block_hash);
        }

        let sequence = self.next_sequence;
        self.next_sequence += 1;
        self.arrival_order.insert(sequence, block_hash);
        self.pending_bytes += size;
        self.pending.insert(
            block_hash,
            PendingBlock {
                share_block,
                sequence,
                received_at,
                size,
            },
        );
        self.evict_overflow();
    }

    /// Evict the oldest pending blocks while the set holds more than
    /// `max_blocks` blocks or `max_bytes` bytes.
    ///
    /// Each pass pops one arrival entry, so the loop ends even if the two
    /// indexes ever disagreed.
    fn evict_overflow(&mut self) {
        while self.is_pending_over_limits()
            && let Some((_, oldest_hash)) = self.arrival_order.pop_first()
        {
            debug!("Evicting pending block {oldest_hash}: the pending buffer is full");
            self.remove_from_pending(&oldest_hash);
        }
    }

    /// Whether the pending set holds more than its block or byte bound.
    fn is_pending_over_limits(&self) -> bool {
        self.pending.len() > self.limits.max_blocks || self.pending_bytes > self.limits.max_bytes
    }

    /// Drop pending blocks that have waited longer than `max_age` at `now`.
    ///
    /// Blocks are checked oldest first, so the walk stops at the first block
    /// still within its age. Each pass pops one arrival entry, so the loop
    /// ends even if the two indexes ever disagreed.
    fn expire_pending(&mut self, now: Instant) {
        while let Some(expired_hash) = self.oldest_pending_expired_at(now) {
            debug!("Dropping pending block {expired_hash}: its ancestry did not arrive in time");
            self.arrival_order.pop_first();
            self.remove_from_pending(&expired_hash);
        }
    }

    /// The oldest arrival entry, if its block has waited longer than
    /// `max_age` at `now` or is no longer pending.
    fn oldest_pending_expired_at(&self, now: Instant) -> Option<BlockHash> {
        let (_, oldest_hash) = self.arrival_order.first_key_value()?;
        let expired = self.pending.get(oldest_hash).is_none_or(|oldest| {
            now.saturating_duration_since(oldest.received_at) > self.limits.max_age
        });
        expired.then_some(*oldest_hash)
    }

    /// Remove a block from the pending set and clean up its entries
    /// in the descendants index.
    fn remove_from_pending(&mut self, block_hash: &BlockHash) -> Option<ShareBlock> {
        let pending_block = self.pending.remove(block_hash)?;
        self.arrival_order.remove(&pending_block.sequence);
        self.pending_bytes -= pending_block.size;

        let parent_hash = pending_block.share_block.header.prev_share_blockhash;
        if !is_terminal_blockhash(&parent_hash)
            && let Some(descendants_list) = self.descendants.get_mut(&parent_hash)
        {
            descendants_list.retain(|hash| hash != block_hash);
            if descendants_list.is_empty() {
                self.descendants.remove(&parent_hash);
            }
        }
        for uncle_hash in &pending_block.share_block.header.uncles {
            if let Some(descendants_list) = self.descendants.get_mut(uncle_hash) {
                descendants_list.retain(|hash| hash != block_hash);
                if descendants_list.is_empty() {
                    self.descendants.remove(uncle_hash);
                }
            }
        }

        Some(pending_block.share_block)
    }

    /// Height below which block bodies are never fetched, so ancestry checks
    /// must not require them.
    ///
    /// Is 0 -- bodies required everywhere -- before there is a candidate
    /// chain. When the candidate tip cannot be read, falls back to the last
    /// height read successfully rather than 0: once bodies are pruned, 0
    /// would demand bodies that no longer exist.
    fn current_prune_height(&mut self) -> u32 {
        match self.chain_store_handle.get_candidate_tip_height() {
            Ok(Some(tip_height)) => {
                self.last_prune_height = prune_height_for_tip(tip_height);
                self.last_prune_height
            }
            Ok(None) => 0,
            Err(error) => {
                warn!(
                    "Could not read candidate tip height for prune boundary, using last known {}: {error}",
                    self.last_prune_height
                );
                self.last_prune_height
            }
        }
    }

    /// Whether the block's parent and uncles have the data validation needs.
    ///
    /// Each must have its block body stored, not merely its header.
    ///
    /// Blocks below `prune_height` are exempt because their bodies are never
    /// fetched; requiring one would stall the chain at the prune boundary.
    ///
    /// An ancestor with no metadata has simply not arrived: the store reports
    /// that as an error and the block is buffered rather than dropped.
    fn ancestry_ready(&self, share_block: &ShareBlock, prune_height: u32) -> bool {
        let header = &share_block.header;
        let mut ancestors = Vec::with_capacity(1 + header.uncles.len());
        if !is_terminal_blockhash(&header.prev_share_blockhash) {
            ancestors.push(header.prev_share_blockhash);
        }
        ancestors.extend_from_slice(&header.uncles);

        match self
            .chain_store_handle
            .all_block_and_uncle_data_available(&ancestors, prune_height)
        {
            Ok(ready) => ready,
            Err(error) => {
                debug!(
                    "Ancestry of block {} not ready: {error}",
                    share_block.block_hash()
                );
                false
            }
        }
    }

    /// ASERT difficulty check against the parent as stored.
    fn validate_asert(&self, share_block: &ShareBlock) -> Result<(), Box<dyn Error + Send + Sync>> {
        let parent_hash = share_block.header.prev_share_blockhash;
        let parent = self.chain_store_handle.get_asert_parent(&parent_hash)?;
        let expected_bits = self
            .share_validator
            .pool_difficulty()
            .calculate_target_clamped(parent);
        if share_block.header.bits != expected_bits {
            let block_hash = share_block.block_hash();
            return Err(format!(
                "ASERT mismatch for {block_hash}: declared bits {:#010x}, expected {:#010x}",
                share_block.header.bits.to_consensus(),
                expected_bits.to_consensus()
            )
            .into());
        }
        Ok(())
    }

    /// The header checks a block passes before it is stored and organised:
    /// ASERT difficulty, then the timestamp rules. A failure drops the block.
    fn validate_header_before_storing(
        &self,
        share_block: &ShareBlock,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.validate_asert(share_block)?;
        self.validate_timestamp(share_block)
    }

    /// Timestamp rules against the stored ancestry, as header sync applies
    /// them (`ShareValidator::validate_header_timestamp`). Ancestry is ready,
    /// so the ancestor headers are stored. Run before the header is
    /// organised, so a block that never went through header sync, such as a
    /// broadcast one below the full-validation zone, has its time checked
    /// too.
    fn validate_timestamp(
        &self,
        share_block: &ShareBlock,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        let ancestor_times = stored_ancestor_times(
            share_block.header.prev_share_blockhash,
            &self.chain_store_handle,
        )?;
        self.share_validator
            .validate_header_timestamp(&share_block.header, &ancestor_times)?;
        Ok(())
    }

    /// Persist a committed block, organise its header, and queue it
    /// for full validation by the validation worker.
    ///
    /// The persist + organise step is performed in a single RocksDB
    /// write batch so a crash cannot leave the share stored but not
    /// organised on the candidate chain.
    async fn store_and_emit_validation(
        &self,
        block_hash: &BlockHash,
        share_block: ShareBlock,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        match self
            .chain_store_handle
            .add_share_block_and_organise_header(share_block)
            .await
        {
            Ok(_) => {}
            Err(StoreError::BelowPruneHeight(_)) => {
                debug!("Not storing block {block_hash}: below the prune target");
                return Ok(());
            }
            Err(error) => {
                error!("Failed to store and organise block {block_hash}: {error}");
                return Err(error.into());
            }
        }

        if let Err(error) = self
            .validation_tx
            .send(ValidationEvent::ValidateBlockHash(*block_hash))
            .await
        {
            error!("Failed to send ValidateBlockHash for {block_hash}: {error}");
            return Err(error.into());
        }

        debug!("Committed and queued validation for block {block_hash}");
        Ok(())
    }

    /// Process an incoming share block.
    ///
    /// Fast path: if the block is already known to the store, drop or
    /// re-emit validation as appropriate.
    ///
    /// A block whose known height is below the prune height is dropped
    /// without storing: its body is never needed and would be pruned.
    ///
    /// Otherwise, if the block's direct parent and all uncles are at
    /// HeaderValid+, validate ASERT, persist, then drive any
    /// descendants buffered in pending. If the parent or uncles are
    /// not yet ready, buffer in pending. The headers-first pipeline
    /// will supply ancestors via header sync and block fetch.
    async fn process_share_block(
        &mut self,
        share_block: ShareBlock,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.process_share_block_at(share_block, Instant::now())
            .await
    }

    /// `process_share_block` with the arrival time given, so the pending
    /// buffer's expiry can be tested. Every arrival first drops the pending
    /// blocks that have waited longer than `max_age`.
    async fn process_share_block_at(
        &mut self,
        share_block: ShareBlock,
        now: Instant,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.expire_pending(now);
        let block_hash = share_block.block_hash();

        // Already-known fast path: only skip work for terminal states.
        // For HeaderValid / Candidate / Confirmed, the header may have
        // been organised during header sync but the block body was not
        // stored, so we still need to call add_share_block via the
        // normal flow below.
        let known_height = match self.chain_store_handle.get_block_metadata(&block_hash) {
            Ok(metadata) => match metadata.status {
                Status::BlockValid => {
                    debug!("Block {block_hash} already BlockValid, ignoring");
                    return Ok(());
                }
                Status::Invalid => {
                    debug!("Block {block_hash} already marked Invalid, dropping");
                    return Ok(());
                }
                _ => metadata.expected_height,
            },
            Err(_) => None,
        };

        // Notify block fetcher that we received this block so it can
        // clear any in-flight request.
        let _ = self
            .block_fetcher_handle
            .send(BlockFetcherEvent::BlockRequestCompleted(block_hash))
            .await;

        let prune_height = self.current_prune_height();
        if known_height.is_some_and(|height| height < prune_height) {
            debug!("Dropping block {block_hash}: below prune height {prune_height}");
            self.remove_from_pending(&block_hash);
            return Ok(());
        }
        if !self.ancestry_ready(&share_block, prune_height) {
            debug!("Block {block_hash} has unready ancestors, buffering");
            self.add_to_pending(block_hash, share_block, now);
            return Ok(());
        }

        self.remove_from_pending(&block_hash);

        // Ancestry is ready: check ASERT and the timestamp, then persist.
        if let Err(error) = self.validate_header_before_storing(&share_block) {
            warn!("Dropping block {block_hash}: {error}");
            return Ok(());
        }
        self.store_and_emit_validation(&block_hash, share_block)
            .await?;

        // Drive any pending descendants now unblocked by this commit.
        self.drive_descendants(block_hash).await
    }

    /// Iteratively re-process pending descendants of `just_committed`,
    /// then descendants of those that successfully commit, etc. Each
    /// descendant is re-checked against the full ancestry-ready
    /// predicate; if it is still waiting on a different parent or
    /// uncle, it remains in pending untouched.
    async fn drive_descendants(
        &mut self,
        just_committed: BlockHash,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        let mut queue: VecDeque<BlockHash> = VecDeque::with_capacity(4);
        if let Some(descendants_list) = self.descendants.get(&just_committed) {
            queue.extend(descendants_list.iter().copied());
        }
        let prune_height = self.current_prune_height();

        while let Some(descendant_hash) = queue.pop_front() {
            let Some(pending_block) = self.pending.get(&descendant_hash) else {
                continue;
            };
            let share_block = pending_block.share_block.clone();

            if !self.ancestry_ready(&share_block, prune_height) {
                // Still waiting on a different ancestor; leave in pending.
                continue;
            }

            if let Err(error) = self.validate_header_before_storing(&share_block) {
                warn!("Dropping cascaded block {descendant_hash}: {error}");
                self.remove_from_pending(&descendant_hash);
                continue;
            }

            // remove_from_pending before storing so we don't keep a
            // duplicate copy in memory while the store write runs.
            self.remove_from_pending(&descendant_hash);
            self.store_and_emit_validation(&descendant_hash, share_block)
                .await?;

            if let Some(next_descendants) = self.descendants.get(&descendant_hash) {
                queue.extend(next_descendants.iter().copied());
            }
        }

        Ok(())
    }

    /// Run the BlockReceiver event loop.
    ///
    /// Processes incoming share blocks. Runs until the event channel
    /// is closed.
    pub async fn run(mut self) {
        loop {
            match self.event_rx.recv().await {
                Some(BlockReceiverEvent::ShareBlockReceived {
                    share_block,
                    result_tx,
                }) => {
                    let result = self.process_share_block(share_block).await;
                    let _ = result_tx.send(result);
                }
                None => {
                    info!("BlockReceiver channel closed, shutting down");
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounting::payout::sharechain_pplns::pplns_window::PRUNE_DEPTH;
    use crate::node::request_response_handler::block_fetcher;
    use crate::node::validation_worker;
    use crate::pool_difficulty::AsertParent;
    #[mockall_double::double]
    use crate::pool_difficulty::PoolDifficulty;
    use crate::shares::validation::{BLOCK_TXS_SIZE_LIMIT, MockDefaultShareValidator};
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

        receiver.add_to_pending(block_hash, share_block, Instant::now());

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

        receiver.add_to_pending(child_hash, child_block, Instant::now());

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

        receiver.add_to_pending(block_hash, share_block, Instant::now());

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

        receiver.add_to_pending(child_hash, child_block, Instant::now());
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

        receiver.add_to_pending(block_hash, share_block, Instant::now());

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
        mock_validator
            .expect_validate_header_timestamp()
            .returning(|_, _| Ok(()));
        mock_store
            .expect_get_share_header()
            .returning(|_| Ok(TestShareBlockBuilder::new().build().header));
        let mut pool_difficulty = PoolDifficulty::default();
        pool_difficulty
            .expect_calculate_target_clamped()
            .returning(|_| {
                CompactTarget::from_consensus(crate::shares::share_block::MAX_POOL_TARGET)
            });
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

    /// A block whose timestamp breaks the rules header sync applies is
    /// dropped before it is stored or organised, as an ASERT mismatch is. The
    /// check reads the parent's stored header for median time past.
    #[tokio::test]
    async fn test_process_share_block_with_an_invalid_timestamp_is_dropped() {
        let parent_hash = BlockHash::from_byte_array([0x88; 32]);
        let parent_header = TestShareBlockBuilder::new()
            .nonce(0xe9695790)
            .time(1_700_000_000)
            .build()
            .header;
        let parent_header_for_asert = parent_header.clone();

        let mut mock_store = ChainStoreHandle::default();
        mock_store
            .expect_get_candidate_tip_height()
            .returning(|| Ok(Some(0)));
        mock_store
            .expect_all_block_and_uncle_data_available()
            .returning(|_, _| Ok(true));
        mock_store
            .expect_get_block_metadata()
            .returning(|_| Err(StoreError::NotFound("not found".to_string())));
        mock_store
            .expect_get_asert_parent()
            .with(mockall::predicate::eq(parent_hash))
            .returning(move |_| {
                Ok(AsertParent::from_header(
                    &parent_header_for_asert,
                    0,
                    parent_header_for_asert.time,
                ))
            });
        mock_store
            .expect_get_share_header()
            .with(mockall::predicate::eq(parent_hash))
            .returning(move |_| Ok(parent_header.clone()));
        mock_store
            .expect_add_share_block_and_organise_header()
            .never();

        let mut mock_validator = MockDefaultShareValidator::default();
        mock_validator
            .expect_validate_header_timestamp()
            .withf(|_, ancestor_times| ancestor_times == [1_700_000_000])
            .times(1)
            .returning(|_, _| {
                Err(crate::shares::validation::ValidationError::consensus(
                    "Share timestamp is not greater than median time past",
                ))
            });
        let mut pool_difficulty = PoolDifficulty::default();
        pool_difficulty
            .expect_calculate_target_clamped()
            .returning(|_| {
                CompactTarget::from_consensus(crate::shares::share_block::MAX_POOL_TARGET)
            });
        mock_validator
            .expect_pool_difficulty()
            .return_const(pool_difficulty);

        let (_, event_rx) = create_block_receiver_channel();
        let (block_fetcher_handle, _block_fetcher_rx) =
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
            .time(1_700_000_000)
            .build();
        child_block.header.bits =
            CompactTarget::from_consensus(crate::shares::share_block::MAX_POOL_TARGET);

        let result = receiver.process_share_block(child_block).await;

        assert!(result.is_ok(), "Expected Ok, got: {}", result.unwrap_err());
        assert_eq!(receiver.pending_count(), 0);
        assert!(validation_rx.try_recv().is_err());
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
        mock_validator
            .expect_validate_header_timestamp()
            .returning(|_, _| Ok(()));
        mock_store
            .expect_get_share_header()
            .returning(|_| Ok(TestShareBlockBuilder::new().build().header));
        let mut pool_difficulty = PoolDifficulty::default();
        pool_difficulty
            .expect_calculate_target_clamped()
            .returning(|_| {
                CompactTarget::from_consensus(crate::shares::share_block::MAX_POOL_TARGET)
            });
        mock_validator
            .expect_pool_difficulty()
            .return_const(pool_difficulty);

        let (_, event_rx) = create_block_receiver_channel();
        let (block_fetcher_handle, _block_fetcher_rx) =
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

    /// Below its bounds the receiver buffers whatever it is given: every
    /// block naming a parent that has not arrived takes its own slot.
    ///
    /// The admission gate in `handle_share_block` runs
    /// `validate_coinbase_proof`, which binds the share fields --
    /// `prev_share_blockhash` included -- to the bitcoin header's proof of
    /// work, so each block reaching this buffer costs a share's worth of real
    /// work. The buffer's own bounds (`PendingLimits`) cap what that work can
    /// hold; see the eviction and expiry tests below. This test drives the
    /// receiver directly, below that gate.
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
        let (block_fetcher_handle, _block_fetcher_rx) =
            block_fetcher::create_block_fetcher_channel();
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

    /// Over `max_blocks` the oldest pending block is evicted, with its
    /// descendants-index entries and its bytes.
    #[test]
    fn test_pending_buffer_evicts_the_oldest_block_beyond_its_block_cap() {
        let (_, event_rx) = create_block_receiver_channel();
        let (block_fetcher_handle, _) = block_fetcher::create_block_fetcher_channel();
        let (validation_tx, _) = validation_worker::create_validation_channel();
        let mut receiver = BlockReceiver::new(
            event_rx,
            Arc::new(MockDefaultShareValidator::default()),
            ChainStoreHandle::default(),
            block_fetcher_handle,
            validation_tx,
        )
        .with_pending_limits(PendingLimits {
            max_blocks: 2,
            ..PendingLimits::default()
        });
        let first = TestShareBlockBuilder::new()
            .prev_share_blockhash(BlockHash::from_byte_array([1; 32]).to_string())
            .build();
        let second = TestShareBlockBuilder::new()
            .prev_share_blockhash(BlockHash::from_byte_array([2; 32]).to_string())
            .build();
        let third = TestShareBlockBuilder::new()
            .prev_share_blockhash(BlockHash::from_byte_array([3; 32]).to_string())
            .build();
        let second_size = buffered_size(&second);
        let third_size = buffered_size(&third);
        let start = Instant::now();

        receiver.add_to_pending(first.block_hash(), first.clone(), start);
        receiver.add_to_pending(second.block_hash(), second.clone(), start);
        receiver.add_to_pending(third.block_hash(), third.clone(), start);

        assert_eq!(receiver.pending_count(), 2);
        assert!(!receiver.pending.contains_key(&first.block_hash()));
        assert!(receiver.pending.contains_key(&second.block_hash()));
        assert!(receiver.pending.contains_key(&third.block_hash()));
        assert!(
            !receiver
                .descendants
                .contains_key(&BlockHash::from_byte_array([1; 32]))
        );
        assert_eq!(receiver.pending_bytes(), second_size + third_size);
        assert_eq!(receiver.arrival_order.len(), 2);
    }

    /// Over `max_bytes` the oldest pending block is evicted too.
    #[test]
    fn test_pending_buffer_evicts_the_oldest_block_beyond_its_byte_cap() {
        let first = TestShareBlockBuilder::new()
            .prev_share_blockhash(BlockHash::from_byte_array([1; 32]).to_string())
            .build();
        let second = TestShareBlockBuilder::new()
            .prev_share_blockhash(BlockHash::from_byte_array([2; 32]).to_string())
            .build();
        let third = TestShareBlockBuilder::new()
            .prev_share_blockhash(BlockHash::from_byte_array([3; 32]).to_string())
            .build();
        let block_size = buffered_size(&first);
        let (_, event_rx) = create_block_receiver_channel();
        let (block_fetcher_handle, _) = block_fetcher::create_block_fetcher_channel();
        let (validation_tx, _) = validation_worker::create_validation_channel();
        let mut receiver = BlockReceiver::new(
            event_rx,
            Arc::new(MockDefaultShareValidator::default()),
            ChainStoreHandle::default(),
            block_fetcher_handle,
            validation_tx,
        )
        .with_pending_limits(PendingLimits {
            max_bytes: 2 * block_size + block_size / 2,
            ..PendingLimits::default()
        });
        let start = Instant::now();

        receiver.add_to_pending(first.block_hash(), first.clone(), start);
        receiver.add_to_pending(second.block_hash(), second.clone(), start);
        receiver.add_to_pending(third.block_hash(), third.clone(), start);

        assert_eq!(receiver.pending_count(), 2);
        assert!(!receiver.pending.contains_key(&first.block_hash()));
        assert!(receiver.pending_bytes() <= 2 * block_size + block_size / 2);
    }

    /// A block larger than the whole byte bound is never buffered.
    #[test]
    fn test_pending_buffer_drops_a_block_larger_than_its_byte_cap() {
        let block = TestShareBlockBuilder::new()
            .prev_share_blockhash(BlockHash::from_byte_array([1; 32]).to_string())
            .build();
        let (_, event_rx) = create_block_receiver_channel();
        let (block_fetcher_handle, _) = block_fetcher::create_block_fetcher_channel();
        let (validation_tx, _) = validation_worker::create_validation_channel();
        let mut receiver = BlockReceiver::new(
            event_rx,
            Arc::new(MockDefaultShareValidator::default()),
            ChainStoreHandle::default(),
            block_fetcher_handle,
            validation_tx,
        )
        .with_pending_limits(PendingLimits {
            max_bytes: buffered_size(&block) - 1,
            ..PendingLimits::default()
        });

        receiver.add_to_pending(block.block_hash(), block, Instant::now());

        assert_eq!(receiver.pending_count(), 0);
        assert_eq!(receiver.pending_bytes(), 0);
        assert!(receiver.descendants.is_empty());
    }

    /// A pending block expires once it has waited longer than `max_age`, and
    /// not before.
    #[test]
    fn test_pending_block_expires_after_its_max_age() {
        let (_, event_rx) = create_block_receiver_channel();
        let (block_fetcher_handle, _) = block_fetcher::create_block_fetcher_channel();
        let (validation_tx, _) = validation_worker::create_validation_channel();
        let mut receiver = BlockReceiver::new(
            event_rx,
            Arc::new(MockDefaultShareValidator::default()),
            ChainStoreHandle::default(),
            block_fetcher_handle,
            validation_tx,
        )
        .with_pending_limits(PendingLimits {
            max_age: Duration::from_secs(10),
            ..PendingLimits::default()
        });
        let block = TestShareBlockBuilder::new()
            .prev_share_blockhash(BlockHash::from_byte_array([1; 32]).to_string())
            .build();
        let received_at = Instant::now();
        receiver.add_to_pending(block.block_hash(), block.clone(), received_at);

        receiver.expire_pending(received_at + Duration::from_secs(10));
        assert_eq!(receiver.pending_count(), 1);

        receiver.expire_pending(received_at + Duration::from_secs(11));
        assert_eq!(receiver.pending_count(), 0);
        assert_eq!(receiver.pending_bytes(), 0);
        assert!(receiver.descendants.is_empty());
        assert!(receiver.arrival_order.is_empty());
    }

    /// Expiry runs on every arrival: a block that waited past
    /// `PENDING_BLOCK_TTL` is gone once the next block comes in.
    #[tokio::test]
    async fn test_process_share_block_drops_pending_blocks_past_their_ttl() {
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
        let (block_fetcher_handle, _block_fetcher_rx) =
            block_fetcher::create_block_fetcher_channel();
        let (validation_tx, _validation_rx) = validation_worker::create_validation_channel();
        let mut receiver = BlockReceiver::new(
            event_rx,
            Arc::new(MockDefaultShareValidator::default()),
            mock_store,
            block_fetcher_handle,
            validation_tx,
        );
        let stale = TestShareBlockBuilder::new()
            .prev_share_blockhash(BlockHash::from_byte_array([0x91; 32]).to_string())
            .build();
        let fresh = TestShareBlockBuilder::new()
            .prev_share_blockhash(BlockHash::from_byte_array([0x92; 32]).to_string())
            .build();
        let start = Instant::now();

        receiver
            .process_share_block_at(stale.clone(), start)
            .await
            .unwrap();
        receiver
            .process_share_block_at(
                fresh.clone(),
                start + PENDING_BLOCK_TTL + Duration::from_secs(1),
            )
            .await
            .unwrap();

        assert_eq!(receiver.pending_count(), 1);
        assert!(!receiver.pending.contains_key(&stale.block_hash()));
        assert!(receiver.pending.contains_key(&fresh.block_hash()));
    }

    /// The default bounds hold at least 160 blocks of the largest size the
    /// admission gate lets through, and at most 32 MiB.
    #[test]
    fn test_default_pending_limits_hold_enough_of_the_largest_blocks() {
        let limits = PendingLimits::default();

        assert_eq!(limits.max_bytes, 32 * 1024 * 1024);
        assert!(limits.max_bytes / BLOCK_TXS_SIZE_LIMIT as usize >= 160);
        assert!(limits.max_blocks >= 160);
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
        mock_validator
            .expect_validate_header_timestamp()
            .returning(|_, _| Ok(()));
        mock_store
            .expect_get_share_header()
            .returning(|_| Ok(TestShareBlockBuilder::new().build().header));
        let mut pool_difficulty = PoolDifficulty::default();
        pool_difficulty
            .expect_calculate_target_clamped()
            .returning(|_| {
                CompactTarget::from_consensus(crate::shares::share_block::MAX_POOL_TARGET)
            });
        mock_validator
            .expect_pool_difficulty()
            .return_const(pool_difficulty);

        let (_, event_rx) = create_block_receiver_channel();
        let (block_fetcher_handle, _block_fetcher_rx) =
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
        let (block_fetcher_handle, _block_fetcher_rx) =
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
        mock_validator
            .expect_validate_header_timestamp()
            .returning(|_, _| Ok(()));
        mock_store
            .expect_get_share_header()
            .returning(|_| Ok(TestShareBlockBuilder::new().build().header));
        let mut pool_difficulty = PoolDifficulty::default();
        pool_difficulty
            .expect_calculate_target_clamped()
            .returning(|_| {
                CompactTarget::from_consensus(crate::shares::share_block::MAX_POOL_TARGET)
            });
        mock_validator
            .expect_pool_difficulty()
            .return_const(pool_difficulty);

        let (_, event_rx) = create_block_receiver_channel();
        let (block_fetcher_handle, _block_fetcher_rx) =
            block_fetcher::create_block_fetcher_channel();
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
        mock_validator
            .expect_validate_header_timestamp()
            .returning(|_, _| Ok(()));
        mock_store
            .expect_get_share_header()
            .returning(|_| Ok(TestShareBlockBuilder::new().build().header));
        let mut pool_difficulty = PoolDifficulty::default();
        pool_difficulty
            .expect_calculate_target_clamped()
            .returning(|_| {
                CompactTarget::from_consensus(crate::shares::share_block::MAX_POOL_TARGET)
            });
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
        let (block_fetcher_handle, _block_fetcher_rx) =
            block_fetcher::create_block_fetcher_channel();
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
}
