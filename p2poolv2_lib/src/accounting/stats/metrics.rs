// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::accounting::stats::pool_local_stats::load_pool_local_stats;
use crate::accounting::stats::user::User;
use crate::accounting::{payout::simple_pplns::SimplePplnsShare, stats::pool_local_stats};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::{mpsc, oneshot, watch};
use tracing::error;

const METRICS_MESSAGE_BUFFER_SIZE: usize = 1000;
pub const INITIAL_USER_MAP_CAPACITY: usize = 1000;
const METRICS_SAVE_INTERVAL: u64 = 5;
/// The sliding window the local (this node's miners) hashrate is computed
/// over, in seconds: ten minutes, matching p2poolv1's local hash rate.
const SUBMISSION_WINDOW_SECS: u64 = 600;
/// Hashes a difficulty-1 share takes on average (2^32).
const HASHES_PER_DIFFICULTY: f64 = 4_294_967_296.0;
/// How often the actor publishes per-miner `w_i` to its watchers. The
/// 600-second window moves on every accepted share, so a per-share publish
/// would rebuild the O(miners) map continuously; a slow tick keeps the
/// synchronous job builders' snapshot fresh enough for tier boundaries.
pub const TIER_PUBLISH_INTERVAL: Duration = Duration::from_secs(1);
/// Publish-time EMA smoothing factor for a miner's `w_i`: the weight of each
/// new raw sample in the published value. Raw `w_i` jitters on every share,
/// which would flip stratum difficulty around a tier boundary; damping makes
/// boundaries move only when the underlying hashpower share moves.
pub const TIER_WEIGHT_SMOOTHING: f64 = 0.2;

/// The current unix time in seconds, 0 before the epoch.
fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// Hashrate of `window` from its work in the trailing
/// [`SUBMISSION_WINDOW_SECS`] seconds, in hashes per second: the work over
/// the span from the first kept share to `now`, which is the full ten minutes
/// once the window is that old. `None` with no work in the window, or no time
/// between the first kept share and `now` (the dash, not zero).
fn window_hashrate(window: &VecDeque<WorkDatum>, now: u64) -> Option<f64> {
    let cutoff = now.saturating_sub(SUBMISSION_WINDOW_SECS);
    let mut first: Option<u64> = None;
    let mut work = 0.0;
    for datum in window {
        if datum.at < cutoff {
            continue;
        }
        first = Some(first.map_or(datum.at, |earliest: u64| earliest.min(datum.at)));
        work += datum.work;
    }
    let first = first?;
    let span = now.saturating_sub(first).min(SUBMISSION_WINDOW_SECS);
    if span == 0 {
        return None;
    }
    Some(work / span as f64)
}

/// One accepted share in the sliding window the local hashrate reads.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct WorkDatum {
    /// Unix seconds the share was accepted.
    at: u64,
    /// The share's work, in hashes: difficulty x HASHES_PER_DIFFICULTY.
    work: f64,
}
/// Represents the metrics for the P2Poolv2 pool, we derive the stats every five minutes from this
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PoolMetrics {
    /// Start time in unix timestamp
    pub start_time: u64,
    /// Last update timestamp, time since epoch in seconds
    pub lastupdate: Option<u64>,
    /// Total number of shares accepted
    pub accepted_total: u64,
    /// Total difficulty of shares accepted
    pub accepted_difficulty_total: u64,
    /// Total rejected shares
    pub rejected_total: u64,
    /// Highest difficulty share on this start
    pub best_share: u64,
    /// Highest difficulty share across restarts
    pub best_share_ever: u64,
    /// User metrics
    pub users: HashMap<String, User>,
    /// Current pool difficulty
    pub pool_difficulty: u64,
    /// Confirmed sharechain pool difficulty since the last bitcoin block was
    /// found; reset to zero on each block find. Numerator of the block effort
    /// metric (`work_since_last_block / network_difficulty`). Runtime-only (not
    /// persisted), mainnet-relative units matching network_difficulty. Pool
    /// hashrate is derived separately from confirmed-chain total work.
    #[serde(default)]
    pub work_since_last_block: f64,
    /// Found blocks sent to BCHN nodes since the node started, by node label
    /// (`primary` for `[bitcoinrpc]`, `extra1` and on for `[[submit_nodes]]`)
    /// and outcome label (`accepted`, `duplicate`, `inconclusive`,
    /// `rejected`, `failed`). Runtime only, not persisted.
    #[serde(skip)]
    pub found_block_submissions: BTreeMap<(String, String), u64>,
    /// Times a found block was sent again to a node after a connection
    /// error, a time-out or a busy node, by node label. Runtime only.
    #[serde(skip)]
    pub found_block_submit_retries: BTreeMap<String, u64>,
    /// Accepted shares in the trailing [`SUBMISSION_WINDOW_SECS`] seconds,
    /// oldest first, the local hashrate is computed from. Runtime only
    /// (not persisted): a restart starts empty, so the local hashrate
    /// reads as unknown until the first share arrives.
    #[serde(skip)]
    pub submission_window: VecDeque<WorkDatum>,
}

impl Default for PoolMetrics {
    fn default() -> Self {
        Self {
            lastupdate: None,
            accepted_total: 0,
            accepted_difficulty_total: 0,
            rejected_total: 0,
            start_time: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            best_share: 0,
            best_share_ever: 0,
            users: HashMap::with_capacity(INITIAL_USER_MAP_CAPACITY),
            pool_difficulty: 0,
            work_since_last_block: 0.0,
            found_block_submissions: BTreeMap::new(),
            found_block_submit_retries: BTreeMap::new(),
            submission_window: VecDeque::new(),
        }
    }
}

