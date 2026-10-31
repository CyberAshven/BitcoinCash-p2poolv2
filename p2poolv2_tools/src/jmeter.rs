// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! JMeter stratum load-test drivers. Replaces `benchmark.sh`,
//! `flamegraph.sh`, `run_remote_load.sh` and `run_jmeter.sh` in
//! `load-tests/jmeter-testing/`.
//!
//! JMeter itself (a Java program) is still the load generator and
//! `stratum.jmx` its test plan; these drivers start the pool under test and
//! the mock bitcoind, run JMeter, and summarise its results.

use crate::cargo_build::{BuildRequest, build, executable};
use crate::error::{ToolError, ToolResult};
use crate::log_file::for_each_line;
use crate::mock_bitcoind::{DEFAULT_PORT, MockNode, NodeKind, serve};
use crate::process::{Signal, find_by_command_line, find_children, find_on_path, send_signal};
use crate::repo;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;

/// Seconds to let a pool settle after starting it.
const SETTLE_SECONDS: u64 = 5;

/// The stratum port the load test drives.
const STRATUM_PORT: u16 = 3333;

/// `[HH:MM:SS] message`.
pub fn log(message: &str) {
    println!("[{}] {message}", chrono::Local::now().format("%H:%M:%S"));
}

fn timestamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
}

fn on_path(name: &str) -> bool {
    find_on_path(name).is_some()
}

/// Whether something holds `port`: it cannot be bound on all interfaces.
pub fn port_in_use(port: u16) -> bool {
    matches!(
        std::net::TcpListener::bind(("0.0.0.0", port)),
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse
    )
}

/// The numeric value `sort -n` and awk read from a field: its leading
/// number, or 0.
fn leading_number(text: &str) -> f64 {
    let trimmed = text.trim_start();
    let length = trimmed
        .char_indices()
        .take_while(|(index, character)| {
            character.is_ascii_digit() || *character == '.' || (*index == 0 && *character == '-')
        })
        .count();
    trimmed[..length].parse().unwrap_or(0.0)
}

/// One sampler's samples from a JTL results file.
#[derive(Debug, Default, Clone, PartialEq)]
struct SamplerSamples {
    elapsed: Vec<f64>,
    errors: usize,
}

/// A value as the shell printed it from the sorted file: whole numbers
/// without a fraction.
fn shown(value: f64) -> String {
    format!("{value}")
}

/// The statistics line for one sampler, from its sorted elapsed times.
fn sampler_line(label: &str, samples: &SamplerSamples) -> String {
    let count = samples.elapsed.len();
    if count == 0 {
        return format!("    {label}: no samples");
    }
    let sorted = &samples.elapsed;
    // awk sums the sorted values in order.
    let average = sorted.iter().sum::<f64>() / count as f64;
    let percentile = |percent: usize| shown(sorted[(count * percent).div_ceil(100).max(1) - 1]);
    let average = format!("{average:.1}");
    let (p50, p95, p99) = (percentile(50), percentile(95), percentile(99));
    let maximum = shown(sorted[count - 1]);
    format!(
        "  {label:<20} samples={count:<8} errors={:<5}  avg={average:<8} p50={p50:<8} p95={p95:<8} p99={p99:<8} max={maximum}",
        samples.errors
    )
}

/// Summary statistics per sampler from a JTL results file (JMeter CSV:
/// timeStamp,elapsed,label,responseCode,responseMessage,threadName,
/// dataType,success,...). Fields are split on every comma, as the script's
/// awk did.
pub fn summarize_jtl(path: &Path, label: &str) -> String {
    if !path.is_file() {
        return format!("  No results file found: {}\n", path.display());
    }
    let mut total = 0usize;
    let mut total_errors = 0usize;
    let mut samplers: BTreeMap<String, SamplerSamples> = BTreeMap::new();
    let mut header = true;
    let read = for_each_line(path, |line| {
        if header {
            header = false;
            return;
        }
        total += 1;
        let fields: Vec<&str> = line.split(',').collect();
        let success = fields.get(7).copied().unwrap_or("");
        total_errors += usize::from(success.contains("false"));
        let sampler = samplers
            .entry(fields.get(2).copied().unwrap_or("").to_string())
            .or_default();
        sampler
            .elapsed
            .push(leading_number(fields.get(1).copied().unwrap_or("")));
        sampler.errors += usize::from(success == "false");
    });
    if let Err(error) = read {
        return format!("  Cannot read {}: {error}\n", path.display());
    }
    if total == 0 {
        return format!("  No samples in {}\n", path.display());
    }
    let mut text = String::with_capacity(256 + samplers.len() * 128);
    let _ = writeln!(text);
    let _ = writeln!(text, "=== {label} ===");
    let _ = writeln!(text, "  Total samples: {total}  Errors: {total_errors}");
    let _ = writeln!(text);
    for (sampler, samples) in &mut samplers {
        samples.elapsed.sort_by(f64::total_cmp);
        let _ = writeln!(text, "{}", sampler_line(sampler, samples));
    }
    text
}

