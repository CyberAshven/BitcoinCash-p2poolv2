// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Validation of weak shares (`docs/sharechain/small-miners-design.md`,
//! part 3, "What a weak share is" and "How a node checks one").
//!
//! A weak share is valid when:
//!
//! 1. its `bits` is exactly the minimum ASERT gives from its parent: a job
//!    at the minimum;
//! 2. its proof-of-work hash meets its parent's weak target and misses its
//!    own `bits`;
//! 3. its Bitcoin Cash coinbase, rebuilt from its scriptSig, total value and
//!    header as a share's on that parent, folds up its merkle branch to its
//!    header's merkle root;
//! 4. it passes every header rule a full share passes but the share target;
//! 5. its proof-of-work hash is credited nowhere else on the chain carrying
//!    it.
//!
//! A share carrying weak shares must also commit to exactly them, carry each
//! with its parent at most `WEAK_DEPTH` shares below it, and carry none an
//! ancestor of it already carries. Its header's `weak_asking` must be the
//! number of them that ask for an easier weak target
//! (`shares::weak::weak_share_asks`, part 6 of the design page), and it may
//! carry more than `WEAK_IDLE_CAP` of them only while more than one in
//! `WEAK_ASK_DIVISOR` ask (`shares::weak::weak_count_allowed`). These are
//! share-chain rules; a Bitcoin Cash node never sees them.

#[cfg(test)]
mod tests;

use super::{
    ChainStoreHandle, DefaultShareValidator, MAX_UNCLES, PplnsWindow, ValidationError,
    check_carried_weak_time, check_median_time_past, check_relayed_weak_time,
    stored_ancestor_times, window_error_verdict,
};
use crate::accounting::CoinbasePayouts;
use crate::accounting::payout::coinbase_budget::split_for_share;
use crate::accounting::payout::payout_levels::PplnsWeights;
use crate::accounting::payout::sharechain_pplns::pplns_window::{
    PayingBlock, bitcoin_block_difficulty,
};
use crate::config::Chain;
use crate::shares::band::{ParentBand, check_share_band};
use crate::shares::share_block::{ShareBlock, ShareHeader, is_terminal_blockhash};
use crate::shares::share_commitment::ShareCommitment;
use crate::shares::share_version::check_share_version_alone;
use crate::shares::weak::{
    WEAK_BYTES, WEAK_CAP, WEAK_DEPTH, WeakShare, parent_weak_target, weak_cap_for_asking,
    weak_count_allowed, weak_root, weak_share_asks,
};
use crate::sim_overrides;
use crate::store::writer::StoreError;
use crate::stratum::work::coinbase::check_bch_coinbase_limit;
use crate::stratum::work::gbt::compute_merkle_root_from_branches;
use crate::utils::parallel::try_map_in_parallel;
use bitcoin::consensus::Encodable;
use bitcoin::{
    BlockHash, OutPoint, Sequence, Target, Transaction, TxIn, Witness, absolute::LockTime,
    transaction::Version,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, RwLock};

/// The most expected payouts a [`WeakPayoutCache`] keeps before it starts
/// over: four templates on each of the `WEAK_DEPTH` parents a weak share
/// may build on.
const WEAK_PAYOUT_CACHE_CAPACITY: usize = 4 * WEAK_DEPTH as usize;

/// The most carried weak shares whose payouts are taken under one hold of
/// the PPLNS window, before the window is let go and their coinbases are
/// checked on several threads
/// ([`DefaultShareValidator::validate_carried_weak_coinbases`]). It bounds
/// the payouts kept besides the cache's, however many weak shares a share
/// carries.
const WEAK_COINBASE_BATCH: usize = 64;

/// What a weak share's expected payouts depend on besides the window's
/// distribution: its parent (the window's anchor), the paying header's bits
/// and Bitcoin Cash prev, the coinbase's total value, its merge-mining output
/// and its own donation and fee. Weak shares on the same template share all
/// of them, so they share one rebuild of the window's payouts.
#[derive(Clone, PartialEq, Eq, Hash)]
struct WeakPayoutKey {
    parent: BlockHash,
    bitcoin_bits: u32,
    bch_prev: BlockHash,
    coinbase_value: u64,
    merge_mining: Vec<u8>,
    cuts: Vec<u8>,
}

