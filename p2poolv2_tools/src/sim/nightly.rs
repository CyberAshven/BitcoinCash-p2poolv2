// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The nightly simulation: an end-to-end sim run with a pass/fail verdict.
//! Replaces `load-tests/sim/nightly.sh`.
//!
//! Starts a regtest bitcoind if none is running, launches a swarm, injects
//! a failure scenario, waits for convergence, collects metrics, stops the
//! swarm, verifies every store, evaluates the pass/fail thresholds and
//! cleans up. The nightly workflow runs it once per scenario, so each
//! failure mode gets its own swarm, verdict and artifacts.

use crate::cargo_build::{BuildRequest, build, executable};
use crate::env_vars;
use crate::error::{ToolError, ToolResult};
use crate::process::{ProcessTable, find_on_path};
use crate::repo;
use crate::sim::bitcoind::{Bitcoind, CallOutcome, RpcSettings};
use crate::sim::failures::{FailureSettings, FailureSummary, inject};
use crate::sim::metrics::{self, MetricsSummary};
use crate::sim::stop::{DEFAULT_GRACE_SECONDS, stop_swarm};
use crate::sim::swarm::{SwarmSettings, run_swarm};
use crate::sim::{Exit, RunDir, check_scenario, log_message};
use serde_json::Value;
use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// The regtest RPC port the nightly starts bitcoind on.
const REGTEST_RPC_PORT: u16 = 19443;

/// The nightly's own settings, from the same environment variables
/// `nightly.sh` read. The swarm and failure settings come from the
/// environment too, as they did for the scripts it ran.
#[derive(Debug, Clone)]
pub struct NightlySettings {
    pub scenario: String,
    pub node_count: usize,
    pub convergence_wait: u64,
    /// `UNCLE_RATE_THRESHOLD` as given, for the results table.
    pub uncle_rate_threshold: String,
    pub bitcoind_datadir: PathBuf,
    pub bitcoind_binary: Option<PathBuf>,
    pub rpc: RpcSettings,
    pub zmq: String,
    pub run_dir: RunDir,
    pub profile: String,
}

impl NightlySettings {
    /// Settings from the environment; `scenario` is the command line
    /// argument, which wins over `SCENARIO`.
    pub fn from_env(scenario: Option<String>) -> ToolResult<Self> {
        Ok(Self {
            scenario: scenario.unwrap_or_else(|| env_vars::string("SCENARIO", "clean")),
            node_count: env_vars::parsed("NODE_COUNT", 20)?,
            convergence_wait: env_vars::parsed("CONVERGENCE_WAIT_SECONDS", 60)?,
            uncle_rate_threshold: env_vars::string("UNCLE_RATE_THRESHOLD", "25"),
            bitcoind_datadir: PathBuf::from(env_vars::string(
                "BITCOIND_DATADIR",
                "/tmp/bitcoind-p2poolv2",
            )),
            bitcoind_binary: env_vars::optional_string("BITCOIND_BIN").map(PathBuf::from),
            rpc: RpcSettings::from_env(),
            zmq: env_vars::string("ZMQ", "tcp://127.0.0.1:28332"),
            run_dir: RunDir::from_env(),
            profile: env_vars::string("PROFILE", "release"),
        })
    }
}

/// `BITCOIND_BIN`, else Homebrew's bitcoind, else bitcoind on `PATH`.
fn find_bitcoind(settings: &NightlySettings) -> Result<PathBuf, Exit> {
    if let Some(binary) = &settings.bitcoind_binary {
        return Ok(binary.clone());
    }
    let homebrew = PathBuf::from("/opt/homebrew/bin/bitcoind");
    if homebrew.is_file() {
        return Ok(homebrew);
    }
    find_on_path("bitcoind").ok_or_else(|| {
        Exit::failure("ERROR: bitcoind not found. Set BITCOIND_BIN or install bitcoind.")
    })
}

/// The binaries the post-swarm checks run.
struct AnalysisBinaries {
    verify_chain: PathBuf,
    cli: PathBuf,
}

