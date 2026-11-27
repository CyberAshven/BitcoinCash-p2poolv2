// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use p2poolv2_wallet::bch_share::{BchShareAddress, BchShareNetwork};
use temp_env::{with_var, with_vars};
use tokio_test::assert_err;

/// The node key sits beside the store unless `node_key_path` names a
/// file, and an ephemeral node keeps none.
#[test]
fn node_key_file_defaults_beside_the_store() {
    let mut network = NetworkConfig::default();
    assert_eq!(
        network.node_key_file("/var/lib/p2pool/store.db"),
        Some(std::path::PathBuf::from("/var/lib/p2pool").join(DEFAULT_NODE_KEY_FILE))
    );
    assert_eq!(
        network.node_key_file("store.db"),
        Some(std::path::PathBuf::from("").join(DEFAULT_NODE_KEY_FILE))
    );
    network.node_key_path = Some("/etc/p2poolv2/node.key".into());
    assert_eq!(
        network.node_key_file("/var/lib/p2pool/store.db"),
        Some(std::path::PathBuf::from("/etc/p2poolv2/node.key"))
    );
    network.ephemeral_node_key = true;
    assert_eq!(network.node_key_file("/var/lib/p2pool/store.db"), None);
}

/// `proxy_clearnet` dials through `tor_proxy`, so loading refuses it
/// without one, and an onion `onlynet` without `tor_proxy` or
/// `listen_onion`, whose service brings the default Tor SOCKS port. With
/// a proxy, an onion-only node dials onion peers alone, opens no
/// clearnet connection of its own and announces only onion addresses.
#[test]
fn tor_settings_without_a_tor_proxy_are_refused() {
    with_var("P2POOL_NETWORK__PROXY_CLEARNET", Some("true"), || {
        let Err(error) = Config::load("../config.sample.toml") else {
            panic!("proxy_clearnet without tor_proxy was accepted");
        };
        assert!(
            error.to_string().contains("proxy_clearnet needs tor_proxy"),
            "{error}"
        );
    });
    with_vars(
        [
            ("P2POOL_NETWORK__PROXY_CLEARNET", Some("true")),
            ("P2POOL_NETWORK__TOR_PROXY", Some("127.0.0.1:9050")),
        ],
        || {
            let config = Config::load("../config.sample.toml").unwrap();
            assert!(config.network.dials(PeerNetwork::Ipv4));
            assert!(!config.network.dials_clearnet_directly());
        },
    );

    let onlynet: Vec<PeerNetwork> = config::Config::builder()
        .add_source(config::File::from_str(
            "onlynet = [\"onion\"]",
            config::FileFormat::Toml,
        ))
        .build()
        .unwrap()
        .get("onlynet")
        .unwrap();
    assert_eq!(onlynet, vec![PeerNetwork::Onion]);
    let mut config = Config::load("../config.sample.toml").unwrap();
    assert!(!config.network.announces_only_onion());
    config.network.onlynet = Some(onlynet);
    config.network.listen_onion = false;
    assert_err!(config.validate_tor_proxy());
    config.network.listen_onion = true;
    config.validate_tor_proxy().unwrap();
    assert!(
        !config.network.dials(PeerNetwork::Onion),
        "not before the onion service brings the Tor SOCKS port"
    );
    assert!(config.network.onlynet_allows(PeerNetwork::Onion));
    config.network.tor_proxy = Some("127.0.0.1:9050".to_string());
    config.validate_tor_proxy().unwrap();
    assert!(config.network.dials(PeerNetwork::Onion));
    assert!(!config.network.dials(PeerNetwork::Ipv4));
    assert!(!config.network.dials_clearnet_directly());
    assert!(config.network.announces_only_onion());
    config.network.onlynet = Some(vec![PeerNetwork::Onion, PeerNetwork::Ipv6]);
    assert!(!config.network.announces_only_onion());
}

/// The sample config keeps the node's identity by default.
#[test]
fn the_sample_config_keeps_the_node_key() {
    let config = Config::load("../config.sample.toml").unwrap();
    assert!(!config.network.ephemeral_node_key);
    assert!(config.network.node_key_path.is_none());
}