impl WeakPayoutKey {
    fn of(weak_share: &WeakShare) -> Self {
        let header = &weak_share.header;
        let mut merge_mining = Vec::with_capacity(64);
        header
            .merge_mining_commitment()
            .consensus_encode(&mut merge_mining)
            .expect("encoding into a vector never fails");
        let mut cuts = Vec::with_capacity(64);
        let cut_fields = (
            header
                .donation_address
                .as_ref()
                .map(|address| address.script_pubkey()),
            header.donation.unwrap_or(0),
            header
                .fee_address
                .as_ref()
                .map(|address| address.script_pubkey()),
            header.fee.unwrap_or(0),
        );
        if let Some(script) = &cut_fields.0 {
            cuts.extend_from_slice(script.as_bytes());
        }
        cuts.extend_from_slice(&cut_fields.1.to_le_bytes());
        if let Some(script) = &cut_fields.2 {
            cuts.extend_from_slice(script.as_bytes());
        }
        cuts.extend_from_slice(&cut_fields.3.to_le_bytes());
        Self {
            parent: header.prev_share_blockhash,
            bitcoin_bits: header.bitcoin_header.bits.to_consensus(),
            bch_prev: header.bitcoin_header.prev_blockhash,
            coinbase_value: weak_share.coinbase_value,
            merge_mining,
            cuts,
        }
    }
}

impl WeakPayoutKey {
    /// The part of the key the window's weights depend on.
    fn weights_key(&self) -> WeakWeightsKey {
        WeakWeightsKey {
            parent: self.parent,
            bitcoin_bits: self.bitcoin_bits,
            bch_prev: self.bch_prev,
        }
    }
}

/// The most window weights a [`WeakPayoutCache`] keeps before it starts
/// over: two Bitcoin Cash blocks on each of the `WEAK_DEPTH` parents a weak
/// share may build on, so a share carrying weak shares from deep parents
/// reuses the walks the shares below it made.
const WEAK_WEIGHTS_CACHE_CAPACITY: usize = 2 * WEAK_DEPTH as usize;

/// What the PPLNS window's weights for a weak share depend on: its parent,
/// the window's anchor, and the Bitcoin Cash block its coinbase pays, by its
/// bits and prev. Weak shares on every template of that block share them,
/// whatever their coinbase value, cuts or merge-mining output, so they share
/// one walk of the window.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct WeakWeightsKey {
    parent: BlockHash,
    bitcoin_bits: u32,
    bch_prev: BlockHash,
}

/// The PPLNS window's weights for weak shares by parent and paid block
/// ([`WeakWeightsKey`]), and the expected payouts built from them by
/// template ([`WeakPayoutKey`]): weak shares on many templates of one block
/// cost one walk of the window, and each weak share one coinbase rebuild and
/// hash.
#[derive(Default)]
pub struct WeakPayoutCache {
    weights: HashMap<WeakWeightsKey, Arc<PplnsWeights>>,
    payouts: HashMap<WeakPayoutKey, Arc<CoinbasePayouts>>,
    /// The weights keys in the order they were cached, oldest first.
    weights_order: VecDeque<WeakWeightsKey>,
    /// The payout keys in the order they were cached, oldest first.
    payouts_order: VecDeque<WeakPayoutKey>,
}

impl WeakPayoutCache {
    pub fn new() -> Self {
        Self {
            weights: HashMap::with_capacity(WEAK_WEIGHTS_CACHE_CAPACITY),
            payouts: HashMap::with_capacity(WEAK_PAYOUT_CACHE_CAPACITY),
            weights_order: VecDeque::with_capacity(WEAK_WEIGHTS_CACHE_CAPACITY),
            payouts_order: VecDeque::with_capacity(WEAK_PAYOUT_CACHE_CAPACITY),
        }
    }

    /// The cached payouts for `key`, or `build_outputs`' from the window's
    /// weights, cached by [`WeakPayoutKey::weights_key`] or else
    /// `walk_window`'s; both kept for the next weak share. A full cache
    /// drops its oldest entry: only node memory, never a verdict, depends on
    /// what it keeps.
    fn get_or_build(
        &mut self,
        key: WeakPayoutKey,
        walk_window: impl FnOnce() -> Result<PplnsWeights, ValidationError>,
        build_outputs: impl FnOnce(&PplnsWeights) -> Result<CoinbasePayouts, ValidationError>,
    ) -> Result<Arc<CoinbasePayouts>, ValidationError> {
        if let Some(payouts) = self.payouts.get(&key) {
            return Ok(Arc::clone(payouts));
        }
        let weights = self.weights_or_walk(key.weights_key(), walk_window)?;
        let payouts = Arc::new(build_outputs(&weights)?);
        if self.payouts.len() >= WEAK_PAYOUT_CACHE_CAPACITY
            && let Some(oldest) = self.payouts_order.pop_front()
        {
            self.payouts.remove(&oldest);
        }
        self.payouts_order.push_back(key.clone());
        self.payouts.insert(key, Arc::clone(&payouts));
        Ok(payouts)
    }

