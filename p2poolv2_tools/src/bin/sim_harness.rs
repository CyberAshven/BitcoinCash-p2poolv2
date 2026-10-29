// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The sim swarm harness. Run it as `cargo sim <command>` from the
//! repository (an alias in `.cargo/config.toml`).

use clap::{Parser, Subcommand};
use p2poolv2_tools::env_vars;
use p2poolv2_tools::process::ProcessTable;
use p2poolv2_tools::sim::bitcoind::{Bitcoind, RpcSettings};
use p2poolv2_tools::sim::failures::{FailureSettings, inject};
use p2poolv2_tools::sim::maturity::{MaturityCheck, Verdict};
use p2poolv2_tools::sim::nightly::{self, NightlySettings};
use p2poolv2_tools::sim::stop::{DEFAULT_GRACE_SECONDS, stop_swarm};
use p2poolv2_tools::sim::swarm::{SwarmSettings, run_swarm};
use p2poolv2_tools::sim::{RunDir, metrics, observe, plot};
use std::path::PathBuf;
use std::process::ExitCode;

const SWARM_HELP: &str = "\
The nodes mine Bitcoin Cash (chain = \"bitcoin-cash\", bitcoin_cash_network =
\"regtest\") against a BCHN regtest node, with a bchreg: payout address and
an rp2pool: share address of their own.

