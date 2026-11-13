// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Launch a swarm of no-PoW sim nodes against one shared BCHN regtest node.
//! Replaces `load-tests/sim/run-swarm.sh`.
//!
//! The nodes mine Bitcoin Cash: each config sets `chain = "bitcoin-cash"`
//! and `bitcoin_cash_network = "regtest"`, a `bchreg:` CashAddr payout
//! address and an `rp2pool:` share address. Each node gets a config with
//! distinct ports, a distinct RNG seed, its own share address, a fresh store
//! and a `dial_peers` topology. The nodes keep running after this returns;
//! `stop-swarm` stops them.
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
use crate::process::{
    ProcessTable, Signal, detach_from_terminal_signals, find_by_command_line, send_signal,
};
use crate::repo;
use crate::seeded::SeededSequence;
use crate::sim::RunDir;
use crate::sim::addresses::{DEFAULT_MINER_ADDRESS, check_payout_address, share_address};
use crate::sim::bitcoind::{Bitcoind, CallOutcome, RpcSettings, WALLET};
use p2poolv2_config::{MAX_PAYOUT_LEVEL, MIN_PAYOUT_LEVEL, payout_level_below_minimum};
use serde_json::Value;
use std::fmt::Write as _;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// The config rejects a pool signature over 16 bytes.
const MAX_POOL_SIGNATURE_LENGTH: usize = 16;

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
    /// `PAYOUT_LEVELS`: the `[stratum] payout_level` of each node, in node
    /// order, comma separated, such as `5,7,7`. A node past the list keeps
    /// the node's default level.
    pub payout_levels: Vec<u8>,
    /// `WEAK_SHARES=1`: every node's miners send weak shares (`[sim]
    /// weak_shares`).
    pub weak_shares: bool,
    /// `VIRTUAL_MINERS`: miners each node models (`[sim] virtual_miners`), so
    /// a small swarm has many payees.
    pub virtual_miners: Option<u32>,
    /// `MERGE_TOKENS`: a token file every node merge-mines (`[stratum]
    /// merge_mining_tokens`), which makes every share a token share.
    pub merge_tokens: Option<String>,
    /// `LOG_LEVEL`: the nodes' log level, `debug` unless set. A long run
    /// with weak shares writes gigabytes at `debug`.
    pub log_level: String,
}

/// The levels of a `PAYOUT_LEVELS` value: comma-separated levels from
/// [`MIN_PAYOUT_LEVEL`] to [`MAX_PAYOUT_LEVEL`], empty for none.
pub fn parse_payout_levels(value: &str) -> ToolResult<Vec<u8>> {
    if value.trim().is_empty() {
        return Ok(Vec::new());
    }
    value
        .split(',')
        .map(|token| parse_one_payout_level(value, token.trim()))
        .collect()
}

