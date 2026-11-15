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
//! The minimum can only ease as far as the chain's ceiling, so no miner gets
//! more than the room left: as many times the minimum's work as the minimum
//! is times the ceiling's. At the ceiling that is one, and every miner is at
//! the minimum. A pool too small for the ceiling's 10-second shares then
//! keeps every share it can make, where heavier shares would only slow its
//! chain further and ease no one's minimum, and the room shrinks smoothly to
//! nothing as a growing pool's minimum nears the ceiling, so ASERT settles
//! on one minimum instead of switching heavier jobs on and off around it.
//!
//! A miner at the minimum that would find fewer than 16 weak shares a month
//! at its job's weak target asks for an easier one (part 6 of the design
//! page, "Node default"): its job's `wanted_bits` is the target at which it
//! would find 16 a month, never easier than the chain's ceiling. The weak
//! shares it finds then carry that ask, and the weak ratio rises while more
//! than one carried weak share in 64 asks (`shares::weak::next_weak_ratio`).
//! A miner on a heavier job, or one the band cap holds at the minimum, makes
//! no ask: it wanted a harder target.
//!
//! Hashes are counted as difficulty times 2^32, as the metrics actor counts a
//! miner's submissions.

use super::prepared_notify::PreparedNotifyParams;
use crate::pool_difficulty::{U512, target_to_u512, u512_to_target};
use crate::shares::band::{BAND_CAP_UNIT, JobBand, heavier_job_band};
use crate::shares::chain::chain_store_handle::ConfirmedHeaderResult;
use bitcoin::{CompactTarget, Target};

/// Shares a pool block a miner may make before it gets heavier shares.
pub const SHARES_PER_POOL_BLOCK: f64 = 4.0;

/// Shares a month a miner may make before it gets heavier shares, whatever
/// the pool's block rate.
pub const SHARES_PER_MONTH: f64 = 300.0;

/// Weak shares a month below which a miner at the minimum asks for an
/// easier weak target, and which the target it asks for would give it. Past
/// about that many, more weak shares barely change what a small miner is
/// paid.
pub const ASK_WEAK_SHARES_PER_MONTH: f64 = 16.0;

/// Seconds in the month [`SHARES_PER_MONTH`] and
/// [`ASK_WEAK_SHARES_PER_MONTH`] count: 30 days.
pub const MONTH_SECONDS: f64 = 30.0 * 24.0 * 60.0 * 60.0;

/// Confirmed shares the pool hashrate is estimated from: ten minutes of
/// 10-second shares.
pub const POOL_HASHRATE_SHARES: u32 = 60;

/// Hashes one unit of difficulty takes on average.
const HASHES_PER_DIFFICULTY: f64 = 4_294_967_296.0;

/// The hashes a target takes on average: its difficulty times 2^32.
pub(crate) fn expected_hashes(bits: CompactTarget) -> f64 {
    expected_target_hashes(Target::from_compact(bits))
}

