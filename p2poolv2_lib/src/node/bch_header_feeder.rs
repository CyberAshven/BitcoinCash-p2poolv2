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
//! These headers come from the node's own BCHN, which has validated them, so
//! they are not run through `BchHeaderRules::check`; headers from peers are.
//!
//! Part of mining on the current chain (V16,
//! `docs/sharechain/current-chain.md`).

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
use tokio::time::{MissedTickBehavior, interval};
use tracing::{debug, info, warn};

/// How often the feeder asks BCHN for its best block.
const FEED_INTERVAL: Duration = Duration::from_secs(5);

/// Headers a node first learns back from BCHN's tip: about 58 days, enough
/// for the payout window (about 14.7 days with its maturity skip), the
/// depth shares are validated to (about 29.4 days) and room besides.
pub const INITIAL_HEADERS: usize = 8_400;

/// The most best-chain index entries one update rewrites: a reorg deeper
/// than BCHN allows (10 blocks), or a first index over the whole store.
const MAX_INDEX_REWRITE: usize = INITIAL_HEADERS + 1;

#[derive(Clone)]
pub(crate) struct BchHeaderFeeder {
    client: BitcoindRpcClient,
    store: ChainStoreHandle,
    initial_headers: usize,
}

impl BchHeaderFeeder {
    pub(crate) fn new(client: BitcoindRpcClient, store: ChainStoreHandle) -> Self {
        Self {
            client,
            store,
            initial_headers: INITIAL_HEADERS,
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
                Ok(added) => debug!("Learned {added} Bitcoin Cash headers from BCHN"),
                Err(error) => warn!("Cannot follow BCHN's headers: {error}"),
            }
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
        Ok(added)
    }

    /// Learn `hash` and the headers below it back to a known one, from BCHN,
    /// and return how many were added. BCHN must know `hash`.
    pub(crate) async fn learn_from(&self, hash: BlockHash) -> Result<usize, String> {
        let mut hash = hash;
        let mut walked: Vec<Header> = Vec::new();
        let oldest_height = loop {
            if let Some(known) = self.known(&hash)? {
                if walked.is_empty() {
                    return Ok(0);
                }
                break known.height + 1;
            }
            if walked.len() == self.initial_headers {
                // A store with nothing to link to: anchor the walk on the
                // height BCHN gives its oldest header.
                let oldest = walked.last().expect("walked is not empty");
                let summary = self
                    .client
                    .get_block_header_summary(&oldest.block_hash().to_string())
                    .await
                    .map_err(|error| format!("getblockheader {}: {error}", oldest.block_hash()))?;
                let height = u32::try_from(summary.height)
                    .map_err(|_| format!("height {} is too high", summary.height))?;
                info!(
                    "Learning {} Bitcoin Cash headers back from BCHN's tip to height {height}",
                    walked.len()
                );
                break height;
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
        let mut updates = Vec::new();
        let mut current = tip;
        while updates.len() < MAX_INDEX_REWRITE {
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
    mut input: NotifyReceiver,
    output: NotifySender,
) {
    let feeder = BchHeaderFeeder::new(client, store);
    let mut learned: Option<BlockHash> = None;
    while let Some(command) = input.recv().await {
        if let NotifyCmd::SendToAll { template } = &command
            && let Ok(prev) = BlockHash::from_str(&template.previousblockhash)
            && learned != Some(prev)
        {
            match feeder.learn_from(prev).await {
                Ok(_) => learned = Some(prev),
                Err(error) => warn!("Cannot learn template parent {prev} from BCHN: {error}"),
            }
        }
        if output.send(command).await.is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests;
