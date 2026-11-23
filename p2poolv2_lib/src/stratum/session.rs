// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::address::Address as P2PoolAddress;
use crate::stratum::error::StratumErrorCode;
use crate::stratum::work::level_policy::PayoutLevelChoice;
use crate::stratum::work::tracker::JobId;
use crate::utils::time_provider::SystemTimeProvider;
use crate::{
    stratum::difficulty_adjuster::DifficultyAdjusterTrait, utils::time_provider::TimeProvider,
};
use bitcoin::secp256k1::rand::{self, Rng};
use p2poolv2_wallet::payout_address::PayoutAddress;
use p2poolv2_wallet::share_owner::ShareOwner;
use std::collections::{HashMap, VecDeque};
use std::time::SystemTime;

/// Use 4 byte extranonce1
pub const EXTRANONCE1_SIZE: usize = 4;
/// Use 8 byte extranonce2
pub const EXTRANONCE2_SIZE: usize = 8;

/// How many recent jobs keep the difficulty they were announced at. One
/// template a minute at most reaches a session, so this holds far more
/// jobs than a session has live; a job pushed out of the ring falls back
/// to the session difficulty when it submits.
const ANNOUNCED_DIFFICULTY_CAPACITY: usize = 64;

/// Manages each sessions for each miner connection.
///
/// Stores the session ID, extranonce1, and other session-related data.
pub struct Session<D: DifficultyAdjusterTrait> {
    /// Unique session ID
    pub id: String,
    /// extranonce1 in le. Sent to the miner, computed from the session ID
    pub enonce1: u32,
    /// Extranonce1 in le, as a hex string. Sent to miner.
    pub enonce1_hex: String,
    /// Did the mine subscribe already?
    pub subscribed: bool,
    /// Optional username of the miner, supplied by the miner, we just store it in session
    pub username: Option<String>,
    /// bitcoin address used as user identifier
    pub btcaddress: Option<String>,
    /// Parsed and network-checked bitcoin Address, set once at authorization time
    pub parsed_address: Option<PayoutAddress>,
    /// Share chain address from the `p2p=` password option, owning this miner's
    /// share coinbase outputs. Set once at authorization time. Distinct from
    /// `parsed_address`, which is the bitcoin payout address on the other chain.
    pub miner_address: Option<P2PoolAddress>,
    /// Share owner committed into the work notify. Taproot for Bitcoin, a
    /// 20-byte P2PKH hash for Bitcoin Cash. Set once at authorization.
    pub share_owner: Option<ShareOwner>,
    /// Bitcoin Cash donation in basis points, resolved from the miner login.
    /// `Some(0)` is an explicit opt-out; Bitcoin leaves this unset.
    pub donation: Option<u16>,
    /// Bitcoin Cash payout level choice from the miner login (`min-payout=`
    /// or `fee-tolerance=`). `None` takes the node's level.
    pub payout_level: Option<PayoutLevelChoice>,
    /// Set once the node has warned this session that it lowered the
    /// miner's payout level (`stratum::work::level_policy`), so the warning
    /// is logged once per session.
    pub payout_level_warned: bool,
    /// Worker name for the mining device
    pub workername: Option<String>,
    /// The miner's `mining.subscribe` user agent, as sent (for example
    /// `bitaxe/24.4.0`). Captured so a bare-address login can name its
    /// worker after the device; never sent anywhere else.
    pub user_agent: Option<String>,
    /// Optional password of the miner, supplied by the miner, we just store it in session
    pub password: Option<String>,
    /// User ID from the store
    pub user_id: Option<u64>,
    /// Worker ID from the store
    pub worker_id: Option<u64>,
    /// Difficulty adjuster for the session
    pub difficulty_adjuster: D,
    /// version_mask used in this session, defaults to one provided from config
    pub version_mask: i32,
    /// Difficulty suggested by the client
    pub suggested_difficulty: Option<u64>,
    /// True when the miner set its difficulty with `d=` or `th=` in the
    /// authorize password. An explicit password difficulty takes precedence
    /// over a later mining.suggest_difficulty, which firmware often sends
    /// automatically.
    pub difficulty_from_password: bool,
    /// Instant when the session was created
    pub connected_at: SystemTime,
    /// Instant when the last valid share was submitted
    pub last_share_time: Option<SystemTime>,
    /// Authorization failed once
    pub auth_failed_once: bool,
    /// Set after authorization to trigger the first notify from the watch channel
    pub needs_first_notify: bool,
    /// Previous block hash and nBits of the last job sent to this miner. A
    /// job on another previous block, or with other nBits, is sent with
    /// `clean_jobs`.
    pub last_job_tip: Option<(String, String)>,
    /// The miner asked for `pickaxe-merge` in `mining.configure` and the
    /// node merge-mines tokens: each job's leaves and each token win's
    /// proof follow (`stratum::merge_disclosure`).
    pub merge_disclosure: bool,
    /// Rejections logged so far on this session, counted per stratum
    /// reason (`stratum::reject_log`): the first rejection of a reason
    /// logs at warn, then every `reject_log::REJECT_LOG_INTERVAL`-th.
    pub reject_logs: HashMap<StratumErrorCode, u64>,
    /// The difficulty each recent job was announced at, in notify order.
    /// A Stratum V1 share is validated and credited against its own job's
    /// announced difficulty (`stratum::message_handlers::submit`), so
    /// shares mined before the session difficulty rises stay valid on the
    /// job they were mined on; the session difficulty takes over when it
    /// falls below the announcement. A job older than the ring falls back
    /// to the session difficulty.
    announced_difficulties: VecDeque<(JobId, u64)>,
}

