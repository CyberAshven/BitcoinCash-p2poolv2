// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The payout level a Bitcoin Cash node gives each miner's jobs
//! (`docs/sharechain/small-miners-design.md`, part 4, "Choosing a level").
//! Node policy, not a share-chain rule: any level from 0 to
//! [`MAX_PAYOUT_LEVEL`] is valid, and the draw pays whatever level each
//! share carries.
//!
//! A miner chooses its level at login with `min-payout=` or
//! `fee-tolerance=` ([`PayoutLevelChoice`]); without either it gets the
//! node's `payout_level`. When the miner's expected payments a month at that
//! level would fall below [`MIN_PAYMENTS_PER_MONTH`], the node lowers the
//! level until they reach it, or to level 0
//! ([`affordable_payout_level`]). The estimate takes the miner's hashrate,
//! the pool's hashrate and the template's coinbase value: the miner earns
//! its share of the pool's hashrate of each block, and at a level above that
//! amount the draw pays it about once for every level's worth earned.

use super::band_policy::{MONTH_SECONDS, expected_hashes, pool_blocks_per_second};
use super::prepared_notify::PreparedNotifyParams;
use crate::accounting::payout::payout_levels::{MAX_PAYOUT_LEVEL, level_amount};
use crate::stratum::difficulty_adjuster::DifficultyAdjusterTrait;
use crate::stratum::session::Session;
use bitcoin::CompactTarget;
use tracing::warn;

/// Payments a month below which the node lowers a miner's payout level.
pub const MIN_PAYMENTS_PER_MONTH: f64 = 3.0;

/// Basis points in one.
const BASIS_POINTS: u128 = 10_000;

/// A miner's choice of payout level at login.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayoutLevelChoice {
    /// `min-payout=<satoshis>`: the smallest level at or above this amount.
    MinPayout(u64),
    /// `fee-tolerance=<percent>`, in basis points: the smallest level whose
    /// payment spending one costs at most this share of.
    FeeTolerance(u16),
}

impl PayoutLevelChoice {
    /// The level this choice gives when spending one payment costs
    /// `spend_cost` satoshis.
    pub fn level(self, spend_cost: u64) -> u8 {
        match self {
            PayoutLevelChoice::MinPayout(satoshis) => level_at_or_above(satoshis),
            PayoutLevelChoice::FeeTolerance(basis_points) => {
                level_within_fee(basis_points, spend_cost)
            }
        }
    }
}

/// The smallest level whose amount is at least `satoshis`, at most the
/// highest level.
pub fn level_at_or_above(satoshis: u64) -> u8 {
    (0..=MAX_PAYOUT_LEVEL)
        .find(|level| level_amount(*level) >= satoshis)
        .unwrap_or(MAX_PAYOUT_LEVEL)
}

/// The smallest level whose `spend_cost / L_k` is at most `basis_points`
/// of one, compared exactly as `spend_cost * 10,000 <= basis_points * L_k`;
/// the highest level when none is.
pub fn level_within_fee(basis_points: u16, spend_cost: u64) -> u8 {
    (0..=MAX_PAYOUT_LEVEL)
        .find(|level| {
            u128::from(spend_cost) * BASIS_POINTS
                <= u128::from(basis_points) * u128::from(level_amount(*level))
        })
        .unwrap_or(MAX_PAYOUT_LEVEL)
}

/// The payments a month a payee can expect at `level` when it earns
/// `amount_per_block` satoshis of each pool block and the pool finds
/// `blocks_per_second` blocks a second: one a block once the amount reaches
/// the level, otherwise the share of a block's draw it wins on average,
/// `amount / L`.
pub fn expected_payments_a_month(amount_per_block: f64, level: u8, blocks_per_second: f64) -> f64 {
    let blocks_a_month = blocks_per_second * MONTH_SECONDS;
    blocks_a_month * (amount_per_block / level_amount(level) as f64).min(1.0)
}

