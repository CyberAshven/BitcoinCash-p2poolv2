// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Launch a swarm of no-PoW sim nodes against one shared regtest bitcoind.
//! Replaces `load-tests/sim/run-swarm.sh`.
//!
//! Each node gets a config with distinct ports, a distinct RNG seed, a fresh
//! store and a `dial_peers` topology. The nodes keep running after this
//! returns; `stop-swarm` stops them.
//!
//! The sim binary is built with the release profile by default on purpose.
//! libp2p-request-response has a `debug_assert_eq!` in
//! `on_connection_closed` that fires under the connection churn of a
//! many-node swarm and aborts the process; it is compiled out in release.
//! This is a sim harness build for load testing, not a shipped binary, and
//! the `sim` feature must never be in a real release.

use crate::cargo_build::{BuildRequest, build, executable};
use crate::env_vars;
use crate::error::{ToolError, ToolResult};
use crate::process::{ProcessTable, Signal, detach_from_terminal_signals, find_by_command_line, send_signal};
use crate::repo;
use crate::seeded::SeededSequence;
use crate::sim::RunDir;
use crate::sim::bitcoind::{Bitcoind, CallOutcome, RpcSettings};
use serde_json::Value;
use std::fmt::Write as _;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// The config rejects a pool signature over 16 bytes.
const MAX_POOL_SIGNATURE_LENGTH: usize = 16;

/// Share chain identities, cycled across nodes. A share address owns share
/// coinbase outputs and lives on a different chain from the bitcoin payout
/// address, so it cannot be derived from `MINER_ADDRESS` and bitcoind cannot
/// mint one. Without it the sim emitter refuses to start and the swarm
/// produces no shares at all, so these are fixed regtest addresses rather
/// than optional. Derived from the well known test pubkeys; throwaway,
/// regtest only.
const SHARE_ADDRESSES: [&str; 5] = [
    "rp2pool1pmfr3p9j00pfxjh0zmgp99y8zftmd3s5pmedqhyptwy6lm87hf5ssmq4eld",
    "rp2pool1pet7ep3czdu9k4wvdlz2fp5p8x2yp7t6ttyqg2c6cmh0lgeuu9lasphrkeq",
    "rp2pool1pgxxyvcmdncdxs06cudd5yvmwwahaesaj6n3eu7st7x4sw9hrchaqgjhspc",
    "rp2pool1pjvtc2mkj9vmfneuj7w9dsqle70a040ms5tyfswhhz4vjyskznj5q9rjdk6",
    "rp2pool1paecncecu260mkwvsr63lw5v4s496v9gfn2en56hv4f0d2w2j97fsplatwc",
];

/// The swarm's settings, from the same environment variables
/// `run-swarm.sh` read.
#[derive(Debug, Clone)]
pub struct SwarmSettings {
    pub node_count: usize,
    pub run_dir: RunDir,
    pub base_p2p: u32,
    pub base_stratum: u32,
    pub base_api: u32,
    pub rpc: RpcSettings,
    pub zmq: String,
    pub miner_address: String,
    pub hashrate: f64,
    /// `equal` or `zipf`.
    pub hashrate_distribution: String,
    pub zipf_alpha: f64,
    /// `block_to_share_ratio`, written to the configs as given.
    pub shares_per_block: String,
    /// Mean outbound delay in 10 s share-interval terms, before scaling by
    /// the ideal block time.
    pub latency_ms: f64,
    /// `ideal_block_time_secs`, written to the configs as given.
    pub ideal_block_time: String,
    pub late_miner: Option<usize>,
    pub fork_group_from: Option<usize>,
    /// `spread` or `equal`.
    pub latency_distribution: String,
    pub distribution_seed: u64,
    pub asert_anchor: String,
    pub network_hashrate: Option<String>,
    pub distinct_addresses: bool,
    pub pool_signature_prefix: String,
    pub dial_fanout: usize,
    /// `release` or `debug`.
    pub profile: String,
}

