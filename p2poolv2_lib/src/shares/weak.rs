// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Weak shares on the Bitcoin Cash share chain
//! (`docs/sharechain/small-miners-design.md`, part 3).
//!
//! A weak share is an easier proof of work on an ordinary job at the minimum
//! that a later share carries, the way it names uncles. It meets the weak
//! target of its parent but not its own share target:
//!
//! ```text
//! T_weak(parent) = min(T_asert(parent) * R(parent), T_ceiling)
//! ```
//!
//! `T_asert(parent)` is the parent's minimum, its `asert_bits`, and `R` the
//! parent's `weak_ratio` in units of 1/1024: a fixed-point state carried
//! from share to share like the band cap ([`next_weak_ratio`]), falling when
//! shares carry more than [`WEAK_FILL`] weak shares and rising when they
//! carry fewer, so the weak difficulty follows the home miners who use it.
//!
//! Every proof of work from a job at the minimum, a full share, an uncle or
//! a weak share, is credited in the PPLNS window at the weak work
//! `2^256 / (T_weak(parent) + 1)` ([`weak_work`]); a share heavier than the
//! minimum at its own work. Weak work never counts in fork choice.
//!
//! These are share-chain rules. A Bitcoin Cash node never sees them: the
//! blocks P2Pool finds stay ordinary Bitcoin Cash blocks, and a weak share
//! only changes who the coinbase of a found block pays.

use crate::pool_difficulty::{U512, target_to_u512, u512_to_target};
use crate::shares::coinbase_proof::maximum_merkle_branch_length;
use crate::shares::share_block::{
    ShareHeader, checked_payout_level, decode_address, decode_optional_address, encode_address,
    encode_optional_address,
};
use bitcoin::consensus::encode::Error::ParseFailed;
use bitcoin::consensus::{Decodable, Encodable};
use bitcoin::hashes::{Hash, sha256d};
use bitcoin::{BlockHash, CompactTarget, ScriptBuf, Target, TxMerkleNode, VarInt};
use p2poolv2_wallet::payout_address::PayoutAddress;
use p2poolv2_wallet::share_owner::{self, ShareOwner};
use serde::{Deserialize, Serialize};

/// `weak_ratio` of a ratio of one, `R = 1`: the weak target is the minimum.
pub const WEAK_RATIO_UNIT: u32 = 1024;

/// `R_MAX`: a weak share is at most 256 times easier than the minimum
/// (4096 until 2026-11-12, study #85).
pub const WEAK_RATIO_MAX_MULTIPLE: u32 = 256;

/// The largest `weak_ratio`, `R_MAX` in units of 1/1024.
pub const WEAK_RATIO_MAX: u32 = WEAK_RATIO_MAX_MULTIPLE * WEAK_RATIO_UNIT;

/// The genesis share's `weak_ratio`: the easiest weak target, so a young
/// chain's first weak shares are easy and `R` falls as the slots fill.
pub const GENESIS_WEAK_RATIO: u32 = WEAK_RATIO_MAX;

/// One step of the weak ratio is `1 / WEAK_RATIO_STEP_DIVISOR` of the ratio.
pub const WEAK_RATIO_STEP_DIVISOR: u32 = 256;

/// `WEAK_CAP`: the most weak shares one share may carry.
pub const WEAK_CAP: usize = 64;

/// `WEAK_FILL`: the number of carried weak shares that keeps `R` where it
/// is, half the cap. More lower `R`, fewer raise it.
pub const WEAK_FILL: u16 = 32;

/// `WEAK_BYTES`: the most bytes of weak shares one share may carry, 96 KiB.
pub const WEAK_BYTES: usize = 96 * 1024;

/// `WEAK_DEPTH`: a carried weak share's parent is at most this many shares
/// below the share that carries it, 8 minutes at
/// 10-second shares (6 until 2026-11-12, study #85: deep enough that a
/// coalition of most of the hashrate gains nothing by refusing to carry
/// others' weak shares).
pub const WEAK_DEPTH: u32 = 48;

/// `INCLUDER_CUT`: the part of a weak share's credit the share carrying it
/// earns, in basis points (1%; 5% until 2026-11-12, study #85).
pub const INCLUDER_CUT: u16 = 100;

/// Basis points in one.
const BASIS_POINTS: u128 = 10_000;

/// The most scriptSig bytes a weak share's Bitcoin Cash coinbase carries:
/// BCH's limit on a coinbase scriptSig.
pub const MAX_WEAK_SCRIPT_SIG_BYTES: usize = 100;

