// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `/sharechain`: the sharechain chart's series as JSON.
//!
//! Reads shares from the store and builds the same five series the
//! server-rendered Sharechain tab draws, so both charts show the same lines.
//! No sampling happens here; the endpoint reuses the SSR's
//! [`share_series`] and [`sharechain_series`] from the dashboard module.

use crate::api::dashboard::history::{self, HashrateHistory, Sample};
use crate::api::dashboard::pages::sharechain_series;
use crate::api::dashboard::share_series;
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

const DEFAULT_WINDOW_SECS: u64 = 3600;
const MAX_WINDOW_SECS: u64 = RETENTION_SECS;

#[derive(Deserialize)]
pub struct SharechainQuery {
    pub window: Option<u64>,
}

/// One point: a unix timestamp and its value.
#[derive(Serialize, Debug, PartialEq)]
pub struct SharechainPoint {
    pub time: u64,
    pub value: f64,
}

/// The five series in the replica's order: height, dmin, dmax, uncles, weak.
#[derive(Serialize)]
pub struct SharechainResponse {
    pub window_secs: u64,
    pub sample_interval_secs: u64,
    pub height: Vec<SharechainPoint>,
    pub diff_min: Vec<SharechainPoint>,
    pub diff_max: Vec<SharechainPoint>,
    pub uncles: Vec<SharechainPoint>,
    pub weak: Vec<SharechainPoint>,
}

struct SharechainState {
    app: Arc<AppState>,
    history: Arc<HashrateHistory>,
}

pub(crate) fn router(app: Arc<AppState>, history: Arc<HashrateHistory>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/sharechain", get(sharechain))
        .with_state(Arc::new(SharechainState { app, history }))
}

fn to_points(series: &[(u64, f64)]) -> Vec<SharechainPoint> {
    series
        .iter()
        .map(|(time, value)| SharechainPoint {
            time: *time,
            value: *value,
        })
        .collect()
}

async fn sharechain(
    State(state): State<Arc<SharechainState>>,
    Query(query): Query<SharechainQuery>,
) -> Result<Json<SharechainResponse>, ApiError> {
    let window = query.window.unwrap_or(DEFAULT_WINDOW_SECS);
    if !(60..=MAX_WINDOW_SECS).contains(&window) {
        return Err(ApiError::BadRequest(format!(
            "window must be between 60 and {MAX_WINDOW_SECS} seconds, got {window}"
        )));
    }
    let samples = state.history.samples();
    // Keep only samples within the window, oldest first.
    let window_samples: Vec<Sample> = {
        let newest_time = samples.last().map(|s| s.time).unwrap_or(0);
        let start = newest_time.saturating_sub(window);
        samples.into_iter().filter(|s| s.time >= start).collect()
    };
    // Share reads go through the store on a blocking thread.
    let blocking_app = Arc::clone(&state.app);
    let (shares, window_secs) = tokio::task::spawn_blocking(move || {
        let shares = share_series(&blocking_app, window);
        (shares, window)
    })
    .await
    .unwrap_or_default();
    let (height, diff_min, diff_max, uncles, weak) = sharechain_series(&shares, &window_samples);
    Ok(Json(SharechainResponse {
        window_secs,
        sample_interval_secs: history::SAMPLE_INTERVAL.as_secs(),
        height: to_points(&height),
        diff_min: to_points(&diff_min),
        diff_max: to_points(&diff_max),
        uncles: to_points(&uncles),
        weak: to_points(&weak),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn to_points_maps_time_and_value() {
        let series = vec![(100, 42.5), (160, 37.0)];
        let points = to_points(&series);
        assert_eq!(points.len(), 2);
        assert_eq!(
            points[0],
            SharechainPoint {
                time: 100,
                value: 42.5
            }
        );
        assert_eq!(
            points[1],
            SharechainPoint {
                time: 160,
                value: 37.0
            }
        );
    }
}
