// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The Umbrel apps in `packaging/umbrel`, loaded the way Umbrel runs them.
//!
//! Umbrel starts an app's `docker-compose.yml` with the variables its
//! dependencies export, and the node in the container reads the app's
//! `p2pool.toml` with the compose file's `P2POOL_*` variables on top. These
//! tests render that config from each app's defaults and load it with
//! `Config::load`, as the docker sample tests in `src/lib.rs` do for
//! `config-docker.sample.toml`. The compose and manifest files are read line
//! by line, so keep their `environment`, `ports` and manifest lists in the
//! block form they use now.

use bitcoin::Network;
use p2poolv2_config::{
    BchShareChain, BitcoinCashNetwork, Chain, Config, DEFAULT_BITCOIN_CASH_DONATION,
};
use p2poolv2_wallet::bch_share::BchShareAddress;

struct App {
    id: &'static str,
    manifest: &'static str,
    compose: &'static str,
}

const BCH_P2POOL: App = App {
    id: "bch-p2pool",
    manifest: include_str!("../../packaging/umbrel/bch-p2pool/umbrel-app.yml"),
    compose: include_str!("../../packaging/umbrel/bch-p2pool/docker-compose.yml"),
};

const BCH_P2POOL_EXTERNAL: App = App {
    id: "bch-p2pool-external",
    manifest: include_str!("../../packaging/umbrel/bch-p2pool-external/umbrel-app.yml"),
    compose: include_str!("../../packaging/umbrel/bch-p2pool-external/docker-compose.yml"),
};

const APPS: [App; 2] = [BCH_P2POOL, BCH_P2POOL_EXTERNAL];

const STORE_MANIFEST: &str = include_str!("../../packaging/umbrel/umbrel-app-store.yml");

/// What the Umbrel App Store's Bitcoin Cash Node app exports to the apps
/// that depend on it (getumbrel/umbrel-apps, `bitcoin-cash-node/exports.sh`
/// at 54f519cb). Umbrel derives the RPC password per device; any value
/// stands in for it here.
const BITCOIN_CASH_NODE_EXPORTS: [(&str, &str); 5] = [
    ("APP_BITCOIN_CASH_NODE_NODE_IP", "10.21.21.50"),
    ("APP_BITCOIN_CASH_NODE_RPC_PORT", "8332"),
    ("APP_BITCOIN_CASH_NODE_ZMQ_HASHBLOCK_PORT", "28332"),
    ("APP_BITCOIN_CASH_NODE_RPC_USER", "umbrel"),
    ("APP_BITCOIN_CASH_NODE_RPC_PASS", "derived-per-umbrel"),
];

/// Host ports Umbrel apps that a Bitcoin Cash miner may run beside these
/// already take:
/// - Bitcoin Node (`bitcoin/exports.sh`): RPC 8332, P2P 8333, Tor 8334,
///   whitebind 8335 and ZMQ 28332 to 28336;
/// - Bitcoin Cash Node (`bitcoin-cash-node`): dashboard 3330, P2P 8335, RPC
///   8332 and ZMQ 28332;
/// - P2Poolv2 for Bitcoin (p2poolv2/umbrel-app): Stratum 3333, P2P 6884 and
///   dashboard 46884;
/// - Public Pool: Stratum 2018 and dashboard 2019.
const TAKEN_HOST_PORTS: [u16; 15] = [
    2018, 2019, 3330, 3333, 6884, 8332, 8333, 8334, 8335, 28332, 28333, 28334, 28335, 28336, 46884,
];

/// Umbrel cuts a setting's note at this many characters.
const MAX_SETTING_NOTE_LENGTH: usize = 300;

/// The value of a top-level `key: value` line, without quotes.
fn manifest_value<'a>(manifest: &'a str, key: &str) -> &'a str {
    let prefix = format!("{key}:");
    manifest
        .lines()
        .find_map(|line| line.strip_prefix(prefix.as_str()))
        .unwrap_or_else(|| panic!("no top-level {key}"))
        .trim()
        .trim_matches('"')
}

/// The items of a top-level block list, `key:` followed by `  - item` lines.
fn manifest_list<'a>(manifest: &'a str, key: &str) -> Vec<&'a str> {
    let heading = format!("{key}:");
    manifest
        .lines()
        .skip_while(|line| line.trim_end() != heading)
        .skip(1)
        .take_while(|line| line.starts_with(' '))
        .filter_map(|line| line.strip_prefix("  - "))
        .map(str::trim)
        .collect()
}

