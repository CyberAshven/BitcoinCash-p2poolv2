// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Merge-mining common format v2: token leaves, the aux tree under a
//! coinbase's output-0 commitment ([`MergeMiningCommitment`]), and
//! `AuxProof` v2, the proof a token claim carries.
//!
//! The format was agreed between BCH P2Pool and Pickaxe so both build and
//! check the same bytes; Pickaxe's golden vectors are in this module's
//! tests. All integers are little-endian, `SHA256` is single SHA-256 and
//! `HASH256` double SHA-256, and hashes are written as hashed.
//!
//! - Slot: `u32le(SHA256(0x02 || salt || category)[0..4]) mod 2^h`.
//! - Leaf hash: `SHA256(0x00 || preimage)`, the 169-byte preimage of
//!   [`TokenLeaf::preimage`].
//! - Node: `SHA256(0x01 || left || right)`; at level `i`, bit `i` of the slot
//!   set means the current node is the right child.
//! - An empty leaf slot is 32 zero bytes; internal nodes are always hashed
//!   from their children.
//!
//! The proof never carries output 0 or the leaf: a verifier rebuilds the
//! leaf from the token's own state and output 0 from the root it folds, so
//! a proof cannot pick its own token or commitment.
//!
//! This checks merge-mined work, not token issuance rules or that the parent
//! is a complete valid BCH block; BCHN is the authority on that.

use super::bch_asert::{BchAsert, U256, bip34_height};
use super::merge_mining::{
    MAX_MERGE_MINING_TREE_HEIGHT, MERGE_MINING_OUTPUT_LENGTH, MergeMiningCommitment,
};
use bitcoin::block::Header;
use bitcoin::consensus::{deserialize, serialize};
use bitcoin::hashes::{Hash, sha256, sha256d};
use bitcoin::{Transaction, TxMerkleNode};
use std::fmt;

/// At most this many tokens in one coinbase's tree.
pub const MAX_TOKEN_LEAVES: usize = 16;

/// Salts the tree builder tries at each height, `0..65,536`, as Pickaxe's
/// builder does, so both pick the same layout. A verifier accepts any salt.
pub const LAYOUT_SALTS: u32 = 1 << 16;

/// Bytes in a token leaf's preimage.
pub const TOKEN_LEAF_LENGTH: usize = 169;

/// The deepest coinbase branch an `AuxProof` v2 carries: `2^25`
/// transactions. It belongs to the proof format, agreed with Pickaxe; a BCH
/// block deeper than that is still a valid share, it just cannot be
/// token-claimed. P2Pool's own coinbase proofs allow more
/// (`MAX_COINBASE_MERKLE_BRANCH_LENGTH`).
pub const MAX_AUX_PROOF_COINBASE_BRANCH_LENGTH: usize = 25;

const LEAF_MAGIC: [u8; 4] = *b"CTML";
const LEAF_VERSION: u8 = 0x02;
const PROOF_MAGIC: [u8; 4] = *b"CTMP";
const PROOF_VERSION: u8 = 0x02;

/// Domain bytes of a leaf, a node and a slot, so none can pose as another.
const LEAF_PREFIX: u8 = 0x00;
const NODE_PREFIX: u8 = 0x01;
const SLOT_PREFIX: u8 = 0x02;
/// Domain byte of a split preimage, the fourth domain beside the leaf, node
/// and slot ones.
const SPLIT_PREFIX: u8 = 0x03;

/// Bytes in a split preimage: the prefix and two slots of a `u16` rate and a
/// 32-byte script hash.
pub const TOKEN_SPLIT_PREIMAGE_LENGTH: usize = 1 + 2 * (2 + 32);

/// The largest split rate, in hundredths of a percent: the whole reward.
pub const MAX_SPLIT_BPS: u16 = 10_000;

/// A coinbase's bytes 4..41: one input spending the null outpoint.
const NULL_INPUT: [u8; 37] = {
    let mut input = [0xff; 37];
    input[0] = 1;
    let mut index = 1;
    while index < 33 {
        input[index] = 0;
        index += 1;
    }
    input
};

/// A coinbase scriptSig is at most 100 bytes (BCH consensus, `bad-cb-length`).
const MAX_COINBASE_SCRIPT_SIG: usize = 100;

/// A proof's bytes besides its branches and coinbase parts: magic, version,
/// header, `h`, salt, two part lengths and the branch depth.
const PROOF_FIXED_LENGTH: usize = 4 + 1 + 80 + 1 + 4 + 2 + 2 + 1;

