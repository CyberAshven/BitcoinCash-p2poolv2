# Sim load-test runbook (local, manual)

A step-by-step guide to running the no-PoW sim by hand and seeing the outcomes.
Companion to the simulation [overview](./overview.md).

Mental model: there is **no proof-of-work**. Each node runs a "miner" that just
*sleeps* between shares (interval ≈ difficulty·2³²/hashrate) and emits
structurally-valid synthetic shares. They propagate over real libp2p, build a
real share chain (ASERT, uncles, reorgs, PPLNS), and on a random "block-find"
a node submits a **real Bitcoin Cash regtest block** to BCHN carrying the
PPLNS payout.

The sim mines Bitcoin Cash: every node config sets `chain = "bitcoin-cash"`
and `bitcoin_cash_network = "regtest"`, and the node is Bitcoin Cash Node
(BCHN). Everything below assumes a Unix shell (the harness signals nodes with
`kill` and finds them with `pgrep`) and that you start from the repo root:

```sh
cd ~/code/p2poolv2
```

Tools used: `jq`, `curl`, and BCHN's `bitcoin-cli`.

---

## 0. One-time shell helpers

Paste these into your terminal; the rest of the runbook uses them.

```sh
# BCHN's bitcoin-cli against the isolated regtest node used for the sim
B() { bitcoin-cli -datadir=/tmp/p2pool-regtest -rpcport=19443 -rpcuser=p2pool -rpcpassword=p2pool "$@"; }
# the same, for calls to the p2pool wallet
W() { B -rpcwallet=p2pool "$@"; }

# raw JSON-RPC (same creds as the configs)
rpc() {
  curl -s --user p2pool:p2pool \
    --data-binary "{\"jsonrpc\":\"1.0\",\"id\":\"r\",\"method\":\"$1\",\"params\":${2:-[]}}" \
    -H 'content-type: text/plain;' http://127.0.0.1:19443/
}
```

---

## 1. Prerequisites: the BCHN regtest node

The sim talks to one local **BCHN regtest** node (RPC + ZMQ). Install BCHN
from its release tarball
(<https://github.com/bitcoin-cash-node/bitcoin-cash-node/releases>; the
version and SHA-256 sums CI uses are pinned in `docker/bchn/Dockerfile`), so
its `bitcoind` and `bitcoin-cli` are on your `PATH`. BCHN's daemon is called
`bitcoind` like Bitcoin Core's; a Bitcoin Core node will not do, and the
harness refuses one. Check it's up:

```sh
B getblockchaininfo | jq '{chain, blocks}'
B getnetworkinfo | jq -r .subversion   # /Bitcoin Cash Node:.../
```

Expect `"chain": "regtest"` and some block count (≥101). If it errors / isn't
running, start it and prime a wallet + coins:

```sh
mkdir -p /tmp/p2pool-regtest
bitcoind -regtest -datadir=/tmp/p2pool-regtest -daemon -rpcport=19443 -rpcuser=p2pool -rpcpassword=p2pool -zmqpubhashblock=tcp://127.0.0.1:28332 -fallbackfee=0.00001 -allowunconnectedmining
sleep 2
B createwallet p2pool 2>/dev/null || B loadwallet p2pool
ADDR=$(W getnewaddress)
B generatetoaddress 101 "$ADDR"     # spendable coinbase + a chain to build on
```

`-allowunconnectedmining` matters: BCHN's `getblocktemplate` refuses work
with "Bitcoin is not connected!" while the node has no peers, on regtest
too, and a lone regtest node has none. BCHN also creates and loads a default
wallet, so wallet calls name the `p2pool` wallet (`W`, or the
`/wallet/p2pool` RPC path the harness uses). A node restarted after its tip
is more than a day old counts itself in initial block download and refuses
templates until a new block arrives: mine one (`B generatetoaddress 1
"$ADDR"`), or start it with `-maxtipage=2000000000` as the Docker regtest
config does.

Sanity:

```sh
B getblockcount          # height
W getnewaddress          # wallet works -> prints a bchreg:q... address
```

> The sim never needs you to mine manually again — block-finds do it. The only
> reason for the wallet here is to hand each node a distinct payout address.

---

## 2. Build the binary (with the `sim` feature)

Two builds matter:

