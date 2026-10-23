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
- **How many payees a coinbase carries is an open design question.** The code
  today caps a coinbase at 11 payouts and pays the rest by a draw. It is being
  redesigned as a hybrid: direct payouts to every miner, and a covenant payout
  tree only for shares that merge-mine tokens once their coinbase passes a
  threshold. See [Pool size limits](../sharechain/pool-size.md).
- **A miner can sell a share's payout right for BCH,** with a contract on each
  chain and no Lightning. The buyer is then paid that share's part of each
  block. See [Selling shares](selling-shares.md).
