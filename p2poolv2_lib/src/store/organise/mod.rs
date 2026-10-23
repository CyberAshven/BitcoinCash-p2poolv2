// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::{ColumnFamily, Store, writer::StoreError};
use bitcoin::{BlockHash, Work, consensus::encode};
use std::collections::VecDeque;

mod candidate;
mod confirmed;
pub mod organise_block;
pub mod organise_header;

const BRANCH_INITIAL_CAPACITY: usize = 16;

/// Type to capture candidate and confirmed chains as vector of
/// height, blockhash pairs
type Chain = Vec<(u32, BlockHash)>;

/// Height type to avoid using u32
type Height = u32;

/// Top of a candidate or confirmed chain: blockhash, height, and cumulative work.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TopResult {
    pub hash: BlockHash,
    pub height: Height,
    pub work: Work,
}

/// Returns key for height with provided suffix
pub(super) fn height_to_key_with_suffix(height: Height, suffix: &str) -> Vec<u8> {
    [&height.to_be_bytes(), suffix.as_bytes()].concat()
}

impl Store {
    /// Fetch blockhashes from the BlockHeight CF for a given height range
    /// and key suffix. Filters iterator results to only include keys whose
    /// suffix matches, avoiding cross-contamination between candidate (":c")
    /// and confirmed (":f") entries that share the same height prefix.
    pub(super) fn get_chain_range(
        &self,
        from: Height,
        to: Height,
        suffix: &str,
    ) -> Result<Chain, StoreError> {
        if from > to {
            return Ok(Vec::new());
        }

        let block_height_cf = self.db.cf_handle(&ColumnFamily::BlockHeight).unwrap();
        let suffix_bytes = suffix.as_bytes();

        let lower_key = height_to_key_with_suffix(from, suffix);
        // Upper bound is exclusive, so use to+1
        let upper_key = height_to_key_with_suffix(to + 1, suffix);

        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_iterate_lower_bound(lower_key.clone());
        read_opts.set_iterate_upper_bound(upper_key);

        let iter = self.db.iterator_cf_opt(
            &block_height_cf,
            read_opts,
            rocksdb::IteratorMode::From(&lower_key, rocksdb::Direction::Forward),
        );

        let capacity = (to - from + 1) as usize;
        let mut results = Vec::with_capacity(capacity);
        for item in iter.flatten() {
            let (key, value) = item;
            if key.ends_with(suffix_bytes) {
                let height_bytes: [u8; 4] = key[..4]
                    .try_into()
                    .map_err(|_| StoreError::Database("Invalid height key length".into()))?;
                let height = u32::from_be_bytes(height_bytes);
                let blockhash = encode::deserialize(&value)?;
                results.push((height, blockhash));
            }
        }
        Ok(results)
    }

    /// Get branch from a blockhash back to the first ancestor on a target chain.
    ///
    /// Walks backwards through the chain collecting blockhashes until finding
    /// one where `is_on_chain` returns true. Returns the branch including
    /// that ancestor. The predicate allows reuse for both candidate and
    /// confirmed chains.
    pub fn get_branch_to_chain(
        &self,
        blockhash: &BlockHash,
        is_on_chain: impl Fn(&BlockHash) -> bool,
    ) -> Result<Option<VecDeque<BlockHash>>, StoreError> {
        let mut branch = VecDeque::with_capacity(BRANCH_INITIAL_CAPACITY);

        let mut current = *blockhash;
        loop {
            if is_on_chain(&current) {
                // Found ancestor on target chain, include it and return
                branch.push_front(current);
                return Ok(Some(branch));
            }

            // Get the share to find its parent
            let Some(share_header) = self.get_share_header(&current)? else {
                // Share not found, branch doesn't terminate on target chain
                return Ok(None);
            };

            branch.push_front(current);
            current = share_header.prev_share_blockhash;
        }
    }
}

#[cfg(test)]
mod tests;
