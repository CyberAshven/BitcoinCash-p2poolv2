// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The share-version vote: how a later share-chain rule change activates on
//! a live Bitcoin Cash share chain, without a flag day on a fresh chain.
//!
//! The idea is P2Pool v1's (lessons doc, V17), reimplemented here: every
//! share says which rule version it follows (`version`) and which version
//! its node would like to follow (`desired_version`, the newest it
//! implements). A share may move to the next version only once most of the
//! work, measured long enough ago, wanted it:
//!
//! - **The rule.** A share's version is its parent's, or one more than its
//!   parent's when at least [`SHARE_VERSION_ACTIVATION_PERCENT`] (60%) of the
//!   work in the vote span desires that version or a newer one. The vote span
//!   is the oldest tenth of the [`SHARE_VERSION_VOTE_DEPTH`] shares below the
//!   parent (the shares from 23,328 to 25,920 below it), so the support was
//!   there about three days earlier and a short burst of hashrate cannot
//!   switch the chain. A switch needs a parent at least that high. A version
//!   is never lowered, and a node refuses a version newer than it implements
//!   ([`MAX_SUPPORTED_SHARE_VERSION`]). A share cannot desire a version below
//!   its own.
//! - **The policy.** A node produces the next version only when
//!   [`SHARE_VERSION_SWITCH_PERCENT`] (95%) of the work in the same span
//!   desires it, so the switch comes after nearly every node has upgraded
//!   and well inside what the rule allows. Nodes still on the old version
//!   then reject the new shares and should upgrade.
//!
//! Work is each share's own (`2^256 / (target + 1)` of its share target), as
//! the PPLNS window weighs it. Uncles are not counted. Version 1 is the rule
//! set of the 2026-10-24 batch, and this build implements no other, so no
//! switch can happen yet: the machinery is in place for the next rule change.

use super::bch_asert::U256;
use bitcoin::consensus::{Decodable, Encodable};
use bitcoin::{Target, Work};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The share rule version this build's shares follow: the 2026-10-24 rule
/// batch on its fresh share chain.
pub const BCH_SHARE_VERSION: u32 = 1;

/// The newest share rule version this build implements, which its shares
/// desire. A later rule change raises it and keeps the old rules for shares
/// of the old version.
pub const MAX_SUPPORTED_SHARE_VERSION: u32 = 1;

/// Shares below a parent that a vote reaches: three days of 10-second
/// shares, the span P2Pool v1 on Bitcoin Cash used (its `CHAIN_LENGTH`).
pub const SHARE_VERSION_VOTE_DEPTH: u32 = 3 * 24 * 60 * 60 / 10;

/// Shares whose votes count: the oldest tenth of the vote depth, 2,592.
pub const SHARE_VERSION_VOTE_SPAN: u32 = SHARE_VERSION_VOTE_DEPTH / 10;

/// Percent of the span's work that must desire the next version for a share
/// to switch to it: a share-chain rule.
pub const SHARE_VERSION_ACTIVATION_PERCENT: u32 = 60;

/// Percent of the span's work that must desire the next version before a
/// node produces it: node policy, not a rule.
pub const SHARE_VERSION_SWITCH_PERCENT: u32 = 95;

/// Name of the share-version rule in Bitcoin Cash protocol strings.
pub const BCH_SHARE_VERSION_RULE_TAG: &str = "version-vote";

/// The rule version a share follows and the one its node desires.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ShareVersion {
    /// The share-chain rule version this share follows.
    pub version: u32,
    /// The newest version the share's node implements: its vote.
    pub desired_version: u32,
}

impl ShareVersion {
    /// What this build's shares carry until a vote moves the chain on.
    pub const CURRENT: Self = Self {
        version: BCH_SHARE_VERSION,
        desired_version: MAX_SUPPORTED_SHARE_VERSION,
    };
}

impl Default for ShareVersion {
    fn default() -> Self {
        Self::CURRENT
    }
}

impl Encodable for ShareVersion {
    fn consensus_encode<W: bitcoin::io::Write + ?Sized>(
        &self,
        writer: &mut W,
    ) -> Result<usize, bitcoin::io::Error> {
        Ok(self.version.consensus_encode(writer)?
            + self.desired_version.consensus_encode(writer)?)
    }
}

impl Decodable for ShareVersion {
    fn consensus_decode<R: bitcoin::io::Read + ?Sized>(
        reader: &mut R,
    ) -> Result<Self, bitcoin::consensus::encode::Error> {
        Ok(Self {
            version: u32::consensus_decode(reader)?,
            desired_version: u32::consensus_decode(reader)?,
        })
    }
}

/// The work in a vote span, by the version each share desires.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VersionTally {
    by_desired_version: BTreeMap<u32, U256>,
    total: U256,
}

impl VersionTally {
    /// Count one share: its work for the version it desires. Sums saturate,
    /// which no real span reaches.
    pub fn add(&mut self, desired_version: u32, work: Work) {
        let work = U256::from_little_endian(&work.to_le_bytes());
        let entry = self.by_desired_version.entry(desired_version).or_default();
        *entry = entry.saturating_add(work);
        self.total = self.total.saturating_add(work);
    }