/// Why a tree could not be built or a proof does not verify.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuxError {
    /// More than [`MAX_TOKEN_LEAVES`] tokens.
    TooManyTokens,
    /// One token category listed twice: one leaf per category.
    DuplicateCategory,
    /// No salt gives every category its own slot up to height 16.
    NoLayout,
    /// The bytes are not exactly one canonical v2 proof.
    Decode,
    /// The proof's parts do not fit their limits.
    Shape,
    /// The tree is taller than the token allows.
    Height,
    /// The token's compact target is not canonical.
    Target,
    /// The coinbase does not start with the fixed-shape head.
    CoinbaseHead,
    /// The coinbase tail has no room for the locktime.
    CoinbaseTail,
    /// Output 0 of the coinbase is not the commitment.
    Commitment,
    /// The coinbase does not reach the header's merkle root.
    MerkleRoot,
    /// The header's hash does not meet the token's target.
    Work,
    /// The header's nBits fail the ASERT format check.
    Asert(String),
    /// A token split, or its preimage, is not canonical.
    Split,
    /// A leaf disclosure (`CTMD`) is not canonical or does not make the
    /// root it discloses, for the reason given.
    Disclosure(&'static str),
}

impl fmt::Display for AuxError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManyTokens => {
                write!(formatter, "at most {MAX_TOKEN_LEAVES} tokens per coinbase")
            }
            Self::DuplicateCategory => write!(formatter, "a token category is listed twice"),
            Self::NoLayout => write!(formatter, "no salt gives every token its own slot"),
            Self::Decode => write!(formatter, "not a canonical merge-mining v2 proof"),
            Self::Shape => write!(formatter, "merge-mining proof parts exceed their limits"),
            Self::Height => write!(
                formatter,
                "merge-mining tree is taller than the token allows"
            ),
            Self::Target => write!(formatter, "invalid token target"),
            Self::CoinbaseHead => write!(formatter, "merge-mining coinbase head is malformed"),
            Self::CoinbaseTail => write!(formatter, "merge-mining coinbase tail is too short"),
            Self::Commitment => write!(formatter, "coinbase output 0 is not the commitment"),
            Self::MerkleRoot => {
                write!(formatter, "merge-mining coinbase does not reach the header")
            }
            Self::Work => write!(formatter, "header does not meet the token target"),
            Self::Asert(reason) => write!(formatter, "header fails the ASERT check: {reason}"),
            Self::Split => write!(formatter, "not a canonical token split"),
            Self::Disclosure(reason) => write!(formatter, "{reason}"),
        }
    }
}

impl std::error::Error for AuxError {}

/// A token's leaf, as its covenant rebuilds it from introspection.
///
/// ```text
/// off len field
/// 0   4   "CTML"
/// 4   1   02
/// 5   32  category      the token category
/// 37  32  lock_hash     HASH256(the token baton's locking bytecode)
/// 69  32  state_hash    token-defined; changes only on a successful claim
/// 101 32  payout_hash   HASH256(the payout output's locking bytecode)
/// 133 4   target_bits   the token's compact target, u32 LE
/// 137 32  ext           token-defined, or 32 zero bytes
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenLeaf {
    pub category: [u8; 32],
    pub lock_hash: [u8; 32],
    pub state_hash: [u8; 32],
    pub payout_hash: [u8; 32],
    pub target_bits: u32,
    pub ext: [u8; 32],
}

impl TokenLeaf {
    /// The leaf of a token in state `state_hash` behind a baton locked by
    /// `baton_locking_bytecode`, paying `payout_locking_bytecode`.
    pub fn new(
        category: [u8; 32],
        baton_locking_bytecode: &[u8],
        state_hash: [u8; 32],
        payout_locking_bytecode: &[u8],
        target_bits: u32,
        ext: [u8; 32],
    ) -> Self {
        Self {
            category,
            lock_hash: hash256(baton_locking_bytecode),
            state_hash,
            payout_hash: hash256(payout_locking_bytecode),
            target_bits,
            ext,
        }
    }

    /// The 169-byte preimage.
    pub fn preimage(&self) -> [u8; TOKEN_LEAF_LENGTH] {
        let mut bytes = [0; TOKEN_LEAF_LENGTH];
        bytes[..4].copy_from_slice(&LEAF_MAGIC);
        bytes[4] = LEAF_VERSION;
        bytes[5..37].copy_from_slice(&self.category);
        bytes[37..69].copy_from_slice(&self.lock_hash);
        bytes[69..101].copy_from_slice(&self.state_hash);
        bytes[101..133].copy_from_slice(&self.payout_hash);
        bytes[133..137].copy_from_slice(&self.target_bits.to_le_bytes());
        bytes[137..].copy_from_slice(&self.ext);
        bytes
    }

