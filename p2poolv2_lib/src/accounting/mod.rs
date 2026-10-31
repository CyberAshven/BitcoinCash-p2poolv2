// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

pub mod calc;
pub mod payout;
pub mod stats;

use bitcoin::TxOut;
use payout::payout_tree::PayoutTree;

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputPair {
    pub address: p2poolv2_wallet::payout_address::PayoutAddress,
    pub amount: bitcoin::Amount,
}

/// What a coinbase pays besides P2Pool's own outputs (the merge-mining
/// record, the padding and the share commitment): the donation, the fee and
/// the direct payees in coinbase order, and on Bitcoin Cash the payout tree
/// for the payees past the coinbase's limit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CoinbasePayouts {
    /// Donation, operator fee and direct payees, in coinbase order.
    pub outputs: Vec<OutputPair>,
    /// The payout tree whose root output follows the direct payees, if any.
    pub tree: Option<PayoutTree>,
}

impl From<Vec<OutputPair>> for CoinbasePayouts {
    fn from(outputs: Vec<OutputPair>) -> Self {
        Self::direct(outputs)
    }
}

impl CoinbasePayouts {
    /// Payouts with no tree, as on Bitcoin.
    pub fn direct(outputs: Vec<OutputPair>) -> Self {
        Self {
            outputs,
            tree: None,
        }
    }

    /// The tree's root output, which the coinbase carries after the direct
    /// payees.
    pub fn tree_output(&self) -> Option<TxOut> {
        self.tree.as_ref().map(PayoutTree::root_output)
    }
}
