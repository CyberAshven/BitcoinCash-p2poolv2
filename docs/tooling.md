# Developer tooling

All code in this fork is Rust (see `AGENTS.md`). The sim harness, the mock
bitcoind, the log analysers and the load-test drivers used to be Python,
JavaScript and shell scripts under `load-tests/` and `debug_tools/`. They are
now binaries of the `p2poolv2_tools` crate, run through cargo aliases defined
in `.cargo/config.toml`:

| Alias | Binary | What it does |
|---|---|---|
| `cargo sim <command>` | `sim_harness` | Launch, observe, measure, break and stop a swarm of no-PoW sim nodes, or run the nightly test |
| `cargo mock-bitcoind` | `mock_bitcoind` | Mock node JSON-RPC server for stratum load tests, answering as BCHN by default |
| `cargo share-lifecycle <log>...` | `share_lifecycle` | Lifecycle of each share in one or two node logs |
| `cargo share-latency <log or glob>...` | `share_latency` | Share propagation and confirmation latencies across node logs |
| `cargo jmeter-bench <command>` | `jmeter_bench` | JMeter stratum load test drivers: A/B benchmark, flamegraph, remote load, GUI, results summary |
| `cargo tcp-monitor` | `tcp_monitor` | TCP connection monitor for a pool's stratum port |
| `cargo pool-diagnose` | `pool_diagnose` | System and process diagnostics for a pool under load |
| `cargo netsim <command>` | `netsim` (crate `p2poolv2_netsim`) | Discrete-event simulation of share propagation at scale; see `docs/simulation/network-sim.md` |
| `cargo donate-qr` | `donate_qr` | Writes the landing page's donation QR codes, `website/donate-*.svg`; see `website/README.md` |
| `cargo safa <command>` | `safa` (crate `p2poolv2_safa`) | The SAFA AuxPoW covenant: its parameters, a miner's leaf, verified claims and an unsigned chipnet genesis; see `p2poolv2_safa/README.md` |
| `cargo daa-sim <command>` | `daa_sim` (crate `p2poolv2_daa`) | Share difficulty simulator: upstream's difficulty analysis ported from Python, on the share chain's own ASERT code; see [difficulty_adjustment/bch-daa-analysis.md](difficulty_adjustment/bch-daa-analysis.md) |

The aliases build with `--release`, except `donate-qr`, a one-shot
generator, and `safa`, a quick command line. Every tool takes `--help`. Arguments go
after the alias, for example `cargo sim run-swarm 20` or
`cargo share-latency --include-uncles 'logs/node-*.log'`.

`verify_chain` (in `p2poolv2_node`, feature `debug-tools`) is unchanged:
`just verify_chain /path/to/store.db`.

## The SAFA covenant tool: `p2poolv2_safa`

