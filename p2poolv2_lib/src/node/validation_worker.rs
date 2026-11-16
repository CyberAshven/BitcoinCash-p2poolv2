// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Validation worker that validates share blocks in capped concurrent tasks.
//!
//! Receives `ValidationEvent` values containing blockhash to validate. It reads the
//! corresponding share block from the chain store, validates them using
//! `shares::validation::validate_share_block`, and on success emits
//! `OrganiseEvent::Block` and `SwarmSend::BroadcastBlock` events. Runs in a dedicated
//! tokio task, decoupled from the P2P message handler hot path.
//!
//! A share is checked once at a time. Peers relay a share to every peer, and
//! a share stored but not yet confirmed is sent here again on each delivery
//! (`handle_share_block`), so the same hash often arrives several times while
//! its first check runs. The worker keeps the shares it has received and not
//! finished checking ([`CheckQueue`]): a delivery of one already queued keeps
//! its place, and a delivery of one being checked is only noted. When that
//! check ends without settling the share (a `Recoverable` or store failure),
//! the noted delivery queues it once more, as the delivery itself would have.
//! A delivery after the check ended is checked as before, which is also how
//! a node that restarted before checking a stored share checks it.

use crate::accounting::payout::sharechain_pplns::pplns_window::ChainDepths;
use crate::config::{BitcoinCashNetwork, Chain};
use crate::node::SwarmSend;
use crate::node::messages::Message;
use crate::node::organise_worker::{OrganiseEvent, OrganiseSender};
#[cfg(test)]
#[mockall_double::double]
use crate::pool_difficulty::PoolDifficulty;
#[cfg(not(test))]
use crate::pool_difficulty::PoolDifficulty;
#[cfg(test)]
#[mockall_double::double]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
#[cfg(not(test))]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
use crate::shares::share_block::ShareBlock;
use crate::shares::validation::check_pplns_zone;
use crate::shares::validation::{DefaultShareValidator, FailureKind, ShareValidator};
use crate::utils::cpu::available_cpus;
use bitcoin::{BlockHash, CompactTarget};
use libp2p::request_response::ResponseChannel;
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::Arc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
use tracing::{debug, error, info, warn};

/// Channel capacity for blocks pending validation. The worker's own queue of
/// shares waiting for a permit is bounded by it too, so a full queue leaves
/// the channel to fill and its senders to wait, as before the queue.
const VALIDATION_CHANNEL_CAPACITY: usize = 8192;

/// Events for the validation worker.
// ValidateShareBlock intentionally carries the block by value to avoid a
// redundant store read; that size difference is the point of the variant.
#[allow(clippy::large_enum_variant)]
pub enum ValidationEvent {
    /// Validate a stored ShareBlock by its hash (reads from store).
    ValidateBlockHash(BlockHash),
    /// Validate a ShareBlock already in memory (avoids redundant store read).
    ValidateShareBlock(ShareBlock),
}

/// Sender half of the validation channel.
pub type ValidationSender = mpsc::Sender<ValidationEvent>;
/// Receiver half of the validation channel.
pub type ValidationReceiver = mpsc::Receiver<ValidationEvent>;

/// Create a validation channel with bounded capacity.
pub fn create_validation_channel() -> (ValidationSender, ValidationReceiver) {
    mpsc::channel(VALIDATION_CHANNEL_CAPACITY)
}

/// Fatal error from the validation worker.
///
/// Returned when the worker encounters an unrecoverable failure,
/// indicating the node should shut down.
#[derive(Debug)]
pub struct ValidationWorkerError {
    message: String,
}

impl fmt::Display for ValidationWorkerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "ValidationWorkerError: {}", self.message)
    }
}

impl std::error::Error for ValidationWorkerError {}

/// What a finished check leaves to decide about its share.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckOutcome {
    /// Valid, invalid, or never decidable on this node: checking it again
    /// gives the same.
    Settled,
    /// Undecided for now (`Recoverable`, a store failure, the block not
    /// readable): a later check may decide it.
    Unsettled,
}

/// A share the worker has received and not finished checking.
enum Check {
    /// Waiting for a permit, with the block when its event carried it.
    Queued(Option<Box<ShareBlock>>),
    /// Being checked; `delivered_again` notes a delivery since it started.
    Running { delivered_again: bool },
}

/// The shares the worker has received and not finished checking, each once,
/// the queued ones in the order they first arrived.
///
/// Owned by the worker's loop alone; checks report their end through the
/// loop's channel rather than touching it, so it needs no lock.
struct CheckQueue {
    order: VecDeque<BlockHash>,
    checks: HashMap<BlockHash, Check>,
}

impl CheckQueue {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            order: VecDeque::with_capacity(capacity),
            checks: HashMap::with_capacity(capacity),
        }
    }

    /// How many shares wait for a permit.
    fn queued_len(&self) -> usize {
        self.order.len()
    }

    /// Take in a delivery of `block_hash`. A share not queued or running is
    /// queued last. A queued one keeps its place, taking the block if its
    /// first event lacked it. A running one is noted as delivered again.
    fn receive(&mut self, block_hash: BlockHash, block: Option<ShareBlock>) {
        match self.checks.get_mut(&block_hash) {
            None => {
                self.checks
                    .insert(block_hash, Check::Queued(block.map(Box::new)));
                self.order.push_back(block_hash);
            }
            Some(Check::Queued(queued_block)) => {
                debug!("Share block {block_hash} is already queued for validation");
                if queued_block.is_none() {
                    *queued_block = block.map(Box::new);
                }
            }
            Some(Check::Running { delivered_again }) => {
                debug!("Share block {block_hash} is being validated, noting the delivery");
                *delivered_again = true;
            }
        }
    }

    /// The first queued share, now running, with its block if its event
    /// carried it.
    fn start_next(&mut self) -> Option<(BlockHash, Option<ShareBlock>)> {
        let block_hash = self.order.pop_front()?;
        let block = match self.checks.insert(
            block_hash,
            Check::Running {
                delivered_again: false,
            },
        ) {
            Some(Check::Queued(block)) => block.map(|block| *block),
            _ => None,
        };
        Some((block_hash, block))
    }

    /// A check of `block_hash` ended. A share delivered again while its check
    /// ran is queued once more when that check left it unsettled.
    fn finish(&mut self, block_hash: BlockHash, outcome: CheckOutcome) {
        let delivered_again = matches!(
            self.checks.remove(&block_hash),
            Some(Check::Running {
                delivered_again: true
            })
        );
        if delivered_again && outcome == CheckOutcome::Unsettled {
            debug!(
                "Share block {block_hash} was delivered during an unsettled check, queuing it again"
            );
            self.receive(block_hash, None);
        }
    }
}

