// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The pool of Bitcoin Cash share transactions waiting for a share.
//!
//! A node takes signed share transactions from its API and from peers,
//! checks them against the confirmed share chain, relays the ones it accepts
//! and puts them in the shares it builds. The pool is bounded in count, bytes
//! and age, and holds only transactions whose inputs spend confirmed,
//! unspent outputs, so it never holds a chain of unconfirmed transactions.
//! See `docs/atomic-swap/bch-share-trading.md`, "Share transactions on the
//! network".
//!
//! Priority: a transaction's fee is the share units it burns, its inputs
//! less its outputs, and its priority is that fee per byte, then its arrival
//! (`Priority`). Shares carry the highest priorities first, and a full pool
//! admits a transaction only by evicting lower ones, lowest first. This is
//! node policy, not a share-chain rule: a share may still carry any valid
//! transaction, with or without a fee.
//!
//! Locking: the pool's state sits behind one mutex, held only for in-memory
//! work. It is never held across an await, a store read or another lock.

use crate::accounting::payout::sharechain_pplns::payout_rights::has_share_transactions;
use crate::accounting::payout::sharechain_pplns::pplns_window::{
    BCH_COINBASE_MATURITY, PAYOUT_DEPTH,
};
use crate::shares::bch_script::{self, BchScriptError};
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
use crate::shares::share_block::ShareTransaction;
use crate::shares::validation::{BCH_MAX_SHARE_SIGCHECKS, TXS_COUNT_LIMIT};
use crate::store::transaction_store::{PrevoutCheck, PrevoutRejection};
use crate::store::writer::StoreError;
use bitcoin::{Amount, BlockHash, OutPoint, Transaction, TxOut, Txid};
use libp2p::PeerId;
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tracing::debug;

/// Most transactions the pool holds.
pub const MAX_POOL_TRANSACTIONS: usize = 1_000;
/// Most bytes of transactions the pool holds: 4 MiB.
pub const MAX_POOL_BYTES: usize = 4 * 1024 * 1024;
/// Largest transaction the pool takes: 50 KiB.
pub const MAX_POOL_TRANSACTION_SIZE: usize = 50 * 1024;
/// Seconds a transaction stays in the pool after it arrived: 3 hours.
pub const POOL_TRANSACTION_EXPIRY_SECS: u64 = 3 * 60 * 60;
/// Confirmed transactions the pool remembers, so a reorg that takes one off
/// the confirmed chain can bring it back.
pub const MAX_RECENTLY_CONFIRMED: usize = 200;
/// Most bytes of pool transactions one share carries: 64 KiB.
pub const MAX_SHARE_TRANSACTION_BYTES: usize = 64 * 1024;
/// Most pool transactions one share carries: all but the coinbase of
/// `TXS_COUNT_LIMIT`.
pub const MAX_SHARE_TRANSACTIONS: usize = TXS_COUNT_LIMIT as usize - 1;
/// Average transactions a second the pool admits from peers, all together.
pub const PEER_TRANSACTIONS_PER_SECOND: u64 = 10;
/// Most transactions the pool admits from peers in one burst.
pub const PEER_TRANSACTION_BURST: u64 = 100;
/// Accepted transactions waiting for the node actor to relay them.
pub const RELAY_CHANNEL_CAPACITY: usize = 256;
/// Most shares between the confirmed tip and a mining base whose spends
/// selection checks. A longer branch gets no pool transactions.
const MAX_BRANCH_WALK: usize = 100;
/// Milli-tokens in one token of the peer rate limiter.
const MILLIS_PER_TOKEN: u64 = 1_000;

/// Where a transaction came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionSource {
    /// This node's own API or CLI. Not rate limited.
    Local,
    /// A peer relayed it. Rate limited, and not relayed back to it.
    Peer(PeerId),
}

/// A transaction the pool accepted, for the node actor to send to peers.
#[derive(Debug, Clone)]
pub struct RelayTransaction {
    pub transaction: Transaction,
    /// The peer it came from, which does not get it back.
    pub source: Option<PeerId>,
}

/// Receiver of the transactions the pool accepted.
pub type RelayReceiver = mpsc::Receiver<RelayTransaction>;

/// What `ShareTransactionPool::submit` did with a transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitOutcome {
    /// Admitted, and queued for relay.
    Accepted(Txid),
    /// Already in the pool; nothing changed and nothing is relayed.
    AlreadyInPool(Txid),
}

impl SubmitOutcome {
    pub fn txid(&self) -> Txid {
        match self {
            SubmitOutcome::Accepted(txid) | SubmitOutcome::AlreadyInPool(txid) => *txid,
        }
    }
}

/// Why the pool refused a transaction.
#[derive(Debug, thiserror::Error)]
pub enum PoolRejection {
    #[error("a coinbase cannot be submitted")]
    Coinbase,
    #[error("transaction of {size} bytes exceeds the pool limit of {limit}")]
    TooLarge { size: usize, limit: usize },
    #[error("transaction has no inputs")]
    NoInputs,
    #[error("transaction has no outputs")]
    NoOutputs,
    #[error("transaction spends {0} twice")]
    DuplicateInput(OutPoint),
    #[error("transaction output values exceed the money supply")]
    ValueOutOfRange,
    #[error("input {outpoint} is already spent by pool transaction {txid}")]
    Conflict { outpoint: OutPoint, txid: Txid },
    #[error("transaction is already on the confirmed share chain")]
    AlreadyConfirmed,
    #[error("an input spends an output the share chain does not have: {0}")]
    UnknownInput(String),
    #[error("an input spends an output of a transaction not on the confirmed share chain")]
    UnconfirmedInput,
    #[error("an input is already spent on the confirmed share chain")]
    SpentInput,
    #[error("{0}")]
    Prevout(PrevoutRejection),
    #[error("script check failed: {0}")]
    Script(BchScriptError),
    #[error("outputs {outputs} exceed inputs {inputs}")]
    OutputsExceedInputs { inputs: Amount, outputs: Amount },
    #[error("transaction is not final at share height {height}: {error}")]
    NotFinal { height: u32, error: BchScriptError },
    #[error("transaction executes {sig_checks} SigChecks, over the {limit} a transaction may")]
    TooManySigChecks { sig_checks: usize, limit: usize },
    #[error("too many transactions from peers; try again later")]
    RateLimited,
    #[error(
        "the pool is full and this transaction does not outbid the ones it would evict, \
         which burn {fee} share units in {size} bytes; burn more units per byte"
    )]
    PoolFull { fee: u64, size: usize },
    #[error("the share chain has no confirmed tip yet")]
    NoChainTip,
    #[error("store read failed: {0}")]
    Store(String),
}

impl From<StoreError> for PoolRejection {
    fn from(error: StoreError) -> Self {
        PoolRejection::Store(error.to_string())
    }
}

/// The pool's bounds. Production uses `PoolLimits::default()`; tests use
/// small ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolLimits {
    pub max_transactions: usize,
    pub max_bytes: usize,
    pub expiry_secs: u64,
    pub max_recently_confirmed: usize,
}

impl Default for PoolLimits {
    fn default() -> Self {
        Self {
            max_transactions: MAX_POOL_TRANSACTIONS,
            max_bytes: MAX_POOL_BYTES,
            expiry_secs: POOL_TRANSACTION_EXPIRY_SECS,
            max_recently_confirmed: MAX_RECENTLY_CONFIRMED,
        }
    }
}