#[test]
fn test_config_builder() {
    let config = Config::load("../config.sample.toml").unwrap();
    let config = config
        .with_listen_address("127.0.0.1:8080".to_string())
        .with_dial_peers(vec![
            "peer1.example.com".to_string(),
            "peer2.example.com".to_string(),
        ])
        .with_max_pending_incoming(10)
        .with_max_pending_outgoing(10)
        .with_max_established_incoming(50)
        .with_max_established_outgoing(50)
        .with_max_established_per_peer(1)
        .with_store_path("/tmp/store".to_string())
        .with_stratum_hostname("stratum.example.com".to_string())
        .with_stratum_port(3333)
        .with_stratum_solo_address("bcrt1qe2qaq0e8qlp425pxytrakala7725dynwhknufr".to_string())
        .with_stratum_zmqpubhashblock("tcp://127.0.0.1:28332".to_string())
        .with_stratum_node_p2p_address(Some("127.0.0.1:8333".to_string()))
        .with_stratum_bootstrap_address("bcrt1qxyz123example456bitcoin789address".to_string())
        .with_start_difficulty(1)
        .with_minimum_difficulty(1)
        .with_maximum_difficulty(Some(100))
        .with_ignore_difficulty(Some(true))
        .with_bitcoinrpc_url("http://localhost:8332".to_string())
        .with_bitcoinrpc_username("testuser".to_string())
        .with_bitcoinrpc_password("testpass".to_string())
        .with_bitcoin_network(bitcoin::Network::Signet)
        .with_stats_dir("./logs/stats".to_string())
        .with_api_hostname("127.0.0.1".to_string())
        .with_api_port(3000)
        .with_api_auth_user(Some("admin".to_string()))
        .with_api_auth_token(Some("secret_token".to_string()));

    assert_eq!(config.network.listen_address, "127.0.0.1:8080");
    assert_eq!(
        config.network.dial_peers,
        vec!["peer1.example.com", "peer2.example.com"]
    );
    assert_eq!(config.store.path, "/tmp/store");

    assert_eq!(config.stratum.hostname, "stratum.example.com");
    assert_eq!(config.stratum.port, 3333);
    assert_eq!(config.stratum.start_difficulty, 1);
    assert_eq!(config.stratum.minimum_difficulty, 1);
    assert_eq!(config.stratum.maximum_difficulty, Some(100));
    assert!(config.stratum.ignore_difficulty.unwrap());
    assert_eq!(
        config.stratum.solo_address,
        Some("bcrt1qe2qaq0e8qlp425pxytrakala7725dynwhknufr".to_string())
    );
    assert_eq!(
        config.stratum.zmqpubhashblock,
        "tcp://127.0.0.1:28332".to_string()
    );
    assert_eq!(
        config.stratum.node_p2p_address,
        Some("127.0.0.1:8333".to_string())
    );
    assert_eq!(
        config.stratum.bootstrap_address.to_string(),
        "bcrt1qxyz123example456bitcoin789address".to_string()
    );

    assert_eq!(config.bitcoinrpc.url, "http://localhost:8332");
    assert_eq!(config.bitcoinrpc.username, "testuser");
    assert_eq!(config.bitcoinrpc.password, "testpass");

    assert_eq!(config.network.max_pending_incoming, 10);
    assert_eq!(config.network.max_pending_outgoing, 10);
    assert_eq!(config.network.max_established_incoming, 50);
    assert_eq!(config.network.max_established_outgoing, 50);
    assert_eq!(config.network.max_established_per_peer, 1);
    assert_eq!(config.logging.stats_dir, "./logs/stats");

    // Test API config
    assert_eq!(config.api.hostname, "127.0.0.1");
    assert_eq!(config.api.port, 3000);
    assert_eq!(config.api.auth_user, Some("admin".to_string()));
    assert_eq!(config.api.auth_token, Some("secret_token".to_string()));

    // Test values from config.sample.toml
    assert_eq!(config.store.pplns_ttl_days, 7);
}

#[test]
fn test_config_store_background_settings() {
    let config = Config::load("../config.sample.toml")
        .unwrap()
        .with_pplns_ttl_days(9);

    assert_eq!(config.store.pplns_ttl_days, 9);
}

/// A node prunes whole weak shares at one payout depth unless its config
/// keeps them, as an explorer or an archive does.
#[test]
fn whole_weak_shares_are_pruned_unless_the_store_keeps_them() {
    let config = Config::load("../config.sample.toml").unwrap();
    assert!(!config.store.keep_whole_weak_shares, "sample default");

    with_var("P2POOL_STORE__KEEP_WHOLE_WEAK_SHARES", Some("true"), || {
        let config = Config::load("../config.sample.toml").unwrap();
        assert!(config.store.keep_whole_weak_shares, "archive setting");
    });
}

/// Characterisation tests for environment-variable overrides.
///
/// `Config::load` layers `config::Environment` with prefix `P2POOL` and
/// separator `_` over the TOML file. The separator is also what appears
/// inside many field names, so which keys are reachable is not obvious.
/// These tests pin the behaviour, because getting it wrong starts the node
/// with the wrong settings rather than failing.
#[test]
fn env_override_applies_to_a_nested_string_field() {
    with_var("P2POOL_LOGGING__LEVEL", Some("trace"), || {
        let config = Config::load("../config.sample.toml").unwrap();
        assert_eq!(config.logging.level, "trace");
    });
}

/// Integers arrive from the environment as strings and have to be coerced.
#[test]
fn env_override_applies_to_a_nested_integer_field() {
    with_var("P2POOL_API__PORT", Some("59999"), || {
        let config = Config::load("../config.sample.toml").unwrap();
        assert_eq!(config.api.port, 59999);
    });
}

/// Booleans are coerced from strings the same way integers are.
#[test]
fn env_override_applies_to_a_nested_bool_field() {
    with_var("P2POOL_LOGGING__CONSOLE", Some("false"), || {
        let config = Config::load("../config.sample.toml").unwrap();
        assert_eq!(config.logging.console, Some(false));
    });
}

/// An Option<String> field is populated, not left None.
#[test]
fn env_override_applies_to_a_nested_option_field() {
    with_var("P2POOL_LOGGING__FILE", Some("/tmp/env.log"), || {
        let config = Config::load("../config.sample.toml").unwrap();
        assert_eq!(config.logging.file, Some("/tmp/env.log".to_string()));
    });
}

/// A field whose own name contains an underscore is reachable, because the
/// nesting separator is `__` and single underscores stay part of the key.
///
/// With a single-underscore separator this override was silently discarded:
/// `P2POOL_STORE_PPLNS_TTL_DAYS` addressed `store.pplns.ttl.days`, which
/// does not exist, so the file value survived and nothing reported it.
#[test]
fn env_override_reaches_a_field_whose_name_contains_an_underscore() {
    with_var("P2POOL_STORE__PPLNS_TTL_DAYS", Some("99"), || {
        let config = Config::load("../config.sample.toml").unwrap();
        assert_eq!(config.store.pplns_ttl_days, 99);
    });
}

