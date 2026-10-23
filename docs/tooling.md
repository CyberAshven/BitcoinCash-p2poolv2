# Developer tooling

All code in this fork is Rust (see `AGENTS.md`). The sim harness, the mock
bitcoind, the log analysers and the load-test drivers used to be Python,
JavaScript and shell scripts under `load-tests/` and `debug_tools/`. They are
now binaries of the `p2poolv2_tools` crate, run through cargo aliases defined
in `.cargo/config.toml`:

| Alias | Binary | What it does |
|---|---|---|
| `cargo sim <command>` | `sim_harness` | Launch, observe, measure, break and stop a swarm of no-PoW sim nodes, or run the nightly test |
| `cargo mock-bitcoind` | `mock_bitcoind` | Mock bitcoind JSON-RPC server for stratum load tests |
| `cargo share-lifecycle <log>...` | `share_lifecycle` | Lifecycle of each share in one or two node logs |
| `cargo share-latency <log or glob>...` | `share_latency` | Share propagation and confirmation latencies across node logs |
| `cargo jmeter-bench <command>` | `jmeter_bench` | JMeter stratum load test drivers: A/B benchmark, flamegraph, remote load, GUI, results summary |
| `cargo tcp-monitor` | `tcp_monitor` | TCP connection monitor for a pool's stratum port |
| `cargo pool-diagnose` | `pool_diagnose` | System and process diagnostics for a pool under load |

The aliases build with `--release`. Every tool takes `--help`. Arguments go
after the alias, for example `cargo sim run-swarm 20` or
`cargo share-latency --include-uncles 'logs/node-*.log'`.

`verify_chain` (in `p2poolv2_node`, feature `debug-tools`) is unchanged:
`just verify_chain /path/to/store.db`.

## The one exception: `tools/safa-auxpow`

`tools/safa-auxpow` stays JavaScript (Node and Libauth). It is the test
harness for the SAFA AuxPoW covenant: it compiles `safa.cash` with pinned
Libauth and runs the covenant in the Bitcoin Cash 2026 VM, which only Libauth
provides. There is no Rust Bitcoin Cash VM in this repository yet, so it
cannot be ported. It is temporary: when Rust SAFA code and a Rust BCH VM
land, port it and remove this exception. CI runs it as the `BCH Covenant
Tests` job in `.github/workflows/rust.yml`, and CodeQL scans it as the
repository's only JavaScript.

## Shell that remains

Only glue is left in shell, and none of it is a tool:

- `run:` steps in `.github/workflows/*.yml`, and
  `.github/workflows/docker/{determine-tags,create-manifests,cleanup-temp-tags}.sh`,
  which the Docker workflow runs;
- `docker/bitcoind/bootstrap.sh` and `docker/bitcoind/bcli`, the bitcoind
  image's entrypoint and CLI wrapper;
- the `justfile` recipes.

## The sim harness

`cargo sim` keeps the scripts' interface: the same environment variables
with the same defaults (an empty variable counts as unset, as
`${VAR:-default}` did) and the same files in `RUN_DIR` (default
`/tmp/p2pool-sim`): `node-<i>.toml`, `node-<i>.log`, `store-<i>.db`,
positional `pids.txt`, `failure-results.txt` and `maturity-check.txt`.
`docs/simulation/RUNBOOK.md` walks through it.

```sh
cargo sim run-swarm 20              # build p2poolv2_sim (release, sim) and launch 20 nodes
cargo sim observe                   # live snapshot: tips (logs), peers (API), alive
cargo sim metrics                   # authoritative log-based summary
cargo sim plot-metrics              # RUN_DIR/metrics.svg
cargo sim failures restart-fast     # inject a failure scenario
LATE_MINER=1 cargo sim maturity-check
cargo sim stop-swarm
cargo sim nightly late-miner        # the whole nightly test; CI runs this
```

The harness needs a Unix host: it signals nodes with `kill` and finds them
with `pgrep -f`, as the scripts did. `tcp_monitor` and `pool_diagnose` read
`/proc` and are Linux only (`tcp_monitor` falls back to `netstat` elsewhere).

## Mapping from the old scripts

