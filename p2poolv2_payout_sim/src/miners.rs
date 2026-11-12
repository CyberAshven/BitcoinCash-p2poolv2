// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Miners: the classes a pool is made of, the mixes the tables use and the
//! two tracked miners whose payouts are simulated one by one.

use crate::constants::{
    ASIC_HASHRATE, DEFAULT_LEVEL, HOME_MINER_HASHRATE, LARGE_FARM_HASHRATE, MEDIUM_MINER_HASHRATES,
    MINI_THRESHOLD_HASHRATE, SMALL_FARM_HASHRATE,
};

/// Ranks of a Zipf spread that get a class each; above them ranks are
/// grouped.
const ZIPF_SINGLE_RANKS: usize = 32;

/// Each group of Zipf ranks above the single ones spans this factor of
/// ranks.
const ZIPF_GROUP_FACTOR: f64 = 1.25;

/// Share of a medium mix's hashrate on home miners.
const MEDIUM_MIX_HOME_FRACTION: f64 = 0.1;

/// Share of the `medium, few big` mix's hashrate on 1 PH/s farms.
const MEDIUM_MIX_BIG_FRACTION: f64 = 0.1;

/// What a miner is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MinerKind {
    /// A home miner of the mix.
    Home,
    /// An industrial machine of the mix.
    Industrial,
    /// A medium miner of the mix, 10 TH/s to 1 PH/s.
    Medium,
    /// A miner whose every payment is simulated.
    Tracked,
    /// The weak-share spam attacker.
    Spammer,
}

/// A group of identical miners. Counts may be fractional: a class stands for
/// its share of the pool's hashrate.
#[derive(Debug, Clone)]
pub struct MinerClass {
    /// Name in reports.
    pub name: String,
    /// What the class is.
    pub kind: MinerKind,
    /// Hashes a second of one miner.
    pub hashrate: f64,
    /// Miners in the class.
    pub count: f64,
    /// Whether its node gives it the node default under the band; false for
    /// a miner that always mines at the minimum.
    pub follows_node_default: bool,
    /// Whether its node carries other nodes' weak shares.
    pub on_carrying_node: bool,
    /// Its payout level `k`.
    pub level: u32,
}

impl MinerClass {
    /// The class's whole hashrate.
    pub fn total_hashrate(&self) -> f64 {
        self.hashrate * self.count
    }
}

/// A miner whose payouts are simulated one by one.
#[derive(Debug, Clone)]
pub struct TrackedMiner {
    /// Name in reports.
    pub name: String,
    /// Hashes a second.
    pub hashrate: f64,
    /// Its payout level `k`.
    pub level: u32,
    /// Whether its node carries other nodes' weak shares.
    pub on_carrying_node: bool,
}

impl TrackedMiner {
    /// The tracked Bitaxe-class home miner, 1.2 TH/s, at the default level.
    pub fn bitaxe() -> Self {
        Self {
            name: "Bitaxe 1.2 TH/s".to_string(),
            hashrate: HOME_MINER_HASHRATE,
            level: DEFAULT_LEVEL,
            on_carrying_node: true,
        }
    }

    /// The tracked ASIC, 100 TH/s, at the default level.
    pub fn asic() -> Self {
        Self {
            name: "ASIC 100 TH/s".to_string(),
            hashrate: ASIC_HASHRATE,
            level: DEFAULT_LEVEL,
            on_carrying_node: true,
        }
    }

    /// The tracked ASIC on a node that never carries other nodes' weak
    /// shares.
    pub fn non_carrying_asic() -> Self {
        Self {
            name: "ASIC 100 TH/s, non-carrying node".to_string(),
            on_carrying_node: false,
            ..Self::asic()
        }
    }

    /// The class of one miner this tracked miner forms in the pool.
    pub fn class(&self) -> MinerClass {
        MinerClass {
            name: self.name.clone(),
            kind: MinerKind::Tracked,
            hashrate: self.hashrate,
            count: 1.0,
            follows_node_default: true,
            on_carrying_node: self.on_carrying_node,
            level: self.level,
        }
    }
}

