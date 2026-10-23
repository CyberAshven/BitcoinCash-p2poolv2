// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::accounting::OutputPair;
use crate::accounting::payout::coinbase_budget::PayoutSplit;
use crate::accounting::payout::payout_distribution::{
    PayoutDistribution, append_proportional_distribution,
};
use crate::config::BitcoinCashNetwork;
#[cfg(test)]
#[mockall_double::double]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
#[cfg(not(test))]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
use crate::stratum::validate_username::validate_bitcoin_cash;
use bitcoin::{Address, Amount, BlockHash};
use std::collections::HashMap;
use std::error::Error;
use std::time::{SystemTime, UNIX_EPOCH};

/// Hydrapool's window target, in bitcoin blocks' worth of difficulty: the
/// walk back through stored shares stops once their difficulty reaches one
/// bitcoin block's. A fixed constant rather than config, so the window is the
/// same on every Hydrapool node. The share chain mode does not use it: its
/// window is `PPLNS_WINDOW_SHARES` mature shares.
const HYDRAPOOL_DIFFICULTY_MULTIPLIER: u128 = 1;

pub struct Payout {
    /// Step size in seconds for batch querying shares from storage.
    /// This determines how far back in time to query in each batch.
    /// Default: 86400 seconds (1 day) for typical pool configurations.
    step_size_seconds: u64,
    bitcoin_cash_network: Option<BitcoinCashNetwork>,
}

impl PayoutDistribution for Payout {
    fn fill_distribution_from_shares(
        &mut self,
        distribution: &mut Vec<OutputPair>,
        chain_store_handle: &ChainStoreHandle,
        // Time-window PPLNS is not anchored on a share; it queries by
        // timestamp, so the share-chain anchor does not apply here.
        _anchor: BlockHash,
        bitcoin_difficulty: u128,
        total_amount: bitcoin::Amount,
        remaining_total_amount: Amount,
        bootstrap_address: Address,
        // Hydrapool's coinbase is not share-chain consensus and pays every
        // payee; the share chain's coinbase budget does not apply.
        _split: PayoutSplit,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        // Only calculate proportional distribution if there's remaining amount for miners
        // This avoids parsing miner addresses when 100% goes to donation/fee
        // This also avoids running PPLNS share look ups when we don't need to use that data
        if remaining_total_amount > bitcoin::Amount::ZERO {
            let total_difficulty =
                bitcoin_difficulty.saturating_mul(HYDRAPOOL_DIFFICULTY_MULTIPLIER);
            let address_difficulty_map =
                self.accumulate_difficulty_by_address(chain_store_handle, total_difficulty)?;

            if address_difficulty_map.is_empty() {
                let amount = if self.bitcoin_cash_network.is_some() {
                    remaining_total_amount
                } else {
                    distribution.clear();
                    total_amount
                };
                distribution.push(OutputPair {
                    address: bootstrap_address,
                    amount,
                });
            } else {
                distribution.reserve(address_difficulty_map.len());

                append_proportional_distribution(
                    &address_difficulty_map,
                    remaining_total_amount,
                    distribution,
                )?;
            }
        }

        Ok(())
    }
}

impl Payout {
    /// Create a payout with the batch step size and optional Bitcoin Cash network.
    /// `None` keeps the Bitcoin address format used by existing pools.
    ///
    /// # Arguments
    /// * `step_size_seconds` - Batch size in seconds for querying shares from
    ///   Default: 86400 (1 day) for typical pool configurations
    pub fn new(step_size_seconds: u64, bitcoin_cash_network: Option<BitcoinCashNetwork>) -> Self {
        Self {
            step_size_seconds,
            bitcoin_cash_network,
        }
    }

    /// Accumulate difficulty per miner address from PPLNS shares.
    ///
    /// Queries shares in time windows going backwards from the current time,
    /// accumulating difficulty per address directly into a HashMap. This avoids
    /// building an intermediate Vec of shares and a second grouping pass.
    ///
    /// Continues querying additional time windows if total difficulty hasn't
    /// been reached.
    fn accumulate_difficulty_by_address(
        &self,
        store: &ChainStoreHandle,
        total_difficulty: u128,
    ) -> Result<HashMap<Address, u128>, Box<dyn Error + Send + Sync>> {
        let mut address_difficulty: HashMap<Address, u128> = HashMap::new();
        let mut accumulated_difficulty: u128 = 0;

        // Start from current time and work backwards
        let current_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let mut end_time = current_time;
        let mut has_more_shares = true;

        // Query shares in batches going back in time
        while accumulated_difficulty < total_difficulty && has_more_shares {
            let start_time = end_time.saturating_sub(self.step_size_seconds);

            // Query shares for this time window
            let batch_shares =
                store.get_pplns_shares_filtered(None, Some(start_time), Some(end_time));

            has_more_shares = !batch_shares.is_empty();

            if has_more_shares {
                for share in batch_shares.into_iter() {
                    if accumulated_difficulty < total_difficulty {
                        accumulated_difficulty += share.difficulty as u128;
                        if let Some(btcaddress) = share.btcaddress {
                            let address = if let Some(network) = self.bitcoin_cash_network {
                                validate_bitcoin_cash(&btcaddress, true, network)?
                                    .parsed_address
                                    .ok_or("Missing validated Bitcoin Cash payout address")?
                            } else {
                                btcaddress
                                    .parse::<bitcoin::Address<_>>()
                                    .map_err(|e| {
                                        format!("Invalid bitcoin address '{btcaddress}': {e}")
                                    })?
                                    .assume_checked()
                            };
                            *address_difficulty.entry(address).or_insert(0) +=
                                share.difficulty as u128;
                        }
                    }
                }

                // Move to the next time window (further back in time) if total diff not reached
                if accumulated_difficulty < total_difficulty {
                    end_time = start_time;
                }
            }
        }

        Ok(address_difficulty)
    }
}

#[cfg(test)]
mod tests;
