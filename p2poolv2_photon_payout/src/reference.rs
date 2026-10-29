// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Independent, checked manifest, Merkle-sum tree and allocation reference.

use std::collections::BTreeMap;

use libauth::runtime::bigint::BigInt;

/// The profile admits at most this many distinct positive beneficiaries.
pub const MAX_LEAVES: usize = 4_096;
/// Terminal distribution transactions reveal at most this many leaves.
pub const TERMINAL_LEAVES: usize = 8;
/// BCH MoneyRange, in satoshis.
pub const MAX_MONEY: u64 = 2_100_000_000_000_000;

/// One canonical beneficiary's raw locking script and integer work weight.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Beneficiary {
    pub script: Vec<u8>,
    pub weight: BigInt,
}

/// Canonically sorted, unique, positive beneficiary entries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub leaves: Vec<Beneficiary>,
    pub total_weight: BigInt,
}

/// Maximum scaled window weight even for bootstrap target 2.
///
/// Strict-positive work at that target is 2^256, and section 3's window
/// consumes at most 4,320 bootstrap work units, scaled by exactly ten.
pub fn maximum_window_weight() -> BigInt {
    BigInt::from_u64(43_200).shl(256)
}

/// Encode an unsigned integer in exactly 64 little-endian bytes.
pub fn weight_bytes(weight: &BigInt) -> Result<[u8; 64], String> {
    if weight.is_negative() || weight.bit_length() > 512 {
        return Err("weight exceeds the unsigned 512-bit profile".into());
    }
    let magnitude = weight.to_bytes_le();
    let mut bytes = [0; 64];
    bytes[..magnitude.len()].copy_from_slice(&magnitude);
    Ok(bytes)
}

/// Identify only the three exact beneficiary locking-script templates.
pub fn valid_payout_script(script: &[u8]) -> bool {
    (script.len() == 25 && script[..3] == [0x76, 0xa9, 0x14] && script[23..] == [0x88, 0xac])
        || (script.len() == 23 && script[..2] == [0xa9, 0x14] && script[22] == 0x87)
        || (script.len() == 35 && script[..2] == [0xaa, 0x20] && script[34] == 0x87)
}

/// Profile BCH minimum, using the largest permitted fungible amount prefix.
pub fn committed_minimum(script: &[u8]) -> Result<u64, String> {
    if !valid_payout_script(script) {
        return Err("beneficiary script is not P2PKH, P2SH20 or P2SH32".into());
    }
    // Eight-byte value, CompactSize script length, token marker, category,
    // bitfield and maximum nine-byte amount prefix, plus ordinary spend cost.
    Ok(3 * (8 + 1 + 1 + 32 + 1 + 9 + script.len() as u64 + 148))
}

impl Manifest {
    /// Aggregate duplicate scripts, omit zero totals and sort by raw script.
    pub fn canonicalize(entries: Vec<Beneficiary>) -> Result<Self, String> {
        let mut aggregate = BTreeMap::<Vec<u8>, BigInt>::new();
        for entry in entries {
            if !valid_payout_script(&entry.script) {
                return Err("beneficiary script is not P2PKH, P2SH20 or P2SH32".into());
            }
            weight_bytes(&entry.weight)?;
            let previous = aggregate.entry(entry.script).or_insert_with(BigInt::zero);
            *previous = previous.add(&entry.weight);
            weight_bytes(previous)?;
        }
        let mut leaves = Vec::with_capacity(aggregate.len());
        for (script, weight) in aggregate {
            if !weight.is_zero() {
                leaves.push(Beneficiary { script, weight });
            }
        }
        Self::from_canonical(leaves)
    }