impl PoolMetrics {
    /// Load existing metrics from file or build new default
    pub fn load_existing(log_dir: &str) -> Result<Self, std::io::Error> {
        let pool_stats = load_pool_local_stats(log_dir)?;
        Ok(PoolMetrics {
            accepted_total: pool_stats.accepted_total,
            accepted_difficulty_total: pool_stats.accepted_difficulty_total,
            rejected_total: pool_stats.rejected_total,
            users: pool_stats.users,
            ..Default::default()
        })
    }

    /// This node's hashrate from accepted submissions in the trailing
    /// [`SUBMISSION_WINDOW_SECS`] seconds, in hashes per second. This is
    /// p2poolv1's local hash rate: the work submitted in the window over
    /// the window's span, which is the full ten minutes once the window is
    /// that old and the time since the first kept share before that.
    /// `None` with no shares in the window, or no time between the first
    /// kept share and `now` (the dash, not zero).
    pub fn local_submission_hashrate(&self, now: u64) -> Option<f64> {
        window_hashrate(&self.submission_window, now)
    }
}

/// Messages that can be sent to the MetricsActor
#[derive(Debug)]
pub enum MetricsMessage {
    RecordShareAccepted {
        btcaddress: String,
        workername: String,
        difficulty: u64,
        truediff: u64,
        response: oneshot::Sender<()>,
    },
    RecordShareRejected {
        response: oneshot::Sender<()>,
    },
    ResetBlockEffort {
        response: oneshot::Sender<()>,
    },
    RecordConfirmedShare {
        difficulty: f64,
        response: oneshot::Sender<()>,
    },
    IncrementWorkerCount {
        btcaddress: String,
        workername: String,
        response: oneshot::Sender<()>,
    },
    DecrementWorkerCount {
        btcaddress: Option<String>,
        workername: String,
        response: oneshot::Sender<()>,
    },
    Commit {
        response: oneshot::Sender<String>,
    },
    GetMetrics {
        response: oneshot::Sender<PoolMetrics>,
    },
    SetLastUpdate {
        lastupdate: u64,
        response: oneshot::Sender<()>,
    },
    RecordBlockSubmission {
        node: String,
        outcome: &'static str,
        retries: u32,
        response: oneshot::Sender<()>,
    },
}

/// The actor that manages pool metrics state
pub struct MetricsActor {
    metrics: PoolMetrics,
    receiver: mpsc::Receiver<MetricsMessage>,
    worker_connections: HashMap<(String, String), usize>,
    /// Accepted-share windows per miner address, keyed by `btcaddress`, the
    /// identity set at authorize. Actor-level state, not on `PoolMetrics`:
    /// it is never scraped nor persisted, and it is pruned on the same
    /// cutoff as the pool-wide window.
    per_miner_windows: HashMap<String, VecDeque<WorkDatum>>,
    /// Smoothed `w_i` per miner, carried between publishes so the EMA damps
    /// tier boundaries instead of restarting at each sample.
    smoothed_tiers: HashMap<String, f64>,
    /// Publishes the per-miner hashrate-share snapshot the job builders
    /// read synchronously through [`MetricsHandle::subscribe_tiers`].
    tier_tx: watch::Sender<Arc<HashMap<String, f64>>>,
}

impl MetricsActor {
    /// Create a new metrics actor with default metrics, publishing `w_i`
    /// snapshots through `tier_tx`.
    pub fn new(
        receiver: mpsc::Receiver<MetricsMessage>,
        tier_tx: watch::Sender<Arc<HashMap<String, f64>>>,
    ) -> Self {
        Self {
            metrics: PoolMetrics::default(),
            receiver,
            worker_connections: HashMap::with_capacity(INITIAL_USER_MAP_CAPACITY),
            per_miner_windows: HashMap::with_capacity(INITIAL_USER_MAP_CAPACITY),
            smoothed_tiers: HashMap::with_capacity(INITIAL_USER_MAP_CAPACITY),
            tier_tx,
        }
    }

    /// Create a metrics actor with metrics loaded from the given log
    /// directory, publishing `w_i` snapshots through `tier_tx`.
    pub fn with_existing_metrics(
        log_dir: &str,
        receiver: mpsc::Receiver<MetricsMessage>,
        tier_tx: watch::Sender<Arc<HashMap<String, f64>>>,
    ) -> Result<Self, std::io::Error> {
        let metrics = PoolMetrics::load_existing(log_dir)?;
        Ok(Self {
            metrics,
            receiver,
            worker_connections: HashMap::with_capacity(INITIAL_USER_MAP_CAPACITY),
            per_miner_windows: HashMap::with_capacity(INITIAL_USER_MAP_CAPACITY),
            smoothed_tiers: HashMap::with_capacity(INITIAL_USER_MAP_CAPACITY),
            tier_tx,
        })
    }

    /// Start the actor's message handling loop, publishing the `w_i`
    /// snapshot on a [`TIER_PUBLISH_INTERVAL`] cadence beside message
    /// handling.
    pub async fn run(mut self) {
        let mut publish_interval = tokio::time::interval(TIER_PUBLISH_INTERVAL);
        // The first tick is immediate; consume it so publishes start on the
        // cadence rather than before any share has been recorded.
        publish_interval.tick().await;
        loop {
            tokio::select! {
                message = self.receiver.recv() => match message {
                    Some(message) => self.handle_message(message).await,
                    None => break,
                },
                _ = publish_interval.tick() => self.publish_tiers(),
            }
        }
    }

    /// Publish the smoothed per-miner hashrate share (`w_i`) snapshot.
    fn publish_tiers(&mut self) {
        let snapshot = self.tier_snapshot(unix_seconds());
        // `send_replace` keeps the latest snapshot available to receivers
        // that subscribe later, even while none is connected.
        self.tier_tx.send_replace(Arc::new(snapshot));
    }

