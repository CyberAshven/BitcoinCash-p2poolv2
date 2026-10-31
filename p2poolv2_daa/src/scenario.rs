// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Hashrate scenarios: upstream's six, ported, and the Bitcoin Cash ones.
//!
//! Hashrate is in hashes per second. Upstream's scenarios are in units of a
//! base hashrate whose steady difficulty gives 10-second shares at genesis;
//! here that unit is 1 PH/s ([`UPSTREAM_UNIT_HASHRATE`]), far above the
//! share chain's ceiling, so the ceiling never binds and the numbers compare
//! with upstream's.

use crate::difficulty::{compact_for_difficulty, steady_difficulty};
use bitcoin::CompactTarget;
use p2poolv2_lib::shares::bch_genesis::{MAIN_ANCHOR_TARGET, MINI_ANCHOR_TARGET};
use p2poolv2_tools::seeded::SeededSequence;

/// Upstream's hashrate 1.0.
pub const UPSTREAM_UNIT_HASHRATE: f64 = 1e15;

/// One BitAxe Gamma: 1.2 TH/s, as its vendors list it.
pub const BITAXE: f64 = 1.2e12;

/// The 4 TH/s ASIC of the chipnet restart behind decision 12.
pub const CHIPNET_ASIC: f64 = 4e12;

/// A current large SHA-256 ASIC, 200 TH/s: the device behind each stratum
/// connection of a farm.
pub const FARM_ASIC: f64 = 200e12;

/// A multi-core CPU miner, 100 MH/s, like the one that mined beside the
/// ASIC on chipnet (`docs/difficulty_adjustment/asert.md`). Its stratum
/// difficulty is the session minimum, 1, below any share difficulty.
pub const CPU_MINER: f64 = 100e6;

pub const MINUTE: f64 = 60.0;
pub const HOUR: f64 = 3_600.0;
pub const DAY: f64 = 86_400.0;

/// Peta- and exahashes per second.
pub const PETAHASH: f64 = 1e15;
pub const EXAHASH: f64 = 1e18;

/// The share chain a scenario runs on, which sets genesis bits and the
/// ceiling.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Genesis {
    /// The Bitcoin Cash main share chain: genesis bits `0x1b4188f5`,
    /// ceiling `MAX_POOL_TARGET` (difficulty about 1,164).
    Main,
    /// The Bitcoin Cash mini share chain: genesis bits and ceiling
    /// `0x1c384bd7` (difficulty about 4.5).
    Mini,
    /// Main-chain ceiling, genesis at the steady difficulty of `hashrate`,
    /// as upstream started its chains (and as the sim swarm does).
    Regulated { hashrate: f64 },
}

impl Genesis {
    pub fn bits(&self) -> CompactTarget {
        match self {
            Genesis::Main => CompactTarget::from_consensus(MAIN_ANCHOR_TARGET),
            Genesis::Mini => CompactTarget::from_consensus(MINI_ANCHOR_TARGET),
            Genesis::Regulated { hashrate } => compact_for_difficulty(steady_difficulty(*hashrate)),
        }
    }
}

/// How long a run lasts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Length {
    /// Stop after this many shares (upstream's runs).
    Shares(usize),
    /// Stop at this many seconds after genesis.
    Seconds(f64),
}

/// A miner group's hashrate over a run.
///
/// Height schedules are read once per share, at the share's height, as
/// upstream did. Time schedules are piecewise constant and change exactly
/// at their boundaries, also in the middle of a share.
#[derive(Clone, Debug, PartialEq)]
pub enum Schedule {
    Constant(f64),
    /// `on` for the first `on_shares` heights of every `period`, `off` for
    /// the rest.
    HeightCycle {
        period: u32,
        on_shares: u32,
        on: f64,
        off: f64,
    },
    /// `before` below height `at`, `after` from it.
    HeightStep {
        at: u32,
        before: f64,
        after: f64,
    },
    /// Linear in height from `from` at height 0 to `to` at height `over`.
    HeightLinear {
        from: f64,
        to: f64,
        over: u32,
    },
    /// Uniform in `[low, high)` at each height, drawn from the run's seed.
    HeightNoise {
        low: f64,
        high: f64,
    },
    /// `(start time, hashrate)` steps in ascending time; zero before the
    /// first.
    TimeSteps(Vec<(f64, f64)>),
    /// From `start`, `on` for the first `on_seconds` of every `period` and
    /// `off` for the rest; `off` before `start`.
    TimeCycle {
        start: f64,
        period: f64,
        on_seconds: f64,
        on: f64,
        off: f64,
    },
}

