// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The Bitcoin Cash payout tree: the coinbase payees that do not fit the
//! coinbase, paid through one P2SH32 output and a few keyless transactions
//! after maturity.
//!
//! A share-chain rule: the producer and every validator build the same tree
//! from the same payees, because the coinbase carries its root. See
//! `docs/architecture/bch-hybrid-payouts.md`, section 5.
//!
//! - **Shape.** The payees, in the order given (canonical payout order), are
//!   grouped `BCH_PAYOUT_TREE_FAN_OUT` to a node; when there is more than one
//!   node, the nodes are grouped the same way, until one root remains. Up to
//!   500 payees take one transaction, up to 250,000 two levels.
//! - **Fees.** Every node transaction is fully determined, so its size is
//!   known when the coinbase is built. At fee rate `R` (satoshis per byte),
//!   with `B` the bytes of every node transaction, `n` payees and
//!   `O = B - sum(s_i)` the bytes that are not payee outputs, payee `i` pays
//!   `R * s_i + ceil(R * O / n)`. Every node but the root pays exactly `R`
//!   times its size; the root pays `R` times its size plus a rounding excess
//!   below `n` satoshis. The fee comes from the tree's own value, never from
//!   whoever broadcasts it.
//! - **Fee rates.** The root accepts the outputs for `R` = 1, 2 and 3
//!   (`BCH_TREE_FEE_RATES`), so a tree can still be relayed if fees rise;
//!   below the root each rate has its own nodes. Every P2Pool node executes at
//!   `BCH_TREE_EXECUTION_FEE_RATE`, 1 satoshi per byte, BCHN's default
//!   minimum relay fee.
//! - **Dust.** A node transaction is relayed, so each of its outputs must
//!   meet BCHN's dust threshold. A payee whose net amount at the highest rate
//!   is below its threshold leaves the tree and its amount goes to the last
//!   payee, the largest; this repeats until no payee but the last is below.

use crate::shares::bch_payout_covenant::{
    NODE_LOCK_TIME, NODE_SEQUENCE, NODE_TRANSACTION_VERSION, NodeCovenant, outputs_hash,
};
use crate::shares::bch_script::{dust_threshold_for_script_length, output_size_for_script_length};
use bitcoin::absolute::LockTime;
use bitcoin::transaction::Version;
use bitcoin::{Amount, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Witness};
use std::fmt;

/// Outputs per node transaction. Provisional until measured at scale; the
/// VM budget at 500 outputs is checked by the covenant tests.
pub const BCH_PAYOUT_TREE_FAN_OUT: usize = 500;

/// The fee rates, in satoshis per byte, a tree's root accepts.
pub const BCH_TREE_FEE_RATES: [u64; 3] = [1, 2, 3];

/// The fee rate every P2Pool node executes trees at: BCHN's default
/// `-minrelaytxfee`, 1,000 satoshis per kilobyte.
pub const BCH_TREE_EXECUTION_FEE_RATE: u64 = 1;

/// The highest fee rate a tree accepts, against which payees are checked for
/// dust.
const MAX_TREE_FEE_RATE: u64 = BCH_TREE_FEE_RATES[BCH_TREE_FEE_RATES.len() - 1];

/// A payee of a payout tree: the script it is paid to and the amount the
/// coinbase owes it before the tree's fee.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreePayee {
    /// Locking script the payee is paid to.
    pub script_pubkey: ScriptBuf,
    /// Satoshis the coinbase owes the payee, before its share of the fee.
    pub amount: u64,
}

impl TreePayee {
    /// Serialized bytes of the payee's output.
    fn output_size(&self) -> usize {
        output_size_for_script_length(self.script_pubkey.len())
    }

    /// BCHN's dust threshold for the payee's output.
    fn dust(&self) -> u64 {
        dust_threshold_for_script_length(self.script_pubkey.len())
    }
}

/// Why a payout tree could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PayoutTreeError {
    /// A tree needs at least one payee.
    NoPayees,
    /// The payees' amounts overflow a satoshi count.
    ValueOverflow,
    /// The fee left for the root is negative: the tree's value cannot cover
    /// its own child nodes. Unreachable for trees built by `build`, kept as a
    /// guard against an arithmetic mistake producing an invalid transaction.
    RootFeeNegative { fee_rate: u64 },
}

