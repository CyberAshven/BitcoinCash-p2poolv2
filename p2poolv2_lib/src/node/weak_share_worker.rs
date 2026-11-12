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
use crate::node::messages::{GetData, Message};
use crate::node::p2p_message_handlers::receivers::request_missing_blocks::request_headers_for_missing_blocks;
pub use crate::node::weak_share_fetch::WeakFetchEvent;
use crate::node::weak_share_fetch::{WeakShareFetch, WeakShareFetchSend};
#[cfg(test)]
#[mockall_double::double]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
#[cfg(not(test))]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
use crate::shares::validation::{FailureKind, ShareValidator, WeakPayoutCache};
use crate::shares::weak::WeakShare;
use crate::shares::weak_pool::{
    HoldOutcome, WeakPool, WeakRelayReceiver, WeakShareRefusal, WeakShareSource, can_be_carried,
    now_millis,
};
use crate::store::writer::StoreError;
use bitcoin::BlockHash;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::{debug, info};

/// Weak shares waiting for their checks.
pub const WEAK_SHARE_EVENT_CHANNEL_CAPACITY: usize = 1024;

/// Inventories and fetch responses waiting for the weak-share worker.
///
/// Sized for one burst of inventories. A full channel drops the event, and
/// an in-flight fetch still times out on the worker's tick.
pub const WEAK_FETCH_EVENT_CHANNEL_CAPACITY: usize = 256;

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

pub type WeakShareEventReceiver = mpsc::Receiver<WeakShareEvent>;

/// The channel into the weak-share worker, shared by stratum and the peer
/// handlers.
///
/// It also carries the pool and the channel of hash announcements. A
/// `GetData` for a weak share is answered from that pool, and an inventory
/// of hashes is fetched once, on the same handle the request context already
/// stores. Cloning shares both channels and the pool.
#[derive(Clone)]
pub struct WeakShareEventSender {
    events: mpsc::Sender<WeakShareEvent>,
    fetches: mpsc::Sender<WeakFetchEvent>,
    pool: WeakPool,
}

impl WeakShareEventSender {
    /// Hand `event` to the worker unless the channel is full or closed.
    #[allow(clippy::result_large_err)]
    pub fn try_send(
        &self,
        event: WeakShareEvent,
    ) -> Result<(), mpsc::error::TrySendError<WeakShareEvent>> {
        self.events.try_send(event)
    }

    /// Hand `event` to the worker, waiting while the channel is full.
    #[allow(clippy::result_large_err)]
    pub async fn send(
        &self,
        event: WeakShareEvent,
    ) -> Result<(), mpsc::error::SendError<WeakShareEvent>> {
        self.events.send(event).await
    }

    /// The pool the worker checks. The clone shares that pool.
    pub(crate) fn pool(&self) -> WeakPool {
        self.pool.clone()
    }

    /// The channel an inventory and a fetch response are sent on.
    ///
    /// The clone is the sender handle, so the caller does not borrow this
    /// one across an await.
    pub(crate) fn fetch_sender(&self) -> mpsc::Sender<WeakFetchEvent> {
        self.fetches.clone()
    }

    /// Hand a fetch event to the worker unless that channel is full or closed.
    #[allow(clippy::result_large_err)]
    pub(crate) fn try_send_fetch(
        &self,
        event: WeakFetchEvent,
    ) -> Result<(), mpsc::error::TrySendError<WeakFetchEvent>> {
        self.fetches.try_send(event)
    }
}

/// What a Bitcoin Cash P2Pool node needs for weak shares: the pool, the
/// receiver of the weak shares it accepts, to relay, and the channel into the
/// worker that checks them, whose sender stratum and the peer handlers share.
pub struct WeakShareNetwork {
    pub pool: WeakPool,
    pub relay_rx: WeakRelayReceiver,
    pub events_tx: WeakShareEventSender,
    pub events_rx: WeakShareEventReceiver,
    /// Announcements and fetch responses. The worker owns this receiver.
    pub fetch_rx: mpsc::Receiver<WeakFetchEvent>,
}

