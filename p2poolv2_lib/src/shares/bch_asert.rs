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
//! The header check is a format check, not block validation: P2Pool has
//! only the header and its coinbase height, not the parent block, so the
//! header's own time stands in for its parent's and the computed target gets
//! a band (see [`BchAsert::check_header_bits`]). BCHN stays the authority on
//! whether a block is valid.

use crate::config::BitcoinCashNetwork;
use bitcoin::CompactTarget;
use std::sync::RwLock;

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

/// The most interval switches one network's schedule holds: one-minute
/// blocks, and room for later switches.
const MAX_SWITCHES: usize = 4;

/// One change of a network's block interval (CHIP-2025-03): every block
/// after `height` has `interval` ticks, until the next switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntervalSwitch {
    /// The last block with the old interval.
    pub height: u32,
    /// The interval, in ticks, of every block after it.
    pub interval: i64,
}

/// A switch a network has scheduled by time, not yet pinned to a block.
///
/// Its activation block is the first whose median time past is at or after
/// `activation_time`; the blocks after it have `interval` ticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnnouncedSwitch {
    /// Median time past at which the switch activates.
    pub activation_time: i64,
    /// The interval, in ticks, of every block after the activation block.
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

/// Switches a node learned while running, from its own Bitcoin Cash node
/// (`node::bch_header_feeder`), by network. Every [`TickSchedule`] of that
/// network reads them when it is used, so the header rules and validators
/// built at startup follow a switch learned later.
static LEARNED_SWITCHES: RwLock<Vec<(BitcoinCashNetwork, IntervalSwitch)>> =
    RwLock::new(Vec::new());

/// A network's block intervals under CHIP-2025-03.
///
/// A block's ticks are the sum of the target intervals of the blocks up to
/// it. Every block has 600 ticks until the first switch; each switch sets
/// the interval of the blocks after its height. Chipnet has one switch, to
/// 60 ticks after block 332,450; later switches, to 30 seconds say, are
/// more entries. Before any switch a block's ticks are `600 * height`, so
/// every rule counted in ticks is the old rule there.
///
/// The schedule holds the switches pinned in code, the switches this
/// process learned from its node ([`learn_switch`]), and a switch the
/// network has announced by time but neither has pinned yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TickSchedule {
    /// The network learned switches are read for, or `None` for a schedule
    /// of pinned switches only.
    network: Option<BitcoinCashNetwork>,
    pinned: [Option<IntervalSwitch>; MAX_SWITCHES],
    announced: Option<AnnouncedSwitch>,
}

impl TickSchedule {
    /// Ten-minute blocks only: no switch, none announced.
    pub const TEN_MINUTES: Self = Self {
        network: None,
        pinned: [None; MAX_SWITCHES],
        announced: None,
    };

    /// Mainnet (also testnet3, testnet4, scalenet and regtest, as in BCHN):
    /// one-minute blocks announced for 2027-05-15 12:00 UTC.
    const MAINNET: Self = Self {
        network: None,
        pinned: [None; MAX_SWITCHES],
        announced: Some(AnnouncedSwitch {
            activation_time: 1_810_382_400,
            interval: 60,
        }),
    };

    /// Chipnet: one-minute blocks after block 332,450, whose median time
    /// past, 1,794,744,504, was the first at or after 2026-11-15 12:00 UTC.
    const CHIPNET: Self = Self {
        network: None,
        pinned: [
            Some(IntervalSwitch {
                height: 332_450,
                interval: 60,
            }),
            None,
            None,
            None,
        ],
        announced: None,
    };

    /// The schedule of `network`: its pinned switches, those this process
    /// learned for it, and its announced switch while neither covers it.
    pub fn for_network(network: BitcoinCashNetwork) -> Self {
        let pinned = match network {
            BitcoinCashNetwork::Chipnet => Self::CHIPNET,
            BitcoinCashNetwork::Mainnet
            | BitcoinCashNetwork::Regtest
            | BitcoinCashNetwork::Scalenet => Self::MAINNET,
        };
        Self {
            network: Some(network),
            ..pinned
        }
    }

