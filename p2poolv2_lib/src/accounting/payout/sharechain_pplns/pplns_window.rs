// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Incremental PPLNS window cache for share chain payout computation.
//!
//! `PplnsWindow` caches confirmed share headers and their uncle data,
//! allowing incremental updates (loading only newly confirmed headers)
//! instead of re-reading the full window from RocksDB on every notify.

use super::address_keys::AddressKeys;
use super::bch_eligibility::{BchPlace, PayingChain};
use super::payout_rights::{ChainShare, PayoutRights, has_share_transactions, share_coinbase_txid};
use crate::config::Chain;
use crate::shares::bch_headers::{BchHeaderRules, KnownBchHeader};
#[cfg(test)]
#[mockall_double::double]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
#[cfg(not(test))]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
use crate::shares::chain::chain_store_handle::ConfirmedHeaderResult;
use crate::shares::share_block::{ShareHeader, ShareTransaction};
use crate::store::dag_store::MAX_UNCLES_DEPTH;
use crate::store::writer::StoreError;
use bitcoin::Address;
use bitcoin::BlockHash;
use bitcoin::hashes::Hash;
use bitcoin::params::Params;
use bitcoin::{CompactTarget, PubkeyHash, ScriptBuf, Target, Txid, Work};
use p2poolv2_wallet::payout_address::PayoutAddress;
use p2poolv2_wallet::share_owner::ShareOwner;
use std::collections::{HashMap, HashSet, VecDeque};
use std::error::Error;
use std::fmt;
use std::hash;
use tracing::{debug, warn};

/// The PPLNS window: a coinbase pays at most `PPLNS_WINDOW_SHARES` shares,
/// each weighted by its difficulty, starting at the newest share that is
/// mature for it (see `COINBASE_MATURITY`).
///
/// On Bitcoin this is the window: a share count, not a difficulty target or a
/// time span. At the 10 s share target that is 6 * 60 * 24 * 14 = 120,960
/// shares, about two weeks. On Bitcoin Cash it is the cap on a window sized by
/// the paying block's difficulty (see `BCH_PPLNS_WINDOW_BLOCKS`).
///
/// The cache, its eviction buffer, `PRUNE_DEPTH`, the PPLNS zone and the
/// prevout depth are all sized by it, together with `COINBASE_MATURITY`
/// through `PAYOUT_DEPTH`.
///
/// The sim build uses 80 shares instead, with the other sim depths below, so
/// a fast sim chain reaches the prune depth and prunes often.
#[cfg(not(feature = "sim"))]
pub const PPLNS_WINDOW_SHARES: usize = 120960;
#[cfg(feature = "sim")]
pub const PPLNS_WINDOW_SHARES: usize = 80;

/// Bitcoin Cash only: a coinbase pays the shares holding this many expected
/// BCH blocks' worth of work, below the maturity skip and at most
/// `PPLNS_WINDOW_SHARES` shares.
///
/// One BCH block's worth of work is the difficulty in the paying share's own
/// BCH header, which BCH sets every block with ASERT. The window therefore
/// spans about this many of the pool's own blocks whatever the pool's size:
/// short for a large pool, long for a small one, and shorter by itself if BCH
/// blocks get faster. With 10-minute blocks, pools under about
/// `BCH_PPLNS_WINDOW_BLOCKS / 2016` of BCH's hashrate reach the cap and pay
/// upstream's fixed window.
///
/// 3, as P2Pool v1 on BCH (jtoomim's `SPREAD`, capped by its share count). A
/// share is paid by the blocks found while it is in the window, which is
/// Poisson with this mean: with 3 a share misses every block e^-3 = 5% of the
/// time and its payout varies by 1 / sqrt(3) = 58%, against 13.5% and 71%
/// with 2, while the window, the payout delay and the time a miner is paid
/// after stopping grow linearly with the value. A share-chain rule: every node
/// must use the same value.
pub const BCH_PPLNS_WINDOW_BLOCKS: u128 = 3;

/// A paying block difficulty at or below this carries no difficulty to size a
/// window by, and the window falls back to `PPLNS_WINDOW_SHARES`.
///
/// Difficulty is measured against the network's proof-of-work limit, so a
/// header at that limit has difficulty 1. Chipnet and testnet allow such a
/// block when it comes 20 minutes after its parent, and every regtest block is
/// one. Mainnet never has one. Falling back to the cap keeps such a block from
/// shrinking the window to a single share. Falling back to an older header
/// would not be safe: only a found block's bits are checked by BCH, so the
/// bits in any other share are whatever its miner chose.
const MIN_SIZING_BLOCK_DIFFICULTY: u128 = 1;

/// Difficulty of the bitcoin block a coinbase pays, from the bits in that
/// block's header, measured against the network's proof-of-work limit.
///
/// The producer passes the bits of the template it builds the job on, which
/// are the bits the share's header carries: a miner cannot change them, and a
/// declared job must keep them. A validator passes the bits in the share's
/// own header. Both must use this function, so both size the window from the
/// same number.
pub fn bitcoin_block_difficulty(bits: CompactTarget, network: bitcoin::Network) -> u128 {
    Target::from_compact(bits).difficulty(network)
}

/// The bitcoin block a coinbase pays, as far as its window needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PayingBlock {
    /// Difficulty of the nBits in the paying block's header
    /// (`bitcoin_block_difficulty`). It sizes a Bitcoin Cash window when the
    /// window cannot compute the nBits from `bch_prev`.
    pub claimed_difficulty: u128,
    /// The block the paying block's header builds on. A Bitcoin Cash window
    /// with header rules (`PplnsWindow::with_bch_header_rules`) sizes itself
    /// from it and pays only the shares on its chain, give or take one block
    /// (mining on the current chain). Other windows ignore it.
    pub bch_prev: Option<BlockHash>,
}

impl PayingBlock {
    /// A paying block claiming `claimed_difficulty` and building on
    /// `bch_prev`.
    pub fn new(claimed_difficulty: u128, bch_prev: BlockHash) -> Self {
        Self {
            claimed_difficulty,
            bch_prev: Some(bch_prev),
        }
    }
}

impl From<u128> for PayingBlock {
    /// A paying block known only by its claimed difficulty: the window is
    /// sized by it and pays every share.
    fn from(claimed_difficulty: u128) -> Self {
        Self {
            claimed_difficulty,
            bch_prev: None,
        }
    }
}

/// Coinbase outputs cannot be spent until the containing block is this
/// many blocks deep. 70% of blocks-per-day at 10s block time (0.70 * 86400 / 10).
///
/// A share is paid by a found block only at the same depth, except that the
/// first `COINBASE_MATURITY` shares of the chain are always mature for payout.
///
/// Bitcoin's maturity. Bitcoin Cash uses `BCH_COINBASE_MATURITY`; read the
/// maturity for a chain with `coinbase_maturity_for`.
///
/// The sim build uses 20 shares, so a short run reaches the payout maturity
/// skip.
#[cfg(not(feature = "sim"))]
pub const COINBASE_MATURITY: usize = 6048;
#[cfg(feature = "sim")]
pub const COINBASE_MATURITY: usize = 20;

/// Bitcoin Cash coinbase maturity, in shares: 100, as Satoshi set coinbase
/// maturity in blocks and BCH keeps it, a count rather than a span of time.
///
/// Like `COINBASE_MATURITY` it is one rule for both jobs: a share coinbase
/// output cannot be spent until it is this deep, and a share is paid by a
/// found block only at this depth (the first 100 shares of the chain are
/// always mature for payout). A block's payout can then differ between
/// share-chain forks only if they split more than 100 shares below the paying
/// share, far deeper than an uncle (`MAX_UNCLES_DEPTH`).
///
/// The same in the sim build: the Bitcoin Cash swarm exercises the full
/// maturity skip, and the Bitcoin Cash protocol strings carry this value.
pub const BCH_COINBASE_MATURITY: usize = 100;

/// The coinbase maturity a chain's consensus uses.
pub fn coinbase_maturity_for(chain: Chain) -> usize {
    match chain {
        Chain::Bitcoin => COINBASE_MATURITY,
        Chain::BitcoinCash => BCH_COINBASE_MATURITY,
    }
}

/// Bitcoin: shares from a paying share down to the oldest share it can pay,
/// the coinbase maturity depth plus the PPLNS window: 127,008 (upstream's).
///
/// Sizes Bitcoin's full-validation zone, the expiry of its share chain
/// outputs, its reorg depth limit and, doubled, `PRUNE_DEPTH`. Bitcoin Cash
/// sizes the same from its own 100-share maturity (`BCH_PAYOUT_DEPTH`); read
/// a chain's depths with [`ChainDepths::for_chain`].
pub const PAYOUT_DEPTH: usize = COINBASE_MATURITY + PPLNS_WINDOW_SHARES;

/// Bitcoin Cash: shares from a paying share down to the deepest share its
/// coinbase can pay: the 100-share maturity skip (`BCH_COINBASE_MATURITY`),
/// the window capped at `PPLNS_WINDOW_SHARES`, and an uncle of the deepest
/// window share, at most `MAX_UNCLES_DEPTH` below it. 121,063 shares, about
/// 14 days at 10 s, against Bitcoin's 127,008 (audit A12).
///
/// A share-chain rule, since it sets when share chain outputs expire: an
/// output whose coinbase root is more than this deep below the spending
/// share can no longer be spent. It also sizes the full-validation zone,
/// so every share a coinbase can pay is fully validated, and the reorg depth
/// limit (`MAX_REORG_DEPTH`), and doubled it is `BCH_PRUNE_DEPTH`.
pub const BCH_PAYOUT_DEPTH: usize =
    BCH_COINBASE_MATURITY + PPLNS_WINDOW_SHARES + MAX_UNCLES_DEPTH as usize;

/// Name of the Bitcoin Cash depth rule in protocol strings: output expiry,
/// the full-validation zone and the reorg limit at `BCH_PAYOUT_DEPTH`, from
/// the 100-share maturity. A node on Bitcoin's 127,008 accepts spends of
/// outputs this node has expired.
pub fn bch_depth_rule_tag() -> String {
    format!("depths-m{BCH_COINBASE_MATURITY}")
}

/// The depths of one chain's share-chain rules and storage, each derived
/// from its coinbase maturity and the PPLNS window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChainDepths {
    /// Depth a share coinbase output must reach before it is spent, and a
    /// share before a found block pays it.
    pub coinbase_maturity: usize,
    /// Depth of the deepest share a coinbase can pay; see `PAYOUT_DEPTH` and
    /// `BCH_PAYOUT_DEPTH`.
    pub payout_depth: usize,
}

impl ChainDepths {
    /// Bitcoin's depths: upstream's, from its 6,048-share maturity.
    pub const BITCOIN: Self = Self {
        coinbase_maturity: COINBASE_MATURITY,
        payout_depth: PAYOUT_DEPTH,
    };

    /// Bitcoin Cash's depths, from its 100-share maturity.
    pub const BITCOIN_CASH: Self = Self {
        coinbase_maturity: BCH_COINBASE_MATURITY,
        payout_depth: BCH_PAYOUT_DEPTH,
    };

    /// The depths a chain's rules use.
    pub const fn for_chain(chain: Chain) -> Self {
        match chain {
            Chain::Bitcoin => Self::BITCOIN,
            Chain::BitcoinCash => Self::BITCOIN_CASH,
        }
    }

    /// Blocks from the tip whose bodies a node keeps: two payout depths, one
    /// fully validated and one below it holding the outputs a spend at the
    /// bottom of the first may still use.
    pub const fn prune_depth(self) -> usize {
        2 * self.payout_depth
    }

    /// Deepest fork below the confirmed tip the candidate chain reorgs onto:
    /// one payout depth (see `MAX_REORG_DEPTH` in `store::prune_blocks`).
    pub const fn max_reorg_depth(self) -> usize {
        self.payout_depth
    }

    /// True when the share at `block_height` is within a payout depth of the
    /// tip at `tip_height`: the zone where shares are fully validated.
    pub fn is_within_payout_depth(self, block_height: u32, tip_height: u32) -> bool {
        block_height > tip_height.saturating_sub(self.payout_depth as u32)
    }
}

// Each chain's depths: the payout walk, its uncles and its spendable outputs
// fit the full-validation zone, and the deepest followed fork with its uncles
// stays above the prune target, where deletion stops.
const _: () = {
    let depths = [ChainDepths::BITCOIN, ChainDepths::BITCOIN_CASH];
    let mut index = 0;
    while index < depths.len() {
        let chain = depths[index];
        assert!(chain.coinbase_maturity + PPLNS_WINDOW_SHARES <= chain.payout_depth);
        assert!(
            chain.max_reorg_depth() + MAX_UNCLES_DEPTH as usize
                <= chain.prune_depth() + PRUNE_INTERVAL
        );
        index += 1;
    }
    // Bitcoin Cash's own: the walk, an uncle below it, exactly.
    assert!(
        BCH_COINBASE_MATURITY + PPLNS_WINDOW_SHARES + MAX_UNCLES_DEPTH as usize == BCH_PAYOUT_DEPTH
    );
};

/// Divisor for the retained eviction buffer beyond the window: the cache
/// keeps the window plus `window / PPLNS_WINDOW_BUFFER_DIVISOR` extra shares
/// (1%, rounded up) so the distribution for an anchor slightly behind the
/// tip -- e.g. a sibling's parent after a competing sibling advanced the tip
/// -- is not truncated by eviction.
///
/// This buffer is what bounds how deep a fork this node can still pay out for,
/// and so which forks it can follow at all. A payout walk starting at an anchor
/// `n` entries below the confirmed tip has `cache_capacity() - n` entries left
/// beneath it, so it can only span the maturity skip and a full
/// `PPLNS_WINDOW_SHARES` while `n` is within the buffer. Past that the walk is truncated by eviction, the
/// distribution would depend on where this node's cache happens to end rather
/// than on the chain, and `get_distribution_from_start_hash` refuses it --
/// which drops the share (`FailureKind::Unresolvable`).
///
/// At the current 1% that is 1,210 entries, about three and a half hours at six
/// shares per minute. Raising it widens the forks this node will follow and
/// costs roughly 104 bytes per entry; lowering it narrows them. It is a local
/// resource choice, not a consensus rule: nodes with different values disagree
/// only about which deep forks they are willing to follow, and a dropped share
/// leaves no verdict behind.
///
/// The sim build uses 4, a 20 entry buffer over its 80 share window: 1% of
/// that window would be one entry, and shares built one parent behind the tip
/// under propagation delay would be dropped.
#[cfg(not(feature = "sim"))]
const PPLNS_WINDOW_BUFFER_DIVISOR: usize = 100;
#[cfg(feature = "sim")]
const PPLNS_WINDOW_BUFFER_DIVISOR: usize = 4;

/// Bitcoin: blocks from the chain tip whose bodies each node retains.
/// Equals 2x `PAYOUT_DEPTH`: one payout depth for full tx validation and one
/// for PoW-only validation that provides output availability.
pub const PRUNE_DEPTH: usize = ChainDepths::BITCOIN.prune_depth();

/// Bitcoin Cash: blocks from the chain tip whose bodies each node retains,
/// 2x `BCH_PAYOUT_DEPTH`: 242,126 shares, about 28 days at 10 s, against
/// Bitcoin's 254,016.
pub const BCH_PRUNE_DEPTH: usize = ChainDepths::BITCOIN_CASH.prune_depth();

/// The gap between the prune height and the prune target, and the size of
/// one prune chunk: 360 shares, about 1 hour at 10s/block (60 * 6 = 360).
///
/// The sim build uses 20 shares, giving a sim prune target 220 shares below
/// the tip.
#[cfg(not(feature = "sim"))]
pub const PRUNE_INTERVAL: usize = 360;
#[cfg(feature = "sim")]
pub const PRUNE_INTERVAL: usize = 20;

/// Scale factor applied to all difficulty contributions.
/// Allows integer representation of 90%/10% uncle/nephew weighting.
pub const DIFFICULTY_SCALE: u128 = 10;

/// Scaled uncle weight: uncle contributes 9/10 of its difficulty.
pub(crate) const UNCLE_SCALED_WEIGHT: u128 = 9;

/// Scaled nephew bonus: nephew receives 1/10 of each uncle's difficulty.
pub(crate) const NEPHEW_SCALED_BONUS: u128 = 1;

/// Maximum depth to search for the fork point before falling back to
/// full invalidation. Limits the cost of `find_fork_height` to at most
/// this many RocksDB point lookups. A malicious miner forking from deep
/// in the window would otherwise force O(window_size) lookups.
const MAX_REORG_SCAN_DEPTH: usize = 100;

/// Initial capacity for the confirmed entries vector.
/// Based on typical PPLNS window of ~6 hours (6 shares/min * 60 * 6).
const INITIAL_ENTRIES_CAPACITY: usize = 2160;

/// A cached confirmed share entry with only the fields needed for payout.
struct ConfirmedEntry {
    /// Donation and fee committed by this share, before its right is sold.
    cuts: ShareCuts,
    blockhash: BlockHash,
    /// Confirmed chain height of this entry.
    height: u32,
    /// Internal key mapping the miner address in AddressKeys.
    internal_key: usize,
    /// Internal key mapping the share owner (the share chain address) in
    /// `owner_keys`. The payout never reads it; it lets a window be looked
    /// up by either of the two addresses a share header carries.
    owner_key: usize,
    /// Base weight of the share (`WeightUnit`): its difficulty on Bitcoin,
    /// its exact work on Bitcoin Cash.
    difficulty: u128,
    /// Uncle entries, if any, referenced by the share
    uncle_entries: Vec<UncleEntry>,
    /// Txid of the share's coinbase, which carries its payout right. Set on
    /// Bitcoin Cash, where the payout rights index needs it to evict the
    /// share; all zeros on Bitcoin.
    coinbase_txid: Txid,
    /// The block the share's bitcoin header builds on, which places its work
    /// on a Bitcoin Cash chain (`bch_eligibility`).
    bch_prev: BlockHash,
}

impl ConfirmedEntry {
    /// Work this entry adds to a window sized by block difficulty: its own
    /// difficulty plus its uncles'.
    ///
    /// An uncle's hashes could have found the block as well as the nephew's,
    /// so both count towards the pool's expected blocks, even though an uncle
    /// takes no window slot. This bounds the walk only; the weights come from
    /// `for_each_credit`.
    fn work(&self) -> u128 {
        self.uncle_entries
            .iter()
            .fold(self.difficulty, |work, uncle_entry| {
                work.saturating_add(uncle_entry.difficulty)
            })
    }
}

/// Which rule sizes the window below the maturity skip.
///
/// Chosen once from the chain the share chain pays on, so the producer, the
/// validator and the API, which share one window, always use the same rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowSizing {
    /// `max_window_shares` shares. Bitcoin, unchanged from upstream.
    FixedShares,
    /// Shares until their work reaches `expected_blocks` times the paying
    /// block's difficulty, at most `max_window_shares` shares. Bitcoin Cash.
    BlockWork { expected_blocks: u128 },
}

/// Protocol tag segment for weighing Bitcoin Cash shares by exact work
/// (`WeightUnit::ExactWork`). A node weighing by rounded-down difficulty
/// builds another coinbase whenever the window holds shares of different
/// difficulties.
pub const BCH_WORK_WEIGHT_RULE_TAG: &str = "weight-work";

/// Protocol tag segment for per-share donation and fee on Bitcoin Cash
/// (`ShareCuts`): each window share's own cuts come off its own weight, the
/// donation key and fee addresses are payees like any other, and the coinbase
/// has no separate donation or fee output. A node taking the paying share's
/// cuts off the whole coinbase builds another coinbase for every share.
pub const BCH_PER_SHARE_CUTS_RULE_TAG: &str = "cuts-per-share";

/// What a share's base weight, before `DIFFICULTY_SCALE` and the uncle
/// split, counts.
///
/// Chosen once from the chain, like `WindowSizing`, so the producer, the
/// validator and the API weigh shares alike.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WeightUnit {
    /// `max_target / target`, rounded down. Bitcoin, unchanged from
    /// upstream.
    IntegerDifficulty,
    /// The share's work, `2^256 / (target + 1)`, as chainwork counts it.
    /// Bitcoin Cash: share difficulties there are small (the mini chain's
    /// easiest is 4.55), so rounding them down would weigh shares of
    /// difficulty 4 to 9 up to about 20% wrong against each other.
    ExactWork,
}

/// `work` as a `u128`, saturating. Share and block work stays far below
/// 2^128 for any target above 2^128.
fn work_to_u128(work: Work) -> u128 {
    let bytes = work.to_le_bytes();
    let (low, high) = bytes.split_at(16);
    if high.iter().any(|byte| *byte != 0) {
        return u128::MAX;
    }
    u128::from_le_bytes(low.try_into().expect("16 bytes"))
}

/// The work of one unit of difficulty on `network`: the work at its
/// proof-of-work limit, `2^256 / (max_target + 1)`. About `2^32` on mainnet,
/// testnet and chipnet, 2 on regtest.
fn difficulty_one_work(network: bitcoin::Network) -> u128 {
    work_to_u128(Params::new(network).max_attainable_target.to_work())
}

/// Why accumulating the window stopped.
///
/// `WindowFull` is a property of the chain, so every node reaches it at the
/// same point. `OutOfEntries` is not: it means the cache ran out, and where
/// the cache ends depends on this node's own confirmed tip.
#[derive(Debug, PartialEq, Eq)]
enum WindowStopReason {
    /// The window reached its bound after the immature skip:
    /// `max_window_shares` shares, or on Bitcoin Cash the work target.
    WindowFull,
    /// The walk consumed every entry available to it.
    OutOfEntries,
}

/// A cached uncle entry with only the fields needed for payout.
struct UncleEntry {
    /// Donation and fee committed by the uncle itself.
    cuts: ShareCuts,
    /// Blockhash of the uncle share, so a window view can name it.
    blockhash: BlockHash,
    /// Internal key mapping the miner address in AddressKeys.
    internal_key: usize,
    /// Internal key mapping the uncle's share owner in `owner_keys`.
    owner_key: usize,
    /// Base weight of the uncle share before weighting (`WeightUnit`).
    difficulty: u128,
    /// The block the uncle's bitcoin header builds on.
    bch_prev: BlockHash,
}

/// Candidate headers collected while walking backward from the anchor to a
/// confirmed ancestor (newest-to-oldest), paired with that ancestor's index
/// in `confirmed_entries`.
type CandidateWalk = (Vec<(BlockHash, ShareHeader)>, usize);

/// The entries a window walk from one anchor runs over, and where in them
/// the window starts.
struct ResolvedWindow {
    /// Candidate entries from the anchor down to the confirmed entry point,
    /// newest-to-oldest. Empty when the anchor itself is confirmed.
    candidate_entries: Vec<ConfirmedEntry>,
    /// Index in `confirmed_entries` the walk continues from.
    confirmed_start_index: usize,
    /// Height of the paying share, the share whose parent is the anchor.
    paying_height: usize,
    /// Shares, counted down from the anchor, not yet mature for payout.
    immature_share_count: usize,
    /// Work the window must hold, or `None` when only the share count bounds
    /// it (see `PplnsWindow::work_target`).
    work_target: Option<u128>,
    /// Payout address key of every share whose payout right is held, as of
    /// the anchor, by someone other than its own miner. Empty on Bitcoin.
    transferred_payees: HashMap<BlockHash, usize>,
    /// Bitcoin Cash with header rules: the work this window pays. `None`
    /// pays every share.
    paid_bch_prevs: Option<PaidBchPrevs>,
}

/// The BCH prevs of the window shares and uncles a paying block pays: those
/// on its chain, give or take one block (`bch_eligibility`).
struct PaidBchPrevs {
    /// The paying block's own BCH prev.
    paying_prev: BlockHash,
    paid: HashSet<BlockHash>,
}

impl ResolvedWindow {
    /// Height of the share at `walk_index` along the walk, where index 0 is
    /// the anchor. Heights are contiguous along parent links, which also
    /// gives the candidate entries, cached with no height, their real one.
    fn height_at(&self, walk_index: usize) -> u32 {
        self.paying_height.saturating_sub(walk_index + 1) as u32
    }

    /// Payout address key for a window share's own weight: the holder of its
    /// payout right when that is not its own miner, else its own
    /// `miner_bitcoin_address`.
    fn share_payee(&self, entry: &ConfirmedEntry) -> usize {
        if self.transferred_payees.is_empty() {
            return entry.internal_key;
        }
        self.transferred_payees
            .get(&entry.blockhash)
            .copied()
            .unwrap_or(entry.internal_key)
    }

    /// Whether this window pays work building on `bch_prev`. Every share is
    /// paid without the Bitcoin Cash chain rule, and so is the chain's first
    /// share, the only one building on the all-zero hash.
    fn pays(&self, bch_prev: &BlockHash) -> bool {
        self.paid_bch_prevs.as_ref().is_none_or(|paid_bch_prevs| {
            *bch_prev == BlockHash::all_zeros() || paid_bch_prevs.paid.contains(bch_prev)
        })
    }

    /// Refuse a window that walked `walked` shares and paid them nothing:
    /// on Bitcoin Cash, a paying block on a chain none of the window's work
    /// is on.
    fn require_paid(
        &self,
        anchor: BlockHash,
        walked: usize,
        paid_weight: u128,
    ) -> Result<(), WindowError> {
        match &self.paid_bch_prevs {
            Some(paid_bch_prevs) if walked > 0 && paid_weight == 0 => {
                Err(WindowError::NothingEligible {
                    anchor,
                    bch_prev: paid_bch_prevs.paying_prev,
                })
            }
            _ => Ok(()),
        }
    }
}

/// One weight a window share contributes, before it is summed per address.
struct Credit {
    /// Cuts of the share or uncle that earned this weight.
    cuts: ShareCuts,
    /// Payout address key the weight is paid to.
    internal_key: usize,
    /// Share owner key of the share or uncle that earned it.
    owner_key: usize,
    /// The share or uncle that earned it.
    blockhash: BlockHash,
    /// The share that includes this uncle, or `None` for the share itself.
    nephew: Option<BlockHash>,
    /// Base weight of the share or uncle before weighting: its difficulty on
    /// Bitcoin, its exact work on Bitcoin Cash.
    difficulty: u128,
    /// Scaled weight credited.
    weight: u128,
}

/// The donation and fee one Bitcoin Cash share takes from its own weight, in
/// basis points, each with its recipient's key in `address_keys`, so the
/// recipients are summed and split like any payee. `None` for no cut.
///
/// Of a credit of weight `w` from a share with donation `d` and fee `f`
/// (fractions of one): `d * w` goes to the donation key, `f * (1 - d) * w`
/// to the share's fee address (the fee comes after the donation, as P2Pool
/// v1 had it), and the rest to the credit's payee. A share-chain rule
/// (`BCH_PER_SHARE_CUTS_RULE_TAG`).
#[derive(Clone, Copy, Default)]
struct ShareCuts {
    donation: Option<(usize, u16)>,
    fee: Option<(usize, u16)>,
}

impl ShareCuts {
    const BASIS_POINTS: u128 = 10_000;
    /// The denominator of an exact split: basis points of basis points.
    const SCALE: u128 = Self::BASIS_POINTS * Self::BASIS_POINTS;

    fn is_empty(self) -> bool {
        self.donation.is_none() && self.fee.is_none()
    }

    /// Add `credit` to `weights` split by these cuts, each part a whole
    /// number of `1 / SCALE` weights, so nothing is rounded before the
    /// coinbase split: the donation key gets `w * d * 10^4`, the fee address
    /// `w * f * (10^4 - d)` and the payee `w * (10^4 - d) * (10^4 - f)`,
    /// which sum to `w * SCALE`.
    fn credit(self, credit: &Credit, weights: &mut [u128]) -> Result<(), &'static str> {
        let donation = self.donation.map_or(0, |(_, cut)| u128::from(cut));
        let fee = self.fee.map_or(0, |(_, cut)| u128::from(cut));
        let after_donation = Self::BASIS_POINTS
            .checked_sub(donation)
            .ok_or("Share donation exceeds one hundred percent")?;
        let after_fee = Self::BASIS_POINTS
            .checked_sub(fee)
            .ok_or("Share fee exceeds one hundred percent")?;
        let mut add = |key: usize, factor: u128| {
            credit
                .weight
                .checked_mul(factor)
                .and_then(|part| weights[key].checked_add(part))
                .map(|sum| weights[key] = sum)
                .ok_or("Per-share payout weights overflow")
        };
        if let Some((key, _)) = self.donation {
            add(key, donation * Self::BASIS_POINTS)?;
        }
        if let Some((key, _)) = self.fee {
            add(key, fee * after_donation)?;
        }
        add(credit.internal_key, after_donation * after_fee)
    }

    fn retain_keys(self, referenced: &mut [bool]) {
        if let Some((key, _)) = self.donation {
            referenced[key] = true;
        }
        if let Some((key, _)) = self.fee {
            referenced[key] = true;
        }
    }
}

