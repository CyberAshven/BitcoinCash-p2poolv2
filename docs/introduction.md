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

- **Payouts in the coinbase:** PPLNS over three Bitcoin Cash blocks of work,
  at most 120,960 shares, counted once 100 deep; uncles get 9/10
  ([PPLNS accounting](sharechain/pplns-accounting.md)).
- **One share chain** per network, with a low target ceiling (difficulty
  about 4.55) for small miners; a big miner's shares are up to 64 times
  harder ([Small miners](small-miners.md)). Mainnet and chipnet run the same
  rules; regtest is for development.
- **1% donation** by default, an optional operator fee (1% unless set), at
  most 10% each, enforced by the share chain.
- **Stratum V2 first** (Noise, standard and extended channels, both Job
  Declaration modes, Template Providers); Stratum V1 for older firmware
  ([Miner options](miner-options.md)).
- **Share trading:** a share's payout right can be sold for BCH through a
  hash time locked contract ([Trading shares for BCH](atomic-swap/bch-share-trading.md)).
- **All Rust,** the built-in dashboard included.

The decisions behind these: [Plan and decisions](bch-plan.md).

## Where to go next

- Run a node: [Running P2Poolv2 on Bitcoin Cash](bch-quickstart.md),
  [Requirements](requirements.md), [Docker, Ansible and Umbrel](deployment.md).
- Mine on a node: [Miner options](miner-options.md).
- Risks: [Known attacks and threat model](known-attacks.md).
- How it works, ported from upstream's documentation site (see the
  [port map](upstream-port-map.md)): [Why P2Poolv2](why-p2poolv2.md),
  [Comparison with SV2 and DATUM](compare-datum-sv2.md),
  [P2P network of miners](p2p-network.md),
  [Node components](node-components.md),
  [Stratum server and vardiff](stratum.md), the
  [Share chain](sharechain/README.md) and [Payouts](payouts/README.md).
- The code: the [Architecture](architecture/README.md) pages and `AGENTS.md`
  in the repository.

## Status

Alpha. The pool runs on chipnet; mainnet has not started. Releases are on
the [releases page](https://github.com/CyberAshven/BitcoinCash-p2poolv2/releases)
and the Docker image is `ghcr.io/cyberashven/bitcoincash-p2poolv2`.

## Sponsor

P2Poolv2 for Bitcoin Cash is free, open-source software. Donations keep it
going. Send BCH, or CashTokens, to the address the pool's own donation pays
in each block it finds (1% by default;
[choose your own](miner-options.md#donation)):

- BCH: `bitcoincash:qze95tc2dqrnltvxfa5yhunwx5952f34fvw4g6365k`
- CashTokens-aware, the same key:
  `bitcoincash:zze95tc2dqrnltvxfa5yhunwx5952f34fvflmylut9`

The landing page's
[donate section](https://cyberashven.github.io/BitcoinCash-p2poolv2/#donate)
shows both with QR codes and Open in wallet links.

## License

AGPL-3.0 (plan decision 13). Files from upstream P2Poolv2 keep their MIT or
Apache-2.0 notices.
