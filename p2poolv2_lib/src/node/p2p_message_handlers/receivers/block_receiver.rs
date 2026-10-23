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
use crate::shares::validation::ShareValidator;
use crate::store::block_tx_metadata::Status;
use crate::store::prune_blocks::prune_height_for_tip;
use crate::store::writer::StoreError;
use bitcoin::BlockHash;
use std::collections::{HashMap, VecDeque};
use std::error::Error;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, error, info, warn};

/// Channel capacity for block receiver events.
const BLOCK_RECEIVER_CHANNEL_CAPACITY: usize = 8192;

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
}

/// Buffers incoming ShareBlocks until their direct parent and uncles are
/// at status HeaderValid (or better) in the store, then validates ASERT
/// difficulty, persists the block, and queues it for full validation.
///
/// Each block is processed in isolation against its parent's metadata
/// from the store. When a block becomes ready and is committed, any
/// previously-buffered descendants whose ancestry is now complete are
/// driven to commit via an iterative descendant worklist.
pub struct BlockReceiver {
    event_rx: BlockReceiverReceiver,
    /// Pending blocks indexed by their block hash.
    pending: HashMap<BlockHash, PendingBlock>,
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
            pending: HashMap::new(),
            descendants: HashMap::new(),
            share_validator,
            chain_store_handle,
            block_fetcher_handle,
            validation_tx,
            last_prune_height: 0,
        }
    }

    /// Number of blocks currently in the pending set.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Add a block to the pending set.
    /// Updates the descendants index for parent and uncles.
    fn add_to_pending(&mut self, block_hash: BlockHash, share_block: ShareBlock) {
        if self.pending.contains_key(&block_hash) {
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

        self.pending
            .insert(block_hash, PendingBlock { share_block });
    }

    /// Remove a block from the pending set and clean up its entries
    /// in the descendants index.
    fn remove_from_pending(&mut self, block_hash: &BlockHash) -> Option<ShareBlock> {
        let pending_block = self.pending.remove(block_hash)?;

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
            self.add_to_pending(block_hash, share_block);
            return Ok(());
        }

        self.remove_from_pending(&block_hash);

        // Ancestry is ready: ASERT-check and persist this block.
        if let Err(error) = self.validate_asert(&share_block) {
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

            if let Err(error) = self.validate_asert(&share_block) {
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
mod tests;
