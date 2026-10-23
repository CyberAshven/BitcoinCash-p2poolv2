// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Organisation worker that updates candidate and confirmed indexes.
//!
//! Receives OrganiseEvent values and triggers atomic organisation of the
//! candidate/confirmed indexes in the chain store. Runs in a dedicated
//! tokio task, decoupled from share producers (emission worker, peer
//! handler, future validation worker).
//!
//! After promoting a block to confirmed, updates the PplnsWindow cache.

#[cfg(test)]
#[mockall_double::double]
use crate::accounting::payout::sharechain_pplns::PplnsWindow;
#[cfg(not(test))]
use crate::accounting::payout::sharechain_pplns::PplnsWindow;
use crate::accounting::stats::metrics::MetricsHandle;
use crate::monitoring_events::{MonitoringEvent, MonitoringEventSender};
use crate::node::validation_worker::{ValidationEvent, ValidationSender};
#[cfg(test)]
#[mockall_double::double]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
#[cfg(not(test))]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
use crate::shares::share_block::{ShareBlock, ShareHeader};
use crate::shares::validation::FailureKind;
use crate::shares::validation::ShareValidator;
use crate::shares::validation::check_pplns_zone;
use crate::store::block_tx_metadata::{ChainMembership, Status};
use crate::store::dag_store::ShareInfo;
use crate::store::writer::StoreError;
use crate::stratum::work::notify::{NotifyCmd, NotifySender};
use bitcoin::BlockHash;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, RwLock};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tracing::{debug, error, info, warn};

/// Channel capacity for shares pending organisation.
///
/// Both this and `PENDING_BLOCKS_CAPACITY` hold whole `ShareBlock`s, which
/// `handle_share_block` bounds at `BLOCK_TXS_SIZE_LIMIT` (200 KB) each on
/// Bitcoin and at `BCH_SHARE_BODY_SIZE_LIMIT` (1.2 MB) on Bitcoin Cash, so the
/// capacities set the worst-case memory of the organise path: about 620 MB
/// for this channel full of the largest Bitcoin Cash shares, which have
/// passed validation and so carry valid proof of work. Bounding it in bytes
/// as well is a follow-up. They are kept
/// small deliberately: this channel is pure backpressure between the parallel
/// validation tasks and the serial organise worker, so a deeper queue buys no
/// throughput -- the organise worker is the bottleneck either way -- and only
/// adds latency between a block's two `check_pplns_zone` reads, which is the
/// window in which its validation tier can flip (see the PPLNS zone tiering
/// notes in docs/architecture/share-processing-pipeline.md).
const ORGANISE_CHANNEL_CAPACITY: usize = 512;

/// Maximum number of blocks buffered while waiting for ancestor
/// confirmation during sync.
///
/// Sized against the channel rather than picked independently: a block is
/// buffered when its parent is not yet `BlockValid`, and a child can sit at
/// most a full channel ahead of its parent, so the buffer must be able to hold
/// a channel's worth of out-of-order arrivals. The headroom matters because
/// overflow drops a block that is already stored, which nothing re-fetches.
const PENDING_BLOCKS_CAPACITY: usize = 2 * ORGANISE_CHANNEL_CAPACITY;

/// Upper bound on how many stranded blocks the startup re-validation seed
/// emits at once.
///
/// Kept below `PENDING_BLOCKS_CAPACITY` so the seed alone cannot overflow the
/// pending buffer; the inline re-drive (`redrive_stranded_parent`) carries any
/// blocks beyond the window as promotion advances.
const REVALIDATION_WINDOW: usize = 512;

/// Events for the organise worker.
// Block dominates the traffic and InvalidBlock is rare, so allow variant.
#[allow(clippy::large_enum_variant)]
pub enum OrganiseEvent {
    /// Promote candidates to confirmed after a block is validated.
    Block {
        /// The validated block.
        share_block: ShareBlock,
        /// Whether stage 1 ran full content validation (`validate_share_block`)
        /// rather than the below-zone `validate_below_pplns_depth`.
        ///
        /// The two stages read `check_pplns_zone` independently, and the
        /// candidate tip can shorten between them, so the organise worker
        /// cannot re-derive which checks actually ran. See
        /// `validate_mark_promote`.
        content_validated: bool,
    },
    /// Mark a block that failed pre-context validation Invalid.
    ///
    /// The validation worker detects these failures but does not mutate chain
    /// state, so it reports the verdict here.
    InvalidBlock(BlockHash),
}

/// Sender half of the organise channel.
pub type OrganiseSender = mpsc::Sender<OrganiseEvent>;
/// Receiver half of the organise channel.
pub type OrganiseReceiver = mpsc::Receiver<OrganiseEvent>;