/// Name of the weak-share rule in Bitcoin Cash protocol strings: at most
/// [`WEAK_CAP`] (64) weak shares a share, an includer cut of
/// [`INCLUDER_CUT`] (100) basis points, a depth of [`WEAK_DEPTH`] (48) and an
/// `R_MAX` of [`WEAK_RATIO_MAX_MULTIPLE`] (256).
///
/// `weak2` since 2026-11-12: a share's weak root commits each carried weak
/// share's whole encoding ([`WeakShare::committed_hash`]), where `weak`
/// committed only their proof-of-work hashes and left the rest of each weak
/// share free for a relaying peer to change. The two compute different roots
/// for every share carrying weak shares, so nodes on them must not peer: a
/// flag day, with a fresh store.
///
/// `weak2-64-100-48-256` since 2026-11-12 (study #85): the cut fell from 500
/// basis points, the depth rose from 6 and `R_MAX` fell from 4096, and the
/// tag gained `R_MAX`. Every share's weak credit and every genesis share
/// (its weak ratio is `R_MAX`) changed: a flag day, with a fresh store.
pub const BCH_WEAK_SHARE_RULE_TAG: &str = "weak2-64-100-48-256";

/// The weak ratio of a share whose parent has `parent_weak_ratio` and
/// carried `parent_weak_count` weak shares: one step from the parent's.
///
/// ```text
/// weak_ratio(child) = parent.weak_ratio - parent.weak_ratio / 256   if the parent carried more than WEAK_FILL
/// weak_ratio(child) = parent.weak_ratio + parent.weak_ratio / 256   if it carried fewer
/// weak_ratio(child) = parent.weak_ratio                            if exactly WEAK_FILL
/// then clamped to 1024 .. R_MAX * 1024
/// ```
///
/// Integer arithmetic only, so every node computes the same ratio.
pub fn next_weak_ratio(parent_weak_ratio: u32, parent_weak_count: u16) -> u32 {
    let weak_ratio = u64::from(parent_weak_ratio);
    let step = weak_ratio / u64::from(WEAK_RATIO_STEP_DIVISOR);
    let next = match parent_weak_count.cmp(&WEAK_FILL) {
        std::cmp::Ordering::Greater => weak_ratio - step,
        std::cmp::Ordering::Less => weak_ratio + step,
        std::cmp::Ordering::Equal => weak_ratio,
    };
    //* The clamp keeps the result within u32.
    next.clamp(u64::from(WEAK_RATIO_UNIT), u64::from(WEAK_RATIO_MAX)) as u32
}

/// The weak target of a share with minimum `minimum` and weak ratio
/// `weak_ratio`, on a chain whose easiest share target is `ceiling`: the
/// target weak shares mined on that share as their parent must meet.
///
/// `min(minimum * weak_ratio / 1024, ceiling)`, compared on the exact
/// 256-bit targets, and never harder than the minimum itself: on a real
/// chain the minimum is never easier than the ceiling, so that bound only
/// matters for a share easier than the ceiling, whose weak target is then
/// its own minimum. A ratio outside `1024 .. R_MAX * 1024` is read clamped,
/// as a header that carries none reads zero.
pub fn weak_target(minimum: CompactTarget, weak_ratio: u32, ceiling: CompactTarget) -> Target {
    let ratio = weak_ratio.clamp(WEAK_RATIO_UNIT, WEAK_RATIO_MAX);
    let minimum_wide = target_to_u512(Target::from_compact(minimum));
    let scaled = minimum_wide * U512::from(ratio) / U512::from(WEAK_RATIO_UNIT);
    let ceiling_wide = target_to_u512(Target::from_compact(ceiling));
    let capped = if scaled > ceiling_wide {
        ceiling_wide
    } else {
        scaled
    };
    let weak = if capped < minimum_wide {
        minimum_wide
    } else {
        capped
    };
    u512_to_target(weak)
}

/// The weak target of `parent`, the share weak shares are mined on: from its
/// minimum and its weak ratio ([`weak_target`]).
pub fn parent_weak_target(parent: &ShareHeader, ceiling: CompactTarget) -> Target {
    weak_target(parent.minimum_bits(), parent.weak_ratio, ceiling)
}

/// The work of a target as the PPLNS window counts it, `2^256 / (T + 1)`
/// rounded down, saturating at `u128::MAX`.
pub fn target_work(target: Target) -> u128 {
    let bytes = target.to_work().to_le_bytes();
    let (low, high) = bytes.split_at(16);
    if high.iter().any(|byte| *byte != 0) {
        return u128::MAX;
    }
    u128::from_le_bytes(low.try_into().expect("16 bytes"))
}