/// The names of the settings the manifest offers on the app's settings page.
fn setting_names(manifest: &str) -> Vec<&str> {
    manifest_list(manifest, "environment")
        .into_iter()
        .map(|item| {
            item.strip_prefix("name: ")
                .unwrap_or_else(|| panic!("an environment item starts with its name: {item}"))
        })
        .collect()
}

/// The `- NAME=value` entries of the compose file's environment list.
fn compose_environment(compose: &str) -> Vec<(&str, &str)> {
    compose
        .lines()
        .filter_map(|line| line.trim().strip_prefix("- "))
        .filter_map(|entry| entry.split_once('='))
        .filter(|(name, _)| {
            name.chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        })
        .collect()
}

/// The compose file's `APP_HOST: value` style line for `key`.
fn compose_value<'a>(compose: &'a str, key: &str) -> &'a str {
    let prefix = format!("{key}:");
    compose
        .lines()
        .find_map(|line| line.trim().strip_prefix(prefix.as_str()))
        .unwrap_or_else(|| panic!("compose sets no {key}"))
        .trim()
}

/// The `- "host:container"` port mappings the compose file publishes.
fn published_ports(compose: &str) -> Vec<(u16, u16)> {
    compose
        .lines()
        .filter_map(|line| line.trim().strip_prefix("- \""))
        .filter_map(|entry| entry.strip_suffix('"'))
        .filter_map(|mapping| mapping.split_once(':'))
        .map(|(host, container)| (host.parse().unwrap(), container.parse().unwrap()))
        .collect()
}

/// `value` with every `${NAME}` replaced from `exports`. A variable Umbrel
/// would not set fails the test: it would reach the node as an empty string.
fn render(value: &str, exports: &[(&str, &str)]) -> String {
    let mut rendered = String::new();
    let mut rest = value;
    while let Some(start) = rest.find("${") {
        rendered.push_str(&rest[..start]);
        let end = start
            + rest[start..]
                .find('}')
                .unwrap_or_else(|| panic!("unclosed variable in {value}"));
        let name = &rest[start + 2..end];
        let (_, export) = exports
            .iter()
            .find(|(exported, _)| *exported == name)
            .unwrap_or_else(|| panic!("{name} is not exported to this app"));
        rendered.push_str(export);
        rest = &rest[end + 1..];
    }
    rendered.push_str(rest);
    assert!(!rendered.contains('$'), "unrendered variable in {value}");
    rendered
}

/// The config the app's node starts with: the app's `p2pool.toml` under its
/// compose file's `P2POOL_*` variables, rendered with `exports`, and
/// `settings` from the app's settings page on top of both.
fn load(app: &App, exports: &[(&str, &str)], settings: &[(&str, &str)]) -> Config {
    // P2POOL_CONFIG names the file for clap; it is not a config key.
    let mut variables: Vec<(String, Option<String>)> = compose_environment(app.compose)
        .into_iter()
        .filter(|(name, _)| name.starts_with("P2POOL_") && *name != "P2POOL_CONFIG")
        .filter(|(name, _)| !settings.iter().any(|(setting, _)| setting == name))
        .map(|(name, value)| (name.to_string(), Some(render(value, exports))))
        .collect();
    variables.extend(
        settings
            .iter()
            .map(|(name, value)| (name.to_string(), Some(value.to_string()))),
    );
    let path = format!("../packaging/umbrel/{}/p2pool.toml", app.id);
    temp_env::with_vars(variables, || Config::load(&path))
        .unwrap_or_else(|error| panic!("{} does not load: {error}", app.id))
}

/// The exports Umbrel gives each app: the Bitcoin Cash Node app's to the
/// app that depends on it, none to the external node app.
fn exports(app: &App) -> &'static [(&'static str, &'static str)] {
    match manifest_list(app.manifest, "dependencies").as_slice() {
        ["bitcoin-cash-node"] => &BITCOIN_CASH_NODE_EXPORTS,
        [] => &[],
        other => panic!("{}: no exports known for {other:?}", app.id),
    }
}

fn load_defaults(app: &App) -> Config {
    load(app, exports(app), &[])
}