/// Build `verify_chain`, for structural integrity, and `p2poolv2_cli`, for
/// the store-based convergence check.
///
/// Always built, never just checked for: both decode share headers out of
/// the node stores, so a binary older than the last consensus change reads
/// them with the wrong format and reports every header as corrupt, which
/// looks like data loss. Cargo is incremental, so this is nearly free when
/// already current. Both live in `p2poolv2_node` and are built with the
/// `sim` feature, so `verify_chain` checks bodies against the sim build's
/// prune depth rather than expecting every body the sim nodes pruned.
fn build_analysis_binaries(profile: &str) -> ToolResult<AnalysisBinaries> {
    log_message(&format!("Building verify_chain and p2poolv2_cli ({profile})..."));
    let built = build(
        &repo::root(),
        &BuildRequest {
            packages: vec!["p2poolv2_node".to_string()],
            binaries: vec!["verify_chain".to_string(), "p2poolv2_cli".to_string()],
            features: vec!["debug-tools".to_string(), "sim".to_string()],
            release: profile == "release",
            environment: Vec::new(),
        },
    )?;
    Ok(AnalysisBinaries {
        verify_chain: executable(&built, "verify_chain")?,
        cli: executable(&built, "p2poolv2_cli")?,
    })
}

/// What the run has started and must clean up.
#[derive(Debug, Default)]
struct Lifecycle {
    started_bitcoind: bool,
    swarm_running: bool,
}