    /// Read a 169-byte preimage with the `CTML` magic and version 2; any
    /// other bytes are not a leaf. The target and `ext` are read as they
    /// are: a covenant, not the leaf format, judges them.
    pub fn from_preimage(bytes: &[u8]) -> Result<Self, AuxError> {
        let bytes: &[u8; TOKEN_LEAF_LENGTH] = bytes
            .try_into()
            .map_err(|_| AuxError::Disclosure("a token leaf is 169 bytes"))?;
        if bytes[..4] != LEAF_MAGIC || bytes[4] != LEAF_VERSION {
            return Err(AuxError::Disclosure("not a merge-mining v2 token leaf"));
        }
        let hash = |at: usize| -> [u8; 32] {
            bytes[at..at + 32]
                .try_into()
                .expect("32 bytes within the preimage")
        };
        Ok(Self {
            category: hash(5),
            lock_hash: hash(37),
            state_hash: hash(69),
            payout_hash: hash(101),
            target_bits: u32::from_le_bytes([bytes[133], bytes[134], bytes[135], bytes[136]]),
            ext: hash(137),
        })
    }

    /// `SHA256(0x00 || preimage)`.
    pub fn hash(&self) -> [u8; 32] {
        let mut engine = sha256::Hash::engine();
        bitcoin::hashes::HashEngine::input(&mut engine, &[LEAF_PREFIX]);
        bitcoin::hashes::HashEngine::input(&mut engine, &self.preimage());
        sha256::Hash::from_engine(engine).to_byte_array()
    }
}

/// One slot of a token's split: a rate in hundredths of a percent and
/// `HASH256` of the recipient's locking bytecode, or empty (rate 0, zero
/// hash).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SplitSlot {
    bps: u16,
    script_hash: [u8; 32],
}

impl SplitSlot {
    /// No share.
    pub const EMPTY: Self = Self {
        bps: 0,
        script_hash: [0; 32],
    };

    /// `bps` of what is left to the locking bytecode `script`; empty at 0.
    pub fn new(bps: u16, script: &[u8]) -> Result<Self, AuxError> {
        if bps == 0 {
            return Ok(Self::EMPTY);
        }
        Self::from_hash(bps, hash256(script))
    }

    /// A slot from its rate and script hash, checked canonical: a rate of
    /// at most 10,000, a zero hash exactly when the rate is 0.
    pub fn from_hash(bps: u16, script_hash: [u8; 32]) -> Result<Self, AuxError> {
        if bps > MAX_SPLIT_BPS || (bps == 0) != (script_hash == [0; 32]) {
            return Err(AuxError::Split);
        }
        Ok(Self { bps, script_hash })
    }

    pub fn bps(&self) -> u16 {
        self.bps
    }

    pub fn script_hash(&self) -> &[u8; 32] {
        &self.script_hash
    }
}

/// How each win of a token is shared, which its leaf's `ext` commits to: a
/// donation slot, then a fee slot.
///
/// ```text
/// ext = 00 x 32                                            no split
/// ext = SHA256(03 || d_bps u16le || d_hash || f_bps u16le || f_hash)
/// ```
///
/// The 69-byte preimage is canonical (see [`SplitSlot::from_hash`]), and two
/// empty slots are the zero `ext`, never a preimage. The claim reveals the
/// preimage and the token's covenant pays it; the shares are token-level
/// rules, not P2Pool's. Pickaxe's common format v2, "Split".
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenSplit {
    pub donation: SplitSlot,
    pub fee: SplitSlot,
}

impl TokenSplit {
    /// True when neither slot takes a share.
    pub fn is_none(&self) -> bool {
        self.donation == SplitSlot::EMPTY && self.fee == SplitSlot::EMPTY
    }

    /// The 69-byte preimage.
    pub fn preimage(&self) -> [u8; TOKEN_SPLIT_PREIMAGE_LENGTH] {
        let mut bytes = [0; TOKEN_SPLIT_PREIMAGE_LENGTH];
        bytes[0] = SPLIT_PREFIX;
        bytes[1..3].copy_from_slice(&self.donation.bps.to_le_bytes());
        bytes[3..35].copy_from_slice(&self.donation.script_hash);
        bytes[35..37].copy_from_slice(&self.fee.bps.to_le_bytes());
        bytes[37..].copy_from_slice(&self.fee.script_hash);
        bytes
    }

    /// The leaf's `ext`: 32 zero bytes without a split, else the
    /// preimage's SHA-256.
    pub fn ext(&self) -> [u8; 32] {
        if self.is_none() {
            [0; 32]
        } else {
            sha256::Hash::hash(&self.preimage()).to_byte_array()
        }
    }

    /// Read a revealed preimage as a covenant does: exactly 69 bytes, the
    /// prefix, two canonical slots, not both empty.
    pub fn parse(preimage: &[u8]) -> Result<Self, AuxError> {
        let bytes: &[u8; TOKEN_SPLIT_PREIMAGE_LENGTH] =
            preimage.try_into().map_err(|_| AuxError::Split)?;
        if bytes[0] != SPLIT_PREFIX {
            return Err(AuxError::Split);
        }
        let slot = |at: usize| {
            SplitSlot::from_hash(
                u16::from_le_bytes([bytes[at], bytes[at + 1]]),
                bytes[at + 2..at + 34].try_into().expect("32-byte hash"),
            )
        };
        let split = Self {
            donation: slot(1)?,
            fee: slot(35)?,
        };
        if split.is_none() {
            return Err(AuxError::Split);
        }
        Ok(split)
    }
}

