// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::store::block_tx_metadata::{ChainMembership, Status};
use crate::{
    shares::share_block::ShareHeader,
    store::{block_tx_metadata::BlockMetadata, writer::StoreError},
};
use bitcoin::{BlockHash, Work};
use tracing::{debug, info};

use super::{Height, Store, TopResult};

impl Store {
    /// Organise a share header into the candidate chain.
    ///
    /// Reads block metadata, top candidate, and top confirmed from the store,
    /// then extends or reorgs the candidate chain as needed. Also records the
    /// header as the nephew of each uncle it references in the uncles index.
    ///
    /// Returns the new candidate height and chain if the candidate chain
    /// changed, or None if unchanged.
    pub(crate) fn organise_header(
        &self,
        header: &ShareHeader,
        batch: &mut rocksdb::WriteBatch,
    ) -> Result<Option<Height>, StoreError> {
        let blockhash = header.block_hash();
        debug!(
            "organise_header called for {blockhash} with prev blockhash {}",
            header.prev_share_blockhash
        );

        let mut metadata = match self.get_block_metadata(&blockhash) {
            Ok(existing) => {
                debug!("Block {blockhash} already has metadata, skipping header setup");
                self.repair_height_index_if_needed(header, &blockhash, &existing, batch)?;
                existing
            }
            Err(_) => {
                let metadata = self.initialise_new_header(header, &blockhash, batch)?;
                // Commit the header and metadata now so that subsequent
                // reads in reorg_candidate -> get_branch_to_chain can
                // find this header in the database. Without this commit
                // the header only exists in the uncommitted WriteBatch
                // and the reorg walk fails with "branch point not found".
                self.commit_batch(std::mem::take(batch))
                    .map_err(|error| StoreError::Database(error.to_string()))?;
                *batch = Store::get_write_batch();
                metadata
            }
        };

        // A Confirmed or Invalid block must not be re-organised into the candidate
        // chain.
        if metadata.chain == ChainMembership::Confirmed || metadata.status == Status::Invalid {
            return Ok(None);
        }

        let top_candidate = self.get_top_candidate().ok();
        let Some(top_confirmed) = self.get_top_confirmed().ok() else {
            return Err(StoreError::Database(
                "organise_header called without a genesis block".into(),
            ));
        };

        debug!("top candidate {:?}", top_candidate);
        debug!("top confirmed {:?}", top_confirmed);

        if let Some(extended_height) =
            self.should_extend_candidates(header, &metadata, top_candidate.as_ref())?
        {
            return self.extend_candidate_chain(&blockhash, extended_height, &mut metadata, batch);
        }

        if self.should_reorg_candidate(&blockhash, &metadata, top_candidate.as_ref()) {
            // Refuse a reorg whose branch descends through an Invalid block.
            if self.reorg_branch_has_invalid(&blockhash)? {
                debug!(
                    "Refusing candidate reorg to {blockhash}: branch passes through an Invalid block"
                );
                return Ok(None);
            }
            // Refuse a reorg forking more than MAX_REORG_DEPTH below the
            // confirmed tip: its blocks may spend outputs this node never
            // fetched or stamped with an unknown coinbase root.
            if self.reorg_branch_forks_below_reorg_floor(&blockhash)? {
                info!(
                    "Refusing candidate reorg to {blockhash}: branch forks more than MAX_REORG_DEPTH below the confirmed tip"
                );
                return Ok(None);
            }
            return self.reorg_candidate_chain(&blockhash, top_candidate.as_ref(), batch);
        }

        Ok(None)
    }

    /// Extend the candidate chain with a new block and walk forward
    /// to include any children that were waiting for this block.
    fn extend_candidate_chain(
        &self,
        blockhash: &BlockHash,
        extended_height: Height,
        metadata: &mut BlockMetadata,
        batch: &mut rocksdb::WriteBatch,
    ) -> Result<Option<Height>, StoreError> {
        debug!("Extending candidate");
        self.append_to_candidates(blockhash, extended_height, metadata, batch)?;

        let mut new_entries = vec![(extended_height, *blockhash)];
        let final_height = self.extend_candidates_with_children(
            extended_height,
            blockhash,
            &mut new_entries,
            batch,
            None,
        )?;
        debug!("new candidate height after extending candidates {final_height}");
        Ok(Some(final_height))
    }