    /// Validate an already canonical manifest; zero leaves and duplicates fail.
    pub fn from_canonical(leaves: Vec<Beneficiary>) -> Result<Self, String> {
        if leaves.is_empty() || leaves.len() > MAX_LEAVES {
            return Err("manifest requires 1 through 4096 positive leaves".into());
        }
        let mut total_weight = BigInt::zero();
        let mut previous_script: Option<&[u8]> = None;
        for leaf in &leaves {
            if !valid_payout_script(&leaf.script) {
                return Err("beneficiary script is not P2PKH, P2SH20 or P2SH32".into());
            }
            weight_bytes(&leaf.weight)?;
            if leaf.weight.is_zero() {
                return Err("canonical manifest contains a zero-weight leaf".into());
            }
            if previous_script.is_some_and(|previous| previous >= leaf.script.as_slice()) {
                return Err("canonical manifest scripts are not unique and sorted".into());
            }
            previous_script = Some(&leaf.script);
            total_weight = total_weight.add(&leaf.weight);
            if total_weight.cmp(&maximum_window_weight()).is_gt() {
                return Err("manifest exceeds the maximum scaled window budget".into());
            }
        }
        Ok(Self {
            leaves,
            total_weight,
        })
    }

    /// Enforce a selected bootstrap target's tighter window budget as well.
    pub fn validate_bootstrap_work(&self, bootstrap_work: &BigInt) -> Result<(), String> {
        if bootstrap_work.is_negative()
            || bootstrap_work.is_zero()
            || bootstrap_work.cmp(&BigInt::from_u64(1).shl(256)).is_gt()
        {
            return Err("invalid bootstrap work".into());
        }
        let budget = bootstrap_work.mul(&BigInt::from_u64(43_200));
        if self.total_weight.cmp(&budget).is_gt() {
            return Err("manifest exceeds the configured bootstrap window budget".into());
        }
        Ok(())
    }

    /// Construct the deterministic binary midpoint Merkle-sum tree.
    pub fn tree(&self) -> Result<TreeNode, String> {
        Self::from_canonical(self.leaves.clone())?;
        TreeNode::from_leaves(&self.leaves)
    }
}

/// A leaf or deterministic midpoint pair. Branches also exist within buckets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Leaf(Beneficiary),
    Branch(Box<TreeNode>, Box<TreeNode>),
}

/// One authenticated Merkle-sum node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeNode {
    pub hash: [u8; 32],
    pub weight: BigInt,
    pub leaf_count: u16,
    pub kind: NodeKind,
}

/// Domain-separated leaf commitment with a minimal one-byte CompactSize.
pub fn leaf_hash(leaf: &Beneficiary) -> Result<[u8; 32], String> {
    if !valid_payout_script(&leaf.script) || leaf.weight.is_zero() {
        return Err("invalid leaf script or zero weight".into());
    }
    let mut preimage = Vec::with_capacity(2 + leaf.script.len() + 64);
    preimage.push(0);
    preimage.push(leaf.script.len() as u8);
    preimage.extend_from_slice(&leaf.script);
    preimage.extend_from_slice(&weight_bytes(&leaf.weight)?);
    Ok(libauth::sha256
        .hash(&preimage)
        .try_into()
        .expect("SHA256 has 32 bytes"))
}

/// Domain-separated internal commitment; counts are two-byte little endian.
pub fn branch_hash(left: &TreeNode, right: &TreeNode) -> Result<[u8; 32], String> {
    let mut preimage = Vec::with_capacity(197);
    preimage.push(1);
    preimage.extend_from_slice(&left.descriptor()?);
    preimage.extend_from_slice(&right.descriptor()?);
    Ok(libauth::sha256
        .hash(&preimage)
        .try_into()
        .expect("SHA256 has 32 bytes"))
}

impl TreeNode {
    /// Verify all public descriptors against canonical leaves and midpoint shape.
    pub fn validate(&self) -> Result<(), String> {
        let leaves = self.leaves().into_iter().cloned().collect();
        let canonical = Manifest::from_canonical(leaves)?.tree()?;
        if *self != canonical {
            return Err("tree descriptors differ from canonical leaf commitments".into());
        }
        Ok(())
    }

    fn from_leaves(leaves: &[Beneficiary]) -> Result<Self, String> {
        if leaves.len() == 1 {
            let leaf = &leaves[0];
            Ok(Self {
                hash: leaf_hash(leaf)?,
                weight: leaf.weight.clone(),
                leaf_count: 1,
                kind: NodeKind::Leaf(leaf.clone()),
            })
        } else {
            let midpoint = leaves.len() / 2;
            let left = Self::from_leaves(&leaves[..midpoint])?;
            let right = Self::from_leaves(&leaves[midpoint..])?;
            let weight = left.weight.add(&right.weight);
            weight_bytes(&weight)?;
            Ok(Self {
                hash: branch_hash(&left, &right)?,
                weight,
                leaf_count: leaves.len() as u16,
                kind: NodeKind::Branch(Box::new(left), Box::new(right)),
            })
        }
    }