/// Start a fresh regtest bitcoind unless one already answers.
async fn ensure_bitcoind_running(
    settings: &NightlySettings,
    bitcoind_binary: &Path,
    bitcoind: &Bitcoind,
    lifecycle: &mut Lifecycle,
) -> Result<(), Exit> {
    if bitcoind.is_regtest().await {
        log_message(&format!(
            "regtest bitcoind already running at {}",
            settings.rpc.url
        ));
        return Ok(());
    }
    log_message("Starting fresh regtest bitcoind...");
    let datadir = &settings.bitcoind_datadir;
    if datadir.exists() {
        std::fs::remove_dir_all(datadir).map_err(|error| ToolError::io("remove", datadir, error))?;
    }
    std::fs::create_dir_all(datadir).map_err(|error| ToolError::io("create", datadir, error))?;
    let status = Command::new(bitcoind_binary)
        .arg("-regtest")
        .arg(format!("-datadir={}", datadir.display()))
        .arg("-daemon")
        .arg(format!("-rpcport={REGTEST_RPC_PORT}"))
        .arg(format!("-rpcuser={}", settings.rpc.user))
        .arg(format!("-rpcpassword={}", settings.rpc.password))
        .arg(format!("-zmqpubhashblock={}", settings.zmq))
        .arg("-fallbackfee=0.0001")
        .stdin(Stdio::null())
        .status()
        .map_err(|error| {
            Exit::failure(format!(
                "cannot start {}: {error}",
                bitcoind_binary.display()
            ))
        })?;
    if !status.success() {
        return Err(Exit::failure(format!("bitcoind failed to start ({status})")));
    }

    const MAX_ATTEMPTS: u32 = 30;
    for attempt in 1..=MAX_ATTEMPTS {
        if bitcoind.is_regtest().await {
            lifecycle.started_bitcoind = true;
            log_message(&format!("bitcoind ready (attempt {attempt})"));
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    Err(Exit::failure(format!(
        "ERROR: bitcoind did not become ready within {MAX_ATTEMPTS}s"
    )))
}

/// Make sure the `p2pool` wallet is loaded and the chain is past coinbase
/// maturity, so the swarm's nodes can get payout addresses.
async fn ensure_wallet_and_coins(bitcoind: &Bitcoind) -> ToolResult<()> {
    log_message("Ensuring wallet and coins...");
    let created = bitcoind
        .call("createwallet", vec![Value::from("p2pool")])
        .await;
    if !matches!(created, Ok(CallOutcome::Result(_))) {
        let _ = bitcoind
            .call("loadwallet", vec![Value::from("p2pool")])
            .await;
    }
    let block_count = bitcoind
        .result("getblockcount", Vec::new())
        .await
        .ok()
        .and_then(|count| count.as_u64())
        .unwrap_or(0);
    if block_count < 101 {
        let blocks_needed = 101 - block_count;
        let address = bitcoind.result("getnewaddress", Vec::new()).await?;
        log_message(&format!(
            "Mining {blocks_needed} blocks to reach height 101..."
        ));
        bitcoind
            .result(
                "generatetoaddress",
                vec![Value::from(blocks_needed), address],
            )
            .await?;
    }
    let height = bitcoind.result("getblockcount", Vec::new()).await?;
    log_message(&format!("Wallet ready, block height: {height}"));
    Ok(())
}

/// Live and total PIDs in `pids.txt`; an empty line is not a node.
fn count_alive(run_dir: &RunDir, processes: &mut ProcessTable) -> (usize, usize) {
    let text = std::fs::read_to_string(run_dir.pids_file()).unwrap_or_default();
    let mut alive = 0;
    let mut total = 0;
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        total += 1;
        if line
            .parse::<u32>()
            .is_ok_and(|pid| processes.is_alive(pid))
        {
            alive += 1;
        }
    }
    (alive, total)
}

/// Run a `p2poolv2_cli` query against a store and return its stdout. On a
/// non-zero exit the CLI's first stderr lines go to our stderr: a silently
/// empty result is indistinguishable from an empty chain, which made a
/// failed nightly undiagnosable from its artifact.
fn query_store(cli: &Path, store: &Path, arguments: &[&str]) -> String {
    let output = Command::new(cli)
        .arg("--db-path")
        .arg(store)
        .args(arguments)
        .stdin(Stdio::null())
        .output();
    match output {
        Ok(output) => {
            if !output.status.success() {
                eprintln!(
                    "[{}]   p2poolv2_cli {} failed on {} (exit {}):",
                    chrono::Local::now().format("%H:%M:%S"),
                    arguments.join(" "),
                    store.display(),
                    output.status.code().map_or_else(|| "signal".to_string(), |code| code.to_string())
                );
                for line in String::from_utf8_lossy(&output.stderr).lines().take(5) {
                    eprintln!("    {line}");
                }
            }
            String::from_utf8_lossy(&output.stdout).into_owned()
        }
        Err(error) => {
            eprintln!("cannot run {}: {error}", cli.display());
            String::new()
        }
    }
}

/// The first share in a `p2poolv2_cli shares` answer.
fn first_share(output: &str) -> Option<Value> {
    let parsed: Value = serde_json::from_str(output).ok()?;
    parsed.get("shares")?.as_array()?.first().cloned()
}

/// Confirmed tip height of a store. Uses `shares --num 1`, not `info`, which
/// reads chain-tip metadata that can fail to decode on some stores.
fn store_tip_height(cli: &Path, store: &Path) -> Option<u64> {
    first_share(&query_store(cli, store, &["shares", "--num", "1"]))?
        .get("height")?
        .as_u64()
}

/// Confirmed block hash at `height` in a store, or `None` when the store
/// has no confirmed block there.
fn store_hash_at_height(cli: &Path, store: &Path, height: u64) -> Option<String> {
    let height_text = height.to_string();
    let share = first_share(&query_store(
        cli,
        store,
        &["shares", "--to", &height_text, "--num", "1"],
    ))?;
    if share.get("height")?.as_u64()? != height {
        return None;
    }
    share.get("blockhash")?.as_str().map(str::to_string)
}

/// The store-based convergence check.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Convergence {
    converged: bool,
    distinct_hashes: String,
    within_two: String,
}

/// Convergence is checked against the stores, not the logs. Reorgs resolve
/// transient forks, but a confirmed-chain reorg does not log per-block
/// promotions, so a log-based check reads stale pre-reorg hashes and
/// reports false divergence. Compare the confirmed hash two heights below
/// the shallowest tip, a height every node has confirmed, across all
/// stores: converged when they all agree and are within two of the deepest
/// tip.
fn check_chain_converged(cli: &Path, run_dir: &RunDir, node_count: usize) -> Convergence {
    let stores: Vec<PathBuf> = (0..node_count)
        .map(|index| run_dir.node_store(index))
        .filter(|store| store.is_dir())
        .collect();
    let tips: Vec<u64> = stores
        .iter()
        .filter_map(|store| store_tip_height(cli, store))
        .collect();
    let (Some(lowest), Some(highest)) = (tips.iter().min().copied(), tips.iter().max().copied())
    else {
        eprintln!(
            "[{}]   no confirmed tip readable from any store in {} (see the p2poolv2_cli errors above)",
            chrono::Local::now().format("%H:%M:%S"),
            run_dir.path().display()
        );
        return Convergence {
            converged: false,
            distinct_hashes: "-1".to_string(),
            within_two: format!("0/{node_count}"),
        };
    };
    let within = tips.iter().filter(|tip| **tip + 2 >= highest).count();
    let converge_height = if lowest > 2 { lowest - 2 } else { lowest };
    let hashes: Vec<String> = stores
        .iter()
        .filter_map(|store| store_hash_at_height(cli, store, converge_height))
        .collect();
    let distinct: HashSet<&String> = hashes.iter().collect();
    let distinct_count = if hashes.is_empty() {
        -1
    } else {
        distinct.len() as i64
    };
    Convergence {
        converged: distinct_count == 1 && hashes.len() == node_count && within == node_count,
        distinct_hashes: distinct_count.to_string(),
        within_two: format!("{within}/{node_count}"),
    }
}