impl SwarmSettings {
    /// Settings from the environment for a swarm of `node_count` nodes.
    pub fn from_env(node_count: usize) -> ToolResult<Self> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        Ok(Self {
            node_count,
            run_dir: RunDir::from_env(),
            base_p2p: env_vars::parsed("BASE_P2P", 7000)?,
            base_stratum: env_vars::parsed("BASE_STRATUM", 7300)?,
            base_api: env_vars::parsed("BASE_API", 7600)?,
            rpc: RpcSettings::from_env(),
            zmq: env_vars::string("ZMQ", "tcp://127.0.0.1:28332"),
            miner_address: env_vars::string(
                "MINER_ADDRESS",
                "bcrt1qqclp5usts33x0cgy2l5839659t7798w7g5a0fu",
            ),
            hashrate: env_vars::parsed("HASHRATE", 1.0e12)?,
            hashrate_distribution: env_vars::string("HASHRATE_DIST", "zipf"),
            zipf_alpha: env_vars::parsed("ZIPF_ALPHA", 1.0)?,
            shares_per_block: env_vars::string("SHARES_PER_BLOCK", "10000"),
            latency_ms: env_vars::parsed("LATENCY_MS", 0.0)?,
            ideal_block_time: env_vars::string("IDEAL_BLOCK_TIME", "10"),
            late_miner: env_vars::optional("LATE_MINER")?,
            fork_group_from: env_vars::optional("FORK_GROUP_FROM")?,
            latency_distribution: env_vars::string("LATENCY_DIST", "spread"),
            distribution_seed: env_vars::parsed("DIST_SEED", 42)?,
            asert_anchor: env_vars::string("ASERT_ANCHOR", &now.to_string()),
            network_hashrate: env_vars::optional_string("NETWORK_HASHRATE"),
            distinct_addresses: env_vars::string("DISTINCT_ADDR", "1") == "1",
            pool_signature_prefix: env_vars::string("POOL_SIGNATURE_PREFIX", "P2Pool-sim-"),
            dial_fanout: env_vars::parsed("DIAL_FANOUT", 3)?,
            profile: env_vars::string("PROFILE", "release"),
        })
    }

    /// The mean latency in milliseconds after time compression: a shorter
    /// share interval scales the latency by the same factor, so the uncle
    /// rate (latency over interval) is preserved.
    pub fn scaled_latency_ms(&self) -> f64 {
        let block_time: f64 = self.ideal_block_time.trim().parse().unwrap_or(0.0);
        format!("{:.0}", self.latency_ms * block_time / 10.0)
            .parse()
            .unwrap_or(0.0)
    }

    /// Total network hashrate written to every config: `NETWORK_HASHRATE`
    /// when set, else the node count times the mean hashrate (zipf keeps the
    /// total). It anchors the genesis difficulty at the steady state so the
    /// chain starts regulated instead of climbing for 15 to 20 minutes.
    pub fn network_hashrate(&self) -> String {
        self.network_hashrate
            .clone()
            .unwrap_or_else(|| format!("{:.0}", self.node_count as f64 * self.hashrate))
    }
}

/// One node's hashrate (as written to its config) and latency.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeShape {
    pub hashrate: String,
    pub latency_ms: u64,
}

/// Per-node hashrate and latency from the chosen distributions.
///
/// * zipf hashrate: `w_i = (i+1)^-alpha`, `hashrate_i = N * HASHRATE * w_i /
///   sum(w)`, so the total network hashrate matches the equal case while
///   the split is skewed: a few big miners and a long tail.
/// * spread latency: log-uniform in `[0.3, 2.5]` times the mean, drawn from
///   the seed so a run is reproducible.
pub fn node_shapes(settings: &SwarmSettings) -> Vec<NodeShape> {
    let count = settings.node_count;
    let weights: Vec<f64> = (0..count)
        .map(|index| (-settings.zipf_alpha * ((index + 1) as f64).ln()).exp())
        .collect();
    let weight_total: f64 = weights.iter().sum();
    let latency = settings.scaled_latency_ms();
    let mut sequence = SeededSequence::new(settings.distribution_seed);
    weights
        .iter()
        .map(|weight| {
            let hashrate = if settings.hashrate_distribution == "equal" {
                settings.hashrate
            } else {
                count as f64 * settings.hashrate * weight / weight_total
            };
            let latency_ms = if latency > 0.0 && settings.latency_distribution == "spread" {
                let factor = 0.3 * (sequence.next_unit() * (2.5f64 / 0.3).ln()).exp();
                (latency * factor + 0.5).trunc() as u64
            } else {
                latency.trunc() as u64
            };
            NodeShape {
                hashrate: format!("{hashrate:.0}"),
                latency_ms,
            }
        })
        .collect()
}

/// Listen addresses node `index` dials: up to `dial_fanout` earlier nodes,
/// which chains every node into one connected swarm (node 0 dials nobody).
/// With `fork_group_from`, a node in the minority group dials only earlier
/// nodes of its own group, so the two groups never connect.
pub fn dial_peers(settings: &SwarmSettings, index: usize) -> String {
    let group_start = match settings.fork_group_from {
        Some(start) if index >= start => start,
        _ => 0,
    };
    (1..=settings.dial_fanout)
        .filter_map(|step| index.checked_sub(step))
        .filter(|peer| *peer >= group_start)
        .map(|peer| format!("\"/ip4/127.0.0.1/tcp/{}\"", settings.base_p2p + peer as u32))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Everything that differs between node configs.
#[derive(Debug, Clone)]
pub struct NodeIdentity<'a> {
    pub index: usize,
    pub payout_address: &'a str,
    pub shape: &'a NodeShape,
}

