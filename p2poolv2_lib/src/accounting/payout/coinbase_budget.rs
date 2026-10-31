// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The Bitcoin Cash hybrid payout: how the PPLNS part of a coinbase becomes
//! direct outputs and, past the coinbase's limit, one payout tree output.
//!
//! A share-chain rule: the producer and every validator build the same
//! outputs. See `docs/architecture/payout-distribution.md`, rule 8, and
//! `docs/architecture/bch-hybrid-payouts.md`, section 3.
//!
//! - **Two kinds of share.** A share whose merge-mining commitment is exactly
//!   the empty one is a no-token share: its coinbase may take Bitcoin Cash's
//!   whole transaction limit, `BCH_MAX_TX_SIZE`. Any other commitment makes a
//!   token share, whose coinbase stays within `BCH_TOKEN_SHARE_COINBASE_LIMIT`
//!   so a token covenant can hash it in the BCH VM.
//! - **Canonical order.** Payees by ascending weight, ties by locking
//!   bytecode, then by address text. The smallest come first.
//! - **The straddle rule.** If every payee's output fits the payout budget
//!   (the limit less `BCH_COINBASE_BYTES_BESIDE_PAYOUTS`, plus the tree
//!   output's bytes, which are not needed), every payee is paid directly.
//!   Otherwise payees are paid directly while their outputs fit the budget;
//!   the first that does not (the straddler) and every payee after it go to
//!   the payout tree (`payout_tree`). Nobody is split and nobody after the
//!   straddler is moved back.
//! - **Amounts.** `amount * weight / total weight`, rounded down; the
//!   remainder goes to the last payee in canonical order, the largest,
//!   wherever it is paid. Direct payouts below dust are paid as they are: BCH
//!   consensus accepts any coinbase output value.

use crate::accounting::OutputPair;
use crate::accounting::payout::payout_distribution::append_proportional_distribution;
use crate::accounting::payout::payout_tree::{
    BCH_PAYOUT_TREE_FAN_OUT, BCH_TREE_FEE_RATES, PayoutTree, PayoutTreeError, TreePayee,
};
use crate::config::Chain;
use crate::shares::bch_asert::U256;
use crate::shares::bch_payout_covenant::{FILLER_BYTES_PER_OUTPUT, NODE_COVENANT_VERSION};
use crate::shares::bch_script::output_size_for_script_length;
use crate::shares::merge_mining::MergeMiningCommitment;
use crate::shares::share_block::{ShareHeader, share_has_token_commitment};
use bitcoin::{Amount, ScriptBuf};
use p2poolv2_wallet::payout_address::PayoutAddress;
use std::collections::HashMap;
use std::error::Error;

/// Bitcoin Cash's largest transaction, coinbase included: `MAX_TX_SIZE` in
/// BCHN's `consensus.h`, checked for the coinbase by `CheckCoinbase`
/// (`bad-txns-oversize`). The limit of a no-token share's coinbase. A BCH
/// consensus number, not a P2Pool one.
pub const BCH_MAX_TX_SIZE: usize = 1_000_000;

/// `T`: the largest coinbase of a token share, in bytes.
///
/// A token claim re-hashes the whole coinbase inside one input of at most
/// 10,000 bytes of unlocking bytecode (P2S, 2026), beside about 2,535 bytes of
/// header, branches and redeem script; 3,072 leaves a margin of about 4,400
/// bytes (`bch-hybrid-payouts.md`, section 4). A share-chain rule carried in
/// the protocol tag: deployed token covenants cannot change, so any raise is
/// a new tag.
pub const BCH_TOKEN_SHARE_COINBASE_LIMIT: usize = 3_072;

/// `S`: the most bytes a P2Pool coinbase takes besides its PPLNS payouts,
/// every field at its worst case.
///
/// Version, input count, outpoint, scriptSig length, sequence, a one-byte
/// output count and lock time (51); BCH's largest scriptSig (100); an output
/// count taking 3 bytes from 253 outputs (2 more); the 53-byte merge-mining
/// record; the tree output (44); the padding before the share commitment (at
/// most 74); and the share commitment (43). There is no donation or fee
/// output: on Bitcoin Cash each window share's donation and fee are payees
/// like any other (`BCH_PER_SHARE_CUTS_RULE_TAG`).
pub const BCH_COINBASE_BYTES_BESIDE_PAYOUTS: usize =
    51 + 100 + 2 + 53 + TREE_OUTPUT_BYTES + 74 + 43;

/// Serialized bytes of a P2PKH output.
pub const P2PKH_OUTPUT_BYTES: usize = 34;

/// Serialized bytes of the tree output, a P2SH32 output.
pub const TREE_OUTPUT_BYTES: usize = 44;

