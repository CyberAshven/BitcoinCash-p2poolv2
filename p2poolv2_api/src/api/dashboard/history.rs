// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Hashrate history the node keeps for its dashboard.
//!
//! A sampler task records the pool's, this node's and the network's hashrate
//! once a minute into a bounded ring, and queues the same sample to the
//! store's `HashrateSamples` column family so a restart reloads the
//! history instead of starting empty. The ring is bounded to the store's
//! 30-day retention, so the in-memory history cannot grow without limit.

use crate::api::server::AppState;
use bitcoin::{CompactTarget, Target};
use p2poolv2_lib::address_display::ShareInfoDisplay;
use p2poolv2_lib::store::dag_store::ShareInfo;
use p2poolv2_lib::store::hashrate_samples::{HashrateSample, RETENTION_SECS};
use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::time::MissedTickBehavior;
use tracing::warn;

/// How often the sampler records a sample.
pub(crate) const SAMPLE_INTERVAL: Duration = Duration::from_secs(60);

/// Samples kept: the store's 30-day retention at one a minute.
pub(crate) const MAX_SAMPLES: usize = (RETENTION_SECS / SAMPLE_INTERVAL.as_secs()) as usize;

/// Shares the pool hashrate is estimated from: the newest this many
/// confirmed shares.
pub(crate) const POOL_HASHRATE_SHARES: u32 = 120;

/// Shares whose timestamp sits at least this far before the newest share in
/// the window are left out of the hashrate span. A seed share (e.g. the
/// genesis share at a fixed past time) would otherwise stretch the span and
/// collapse the reported hashrate by orders of magnitude.
pub(crate) const MAX_SHARE_GAP_SECS: u32 = 3600;

/// Expected seconds between Bitcoin Cash blocks, for network hashrate.
const BLOCK_INTERVAL_SECONDS: f64 = 600.0;

/// Hashes a difficulty-1 share takes on average.
const HASHES_PER_DIFFICULTY: f64 = 4_294_967_296.0;

/// One sample of the hashrates the dashboard charts, in hashes per second.
/// `None` where the value could not be known at that time.
///
/// The same shape the store persists (`p2poolv2_lib`'s `HashrateSample`),
/// re-exported so the API never field-renames on the write-through path.
pub(crate) type Sample = HashrateSample;

#[derive(Default)]
struct Inner {
    samples: VecDeque<Sample>,
}

/// The dashboard's history. Cheap to share; every method takes the lock
/// briefly and never across an await.
#[derive(Default)]
pub(crate) struct HashrateHistory {
    inner: Mutex<Inner>,
}

impl HashrateHistory {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // A panic while holding this lock leaves plain data behind, still
        // fine to read.
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Samples, oldest first.
    pub(crate) fn samples(&self) -> Vec<Sample> {
        self.lock().samples.iter().copied().collect()
    }

    pub(crate) fn push_sample(&self, sample: Sample) {
        let mut inner = self.lock();
        if inner.samples.len() == MAX_SAMPLES {
            inner.samples.pop_front();
        }
        inner.samples.push_back(sample);
    }

    /// Replace the ring with `samples`, oldest first, as the server does
    /// once at startup after loading the store. Newest beyond the ring's
    /// bound are kept.
    pub(crate) fn load(&self, samples: Vec<Sample>) {
        let mut inner = self.lock();
        let kept: VecDeque<Sample> = if samples.len() > MAX_SAMPLES {
            samples[samples.len() - MAX_SAMPLES..]
                .iter()
                .copied()
                .collect()
        } else {
            samples.into_iter().collect()
        };
        inner.samples = kept;
    }
}

/// Work one share of `bits` stands for, in hashes.
pub(crate) fn work_from_bits(bits: CompactTarget) -> f64 {
    Target::from_compact(bits).difficulty_float() * HASHES_PER_DIFFICULTY
}