/// How the pool's hashrate, beside the tracked miners, is split.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MinerMix {
    /// Every miner a 1.2 TH/s home miner.
    Home,
    /// Half home miners, half industrial (1 PH/s and 10 PH/s machines,
    /// equal hashrate each).
    HalfIndustrial,
    /// A tenth home miners, nine tenths industrial.
    MostlyIndustrial,
    /// A hundredth home miners, the rest industrial: almost no hashrate at
    /// the minimum, where `R_MAX` binds first.
    AlmostAllIndustrial,
    /// A Zipf spread of sizes: the `k`-th largest miner has `1/k` of the
    /// largest one's hashrate, the smallest a home miner's.
    Zipf,
    /// Nine tenths medium miners (30 to 300 TH/s), a tenth home miners, no
    /// big ones.
    Medium,
    /// Eight tenths medium miners, a tenth home miners and a tenth 1 PH/s
    /// farms.
    MediumFewBig,
}

impl MinerMix {
    /// Every mix, in table order.
    pub const ALL: [MinerMix; 3] = [
        MinerMix::Home,
        MinerMix::HalfIndustrial,
        MinerMix::MostlyIndustrial,
    ];

    /// The mixes of the study's miner-mix table, in table order.
    pub const STUDY: [MinerMix; 3] = [MinerMix::Zipf, MinerMix::Medium, MinerMix::MediumFewBig];

    /// Name in tables and on the command line.
    pub fn name(&self) -> &'static str {
        match self {
            MinerMix::Home => "home",
            MinerMix::HalfIndustrial => "50% industrial",
            MinerMix::MostlyIndustrial => "90% industrial",
            MinerMix::AlmostAllIndustrial => "99% industrial",
            MinerMix::Zipf => "zipf",
            MinerMix::Medium => "medium",
            MinerMix::MediumFewBig => "medium, few big",
        }
    }

    /// The share of the hashrate on industrial machines in the home and
    /// farm mixes; the other mixes say their own split.
    pub fn industrial_fraction(&self) -> f64 {
        match self {
            MinerMix::HalfIndustrial => 0.5,
            MinerMix::MostlyIndustrial => 0.9,
            MinerMix::AlmostAllIndustrial => 0.99,
            MinerMix::MediumFewBig => MEDIUM_MIX_BIG_FRACTION,
            MinerMix::Home | MinerMix::Zipf | MinerMix::Medium => 0.0,
        }
    }

    /// The classes making up `hashrate`, every miner at `level`.
    pub fn classes(&self, hashrate: f64, level: u32) -> Vec<MinerClass> {
        match self {
            MinerMix::Home
            | MinerMix::HalfIndustrial
            | MinerMix::MostlyIndustrial
            | MinerMix::AlmostAllIndustrial => self.home_and_farm_classes(hashrate, level),
            MinerMix::Zipf => zipf_classes(hashrate, level),
            MinerMix::Medium => medium_classes(hashrate, 0.0, level),
            MinerMix::MediumFewBig => medium_classes(hashrate, MEDIUM_MIX_BIG_FRACTION, level),
        }
    }

    /// The home and farm mixes: home miners and an industrial part split
    /// equally between 1 PH/s and 10 PH/s machines.
    fn home_and_farm_classes(&self, hashrate: f64, level: u32) -> Vec<MinerClass> {
        let industrial = hashrate * self.industrial_fraction();
        let home = hashrate - industrial;
        let parts = [
            ("home 1.2 TH/s", MinerKind::Home, HOME_MINER_HASHRATE, home),
            (
                "farm 1 PH/s",
                MinerKind::Industrial,
                SMALL_FARM_HASHRATE,
                industrial / 2.0,
            ),
            (
                "farm 10 PH/s",
                MinerKind::Industrial,
                LARGE_FARM_HASHRATE,
                industrial / 2.0,
            ),
        ];
        let mut classes = Vec::with_capacity(parts.len());
        parts
            .into_iter()
            .filter(|(_, _, _, part_hashrate)| *part_hashrate > 0.0)
            .for_each(|(name, kind, machine_hashrate, part_hashrate)| {
                classes.push(MinerClass {
                    name: name.to_string(),
                    kind,
                    hashrate: machine_hashrate,
                    count: part_hashrate / machine_hashrate,
                    follows_node_default: true,
                    on_carrying_node: true,
                    level,
                });
            });
        classes
    }
}

