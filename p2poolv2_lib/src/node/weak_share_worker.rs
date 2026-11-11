// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The worker that checks weak shares before the weak pool keeps and relays
//! them (`shares::weak_pool`).
//!
//! Weak shares reach it from this node's stratum, which found them, and
//! from peers, which relayed them. The pool admits a proof-of-work hash that
//! has not passed before, from a peer within its rate, and the worker then
//! runs every rule but the carrier's (`ShareValidator::validate_weak_share`),
//! with one cache of the window's payouts across weak shares on the same
//! template. A weak share that passes goes into the pool, which remembers
//! its hash and queues it for the node actor to relay; one that fails is
//! not remembered, so a bad copy relayed first cannot keep the real one out.
//! One whose check cannot finish yet is held and checked again once the
//! confirmed tip moves.
//!
//! A weak share whose parent this node does not have is held in the pool,
//! and the parent asked for from the peer that sent it, as the parent of a
//! share block is (`request_headers_for_missing_blocks`). Every
//! [`HELD_RELEASE_INTERVAL`] the worker looks up the parents the held weak
//! shares wait for, and checks the ones whose parent arrived as new.
//!
//! One task, fed by a bounded channel: a burst of weak shares waits here
//! rather than in the actor loop or the request handlers.

#[cfg(test)]
#[mockall_double::double]
use crate::accounting::payout::sharechain_pplns::PplnsWindow;
#[cfg(not(test))]
use crate::accounting::payout::sharechain_pplns::PplnsWindow;
use crate::node::SwarmSend;
use crate::node::p2p_message_handlers::receivers::request_missing_blocks::request_headers_for_missing_blocks;
#[cfg(test)]
#[mockall_double::double]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
#[cfg(not(test))]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
use crate::shares::validation::{FailureKind, ShareValidator, WeakPayoutCache};
use crate::shares::weak::WeakShare;
use crate::shares::weak_pool::{
    HoldOutcome, WeakPool, WeakRelayReceiver, WeakShareRefusal, WeakShareSource, now_millis,
};
use crate::store::writer::StoreError;
use bitcoin::BlockHash;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, info};

/// Weak shares waiting for their checks.
pub const WEAK_SHARE_EVENT_CHANNEL_CAPACITY: usize = 1024;

/// How often the worker logs what the weak pool has done.
const WEAK_POOL_REPORT_INTERVAL: Duration = Duration::from_secs(60);

/// How often the worker looks for the parents of held weak shares.
pub const HELD_RELEASE_INTERVAL: Duration = Duration::from_secs(1);

/// A weak share to check, and where it came from.
#[derive(Debug, Clone)]
pub struct WeakShareEvent {
    pub weak_share: WeakShare,
    pub source: WeakShareSource,
}

pub type WeakShareEventSender = mpsc::Sender<WeakShareEvent>;
pub type WeakShareEventReceiver = mpsc::Receiver<WeakShareEvent>;

/// What a Bitcoin Cash P2Pool node needs for weak shares: the pool, the
/// receiver of the weak shares it accepts, to relay, and the channel into the
/// worker that checks them, whose sender stratum and the peer handlers share.
pub struct WeakShareNetwork {
    pub pool: WeakPool,
    pub relay_rx: WeakRelayReceiver,
    pub events_tx: WeakShareEventSender,
    pub events_rx: WeakShareEventReceiver,
}

impl WeakShareNetwork {
    /// A pool over `pool` and a fresh channel into its worker.
    pub fn new(pool: WeakPool, relay_rx: WeakRelayReceiver) -> Self {
        let (events_tx, events_rx) = mpsc::channel(WEAK_SHARE_EVENT_CHANNEL_CAPACITY);
        Self {
            pool,
            relay_rx,
            events_tx,
            events_rx,
        }
    }
}

