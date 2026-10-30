// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Keep the known Bitcoin Cash headers in step with this node's own BCHN.
//!
//! Every few seconds the feeder asks BCHN for its best block. When the store
//! does not know it, the feeder walks back with `getblockheader` until it
//! reaches a known header and stores the headers it walked, oldest first,
//! each one height above its parent. A reorg is the same walk: the new
//! branch is added and the old one stays known, since a share on a block
//! that lost an orphan race is still placed on the chain.
//!
//! On a node whose store knows no header yet, the walk stops after
//! [`INITIAL_HEADERS`] blocks and asks BCHN for the height of the oldest.
//! These headers come from the node's own BCHN, which has validated them, so
//! they are not run through `BchHeaderRules::check`; headers from peers are.
//!
//! Part of mining on the current chain (V16,
//! `docs/sharechain/current-chain.md`). Nothing reads the store yet, so this
//! changes no share-chain rule.

use crate::shares::bch_headers::KnownBchHeader;
#[cfg(test)]
#[mockall_double::double]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
#[cfg(not(test))]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
use bitcoin::BlockHash;
use bitcoin::block::Header;
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

    /// Learn the headers from BCHN's best block back to a known one, and
    /// return how many were added.
    async fn sync_once(&self) -> Result<usize, String> {
        let best = self
            .client
            .get_best_block_hash()
            .await
            .map_err(|error| format!("getbestblockhash: {error}"))?;
        let mut hash =
            BlockHash::from_str(&best).map_err(|error| format!("best block hash: {error}"))?;
        let mut walked: Vec<Header> = Vec::new();
        let parent_height = loop {
            if let Some(known) = self.known(&hash)? {
                if walked.is_empty() {
                    return Ok(0);
                }
                break known.height;
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
                break height
                    .checked_sub(1)
                    .ok_or_else(|| "the walk reached genesis".to_string())?;
            }
            let header = self
                .client
                .get_block_header(&hash.to_string())
                .await
                .map_err(|error| format!("getblockheader {hash}: {error}"))?;
            hash = header.prev_blockhash;
            walked.push(header);
        };
        let added = walked.len();
        let headers = walked
            .into_iter()
            .rev()
            .zip(1..)
            .map(|(header, offset)| KnownBchHeader {
                height: parent_height + offset,
                header,
            })
            .collect();
        self.store
            .add_bch_headers(headers)
            .await
            .map_err(|error| format!("storing headers: {error}"))?;
        Ok(added)
    }

    fn known(&self, hash: &BlockHash) -> Result<Option<KnownBchHeader>, String> {
        self.store
            .get_bch_header(hash)
            .map_err(|error| format!("reading header {hash}: {error}"))
    }
}

#[cfg(test)]
mod tests;
