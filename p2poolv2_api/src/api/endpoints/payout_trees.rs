// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The Bitcoin Cash payout trees this node keeps for the blocks the pool
//! found: every tree's payees and its node transactions, so a payee or a
//! wallet can save them and broadcast them after maturity without P2Pool.

use crate::api::error::ApiError;
use crate::api::server::AppState;
use axum::{
    Json,
    extract::{Query, State},
};
use bitcoin::consensus::encode::serialize_hex;
use bitcoin::{Address, Network, Txid};
use p2poolv2_lib::accounting::payout::payout_tree::{
    BCH_TREE_EXECUTION_FEE_RATE, BCH_TREE_FEE_RATES,
};
use p2poolv2_lib::address::bch_share::BchShareNetwork;
use p2poolv2_lib::address_display::payout_address_string;
use p2poolv2_lib::store::payout_tree::StoredPayoutTree;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::sync::Arc;

/// One kept tree, as `/payout_trees` lists it.
#[derive(Serialize, Debug, PartialEq)]
pub struct PayoutTreeSummary {
    pub coinbase_txid: String,
    pub root_vout: u32,
    pub bitcoin_blockhash: String,
    pub bitcoin_height: u64,
    pub share_blockhash: String,
    /// Satoshis the tree holds.
    pub value: u64,
    pub payee_count: usize,
    /// Whether this node has seen the tree executed and stopped broadcasting.
    pub executed: bool,
}

/// One payee of a tree.
#[derive(Serialize, Debug, PartialEq)]
pub struct PayoutTreePayee {
    /// The payee's address, or its locking script in hex when no address
    /// type here can spell it (P2SH32).
    pub address: String,
    pub script_hex: String,
    /// Satoshis the coinbase owes the payee, before its share of the fee.
    pub amount: u64,
}

/// One node transaction of a tree.
#[derive(Serialize, Debug, PartialEq)]
pub struct PayoutTreeTransaction {
    pub txid: String,
    /// The raw transaction, ready for `sendrawtransaction`.
    pub hex: String,
}

/// A tree's node transactions at one fee rate, root first and each node
/// before its children.
#[derive(Serialize, Debug, PartialEq)]
pub struct PayoutTreeVariant {
    /// Satoshis per byte.
    pub fee_rate: u64,
    /// Satoshis the whole tree pays in fees at this rate.
    pub total_fee: u64,
    pub transactions: Vec<PayoutTreeTransaction>,
}

/// One kept tree in full, as `/payout_tree` returns it.
#[derive(Serialize, Debug, PartialEq)]
pub struct PayoutTreeDetail {
    #[serde(flatten)]
    pub summary: PayoutTreeSummary,
    /// The rate every P2Pool node executes at.
    pub execution_fee_rate: u64,
    pub payees: Vec<PayoutTreePayee>,
    /// The node transactions at every rate the tree's root accepts.
    pub variants: Vec<PayoutTreeVariant>,
}

/// Query of `/payout_tree`.
#[derive(Deserialize)]
pub struct PayoutTreeQuery {
    /// Txid of the coinbase holding the tree's root.
    pub coinbase_txid: String,
}

fn summary(tree: &StoredPayoutTree) -> PayoutTreeSummary {
    PayoutTreeSummary {
        coinbase_txid: tree.coinbase_txid.to_string(),
        root_vout: tree.root_vout,
        bitcoin_blockhash: tree.bitcoin_blockhash.to_string(),
        bitcoin_height: tree.bitcoin_height,
        share_blockhash: tree.share_blockhash.to_string(),
        value: tree.payees.iter().map(|payee| payee.amount).sum(),
        payee_count: tree.payees.len(),
        executed: tree.executed,
    }
}