#[test]
fn bch_p2pool_mines_on_the_bitcoin_cash_node_app_on_mainnet() {
    let config = load_defaults(&BCH_P2POOL);

    assert_eq!(config.stratum.chain, Chain::BitcoinCash);
    assert_eq!(
        config.stratum.bitcoin_cash_network,
        Some(BitcoinCashNetwork::Mainnet)
    );
    assert_eq!(config.stratum.network, Network::Bitcoin);
    assert_eq!(config.stratum.bch_share_chain, BchShareChain::Mini);
    assert_eq!(config.bitcoinrpc.url, "http://10.21.21.50:8332");
    assert_eq!(config.bitcoinrpc.username, "umbrel");
    assert_eq!(config.bitcoinrpc.password, "derived-per-umbrel");
    assert_eq!(config.stratum.zmqpubhashblock, "tcp://10.21.21.50:28332");
    assert!(!config.network.dns_seed);

    let parsed = config.stratum.parse().unwrap();
    assert_eq!(parsed.network, Network::Bitcoin);
    assert_eq!(parsed.donation, Some(DEFAULT_BITCOIN_CASH_DONATION));
    assert!(parsed.bch_miner_address().is_none());
}

#[test]
fn bch_p2pool_external_is_a_chipnet_node_until_bchn_is_set() {
    let config = load_defaults(&BCH_P2POOL_EXTERNAL);

    assert_eq!(config.stratum.chain, Chain::BitcoinCash);
    assert_eq!(
        config.stratum.bitcoin_cash_network,
        Some(BitcoinCashNetwork::Chipnet)
    );
    assert_eq!(config.stratum.network, Network::Testnet);
    assert_eq!(config.stratum.bch_share_chain, BchShareChain::Mini);
    // bchn.example is reserved (RFC 2606) and never resolves, so an
    // unconfigured app stops with a lookup error naming it.
    assert_eq!(config.bitcoinrpc.url, "http://bchn.example:48332");
    assert_eq!(config.stratum.zmqpubhashblock, "tcp://bchn.example:28332");
    assert!(!config.network.dns_seed);

    let parsed = config.stratum.parse().unwrap();
    assert_eq!(parsed.network, Network::Testnet);
    assert_eq!(parsed.donation, Some(DEFAULT_BITCOIN_CASH_DONATION));
}

/// The settings page reaches BCHN on another computer.
#[test]
fn bch_p2pool_external_takes_bchn_from_its_settings() {
    let config = load(
        &BCH_P2POOL_EXTERNAL,
        &[],
        &[
            ("P2POOL_BITCOINRPC__URL", "http://192.0.2.10:48332"),
            ("P2POOL_BITCOINRPC__USERNAME", "miner"),
            ("P2POOL_BITCOINRPC__PASSWORD", "0123"),
            ("P2POOL_STRATUM__ZMQPUBHASHBLOCK", "tcp://192.0.2.10:28332"),
        ],
    );

    assert_eq!(config.bitcoinrpc.url, "http://192.0.2.10:48332");
    assert_eq!(config.bitcoinrpc.username, "miner");
    // A numeric password stays a string, leading zero included.
    assert_eq!(config.bitcoinrpc.password, "0123");
    assert_eq!(config.stratum.zmqpubhashblock, "tcp://192.0.2.10:28332");
}

/// Each network and share chain needs its own store: the node refuses a
/// store holding another share chain.
#[test]
fn umbrel_app_stores_are_named_after_network_and_share_chain() {
    for app in &APPS {
        let config = load_defaults(app);
        let network = match config.stratum.bitcoin_cash_network {
            Some(BitcoinCashNetwork::Mainnet) => "mainnet",
            Some(BitcoinCashNetwork::Chipnet) => "chipnet",
            other => panic!("{}: not a public network: {other:?}", app.id),
        };
        let share_chain = match config.stratum.bch_share_chain {
            BchShareChain::Main => "main",
            BchShareChain::Mini => "mini",
        };
        assert_eq!(
            config.store.path,
            format!("/p2poolv2/data/{network}-{share_chain}"),
            "{}",
            app.id
        );
    }
}

