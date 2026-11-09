// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::accounting::stats::pool_local_stats::load_pool_local_stats;
use crate::accounting::stats::user::User;
use crate::accounting::{payout::simple_pplns::SimplePplnsShare, stats::pool_local_stats};
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
use crate::store::writer::StoreError;
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
/// Confirmed share headers summed per store read when the block effort
/// numerator is recomputed from the chain at startup, bounding the walk's
/// peak memory on a long share chain.
const EFFORT_SEED_WALK_CHUNK: u32 = 4096;

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
    /// Current pool share difficulty in the share chain's units
    /// (`difficulty_float` of the newest prepared notify's target). The
    /// metrics actor never writes it: `/metrics` fills it from
    /// `JobTracker::get_pool_difficulty` at scrape time, so it is 0.0
    /// until the first notify is built. Runtime-only (not persisted).
    #[serde(default)]
    pub pool_difficulty: f64,
    /// Confirmed sharechain pool difficulty since the last bitcoin block was
    /// found; reset to zero on each block find. Numerator of the block effort
    /// metric (`work_since_last_block / network_difficulty`). Seeded at
    /// startup from the store by [`work_since_last_block_from_chain`] so work
    /// already done survives a restart, then incremented live per confirmed
    /// share. It is not written anywhere itself: the confirmed chain is the
    /// source of truth. Mainnet-relative units matching network_difficulty.
    /// Pool hashrate is derived separately from confirmed-chain total work.
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
            pool_difficulty: 0.0,
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

/// Hashrate of `window` from its work in the trailing
/// [`SUBMISSION_WINDOW_SECS`] seconds, in hashes per second: the work over
/// the span from the first kept share to `now`, which is the full ten minutes
/// once the window is that old. `None` with no work in the window, or no time
/// between the first kept share and `now` (the dash, not zero). Based on Cash
/// Dragon's alpha-vardiff-v2, which shares it between the pool and each miner.
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

/// Each miner's hashrate, in hashes per second, by the payout address it
/// authorized with: what the job builders size heavier Bitcoin Cash shares
/// by (`stratum::work::band_policy`).
pub type MinerHashrates = Arc<HashMap<String, f64>>;

/// How often the actor publishes each miner's hashrate to the job builders.
/// The ten-minute window moves on every accepted submission; a slow tick
/// keeps the snapshot fresh enough for a job's target without rebuilding the
/// map on every submission.
pub const MINER_HASHRATE_PUBLISH_INTERVAL: Duration = Duration::from_secs(5);

/// The current unix time in seconds, 0 before the epoch.
fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
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
    /// Accepted submissions in the trailing [`SUBMISSION_WINDOW_SECS`]
    /// seconds per miner, by the payout address it authorized with. Runtime
    /// only, pruned on the same cutoff as the pool-wide window.
    miner_windows: HashMap<String, VecDeque<WorkDatum>>,
    /// Publishes each miner's hashrate to the job builders, which read it
    /// through [`MetricsHandle::miner_hashrate`].
    miner_hashrates_tx: watch::Sender<MinerHashrates>,
}

impl MetricsActor {
    /// Create a new metrics actor with default metrics
    pub fn new(receiver: mpsc::Receiver<MetricsMessage>) -> Self {
        Self::with_metrics(PoolMetrics::default(), receiver)
    }

    /// Create a metrics actor with metrics loaded from the given log directory
    pub fn with_existing_metrics(
        log_dir: &str,
        receiver: mpsc::Receiver<MetricsMessage>,
    ) -> Result<Self, std::io::Error> {
        let metrics = PoolMetrics::load_existing(log_dir)?;
        Ok(Self::with_metrics(metrics, receiver))
    }

    fn with_metrics(metrics: PoolMetrics, receiver: mpsc::Receiver<MetricsMessage>) -> Self {
        let (miner_hashrates_tx, _miner_hashrates_rx) =
            watch::channel(Arc::new(HashMap::with_capacity(INITIAL_USER_MAP_CAPACITY)));
        Self {
            metrics,
            receiver,
            worker_connections: HashMap::with_capacity(INITIAL_USER_MAP_CAPACITY),
            miner_windows: HashMap::with_capacity(INITIAL_USER_MAP_CAPACITY),
            miner_hashrates_tx,
        }
    }