/// What the store knows of a weak share's parent.
enum ParentHeight {
    /// The parent is stored at this height.
    Known(u32),
    /// The parent, or its height, is not stored yet.
    Unknown,
    /// The store could not be read.
    Unreadable(StoreError),
}

/// Checks weak shares and hands the ones that pass to the weak pool.
///
/// Generic over the response channel type `C` of `swarm_tx`, as the request
/// handlers are, so tests can stand in for libp2p's opaque channel.
pub struct WeakShareWorker<C> {
    events_rx: WeakShareEventReceiver,
    pool: WeakPool,
    chain_store_handle: ChainStoreHandle,
    share_validator: Arc<dyn ShareValidator + Send + Sync>,
    pplns_window: Arc<RwLock<PplnsWindow>>,
    cache: WeakPayoutCache,
    /// Where the request for a held weak share's parent goes.
    swarm_tx: mpsc::Sender<SwarmSend<C>>,
}

impl<C> WeakShareWorker<C> {
    pub fn new(
        events_rx: WeakShareEventReceiver,
        pool: WeakPool,
        chain_store_handle: ChainStoreHandle,
        share_validator: Arc<dyn ShareValidator + Send + Sync>,
        pplns_window: Arc<RwLock<PplnsWindow>>,
        swarm_tx: mpsc::Sender<SwarmSend<C>>,
    ) -> Self {
        Self {
            events_rx,
            pool,
            chain_store_handle,
            share_validator,
            pplns_window,
            cache: WeakPayoutCache::new(),
            swarm_tx,
        }
    }

    /// Check weak shares until every sender is gone, releasing held weak
    /// shares whose parent arrived every [`HELD_RELEASE_INTERVAL`] and
    /// logging the pool's counts every [`WEAK_POOL_REPORT_INTERVAL`].
    pub async fn run(mut self) -> Result<(), String> {
        let mut report = tokio::time::interval(WEAK_POOL_REPORT_INTERVAL);
        report.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        report.tick().await;
        let mut release = tokio::time::interval(HELD_RELEASE_INTERVAL);
        release.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        release.tick().await;
        loop {
            tokio::select! {
                event = self.events_rx.recv() => match event {
                    Some(event) => self.handle(event).await,
                    None => return Ok(()),
                },
                _ = release.tick() => self.release_held(),
                _ = report.tick() => self.report(),
            }
        }
    }

    /// Check one weak share and pool it if it passes, or hold it while its
    /// parent is unknown and ask its peer for the parent.
    async fn handle(&mut self, event: WeakShareEvent) {
        let WeakShareEvent { weak_share, source } = event;
        let pow_hash = weak_share.pow_hash();
        if !self.pool.admit(pow_hash, source) {
            return;
        }
        let parent = weak_share.header.prev_share_blockhash;
        match self.parent_height(&parent) {
            ParentHeight::Known(parent_height) => {
                self.check_and_pool(weak_share, source, parent_height, now_millis())
            }
            ParentHeight::Unknown => self.hold(weak_share, source).await,
            ParentHeight::Unreadable(error) => {
                debug!("Cannot read the parent of weak share {pow_hash}: {error}");
                self.pool.refuse(WeakShareRefusal::Unchecked);
            }
        }
    }

    /// What the store knows of `parent`.
    fn parent_height(&self, parent: &BlockHash) -> ParentHeight {
        match self.chain_store_handle.get_block_metadata(parent) {
            Ok(metadata) => match metadata.expected_height {
                Some(parent_height) => ParentHeight::Known(parent_height),
                None => ParentHeight::Unknown,
            },
            Err(StoreError::NotFound(_)) => ParentHeight::Unknown,
            Err(error) => ParentHeight::Unreadable(error),
        }
    }