/// Create an organise channel with bounded capacity.
pub fn create_organise_channel() -> (OrganiseSender, OrganiseReceiver) {
    mpsc::channel(ORGANISE_CHANNEL_CAPACITY)
}

/// A block waiting for its parent, with the validation tier stage 1 gave it.
struct BufferedBlock {
    /// Carried rather than recomputed: `buffer_block` compares it against every
    /// entry already at the height
    block_hash: BlockHash,
    share_block: ShareBlock,
    content_validated: bool,
}

/// Fatal error from the organise worker.
///
/// Returned when the worker encounters an unrecoverable failure,
/// indicating the node should shut down.
#[derive(Debug)]
pub struct OrganiseError {
    message: String,
}

impl fmt::Display for OrganiseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "OrganiseError: {}", self.message)
    }
}

impl std::error::Error for OrganiseError {}

/// Worker that triggers organisation of headers and blocks.
///
/// Receives `OrganiseEvent` values that have already been stored in the
/// chain and triggers atomic updates to the candidate/confirmed indexes
/// via `ChainStoreHandle`. After promoting a block, updates the shared
/// PplnsWindow cache.
pub struct OrganiseWorker {
    organise_rx: OrganiseReceiver,
    /// Re-drives validation of a stranded parent (a block whose body is stored
    /// but which never became `BlockValid`) when a descendant arrives on top of
    /// it. Best-effort via `try_send`, so a full channel defers the nudge to the
    /// next arrival rather than blocking the worker.
    validation_tx: ValidationSender,
    chain_store_handle: ChainStoreHandle,
    monitoring_event_sender: MonitoringEventSender,
    notify_tx: NotifySender,
    pplns_window: Arc<RwLock<PplnsWindow>>,
    share_validator: Arc<dyn ShareValidator + Send + Sync>,
    metrics: MetricsHandle,
    /// Blocks deferred because their parent is not yet validated, keyed by the
    /// parent's expected height. Multiple blocks may share the same parent
    /// height during forks or rapid sync. Drained when the parent becomes
    /// BlockValid or confirmed.
    pending_blocks: BTreeMap<u32, Vec<BufferedBlock>>,
}

/// Validation state of a block's parent, deciding how the block is handled.
///
/// Chain-context validation runs only when the parent is `Valid`; until then
/// the block is deferred. This keeps the `BlockValid` chain transitive from the
/// confirmed tip up, and guarantees that once we do validate, every dependency
/// is present in the store.
enum ParentState {
    /// Parent is `BlockValid` or on the confirmed chain. Carries its height.
    Valid(u32),
    /// Parent is `HeaderValid` and its body is stored, yet it never became
    /// `BlockValid` -- stranded (its validation event was lost on restart, or
    /// dropped). Re-drive its validation, then defer this block behind it.
    /// Carries the parent height used as the buffer key.
    Stranded(u32),
    /// Parent is only `HeaderValid` without its body yet, or `Pending`; defer
    /// the block and wait for the fetcher / the parent's own validation.
    /// Carries the parent height used as the buffer key.
    Pending(u32),
    /// Parent is `Invalid`; the block is invalid by descent.
    Invalid,
    /// Parent metadata is missing or has no height. This is an anomaly (the
    /// block receiver admits a block only after its parent is `HeaderValid`),
    /// so the block is dropped and left for re-delivery.
    Unknown,
}

/// Outcome of processing one share block through organise.
enum ProcessOutcome {
    /// The block became `BlockValid` or was promoted. Carries the block's own
    /// height, from which dependent buffered blocks are drained.
    Advanced(u32),
    /// The block was deferred (parent not yet validated).
    Buffered,
    /// The block was dropped (invalid-by-descent or unknown parent).
    Dropped,
    /// The block failed chain-context validation and was marked Invalid, which
    /// reorged the candidate chain -- rebuilding it from the invalid block's
    /// parent onto the best surviving branch (a sibling if one remains, else the
    /// parent itself). Confirmation should try to advance onto the rebuilt chain.
    Invalidated,
    /// Validated with a valid parent but did not advance: an unexpected
    /// Recoverable error, or a failed BlockValid mark or promote.
    NotAdvanced,
}

impl OrganiseWorker {
    /// Creates a new organise worker.
    #[allow(clippy::too_many_arguments)] // wiring constructor: each parameter is a distinct collaborator, a params struct would only move the list
    pub fn new(
        organise_rx: OrganiseReceiver,
        validation_tx: ValidationSender,
        chain_store_handle: ChainStoreHandle,
        monitoring_event_sender: MonitoringEventSender,
        notify_tx: NotifySender,
        metrics: MetricsHandle,
        pplns_window: Arc<RwLock<PplnsWindow>>,
        share_validator: Arc<dyn ShareValidator + Send + Sync>,
    ) -> Self {
        Self {
            organise_rx,
            validation_tx,
            chain_store_handle,
            monitoring_event_sender,
            notify_tx,
            pplns_window,
            share_validator,
            metrics,
            pending_blocks: BTreeMap::new(),
        }
    }

