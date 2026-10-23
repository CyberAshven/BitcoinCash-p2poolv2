// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::block_tx_metadata::BlockMetadata;
use super::{ColumnFamily, Store, writer::StoreError};
use crate::shares::share_block::{
    MerkleBranches, ShareBlock, ShareHeader, ShareTransaction, Txids, is_terminal_blockhash,
};
use crate::store::block_tx_metadata::ChainMembership;
use crate::store::block_tx_metadata::Status::{BlockValid, HeaderValid, Invalid, Pending};
use crate::store::transaction_store::{BLOCK_TXIDS_SUFFIX, txids_key};
use bitcoin::BlockHash;
use bitcoin::Transaction;
use bitcoin::TxMerkleNode;
use bitcoin::consensus::{self, Encodable, encode};
use std::collections::{HashMap, HashSet};
use tracing::debug;

impl Store {
    /// Add a share to the store.
    ///
    /// Returns early if the block already exists (duplicate guard).
    /// Returns `StoreError::BelowPruneHeight` for a block below the prune
    /// target, whose body would be deleted again; this also closes the race
    /// between a block passing the receiver's check and a prune committing.
    /// Stores the header in the Header CF, transaction indexes in
    /// BlockTxids CF, template merkle branches in TemplateMerkleBranches
    /// CF, and the bitcoin coinbase in BitcoinCoinbase CF. All writes are
    /// done in a single atomic batch.
    pub fn add_share_block(
        &self,
        share: &ShareBlock,
        batch: &mut rocksdb::WriteBatch,
    ) -> Result<(), StoreError> {
        let blockhash = share.block_hash();

        // Skip if this block already exists in the store
        if self.share_block_exists(&blockhash) {
            debug!("Share block {blockhash} already exists in store, skipping");
            return Ok(());
        }

        debug!(
            "Adding share to store with {} txs: {:?} work: {:?}",
            share.transactions.len(),
            blockhash,
            share.header.get_work()
        );

        let block_height =
            self.compute_block_height_from_parent(&share.header.prev_share_blockhash)?;
        if block_height < self.prune_target()? {
            return Err(StoreError::BelowPruneHeight(blockhash));
        }

        // Store transactions and get their metadata
        let txs_metadata = self.add_sharechain_txs(&share.transactions, block_height, batch)?;

        let txids = Txids(txs_metadata.iter().map(|t| t.txid).collect());
        // Store block -> txids index
        self.add_block_to_txids_index(
            &blockhash,
            &txids,
            batch,
            BLOCK_TXIDS_SUFFIX,
            ColumnFamily::BlockTxids,
        )?;

        self.add_txids_to_blocks_index(&blockhash, &txids, batch)?;

        // Store template merkle branches for validation
        let branches = MerkleBranches(share.template_merkle_branches.clone());
        self.add_template_merkle_branches(&blockhash, &branches, batch)?;

        self.add_bitcoin_coinbase(&blockhash, &share.bitcoin_coinbase, batch)?;

        // Update block index for parent
        self.update_block_index(&share.header.prev_share_blockhash, &blockhash, batch)?;

        // Store the header in the dedicated Header CF
        self.add_share_header(&share.header, batch)?;

        Ok(())
    }

    /// Compute the block height from the parent's metadata.
    ///
    /// Genesis (parent all-zeros) has height 0. For all other blocks,
    /// returns an error if parent metadata is missing to avoid
    /// permanently persisting height 0 which would make outputs
    /// unspendable once the chain tip advances.
    fn compute_block_height_from_parent(
        &self,
        parent_blockhash: &BlockHash,
    ) -> Result<u32, StoreError> {
        if is_terminal_blockhash(parent_blockhash) {
            return Ok(0);
        }
        let parent_metadata = self.get_block_metadata(parent_blockhash).map_err(|error| {
            StoreError::NotFound(format!(
                "Parent {parent_blockhash} metadata not found for coinbase_root_height: {error}",
            ))
        })?;
        parent_metadata
            .expected_height
            .map(|height| height + 1)
            .ok_or_else(|| {
                StoreError::NotFound(format!(
                    "Parent {parent_blockhash} has no expected_height for coinbase_root_height",
                ))
            })
    }