    /// This schedule with `switch` pinned, refused when the CHIP does not
    /// allow it here. A switch to the announced interval settles the
    /// announcement.
    pub fn with_switch(mut self, switch: IntervalSwitch) -> Result<Self, ScheduleError> {
        self.check_switch(switch)?;
        let slot = self
            .pinned
            .iter_mut()
            .find(|slot| slot.is_none())
            .ok_or(ScheduleError::Full)?;
        *slot = Some(switch);
        if self
            .announced
            .is_some_and(|announced| announced.interval == switch.interval)
        {
            self.announced = None;
        }
        Ok(self)
    }

    /// Whether `switch` may follow this schedule's switches.
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

    /// The pinned and learned switches, in height order.
    pub fn switches(&self) -> Vec<IntervalSwitch> {
        let mut switches: Vec<IntervalSwitch> = self.pinned.iter().flatten().copied().collect();
        if let Some(network) = self.network {
            let learned = LEARNED_SWITCHES
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            switches.extend(
                learned
                    .iter()
                    .filter(|(learned_for, _)| *learned_for == network)
                    .map(|(_, switch)| *switch),
            );
        }
        switches.sort_by_key(|switch| switch.height);
        switches.dedup_by_key(|switch| switch.height);
        switches
    }

    /// The network this schedule reads learned switches for.
    pub fn network(&self) -> Option<BitcoinCashNetwork> {
        self.network
    }

    /// The switch the network announced by time and neither code nor this
    /// process has pinned to a block yet.
    pub fn announced(&self) -> Option<AnnouncedSwitch> {
        let announced = self.announced?;
        let settled = self
            .switches()
            .iter()
            .any(|switch| switch.interval == announced.interval);
        (!settled).then_some(announced)
    }

    /// The ticks of the block at `height`: 600 for each block up to the
    /// first switch, then each switch's interval for the blocks after it.
    /// Genesis has none.
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

    /// The target interval, in ticks, of the block at `height`. Genesis
    /// reports 600.
    pub fn interval_of(&self, height: u32) -> i64 {
        self.switches()
            .iter()
            .rev()
            .find(|switch| switch.height < height)
            .map_or(BCH_BLOCK_SPACING, |switch| switch.interval)
    }

    /// The target interval, in ticks, of the block after the one at
    /// `height`.
    pub fn interval_after(&self, height: u32) -> i64 {
        self.interval_of(height.saturating_add(1))
    }

    /// The highest block height whose ticks are at most `ticks`.
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
    /// on the node's chain, can be spent in the next block: that block's
    /// ticks are at least [`BCH_COINBASE_MATURITY_TICKS`] past the
    /// coinbase's. That is 100 ten-minute blocks or 1,000 one-minute blocks.
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
    /// final.
    ///
    /// CHIP-2025-03 keeps a lock height in 600-tick units: it is final in a
    /// block whose ticks are past `600 * lock_height`. That is block
    /// `lock_height + 1` while blocks are ten minutes, and later after a
    /// switch to shorter blocks: ten one-minute blocks per unit.
    pub fn first_block_after_lock_height(&self, lock_height: u32) -> u32 {
        self.height_at_or_below(i64::from(lock_height) * BCH_BLOCK_SPACING)
            .saturating_add(1)
    }

    /// How many blocks back from the block at `tip` span at least `span`
    /// ticks: the fewest `n` with `ticks(tip) - ticks(tip - n) >= span`, or
    /// `tip` when the chain is shorter. 8,400 ten-minute blocks hold the
    /// span 84,000 one-minute blocks do.
    pub fn blocks_spanning(&self, tip: u32, span: i64) -> usize {
        let start = self.height_at_or_below(self.ticks(tip) - span).min(tip);
        (tip - start) as usize
    }
}

/// Whether `interval` is one CHIP-2025-03 allows: 1 to 600 ticks, dividing
/// 600.
fn divides_600(interval: i64) -> bool {
    (1..=BCH_BLOCK_SPACING).contains(&interval) && BCH_BLOCK_SPACING % interval == 0
}