    /// Runs the organise worker until the channel closes or a fatal error occurs.
    ///
    /// Returns `Ok(())` on clean shutdown (channel closed).
    /// Returns `Err(OrganiseError)` on fatal failure (store writer dead).
    pub async fn run(mut self) -> Result<(), OrganiseError> {
        info!("Organise worker started");

        // Make sure pplns window is warmed up with current chain state in store
        self.update_pplns_window();

        // Resume any blocks stranded across a restart (body stored, still only
        // HeaderValid): nothing has arrived on top to trigger the inline
        // re-drive yet, so seed the cascade from the confirmed tip.
        self.seed_revalidation()?;

        while let Some(event) = self.organise_rx.recv().await {
            match event {
                OrganiseEvent::Block {
                    share_block,
                    content_validated,
                } => {
                    self.handle_organise_block_event(share_block, content_validated)
                        .await?;
                }
                OrganiseEvent::InvalidBlock(blockhash) => {
                    self.handle_invalid_block_event(blockhash).await?;
                }
            }
        }
        info!("Organise worker stopped - channel closed");
        Ok(())
    }

    /// Handle a single share block: gate it on its parent, then follow up.
    ///
    /// A block that advances (becomes BlockValid or is promoted) drains its
    /// dependents. A buffered block opportunistically tries to advance the
    /// confirmed chain from the candidate chain (which may have reorged). Only
    /// `StoreError::ChannelClosed` (and, in Phase 2, a store error during
    /// validation) is propagated as a fatal `OrganiseError`.
    async fn handle_organise_block_event(
        &mut self,
        share_block: ShareBlock,
        content_validated: bool,
    ) -> Result<(), OrganiseError> {
        match self
            .process_share_block(share_block, content_validated)
            .await?
        {
            ProcessOutcome::Advanced(height) => {
                self.drain_pending_blocks(height).await?;
            }
            ProcessOutcome::Buffered | ProcessOutcome::Invalidated => {
                // The candidate and confirmed chains advance on different events:
                // Header events reorg the candidate chain (organise_header)
                // without touching confirmed. Buffered: a Header event may have
                // reorged the candidate onto an already-BlockValid fork since the
                // last organise_block. Invalidated: mark_invalid just rebuilt the
                // candidate chain from the invalid block's parent (best surviving
                // branch). Either way, catch the confirmed chain up.
                self.advance_confirmed_and_drain().await?;
            }
            ProcessOutcome::Dropped | ProcessOutcome::NotAdvanced => {}
        }
        Ok(())
    }

    /// Mark a block that failed pre-context validation Invalid, then catch the
    /// confirmed chain up.
    ///
    /// Without this the block keeps its `HeaderValid` status on the candidate
    /// chain, and `contiguous_candidates_with_block_data` stops there, so
    /// confirmation stalls at that height until some branch out-works it.
    /// Marking it Invalid rebuilds the candidate chain from its parent onto the
    /// best surviving branch, the same follow-up a chain-context Consensus
    /// failure needs.
    ///
    /// When the mark fails nothing was committed, so there is no rebuilt chain
    /// to advance onto and the follow-up is skipped. Only the hash arrives here,
    /// so the block cannot be re-buffered for a local retry: the retry is a peer
    /// re-delivering it, which re-runs validation and reports the verdict again.
    async fn handle_invalid_block_event(
        &mut self,
        blockhash: BlockHash,
    ) -> Result<(), OrganiseError> {
        if let Err(mark_error) = self.chain_store_handle.mark_invalid(blockhash).await {
            if matches!(mark_error, StoreError::ChannelClosed) {
                return Err(OrganiseError {
                    message: format!("Store writer channel closed marking {blockhash} Invalid"),
                });
            }
            error!(
                "Failed to mark {blockhash} Invalid, confirmation stops at its height: {mark_error}"
            );
            return Ok(());
        }
        self.advance_confirmed_and_drain().await
    }

