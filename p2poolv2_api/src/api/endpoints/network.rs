// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `/network`: the chain this node mines, named as the dashboard names it.
//!
//! A frontend such as `p2poolv2_web` shows it in the same header badge and
//! footer as the server-rendered pages.

use crate::api::dashboard;
use crate::api::server::AppState;
use axum::{Json, extract::State};
use serde::Serialize;
use std::sync::Arc;

/// The network badge as the dashboard names it: `auth` tells a frontend
/// whether the node asks for a login at all.
#[derive(Serialize)]
pub struct NetworkBadge {
    network: String,
    badge: String,
    auth: bool,
}

/// Returns `{"network": "Bitcoin Cash chipnet", "badge": "chipnet",
/// "auth": false}` for a public chipnet node.
pub(crate) async fn network(State(state): State<Arc<AppState>>) -> Json<NetworkBadge> {
    let site = dashboard::site(&state);
    Json(NetworkBadge {
        network: site.network,
        badge: site.badge,
        auth: site.auth,
    })
}
