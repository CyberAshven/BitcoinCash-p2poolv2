// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::block_template::BlockTemplate;
use super::merge_tokens::MinerTokenJob;
use crate::accounting::payout::payout_tree::PayoutTree;
use crate::shares::share_block::ShareTransaction;
use crate::shares::share_commitment::ShareCommitment;
use crate::shares::weak::WeakShare;
use bitcoin::BlockHash;
use bitcoin::Target;
use bitcoin::pow::CompactTarget;
use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use tracing::debug;
pub mod parse_coinbase;

const MAX_JOB_AGE_SECS: u64 = 15 * 60; // 15 minutes
const EXPECTED_SHARES_PER_JOB: usize = 4; // new jobs are created every 10s. Target share period 3s.
/// First id a declared job gets ([`JobTracker::get_next_declared_job_id`]):
/// above every id this node's own jobs take, which start from the
/// nanoseconds since 1970 (below 2^63 until the year 2262).
const DECLARED_JOB_ID_BASE: u64 = 1 << 63;

/// The job id sent to miners.
/// A job id matches a block template.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct JobId(pub u64);

/// Delegate to u64's lower hex
impl std::fmt::LowerHex for JobId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::LowerHex::fmt(&self.0, f)
    }
}

/// Implement Add for JobId
impl std::ops::Add<u64> for JobId {
    type Output = Self;

    fn add(self, rhs: u64) -> Self::Output {
        Self(self.0 + rhs)
    }
}

/// Capture job details to be used when reconstructing the block from a submitted job.
#[derive(Debug, Clone)]
pub struct JobDetails {
    /// False for private Coinbase-only jobs: only the declarator can broadcast.
    pub block_available: bool,
    pub blocktemplate: Arc<BlockTemplate>,
    pub coinbase1: String,
    pub coinbase2: String,
    pub generation_timestamp: u64,
    pub share_commitment: Option<ShareCommitment>,
    /// Nanosecond timestamp embedded in the coinbase scriptSig for this job.
    pub coinbase_nsecs: u64,
    /// Merkle branches for the template transactions (excluding coinbase).
    pub template_merkle_branches: Vec<bitcoin::TxMerkleNode>,
    /// Share transactions after the share coinbase, in order. The share
    /// commitment's `share_witness_root` covers exactly these, so a share
    /// found on this job is built with them. Shared by every job on one
    /// notify; empty for declared jobs.
    pub share_transactions: Arc<[ShareTransaction]>,
    /// The miner's token leaves behind output 0, when the node merge-mines
    /// tokens for them: for their disclosure and the proofs of their wins.
    /// `None` for the empty commitment and for declared jobs, whose
    /// declarator holds its own leaves.
    pub merge_tokens: Option<Arc<MinerTokenJob>>,
    /// The exact payout tree committed by this job's coinbase, retained so
    /// a found block's funds can be recovered before share validation.
    pub payout_tree: Option<Arc<PayoutTree>>,
    /// The weak shares a share found on this job carries, which its
    /// commitment's weak root covers. Shared by every job on one notify;
    /// empty on Bitcoin and for declared jobs.
    pub weak_shares: Arc<[WeakShare]>,
    /// The weak target of a Bitcoin Cash job at the minimum
    /// (`shares::weak::parent_weak_target`): a submission meeting it but not
    /// the share target becomes a weak share. `None` for heavier jobs,
    /// declared jobs and on Bitcoin.
    pub weak_target: Option<Target>,
}

