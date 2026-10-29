// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Every Bitcoin Cash P2Pool node executes the payout trees of found blocks.
//!
//! The node that finds a block keeps its payout tree in the store before
//! submitting it (`submit_found_block`), and the validator hands the
//! executor the payout tree of every share it validates that is a BCH block
//! (`DefaultShareValidator::with_payout_tree_sink`). The executor keeps it,
//! whatever the share's later status on the share chain, and on each new BCH
//! tip:
//!
//! - waits until the block holding the tree's coinbase has 100
//!   confirmations, so the coinbase output can be spent in the next block
//!   (BCH's `COINBASE_MATURITY`);
//! - broadcasts the tree's node transactions at
//!   `BCH_TREE_EXECUTION_FEE_RATE`, 1 satoshi per byte, root first and each
//!   node before its children, and the child nodes of the other accepted
//!   rates, so a tree someone else started at 2 or 3 satoshis per byte is
//!   finished too;
//! - rebroadcasts until every node transaction of one root variant is
//!   confirmed for `SETTLED_TIPS` tips in a row, then marks the tree executed;
//! - drops the record of a block BCHN still does not know after
//!   `UNKNOWN_BLOCK_TIP_LIMIT` new tips, counted only outside initial block
//!   download, so minimum-difficulty claims on test networks cannot build an
//!   unbounded polling backlog.
//!
//! It needs no keys and no money: the covenant fixes every transaction, so
//! every node broadcasts the same bytes and duplicates are harmless. BCH has
//! no unconfirmed chain limit since 2021, so a whole tree goes at once. A
//! node-local task, not a share-chain rule.

use crate::accounting::payout::payout_tree::{BCH_TREE_EXECUTION_FEE_RATE, BCH_TREE_FEE_RATES};
#[cfg(test)]
#[mockall_double::double]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
#[cfg(not(test))]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
use crate::store::payout_tree::StoredPayoutTree;
use async_trait::async_trait;
use bitcoin::consensus::encode::serialize_hex;
use bitcoin::{BlockHash, OutPoint, Transaction, Txid};
use bitcoindrpc::{BitcoindRpcClient, BitcoindRpcError};
use std::collections::HashMap;
use std::str::FromStr;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// How often the executor asks BCHN for its tip.
pub const EXECUTOR_POLL_INTERVAL: Duration = Duration::from_secs(30);

/// Confirmations the coinbase's block needs before its outputs can be spent
/// in the next block: BCH's `COINBASE_MATURITY`, 100.
pub const COINBASE_MATURITY_CONFIRMATIONS: i64 = 100;

/// New tips in a row with every transaction of a root variant confirmed
/// before a tree counts as executed.
pub const SETTLED_TIPS: u32 = 6;

/// BCHN's `RPC_VERIFY_ALREADY_IN_CHAIN`.
const RPC_VERIFY_ALREADY_IN_CHAIN: i32 = -27;
/// BCHN's `RPC_TRANSACTION_ERROR`, which `sendrawtransaction` answers for
/// missing or spent inputs.
const RPC_TRANSACTION_ERROR: i32 = -25;
/// BCHN's `RPC_INVALID_ADDRESS_OR_KEY`, which `getblockheader` answers for a
/// block it does not know.
const RPC_INVALID_ADDRESS_OR_KEY: i32 = -5;

/// Sends the validator's found trees to the executor.
pub type PayoutTreeSender = mpsc::UnboundedSender<StoredPayoutTree>;
/// The executor's end of `PayoutTreeSender`.
pub type PayoutTreeReceiver = mpsc::UnboundedReceiver<StoredPayoutTree>;

/// A channel from the validator to the executor.
pub fn payout_tree_channel() -> (PayoutTreeSender, PayoutTreeReceiver) {
    mpsc::unbounded_channel()
}

/// What BCHN said about one node transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BroadcastOutcome {
    /// Accepted, or already waiting in the mempool.
    Accepted,
    /// Already confirmed, with an output still unspent.
    InChain,
    /// An input is missing or spent: the parent is not out yet, the
    /// transaction is confirmed and spent, or another rate's spends it.
    MissingInputs,
    /// Refused for another reason, such as a relay fee above the tree's.
    Rejected(String),
}

/// The BCHN calls the executor makes. A trait so tests can stand in for
/// BCHN.
#[async_trait]
pub trait TreeExecutionNode: Send + Sync {
    /// The hash of the node's best block.
    async fn best_block_hash(&self) -> Result<BlockHash, String>;
    /// Whether the node is still in initial block download, when its not
    /// knowing a block says nothing.
    async fn in_initial_block_download(&self) -> Result<bool, String>;
    /// Confirmations of `blockhash`: -1 when it is off the main chain, `None`
    /// when the node does not know it.
    async fn block_confirmations(&self, blockhash: BlockHash) -> Result<Option<i64>, String>;
    /// Broadcast `transaction`.
    async fn send_raw_transaction(&self, transaction: &Transaction) -> BroadcastOutcome;
    /// Whether `outpoint` is an unspent output of a confirmed transaction.
    async fn unspent_on_chain(&self, outpoint: OutPoint) -> Result<bool, String>;
}

