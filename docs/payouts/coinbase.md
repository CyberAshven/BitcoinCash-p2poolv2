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
3.125 BCH (fees left out), and the node keeps the default 1.5% donation and
no operator fee:

```text
PPLNS window, newest first:  B  A  C  A  B  A

Bitcoin Cash coinbase, 3.125 BCH:
  donation, 1.5%                      0.046875   BCH
  Miner A, 3 of 6 shares (50%)        1.5390625  BCH
  Miner B, 2 of 6 shares (33.3%)      1.02604166 BCH
  Miner C, 1 of 6 shares (16.7%)      0.51302084 BCH
  merge-mining commitment             0
  padding                             0
  share commitment                    0
```

Amounts are split in whole satoshis: each payee's part is rounded down and the
last address in order gets the remainder. The example leaves out uncles; how
they are paid is on
[PPLNS accounting](../sharechain/pplns-accounting.md#uncles).

## Coinbase outputs

In order:

1. **Donation.** 1.5% of the block by default, to the project's fixed key
   (plan decision 9). An operator can raise, lower or turn it off.
2. **Operator fee,** if the node sets `fee_address` and `fee`. Donation and
   fee together are capped at 10% by the share chain.
3. **PPLNS payouts,** sorted by address. A miner is paid at the CashAddr
   in its Stratum username, P2PKH or P2SH
   ([Miner options](../miner-options.md)). If the share's payout right has
   been sold, its weight is paid to the right's holder instead
   ([Trading shares for BCH](../atomic-swap/bch-share-trading.md#payout-rights)).
4. **Merge-mining commitment,** a zero-value output for the tokens the miner
   merge-mines; empty if none ([Auxiliary work proofs](../bch-auxpow.md)).
5. **Padding and share commitment,** zero-value outputs that tie the share to
   this coinbase ([Share commitments](../sharechain/share-commitments.md)).

There is no SegWit witness commitment on Bitcoin Cash.

## How many payouts: an open design question

**Today** (plan decision 14) a Bitcoin Cash coinbase carries at most 11
payouts, in at most 784 bytes, and no payout below BCHN's dust threshold. If
the window's payees fit, every one is paid as above. If not, the largest are
paid their exact share and the rest of the amount goes to one of the remaining
payees, drawn by weight from the share's parent hash, so each one's expected
income is still its share. Full rule:
[Payout distribution](../architecture/payout-distribution.md#bitcoin-cash-coinbase-budget).

**Being redesigned.** The cap came from upstream's report about stock ASIC
firmware on Bitcoin and is being replaced by a hybrid: direct payouts to every
miner with no cap for shares that merge-mine no tokens, and, for shares that
do, a coinbase kept within a threshold with the payees past it paid through a
covenant payout tree. The threshold, the rule for the payee that straddles
it, and the tree itself are not decided yet. See
[Pool size limits](../sharechain/pool-size.md#open-design-question-the-hybrid-payout).

## Small payouts on Bitcoin Cash

A payout is spent like any other Bitcoin Cash output: at about 1 satoshi per
byte, moving it costs about two hundred satoshis. Upstream's need to keep tiny
payouts off the chain, or to move them over Lightning, does not apply. The
dust threshold (546 satoshis for P2PKH) is the floor; what a small miner can
expect per block is worked out on [Small miners](../small-miners.md).