/// The listen address is the other field docker-compose sets that a
/// single-underscore separator could not reach.
#[test]
fn env_override_reaches_the_network_listen_address() {
    with_var(
        "P2POOL_NETWORK__LISTEN_ADDRESS",
        Some("/ip4/10.0.0.1/tcp/6884"),
        || {
            let config = Config::load("../config.sample.toml").unwrap();
            assert_eq!(config.network.listen_address, "/ip4/10.0.0.1/tcp/6884");
        },
    );
}

/// dial_peers is optional, so it can be commented out of the config and
/// the node relies on DNS seeds alone.
#[test]
fn dial_peers_defaults_to_empty_when_absent() {
    let config = Config::load("../config.sample.toml").unwrap();
    assert!(config.network.dial_peers.is_empty());
}

/// DNS seeds are on unless the operator turns them off.
#[test]
fn dns_seed_defaults_to_true_when_absent() {
    let config = Config::load("../config-bitcoin.sample.toml").unwrap();
    assert!(config.network.dns_seed);
}

/// The sample configs leave target_outgoing_peers commented out, so a
/// node keeps 16 outgoing peers unless its operator says otherwise.
#[test]
fn target_outgoing_peers_defaults_to_16_when_absent() {
    let config = Config::load("../config.sample.toml").unwrap();
    assert_eq!(config.network.target_outgoing_peers, 16);
}

/// The sample config leaves `[electrum]` commented out, so the server is
/// off and every field takes its default.
#[test]
fn electrum_defaults_to_disabled_when_section_absent() {
    let config = Config::load("../config.sample.toml").unwrap();
    assert!(!config.electrum.enabled);
    assert_eq!(config.electrum.hostname, "127.0.0.1");
    assert_eq!(config.electrum.port, 50001);
    assert_eq!(config.electrum.max_connections, 64);
}

#[test]
fn electrum_section_values_are_parsed() {
    let electrum: ElectrumConfig = config::Config::builder()
        .add_source(config::File::from_str(
            "enabled = true\nhostname = \"127.0.0.2\"\nport = 50101\nmax_connections = 8\n",
            config::FileFormat::Toml,
        ))
        .build()
        .unwrap()
        .try_deserialize()
        .unwrap();
    assert!(electrum.enabled);
    assert_eq!(electrum.hostname, "127.0.0.2");
    assert_eq!(electrum.port, 50101);
    assert_eq!(electrum.max_connections, 8);
}

#[test]
fn env_override_sets_electrum_port() {
    with_var("P2POOL_ELECTRUM__PORT", Some("50999"), || {
        let config = Config::load("../config.sample.toml").unwrap();
        assert_eq!(config.electrum.port, 50999);
    });
}

#[test]
fn test_config_builder_sets_electrum_fields() {
    let config = Config::load("../config.sample.toml")
        .unwrap()
        .with_electrum_enabled(true)
        .with_electrum_hostname("127.0.0.3".to_string())
        .with_electrum_port(50202);
    assert!(config.electrum.enabled);
    assert_eq!(config.electrum.hostname, "127.0.0.3");
    assert_eq!(config.electrum.port, 50202);
}

#[test]
fn env_override_disables_dns_seed() {
    with_var("P2POOL_NETWORK__DNS_SEED", Some("false"), || {
        let config = Config::load("../config.sample.toml").unwrap();
        assert!(!config.network.dns_seed);
    });
}

/// A Bitcoin Cash node keeps `dns_seed` as set; its seeds are picked by
/// `bitcoin_cash_network`, so it never gets Bitcoin's.
#[test]
fn bitcoin_cash_load_keeps_dns_seed_as_set() {
    for (value, expected) in [("true", true), ("false", false)] {
        with_vars(
            [
                ("P2POOL_STRATUM__CHAIN", Some("bitcoin-cash")),
                ("P2POOL_STRATUM__BITCOIN_CASH_NETWORK", Some("mainnet")),
                ("P2POOL_NETWORK__DNS_SEED", Some(value)),
            ],
            || {
                let config = Config::load("../config.sample.toml").unwrap();
                assert_eq!(config.stratum.network, Network::Bitcoin);
                assert_eq!(config.network.dns_seed, expected);
            },
        );
    }
}

/// `dns_seed_hosts` is empty unless the file lists hostnames.
#[test]
fn dns_seed_hosts_load_from_the_file() {
    let config = Config::load("../config.sample.toml").unwrap();
    assert!(config.network.dns_seed_hosts.is_empty());

    let sample = std::fs::read_to_string("../config.sample.toml")
        .unwrap()
        .replace("\r\n", "\n");
    let with_seeds = sample.replacen(
        "\n[network]\n",
        "\n[network]\ndns_seed_hosts = [\"seed.example.org\"]\n",
        1,
    );
    assert_ne!(with_seeds, sample, "the sample has a [network] table");
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    std::fs::write(&path, with_seeds).unwrap();

    let config = Config::load(path.to_str().unwrap()).unwrap();
    assert_eq!(config.network.dns_seed_hosts, vec!["seed.example.org"]);
}

/// A single-underscore key no longer resolves, so an operator carrying the
/// old spelling forward gets the file value rather than a silent surprise
/// in a different field.
#[test]
fn env_override_ignores_the_old_single_underscore_spelling() {
    with_var("P2POOL_API_PORT", Some("59999"), || {
        let config = Config::load("../config.sample.toml").unwrap();
        assert_ne!(config.api.port, 59999);
    });
}

