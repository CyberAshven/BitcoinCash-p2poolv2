// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Bitcoin Cash's own block difficulty, aserti3-2d, and the header check
//! P2Pool runs with it on merge-mining proofs and token shares.
//!
//! The arithmetic follows `CalculateASERT` and `GetNextASERTWorkRequired` in
//! Bitcoin Cash Node (`src/pow.cpp`) integer for integer: the exponent is a
//! truncating 64-bit division, its integer part an arithmetic (flooring)
//! shift, the fraction goes through BCHN's cubic, and the result is clamped
//! to the proof-of-work limit and to at least 1. The per-network anchors and
//! half-lives are BCHN's chain parameters (`src/chainparams.cpp`,
//! `asertAnchorParams` and `nASERTHalfLife`). The tests run the aserti3-2d
//! specification's twelve vector runs and real mainnet and chipnet headers.
//!
//! CHIP-2025-03 counts block time in ticks: ASERT reads the parent's ticks
//! and the new block's interval ([`TickInfo`]), which each known header
//! carries (`shares::bch_headers::KnownBchHeader`) and [`TickSchedule`] works
//! out from a block's median time past, as BCHN's `GetPowTargetSpacing`
//! does. The tests also run the CHIP's ASERT and tick vectors.
//!
//! The header check is a format check, not block validation: P2Pool has
//! only the header and its coinbase height, not the parent block, so the
//! header's own time stands in for its parent's and the computed target gets
//! a band (see [`BchAsert::check_header_bits`]). BCHN stays the authority on
//! whether a block is valid.

use crate::config::BitcoinCashNetwork;
use bitcoin::CompactTarget;

// The generated arithmetic trips these lints inside the external macro body,
// where they cannot be fixed, so the allow wraps the invocation in a module.
mod u256 {
    #![allow(clippy::manual_div_ceil, clippy::assign_op_pattern)]

    uint::construct_uint! {
        /// BCHN's `arith_uint256`: the unsigned 256-bit integer its
        /// difficulty arithmetic works in.
        pub struct U256(4);
    }
}
pub use u256::U256;

/// The interval, in ticks (seconds of target time), of every Bitcoin Cash
/// block before a network's first interval switch, and of the ASERT anchor
/// block on every network: 600.
pub const BCH_BLOCK_SPACING: i64 = 600;

/// The blocks a median time past reads: a block and its ten ancestors
/// (`nMedianTimeSpan` in BCHN's `src/chain.h`).
pub const MEDIAN_TIME_SPAN: usize = 11;

/// The most switches pinned to blocks that one schedule holds: one-minute
/// blocks, and room for later switches.
const MAX_SWITCHES: usize = 4;

/// A change of a network's block interval by time (CHIP-2025-03): a block
/// whose median time past is at or after `activation_time` has a next block
/// of `interval` ticks, as BCHN's `IsUpgrade2027Enabled` decides it.
///
/// The CHIP keeps a switch on the 600-tick grid: the block a switch follows
/// has ticks that are a multiple of 600. A switch from ten-minute blocks
/// always is, since every block's ticks are `600 * height` before it. A later
/// switch from shorter blocks needs an activation that keeps to the grid,
/// which its upgrade defines; it is added here with that rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimedSwitch {
    /// Median time past at which the switch activates.
    pub activation_time: i64,
    /// The interval, in ticks, of a block built on one that reached it.
    pub interval: i64,
}

/// One-minute blocks on mainnet, testnet3, testnet4, scalenet and regtest:
/// 2027-05-15 12:00 UTC, BCHN's `upgrade2027ActivationTime`.
const MAINNET_SWITCHES: &[TimedSwitch] = &[TimedSwitch {
    activation_time: 1_810_382_400,
    interval: 60,
}];

/// One-minute blocks on chipnet: 2026-11-15 12:00 UTC.
const CHIPNET_SWITCHES: &[TimedSwitch] = &[TimedSwitch {
    activation_time: 1_794_744_000,
    interval: 60,
}];

/// A switch pinned to a block: every block after `height` has `interval`
/// ticks, until the next switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntervalSwitch {
    /// The last block with the old interval.
    pub height: u32,
    /// The interval, in ticks, of every block after it.
    pub interval: i64,
}

