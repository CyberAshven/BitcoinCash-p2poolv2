// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Bounded AuxPoW trees and proofs for the BCH coinbase commitment output.
//!
//! Token adapters supply the work hash, chain id and target from their own
//! covenant rules. This verifies committed work, not token emission rules or
//! acceptance of the parent as a complete BCH block.

use super::coinbase_proof::MAX_COINBASE_MERKLE_BRANCH_LENGTH;
use super::merge_mining::{AUXPOW_MAGIC, MergeMiningCommitment};
use crate::stratum::work::gbt::compute_merkle_root_from_branches;
use bitcoin::block::Header;
use bitcoin::hashes::{Hash, sha256d};
use bitcoin::{Target, Transaction, TxMerkleNode};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

/// Local work-selection budget; the tree itself may be sparse.
pub const MAX_AUXILIARY_CHAINS: usize = 64;
/// The commitment's maximum width is 2^30.
pub const MAX_AUXILIARY_BRANCH_LENGTH: usize = 30;
/// SV2's 65,535 output bytes plus at most 150 non-output coinbase bytes.
pub const MAX_AUXPOW_COINBASE_SIZE: usize = 65_685;

/// A token adapter's independently constructed work commitment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuxiliaryWork {
    pub chain_id: u32,
    pub hash: sha256d::Hash,
}

/// Siblings, leaf first, at the slot determined by the chain id and nonce.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuxiliaryWorkProof {
    pub work: AuxiliaryWork,
    pub branch: Vec<sha256d::Hash>,
}

/// One commitment covering every selected work item, with individual proofs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuxiliaryTree {
    pub commitment: MergeMiningCommitment,
    pub proofs: Vec<AuxiliaryWorkProof>,
}

impl AuxiliaryTree {
    /// Build the smallest collision-free tree, using zero hashes in unused slots.
    /// Sparse levels avoid allocating up to 2^30 leaf slots for just two chains.
    pub fn build(work: &[AuxiliaryWork], nonce: u32) -> Result<Self, &'static str> {
        if work.len() > MAX_AUXILIARY_CHAINS {
            return Err("too many auxiliary chains");
        }
        if work.is_empty() {
            return Ok(Self {
                commitment: MergeMiningCommitment::reserved(),
                proofs: Vec::new(),
            });
        }
        let mut identities = HashSet::with_capacity(work.len());
        if !work.iter().all(|item| identities.insert(item.chain_id)) {
            return Err("duplicate auxiliary chain id");
        }
        let height = (0..=MAX_AUXILIARY_BRANCH_LENGTH)
            .find(|height| {
                let mut slots = HashSet::with_capacity(work.len());
                work.iter()
                    .all(|item| slots.insert(auxiliary_slot(item.chain_id, nonce, 1 << height)))
            })
            .ok_or("auxiliary chain ids collide at every supported tree width")?;
        let tree_size = 1 << height;
        let mut nodes: BTreeMap<u32, sha256d::Hash> = work
            .iter()
            .map(|item| (auxiliary_slot(item.chain_id, nonce, tree_size), item.hash))
            .collect();
        let mut proofs: Vec<_> = work
            .iter()
            .map(|item| AuxiliaryWorkProof {
                work: *item,
                branch: Vec::with_capacity(height),
            })
            .collect();
        let mut empty = sha256d::Hash::all_zeros();
        for level in 0..height {
            for proof in &mut proofs {
                let index = auxiliary_slot(proof.work.chain_id, nonce, tree_size) >> level;
                proof
                    .branch
                    .push(*nodes.get(&(index ^ 1)).unwrap_or(&empty));
            }
            let mut parents = BTreeMap::new();
            for index in nodes.keys() {
                let parent = index / 2;
                parents.entry(parent).or_insert_with(|| {
                    hash_pair(
                        nodes.get(&(parent * 2)).unwrap_or(&empty),
                        nodes.get(&(parent * 2 + 1)).unwrap_or(&empty),
                    )
                });
            }
            nodes = parents;
            empty = hash_pair(&empty, &empty);
        }
        let mut merkle_root = nodes[&0].to_byte_array();
        merkle_root.reverse();
        Ok(Self {
            commitment: MergeMiningCommitment {
                merkle_root,
                tree_size,
                nonce,
            },
            proofs,
        })
    }
}

