// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The app's pages, one module per page of the node's dashboard, with the
//! same markup and text. Only the settings page has no counterpart there:
//! the node's pages need no API URL or login of their own.

mod blocks;
mod help;
mod miners;
mod overview;
mod search;
mod settings;
mod share;

pub use blocks::{BlockPage, BlocksPage};
pub use help::HelpPage;
pub use miners::{MinerPage, MinersPage};
pub use overview::OverviewPage;
pub use search::SearchPage;
pub use settings::SettingsPage;
pub use share::SharePage;
