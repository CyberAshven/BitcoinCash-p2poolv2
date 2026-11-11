// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The pool of Bitcoin Cash weak shares waiting for a share to carry them
//! (`docs/sharechain/small-miners-design.md`, part 3, "How a share carries
//! them").
//!
//! A node takes weak shares from its own miners and from peers, checks
//! each before it relays it (`ShareValidator::validate_weak_share`), keeps
//! it until the best chain carries it or its parent falls more than
//! `WEAK_DEPTH` below the tip, and fills its own shares from it, oldest
//! parent first so none expire. Node policy, not a share-chain rule: a share
//! may carry any valid weak shares, and a node may carry none.
//!
//! Each weak share is known by its proof-of-work hash. A hash seen once is
//! not taken again from anyone, and weak shares from peers are rate limited
//! per peer.
//!
//! Locking: the pool's state sits behind one mutex, held only for in-memory
//! work. It is never held across an await, a store read or another lock.

use crate::shares::chain::chain_store_handle::ChainStoreHandle;
use crate::shares::share_block::is_terminal_blockhash;
use crate::shares::weak::{WEAK_CAP, WEAK_DEPTH, WeakShare};
use bitcoin::BlockHash;
use libp2p::PeerId;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Write;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tracing::debug;

/// Most weak shares the pool holds: four full shares' worth on each of the
/// `WEAK_DEPTH` parents a weak share may build on.
pub const MAX_POOL_WEAK_SHARES: usize = 4 * WEAK_CAP * WEAK_DEPTH as usize;
/// Proof-of-work hashes the pool remembers having seen, so a weak share
/// relayed by several peers is checked once.
pub const MAX_SEEN_WEAK_SHARES: usize = 8 * MAX_POOL_WEAK_SHARES;
/// Average weak shares a second the pool takes from one peer. A full pool
/// makes at most `WEAK_CAP` per share, 6.4 a second at 10-second shares.
pub const PEER_WEAK_SHARES_PER_SECOND: u64 = 20;
/// Most weak shares the pool takes from one peer in one burst.
pub const PEER_WEAK_SHARE_BURST: u64 = 2 * WEAK_CAP as u64;
/// Accepted weak shares waiting for the node actor to relay them.
pub const WEAK_RELAY_CHANNEL_CAPACITY: usize = 512;
/// Milli-tokens in one token of a peer's rate limiter.
const MILLIS_PER_TOKEN: u64 = 1_000;

/// Where a weak share came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeakShareSource {
    /// One of this node's own miners. Not rate limited.
    Local,
    /// A peer relayed it. Rate limited, and not relayed back to it.
    Peer(PeerId),
}

impl WeakShareSource {
    fn peer(self) -> Option<PeerId> {
        match self {
            WeakShareSource::Local => None,
            WeakShareSource::Peer(peer) => Some(peer),
        }
    }
}

/// A weak share the pool accepted, for the node actor to send to every peer
/// but `source`.
#[derive(Debug, Clone)]
pub struct RelayWeakShare {
    pub weak_share: WeakShare,
    pub source: Option<PeerId>,
}

pub type WeakRelayReceiver = mpsc::Receiver<RelayWeakShare>;

/// Why the pool did not take a weak share: the `reason` label of
/// `weak_shares_refused_total`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeakShareRefusal {
    /// Its proof-of-work hash was seen before, usually from another peer.
    Duplicate,
    /// Its peer sent weak shares faster than [`PEER_WEAK_SHARES_PER_SECOND`].
    RateLimited,
    /// It broke a share-chain rule.
    Invalid,
    /// It could not be checked: data the check needs was missing, or a store
    /// read failed.
    Unchecked,
    /// The pool was full of weak shares on parents no older than its own.
    PoolFull,
}

impl WeakShareRefusal {
    /// Every reason, in the order the exposition lists them.
    pub const ALL: [WeakShareRefusal; 5] = [
        WeakShareRefusal::Duplicate,
        WeakShareRefusal::RateLimited,
        WeakShareRefusal::Invalid,
        WeakShareRefusal::Unchecked,
        WeakShareRefusal::PoolFull,
    ];

