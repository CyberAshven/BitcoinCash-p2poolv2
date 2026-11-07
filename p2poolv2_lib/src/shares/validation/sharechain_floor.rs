// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The vardiff share chain's function-derived difficulty floor.
//!
//! Implements the consensus arithmetic of the floor rule in
//! `docs/sharechain/vardiff-sharechain.md` (section "V2: function-derived
//! floor and per-miner tiers"): over a trailing window of accepted shares,
//! every share's declared difficulty `d` and its uncles' `d` are summed per
//! payout identity, and the floor is the smallest eligible identity's share
//! of that work, scaled by the parent chain's ASERT difficulty:
//!
//! ```text
//! W_total = sum of d over all window credits
//! W_min   = min over miners with at least N window shares of that miner's work
//! floor   = ceil(W_min * D_asert_capped / W_total)
//! ```
//!
//! with `N` the presence minimum ([`SHARECHAIN_FLOOR_PRESENCE_MINIMUM`]).
//! An identity's work counts each window share's own `d` to the share's
//! `miner_bitcoin_address` and each referenced uncle's `d` to the uncle's own
//! `miner_bitcoin_address`; the nephew bonus is a payout weight and never
//! enters these sums. Presence counts window shares only: an identity needs
//! at least `N` shares of its own in the window before its work can become
//! `W_min`, while its work still adds to `W_total` below that.
//!
//! The chain ceiling is deliberately absent from the returned value: this
//! module derives the integer floor the band receives, and the fractional
//! ceiling is enforced as a separate target-space input by
//! `check_sharechain_band` and as `D_ceiling` in the tier formula. An empty
//! window, or one with no identity at the presence minimum, returns floor 0,
//! which the band's ceiling input turns into an effective floor of the chain
//! ceiling.
//!
//! All arithmetic is exact integer arithmetic: `u128` sums with
//! `saturating_add`, an integer division rounded up (never easier), then a
//! narrowing to the `u64` floor argument that fails closed, never saturates,
//! when the derived value exceeds `u64::MAX`.

use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::hash::Hash;

/// Fewest shares an identity needs in the floor window before its work can
/// be the window minimum.
///
/// A drive-by identity cannot lower the floor: with fewer than this many
/// accepted shares it still adds to `W_total` but can never be `W_min`.
pub(crate) const SHARECHAIN_FLOOR_PRESENCE_MINIMUM: u64 = 3;

/// One declared-difficulty credit the floor window counts.
///
/// A window entry contributes its own credit (its `d` to its miner, counted
/// for presence) and one credit per referenced uncle (the uncle's `d` to the
/// uncle's own miner, not counted for presence, since an uncle takes no
/// window slot).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FloorCredit<Miner> {
    /// The payout identity the credit belongs to, the share's or uncle's own
    /// `miner_bitcoin_address`. Only equality within one window is needed,
    /// never ordering.
    pub(crate) miner: Miner,
    /// The credit's declared difficulty in mainnet proof-of-work space:
    /// `Target::difficulty(Network::Bitcoin)` of the share's or uncle's bits.
    pub(crate) declared_difficulty: u128,
    /// Whether this credit is a window share and so counts towards the
    /// presence minimum. False for uncle credits.
    pub(crate) counts_for_presence: bool,
}

/// Why this module's arithmetic could not derive a floor.
///
/// The walk-level failures around the arithmetic -- an exhausted window
/// walk, a missing window uncle -- are the window cache's own cases and are
/// reported as `WindowError` variants by
/// `PplnsWindow::get_sharechain_floor_from_start_hash`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FloorError {
    /// The derived floor exceeds `u64::MAX` and leaves the rule's
    /// representable domain. The narrowing fails closed; saturation would
    /// not land at the ceiling band and would misorder the band's
    /// target-space minimum.
    WindowWorkOverflow,
}

impl fmt::Display for FloorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FloorError::WindowWorkOverflow => write!(
                formatter,
                "Floor window arithmetic exceeds u64::MAX: the derived floor is outside the \
                 representable range"
            ),
        }
    }
}

impl Error for FloorError {}

/// A miner's window presence and work while the sums accumulate.
#[derive(Default)]
struct MinerWindow {
    /// Window shares of this miner, the presence count.
    shares: u64,
    /// Sum of this miner's window credits' declared difficulties.
    work: u128,
}