    /// Compute the smoothed per-miner hashrate share (`w_i`) snapshot at
    /// `now`.
    ///
    /// `w_i = miner_hashrate / pool_hashrate` over the same trailing
    /// [`SUBMISSION_WINDOW_SECS`] window the pool hashrate reads. A pool
    /// with no work in its window yields an empty snapshot, so the tier
    /// formula sees no miner above the floor and ceiling. New samples are
    /// damped by [`TIER_WEIGHT_SMOOTHING`] so a miner's tier does not flip
    /// on every share. Miners whose window holds no work in the trailing
    /// window are dropped, along with their smoothing state; a miner with
    /// only a fresh share (zero span) keeps its window but contributes no
    /// sample this round.
    fn tier_snapshot(&mut self, now: u64) -> HashMap<String, f64> {
        let cutoff = now.saturating_sub(SUBMISSION_WINDOW_SECS);
        self.per_miner_windows.retain(|_, window| {
            while window.front().is_some_and(|datum| datum.at < cutoff) {
                window.pop_front();
            }
            !window.is_empty()
        });
        self.smoothed_tiers
            .retain(|address, _| self.per_miner_windows.contains_key(address));
        let mut snapshot = HashMap::with_capacity(self.per_miner_windows.len());
        if let Some(pool_hashrate) =
            window_hashrate(&self.metrics.submission_window, now).filter(|hashrate| *hashrate > 0.0)
        {
            for (address, window) in &self.per_miner_windows {
                if let Some(miner_hashrate) = window_hashrate(window, now) {
                    let sample = miner_hashrate / pool_hashrate;
                    let smoothed = match self.smoothed_tiers.get(address) {
                        Some(previous) => previous + TIER_WEIGHT_SMOOTHING * (sample - previous),
                        None => sample,
                    };
                    self.smoothed_tiers.insert(address.clone(), smoothed);
                    snapshot.insert(address.clone(), smoothed);
                }
            }
        }
        snapshot
    }

    async fn handle_message(&mut self, msg: MetricsMessage) {
        match msg {
            MetricsMessage::RecordShareAccepted {
                btcaddress,
                workername,
                difficulty,
                truediff,
                response,
            } => {
                self.record_share_accepted(btcaddress, workername, difficulty, truediff);
                let _ = response.send(());
            }
            MetricsMessage::RecordShareRejected { response } => {
                self.record_share_rejected();
                let _ = response.send(());
            }
            MetricsMessage::ResetBlockEffort { response } => {
                self.reset_block_effort();
                let _ = response.send(());
            }
            MetricsMessage::RecordConfirmedShare {
                difficulty,
                response,
            } => {
                self.record_confirmed_share(difficulty);
                let _ = response.send(());
            }
            MetricsMessage::IncrementWorkerCount {
                btcaddress,
                workername,
                response,
            } => {
                self.worker_authorized(btcaddress, workername);
                let _ = response.send(());
            }
            MetricsMessage::DecrementWorkerCount {
                btcaddress,
                workername,
                response,
            } => {
                self.mark_worker_inactive(btcaddress, workername);
                let _ = response.send(());
            }
            MetricsMessage::Commit { response } => {
                let result = self.commit();
                let _ = response.send(result);
            }
            MetricsMessage::GetMetrics { response } => {
                let _ = response.send(self.metrics.clone());
            }
            MetricsMessage::SetLastUpdate {
                lastupdate,
                response,
            } => {
                self.set_last_update(lastupdate);
                let _ = response.send(());
            }
            MetricsMessage::RecordBlockSubmission {
                node,
                outcome,
                retries,
                response,
            } => {
                self.record_block_submission(node, outcome, retries);
                let _ = response.send(());
            }
        }
    }

    /// Count one found block sent to `node` with `outcome`, and the times it
    /// was sent again.
    fn record_block_submission(&mut self, node: String, outcome: &'static str, retries: u32) {
        *self
            .metrics
            .found_block_submit_retries
            .entry(node.clone())
            .or_insert(0) += u64::from(retries);
        *self
            .metrics
            .found_block_submissions
            .entry((node, outcome.to_string()))
            .or_insert(0) += 1;
    }

    /// Update metrics from accepted share
    ///
    /// Pushes the share's work onto the pool-wide and the submitting
    /// miner's trailing windows, both pruned to
    /// [`SUBMISSION_WINDOW_SECS`].
    fn record_share_accepted(
        &mut self,
        btcaddress: String,
        workername: String,
        difficulty: u64,
        truediff: u64,
    ) {
        let current_unix_timestamp = unix_seconds();
        self.metrics.accepted_total += 1;
        self.metrics.accepted_difficulty_total += difficulty;
        self.metrics.lastupdate = Some(current_unix_timestamp);
        self.metrics.best_share = self.metrics.best_share.max(truediff);
        self.metrics.best_share_ever = self.metrics.best_share_ever.max(truediff);
        let cutoff = current_unix_timestamp.saturating_sub(SUBMISSION_WINDOW_SECS);
        while self
            .metrics
            .submission_window
            .front()
            .is_some_and(|datum| datum.at < cutoff)
        {
            self.metrics.submission_window.pop_front();
        }
        let datum = WorkDatum {
            at: current_unix_timestamp,
            work: difficulty as f64 * HASHES_PER_DIFFICULTY,
        };
        self.metrics.submission_window.push_back(datum);
        let miner_window = self
            .per_miner_windows
            .entry(btcaddress.clone())
            .or_default();
        while miner_window.front().is_some_and(|datum| datum.at < cutoff) {
            miner_window.pop_front();
        }
        miner_window.push_back(datum);
        if let Some(user) = self.metrics.users.get_mut(&btcaddress) {
            user.record_share(&workername, difficulty, truediff, current_unix_timestamp);
        }
    }

    /// Update metrics from rejected share
    fn record_share_rejected(&mut self) {
        self.metrics.rejected_total += 1;
    }

    /// Reset the block effort accumulator after the pool finds a bitcoin
    /// block, so work counts toward the next block.
    ///
    /// The found block itself is recorded in the store, not here.
    fn reset_block_effort(&mut self) {
        self.metrics.work_since_last_block = 0.0;
    }

