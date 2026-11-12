// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;

fn settings(node_count: usize) -> SwarmSettings {
    SwarmSettings {
        node_count,
        run_dir: RunDir::new("/tmp/p2pool-sim"),
        base_p2p: 7000,
        base_stratum: 7300,
        base_api: 7600,
        rpc: RpcSettings {
            url: "http://127.0.0.1:19443".to_string(),
            user: "p2pool".to_string(),
            password: "p2pool".to_string(),
        },
        zmq: "tcp://127.0.0.1:28332".to_string(),
        miner_address: DEFAULT_MINER_ADDRESS.to_string(),
        hashrate: 1.0e12,
        hashrate_distribution: "zipf".to_string(),
        zipf_alpha: 1.0,
        shares_per_block: "10000".to_string(),
        latency_ms: 0.0,
        ideal_block_time: "10".to_string(),
        late_miner: None,
        fork_group_from: None,
        latency_distribution: "spread".to_string(),
        distribution_seed: 42,
        asert_anchor: "1700000000".to_string(),
        network_hashrate: None,
        distinct_addresses: true,
        pool_signature_prefix: "P2Pool-sim-".to_string(),
        dial_fanout: 3,
        profile: "release".to_string(),
        payout_levels: Vec::new(),
        weak_shares: false,
        virtual_miners: None,
        merge_tokens: None,
        log_level: "debug".to_string(),
    }
}

/// `PAYOUT_LEVELS=5,7` gives node 1 `payout_level = 7` under `[stratum]`.
#[test]
fn node_config_writes_the_nodes_payout_level() {
    let mut swarm = settings(2);
    swarm.payout_levels = vec![5, 7];
    let shape = NodeShape {
        hashrate: "5".to_string(),
        latency_ms: 0,
    };
    let config = node_config(
        &swarm,
        &NodeIdentity {
            index: 1,
            payout_address: "bchreg:qpayout",
            shape: &shape,
        },
    );
    let parsed: toml::Table = config.parse().expect("config parses");

    assert_eq!(
        parsed
            .get("stratum")
            .and_then(|table| table.get("payout_level"))
            .and_then(toml::Value::as_integer),
        Some(7)
    );
}

/// A node past `PAYOUT_LEVELS` gets no `payout_level`: the node's default.
#[test]
fn node_config_leaves_out_a_payout_level_past_the_list() {
    let mut swarm = settings(2);
    swarm.payout_levels = vec![5];
    let shape = NodeShape {
        hashrate: "5".to_string(),
        latency_ms: 0,
    };
    let config = node_config(
        &swarm,
        &NodeIdentity {
            index: 1,
            payout_address: "bchreg:qpayout",
            shape: &shape,
        },
    );
    let parsed: toml::Table = config.parse().expect("config parses");

    assert!(
        parsed
            .get("stratum")
            .and_then(|table| table.get("payout_level"))
            .is_none()
    );
}

#[test]
fn payout_levels_parse_as_a_comma_separated_list() {
    assert_eq!(parse_payout_levels("5, 7,0").unwrap(), vec![5, 7, 0]);
}

#[test]
fn an_empty_payout_levels_value_is_no_levels() {
    assert!(parse_payout_levels("").unwrap().is_empty());
}

#[test]
fn a_payout_level_above_seven_is_refused() {
    assert!(parse_payout_levels("5,8").is_err());
}

#[test]
fn zipf_hashrates_keep_the_total() {
    let shapes = node_shapes(&settings(3));
    assert_eq!(shapes[0].hashrate, "1636363636364");
    assert_eq!(shapes[1].hashrate, "818181818182");
    assert_eq!(shapes[2].hashrate, "545454545455");
}

#[test]
fn equal_hashrates_are_the_mean() {
    let mut equal = settings(2);
    equal.hashrate_distribution = "equal".to_string();
    assert_eq!(node_shapes(&equal)[1].hashrate, "1000000000000");
}

#[test]
fn zero_latency_stays_zero() {
    assert_eq!(node_shapes(&settings(2))[1].latency_ms, 0);
}

#[test]
fn spread_latency_stays_within_its_bounds() {
    let mut spread = settings(1);
    spread.latency_ms = 100.0;
    let latency = node_shapes(&spread)[0].latency_ms;
    assert!((30..=250).contains(&latency), "latency {latency}");
}

#[test]
fn latency_scales_with_the_block_time() {
    let mut compressed = settings(1);
    compressed.latency_ms = 100.0;
    compressed.ideal_block_time = "1".to_string();
    assert_eq!(compressed.scaled_latency_ms(), 10.0);
}

#[test]
fn network_hashrate_defaults_to_the_total() {
    assert_eq!(settings(10).network_hashrate(), "10000000000000");
}

#[test]
fn node_zero_dials_nobody() {
    assert_eq!(dial_peers(&settings(5), 0), "");
}

#[test]
fn nodes_dial_up_to_the_fanout_of_earlier_nodes() {
    assert_eq!(
        dial_peers(&settings(5), 4),
        "\"/ip4/127.0.0.1/tcp/7003\", \"/ip4/127.0.0.1/tcp/7002\", \"/ip4/127.0.0.1/tcp/7001\""
    );
}

#[test]
fn minority_group_dials_only_its_own_group() {
    let mut forked = settings(6);
    forked.fork_group_from = Some(4);
    assert_eq!(dial_peers(&forked, 5), "\"/ip4/127.0.0.1/tcp/7004\"");
    assert_eq!(dial_peers(&forked, 4), "");
}

