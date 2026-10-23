// SPDX-FileCopyrightText: 2024 PDM Authors
// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Terminal UI for a Bitcoin Cash P2Poolv2 node and the BCHN it mines on.
//!
//! Ported from P2Poolv2 PDM (<https://github.com/p2poolv2/pdm>), the P2Poolv2
//! Deployment and Management Tool, for Bitcoin Cash: BCHN over JSON-RPC
//! instead of Bitcoin Core, the fork's node API, and no Lightning.
//!
//! The views draw from plain data ([`model`], [`remote::Remote`]) and never
//! fetch anything, so another ratatui program can draw them from its own
//! data with `default-features = false`. The `client` feature adds the API
//! and BCHN clients, the live feed, the node config editor, the settings
//! file and the `p2poolv2_tui` binary.

#![forbid(unsafe_code)]

pub mod explorer;
pub mod format;
pub mod model;
pub mod network;
pub mod remote;
pub mod views;

#[cfg(feature = "client")]
pub mod app;
#[cfg(feature = "client")]
pub mod client;
#[cfg(feature = "client")]
pub mod config_editor;
#[cfg(feature = "client")]
pub mod input;
#[cfg(feature = "client")]
pub mod node_config;
#[cfg(feature = "client")]
pub mod settings;

#[cfg(test)]
pub(crate) mod test_utils;
