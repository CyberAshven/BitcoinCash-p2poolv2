// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Canonical SAFA state, merge-mining proofs, trees, and payout splits.

use std::collections::HashSet;

use crate::{
    BigInt, LOCKTIME_THRESHOLD, MAX_COINBASE_BRANCH, MAX_SPLIT_BPS, MAX_TREE_HEIGHT,
    MIN_SHARE_SATS, expand_token_target,
};

/// Decode a strictly hexadecimal field and optionally require its byte length.
pub fn from_hex(value: &str, length: Option<usize>, name: &str) -> Result<Vec<u8>, String> {
    let bytes = hex::decode(value).map_err(|_| format!("{name} must be hexadecimal bytes"))?;
    if let Some(length) = length
        && bytes.len() != length
    {
        return Err(format!("{name} must be {length} bytes"));
    }
    Ok(bytes)
}

/// Read a little-endian u32; the caller must provide four bytes at `offset`.
pub fn read_uint32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("four bytes"))
}

/// Read an unsigned little-endian integer without a fixed width.
pub fn read_unsigned(bytes: &[u8]) -> BigInt {
    BigInt::from_bytes_le(bytes)
}

/// SHA256, matching libauth's byte-oriented API.
pub fn sha256_hash(bytes: &[u8]) -> Vec<u8> {
    libauth::sha256.hash(bytes)
}

/// Double SHA256, matching libauth's byte-oriented API.
pub fn hash256(bytes: &[u8]) -> Vec<u8> {
    libauth::hash256(bytes, None)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub thread: u32,
    pub height: u32,
    pub target_bits: u32,
    pub ref_bits: u32,
}

/// Encode the four little-endian u32 fields of a thread's NFT commitment.
pub fn encode_state(state: &State) -> Result<Vec<u8>, String> {
    if state.height >= LOCKTIME_THRESHOLD {
        return Err("state height must be a block height".into());
    }
    expand_token_target(state.target_bits)?;
    let mut bytes = Vec::with_capacity(16);
    bytes.extend_from_slice(&state.thread.to_le_bytes());
    bytes.extend_from_slice(&state.height.to_le_bytes());
    bytes.extend_from_slice(&state.target_bits.to_le_bytes());
    bytes.extend_from_slice(&state.ref_bits.to_le_bytes());
    Ok(bytes)
}

/// Decode exactly 16 bytes of thread state, preserving all raw fields.
pub fn decode_state(bytes: &[u8]) -> Result<State, String> {
    if bytes.len() != 16 {
        return Err("a thread state is 16 bytes".into());
    }
    Ok(State {
        thread: read_uint32(bytes, 0),
        height: read_uint32(bytes, 4),
        target_bits: read_uint32(bytes, 8),
        ref_bits: read_uint32(bytes, 12),
    })
}

/// Category slot from SHA256(02 || salt_le || category), modulo 2^height.
pub fn slot(category: &[u8], salt: u32, height: u8) -> u32 {
    let mut preimage = Vec::with_capacity(5 + category.len());
    preimage.push(2);
    preimage.extend_from_slice(&salt.to_le_bytes());
    preimage.extend_from_slice(category);
    let digest = sha256_hash(&preimage);
    let mask = if height >= 32 {
        u32::MAX
    } else {
        (1u32 << height) - 1
    };
    read_uint32(&digest, 0) & mask
}

/// Domain-separated parent of two auxiliary-tree hashes.
pub fn node_hash(left: &[u8], right: &[u8]) -> Vec<u8> {
    let mut preimage = Vec::with_capacity(1 + left.len() + right.len());
    preimage.push(1);
    preimage.extend_from_slice(left);
    preimage.extend_from_slice(right);
    sha256_hash(&preimage)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Leaf {
    pub category: Vec<u8>,
    pub lock_hash: Vec<u8>,
    pub state_hash: Vec<u8>,
    pub payout_hash: Vec<u8>,
    pub target_bits: u32,
    pub ext: Vec<u8>,
}

/// Encode the 169-byte CTML v2 leaf preimage (all hashes as hashed).
pub fn leaf_preimage(leaf: &Leaf) -> Result<Vec<u8>, String> {
    for (value, name) in [
        (&leaf.category, "category"),
        (&leaf.lock_hash, "lock_hash"),
        (&leaf.state_hash, "state_hash"),
        (&leaf.payout_hash, "payout_hash"),
        (&leaf.ext, "ext"),
    ] {
        if value.len() != 32 {
            return Err(format!("{name} must be 32 bytes"));
        }
    }
    let mut bytes = Vec::with_capacity(169);
    bytes.extend_from_slice(b"CTML\x02");
    bytes.extend_from_slice(&leaf.category);
    bytes.extend_from_slice(&leaf.lock_hash);
    bytes.extend_from_slice(&leaf.state_hash);
    bytes.extend_from_slice(&leaf.payout_hash);
    bytes.extend_from_slice(&leaf.target_bits.to_le_bytes());
    bytes.extend_from_slice(&leaf.ext);
    Ok(bytes)
}

/// SHA256(00 || CTML v2 leaf preimage).
pub fn leaf_hash(leaf: &Leaf) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::with_capacity(170);
    bytes.push(0);
    bytes.extend_from_slice(&leaf_preimage(leaf)?);
    Ok(sha256_hash(&bytes))
}

