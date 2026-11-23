// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Throttled warn logging for shares and authorizations the server rejects.
//!
//! Every rejection used to be silent (or debug-only), which left a rejected
//! burst with no trace in the log - a miner reconnect-storm showed up only as
//! a counter jump. This module logs each rejection at warn with its stratum
//! reason code, throttled per session and per reason: the first rejection of
//! a reason on a session always logs, then every `REJECT_LOG_INTERVAL`-th.

use crate::stratum::difficulty_adjuster::DifficultyAdjusterTrait;
use crate::stratum::error::StratumErrorCode;
use crate::stratum::session::Session;
use tracing::warn;

/// Log the first rejection of each reason on a session, then every
/// 100th. At a peak burst (roughly 33 rejects/s seen in the field) an
/// unthrottled warn would add thousands of lines per minute, while the
/// opening line of each reason still pinpoints when the problem started.
pub const REJECT_LOG_INTERVAL: u64 = 100;

/// Record one rejection on this session and log it at warn if the
/// throttle allows: the first rejection of `reason` on the session, then
/// every `REJECT_LOG_INTERVAL`-th.
///
/// `job_id` and `detail` are logged when present; `detail` carries the
/// specifics a reason alone lacks (the validation error, or the share's
/// true difficulty against the session difficulty). Callers pass no job
/// for rejections that happen before a job is resolved.
pub(crate) fn handle_reject<D: DifficultyAdjusterTrait>(
    session: &mut Session<D>,
    reason: StratumErrorCode,
    job_id: Option<u64>,
    detail: Option<&str>,
) -> bool {
    let seen = session.reject_logs.entry(reason).or_insert(0_u64);
    *seen += 1;
    let count = *seen;
    if !(count - 1).is_multiple_of(REJECT_LOG_INTERVAL) {
        return false;
    }
    let label = reason.label();
    let job_part = job_id.map_or_else(String::new, |job| format!(", job {job:#x}"));
    let detail_part = detail.map_or_else(String::new, |text| format!(": {text}"));
    warn!(
        "Stratum reject {label} (code {}){job_part}{detail_part}",
        reason as i32
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stratum::difficulty_adjuster::DifficultyAdjuster;
    use p2poolv2_config::DEFAULT_VERSION_MASK;

    #[test]
    fn a_session_starts_with_no_reject_log_state() {
        let session = Session::<DifficultyAdjuster>::new(100, 1, Some(2000), DEFAULT_VERSION_MASK);
        assert!(session.reject_logs.is_empty());
    }

    #[test]
    fn first_rejection_of_each_reason_is_logged() {
        let mut session =
            Session::<DifficultyAdjuster>::new(100, 1, Some(2000), DEFAULT_VERSION_MASK);
        assert!(handle_reject(
            &mut session,
            StratumErrorCode::NotSubscribed,
            None,
            None
        ));
        assert!(handle_reject(
            &mut session,
            StratumErrorCode::UnauthorizedWorker,
            None,
            None
        ));
        assert!(handle_reject(
            &mut session,
            StratumErrorCode::JobNotFound,
            Some(1),
            None
        ));
        assert!(handle_reject(
            &mut session,
            StratumErrorCode::DuplicateShare,
            Some(1),
            None
        ));
        assert!(handle_reject(
            &mut session,
            StratumErrorCode::LowDifficultyShare,
            Some(1),
            Some("truediff 1 below session difficulty 2")
        ));
        assert!(handle_reject(
            &mut session,
            StratumErrorCode::OtherUnknown,
            Some(1),
            Some("nTime out of range")
        ));
    }

    #[test]
    fn repeated_reason_is_suppressed_until_the_interval() {
        let mut session =
            Session::<DifficultyAdjuster>::new(100, 1, Some(2000), DEFAULT_VERSION_MASK);
        assert!(handle_reject(
            &mut session,
            StratumErrorCode::JobNotFound,
            Some(7),
            None
        ));
        // Calls 2 through 100 on this session stay silent.
        for _ in 1..REJECT_LOG_INTERVAL {
            assert!(!handle_reject(
                &mut session,
                StratumErrorCode::JobNotFound,
                Some(7),
                None
            ));
        }
        // The 100th rejection after the first logs again.
        assert!(handle_reject(
            &mut session,
            StratumErrorCode::JobNotFound,
            Some(7),
            None
        ));
    }

    #[test]
    fn suppression_counts_per_reason_independently() {
        let mut session =
            Session::<DifficultyAdjuster>::new(100, 1, Some(2000), DEFAULT_VERSION_MASK);
        assert!(handle_reject(
            &mut session,
            StratumErrorCode::JobNotFound,
            Some(1),
            None
        ));
        assert!(handle_reject(
            &mut session,
            StratumErrorCode::DuplicateShare,
            Some(1),
            None
        ));
        assert!(!handle_reject(
            &mut session,
            StratumErrorCode::JobNotFound,
            Some(1),
            None
        ));
        assert!(!handle_reject(
            &mut session,
            StratumErrorCode::DuplicateShare,
            Some(1),
            None
        ));
    }
}