/// Reports a check's end to the worker's loop, and when dropped unreported,
/// as by a check that panicked, reports it unsettled, so its share is never
/// left marked running. A send fails only once the loop has stopped, when
/// nobody needs to know.
struct FinishedNotice {
    block_hash: BlockHash,
    finished_tx: Option<mpsc::UnboundedSender<(BlockHash, CheckOutcome)>>,
}

impl FinishedNotice {
    /// Report that the check ended with `outcome`.
    fn report(mut self, outcome: CheckOutcome) {
        self.send(outcome);
    }

    fn send(&mut self, outcome: CheckOutcome) {
        if let Some(finished_tx) = self.finished_tx.take() {
            let _ = finished_tx.send((self.block_hash, outcome));
        }
    }
}

impl Drop for FinishedNotice {
    fn drop(&mut self) {
        self.send(CheckOutcome::Unsettled);
    }
}

/// Worker that validates share blocks in capped concurrent tasks.
///
/// Receives `ValidationEvent` values containing block hashes, reads
/// the share block from the chain store, validates it, and on success
/// emits `OrganiseEvent::Block` for confirmed promotion and
/// `SwarmSend::BroadcastBlock` for block relay to peers.
///
/// After successful validation, the organise worker handles PPLNS
/// updates and dependent scheduling.
pub struct ValidationWorker {
    validation_rx: ValidationReceiver,
    chain_store_handle: ChainStoreHandle,
    organise_tx: OrganiseSender,
    swarm_tx: mpsc::Sender<SwarmSend<ResponseChannel<Message>>>,
    semaphore: Arc<Semaphore>,
    pool_difficulty: PoolDifficulty,
    chain: Chain,
    /// Easiest share target the validator accepts. Defaults to the main ceiling.
    target_ceiling: CompactTarget,
    /// The Bitcoin Cash network whose ASERT checks token shares' headers.
    bitcoin_cash_network: Option<BitcoinCashNetwork>,
    /// The validator to check with; `None` builds the default one from the
    /// settings above when the worker runs. Tests put a mock here.
    share_validator: Option<Arc<dyn ShareValidator + Send + Sync>>,
}

impl ValidationWorker {
    /// Creates a new validation worker.
    pub fn new(
        validation_rx: ValidationReceiver,
        chain_store_handle: ChainStoreHandle,
        organise_tx: OrganiseSender,
        swarm_tx: mpsc::Sender<SwarmSend<ResponseChannel<Message>>>,
        pool_difficulty: PoolDifficulty,
    ) -> Self {
        Self {
            validation_rx,
            chain_store_handle,
            organise_tx,
            swarm_tx,
            semaphore: Arc::new(Semaphore::new(available_cpus())),
            pool_difficulty,
            chain: Chain::Bitcoin,
            target_ceiling: CompactTarget::from_consensus(
                crate::shares::share_block::MAX_POOL_TARGET,
            ),
            bitcoin_cash_network: None,
            share_validator: None,
        }
    }

    /// Check with `share_validator` instead of the default validator.
    #[cfg(test)]
    fn with_share_validator(
        mut self,
        share_validator: Arc<dyn ShareValidator + Send + Sync>,
    ) -> Self {
        self.share_validator = Some(share_validator);
        self
    }

    /// Select the Bitcoin Cash network whose ASERT parameters check a token
    /// share's bitcoin header (`DefaultShareValidator::with_bitcoin_cash_network`).
    pub fn with_bitcoin_cash_network(mut self, network: Option<BitcoinCashNetwork>) -> Self {
        self.bitcoin_cash_network = network;
        self
    }

    /// Select the chain whose share owner this worker's validator accepts.
    pub fn for_chain(mut self, chain: Chain) -> Self {
        self.chain = chain;
        self
    }

    /// Set the easiest share target this worker's validator accepts.
    pub fn with_target_ceiling(mut self, target_ceiling: CompactTarget) -> Self {
        self.target_ceiling = target_ceiling;
        self
    }

    /// Runs the validation worker until the channel closes and every share
    /// it received has started its check.
    ///
    /// Returns `Ok(())` on clean shutdown (channel closed).
    /// Returns `Err(ValidationWorkerError)` on fatal failure.
    pub async fn run(self) -> Result<(), ValidationWorkerError> {
        info!("Validation worker started");
        let ValidationWorker {
            mut validation_rx,
            chain_store_handle,
            organise_tx,
            swarm_tx,
            semaphore,
            pool_difficulty,
            chain,
            target_ceiling,
            bitcoin_cash_network,
            share_validator,
        } = self;
        let depths = ChainDepths::for_chain(chain);

        let share_validator: Arc<dyn ShareValidator + Send + Sync> = share_validator
            .unwrap_or_else(|| {
                Arc::new(
                    DefaultShareValidator::new(pool_difficulty)
                        .for_chain(chain)
                        .with_target_ceiling(target_ceiling)
                        .with_bitcoin_cash_network(bitcoin_cash_network),
                )
            });

        let (finished_tx, mut finished_rx) = mpsc::unbounded_channel();
        let spawn_check =
            |permit: OwnedSemaphorePermit, block_hash: BlockHash, prefetched_block| {
                let chain_store_handle = chain_store_handle.clone();
                let organise_tx = organise_tx.clone();
                let swarm_tx = swarm_tx.clone();
                let validator = Arc::clone(&share_validator);
                let notice = FinishedNotice {
                    block_hash,
                    finished_tx: Some(finished_tx.clone()),
                };
                tokio::spawn(async move {
                    let _permit = permit;
                    let outcome = validate_and_emit(
                        block_hash,
                        prefetched_block,
                        validator,
                        chain_store_handle,
                        organise_tx,
                        swarm_tx,
                        depths,
                    )
                    .await;
                    notice.report(outcome);
                });
            };

        //* Apply finished checks first, so a delivery after a check ended is
        //* a new one; then start queued checks as permits free; then receive.
        let mut queue = CheckQueue::with_capacity(available_cpus());
        let mut receiving = true;
        while receiving || queue.queued_len() > 0 {
            let has_queued = queue.queued_len() > 0;
            let can_receive = receiving && queue.queued_len() < VALIDATION_CHANNEL_CAPACITY;
            tokio::select! {
                biased;
                Some((block_hash, outcome)) = finished_rx.recv() => queue.finish(block_hash, outcome),
                permit = Arc::clone(&semaphore).acquire_owned(), if has_queued => {
                    let permit = permit.map_err(|_| {
                        error!("Validation semaphore closed");
                        ValidationWorkerError {
                            message: "Validation semaphore closed".to_string(),
                        }
                    })?;
                    if let Some((block_hash, prefetched_block)) = queue.start_next() {
                        spawn_check(permit, block_hash, prefetched_block);
                    }
                }
                event = validation_rx.recv(), if can_receive => match event {
                    Some(ValidationEvent::ValidateBlockHash(block_hash)) => {
                        queue.receive(block_hash, None);
                    }
                    Some(ValidationEvent::ValidateShareBlock(share_block)) => {
                        queue.receive(share_block.block_hash(), Some(share_block));
                    }
                    None => receiving = false,
                },
            }
        }
        info!("Validation worker stopped - channel closed");
        Ok(())
    }
}

