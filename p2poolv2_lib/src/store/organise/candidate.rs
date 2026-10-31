// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::{
    shares::share_block::ShareHeader,
    store::{
        ColumnFamily, Store,
        block_tx_metadata::{BlockMetadata, ChainMembership, Status},
        writer::StoreError,
    },
};
use bitcoin::{
    BlockHash,
    consensus::{self, encode},
};
use tracing::debug;

use std::collections::{HashMap, HashSet, VecDeque};

use super::{Chain, Height, TopResult, height_to_key_with_suffix};
use crate::store::prune_blocks::prune_height_for_tip;

const CANDIDATE_SUFFIX: &str = ":c";
const TOP_CANDIDATE_KEY: &str = "meta:top_candidate_height";

/// Height span scanned per batch by `get_candidate_blocks_needing_validation`.
///
/// Bounds the per-batch working set (blockhashes plus their metadata read) so a
/// restart with the confirmed tip far below the candidate tip does not load the
/// entire unconfirmed range up front. One batch usually already yields a full
/// result window.
const NEEDING_VALIDATION_SCAN_BATCH_HEIGHTS: u32 = 512;

impl Store {
    /// Increment top candidate key if height is one more than current height
    ///
    /// Only updates top if it is more than one higher.
    fn increment_top_candidate(
        &self,
        height: Height,
        batch: &mut rocksdb::WriteBatch,
    ) -> Result<Height, StoreError> {
        let block_height_cf = self.db.cf_handle(&ColumnFamily::BlockHeight).unwrap();

        let use_height = match self.get_top_candidate_height() {
            Ok(current_top_height) => {
                if height.saturating_sub(current_top_height) == 1 {
                    height
                } else {
                    return Err(StoreError::Database("Mismatch in top height".into()));
                }
            }
            Err(StoreError::NotFound(_reason)) => height, // Use share height if no candidate top present
            Err(e) => return Err(e),
        };
        let serialized_height = consensus::serialize(&use_height);
        batch.put_cf(
            &block_height_cf,
            TOP_CANDIDATE_KEY.as_bytes(),
            serialized_height,
        );
        Ok(use_height)
    }

    /// Directly set top candidate height without consecutive-height validation.
    ///
    /// Used by `reorg_candidate` which computes the correct final height
    /// locally instead of reading stale DB state within a single WriteBatch.
    pub(crate) fn set_top_candidate_height(&self, height: Height, batch: &mut rocksdb::WriteBatch) {
        let block_height_cf = self.db.cf_handle(&ColumnFamily::BlockHeight).unwrap();
        let serialized_height = consensus::serialize(&height);
        batch.put_cf(
            &block_height_cf,
            TOP_CANDIDATE_KEY.as_bytes(),
            serialized_height,
        );
    }

    /// Get top candidate height from candidates index
    pub fn get_top_candidate_height(&self) -> Result<Height, StoreError> {
        let block_height_cf = self.db.cf_handle(&ColumnFamily::BlockHeight).unwrap();
        match self
            .db
            .get_cf(&block_height_cf, TOP_CANDIDATE_KEY.as_bytes())
        {
            Ok(Some(height_bytes)) => Ok(encode::deserialize(&height_bytes)?),
            Ok(None) => Err(StoreError::NotFound("No candidate found at top".into())),
            Err(e) => Err(e.into()),
        }
    }

    fn put_candidate_entry(
        &self,
        height: Height,
        blockhash: &BlockHash,
        batch: &mut rocksdb::WriteBatch,
    ) {
        let block_height_cf = self.db.cf_handle(&ColumnFamily::BlockHeight).unwrap();
        let key = height_to_key_with_suffix(height, CANDIDATE_SUFFIX);
        let serialized = consensus::serialize(blockhash);
        batch.put_cf(&block_height_cf, key, serialized);
    }

    /// Delete a candidate index entry from the batch.
    pub(crate) fn delete_candidate_entry(&self, height: Height, batch: &mut rocksdb::WriteBatch) {
        let block_height_cf = self.db.cf_handle(&ColumnFamily::BlockHeight).unwrap();
        let key = height_to_key_with_suffix(height, CANDIDATE_SUFFIX);
        batch.delete_cf(&block_height_cf, key);
    }

    /// Get top candidate after looking up top candidate height
    pub(crate) fn get_top_candidate(&self) -> Result<TopResult, StoreError> {
        let height = self.get_top_candidate_height()?;
        let hash = self.get_candidate_at_height(height)?;
        let metadata = self.get_block_metadata(&hash)?;
        Ok(TopResult {
            hash,
            height,
            work: metadata.chain_work,
        })
    }

