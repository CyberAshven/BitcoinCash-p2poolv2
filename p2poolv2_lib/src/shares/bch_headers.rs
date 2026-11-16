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
//!   a test network block dated more than twice its interval after its
//!   parent, or the parent's nBits on regtest, which never retargets;
//! - it is dated at most two hours ahead of this node's clock, BCHN's
//!   `MAX_FUTURE_BLOCK_TIME`. Under aserti3-2d every block's nBits depends on
//!   timestamps, so without the limit a made-up branch could date its blocks
//!   to ease its nBits.
//!
//! A header that passes cost a full block of real proof of work. It does not
//! prove a valid body exists, which the payout rule does not need. Unlike
//! [`BchAsert::check_header_bits`], which has only a header and its height
//! and so allows a band, this check has the parent and requires the exact
//! nBits.
//!
//! Each known header carries its place in CHIP-2025-03's ticks, as BCHN keeps
//! it in its block index: its ticks, its parent's ticks plus its parent's
//! next interval, and the interval of a block built on it, which its own
//! median time past (it and its ten ancestors) decides against the network's
//! timed switches ([`TickSchedule::next_interval`]). A header gets them when
//! it becomes known, from its parent and ancestors, so every node that knows
//! a branch gives its blocks the same ticks and the same required nBits, and
//! a reorg onto a branch that switched at another block follows that branch.
//! Where fewer than ten ancestors are known, as for the oldest headers of a
//! new store, the network's pinned switches answer by height.
//!
//! Scalenet has no ASERT anchor here (see [`BchAsert::for_network`]), so its
//! headers are checked for linkage, proof of work and time, not exact nBits.

use super::bch_asert::{
    BCH_POW_LIMIT_BITS, BchAsert, MEDIAN_TIME_SPAN, TickInfo, TickSchedule, bch_pow_limit,
    divides_600, set_compact,
};
use crate::config::BitcoinCashNetwork;
use bitcoin::block::Header;
use bitcoin::consensus::{Decodable, Encodable};
use bitcoin::{BlockHash, CompactTarget, Target};

/// Protocol tag segment for mining on the current chain: Bitcoin Cash
/// headers travel with shares, and a share is paid only for work on the
/// paying share's BCH chain or one block off it.
///
/// `bch-prev-1off-ticks` since 2026-11-15: each known header carries its
/// CHIP-2025-03 ticks and next interval, worked out from its own branch's
/// median time past as BCHN does, and the nBits a header from a peer must
/// carry and the nBits that size a payout window follow them. On
/// `bch-prev-1off` a node learned one-minute blocks while running, from its
/// own BCHN, so two nodes could require different nBits of one header and
/// size one window differently. Nodes on the two do not accept the same
/// shares: a flag day, with a fresh store.
pub const BCH_CURRENT_CHAIN_RULE_TAG: &str = "bch-prev-1off-ticks";

/// BCHN's `MAX_FUTURE_BLOCK_TIME`: a block may be dated at most two hours
/// ahead of the node's clock.
pub const MAX_FUTURE_BLOCK_TIME: u32 = 2 * 60 * 60;

/// A Bitcoin Cash block header this node knows, with its height and its
/// place in CHIP-2025-03's ticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownBchHeader {
    /// Height of the block in its chain.
    pub height: u32,
    /// The 80-byte header.
    pub header: Header,
    /// The block's ticks: its parent's ticks plus its parent's next
    /// interval, 0 for the chain's first block.
    pub ticks: i64,
    /// The target interval, in ticks, of a block built on this one.
    pub next_interval: i64,
}

impl KnownBchHeader {
    /// The block's hash.
    pub fn block_hash(&self) -> BlockHash {
        self.header.block_hash()
    }

    /// The block's ticks and the interval of a block built on it.
    pub fn tick_info(&self) -> TickInfo {
        TickInfo {
            ticks: self.ticks,
            next_interval: self.next_interval,
        }
    }
}

impl Encodable for KnownBchHeader {
    fn consensus_encode<W: bitcoin::io::Write + ?Sized>(
        &self,
        writer: &mut W,
    ) -> Result<usize, bitcoin::io::Error> {
        Ok(self.height.consensus_encode(writer)?
            + self.header.consensus_encode(writer)?
            + self.ticks.consensus_encode(writer)?
            + self.next_interval.consensus_encode(writer)?)
    }
}

impl Decodable for KnownBchHeader {
    fn consensus_decode<R: bitcoin::io::Read + ?Sized>(
        reader: &mut R,
    ) -> Result<Self, bitcoin::consensus::encode::Error> {
        let known = Self {
            height: u32::consensus_decode(reader)?,
            header: Header::consensus_decode(reader)?,
            ticks: i64::consensus_decode(reader)?,
            next_interval: i64::consensus_decode(reader)?,
        };
        if known.ticks < 0 || !divides_600(known.next_interval) {
            return Err(bitcoin::consensus::encode::Error::ParseFailed(
                "known header ticks or interval outside CHIP-2025-03's range",
            ));
        }
        Ok(known)
    }
}

