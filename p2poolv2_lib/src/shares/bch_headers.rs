// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The Bitcoin Cash block headers a node knows, and the check a header from
//! a peer passes before it becomes known.
//!
//! This is the first part of mining on the current chain (V16,
//! `docs/sharechain/current-chain.md`): a share will be paid only for work on
//! the BCH chain its paying share extends, give or take one block, and nodes
//! place shares on that chain with headers they all know. A known header
//! either came from this node's own BCHN or passed [`BchHeaderRules::check`]:
//!
//! - it links to a known header, its parent;
//! - its hash meets its own nBits;
//! - its nBits is exactly what BCHN requires after that parent
//!   (`GetNextWorkRequired` in BCHN's `src/pow.cpp`): aserti3-2d from the
//!   network's anchor ([`BchAsert::next_bits`]), the proof-of-work limit for
//!   a test network block dated more than 20 minutes after its parent, or
//!   the parent's nBits on regtest, which never retargets;
//! - it is dated at most two hours ahead of this node's clock, BCHN's
//!   `MAX_FUTURE_BLOCK_TIME`. Under aserti3-2d every block's nBits depends on
//!   timestamps, so without the limit a made-up branch could date its blocks
//!   to ease its nBits.
//!
//! A header that passes cost a full block of real proof of work at the
//! difficulty Bitcoin Cash required. It does not prove a valid body exists,
//! which the payout rule does not need. Unlike [`BchAsert::check_header_bits`],
//! which has only a header and its height and so allows a band, this check
//! has the parent and requires the exact nBits.
//!
//! Scalenet has no ASERT anchor here (see [`BchAsert::for_network`]), so its
//! headers are checked for linkage, proof of work and time, not exact nBits.

use super::bch_asert::{BCH_POW_LIMIT_BITS, BchAsert, bch_pow_limit, set_compact};
use crate::config::BitcoinCashNetwork;
use bitcoin::block::Header;
use bitcoin::consensus::{Decodable, Encodable};
use bitcoin::{BlockHash, CompactTarget, Target};

/// Protocol tag segment for mining on the current chain: Bitcoin Cash
/// headers travel with shares, and a share is paid only for work on the
/// paying share's BCH chain or one block off it.
pub const BCH_CURRENT_CHAIN_RULE_TAG: &str = "bch-prev-1off";

/// BCHN's `MAX_FUTURE_BLOCK_TIME`: a block may be dated at most two hours
/// ahead of the node's clock.
pub const MAX_FUTURE_BLOCK_TIME: u32 = 2 * 60 * 60;

/// BCHN's `nPowTargetSpacing`, ten minutes.
const TARGET_SPACING: u32 = 10 * 60;

/// A test network block dated more than this after its parent may use the
/// proof-of-work limit: `2 * nPowTargetSpacing`, 20 minutes.
const MIN_DIFFICULTY_GAP: u32 = 2 * TARGET_SPACING;

/// A Bitcoin Cash block header this node knows, with its height.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownBchHeader {
    /// Height of the block in its chain.
    pub height: u32,
    /// The 80-byte header.
    pub header: Header,
}

impl KnownBchHeader {
    /// The block's hash.
    pub fn block_hash(&self) -> BlockHash {
        self.header.block_hash()
    }
}

impl Encodable for KnownBchHeader {
    fn consensus_encode<W: bitcoin::io::Write + ?Sized>(
        &self,
        writer: &mut W,
    ) -> Result<usize, bitcoin::io::Error> {
        Ok(self.height.consensus_encode(writer)? + self.header.consensus_encode(writer)?)
    }
}

impl Decodable for KnownBchHeader {
    fn consensus_decode<R: bitcoin::io::Read + ?Sized>(
        reader: &mut R,
    ) -> Result<Self, bitcoin::consensus::encode::Error> {
        Ok(Self {
            height: u32::consensus_decode(reader)?,
            header: Header::consensus_decode(reader)?,
        })
    }
}

/// Why a header from a peer is not known.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BchHeaderError {
    /// The header does not name `parent` as its previous block.
    #[error("header {header} does not build on {parent}")]
    NotChild {
        header: BlockHash,
        parent: BlockHash,
    },
    /// The header's hash does not meet its own nBits.
    #[error("header {0} does not meet its own nBits")]
    ProofOfWork(BlockHash),
    /// The header's nBits is not the one Bitcoin Cash requires.
    #[error("header {header} has nBits {declared:#010x}, Bitcoin Cash requires {required:#010x}")]
    Bits {
        header: BlockHash,
        declared: u32,
        required: u32,
    },
    /// The header is dated too far ahead of this node's clock.
    #[error("header {header} is dated {time}, more than two hours after {now}")]
    TooFarAhead {
        header: BlockHash,
        time: u32,
        now: u32,
    },
    /// The parent is at the highest height a `u32` holds.
    #[error("header {0} is above the highest height")]
    Height(BlockHash),
}