The SAFA AuxPoW covenant tool was the last JavaScript in the repository
(`tools/safa-auxpow`, Node and Libauth). It is now the Rust crate
`p2poolv2_safa` (2026-10-29). It compiles `safa.cash` and runs the
covenant in the Bitcoin Cash 2026 VM of
[libauth-rs](https://github.com/mainnet-pat/libauth-rs), a Rust port of
`@bitauth/libauth` 3.1.0-next.8, the version the Node tool pinned. It is a
git dependency pinned to commit `be3ed16396c9eb9c0e57343474d36079a37c8dae`.
Before the Node tool was removed, both printed the same bytes for every
command over the fixtures (the port's commit message lists the runs).
CI runs it as the `BCH Covenant Tests` job in `.github/workflows/rust.yml`.

libauth-rs is MIT licensed: its `Cargo.toml` declares `license = "MIT"`, as
its author confirmed, so `cargo deny` reads the licence from the crate itself.

## Shell that remains

Only glue is left in shell, and none of it is a tool:

- `run:` steps in `.github/workflows/*.yml`, and
  `.github/workflows/docker/{determine-tags,create-manifests,cleanup-temp-tags}.sh`,
  which the Docker workflow runs;
- `docker/bchn/bootstrap.sh`, `docker/bchn/healthcheck.sh` and
  `docker/bchn/bcli`, the BCHN image's regtest bootstrap, health check and
  CLI wrapper, and `docker/regtest/setup-miner.sh`, which gives the regtest
  CPU miner its identity;
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

### Bitcoin Cash and BCHN

The swarm mines Bitcoin Cash against a Bitcoin Cash Node (BCHN) regtest
node. Each node config sets `chain = "bitcoin-cash"` and
`bitcoin_cash_network = "regtest"`, pays a `bchreg:` CashAddr and owns its
shares with an `rp2pool:` share address of its own:

- `MINER_ADDRESS` defaults to `bchreg:qp63uahgrxged4z5jswyt5dn5v3lzsem6c6mz8vuwd`,
  the key with secret 1, and `run-swarm` checks it with the node's own
  CashAddr parser before it launches anything. With `DISTINCT_ADDR=1` (the
  default) each node takes its own address from the `p2pool` wallet with
  `getnewaddress`, and BCHN answers with a CashAddr.
- Node `i` owns its shares with the `rp2pool:` address of the key with
  secret `i + 2` (`p2poolv2_tools/src/sim/addresses.rs`). These keys are
  throwaway and regtest only.
- `p2poolv2_sim` reads both addresses with the node's own parsers for the
  configured chain, so a sim node and a production node accept the same
  addresses.
- Bitcoin Cash keeps its 100-share coinbase maturity in the sim build (a
  share-chain rule; only Bitcoin's is shortened to 20), so `late-miner`
  waits for a 200-share tip before the late node starts mining.

`nightly` starts BCHN (`bitcoind` from `BITCOIND_BIN` or `PATH`) when no
regtest node answers at `RPC_URL`. It adds `-allowunconnectedmining`, which
a lone node needs: BCHN's `getblocktemplate` refuses work with "Bitcoin is
not connected!" while it has no peers, on regtest too, where Bitcoin Core
skips that check. Wallet calls go to `RPC_URL/wallet/p2pool`, because BCHN
also creates and loads a default wallet. `run-swarm` and `nightly` refuse a
node whose `getnetworkinfo` user agent is not BCHN's. The sim-nightly
workflow installs BCHN from its release tarball, checked against the
version and per-architecture SHA-256 pinned in `docker/bchn/Dockerfile`.

## Bitcoin Cash and the remaining Bitcoin assumptions

The ports carry no Bitcoin constant they do not need. The sim swarm and
its nightly run Bitcoin Cash against BCHN (above). The nightly's wallet
fallback fee is 1 sat/byte (`0.00001` per kilobyte), not the script's
10 sat/vbyte, and new payout addresses come from `getnewaddress` with only a
label, so BCHN picks its own address type (CashAddr) instead of the
Bitcoin-only `bech32`. The log tools, `tcp-monitor` and `pool-diagnose`
are chain-agnostic, and `maturity-check` reads payees from either form of
`getblock` output.

`mock-bitcoind` answers as BCHN by default: it serves a BCHN regtest
template (`bitcoindrpc/test_data/bchn-regtest-template.json`; `--network
chipnet` serves the chipnet one) and answers `validateaddress` as BCHN does
for a CashAddr, with or without its prefix: `isvalid`, the prefixed
address, `scriptPubKey`, `isscript` and `istokenaware`. It reads the address
with the node's own payout parser, which takes the 20-byte P2PKH and P2SH
addresses this node pays, token-aware (`z...`, `r...`) or not, so a P2SH32
or legacy base58 address, which BCHN also accepts, answers `isvalid: false`
alone.

One Bitcoin assumption remains, because the test it drives still mines
Bitcoin:

- The JMeter plan and configs under `load-tests/jmeter-testing/` mine
  Bitcoin signet: `stratum.jmx` authorizes a `tb1` address,
  `config-load-test.toml` runs the pool on signet, and the CKPool configs
  are Bitcoin, which CKPool needs (it reads no CashAddr). `jmeter-bench`
  therefore runs the mock with `--node bitcoin-core`, which keeps the
  Bitcoin signet template and the script's `validateaddress` answer
  (every address valid, with `iswitness`).

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
- The scripts ran the swarm on Bitcoin against Bitcoin Core; the harness
  runs it on Bitcoin Cash against BCHN (see "Bitcoin Cash and BCHN" above).
- `nightly` talks to BCHN over JSON-RPC at `RPC_URL` and no longer needs
  `bitcoin-cli` (`BITCOIN_CLI_BIN` is gone). It still starts `bitcoind`
  itself when none answers, now BCHN's, found through `BITCOIND_BIN` or
  `PATH`; the script's Homebrew fallback found Bitcoin Core.
- `mock-bitcoind` answers as BCHN unless given `--node bitcoin-core`, which
  answers as the JavaScript server did.
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
