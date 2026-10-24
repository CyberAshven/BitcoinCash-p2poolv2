# BCH auxiliary work proofs

Every Bitcoin Cash coinbase P2Pool builds carries a merge-mining commitment
as output 0, in the **merge-mining common format v2** agreed with Pickaxe,
so both build and check the same bytes. `shares::merge_mining` holds the
commitment, `shares::auxpow` the token leaves, the tree and `AuxProof` v2,
and `shares::bch_asert` the ASERT header check. Pickaxe's specification is
[`docs/merge-mining.md`](https://github.com/CyberAshven/pickaxe-miner/blob/feat/merge-mining-v2/docs/merge-mining.md)
in pickaxe-miner (PR #70). P2Pool's unit tests pin those vectors to commit
[`a1c80a1ba6ad6b1542af54f4af28058d3241c3df`](https://github.com/CyberAshven/pickaxe-miner/commit/a1c80a1ba6ad6b1542af54f4af28058d3241c3df):
the leaf, the two-token root
`5b61ecfe96c714397bc35ed813890f49f2b0287a78111e19d156f4eecd1c38d0`,
the 53-byte output 0, the exact empty commitment, the 224-byte proof and
the split leaf
all match byte for byte. `shares::auxpow::tests` checks proof encoding and
verification, including rejection when the expected state hash changes;
`shares::merge_mining::tests` checks both commitment outputs.

This replaces the earlier format, a `fabe6d6d` output after the payouts with
Namecoin's slot scheme and the whole coinbase in each proof (audit items
A16 and A15). It is a share-chain rule: it changes the coinbase every BCH
share carries, not BCH consensus.

All integers are little-endian. `SHA256` is single SHA-256 and `HASH256`
double SHA-256. Hashes are written as hashed, never byte-reversed.

## Output 0

```text
off len value
0   8   00 00 00 00 00 00 00 00   value 0
8   1   2c                        script length 44
9   1   6a                        OP_RETURN
10  1   2a                        push 42
11  4   43 54 4d 4d               "CTMM"
15  1   02                        version
16  32  root
48  1   h                         tree height, 0 to 16
49  4   salt                      u32 LE
```

**The exact empty commitment** is the zero root with `h = 0` and salt 0. It
says the coinbase merge-mines no token. A zero root with any other `h` or
salt is not a commitment, and `h` above 16 is not either; P2Pool refuses
both (`MergeMiningCommitment::validate`).

Pool-built jobs carry the empty commitment when the node has no tokens to
merge-mine. When tokens are configured, the node builds a root from each
miner's own token leaves. A miner can also declare a token commitment in
its own coinbase through [Job Declaration](bch-job-declaration.md). The
share header carries the root, `h` and salt in `pool_merge_mining` for a
pool-built share or in `bch_coinbase` for a declared share. Every peer
rebuilds output 0 from that commitment byte for byte when it rebuilds the
coinbase.

A share is a **token share** when its commitment is anything but the exact
empty one (`share_has_token_commitment`), and a **no-token share**
otherwise. The [hybrid payout](architecture/bch-hybrid-payouts.md) treats
the two differently.

A coinbase's head is fixed-shape, so a covenant finds output 0 without a
search:

```text
version (4) | 01 | 00 x 32 | ffffffff | L (1, at most 100) | scriptSig (L) | sequence (4) | output count (1, 1..=0xfc)
```

Output 0 is `cb[47+L .. 100+L]`.

## The tree

- **Slot:** `u32le(SHA256(0x02 ‖ salt_le4 ‖ category32)[0..4]) mod 2^h`.
- **Leaf hash:** `SHA256(0x00 ‖ preimage)`, the 169-byte preimage below.
- **Node:** `SHA256(0x01 ‖ left ‖ right)`. At level `i`, bit `i` of the
  slot set means the current node is the right child.
- An empty leaf slot is 32 zero bytes; internal nodes are always hashed
  from their children. One leaf per token category, at most 16 tokens.

The builder (`AuxiliaryTree::build`) starts at the smallest `h` that holds
the leaves, tries salts 0 to 65,535 at each `h`, then the next `h`, up to
16, exactly as Pickaxe's builder does, so the same tokens give the same
commitment in both. No tokens give the empty commitment. A verifier accepts
any salt.

Leaf preimage (`TokenLeaf::preimage`):

```text
off len field
0   4   "CTML"
4   1   02
5   32  category      the token category
37  32  lock_hash     HASH256(the token baton's locking bytecode)
69  32  state_hash    token-defined; changes only on a successful claim
101 32  payout_hash   HASH256(the payout output's locking bytecode)
133 4   target_bits   the token's compact target, u32 LE
137 32  ext           the token's split (below), or 32 zero bytes
```

**The split** (`TokenSplit`) shares each win between the miner, a donation
and a fee, in that order:

```text
ext = 00 x 32                                                  no split
ext = SHA256(03 || d_bps u16le || d_hash 32 || f_bps u16le || f_hash 32)
```

The 69-byte preimage is canonical: each rate is 0 to 10,000 hundredths of a
percent, a rate of 0 has a zero hash and a rate above 0 a non-zero one, and
two empty slots are the zero `ext`, never a preimage. A hash is `HASH256` of
the recipient's locking bytecode, as `payout_hash` is. The claim reveals the
preimage and the token's covenant pays the shares; how the shares are
computed is a token rule, not P2Pool's. Pickaxe's split leaf (token A with a
1% donation to P2PKH(`d0` x 20) and a 1% fee to P2PKH(`fe` x 20): `ext`
`79f31f39…8faa85`, leaf hash `c4027136…033e41`) matches byte for byte.

## `AuxProof` v2

```text
off  len    field
0    4      "CTMP"
4    1      02
5    80     header
85   1      h (at most 16)
86   4      salt, u32 LE
90   32h    aux branch, leaf to root
..   2+n    u16 LE n, then the coinbase head (47 + L bytes)
..   2+n    u16 LE n, then the coinbase tail (outputs 1.. and the locktime)
..   1      d (at most 25)
..   32d    coinbase branch, the coinbase at index 0
```

Output 0 and the leaf are never carried: the verifier rebuilds the leaf from
the token's own state and output 0 from the root it folds. The encoding is
canonical: exact lengths, nothing trailing.

The `d ≤ 25` limit (2^25 transactions) belongs to the proof format, as
agreed with Pickaxe. P2Pool's own coinbase proofs between share-chain nodes
are not limited by it: a BCH block deeper than 25 levels is still a valid
share, it just cannot be token-claimed.

`AuxProof::verify` checks, for a leaf the token's rules supply:

1. the shape limits;
2. the slot from the category, the salt and `h`, with `h` at most the
   token's own limit;
3. the aux branch folds the leaf hash to a root;
4. output 0 rebuilt from that root, `h` and the salt;
5. the coinbase head has the fixed shape and the tail holds a locktime;
6. `HASH256(head ‖ output 0 ‖ tail)`, folded up the coinbase branch, is the
   header's merkle root;
7. the header's hash meets the token's target, never the header's own
   nBits; `target_bits` must be canonical compact (`3 ≤ exp ≤ 32`,
   `0 < mantissa < 0x800000`);
8. with a network that has an ASERT anchor, the ASERT header check below.

This checks merge-mined work. It does not check token issuance rules or that
the parent is a complete valid BCH block; BCHN is the authority on that.

## The ASERT header check

A token would otherwise accept any SHA-256 work whose header passes these
checks. To tie the header's difficulty to Bitcoin Cash's, the header's
nBits must be what BCH's aserti3-2d gives at the coinbase's BIP34 height,
within a band (`BchAsert::check_header_bits`):

- mainnet: `computed / 2 ≤ header_target ≤ computed × 1.05`;
- test networks: `computed / 8 ≤ header_target ≤ computed × 1.05`, or nBits
  exactly the proof-of-work limit's (`0x1d00ffff`, a minimum-difficulty
  block).

`computed` is ASERT's target for a parent at `height - 1` dated with the
header's own time, which stands in for the parent's (a proof has no parent
header). A header minted hours after its parent is harder than `computed`,
hence the wide hard side; a header's time can fall before its parent's only
a little, hence the narrow easy side. The bands were agreed with Pickaxe.

The arithmetic follows `CalculateASERT` and `GetNextASERTWorkRequired` in
BCHN's `src/pow.cpp` integer for integer: a truncating 64-bit division for
the exponent, an arithmetic (flooring) shift for its integer part, BCHN's
cubic for the fraction, then the clamps. The anchors and half-lives are
BCHN's chain parameters (`src/chainparams.cpp`):

| Network | Anchor height | Anchor nBits | Anchor parent time | Half-life |
|---|---|---|---|---|
| Mainnet | 661,647 | `0x1804dafe` | 1,605,447,844 | 172,800 s |
| Chipnet and testnet4 | 16,844 | `0x1d00ffff` | 1,605,451,779 | 3,600 s |
| Scalenet and regtest | no anchor | | | the check is skipped |

Tested against:

- every line of the aserti3-2d specification's twelve test vector runs
  (14,000 lines), from BCHN's QA assets
  ([`test_vectors/aserti3-2d`](https://gitlab.com/bitcoin-cash-node/bchn-sw/qa-assets/-/tree/master/test_vectors/aserti3-2d),
  MIT, vendored unchanged beside `shares/bch_asert.rs`);
- nine real mainnet and nine real chipnet header pairs, fetched from public
  Fulcrum servers on 2026-10-23, from the first ASERT blocks to the tips
  (mainnet 974,001, chipnet 329,001), including chipnet's minimum-difficulty
  block 300,001: each child carries exactly the nBits ASERT gives from its
  parent, and passes the header check with only its own header.

**Where P2Pool runs it.** On every proof `AuxProof::verify` checks with a
network, and on every **token share's** bitcoin header, at the header gates
(`validate_header_minimum_difficulty`), with the node's
`bitcoin_cash_network`. A token claim's covenant checks the same band, so a
token share outside it could not be claimed anyway. No-token shares are not
checked: BCHN alone judges their blocks. On a test network, a parent dated
more than about four minutes after the share's header time (one-hour
half-life, 1.05 band) fails the easy side; such a token share is refused.

## Offline CLI

No node configuration or wallet is required. Given `tokens.json`:

```json
{"tokens": [{
  "category": "1111111111111111111111111111111111111111111111111111111111111111",
  "lock_hash": "953ccfa596a6c6d39e5980194539124fdcff116a571455a212baed811f585ee0",
  "state_hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  "payout_hash": "22650758a42ebfd7306e771ad24c442cad795ed9e549392233729f647a2b1be5",
  "target_bits": "207fffff",
  "ext": "0000000000000000000000000000000000000000000000000000000000000000"
}]}
```

```sh
p2poolv2_cli bch-auxpow build tokens.json
p2poolv2_cli bch-auxpow verify proof.hex token.json --network chipnet
```

`build` prints the commitment, output 0 in hex and each token's slot and
branch. `verify` takes the proof's bytes in hex and one token's leaf fields
as JSON (the same object as above), with `--max-height` for the token's
tree limit (16 by default) and `--network` for the ASERT check (regtest
skips it). Instead of `ext`, a token may give its split, from which `ext`
is computed: `"split": {"donation_bps": 100, "donation_script": "<hex>",
"fee_bps": 100, "fee_script": "<hex>"}`, scripts being locking bytecode
(left out at a rate of 0); without either, `ext` is 32 zero bytes. Each
input file is limited to 1 MiB. The token above is Pickaxe's synthetic
token A, not a real token.

## The SAFA covenant

The experimental [SAFA AuxPoW covenant](../tools/safa-auxpow/README.md)
verifies v2 proofs inside the BCH 2026 VM. It rebuilds the leaf from
introspection (the vault's category, locking bytecode and state, the
claim's payout and target, and the split's `ext`), folds both branches,
checks the header's work against the token's target and its nBits against
ASERT at the coinbase's BIP34 height (the band above, with a fallback after
4,032 blocks without a claim), binds that height with
`OP_CHECKLOCKTIMEVERIFY`, and pays the split. Its tests run Pickaxe's
vectors, BCHN's 14,000 aserti3-2d vectors and the real headers from this
repository's Rust tests in the VM. A chipnet test token (tSAFA) is
prepared but not deployed. The v1 variant passed a real regtest withdrawal
on 2026-10-16; the v2 covenant has not yet run against BCHN. Its separate
checks still apply:

SAFA is the first requested token target. Its
[author's draft and linked Bitauth template](https://bitcoincashresearch.org/t/safas-a-sha256-asic-minable-automated-token-market/2123)
check `SHA256d(next NFT baton)` against a difficulty target, with no parent
header, coinbase or inclusion branch. Committing its baton hash in BCH does
not satisfy that mining rule. The variant keeps the draft's threads, market
and difficulty adjustment and replaces the mining hash check with a
merge-mining check. It is not a deployed version of the author's token or a
final SAFA specification. Independent review, the economic parameters (the
virtual reserve among them) and a final SAFA policy remain open.

The linked draft's release and swap scenarios both pass in Libauth
`3.1.0-next.8`'s standard BCH 2026 VM. This is not a security endorsement:
the release scenario's hash exceeds its unsigned target but passes the
script's direct `OP_BIN2NUM` comparison. A covenant must append a zero sign
byte before converting a 32-byte hash to an integer, and test the high-bit
case.

A token covenant must also enforce its own standard transaction and VM
budgets: the [2026 P2S rules](https://github.com/bitjson/bch-p2s#unlocking-bytecode-length)
allow 10,000 bytes of standard unlocking bytecode, including the redeem
script, and a v2 proof carries the whole coinbase but output 0.

## Regtest evidence, v1 format (2026-10-16)

Before v2, the opt-in Job Declaration test passed against two connected
BCHN 29.2.0 nodes with the v1 output. Both nodes accepted Full-Template
block 207 (`076b69a0…7bb9e`, two work items) and Coinbase-Only block 208
(`3f97953b…d5bef`, one item); each coinbase paid 2,500,000,219 satoshis,
including 219 satoshis in fees, and every exported proof verified. A SAFA
withdrawal using such a proof was accepted by both nodes in block 282; BCHN
rejected a copy with an altered merkle sibling. See
[the JD test setup](bch-job-declaration.md#two-node-bchn-regtest-check) and
the [variant's evidence record](../tools/safa-auxpow/README.md#regtest-evidence-and-reproduction).
The same opt-in test now builds and verifies v2 proofs; it has not been
rerun against BCHN since the change.