/// The token's target from canonical compact bits: `3 <= exp <= 32` and
/// `0 < mantissa < 0x800000`, stricter than a header's nBits so a covenant
/// expands it with one shift. Little-endian.
pub fn expand_token_target(bits: u32) -> Option<[u8; 32]> {
    let exponent = (bits >> 24) as usize;
    let mantissa = bits & 0x00ff_ffff;
    if !(3..=32).contains(&exponent) || mantissa == 0 || mantissa >= 0x0080_0000 {
        return None;
    }
    let mut target = [0; 32];
    target[exponent - 3..exponent].copy_from_slice(&mantissa.to_le_bytes()[..3]);
    Some(target)
}

/// The easiest canonical token target, `0x7fffff << 232` (bits `0x207fffff`).
pub const MAX_TOKEN_TARGET_BITS: u32 = 0x207f_ffff;

/// Canonical compact bits for a token target from 1 to `0x7fffff << 232`,
/// rounded down: the exponent is the target's length as a VM number (its
/// bytes plus one when the top bit is set), at least 3. The inverse of
/// [`expand_token_target`], as the SAFA covenant and `encode_token_target`
/// of `p2poolv2_safa` encode it. `None` outside that range.
pub fn encode_token_target(target: U256) -> Option<u32> {
    let maximum = U256::from_little_endian(&expand_token_target(MAX_TOKEN_TARGET_BITS)?);
    if target.is_zero() || target > maximum {
        return None;
    }
    let size = (target.bits() / 8 + 1).max(3);
    let mantissa = (target >> (8 * (size - 3))).low_u32();
    Some(((size as u32) << 24) | mantissa)
}

/// The slot of `category` in a tree of height `height`, at most 16.
pub fn slot(category: &[u8; 32], salt: u32, height: u8) -> u32 {
    let mut seed = [0; 37];
    seed[0] = SLOT_PREFIX;
    seed[1..5].copy_from_slice(&salt.to_le_bytes());
    seed[5..].copy_from_slice(category);
    let digest = sha256::Hash::hash(&seed).to_byte_array();
    let value = u32::from_le_bytes([digest[0], digest[1], digest[2], digest[3]]);
    value & (1u32 << height.min(MAX_MERGE_MINING_TREE_HEIGHT)).wrapping_sub(1)
}

/// `SHA256(0x01 || left || right)`.
pub fn node_hash(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut pair = [0; 65];
    pair[0] = NODE_PREFIX;
    pair[1..33].copy_from_slice(left);
    pair[33..].copy_from_slice(right);
    sha256::Hash::hash(&pair).to_byte_array()
}

/// Fold a leaf hash at `slot` up its `branch` to the root.
pub fn fold(leaf: [u8; 32], slot: u32, branch: &[[u8; 32]]) -> [u8; 32] {
    let mut current = leaf;
    let mut index = slot;
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

/// One token's place in a tree: its slot and the siblings from its leaf up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenBranch {
    pub category: [u8; 32],
    pub slot: u32,
    pub branch: Vec<[u8; 32]>,
}

/// The commitment for a set of tokens and each token's branch, in the order
/// the tokens were given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuxiliaryTree {
    pub commitment: MergeMiningCommitment,
    pub branches: Vec<TokenBranch>,
}

/// Where a tree puts its tokens' leaves: its height `h`, its salt and each
/// category's slot, in the order the categories were given. It depends only
/// on the categories, so a set of tokens searches it once and every job's
/// tree, whatever its leaves bind, reuses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeLayout {
    pub height: u8,
    pub salt: u32,
    pub slots: Vec<u32>,
}

impl TreeLayout {
    /// The builder's layout for `categories`: the smallest height that
    /// holds them, the first salt below [`LAYOUT_SALTS`] that gives every
    /// category its own slot, else the next height, up to `max_height` (at
    /// most 16), as Pickaxe's builder searches. No categories take height 0
    /// and salt 0.
    pub fn search(categories: &[[u8; 32]], max_height: u8) -> Result<Self, AuxError> {
        if categories.len() > MAX_TOKEN_LEAVES {
            return Err(AuxError::TooManyTokens);
        }
        if categories
            .iter()
            .enumerate()
            .any(|(index, category)| categories[..index].contains(category))
        {
            return Err(AuxError::DuplicateCategory);
        }
        let minimum_height = categories.len().next_power_of_two().trailing_zeros() as u8;
        for height in minimum_height..=max_height.min(MAX_MERGE_MINING_TREE_HEIGHT) {
            for salt in 0..LAYOUT_SALTS {
                if let Ok(layout) = Self::at(categories, height, salt) {
                    return Ok(layout);
                }
            }
        }
        Err(AuxError::NoLayout)
    }

