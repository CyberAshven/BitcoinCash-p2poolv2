// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The tokens a Bitcoin Cash P2Pool node merge-mines for its miners, and
//! each miner's leaves.
//!
//! On a public node every miner mines a root of their own, in the
//! merge-mining common format v2 agreed with Pickaxe (`docs/bch-auxpow.md`):
//! each leaf binds the miner's registered payout script (`payout_hash`), and
//! its `ext` the split of each win, the miner's P2Pool donation in the
//! donation slot and the node's fee in the fee slot, each as a per-win cut
//! to the script the coinbase pays it to. A token's covenant pays those
//! cuts; P2Pool never sees a claim.
//!
//! A token set's layout (its height, salt and slots) depends only on its
//! categories, so it is searched once when the set is read, and every
//! miner's tree reuses it: miners differ only in their root.
//!
//! This is node behaviour, not a share-chain rule. A share's header carries
//! its root (`ShareHeader::pool_merge_mining`), and peers check only its
//! format and rebuild output 0 from it; share validity never depends on the
//! tokens.

use crate::shares::auxpow::{
    AuxError, AuxProof, AuxiliaryTree, DisclosedLeaf, Disclosure, MAX_TOKEN_TARGET_BITS,
    TokenBranch, TokenLeaf, TokenSplit, TreeLayout, encode_token_target, expand_token_target,
};
use crate::shares::bch_asert::{BchAsert, U256};
use crate::shares::merge_mining::{
    MAX_MERGE_MINING_TREE_HEIGHT, MERGE_MINING_OUTPUT_LENGTH, MergeMiningCommitment,
};
use bitcoin::block::Header;
use bitcoin::consensus::serialize;
use bitcoin::hashes::Hash;
use bitcoin::{Transaction, TxMerkleNode};
use serde::Deserialize;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;
use tracing::{error, info, warn};

/// The largest token file the node reads, as the CLI's input limit.
pub const MAX_TOKEN_FILE_BYTES: u64 = 1024 * 1024;

/// The root a coinbase is sized and laid out with before any miner's root
/// replaces it: not zero, so it is a commitment, and never a real root.
const PLACEHOLDER_ROOT: [u8; 32] = [0xff; 32];

/// One token the node merge-mines, in its current state: the leaf fields a
/// miner's leaf takes from the token, and the tallest tree its covenant
/// accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeMinedToken {
    /// The token category, as hashed.
    pub category: [u8; 32],
    /// `HASH256` of the token baton's locking bytecode.
    pub lock_hash: [u8; 32],
    /// The token's state hash; it moves on each successful claim, and the
    /// operator's token file follows it.
    pub state_hash: [u8; 32],
    /// The token's compact target, canonical (`expand_token_target`).
    pub target_bits: u32,
    /// The tallest aux tree the token's covenant accepts, at most 16.
    pub max_height: u8,
    /// A SAFA vault's thread, whose claim target eases with the blocks
    /// since its last claim: `target_bits` is then the state's own target,
    /// and each job's leaf takes the target at its height
    /// ([`TokenSet::at_height`]).
    pub safa: Option<SafaThread>,
}

/// The most threads a SAFA deployment has (`MAX_THREADS` in
/// `p2poolv2_safa`).
pub const SAFA_MAX_THREADS: u32 = 63;

/// A claim's target eases by at most this many blocks since the thread's
/// state (`EASE_CAP`).
pub const SAFA_EASE_CAP: u32 = 288;

/// One SAFA vault the node merge-mines: its deployment's thread count and
/// the thread's 16-byte state, the vault's NFT commitment (`thread u32le,
/// height u32le, target_bits u32le, ref_bits u32le`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SafaThread {
    pub threads: u32,
    pub state: [u8; 16],
}

impl SafaThread {
    /// The coinbase height of the thread's last claim (or genesis).
    pub fn state_height(&self) -> u32 {
        u32::from_le_bytes(self.state[4..8].try_into().expect("four bytes"))
    }

    /// The target the thread's state holds, before easing.
    pub fn state_target_bits(&self) -> u32 {
        u32::from_le_bytes(self.state[8..12].try_into().expect("four bytes"))
    }