/// Run JMeter and print only its `summary` lines. True when JMeter exits
/// successfully.
fn run_jmeter_filtered(arguments: &[String]) -> ToolResult<bool> {
    let mut child = Command::new("jmeter")
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| ToolError::message(format!("cannot run jmeter: {error}")))?;
    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if line.starts_with("summary") {
                println!("{line}");
            }
        }
    }
    let status = child
        .wait()
        .map_err(|error| ToolError::message(format!("cannot wait for jmeter: {error}")))?;
    Ok(status.success())
}

/// Run the load test plan non-interactively, writing samples to `results`
/// and JMeter's own log next to it.
fn run_jmeter(plan: &Path, results: &Path) -> ToolResult<()> {
    let jmeter_log = results.with_file_name(format!(
        "{}-jmeter.log",
        results
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default()
    ));
    log(&format!(
        "Running jmeter load test -> {}",
        results.display()
    ));
    run_jmeter_filtered(&[
        "-n".to_string(),
        "-t".to_string(),
        plan.display().to_string(),
        "-l".to_string(),
        results.display().to_string(),
        "-j".to_string(),
        jmeter_log.display().to_string(),
    ])?;
    log("jmeter complete");
    Ok(())
}

/// Serve the mock bitcoind in this process on its usual port. The server
/// runs until the process ends.
///
/// It answers as Bitcoin Core on signet: the JMeter plan, the P2Pool config
/// it drives (`config-load-test.toml`) and the CKPool configs all mine
/// Bitcoin signet, and CKPool reads `validateaddress` for its payout
/// address.
pub async fn start_mock_bitcoind() -> ToolResult<()> {
    log("Starting mock-bitcoind...");
    let node = Arc::new(MockNode::load(NodeKind::BitcoinCore, None)?);
    let listener = TcpListener::bind(("0.0.0.0", DEFAULT_PORT))
        .await
        .map_err(|error| {
            ToolError::message(format!(
                "ERROR: mock-bitcoind failed to start: cannot listen on port {DEFAULT_PORT}: {error}"
            ))
        })?;
    tokio::spawn(serve(
        listener,
        node,
        crate::mock_bitcoind::DEFAULT_MAX_CONNECTIONS,
    ));
    log(&format!(
        "mock-bitcoind running (in this process, port {DEFAULT_PORT})"
    ));
    Ok(())
}

/// A pool process under test, stopped on drop.
pub struct PoolProcess {
    name: &'static str,
    child: Child,
}

impl PoolProcess {
    /// Start `command` with its output in `log_file`, give it time to
    /// settle and check it is still running.
    pub async fn start(
        name: &'static str,
        command: &mut Command,
        log_file: &Path,
    ) -> ToolResult<Self> {
        log(&format!("Starting {name} (log: {})...", log_file.display()));
        let output = std::fs::File::create(log_file)
            .map_err(|error| ToolError::io("create", log_file, error))?;
        let errors = output
            .try_clone()
            .map_err(|error| ToolError::io("open", log_file, error))?;
        let child = command
            .stdin(Stdio::null())
            .stdout(output)
            .stderr(errors)
            .spawn()
            .map_err(|error| {
                ToolError::message(format!("ERROR: {name} failed to start: {error}"))
            })?;
        let mut process = Self { name, child };
        tokio::time::sleep(Duration::from_secs(SETTLE_SECONDS)).await;
        if !matches!(process.child.try_wait(), Ok(None)) {
            return Err(ToolError::message(format!("ERROR: {name} failed to start")));
        }
        log(&format!("{name} running (PID: {})", process.child.id()));
        Ok(process)
    }

    /// SIGTERM the process and wait for it to exit.
    pub fn stop(mut self) {
        self.terminate(true);
    }