/// The exact keys docker/docker-compose.p2poolv2.yml sets, asserted
/// together so the container and the config schema cannot drift apart.
///
/// Compose deliberately does not set `bitcoinrpc.url`. That is the
/// operator's, from the mounted config.toml: an override here would
/// silently contradict that file and fail late, against the wrong BCHN.
/// `env_override_reaches_the_network_listen_address` covers the one key
/// that used to be unreachable and now applies.
#[test]
fn env_overrides_used_by_docker_compose_all_apply() {
    temp_env::with_vars(
        [
            ("P2POOL_STRATUM__CHAIN", Some("bitcoin-cash")),
            ("P2POOL_STRATUM__BITCOIN_CASH_NETWORK", Some("regtest")),
            ("P2POOL_STORE__PATH", Some("/p2poolv2/data/regtest")),
            ("P2POOL_STRATUM__PORT", Some("3333")),
            ("P2POOL_API__PORT", Some("46884")),
            (
                "P2POOL_NETWORK__LISTEN_ADDRESS",
                Some("/ip4/0.0.0.0/tcp/6884"),
            ),
        ],
        || {
            let config = Config::load("../config.sample.toml").unwrap();
            assert_eq!(config.stratum.chain, Chain::BitcoinCash);
            assert_eq!(
                config.stratum.bitcoin_cash_network,
                Some(BitcoinCashNetwork::Regtest)
            );
            assert_eq!(config.stratum.network, Network::Regtest);
            assert_eq!(config.store.path, "/p2poolv2/data/regtest");
            assert_eq!(config.stratum.port, 3333);
            assert_eq!(config.api.port, 46884);
            assert_eq!(config.network.listen_address, "/ip4/0.0.0.0/tcp/6884");
        },
    );
}

/// The bitcoind endpoint must stay a passthrough in compose, never a
/// forced value.
#[test]
fn docker_compose_passes_the_bitcoind_endpoint_through_without_a_default() {
    let entries: Vec<&str> = include_str!("../../docker/docker-compose.p2poolv2.yml")
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("- P2POOL_"))
        .collect();
    let endpoint = entries
        .iter()
        .find(|line| line.contains("BITCOINRPC__URL"))
        .expect("compose should pass the bitcoind endpoint through");
    assert_eq!(
        *endpoint, "- P2POOL_BITCOINRPC__URL",
        "must be a bare passthrough, with no value or default"
    );
}

/// NETWORK drives both the Bitcoin Cash network and the store directory,
/// and is required rather than defaulted.
#[test]
fn docker_compose_requires_network_and_uses_it_for_bitcoin_cash_network_and_store() {
    let entries: Vec<&str> = include_str!("../../docker/docker-compose.p2poolv2.yml")
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("- P2POOL_"))
        .collect();
    let network = entries
        .iter()
        .find(|line| line.contains("STRATUM__BITCOIN_CASH_NETWORK"))
        .expect("compose should set bitcoin_cash_network from NETWORK");
    assert!(
        network.contains("${NETWORK:?"),
        "NETWORK must be required, not defaulted: {network}"
    );
    let store = entries
        .iter()
        .find(|line| line.contains("STORE__PATH"))
        .expect("compose should set the store path");
    assert!(
        store.contains("${NETWORK}"),
        "store path must follow the same NETWORK as the node: {store}"
    );
}

/// The compose stack is a Bitcoin Cash stack whatever the mounted config
/// says.
#[test]
fn docker_compose_sets_the_bitcoin_cash_chain() {
    let sets_chain = include_str!("../../docker/docker-compose.p2poolv2.yml")
        .lines()
        .map(str::trim)
        .any(|line| line == "- P2POOL_STRATUM__CHAIN=bitcoin-cash");
    assert!(sets_chain, "compose should set chain = bitcoin-cash");
}

/// Chipnet and mainnet BCHN never ship a known RPC password:
/// docker/bchn/entrypoint.sh adds one at start. Only regtest keeps the
/// public throwaway login, which config-docker.sample.toml uses.
#[test]
fn docker_bchn_ships_an_rpc_password_only_for_regtest() {
    let sets_password = |conf: &str| {
        conf.lines()
            .any(|line| line.trim_start().starts_with("rpcpassword"))
    };
    assert!(!sets_password(include_str!(
        "../../docker/bchn/bitcoin-chipnet.conf"
    )));
    assert!(!sets_password(include_str!(
        "../../docker/bchn/bitcoin-mainnet.conf"
    )));
    assert!(
        include_str!("../../docker/bchn/bitcoin-regtest.conf")
            .lines()
            .any(|line| line.trim() == "rpcpassword=p2pool")
    );
    with_var("P2POOL_BITCOINRPC__PASSWORD", None::<&str>, || {
        let config = Config::load("../config-docker.sample.toml").unwrap();
        assert_eq!(config.bitcoinrpc.password, "p2pool");
    });
}

/// The node gets BCHN's password from the volume BCHN writes it to,
/// mounted read-only, as the existing P2POOL_BITCOINRPC__PASSWORD
/// override, under the file name entrypoint.sh uses.
#[test]
fn docker_compose_hands_the_bchn_rpc_password_to_the_node() {
    let compose = include_str!("../../docker/docker-compose.p2poolv2.yml");
    assert!(
        compose
            .lines()
            .any(|line| line.trim() == "- bchn_rpc:/run/bchn-rpc:ro")
    );
    assert!(
        compose
            .contains("secret=\"/run/bchn-rpc/password-$${P2POOL_STRATUM__BITCOIN_CASH_NETWORK}\"")
    );
    assert!(compose.contains("export P2POOL_BITCOINRPC__PASSWORD"));
    let entrypoint = include_str!("../../docker/bchn/entrypoint.sh");
    assert!(entrypoint.contains("RPC_SECRET_DIR=${RPC_SECRET_DIR:-/run/bchn-rpc}"));
    assert!(entrypoint.contains("secret=\"$RPC_SECRET_DIR/password-$NETWORK\""));
    let compose = include_str!("../../docker/docker-compose.yml");
    assert!(
        compose
            .lines()
            .any(|line| line.trim() == "- bchn_rpc:/run/bchn-rpc")
    );
}

