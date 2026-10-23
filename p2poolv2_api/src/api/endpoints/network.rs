// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `/network`: the chain this node mines, named as the dashboard names it.
//!
//! A frontend such as `p2poolv2_web` shows it in the same header badge and
//! footer as the server-rendered pages.

use crate::api::dashboard::{self, pages::Site};
use crate::api::server::AppState;
use axum::{Json, extract::State};
use std::sync::Arc;

/// Returns `{"network": "Bitcoin Cash chipnet", "badge": "chipnet"}` for a
/// chipnet node.
pub(crate) async fn network(State(state): State<Arc<AppState>>) -> Json<Site> {
    Json(dashboard::site(&state))
}
