// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Miners: the classes a pool is made of, the mixes the tables use and the
//! two tracked miners whose payouts are simulated one by one.

use crate::constants::{
    ASIC_HASHRATE, DEFAULT_LEVEL, HOME_MINER_HASHRATE, LARGE_FARM_HASHRATE, SMALL_FARM_HASHRATE,
};

/// What a miner is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MinerKind {
    /// A home miner of the mix.
    Home,
    /// An industrial machine of the mix.
    Industrial,
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
}

impl MinerMix {
    /// Every mix, in table order.
    pub const ALL: [MinerMix; 3] = [
        MinerMix::Home,
        MinerMix::HalfIndustrial,
        MinerMix::MostlyIndustrial,
    ];

    /// Name in tables and on the command line.
    pub fn name(&self) -> &'static str {
        match self {
            MinerMix::Home => "home",
            MinerMix::HalfIndustrial => "50% industrial",
            MinerMix::MostlyIndustrial => "90% industrial",
        }
    }

    /// The share of the hashrate on industrial machines.
    pub fn industrial_fraction(&self) -> f64 {
        match self {
            MinerMix::Home => 0.0,
            MinerMix::HalfIndustrial => 0.5,
            MinerMix::MostlyIndustrial => 0.9,
        }
    }

    /// The classes making up `hashrate`, every miner at `level`. The
    /// industrial part is split equally between 1 PH/s and 10 PH/s machines.
    pub fn classes(&self, hashrate: f64, level: u32) -> Vec<MinerClass> {
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

#[cfg(test)]
mod tests;
