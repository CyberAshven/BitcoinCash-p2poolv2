// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Verify candidate Bitcoin Cash finds against BCHN without blocking share
//! confirmation. A header meeting its claimed target is only a candidate.

use crate::accounting::stats::metrics::MetricsHandle;
#[cfg(test)]
#[mockall_double::double]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
#[cfg(not(test))]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
use crate::store::found_block::FoundBlock;
use bitcoin::BlockHash;
use bitcoindrpc::BitcoindRpcClient;
use std::collections::HashSet;
use std::time::Duration;
use tokio::time::{MissedTickBehavior, interval, timeout};
use tracing::{debug, info, warn};

/// Bound work per round so unavailable and forged candidates cannot monopolize
/// BCHN. A rotating cursor lets later candidates progress on subsequent rounds.
const CANDIDATES_PER_ROUND: usize = 32;
const VERIFICATION_INTERVAL: Duration = Duration::from_secs(5);
const VERIFICATION_TIMEOUT: Duration = Duration::from_secs(5);

/// BCHN finalizes blocks this deep and will not reorganise below them
/// (`-maxreorgdepth`, default 10). A candidate at least this far below
/// BCHN's tip, with another block at its height, can never be accepted.
const BCHN_FINALIZATION_DEPTH: u64 = 10;

pub(crate) struct FoundBlockVerifier {
    client: BitcoindRpcClient,
    store: ChainStoreHandle,
    metrics: MetricsHandle,
    next_candidate: usize,
    initialized: bool,
    /// Existing records and candidates seen while syncing must not reset the
    /// runtime effort counter when a historical block is finally verified.
    historical_candidates: HashSet<BlockHash>,
    /// Candidates BCHN can no longer accept, skipped until a restart. A
    /// share can claim a target its BCH header does not meet on BCH, so
    /// without this such candidates would be queried forever.
    settled: HashSet<BlockHash>,
    /// BCHN's tip height for this round, read at most once and only when a
    /// candidate's height holds another block.
    round_tip: Option<Option<u64>>,
}

impl FoundBlockVerifier {
    pub(crate) fn new(
        client: BitcoindRpcClient,
        store: ChainStoreHandle,
        metrics: MetricsHandle,
    ) -> Self {
        Self {
            client,
            store,
            metrics,
            next_candidate: 0,
            initialized: false,
            historical_candidates: HashSet::with_capacity(CANDIDATES_PER_ROUND),
            settled: HashSet::with_capacity(CANDIDATES_PER_ROUND),
            round_tip: None,
        }
    }

    /// Retry unverified candidates until BCHN reports them in its best chain.
    /// RPC and local store failures defer accounting; neither changes share
    /// validity nor stops the mining or organisation workers.
    pub(crate) async fn run(mut self) {
        let mut timer = interval(VERIFICATION_INTERVAL);
        timer.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            timer.tick().await;
            self.verify_round().await;
        }
    }

    async fn verify_round(&mut self) {
        let candidates: Vec<FoundBlock> = match self.store.get_candidate_found_blocks() {
            Ok(candidates) => candidates
                .into_iter()
                .filter(|candidate| !self.settled.contains(&candidate.bitcoin_blockhash))
                .collect(),
            Err(error) => {
                warn!("Cannot read candidate Bitcoin Cash finds: {error}");
                return;
            }
        };
        if !self.initialized || !self.store.is_current() {
            self.historical_candidates.extend(
                candidates
                    .iter()
                    .map(|candidate| candidate.bitcoin_blockhash),
            );
        }
        self.initialized = true;
        if candidates.is_empty() {
            self.next_candidate = 0;
            return;
        }
        self.round_tip = None;
        let start = self.next_candidate % candidates.len();
        let count = candidates.len().min(CANDIDATES_PER_ROUND);
        for offset in 0..count {
            let candidate = &candidates[(start + offset) % candidates.len()];
            self.verify_candidate(candidate).await;
        }
        self.next_candidate = (start + count) % candidates.len();
    }

    /// BCHN's best chain height, or `None` when BCHN cannot say. Without it
    /// no candidate is settled this round.
    async fn tip_height(&mut self) -> Option<u64> {
        if let Some(tip) = self.round_tip {
            return tip;
        }
        let tip = match timeout(VERIFICATION_TIMEOUT, self.client.getblockchaininfo()).await {
            Ok(Ok(info)) => info.blocks,
            _ => None,
        };
        self.round_tip = Some(tip);
        tip
    }

    /// Trust the node's chain hash at the declared height, never the header's
    /// own nBits, and persist acceptance before changing any found metrics.
    async fn verify_candidate(&mut self, candidate: &FoundBlock) {
        let block_hash = candidate.bitcoin_blockhash;
        let reported = timeout(
            VERIFICATION_TIMEOUT,
            self.client.get_block_hash(candidate.bitcoin_height),
        )
        .await;
        match reported {
            Ok(Ok(reported))
                if reported
                    .parse::<BlockHash>()
                    .is_ok_and(|reported| reported == block_hash) =>
            {
                match self
                    .store
                    .record_accepted_found_block(candidate.clone())
                    .await
                {
                    Ok(true) => {
                        info!("BCHN confirmed pool block {block_hash} in its chain");
                        let historical = self.historical_candidates.remove(&block_hash);
                        if !historical && self.store.is_current() {
                            let _ = self.metrics.reset_block_effort().await;
                        }
                    }
                    Ok(false) => {}
                    Err(error) => {
                        warn!("Cannot record BCHN acceptance of pool block {block_hash}: {error}")
                    }
                }
            }
            Ok(Ok(_)) => {
                let final_height = candidate
                    .bitcoin_height
                    .saturating_add(BCHN_FINALIZATION_DEPTH);
                if self
                    .tip_height()
                    .await
                    .is_some_and(|tip| tip >= final_height)
                {
                    info!(
                        "Candidate pool block {block_hash} lost its height to a final BCHN block; \
                         no longer checking it"
                    );
                    self.settled.insert(block_hash);
                } else {
                    debug!("Candidate pool block {block_hash} is not in BCHN's chain");
                }
            }
            Ok(Err(error)) => {
                debug!("BCHN cannot confirm candidate pool block {block_hash} yet: {error}");
            }
            Err(_) => warn!("BCHN timed out checking candidate pool block {block_hash}"),
        }
    }
}

#[cfg(test)]
mod tests;
