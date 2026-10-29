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
`shares::merge_mining::tests` checks both commitment outputs. The leaf
disclosure, its Stratum V1 and V2 transports and the Job Declaration leaf
rules follow PR #70 at
[`a92fef7`](https://github.com/CyberAshven/pickaxe-miner/commit/a92fef7),
"Disclosure" and "Job Declaration leaves", whose formats are unchanged from
`a1c80a1`'s vectors.

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
miner's own token leaves ([Merge-mining tokens on a public
node](#merge-mining-tokens-on-a-public-node)). A miner can also declare a
token commitment in its own coinbase through [Job
Declaration](bch-job-declaration.md). The share header carries the root,
`h` and salt in `pool_merge_mining` for a pool-built share (under its own
header tag, `0x23`, and only when the root is not the empty one) or in
`bch_coinbase` for a declared share, and the share commitment digests it,
so one proof of work stands behind one root. Every peer rebuilds output 0
from that commitment byte for byte when it rebuilds the coinbase, and checks
only its format: no peer knows the tokens behind a root, and share validity
never depends on them. The rule tag `mm2-roots-asert8` names this header
format with the ASERT band below (`8`: the test networks' eighth to eight
times).

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
within the agreed network band:

- mainnet: `computed / 2 ≤ header_target ≤ computed × 1.05`;
- test networks: `computed / 8 ≤ header_target ≤ computed × 8`, or nBits
  exactly the proof-of-work limit's (`0x1d00ffff`, a minimum-difficulty
  block).

`computed` is ASERT's target for a parent at `height - 1` dated with the
header's own time, which stands in for the parent's (a proof has no parent
header). A header minted hours after its parent is harder than `computed`,
hence the wide hard side. A test-network header can also predate its
parent by hours ([BCHN timestamp checks](https://gitlab.com/bitcoin-cash-node/bitcoin-cash-node/-/blob/master/src/validation.cpp)).
Chipnet's one-hour half-life makes a 5% easy side too
narrow, so its band permits up to x8 on both sides. Mainnet retains the
original asymmetric band. These are proof-format checks agreed with
Pickaxe, not BCH block validation.

Local share-block validation runs the same token-header gate as peer header
validation. The notifier drops tokens from a template outside the band.
Stratum V2 also checks a declared token job's nBits and requested minimum
time before proposal validation or job installation, returning
`invalid-token-asert` on failure. An exactly empty commitment does not need
this token-format check.

P2Pool's `BchAsert::check_header_bits` and the SAFA covenant use the same
bands, number for number: mainnet `target × 100 ≤ computed × 105`, test
networks `target ≤ computed × 8`, and `computed / d` floored on the hard
side. With a header dated two hours before its parent, which BCHN accepts
when the parent is dated ahead, the computed target is 2^(7200 / 172800),
about 1.029, times too hard on mainnet and 4 times on the test networks;
both fit their bands. A test runs real chipnet block 329,001 dated 200 s,
260 s, 10 minutes, one hour and two hours before its parent.

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
network, and on every **token share's** bitcoin header, with the node's
`bitcoin_cash_network`: at the header gate peers run
(`validate_header_minimum_difficulty`), and again in
`validate_share_block`, which a share the node mined itself reaches
without that gate. A node builds no token job on a template whose nBits
fail the band at its height and `curtime`; it mines that template without
tokens. No-token shares are not checked: BCHN alone judges their blocks.

## Merge-mining tokens on a public node

Owner's decision (2026-10-24): on a public P2Pool node the operator's fee
and the P2Pool donation apply to merge-mined tokens as they apply to BCH.
This is node behaviour, not a share-chain rule: peers check only the format
of a share's root, and share validity never depends on the tokens.

**The token file.** `[stratum] merge_mining_tokens` names a JSON file of
the tokens the node merge-mines (Bitcoin Cash in P2Pool mode only):

```json
{"tokens": [{
  "category": "1111111111111111111111111111111111111111111111111111111111111111",
  "lock_hash": "953ccfa596a6c6d39e5980194539124fdcff116a571455a212baed811f585ee0",
  "state_hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  "target_bits": "207fffff",
  "max_height": 16
}]}
```

Each token is its category, `HASH256` of its baton's locking bytecode, its
current state hash, its canonical compact target and, optionally, the
tallest tree its covenant accepts (16 when left out).

A SAFA vault gives its thread instead of `state_hash` and `target_bits`:
`"safa": {"threads": 63, "state": "<16-byte NFT commitment in hex>"}`.
The SAFA covenant rebuilds a claim's leaf with the state's target eased by
the blocks since the state (`min(dh, 288) / threads`), so a fixed target
would only match a claim 288 blocks after the last. The node therefore
computes each job's leaf from the thread, at the job's coinbase height:
`state_hash` is `HASH256` of the state and `target_bits` is the SAFA
tool's `claim_target_bits` (`p2poolv2_safa`), integer for integer
(`safa_claim_target_bits`; tests compare the two over every thread count
and pin vectors printed by the former Node tool's `claimTargetBits` and
`claimLeaf`). A template at or below the state's height is mined without
tokens.

At most 16 tokens,
one per category; the set must fit one tree no taller than the lowest
limit among them, or the file is refused. An empty list merge-mines
nothing. A node does not start on a file it cannot read or that is not
valid (at most 1 MiB). It reads the file again whenever its modification
time changes, so the tool that claims a token can write the new state
hash; a changed file that is broken keeps the last good tokens and logs
why. No token is deployed yet, so no file ships with the node.

**Each miner's root.** Every pool-built job (Stratum V1, and Stratum V2
standard and extended channels) carries a root of its miner's own leaves at
output 0 (`stratum::work::merge_tokens`):

- each leaf's `payout_hash` is `HASH256` of the miner's registered payout
  script, the address in their login;
- its `ext` commits to the split of each win: the **donation slot** is the
  miner's P2Pool donation (their `donation=` choice, or the node's default
  of 1%) to the P2Pool donation address, and the **fee slot** is the node's
  fee (1% by default) to its fee address, both at the rates and to the
  scripts the coinbase pays them, each with its own 10% cap. A rate of 0
  leaves its slot empty; with neither, `ext` is zero.