/// Why an interval switch cannot join a schedule.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScheduleError {
    /// CHIP-2025-03 allows intervals from 1 to 600 ticks that divide 600.
    #[error("interval {0} does not divide 600")]
    Interval(i64),
    /// Switches come in ascending height order.
    #[error("switch at height {height} is not above the last switch, at {last}")]
    Order { height: u32, last: u32 },
    /// A switch sits where a block's ticks are a multiple of 600.
    #[error("switch at height {height} has ticks {ticks}, not a multiple of 600")]
    Grid { height: u32, ticks: i64 },
    /// The schedule is full.
    #[error("a schedule holds at most {MAX_SWITCHES} switches")]
    Full,
}

/// A block's place in CHIP-2025-03's ticks, as BCHN keeps it in its block
/// index (`CBlockIndex::tickInfo`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TickInfo {
    /// The block's ticks: the sum of the target intervals of the blocks up
    /// to it. The chain's first block has none.
    pub ticks: i64,
    /// The target interval, in ticks, of a block built on it.
    pub next_interval: i64,
}

impl TickInfo {
    /// The ticks of a block built on this one (BCHN's `GetNextBlockTicks`).
    pub fn next_ticks(&self) -> i64 {
        self.ticks + self.next_interval
    }

    /// How long after this block a test network block built on it may take
    /// the proof-of-work limit: more than twice the new block's interval, 20
    /// minutes for a ten-minute block and 2 for a one-minute one.
    pub fn min_difficulty_gap(&self) -> u32 {
        (2 * self.next_interval) as u32
    }

    /// Whether a coinbase in a block with `coin_ticks` can be spent in a
    /// block built on this one: that block is at least
    /// [`BCH_COINBASE_MATURITY_TICKS`] past the coinbase's, as BCHN's
    /// `CheckTxInputs` requires.
    pub fn coinbase_spendable_in_next_block(&self, coin_ticks: i64) -> bool {
        self.next_ticks() - coin_ticks >= BCH_COINBASE_MATURITY_TICKS
    }
}

/// BCHN's `GetMedianTimePast` over `times`: the middle one once sorted.
/// `None` for no times.
pub fn median_time_past(times: &[u32]) -> Option<i64> {
    let mut sorted = times.to_vec();
    sorted.sort_unstable();
    sorted.get(sorted.len() / 2).map(|time| i64::from(*time))
}

/// A network's block intervals under CHIP-2025-03.
///
/// A block's ticks are the sum of the target intervals of the blocks up to
/// it. Every block has 600 ticks until the first switch. A switch is timed
/// ([`TimedSwitch`]): a block whose own median time past (it and its ten
/// ancestors) has reached the switch's time has a next block of the
/// switch's interval, as BCHN decides it for each block it adds to its
/// index. Each known header carries the ticks and next interval worked out
/// that way (`shares::bch_headers::KnownBchHeader`), so every node that knows
/// a branch gives its blocks the same ticks, and a reorg onto a branch that
/// switched at another block follows that branch.
///
/// The schedule also pins switches to the blocks they follow, where a
/// network's switch has happened: chipnet's last ten-minute block is
/// 332,450. A pin answers by height where no median time past can be worked
/// out (a header with fewer than ten known ancestors, such as the oldest of a
/// new store) or no header is known at all; the tests check that the timed
/// rule switches at the pinned block. Before any switch a block's ticks are
/// `600 * height`, so every rule counted in ticks is the old rule there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TickSchedule {
    /// The switches by time, in activation order.
    timed: &'static [TimedSwitch],
    /// The switches pinned to blocks, in height order.
    pinned: [Option<IntervalSwitch>; MAX_SWITCHES],
}

impl TickSchedule {
    /// Ten-minute blocks only: no switch.
    pub const TEN_MINUTES: Self = Self {
        timed: &[],
        pinned: [None; MAX_SWITCHES],
    };

    /// Mainnet (also testnet3, testnet4, scalenet and regtest, as in BCHN):
    /// one-minute blocks from 2027-05-15 12:00 UTC, no block pinned yet.
    /// The header gate checks token shares by the pins, so pinning the
    /// switch block changes its verdicts: a share-chain rule change, with a
    /// new protocol segment, never a quiet update. The SAFA covenant
    /// (`p2poolv2_safa`) is compiled with the same pin, so it is pinned there
    /// in the same change.
    const MAINNET: Self = Self {
        timed: MAINNET_SWITCHES,
        pinned: [None; MAX_SWITCHES],
    };