/// Validate a share block and emit events on success.
///
/// When `prefetched_block` is `Some`, uses the provided block directly
/// (avoids a redundant store read for locally mined shares). Otherwise
/// reads the block from the chain store by hash.
///
/// Sends `OrganiseEvent::Block` for chain promotion. Locally mined
/// blocks (prefetched) are always broadcast to peers. Blocks received
/// from peers are only broadcast when the chain is current,
/// suppressing relay of historic blocks during initial sync or catchup.
///
/// Duplicate validation of already-confirmed blocks is handled by
/// `validate_share_block` which returns Ok immediately for blocks
/// with `BlockValid` status.
///
/// Returns whether the check settled the share: `Unsettled` when the block
/// could not be read, the zone not decided, or the check failed
/// `Recoverable` or on a store read, so a delivery of the share during the
/// check queues it again ([`CheckQueue::finish`]).
async fn validate_and_emit(
    block_hash: BlockHash,
    prefetched_block: Option<ShareBlock>,
    share_validator: Arc<dyn ShareValidator + Send + Sync>,
    chain_store_handle: ChainStoreHandle,
    organise_tx: OrganiseSender,
    swarm_tx: mpsc::Sender<SwarmSend<ResponseChannel<Message>>>,
    depths: ChainDepths,
) -> CheckOutcome {
    let locally_mined = prefetched_block.is_some();
    let share_block = match prefetched_block {
        Some(block) => block,
        None => match chain_store_handle.get_share(&block_hash) {
            Some(share_block) => share_block,
            None => {
                error!("Share block {block_hash} not found in store for validation");
                return CheckOutcome::Unsettled;
            }
        },
    };

    let in_pplns_zone = match check_pplns_zone(&block_hash, &chain_store_handle, depths) {
        Ok(result) => result,
        Err(error_message) => {
            error!("Error checking for pplns zone: {error_message}");
            return CheckOutcome::Unsettled;
        }
    };

    let validation_result = if in_pplns_zone {
        share_validator.validate_share_block(&share_block, &chain_store_handle)
    } else {
        share_validator.validate_below_pplns_depth(&share_block, &chain_store_handle)
    };

    if let Err(validation_error) = validation_result {
        match validation_error.kind() {
            FailureKind::Consensus => {
                // Every dependency this check needed was present, so the block
                // is genuinely invalid. Report it: left alone it keeps its
                // HeaderValid status on the candidate chain and blocks
                // confirmation at that height.
                error!("Share block {block_hash} validation failed: {validation_error}");
                if let Err(send_error) = organise_tx
                    .send(OrganiseEvent::InvalidBlock(block_hash))
                    .await
                {
                    error!(
                        "Failed to report invalid block {block_hash} to organise worker: {send_error}"
                    );
                }
            }
            FailureKind::Recoverable => {
                // A dependency (a deeper ancestor's header, an uncle's height)
                // has not arrived yet, so the block may still be valid and must
                // not be marked Invalid. It stays HeaderValid until a peer
                // sends it again, which handle_share_block re-queues for
                // validation; this node does not solicit that re-delivery.
                warn!("Share block {block_hash} cannot be validated yet: {validation_error}");
            }
            FailureKind::StoreAccess => {
                error!("Store error validating share block {block_hash}: {validation_error}");
            }
            FailureKind::Unresolvable => {
                // Data the check needs is gone rather than late, so a retry
                // cannot decide it. The block keeps its HeaderValid status and
                // no verdict is recorded: another node may well judge it valid.
                warn!("Share block {block_hash} cannot be resolved: {validation_error}");
            }
        }
        return match validation_error.kind() {
            FailureKind::Recoverable | FailureKind::StoreAccess => CheckOutcome::Unsettled,
            FailureKind::Consensus | FailureKind::Unresolvable => CheckOutcome::Settled,
        };
    }

    debug!("Share block {block_hash} validated successfully");

    if let Err(send_error) = organise_tx
        .send(OrganiseEvent::Block {
            share_block: share_block.clone(),
            // Which tier ran here, so the organise worker does not re-derive it
            // from a candidate tip that may have shortened in between.
            content_validated: in_pplns_zone,
        })
        .await
    {
        error!("Failed to send validated block to organise worker: {send_error}");
    }

    // Always broadcast locally mined blocks. For blocks received from
    // peers, only broadcast when the chain is current to avoid relaying
    // historic blocks during initial sync or catchup.
    if locally_mined || chain_store_handle.is_current() {
        #[cfg(not(feature = "sim"))]
        {
            let bch_headers =
                chain_store_handle.carried_bch_headers(std::slice::from_ref(&share_block.header));
            if let Err(send_error) = swarm_tx
                .send(SwarmSend::BroadcastBlock(share_block, bch_headers))
                .await
            {
                error!("Failed to send broadcast for validated block {block_hash}: {send_error}");
            }
        }
        #[cfg(feature = "sim")]
        {
            crate::sim_overrides::spawn_delayed_broadcast(swarm_tx, share_block);
        }
    } else {
        debug!("Skipping broadcast for {block_hash} - chain not current");
    }
    CheckOutcome::Settled
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::organise_worker;
    use crate::shares::chain::chain_store_handle::MockChainStoreHandle;
    use crate::shares::validation::{MockDefaultShareValidator, ValidationError};
    use crate::store::block_tx_metadata::{BlockMetadata, ChainMembership, Status};
    use crate::test_utils::TestShareBlockBuilder;

    use bitcoin::Work;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    /// Build a BlockMetadata value that places the block inside the PPLNS zone.
    ///
    /// Uses height 1 so that `is_in_pplns_zone(1, candidate_tip=1)` returns true,
    /// causing `validate_share_block` (not `validate_below_pplns_depth`) to run.
    fn pplns_zone_metadata() -> BlockMetadata {
        BlockMetadata {
            expected_height: Some(1),
            chain_work: Work::from_be_bytes([0u8; 32]),
            status: Status::HeaderValid,
            chain: ChainMembership::Candidate,
        }
    }

    #[tokio::test]
    async fn test_validation_worker_validates_and_sends_organise_event() {
        let (validation_tx, validation_rx) = create_validation_channel();
        let mut mock_chain_handle = MockChainStoreHandle::new();
        mock_chain_handle
            .expect_carried_bch_headers()
            .returning(|_| Vec::new());

        let share_block = TestShareBlockBuilder::new().build();
        let block_hash = share_block.block_hash();
        let share_block_clone = share_block.clone();

        // Mock clone for spawned task
        let mut mock_clone = MockChainStoreHandle::new();
        mock_clone
            .expect_carried_bch_headers()
            .returning(|_| Vec::new());
        mock_clone
            .expect_get_share()
            .returning(move |_| Some(share_block_clone.clone()));
        mock_clone
            .expect_get_block_metadata()
            .returning(|_| Ok(pplns_zone_metadata()));
        mock_clone
            .expect_get_candidate_tip_height()
            .returning(|| Ok(Some(1)));
        mock_clone.expect_has_status().returning(|_, _| true);
        mock_clone.expect_is_current().returning(|| true);
        mock_chain_handle
            .expect_clone()
            .return_once(move || mock_clone);

        let (organise_tx, mut organise_rx) = organise_worker::create_organise_channel();
        let (swarm_tx, _swarm_rx) = mpsc::channel(32);

        let worker = ValidationWorker::new(
            validation_rx,
            mock_chain_handle,
            organise_tx,
            swarm_tx,
            PoolDifficulty::default(),
        );

        let worker_handle = tokio::spawn(worker.run());

        validation_tx
            .send(ValidationEvent::ValidateBlockHash(block_hash))
            .await
            .unwrap();

        let organise_event = tokio::time::timeout(Duration::from_secs(2), organise_rx.recv()).await;
        if let Ok(Some(OrganiseEvent::Block {
            share_block: received_block,
            ..
        })) = organise_event
        {
            assert_eq!(received_block.block_hash(), block_hash);
        } else {
            panic!("Expected OrganiseEvent::Block after successful validation");
        }

        worker_handle.abort();
    }

    #[tokio::test]
    async fn test_validation_worker_sends_broadcast_on_success() {
        let (validation_tx, validation_rx) = create_validation_channel();
        let mut mock_chain_handle = MockChainStoreHandle::new();
        mock_chain_handle
            .expect_carried_bch_headers()
            .returning(|_| Vec::new());

        let share_block = TestShareBlockBuilder::new().build();
        let block_hash = share_block.block_hash();
        let share_block_clone = share_block.clone();

        let mut mock_clone = MockChainStoreHandle::new();
        mock_clone
            .expect_carried_bch_headers()
            .returning(|_| Vec::new());
        mock_clone
            .expect_get_share()
            .returning(move |_| Some(share_block_clone.clone()));
        mock_clone
            .expect_get_block_metadata()
            .returning(|_| Ok(pplns_zone_metadata()));
        mock_clone
            .expect_get_candidate_tip_height()
            .returning(|| Ok(Some(1)));
        mock_clone.expect_has_status().returning(|_, _| true);
        mock_clone.expect_is_current().returning(|| true);
        mock_chain_handle
            .expect_clone()
            .return_once(move || mock_clone);

        let (organise_tx, _organise_rx) = organise_worker::create_organise_channel();
        let (swarm_tx, mut swarm_rx) = mpsc::channel(32);

        let worker = ValidationWorker::new(
            validation_rx,
            mock_chain_handle,
            organise_tx,
            swarm_tx,
            PoolDifficulty::default(),
        );

        let worker_handle = tokio::spawn(worker.run());

        validation_tx
            .send(ValidationEvent::ValidateBlockHash(block_hash))
            .await
            .unwrap();

        let swarm_event = tokio::time::timeout(Duration::from_secs(2), swarm_rx.recv()).await;
        if let Ok(Some(SwarmSend::BroadcastBlock(broadcast_block, _))) = swarm_event {
            assert_eq!(broadcast_block.block_hash(), block_hash);
        } else {
            panic!("Expected SwarmSend::BroadcastBlock after successful validation");
        }

        worker_handle.abort();
    }

    #[tokio::test]
    async fn test_validation_worker_skips_broadcast_when_not_current() {
        let (validation_tx, validation_rx) = create_validation_channel();
        let mut mock_chain_handle = MockChainStoreHandle::new();
        mock_chain_handle
            .expect_carried_bch_headers()
            .returning(|_| Vec::new());

        let share_block = TestShareBlockBuilder::new().build();
        let block_hash = share_block.block_hash();
        let share_block_clone = share_block.clone();

        let mut mock_clone = MockChainStoreHandle::new();
        mock_clone
            .expect_carried_bch_headers()
            .returning(|_| Vec::new());
        mock_clone
            .expect_get_share()
            .returning(move |_| Some(share_block_clone.clone()));
        mock_clone
            .expect_get_block_metadata()
            .returning(|_| Ok(pplns_zone_metadata()));
        mock_clone
            .expect_get_candidate_tip_height()
            .returning(|| Ok(Some(1)));
        mock_clone.expect_has_status().returning(|_, _| true);
        mock_clone.expect_is_current().returning(|| false);
        mock_chain_handle
            .expect_clone()
            .return_once(move || mock_clone);

        let (organise_tx, mut organise_rx) = organise_worker::create_organise_channel();
        let (swarm_tx, mut swarm_rx) = mpsc::channel(32);

        let worker = ValidationWorker::new(
            validation_rx,
            mock_chain_handle,
            organise_tx,
            swarm_tx,
            PoolDifficulty::default(),
        );

        let worker_handle = tokio::spawn(worker.run());

        validation_tx
            .send(ValidationEvent::ValidateBlockHash(block_hash))
            .await
            .unwrap();

        // Organisation event should still be sent
        let organise_event = tokio::time::timeout(Duration::from_secs(2), organise_rx.recv()).await;
        if let Ok(Some(OrganiseEvent::Block {
            share_block: received_block,
            ..
        })) = organise_event
        {
            assert_eq!(received_block.block_hash(), block_hash);
        } else {
            panic!("Expected OrganiseEvent::Block even when not current");
        }

        // Broadcast should NOT be sent during sync
        let swarm_result = tokio::time::timeout(Duration::from_millis(500), swarm_rx.recv()).await;
        assert!(
            swarm_result.is_err(),
            "No SwarmSend expected when chain is not current"
        );

        worker_handle.abort();
    }

    #[tokio::test]
    async fn test_locally_mined_block_broadcasts_when_not_current() {
        let (validation_tx, validation_rx) = create_validation_channel();
        let mut mock_chain_handle = MockChainStoreHandle::new();
        mock_chain_handle
            .expect_carried_bch_headers()
            .returning(|_| Vec::new());

        let share_block = TestShareBlockBuilder::new().build();
        let block_hash = share_block.block_hash();

        let mut mock_clone = MockChainStoreHandle::new();
        mock_clone
            .expect_carried_bch_headers()
            .returning(|_| Vec::new());
        mock_clone.expect_get_share().never();
        mock_clone
            .expect_get_block_metadata()
            .returning(|_| Ok(pplns_zone_metadata()));
        mock_clone
            .expect_get_candidate_tip_height()
            .returning(|| Ok(Some(1)));
        mock_clone.expect_has_status().returning(|_, _| true);
        mock_clone.expect_is_current().returning(|| false);
        mock_chain_handle
            .expect_clone()
            .return_once(move || mock_clone);

        let (organise_tx, _organise_rx) = organise_worker::create_organise_channel();
        let (swarm_tx, mut swarm_rx) = mpsc::channel(32);

        let worker = ValidationWorker::new(
            validation_rx,
            mock_chain_handle,
            organise_tx,
            swarm_tx,
            PoolDifficulty::default(),
        );

        let worker_handle = tokio::spawn(worker.run());

        validation_tx
            .send(ValidationEvent::ValidateShareBlock(share_block))
            .await
            .unwrap();

        let swarm_event = tokio::time::timeout(Duration::from_secs(2), swarm_rx.recv()).await;
        if let Ok(Some(SwarmSend::BroadcastBlock(broadcast_block, _))) = swarm_event {
            assert_eq!(broadcast_block.block_hash(), block_hash);
        } else {
            panic!(
                "Expected SwarmSend::BroadcastBlock for locally mined block even when not current"
            );
        }

        worker_handle.abort();
    }

    #[tokio::test]
    async fn test_validation_worker_skips_missing_block() {
        let (validation_tx, validation_rx) = create_validation_channel();
        let mut mock_chain_handle = MockChainStoreHandle::new();
        mock_chain_handle
            .expect_carried_bch_headers()
            .returning(|_| Vec::new());

        let block_hash = TestShareBlockBuilder::new().build().block_hash();

        let mut mock_clone = MockChainStoreHandle::new();
        mock_clone
            .expect_carried_bch_headers()
            .returning(|_| Vec::new());
        mock_clone.expect_get_share().returning(|_| None);
        mock_chain_handle
            .expect_clone()
            .return_once(move || mock_clone);

        let (organise_tx, mut organise_rx) = organise_worker::create_organise_channel();
        let (swarm_tx, mut swarm_rx) = mpsc::channel(32);

        let worker = ValidationWorker::new(
            validation_rx,
            mock_chain_handle,
            organise_tx,
            swarm_tx,
            PoolDifficulty::default(),
        );

        let worker_handle = tokio::spawn(worker.run());

        validation_tx
            .send(ValidationEvent::ValidateBlockHash(block_hash))
            .await
            .unwrap();

        // Block not found in store -- no events should be produced.
        let organise_result =
            tokio::time::timeout(Duration::from_millis(500), organise_rx.recv()).await;
        assert!(
            organise_result.is_err(),
            "No OrganiseEvent expected for missing block"
        );

        let swarm_result = tokio::time::timeout(Duration::from_millis(500), swarm_rx.recv()).await;
        assert!(
            swarm_result.is_err(),
            "No SwarmSend expected for missing block"
        );

        worker_handle.abort();
    }

    /// Build a validator whose `validate_share_block` fails with `error`.
    fn validator_failing_with(error: ValidationError) -> Arc<dyn ShareValidator + Send + Sync> {
        let mut mock_validator = MockDefaultShareValidator::new();
        let mut error_slot = Some(error);
        mock_validator
            .expect_validate_share_block()
            .returning(move |_, _| Err(error_slot.take().expect("validated more than once")));
        Arc::new(mock_validator)
    }

    /// A Consensus failure means every dependency was present and the block
    /// broke a rule, so the organise worker must be told to mark it Invalid.
    /// Left alone the block keeps its HeaderValid status on the candidate chain
    /// and confirmation stalls at that height.
    #[tokio::test]
    async fn test_consensus_failure_reports_invalid_block_to_organise_worker() {
        let share_block = TestShareBlockBuilder::new().build();
        let block_hash = share_block.block_hash();

        let mut mock_chain_handle = MockChainStoreHandle::new();
        mock_chain_handle
            .expect_carried_bch_headers()
            .returning(|_| Vec::new());
        mock_chain_handle
            .expect_get_block_metadata()
            .returning(|_| Ok(pplns_zone_metadata()));
        mock_chain_handle
            .expect_get_candidate_tip_height()
            .returning(|| Ok(Some(1)));

        let (organise_tx, mut organise_rx) = organise_worker::create_organise_channel();
        let (swarm_tx, mut swarm_rx) = mpsc::channel(32);

        validate_and_emit(
            block_hash,
            Some(share_block),
            validator_failing_with(ValidationError::consensus("share has duplicate uncles")),
            mock_chain_handle,
            organise_tx,
            swarm_tx,
            ChainDepths::BITCOIN,
        )
        .await;

        // validate_and_emit has returned and dropped both senders, so recv()
        // resolves immediately: Some(event) if one was sent, None if not.
        match organise_rx.recv().await {
            Some(OrganiseEvent::InvalidBlock(reported)) => assert_eq!(reported, block_hash),
            _ => panic!("Expected OrganiseEvent::InvalidBlock for {block_hash}"),
        }
        assert!(
            swarm_rx.recv().await.is_none(),
            "An invalid block must not be broadcast"
        );
    }

    /// A Recoverable failure means a dependency has not arrived yet. The block
    /// is re-delivered once it lands, so the verdict stays open: nothing is
    /// reported to the organise worker and nothing is broadcast.
    #[tokio::test]
    async fn test_recoverable_failure_leaves_block_for_retry() {
        let share_block = TestShareBlockBuilder::new().build();
        let block_hash = share_block.block_hash();

        let mut mock_chain_handle = MockChainStoreHandle::new();
        mock_chain_handle
            .expect_carried_bch_headers()
            .returning(|_| Vec::new());
        mock_chain_handle
            .expect_get_block_metadata()
            .returning(|_| Ok(pplns_zone_metadata()));
        mock_chain_handle
            .expect_get_candidate_tip_height()
            .returning(|| Ok(Some(1)));

        let (organise_tx, mut organise_rx) = organise_worker::create_organise_channel();
        let (swarm_tx, mut swarm_rx) = mpsc::channel(32);

        validate_and_emit(
            block_hash,
            Some(share_block),
            validator_failing_with(ValidationError::recoverable("uncle not found in store")),
            mock_chain_handle,
            organise_tx,
            swarm_tx,
            ChainDepths::BITCOIN,
        )
        .await;

        assert!(
            organise_rx.recv().await.is_none(),
            "No OrganiseEvent expected while a dependency is still missing"
        );
        assert!(swarm_rx.recv().await.is_none(), "No SwarmSend expected");
    }

    #[tokio::test]
    async fn test_validate_share_block_skips_store_read() {
        let (validation_tx, validation_rx) = create_validation_channel();
        let mut mock_chain_handle = MockChainStoreHandle::new();
        mock_chain_handle
            .expect_carried_bch_headers()
            .returning(|_| Vec::new());

        let share_block = TestShareBlockBuilder::new().build();
        let block_hash = share_block.block_hash();

        let mut mock_clone = MockChainStoreHandle::new();
        mock_clone
            .expect_carried_bch_headers()
            .returning(|_| Vec::new());
        mock_clone.expect_get_share().never();
        mock_clone
            .expect_get_block_metadata()
            .returning(|_| Ok(pplns_zone_metadata()));
        mock_clone
            .expect_get_candidate_tip_height()
            .returning(|| Ok(Some(1)));
        mock_clone.expect_has_status().returning(|_, _| true);
        mock_clone.expect_is_current().returning(|| true);
        mock_chain_handle
            .expect_clone()
            .return_once(move || mock_clone);

        let (organise_tx, mut organise_rx) = organise_worker::create_organise_channel();
        let (swarm_tx, mut swarm_rx) = mpsc::channel(32);

        let worker = ValidationWorker::new(
            validation_rx,
            mock_chain_handle,
            organise_tx,
            swarm_tx,
            PoolDifficulty::default(),
        );

        let worker_handle = tokio::spawn(worker.run());

        validation_tx
            .send(ValidationEvent::ValidateShareBlock(share_block))
            .await
            .unwrap();

        let organise_event = tokio::time::timeout(Duration::from_secs(2), organise_rx.recv()).await;
        if let Ok(Some(OrganiseEvent::Block {
            share_block: received_block,
            ..
        })) = organise_event
        {
            assert_eq!(received_block.block_hash(), block_hash);
        } else {
            panic!("Expected OrganiseEvent::Block for prefetched share block");
        }

        let swarm_event = tokio::time::timeout(Duration::from_secs(2), swarm_rx.recv()).await;
        if let Ok(Some(SwarmSend::BroadcastBlock(broadcast_block, _))) = swarm_event {
            assert_eq!(broadcast_block.block_hash(), block_hash);
        } else {
            panic!("Expected SwarmSend::BroadcastBlock for prefetched share block");
        }

        worker_handle.abort();
    }

    /// Prune-zone block (height 100, candidate tip 300000) is validated
    /// via the below-PPLNS path. The BlockValid shortcut fires (same as
    /// PPLNS-zone tests) because test fixture blocks don't carry valid
    /// PoW against pool difficulty. The test verifies the block reaches
    /// the organise worker, confirming the below-PPLNS path returns Ok.
    #[tokio::test]
    async fn test_validation_worker_successfully_validates_blocks_in_prune_zone() {
        let (validation_tx, validation_rx) = create_validation_channel();
        let mut mock_chain_handle = MockChainStoreHandle::new();
        mock_chain_handle
            .expect_carried_bch_headers()
            .returning(|_| Vec::new());

        let share_block = TestShareBlockBuilder::new().build();
        let block_hash = share_block.block_hash();
        let share_block_clone = share_block.clone();

        let mut mock_clone = MockChainStoreHandle::new();
        mock_clone
            .expect_carried_bch_headers()
            .returning(|_| Vec::new());
        mock_clone
            .expect_get_share()
            .returning(move |_| Some(share_block_clone.clone()));
        mock_clone.expect_get_block_metadata().returning(|_| {
            Ok(BlockMetadata {
                expected_height: Some(100),
                chain_work: Work::from_be_bytes([0u8; 32]),
                status: Status::HeaderValid,
                chain: ChainMembership::Candidate,
            })
        });
        mock_clone
            .expect_get_candidate_tip_height()
            .returning(|| Ok(Some(300_000)));
        // BlockValid shortcut fires in validate_below_pplns_depth
        mock_clone.expect_has_status().returning(|_, _| true);
        mock_clone.expect_is_current().returning(|| false);
        mock_chain_handle
            .expect_clone()
            .return_once(move || mock_clone);

        let (organise_tx, mut organise_rx) = organise_worker::create_organise_channel();
        let (swarm_tx, _swarm_rx) = mpsc::channel(32);

        let worker = ValidationWorker::new(
            validation_rx,
            mock_chain_handle,
            organise_tx,
            swarm_tx,
            PoolDifficulty::default(),
        );

        let worker_handle = tokio::spawn(worker.run());

        validation_tx
            .send(ValidationEvent::ValidateBlockHash(block_hash))
            .await
            .unwrap();

        let organise_event = tokio::time::timeout(Duration::from_secs(2), organise_rx.recv()).await;
        if let Ok(Some(OrganiseEvent::Block {
            share_block: received_block,
            ..
        })) = organise_event
        {
            assert_eq!(received_block.block_hash(), block_hash);
        } else {
            panic!("Expected OrganiseEvent::Block for prune-zone block");
        }

        worker_handle.abort();
    }

    /// A share delivered twice more while its first check runs, as peers
    /// relaying it do, is checked once: the later deliveries queue nothing.
    #[tokio::test(flavor = "multi_thread", worker_threads = 3)]
    async fn test_deliveries_during_a_check_queue_nothing() {
        let share_block = TestShareBlockBuilder::new().build();
        let block_hash = share_block.block_hash();
        let checks = Arc::new(AtomicUsize::new(0));
        let (started_tx, mut started_rx) = mpsc::unbounded_channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let mut mock_validator = MockDefaultShareValidator::new();
        let counted = Arc::clone(&checks);
        mock_validator
            .expect_validate_share_block()
            .returning(move |_, _| {
                counted.fetch_add(1, Ordering::SeqCst);
                let _ = started_tx.send(());
                let _ = release_rx.recv();
                Ok(())
            });
        let mut mock_chain_handle = MockChainStoreHandle::new();
        mock_chain_handle.expect_clone().returning(move || {
            let mut mock_clone = MockChainStoreHandle::new();
            let stored_block = share_block.clone();
            mock_clone
                .expect_get_share()
                .returning(move |_| Some(stored_block.clone()));
            mock_clone
                .expect_get_block_metadata()
                .returning(|_| Ok(pplns_zone_metadata()));
            mock_clone
                .expect_get_candidate_tip_height()
                .returning(|| Ok(Some(1)));
            mock_clone.expect_is_current().returning(|| false);
            mock_clone
        });
        let (validation_tx, validation_rx) = create_validation_channel();
        let (organise_tx, mut organise_rx) = organise_worker::create_organise_channel();
        let (swarm_tx, _swarm_rx) = mpsc::channel(32);
        let worker = ValidationWorker::new(
            validation_rx,
            mock_chain_handle,
            organise_tx,
            swarm_tx,
            PoolDifficulty::default(),
        )
        .with_share_validator(Arc::new(mock_validator));
        let worker_handle = tokio::spawn(worker.run());

        validation_tx
            .send(ValidationEvent::ValidateBlockHash(block_hash))
            .await
            .unwrap();
        started_rx.recv().await.expect("the first check starts");
        validation_tx
            .send(ValidationEvent::ValidateBlockHash(block_hash))
            .await
            .unwrap();
        validation_tx
            .send(ValidationEvent::ValidateBlockHash(block_hash))
            .await
            .unwrap();
        // Let the worker take both deliveries in while the check still runs.
        tokio::time::sleep(Duration::from_millis(300)).await;
        drop(release_tx);

        let organise_event = tokio::time::timeout(Duration::from_secs(2), organise_rx.recv()).await;
        assert!(
            matches!(organise_event, Ok(Some(OrganiseEvent::Block { .. }))),
            "the check reports the share valid"
        );
        let another = tokio::time::timeout(Duration::from_millis(500), organise_rx.recv()).await;
        assert!(another.is_err(), "the deliveries queue no second check");
        assert_eq!(checks.load(Ordering::SeqCst), 1);

        worker_handle.abort();
    }

    /// A share whose check fails `Recoverable` is checked again: at the end
    /// of that check when it was delivered meanwhile, and on its next
    /// delivery otherwise, as `handle_share_block`'s re-send intends.
    #[tokio::test(flavor = "multi_thread", worker_threads = 3)]
    async fn test_a_recoverable_share_is_checked_again() {
        let share_block = TestShareBlockBuilder::new().build();
        let block_hash = share_block.block_hash();
        let checks = Arc::new(AtomicUsize::new(0));
        let (call_tx, mut call_rx) = mpsc::unbounded_channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let mut mock_validator = MockDefaultShareValidator::new();
        let counted = Arc::clone(&checks);
        mock_validator
            .expect_validate_share_block()
            .returning(move |_, _| {
                let call = counted.fetch_add(1, Ordering::SeqCst) + 1;
                let _ = call_tx.send(call);
                if call == 1 {
                    let _ = release_rx.recv();
                }
                if call <= 2 {
                    Err(ValidationError::recoverable("uncle not found in store"))
                } else {
                    Ok(())
                }
            });
        let mut mock_chain_handle = MockChainStoreHandle::new();
        mock_chain_handle.expect_clone().returning(move || {
            let mut mock_clone = MockChainStoreHandle::new();
            let stored_block = share_block.clone();
            mock_clone
                .expect_get_share()
                .returning(move |_| Some(stored_block.clone()));
            mock_clone
                .expect_get_block_metadata()
                .returning(|_| Ok(pplns_zone_metadata()));
            mock_clone
                .expect_get_candidate_tip_height()
                .returning(|| Ok(Some(1)));
            mock_clone.expect_is_current().returning(|| false);
            mock_clone
        });
        let (validation_tx, validation_rx) = create_validation_channel();
        let (organise_tx, mut organise_rx) = organise_worker::create_organise_channel();
        let (swarm_tx, _swarm_rx) = mpsc::channel(32);
        let worker = ValidationWorker::new(
            validation_rx,
            mock_chain_handle,
            organise_tx,
            swarm_tx,
            PoolDifficulty::default(),
        )
        .with_share_validator(Arc::new(mock_validator));
        let worker_handle = tokio::spawn(worker.run());

        validation_tx
            .send(ValidationEvent::ValidateBlockHash(block_hash))
            .await
            .unwrap();
        assert_eq!(call_rx.recv().await, Some(1));
        validation_tx
            .send(ValidationEvent::ValidateBlockHash(block_hash))
            .await
            .unwrap();
        // Let the worker take the delivery in while the first check runs.
        tokio::time::sleep(Duration::from_millis(300)).await;
        drop(release_tx);

        let second = tokio::time::timeout(Duration::from_secs(2), call_rx.recv()).await;
        assert_eq!(
            second,
            Ok(Some(2)),
            "the delivery during a Recoverable check queues it again"
        );
        let none_yet = tokio::time::timeout(Duration::from_millis(500), call_rx.recv()).await;
        assert!(
            none_yet.is_err(),
            "with no delivery during it, a Recoverable check waits for the next"
        );
        validation_tx
            .send(ValidationEvent::ValidateBlockHash(block_hash))
            .await
            .unwrap();
        let third = tokio::time::timeout(Duration::from_secs(2), call_rx.recv()).await;
        assert_eq!(third, Ok(Some(3)), "the next delivery checks it again");
        let organise_event = tokio::time::timeout(Duration::from_secs(2), organise_rx.recv()).await;
        assert!(
            matches!(organise_event, Ok(Some(OrganiseEvent::Block { .. }))),
            "the third check reports the share valid"
        );

        worker_handle.abort();
    }

    /// A node that stops while a stored share's check runs checks it again
    /// after the restart: the new worker keeps nothing of the old one's
    /// queue, so the share's re-send is checked though the old worker had
    /// taken it in.
    #[tokio::test(flavor = "multi_thread", worker_threads = 3)]
    async fn test_a_restart_checks_a_share_the_stopped_worker_was_checking() {
        let share_block = TestShareBlockBuilder::new().build();
        let block_hash = share_block.block_hash();
        let (started_tx, mut started_rx) = mpsc::unbounded_channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let mut stopped_validator = MockDefaultShareValidator::new();
        stopped_validator
            .expect_validate_share_block()
            .returning(move |_, _| {
                let _ = started_tx.send(());
                let _ = release_rx.recv();
                Err(ValidationError::recoverable("stopped before it ended"))
            });
        let mut restarted_validator = MockDefaultShareValidator::new();
        restarted_validator
            .expect_validate_share_block()
            .times(1)
            .returning(|_, _| Ok(()));
        let stored_block = share_block.clone();
        let mut stopped_chain_handle = MockChainStoreHandle::new();
        stopped_chain_handle.expect_clone().returning(move || {
            let mut mock_clone = MockChainStoreHandle::new();
            let stored_block = stored_block.clone();
            mock_clone
                .expect_get_share()
                .returning(move |_| Some(stored_block.clone()));
            mock_clone
                .expect_get_block_metadata()
                .returning(|_| Ok(pplns_zone_metadata()));
            mock_clone
                .expect_get_candidate_tip_height()
                .returning(|| Ok(Some(1)));
            mock_clone
        });
        let mut restarted_chain_handle = MockChainStoreHandle::new();
        restarted_chain_handle.expect_clone().returning(move || {
            let mut mock_clone = MockChainStoreHandle::new();
            let stored_block = share_block.clone();
            mock_clone
                .expect_get_share()
                .returning(move |_| Some(stored_block.clone()));
            mock_clone
                .expect_get_block_metadata()
                .returning(|_| Ok(pplns_zone_metadata()));
            mock_clone
                .expect_get_candidate_tip_height()
                .returning(|| Ok(Some(1)));
            mock_clone.expect_is_current().returning(|| false);
            mock_clone
        });

        let (stopped_tx, stopped_rx) = create_validation_channel();
        let (stopped_organise_tx, _stopped_organise_rx) =
            organise_worker::create_organise_channel();
        let (stopped_swarm_tx, _stopped_swarm_rx) = mpsc::channel(32);
        let stopped_worker = tokio::spawn(
            ValidationWorker::new(
                stopped_rx,
                stopped_chain_handle,
                stopped_organise_tx,
                stopped_swarm_tx,
                PoolDifficulty::default(),
            )
            .with_share_validator(Arc::new(stopped_validator))
            .run(),
        );
        stopped_tx
            .send(ValidationEvent::ValidateBlockHash(block_hash))
            .await
            .unwrap();
        started_rx
            .recv()
            .await
            .expect("the stopped worker starts its check");
        stopped_worker.abort();

        let (restarted_tx, restarted_rx) = create_validation_channel();
        let (organise_tx, mut organise_rx) = organise_worker::create_organise_channel();
        let (swarm_tx, _swarm_rx) = mpsc::channel(32);
        let restarted_worker = tokio::spawn(
            ValidationWorker::new(
                restarted_rx,
                restarted_chain_handle,
                organise_tx,
                swarm_tx,
                PoolDifficulty::default(),
            )
            .with_share_validator(Arc::new(restarted_validator))
            .run(),
        );
        restarted_tx
            .send(ValidationEvent::ValidateBlockHash(block_hash))
            .await
            .unwrap();

        let organise_event = tokio::time::timeout(Duration::from_secs(2), organise_rx.recv()).await;
        assert!(
            matches!(organise_event, Ok(Some(OrganiseEvent::Block { .. }))),
            "the restarted worker checks the share"
        );

        drop(release_tx);
        restarted_worker.abort();
    }
}