    fn terminate(&mut self, announce: bool) {
        if matches!(self.child.try_wait(), Ok(None)) {
            if announce {
                log(&format!(
                    "Stopping {} (PID: {})...",
                    self.name,
                    self.child.id()
                ));
            }
            if !send_signal(&[self.child.id()], Signal::Terminate) {
                let _ = self.child.kill();
            }
            let _ = self.child.wait();
        }
    }
}

impl Drop for PoolProcess {
    fn drop(&mut self) {
        self.terminate(false);
    }
}

/// Options of the A/B benchmark.
#[derive(Debug, Clone)]
pub struct BenchmarkOptions {
    pub config: PathBuf,
    pub skip_default: bool,
    pub skip_native: bool,
    pub ckpool: Option<PathBuf>,
}

/// Build the pool binary, natively tuned or portable, and return its path.
fn build_pool(variant: &str) -> ToolResult<PathBuf> {
    log(&format!("Building P2Poolv2 ({variant})..."));
    let environment = if variant == "native" {
        vec![("RUSTFLAGS".to_string(), "-C target-cpu=native".to_string())]
    } else {
        Vec::new()
    };
    let built = build(
        &repo::root(),
        &BuildRequest {
            packages: vec!["p2poolv2_node".to_string()],
            binaries: vec!["p2poolv2".to_string()],
            features: Vec::new(),
            release: true,
            environment,
        },
    )?;
    log(&format!("Build complete ({variant})"));
    executable(&built, "p2poolv2")
}

async fn benchmark_variant(
    variant: &str,
    options: &BenchmarkOptions,
    plan: &Path,
    results: &Path,
    run_stamp: &str,
) -> ToolResult<PathBuf> {
    let binary = build_pool(variant)?;
    let mut command = Command::new(binary);
    command.arg(format!("--config={}", options.config.display()));
    let pool = PoolProcess::start(
        "P2Poolv2",
        &mut command,
        &results.join(format!("p2poolv2-{variant}-{run_stamp}.log")),
    )
    .await?;
    let samples = results.join(format!("p2poolv2-{variant}-{run_stamp}.jtl"));
    run_jmeter(plan, &samples)?;
    pool.stop();
    Ok(samples)
}

/// Compare the portable and native P2Poolv2 builds, and optionally CKPool,
/// under the same JMeter load against the in-process mock bitcoind.
pub async fn benchmark(options: &BenchmarkOptions) -> ToolResult<()> {
    let jmeter_dir = repo::jmeter_dir();
    let plan = jmeter_dir.join("stratum.jmx");
    let results = jmeter_dir.join("results");
    let ckpool_config = jmeter_dir.join("ckpool-testnet4-solo.json");

    let missing: Vec<&str> = ["jmeter", "cargo"]
        .into_iter()
        .filter(|tool| !on_path(tool))
        .collect();
    if !missing.is_empty() {
        let mut message = String::with_capacity(64);
        for tool in &missing {
            let _ = writeln!(message, "ERROR: {tool} is not installed or not in PATH");
        }
        return Err(ToolError::message(message.trim_end()));
    }
    if !plan.is_file() {
        return Err(ToolError::message(format!(
            "ERROR: jmeter test file not found: {}",
            plan.display()
        )));
    }
    if !options.config.is_file() {
        return Err(ToolError::message(format!(
            "ERROR: P2Poolv2 config not found: {}",
            options.config.display()
        )));
    }
    if let Some(ckpool) = &options.ckpool
        && !ckpool.is_file()
    {
        return Err(ToolError::message(format!(
            "ERROR: ckpool binary not found or not executable: {}",
            ckpool.display()
        )));
    }

    let run_stamp = timestamp();
    std::fs::create_dir_all(&results).map_err(|error| ToolError::io("create", &results, error))?;
    log(&format!("Starting benchmark run: {run_stamp}"));
    log(&format!("Config: {}", options.config.display()));
    start_mock_bitcoind().await?;

    let mut summaries: Vec<(PathBuf, &str)> = Vec::with_capacity(3);
    if !options.skip_default {
        log("--- Default build variant ---");
        let samples = benchmark_variant("default", options, &plan, &results, &run_stamp).await?;
        summaries.push((samples, "Default build (portable)"));
    }
    if !options.skip_native {
        log("--- Native build variant ---");
        let samples = benchmark_variant("native", options, &plan, &results, &run_stamp).await?;
        summaries.push((samples, "Native build (target-cpu=native)"));
    }
    if let Some(ckpool) = &options.ckpool {
        log("--- CKPool ---");
        log(&format!(
            "Waiting for port {STRATUM_PORT} to be released..."
        ));
        let mut waited = 0;
        while waited < 30 && port_in_use(STRATUM_PORT) {
            tokio::time::sleep(Duration::from_secs(1)).await;
            waited += 1;
        }
        let mut command = Command::new(ckpool);
        command
            .arg("--btcsolo")
            .arg(format!("--config={}", ckpool_config.display()));
        let pool = PoolProcess::start(
            "CKPool",
            &mut command,
            &results.join(format!("ckpool-{run_stamp}.log")),
        )
        .await?;
        let samples = results.join(format!("ckpool-{run_stamp}.jtl"));
        run_jmeter(&plan, &samples)?;
        pool.stop();
        summaries.push((samples, "CKPool"));
    }

    println!();
    println!("========================================");
    println!("  Benchmark Results: {run_stamp}");
    println!("========================================");
    for (samples, label) in &summaries {
        print!("{}", summarize_jtl(samples, label));
    }
    println!();
    println!("Raw results saved in: {}/", results.display());
    log("Benchmark complete");
    Ok(())
}