/// One pool transaction.
#[derive(Debug, Clone)]
struct PoolEntry {
    transaction: Arc<ShareTransaction>,
    txid: Txid,
    size: usize,
    sig_checks: usize,
    /// Share units the transaction burns: its inputs less its outputs. The
    /// share coinbase pays a fixed unit, so nobody receives them.
    fee: u64,
    arrived_at: u64,
}

impl PoolEntry {
    fn spends(&self) -> impl Iterator<Item = OutPoint> + '_ {
        self.transaction
            .input
            .iter()
            .map(|input| input.previous_output)
    }

    fn priority(&self, sequence: u64) -> Priority {
        Priority {
            fee: self.fee,
            size: self.size,
            sequence,
        }
    }
}

/// Where a pool transaction stands against the others: by fee rate, the
/// share units it burns per byte, then by arrival. Greater is mined first
/// and evicted last.
///
/// Rates are compared exactly, as fractions (`fee * other size` against
/// `other fee * size`), so no rounding decides between two transactions.
/// At equal rates the earlier arrival (smaller sequence) is greater: the
/// first seen keeps its place, and a newcomer must pay a higher rate to
/// evict anything. Sequences are unique, so no two priorities are equal.
#[derive(Debug, Clone, Copy)]
struct Priority {
    fee: u64,
    size: usize,
    sequence: u64,
}

impl Ord for Priority {
    fn cmp(&self, other: &Self) -> Ordering {
        let own_rate = u128::from(self.fee) * other.size as u128;
        let other_rate = u128::from(other.fee) * self.size as u128;
        own_rate
            .cmp(&other_rate)
            .then_with(|| other.sequence.cmp(&self.sequence))
    }
}

impl PartialOrd for Priority {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Priority {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Priority {}

/// Token bucket for transactions from peers, in milli-tokens.
#[derive(Debug, Clone, Copy)]
struct TokenBucket {
    milli_tokens: u64,
    refilled_at_millis: u64,
}

impl TokenBucket {
    fn full(now_millis: u64) -> Self {
        Self {
            milli_tokens: PEER_TRANSACTION_BURST * MILLIS_PER_TOKEN,
            refilled_at_millis: now_millis,
        }
    }

    /// Take one token if there is one, after refilling for the time passed.
    fn try_take(&mut self, now_millis: u64) -> bool {
        let elapsed = now_millis.saturating_sub(self.refilled_at_millis);
        self.milli_tokens = self
            .milli_tokens
            .saturating_add(elapsed.saturating_mul(PEER_TRANSACTIONS_PER_SECOND))
            .min(PEER_TRANSACTION_BURST * MILLIS_PER_TOKEN);
        self.refilled_at_millis = self.refilled_at_millis.max(now_millis);
        if self.milli_tokens < MILLIS_PER_TOKEN {
            return false;
        }
        self.milli_tokens -= MILLIS_PER_TOKEN;
        true
    }
}

/// The pool's in-memory state. Every method is bounded by the pool's limits
/// and reads no store.
#[derive(Debug)]
struct PoolState {
    limits: PoolLimits,
    entries: HashMap<Txid, PoolEntry>,
    /// Arrival order: sequence number to txid, oldest first.
    arrival: BTreeMap<u64, Txid>,
    /// Priority order: lowest first, so eviction takes from the front and
    /// selection from the back.
    by_priority: BTreeMap<Priority, Txid>,
    /// Sequence number of each entry, to remove it from `arrival` and
    /// `by_priority`.
    sequence_of: HashMap<Txid, u64>,
    /// Which pool transaction spends each outpoint.
    spent_by: HashMap<OutPoint, Txid>,
    total_bytes: usize,
    next_sequence: u64,
    /// Confirmed transactions, newest first.
    recently_confirmed: VecDeque<PoolEntry>,
    peer_tokens: TokenBucket,
}

impl PoolState {
    fn new(limits: PoolLimits, now_millis: u64) -> Self {
        Self {
            limits,
            entries: HashMap::with_capacity(limits.max_transactions),
            arrival: BTreeMap::new(),
            by_priority: BTreeMap::new(),
            sequence_of: HashMap::with_capacity(limits.max_transactions),
            spent_by: HashMap::with_capacity(limits.max_transactions),
            total_bytes: 0,
            next_sequence: 0,
            recently_confirmed: VecDeque::with_capacity(limits.max_recently_confirmed),
            peer_tokens: TokenBucket::full(now_millis),
        }
    }

    fn contains(&self, txid: &Txid) -> bool {
        self.entries.contains_key(txid)
    }

    /// The first input of `transaction` another pool transaction spends.
    fn conflict(&self, transaction: &Transaction) -> Option<(OutPoint, Txid)> {
        transaction.input.iter().find_map(|input| {
            self.spent_by
                .get(&input.previous_output)
                .map(|txid| (input.previous_output, *txid))
        })
    }

    /// Add `entry`, first dropping expired entries and then, if it still
    /// does not fit, the lowest-priority entries below it. Returns the txids
    /// it dropped.
    ///
    /// Refuses `entry`, changing nothing but the expiry, when the entries it
    /// would have to evict are not all below it (`PoolRejection::PoolFull`).
    fn insert(&mut self, entry: PoolEntry, now: u64) -> Result<Vec<Txid>, PoolRejection> {
        if let Some((outpoint, txid)) = self.conflict(&entry.transaction) {
            return Err(PoolRejection::Conflict { outpoint, txid });
        }
        let mut evicted = self.expire(now);
        let sequence = self.next_sequence;
        let to_evict = self.eviction_for(entry.size, entry.priority(sequence))?;
        for txid in &to_evict {
            self.remove(txid);
        }
        evicted.extend(to_evict);
        for outpoint in entry.spends() {
            self.spent_by.insert(outpoint, entry.txid);
        }
        self.next_sequence += 1;
        self.arrival.insert(sequence, entry.txid);
        self.by_priority
            .insert(entry.priority(sequence), entry.txid);
        self.sequence_of.insert(entry.txid, sequence);
        self.total_bytes += entry.size;
        self.entries.insert(entry.txid, entry);
        Ok(evicted)
    }

    /// The lowest-priority entries to evict, lowest first, so an entry of
    /// `size` bytes fits the count and byte limits, every one of them below
    /// `priority`. `PoolFull` names the first entry in the way.
    fn eviction_for(&self, size: usize, priority: Priority) -> Result<Vec<Txid>, PoolRejection> {
        let mut count = self.entries.len();
        let mut bytes = self.total_bytes;
        let mut to_evict = Vec::new();
        let mut lowest_first = self.by_priority.iter();
        while count + 1 > self.limits.max_transactions || bytes + size > self.limits.max_bytes {
            match lowest_first.next() {
                Some((lower, txid)) if *lower < priority => {
                    count -= 1;
                    bytes -= lower.size;
                    to_evict.push(*txid);
                }
                Some((in_the_way, _)) => {
                    return Err(PoolRejection::PoolFull {
                        fee: in_the_way.fee,
                        size: in_the_way.size,
                    });
                }
                None => {
                    return Err(PoolRejection::TooLarge {
                        size,
                        limit: self.limits.max_bytes,
                    });
                }
            }
        }
        Ok(to_evict)
    }

    fn remove(&mut self, txid: &Txid) -> Option<PoolEntry> {
        let entry = self.entries.remove(txid)?;
        if let Some(sequence) = self.sequence_of.remove(txid) {
            self.arrival.remove(&sequence);
            self.by_priority.remove(&entry.priority(sequence));
        }
        for outpoint in entry.spends() {
            if self.spent_by.get(&outpoint) == Some(txid) {
                self.spent_by.remove(&outpoint);
            }
        }
        self.total_bytes -= entry.size;
        Some(entry)
    }

