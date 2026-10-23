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
        miner_address: "bcrt1qminer".to_string(),
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
    }
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
            payout_address: "bcrt1qpayout",
            shape: &shape,
        },
    );
    assert!(config.contains("\n[sim]\nenabled = false\nminer_address = \"bcrt1qpayout\"\n"));
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
