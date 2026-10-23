// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::store::{
    ColumnFamily, Store,
    block_tx_metadata::{BlockMetadata, ChainMembership, Status},
    transaction_store::ConfirmationOverlay,
    writer::StoreError,
};
use bitcoin::{
    BlockHash,
    consensus::{self, encode},
};
use std::collections::{HashMap, HashSet};
use tracing::debug;

use super::{Chain, Height, TopResult, height_to_key_with_suffix};

const CONFIRMED_SUFFIX: &str = ":f";
const TOP_CONFIRMED_KEY: &str = "meta:top_confirmed_height";

impl Store {
    /// Check that every block in the list has its body stored and
    /// that all uncles referenced by each block also have their
    /// bodies stored. Skip blocks below `prune_height` in the check
    /// as they are organised by header only.
    ///
    /// Returns `Ok(false)` (with a debug log) on the first missing block
    /// body or uncle body. Returns `Err` if block metadata is missing or
    /// has no expected_height. Used by `should_extend_confirmed` and
    /// `reorg_confirmed` to ensure PPLNS can resolve all uncle data
    /// before a block is promoted, and by the BlockReceiver to hold a block
    /// back until its parent's body is stored.
    pub(crate) fn all_block_and_uncle_data_available(
        &self,
        blockhashes: &[BlockHash],
        prune_height: u32,
    ) -> Result<bool, StoreError> {
        for blockhash in blockhashes {
            let block_height = self.get_block_height_from_metadata(blockhash)?;
            if block_height < prune_height {
                continue;
            }
            if !self.share_block_exists(blockhash) {
                debug!("Block {blockhash} missing block data");
                return Ok(false);
            }
            let header = self.get_share_header(blockhash)?.ok_or_else(|| {
                StoreError::NotFound(format!(
                    "Header missing for {blockhash} in uncle data check"
                ))
            })?;
            for uncle in &header.uncles {
                let uncle_height = self.get_block_height_from_metadata(uncle)?;
                if uncle_height < prune_height {
                    continue;
                }
                if !self.share_block_exists(uncle) {
                    debug!("Uncle {uncle} of block {blockhash} missing block data");
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    /// Check that every in-prune-depth block in the list is `BlockValid`.
    ///
    /// Mirrors the extend path's promotion gate
    /// (`contiguous_candidates_with_block_data`): within the prune depth
    /// (`height >= prune_height`) a block must have passed validation and been
    /// marked `BlockValid` before it can be confirmed. Blocks below
    /// `prune_height` are skipped -- they are promoted header-only (ASERT/PoW).
    ///
    /// Checks status only (not candidate membership): `reorg_confirmed` derives
    /// its branch by walking parent pointers, and a block being reorged in is
    /// about to become confirmed, so the gate is about validation, not chain
    /// membership.
    ///
    /// Already-confirmed blocks (the fork point) are skipped: they were
    /// validated when confirmed, and genesis and below-zone confirmed blocks are
    /// `HeaderValid`, not `BlockValid`. The gate applies only to the blocks the
    /// reorg newly confirms.
    ///
    /// Returns `Ok(false)` (with a debug log) on the first in-depth block that
    /// is not `BlockValid`. Used by `reorg_confirmed` so a deep reorg cannot
    /// confirm an unvalidated fork block that the gated `candidates` list did
    /// not cover.
    ///
    /// Iterates the blockhashes asked for rather than the pairs
    /// `get_block_metadata_batch` returns, and errors on any it did not.
    pub(super) fn all_in_zone_blocks_block_valid(
        &self,
        blockhashes: &[BlockHash],
        prune_height: u32,
    ) -> Result<bool, StoreError> {
        let metadata_by_hash: HashMap<BlockHash, BlockMetadata> = self
            .get_block_metadata_batch(blockhashes)?
            .into_iter()
            .collect();
        for blockhash in blockhashes {
            let metadata = metadata_by_hash.get(blockhash).ok_or_else(|| {
                StoreError::NotFound(format!(
                    "Block {blockhash} has no metadata in the reorg validation gate"
                ))
            })?;
            if metadata.chain == ChainMembership::Confirmed {
                continue;
            }
            let block_height = metadata.expected_height.ok_or_else(|| {
                StoreError::NotFound(format!("Block {blockhash} has no expected_height"))
            })?;
            if block_height < prune_height {
                continue;
            }
            if metadata.status != Status::BlockValid {
                debug!("Reorg block {blockhash} in prune depth is not BlockValid");
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Check if the candidate chain should reorg the confirmed chain.
    ///
    /// Returns true when the last candidate has more cumulative work
    /// than the current top confirmed AND the candidates are not a
    /// simple extension of the confirmed chain.
    pub(super) fn should_reorg_confirmed(
        &self,
        top_confirmed: &TopResult,
        candidates: &Chain,
    ) -> bool {
        let Some((_, last_hash)) = candidates.last() else {
            return false;
        };

        let Ok(last_metadata) = self.get_block_metadata(last_hash) else {
            return false;
        };

        // Last candidate must have more work than top confirmed
        if last_metadata.chain_work <= top_confirmed.work {
            debug!(
                "Candidate chain work {:?} <= confirmed work {:?}, no reorg",
                last_metadata.chain_work, top_confirmed.work
            );
            return false;
        }

        // Candidates must NOT be extending the confirmed chain
        let (first_height, first_hash) = &candidates[0];
        let Ok(Some(first_header)) = self.get_share_header(first_hash) else {
            return false;
        };

        if first_header.prev_share_blockhash == top_confirmed.hash
            && top_confirmed.height + 1 == *first_height
        {
            debug!("Candidates extend confirmed chain, no reorg needed");
            return false;
        }

        debug!(
            "Confirmed reorg needed: candidate work {:?} > confirmed work {:?}",
            last_metadata.chain_work, top_confirmed.work
        );
        true
    }

    /// Reorg confirmed chain to include the fork branch from the candidate chain.
    ///
    /// Walks from the candidate tip backward to the first confirmed ancestor
    /// (fork point), replaces the confirmed entries from fork point to top
    /// with the new branch, and cleans up the candidate index.
    ///
    /// Each fork block's prevouts are re-checked against the confirmed state
    /// this reorg produces before it is confirmed: the reorged-out suffix stops
    /// counting as a confirmed prevout source, its spends are removed, and
    /// spends by earlier fork blocks are applied. Only the leading run that
    /// passes is confirmed and the first failing block is marked Invalid. The
    /// re-check and the work comparison run before any rewind, so a fork whose
    /// surviving prefix no longer outweighs the confirmed chain leaves it
    /// untouched rather than regressing it.
    pub(super) fn reorg_confirmed(
        &self,
        top_confirmed: &TopResult,
        candidates: &Chain,
        prune_height: u32,
        batch: &mut rocksdb::WriteBatch,
    ) -> Result<Option<Height>, StoreError> {
        let (_, tip_hash) = candidates
            .last()
            .ok_or_else(|| StoreError::NotFound("Empty candidates in reorg_confirmed".into()))?;

        // Walk from candidate tip backward to the confirmed chain
        let fork_branch = self
            .get_branch_to_chain(tip_hash, |h| self.is_confirmed(h))?
            .ok_or_else(|| {
                StoreError::NotFound(format!(
                    "Fork point for {tip_hash} to reorg confirmed chain not found."
                ))
            })?;
        let fork_point = fork_branch.front().ok_or_else(|| {
            StoreError::NotFound("Empty branch returned from get_branch_to_chain.".into())
        })?;

        // Do not reorg if any block above the prune boundary lacks body data.
        let fork_hashes: Vec<BlockHash> = fork_branch.iter().copied().collect();
        if !self.all_block_and_uncle_data_available(&fork_hashes, prune_height)? {
            debug!("Reorg skipped: block or uncle missing block data");
            return Ok(None);
        }

        // Do not reorg if any block above the prune boundary is not BlockValid.
        if !self.all_in_zone_blocks_block_valid(&fork_hashes, prune_height)? {
            debug!("Reorg skipped: in-zone fork block not BlockValid");
            return Ok(None);
        }

        let reorged_out_chain = self.get_confirmed_chain(fork_point, Some(top_confirmed))?;
        let mut overlay = self.reorg_out_overlay(&reorged_out_chain, fork_point)?;

        // The fork branch heads with the already-confirmed fork point; the rest
        // are the blocks this reorg would newly confirm, in confirmation order.
        let fork_blocks_with_heights = self.blocks_with_heights(&fork_hashes)?;
        let (fork_block_and_height, new_blocks_with_heights) =
            fork_blocks_with_heights.split_first().ok_or_else(|| {
                StoreError::NotFound("Empty branch returned from get_branch_to_chain.".into())
            })?;

        let (prefix_len, first_invalid) =
            self.confirmable_prefix(new_blocks_with_heights, &mut overlay)?;
        let confirmable = &new_blocks_with_heights[..prefix_len];

        // Reorg only if the fork blocks that survive the re-check still outweigh
        // the confirmed chain, so a truncated fork never regresses the tip.
        let (_, prefix_tip) = confirmable.last().unwrap_or(fork_block_and_height);
        if self.get_block_metadata(prefix_tip)?.chain_work <= top_confirmed.work {
            debug!("Reorg skipped: validated fork prefix does not outweigh confirmed chain");
            return Ok(None);
        }

        self.rewind_confirmed_entries(&reorged_out_chain, batch)?;

        // Rewrite the fork point (the rewind removed it) followed by the
        // validated prefix of the fork branch.
        let mut to_confirm_list = Vec::with_capacity(prefix_len + 1);
        to_confirm_list.push(*fork_block_and_height);
        to_confirm_list.extend_from_slice(confirmable);
        let new_top_height = self.confirm_blocks(&to_confirm_list, batch)?;

        if let Some(invalid_hash) = first_invalid {
            debug!("Reorg stopped at {invalid_hash}: prevout re-check failed, marking Invalid");
            self.mark_invalid(&invalid_hash, new_top_height, batch)?;
        }

        tracing::info!("Chain reorg completed to height {new_top_height:?}");
        Ok(new_top_height)
    }

    /// Describe what a reorg removes from the confirmed chain: the blocks that
    /// leave it and the spends they unspend.
    ///
    /// `reorged_out_chain` heads with the fork point, which stays confirmed (it
    /// is rewritten after the rewind), so the fork point is excluded.
    fn reorg_out_overlay(
        &self,
        reorged_out_chain: &Chain,
        fork_point: &BlockHash,
    ) -> Result<ConfirmationOverlay, StoreError> {
        let mut overlay = ConfirmationOverlay {
            removed_blockhashes: HashSet::with_capacity(reorged_out_chain.len()),
            ..Default::default()
        };
        for (_, unconfirm) in reorged_out_chain {
            if unconfirm != fork_point {
                overlay.removed_blockhashes.insert(*unconfirm);
                self.accumulate_spent_outpoints(unconfirm, &mut overlay.removed_spends)?;
            }
        }
        Ok(overlay)
    }

    /// Pair each blockhash with its expected height, preserving order.
    ///
    /// Errors when a block has no metadata or no expected height: both are
    /// required to place it on the confirmed chain.
    fn blocks_with_heights(
        &self,
        blockhashes: &[BlockHash],
    ) -> Result<Vec<(Height, BlockHash)>, StoreError> {
        let mut blocks = Vec::with_capacity(blockhashes.len());
        for blockhash in blockhashes {
            let height = self
                .get_block_metadata(blockhash)?
                .expected_height
                .ok_or_else(|| {
                    StoreError::NotFound(format!(
                        "Block {blockhash} metadata missing expected_height for confirmation"
                    ))
                })?;
            blocks.push((height, *blockhash));
        }
        Ok(blocks)
    }

    /// Take `reorged_out_chain` off the confirmed chain: drop each confirmed
    /// index entry, remove the spends its transactions recorded, and clear its
    /// chain membership.
    ///
    /// Membership drops to `None` rather than `Candidate` because `Candidate`
    /// is reserved for blocks on the candidate chain; a block that is later
    /// reorged back into the candidate chain is marked then.
    fn rewind_confirmed_entries(
        &self,
        reorged_out_chain: &Chain,
        batch: &mut rocksdb::WriteBatch,
    ) -> Result<(), StoreError> {
        for (height, unconfirm) in reorged_out_chain {
            self.delete_confirmed_entry(*height, batch);
            let reorged_out_txs = self.get_txs_by_blockhash_index(unconfirm)?;
            self.remove_spends_for_block(&reorged_out_txs, batch);
            let mut metadata = self.get_block_metadata(unconfirm)?;
            metadata.chain = ChainMembership::None;
            self.update_block_metadata(unconfirm, &metadata, batch)?;
        }
        Ok(())
    }

    /// Confirm `blocks` in order: write each confirmed index entry, mark its
    /// metadata Confirmed, record any bitcoin blocks it or its uncles
    /// carry, and advance the top confirmed height to the last of
    /// them. Returns the new top confirmed height, or None when `blocks` is
    /// empty (nothing was confirmed, so the top is left alone).
    ///
    /// Shared by the extend and reorg paths, which differ only in which blocks
    /// they hand over.
    fn confirm_blocks(
        &self,
        blocks: &[(Height, BlockHash)],
        batch: &mut rocksdb::WriteBatch,
    ) -> Result<Option<Height>, StoreError> {
        let mut new_top_height = None;
        for (height, blockhash) in blocks {
            self.put_confirmed_entry(*height, blockhash, batch)?;
            let mut metadata = self.get_block_metadata(blockhash)?;
            metadata.chain = ChainMembership::Confirmed;
            self.update_block_metadata(blockhash, &metadata, batch)?;
            self.record_found_blocks(blockhash, batch)?;
            new_top_height = Some(*height);
        }
        if let Some(top_height) = new_top_height {
            self.set_top_confirmed_height(top_height, batch);
        }
        Ok(new_top_height)
    }

    /// Write a confirmed index entry and record every spend that the
    /// block's transactions make into `SpendsIndex`.
    ///
    /// This is the single point where "a block just became confirmed" is
    /// expressed. `SpendsIndex` presence therefore means exactly
    /// "spent by a transaction on the confirmed sharechain".
    ///
    /// The tx list is loaded from the committed store via
    /// `get_txs_by_blockhash_index`. Callers must ensure the
    /// block's tx data has already been committed in a prior batch - the
    /// read against a pending `WriteBatch` will not see its contents.
    ///
    /// For blocks with body data, fetches transactions and records
    /// spends. For prune-zone blocks without body data, only writes the
    /// confirmed height index (SpendsIndex not needed because their
    /// outputs are unspendable via coinbase_root_height check).
    pub(super) fn put_confirmed_entry(
        &self,
        height: Height,
        blockhash: &BlockHash,
        batch: &mut rocksdb::WriteBatch,
    ) -> Result<(), StoreError> {
        let block_height_cf = self.db.cf_handle(&ColumnFamily::BlockHeight).unwrap();
        let key = height_to_key_with_suffix(height, CONFIRMED_SUFFIX);
        let serialized = consensus::serialize(blockhash);
        batch.put_cf(&block_height_cf, key, serialized);

        // If block body data exists, we can fetch transactions and update the spends index.
        // (Blocks promoted header-only in the prune zone will not have body data.)
        if self.share_block_exists(blockhash) {
            let txs = self.get_txs_by_blockhash_index(blockhash)?;
            self.add_spends_for_block(&txs, batch)?;
        }
        Ok(())
    }

    /// Delete a confirmed index entry from the batch.
    fn delete_confirmed_entry(&self, height: Height, batch: &mut rocksdb::WriteBatch) {
        let block_height_cf = self.db.cf_handle(&ColumnFamily::BlockHeight).unwrap();
        let key = height_to_key_with_suffix(height, CONFIRMED_SUFFIX);
        batch.delete_cf(&block_height_cf, key);
    }

    /// Set top confirmed height.
    /// The required height checks are already made in make_confirmed.
    pub(crate) fn set_top_confirmed_height(&self, height: Height, batch: &mut rocksdb::WriteBatch) {
        let block_height_cf = self.db.cf_handle(&ColumnFamily::BlockHeight).unwrap();
        let serialized_height = consensus::serialize(&height);
        batch.put_cf(
            &block_height_cf,
            TOP_CONFIRMED_KEY.as_bytes(),
            serialized_height,
        );
    }

    /// Get top confirmed height from confirmed index
    pub fn get_top_confirmed_height(&self) -> Result<Height, StoreError> {
        let block_height_cf = self.db.cf_handle(&ColumnFamily::BlockHeight).unwrap();
        match self
            .db
            .get_cf(&block_height_cf, TOP_CONFIRMED_KEY.as_bytes())
        {
            Ok(Some(height_bytes)) => Ok(encode::deserialize(&height_bytes)?),
            Ok(None) => Err(StoreError::NotFound("No confirmed found at top".into())),
            Err(e) => Err(e.into()),
        }
    }

    /// Get top confirmed after looking up top confirmed height
    pub(crate) fn get_top_confirmed(&self) -> Result<TopResult, StoreError> {
        let height = self.get_top_confirmed_height()?;
        let hash = self.get_confirmed_at_height(height)?;
        let metadata = self.get_block_metadata(&hash)?;
        Ok(TopResult {
            hash,
            height,
            work: metadata.chain_work,
        })
    }

    /// Get list of (height, blockhash) pairs from given blockhash up to top confirmed.
    /// The blockhash is known to be on the confirmed chain.
    pub(crate) fn get_confirmed_chain(
        &self,
        blockhash: &BlockHash,
        top_confirmed: Option<&TopResult>,
    ) -> Result<Chain, StoreError> {
        let Ok(metadata) = self.get_block_metadata(blockhash) else {
            return Err(StoreError::NotFound(
                "Block metadata not found for branch point".into(),
            ));
        };
        let Some(height) = metadata.expected_height else {
            return Err(StoreError::NotFound(
                "Block metadata doesn't have an expected height".into(),
            ));
        };
        let Some(top) = top_confirmed else {
            return Err(StoreError::NotFound(
                "No top confirmed height found when fetching confirmed chain".into(),
            ));
        };
        self.get_confirmed(height, top.height)
    }

    /// Fetch a list of (height, blockhash) pairs on the confirmed chain between
    /// the given heights, inclusive.
    pub(crate) fn get_confirmed(&self, from: Height, to: Height) -> Result<Chain, StoreError> {
        self.get_chain_range(from, to, CONFIRMED_SUFFIX)
    }

    /// Add blockhash as a confirmed at provided height.
    ///
    /// Only adds to the confirmed index if the height is one more than the
    /// current top confirmed height (or if there is no top yet).
    ///
    /// Also updates the metadata status to Confirmed.
    ///
    /// Returns error if height check fails.
    pub(crate) fn append_to_confirmed(
        &self,
        blockhash: &BlockHash,
        height: Height,
        metadata: &mut BlockMetadata,
        batch: &mut rocksdb::WriteBatch,
    ) -> Result<(), StoreError> {
        let current_top = match self.get_top_confirmed_height() {
            Ok(top) => top,
            Err(StoreError::NotFound(_)) => 0,
            Err(e) => return Err(e),
        };

        // Only add if this is the first entry or height is exactly one more than current top
        // Or if it is the first confirmation
        if height.saturating_sub(current_top) != 1 && !(height == 0 && current_top == 0) {
            return Err(StoreError::Database("Incorrect confirmation".into()));
        }

        self.put_confirmed_entry(height, blockhash, batch)?;
        self.set_top_confirmed_height(height, batch);

        metadata.chain = ChainMembership::Confirmed;
        self.update_block_metadata(blockhash, metadata, batch)?;
        Ok(())
    }

    /// Get the confirmed blockhash at a specific height
    pub fn get_confirmed_at_height(&self, height: Height) -> Result<BlockHash, StoreError> {
        let block_height_cf = self.db.cf_handle(&ColumnFamily::BlockHeight).unwrap();
        let key = height_to_key_with_suffix(height, CONFIRMED_SUFFIX);

        match self.db.get_cf::<&[u8]>(&block_height_cf, key.as_ref()) {
            Ok(Some(blockhash_bytes)) => Ok(encode::deserialize(&blockhash_bytes)?),
            Ok(None) => Err(StoreError::NotFound(format!(
                "No confirmed found at height {height}"
            ))),
            Err(e) => Err(e.into()),
        }
    }

    /// Check if a blockhash is on the confirmed chain.
    pub fn is_confirmed(&self, blockhash: &BlockHash) -> bool {
        self.get_block_metadata(blockhash)
            .map(|m| m.chain == ChainMembership::Confirmed)
            .unwrap_or(false)
    }

    /// Check if the confirmed chain can be extended by the local candidate chain.
    ///
    /// Returns true when the first candidate is a child of the top
    /// confirmed AND all candidates at or above `prune_height` (plus
    /// their uncles) have block body data available in the store.
    /// Candidates below `prune_height` are promoted header-only since
    /// their PoW was validated at header sync time and their outputs
    /// are unspendable.
    pub(super) fn should_extend_confirmed(
        &self,
        candidates: &Chain,
        top_confirmed_height: Height,
        top_confirmed_hash: BlockHash,
        prune_height: u32,
    ) -> Result<bool, StoreError> {
        debug!(
            "top confirmed height {}, top confirmed hash {}",
            top_confirmed_height, top_confirmed_hash
        );

        if candidates.is_empty() {
            debug!("No candidates found");
            return Ok(false);
        }

        let Some(first_candidate_header) = self.get_share_header(&candidates[0].1)? else {
            return Err(StoreError::NotFound(
                "No candidate header found in extending confirmed".into(),
            ));
        };

        debug!("First candidate header {:?}", first_candidate_header);

        if first_candidate_header.prev_share_blockhash != top_confirmed_hash
            || top_confirmed_height + 1 != candidates[0].0
        {
            return Ok(false);
        }

        let candidate_hashes: Vec<BlockHash> = candidates.iter().map(|(_, hash)| *hash).collect();
        self.all_block_and_uncle_data_available(&candidate_hashes, prune_height)
    }

    /// Promote candidates to confirmed.
    ///
    /// Each candidate's prevouts are re-checked against the confirmed state
    /// this batch is building before it is confirmed, because a candidate was
    /// prevout-validated at ingest when a later candidate's spend was not yet
    /// confirmed. Only the leading run that passes is confirmed; the first
    /// candidate that fails is marked Invalid (rebuilding the candidate chain)
    /// and nothing above it is promoted. Extending is append-only, so
    /// confirming a shorter prefix never regresses the confirmed chain.
    ///
    /// Writes each confirmed entry to the confirmed index and updates metadata
    /// to Confirmed status. The candidate chain is left intact so it coexists
    /// with the confirmed chain. Returns the new top confirmed height, or None
    /// when the first candidate itself failed the re-check.
    pub(super) fn extend_confirmed(
        &self,
        candidates: &[(Height, BlockHash)],
        batch: &mut rocksdb::WriteBatch,
    ) -> Result<Option<Height>, StoreError> {
        // An extend removes nothing from the confirmed chain, so only the
        // in-batch spends accumulate.
        let mut overlay = ConfirmationOverlay {
            spent_in_batch: HashSet::with_capacity(candidates.len()),
            ..Default::default()
        };
        let (prefix_len, first_invalid) = self.confirmable_prefix(candidates, &mut overlay)?;
        let new_top_height = self.confirm_blocks(&candidates[..prefix_len], batch)?;

        if let Some(invalid_hash) = first_invalid {
            debug!("Extend stopped at {invalid_hash}: prevout re-check failed, marking Invalid");
            self.mark_invalid(&invalid_hash, new_top_height, batch)?;
        }

        Ok(new_top_height)
    }
}

#[cfg(test)]
mod tests;