/// The weak work of a proof mined on `parent`: the work of the parent's
/// weak target ([`parent_weak_target`]), the credit every proof from a job
/// at the minimum on that parent earns.
pub fn weak_work(parent: &ShareHeader, ceiling: CompactTarget) -> u128 {
    target_work(parent_weak_target(parent, ceiling))
}

/// Whether `header` was mined on a job at the minimum: its own target is
/// its minimum. Only such proofs earn the weak work; a heavier share is
/// credited at its own work, and a Bitcoin header, which carries no band,
/// is never at a minimum of its own.
pub fn is_at_minimum(header: &ShareHeader) -> bool {
    header.carries_band() && header.bits == header.asert_bits
}

/// The part of a weak credit of `scaled_weight` the carrying share earns,
/// [`INCLUDER_CUT`] of it, rounded down; the weak share's own payee earns
/// the rest, so the two add up to the whole credit.
pub fn includer_part(scaled_weight: u128) -> u128 {
    scaled_weight.saturating_mul(u128::from(INCLUDER_CUT)) / BASIS_POINTS
}

/// A weak share as it is relayed and carried: the share header as mined,
/// with the parts of its Bitcoin Cash coinbase a node cannot rebuild itself.
///
/// The payout outputs are not carried: every node computes them from the
/// PPLNS window anchored at the header's parent, for the coinbase's total
/// value. A weak share is identified by its proof-of-work hash, the hash of
/// the Bitcoin Cash header it carries ([`Self::pow_hash`]).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct WeakShare {
    /// The share header the miner hashed, exactly as a full share on its job
    /// would carry it.
    pub header: ShareHeader,
    /// The Bitcoin Cash coinbase's scriptSig: height, aux flags, extranonce,
    /// nanosecond timestamp and pool signature. At most
    /// [`MAX_WEAK_SCRIPT_SIG_BYTES`].
    pub coinbase_script_sig: ScriptBuf,
    /// The coinbase's total output value in satoshis, which the window's
    /// payouts divide.
    pub coinbase_value: u64,
    /// The coinbase merkle branch, from the coinbase to the Bitcoin Cash
    /// header's merkle root.
    pub merkle_branch: Vec<TxMerkleNode>,
}

impl WeakShare {
    /// The weak share's identity: the hash of the Bitcoin Cash header it
    /// carries, its proof of work. One proof of work is credited once, as a
    /// share or as a weak share.
    pub fn pow_hash(&self) -> BlockHash {
        self.header.bitcoin_header.block_hash()
    }

    /// What a carrying share's weak root commits for this weak share
    /// ([`weak_root`]): the SHA-256d of its whole consensus encoding. The
    /// carrier's proof of work then fixes every field a weak share check
    /// reads: its share header with the coinbase proof in it, its coinbase
    /// scriptSig, its coinbase value and its merkle branch. Its identity, for
    /// the repeat and carried-once checks, stays [`Self::pow_hash`].
    pub fn committed_hash(&self) -> sha256d::Hash {
        let mut engine = sha256d::Hash::engine();
        self.consensus_encode(&mut engine)
            .expect("engine doesn't error");
        sha256d::Hash::from_engine(engine)
    }

    /// Size of the weak share's consensus encoding, the bytes it takes of a
    /// carrying share's [`WEAK_BYTES`].
    pub fn encoded_size(&self) -> usize {
        self.consensus_encode(&mut bitcoin::io::sink())
            .unwrap_or(usize::MAX)
    }
}

impl Encodable for WeakShare {
    fn consensus_encode<W: bitcoin::io::Write + ?Sized>(
        &self,
        writer: &mut W,
    ) -> Result<usize, bitcoin::io::Error> {
        let mut length = self.header.consensus_encode(writer)?;
        length += self.coinbase_script_sig.consensus_encode(writer)?;
        length += self.coinbase_value.consensus_encode(writer)?;
        length += VarInt(self.merkle_branch.len() as u64).consensus_encode(writer)?;
        for node in &self.merkle_branch {
            length += node.consensus_encode(writer)?;
        }
        Ok(length)
    }
}