/// The config of one node.
pub fn node_config(settings: &SwarmSettings, node: &NodeIdentity<'_>) -> String {
    let index = node.index;
    let p2p_port = settings.base_p2p + index as u32;
    let stratum_port = settings.base_stratum + index as u32;
    let api_port = settings.base_api + index as u32;
    let enabled = settings.late_miner != Some(index);
    let share_address = SHARE_ADDRESSES[index % SHARE_ADDRESSES.len()];
    let run_dir = settings.run_dir.path().display().to_string();
    let store = settings.run_dir.node_store(index).display().to_string();
    let mut config = String::with_capacity(2048);
    let _ = write!(
        config,
        r#"[network]
listen_address = "/ip4/127.0.0.1/tcp/{p2p_port}"
dial_peers = [{dial}]
max_pending_incoming = 10
max_pending_outgoing = 10
max_established_incoming = 50
max_established_outgoing = 50
max_established_per_peer = 1
max_workbase_per_second = 10
max_userworkbase_per_second = 10
max_miningshare_per_second = 100
max_inventory_per_second = 100
max_transaction_per_second = 100
max_requests_per_second = 100
dial_timeout_secs = 30

[store]
path = "{store}"
pplns_ttl_days = 7

[stratum]
wait_for_chain_sync = false
hostname = "127.0.0.1"
port = {stratum_port}
start_difficulty = 1
minimum_difficulty = 1
solo_address = "{miner}"
bootstrap_address = "{miner}"
zmqpubhashblock = "{zmq}"
network = "regtest"
version_mask = "1fffffe0"
pool_signature = "{prefix}{index}"

[bitcoinrpc]
url = "{rpc_url}"
username = "{rpc_user}"
password = "{rpc_password}"

[logging]
console = true
level = "debug"
stats_dir = "{run_dir}/stats-{index}"

[api]
hostname = "127.0.0.1"
port = {api_port}

[sim]
enabled = {enabled}
miner_address = "{payout}"
share_address = "{share_address}"
hashrate = {hashrate}
block_to_share_ratio = {shares_per_block}
seed = {seed}
propagation_delay_ms = {latency}
asert_anchor_time = {anchor}
network_hashrate = {network_hashrate}
ideal_block_time_secs = {block_time}
"#,
        dial = dial_peers(settings, index),
        miner = settings.miner_address,
        zmq = settings.zmq,
        prefix = settings.pool_signature_prefix,
        rpc_url = settings.rpc.url,
        rpc_user = settings.rpc.user,
        rpc_password = settings.rpc.password,
        payout = node.payout_address,
        hashrate = node.shape.hashrate,
        shares_per_block = settings.shares_per_block,
        seed = index + 1,
        latency = node.shape.latency_ms,
        anchor = settings.asert_anchor,
        network_hashrate = settings.network_hashrate(),
        block_time = settings.ideal_block_time,
    );
    config
}

/// The longest pool signature, the highest node's, must fit the config's
/// 16-byte limit.
fn check_pool_signature(settings: &SwarmSettings) -> ToolResult<()> {
    let longest = format!(
        "{}{}",
        settings.pool_signature_prefix,
        settings.node_count.saturating_sub(1)
    );
    if longest.len() > MAX_POOL_SIGNATURE_LENGTH {
        return Err(ToolError::message(format!(
            "pool signature '{longest}' exceeds {MAX_POOL_SIGNATURE_LENGTH} bytes; shorten POOL_SIGNATURE_PREFIX"
        )));
    }
    Ok(())
}

/// Build `p2poolv2_sim` with the `sim` feature in `profile` and return its
/// path.
pub fn build_sim_binary(profile: &str) -> ToolResult<PathBuf> {
    println!("Building p2poolv2_sim ({profile}) with --features sim ...");
    let built = build(
        &repo::root(),
        &BuildRequest {
            packages: vec!["p2poolv2_sim".to_string()],
            binaries: vec!["p2poolv2_sim".to_string()],
            features: vec!["sim".to_string()],
            release: profile == "release",
            environment: Vec::new(),
        },
    )?;
    executable(&built, "p2poolv2_sim")
}

/// A fresh payout address from the regtest wallet, or `None` without one.
async fn new_address(bitcoind: &Bitcoind) -> Option<String> {
    match bitcoind
        .call("getnewaddress", vec![Value::from("sim"), Value::from("bech32")])
        .await
    {
        Ok(CallOutcome::Result(Value::String(address))) if !address.is_empty() => Some(address),
        _ => None,
    }
}