    /// Drop every entry that arrived `expiry_secs` or more before `now`.
    fn expire(&mut self, now: u64) -> Vec<Txid> {
        let expired: Vec<Txid> = self
            .arrival
            .values()
            .filter(|txid| {
                self.entries.get(*txid).is_some_and(|entry| {
                    entry.arrived_at.saturating_add(self.limits.expiry_secs) <= now
                })
            })
            .copied()
            .collect();
        expired
            .iter()
            .filter_map(|txid| self.remove(txid).map(|entry| entry.txid))
            .collect()
    }

    /// The entries, highest priority first: the order shares carry them.
    fn entries_by_priority(&self) -> Vec<PoolEntry> {
        self.by_priority
            .values()
            .rev()
            .filter_map(|txid| self.entries.get(txid).cloned())
            .collect()
    }

    /// Move a confirmed entry to the recently confirmed list.
    fn mark_confirmed(&mut self, txid: &Txid) {
        if let Some(entry) = self.remove(txid) {
            self.recently_confirmed.push_front(entry);
            self.recently_confirmed
                .truncate(self.limits.max_recently_confirmed);
        }
    }

    /// Take a recently confirmed entry off the list, for a reorg check.
    fn take_recently_confirmed(&mut self, txid: &Txid) -> Option<PoolEntry> {
        let position = self
            .recently_confirmed
            .iter()
            .position(|entry| entry.txid == *txid)?;
        self.recently_confirmed.remove(position)
    }
}

/// Where a pool transaction stands on a mining base.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Readiness {
    /// Valid in a share on the mining base.
    Ready,
    /// Not yet: an input is immature, its source is off the confirmed chain
    /// for now, or its lock time is ahead.
    Waiting,
    /// On the confirmed chain.
    Confirmed,
    /// Never valid again: an input is gone, expired or spent by another
    /// transaction.
    Invalid,
}

/// The shared, bounded pool of share transactions. Cloning shares the pool.
#[derive(Clone)]
pub struct ShareTransactionPool {
    state: Arc<Mutex<PoolState>>,
    chain_store_handle: ChainStoreHandle,
    relay_tx: mpsc::Sender<RelayTransaction>,
}

impl ShareTransactionPool {
    /// An empty pool reading `chain_store_handle`, and the receiver of the
    /// transactions it accepts, for the node actor to relay.
    pub fn new(chain_store_handle: ChainStoreHandle) -> (Self, RelayReceiver) {
        Self::with_limits(chain_store_handle, PoolLimits::default())
    }

    /// `new` with other bounds, for tests.
    pub fn with_limits(
        chain_store_handle: ChainStoreHandle,
        limits: PoolLimits,
    ) -> (Self, RelayReceiver) {
        let (relay_tx, relay_rx) = mpsc::channel(RELAY_CHANNEL_CAPACITY);
        let pool = Self {
            state: Arc::new(Mutex::new(PoolState::new(limits, now_millis()))),
            chain_store_handle,
            relay_tx,
        };
        (pool, relay_rx)
    }

    /// Check `transaction` against every share-chain rule on top of the
    /// confirmed chain and admit it, then queue it for relay.
    ///
    /// A full pool admits it only by evicting transactions of lower priority
    /// (`Priority`: share units burned per byte, then arrival), and refuses
    /// it with `PoolRejection::PoolFull` otherwise.
    ///
    /// Transactions from peers are rate limited, except copies of one the
    /// pool already has, which several peers relay. A transaction already in
    /// the pool is reported as such and not relayed again, which is what ends
    /// a relay.
    pub fn submit(
        &self,
        transaction: Transaction,
        source: TransactionSource,
    ) -> Result<SubmitOutcome, PoolRejection> {
        let txid = transaction.compute_txid();
        {
            let mut state = self.lock_state();
            if state.contains(&txid) {
                return Ok(SubmitOutcome::AlreadyInPool(txid));
            }
            if let TransactionSource::Peer(_) = source
                && !state.peer_tokens.try_take(now_millis())
            {
                return Err(PoolRejection::RateLimited);
            }
            if let Some((outpoint, conflicting)) = state.conflict(&transaction) {
                return Err(PoolRejection::Conflict {
                    outpoint,
                    txid: conflicting,
                });
            }
        }
        let size = check_transaction_shape(&transaction)?;

        if self.chain_store_handle.are_all_txids_confirmed(&[txid])? {
            return Err(PoolRejection::AlreadyConfirmed);
        }
        let next_height = self
            .chain_store_handle
            .get_tip_height()?
            .ok_or(PoolRejection::NoChainTip)?
            + 1;
        let checked =
            check_against_confirmed_chain(&self.chain_store_handle, &transaction, next_height)?;

        let entry = PoolEntry {
            transaction: Arc::new(ShareTransaction(transaction.clone())),
            txid,
            size,
            sig_checks: checked.sig_checks,
            fee: checked.fee,
            arrived_at: now_seconds(),
        };
        {
            let mut state = self.lock_state();
            if state.contains(&txid) {
                return Ok(SubmitOutcome::AlreadyInPool(txid));
            }
            let evicted = state.insert(entry, now_seconds())?;
            if !evicted.is_empty() {
                debug!(
                    "Share transaction pool evicted {} transactions",
                    evicted.len()
                );
            }
        }

        let relay = RelayTransaction {
            transaction,
            source: match source {
                TransactionSource::Local => None,
                TransactionSource::Peer(peer) => Some(peer),
            },
        };
        if let Err(error) = self.relay_tx.try_send(relay) {
            debug!("Share transaction {txid} not queued for relay: {error}");
        }
        Ok(SubmitOutcome::Accepted(txid))
    }

    /// The pool transactions to put in a share built on `mining_base`,
    /// highest priority first (share units burned per byte, then arrival),
    /// within the share budgets.
    ///
    /// Rechecks the pool on the way: a transaction now on the confirmed chain
    /// moves to the recently confirmed list, one a reorg took off the
    /// confirmed chain comes back, and one that can never be valid again is
    /// dropped. A transaction is chosen when it is valid in a share on
    /// `mining_base`: its inputs unspent on the confirmed chain and by every
    /// share from the confirmed tip up to `mining_base`, and it is final and
    /// mature at the share's height. A store failure gives an empty set
    /// rather than stop the notifier: the share is then simply built without
    /// pool transactions.
    pub fn select_for_share(&self, mining_base: BlockHash) -> Vec<ShareTransaction> {
        match self.try_select_for_share(mining_base) {
            Ok(selected) => selected,
            Err(error) => {
                debug!("No pool transactions for a share on {mining_base}: {error}");
                Vec::new()
            }
        }
    }

    /// Number of transactions in the pool.
    pub fn len(&self) -> usize {
        self.lock_state().entries.len()
    }

    /// Whether the pool is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether `txid` is in the pool.
    pub fn contains(&self, txid: &Txid) -> bool {
        self.lock_state().contains(txid)
    }