/// Fold a leaf's branch, using the slot bits from least significant first.
pub fn fold_aux(leaf: &[u8], leaf_slot: u32, branch: &[Vec<u8>]) -> Vec<u8> {
    let mut current = leaf.to_vec();
    let mut index = leaf_slot;
    for sibling in branch {
        current = if index & 1 == 1 {
            node_hash(sibling, &current)
        } else {
            node_hash(&current, sibling)
        };
        index >>= 1;
    }
    current
}

/// Serialize the zero-valued, 53-byte CTMM v2 coinbase output.
pub fn commitment_output(root: &[u8], height: u8, salt: u32) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(53);
    bytes.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 44, 0x6a, 42]);
    bytes.extend_from_slice(b"CTMM\x02");
    bytes.extend_from_slice(root);
    bytes.push(height);
    bytes.extend_from_slice(&salt.to_le_bytes());
    bytes
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeLeaf {
    pub category: Vec<u8>,
    pub hash: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tree {
    pub root: Vec<u8>,
    pub height: u8,
    pub salt: u32,
    pub slots: Vec<u32>,
    pub branches: Vec<Vec<Vec<u8>>>,
}

/// Build the smallest collision-free category tree, choosing the first salt.
pub fn build_tree(leaves: &[TreeLeaf]) -> Result<Tree, String> {
    if leaves.is_empty() {
        return Ok(Tree {
            root: vec![0; 32],
            height: 0,
            salt: 0,
            slots: Vec::with_capacity(0),
            branches: Vec::with_capacity(0),
        });
    }
    if leaves.len() > 16 {
        return Err("at most 16 tokens per coinbase".into());
    }
    let mut categories = HashSet::with_capacity(leaves.len());
    for leaf in leaves {
        if leaf.category.len() != 32 || leaf.hash.len() != 32 {
            return Err("tree categories and hashes must be 32 bytes".into());
        }
        if !categories.insert(&leaf.category) {
            return Err("no salt gives every token its own slot".into());
        }
    }
    let minimum = leaves.len().next_power_of_two().trailing_zeros() as u8;
    for height in minimum..=MAX_TREE_HEIGHT {
        for salt in 0..65_536 {
            let slots: Vec<u32> = leaves
                .iter()
                .map(|leaf| slot(&leaf.category, salt, height))
                .collect();
            let mut distinct = HashSet::with_capacity(slots.len());
            if slots.iter().all(|index| distinct.insert(*index)) {
                return Ok(tree_at_slots(leaves, slots, height, salt));
            }
        }
    }
    Err("no salt gives every token its own slot".into())
}

fn tree_at_slots(leaves: &[TreeLeaf], slots: Vec<u32>, height: u8, salt: u32) -> Tree {
    let mut levels = Vec::with_capacity(usize::from(height) + 1);
    let mut level = vec![vec![0; 32]; 1usize << height];
    for (leaf, index) in leaves.iter().zip(&slots) {
        level[*index as usize] = leaf.hash.clone();
    }
    levels.push(level);
    while levels.last().expect("nonempty levels").len() > 1 {
        let previous = levels.last().expect("nonempty levels");
        let mut parents = Vec::with_capacity(previous.len() / 2);
        for [left, right] in previous.as_chunks::<2>().0 {
            parents.push(node_hash(left, right));
        }
        levels.push(parents);
    }
    let branches = slots
        .iter()
        .map(|slot| {
            let mut index = *slot as usize;
            levels[..levels.len() - 1]
                .iter()
                .map(|level| {
                    let sibling = level[index ^ 1].clone();
                    index >>= 1;
                    sibling
                })
                .collect()
        })
        .collect();
    Tree {
        root: levels.last().expect("nonempty levels")[0].clone(),
        height,
        salt,
        slots,
        branches,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplitCommitment {
    pub donation_bps: u16,
    pub donation_hash: Vec<u8>,
    pub fee_bps: u16,
    pub fee_hash: Vec<u8>,
}

/// Encode and validate a canonical 69-byte donation/fee commitment.
pub fn split_preimage(split: &SplitCommitment) -> Result<Vec<u8>, String> {
    if split.donation_hash.len() != 32 || split.fee_hash.len() != 32 {
        return Err("split hashes must be 32 bytes".into());
    }
    let mut bytes = Vec::with_capacity(69);
    bytes.push(3);
    bytes.extend_from_slice(&split.donation_bps.to_le_bytes());
    bytes.extend_from_slice(&split.donation_hash);
    bytes.extend_from_slice(&split.fee_bps.to_le_bytes());
    bytes.extend_from_slice(&split.fee_hash);
    parse_split(&bytes)?;
    Ok(bytes)
}

/// Read a canonical split; an empty preimage means no split.
pub fn parse_split(bytes: &[u8]) -> Result<Option<SplitCommitment>, String> {
    if bytes.is_empty() {
        return Ok(None);
    }
    if bytes.len() != 69 || bytes[0] != 3 {
        return Err("a split preimage is 69 bytes starting with 03".into());
    }
    let donation_bps = u16::from_le_bytes([bytes[1], bytes[2]]);
    let fee_bps = u16::from_le_bytes([bytes[35], bytes[36]]);
    let donation_hash = bytes[3..35].to_vec();
    let fee_hash = bytes[37..69].to_vec();
    if donation_bps > MAX_SPLIT_BPS
        || fee_bps > MAX_SPLIT_BPS
        || (donation_bps == 0) != donation_hash.iter().all(|byte| *byte == 0)
        || (fee_bps == 0) != fee_hash.iter().all(|byte| *byte == 0)
    {
        return Err("a split slot is not canonical".into());
    }
    if donation_bps == 0 && fee_bps == 0 {
        return Err("an empty split is the zero ext, not a preimage".into());
    }
    Ok(Some(SplitCommitment {
        donation_bps,
        donation_hash,
        fee_bps,
        fee_hash,
    }))
}

/// The leaf extension hash, or zero for an absent split.
pub fn split_ext(preimage: &[u8]) -> Vec<u8> {
    if preimage.is_empty() {
        vec![0; 32]
    } else {
        sha256_hash(preimage)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shares {
    pub miner: u64,
    pub donation: u64,
    pub fee: u64,
}

/// Split the reward donation-first, returning sub-threshold shares to the miner.
/// Rates must have been checked as canonical basis-point values (at most 10,000).
pub fn split_shares(
    reward: u64,
    donation_bps: u16,
    fee_bps: u16,
    numerator: u128,
    tokens: u64,
) -> Shares {
    assert!(
        donation_bps <= MAX_SPLIT_BPS && fee_bps <= MAX_SPLIT_BPS,
        "canonical split rates required"
    );
    let donation = (u128::from(reward) * u128::from(donation_bps) / 10_000) as u64;
    let fee = (u128::from(reward - donation) * u128::from(fee_bps) / 10_000) as u64;
    let keep = |share: u64| {
        let valuable = u128::from(share)
            .checked_mul(numerator)
            .is_none_or(|value| value >= u128::from(tokens) * u128::from(MIN_SHARE_SATS));
        if tokens > 0 && valuable { share } else { 0 }
    };
    let donation = keep(donation);
    let fee = keep(fee);
    Shares {
        miner: reward - donation - fee,
        donation,
        fee,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proof {
    pub header: Vec<u8>,
    pub height: u8,
    pub salt: u32,
    pub aux_branch: Vec<Vec<u8>>,
    pub head: Vec<u8>,
    pub tail: Vec<u8>,
    pub coinbase_branch: Vec<Vec<u8>>,
}

/// Encode one canonical CTMP v2 proof with bounded tree and coinbase branches.
pub fn encode_proof(proof: &Proof) -> Result<Vec<u8>, String> {
    if proof.header.len() != 80
        || proof.height > MAX_TREE_HEIGHT
        || proof.aux_branch.len() != usize::from(proof.height)
        || proof.coinbase_branch.len() > MAX_COINBASE_BRANCH
        || proof.head.len() > 65_535
        || proof.tail.len() > 65_535
        || proof
            .aux_branch
            .iter()
            .chain(&proof.coinbase_branch)
            .any(|sibling| sibling.len() != 32)
    {
        return Err("proof parts exceed the v2 limits".into());
    }
    let mut bytes = Vec::with_capacity(
        95 + proof.head.len()
            + proof.tail.len()
            + 32 * (proof.aux_branch.len() + proof.coinbase_branch.len()),
    );
    bytes.extend_from_slice(b"CTMP\x02");
    bytes.extend_from_slice(&proof.header);
    bytes.push(proof.height);
    bytes.extend_from_slice(&proof.salt.to_le_bytes());
    for sibling in &proof.aux_branch {
        bytes.extend_from_slice(sibling);
    }
    bytes.extend_from_slice(&(proof.head.len() as u16).to_le_bytes());
    bytes.extend_from_slice(&proof.head);
    bytes.extend_from_slice(&(proof.tail.len() as u16).to_le_bytes());
    bytes.extend_from_slice(&proof.tail);
    bytes.push(proof.coinbase_branch.len() as u8);
    for sibling in &proof.coinbase_branch {
        bytes.extend_from_slice(sibling);
    }
    Ok(bytes)
}

struct ProofReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> ProofReader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], String> {
        let end = self
            .offset
            .checked_add(count)
            .ok_or("not a canonical AuxProof v2")?;
        let part = self
            .bytes
            .get(self.offset..end)
            .ok_or("not a canonical AuxProof v2")?;
        self.offset = end;
        Ok(part)
    }

    fn length_prefixed(&mut self) -> Result<Vec<u8>, String> {
        let length = self.take(2)?;
        let count = usize::from(u16::from_le_bytes([length[0], length[1]]));
        Ok(self.take(count)?.to_vec())
    }

    fn branch(&mut self, count: usize) -> Result<Vec<Vec<u8>>, String> {
        let bytes = self.take(count * 32)?;
        Ok(bytes
            .as_chunks::<32>()
            .0
            .iter()
            .map(|hash| hash.to_vec())
            .collect())
    }
}

/// Decode one proof and reject truncation, excessive branches, and trailing bytes.
pub fn decode_proof(bytes: &[u8]) -> Result<Proof, String> {
    let mut reader = ProofReader { bytes, offset: 0 };
    if reader.take(5)? != b"CTMP\x02" {
        return Err("not a canonical AuxProof v2".into());
    }
    let header = reader.take(80)?.to_vec();
    let height = reader.take(1)?[0];
    if height > MAX_TREE_HEIGHT {
        return Err("not a canonical AuxProof v2".into());
    }
    let salt = read_uint32(reader.take(4)?, 0);
    let aux_branch = reader.branch(usize::from(height))?;
    let head = reader.length_prefixed()?;
    let tail = reader.length_prefixed()?;
    let depth = usize::from(reader.take(1)?[0]);
    if depth > MAX_COINBASE_BRANCH {
        return Err("not a canonical AuxProof v2".into());
    }
    let coinbase_branch = reader.branch(depth)?;
    if reader.offset != bytes.len() {
        return Err("not a canonical AuxProof v2".into());
    }
    Ok(Proof {
        header,
        height,
        salt,
        aux_branch,
        head,
        tail,
        coinbase_branch,
    })
}

/// Read the minimal positive BIP34 height from the covenant's fixed coinbase head.
pub fn coinbase_height(head: &[u8]) -> Result<u32, String> {
    let length = usize::from(*head.get(41).ok_or("the coinbase head is malformed")?);
    if head.len() != 47 + length
        || length > 100
        || head[4] != 1
        || head[5..37] != [0; 32]
        || head[37..41] != [0xff; 4]
        || !(1..=0xfc).contains(&head[head.len() - 1])
    {
        return Err("the coinbase head is malformed".into());
    }
    let push = usize::from(head[42]);
    if !(1..=5).contains(&push) || length < 1 + push {
        return Err("the coinbase has no BIP34 height".into());
    }
    let bytes = &head[43..43 + push];
    let last = bytes[bytes.len() - 1];
    if last & 0x80 != 0 || last == 0 && (bytes.len() == 1 || bytes[bytes.len() - 2] & 0x80 == 0) {
        return Err("the BIP34 height is not a minimal positive number".into());
    }
    let height = read_unsigned(bytes)
        .to_u64()
        .ok_or("BIP34 height overflow")?;
    u32::try_from(height).map_err(|_| "BIP34 height overflow".into())
}

/// Fold the coinbase's index-zero Merkle branch using double SHA256.
pub fn coinbase_merkle_root(coinbase: &[u8], branch: &[Vec<u8>]) -> Vec<u8> {
    branch.iter().fold(hash256(coinbase), |current, sibling| {
        let mut bytes = Vec::with_capacity(current.len() + sibling.len());
        bytes.extend_from_slice(&current);
        bytes.extend_from_slice(sibling);
        hash256(&bytes)
    })
}
