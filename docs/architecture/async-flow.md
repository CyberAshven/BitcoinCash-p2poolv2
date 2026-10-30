**The architecture files are LLM generated. The goal is make it easier
for new comers to the code base and other LLMs to quickly understand
the design and remain consistent with the design choices.**

# Async Flow and Concurrency Model

## The Node Actor Loop

`NodeActor::run` (`node/actor.rs`) owns the libp2p `Swarm` and is the only
task that polls it or calls it. It is a **non-blocking dispatcher**:

> Every `select!` arm body is a short synchronous handler. It either calls the
> `Swarm` directly (the loop owns it) or hands work to a worker through a
> bounded channel with `try_send`. No arm body awaits.

The arms:

| Source | Handler | What it does |
|---|---|---|
| `swarm.select_next_some()` | `Node::handle_swarm_event` | Connection lifecycle, identify/ping/kademlia, request-response dispatch |
| `swarm_rx` (outbound ops) | `on_swarm_send` | Executes `SwarmSend` from workers: `send_request`, `send_response`, block broadcast, disconnect |
| `command_rx` (control plane) | `on_command` | `GetPeers`, `SendToPeer`, `BlockIp`, `Shutdown`, ... answered on oneshot replies |
| `workers.join_next_with_id()` | `on_worker_exit` | Supervision: a fatal error or panic in any worker stops the node |
| `share_transaction_relay` (Bitcoin Cash) | `on_relay_transaction` | Sends a share transaction the pool accepted to every connected peer but its source, as `Transaction` requests |
| reconnect / sync-retry / kademlia intervals | inline / `on_sync_retry_tick` | Periodic swarm maintenance. The sync retry (60s, only while not current) is skipped while the confirmed tip keeps moving: during body sync each retry reply re-runs the missing-block scan on the ResponseWorker and stalls block responses, and every case the retry rescues (rejected batch, dead peer, dropped body, drained fetcher) stops the confirmed tip anyway |

The invariant is enforced by types: `handle_swarm_event` and the
request-response handlers are plain `fn`s, so an `.await` on a channel send
inside them does not compile. Code on the loop sends on the swarm through the
`RequestSender` trait (`send_request`, `disconnect_peer`) and builds messages
with the synchronous `build_handshake_message` / `build_getheaders_message`.
A non-async `#[test]` calling `dispatch_response` guards this.

**Why it matters.** The loop is the sole consumer of `swarm_tx`. If an arm
awaited a send on `swarm_tx` while the channel was full, nothing would drain
it and the node would stop answering P2P traffic and commands entirely -- the
2026-09-19 deadlock. With synchronous arms the loop drains `swarm_tx` every
iteration, so producers sending on it see bounded backpressure that always
completes.

**What may still run inline.** Driver-owned state (`connection_tracker`,
`peer_reconnector`, peer block knowledge) is updated in place; it has a single
owner and needs no lock. Three low-frequency handlers read the store on the
loop: the handshake on connection, the sync-retry locator, and
`Command::GetPplnsShares`. They are infrequent, so the read is accepted; if
profiling ever shows loop stalls, move them to `spawn_blocking` or a worker.

**Shutdown.** Every exit breaks the loop with a `StopReason` and runs one
teardown, `shut_down`: it stops all workers (`JoinSet::shutdown`) and only
then answers `Command::Shutdown`, or, for a fatal stop (worker failure, closed
command channel), signals `stopping_tx`, which the node binary treats as an
error exit. Workers are cancelled at their current await point; durable
effects go through the StoreWriter's atomic batches and in-memory state is
rebuilt on restart, so there is nothing to clean up.

## Workers and Channels

Heavy work runs on dedicated tasks, all spawned into the actor's `JoinSet`
except the per-peer services and the StoreWriter thread:

- **ResponseWorker** -- runs `handle_response` for inbound responses
  (`ShareHeaders` can mean up to `MAX_HEADERS_IN_RESPONSE` (900)
  `organise_header` calls). One task, FIFO, so each peer's header batches
  stay in order.
- **Per-peer service tasks** -- Tower stack (rate limit, inactivity) running
  inbound request handlers; one per connected peer.
- **BlockReceiver** -- persists received share blocks, resolves dependencies,
  queues validation.
- **BlockFetcher** -- distributes `GetData` requests across peers with timeouts.
- **ValidationWorker** -- context-free validation, one spawned task per block,
  concurrency capped by a semaphore sized to the CPU count.
- **OrganiseWorker** -- serial candidate-to-confirmed promotion.
- **FoundBlockVerifier** -- on Bitcoin Cash, checks candidate block hashes
  against BCHN's chain independently of share confirmation. RPC failures
  defer accounting and do not stop share processing. Positive acceptance is
  persisted through StoreWriter before resetting runtime block effort.
- **EmissionWorker** -- turns stratum shares into share blocks (P2Poolv2
  mode).
- **PplnsShareWorker** -- stores hydrapool PPLNS shares that met their session
  difficulty; spawned by `build_node`, outside the actor.
- **StoreWriter** -- serializes all RocksDB writes on a `spawn_blocking` thread.

Every channel is bounded, and every edge has an explicit policy:

| From -> To | Capacity | Policy on full |
|---|---|---|
| Actor -> per-peer service | max(rps, 16) | `try_send`; disconnect the peer |
| Actor -> ResponseWorker | 1024 | `try_send`; warn and drop (header sync re-requests, fetcher times out) |
| Actor -> BlockFetcher (`PeerRemoved`) | 8192 | `try_send`; warn (in-flight requests time out) |
| Workers / services -> Actor (`swarm_tx`) | 100 | awaited send; bounded because the actor always drains |
| ResponseWorker -> BlockReceiver | 8192 | awaited send |
| BlockReceiver -> ValidationWorker | 8192 | awaited send |
| BlockReceiver -> BlockFetcher | 8192 | awaited send |
| EmissionWorker -> ValidationWorker | 8192 | awaited send |
| Stratum connection -> PplnsShareWorker | 1000 | awaited send (enqueue only) |
| ValidationWorker -> OrganiseWorker | 512 | awaited send, holding the validation permit |
| OrganiseWorker -> ValidationWorker (stranded re-drive, startup seed) | 8192 | `try_send`; skip (next arrival re-drives) |
| Share transaction pool -> Actor (relay, Bitcoin Cash) | 256 | `try_send`; drop the relay (the transaction stays in the pool and is mined) |

**The cycle rule.** The channel graph must have no cycle of awaited sends.
Where a cycle is unavoidable, at least one edge in it is non-blocking:

- `swarm_tx` <-> actor: the actor never awaits; it drains `swarm_tx` and
  hands inbound work out with `try_send`.
- Validation <-> Organise: validation awaits `organise_tx`; organise reaches
  validation only through `try_send`.

**Validation permits.** A validation task holds its semaphore permit until it
finishes, including the broadcast send on `swarm_tx`. The permit therefore also
bounds how many validated blocks can wait on a full `swarm_tx`. Releasing it
before the broadcast would let tasks pile up without limit, so do not.

When adding a channel, add it to this table with its capacity and policy, and
check that it does not close a cycle of awaited sends.

## RocksDB Reads in Request Handlers

Request handlers (`getheaders`, `getblocks`, `getdata_block`) call
`ChainStoreHandle` methods that read RocksDB synchronously on the tokio
thread. These do NOT need `spawn_blocking` because:

1. RocksDB checks memtable then block cache before touching disk
2. At P2Pool scale, the working set of recent shares and headers fits
   comfortably in the block cache and OS page cache
3. Reads complete in single-digit microseconds in the common case
4. `spawn_blocking` overhead (thread scheduling, context switch, future
   wakeup) would cost more than the reads themselves

Write operations correctly use async channels to the StoreWriter thread
because they involve compaction-visible mutations and batch commits.

## BCH Storage and Memory Budget (2026-10-30)

These are planning estimates, not disk or cache measurements. MB and GB
use decimal bytes. The rate assumption is one stored share per 10 seconds.
The BCH payout window is capped at 120,960 shares, about two weeks. Body
retention is longer: `BCH_PRUNE_DEPTH` is 242,126 shares, about 28 days,
before the prune interval and background-task lag. Headers and merkle
branches remain after body pruning.

A share stores its BCH coinbase, coinbase merkle branch and share-chain
transactions. It does not store every transaction from the BCH template.
`BitcoinTxids` has no production writer. The earlier estimate of 7.7 GB of
template txids and 2 GB of deduplicated BCH bodies did not describe this
store. See `store/share_store.rs` and `store/prune_blocks.rs`.

For no-token shares with `N` direct P2PKH payees, use `367 + 34 * N`
bytes as a conservative coinbase budget. The 367 bytes reserve every
non-payee field at its largest size, including a tree output even when
none is needed. P2SH20 outputs take 32 bytes each; P2SH32 outputs take 44.
There is no BCH witness data or witness commitment. Token shares cap the
coinbase at 3,072 bytes and move excess payees to a payout tree. No-token
coinbases may reach 1,000,000 bytes. See `accounting/payout/coinbase_budget.rs`.

| Coinbase scenario | Bytes per coinbase | 242,126 retained coinbases |
|---|---|---|
| 10 direct P2PKH payees | at most 707 | 0.17 GB |
| 100 direct P2PKH payees | at most 3,767 | 0.91 GB |
| 1,000 direct P2PKH payees | at most 34,367 | 8.32 GB |
| Token-share limit | 3,072 | 0.74 GB |
| No-token limit | 1,000,000 | 242.13 GB |

The share-chain rule permits another 204,800 bytes of share transactions.
Filling both body allowances on every retained share gives 291.71 GB of
raw transaction bytes. This excludes headers, branches, keys, transaction
indexes, payout-tree data, forks, RocksDB overhead and pruning lag. It is
not an on-disk upper bound. See `shares/validation/mod.rs`.

At the same rate, permanent header and branch data grows by an estimated
3.7-4.2 GB per year in the 32 MB to 1 GB template scenarios in the
[pruning appendix](pruning.md#appendix-growth-of-data-kept-forever).
BCHN holds full BCH block data separately. At 600 seconds per BCH block,
two weeks contain about 2,016 blocks: 32 MB blocks would add 64.51 GB;
128 MB blocks, 258.05 GB; and 1 GB blocks, 2.02 TB. These are hypothetical
filled-block volumes. Mainnet's 32 MB ABLA floor is not a fixed block-size
cap. Use the actual template size and BCHN's pruning settings when sizing
a deployment. See the [ABLA specification](https://gitlab.com/0353F40E/ebaa/-/blob/main/README.md#configuration-constants).

Serialized byte counts do not predict heap use or cache hit rates. A
large template also occupies memory in RPC decoding and job construction.
Measure those paths and old-share reads under the intended load before
assuming the working set fits in memory.

## When to Revisit

- If the share chain grows large enough to exceed available memory
- If RocksDB compaction stalls become measurable under load
- If peer counts increase significantly beyond current expectations