A node mining Pickaxe's token A for a miner with a 1% donation and a 1% fee
builds Pickaxe's whole split leaf byte for byte (leaf hash
`c4027136…033e41`). The layout (height, salt and slots) depends only on the
categories, so it is searched once per file and every miner's tree reuses
it; miners differ only in their root, which replaces a placeholder in the
coinbase every miner on a template shares. A job whose miner has no payout
address carries the empty commitment.

Every such share is a **token share**: its coinbase follows the token
share's 3,072-byte limit, so with more payees than fit the rest go through
the payout tree, and its bitcoin header takes the ASERT check above. A
share wins a token when its hash meets the token's target, whatever its
miner's own target; P2Pool does not ease a device's difficulty towards the
easiest token target as Pickaxe does, so a device whose difficulty is above
a token's target misses some of that token's wins.

## Disclosure to miners

A miner on someone else's pool checks before hashing that their leaves pay
them, and only the donation and fee they agreed to, and claims their own
wins. P2Pool discloses both in Pickaxe's formats, so miners and tools work
at both kinds of pool (`stratum::merge_disclosure`). Disclosure is opt-in:
firmware that does not ask sees nothing new.

**The leaves (`CTMD` v2),** the same bytes Job Declaration uses
(`shares::auxpow::Disclosure`):

```text
off len   field
0   4     "CTMD"
4   1     02
5   1     n, the leaves (0 to 16)
then n times:
    169   the leaf preimage
    1     00: ext is zero; 01: a split follows
    69    the split preimage (with 01 only)
```

A reader accepts exactly one disclosure (nothing trailing; each leaf and
split canonical; each split's SHA-256 equal to its leaf's `ext`), then
rebuilds the tree from these leaves alone at output 0's `h` and salt and
requires output 0's root: a leaf left out, added or changed gives another
root.

**Stratum V2.** A connection asks for extension `0x4D4D` through the
extensions negotiation (type `0x0001`, `RequestExtensions` `0x00`: `u16
request_id | u16 n | n x u16`), after `SetupConnection`; the node answers
`RequestExtensions.Success` (`0x01`) granting `0x4D4D` on a connection with
mining channels, and no other extension. The connection then gets, on the
channel bit:

```text
JobLeaves (0x00):  channel u32 | job u32 | output 0 (53) | u16 n | CTMD (n)
                   | each leaf's aux branch, in CTMD order (32 x h, leaf to root)
WinProof  (0x01):  channel u32 | job u32 | category (32) | u16 n | AuxProof v2 (n)
                   | u8 m (0 or 69) | the split preimage (m)
```

`JobLeaves` follows each job that commits a root, on standard and extended
channels; `WinProof` follows the acknowledgement of each share that wins a
token. A message of any other extension closes the connection.

**Stratum V1.** Firmware asks for `pickaxe-merge` in `mining.configure`
(beside `version-rolling`); the answer is `true` on a node that merge-mines
tokens, otherwise `false`. Then, after each `mining.notify`,

```text
{"id":null,"method":"pickaxe.merge_leaves",
 "params":["<job id>","<output 0 hex>","<CTMD hex>",["<aux branch hex>", ...]]}
```

and, after the acknowledgement of a winning `mining.submit`,

```text
{"id":null,"method":"pickaxe.merge_proof",
 "params":["<job id>","<category hex>","<AuxProof v2 hex>","<split preimage hex or empty>"]}
```