#[test]
fn late_miner_starts_with_its_emitter_off() {
    let mut late = settings(3);
    late.late_miner = Some(1);
    let shape = NodeShape {
        hashrate: "5".to_string(),
        latency_ms: 0,
    };
    let config = node_config(
        &late,
        &NodeIdentity {
            index: 1,
            payout_address: "bchreg:qpayout",
            shape: &shape,
        },
    );
    assert!(config.contains("\n[sim]\nenabled = false\nminer_address = \"bchreg:qpayout\"\n"));
    assert!(config.contains("pool_signature = \"P2Pool-sim-1\"\n"));
    assert!(config.contains("dial_peers = [\"/ip4/127.0.0.1/tcp/7000\"]\n"));
    assert!(config.contains("seed = 2\n"));
}

#[test]
fn long_pool_signature_is_refused() {
    // "P2Pool-sim-xyz-99" is 17 bytes.
    let mut long = settings(100);
    long.pool_signature_prefix = "P2Pool-sim-xyz-".to_string();
    assert!(check_pool_signature(&long).is_err());
}

#[test]
fn pool_signature_within_the_limit_is_accepted() {
    // "P2Pool-sim-x-99" is 15 bytes.
    let mut fitting = settings(100);
    fitting.pool_signature_prefix = "P2Pool-sim-x-".to_string();
    assert!(check_pool_signature(&fitting).is_ok());
}

#[test]
fn toml_basic_string_round_trips_a_windows_path() {
    let path = r"C:\p2pool\sim\store-0.db";
    let document = format!("path = \"{}\"", toml_basic_string(path));
    let table: toml::Table = document.parse().expect("parse");
    assert_eq!(table["path"].as_str(), Some(path));
    assert_eq!(toml_basic_string("say \"hi\" \\ "), "say \\\"hi\\\" \\\\ ");
}

#[test]
fn node_config_escapes_paths_for_toml() {
    let mut swarm = settings(1);
    swarm.run_dir = RunDir::new(r"C:\p2pool\sim");
    swarm.rpc.password = "quo\"te\\slash".to_string();
    let shape = NodeShape {
        hashrate: "5".to_string(),
        latency_ms: 0,
    };
    let config = node_config(
        &swarm,
        &NodeIdentity {
            index: 0,
            payout_address: "bchreg:qpayout",
            shape: &shape,
        },
    );
    let parsed: toml::Table = config.parse().expect("config parses");
    let store = swarm.run_dir.node_store(0).display().to_string();
    assert_eq!(
        parsed
            .get("store")
            .and_then(|table| table.get("path"))
            .and_then(toml::Value::as_str),
        Some(store.as_str())
    );
    let stats_dir = format!("{}/stats-0", swarm.run_dir.path().display());
    assert_eq!(
        parsed
            .get("logging")
            .and_then(|table| table.get("stats_dir"))
            .and_then(toml::Value::as_str),
        Some(stats_dir.as_str())
    );
    assert_eq!(
        parsed
            .get("bitcoinrpc")
            .and_then(|table| table.get("password"))
            .and_then(toml::Value::as_str),
        Some("quo\"te\\slash")
    );
}

#[test]
fn nodes_mine_bitcoin_cash_regtest() {
    let shape = NodeShape {
        hashrate: "5".to_string(),
        latency_ms: 0,
    };
    let config = node_config(
        &settings(3),
        &NodeIdentity {
            index: 2,
            payout_address: DEFAULT_MINER_ADDRESS,
            shape: &shape,
        },
    );

    assert!(config.contains("\nchain = \"bitcoin-cash\"\nbitcoin_cash_network = \"regtest\"\n"));
    assert!(config.contains(&format!(
        "\nbootstrap_address = \"{DEFAULT_MINER_ADDRESS}\"\n"
    )));
    assert!(config.contains(&format!("\nshare_address = \"{}\"\n", share_address(2))));
}

/// `WEAK_SHARES`, `VIRTUAL_MINERS` and `MERGE_TOKENS` reach the node's
/// config as `[sim]` and `[stratum]` keys, and a swarm without them writes
/// none.
#[test]
fn the_optional_settings_reach_the_node_config() {
    let shape = NodeShape {
        hashrate: "5".to_string(),
        latency_ms: 0,
    };
    let identity = NodeIdentity {
        index: 0,
        payout_address: DEFAULT_MINER_ADDRESS,
        shape: &shape,
    };
    let plain = node_config(&settings(2), &identity);
    let with_options = node_config(
        &SwarmSettings {
            weak_shares: true,
            virtual_miners: Some(100),
            merge_tokens: Some("D:\\run\\tokens.json".to_string()),
            ..settings(2)
        },
        &identity,
    );

    assert!(!plain.contains("weak_shares"));
    assert!(!plain.contains("virtual_miners"));
    assert!(!plain.contains("merge_mining_tokens"));
    let parsed: toml::Value = toml::from_str(&with_options).unwrap();
    let sim = parsed.get("sim").unwrap();
    assert_eq!(
        sim.get("weak_shares").and_then(toml::Value::as_bool),
        Some(true)
    );
    assert_eq!(
        sim.get("virtual_miners").and_then(toml::Value::as_integer),
        Some(100)
    );
    assert_eq!(
        parsed
            .get("stratum")
            .and_then(|table| table.get("merge_mining_tokens"))
            .and_then(toml::Value::as_str),
        Some("D:\\run\\tokens.json")
    );
}
