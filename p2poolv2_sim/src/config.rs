// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Configuration loading for the sim binary.
//!
//! Uses `#[serde(flatten)]` to deserialize both the regular node config and
//! the sim-specific `[sim]` section from a single TOML file.

use p2poolv2_config::{Config, SimConfig};
use serde::Deserialize;

/// Combined config that loads the regular node config plus the `[sim]` section.
#[derive(Debug, Deserialize, Clone)]
pub struct SimNodeConfig {
    #[serde(flatten)]
    pub node: Config,
    pub sim: SimConfig,
}

impl SimNodeConfig {
    /// Load from a TOML config file path.
    ///
    /// Applies the same Bitcoin Cash adjustments as `Config::load`: on
    /// `chain = "bitcoin-cash"`, `stratum.network` becomes the network
    /// `bitcoin_cash_network` implies and the DNS seeds are off.
    pub fn load(path: &str) -> Result<Self, config::ConfigError> {
        let loaded: Self = config::Config::builder()
            .add_source(config::File::with_name(path))
            // Nesting uses `__` so that a single `_` stays part of a field name.
            // With a single-underscore separator, P2POOL_STORE_PPLNS_TTL_DAYS
            // addresses store.pplns.ttl.days, which does not exist, and the
            // override is silently discarded. See the env_override tests.
            .add_source(
                config::Environment::with_prefix("P2POOL")
                    .prefix_separator("_")
                    .separator("__"),
            )
            .build()?
            .try_deserialize()?;
        Ok(Self {
            node: loaded
                .node
                .with_bitcoin_cash_network_derived()
                .with_bitcoin_cash_dns_seed_off(),
            sim: loaded.sim,
        })
    }
}