    /// The reason as its metric label.
    pub fn label(self) -> &'static str {
        match self {
            WeakShareRefusal::Duplicate => "duplicate",
            WeakShareRefusal::RateLimited => "rate_limited",
            WeakShareRefusal::Invalid => "invalid",
            WeakShareRefusal::Unchecked => "unchecked",
            WeakShareRefusal::PoolFull => "pool_full",
        }
    }
}

/// Weak shares the pool did not take since the node started, by reason.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WeakShareRefusals {
    pub duplicate: u64,
    pub rate_limited: u64,
    pub invalid: u64,
    pub unchecked: u64,
    pub pool_full: u64,
}

impl WeakShareRefusals {
    /// Count one weak share refused for `refusal`.
    fn count(&mut self, refusal: WeakShareRefusal) {
        match refusal {
            WeakShareRefusal::Duplicate => self.duplicate += 1,
            WeakShareRefusal::RateLimited => self.rate_limited += 1,
            WeakShareRefusal::Invalid => self.invalid += 1,
            WeakShareRefusal::Unchecked => self.unchecked += 1,
            WeakShareRefusal::PoolFull => self.pool_full += 1,
        }
    }

    /// Weak shares refused for `refusal`.
    pub fn of(&self, refusal: WeakShareRefusal) -> u64 {
        match refusal {
            WeakShareRefusal::Duplicate => self.duplicate,
            WeakShareRefusal::RateLimited => self.rate_limited,
            WeakShareRefusal::Invalid => self.invalid,
            WeakShareRefusal::Unchecked => self.unchecked,
            WeakShareRefusal::PoolFull => self.pool_full,
        }
    }
}

/// What the pool has done since the node started.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WeakPoolCounts {
    /// Weak shares this node's own miners found that passed the checks.
    pub found: u64,
    /// Weak shares from peers that passed the checks.
    pub received: u64,
    /// Weak shares queued for relay to every peer but the one that sent
    /// them.
    pub relayed: u64,
    /// Weak shares the best chain carried, dropped from the pool.
    pub carried: u64,
    /// Weak shares whose parent fell more than `WEAK_DEPTH` below the tip
    /// before any share on the best chain carried them.
    pub expired: u64,
    /// Weak shares the pool did not take, by reason. A weak share dropped
    /// on a full worker queue, before the pool sees it, is not counted.
    pub refused: WeakShareRefusals,
}

/// What the pool has done since the node started and what it holds now:
/// the weak pool's series of `/metrics`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WeakPoolStats {
    pub counts: WeakPoolCounts,
    /// Weak shares waiting in the pool for a share to carry them.
    pub pooled: usize,
}

impl WeakPoolStats {
    /// Render as Prometheus exposition text. The counters reset when the
    /// node restarts, which `rate()` handles; the refusal reasons are a fixed
    /// set, so the series stay bounded.
    pub fn exposition(&self) -> String {
        let counts = &self.counts;
        let series: [(&str, &str, &str, u64); 5] = [
            (
                "weak_shares_found_total",
                "counter",
                "Weak shares this node's miners found that passed the checks",
                counts.found,
            ),
            (
                "weak_shares_received_total",
                "counter",
                "Weak shares from peers that passed the checks",
                counts.received,
            ),
            (
                "weak_shares_relayed_total",
                "counter",
                "Weak shares queued for relay to every peer but the one that sent them",
                counts.relayed,
            ),
            (
                "weak_shares_expired_total",
                "counter",
                "Weak shares dropped from the pool because no share could carry them any more",
                counts.expired,
            ),
            (
                "weak_pool_size",
                "gauge",
                "Weak shares waiting in the pool for a share to carry them",
                self.pooled as u64,
            ),
        ];
        let mut exposition =
            String::with_capacity((series.len() + WeakShareRefusal::ALL.len()) * 160);
        for (name, kind, help, value) in series {
            // Writing to a String cannot fail.
            let _ = writeln!(exposition, "# HELP {name} {help}");
            let _ = writeln!(exposition, "# TYPE {name} {kind}");
            let _ = writeln!(exposition, "{name} {value}");
        }
        let _ = writeln!(
            exposition,
            "# HELP weak_shares_refused_total Weak shares the pool did not take, by reason"
        );
        let _ = writeln!(exposition, "# TYPE weak_shares_refused_total counter");
        for refusal in WeakShareRefusal::ALL {
            let _ = writeln!(
                exposition,
                "weak_shares_refused_total{{reason=\"{}\"}} {}",
                refusal.label(),
                counts.refused.of(refusal)
            );
        }
        exposition
    }
}