    /// Store a share header in the dedicated Header column family.
    ///
    /// This is idempotent -- writing the same header twice is safe.
    /// Called during header sync (via organise_header) and when
    /// storing full blocks (via add_share_block).
    pub fn add_share_header(
        &self,
        header: &ShareHeader,
        batch: &mut rocksdb::WriteBatch,
    ) -> Result<(), StoreError> {
        let blockhash = header.block_hash();
        let header_cf = self.db.cf_handle(&ColumnFamily::Header).unwrap();
        let mut encoded_header = Vec::new();
        header.consensus_encode(&mut encoded_header)?;
        batch.put_cf::<&[u8], Vec<u8>>(&header_cf, blockhash.as_ref(), encoded_header);
        Ok(())
    }

    /// Store the coinbase merkle branches of headers received without their
    /// bodies, so the node can serve those headers on: a header is only
    /// accepted with the branch its coinbase proof is checked against.
    pub(crate) fn add_header_template_merkle_branches(
        &self,
        entries: &[(BlockHash, Vec<TxMerkleNode>)],
        batch: &mut rocksdb::WriteBatch,
    ) -> Result<(), StoreError> {
        for (blockhash, branches) in entries {
            self.add_template_merkle_branches(blockhash, &MerkleBranches(branches.clone()), batch)?;
        }
        Ok(())
    }

    /// Store template merkle branches for a share block.
    ///
    /// Key is the consensus-serialized blockhash, value is the
    /// consensus-encoded MerkleBranches.
    fn add_template_merkle_branches(
        &self,
        blockhash: &BlockHash,
        branches: &MerkleBranches,
        batch: &mut rocksdb::WriteBatch,
    ) -> Result<(), StoreError> {
        let column_family = self
            .db
            .cf_handle(&ColumnFamily::TemplateMerkleBranches)
            .unwrap();
        let key = consensus::serialize(blockhash);
        let mut value = Vec::new();
        branches.consensus_encode(&mut value)?;
        batch.put_cf::<&[u8], Vec<u8>>(&column_family, &key, value);
        Ok(())
    }

    /// Store the bitcoin coinbase a share block carries, keyed by the
    /// consensus-serialized blockhash.
    fn add_bitcoin_coinbase(
        &self,
        blockhash: &BlockHash,
        bitcoin_coinbase: &Transaction,
        batch: &mut rocksdb::WriteBatch,
    ) -> Result<(), StoreError> {
        let column_family = self.db.cf_handle(&ColumnFamily::BitcoinCoinbase).unwrap();
        let key = consensus::serialize(blockhash);
        batch.put_cf::<&[u8], Vec<u8>>(
            &column_family,
            &key,
            consensus::serialize(bitcoin_coinbase),
        );
        Ok(())
    }

    /// Retrieve the bitcoin coinbase of a stored share block.
    ///
    /// Returns `None` when no coinbase is stored, which is the case for a
    /// header held without its body.
    pub fn get_bitcoin_coinbase(
        &self,
        blockhash: &BlockHash,
    ) -> Result<Option<Transaction>, StoreError> {
        let column_family = self.db.cf_handle(&ColumnFamily::BitcoinCoinbase).unwrap();
        let key = consensus::serialize(blockhash);
        match self.db.get_cf::<&[u8]>(&column_family, &key) {
            Ok(Some(data)) => Ok(Some(encode::deserialize::<Transaction>(&data)?)),
            Ok(None) => Ok(None),
            Err(error) => Err(StoreError::Database(error.to_string())),
        }
    }

    /// Retrieve template merkle branches for a share block.
    ///
    /// Returns an empty vector if no branches are stored (e.g. for
    /// shares stored before this feature was added). Returns a
    /// StoreError if stored data fails to deserialize.
    pub fn get_template_merkle_branches(
        &self,
        blockhash: &BlockHash,
    ) -> Result<Vec<TxMerkleNode>, StoreError> {
        let column_family = self
            .db
            .cf_handle(&ColumnFamily::TemplateMerkleBranches)
            .unwrap();
        let key = consensus::serialize(blockhash);
        match self.db.get_cf::<&[u8]>(&column_family, &key) {
            Ok(Some(data)) => {
                let branches = encode::deserialize::<MerkleBranches>(&data)?;
                Ok(branches.0)
            }
            Ok(None) => Ok(Vec::new()),
            Err(error) => Err(StoreError::Database(error.to_string())),
        }
    }