    /// `HASH256` of the state, the leaf's `state_hash`.
    pub fn state_hash(&self) -> [u8; 32] {
        bitcoin::hashes::sha256d::Hash::hash(&self.state).to_byte_array()
    }

    /// The target a claim of a coinbase at `height` must meet, as the
    /// covenant rebuilds the leaf: the state's target eased by
    /// `min(height - state height, 288) / threads`. `None` when `height` is
    /// not above the state's.
    pub fn claim_target_bits(&self, height: u32) -> Option<u32> {
        let height_delta = height.checked_sub(self.state_height()).filter(|&d| d > 0)?;
        safa_claim_target_bits(self.state_target_bits(), height_delta, self.threads)
    }
}

/// `claim_target_bits` of `p2poolv2_safa` (the SAFA tool), integer for integer:
/// `target * (min(dh, 288) * 5000 / threads + 5000) / 10000`, each division
/// floored, clamped to 1 and to the easiest token target, encoded as
/// canonical token bits. `None` for bits that are not a canonical token
/// target or no threads.
pub fn safa_claim_target_bits(state_bits: u32, height_delta: u32, threads: u32) -> Option<u32> {
    if threads == 0 {
        return None;
    }
    let target = U256::from_little_endian(&expand_token_target(state_bits)?);
    let factor =
        U256::from(u64::from(height_delta.min(SAFA_EASE_CAP)) * 5_000 / u64::from(threads) + 5_000);
    let maximum = U256::from_little_endian(&expand_token_target(MAX_TOKEN_TARGET_BITS)?);
    let denominator = U256::from(10_000);
    // target * factor / 10000 without overflowing 256 bits: the quotient
    // and remainder parts separately; an overflow is past the maximum.
    let eased = (target / denominator)
        .checked_mul(factor)
        .and_then(|whole| whole.checked_add(target % denominator * factor / denominator))
        .unwrap_or(maximum);
    encode_token_target(eased.clamp(U256::one(), maximum))
}

/// The tokens a node merge-mines and the layout every miner's tree shares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenSet {
    tokens: Vec<MergeMinedToken>,
    /// Each token's target, little-endian, as share hashes compare.
    targets: Vec<[u8; 32]>,
    layout: TreeLayout,
}

/// The token file: `{"tokens": [...]}`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TokenFileJson {
    tokens: Vec<TokenJson>,
}

/// One token in the file: 32-byte values in hex as hashed, the compact
/// target as eight hex digits (`"207fffff"`), and optionally the tallest
/// tree its covenant accepts (16 when left out). A SAFA vault gives its
/// thread instead of `state_hash` and `target_bits`, which follow from it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TokenJson {
    category: String,
    lock_hash: String,
    #[serde(default)]
    state_hash: Option<String>,
    #[serde(default)]
    target_bits: Option<String>,
    #[serde(default)]
    max_height: Option<u8>,
    #[serde(default)]
    safa: Option<SafaJson>,
}

/// A SAFA vault's thread: the deployment's thread count and the vault's
/// 16-byte state (its NFT commitment) in hex.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SafaJson {
    threads: u32,
    state: String,
}

impl TokenJson {
    fn token(&self) -> Result<MergeMinedToken, String> {
        let (state_hash, target_bits, safa) =
            match (&self.state_hash, &self.target_bits, &self.safa) {
                (Some(state_hash), Some(target_bits), None) => (
                    hash32(state_hash, "state_hash")?,
                    u32::from_str_radix(target_bits, 16)
                        .ok()
                        .filter(|_| target_bits.len() == 8)
                        .ok_or("target_bits must be eight hexadecimal digits")?,
                    None,
                ),
                (None, None, Some(safa)) => {
                    if !(1..=SAFA_MAX_THREADS).contains(&safa.threads) {
                        return Err(format!("safa.threads must be from 1 to {SAFA_MAX_THREADS}"));
                    }
                    let thread = SafaThread {
                        threads: safa.threads,
                        state: hex::decode(&safa.state)
                            .ok()
                            .and_then(|bytes| bytes.try_into().ok())
                            .ok_or("safa.state must be 16 bytes of hex")?,
                    };
                    (
                        thread.state_hash(),
                        thread.state_target_bits(),
                        Some(thread),
                    )
                }
                _ => {
                    return Err(
                        "a token gives either state_hash and target_bits, or safa".to_string()
                    );
                }
            };
        Ok(MergeMinedToken {
            category: hash32(&self.category, "category")?,
            lock_hash: hash32(&self.lock_hash, "lock_hash")?,
            state_hash,
            target_bits,
            max_height: self.max_height.unwrap_or(MAX_MERGE_MINING_TREE_HEIGHT),
            safa,
        })
    }
}