    /// Chipnet: one-minute blocks from 2026-11-15 12:00 UTC, after block
    /// 332,450, whose median time past, 1,794,744,504, was the first at or
    /// after it.
    const CHIPNET: Self = Self {
        timed: CHIPNET_SWITCHES,
        pinned: [
            Some(IntervalSwitch {
                height: 332_450,
                interval: 60,
            }),
            None,
            None,
            None,
        ],
    };

    /// The schedule of `network`.
    pub fn for_network(network: BitcoinCashNetwork) -> Self {
        match network {
            BitcoinCashNetwork::Chipnet => Self::CHIPNET,
            BitcoinCashNetwork::Mainnet
            | BitcoinCashNetwork::Regtest
            | BitcoinCashNetwork::Scalenet => Self::MAINNET,
        }
    }

    /// This schedule with `switch` pinned, refused when the CHIP does not
    /// allow it here.
    pub fn with_switch(mut self, switch: IntervalSwitch) -> Result<Self, ScheduleError> {
        self.check_switch(switch)?;
        let slot = self
            .pinned
            .iter_mut()
            .find(|slot| slot.is_none())
            .ok_or(ScheduleError::Full)?;
        *slot = Some(switch);
        Ok(self)
    }

    /// Whether `switch` may follow this schedule's pinned switches.
    fn check_switch(&self, switch: IntervalSwitch) -> Result<(), ScheduleError> {
        if !divides_600(switch.interval) {
            return Err(ScheduleError::Interval(switch.interval));
        }
        if let Some(last) = self.switches().last()
            && switch.height <= last.height
        {
            return Err(ScheduleError::Order {
                height: switch.height,
                last: last.height,
            });
        }
        let ticks = self.ticks(switch.height);
        if ticks % BCH_BLOCK_SPACING != 0 {
            return Err(ScheduleError::Grid {
                height: switch.height,
                ticks,
            });
        }
        Ok(())
    }