impl Schedule {
    /// Hashrate for the share at `height`, at `time` seconds after genesis,
    /// in the run seeded with `seed`.
    pub fn hashrate(&self, height: u32, time: f64, seed: u64) -> f64 {
        match self {
            Schedule::Constant(hashrate) => *hashrate,
            Schedule::HeightCycle {
                period,
                on_shares,
                on,
                off,
            } => {
                if height % period < *on_shares {
                    *on
                } else {
                    *off
                }
            }
            Schedule::HeightStep { at, before, after } => {
                if height < *at {
                    *before
                } else {
                    *after
                }
            }
            Schedule::HeightLinear { from, to, over } => {
                from + (to - from) * (height as f64 / *over as f64)
            }
            Schedule::HeightNoise { low, high } => low + (high - low) * noise(seed, height),
            Schedule::TimeSteps(steps) => {
                let index = steps.partition_point(|(start, _)| *start <= time);
                match index.checked_sub(1) {
                    Some(current) => steps[current].1,
                    None => 0.0,
                }
            }
            Schedule::TimeCycle {
                start,
                period,
                on_seconds,
                on,
                off,
            } => {
                if time < *start || (time - start).rem_euclid(*period) >= *on_seconds {
                    *off
                } else {
                    *on
                }
            }
        }
    }

    /// First time after `time` at which a time schedule changes. Height
    /// schedules change only between shares and return `None`.
    pub fn next_change(&self, time: f64) -> Option<f64> {
        match self {
            Schedule::TimeSteps(steps) => {
                let index = steps.partition_point(|(start, _)| *start <= time);
                steps.get(index).map(|(start, _)| *start)
            }
            Schedule::TimeCycle {
                start,
                period,
                on_seconds,
                ..
            } => {
                if time < *start {
                    Some(*start)
                } else {
                    let cycle_start = start + ((time - start) / period).floor() * period;
                    // Sorted candidates; the first also covers a floor that
                    // rounded up past `time`.
                    [
                        cycle_start,
                        cycle_start + on_seconds,
                        cycle_start + period,
                        cycle_start + period + on_seconds,
                    ]
                    .into_iter()
                    .find(|boundary| *boundary > time)
                }
            }
            Schedule::Constant(_)
            | Schedule::HeightCycle { .. }
            | Schedule::HeightStep { .. }
            | Schedule::HeightLinear { .. }
            | Schedule::HeightNoise { .. } => None,
        }
    }
}

/// A value in `[0, 1)` fixed by `seed` and `height`.
fn noise(seed: u64, height: u32) -> f64 {
    SeededSequence::new(seed ^ (height as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)).next_unit()
}

/// Exponential growth from `from` to `to` hashes per second, in steps of
/// `step` seconds from `start` to `start + duration`; zero before `start`.
pub fn exponential_growth(from: f64, to: f64, start: f64, duration: f64, step: f64) -> Schedule {
    let step_count = (duration / step).round() as usize;
    let steps = (0..=step_count)
        .map(|index| {
            let fraction = index as f64 / step_count as f64;
            (
                start + index as f64 * step,
                from * (to / from).powf(fraction),
            )
        })
        .collect();
    Schedule::TimeSteps(steps)
}

/// Miners that share a schedule.
#[derive(Clone, Debug, PartialEq)]
pub struct MinerGroup {
    pub name: &'static str,
    /// Hashrate behind each stratum connection. The node's vardiff gives a
    /// connection the difficulty at which it submits every 3.33 seconds, and
    /// a miner sends only work that meets that difficulty, so its shares can
    /// arrive no faster than its submissions. `None` leaves share arrivals
    /// unbounded, as upstream's model did.
    pub device_hashrate: Option<f64>,
    pub schedule: Schedule,
}