```sh
# debug -- fine for a single node
cargo build -p p2poolv2_sim --features sim

# release -- REQUIRED for multi-node swarms (see gotcha #1)
cargo build -p p2poolv2_sim --features sim --release
```


---

## 3. Test A — single node grows the share chain (Phase 1)

The node needs a Bitcoin Cash regtest config with a `[sim]` block (see the
overview, "Running a single sim node"). The quickest one is the config
`run-swarm` writes for a one-node swarm; stop that swarm and copy it:

```sh
cargo sim run-swarm 1 && cargo sim stop-swarm
cp /tmp/p2pool-sim/node-0.toml /tmp/sim-A.toml
```

Run with a throwaway store so existing data is untouched, logging to a file.
Run it as a **single line** (don't split it with a `\` — pasting the
continuation often drops you into a `dquote>` prompt; if that happens, Ctrl-C):

```sh
P2POOL_STORE__PATH=/tmp/store-sim-A.db ./target/debug/p2poolv2_sim --config /tmp/sim-A.toml > /tmp/sim-A.log 2>&1 &
```

The shell prints the background pid itself (e.g. `[1] 12345`).

Watch the chain grow (each line = one confirmed synthetic share):

```sh
sleep 20
grep "Promoted block" /tmp/sim-A.log | tail -5
```

Expect heights climbing (`... to confirmed height Some(1)`, `Some(2)`, …) every
few seconds. Cross-check via the API (port 7600 in the copied config):

```sh
curl -s http://127.0.0.1:7600/chain_info | jq '{tip: .chain_tip_height}'
```

**What you're seeing:** the closed-loop emitter pacing at ≈ difficulty·2³²/H,
shares passing the *real* validation/organise path (no errors = ASERT, merkle,
commitment all check out). No p2p, no blocks yet.

Stop it:

```sh
kill %1    # or: pkill -f 'target/debug/p2poolv2_sim'
```

---

## 4. Test B — 20-node swarm: propagation + convergence (Phase 2)

The harness is `cargo sim`, the `sim_harness` binary in `p2poolv2_tools` (see
`docs/tooling.md`). Launch 20 nodes (release):

```sh
cargo sim run-swarm 20
```

It builds release, generates one config per node under `/tmp/p2pool-sim/`
(distinct ports, distinct seed, distinct payout address, fresh store, a
`dial_peers` topology), launches them, and writes PIDs. Give them ~30 s, then:

```sh
cargo sim observe
```

`observe` is a *live* snapshot — tip height/hash and convergence come from the
logs (load-immune), peer count from the API. Read the table:
- **tip_h** all equal (or split across the latest 1–2 heights) → converging. A
  snapshot often straddles two heights because the tip is advancing; that's
  normal, not a partition.
- **distinct tips: 1** → one shared chain. **alive: 20/20**, **peers > 0**.

For the authoritative numbers — convergence, share rate, ASERT difficulty,
emission∝hashrate, uncles, block-finds, rejections — run the **log-based**
summary. It reads only the logs, so it's load-immune and is the source of truth
(safe to run during or after the run):

```sh
cargo sim metrics
```

> **Logs vs API.** Measure *outcomes* from the logs (`metrics`, `plot-metrics`):
> they record what each node actually committed and don't perturb the run. Use the
> HTTP API (`observe`, `curl :7600/...`) only for *live* watching and current
> peer state — it's advisory and can time out while a node is busy, so never treat
> an API non-response as "diverged."

Leave it running for the next test, or stop with:

```sh
cargo sim stop-swarm
```

---

## 5. Test C — latency makes uncles appear (Phase 2 headline)

Over loopback, propagation beats the block interval, so with no delay the chain
is near-linear (≈0% uncles). Inject per-node announcement latency and the uncle
rate rises. Run three swarms and compare (stop between each):

```sh
for L in 0 200 750; do
  cargo sim stop-swarm >/dev/null 2>&1
  LATENCY_MS=$L cargo sim run-swarm 20 >/dev/null 2>&1
  sleep 45
  echo "=== LATENCY_MS=$L ==="
  cargo sim observe | grep "node 0:"
done
cargo sim stop-swarm
```