    /// Run organise_block to advance the confirmed chain, then drain the blocks
    /// buffered at each newly confirmed height.
    ///
    /// Reads the confirmed tip first so the drain covers every height the call
    /// confirms, not just the last one.
    async fn advance_confirmed_and_drain(&mut self) -> Result<(), OrganiseError> {
        let previous_tip = match self.chain_store_handle.get_tip_height() {
            Ok(tip) => tip,
            Err(error) => {
                error!("Error reading confirmed tip before drain: {error}");
                None
            }
        };
        match self.chain_store_handle.organise_block().await {
            Ok(Some(height)) => {
                self.confirmed_advanced(previous_tip, height).await;
                let from = previous_tip.map_or(height, |tip| (tip + 1).min(height));
                for drain_height in from..=height {
                    self.drain_pending_blocks(drain_height).await?;
                }
                Ok(())
            }
            Ok(None) => {
                // Confirmation did not advance, so something is wedging the
                // promotion prefix. That block's parent is valid -- which is
                // why it reached validation at all -- so confirmation stopped
                // exactly at the parent, and the block sits buffered under the
                // confirmed tip's own height. Draining here re-attempts it, and
                // drain_pending_blocks cascades upward if it now advances.
                if let Some(tip) = previous_tip {
                    self.drain_pending_blocks(tip).await?;
                }
                Ok(())
            }
            Err(StoreError::ChannelClosed) => Err(OrganiseError {
                message: "Store writer channel closed".to_string(),
            }),
            Err(error) => {
                error!("Error advancing confirmed chain: {error}");
                Ok(())
            }
        }
    }

    /// Gate a block on its parent's state, then buffer, drop, or
    /// validate-and-promote it. Shared by fresh events and the drain path.
    async fn process_share_block(
        &mut self,
        share_block: ShareBlock,
        content_validated: bool,
    ) -> Result<ProcessOutcome, OrganiseError> {
        let blockhash = share_block.block_hash();
        match self.parent_state(&share_block.header.prev_share_blockhash) {
            ParentState::Invalid => {
                debug!("Dropping {blockhash}: parent is Invalid (invalid by descent)");
                Ok(ProcessOutcome::Dropped)
            }
            ParentState::Unknown => {
                debug!("Dropping {blockhash}: parent metadata unavailable");
                Ok(ProcessOutcome::Dropped)
            }
            ParentState::Stranded(parent_height) => {
                self.redrive_stranded_parent(&share_block.header.prev_share_blockhash);
                self.buffer_block(parent_height, share_block, content_validated);
                Ok(ProcessOutcome::Buffered)
            }
            ParentState::Pending(parent_height) => {
                self.buffer_block(parent_height, share_block, content_validated);
                Ok(ProcessOutcome::Buffered)
            }
            ParentState::Valid(parent_height) => {
                self.validate_mark_promote(share_block, parent_height, content_validated)
                    .await
            }
        }
    }

    /// At startup, re-drive validation of the stranded run above the confirmed
    /// tip -- blocks whose bodies are stored but which were left `HeaderValid`
    /// when their validation events were lost on restart.
    ///
    /// The inline re-drive (`redrive_stranded_parent`) only fires when a
    /// descendant arrives on top of a stranded block; on a fresh restart nothing
    /// has arrived yet, so this seeds the cascade. Best-effort via `try_send`
    /// and bounded by `REVALIDATION_WINDOW` so it cannot itself overflow the
    /// pending buffer; validating the seeded run promotes it and the inline path
    /// carries any blocks beyond the window as the confirmed tip climbs.
    ///
    /// A store read failure (the confirmed tip or the stranded scan) is fatal:
    /// it must not be mistaken for an empty chain, which would run a full-history
    /// recovery scan and continue past a real store fault. `Ok(None)` for the
    /// tip is the genuine empty case and scans from the first height.
    fn seed_revalidation(&self) -> Result<(), OrganiseError> {
        let confirmed = match self.chain_store_handle.get_tip_height() {
            Ok(Some(height)) => height,
            // No confirmed tip yet (empty or genesis-only chain): scan from the
            // first height above genesis.
            Ok(None) => 0,
            Err(error) => {
                return Err(OrganiseError {
                    message: format!(
                        "Failed to read confirmed tip for startup re-validation: {error}"
                    ),
                });
            }
        };
        let stranded = self
            .chain_store_handle
            .get_candidate_blocks_needing_validation(confirmed + 1, REVALIDATION_WINDOW)
            .map_err(|error| OrganiseError {
                message: format!("Failed to scan for stranded blocks at startup: {error}"),
            })?;
        if stranded.is_empty() {
            return Ok(());
        }
        info!(
            "Re-driving validation of {} stranded block(s) above confirmed height {confirmed} at startup",
            stranded.len()
        );
        for blockhash in stranded {
            if let Err(send_error) = self
                .validation_tx
                .try_send(ValidationEvent::ValidateBlockHash(blockhash))
            {
                // A full or closed channel ends the seed but is not fatal: the
                // inline re-drive picks up the remainder as descendants arrive.
                debug!("Stopping startup re-validation seed at {blockhash}: {send_error}");
                return Ok(());
            }
        }
        Ok(())
    }