/// [`expected_hashes`] of a target no compact form need write, as a weak
/// target is.
fn expected_target_hashes(target: Target) -> f64 {
    target.difficulty_float() * HASHES_PER_DIFFICULTY
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

/// How many times the minimum's work a heavier job may be before the
/// minimum's room to ease runs out: the minimum's work over the `ceiling`'s,
/// at least one, or unbounded without a ceiling.
fn room_above_ceiling(minimum: CompactTarget, ceiling: Option<CompactTarget>) -> f64 {
    ceiling.map_or(f64::INFINITY, |ceiling| {
        (expected_hashes(minimum) / expected_hashes(ceiling)).max(1.0)
    })
}

/// The band of the next job for a miner hashing at `miner_hashrate`, hashes
/// a second, on `prepared`: `None`, the minimum, unless the miner would make
/// more shares at the minimum than `prepared`'s share rate limit; then the
/// multiple of the minimum's work that brings it down to that rate, at most
/// the minimum's room above the chain's ceiling ([`room_above_ceiling`]) and
/// capped at the band ([`heavier_job_band`]). The room bounds what the node
/// wants, so a job held back by it does not push the band cap up.
///
/// A miner that stays at the minimum and would find too few weak shares
/// there gets the minimum with a wanted target easier than its weak target,
/// which asks for an easier one ([`asking_job_band`]). A miner whose
/// hashrate is not known yet gets the minimum and asks for nothing.
pub(crate) fn miner_job_band(
    prepared: &PreparedNotifyParams,
    miner_hashrate: Option<f64>,
) -> Option<JobBand> {
    let hashrate = miner_hashrate.filter(|hashrate| *hashrate > 0.0)?;
    match heavier_multiple(prepared, hashrate) {
        Some(wanted_multiple) => Some(heavier_job_band(
            prepared.bits(),
            prepared.band_cap(),
            wanted_multiple,
        )),
        None => asking_job_band(prepared, hashrate),
    }
}

/// The multiple of the minimum's work, in units of 1/1024, the node wants
/// for a miner hashing at `hashrate` on `prepared`: its share rate at the
/// minimum over `prepared`'s share rate limit, at most the minimum's room
/// above the chain's ceiling. `None` when that is no more than one, or
/// without a share rate limit: the miner stays at the minimum.
fn heavier_multiple(prepared: &PreparedNotifyParams, hashrate: f64) -> Option<u64> {
    let limit = prepared.share_rate_limit()?;
    let share_rate = hashrate / expected_hashes(prepared.bits());
    let multiple = (share_rate / limit).min(room_above_ceiling(
        prepared.bits(),
        prepared.target_ceiling(),
    ));
    //* `as` saturates, so a huge multiple stays at u64::MAX.
    (multiple > 1.0).then(|| (multiple * f64::from(BAND_CAP_UNIT)) as u64)
}

/// The band of a job at the minimum whose miner, hashing at `hashrate`,
/// would find fewer than [`ASK_WEAK_SHARES_PER_MONTH`] weak shares a month at
/// the job's weak target: the minimum, wanting the target at which the miner
/// would find that many, never easier than the chain's ceiling.
///
/// `None`, a job that asks for nothing, when the miner finds enough, when
/// `prepared` has no weak target or no ceiling to hold the ask to, or when
/// the wanted target is not easier than the weak target, as at the ceiling.
fn asking_job_band(prepared: &PreparedNotifyParams, hashrate: f64) -> Option<JobBand> {
    let weak_target = prepared.weak_target()?;
    let ceiling = prepared.target_ceiling()?;
    let weak_shares_per_month = hashrate * MONTH_SECONDS / expected_target_hashes(weak_target);
    if weak_shares_per_month >= ASK_WEAK_SHARES_PER_MONTH {
        return None;
    }
    let wanted_bits = easier_target(
        weak_target,
        ASK_WEAK_SHARES_PER_MONTH / weak_shares_per_month,
        ceiling,
    );
    (Target::from_compact(wanted_bits) > weak_target).then(|| JobBand {
        bits: prepared.bits(),
        wanted_bits,
    })
}

/// The target `multiple` times easier than `target`, at most `ceiling`,
/// written as the compact target at or below it, so the compact form is
/// never easier than the ceiling. Hashes meet a target in proportion to it,
/// so a miner finds `multiple` times as many proofs at the result.
fn easier_target(target: Target, multiple: f64, ceiling: CompactTarget) -> CompactTarget {
    //* `as` saturates, so a huge multiple stays at u64::MAX, and a 256-bit
    //* target times it stays within 512 bits.
    let multiple = (multiple * f64::from(BAND_CAP_UNIT)) as u64;
    let scaled = target_to_u512(target) * U512::from(multiple) / U512::from(BAND_CAP_UNIT);
    let ceiling = target_to_u512(Target::from_compact(ceiling));
    let capped = if scaled > ceiling { ceiling } else { scaled };
    u512_to_target(capped).to_compact_lossy()
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

    /// A tiny pool: the minimum is at the ceiling, so the chain already
    /// makes shares slower than it should and the minimum cannot ease. A
    /// miner far above the share rate limit still mines at the minimum.
    #[test]
    fn a_miner_at_the_ceiling_gets_the_minimum() {
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
        .bits(CompactTarget::from_consensus(0x1c40_0000))
        .band_cap(65_536)
        .share_rate_limit(Some(1.0))
        .target_ceiling(Some(CompactTarget::from_consensus(0x1c40_0000)))
        .build()
        .unwrap();
        let minimum_hashes = expected_hashes(CompactTarget::from_consensus(0x1c40_0000));

        assert_eq!(
            miner_job_band(&prepared, Some(1_000.0 * minimum_hashes)),
            None
        );
    }

    /// The minimum at twice the ceiling's work can ease twice before the
    /// ceiling, so a miner that would want eight times the minimum's work
    /// gets two, and wants no more: its share does not push the band cap up.
    #[test]
    fn a_miner_near_the_ceiling_gets_only_the_room_left() {
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
        .bits(CompactTarget::from_consensus(0x1c40_0000))
        .band_cap(65_536)
        .share_rate_limit(Some(1.0))
        .target_ceiling(Some(CompactTarget::from_consensus(0x1c7f_ffff)))
        .build()
        .unwrap();
        let minimum_hashes = expected_hashes(CompactTarget::from_consensus(0x1c40_0000));

        let band = miner_job_band(&prepared, Some(8.0 * minimum_hashes)).unwrap();

        let room = room_above_ceiling(
            CompactTarget::from_consensus(0x1c40_0000),
            Some(CompactTarget::from_consensus(0x1c7f_ffff)),
        );
        assert!((room - 2.0).abs() < 1e-6, "{room}");
        assert_eq!(
            band,
            heavier_job_band(
                CompactTarget::from_consensus(0x1c40_0000),
                65_536,
                (room * f64::from(BAND_CAP_UNIT)) as u64
            )
        );
        assert_eq!(band.wanted_bits, band.bits);
    }

    //* The node default that asks. The minimum is `2^222`, the weak target
    //* four times easier, `2^224`, which takes about 2^32 hashes, and the
    //* ceiling far above both unless a test says otherwise.

    /// A miner that would find 4 weak shares a month at its job's weak
    /// target gets the minimum wanting a target four times easier, at which
    /// it would find 16. No share rate limit is needed for that.
    #[test]
    fn a_miner_finding_too_few_weak_shares_asks_for_the_target_that_gives_it_sixteen() {
        let template: BlockTemplate = serde_json::from_str(include_str!(
            "../../../../bitcoindrpc/test_data/bchn-regtest-template.json"
        ))
        .unwrap();
        let weak_target = Target::from_compact(CompactTarget::from_consensus(0x1d01_0000));
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
        .include_merge_mining(true)
        .weak_target(Some(weak_target))
        .target_ceiling(Some(CompactTarget::from_consensus(0x1e00_ffff)))
        .build()
        .unwrap();
        let hashrate = 4.0 * expected_target_hashes(weak_target) / MONTH_SECONDS;

        let band = miner_job_band(&prepared, Some(hashrate)).unwrap();

        assert_eq!(band.bits, CompactTarget::from_consensus(0x1c40_0000));
        assert!(Target::from_compact(band.wanted_bits) > weak_target);
        let weak_shares_per_month = hashrate * MONTH_SECONDS / expected_hashes(band.wanted_bits);
        assert!(
            (15.9..16.1).contains(&weak_shares_per_month),
            "{weak_shares_per_month} weak shares a month at the wanted target"
        );
    }

    /// A miner that finds 16 weak shares a month or more asks for nothing.
    #[test]
    fn a_miner_finding_enough_weak_shares_does_not_ask() {
        let template: BlockTemplate = serde_json::from_str(include_str!(
            "../../../../bitcoindrpc/test_data/bchn-regtest-template.json"
        ))
        .unwrap();
        let weak_target = Target::from_compact(CompactTarget::from_consensus(0x1d01_0000));
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
        .include_merge_mining(true)
        .weak_target(Some(weak_target))
        .target_ceiling(Some(CompactTarget::from_consensus(0x1e00_ffff)))
        .build()
        .unwrap();
        let hashrate = 17.0 * expected_target_hashes(weak_target) / MONTH_SECONDS;

        assert_eq!(miner_job_band(&prepared, Some(hashrate)), None);
    }

    /// Until a miner's hashrate is measured its node asks for nothing.
    #[test]
    fn a_miner_of_unknown_hashrate_does_not_ask() {
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
        .bits(CompactTarget::from_consensus(0x1c40_0000))
        .include_merge_mining(true)
        .weak_target(Some(Target::from_compact(CompactTarget::from_consensus(
            0x1d01_0000,
        ))))
        .target_ceiling(Some(CompactTarget::from_consensus(0x1e00_ffff)))
        .build()
        .unwrap();

        assert_eq!(miner_job_band(&prepared, None), None);
        assert_eq!(miner_job_band(&prepared, Some(0.0)), None);
    }

    /// A miner of one hash a second would need a target far easier than the
    /// chain's ceiling for 16 weak shares a month: it asks for the ceiling,
    /// here twice the weak target, and no more.
    #[test]
    fn a_miner_never_asks_for_a_target_easier_than_the_ceiling() {
        let template: BlockTemplate = serde_json::from_str(include_str!(
            "../../../../bitcoindrpc/test_data/bchn-regtest-template.json"
        ))
        .unwrap();
        let ceiling = CompactTarget::from_consensus(0x1d02_0000);
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
        .include_merge_mining(true)
        .weak_target(Some(Target::from_compact(CompactTarget::from_consensus(
            0x1d01_0000,
        ))))
        .target_ceiling(Some(ceiling))
        .build()
        .unwrap();

        let band = miner_job_band(&prepared, Some(1.0)).unwrap();

        assert_eq!(band.bits, CompactTarget::from_consensus(0x1c40_0000));
        assert_eq!(band.wanted_bits, ceiling);
    }

    /// With the weak target at the ceiling nothing is easier, so the miner
    /// that finds too few weak shares asks for nothing.
    #[test]
    fn a_miner_whose_weak_target_is_the_ceiling_does_not_ask() {
        let template: BlockTemplate = serde_json::from_str(include_str!(
            "../../../../bitcoindrpc/test_data/bchn-regtest-template.json"
        ))
        .unwrap();
        let ceiling = CompactTarget::from_consensus(0x1d01_0000);
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
        .include_merge_mining(true)
        .weak_target(Some(Target::from_compact(ceiling)))
        .target_ceiling(Some(ceiling))
        .build()
        .unwrap();

        assert_eq!(miner_job_band(&prepared, Some(1.0)), None);
    }

    /// A miner the band cap holds at the minimum wanted a harder target: its
    /// job keeps that wish and asks for nothing, though it too finds fewer
    /// than 16 weak shares a month.
    #[test]
    fn a_miner_the_cap_stopped_does_not_ask() {
        let template: BlockTemplate = serde_json::from_str(include_str!(
            "../../../../bitcoindrpc/test_data/bchn-regtest-template.json"
        ))
        .unwrap();
        let weak_target = Target::from_compact(CompactTarget::from_consensus(0x1d01_0000));
        let minimum_hashes = expected_hashes(CompactTarget::from_consensus(0x1c40_0000));
        let hashrate = 4.0 * expected_target_hashes(weak_target) / MONTH_SECONDS;
        //* A share rate limit a tenth of what the miner makes at the minimum.
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
        .band_cap(BAND_CAP_UNIT)
        .share_rate_limit(Some(hashrate / minimum_hashes / 10.0))
        .include_merge_mining(true)
        .weak_target(Some(weak_target))
        .target_ceiling(Some(CompactTarget::from_consensus(0x1e00_ffff)))
        .build()
        .unwrap();

        let band = miner_job_band(&prepared, Some(hashrate)).unwrap();

        assert_eq!(band.bits, CompactTarget::from_consensus(0x1c40_0000));
        assert!(
            Target::from_compact(band.wanted_bits) < Target::from_compact(band.bits),
            "wanted {:#010x}",
            band.wanted_bits.to_consensus()
        );
    }

    /// Far from the ceiling the room does not bind: the miner gets the
    /// multiple its share rate asks for.
    #[test]
    fn a_miner_far_from_the_ceiling_gets_its_full_multiple() {
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
        .bits(CompactTarget::from_consensus(0x1c40_0000))
        .band_cap(4096)
        .share_rate_limit(Some(1.0))
        .target_ceiling(Some(CompactTarget::from_consensus(0x1d00_ffff)))
        .build()
        .unwrap();
        let minimum_hashes = expected_hashes(CompactTarget::from_consensus(0x1c40_0000));

        let band = miner_job_band(&prepared, Some(2.0 * minimum_hashes)).unwrap();

        assert_eq!(band.bits, CompactTarget::from_consensus(0x1c20_0000));
    }
}
