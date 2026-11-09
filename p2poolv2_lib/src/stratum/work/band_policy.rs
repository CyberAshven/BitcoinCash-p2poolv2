// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The node's default target for each miner's jobs on Bitcoin Cash
//! (`docs/sharechain/small-miners-design.md`, part 2, "Node default"). Node
//! policy, not a share-chain rule: the band allows heavier shares, and
//! nothing forces them.
//!
//! A node gives each miner the minimum target unless the miner would then
//! make more than `max(4 shares per pool block, 300 shares a month)`. Such a
//! miner gets a harder target, up to the band's `C` times the minimum, sized
//! so it makes about that many; `wanted_bits` records the target it would
//! have had without the cap. Big miners then make fewer, heavier shares, the
//! pool makes fewer shares in all, and ASERT lowers the minimum for everyone
//! else.
//!
//! Hashes are counted as difficulty times 2^32, as the metrics actor counts a
//! miner's submissions.

use super::prepared_notify::PreparedNotifyParams;
use crate::shares::band::{BAND_CAP_UNIT, JobBand, heavier_job_band};
use crate::shares::chain::chain_store_handle::ConfirmedHeaderResult;
use bitcoin::{CompactTarget, Target};

/// Shares a pool block a miner may make before it gets heavier shares.
pub const SHARES_PER_POOL_BLOCK: f64 = 4.0;

/// Shares a month a miner may make before it gets heavier shares, whatever
/// the pool's block rate.
pub const SHARES_PER_MONTH: f64 = 300.0;

/// Seconds in the month [`SHARES_PER_MONTH`] counts: 30 days.
pub const MONTH_SECONDS: f64 = 30.0 * 24.0 * 60.0 * 60.0;

/// Confirmed shares the pool hashrate is estimated from: ten minutes of
/// 10-second shares.
pub const POOL_HASHRATE_SHARES: u32 = 60;

/// Hashes one unit of difficulty takes on average.
const HASHES_PER_DIFFICULTY: f64 = 4_294_967_296.0;

/// The hashes a target takes on average: its difficulty times 2^32.
pub(crate) fn expected_hashes(bits: CompactTarget) -> f64 {
    Target::from_compact(bits).difficulty_float() * HASHES_PER_DIFFICULTY
}

/// The pool's blocks a second at `pool_hashrate`, hashes a second, when a
/// block takes `block_bits`' expected hashes.
pub fn pool_blocks_per_second(pool_hashrate: f64, block_bits: CompactTarget) -> f64 {
    pool_hashrate / expected_hashes(block_bits)
}

/// The pool's hashrate, in hashes a second, behind `headers`, the newest
/// confirmed shares in any order: their work over the time from the oldest
/// to the newest. The oldest share only opens the span; its work was done
/// before it. `None` with fewer than two shares or no time between them.
pub fn recent_pool_hashrate(headers: &[ConfirmedHeaderResult]) -> Option<f64> {
    let newest = headers.iter().max_by_key(|result| result.header.time)?;
    let oldest = headers.iter().min_by_key(|result| result.header.time)?;
    let span = newest.header.time.checked_sub(oldest.header.time)?;
    if span == 0 {
        None
    } else {
        let work: f64 = headers
            .iter()
            .map(|result| expected_hashes(result.header.bits))
            .sum();
        Some((work - expected_hashes(oldest.header.bits)) / f64::from(span))
    }
}

/// The share rate, in shares a second, above which a miner gets heavier
/// shares: `max(4 shares per pool block, 300 shares a month)`, a pool block
/// taking `block_bits`' expected hashes at `pool_hashrate`.
pub(crate) fn share_rate_limit(pool_hashrate: f64, block_bits: CompactTarget) -> f64 {
    (SHARES_PER_POOL_BLOCK * pool_blocks_per_second(pool_hashrate, block_bits))
        .max(SHARES_PER_MONTH / MONTH_SECONDS)
}

