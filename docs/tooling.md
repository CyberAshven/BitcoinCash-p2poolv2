# Developer tooling

All code in this fork is Rust (see `AGENTS.md`), tools included. The former
Python, JavaScript and shell scripts under `load-tests/` and `debug_tools/`
are binaries of the `p2poolv2_tools` crate, run through cargo aliases in
`.cargo/config.toml`:

| Alias | Binary | What it does |
|---|---|---|
| `cargo sim <command>` | `sim_harness` | Launch, observe, measure, break and stop a swarm of no-PoW sim nodes, or run the nightly test |
| `cargo mock-bitcoind` | `mock_bitcoind` | Mock node JSON-RPC server for stratum load tests, answering as BCHN by default |
| `cargo share-lifecycle <log>...` | `share_lifecycle` | Lifecycle of each share in one or two node logs |
| `cargo share-latency <log or glob>...` | `share_latency` | Share propagation and confirmation latencies across node logs |
| `cargo jmeter-bench <command>` | `jmeter_bench` | JMeter stratum load tests: A/B benchmark, flamegraph, remote load, GUI, results summary |
| `cargo tcp-monitor` | `tcp_monitor` | TCP connection monitor for a pool's stratum port |
| `cargo pool-diagnose` | `pool_diagnose` | System and process diagnostics for a pool under load |
| `cargo netsim <command>` | `netsim` (crate `p2poolv2_netsim`) | Discrete-event simulation of share propagation at scale; see `docs/simulation/network-sim.md` |
| `cargo donate-qr` | `donate_qr` | Writes the landing page's donation QR codes, `website/donate-*.svg`; see `website/README.md` |
| `cargo safa <command>` | `safa` (crate `p2poolv2_safa`) | The SAFA AuxPoW covenant: parameters, a miner's leaf, verified claims and an unsigned chipnet genesis; see `p2poolv2_safa/README.md` |
| `cargo photon-payout-measure` | `photon-payout-measure` (crate `p2poolv2_photon_payout`) | PHOTON payout covenant VM counters, transaction costs and tree execution, offline; see `p2poolv2_photon_payout/README.md` |
| `cargo daa-sim <command>` | `daa_sim` (crate `p2poolv2_daa`) | Share difficulty simulator on the share chain's own ASERT code; see [difficulty_adjustment/bch-daa-analysis.md](difficulty_adjustment/bch-daa-analysis.md) |
| `cargo payout-sim` | `payout-sim` (crate `p2poolv2_payout_sim`) | Monte Carlo of payout variance and fees under the share chain designs of issue #79; see [p2poolv2_payout_sim/README.md](../p2poolv2_payout_sim/README.md) |

The aliases build with `--release`, except `donate-qr` and `safa`. Every tool
takes `--help`. Arguments go after the alias: `cargo sim run-swarm 20`,
`cargo share-latency --include-uncles 'logs/node-*.log'`.

`verify_chain` (in `p2poolv2_node`, feature `debug-tools`):
`just verify_chain /path/to/store.db`.

## The SAFA covenant tool: `p2poolv2_safa`