/// A weak share waiting in the pool.
struct PooledWeakShare {
    weak_share: WeakShare,
    /// Height of the share it is mined on.
    parent_height: u32,
    /// Arrival order, which breaks ties between equal parent heights.
    sequence: u64,
}

/// A peer's token bucket, in milli-tokens.
struct PeerAllowance {
    tokens: u64,
    updated_at_millis: u64,
}

struct WeakPoolState {
    pooled: HashMap<BlockHash, PooledWeakShare>,
    seen: HashSet<BlockHash>,
    seen_order: VecDeque<BlockHash>,
    allowances: HashMap<PeerId, PeerAllowance>,
    next_sequence: u64,
    counts: WeakPoolCounts,
}

impl WeakPoolState {
    fn new() -> Self {
        Self {
            pooled: HashMap::with_capacity(MAX_POOL_WEAK_SHARES),
            seen: HashSet::with_capacity(MAX_SEEN_WEAK_SHARES),
            seen_order: VecDeque::with_capacity(MAX_SEEN_WEAK_SHARES),
            allowances: HashMap::with_capacity(16),
            next_sequence: 0,
            counts: WeakPoolCounts::default(),
        }
    }

    /// Remember `pow_hash` as seen, forgetting the oldest beyond the bound.
    fn mark_seen(&mut self, pow_hash: BlockHash) -> bool {
        if !self.seen.insert(pow_hash) {
            return false;
        }
        self.seen_order.push_back(pow_hash);
        if self.seen_order.len() > MAX_SEEN_WEAK_SHARES
            && let Some(oldest) = self.seen_order.pop_front()
        {
            self.seen.remove(&oldest);
        }
        true
    }

    /// Take one token from `peer`'s bucket, refilled at
    /// [`PEER_WEAK_SHARES_PER_SECOND`] up to [`PEER_WEAK_SHARE_BURST`].
    fn take_peer_token(&mut self, peer: PeerId, now_millis: u64) -> bool {
        let full = PEER_WEAK_SHARE_BURST * MILLIS_PER_TOKEN;
        let allowance = self.allowances.entry(peer).or_insert(PeerAllowance {
            tokens: full,
            updated_at_millis: now_millis,
        });
        let elapsed = now_millis.saturating_sub(allowance.updated_at_millis);
        allowance.tokens = allowance
            .tokens
            .saturating_add(elapsed.saturating_mul(PEER_WEAK_SHARES_PER_SECOND))
            .min(full);
        allowance.updated_at_millis = now_millis;
        if allowance.tokens < MILLIS_PER_TOKEN {
            return false;
        }
        allowance.tokens -= MILLIS_PER_TOKEN;
        true
    }

    /// Drop the weak shares whose parent is below `min_parent_height`.
    fn expire_below(&mut self, min_parent_height: u32) {
        let before = self.pooled.len();
        self.pooled
            .retain(|_, pooled| pooled.parent_height >= min_parent_height);
        self.counts.expired += (before - self.pooled.len()) as u64;
    }