impl fmt::Display for PayoutTreeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PayoutTreeError::NoPayees => write!(formatter, "a payout tree needs payees"),
            PayoutTreeError::ValueOverflow => {
                write!(formatter, "payout tree amounts overflow")
            }
            PayoutTreeError::RootFeeNegative { fee_rate } => write!(
                formatter,
                "payout tree value cannot pay its nodes at {fee_rate} sat/byte"
            ),
        }
    }
}

impl std::error::Error for PayoutTreeError {}

/// Where one output of a node goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodeOutput {
    /// The payee at this index.
    Payee(usize),
    /// The node at this index.
    Node(usize),
}

/// The tree's shape: nodes listed root first, each before its children.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TreeShape {
    nodes: Vec<Vec<NodeOutput>>,
}

impl TreeShape {
    /// Group `payee_count` payees `BCH_PAYOUT_TREE_FAN_OUT` to a node, then
    /// the nodes the same way, until one root remains.
    fn for_payees(payee_count: usize) -> Self {
        let mut levels: Vec<Vec<Vec<NodeOutput>>> = Vec::with_capacity(3);
        let mut current: Vec<NodeOutput> = (0..payee_count).map(NodeOutput::Payee).collect();
        loop {
            let groups: Vec<Vec<NodeOutput>> = current
                .chunks(BCH_PAYOUT_TREE_FAN_OUT)
                .map(<[NodeOutput]>::to_vec)
                .collect();
            let group_count = groups.len();
            levels.push(groups);
            if group_count == 1 {
                break;
            }
            // Placeholder indices within the level just pushed; renumbered
            // once every level is known.
            current = (0..group_count).map(NodeOutput::Node).collect();
        }
        // Number nodes root first: the last level is the root, then each level
        // below it in order.
        let mut offsets = Vec::with_capacity(levels.len());
        let mut next = 0;
        for level in levels.iter().rev() {
            offsets.push(next);
            next += level.len();
        }
        offsets.reverse();
        let mut nodes = Vec::with_capacity(next);
        for (level_index, level) in levels.iter().enumerate().rev() {
            for group in level {
                let outputs = group
                    .iter()
                    .map(|output| match output {
                        NodeOutput::Payee(index) => NodeOutput::Payee(*index),
                        NodeOutput::Node(index) => {
                            NodeOutput::Node(offsets[level_index - 1] + index)
                        }
                    })
                    .collect();
                nodes.push(outputs);
            }
        }
        Self { nodes }
    }

    /// Parent of each node and the output index it holds there; `None` for
    /// the root.
    fn parents(&self) -> Vec<Option<(usize, usize)>> {
        let mut parents = vec![None; self.nodes.len()];
        for (parent, outputs) in self.nodes.iter().enumerate() {
            for (output_index, output) in outputs.iter().enumerate() {
                if let NodeOutput::Node(child) = output {
                    parents[*child] = Some((parent, output_index));
                }
            }
        }
        parents
    }
}

/// One node of a tree at one fee rate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeNode {
    covenant: NodeCovenant,
    outputs: Vec<TxOut>,
}

impl TreeNode {
    /// The node's covenant.
    pub fn covenant(&self) -> &NodeCovenant {
        &self.covenant
    }

    /// The outputs the node's transaction pays.
    pub fn outputs(&self) -> &[TxOut] {
        &self.outputs
    }
}

/// A tree's nodes at one fee rate, root first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeVariant {
    fee_rate: u64,
    nodes: Vec<TreeNode>,
}

impl TreeVariant {
    /// Satoshis per byte this variant pays.
    pub fn fee_rate(&self) -> u64 {
        self.fee_rate
    }

    /// The nodes, root first, each before its children.
    pub fn nodes(&self) -> &[TreeNode] {
        &self.nodes
    }
}

/// A payout tree: its payees, and its nodes at every accepted fee rate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayoutTree {
    payees: Vec<TreePayee>,
    value: u64,
    shape: TreeShape,
    variants: Vec<TreeVariant>,
}

