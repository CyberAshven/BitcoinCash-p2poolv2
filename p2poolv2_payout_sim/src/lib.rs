// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! An offline Monte Carlo of payout variance and fees under the Bitcoin Cash
//! share chain designs of `docs/sharechain/small-miners-design.md` (issue
//! #79, row 2).
//!
//! It is not a network simulation. Each chain is taken in steady state:
//! ASERT has settled the minimum difficulty, the band cap is fixed, the weak
//! ratio has settled. Two tracked miners, a 1.2 TH/s Bitaxe and a 100 TH/s
//! ASIC, are simulated share by share and block by block over many months,
//! against the expected work of the rest of the pool, to measure how often
//! they are paid, how much each payment is, how much their monthly income
//! swings and what spending the payments costs. Every run is a pure
//! function of its scenario and seed.
//!
//! See `README.md` for the model, its assumptions and the results.

pub mod constants;
pub mod difficulty;
pub mod levels;
pub mod miners;
pub mod payouts;
pub mod races;
pub mod report;
pub mod sampling;
pub mod scenario;
pub mod study;
pub mod tables;
pub mod transition;
pub mod weak;
pub mod window;
