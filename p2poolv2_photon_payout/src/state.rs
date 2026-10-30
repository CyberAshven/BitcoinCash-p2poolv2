// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The frozen 336-byte immutable payout state and redeem construction.

use libauth::runtime::bigint::BigInt;
use libauth::{hash160, hash256};

use crate::covenant::{program_hash, redeem_script};
use crate::reference::{TreeNode, allocation, maximum_window_weight, root_depth, weight_bytes};

/// Exact immutable state length, excluding its three-byte push header.
pub const STATE_BYTES: usize = 336;

/// Initial prototype profile; network values are 0 mainnet, 1 testnet,
/// 2 regtest, 3 chipnet and 4 scalenet. All data remains offline.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub version: u16,
    pub mode: u8,
    pub depth: u8,
    pub network: u32,
    /// Raw wire bytes, opposite libauth Token.category's display byte order.
    pub category: [u8; 32],
    pub context: [u8; 32],
    pub program_hash: [u8; 32],
    pub subtree_hash: [u8; 32],
    pub total_weight: BigInt,
    pub lower: BigInt,
    pub upper: BigInt,
    /// Root zero sentinel, or the original positive root reward for a child.
    pub reward: u64,
}

impl State {
    /// Bind a canonical root to its network, token category and proof context.
    pub fn root(
        node: &TreeNode,
        network: u32,
        category: [u8; 32],
        context: [u8; 32],
    ) -> Result<Self, String> {
        node.validate()?;
        let state = Self {
            version: 1,
            mode: 0,
            depth: root_depth(node.leaf_count)?,
            network,
            category,
            context,
            program_hash: program_hash()?,
            subtree_hash: node.hash,
            total_weight: node.weight.clone(),
            lower: BigInt::zero(),
            upper: node.weight.clone(),
            reward: 0,
        };
        state.validate()?;
        Ok(state)
    }

    /// Derive a child's canonical immutable state and pin the original reward.
    pub fn child(&self, node: &TreeNode, lower: &BigInt, reward: u64) -> Result<Self, String> {
        let child = Self {
            version: self.version,
            mode: 1,
            depth: self.depth.checked_sub(1).ok_or("child at depth zero")?,
            network: self.network,
            category: self.category,
            context: self.context,
            program_hash: self.program_hash,
            subtree_hash: node.hash,
            total_weight: self.total_weight.clone(),
            lower: lower.clone(),
            upper: lower.add(&node.weight),
            reward,
        };
        child.validate()?;
        if lower.cmp(&self.lower).is_lt() || child.upper.cmp(&self.upper).is_gt() {
            return Err("child lies outside its parent's interval".into());
        }
        if usize::from(node.leaf_count) > (8usize << child.depth) {
            return Err("child leaf count cannot terminate within remaining depth".into());
        }
        Ok(child)
    }

    /// Reject unsupported state semantics and numeric bounds.
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1 || self.mode > 1 || self.depth > 9 || self.network > 4 {
            return Err("unsupported state version, mode, depth or network".into());
        }
        if self.program_hash != program_hash()? {
            return Err("state does not identify the audited program".into());
        }
        weight_bytes(&self.total_weight)?;
        weight_bytes(&self.lower)?;
        weight_bytes(&self.upper)?;
        if self.total_weight.is_zero()
            || self.total_weight.cmp(&maximum_window_weight()).is_gt()
            || !self.lower.cmp(&self.upper).is_lt()
            || self.upper.cmp(&self.total_weight).is_gt()
        {
            return Err("invalid state total or interval".into());
        }
        if self.mode == 0 {
            if self.reward != 0 || !self.lower.is_zero() || self.upper != self.total_weight {
                return Err("root requires a zero reward sentinel and the full interval".into());
            }
        } else if self.reward == 0 || self.reward > i64::MAX as u64 {
            return Err("child must pin a positive original reward".into());
        }
        Ok(())
    }

    /// Serialize fields byte for byte in the frozen section 5 order.
    pub fn encode(&self) -> Result<[u8; STATE_BYTES], String> {
        self.validate()?;
        let mut bytes = [0; STATE_BYTES];
        bytes[..2].copy_from_slice(&self.version.to_le_bytes());
        bytes[2] = self.mode;
        bytes[3] = self.depth;
        bytes[4..8].copy_from_slice(&self.network.to_le_bytes());
        bytes[8..40].copy_from_slice(&self.category);
        bytes[40..72].copy_from_slice(&self.context);
        bytes[72..104].copy_from_slice(&self.program_hash);
        bytes[104..136].copy_from_slice(&self.subtree_hash);
        bytes[136..200].copy_from_slice(&weight_bytes(&self.total_weight)?);
        bytes[200..264].copy_from_slice(&weight_bytes(&self.lower)?);
        bytes[264..328].copy_from_slice(&weight_bytes(&self.upper)?);
        bytes[328..336].copy_from_slice(&self.reward.to_le_bytes());
        Ok(bytes)
    }

    /// Decode only an exact-length, supported state with checked bounds.
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() != STATE_BYTES {
            return Err("payout state must contain exactly 336 bytes".into());
        }
        let state = Self {
            version: u16::from_le_bytes(bytes[..2].try_into().expect("two bytes")),
            mode: bytes[2],
            depth: bytes[3],
            network: u32::from_le_bytes(bytes[4..8].try_into().expect("four bytes")),
            category: bytes[8..40].try_into().expect("32 bytes"),
            context: bytes[40..72].try_into().expect("32 bytes"),
            program_hash: bytes[72..104].try_into().expect("32 bytes"),
            subtree_hash: bytes[104..136].try_into().expect("32 bytes"),
            total_weight: BigInt::from_bytes_le(&bytes[136..200]),
            lower: BigInt::from_bytes_le(&bytes[200..264]),
            upper: BigInt::from_bytes_le(&bytes[264..328]),
            reward: u64::from_le_bytes(bytes[328..336].try_into().expect("eight bytes")),
        };
        state.validate()?;
        Ok(state)
    }

    /// Build the minimal OP_PUSHDATA2 state prefix and fixed program suffix.
    pub fn redeem(&self) -> Result<Vec<u8>, String> {
        redeem_script(&self.encode()?)
    }

    /// Calculate this node's exact prescribed token amount.
    pub fn amount(&self, root_reward: u64) -> Result<u64, String> {
        if self.mode == 1 && self.reward != root_reward {
            return Err("child reward differs from the original pinned reward".into());
        }
        allocation(root_reward, &self.lower, &self.upper, &self.total_weight)
    }
}

/// Select the parameterized root script hash. Every child uses P2SH32.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootLock {
    P2sh20,
    P2sh32,
}

/// Wrap canonical redeem bytecode in its exact BCH P2SH template.
pub fn script_hash_lock(redeem: &[u8], lock: RootLock) -> Vec<u8> {
    let mut script = Vec::with_capacity(35);
    match lock {
        RootLock::P2sh20 => {
            script.extend_from_slice(&[0xa9, 0x14]);
            script.extend_from_slice(&hash160(redeem, None));
        }
        RootLock::P2sh32 => {
            script.extend_from_slice(&[0xaa, 0x20]);
            script.extend_from_slice(&hash256(redeem, None));
        }
    }
    script.push(0x87);
    script
}