    /// Record the pool difficulty of a confirmed sharechain share for the
    /// per-block effort accumulator. Hashrate is derived separately from
    /// confirmed-chain total work.
    fn record_confirmed_share(&mut self, difficulty: f64) {
        self.metrics.work_since_last_block += difficulty;
    }

    /// Increment worker counts - called after worker has authorised successfully.
    /// Uses entry().or_default() to preserve existing worker stats on reconnect.
    fn worker_authorized(&mut self, btcaddress: String, workername: String) {
        *self
            .worker_connections
            .entry((btcaddress.clone(), workername.clone()))
            .or_insert(0) += 1;
        let worker = self
            .metrics
            .users
            .entry(btcaddress)
            .or_default()
            .workers
            .entry(workername)
            .or_default();
        worker.active = true;
        worker.best_share = 0;
    }

    /// Decrement pool wide worker counts, if worker found as authorised. Unauthorised workers are not counted.
    /// Keep a worker active until its last SV1 connection or SV2 channel closes.
    fn mark_worker_inactive(&mut self, btcaddress: Option<String>, workername: String) {
        if let Some(btcaddress) = btcaddress {
            let identity = (btcaddress, workername);
            let active = self
                .worker_connections
                .get_mut(&identity)
                .is_some_and(|count| {
                    *count = count.saturating_sub(1);
                    *count > 0
                });
            if !active {
                self.worker_connections.remove(&identity);
            }
            if let Some(user) = self.metrics.users.get_mut(&identity.0)
                && let Some(worker) = user.workers.get_mut(&identity.1)
            {
                worker.active = active;
            }
        }
    }

    /// Commit metrics
    /// Export current metrics as json, returning the serialized json
    /// Reset the metrics to start again
    fn commit(&mut self) -> String {
        serde_json::to_string(&self.metrics).unwrap()
    }

    /// Set last update time. Largely used for testing.
    fn set_last_update(&mut self, lastupdate: u64) {
        self.metrics.lastupdate = Some(lastupdate);
        for user in self.metrics.users.values_mut() {
            user.last_share_at = lastupdate;
            for worker in user.workers.values_mut() {
                worker.last_share_at = lastupdate;
            }
        }
    }
}

/// A handle to interact with the MetricsActor
#[derive(Clone)]
pub struct MetricsHandle {
    sender: mpsc::Sender<MetricsMessage>,
    /// The actor's `w_i` snapshot publication, mirrored onto the handle so
    /// job builders can subscribe synchronously.
    tier_tx: watch::Sender<Arc<HashMap<String, f64>>>,
}

impl MetricsHandle {
    /// Subscribe to the per-miner `w_i` snapshot the metrics actor
    /// publishes: hashrate share keyed by `btcaddress`.
    ///
    /// The receiver reads the latest published snapshot with `borrow`; it
    /// need not be driven to completion.
    pub fn subscribe_tiers(&self) -> watch::Receiver<Arc<HashMap<String, f64>>> {
        self.tier_tx.subscribe()
    }

    /// Record an accepted share with the given difficulty
    pub async fn record_share_accepted(
        &self,
        share: SimplePplnsShare,
        truediff: u64,
    ) -> Result<(), tokio::sync::oneshot::error::RecvError> {
        let (response_tx, response_rx) = oneshot::channel();
        self.sender
            .send(MetricsMessage::RecordShareAccepted {
                btcaddress: share.btcaddress.unwrap_or_default(),
                workername: share.workername.unwrap_or_default(),
                difficulty: share.difficulty,
                truediff,
                response: response_tx,
            })
            .await
            .expect("Error recording share");
        response_rx.await
    }

    /// Record a rejected share
    /// We don't difficulty or user info for rejected shares as the could be rejected for any reason
    pub async fn record_share_rejected(
        &self,
    ) -> Result<(), tokio::sync::oneshot::error::RecvError> {
        let (response_tx, response_rx) = oneshot::channel();
        self.sender
            .send(MetricsMessage::RecordShareRejected {
                response: response_tx,
            })
            .await
            .expect("Error recording share");
        response_rx.await
    }

    /// Reset the block effort accumulator after the pool finds a bitcoin
    /// block.
    pub async fn reset_block_effort(&self) -> Result<(), tokio::sync::oneshot::error::RecvError> {
        let (response_tx, response_rx) = oneshot::channel();
        self.sender
            .send(MetricsMessage::ResetBlockEffort {
                response: response_tx,
            })
            .await
            .expect("Error resetting block effort");
        response_rx.await
    }

    /// Record the pool difficulty of a confirmed sharechain share for the
    /// block effort metric.
    pub async fn record_confirmed_share(
        &self,
        difficulty: f64,
    ) -> Result<(), tokio::sync::oneshot::error::RecvError> {
        let (response_tx, response_rx) = oneshot::channel();
        self.sender
            .send(MetricsMessage::RecordConfirmedShare {
                difficulty,
                response: response_tx,
            })
            .await
            .expect("Error recording confirmed share");
        response_rx.await
    }

    /// Increment worker count
    pub async fn increment_worker_count(
        &self,
        btcaddress: String,
        workername: String,
    ) -> Result<(), tokio::sync::oneshot::error::RecvError> {
        let (response_tx, response_rx) = oneshot::channel();
        self.sender
            .send(MetricsMessage::IncrementWorkerCount {
                response: response_tx,
                btcaddress,
                workername,
            })
            .await
            .expect("Error incrementing worker count");
        response_rx.await
    }

    /// Decrement worker count
    pub async fn decrement_worker_count(
        &self,
        btcaddress: Option<String>,
        workername: String,
    ) -> Result<(), tokio::sync::oneshot::error::RecvError> {
        let (response_tx, response_rx) = oneshot::channel();
        self.sender
            .send(MetricsMessage::DecrementWorkerCount {
                btcaddress,
                workername,
                response: response_tx,
            })
            .await
            .expect("Error decrementing worker count");
        response_rx.await
    }

