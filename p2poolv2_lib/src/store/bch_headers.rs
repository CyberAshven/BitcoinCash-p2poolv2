// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The Bitcoin Cash headers this node knows, keyed by block hash, each with
//! its height (`KnownBchHeader`). A header gets here from this node's own
//! BCHN, or from a peer after `BchHeaderRules::check`. Kept forever: about
//! 52,560 headers a year at 84 bytes each, 4.4 MB.

use super::{ColumnFamily, Store, writer::StoreError};
use crate::shares::bch_headers::KnownBchHeader;
use bitcoin::BlockHash;
use bitcoin::consensus::encode;

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
}