    fn try_select_for_share(
        &self,
        mining_base: BlockHash,
    ) -> Result<Vec<ShareTransaction>, PoolRejection> {
        let next_height = self
            .chain_store_handle
            .get_block_metadata(&mining_base)?
            .expected_height
            .ok_or(PoolRejection::NoChainTip)?
            + 1;
        let Some(branch_spends) = self.branch_spends(mining_base)? else {
            return Ok(Vec::new());
        };

        let (pending, recently_confirmed) = {
            let mut state = self.lock_state();
            state.expire(now_seconds());
            (
                state.entries_by_priority(),
                state.recently_confirmed.iter().cloned().collect::<Vec<_>>(),
            )
        };
        let mut pending_readiness = Vec::with_capacity(pending.len());
        for entry in &pending {
            pending_readiness.push(self.readiness(entry, next_height)?);
        }
        let mut confirmed_readiness = Vec::with_capacity(recently_confirmed.len());
        for entry in &recently_confirmed {
            confirmed_readiness.push(self.readiness(entry, next_height)?);
        }

        let mut ready_txids: HashSet<Txid> = pending
            .iter()
            .zip(&pending_readiness)
            .filter(|(_, readiness)| **readiness == Readiness::Ready)
            .map(|(entry, _)| entry.txid)
            .collect();
        let mut state = self.lock_state();
        for (entry, readiness) in pending.iter().zip(&pending_readiness) {
            match readiness {
                Readiness::Confirmed => state.mark_confirmed(&entry.txid),
                Readiness::Invalid => {
                    state.remove(&entry.txid);
                }
                Readiness::Ready | Readiness::Waiting => {}
            }
        }
        let mut returned = 0;
        for (entry, readiness) in recently_confirmed.iter().zip(&confirmed_readiness) {
            match readiness {
                Readiness::Confirmed => {}
                Readiness::Invalid => {
                    state.take_recently_confirmed(&entry.txid);
                }
                Readiness::Ready | Readiness::Waiting => {
                    if let Some(mut entry) = state.take_recently_confirmed(&entry.txid) {
                        entry.arrived_at = now_seconds();
                        let txid = entry.txid;
                        if state.insert(entry, now_seconds()).is_ok() {
                            returned += 1;
                            if *readiness == Readiness::Ready {
                                ready_txids.insert(txid);
                            }
                        }
                    }
                }
            }
        }
        // Highest priority first, over the transactions still in the pool:
        // a reorged one returns in its own place, and one a concurrent
        // submit evicted is not chosen.
        let ready: Vec<PoolEntry> = state
            .entries_by_priority()
            .into_iter()
            .filter(|entry| ready_txids.contains(&entry.txid))
            .collect();
        drop(state);
        if returned > 0 {
            debug!(
                "{returned} share transactions left the confirmed chain and are back in the pool"
            );
        }

        Ok(select_within_budget(ready.iter(), &branch_spends))
    }

    /// The outpoints spent by the shares from just above the confirmed tip
    /// up to `mining_base`, or `None` when that branch is longer than
    /// `MAX_BRANCH_WALK`.
    fn branch_spends(
        &self,
        mining_base: BlockHash,
    ) -> Result<Option<HashSet<OutPoint>>, PoolRejection> {
        let mut spends = HashSet::new();
        let mut current = mining_base;
        let mut steps = 0;
        while !self.chain_store_handle.is_block_confirmed(&current) {
            if steps == MAX_BRANCH_WALK {
                return Ok(None);
            }
            steps += 1;
            let header = self.chain_store_handle.get_share_header(&current)?;
            if has_share_transactions(&header) {
                for transaction in self.chain_store_handle.get_share_transactions(&current)? {
                    if !transaction.is_coinbase() {
                        spends.extend(transaction.input.iter().map(|input| input.previous_output));
                    }
                }
            }
            current = header.prev_share_blockhash;
        }
        Ok(Some(spends))
    }

    /// Where `entry` stands for a share at `height` on the confirmed chain.
    fn readiness(&self, entry: &PoolEntry, height: u32) -> Result<Readiness, PoolRejection> {
        let store = &self.chain_store_handle;
        if store.are_all_txids_confirmed(&[entry.txid])? {
            return Ok(Readiness::Confirmed);
        }
        let outpoints: Vec<OutPoint> = entry.spends().collect();
        match store.check_prevouts(
            &outpoints,
            height,
            height.saturating_sub(PAYOUT_DEPTH as u32),
            BCH_COINBASE_MATURITY,
        )? {
            PrevoutCheck::Accepted => {}
            PrevoutCheck::Rejected(PrevoutRejection::ImmatureCoinbase { .. }) => {
                return Ok(Readiness::Waiting);
            }
            PrevoutCheck::Rejected(_) => return Ok(Readiness::Invalid),
        }
        if store.is_any_prevout_spent(&outpoints)? {
            return Ok(Readiness::Invalid);
        }
        if !store.are_all_txids_confirmed(&source_txids(&entry.transaction))? {
            return Ok(Readiness::Waiting);
        }
        if bch_script::verify_final(&entry.transaction, height).is_err() {
            return Ok(Readiness::Waiting);
        }
        Ok(Readiness::Ready)
    }

