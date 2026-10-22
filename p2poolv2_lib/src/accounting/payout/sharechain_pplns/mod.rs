// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Share chain PPLNS payout distribution.
//!
//! Computes payout distributions directly from the confirmed share chain,
//! applying uncle weighting: uncles receive 90% of their work, and
//! confirmed shares that reference uncles receive a 10% bonus per uncle.
//! On Bitcoin Cash a share's own weight goes to whoever holds its payout
//! right (`payout_rights`).

mod address_keys;
pub mod payout;
pub mod payout_rights;
pub mod pplns_window;

pub use payout::Payout;
#[cfg(test)]
pub use pplns_window::MockPplnsWindow;
pub use pplns_window::PplnsWindow;