    /// The cached window weights for `key`, or `walk_window`'s, kept for the
    /// next weak share.
    fn weights_or_walk(
        &mut self,
        key: WeakWeightsKey,
        walk_window: impl FnOnce() -> Result<PplnsWeights, ValidationError>,
    ) -> Result<Arc<PplnsWeights>, ValidationError> {
        if let Some(weights) = self.weights.get(&key) {
            return Ok(Arc::clone(weights));
        }
        let weights = Arc::new(walk_window()?);
        if self.weights.len() >= WEAK_WEIGHTS_CACHE_CAPACITY
            && let Some(oldest) = self.weights_order.pop_front()
        {
            self.weights.remove(&oldest);
        }
        self.weights_order.push_back(key);
        self.weights.insert(key, Arc::clone(&weights));
        Ok(weights)
    }
}

/// The body check of a share's weak shares, against its header alone: as
/// many as `weak_count`, no proof-of-work hash twice, and the merkle root of
/// their whole encodings (`weak_root`) the header's `weak_root`. A failure is
/// a bad copy of the share, as for its other body parts.
///
/// The root binds every field of every carried weak share, so a copy that
/// alters any of them fails here, before it is stored, and is fetched again.
/// The weak share checks that run once the body is stored
/// ([`DefaultShareValidator::validate_carried_weak_headers`] and
/// [`DefaultShareValidator::validate_carried_weak_coinbases`]) then read only
/// what the carrier's proof of work fixes, the store's own data aside, so
/// their failure is a verdict on the share and never a bad copy.
///
/// A repeat is refused before the root is compared: the tree pairs the
/// last leaf of an odd level with itself, so a repeated last weak share
/// would keep the root (CVE-2012-2459). It compares proof-of-work hashes,
/// so two encodings of one proof of work are a repeat too.
pub(crate) fn check_weak_shares_match_header(share: &ShareBlock) -> Result<(), ValidationError> {
    let header = &share.header;
    if share.weak_shares.len() != usize::from(header.weak_count) {
        return Err(ValidationError::consensus(format!(
            "Share carries {} weak shares, its header commits {}",
            share.weak_shares.len(),
            header.weak_count
        )));
    }
    let mut seen = HashSet::with_capacity(share.weak_shares.len());
    if let Some(repeated) = share
        .weak_shares
        .iter()
        .map(WeakShare::pow_hash)
        .find(|pow_hash| !seen.insert(*pow_hash))
    {
        return Err(ValidationError::consensus(format!(
            "Weak share {repeated} is carried more than once"
        )));
    }
    let root = weak_root(&share.weak_shares);
    if root != header.weak_root {
        return Err(ValidationError::consensus(format!(
            "Carried weak shares have root {root}, the header commits {}",
            header.weak_root
        )));
    }
    Ok(())
}

/// A share carries at most `WEAK_CAP` weak shares in `WEAK_BYTES`.
pub(crate) fn check_weak_shares_size(weak_shares: &[WeakShare]) -> Result<(), ValidationError> {
    if weak_shares.len() > WEAK_CAP {
        return Err(ValidationError::consensus(format!(
            "Share carries {} weak shares, more than {WEAK_CAP}",
            weak_shares.len()
        )));
    }
    let bytes: usize = weak_shares.iter().map(WeakShare::encoded_size).sum();
    if bytes > WEAK_BYTES {
        return Err(ValidationError::consensus(format!(
            "Share carries {bytes} bytes of weak shares, more than {WEAK_BYTES}"
        )));
    }
    Ok(())
}