    /// The pinned switches, in height order.
    pub fn switches(&self) -> impl DoubleEndedIterator<Item = IntervalSwitch> + '_ {
        self.pinned.iter().flatten().copied()
    }

    /// The switches by time, in activation order.
    pub fn timed_switches(&self) -> &'static [TimedSwitch] {
        self.timed
    }

    /// The interval of a block built on one whose median time past is
    /// `median_time_past`: the latest timed switch it has reached, or 600
    /// (BCHN's `GetPowTargetSpacing`).
    pub fn interval_reached(&self, median_time_past: i64) -> i64 {
        self.timed
            .iter()
            .rev()
            .find(|switch| switch.activation_time <= median_time_past)
            .map_or(BCH_BLOCK_SPACING, |switch| switch.interval)
    }

    /// The interval of a block built on the one at `height`, whose own time
    /// and its ancestors' are `times`, newest first: the interval its median
    /// time past reaches ([`Self::interval_reached`]). A median time past
    /// reads the block and its ten ancestors, or every block down to the
    /// chain's first for a block below height 10. With fewer times than that
    /// the pins answer by height ([`Self::interval_after`]).
    pub fn next_interval(&self, height: u32, times: &[u32]) -> i64 {
        let span = usize::try_from(height)
            .map_or(MEDIAN_TIME_SPAN, |height| MEDIAN_TIME_SPAN.min(height + 1));
        match times.get(..span).and_then(median_time_past) {
            Some(median_time_past) => self.interval_reached(median_time_past),
            None => self.interval_after(height),
        }
    }

    /// The tick info of the block at `height` built on a block with
    /// `parent`'s, whose own time and its ancestors' are `times`, newest
    /// first: its ticks follow its parent's, and its next interval is the one
    /// its median time past reaches ([`Self::next_interval`]). This is how
    /// BCHN sets a block's ticks when it adds the block to its index
    /// (`AddToBlockIndex`).
    pub fn tick_info_on(&self, parent: TickInfo, height: u32, times: &[u32]) -> TickInfo {
        TickInfo {
            ticks: parent.next_ticks(),
            next_interval: self.next_interval(height, times),
        }
    }

    /// The tick info the pins give the block at `height`, for where no
    /// header is known or a header's parent is not.
    pub fn tick_info(&self, height: u32) -> TickInfo {
        TickInfo {
            ticks: self.ticks(height),
            next_interval: self.interval_after(height),
        }
    }

    /// The ticks the pins give the block at `height`: 600 for each block up
    /// to the first switch, then each switch's interval for the blocks after
    /// it. Genesis has none.
    pub fn ticks(&self, height: u32) -> i64 {
        let height = i64::from(height);
        let (mut ticks, mut from, mut interval) = (0, 0, BCH_BLOCK_SPACING);
        for switch in self.switches() {
            let switch_height = i64::from(switch.height);
            if height <= switch_height {
                break;
            }
            ticks += (switch_height - from) * interval;
            from = switch_height;
            interval = switch.interval;
        }
        ticks + (height - from) * interval
    }

    /// The target interval, in ticks, the pins give the block at `height`.
    /// Genesis reports 600.
    pub fn interval_of(&self, height: u32) -> i64 {
        self.switches()
            .rev()
            .find(|switch| switch.height < height)
            .map_or(BCH_BLOCK_SPACING, |switch| switch.interval)
    }

    /// The target interval, in ticks, the pins give the block after the one
    /// at `height`.
    pub fn interval_after(&self, height: u32) -> i64 {
        self.interval_of(height.saturating_add(1))
    }

    /// The highest block height whose ticks, by the pins, are at most
    /// `ticks`.
    fn height_at_or_below(&self, ticks: i64) -> u32 {
        let ticks = ticks.max(0);
        let (mut from, mut from_ticks, mut interval) = (0, 0, BCH_BLOCK_SPACING);
        for switch in self.switches() {
            let switch_height = i64::from(switch.height);
            let switch_ticks = from_ticks + (switch_height - from) * interval;
            if ticks < switch_ticks {
                break;
            }
            (from, from_ticks, interval) = (switch_height, switch_ticks, switch.interval);
        }
        u32::try_from(from + (ticks - from_ticks) / interval).unwrap_or(u32::MAX)
    }

    /// Whether the coinbase of the block at `height`, with `confirmations`
    /// on the node's chain, can be spent in the next block, by the pins:
    /// that block's ticks are at least [`BCH_COINBASE_MATURITY_TICKS`] past
    /// the coinbase's. That is 100 ten-minute blocks or 1,000 one-minute
    /// blocks. Where both blocks' headers are known,
    /// [`TickInfo::coinbase_spendable_in_next_block`] reads their own ticks.
    pub fn coinbase_spendable_in_next_block(&self, height: u32, confirmations: i64) -> bool {
        let Some(next) = i64::from(height)
            .checked_add(confirmations)
            .and_then(|next| u32::try_from(next).ok())
        else {
            return false;
        };
        confirmations > 0 && self.ticks(next) - self.ticks(height) >= BCH_COINBASE_MATURITY_TICKS
    }

    /// The first block height at which a transaction locked to height
    /// `lock_height` (`nLockTime`, or `OP_CHECKLOCKTIMEVERIFY`'s operand) is
    /// final, by the pins.
    ///
    /// CHIP-2025-03 keeps a lock height in 600-tick units: it is final in a
    /// block whose ticks are past `600 * lock_height`. That is block
    /// `lock_height + 1` while blocks are ten minutes, and later after a
    /// switch to shorter blocks: ten one-minute blocks per unit.
    pub fn first_block_after_lock_height(&self, lock_height: u32) -> u32 {
        self.height_at_or_below(i64::from(lock_height) * BCH_BLOCK_SPACING)
            .saturating_add(1)
    }
}

/// Whether `interval` is one CHIP-2025-03 allows: 1 to 600 ticks, dividing
/// 600.
pub fn divides_600(interval: i64) -> bool {
    (1..=BCH_BLOCK_SPACING).contains(&interval) && BCH_BLOCK_SPACING % interval == 0
}