impl PayoutTree {
    /// Build the tree for `payees`, in canonical payout order with the
    /// largest last. Merges payees below dust into the last one first.
    pub fn build(payees: Vec<TreePayee>) -> Result<Self, PayoutTreeError> {
        if payees.is_empty() {
            return Err(PayoutTreeError::NoPayees);
        }
        let value = payees
            .iter()
            .try_fold(0u64, |total, payee| total.checked_add(payee.amount))
            .ok_or(PayoutTreeError::ValueOverflow)?;
        let payees = merge_dust_payees(payees);
        let shape = TreeShape::for_payees(payees.len());
        let sizes = node_sizes(&shape, &payees);
        let mut variants = Vec::with_capacity(BCH_TREE_FEE_RATES.len());
        for fee_rate in BCH_TREE_FEE_RATES {
            variants.push(build_variant(&shape, &sizes, &payees, value, fee_rate)?);
        }
        let root_hashes: Vec<[u8; 32]> = variants
            .iter()
            .map(|variant| outputs_hash(&variant.nodes[0].outputs))
            .collect();
        let root_output_count = shape.nodes[0].len();
        for variant in &mut variants {
            variant.nodes[0].covenant =
                NodeCovenant::new(value, root_output_count, root_hashes.clone());
        }
        Ok(Self {
            payees,
            value,
            shape,
            variants,
        })
    }

    /// The payees after dust merging, in canonical order.
    pub fn payees(&self) -> &[TreePayee] {
        &self.payees
    }

    /// Satoshis the tree holds: the value of its coinbase output.
    pub fn value(&self) -> u64 {
        self.value
    }

    /// The root's coinbase output.
    pub fn root_output(&self) -> TxOut {
        self.variants[0].nodes[0].covenant.output()
    }

    /// Node transactions in the tree.
    pub fn node_count(&self) -> usize {
        self.shape.nodes.len()
    }

    /// The tree's nodes at `fee_rate`, or `None` for a rate it does not accept.
    pub fn variant(&self, fee_rate: u64) -> Option<&TreeVariant> {
        self.variants
            .iter()
            .find(|variant| variant.fee_rate == fee_rate)
    }

    /// The tree's node transactions at `fee_rate`, root first and each node
    /// before its children, spending the root from `root_outpoint`.
    pub fn node_transactions(
        &self,
        fee_rate: u64,
        root_outpoint: OutPoint,
    ) -> Option<Vec<Transaction>> {
        let variant = self.variant(fee_rate)?;
        let parents = self.shape.parents();
        let mut transactions: Vec<Transaction> = Vec::with_capacity(variant.nodes.len());
        for (index, node) in variant.nodes.iter().enumerate() {
            let previous_output = match parents[index] {
                None => root_outpoint,
                Some((parent, output_index)) => OutPoint::new(
                    transactions[parent].compute_txid(),
                    u32::try_from(output_index).expect("a node has at most 500 outputs"),
                ),
            };
            transactions.push(node_transaction(
                &node.covenant,
                &node.outputs,
                previous_output,
            ));
        }
        Some(transactions)
    }

    /// Satoshis the whole tree pays in fees at `fee_rate`: its value less
    /// every payee output.
    pub fn total_fee(&self, fee_rate: u64) -> Option<u64> {
        let variant = self.variant(fee_rate)?;
        let paid: u64 = self
            .shape
            .nodes
            .iter()
            .zip(&variant.nodes)
            .flat_map(|(shape_outputs, node)| shape_outputs.iter().zip(&node.outputs))
            .filter(|(shape_output, _)| matches!(shape_output, NodeOutput::Payee(_)))
            .map(|(_, output)| output.value.to_sat())
            .sum();
        Some(self.value - paid)
    }
}

/// The transaction spending a node from `previous_output` into `outputs`.
fn node_transaction(
    covenant: &NodeCovenant,
    outputs: &[TxOut],
    previous_output: OutPoint,
) -> Transaction {
    Transaction {
        version: Version(NODE_TRANSACTION_VERSION),
        lock_time: LockTime::from_consensus(NODE_LOCK_TIME),
        input: vec![TxIn {
            previous_output,
            script_sig: covenant.unlocking_script(),
            sequence: Sequence(NODE_SEQUENCE),
            witness: Witness::new(),
        }],
        output: outputs.to_vec(),
    }
}

/// Serialized bytes of every node transaction, in shape order.
///
/// A node's size depends on its output count and output scripts only: its
/// value is a fixed 8-byte push, its hashes fixed 32-byte pushes, and output
/// values fixed 8 bytes. So it is measured from a transaction with zero
/// values and placeholder hashes.
fn node_sizes(shape: &TreeShape, payees: &[TreePayee]) -> Vec<usize> {
    shape
        .nodes
        .iter()
        .enumerate()
        .map(|(index, outputs)| {
            let hash_count = if index == 0 {
                BCH_TREE_FEE_RATES.len()
            } else {
                1
            };
            let covenant = NodeCovenant::new(0, outputs.len(), vec![[0u8; 32]; hash_count]);
            let placeholder_outputs: Vec<TxOut> = outputs
                .iter()
                .map(|output| match output {
                    NodeOutput::Payee(payee) => TxOut {
                        value: Amount::ZERO,
                        script_pubkey: payees[*payee].script_pubkey.clone(),
                    },
                    NodeOutput::Node(_) => covenant.output(),
                })
                .collect();
            node_transaction(&covenant, &placeholder_outputs, OutPoint::null()).total_size()
        })
        .collect()
}