    /// Add blockhash as a candidate at provided height.
    ///
    /// Also Updates the metadata status to Candidate.
    pub(crate) fn append_to_candidates(
        &self,
        blockhash: &BlockHash,
        height: Height,
        metadata: &mut BlockMetadata,
        batch: &mut rocksdb::WriteBatch,
    ) -> Result<Option<Height>, StoreError> {
        let block_height_cf = self.db.cf_handle(&ColumnFamily::BlockHeight).unwrap();
        let key = height_to_key_with_suffix(height, CANDIDATE_SUFFIX);

        let serialized_blockhash = consensus::serialize(blockhash);
        batch.put_cf(&block_height_cf, key, serialized_blockhash);

        self.increment_top_candidate(height, batch)?;

        metadata.chain = ChainMembership::Candidate;
        self.update_block_metadata(blockhash, metadata, batch)?;
        Ok(Some(height))
    }

    /// Get list of (height, blockhash) pairs from given blockhash up to top candidate.
    /// The blockhash is known to be on the candidates chain.
    pub(crate) fn get_candidates_chain(
        &self,
        blockhash: &BlockHash,
        top_candidate: Option<&TopResult>,
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
        let Some(top) = top_candidate else {
            return Err(StoreError::NotFound(
                "No top candidate height found when reorging candidate chain".into(),
            ));
        };
        self.get_candidates(height, top.height)
    }

    /// Fetch a list of (height, blockhash) pairs on the candidates chain between
    /// the given heights, inclusive.
    pub(crate) fn get_candidates(&self, from: Height, to: Height) -> Result<Chain, StoreError> {
        debug!("Get candidates from {from} to {to}");
        self.get_chain_range(from, to, CANDIDATE_SUFFIX)
    }

    /// Get the candidate blockhash at a specific height
    pub fn get_candidate_at_height(&self, height: Height) -> Result<BlockHash, StoreError> {
        let block_height_cf = self.db.cf_handle(&ColumnFamily::BlockHeight).unwrap();
        let key = height_to_key_with_suffix(height, CANDIDATE_SUFFIX);

        match self.db.get_cf::<&[u8]>(&block_height_cf, key.as_ref()) {
            Ok(Some(blockhash_bytes)) => Ok(encode::deserialize(&blockhash_bytes)?),
            Ok(None) => Err(StoreError::NotFound(format!(
                "No candidate found at height {height}"
            ))),
            Err(e) => Err(e.into()),
        }
    }

    /// Return blockhashes of all shares between confirmed top and
    /// candidate top that are missing full block data, including any
    /// uncle blocks referenced by candidate headers.
    ///
    /// When `fork_height` is provided, the scan starts from
    /// `min(fork_height, confirmed_top + 1)` instead of
    /// `confirmed_top + 1`. This allows detection of fork blocks at
    /// or below the confirmed height that need fetching for a
    /// potential reorg.
    ///
    /// Queries every height from the scan start to candidate_top
    /// using the BlockHeight index, which stores all blockhashes at
    /// each height including uncle blocks that are not on the
    /// candidate chain. A share is included if its metadata status is
    /// Candidate or HeaderValid, meaning it still needs block data.
    ///
    /// Uncle blocks typically sit at heights already covered by the
    /// confirmed chain, so the height scan alone misses them. A
    /// second pass reads the header of each candidate block and adds
    /// any referenced uncle that lacks block data.
    pub fn get_candidate_blocks_missing_data(
        &self,
        fork_height: Option<u32>,
    ) -> Result<Vec<BlockHash>, StoreError> {
        let scan_start = self.missing_data_scan_start(fork_height)?;
        let candidate_height = match self.get_top_candidate_height() {
            Ok(height) => height,
            Err(StoreError::NotFound(_)) => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };

        if candidate_height < scan_start {
            return Ok(Vec::new());
        }

        let (all_blockhashes, missing) =
            self.scan_heights_for_missing_blocks(scan_start, candidate_height)?;

        let missing_uncles = self.collect_missing_uncle_blocks(&all_blockhashes);
        debug!(
            "get_candidate_blocks_missing_data: scan_start={scan_start}, candidate_height={candidate_height}, all_blocks={}, missing_blocks={}, missing_uncles={}",
            all_blockhashes.len(),
            missing.len(),
            missing_uncles.len()
        );

        let mut result = Vec::with_capacity(missing_uncles.len() + missing.len());
        result.extend(missing_uncles);
        result.extend(missing);
        Ok(result)
    }