    /// Check whether a share header exists in the Header column family.
    ///
    /// Uses key_may_exist as a fast negative filter, then confirms
    /// with a full read on a positive result since key_may_exist can
    /// return false positives.
    pub fn share_header_exists(&self, blockhash: &BlockHash) -> bool {
        let header_cf = self.db.cf_handle(&ColumnFamily::Header).unwrap();
        let serialized = consensus::serialize(blockhash);
        if !self.db.key_may_exist_cf(&header_cf, &serialized) {
            return false;
        }
        self.db
            .get_cf::<&[u8]>(&header_cf, &serialized)
            .ok()
            .flatten()
            .is_some()
    }

    /// Check whether a share block (full block data) exists in the store.
    ///
    /// Checks the BlockTxids CF for the txids key, since txids are
    /// written as part of add_share_block and their presence indicates
    /// the full block data has been stored.
    ///
    /// TODO: Replace this with a has_block field in metadata. Will require migration, so pending till we add migration support. See `missing_share_blocks`.
    pub fn share_block_exists(&self, blockhash: &BlockHash) -> bool {
        let block_txids_cf = self.db.cf_handle(&ColumnFamily::BlockTxids).unwrap();
        let key = txids_key(blockhash, BLOCK_TXIDS_SUFFIX);
        self.db
            .get_cf::<&[u8]>(&block_txids_cf, &key)
            .ok()
            .flatten()
            .is_some()
    }

    /// Return blockhashes from the input that do NOT have block data in
    /// the store. Uses a single `multi_get_cf` on the BlockTxids CF.
    ///
    /// TODO: Replace this with a has_block field in metadata. Will require migration, so pending till we add migration support. See `share_block_exists`.
    pub fn missing_share_blocks(&self, blockhashes: &[BlockHash]) -> Vec<BlockHash> {
        let block_txids_cf = self.db.cf_handle(&ColumnFamily::BlockTxids).unwrap();
        let keys: Vec<_> = blockhashes
            .iter()
            .map(|hash| (&block_txids_cf, txids_key(hash, BLOCK_TXIDS_SUFFIX)))
            .collect();
        let results = self.db.multi_get_cf(keys);

        let mut missing = Vec::new();
        for (blockhash, result) in blockhashes.iter().zip(results) {
            if !matches!(result, Ok(Some(_))) {
                missing.push(*blockhash);
            }
        }
        missing
    }

    /// Get a share from the store by reconstructing it from the Header CF
    /// and transaction CFs.
    ///
    /// Returns None if the header, txids or bitcoin coinbase are missing.
    pub fn get_share(&self, blockhash: &BlockHash) -> Option<ShareBlock> {
        let header = match self.get_share_header(blockhash) {
            Ok(Some(header)) => header,
            _ => return None,
        };
        if !self.share_block_exists(blockhash) {
            return None;
        }
        let transactions: Vec<ShareTransaction> = self
            .get_txs_for_blockhash(blockhash)
            .into_iter()
            .map(ShareTransaction)
            .collect();
        let template_merkle_branches = self.get_template_merkle_branches(blockhash).ok()?;
        let bitcoin_coinbase = self.get_bitcoin_coinbase(blockhash).ok()??;
        Some(ShareBlock {
            header,
            transactions,
            template_merkle_branches,
            bitcoin_coinbase,
        })
    }

    /// Get current confirmed chain tip and find the ShareBlock for it.
    pub fn get_share_at_tip(&self) -> Option<ShareBlock> {
        let tip = self.get_chain_tip().ok()?;
        self.get_share(&tip)
    }

    /// Get share headers matching the vector of blockhashes from the Header CF.
    ///
    /// Returns (BlockHash, ShareHeader) pairs in the same order as the input,
    /// skipping any hashes not found. Preserving order and returning the
    /// blockhash avoids callers needing to recompute block_hash().
    pub fn get_share_headers(
        &self,
        blockhashes: &[BlockHash],
    ) -> Result<Vec<(BlockHash, ShareHeader)>, StoreError> {
        let header_cf = self.db.cf_handle(&ColumnFamily::Header).unwrap();
        let keys = blockhashes
            .iter()
            .map(|h| (&header_cf, consensus::serialize(h)))
            .collect::<Vec<_>>();
        let results = self.db.multi_get_cf(keys);
        let mut share_headers = Vec::with_capacity(blockhashes.len());
        for (blockhash, result) in blockhashes.iter().zip(results) {
            if let Ok(Some(data)) = result
                && let Ok(header) = encode::deserialize::<ShareHeader>(&data)
            {
                share_headers.push((*blockhash, header));
            }
        }
        Ok(share_headers)
    }

