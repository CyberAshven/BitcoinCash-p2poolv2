# Share chain

> Ported from upstream P2Poolv2's documentation: `architecture/sharechain.adoc`
> in [p2poolv2/docs](https://github.com/p2poolv2/docs) at commit `623e4c1`,
> Copyright 2024-2026 P2Poolv2 Developers, MIT or Apache-2.0. Adapted to
> Bitcoin Cash by this fork; see the [port map](../upstream-port-map.md).

A share chain is a blockchain of share blocks, and each share block embeds a
Bitcoin Cash block header. The share chain's difficulty is far below Bitcoin
Cash's, so the share chain is a chain of "weak" Bitcoin Cash blocks: one
every 10 seconds on each share chain, against one Bitcoin Cash block every
10 minutes.

## Share header and Bitcoin Cash header

The two `bits` fields hold one target each: the Bitcoin Cash header's `bits`
for the Bitcoin Cash block, and the share header's `bits` for the share
chain.

```text
Share block n
+-- Share header ---------------------------------------+
|  previous share block hash  ----------------------->  share block n-1
|  bits: share chain target                             |
|  time                                                 |
|  (other fields: see the table below)                  |
|  +-- Bitcoin Cash header ----------------------------+|
|  |  version                                          ||
|  |  previous Bitcoin Cash block hash                 ||
|  |  merkle root                                      ||
|  |  time                                             ||
|  |  bits: Bitcoin Cash target                        ||
|  |  nonce                                            ||
|  +---------------------------------------------------+|
+-------------------------------------------------------+

Proof of work = double SHA-256 of the Bitcoin Cash header
  meets the share chain target  -> a valid share
  also meets the BCH target     -> a Bitcoin Cash block
```

## Share block format

A share block has a header and a body. The share block hash covers every
header field and none of the body. Each header field is fixed by the proof
of work, so one proof of work can stand behind only one share block hash.

| Field | Contents | Fixed by |
|---|---|---|
| Previous share block hash | Hash of the parent share block | Share commitment |
| Uncles | Hashes of up to three uncle share blocks | Share commitment |
| Miner payout address | The Bitcoin Cash address that receives the miner's payouts (`miner_bitcoin_address`), a CashAddr P2PKH or P2SH script, with its network class | Share commitment |
| Miner share address | The share owner (`miner_address`): on Bitcoin Cash a 20-byte P2PKH key hash, written as a `p2pool:` CashAddr | Share commitment |
| Bitcoin Cash header | The weak block header the miner hashed | The proof of work itself |
| `bits` | Share chain target in compact form | Share commitment |
| Time | Share timestamp set by the miner | Share commitment |
| Donation and fee | Donation and operator fee, each an address and basis points | Share commitment |
| Bitcoin Cash height | Height of the block being mined | The coinbase locktime |
| Coinbase proof | SHA-256 midstate of the coinbase prefix, the prefix length, and the share transaction root | The Bitcoin Cash header's merkle root |
| Declared coinbase (Job Declaration only) | The miner's scriptSig prefix and merge-mining commitment | Share commitment |

The share commitment and the coinbase proof are described on
[Share commitments](share-commitments.md). The two addresses are separate
keys on separate chains: never derive one from the other
([Share chain address format](../architecture/address-format.md#bitcoin-cash)).

The body holds what the proof of work fixes but the header cannot check by
itself:

- **Share transactions.** The share coinbase, then the share's other
  transactions, whose root the coinbase proof carries.
- **Coinbase merkle branch.** The path from the Bitcoin Cash coinbase to the
  header's merkle root. Shares mined on one template share a branch.
- **Bitcoin Cash coinbase.** The coinbase transaction the miner hashed. On a
  node's own jobs its scriptSig carries the block height, the template's
  `coinbaseaux` flags, the extranonce, a nanosecond timestamp and the pool
  signature; a declared job's scriptSig is the miner's own prefix followed by
  the extranonce. Either way it stays within Bitcoin Cash's 100-byte limit.
  Its outputs carry the merge-mining commitment (output 0), the donation,
  the fee, the payouts, a padding output and the share commitment. Bitcoin
  Cash has no SegWit, so there is no witness commitment output.

A node checks the body against the header before storing it; see
[Share validation](validation.md#the-body-matches-the-header).

## Share chain and Bitcoin Cash chain

A node hashes the Bitcoin Cash header and compares the hash with the share
header's `bits` to check that the share meets the share chain target, and
with the Bitcoin Cash header's `bits` to see whether the share is also a
Bitcoin Cash block it can broadcast.

Several shares can build on the same Bitcoin Cash block. Below, share 3's
proof of work met the Bitcoin Cash target, so it became block 101:

```text
Share chain:   S1 <- S2 <- S3 <- S4 <- S5 <- S6
               |     |     |     |     |     |
builds on:    100   100   100   101   101   102

Bitcoin Cash:  100 <- 101 <- 102
                      ^
                      S3's header became block 101
```

## The share chain agrees on the payouts

A share chain gives every node Nakamoto consensus on the weak shares (the
share chain's own agreement; Bitcoin Cash consensus is untouched). All
nodes converge on the same chain, so they compute the same payouts when they
build shares, and every node can check that every share pays out honestly.
A node rebuilds each share's coinbase from the PPLNS window anchored at the
share's parent and rejects a share whose coinbase differs. See
[PPLNS accounting](pplns-accounting.md) and [Share validation](validation.md).

## A share has a value

When the pool finds a block, every share in the PPLNS window earns part of
it. A share therefore earns BCH if the pool finds blocks while it is in the
window, which makes it behave like an option with a known expiry. What a
buyer can read to value one:

1. Its place in the window, and how long it has left there.
2. The share difficulty it was mined at: the share header's `bits`.
3. The Bitcoin Cash difficulty: on Bitcoin Cash the window holds 3 blocks'
   worth of work, sized by the paying block's own difficulty, so a share's
   slice of each block depends on it.
4. Its height in the share chain.

On Bitcoin Cash what earns is the share's **payout right**, which starts with
its miner and can be sold for BCH
([Trading shares for BCH](../atomic-swap/bch-share-trading.md)). Pricing a
share is an open question; see [Selling shares](../payouts/selling-shares.md).

## The share chain has transactions

The share chain has UTXO and script based transactions. Each share's
coinbase pays its miner one share unit, and on Bitcoin Cash that output
carries the share's payout right. With transactions, a miner can sell a
share for BCH through a contract on each chain. Bitcoin Cash share
transactions accept only P2PKH spends and that contract. See
[Share transactions](transactions.md).