/// The times a median time past of a block dated `time` on `parent` reads:
/// `time`, then `parent`'s and those of its ancestors that `known` finds,
/// newest first, at most [`MEDIAN_TIME_SPAN`].
pub fn median_times_on(
    time: u32,
    parent: &KnownBchHeader,
    known: impl Fn(&BlockHash) -> Option<KnownBchHeader>,
) -> Vec<u32> {
    let mut times = Vec::with_capacity(MEDIAN_TIME_SPAN);
    times.push(time);
    let mut current = Some(*parent);
    while let Some(header) = current {
        times.push(header.header.time);
        if times.len() == MEDIAN_TIME_SPAN || header.height == 0 {
            break;
        }
        current = known(&header.header.prev_blockhash);
    }
    times
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
    /// The network's block intervals (CHIP-2025-03).
    tick_schedule: TickSchedule,
}

impl BchHeaderRules {
    /// The rules of `network`.
    pub fn for_network(network: BitcoinCashNetwork) -> Self {
        let bits = match (network, BchAsert::for_network(network)) {
            (_, Some(asert)) => BitsRule::Asert(asert),
            (BitcoinCashNetwork::Regtest, None) => BitsRule::Parent,
            (_, None) => BitsRule::Unchecked,
        };
        Self {
            bits,
            tick_schedule: TickSchedule::for_network(network),
        }
    }

    /// The network's block intervals.
    pub fn tick_schedule(&self) -> TickSchedule {
        self.tick_schedule
    }

    /// `header` as a known header at `height` on `parent`: its ticks follow
    /// the parent's, and its next interval is the one its median time past
    /// reaches. `known` finds the parent's ancestors; with fewer than ten of
    /// them known the pins decide the next interval by height.
    pub fn known_on(
        &self,
        header: Header,
        parent: &KnownBchHeader,
        known: impl Fn(&BlockHash) -> Option<KnownBchHeader>,
    ) -> Option<KnownBchHeader> {
        let height = parent.height.checked_add(1)?;
        let times = median_times_on(header.time, parent, known);
        let tick_info = self
            .tick_schedule
            .tick_info_on(parent.tick_info(), height, &times);
        Some(KnownBchHeader {
            height,
            header,
            ticks: tick_info.ticks,
            next_interval: tick_info.next_interval,
        })
    }

    /// The nBits Bitcoin Cash requires of a block on `parent` dated
    /// `header_time`, or `None` where this node does not check it.
    ///
    /// The tick difference from the anchor and the new block's interval are
    /// the parent's own, and a test network block may take the proof-of-work
    /// limit when dated more than twice that interval after the parent.
    pub fn required_bits(
        &self,
        parent: &KnownBchHeader,
        header_time: u32,
    ) -> Option<CompactTarget> {
        match self.bits {
            BitsRule::Asert(asert) => {
                if asert.allow_pow_limit
                    && header_time
                        > parent
                            .header
                            .time
                            .saturating_add(parent.tick_info().min_difficulty_gap())
                {
                    return Some(CompactTarget::from_consensus(BCH_POW_LIMIT_BITS));
                }
                asert.next_bits(parent.tick_info(), parent.header.time)
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
    /// minimum-difficulty rule, which depends on the paying block's own time:
    /// a block dated late may take the proof-of-work limit, and a window
    /// sized by that would fall back to its share cap. Every node computes
    /// the same nBits from the same known parent, so no miner can choose it.
    pub fn sizing_bits(&self, parent: &KnownBchHeader) -> Option<CompactTarget> {
        match self.bits {
            BitsRule::Asert(asert) => asert.next_bits(parent.tick_info(), parent.header.time),
            BitsRule::Parent => Some(parent.header.bits),
            BitsRule::Unchecked => None,
        }
    }

    /// Check `header` from a peer against its known `parent`, at `now`
    /// (seconds since the epoch), and return it as known. `known` finds the
    /// parent's ancestors, for the header's median time past.
    pub fn check(
        &self,
        header: &Header,
        parent: &KnownBchHeader,
        known: impl Fn(&BlockHash) -> Option<KnownBchHeader>,
        now: u32,
    ) -> Result<KnownBchHeader, BchHeaderError> {
        let hash = header.block_hash();
        if header.prev_blockhash != parent.block_hash() {
            return Err(BchHeaderError::NotChild {
                header: hash,
                parent: parent.block_hash(),
            });
        }
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
        self.known_on(*header, parent, known)
            .ok_or(BchHeaderError::Height(hash))
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
pub(crate) mod tests;
