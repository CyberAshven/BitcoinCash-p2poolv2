// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Hydrapool PPLNS share worker stores accepted stratum shares for
//! time-window PPLNS payouts in a dedicated task.
//!
//! The stratum connection task only enqueues a share and returns, so it can
//! send the next job to the miner without waiting on the store writer.

use crate::accounting::payout::simple_pplns::SimplePplnsShare;
#[cfg(test)]
#[mockall_double::double]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
#[cfg(not(test))]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
use tokio::sync::mpsc;
use tracing::{error, info};

pub type PplnsShareSender = mpsc::Sender<SimplePplnsShare>;
pub type PplnsShareReceiver = mpsc::Receiver<SimplePplnsShare>;

/// Worker that stores hydrapool PPLNS shares received from the stratum server.
///
/// This is the only path into PPLNS share storage. Every share on the
/// channel is stored and credited at its `difficulty`, so producers must only
/// send shares whose hash meets that difficulty.
pub struct PplnsShareWorker {
    pplns_shares_rx: PplnsShareReceiver,
    chain_store_handle: ChainStoreHandle,
}

impl PplnsShareWorker {
    /// Creates a new PPLNS share worker.
    pub fn new(pplns_shares_rx: PplnsShareReceiver, chain_store_handle: ChainStoreHandle) -> Self {
        Self {
            pplns_shares_rx,
            chain_store_handle,
        }
    }

    /// Runs the worker until every sender has been dropped.
    ///
    /// A failed store write is logged and the worker moves on to the next
    /// share, so one bad write does not stop PPLNS accounting.
    pub async fn run(mut self) {
        info!("PPLNS share worker started");
        while let Some(pplns_share) = self.pplns_shares_rx.recv().await {
            if let Err(error) = self.chain_store_handle.add_pplns_share(pplns_share).await {
                error!("Failed to store PPLNS share: {error}");
            }
        }
        info!("PPLNS share worker stopped - channel closed");
    }
}

#[cfg(test)]
mod tests;