/// A token share's coinbase has fewer outputs than this, so its output count
/// is the single byte a token covenant expects.
pub const MAX_TOKEN_SHARE_COINBASE_OUTPUTS: usize = 253;

const _: () = assert!(BCH_COINBASE_BYTES_BESIDE_PAYOUTS == 367);

/// The share-chain rule tag of the hybrid payout: its limits and its tree.
///
/// Built from the constants, so a node built with another limit, fan-out,
/// fee range, filler or covenant derives another tag and cannot peer with
/// this one: the two would build different coinbases. For the protocol
/// string, in place of the old `payout-cap11`.
pub fn bch_payout_rule_tag() -> String {
    let lowest_rate = BCH_TREE_FEE_RATES[0];
    let highest_rate = BCH_TREE_FEE_RATES[BCH_TREE_FEE_RATES.len() - 1];
    format!(
        "payout-hybrid-t{BCH_TOKEN_SHARE_COINBASE_LIMIT}-f{BCH_PAYOUT_TREE_FAN_OUT}-r{lowest_rate}to{highest_rate}-z{FILLER_BYTES_PER_OUTPUT}-cov{NODE_COVENANT_VERSION}"
    )
}

/// How the PPLNS part of a coinbase becomes outputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayoutSplit {
    /// Every payee gets an output: Bitcoin, upstream's rule.
    EveryPayee,
    /// Bitcoin Cash: direct payouts within `coinbase_limit` bytes of
    /// coinbase, the rest through a payout tree.
    BchHybrid { coinbase_limit: usize },
}

impl PayoutSplit {
    /// The split a chain's share-chain rules use for a share that does
    /// (`token_share`) or does not carry a merge-mining token commitment.
    pub fn for_chain(chain: Chain, token_share: bool) -> Self {
        match chain {
            Chain::Bitcoin => PayoutSplit::EveryPayee,
            Chain::BitcoinCash => PayoutSplit::BchHybrid {
                coinbase_limit: if token_share {
                    BCH_TOKEN_SHARE_COINBASE_LIMIT
                } else {
                    BCH_MAX_TX_SIZE
                },
            },
        }
    }

    /// The coinbase size the split keeps within, on Bitcoin Cash.
    pub fn coinbase_limit(&self) -> Option<usize> {
        match self {
            PayoutSplit::EveryPayee => None,
            PayoutSplit::BchHybrid { coinbase_limit } => Some(*coinbase_limit),
        }
    }
}

/// The split for a Bitcoin Cash coinbase carrying `commitment` at output 0:
/// the token share's limit unless it is the exact empty commitment.
pub fn bch_split_for_commitment(commitment: &MergeMiningCommitment) -> PayoutSplit {
    PayoutSplit::for_chain(Chain::BitcoinCash, !commitment.is_empty())
}

/// The split a share's coinbase follows: the chain's, at the token share's
/// limit when the header's merge-mining commitment names tokens
/// (`share_has_token_commitment`).
pub fn split_for_share(chain: Chain, header: &ShareHeader) -> PayoutSplit {
    PayoutSplit::for_chain(chain, share_has_token_commitment(header))
}

/// Append the PPLNS outputs `split` makes of `amount` for `weights`, and
/// return the payout tree, on Bitcoin Cash when some payees go to one.
///
/// `EveryPayee` is `append_proportional_distribution`. `BchHybrid` is the
/// hybrid payout (see the module docs).
pub(crate) fn append_payout_split(
    weights: &HashMap<PayoutAddress, u128>,
    amount: Amount,
    split: PayoutSplit,
    distribution: &mut Vec<OutputPair>,
) -> Result<Option<PayoutTree>, Box<dyn Error + Send + Sync>> {
    match split {
        PayoutSplit::EveryPayee => {
            append_proportional_distribution(weights, amount, distribution)?;
            Ok(None)
        }
        PayoutSplit::BchHybrid { coinbase_limit } => {
            append_hybrid_distribution(weights, amount, coinbase_limit, distribution)
        }
    }
}

/// One payee in canonical order, with what the split needs to know of it.
struct CanonicalPayee<'weights> {
    address: &'weights PayoutAddress,
    script_pubkey: ScriptBuf,
    weight: u128,
    amount: u64,
}

impl CanonicalPayee<'_> {
    fn output_bytes(&self) -> usize {
        output_size_for_script_length(self.script_pubkey.len())
    }
}