/// What kind a mix miner of `hashrate` is: home below 10 TH/s, medium below
/// 1 PH/s, industrial from 1 PH/s.
pub fn kind_of(hashrate: f64) -> MinerKind {
    if hashrate < MINI_THRESHOLD_HASHRATE {
        MinerKind::Home
    } else if hashrate < SMALL_FARM_HASHRATE {
        MinerKind::Medium
    } else {
        MinerKind::Industrial
    }
}

/// A class of `count` identical mix miners holding `total` hashes a second
/// between them.
fn mix_class(name: String, total: f64, count: f64, level: u32) -> MinerClass {
    let hashrate = total / count;
    MinerClass {
        name,
        kind: kind_of(hashrate),
        hashrate,
        count,
        follows_node_default: true,
        on_carrying_node: true,
        level,
    }
}

/// The smallest number of Zipf miners whose hashrate reaches `hashrate`
/// when the smallest is a home miner: `N` with `home * N * H_N` at least
/// `hashrate`, `H_N` the `N`-th harmonic number. Returns `N` and `H_N`.
pub fn zipf_miner_count(hashrate: f64) -> (usize, f64) {
    let mut count = 1;
    let mut harmonic = 1.0;
    while HOME_MINER_HASHRATE * count as f64 * harmonic < hashrate {
        count += 1;
        harmonic += 1.0 / count as f64;
    }
    (count, harmonic)
}

/// Zipf's spread: `N` miners, the `k`-th largest with `h_1 / k`, `h_1` set
/// so they add up to `hashrate` exactly (the smallest is then a little
/// above a home miner). The largest ranks get a class each; above them the
/// ranks are grouped by a constant factor, each group a class at its ranks'
/// mean hashrate.
fn zipf_classes(hashrate: f64, level: u32) -> Vec<MinerClass> {
    let mut classes = Vec::with_capacity(2 * ZIPF_SINGLE_RANKS);
    if hashrate > 0.0 {
        let (count, harmonic) = zipf_miner_count(hashrate);
        let largest = hashrate / harmonic;
        let mut first_rank = 1;
        while first_rank <= count {
            let last_rank = if first_rank <= ZIPF_SINGLE_RANKS {
                first_rank
            } else {
                ((first_rank as f64 * ZIPF_GROUP_FACTOR) as usize).min(count)
            };
            let group_total: f64 = (first_rank..=last_rank)
                .map(|rank| largest / rank as f64)
                .sum();
            let ranks = (last_rank - first_rank + 1) as f64;
            classes.push(mix_class(
                format!("zipf ranks {first_rank}-{last_rank}"),
                group_total,
                ranks,
                level,
            ));
            first_rank = last_rank + 1;
        }
    }
    classes
}

/// The medium mixes: a tenth home miners, `big_fraction` on 1 PH/s farms
/// and the rest split equally among the five medium sizes.
fn medium_classes(hashrate: f64, big_fraction: f64, level: u32) -> Vec<MinerClass> {
    let home = hashrate * MEDIUM_MIX_HOME_FRACTION;
    let big = hashrate * big_fraction;
    let medium = (hashrate - home - big) / MEDIUM_MINER_HASHRATES.len() as f64;
    let mut classes = Vec::with_capacity(MEDIUM_MINER_HASHRATES.len() + 2);
    if home > 0.0 {
        classes.push(mix_class(
            "home 1.2 TH/s".to_string(),
            home,
            home / HOME_MINER_HASHRATE,
            level,
        ));
    }
    if medium > 0.0 {
        MEDIUM_MINER_HASHRATES.iter().for_each(|size| {
            classes.push(mix_class(
                format!("medium {:.0} TH/s", size / 1.0e12),
                medium,
                medium / size,
                level,
            ));
        });
    }
    if big > 0.0 {
        classes.push(mix_class(
            "farm 1 PH/s".to_string(),
            big,
            big / SMALL_FARM_HASHRATE,
            level,
        ));
    }
    classes
}

#[cfg(test)]
mod tests;