    /// Find the first blockhash from the slice that has a header in the store.
    ///
    /// Uses multi_get_cf for a single bulk query, then returns the first
    /// blockhash (in slice order) that has a result.
    pub fn first_existing_share_header(&self, blockhashes: &[BlockHash]) -> Option<BlockHash> {
        let header_cf = self.db.cf_handle(&ColumnFamily::Header).unwrap();
        let keys = blockhashes
            .iter()
            .map(|hash| (&header_cf, consensus::serialize(hash)))
            .collect::<Vec<_>>();
        let results = self.db.multi_get_cf(keys);
        for (blockhash, result) in blockhashes.iter().zip(results) {
            if let Ok(Some(_)) = result {
                return Some(*blockhash);
            }
        }
        None
    }

    /// Get multiple shares from the store by reconstructing each from
    /// the Header CF and transaction CFs.
    pub fn get_shares(
        &self,
        blockhashes: &[BlockHash],
    ) -> Result<HashMap<BlockHash, ShareBlock>, StoreError> {
        let found_shares = blockhashes
            .iter()
            .filter_map(|blockhash| {
                let share = self.get_share(blockhash)?;
                Some((*blockhash, share))
            })
            .collect();
        Ok(found_shares)
    }

    /// Set the height for the blockhash, storing it in a vector of blockhashes for that height
    /// We are fine with Vector instead of HashSet as we are not going to have a lot of blockhashes at the same height
    /// Uses merge operator for atomic append without read-modify-write
    pub fn set_height_to_blockhash(
        &self,
        blockhash: &BlockHash,
        height: u32,
        batch: &mut rocksdb::WriteBatch,
    ) -> Result<(), StoreError> {
        let column_family = self.db.cf_handle(&ColumnFamily::BlockHeight).unwrap();
        let mut key = b"h:".to_vec();
        let height_bytes = height.to_be_bytes();
        key.extend_from_slice(&height_bytes);

        // Serialize the single BlockHash to merge
        let mut serialized = Vec::new();
        blockhash.consensus_encode(&mut serialized)?;

        // Use merge operator to atomically append
        batch.merge_cf(&column_family, key, serialized);
        Ok(())
    }

    /// Get the blockhashes for a specific height
    pub fn get_blockhashes_for_height(&self, height: u32) -> Vec<BlockHash> {
        let column_family = self.db.cf_handle(&ColumnFamily::BlockHeight).unwrap();
        let mut key = b"h:".to_vec();
        let height_bytes = height.to_be_bytes();
        key.extend_from_slice(&height_bytes);

        match self.db.get_cf::<&[u8]>(&column_family, key.as_ref()) {
            Ok(Some(blockhashes)) => encode::deserialize(&blockhashes).unwrap_or_default(),
            Ok(None) | Err(_) => vec![],
        }
    }

    /// Get all blockhashes for a range of heights in a single iterator pass.
    ///
    /// Uses a RocksDB range iterator on the BlockHeight CF instead of
    /// per-height point reads. Returns (height, blockhashes) pairs sorted
    /// by height. Heights with no entries are omitted.
    pub fn get_blockhashes_for_height_range(
        &self,
        from_height: u32,
        to_height: u32,
    ) -> Vec<(u32, Vec<BlockHash>)> {
        if from_height > to_height {
            return Vec::new();
        }

        let block_height_cf = self.db.cf_handle(&ColumnFamily::BlockHeight).unwrap();

        let mut lower_key = b"h:".to_vec();
        lower_key.extend_from_slice(&from_height.to_be_bytes());

        let mut upper_key = b"h:".to_vec();
        upper_key.extend_from_slice(&to_height.saturating_add(1).to_be_bytes());

        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_iterate_lower_bound(lower_key.clone());
        read_opts.set_iterate_upper_bound(upper_key);

        let iter = self.db.iterator_cf_opt(
            &block_height_cf,
            read_opts,
            rocksdb::IteratorMode::From(&lower_key, rocksdb::Direction::Forward),
        );

        let capacity = (to_height - from_height + 1) as usize;
        let mut results = Vec::with_capacity(capacity);

        for item in iter.flatten() {
            let (key, value) = item;
            let height = u32::from_be_bytes(key[2..6].try_into().unwrap());
            let blockhashes: Vec<BlockHash> = encode::deserialize(&value).unwrap_or_default();
            if !blockhashes.is_empty() {
                results.push((height, blockhashes));
            }
        }

        results
    }