    /// Commit metrics and get JSON result
    pub async fn commit(&self) -> Result<String, tokio::sync::oneshot::error::RecvError> {
        let (response_tx, response_rx) = oneshot::channel();
        self.sender
            .send(MetricsMessage::Commit {
                response: response_tx,
            })
            .await
            .expect("Error committing metrics");
        response_rx.await
    }

    /// Get current metrics
    pub async fn get_metrics(&self) -> PoolMetrics {
        let (response_tx, response_rx) = oneshot::channel();
        self.sender
            .send(MetricsMessage::GetMetrics {
                response: response_tx,
            })
            .await
            .expect("Error getting metrics");
        response_rx.await.expect("Error getting metrics")
    }

    /// Count a found block sent to the BCHN node labelled `node` with
    /// `outcome` (a [`crate::stratum::block_submit::BlockSubmission`] label)
    /// after `retries` attempts that got no answer. A stopped actor is an
    /// error, not a panic: a found block is never lost to a metric.
    pub async fn record_block_submission(
        &self,
        node: String,
        outcome: &'static str,
        retries: u32,
    ) -> Result<(), tokio::sync::oneshot::error::RecvError> {
        let (response_tx, response_rx) = oneshot::channel();
        // A send to a stopped actor drops the message with its response
        // sender, which `response_rx` reports as an error.
        let _ = self
            .sender
            .send(MetricsMessage::RecordBlockSubmission {
                node,
                outcome,
                retries,
                response: response_tx,
            })
            .await;
        response_rx.await
    }

    /// Set last update time. Largely used for testing.
    pub async fn set_last_update(
        &self,
        lastupdate: u64,
    ) -> Result<(), tokio::sync::oneshot::error::RecvError> {
        let (response_tx, response_rx) = oneshot::channel();
        self.sender
            .send(MetricsMessage::SetLastUpdate {
                lastupdate,
                response: response_tx,
            })
            .await
            .expect("Error setting last update");
        response_rx.await
    }
}

/// Spawn a metrics actor with default metrics and return its handle.
///
/// Test-only helper for wiring components that require a `MetricsHandle`
/// without touching disk or the stats saver.
#[cfg(test)]
pub(crate) fn spawn_test_metrics_handle() -> MetricsHandle {
    let (sender, receiver) = mpsc::channel(METRICS_MESSAGE_BUFFER_SIZE);
    let (tier_tx, _tier_rx) = watch::channel(Arc::new(HashMap::new()));
    let actor_tier_tx = tier_tx.clone();
    tokio::spawn(async move {
        MetricsActor::new(receiver, actor_tier_tx).run().await;
    });
    MetricsHandle { sender, tier_tx }
}

