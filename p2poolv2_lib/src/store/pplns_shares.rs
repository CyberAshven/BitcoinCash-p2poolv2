// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::{Store, column_families::ColumnFamily, writer::StoreError};
use crate::accounting::payout::simple_pplns::SimplePplnsShare;
#[cfg(any(feature = "hydrapool-pplns-accounting", test))]
use crate::utils::snowflake_simplified::get_next_id;
#[cfg(any(feature = "hydrapool-pplns-accounting", test))]
use bitcoin::consensus::Encodable;
use bitcoin::consensus::encode;
use std::time::{SystemTime, UNIX_EPOCH};
const INITIAL_SHARE_VEC_CAPACITY: usize = 100_000;

impl Store {
    /// No-op when hydrapool-pplns-accounting feature is disabled.
    #[cfg(not(any(feature = "hydrapool-pplns-accounting", test)))]
    pub fn add_pplns_share(&self, _pplns_share: SimplePplnsShare) -> Result<(), StoreError> {
        Ok(())
    }

    /// Add PPLNS Share to pplns_share_cf.
    /// btcaddress and workername are skipped during serialization (serde(skip)) to minimize storage.
    ///
    /// Key is timestamp (8) + user_id (8) + share id (8) = 24 bytes.
    #[cfg(any(feature = "hydrapool-pplns-accounting", test))]
    pub fn add_pplns_share(&self, pplns_share: SimplePplnsShare) -> Result<(), StoreError> {
        let pplns_share_cf = self.db.cf_handle(&ColumnFamily::Share).unwrap();

        let mut serialized = Vec::new();
        pplns_share.consensus_encode(&mut serialized)?;

        let n_time = pplns_share.n_time * 1_000_000;
        let key = SimplePplnsShare::make_key(n_time, pplns_share.user_id, get_next_id());
        self.db.put_cf(&pplns_share_cf, key, serialized)?;
        Ok(())
    }

    // Get PPLNS shares, no filter yet
    pub fn get_pplns_shares(&self) -> Vec<SimplePplnsShare> {
        self.get_pplns_shares_filtered(None, None, None)
    }

    /// Get PPLNS shares with filtering support using timestamp-based keys for efficient range queries
    /// Deserializes SimplePplnsShare from DB (btcaddress/workername are skipped during serialization)
    /// and enriches with btcaddress from user store
    pub fn get_pplns_shares_filtered(
        &self,
        limit: Option<usize>,
        start_time: Option<u64>,
        end_time: Option<u64>,
    ) -> Vec<SimplePplnsShare> {
        let pplns_share_cf = self.db.cf_handle(&ColumnFamily::Share).unwrap();

        // Convert end_time to microseconds, default to current time if not specified
        let effective_end_time = end_time.map(|t| t * 1_000_000).unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_micros() as u64
        });

        let mut read_opts = rocksdb::ReadOptions::default();
        // Avoid polluting the block cache with scan data -- this range scan
        // can touch 100K+ items and would evict hot tip/header data.
        read_opts.fill_cache(false);

        // Set lower bound for start_time if specified (exclusive lower bound)
        if let Some(start) = start_time {
            let start_micros = start * 1_000_000;
            // Create boundary key with minimum user_id and seq for lower bound
            let start_key = SimplePplnsShare::make_key(start_micros, 0, 0);
            read_opts.set_iterate_lower_bound(start_key);
        }

        // Create starting boundary key with maximum user_id and seq for reverse iteration
        let end_key = SimplePplnsShare::make_key(effective_end_time, u64::MAX, u64::MAX);

        let iter = self.db.iterator_cf_opt(
            &pplns_share_cf,
            read_opts,
            rocksdb::IteratorMode::From(&end_key, rocksdb::Direction::Reverse),
        );

        let use_limit = limit.unwrap_or(INITIAL_SHARE_VEC_CAPACITY);
        let mut shares: Vec<SimplePplnsShare> = Vec::with_capacity(use_limit);

        for (_key, value) in iter.take(use_limit).flatten() {
            if let Ok(share) = encode::deserialize::<SimplePplnsShare>(&value) {
                shares.push(share);
            }
        }

        self.populate_btcaddresses(&shares)
    }

    /// Populate the btcaddress for user_ids in the shares
    /// Uses get_btcaddresses_for_userids
    fn populate_btcaddresses(&self, shares: &[SimplePplnsShare]) -> Vec<SimplePplnsShare> {
        // Collect unique user_ids and query btcaddresses
        let user_ids: Vec<u64> = shares.iter().map(|s| s.user_id).collect();

        // Pass HashSet directly - no intermediate Vec allocation needed
        let btcaddress_map: std::collections::HashMap<u64, String> = self
            .get_btcaddresses_for_user_ids(&user_ids)
            .unwrap_or_default()
            .into_iter()
            .collect();

        // Populate btcaddress field (workername remains None as it's not stored)
        shares
            .iter()
            .filter_map(|share| {
                btcaddress_map.get(&share.user_id).map(|btcaddress| {
                    let mut enriched_share = share.clone();
                    enriched_share.btcaddress = Some(btcaddress.clone());
                    enriched_share
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