**The logs.** A standard channel's device never sees its coinbase, so its
leaves are a trust assumption that anyone can audit after the fact. Beside
the token file the node writes, readable by its owner alone (mode 0600 on
Unix; the directory's permissions elsewhere):

```text
<token file>.leaves.jsonl, one line per standard-channel job that commits a root:
{"time":<unix seconds>,"channel":<id>,"job":<id>,"output0":"<hex>","leaves":"<CTMD hex>"}
<token file>.wins.jsonl, one line per token a share wins:
{"time":<unix seconds>,"channel":<id>,"job":<id>,"payout_hash":"<hex>","category":"<hex>","proof":"<hex>","split":"<hex or empty>"}
```

An SV1 win's line names its job by the notify's hex job id and has no
channel. No line names an address: leaves carry `HASH256` of the payout
script, which a miner matches against their own. Writer threads take the
lines through bounded queues, so a connection never waits on the disk, and
past 8 MiB a log moves to `.jsonl.1` and starts again. The operator may
publish the leaf log, and hands a miner the lines of their wins.

**The proofs.** Each win's `AuxProof` v2 is built from the job's coinbase
and branch and checked as a covenant would check it, with the ASERT check
on mainnet and chipnet, before it is sent or logged; one that fails is
logged and dropped. A proof needs the block's coinbase branch to be at most
25 deep, the proof format's limit. P2Pool builds no claim transactions:
claiming is the miner's, or their tool's, with the token's covenant.

## Job Declaration leaves

A Full-Template client whose declared coinbase's output 0 is a token root
puts the `CTMD` disclosure of its leaves in `DeclareMiningJob.excess_data`,
as Pickaxe's client does. The node refuses the declaration with
`invalid-coinbase-tx`, the reason in its error details, when output 0 is a
root and `excess_data` is empty, when `excess_data` is not empty without a
root, or when the disclosure does not decode or its leaves do not make the
root. When the job is installed, each leaf must pay the channel's miner and
carry the job's terms as its split (the donation the token chose, then the
node's fee, each to the script the coinbase pays it to), so the node's fee
and the miner's P2Pool donation apply to declared tokens too. A
Coinbase-only custom job sends no declaration, so its output 0 must be the
empty commitment. See [Job Declaration](bch-job-declaration.md).

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

The experimental [SAFA AuxPoW covenant](../p2poolv2_safa/README.md)
verifies v2 proofs inside the BCH 2026 VM (`p2poolv2_safa`, on
libauth-rs; it replaced the Node tool `tools/safa-auxpow` with
byte-for-byte parity on 2026-10-29). It rebuilds the leaf from
introspection (the vault's category, locking bytecode and state, the
claim's payout and target, and the split's `ext`), folds both branches,
checks the header's work against the token's target and its nBits against
ASERT at the coinbase's BIP34 height (the corrected band above, with a
fallback after 4,032 blocks without a claim), binds that height with
`OP_CHECKLOCKTIMEVERIFY`, and pays the split to scripts of at most 35
bytes. The vault is the claim's only input: it pays the fee from its own
cash, `S` to `3S` satoshis for `S` bytes of claim bound, and keeps at least
800 satoshis. Its tests run Pickaxe's
vectors, BCHN's 14,000 aserti3-2d vectors and the real headers from this
repository's Rust tests in the VM. A chipnet test token (tSAFA) is
prepared but not deployed. The v1 variant passed a real regtest withdrawal
on 2026-10-16; the v2 covenant has not yet run against BCHN.

Deployment compilation and the genesis CLI accept only 1 to 63 threads.
At 288 or more, the age cap prevents the target from ever easing above its
previous value. The stricter bound preserves room to recover before that
cap. Both fixes must precede genesis because the deployed covenant is
immutable. The regenerated chipnet fixture uses outpoint `11` x 32, output
0, four threads and 84,000,000,000 virtual satoshis. Its new `lock_hash`
requires new leaves and newly mined proofs; old proofs cannot claim it.
The regression checks chipnet block 329,001's nBits with a header timestamp
two hours before its real parent's time and rejects a target one unit
above the x8 edge. The mainnet edges are unchanged. The
[rebuilt Pickaxe claim](../p2poolv2_safa/README.md#chipnet-tsafa-deployment)
passes the standard BCH 2026 VM offline; this is not a BCHN deployment or
broadcast.

SAFA is the first requested token target. Its
[author's draft and linked Bitauth template](https://bitcoincashresearch.org/t/safas-a-sha256-asic-minable-automated-token-market/2123)
check `SHA256d(next NFT baton)` against a difficulty target, with no parent
header, coinbase or inclusion branch. Committing its baton hash in BCH does
not satisfy that mining rule. The variant keeps the draft's threads, market
and difficulty adjustment and replaces the mining hash check with a
merge-mining check. It is not a deployed version of the author's token or a
final SAFA specification. A complete independent review, the economic parameters (the
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
the [variant's evidence record](../p2poolv2_safa/README.md#regtest-evidence-and-reproduction).
The same opt-in test now builds and verifies v2 proofs; it has not been
rerun against BCHN since the change.