/// The highest level at or below `chosen` at which a payee earning
/// `amount_per_block` of each of `blocks_per_second` pool blocks a second
/// expects at least [`MIN_PAYMENTS_PER_MONTH`] payments a month, or level 0
/// when none does.
pub fn affordable_payout_level(chosen: u8, amount_per_block: f64, blocks_per_second: f64) -> u8 {
    (0..=chosen)
        .rev()
        .find(|level| {
            expected_payments_a_month(amount_per_block, *level, blocks_per_second)
                >= MIN_PAYMENTS_PER_MONTH
        })
        .unwrap_or(0)
}

/// What a miner hashing at `miner_hashrate` earns of a pool hashing at
/// `pool_hashrate` on blocks of `block_bits` and `coinbase_value` satoshis:
/// its satoshis a block and the pool's blocks a second. `None` without
/// hashrates to estimate from.
pub fn miner_earnings(
    miner_hashrate: f64,
    pool_hashrate: f64,
    block_bits: CompactTarget,
    coinbase_value: u64,
) -> Option<(f64, f64)> {
    if miner_hashrate <= 0.0 || pool_hashrate <= 0.0 || expected_hashes(block_bits) <= 0.0 {
        return None;
    }
    let share = (miner_hashrate / pool_hashrate).min(1.0);
    Some((
        share * coinbase_value as f64,
        pool_blocks_per_second(pool_hashrate, block_bits),
    ))
}

/// The payout level a miner's job on `prepared` carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MinerPayoutLevel {
    /// The level the job commits.
    pub level: u8,
    /// The level the miner asked for, or the node's, when the policy lowered
    /// it to `level`.
    pub lowered_from: Option<u8>,
}

/// The payout level of the next job for a miner who chose `choice` at login
/// and hashes at `miner_hashrate`, hashes a second, on `prepared`: the
/// chosen level, or the node's without a choice, lowered while the miner
/// would expect fewer than [`MIN_PAYMENTS_PER_MONTH`] payments a month
/// ([`affordable_payout_level`]). Without the miner's or the pool's hashrate
/// the level is not lowered.
pub(crate) fn miner_payout_level(
    prepared: &PreparedNotifyParams,
    choice: Option<PayoutLevelChoice>,
    miner_hashrate: Option<f64>,
) -> MinerPayoutLevel {
    let chosen = choice.map_or(prepared.payout_level(), |choice| {
        choice.level(prepared.spend_cost())
    });
    let template = prepared.template();
    let earnings = miner_hashrate
        .zip(prepared.pool_hashrate())
        .zip(CompactTarget::from_unprefixed_hex(&template.bits).ok())
        .and_then(|((miner_hashrate, pool_hashrate), block_bits)| {
            miner_earnings(
                miner_hashrate,
                pool_hashrate,
                block_bits,
                template.coinbasevalue,
            )
        });
    let level = earnings.map_or(chosen, |(amount_per_block, blocks_per_second)| {
        affordable_payout_level(chosen, amount_per_block, blocks_per_second)
    });
    MinerPayoutLevel {
        level,
        lowered_from: (level < chosen).then_some(chosen),
    }
}