impl AuxiliaryWorkProof {
    /// Check identity, exact depth, deterministic slot and root against trusted work.
    pub fn verify(
        &self,
        commitment: MergeMiningCommitment,
        expected: AuxiliaryWork,
    ) -> Result<(), &'static str> {
        commitment.validate()?;
        if self.work != expected {
            return Err("auxiliary work differs from the requested work");
        }
        if commitment.tree_size == 0
            || self.branch.len() > MAX_AUXILIARY_BRANCH_LENGTH
            || self.branch.len() != commitment.tree_size.trailing_zeros() as usize
        {
            return Err("auxiliary branch does not match the committed tree width");
        }
        let mut index = auxiliary_slot(expected.chain_id, commitment.nonce, commitment.tree_size);
        let mut current = expected.hash;
        for sibling in &self.branch {
            current = if index & 1 == 0 {
                hash_pair(&current, sibling)
            } else {
                hash_pair(sibling, &current)
            };
            index >>= 1;
        }
        let mut root = current.to_byte_array();
        root.reverse();
        if root != commitment.merkle_root {
            return Err("auxiliary branch does not match the committed root");
        }
        Ok(())
    }
}

/// Full coinbase proof for a token adapter; no SHA256 midstate is trusted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuxPowProof {
    pub parent_header: Header,
    pub coinbase: Transaction,
    pub coinbase_branch: Vec<TxMerkleNode>,
    pub auxiliary: AuxiliaryWorkProof,
}

impl AuxPowProof {
    /// Verify against the work and target supplied by the token's rules.
    /// The parent's advertised nBits is not an authority for the token's target.
    pub fn verify(&self, expected: AuxiliaryWork, target: Target) -> Result<(), &'static str> {
        if target == Target::ZERO {
            return Err("auxiliary target must be nonzero");
        }
        if self.coinbase_branch.len() > MAX_COINBASE_MERKLE_BRANCH_LENGTH
            || self.coinbase.total_size() > MAX_AUXPOW_COINBASE_SIZE
            || !self.coinbase.is_coinbase()
            || !self.coinbase.input[0].witness.is_empty()
            || !(2..=100).contains(&self.coinbase.input[0].script_sig.len())
        {
            return Err("invalid or oversized auxiliary coinbase proof");
        }
        let mut commitment = None;
        for output in &self.coinbase.output {
            let script = output.script_pubkey.as_bytes();
            if script.starts_with(&[0x6a, 44]) && script.get(2..6) == Some(AUXPOW_MAGIC.as_slice())
            {
                let decoded = MergeMiningCommitment::from_output(output)?;
                if commitment.replace(decoded).is_some() {
                    return Err("multiple auxiliary commitments in coinbase");
                }
            }
        }
        self.auxiliary.verify(
            commitment.ok_or("auxiliary commitment missing from coinbase")?,
            expected,
        )?;
        if compute_merkle_root_from_branches(self.coinbase.compute_txid(), &self.coinbase_branch)
            != self.parent_header.merkle_root
        {
            return Err("auxiliary coinbase is not in the parent header");
        }
        if !target.is_met_by(self.parent_header.block_hash()) {
            return Err("parent header does not meet the auxiliary target");
        }
        Ok(())
    }
}

fn hash_pair(left: &sha256d::Hash, right: &sha256d::Hash) -> sha256d::Hash {
    let mut pair = [0; 64];
    pair[..32].copy_from_slice(left.as_byte_array());
    pair[32..].copy_from_slice(right.as_byte_array());
    sha256d::Hash::hash(&pair)
}

fn auxiliary_slot(chain_id: u32, nonce: u32, tree_size: u32) -> u32 {
    nonce
        .wrapping_mul(1_103_515_245)
        .wrapping_add(12_345)
        .wrapping_add(chain_id)
        .wrapping_mul(1_103_515_245)
        .wrapping_add(12_345)
        % tree_size
}

#[cfg(test)]
mod tests;
