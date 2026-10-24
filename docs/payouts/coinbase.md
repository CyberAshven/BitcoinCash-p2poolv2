# Coinbase payouts

> Ported from upstream P2Poolv2's documentation: `payouts/coinbase.adoc` in
> [p2poolv2/docs](https://github.com/p2poolv2/docs) at commit `623e4c1`,
> Copyright 2024-2026 P2Poolv2 Developers, MIT or Apache-2.0. Adapted to
> Bitcoin Cash by this fork; see the [port map](../upstream-port-map.md).

Every node that builds a share computes the payouts from the PPLNS window
([PPLNS accounting](../sharechain/pplns-accounting.md)) and builds a Bitcoin
Cash coinbase that pays the miners directly. Every node that receives the
share rebuilds that coinbase and rejects the share if it differs
([Share validation](../sharechain/validation.md)). If the share also meets the
Bitcoin Cash target, its coinbase becomes the block's coinbase and the
payouts land on the Bitcoin Cash chain, spendable after 100 blocks.

## An example

Three miners have found the six shares in a window, all at the same
difficulty, so each miner's part is its count of shares. The block reward is
3.125 BCH (fees left out), and the node keeps the default 1% donation and
no operator fee:

```text
PPLNS window, newest first:  B  A  C  A  B  A

Bitcoin Cash coinbase, 3.125 BCH:
  donation, 1%                        0.03125000 BCH
  Miner C, 1 of 6 shares (16.7%)      0.51562500 BCH
  Miner B, 2 of 6 shares (33.3%)      1.03125000 BCH
  Miner A, 3 of 6 shares (50%)        1.54687500 BCH
  merge-mining commitment             0
  padding                             0
  share commitment                    0
```

Amounts are split in whole satoshis: each payee's part is rounded down and the
largest payee gets the remainder. The example leaves out uncles; how
they are paid is on
[PPLNS accounting](../sharechain/pplns-accounting.md#uncles).

## Coinbase outputs

In order:

1. **Donation.** To the project's fixed key (plan decision 9): the
   donation the finding miner's login chose with `donation=` (0 to 10% in
   steps of 0.5%, [Miner options](../miner-options.md#donation)), or the
   node's `donation`, 1% by default, when it chose none. The share header
   commits to it, and peers rebuild the coinbase from that.
2. **Operator fee,** if the node sets `fee_address`: `fee`, or 1% when
   `fee` is not set. Donation and fee are each capped at 10% by the share
   chain, with no combined cap; the fee is taken from what the donation
   leaves.
3. **PPLNS payouts,** smallest weight first, ties by locking script, then
   the payout tree output if some payees do not fit (below). A miner is
   paid at the CashAddr in its Stratum username, P2PKH or P2SH
   ([Miner options](../miner-options.md)). If the share's payout right has
   been sold, its weight is paid to the right's holder instead
   ([Trading shares for BCH](../atomic-swap/bch-share-trading.md#payout-rights)).
4. **Merge-mining commitment,** a zero-value output for the tokens the miner
   merge-mines, the exact empty commitment if none. It is output 0, before
   the donation ([Auxiliary work proofs](../bch-auxpow.md)).
5. **Padding and share commitment,** zero-value outputs that tie the share to
   this coinbase ([Share commitments](../sharechain/share-commitments.md)).

There is no SegWit witness commitment on Bitcoin Cash.

## How many payouts

A share whose merge-mining commitment is the exact empty one, as on every
pool-built job, pays every payee directly: only Bitcoin Cash's
1,000,000-byte transaction limit bounds its coinbase, about 29,400 P2PKH
payouts. A share that merge-mines tokens keeps its coinbase within 3,072
bytes (77 P2PKH payouts), so a token covenant can hash it. The payee whose
output would pass the limit and every payee after it are paid through one
payout tree output: keyless covenant transactions of up to 500 outputs each
that anyone can broadcast once the coinbase is mature, paying each payee
exactly, less its share of the fee at 1 to 3 sat/byte. Every P2Pool node
broadcasts the trees of its found blocks at 1 sat/byte, and the API serves
them (`/payout_trees`, `/payout_tree?coinbase_txid=`) so payees can broadcast
them without P2Pool. Full rule:
[Payout distribution](../architecture/payout-distribution.md) and
[Hybrid payouts on Bitcoin Cash](../architecture/bch-hybrid-payouts.md); limits
on [Pool size limits](../sharechain/pool-size.md#the-hybrid-payout).

## Small payouts on Bitcoin Cash

A payout is spent like any other Bitcoin Cash output: at about 1 satoshi per
byte, moving it costs about two hundred satoshis. Upstream's need to keep tiny
payouts off the chain, or to move them over Lightning, does not apply. A
direct payout below the dust threshold (546 satoshis for P2PKH) is still
paid, since a coinbase may hold any value; in the payout tree, whose
transactions are relayed, a payee below it is merged into the largest. What
a small miner can expect per block is worked out on
[Small miners](../small-miners.md).