impl WeakShareNetwork {
    /// A pool over `pool` and fresh channels into its worker.
    ///
    /// The sender keeps a clone of the pool so a `GetData` can be answered
    /// from it while the worker holds the other clone. Both are the same pool.
    pub fn new(pool: WeakPool, relay_rx: WeakRelayReceiver) -> Self {
        let (events_tx, events_rx) = mpsc::channel(WEAK_SHARE_EVENT_CHANNEL_CAPACITY);
        let (fetch_tx, fetch_rx) = mpsc::channel(WEAK_FETCH_EVENT_CHANNEL_CAPACITY);
        let events_tx = WeakShareEventSender {
            events: events_tx,
            fetches: fetch_tx,
            pool: pool.clone(),
        };
        Self {
            pool,
            relay_rx,
            events_tx,
            events_rx,
            fetch_rx,
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
    fetch_rx: mpsc::Receiver<WeakFetchEvent>,
    /// Set once the fetch channel closes, so the loop does not spin on it.
    /// The worker still runs until the body channel closes.
    fetch_closed: bool,
    /// Which announced hashes this node has asked for. This task alone reads it.
    fetch: WeakShareFetch,
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
        fetch_rx: mpsc::Receiver<WeakFetchEvent>,
        pool: WeakPool,
        chain_store_handle: ChainStoreHandle,
        share_validator: Arc<dyn ShareValidator + Send + Sync>,
        pplns_window: Arc<RwLock<PplnsWindow>>,
        swarm_tx: mpsc::Sender<SwarmSend<C>>,
    ) -> Self {
        Self {
            events_rx,
            fetch_rx,
            fetch_closed: false,
            fetch: WeakShareFetch::new(),
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
                event = self.fetch_rx.recv(), if !self.fetch_closed => match event {
                    Some(event) => self.on_fetch(event).await,
                    None => self.fetch_closed = true,
                },
                _ = release.tick() => {
                    let mut sends = self.release_held();
                    sends.extend(self.fetch.poll(Instant::now()));
                    self.dispatch_fetches(sends).await;
                }
                _ = report.tick() => self.report(),
            }
        }
    }

    /// Check one weak share and pool it if it passes, or hold it while its
    /// parent is unknown and ask its peer for the parent.
    ///
    /// An in-flight fetch of this hash is cleared before the pool admits it,
    /// so the slot is free even when the body is a duplicate. Announcers stay
    /// parked until the check says whether to ask the next peer.
    async fn handle(&mut self, event: WeakShareEvent) {
        let WeakShareEvent { weak_share, source } = event;
        let pow_hash = weak_share.pow_hash();
        let now = Instant::now();
        self.fetch.body_arrived(pow_hash);
        let seen = self.pool.has_seen(&pow_hash);
        if !self.pool.admit(pow_hash, source) {
            // `admit` is false for a duplicate or for a peer over its rate.
            // Seen was read first so a duplicate is not treated as the rate:
            // only the rate must wait, or the next announcer would spend a
            // second peer's allowance on the same hash.
            if !seen && source.peer().is_some() {
                self.fetch.rate_limited(pow_hash, now);
            } else {
                self.fetch.settled(pow_hash);
            }
            self.dispatch_fetches(Vec::new()).await;
            return;
        }
        let parent = weak_share.header.prev_share_blockhash;
        let sends = match self.parent_height(&parent) {
            ParentHeight::Known(parent_height) if self.too_deep(parent_height) => {
                debug!("Weak share {pow_hash} from {source:?} is too deep to carry, dropping it");
                self.pool.refuse(WeakShareRefusal::TooDeep);
                self.fetch.settled(pow_hash);
                Vec::new()
            }
            ParentHeight::Known(parent_height) => {
                self.check_and_pool(weak_share, source, parent_height, now_millis())
            }
            ParentHeight::Unknown => self.hold(weak_share, source).await,
            ParentHeight::Unreadable(error) => {
                debug!("Cannot read the parent of weak share {pow_hash}: {error}");
                self.pool.refuse(WeakShareRefusal::Unchecked);
                self.fetch.settled(pow_hash);
                Vec::new()
            }
        };
        self.dispatch_fetches(sends).await;
    }

    /// Record an announcement, a fetched body, or a `NotFound`, and send the
    /// `GetData`s the book asks for.
    async fn on_fetch(&mut self, event: WeakFetchEvent) {
        let now = Instant::now();
        match event {
            WeakFetchEvent::Announced { peer, hashes } => {
                for hash in hashes {
                    let seen = self.pool.has_seen(&hash);
                    self.fetch.note(peer, hash, seen, now);
                }
                self.dispatch_fetches(Vec::new()).await;
            }
            WeakFetchEvent::Body { peer, weak_share } => {
                self.handle(WeakShareEvent {
                    weak_share,
                    source: WeakShareSource::Peer(peer),
                })
                .await;
            }
            WeakFetchEvent::NotFound { hash } => {
                let sends = self.fetch.not_found(hash, now);
                self.dispatch_fetches(sends).await;
            }
        }
    }

