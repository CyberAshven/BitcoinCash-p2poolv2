# Requirements

Two parts: what a machine needs to run a Bitcoin Cash P2Poolv2 node, and the
design requirements P2Poolv2 sets itself, with where the Bitcoin Cash fork
stands on each.

## Running a node

A node is Bitcoin Cash Node (BCHN) plus the P2Poolv2 node, on the same
machine or on a trusted LAN. Mainnet and chipnet run the same P2Poolv2 rules;
only the network setting differs. Regtest is for development.

### Bitcoin Cash Node

- BCHN 29.2.0 or later, from [bitcoincashnode.org](https://bitcoincashnode.org).
  The Docker stack pins the release tarball by SHA-256
  (`docker/bchn/Dockerfile`).
- Synced to the network the pool mines. P2Poolv2 asks for templates with
  `getblocktemplate`, which BCHN refuses during initial block download
  ("Bitcoin is downloading blocks...") and, unless `allowunconnectedmining=1`,
  without peers ("Bitcoin is not connected!"). Use `allowunconnectedmining=1`
  on regtest only.
- In `bitcoin.conf`:

  ```ini
  server=1
  rpcuser=p2pool
  rpcpassword=<a long random password>
  zmqpubhashblock=tcp://127.0.0.1:28332
  ```

  P2Poolv2 logs in with a user and password; it cannot read BCHN's `.cookie`
  file yet.
- A wallet is not needed (`disablewallet=1` is fine), nor `txindex`.
  Pruning is fine: the pool needs templates, not old blocks.
- Leave `-percentblockmaxsize` below 100% (BCHN's mainnet default is 50%):
  the pool adds a coinbase with one output per miner in the window.

### The P2Poolv2 node

- Linux (x64 or ARM64), macOS (Apple Silicon) or Windows x64, from the
  [releases](https://github.com/CyberAshven/BitcoinCash-p2poolv2/releases),
  the Docker image `ghcr.io/cyberashven/bitcoincash-p2poolv2`, or built with
  Rust 1.88 or later.
- A stable clock (NTP): shares more than 120 seconds in the future are
  rejected, and share difficulty follows share timestamps.
- An always-on connection. A share is about 1 KB and the chain makes one
  every 10 seconds, so share relay is light; latency matters more than
  bandwidth, because a share that reaches peers late becomes an uncle.

### Disk

| What | Size |
|---|---|
| BCHN mainnet, full | about 218 GB of blocks (Blockchair, 2026-10-23), plus the UTXO set |
| BCHN mainnet, pruned | the prune target (the Docker stack keeps 10 GB) plus the UTXO set |
| BCHN chipnet | a small fraction of mainnet; the Docker stack prunes to 2 GB |
| P2Poolv2 share chain, kept forever | about 1 KB a share, 3.1 GB a year per share chain |
| P2Poolv2 block bodies, pruned | the last 254,376 shares (about 29 days), mostly coinbases: from about 0.1 GB with ten miners to about 8 GB with a thousand |

The share chain figures are from
[architecture/pruning.md](architecture/pruning.md), "Growth of Data Kept
Forever"; Bitcoin Cash shares are slightly smaller than Bitcoin's (a 20-byte
P2PKH owner instead of a 32-byte Taproot key). Use an SSD: both BCHN and
RocksDB do many small random reads.

### Memory and CPU

Not measured yet; there are no reference figures for the P2Poolv2 node.
BCHN's own guidance applies to BCHN. The P2Poolv2 node keeps the shares of
its PPLNS window (about 128,000 at most) and RocksDB's caches in memory.

### Ports

| Port | Service | Who connects | Open to |
|---|---|---|---|
| 3333 | Stratum V1 (`[stratum] port`) | miners | your miners: LAN, or the internet for a public pool |
| 3336 | Stratum V2 (`sv2_port`, optional) | SV2 miners, proxies, Job Declaration clients | LAN without authority keys; with Noise keys, as you choose |
| 6884 | Share chain P2P (`[network] listen_address`) | other P2Poolv2 nodes | the internet, so others can dial you; set `external_address` behind NAT |
| 46884 | API and dashboard (`[api] port`) | you, Prometheus, a reverse proxy | localhost or a reverse proxy; `public_dashboard` for a public node |
| 8332 / 48332 / 18443 | BCHN RPC: mainnet / chipnet / regtest | the P2Poolv2 node | localhost only |
| 8333 / 48333 / 18444 | BCHN P2P: mainnet / chipnet / regtest | Bitcoin Cash peers | the internet (outbound is enough) |
| 28332 | BCHN ZMQ `hashblock` | the P2Poolv2 node | localhost only |
| 9090, 3000 | Prometheus, Grafana (optional) | you | localhost or a reverse proxy |

Bitcoin Cash has no P2Poolv2 DNS seeds yet, so a node joining a share chain
needs at least one peer in `[network] dial_peers`.

## Design requirements

Upstream P2Poolv2's requirements (its docs, `requirements.adoc`), checked
against this fork. "Met" means implemented and tested on regtest or chipnet;
mainnet has not started.

### Functional

| Requirement | Bitcoin Cash fork |
|---|---|
| **Nakamoto consensus.** The share chain uses proof of work and most-work fork choice; nodes converge. | Met. Main and mini share chains, each from its own genesis share. |
| **Reward for all work.** Stale shares are paid, not orphaned. | Met. An uncle is paid 9/10 of its difficulty and its nephew 1/10. |
| **No centralized operator.** No service is needed for accounting or payouts. | Met. Every node builds and checks the same coinbase. |
| **Permissionless mining.** Anyone joins without permission. | Met. |
| **Trustless joins.** A new node follows the most-work valid chain, trusting no one. | Met. Headers sync from genesis with full difficulty checks. |
| **Sovereign templates.** Every node builds its own templates. | Met. The node's own BCHN, or a Stratum V2 Template Provider; miners can declare their own jobs (Job Declaration, both modes). |
| **Mining on the current chain.** A coinbase pays only work on the BCH chain its block extends, give or take one block. | **Not met.** Designed upstream (`architecture/mine-on-bitcoin-chain.md`) but not built. On BCH it needs a header store and a header validator for aserti3-2d and chipnet's 20-minute rule. Planned before mainnet. |
| **Precise hashrate reporting.** Miners see their hashrate from Stratum shares. | Met. The dashboard, `/miner` and Prometheus metrics. |

### Payouts

| Requirement | Bitcoin Cash fork |
|---|---|
| **Unilateral exits.** Nobody holds payouts or must sign before miners own them. | Met. Payouts are coinbase outputs to miners' own CashAddr scripts, spendable after 100 blocks. |
| **Payout commitment at template time.** The work commits to the payouts. | Met. The job's coinbase is the payout; the share commits to it. |
| **Deterministic, verifiable payouts.** Payouts follow from the share chain and every node checks them. | Met. Validators rebuild each share's coinbase: window, donation, fee and payout rights to traded shares. |
| **Scalability.** Thousands of miners without hitting coinbase size limits. | Met with limits. Bitcoin Cash blocks have room (ABLA). A coinbase pays every miner directly up to Bitcoin Cash's 1,000,000-byte transaction limit, about 29,400 P2PKH payees; a share that merge-mines tokens keeps its coinbase within 3,072 bytes, and payees past a share's limit are paid through a covenant payout tree (plan decision 14). Not yet run at that scale. An SV2 extended channel takes a coinbase of about 1,900 P2PKH payouts at most, and upstream's report that stock ASIC firmware takes only about a dozen outputs is not yet measured on BCH hardware; Stratum V2 standard channels hash headers only. See [Pool size limits](sharechain/pool-size.md). |
| **Small miner support.** Small miners can spend their earnings without third parties. | Met with limits. The mini share chain and the work-sized window keep payouts above dust (see [small-miners.md](small-miners.md)); shares can be sold for BCH through a hash time locked contract ([atomic-swap/bch-share-trading.md](atomic-swap/bch-share-trading.md)). |

### Bitcoin Cash additions

These come from the fork's plan ([bch-plan.md](bch-plan.md), "Decisions so
far").

| Requirement | Status |
|---|---|
| Mainnet and chipnet run the same rules; only the network switch differs. | Met. |
| Fixed 1% donation by default, operator fee optional (1% when only its address is set), a 10% cap on each (no combined cap, since 2026-10-24), enforced by the share chain. | Met. |
| CashAddr payouts, P2PKH share owners, canonical transaction order. | Met. |
| Stratum V2 first, with Noise encryption, both Job Declaration modes and Template Distribution. | Met; interoperability with independent clients is still being tested. |
| All code Rust. | Met. |