    /// Count a share by its share target.
    pub fn add_share(&mut self, share_version: ShareVersion, bits: bitcoin::CompactTarget) {
        self.add(
            share_version.desired_version,
            Target::from_compact(bits).to_work(),
        );
    }

    /// True when at least `percent` (at most 100) of the tallied work desires
    /// `version` or a newer one. An empty tally supports nothing.
    pub fn supports(&self, version: u32, percent: u32) -> bool {
        if self.total.is_zero() {
            return false;
        }
        let supporting = self
            .by_desired_version
            .range(version..)
            .fold(U256::zero(), |sum, (_, work)| sum.saturating_add(*work));
        // supporting * 100 >= total * percent, with both shifted right
        // enough that neither product can overflow 256 bits.
        let shift = self.total.bits().saturating_sub(248);
        (supporting >> shift) * U256::from(100u8)
            >= (self.total >> shift) * U256::from(percent.min(100))
    }
}

/// Why a share's version breaks the vote rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShareVersionError {
    /// The share desires a version older than the one it follows.
    DesiresOlder { version: u32, desired_version: u32 },
    /// The share follows a version this build does not implement.
    Unsupported { version: u32 },
    /// The share's version is neither its parent's nor the next one.
    NotParentOrNext { version: u32, parent_version: u32 },
    /// The share switches before the chain has a full vote depth below it.
    SwitchWithoutHistory { parent_height: u32 },
    /// The share switches without the work in the vote span behind it.
    SwitchWithoutSupport { version: u32 },
}

impl std::fmt::Display for ShareVersionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DesiresOlder {
                version,
                desired_version,
            } => write!(
                formatter,
                "share follows version {version} but desires older version {desired_version}"
            ),
            Self::Unsupported { version } => write!(
                formatter,
                "share follows version {version}, newer than this node implements ({MAX_SUPPORTED_SHARE_VERSION}); upgrade the node"
            ),
            Self::NotParentOrNext {
                version,
                parent_version,
            } => write!(
                formatter,
                "share version {version} cannot follow parent version {parent_version}"
            ),
            Self::SwitchWithoutHistory { parent_height } => write!(
                formatter,
                "share switches version above parent height {parent_height}, below the {SHARE_VERSION_VOTE_DEPTH}-share vote depth"
            ),
            Self::SwitchWithoutSupport { version } => write!(
                formatter,
                "share switches to version {version} without {SHARE_VERSION_ACTIVATION_PERCENT}% of the vote span's work desiring it"
            ),
        }
    }
}

impl std::error::Error for ShareVersionError {}

/// Checks a share needs only its own header for: it implements a version
/// this build knows, and does not desire an older one.
pub fn check_share_version_alone(share_version: ShareVersion) -> Result<(), ShareVersionError> {
    if share_version.version > MAX_SUPPORTED_SHARE_VERSION {
        return Err(ShareVersionError::Unsupported {
            version: share_version.version,
        });
    }
    if share_version.desired_version < share_version.version {
        return Err(ShareVersionError::DesiresOlder {
            version: share_version.version,
            desired_version: share_version.desired_version,
        });
    }
    Ok(())
}

/// The vote rule for a share whose parent follows `parent_version` at
/// `parent_height`. `tally` counts the parent's vote span; it is asked for
/// only when the share switches, since that walk reaches thousands of shares.
pub fn check_share_version<E>(
    share_version: ShareVersion,
    parent_version: u32,
    parent_height: u32,
    tally: impl FnOnce() -> Result<VersionTally, E>,
) -> Result<Result<(), ShareVersionError>, E> {
    if let Err(error) = check_share_version_alone(share_version) {
        return Ok(Err(error));
    }
    let version = share_version.version;
    if version == parent_version {
        return Ok(Ok(()));
    }
    if parent_version.checked_add(1) != Some(version) {
        return Ok(Err(ShareVersionError::NotParentOrNext {
            version,
            parent_version,
        }));
    }
    if parent_height < SHARE_VERSION_VOTE_DEPTH {
        return Ok(Err(ShareVersionError::SwitchWithoutHistory {
            parent_height,
        }));
    }
    if !tally()?.supports(version, SHARE_VERSION_ACTIVATION_PERCENT) {
        return Ok(Err(ShareVersionError::SwitchWithoutSupport { version }));
    }
    Ok(Ok(()))
}

/// The version a node gives its next share on a parent following
/// `parent_version` at `parent_height`: the next version once
/// [`SHARE_VERSION_SWITCH_PERCENT`] of the vote span's work desires it and
/// this build implements it, else the parent's. `tally` is asked for only
/// when this build implements a newer version than the parent's.
pub fn next_share_version<E>(
    parent_version: u32,
    parent_height: u32,
    tally: impl FnOnce() -> Result<VersionTally, E>,
) -> Result<ShareVersion, E> {
    let next = parent_version.saturating_add(1);
    let version = if next <= MAX_SUPPORTED_SHARE_VERSION
        && parent_height >= SHARE_VERSION_VOTE_DEPTH
        && tally()?.supports(next, SHARE_VERSION_SWITCH_PERCENT)
    {
        next
    } else {
        parent_version
    };
    Ok(ShareVersion {
        version,
        desired_version: MAX_SUPPORTED_SHARE_VERSION.max(version),
    })
}

#[cfg(test)]
mod tests;