| Old file | Replacement |
|---|---|
| `load-tests/sim/run-swarm.sh` | `cargo sim run-swarm` (`p2poolv2_tools/src/sim/swarm.rs`) |
| `load-tests/sim/stop-swarm.sh` | `cargo sim stop-swarm` (`sim/stop.rs`) |
| `load-tests/sim/observe.sh` | `cargo sim observe` (`sim/observe.rs`) |
| `load-tests/sim/metrics.sh` | `cargo sim metrics` (`sim/metrics.rs`) |
| `load-tests/sim/failures.sh` | `cargo sim failures` (`sim/failures.rs`) |
| `load-tests/sim/nightly.sh` | `cargo sim nightly` (`sim/nightly.rs`) |
| `load-tests/sim/maturity_check.py` | `cargo sim maturity-check` (`sim/maturity.rs`) |
| `load-tests/sim/plot-metrics.py`, `plot-metrics.sh` | `cargo sim plot-metrics` (`sim/plot.rs`) |
| `load-tests/jmeter-testing/mock-bitcoind/server.js` (and its `package.json`, `.nvmrc`, `README.md`) | `cargo mock-bitcoind` (`mock_bitcoind.rs`) |
| `load-tests/jmeter-testing/benchmark.sh` | `cargo jmeter-bench benchmark` (`jmeter.rs`) |
| `load-tests/jmeter-testing/flamegraph.sh` | `cargo jmeter-bench flamegraph` (`jmeter.rs`) |
| `load-tests/jmeter-testing/run_remote_load.sh` | `cargo jmeter-bench remote` (`jmeter.rs`) |
| `load-tests/jmeter-testing/run_jmeter.sh` | `cargo jmeter-bench gui` (`jmeter.rs`) |
| `load-tests/diagnosis/tcp-monitor.sh` | `cargo tcp-monitor` (`tcp_monitor.rs`) |
| `load-tests/diagnosis/p2pool_diagnose.sh` | `cargo pool-diagnose` (`diagnose.rs`) |
| `debug_tools/parse_share_lifecycle.py` | `cargo share-lifecycle` (`share_lifecycle.rs`) |
| `debug_tools/share_latency.py` | `cargo share-latency` (`share_latency.rs`) |
| `debug_tools/README.md` | this file |

The JMeter test plan and configs (`load-tests/jmeter-testing/stratum.jmx`,
`config-load-test.toml`, `ckpool-*.json`) stay where they were.

### Merging upstream

Upstream P2Poolv2 still has these scripts and keeps editing them, the sim
scripts most of all. A sync that touches one shows up as a modify/delete
conflict on the deleted file. Resolve it by keeping the deletion
(`git rm <file>`) and porting the upstream change into the Rust module the
table names; `git diff <last synced upstream commit> <upstream commit> -- <file>`
shows what to port. Keep the output lines the nightly reads (the
`failure-results.txt` lines, the metrics numbers) the same, and add a test
next to the module for the new behaviour. A new upstream script gets a new
module and a row here.

## Where the ports differ

They behave as the scripts did, with these deliberate exceptions:

- `plot-metrics` writes SVG (`RUN_DIR/metrics.svg`), not PNG. Rendering PNG
  in Rust needs a font file and an image stack; SVG leaves text to the
  viewer, opens in any browser, and adds no dependency.
- Seeded choices (per-node latency spread, failure targets) use SplitMix64
  instead of awk's `rand()`. A seed still reproduces a run, now on every
  platform, but picks different nodes and latencies than the scripts did.
- Built binaries are found from cargo's own build messages, so
  `CARGO_TARGET_DIR` works; the scripts assumed `target/<profile>`.
- `nightly` talks to bitcoind over JSON-RPC at `RPC_URL` and no longer needs
  `bitcoin-cli` (`BITCOIN_CLI_BIN` is gone). It still starts `bitcoind` itself
  when none answers.
- `metrics` measures the share-rate span from full timestamps, so a run
  across midnight no longer reports a zero rate. Its headings say `->` and
  "proportional to" where the script printed non-ASCII symbols.
- `maturity-check` reads a coinbase output's `addresses` list as well as its
  `address`, since node versions differ in which one `getblock` prints.
- `jmeter-bench benchmark` serves the mock bitcoind in-process and builds only
  the `p2poolv2` binary (`cargo build -p p2poolv2_node --release`) rather than
  the whole workspace.
- `tcp-monitor` reads `/proc/net/tcp` directly and matches the pool port
  exactly. The script counted `ss` lines starting with `tcp`, which `ss -tan`
  never prints, so its totals were always zero, and its sudden-drop warning
  was cleared before it could be shown. Its symbols are ASCII.
- `pool-diagnose` counts threads, file descriptors and the process's TCP
  connections from `/proc` (the script's `ps -T | wc -l` counted the header),
  and takes `--process` for a name other than `hydrapool`.
- `share-lifecycle` lays two logs out at the width in `--width`, `COLUMNS`
  or the terminal's, else 160.
