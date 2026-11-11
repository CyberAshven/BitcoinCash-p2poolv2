// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The worker that checks weak shares before the weak pool keeps and relays
//! them (`shares::weak_pool`).
//!
//! Weak shares reach it from this node's stratum, which found them, and
//! from peers, which relayed them. Each proof-of-work hash is checked once:
//! the pool admits a hash it has not seen, from a peer within its rate, and
//! the worker then runs every rule but the carrier's
//! (`ShareValidator::validate_weak_share`), with one cache of the window's
//! payouts across weak shares on the same template. A weak share that
//! passes goes into the pool, which queues it for the node actor to relay.
//!
//! One task, fed by a bounded channel: a burst of weak shares waits here
//! rather than in the actor loop or the request handlers.

#[cfg(test)]
#[mockall_double::double]
use crate::accounting::payout::sharechain_pplns::PplnsWindow;
#[cfg(not(test))]
use crate::accounting::payout::sharechain_pplns::PplnsWindow;
#[cfg(test)]
#[mockall_double::double]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
#[cfg(not(test))]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
use crate::shares::validation::{FailureKind, ShareValidator, WeakPayoutCache};
use crate::shares::weak::WeakShare;
use crate::shares::weak_pool::{WeakPool, WeakRelayReceiver, WeakShareRefusal, WeakShareSource};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, info};

/// Weak shares waiting for their checks.
pub const WEAK_SHARE_EVENT_CHANNEL_CAPACITY: usize = 1024;

/// How often the worker logs what the weak pool has done.
const WEAK_POOL_REPORT_INTERVAL: Duration = Duration::from_secs(60);

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

/// Checks weak shares and hands the ones that pass to the weak pool.
pub struct WeakShareWorker {
    events_rx: WeakShareEventReceiver,
    pool: WeakPool,
    chain_store_handle: ChainStoreHandle,
    share_validator: Arc<dyn ShareValidator + Send + Sync>,
    pplns_window: Arc<RwLock<PplnsWindow>>,
    cache: WeakPayoutCache,
}

impl WeakShareWorker {
    pub fn new(
        events_rx: WeakShareEventReceiver,
        pool: WeakPool,
        chain_store_handle: ChainStoreHandle,
        share_validator: Arc<dyn ShareValidator + Send + Sync>,
        pplns_window: Arc<RwLock<PplnsWindow>>,
    ) -> Self {
        Self {
            events_rx,
            pool,
            chain_store_handle,
            share_validator,
            pplns_window,
            cache: WeakPayoutCache::new(),
        }
    }

    /// Check weak shares until every sender is gone, logging the pool's
    /// counts every [`WEAK_POOL_REPORT_INTERVAL`].
    pub async fn run(mut self) -> Result<(), String> {
        let mut report = tokio::time::interval(WEAK_POOL_REPORT_INTERVAL);
        report.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        report.tick().await;
        loop {
            tokio::select! {
                event = self.events_rx.recv() => match event {
                    Some(event) => self.handle(event),
                    None => return Ok(()),
                },
                _ = report.tick() => self.report(),
            }
        }
    }

    /// Check one weak share and pool it if it passes.
    fn handle(&mut self, event: WeakShareEvent) {
        let WeakShareEvent { weak_share, source } = event;
        let pow_hash = weak_share.pow_hash();
        if !self.pool.admit(pow_hash, source) {
            return;
        }
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
                _ => {
                    debug!("Cannot check weak share {pow_hash} from {source:?} yet: {error}");
                    self.pool.refuse(WeakShareRefusal::Unchecked);
                }
            }
            return;
        }
        let parent = weak_share.header.prev_share_blockhash;
        match self
            .chain_store_handle
            .get_block_metadata(&parent)
            .ok()
            .and_then(|metadata| metadata.expected_height)
        {
            Some(parent_height) => {
                debug!("Pooled weak share {pow_hash} on {parent} at height {parent_height}");
                self.pool.insert(weak_share, parent_height, source);
            }
            None => {
                debug!("Weak share {pow_hash} has no parent height, dropping it");
                self.pool.refuse(WeakShareRefusal::Unchecked);
            }
        }
    }

    /// Log what the weak pool has done since the node started.
    fn report(&self) {
        let counts = self.pool.counts();
        info!(
            "weak shares: found={} received={} carried={} expired={} pooled={}",
            counts.found,
            counts.received,
            counts.carried,
            counts.expired,
            self.pool.len()
        );
    }
}

#[cfg(test)]
mod tests;