/// Construct a new metrics actor with existing metrics and return its handle
pub async fn start_metrics(log_dir: String) -> Result<MetricsHandle, std::io::Error> {
    let (sender, receiver) = mpsc::channel(METRICS_MESSAGE_BUFFER_SIZE);
    let (tier_tx, _tier_rx) = watch::channel(Arc::new(HashMap::new()));
    let actor = MetricsActor::with_existing_metrics(&log_dir, receiver, tier_tx.clone())?;
    tokio::spawn(async move {
        actor.run().await;
    });
    let handle = MetricsHandle { sender, tier_tx };
    match pool_local_stats::start_stats_saver(
        handle.clone(),
        METRICS_SAVE_INTERVAL,
        log_dir.to_string(),
    )
    .await
    {
        Ok(_) => {}
        Err(e) => {
            error!("Failed to start stats saver: {e}");
            return Err(std::io::Error::other("Failed to start stats saver"));
        }
    }
    Ok(handle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounting::stats::pool_local_stats::save_pool_local_stats;

    #[test]
    fn shared_worker_stays_active_until_the_last_connection_closes() {
        let (_sender, receiver) = mpsc::channel(8);
        let (tier_tx, _tier_rx) = watch::channel(Arc::new(HashMap::new()));
        let mut actor = MetricsActor::new(receiver, tier_tx);
        actor.worker_authorized("miner".into(), "worker".into());
        actor.worker_authorized("miner".into(), "worker".into());
        actor.mark_worker_inactive(Some("miner".into()), "worker".into());
        assert!(actor.metrics.users["miner"].workers["worker"].active);
        actor.mark_worker_inactive(Some("miner".into()), "worker".into());
        assert!(!actor.metrics.users["miner"].workers["worker"].active);
        assert!(actor.worker_connections.is_empty());
        actor.mark_worker_inactive(Some("miner".into()), "worker".into());
        actor.worker_authorized("miner".into(), "worker".into());
        assert!(actor.metrics.users["miner"].workers["worker"].active);
    }

    #[test]
    fn tier_snapshot_prunes_per_miner_windows_older_than_the_submission_window() {
        let (_sender, receiver) = mpsc::channel(8);
        let (tier_tx, _tier_rx) = watch::channel(Arc::new(HashMap::new()));
        let mut actor = MetricsActor::new(receiver, tier_tx);
        let now = unix_seconds();
        actor.per_miner_windows.insert(
            "stale".to_string(),
            [WorkDatum {
                at: now.saturating_sub(SUBMISSION_WINDOW_SECS + 100),
                work: HASHES_PER_DIFFICULTY,
            }]
            .into(),
        );
        actor.record_share_accepted("fresh".to_string(), "worker".to_string(), 100, 100);
        let snapshot = actor.tier_snapshot(now + 10);
        assert!(!actor.per_miner_windows.contains_key("stale"));
        assert!(!actor.smoothed_tiers.contains_key("stale"));
        assert!(actor.per_miner_windows.contains_key("fresh"));
        assert_eq!(snapshot["fresh"], 1.0);
    }

    #[test]
    fn tier_snapshot_damps_a_step_with_the_smoothing_alpha() {
        let (_sender, receiver) = mpsc::channel(8);
        let (tier_tx, _tier_rx) = watch::channel(Arc::new(HashMap::new()));
        let mut actor = MetricsActor::new(receiver, tier_tx);
        let now = unix_seconds();
        let at = now.saturating_sub(10);
        let first = WorkDatum {
            at,
            work: HASHES_PER_DIFFICULTY,
        };
        actor
            .per_miner_windows
            .insert("a".to_string(), [first].into());
        actor
            .per_miner_windows
            .insert("b".to_string(), [first].into());
        actor.metrics.submission_window.extend([first, first]);
        let first_snapshot = actor.tier_snapshot(now);
        assert!((first_snapshot["a"] - 0.5).abs() < 1e-9);

        let step = WorkDatum {
            at,
            work: 3.0 * HASHES_PER_DIFFICULTY,
        };
        actor
            .per_miner_windows
            .get_mut("a")
            .unwrap()
            .push_back(step);
        actor.metrics.submission_window.push_back(step);
        let second_snapshot = actor.tier_snapshot(now);
        // a is now 4 of 5 work units: 0.8 raw, damped to
        // 0.5 + 0.2 * (0.8 - 0.5) = 0.56.
        assert!((second_snapshot["a"] - 0.56).abs() < 1e-9);
    }

    #[test]
    fn tier_snapshot_is_empty_when_the_pool_has_no_work() {
        let (_sender, receiver) = mpsc::channel(8);
        let (tier_tx, _tier_rx) = watch::channel(Arc::new(HashMap::new()));
        let mut actor = MetricsActor::new(receiver, tier_tx);
        let now = unix_seconds();
        actor.per_miner_windows.insert(
            "miner".to_string(),
            [WorkDatum {
                at: now.saturating_sub(10),
                work: HASHES_PER_DIFFICULTY,
            }]
            .into(),
        );
        let snapshot = actor.tier_snapshot(now);
        assert!(snapshot.is_empty());
        assert!(actor.per_miner_windows.contains_key("miner"));
    }

    #[test]
    fn tier_snapshot_is_empty_when_pool_hashrate_is_zero() {
        let (_sender, receiver) = mpsc::channel(8);
        let (tier_tx, _tier_rx) = watch::channel(Arc::new(HashMap::new()));
        let mut actor = MetricsActor::new(receiver, tier_tx);
        let now = unix_seconds();
        actor.metrics.submission_window.push_back(WorkDatum {
            at: now.saturating_sub(10),
            work: 0.0,
        });
        actor.per_miner_windows.insert(
            "miner".to_string(),
            [WorkDatum {
                at: now.saturating_sub(10),
                work: HASHES_PER_DIFFICULTY,
            }]
            .into(),
        );
        assert!(actor.tier_snapshot(now).is_empty());
    }

    #[test]
    fn tier_snapshot_publishes_a_lone_miner_as_the_whole_pool() {
        let (_sender, receiver) = mpsc::channel(8);
        let (tier_tx, _tier_rx) = watch::channel(Arc::new(HashMap::new()));
        let mut actor = MetricsActor::new(receiver, tier_tx);
        actor.record_share_accepted("miner".to_string(), "worker".to_string(), 100, 100);
        // A later `now` gives both windows the same non-zero span; the lone
        // miner is the whole pool.
        let snapshot = actor.tier_snapshot(unix_seconds() + 10);
        assert_eq!(snapshot["miner"], 1.0);
    }

    #[test]
    fn test_pool_metrics_default() {
        let metrics = PoolMetrics::default();
        assert!(metrics.lastupdate.is_none());
        assert!(metrics.best_share == 0);
    }

    #[tokio::test]
    async fn test_record_share_accepted() {
        let log_dir = tempfile::tempdir().unwrap();
        let handle = start_metrics(log_dir.path().to_str().unwrap().to_string())
            .await
            .unwrap();
        let _ = handle
            .record_share_accepted(
                SimplePplnsShare {
                    user_id: 1,
                    difficulty: 100,
                    btcaddress: Some("user1".to_string()),
                    workername: Some("worker1".to_string()),
                    n_time: 1000,
                    job_id: "test_job".to_string(),
                    extranonce2: "test_extra".to_string(),
                    nonce: "test_nonce".to_string(),
                },
                110,
            )
            .await;

        let metrics = handle.get_metrics().await;
        assert!(metrics.lastupdate.is_some());
        assert_eq!(metrics.best_share, 110);

        // Test that highest difficulty is updated correctly
        let _ = handle
            .record_share_accepted(
                SimplePplnsShare {
                    user_id: 1,
                    difficulty: 50,
                    btcaddress: Some("user1".to_string()),
                    workername: Some("worker1".to_string()),
                    n_time: 1000,
                    job_id: "test_job".to_string(),
                    extranonce2: "test_extra".to_string(),
                    nonce: "test_nonce".to_string(),
                },
                55,
            )
            .await;
        let metrics = handle.get_metrics().await;
        assert_eq!(metrics.best_share, 110);

        let _ = handle
            .record_share_accepted(
                SimplePplnsShare {
                    user_id: 1,
                    difficulty: 200,
                    btcaddress: Some("user1".to_string()),
                    workername: Some("worker1".to_string()),
                    n_time: 1000,
                    job_id: "test_job".to_string(),
                    extranonce2: "test_extra".to_string(),
                    nonce: "test_nonce".to_string(),
                },
                220,
            )
            .await;
        let metrics = handle.get_metrics().await;
        assert_eq!(metrics.best_share, 220);
    }

    #[tokio::test]
    async fn test_record_share_rejected() {
        let log_dir = tempfile::tempdir().unwrap();
        let handle = start_metrics(log_dir.path().to_str().unwrap().to_string())
            .await
            .unwrap();
        let _ = handle.record_share_rejected().await;

        let _ = handle.record_share_rejected().await;
    }

    #[tokio::test]
    async fn test_confirmed_share_and_block_effort_reset() {
        let log_dir = tempfile::tempdir().unwrap();
        let handle = start_metrics(log_dir.path().to_str().unwrap().to_string())
            .await
            .unwrap();

        // Confirmed sharechain work accumulates into the effort numerator.
        let _ = handle.record_confirmed_share(500.0).await;
        let _ = handle.record_confirmed_share(250.0).await;

        let metrics = handle.get_metrics().await;
        assert_eq!(metrics.work_since_last_block, 750.0);

        let _ = handle.reset_block_effort().await;

        let metrics = handle.get_metrics().await;
        // Finding a block resets effort toward the next block.
        assert_eq!(metrics.work_since_last_block, 0.0);
    }

    #[tokio::test]
    async fn test_metrics_commit() {
        let log_dir = tempfile::tempdir().unwrap();
        let handle = start_metrics(log_dir.path().to_str().unwrap().to_string())
            .await
            .unwrap();

        let _ = handle
            .increment_worker_count("user1".to_string(), "worker1".to_string())
            .await;

        let _ = handle
            .record_share_accepted(
                SimplePplnsShare {
                    user_id: 1,
                    difficulty: 1000,
                    btcaddress: Some("user1".to_string()),
                    workername: Some("worker1".to_string()),
                    n_time: 1000,
                    job_id: "test_job".to_string(),
                    extranonce2: "test_extra".to_string(),
                    nonce: "test_nonce".to_string(),
                },
                1100,
            )
            .await;
        let _ = handle
            .record_share_accepted(
                SimplePplnsShare {
                    user_id: 1,
                    difficulty: 2000,
                    btcaddress: Some("user1".to_string()),
                    workername: Some("worker1".to_string()),
                    n_time: 1000,
                    job_id: "test_job".to_string(),
                    extranonce2: "test_extra".to_string(),
                    nonce: "test_nonce".to_string(),
                },
                2200,
            )
            .await;
        let _ = handle.record_share_rejected().await;

        let current_unix_timestamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = handle.set_last_update(current_unix_timestamp - 60).await;

        let json_str = handle.commit().await;
        let json: serde_json::Value = serde_json::from_str(&json_str.unwrap()).unwrap();

        // Check that lastupdate exists and is a recent timestamp
        assert!(json["lastupdate"].as_u64().is_some());

        let metrics = handle.get_metrics().await;
        // After commit, the metrics should be reset
        assert_eq!(metrics.best_share, 2200);
    }

    #[tokio::test]
    async fn test_get_metrics_consistency() {
        let log_dir = tempfile::tempdir().unwrap();
        let handle = start_metrics(log_dir.path().to_str().unwrap().to_string())
            .await
            .unwrap();

        // Create user1/worker1 before recording shares (as would happen in real flow)
        let _ = handle
            .increment_worker_count("user1".to_string(), "worker1".to_string())
            .await;
        let _ = handle
            .record_share_accepted(
                SimplePplnsShare {
                    user_id: 1,
                    difficulty: 123,
                    btcaddress: Some("user1".to_string()),
                    workername: Some("worker1".to_string()),
                    n_time: 1000,
                    job_id: "test_job".to_string(),
                    extranonce2: "test_extra".to_string(),
                    nonce: "test_nonce".to_string(),
                },
                134,
            )
            .await;
        let _ = handle.record_share_rejected().await;
        // Create an inactive worker (no shares submitted)
        let _ = handle
            .increment_worker_count("user4".to_string(), "workerD".to_string())
            .await;

        let metrics = handle.get_metrics().await;
        // Inactive worker exists in memory
        assert!(metrics.users.contains_key("user4"));
        assert!(
            metrics
                .users
                .get("user4")
                .unwrap()
                .workers
                .contains_key("workerD")
        );
        assert_eq!(metrics.accepted_total, 1);
        assert_eq!(metrics.accepted_difficulty_total, 123);
        assert_eq!(metrics.rejected_total, 1);

        // save and reload metrics to verify persistence
        let _ = save_pool_local_stats(&metrics, log_dir.path().to_str().unwrap());
        let reloaded = PoolMetrics::load_existing(log_dir.path().to_str().unwrap()).unwrap();
        assert_eq!(
            reloaded.accepted_difficulty_total,
            metrics.accepted_difficulty_total
        );
        assert_eq!(reloaded.accepted_total, metrics.accepted_total);
        assert_eq!(reloaded.rejected_total, metrics.rejected_total);
        // Users that never submitted a share are filtered out when saving to JSON
        assert!(!reloaded.users.contains_key("user4"));
        // Active user1 with worker1 should be present
        assert!(reloaded.users.contains_key("user1"));
        assert!(
            reloaded
                .users
                .get("user1")
                .unwrap()
                .workers
                .contains_key("worker1")
        );
    }

    #[tokio::test]
    async fn test_record_share_updates_user_stats() {
        let log_dir = tempfile::tempdir().unwrap();
        let handle = start_metrics(log_dir.path().to_str().unwrap().to_string())
            .await
            .unwrap();

        let btcaddress = "userY".to_string();
        let workername = "workerY".to_string();

        let _ = handle
            .increment_worker_count(btcaddress.clone(), workername.clone())
            .await;

        let _ = handle
            .record_share_accepted(
                SimplePplnsShare {
                    user_id: 1,
                    difficulty: 77,
                    btcaddress: Some(btcaddress.clone()),
                    workername: Some(workername.clone()),
                    n_time: 3000,
                    job_id: "test_job".to_string(),
                    extranonce2: "test_extra".to_string(),
                    nonce: "test_nonce".to_string(),
                },
                84,
            )
            .await;

        let metrics = handle.get_metrics().await;
        let user = metrics.users.get(&btcaddress).unwrap();
        let worker = user.workers.get(&workername).unwrap();
        // Check that worker exists and is active
        assert!(worker.active);
        // Check that user stats are updated
        assert_eq!(user.shares_valid_total, 77);
        assert_eq!(user.best_share, 84);
        assert!(user.last_share_at > 0);
    }

    #[tokio::test]
    async fn test_record_share_multiple_users_and_workers() {
        let log_dir = tempfile::tempdir().unwrap();
        let handle = start_metrics(log_dir.path().to_str().unwrap().to_string())
            .await
            .unwrap();

        let _ = handle
            .increment_worker_count("userA".to_string(), "workerA1".to_string())
            .await;
        let _ = handle
            .increment_worker_count("userA".to_string(), "workerA2".to_string())
            .await;
        let _ = handle
            .increment_worker_count("userB".to_string(), "workerB1".to_string())
            .await;

        let _ = handle
            .record_share_accepted(
                SimplePplnsShare {
                    user_id: 1,
                    difficulty: 10,
                    btcaddress: Some("userA".to_string()),
                    workername: Some("workerA1".to_string()),
                    n_time: 4000,
                    job_id: "test_job".to_string(),
                    extranonce2: "test_extra".to_string(),
                    nonce: "test_nonce".to_string(),
                },
                11,
            )
            .await;
        let _ = handle
            .record_share_accepted(
                SimplePplnsShare {
                    user_id: 1,
                    difficulty: 20,
                    btcaddress: Some("userA".to_string()),
                    workername: Some("workerA2".to_string()),
                    n_time: 5000,
                    job_id: "test_job".to_string(),
                    extranonce2: "test_extra".to_string(),
                    nonce: "test_nonce".to_string(),
                },
                22,
            )
            .await;
        let _ = handle
            .record_share_accepted(
                SimplePplnsShare {
                    user_id: 1,
                    difficulty: 30,
                    btcaddress: Some("userB".to_string()),
                    workername: Some("workerB1".to_string()),
                    n_time: 6000,
                    job_id: "test_job".to_string(),
                    extranonce2: "test_extra".to_string(),
                    nonce: "test_nonce".to_string(),
                },
                33,
            )
            .await;

        let metrics = handle.get_metrics().await;

        let user_a = metrics.users.get("userA").unwrap();
        assert_eq!(user_a.shares_valid_total, 30);
        assert_eq!(user_a.best_share, 22);
        assert!(user_a.workers.contains_key("workerA1"));
        assert!(user_a.workers.contains_key("workerA2"));

        let user_b = metrics.users.get("userB").unwrap();
        assert_eq!(user_b.shares_valid_total, 30);
        assert_eq!(user_b.best_share, 33);
        assert!(user_b.workers.contains_key("workerB1"));
    }

    #[tokio::test]
    async fn test_worker_reauthorize_preserves_stats() {
        let log_dir = tempfile::tempdir().unwrap();
        let handle = start_metrics(log_dir.path().to_str().unwrap().to_string())
            .await
            .unwrap();

        let _ = handle
            .increment_worker_count("miner1".to_string(), "rig1".to_string())
            .await;

        let _ = handle
            .record_share_accepted(
                SimplePplnsShare {
                    user_id: 1,
                    difficulty: 1000,
                    btcaddress: Some("miner1".to_string()),
                    workername: Some("rig1".to_string()),
                    n_time: 1000,
                    job_id: "job1".to_string(),
                    extranonce2: "extra1".to_string(),
                    nonce: "nonce1".to_string(),
                },
                5500,
            )
            .await;

        let _ = handle
            .record_share_accepted(
                SimplePplnsShare {
                    user_id: 1,
                    difficulty: 1000,
                    btcaddress: Some("miner1".to_string()),
                    workername: Some("rig1".to_string()),
                    n_time: 1001,
                    job_id: "job2".to_string(),
                    extranonce2: "extra2".to_string(),
                    nonce: "nonce2".to_string(),
                },
                3200,
            )
            .await;

        let metrics = handle.get_metrics().await;
        let worker = metrics
            .users
            .get("miner1")
            .unwrap()
            .workers
            .get("rig1")
            .unwrap();
        assert_eq!(worker.shares_valid_total, 2000);
        assert_eq!(worker.best_share, 5500);
        assert_eq!(worker.best_share_ever, 5500);

        // Worker disconnects
        let _ = handle
            .decrement_worker_count(Some("miner1".to_string()), "rig1".to_string())
            .await;

        // Worker reconnects - re-authorizes with same name
        let _ = handle
            .increment_worker_count("miner1".to_string(), "rig1".to_string())
            .await;

        // Stats should be preserved, best_share reset for new session
        let metrics = handle.get_metrics().await;
        let user = metrics.users.get("miner1").unwrap();
        let worker = user.workers.get("rig1").unwrap();
        assert_eq!(worker.shares_valid_total, 2000);
        assert_eq!(worker.best_share, 0);
        assert_eq!(worker.best_share_ever, 5500);
        assert!(worker.active);

        // New shares accumulate on top of existing stats
        let _ = handle
            .record_share_accepted(
                SimplePplnsShare {
                    user_id: 1,
                    difficulty: 1000,
                    btcaddress: Some("miner1".to_string()),
                    workername: Some("rig1".to_string()),
                    n_time: 2000,
                    job_id: "job3".to_string(),
                    extranonce2: "extra3".to_string(),
                    nonce: "nonce3".to_string(),
                },
                4100,
            )
            .await;

        let metrics = handle.get_metrics().await;
        let user = metrics.users.get("miner1").unwrap();
        let worker = user.workers.get("rig1").unwrap();
        assert_eq!(worker.shares_valid_total, 3000);
        assert_eq!(user.shares_valid_total, 3000);
        assert_eq!(worker.best_share, 4100);
        assert_eq!(worker.best_share_ever, 5500);
    }
}