/// Lock-free job tracker using DashMap for concurrent access.
#[derive(Debug)]
pub struct JobTracker {
    job_details: DashMap<JobId, JobDetails>,
    /// Tracks submitted share hashes per job for duplicate detection (internal only).
    /// Vec is used instead because each job accumulates very few shares (~3),
    /// making linear scan faster than hash lookups with far less memory overhead.
    job_shares: DashMap<JobId, Vec<BlockHash>>,
    latest_job_id: AtomicU64,
    /// `bits` of the newest block template seen. Kept beside the jobs rather
    /// than read off the newest job id: jobs are inserted per miner notify
    /// and expire after [`MAX_JOB_AGE_SECS`], so a tracker without recent
    /// miners would otherwise lose the network difficulty even while
    /// templates keep arriving. Read on every `/hashrate` and `/metrics`
    /// request, hence a lock the readers share.
    latest_network_bits: RwLock<Option<CompactTarget>>,
    /// The share target the newest prepared notify asks miners to meet
    /// (`PoolDifficulty::calculate_target_clamped`, refreshed on every
    /// notify build). Kept like `latest_network_bits` rather than derived
    /// from jobs so the current pool difficulty survives job expiry and
    /// reads the same live feed. `None` until the first notify is built.
    pool_target: RwLock<Option<CompactTarget>>,
    /// Ids for jobs miners declare, apart from `latest_job_id`.
    declared_job_id: AtomicU64,
}

impl JobTracker {
    /// Create a new Tracker with timestamp-based initial job ID
    pub fn new() -> Self {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        Self {
            job_details: DashMap::new(),
            job_shares: DashMap::new(),
            latest_job_id: AtomicU64::new(timestamp),
            latest_network_bits: RwLock::new(None),
            pool_target: RwLock::new(None),
            declared_job_id: AtomicU64::new(DECLARED_JOB_ID_BASE),
        }
    }

    /// Insert a block template with the specified job id
    #[allow(clippy::too_many_arguments)] // wiring constructor: each parameter is a distinct collaborator, a params struct would only move the list
    pub fn insert_job(
        &self,
        block_template: Arc<BlockTemplate>,
        coinbase1: String,
        coinbase2: String,
        share_commitment: Option<ShareCommitment>,
        coinbase_nsecs: u64,
        template_merkle_branches: Vec<bitcoin::TxMerkleNode>,
        job_id: JobId,
    ) -> JobId {
        self.insert_job_details(
            job_id,
            JobDetails {
                block_available: true,
                blocktemplate: block_template,
                coinbase1,
                coinbase2,
                generation_timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
                share_commitment,
                coinbase_nsecs,
                template_merkle_branches,
                share_transactions: Arc::from([]),
                merge_tokens: None,
                payout_tree: None,
                weak_shares: Arc::from([]),
                weak_target: None,
            },
        );
        job_id
    }

    /// Install a fully validated job and initialize its duplicate tracking.
    pub(crate) fn insert_job_details(&self, job_id: JobId, details: JobDetails) {
        self.job_details.insert(job_id, details);
        self.job_shares
            .insert(job_id, Vec::with_capacity(EXPECTED_SHARES_PER_JOB));
    }

    /// Get the next job id, incrementing it atomically
    pub fn get_next_job_id(&self) -> JobId {
        JobId(self.latest_job_id.fetch_add(1, Ordering::SeqCst) + 1)
    }

    /// Get the latest job id without incrementing
    pub fn get_latest_job_id(&self) -> JobId {
        JobId(self.latest_job_id.load(Ordering::SeqCst))
    }

    /// The next id for a job a miner declared (an SV2 custom job). Declared
    /// ids come from their own range, so a declared job never becomes
    /// [`Self::get_latest_job_id`]: the latest job stays the latest one this
    /// node built, which its coinbase metrics, network difficulty and
    /// `/jd_prepare` read with this node's coinbase layout.
    pub fn get_next_declared_job_id(&self) -> JobId {
        JobId(self.declared_job_id.fetch_add(1, Ordering::SeqCst))
    }

    /// Get job details by job id
    pub fn get_job(&self, job_id: JobId) -> Option<JobDetails> {
        self.job_details.get(&job_id).map(|r| r.clone())
    }

    /// Record the newest block template's `bits`.
    ///
    /// Called for every template this node prepares, so the network
    /// difficulty tracks the template feed even when no miner is connected
    /// to receive a job and the job map has been cleaned out. Inserting a
    /// job does not call it: a job a miner declares over SV2 carries the
    /// miner's `bits`, which can be a block behind the node's newest
    /// template, and must not replace them.
    pub fn set_network_bits(&self, bits: CompactTarget) {
        let mut slot = self
            .latest_network_bits
            .write()
            .expect("network bits lock poisoned");
        *slot = Some(bits);
    }

