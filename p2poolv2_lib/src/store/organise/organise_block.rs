// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::{Chain, Height, Store, TopResult};
use crate::node::request_response_handler::block_fetcher::FETCH_BATCH_SIZE;
use crate::store::prune_blocks::prune_height_for_tip;
use crate::store::writer::StoreError;
use tracing::debug;

impl Store {
    /// Promote candidates to confirmed if conditions are met.
    ///
    /// Strictly follows the candidate chain order: finds the
    /// contiguous prefix of candidates (from confirmed tip + 1) that
    /// have block and uncle data available, then extends or reorgs
    /// the confirmed chain accordingly.
    ///
    /// Returns the new confirmed height if it changed, or None if
    /// unchanged.
    pub(crate) fn organise_block(
        &self,
        batch: &mut rocksdb::WriteBatch,
    ) -> Result<Option<Height>, StoreError> {
        let top_confirmed = self.get_top_confirmed().map_err(|_| {
            StoreError::Database("organise_block called without a genesis block".into())
        })?;

        let candidate_tip_height = match self.get_top_candidate_height() {
            Ok(height) => height,
            Err(StoreError::NotFound(_)) => return Ok(None),
            Err(error) => return Err(error),
        };
        let prune_height = prune_height_for_tip(candidate_tip_height);

        let candidates =
            self.find_promotable_candidates(&top_confirmed, candidate_tip_height, prune_height)?;

        if !candidates.is_empty() {
            if self.should_extend_confirmed(
                &candidates,
                top_confirmed.height,
                top_confirmed.hash,
                prune_height,
            )? {
                return self.extend_confirmed(&candidates, batch);
            }

            if self.should_reorg_confirmed(&top_confirmed, &candidates) {
                return self.reorg_confirmed(&top_confirmed, &candidates, prune_height, batch);
            }
        }
        Ok(None)
    }

    /// Find the contiguous prefix of candidates that are eligible for
    /// promotion to confirmed.
    ///
    /// Blocks below the prune boundary (candidate_tip - PRUNE_DEPTH) do
    /// not require block body data -- they are promoted header-only.
    /// Blocks at or above the boundary require full block data.
    ///
    /// Returns an empty vec when no candidate chain exists or when the
    /// first block above the prune boundary lacks block data.
    fn find_promotable_candidates(
        &self,
        top_confirmed: &TopResult,
        candidate_tip_height: u32,
        prune_height: u32,
    ) -> Result<Chain, StoreError> {
        debug!(
            "Finding promotable candidates: top_confirmed {:?}, candidate_tip_height {}",
            top_confirmed, candidate_tip_height
        );

        let scan_limit = top_confirmed.height + 1 + FETCH_BATCH_SIZE as u32;
        let scan_end = std::cmp::min(scan_limit, candidate_tip_height);
        let all_candidates = self.get_candidates(top_confirmed.height + 1, scan_end)?;
        Ok(self.contiguous_candidates_with_block_data(&all_candidates, prune_height))
    }

    /// Return the contiguous prefix of candidates eligible for promotion.
    ///
    /// Below `prune_height` a block is promoted header-only: it only needs to
    /// be on the candidate chain, and its body is not required (its PoW was
    /// validated at header time).
    ///
    /// At or above `prune_height` -- the PPLNS zone -- a block must be on the
    /// candidate chain, `BlockValid` (chain-context validation complete), and
    /// have its body stored. This keeps a block from reaching the confirmed
    /// chain before it is fully validated. Stops at the first block that
    /// fails its tier's requirement.
    fn contiguous_candidates_with_block_data(
        &self,
        candidates: &Chain,
        prune_height: u32,
    ) -> Chain {
        let mut result = Vec::with_capacity(candidates.len());
        for (height, blockhash) in candidates {
            if *height >= prune_height {
                if !self.is_candidate_and_block_valid(blockhash) {
                    debug!(
                        "Candidate at height {} ({}) not Candidate+BlockValid, stopping promotion",
                        height, blockhash
                    );
                    return result;
                }
                if !self.share_block_exists(blockhash) {
                    debug!(
                        "Candidate at height {} ({}) missing block data, stopping promotion",
                        height, blockhash
                    );
                    return result;
                }
            } else if !self.is_candidate(blockhash) {
                debug!(
                    "Candidate at height {} ({}) not on candidate chain, stopping promotion",
                    height, blockhash
                );
                return result;
            }
            result.push((*height, *blockhash));
        }
        result
    }
}

#[cfg(test)]
mod tests;