/// Run `verify_chain` on every node store. Returns failures and total.
fn verify_all_chains(verify_chain: &Path, run_dir: &RunDir, node_count: usize) -> (usize, usize) {
    log_message(&format!(
        "Running verify_chain on all {node_count} node stores..."
    ));
    let mut failures = 0;
    for index in 0..node_count {
        let store = run_dir.node_store(index);
        if !store.is_dir() {
            log_message(&format!(
                "  node {index}: FAIL (store not found at {})",
                store.display()
            ));
            failures += 1;
        } else {
            let report = run_dir.path().join(format!("verify-{index}.log"));
            let passed = std::fs::File::create(&report)
                .and_then(|output| {
                    let errors = output.try_clone()?;
                    Command::new(verify_chain)
                        .arg(&store)
                        .stdin(Stdio::null())
                        .stdout(output)
                        .stderr(errors)
                        .status()
                })
                .is_ok_and(|status| status.success());
            if passed {
                log_message(&format!("  node {index}: PASS"));
            } else {
                log_message(&format!(
                    "  node {index}: FAIL (see {})",
                    report.display()
                ));
                failures += 1;
            }
        }
    }
    (failures, node_count)
}

/// Everything the verdict reads.
#[derive(Debug, Clone)]
pub struct Evidence {
    pub alive: (usize, usize),
    pub convergence: Option<(bool, String, String)>,
    pub metrics: Option<MetricsSummary>,
    pub verify: (usize, usize),
    pub failures: Option<FailureSummary>,
}

fn pass_fail(passed: bool) -> &'static str {
    if passed { "PASS" } else { "FAIL" }
}