    /// Whether a weak share on a parent at `parent_height` is too deep for
    /// the next share on the confirmed tip to carry, more than `WEAK_DEPTH`
    /// below it ([`can_be_carried`]). Checked before any other rule, so a
    /// replayed old weak share costs a metadata read, not a window walk. A
    /// tip that cannot be read leaves the depth to the carrier's check.
    fn too_deep(&self, parent_height: u32) -> bool {
        self.chain_store_handle
            .get_tip_height()
            .ok()
            .flatten()
            .is_some_and(|tip_height| !can_be_carried(parent_height, tip_height))
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
    ///
    /// A share the pool kept is marked held in the fetch book, so a second
    /// announcement does not fetch another copy. One the pool did not keep
    /// is settled. Either way nothing is sent from here: the caller sends.
    async fn hold(
        &mut self,
        weak_share: WeakShare,
        source: WeakShareSource,
    ) -> Vec<WeakShareFetchSend> {
        let pow_hash = weak_share.pow_hash();
        let parent = weak_share.header.prev_share_blockhash;
        let outcome = self.pool.hold(weak_share, source);
        debug!("Holding weak share {pow_hash} until its parent {parent} arrives: {outcome:?}");
        if outcome == HoldOutcome::NotHeld {
            self.fetch.settled(pow_hash);
        } else {
            self.fetch.held(pow_hash);
        }
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
        Vec::new()
    }

    /// Check the held weak shares whose parent arrived as new, dropping
    /// those too deep for a share on the confirmed tip to carry.
    ///
    /// The `GetData`s a failed check asks for are returned. The caller sends
    /// them. This stays synchronous so the existing tests can call it without
    /// waiting on the swarm channel.
    fn release_held(&mut self) -> Vec<WeakShareFetchSend> {
        let parents = self.pool.held_parents();
        if parents.is_empty() {
            return Vec::new();
        }
        let parent_heights: HashMap<BlockHash, u32> =
            match self.chain_store_handle.get_block_metadata_batch(&parents) {
                Ok(rows) => rows
                    .into_iter()
                    .filter_map(|(parent, metadata)| Some((parent, metadata.expected_height?)))
                    .collect(),
                Err(error) => {
                    debug!("Cannot read the parents of held weak shares: {error}");
                    return Vec::new();
                }
            };
        if parent_heights.is_empty() {
            return Vec::new();
        }
        let tip_height = match self.chain_store_handle.get_tip_height() {
            Ok(Some(tip_height)) => tip_height,
            Ok(None) => return Vec::new(),
            Err(error) => {
                debug!("Cannot read the tip for held weak shares: {error}");
                return Vec::new();
            }
        };
        let mut sends = Vec::new();
        for released in self.pool.release_held(&parent_heights, tip_height) {
            sends.extend(self.check_and_pool(
                released.weak_share,
                released.source,
                released.parent_height,
                released.held_at_millis,
            ));
        }
        sends
    }

    /// Run a weak share's checks and pool it, on its parent at
    /// `parent_height`, if it passes. One whose check cannot finish yet (a
    /// `Recoverable` failure) is held, from `held_at_millis`, to be checked
    /// again once the confirmed tip moves; one that fails is refused but not
    /// remembered, so another copy of its proof of work is still checked.
    ///
    /// A consensus failure asks the next announcer. The pool does not mark
    /// that hash seen, so the real copy can still pass. A rate is not this
    /// path. Anything else settles the hash: it is not fetched again from
    /// this announcement.
    fn check_and_pool(
        &mut self,
        weak_share: WeakShare,
        source: WeakShareSource,
        parent_height: u32,
        held_at_millis: u64,
    ) -> Vec<WeakShareFetchSend> {
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
                    return self.fetch.invalid(pow_hash, Instant::now());
                }
                FailureKind::Recoverable => {
                    debug!("Holding weak share {pow_hash} from {source:?} to check again: {error}");
                    let tip_height = self.chain_store_handle.get_tip_height().ok().flatten();
                    let outcome =
                        self.pool
                            .hold_for_retry(weak_share, source, held_at_millis, tip_height);
                    if outcome == HoldOutcome::NotHeld {
                        self.fetch.settled(pow_hash);
                    } else {
                        self.fetch.held(pow_hash);
                    }
                }
                _ => {
                    debug!("Cannot check weak share {pow_hash} from {source:?}: {error}");
                    self.pool.refuse(WeakShareRefusal::Unchecked);
                    self.fetch.settled(pow_hash);
                }
            }
            return Vec::new();
        }
        debug!(
            "Pooled weak share {pow_hash} on {} at height {parent_height}",
            weak_share.header.prev_share_blockhash
        );
        self.pool.insert(weak_share, parent_height, source);
        self.fetch.settled(pow_hash);
        Vec::new()
    }

    /// Forget holds the pool has dropped, then send `sends` and any further
    /// fetch the freed slots allow.
    ///
    /// The pool is cloned because the book is borrowed mutably while it asks
    /// which hashes are still held. The clone shares the same pool.
    async fn dispatch_fetches(&mut self, mut sends: Vec<WeakShareFetchSend>) {
        let pool = self.pool.clone();
        self.fetch.retain_holds(|hash| pool.holds(hash));
        sends.extend(self.fetch.take_next(Instant::now()));
        self.send_fetches(sends).await;
    }

    /// Send each `GetData` for a weak share. A send that fails leaves the
    /// in-flight entry for the ten-second poll, which asks the next announcer.
    ///
    /// The await is the same shape as the block fetcher's
    /// `swarm_tx.send(SwarmSend::Request(peer_id, message)).await`
    /// (`request_response_handler/block_fetcher/mod.rs`).
    async fn send_fetches(&self, sends: Vec<WeakShareFetchSend>) {
        for send in sends {
            if let Err(error) = self
                .swarm_tx
                .send(SwarmSend::Request(
                    send.peer,
                    Message::GetData(GetData::WeakShare(send.hash)),
                ))
                .await
            {
                let hash = send.hash;
                debug!("Failed to send GetData for weak share {hash}: {error}");
            }
        }
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