/// A hashrate scenario.
#[derive(Clone, Debug, PartialEq)]
pub struct Scenario {
    pub id: &'static str,
    pub title: &'static str,
    /// Plain-words description for the report.
    pub description: &'static str,
    pub genesis: Genesis,
    pub groups: Vec<MinerGroup>,
    pub length: Length,
    /// Shares left out of the interval statistics (upstream: 100).
    pub warmup_shares: usize,
    /// Times at which hashrate changes, from which settling is measured.
    pub events: Vec<f64>,
    /// Counted twice in upstream's composite score and in its attack score.
    pub attack: bool,
}

impl Scenario {
    /// Total hashrate at `height` and `time`.
    pub fn total_hashrate(&self, height: u32, time: f64, seed: u64) -> f64 {
        self.groups
            .iter()
            .map(|group| group.schedule.hashrate(height, time, seed))
            .sum()
    }

    /// Shares a run is likely to hold, for preallocation: 10-second shares
    /// over the run while any group mines.
    pub fn expected_shares(&self, ideal_interval: f64) -> usize {
        match self.length {
            Length::Shares(count) => count,
            Length::Seconds(seconds) => (seconds / ideal_interval) as usize + 1,
        }
    }
}

/// Upstream's six scenarios: 5,000 shares, the first 100 left out.
pub fn upstream_suite() -> Vec<Scenario> {
    let unit = UPSTREAM_UNIT_HASHRATE;
    let base = |id, title, description, schedule, attack| Scenario {
        id,
        title,
        description,
        genesis: Genesis::Regulated { hashrate: unit },
        groups: vec![MinerGroup {
            name: "pool",
            device_hashrate: None,
            schedule,
        }],
        length: Length::Shares(5_000),
        warmup_shares: 100,
        events: Vec::new(),
        attack,
    };
    vec![
        base(
            "steady",
            "Steady State",
            "Constant hashrate.",
            Schedule::Constant(unit),
            false,
        ),
        base(
            "growth-10x",
            "Gradual 10x Growth",
            "Hashrate grows linearly to ten times over the run.",
            Schedule::HeightLinear {
                from: unit,
                to: 10.0 * unit,
                over: 5_000,
            },
            false,
        ),
        base(
            "oscillation-100",
            "Oscillation Attack (100-share cycle)",
            "A miner with nine times the base hashrate mines 50 shares, leaves for 50, and repeats.",
            Schedule::HeightCycle {
                period: 100,
                on_shares: 50,
                on: 10.0 * unit,
                off: unit,
            },
            true,
        ),
        base(
            "oscillation-20",
            "Rapid Oscillation (20-share cycle)",
            "The same miner on for 10 shares and off for 10.",
            Schedule::HeightCycle {
                period: 20,
                on_shares: 10,
                on: 10.0 * unit,
                off: unit,
            },
            true,
        ),
        base(
            "drop-90",
            "Sudden 90% Drop",
            "Ten times the base hashrate until share 1,000, then the base.",
            Schedule::HeightStep {
                at: 1_000,
                before: 10.0 * unit,
                after: unit,
            },
            true,
        ),
        base(
            "noisy",
            "Noisy Random",
            "Hashrate drawn uniformly between half and twice the base at every share.",
            Schedule::HeightNoise {
                low: 0.5 * unit,
                high: 2.0 * unit,
            },
            false,
        ),
    ]
}