/// The results table and whether the run passed.
pub fn evaluate(scenario: &str, uncle_rate_threshold: &str, evidence: &Evidence) -> (String, bool) {
    let mut failed = false;
    let mut check = |passed: bool| {
        failed |= !passed;
        pass_fail(passed)
    };

    let (alive_count, alive_total) = evidence.alive;
    let alive_result = check(alive_count == alive_total && alive_total > 0);

    // deep-fork-refused ends with two chains by design: the minority refuses
    // the majority's deep fork. Its own verdict compares the two groups.
    let (converged_result, distinct, within) = match &evidence.convergence {
        None => ("SKIP", "n/a".to_string(), "n/a".to_string()),
        Some((converged, distinct, within)) => (check(*converged), distinct.clone(), within.clone()),
    };

    // Values the scripts read back from the metrics text, with the same
    // fallbacks when the metrics could not be collected.
    let promotions = evidence.metrics.as_ref().map_or(0, |metrics| metrics.promotions);
    let grew_result = check(promotions > 0);
    let panics = evidence
        .metrics
        .as_ref()
        .map_or(-1, |metrics| metrics.panicked_nodes as i64);
    let panics_result = check(panics == 0);
    let asert = evidence
        .metrics
        .as_ref()
        .map_or(-1, |metrics| metrics.asert_mismatches as i64);
    let merkle = evidence
        .metrics
        .as_ref()
        .map_or(-1, |metrics| metrics.merkle_payout_mismatches as i64);
    let rejections_result = check(asert == 0 && merkle == 0);
    let uncle_rate = evidence
        .metrics
        .as_ref()
        .and_then(|metrics| metrics.uncle_rate.clone())
        .unwrap_or_else(|| "0".to_string());
    let threshold: f64 = uncle_rate_threshold.trim().parse().unwrap_or(0.0);
    let uncle_result = check(uncle_rate.parse::<f64>().unwrap_or(0.0) < threshold);

    let (verify_failures, verify_total) = evidence.verify;
    let verify_result = check(verify_failures == 0);

    let (targets, rejoined, unrecovered) = match &evidence.failures {
        Some(summary) => (
            if summary.targets.is_empty() {
                "unknown".to_string()
            } else {
                summary.targets.clone()
            },
            summary.rejoined.clone(),
            summary.unrecovered as i64,
        ),
        None => ("unknown".to_string(), "unknown".to_string(), -1),
    };
    let recovery_result = check(unrecovered == 0);

    let verdict_of = |pick: fn(&FailureSummary) -> &Option<String>| {
        evidence
            .failures
            .as_ref()
            .and_then(|summary| pick(summary).clone())
            .unwrap_or_else(|| "missing".to_string())
    };
    let (maturity_result, maturity) = if scenario == "late-miner" {
        let verdict = verdict_of(|summary| &summary.maturity_verdict);
        (check(verdict == "PASS"), verdict)
    } else {
        ("SKIP", "late-miner only".to_string())
    };
    let (join_result, join) = if scenario == "pruned-join" {
        let verdict = verdict_of(|summary| &summary.join_verdict);
        (check(verdict == "PASS"), verdict)
    } else {
        ("SKIP", "pruned-join only".to_string())
    };
    let (fork_result, fork) =
        if scenario == "deep-fork-refused" || scenario == "shallow-fork-reorged" {
            let verdict = verdict_of(|summary| &summary.fork_verdict);
            (check(verdict == "PASS"), verdict)
        } else {
            ("SKIP", "fork scenarios only".to_string())
        };

    let mut text = String::with_capacity(1024);
    let _ = writeln!(text, "=== NIGHTLY SIM RESULTS (scenario: {scenario}) ===");
    let _ = writeln!(text, "  nodes alive:        {alive_result:<4}  ({alive_count}/{alive_total})");
    let _ = writeln!(
        text,
        "  chain converged:    {converged_result:<4}  (distinct hashes: {distinct}, within 2 of tip: {within})"
    );
    let _ = writeln!(text, "  chain grew:         {grew_result:<4}  (promotions: {promotions})");
    let _ = writeln!(text, "  no panics:          {panics_result:<4}  (panicked nodes: {panics})");
    let _ = writeln!(
        text,
        "  no rejections:      {rejections_result:<4}  (AsertMismatch={asert} merkle/payout={merkle})"
    );
    let _ = writeln!(
        text,
        "  uncle rate:         {uncle_result:<4}  ({uncle_rate}% < {uncle_rate_threshold}%)"
    );
    let _ = writeln!(
        text,
        "  verify_chain:       {verify_result:<4}  ({}/{verify_total} passed)",
        verify_total - verify_failures
    );
    let _ = writeln!(
        text,
        "  failure recovery:   {recovery_result:<4}  (targets: {targets}, rejoined {rejoined}, unrecovered {unrecovered})"
    );
    let _ = writeln!(text, "  payout maturity:    {maturity_result:<4}  ({maturity})");
    let _ = writeln!(text, "  pruned join:        {join_result:<4}  ({join})");
    let _ = writeln!(text, "  fork depth limit:   {fork_result:<4}  ({fork})");
    let _ = writeln!(text, "RESULT: {}", pass_fail(!failed));
    (text, !failed)
}