    /// The layout `categories` take at `height` and `salt`, as a reader of
    /// someone else's tree places them; an error when two share a slot.
    pub fn at(categories: &[[u8; 32]], height: u8, salt: u32) -> Result<Self, AuxError> {
        let mut slots = Vec::with_capacity(categories.len());
        for category in categories {
            let placed = slot(category, salt, height);
            if slots.contains(&placed) {
                return Err(AuxError::NoLayout);
            }
            slots.push(placed);
        }
        Ok(Self {
            height,
            salt,
            slots,
        })
    }
}

impl AuxiliaryTree {
    /// Build the tree for `leaves`: the smallest height that holds them,
    /// the first salt below [`LAYOUT_SALTS`] that gives every category its
    /// own slot, else the next height, up to 16 ([`TreeLayout::search`]).
    /// No tokens give the exact empty commitment.
    pub fn build(leaves: &[TokenLeaf]) -> Result<Self, AuxError> {
        if leaves.is_empty() {
            return Ok(Self {
                commitment: MergeMiningCommitment::empty(),
                branches: Vec::new(),
            });
        }
        let categories: Vec<[u8; 32]> = leaves.iter().map(|leaf| leaf.category).collect();
        let layout = TreeLayout::search(&categories, MAX_MERGE_MINING_TREE_HEIGHT)?;
        Ok(Self::build_at(&layout, leaves))
    }

    /// The tree of `leaves` placed by `layout`, which holds one slot for
    /// each of them, in order: each leaf hash in its slot, every other slot
    /// empty. No leaves give the exact empty commitment.
    pub fn build_at(layout: &TreeLayout, leaves: &[TokenLeaf]) -> Self {
        debug_assert_eq!(layout.slots.len(), leaves.len());
        if leaves.is_empty() {
            return Self {
                commitment: MergeMiningCommitment::empty(),
                branches: Vec::new(),
            };
        }
        let TreeLayout {
            height,
            salt,
            slots,
        } = layout;
        let (height, salt) = (*height, *salt);
        let mut level = vec![[0u8; 32]; 1 << height];
        for (leaf, slot) in leaves.iter().zip(slots) {
            level[*slot as usize] = leaf.hash();
        }
        let mut levels = vec![level];
        while let Some(last) = levels.last().filter(|level| level.len() > 1) {
            let parents = last
                .as_chunks::<2>()
                .0
                .iter()
                .map(|[left, right]| node_hash(left, right))
                .collect();
            levels.push(parents);
        }
        let root = levels.last().expect("at least the leaf level")[0];
        let branches = leaves
            .iter()
            .zip(slots)
            .map(|(leaf, slot)| {
                let mut index = *slot as usize;
                let branch = levels[..levels.len() - 1]
                    .iter()
                    .map(|level| {
                        let sibling = level[index ^ 1];
                        index >>= 1;
                        sibling
                    })
                    .collect();
                TokenBranch {
                    category: leaf.category,
                    slot: *slot,
                    branch,
                }
            })
            .collect();
        Self {
            commitment: MergeMiningCommitment { root, height, salt },
            branches,
        }
    }
}

/// The magic a leaf disclosure starts with: `"CTMD"`.
const DISCLOSURE_MAGIC: [u8; 4] = *b"CTMD";
const DISCLOSURE_VERSION: u8 = 0x02;

/// The flag before a disclosed leaf's split: none, its `ext` is zero.
const DISCLOSED_WITHOUT_SPLIT: u8 = 0x00;
/// The flag before a disclosed leaf's split: its 69-byte preimage follows.
const DISCLOSED_WITH_SPLIT: u8 = 0x01;

/// One disclosed leaf and the split its `ext` commits to (none for a zero
/// `ext`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisclosedLeaf {
    pub leaf: TokenLeaf,
    pub split: TokenSplit,
}

/// The leaves behind one output 0 (`CTMD` v2), in the format agreed with
/// Pickaxe (its `docs/merge-mining.md`, "Disclosure"): what a Job
/// Declaration client puts in `DeclareMiningJob.excess_data`, and what a
/// pool shows a miner who asks for their leaves.
///
/// ```text
/// off len   field
/// 0   4     "CTMD"
/// 4   1     02
/// 5   1     n, the leaves (0 to 16)
/// then n times:
///     169   the leaf preimage
///     1     00: ext is zero; 01: a split follows
///     69    the split preimage (with 01 only)
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Disclosure {
    pub leaves: Vec<DisclosedLeaf>,
}