    /// Re-drive validation of a stranded parent -- a block whose body is stored
    /// but which never became `BlockValid` (its validation event was lost on
    /// restart, or dropped). `ValidateBlockHash` re-reads the block from the
    /// store and runs the full pipeline, and validating an already-`BlockValid`
    /// block is a no-op, so the nudge is idempotent.
    ///
    /// Best-effort by `try_send`: a blocking send onto `validation_tx` could
    /// deadlock the cycle organise -> validation -> organise if both bounded
    /// channels fill, so a full channel defers the nudge. The next descendant to
    /// arrive re-drives it, and the startup seed covers a restart with no
    /// arrivals.
    ///
    /// A stranded parent found while the node is caught up (`is_current`) means
    /// an event was lost in normal operation -- a real bug -- so it is logged at
    /// `warn`; during catch-up it is an expected restart/sync artefact logged at
    /// `debug`.
    fn redrive_stranded_parent(&self, parent_hash: &BlockHash) {
        if self.chain_store_handle.is_current() {
            warn!(
                "Parent {parent_hash} has its body but is not BlockValid while current; re-validating (a validation event was lost)"
            );
        } else {
            debug!("Re-driving validation of stranded parent {parent_hash}");
        }
        match self
            .validation_tx
            .try_send(ValidationEvent::ValidateBlockHash(*parent_hash))
        {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                debug!(
                    "Validation channel full; deferring re-drive of stranded parent {parent_hash}"
                );
            }
            Err(TrySendError::Closed(_)) => {
                error!("Validation channel closed; cannot re-drive stranded parent {parent_hash}");
            }
        }
    }

    /// Validate a block whose parent is already valid, mark it BlockValid, and
    /// promote it.
    ///
    /// In the PPLNS zone the block runs chain-context validation here; because
    /// the parent is validated every dependency is present, so a failure is a
    /// genuine consensus violation (the block is marked Invalid). Below the zone
    /// (the prune window) the block was already ASERT/PoW-validated by the
    /// validation worker (`validate_below_pplns_depth`), so it is marked
    /// BlockValid without re-validation.
    ///
    /// `content_validated` says which of those two the validation worker
    /// actually ran. Both stages read `check_pplns_zone` independently against
    /// the candidate tip, and that tip can *shorten* -- a reorg onto a shorter
    /// branch, or a rebuild after an invalidation -- so a block tiered below the
    /// zone at stage 1 can be back inside it here. Re-deriving the tier would
    /// then mark it BlockValid having never had its merkle root, coinbase,
    /// witness commitment or scripts checked. When that flip happens the skipped
    /// checks are run now, before anything else. The opposite flip needs no
    /// handling: content validation already ran, and skipping chain context is
    /// exactly the treatment every below-zone block gets.
    ///
    /// Both tiers are marked BlockValid before promotion, so the
    /// promotion gate -- which requires BlockValid down to the prune
    /// depth -- can confirm them. Returns `Advanced(height)` when the
    /// block becomes BlockValid or is promoted, so its dependents can
    /// be drained.
    async fn validate_mark_promote(
        &mut self,
        share_block: ShareBlock,
        parent_height: u32,
        content_validated: bool,
    ) -> Result<ProcessOutcome, OrganiseError> {
        let blockhash = share_block.block_hash();
        debug!("Organising block: {blockhash:?}");

        let in_pplns_zone = match check_pplns_zone(&blockhash, &self.chain_store_handle) {
            Ok(result) => result,
            Err(error_message) => {
                error!("Error checking for zone, will retry {blockhash}: {error_message}");
                self.buffer_block(parent_height, share_block, content_validated);
                return Ok(ProcessOutcome::NotAdvanced);
            }
        };

        let validation_result = if !in_pplns_zone {
            Ok(())
        } else if content_validated {
            self.share_validator.validate_with_chain_context(
                &share_block,
                &self.chain_store_handle,
                Arc::clone(&self.pplns_window),
            )
        } else {
            // The candidate tip shortened back across the boundary since stage
            // 1 tiered this block below the zone. Run the content checks it
            // skipped before the chain-context ones.
            self.share_validator
                .validate_share_block(&share_block, &self.chain_store_handle)
                .and_then(|()| {
                    self.share_validator.validate_with_chain_context(
                        &share_block,
                        &self.chain_store_handle,
                        Arc::clone(&self.pplns_window),
                    )
                })
        };

        if let Err(validation_error) = validation_result {
            match validation_error.kind() {
                FailureKind::Consensus => {
                    // The parent is validated, so all dependencies are present:
                    // this is a genuine consensus violation. Mark Invalid, which
                    // rebuilds the candidate chain from the invalid block's parent
                    // onto the best surviving branch, and return Invalidated so the
                    // caller advances confirmation onto it instead of waiting for
                    // the next event.
                    error!("Chain-context validation failed for {blockhash}: {validation_error}");
                    if let Err(mark_error) = self.chain_store_handle.mark_invalid(blockhash).await {
                        if matches!(mark_error, StoreError::ChannelClosed) {
                            return Err(OrganiseError {
                                message: format!(
                                    "Store writer channel closed marking {blockhash} Invalid"
                                ),
                            });
                        }
                        // The batch was dropped uncommitted, so the block still
                        // has its HeaderValid status and Candidate membership,
                        // and contiguous_candidates_with_block_data still stops
                        // at its height. Reporting Invalidated would send the
                        // caller to advance confirmation onto a rebuilt
                        // candidate chain that does not exist. Re-buffer so the
                        // verdict is re-attempted instead.
                        error!("Failed to mark {blockhash} Invalid, will retry: {mark_error}");
                        self.buffer_block(parent_height, share_block, content_validated);
                        return Ok(ProcessOutcome::NotAdvanced);
                    }
                    return Ok(ProcessOutcome::Invalidated);
                }
                FailureKind::StoreAccess => {
                    // The read against the store itself failed, or data whose
                    // absence can only mean corruption is gone. Verdicts a
                    // peer's block can provoke are classified Consensus or
                    // Recoverable, so reaching here is a local fault: fail fast
                    // rather than silently diverge.
                    error!(
                        "Fatal store error validating {blockhash} (parent is valid): {validation_error}"
                    );
                    return Err(OrganiseError {
                        message: format!(
                            "Store error validating {blockhash} with a valid parent: {validation_error}"
                        ),
                    });
                }
                FailureKind::Recoverable => {
                    // A chain-context check could not be decided because data
                    // it needs has not arrived yet (an ancestor header, a
                    // transient store read). The block may well be valid, so
                    // leave it for retry rather than marking it Invalid or
                    // crashing.
                    warn!(
                        "Recoverable validation error for {blockhash} at organise: {validation_error}"
                    );
                    self.buffer_block(parent_height, share_block, content_validated);
                    return Ok(ProcessOutcome::NotAdvanced);
                }
                FailureKind::Unresolvable => {
                    // What the check needs is gone rather than late -- an
                    // anchor below the confirmed entries the PPLNS window
                    // retains -- and the window's oldest entry only moves
                    // forward, so no retry can decide it.
                    warn!("Dropping {blockhash}, cannot be resolved: {validation_error}");
                    return Ok(ProcessOutcome::Dropped);
                }
            }
        }

        // Mark BlockValid for both promotion tiers before promoting.
        if let Err(mark_error) = self.chain_store_handle.mark_block_valid(blockhash).await {
            // A rejected transition means the block is Pending or already
            // Invalid, which no retry can change; anything else is a store
            // failure that may not recur.
            if matches!(mark_error, StoreError::InvalidStatusTransition(_)) {
                error!("Cannot mark {blockhash} BlockValid: {mark_error}");
            } else {
                error!("Failed to mark {blockhash} BlockValid, will retry: {mark_error}");
                self.buffer_block(parent_height, share_block, content_validated);
            }
            return Ok(ProcessOutcome::NotAdvanced);
        }

        // Read before promoting: promote_block returns the new confirmed tip,
        // and the follow-up needs the range it advanced over, not just its end.
        let previous_tip = self.chain_store_handle.get_tip_height().ok().flatten();
        match self
            .chain_store_handle
            .promote_block(share_block.header.clone())
            .await
        {
            Ok(Some(new_tip)) => {
                self.confirmed_advanced(previous_tip, new_tip).await;
                // Drain dependents from this block's own height, not the
                // confirmed height, so a promotion cascade doesn't skip them.
                Ok(ProcessOutcome::Advanced(parent_height + 1))
            }
            Ok(None) => {
                // Marked BlockValid above even if it did not promote to confirmed
                // yet (e.g. its parent is not confirmed), so its dependents can be
                // drained from its height.
                Ok(ProcessOutcome::Advanced(parent_height + 1))
            }
            Err(StoreError::ChannelClosed) => {
                error!("Store writer channel closed during promote block");
                Err(OrganiseError {
                    message: "Store writer channel closed".to_string(),
                })
            }
            Err(error) => {
                error!("Error promoting block {blockhash}, will retry: {error}");
                self.buffer_block(parent_height, share_block, content_validated);
                Ok(ProcessOutcome::NotAdvanced)
            }
        }
    }

    /// Classify a block's parent for the organise gate.
    ///
    /// `Valid` (BlockValid, or confirmed for below-zone blocks that are only
    /// HeaderValid) lets the block be validated and marked; `Pending` defers
    /// it; `Invalid` drops it by descent; `Unknown` (missing metadata/height)
    /// drops it for re-delivery.
    fn parent_state(&self, parent_hash: &BlockHash) -> ParentState {
        let metadata = match self.chain_store_handle.get_block_metadata(parent_hash) {
            Ok(metadata) => metadata,
            Err(error) => {
                debug!("Parent {parent_hash} metadata unavailable, deferring: {error}");
                return ParentState::Unknown;
            }
        };
        if metadata.status == Status::Invalid {
            return ParentState::Invalid;
        }
        let Some(height) = metadata.expected_height else {
            return ParentState::Unknown;
        };
        if metadata.status == Status::BlockValid || metadata.chain == ChainMembership::Confirmed {
            ParentState::Valid(height)
        } else if metadata.status == Status::HeaderValid
            && self.chain_store_handle.share_block_exists(parent_hash)
        {
            // Body stored but never promoted to BlockValid: the parent is
            // stranded, not merely waiting on the fetcher.
            ParentState::Stranded(height)
        } else {
            ParentState::Pending(height)
        }
    }

    /// Follow-up for a promotion that advanced the confirmed chain from
    /// `previous_tip` to `new_tip`.
    ///
    /// Shared by both paths that confirm blocks -- `validate_mark_promote` via
    /// `promote_block`, and `advance_confirmed_and_drain` via `organise_block`
    /// -- so the two cannot drift.
    ///
    /// Split across two granularities, which is what an earlier per-share
    /// version conflated. The PPLNS window tracks the confirmed tip, which
    /// moves once per promotion, and updating it takes the write lock every
    /// stratum job build contends for; doing it per block would take that lock
    /// once per confirmed block to accomplish what one call does. It also has
    /// to run before dependents are drained, since `validate_bitcoin_payout`
    /// reads the shared window without updating it. The share feed and the
    /// effort numerator are per block: one promotion can confirm a whole
    /// prefix, and every block in it did real work.
    async fn confirmed_advanced(&self, previous_tip: Option<u32>, new_tip: u32) {
        self.update_pplns_window();

        let from = previous_tip.map_or(new_tip, |tip| (tip + 1).min(new_tip));
        for height in from..=new_tip {
            self.record_confirmed_block(height).await;
        }

        match self.chain_store_handle.get_candidate_tip_height() {
            Ok(Some(candidate_tip_height)) if new_tip >= candidate_tip_height => {
                self.send_new_notify(new_tip, candidate_tip_height).await;
            }
            Ok(Some(_)) => {}
            _ => debug!("No candidate tip found"),
        }
    }

    /// Record one newly confirmed block: its work for the effort metric, a
    /// bitcoin block find if it is one, and a Share event for subscribers.
    ///
    /// Reads are current: the writer commits the confirmation batch before
    /// replying to `organise_block`, and the organise worker is the only thing
    /// driving it, so nothing moves the confirmed tip in between.
    ///
    /// A height inside a range just confirmed must resolve. If it does not,
    /// that is a store inconsistency rather than a fact about the chain, and a
    /// monitoring follow-up must not stop confirmation, so it is logged and
    /// skipped.
    async fn record_confirmed_block(&self, height: u32) {
        let header = match self
            .chain_store_handle
            .get_confirmed_at_height(height)
            .and_then(|blockhash| self.chain_store_handle.get_share_header(&blockhash))
        {
            Ok(header) => header,
            Err(error) => {
                warn!("No confirmed block to record at height {height}: {error}");
                return;
            }
        };

        // Feed the block effort accumulator with this confirmed share's pool
        // difficulty. Uncles are excluded so the effort numerator tracks the
        // same confirmed-chain work as the hashrate (derived from chain work).
        //
        // Skip while syncing: the confirmed chain replays the whole backlog,
        // and work_since_last_block never resets (no real bitcoin block is
        // found during replay), so it would balloon to the entire chain's work
        // and report an absurd effort. Only real-time confirmed shares count.
        if self.chain_store_handle.is_current() {
            let share_difficulty = bitcoin::Target::from_compact(header.bits).difficulty_float();
            let _ = self.metrics.record_confirmed_share(share_difficulty).await;

            // If this confirmed share is itself a bitcoin block (found by any
            // node's miners), reset the effort accumulator toward the next
            // block. The find itself is recorded in the store on confirmation.
            if header.meets_bitcoin_difficulty() {
                let _ = self.metrics.reset_block_effort().await;
            }
        }

        self.emit_share_monitoring_event(&header, height);
    }

    /// Insert a block into the pending buffer, keyed by its parent height.
    ///
    /// A block already buffered at that height is not added again.
    ///
    /// The check runs before the capacity check so a duplicate cannot trip it.
    ///
    /// Drops the block with an error log if the buffer is at capacity.
    /// The capacity check counts total blocks across all heights.
    fn buffer_block(
        &mut self,
        parent_height: u32,
        share_block: ShareBlock,
        content_validated: bool,
    ) {
        let block_hash = share_block.block_hash();
        if let Some(buffered) = self.pending_blocks.get(&parent_height)
            && buffered.iter().any(|entry| entry.block_hash == block_hash)
        {
            debug!("Block {block_hash} is already buffered at parent height {parent_height}");
            return;
        }

        let total_blocks: usize = self.pending_blocks.values().map(|v| v.len()).sum();
        if total_blocks >= PENDING_BLOCKS_CAPACITY {
            error!(
                "Pending block buffer full ({PENDING_BLOCKS_CAPACITY}), dropping block {block_hash}"
            );
            return;
        }
        debug!(
            "Buffering block {block_hash} at parent height {parent_height} (confirmed tip not yet reached)"
        );
        self.pending_blocks
            .entry(parent_height)
            .or_insert_with(|| Vec::with_capacity(2))
            .push(BufferedBlock {
                block_hash,
                share_block,
                content_validated,
            });
    }

    /// Re-attempt buffered blocks whose parent just became valid, from
    /// `from_height` upward.
    ///
    /// Blocks are keyed in the buffer by their parent's height, so the children
    /// of a block that just became valid at height `h` sit under key `h`. The
    /// scan walks heights contiguously from `from_height`, advancing to the
    /// next height only when the current one advanced something -- so it follows
    /// the newly-validated frontier and stops as soon as nothing more proceeds.
    /// This keeps a full sync linear rather than quadratic. A re-attempted block
    /// whose parent is still pending cheaply re-buffers (a metadata read, not a
    /// full validation).
    async fn drain_pending_blocks(&mut self, from_height: u32) -> Result<(), OrganiseError> {
        let mut height = Some(from_height);
        while let Some(current) = height
            && let Some(blocks) = self.pending_blocks.remove(&current)
        {
            debug!(
                "Draining {} buffered blocks waiting on height {current}",
                blocks.len()
            );
            let mut next_height = None;
            for buffered in blocks {
                if let ProcessOutcome::Advanced(advanced_height) = self
                    .process_share_block(buffered.share_block, buffered.content_validated)
                    .await?
                {
                    next_height = Some(advanced_height);
                }
            }
            height = next_height;
        }
        Ok(())
    }

    /// Update the shared PplnsWindow cache after a block is promoted.
    ///
    /// Acquires a write lock, calls update, and releases the lock before
    /// returning. This ensures the window reflects the latest confirmed
    /// chain state before dependents are scheduled for validation.
    fn update_pplns_window(&self) {
        let mut window = self
            .pplns_window
            .write()
            .expect("PPLNS window lock poisoned on write");
        if let Err(error) = window.update(&self.chain_store_handle) {
            error!("Failed to update PPLNS window: {error}");
        }
    }

    /// Send new notify message to workers after share chain is
    /// extended. This is important to keep uncles from being generated.
    ///
    /// Sends a NewNotify command to the notifier when the confirmed chain
    /// catches up to the candidate tip.
    async fn send_new_notify(&self, confirmed_height: u32, candidate_tip_height: u32) {
        debug!(
            "Confirmed height {confirmed_height} caught up to candidate tip {candidate_tip_height}. Sending new work to miners."
        );
        if self.notify_tx.send(NotifyCmd::NewNotify).await.is_err() {
            error!("Notify channel closed. Cannot send new work to miners.");
        }
    }

    /// Emits a Share monitoring event for a confirmed share.
    ///
    /// Looks up uncle details from the store so that subscribers receive
    /// full uncle information in a single event.
    fn emit_share_monitoring_event(&self, header: &ShareHeader, height: u32) {
        let uncle_infos = self.chain_store_handle.get_uncle_infos(&header.uncles);

        let share_info = ShareInfo {
            blockhash: header.block_hash(),
            prev_blockhash: header.prev_share_blockhash,
            height,
            miner_bitcoin_address: header.miner_bitcoin_address.clone(),
            miner_address: header.miner_address,
            timestamp: header.time,
            bits: header.bits,
            uncles: uncle_infos,
        };
        let event = MonitoringEvent::Share(share_info);
        if self.monitoring_event_sender.send(event).is_err() {
            debug!("No monitoring subscribers for Share event");
        }
    }
}

#[cfg(test)]
mod tests;