    fn lock_state(&self) -> MutexGuard<'_, PoolState> {
        // A panic while the lock was held leaves bounded, self-consistent
        // maps behind (each method finishes its own updates before it can
        // fail), so recover the state rather than take the node down.
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Choose `ready` entries, in order, that spend nothing in `branch_spends`
/// and fit the share budgets.
fn select_within_budget<'entry>(
    ready: impl Iterator<Item = &'entry PoolEntry>,
    branch_spends: &HashSet<OutPoint>,
) -> Vec<ShareTransaction> {
    let mut selected = Vec::new();
    let mut bytes = 0;
    let mut sig_checks = 0;
    for entry in ready {
        let fits = selected.len() < MAX_SHARE_TRANSACTIONS
            && bytes + entry.size <= MAX_SHARE_TRANSACTION_BYTES
            && sig_checks + entry.sig_checks <= BCH_MAX_SHARE_SIGCHECKS;
        let conflicts = entry
            .spends()
            .any(|outpoint| branch_spends.contains(&outpoint));
        if fits && !conflicts {
            bytes += entry.size;
            sig_checks += entry.sig_checks;
            selected.push(entry.transaction.as_ref().clone());
        }
    }
    selected
}

/// The checks a share block makes of a transaction without its context:
/// not a coinbase, inputs and outputs present, no input spent twice, values
/// within the money supply, and the pool's size limit. Returns its size.
fn check_transaction_shape(transaction: &Transaction) -> Result<usize, PoolRejection> {
    if transaction.is_coinbase() {
        return Err(PoolRejection::Coinbase);
    }
    let size = transaction.total_size();
    if size > MAX_POOL_TRANSACTION_SIZE {
        return Err(PoolRejection::TooLarge {
            size,
            limit: MAX_POOL_TRANSACTION_SIZE,
        });
    }
    if transaction.input.is_empty() {
        return Err(PoolRejection::NoInputs);
    }
    if transaction.output.is_empty() {
        return Err(PoolRejection::NoOutputs);
    }
    let mut seen = HashSet::with_capacity(transaction.input.len());
    if let Some(repeated) = transaction
        .input
        .iter()
        .map(|input| input.previous_output)
        .find(|outpoint| !seen.insert(*outpoint))
    {
        return Err(PoolRejection::DuplicateInput(repeated));
    }
    output_total(&transaction.output).ok_or(PoolRejection::ValueOutOfRange)?;
    Ok(size)
}

/// Total value of `outputs`, or `None` past the money supply.
fn output_total(outputs: &[TxOut]) -> Option<Amount> {
    outputs
        .iter()
        .try_fold(Amount::ZERO, |total, output| {
            (output.value <= Amount::MAX_MONEY)
                .then_some(())
                .and_then(|()| total.checked_add(output.value))
        })
        .filter(|total| *total <= Amount::MAX_MONEY)
}

/// The distinct txids `transaction` spends outputs of.
fn source_txids(transaction: &Transaction) -> Vec<Txid> {
    let mut seen = HashSet::with_capacity(transaction.input.len());
    transaction
        .input
        .iter()
        .map(|input| input.previous_output.txid)
        .filter(|txid| seen.insert(*txid))
        .collect()
}

/// The checks a share at `height` on the confirmed tip makes of
/// `transaction`: inputs exist, come from confirmed transactions and are
/// unspent on the confirmed chain; coinbase inputs are mature and no input's
/// coinbase root has expired; the Bitcoin Cash interpreter accepts every
/// input; outputs are P2PKH or P2SH32 above dust; inputs cover outputs; it is
/// final at `height`; it executes at most `MAX_TX_SIGCHECKS`. Returns its
/// SigChecks and the share units it burns.
fn check_against_confirmed_chain(
    chain_store_handle: &ChainStoreHandle,
    transaction: &Transaction,
    height: u32,
) -> Result<ChainCheck, PoolRejection> {
    let spent_outputs = chain_store_handle
        .get_all_prevouts(transaction)
        .map_err(|error| match error {
            StoreError::NotFound(message) => PoolRejection::UnknownInput(message),
            other => PoolRejection::from(other),
        })?;
    if !chain_store_handle.are_all_txids_confirmed(&source_txids(transaction))? {
        return Err(PoolRejection::UnconfirmedInput);
    }
    let outpoints: Vec<OutPoint> = transaction
        .input
        .iter()
        .map(|input| input.previous_output)
        .collect();
    if chain_store_handle.is_any_prevout_spent(&outpoints)? {
        return Err(PoolRejection::SpentInput);
    }
    if let PrevoutCheck::Rejected(rejection) = chain_store_handle.check_prevouts(
        &outpoints,
        height,
        height.saturating_sub(PAYOUT_DEPTH as u32),
        BCH_COINBASE_MATURITY,
    )? {
        return Err(PoolRejection::Prevout(rejection));
    }
    bch_script::verify_transaction(transaction, &spent_outputs).map_err(PoolRejection::Script)?;
    bch_script::verify_outputs(transaction).map_err(PoolRejection::Script)?;

    let inputs = spent_outputs
        .iter()
        .try_fold(Amount::ZERO, |total, (_, output)| {
            total.checked_add(output.value)
        })
        .ok_or(PoolRejection::ValueOutOfRange)?;
    let outputs = output_total(&transaction.output).ok_or(PoolRejection::ValueOutOfRange)?;
    if outputs > inputs {
        return Err(PoolRejection::OutputsExceedInputs { inputs, outputs });
    }
    bch_script::verify_final(transaction, height)
        .map_err(|error| PoolRejection::NotFinal { height, error })?;

    let sig_checks = check_sig_checks(transaction)?;
    Ok(ChainCheck {
        sig_checks,
        fee: (inputs - outputs).to_sat(),
    })
}

/// What `check_against_confirmed_chain` learns of an admissible transaction.
struct ChainCheck {
    /// SigChecks the transaction executes, as share validation counts them.
    sig_checks: usize,
    /// Share units the transaction burns: its inputs less its outputs.
    fee: u64,
}

/// The SigChecks of an accepted `transaction`, refused past BCH's
/// `MAX_TX_SIGCHECKS`, as share validation counts them.
fn check_sig_checks(transaction: &Transaction) -> Result<usize, PoolRejection> {
    let sig_checks = bch_script::sig_checks(transaction);
    if sig_checks > bch_script::MAX_TX_SIGCHECKS {
        return Err(PoolRejection::TooManySigChecks {
            sig_checks,
            limit: bch_script::MAX_TX_SIGCHECKS,
        });
    }
    Ok(sig_checks)
}

fn now_seconds() -> u64 {
    now_millis() / 1_000
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::address::share_owner::ShareOwner;
    use crate::shares::bch_htlc::{HtlcContract, secret_hash};
    #[cfg(not(feature = "sim"))]
    use crate::shares::bch_trade::build_refund_transaction;
    use crate::shares::bch_trade::{Coin, TradeKey, build_lock_transaction};
    use crate::shares::share_block::ShareBlock;
    use crate::test_utils::{
        TRADE_BUYER_SECRET_KEY, TRADE_SECRET, TRADE_SELLER_SECRET_KEY,
        build_confirmed_chain_owned_from, setup_test_chain_store_handle, test_coinbase_transaction,
    };
    #[cfg(not(feature = "sim"))]
    use crate::test_utils::{TestShareBlockBuilder, confirm_test_block};
    use bitcoin::hashes::Hash;
    use bitcoin::{
        NetworkKind, PrivateKey, ScriptBuf, Sequence, TxIn, Witness, absolute, transaction,
    };

    /// Units in one share coinbase output.
    const SHARE_UNITS: u64 = 100_000_000;

    fn seller() -> TradeKey {
        TradeKey::new(PrivateKey::from_slice(&TRADE_SELLER_SECRET_KEY, NetworkKind::Test).unwrap())
    }

    fn buyer() -> TradeKey {
        TradeKey::new(PrivateKey::from_slice(&TRADE_BUYER_SECRET_KEY, NetworkKind::Test).unwrap())
    }

    fn contract(refund_height: u32) -> HtlcContract {
        HtlcContract::new(
            secret_hash(&TRADE_SECRET),
            buyer().pubkey_hash(),
            seller().pubkey_hash(),
            refund_height,
        )
        .unwrap()
    }

    /// The seller's lock of `amount` units of the share coinbase in
    /// `share`, the rest back to the seller.
    fn lock_of(share: &ShareBlock, amount: u64) -> Transaction {
        build_lock_transaction(
            &[Coin {
                outpoint: OutPoint::new(share.transactions[0].compute_txid(), 0),
                value: Amount::from_sat(SHARE_UNITS),
            }],
            &seller(),
            &contract(600),
            Amount::from_sat(amount),
            &seller().p2pkh_script(),
            0,
        )
        .unwrap()
    }

    /// A pool entry spending `inputs`, of `size` bytes, burning no share
    /// units, arrived at `now`.
    fn entry(inputs: &[OutPoint], size: usize, arrived_at: u64) -> PoolEntry {
        let transaction = Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: inputs
                .iter()
                .map(|outpoint| TxIn {
                    previous_output: *outpoint,
                    script_sig: ScriptBuf::new(),
                    sequence: Sequence::MAX,
                    witness: Witness::new(),
                })
                .collect(),
            output: vec![TxOut {
                value: Amount::from_sat(1_000),
                script_pubkey: ScriptBuf::new(),
            }],
        };
        PoolEntry {
            txid: transaction.compute_txid(),
            transaction: Arc::new(ShareTransaction(transaction)),
            size,
            sig_checks: inputs.len(),
            fee: 0,
            arrived_at,
        }
    }

    /// `entry`, arrived at 0, burning `fee` share units.
    fn paying_entry(inputs: &[OutPoint], size: usize, fee: u64) -> PoolEntry {
        PoolEntry {
            fee,
            ..entry(inputs, size, 0)
        }
    }

    fn outpoint(byte: u8) -> OutPoint {
        OutPoint::new(Txid::from_byte_array([byte; 32]), 0)
    }

    fn limits(max_transactions: usize, max_bytes: usize) -> PoolLimits {
        PoolLimits {
            max_transactions,
            max_bytes,
            expiry_secs: 100,
            max_recently_confirmed: 2,
        }
    }

