// SPDX-License-Identifier: AGPL-3.0-or-later

//! Direct, minimum-payout split prototype. No share-chain or wallet state.

pub mod program;
pub mod reference;
#[cfg(feature = "test-utils")]
pub mod test_utils;
pub mod transaction;

pub use reference::{Entry, ListOrder, Manifest, ProofBinding};
pub use transaction::{BuiltSplit, SplitOptions, build_split};