impl Decodable for WeakShare {
    fn consensus_decode<R: bitcoin::io::Read + ?Sized>(
        reader: &mut R,
    ) -> Result<Self, bitcoin::consensus::encode::Error> {
        let header = ShareHeader::consensus_decode(reader)?;
        if !header.carries_band() {
            return Err(ParseFailed(
                "a weak share needs a Bitcoin Cash share header",
            ));
        }
        let coinbase_script_sig = ScriptBuf::consensus_decode(reader)?;
        if coinbase_script_sig.len() > MAX_WEAK_SCRIPT_SIG_BYTES {
            return Err(ParseFailed("weak share coinbase scriptSig too long"));
        }
        let coinbase_value = u64::consensus_decode(reader)?;
        let branch_length = VarInt::consensus_decode(reader)?.0 as usize;
        if branch_length > maximum_merkle_branch_length(header.miner_address) {
            return Err(ParseFailed("weak share merkle branch too long"));
        }
        let mut merkle_branch = Vec::with_capacity(branch_length);
        for _ in 0..branch_length {
            merkle_branch.push(TxMerkleNode::consensus_decode(reader)?);
        }
        Ok(Self {
            header,
            coinbase_script_sig,
            coinbase_value,
            merkle_branch,
        })
    }
}

/// The root a share header commits for the weak shares it carries: the
/// merkle root of their committed hashes ([`WeakShare::committed_hash`], the
/// SHA-256d of each one's whole encoding), in the order carried, or all zeros
/// for none.
///
/// Every byte of a carried weak share is in its leaf, so a body whose weak
/// shares differ in any field from the ones the miner carried fails the body
/// check as a bad copy, and a weak share check that fails once the body is
/// stored judges what the miner committed. Until 2026-11-12 the leaves were
/// the weak shares' proof-of-work hashes, which left the rest of each weak
/// share unbound: a peer could change one in an honest share's body and have
/// the share marked invalid (`BCH_WEAK_SHARE_RULE_TAG`).
///
/// The tree pairs the last leaf of an odd level with itself, so a repeated
/// last weak share would keep the root (CVE-2012-2459); the body check
/// refuses a list that repeats a proof-of-work hash before it compares the
/// root.
pub fn weak_root<'a>(weak_shares: impl IntoIterator<Item = &'a WeakShare>) -> sha256d::Hash {
    bitcoin::merkle_tree::calculate_root(weak_shares.into_iter().map(WeakShare::committed_hash))
        .unwrap_or_else(sha256d::Hash::all_zeros)
}

/// The root a header carrying no weak shares commits: all zeros.
pub fn empty_weak_root() -> sha256d::Hash {
    sha256d::Hash::all_zeros()
}

/// The carried weak shares of one share that pay one payee alike: mined on
/// the same parent, paying the same address with the same donation, fee and
/// payout level and building on the same Bitcoin Cash block. Each is
/// credited at the weak work of that parent, so a group stands for `count`
/// equal credits.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct WeakCreditGroup {
    /// The share the weak shares were mined on: its weak target sets their
    /// weak work.
    pub parent: BlockHash,
    /// Where their credit goes, after their own donation and fee.
    pub payout_address: PayoutAddress,
    /// Their share owner, for looking a window up by share chain address.
    pub share_owner: ShareOwner,
    /// Their donation address and basis points (`cuts-per-share`).
    pub donation_address: Option<PayoutAddress>,
    pub donation: Option<u16>,
    /// Their fee address and basis points.
    pub fee_address: Option<PayoutAddress>,
    pub fee: Option<u16>,
    /// Their payout level (`ShareHeader::payout_level`): the level their
    /// payee is drawn at when one of them is its newest credited entry.
    pub payout_level: u8,
    /// The Bitcoin Cash block they build on, which places their work on a
    /// Bitcoin Cash chain (`bch_eligibility`).
    pub bch_prev: BlockHash,
    /// How many carried weak shares the group holds.
    pub count: u32,
}

impl WeakCreditGroup {
    /// Whether `header`'s weak share belongs to this group.
    fn holds(&self, header: &ShareHeader) -> bool {
        self.parent == header.prev_share_blockhash
            && self.payout_address == header.miner_bitcoin_address
            && self.share_owner == header.miner_address
            && self.donation_address == header.donation_address
            && self.donation == header.donation
            && self.fee_address == header.fee_address
            && self.fee == header.fee
            && self.payout_level == header.payout_level
            && self.bch_prev == header.bitcoin_header.prev_blockhash
    }

    /// A group of one weak share with `header`.
    fn of(header: &ShareHeader) -> Self {
        Self {
            parent: header.prev_share_blockhash,
            payout_address: header.miner_bitcoin_address.clone(),
            share_owner: header.miner_address,
            donation_address: header.donation_address.clone(),
            donation: header.donation,
            fee_address: header.fee_address.clone(),
            fee: header.fee,
            payout_level: header.payout_level,
            bch_prev: header.bitcoin_header.prev_blockhash,
            count: 1,
        }
    }
}