    #[test]
    fn insert_refuses_an_input_another_pool_transaction_spends() {
        let mut state = PoolState::new(limits(10, 10_000), 0);
        let first = entry(&[outpoint(1)], 100, 0);
        let first_txid = first.txid;
        state.insert(first, 0).unwrap();

        let conflict = state.insert(entry(&[outpoint(2), outpoint(1)], 100, 0), 0);

        assert!(matches!(
            conflict,
            Err(PoolRejection::Conflict { outpoint: spent, txid }) if spent == outpoint(1) && txid == first_txid
        ));
        assert_eq!(state.entries.len(), 1);
    }

    /// At the count limit a higher fee rate evicts the lowest one, not the
    /// oldest transaction.
    #[test]
    fn insert_evicts_the_lowest_fee_rate_at_the_count_limit() {
        let mut state = PoolState::new(limits(2, 10_000), 0);
        let oldest = paying_entry(&[outpoint(1)], 100, 200);
        let oldest_txid = oldest.txid;
        let cheapest = paying_entry(&[outpoint(2)], 100, 100);
        let cheapest_txid = cheapest.txid;
        state.insert(oldest, 0).unwrap();
        state.insert(cheapest, 0).unwrap();

        let evicted = state
            .insert(paying_entry(&[outpoint(3)], 100, 300), 0)
            .unwrap();

        assert_eq!(evicted, vec![cheapest_txid]);
        assert_eq!(state.entries.len(), 2);
        assert!(state.contains(&oldest_txid));
        assert!(!state.spent_by.contains_key(&outpoint(2)));
        assert_eq!(state.by_priority.len(), 2);
    }

    /// A full pool refuses a transaction whose fee rate only matches the
    /// lowest one, and keeps what it has: the first seen keeps its place.
    #[test]
    fn a_full_pool_refuses_an_equal_fee_rate() {
        let mut state = PoolState::new(limits(2, 10_000), 0);
        state
            .insert(paying_entry(&[outpoint(1)], 100, 100), 0)
            .unwrap();
        state
            .insert(paying_entry(&[outpoint(2)], 100, 100), 0)
            .unwrap();

        let refused = state.insert(paying_entry(&[outpoint(3)], 200, 200), 0);

        assert!(matches!(
            refused,
            Err(PoolRejection::PoolFull {
                fee: 100,
                size: 100
            })
        ));
        assert_eq!(state.entries.len(), 2);
        assert!(!state.spent_by.contains_key(&outpoint(3)));
    }

    /// A full pool of transactions burning nothing still admits one that
    /// burns a unit, and refuses another that burns nothing.
    #[test]
    fn a_pool_full_of_free_transactions_admits_only_a_paying_one() {
        let mut state = PoolState::new(limits(1, 10_000), 0);
        state.insert(entry(&[outpoint(1)], 100, 0), 0).unwrap();

        let free = state.insert(entry(&[outpoint(2)], 100, 0), 0);
        let paying = state.insert(paying_entry(&[outpoint(3)], 100, 1), 0);

        assert!(matches!(free, Err(PoolRejection::PoolFull { .. })));
        assert_eq!(paying.unwrap().len(), 1);
        assert!(state.spent_by.contains_key(&outpoint(3)));
    }

    /// At equal fee rates the newest is evicted first.
    #[test]
    fn equal_fee_rates_are_evicted_newest_first() {
        let mut state = PoolState::new(limits(2, 10_000), 0);
        let older = paying_entry(&[outpoint(1)], 100, 100);
        let older_txid = older.txid;
        let newer = paying_entry(&[outpoint(2)], 100, 100);
        let newer_txid = newer.txid;
        state.insert(older, 0).unwrap();
        state.insert(newer, 0).unwrap();

        let evicted = state
            .insert(paying_entry(&[outpoint(3)], 100, 101), 0)
            .unwrap();

        assert_eq!(evicted, vec![newer_txid]);
        assert!(state.contains(&older_txid));
    }

    /// The lowest fee rates make room for the bytes a higher one needs.
    #[test]
    fn insert_evicts_lower_fee_rates_until_the_bytes_fit() {
        let mut state = PoolState::new(limits(10, 1_000), 0);
        state
            .insert(paying_entry(&[outpoint(1)], 400, 400), 0)
            .unwrap();
        state
            .insert(paying_entry(&[outpoint(2)], 400, 800), 0)
            .unwrap();

        let evicted = state
            .insert(paying_entry(&[outpoint(3)], 900, 2_700), 0)
            .unwrap();

        assert_eq!(evicted.len(), 2);
        assert_eq!(state.entries.len(), 1);
        assert_eq!(state.total_bytes, 900);
    }

    /// When the bytes a newcomer needs would take a transaction paying a
    /// higher rate than it, nothing is evicted and it is refused.
    #[test]
    fn insert_evicts_nothing_when_a_higher_fee_rate_is_in_the_way() {
        let mut state = PoolState::new(limits(10, 1_000), 0);
        let cheap = paying_entry(&[outpoint(1)], 400, 400);
        let cheap_txid = cheap.txid;
        state.insert(cheap, 0).unwrap();
        state
            .insert(paying_entry(&[outpoint(2)], 400, 1_200), 0)
            .unwrap();

        let refused = state.insert(paying_entry(&[outpoint(3)], 900, 1_800), 0);

        assert!(matches!(
            refused,
            Err(PoolRejection::PoolFull {
                fee: 1_200,
                size: 400
            })
        ));
        assert!(state.contains(&cheap_txid));
        assert_eq!(state.total_bytes, 800);
    }

    /// Shares carry the highest fee rate first and, at equal rates, the
    /// earliest arrival; rates are compared exactly, not rounded.
    #[test]
    fn entries_are_ordered_by_fee_rate_then_arrival() {
        let mut state = PoolState::new(limits(10, 10_000), 0);
        let one_per_byte = paying_entry(&[outpoint(1)], 300, 300);
        let three_per_byte = paying_entry(&[outpoint(2)], 100, 300);
        let one_per_byte_later = paying_entry(&[outpoint(3)], 600, 600);
        let just_over_one = paying_entry(&[outpoint(4)], 300, 301);
        let expected = vec![
            three_per_byte.txid,
            just_over_one.txid,
            one_per_byte.txid,
            one_per_byte_later.txid,
        ];
        state.insert(one_per_byte, 0).unwrap();
        state.insert(three_per_byte, 0).unwrap();
        state.insert(one_per_byte_later, 0).unwrap();
        state.insert(just_over_one, 0).unwrap();

        let order: Vec<Txid> = state
            .entries_by_priority()
            .iter()
            .map(|entry| entry.txid)
            .collect();

        assert_eq!(order, expected);
    }

    #[test]
    fn expire_drops_transactions_older_than_the_expiry() {
        let mut state = PoolState::new(limits(10, 10_000), 0);
        let old = entry(&[outpoint(1)], 100, 0);
        let old_txid = old.txid;
        state.insert(old, 0).unwrap();
        state.insert(entry(&[outpoint(2)], 100, 50), 50).unwrap();

        let expired = state.expire(100);

        assert_eq!(expired, vec![old_txid]);
        assert_eq!(state.entries.len(), 1);
        assert_eq!(state.total_bytes, 100);
    }

    #[test]
    fn removing_a_transaction_frees_its_inputs() {
        let mut state = PoolState::new(limits(10, 10_000), 0);
        let first = entry(&[outpoint(1)], 100, 0);
        let first_txid = first.txid;
        state.insert(first, 0).unwrap();

        state.remove(&first_txid);

        assert!(
            state
                .insert(entry(&[outpoint(1), outpoint(2)], 100, 0), 0)
                .is_ok()
        );
        assert_eq!(state.arrival.len(), 1);
        assert_eq!(state.sequence_of.len(), 1);
        assert_eq!(state.by_priority.len(), 1);
    }