impl Disclosure {
    /// The canonical bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(
            DISCLOSURE_MAGIC.len()
                + 2
                + self.leaves.len() * (TOKEN_LEAF_LENGTH + 1 + TOKEN_SPLIT_PREIMAGE_LENGTH),
        );
        bytes.extend_from_slice(&DISCLOSURE_MAGIC);
        bytes.push(DISCLOSURE_VERSION);
        bytes.push(self.leaves.len() as u8);
        for disclosed in &self.leaves {
            bytes.extend_from_slice(&disclosed.leaf.preimage());
            if disclosed.split.is_none() {
                bytes.push(DISCLOSED_WITHOUT_SPLIT);
            } else {
                bytes.push(DISCLOSED_WITH_SPLIT);
                bytes.extend_from_slice(&disclosed.split.preimage());
            }
        }
        bytes
    }

    /// Read exactly one disclosure: the magic and version, at most 16
    /// leaves, each a `CTML` v2 leaf whose split is canonical and hashes to
    /// its `ext` (a zero `ext` without one), and nothing trailing.
    pub fn decode(bytes: &[u8]) -> Result<Self, AuxError> {
        let malformed = AuxError::Disclosure("not a canonical merge-mining leaf disclosure");
        let mut reader = Reader(bytes);
        if reader.take(4).map_err(|_| malformed.clone())? != DISCLOSURE_MAGIC
            || reader.byte().map_err(|_| malformed.clone())? != DISCLOSURE_VERSION
        {
            return Err(malformed);
        }
        let count = usize::from(reader.byte().map_err(|_| malformed.clone())?);
        if count > MAX_TOKEN_LEAVES {
            return Err(AuxError::TooManyTokens);
        }
        let mut leaves = Vec::with_capacity(count);
        for _ in 0..count {
            let leaf = TokenLeaf::from_preimage(
                reader
                    .take(TOKEN_LEAF_LENGTH)
                    .map_err(|_| malformed.clone())?,
            )?;
            let split = match reader.byte().map_err(|_| malformed.clone())? {
                DISCLOSED_WITHOUT_SPLIT => TokenSplit::default(),
                DISCLOSED_WITH_SPLIT => TokenSplit::parse(
                    reader
                        .take(TOKEN_SPLIT_PREIMAGE_LENGTH)
                        .map_err(|_| malformed.clone())?,
                )?,
                _ => return Err(malformed),
            };
            if split.ext() != leaf.ext {
                return Err(AuxError::Disclosure(
                    "a disclosed leaf's ext is not its split's",
                ));
            }
            leaves.push(DisclosedLeaf { leaf, split });
        }
        if !reader.0.is_empty() {
            return Err(malformed);
        }
        Ok(Self { leaves })
    }

    /// Whether these leaves, and no others, make `commitment`'s root: placed
    /// at its height and salt, each in its own slot and every other slot
    /// empty. A leaf left out, added or changed gives another root. No
    /// leaves disclose only the exact empty commitment.
    pub fn check_root(&self, commitment: &MergeMiningCommitment) -> Result<(), AuxError> {
        if self.leaves.is_empty() {
            return if commitment.is_empty() {
                Ok(())
            } else {
                Err(AuxError::Disclosure(
                    "the disclosure names no leaf for a non-empty root",
                ))
            };
        }
        let categories: Vec<[u8; 32]> = self
            .leaves
            .iter()
            .map(|disclosed| disclosed.leaf.category)
            .collect();
        let layout = TreeLayout::at(&categories, commitment.height, commitment.salt)
            .map_err(|_| AuxError::Disclosure("two disclosed leaves share a slot"))?;
        let leaves: Vec<TokenLeaf> = self.leaves.iter().map(|disclosed| disclosed.leaf).collect();
        if AuxiliaryTree::build_at(&layout, &leaves).commitment != *commitment {
            return Err(AuxError::Disclosure(
                "the disclosed leaves do not make output 0's root",
            ));
        }
        Ok(())
    }
}

/// `AuxProof` v2: what a token claim hands its covenant.
///
/// ```text
/// 0    4     "CTMP"
/// 4    1     02
/// 5    80    header
/// 85   1     h (at most 16)
/// 86   4     salt, u32 LE
/// 90   32h   aux branch, leaf to root
/// ..   2+n   u16 LE n, coinbase head (47 + L bytes: up to output 0)
/// ..   2+n   u16 LE n, coinbase tail (outputs 1.. and the locktime)
/// ..   1     d (at most 25)
/// ..   32d   coinbase branch, the coinbase at index 0
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuxProof {
    pub header: Header,
    pub height: u8,
    pub salt: u32,
    pub aux_branch: Vec<[u8; 32]>,
    pub coinbase_head: Vec<u8>,
    pub coinbase_tail: Vec<u8>,
    pub coinbase_branch: Vec<[u8; 32]>,
}