/// Pin `switch` for `network` in this process, as its own Bitcoin Cash node
/// reported it. Returns whether it was new; a switch the schedule already
/// has is not added again, and one the CHIP does not allow there is
/// refused.
pub fn learn_switch(
    network: BitcoinCashNetwork,
    switch: IntervalSwitch,
) -> Result<bool, ScheduleError> {
    let schedule = TickSchedule::for_network(network);
    if schedule.switches().contains(&switch) {
        return Ok(false);
    }
    schedule.with_switch(switch)?;
    let mut learned = LEARNED_SWITCHES
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    learned.push((network, switch));
    Ok(true)
}

/// What a node reports about one of its blocks under CHIP-2025-03, from a
/// verbose `getblockheader`: each value is `None` when the node does not
/// report it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReportedTicks {
    /// The block's height.
    pub height: u32,
    /// Its ticks (`ticks`).
    pub ticks: Option<i64>,
    /// Its own interval (`target_time`).
    pub interval: Option<i64>,
    /// The interval of a block built on it (`next_target_time`).
    pub next_interval: Option<i64>,
}

/// The interval switch a node's report shows and `schedule` lacks, if any.
///
/// A node that reports a next interval other than the schedule's has a
/// switch right after this block. One whose block already has another
/// interval switched earlier: with its ticks the switch height is solved
/// from the last switch the schedule knows, `ticks = T(last) + (H - last) *
/// old + (height - H) * new`. Any interval the CHIP allows is followed, so
/// a later switch, to 30 seconds say, needs no new code. An error is a
/// report the schedule cannot follow: no whole switch height, or one the
/// CHIP refuses.
pub fn switch_from_report(
    schedule: &TickSchedule,
    report: ReportedTicks,
) -> Result<Option<IntervalSwitch>, String> {
    let height = report.height;
    let agrees = report
        .ticks
        .is_none_or(|ticks| ticks == schedule.ticks(height))
        && report
            .interval
            .is_none_or(|interval| interval == schedule.interval_of(height))
        && report
            .next_interval
            .is_none_or(|next| next == schedule.interval_after(height));
    if agrees {
        return Ok(None);
    }
    let candidate = match (report.ticks, report.interval, report.next_interval) {
        // Only the next block's interval differs: the switch follows this
        // block.
        (ticks, interval, Some(next))
            if ticks.is_none_or(|ticks| ticks == schedule.ticks(height))
                && interval.is_none_or(|interval| interval == schedule.interval_of(height)) =>
        {
            IntervalSwitch {
                height,
                interval: next,
            }
        }
        // This block already has the new interval: solve for where it
        // started.
        (Some(ticks), Some(interval), _) => {
            let (last_height, last_ticks, old) =
                schedule
                    .switches()
                    .last()
                    .map_or((0, 0, BCH_BLOCK_SPACING), |switch| {
                        (
                            i64::from(switch.height),
                            schedule.ticks(switch.height),
                            switch.interval,
                        )
                    });
            let numerator = ticks - last_ticks + last_height * old - i64::from(height) * interval;
            let denominator = old - interval;
            if denominator == 0 || numerator % denominator != 0 {
                return Err(format!(
                    "the node reports ticks {ticks} and interval {interval} at height {height}, which no single switch after height {last_height} gives"
                ));
            }
            let switch_height = numerator / denominator;
            if switch_height <= last_height || switch_height >= i64::from(height) {
                return Err(format!(
                    "the node's ticks {ticks} at height {height} put a switch at height {switch_height}, outside {last_height}..{height}"
                ));
            }
            IntervalSwitch {
                height: switch_height as u32,
                interval,
            }
        }
        _ => {
            return Err(format!(
                "the node reports {report:?}, which this schedule does not give, without the ticks to place a switch"
            ));
        }
    };
    let followed = schedule.with_switch(candidate).map_err(|error| {
        format!("the node's switch {candidate:?} is not one the CHIP allows: {error}")
    })?;
    let consistent = report
        .ticks
        .is_none_or(|ticks| ticks == followed.ticks(height))
        && report
            .interval
            .is_none_or(|interval| interval == followed.interval_of(height))
        && report
            .next_interval
            .is_none_or(|next| next == followed.interval_after(height));
    if !consistent {
        return Err(format!(
            "the node's report {report:?} still disagrees with the schedule after the switch {candidate:?}"
        ));
    }
    Ok(Some(candidate))
}