    /// Get the shares for a specific height
    pub fn get_shares_at_height(
        &self,
        height: u32,
    ) -> Result<HashMap<BlockHash, ShareBlock>, StoreError> {
        let blockhashes = self.get_blockhashes_for_height(height);
        self.get_shares(&blockhashes)
    }

    /// Get the expected height for a block from its metadata.
    ///
    /// Returns an error if metadata is missing or has no expected_height.
    pub fn get_block_height_from_metadata(&self, blockhash: &BlockHash) -> Result<u32, StoreError> {
        let metadata = self.get_block_metadata(blockhash)?;
        metadata.expected_height.ok_or_else(|| {
            StoreError::NotFound(format!("Block {blockhash} has no expected_height"))
        })
    }

    /// Get the block metadata for a blockhash
    pub fn get_block_metadata(&self, blockhash: &BlockHash) -> Result<BlockMetadata, StoreError> {
        let block_metadata_cf = self.db.cf_handle(&ColumnFamily::BlockMetadata).unwrap();
        let metadata_key = consensus::serialize(blockhash);

        match self.db.get_cf::<&[u8]>(&block_metadata_cf, &metadata_key) {
            Ok(Some(metadata_serialized)) => match encode::deserialize(&metadata_serialized) {
                Ok(metadata) => Ok(metadata),
                Err(e) => Err(StoreError::Serialization(format!(
                    "Error deserializing block metadata: {e}"
                ))),
            },
            Ok(None) | Err(_) => Err(StoreError::NotFound(format!(
                "No metadata found for blockhash: {blockhash}"
            ))),
        }
    }

    /// Batch-fetch block metadata for the given blockhashes using multi_get_cf.
    ///
    /// Deduplicates the input so each blockhash is looked up at most once.
    ///
    /// A blockhash with no metadata row is omitted from the result rather than
    /// reported: for most callers that is the answer they are asking for -- an
    /// unknown parent during header sync, a block that is not yet valid, an
    /// uncle with no height. Callers for which an unknown block is a fault must
    /// compare the result against what they asked for; see
    /// `all_in_zone_blocks_block_valid`.
    ///
    /// A read that fails, or a row that will not deserialize, is a different
    /// thing entirely and comes back as `Err`. Folding those in with absence
    /// let a disk fault or a corrupt row read as a fact about the chain.
    pub fn get_block_metadata_batch(
        &self,
        blockhashes: &[BlockHash],
    ) -> Result<Vec<(BlockHash, BlockMetadata)>, StoreError> {
        let mut seen = HashSet::with_capacity(blockhashes.len());
        let unique_blockhashes: Vec<BlockHash> = blockhashes
            .iter()
            .copied()
            .filter(|hash| seen.insert(*hash))
            .collect();

        let block_metadata_cf = self.db.cf_handle(&ColumnFamily::BlockMetadata).unwrap();
        let keys: Vec<_> = unique_blockhashes
            .iter()
            .map(|hash| (&block_metadata_cf, consensus::serialize(hash)))
            .collect();
        let results = self.db.multi_get_cf(keys);
        let mut metadata_results = Vec::with_capacity(unique_blockhashes.len());
        for (blockhash, result) in unique_blockhashes.iter().zip(results) {
            let Some(data) = result? else {
                continue;
            };
            let metadata = encode::deserialize::<BlockMetadata>(&data).map_err(|error| {
                StoreError::Serialization(format!(
                    "Failed to deserialize metadata for {blockhash}: {error}"
                ))
            })?;
            metadata_results.push((*blockhash, metadata));
        }
        Ok(metadata_results)
    }

    /// Check which blockhashes from the provided list are missing from the store.
    ///
    /// Uses share_header_exists, so "missing" means we have never seen this header.
    pub fn get_missing_blockhashes(&self, blockhashes: &[BlockHash]) -> Vec<BlockHash> {
        blockhashes
            .iter()
            .filter(|&hash| !self.share_header_exists(hash))
            .cloned()
            .collect()
    }