    /// Record the share target of the newest prepared notify.
    ///
    /// Called once per notify build with the ASERT-clamped target miners
    /// must meet, so the current pool difficulty is readable while no
    /// miner is connected and after the job map has been cleaned out.
    pub fn set_pool_target(&self, target: CompactTarget) {
        let mut slot = self.pool_target.write().expect("pool target lock poisoned");
        *slot = Some(target);
    }

    /// Current pool share difficulty in the share chain's units, from the
    /// newest prepared notify's target ([`Self::set_pool_target`]). Returns
    /// None until the first notify is built. Uses `difficulty_float`, the
    /// same mainnet-relative conversion as
    /// [`Self::get_network_difficulty`], so both gauges share units.
    pub fn get_pool_difficulty(&self) -> Option<f64> {
        let target_bits = *self.pool_target.read().expect("pool target lock poisoned");
        let target = bitcoin::Target::from_compact(target_bits?);
        Some(target.difficulty_float())
    }

    /// Current bitcoin network difficulty derived from the newest block
    /// template's `bits` ([`Self::set_network_bits`], refreshed on every
    /// prepared template). Returns None until the first
    /// template arrives. Uses the mainnet-relative difficulty
    /// (`difficulty_float`) so it shares units with the pool's truediffone
    /// share difficulties. This lets Grafana divide work by this value.
    ///
    /// Independent of the job map: jobs are per-miner and expire after
    /// [`MAX_JOB_AGE_SECS`], while the template feed is what sets the
    /// network difficulty.
    pub fn get_network_difficulty(&self) -> Option<f64> {
        let bits = *self
            .latest_network_bits
            .read()
            .expect("network bits lock poisoned");
        let target = bitcoin::Target::from_compact(bits?);
        Some(target.difficulty_float())
    }

    /// Remove job details that are older than the specified duration in seconds
    /// Returns the number of jobs that were removed
    pub fn cleanup_old_jobs(&self, max_age_secs: u64) -> usize {
        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let before_count = self.job_details.len();

        self.job_details.retain(|job_id, details| {
            let keep = current_time.saturating_sub(details.generation_timestamp) < max_age_secs;
            if !keep {
                // Also remove shares for this job
                self.job_shares.remove(job_id);
            }
            keep
        });

        before_count - self.job_details.len()
    }

    /// Add a share to shares tracker for duplicate detection.
    /// Returns true if share is newly inserted, false if job not found or share already exists.
    pub fn add_share(&self, job_id: JobId, blockhash: BlockHash) -> bool {
        let Some(mut shares) = self.job_shares.get_mut(&job_id) else {
            return false;
        };
        if !shares.contains(&blockhash) {
            shares.push(blockhash);
            true
        } else {
            false
        }
    }
}

