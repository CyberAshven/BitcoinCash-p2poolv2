// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A new Bitcoin Cash share relayed without the outputs of its Bitcoin Cash
//! coinbase (`Message::ShareWithoutPayouts`).
//!
//! The receiver rebuilds the outputs from its PPLNS window at the share's
//! parent and its header, as validation rebuilds a share's coinbase
//! (`ShareValidator::rebuild_coinbase_without_payouts`), and goes on only
//! with a coinbase whose txid is the one the header's coinbase proof gives:
//! the coinbase the miner hashed. The share then has its weak shares filled
//! (`handle_share_with_weak_hashes`) and is checked as a share sent whole.
//!
//! A share it cannot rebuild -- its parent or the window below it not here
//! yet, the window still loading, or a coinbase that comes out with another
//! txid -- is not complete, never invalid: it is asked for whole through
//! the block fetcher, from the peer that relayed it first, as a share whose
//! weak shares cannot be filled is. No peer is disconnected for it: a node
//! relays a share once its header checks out, before it validates the body
//! (`BlockReceiver`), so a share whose coinbase does not pay its window
//! reaches every node through honest peers, and only the whole share shows
//! that it is invalid.

#[cfg(test)]
#[mockall_double::double]
use crate::accounting::payout::sharechain_pplns::PplnsWindow;
#[cfg(not(test))]
use crate::accounting::payout::sharechain_pplns::PplnsWindow;
use crate::node::SwarmSend;
use crate::node::messages::ShareWithoutPayouts;
use crate::node::p2p_message_handlers::receivers::block_receiver::BlockReceiverHandle;
use crate::node::p2p_message_handlers::receivers::share_blocks::handle_share_block;
use crate::node::p2p_message_handlers::receivers::share_with_weak_hashes::{
    fetch_share_whole, handle_share_with_weak_hashes,
};
use crate::node::request_response_handler::block_fetcher::BlockFetcherHandle;
use crate::node::validation_worker::ValidationSender;
use crate::node::weak_share_worker::WeakShareEventSender;
#[cfg(test)]
#[mockall_double::double]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
#[cfg(not(test))]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
use crate::shares::validation::{ShareValidator, ValidationError, WeakPayoutCache};
use bitcoin::Transaction;
use libp2p::PeerId;
use std::error::Error;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;
use tokio::sync::mpsc;
use tracing::debug;

/// Window weights the rebuild cache keeps: the parents a new share builds
/// on at once, a tip and its siblings.
const REBUILD_WEIGHTS_CAPACITY: usize = 4;

/// Expected payouts the rebuild cache keeps: a share's template on each of
/// those parents, twice. The copies of one share that several peers relay
/// before the first is stored then cost one walk of the window and one
/// build of its payouts.
const REBUILD_PAYOUTS_CAPACITY: usize = 8;

/// What a node needs to rebuild the Bitcoin Cash coinbases of the shares its
/// peers relay without their payouts: the PPLNS window validation reads, and
/// a small cache of the window's payouts. Cloned into each peer's request
/// service; the clones share the window, the cache and the counts of how
/// the rebuilds went, which P2P health metrics read.
#[derive(Clone)]
pub struct CoinbaseRebuilder {
    pplns_window: Arc<RwLock<PplnsWindow>>,
    cache: Arc<Mutex<WeakPayoutCache>>,
    counters: Arc<RebuildCounters>,
}

/// The counts behind [`RebuildCounts`], atomic so the node actor reads them
/// without waiting for a rebuild.
#[derive(Default)]
struct RebuildCounters {
    rebuilt: AtomicU64,
    fetched_whole: AtomicU64,
    micros: AtomicU64,
    max_micros: AtomicU64,
}

/// How a node's rebuilds of shares relayed without their payouts went since
/// it started, for P2P health metrics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RebuildCounts {
    /// Shares whose coinbase was rebuilt to the txid their header commits.
    pub rebuilt: u64,
    /// Shares asked for whole instead.
    pub fetched_whole: u64,
    /// Microseconds spent rebuilding, waiting for the window included.
    pub micros: u64,
    /// The longest single rebuild, in microseconds.
    pub max_micros: u64,
}

impl CoinbaseRebuilder {
    /// A rebuilder reading `pplns_window`, the window validation reads.
    pub fn new(pplns_window: Arc<RwLock<PplnsWindow>>) -> Self {
        Self {
            pplns_window,
            cache: Arc::new(Mutex::new(WeakPayoutCache::with_capacities(
                REBUILD_WEIGHTS_CAPACITY,
                REBUILD_PAYOUTS_CAPACITY,
            ))),
            counters: Arc::new(RebuildCounters::default()),
        }
    }

    /// How the rebuilds went since the node started.
    pub fn counts(&self) -> RebuildCounts {
        RebuildCounts {
            rebuilt: self.counters.rebuilt.load(Ordering::Relaxed),
            fetched_whole: self.counters.fetched_whole.load(Ordering::Relaxed),
            micros: self.counters.micros.load(Ordering::Relaxed),
            max_micros: self.counters.max_micros.load(Ordering::Relaxed),
        }
    }