/// Derive the share chain difficulty floor from one window's credits.
///
/// Returns the integer floor the band receives: `ceil(W_min *
/// d_asert_capped / W_total)`, 0 when no identity reaches
/// `presence_minimum`, and [`FloorError::WindowWorkOverflow`] when the
/// derived value exceeds `u64::MAX`.
pub(crate) fn derive_sharechain_floor<Miner: Eq + Hash>(
    credits: impl IntoIterator<Item = FloorCredit<Miner>>,
    d_asert_capped: u128,
    presence_minimum: u64,
) -> Result<u64, FloorError> {
    let mut total_declared_difficulty: u128 = 0;
    let mut per_miner: HashMap<Miner, MinerWindow> = HashMap::new();
    for credit in credits {
        total_declared_difficulty =
            total_declared_difficulty.saturating_add(credit.declared_difficulty);
        let window = per_miner.entry(credit.miner).or_default();
        if credit.counts_for_presence {
            window.shares = window.shares.saturating_add(1);
        }
        window.work = window.work.saturating_add(credit.declared_difficulty);
    }

    if total_declared_difficulty == 0 {
        return Ok(0);
    }

    let smallest_miner_work = per_miner
        .values()
        .filter(|window| window.shares >= presence_minimum)
        .map(|window| window.work)
        .min();
    let Some(smallest_miner_work) = smallest_miner_work else {
        return Ok(0);
    };

    let numerator = smallest_miner_work.saturating_mul(d_asert_capped);
    let floor = numerator.div_ceil(total_declared_difficulty);
    u64::try_from(floor).map_err(|_| FloorError::WindowWorkOverflow)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_returns_zero_for_empty_window() {
        let credits: Vec<FloorCredit<u64>> = Vec::new();

        let floor = derive_sharechain_floor(credits, 250, SHARECHAIN_FLOOR_PRESENCE_MINIMUM);

        assert_eq!(floor, Ok(0));
    }

    #[test]
    fn derive_returns_zero_when_no_miner_has_presence() {
        let floor = derive_sharechain_floor(
            [
                FloorCredit {
                    miner: 1,
                    declared_difficulty: 100,
                    counts_for_presence: true,
                },
                FloorCredit {
                    miner: 1,
                    declared_difficulty: 100,
                    counts_for_presence: true,
                },
                FloorCredit {
                    miner: 2,
                    declared_difficulty: 100,
                    counts_for_presence: true,
                },
            ],
            250,
            SHARECHAIN_FLOOR_PRESENCE_MINIMUM,
        );

        assert_eq!(floor, Ok(0));
    }

    #[test]
    fn derive_returns_asert_difficulty_for_a_single_miner_window() {
        let floor = derive_sharechain_floor(
            [
                FloorCredit {
                    miner: 1,
                    declared_difficulty: 10,
                    counts_for_presence: true,
                },
                FloorCredit {
                    miner: 1,
                    declared_difficulty: 10,
                    counts_for_presence: true,
                },
                FloorCredit {
                    miner: 1,
                    declared_difficulty: 10,
                    counts_for_presence: true,
                },
            ],
            250,
            SHARECHAIN_FLOOR_PRESENCE_MINIMUM,
        );

        assert_eq!(floor, Ok(250));
    }

    #[test]
    fn derive_dilutes_the_ratio_by_a_sub_presence_miner_work() {
        let floor = derive_sharechain_floor(
            [
                FloorCredit {
                    miner: 1,
                    declared_difficulty: 100,
                    counts_for_presence: true,
                },
                FloorCredit {
                    miner: 1,
                    declared_difficulty: 100,
                    counts_for_presence: true,
                },
                FloorCredit {
                    miner: 1,
                    declared_difficulty: 100,
                    counts_for_presence: true,
                },
                FloorCredit {
                    miner: 2,
                    declared_difficulty: 100,
                    counts_for_presence: true,
                },
                FloorCredit {
                    miner: 2,
                    declared_difficulty: 100,
                    counts_for_presence: true,
                },
            ],
            100,
            SHARECHAIN_FLOOR_PRESENCE_MINIMUM,
        );

        assert_eq!(floor, Ok(60));
    }

    #[test]
    fn derive_counts_uncle_work_for_the_uncle_miner() {
        let floor = derive_sharechain_floor(
            [
                FloorCredit {
                    miner: 1,
                    declared_difficulty: 10,
                    counts_for_presence: true,
                },
                FloorCredit {
                    miner: 2,
                    declared_difficulty: 90,
                    counts_for_presence: false,
                },
                FloorCredit {
                    miner: 1,
                    declared_difficulty: 10,
                    counts_for_presence: true,
                },
                FloorCredit {
                    miner: 2,
                    declared_difficulty: 90,
                    counts_for_presence: false,
                },
                FloorCredit {
                    miner: 1,
                    declared_difficulty: 10,
                    counts_for_presence: true,
                },
                FloorCredit {
                    miner: 2,
                    declared_difficulty: 90,
                    counts_for_presence: false,
                },
            ],
            100,
            SHARECHAIN_FLOOR_PRESENCE_MINIMUM,
        );

        assert_eq!(floor, Ok(10));
    }

    #[test]
    fn derive_uncle_work_joins_an_eligible_miner_work() {
        let floor = derive_sharechain_floor(
            [
                FloorCredit {
                    miner: 1,
                    declared_difficulty: 10,
                    counts_for_presence: true,
                },
                FloorCredit {
                    miner: 2,
                    declared_difficulty: 5,
                    counts_for_presence: true,
                },
                FloorCredit {
                    miner: 1,
                    declared_difficulty: 10,
                    counts_for_presence: true,
                },
                FloorCredit {
                    miner: 2,
                    declared_difficulty: 5,
                    counts_for_presence: true,
                },
                FloorCredit {
                    miner: 1,
                    declared_difficulty: 10,
                    counts_for_presence: true,
                },
                FloorCredit {
                    miner: 2,
                    declared_difficulty: 5,
                    counts_for_presence: true,
                },
                FloorCredit {
                    miner: 2,
                    declared_difficulty: 5,
                    counts_for_presence: false,
                },
                FloorCredit {
                    miner: 2,
                    declared_difficulty: 5,
                    counts_for_presence: false,
                },
                FloorCredit {
                    miner: 2,
                    declared_difficulty: 5,
                    counts_for_presence: false,
                },
            ],
            101,
            SHARECHAIN_FLOOR_PRESENCE_MINIMUM,
        );

        assert_eq!(floor, Ok(51));
    }

    #[test]
    fn derive_rounds_the_ratio_up() {
        let floor = derive_sharechain_floor(
            [
                FloorCredit {
                    miner: 1,
                    declared_difficulty: 1,
                    counts_for_presence: true,
                },
                FloorCredit {
                    miner: 1,
                    declared_difficulty: 1,
                    counts_for_presence: true,
                },
                FloorCredit {
                    miner: 1,
                    declared_difficulty: 1,
                    counts_for_presence: true,
                },
                FloorCredit {
                    miner: 2,
                    declared_difficulty: 2,
                    counts_for_presence: true,
                },
                FloorCredit {
                    miner: 2,
                    declared_difficulty: 2,
                    counts_for_presence: true,
                },
                FloorCredit {
                    miner: 2,
                    declared_difficulty: 2,
                    counts_for_presence: true,
                },
            ],
            10,
            SHARECHAIN_FLOOR_PRESENCE_MINIMUM,
        );

        assert_eq!(floor, Ok(4));
    }

    #[test]
    fn derive_refuses_a_floor_above_u64_max() {
        let floor = derive_sharechain_floor(
            [
                FloorCredit {
                    miner: 1,
                    declared_difficulty: 1,
                    counts_for_presence: true,
                },
                FloorCredit {
                    miner: 1,
                    declared_difficulty: 1,
                    counts_for_presence: true,
                },
                FloorCredit {
                    miner: 1,
                    declared_difficulty: 1,
                    counts_for_presence: true,
                },
            ],
            u128::from(u64::MAX) + 1,
            SHARECHAIN_FLOOR_PRESENCE_MINIMUM,
        );

        assert_eq!(floor, Err(FloorError::WindowWorkOverflow));
    }
}