`observe`'s `node 0:` line reports `uncle-rate`. At the regulated 10s interval
uncle rate ≈ latency/10s (e.g. 750 ms → ~7%), but a 45s read is only ~4 blocks —
too noisy to trust. For a real number in ~5 min add `IDEAL_BLOCK_TIME=1` (10× more
blocks, latency auto-scales), e.g.
`IDEAL_BLOCK_TIME=1 LATENCY_MS=1500 SHARES_PER_BLOCK=20 cargo sim run-swarm 20`
→ ~12% (≈50 min-equivalent). Knob in §11.

Under the hood it greps the `sim-uncle:` log lines:

```sh
grep "sim-uncle:" /tmp/p2pool-sim/node-0.log | tail -3
```

---

## 6. Test D — block-finds submit real BCHN regtest blocks (Phase 3)

Each emitted share is a block with probability `1/SHARES_PER_BLOCK`. Lower `SHARES_PER_BLOCK` to make
this frequent and watch the **BCHN** block height climb (the sim nodes are mining
real Bitcoin Cash regtest blocks).

```sh
cargo sim stop-swarm >/dev/null 2>&1
echo "BCHN height before: $(B getblockcount)"
SHARES_PER_BLOCK=20 LATENCY_MS=100 cargo sim run-swarm 20
sleep 40
echo "BCHN height after:  $(B getblockcount)"
grep -h "sim block-find" /tmp/p2pool-sim/node-*.log | wc -l        # attempts
grep -h "Block submitted successfully" /tmp/p2pool-sim/node-*.log | wc -l   # accepted
```

Expect the BCHN height to jump several blocks and "submitted successfully"
lines. The full real path runs for free: block submit → ZMQ `hashblock` → GBT
refresh → the share chain continues on the new BCHN tip.

---

## 7. Test E — the PPLNS payout is a real multi-way split

Decode the coinbase of a recently sim-mined block (keep the swarm from Test D
running so the window has accumulated shares from several distinct miners):

```sh
TIP=$(B getblockcount)
for ht in $((TIP-2)) $((TIP-1)) $TIP; do
  h=$(B getblockhash $ht)
  echo "=== block $ht coinbase ==="
  B getblock "$h" 2 | jq -r '.tx[0].vout | "outputs: \(length)",
    (.[] | "  \(.value)  \((.scriptPubKey.addresses // [.scriptPubKey.address // .scriptPubKey.type]) | join(","))")'
done
```

Expect some blocks paying **several distinct `bchreg:q…` addresses** with
amounts that sum to the subsidy (e.g. `13.75 + 11.25`), plus `nulldata`
outputs: the share commitment (`OP_RETURN` and a 32-byte hash) last, and
sometimes a zero-byte padding output before it. A Bitcoin Cash coinbase
has no witness commitment. The split is the PPLNS distribution,
proportional to each miner's share contribution in the window.

> Breadth: the number of payees = distinct miners with weight in the PPLNS
> window, up to `BCH_MAX_COINBASE_PAYOUTS` (11) payout outputs. Past 11
> payees the largest are paid their exact share and the rest goes to one
> payee drawn by weight (`docs/architecture/payout-distribution.md`,
> "Bitcoin Cash coinbase budget"); both are share-chain rules. The window
> starts below the shares not yet mature for the paying share
> (`BCH_COINBASE_MATURITY`, 100 shares, which the sim build keeps) and is
> capped at `PPLNS_WINDOW_SHARES` (80 in the sim build); regtest headers
> always hit that cap. The first 100 shares of the chain are always mature.
> With 20 nodes a block pays at most 11 payout outputs once the chain is
> deeper than maturity plus window; with 10 nodes, up to all 10 miners
> (a small miner may have no share in an 80-share window).

Stop everything:

```sh
cargo sim stop-swarm
```

---

## 8. Test F — a node fails and catches up (Phase 4)

Failures are injected into an already-running swarm by `failures`, which
`SIGKILL`s nodes, restarts them against the *same* config and store, and then
polls each one's log until its confirmed tip reaches where the rest of the
swarm has got to.

Start a swarm and, in a second terminal, kill a node and watch it come back:

```sh
cargo sim run-swarm 6
FAILURE_WARMUP=20 FAILURE_FAST_DOWN=10 cargo sim failures restart-fast
```

Expect the target to report `alive NO` in `observe` during the downtime,
then `alive yes` with a climbing tip, and a summary ending:

```
node 3: rejoined=yes catchup_secs=5 gap_heights=14 target_height=32
rejoined: 1/1
unrecovered nodes: 0
```

The interesting one is the long downtime, which is the default:

```sh
cargo sim failures restart-delayed     # 330s down
```

330s crosses the 300s `MAX_TIP_AGE_SECS` threshold, so the node rejoins *not*
current. Inv gossip is deliberately ignored in that state, so the catch-up has
to come from the header-sync path. In the restarted node's log, after the
`=== failure injection: restarting node <i> ===` marker, expect a fresh
`Local peer id:`, then the handshake driving a header sync rather than
inv-driven block fetches:

```
Received Handshake from peer ...: peer_height=187, ..., local_height=30
Local chain behind peer ... (30 < 187), sending getheaders
Sending GetHeaders to peer ...: GetShareHeaders([...15-hash locator...], 0000...)
Received 160 ShareHeaders
```

The `partition` scenario kills `DIAL_FANOUT` consecutive nodes at once, cutting
the `dial_peers` chain. Whether that is a real partition is an open question at
runtime — Kademlia dials peers that were never in `dial_peers` — so the summary
counts outbound connections crossing the cut and reports what actually happened:

```sh
cargo sim failures partition
# cross-side links at +15s: lower_to_upper=0 upper_to_lower=0   <- a real partition
```

The whole thing runs end to end, with the usual pass/fail verdict, via:

```sh
cargo sim nightly restart-delayed
```

**Gotchas specific to failure injection**

1. **Node 0 is never a target.** `metrics` derives the share rate and uncle
   rate from `node-0.log` alone, so killing it corrupts the run's own metrics.
2. **Restarts append to the node log** (`>>`, not `>`). Truncating would throw
   away the promotion history every log-based metric is computed from.
3. **A restarted node has a new PeerId.** The libp2p keypair is generated per
   process start and never persisted, so a rejoin looks like a new node to its
   peers. The Kademlia routing table is in-memory too, so it starts empty.
4. **Mid-run tip readings come from logs or the API, never the store.** RocksDB
   holds an exclusive lock while a node runs, which is why `nightly` only
   does its store-based convergence check after `stop-swarm`.
5. **`pids.txt` is positional** (line `i+1` is node `i`). `failures` rewrites
   the line on restart, which is what keeps `observe` and the nightly's
   alive check honest.

---

## 9. Test G — payout maturity (late miner)