/// The activation block of `announced` among `times`: (height, nTime) of
/// consecutive blocks in height order, oldest first. It is the first block
/// whose median time past (it and its ten ancestors) is at or after the
/// activation time; `None` when no block in `times` with ten ancestors
/// there has reached it, or the first that has is the oldest such, so the
/// crossing may lie further back.
pub fn activation_height_from_times(
    announced: AnnouncedSwitch,
    times: &[(u32, u32)],
) -> Option<u32> {
    let mut previous_reached = None;
    for window in times.windows(11) {
        let mut sorted: Vec<u32> = window.iter().map(|(_, time)| *time).collect();
        sorted.sort_unstable();
        let reached = i64::from(sorted[5]) >= announced.activation_time;
        match (previous_reached, reached) {
            (Some(false), true) => return Some(window[10].0),
            // The first block measured is already past it: the crossing is
            // further back than `times` reaches.
            (None, true) => return None,
            _ => previous_reached = Some(reached),
        }
    }
    None
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
    /// The network's block intervals (CHIP-2025-03).
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
        let asert = match network {
            BitcoinCashNetwork::Mainnet => Self::MAINNET,
            BitcoinCashNetwork::Chipnet => Self::CHIPNET,
            BitcoinCashNetwork::Regtest | BitcoinCashNetwork::Scalenet => return None,
        };
        // The schedule that reads the switches this process learns.
        Some(Self {
            tick_schedule: TickSchedule::for_network(network),
            ..asert
        })
    }

    /// The target ASERT requires of the block after one at `parent_height`
    /// and `parent_time`, as `GetNextASERTWorkRequired` computes it (before
    /// a test network's minimum-difficulty rule). `None` below the anchor,
    /// where ASERT does not govern.
    ///
    /// The parent's ticks and the new block's interval come from the
    /// network's activation ([`TickSchedule`]): after it the target is ten
    /// times easier, for a block ten times as frequent.
    pub fn next_target(&self, parent_height: u32, parent_time: u32) -> Option<U256> {
        if parent_height < self.anchor_height {
            return None;
        }
        let tick_delta = self.tick_schedule.ticks(parent_height)
            - i64::from(self.anchor_height) * BCH_BLOCK_SPACING;
        let time_diff = i64::from(parent_time) - self.anchor_parent_time;
        let reference = set_compact(self.anchor_bits)?;
        calculate_asert(
            reference,
            time_diff,
            tick_delta,
            BCH_BLOCK_SPACING,
            self.tick_schedule.interval_after(parent_height),
            self.half_life,
            bch_pow_limit(),
        )
    }

    /// How long after its parent a test network block may take the
    /// proof-of-work limit: more than twice the new block's interval, 20
    /// minutes before activation and 2 minutes after.
    pub fn min_difficulty_gap(&self, parent_height: u32) -> u32 {
        (2 * self.tick_schedule.interval_after(parent_height)) as u32
    }

    /// [`Self::next_target`] in compact form: the nBits BCHN requires.
    pub fn next_bits(&self, parent_height: u32, parent_time: u32) -> Option<CompactTarget> {
        self.next_target(parent_height, parent_time)
            .map(get_compact)
    }

    /// The ASERT format check of a header at `height` (its coinbase's BIP34
    /// height) with nBits `bits` and time `header_time`.
    ///
    /// The header's own time stands in for its parent's, so the target is
    /// computed as ASERT would from a parent at `height - 1` dated
    /// `header_time`, and the header's target must lie in the agreed band:
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
    /// exactly (`target * 100 <= computed * 105` on mainnet). The bands are
    /// the SAFA covenant's (`p2poolv2_safa`), number for number.
    pub fn check_header_bits(
        &self,
        bits: CompactTarget,
        header_time: u32,
        height: u64,
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
        let computed = self
            .next_target(parent_height, header_time)
            .ok_or_else(|| {
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
