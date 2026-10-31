# PPLNS accounting

> Ported from upstream P2Poolv2's documentation:
> `architecture/pplns_accounting.adoc` in
> [p2poolv2/docs](https://github.com/p2poolv2/docs) at commit `623e4c1`,
> Copyright 2024-2026 P2Poolv2 Developers, MIT or Apache-2.0. Adapted to
> Bitcoin Cash by this fork; see the [port map](../upstream-port-map.md).

Each share carries the Bitcoin Cash coinbase its miner hashed. A node reads
the parts the miner chose (the coinbase flags, extranonce, nanosecond
timestamp and pool signature) out of that coinbase, rebuilds the coinbase
from its own view of the PPLNS window, and compares the two whole. If they
match, the payouts are right; otherwise the share is rejected for a PPLNS
mismatch. Bitcoin Cash has no SegWit, so unlike upstream there is no witness
commitment to read back.

PPLNS is pay per last N shares: a block pays the shares in a window below the
share that found it. The full rules, with the code that implements them, are
in [Payout distribution](../architecture/payout-distribution.md); this page
explains them.

## Maturity, the window and the trading window

Counted back from the paying share's parent:

```text
older shares                                                      newest
... | expired | PPLNS window: paid and tradable | maturity skip | tip
              | 3 BCH blocks of work,          | 100 shares,   |
              | at most 120,960 shares         | not paid yet  |
```

- **Maturity skip.** The newest 100 shares are not paid yet
  (`BCH_COINBASE_MATURITY`). A share is paid once it is 100 deep, as Bitcoin
  Cash spends a coinbase after 100 blocks: a count of the chain's own blocks,
  not a span of time. At 10-second shares that is about 17 minutes. The same
  depth makes a share's coinbase output spendable on the share chain. The
  first 100 shares of a new chain are always mature for payout.
- **PPLNS window.** Below the skip, a Bitcoin Cash coinbase pays the shares
  holding three Bitcoin Cash blocks' worth of work (`BCH_PPLNS_WINDOW_BLOCKS`),
  at most 120,960 shares (`PPLNS_WINDOW_SHARES`, two weeks of 10-second
  shares). One block's worth of work is the difficulty in the paying share's
  own Bitcoin Cash header, which Bitcoin Cash sets every block with ASERT.
  So the window spans about three of the pool's own blocks whatever the
  pool's size.
- **Trading window.** A share earns while it is in the window, and its payout
  right can be sold for BCH during that time
  ([Selling shares](../payouts/selling-shares.md)).

How long the window is depends on the pool's share of Bitcoin Cash's
hashrate (10-minute blocks, 10-second shares):

| Pool share of BCH hashrate | Window |
|---|---|
| 0.05% | the 120,960-share cap, about 14 days |
| 1% | about 18,000 shares, 50 hours |
| 10% | about 1,800 shares, 5 hours |
| 30% | about 600 shares, 100 minutes |

### How this differs from upstream

| | Upstream (Bitcoin) | This fork (Bitcoin Cash) |
|---|---|---|
| Window | 120,960 shares, always | 3 BCH blocks of work, at most 120,960 shares |
| Maturity | 6,048 shares, the time of 100 Bitcoin blocks | 100 shares, the count of Bitcoin Cash's coinbase maturity |
| Sized by | share count | the paying header's difficulty |

Why three blocks: it is the value P2Pool v1 ran on Bitcoin Cash (`SPREAD = 3`
in jtoomim's `p2pool/networks/bitcoincash.py`). A share is paid by the blocks
found while it is in the window, a Poisson count with mean 3: it misses every
block about 5% of the time, against 13.5% with two. The reasoning, the
simulation and the prior art are in
[Payout distribution](../architecture/payout-distribution.md#bitcoin-cash-window)
and plan decision 11 ([bch-plan.md](../bch-plan.md)). What it means for a
small miner is worked out on [Small miners](../small-miners.md).

These values are share-chain rules. Every Bitcoin Cash protocol string carries
them as `pplns-k3-m100`, so a node on other values cannot peer.

## Weights

Each share in the window counts its difficulty; on Bitcoin Cash, its exact
work, 2^256 / (target + 1), not its difficulty rounded down to a whole
number. A coinbase splits what is left
after the donation and the operator fee in proportion to those weights. On
Bitcoin Cash, a share's weight goes to whoever holds the share's payout right
when the block is found: the miner, or a buyer
([Trading shares for BCH](../atomic-swap/bch-share-trading.md#payout-rights)).

How the weights become coinbase outputs is on
[Coinbase payouts](../payouts/coinbase.md).

## Uncles

A share can name at most three uncles, each at most three shares below it, and
an uncle may have only one nephew on a chain
([Share validation](validation.md#uncles-follow-the-rules)).

```text
Main chain:  S1 <- S2 <- S3 <- S4 <- S5
             ^     ^
             |     |
Uncles:      A     B      (Uncle A builds on S1, Uncle B on S2)

S4's header names Uncle A and Uncle B.
```

Uncle A and Uncle B are valid shares that lost the race to the main chain. PPLNS
counts an uncle's work in full but splits it between two miners:

- the uncle's miner earns 90% of the uncle's difficulty;
- the nephew's miner earns the nephew's own difficulty plus 10% of each
  uncle's.

If every share above has difficulty `d`, the miners of Uncle A and Uncle B
earn `0.9d` each and the miner of S4 earns `1.2d`: together `3d`, the same as
three shares on the main chain, so no work goes uncounted. The bonus gives
miners a reason to name uncles, and the discount keeps a main-chain share
worth more than an uncle.

On Bitcoin Cash, a share's work for sizing the window is its difficulty plus
its uncles', and uncles take no window slots of their own. An uncle's weight
always goes to the uncle's own payout address; only a main-chain share's
weight follows its payout right.