impl Default for JobTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Start a new JobTracker and return an Arc to it.
///
/// Also spawns a background cleanup task that periodically removes old jobs.
pub fn start_tracker_actor() -> Arc<JobTracker> {
    let tracker = Arc::new(JobTracker::new());

    // Spawn a task for periodic cleanup
    let cleanup_tracker = tracker.clone();
    tokio::spawn(async move {
        let cleanup_interval = tokio::time::Duration::from_secs(MAX_JOB_AGE_SECS);
        loop {
            tokio::time::sleep(cleanup_interval).await;
            let count = cleanup_tracker.cleanup_old_jobs(MAX_JOB_AGE_SECS);
            if count > 0 {
                debug!("Cleaned up {} old job IDs", count);
            }
        }
    });

    tracker
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{TEST_COINBASE_NSECS, create_test_commitment};

    #[test]
    fn test_declared_job_ids_do_not_move_the_latest_job() {
        let tracker = JobTracker::new();
        let own = tracker.get_next_job_id();

        let declared = tracker.get_next_declared_job_id();
        let declared2 = tracker.get_next_declared_job_id();

        assert_eq!(tracker.get_latest_job_id(), own);
        assert_eq!(declared.0, DECLARED_JOB_ID_BASE);
        assert_eq!(declared2.0, DECLARED_JOB_ID_BASE + 1);
        assert!(own.0 < DECLARED_JOB_ID_BASE);
    }

    #[test]
    fn test_job_id_generation() {
        let tracker = JobTracker::new();
        let initial_job_id = tracker.get_latest_job_id();

        // Get next job id should increment
        let next_job_id = tracker.get_next_job_id();
        assert_eq!(next_job_id.0, initial_job_id.0 + 1);

        // Latest job id should reflect the increment
        let latest_job_id = tracker.get_latest_job_id();
        assert_eq!(latest_job_id.0, next_job_id.0);

        // Multiple calls should continue incrementing
        let next_job_id2 = tracker.get_next_job_id();
        assert_eq!(next_job_id2.0, next_job_id.0 + 1);
    }

    #[tokio::test]
    async fn test_job_id_generation_tracker() {
        let tracker = start_tracker_actor();

        // Get the initial latest job id
        let initial_job_id = tracker.get_latest_job_id();

        // Get next job id should increment
        let next_job_id = tracker.get_next_job_id();
        assert_eq!(next_job_id.0, initial_job_id.0 + 1);

        // Latest job id should reflect the increment
        let latest_job_id = tracker.get_latest_job_id();
        assert_eq!(latest_job_id.0, next_job_id.0);

        // Multiple calls should continue incrementing
        let next_job_id2 = tracker.get_next_job_id();
        assert_eq!(next_job_id2.0, next_job_id.0 + 1);
    }

    #[tokio::test]
    async fn test_get_network_difficulty() {
        let tracker = start_tracker_actor();

        // No template prepared yet -> no difficulty available
        assert!(tracker.get_network_difficulty().is_none());

        let template_str = include_str!(
            "../../../../../p2poolv2_tests/test_data/gbt/signet/gbt-no-transactions.json"
        );
        let template: BlockTemplate = serde_json::from_str(template_str).unwrap();
        let bits = bitcoin::pow::CompactTarget::from_unprefixed_hex(&template.bits).unwrap();
        let expected = bitcoin::Target::from_compact(bits).difficulty_float();

        tracker.set_network_bits(bits);

        let difficulty = tracker.get_network_difficulty().unwrap();
        assert_eq!(difficulty, expected);
        assert!(difficulty > 0.0);
    }

    #[tokio::test]
    async fn a_declared_job_does_not_move_the_network_difficulty() {
        let tracker = start_tracker_actor();

        let template_str = include_str!(
            "../../../../../p2poolv2_tests/test_data/gbt/signet/gbt-no-transactions.json"
        );
        let template: BlockTemplate = serde_json::from_str(template_str).unwrap();
        let own_bits = bitcoin::pow::CompactTarget::from_unprefixed_hex(&template.bits).unwrap();
        tracker.set_network_bits(own_bits);
        let expected = bitcoin::Target::from_compact(own_bits).difficulty_float();

        // A miner declares a job on the block before, with other bits.
        let mut declared_template = template.clone();
        declared_template.bits = format!("{:08x}", own_bits.to_consensus() + 1);
        let declared_id = tracker.get_next_declared_job_id();
        tracker.insert_job(
            Arc::new(declared_template),
            "cb1".to_string(),
            "cb2".to_string(),
            Some(create_test_commitment()),
            TEST_COINBASE_NSECS,
            vec![],
            declared_id,
        );

        assert!(tracker.get_job(declared_id).is_some());
        assert_eq!(tracker.get_network_difficulty(), Some(expected));
    }

    #[tokio::test]
    async fn network_difficulty_survives_job_cleanup() {
        let tracker = start_tracker_actor();

        let template_str = include_str!(
            "../../../../../p2poolv2_tests/test_data/gbt/signet/gbt-no-transactions.json"
        );
        let template: BlockTemplate = serde_json::from_str(template_str).unwrap();
        let bits = bitcoin::pow::CompactTarget::from_unprefixed_hex(&template.bits).unwrap();
        let expected = bitcoin::Target::from_compact(bits).difficulty_float();

        tracker.set_network_bits(bits);
        let job_id = tracker.get_next_job_id();
        tracker.insert_job(
            Arc::new(template),
            "cb1".to_string(),
            "cb2".to_string(),
            Some(create_test_commitment()),
            TEST_COINBASE_NSECS,
            vec![],
            job_id,
        );

        assert_eq!(tracker.cleanup_old_jobs(0), 1);
        assert!(
            tracker.get_job(job_id).is_none(),
            "the job map must actually be empty"
        );
        assert_eq!(tracker.get_network_difficulty(), Some(expected));
    }

    #[tokio::test]
    async fn network_difficulty_follows_the_newest_template_bits() {
        let tracker = start_tracker_actor();

        let old_bits = CompactTarget::from_consensus(0x1b4188f5);
        let new_bits = CompactTarget::from_consensus(0x1b4188f6);
        let old_difficulty = bitcoin::Target::from_compact(old_bits).difficulty_float();
        let new_difficulty = bitcoin::Target::from_compact(new_bits).difficulty_float();

        tracker.set_network_bits(old_bits);
        assert_eq!(tracker.get_network_difficulty(), Some(old_difficulty));

        tracker.set_network_bits(new_bits);
        assert_eq!(tracker.get_network_difficulty(), Some(new_difficulty));
    }

    #[tokio::test]
    async fn pool_difficulty_is_none_before_the_first_notify() {
        let tracker = start_tracker_actor();

        assert!(tracker.get_pool_difficulty().is_none());
    }

    #[tokio::test]
    async fn pool_difficulty_follows_the_newest_notify_target() {
        let tracker = start_tracker_actor();

        let old_target = CompactTarget::from_consensus(0x1a4188f5);
        let new_target = CompactTarget::from_consensus(0x1a4188f6);
        let old_difficulty = bitcoin::Target::from_compact(old_target).difficulty_float();
        let new_difficulty = bitcoin::Target::from_compact(new_target).difficulty_float();

        tracker.set_pool_target(old_target);
        assert_eq!(tracker.get_pool_difficulty(), Some(old_difficulty));

        tracker.set_pool_target(new_target);
        assert_eq!(tracker.get_pool_difficulty(), Some(new_difficulty));
    }

    #[tokio::test]
    async fn test_block_template_operations() {
        let template_str = include_str!(
            "../../../../../p2poolv2_tests/test_data/gbt/signet/gbt-no-transactions.json"
        );

        let template: BlockTemplate = serde_json::from_str(template_str).unwrap();
        let cloned_template = template.clone();

        let tracker = start_tracker_actor();

        let job_id = tracker.insert_job(
            Arc::new(template),
            "cb1".to_string(),
            "cb2".to_string(),
            Some(create_test_commitment()),
            TEST_COINBASE_NSECS,
            vec![],
            JobId(1),
        );

        // Test inserting a block template
        assert_eq!(job_id, JobId(1));

        // Test finding the job
        let retrieved_job = tracker.get_job(job_id).unwrap();
        assert_eq!(
            cloned_template.previousblockhash,
            retrieved_job.blocktemplate.previousblockhash
        );
        assert_eq!(retrieved_job.coinbase1, "cb1".to_string());
        assert_eq!(retrieved_job.coinbase2, "cb2".to_string());
        let current_timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        assert!(current_timestamp >= retrieved_job.generation_timestamp);
        assert!(current_timestamp - retrieved_job.generation_timestamp <= 5);

        // Test with non-existent job id
        let retrieved_job = tracker.get_job(JobId(9997));
        assert!(retrieved_job.is_none());
    }

    #[tokio::test]
    async fn test_job_cleanup() {
        let template_str = include_str!(
            "../../../../../p2poolv2_tests/test_data/gbt/signet/gbt-no-transactions.json"
        );

        let template: BlockTemplate = serde_json::from_str(template_str).unwrap();

        // Create tracker directly
        let tracker = JobTracker::new();

        // Insert jobs with different timestamps
        let old_job_id = JobId(1);
        tracker.insert_job(
            Arc::new(template.clone()),
            "old_cb1".to_string(),
            "old_cb2".to_string(),
            Some(create_test_commitment()),
            TEST_COINBASE_NSECS,
            vec![],
            old_job_id,
        );

        // Manually set an old timestamp (20 minutes ago)
        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if let Some(mut job) = tracker.job_details.get_mut(&old_job_id) {
            job.generation_timestamp = current_time - (20 * 60);
        }

        // Insert a new job
        let new_job_id = JobId(2);
        tracker.insert_job(
            Arc::new(template.clone()),
            "new_cb1".to_string(),
            "new_cb2".to_string(),
            None,
            TEST_COINBASE_NSECS,
            vec![],
            new_job_id,
        );

        // Verify both jobs exist
        assert!(tracker.job_details.contains_key(&old_job_id));
        assert!(tracker.job_details.contains_key(&new_job_id));

        // Run cleanup (15 minutes max age)
        let removed = tracker.cleanup_old_jobs(15 * 60);
        assert_eq!(removed, 1);

        // Verify only the new job remains
        assert!(!tracker.job_details.contains_key(&old_job_id));
        assert!(tracker.job_details.contains_key(&new_job_id));

        // Test with tracker
        let tracker = start_tracker_actor();

        // Insert a job
        let job_id = tracker.insert_job(
            Arc::new(template.clone()),
            "actor_cb1".to_string(),
            "actor_cb2".to_string(),
            None,
            TEST_COINBASE_NSECS,
            vec![],
            JobId(3),
        );

        // Cleanup with 0 max age should remove all jobs
        let removed = tracker.cleanup_old_jobs(0);
        assert_eq!(removed, 1);

        // Verify job was removed
        let result = tracker.get_job(job_id);
        assert!(result.is_none());
    }

    #[test]
    fn test_add_share_rejects_duplicates() {
        let template_str = include_str!(
            "../../../../../p2poolv2_tests/test_data/gbt/signet/gbt-no-transactions.json"
        );
        let template: BlockTemplate = serde_json::from_str(template_str).unwrap();

        let tracker = JobTracker::new();
        let job_id = JobId(1);

        // Insert a job
        tracker.insert_job(
            Arc::new(template),
            "cb1".to_string(),
            "cb2".to_string(),
            None,
            TEST_COINBASE_NSECS,
            vec![],
            job_id,
        );

        // Create a test blockhash
        let blockhash: BlockHash =
            "0000000000000000000000000000000000000000000000000000000000000001"
                .parse()
                .unwrap();

        // First add should succeed (returns true for newly inserted)
        assert!(tracker.add_share(job_id, blockhash));

        // Second add of same blockhash should fail (returns false for duplicate)
        assert!(!tracker.add_share(job_id, blockhash));

        // Third add should still fail
        assert!(!tracker.add_share(job_id, blockhash));

        // Adding a different blockhash should succeed
        let blockhash2: BlockHash =
            "0000000000000000000000000000000000000000000000000000000000000002"
                .parse()
                .unwrap();
        assert!(tracker.add_share(job_id, blockhash2));

        // But adding blockhash2 again should fail
        assert!(!tracker.add_share(job_id, blockhash2));

        // Adding to non-existent job should return false
        let non_existent_job = JobId(999);
        assert!(!tracker.add_share(non_existent_job, blockhash));
    }

    #[test]
    fn test_concurrent_access() {
        use std::thread;

        let tracker = Arc::new(JobTracker::new());

        // Get the initial job ID before spawning threads
        let initial_id = tracker.get_latest_job_id();

        // Spawn multiple threads doing concurrent operations
        let mut trackers = vec![];

        for _ in 0..10 {
            let tracker_clone = tracker.clone();
            let tracker = thread::spawn(move || {
                // Each thread gets job IDs
                for _ in 0..100 {
                    let _job_id = tracker_clone.get_next_job_id();
                }
            });
            trackers.push(tracker);
        }

        // Wait for all threads
        for tracker in trackers {
            tracker.join().unwrap();
        }

        // Should have incremented 1000 times total (10 threads * 100 iterations)
        let final_id = tracker.get_latest_job_id();
        assert_eq!(final_id.0, initial_id.0 + 1000);
    }
}