/// The last `count` lines of the file at `path`.
fn tail(path: &Path, count: usize) -> String {
    let mut lines: std::collections::VecDeque<String> =
        std::collections::VecDeque::with_capacity(count + 1);
    let _ = for_each_line(path, |line| {
        lines.push_back(line.to_string());
        if lines.len() > count {
            lines.pop_front();
        }
    });
    lines.into_iter().collect::<Vec<_>>().join("\n")
}

/// `cargo flamegraph` running the pool under perf, interrupted on drop.
struct ProfiledPool {
    child: Child,
}

impl ProfiledPool {
    /// SIGINT the `perf record` under cargo-flamegraph so it writes its data
    /// and the flamegraph is generated.
    fn interrupt_recording(&self) {
        let perf = find_by_command_line("perf record.*p2poolv2");
        if send_signal(&perf, Signal::Interrupt) {
            let pids: Vec<String> = perf.iter().map(u32::to_string).collect();
            log(&format!(
                "Sent SIGINT to perf record (PID: {})",
                pids.join(" ")
            ));
        }
    }
}

impl Drop for ProfiledPool {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            log("Cleaning up...");
            let perf = find_children(self.child.id(), "perf record");
            send_signal(&perf, Signal::Interrupt);
            send_signal(&[self.child.id()], Signal::Interrupt);
            let _ = self.child.wait();
        }
    }
}

/// Profile P2Poolv2 under JMeter load with `cargo flamegraph` (which needs
/// perf) and write `flamegraph_loadtest.svg` at the repository root.
pub async fn flamegraph() -> ToolResult<()> {
    const OUTPUT: &str = "flamegraph_loadtest.svg";
    let root = repo::root();
    let plan = repo::jmeter_dir().join("stratum.jmx");
    let config = root.join("config-load-test.toml");
    let temporary = std::env::temp_dir();
    let build_log = temporary.join("flamegraph-build.log");

    start_mock_bitcoind().await?;

    log("Building P2Poolv2 with debug symbols and starting under perf...");
    let output = std::fs::File::create(&build_log)
        .map_err(|error| ToolError::io("create", &build_log, error))?;
    let errors = output
        .try_clone()
        .map_err(|error| ToolError::io("open", &build_log, error))?;
    let child = Command::new("cargo")
        .current_dir(&root)
        .args(["flamegraph", "-p", "p2poolv2_node", "-o", OUTPUT, "--"])
        .arg(format!("--config={}", config.display()))
        .env("CARGO_PROFILE_RELEASE_DEBUG", "true")
        .env("CARGO_PROFILE_RELEASE_STRIP", "false")
        .env("RUSTFLAGS", "-C force-frame-pointers=yes")
        .stdin(Stdio::null())
        .stdout(output)
        .stderr(errors)
        .spawn()
        .map_err(|error| ToolError::message(format!("cannot run cargo flamegraph: {error}")))?;
    let mut profiled = ProfiledPool { child };

    log("Waiting for P2Poolv2 to start...");
    let mut waited = 0;
    while waited < 180 && !port_in_use(STRATUM_PORT) {
        tokio::time::sleep(Duration::from_secs(1)).await;
        waited += 1;
    }
    if !port_in_use(STRATUM_PORT) {
        log("ERROR: P2Poolv2 did not start within 180s");
        println!("{}", tail(&build_log, 20));
        return Err(ToolError::message("P2Poolv2 did not start"));
    }
    log(&format!(
        "P2Poolv2 stratum listening on port {STRATUM_PORT} (after {waited}s)"
    ));

    log("Running jmeter load test...");
    run_jmeter_filtered(&[
        "-n".to_string(),
        "-t".to_string(),
        plan.display().to_string(),
        "-l".to_string(),
        temporary
            .join("flamegraph-loadtest.jtl")
            .display()
            .to_string(),
        "-j".to_string(),
        temporary
            .join("flamegraph-jmeter.log")
            .display()
            .to_string(),
    ])?;
    log("jmeter complete");

    log("Stopping P2Poolv2 and generating flamegraph...");
    profiled.interrupt_recording();
    log("Waiting for flamegraph generation (symbol resolution takes time)...");
    let _ = profiled.child.wait();

    let graph = root.join(OUTPUT);
    if graph.is_file() {
        log(&format!("Flamegraph saved to {}", graph.display()));
        log(&format!("Open with: xdg-open {}", graph.display()));
        Ok(())
    } else {
        log("ERROR: Flamegraph was not generated");
        println!("{}", tail(&build_log, 20));
        Err(ToolError::message("the flamegraph was not generated"))
    }
}

