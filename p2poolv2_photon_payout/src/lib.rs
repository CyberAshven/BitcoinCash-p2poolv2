// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Isolated, offline PHOTON payout covenant feasibility prototype.

pub mod covenant;
pub mod lean;
pub mod reference;
pub mod simulation;
pub mod state;
#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils;
pub mod transaction;
pub mod vm;