/// The override the compose stack sets replaces the sample's regtest
/// password.
#[test]
fn env_override_replaces_the_docker_sample_rpc_password() {
    with_var(
        "P2POOL_BITCOINRPC__PASSWORD",
        Some("4f1c0a9e7d2b6c38"),
        || {
            let config = Config::load("../config-docker.sample.toml").unwrap();
            assert_eq!(config.bitcoinrpc.password, "4f1c0a9e7d2b6c38");
        },
    );
}

/// config-docker.sample.toml is the image's default config and the
/// compose stack's default mount: a Bitcoin Cash regtest node.
#[test]
fn docker_sample_config_is_a_bitcoin_cash_regtest_node() {
    let config = Config::load("../config-docker.sample.toml").unwrap();
    assert_eq!(config.stratum.chain, Chain::BitcoinCash);
    assert_eq!(
        config.stratum.bitcoin_cash_network,
        Some(BitcoinCashNetwork::Regtest)
    );
    assert!(config.network.dns_seed);
}

/// Its CashAddr bootstrap address and the default donation parse on
/// regtest, so the node starts from the sample unedited.
#[test]
fn docker_sample_config_parses() {
    let config = Config::load("../config-docker.sample.toml").unwrap();
    let parsed = config.stratum.parse().unwrap();
    assert_eq!(parsed.network, Network::Regtest);
    assert_eq!(parsed.version_mask, DEFAULT_VERSION_MASK);
}

#[test]
fn node_chain_names_match_bchn_then_bchd() {
    assert_eq!(
        BitcoinCashNetwork::Mainnet.node_chain_names(),
        ["main", "mainnet"]
    );
    assert_eq!(
        BitcoinCashNetwork::Chipnet.node_chain_names(),
        ["chip", "chipnet"]
    );
    assert_eq!(BitcoinCashNetwork::Regtest.node_chain_names(), ["regtest"]);
}

/// BCHN `src/chainparams.cpp`, `CMainParams` checkpoint "Axion
/// activation", the first block after eCash split off.
#[test]
fn the_mainnet_bitcoin_cash_block_is_bchn_axion_checkpoint() {
    assert_eq!(
        BitcoinCashNetwork::Mainnet.bch_only_block(),
        Some((
            661_648,
            "0000000000000000029e471c41818d24b8b74c911071c4ef0b4a0509f9b5a8ce"
        ))
    );
}

/// BCHN `src/chainparams.cpp`, `CChipNetParams` checkpoint "Fork block
/// for chipnet", which testnet4 does not have.
#[test]
fn the_chipnet_bitcoin_cash_block_is_the_chipnet_fork_block() {
    assert_eq!(
        BitcoinCashNetwork::Chipnet.bch_only_block(),
        Some((
            115_252,
            "00000000040ba9641ba98a37b2e5ceead38e4e2930ac8f145c8094f94c708727"
        ))
    );
}

#[test]
fn regtest_has_no_bitcoin_cash_only_block() {
    assert_eq!(BitcoinCashNetwork::Regtest.bch_only_block(), None);
}

/// BCHN `src/chainparams.cpp`, `CScaleNetParams` checkpoint 10,000, the
/// height scalenet is reorged back to.
#[test]
fn the_scalenet_bitcoin_cash_block_is_its_reorg_floor_checkpoint() {
    assert_eq!(
        BitcoinCashNetwork::Scalenet.bch_only_block(),
        Some((
            10_000,
            "00000000b711dc753130e5083888d106f99b920b1b8a492eb5ac41d40e482905"
        ))
    );
    assert_eq!(BitcoinCashNetwork::Scalenet.node_chain_names(), ["scale"]);
}

/// Scalenet takes BCHN's test network address forms: the `bchtest`
/// CashAddr prefix and testnet's base58 versions and proof-of-work limit.
#[test]
fn scalenet_uses_the_test_network_address_forms() {
    assert_eq!(BitcoinCashNetwork::Scalenet.cashaddr_prefix(), "bchtest");
    assert_eq!(
        BitcoinCashNetwork::Scalenet.legacy_network(),
        Network::Testnet
    );
    assert_eq!(
        BitcoinCashNetwork::Scalenet.share_network(),
        BchShareNetwork::Scalenet
    );
    assert_eq!(
        BitcoinCashNetwork::from_share_network(BchShareNetwork::Scalenet),
        BitcoinCashNetwork::Scalenet
    );
}

#[test]
fn scalenet_is_read_from_the_config() {
    let config: Config = config::Config::builder()
        .add_source(config::File::with_name("../config.sample.toml"))
        .add_source(config::File::from_str(
            r#"
                [stratum]
                chain = "bitcoin-cash"
                bitcoin_cash_network = "scalenet"
                "#,
            config::FileFormat::Toml,
        ))
        .build()
        .unwrap()
        .try_deserialize()
        .unwrap();

    assert_eq!(
        config.stratum.bitcoin_cash_network,
        Some(BitcoinCashNetwork::Scalenet)
    );
}