    /// Update block metadata for a blockhash
    pub fn update_block_metadata(
        &self,
        blockhash: &BlockHash,
        metadata: &BlockMetadata,
        batch: &mut rocksdb::WriteBatch,
    ) -> Result<(), StoreError> {
        let block_metadata_cf = self.db.cf_handle(&ColumnFamily::BlockMetadata).unwrap();
        let metadata_key = consensus::serialize(blockhash);

        let mut serialized = Vec::new();
        metadata.consensus_encode(&mut serialized)?;

        batch.put_cf(&block_metadata_cf, &metadata_key, serialized);
        Ok(())
    }

    /// Mark a block Invalid so it is never promoted to confirmed (see
    /// organise_block's Invalid gate). Called when chain-context validation
    /// fails. A no-op only when the block is on the confirmed chain -- a
    /// finalized block must not be downgraded on a later (possibly transient)
    /// failure. A block on the candidate chain (or off-chain) can now be
    /// invalidated even if it was previously `BlockValid`, because validation
    /// state is tracked independently of chain membership. Errors if the block
    /// has no metadata.
    ///
    /// When the block was on the candidate chain, invalidating it also
    /// reorgs the candidate chain: the block and its candidate descendants
    /// leave the chain and the best surviving branch is rebuilt from the
    /// block's parent (see `reorg_candidate_after_invalidation`).
    ///
    /// `pending_confirmed_top` is the confirmed top this batch has already
    /// queued, or `None` when the batch leaves the confirmed chain alone.
    pub fn mark_invalid(
        &self,
        blockhash: &BlockHash,
        pending_confirmed_top: Option<u32>,
        batch: &mut rocksdb::WriteBatch,
    ) -> Result<(), StoreError> {
        let mut metadata = self.get_block_metadata(blockhash)?;
        // Confired blocks can't be marked invalid
        if metadata.chain == ChainMembership::Confirmed {
            return Ok(());
        }
        let was_candidate = metadata.chain == ChainMembership::Candidate;
        metadata.status = Invalid;
        metadata.chain = ChainMembership::None;
        self.update_block_metadata(blockhash, &metadata, batch)?;
        if was_candidate {
            let confirmed_top = match pending_confirmed_top {
                Some(height) => height,
                None => self.get_top_confirmed_height()?,
            };
            self.reorg_candidate_after_invalidation(blockhash, &metadata, confirmed_top, batch)?;
        }
        Ok(())
    }

    /// Mark a block BlockValid after it passes chain-context validation.
    ///
    /// Upgrades a HeaderValid block to BlockValid. An already-BlockValid
    /// block is an idempotent no-op. A Pending or Invalid block is a
    /// precondition violation -- block validation must run on a
    /// header-validated, not-yet-rejected block -- and returns
    /// `StoreError::InvalidStatusTransition` rather than silently succeeding.
    ///
    /// Chain membership is a separate field, so this never affects
    /// candidate/confirmed position. Errors if the block has no metadata.
    pub fn mark_block_valid(
        &self,
        blockhash: &BlockHash,
        batch: &mut rocksdb::WriteBatch,
    ) -> Result<(), StoreError> {
        let mut metadata = self.get_block_metadata(blockhash)?;
        match metadata.status {
            HeaderValid => {
                metadata.status = BlockValid;
                self.update_block_metadata(blockhash, &metadata, batch)?;
                Ok(())
            }
            // Already validated: idempotent no-op.
            BlockValid => Ok(()),
            // Pending (header never validated) or Invalid (already rejected)
            // both violate the precondition that block validation runs on a
            // header-valid block. Surface it instead of silently succeeding.
            Pending | Invalid => Err(StoreError::InvalidStatusTransition(format!(
                "Cannot mark block {blockhash} BlockValid from status {:?}",
                metadata.status
            ))),
        }
    }

    /// Get a share header from the Header column family.
    pub fn get_share_header(
        &self,
        blockhash: &BlockHash,
    ) -> Result<Option<ShareHeader>, StoreError> {
        let header_cf = self.db.cf_handle(&ColumnFamily::Header).unwrap();
        match self.db.get_cf::<&[u8]>(&header_cf, blockhash.as_ref())? {
            Some(data) => {
                let header: ShareHeader = encode::deserialize(&data)?;
                Ok(Some(header))
            }
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests;