/// Hashrate behind a run of shares, from their bits and timestamps.
///
/// Shares older than [`MAX_SHARE_GAP_SECS`] before the newest are dropped so
/// a seed share (e.g. the genesis share at a fixed past time) cannot inflate
/// the span. The oldest share only opens the time span: its work was done
/// before it, outside the span. `None` with fewer than two shares or no time
/// between the oldest and the newest.
pub(crate) fn share_chain_hashrate(shares: &[(u32, CompactTarget)]) -> Option<f64> {
    let newest = shares.iter().max_by_key(|(time, _)| *time)?;
    let kept: Vec<&(u32, CompactTarget)> = shares
        .iter()
        .filter(|(time, _)| newest.0.saturating_sub(*time) <= MAX_SHARE_GAP_SECS)
        .collect();
    let oldest = kept.iter().min_by_key(|(time, _)| *time)?;
    let span = newest.0.checked_sub(oldest.0).filter(|span| *span > 0)?;
    let total: f64 = kept.iter().map(|(_, bits)| work_from_bits(*bits)).sum();
    Some((total - work_from_bits(oldest.1)) / f64::from(span))
}

/// Network hashrate implied by a block difficulty.
pub(crate) fn network_hashrate(difficulty: f64) -> f64 {
    difficulty * HASHES_PER_DIFFICULTY / BLOCK_INTERVAL_SECONDS
}

