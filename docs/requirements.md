# Requirements

Two parts: what a machine needs to run a Bitcoin Cash P2Poolv2 node, and the
design requirements P2Poolv2 sets itself, with this fork's status on each.

## Running a node

A node is Bitcoin Cash Node (BCHN) plus the P2Poolv2 node, on one machine or
a trusted LAN. Mainnet and chipnet run the same P2Poolv2 rules; regtest is
for development.

### Bitcoin Cash Node

- BCHN 29.2.0 or later, from [bitcoincashnode.org](https://bitcoincashnode.org).
  The Docker stack pins the release tarball by SHA-256
  (`docker/bchn/Dockerfile`).
- Synced to the network the pool mines: BCHN refuses `getblocktemplate`
  during initial block download ("Bitcoin is downloading blocks...") and,
  unless `allowunconnectedmining=1` (regtest only), without peers ("Bitcoin
  is not connected!").
- `bitcoin.conf` as in [the quickstart](bch-quickstart.md#1-run-bitcoin-cash-node),
  with a long random `rpcpassword`. P2Poolv2 logs in with a user and
  password; it cannot read BCHN's `.cookie` file yet.
- No wallet (`disablewallet=1` is fine) and no `txindex`. Pruning is fine.
- `-percentblockmaxsize` below 100% (mainnet default 50%), to leave room for
  the pool's coinbase.

### The P2Poolv2 node

- Linux (x64 or ARM64), macOS (Apple Silicon) or Windows x64, from the
  [releases](https://github.com/CyberAshven/BitcoinCash-p2poolv2/releases),
  the Docker image `ghcr.io/cyberashven/bitcoincash-p2poolv2`, or built with
  Rust 1.88 or later.
- A stable clock (NTP): shares more than 120 seconds in the future are
  rejected, and share difficulty follows share timestamps.
- An always-on connection. A share is about 1 KB, one every 10 seconds;
  latency matters more than bandwidth, because a share that reaches peers
  late becomes an uncle.

### Disk

| What | Size |
|---|---|
| BCHN mainnet, full | about 218 GB of blocks (Blockchair, October 2026), plus the UTXO set |
| BCHN mainnet, pruned | the prune target (the Docker stack keeps 10 GB) plus the UTXO set |
| BCHN chipnet | a small fraction of mainnet; the Docker stack prunes to 2 GB |
| P2Poolv2 share chain, kept forever | about 1 KB a share, 3.1 GB a year |
| P2Poolv2 block bodies, pruned | the last 242,534 shares (`BCH_PRUNE_DEPTH` plus `PRUNE_INTERVAL`, about 28 days), mostly coinbases: from about 0.1 GB with ten miners to about 8 GB with a thousand |

Share chain figures: [architecture/pruning.md](architecture/pruning.md),
"Growth of Data Kept Forever" (Bitcoin Cash shares are slightly smaller: a
20-byte P2PKH owner instead of a 32-byte Taproot key). Use an SSD: BCHN and
RocksDB do many small random reads.

### Memory and CPU

Not measured yet. The P2Poolv2 node keeps the shares of its PPLNS window (at
most 120,960) and RocksDB's caches in memory.

### Ports

| Port | Service | Who connects | Open to |
|---|---|---|---|
| 3333 | Stratum V1 (`[stratum] port`) | miners | your miners: LAN, or the internet for a public pool |
| 3336 | Stratum V2 (`sv2_port`, optional) | SV2 miners, proxies, Job Declaration clients | LAN without authority keys; with Noise keys, as you choose |
| 6884 / 6886 | Share chain P2P (`[network] listen_address`): mainnet / chipnet | other P2Poolv2 nodes | the internet, so others can dial you; set `external_address` behind NAT |
| 46884 | API and dashboard (`[api] port`) | you, Prometheus, a reverse proxy | localhost or a reverse proxy; `public_dashboard` for a public node |
| 8332 / 48332 / 18443 | BCHN RPC: mainnet / chipnet / regtest | the P2Poolv2 node | localhost only |
| 8333 / 48333 / 18444 | BCHN P2P: mainnet / chipnet / regtest | Bitcoin Cash peers; the P2Poolv2 node with `node_p2p_address` | the internet (outbound is enough) |
| 28332 | BCHN ZMQ `hashblock` | the P2Poolv2 node | localhost only |
| 9090, 3000 | Prometheus, Grafana (optional) | you | localhost or a reverse proxy |

Bitcoin Cash has no P2Poolv2 DNS seeds, so a joining node needs a peer in
`[network] dial_peers` or one found through Nostr
([P2P network](p2p-network.md#finding-peers-through-nostr)).

## Design requirements

Upstream P2Poolv2's requirements (its docs, `requirements.adoc`), checked
against this fork. "Met" means implemented and tested on regtest or chipnet;
mainnet has not started.

### Functional

| Requirement | Bitcoin Cash fork |
|---|---|
| **Nakamoto consensus.** The share chain uses proof of work and most-work fork choice; nodes converge. | Met. One share chain per network, from its own genesis share. |
| **Reward for all work.** Stale shares are paid, not orphaned. | Met. An uncle is paid 9/10 of its difficulty and its nephew 1/10. |
| **No centralized operator.** No service is needed for accounting or payouts. | Met. Every node builds and checks the same coinbase. |
| **Permissionless mining.** Anyone joins without permission. | Met. |
| **Trustless joins.** A new node follows the most-work valid chain, trusting no one. | Met. Headers sync from genesis with full difficulty checks. |
| **Sovereign templates.** Every node builds its own templates. | Met. The node's own BCHN or a Stratum V2 Template Provider; miners can declare their own jobs (Job Declaration, both modes). |
| **Mining on the current chain.** A coinbase pays only work on the BCH chain its block extends, give or take one block. | Built, not yet run on chipnet: upstream's design (`architecture/mine-on-bitcoin-chain.md`) with an aserti3-2d header check; see [Mining on the current chain](sharechain/current-chain.md). |
| **Precise hashrate reporting.** Miners see their hashrate from Stratum shares. | Met. The dashboard, `/miner` and Prometheus metrics. |

### Payouts

| Requirement | Bitcoin Cash fork |
|---|---|
| **Unilateral exits.** Nobody holds payouts or must sign before miners own them. | Met. Payouts are coinbase outputs to miners' own CashAddr scripts, spendable after 100 blocks. |
| **Payout commitment at template time.** The work commits to the payouts. | Met. The job's coinbase is the payout; the share commits to it. |
| **Deterministic, verifiable payouts.** Payouts follow from the share chain and every node checks them. | Met. Validators rebuild each share's coinbase: window, donation, fee and payout rights to traded shares. |
| **Scalability.** Thousands of miners without hitting coinbase size limits. | Met with limits, not yet run at scale. A coinbase pays every miner directly up to the 1,000,000-byte transaction limit, about 29,400 P2PKH payees (3,072 bytes on a share that merge-mines tokens), and the rest through a covenant payout tree (plan decision 14). An SV2 extended channel takes about 1,900 P2PKH payouts; stock ASIC firmware limits are not yet measured on BCH hardware. See [Pool size limits](sharechain/pool-size.md). |
| **Small miner support.** Small miners can spend their earnings without third parties. | Met with limits. The share chain's easy target ceiling and the work-sized window keep payouts above dust ([small-miners.md](small-miners.md)); shares can be sold for BCH through a hash time locked contract ([atomic-swap/bch-share-trading.md](atomic-swap/bch-share-trading.md)). |

### Bitcoin Cash additions

From the fork's plan ([bch-plan.md](bch-plan.md), "Decisions so far").

| Requirement | Status |
|---|---|
| Mainnet and chipnet run the same rules; only the network switch differs. | Met. |
| 1% donation by default, optional operator fee (1% when only its address is set), at most 10% each (no combined cap), enforced by the share chain. | Met. |
| CashAddr payouts, P2PKH share owners, canonical transaction order. | Met. |
| Stratum V2 first, with Noise encryption, both Job Declaration modes and Template Distribution. | Met; interoperability with independent clients is still being tested. |
| All code Rust. | Met. |