Env:
  RUN_DIR        work dir for configs/stores/logs   (default /tmp/p2pool-sim)
  BASE_P2P       first libp2p port                  (default 7000)
  BASE_STRATUM   first stratum port                 (default 7300)
  BASE_API       first API port                     (default 7600)
  RPC_URL        BCHN rpc url                       (default http://127.0.0.1:19443)
  RPC_USER       BCHN rpc user                      (default p2pool)
  RPC_PASS       BCHN rpc pass                      (default p2pool)
  ZMQ            zmqpubhashblock                    (default tcp://127.0.0.1:28332)
  MINER_ADDRESS  bchreg: payout address for nodes   (default bchreg:qp63...vuwd,
                 without their own                  the key with secret 1)
  HASHRATE       mean per-node hashrate h/s         (default 1.0e12)
  HASHRATE_DIST  equal | zipf                       (default zipf: power-law,
                 total N*HASHRATE preserved; a few big miners and a long tail)
  ZIPF_ALPHA     power-law exponent for zipf        (default 1.0)
  SHARES_PER_BLOCK  block_to_share_ratio            (default 10000)
  LATENCY_MS     mean per-node outbound delay (ms)  (default 0; raise to see uncles)
  LATENCY_DIST   equal | spread                     (default spread: per-node
                 log-uniform around LATENCY_MS; models heterogeneous links)
  DIST_SEED      seed for the latency spread        (default 42; reproducible)
  IDEAL_BLOCK_TIME  share interval in seconds       (default 10; lower = time-compressed)
  ASERT_ANCHOR   shared ASERT anchor time           (default now)
  NETWORK_HASHRATE  total network hashrate          (default N*HASHRATE)
  DISTINCT_ADDR  1 = each node gets its own payout  (default 1; needs a loaded
                 address via RPC getnewaddress            p2pool wallet on BCHN)
  POOL_SIGNATURE_PREFIX  node i signs as PREFIX<i>  (default P2Pool-sim-;
                 each must stay within the 16-byte config limit)
  DIAL_FANOUT    how many earlier peers each dials  (default 3)
  LATE_MINER     node index launched with its emitter off (default unset); it
                 syncs but mines nothing until `failures late-miner` restarts it
  FORK_GROUP_FROM  split the swarm into two groups that never connect (default
                 unset); nodes at this index and above dial only each other
  PROFILE        cargo profile: release | debug     (default release)";

const FAILURES_HELP: &str = "\
Scenarios:
  clean             no-op (the nightly's default)
  restart-fast      one node fails and comes back immediately
  restart-delayed   one node fails and comes back after a long delay; the
                    default downtime crosses the 300s tip-age threshold, so the
                    node rejoins \"not current\" and bulk-syncs via getheaders
  partition         DIAL_FANOUT consecutive nodes fail at once, cutting the
                    dial chain, then all come back
  all               the three above in sequence (local use, not CI)
  late-miner        payout maturity: node LATE_MINER starts mining once the
                    chain is past the maturity depth; the maturity check then
                    verifies it is paid only once mature. Not part of `all`
  pruned-join       once every node has pruned block bodies, one node is wiped
                    and rejoins as a fresh node. Not part of `all`
  deep-fork-refused reorg depth limit: the swarm is launched split
                    (FORK_GROUP_FROM); once every node has pruned, one
                    minority node reconnects and must refuse the deep reorg
  shallow-fork-reorged  control for deep-fork-refused: the minority node
                    reconnects before the prune target and must reorg

Env:
  RUN_DIR                   swarm work dir                (default /tmp/p2pool-sim)
  BASE_P2P / BASE_API       first libp2p / API port       (default 7000 / 7600)
  PROFILE                   cargo profile: release|debug  (default release)
  SIM_BIN                   sim binary path               (default from PROFILE)
  DIAL_FANOUT               peers each node dials         (default 3)
  FAILURE_SEED              target selection seed         (default 42)
  FAILURE_WARMUP            settle before injecting (s)   (default 60)
  FAILURE_FAST_DOWN         restart-fast downtime (s)     (default 10)
  FAILURE_SLOW_DOWN         restart-delayed downtime (s)  (default 330)
  FAILURE_PARTITION_DOWN    partition downtime (s)        (default 90)
  FAILURE_CATCHUP_TIMEOUT   max wait for catch-up (s)     (default 240)
  LATE_MINER                late-miner target node        (required for late-miner)
  LATE_MINER_TIP_TIMEOUT    max wait for tip 2*maturity (s) (default 900)
  LATE_MINER_CHECK_TIMEOUT  max wait for a verdict (s)    (default 900)
  PRUNED_JOIN_TIP_TIMEOUT   max wait for every node to prune (s) (default 900)
  FORK_GROUP_FROM           first node of the minority group (fork scenarios)
  FORK_TIP_TIMEOUT          max wait to reach the reconnect point (s) (default 900)
  FORK_OBSERVE_SECS         watch after the reconnect (s) (default 90)
  RPC_URL / RPC_USER / RPC_PASS  BCHN RPC, for the maturity check

Node 0 is never a target: metrics derives the share rate and uncle rate from
node-0.log alone, so killing it would corrupt the run's own metrics.

Examples:
  cargo sim failures restart-fast
  FAILURE_WARMUP=20 FAILURE_SLOW_DOWN=60 cargo sim failures restart-delayed
  LATE_MINER=1 cargo sim failures late-miner
  FORK_GROUP_FROM=4 cargo sim failures deep-fork-refused";

const NIGHTLY_HELP: &str = "\
Scenarios: see `cargo sim failures --help`.

Examples:
  cargo sim nightly                                      # 20 nodes, no failures
  cargo sim nightly restart-delayed                      # long-downtime catch-up
  NODE_COUNT=5 CONVERGENCE_WAIT_SECONDS=30 cargo sim nightly
  IDEAL_BLOCK_TIME=1 SHARES_PER_BLOCK=100 cargo sim nightly   # time-compressed
  IDEAL_BLOCK_TIME=1 SHARES_PER_BLOCK=20 LATE_MINER=1 cargo sim nightly late-miner
  IDEAL_BLOCK_TIME=1 SHARES_PER_BLOCK=20 cargo sim nightly pruned-join
  IDEAL_BLOCK_TIME=1 SHARES_PER_BLOCK=20 NODE_COUNT=6 FORK_GROUP_FROM=4 HASHRATE=1e13 \\
    cargo sim nightly deep-fork-refused

Env (nightly-specific):
  SCENARIO                   failure scenario to inject    (default clean)
  NODE_COUNT                 number of sim nodes           (default 20)
  CONVERGENCE_WAIT_SECONDS   settle time after the scenario, before the checks (default 60)
  UNCLE_RATE_THRESHOLD       max uncle rate % to pass      (default 25)
  BITCOIND_DATADIR           regtest data directory        (default /tmp/bitcoind-p2poolv2)
  BITCOIND_BIN               path to BCHN's bitcoind        (default: bitcoind on PATH)

Every run-swarm and failures variable (RUN_DIR, RPC_URL, SHARES_PER_BLOCK,
LATENCY_MS, IDEAL_BLOCK_TIME, FAILURE_SEED, ...) applies as well.";

/// The sim swarm harness: launch, observe, measure, break and stop a swarm
/// of no-PoW sim nodes against a BCHN regtest node, or run the whole nightly
/// test. Configured through environment variables; see each command's
/// --help.
#[derive(Parser)]
#[command(name = "sim_harness")]
struct Arguments {
    #[command(subcommand)]
    command: HarnessCommand,
}

#[derive(Subcommand)]
enum HarnessCommand {
    /// Build the sim binary and launch a swarm of N nodes against one shared
    /// BCHN regtest node. The nodes keep running; stop them with stop-swarm.
    #[command(after_help = SWARM_HELP)]
    RunSwarm {
        /// Number of nodes.
        #[arg(default_value_t = 20)]
        node_count: usize,
    },
    /// Stop the swarm in RUN_DIR (SIGTERM, then SIGKILL after
    /// STOP_GRACE_SECONDS, default 10) and wait until every node has exited.
    StopSwarm,
    /// Live snapshot: tip height and hash (from logs), peers (from the API),
    /// alive, convergence, and an error scan. Env: RUN_DIR, BASE_API.
    Observe,
    /// The authoritative log-based summary of a run: convergence, share rate,
    /// difficulty, emission against hashrate, uncles and health. Env: RUN_DIR.
    Metrics,
    /// Inject one failure scenario into a running swarm and verify recovery.
    /// Writes RUN_DIR/failure-results.txt.
    #[command(after_help = FAILURES_HELP)]
    Failures {
        /// Scenario to inject.
        #[arg(default_value = "clean")]
        scenario: String,
    },
    /// End-to-end nightly run with a pass/fail verdict: BCHN, swarm,
    /// failure scenario, convergence, metrics, verify_chain on every store.
    #[command(after_help = NIGHTLY_HELP)]
    Nightly {
        /// Scenario to inject; defaults to SCENARIO, then clean.
        scenario: Option<String>,
    },
    /// Check from the regtest coinbases that a late miner is paid only once
    /// its shares are mature. Exit 0 pass, 1 rule violated, 2 inconclusive,
    /// 3 setup error. Env: RUN_DIR, LATE_MINER (required), BASE_API,
    /// RPC_URL, RPC_USER, RPC_PASS.
    MaturityCheck,
    /// Plot share rate, pool difficulty, uncle rate, block-finds and
    /// emission against hashrate to an SVG. Env: RUN_DIR.
    PlotMetrics {
        /// Output file (default RUN_DIR/metrics.svg).
        output: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    let arguments = Arguments::parse();
    ExitCode::from(dispatch(arguments.command).await)
}

/// Print `error` and return exit code 1.
fn failed(error: impl std::fmt::Display) -> u8 {
    eprintln!("{error}");
    1
}

async fn dispatch(command: HarnessCommand) -> u8 {
    match command {
        HarnessCommand::RunSwarm { node_count } => {
            let mut processes = ProcessTable::new();
            match SwarmSettings::from_env(node_count) {
                Ok(settings) => match run_swarm(&settings, &mut processes).await {
                    Ok(_) => 0,
                    Err(error) => failed(error),
                },
                Err(error) => failed(error),
            }
        }
        HarnessCommand::StopSwarm => {
            let grace = match env_vars::parsed("STOP_GRACE_SECONDS", DEFAULT_GRACE_SECONDS) {
                Ok(grace) => grace,
                Err(error) => return failed(error),
            };
            let mut processes = ProcessTable::new();
            u8::from(!stop_swarm(&RunDir::from_env(), &mut processes, grace).await)
        }
        HarnessCommand::Observe => {
            let base_api = match env_vars::parsed("BASE_API", 7600) {
                Ok(base_api) => base_api,
                Err(error) => return failed(error),
            };
            match observe::snapshot(&RunDir::from_env(), base_api).await {
                Ok(text) => {
                    print!("{text}");
                    0
                }
                Err(error) => failed(error),
            }
        }
        HarnessCommand::Metrics => match metrics::collect(&RunDir::from_env()) {
            Ok(report) => {
                print!("{}", report.text);
                0
            }
            Err(error) => failed(error),
        },
        HarnessCommand::Failures { scenario } => {
            let settings = match FailureSettings::from_env(&scenario) {
                Ok(settings) => settings,
                Err(error) => return failed(error),
            };
            let mut processes = ProcessTable::new();
            match inject(&settings, &mut processes).await {
                Ok(summary) => u8::from(!summary.passed()),
                Err(exit) => {
                    eprintln!("{}", exit.message);
                    exit.code
                }
            }
        }
        HarnessCommand::Nightly { scenario } => match NightlySettings::from_env(scenario) {
            Ok(settings) => nightly::run(&settings).await,
            Err(error) => failed(error),
        },
        HarnessCommand::MaturityCheck => {
            let check = match (
                env_vars::optional("LATE_MINER"),
                env_vars::parsed("BASE_API", 7600),
            ) {
                (Ok(late_miner), Ok(base_api)) => MaturityCheck {
                    run_dir: RunDir::from_env(),
                    late_miner,
                    base_api,
                },
                (Err(error), _) | (_, Err(error)) => {
                    println!("maturity check: SETUP ERROR ({error})");
                    return Verdict::SetupError as u8;
                }
            };
            let bitcoind = match Bitcoind::new(&RpcSettings::from_env()) {
                Ok(bitcoind) => bitcoind,
                Err(error) => {
                    println!("maturity check: SETUP ERROR ({error})");
                    return Verdict::SetupError as u8;
                }
            };
            let (verdict, report) = check.run(&bitcoind).await;
            print!("{report}");
            verdict as u8
        }
        HarnessCommand::PlotMetrics { output } => {
            let run_dir = RunDir::from_env();
            let output = output.unwrap_or_else(|| run_dir.path().join("metrics.svg"));
            if output
                .extension()
                .is_some_and(|extension| !extension.eq_ignore_ascii_case("svg"))
            {
                return failed(format!(
                    "plot-metrics writes SVG; give an output path ending in .svg, not {}",
                    output.display()
                ));
            }
            match plot::plot(&run_dir, &output) {
                Ok(line) => {
                    println!("{line}");
                    0
                }
                Err(error) => failed(error),
            }
        }
    }
}