Compiles `safa.cash` and runs the covenant in the Bitcoin Cash 2026 VM of
[libauth-rs](https://github.com/mainnet-pat/libauth-rs), a Rust port of
`@bitauth/libauth` 3.1.0-next.8, pinned as a git dependency at commit
`be3ed16396c9eb9c0e57343474d36079a37c8dae`. It replaced the Node tool
`tools/safa-auxpow` after printing the same bytes for every command. CI runs
it as the `BCH Covenant Tests` job in `.github/workflows/rust.yml`.
`p2poolv2_safa/test_data/parity/cases.json` holds the tool to its own
recorded output (`p2poolv2_safa/README.md`, "Fixtures and the covenant's
bytes").

libauth-rs declares `license = "MIT"`, as its author confirms, but has no
licence file yet, so `deny.toml` clarifies that crate as
`MIT OR AGPL-3.0-or-later`; remove the clarification once upstream
publishes its licence file.

P2Pool's payout tree and payout covenant tests also run each tested input in
libauth-rs's standard BCH 2026 VM and require it to agree with the existing
test machine, decoding the real serialized transaction and source outputs,
CashTokens prefixes included. That checks scripts only;
admission and relay belong to BCHN.

## PHOTON payout feasibility: `p2poolv2_photon_payout`

A test-only prototype: it compiles annotated BCH Script, builds Merkle-sum
manifests and funded payout transactions, and verifies them in the same
libauth-rs VM in standard and non-standard modes. `cargo
photon-payout-measure` prints the resource and BCH cost tables;
`--spends-only` skips full-tree execution and host hashing timing. It uses
synthetic fixtures, contacts no network and is excluded from releases.

## Shell that remains

Only glue, none of it a tool:

- `run:` steps in `.github/workflows/*.yml`, and
  `.github/workflows/docker/{determine-tags,create-manifests,cleanup-temp-tags}.sh`;
- `docker/bchn/bootstrap.sh`, `docker/bchn/healthcheck.sh`, `docker/bchn/bcli`
  and `docker/regtest/setup-miner.sh` (the BCHN image's regtest bootstrap,
  health check and CLI wrapper, and the regtest miner's identity);
- the `justfile` recipes.

## The sim harness

`cargo sim` keeps the old scripts' interface: the same environment variables
and defaults (an empty variable counts as unset) and the same files in
`RUN_DIR` (default `/tmp/p2pool-sim`): `node-<i>.toml`, `node-<i>.log`,
`store-<i>.db`, positional `pids.txt`, `failure-results.txt` and
`maturity-check.txt`. `docs/simulation/RUNBOOK.md` walks through it.

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

The harness needs a Unix host (`kill`, `pgrep -f`). `tcp_monitor` and
`pool_diagnose` read `/proc` and are Linux only (`tcp_monitor` falls back to
`netstat` elsewhere).

### Bitcoin Cash and BCHN

The swarm mines Bitcoin Cash against a BCHN regtest node. Each node config
sets `chain = "bitcoin-cash"` and `bitcoin_cash_network = "regtest"`:

- `MINER_ADDRESS` defaults to `bchreg:qp63uahgrxged4z5jswyt5dn5v3lzsem6c6mz8vuwd`
  (the key with secret 1), checked with the node's CashAddr parser before
  launch. With `DISTINCT_ADDR=1` (the default) each node takes its own
  address from the `p2pool` wallet with `getnewaddress`.
- Node `i` owns its shares with the `rp2pool:` address of the key with secret
  `i + 2` (`p2poolv2_tools/src/sim/addresses.rs`), throwaway and regtest
  only. `p2poolv2_sim` reads both addresses with the node's own parsers.
- Bitcoin Cash keeps its 100-share coinbase maturity in the sim build (only
  Bitcoin's is shortened, to 20), so `late-miner` waits for a 200-share tip.
- `nightly` starts BCHN (`bitcoind` from `BITCOIND_BIN` or `PATH`) when none
  answers at `RPC_URL`, with `-allowunconnectedmining`, which a lone BCHN
  needs. Wallet calls go to `RPC_URL/wallet/p2pool`, since BCHN also loads a
  default wallet. `run-swarm` and `nightly` refuse a node whose
  `getnetworkinfo` user agent is not BCHN's. The sim-nightly workflow
  installs BCHN from its release tarball, checked against the version and
  per-architecture SHA-256 in `docker/bchn/Dockerfile`.
- The nightly's wallet fallback fee is 1 sat/byte (`0.00001` per kilobyte),
  and new payout addresses come from `getnewaddress` with only a label, so
  BCHN picks CashAddr, not Bitcoin's `bech32`. The log tools, `tcp-monitor`
  and `pool-diagnose` are chain-agnostic.

`mock-bitcoind` serves a BCHN regtest template
(`bitcoindrpc/test_data/bchn-regtest-template.json`; `--network chipnet`
serves the chipnet one) and answers `validateaddress` as BCHN does for a
CashAddr, with or without prefix: `isvalid`, the prefixed address,
`scriptPubKey`, `isscript` and `istokenaware`. It parses with the node's
payout parser (P2PKH, P2SH20, P2SH32, token-aware or not), so a legacy base58
address answers `isvalid: false`.

One Bitcoin assumption remains: the JMeter plan and configs under
`load-tests/jmeter-testing/` mine Bitcoin signet (`stratum.jmx` authorizes a
`tb1` address, `config-load-test.toml` runs the pool on signet, and the
CKPool configs are Bitcoin, since CKPool reads no CashAddr). `jmeter-bench`
therefore runs the mock with `--node bitcoin-core`, which keeps the signet
template and answers every address valid, with `iswitness`.

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

Upstream still has these scripts and keeps editing them. A sync that touches
one shows up as a modify/delete conflict: keep the deletion (`git rm <file>`)
and port the change into the Rust module the table names
(`git diff <last synced upstream commit> <upstream commit> -- <file>` shows
what to port). Keep the output lines the nightly reads (the
`failure-results.txt` lines, the metrics numbers) the same, and add a test
for the new behaviour. A new upstream script gets a new module and a row here.

## Where the ports differ

They behave as the scripts did, except:

- `plot-metrics` writes SVG (`RUN_DIR/metrics.svg`), not PNG, which would
  need a font file and an image stack.
- Seeded choices (latency spread, failure targets) use SplitMix64 instead of
  awk's `rand()`: a seed reproduces a run on every platform, but picks
  differently than the scripts did.
- Binaries are found from cargo's build messages, so `CARGO_TARGET_DIR`
  works.
- The swarm runs Bitcoin Cash against BCHN, not Bitcoin against Bitcoin Core.
- `nightly` talks to BCHN over JSON-RPC at `RPC_URL` and no longer needs
  `bitcoin-cli` (`BITCOIN_CLI_BIN` is gone); there is no Homebrew fallback.
- `mock-bitcoind` answers as BCHN unless given `--node bitcoin-core`, which
  answers as the JavaScript server did.
- `metrics` measures the share-rate span from full timestamps (no zero rate
  across midnight) and prints `->` and "proportional to" in ASCII.
- `maturity-check` reads a coinbase output's `addresses` list as well as its
  `address`, since node versions differ in which one `getblock` prints.
- `jmeter-bench benchmark` serves the mock in-process and builds only
  `p2poolv2` (`cargo build -p p2poolv2_node --release`).
- `tcp-monitor` reads `/proc/net/tcp` and matches the pool port exactly (the
  script's `ss` count was always zero), with ASCII symbols.
- `pool-diagnose` counts threads, file descriptors and TCP connections from
  `/proc`, and looks for the `p2poolv2` process (the script looked for
  `hydrapool`); `--process` names another.
- `share-lifecycle` lays two logs out at `--width`, `COLUMNS` or the
  terminal's width, else 160.