/// A share's header commits how many of the weak shares it carries ask for
/// an easier weak target, `weak_asking`, which moves the next share's weak
/// ratio (`shares::weak::next_weak_ratio`): it must be `asking`, the number
/// of carried weak shares whose `wanted_bits` is easier than the weak target
/// each met (`shares::weak::weak_share_asks`).
///
/// Every node counts the same: a carried weak share's `wanted_bits` is fixed
/// by the carrier's weak root, and the weak target it met by its parent's
/// header. So a share can neither claim asks its weak shares do not make
/// nor hide the ones they do.
fn check_weak_asking_matches_header(
    header: &ShareHeader,
    asking: usize,
) -> Result<(), ValidationError> {
    if usize::from(header.weak_asking) != asking {
        return Err(ValidationError::consensus(format!(
            "Share header commits {} asking weak shares, its body carries {asking}",
            header.weak_asking
        )));
    }
    check_weak_count_allowed(usize::from(header.weak_count), asking)
}

/// A share carries more than `WEAK_IDLE_CAP` weak shares only while more
/// than one in `WEAK_ASK_DIVISOR` of them ask
/// (`shares::weak::weak_count_allowed`). A header breaking it does not
/// decode; this holds a share built in this node to it too.
fn check_weak_count_allowed(weak_count: usize, asking: usize) -> Result<(), ValidationError> {
    if !weak_count_allowed(weak_count, asking) {
        return Err(ValidationError::consensus(format!(
            "Share carries {weak_count} weak shares with {asking} asking, more than the {} they allow",
            weak_cap_for_asking(asking)
        )));
    }
    Ok(())
}

/// The coinbase a weak share carries as far as a rebuild needs it: one input
/// with its scriptSig, no outputs.
fn carried_script_sig(weak_share: &WeakShare) -> Transaction {
    Transaction {
        version: Version::TWO,
        lock_time: LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::null(),
            script_sig: weak_share.coinbase_script_sig.clone(),
            sequence: Sequence::MAX,
            witness: Witness::new(),
        }],
        output: Vec::new(),
    }
}

/// The store read a weak share check could not make: `Recoverable` while
/// the data can still arrive, `StoreAccess` for any other failure.
fn weak_read_error(what: &str, error: StoreError) -> ValidationError {
    match error {
        StoreError::NotFound(_) => {
            ValidationError::recoverable(format!("Weak share check cannot read {what}: {error}"))
        }
        _ => ValidationError::store_access(format!(
            "Weak share check failed to read {what}: {error}"
        )),
    }
}

impl DefaultShareValidator {
    /// Rules 1, 2 and 4 of a weak share: every header rule a full share
    /// passes but the share target, its bits the minimum from its parent,
    /// and its hash meeting its parent's weak target while missing its own
    /// bits. Needs the parent header and its ancestry, not the PPLNS window.
    ///
    /// A parent, ancestor or Bitcoin Cash prev not stored yet is
    /// `Recoverable`.
    ///
    /// The future limit of its time is not checked here, because it depends on
    /// who asks: a relayed weak share is held to this node's clock
    /// ([`Self::validate_relayed_weak_share`]) and a carried one to its
    /// carrier's time ([`Self::validate_carried_weak_headers`]).
    pub fn validate_weak_share_header(
        &self,
        weak_share: &WeakShare,
        chain_store_handle: &ChainStoreHandle,
    ) -> Result<(), ValidationError> {
        self.validate_weak_share_header_for_target(weak_share, chain_store_handle)
            .map(|_weak_target| ())
    }