/// How a network sets each block's nBits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BitsRule {
    /// aserti3-2d from the network's anchor.
    Asert(BchAsert),
    /// Regtest never retargets: every block repeats its parent's nBits.
    Parent,
    /// No anchor is known here (scalenet): nBits is not checked.
    Unchecked,
}

/// The header rules of one Bitcoin Cash network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BchHeaderRules {
    bits: BitsRule,
}

impl BchHeaderRules {
    /// The rules of `network`.
    pub fn for_network(network: BitcoinCashNetwork) -> Self {
        let bits = match (network, BchAsert::for_network(network)) {
            (_, Some(asert)) => BitsRule::Asert(asert),
            (BitcoinCashNetwork::Regtest, None) => BitsRule::Parent,
            (_, None) => BitsRule::Unchecked,
        };
        Self { bits }
    }

    /// The nBits Bitcoin Cash requires of a block on `parent` dated
    /// `header_time`, or `None` where this node does not check it.
    pub fn required_bits(
        &self,
        parent: &KnownBchHeader,
        header_time: u32,
    ) -> Option<CompactTarget> {
        match self.bits {
            BitsRule::Asert(asert) => {
                if asert.allow_pow_limit
                    && header_time > parent.header.time.saturating_add(MIN_DIFFICULTY_GAP)
                {
                    return Some(CompactTarget::from_consensus(BCH_POW_LIMIT_BITS));
                }
                asert.next_bits(parent.height, parent.header.time)
            }
            BitsRule::Parent => Some(parent.header.bits),
            BitsRule::Unchecked => None,
        }
    }

    /// The nBits that sizes a payout window paid by a block on `parent`: the
    /// nBits Bitcoin Cash requires of `parent`'s next block, or `None` where
    /// this node does not check nBits (scalenet).
    ///
    /// Unlike [`Self::required_bits`] it leaves out the test networks'
    /// 20-minute rule, which depends on the paying block's own time: a block
    /// dated late may take the proof-of-work limit, and a window sized by
    /// that would fall back to its share cap. Every node computes the same
    /// nBits from the same known parent, so no miner can choose it.
    pub fn sizing_bits(&self, parent: &KnownBchHeader) -> Option<CompactTarget> {
        match self.bits {
            BitsRule::Asert(asert) => asert.next_bits(parent.height, parent.header.time),
            BitsRule::Parent => Some(parent.header.bits),
            BitsRule::Unchecked => None,
        }
    }

    /// Check `header` from a peer against its known `parent`, at `now`
    /// (seconds since the epoch), and return it as known.
    pub fn check(
        &self,
        header: &Header,
        parent: &KnownBchHeader,
        now: u32,
    ) -> Result<KnownBchHeader, BchHeaderError> {
        let hash = header.block_hash();
        if header.prev_blockhash != parent.block_hash() {
            return Err(BchHeaderError::NotChild {
                header: hash,
                parent: parent.block_hash(),
            });
        }
        let height = parent
            .height
            .checked_add(1)
            .ok_or(BchHeaderError::Height(hash))?;
        if header.time > now.saturating_add(MAX_FUTURE_BLOCK_TIME) {
            return Err(BchHeaderError::TooFarAhead {
                header: hash,
                time: header.time,
                now,
            });
        }
        if let Some(required) = self.required_bits(parent, header.time)
            && header.bits != required
        {
            return Err(BchHeaderError::Bits {
                header: hash,
                declared: header.bits.to_consensus(),
                required: required.to_consensus(),
            });
        }
        if !meets_own_bits(header) {
            return Err(BchHeaderError::ProofOfWork(hash));
        }
        Ok(KnownBchHeader {
            height,
            header: *header,
        })
    }
}

/// True when the header's hash meets its nBits, and the nBits is a valid
/// target no easier than Bitcoin Cash's proof-of-work limit.
fn meets_own_bits(header: &Header) -> bool {
    let Some(target) = set_compact(header.bits.to_consensus()) else {
        return false;
    };
    if target > bch_pow_limit() {
        return false;
    }
    Target::from_compact(header.bits).is_met_by(header.block_hash())
}

#[cfg(test)]
mod tests;