#[async_trait]
impl TreeExecutionNode for BitcoindRpcClient {
    async fn best_block_hash(&self) -> Result<BlockHash, String> {
        let hash: String = self
            .request("getbestblockhash", Vec::new())
            .await
            .map_err(|error| error.to_string())?;
        BlockHash::from_str(&hash).map_err(|error| error.to_string())
    }

    async fn in_initial_block_download(&self) -> Result<bool, String> {
        let info: serde_json::Value = self
            .request("getblockchaininfo", Vec::new())
            .await
            .map_err(|error| error.to_string())?;
        info.get("initialblockdownload")
            .and_then(serde_json::Value::as_bool)
            .ok_or_else(|| "getblockchaininfo has no initialblockdownload".to_string())
    }

    async fn block_confirmations(&self, blockhash: BlockHash) -> Result<Option<i64>, String> {
        let header: Result<serde_json::Value, BitcoindRpcError> = self
            .request(
                "getblockheader",
                vec![
                    serde_json::Value::String(blockhash.to_string()),
                    serde_json::Value::Bool(true),
                ],
            )
            .await;
        match header {
            Ok(header) => Ok(header.get("confirmations").and_then(|value| value.as_i64())),
            Err(BitcoindRpcError::RpcError { code, .. }) if code == RPC_INVALID_ADDRESS_OR_KEY => {
                Ok(None)
            }
            Err(error) => Err(error.to_string()),
        }
    }

    async fn send_raw_transaction(&self, transaction: &Transaction) -> BroadcastOutcome {
        let sent: Result<String, BitcoindRpcError> = self
            .request(
                "sendrawtransaction",
                vec![serde_json::Value::String(serialize_hex(transaction))],
            )
            .await;
        match sent {
            Ok(_) => BroadcastOutcome::Accepted,
            Err(BitcoindRpcError::RpcError { code, message }) => classify_rejection(code, &message),
            Err(error) => BroadcastOutcome::Rejected(error.to_string()),
        }
    }

    async fn unspent_on_chain(&self, outpoint: OutPoint) -> Result<bool, String> {
        let output: serde_json::Value = self
            .request(
                "gettxout",
                vec![
                    serde_json::Value::String(outpoint.txid.to_string()),
                    serde_json::Value::from(outpoint.vout),
                    serde_json::Value::Bool(false),
                ],
            )
            .await
            .map_err(|error| error.to_string())?;
        Ok(!output.is_null())
    }
}

/// Read a `sendrawtransaction` error as a broadcast outcome.
fn classify_rejection(code: i32, message: &str) -> BroadcastOutcome {
    let missing_inputs = message.contains("issing inputs")
        || message.contains("bad-txns-inputs-missingorspent")
        || message.contains("txn-mempool-conflict");
    match code {
        RPC_VERIFY_ALREADY_IN_CHAIN => BroadcastOutcome::InChain,
        RPC_TRANSACTION_ERROR => BroadcastOutcome::MissingInputs,
        _ if missing_inputs => BroadcastOutcome::MissingInputs,
        _ => BroadcastOutcome::Rejected(format!("RPC error {code}: {message}")),
    }
}

/// Keeps found payout trees and executes them after maturity.
pub struct PayoutTreeExecutor<N: TreeExecutionNode> {
    node: N,
    chain_store_handle: ChainStoreHandle,
    found_rx: PayoutTreeReceiver,
    last_tip: Option<BlockHash>,
    /// New tips in a row each tree has looked settled, by coinbase txid.
    settled_tips: HashMap<Txid, u32>,
}

impl<N: TreeExecutionNode> PayoutTreeExecutor<N> {
    /// An executor talking to `node`, keeping trees in `chain_store_handle`
    /// and receiving them on `found_rx`.
    pub fn new(
        node: N,
        chain_store_handle: ChainStoreHandle,
        found_rx: PayoutTreeReceiver,
    ) -> Self {
        Self {
            node,
            chain_store_handle,
            found_rx,
            last_tip: None,
            settled_tips: HashMap::new(),
        }
    }

    /// Keep every tree sent on the channel and execute on each poll, until
    /// the channel closes.
    pub async fn run(mut self) {
        let mut interval = tokio::time::interval(EXECUTOR_POLL_INTERVAL);
        loop {
            tokio::select! {
                found = self.found_rx.recv() => match found {
                    Some(tree) => self.keep(tree).await,
                    None => break,
                },
                _ = interval.tick() => self.execute_round().await,
            }
        }
        info!("Payout tree executor stopped: the validator's channel closed");
    }

    /// Keep a found block's tree.
    pub async fn keep(&self, tree: StoredPayoutTree) {
        let coinbase_txid = tree.coinbase_txid;
        match self.chain_store_handle.put_payout_tree(tree).await {
            Ok(()) => info!("Kept the payout tree of coinbase {coinbase_txid}"),
            Err(error) => warn!("Failed to keep the payout tree of {coinbase_txid}: {error}"),
        }
    }