/// Whether a share's Bitcoin Cash header met its own block target.
pub(crate) fn is_block(header: &bitcoin::block::Header) -> bool {
    header.validate_pow(header.target()).is_ok()
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

/// The newest confirmed shares, up to `count`, newest first.
pub(crate) fn newest_shares(state: &AppState, count: u32) -> Vec<ShareInfo> {
    let Ok(Some(tip)) = state.chain_store_handle.get_tip_height() else {
        return Vec::new();
    };
    let from = tip.saturating_sub(count.saturating_sub(1));
    state
        .chain_store_handle
        .store_handle()
        .store()
        .query_shares(from, tip)
        .unwrap_or_default()
}

/// The pool hashrate from the newest confirmed shares.
pub(crate) fn pool_hashrate(state: &AppState) -> Option<f64> {
    let shares = newest_shares(state, POOL_HASHRATE_SHARES);
    let points: Vec<(u32, CompactTarget)> = shares
        .iter()
        .map(|share| (share.timestamp, share.bits))
        .collect();
    share_chain_hashrate(&points)
}

/// This node's hashrate: the work of shares in the newest confirmed window
/// submitted by miners connected to this node, over the same time span as
/// the pool hashrate.
///
/// `connected` holds the payout and share chain addresses of those miners,
/// as [`crate::api::dashboard::connected_miners`] spells them. Shares older
/// than [`MAX_SHARE_GAP_SECS`] before the newest share are dropped (as in
/// [`share_chain_hashrate`]) so both rates share one span. The oldest share
/// of the kept window only opens the span; its work is never counted, even
/// when it belongs to a connected miner. `None` with fewer than two shares
/// or no time between the oldest and the newest.
pub(crate) fn local_chain_hashrate(
    state: &AppState,
    shares: &[ShareInfo],
    connected: &HashSet<String>,
) -> Option<f64> {
    let newest = shares.iter().max_by_key(|share| share.timestamp)?;
    let kept: Vec<&ShareInfo> = shares
        .iter()
        .filter(|share| newest.timestamp.saturating_sub(share.timestamp) <= MAX_SHARE_GAP_SECS)
        .collect();
    let oldest = kept.iter().min_by_key(|share| share.timestamp)?;
    let span = newest
        .timestamp
        .checked_sub(oldest.timestamp)
        .filter(|span| *span > 0)?;
    let work: f64 = kept
        .iter()
        .filter(|share| {
            if std::ptr::eq(*share, oldest) {
                return false;
            }
            let display = ShareInfoDisplay::from_share_info(
                share,
                Some(state.app_config.network),
                state.app_config.cash_network,
            );
            connected.contains(&display.miner_bitcoin_address)
                || connected.contains(&display.miner_address)
        })
        .map(|share| work_from_bits(share.bits))
        .sum();
    Some(work / f64::from(span))
}

/// Record one sample.
async fn tick(state: &Arc<AppState>, history: &Arc<HashrateHistory>) {
    let time = unix_now();
    let network = state
        .tracker_handle
        .get_network_difficulty()
        .map(network_hashrate);

    let connected: HashSet<String> = crate::api::dashboard::connected_miners(state).await;

    // The store is read synchronously; keep that off the async workers.
    let blocking_state = Arc::clone(state);
    let blocking_connected = connected.clone();
    let (pool, local) = match tokio::task::spawn_blocking(move || {
        let shares = newest_shares(&blocking_state, POOL_HASHRATE_SHARES);
        let points: Vec<(u32, CompactTarget)> = shares
            .iter()
            .map(|share| (share.timestamp, share.bits))
            .collect();
        (
            share_chain_hashrate(&points),
            local_chain_hashrate(&blocking_state, &shares, &blocking_connected),
        )
    })
    .await
    {
        Ok(result) => result,
        Err(error) => {
            warn!("Dashboard sampler could not read the share chain: {error}");
            (None, None)
        }
    };

    let sample = Sample {
        time,
        pool,
        local,
        network,
    };
    history.push_sample(sample);
    // Queue the same sample to the store, off the hot path: the channel is
    // unbounded, so this never blocks the sampler's async worker.
    state
        .chain_store_handle
        .store_handle()
        .add_hashrate_sample(sample);
}

/// Evict store samples older than the retention window and reload the
/// store's samples into `history`, oldest first. The blocking half of
/// [`reload_persisted`], for callers already on a blocking thread
/// (the startup load in `server.rs`).
pub(crate) fn reload_persisted_blocking(state: &AppState, history: &HashrateHistory) {
    let store = state.chain_store_handle.store_handle().store();
    let oldest = unix_now().saturating_sub(RETENTION_SECS);
    if let Err(error) = store.evict_hashrate_samples_before(oldest) {
        warn!("hashrate sample eviction failed: {error}");
    }
    history.load(store.load_hashrate_samples(oldest));
}

/// Evict store samples older than the retention window and reload the
/// store's samples into `history`, oldest first.
///
/// Returns how many samples were loaded. The sampler calls it every
/// [`run_sampler`]'s eviction interval so a clock jump or a long downtime
/// leaves disk and ring in agreement.
pub(crate) async fn reload_persisted(state: Arc<AppState>, history: Arc<HashrateHistory>) -> usize {
    let store = Arc::clone(state.chain_store_handle.store_handle().store());
    let oldest = unix_now().saturating_sub(RETENTION_SECS);
    let loaded = tokio::task::spawn_blocking(move || {
        if let Err(error) = store.evict_hashrate_samples_before(oldest) {
            warn!("hashrate sample eviction failed: {error}");
        }
        store.load_hashrate_samples(oldest)
    })
    .await;
    match loaded {
        Ok(samples) => {
            let count = samples.len();
            history.load(samples);
            count
        }
        Err(error) => {
            warn!("Could not reload hashrate history from the store: {error}");
            0
        }
    }
}

/// Sample once a minute until the history is dropped with the API server.
///
/// Holds the history only weakly, so it never keeps it alive. Every
/// EVICTION_INTERVAL samples it drops store rows past the retention
/// window and reloads what remains, so disk and ring agree after a
/// clock jump or a long downtime.
pub(crate) async fn run_sampler(state: Arc<AppState>, history: Weak<HashrateHistory>) {
    const EVICTION_INTERVAL: u32 = 60;
    let mut interval = tokio::time::interval(SAMPLE_INTERVAL);
    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut since_eviction: u32 = EVICTION_INTERVAL;
    loop {
        interval.tick().await;
        let Some(history) = history.upgrade() else {
            return;
        };
        tick(&state, &history).await;
        since_eviction = since_eviction.saturating_add(1);
        if since_eviction >= EVICTION_INTERVAL {
            since_eviction = 0;
            reload_persisted(Arc::clone(&state), Arc::clone(&history)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::server::{AppConfig, AppState};
    use bitcoin::hashes::Hash;
    use p2poolv2_lib::accounting::stats::metrics;
    use p2poolv2_lib::address::share_owner::ShareOwner;
    use p2poolv2_lib::monitoring_events::create_monitoring_event_channel;
    use p2poolv2_lib::node::actor::NodeHandle;
    use p2poolv2_lib::stratum::work::tracker::start_tracker_actor;
    use p2poolv2_lib::test_utils::setup_test_chain_store_handle;
    use std::str::FromStr;

    /// Bits of a difficulty-1 target.
    fn difficulty_one() -> CompactTarget {
        CompactTarget::from_consensus(0x1d00ffff)
    }

    /// The miner payout address every [`share_at`] share carries, which
    /// [`ShareInfoDisplay`] spells for any network.
    const CONNECTED_PAYOUT: &str = "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4";

    /// A difficulty-1 share at `time` from [`CONNECTED_PAYOUT`].
    fn share_at(time: u32) -> ShareInfo {
        ShareInfo {
            blockhash: bitcoin::BlockHash::all_zeros(),
            prev_blockhash: bitcoin::BlockHash::all_zeros(),
            height: 1,
            miner_bitcoin_address: bitcoin::Address::from_str(CONNECTED_PAYOUT)
                .expect("valid address")
                .assume_checked(),
            miner_address: ShareOwner::CashP2pkh([0x11; 20]),
            timestamp: time,
            bits: difficulty_one(),
            uncles: Vec::new(),
        }
    }

    /// An AppState whose only meaningful field here is `app_config`.
    /// [`local_chain_hashrate`] never touches the rest of the struct.
    async fn state() -> AppState {
        let (chain_store_handle, _store_dir) = setup_test_chain_store_handle(false).await;
        let metrics_dir = tempfile::tempdir().expect("metrics tempdir");
        let metrics_handle =
            metrics::start_metrics(metrics_dir.path().to_str().expect("path").to_string())
                .await
                .expect("start metrics");
        AppState {
            app_config: AppConfig {
                pool_signature_length: 0,
                network: bitcoin::Network::Bitcoin,
                cash_network: None,
                cors_allowed: false,
            },
            chain_store_handle,
            metrics_handle,
            tracker_handle: start_tracker_actor(),
            node_handle: NodeHandle::new_for_test(),
            monitoring_event_sender: create_monitoring_event_channel().0,
            auth_user: None,
            auth_token: None,
            pplns_window: None,
        }
    }

    #[test]
    fn a_difficulty_one_share_is_two_to_the_thirty_two_hashes() {
        assert!((work_from_bits(difficulty_one()) - HASHES_PER_DIFFICULTY).abs() < 1.0);
    }

    #[test]
    fn share_chain_hashrate_leaves_out_the_oldest_shares_work() {
        // Three difficulty-1 shares over 20 seconds: two shares of work
        // after the first one.
        let shares = [
            (1_000, difficulty_one()),
            (1_010, difficulty_one()),
            (1_020, difficulty_one()),
        ];
        let rate = share_chain_hashrate(&shares).unwrap();
        assert!((rate - 2.0 * HASHES_PER_DIFFICULTY / 20.0).abs() < 1.0);
    }

    #[test]
    fn share_chain_hashrate_does_not_depend_on_order() {
        let ordered = [(1_000, difficulty_one()), (1_030, difficulty_one())];
        let reversed = [(1_030, difficulty_one()), (1_000, difficulty_one())];
        assert_eq!(
            share_chain_hashrate(&ordered),
            share_chain_hashrate(&reversed)
        );
    }

    #[test]
    fn share_chain_hashrate_needs_two_shares_and_a_span() {
        assert_eq!(share_chain_hashrate(&[]), None);
        assert_eq!(share_chain_hashrate(&[(1_000, difficulty_one())]), None);
        assert_eq!(
            share_chain_hashrate(&[(1_000, difficulty_one()), (1_000, difficulty_one())]),
            None
        );
    }

    #[test]
    fn share_chain_hashrate_drops_shares_far_older_than_the_newest() {
        // The seed share sits more than MAX_SHARE_GAP_SECS before the
        // newest and is dropped. Of the kept recent shares, the oldest
        // (1_000 + MAX + 1) only opens the span and its work is skipped;
        // the two newer shares contribute work over the 10 s span.
        let shares = [
            (1_000, difficulty_one()),
            (1_000 + MAX_SHARE_GAP_SECS + 1, difficulty_one()),
            (1_000 + MAX_SHARE_GAP_SECS + 6, difficulty_one()),
            (1_000 + MAX_SHARE_GAP_SECS + 11, difficulty_one()),
        ];
        let rate = share_chain_hashrate(&shares).unwrap();
        assert!((rate - 2.0 * HASHES_PER_DIFFICULTY / 10.0).abs() < 1.0);
    }

    #[test]
    fn network_hashrate_spreads_difficulty_over_ten_minutes() {
        assert!((network_hashrate(600.0) - HASHES_PER_DIFFICULTY).abs() < 1.0);
    }

    #[tokio::test]
    async fn local_chain_hashrate_sums_connected_work_over_pool_span() {
        let connected: HashSet<String> = [CONNECTED_PAYOUT.to_string()].into_iter().collect();
        // Three difficulty-1 shares over 20 seconds from one connected miner.
        let shares = [share_at(1_000), share_at(1_010), share_at(1_020)];
        let rate = local_chain_hashrate(&state().await, &shares, &connected).unwrap();
        assert!((rate - 2.0 * HASHES_PER_DIFFICULTY / 20.0).abs() < 1.0);
    }

    #[tokio::test]
    async fn local_chain_hashrate_is_zero_when_no_miner_is_connected() {
        let connected: HashSet<String> = HashSet::new();
        let shares = [share_at(1_000), share_at(1_010), share_at(1_020)];
        assert_eq!(
            local_chain_hashrate(&state().await, &shares, &connected),
            Some(0.0)
        );
    }

    #[tokio::test]
    async fn local_chain_hashrate_drops_shares_far_older_than_the_newest() {
        let connected: HashSet<String> = [CONNECTED_PAYOUT.to_string()].into_iter().collect();
        // The seed share at t=1_000 sits more than MAX_SHARE_GAP_SECS
        // before the newest and is dropped. Of the kept recent shares,
        // the oldest only opens the span; the two newer shares contribute
        // work over the 10 s span.
        let shares = [
            share_at(1_000),
            share_at(1_000 + MAX_SHARE_GAP_SECS + 1),
            share_at(1_000 + MAX_SHARE_GAP_SECS + 6),
            share_at(1_000 + MAX_SHARE_GAP_SECS + 11),
        ];
        let rate = local_chain_hashrate(&state().await, &shares, &connected).unwrap();
        assert!((rate - 2.0 * HASHES_PER_DIFFICULTY / 10.0).abs() < 1.0);
    }

    #[tokio::test]
    async fn local_chain_hashrate_needs_two_shares_and_a_span() {
        let connected: HashSet<String> = [CONNECTED_PAYOUT.to_string()].into_iter().collect();
        assert_eq!(local_chain_hashrate(&state().await, &[], &connected), None);
        assert_eq!(
            local_chain_hashrate(&state().await, &[share_at(1_000)], &connected),
            None
        );
        assert_eq!(
            local_chain_hashrate(
                &state().await,
                &[share_at(1_000), share_at(1_000)],
                &connected
            ),
            None
        );
    }

    #[tokio::test]
    async fn local_chain_hashrate_skips_the_oldest_shares_work() {
        // The oldest share is connected but its work is outside the span;
        // only the two newer shares count.
        let connected: HashSet<String> = [CONNECTED_PAYOUT.to_string()].into_iter().collect();
        let shares = [share_at(1_000), share_at(1_010), share_at(1_020)];
        let rate = local_chain_hashrate(&state().await, &shares, &connected).unwrap();
        assert!((rate - 2.0 * HASHES_PER_DIFFICULTY / 20.0).abs() < 1.0);
    }

    #[tokio::test]
    async fn local_chain_hashrate_does_not_depend_on_order() {
        let connected: HashSet<String> = [CONNECTED_PAYOUT.to_string()].into_iter().collect();
        let ordered = [share_at(1_000), share_at(1_030)];
        let reversed = [share_at(1_030), share_at(1_000)];
        assert_eq!(
            local_chain_hashrate(&state().await, &ordered, &connected),
            local_chain_hashrate(&state().await, &reversed, &connected)
        );
    }

    #[test]
    fn samples_are_capped_at_the_retention_window() {
        let history = HashrateHistory::default();
        for time in 0..(MAX_SAMPLES as u64 + 10) {
            history.push_sample(Sample {
                time,
                pool: None,
                local: None,
                network: None,
            });
        }
        let samples = history.samples();
        assert_eq!(samples.len(), MAX_SAMPLES);
        assert_eq!(samples.first().unwrap().time, 10);
    }

    #[test]
    fn load_replaces_the_ring_with_the_newest_bound_samples() {
        let history = HashrateHistory::default();
        let loaded: Vec<Sample> = (0..(MAX_SAMPLES as u64 + 5))
            .map(|time| Sample {
                time,
                pool: None,
                local: None,
                network: None,
            })
            .collect();
        history.load(loaded);
        let samples = history.samples();
        assert_eq!(samples.len(), MAX_SAMPLES);
        assert_eq!(samples.first().unwrap().time, 5);
    }

    #[test]
    fn a_header_meeting_its_own_target_is_a_block() {
        // Regtest's easiest target: almost any hash meets it.
        let mut header = bitcoin::constants::genesis_block(bitcoin::Network::Regtest).header;
        assert!(is_block(&header));
        // A target no hash can meet.
        header.bits = CompactTarget::from_consensus(0x0300_0001);
        assert!(!is_block(&header));
    }
}
