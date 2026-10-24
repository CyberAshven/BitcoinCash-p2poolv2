# P2Poolv2 for Bitcoin Cash

P2Poolv2 for Bitcoin Cash is a peer-to-peer mining pool. Miners run their own
node next to Bitcoin Cash Node (BCHN), build their own block templates, and
agree on a share chain with their peers instead of trusting a pool operator.
Every block the pool finds pays the miners directly in its coinbase: nothing
is held, and nobody has to sign before a miner owns its payout.

This fork, [CyberAshven/BitcoinCash-p2poolv2](https://github.com/CyberAshven/BitcoinCash-p2poolv2),
ports [P2Poolv2](https://github.com/p2poolv2/p2poolv2) to Bitcoin Cash. The
upstream project, its design and most of its code are the work of the
P2Poolv2 developers.

## In short

- **Payouts in the coinbase.** PPLNS over a window of three Bitcoin Cash
  blocks of work, at most 120,960 shares; shares count once 100 deep. Uncles
  are paid 9/10 of their difficulty.
- **Main and mini share chains.** The mini chain's share target is 256 times
  easier, for small miners. See [Small miners](small-miners.md).
- **Mainnet and chipnet run the same rules;** only the network setting
  differs. Regtest is for development.
- **1% donation** to the project by default, an optional operator fee (1%
  unless set), and at most 10% for each, enforced by the share chain.
- **Stratum V2 first:** Noise encryption, standard and extended channels,
  both Job Declaration modes, and templates from a Template Provider. Stratum
  V1 stays for older firmware. See [Miner options](miner-options.md).
- **Share trading:** a share's payout right can be sold for BCH through a hash
  time locked contract on Bitcoin Cash. See
  [Trading shares for BCH](atomic-swap/bch-share-trading.md).
- **All Rust,** including the built-in dashboard.

The decisions behind these are in [Plan and decisions](bch-plan.md).

## Where to start

- Run a node: [Running P2Poolv2 on Bitcoin Cash](bch-quickstart.md), then
  [Requirements](requirements.md) and [Docker, Ansible and Umbrel](deployment.md).
- Point a miner at a node: [Miner options](miner-options.md).
- Understand the risks: [Known attacks and threat model](known-attacks.md).
- Work on the code: the [Architecture](architecture/README.md) pages and
  `AGENTS.md` in the repository.

## How it works

Ported from upstream's documentation site and adapted to Bitcoin Cash (the
[port map](upstream-port-map.md) lists every page):

- Why: [Why P2Poolv2](why-p2poolv2.md) and
  [Comparison with SV2 and DATUM](compare-datum-sv2.md).
- The network and a node: [P2P network of miners](p2p-network.md),
  [Node components](node-components.md) and
  [Stratum server and vardiff](stratum.md).
- The [Share chain](sharechain/README.md): commitments, validation, PPLNS
  accounting, share transactions and pool size limits.
- [Payouts](payouts/README.md): in the coinbase, and by selling shares.

## Status

Alpha. The pool runs on chipnet; mainnet has not started. Releases are on
the [releases page](https://github.com/CyberAshven/BitcoinCash-p2poolv2/releases)
and the Docker image is `ghcr.io/cyberashven/bitcoincash-p2poolv2`.

## License

The fork is distributed under AGPL-3.0 (plan decision 13). Files from
upstream P2Poolv2 keep their MIT or Apache-2.0 notices.
