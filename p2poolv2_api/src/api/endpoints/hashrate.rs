// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `/hashrate`: the dashboard's hashrate history as JSON.
//!
//! A view over the same [`HashrateHistory`] the server-rendered dashboard
//! draws, so both show the same numbers. No sampling happens here.

use crate::api::dashboard::history::{self, HashrateHistory, Sample};
use crate::api::error::ApiError;
use crate::api::server::AppState;
use axum::{
    Json, Router,
    extract::{Query, State},
    routing::get,
};
use p2poolv2_lib::store::hashrate_samples::RETENTION_SECS;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Default window: one hour. Maximum: the store's 30-day retention, from
/// which the ring's bound is derived too.
const DEFAULT_WINDOW_SECS: u64 = 3600;
const MAX_WINDOW_SECS: u64 = RETENTION_SECS;

#[derive(Deserialize)]
pub struct HashrateQuery {
    /// Seconds of history to return, ending at the newest sample.
    pub window: Option<u64>,
}

/// One sample, in hashes per second; `null` where it could not be known.
#[derive(Serialize, Debug, PartialEq)]
pub struct HashratePoint {
    pub time: u64,
    pub pool_hashrate: Option<f64>,
    pub local_hashrate: Option<f64>,
    pub network_hashrate: Option<f64>,
}

impl From<&Sample> for HashratePoint {
    fn from(sample: &Sample) -> Self {
        Self {
            time: sample.time,
            pool_hashrate: sample.pool,
            local_hashrate: sample.local,
            network_hashrate: sample.network,
        }
    }
}

#[derive(Serialize)]
pub struct HashrateResponse {
    pub window_secs: u64,
    pub sample_interval_secs: u64,
    /// Current values: the newest sample, or read live before the first one.
    pub pool_hashrate: Option<f64>,
    pub local_hashrate: Option<f64>,
    pub network_hashrate: Option<f64>,
    /// Current pool share difficulty (the ASERT target of the newest
    /// prepared notify, in the share chain's units). Live tracker read,
    /// independent of the samples: None until the first notify is built.
    pub share_difficulty: Option<f64>,
    /// Samples in the window, oldest first.
    pub history: Vec<HashratePoint>,
}

struct HashrateState {
    app: Arc<AppState>,
    history: Arc<HashrateHistory>,
}

pub(crate) fn router(app: Arc<AppState>, history: Arc<HashrateHistory>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/hashrate", get(hashrate))
        .with_state(Arc::new(HashrateState { app, history }))
}

/// The samples within `window` seconds of the newest one.
fn in_window(samples: &[Sample], window: u64) -> Vec<HashratePoint> {
    let Some(newest) = samples.last() else {
        return Vec::new();
    };
    let start = newest.time.saturating_sub(window);
    samples
        .iter()
        .filter(|sample| sample.time >= start)
        .map(HashratePoint::from)
        .collect()
}

/// GET /hashrate: the dashboard's hashrate history as JSON.
///
/// `window` is seconds of history ending at the newest sample, clamped
/// to `[60, MAX_WINDOW_SECS]` and validated out of range as 400 rather
/// than silently clamped. `MAX_WINDOW_SECS` is the store's 30-day
/// retention, the 30-day surface; the overview chart stays at 1440
/// points so its "last day" title remains truthful.
async fn hashrate(
    State(state): State<Arc<HashrateState>>,
    Query(query): Query<HashrateQuery>,
) -> Result<Json<HashrateResponse>, ApiError> {
    let window = query.window.unwrap_or(DEFAULT_WINDOW_SECS);
    if !(60..=MAX_WINDOW_SECS).contains(&window) {
        return Err(ApiError::BadRequest(format!(
            "window must be between 60 and {MAX_WINDOW_SECS} seconds, got {window}"
        )));
    }
    let samples = state.history.samples();
    let current = match samples.last() {
        Some(newest) => HashratePoint::from(newest),
        None => {
            // Before the first sample: read pool and network live (the
            // store read is synchronous, so keep it off the async workers)
            // and the local rate from the metrics actor's window.
            let app = Arc::clone(&state.app);
            let now = history::unix_now();
            let local = state
                .app
                .metrics_handle
                .get_metrics()
                .await
                .local_submission_hashrate(now);
            let pool = tokio::task::spawn_blocking(move || {
                let shares = history::newest_shares(&app, history::POOL_HASHRATE_SHARES);
                let points: Vec<(u32, bitcoin::CompactTarget)> = shares
                    .iter()
                    .map(|share| (share.timestamp, share.bits))
                    .collect();
                history::share_chain_hashrate(&points, now)
            })
            .await
            .unwrap_or_default();
            HashratePoint {
                time: 0,
                pool_hashrate: history::normalize_rate(pool),
                local_hashrate: history::normalize_rate(local),
                network_hashrate: state
                    .app
                    .tracker_handle
                    .get_network_difficulty()
                    .map(history::network_hashrate),
            }
        }
    };
    Ok(Json(HashrateResponse {
        window_secs: window,
        sample_interval_secs: history::SAMPLE_INTERVAL.as_secs(),
        pool_hashrate: current.pool_hashrate,
        local_hashrate: current.local_hashrate,
        network_hashrate: current.network_hashrate,
        share_difficulty: state.app.tracker_handle.get_pool_difficulty(),
        history: in_window(&samples, window),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(time: u64) -> Sample {
        Sample {
            time,
            pool: Some(1.0),
            local: None,
            network: Some(2.0),
        }
    }

    #[test]
    fn test_in_window_ends_at_newest_sample() {
        let samples = [sample(0), sample(60), sample(120), sample(180)];
        let points = in_window(&samples, 60);
        assert_eq!(
            points.iter().map(|p| p.time).collect::<Vec<_>>(),
            vec![120, 180]
        );
        assert_eq!(points[0].pool_hashrate, Some(1.0));
        assert_eq!(points[0].local_hashrate, None);
        assert!(in_window(&[], 60).is_empty());
    }

    #[test]
    fn a_full_retention_window_is_accepted() {
        assert_eq!(MAX_WINDOW_SECS, RETENTION_SECS);
        assert_eq!(RETENTION_SECS, 2_592_000);
    }

    #[test]
    fn a_window_above_retention_is_rejected() {
        assert!(!(60..=MAX_WINDOW_SECS).contains(&(RETENTION_SECS + 1)));
    }

    #[test]
    fn in_window_keeps_the_newest_full_retention_span() {
        let samples = [
            sample(0),
            sample(RETENTION_SECS - 60),
            sample(RETENTION_SECS),
        ];
        let points = in_window(&samples, RETENTION_SECS);
        assert_eq!(
            points.iter().map(|point| point.time).collect::<Vec<_>>(),
            vec![0, RETENTION_SECS - 60, RETENTION_SECS]
        );
    }
}