/// The band of the next job for a miner hashing at `miner_hashrate`, hashes
/// a second, on `prepared`: `None`, the minimum, unless the miner would make
/// more shares at the minimum than `prepared`'s share rate limit; then the
/// multiple of the minimum's work that brings it down to that rate, capped
/// at the band ([`heavier_job_band`]).
pub(crate) fn miner_job_band(
    prepared: &PreparedNotifyParams,
    miner_hashrate: Option<f64>,
) -> Option<JobBand> {
    let limit = prepared.share_rate_limit()?;
    let hashrate = miner_hashrate.filter(|hashrate| *hashrate > 0.0)?;
    let share_rate = hashrate / expected_hashes(prepared.bits());
    if share_rate <= limit {
        None
    } else {
        //* `as` saturates, so a huge multiple stays at u64::MAX.
        let wanted_multiple = (share_rate / limit * f64::from(BAND_CAP_UNIT)) as u64;
        Some(heavier_job_band(
            prepared.bits(),
            prepared.band_cap(),
            wanted_multiple,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounting::OutputPair;
    use crate::stratum::work::block_template::BlockTemplate;
    use crate::stratum::work::prepared_notify::PreparedNotifyParamsBuilder;
    use crate::test_utils::TestShareBlockBuilder;
    use bitcoin::hashes::Hash;
    use bitcoin::{Amount, BlockHash};
    use std::sync::Arc;

    /// The pool makes 4 shares a pool block at 4 times the block rate, when
    /// that is above 300 a month.
    #[test]
    fn the_share_rate_limit_is_four_shares_a_pool_block() {
        // A block takes difficulty 1, 2^32 hashes; the pool does 2^32 a
        // second, a block a second.
        let limit = share_rate_limit(
            HASHES_PER_DIFFICULTY,
            CompactTarget::from_consensus(0x1d00_ffff),
        );

        assert!((limit - 4.0).abs() < 1e-6, "{limit}");
    }

    /// A pool that finds a block a year still lets each miner make 300
    /// shares a month at the minimum.
    #[test]
    fn the_share_rate_limit_is_at_least_three_hundred_shares_a_month() {
        let limit = share_rate_limit(1.0, CompactTarget::from_consensus(0x1d00_ffff));

        assert_eq!(limit, SHARES_PER_MONTH / MONTH_SECONDS);
    }

    #[test]
    fn the_pool_hashrate_leaves_out_the_oldest_shares_work() {
        let older = TestShareBlockBuilder::new()
            .bits(CompactTarget::from_consensus(0x1d00_ffff))
            .time(1_000)
            .build()
            .header;
        let newer = TestShareBlockBuilder::new()
            .bits(CompactTarget::from_consensus(0x1d00_ffff))
            .time(1_010)
            .build()
            .header;
        let headers = vec![
            ConfirmedHeaderResult {
                height: 2,
                blockhash: newer.block_hash(),
                header: newer,
            },
            ConfirmedHeaderResult {
                height: 1,
                blockhash: older.block_hash(),
                header: older,
            },
        ];

        let hashrate = recent_pool_hashrate(&headers).unwrap();

        assert!(
            (hashrate - HASHES_PER_DIFFICULTY / 10.0).abs() < 1.0,
            "{hashrate}"
        );
    }

    #[test]
    fn the_pool_hashrate_needs_two_shares() {
        let only = TestShareBlockBuilder::new().time(1_000).build().header;
        let headers = vec![ConfirmedHeaderResult {
            height: 1,
            blockhash: only.block_hash(),
            header: only,
        }];

        assert_eq!(recent_pool_hashrate(&headers), None);
    }

    #[test]
    fn a_job_without_a_share_rate_limit_is_at_the_minimum() {
        let template: BlockTemplate = serde_json::from_str(include_str!(
            "../../../../bitcoindrpc/test_data/bchn-regtest-template.json"
        ))
        .unwrap();
        let prepared = PreparedNotifyParamsBuilder::new(
            Arc::new(template),
            vec![OutputPair {
                address: crate::test_utils::make_test_address(1),
                amount: Amount::from_sat(1),
            }],
            b"test",
            false,
        )
        .prev_share_blockhash(BlockHash::from_byte_array([1; 32]))
        .bits(CompactTarget::from_consensus(0x1c38_4bd7))
        .build()
        .unwrap();

        assert_eq!(miner_job_band(&prepared, Some(1e18)), None);
    }

    #[test]
    fn a_miner_below_the_share_rate_limit_gets_the_minimum() {
        let template: BlockTemplate = serde_json::from_str(include_str!(
            "../../../../bitcoindrpc/test_data/bchn-regtest-template.json"
        ))
        .unwrap();
        // Minimum difficulty 1: 2^32 hashes a share. A limit of one share a
        // second and a miner doing half of 2^32 hashes a second.
        let prepared = PreparedNotifyParamsBuilder::new(
            Arc::new(template),
            vec![OutputPair {
                address: crate::test_utils::make_test_address(1),
                amount: Amount::from_sat(1),
            }],
            b"test",
            false,
        )
        .prev_share_blockhash(BlockHash::from_byte_array([1; 32]))
        .bits(CompactTarget::from_consensus(0x1d00_ffff))
        .band_cap(4096)
        .share_rate_limit(Some(1.0))
        .build()
        .unwrap();

        assert_eq!(
            miner_job_band(&prepared, Some(HASHES_PER_DIFFICULTY / 2.0)),
            None
        );
    }

    #[test]
    fn a_miner_above_the_share_rate_limit_gets_a_heavier_job_sized_to_the_limit() {
        let template: BlockTemplate = serde_json::from_str(include_str!(
            "../../../../bitcoindrpc/test_data/bchn-regtest-template.json"
        ))
        .unwrap();
        // Two shares a second at the minimum against a limit of one: twice
        // the minimum's work, inside a band of four.
        let prepared = PreparedNotifyParamsBuilder::new(
            Arc::new(template),
            vec![OutputPair {
                address: crate::test_utils::make_test_address(1),
                amount: Amount::from_sat(1),
            }],
            b"test",
            false,
        )
        .prev_share_blockhash(BlockHash::from_byte_array([1; 32]))
        .bits(CompactTarget::from_consensus(0x1c40_0000))
        .band_cap(4096)
        .share_rate_limit(Some(1.0))
        .build()
        .unwrap();
        let minimum_hashes = expected_hashes(CompactTarget::from_consensus(0x1c40_0000));

        let band = miner_job_band(&prepared, Some(2.0 * minimum_hashes)).unwrap();

        assert_eq!(
            band,
            heavier_job_band(CompactTarget::from_consensus(0x1c40_0000), 4096, 2048)
        );
        assert_eq!(band.bits, CompactTarget::from_consensus(0x1c20_0000));
    }

    #[test]
    fn a_miner_past_the_cap_mines_at_the_cap_and_wants_its_own_target() {
        let template: BlockTemplate = serde_json::from_str(include_str!(
            "../../../../bitcoindrpc/test_data/bchn-regtest-template.json"
        ))
        .unwrap();
        // Eight shares a second at the minimum against a limit of one, in a
        // band of four.
        let prepared = PreparedNotifyParamsBuilder::new(
            Arc::new(template),
            vec![OutputPair {
                address: crate::test_utils::make_test_address(1),
                amount: Amount::from_sat(1),
            }],
            b"test",
            false,
        )
        .prev_share_blockhash(BlockHash::from_byte_array([1; 32]))
        .bits(CompactTarget::from_consensus(0x1c40_0000))
        .band_cap(4096)
        .share_rate_limit(Some(1.0))
        .build()
        .unwrap();
        let minimum_hashes = expected_hashes(CompactTarget::from_consensus(0x1c40_0000));

        let band = miner_job_band(&prepared, Some(8.0 * minimum_hashes)).unwrap();

        assert_eq!(band.bits, CompactTarget::from_consensus(0x1c10_0000));
        assert_eq!(band.wanted_bits, CompactTarget::from_consensus(0x1c08_0000));
    }
}