/// Bitcoin Cash coinbase maturity in ticks, CHIP-2025-03: 60,000, which is
/// 100 ten-minute blocks or 1,000 one-minute blocks.
pub const BCH_COINBASE_MATURITY_TICKS: i64 = 60_000;

/// The proof-of-work limit of mainnet, testnet3, testnet4, chipnet and
/// scalenet: `00000000ffff...ff`, 2^224 - 1. Its compact form is
/// [`BCH_POW_LIMIT_BITS`].
pub fn bch_pow_limit() -> U256 {
    (U256::one() << 224) - U256::one()
}

/// `GetCompact` of [`bch_pow_limit`]: the bits of a minimum-difficulty block.
pub const BCH_POW_LIMIT_BITS: u32 = 0x1d00_ffff;

/// Mainnet's band below the computed target: a header may be up to twice as
/// hard as its own time implies.
const MAINNET_HARD_DIVISOR: u32 = 2;

/// Test networks' band below the computed target: up to 8 times as hard.
const TEST_NETWORK_HARD_DIVISOR: u32 = 8;

/// Mainnet's band above the computed target: at most 5% easier,
/// `105 / 100`. A header's time can fall before its parent's by at most
/// BCHN's two-hour future limit, which with mainnet's two-day half-life
/// is a factor of 2^(7200 / 172800), about 1.029.
const MAINNET_EASY: (u32, u32) = (105, 100);

/// Test networks' band above the computed target: up to 8 times easier,
/// `8 / 1`. With the one-hour half-life a header dated two hours before
/// its parent computes a target 4 times too hard; 8 leaves room.
const TEST_NETWORK_EASY: (u32, u32) = (8, 1);

/// One network's ASERT anchor and half-life, from BCHN's chain parameters,
/// and the band P2Pool's header check allows around the computed target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BchAsert {
    /// Height of the anchor block.
    pub anchor_height: u32,
    /// The anchor block's nBits.
    pub anchor_bits: u32,
    /// Time of the anchor block's parent, which the absolute formulation of
    /// ASERT measures from.
    pub anchor_parent_time: i64,
    /// Seconds behind (ahead of) schedule that double (halve) the target.
    pub half_life: i64,
    /// Test networks allow minimum-difficulty blocks
    /// (`fPowAllowMinDifficultyBlocks`).
    pub allow_pow_limit: bool,
    /// A header may be this many times harder than the computed target.
    pub hard_divisor: u32,
    /// A header may be up to `easy_numerator / easy_denominator` times
    /// easier than the computed target.
    pub easy_numerator: u32,
    /// See [`Self::easy_numerator`].
    pub easy_denominator: u32,
    /// The network's block intervals (CHIP-2025-03), for a header whose
    /// parent is not known.
    pub tick_schedule: TickSchedule,
}

impl BchAsert {
    /// Mainnet (`CMainParams`): anchor block 661,647 on bits `0x1804dafe`,
    /// its parent at 1,605,447,844, and a two-day half-life.
    pub const MAINNET: Self = Self {
        anchor_height: 661_647,
        anchor_bits: 0x1804_dafe,
        anchor_parent_time: 1_605_447_844,
        half_life: 2 * 24 * 60 * 60,
        allow_pow_limit: false,
        hard_divisor: MAINNET_HARD_DIVISOR,
        easy_numerator: MAINNET_EASY.0,
        easy_denominator: MAINNET_EASY.1,
        tick_schedule: TickSchedule::MAINNET,
    };

    /// Chipnet (`CChipNetParams`), which shares testnet4's anchor
    /// (`CTestNet4Params`): block 16,844 on bits `0x1d00ffff`, its parent at
    /// 1,605,451,779, and a one-hour half-life.
    pub const CHIPNET: Self = Self {
        anchor_height: 16_844,
        anchor_bits: 0x1d00_ffff,
        anchor_parent_time: 1_605_451_779,
        half_life: 60 * 60,
        allow_pow_limit: true,
        hard_divisor: TEST_NETWORK_HARD_DIVISOR,
        easy_numerator: TEST_NETWORK_EASY.0,
        easy_denominator: TEST_NETWORK_EASY.1,
        tick_schedule: TickSchedule::CHIPNET,
    };