    /// Drop the weak shares the best chain carries.
    fn remove_carried(&mut self, carried: &HashSet<BlockHash>) {
        let before = self.pooled.len();
        self.pooled
            .retain(|pow_hash, _| !carried.contains(pow_hash));
        self.counts.carried += (before - self.pooled.len()) as u64;
    }

    /// The pooled weak shares mined on parents up to `max_parent_height`,
    /// oldest parent first, then in arrival order.
    fn oldest_first(&self, max_parent_height: u32) -> Vec<&PooledWeakShare> {
        let mut candidates: Vec<&PooledWeakShare> = self
            .pooled
            .values()
            .filter(|pooled| pooled.parent_height <= max_parent_height)
            .collect();
        candidates.sort_by_key(|pooled| (pooled.parent_height, pooled.sequence));
        candidates
    }
}

/// The shared, bounded pool of weak shares. Cloning shares the pool.
#[derive(Clone)]
pub struct WeakPool {
    state: Arc<Mutex<WeakPoolState>>,
    chain_store_handle: ChainStoreHandle,
    relay_tx: mpsc::Sender<RelayWeakShare>,
}

impl WeakPool {
    /// An empty pool reading `chain_store_handle`, and the receiver of the
    /// weak shares it accepts, for the node actor to relay.
    pub fn new(chain_store_handle: ChainStoreHandle) -> (Self, WeakRelayReceiver) {
        let (relay_tx, relay_rx) = mpsc::channel(WEAK_RELAY_CHANNEL_CAPACITY);
        let pool = Self {
            state: Arc::new(Mutex::new(WeakPoolState::new())),
            chain_store_handle,
            relay_tx,
        };
        (pool, relay_rx)
    }