    /// Reorg the candidate chain to a fork with more work, then walk
    /// forward to include any children beyond the new tip.
    fn reorg_candidate_chain(
        &self,
        blockhash: &BlockHash,
        top_candidate: Option<&TopResult>,
        batch: &mut rocksdb::WriteBatch,
    ) -> Result<Option<Height>, StoreError> {
        let (new_height, reorg_chain) = self.reorg_candidate(blockhash, top_candidate, batch)?;
        debug!("new candidate height after reorging candidates {new_height}");

        let reorg_tip_hash = reorg_chain.last().expect("reorg chain is non-empty").1;
        let mut new_entries = Vec::new();
        let final_height = self.extend_candidates_with_children(
            new_height,
            &reorg_tip_hash,
            &mut new_entries,
            batch,
            None,
        )?;
        debug!("new candidate height after reorg + forward walk {final_height}");
        Ok(Some(final_height))
    }

    /// Ensure the height-to-blockhash index contains this block.
    ///
    /// When a block's metadata already exists (e.g. from a previous
    /// gossip or partial sync), `initialise_new_header` is skipped.
    /// This means `set_height_to_blockhash` was never called for this
    /// height, leaving the block invisible to height-based scans such
    /// as `get_candidate_blocks_missing_data`. This function checks
    /// whether the block appears in the height index at its expected
    /// height and adds it if missing.
    fn repair_height_index_if_needed(
        &self,
        header: &ShareHeader,
        blockhash: &BlockHash,
        metadata: &BlockMetadata,
        batch: &mut rocksdb::WriteBatch,
    ) -> Result<(), StoreError> {
        let expected_height = match metadata.expected_height {
            Some(height) => height,
            None => return Ok(()),
        };

        let existing_hashes = self.get_blockhashes_for_height(expected_height);
        if existing_hashes.contains(blockhash) {
            return Ok(());
        }

        debug!("Repairing missing height index entry for {blockhash} at height {expected_height}");
        self.add_share_header(header, batch)?;
        self.set_height_to_blockhash(blockhash, expected_height, batch)?;
        Ok(())
    }

    /// Persist a new header and initialise its metadata.
    ///
    /// Stores the header, computes height and chain work from the
    /// parent, records uncle references in the block and uncles
    /// indexes, and writes initial metadata with HeaderValid status.
    ///
    /// Chain work is the parent's chain work plus the header's own work plus
    /// the own work of each uncle it lists, a share-chain rule from
    /// 2026-10-30 (upstream P2Poolv2 commit 604fe3ce). Errors if the parent
    /// or any uncle header is not in the store.
    fn initialise_new_header(
        &self,
        header: &ShareHeader,
        blockhash: &BlockHash,
        batch: &mut rocksdb::WriteBatch,
    ) -> Result<BlockMetadata, StoreError> {
        self.add_share_header(header, batch)?;

        let share_work = header.get_work();

        let prev_metadata = self
            .get_block_metadata(&header.prev_share_blockhash)
            .map_err(|_| {
                StoreError::NotFound(format!(
                    "Parent {} not found for block {}",
                    header.prev_share_blockhash, blockhash
                ))
            })?;
        let prev_height = prev_metadata.expected_height.ok_or_else(|| {
            StoreError::Database(format!(
                "Parent {} has no expected height for block {}",
                header.prev_share_blockhash, blockhash
            ))
        })?;
        let new_height = prev_height + 1;
        let uncles_work = self.sum_uncles_work(header, blockhash)?;
        let new_chain_work = prev_metadata.chain_work + share_work + uncles_work;

        for uncle_blockhash in &header.uncles {
            self.update_block_index(uncle_blockhash, blockhash, batch)?;
            self.add_to_uncles_index(uncle_blockhash, blockhash, batch)?;
        }

        self.set_height_to_blockhash(blockhash, new_height, batch)?;
        let metadata = BlockMetadata {
            expected_height: Some(new_height),
            chain_work: new_chain_work,
            status: Status::HeaderValid,
            chain: ChainMembership::None,
        };
        self.update_block_metadata(blockhash, &metadata, batch)?;

        Ok(metadata)
    }

    /// Sum the own work of the uncles listed by `header`, reading their
    /// headers in one batch. Only each uncle's own work counts, not the work
    /// of its uncles, so every uncle is counted once per chain.
    ///
    /// Errors if any uncle header is not in the store. Ported from
    /// pool2win's upstream P2Poolv2 commit 604fe3ce (MIT OR Apache-2.0).
    pub(crate) fn sum_uncles_work(
        &self,
        header: &ShareHeader,
        blockhash: &BlockHash,
    ) -> Result<Work, StoreError> {
        let uncle_headers = self.get_share_headers(&header.uncles)?;
        if uncle_headers.len() != header.uncles.len() {
            return Err(StoreError::NotFound(format!(
                "Uncle header not found for block {blockhash}"
            )));
        }
        Ok(uncle_headers.iter().fold(
            Work::from_be_bytes([0u8; 32]),
            |total, (_, uncle_header)| total + uncle_header.get_work(),
        ))
    }
}

#[cfg(test)]
mod tests;