    /// Rebuild the coinbase of `share` on the blocking thread pool, timed
    /// and counted. The rebuild holds the window's lock while it walks the
    /// window, as the weak share worker and the organise worker do, so it
    /// runs where it cannot hold up a runtime thread that peers' requests
    /// share.
    async fn rebuild(
        &self,
        share: ShareWithoutPayouts,
        chain_store_handle: &ChainStoreHandle,
        share_validator: &Arc<dyn ShareValidator + Send + Sync>,
    ) -> Result<(ShareWithoutPayouts, Transaction), Box<dyn Error + Send + Sync>> {
        let rebuilder = self.clone();
        let chain_store_handle = chain_store_handle.clone();
        let share_validator = share_validator.clone();
        let started = Instant::now();
        let (share, rebuilt) = tokio::task::spawn_blocking(move || {
            let rebuilt =
                rebuilder.rebuild_now(&share, &chain_store_handle, share_validator.as_ref());
            (share, rebuilt)
        })
        .await?;
        let micros = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
        self.counters.micros.fetch_add(micros, Ordering::Relaxed);
        self.counters
            .max_micros
            .fetch_max(micros, Ordering::Relaxed);
        let coinbase = rebuilt?;
        self.counters.rebuilt.fetch_add(1, Ordering::Relaxed);
        debug!(
            "Rebuilt the coinbase of share {} relayed without payouts in {micros} us",
            share.block_hash()
        );
        Ok((share, coinbase))
    }

    /// The rebuild itself, under the cache's lock and then the window's.
    /// Only rebuilds take the cache's lock, so the two are always taken in
    /// that order.
    fn rebuild_now(
        &self,
        share: &ShareWithoutPayouts,
        chain_store_handle: &ChainStoreHandle,
        share_validator: &(dyn ShareValidator + Send + Sync),
    ) -> Result<Transaction, ValidationError> {
        let share_block = share.share_block();
        let mut cache = self
            .cache
            .lock()
            .expect("coinbase rebuild cache lock poisoned");
        share_validator.rebuild_coinbase_without_payouts(
            &share_block.header,
            &share_block.bitcoin_coinbase,
            share.coinbase_value(),
            chain_store_handle,
            &self.pplns_window,
            &mut cache,
        )
    }

    /// Count a share relayed without its payouts that was asked for whole.
    fn count_fetched_whole(&self) {
        self.counters.fetched_whole.fetch_add(1, Ordering::Relaxed);
    }
}

/// Handle a new share a peer relayed without its coinbase's outputs.
///
/// A stored share needs nothing rebuilt, and one that fails the header or
/// coinbase proof check is refused before anything is rebuilt or fetched
/// for it: both go to `handle_share_block` as they came, which then reads
/// only the share's hash and header, as for a share sent whole. Any other
/// share has its coinbase rebuilt by `coinbase_rebuilder` and goes on, with
/// the rebuilt coinbase, to `handle_share_with_weak_hashes`. One that cannot
/// be rebuilt to the txid its header commits, or that reaches a node keeping
/// no window, is asked for whole from `peer`.
#[allow(clippy::too_many_arguments)] // handler wiring: each parameter is a distinct collaborator, as for handle_share_block, a params struct would only move the list
pub async fn handle_share_without_payouts<C: Send + Sync + 'static>(
    share: ShareWithoutPayouts,
    peer: PeerId,
    chain_store_handle: &ChainStoreHandle,
    validation_tx: ValidationSender,
    block_receiver_handle: &BlockReceiverHandle,
    block_fetcher_handle: &BlockFetcherHandle,
    share_validator: &Arc<dyn ShareValidator + Send + Sync>,
    weak_share_tx: Option<&WeakShareEventSender>,
    coinbase_rebuilder: Option<&CoinbaseRebuilder>,
    swarm_tx: mpsc::Sender<SwarmSend<C>>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let block_hash = share.block_hash();
    let header = &share.share_block().header;
    if chain_store_handle.share_block_exists(&block_hash)
        || share_validator.validate_header_on_arrival(header).is_err()
        || share_validator
            .validate_coinbase_proof(header, &share.share_block().template_merkle_branches)
            .is_err()
    {
        return handle_share_block(
            share.into_share_block(),
            chain_store_handle,
            validation_tx,
            block_receiver_handle,
            block_fetcher_handle,
            share_validator.as_ref(),
        )
        .await;
    }
    let Some(coinbase_rebuilder) = coinbase_rebuilder else {
        debug!("No PPLNS window to rebuild share {block_hash} from, asking {peer} for it whole");
        fetch_share_whole(block_hash, peer, block_fetcher_handle, None).await;
        return Ok(());
    };
    match coinbase_rebuilder
        .rebuild(share, chain_store_handle, share_validator)
        .await
    {
        Ok((share, coinbase)) => {
            handle_share_with_weak_hashes(
                share.with_coinbase(coinbase),
                peer,
                chain_store_handle,
                validation_tx,
                block_receiver_handle,
                block_fetcher_handle,
                share_validator,
                weak_share_tx,
                swarm_tx,
            )
            .await
        }
        Err(error) => {
            debug!(
                "Cannot rebuild the coinbase of share {block_hash} from {peer} ({error}), asking for it whole"
            );
            coinbase_rebuilder.count_fetched_whole();
            fetch_share_whole(block_hash, peer, block_fetcher_handle, None).await;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests;