/// What the PPLNS window and the carried-once rule read of a share's carried
/// weak shares, stored beside the share so neither reads the weak shares
/// themselves: their proof-of-work hashes, in order, and their credit groups
/// ([`WeakCreditGroup`]), in the order each group first appears.
///
/// A function of the carried list alone, which the header's weak root fixes,
/// so every node stores the same summary for the same share.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct WeakSummary {
    pub pow_hashes: Vec<BlockHash>,
    pub groups: Vec<WeakCreditGroup>,
}

impl WeakSummary {
    /// The summary of the weak shares a share carries.
    pub fn of(weak_shares: &[WeakShare]) -> Self {
        let mut groups: Vec<WeakCreditGroup> = Vec::with_capacity(weak_shares.len());
        for weak_share in weak_shares {
            match groups
                .iter_mut()
                .find(|group| group.holds(&weak_share.header))
            {
                Some(group) => group.count += 1,
                None => groups.push(WeakCreditGroup::of(&weak_share.header)),
            }
        }
        Self {
            pow_hashes: weak_shares.iter().map(WeakShare::pow_hash).collect(),
            groups,
        }
    }
}

impl Encodable for WeakCreditGroup {
    fn consensus_encode<W: bitcoin::io::Write + ?Sized>(
        &self,
        writer: &mut W,
    ) -> Result<usize, bitcoin::io::Error> {
        let mut length = self.parent.consensus_encode(writer)?;
        length += encode_address(&self.payout_address, writer)?;
        length += share_owner::consensus_encode(&self.share_owner, writer)?;
        length += encode_optional_address(&self.donation_address, writer)?;
        length += self.donation.unwrap_or(0).consensus_encode(writer)?;
        length += encode_optional_address(&self.fee_address, writer)?;
        length += self.fee.unwrap_or(0).consensus_encode(writer)?;
        length += self.payout_level.consensus_encode(writer)?;
        length += self.bch_prev.consensus_encode(writer)?;
        length += self.count.consensus_encode(writer)?;
        Ok(length)
    }
}

impl Decodable for WeakCreditGroup {
    fn consensus_decode<R: bitcoin::io::Read + ?Sized>(
        reader: &mut R,
    ) -> Result<Self, bitcoin::consensus::encode::Error> {
        let parent = BlockHash::consensus_decode(reader)?;
        let payout_address = decode_address(reader)?;
        let share_owner = share_owner::consensus_decode(reader)?;
        let donation_address = decode_optional_address(reader)?;
        let donation = Some(u16::consensus_decode(reader)?).filter(|cut| *cut > 0);
        let fee_address = decode_optional_address(reader)?;
        let fee = Some(u16::consensus_decode(reader)?).filter(|cut| *cut > 0);
        let payout_level = checked_payout_level(u8::consensus_decode(reader)?)?;
        Ok(Self {
            parent,
            payout_address,
            share_owner,
            donation_address,
            donation,
            fee_address,
            fee,
            payout_level,
            bch_prev: BlockHash::consensus_decode(reader)?,
            count: u32::consensus_decode(reader)?,
        })
    }
}

impl Encodable for WeakSummary {
    fn consensus_encode<W: bitcoin::io::Write + ?Sized>(
        &self,
        writer: &mut W,
    ) -> Result<usize, bitcoin::io::Error> {
        let mut length = self.pow_hashes.consensus_encode(writer)?;
        length += VarInt(self.groups.len() as u64).consensus_encode(writer)?;
        for group in &self.groups {
            length += group.consensus_encode(writer)?;
        }
        Ok(length)
    }
}

impl Decodable for WeakSummary {
    fn consensus_decode<R: bitcoin::io::Read + ?Sized>(
        reader: &mut R,
    ) -> Result<Self, bitcoin::consensus::encode::Error> {
        let pow_hashes = Vec::<BlockHash>::consensus_decode(reader)?;
        let group_count = VarInt::consensus_decode(reader)?.0 as usize;
        if pow_hashes.len() > WEAK_CAP || group_count > pow_hashes.len() {
            return Err(ParseFailed("weak share summary too long"));
        }
        let mut groups = Vec::with_capacity(group_count);
        for _ in 0..group_count {
            groups.push(WeakCreditGroup::consensus_decode(reader)?);
        }
        Ok(Self { pow_hashes, groups })
    }
}

