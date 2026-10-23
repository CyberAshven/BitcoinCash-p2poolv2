// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `/miners`: the miners mining on this node and their workers, as JSON.
//!
//! The rows the dashboard's miners page (`/dashboard/miners`) shows, built by
//! the same code, so a frontend such as `p2poolv2_web` shows the same numbers.
//! The list is paged like that page, and only the returned page's miners are
//! looked up in the PPLNS window for their payout fraction.

use crate::api::dashboard::pages::{MinerRow, PAGE_SIZE};
use crate::api::dashboard::{is_looked_up_miner, local_miners, miners_page, online_counts};
use crate::api::endpoints::miner::miner_report;
use crate::api::server::AppState;
use axum::{
    Json,
    extract::{Query, State},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Query parameters for the /miners endpoint.
#[derive(Deserialize)]
pub(crate) struct MinersQuery {
    /// Page of the list, counted from 1. Defaults to the first.
    pub page: Option<usize>,
    /// Only the miner with this payout address, typed as the miner page
    /// takes it.
    pub address: Option<String>,
}

/// JSON response for the /miners endpoint.
#[derive(Serialize)]
pub(crate) struct MinersResponse {
    /// Miners on this node with a worker online, whatever `address` says.
    pub miners_online: usize,
    /// Workers online on this node in all.
    pub workers_online: usize,
    /// Miners listed, on every page.
    pub total: usize,
    /// The page returned, counted from 1.
    pub page: usize,
    /// Pages there are.
    pub pages: usize,
    /// Most miners on a page.
    pub page_size: usize,
    /// This page's miners by accepted work, most first, with their workers.
    pub miners: Vec<MinerRow>,
}

/// Returns a page of the miners on this node, or the one named by `address`.
pub(crate) async fn miners(
    State(state): State<Arc<AppState>>,
    Query(query): Query<MinersQuery>,
) -> Json<MinersResponse> {
    let all = local_miners(&state).await;
    let (miners_online, workers_online) = online_counts(&all);
    let typed = query
        .address
        .as_deref()
        .map(str::trim)
        .filter(|address| !address.is_empty());
    let listed = match typed {
        Some(typed) => {
            let canonical = miner_report(&state, typed, 1)
                .await
                .ok()
                .map(|report| report.address);
            all.into_iter()
                .filter(|miner| is_looked_up_miner(miner, typed, canonical.as_deref()))
                .collect()
        }
        None => all,
    };
    let total = listed.len();
    let (miners, paging) = miners_page(&state, listed, query.page).await;
    Json(MinersResponse {
        miners_online,
        workers_online,
        total,
        page: paging.page,
        pages: paging.pages,
        page_size: PAGE_SIZE,
        miners,
    })
}
