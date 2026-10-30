// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The sim emitter's two identities, read from the config with the parsers
//! the node uses for the configured chain.
//!
//! * The payout address (`[sim] miner_address`) is what each share commits
//!   to. On Bitcoin Cash it is a CashAddr whose prefix matches
//!   `bitcoin_cash_network` (`bchreg:` on regtest), decoded by the node's own
//!   `parse_payout_address`; on Bitcoin it is a Bitcoin address.
//! * The share owner (`[sim] share_address`, else `[stratum] miner_address`)
//!   owns each share coinbase. On Bitcoin Cash it is a share address
//!   (`rp2pool:` on regtest) whose 20-byte P2PKH hash is stored; on Bitcoin
//!   it is a bech32m taproot share address.
//!
//! The two are separate settings: neither is derived from the other.

use bitcoin::Network;
use p2poolv2_lib::address::bch_share;
use p2poolv2_lib::address::share_owner::ShareOwner;
use p2poolv2_lib::config::PayoutAddress;
use p2poolv2_lib::config::{BitcoinCashNetwork, Chain, parse_payout_address, parse_share_address};

/// What the sim emitter mines with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimIdentity {
    /// The payout address every share commits to.
    pub payout: PayoutAddress,
    /// The share chain identity owning every share coinbase.
    pub share_owner: ShareOwner,
}

/// The chain a sim node mines, as its `[stratum]` section selects it.
#[derive(Debug, Clone, Copy)]
pub struct ChainSelection {
    pub chain: Chain,
    /// `stratum.network`; on Bitcoin Cash the network `bitcoin_cash_network`
    /// implies (`SimNodeConfig::load` derives it).
    pub network: Network,
    pub bitcoin_cash_network: Option<BitcoinCashNetwork>,
}

/// Parse the payout address and the share owner for `selection`.
///
/// `share_address` is `[sim] share_address`, falling back to
/// `stratum_miner_address`. A missing or unreadable address is an error
/// naming the setting, so the node logs why it is not mining.
pub fn resolve(
    selection: ChainSelection,
    payout_address: &str,
    share_address: Option<&str>,
    stratum_miner_address: Option<&str>,
) -> Result<SimIdentity, String> {
    let payout = parse_payout_address(
        payout_address,
        selection.chain,
        selection.network,
        selection.bitcoin_cash_network,
    )
    .map_err(|error| format!("Invalid sim miner_address: {error}"))?;
    let (setting, share_text) = match (share_address, stratum_miner_address) {
        (Some(address), _) => ("sim share_address", address),
        (None, Some(address)) => ("stratum miner_address", address),
        (None, None) => {
            return Err(
                "Sim emitter needs a share chain address: set [sim] share_address or [stratum] miner_address"
                    .to_string(),
            );
        }
    };
    let share_owner = parse_share_owner(selection, share_text)
        .map_err(|error| format!("Invalid {setting}: {error}"))?;
    Ok(SimIdentity {
        payout,
        share_owner,
    })
}

/// A share address for the selected chain, as the share owner it stores.
fn parse_share_owner(selection: ChainSelection, address: &str) -> Result<ShareOwner, String> {
    match selection.chain {
        Chain::Bitcoin => parse_share_address(address, selection.network)
            .map(|parsed| ShareOwner::Taproot(parsed.witness_program()))
            .map_err(|error| error.to_string()),
        Chain::BitcoinCash => {
            let cash_network = selection
                .bitcoin_cash_network
                .ok_or("bitcoin_cash_network is required when chain is bitcoin-cash")?;
            bch_share::parse(address, cash_network.share_network())
                .map(|parsed| ShareOwner::CashP2pkh(parsed.hash))
                .map_err(|error| error.to_string())
        }
    }
}

#[cfg(test)]
mod tests;