fn hash32(text: &str, name: &str) -> Result<[u8; 32], String> {
    hex::decode(text)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| format!("{name} must be 32 bytes of hex"))
}

impl TokenSet {
    /// A set of 1 to 16 tokens, each with a canonical compact target, a
    /// tree limit of at most 16 and its own category, that fit one tree no
    /// taller than the lowest limit among them.
    pub fn new(tokens: Vec<MergeMinedToken>) -> Result<Self, String> {
        if tokens.is_empty() {
            return Err("a token set needs at least one token".into());
        }
        let targets = tokens
            .iter()
            .map(|token| {
                expand_token_target(token.target_bits).ok_or_else(|| {
                    format!(
                        "token {} has no canonical compact target in {:08x}",
                        hex::encode(token.category),
                        token.target_bits
                    )
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        if let Some(token) = tokens
            .iter()
            .find(|token| token.max_height > MAX_MERGE_MINING_TREE_HEIGHT)
        {
            return Err(format!(
                "token {} allows a tree taller than {MAX_MERGE_MINING_TREE_HEIGHT}",
                hex::encode(token.category)
            ));
        }
        if let Some(token) = tokens.iter().find(|token| {
            token.safa.is_some_and(|thread| {
                !(1..=SAFA_MAX_THREADS).contains(&thread.threads)
                    || thread.state_hash() != token.state_hash
                    || thread.state_target_bits() != token.target_bits
            })
        }) {
            return Err(format!(
                "SAFA token {} needs 1 to {SAFA_MAX_THREADS} threads and the state hash and target of its thread",
                hex::encode(token.category)
            ));
        }
        let categories: Vec<[u8; 32]> = tokens.iter().map(|token| token.category).collect();
        let max_height = tokens
            .iter()
            .map(|token| token.max_height)
            .min()
            .unwrap_or(MAX_MERGE_MINING_TREE_HEIGHT);
        let layout = TreeLayout::search(&categories, max_height).map_err(|error| match error {
            AuxError::NoLayout => format!(
                "the tokens do not fit one tree at most {max_height} high, the lowest tree limit among them"
            ),
            error => error.to_string(),
        })?;
        Ok(Self {
            tokens,
            targets,
            layout,
        })
    }

    /// Read a token file's bytes: `None` for a file that lists no token.
    pub fn from_json(bytes: &[u8]) -> Result<Option<Self>, String> {
        let file: TokenFileJson =
            serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        if file.tokens.is_empty() {
            return Ok(None);
        }
        let tokens = file
            .tokens
            .iter()
            .map(TokenJson::token)
            .collect::<Result<Vec<_>, _>>()?;
        Self::new(tokens).map(Some)
    }

    pub fn tokens(&self) -> &[MergeMinedToken] {
        &self.tokens
    }

    pub fn layout(&self) -> &TreeLayout {
        &self.layout
    }

    /// The set as a job whose coinbase is at `height` mines it: each SAFA
    /// token's target eased by the blocks since its thread's state, as the
    /// covenant rebuilds the leaf it checks (`claim_leaf` in
    /// `p2poolv2_safa`). Other tokens, and a set without SAFA
    /// tokens, are unchanged. An error when a SAFA thread's state is not
    /// below `height`: no claim of that coinbase could succeed.
    pub fn at_height(self: &Arc<Self>, height: u32) -> Result<Arc<Self>, String> {
        if self.tokens.iter().all(|token| token.safa.is_none()) {
            return Ok(Arc::clone(self));
        }
        let mut tokens = self.tokens.clone();
        let mut targets = self.targets.clone();
        for (token, target) in tokens.iter_mut().zip(targets.iter_mut()) {
            let Some(thread) = token.safa else {
                continue;
            };
            token.target_bits = thread.claim_target_bits(height).ok_or_else(|| {
                format!(
                    "SAFA token {}'s thread state is at height {}, not below the coinbase's {height}",
                    hex::encode(token.category),
                    thread.state_height()
                )
            })?;
            *target = expand_token_target(token.target_bits)
                .expect("claim target bits are canonical token bits");
        }
        Ok(Arc::new(Self {
            tokens,
            targets,
            layout: self.layout.clone(),
        }))
    }

    /// The commitment a job's coinbase is built and sized with before each
    /// miner's root replaces its root: this set's height and salt, a
    /// placeholder root.
    pub(crate) fn placeholder_commitment(&self) -> MergeMiningCommitment {
        MergeMiningCommitment {
            root: PLACEHOLDER_ROOT,
            height: self.layout.height,
            salt: self.layout.salt,
        }
    }

    /// One miner's job: a leaf per token paying `payout_script`, the
    /// miner's registered payout, with `split` in its `ext`, the tree those
    /// leaves make at this set's layout, and its root.
    pub fn miner_job(&self, payout_script: &[u8], split: TokenSplit) -> MinerTokenJob {
        let ext = split.ext();
        let leaves: Vec<TokenLeaf> = self
            .tokens
            .iter()
            .map(|token| TokenLeaf {
                category: token.category,
                lock_hash: token.lock_hash,
                state_hash: token.state_hash,
                payout_hash: bitcoin::hashes::sha256d::Hash::hash(payout_script).to_byte_array(),
                target_bits: token.target_bits,
                ext,
            })
            .collect();
        let tree = AuxiliaryTree::build_at(&self.layout, &leaves);
        let entries = leaves
            .into_iter()
            .zip(tree.branches)
            .zip(self.tokens.iter().zip(&self.targets))
            .map(|((leaf, branch), (token, target))| TokenEntry {
                leaf,
                split,
                branch,
                target: *target,
                max_height: token.max_height,
            })
            .collect();
        MinerTokenJob {
            commitment: tree.commitment,
            entries,
        }
    }
}

/// One token of a miner's job: its leaf, the split its `ext` commits to,
/// its place in the tree, and what a win must meet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenEntry {
    pub leaf: TokenLeaf,
    pub split: TokenSplit,
    pub branch: TokenBranch,
    /// The token's target, little-endian.
    target: [u8; 32],
    max_height: u8,
}

impl TokenEntry {
    /// Whether a header whose hash is `block_hash` meets this token's
    /// target: the hash, read as an unsigned little-endian number, is at
    /// most the target.
    fn is_won_by(&self, block_hash: &bitcoin::BlockHash) -> bool {
        let hash = block_hash.to_byte_array();
        hash.iter().rev().cmp(self.target.iter().rev()).is_le()
    }
}

/// One miner's token leaves for one job, and the root they make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MinerTokenJob {
    /// Output 0 of the miner's coinbase.
    pub commitment: MergeMiningCommitment,
    /// The tokens in the set's order.
    pub entries: Vec<TokenEntry>,
}

/// The proof of one token a share won, for its miner to claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenWin {
    pub category: [u8; 32],
    /// The canonical `AuxProof` v2 bytes.
    pub proof: Vec<u8>,
    /// The split the claim reveals: the leaf's `ext` is its hash.
    pub split: TokenSplit,
}

impl MinerTokenJob {
    /// The leaves behind output 0 (`CTMD` v2), each with its split.
    pub fn disclosure(&self) -> Disclosure {
        Disclosure {
            leaves: self
                .entries
                .iter()
                .map(|entry| DisclosedLeaf {
                    leaf: entry.leaf,
                    split: entry.split,
                })
                .collect(),
        }
    }

