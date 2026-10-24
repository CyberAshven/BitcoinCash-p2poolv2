# Spending share outputs

> Ported from upstream P2Poolv2's documentation:
> `architecture/spending_utxos.adoc` in
> [p2poolv2/docs](https://github.com/p2poolv2/docs) at commit `623e4c1`,
> Copyright 2024-2026 P2Poolv2 Developers, MIT or Apache-2.0. Adapted to
> Bitcoin Cash by this fork; see the [port map](../upstream-port-map.md).

Spending a share coinbase creates new outputs, and each later spend creates
more: a chain of outputs rooted at the coinbase where the units were first
created. The height of that coinbase is the output's **root height**, and
every output descended from it carries it. Once an output's root height is
too deep, the output can no longer be spent. This limits every output to the
life of the proof of work behind its root coinbase.

On Bitcoin Cash the limit is `BCH_PAYOUT_DEPTH`: an output can be spent in a
share at height `h` only if its root height is at least `h - 121,063`. That
is Bitcoin Cash's 100-share maturity, the 120,960-share window and 3 shares
for an uncle of the deepest window share, the deepest share a coinbase can
pay; it also bounds how much of the chain every node must keep
([Pruning](../architecture/pruning.md)). Bitcoin keeps upstream's
`PAYOUT_DEPTH`, 127,008 (its 6,048-share maturity plus the window). A share
coinbase output itself can first be spent 100 shares after its share on
Bitcoin Cash (6,048 on Bitcoin).

```text
coinbase output, root height 1,000
  | spent at 1,100 or later: the 100-share maturity on Bitcoin Cash
  v
output, root height 1,000
  | spent at 60,000
  v
output, root height 1,000
  | a spend at 122,064 or later is rejected: past 1,000 + 121,063
  x
```

## Splitting outputs

When an output is split into several, each new output keeps the root height of
the output it spends.

```text
output (root 1,000) --> transaction --> output A (root 1,000)
                                    \-> output B (root 1,000)
```

## Merging outputs

When a transaction spends several outputs, every new output takes the
**lowest** root height among them: it expires when the oldest root coinbase
expires.

```text
output (root 1,000) --\
                       --> transaction --> output A (root 1,000)
output (root 5,000) --/                \-> output B (root 1,000)
```

Wallets that split and merge share outputs must choose inputs with care, and
should warn before a merge shortens an output's life. An output whose
ancestry a node could not resolve when it stored it is refused as a spend
until the node has it.

## What expiry means on Bitcoin Cash

On Bitcoin, upstream ties a share output's earnings to its root height: the
output earns until its root leaves the window. On Bitcoin Cash, units earn
nothing; a share's BCH income follows its payout right, which earns only
while its own share is in the PPLNS window (three Bitcoin Cash blocks of
work, at most 120,960 shares). Root heights therefore only decide how long
units can be spent, mostly to pay fees and to carry a right through a trade.
Merging also matters for rights: a transaction moves the right on input `i`
to its output `i`, so a merge into fewer outputs freezes the rights on the
extra inputs with their current holder
([Trading shares for BCH](../atomic-swap/bch-share-trading.md#consequences)).
