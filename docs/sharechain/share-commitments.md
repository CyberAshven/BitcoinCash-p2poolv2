# Share commitments

> Ported from upstream P2Poolv2's documentation:
> `architecture/share_commitment.adoc` in
> [p2poolv2/docs](https://github.com/p2poolv2/docs) at commit `623e4c1`,
> Copyright 2024-2026 P2Poolv2 Developers, MIT or Apache-2.0. Adapted to
> Bitcoin Cash by this fork; see the [port map](../upstream-port-map.md).

Each share is committed to in the Bitcoin Cash coinbase, which the Bitcoin
Cash header commits to through its merkle root, and that header is embedded
in the share header. The commitment serves two goals:

1. Each proof of work can build only one share.
2. Each share coinbase is unique, even among shares mined on the same block
   template.

## What is committed

The share commitment is a SHA-256 hash of the share header's fields and the
share transaction root (`ShareCommitment::hash` in
`p2poolv2_lib/src/shares/share_commitment.rs`):

- previous share block hash, uncles, `bits`, time;
- donation address and basis points, fee address and basis points;
- the share transaction root;
- on a declared Bitcoin Cash job, the miner's scriptSig prefix and
  merge-mining commitment;
- the miner payout address (with its network class) and the miner share
  address.

The **share transaction root** is a merkle root over the share's
non-coinbase transactions, with the coinbase's leaf set to zero. The code
calls it `share_witness_root`, a name it keeps from upstream, where Bitcoin
share transactions carry witnesses. Bitcoin Cash transactions have no
witness, so on Bitcoin Cash each leaf is simply the transaction id.

The commitment sits in the last output of the Bitcoin Cash coinbase, as
`OP_RETURN` followed by the 32-byte hash. The coinbase's transaction id
feeds the header's merkle root, and the miner hashes that header. So the
committed fields fix the block hash, and one proof of work can stand behind
only one share: the first goal.

The share coinbase's scriptSig carries the weak block hash. Each proof of
work has its own nonce and so its own block hash, which makes each share
coinbase unique: the second goal.

The share coinbase itself cannot be in the commitment, because the block hash
exists only once the work is done. The commitment binds the miner's share
address and the share transaction root instead, and with those fixed, the
header alone determines the share coinbase: one output of one share unit to
the miner's P2PKH script. On Bitcoin Cash the share coinbase has no witness
commitment output (`build_sharechain_coinbase_for_witness_root` in
`shares/transactions/coinbase.rs`).

```text
Share header fields + share transaction root
        |
        |  SHA-256
        v
Share commitment --> last output of the Bitcoin Cash coinbase
                         |
                         |  coinbase txid, folded up the merkle branch
                         v
                     merkle root in the Bitcoin Cash header
                         |
                         |  the miner hashes the header
                         v
                     weak block hash (the proof of work)
                         |
                         v
                     scriptSig of the share coinbase
```

## Checking the commitment from the header

P2Poolv2 uses the commitment trick of the original P2Pool to tie the share
header to the Bitcoin Cash coinbase. A node checks the tie from the header
alone, before it has the body and without reading the PPLNS window from its
store.

The serialized coinbase is split into a prefix and a tail:

```text
Bitcoin Cash coinbase, serialized
+-- prefix: a whole number of 64-byte SHA-256 blocks ------------------+
|  version                                                             |
|  input: height, aux flags, extranonce, timestamp, pool signature     |
|  outputs: donation, fee, PPLNS payouts                               |
|  output: merge-mining commitment                                     |
|  output: padding (OP_RETURN, 0 to 63 bytes)                          |
+----------------------------------------------------------------------+
+-- tail: 47 bytes ----------------------------------------------------+
|  output: share commitment (8-byte value, 1-byte length, 34 bytes)    |
|  locktime (4 bytes)                                                  |
+----------------------------------------------------------------------+
```

The padding brings the prefix to a whole number of 64-byte SHA-256 blocks,
so SHA-256 can hash the prefix and pause. The state it pauses in, the
midstate, stands in for the whole prefix. The miner puts the midstate and the
prefix length in the share header's coinbase proof, beside the share
transaction root.

A node rebuilds the tail from the header: the commitment output from the
share commitment, and the locktime from the Bitcoin Cash height (height
minus one). It then checks the proof in three steps (`CoinbaseProof::verify`
in `shares/coinbase_proof.rs`):

1. Rebuild the share commitment from the header fields and the share
   transaction root.
2. Resume SHA-256 from the midstate over the commitment output and the
   locktime, then hash again to get the coinbase transaction id.
3. Fold the coinbase merkle branch into the transaction id, and require the
   result to equal the header's merkle root.

The proof of work fixes the merkle root, so a forged proof needs a SHA-256
collision, and one Bitcoin Cash header can stand behind only one share. The
check is part of [share validation](validation.md).

## Bitcoin Cash notes

- **No witness commitment.** Bitcoin coinbases carry a BIP141 witness
  commitment output inside the prefix. BCHN's templates have none, and this
  fork drops a template that asks for Bitcoin's SegWit rules
  (`stratum/work/block_template.rs`).
- **Branch length.** Blocks can grow under ABLA, so the coinbase merkle
  branch may be longer than on Bitcoin. The decoder accepts up to 32 levels,
  a block of up to 2^32 transactions, since Bitcoin Cash has no fixed maximum
  (`MAX_COINBASE_MERKLE_BRANCH_LENGTH`); Bitcoin keeps 16. A merge-mining
  proof keeps its own 25 ([Auxiliary work proofs](../bch-auxpow.md)).
- **The merge-mining output** sits inside the prefix, so the midstate covers
  it; see [Auxiliary work proofs](../bch-auxpow.md).