/// One weight a miner earns in a payout window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowCredit {
    /// The share that earned it: a window share, or an uncle one includes.
    pub blockhash: BlockHash,
    /// Height of the window share carrying the weight. An uncle is credited
    /// at the height of the share that includes it.
    pub height: u32,
    /// The window share that includes this uncle, or `None` when
    /// `blockhash` is itself a window share.
    pub nephew: Option<BlockHash>,
    /// Base weight of the share or uncle before weighting: its difficulty on
    /// Bitcoin, its exact work on Bitcoin Cash.
    pub difficulty: u128,
    /// Weight credited, in the scaled units the distribution sums: a share
    /// earns `DIFFICULTY_SCALE` times its difficulty plus the nephew bonus
    /// for each uncle it includes, an uncle `UNCLE_SCALED_WEIGHT` times its
    /// difficulty.
    pub weight: u128,
    /// Payout address the weight is paid to: the share's or the uncle's own
    /// `miner_bitcoin_address`, or on Bitcoin Cash the P2PKH address of
    /// whoever else holds the share's payout right.
    pub payout_address: PayoutAddress,
}

/// What one miner earns in a payout window.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MinerCredits {
    /// Window shares the miner mined.
    pub share_count: usize,
    /// Uncles the miner mined that window shares include.
    pub uncle_count: usize,
    /// Sum of the miner's weights.
    pub weight: u128,
    /// The miner's weights, newest first, at most the requested number.
    pub credits: Vec<WindowCredit>,
}

impl MinerCredits {
    /// Count one credit, and list it while fewer than `max_credits` are listed.
    fn add(
        &mut self,
        credit: &Credit,
        height: u32,
        payout_address: Option<&PayoutAddress>,
        max_credits: usize,
    ) {
        if credit.nephew.is_some() {
            self.uncle_count += 1;
        } else {
            self.share_count += 1;
        }
        self.weight = self.weight.saturating_add(credit.weight);
        if self.credits.len() < max_credits
            && let Some(payout_address) = payout_address
        {
            self.credits.push(WindowCredit {
                blockhash: credit.blockhash,
                height,
                nephew: credit.nephew,
                difficulty: credit.difficulty,
                weight: credit.weight,
                payout_address: payout_address.clone(),
            });
        }
    }
}

/// Every miner's credits in one payout window, and the totals of that window.
///
/// Built by the same walk and weighting as
/// `get_distribution_from_start_hash`, so each payout address's `weight` is
/// its entry in that distribution, and `window_weight` is the distribution's
/// total. Miners are listed by either of the two addresses a share header
/// carries, neither derived from the other: a share owner's credits are paid
/// to the payout address each credit names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowCredits {
    /// Height of the anchor the window was resolved from.
    pub anchor_height: u32,
    /// Height of the newest share in the window, the first one past the
    /// immature skip.
    pub newest_height: Option<u32>,
    /// Height of the oldest share in the window.
    pub oldest_height: Option<u32>,
    /// Number of shares in the window.
    pub window_share_count: usize,
    /// Sum of every weight in the window: what each payout is a share of.
    pub window_weight: u128,
    /// Credits of each payout address (`miner_bitcoin_address`) the window
    /// credits.
    pub by_payout_address: HashMap<PayoutAddress, MinerCredits>,
    /// Credits of each share owner (`miner_address`, the share chain
    /// address) the window credits.
    pub by_share_owner: HashMap<ShareOwner, MinerCredits>,
}

/// Why a payout distribution could not be produced.
///
/// The cases are distinguished by the response validation needs:
/// `InsufficientEntries` and `RightsHistoryTooShallow` are permanent for this
/// node and drop the share without a verdict, `NothingEligible` is the
/// share's own fault and makes it invalid, and everything else may still
/// resolve and leaves it for a retry.
#[derive(Debug)]
pub enum WindowError {
    /// The walk ran out of cached entries before reaching either bound, and
    /// the cache no longer reaches the chain start. The anchor sits deeper
    /// than the retained buffer (see `PPLNS_WINDOW_BUFFER_DIVISOR`), and since
    /// the cache's oldest entry only moves forward with the confirmed tip, no
    /// retry can bring it back into range.
    InsufficientEntries {
        anchor: BlockHash,
        oldest_cached_height: u32,
        max_window_shares: usize,
    },
    /// Bitcoin Cash: the anchor forks off the cached chain deeper than the
    /// payout rights index can undo, so who holds each window share's right
    /// at the anchor is unknown. Like `InsufficientEntries`, permanent for
    /// this node: the undo log only moves forward with the confirmed tip.
    RightsHistoryTooShallow {
        anchor: BlockHash,
        fork_depth: usize,
        undo_depth: usize,
    },
    /// Bitcoin Cash: the block the paying block, or a window share or
    /// uncle, builds on is not known yet. It may become known once its header
    /// arrives.
    UnknownBchPrev { block: BlockHash },
    /// Bitcoin Cash: no share in the window is on the paying block's BCH
    /// chain or one block off it, so the block would pay no one. A property
    /// of the chain: the paying block builds on a branch none of the window's
    /// work is on.
    NothingEligible {
        anchor: BlockHash,
        bch_prev: BlockHash,
    },
    /// A store read failed, or the anchor's ancestry is not stored. Either may
    /// resolve later.
    ReadFailure(Box<dyn Error + Send + Sync>),
}

impl fmt::Display for WindowError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WindowError::InsufficientEntries {
                anchor,
                oldest_cached_height,
                max_window_shares,
            } => write!(
                formatter,
                "PPLNS window for anchor {anchor} is truncated by eviction: the walk ran out of \
                 cached entries at height {oldest_cached_height}, short of its window of at most \
                 {max_window_shares} shares"
            ),
            WindowError::RightsHistoryTooShallow {
                anchor,
                fork_depth,
                undo_depth,
            } => write!(
                formatter,
                "Payout rights for anchor {anchor} cannot be resolved: it forks {fork_depth} \
                 shares below the cached tip, and the rights index undoes at most {undo_depth}"
            ),
            WindowError::UnknownBchPrev { block } => {
                write!(formatter, "Bitcoin Cash block {block} is not known yet")
            }
            WindowError::NothingEligible { anchor, bch_prev } => write!(
                formatter,
                "PPLNS window for anchor {anchor} pays no share: none is on the chain of \
                 Bitcoin Cash block {bch_prev} or one block off it"
            ),
            WindowError::ReadFailure(error) => write!(formatter, "{error}"),
        }
    }
}

impl Error for WindowError {}

impl From<Box<dyn Error + Send + Sync>> for WindowError {
    fn from(error: Box<dyn Error + Send + Sync>) -> Self {
        WindowError::ReadFailure(error)
    }
}

/// Incremental PPLNS window cache.
///
/// Caches confirmed share headers and uncle data from the share chain,
/// allowing incremental loading of only newly confirmed headers on each
/// update rather than re-reading the full window from RocksDB.
///
/// Caches `COINBASE_MATURITY + PPLNS_WINDOW_SHARES` confirmed entries, plus
/// the eviction buffer, and their uncles, no matter how far back in time we
/// need to go to get to those many entries. This simplifies eviction and the
/// cache maintenance logic.
pub struct PplnsWindow {
    /// Confirmed share entries ordered newest-to-oldest, capped at the window
    /// plus its eviction buffer (`cache_capacity`).
    confirmed_entries: VecDeque<ConfirmedEntry>,
    /// The blockhash of the chain tip when this cache was last updated.
    cached_tip_blockhash: Option<BlockHash>,
    /// The height of the highest confirmed share in the cache.
    cached_top_height: Option<u32>,
    /// Address internal key mapping
    address_keys: AddressKeys,
    /// Share owner internal key mapping, for lookups by share chain address.
    owner_keys: AddressKeys<ShareOwner>,
    /// Bitcoin network used for computing integer difficulty from Target.
    pub(crate) network: bitcoin::Network,
    /// Number of shares in the payout window. Defaults to
    /// `PPLNS_WINDOW_SHARES`; overridable in tests so eviction can be
    /// exercised without a full-capacity fixture.
    max_window_shares: usize,
    /// Depth a share must reach before it is paid: `coinbase_maturity_for`
    /// the chain `for_chain` selects, `COINBASE_MATURITY` until then.
    maturity_skip: usize,
    /// Rule sizing the window below the maturity skip. `FixedShares` until
    /// `for_chain` selects Bitcoin Cash.
    sizing: WindowSizing,
    /// What a share's weight counts: integer difficulty on Bitcoin, exact
    /// work on Bitcoin Cash. Set by `for_chain`.
    weight_unit: WeightUnit,
    /// Bitcoin Cash: who holds each cached share's payout right, at the
    /// cached tip. `None` on Bitcoin, which pays `miner_bitcoin_address`.
    rights: Option<PayoutRights>,
    /// Bitcoin Cash: each window share's own donation and fee come off its
    /// own weight (`ShareCuts`). Bitcoin takes the paying share's cuts off
    /// the whole coinbase instead. Set by `for_chain`.
    per_share_cuts: bool,
    /// Bitcoin Cash: the header rules that size a window from the paying
    /// block's BCH prev and pay only the work on its chain
    /// (`with_bch_header_rules`). `None` sizes by the claimed difficulty and
    /// pays every share.
    bch_rules: Option<BchHeaderRules>,
    /// Places of the BCH blocks window shares and paying chains build on,
    /// read from the store once. A header never changes, so a place never
    /// goes stale; `prune_unreferenced_keys` drops the ones nothing cached
    /// names.
    bch_places: HashMap<BlockHash, BchPlace>,
    /// The paying chain walked last, for the next window paid by a block on
    /// the same BCH prev.
    paying_chain: Option<PayingChain>,
}

impl PplnsWindow {
    /// Create an empty PplnsWindow with preallocated capacity.
    ///
    /// The window is the fixed `PPLNS_WINDOW_SHARES` shares, as on Bitcoin.
    /// A Bitcoin Cash node must call `for_chain`.
    pub fn new(network: bitcoin::Network) -> Self {
        Self {
            confirmed_entries: VecDeque::with_capacity(INITIAL_ENTRIES_CAPACITY),
            cached_tip_blockhash: None,
            cached_top_height: None,
            address_keys: AddressKeys::default(),
            owner_keys: AddressKeys::default(),
            network,
            max_window_shares: PPLNS_WINDOW_SHARES,
            maturity_skip: COINBASE_MATURITY,
            sizing: WindowSizing::FixedShares,
            weight_unit: WeightUnit::IntegerDifficulty,
            rights: None,
            per_share_cuts: false,
            bch_rules: None,
            bch_places: HashMap::new(),
            paying_chain: None,
        }
    }

    /// Select the window rule for the chain this share chain pays on.
    ///
    /// Bitcoin keeps the fixed `PPLNS_WINDOW_SHARES` shares behind
    /// `COINBASE_MATURITY`, paid to each share's `miner_bitcoin_address`.
    /// Bitcoin Cash pays `BCH_PPLNS_WINDOW_BLOCKS` expected blocks' worth of
    /// work, capped at `PPLNS_WINDOW_SHARES` shares, behind
    /// `BCH_COINBASE_MATURITY`, to whoever holds each share's payout right
    /// (see `payout_rights`), each share weighted by its exact work rather
    /// than its rounded-down difficulty (`WeightUnit::ExactWork`). This is a
    /// share-chain rule: every node on a share chain must select the same
    /// chain.
    pub fn for_chain(mut self, chain: Chain) -> Self {
        self.sizing = match chain {
            Chain::Bitcoin => WindowSizing::FixedShares,
            Chain::BitcoinCash => WindowSizing::BlockWork {
                expected_blocks: BCH_PPLNS_WINDOW_BLOCKS,
            },
        };
        self.weight_unit = match chain {
            Chain::Bitcoin => WeightUnit::IntegerDifficulty,
            Chain::BitcoinCash => WeightUnit::ExactWork,
        };
        self.maturity_skip = coinbase_maturity_for(chain);
        self.rights = match chain {
            Chain::Bitcoin => None,
            Chain::BitcoinCash => Some(PayoutRights::default()),
        };
        self.per_share_cuts = chain == Chain::BitcoinCash;
        self
    }

    /// Mining on the current chain, after `for_chain`: size each window from
    /// the nBits Bitcoin Cash requires after the paying block's BCH prev
    /// (`BchHeaderRules::sizing_bits`), and pay only the shares on that
    /// prev's chain or one block off it (`bch_eligibility`). A share-chain
    /// rule (`BCH_CURRENT_CHAIN_RULE_TAG`): every node of a Bitcoin Cash
    /// share chain must set its network's rules.
    pub fn with_bch_header_rules(mut self, rules: Option<BchHeaderRules>) -> Self {
        self.bch_rules = rules;
        self
    }

    /// Whether each window share's own donation and fee come off its own
    /// weight (Bitcoin Cash), rather than the paying share's off the whole
    /// coinbase (Bitcoin).
    pub(crate) fn has_per_share_cuts(&self) -> bool {
        self.per_share_cuts
    }

    /// The donation and fee `header` takes from its own weight, with their
    /// recipients interned like payees. A zero or recipient-less cut is none.
    /// Always none on Bitcoin.
    fn share_cuts(&mut self, header: &ShareHeader) -> ShareCuts {
        if !self.per_share_cuts {
            return ShareCuts::default();
        }
        let mut cut = |address: &Option<PayoutAddress>, basis_points: Option<u16>| {
            address
                .as_ref()
                .zip(basis_points)
                .filter(|(_, basis_points)| *basis_points > 0)
                .map(|(address, basis_points)| {
                    (self.address_keys.key_for(address.clone()), basis_points)
                })
        };
        ShareCuts {
            donation: cut(&header.donation_address, header.donation),
            fee: cut(&header.fee_address, header.fee),
        }
    }

    /// Override the maturity skip, after `for_chain`, for the offline payout
    /// comparison (`pplns_compare` in `p2poolv2_sim`), which measures other
    /// skips than the chain's own. Consensus code never calls it.
    #[cfg(any(test, feature = "test-utils"))]
    pub fn with_maturity_skip(mut self, maturity_skip: usize) -> Self {
        self.maturity_skip = maturity_skip;
        self
    }

    /// Test-only constructor with an injectable maturity depth and window
    /// size, so the eviction path can be driven without building a
    /// `PAYOUT_DEPTH`-sized fixture.
    #[cfg(test)]
    pub(super) fn new_with_window_sizes(
        network: bitcoin::Network,
        maturity_skip: usize,
        max_window_shares: usize,
    ) -> Self {
        Self::new(network).with_window_sizes(maturity_skip, max_window_shares)
    }

    /// Test-only override of the maturity depth and window size, applied
    /// after `for_chain`, which sets the chain's own maturity.
    #[cfg(test)]
    pub(super) fn with_window_sizes(
        mut self,
        maturity_skip: usize,
        max_window_shares: usize,
    ) -> Self {
        self.maturity_skip = maturity_skip;
        self.max_window_shares = max_window_shares;
        self
    }
}

impl PplnsWindow {
    /// Check whether the cache has any confirmed entries.
    pub fn is_empty(&self) -> bool {
        self.confirmed_entries.is_empty()
    }

    /// Return the bitcoin network this window was created for.
    pub fn network(&self) -> bitcoin::Network {
        self.network
    }

    /// Read-only payout distribution starting from a given blockhash.
    ///
    /// Walks backward from start_hash through parent pointers until
    /// finding a confirmed entry in the window. Candidate entries
    /// along the walk come first, then confirmed entries from that
    /// entry point onward.
    ///
    /// When start_hash is already in the confirmed entries, no store
    /// reads are needed and the walk produces zero candidate entries.
    ///
    /// The walk first skips the shares that are not yet mature for the paying
    /// share -- the share whose parent is `start_hash` -- and then pays the
    /// next shares, each weighted by its difficulty. A share at height `h` is
    /// mature for a paying share at height `p` when `p - h >= maturity_skip`,
    /// the same depth spending a coinbase needs, or when `h < maturity_skip`:
    /// the first `maturity_skip` shares are always mature for payout, so a
    /// young chain pays its shares instead of nothing (see
    /// `immature_share_count`).
    ///
    /// How many shares are paid depends on the window rule (see
    /// `work_target`). On Bitcoin it is `max_window_shares` and
    /// `paying_block` plays no part. On Bitcoin Cash it is the shares whose
    /// work first reaches `BCH_PPLNS_WINDOW_BLOCKS` times the paying block's
    /// difficulty, at most `max_window_shares`. That difficulty comes from
    /// the paying share's own bitcoin header, never from this node's own BCH
    /// node or config: with header rules, from the nBits Bitcoin Cash
    /// requires after the header's prev; without them, from the nBits the
    /// header claims.
    ///
    /// With header rules a Bitcoin Cash window pays only the work on the
    /// paying block's BCH chain, give or take one block (`bch_eligibility`).
    /// A share off it keeps its slot and its work in the window but earns
    /// nothing, nor does an uncle off it, and a share earns an uncle's nephew
    /// bonus only when both are paid. Errors with `UnknownBchPrev` when the
    /// paying block's prev, or a window share's or uncle's, is not a known
    /// header, and with `NothingEligible` when the window pays no one.
    ///
    /// Candidate shares count towards the skip and the window like confirmed
    /// ones. The skip, the window and the paying header are properties of the
    /// chain, so every node derives the same distribution for the same anchor
    /// and header whatever its own confirmed tip -- which is the point: the
    /// producer and a validator that has since advanced must reconstruct an
    /// identical coinbase.
    ///
    /// Errors when `start_hash` cannot be resolved to a confirmed ancestor
    /// (e.g. an anchor whose ancestry is not stored) or when a store read
    /// fails, so callers fail rather than silently treat an unresolved
    /// anchor or a transient read error as an empty distribution. The
    /// explicit empty/genesis case is handled by callers before this call
    /// (they check `is_empty`).
    ///
    /// Also errors when the walk exhausts the cache before skipping the
    /// immature shares and counting a full window, and the cache no longer
    /// reaches the chain start. The result would
    /// then depend on where eviction has trimmed the back, which is a function
    /// of this node's tip rather than of the chain, so a truncated distribution
    /// is refused rather than returned.
    pub fn get_distribution_from_start_hash(
        &mut self,
        start_hash: BlockHash,
        paying_block: PayingBlock,
        chain_store_handle: &ChainStoreHandle,
    ) -> Result<HashMap<PayoutAddress, u128>, WindowError> {
        let resolved = self.resolve_window(start_hash, paying_block, chain_store_handle)?;
        self.distribution_for_resolved(start_hash, &resolved)
    }

    /// The distribution of a resolved window: walk it and sum each address's
    /// credits.
    ///
    /// On Bitcoin Cash each credit is split by the cuts of the share or
    /// uncle that earned it (`ShareCuts::credit`), in units of
    /// `1 / ShareCuts::SCALE` of a weight so the split is exact. A window in
    /// which no share takes a cut has every weight a whole multiple of
    /// `SCALE`, and is returned in whole weights; the coinbase split is
    /// proportional, so the unit never changes a payout.
    fn distribution_for_resolved(
        &self,
        start_hash: BlockHash,
        resolved: &ResolvedWindow,
    ) -> Result<HashMap<PayoutAddress, u128>, WindowError> {
        let mut weight_by_key = vec![0u128; self.address_keys.len()];
        let mut walked = 0;
        if !self.per_share_cuts {
            self.walk_window(start_hash, resolved, |_, entry| {
                walked += 1;
                Self::add_entry_difficulty(entry, resolved, &mut weight_by_key);
            })?;
            resolved.require_paid(start_hash, walked, total_weight(&weight_by_key))?;
            return self.collect_distribution(&weight_by_key);
        }
        let mut any_cut = false;
        let mut credited = Ok(());
        self.walk_window(start_hash, resolved, |_, entry| {
            walked += 1;
            Self::for_each_credit(entry, resolved, |credit| {
                any_cut |= !credit.cuts.is_empty();
                if credited.is_ok() {
                    credited = credit.cuts.credit(&credit, &mut weight_by_key);
                }
            });
        })?;
        credited.map_err(|error| WindowError::ReadFailure(error.into()))?;
        resolved.require_paid(start_hash, walked, total_weight(&weight_by_key))?;
        if !any_cut {
            for weight in &mut weight_by_key {
                *weight /= ShareCuts::SCALE;
            }
        }
        self.collect_distribution(&weight_by_key)
    }

    /// Read-only view of every miner's credits in the window that
    /// `get_distribution_from_start_hash` pays from `start_hash`.
    ///
    /// Runs the same walk and the same weighting, so a payout address's
    /// `weight` is exactly its entry in that distribution and `window_weight`
    /// is the distribution's total. A share owner, the share chain address,
    /// gets the shares and uncles that name it as owner; their weight is paid
    /// to the payout address each of them names, which its credits report.
    ///
    /// One walk serves every miner, so a caller can build this once per
    /// anchor and answer any number of lookups without taking the window
    /// again. Each miner lists at most `max_credits` credits, newest first,
    /// while its counts and weight cover the whole window.
    ///
    /// `paying_block` sizes the window, and on Bitcoin Cash picks the shares
    /// it pays, as it does for the distribution. A view of a block not yet
    /// found has no paying header, so a caller passes its best estimate, such
    /// as the anchor's own header.
    ///
    /// Errors exactly when `get_distribution_from_start_hash` does.
    pub fn get_window_credits_from_start_hash(
        &mut self,
        start_hash: BlockHash,
        paying_block: PayingBlock,
        chain_store_handle: &ChainStoreHandle,
        max_credits: usize,
    ) -> Result<WindowCredits, WindowError> {
        let resolved = self.resolve_window(start_hash, paying_block, chain_store_handle)?;

        let mut by_payout_key = vec![MinerCredits::default(); self.address_keys.len()];
        let mut by_owner_key = vec![MinerCredits::default(); self.owner_keys.len()];
        let mut window_credits = WindowCredits {
            anchor_height: resolved.height_at(0),
            newest_height: None,
            oldest_height: None,
            window_share_count: 0,
            window_weight: 0,
            by_payout_address: HashMap::new(),
            by_share_owner: HashMap::new(),
        };
        self.walk_window(start_hash, &resolved, |height, entry| {
            window_credits.newest_height.get_or_insert(height);
            window_credits.oldest_height = Some(height);
            window_credits.window_share_count += 1;
            Self::for_each_credit(entry, &resolved, |credit| {
                window_credits.window_weight =
                    window_credits.window_weight.saturating_add(credit.weight);
                let payout_address = self.address_keys.value_for(credit.internal_key);
                by_payout_key[credit.internal_key].add(
                    &credit,
                    height,
                    payout_address,
                    max_credits,
                );
                by_owner_key[credit.owner_key].add(&credit, height, payout_address, max_credits);
            });
        })?;

        resolved.require_paid(
            start_hash,
            window_credits.window_share_count,
            window_credits.window_weight,
        )?;
        window_credits.by_payout_address = credited_by_value(&self.address_keys, by_payout_key);
        window_credits.by_share_owner = credited_by_value(&self.owner_keys, by_owner_key);
        Ok(window_credits)
    }

    /// Resolve the anchor and work out where its window starts and what
    /// bounds it: the walk's entries, the paying share's height, the immature
    /// skip and the work target, and on Bitcoin Cash with header rules the
    /// work it pays.
    fn resolve_window(
        &mut self,
        start_hash: BlockHash,
        paying_block: PayingBlock,
        chain_store_handle: &ChainStoreHandle,
    ) -> Result<ResolvedWindow, WindowError> {
        let (candidate_headers, confirmed_start_index) =
            self.resolve_start_hash(start_hash, chain_store_handle)?;
        let transferred_payees = self.transferred_payees(
            start_hash,
            &candidate_headers,
            confirmed_start_index,
            chain_store_handle,
        )?;
        let candidate_entries =
            self.build_entries_from_headers(&candidate_headers, chain_store_handle)?;
        let paying_prev = self.paying_bch_prev(paying_block, chain_store_handle)?;
        let paying_block_difficulty =
            self.sizing_difficulty(paying_block.claimed_difficulty, paying_prev.as_ref());
        let mut resolved = self.resolved_window(
            candidate_entries,
            confirmed_start_index,
            paying_block_difficulty,
            transferred_payees,
        );
        if let Some(paying_prev) = paying_prev {
            resolved.paid_bch_prevs =
                Some(self.paid_bch_prevs(&resolved, &paying_prev, chain_store_handle)?);
        }
        Ok(resolved)
    }

    /// The known header of the block `paying_block` builds on, when this
    /// window pays by the Bitcoin Cash chain: with header rules and a BCH
    /// prev. The all-zero prev names no block. Only the chain's first share
    /// builds on it, and it pays no window.
    fn paying_bch_prev(
        &self,
        paying_block: PayingBlock,
        chain_store_handle: &ChainStoreHandle,
    ) -> Result<Option<KnownBchHeader>, WindowError> {
        let Some(prev) = paying_block.bch_prev.filter(|_| self.bch_rules.is_some()) else {
            return Ok(None);
        };
        if prev == BlockHash::all_zeros() {
            return Ok(None);
        }
        chain_store_handle
            .get_bch_header(&prev)
            .map_err(|error| {
                WindowError::ReadFailure(
                    format!("Cannot read Bitcoin Cash header {prev}: {error}").into(),
                )
            })?
            .map(Some)
            .ok_or(WindowError::UnknownBchPrev { block: prev })
    }

    /// The difficulty that sizes a window: from the nBits Bitcoin Cash
    /// requires after `paying_prev` when the header rules give one, else the
    /// difficulty the paying block claims.
    fn sizing_difficulty(
        &self,
        claimed_difficulty: u128,
        paying_prev: Option<&KnownBchHeader>,
    ) -> u128 {
        paying_prev
            .zip(self.bch_rules)
            .and_then(|(paying_prev, rules)| rules.sizing_bits(paying_prev))
            .map_or(claimed_difficulty, |bits| {
                bitcoin_block_difficulty(bits, self.network)
            })
    }

    /// The BCH prevs of the shares and uncles in `resolved` that a block on
    /// `paying_prev` pays: those on its chain or one block off it.
    ///
    /// Errors with `UnknownBchPrev` when a share's or uncle's prev is not a
    /// known header. A share is stored only once its prev is known, so this
    /// node has not validated such a window yet.
    fn paid_bch_prevs(
        &mut self,
        resolved: &ResolvedWindow,
        paying_prev: &KnownBchHeader,
        chain_store_handle: &ChainStoreHandle,
    ) -> Result<PaidBchPrevs, WindowError> {
        let mut prevs = HashSet::new();
        self.walk_window_entries(resolved, |_, entry| {
            prevs.insert(entry.bch_prev);
            prevs.extend(entry.uncle_entries.iter().map(|uncle| uncle.bch_prev));
        });
        prevs.remove(&BlockHash::all_zeros());

        let mut placed = Vec::with_capacity(prevs.len());
        for prev in prevs {
            let place = self
                .bch_place(&prev, chain_store_handle)?
                .ok_or(WindowError::UnknownBchPrev { block: prev })?;
            placed.push((prev, place));
        }

        let top = paying_prev.block_hash();
        let low_height = placed
            .iter()
            .map(|(_, place)| place.height.saturating_sub(1))
            .min()
            .unwrap_or(paying_prev.height);
        if !self
            .paying_chain
            .as_ref()
            .is_some_and(|chain| chain.serves(&top, low_height))
        {
            let top_place = BchPlace {
                height: paying_prev.height,
                parent: paying_prev.header.prev_blockhash,
            };
            let chain = PayingChain::walk(top, top_place, low_height, |hash| {
                self.bch_place(hash, chain_store_handle)
            })?;
            self.paying_chain = Some(chain);
        }
        let chain = self.paying_chain.as_ref().expect("walked above");
        let paid = placed
            .into_iter()
            .filter(|(prev, place)| chain.pays(prev, place))
            .map(|(prev, _)| prev)
            .collect();
        Ok(PaidBchPrevs {
            paying_prev: top,
            paid,
        })
    }

    /// Where the BCH block `hash` sits, from the cache or the store, or
    /// `None` when it is not a known header.
    fn bch_place(
        &mut self,
        hash: &BlockHash,
        chain_store_handle: &ChainStoreHandle,
    ) -> Result<Option<BchPlace>, WindowError> {
        if let Some(place) = self.bch_places.get(hash) {
            return Ok(Some(*place));
        }
        let known = chain_store_handle.get_bch_header(hash).map_err(|error| {
            WindowError::ReadFailure(
                format!("Cannot read Bitcoin Cash header {hash}: {error}").into(),
            )
        })?;
        Ok(known.map(|known| {
            let place = BchPlace {
                height: known.height,
                parent: known.header.prev_blockhash,
            };
            self.bch_places.insert(*hash, place);
            place
        }))
    }

    /// The window for an anchor already resolved to its candidate entries and
    /// confirmed entry point.
    fn resolved_window(
        &self,
        candidate_entries: Vec<ConfirmedEntry>,
        confirmed_start_index: usize,
        paying_block_difficulty: u128,
        transferred_payees: HashMap<BlockHash, usize>,
    ) -> ResolvedWindow {
        let paying_height = self.paying_height(&candidate_entries, confirmed_start_index);
        let immature_share_count = self.immature_share_count(paying_height);
        ResolvedWindow {
            candidate_entries,
            confirmed_start_index,
            paying_height,
            immature_share_count,
            work_target: self.work_target(paying_block_difficulty),
            transferred_payees,
            paid_bch_prevs: None,
        }
    }