    /// Hash, weight and count encoding authenticated by an internal parent.
    pub fn descriptor(&self) -> Result<Vec<u8>, String> {
        if self.leaf_count == 0 || usize::from(self.leaf_count) > MAX_LEAVES {
            return Err("node leaf count exceeds profile".into());
        }
        let mut bytes = Vec::with_capacity(98);
        bytes.extend_from_slice(&self.hash);
        bytes.extend_from_slice(&weight_bytes(&self.weight)?);
        bytes.extend_from_slice(&self.leaf_count.to_le_bytes());
        Ok(bytes)
    }

    /// Reveal canonical leaves in script order without exposing branch details.
    pub fn leaves(&self) -> Vec<&Beneficiary> {
        let mut leaves = Vec::with_capacity(usize::from(self.leaf_count));
        self.collect_leaves(&mut leaves);
        leaves
    }

    fn collect_leaves<'a>(&'a self, leaves: &mut Vec<&'a Beneficiary>) {
        match &self.kind {
            NodeKind::Leaf(leaf) => leaves.push(leaf),
            NodeKind::Branch(left, right) => {
                left.collect_leaves(leaves);
                right.collect_leaves(leaves);
            }
        }
    }

    /// Packed witness consumed by the fixed covenant, without the redeem push.
    pub fn witness(&self) -> Result<Vec<u8>, String> {
        let mut witness = Vec::with_capacity(2 + if self.leaf_count > 8 { 196 } else { 800 });
        witness.extend_from_slice(&self.leaf_count.to_le_bytes());
        if self.leaf_count > 8 {
            match &self.kind {
                NodeKind::Branch(left, right) => {
                    witness.extend_from_slice(&left.descriptor()?);
                    witness.extend_from_slice(&right.descriptor()?);
                }
                NodeKind::Leaf(_) => return Err("internal count has no child pair".into()),
            }
        } else {
            for leaf in self.leaves() {
                witness.push(leaf.script.len() as u8);
                witness.extend_from_slice(&leaf.script);
                witness.extend_from_slice(&weight_bytes(&leaf.weight)?);
            }
        }
        Ok(witness)
    }
}

/// Exact initial distribution depth, ceil(log2(ceil(n / 8))).
pub fn root_depth(leaf_count: u16) -> Result<u8, String> {
    if leaf_count == 0 || usize::from(leaf_count) > MAX_LEAVES {
        return Err("root requires 1 through 4096 leaves".into());
    }
    let bucket_count = u32::from(leaf_count).div_ceil(TERMINAL_LEAVES as u32);
    Ok((u32::BITS - (bucket_count - 1).leading_zeros()) as u8)
}

/// Exact telescoping token allocation, with checked unsigned 575-bit products.
pub fn allocation(
    reward: u64,
    lower: &BigInt,
    upper: &BigInt,
    total: &BigInt,
) -> Result<u64, String> {
    if reward == 0 || reward > i64::MAX as u64 {
        return Err("reward must be 1 through 2^63 - 1".into());
    }
    weight_bytes(lower)?;
    weight_bytes(upper)?;
    weight_bytes(total)?;
    if total.is_zero() || !lower.cmp(upper).is_lt() || upper.cmp(total).is_gt() {
        return Err("allocation interval must satisfy 0 <= lower < upper <= total".into());
    }
    let reward_number = BigInt::from_u64(reward);
    let lower_product = lower.mul(&reward_number);
    let upper_product = upper.mul(&reward_number);
    if lower_product.bit_length() > 575 || upper_product.bit_length() > 575 {
        return Err("allocation product exceeds unsigned 575-bit intermediate".into());
    }
    let lower_floor = lower_product.div_mod(total).0;
    let upper_floor = upper_product.div_mod(total).0;
    upper_floor
        .sub(&lower_floor)
        .to_u64()
        .ok_or_else(|| "allocation exceeds u64".into())
}