    /// [`Self::validate_weak_share_header`], giving the weak target the weak
    /// share met, its parent's: what tells whether it asks for an easier one
    /// (`shares::weak::weak_share_asks`), with no second read of its parent.
    fn validate_weak_share_header_for_target(
        &self,
        weak_share: &WeakShare,
        chain_store_handle: &ChainStoreHandle,
    ) -> Result<Target, ValidationError> {
        let header = &weak_share.header;
        self.validate_weak_share_alone(weak_share)?;

        let parent_hash = header.prev_share_blockhash;
        if is_terminal_blockhash(&parent_hash) {
            return Err(ValidationError::consensus(
                "A weak share must be mined on a share",
            ));
        }
        let asert_parent = chain_store_handle
            .get_asert_parent(&parent_hash)
            .map_err(|error| weak_read_error("the weak share's parent", error))?;
        let minimum = self.pool_difficulty.calculate_target_clamped(asert_parent);
        if header.asert_bits != minimum {
            return Err(ValidationError::consensus(format!(
                "Weak share minimum {:#010x} is not the {:#010x} ASERT gives from its parent",
                header.asert_bits.to_consensus(),
                minimum.to_consensus()
            )));
        }
        if header.bits != header.asert_bits {
            return Err(ValidationError::consensus(format!(
                "Weak share bits {:#010x} are not its minimum {:#010x}: weak shares come only from jobs at the minimum",
                header.bits.to_consensus(),
                header.asert_bits.to_consensus()
            )));
        }
        let parent = chain_store_handle
            .get_share_header(&parent_hash)
            .map_err(|error| weak_read_error("the weak share's parent", error))?;
        check_share_band(header, &ParentBand::from_header(&parent)).map_err(|error| {
            ValidationError::consensus(format!(
                "Weak share is outside the difficulty band: {error}"
            ))
        })?;
        let weak_target = self.validate_weak_share_proof_of_work(weak_share, &parent)?;

        self.validate_share_version(header, chain_store_handle)?;
        self.validate_bch_prev_known(header, chain_store_handle)?;
        let ancestor_times = stored_ancestor_times(parent_hash, chain_store_handle)?;
        check_median_time_past(header.time, &ancestor_times)?;
        Ok(weak_target)
    }

    /// The header rules of a weak share that need no store: a Bitcoin Cash
    /// share owner, payout scripts, donation and fee caps, uncle count, a
    /// target no easier than the ceiling, the share version on its own, the
    /// token share ASERT check, and the coinbase proof that ties the header's
    /// commitment to its merkle root through its branch.
    fn validate_weak_share_alone(&self, weak_share: &WeakShare) -> Result<(), ValidationError> {
        let header = &weak_share.header;
        if self.chain != Chain::BitcoinCash {
            return Err(ValidationError::consensus(
                "Weak shares are a Bitcoin Cash share-chain rule",
            ));
        }
        self.validate_miner_address(header)?;
        self.validate_payout_cuts(header)?;
        self.validate_payout_scripts(header)?;
        if header.uncles.len() > MAX_UNCLES {
            return Err(ValidationError::consensus(format!(
                "Weak share names {} uncles, more than {MAX_UNCLES}",
                header.uncles.len()
            )));
        }
        if Target::from_compact(header.bits) > Target::from_compact(self.target_ceiling) {
            return Err(ValidationError::consensus(
                "Weak share bits are easier than the share chain's ceiling",
            ));
        }
        check_share_version_alone(header.share_version).map_err(|error| {
            ValidationError::consensus(format!("Invalid share version: {error}"))
        })?;
        self.validate_token_share_asert(header)?;
        super::ShareValidator::validate_coinbase_proof(self, header, &weak_share.merkle_branch)
    }

    /// Rule 2: the weak share's hash meets `parent`'s weak target and misses
    /// its own bits. A hash that meets its bits is a full share and travels
    /// as one, so one proof of work is credited once. Gives the weak target
    /// it met.
    fn validate_weak_share_proof_of_work(
        &self,
        weak_share: &WeakShare,
        parent: &ShareHeader,
    ) -> Result<Target, ValidationError> {
        let pow_hash = weak_share.pow_hash();
        let weak_target = parent_weak_target(parent, self.target_ceiling);
        if !sim_overrides::pow_meets(weak_target, pow_hash) {
            return Err(ValidationError::consensus(format!(
                "Weak share hash {pow_hash} misses its parent's weak target {weak_target}"
            )));
        }
        let share_target = Target::from_compact(weak_share.header.bits);
        if !sim_overrides::pow_misses(share_target, pow_hash) {
            return Err(ValidationError::consensus(format!(
                "Weak share hash {pow_hash} meets its share target {share_target}: it is a full share"
            )));
        }
        Ok(weak_target)
    }

    /// Rule 3: the weak share's Bitcoin Cash coinbase pays the PPLNS window
    /// anchored at its parent, exactly as a full share's must. The node
    /// rebuilds it from the scriptSig the weak share carries, the window's
    /// payouts for its total value, its header's merge-mining output, payout
    /// tree, padding and commitment, and the locktime from its height, then
    /// hashes it and folds the carried merkle branch, which must give the
    /// header's merkle root.
    ///
    /// The window's weights come from `cache` when another weak share on the
    /// same parent and Bitcoin Cash block already walked the window, and its
    /// payouts when one on the same template already built them.
    pub fn validate_weak_share_coinbase(
        &self,
        weak_share: &WeakShare,
        chain_store_handle: &ChainStoreHandle,
        window: &mut PplnsWindow,
        cache: &mut WeakPayoutCache,
    ) -> Result<(), ValidationError> {
        let payouts = self.weak_share_payouts(weak_share, chain_store_handle, window, cache)?;
        self.check_weak_share_pays(weak_share, &payouts)
    }

