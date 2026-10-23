// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Share difficulty adjustment simulator for the Bitcoin Cash share chain.
//!
//! A Rust port of `p2poolv2/pool_difficulty_adjustment_analysis`
//! (`daa_simulation.py` at commit `6e6ce1a`, by pool2win for P2Poolv2),
//! which compared six difficulty adjustment algorithms on a 10-second
//! chain. That repository carries no license file, so this crate is a new
//! implementation of its model: the same scenarios, algorithms, metrics and
//! scores, written from the published description and code, with credit.
//!
//! What changes from upstream:
//!
//! - The ASERT rules are the share chain's own integer code from
//!   `p2poolv2_lib::pool_difficulty`, not a floating-point re-implementation:
//!   [`rules::Rule::ParentAnchor`] is the production Bitcoin Cash rule
//!   (decision 12) through `PoolDifficulty`, and the genesis anchor upstream
//!   chose runs through the same type on its Bitcoin rule.
//! - Every rule reads what a share header carries: whole-second timestamps
//!   and compact targets, under the chain's ceiling.
//! - Hashrate is in hashes per second and difficulty in share difficulty, so
//!   scenarios can name real devices, and stratum's per-connection
//!   difficulty bounds how fast shares can arrive (see [`simulate`]).
//! - Bitcoin Cash scenarios: miners switching between BTC and BCH, a pool
//!   growing from one BitAxe to exahashes, and long quiet periods.
//!
//! Every run is seeded ([`p2poolv2_tools::seeded::SeededSequence`]), so a
//! seed gives the same numbers on the same platform. The analysis is in
//! `docs/difficulty_adjustment/bch-daa-analysis.md`.

pub mod chart;
pub mod difficulty;
pub mod metrics;
pub mod report;
pub mod rules;
pub mod scenario;
pub mod simulate;
pub mod suite;
