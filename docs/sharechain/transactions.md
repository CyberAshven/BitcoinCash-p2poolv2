# Share transactions

> Ported from upstream P2Poolv2's documentation: `architecture/transactions.adoc`
> in [p2poolv2/docs](https://github.com/p2poolv2/docs) at commit `623e4c1`,
> Copyright 2024-2026 P2Poolv2 Developers, MIT or Apache-2.0. Adapted to
> Bitcoin Cash by this fork; see the [port map](../upstream-port-map.md).

Miners on the original P2Pool were paid only in the coinbase. Upstream
explains its share transactions by the limits that put on pool size on
Bitcoin: coinbase size, set by some firmware and by the price of block space.
On Bitcoin Cash, block space is not the constraint (blocks grow under ABLA
and fees are about 1 satoshi per byte); what limits the coinbase is discussed
on [Pool size limits](pool-size.md).

Upstream pays larger miners from the coinbase and lets smaller miners sell
their shares to miners or market makers, who then collect those shares'
payouts. To support that, and other payout schemes later, the share chain
has UTXO and script based transactions. This fork keeps them, with Bitcoin
Cash scripts: a miner can sell a share's payout right for BCH
([Trading shares for BCH](../atomic-swap/bch-share-trading.md)).

## Rules

Share transactions and share blocks follow Bitcoin Cash's transaction rules,
with these differences:

1. **Coinbase maturity** is 100 shares (on Bitcoin, upstream uses 6,048
   shares); see [PPLNS accounting](pplns-accounting.md).
2. **Size.** A share holds at most 100 transactions, and its body (its share
   transactions plus the Bitcoin Cash coinbase it carries) at most 200 KiB.
3. **The share coinbase** pays a fixed one share unit, with no subsidy and no
   fees.
4. **Outputs expire** by the height of the coinbase they descend from; see
   [Spending share outputs](spending-outputs.md).
5. **Scripts.** Only P2PKH spends, signed with `SIGHASH_FORKID` by ECDSA or
   Bitcoin Cash Schnorr, and the share-trading contract behind P2SH32 are
   accepted. Spending anything else, including P2SH20, multisig and
   token-prefixed outputs, is refused: stricter than Bitcoin Cash.
6. **Outputs** must be P2PKH or P2SH32 and at or above BCHN's dust threshold
   (546 units for P2PKH).
7. **Finality.** A transaction must be final at its share's height, so the
   contract's height locks work.
8. **Share owners** use their own address type; see below.

## The share coinbase and its value

Each share has a coinbase that pays the miner who found it one share unit,
10^8 of the share chain's smallest unit. The share commitment in the Bitcoin
Cash coinbase fixes it; see [Share commitments](share-commitments.md).

On Bitcoin Cash, the coinbase output also carries the share's **payout
right**: whoever holds the right when the pool finds a block is paid that
share's PPLNS weight. The units and the right are separate: units are
divisible down to the dust floor, while the right is whole and moves from
input `i` to output `i`. The full rule is in
[Trading shares for BCH](../atomic-swap/bch-share-trading.md#payout-rights).

## Share owner addresses

The share chain uses its own address type so that a share address is never
mistaken for a payout address. On Bitcoin Cash a share owner is a 20-byte
P2PKH key hash, written as CashAddr with its own prefix:

| Bitcoin Cash network | Share address prefix | Payout address prefix |
|---|---|---|
| Mainnet | `p2pool` | `bitcoincash` |
| Chipnet | `tp2pool` | `bchtest` |
| Regtest | `rp2pool` | `bchreg` |

`p2poolv2_cli bch-share-address` turns a wallet's P2PKH address into a share
address with the same key hash. Upstream's Bitcoin form (bech32m over a
Taproot key, with `p2pool`, `tp2pool`, `sp2pool` and `rp2pool`) is not used
on Bitcoin Cash. See
[Share chain address format](../architecture/address-format.md#bitcoin-cash).

## Fees

Upstream plans to pay share-transaction fees to the miner of the share that
includes them, with the fee output keeping its inputs' root height
([spending rules](spending-outputs.md)), and to raise fees with an output's
depth.

This fork does not. On Bitcoin Cash a transaction's fee is the share units it
burns, its inputs less its outputs, and nobody receives them. Nodes rank the
transactions they relay and mine by units burned per byte, first seen first
at equal rates. This is node policy, not a share-chain rule: a share may carry
any valid transaction, with or without a fee. Units carry no payout on
Bitcoin Cash (the payout right does), so a fee to the share's miner would hand
out units with no right attached. The reasoning is in plan decision 15
([bch-plan.md](../bch-plan.md)) and in
[Trading shares for BCH](../atomic-swap/bch-share-trading.md#why-a-priority-not-a-fee-rule).

## Code

Upstream builds transactions with `rust-bitcoin` and validates them with
`bitcoinconsensus`, and plans to move share scripts to `rust-miniscript`.
Neither is built for Bitcoin Cash: `bitcoinconsensus` applies Bitcoin's
script rules, so a Bitcoin Cash signature (`SIGHASH_FORKID`, Bitcoin Cash
Schnorr) would fail there. Bitcoin Cash share transactions are checked by
this fork's own interpreter (`p2poolv2_lib/src/shares/bch_script.rs`), and the
trade contract is built in `shares/bch_htlc.rs`. Changing what that
interpreter accepts is a share-chain rule change and moves the `trade-htlc`
tag in the protocol strings.

Nodes keep a bounded pool of signed share transactions, relay them and mine
them in the shares they build; `p2poolv2_cli transactions submit` or
`POST /share_transaction` hands one to a node. See
[Trading shares for BCH](../atomic-swap/bch-share-trading.md#share-transactions-on-the-network).

## Life of a share output

```text
Miner mines a share
  -> share coinbase output: one share unit, the payout right,
     root height = the share's height
  -> cannot be spent for 100 shares
  -> while the share is in the PPLNS window:
       the right's holder is paid the share's weight by each block found;
       the holder may sell the right for BCH (a share transaction)
  -> the share leaves the window: the right earns nothing more
  -> more than 127,008 shares after the root: the output can no longer be spent
```

Upstream ties a share output's earnings to its root height. On Bitcoin Cash
they follow the share's payout right instead, and the right earns only while
its own share is in the window, which is usually far shorter than the time
the output stays spendable.