/// The default sample is a Bitcoin Cash chipnet node that names no
/// Bitcoin network and parses as shipped.
#[test]
fn the_default_sample_is_a_bitcoin_cash_chipnet_node() {
    let config = Config::load("../config.sample.toml").unwrap();
    assert_eq!(config.stratum.chain, Chain::BitcoinCash);
    assert_eq!(
        config.stratum.bitcoin_cash_network,
        Some(BitcoinCashNetwork::Chipnet)
    );
    assert_eq!(config.stratum.network, Network::Testnet);
    assert!(config.network.dns_seed);
    assert_eq!(config.bitcoinrpc.url, "http://127.0.0.1:48332");

    let parsed = config.stratum.parse().unwrap();
    assert_eq!(parsed.donation, Some(DEFAULT_BITCOIN_CASH_DONATION));
    assert!(parsed.bootstrap_address_parsed.is_some());
}

/// Upstream's Bitcoin sample is kept, for Bitcoin mode.
#[test]
fn the_bitcoin_sample_is_a_bitcoin_signet_node() {
    let config = Config::load("../config-bitcoin.sample.toml").unwrap();
    assert_eq!(config.stratum.chain, Chain::Bitcoin);
    assert_eq!(config.stratum.network, Network::Signet);
    assert!(config.stratum.parse().is_ok());
}

/// A Bitcoin config must name its network; only Bitcoin Cash derives it.
#[test]
fn a_bitcoin_config_without_a_network_is_refused() {
    let sample = std::fs::read_to_string("../config-bitcoin.sample.toml").unwrap();
    let without_network: String = sample
        .lines()
        .filter(|line| !line.starts_with("network = "))
        .collect::<Vec<_>>()
        .join("\n");
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    std::fs::write(&path, without_network).unwrap();

    let result = Config::load(path.to_str().unwrap());

    let Err(error) = result else {
        panic!("a Bitcoin config without network loaded");
    };
    assert!(error.to_string().contains("network is required"), "{error}");
}

/// The main and mini share chains merged, so a config still choosing one
/// is refused at start-up instead of being read as the merged chain.
#[test]
fn a_config_that_still_sets_bch_share_chain_is_refused() {
    let sample = std::fs::read_to_string("../config.sample.toml")
        .unwrap()
        .replace("\r\n", "\n");
    let with_share_chain = sample.replace(
        "\nbitcoin_cash_network = \"chipnet\"\n",
        "\nbitcoin_cash_network = \"chipnet\"\nbch_share_chain = \"main\"\n",
    );
    assert_ne!(with_share_chain, sample);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    std::fs::write(&path, with_share_chain).unwrap();

    let result = Config::load(path.to_str().unwrap());

    let Err(error) = result else {
        panic!("a config setting bch_share_chain loaded");
    };
    assert!(
        error
            .to_string()
            .contains("main and mini Bitcoin Cash share chains merged into one"),
        "{error}"
    );
}

/// The compose stack once set the share chain from the environment; that
/// is refused the same way.
#[test]
fn a_bch_share_chain_environment_override_is_refused() {
    temp_env::with_var("P2POOL_STRATUM__BCH_SHARE_CHAIN", Some("mini"), || {
        let Err(error) = Config::load("../config.sample.toml") else {
            panic!("P2POOL_STRATUM__BCH_SHARE_CHAIN was accepted");
        };
        assert!(
            error
                .to_string()
                .contains("bch_share_chain is no longer a setting"),
            "{error}"
        );
    });
}

#[test]
fn a_config_without_submit_nodes_has_no_extra_nodes() {
    let config = Config::load("../config.sample.toml").unwrap();
    assert!(config.submit_nodes.is_empty());
}

#[test]
fn submit_nodes_are_read_in_file_order() {
    let config: Config = config::Config::builder()
        .add_source(config::File::with_name("../config.sample.toml"))
        .add_source(config::File::from_str(
            r#"
                [[submit_nodes]]
                url = "http://bchn-a.example:8332"
                username = "a"
                password = "secret-a"

                [[submit_nodes]]
                url = "http://bchn-b.example:8332"
                username = "b"
                password = "secret-b"
                "#,
            config::FileFormat::Toml,
        ))
        .build()
        .unwrap()
        .try_deserialize()
        .unwrap();

    assert_eq!(config.submit_nodes.len(), 2);
    assert_eq!(config.submit_nodes[0].url, "http://bchn-a.example:8332");
    assert_eq!(config.submit_nodes[1].username, "b");
    assert_eq!(config.submit_nodes[1].password, "secret-b");
}

/// Both samples offer miners the code's default version rolling mask,
/// BIP 320's general purpose bits.
#[test]
fn sample_config_version_mask_is_the_default() {
    let config = Config::load("../config.sample.toml").unwrap();
    assert_eq!(config.stratum.version_mask, DEFAULT_VERSION_MASK);
}

#[test]
fn test_config_from_env_vars() {
    with_var(
        "P2POOL_BITCOINRPC__URL",
        Some("http://bitcoin-from-env:8332"),
        || {
            // Load config from file first
            let config = Config::load("../config.sample.toml").unwrap();

            // Check that the environment variable overrides the config file value
            assert_eq!(config.bitcoinrpc.url, "http://bitcoin-from-env:8332");
        },
    );
}

#[test]
fn test_default_network_config() {
    let config = NetworkConfig::default();
    assert!(config.listen_address.is_empty());
}