    #[test]
    fn the_recently_confirmed_list_is_bounded() {
        let mut state = PoolState::new(limits(10, 10_000), 0);
        let first = entry(&[outpoint(1)], 100, 0);
        let second = entry(&[outpoint(2)], 100, 0);
        let third = entry(&[outpoint(3)], 100, 0);
        let (first_txid, second_txid, third_txid) = (first.txid, second.txid, third.txid);
        state.insert(first, 0).unwrap();
        state.insert(second, 0).unwrap();
        state.insert(third, 0).unwrap();

        state.mark_confirmed(&first_txid);
        state.mark_confirmed(&second_txid);
        state.mark_confirmed(&third_txid);

        assert!(state.entries.is_empty());
        assert_eq!(state.recently_confirmed.len(), 2);
        assert!(state.take_recently_confirmed(&first_txid).is_none());
        assert!(state.take_recently_confirmed(&third_txid).is_some());
    }

    #[test]
    fn the_peer_rate_limit_refuses_past_the_burst_and_refills_with_time() {
        let mut bucket = TokenBucket::full(0);
        let admitted_in_burst = (0..PEER_TRANSACTION_BURST + 5)
            .filter(|_| bucket.try_take(0))
            .count() as u64;

        assert_eq!(admitted_in_burst, PEER_TRANSACTION_BURST);
        assert!(!bucket.try_take(0));
        assert!(bucket.try_take(1_000 / PEER_TRANSACTIONS_PER_SECOND));
        assert!(!bucket.try_take(1_000 / PEER_TRANSACTIONS_PER_SECOND));
    }

    #[test]
    fn selection_stops_at_the_share_byte_budget() {
        let small = entry(&[outpoint(1)], 1_000, 0);
        let large = entry(&[outpoint(2)], MAX_SHARE_TRANSACTION_BYTES, 0);
        let fits_after = entry(&[outpoint(3)], 1_000, 0);

        let selected =
            select_within_budget([&small, &large, &fits_after].into_iter(), &HashSet::new());

        assert_eq!(
            selected
                .iter()
                .map(|transaction| transaction.compute_txid())
                .collect::<Vec<_>>(),
            vec![small.txid, fits_after.txid]
        );
    }

    /// The share's SigChecks budget: selection skips a transaction that
    /// would take the share past `BCH_MAX_SHARE_SIGCHECKS` and keeps taking
    /// ones that fit.
    #[test]
    fn selection_stops_at_the_share_sig_checks_budget() {
        let mut heavy = entry(&[outpoint(1)], 1_000, 0);
        heavy.sig_checks = BCH_MAX_SHARE_SIGCHECKS - 1;
        let mut over = entry(&[outpoint(2)], 1_000, 0);
        over.sig_checks = 2;
        let last = entry(&[outpoint(3)], 1_000, 0);

        let selected = select_within_budget([&heavy, &over, &last].into_iter(), &HashSet::new());

        assert_eq!(
            selected
                .iter()
                .map(|transaction| transaction.compute_txid())
                .collect::<Vec<_>>(),
            vec![heavy.txid, last.txid]
        );
    }

    /// A transaction executing more than BCH's 3,000 SigChecks is refused,
    /// whatever its scripts; 3,000 is accepted.
    #[test]
    fn a_transaction_past_max_tx_sig_checks_is_refused() {
        let inputs: Vec<OutPoint> = (0..=bch_script::MAX_TX_SIGCHECKS as u32)
            .map(|vout| OutPoint::new(Txid::from_byte_array([7; 32]), vout))
            .collect();
        let over = entry(&inputs, 1_000, 0);
        assert!(matches!(
            check_sig_checks(&over.transaction.0),
            Err(PoolRejection::TooManySigChecks {
                sig_checks: 3_001,
                limit: 3_000
            })
        ));
        let at_limit = entry(&inputs[1..], 1_000, 0);
        assert_eq!(check_sig_checks(&at_limit.transaction.0).unwrap(), 3_000);
    }