    /// Testnet4: the same anchor and half-life as chipnet, and mainnet's
    /// activation time.
    pub const TESTNET4: Self = Self {
        tick_schedule: TickSchedule::MAINNET,
        ..Self::CHIPNET
    };

    /// The parameters for a network P2Pool runs on. Regtest has no anchor in
    /// BCHN (nor does scalenet), so it has none here and its headers are not
    /// checked.
    pub fn for_network(network: BitcoinCashNetwork) -> Option<Self> {
        match network {
            BitcoinCashNetwork::Mainnet => Some(Self::MAINNET),
            BitcoinCashNetwork::Chipnet => Some(Self::CHIPNET),
            BitcoinCashNetwork::Regtest | BitcoinCashNetwork::Scalenet => None,
        }
    }

    /// The target ASERT requires of a block on a parent with tick info
    /// `parent`, dated `parent_time`, as `GetNextASERTWorkRequired` computes
    /// it (before a test network's minimum-difficulty rule). `None` below the
    /// anchor, where ASERT does not govern.
    ///
    /// The tick difference is the parent's ticks less the anchor's, `600 *
    /// anchor_height` (every anchor is a ten-minute block), and the new
    /// block's interval is the parent's next interval: after a switch to
    /// one-minute blocks the target is ten times easier, for a block ten
    /// times as frequent.
    pub fn next_target(&self, parent: TickInfo, parent_time: u32) -> Option<U256> {
        let tick_delta = parent.ticks - i64::from(self.anchor_height) * BCH_BLOCK_SPACING;
        if tick_delta < 0 {
            return None;
        }
        let time_diff = i64::from(parent_time) - self.anchor_parent_time;
        let reference = set_compact(self.anchor_bits)?;
        calculate_asert(
            reference,
            time_diff,
            tick_delta,
            BCH_BLOCK_SPACING,
            parent.next_interval,
            self.half_life,
            bch_pow_limit(),
        )
    }

    /// [`Self::next_target`] in compact form: the nBits BCHN requires.
    pub fn next_bits(&self, parent: TickInfo, parent_time: u32) -> Option<CompactTarget> {
        self.next_target(parent, parent_time).map(get_compact)
    }

    /// The ASERT format check of a header at `height` (its coinbase's BIP34
    /// height) with nBits `bits` and time `header_time`, on a parent with
    /// tick info `parent` when the parent's header is known.
    ///
    /// The header's own time stands in for its parent's, so the target is
    /// computed as ASERT would from that parent dated `header_time`, and the
    /// header's target must lie in the agreed band:
    ///
    /// - mainnet: `computed / 2 <= header_target <= computed * 1.05`;
    /// - test networks: `computed / 8 <= header_target <= computed * 8`, or
    ///   nBits exactly the proof-of-work limit's (a minimum-difficulty
    ///   block).
    ///
    /// The hard side is wide because a header can be minted hours after its
    /// parent. The easy side covers a header dated up to two hours before
    /// its parent, which BCHN accepts when the parent is dated ahead: a
    /// factor of about 1.029 on mainnet, 4 on the test networks' one-hour
    /// half-life. `computed / d` is floored and the easy side is compared
    /// exactly (`target * 100 <= computed * 105` on mainnet).
    ///
    /// The parent's ticks and the new block's interval are the parent's own
    /// (`KnownBchHeader`) when it is known. Without it (`None`) they come
    /// from the pins by height ([`TickSchedule::tick_info`]), which match the
    /// header's own values on chipnet's chain but give ten-minute blocks on a
    /// network whose switch is not pinned yet, as mainnet's is not.
    ///
    /// The bands are the SAFA covenant's (`p2poolv2_safa`), number for
    /// number, and so is the computed target without a known parent: the
    /// covenant is compiled with the same pinned switch and counts the
    /// parent's ticks and the new block's interval from it, as
    /// [`TickSchedule::tick_info`] does. A deployed covenant keeps the pins
    /// it was compiled with, so a pin added here later needs a new covenant
    /// with it; the tests compare the two for every network.
    pub fn check_header_bits(
        &self,
        bits: CompactTarget,
        header_time: u32,
        height: u64,
        parent: Option<TickInfo>,
    ) -> Result<(), String> {
        let bits = bits.to_consensus();
        let target = set_compact(bits).ok_or_else(|| format!("invalid nBits {bits:#010x}"))?;
        if target > bch_pow_limit() {
            return Err(format!(
                "nBits {bits:#010x} is above the proof-of-work limit"
            ));
        }
        if self.allow_pow_limit && bits == BCH_POW_LIMIT_BITS {
            return Ok(());
        }
        let parent_height = height
            .checked_sub(1)
            .and_then(|height| u32::try_from(height).ok())
            .ok_or_else(|| format!("height {height} has no ASERT parent"))?;
        let parent = parent.unwrap_or_else(|| self.tick_schedule.tick_info(parent_height));
        let computed = self.next_target(parent, header_time).ok_or_else(|| {
            format!(
                "height {height} is not above the ASERT anchor {}",
                self.anchor_height
            )
        })?;
        let hard_floor = computed / U256::from(self.hard_divisor);
        if target < hard_floor {
            return Err(format!(
                "nBits {bits:#010x} is more than {} times harder than ASERT's {:#010x} at height {height}",
                self.hard_divisor,
                get_compact(computed).to_consensus()
            ));
        }
        // Both sides stay far below 2^256: the target is at most the limit,
        // 2^224 - 1, and so is the computed target.
        if target * U256::from(self.easy_denominator) > computed * U256::from(self.easy_numerator) {
            return Err(format!(
                "nBits {bits:#010x} is more than {}/{} times easier than ASERT's {:#010x} at height {height}",
                self.easy_numerator,
                self.easy_denominator,
                get_compact(computed).to_consensus()
            ));
        }
        Ok(())
    }