    /// Bitcoin Cash: the payout address key of every share whose payout right
    /// is held, at the anchor, by someone other than its own miner.
    ///
    /// The rights at the anchor are the index's, with the cached shares above
    /// the anchor's fork point undone and the anchor's branch shares (the
    /// candidate headers, newest first) applied, so the answer depends on the
    /// anchor's own ancestry only. Empty on Bitcoin.
    ///
    /// Errors with `RightsHistoryTooShallow` when the fork point is deeper
    /// than the index can undo, and with `ReadFailure` when a branch share's
    /// transactions cannot be read.
    fn transferred_payees(
        &mut self,
        start_hash: BlockHash,
        candidate_headers: &[(BlockHash, ShareHeader)],
        confirmed_start_index: usize,
        chain_store_handle: &ChainStoreHandle,
    ) -> Result<HashMap<BlockHash, usize>, WindowError> {
        let Some(rights) = &self.rights else {
            return Ok(HashMap::new());
        };
        let fork_height = self.confirmed_entries[confirmed_start_index].height;
        let holders = {
            let mut overlay =
                rights
                    .overlay(confirmed_start_index, fork_height)
                    .map_err(|too_shallow| WindowError::RightsHistoryTooShallow {
                        anchor: start_hash,
                        fork_depth: too_shallow.requested,
                        undo_depth: too_shallow.available,
                    })?;
            for (height_above_fork, (blockhash, header)) in
                (1u32..).zip(candidate_headers.iter().rev())
            {
                let transactions =
                    branch_share_transactions(blockhash, header, chain_store_handle)?;
                overlay.apply_share(
                    ChainShare::from_header(*blockhash, fork_height + height_above_fork, header),
                    share_coinbase_txid(header),
                    &transactions,
                );
            }
            overlay.transferred_holders()
        };

        let mut key_by_holder: HashMap<[u8; 20], usize> = HashMap::with_capacity(holders.len());
        let mut payees = HashMap::with_capacity(holders.len());
        for (share, holder) in holders {
            let key = *key_by_holder.entry(holder).or_insert_with(|| {
                self.address_keys.key_for(
                    Address::p2pkh(PubkeyHash::from_byte_array(holder), self.network).into(),
                )
            });
            payees.insert(share, key);
        }
        Ok(payees)
    }

    /// Work the window must hold for a paying block of this difficulty, or
    /// `None` when only the `max_window_shares` count bounds it.
    ///
    /// `None` on Bitcoin, and on Bitcoin Cash for a paying header at the
    /// network's proof-of-work limit (see `MIN_SIZING_BLOCK_DIFFICULTY`).
    ///
    /// In the unit shares are weighed in: with exact work, `expected_blocks`
    /// times the paying difficulty times the work of one unit of difficulty
    /// (`difficulty_one_work`). The paying difficulty is the whole number
    /// `bitcoin_block_difficulty` gives, so the target is within one unit of
    /// difficulty of the block's exact work: a relative error of about
    /// 2 * 10^-12 at mainnet's difficulty.
    fn work_target(&self, paying_block_difficulty: u128) -> Option<u128> {
        match self.sizing {
            WindowSizing::FixedShares => None,
            WindowSizing::BlockWork { .. }
                if paying_block_difficulty <= MIN_SIZING_BLOCK_DIFFICULTY =>
            {
                None
            }
            WindowSizing::BlockWork { expected_blocks } => Some(
                paying_block_difficulty
                    .saturating_mul(expected_blocks)
                    .saturating_mul(self.weight_of_one_difficulty()),
            ),
        }
    }

    /// What one unit of difficulty weighs in this window's unit.
    fn weight_of_one_difficulty(&self) -> u128 {
        match self.weight_unit {
            WeightUnit::IntegerDifficulty => 1,
            WeightUnit::ExactWork => difficulty_one_work(self.network),
        }
    }

    /// A share's base weight: its integer difficulty on Bitcoin, its exact
    /// work on Bitcoin Cash (see `WeightUnit`).
    fn base_weight(&self, header: &ShareHeader) -> u128 {
        match self.weight_unit {
            WeightUnit::IntegerDifficulty => header.get_difficulty(self.network),
            WeightUnit::ExactWork => work_to_u128(header.get_work()),
        }
    }

    /// Call `visit` with each share in the window resolved from `start_hash`
    /// and its height, newest first, and refuse a walk eviction truncated.
    ///
    /// Errors when the walk exhausts the cache before skipping the immature
    /// shares and counting a full window, and the cache no longer reaches the
    /// chain start (see `get_distribution_from_start_hash`).
    fn walk_window(
        &self,
        start_hash: BlockHash,
        resolved: &ResolvedWindow,
        visit: impl FnMut(u32, &ConfirmedEntry),
    ) -> Result<(), WindowError> {
        let stop_reason = self.walk_window_entries(resolved, visit);

        if stop_reason == WindowStopReason::OutOfEntries && !self.reaches_chain_start() {
            return Err(WindowError::InsufficientEntries {
                anchor: start_hash,
                oldest_cached_height: self
                    .confirmed_entries
                    .back()
                    .map_or(0, |entry| entry.height),
                max_window_shares: self.max_window_shares,
            });
        }
        Ok(())
    }

    /// Whether the oldest cached entry is the first block of the chain, so a
    /// walk that consumes every cached entry has covered the whole chain and
    /// is the same on every node.
    ///
    /// Once eviction has trimmed the back, the earliest cached height depends
    /// on this node's own confirmed tip, so a walk that reaches it would give
    /// a different answer on a node at a different tip.
    fn reaches_chain_start(&self) -> bool {
        self.confirmed_entries
            .back()
            .is_none_or(|entry| entry.height == 0)
    }

    /// Resolve the anchor to its candidate headers and confirmed entry point.
    ///
    /// If start_hash is in confirmed_entries, returns no candidate headers
    /// and the confirmed index. Otherwise walks backward through parent
    /// pointers in the store, collecting candidate headers (newest first)
    /// until a confirmed ancestor is found. Errors when no confirmed ancestor
    /// is reachable or a store read fails.
    fn resolve_start_hash(
        &self,
        start_hash: BlockHash,
        chain_store_handle: &ChainStoreHandle,
    ) -> Result<CandidateWalk, Box<dyn Error + Send + Sync>> {
        if let Some(confirmed_index) = self.find_start_index(start_hash) {
            return Ok((Vec::new(), confirmed_index));
        }
        self.collect_candidate_headers(start_hash, chain_store_handle)
    }

    /// Walk backward from start_hash through parent pointers until
    /// finding a block whose parent is in the confirmed entries.
    /// Returns the collected headers (newest-to-oldest) and the
    /// index of the confirmed entry point the walk lands on.
    ///
    /// The returned headers begin with `start_hash` itself, so the anchor
    /// share counts towards the immature skip and the window like any other.
    ///
    /// The walk follows parent links regardless of validation `status`: the
    /// payout must be a pure function of the chain shape (headers), not of
    /// per-node, timing-dependent validation state, or the producer and a
    /// validator computing the window at different moments would derive
    /// different distributions.
    ///
    /// Errors when the walk runs off the end of the stored chain without
    /// reaching a confirmed ancestor (`get_share_header` returns
    /// `NotFound`: the anchor is unresolvable) or when any store read
    /// fails; both propagate so an unresolvable anchor or a transient read
    /// failure is never mistaken for an empty distribution.
    fn collect_candidate_headers(
        &self,
        start_hash: BlockHash,
        chain_store_handle: &ChainStoreHandle,
    ) -> Result<CandidateWalk, Box<dyn Error + Send + Sync>> {
        const INITIAL_CAPACITY: usize = 8;
        let mut candidate_headers = Vec::with_capacity(INITIAL_CAPACITY);
        let mut current_hash = start_hash;
        let mut confirmed_index = None;

        while confirmed_index.is_none() {
            let header = chain_store_handle.get_share_header(&current_hash)?;
            let parent_hash = header.prev_share_blockhash;
            candidate_headers.push((current_hash, header));
            current_hash = parent_hash;
            confirmed_index = self.find_start_index(current_hash);
        }

        match confirmed_index {
            Some(index) => Ok((candidate_headers, index)),
            None => Err("walk ended without reaching a confirmed ancestor".into()),
        }
    }

    /// Convert collected candidate headers into ConfirmedEntry values
    /// by resolving their difficulty and uncle data from the store.
    fn build_entries_from_headers(
        &mut self,
        candidate_headers: &[(BlockHash, ShareHeader)],
        chain_store_handle: &ChainStoreHandle,
    ) -> Result<Vec<ConfirmedEntry>, Box<dyn Error + Send + Sync>> {
        let all_uncle_hashes: Vec<BlockHash> = candidate_headers
            .iter()
            .flat_map(|(_, header)| header.uncles.iter().copied())
            .collect();
        let uncle_lookup_table =
            self.build_uncle_entry_lookup_table(chain_store_handle, &all_uncle_hashes)?;

        let entries = candidate_headers
            .iter()
            .map(|(blockhash, header)| {
                let difficulty = self.base_weight(header);
                let uncle_entries = resolve_uncle_entries(&header.uncles, &uncle_lookup_table);
                let mut entry = self.build_confirmed_entry(
                    *blockhash,
                    0,
                    header.miner_bitcoin_address.clone(),
                    header.miner_address,
                    difficulty,
                    uncle_entries,
                );
                entry.cuts = self.share_cuts(header);
                entry.bch_prev = header.bitcoin_header.prev_blockhash;
                entry
            })
            .collect();

        Ok(entries)
    }

    /// Find the index of the entry matching the given blockhash.
    /// Returns `None` when the hash is not found in the window.
    fn find_start_index(&self, start_hash: BlockHash) -> Option<usize> {
        self.confirmed_entries
            .iter()
            .position(|entry| entry.blockhash == start_hash)
    }

    /// Height of the paying share: the share whose parent is the anchor.
    ///
    /// The confirmed entry point is the parent of the oldest candidate entry,
    /// or the anchor itself when there are no candidates. Heights are
    /// contiguous along parent links, so no store read is needed.
    fn paying_height(
        &self,
        candidate_entries: &[ConfirmedEntry],
        confirmed_start_index: usize,
    ) -> usize {
        self.confirmed_entries[confirmed_start_index].height as usize + candidate_entries.len() + 1
    }

    /// Number of shares, counted down from the anchor, that are not yet
    /// mature for a paying share at `paying_height`.
    ///
    /// The shares at depth below `maturity_skip` are immature, which is
    /// `maturity_skip - 1` shares starting at the anchor. Shares below height
    /// `maturity_skip` are always mature, so the count is capped at the shares
    /// from height `maturity_skip` up to the anchor. Below that height nothing
    /// is skipped, and the paid set never shrinks as the chain grows.
    fn immature_share_count(&self, paying_height: usize) -> usize {
        self.maturity_skip
            .saturating_sub(1)
            .min(paying_height.saturating_sub(self.maturity_skip))
    }

    /// Walk the candidate entries and then the confirmed entries from the
    /// confirmed entry point, skip the first `immature_share_count`, and
    /// call `visit` with each share of the window that follows and its height.
    ///
    /// The window ends after `max_window_shares` shares, or, with a
    /// `work_target`, after the first share that brings the window's work
    /// (see `ConfirmedEntry::work`) to the target, whichever comes first. So a
    /// window holds at least one share, and at least the target work unless
    /// the share count bounds it.
    ///
    /// Returns the reason the walk stopped. `OutOfEntries` means the cache
    /// itself ran out, which is the one stop reason that is not a property of
    /// the chain: the caller must check whether eviction has trimmed the back
    /// before trusting the result.
    fn walk_window_entries(
        &self,
        resolved: &ResolvedWindow,
        mut visit: impl FnMut(u32, &ConfirmedEntry),
    ) -> WindowStopReason {
        let mut window_entries = resolved
            .candidate_entries
            .iter()
            .chain(
                self.confirmed_entries
                    .iter()
                    .skip(resolved.confirmed_start_index),
            )
            .enumerate()
            .skip(resolved.immature_share_count)
            .take(self.max_window_shares);
        let reached_work_target = |window_work: u128| {
            resolved
                .work_target
                .is_some_and(|target| window_work >= target)
        };

        let mut consumed = 0;
        let mut window_work: u128 = 0;
        while !reached_work_target(window_work)
            && let Some((walk_index, entry)) = window_entries.next()
        {
            consumed += 1;
            window_work = window_work.saturating_add(entry.work());
            visit(resolved.height_at(walk_index), entry);
        }

        // Distinguish "reached the window's bound" from "the cache ran out":
        // only the latter depends on where eviction trimmed the back.
        if consumed == self.max_window_shares || reached_work_target(window_work) {
            WindowStopReason::WindowFull
        } else {
            WindowStopReason::OutOfEntries
        }
    }

    /// Credit one share to the distribution: the share's payee gets the
    /// share's scaled difficulty plus the nephew bonus for each uncle it
    /// includes, and each uncle's miner gets the uncle weight.
    fn add_entry_difficulty(
        entry: &ConfirmedEntry,
        resolved: &ResolvedWindow,
        difficulty_by_key: &mut [u128],
    ) {
        Self::for_each_credit(entry, resolved, |credit| {
            difficulty_by_key[credit.internal_key] =
                difficulty_by_key[credit.internal_key].saturating_add(credit.weight);
        });
    }

    /// Call `credit` with each weight one window share contributes: first the
    /// uncle weight for each uncle it includes, then the share's own scaled
    /// difficulty plus the nephew bonus for those uncles.
    ///
    /// The share's own weight goes to the share's own `miner_bitcoin_address`
    /// key, or on Bitcoin Cash the key of whoever else holds its payout right
    /// (`ResolvedWindow::share_payee`). An uncle's weight always goes to the
    /// uncle's own `miner_bitcoin_address`: an uncle's coinbase output can
    /// never be spent on the chain, so its right never moves.
    ///
    /// On Bitcoin Cash with header rules, a share or uncle off the paying
    /// block's BCH chain is credited nothing (`ResolvedWindow::pays`), and a
    /// share earns an uncle's nephew bonus only when both are paid.
    ///
    /// The one place the PPLNS weighting is applied, so the distribution and
    /// a single miner's view of the window cannot disagree.
    fn for_each_credit(
        entry: &ConfirmedEntry,
        resolved: &ResolvedWindow,
        mut credit: impl FnMut(Credit),
    ) {
        let share_paid = resolved.pays(&entry.bch_prev);
        let mut nephew_bonus: u128 = 0;
        for uncle_entry in &entry.uncle_entries {
            if !resolved.pays(&uncle_entry.bch_prev) {
                continue;
            }
            credit(Credit {
                cuts: uncle_entry.cuts,
                internal_key: uncle_entry.internal_key,
                owner_key: uncle_entry.owner_key,
                blockhash: uncle_entry.blockhash,
                nephew: Some(entry.blockhash),
                difficulty: uncle_entry.difficulty,
                weight: uncle_entry.difficulty.saturating_mul(UNCLE_SCALED_WEIGHT),
            });
            nephew_bonus = nephew_bonus
                .saturating_add(uncle_entry.difficulty.saturating_mul(NEPHEW_SCALED_BONUS));
        }

        if !share_paid {
            return;
        }
        credit(Credit {
            cuts: entry.cuts,
            internal_key: resolved.share_payee(entry),
            owner_key: entry.owner_key,
            blockhash: entry.blockhash,
            nephew: None,
            difficulty: entry.difficulty,
            weight: entry
                .difficulty
                .saturating_mul(DIFFICULTY_SCALE)
                .saturating_add(nephew_bonus),
        });
    }

    /// Free address-key slots no longer referenced by any cached entry.
    ///
    /// A miner address is retained while it appears as a share miner or an
    /// uncle miner in any `confirmed_entries` slot (including the eviction
    /// buffer past the window, since those entries stay cached).
    /// Once its last referencing entry leaves the cache -- via eviction or a
    /// reorg -- the slot is freed so the `AddressKeys` interner stays bounded
    /// and its linear `key_for` scan does not grow without bound. Runs after
    /// eviction in `update`; replaces the stale-key cleanup that the removed
    /// tip-anchored `get_distribution` performed inline.
    fn prune_unreferenced_keys(&mut self) {
        let mut referenced = vec![false; self.address_keys.len()];
        let mut owner_referenced = vec![false; self.owner_keys.len()];
        for entry in &self.confirmed_entries {
            entry.cuts.retain_keys(&mut referenced);
            referenced[entry.internal_key] = true;
            owner_referenced[entry.owner_key] = true;
            for uncle_entry in &entry.uncle_entries {
                uncle_entry.cuts.retain_keys(&mut referenced);
                referenced[uncle_entry.internal_key] = true;
                owner_referenced[uncle_entry.owner_key] = true;
            }
        }
        remove_unreferenced_keys(&mut self.address_keys, &referenced);
        remove_unreferenced_keys(&mut self.owner_keys, &owner_referenced);
        self.prune_unreferenced_bch_places();
    }

    /// Drop the cached places of BCH blocks no cached share, uncle or paying
    /// chain names, so the cache stays the size of the window.
    fn prune_unreferenced_bch_places(&mut self) {
        if self.bch_places.is_empty() {
            return;
        }
        let mut named: HashSet<BlockHash> = self
            .confirmed_entries
            .iter()
            .flat_map(|entry| {
                std::iter::once(entry.bch_prev)
                    .chain(entry.uncle_entries.iter().map(|uncle| uncle.bch_prev))
            })
            .collect();
        if let Some(chain) = &self.paying_chain {
            named.extend(chain.hashes());
        }
        self.bch_places.retain(|hash, _| named.contains(hash));
    }

    /// Aggregate weights by locking bytecode, including cuts whose script
    /// is also a miner's, then retain one address for each script.
    fn collect_distribution(
        &self,
        difficulty_by_key: &[u128],
    ) -> Result<HashMap<PayoutAddress, u128>, WindowError> {
        let mut by_script: HashMap<ScriptBuf, (PayoutAddress, u128)> =
            HashMap::with_capacity(difficulty_by_key.len());
        for (index, difficulty) in difficulty_by_key.iter().enumerate() {
            if *difficulty > 0
                && let Some(address) = self.address_keys.value_for(index)
            {
                let (representative, weight) = by_script
                    .entry(address.script_pubkey())
                    .or_insert_with(|| (address.clone(), 0));
                *weight = weight.checked_add(*difficulty).ok_or_else(|| {
                    WindowError::ReadFailure("Aggregated payout weights overflow".into())
                })?;
                if address.to_string() < representative.to_string() {
                    *representative = address.clone();
                }
            }
        }
        Ok(by_script.into_values().collect())
    }

    /// Update the cache from the chain store incrementally.
    ///
    /// Loads only newly confirmed headers since the last cached height,
    /// handles reorgs by removing only the divergent entries and loading
    /// the new fork, and evicts entries beyond `cache_capacity`.
    ///
    /// Returns Ok(true) if the cache was updated, Ok(false) if no changes.
    pub fn update(
        &mut self,
        chain_store_handle: &ChainStoreHandle,
    ) -> Result<bool, Box<dyn Error + Send + Sync>> {
        let tip_blockhash = chain_store_handle.get_chain_tip()?;

        if self.cached_tip_blockhash == Some(tip_blockhash) {
            return Ok(false);
        }

        let tip_metadata = chain_store_handle.get_block_metadata(&tip_blockhash)?;
        let Some(tip_height) = tip_metadata.expected_height else {
            return Ok(false);
        };

        if let Err(error) = self.load_to_tip(tip_height, chain_store_handle) {
            // A load that failed part way has pushed some entries, and on
            // Bitcoin Cash applied them to the payout rights index, without
            // moving the cached tip. Start over on the next update rather than
            // load the same shares twice.
            self.invalidate();
            return Err(error);
        }

        self.evict_overflow();
        self.prune_unreferenced_keys();
        self.cached_tip_blockhash = Some(tip_blockhash);
        self.cached_top_height = Some(tip_height);

        Ok(true)
    }

    /// Bring the cached entries from the cached tip to the confirmed tip at
    /// `tip_height`: extend them, roll back a reorg, or load them all on the
    /// first update.
    fn load_to_tip(
        &mut self,
        tip_height: u32,
        chain_store_handle: &ChainStoreHandle,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        match self.cached_top_height {
            Some(cached_height) => {
                if self.is_simple_extension(cached_height, tip_height, chain_store_handle)? {
                    self.load_range(chain_store_handle, cached_height + 1, tip_height)
                } else {
                    self.handle_reorg(chain_store_handle, tip_height)
                }
            }
            None => {
                let estimated_min_height = tip_height.saturating_sub(self.cache_capacity() as u32);
                self.load_range(chain_store_handle, estimated_min_height, tip_height)
            }
        }
    }

    /// Check whether the new tip is a simple extension of the cached chain.
    ///
    /// Returns true when the tip is higher than the cached height and the
    /// confirmed blockhash at the cached height still matches the front
    /// entry in the cache.
    fn is_simple_extension(
        &self,
        cached_height: u32,
        tip_height: u32,
        chain_store_handle: &ChainStoreHandle,
    ) -> Result<bool, Box<dyn Error + Send + Sync>> {
        if tip_height <= cached_height {
            return Ok(false);
        }

        let confirmed_at_cached = chain_store_handle.get_confirmed_at_height(cached_height)?;
        match self.confirmed_entries.front() {
            Some(entry) if entry.blockhash == confirmed_at_cached => Ok(true),
            _ => Ok(false),
        }
    }

    /// Handle a reorg by finding the fork point and doing targeted removal
    /// and reload. Falls back to full invalidation for deep reorgs, and on
    /// Bitcoin Cash for a fork deeper than the payout rights index can undo.
    fn handle_reorg(
        &mut self,
        chain_store_handle: &ChainStoreHandle,
        tip_height: u32,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        match self.find_fork_height(chain_store_handle)? {
            Some(fork_height) if self.rights_can_undo_above(fork_height) => {
                debug!("Reorg detected in PPLNS window, rolling back to fork height {fork_height}");
                self.remove_entries_above_height(fork_height);
                self.cached_top_height = Some(fork_height);
                self.load_range(chain_store_handle, fork_height + 1, tip_height)?;
            }
            _ => {
                debug!("Deep reorg detected in PPLNS window, full cache invalidation");
                self.invalidate();
                let estimated_min_height = tip_height.saturating_sub(self.cache_capacity() as u32);
                self.load_range(chain_store_handle, estimated_min_height, tip_height)?;
            }
        }
        Ok(())
    }

    /// Whether the payout rights index can undo every cached share above
    /// `fork_height`. Always true on Bitcoin, which has no index.
    fn rights_can_undo_above(&self, fork_height: u32) -> bool {
        let shares_above_fork = self
            .confirmed_entries
            .iter()
            .take_while(|entry| entry.height > fork_height)
            .count();
        self.rights
            .as_ref()
            .is_none_or(|rights| rights.undo_depth() >= shares_above_fork)
    }

    /// Clear all cached state, forcing a full reload on next update.
    fn invalidate(&mut self) {
        self.confirmed_entries.clear();
        self.cached_tip_blockhash = None;
        self.cached_top_height = None;
        if let Some(rights) = &mut self.rights {
            rights.clear();
        }
    }

    /// Find the fork height by walking cached entries from newest to oldest.
    ///
    /// Checks at most MAX_REORG_SCAN_DEPTH entries to bound the cost of
    /// deep reorgs. Returns the height of the first cached entry whose
    /// blockhash still matches the confirmed chain (the fork point), or
    /// None if no match is found within the scan limit.
    fn find_fork_height(
        &self,
        chain_store_handle: &ChainStoreHandle,
    ) -> Result<Option<u32>, Box<dyn Error + Send + Sync>> {
        let scan_limit = MAX_REORG_SCAN_DEPTH.min(self.confirmed_entries.len());

        for index in 0..scan_limit {
            let entry = &self.confirmed_entries[index];
            let confirmed_hash = chain_store_handle.get_confirmed_at_height(entry.height);

            match confirmed_hash {
                Ok(hash) if hash == entry.blockhash => return Ok(Some(entry.height)),
                _ => {}
            }
        }

        Ok(None)
    }

    /// Remove all cached entries with height strictly above the fork height.
    ///
    /// Pops entries from the front of the deque (newest first) until the
    /// front entry's height equals the fork height, undoing each one's
    /// changes to the payout rights index on Bitcoin Cash. The caller checks
    /// `rights_can_undo_above` first.
    fn remove_entries_above_height(&mut self, fork_height: u32) {
        while let Some(front) = self.confirmed_entries.front() {
            if front.height <= fork_height {
                return;
            }
            if let Some(entry) = self.confirmed_entries.pop_front()
                && let Some(rights) = &mut self.rights
                && !rights.undo_share(&entry.blockhash)
            {
                warn!(
                    "Payout rights index has no undo record for share {}",
                    entry.blockhash
                );
            }
        }
    }

    /// Load confirmed headers for a height range and add them to the cache.
    ///
    /// Fetches headers from the chain store, resolves uncle data and builds
    /// confirmed entries. New entries are prepended (newest at front). On
    /// Bitcoin Cash each share is also applied to the payout rights index,
    /// oldest first, reading its transactions when its header commits to any.
    fn load_range(
        &mut self,
        chain_store_handle: &ChainStoreHandle,
        from_height: u32,
        to_height: u32,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        let confirmed_headers =
            chain_store_handle.get_confirmed_headers_in_range(from_height, to_height)?;

        if confirmed_headers.is_empty() {
            return Ok(());
        }

        let all_uncle_hashes = collect_unique_uncle_hashes(&confirmed_headers);
        let uncle_lookup_table =
            self.build_uncle_entry_lookup_table(chain_store_handle, &all_uncle_hashes)?;

        // Headers arrive newest-to-oldest. Reverse to oldest-first so
        // the newest entry ends up at position 0 in the deque after push_front.
        for ConfirmedHeaderResult {
            height,
            blockhash,
            header,
        } in confirmed_headers.into_iter().rev()
        {
            let difficulty = self.base_weight(&header);
            let uncle_entries = resolve_uncle_entries(&header.uncles, &uncle_lookup_table);
            let coinbase_txid = match &mut self.rights {
                Some(rights) => {
                    let coinbase_txid = share_coinbase_txid(&header);
                    let transactions =
                        confirmed_share_transactions(&blockhash, &header, chain_store_handle)?;
                    rights.apply_share(
                        ChainShare::from_header(blockhash, height, &header),
                        coinbase_txid,
                        &transactions,
                    );
                    coinbase_txid
                }
                None => Txid::all_zeros(),
            };
            let cuts = self.share_cuts(&header);
            let mut entry = self.build_confirmed_entry(
                blockhash,
                height,
                header.miner_bitcoin_address,
                header.miner_address,
                difficulty,
                uncle_entries,
            );
            entry.coinbase_txid = coinbase_txid;
            entry.cuts = cuts;
            entry.bch_prev = header.bitcoin_header.prev_blockhash;
            self.confirmed_entries.push_front(entry);
        }

        Ok(())
    }

    /// Total confirmed entries the cache loads and retains: the maturity depth
    /// and the window, plus a ~1% buffer of the window (see
    /// `PPLNS_WINDOW_BUFFER_DIVISOR`) so an anchor slightly behind the tip
    /// still has its full window available.
    ///
    /// The buffer is therefore the supported anchor depth: an anchor at most
    /// `max_window_shares / PPLNS_WINDOW_BUFFER_DIVISOR` entries behind the tip
    /// still has `maturity_skip + max_window_shares` entries cached below it,
    /// which is what `get_distribution_from_start_hash` needs to apply its
    /// count bound. A deeper anchor is reported as an error rather than
    /// silently truncated.
    fn cache_capacity(&self) -> usize {
        self.maturity_skip
            + self.max_window_shares
            + self.max_window_shares.div_ceil(PPLNS_WINDOW_BUFFER_DIVISOR)
    }

    /// Trim the oldest confirmed entries down to `cache_capacity`.
    ///
    /// This bounds memory only. It must not be what bounds a window query:
    /// the back of the deque moves as this node's tip advances, so a walk that
    /// stopped there would give different answers on different nodes. Query
    /// bounds live in `get_distribution_from_start_hash`, which counts from the
    /// anchor and refuses a result that ran into an evicted back.
    fn evict_overflow(&mut self) {
        while self.confirmed_entries.len() > self.cache_capacity() {
            if let Some(entry) = self.confirmed_entries.pop_back()
                && let Some(rights) = &mut self.rights
            {
                rights.evict_share(&entry.blockhash, &entry.coinbase_txid);
            }
        }
        let cached_shares = self.confirmed_entries.len();
        if let Some(rights) = &mut self.rights {
            rights.limit_undo(cached_shares);
        }
    }

    /// Build a ConfirmedEntry, resolving the miner address and the share
    /// owner to internal keys.
    fn build_confirmed_entry(
        &mut self,
        blockhash: BlockHash,
        height: u32,
        miner_address: PayoutAddress,
        share_owner: ShareOwner,
        difficulty: u128,
        uncle_entries: Vec<UncleEntry>,
    ) -> ConfirmedEntry {
        let internal_key = self.address_keys.key_for(miner_address);
        let owner_key = self.owner_keys.key_for(share_owner);

        ConfirmedEntry {
            cuts: ShareCuts::default(),
            blockhash,
            height,
            difficulty,
            uncle_entries,
            internal_key,
            owner_key,
            coinbase_txid: Txid::all_zeros(),
            bch_prev: BlockHash::all_zeros(),
        }
    }