/// A share's carried weak shares as one stored value: a count, then each.
pub(crate) struct StoredWeakShares(pub Vec<WeakShare>);

impl Encodable for StoredWeakShares {
    fn consensus_encode<W: bitcoin::io::Write + ?Sized>(
        &self,
        writer: &mut W,
    ) -> Result<usize, bitcoin::io::Error> {
        let mut length = VarInt(self.0.len() as u64).consensus_encode(writer)?;
        for weak_share in &self.0 {
            length += weak_share.consensus_encode(writer)?;
        }
        Ok(length)
    }
}

impl Decodable for StoredWeakShares {
    fn consensus_decode<R: bitcoin::io::Read + ?Sized>(
        reader: &mut R,
    ) -> Result<Self, bitcoin::consensus::encode::Error> {
        let count = VarInt::consensus_decode(reader)?.0 as usize;
        if count > WEAK_CAP {
            return Err(ParseFailed("too many stored weak shares"));
        }
        let mut weak_shares = Vec::with_capacity(count);
        for _ in 0..count {
            weak_shares.push(WeakShare::consensus_decode(reader)?);
        }
        Ok(Self(weak_shares))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::TestShareBlockBuilder;
    use bitcoin::consensus::{deserialize, serialize};
    use p2poolv2_wallet::share_owner::ShareOwner;

    /// A minimum whose mantissa leaves room to multiply exactly: `2^222`.
    const MINIMUM: u32 = 0x1c40_0000;

    /// A credit group's payout level is the same Bitcoin Cash level as a
    /// header's: level 0 does not decode, and the lowest level does.
    #[test]
    fn a_credit_group_below_the_lowest_level_does_not_decode() {
        use crate::accounting::payout::payout_levels::{MIN_PAYOUT_LEVEL, payout_level_too_low};
        use bitcoin::hashes::Hash;
        use bitcoin::{Address, BlockHash, Network, PubkeyHash};

        let group = |payout_level| WeakCreditGroup {
            parent: BlockHash::all_zeros(),
            payout_address: Address::p2pkh(
                PubkeyHash::from_byte_array([0x11; 20]),
                Network::Regtest,
            )
            .into(),
            share_owner: ShareOwner::CashP2pkh([0x11; 20]),
            donation_address: None,
            donation: None,
            fee_address: None,
            fee: None,
            payout_level,
            bch_prev: BlockHash::all_zeros(),
            count: 1,
        };

        let error = deserialize::<WeakCreditGroup>(&serialize(&group(0))).unwrap_err();
        assert!(
            error.to_string().contains(&payout_level_too_low(0)),
            "credit group level 0: {error}"
        );
        let lowest = group(MIN_PAYOUT_LEVEL);
        let decoded: WeakCreditGroup = deserialize(&serialize(&lowest)).unwrap();
        assert_eq!(decoded, lowest, "credit group lowest level");
    }

    #[test]
    fn the_rule_tag_names_the_cap_the_includer_cut_and_the_depth() {
        assert_eq!(
            BCH_WEAK_SHARE_RULE_TAG,
            format!("weak2-{WEAK_CAP}-{INCLUDER_CUT}-{WEAK_DEPTH}-{WEAK_RATIO_MAX_MULTIPLE}")
        );
    }

    #[test]
    fn a_parent_carrying_more_than_the_fill_lowers_the_ratio_one_step() {
        assert_eq!(next_weak_ratio(256_000, WEAK_FILL + 1), 256_000 - 1_000);
    }

    #[test]
    fn a_parent_carrying_fewer_than_the_fill_raises_the_ratio_one_step() {
        assert_eq!(next_weak_ratio(256_000, WEAK_FILL - 1), 256_000 + 1_000);
    }

    #[test]
    fn a_parent_carrying_exactly_the_fill_keeps_the_ratio() {
        assert_eq!(next_weak_ratio(256_000, WEAK_FILL), 256_000);
    }

    #[test]
    fn a_genesis_parent_keeps_the_easiest_ratio() {
        assert_eq!(next_weak_ratio(GENESIS_WEAK_RATIO, 0), WEAK_RATIO_MAX);
    }

    #[test]
    fn the_ratio_never_rises_past_r_max() {
        assert_eq!(next_weak_ratio(WEAK_RATIO_MAX - 1, 0), WEAK_RATIO_MAX);
    }

    #[test]
    fn the_ratio_never_falls_below_one() {
        assert_eq!(
            next_weak_ratio(WEAK_RATIO_UNIT, WEAK_CAP as u16),
            WEAK_RATIO_UNIT
        );
    }

    #[test]
    fn a_ratio_below_one_from_a_parent_without_one_steps_to_one() {
        assert_eq!(next_weak_ratio(0, 0), WEAK_RATIO_UNIT);
    }

    #[test]
    fn the_weak_target_is_the_minimum_times_the_ratio() {
        let target = weak_target(
            CompactTarget::from_consensus(MINIMUM),
            4 * WEAK_RATIO_UNIT,
            CompactTarget::from_consensus(0x1e00_ffff),
        );

        assert_eq!(
            target,
            Target::from_compact(CompactTarget::from_consensus(0x1d01_0000))
        );
    }

    #[test]
    fn the_weak_target_stops_at_the_ceiling() {
        let ceiling = CompactTarget::from_consensus(0x1c38_4bd7);
        let target = weak_target(
            CompactTarget::from_consensus(0x1c10_0000),
            WEAK_RATIO_MAX,
            ceiling,
        );

        assert_eq!(target, Target::from_compact(ceiling));
    }

    #[test]
    fn the_weak_target_is_never_harder_than_the_minimum() {
        let minimum = CompactTarget::from_consensus(0x1e03_77ae);
        let target = weak_target(
            minimum,
            WEAK_RATIO_MAX,
            CompactTarget::from_consensus(0x1c38_4bd7),
        );

        assert_eq!(target, Target::from_compact(minimum));
    }

    #[test]
    fn at_a_ratio_of_one_the_weak_work_is_the_minimums_work() {
        let parent = TestShareBlockBuilder::new()
            .share_owner(ShareOwner::CashP2pkh([0x11; 20]))
            .bits(CompactTarget::from_consensus(MINIMUM))
            .weak_ratio(WEAK_RATIO_UNIT)
            .build()
            .header;

        assert_eq!(
            weak_work(&parent, CompactTarget::from_consensus(0x1d00_ffff)),
            target_work(Target::from_compact(CompactTarget::from_consensus(MINIMUM)))
        );
    }

    #[test]
    fn the_includer_part_is_one_percent_rounded_down() {
        assert_eq!(includer_part(1_000), 10);
        assert_eq!(includer_part(99), 0);
        assert_eq!(includer_part(100), 1);
    }

    #[test]
    fn a_weak_share_round_trips() {
        let header = TestShareBlockBuilder::new()
            .share_owner(ShareOwner::CashP2pkh([0x11; 20]))
            .build()
            .header;
        let weak_share = WeakShare {
            header,
            coinbase_script_sig: ScriptBuf::from_bytes(vec![0x51; 40]),
            coinbase_value: 50_000_000,
            merkle_branch: vec![TxMerkleNode::from_byte_array([0x33; 32])],
        };

        let decoded: WeakShare = deserialize(&serialize(&weak_share)).unwrap();

        assert_eq!(decoded, weak_share);
        assert_eq!(weak_share.encoded_size(), serialize(&weak_share).len());
    }

    #[test]
    fn a_weak_share_with_a_scriptsig_past_bchs_limit_does_not_decode() {
        let header = TestShareBlockBuilder::new()
            .share_owner(ShareOwner::CashP2pkh([0x11; 20]))
            .build()
            .header;
        let weak_share = WeakShare {
            header,
            coinbase_script_sig: ScriptBuf::from_bytes(vec![0x51; MAX_WEAK_SCRIPT_SIG_BYTES + 1]),
            coinbase_value: 50_000_000,
            merkle_branch: Vec::new(),
        };

        assert!(deserialize::<WeakShare>(&serialize(&weak_share)).is_err());
    }

    #[test]
    fn a_weak_share_with_a_bitcoin_header_does_not_decode() {
        let weak_share = WeakShare {
            header: TestShareBlockBuilder::new().build().header,
            coinbase_script_sig: ScriptBuf::new(),
            coinbase_value: 50_000_000,
            merkle_branch: Vec::new(),
        };

        assert!(deserialize::<WeakShare>(&serialize(&weak_share)).is_err());
    }

    #[test]
    fn no_weak_shares_have_the_zero_root() {
        assert_eq!(weak_root(&[]), empty_weak_root());
    }

    #[test]
    fn the_weak_root_follows_the_order_carried() {
        let first = WeakShare {
            header: TestShareBlockBuilder::new()
                .share_owner(ShareOwner::CashP2pkh([0x11; 20]))
                .nonce(1)
                .build()
                .header,
            coinbase_script_sig: ScriptBuf::new(),
            coinbase_value: 1,
            merkle_branch: Vec::new(),
        };
        let second = WeakShare {
            header: TestShareBlockBuilder::new()
                .share_owner(ShareOwner::CashP2pkh([0x11; 20]))
                .nonce(2)
                .build()
                .header,
            coinbase_script_sig: ScriptBuf::new(),
            coinbase_value: 1,
            merkle_branch: Vec::new(),
        };

        assert_ne!(weak_root([&first, &second]), weak_root([&second, &first]));
        assert_eq!(weak_root([&first]), sha256d::Hash::hash(&serialize(&first)));
    }

    //* Each field a weak share check reads moves the root, while the
    //* proof-of-work hash, all the root covered before, stays the same.

    #[test]
    fn the_weak_root_changes_with_a_weak_shares_coinbase_value() {
        let honest = WeakShare {
            header: TestShareBlockBuilder::new()
                .share_owner(ShareOwner::CashP2pkh([0x11; 20]))
                .build()
                .header,
            coinbase_script_sig: ScriptBuf::from_bytes(vec![0x51; 40]),
            coinbase_value: 50_000_000,
            merkle_branch: vec![TxMerkleNode::from_byte_array([0x33; 32])],
        };
        let mut altered = honest.clone();
        altered.coinbase_value += 1;

        assert_eq!(altered.pow_hash(), honest.pow_hash());
        assert_ne!(weak_root([&altered]), weak_root([&honest]));
    }

    #[test]
    fn the_weak_root_changes_with_a_weak_shares_merkle_branch() {
        let honest = WeakShare {
            header: TestShareBlockBuilder::new()
                .share_owner(ShareOwner::CashP2pkh([0x11; 20]))
                .build()
                .header,
            coinbase_script_sig: ScriptBuf::from_bytes(vec![0x51; 40]),
            coinbase_value: 50_000_000,
            merkle_branch: vec![TxMerkleNode::from_byte_array([0x33; 32])],
        };
        let mut altered = honest.clone();
        altered.merkle_branch = vec![TxMerkleNode::from_byte_array([0x34; 32])];

        assert_eq!(altered.pow_hash(), honest.pow_hash());
        assert_ne!(weak_root([&altered]), weak_root([&honest]));
    }

    #[test]
    fn the_weak_root_changes_with_a_weak_shares_script_sig() {
        let honest = WeakShare {
            header: TestShareBlockBuilder::new()
                .share_owner(ShareOwner::CashP2pkh([0x11; 20]))
                .build()
                .header,
            coinbase_script_sig: ScriptBuf::from_bytes(vec![0x51; 40]),
            coinbase_value: 50_000_000,
            merkle_branch: vec![TxMerkleNode::from_byte_array([0x33; 32])],
        };
        let mut altered = honest.clone();
        altered.coinbase_script_sig = ScriptBuf::from_bytes(vec![0x52; 40]);

        assert_eq!(altered.pow_hash(), honest.pow_hash());
        assert_ne!(weak_root([&altered]), weak_root([&honest]));
    }

    #[test]
    fn the_weak_root_changes_with_a_weak_shares_coinbase_proof() {
        let honest = WeakShare {
            header: TestShareBlockBuilder::new()
                .share_owner(ShareOwner::CashP2pkh([0x11; 20]))
                .build()
                .header,
            coinbase_script_sig: ScriptBuf::from_bytes(vec![0x51; 40]),
            coinbase_value: 50_000_000,
            merkle_branch: vec![TxMerkleNode::from_byte_array([0x33; 32])],
        };
        let mut altered = honest.clone();
        altered.header.coinbase_proof.midstate[0] ^= 0x01;

        assert_eq!(altered.pow_hash(), honest.pow_hash());
        assert_ne!(weak_root([&altered]), weak_root([&honest]));
    }

    #[test]
    fn the_weak_root_changes_with_a_weak_shares_share_header() {
        let honest = WeakShare {
            header: TestShareBlockBuilder::new()
                .share_owner(ShareOwner::CashP2pkh([0x11; 20]))
                .build()
                .header,
            coinbase_script_sig: ScriptBuf::from_bytes(vec![0x51; 40]),
            coinbase_value: 50_000_000,
            merkle_branch: vec![TxMerkleNode::from_byte_array([0x33; 32])],
        };
        let mut altered = honest.clone();
        altered.header.time += 1;

        assert_eq!(altered.pow_hash(), honest.pow_hash());
        assert_ne!(weak_root([&altered]), weak_root([&honest]));
    }
}