    /// [`Self::check_header_bits`] as every peer applies it to a token
    /// share: by the pins, as the header gate does without a store, and by
    /// `known_parent`, the tick info of the block the header builds on, as
    /// block validation does once that block is known. A share must pass
    /// both, so a token job or a token win is only worth handing a miner
    /// when it does.
    pub fn check_header_bits_for_peers(
        &self,
        bits: CompactTarget,
        header_time: u32,
        height: u64,
        known_parent: Option<TickInfo>,
    ) -> Result<(), String> {
        self.check_header_bits(bits, header_time, height, None)?;
        match known_parent {
            Some(parent) => self.check_header_bits(bits, header_time, height, Some(parent)),
            None => Ok(()),
        }
    }
}

/// BCHN's `CalculateASERT` with CHIP-2025-03's ticks: `reference *
/// 2^((time_diff - (tick_delta + anchor_interval)) / half_life) *
/// anchor_interval / next_interval` in 16.16 fixed point, clamped to
/// `pow_limit` and to at least 1.
///
/// `tick_delta` is the parent's ticks less the anchor's, `600 *
/// height_diff` on a chain that has not activated one-minute blocks.
/// `anchor_interval` is the anchor block's interval in ticks, 600 on every
/// Bitcoin Cash network ([`BCH_BLOCK_SPACING`]), and `next_interval` the new
/// block's, 600 or 60. Scaling by the ratio of the two can pass 2^256; the
/// CHIP's reference then returns the limit, not the wrapped value, and so
/// does this.
///
/// `None` where BCHN asserts instead: a zero reference or one above the
/// limit, a negative tick difference, a non-positive half-life, an
/// interval that does not divide 600, or a time so far off schedule that
/// the fixed-point exponent would overflow (`|time_diff - tick_delta| >=
/// 2^47`).
pub fn calculate_asert(
    reference: U256,
    time_diff: i64,
    tick_delta: i64,
    anchor_interval: i64,
    next_interval: i64,
    half_life: i64,
    pow_limit: U256,
) -> Option<U256> {
    if reference.is_zero() || reference > pow_limit || tick_delta < 0 || half_life <= 0 {
        return None;
    }
    let divides_600 = |interval: i64| {
        (1..=BCH_BLOCK_SPACING).contains(&interval) && BCH_BLOCK_SPACING % interval == 0
    };
    if !divides_600(anchor_interval) || !divides_600(next_interval) {
        return None;
    }
    // BCHN needs leading zero bits in the limit to detect overflow; every
    // network's limit has 32.
    if !(pow_limit >> 224).is_zero() {
        return None;
    }
    let span = time_diff.checked_sub(tick_delta)?;
    if span.unsigned_abs() >= 1 << (63 - 16) {
        return None;
    }
    // C++ and Rust both truncate toward zero in integer division.
    let exponent = (span - anchor_interval) * 65_536 / half_life;
    // `>>` on a negative i64 is arithmetic in both: it floors, so the
    // fraction below is always in 0..65536.
    let mut shifts = exponent >> 16;
    let fraction = u128::from(exponent as u16);
    // 65536 * 2^(fraction / 65536), BCHN's cubic; at most 2^64 - 2^47 or so,
    // computed wide to be safe.
    let factor = 65_536
        + ((195_766_423_245_049 * fraction
            + 971_821_376 * fraction * fraction
            + 5_127 * fraction * fraction * fraction
            + (1 << 47))
            >> 48);
    // Below 2^241: the reference is below 2^224 and the factor below 2^17.
    let mut next = reference * U256::from(factor as u64);
    shifts -= 16;
    if shifts <= 0 {
        let right = shifts.unsigned_abs();
        next = if right >= 256 {
            U256::zero()
        } else {
            next >> right as usize
        };
    } else if next.bits() as u64 + shifts.unsigned_abs() > 256 {
        // BCHN's 256-bit shift would discard high bits: the limit.
        next = pow_limit;
    } else {
        next <<= shifts as usize;
    }
    // The interval ratio, a whole number from 1 to 600 either way. A
    // product past 2^256 is the limit (the CHIP's `ratio_guard_wrap_low`
    // vector).
    next = if anchor_interval >= next_interval {
        next.checked_mul(U256::from((anchor_interval / next_interval) as u64))
            .unwrap_or(pow_limit)
    } else {
        next / U256::from((next_interval / anchor_interval) as u64)
    };
    if next.is_zero() {
        next = U256::one();
    } else if next > pow_limit {
        next = pow_limit;
    }
    Some(next)
}

