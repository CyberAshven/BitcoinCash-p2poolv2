# Payouts

> Ported from upstream P2Poolv2's documentation: `payouts/index.adoc` in
> [p2poolv2/docs](https://github.com/p2poolv2/docs) at commit `623e4c1`,
> Copyright 2024-2026 P2Poolv2 Developers, MIT or Apache-2.0. Adapted to
> Bitcoin Cash by this fork; see the [port map](../upstream-port-map.md).

Upstream pays miners in two ways: larger miners directly from the coinbase,
and smaller miners by selling their shares to other miners or to market
makers, who then collect those shares' payouts from the coinbase.

On Bitcoin Cash:

- **Every block pays miners in its coinbase.** Nobody holds a balance, and the
  outputs are spendable after 100 blocks. See
  [Coinbase payouts](coinbase.md).
- **A coinbase pays every miner directly,** up to Bitcoin Cash's
  1,000,000-byte transaction limit. Only a share that merge-mines tokens
  keeps its coinbase within 3,072 bytes and pays the payees past that through
  a covenant payout tree. See [Pool size limits](../sharechain/pool-size.md).
- **A miner can sell a share's payout right for BCH,** with a contract on each
  chain and no Lightning. The buyer is then paid that share's part of each
  block. See [Selling shares](selling-shares.md).