impl<D: DifficultyAdjusterTrait> Session<D> {
    /// Creates a new session with the given minimum difficulty.
    pub fn new(
        start_difficulty: u64,
        minimum_difficulty: u64,
        maximum_difficulty: Option<u64>,
        version_mask: i32,
    ) -> Self {
        let id = Session::<D>::generate_id();
        let enonce1 = id.to_le();
        let now = SystemTimeProvider.now();
        Self {
            id: hex::encode(id.to_be_bytes()),
            enonce1,
            enonce1_hex: hex::encode(enonce1.to_le_bytes()),
            subscribed: false,
            username: None,
            workername: None,
            user_agent: None,
            btcaddress: None,
            parsed_address: None,
            miner_address: None,
            share_owner: None,
            donation: None,
            payout_level: None,
            payout_level_warned: false,
            password: None,
            user_id: None,
            worker_id: None,
            difficulty_adjuster: D::new(start_difficulty, minimum_difficulty, maximum_difficulty),
            version_mask,
            suggested_difficulty: None,
            difficulty_from_password: false,
            connected_at: now,
            last_share_time: None,
            auth_failed_once: false,
            needs_first_notify: false,
            last_job_tip: None,
            merge_disclosure: false,
            reject_logs: HashMap::new(),
            announced_difficulties: VecDeque::with_capacity(ANNOUNCED_DIFFICULTY_CAPACITY),
        }
    }

    /// Remember the difficulty `mining.notify` announced for `job_id`, so a
    /// share on it is judged against that difficulty and not the live
    /// session difficulty. Re-announcing a job id overwrites its entry.
    pub fn record_job_announcement(&mut self, job_id: JobId, difficulty: u64) {
        if let Some(entry) = self
            .announced_difficulties
            .iter_mut()
            .find(|(announced_job, _)| *announced_job == job_id)
        {
            entry.1 = difficulty;
            return;
        }
        if self.announced_difficulties.len() == ANNOUNCED_DIFFICULTY_CAPACITY {
            self.announced_difficulties.pop_front();
        }
        self.announced_difficulties.push_back((job_id, difficulty));
    }

    /// The difficulty `job_id` was announced at, or `None` once it has
    /// aged out of [`Self::record_job_announcement`]'s ring.
    pub fn announced_difficulty(&self, job_id: JobId) -> Option<u64> {
        self.announced_difficulties
            .iter()
            .find(|(announced_job, _)| *announced_job == job_id)
            .map(|(_, difficulty)| *difficulty)
    }