    /// The payouts rule 3 rebuilds a weak share's coinbase with: the PPLNS
    /// window's anchored at its parent, for its total value, cuts and
    /// merge-mining output, from `cache` or a walk of `window`. The only part
    /// of rule 3 that needs the window.
    fn weak_share_payouts(
        &self,
        weak_share: &WeakShare,
        chain_store_handle: &ChainStoreHandle,
        window: &mut PplnsWindow,
        cache: &mut WeakPayoutCache,
    ) -> Result<Arc<CoinbasePayouts>, ValidationError> {
        let header = &weak_share.header;
        let split = split_for_share(self.chain, header);
        cache.get_or_build(
            WeakPayoutKey::of(weak_share),
            || {
                let paying_block = PayingBlock::new(
                    bitcoin_block_difficulty(header.bitcoin_header.bits, window.network()),
                    header.bitcoin_header.prev_blockhash,
                );
                window
                    .get_weights_from_start_hash(
                        header.prev_share_blockhash,
                        paying_block,
                        chain_store_handle,
                    )
                    .map_err(window_error_verdict)
            },
            |weights| {
                Self::build_expected_outputs(header, weights, weak_share.coinbase_value, split)
            },
        )
    }

    /// The rest of rule 3, against the window's `payouts`
    /// ([`Self::weak_share_payouts`]): the coinbase rebuilt from them and the
    /// weak share's scriptSig, within Bitcoin Cash's coinbase limit, folds up
    /// its branch to its header's merkle root. Reads neither the window nor
    /// the store, so it runs with the window let go, on any thread.
    fn check_weak_share_pays(
        &self,
        weak_share: &WeakShare,
        payouts: &CoinbasePayouts,
    ) -> Result<(), ValidationError> {
        let header = &weak_share.header;
        let split = split_for_share(self.chain, header);
        let commitment_hash = ShareCommitment::from_share_header_and_root(
            header,
            header.coinbase_proof.share_witness_root,
        )
        .hash();
        let coinbase = self.rebuild_bitcoin_coinbase(
            header,
            &carried_script_sig(weak_share),
            payouts,
            commitment_hash,
        )?;
        check_bch_coinbase_limit(&coinbase, split)
            .map_err(|error| ValidationError::consensus(error.to_string()))?;
        let root =
            compute_merkle_root_from_branches(coinbase.compute_txid(), &weak_share.merkle_branch);
        if root != header.bitcoin_header.merkle_root {
            return Err(ValidationError::consensus(format!(
                "Weak share {} does not pay its PPLNS window: its rebuilt coinbase gives merkle root {root}, not its header's",
                weak_share.pow_hash()
            )));
        }
        Ok(())
    }

    /// Every check of a relayed weak share that does not depend on a
    /// carrier: rules 1 to 4 ([`Self::validate_weak_share_header`] and
    /// [`Self::validate_weak_share_coinbase`]). Rule 5 and the depth rule are
    /// the carrier's ([`Self::validate_carried_weak_positions`]); the weak
    /// pool applies them against the best chain.
    ///
    /// The window is held only while the weak share's payouts are taken; its
    /// coinbase is rebuilt and hashed after the window is let go.
    pub fn validate_relayed_weak_share(
        &self,
        weak_share: &WeakShare,
        chain_store_handle: &ChainStoreHandle,
        pplns_window: &RwLock<PplnsWindow>,
        cache: &mut WeakPayoutCache,
    ) -> Result<(), ValidationError> {
        self.validate_weak_share_header(weak_share, chain_store_handle)?;
        check_relayed_weak_time(
            weak_share.header.time,
            self.time_provider.seconds_since_epoch(),
        )?;
        let payouts = self.weak_share_payouts(
            weak_share,
            chain_store_handle,
            &mut pplns_window
                .write()
                .expect("PPLNS window lock poisoned on write"),
            cache,
        )?;
        self.check_weak_share_pays(weak_share, &payouts)
    }

