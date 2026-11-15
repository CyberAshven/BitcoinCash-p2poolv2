// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Keep the known Bitcoin Cash headers in step with this node's own BCHN.
//!
//! Every few seconds the feeder asks BCHN for its best block. When the store
//! does not know it, the feeder walks back with `getblockheader` until it
//! reaches a known header, or the chain's first block, and stores the
//! headers it walked, oldest first, each one height above its parent. A
//! reorg is the same walk: the new branch is added and the old one stays
//! known, since a share on a block that lost an orphan race is still placed
//! on the chain. [`BchHeaderFeeder::learn_from`] runs the same walk from any
//! block BCHN knows, such as a new template's previous block.
//!
//! The feeder also keeps an index of BCHN's best chain, height to hash. After
//! each walk it points the tip's height and every height below it that
//! disagrees at the best chain's blocks, and drops heights above a tip that
//! moved down. A sender uses the index to tell which headers its peers
//! surely have.
//!
//! On a node whose store knows no header yet, the walk stops after
//! [`INITIAL_HEADERS`] blocks and asks BCHN for the height of the oldest.
//! The node runs one round before it dials peers
//! ([`BchHeaderFeeder::first_sync`]), so the share bodies they send find
//! their BCH prevs known instead of being dropped and fetched again. One walk
//! runs at a time in a node: the feeder, the template relay and that first
//! round each walk BCHN's headers, and a walk that waited finds them known.
//! These headers come from the node's own BCHN, which has validated them, so
//! they are not run through `BchHeaderRules::check`; headers from peers are.
//!
//! Part of mining on the current chain (V16,
//! `docs/sharechain/current-chain.md`).

use crate::shares::bch_asert::{
    BCH_BLOCK_SPACING, IntervalSwitch, ReportedTicks, TickSchedule, activation_height_from_times,
    learn_switch, switch_from_report,
};
use crate::shares::bch_headers::KnownBchHeader;
#[cfg(test)]
#[mockall_double::double]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
#[cfg(not(test))]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
use crate::stratum::work::notify::{NotifyCmd, NotifyReceiver, NotifySender};
use bitcoin::BlockHash;
use bitcoin::block::Header;
use bitcoin::hashes::Hash;
use bitcoindrpc::BitcoindRpcClient;
use std::str::FromStr;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::{MissedTickBehavior, interval, timeout};
use tracing::{debug, info, warn};

/// How often the feeder asks BCHN for its best block.
const FEED_INTERVAL: Duration = Duration::from_secs(5);

/// The fewest headers a node first learns back from BCHN's tip: 8,400
/// ten-minute blocks, [`INITIAL_HEADER_TICKS`].
pub const INITIAL_HEADERS: usize = 8_400;

/// How far back a node first learns BCHN's headers, in ticks (seconds of
/// target time): about 58 days, enough for the payout window (about 14.7
/// days with its maturity skip), the depth shares are validated to (about
/// 29.4 days) and room besides. That is 8,400 ten-minute blocks or 84,000
/// one-minute blocks (CHIP-2025-03).
pub const INITIAL_HEADER_TICKS: i64 = INITIAL_HEADERS as i64 * BCH_BLOCK_SPACING;

/// The most known headers the feeder walks back to find an announced
/// switch from block times: about a week of one-minute blocks.
const MAX_ACTIVATION_SEARCH: usize = 10_000;

/// How long a node waits for its first round of BCHN's headers before it
/// syncs shares anyway. A new store's walk of [`INITIAL_HEADERS`] took about
/// a minute against a BCHN over Tailscale.
pub(crate) const FIRST_SYNC_LIMIT: Duration = Duration::from_secs(300);

/// One header walk at a time in a node process. Without it, the walks of a
/// new store by the feeder, the template relay and the first round would
/// each fetch the same [`INITIAL_HEADERS`] from BCHN.
static WALK: Mutex<()> = Mutex::const_new(());

#[derive(Clone)]
pub(crate) struct BchHeaderFeeder {
    client: BitcoindRpcClient,
    store: ChainStoreHandle,
    initial_headers: usize,
    /// How far back, in ticks, a store with nothing to link to learns:
    /// [`INITIAL_HEADER_TICKS`].
    initial_header_ticks: i64,
    /// The network's move to one-minute blocks, which sets how many
    /// headers `initial_header_ticks` is.
    tick_schedule: TickSchedule,
}

impl BchHeaderFeeder {
    pub(crate) fn new(
        client: BitcoindRpcClient,
        store: ChainStoreHandle,
        tick_schedule: TickSchedule,
    ) -> Self {
        Self {
            client,
            store,
            initial_headers: INITIAL_HEADERS,
            initial_header_ticks: INITIAL_HEADER_TICKS,
            tick_schedule,
        }
    }

