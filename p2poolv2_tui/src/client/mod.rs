// SPDX-FileCopyrightText: 2024 PDM Authors
// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Clients for the node's API, its `/ws` live feed and BCHN's JSON-RPC.
//!
//! Every request has a timeout and a cap on the body it reads, and no error
//! message carries a URL, so a login in a URL never reaches the screen.

pub mod api;
pub mod bchn;
pub mod http;
pub mod live;