    /// Where a share may carry its weak shares: each one's parent at most
    /// `WEAK_DEPTH` shares below the carrier and below it, and none already
    /// carried by one of the carrier's ancestors, so a weak share is
    /// credited once on a chain (rule 5), as an uncle has one nephew.
    ///
    /// An ancestor that carried a weak share sits above that weak share's
    /// parent, so the walk reads at most `WEAK_DEPTH - 1` ancestors, and of
    /// those only the ones whose headers count carried weak shares. A weak
    /// share's hash can be no full share's or uncle's: a full share with
    /// that hash would have the same header, whose bits the hash misses.
    pub(crate) fn validate_carried_weak_positions(
        &self,
        share: &ShareBlock,
        chain_store_handle: &ChainStoreHandle,
    ) -> Result<(), ValidationError> {
        if share.weak_shares.is_empty() {
            return Ok(());
        }
        let carrier_parent = share.header.prev_share_blockhash;
        let mut hashes = Vec::with_capacity(1 + share.weak_shares.len());
        hashes.push(carrier_parent);
        hashes.extend(
            share
                .weak_shares
                .iter()
                .map(|weak_share| weak_share.header.prev_share_blockhash),
        );
        let heights: HashMap<BlockHash, u32> = chain_store_handle
            .get_block_metadata_batch(&hashes)
            .map_err(|error| {
                ValidationError::store_access(format!(
                    "Failed to read metadata for the weak share depth check: {error}"
                ))
            })?
            .into_iter()
            .filter_map(|(blockhash, metadata)| Some((blockhash, metadata.expected_height?)))
            .collect();
        let carrier_height = heights
            .get(&carrier_parent)
            .map(|height| height + 1)
            .ok_or_else(|| {
                ValidationError::recoverable(format!(
                    "Parent {carrier_parent} has no height for the weak share depth check"
                ))
            })?;
        for weak_share in &share.weak_shares {
            let parent = weak_share.header.prev_share_blockhash;
            let parent_height = heights.get(&parent).ok_or_else(|| {
                ValidationError::recoverable(format!(
                    "Weak share parent {parent} has no height for the depth check"
                ))
            })?;
            let depth = carrier_height.saturating_sub(*parent_height);
            if depth == 0 || depth > WEAK_DEPTH {
                return Err(ValidationError::consensus(format!(
                    "Weak share {} has its parent {depth} shares below its carrier, not 1 to {WEAK_DEPTH}",
                    weak_share.pow_hash()
                )));
            }
        }
        let carried_by_ancestors =
            self.weak_shares_carried_by_ancestors(carrier_parent, chain_store_handle)?;
        match share
            .weak_shares
            .iter()
            .map(WeakShare::pow_hash)
            .find(|pow_hash| carried_by_ancestors.contains(pow_hash))
        {
            Some(carried) => Err(ValidationError::consensus(format!(
                "Weak share {carried} is already carried by an ancestor of its carrier"
            ))),
            None => Ok(()),
        }
    }

    /// The proof-of-work hashes of the weak shares the `WEAK_DEPTH - 1`
    /// shares ending at `parent_hash` carry, following parent pointers.
    fn weak_shares_carried_by_ancestors(
        &self,
        parent_hash: BlockHash,
        chain_store_handle: &ChainStoreHandle,
    ) -> Result<HashSet<BlockHash>, ValidationError> {
        let ancestor_count = WEAK_DEPTH as usize - 1;
        let mut carriers = Vec::with_capacity(ancestor_count);
        let mut current = parent_hash;
        let mut steps = 0;
        while steps < ancestor_count && !is_terminal_blockhash(&current) {
            let header = chain_store_handle
                .get_share_header(&current)
                .map_err(|error| weak_read_error("an ancestor of the carrier", error))?;
            if header.weak_count > 0 {
                carriers.push(current);
            }
            current = header.prev_share_blockhash;
            steps += 1;
        }
        if carriers.is_empty() {
            return Ok(HashSet::new());
        }
        let summaries = chain_store_handle
            .get_weak_summaries(&carriers)
            .map_err(|error| weak_read_error("the weak shares of an ancestor", error))?;
        if summaries.len() != carriers.len() {
            return Err(ValidationError::recoverable(
                "The weak shares of an ancestor of the carrier are not stored yet",
            ));
        }
        Ok(summaries
            .into_values()
            .flat_map(|summary| summary.pow_hashes)
            .collect())
    }