    /// Output 0 as the coinbase carries it: 53 bytes.
    pub fn output(&self) -> [u8; MERGE_MINING_OUTPUT_LENGTH] {
        serialize(&self.commitment.output())
            .try_into()
            .expect("a merge-mining output is 53 bytes")
    }

    /// Each leaf's aux branch, leaf to root, in the entries' order.
    pub fn branches(&self) -> Vec<Vec<[u8; 32]>> {
        self.entries
            .iter()
            .map(|entry| entry.branch.branch.clone())
            .collect()
    }

    /// The proofs of the tokens a share won: `header`, mined on `coinbase`
    /// at index 0 of `coinbase_branch`, has a hash meeting their target.
    ///
    /// Each proof is checked as a covenant would check it, the ASERT format
    /// check included where `asert` names the network's anchor, before it
    /// is handed out. A proof that cannot be built (a block deeper than the
    /// proof format's 25 levels) or does not verify is logged and left out.
    pub fn wins(
        &self,
        header: &Header,
        coinbase: &Transaction,
        coinbase_branch: &[TxMerkleNode],
        asert: Option<BchAsert>,
    ) -> Vec<TokenWin> {
        let block_hash = header.block_hash();
        self.entries
            .iter()
            .filter(|entry| entry.is_won_by(&block_hash))
            .filter_map(|entry| {
                let proof = AuxProof::new(
                    *header,
                    coinbase,
                    coinbase_branch,
                    self.commitment,
                    &entry.branch,
                )
                .and_then(|proof| {
                    proof.verify(&entry.leaf, entry.max_height, asert)?;
                    proof.to_bytes()
                });
                match proof {
                    Ok(proof) => Some(TokenWin {
                        category: entry.leaf.category,
                        proof,
                        split: entry.split,
                    }),
                    Err(error) => {
                        warn!(
                            "Share {block_hash} meets token {}'s target, but its proof cannot be given: {error}",
                            hex::encode(entry.leaf.category)
                        );
                        None
                    }
                }
            })
            .collect()
    }
}

/// The node's token file, read again whenever its modification time
/// changes, so the tool that claims a token can move its state.
///
/// A file that becomes unreadable or invalid keeps the last good set and
/// says so in the log until it is fixed.
#[derive(Debug)]
pub struct TokenFile {
    path: PathBuf,
    modified: Option<SystemTime>,
    current: Option<Arc<TokenSet>>,
}

impl TokenFile {
    /// Read the token file at `path` now; a file that cannot be read or is
    /// not valid is an error, so a node does not start on a broken one.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, String> {
        let path = path.into();
        let modified = modified(&path)?;
        let current = read_token_file(&path)?.map(Arc::new);
        log_token_set(&path, current.as_deref());
        Ok(Self {
            path,
            modified,
            current,
        })
    }