Checks that a block-find pays a miner only once its shares are mature. The
swarm mines Bitcoin Cash, whose coinbase maturity is 100 shares
(`BCH_COINBASE_MATURITY`, a share-chain rule the sim build keeps; only
Bitcoin's is shortened to 20). Each node logs it at startup as
`Sim depths: coinbase maturity 100 shares, ...`. A time-compressed run
reaches it in a few minutes. The swarm is launched with `LATE_MINER`, the
node that starts with its emitter off:

```sh
IDEAL_BLOCK_TIME=1 SHARES_PER_BLOCK=20 LATE_MINER=1 \
  cargo sim run-swarm 10
LATE_MINER=1 cargo sim failures late-miner
```

Or end to end, with BCHN setup and the nightly checks:

```sh
IDEAL_BLOCK_TIME=1 SHARES_PER_BLOCK=20 LATE_MINER=1 \
  NODE_COUNT=10 cargo sim nightly late-miner
```

The scenario waits for the tip to reach twice the maturity (200 shares), so
the payout skip is at full size, restarts node 1 with its emitter on, and
then runs `cargo sim maturity-check` until it gives a verdict. The checker
reads only the logs, the node 0 API and BCHN. It maps each `sim block-find`
line (`... prev_share <hash>`) to the paying share's height and decodes the
regtest coinbase. A report looks like this (heights and hashes vary):

```
late miner: node 1 (bchreg:q...), coinbase maturity 100
first late share: confirmed 231, confirmed or uncle 231
bitcoin block        paying_h  p-first  mature  late_paid
3f1c0b9e2a7d4c11..        318       87      no         no
9a02d7e4b1c35f80..        337      106     yes        yes
maturity check: PASS (late miner first paid at paying height 337)
```

It fails if the late miner is paid while `p - first` is below the maturity, or is
not paid by the first block-find past it. The full report is in
`$RUN_DIR/maturity-check.txt`. Exit codes: 0 pass, 1 fail, 2 no qualifying
block-find yet, 3 setup error (e.g. the late node shares its payout address
with another node; keep `DISTINCT_ADDR=1` and the `p2pool` wallet loaded).

Spend maturity is not covered: sim shares carry no share transactions. Unit
tests pin it.

---

## 9a. Test H — a fresh node joins pruned peers

Checks that a node syncing from genesis gets everything it needs from peers
that have pruned block bodies. The sim build prunes below 220 shares under
the tip (prune depth 200 plus interval 20), every 30s, so the run is
time-compressed to get there in minutes:

```sh
IDEAL_BLOCK_TIME=1 SHARES_PER_BLOCK=20 NODE_COUNT=5 \
  cargo sim nightly pruned-join
```

The scenario waits for the swarm tip to pass the prune depth plus three
intervals and for every node to log `Pruned block bodies at heights ...`.
It then kills one node, deletes its store, moves its log to
`prejoin-node-<i>.log`, and restarts it. The fresh node must:

- catch up to the swarm tip;
- report the same confirmed hash as node 0 at a height it holds header-only
  (below its own prune height) and at one near the tip;
- get no `NotFound` reply to a body request, so it never asked for a body
  its peers deleted;
- log no `Failed to store and organise block`.

The summary prints `pruned-join verdict: PASS` or `FAIL (<reasons>)`, and
`verify_chain` then checks every store, the fresh one included.

---

## 9b. Test I — the reorg depth limit refuses a deep fork

Checks that a node refuses to reorg onto a heavier branch whose fork point is
below its prune target, instead of stalling on bodies every node has deleted.
`FORK_GROUP_FROM` launches the swarm in two groups that never connect: nodes
at that index and above dial only each other. Hashrate is zipf-weighted by
index, so the lower group is heavier.

```sh
IDEAL_BLOCK_TIME=1 SHARES_PER_BLOCK=20 NODE_COUNT=6 FORK_GROUP_FROM=4 HASHRATE=1e13 \
  cargo sim nightly deep-fork-refused
IDEAL_BLOCK_TIME=1 SHARES_PER_BLOCK=20 NODE_COUNT=6 FORK_GROUP_FROM=4 HASHRATE=1e13 \
  cargo sim nightly shallow-fork-reorged
```

`HASHRATE=1e13` matters. Each group's ASERT regulates its own rate, but the
share target cannot get easier than the clamp (difficulty about 1,000). At
1s shares a group needs about 4.3e12 H/s to keep pace; with the default
1e12 per node the minority mines at a fraction of the rate and never reaches
the prune target.

`deep-fork-refused` waits for every node to prune, then restarts node
`FORK_GROUP_FROM` with node 0 added to its `dial_peers` and watches it for
`FORK_OBSERVE_SECS` (default 90). It passes when the node connects to node 0,
logs `Refusing candidate reorg to <hash>: branch forks more than
MAX_REORG_DEPTH below the confirmed tip`, keeps the same confirmed hash at a
height below its tip, and keeps confirming, while node 0 keeps advancing. The
convergence check is skipped. The sim reorg floor is `PAYOUT_DEPTH` (100
shares) below the confirmed tip, so the genesis fork is refused well before
every node has pruned; the test still waits for pruning so the fork point is
below the prune target too.

`shallow-fork-reorged` is the control: it reconnects at height 60, before any
node prunes. The node must log no refusal and end up with node 0's confirmed
hash, and the normal convergence check applies. Height 60 also keeps the fork
within the sim PPLNS window cache (200 entries on Bitcoin Cash: maturity
100, window 80, buffer 20), which bounds how deep a fork a node can pay out
for.

The summary prints `fork verdict: PASS` or `FAIL (<reasons>)`.

---

## 10. Where things live

| Thing              | Path                                                      |
|--------------------|-----------------------------------------------------------|
| Single-node config | `/tmp/sim-A.toml` (Test A; has `[sim]`)                   |
| Swarm run dir      | `/tmp/p2pool-sim/`                                        |
| Per-node config    | `/tmp/p2pool-sim/node-<i>.toml`                           |
| Per-node log       | `/tmp/p2pool-sim/node-<i>.log`                            |
| Per-node store     | `/tmp/p2pool-sim/store-<i>.db`                            |
| PIDs               | `/tmp/p2pool-sim/pids.txt` (line `i+1` is node `i`)        |
| Failure summary    | `/tmp/p2pool-sim/failure-results.txt`                     |
| Maturity report    | `/tmp/p2pool-sim/maturity-check.txt` (late-miner only)    |
| Pre-wipe log       | `/tmp/p2pool-sim/prejoin-node-<i>.log` (pruned-join only) |
| Node APIs          | `http://127.0.0.1:760<i>` (node i)                        |
| BCHN regtest node  | datadir `/tmp/p2pool-regtest`, RPC `:19443`, ZMQ `:28332` |

Useful log greps (per node or across all):

```sh
grep "Promoted block" /tmp/p2pool-sim/node-0.log | tail        # chain growth
grep "sim-uncle:"      /tmp/p2pool-sim/node-0.log | wc -l       # uncles
grep "sim block-find"  /tmp/p2pool-sim/node-*.log | wc -l       # block-finds
grep -iE "error|panic" /tmp/p2pool-sim/node-*.log | grep -v median   # problems
```

Useful API calls (node 0) — **live/advisory only** (can time out under load; for
results use the log greps above or `metrics`):

```sh
curl -s :7600/peers | jq length          # peer count (live state — API is right here)
curl -s :7600/chain_info | jq            # tip/candidate heights (prefer logs for convergence)
```

---

## 11. Env knobs for `run-swarm`

`run-swarm [N]` (default N=20). Override with env vars:

| Var                                  | Default         | Meaning                                                                                                            |
|--------------------------------------|-----------------|--------------------------------------------------------------------------------------------------------------------|
| `SHARES_PER_BLOCK`                              | 10000           | shares per block-find (lower = more blocks)                                                                        |
| `LATENCY_MS`                         | 0               | per-node outbound announce delay (raise for uncles)                                                                |
| `IDEAL_BLOCK_TIME`                   | 10              | share interval (s); lower = time-compressed, more blocks/min for faster data (auto-scales `LATENCY_MS`); floor ~1s |
| `HASHRATE`                           | 1.0e12          | modeled per-node hashrate (higher = faster shares)                                                                 |
| `LATE_MINER`                         | unset           | node index launched with its emitter off, for `failures late-miner` (Test G)                                       |
| `FORK_GROUP_FROM`                    | unset           | split the swarm: nodes at this index and above dial only each other, for the fork scenarios (Test I)               |
| `MINER_ADDRESS`                      | `bchreg:qp63…vuwd` | payout address of a node without its own; a `bchreg:` CashAddr (the key with secret 1)                       |
| `DISTINCT_ADDR`                      | 1               | each node gets its own payout address via the wallet                                                               |
| `DIAL_FANOUT`                        | 3               | how many earlier peers each node dials                                                                             |
| `PROFILE`                            | release         | `release` or `debug`                                                                                               |
| `RUN_DIR`                            | /tmp/p2pool-sim | where configs/logs/stores go                                                                                       |
| `BASE_P2P`/`BASE_STRATUM`/`BASE_API` | 7000/7300/7600  | first ports                                                                                                        |

---

## 12. Gotchas

1. **Swarms must run release.** A debug build hits a `debug_assert_eq!` inside
   `libp2p-request-response` under connection churn and nodes abort (you'd see
   ~most of 20 die). `run-swarm` defaults to release; don't override
   `PROFILE=debug` for multi-node runs.
2. **Nodes run in the background.** `run-swarm` returns to your prompt while
   20 nodes keep running. Always end a session with `stop-swarm` (or
   `pkill -f release/p2poolv2`) or you'll have orphans on the next run (port
   clashes).
3. **High block rates drop a few shares.** At very fast chains you'll see
   `Share timestamp … not greater than median time past` rejections — a benign
   1-second-granularity artifact, not a failure.
4. **`getnewaddress` needs the `p2pool` wallet.** If `DISTINCT_ADDR=1` can't
   reach it (`RPC_URL/wallet/p2pool`) it silently falls back to one shared
   address (you'd see one payout address). Confirm the wallet with the sanity
   check in step 1.
