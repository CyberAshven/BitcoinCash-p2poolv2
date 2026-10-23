// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! A discrete-event simulator of share propagation in a Bitcoin Cash
//! P2Poolv2 network, to find what limits P2Pool on BCH as it grows: the
//! share chain, the coinbase or bandwidth.
//!
//! It ports the idea of upstream's ns-3 simulation (p2poolv2/Ns3_simulation,
//! `scratch/p2pool_v2_with_share.cc`: miners on a small-world network
//! gossiping shares, counting uncles and orphans) to Rust, without ns-3 and
//! without copying its code, and makes it about Bitcoin Cash and scale:
//! share sizes counted from this repository's encoders, coinbases for up to
//! a million payees under the owner's planned hybrid payout rule, BCH blocks
//! from 2 MB to BCHN's 2 GB guard, and bandwidth and latency drawn from
//! distributions. Every run is a pure function of its scenario and seed.
//!
//! See `docs/simulation/network-sim.md` for the model, the results and what
//! they mean.

pub mod constants;
pub mod distribution;
pub mod engine;
pub mod payees;
pub mod random;
pub mod report;
pub mod scenario;
pub mod sizes;
pub mod sweep;
pub mod topology;
pub mod traffic;
