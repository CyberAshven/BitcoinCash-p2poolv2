// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::accounting::payout::simple_pplns::SimplePplnsShare;
use crate::store::Store;
use crate::store::column_families::ColumnFamily;
use crate::store::writer::StoreError;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::debug;

impl Store {
    /// Delete all PPLNS shares older than the given TTL.
    ///
    /// Uses RocksDB range delete for efficient bulk deletion.
    /// Keys are in format: n_time(8 bytes) + user_id(8 bytes) + seq(8 bytes)
    pub fn prune_shares(&self, pplns_ttl: Duration) -> Result<(), StoreError> {
        let pplns_share_cf = self.db.cf_handle(&ColumnFamily::Share).unwrap();

        // Calculate cutoff time in microseconds
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_micros() as u64;
        let cutoff_micros = now.saturating_sub(pplns_ttl.as_micros() as u64);

        debug!(
            "Cleaning up PPLNS shares older than {} seconds (cutoff: {})",
            pplns_ttl.as_secs(),
            cutoff_micros
        );

        // Create range: from beginning (min key) to cutoff (exclusive)
        // We want to delete all shares with timestamp < cutoff_micros
        // Since delete_range_cf uses [start, end), we set end to cutoff + 1 to include cutoff
        let start_key = SimplePplnsShare::make_key(0, 0, 0);
        let end_key = SimplePplnsShare::make_key(cutoff_micros.saturating_add(1), 0, 0);

        // Use RocksDB range delete for efficient bulk deletion
        // delete_range_cf deletes all keys in [start_key, end_key)
        self.db
            .delete_range_cf(&pplns_share_cf, &start_key, &end_key)?;

        debug!("Deleted PPLNS shares older than cutoff time");

        Ok(())
    }
}

#[cfg(test)]
mod tests;