    /// The miner hashrates this actor publishes once it runs, for its
    /// handle.
    fn subscribe_miner_hashrates(&self) -> watch::Receiver<MinerHashrates> {
        self.miner_hashrates_tx.subscribe()
    }

    /// Start the actor's message handling loop, publishing each miner's
    /// hashrate every [`MINER_HASHRATE_PUBLISH_INTERVAL`] beside it.
    pub async fn run(mut self) {
        let mut publish_interval = tokio::time::interval(MINER_HASHRATE_PUBLISH_INTERVAL);
        let mut running = true;
        while running {
            tokio::select! {
                message = self.receiver.recv() => match message {
                    Some(message) => self.handle_message(message).await,
                    None => running = false,
                },
                _ = publish_interval.tick() => self.publish_miner_hashrates(unix_seconds()),
            }
        }
    }

    /// Publish each miner's hashrate at `now`, dropping the miners whose
    /// window holds no submission any more.
    fn publish_miner_hashrates(&mut self, now: u64) {
        let cutoff = now.saturating_sub(SUBMISSION_WINDOW_SECS);
        self.miner_windows.retain(|_, window| {
            while window.front().is_some_and(|datum| datum.at < cutoff) {
                window.pop_front();
            }
            !window.is_empty()
        });
        let mut hashrates = HashMap::with_capacity(self.miner_windows.len());
        for (address, window) in &self.miner_windows {
            if let Some(hashrate) = window_hashrate(window, now) {
                hashrates.insert(address.clone(), hashrate);
            }
        }
        self.miner_hashrates_tx.send_replace(Arc::new(hashrates));
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
    fn record_share_accepted(
        &mut self,
        btcaddress: String,
        workername: String,
        difficulty: u64,
        truediff: u64,
    ) {
        let current_unix_timestamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
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
        let miner_window = self.miner_windows.entry(btcaddress.clone()).or_default();
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
    /// The actor's latest snapshot of each miner's hashrate, read
    /// synchronously by the job builders.
    miner_hashrates: watch::Receiver<MinerHashrates>,
}

impl MetricsHandle {
    /// A handle on the actor behind `sender`, reading the hashrates it
    /// publishes through `miner_hashrates`.
    fn new(
        sender: mpsc::Sender<MetricsMessage>,
        miner_hashrates: watch::Receiver<MinerHashrates>,
    ) -> Self {
        Self {
            sender,
            miner_hashrates,
        }
    }

    /// The hashrate, in hashes per second, of the miner that authorized with
    /// `btcaddress`, from its accepted submissions in the trailing ten
    /// minutes as last published; `None` before its first one. Reads the
    /// latest snapshot without waiting on the actor.
    pub fn miner_hashrate(&self, btcaddress: &str) -> Option<f64> {
        self.miner_hashrates.borrow().get(btcaddress).copied()
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
    let actor = MetricsActor::new(receiver);
    let miner_hashrates = actor.subscribe_miner_hashrates();
    tokio::spawn(async move {
        actor.run().await;
    });
    MetricsHandle::new(sender, miner_hashrates)
}

/// The block effort numerator recomputed from the store: the summed
/// difficulty of the confirmed sharechain shares above the last
/// BCHN-accepted found block.
///
/// The window is the confirmed chain strictly above an anchor share: the
/// accepted found block whose share sits at the greatest height, falling
/// back to the genesis share when no block has been accepted yet, so the
/// genesis share itself never counts and work counting toward an already
/// found block never carries into the next round. Uncles are excluded,
/// matching the live increment in the organise worker, which feeds only
/// each confirmed share's own header difficulty. This is p2poolv1's
/// approach: the share rows are the source of truth, and no counter is
/// written for the metric.
///
/// Height metadata missing for the genesis share or an accepted found
/// block is an error rather than a wider window: counting back past a
/// found block would overstate effort, and the caller decides whether to
/// fail or to start at zero.
pub fn work_since_last_block_from_chain(
    chain_store_handle: &ChainStoreHandle,
) -> Result<f64, StoreError> {
    let anchor_height = effort_anchor_height(chain_store_handle)?;
    let tip_height = chain_store_handle
        .get_tip_height()?
        .ok_or_else(|| StoreError::NotFound("confirmed chain tip height".to_string()))?;
    let mut work = 0.0;
    let mut window_start = anchor_height + 1;
    while window_start <= tip_height {
        let window_end = (window_start + EFFORT_SEED_WALK_CHUNK - 1).min(tip_height);
        let headers = chain_store_handle
            .store_handle()
            .store()
            .query_share_headers(window_start, window_end)?;
        work += headers
            .iter()
            .map(|header| bitcoin::Target::from_compact(header.bits).difficulty_float())
            .sum::<f64>();
        window_start = window_end + 1;
    }
    Ok(work)
}

/// The confirmed height of the share the effort window starts above: the
/// accepted found block's share with the greatest height, or the genesis
/// share when no block has been accepted yet.
fn effort_anchor_height(chain_store_handle: &ChainStoreHandle) -> Result<u32, StoreError> {
    let genesis_hash = chain_store_handle
        .get_genesis_blockhash()
        .ok_or_else(|| StoreError::NotFound("genesis share".to_string()))?;
    let mut anchor_height = share_confirmed_height(chain_store_handle, &genesis_hash)?;
    for found_block in chain_store_handle.get_found_blocks()? {
        let found_height =
            share_confirmed_height(chain_store_handle, &found_block.share_blockhash)?;
        anchor_height = anchor_height.max(found_height);
    }
    Ok(anchor_height)
}

/// The confirmed height of `share_hash` from its stored metadata.
fn share_confirmed_height(
    chain_store_handle: &ChainStoreHandle,
    share_hash: &bitcoin::BlockHash,
) -> Result<u32, StoreError> {
    chain_store_handle
        .get_block_metadata(share_hash)?
        .expected_height
        .ok_or_else(|| StoreError::NotFound(format!("share height for {share_hash}")))
}

/// Construct a new metrics actor with existing metrics and return its handle.
///
/// The block effort numerator starts at zero: a caller with no share chain
/// has no work to restore. The node starts the metrics with
/// [`start_metrics_with_effort_seed`] instead.
pub async fn start_metrics(log_dir: String) -> Result<MetricsHandle, std::io::Error> {
    start_metrics_with_effort_seed(log_dir, 0.0).await
}

/// Construct a new metrics actor with existing metrics and return its
/// handle, seeding the block effort numerator with `work_since_last_block`
/// (from [`work_since_last_block_from_chain`]) so work the pool already did
/// is not lost across a restart.
///
/// The seed is applied before the actor spawns, so no reader can observe
/// zero effort once the handle is returned.
pub async fn start_metrics_with_effort_seed(
    log_dir: String,
    work_since_last_block: f64,
) -> Result<MetricsHandle, std::io::Error> {
    let (sender, receiver) = mpsc::channel(METRICS_MESSAGE_BUFFER_SIZE);
    let mut actor = MetricsActor::with_existing_metrics(&log_dir, receiver)?;
    actor.metrics.work_since_last_block = work_since_last_block;
    let miner_hashrates = actor.subscribe_miner_hashrates();
    tokio::spawn(async move {
        actor.run().await;
    });
    let handle = MetricsHandle::new(sender, miner_hashrates);
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
    use crate::store::found_block::FoundBlock;
    use crate::test_utils::{
        TestShareBlockBuilder, build_confirmed_test_chain, confirm_test_block, genesis_for_tests,
        setup_test_chain_store_handle, test_funding_transaction,
    };
    use bitcoin::TxMerkleNode;
    use bitcoin::hashes::Hash;

    /// A metrics with `work` pushed at `at` on its submission window.
    fn window_of(at: u64, work: f64) -> PoolMetrics {
        let mut metrics = PoolMetrics::default();
        metrics.submission_window.push_back(WorkDatum { at, work });
        metrics
    }

    #[test]
    fn local_submission_hashrate_sums_the_trailing_ten_minutes_of_work() {
        let metrics = PoolMetrics {
            submission_window: [
                WorkDatum {
                    at: 1_000,
                    work: HASHES_PER_DIFFICULTY,
                },
                WorkDatum {
                    at: 1_010,
                    work: HASHES_PER_DIFFICULTY,
                },
            ]
            .into(),
            ..Default::default()
        };
        let rate = metrics.local_submission_hashrate(1_020).unwrap();
        assert!((rate - 2.0 * HASHES_PER_DIFFICULTY / 20.0).abs() < 1.0);
    }

    #[test]
    fn local_submission_hashrate_ignores_work_older_than_ten_minutes() {
        // The t=1_000 share falls outside the ten minutes ending at t=3_000;
        // only the t=2_500 share counts, over the span from t=2_500 to now.
        let metrics = PoolMetrics {
            submission_window: [
                WorkDatum {
                    at: 1_000,
                    work: HASHES_PER_DIFFICULTY,
                },
                WorkDatum {
                    at: 2_500,
                    work: HASHES_PER_DIFFICULTY,
                },
            ]
            .into(),
            ..Default::default()
        };
        let rate = metrics.local_submission_hashrate(3_000).unwrap();
        assert!((rate - HASHES_PER_DIFFICULTY / 500.0).abs() < 1.0);
    }

    #[test]
    fn local_submission_hashrate_is_none_with_no_shares() {
        assert_eq!(
            PoolMetrics::default().local_submission_hashrate(1_000),
            None
        );
    }

    #[test]
    fn local_submission_hashrate_is_none_when_the_span_is_zero() {
        let metrics = window_of(1_000, HASHES_PER_DIFFICULTY);
        assert_eq!(metrics.local_submission_hashrate(1_000), None);
    }

    #[test]
    fn local_submission_hashrate_divides_by_the_elapsed_time_before_ten_minutes() {
        // The window is younger than ten minutes: the span is from the
        // first share to now, not the full ten minutes.
        let metrics = window_of(1_000, 2.0 * HASHES_PER_DIFFICULTY);
        let rate = metrics.local_submission_hashrate(1_010).unwrap();
        assert!((rate - 2.0 * HASHES_PER_DIFFICULTY / 10.0).abs() < 1.0);
    }

    /// Each miner's hashrate is published from its own accepted
    /// submissions, under the address it authorized with.
    #[test]
    fn each_miner_hashrate_is_published_by_its_address() {
        let (_sender, receiver) = mpsc::channel(8);
        let mut actor = MetricsActor::new(receiver);
        let handle = MetricsHandle::new(mpsc::channel(1).0, actor.subscribe_miner_hashrates());
        actor.record_share_accepted("big".into(), "worker".into(), 10, 10);
        actor.record_share_accepted("small".into(), "worker".into(), 1, 1);
        let now = actor.metrics.submission_window[0].at + 10;

        actor.publish_miner_hashrates(now);

        let big_span = (now - actor.miner_windows["big"][0].at) as f64;
        let small_span = (now - actor.miner_windows["small"][0].at) as f64;
        let big = handle.miner_hashrate("big").unwrap();
        let small = handle.miner_hashrate("small").unwrap();
        assert!(
            (big - 10.0 * HASHES_PER_DIFFICULTY / big_span).abs() < 1.0,
            "{big}"
        );
        assert!(
            (small - HASHES_PER_DIFFICULTY / small_span).abs() < 1.0,
            "{small}"
        );
        assert_eq!(handle.miner_hashrate("absent"), None);
    }

    /// A miner whose submissions all left the ten-minute window is dropped.
    #[test]
    fn a_miner_without_recent_submissions_is_dropped() {
        let (_sender, receiver) = mpsc::channel(8);
        let mut actor = MetricsActor::new(receiver);
        let handle = MetricsHandle::new(mpsc::channel(1).0, actor.subscribe_miner_hashrates());
        actor.record_share_accepted("gone".into(), "worker".into(), 10, 10);
        let now = actor.metrics.submission_window[0].at + SUBMISSION_WINDOW_SECS + 1;

        actor.publish_miner_hashrates(now);

        assert_eq!(handle.miner_hashrate("gone"), None);
        assert!(actor.miner_windows.is_empty());
    }

    #[test]
    fn shared_worker_stays_active_until_the_last_connection_closes() {
        let (_sender, receiver) = mpsc::channel(8);
        let mut actor = MetricsActor::new(receiver);
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

    /// The seed given at start is the numerator the actor reports before
    /// any share arrives.
    #[tokio::test]
    async fn test_start_metrics_seeds_work_since_last_block() {
        let log_dir = tempfile::tempdir().unwrap();
        let handle =
            start_metrics_with_effort_seed(log_dir.path().to_str().unwrap().to_string(), 42.0)
                .await
                .unwrap();

        let metrics = handle.get_metrics().await;
        assert_eq!(metrics.work_since_last_block, 42.0);
    }

    /// The seed sums every confirmed share above genesis: the genesis share
    /// itself is never pool work.
    #[tokio::test]
    async fn test_effort_seed_sums_confirmed_shares_after_genesis() {
        let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
        let blocks = build_confirmed_test_chain(&chain_store_handle, 3).await;

        let expected: f64 = blocks
            .iter()
            .skip(1)
            .map(|block| bitcoin::Target::from_compact(block.header.bits).difficulty_float())
            .sum();
        let work = work_since_last_block_from_chain(&chain_store_handle).unwrap();

        assert_eq!(work, expected);
    }

    /// An accepted found block anchors the window: work before it counted
    /// toward the block it found and does not carry into the next round.
    #[tokio::test]
    async fn test_effort_seed_starts_after_accepted_found_block() {
        let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
        let blocks = build_confirmed_test_chain(&chain_store_handle, 4).await;
        let found_block = FoundBlock {
            bitcoin_height: 900_000,
            bitcoin_blockhash: bitcoin::BlockHash::from_byte_array([1u8; 32]),
            share_blockhash: blocks[2].block_hash(),
            bitcoin_time: 1_700_000_002,
            miner_bitcoin_address: "miner".to_string(),
        };
        let stored = chain_store_handle
            .store_handle()
            .store()
            .record_accepted_found_block(&found_block)
            .unwrap();
        assert!(stored);

        let expected: f64 = blocks
            .iter()
            .skip(3)
            .map(|block| bitcoin::Target::from_compact(block.header.bits).difficulty_float())
            .sum();
        let work = work_since_last_block_from_chain(&chain_store_handle).unwrap();

        assert_eq!(work, expected);
    }

    /// Only genesis is confirmed, so nothing lies above the anchor and the
    /// numerator is zero rather than the genesis share's own difficulty.
    #[tokio::test]
    async fn test_effort_seed_is_zero_with_only_the_genesis_share() {
        let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
        chain_store_handle
            .init_or_setup_genesis(genesis_for_tests())
            .await
            .unwrap();

        let work = work_since_last_block_from_chain(&chain_store_handle).unwrap();

        assert_eq!(work, 0.0);
    }

    /// An empty store has nothing to anchor to; the error keeps the caller
    /// from silently widening the window.
    #[tokio::test]
    async fn test_effort_seed_errors_without_a_genesis_share() {
        let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;

        let result = work_since_last_block_from_chain(&chain_store_handle);

        assert!(result.is_err());
    }

    /// Uncle work never enters the numerator: only each confirmed share's
    /// own difficulty counts, as the live increment counts it.
    #[tokio::test]
    async fn test_effort_seed_excludes_uncle_difficulty() {
        let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
        let blocks = build_confirmed_test_chain(&chain_store_handle, 2).await;
        let mut uncle = TestShareBlockBuilder::new()
            .prev_share_blockhash(blocks[1].block_hash().to_string())
            .time(1_700_000_050)
            .add_transaction(test_funding_transaction(50))
            .build();
        uncle.template_merkle_branches = vec![TxMerkleNode::from_byte_array([50u8; 32])];
        chain_store_handle
            .add_share_block(uncle.clone())
            .await
            .unwrap();
        chain_store_handle
            .organise_header(uncle.header.clone())
            .await
            .unwrap();
        chain_store_handle
            .mark_block_valid(uncle.block_hash())
            .await
            .unwrap();

        let mut extending_block = TestShareBlockBuilder::new()
            .prev_share_blockhash(blocks[2].block_hash().to_string())
            .time(1_700_000_003)
            .add_transaction(test_funding_transaction(3))
            .uncles(vec![uncle.block_hash()])
            .build();
        extending_block.template_merkle_branches = vec![TxMerkleNode::from_byte_array([3u8; 32])];
        confirm_test_block(&chain_store_handle, &extending_block).await;

        let mut expected: f64 = blocks
            .iter()
            .skip(1)
            .map(|block| bitcoin::Target::from_compact(block.header.bits).difficulty_float())
            .sum();
        expected += bitcoin::Target::from_compact(extending_block.header.bits).difficulty_float();
        let uncle_difficulty = bitcoin::Target::from_compact(uncle.header.bits).difficulty_float();
        assert!(uncle_difficulty > 0.0);
        let work = work_since_last_block_from_chain(&chain_store_handle).unwrap();

        assert_eq!(work, expected);
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