    /// Build an UncleEntry, resolving the miner address and the share owner
    /// to internal keys.
    fn build_uncle_entry(
        &mut self,
        blockhash: BlockHash,
        miner_address: PayoutAddress,
        share_owner: ShareOwner,
        difficulty: u128,
    ) -> UncleEntry {
        let internal_key = self.address_keys.key_for(miner_address);
        let owner_key = self.owner_keys.key_for(share_owner);
        UncleEntry {
            cuts: ShareCuts::default(),
            blockhash,
            internal_key,
            owner_key,
            difficulty,
            bch_prev: BlockHash::all_zeros(),
        }
    }

    /// Build uncle entries and return a hashmap of uncle block hash
    /// -> uncle entry.
    /// The uncle headers are queried in a batch header query.
    fn build_uncle_entry_lookup_table(
        &mut self,
        chain_store_handle: &ChainStoreHandle,
        uncle_hashes: &[BlockHash],
    ) -> Result<HashMap<BlockHash, UncleEntry>, Box<dyn Error + Send + Sync>> {
        if uncle_hashes.is_empty() {
            return Ok(HashMap::new());
        }

        let uncle_headers = chain_store_handle.get_share_headers(uncle_hashes)?;
        let mut uncle_lookup = HashMap::with_capacity(uncle_headers.len());
        for (blockhash, header) in uncle_headers {
            let difficulty = self.base_weight(&header);
            let cuts = self.share_cuts(&header);
            let mut uncle_entry = self.build_uncle_entry(
                blockhash,
                header.miner_bitcoin_address,
                header.miner_address,
                difficulty,
            );
            uncle_entry.cuts = cuts;
            uncle_entry.bch_prev = header.bitcoin_header.prev_blockhash;
            uncle_lookup.insert(blockhash, uncle_entry);
        }
        Ok(uncle_lookup)
    }
}

/// The transactions of a confirmed share being added to the cache, for the
/// payout rights index: none when its header commits to none.
///
/// A confirmed share with transactions but no stored body was confirmed from
/// its header alone, which happens only far below the candidate tip during
/// sync (deeper than `PRUNE_DEPTH`, where bodies are never fetched). Its
/// transactions can only move rights of shares at least that deep, which no
/// payout validated in the PPLNS zone reaches, so it is applied as carrying
/// none. Any other read failure is an error.
fn confirmed_share_transactions(
    blockhash: &BlockHash,
    header: &ShareHeader,
    chain_store_handle: &ChainStoreHandle,
) -> Result<Vec<ShareTransaction>, Box<dyn Error + Send + Sync>> {
    if !has_share_transactions(header) {
        return Ok(Vec::new());
    }
    match chain_store_handle.get_share_transactions(blockhash) {
        Ok(transactions) => Ok(transactions),
        Err(StoreError::NotFound(_)) => {
            debug!(
                "Confirmed share {blockhash} has no stored body; its transactions move no payout rights here"
            );
            Ok(Vec::new())
        }
        Err(error) => Err(error.into()),
    }
}

/// The transactions of a share on a payout anchor's branch, above the
/// cached chain: none when its header commits to none.
///
/// Unlike a confirmed share, a branch share's transactions can move the
/// rights of the anchor's own window, so a missing body is a read failure
/// the caller may retry, never an empty share.
fn branch_share_transactions(
    blockhash: &BlockHash,
    header: &ShareHeader,
    chain_store_handle: &ChainStoreHandle,
) -> Result<Vec<ShareTransaction>, WindowError> {
    if !has_share_transactions(header) {
        return Ok(Vec::new());
    }
    chain_store_handle
        .get_share_transactions(blockhash)
        .map_err(|error| {
            WindowError::ReadFailure(
                format!("Failed to read the transactions of branch share {blockhash}: {error}")
                    .into(),
            )
        })
}

/// Sum of a window's weights, saturating: only whether it is zero matters.
fn total_weight(weights: &[u128]) -> u128 {
    weights
        .iter()
        .fold(0, |total: u128, weight| total.saturating_add(*weight))
}

/// Key each miner's credits by the value its slot in `keys` holds, keeping
/// only the miners the window credits.
fn credited_by_value<T: Clone + Eq + hash::Hash>(
    keys: &AddressKeys<T>,
    credits_by_key: Vec<MinerCredits>,
) -> HashMap<T, MinerCredits> {
    let mut by_value = HashMap::with_capacity(credits_by_key.len());
    for (index, miner_credits) in credits_by_key.into_iter().enumerate() {
        let is_credited = miner_credits.share_count + miner_credits.uncle_count > 0;
        if is_credited && let Some(value) = keys.value_for(index) {
            by_value.insert(value.clone(), miner_credits);
        }
    }
    by_value
}

/// Free each slot of `keys` that `referenced` marks unreferenced.
fn remove_unreferenced_keys<T: PartialEq>(keys: &mut AddressKeys<T>, referenced: &[bool]) {
    for (index, is_referenced) in referenced.iter().enumerate() {
        if !is_referenced {
            keys.remove(index);
        }
    }
}

/// Collect unique uncle blockhashes from confirmed headers for batch fetching.
fn collect_unique_uncle_hashes(confirmed_headers: &[ConfirmedHeaderResult]) -> Vec<BlockHash> {
    let mut seen_uncles: HashSet<BlockHash> = HashSet::new();
    let mut all_uncle_hashes = Vec::with_capacity(confirmed_headers.len());

    for result in confirmed_headers {
        let header = &result.header;
        for uncle_hash in &header.uncles {
            if seen_uncles.insert(*uncle_hash) {
                all_uncle_hashes.push(*uncle_hash);
            }
        }
    }

    all_uncle_hashes
}

/// Resolve uncle hashes into UncleEntry values using a pre-built lookup table.
fn resolve_uncle_entries(
    uncle_hashes: &[BlockHash],
    uncle_lookup: &HashMap<BlockHash, UncleEntry>,
) -> Vec<UncleEntry> {
    let mut entries = Vec::with_capacity(uncle_hashes.len());
    for uncle_hash in uncle_hashes {
        if let Some(uncle_entry) = uncle_lookup.get(uncle_hash) {
            entries.push(UncleEntry {
                cuts: uncle_entry.cuts,
                blockhash: uncle_entry.blockhash,
                difficulty: uncle_entry.difficulty,
                internal_key: uncle_entry.internal_key,
                owner_key: uncle_entry.owner_key,
                bch_prev: uncle_entry.bch_prev,
            });
        } else {
            tracing::warn!("Uncle header not found for {uncle_hash}, skipping");
        }
    }
    entries
}

/// Share owner every benchmark entry gets.
#[cfg(any(test, feature = "test-utils", feature = "sim"))]
const BENCHMARK_SHARE_OWNER: ShareOwner = ShareOwner::CashP2pkh([0u8; 20]);

#[cfg(any(test, feature = "test-utils", feature = "sim"))]
impl PplnsWindow {
    /// Populate the window cache directly for benchmarking.
    ///
    /// Accepts confirmed shares and uncles as tuples of primitives,
    /// bypassing the chain store. Confirmed shares should be ordered
    /// newest-to-oldest. Uncle data is provided as (miner_address_string, difficulty).
    ///
    /// The payout never reads share owners or uncle hashes, so every entry
    /// gets the same placeholder owner and uncles the all-zero hash.
    #[allow(clippy::type_complexity)] // benchmark-only shim: tuples mirror the store rows verbatim
    pub fn populate_for_benchmark(
        &mut self,
        confirmed_shares: Vec<(BlockHash, String, u128, Vec<(String, u128)>)>,
    ) {
        self.invalidate();
        self.confirmed_entries.reserve(confirmed_shares.len());

        let total_count = confirmed_shares.len() as u32;
        for (index, (blockhash, miner_address_string, difficulty, uncle_data)) in
            confirmed_shares.into_iter().enumerate()
        {
            let mut uncle_entries = Vec::with_capacity(uncle_data.len());
            for (uncle_miner_string, uncle_difficulty) in uncle_data {
                let uncle_miner = uncle_miner_string
                    .parse::<bitcoin::Address<_>>()
                    .unwrap()
                    .assume_checked();
                uncle_entries.push(self.build_uncle_entry(
                    BlockHash::all_zeros(),
                    uncle_miner.into(),
                    BENCHMARK_SHARE_OWNER,
                    uncle_difficulty,
                ));
            }

            let miner_address = miner_address_string
                .parse::<bitcoin::Address<_>>()
                .unwrap()
                .assume_checked();
            let height = total_count - 1 - index as u32;
            let entry = self.build_confirmed_entry(
                blockhash,
                height,
                miner_address.into(),
                BENCHMARK_SHARE_OWNER,
                difficulty,
                uncle_entries,
            );
            self.confirmed_entries.push_back(entry);
        }

        if let Some(entry) = self.confirmed_entries.front() {
            self.cached_tip_blockhash = Some(entry.blockhash);
        }
        self.cached_top_height = Some(total_count.saturating_sub(1));
        eprintln!("  populated {} in window", self.confirmed_entries.len());
    }

    /// Expose the stale-key sweep for benchmarking. In production this runs
    /// only as an internal step of `update` after eviction.
    pub fn prune_unreferenced_keys_for_benchmark(&mut self) {
        self.prune_unreferenced_keys();
    }

    /// Distribution for an anchor that is already cached, without a chain
    /// store, for benchmarks and the offline payout simulation.
    ///
    /// Runs the same skip, window walk and truncation check as
    /// `get_distribution_from_start_hash`. Errors with `ReadFailure` when the
    /// anchor is not cached.
    pub fn distribution_for_cached_anchor(
        &self,
        start_hash: BlockHash,
        paying_block_difficulty: u128,
    ) -> Result<HashMap<PayoutAddress, u128>, WindowError> {
        let confirmed_start_index = self.find_start_index(start_hash).ok_or_else(|| {
            WindowError::ReadFailure(format!("anchor {start_hash} is not cached").into())
        })?;
        let resolved = self.resolved_window(
            Vec::new(),
            confirmed_start_index,
            paying_block_difficulty,
            HashMap::new(),
        );
        self.distribution_for_resolved(start_hash, &resolved)
    }
}