/// Each payee's share of the fee at `fee_rate` besides its own output:
/// `ceil(R * O / n)`.
fn shared_fee(sizes: &[usize], payees: &[TreePayee], fee_rate: u64) -> u64 {
    let total_bytes: u64 = sizes.iter().map(|size| *size as u64).sum();
    let payee_bytes: u64 = payees.iter().map(|payee| payee.output_size() as u64).sum();
    let other_bytes = total_bytes - payee_bytes;
    (fee_rate * other_bytes).div_ceil(payees.len() as u64)
}

/// What payee `payee` receives at `fee_rate`: its amount less its output's
/// bytes and its share of the rest, never below zero.
fn net_amount(payee: &TreePayee, shared_fee: u64, fee_rate: u64) -> u64 {
    payee
        .amount
        .saturating_sub(fee_rate * payee.output_size() as u64 + shared_fee)
}

/// Remove payees whose net amount at the highest rate is below their dust
/// threshold, adding their amounts to the last payee, until none is left
/// below it. The last payee is never removed, so this ends.
fn merge_dust_payees(mut payees: Vec<TreePayee>) -> Vec<TreePayee> {
    loop {
        let shape = TreeShape::for_payees(payees.len());
        let sizes = node_sizes(&shape, &payees);
        let shared = shared_fee(&sizes, &payees, MAX_TREE_FEE_RATE);
        let last = payees.len() - 1;
        let mut kept = Vec::with_capacity(payees.len());
        let mut merged_amount: u64 = 0;
        for (index, payee) in payees.into_iter().enumerate() {
            if index == last || net_amount(&payee, shared, MAX_TREE_FEE_RATE) >= payee.dust() {
                kept.push(payee);
            } else {
                merged_amount += payee.amount;
            }
        }
        let merged_any = kept.len() <= last;
        let last_payee = kept.last_mut().expect("the last payee is always kept");
        last_payee.amount += merged_amount;
        if !merged_any {
            return kept;
        }
        payees = kept;
    }
}

/// The nodes at `fee_rate`. The root's covenant is a placeholder here: it
/// accepts every rate's outputs and is set once all rates are built.
fn build_variant(
    shape: &TreeShape,
    sizes: &[usize],
    payees: &[TreePayee],
    value: u64,
    fee_rate: u64,
) -> Result<TreeVariant, PayoutTreeError> {
    let shared = shared_fee(sizes, payees, fee_rate);
    let mut nodes: Vec<Option<TreeNode>> = vec![None; shape.nodes.len()];
    // Children come after their parents, so building in reverse order has
    // every child ready before its parent.
    for index in (0..shape.nodes.len()).rev() {
        let outputs: Vec<TxOut> = shape.nodes[index]
            .iter()
            .map(|output| match output {
                NodeOutput::Payee(payee) => TxOut {
                    value: Amount::from_sat(net_amount(&payees[*payee], shared, fee_rate)),
                    script_pubkey: payees[*payee].script_pubkey.clone(),
                },
                NodeOutput::Node(child) => nodes[*child]
                    .as_ref()
                    .expect("children are built before their parents")
                    .covenant
                    .output(),
            })
            .collect();
        let paid: u64 = outputs.iter().map(|output| output.value.to_sat()).sum();
        let node_value = if index == 0 {
            let root_fee = value
                .checked_sub(paid)
                .ok_or(PayoutTreeError::RootFeeNegative { fee_rate })?;
            if root_fee < fee_rate * sizes[0] as u64 && shape.nodes.len() > 1 {
                return Err(PayoutTreeError::RootFeeNegative { fee_rate });
            }
            value
        } else {
            paid + fee_rate * sizes[index] as u64
        };
        let covenant = NodeCovenant::new(node_value, outputs.len(), vec![outputs_hash(&outputs)]);
        nodes[index] = Some(TreeNode { covenant, outputs });
    }
    Ok(TreeVariant {
        fee_rate,
        nodes: nodes
            .into_iter()
            .map(|node| node.expect("every node was built"))
            .collect(),
    })
}

#[cfg(test)]
mod tests;