/// A share address belongs to a network exactly when its prefix is the one
/// that network expects, so a network with no prefix can never accept one.
/// Caught at startup: otherwise the node runs and rejects every authorize,
/// forever, for a reason no miner can fix.
///
/// `miner_address` is unset here, which is an ordinary p2poolv2 setup --
/// miners supply their own with `p2p=`. So the network cannot be validated
/// through a configured address; it has to be checked on its own.
#[test]
fn parse_rejects_a_network_with_no_share_address_prefix() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.network = Network::Testnet;
    config.mode = PoolMode::P2poolv2;
    assert!(config.miner_address.is_none());

    let error = config.parse().expect_err("testnet v3 has no share prefix");
    assert!(
        error.message.contains("cannot be used for a share chain"),
        "unexpected message: {}",
        error.message
    );
}

/// Hydrapool builds no share commitment, so it needs no share address and
/// must not be held to a rule that does not apply to it.
#[test]
fn parse_allows_an_unusable_network_in_hydrapool_mode() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.network = Network::Testnet;
    config.mode = PoolMode::Hydrapool;
    config.miner_address = None;

    assert!(config.parse().is_ok());
}

#[test]
fn test_pool_signature_option() {
    // Test with None
    let config = StratumConfig::<Raw>::new_for_test_default();
    assert_eq!(config.pool_signature, None);

    // Test parsing preserves pool_signature
    let parsed = config.parse().unwrap();
    assert_eq!(parsed.pool_signature, None);

    // Test with Some value
    let mut config_with_sig = StratumConfig::<Raw>::new_for_test_default();
    config_with_sig.pool_signature = Some("MyPool/1.0".to_string());
    assert_eq!(
        config_with_sig.pool_signature,
        Some("MyPool/1.0".to_string())
    );

    // Test parsing preserves Some value
    let parsed_with_sig = config_with_sig.parse().unwrap();
    assert_eq!(
        parsed_with_sig.pool_signature,
        Some("MyPool/1.0".to_string())
    );

    // Test sig length limit
    let mut config_with_sig = StratumConfig::<Raw>::new_for_test_default();
    config_with_sig.pool_signature = Some("MyPool/1.0 and some more bytes....".to_string());
    assert_err!(config_with_sig.parse());
}

#[test]
fn test_parse_fails_donation_address_without_amount() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.donation_address = Some("tb1qyazxde6558qj6z3d9np5e6msmrspwpf6k0qggk".to_string());
    config.donation = None;
    let result = config.parse();
    assert_err!(&result);
    assert!(
        result
            .unwrap_err()
            .message
            .contains("donation_address and donation")
    );
}

#[test]
fn test_parse_fails_donation_amount_without_address() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.donation_address = None;
    config.donation = Some(50);
    let result = config.parse();
    assert_err!(&result);
    assert!(
        result
            .unwrap_err()
            .message
            .contains("donation_address and donation")
    );
}

#[test]
fn test_parse_fails_fee_address_without_amount() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.fee_address = Some("tb1qyazxde6558qj6z3d9np5e6msmrspwpf6k0qggk".to_string());
    config.fee = None;
    let result = config.parse();
    assert_err!(&result);
    assert!(result.unwrap_err().message.contains("fee_address and fee"));
}

#[test]
fn test_parse_fails_fee_amount_without_address() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.fee_address = None;
    config.fee = Some(200);
    let result = config.parse();
    assert_err!(&result);
    assert!(result.unwrap_err().message.contains("fee_address and fee"));
}

#[test]
fn test_parse_succeeds_with_both_donation_fields() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.donation_address = Some("tb1qyazxde6558qj6z3d9np5e6msmrspwpf6k0qggk".to_string());
    config.donation = Some(50);
    let parsed = config.parse().unwrap();
    assert!(parsed.donation_address_parsed.is_some());
    assert_eq!(parsed.donation, Some(50));
}

#[test]
fn test_parse_succeeds_with_both_fee_fields() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.fee_address = Some("tb1qyazxde6558qj6z3d9np5e6msmrspwpf6k0qggk".to_string());
    config.fee = Some(200);
    let parsed = config.parse().unwrap();
    assert!(parsed.fee_address_parsed.is_some());
    assert_eq!(parsed.fee, Some(200));
}

#[test]
fn test_parse_succeeds_with_neither_donation_nor_fee() {
    let config = StratumConfig::<Raw>::new_for_test_default();
    assert!(config.donation_address.is_none());
    assert!(config.donation.is_none());
    assert!(config.fee_address.is_none());
    assert!(config.fee.is_none());
    let parsed = config.parse().unwrap();
    assert!(parsed.donation_address_parsed.is_none());
    assert!(parsed.fee_address_parsed.is_none());
}

/// An unset maximum difficulty becomes the default cap, so no session can
/// ask for an unbounded difficulty.
#[test]
fn test_parse_fills_an_unset_maximum_difficulty_with_the_default() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.maximum_difficulty = None;
    let parsed = config.parse().unwrap();
    assert_eq!(parsed.maximum_difficulty, Some(DEFAULT_MAXIMUM_DIFFICULTY));
}

#[test]
fn test_parse_keeps_a_configured_maximum_difficulty() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.maximum_difficulty = Some(u64::MAX);
    let parsed = config.parse().unwrap();
    assert_eq!(parsed.maximum_difficulty, Some(u64::MAX));
}

#[test]
fn test_pool_mode_defaults_to_p2poolv2_when_absent() {
    let config = Config::load("../config.sample.toml").unwrap();
    assert_eq!(config.stratum.mode, PoolMode::P2poolv2);
}

