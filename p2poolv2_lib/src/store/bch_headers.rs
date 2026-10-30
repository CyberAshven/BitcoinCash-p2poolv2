// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The Bitcoin Cash headers this node knows, keyed by block hash, each with
//! its height (`KnownBchHeader`). A header gets here from this node's own
//! BCHN, or from a peer after `BchHeaderRules::check`. Kept forever: about
//! 52,560 headers a year at 84 bytes each, 4.4 MB.
//!
//! Beside them, an index of the node's own BCHN best chain, height to hash,
//! with the tip height under its own key. A sender uses it to tell which
//! headers a peer surely has: those on the best chain at the time.

use super::{ColumnFamily, Store, writer::StoreError};
use crate::shares::bch_headers::KnownBchHeader;
use bitcoin::BlockHash;
use bitcoin::consensus::encode;

/// Key of the best-chain tip height. Heights are 4-byte keys, so this
/// 3-byte key never collides with one.
const BEST_TIP_KEY: &[u8] = b"tip";

fn height_key(height: u32) -> [u8; 4] {
    height.to_be_bytes()
}

impl Store {
    /// Add `known` to the known Bitcoin Cash headers in `batch`.
    pub fn add_bch_header(&self, known: &KnownBchHeader, batch: &mut rocksdb::WriteBatch) {
        let column_family = self.db.cf_handle(&ColumnFamily::BchHeaders).unwrap();
        batch.put_cf(
            &column_family,
            encode::serialize(&known.block_hash()),
            encode::serialize(known),
        );
    }

    /// Add every header in `headers` to the known Bitcoin Cash headers.
    pub fn add_bch_headers(&self, headers: &[KnownBchHeader]) -> Result<(), StoreError> {
        let mut batch = Self::get_write_batch();
        for known in headers {
            self.add_bch_header(known, &mut batch);
        }
        self.commit_batch(batch)?;
        Ok(())
    }

    /// Point each `(height, hash)` of `updates` into the best-chain index,
    /// set its tip to `tip_height`, and drop heights above the tip.
    pub fn set_bch_best_chain(
        &self,
        updates: &[(u32, BlockHash)],
        tip_height: u32,
    ) -> Result<(), StoreError> {
        let column_family = self.db.cf_handle(&ColumnFamily::BchBestChain).unwrap();
        let mut batch = Self::get_write_batch();
        if let Some(old_tip) = self.get_bch_best_tip()? {
            for height in tip_height.saturating_add(1)..=old_tip {
                batch.delete_cf(&column_family, height_key(height));
            }
        }
        for (height, hash) in updates {
            batch.put_cf(&column_family, height_key(*height), encode::serialize(hash));
        }
        batch.put_cf(&column_family, BEST_TIP_KEY, tip_height.to_be_bytes());
        self.commit_batch(batch)?;
        Ok(())
    }

    /// The hash at `height` on the node's BCHN best chain, as last indexed.
    pub fn get_bch_best_hash(&self, height: u32) -> Result<Option<BlockHash>, StoreError> {
        let column_family = self.db.cf_handle(&ColumnFamily::BchBestChain).unwrap();
        self.db
            .get_pinned_cf(&column_family, height_key(height))?
            .map(|bytes| {
                encode::deserialize(&bytes)
                    .map_err(|error| StoreError::Serialization(error.to_string()))
            })
            .transpose()
    }

    /// Height of the indexed best-chain tip, if any.
    pub fn get_bch_best_tip(&self) -> Result<Option<u32>, StoreError> {
        let column_family = self.db.cf_handle(&ColumnFamily::BchBestChain).unwrap();
        self.db
            .get_pinned_cf(&column_family, BEST_TIP_KEY)?
            .map(|bytes| {
                <[u8; 4]>::try_from(bytes.as_ref())
                    .map(u32::from_be_bytes)
                    .map_err(|_| StoreError::Serialization("best-chain tip height".into()))
            })
            .transpose()
    }

    /// The known Bitcoin Cash header with hash `block_hash`, if any.
    pub fn get_bch_header(
        &self,
        block_hash: &BlockHash,
    ) -> Result<Option<KnownBchHeader>, StoreError> {
        let column_family = self.db.cf_handle(&ColumnFamily::BchHeaders).unwrap();
        self.db
            .get_pinned_cf(&column_family, encode::serialize(block_hash))?
            .map(|bytes| {
                encode::deserialize(&bytes)
                    .map_err(|error| StoreError::Serialization(error.to_string()))
            })
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shares::bch_asert::tests::{MAINNET_PAIRS, header};
    use bitcoin::hashes::Hash;
    use tempfile::tempdir;

    #[test]
    fn a_known_header_is_found_by_its_hash_after_a_reopen() {
        let directory = tempdir().unwrap();
        let path = directory.path().to_str().unwrap().to_string();
        let known = KnownBchHeader {
            height: MAINNET_PAIRS[0].0,
            header: header(MAINNET_PAIRS[0].1),
        };
        let store = Store::new(path.clone(), false).unwrap();
        assert_eq!(store.get_bch_header(&known.block_hash()).unwrap(), None);
        let mut batch = Store::get_write_batch();
        store.add_bch_header(&known, &mut batch);
        store.commit_batch(batch).unwrap();
        drop(store);

        let store = Store::new(path, false).unwrap();
        assert_eq!(
            store.get_bch_header(&known.block_hash()).unwrap(),
            Some(known)
        );
    }

    /// A reorg to a shorter chain drops the heights above the new tip.
    #[test]
    fn the_best_chain_index_follows_its_tip_up_and_down() {
        let directory = tempdir().unwrap();
        let store = Store::new(directory.path().to_str().unwrap().to_string(), false).unwrap();
        let hash = |byte: u8| BlockHash::from_byte_array([byte; 32]);
        assert_eq!(store.get_bch_best_tip().unwrap(), None);

        store
            .set_bch_best_chain(&[(10, hash(1)), (11, hash(2)), (12, hash(3))], 12)
            .unwrap();
        assert_eq!(store.get_bch_best_tip().unwrap(), Some(12));
        assert_eq!(store.get_bch_best_hash(11).unwrap(), Some(hash(2)));

        store.set_bch_best_chain(&[(11, hash(9))], 11).unwrap();
        assert_eq!(store.get_bch_best_tip().unwrap(), Some(11));
        assert_eq!(store.get_bch_best_hash(10).unwrap(), Some(hash(1)));
        assert_eq!(store.get_bch_best_hash(11).unwrap(), Some(hash(9)));
        assert_eq!(store.get_bch_best_hash(12).unwrap(), None);
    }
}
