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
3.125 BCH (fees left out). Miners B and C keep the default 1% donation, miner
A chose `donation=0`, and no node takes an operator fee. Each share's
donation comes off that share's own part, whoever finds the block:

```text
PPLNS window, newest first:  B  A  C  A  B  A

Bitcoin Cash coinbase, 3.125 BCH:
  merge-mining commitment                 0
  donation, 1% of B's and C's shares      0.01562500 BCH
  Miner C, 1 of 6 shares, less 1%         0.51562500 BCH
  Miner B, 2 of 6 shares, less 1%         1.03125000 BCH
  Miner A, 3 of 6 shares                  1.56250000 BCH
  padding                                 0
  share commitment                        0
```

Amounts are split in whole satoshis: each payee's part is rounded down and the
largest payee gets the remainder. The example leaves out uncles; how
they are paid is on
[PPLNS accounting](../sharechain/pplns-accounting.md#uncles).

## Coinbase outputs

In order:

1. **Merge-mining commitment,** a zero-value output for the tokens the miner
   merge-mines, the exact empty commitment if none. It is output 0
   ([Auxiliary work proofs](../bch-auxpow.md)). On a node that merge-mines
   tokens it is the root of the miner's own token leaves, whose split
   carries the miner's donation and the node's fee for each token win.
2. **PPLNS payouts,** smallest weight first, ties by locking script, then
   the payout tree output if some payees do not fit (below). A miner is
   paid at the CashAddr in its Stratum username, P2PKH or P2SH
   ([Miner options](../miner-options.md)). If the share's payout right has
   been sold, its weight is paid to the right's holder instead
   ([Trading shares for BCH](../atomic-swap/bch-share-trading.md#payout-rights)).
   The project's donation key and node operators' fee addresses are payees
   like any miner:
   - **Donation.** Each share records the donation its miner chose with
     `donation=` (0 to 10% in steps of 0.5%,
     [Miner options](../miner-options.md#donation)), or its node's
     `donation`, 1% by default, when the miner chose none. That part of the
     share's own weight is paid to the project's fixed key (plan decision 9).
   - **Operator fee.** Each share records the fee of the node that built
     it, if that node sets `fee_address`: `fee`, or 1% when `fee` is not
     set, taken from what the share's donation leaves and paid to that
     node's fee address.
   Donation and fee are each capped at 10% by the share chain, with no
   combined cap. They come off each share's own weight only: the share that
   finds the block cannot charge them to the rest of the window.
3. **Padding and share commitment,** zero-value outputs that tie the share to
   this coinbase ([Share commitments](../sharechain/share-commitments.md)).

There is no SegWit witness commitment on Bitcoin Cash.

## How many payouts

A share that merge-mines no tokens pays every payee directly, up to Bitcoin
Cash's 1,000,000-byte transaction limit: about 29,400 P2PKH payouts. A share
that merge-mines tokens keeps its coinbase within 3,072 bytes (79 P2PKH
payouts). Past the limit, the largest payees are paid through one payout
tree output: keyless covenant transactions that every P2Pool node
broadcasts once the coinbase is mature, paying each payee exactly, less its
share of a fee of 1 to 3 sat/byte. The API serves them
(`/payout_trees`, `/payout_tree?coinbase_txid=`), so a payee can broadcast
them without P2Pool. Full rule:
[Hybrid payouts on Bitcoin Cash](../architecture/bch-hybrid-payouts.md);
limits on [Pool size limits](../sharechain/pool-size.md#the-hybrid-payout).

## Small payouts on Bitcoin Cash

A payout is spent like any other Bitcoin Cash output: at about 1 satoshi per
byte, moving it costs about two hundred satoshis. Upstream's need to keep tiny
payouts off the chain, or to move them over Lightning, does not apply. A
direct payout below the dust threshold (546 satoshis for P2PKH) is still
paid, since a coinbase may hold any value; in the payout tree, whose
transactions are relayed, a payee below it is merged into the largest. What
a small miner can expect per block is worked out on
[Small miners](../small-miners.md).

## Payout levels

Every Bitcoin Cash share carries its miner's payout level `k`, 1 to 7, the
smallest payment the miner wants: `L_k = 546 * 2^k` satoshis, from 1,092 to
69,888 ([Miner options](../miner-options.md#payout-level)). A payee's level
is that of its newest credited entry in the window. A payee whose amount in
a block is below its level is pooled with the others at that level, and the
pot is paid by a draw among them, in parts of at least the level, within
the same block, so nothing is held and nothing is owed. The donation key and
fee addresses are never drawn, and a payee that wins nothing gets no output
in that block. The draw is seeded from the paying share's parent hash, so
every node draws the same way and the share that pays cannot change it. The
exact steps are on
[Share chain redesign](../sharechain/small-miners-design.md#4-payout-levels);
the rule's tag is `levels-546x2-k1`. Bitcoin Cash sees only fewer, larger
coinbase outputs.

Each level's pot is paid exactly, and each payee's expected payment is its
amount, up to a few satoshis. A miner at a high level is paid less often and
more each time: at level 5 a miner earning 1,000 satoshis a block wins a
17,472-satoshi payment about once in 17 blocks.

For example, three payees at level 3 with 9,000 satoshis between them (`S`)
share `n` = 2 payments of `D` = 4,500, placed by the seed along their
running totals:

```text
level 3 (4,368 sat), S = 9,000, n = 2, D = 4,500, seed mod D = 1,200

  payee   amount   running total   points 1,200 and 5,700
  X       2,000    2,000           1,200 -> X wins 4,500
  Y       3,000    5,000
  Z       4,000    9,000           5,700 -> Z wins 4,500
```