#[cfg(test)]
mockall::mock! {
    pub PplnsWindow {
        pub fn new(network: bitcoin::Network) -> Self;
        pub fn network(&self) -> bitcoin::Network;
        pub fn update(
            &mut self,
            chain_store_handle: &ChainStoreHandle,
        ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>>;
        pub fn get_distribution_from_start_hash(
            &mut self,
            start_hash: BlockHash,
            paying_block: PayingBlock,
            chain_store_handle: &ChainStoreHandle,
        ) -> Result<HashMap<PayoutAddress, u128>, WindowError>;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shares::chain::chain_store_handle::MockChainStoreHandle;
    use crate::shares::share_block::ShareHeader;
    use crate::store::block_tx_metadata::{BlockMetadata, ChainMembership, Status};
    use crate::test_utils::{
        PUBKEY_2G, PUBKEY_3G, PUBKEY_4G, PUBKEY_5G, PUBKEY_G, TestShareBlockBuilder,
        build_test_header, build_test_header_with_uncles,
    };
    use bitcoin::hashes::Hash;
    use bitcoin::{Network, Work};

    /// Network used for test difficulty calculations.
    const TEST_NETWORK: bitcoin::Network = bitcoin::Network::Signet;

    /// Paying block difficulty for tests of the fixed Bitcoin window, which
    /// does not read it.
    const BITCOIN_BLOCK_DIFFICULTY: u128 = 1;

    /// Create a ConfirmedEntry from a ShareHeader with no uncles.
    fn entry_from_header(
        window: &mut PplnsWindow,
        header: &ShareHeader,
        height: u32,
    ) -> ConfirmedEntry {
        let mut entry = window.build_confirmed_entry(
            header.block_hash(),
            height,
            header.miner_bitcoin_address.clone(),
            header.miner_address,
            window.base_weight(header),
            Vec::new(),
        );
        entry.cuts = window.share_cuts(header);
        entry.bch_prev = header.bitcoin_header.prev_blockhash;
        entry
    }

    /// Create a ConfirmedEntry from a ShareHeader with resolved uncle entries.
    fn entry_from_header_with_uncles(
        window: &mut PplnsWindow,
        header: &ShareHeader,
        height: u32,
        uncle_entries: Vec<UncleEntry>,
    ) -> ConfirmedEntry {
        let mut entry = window.build_confirmed_entry(
            header.block_hash(),
            height,
            header.miner_bitcoin_address.clone(),
            header.miner_address,
            window.base_weight(header),
            uncle_entries,
        );
        entry.cuts = window.share_cuts(header);
        entry.bch_prev = header.bitcoin_header.prev_blockhash;
        entry
    }

    /// Create an UncleEntry from a ShareHeader for test setup.
    fn uncle_entry_from_header(window: &mut PplnsWindow, header: &ShareHeader) -> UncleEntry {
        let mut entry = window.build_uncle_entry(
            header.block_hash(),
            header.miner_bitcoin_address.clone(),
            header.miner_address,
            window.base_weight(header),
        );
        entry.cuts = window.share_cuts(header);
        entry.bch_prev = header.bitcoin_header.prev_blockhash;
        entry
    }

    /// Create a BlockMetadata with the given height and Confirmed status.
    fn metadata_at_height(height: u32) -> BlockMetadata {
        BlockMetadata {
            expected_height: Some(height),
            chain_work: Work::from_le_bytes([0u8; 32]),
            status: Status::BlockValid,
            chain: ChainMembership::Confirmed,
        }
    }

    /// Create a BlockMetadata with no height (empty chain).
    fn metadata_no_height() -> BlockMetadata {
        BlockMetadata {
            expected_height: None,
            chain_work: Work::from_le_bytes([0u8; 32]),
            status: Status::BlockValid,
            chain: ChainMembership::Confirmed,
        }
    }

    /// Build a chain of headers for testing.
    /// Returns (headers_vec, tip_hash) where headers_vec is newest-to-oldest
    /// with heights assigned starting from 0.
    fn build_test_chain(
        count: usize,
        miner_pubkeys: &[&str],
    ) -> (Vec<ConfirmedHeaderResult>, BlockHash) {
        let genesis_hash = BlockHash::all_zeros();
        let mut headers = Vec::with_capacity(count);
        let mut prev_hash = genesis_hash.to_string();

        for index in 0..count {
            let pubkey = miner_pubkeys[index % miner_pubkeys.len()];
            let header = build_test_header(&prev_hash, pubkey, 2);
            let blockhash = header.block_hash();
            prev_hash = blockhash.to_string();
            headers.push(ConfirmedHeaderResult {
                height: index as u32,
                blockhash,
                header,
            });
        }

        let tip_hash = headers.last().unwrap().blockhash;
        // Reverse to newest-to-oldest
        headers.reverse();
        (headers, tip_hash)
    }

    #[test]
    fn test_initial_full_load() {
        let (headers, tip_hash) = build_test_chain(5, &[PUBKEY_G, PUBKEY_2G]);

        let mut mock = MockChainStoreHandle::default();
        let headers_clone = headers.clone();
        mock.expect_get_chain_tip().returning(move || Ok(tip_hash));
        mock.expect_get_block_metadata()
            .returning(move |_| Ok(metadata_at_height(4)));
        mock.expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(headers_clone.clone()));
        mock.expect_get_share_headers()
            .returning(|_| Ok(Vec::new()));

        let mut window = PplnsWindow::new(TEST_NETWORK);
        let updated = window.update(&mock).unwrap();

        assert!(updated);
        assert!(!window.is_empty());
        assert_eq!(window.confirmed_entries.len(), 5);
        assert_eq!(window.cached_tip_blockhash, Some(tip_hash));
        assert_eq!(window.cached_top_height, Some(4));
    }

    #[test]
    fn test_incremental_load_new_shares() {
        let (all_headers, _) = build_test_chain(7, &[PUBKEY_G, PUBKEY_2G]);
        // Initial chain: headers at heights 0-4 (indices 2..7 in all_headers reversed)
        let initial_headers: Vec<ConfirmedHeaderResult> = all_headers[2..].to_vec();
        let initial_tip_hash = initial_headers[0].blockhash;

        // Set up initial load
        let mut mock = MockChainStoreHandle::default();
        let initial_headers_clone = initial_headers.clone();
        mock.expect_get_chain_tip()
            .returning(move || Ok(initial_tip_hash));
        mock.expect_get_block_metadata()
            .returning(move |_| Ok(metadata_at_height(4)));
        mock.expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(initial_headers_clone.clone()));
        mock.expect_get_share_headers()
            .returning(|_| Ok(Vec::new()));

        let mut window = PplnsWindow::new(TEST_NETWORK);
        window.update(&mock).unwrap();
        assert_eq!(window.confirmed_entries.len(), 5);

        // Now extend to height 6
        let new_tip_hash = all_headers[0].blockhash;
        let new_headers: Vec<ConfirmedHeaderResult> = all_headers[0..2].to_vec();
        let older_confirmed_tip_header = initial_headers[0].blockhash;

        let mut mock2 = MockChainStoreHandle::default();
        mock2
            .expect_get_chain_tip()
            .returning(move || Ok(new_tip_hash));
        mock2
            .expect_get_block_metadata()
            .returning(move |_| Ok(metadata_at_height(6)));
        mock2
            .expect_get_confirmed_at_height()
            .returning(move |_| Ok(older_confirmed_tip_header));
        mock2
            .expect_get_confirmed_headers_in_range()
            .withf(|from, to| *from == 5 && *to == 6)
            .returning(move |_, _| Ok(new_headers.clone()));
        mock2
            .expect_get_share_headers()
            .returning(|_| Ok(Vec::new()));

        let updated = window.update(&mock2).unwrap();
        assert!(updated);
        assert_eq!(window.confirmed_entries.len(), 7);
        assert_eq!(window.cached_top_height, Some(6));
    }

    #[test]
    fn test_incremental_load_preserves_newest_first_ordering() {
        // Build a chain of 9 headers (heights 0..8), load in three batches
        // and verify the deque stays in newest-to-oldest order throughout.
        let (all_headers, _) = build_test_chain(9, &[PUBKEY_G, PUBKEY_2G, PUBKEY_3G]);

        // Batch 1: heights 0-2 (indices 6..9 in newest-to-oldest all_headers)
        let batch1: Vec<ConfirmedHeaderResult> = all_headers[6..].to_vec();
        let tip1 = batch1[0].blockhash;

        let mut mock1 = MockChainStoreHandle::default();
        let batch1_clone = batch1.clone();
        mock1.expect_get_chain_tip().returning(move || Ok(tip1));
        mock1
            .expect_get_block_metadata()
            .returning(move |_| Ok(metadata_at_height(2)));
        mock1
            .expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(batch1_clone.clone()));
        mock1
            .expect_get_share_headers()
            .returning(|_| Ok(Vec::new()));

        let mut window = PplnsWindow::new(TEST_NETWORK);
        window.update(&mock1).unwrap();
        assert_eq!(window.confirmed_entries.len(), 3);
        // Deque should be [height2, height1, height0]
        assert_eq!(
            window.confirmed_entries[0].blockhash,
            all_headers[6].blockhash
        );
        assert_eq!(
            window.confirmed_entries[1].blockhash,
            all_headers[7].blockhash
        );
        assert_eq!(
            window.confirmed_entries[2].blockhash,
            all_headers[8].blockhash
        );

        // Batch 2: heights 3-5 (indices 3..6)
        let batch2: Vec<ConfirmedHeaderResult> = all_headers[3..6].to_vec();
        let tip2 = batch2[0].blockhash;
        let confirmed_at_2 = batch1[0].blockhash;

        let mut mock2 = MockChainStoreHandle::default();
        let batch2_clone = batch2.clone();
        mock2.expect_get_chain_tip().returning(move || Ok(tip2));
        mock2
            .expect_get_block_metadata()
            .returning(move |_| Ok(metadata_at_height(5)));
        mock2
            .expect_get_confirmed_at_height()
            .returning(move |_| Ok(confirmed_at_2));
        mock2
            .expect_get_confirmed_headers_in_range()
            .withf(|from, to| *from == 3 && *to == 5)
            .returning(move |_, _| Ok(batch2_clone.clone()));
        mock2
            .expect_get_share_headers()
            .returning(|_| Ok(Vec::new()));

        window.update(&mock2).unwrap();
        assert_eq!(window.confirmed_entries.len(), 6);
        // Deque should be [height5, height4, height3, height2, height1, height0]
        assert_eq!(
            window.confirmed_entries[0].blockhash,
            all_headers[3].blockhash
        );
        assert_eq!(
            window.confirmed_entries[1].blockhash,
            all_headers[4].blockhash
        );
        assert_eq!(
            window.confirmed_entries[2].blockhash,
            all_headers[5].blockhash
        );
        assert_eq!(
            window.confirmed_entries[3].blockhash,
            all_headers[6].blockhash
        );
        assert_eq!(
            window.confirmed_entries[4].blockhash,
            all_headers[7].blockhash
        );
        assert_eq!(
            window.confirmed_entries[5].blockhash,
            all_headers[8].blockhash
        );

        // Batch 3: heights 6-8 (indices 0..3)
        let batch3: Vec<ConfirmedHeaderResult> = all_headers[0..3].to_vec();
        let tip3 = batch3[0].blockhash;
        let confirmed_at_5 = batch2[0].blockhash;

        let mut mock3 = MockChainStoreHandle::default();
        let batch3_clone = batch3.clone();
        mock3.expect_get_chain_tip().returning(move || Ok(tip3));
        mock3
            .expect_get_block_metadata()
            .returning(move |_| Ok(metadata_at_height(8)));
        mock3
            .expect_get_confirmed_at_height()
            .returning(move |_| Ok(confirmed_at_5));
        mock3
            .expect_get_confirmed_headers_in_range()
            .withf(|from, to| *from == 6 && *to == 8)
            .returning(move |_, _| Ok(batch3_clone.clone()));
        mock3
            .expect_get_share_headers()
            .returning(|_| Ok(Vec::new()));

        window.update(&mock3).unwrap();
        assert_eq!(window.confirmed_entries.len(), 9);
        // Full deque: [height8, height7, ..., height0] -- strict newest-to-oldest
        for (index, entry) in window.confirmed_entries.iter().enumerate() {
            assert_eq!(
                entry.blockhash, all_headers[index].blockhash,
                "entry at position {index} has wrong blockhash"
            );
        }
    }

    #[test]
    fn test_no_update_when_tip_unchanged() {
        let (headers, tip_hash) = build_test_chain(3, &[PUBKEY_G]);

        let mut mock = MockChainStoreHandle::default();
        let headers_clone = headers.clone();
        mock.expect_get_chain_tip().returning(move || Ok(tip_hash));
        mock.expect_get_block_metadata()
            .returning(move |_| Ok(metadata_at_height(2)));
        mock.expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(headers_clone.clone()));
        mock.expect_get_share_headers()
            .returning(|_| Ok(Vec::new()));

        let mut window = PplnsWindow::new(TEST_NETWORK);
        window.update(&mock).unwrap();

        // Second update with same tip
        let mut mock2 = MockChainStoreHandle::default();
        mock2.expect_get_chain_tip().returning(move || Ok(tip_hash));

        let updated = window.update(&mock2).unwrap();
        assert!(!updated);
    }

    #[test]
    fn test_reorg_invalidates_and_reloads() {
        let (headers_a, tip_a) = build_test_chain(5, &[PUBKEY_G]);

        // Initial load of chain A
        let mut mock = MockChainStoreHandle::default();
        let headers_a_clone = headers_a.clone();
        mock.expect_get_chain_tip().returning(move || Ok(tip_a));
        mock.expect_get_block_metadata()
            .returning(move |_| Ok(metadata_at_height(4)));
        mock.expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(headers_a_clone.clone()));
        mock.expect_get_share_headers()
            .returning(|_| Ok(Vec::new()));

        let mut window = PplnsWindow::new(TEST_NETWORK);
        window.update(&mock).unwrap();
        assert_eq!(window.confirmed_entries.len(), 5);

        // Reorg: different chain B at same heights
        let (headers_b, tip_b) = build_test_chain(5, &[PUBKEY_2G]);
        let different_hash_at_4 = headers_b[0].blockhash;

        let mut mock2 = MockChainStoreHandle::default();
        let headers_b_clone = headers_b.clone();
        mock2.expect_get_chain_tip().returning(move || Ok(tip_b));
        mock2
            .expect_get_block_metadata()
            .returning(move |_| Ok(metadata_at_height(4)));
        mock2
            .expect_get_confirmed_at_height()
            .returning(move |_| Ok(different_hash_at_4));
        mock2
            .expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(headers_b_clone.clone()));
        mock2
            .expect_get_share_headers()
            .returning(|_| Ok(Vec::new()));

        let updated = window.update(&mock2).unwrap();
        assert!(updated);
        assert_eq!(window.confirmed_entries.len(), 5);
        assert_eq!(window.cached_tip_blockhash, Some(tip_b));
    }

    #[test]
    fn test_count_based_eviction() {
        let genesis_hash = BlockHash::all_zeros();
        let header_a = build_test_header(&genesis_hash.to_string(), PUBKEY_G, 2);
        let header_b = build_test_header(&header_a.block_hash().to_string(), PUBKEY_2G, 2);
        let header_c = build_test_header(&header_b.block_hash().to_string(), PUBKEY_3G, 2);

        let mut window = PplnsWindow::new(TEST_NETWORK);
        // Newest at front: c, b, a
        let entry_c = entry_from_header(&mut window, &header_c, 2);
        let entry_b = entry_from_header(&mut window, &header_b, 1);
        let entry_a = entry_from_header(&mut window, &header_a, 0);
        window.confirmed_entries.push_back(entry_c);
        window.confirmed_entries.push_back(entry_b);
        window.confirmed_entries.push_back(entry_a);

        // With 3 entries and PPLNS_WINDOW_SHARES >> 3, no eviction occurs
        window.evict_overflow();
        assert_eq!(window.confirmed_entries.len(), 3);
    }

    #[test]
    fn test_count_based_eviction_truncates_oldest() {
        let genesis_hash = BlockHash::all_zeros();
        let header_a = build_test_header(&genesis_hash.to_string(), PUBKEY_G, 2);
        let hash_a = header_a.block_hash();
        let header_b = build_test_header(&header_a.block_hash().to_string(), PUBKEY_2G, 2);
        let hash_b = header_b.block_hash();

        // Injected small window cap so eviction runs without a 120,960-entry
        // fixture. Eviction retains cache_capacity() = cap + ~1% buffer.
        const CAP: usize = 200;
        let mut window = PplnsWindow::new_with_window_sizes(TEST_NETWORK, 0, CAP);
        let capacity = window.cache_capacity();

        // Newest-to-oldest at the front: header_b, header_a, then padding.
        let entry_b = entry_from_header(&mut window, &header_b, 1);
        window.confirmed_entries.push_back(entry_b);
        let entry_a = entry_from_header(&mut window, &header_a, 0);
        window.confirmed_entries.push_back(entry_a);

        // Pad so the total exceeds the retained capacity (window cap + buffer).
        let padding_address = header_a.miner_bitcoin_address.clone();
        for index in 0..capacity {
            let entry = window.build_confirmed_entry(
                BlockHash::all_zeros(),
                index as u32 + 2,
                padding_address.clone(),
                header_a.miner_address,
                1u128,
                Vec::new(),
            );
            window.confirmed_entries.push_back(entry);
        }

        assert_eq!(window.confirmed_entries.len(), capacity + 2);

        window.evict_overflow();

        // Truncated to the retained capacity, dropping the 2 oldest from the back.
        assert_eq!(window.confirmed_entries.len(), capacity);
        // The newest entries (header_b, header_a) at front should still be present.
        assert_eq!(window.confirmed_entries[0].blockhash, hash_b);
        assert_eq!(window.confirmed_entries[1].blockhash, hash_a);
    }

    #[test]
    fn test_distribution_from_tip_includes_all_entries() {
        let genesis_hash = BlockHash::all_zeros();
        let header1 = build_test_header(&genesis_hash.to_string(), PUBKEY_G, 2);
        let header2 = build_test_header(&header1.block_hash().to_string(), PUBKEY_2G, 2);
        let difficulty1 = header1.get_difficulty(TEST_NETWORK);
        let difficulty2 = header2.get_difficulty(TEST_NETWORK);

        let mut window = PplnsWindow::new(TEST_NETWORK);
        let entry2 = entry_from_header(&mut window, &header2, 1);
        let entry1 = entry_from_header(&mut window, &header1, 0);
        window.confirmed_entries.push_back(entry2);
        window.confirmed_entries.push_back(entry1);

        // Anchoring on the tip (header2 at the front) walks the whole window.
        let result = window
            .get_distribution_from_start_hash(
                header2.block_hash(),
                BITCOIN_BLOCK_DIFFICULTY.into(),
                &MockChainStoreHandle::default(),
            )
            .expect("tip should be in window");

        assert_eq!(result.len(), 2);
        assert_eq!(
            result[&header1.miner_bitcoin_address],
            difficulty1 * DIFFICULTY_SCALE
        );
        assert_eq!(
            result[&header2.miner_bitcoin_address],
            difficulty2 * DIFFICULTY_SCALE
        );
    }

    #[test]
    fn test_get_distribution_with_start_hash() {
        let genesis_hash = BlockHash::all_zeros();
        let header1 = build_test_header(&genesis_hash.to_string(), PUBKEY_G, 2);
        let header2 = build_test_header(&header1.block_hash().to_string(), PUBKEY_2G, 2);
        let header3 = build_test_header(&header2.block_hash().to_string(), PUBKEY_3G, 2);
        let difficulty1 = header1.get_difficulty(TEST_NETWORK);
        let difficulty2 = header2.get_difficulty(TEST_NETWORK);

        // Entries are newest-to-oldest: header3 (index 0), header2 (index 1), header1 (index 2)
        let mut window = PplnsWindow::new(TEST_NETWORK);
        let entry3 = entry_from_header(&mut window, &header3, 2);
        let entry2 = entry_from_header(&mut window, &header2, 1);
        let entry1 = entry_from_header(&mut window, &header1, 0);
        window.confirmed_entries.push_back(entry3);
        window.confirmed_entries.push_back(entry2);
        window.confirmed_entries.push_back(entry1);

        // Starting from header2 should skip header3, include header2 and header1
        let result = window
            .get_distribution_from_start_hash(
                header2.block_hash(),
                BITCOIN_BLOCK_DIFFICULTY.into(),
                &MockChainStoreHandle::default(),
            )
            .expect("header2 should be in window");
        assert_eq!(result.len(), 2);
        assert_eq!(
            result[&header2.miner_bitcoin_address],
            difficulty2 * DIFFICULTY_SCALE
        );
        assert_eq!(
            result[&header1.miner_bitcoin_address],
            difficulty1 * DIFFICULTY_SCALE
        );
        assert!(!result.contains_key(&header3.miner_bitcoin_address));

        // Starting from the oldest entry should only include that entry
        let result = window
            .get_distribution_from_start_hash(
                header1.block_hash(),
                BITCOIN_BLOCK_DIFFICULTY.into(),
                &MockChainStoreHandle::default(),
            )
            .expect("header1 should be in window");
        assert_eq!(result.len(), 1);
        assert_eq!(
            result[&header1.miner_bitcoin_address],
            difficulty1 * DIFFICULTY_SCALE
        );
    }

    #[test]
    fn test_get_distribution_with_unknown_start_hash_returns_error() {
        let genesis_hash = BlockHash::all_zeros();
        let header1 = build_test_header(&genesis_hash.to_string(), PUBKEY_G, 2);
        let header2 = build_test_header(&header1.block_hash().to_string(), PUBKEY_2G, 2);

        let mut window = PplnsWindow::new(TEST_NETWORK);
        let entry2 = entry_from_header(&mut window, &header2, 1);
        let entry1 = entry_from_header(&mut window, &header1, 0);
        window.confirmed_entries.push_back(entry2);
        window.confirmed_entries.push_back(entry1);

        // A hash not in the window and not in the store cannot be resolved
        // to a confirmed ancestor, so the walk errors rather than silently
        // producing an empty distribution.
        let unknown_hash = build_test_header(&genesis_hash.to_string(), PUBKEY_3G, 3).block_hash();
        let mut mock_store = MockChainStoreHandle::default();
        mock_store.expect_get_share_header().returning(|_| {
            Err(crate::store::writer::StoreError::NotFound(
                "not found".into(),
            ))
        });
        let result = window.get_distribution_from_start_hash(
            unknown_hash,
            BITCOIN_BLOCK_DIFFICULTY.into(),
            &mock_store,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_uncle_weighting() {
        let genesis_hash = BlockHash::all_zeros();

        // Uncle header
        let uncle_header = build_test_header(&genesis_hash.to_string(), PUBKEY_3G, 2);
        let uncle_hash = uncle_header.block_hash();
        let uncle_difficulty = uncle_header.get_difficulty(TEST_NETWORK);

        // Nephew that references the uncle
        let nephew_header =
            build_test_header_with_uncles(&genesis_hash.to_string(), PUBKEY_G, 2, vec![uncle_hash]);
        let nephew_difficulty = nephew_header.get_difficulty(TEST_NETWORK);

        let mut window = PplnsWindow::new(TEST_NETWORK);
        let uncle_entry = uncle_entry_from_header(&mut window, &uncle_header);
        let nephew_entry =
            entry_from_header_with_uncles(&mut window, &nephew_header, 0, vec![uncle_entry]);
        window.confirmed_entries.push_back(nephew_entry);

        let result = window
            .get_distribution_from_start_hash(
                nephew_header.block_hash(),
                BITCOIN_BLOCK_DIFFICULTY.into(),
                &MockChainStoreHandle::default(),
            )
            .expect("nephew should be in window");

        // Uncle gets UNCLE_SCALED_WEIGHT (9) times its difficulty
        let expected_uncle_weight = uncle_difficulty * UNCLE_SCALED_WEIGHT;
        assert_eq!(
            result[&uncle_header.miner_bitcoin_address],
            expected_uncle_weight
        );

        // Nephew gets base difficulty * DIFFICULTY_SCALE + uncle_difficulty * NEPHEW_SCALED_BONUS
        let expected_nephew_weight =
            nephew_difficulty * DIFFICULTY_SCALE + uncle_difficulty * NEPHEW_SCALED_BONUS;
        assert_eq!(
            result[&nephew_header.miner_bitcoin_address],
            expected_nephew_weight
        );
    }

    /// The window credits every uncle reference it walks, so an uncle that
    /// two nephews on one chain both name is paid twice, and each nephew gets
    /// its bonus. This is why an uncle may have only one nephew on a chain:
    /// `validate_uncles` rejects the second nephew, so no valid chain holds
    /// this shape.
    #[test]
    fn test_window_pays_an_uncle_once_for_each_nephew_naming_it() {
        let genesis_hash = BlockHash::all_zeros();
        let header_one = build_test_header(&genesis_hash.to_string(), PUBKEY_2G, 2);
        let uncle_header = build_test_header(&genesis_hash.to_string(), PUBKEY_3G, 2);
        let uncle_difficulty = uncle_header.get_difficulty(TEST_NETWORK);
        let first_nephew_header = build_test_header_with_uncles(
            &header_one.block_hash().to_string(),
            PUBKEY_G,
            2,
            vec![uncle_header.block_hash()],
        );
        let second_nephew_header = build_test_header_with_uncles(
            &first_nephew_header.block_hash().to_string(),
            PUBKEY_4G,
            2,
            vec![uncle_header.block_hash()],
        );
        let second_nephew_difficulty = second_nephew_header.get_difficulty(TEST_NETWORK);

        let mut window = PplnsWindow::new(TEST_NETWORK);
        let first_uncle_entry = uncle_entry_from_header(&mut window, &uncle_header);
        let second_uncle_entry = uncle_entry_from_header(&mut window, &uncle_header);
        let second_nephew_entry = entry_from_header_with_uncles(
            &mut window,
            &second_nephew_header,
            2,
            vec![second_uncle_entry],
        );
        let first_nephew_entry = entry_from_header_with_uncles(
            &mut window,
            &first_nephew_header,
            1,
            vec![first_uncle_entry],
        );
        let entry_one = entry_from_header(&mut window, &header_one, 0);
        window.confirmed_entries.push_back(second_nephew_entry);
        window.confirmed_entries.push_back(first_nephew_entry);
        window.confirmed_entries.push_back(entry_one);

        let distribution = window
            .get_distribution_from_start_hash(
                second_nephew_header.block_hash(),
                BITCOIN_BLOCK_DIFFICULTY.into(),
                &MockChainStoreHandle::default(),
            )
            .expect("the anchor is in the window");

        assert_eq!(
            distribution[&uncle_header.miner_bitcoin_address],
            2 * uncle_difficulty * UNCLE_SCALED_WEIGHT
        );
        assert_eq!(
            distribution[&second_nephew_header.miner_bitcoin_address],
            second_nephew_difficulty * DIFFICULTY_SCALE + uncle_difficulty * NEPHEW_SCALED_BONUS
        );
    }

    #[test]
    fn test_empty_chain() {
        let genesis_hash = BlockHash::all_zeros();

        let mut mock = MockChainStoreHandle::default();
        mock.expect_get_chain_tip()
            .returning(move || Ok(genesis_hash));
        mock.expect_get_block_metadata()
            .returning(move |_| Ok(metadata_no_height()));

        let mut window = PplnsWindow::new(TEST_NETWORK);
        let updated = window.update(&mock).unwrap();

        assert!(!updated);
        assert!(window.is_empty());
    }

    #[test]
    fn test_reorg_to_shorter_chain() {
        let (headers, tip_hash) = build_test_chain(5, &[PUBKEY_G]);

        // Initial load at height 4
        let mut mock = MockChainStoreHandle::default();
        let headers_clone = headers.clone();
        mock.expect_get_chain_tip().returning(move || Ok(tip_hash));
        mock.expect_get_block_metadata()
            .returning(move |_| Ok(metadata_at_height(4)));
        mock.expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(headers_clone.clone()));
        mock.expect_get_share_headers()
            .returning(|_| Ok(Vec::new()));

        let mut window = PplnsWindow::new(TEST_NETWORK);
        window.update(&mock).unwrap();
        assert_eq!(window.cached_top_height, Some(4));

        // Reorg to shorter chain at height 2
        let (short_headers, short_tip) = build_test_chain(3, &[PUBKEY_2G]);

        let mut mock2 = MockChainStoreHandle::default();
        let short_headers_clone = short_headers.clone();
        let short_headers_for_lookup = short_headers.clone();
        mock2
            .expect_get_chain_tip()
            .returning(move || Ok(short_tip));
        mock2
            .expect_get_block_metadata()
            .returning(move |_| Ok(metadata_at_height(2)));
        // find_fork_height queries confirmed_at_height for cached entries;
        // the short chain has different blockhashes, so return those (no match).
        // Heights above the short chain's tip return an error.
        mock2
            .expect_get_confirmed_at_height()
            .returning(move |height| {
                let max_height = short_headers_for_lookup.len() as u32 - 1;
                if height > max_height {
                    return Err(crate::store::writer::StoreError::NotFound(
                        "height not found".into(),
                    ));
                }
                let index = (max_height - height) as usize;
                Ok(short_headers_for_lookup[index].blockhash)
            });
        mock2
            .expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(short_headers_clone.clone()));
        mock2
            .expect_get_share_headers()
            .returning(|_| Ok(Vec::new()));

        let updated = window.update(&mock2).unwrap();
        assert!(updated);
        assert_eq!(window.confirmed_entries.len(), 3);
        assert_eq!(window.cached_top_height, Some(2));
        assert_eq!(
            window.cached_tip_blockhash,
            Some(short_headers[0].blockhash)
        );
    }

    #[test]
    fn test_uncle_data_is_evicted_when_entry_is_evicted() {
        let genesis_hash = BlockHash::all_zeros();

        let uncle_header = build_test_header(&genesis_hash.to_string(), PUBKEY_3G, 2);
        let uncle_hash = uncle_header.block_hash();

        let nephew1 =
            build_test_header_with_uncles(&genesis_hash.to_string(), PUBKEY_G, 2, vec![uncle_hash]);

        let nephew2 = build_test_header_with_uncles(
            &nephew1.block_hash().to_string(),
            PUBKEY_2G,
            2,
            vec![uncle_hash],
        );

        let mut window = PplnsWindow::new(TEST_NETWORK);
        let uncle_entry_1 = uncle_entry_from_header(&mut window, &uncle_header);
        let entry_nephew1 =
            entry_from_header_with_uncles(&mut window, &nephew1, 1, vec![uncle_entry_1]);
        window.confirmed_entries.push_back(entry_nephew1);

        let uncle_entry_2 = uncle_entry_from_header(&mut window, &uncle_header);
        let entry_nephew2 =
            entry_from_header_with_uncles(&mut window, &nephew2, 0, vec![uncle_entry_2]);
        window.confirmed_entries.push_back(entry_nephew2);

        assert_eq!(window.confirmed_entries.len(), 2);
        assert_eq!(window.confirmed_entries[0].uncle_entries.len(), 1);
        assert_eq!(window.confirmed_entries[1].uncle_entries.len(), 1);

        // Truncate to keep only nephew1; nephew2's uncle data is dropped with it
        window.confirmed_entries.truncate(1);
        assert_eq!(window.confirmed_entries.len(), 1);
        assert_eq!(window.confirmed_entries[0].uncle_entries.len(), 1);

        // Clear everything
        window.confirmed_entries.clear();
        assert!(window.confirmed_entries.is_empty());
    }

    #[test]
    fn test_shallow_reorg_preserves_older_entries() {
        // Build a chain of 5 shares (heights 0-4), load into window.
        // Then reorg last 2 (heights 3-4) with different shares.
        // Heights 0-2 should be preserved, heights 3-4 replaced.
        let (headers_a, tip_a) = build_test_chain(5, &[PUBKEY_G]);

        // Initial load
        let mut mock = MockChainStoreHandle::default();
        let headers_a_clone = headers_a.clone();
        mock.expect_get_chain_tip().returning(move || Ok(tip_a));
        mock.expect_get_block_metadata()
            .returning(move |_| Ok(metadata_at_height(4)));
        mock.expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(headers_a_clone.clone()));
        mock.expect_get_share_headers()
            .returning(|_| Ok(Vec::new()));

        let mut window = PplnsWindow::new(TEST_NETWORK);
        window.update(&mock).unwrap();
        assert_eq!(window.confirmed_entries.len(), 5);

        // Save the blockhashes for heights 0-2 (they should survive the reorg)
        // In the deque, front=newest: [height4, height3, height2, height1, height0]
        let preserved_hash_2 = window.confirmed_entries[2].blockhash;
        let preserved_hash_1 = window.confirmed_entries[3].blockhash;
        let preserved_hash_0 = window.confirmed_entries[4].blockhash;

        // Build a new fork: 2 new shares at heights 3-4 with different miner
        let fork_parent_hash = headers_a[2].blockhash;
        let fork_header_3 = build_test_header(&fork_parent_hash.to_string(), PUBKEY_2G, 2);
        let fork_hash_3 = fork_header_3.block_hash();
        let fork_header_4 = build_test_header(&fork_hash_3.to_string(), PUBKEY_2G, 2);
        let fork_hash_4 = fork_header_4.block_hash();

        // New headers for the fork (newest-to-oldest)
        let new_fork_headers = vec![
            ConfirmedHeaderResult {
                height: 4,
                blockhash: fork_hash_4,
                header: fork_header_4,
            },
            ConfirmedHeaderResult {
                height: 3,
                blockhash: fork_hash_3,
                header: fork_header_3,
            },
        ];

        let mut mock2 = MockChainStoreHandle::default();
        let new_fork_clone = new_fork_headers.clone();
        mock2
            .expect_get_chain_tip()
            .returning(move || Ok(fork_hash_4));
        mock2
            .expect_get_block_metadata()
            .returning(move |_| Ok(metadata_at_height(4)));

        // is_simple_extension checks height 4 -- return the new (different) hash
        // find_fork_height walks entries: height 4 (no match), height 3 (no match),
        // height 2 (match!). Return the preserved hash at height 2.
        let preserved_2_copy = preserved_hash_2;
        mock2
            .expect_get_confirmed_at_height()
            .returning(move |height| match height {
                4 => Ok(fork_hash_4),
                3 => Ok(fork_hash_3),
                2 => Ok(preserved_2_copy),
                _ => Err(crate::store::writer::StoreError::NotFound(
                    "not needed".into(),
                )),
            });

        // load_range(3, 4) fetches the fork headers
        mock2
            .expect_get_confirmed_headers_in_range()
            .withf(|from, to| *from == 3 && *to == 4)
            .returning(move |_, _| Ok(new_fork_clone.clone()));
        mock2
            .expect_get_share_headers()
            .returning(|_| Ok(Vec::new()));

        let updated = window.update(&mock2).unwrap();
        assert!(updated);
        assert_eq!(window.confirmed_entries.len(), 5);

        // Verify heights 0-2 are preserved (at back of deque)
        assert_eq!(window.confirmed_entries[2].blockhash, preserved_hash_2);
        assert_eq!(window.confirmed_entries[3].blockhash, preserved_hash_1);
        assert_eq!(window.confirmed_entries[4].blockhash, preserved_hash_0);

        // Verify heights 3-4 are the new fork entries (at front of deque)
        assert_eq!(window.confirmed_entries[0].blockhash, fork_hash_4);
        assert_eq!(window.confirmed_entries[1].blockhash, fork_hash_3);
    }

    #[test]
    fn test_reorg_updates_distribution_correctly() {
        // After a shallow reorg, address_difficulty_map should reflect
        // only the surviving + new entries.
        let (headers_a, tip_a) = build_test_chain(3, &[PUBKEY_G]);

        // Initial load: 3 shares all by PUBKEY_G
        let mut mock = MockChainStoreHandle::default();
        let headers_a_clone = headers_a.clone();
        mock.expect_get_chain_tip().returning(move || Ok(tip_a));
        mock.expect_get_block_metadata()
            .returning(move |_| Ok(metadata_at_height(2)));
        mock.expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(headers_a_clone.clone()));
        mock.expect_get_share_headers()
            .returning(|_| Ok(Vec::new()));

        let mut window = PplnsWindow::new(TEST_NETWORK);
        window.update(&mock).unwrap();

        let difficulty = headers_a[0].header.get_difficulty(TEST_NETWORK);
        let miner_g = &headers_a[0].header.miner_bitcoin_address;

        // All 3 shares are by PUBKEY_G
        let dist = window
            .get_distribution_from_start_hash(
                tip_a,
                BITCOIN_BLOCK_DIFFICULTY.into(),
                &MockChainStoreHandle::default(),
            )
            .expect("tip should be in window");
        assert_eq!(dist.len(), 1);
        assert_eq!(dist[miner_g], 3 * difficulty * DIFFICULTY_SCALE);

        // Reorg: replace height 2 with a share by PUBKEY_2G
        let preserved_hash_at_1 = headers_a[1].blockhash; // height 1

        let fork_header = build_test_header(&preserved_hash_at_1.to_string(), PUBKEY_2G, 2);
        let fork_hash = fork_header.block_hash();
        let fork_headers = vec![ConfirmedHeaderResult {
            height: 2,
            blockhash: fork_hash,
            header: fork_header.clone(),
        }];

        let mut mock2 = MockChainStoreHandle::default();
        let fork_headers_clone = fork_headers.clone();
        mock2
            .expect_get_chain_tip()
            .returning(move || Ok(fork_hash));
        mock2
            .expect_get_block_metadata()
            .returning(move |_| Ok(metadata_at_height(2)));

        let preserved_1 = preserved_hash_at_1;
        mock2
            .expect_get_confirmed_at_height()
            .returning(move |height| match height {
                2 => Ok(fork_hash),
                1 => Ok(preserved_1),
                _ => Err(crate::store::writer::StoreError::NotFound(
                    "not needed".into(),
                )),
            });

        mock2
            .expect_get_confirmed_headers_in_range()
            .withf(|from, to| *from == 2 && *to == 2)
            .returning(move |_, _| Ok(fork_headers_clone.clone()));
        mock2
            .expect_get_share_headers()
            .returning(|_| Ok(Vec::new()));

        window.update(&mock2).unwrap();

        let miner_2g = &fork_header.miner_bitcoin_address;

        // Now: 2 shares by PUBKEY_G (heights 0-1) + 1 share by PUBKEY_2G (height 2)
        let dist = window
            .get_distribution_from_start_hash(
                fork_hash,
                BITCOIN_BLOCK_DIFFICULTY.into(),
                &MockChainStoreHandle::default(),
            )
            .expect("fork tip should be in window");
        assert_eq!(dist.len(), 2);
        assert_eq!(
            dist[miner_g],
            2 * difficulty * DIFFICULTY_SCALE,
            "PUBKEY_G should have 2x difficulty scaled, got {}",
            dist[miner_g]
        );
        assert_eq!(
            dist[miner_2g],
            difficulty * DIFFICULTY_SCALE,
            "PUBKEY_2G should have 1x difficulty scaled, got {}",
            dist[miner_2g]
        );
    }

    #[test]
    fn test_confirmed_entry_has_correct_height() {
        let (headers, tip_hash) = build_test_chain(5, &[PUBKEY_G, PUBKEY_2G]);

        let mut mock = MockChainStoreHandle::default();
        let headers_clone = headers.clone();
        mock.expect_get_chain_tip().returning(move || Ok(tip_hash));
        mock.expect_get_block_metadata()
            .returning(move |_| Ok(metadata_at_height(4)));
        mock.expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(headers_clone.clone()));
        mock.expect_get_share_headers()
            .returning(|_| Ok(Vec::new()));

        let mut window = PplnsWindow::new(TEST_NETWORK);
        window.update(&mock).unwrap();

        // Deque is newest-to-oldest: [height4, height3, height2, height1, height0]
        assert_eq!(window.confirmed_entries[0].height, 4);
        assert_eq!(window.confirmed_entries[1].height, 3);
        assert_eq!(window.confirmed_entries[2].height, 2);
        assert_eq!(window.confirmed_entries[3].height, 1);
        assert_eq!(window.confirmed_entries[4].height, 0);
    }

    #[test]
    fn test_stale_address_key_removed_after_entry_evicted() {
        let genesis_hash = BlockHash::all_zeros();

        // Miner A at height 0, Miner B at height 1
        let header_a = build_test_header(&genesis_hash.to_string(), PUBKEY_G, 2);
        let header_b = build_test_header(&header_a.block_hash().to_string(), PUBKEY_2G, 2);

        let mut window = PplnsWindow::new(TEST_NETWORK);
        // entry_b built first -> gets key 0, entry_a built second -> gets key 1
        let entry_b = entry_from_header(&mut window, &header_b, 1);
        let entry_a = entry_from_header(&mut window, &header_a, 0);
        let miner_b_key = entry_b.internal_key;
        let miner_a_key = entry_a.internal_key;
        window.confirmed_entries.push_back(entry_b);
        window.confirmed_entries.push_back(entry_a);

        assert_eq!(window.address_keys.len(), 2);
        assert!(window.address_keys.value_for(miner_a_key).is_some());
        assert!(window.address_keys.value_for(miner_b_key).is_some());

        // Evict miner A's entry (oldest, at back)
        window.confirmed_entries.pop_back();

        // Pruning should free miner A's now-unreferenced key.
        window.prune_unreferenced_keys();

        assert!(
            window.address_keys.value_for(miner_a_key).is_none(),
            "stale key for evicted miner should be removed"
        );
        assert!(
            window.address_keys.value_for(miner_b_key).is_some(),
            "active miner key should be preserved"
        );
    }

    #[test]
    fn test_prune_preserves_keys_of_all_cached_entries() {
        let genesis_hash = BlockHash::all_zeros();

        // Three miners: A (oldest), B (middle), C (newest)
        let header_a = build_test_header(&genesis_hash.to_string(), PUBKEY_G, 2);
        let header_b = build_test_header(&header_a.block_hash().to_string(), PUBKEY_2G, 2);
        let header_c = build_test_header(&header_b.block_hash().to_string(), PUBKEY_3G, 2);

        let mut window = PplnsWindow::new(TEST_NETWORK);
        let entry_c = entry_from_header(&mut window, &header_c, 2);
        let entry_b = entry_from_header(&mut window, &header_b, 1);
        let entry_a = entry_from_header(&mut window, &header_a, 0);
        window.confirmed_entries.push_back(entry_c);
        window.confirmed_entries.push_back(entry_b);
        window.confirmed_entries.push_back(entry_a);

        assert_eq!(window.address_keys.len(), 3);

        // Nothing has left the cache, so pruning keeps every key -- retention
        // depends only on whether an entry references the key, not on whether
        // the entry falls inside the window.
        window.prune_unreferenced_keys();

        assert!(
            window.address_keys.value_for(0).is_some(),
            "cached miner key should be preserved"
        );
        assert!(
            window.address_keys.value_for(1).is_some(),
            "cached miner key should be preserved"
        );
        assert!(
            window.address_keys.value_for(2).is_some(),
            "cached miner key should be preserved"
        );
    }

    #[test]
    fn test_stale_key_slot_reused_by_new_address() {
        let genesis_hash = BlockHash::all_zeros();

        // Miner A at height 0, Miner B at height 1
        let header_a = build_test_header(&genesis_hash.to_string(), PUBKEY_G, 2);
        let header_b = build_test_header(&header_a.block_hash().to_string(), PUBKEY_2G, 2);

        let mut window = PplnsWindow::new(TEST_NETWORK);
        // entry_b built first -> gets key 0, entry_a built second -> gets key 1
        let entry_b = entry_from_header(&mut window, &header_b, 1);
        let entry_a = entry_from_header(&mut window, &header_a, 0);
        let miner_a_key = entry_a.internal_key;
        window.confirmed_entries.push_back(entry_b);
        window.confirmed_entries.push_back(entry_a);

        assert_eq!(window.address_keys.len(), 2);

        // Evict miner A, then trigger cleanup
        window.confirmed_entries.pop_back();
        window.prune_unreferenced_keys();

        assert!(
            window.address_keys.value_for(miner_a_key).is_none(),
            "stale key should be removed after cleanup"
        );

        // Add a new entry with miner C; it should reuse miner A's freed slot
        let header_c = build_test_header(&header_b.block_hash().to_string(), PUBKEY_3G, 2);
        let entry_c = entry_from_header(&mut window, &header_c, 2);
        let miner_c_key = entry_c.internal_key;
        window.confirmed_entries.push_front(entry_c);

        // Miner C should have taken the freed slot
        assert_eq!(
            miner_c_key, miner_a_key,
            "new miner should reuse the freed key slot"
        );
        assert_eq!(
            window.address_keys.value_for(miner_c_key),
            Some(&header_c.miner_bitcoin_address),
            "freed slot should now hold the new miner address"
        );
        // Total slots should still be 2, not grown to 3
        assert_eq!(
            window.address_keys.len(),
            2,
            "address keys should not grow when freed slots are available"
        );
    }

    #[test]
    fn test_prune_preserves_uncle_key_of_cached_entry() {
        let genesis_hash = BlockHash::all_zeros();

        // Uncle miner (PUBKEY_3G) appears only as an uncle, never as a share miner.
        let uncle_header = build_test_header(&genesis_hash.to_string(), PUBKEY_3G, 2);
        let uncle_hash = uncle_header.block_hash();

        // Nephew at height 0 references the uncle.
        let nephew_header =
            build_test_header_with_uncles(&genesis_hash.to_string(), PUBKEY_G, 2, vec![uncle_hash]);

        // A second entry at height 1 by a different miner.
        let header_top = build_test_header(&nephew_header.block_hash().to_string(), PUBKEY_2G, 2);

        let mut window = PplnsWindow::new(TEST_NETWORK);
        // Uncle built first -> PUBKEY_3G gets key 0
        let uncle_entry = uncle_entry_from_header(&mut window, &uncle_header);
        let uncle_miner_key = uncle_entry.internal_key;
        let entry_top = entry_from_header(&mut window, &header_top, 1);
        let entry_nephew =
            entry_from_header_with_uncles(&mut window, &nephew_header, 0, vec![uncle_entry]);

        window.confirmed_entries.push_back(entry_top);
        window.confirmed_entries.push_back(entry_nephew);

        // The nephew is still cached, so pruning must keep its uncle's key even
        // though that miner never appears as a share miner.
        window.prune_unreferenced_keys();

        assert!(
            window.address_keys.value_for(uncle_miner_key).is_some(),
            "uncle miner key referenced by a cached nephew should be preserved"
        );
    }

    #[test]
    fn test_update_prunes_key_of_evicted_only_miner() {
        // Height 0 is mined by a unique miner (PUBKEY_5G); heights 1-4 by
        // PUBKEY_G. With a small window cap, the initial load pulls all five
        // then evicts the oldest, dropping height 0. update() must then prune
        // PUBKEY_5G's now-unreferenced key while keeping PUBKEY_G's.
        let (headers, tip_hash) =
            build_test_chain(5, &[PUBKEY_5G, PUBKEY_G, PUBKEY_G, PUBKEY_G, PUBKEY_G]);
        // headers are newest-to-oldest; height 0 (PUBKEY_5G) is at the back.
        let evicted_only_miner = headers[4].header.miner_bitcoin_address.clone();
        let active_miner = headers[0].header.miner_bitcoin_address.clone();

        let mut mock = MockChainStoreHandle::default();
        let headers_clone = headers.clone();
        mock.expect_get_chain_tip().returning(move || Ok(tip_hash));
        mock.expect_get_block_metadata()
            .returning(move |_| Ok(metadata_at_height(4)));
        mock.expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(headers_clone.clone()));
        mock.expect_get_share_headers()
            .returning(|_| Ok(Vec::new()));

        // cache_capacity for cap 2 is 2 + ceil(2/100) = 3, so heights 0 and 1
        // are evicted, leaving heights 2-4 (all PUBKEY_G).
        let mut window = PplnsWindow::new_with_window_sizes(TEST_NETWORK, 0, 2);
        window.update(&mock).unwrap();

        assert_eq!(window.confirmed_entries.len(), 3);

        let distribution = window
            .get_distribution_from_start_hash(
                tip_hash,
                BITCOIN_BLOCK_DIFFICULTY.into(),
                &MockChainStoreHandle::default(),
            )
            .expect("tip should be in window");
        assert!(
            !distribution.contains_key(&evicted_only_miner),
            "miner present only in evicted entries should not be paid"
        );
        assert!(
            distribution.contains_key(&active_miner),
            "retained miner should be paid"
        );

        // The interner slot for the evicted-only miner must be freed; the
        // active miner's slot must survive.
        let mut freed_evicted_miner = true;
        let mut kept_active_miner = false;
        for index in 0..window.address_keys.len() {
            match window.address_keys.value_for(index) {
                Some(address) if *address == evicted_only_miner => freed_evicted_miner = false,
                Some(address) if *address == active_miner => kept_active_miner = true,
                _ => {}
            }
        }
        assert!(
            freed_evicted_miner,
            "evicted-only miner key should be pruned from the interner"
        );
        assert!(
            kept_active_miner,
            "active miner key should be retained in the interner"
        );
    }

    #[test]
    fn test_deep_reorg_falls_back_to_full_invalidation() {
        // Build a chain longer than MAX_REORG_SCAN_DEPTH, reorg it fully.
        // find_fork_height should give up after MAX_REORG_SCAN_DEPTH lookups.
        let chain_len = MAX_REORG_SCAN_DEPTH + 10;
        let (headers_a, tip_a) = build_test_chain(chain_len, &[PUBKEY_G]);
        let top_height = (chain_len - 1) as u32;

        // Initial load
        let mut mock = MockChainStoreHandle::default();
        let headers_a_clone = headers_a.clone();
        mock.expect_get_chain_tip().returning(move || Ok(tip_a));
        mock.expect_get_block_metadata()
            .returning(move |_| Ok(metadata_at_height(top_height)));
        mock.expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(headers_a_clone.clone()));
        mock.expect_get_share_headers()
            .returning(|_| Ok(Vec::new()));

        let mut window = PplnsWindow::new(TEST_NETWORK);
        window.update(&mock).unwrap();
        assert_eq!(window.confirmed_entries.len(), chain_len);

        // Reorg with completely different chain at same heights
        let (headers_b, tip_b) = build_test_chain(chain_len, &[PUBKEY_2G]);
        let headers_b_clone = headers_b.clone();
        let headers_b_for_lookup = headers_b.clone();

        let mut mock2 = MockChainStoreHandle::default();
        mock2.expect_get_chain_tip().returning(move || Ok(tip_b));
        mock2
            .expect_get_block_metadata()
            .returning(move |_| Ok(metadata_at_height(top_height)));
        // All heights return different hashes (different chain)
        mock2
            .expect_get_confirmed_at_height()
            .returning(move |height| {
                let index = (top_height - height) as usize;
                if index < headers_b_for_lookup.len() {
                    Ok(headers_b_for_lookup[index].blockhash)
                } else {
                    Err(crate::store::writer::StoreError::NotFound(
                        "not found".into(),
                    ))
                }
            });
        mock2
            .expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(headers_b_clone.clone()));
        mock2
            .expect_get_share_headers()
            .returning(|_| Ok(Vec::new()));

        let updated = window.update(&mock2).unwrap();
        assert!(updated);
        // Full invalidation + reload means all entries are from chain B
        assert_eq!(window.confirmed_entries.len(), chain_len);
        assert_eq!(window.cached_tip_blockhash, Some(tip_b));
        // Verify entries are from chain B (check first entry)
        assert_eq!(
            window.confirmed_entries[0].blockhash,
            headers_b[0].blockhash
        );
    }

    /// When the confirmed chain has share_a at height 3 but the
    /// candidate chain has a competing share_b at height 3 (same
    /// parent), a child of share_b should be able to look up its
    /// parent in the PPLNS window.
    ///
    /// share_b's children can validate as their parent is now in
    /// PPLNS window - thanks to candidate chain being followed to
    /// confirmed entries in pplns window.
    #[test]
    fn test_competing_block_not_in_pplns_window_confirmed_entries_but_is_building_on_candidate_chain()
     {
        let genesis_hash = BlockHash::all_zeros();

        // Build confirmed chain: genesis -> share1(h:0) -> share2(h:1) -> share_a(h:2)
        let share1 = build_test_header(&genesis_hash.to_string(), PUBKEY_G, 2);
        let share2 = build_test_header(&share1.block_hash().to_string(), PUBKEY_2G, 3);
        let share_a = build_test_header(&share2.block_hash().to_string(), PUBKEY_G, 4);

        // share_b is a competing block at same height as share_a
        // (same parent share2, different miner)
        let share_b = build_test_header(&share2.block_hash().to_string(), PUBKEY_3G, 5);

        // share_c is a child of share_b -- this is the block that
        // needs to validate during sync
        let share_c = build_test_header(&share_b.block_hash().to_string(), PUBKEY_2G, 6);

        // Populate PPLNS window with confirmed entries only (share_a branch)
        let mut window = PplnsWindow::new(TEST_NETWORK);
        let entry_a = entry_from_header(&mut window, &share_a, 2);
        let entry_2 = entry_from_header(&mut window, &share2, 1);
        let entry_1 = entry_from_header(&mut window, &share1, 0);
        window.confirmed_entries.push_back(entry_a);
        window.confirmed_entries.push_back(entry_2);
        window.confirmed_entries.push_back(entry_1);

        // share_a (confirmed) is findable in the window
        let result = window.get_distribution_from_start_hash(
            share_a.block_hash(),
            BITCOIN_BLOCK_DIFFICULTY.into(),
            &MockChainStoreHandle::default(),
        );
        assert!(
            result.is_ok(),
            "share_a should be in the PPLNS window (it is confirmed)"
        );

        // share_b is a competing block at the same height as share_a.
        // Both share the same parent (share2), so share_b's PPLNS
        // distribution should be computable from the confirmed
        // entries below it (share2, share1). A child of share_b
        // must be able to validate by looking up share_b in the
        // PPLNS window through the candiadtes chain.
        //
        // Set up a mock chain store so resolve_start_hash can walk
        // from share_c's prev (share_b) back to share2 (confirmed).
        let mut mock_store = MockChainStoreHandle::default();
        let share_b_clone = share_b.clone();
        let share_b_hash = share_b.block_hash();
        mock_store
            .expect_get_share_header()
            .withf(move |hash| *hash == share_b_hash)
            .returning(move |_| Ok(share_b_clone.clone()));
        // share_b is only HeaderValid and off any chain -- the walk follows
        // parent links regardless of validation status, so it is still
        // included (the old status/chain filter would have rejected it).
        mock_store.expect_get_block_metadata().returning(|_| {
            Ok(BlockMetadata {
                expected_height: Some(1),
                chain_work: Work::from_le_bytes([0u8; 32]),
                status: Status::HeaderValid,
                chain: ChainMembership::None,
            })
        });
        // No uncle headers needed for this test
        mock_store
            .expect_get_share_headers()
            .returning(|_| Ok(Vec::new()));

        assert_eq!(
            share_c.prev_share_blockhash,
            share_b.block_hash(),
            "share_c's parent is share_b"
        );
        let result = window.get_distribution_from_start_hash(
            share_c.prev_share_blockhash,
            BITCOIN_BLOCK_DIFFICULTY.into(),
            &mock_store,
        );
        assert!(
            result.is_ok(),
            "share_c must be able to find its parent share_b in the PPLNS window during sync"
        );
    }

    //* Regression for the testnet4 coinbase-mismatch wedge (hive.log
    //* 2026-08-06). Two siblings at height 197110 shared a parent
    //* (f1455ab0 @197109) and an identical coinbase. The first sibling
    //* confirmed, advancing the tip to 197110; post_promote updated the
    //* window and evict_overflow dropped the oldest share. Validating the
    //* second sibling then computed get_distribution_from_start_hash at the
    //* same parent against a window short by the evicted share, so its
    //* identical coinbase failed "Coinbase and template merkle root don't
    //* match merkle root", and the chain wedged.
    //*
    //* Invariant: the distribution at a fixed anchor must not change when a
    //* sibling promotion advances the tip and triggers eviction. Driven with
    //* an injected window cap of 3 and a bounded total_difficulty (3 shares)
    //* so eviction bites without a 120,960-entry fixture. The ~1% retained
    //* buffer (`cache_capacity`) keeps the anchor's oldest window share alive
    //* across the promotion; without it (evicting at the bare window cap) the
    //* second sibling's identical coinbase would fail validation, as
    //* it did in the testnet4 network with three nodes.
    #[test]
    fn test_distribution_at_anchor_invariant_to_sibling_promotion() {
        const MAX_SHARES: usize = 3;
        let genesis = BlockHash::all_zeros();

        // Confirmed chain a(0) -> b(1) -> c(2) -> parent(3), distinct miners.
        let a = build_test_header(&genesis.to_string(), PUBKEY_G, 2);
        let b = build_test_header(&a.block_hash().to_string(), PUBKEY_2G, 2);
        let c = build_test_header(&b.block_hash().to_string(), PUBKEY_3G, 2);
        let parent = build_test_header(&c.block_hash().to_string(), PUBKEY_4G, 2);
        // Sibling extends the parent; confirming it advances the tip past the
        // anchor and evicts the oldest window share.
        let sibling = build_test_header(&parent.block_hash().to_string(), PUBKEY_5G, 2);
        let parent_hash = parent.block_hash();

        // Window of exactly MAX_SHARES (3) shares: parent, c, b. The ~1%
        // buffer keeps b alive when the sibling promotion evicts the oldest
        // cache entry.
        let mut window = PplnsWindow::new_with_window_sizes(TEST_NETWORK, 0, MAX_SHARES);

        // Update 1: tip = parent (height 3). Loads [parent, c, b, a]; the
        // buffer (cap 3 + 1) retains all four.
        let to_parent = vec![
            ConfirmedHeaderResult {
                height: 3,
                blockhash: parent.block_hash(),
                header: parent.clone(),
            },
            ConfirmedHeaderResult {
                height: 2,
                blockhash: c.block_hash(),
                header: c.clone(),
            },
            ConfirmedHeaderResult {
                height: 1,
                blockhash: b.block_hash(),
                header: b.clone(),
            },
            ConfirmedHeaderResult {
                height: 0,
                blockhash: a.block_hash(),
                header: a.clone(),
            },
        ];
        let parent_tip = parent.block_hash();
        let mut mock1 = MockChainStoreHandle::default();
        mock1
            .expect_get_chain_tip()
            .returning(move || Ok(parent_tip));
        mock1
            .expect_get_block_metadata()
            .returning(|_| Ok(metadata_at_height(3)));
        mock1
            .expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(to_parent.clone()));
        window.update(&mock1).unwrap();

        let before = window
            .get_distribution_from_start_hash(
                parent_hash,
                BITCOIN_BLOCK_DIFFICULTY.into(),
                &MockChainStoreHandle::default(),
            )
            .expect("parent is in the window at the tip");

        // Update 2: tip = sibling (height 4). Simple extension -> loads
        // sibling, evicts the oldest (a) -> cache [sibling, parent, c, b].
        // The buffer keeps b, so parent's 3-share window is intact.
        let to_sibling = vec![ConfirmedHeaderResult {
            height: 4,
            blockhash: sibling.block_hash(),
            header: sibling.clone(),
        }];
        let sibling_tip = sibling.block_hash();
        let parent_at_cached = parent.block_hash();
        let mut mock2 = MockChainStoreHandle::default();
        mock2
            .expect_get_chain_tip()
            .returning(move || Ok(sibling_tip));
        mock2
            .expect_get_block_metadata()
            .returning(|_| Ok(metadata_at_height(4)));
        mock2
            .expect_get_confirmed_at_height()
            .returning(move |_| Ok(parent_at_cached));
        mock2
            .expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(to_sibling.clone()));
        window.update(&mock2).unwrap();

        let after = window
            .get_distribution_from_start_hash(
                parent_hash,
                BITCOIN_BLOCK_DIFFICULTY.into(),
                &MockChainStoreHandle::default(),
            )
            .expect("parent is still in the window after the sibling promotion");

        assert_eq!(
            before, after,
            "distribution at the parent anchor changed after a sibling promotion evicted a \
             window share -- an identical coinbase would fail validation"
        );
    }

    //* The walk must stop at a chain position, not at the back of the cache,
    //* whose position depends on this node's own tip: two nodes one block
    //* apart would otherwise derive different payouts for the same anchor.
    //* The share count stops it at the same chain position on both.
    #[test]
    fn test_distribution_at_anchor_invariant_to_tip() {
        const MAX_SHARES: usize = 3;
        let genesis = BlockHash::all_zeros();

        let a = build_test_header(&genesis.to_string(), PUBKEY_G, 2);
        let b = build_test_header(&a.block_hash().to_string(), PUBKEY_2G, 2);
        let c = build_test_header(&b.block_hash().to_string(), PUBKEY_3G, 2);
        let parent = build_test_header(&c.block_hash().to_string(), PUBKEY_4G, 2);
        let sibling = build_test_header(&parent.block_hash().to_string(), PUBKEY_5G, 2);
        let parent_hash = parent.block_hash();

        let mut window = PplnsWindow::new_with_window_sizes(TEST_NETWORK, 0, MAX_SHARES);

        // Tip = parent (height 3). Cache holds [parent, c, b, a].
        let to_parent = vec![
            ConfirmedHeaderResult {
                height: 3,
                blockhash: parent.block_hash(),
                header: parent.clone(),
            },
            ConfirmedHeaderResult {
                height: 2,
                blockhash: c.block_hash(),
                header: c.clone(),
            },
            ConfirmedHeaderResult {
                height: 1,
                blockhash: b.block_hash(),
                header: b.clone(),
            },
            ConfirmedHeaderResult {
                height: 0,
                blockhash: a.block_hash(),
                header: a.clone(),
            },
        ];
        let parent_tip = parent.block_hash();
        let mut mock1 = MockChainStoreHandle::default();
        mock1
            .expect_get_chain_tip()
            .returning(move || Ok(parent_tip));
        mock1
            .expect_get_block_metadata()
            .returning(|_| Ok(metadata_at_height(3)));
        mock1
            .expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(to_parent.clone()));
        window.update(&mock1).unwrap();

        let before = window
            .get_distribution_from_start_hash(
                parent_hash,
                BITCOIN_BLOCK_DIFFICULTY.into(),
                &MockChainStoreHandle::default(),
            )
            .expect("the anchor has a full window of cached entries below it");

        // Tip = sibling (height 4). Cache becomes [sibling, parent, c, b]:
        // the anchor is now one entry behind the tip and `a` has been evicted.
        let to_sibling = vec![ConfirmedHeaderResult {
            height: 4,
            blockhash: sibling.block_hash(),
            header: sibling.clone(),
        }];
        let sibling_tip = sibling.block_hash();
        let parent_at_cached = parent.block_hash();
        let mut mock2 = MockChainStoreHandle::default();
        mock2
            .expect_get_chain_tip()
            .returning(move || Ok(sibling_tip));
        mock2
            .expect_get_block_metadata()
            .returning(|_| Ok(metadata_at_height(4)));
        mock2
            .expect_get_confirmed_at_height()
            .returning(move |_| Ok(parent_at_cached));
        mock2
            .expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(to_sibling.clone()));
        window.update(&mock2).unwrap();

        let after = window
            .get_distribution_from_start_hash(
                parent_hash,
                BITCOIN_BLOCK_DIFFICULTY.into(),
                &MockChainStoreHandle::default(),
            )
            .expect("the anchor still has a full window of cached entries below it");

        assert_eq!(
            before, after,
            "distribution at a fixed anchor changed as the local tip advanced"
        );
        // Exactly MAX_SHARES entries contributed: parent, c and b, one miner
        // each. `a` is excluded by the count bound, not by where eviction fell.
        assert_eq!(before.len(), MAX_SHARES);
    }

    /// The window is a share count, not a difficulty target: it pays exactly
    /// the last `max_window_shares` shares however small the bitcoin
    /// difficulty of the block being paid for.
    #[test]
    fn test_distribution_pays_exactly_the_last_window_shares() {
        const MAX_SHARES: usize = 3;
        let genesis = BlockHash::all_zeros();
        let a = build_test_header(&genesis.to_string(), PUBKEY_G, 2);
        let b = build_test_header(&a.block_hash().to_string(), PUBKEY_2G, 2);
        let c = build_test_header(&b.block_hash().to_string(), PUBKEY_3G, 2);
        let parent = build_test_header(&c.block_hash().to_string(), PUBKEY_4G, 2);
        let parent_hash = parent.block_hash();
        let cached = vec![
            ConfirmedHeaderResult {
                height: 3,
                blockhash: parent.block_hash(),
                header: parent.clone(),
            },
            ConfirmedHeaderResult {
                height: 2,
                blockhash: c.block_hash(),
                header: c.clone(),
            },
            ConfirmedHeaderResult {
                height: 1,
                blockhash: b.block_hash(),
                header: b.clone(),
            },
            ConfirmedHeaderResult {
                height: 0,
                blockhash: a.block_hash(),
                header: a.clone(),
            },
        ];
        let mut store = MockChainStoreHandle::default();
        store
            .expect_get_chain_tip()
            .returning(move || Ok(parent_hash));
        store
            .expect_get_block_metadata()
            .returning(|_| Ok(metadata_at_height(3)));
        store
            .expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(cached.clone()));
        let mut window = PplnsWindow::new_with_window_sizes(TEST_NETWORK, 0, MAX_SHARES);
        window.update(&store).unwrap();

        let distribution = window
            .get_distribution_from_start_hash(
                parent_hash,
                BITCOIN_BLOCK_DIFFICULTY.into(),
                &MockChainStoreHandle::default(),
            )
            .unwrap();

        // parent, c and b: the last three shares, one miner each; `a` is
        // outside the window.
        assert_eq!(distribution.len(), MAX_SHARES);
        assert!(!distribution.contains_key(&a.miner_bitcoin_address));
    }

    /// A chain shorter than the window pays every share back to genesis.
    #[test]
    fn test_distribution_pays_every_share_of_a_chain_shorter_than_the_window() {
        const MAX_SHARES: usize = 10;
        let genesis = BlockHash::all_zeros();
        let a = build_test_header(&genesis.to_string(), PUBKEY_G, 2);
        let b = build_test_header(&a.block_hash().to_string(), PUBKEY_2G, 2);
        let c = build_test_header(&b.block_hash().to_string(), PUBKEY_3G, 2);
        let parent = build_test_header(&c.block_hash().to_string(), PUBKEY_4G, 2);
        let parent_hash = parent.block_hash();
        let cached = vec![
            ConfirmedHeaderResult {
                height: 3,
                blockhash: parent.block_hash(),
                header: parent.clone(),
            },
            ConfirmedHeaderResult {
                height: 2,
                blockhash: c.block_hash(),
                header: c.clone(),
            },
            ConfirmedHeaderResult {
                height: 1,
                blockhash: b.block_hash(),
                header: b.clone(),
            },
            ConfirmedHeaderResult {
                height: 0,
                blockhash: a.block_hash(),
                header: a.clone(),
            },
        ];
        let mut store = MockChainStoreHandle::default();
        store
            .expect_get_chain_tip()
            .returning(move || Ok(parent_hash));
        store
            .expect_get_block_metadata()
            .returning(|_| Ok(metadata_at_height(3)));
        store
            .expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(cached.clone()));
        let mut window = PplnsWindow::new_with_window_sizes(TEST_NETWORK, 0, MAX_SHARES);
        window.update(&store).unwrap();

        let distribution = window
            .get_distribution_from_start_hash(
                parent_hash,
                BITCOIN_BLOCK_DIFFICULTY.into(),
                &MockChainStoreHandle::default(),
            )
            .unwrap();

        assert_eq!(distribution.len(), 4);
        assert!(distribution.contains_key(&a.miner_bitcoin_address));
    }

    //* An anchor deeper than the retained buffer has fewer than
    //* `max_window_shares` entries cached below it, so the walk runs out. The
    //* result would depend on where eviction trimmed the back, so it is
    //* refused rather than returned truncated.
    #[test]
    fn test_distribution_errors_when_eviction_truncates_the_window() {
        const MAX_SHARES: usize = 3;
        let genesis = BlockHash::all_zeros();

        let a = build_test_header(&genesis.to_string(), PUBKEY_G, 2);
        let b = build_test_header(&a.block_hash().to_string(), PUBKEY_2G, 2);
        let c = build_test_header(&b.block_hash().to_string(), PUBKEY_3G, 2);
        let parent = build_test_header(&c.block_hash().to_string(), PUBKEY_4G, 2);
        let sibling = build_test_header(&parent.block_hash().to_string(), PUBKEY_5G, 2);

        let mut window = PplnsWindow::new_with_window_sizes(TEST_NETWORK, 0, MAX_SHARES);

        let all_headers = vec![
            ConfirmedHeaderResult {
                height: 4,
                blockhash: sibling.block_hash(),
                header: sibling.clone(),
            },
            ConfirmedHeaderResult {
                height: 3,
                blockhash: parent.block_hash(),
                header: parent.clone(),
            },
            ConfirmedHeaderResult {
                height: 2,
                blockhash: c.block_hash(),
                header: c.clone(),
            },
            ConfirmedHeaderResult {
                height: 1,
                blockhash: b.block_hash(),
                header: b.clone(),
            },
            ConfirmedHeaderResult {
                height: 0,
                blockhash: a.block_hash(),
                header: a.clone(),
            },
        ];
        let sibling_tip = sibling.block_hash();
        let mut mock = MockChainStoreHandle::default();
        mock.expect_get_chain_tip()
            .returning(move || Ok(sibling_tip));
        mock.expect_get_block_metadata()
            .returning(|_| Ok(metadata_at_height(4)));
        mock.expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(all_headers.clone()));
        window.update(&mock).unwrap();

        // Cache is capped at MAX_SHARES + 1, so it holds [sibling, parent, c, b]
        // and `a` is evicted: the back no longer reaches the chain start.
        let error = window
            .get_distribution_from_start_hash(
                c.block_hash(),
                BITCOIN_BLOCK_DIFFICULTY.into(),
                &MockChainStoreHandle::default(),
            )
            .expect_err("only two entries are cached below the anchor, short of the window");
        assert!(
            error.to_string().contains("truncated by eviction"),
            "unexpected error: {error}"
        );
    }

    //* Steady state: with a maturity of 3, a share is paid once the paying
    //* share is at least 3 above it. The chain is heights 0..=5 and the
    //* anchor is height 5, so the paying share is at height 6. Heights 5
    //* and 4 (depth 1 and 2) are immature and skipped. Height 3 (depth 3,
    //* the spend maturity depth) is the first paid, and the window of 2
    //* ends at height 2.
    #[test]
    fn test_distribution_skips_immature_shares_from_anchor() {
        let miners = [PUBKEY_G, PUBKEY_2G, PUBKEY_3G, PUBKEY_4G, PUBKEY_5G];
        // Newest first: index 0 is height 5 (PUBKEY_G), index 1 is height 4
        // (PUBKEY_5G), index 2 is height 3 (PUBKEY_4G), index 3 is height 2
        // (PUBKEY_3G).
        let (headers, tip_hash) = build_test_chain(6, &miners);
        let height_four_miner = headers[1].header.miner_bitcoin_address.clone();
        let height_three_miner = headers[2].header.miner_bitcoin_address.clone();
        let height_two_miner = headers[3].header.miner_bitcoin_address.clone();
        let mut store = MockChainStoreHandle::default();
        store.expect_get_chain_tip().returning(move || Ok(tip_hash));
        store
            .expect_get_block_metadata()
            .returning(|_| Ok(metadata_at_height(5)));
        store
            .expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(headers.clone()));
        let mut window = PplnsWindow::new_with_window_sizes(TEST_NETWORK, 3, 2);
        window.update(&store).unwrap();

        let distribution = window
            .get_distribution_from_start_hash(
                tip_hash,
                BITCOIN_BLOCK_DIFFICULTY.into(),
                &MockChainStoreHandle::default(),
            )
            .unwrap();

        assert_eq!(distribution.len(), 2);
        assert!(distribution.contains_key(&height_three_miner));
        assert!(distribution.contains_key(&height_two_miner));
        assert!(!distribution.contains_key(&height_four_miner));
    }

    //* The skip counts candidate and confirmed shares alike. The confirmed
    //* chain is heights 0..=4 and the anchor is a candidate at height 5, so
    //* the paying share is at height 6. The skip of 2 covers the candidate
    //* (height 5) and the confirmed tip (height 4), and the window pays
    //* heights 3 and 2.
    #[test]
    fn test_distribution_skip_spans_candidate_and_confirmed_entries() {
        let miners = [PUBKEY_G, PUBKEY_2G, PUBKEY_3G, PUBKEY_4G, PUBKEY_5G];
        // Newest first: index 0 is height 4 (PUBKEY_5G), index 1 is height 3
        // (PUBKEY_4G), index 2 is height 2 (PUBKEY_3G).
        let (headers, tip_hash) = build_test_chain(5, &miners);
        let height_four_miner = headers[0].header.miner_bitcoin_address.clone();
        let height_three_miner = headers[1].header.miner_bitcoin_address.clone();
        let height_two_miner = headers[2].header.miner_bitcoin_address.clone();
        let candidate = build_test_header(&tip_hash.to_string(), PUBKEY_G, 2);
        let candidate_hash = candidate.block_hash();
        let mut store = MockChainStoreHandle::default();
        store.expect_get_chain_tip().returning(move || Ok(tip_hash));
        store
            .expect_get_block_metadata()
            .returning(|_| Ok(metadata_at_height(4)));
        store
            .expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(headers.clone()));
        let mut window = PplnsWindow::new_with_window_sizes(TEST_NETWORK, 3, 2);
        window.update(&store).unwrap();
        let mut candidate_store = MockChainStoreHandle::default();
        candidate_store
            .expect_get_share_header()
            .withf(move |hash| *hash == candidate_hash)
            .returning(move |_| Ok(candidate.clone()));
        candidate_store
            .expect_get_share_headers()
            .returning(|_| Ok(Vec::new()));

        let distribution = window
            .get_distribution_from_start_hash(
                candidate_hash,
                BITCOIN_BLOCK_DIFFICULTY.into(),
                &candidate_store,
            )
            .unwrap();

        assert_eq!(distribution.len(), 2);
        assert!(distribution.contains_key(&height_three_miner));
        assert!(distribution.contains_key(&height_two_miner));
        assert!(!distribution.contains_key(&height_four_miner));
    }

    //* The cache holds maturity + window + buffer = 3 + 2 + 1 = 6 entries, so
    //* loading heights 0..=7 evicts heights 0 and 1. Anchored at height 4 the
    //* walk skips heights 4 and 3, pays height 2, and then runs out of cached
    //* entries one share short of the window. The back is not the chain
    //* start, so the truncated result is refused.
    #[test]
    fn test_distribution_refuses_when_skip_and_window_exceed_cache() {
        let miners = [PUBKEY_G, PUBKEY_2G, PUBKEY_3G, PUBKEY_4G, PUBKEY_5G];
        // Newest first: index 3 is height 4.
        let (headers, tip_hash) = build_test_chain(8, &miners);
        let height_four_hash = headers[3].blockhash;
        let mut store = MockChainStoreHandle::default();
        store.expect_get_chain_tip().returning(move || Ok(tip_hash));
        store
            .expect_get_block_metadata()
            .returning(|_| Ok(metadata_at_height(7)));
        store
            .expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(headers.clone()));
        let mut window = PplnsWindow::new_with_window_sizes(TEST_NETWORK, 3, 2);
        window.update(&store).unwrap();

        let error = window
            .get_distribution_from_start_hash(
                height_four_hash,
                BITCOIN_BLOCK_DIFFICULTY.into(),
                &MockChainStoreHandle::default(),
            )
            .expect_err("the skip leaves one cached entry below the anchor, short of the window");

        assert!(
            error.to_string().contains("truncated by eviction"),
            "unexpected error: {error}"
        );
    }

    //* Before the paying height reaches the maturity depth every share is
    //* below it, so all are mature and nothing is skipped. Heights 0 and 1,
    //* anchored at height 1: the paying share is at height 2, below 3.
    #[test]
    fn test_distribution_does_not_skip_before_maturity_height() {
        let miners = [PUBKEY_G, PUBKEY_2G];
        let (headers, tip_hash) = build_test_chain(2, &miners);
        let mut store = MockChainStoreHandle::default();
        store.expect_get_chain_tip().returning(move || Ok(tip_hash));
        store
            .expect_get_block_metadata()
            .returning(|_| Ok(metadata_at_height(1)));
        store
            .expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(headers.clone()));
        let mut window = PplnsWindow::new_with_window_sizes(TEST_NETWORK, 3, 10);
        window.update(&store).unwrap();

        let distribution = window
            .get_distribution_from_start_hash(
                tip_hash,
                BITCOIN_BLOCK_DIFFICULTY.into(),
                &MockChainStoreHandle::default(),
            )
            .unwrap();

        assert_eq!(distribution.len(), 2);
    }

    //* Shares below the maturity depth stay mature after it is passed. Heights
    //* 0..=3, anchored at height 3: the paying share is at height 4. Height 3
    //* is at or above the maturity depth and only 1 deep, so it is skipped.
    //* Heights 2, 1 and 0 are below the maturity depth and all paid.
    #[test]
    fn test_distribution_keeps_early_shares_mature_after_bootstrap() {
        let miners = [PUBKEY_G, PUBKEY_2G, PUBKEY_3G, PUBKEY_4G];
        // Newest first: index 0 is height 3 (PUBKEY_4G).
        let (headers, tip_hash) = build_test_chain(4, &miners);
        let height_three_miner = headers[0].header.miner_bitcoin_address.clone();
        let mut store = MockChainStoreHandle::default();
        store.expect_get_chain_tip().returning(move || Ok(tip_hash));
        store
            .expect_get_block_metadata()
            .returning(|_| Ok(metadata_at_height(3)));
        store
            .expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(headers.clone()));
        let mut window = PplnsWindow::new_with_window_sizes(TEST_NETWORK, 3, 10);
        window.update(&store).unwrap();

        let distribution = window
            .get_distribution_from_start_hash(
                tip_hash,
                BITCOIN_BLOCK_DIFFICULTY.into(),
                &MockChainStoreHandle::default(),
            )
            .unwrap();

        assert_eq!(distribution.len(), 3);
        assert!(!distribution.contains_key(&height_three_miner));
    }

    //* At paying height 2 * maturity - 1 the skip reaches its full
    //* maturity - 1. Heights 0..=4, anchored at height 4: the paying share is
    //* at height 5. Heights 4 and 3 are skipped, and heights 2, 1 and 0 are
    //* paid.
    #[test]
    fn test_distribution_skips_full_maturity_after_early_shares_age_out() {
        let miners = [PUBKEY_G, PUBKEY_2G, PUBKEY_3G, PUBKEY_4G, PUBKEY_5G];
        // Newest first: index 0 is height 4 (PUBKEY_5G), index 1 is height 3
        // (PUBKEY_4G).
        let (headers, tip_hash) = build_test_chain(5, &miners);
        let height_four_miner = headers[0].header.miner_bitcoin_address.clone();
        let height_three_miner = headers[1].header.miner_bitcoin_address.clone();
        let mut store = MockChainStoreHandle::default();
        store.expect_get_chain_tip().returning(move || Ok(tip_hash));
        store
            .expect_get_block_metadata()
            .returning(|_| Ok(metadata_at_height(4)));
        store
            .expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(headers.clone()));
        let mut window = PplnsWindow::new_with_window_sizes(TEST_NETWORK, 3, 10);
        window.update(&store).unwrap();

        let distribution = window
            .get_distribution_from_start_hash(
                tip_hash,
                BITCOIN_BLOCK_DIFFICULTY.into(),
                &MockChainStoreHandle::default(),
            )
            .unwrap();

        assert_eq!(distribution.len(), 3);
        assert!(!distribution.contains_key(&height_four_miner));
        assert!(!distribution.contains_key(&height_three_miner));
    }

    //* The credits walk and weigh the window exactly as the distribution
    //* does: every payout address gets its distribution entry, a nephew's
    //* weight is its scaled difficulty plus the bonus for the uncle it
    //* includes, and the window total is the sum of the distribution.
    #[test]
    fn test_window_credits_match_the_distribution_for_every_payout_address() {
        let genesis_hash = BlockHash::all_zeros();
        let header_one = build_test_header(&genesis_hash.to_string(), PUBKEY_2G, 2);
        let uncle_header = build_test_header(&genesis_hash.to_string(), PUBKEY_3G, 2);
        let nephew_header = build_test_header_with_uncles(
            &header_one.block_hash().to_string(),
            PUBKEY_G,
            2,
            vec![uncle_header.block_hash()],
        );

        let mut window = PplnsWindow::new(TEST_NETWORK);
        let uncle_entry = uncle_entry_from_header(&mut window, &uncle_header);
        let nephew_entry =
            entry_from_header_with_uncles(&mut window, &nephew_header, 1, vec![uncle_entry]);
        let entry_one = entry_from_header(&mut window, &header_one, 0);
        window.confirmed_entries.push_back(nephew_entry);
        window.confirmed_entries.push_back(entry_one);

        let distribution = window
            .get_distribution_from_start_hash(
                nephew_header.block_hash(),
                BITCOIN_BLOCK_DIFFICULTY.into(),
                &MockChainStoreHandle::default(),
            )
            .unwrap();
        let window_credits = window
            .get_window_credits_from_start_hash(
                nephew_header.block_hash(),
                BITCOIN_BLOCK_DIFFICULTY.into(),
                &MockChainStoreHandle::default(),
                10,
            )
            .unwrap();

        let nephew_miner = &nephew_header.miner_bitcoin_address;
        let uncle_miner = &uncle_header.miner_bitcoin_address;
        let miner_one = &header_one.miner_bitcoin_address;
        assert_eq!(window_credits.by_payout_address.len(), distribution.len());
        assert_eq!(
            window_credits.by_payout_address[nephew_miner].weight,
            distribution[nephew_miner]
        );
        assert_eq!(
            window_credits.by_payout_address[uncle_miner].weight,
            distribution[uncle_miner]
        );
        assert_eq!(
            window_credits.by_payout_address[miner_one].weight,
            distribution[miner_one]
        );
        assert_eq!(
            window_credits.window_weight,
            distribution.values().sum::<u128>()
        );
        assert_eq!(window_credits.anchor_height, 1);
        assert_eq!(window_credits.window_share_count, 2);
        assert_eq!(window_credits.newest_height, Some(1));
        assert_eq!(window_credits.oldest_height, Some(0));
        let nephew_credits = &window_credits.by_payout_address[nephew_miner];
        assert_eq!(nephew_credits.share_count, 1);
        assert_eq!(nephew_credits.uncle_count, 0);
        assert_eq!(
            nephew_credits.credits,
            vec![WindowCredit {
                blockhash: nephew_header.block_hash(),
                height: 1,
                nephew: None,
                difficulty: nephew_header.get_difficulty(TEST_NETWORK),
                weight: distribution[nephew_miner],
                payout_address: nephew_miner.clone(),
            }]
        );
    }

    //* An uncle is credited at the height of the window share that includes
    //* it, and the credit names both the uncle and that nephew.
    #[test]
    fn test_window_credits_list_an_uncle_at_its_nephew_height() {
        let genesis_hash = BlockHash::all_zeros();
        let header_one = build_test_header(&genesis_hash.to_string(), PUBKEY_2G, 2);
        let uncle_header = build_test_header(&genesis_hash.to_string(), PUBKEY_3G, 2);
        let uncle_difficulty = uncle_header.get_difficulty(TEST_NETWORK);
        let nephew_header = build_test_header_with_uncles(
            &header_one.block_hash().to_string(),
            PUBKEY_G,
            2,
            vec![uncle_header.block_hash()],
        );

        let mut window = PplnsWindow::new(TEST_NETWORK);
        let uncle_entry = uncle_entry_from_header(&mut window, &uncle_header);
        let nephew_entry =
            entry_from_header_with_uncles(&mut window, &nephew_header, 1, vec![uncle_entry]);
        let entry_one = entry_from_header(&mut window, &header_one, 0);
        window.confirmed_entries.push_back(nephew_entry);
        window.confirmed_entries.push_back(entry_one);

        let window_credits = window
            .get_window_credits_from_start_hash(
                nephew_header.block_hash(),
                BITCOIN_BLOCK_DIFFICULTY.into(),
                &MockChainStoreHandle::default(),
                10,
            )
            .unwrap();

        let uncle_credits = &window_credits.by_payout_address[&uncle_header.miner_bitcoin_address];
        assert_eq!(uncle_credits.share_count, 0);
        assert_eq!(uncle_credits.uncle_count, 1);
        assert_eq!(uncle_credits.weight, uncle_difficulty * UNCLE_SCALED_WEIGHT);
        assert_eq!(
            uncle_credits.credits,
            vec![WindowCredit {
                blockhash: uncle_header.block_hash(),
                height: 1,
                nephew: Some(nephew_header.block_hash()),
                difficulty: uncle_difficulty,
                weight: uncle_difficulty * UNCLE_SCALED_WEIGHT,
                payout_address: uncle_header.miner_bitcoin_address.clone(),
            }]
        );
    }

    //* Two shares pay the same payout address but name different share
    //* owners. Each owner gets only its own share and the payout address
    //* that share names, while the payout address gets both.
    #[test]
    fn test_window_credits_key_share_owners_apart_from_payout_addresses() {
        let genesis_hash = BlockHash::all_zeros();
        let owner_one = ShareOwner::CashP2pkh([0x11; 20]);
        let owner_two = ShareOwner::CashP2pkh([0x22; 20]);
        let header_one = TestShareBlockBuilder::new()
            .prev_share_blockhash(genesis_hash.to_string())
            .miner_pubkey(PUBKEY_G)
            .share_owner(owner_one)
            .build()
            .header;
        let header_two = TestShareBlockBuilder::new()
            .prev_share_blockhash(header_one.block_hash().to_string())
            .miner_pubkey(PUBKEY_G)
            .share_owner(owner_two)
            .build()
            .header;

        let mut window = PplnsWindow::new(TEST_NETWORK);
        let entry_two = entry_from_header(&mut window, &header_two, 1);
        let entry_one = entry_from_header(&mut window, &header_one, 0);
        window.confirmed_entries.push_back(entry_two);
        window.confirmed_entries.push_back(entry_one);

        let window_credits = window
            .get_window_credits_from_start_hash(
                header_two.block_hash(),
                BITCOIN_BLOCK_DIFFICULTY.into(),
                &MockChainStoreHandle::default(),
                10,
            )
            .unwrap();

        let weight_one = header_one.get_difficulty(TEST_NETWORK) * DIFFICULTY_SCALE;
        let owner_one_credits = &window_credits.by_share_owner[&owner_one];
        assert_eq!(owner_one_credits.share_count, 1);
        assert_eq!(owner_one_credits.weight, weight_one);
        assert_eq!(owner_one_credits.credits.len(), 1);
        assert_eq!(
            owner_one_credits.credits[0].blockhash,
            header_one.block_hash()
        );
        assert_eq!(owner_one_credits.credits[0].height, 0);
        assert_eq!(
            owner_one_credits.credits[0].payout_address,
            header_one.miner_bitcoin_address
        );
        assert_eq!(window_credits.by_share_owner[&owner_two].share_count, 1);
        assert_eq!(
            window_credits.by_payout_address[&header_one.miner_bitcoin_address].share_count,
            2
        );
    }

    //* A miner with nothing in the window is absent, and the window's totals
    //* are still there.
    #[test]
    fn test_window_credits_leave_out_a_miner_with_nothing_in_the_window() {
        let genesis_hash = BlockHash::all_zeros();
        let header_one = build_test_header(&genesis_hash.to_string(), PUBKEY_G, 2);
        let header_two = build_test_header(&header_one.block_hash().to_string(), PUBKEY_2G, 2);
        let unknown = build_test_header(&genesis_hash.to_string(), PUBKEY_3G, 2);

        let mut window = PplnsWindow::new(TEST_NETWORK);
        let entry_two = entry_from_header(&mut window, &header_two, 1);
        let entry_one = entry_from_header(&mut window, &header_one, 0);
        window.confirmed_entries.push_back(entry_two);
        window.confirmed_entries.push_back(entry_one);

        let window_credits = window
            .get_window_credits_from_start_hash(
                header_two.block_hash(),
                BITCOIN_BLOCK_DIFFICULTY.into(),
                &MockChainStoreHandle::default(),
                10,
            )
            .unwrap();

        let window_weight = (header_one.get_difficulty(TEST_NETWORK)
            + header_two.get_difficulty(TEST_NETWORK))
            * DIFFICULTY_SCALE;
        assert_eq!(window_credits.window_share_count, 2);
        assert_eq!(window_credits.window_weight, window_weight);
        assert!(
            !window_credits
                .by_payout_address
                .contains_key(&unknown.miner_bitcoin_address)
        );
        assert!(
            !window_credits
                .by_share_owner
                .contains_key(&unknown.miner_address)
        );
    }

    //* The listed credits stop at the requested number, newest first, while
    //* the counts and the weight still cover every share in the window.
    #[test]
    fn test_window_credits_cap_listed_credits() {
        let genesis_hash = BlockHash::all_zeros();
        let header_one = build_test_header(&genesis_hash.to_string(), PUBKEY_G, 2);
        let header_two = build_test_header(&header_one.block_hash().to_string(), PUBKEY_G, 2);
        let header_three = build_test_header(&header_two.block_hash().to_string(), PUBKEY_G, 2);

        let mut window = PplnsWindow::new(TEST_NETWORK);
        let entry_three = entry_from_header(&mut window, &header_three, 2);
        let entry_two = entry_from_header(&mut window, &header_two, 1);
        let entry_one = entry_from_header(&mut window, &header_one, 0);
        window.confirmed_entries.push_back(entry_three);
        window.confirmed_entries.push_back(entry_two);
        window.confirmed_entries.push_back(entry_one);

        let window_credits = window
            .get_window_credits_from_start_hash(
                header_three.block_hash(),
                BITCOIN_BLOCK_DIFFICULTY.into(),
                &MockChainStoreHandle::default(),
                2,
            )
            .unwrap();

        let miner_credits = &window_credits.by_payout_address[&header_one.miner_bitcoin_address];
        assert_eq!(miner_credits.share_count, 3);
        assert_eq!(miner_credits.weight, window_credits.window_weight);
        assert_eq!(miner_credits.credits.len(), 2);
        assert_eq!(
            miner_credits.credits[0].blockhash,
            header_three.block_hash()
        );
        assert_eq!(miner_credits.credits[1].blockhash, header_two.block_hash());
    }

    //* Same chain as test_distribution_skips_immature_shares_from_anchor:
    //* heights 5 and 4 are immature and the window is heights 3 and 2, so the
    //* height four miner has nothing in it.
    #[test]
    fn test_window_credits_skip_immature_shares_like_the_distribution() {
        let miners = [PUBKEY_G, PUBKEY_2G, PUBKEY_3G, PUBKEY_4G, PUBKEY_5G];
        let (headers, tip_hash) = build_test_chain(6, &miners);
        let height_four_miner = headers[1].header.miner_bitcoin_address.clone();
        let mut store = MockChainStoreHandle::default();
        store.expect_get_chain_tip().returning(move || Ok(tip_hash));
        store
            .expect_get_block_metadata()
            .returning(|_| Ok(metadata_at_height(5)));
        store
            .expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(headers.clone()));
        let mut window = PplnsWindow::new_with_window_sizes(TEST_NETWORK, 3, 2);
        window.update(&store).unwrap();

        let window_credits = window
            .get_window_credits_from_start_hash(
                tip_hash,
                BITCOIN_BLOCK_DIFFICULTY.into(),
                &MockChainStoreHandle::default(),
                10,
            )
            .unwrap();

        assert_eq!(window_credits.anchor_height, 5);
        assert_eq!(window_credits.newest_height, Some(3));
        assert_eq!(window_credits.oldest_height, Some(2));
        assert_eq!(window_credits.window_share_count, 2);
        assert!(
            !window_credits
                .by_payout_address
                .contains_key(&height_four_miner)
        );
    }

    //* A candidate anchor above the confirmed tip is cached with no height;
    //* the credits give it its real one. The confirmed chain is heights 0..=4
    //* and PUBKEY_G mined height 0 and the candidate at height 5.
    #[test]
    fn test_window_credits_give_candidate_entries_their_heights() {
        let miners = [PUBKEY_G, PUBKEY_2G, PUBKEY_3G, PUBKEY_4G, PUBKEY_5G];
        let (headers, tip_hash) = build_test_chain(5, &miners);
        let height_zero_hash = headers[4].blockhash;
        let candidate = build_test_header(&tip_hash.to_string(), PUBKEY_G, 2);
        let candidate_hash = candidate.block_hash();
        let candidate_miner = candidate.miner_bitcoin_address.clone();
        let mut store = MockChainStoreHandle::default();
        store.expect_get_chain_tip().returning(move || Ok(tip_hash));
        store
            .expect_get_block_metadata()
            .returning(|_| Ok(metadata_at_height(4)));
        store
            .expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(headers.clone()));
        let mut window = PplnsWindow::new_with_window_sizes(TEST_NETWORK, 0, 10);
        window.update(&store).unwrap();
        let mut candidate_store = MockChainStoreHandle::default();
        candidate_store
            .expect_get_share_header()
            .withf(move |hash| *hash == candidate_hash)
            .returning(move |_| Ok(candidate.clone()));
        candidate_store
            .expect_get_share_headers()
            .returning(|_| Ok(Vec::new()));

        let window_credits = window
            .get_window_credits_from_start_hash(
                candidate_hash,
                BITCOIN_BLOCK_DIFFICULTY.into(),
                &candidate_store,
                10,
            )
            .unwrap();

        let miner_credits = &window_credits.by_payout_address[&candidate_miner];
        assert_eq!(window_credits.window_share_count, 6);
        assert_eq!(window_credits.newest_height, Some(5));
        assert_eq!(miner_credits.share_count, 2);
        assert_eq!(miner_credits.credits[0].blockhash, candidate_hash);
        assert_eq!(miner_credits.credits[0].height, 5);
        assert_eq!(miner_credits.credits[1].blockhash, height_zero_hash);
        assert_eq!(miner_credits.credits[1].height, 0);
    }

    //* Same cache as test_distribution_errors_when_eviction_truncates_the_window:
    //* the credits refuse the truncated window just as the distribution does.
    #[test]
    fn test_window_credits_error_when_eviction_truncates_the_window() {
        const MAX_SHARES: usize = 3;
        let genesis = BlockHash::all_zeros();

        let a = build_test_header(&genesis.to_string(), PUBKEY_G, 2);
        let b = build_test_header(&a.block_hash().to_string(), PUBKEY_2G, 2);
        let c = build_test_header(&b.block_hash().to_string(), PUBKEY_3G, 2);
        let parent = build_test_header(&c.block_hash().to_string(), PUBKEY_4G, 2);
        let sibling = build_test_header(&parent.block_hash().to_string(), PUBKEY_5G, 2);

        let mut window = PplnsWindow::new_with_window_sizes(TEST_NETWORK, 0, MAX_SHARES);

        let all_headers = vec![
            ConfirmedHeaderResult {
                height: 4,
                blockhash: sibling.block_hash(),
                header: sibling.clone(),
            },
            ConfirmedHeaderResult {
                height: 3,
                blockhash: parent.block_hash(),
                header: parent.clone(),
            },
            ConfirmedHeaderResult {
                height: 2,
                blockhash: c.block_hash(),
                header: c.clone(),
            },
            ConfirmedHeaderResult {
                height: 1,
                blockhash: b.block_hash(),
                header: b.clone(),
            },
            ConfirmedHeaderResult {
                height: 0,
                blockhash: a.block_hash(),
                header: a.clone(),
            },
        ];
        let sibling_tip = sibling.block_hash();
        let mut mock = MockChainStoreHandle::default();
        mock.expect_get_chain_tip()
            .returning(move || Ok(sibling_tip));
        mock.expect_get_block_metadata()
            .returning(|_| Ok(metadata_at_height(4)));
        mock.expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(all_headers.clone()));
        window.update(&mock).unwrap();

        let error = window
            .get_window_credits_from_start_hash(
                c.block_hash(),
                BITCOIN_BLOCK_DIFFICULTY.into(),
                &MockChainStoreHandle::default(),
                10,
            )
            .expect_err("only two entries are cached below the anchor, short of the window");
        assert!(
            error.to_string().contains("truncated by eviction"),
            "unexpected error: {error}"
        );
    }

    //* Share owners are interned like payout addresses, and freed with them
    //* once the last entry naming one is evicted.
    #[test]
    fn test_update_prunes_owner_key_of_evicted_share() {
        let genesis_hash = BlockHash::all_zeros();
        let header_a = build_test_header(&genesis_hash.to_string(), PUBKEY_G, 2);
        let header_b = build_test_header(&header_a.block_hash().to_string(), PUBKEY_2G, 2);

        let mut window = PplnsWindow::new(TEST_NETWORK);
        let entry_b = entry_from_header(&mut window, &header_b, 1);
        let entry_a = entry_from_header(&mut window, &header_a, 0);
        let owner_key_a = entry_a.owner_key;
        let owner_key_b = entry_b.owner_key;
        window.confirmed_entries.push_back(entry_b);
        window.confirmed_entries.push_back(entry_a);
        assert_eq!(
            window.owner_keys.value_for(owner_key_a),
            Some(&header_a.miner_address)
        );

        window.confirmed_entries.pop_back();
        window.prune_unreferenced_keys();

        assert_eq!(window.owner_keys.value_for(owner_key_a), None);
        assert_eq!(
            window.owner_keys.value_for(owner_key_b),
            Some(&header_b.miner_address)
        );
    }

    //* Bitcoin Cash window tests. Unless noted, five shares at heights 4 down
    //* to 0, one miner each, all of the same difficulty `share_difficulty`.
    //* There is no maturity skip and the cap is four shares, so the walk
    //* starts at the anchor (height 4) and the cap alone pays heights 4 to 1.
    //* A paying block difficulty equal to `share_difficulty` means every
    //* share is one block's worth of work: the large pool regime.

    #[test]
    fn test_bch_window_large_pool_pays_three_blocks_of_work() {
        let miners = [PUBKEY_G, PUBKEY_2G, PUBKEY_3G, PUBKEY_4G, PUBKEY_5G];
        let (headers, tip_hash) = build_test_chain(5, &miners);
        let newest_header = headers[0].header.clone();
        let height_four_miner = headers[0].header.miner_bitcoin_address.clone();
        let height_three_miner = headers[1].header.miner_bitcoin_address.clone();
        let height_two_miner = headers[2].header.miner_bitcoin_address.clone();
        let mut store = MockChainStoreHandle::default();
        store.expect_get_chain_tip().returning(move || Ok(tip_hash));
        store
            .expect_get_block_metadata()
            .returning(|_| Ok(metadata_at_height(4)));
        store
            .expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(headers.clone()));
        let mut window = PplnsWindow::new(TEST_NETWORK)
            .for_chain(Chain::BitcoinCash)
            .with_window_sizes(0, 4);
        window.update(&store).unwrap();
        //* Weights are exact work. A block of the share's difficulty, in whole
        //* units of difficulty, holds at most one share's work, so three
        //* blocks are reached at the third share.
        let share_weight = window.base_weight(&newest_header);
        let paying_block_difficulty = share_weight / difficulty_one_work(TEST_NETWORK);

        let distribution = window
            .get_distribution_from_start_hash(
                tip_hash,
                paying_block_difficulty.into(),
                &MockChainStoreHandle::default(),
            )
            .unwrap();

        assert_eq!(distribution.len(), 3);
        assert_eq!(
            distribution[&height_four_miner],
            share_weight * DIFFICULTY_SCALE
        );
        assert_eq!(
            distribution[&height_three_miner],
            share_weight * DIFFICULTY_SCALE
        );
        assert_eq!(
            distribution[&height_two_miner],
            share_weight * DIFFICULTY_SCALE
        );
    }

    //* A target of three blocks plus three difficulty units is not reached by
    //* heights 4 to 2, so height 1, the share that reaches it, is paid too.
    //* The cap is five shares here, so height 0 is left out by the target.
    #[test]
    fn test_bch_window_includes_the_share_that_reaches_the_work_target() {
        let miners = [PUBKEY_G, PUBKEY_2G, PUBKEY_3G, PUBKEY_4G, PUBKEY_5G];
        let (headers, tip_hash) = build_test_chain(5, &miners);
        let share_difficulty = headers[0].header.get_difficulty(TEST_NETWORK);
        let height_one_miner = headers[3].header.miner_bitcoin_address.clone();
        let height_zero_miner = headers[4].header.miner_bitcoin_address.clone();
        let mut store = MockChainStoreHandle::default();
        store.expect_get_chain_tip().returning(move || Ok(tip_hash));
        store
            .expect_get_block_metadata()
            .returning(|_| Ok(metadata_at_height(4)));
        store
            .expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(headers.clone()));
        let mut window = PplnsWindow::new(TEST_NETWORK)
            .for_chain(Chain::BitcoinCash)
            .with_window_sizes(0, 5);
        window.update(&store).unwrap();

        let distribution = window
            .get_distribution_from_start_hash(
                tip_hash,
                (share_difficulty + 1).into(),
                &MockChainStoreHandle::default(),
            )
            .unwrap();

        assert_eq!(distribution.len(), 4);
        assert!(distribution.contains_key(&height_one_miner));
        assert!(!distribution.contains_key(&height_zero_miner));
    }

    //* A block worth 100 shares asks for 300 shares of work, more than the
    //* four-share cap: the small pool regime pays the capped window, heights
    //* 4 to 1, and height 0 is left out.
    #[test]
    fn test_bch_window_small_pool_is_capped_at_window_shares() {
        let miners = [PUBKEY_G, PUBKEY_2G, PUBKEY_3G, PUBKEY_4G, PUBKEY_5G];
        let (headers, tip_hash) = build_test_chain(5, &miners);
        let share_difficulty = headers[0].header.get_difficulty(TEST_NETWORK);
        let height_zero_miner = headers[4].header.miner_bitcoin_address.clone();
        let mut store = MockChainStoreHandle::default();
        store.expect_get_chain_tip().returning(move || Ok(tip_hash));
        store
            .expect_get_block_metadata()
            .returning(|_| Ok(metadata_at_height(4)));
        store
            .expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(headers.clone()));
        let mut window = PplnsWindow::new(TEST_NETWORK)
            .for_chain(Chain::BitcoinCash)
            .with_window_sizes(0, 4);
        window.update(&store).unwrap();

        let distribution = window
            .get_distribution_from_start_hash(
                tip_hash,
                (share_difficulty * 100).into(),
                &MockChainStoreHandle::default(),
            )
            .unwrap();

        assert_eq!(distribution.len(), 4);
        assert!(!distribution.contains_key(&height_zero_miner));
    }

    //* A chipnet block mined under the 20-minute rule has difficulty 1. It
    //* must not shrink the window to one share: it pays the capped window,
    //* heights 4 to 1, as a fixed window would.
    #[test]
    fn test_bch_window_minimum_difficulty_block_pays_the_capped_window() {
        let miners = [PUBKEY_G, PUBKEY_2G, PUBKEY_3G, PUBKEY_4G, PUBKEY_5G];
        let (headers, tip_hash) = build_test_chain(5, &miners);
        let height_zero_miner = headers[4].header.miner_bitcoin_address.clone();
        let mut store = MockChainStoreHandle::default();
        store.expect_get_chain_tip().returning(move || Ok(tip_hash));
        store
            .expect_get_block_metadata()
            .returning(|_| Ok(metadata_at_height(4)));
        store
            .expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(headers.clone()));
        let mut window = PplnsWindow::new(TEST_NETWORK)
            .for_chain(Chain::BitcoinCash)
            .with_window_sizes(0, 4);
        window.update(&store).unwrap();
        let chipnet_minimum_difficulty =
            bitcoin_block_difficulty(CompactTarget::from_consensus(0x1d00ffff), Network::Testnet);

        let distribution = window
            .get_distribution_from_start_hash(
                tip_hash,
                chipnet_minimum_difficulty.into(),
                &MockChainStoreHandle::default(),
            )
            .unwrap();

        assert_eq!(distribution.len(), 4);
        assert!(!distribution.contains_key(&height_zero_miner));
    }

    //* Bits easier than the proof-of-work limit give difficulty 0. They size
    //* nothing either, so they also pay the capped window.
    #[test]
    fn test_bch_window_zero_difficulty_block_pays_the_capped_window() {
        let miners = [PUBKEY_G, PUBKEY_2G, PUBKEY_3G, PUBKEY_4G, PUBKEY_5G];
        let (headers, tip_hash) = build_test_chain(5, &miners);
        let mut store = MockChainStoreHandle::default();
        store.expect_get_chain_tip().returning(move || Ok(tip_hash));
        store
            .expect_get_block_metadata()
            .returning(|_| Ok(metadata_at_height(4)));
        store
            .expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(headers.clone()));
        let mut window = PplnsWindow::new(TEST_NETWORK)
            .for_chain(Chain::BitcoinCash)
            .with_window_sizes(0, 4);
        window.update(&store).unwrap();

        let distribution = window
            .get_distribution_from_start_hash(
                tip_hash,
                0_u128.into(),
                &MockChainStoreHandle::default(),
            )
            .unwrap();

        assert_eq!(distribution.len(), 4);
    }

    //* Difficulty 2 is the easiest block that sizes the window: six units of
    //* work, which the anchor's share alone holds.
    #[test]
    fn test_bch_window_difficulty_two_block_is_sized() {
        let miners = [PUBKEY_G, PUBKEY_2G, PUBKEY_3G, PUBKEY_4G, PUBKEY_5G];
        let (headers, tip_hash) = build_test_chain(5, &miners);
        let height_four_miner = headers[0].header.miner_bitcoin_address.clone();
        let mut store = MockChainStoreHandle::default();
        store.expect_get_chain_tip().returning(move || Ok(tip_hash));
        store
            .expect_get_block_metadata()
            .returning(|_| Ok(metadata_at_height(4)));
        store
            .expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(headers.clone()));
        let mut window = PplnsWindow::new(TEST_NETWORK)
            .for_chain(Chain::BitcoinCash)
            .with_window_sizes(0, 4);
        window.update(&store).unwrap();

        let distribution = window
            .get_distribution_from_start_hash(
                tip_hash,
                2_u128.into(),
                &MockChainStoreHandle::default(),
            )
            .unwrap();

        assert_eq!(distribution.len(), 1);
        assert!(distribution.contains_key(&height_four_miner));
    }

    //* Bitcoin keeps upstream's fixed window: a block worth one share still
    //* pays the four-share window.
    #[test]
    fn test_bitcoin_window_ignores_paying_block_difficulty() {
        let miners = [PUBKEY_G, PUBKEY_2G, PUBKEY_3G, PUBKEY_4G, PUBKEY_5G];
        let (headers, tip_hash) = build_test_chain(5, &miners);
        let share_difficulty = headers[0].header.get_difficulty(TEST_NETWORK);
        let mut store = MockChainStoreHandle::default();
        store.expect_get_chain_tip().returning(move || Ok(tip_hash));
        store
            .expect_get_block_metadata()
            .returning(|_| Ok(metadata_at_height(4)));
        store
            .expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(headers.clone()));
        let mut window = PplnsWindow::new(TEST_NETWORK)
            .for_chain(Chain::Bitcoin)
            .with_window_sizes(0, 4);
        window.update(&store).unwrap();

        let distribution = window
            .get_distribution_from_start_hash(
                tip_hash,
                share_difficulty.into(),
                &MockChainStoreHandle::default(),
            )
            .unwrap();

        assert_eq!(distribution.len(), 4);
    }

    //* An uncle's work counts towards the target. The share at height 1 and
    //* its uncle hold three blocks' worth together, so the window ends there
    //* and the share at height 0 is not paid, though the uncle takes no slot.
    #[test]
    fn test_bch_window_counts_uncle_work_towards_the_target() {
        let genesis_hash = BlockHash::all_zeros();
        let header_a = build_test_header(&genesis_hash.to_string(), PUBKEY_G, 2);
        let uncle = build_test_header(&genesis_hash.to_string(), PUBKEY_2G, 2);
        let header_b = build_test_header_with_uncles(
            &header_a.block_hash().to_string(),
            PUBKEY_3G,
            2,
            vec![uncle.block_hash()],
        );
        let mut window = PplnsWindow::new(TEST_NETWORK)
            .for_chain(Chain::BitcoinCash)
            .with_window_sizes(0, 4);
        let share_difficulty = window.base_weight(&header_b);
        let uncle_difficulty = window.base_weight(&uncle);
        let uncle_entry = uncle_entry_from_header(&mut window, &uncle);
        let entry_b = entry_from_header_with_uncles(&mut window, &header_b, 1, vec![uncle_entry]);
        let entry_a = entry_from_header(&mut window, &header_a, 0);
        window.confirmed_entries.push_back(entry_b);
        window.confirmed_entries.push_back(entry_a);

        let distribution = window
            .get_distribution_from_start_hash(
                header_b.block_hash(),
                ((share_difficulty + uncle_difficulty) / 3 / difficulty_one_work(TEST_NETWORK))
                    .into(),
                &MockChainStoreHandle::default(),
            )
            .unwrap();

        assert_eq!(distribution.len(), 2);
        assert_eq!(
            distribution[&header_b.miner_bitcoin_address],
            share_difficulty * DIFFICULTY_SCALE + uncle_difficulty * NEPHEW_SCALED_BONUS
        );
        assert_eq!(
            distribution[&uncle.miner_bitcoin_address],
            uncle_difficulty * UNCLE_SCALED_WEIGHT
        );
        assert!(!distribution.contains_key(&header_a.miner_bitcoin_address));
    }

    /// The mini chain's easiest share (`0x1c384bd7`, difficulty 4.55) and a
    /// difficulty-5 share weigh in the ratio of their work, about 0.91, not
    /// 4 to 5 as rounded-down difficulty would have it.
    #[test]
    fn test_bch_window_weighs_shares_by_exact_work() {
        let window = PplnsWindow::new(Network::Testnet).for_chain(Chain::BitcoinCash);
        let mut easiest = build_test_header(&BlockHash::all_zeros().to_string(), PUBKEY_G, 2);
        easiest.bits = CompactTarget::from_consensus(0x1c38_4bd7);
        let mut harder = easiest.clone();
        harder.bits = CompactTarget::from_consensus(0x1c33_3300);

        let easiest_weight = window.base_weight(&easiest);
        let harder_weight = window.base_weight(&harder);

        assert_eq!(easiest.get_difficulty(Network::Testnet), 4);
        assert_eq!(harder.get_difficulty(Network::Testnet), 5);
        assert_eq!(
            easiest_weight,
            work_to_u128(Target::from_compact(easiest.bits).to_work())
        );
        let ratio = easiest_weight as f64 / harder_weight as f64;
        assert!((0.90..0.92).contains(&ratio), "ratio {ratio}");
    }

    #[test]
    fn test_bitcoin_window_weighs_shares_by_integer_difficulty() {
        let window = PplnsWindow::new(Network::Testnet).for_chain(Chain::Bitcoin);
        let mut easiest = build_test_header(&BlockHash::all_zeros().to_string(), PUBKEY_G, 2);
        easiest.bits = CompactTarget::from_consensus(0x1c38_4bd7);

        assert_eq!(window.base_weight(&easiest), 4);
    }

    /// One unit of difficulty weighs the work at the network's limit: about
    /// 2^32 on mainnet, testnet and chipnet, 2 on regtest.
    #[test]
    fn test_difficulty_one_work_per_network() {
        assert_eq!(difficulty_one_work(Network::Bitcoin), 0x1_0001_0001);
        assert_eq!(difficulty_one_work(Network::Testnet), 0x1_0001_0001);
        assert_eq!(difficulty_one_work(Network::Regtest), 2);
    }

    /// The work target is three blocks in units of difficulty-one work, so a
    /// window of exact-work shares reaches it as a window of difficulties did.
    #[test]
    fn test_bch_work_target_is_in_units_of_work() {
        let window = PplnsWindow::new(Network::Testnet).for_chain(Chain::BitcoinCash);
        assert_eq!(window.work_target(1_000), Some(3_000 * 0x1_0001_0001));
        assert_eq!(window.work_target(1), None);
    }

    //* As in test_distribution_refuses_when_skip_and_window_exceed_cache, the
    //* cache holds heights 7 down to 2 and the walk anchored at height 4 skips
    //* heights 4 and 3. Three blocks' worth needs more than height 2, and
    //* height 1 is evicted, so the truncated result is refused.
    #[test]
    fn test_bch_window_refuses_when_work_target_runs_past_the_cache() {
        let miners = [PUBKEY_G, PUBKEY_2G, PUBKEY_3G, PUBKEY_4G, PUBKEY_5G];
        // Newest first: index 3 is height 4.
        let (headers, tip_hash) = build_test_chain(8, &miners);
        let height_four_hash = headers[3].blockhash;
        let share_difficulty = headers[0].header.get_difficulty(TEST_NETWORK);
        let mut store = MockChainStoreHandle::default();
        store.expect_get_chain_tip().returning(move || Ok(tip_hash));
        store
            .expect_get_block_metadata()
            .returning(|_| Ok(metadata_at_height(7)));
        store
            .expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(headers.clone()));
        let mut window = PplnsWindow::new(TEST_NETWORK)
            .for_chain(Chain::BitcoinCash)
            .with_window_sizes(3, 2);
        window.update(&store).unwrap();

        let error = window
            .get_distribution_from_start_hash(
                height_four_hash,
                share_difficulty.into(),
                &MockChainStoreHandle::default(),
            )
            .expect_err("three blocks' worth needs the evicted height 1");

        assert!(
            error.to_string().contains("truncated by eviction"),
            "unexpected error: {error}"
        );
    }

    //* The same cache and anchor, with a block worth a third of a share: height 2
    //* alone reaches the target, so the walk ends inside the cache and the
    //* result is the same on every node, though the cache misses height 0.
    #[test]
    fn test_bch_window_reached_inside_the_cache_is_paid() {
        let miners = [PUBKEY_G, PUBKEY_2G, PUBKEY_3G, PUBKEY_4G, PUBKEY_5G];
        // Newest first: index 3 is height 4, index 5 is height 2.
        let (headers, tip_hash) = build_test_chain(8, &miners);
        let height_four_hash = headers[3].blockhash;
        let height_two_miner = headers[5].header.miner_bitcoin_address.clone();
        let share_difficulty = headers[0].header.get_difficulty(TEST_NETWORK);
        let mut store = MockChainStoreHandle::default();
        store.expect_get_chain_tip().returning(move || Ok(tip_hash));
        store
            .expect_get_block_metadata()
            .returning(|_| Ok(metadata_at_height(7)));
        store
            .expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(headers.clone()));
        let mut window = PplnsWindow::new(TEST_NETWORK)
            .for_chain(Chain::BitcoinCash)
            .with_window_sizes(3, 2);
        window.update(&store).unwrap();

        let distribution = window
            .get_distribution_from_start_hash(
                height_four_hash,
                (share_difficulty / 3).into(),
                &MockChainStoreHandle::default(),
            )
            .unwrap();

        assert_eq!(distribution.len(), 1);
        assert!(distribution.contains_key(&height_two_miner));
    }

    //* The offline simulation's entry point runs the same walk as the store
    //* entry point for a cached anchor.
    #[test]
    fn test_cached_anchor_distribution_matches_store_walk() {
        let miners = [PUBKEY_G, PUBKEY_2G, PUBKEY_3G, PUBKEY_4G, PUBKEY_5G];
        let (headers, tip_hash) = build_test_chain(5, &miners);
        let share_difficulty = headers[0].header.get_difficulty(TEST_NETWORK);
        let mut store = MockChainStoreHandle::default();
        store.expect_get_chain_tip().returning(move || Ok(tip_hash));
        store
            .expect_get_block_metadata()
            .returning(|_| Ok(metadata_at_height(4)));
        store
            .expect_get_confirmed_headers_in_range()
            .returning(move |_, _| Ok(headers.clone()));
        let mut window = PplnsWindow::new(TEST_NETWORK)
            .for_chain(Chain::BitcoinCash)
            .with_window_sizes(0, 4);
        window.update(&store).unwrap();

        let from_store = window
            .get_distribution_from_start_hash(
                tip_hash,
                (share_difficulty + 1).into(),
                &MockChainStoreHandle::default(),
            )
            .unwrap();
        let from_cache = window
            .distribution_for_cached_anchor(tip_hash, share_difficulty + 1)
            .unwrap();

        assert_eq!(from_cache, from_store);
    }

    #[test]
    fn test_chipnet_minimum_difficulty_bits_have_difficulty_one() {
        let difficulty =
            bitcoin_block_difficulty(CompactTarget::from_consensus(0x1d00ffff), Network::Testnet);

        assert_eq!(difficulty, MIN_SIZING_BLOCK_DIFFICULTY);
    }

    #[test]
    fn test_regtest_bits_have_difficulty_one() {
        let difficulty =
            bitcoin_block_difficulty(CompactTarget::from_consensus(0x207fffff), Network::Regtest);

        assert_eq!(difficulty, MIN_SIZING_BLOCK_DIFFICULTY);
    }

    //* Bitcoin Cash matures a share after 100 shares, as Satoshi's coinbase
    //* maturity counts 100 blocks: a paying share at height 1,000 skips the 99
    //* shares above height 900.
    #[test]
    fn test_bch_window_matures_shares_after_one_hundred_shares() {
        let window = PplnsWindow::new(TEST_NETWORK).for_chain(Chain::BitcoinCash);

        assert_eq!(
            window.immature_share_count(1_000),
            BCH_COINBASE_MATURITY - 1
        );
    }

    //* The first 100 shares of a Bitcoin Cash chain are always mature for
    //* payout, so a paying share at height 150 skips only the 50 shares from
    //* height 100 up.
    #[test]
    fn test_bch_window_keeps_the_first_hundred_shares_mature() {
        let window = PplnsWindow::new(TEST_NETWORK).for_chain(Chain::BitcoinCash);

        assert_eq!(window.immature_share_count(150), 50);
    }

    #[test]
    fn test_bitcoin_window_keeps_upstream_maturity() {
        let window = PplnsWindow::new(TEST_NETWORK).for_chain(Chain::Bitcoin);

        assert_eq!(window.immature_share_count(20_000), COINBASE_MATURITY - 1);
    }

    #[test]
    fn test_bch_transferred_share_and_uncle_keep_their_own_cuts() {
        let mut window = PplnsWindow::new(Network::Regtest).for_chain(Chain::BitcoinCash);
        let share_payee = PayoutAddress::from(Address::p2pkh(
            PubkeyHash::from_byte_array([1; 20]),
            Network::Regtest,
        ));
        let uncle_payee = PayoutAddress::from(Address::p2pkh(
            PubkeyHash::from_byte_array([2; 20]),
            Network::Regtest,
        ));
        let buyer = PayoutAddress::from(Address::p2pkh(
            PubkeyHash::from_byte_array([3; 20]),
            Network::Regtest,
        ));
        let donation = PayoutAddress::from(Address::p2pkh(
            PubkeyHash::from_byte_array([4; 20]),
            Network::Regtest,
        ));
        let share_fee = PayoutAddress::from(Address::p2pkh(
            PubkeyHash::from_byte_array([5; 20]),
            Network::Regtest,
        ));
        let uncle_fee = PayoutAddress::from(Address::p2pkh(
            PubkeyHash::from_byte_array([6; 20]),
            Network::Regtest,
        ));
        let donation_key = window.address_keys.key_for(donation.clone());
        let share_fee_key = window.address_keys.key_for(share_fee.clone());
        let uncle_fee_key = window.address_keys.key_for(uncle_fee.clone());
        let buyer_key = window.address_keys.key_for(buyer.clone());
        let mut uncle = window.build_uncle_entry(
            BlockHash::from_byte_array([2; 32]),
            uncle_payee.clone(),
            BENCHMARK_SHARE_OWNER,
            3,
        );
        uncle.cuts = ShareCuts {
            donation: None,
            fee: Some((uncle_fee_key, 500)),
        };
        let anchor = BlockHash::from_byte_array([1; 32]);
        let mut share = window.build_confirmed_entry(
            anchor,
            0,
            share_payee.clone(),
            BENCHMARK_SHARE_OWNER,
            2,
            vec![uncle],
        );
        share.cuts = ShareCuts {
            donation: Some((donation_key, 1_000)),
            fee: Some((share_fee_key, 1_000)),
        };
        window.confirmed_entries.push_back(share);
        let resolved =
            window.resolved_window(Vec::new(), 0, 1, HashMap::from([(anchor, buyer_key)]));
        let weights = window.distribution_for_resolved(anchor, &resolved).unwrap();

        assert!(!weights.contains_key(&share_payee));
        assert_eq!(weights[&donation], 230_000_000);
        assert_eq!(weights[&share_fee], 207_000_000);
        assert_eq!(weights[&buyer], 1_863_000_000);
        assert_eq!(weights[&uncle_fee], 135_000_000);
        assert_eq!(weights[&uncle_payee], 2_565_000_000);
        assert_eq!(weights.values().sum::<u128>(), 50 * ShareCuts::SCALE);
    }

    #[test]
    fn test_bch_cut_payees_aggregate_by_script() {
        let mut window = PplnsWindow::new(Network::Regtest).for_chain(Chain::BitcoinCash);
        let address = PayoutAddress::from(Address::p2pkh(
            PubkeyHash::from_byte_array([1; 20]),
            Network::Regtest,
        ));
        let alias = PayoutAddress::from(Address::p2pkh(
            PubkeyHash::from_byte_array([1; 20]),
            Network::Bitcoin,
        ));
        window.address_keys.key_for(address);
        window.address_keys.key_for(alias.clone());

        let weights = window.collect_distribution(&[81, 19]).unwrap();

        assert_eq!(weights.len(), 1);
        assert_eq!(weights.values().copied().collect::<Vec<_>>(), vec![100]);
        assert_eq!(
            weights.keys().next().unwrap().script_pubkey(),
            alias.script_pubkey()
        );
    }

    //* Mining on the current chain. The BCH blocks are a made-up regtest
    //* tree: the paying chain c0 <- c1 <- c2 <- c3, where c3 is the paying
    //* block's BCH prev; s2, a sibling of c2; f2 <- f3, a fork off c1 that
    //* puts f3 two blocks off the chain; and n4, the block after c3.

    /// A made-up regtest BCH block on `parent` at `height`, told apart by
    /// `tag`.
    fn bch_block(parent: BlockHash, height: u32, tag: u32) -> KnownBchHeader {
        KnownBchHeader {
            height,
            header: bitcoin::block::Header {
                version: bitcoin::block::Version::from_consensus(0x2000_0000),
                prev_blockhash: parent,
                merkle_root: bitcoin::TxMerkleNode::all_zeros(),
                time: 1_700_000_000 + height * 600,
                bits: CompactTarget::from_consensus(0x207f_ffff),
                nonce: tag,
            },
        }
    }

    fn bch_tree() -> HashMap<&'static str, KnownBchHeader> {
        let c0 = bch_block(BlockHash::all_zeros(), 0, 0);
        let c1 = bch_block(c0.block_hash(), 1, 1);
        let c2 = bch_block(c1.block_hash(), 2, 2);
        let c3 = bch_block(c2.block_hash(), 3, 3);
        let s2 = bch_block(c1.block_hash(), 2, 12);
        let f2 = bch_block(c1.block_hash(), 2, 22);
        let f3 = bch_block(f2.block_hash(), 3, 23);
        let n4 = bch_block(c3.block_hash(), 4, 34);
        HashMap::from([
            ("c0", c0),
            ("c1", c1),
            ("c2", c2),
            ("c3", c3),
            ("s2", s2),
            ("f2", f2),
            ("f3", f3),
            ("n4", n4),
        ])
    }

    /// A store knowing `blocks` as BCH headers.
    fn store_knowing(blocks: impl IntoIterator<Item = KnownBchHeader>) -> MockChainStoreHandle {
        let known: HashMap<BlockHash, KnownBchHeader> = blocks
            .into_iter()
            .map(|block| (block.block_hash(), block))
            .collect();
        let mut store = MockChainStoreHandle::default();
        store
            .expect_get_bch_header()
            .returning(move |hash| Ok(known.get(hash).copied()));
        store
    }

    /// A share by `miner` whose bitcoin header builds on `bch_prev`.
    fn share_on(bch_prev: BlockHash, miner: &str, uncles: Vec<BlockHash>) -> ShareHeader {
        let mut header =
            build_test_header_with_uncles(&BlockHash::all_zeros().to_string(), miner, 2, uncles);
        header.bitcoin_header.prev_blockhash = bch_prev;
        header
    }

    /// A Bitcoin Cash window paying by the regtest chain, with no maturity
    /// skip and a cap of `max_window_shares`.
    fn current_chain_window(max_window_shares: usize) -> PplnsWindow {
        PplnsWindow::new(TEST_NETWORK)
            .for_chain(Chain::BitcoinCash)
            .with_bch_header_rules(Some(BchHeaderRules::for_network(
                crate::config::BitcoinCashNetwork::Regtest,
            )))
            .with_window_sizes(0, max_window_shares)
    }

    /// Cache `headers`, newest first, at heights counting down to 0.
    fn cache_shares(window: &mut PplnsWindow, headers: &[ShareHeader]) {
        for (index, header) in headers.iter().enumerate() {
            let height = (headers.len() - 1 - index) as u32;
            let entry = entry_from_header(window, header, height);
            window.confirmed_entries.push_back(entry);
        }
    }

    /// Heights 4 to 0 build on c3, n4, f3, s2 and the all-zero hash.
    fn shares_around_the_paying_chain(tree: &HashMap<&str, KnownBchHeader>) -> Vec<ShareHeader> {
        vec![
            share_on(tree["c3"].block_hash(), PUBKEY_G, vec![]),
            share_on(tree["n4"].block_hash(), PUBKEY_2G, vec![]),
            share_on(tree["f3"].block_hash(), PUBKEY_3G, vec![]),
            share_on(tree["s2"].block_hash(), PUBKEY_4G, vec![]),
            share_on(BlockHash::all_zeros(), PUBKEY_5G, vec![]),
        ]
    }

    /// A block on c3 pays the shares on c3, on n4 after it, on the sibling
    /// s2 and the chain's first share on the all-zero hash, but not the
    /// share on f3, two blocks off the chain. The window credits agree.
    #[test]
    fn test_bch_window_pays_only_work_on_the_paying_chain_or_one_block_off() {
        let tree = bch_tree();
        let headers = shares_around_the_paying_chain(&tree);
        let store = store_knowing(tree.values().copied());
        let paying_block = PayingBlock::new(1, tree["c3"].block_hash());
        let mut window = current_chain_window(5);
        cache_shares(&mut window, &headers);

        let distribution = window
            .get_distribution_from_start_hash(headers[0].block_hash(), paying_block, &store)
            .unwrap();
        let credits = window
            .get_window_credits_from_start_hash(headers[0].block_hash(), paying_block, &store, 10)
            .unwrap();

        let paid = |index: usize| distribution.contains_key(&headers[index].miner_bitcoin_address);
        assert!(paid(0) && paid(1) && paid(3) && paid(4));
        assert!(!paid(2));
        assert_eq!(
            distribution[&headers[0].miner_bitcoin_address],
            window.base_weight(&headers[0]) * DIFFICULTY_SCALE
        );
        assert_eq!(credits.window_share_count, 5);
        assert_eq!(credits.by_payout_address.len(), 4);
        assert!(
            !credits
                .by_payout_address
                .contains_key(&headers[2].miner_bitcoin_address)
        );
    }

    /// A share off the chain keeps its slot: with a four-share cap the window
    /// is heights 4 to 1, the share on f3 at height 2 earns nothing, and
    /// height 0 is still left out.
    #[test]
    fn test_bch_window_unpaid_share_keeps_its_slot() {
        let tree = bch_tree();
        let headers = shares_around_the_paying_chain(&tree);
        let store = store_knowing(tree.values().copied());
        let mut window = current_chain_window(4);
        cache_shares(&mut window, &headers);

        let distribution = window
            .get_distribution_from_start_hash(
                headers[0].block_hash(),
                PayingBlock::new(1, tree["c3"].block_hash()),
                &store,
            )
            .unwrap();

        let paid = |index: usize| distribution.contains_key(&headers[index].miner_bitcoin_address);
        assert!(paid(0) && paid(1) && paid(3));
        assert!(!paid(2) && !paid(4));
    }

    /// An uncle on the paying chain earns its weight even under a nephew off
    /// it; an uncle off it earns nothing; and a nephew earns an uncle's bonus
    /// only when both are paid.
    #[test]
    fn test_bch_window_pays_uncles_and_nephew_bonuses_on_the_paying_chain() {
        let tree = bch_tree();
        let uncle_on = share_on(tree["c2"].block_hash(), PUBKEY_3G, vec![]);
        let uncle_off = share_on(tree["f3"].block_hash(), PUBKEY_4G, vec![]);
        let uncle_under_off = share_on(tree["c1"].block_hash(), PUBKEY_5G, vec![]);
        let nephew = share_on(
            tree["c3"].block_hash(),
            PUBKEY_G,
            vec![uncle_on.block_hash(), uncle_off.block_hash()],
        );
        let nephew_off = share_on(
            tree["f3"].block_hash(),
            PUBKEY_2G,
            vec![uncle_under_off.block_hash()],
        );
        let store = store_knowing(tree.values().copied());
        let mut window = current_chain_window(5);
        let uncles = vec![
            uncle_entry_from_header(&mut window, &uncle_on),
            uncle_entry_from_header(&mut window, &uncle_off),
        ];
        let nephew_entry = entry_from_header_with_uncles(&mut window, &nephew, 1, uncles);
        let uncles_off = vec![uncle_entry_from_header(&mut window, &uncle_under_off)];
        let nephew_off_entry =
            entry_from_header_with_uncles(&mut window, &nephew_off, 0, uncles_off);
        window.confirmed_entries.push_back(nephew_entry);
        window.confirmed_entries.push_back(nephew_off_entry);

        let distribution = window
            .get_distribution_from_start_hash(
                nephew.block_hash(),
                PayingBlock::new(1, tree["c3"].block_hash()),
                &store,
            )
            .unwrap();

        let weight = |header: &ShareHeader| window.base_weight(header);
        assert_eq!(
            distribution[&nephew.miner_bitcoin_address],
            weight(&nephew) * DIFFICULTY_SCALE + weight(&uncle_on) * NEPHEW_SCALED_BONUS
        );
        assert_eq!(
            distribution[&uncle_on.miner_bitcoin_address],
            weight(&uncle_on) * UNCLE_SCALED_WEIGHT
        );
        assert_eq!(
            distribution[&uncle_under_off.miner_bitcoin_address],
            weight(&uncle_under_off) * UNCLE_SCALED_WEIGHT
        );
        assert!(!distribution.contains_key(&uncle_off.miner_bitcoin_address));
        assert!(!distribution.contains_key(&nephew_off.miner_bitcoin_address));
    }

    /// A block on a chain none of the window's work is on would pay no one,
    /// which the distribution and the window credits both refuse.
    #[test]
    fn test_bch_window_paying_no_one_is_refused() {
        let tree = bch_tree();
        let headers = vec![
            share_on(tree["f3"].block_hash(), PUBKEY_G, vec![]),
            share_on(tree["f3"].block_hash(), PUBKEY_2G, vec![]),
        ];
        let store = store_knowing(tree.values().copied());
        let paying_block = PayingBlock::new(1, tree["c3"].block_hash());
        let mut window = current_chain_window(5);
        cache_shares(&mut window, &headers);

        let error = window
            .get_distribution_from_start_hash(headers[0].block_hash(), paying_block, &store)
            .unwrap_err();
        assert!(
            matches!(
                error,
                WindowError::NothingEligible { bch_prev, .. } if bch_prev == tree["c3"].block_hash()
            ),
            "{error}"
        );
        let error = window
            .get_window_credits_from_start_hash(headers[0].block_hash(), paying_block, &store, 10)
            .unwrap_err();
        assert!(
            matches!(error, WindowError::NothingEligible { .. }),
            "{error}"
        );
    }

    /// The paying block's prev, or a window share's, that is not a known
    /// header leaves the window unresolved until it arrives.
    #[test]
    fn test_bch_window_waits_for_unknown_bch_blocks() {
        let tree = bch_tree();
        let headers = shares_around_the_paying_chain(&tree);
        let without = |missing: &str| {
            store_knowing(
                tree.iter()
                    .filter(|(name, _)| **name != missing)
                    .map(|(_, block)| *block),
            )
        };
        let mut window = current_chain_window(5);
        cache_shares(&mut window, &headers);

        for missing in ["c3", "s2"] {
            let error = window
                .get_distribution_from_start_hash(
                    headers[0].block_hash(),
                    PayingBlock::new(1, tree["c3"].block_hash()),
                    &without(missing),
                )
                .unwrap_err();
            assert!(
                matches!(
                    error,
                    WindowError::UnknownBchPrev { block } if block == tree[missing].block_hash()
                ),
                "{missing}: {error}"
            );
        }
    }

    /// The window is sized by the nBits Bitcoin Cash requires after the
    /// paying block's prev, on regtest the prev's own, whatever the paying
    /// block claims. A prev at the share difficulty sizes three shares, as in
    /// `test_bch_window_large_pool_pays_three_blocks_of_work`, where the
    /// claimed difficulty of 1 alone pays the five-share cap.
    #[test]
    fn test_bch_window_is_sized_by_the_paying_prev_not_the_claim() {
        let miners = [PUBKEY_G, PUBKEY_2G, PUBKEY_3G, PUBKEY_4G, PUBKEY_5G];
        let mut prev = bch_block(BlockHash::all_zeros(), 3, 7);
        let mut headers: Vec<ShareHeader> = miners
            .iter()
            .map(|miner| share_on(BlockHash::all_zeros(), miner, vec![]))
            .collect();
        prev.header.bits = headers[0].bits;
        for header in &mut headers {
            header.bitcoin_header.prev_blockhash = prev.block_hash();
        }
        let store = store_knowing([prev]);
        let mut window = current_chain_window(5);
        cache_shares(&mut window, &headers);

        let sized = window
            .get_distribution_from_start_hash(
                headers[0].block_hash(),
                PayingBlock::new(1, prev.block_hash()),
                &store,
            )
            .unwrap();
        let claimed_only = window
            .get_distribution_from_start_hash(headers[0].block_hash(), 1_u128.into(), &store)
            .unwrap();

        assert_eq!(sized.len(), 3);
        assert_eq!(claimed_only.len(), 5);
    }
}