    /// Follow BCHN's best block until the node stops. A failed round is
    /// logged and retried on the next tick; it never stops other workers.
    pub(crate) async fn run(self) {
        let mut timer = interval(FEED_INTERVAL);
        timer.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            timer.tick().await;
            match self.sync_once().await {
                Ok(0) => {}
                Ok(added) => debug!("Learned {added} Bitcoin Cash headers from the node"),
                Err(error) => warn!("Cannot follow the node's headers: {error}"),
            }
        }
    }

    /// One round before this node syncs shares from peers, so the share
    /// bodies it fetches find their BCH prevs known. Gives up after `limit`,
    /// or on a BCHN error, and leaves the rest to [`Self::run`]: a node
    /// whose BCHN is down still starts.
    pub(crate) async fn first_sync(&self, limit: Duration) {
        match timeout(limit, self.sync_once()).await {
            Ok(Ok(added)) => {
                info!("Learned {added} Bitcoin Cash headers from the node before syncing shares")
            }
            Ok(Err(error)) => {
                warn!("Cannot follow the node's headers before syncing shares: {error}")
            }
            Err(_) => warn!(
                "The node's headers took longer than {} s; syncing shares while the feeder goes on",
                limit.as_secs()
            ),
        }
    }

    /// Learn the headers from BCHN's best block back to a known one, index
    /// BCHN's best chain, and return how many headers were added.
    async fn sync_once(&self) -> Result<usize, String> {
        let best = self
            .client
            .get_best_block_hash()
            .await
            .map_err(|error| format!("getbestblockhash: {error}"))?;
        let best =
            BlockHash::from_str(&best).map_err(|error| format!("best block hash: {error}"))?;
        let added = self.learn_from(best).await?;
        self.index_best_chain(best).await?;
        if added > 0
            && let Err(error) = self.follow_schedule(best).await
        {
            warn!("Cannot follow the node's block interval: {error}");
        }
        Ok(added)
    }

    /// Learn `hash` and the headers below it back to a known one, from BCHN,
    /// and return how many were added. BCHN must know `hash`.
    pub(crate) async fn learn_from(&self, hash: BlockHash) -> Result<usize, String> {
        let _walk = WALK.lock().await;
        let mut hash = hash;
        let mut walked: Vec<Header> = Vec::new();
        let mut limit = self.initial_headers;
        let oldest_height = loop {
            if let Some(known) = self.known(&hash)? {
                if walked.is_empty() {
                    return Ok(0);
                }
                break known.height + 1;
            }
            if walked.len() == limit {
                // A store with nothing to link to: anchor the walk on the
                // height BCHN gives its oldest header, once the walk spans
                // `INITIAL_HEADER_TICKS`. One-minute blocks need more
                // headers than the first limit for that span.
                let oldest = walked.last().expect("walked is not empty");
                let summary = self
                    .client
                    .get_block_header_summary(&oldest.block_hash().to_string())
                    .await
                    .map_err(|error| format!("getblockheader {}: {error}", oldest.block_hash()))?;
                let height = u32::try_from(summary.height)
                    .map_err(|_| format!("height {} is too high", summary.height))?;
                let tip_height = height.saturating_add(walked.len() as u32 - 1);
                let wanted = self
                    .tick_schedule
                    .blocks_spanning(tip_height, self.initial_header_ticks)
                    .max(self.initial_headers);
                if walked.len() >= wanted || height == 0 {
                    info!(
                        "Learning {} Bitcoin Cash headers back from the node's tip to height {height}",
                        walked.len()
                    );
                    break height;
                }
                limit = wanted;
            }
            let header = self
                .client
                .get_block_header(&hash.to_string())
                .await
                .map_err(|error| format!("getblockheader {hash}: {error}"))?;
            hash = header.prev_blockhash;
            walked.push(header);
            if hash == BlockHash::all_zeros() {
                // The chain's first block, at height 0.
                break 0;
            }
        };
        let added = walked.len();
        let headers = walked
            .into_iter()
            .rev()
            .zip(oldest_height..)
            .map(|(header, height)| KnownBchHeader { height, header })
            .collect();
        self.store
            .add_bch_headers(headers)
            .await
            .map_err(|error| format!("storing headers: {error}"))?;
        Ok(added)
    }

    /// Point the best-chain index at the chain ending at `tip`: every height
    /// from the tip down to the first one already indexed with the right
    /// block, and no height above the tip.
    async fn index_best_chain(&self, tip: BlockHash) -> Result<(), String> {
        let tip = self
            .known(&tip)?
            .ok_or_else(|| format!("best block {tip} is not known"))?;
        // The most entries one update rewrites: a reorg deeper than BCHN
        // allows (10 blocks), or a first index over the whole store, which
        // spans at most the initial ticks at this tip.
        let max_rewrite = self
            .tick_schedule
            .blocks_spanning(tip.height, self.initial_header_ticks)
            .max(self.initial_headers)
            + 1;
        let mut updates = Vec::new();
        let mut current = tip;
        while updates.len() < max_rewrite {
            let indexed = self
                .store
                .get_bch_best_hash(current.height)
                .map_err(|error| format!("reading the best-chain index: {error}"))?;
            if indexed == Some(current.block_hash()) {
                break;
            }
            updates.push((current.height, current.block_hash()));
            let parent = current.header.prev_blockhash;
            match self.known(&parent)? {
                Some(parent) if current.height > 0 => current = parent,
                _ => break,
            }
        }
        let above_tip = self
            .store
            .get_bch_best_hash(tip.height.saturating_add(1))
            .map_err(|error| format!("reading the best-chain index: {error}"))?;
        if updates.is_empty() && above_tip.is_none() {
            return Ok(());
        }
        self.store
            .set_bch_best_chain(updates, tip.height)
            .await
            .map_err(|error| format!("writing the best-chain index: {error}"))
    }

    /// Follow the node's block intervals (CHIP-2025-03): learn the switch
    /// its report on `tip` shows (`switch_from_report`), or, from a node
    /// that reports no ticks, the activation block of an announced switch
    /// from the median time past of the known headers. A learned switch
    /// reaches every schedule of the network (`learn_switch`).
    async fn follow_schedule(&self, tip: BlockHash) -> Result<(), String> {
        let Some(network) = self.tick_schedule.network() else {
            return Ok(());
        };
        let summary = self
            .client
            .get_block_header_summary(&tip.to_string())
            .await
            .map_err(|error| format!("getblockheader {tip}: {error}"))?;
        let report = ReportedTicks {
            height: u32::try_from(summary.height)
                .map_err(|_| format!("height {} is too high", summary.height))?,
            ticks: summary.ticks,
            interval: summary.target_time,
            next_interval: summary.next_target_time,
        };
        let reports_ticks =
            report.ticks.is_some() || report.interval.is_some() || report.next_interval.is_some();
        let switch = if reports_ticks {
            switch_from_report(&self.tick_schedule, report)?
        } else {
            self.announced_switch_from_times(tip)?
        };
        let Some(switch) = switch else {
            return Ok(());
        };
        if learn_switch(network, switch)
            .map_err(|error| format!("the node's switch {switch:?}: {error}"))?
        {
            info!(
                "Following the node's block interval: {} ticks for every block after height {}",
                switch.interval, switch.height
            );
        }
        Ok(())
    }

    /// The activation block of the network's announced switch, from the
    /// known headers below `tip`, once `tip` is dated past the activation
    /// time. Walks back at most [`MAX_ACTIVATION_SEARCH`] headers.
    fn announced_switch_from_times(
        &self,
        tip: BlockHash,
    ) -> Result<Option<IntervalSwitch>, String> {
        let Some(announced) = self.tick_schedule.announced() else {
            return Ok(None);
        };
        let mut times = Vec::new();
        let mut current = self.known(&tip)?;
        if current
            .as_ref()
            .is_none_or(|header| i64::from(header.header.time) < announced.activation_time)
        {
            return Ok(None);
        }
        while let Some(header) = current {
            times.push((header.height, header.header.time));
            if times.len() == MAX_ACTIVATION_SEARCH || header.height == 0 {
                break;
            }
            current = self.known(&header.header.prev_blockhash)?;
        }
        times.reverse();
        Ok(
            activation_height_from_times(announced, &times).map(|height| IntervalSwitch {
                height,
                interval: announced.interval,
            }),
        )
    }

    fn known(&self, hash: &BlockHash) -> Result<Option<KnownBchHeader>, String> {
        self.store
            .get_bch_header(hash)
            .map_err(|error| format!("reading header {hash}: {error}"))
    }
}

/// Relay block templates from `input` to the notifier on `output`, first
/// learning each new template's previous block from BCHN. Without it a
/// share mined in the seconds after a new block, before the next feeder
/// round, would build on a block this node does not know: it could not
/// check the share, nor send peers the header to place it.
///
/// A template whose previous block cannot be learned is still relayed: the
/// feeder's next round retries, and holding work back would idle miners.
pub async fn relay_templates_learning_prevs(
    client: BitcoindRpcClient,
    store: ChainStoreHandle,
    tick_schedule: TickSchedule,
    mut input: NotifyReceiver,
    output: NotifySender,
) {
    let feeder = BchHeaderFeeder::new(client, store, tick_schedule);
    let mut learned: Option<BlockHash> = None;
    while let Some(command) = input.recv().await {
        if let NotifyCmd::SendToAll { template } = &command
            && let Ok(prev) = BlockHash::from_str(&template.previousblockhash)
            && learned != Some(prev)
        {
            match feeder.learn_from(prev).await {
                Ok(_) => learned = Some(prev),
                Err(error) => warn!("Cannot learn template parent {prev} from the node: {error}"),
            }
        }
        if output.send(command).await.is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests;