/// The hybrid payout of `amount` over `weights` within `coinbase_limit`.
///
/// Outputs already in `distribution` take their bytes from the limit: none
/// on a Bitcoin Cash share chain, whose donation and fee are payees in
/// `weights`, but Hydrapool's donation and fee outputs on Bitcoin Cash.
///
/// A tree tail too small to pay its own fee at 3 sat/byte and leave its
/// largest payee above dust (`PayoutTreeError::InsufficientValue`) is merged
/// into that payee, as dust is, and paid directly in the tree output's place.
fn append_hybrid_distribution(
    weights: &HashMap<PayoutAddress, u128>,
    amount: Amount,
    coinbase_limit: usize,
    distribution: &mut Vec<OutputPair>,
) -> Result<Option<PayoutTree>, Box<dyn Error + Send + Sync>> {
    let payees = canonical_payees(weights, amount)?;
    let output_bytes: Vec<usize> = payees.iter().map(CanonicalPayee::output_bytes).collect();
    let placed_bytes: usize = distribution
        .iter()
        .map(|output| output_size_for_script_length(output.address.script_pubkey().len()))
        .sum();
    let direct_count =
        direct_payee_count(&output_bytes, coinbase_limit.saturating_sub(placed_bytes));
    let (direct, tree) = payees.split_at(direct_count);
    distribution.extend(direct.iter().map(|payee| OutputPair {
        address: payee.address.clone(),
        amount: Amount::from_sat(payee.amount),
    }));
    if tree.is_empty() {
        return Ok(None);
    }
    let tree_payees = tree
        .iter()
        .map(|payee| TreePayee {
            script_pubkey: payee.script_pubkey.clone(),
            amount: payee.amount,
        })
        .collect();
    match PayoutTree::build(tree_payees) {
        Ok(tree) => Ok(Some(tree)),
        Err(PayoutTreeError::InsufficientValue) => {
            let largest = tree.last().expect("the tree tail is nonempty");
            distribution.push(OutputPair {
                address: largest.address.clone(),
                amount: Amount::from_sat(tree.iter().map(|payee| payee.amount).sum()),
            });
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}

/// The payees in canonical order with their amounts: ascending weight, ties
/// by locking bytecode, then by address text; `amount * weight / total`
/// rounded down, and the remainder to the last.
///
/// Errors on an empty or zero total weight, as rule 7 does, or a total
/// weight that leaves 128 bits. `amount * weight` is taken in 256 bits:
/// per-share cuts express weights in units of `1 / 10^8`, whose product
/// with a satoshi amount can pass 128 bits.
fn canonical_payees(
    weights: &HashMap<PayoutAddress, u128>,
    amount: Amount,
) -> Result<Vec<CanonicalPayee<'_>>, Box<dyn Error + Send + Sync>> {
    let total_weight = weights
        .values()
        .try_fold(0u128, |total, weight| total.checked_add(*weight))
        .ok_or("PPLNS weights overflow when splitting the coinbase")?;
    if total_weight == 0 {
        return Err("Invalid total difficulty (0) when computing proportional payout".into());
    }
    let amount_sats = U256::from(amount.to_sat());
    let mut payees = Vec::with_capacity(weights.len());
    for (address, weight) in weights {
        let pro_rata = (amount_sats * U256::from(*weight) / U256::from(total_weight)).low_u64();
        payees.push(CanonicalPayee {
            address,
            script_pubkey: address.script_pubkey(),
            weight: *weight,
            amount: pro_rata,
        });
    }
    payees.sort_by(|first, second| {
        (first.weight, first.script_pubkey.as_bytes())
            .cmp(&(second.weight, second.script_pubkey.as_bytes()))
            .then_with(|| first.address.to_string().cmp(&second.address.to_string()))
    });
    let paid: u64 = payees.iter().map(|payee| payee.amount).sum();
    let remainder = amount
        .to_sat()
        .checked_sub(paid)
        .ok_or("PPLNS amounts exceed the amount they split")?;
    if let Some(last) = payees.last_mut() {
        last.amount += remainder;
    }
    Ok(payees)
}

/// How many payees, in canonical order, are paid directly in a coinbase of
/// at most `coinbase_limit` bytes (the straddle rule).
///
/// `output_bytes` holds each payee's output size in canonical order. If
/// they all fit the payout budget without a tree output, all are direct.
/// Otherwise the count is the longest prefix fitting the budget with a tree
/// output: the straddler and everyone after it go to the tree.
pub fn direct_payee_count(output_bytes: &[usize], coinbase_limit: usize) -> usize {
    let budget = coinbase_limit.saturating_sub(BCH_COINBASE_BYTES_BESIDE_PAYOUTS);
    let total: usize = output_bytes.iter().sum();
    if total <= budget + TREE_OUTPUT_BYTES {
        return output_bytes.len();
    }
    let mut used = 0;
    for (index, bytes) in output_bytes.iter().enumerate() {
        if used + bytes > budget {
            return index;
        }
        used += bytes;
    }
    output_bytes.len()
}

#[cfg(test)]
mod tests;