/// Run the nightly: everything after the binaries are built, with cleanup
/// left to the caller.
async fn run_after_build(
    settings: &NightlySettings,
    binaries: &AnalysisBinaries,
    bitcoind_binary: &Path,
    lifecycle: &mut Lifecycle,
    processes: &mut ProcessTable,
) -> Result<bool, Exit> {
    let bitcoind = Bitcoind::new(&settings.rpc)?;
    ensure_bitcoind_running(settings, bitcoind_binary, &bitcoind, lifecycle).await?;
    ensure_wallet_and_coins(&bitcoind).await?;

    log_message(&format!(
        "Starting {}-node sim swarm...",
        settings.node_count
    ));
    let swarm = SwarmSettings::from_env(settings.node_count)?;
    // A swarm that fails part-way through its launch still has nodes to stop.
    lifecycle.swarm_running = true;
    let sim_binary = run_swarm(&swarm, processes).await?;

    // The failure scenario's progress streams straight through, so a long
    // scenario is visible in the CI log while it runs.
    log_message(&format!(
        "Injecting failure scenario '{}'...",
        settings.scenario
    ));
    let mut failure_settings = FailureSettings::from_env(&settings.scenario)?;
    failure_settings.sim_binary = Some(sim_binary);
    let failures = match inject(&failure_settings, processes).await {
        Ok(summary) => Some(summary),
        Err(exit) => {
            eprintln!("{}", exit.message);
            None
        }
    };

    log_message(&format!(
        "Waiting {}s for convergence...",
        settings.convergence_wait
    ));
    tokio::time::sleep(Duration::from_secs(settings.convergence_wait)).await;
    let alive = count_alive(&settings.run_dir, processes);

    log_message("Collecting metrics...");
    let metrics = match metrics::collect(&settings.run_dir) {
        Ok(report) => {
            println!("\n{}", report.text);
            Some(report.summary)
        }
        Err(error) => {
            println!("\n{error}\n");
            None
        }
    };

    log_message("Stopping swarm...");
    stop_swarm(&settings.run_dir, processes, DEFAULT_GRACE_SECONDS).await;
    lifecycle.swarm_running = false;

    let verify = verify_all_chains(&binaries.verify_chain, &settings.run_dir, settings.node_count);
    let convergence = if settings.scenario == "deep-fork-refused" {
        None
    } else {
        let convergence = check_chain_converged(&binaries.cli, &settings.run_dir, settings.node_count);
        Some((
            convergence.converged,
            convergence.distinct_hashes,
            convergence.within_two,
        ))
    };
    let (table, passed) = evaluate(
        &settings.scenario,
        &settings.uncle_rate_threshold,
        &Evidence {
            alive,
            convergence,
            metrics,
            verify,
            failures,
        },
    );
    print!("{table}");
    Ok(passed)
}

/// Stop what the run started.
async fn cleanup(
    settings: &NightlySettings,
    bitcoind: Option<&Bitcoind>,
    lifecycle: &mut Lifecycle,
    processes: &mut ProcessTable,
) {
    log_message("Cleaning up...");
    if lifecycle.swarm_running {
        log_message("Stopping swarm...");
        stop_swarm(&settings.run_dir, processes, DEFAULT_GRACE_SECONDS).await;
        lifecycle.swarm_running = false;
    }
    if lifecycle.started_bitcoind {
        log_message("Stopping bitcoind (started by this script)...");
        if let Some(bitcoind) = bitcoind {
            let _ = bitcoind.call("stop", Vec::new()).await;
        }
        lifecycle.started_bitcoind = false;
    }
}