/// The Bitcoin Cash scenarios.
pub fn bch_suite() -> Vec<Scenario> {
    vec![
        bitaxe_alone(),
        bitaxe_to_exahash(),
        farm_joins_bitaxe(),
        switch_farm(
            "switch-4h",
            "BTC/BCH switch farm, 4 h on, 8 h off",
            "A 50 PH/s pool; a 2 EH/s farm, about half of Bitcoin Cash's hashrate (3.7 EH/s on 2026-10-23, docs/small-miners.md), mines on it for 4 hours of every 12 and on BTC the rest of the time.",
            4.0 * HOUR,
            12.0 * HOUR,
            3.0 * DAY,
        ),
        switch_farm(
            "switch-30m",
            "BTC/BCH switch farm, 30 min on, 30 min off",
            "The same farm switching every 30 minutes, as profit-switching services can.",
            30.0 * MINUTE,
            HOUR,
            DAY,
        ),
        farm_leaves(),
        quiet(
            "quiet-1h",
            "Pool quiet for 1 hour",
            "A 1 PH/s pool stops for 1 hour and comes back.",
            HOUR,
        ),
        quiet(
            "quiet-12h",
            "Pool quiet for 12 hours",
            "A 1 PH/s pool stops for 12 hours and comes back.",
            12.0 * HOUR,
        ),
        outage_two_days_mini(),
        late_start_mini(),
    ]
}

fn bitaxe_group() -> MinerGroup {
    MinerGroup {
        name: "BitAxe",
        device_hashrate: Some(BITAXE),
        schedule: Schedule::Constant(BITAXE),
    }
}

fn bitaxe_alone() -> Scenario {
    Scenario {
        id: "bitaxe-alone",
        title: "One BitAxe, main chain",
        description: "One 1.2 TH/s BitAxe mines the main share chain alone for a day. Its steady difficulty, about 2,800, is 2.4 times the ceiling.",
        genesis: Genesis::Main,
        groups: vec![bitaxe_group()],
        length: Length::Seconds(DAY),
        warmup_shares: 100,
        events: Vec::new(),
        attack: false,
    }
}

fn bitaxe_to_exahash() -> Scenario {
    let start = 6.0 * HOUR;
    Scenario {
        id: "bitaxe-to-exahash",
        title: "One BitAxe grows into a 10 EH/s pool in a week",
        description: "A BitAxe mines alone for 6 hours; then farms join and the pool grows exponentially from 1 TH/s to 10 EH/s over 7 days, doubling about every 7 hours.",
        genesis: Genesis::Main,
        groups: vec![
            bitaxe_group(),
            MinerGroup {
                name: "farms",
                device_hashrate: Some(FARM_ASIC),
                schedule: exponential_growth(1e12, 10.0 * EXAHASH, start, 7.0 * DAY, 10.0 * MINUTE),
            },
        ],
        length: Length::Seconds(start + 7.0 * DAY + 6.0 * HOUR),
        warmup_shares: 100,
        events: Vec::new(),
        attack: false,
    }
}

fn farm_joins_bitaxe() -> Scenario {
    let join = 2.0 * HOUR;
    Scenario {
        id: "farm-joins-bitaxe",
        title: "A 5 EH/s farm joins a one-BitAxe pool",
        description: "A BitAxe mines alone for 2 hours, then a 5 EH/s farm of 200 TH/s machines points at the pool at once: a four-million-fold step.",
        genesis: Genesis::Main,
        groups: vec![
            bitaxe_group(),
            MinerGroup {
                name: "farm",
                device_hashrate: Some(FARM_ASIC),
                schedule: Schedule::TimeSteps(vec![(0.0, 0.0), (join, 5.0 * EXAHASH)]),
            },
        ],
        length: Length::Seconds(join + 6.0 * HOUR),
        warmup_shares: 100,
        events: vec![join],
        attack: false,
    }
}

fn switch_farm(
    id: &'static str,
    title: &'static str,
    description: &'static str,
    on_seconds: f64,
    period: f64,
    duration: f64,
) -> Scenario {
    let start = 2.0 * HOUR;
    let cycles = (duration / period).round() as usize;
    let events = (0..cycles)
        .flat_map(|cycle| {
            let cycle_start = start + cycle as f64 * period;
            [cycle_start, cycle_start + on_seconds]
        })
        .collect();
    Scenario {
        id,
        title,
        description,
        genesis: Genesis::Main,
        groups: vec![
            MinerGroup {
                name: "base pool",
                device_hashrate: Some(FARM_ASIC),
                schedule: Schedule::Constant(50.0 * PETAHASH),
            },
            MinerGroup {
                name: "switch farm",
                device_hashrate: Some(FARM_ASIC),
                schedule: Schedule::TimeCycle {
                    start,
                    period,
                    on_seconds,
                    on: 2.0 * EXAHASH,
                    off: 0.0,
                },
            },
        ],
        length: Length::Seconds(start + duration),
        warmup_shares: 100,
        events,
        attack: false,
    }
}