/// Every setting the app offers on its settings page is a config key the
/// node reads, so a value typed there takes effect.
#[test]
fn umbrel_app_settings_reach_the_node_config() {
    for app in &APPS {
        let defaults = load_defaults(app);
        let network = defaults.stratum.bitcoin_cash_network.unwrap();
        let p2p_port = published_ports(app.compose)[1].0;
        let names = setting_names(app.manifest);
        assert!(
            names.contains(&"P2POOL_STRATUM__MINER_ADDRESS"),
            "{}: settings read: {names:?}",
            app.id
        );

        for name in names {
            if name == "RUST_LOG" {
                // Read by the log filter, not the config; the compose file
                // sets the default the settings page offers.
                assert!(
                    compose_environment(app.compose).contains(&("RUST_LOG", "info")),
                    "{}",
                    app.id
                );
                continue;
            }
            let value = match name {
                "P2POOL_STRATUM__MINER_ADDRESS" => {
                    BchShareAddress::new(network.share_network(), [7; 20]).encode()
                }
                "P2POOL_NETWORK__EXTERNAL_ADDRESS" => format!("/ip4/203.0.113.1/tcp/{p2p_port}"),
                "P2POOL_BITCOINRPC__URL" => "http://192.0.2.10:8332".to_string(),
                "P2POOL_BITCOINRPC__USERNAME" => "miner".to_string(),
                "P2POOL_BITCOINRPC__PASSWORD" => "secret".to_string(),
                "P2POOL_STRATUM__ZMQPUBHASHBLOCK" => "tcp://192.0.2.10:28332".to_string(),
                other => panic!("{}: add a check for the {other} setting", app.id),
            };
            let config = load(app, exports(app), &[(name, value.as_str())]);
            let applied = match name {
                "P2POOL_STRATUM__MINER_ADDRESS" => config.stratum.miner_address.clone(),
                "P2POOL_NETWORK__EXTERNAL_ADDRESS" => config.network.external_address.clone(),
                "P2POOL_BITCOINRPC__URL" => Some(config.bitcoinrpc.url.clone()),
                "P2POOL_BITCOINRPC__USERNAME" => Some(config.bitcoinrpc.username.clone()),
                "P2POOL_BITCOINRPC__PASSWORD" => Some(config.bitcoinrpc.password.clone()),
                "P2POOL_STRATUM__ZMQPUBHASHBLOCK" => Some(config.stratum.zmqpubhashblock.clone()),
                _ => unreachable!(),
            };
            assert_eq!(applied, Some(value), "{}: {name}", app.id);
            config
                .stratum
                .parse()
                .unwrap_or_else(|error| panic!("{}: {name}: {error}", app.id));
        }
    }
}

/// The node listens on the ports the app publishes, and Umbrel's app proxy
/// finds its API.
#[test]
fn umbrel_apps_publish_the_ports_their_nodes_listen_on() {
    for app in &APPS {
        let config = load_defaults(app);
        let listen_port: u16 = config
            .network
            .listen_address
            .rsplit('/')
            .next()
            .unwrap()
            .parse()
            .unwrap();

        assert_eq!(
            published_ports(app.compose),
            [
                (config.stratum.port, config.stratum.port),
                (listen_port, listen_port)
            ],
            "{}: Stratum V1 then P2P, on the same port inside and out",
            app.id
        );
        assert_eq!(
            compose_value(app.compose, "APP_HOST"),
            format!("{}_p2pool_1", app.id)
        );
        assert_eq!(
            compose_value(app.compose, "APP_PORT"),
            config.api.port.to_string(),
            "{}",
            app.id
        );
    }
}

/// The apps can run beside each other, the Bitcoin P2Poolv2 app and
/// Umbrel's Bitcoin apps.
#[test]
fn umbrel_app_host_ports_are_free() {
    let mut used: Vec<u16> = Vec::new();
    for app in &APPS {
        let dashboard: u16 = manifest_value(app.manifest, "port").parse().unwrap();
        let host_ports = published_ports(app.compose)
            .into_iter()
            .map(|(host, _)| host)
            .chain([dashboard]);
        for port in host_ports {
            assert!(
                !TAKEN_HOST_PORTS.contains(&port),
                "{}: port {port} is taken",
                app.id
            );
            assert!(!used.contains(&port), "{}: port {port} used twice", app.id);
            used.push(port);
        }
    }
}

/// Umbrel lists only apps whose id starts with the store's id, and takes an
/// app's id from its folder.
#[test]
fn umbrel_apps_belong_to_the_store() {
    let store_id = manifest_value(STORE_MANIFEST, "id");
    for app in &APPS {
        assert_eq!(manifest_value(app.manifest, "id"), app.id);
        assert!(app.id.starts_with(store_id), "{}", app.id);
        assert!(
            compose_value(app.compose, "image")
                .starts_with("ghcr.io/cyberashven/bitcoincash-p2poolv2:"),
            "{}: the fork's image",
            app.id
        );
        for note in app
            .manifest
            .lines()
            .filter_map(|line| line.trim().strip_prefix("note: "))
        {
            assert!(
                note.trim_matches('"').chars().count() <= MAX_SETTING_NOTE_LENGTH,
                "{}: note too long: {note}",
                app.id
            );
        }
    }
    assert_eq!(
        manifest_list(BCH_P2POOL.manifest, "dependencies"),
        ["bitcoin-cash-node"]
    );
    assert!(manifest_list(BCH_P2POOL_EXTERNAL.manifest, "dependencies").is_empty());
}
