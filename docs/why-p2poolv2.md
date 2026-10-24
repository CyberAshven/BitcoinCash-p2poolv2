# Why P2Poolv2

> Ported from upstream P2Poolv2's documentation: `why_p2poolv2.adoc` in
> [p2poolv2/docs](https://github.com/p2poolv2/docs) at commit `623e4c1`,
> Copyright 2024-2026 P2Poolv2 Developers, MIT or Apache-2.0. Adapted to
> Bitcoin Cash by this fork; see the [port map](upstream-port-map.md).

Mining pools are a centralizing force. A pool operator picks the
transactions in every block its miners find, holds or routes their rewards,
and decides who may mine with it. A few large pools therefore decide what
most blocks contain.

Bitcoin Cash is mined with the same SHA-256 machines as Bitcoin, through the
same kind of pools. Upstream made its case for Bitcoin from the pool shares
on mempool.space. This page does not measure Bitcoin Cash's pool shares; a
block explorer's per-pool statistics show them at any time.

The goal is the same on both chains: **let miners find blocks without
depending on a server that one organization controls.**

## What P2Poolv2 does about it

P2Poolv2 is open source software that miners run to pool their hashrate.
The nodes agree among themselves how much each miner is paid, and every node
checks all the work, so no single operator can stall the pool, hijack it, or
leave a miner out. Control over mining goes back to the miners:

- **Each miner, or the node a group of miners trusts, builds its own block
  templates** from its own Bitcoin Cash Node (BCHN), or from a Stratum V2
  Template Provider it chooses.
- **Payouts are coinbase outputs** of the blocks the pool finds, checked by
  every node. Nobody holds a balance for anyone.
- **Anyone can join** the peer-to-peer network and start mining.

P2Poolv2 reboots the original P2Pool, which also has a Bitcoin Cash network
(`p2pool/networks/bitcoincash.py` in
[jtoomim/p2pool](https://github.com/jtoomim/p2pool)). This fork ports
P2Poolv2, written in Rust, to Bitcoin Cash, and reimplements
in Rust the ideas it takes from P2Pool v1 (plan decision 10 in
[bch-plan.md](bch-plan.md)).

## What is different on Bitcoin Cash

Upstream designs around Bitcoin's constraints: scarce block space, rising
fees, SegWit, and Lightning for small payouts. Bitcoin Cash has a different
set, and this fork designs for scale: Bitcoin Cash can scale and will scale.

- **Block space is not the limit.** Since May 2024 the block size limit
  follows the Adaptive Blocksize Limit Algorithm
  ([CHIP-2023-04](https://gitlab.com/0353F40E/ebaa/-/blob/main/README.md),
  [upgrade specification](https://upgradespecs.bitcoincashnode.org/2024-05-15-upgrade/)),
  with 32 MB as its floor on mainnet and growth driven by real use. BCHN
  enforces it; P2Poolv2 takes its templates from BCHN and adds no block
  limits of its own.
- **Fees are about 1 satoshi per byte.** BCHN's default minimum relay fee
  is 1,000 satoshis per kB (`DEFAULT_MIN_RELAY_TX_FEE_PER_KB` in BCHN's
  `src/validation.h`), so spending a small payout costs about two hundred
  satoshis.
- **No SegWit, and no Lightning in use.** Payouts go to CashAddr P2PKH or
  P2SH outputs, and selling shares is an on-chain contract on both chains
  ([Trading shares for BCH](atomic-swap/bch-share-trading.md)).
- **Covenants exist today.** Native introspection (May 2022), CashTokens
  and P2SH32 (May 2023) let a script check the transaction that spends it.
  This fork uses P2SH32 for share trades and for a covenant payout tree.
- **The coinbase pays every miner directly,** up to Bitcoin Cash's
  1,000,000-byte transaction limit, with no cap taken from Bitcoin's
  firmware reports. Only a share that merge-mines tokens keeps its coinbase
  within 3,072 bytes and pays the payees past that through the payout tree.
  See [Pool size limits](sharechain/pool-size.md) and
  [Coinbase payouts](payouts/coinbase.md).

The [Plan and decisions](bch-plan.md) page lists every Bitcoin Cash decision
and its status. [Comparison with SV2 and DATUM](compare-datum-sv2.md) shows
how this differs from pools where miners build templates but an operator
still keeps the accounts.