/// BCHN's `arith_uint256::SetCompact`, refusing a negative, zero or
/// overflowing value.
pub fn set_compact(bits: u32) -> Option<U256> {
    let size = bits >> 24;
    let word = bits & 0x007f_ffff;
    let value = if size <= 3 {
        U256::from(word >> (8 * (3 - size)))
    } else {
        let overflow =
            word != 0 && (size > 34 || (word > 0xff && size > 33) || (word > 0xffff && size > 32));
        if overflow {
            return None;
        }
        U256::from(word) << (8 * (size as usize - 3))
    };
    let negative = word != 0 && bits & 0x0080_0000 != 0;
    if negative || value.is_zero() {
        return None;
    }
    Some(value)
}

/// BCHN's `arith_uint256::GetCompact` for a non-negative value.
pub fn get_compact(value: U256) -> CompactTarget {
    let mut size = value.bits().div_ceil(8) as u32;
    let mut compact = if size <= 3 {
        (value.low_u64() << (8 * (3 - size))) as u32
    } else {
        (value >> (8 * (size as usize - 3))).low_u32()
    };
    if compact & 0x0080_0000 != 0 {
        compact >>= 8;
        size += 1;
    }
    CompactTarget::from_consensus(compact | size << 24)
}

/// The height a BCH coinbase scriptSig starts with (BIP34, as BCHN writes
/// it: `CScript() << nHeight`): `OP_1` to `OP_16`, or a minimal push of a
/// positive little-endian number of up to 5 bytes.
pub fn bip34_height(script_sig: &[u8]) -> Option<u64> {
    let (&opcode, rest) = script_sig.split_first()?;
    match opcode {
        0x51..=0x60 => Some(u64::from(opcode - 0x50)),
        1..=5 => {
            let bytes = rest.get(..usize::from(opcode))?;
            let last = *bytes.last()?;
            // Positive and minimal: no sign bit, and no needless zero byte.
            if last & 0x80 != 0
                || (last == 0 && (bytes.len() == 1 || bytes[bytes.len() - 2] & 0x80 == 0))
            {
                return None;
            }
            Some(
                bytes
                    .iter()
                    .rev()
                    .fold(0u64, |value, byte| value << 8 | u64::from(*byte)),
            )
        }
        _ => None,
    }
}

#[cfg(test)]
pub(crate) mod tests;