    /// Candidate-chain blocks from `from_height` upward whose body is stored but
    /// which are still only `HeaderValid` -- stranded: everything needed to
    /// validate them is present, yet they never became `BlockValid` (their
    /// validation events were lost on restart, or dropped).
    ///
    /// The complement of [`Store::get_candidate_blocks_missing_data`]: that
    /// returns candidate blocks whose body is *missing*; this returns those
    /// whose body is *present* but still unvalidated. The organise worker's
    /// startup seed uses it to resume promotion after a restart.
    ///
    /// Restricted to `ChainMembership::Candidate` so off-chain siblings -- a
    /// dense height's flood of HeaderValid-with-body blocks -- are never
    /// re-driven. Returns at most `limit` hashes, height-ascending (parents
    /// before children).
    ///
    /// Scans the candidate range in bounded height batches and stops as soon as
    /// `limit` blocks are found, so a restart with the confirmed tip far below
    /// the candidate tip never materialises the whole unconfirmed range (every
    /// height and every dense-height sibling) or reads its metadata in one shot.
    pub fn get_candidate_blocks_needing_validation(
        &self,
        from_height: u32,
        limit: usize,
    ) -> Result<Vec<BlockHash>, StoreError> {
        self.scan_candidate_blocks_needing_validation(
            from_height,
            limit,
            NEEDING_VALIDATION_SCAN_BATCH_HEIGHTS,
        )
    }