impl AuxProof {
    /// The proof of `token`'s win in a block whose coinbase carries
    /// `commitment` as output 0, under `header` with `coinbase_branch`.
    pub fn new(
        header: Header,
        coinbase: &Transaction,
        coinbase_branch: &[TxMerkleNode],
        commitment: MergeMiningCommitment,
        token: &TokenBranch,
    ) -> Result<Self, AuxError> {
        let bytes = serialize(coinbase);
        let head = coinbase_head_length(&bytes).ok_or(AuxError::CoinbaseHead)?;
        if serialize(&commitment.output()).as_slice()
            != bytes
                .get(head..head + MERGE_MINING_OUTPUT_LENGTH)
                .ok_or(AuxError::Commitment)?
        {
            return Err(AuxError::Commitment);
        }
        let proof = Self {
            header,
            height: commitment.height,
            salt: commitment.salt,
            aux_branch: token.branch.clone(),
            coinbase_head: bytes[..head].to_vec(),
            coinbase_tail: bytes[head + MERGE_MINING_OUTPUT_LENGTH..].to_vec(),
            coinbase_branch: coinbase_branch
                .iter()
                .map(|node| node.to_byte_array())
                .collect(),
        };
        proof.check_shape()?;
        Ok(proof)
    }

    /// The limits every v2 proof keeps: `h` at most 16 and matching the aux
    /// branch, a coinbase branch of at most 25, and parts that fit their
    /// length fields.
    pub fn check_shape(&self) -> Result<(), AuxError> {
        if self.height <= MAX_MERGE_MINING_TREE_HEIGHT
            && self.aux_branch.len() == usize::from(self.height)
            && self.coinbase_branch.len() <= MAX_AUX_PROOF_COINBASE_BRANCH_LENGTH
            && self.coinbase_head.len() <= usize::from(u16::MAX)
            && self.coinbase_tail.len() <= usize::from(u16::MAX)
        {
            Ok(())
        } else {
            Err(AuxError::Shape)
        }
    }

    /// The canonical encoding.
    pub fn to_bytes(&self) -> Result<Vec<u8>, AuxError> {
        self.check_shape()?;
        let mut bytes = Vec::with_capacity(
            PROOF_FIXED_LENGTH
                + 32 * (self.aux_branch.len() + self.coinbase_branch.len())
                + self.coinbase_head.len()
                + self.coinbase_tail.len(),
        );
        bytes.extend_from_slice(&PROOF_MAGIC);
        bytes.push(PROOF_VERSION);
        bytes.extend_from_slice(&serialize(&self.header));
        bytes.push(self.height);
        bytes.extend_from_slice(&self.salt.to_le_bytes());
        self.aux_branch
            .iter()
            .for_each(|hash| bytes.extend_from_slice(hash));
        for part in [&self.coinbase_head, &self.coinbase_tail] {
            bytes.extend_from_slice(&(part.len() as u16).to_le_bytes());
            bytes.extend_from_slice(part);
        }
        bytes.push(self.coinbase_branch.len() as u8);
        self.coinbase_branch
            .iter()
            .for_each(|hash| bytes.extend_from_slice(hash));
        Ok(bytes)
    }