/// Start the node with config `config`, its output going to `log`, which is
/// truncated (`append` false) or appended to. Returns its PID.
pub fn start_node(
    processes: &mut ProcessTable,
    binary: &Path,
    config: &Path,
    log: &Path,
    append: bool,
) -> ToolResult<u32> {
    let output = File::options()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .open(log)
        .map_err(|error| ToolError::io("open", log, error))?;
    let errors = output
        .try_clone()
        .map_err(|error| ToolError::io("open", log, error))?;
    let mut command = Command::new(binary);
    command
        .arg("--config")
        .arg(config)
        .stdin(Stdio::null())
        .stdout(output)
        .stderr(errors);
    detach_from_terminal_signals(&mut command);
    processes.spawn(&mut command)
}

/// Kill whatever is left of an earlier swarm in this run directory before it
/// is wiped; leftovers keep their ports and new nodes panic on bind.
async fn kill_leftovers(run_dir: &RunDir) {
    let leftovers = find_by_command_line(&run_dir.process_pattern());
    if !leftovers.is_empty() {
        println!(
            "Killing leftover swarm processes in {} ...",
            run_dir.path().display()
        );
        send_signal(&leftovers, Signal::Kill);
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

/// Build the sim binary, check bitcoind, wipe the run directory and launch
/// the swarm. The node processes are recorded in `processes` and, one PID
/// per line, in `pids.txt`. Returns the sim binary's path, which restarts
/// of the same swarm use.
///
/// This wipes the whole run directory, including any file a caller is
/// already writing there. Do not point a `tee` or any other open handle
/// inside `RUN_DIR` first: the file is unlinked and its contents lost.
pub async fn run_swarm(
    settings: &SwarmSettings,
    processes: &mut ProcessTable,
) -> ToolResult<PathBuf> {
    check_pool_signature(settings)?;
    let binary = build_sim_binary(&settings.profile)?;

    let bitcoind = Bitcoind::new(&settings.rpc)?;
    if !bitcoind.is_regtest().await {
        return Err(ToolError::message(format!(
            "ERROR: regtest bitcoind not reachable at {} (need rpc + zmq up).",
            settings.rpc.url
        )));
    }

    let run_dir = &settings.run_dir;
    kill_leftovers(run_dir).await;
    if run_dir.path().exists() {
        std::fs::remove_dir_all(run_dir.path())
            .map_err(|error| ToolError::io("remove", run_dir.path(), error))?;
    }
    std::fs::create_dir_all(run_dir.path())
        .map_err(|error| ToolError::io("create", run_dir.path(), error))?;
    let pids_file = run_dir.pids_file();
    std::fs::write(&pids_file, "").map_err(|error| ToolError::io("write", &pids_file, error))?;

    let shapes = node_shapes(settings);
    println!(
        "Launching {} sim nodes into {} ...",
        settings.node_count,
        run_dir.path().display()
    );
    let mut pid_lines = String::with_capacity(settings.node_count * 8);
    for (index, shape) in shapes.iter().enumerate() {
        let stats_dir = run_dir.path().join(format!("stats-{index}"));
        std::fs::create_dir_all(&stats_dir)
            .map_err(|error| ToolError::io("create", &stats_dir, error))?;

        // Distinct payout addresses make the PPLNS coinbase a real multi-way
        // split; without a wallet every node falls back to MINER_ADDRESS.
        let payout_address = if settings.distinct_addresses {
            new_address(&bitcoind).await
        } else {
            None
        }
        .unwrap_or_else(|| settings.miner_address.clone());

        let config_path = run_dir.node_config(index);
        let config = node_config(
            settings,
            &NodeIdentity {
                index,
                payout_address: &payout_address,
                shape,
            },
        );
        std::fs::write(&config_path, config)
            .map_err(|error| ToolError::io("write", &config_path, error))?;

        let pid = start_node(processes, &binary, &config_path, &run_dir.node_log(index), false)?;
        let _ = writeln!(pid_lines, "{pid}");
        std::fs::write(&pids_file, &pid_lines)
            .map_err(|error| ToolError::io("write", &pids_file, error))?;
        println!(
            "  node {index:>2}  pid {pid:<7}  p2p {}  api {}  hashrate {:<10} lat {}ms",
            settings.base_p2p + index as u32,
            settings.base_api + index as u32,
            shape.hashrate,
            shape.latency_ms
        );
        // A small stagger so early dials land on peers already listening.
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    println!();
    println!(
        "{} nodes launched. PIDs in {}",
        settings.node_count,
        pids_file.display()
    );
    println!("Observe:  cargo sim observe");
    println!("Stop:     cargo sim stop-swarm");
    Ok(binary)
}

#[cfg(test)]
mod tests {
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
}