    /// The store checks of the weak shares a share carries, run with its
    /// other parallel checks: each one's header rules
    /// ([`Self::validate_weak_share_header`]), how many of them ask
    /// ([`check_weak_asking_matches_header`]) and where each may be carried
    /// ([`Self::validate_carried_weak_positions`]).
    ///
    /// Runs on a stored body, which passed [`check_weak_shares_match_header`]:
    /// the carrier's weak root fixes every weak share it reads, so a
    /// `Consensus` failure here is a verdict on what the miner carried.
    ///
    /// The weak shares' header rules read only the store, each for its own
    /// weak share, so they run on several threads ([`try_map_in_parallel`]);
    /// the verdict is the first failing weak share's, in carried order, as
    /// checking them one by one gives.
    pub(crate) fn validate_carried_weak_headers(
        &self,
        share: &ShareBlock,
        chain_store_handle: &ChainStoreHandle,
    ) -> Result<(), ValidationError> {
        let weak_targets = try_map_in_parallel(&share.weak_shares, |weak_share| {
            check_carried_weak_time(weak_share.header.time, share.header.time)?;
            self.validate_weak_share_header_for_target(weak_share, chain_store_handle)
        })?;
        let asking = share
            .weak_shares
            .iter()
            .zip(weak_targets)
            .filter(|(weak_share, weak_target)| weak_share_asks(&weak_share.header, *weak_target))
            .count();
        check_weak_asking_matches_header(&share.header, asking)?;
        self.validate_carried_weak_positions(share, chain_store_handle)
    }

    /// The coinbase check of every weak share a share carries
    /// ([`Self::validate_weak_share_coinbase`]), with one payout cache for
    /// the share.
    ///
    /// Runs on a stored body, which passed [`check_weak_shares_match_header`]:
    /// the scriptSig, total value, branch and header each weak share's
    /// coinbase is rebuilt from are all fixed by the carrier's weak root, so a
    /// `Consensus` failure here is a verdict on what the miner carried.
    ///
    /// The weak shares go in batches of [`WEAK_COINBASE_BATCH`]: the window is
    /// held while a batch's payouts are taken ([`Self::batch_payouts`]), then
    /// let go while their coinbases are rebuilt and hashed on several threads
    /// ([`try_map_in_parallel`]). The verdict is the first failing weak
    /// share's, in carried order, as checking them one by one gives.
    pub(crate) fn validate_carried_weak_coinbases(
        &self,
        share: &ShareBlock,
        chain_store_handle: &ChainStoreHandle,
        pplns_window: &Arc<RwLock<PplnsWindow>>,
    ) -> Result<(), ValidationError> {
        if share.weak_shares.is_empty() {
            return Ok(());
        }
        let mut cache = WeakPayoutCache::new();
        for batch in share.weak_shares.chunks(WEAK_COINBASE_BATCH) {
            let (payouts, payouts_outcome) =
                self.batch_payouts(batch, chain_store_handle, pplns_window, &mut cache);
            let paid: Vec<(&WeakShare, Arc<CoinbasePayouts>)> = batch.iter().zip(payouts).collect();
            try_map_in_parallel(&paid, |(weak_share, payouts)| {
                self.check_weak_share_pays(weak_share, payouts)
            })?;
            payouts_outcome?;
        }
        Ok(())
    }

    /// The payouts of `batch`'s weak shares in order, from `cache` or the
    /// window, held once for the whole batch, up to the first weak share whose
    /// payouts cannot be had, and that one's failure.
    ///
    /// The window is the only lock taken here, and it is let go before any
    /// coinbase of the batch is checked, so no thread checking a coinbase ever
    /// waits for it.
    fn batch_payouts(
        &self,
        batch: &[WeakShare],
        chain_store_handle: &ChainStoreHandle,
        pplns_window: &RwLock<PplnsWindow>,
        cache: &mut WeakPayoutCache,
    ) -> (Vec<Arc<CoinbasePayouts>>, Result<(), ValidationError>) {
        let mut window = pplns_window
            .write()
            .expect("PPLNS window lock poisoned on write");
        let mut payouts = Vec::with_capacity(batch.len());
        let outcome = batch.iter().try_for_each(|weak_share| {
            payouts.push(self.weak_share_payouts(
                weak_share,
                chain_store_handle,
                &mut window,
                cache,
            )?);
            Ok(())
        });
        (payouts, outcome)
    }
}