/// One `PAYOUT_LEVELS` token, or why it is not a Bitcoin Cash payout level.
fn parse_one_payout_level(value: &str, token: &str) -> ToolResult<u8> {
    let level: u8 = token.parse().map_err(|_| {
        ToolError::message(format!(
            "PAYOUT_LEVELS={value:?}: {token:?} is not a payout level from \
             {MIN_PAYOUT_LEVEL} to {MAX_PAYOUT_LEVEL}"
        ))
    })?;
    if level < MIN_PAYOUT_LEVEL {
        return Err(ToolError::message(format!(
            "PAYOUT_LEVELS={value:?}: {}",
            payout_level_below_minimum(level)
        )));
    }
    if level > MAX_PAYOUT_LEVEL {
        return Err(ToolError::message(format!(
            "PAYOUT_LEVELS={value:?}: {token:?} is not a payout level from \
             {MIN_PAYOUT_LEVEL} to {MAX_PAYOUT_LEVEL}"
        )));
    }
    Ok(level)
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
            miner_address: env_vars::string("MINER_ADDRESS", DEFAULT_MINER_ADDRESS),
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
            payout_levels: parse_payout_levels(&env_vars::string("PAYOUT_LEVELS", ""))?,
            weak_shares: env_vars::string("WEAK_SHARES", "0") == "1",
            virtual_miners: env_vars::optional("VIRTUAL_MINERS")?,
            merge_tokens: env_vars::optional_string("MERGE_TOKENS"),
            log_level: env_vars::string("LOG_LEVEL", "debug"),
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

/// `value` escaped for a TOML basic string: `\` and `"` become `\\` and
/// `\"`. A Windows path otherwise fails to parse (`\s` in `\sim` is not a
/// TOML escape) and the node exits without loading its config.
fn toml_basic_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if character == '\\' || character == '"' {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

/// The config of one node.
pub fn node_config(settings: &SwarmSettings, node: &NodeIdentity<'_>) -> String {
    let index = node.index;
    let p2p_port = settings.base_p2p + index as u32;
    let stratum_port = settings.base_stratum + index as u32;
    let api_port = settings.base_api + index as u32;
    let enabled = settings.late_miner != Some(index);
    let share_address = toml_basic_string(&share_address(index));
    let run_dir = toml_basic_string(&settings.run_dir.path().display().to_string());
    let store = toml_basic_string(&settings.run_dir.node_store(index).display().to_string());
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
chain = "bitcoin-cash"
bitcoin_cash_network = "regtest"
# Required, but Bitcoin Cash derives it from bitcoin_cash_network.
network = "regtest"
version_mask = "1fffe000"
pool_signature = "{prefix}{index}"
{payout_level}{merge_tokens}
[bitcoinrpc]
url = "{rpc_url}"
username = "{rpc_user}"
password = "{rpc_password}"

[logging]
console = true
level = "{log_level}"
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
{sim_options}"#,
        // `dial` is already a TOML fragment (`"/ip4/..."`), not one string.
        dial = dial_peers(settings, index),
        miner = toml_basic_string(&settings.miner_address),
        zmq = toml_basic_string(&settings.zmq),
        prefix = toml_basic_string(&settings.pool_signature_prefix),
        rpc_url = toml_basic_string(&settings.rpc.url),
        rpc_user = toml_basic_string(&settings.rpc.user),
        rpc_password = toml_basic_string(&settings.rpc.password),
        payout = toml_basic_string(node.payout_address),
        hashrate = node.shape.hashrate,
        shares_per_block = settings.shares_per_block,
        seed = index + 1,
        latency = node.shape.latency_ms,
        anchor = settings.asert_anchor,
        network_hashrate = settings.network_hashrate(),
        block_time = settings.ideal_block_time,
        payout_level = settings
            .payout_levels
            .get(index)
            .map(|level| format!("payout_level = {level}\n"))
            .unwrap_or_default(),
        merge_tokens = settings
            .merge_tokens
            .as_ref()
            .map(|path| format!("merge_mining_tokens = \"{}\"\n", toml_basic_string(path)))
            .unwrap_or_default(),
        log_level = toml_basic_string(&settings.log_level),
        sim_options = sim_options(settings),
    );
    config
}

/// The `[sim]` lines of the swarm's optional settings: weak shares and the
/// miners each node models.
fn sim_options(settings: &SwarmSettings) -> String {
    let weak_shares = if settings.weak_shares {
        "weak_shares = true\n"
    } else {
        ""
    };
    let virtual_miners = settings
        .virtual_miners
        .map(|count| format!("virtual_miners = {count}\n"))
        .unwrap_or_default();
    format!("{weak_shares}{virtual_miners}")
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

/// A fresh payout address from the `p2pool` wallet, or `None` without one.
///
/// Only the label is passed, so the node picks its own default address type:
/// BCHN answers with a `bchreg:` CashAddr. The script asked for `bech32`, a
/// Bitcoin-only type that BCHN rejects.
async fn new_address(wallet: &Bitcoind) -> Option<String> {
    match wallet.call("getnewaddress", vec![Value::from("sim")]).await {
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
    check_payout_address(&settings.miner_address).map_err(ToolError::message)?;
    let binary = build_sim_binary(&settings.profile)?;

    let bitcoind = Bitcoind::new(&settings.rpc)?;
    if !bitcoind.is_regtest().await {
        return Err(ToolError::message(format!(
            "ERROR: regtest BCHN not reachable at {} (need rpc + zmq up).",
            settings.rpc.url
        )));
    }
    bitcoind.require_bchn().await?;
    let wallet = Bitcoind::new(&settings.rpc.for_wallet(WALLET))?;

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
            new_address(&wallet).await
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

        let pid = start_node(
            processes,
            &binary,
            &config_path,
            &run_dir.node_log(index),
            false,
        )?;
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
mod tests;
