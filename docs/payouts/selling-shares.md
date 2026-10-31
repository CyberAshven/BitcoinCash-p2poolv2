# Selling shares

> Ported from upstream P2Poolv2's documentation: `payouts/marketplace.adoc`
> ("Sell Shares for Payouts"), with `payouts/onchain.adoc`,
> `payouts/l2_swaps.adoc`, `payouts/marketplace_ux.adoc` and
> `payouts/pricing_shares_as_options.adoc`, in
> [p2poolv2/docs](https://github.com/p2poolv2/docs) at commit `623e4c1`,
> Copyright 2024-2026 P2Poolv2 Developers, MIT or Apache-2.0. Adapted to
> Bitcoin Cash by this fork; see the [port map](../upstream-port-map.md).

## Why sell a share

Upstream's case: once a pool grows past what a coinbase can carry, the smaller
miners need another way to be paid. Its requirement that every miner can exit
unilaterally points to covenant-based joinpools, which Bitcoin does not have,
so upstream's first answer is a market: miners sell their shares to the
highest bidder, and buyers who hold enough shares are paid in the coinbase.

On Bitcoin Cash the premise differs:

- **Covenants exist.** Introspection (May 2022), CashTokens and P2SH32 (May
  2023) let a script check the transaction that spends it, so a covenant
  payout tree needs no change to Bitcoin Cash. Coinbase payouts use one for
  the payees past a share's coinbase limit; see
  [Pool size limits](../sharechain/pool-size.md#the-hybrid-payout).
- **Selling is still useful.** A share is paid a little by each block the pool
  finds while it is in the window. A miner who sells gets one payment now,
  and hands the risk of the pool's luck to the buyer.

## What is sold

On Bitcoin Cash a buyer gets a share's **payout right**: from the claim on,
every block found while that share is in its PPLNS window pays the share's
weight to the buyer's key hash. The right is whole, and travels with the share
output at the same input and output position
([Trading shares for BCH](../atomic-swap/bch-share-trading.md#payout-rights)).

The share chain sets two limits:

1. **A share can be sold only once it is 100 shares deep,** when its coinbase
   output matures. It is not paid before that depth either.
2. **A share is worth nothing once it leaves the window.** Its output can still
   be spent for a while ([Spending share outputs](../sharechain/spending-outputs.md)),
   but its right earns nothing more.

## Holding or selling

Holding a share: after maturity the miner is paid by each block the pool finds
while the share is in the window.

```text
mined | maturity: 100 shares | window: about 3 of the pool's blocks | expired
      | nothing paid         | +x     +x  +x                         |
miner total: 3x on average (Poisson: 5% of shares are paid by no block)
```

Selling a share: the miner takes one payment at the sale, and the buyer
collects the payouts.

```text
mined | maturity | sale | window: about 3 of the pool's blocks      | expired
      |          | +y   | buyer: +x  +x  +x                        |
miner total: y, now; buyer total: 3x on average, later
```

So miners trade risk for an immediate payout, and buyers take on the risk for
a discount: a sale makes sense to both when `y` is a little below what the
buyer expects to collect.

Upstream draws the window as about two weeks. On Bitcoin Cash it is three
blocks' worth of work: two weeks only for pools below about 0.15% of Bitcoin
Cash's hashrate, about 50 hours at 1% and about 5 hours at 10%
([PPLNS accounting](../sharechain/pplns-accounting.md)).

## On-chain swaps

Upstream's on-chain swap page is a heading with no text yet. On Bitcoin Cash
the swap is built: one hash time locked contract behind P2SH32 on each chain,
under the same secret hash. The seller locks the share first, the buyer locks
BCH, the seller's BCH claim reveals the secret, and the buyer uses it to claim
the share; height locks refund both sides otherwise. Nodes relay and mine the
share-chain transactions. Steps, timeouts and tools
(`p2poolv2_cli bch-trade`) are in
[Trading shares for BCH](../atomic-swap/bch-share-trading.md).

## Off-chain swaps

Upstream plans swaps over Ark and Lightning ("coming soon"). Neither is
planned here: Bitcoin Cash has no Lightning network in use, Ark is a Bitcoin
design, and plan decision 13 keeps share trading on Bitcoin Cash, where the
on-chain contract costs a few hundred satoshis. Upstream's Lightning example
is kept as background:
[Shares for Lightning bitcoin](../atomic-swap/p2pool-2-lightinig-example.md).

## Marketplace application

Upstream's mock-up page has headings only (miner and marketplace views). There
is no marketplace application here either: a trade is two parties using
`p2poolv2_cli bch-trade`. An application that matches sellers and buyers is
not designed.

## Pricing a share: an open question

Upstream treats a share as an option with a known expiry and has not yet
written how to price it. This fork has not either. What a buyer can read:

- the share's place in the window and how much of it is left;
- the share's difficulty, the `bits` in its share header;
- the Bitcoin Cash difficulty, the `bits` of the paying blocks, which sizes
  the window;
- the share's height on the share chain, and who holds its right.

One estimate, not a pricing model: on Bitcoin Cash, below the window's cap,
a share of difficulty `d` is paid `R' × d / (3 × D)` by each block found while
it is in the window, where `R'` is the block reward left after the donation
and fee and `D` is the block's difficulty. The window holds three blocks of
work, so a share is paid by three blocks on average, wherever the pool's
hashrate goes, and its expected value at maturity is about `R' × d / D`: the
same as its share of one block's expected work. A share part way through the
window has about `3 × (fraction of the window left)` blocks to go. The spread
around those means (5% of shares are paid by no block), changes in Bitcoin
Cash difficulty, fees in the blocks, and the time value of BCH are what a
price must cover. At the 120,960-share cap, for small pools, the window is a
share count instead, and the expected number of blocks depends on the pool's
hashrate.