    /// Batched scan behind `get_candidate_blocks_needing_validation`, with the
    /// per-batch height span parameterised so tests can exercise the multi-batch
    /// stitching without building a full window of blocks.
    fn scan_candidate_blocks_needing_validation(
        &self,
        from_height: u32,
        limit: usize,
        batch_heights: u32,
    ) -> Result<Vec<BlockHash>, StoreError> {
        let candidate_height = match self.get_top_candidate_height() {
            Ok(height) => height,
            Err(StoreError::NotFound(_)) => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        if limit == 0 || candidate_height < from_height {
            return Ok(Vec::new());
        }

        let mut stranded = Vec::with_capacity(limit);
        let mut batch_start = from_height;
        while batch_start <= candidate_height && stranded.len() < limit {
            let batch_end = batch_start
                .saturating_add(batch_heights - 1)
                .min(candidate_height);

            let height_entries = self.get_blockhashes_for_height_range(batch_start, batch_end);
            let batch_total: usize = height_entries.iter().map(|(_, hashes)| hashes.len()).sum();
            let mut batch_hashes = Vec::with_capacity(batch_total);
            for (_, blockhashes) in &height_entries {
                batch_hashes.extend(blockhashes);
            }

            let metadata_map: HashMap<BlockHash, BlockMetadata> = self
                .get_block_metadata_batch(&batch_hashes)?
                .into_iter()
                .collect();

            let remaining = limit - stranded.len();
            stranded.extend(
                batch_hashes
                    .into_iter()
                    .filter(|blockhash| {
                        metadata_map.get(blockhash).is_some_and(|metadata| {
                            metadata.status == Status::HeaderValid
                                && metadata.chain == ChainMembership::Candidate
                        }) && self.share_block_exists(blockhash)
                    })
                    .take(remaining),
            );

            batch_start = batch_end.saturating_add(1);
        }
        Ok(stranded)
    }

    /// Compute the starting height for the missing-data scan.
    ///
    /// Defaults to `confirmed_top + 1`. When `fork_height` is
    /// provided, uses the lower of the two so that fork blocks below
    /// the confirmed tip are included.
    ///
    /// The result is clamped upward to the prune boundary
    /// (candidate_tip - PRUNE_DEPTH) so blocks under the prune zone are
    /// never fetched. Their PoW is already validated at header sync
    /// time and they don't need block bodies.
    fn missing_data_scan_start(&self, fork_height: Option<u32>) -> Result<u32, StoreError> {
        let confirmed_height = match self.get_top_confirmed_height() {
            Ok(height) => height,
            Err(StoreError::NotFound(_)) => 0,
            Err(error) => return Err(error),
        };
        let next_confirmed_height = confirmed_height + 1;
        let min_fork_and_confirmed_height = match fork_height {
            Some(fork_height) => std::cmp::min(fork_height, next_confirmed_height),
            None => next_confirmed_height,
        };

        let candidate_height = match self.get_top_candidate_height() {
            Ok(height) => height,
            Err(StoreError::NotFound(_)) => return Ok(min_fork_and_confirmed_height),
            Err(error) => return Err(error),
        };
        let prune_floor = prune_height_for_tip(self.depths(), candidate_height);

        Ok(std::cmp::max(min_fork_and_confirmed_height, prune_floor))
    }

    /// Walk heights from `scan_start` to `candidate_height`, collecting
    /// all blockhashes and identifying those that need block data.
    ///
    /// Returns `(all_blockhashes, missing)` where `missing` contains
    /// hashes whose block body is not stored and whose status is not
    /// yet BlockValid or Confirmed.
    fn scan_heights_for_missing_blocks(
        &self,
        scan_start: u32,
        candidate_height: u32,
    ) -> Result<(Vec<BlockHash>, Vec<BlockHash>), StoreError> {
        let height_entries = self.get_blockhashes_for_height_range(scan_start, candidate_height);

        let total_blocks: usize = height_entries.iter().map(|(_, hashes)| hashes.len()).sum();
        let mut all_blockhashes = Vec::with_capacity(total_blocks);
        for (_, blockhashes) in &height_entries {
            all_blockhashes.extend(blockhashes);
        }

        let metadata_results = self.get_block_metadata_batch(&all_blockhashes)?;
        let already_valid: HashSet<BlockHash> = metadata_results
            .into_iter()
            .filter(|(_, metadata)| {
                metadata.status == Status::BlockValid
                    || metadata.chain == ChainMembership::Confirmed
            })
            .map(|(blockhash, _)| blockhash)
            .collect();

        let candidates_to_check: Vec<BlockHash> = all_blockhashes
            .iter()
            .filter(|hash| !already_valid.contains(*hash))
            .copied()
            .collect();
        let missing = self.missing_share_blocks(&candidates_to_check);

        Ok((all_blockhashes, missing))
    }

    /// Collect uncle blocks whose bodies are missing, recursively
    /// following each uncle's own uncles until no new missing bodies
    /// are found.
    fn collect_missing_uncle_blocks(&self, blockhashes: &[BlockHash]) -> Vec<BlockHash> {
        let mut missing_uncles: Vec<BlockHash> = Vec::new();

        // First pass: scan uncles of all provided blockhashes
        for blockhash in blockhashes {
            let Ok(Some(header)) = self.get_share_header(blockhash) else {
                continue;
            };
            for uncle_hash in &header.uncles {
                if !self.share_block_exists(uncle_hash)
                    && !missing_uncles.contains(uncle_hash)
                    && !blockhashes.contains(uncle_hash)
                {
                    debug!(
                        "collect_missing_uncle_blocks: uncle {uncle_hash} missing body (referenced by {blockhash})"
                    );
                    missing_uncles.push(*uncle_hash);
                }
            }
        }

        // Recursive passes: each newly discovered missing uncle may
        // itself reference further uncles whose bodies are also missing.
        let mut scan_start = 0;
        loop {
            let scan_end = missing_uncles.len();
            if scan_start >= scan_end {
                return missing_uncles;
            }

            let mut new_uncles: Vec<BlockHash> = Vec::new();
            for index in scan_start..scan_end {
                let uncle_hash = missing_uncles[index];
                let Ok(Some(header)) = self.get_share_header(&uncle_hash) else {
                    continue;
                };
                for nested_uncle in &header.uncles {
                    if !self.share_block_exists(nested_uncle)
                        && !missing_uncles.contains(nested_uncle)
                        && !blockhashes.contains(nested_uncle)
                        && !new_uncles.contains(nested_uncle)
                    {
                        debug!(
                            "collect_missing_uncle_blocks: nested uncle {nested_uncle} missing body (referenced by {uncle_hash})"
                        );
                        new_uncles.push(*nested_uncle);
                    }
                }
            }
            scan_start = scan_end;
            missing_uncles.extend(new_uncles);
        }
    }

    /// Check if a blockhash is both on the candidate chain and BlockValid.
    ///
    /// This is the promotion gate inside the PPLNS zone (at or above the
    /// prune boundary): a block may only be confirmed there once its
    /// chain-context validation has completed, so nothing reaches the
    /// confirmed chain before it is fully validated.
    pub(super) fn is_candidate_and_block_valid(&self, blockhash: &BlockHash) -> bool {
        self.get_block_metadata(blockhash)
            .map(|m| m.chain == ChainMembership::Candidate && m.status == Status::BlockValid)
            .unwrap_or(false)
    }

    /// Check if a blockhash is on the candidate chain.
    pub fn is_candidate(&self, blockhash: &BlockHash) -> bool {
        self.get_block_metadata(blockhash)
            .map(|m| m.chain == ChainMembership::Candidate)
            .unwrap_or(false)
    }

    /// Extends candidate chain, if:
    /// 1. new share's height is one more than top candidate
    /// 2. new share's prev hash is top candidate hash
    /// 3. new share's chain work is more than top candidate's chain work.
    /// 4. Or, adds to candidate chain if it is empty.
    ///
    /// The same function is used to check if the share extending the
    /// top_candidate or top_confirmed chains using `top_at_chain` param.
    ///
    /// Returns true if candidate chain is extended.
    pub(super) fn should_extend_candidates(
        &self,
        header: &ShareHeader,
        metadata: &BlockMetadata,
        top_at_chain: Option<&TopResult>,
    ) -> Result<Option<Height>, StoreError> {
        match top_at_chain {
            None => Ok(metadata.expected_height),
            Some(top) => {
                let expected_height = metadata.expected_height.unwrap_or_default();
                if top.hash == header.prev_share_blockhash
                    && expected_height == top.height + 1
                    && metadata.chain_work > top.work
                {
                    Ok(Some(expected_height))
                } else {
                    Ok(None)
                }
            }
        }
    }

    /// Returns true if the share being organised has more cumulative
    /// work than the top candidate. This identifies the case where
    /// share is not building on the current top_candidate, but is a
    /// different branch that needs to be reorged in.
    pub(super) fn should_reorg_candidate(
        &self,
        share_blockhash: &BlockHash,
        metadata: &BlockMetadata,
        top_candidate: Option<&TopResult>,
    ) -> bool {
        match top_candidate {
            Some(top) => metadata.chain_work > top.work && top.hash != *share_blockhash,
            None => false,
        }
    }

    /// Whether the reorg branch ending at `blockhash` passes through an
    /// `Invalid` block.
    ///
    /// `reorg_candidate` walks `get_branch_to_chain` back to the first
    /// candidate/confirmed ancestor and writes every block above it as a
    /// candidate. A detached `Invalid` block has `chain == None`, so it does not
    /// stop that walk and would be silently re-admitted. This lets the caller
    /// refuse such a reorg, so an invalid-by-descent branch never becomes the
    /// candidate chain.
    pub(super) fn reorg_branch_has_invalid(
        &self,
        blockhash: &BlockHash,
    ) -> Result<bool, StoreError> {
        let Some(branch) =
            self.get_branch_to_chain(blockhash, |h| self.is_candidate(h) || self.is_confirmed(h))?
        else {
            return Ok(false);
        };
        for hash in &branch {
            if self.get_block_metadata(hash)?.status == Status::Invalid {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Whether the reorg branch ending at `blockhash` forks from the candidate
    /// or confirmed chain below the reorg floor (`Store::reorg_floor`), more
    /// than `MAX_REORG_DEPTH` (`PAYOUT_DEPTH`) below the confirmed tip.
    ///
    /// A block on such a branch may validly spend an output this node never
    /// fetched or stamped with an unknown coinbase root, so it could reject a
    /// valid block (see `MAX_REORG_DEPTH`). Below the prune target the fork
    /// point's body is deleted too, and `reorg_confirmed` could never adopt
    /// the branch. Refusing the candidate reorg turns either into a clean
    /// refusal, a reorg depth limit. Following such a branch would take
    /// out-working the whole pool for about `PAYOUT_DEPTH` blocks.
    ///
    /// The floor is far above the prune target, so the branch's uncles, at
    /// most `MAX_UNCLES_DEPTH - 1` heights below the fork point, still have
    /// their bodies (a compile-time assertion in `prune_blocks.rs`).
    ///
    /// Returns false when the confirmed tip is within `MAX_REORG_DEPTH` of
    /// genesis (or absent), or when the branch does not reach either chain.
    pub(super) fn reorg_branch_forks_below_reorg_floor(
        &self,
        blockhash: &BlockHash,
    ) -> Result<bool, StoreError> {
        let reorg_floor = self.reorg_floor()?;
        if reorg_floor == 0 {
            return Ok(false);
        }
        let Some(branch) =
            self.get_branch_to_chain(blockhash, |h| self.is_candidate(h) || self.is_confirmed(h))?
        else {
            return Ok(false);
        };
        let Some(branch_point) = branch.front() else {
            return Ok(false);
        };
        let branch_point_height = self
            .get_block_metadata(branch_point)?
            .expected_height
            .ok_or_else(|| {
                StoreError::NotFound(format!(
                    "Branch point {branch_point} missing expected_height"
                ))
            })?;
        Ok(branch_point_height < reorg_floor)
    }

    /// Reorgs the candidate chain to the branch ending at `blockhash`.
    ///
    /// Walks back from `blockhash` to the first ancestor on the
    /// candidate or confirmed chain. Delegates to
    /// `reorg_candidate_from_candidate` or
    /// `reorg_candidate_from_confirmed` depending on the branch point.
    pub(super) fn reorg_candidate(
        &self,
        blockhash: &BlockHash,
        top_candidate: Option<&TopResult>,
        batch: &mut rocksdb::WriteBatch,
    ) -> Result<(Height, Chain), StoreError> {
        let branch = self
            .get_branch_to_chain(blockhash, |h| self.is_candidate(h) || self.is_confirmed(h))?
            .ok_or_else(|| {
                StoreError::NotFound(format!(
                    "Branch point {blockhash} to reorg candidate chain not found."
                ))
            })?;
        let branch_point = branch.front().ok_or_else(|| {
            StoreError::NotFound("Empty branch returned from get_branch_to_chain.".into())
        })?;

        if self.is_confirmed(branch_point) {
            self.reorg_candidate_from_confirmed(&branch, batch)
        } else {
            self.reorg_candidate_from_candidate(&branch, top_candidate, batch)
        }
    }

    /// Reorg the candidate chain after invalidating `invalid_hash`.
    ///
    /// The invalid block and every candidate above it leave the candidate
    /// chain (`chain = None`), then the best surviving branch is rebuilt from
    /// the invalid block's parent (the branch point, a candidate or the
    /// confirmed tip). The now-invalid block is filtered out of the parent's
    /// children before `pick_best_child` runs -- so we never re-read its
    /// (still-uncommitted) Invalid status -- and `pick_best_child` also skips
    /// any non-HeaderValid block, so the best valid alternative is chosen.
    ///
    /// The caller (`mark_invalid`) has already set `invalid_hash` to
    /// `Invalid` + `chain = None` in the same batch.
    pub(crate) fn reorg_candidate_after_invalidation(
        &self,
        invalid_hash: &BlockHash,
        invalid_metadata: &BlockMetadata,
        confirmed_top: Height,
        batch: &mut rocksdb::WriteBatch,
    ) -> Result<(), StoreError> {
        let invalid_height = invalid_metadata.expected_height.ok_or_else(|| {
            StoreError::NotFound("Invalidated candidate missing expected_height".into())
        })?;
        self.detach_candidate_branch(invalid_hash, invalid_height, batch)?;
        let final_top = self.rebuild_candidate_from_parent(invalid_hash, invalid_height, batch)?;
        self.set_top_candidate_height_after_rebuild(final_top, confirmed_top, batch);
        Ok(())
    }

    /// Remove the invalid block and every candidate above it from the
    /// candidate index, clearing their candidate membership.
    ///
    /// The invalid block's own metadata was already set to `Invalid` +
    /// `chain = None` by the caller in this same (uncommitted) batch, so it
    /// is only deleted from the index here -- re-reading it would return the
    /// stale committed metadata and clobber that write.
    fn detach_candidate_branch(
        &self,
        invalid_hash: &BlockHash,
        from_height: Height,
        batch: &mut rocksdb::WriteBatch,
    ) -> Result<(), StoreError> {
        let top = self.get_top_candidate_height()?;
        for height in from_height..=top {
            let hash = self.get_candidate_at_height(height)?;
            self.delete_candidate_entry(height, batch);
            if hash != *invalid_hash {
                let mut metadata = self.get_block_metadata(&hash)?;
                metadata.chain = ChainMembership::None;
                self.update_block_metadata(&hash, &metadata, batch)?;
            }
        }
        Ok(())
    }

    /// Rebuild the best-work candidate branch from the invalid block's parent
    /// (the branch point, a candidate or the confirmed tip).
    ///
    /// Walks forward from the parent with `extend_candidates_with_children`,
    /// excluding the invalid block so its still-uncommitted Invalid status is
    /// never re-read; `pick_best_child` also skips non-HeaderValid blocks, so
    /// the best valid alternative is chosen. Returns the new candidate top
    /// height, or the parent's height when no child survives.
    fn rebuild_candidate_from_parent(
        &self,
        invalid_hash: &BlockHash,
        invalid_height: Height,
        batch: &mut rocksdb::WriteBatch,
    ) -> Result<Height, StoreError> {
        let parent_hash = self
            .get_share_header(invalid_hash)?
            .ok_or_else(|| {
                StoreError::NotFound(format!("No header for invalidated block {invalid_hash}"))
            })?
            .prev_share_blockhash;
        let parent_height = invalid_height - 1;
        let mut new_candidates: Chain = Vec::new();
        self.extend_candidates_with_children(
            parent_height,
            &parent_hash,
            &mut new_candidates,
            batch,
            Some(invalid_hash),
        )
    }

    /// Record the rebuilt candidate top.
    ///
    /// `extend_candidates_with_children` only raises the top when it appends,
    /// so the no-surviving-child and single-child cases are handled here.
    ///
    /// When nothing survives above the confirmed tip the candidate top becomes
    /// the confirmed tip itself. That is the ordinary steady state -- every
    /// promotion leaves the candidate top equal to the confirmed top, with the
    /// promoted block's candidate index entry left in place -- so the next
    /// block extends from it through the normal path.
    ///
    /// The key is never deleted. Its absence means "no candidate chain has ever
    /// been written", which is only true before the first block after genesis,
    /// and every reader treats it that way: `organise_block` returns `Ok(None)`
    /// and stops advancing the confirmed chain, `should_extend_candidates`
    /// takes its bootstrap arm and accepts any header at any height as the new
    /// tip with no parent, contiguity or work check, and
    /// `get_candidate_tip_height` returns `None`, which leaves
    /// `check_pplns_zone` and the block receiver's prune height falling back to
    /// zero.
    ///
    /// `confirmed_top` is passed in rather than read: `mark_invalid` runs after
    /// `confirm_blocks` has queued a new confirmed top into the same batch, and
    /// a store read here would see the pre-batch value.
    fn set_top_candidate_height_after_rebuild(
        &self,
        final_top: Height,
        confirmed_top: Height,
        batch: &mut rocksdb::WriteBatch,
    ) {
        self.set_top_candidate_height(final_top.max(confirmed_top), batch);
    }

    /// Reorg when the branch point is on the candidate chain.
    ///
    /// Removes old candidate entries from the branch point to the
    /// current top, resetting their status to HeaderValid. Then writes
    /// the new branch as candidates.
    fn reorg_candidate_from_candidate(
        &self,
        branch: &VecDeque<BlockHash>,
        top_candidate: Option<&TopResult>,
        batch: &mut rocksdb::WriteBatch,
    ) -> Result<(Height, Chain), StoreError> {
        let branch_point = branch.front().ok_or_else(|| {
            StoreError::NotFound("Empty branch in reorg_candidate_from_candidate.".into())
        })?;
        let reorged_out_chain = self.get_candidates_chain(branch_point, top_candidate)?;
        for (height, uncandidate) in &reorged_out_chain {
            self.delete_candidate_entry(*height, batch);
            let mut metadata = self.get_block_metadata(uncandidate)?;
            metadata.chain = ChainMembership::None;
            self.update_block_metadata(uncandidate, &metadata, batch)?;
        }

        self.write_branch_as_candidates(branch, batch)
    }

    /// Reorg when the branch point is on the confirmed chain.
    ///
    /// No old candidate entries need removal. Writes all branch
    /// members above the confirmed branch point as candidates.
    fn reorg_candidate_from_confirmed(
        &self,
        branch: &VecDeque<BlockHash>,
        batch: &mut rocksdb::WriteBatch,
    ) -> Result<(Height, Chain), StoreError> {
        let mut entries = branch.iter();
        entries.next(); // skip the confirmed branch point
        let above_confirmed: VecDeque<BlockHash> = entries.copied().collect();
        self.write_branch_as_candidates(&above_confirmed, batch)
    }

    /// Write a sequence of blockhashes as candidate entries, updating
    /// their metadata to Candidate and setting the top candidate height.
    ///
    /// Returns the new top height and the candidate chain.
    fn write_branch_as_candidates(
        &self,
        branch: &VecDeque<BlockHash>,
        batch: &mut rocksdb::WriteBatch,
    ) -> Result<(Height, Chain), StoreError> {
        let mut new_top_height = 0u32;
        let mut new_chain = Vec::with_capacity(branch.len());
        for candidate in branch {
            let mut metadata = self.get_block_metadata(candidate)?;
            let height = metadata.expected_height.ok_or_else(|| {
                StoreError::NotFound("Block metadata missing expected_height for candidate".into())
            })?;
            self.put_candidate_entry(height, candidate, batch);
            metadata.chain = ChainMembership::Candidate;
            self.update_block_metadata(candidate, &metadata, batch)?;
            new_chain.push((height, *candidate));
            new_top_height = height;
        }
        self.set_top_candidate_height(new_top_height, batch);
        Ok((new_top_height, new_chain))
    }

    /// Walk forward from the current candidate tip, discovering
    /// children already stored and appending them to the candidate
    /// chain in the same WriteBatch.
    ///
    /// Among multiple children at the same height, selects the one with
    /// the highest `chain_work`. Verifies parent hash to exclude uncle
    /// relationships from the block index. Overwrites `top_candidate_height`
    /// only if at least one child was appended. `excluded` is forwarded to
    /// `pick_best_child` (see there) so a specific block is never selected.
    pub(super) fn extend_candidates_with_children(
        &self,
        current_top_height: Height,
        current_top_hash: &BlockHash,
        candidates: &mut Chain,
        batch: &mut rocksdb::WriteBatch,
        excluded: Option<&BlockHash>,
    ) -> Result<Height, StoreError> {
        let mut height = current_top_height;
        let mut tip_hash = *current_top_hash;
        let mut found_child = true;

        while found_child {
            found_child = false;

            let children = self
                .get_children_blockhashes(&tip_hash)?
                .unwrap_or_default();

            if let Some((best_hash, mut best_metadata)) =
                self.pick_best_child(&children, &tip_hash, height + 1, excluded)?
            {
                let next_height = height + 1;
                self.put_candidate_entry(next_height, &best_hash, batch);
                best_metadata.chain = ChainMembership::Candidate;
                self.update_block_metadata(&best_hash, &best_metadata, batch)?;
                candidates.push((next_height, best_hash));

                height = next_height;
                tip_hash = best_hash;
                found_child = true;
            }
        }

        if height > current_top_height {
            self.set_top_candidate_height(height, batch);
        }
        Ok(height)
    }

    /// Select the best qualifying child from a list of children.
    ///
    /// Filters for a valid header (`HeaderValid` or `BlockValid` -- both
    /// carry a valid PoW header and are eligible for the candidate chain;
    /// `BlockValid` is a candidate that has since passed chain-context
    /// validation), correct `expected_height`, and matching parent hash
    /// (to exclude uncle links in the block index). `excluded`, when set,
    /// skips a specific child by hash -- the invalidation reorg uses it so a
    /// block whose Invalid status is still uncommitted in the batch is never
    /// re-selected. Among qualifying children, returns the one with the
    /// highest `chain_work`.
    fn pick_best_child(
        &self,
        children: &[BlockHash],
        parent_hash: &BlockHash,
        expected_height: Height,
        excluded: Option<&BlockHash>,
    ) -> Result<Option<(BlockHash, BlockMetadata)>, StoreError> {
        let mut top_work_child: Option<(BlockHash, BlockMetadata)> = None;

        for child_hash in children {
            let all_children = self
                .get_block_metadata(child_hash)
                .ok()
                .filter(|_| excluded != Some(child_hash))
                .filter(|m| matches!(m.status, Status::HeaderValid | Status::BlockValid))
                .filter(|m| m.expected_height == Some(expected_height))
                .and_then(|m| {
                    self.get_share_header(child_hash)
                        .ok()
                        .flatten()
                        .filter(|h| h.prev_share_blockhash == *parent_hash)
                        .map(|_| (*child_hash, m))
                });

            if let Some((child_hash, child_metadata)) = all_children {
                let has_more_work = top_work_child
                    .as_ref()
                    .map(|(_, current_top_metadata)| {
                        child_metadata.chain_work > current_top_metadata.chain_work
                    })
                    .unwrap_or(true);
                if has_more_work {
                    top_work_child = Some((child_hash, child_metadata));
                }
            }
        }

        Ok(top_work_child)
    }
}

#[cfg(test)]
mod tests;