fn farm_leaves() -> Scenario {
    let leave = 2.0 * HOUR;
    let back = leave + 12.0 * HOUR;
    Scenario {
        id: "farm-leaves",
        title: "A 1 PH/s farm leaves a BitAxe alone for 12 hours",
        description: "The farm, 99.9% of the pool, switches away at once and returns 12 hours later; one BitAxe keeps mining.",
        genesis: Genesis::Main,
        groups: vec![
            bitaxe_group(),
            MinerGroup {
                name: "farm",
                device_hashrate: Some(FARM_ASIC),
                schedule: Schedule::TimeSteps(vec![
                    (0.0, PETAHASH),
                    (leave, 0.0),
                    (back, PETAHASH),
                ]),
            },
        ],
        length: Length::Seconds(back + 6.0 * HOUR),
        warmup_shares: 100,
        events: vec![leave, back],
        attack: false,
    }
}

fn quiet(id: &'static str, title: &'static str, description: &'static str, gap: f64) -> Scenario {
    let stop = 2.0 * HOUR;
    let back = stop + gap;
    Scenario {
        id,
        title,
        description,
        genesis: Genesis::Main,
        groups: vec![MinerGroup {
            name: "pool",
            device_hashrate: Some(FARM_ASIC),
            schedule: Schedule::TimeSteps(vec![(0.0, PETAHASH), (stop, 0.0), (back, PETAHASH)]),
        }],
        length: Length::Seconds(back + 6.0 * HOUR),
        warmup_shares: 100,
        events: vec![stop, back],
        attack: false,
    }
}

/// The chipnet miners on the mini chain, the CPU miner first: both mine
/// from each `(time, true)` and stop at each `(time, false)`.
fn mini_miners(switches: &[(f64, bool)]) -> Vec<MinerGroup> {
    let group = |name, hashrate: f64| MinerGroup {
        name,
        device_hashrate: Some(hashrate),
        schedule: Schedule::TimeSteps(
            switches
                .iter()
                .map(|(time, on)| (*time, if *on { hashrate } else { 0.0 }))
                .collect(),
        ),
    };
    vec![group("CPU miner", CPU_MINER), group("ASIC", CHIPNET_ASIC)]
}

fn outage_two_days_mini() -> Scenario {
    let stop = 6.0 * HOUR;
    let back = stop + 2.0 * DAY;
    Scenario {
        id: "outage-2d-mini",
        title: "Two-day outage on the mini chain (decision 12)",
        description: "The 4 TH/s chipnet ASIC and a 100 MH/s CPU miner mine the mini chain, stop for two days and come back: the outage case behind decision 12.",
        genesis: Genesis::Mini,
        groups: mini_miners(&[(0.0, true), (stop, false), (back, true)]),
        length: Length::Seconds(back + 30.0 * HOUR),
        warmup_shares: 100,
        events: vec![stop, back],
        attack: false,
    }
}

fn late_start_mini() -> Scenario {
    let start = 180.0 * DAY;
    Scenario {
        id: "late-start-mini",
        title: "Mini chain started 180 days after genesis (decision 12)",
        description: "The 4 TH/s ASIC and a 100 MH/s CPU miner start the mini chain 180 days after its genesis time, as the chipnet restart of 2026-10-19 did.",
        genesis: Genesis::Mini,
        groups: mini_miners(&[(0.0, false), (start, true)]),
        length: Length::Seconds(start + 12.0 * HOUR),
        // The first share's interval is the 180 days nobody mined.
        warmup_shares: 1,
        events: vec![start],
        attack: false,
    }
}

#[cfg(test)]
mod tests;