#[test]
fn test_pool_mode_defaults_to_p2poolv2_in_test_helper() {
    let config = StratumConfig::<Raw>::new_for_test_default();
    assert_eq!(config.mode, PoolMode::P2poolv2);
}

#[test]
fn test_pool_mode_survives_parse() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.mode = PoolMode::Hydrapool;
    let parsed = config.parse().unwrap();
    assert_eq!(parsed.mode, PoolMode::Hydrapool);
}

#[test]
fn absent_chain_stays_on_bitcoin() {
    let config = Config::load("../config-bitcoin.sample.toml").unwrap();
    assert_eq!(config.stratum.chain, Chain::Bitcoin);
    assert_eq!(config.stratum.bitcoin_cash_network, None);
}

#[test]
fn bitcoin_cash_chipnet_accepts_a_matching_cashaddr_payout() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.chain = Chain::BitcoinCash;
    config.bitcoin_cash_network = Some(BitcoinCashNetwork::Chipnet);
    config.bootstrap_address = "bchtest:pr6m7j9njldwwzlg9v7v53unlr4jkmx6eyvwc0uz5t".to_string();

    let parsed = config.parse().unwrap();
    assert_eq!(parsed.chain, Chain::BitcoinCash);
    assert_eq!(
        parsed.bitcoin_cash_network,
        Some(BitcoinCashNetwork::Chipnet)
    );
    assert_eq!(
        parsed.bootstrap_address().script_pubkey().as_bytes(),
        [
            0xa9, 0x14, 0xf5, 0xbf, 0x48, 0xb3, 0x97, 0xda, 0xe7, 0x0b, 0xe8, 0x2b, 0x3c, 0xca,
            0x47, 0x93, 0xf8, 0xeb, 0x2b, 0x6c, 0xda, 0xc9, 0x87,
        ]
    );
}

#[test]
fn bitcoin_cash_accepts_a_share_address_and_rejects_a_payout_address() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.chain = Chain::BitcoinCash;
    config.bitcoin_cash_network = Some(BitcoinCashNetwork::Chipnet);
    config.bootstrap_address = "bchtest:pr6m7j9njldwwzlg9v7v53unlr4jkmx6eyvwc0uz5t".to_string();
    config.miner_address =
        Some(BchShareAddress::new(BchShareNetwork::Chipnet, [0x22; 20]).encode());
    let parsed = config.parse().unwrap();
    assert_eq!(
        parsed.bch_miner_address().map(|address| address.hash),
        Some([0x22; 20])
    );
    assert!(parsed.miner_address().is_none());

    let mut payout_as_share = StratumConfig::<Raw>::new_for_test_default();
    payout_as_share.chain = Chain::BitcoinCash;
    payout_as_share.bitcoin_cash_network = Some(BitcoinCashNetwork::Chipnet);
    payout_as_share.bootstrap_address =
        "bchtest:pr6m7j9njldwwzlg9v7v53unlr4jkmx6eyvwc0uz5t".to_string();
    payout_as_share.miner_address =
        Some("bchtest:pr6m7j9njldwwzlg9v7v53unlr4jkmx6eyvwc0uz5t".to_string());
    assert!(payout_as_share.parse().is_err());
}

#[test]
fn bitcoin_cash_rejects_a_bitcoin_witness_payout() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.chain = Chain::BitcoinCash;
    config.bitcoin_cash_network = Some(BitcoinCashNetwork::Regtest);

    let error = config.parse().unwrap_err();
    assert!(
        error
            .message
            .contains("Invalid Bitcoin Cash payout address"),
        "unexpected message: {}",
        error.message
    );
}

#[test]
fn bitcoin_cash_requires_its_network() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.chain = Chain::BitcoinCash;

    let error = config.parse().unwrap_err();
    assert!(
        error
            .message
            .contains("bitcoin_cash_network is required when chain is bitcoin-cash"),
        "unexpected message: {}",
        error.message
    );
}

#[test]
fn bitcoin_rejects_a_bitcoin_cash_network() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.bitcoin_cash_network = Some(BitcoinCashNetwork::Mainnet);

    let error = config.parse().unwrap_err();
    assert!(
        error
            .message
            .contains("bitcoin_cash_network is only valid when chain is bitcoin-cash"),
        "unexpected message: {}",
        error.message
    );
}

#[test]
fn bitcoin_cash_regtest_parses_on_the_regtest_network_whatever_network_says() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.chain = Chain::BitcoinCash;
    config.bitcoin_cash_network = Some(BitcoinCashNetwork::Regtest);
    config.network = Network::Signet;
    config.bootstrap_address = "bchreg:qqzldt8u5mafprrw6uljppyqg30uhl97ey890dqm34".to_string();
    assert_eq!(config.effective_network(), Network::Regtest);
    assert_eq!(config.parse().unwrap().network, Network::Regtest);
}

#[test]
fn bitcoin_cash_mainnet_uses_the_bitcoin_network() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.chain = Chain::BitcoinCash;
    config.bitcoin_cash_network = Some(BitcoinCashNetwork::Mainnet);
    config.network = Network::Signet;
    assert_eq!(config.effective_network(), Network::Bitcoin);
}

#[test]
fn bitcoin_cash_chipnet_uses_the_testnet_network() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.chain = Chain::BitcoinCash;
    config.bitcoin_cash_network = Some(BitcoinCashNetwork::Chipnet);
    config.network = Network::Signet;
    assert_eq!(config.effective_network(), Network::Testnet);
}

#[test]
fn bitcoin_keeps_its_configured_network() {
    let config = StratumConfig::<Raw>::new_for_test_default();
    assert_eq!(config.effective_network(), Network::Signet);
}