    /// Read exactly one canonical v2 proof; anything else, trailing bytes
    /// included, is refused.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, AuxError> {
        let mut reader = Reader(bytes);
        if reader.take(4)? != PROOF_MAGIC || reader.byte()? != PROOF_VERSION {
            return Err(AuxError::Decode);
        }
        let header: Header = deserialize(reader.take(80)?).map_err(|_| AuxError::Decode)?;
        let height = reader.byte()?;
        if height > MAX_MERGE_MINING_TREE_HEIGHT {
            return Err(AuxError::Decode);
        }
        let salt = u32::from_le_bytes(reader.array()?);
        let aux_branch = reader.hashes(usize::from(height))?;
        let coinbase_head = reader.part()?;
        let coinbase_tail = reader.part()?;
        let depth = usize::from(reader.byte()?);
        if depth > MAX_AUX_PROOF_COINBASE_BRANCH_LENGTH {
            return Err(AuxError::Decode);
        }
        let coinbase_branch = reader.hashes(depth)?;
        if !reader.0.is_empty() {
            return Err(AuxError::Decode);
        }
        Ok(Self {
            header,
            height,
            salt,
            aux_branch,
            coinbase_head,
            coinbase_tail,
            coinbase_branch,
        })
    }

    /// The BIP34 height the coinbase head's scriptSig starts with.
    pub fn coinbase_height(&self) -> Option<u64> {
        let length = usize::from(*self.coinbase_head.get(41)?);
        bip34_height(self.coinbase_head.get(42..42 + length)?)
    }

    /// Verify this proof for `leaf`, which the token's own rules supply,
    /// with a tree at most `max_height` tall, as a covenant would:
    ///
    /// 1. the shape limits;
    /// 2. the slot from the category, the salt and `h`;
    /// 3. the aux branch folds the leaf hash to a root;
    /// 4. output 0 is rebuilt from that root, `h` and the salt;
    /// 5. the coinbase head has the fixed shape and the tail holds a
    ///    locktime;
    /// 6. the coinbase txid, `HASH256(head || output 0 || tail)`, folds up the
    ///    coinbase branch at index 0 to the header's merkle root;
    /// 7. the header's hash meets the token's target (never the header's
    ///    own nBits).
    ///
    /// With `asert`, the header's nBits must also pass the ASERT format
    /// check at the coinbase's BIP34 height ([`BchAsert::check_header_bits`]);
    /// regtest and scalenet have no anchor and pass `None`.
    pub fn verify(
        &self,
        leaf: &TokenLeaf,
        max_height: u8,
        asert: Option<BchAsert>,
    ) -> Result<(), AuxError> {
        self.check_shape()?;
        let target = expand_token_target(leaf.target_bits).ok_or(AuxError::Target)?;
        if self.height > max_height.min(MAX_MERGE_MINING_TREE_HEIGHT) {
            return Err(AuxError::Height);
        }
        let token_slot = slot(&leaf.category, self.salt, self.height);
        let root = fold(leaf.hash(), token_slot, &self.aux_branch);
        let commitment = MergeMiningCommitment {
            root,
            height: self.height,
            salt: self.salt,
        };
        check_coinbase_head(&self.coinbase_head).ok_or(AuxError::CoinbaseHead)?;
        if self.coinbase_tail.len() < 4 {
            return Err(AuxError::CoinbaseTail);
        }
        let mut coinbase = Vec::with_capacity(
            self.coinbase_head.len() + MERGE_MINING_OUTPUT_LENGTH + self.coinbase_tail.len(),
        );
        coinbase.extend_from_slice(&self.coinbase_head);
        coinbase.extend_from_slice(&serialize(&commitment.output()));
        coinbase.extend_from_slice(&self.coinbase_tail);
        let mut merkle = hash256(&coinbase);
        for sibling in &self.coinbase_branch {
            let mut pair = [0; 64];
            pair[..32].copy_from_slice(&merkle);
            pair[32..].copy_from_slice(sibling);
            merkle = hash256(&pair);
        }
        if merkle != self.header.merkle_root.to_byte_array() {
            return Err(AuxError::MerkleRoot);
        }
        let mut hash = self.header.block_hash().to_byte_array();
        hash.reverse();
        // Both big-endian now: compare as unsigned numbers.
        let mut target_big_endian = target;
        target_big_endian.reverse();
        if hash > target_big_endian {
            return Err(AuxError::Work);
        }
        if let Some(asert) = asert {
            let height = self
                .coinbase_height()
                .ok_or_else(|| AuxError::Asert("coinbase has no BIP34 height".into()))?;
            asert
                .check_header_bits(self.header.bits, self.header.time, height)
                .map_err(AuxError::Asert)?;
        }
        Ok(())
    }
}

/// The length of a coinbase's fixed-shape head, `47 + L`, when it has one.
fn coinbase_head_length(coinbase: &[u8]) -> Option<usize> {
    let length = 47 + usize::from(*coinbase.get(41)?);
    check_coinbase_head(coinbase.get(..length)?)?;
    Some(length)
}

/// A coinbase head is `version (4) | 01 | 00 x 32 | ffffffff | L | scriptSig
/// (L, at most 100) | sequence (4) | output count (1, 1..=0xfc)`.
fn check_coinbase_head(head: &[u8]) -> Option<()> {
    let length = usize::from(*head.get(41)?);
    (length <= MAX_COINBASE_SCRIPT_SIG
        && head.len() == 47 + length
        && head[4..41] == NULL_INPUT
        && (1..0xfd).contains(&head[head.len() - 1]))
    .then_some(())
}

fn hash256(bytes: &[u8]) -> [u8; 32] {
    sha256d::Hash::hash(bytes).to_byte_array()
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], AuxError> {
        if self.0.len() < count {
            return Err(AuxError::Decode);
        }
        let (taken, rest) = self.0.split_at(count);
        self.0 = rest;
        Ok(taken)
    }

    fn byte(&mut self) -> Result<u8, AuxError> {
        Ok(self.take(1)?[0])
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], AuxError> {
        self.take(N)?.try_into().map_err(|_| AuxError::Decode)
    }

    fn hashes(&mut self, count: usize) -> Result<Vec<[u8; 32]>, AuxError> {
        (0..count).map(|_| self.array()).collect()
    }

    fn part(&mut self) -> Result<Vec<u8>, AuxError> {
        let length = u16::from_le_bytes(self.array()?);
        Ok(self.take(usize::from(length))?.to_vec())
    }
}

#[cfg(test)]
mod tests;