    #[test]
    fn selection_skips_a_transaction_spending_what_the_branch_spent() {
        let spent_on_branch = entry(&[outpoint(1)], 1_000, 0);
        let free = entry(&[outpoint(2)], 1_000, 0);
        let branch_spends = HashSet::from([outpoint(1)]);

        let selected = select_within_budget([&spent_on_branch, &free].into_iter(), &branch_spends);

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].compute_txid(), free.txid);
    }

    #[test]
    fn a_coinbase_is_refused() {
        let coinbase = test_coinbase_transaction(1);

        assert!(matches!(
            check_transaction_shape(&coinbase),
            Err(PoolRejection::Coinbase)
        ));
    }

    #[test]
    fn a_transaction_spending_one_output_twice_is_refused() {
        let twice = entry(&[outpoint(1), outpoint(1)], 100, 0);

        assert!(matches!(
            check_transaction_shape(&twice.transaction),
            Err(PoolRejection::DuplicateInput(spent)) if spent == outpoint(1)
        ));
    }

    #[tokio::test]
    async fn submit_accepts_a_lock_of_a_mature_share_output_and_queues_it_for_relay() {
        let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
        let blocks = build_confirmed_chain_owned_from(
            &chain_store_handle,
            ShareOwner::CashP2pkh(seller().pubkey_hash()),
            100,
        )
        .await;
        let (pool, mut relay_rx) = ShareTransactionPool::new(chain_store_handle);
        let lock = lock_of(&blocks[1], 40_000_000);

        let outcome = pool.submit(lock.clone(), TransactionSource::Local).unwrap();

        assert_eq!(outcome, SubmitOutcome::Accepted(lock.compute_txid()));
        assert!(pool.contains(&lock.compute_txid()));
        let relay = relay_rx.try_recv().unwrap();
        assert_eq!(relay.transaction, lock);
        assert_eq!(relay.source, None);
    }

    #[tokio::test]
    async fn a_transaction_already_in_the_pool_is_not_relayed_again() {
        let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
        let blocks = build_confirmed_chain_owned_from(
            &chain_store_handle,
            ShareOwner::CashP2pkh(seller().pubkey_hash()),
            100,
        )
        .await;
        let (pool, mut relay_rx) = ShareTransactionPool::new(chain_store_handle);
        let lock = lock_of(&blocks[1], 40_000_000);
        pool.submit(lock.clone(), TransactionSource::Local).unwrap();
        relay_rx.try_recv().unwrap();

        let outcome = pool
            .submit(lock.clone(), TransactionSource::Peer(PeerId::random()))
            .unwrap();

        assert_eq!(outcome, SubmitOutcome::AlreadyInPool(lock.compute_txid()));
        assert!(relay_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn a_relayed_transaction_names_the_peer_it_came_from() {
        let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
        let blocks = build_confirmed_chain_owned_from(
            &chain_store_handle,
            ShareOwner::CashP2pkh(seller().pubkey_hash()),
            100,
        )
        .await;
        let (pool, mut relay_rx) = ShareTransactionPool::new(chain_store_handle);
        let peer = PeerId::random();

        pool.submit(
            lock_of(&blocks[1], 40_000_000),
            TransactionSource::Peer(peer),
        )
        .unwrap();

        assert_eq!(relay_rx.try_recv().unwrap().source, Some(peer));
    }

    #[tokio::test]
    async fn a_spend_of_an_immature_share_coinbase_is_refused() {
        let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
        let blocks = build_confirmed_chain_owned_from(
            &chain_store_handle,
            ShareOwner::CashP2pkh(seller().pubkey_hash()),
            50,
        )
        .await;
        let (pool, _relay_rx) = ShareTransactionPool::new(chain_store_handle);

        let refused = pool.submit(lock_of(&blocks[1], 40_000_000), TransactionSource::Local);

        assert!(matches!(
            refused,
            Err(PoolRejection::Prevout(
                PrevoutRejection::ImmatureCoinbase { .. }
            ))
        ));
        assert!(pool.is_empty());
    }

    #[tokio::test]
    async fn a_second_spend_of_a_pool_transaction_input_is_refused() {
        let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
        let blocks = build_confirmed_chain_owned_from(
            &chain_store_handle,
            ShareOwner::CashP2pkh(seller().pubkey_hash()),
            100,
        )
        .await;
        let (pool, _relay_rx) = ShareTransactionPool::new(chain_store_handle);
        pool.submit(lock_of(&blocks[1], 40_000_000), TransactionSource::Local)
            .unwrap();

        let refused = pool.submit(lock_of(&blocks[1], 30_000_000), TransactionSource::Local);

        assert!(matches!(refused, Err(PoolRejection::Conflict { .. })));
        assert_eq!(pool.len(), 1);
    }

    #[tokio::test]
    async fn a_spend_signed_by_another_key_is_refused() {
        let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
        let blocks = build_confirmed_chain_owned_from(
            &chain_store_handle,
            ShareOwner::CashP2pkh(seller().pubkey_hash()),
            100,
        )
        .await;
        let (pool, _relay_rx) = ShareTransactionPool::new(chain_store_handle);
        let signed_by_the_buyer = build_lock_transaction(
            &[Coin {
                outpoint: OutPoint::new(blocks[1].transactions[0].compute_txid(), 0),
                value: Amount::from_sat(SHARE_UNITS),
            }],
            &buyer(),
            &contract(600),
            Amount::from_sat(SHARE_UNITS),
            &buyer().p2pkh_script(),
            0,
        )
        .unwrap();

        let refused = pool.submit(signed_by_the_buyer, TransactionSource::Local);

        assert!(matches!(refused, Err(PoolRejection::Script(_))));
    }

    // The sim build's PAYOUT_DEPTH is 100, so there a share coinbase output
    // can be spent only exactly 100 shares deep and anything built on it
    // expires a share later. The tests that go deeper run in production
    // builds only.
    #[cfg(not(feature = "sim"))]
    #[tokio::test]
    async fn a_refund_before_its_refund_height_is_refused() {
        let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
        let blocks = build_confirmed_chain_owned_from(
            &chain_store_handle,
            ShareOwner::CashP2pkh(seller().pubkey_hash()),
            100,
        )
        .await;
        let lock = lock_of(&blocks[1], SHARE_UNITS);
        let lock_block = TestShareBlockBuilder::new()
            .prev_share_blockhash(blocks[100].block_hash().to_string())
            .time(1_700_000_101)
            .add_transaction(lock.clone())
            .build();
        confirm_test_block(&chain_store_handle, &lock_block).await;
        let refund = build_refund_transaction(
            &Coin {
                outpoint: OutPoint::new(lock.compute_txid(), 0),
                value: Amount::from_sat(SHARE_UNITS),
            },
            &contract(600),
            &seller(),
            &seller().p2pkh_script(),
            0,
        )
        .unwrap();
        let (pool, _relay_rx) = ShareTransactionPool::new(chain_store_handle);

        let refused = pool.submit(refund, TransactionSource::Local);

        assert!(matches!(
            refused,
            Err(PoolRejection::NotFinal { height: 102, .. })
        ));
    }

    #[tokio::test]
    async fn selection_returns_a_ready_transaction_for_the_confirmed_tip() {
        let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
        let blocks = build_confirmed_chain_owned_from(
            &chain_store_handle,
            ShareOwner::CashP2pkh(seller().pubkey_hash()),
            100,
        )
        .await;
        let (pool, _relay_rx) = ShareTransactionPool::new(chain_store_handle);
        let lock = lock_of(&blocks[1], 40_000_000);
        pool.submit(lock.clone(), TransactionSource::Local).unwrap();

        let selected = pool.select_for_share(blocks[100].block_hash());

        assert_eq!(selected, vec![ShareTransaction(lock)]);
    }

    #[cfg(not(feature = "sim"))]
    #[tokio::test]
    async fn a_confirmed_transaction_leaves_the_pool_and_a_reorg_brings_it_back() {
        let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
        let blocks = build_confirmed_chain_owned_from(
            &chain_store_handle,
            ShareOwner::CashP2pkh(seller().pubkey_hash()),
            100,
        )
        .await;
        let (pool, _relay_rx) = ShareTransactionPool::new(chain_store_handle.clone());
        let lock = lock_of(&blocks[1], 40_000_000);
        pool.submit(lock.clone(), TransactionSource::Local).unwrap();
        let lock_block = TestShareBlockBuilder::new()
            .prev_share_blockhash(blocks[100].block_hash().to_string())
            .time(1_700_000_101)
            .add_transaction(lock.clone())
            .build();
        confirm_test_block(&chain_store_handle, &lock_block).await;

        let on_the_lock_block = pool.select_for_share(lock_block.block_hash());

        assert!(on_the_lock_block.is_empty());
        assert!(pool.is_empty());

        // A heavier fork from below the lock block, without the lock.
        let fork_first = TestShareBlockBuilder::new()
            .prev_share_blockhash(blocks[100].block_hash().to_string())
            .time(1_700_000_202)
            .build();
        let fork_second = TestShareBlockBuilder::new()
            .prev_share_blockhash(fork_first.block_hash().to_string())
            .time(1_700_000_203)
            .build();
        confirm_test_block(&chain_store_handle, &fork_first).await;
        confirm_test_block(&chain_store_handle, &fork_second).await;
        assert!(!chain_store_handle.is_block_confirmed(&lock_block.block_hash()));

        let on_the_fork = pool.select_for_share(fork_second.block_hash());

        assert_eq!(on_the_fork, vec![ShareTransaction(lock.clone())]);
        assert!(pool.contains(&lock.compute_txid()));
    }

    #[cfg(not(feature = "sim"))]
    #[tokio::test]
    async fn selection_skips_a_transaction_a_share_above_the_confirmed_tip_carries() {
        let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
        let blocks = build_confirmed_chain_owned_from(
            &chain_store_handle,
            ShareOwner::CashP2pkh(seller().pubkey_hash()),
            100,
        )
        .await;
        let (pool, _relay_rx) = ShareTransactionPool::new(chain_store_handle.clone());
        let lock = lock_of(&blocks[1], 40_000_000);
        pool.submit(lock.clone(), TransactionSource::Local).unwrap();
        let unconfirmed_lock_block = TestShareBlockBuilder::new()
            .prev_share_blockhash(blocks[100].block_hash().to_string())
            .time(1_700_000_101)
            .add_transaction(lock.clone())
            .build();
        chain_store_handle
            .add_share_block(unconfirmed_lock_block.clone())
            .await
            .unwrap();
        chain_store_handle
            .organise_header(unconfirmed_lock_block.header.clone())
            .await
            .unwrap();
        assert!(!chain_store_handle.is_block_confirmed(&unconfirmed_lock_block.block_hash()));

        let selected = pool.select_for_share(unconfirmed_lock_block.block_hash());

        assert!(selected.is_empty());
        assert!(pool.contains(&lock.compute_txid()));
    }
}