    /// Generates a random session ID.
    fn generate_id() -> u32 {
        let mut rng = rand::thread_rng();
        rng.r#gen::<u32>()
    }
}

#[cfg(test)]
mod tests {
    use p2poolv2_config::DEFAULT_VERSION_MASK;

    use super::*;
    use crate::stratum::difficulty_adjuster::DifficultyAdjuster;

    #[test]
    fn test_new_session() {
        let min_difficulty = 1000;
        let start_difficulty = 100;
        let session = Session::<DifficultyAdjuster>::new(
            start_difficulty,
            min_difficulty,
            Some(2000),
            DEFAULT_VERSION_MASK,
        );

        assert_eq!(
            session.difficulty_adjuster.pool_minimum_difficulty,
            min_difficulty
        );
        assert_eq!(
            session.difficulty_adjuster.current_difficulty,
            start_difficulty
        );
        assert_ne!(session.id, "");

        // Verify that id and enonce1_hex are reverse encodings of each other
        let id_bytes = hex::decode(&session.id).unwrap();
        let enonce1_hex_bytes = hex::decode(&session.enonce1_hex).unwrap();
        assert_eq!(
            id_bytes,
            enonce1_hex_bytes.iter().rev().cloned().collect::<Vec<_>>()
        );

        // session.id is BE encoded
        assert_eq!(
            session.enonce1,
            u32::from_be_bytes(
                hex::decode(session.id)
                    .unwrap()
                    .as_slice()
                    .try_into()
                    .unwrap()
            )
        );

        // session.enonce1 is LE encoded
        assert_eq!(
            session.enonce1,
            u32::from_le_bytes(
                hex::decode(&session.enonce1_hex)
                    .unwrap()
                    .try_into()
                    .unwrap()
            )
        );
        assert!(!session.subscribed);
        assert_eq!(session.donation, None);
    }

    #[test]
    fn test_get_current_difficulty() {
        let session =
            Session::<DifficultyAdjuster>::new(100, 2000, Some(3000), DEFAULT_VERSION_MASK);

        assert_eq!(session.difficulty_adjuster.current_difficulty, 100);
    }

    #[test]
    fn a_sessions_announced_difficulty_remembers_each_jobs_own() {
        let mut session = Session::<DifficultyAdjuster>::new(1, 1, None, DEFAULT_VERSION_MASK);
        session.record_job_announcement(JobId(11), 1);
        session.record_job_announcement(JobId(12), 16_384);

        assert_eq!(session.announced_difficulty(JobId(11)), Some(1));
        assert_eq!(session.announced_difficulty(JobId(12)), Some(16_384));
        assert_eq!(session.announced_difficulty(JobId(13)), None);
    }

    #[test]
    fn re_announcing_a_job_id_replaces_its_difficulty_in_place() {
        let mut session = Session::<DifficultyAdjuster>::new(1, 1, None, DEFAULT_VERSION_MASK);
        session.record_job_announcement(JobId(7), 1);
        session.record_job_announcement(JobId(8), 1);
        session.record_job_announcement(JobId(7), 4096);

        assert_eq!(session.announced_difficulty(JobId(7)), Some(4096));
        assert_eq!(session.announced_difficulty(JobId(8)), Some(1));
    }

    #[test]
    fn a_job_announced_too_long_ago_falls_out_of_the_ring() {
        let mut session = Session::<DifficultyAdjuster>::new(1, 1, None, DEFAULT_VERSION_MASK);
        session.record_job_announcement(JobId(1), 1);
        for job_index in 2..=(ANNOUNCED_DIFFICULTY_CAPACITY as u64 + 1) {
            session.record_job_announcement(JobId(job_index), 1);
        }

        assert_eq!(session.announced_difficulty(JobId(1)), None);
        assert_eq!(
            session.announced_difficulty(JobId(ANNOUNCED_DIFFICULTY_CAPACITY as u64)),
            Some(1)
        );
        assert_eq!(
            session.announced_difficulty(JobId(ANNOUNCED_DIFFICULTY_CAPACITY as u64 + 1)),
            Some(1)
        );
    }
}
