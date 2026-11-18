# Why P2Poolv2

> Ported from upstream P2Poolv2's documentation: `why_p2poolv2.adoc` in
> [p2poolv2/docs](https://github.com/p2poolv2/docs) at commit `623e4c1`,
> Copyright 2024-2026 P2Poolv2 Developers, MIT or Apache-2.0. Adapted to
> Bitcoin Cash by this fork; see the [port map](upstream-port-map.md).

Mining pools are a centralizing force. A pool operator picks the
transactions in every block its miners find, holds or routes their rewards,
and decides who may mine with it, so a few large pools decide what most
blocks contain. Bitcoin Cash is mined with the same SHA-256 machines as
Bitcoin, through the same kind of pools.

The goal is the same on both chains: **let miners find blocks without
depending on a server that one organization controls.**

## What P2Poolv2 does about it

Miners run P2Poolv2 to pool their hashrate. The nodes agree among themselves
how much each miner is paid, and every node checks all the work, so no
single operator can stall the pool, hijack it, or leave a miner out:

- **Each miner, or the node a group of miners trusts, builds its own block
  templates** from its own Bitcoin Cash Node (BCHN) or a Stratum V2 Template
  Provider it chooses.
- **Payouts are coinbase outputs** every node checks. Nobody holds a balance
  for anyone.
- **Anyone can join** the peer-to-peer network and start mining.

P2Poolv2 reboots the original P2Pool, which also has a Bitcoin Cash network
(`p2pool/networks/bitcoincash.py` in
[jtoomim/p2pool](https://github.com/jtoomim/p2pool)). This fork reimplements
in Rust the ideas it takes from P2Pool v1 (plan decision 10 in
[bch-plan.md](bch-plan.md)).

## What is different on Bitcoin Cash

Upstream designs around Bitcoin's scarce block space, rising fees, SegWit
and Lightning. Bitcoin Cash can scale and will scale, and this fork designs
for that:

- **Block space is not the limit.** The Adaptive Blocksize Limit Algorithm
  ([CHIP-2023-04](https://gitlab.com/0353F40E/ebaa/-/blob/main/README.md),
  [upgrade specification](https://upgradespecs.bitcoincashnode.org/2024-05-15-upgrade/))
  keeps the limit at 32 MB or more on mainnet. P2Poolv2 adds no block limits
  of its own.
- **Fees are about 1 satoshi per byte** (`DEFAULT_MIN_RELAY_TX_FEE_PER_KB`,
  1,000 satoshis per kB, in BCHN's `src/validation.h`): spending a small
  payout costs about two hundred satoshis.
- **No SegWit, and no Lightning in use.** Payouts go to CashAddr P2PKH or
  P2SH outputs; selling shares is an on-chain contract
  ([Trading shares for BCH](atomic-swap/bch-share-trading.md)).
- **Covenants exist today** (introspection, CashTokens, P2SH32). This fork
  uses P2SH32 for share trades and a covenant payout tree.
- **The coinbase pays every miner directly** up to the 1,000,000-byte
  transaction limit, and the rest through the payout tree
  ([Pool size limits](sharechain/pool-size.md)).

Every decision and its status: [Plan and decisions](bch-plan.md). Pools
where miners build templates but an operator keeps the accounts:
[Comparison with SV2 and DATUM](compare-datum-sv2.md).