    /// The tokens to merge-mine now, after reading the file again if it
    /// changed. `None` when it lists none.
    pub fn current(&mut self) -> Option<Arc<TokenSet>> {
        match modified(&self.path) {
            Ok(modified) if modified != self.modified => match read_token_file(&self.path) {
                Ok(set) => {
                    self.current = set.map(Arc::new);
                    self.modified = modified;
                    log_token_set(&self.path, self.current.as_deref());
                }
                Err(reason) => error!(
                    "Token file {} is not valid, still merge-mining the tokens read before: {reason}",
                    self.path.display()
                ),
            },
            Ok(_) => {}
            Err(reason) => error!(
                "Token file {} cannot be read, still merge-mining the tokens read before: {reason}",
                self.path.display()
            ),
        }
        self.current.clone()
    }
}

fn modified(path: &Path) -> Result<Option<SystemTime>, String> {
    std::fs::metadata(path)
        .map(|metadata| metadata.modified().ok())
        .map_err(|error| format!("{}: {error}", path.display()))
}

fn read_token_file(path: &Path) -> Result<Option<TokenSet>, String> {
    let file = File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut bytes = Vec::with_capacity(4096);
    file.take(MAX_TOKEN_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if bytes.len() as u64 > MAX_TOKEN_FILE_BYTES {
        return Err(format!(
            "{} is larger than {MAX_TOKEN_FILE_BYTES} bytes",
            path.display()
        ));
    }
    TokenSet::from_json(&bytes).map_err(|reason| format!("{}: {reason}", path.display()))
}

fn log_token_set(path: &Path, set: Option<&TokenSet>) {
    match set {
        Some(set) => info!(
            "Merge-mining {} token(s) from {} for every miner, each with their own root (tree height {}, salt {})",
            set.tokens.len(),
            path.display(),
            set.layout.height,
            set.layout.salt
        ),
        None => info!(
            "Token file {} lists no token: jobs carry the empty merge-mining commitment",
            path.display()
        ),
    }
}

#[cfg(test)]
mod tests;