fn detail(
    stored: &StoredPayoutTree,
    network: Network,
    cash_network: Option<BchShareNetwork>,
) -> Result<PayoutTreeDetail, ApiError> {
    let tree = stored
        .tree()
        .map_err(|error| ApiError::ServerError(format!("Cannot rebuild the tree: {error}")))?;
    let payees = stored
        .payees
        .iter()
        .map(|payee| PayoutTreePayee {
            address: Address::from_script(&payee.script_pubkey, network)
                .map(|address| payout_address_string(&address, cash_network))
                .unwrap_or_else(|_| payee.script_pubkey.to_hex_string()),
            script_hex: payee.script_pubkey.to_hex_string(),
            amount: payee.amount,
        })
        .collect();
    let variants = BCH_TREE_FEE_RATES
        .iter()
        .filter_map(|fee_rate| {
            let transactions = tree.node_transactions(*fee_rate, stored.root_outpoint())?;
            Some(PayoutTreeVariant {
                fee_rate: *fee_rate,
                total_fee: tree.total_fee(*fee_rate)?,
                transactions: transactions
                    .iter()
                    .map(|transaction| PayoutTreeTransaction {
                        txid: transaction.compute_txid().to_string(),
                        hex: serialize_hex(transaction),
                    })
                    .collect(),
            })
        })
        .collect();
    Ok(PayoutTreeDetail {
        summary: summary(stored),
        execution_fee_rate: BCH_TREE_EXECUTION_FEE_RATE,
        payees,
        variants,
    })
}

/// Every payout tree this node keeps, newest block first.
pub(crate) async fn payout_trees(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<PayoutTreeSummary>>, ApiError> {
    let trees = state
        .chain_store_handle
        .get_payout_trees()
        .map_err(|error| ApiError::ServerError(format!("Failed to read payout trees: {error}")))?;
    let mut response: Vec<PayoutTreeSummary> = trees.iter().map(summary).collect();
    response.sort_by_key(|tree| std::cmp::Reverse(tree.bitcoin_height));
    Ok(Json(response))
}

/// One payout tree in full: its payees and its node transactions at every
/// accepted fee rate.
pub(crate) async fn payout_tree(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PayoutTreeQuery>,
) -> Result<Json<PayoutTreeDetail>, ApiError> {
    let coinbase_txid = Txid::from_str(&query.coinbase_txid)
        .map_err(|error| ApiError::BadRequest(format!("Invalid coinbase txid: {error}")))?;
    let stored = state
        .chain_store_handle
        .get_payout_tree(&coinbase_txid)
        .map_err(|error| ApiError::ServerError(format!("Failed to read payout tree: {error}")))?
        .ok_or_else(|| {
            ApiError::NotFound(format!("No payout tree for coinbase {coinbase_txid}"))
        })?;
    Ok(Json(detail(
        &stored,
        state.app_config.network,
        state.app_config.cash_network,
    )?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::hashes::Hash;
    use bitcoin::{BlockHash, PubkeyHash, ScriptBuf};
    use p2poolv2_lib::accounting::payout::payout_tree::{PayoutTree, TreePayee};

    fn stored() -> StoredPayoutTree {
        let payees: Vec<TreePayee> = (1..=3u8)
            .map(|byte| TreePayee {
                script_pubkey: ScriptBuf::new_p2pkh(&PubkeyHash::from_byte_array([byte; 20])),
                amount: 100_000,
            })
            .collect();
        StoredPayoutTree::new(
            &PayoutTree::build(payees).unwrap(),
            Txid::from_byte_array([0x44; 32]),
            2,
            BlockHash::from_byte_array([0x55; 32]),
            900_000,
            BlockHash::from_byte_array([0x66; 32]),
        )
    }

    #[test]
    fn a_tree_is_served_with_its_payees_and_transactions_at_every_rate() {
        let detail = detail(&stored(), Network::Regtest, None).unwrap();

        assert_eq!(detail.summary.value, 300_000);
        assert_eq!(detail.summary.payee_count, 3);
        assert_eq!(detail.execution_fee_rate, 1);
        assert_eq!(detail.payees.len(), 3);
        assert_eq!(detail.variants.len(), 3);
        assert_eq!(detail.variants[0].fee_rate, 1);
        assert_eq!(detail.variants[0].transactions.len(), 1);
        assert!(detail.variants[0].total_fee < detail.variants[2].total_fee);
    }

    #[test]
    fn a_p2sh32_payee_is_shown_by_its_script() {
        let mut tree = stored();
        let mut script = vec![0xaa, 0x20];
        script.extend_from_slice(&[0x77; 32]);
        script.push(0x87);
        tree.payees[0].script_pubkey = ScriptBuf::from_bytes(script.clone());

        let detail = detail(&tree, Network::Regtest, None).unwrap();

        assert_eq!(detail.payees[0].address, hex::encode(&script));
    }
}