    /// Hold a weak share whose parent is unknown, and ask the peer that sent
    /// it for the parent when no held weak share waited for it before.
    async fn hold(&mut self, weak_share: WeakShare, source: WeakShareSource) {
        let pow_hash = weak_share.pow_hash();
        let parent = weak_share.header.prev_share_blockhash;
        let outcome = self.pool.hold(weak_share, source);
        debug!("Holding weak share {pow_hash} until its parent {parent} arrives: {outcome:?}");
        if let (HoldOutcome::NewParent, Some(peer)) = (outcome, source.peer())
            && let Err(error) = request_headers_for_missing_blocks(
                &[parent],
                peer,
                self.chain_store_handle.clone(),
                self.swarm_tx.clone(),
            )
            .await
        {
            debug!("Failed to ask peer {peer} for weak share parent {parent}: {error}");
        }
    }

    /// Check the held weak shares whose parent arrived as new, dropping
    /// those too deep for a share on the confirmed tip to carry.
    fn release_held(&mut self) {
        let parents = self.pool.held_parents();
        if parents.is_empty() {
            return;
        }
        let parent_heights: HashMap<BlockHash, u32> =
            match self.chain_store_handle.get_block_metadata_batch(&parents) {
                Ok(rows) => rows
                    .into_iter()
                    .filter_map(|(parent, metadata)| Some((parent, metadata.expected_height?)))
                    .collect(),
                Err(error) => {
                    debug!("Cannot read the parents of held weak shares: {error}");
                    return;
                }
            };
        if parent_heights.is_empty() {
            return;
        }
        let tip_height = match self.chain_store_handle.get_tip_height() {
            Ok(Some(tip_height)) => tip_height,
            Ok(None) => return,
            Err(error) => {
                debug!("Cannot read the tip for held weak shares: {error}");
                return;
            }
        };
        for released in self.pool.release_held(&parent_heights, tip_height) {
            self.check_and_pool(
                released.weak_share,
                released.source,
                released.parent_height,
                released.held_at_millis,
            );
        }
    }

    /// Run a weak share's checks and pool it, on its parent at
    /// `parent_height`, if it passes. One whose check cannot finish yet (a
    /// `Recoverable` failure) is held, from `held_at_millis`, to be checked
    /// again once the confirmed tip moves; one that fails is refused but not
    /// remembered, so another copy of its proof of work is still checked.
    fn check_and_pool(
        &mut self,
        weak_share: WeakShare,
        source: WeakShareSource,
        parent_height: u32,
        held_at_millis: u64,
    ) {
        let pow_hash = weak_share.pow_hash();
        if let Err(error) = self.share_validator.validate_weak_share(
            &weak_share,
            &self.chain_store_handle,
            &self.pplns_window,
            &mut self.cache,
        ) {
            match error.kind() {
                FailureKind::Consensus => {
                    info!("Refused weak share {pow_hash} from {source:?}: {error}");
                    self.pool.refuse(WeakShareRefusal::Invalid);
                }
                FailureKind::Recoverable => {
                    debug!("Holding weak share {pow_hash} from {source:?} to check again: {error}");
                    let tip_height = self.chain_store_handle.get_tip_height().ok().flatten();
                    self.pool
                        .hold_for_retry(weak_share, source, held_at_millis, tip_height);
                }
                _ => {
                    debug!("Cannot check weak share {pow_hash} from {source:?}: {error}");
                    self.pool.refuse(WeakShareRefusal::Unchecked);
                }
            }
            return;
        }
        debug!(
            "Pooled weak share {pow_hash} on {} at height {parent_height}",
            weak_share.header.prev_share_blockhash
        );
        self.pool.insert(weak_share, parent_height, source);
    }

    /// Log what the weak pool has done since the node started.
    fn report(&self) {
        let stats = self.pool.stats();
        info!(
            "weak shares: found={} received={} carried={} expired={} pooled={} waiting_for_parent={}",
            stats.counts.found,
            stats.counts.received,
            stats.counts.carried,
            stats.counts.expired,
            stats.pooled,
            stats.held
        );
    }
}

#[cfg(test)]
mod tests;