/// The payout level of the next job of the miner behind `session` on
/// `prepared`, its hashrate `miner_hashrate` ([`miner_payout_level`]). The
/// first time the policy lowers the level in a session, a warning says so.
pub(crate) fn session_payout_level<D: DifficultyAdjusterTrait>(
    session: &mut Session<D>,
    prepared: &PreparedNotifyParams,
    miner_hashrate: Option<f64>,
) -> u8 {
    let miner_level = miner_payout_level(prepared, session.payout_level, miner_hashrate);
    if let Some(chosen) = miner_level.lowered_from
        && !session.payout_level_warned
    {
        session.payout_level_warned = true;
        warn!(
            "{} would expect fewer than {MIN_PAYMENTS_PER_MONTH} payments a month at payout \
             level {chosen} ({} satoshis); it mines at level {} ({} satoshis) instead",
            session.username.as_deref().unwrap_or("A miner"),
            level_amount(chosen),
            miner_level.level,
            level_amount(miner_level.level)
        );
    }
    miner_level.level
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pool that finds one block a day.
    const ONE_BLOCK_A_DAY: f64 = 1.0 / 86_400.0;

    #[test]
    fn min_payout_takes_the_smallest_level_at_or_above_it() {
        assert_eq!(PayoutLevelChoice::MinPayout(10_000).level(148), 5);
    }

    #[test]
    fn min_payout_at_a_level_takes_that_level() {
        assert_eq!(PayoutLevelChoice::MinPayout(17_472).level(148), 5);
    }

    #[test]
    fn min_payout_of_zero_takes_level_zero() {
        assert_eq!(PayoutLevelChoice::MinPayout(0).level(148), 0);
    }

    #[test]
    fn min_payout_above_the_highest_level_is_capped_at_level_seven() {
        assert_eq!(PayoutLevelChoice::MinPayout(1_000_000).level(148), 7);
    }

    /// 0.85% of a payment at 148 satoshis needs 17,412 satoshis: level 5.
    #[test]
    fn fee_tolerance_takes_the_smallest_level_within_it() {
        assert_eq!(PayoutLevelChoice::FeeTolerance(85).level(148), 5);
    }

    /// 148 / 546 is 27.1%, so a tolerance of 30% is met at level 0.
    #[test]
    fn a_wide_fee_tolerance_takes_level_zero() {
        assert_eq!(PayoutLevelChoice::FeeTolerance(3_000).level(148), 0);
    }

    /// No level keeps the spend within 0.1%: 148 / 69,888 is 0.21%.
    #[test]
    fn a_fee_tolerance_no_level_meets_takes_the_highest() {
        assert_eq!(PayoutLevelChoice::FeeTolerance(10).level(148), 7);
    }

    #[test]
    fn the_fee_tolerance_follows_the_nodes_spend_cost() {
        assert_eq!(PayoutLevelChoice::FeeTolerance(85).level(296), 6);
    }

    /// A payee earning its level in every block is paid every block.
    #[test]
    fn a_payee_at_its_level_is_paid_every_block() {
        let payments = expected_payments_a_month(20_000.0, 5, ONE_BLOCK_A_DAY);

        assert!((payments - 30.0).abs() < 1e-9, "{payments}");
    }

    /// Earning a tenth of its level a block, it is paid about every tenth
    /// block.
    #[test]
    fn a_payee_below_its_level_is_paid_in_proportion() {
        let payments = expected_payments_a_month(1_747.2, 5, ONE_BLOCK_A_DAY);

        assert!((payments - 3.0).abs() < 1e-9, "{payments}");
    }

    /// At 1,000 satoshis a block and a block a day, level 5 gives 1.7
    /// payments a month and level 3 gives 6.9: level 4 gives 3.4, the
    /// highest with at least 3.
    #[test]
    fn a_level_with_too_few_payments_is_lowered_until_they_reach_three() {
        assert_eq!(affordable_payout_level(5, 1_000.0, ONE_BLOCK_A_DAY), 4);
    }

    #[test]
    fn a_level_with_enough_payments_is_kept() {
        assert_eq!(affordable_payout_level(5, 20_000.0, ONE_BLOCK_A_DAY), 5);
    }

    /// A payee too small for 3 payments a month even at level 0 gets level 0.
    #[test]
    fn a_payee_too_small_for_any_level_gets_level_zero() {
        assert_eq!(affordable_payout_level(5, 1.0, ONE_BLOCK_A_DAY), 0);
    }

    /// A miner with a tenth of the pool's hashrate earns a tenth of each
    /// block's coinbase value.
    #[test]
    fn a_miner_earns_its_share_of_the_pools_hashrate() {
        let (amount, blocks_per_second) = miner_earnings(
            1.0,
            10.0,
            CompactTarget::from_consensus(0x1d00_ffff),
            1_000_000,
        )
        .unwrap();

        assert!((amount - 100_000.0).abs() < 1e-6, "{amount}");
        assert!(blocks_per_second > 0.0);
    }

    #[test]
    fn no_earnings_without_a_miner_hashrate() {
        assert!(
            miner_earnings(
                0.0,
                10.0,
                CompactTarget::from_consensus(0x1d00_ffff),
                1_000_000
            )
            .is_none()
        );
    }
}