    fn lock(&self) -> MutexGuard<'_, WeakPoolState> {
        self.state.lock().expect("weak pool lock poisoned")
    }

    /// Whether to check the weak share with `pow_hash`: one not seen before,
    /// and from a peer within its rate. A weak share admitted is remembered
    /// as seen whatever its check finds, so no copy of it is checked again.
    /// A weak share not admitted is counted as refused, a duplicate or over
    /// its peer's rate.
    pub fn admit(&self, pow_hash: BlockHash, source: WeakShareSource) -> bool {
        let mut state = self.lock();
        if state.seen.contains(&pow_hash) {
            state.counts.refused.count(WeakShareRefusal::Duplicate);
            return false;
        }
        if let Some(peer) = source.peer()
            && !state.take_peer_token(peer, now_millis())
        {
            debug!("Peer {peer} is over its weak share rate, dropping {pow_hash}");
            state.counts.refused.count(WeakShareRefusal::RateLimited);
            return false;
        }
        state.mark_seen(pow_hash)
    }

    /// Count a weak share the worker did not pool, for `refusal`.
    pub fn refuse(&self, refusal: WeakShareRefusal) {
        self.lock().counts.refused.count(refusal);
    }

    /// Keep a weak share that passed its checks, mined on a share at
    /// `parent_height`, and queue it for relay to every peer but its source.
    /// A full pool keeps it only in place of a weak share on an older parent,
    /// which would expire first.
    pub fn insert(&self, weak_share: WeakShare, parent_height: u32, source: WeakShareSource) {
        let pow_hash = weak_share.pow_hash();
        {
            let mut state = self.lock();
            if state.pooled.len() >= MAX_POOL_WEAK_SHARES {
                let oldest = state
                    .pooled
                    .iter()
                    .min_by_key(|(_, pooled)| (pooled.parent_height, pooled.sequence))
                    .map(|(hash, pooled)| (*hash, pooled.parent_height));
                match oldest {
                    Some((oldest_hash, oldest_height)) if oldest_height < parent_height => {
                        state.pooled.remove(&oldest_hash);
                        state.counts.expired += 1;
                    }
                    _ => {
                        debug!("Weak pool full, dropping weak share {pow_hash}");
                        state.counts.refused.count(WeakShareRefusal::PoolFull);
                        return;
                    }
                }
            }
            let sequence = state.next_sequence;
            state.next_sequence += 1;
            match source {
                WeakShareSource::Local => state.counts.found += 1,
                WeakShareSource::Peer(_) => state.counts.received += 1,
            }
            state.pooled.insert(
                pow_hash,
                PooledWeakShare {
                    weak_share: weak_share.clone(),
                    parent_height,
                    sequence,
                },
            );
        }
        let relay = RelayWeakShare {
            weak_share,
            source: source.peer(),
        };
        if self.relay_tx.try_send(relay).is_err() {
            debug!("Weak share relay queue full, not relaying {pow_hash}");
        } else {
            self.lock().counts.relayed += 1;
        }
    }

    /// The weak shares a share built on `mining_base`, at
    /// `mining_base_height`, may carry: each mined on a parent at most
    /// `WEAK_DEPTH` below that share and below it, and none carried by the
    /// mining base or its ancestors that could have carried it. Oldest parent
    /// first, so none expire; the job builder keeps the first that fit
    /// `WEAK_CAP` and `WEAK_BYTES`.
    ///
    /// Drops the weak shares that expired and those the chain ending at the
    /// mining base, the best chain, carries. A store read that fails leaves
    /// the share carrying none, which is always valid.
    pub fn select_for_share(
        &self,
        mining_base: BlockHash,
        mining_base_height: u32,
    ) -> Vec<WeakShare> {
        let carrier_height = mining_base_height + 1;
        let min_parent_height = carrier_height.saturating_sub(WEAK_DEPTH);
        let any_pooled = {
            let mut state = self.lock();
            state.expire_below(min_parent_height);
            !state.pooled.is_empty()
        };
        if !any_pooled {
            return Vec::new();
        }
        let carried = match carried_by_chain(mining_base, &self.chain_store_handle) {
            Ok(carried) => carried,
            Err(error) => {
                debug!("No weak shares for a share on {mining_base}: {error}");
                return Vec::new();
            }
        };
        let mut state = self.lock();
        state.remove_carried(&carried);
        state
            .oldest_first(mining_base_height)
            .into_iter()
            .take(2 * WEAK_CAP)
            .map(|pooled| pooled.weak_share.clone())
            .collect()
    }

    /// What the pool has done since the node started.
    pub fn counts(&self) -> WeakPoolCounts {
        self.lock().counts
    }

    /// What the pool has done since the node started and how many weak
    /// shares it holds, read under one lock.
    pub fn stats(&self) -> WeakPoolStats {
        let state = self.lock();
        WeakPoolStats {
            counts: state.counts,
            pooled: state.pooled.len(),
        }
    }

    /// Weak shares waiting in the pool.
    pub fn len(&self) -> usize {
        self.lock().pooled.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lock().pooled.is_empty()
    }
}

/// The proof-of-work hashes of the weak shares that `mining_base` and the
/// `WEAK_DEPTH - 2` shares below it carry: every share of the best chain
/// that could already carry a weak share a share on `mining_base` may carry.
fn carried_by_chain(
    mining_base: BlockHash,
    chain_store_handle: &ChainStoreHandle,
) -> Result<HashSet<BlockHash>, crate::store::writer::StoreError> {
    let ancestor_count = WEAK_DEPTH as usize - 1;
    let mut carriers = Vec::with_capacity(ancestor_count);
    let mut current = mining_base;
    let mut steps = 0;
    while steps < ancestor_count && !is_terminal_blockhash(&current) {
        let header = chain_store_handle.get_share_header(&current)?;
        if header.weak_count > 0 {
            carriers.push(current);
        }
        current = header.prev_share_blockhash;
        steps += 1;
    }
    if carriers.is_empty() {
        return Ok(HashSet::new());
    }
    Ok(chain_store_handle
        .get_weak_summaries(&carriers)?
        .into_values()
        .flat_map(|summary| summary.pow_hashes)
        .collect())
}

/// Milliseconds since the epoch, 0 before it.
fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