    /// On a new BCH tip, broadcast every mature tree not yet executed.
    pub async fn execute_round(&mut self) {
        let tip = match self.node.best_block_hash().await {
            Ok(tip) => tip,
            Err(error) => {
                debug!("Payout tree executor cannot read BCHN's tip: {error}");
                return;
            }
        };
        if self.last_tip == Some(tip) {
            return;
        }
        self.last_tip = Some(tip);
        let trees = match self.chain_store_handle.get_payout_trees() {
            Ok(trees) => trees,
            Err(error) => {
                warn!("Failed to read the kept payout trees: {error}");
                return;
            }
        };
        // An unknown block counts towards expiry only on a synced BCHN: one
        // still downloading the chain does not know blocks it will soon have.
        let synced = matches!(self.node.in_initial_block_download().await, Ok(false));
        for tree in trees.iter().filter(|tree| !tree.executed) {
            self.execute_tree(tree, tip, synced).await;
        }
    }

    /// Broadcast one tree if it is mature, and mark it executed once it has
    /// settled.
    async fn execute_tree(
        &mut self,
        stored: &StoredPayoutTree,
        bitcoin_tip: BlockHash,
        synced: bool,
    ) {
        match self
            .node
            .block_confirmations(stored.bitcoin_blockhash)
            .await
        {
            Ok(Some(confirmations)) if confirmations >= COINBASE_MATURITY_CONFIRMATIONS => {}
            Ok(None) => {
                self.settled_tips.remove(&stored.coinbase_txid);
                if !synced {
                    return;
                }
                match self
                    .chain_store_handle
                    .record_unknown_payout_tree_tip(stored.coinbase_txid, bitcoin_tip)
                    .await
                {
                    Ok(true) => info!(
                        "Expired payout tree of unknown block {} (coinbase {})",
                        stored.bitcoin_blockhash, stored.coinbase_txid
                    ),
                    Ok(false) => {}
                    Err(error) => warn!(
                        "Cannot age payout tree of unknown block {}: {error}",
                        stored.bitcoin_blockhash
                    ),
                }
                return;
            }
            Ok(Some(_)) => {
                self.settled_tips.remove(&stored.coinbase_txid);
                return;
            }
            Err(error) => {
                self.settled_tips.remove(&stored.coinbase_txid);
                debug!(
                    "Cannot read the confirmations of block {}: {error}",
                    stored.bitcoin_blockhash
                );
                return;
            }
        }
        let tree = match stored.tree() {
            Ok(tree) => tree,
            Err(error) => {
                warn!(
                    "Cannot rebuild the payout tree of {}: {error}",
                    stored.coinbase_txid
                );
                return;
            }
        };
        let mut confirmed_variant = false;
        for fee_rate in BCH_TREE_FEE_RATES {
            let Some(transactions) = tree.node_transactions(fee_rate, stored.root_outpoint())
            else {
                continue;
            };
            // Only P2Pool's own rate starts a tree; the others are finished.
            let skip = usize::from(fee_rate != BCH_TREE_EXECUTION_FEE_RATE);
            let mut root_confirmed = false;
            let mut descendants_confirmed = true;
            for (index, transaction) in transactions.iter().enumerate().skip(skip) {
                match self.node.send_raw_transaction(transaction).await {
                    BroadcastOutcome::InChain => {
                        // A confirmed descendant also proves its root was
                        // confirmed, even after every root output is spent.
                        root_confirmed = true;
                    }
                    BroadcastOutcome::Accepted | BroadcastOutcome::MissingInputs => {
                        if index != 0 {
                            descendants_confirmed = false;
                        }
                    }
                    BroadcastOutcome::Rejected(reason) => {
                        if index != 0 {
                            descendants_confirmed = false;
                        }
                        warn!(
                            "BCHN refused payout tree transaction {} of coinbase {}: {reason}",
                            transaction.compute_txid(),
                            stored.coinbase_txid
                        );
                    }
                }
            }
            // An alternate variant with no children is never broadcast by
            // this node. An unspent confirmed output proves it was mined.
            if skip == 1 && transactions.len() == 1 {
                root_confirmed = matches!(
                    self.node
                        .unspent_on_chain(OutPoint::new(transactions[0].compute_txid(), 0))
                        .await,
                    Ok(true)
                );
            }
            confirmed_variant |= root_confirmed && descendants_confirmed;
        }
        let settled = self.settled_tips.entry(stored.coinbase_txid).or_insert(0);
        if !confirmed_variant {
            *settled = 0;
            return;
        }
        *settled += 1;
        if *settled < SETTLED_TIPS {
            return;
        }
        match self
            .chain_store_handle
            .mark_payout_tree_executed(stored.coinbase_txid)
            .await
        {
            Ok(()) => {
                self.settled_tips.remove(&stored.coinbase_txid);
                info!("Payout tree of coinbase {} executed", stored.coinbase_txid);
            }
            Err(error) => warn!(
                "Failed to mark the payout tree of {} executed: {error}",
                stored.coinbase_txid
            ),
        }
    }
}

#[cfg(test)]
mod tests;