/// Run the nightly. Returns the process exit code: 0 pass, 1 fail, 2 bad
/// scenario, 130 interrupted.
pub async fn run(settings: &NightlySettings) -> u8 {
    if let Err(error) = check_scenario(&settings.scenario) {
        eprintln!("ERROR: {error}");
        return 2;
    }
    let prepared = find_bitcoind(settings).and_then(|bitcoind_binary| {
        let binaries = build_analysis_binaries(&settings.profile)?;
        Ok((bitcoind_binary, binaries))
    });
    let (bitcoind_binary, binaries) = match prepared {
        Ok(prepared) => prepared,
        Err(exit) => {
            eprintln!("{}", exit.message);
            return exit.code;
        }
    };

    let mut lifecycle = Lifecycle::default();
    let mut processes = ProcessTable::new();
    let outcome = tokio::select! {
        outcome = run_after_build(settings, &binaries, &bitcoind_binary, &mut lifecycle, &mut processes) => Some(outcome),
        _ = tokio::signal::ctrl_c() => None,
    };
    let bitcoind = Bitcoind::new(&settings.rpc).ok();
    cleanup(settings, bitcoind.as_ref(), &mut lifecycle, &mut processes).await;
    match outcome {
        Some(Ok(true)) => 0,
        Some(Ok(false)) => 1,
        Some(Err(exit)) => {
            eprintln!("{}", exit.message);
            exit.code
        }
        None => 130,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passing_evidence() -> Evidence {
        Evidence {
            alive: (3, 3),
            convergence: Some((true, "1".to_string(), "3/3".to_string())),
            metrics: Some(MetricsSummary {
                promotions: 50,
                panicked_nodes: 0,
                asert_mismatches: 0,
                merkle_payout_mismatches: 0,
                uncle_rate: Some("12.5".to_string()),
            }),
            verify: (0, 3),
            failures: Some(FailureSummary {
                text: String::new(),
                targets: "none".to_string(),
                rejoined: "0/0".to_string(),
                unrecovered: 0,
                maturity_verdict: None,
                join_verdict: None,
                fork_verdict: None,
            }),
        }
    }

    #[test]
    fn clean_run_passes() {
        let (text, passed) = evaluate("clean", "25", &passing_evidence());
        assert!(passed);
        assert_eq!(
            text,
            concat!(
                "=== NIGHTLY SIM RESULTS (scenario: clean) ===\n",
                "  nodes alive:        PASS  (3/3)\n",
                "  chain converged:    PASS  (distinct hashes: 1, within 2 of tip: 3/3)\n",
                "  chain grew:         PASS  (promotions: 50)\n",
                "  no panics:          PASS  (panicked nodes: 0)\n",
                "  no rejections:      PASS  (AsertMismatch=0 merkle/payout=0)\n",
                "  uncle rate:         PASS  (12.5% < 25%)\n",
                "  verify_chain:       PASS  (3/3 passed)\n",
                "  failure recovery:   PASS  (targets: none, rejoined 0/0, unrecovered 0)\n",
                "  payout maturity:    SKIP  (late-miner only)\n",
                "  pruned join:        SKIP  (pruned-join only)\n",
                "  fork depth limit:   SKIP  (fork scenarios only)\n",
                "RESULT: PASS\n",
            )
        );
    }

    #[test]
    fn uncle_rate_at_the_threshold_fails() {
        let mut evidence = passing_evidence();
        if let Some(metrics) = evidence.metrics.as_mut() {
            metrics.uncle_rate = Some("25.0".to_string());
        }
        assert!(!evaluate("clean", "25", &evidence).1);
    }

    #[test]
    fn missing_metrics_fail_with_script_fallbacks() {
        let mut evidence = passing_evidence();
        evidence.metrics = None;
        let (text, passed) = evaluate("clean", "25", &evidence);
        assert!(!passed);
        assert!(text.contains("  no panics:          FAIL  (panicked nodes: -1)\n"));
        assert!(text.contains("  uncle rate:         PASS  (0% < 25%)\n"));
    }

    #[test]
    fn missing_failure_summary_fails_recovery() {
        let mut evidence = passing_evidence();
        evidence.failures = None;
        let (text, passed) = evaluate("restart-fast", "25", &evidence);
        assert!(!passed);
        assert!(text.contains(
            "  failure recovery:   FAIL  (targets: unknown, rejoined unknown, unrecovered -1)\n"
        ));
    }

    #[test]
    fn late_miner_needs_a_passing_maturity_verdict() {
        let mut evidence = passing_evidence();
        let (text, passed) = evaluate("late-miner", "25", &evidence);
        assert!(!passed);
        assert!(text.contains("  payout maturity:    FAIL  (missing)\n"));
        if let Some(summary) = evidence.failures.as_mut() {
            summary.maturity_verdict = Some("PASS".to_string());
        }
        assert!(evaluate("late-miner", "25", &evidence).1);
    }

    #[test]
    fn deep_fork_skips_convergence() {
        let mut evidence = passing_evidence();
        evidence.convergence = None;
        let (text, _) = evaluate("deep-fork-refused", "25", &evidence);
        assert!(text.contains("  chain converged:    SKIP  (distinct hashes: n/a, within 2 of tip: n/a)\n"));
    }

    #[test]
    fn first_share_reads_the_cli_answer() {
        let share = first_share(r#"{"from_height":0,"to_height":5,"shares":[{"height":5,"blockhash":"ab"}]}"#);
        assert_eq!(share.and_then(|share| share.get("height").and_then(Value::as_u64)), Some(5));
    }
}