/// Options of a remote load run.
#[derive(Debug, Clone)]
pub struct RemoteOptions {
    pub host: String,
    pub port: u16,
    pub threads: u32,
    pub ramp: u32,
    pub duration: u32,
    pub delay: u32,
}

/// Drive load at a stratum server already running on another host.
pub fn remote(options: &RemoteOptions) -> ToolResult<()> {
    if !on_path("jmeter") {
        return Err(ToolError::message(
            "ERROR: jmeter is not installed or not in PATH",
        ));
    }
    let jmeter_dir = repo::jmeter_dir();
    let plan = jmeter_dir.join("stratum.jmx");
    if !plan.is_file() {
        return Err(ToolError::message(format!(
            "ERROR: JMX file not found: {}",
            plan.display()
        )));
    }
    let results = jmeter_dir.join("results");
    std::fs::create_dir_all(&results).map_err(|error| ToolError::io("create", &results, error))?;
    let run_stamp = timestamp();
    let samples = results.join(format!("remote-{}-{run_stamp}.jtl", options.host));
    let jmeter_log = results.join(format!("remote-{}-{run_stamp}-jmeter.log", options.host));

    log(&format!("Target: {}:{}", options.host, options.port));
    log(&format!(
        "Threads: {}  Ramp: {}s  Duration: {}s  Submit delay: {}ms",
        options.threads, options.ramp, options.duration, options.delay
    ));
    log(&format!("Results: {}", samples.display()));
    log("Running jmeter load test...");
    run_jmeter_filtered(&[
        "-n".to_string(),
        "-t".to_string(),
        plan.display().to_string(),
        "-l".to_string(),
        samples.display().to_string(),
        "-j".to_string(),
        jmeter_log.display().to_string(),
        format!("-Jstratum.host={}", options.host),
        format!("-Jstratum.port={}", options.port),
        format!("-JThreadGroup.num_threads={}", options.threads),
        format!("-JThreadGroup.ramp_time={}", options.ramp),
        format!("-JThreadGroup.duration={}", options.duration),
        format!("-JConstantTimer.delay={}", options.delay),
    ])?;
    log("jmeter complete");
    log(&format!("Results: {}", samples.display()));
    log(&format!("Log: {}", jmeter_log.display()));
    Ok(())
}

/// Open the JMeter GUI scaled for high-DPI displays, with TCP protocol
/// logging at DEBUG. Returns JMeter's exit status.
pub fn gui() -> ToolResult<bool> {
    Command::new("jmeter")
        .arg("-Dsun.java2d.uiScale=2.0")
        .arg("-Jlog_level.jmeter.protocol.tcp=DEBUG")
        .status()
        .map(|status| status.success())
        .map_err(|error| ToolError::message(format!("cannot run jmeter: {error}")))
}

#[cfg(test)]
mod tests;
