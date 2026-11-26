# BCH auxiliary work proofs

Every Bitcoin Cash coinbase P2Pool builds carries a merge-mining commitment
as output 0, in the **merge-mining common format v2** agreed with Pickaxe,
so both build and check the same bytes. It is a share-chain rule: it
changes the coinbase every BCH share carries, not Bitcoin Cash consensus.
`shares::merge_mining` holds the commitment, `shares::auxpow` the token
leaves, the tree and `AuxProof` v2, and `shares::bch_asert` the ASERT
header check. Earlier formats: [Rule history](sharechain/rule-history.md).

Pickaxe's specification is
[`docs/merge-mining.md`](https://github.com/CyberAshven/pickaxe-miner/blob/feat/merge-mining-v2/docs/merge-mining.md)
in pickaxe-miner (PR #70). P2Pool's unit tests pin its vectors at commit
[`a1c80a1ba6ad6b1542af54f4af28058d3241c3df`](https://github.com/CyberAshven/pickaxe-miner/commit/a1c80a1ba6ad6b1542af54f4af28058d3241c3df):
the leaf, the two-token root
`5b61ecfe96c714397bc35ed813890f49f2b0287a78111e19d156f4eecd1c38d0`, the
53-byte output 0, the exact empty commitment, the 224-byte proof and the
split leaf all match byte for byte. The leaf disclosure, its Stratum V1 and
V2 transports and the Job Declaration leaf rules follow PR #70 at
[`a92fef7`](https://github.com/CyberAshven/pickaxe-miner/commit/a92fef7)
("Disclosure" and "Job Declaration leaves"), with the same vectors.
`shares::auxpow::tests` checks proof encoding and verification, including
refusal when the expected state hash changes, and
`shares::merge_mining::tests` both commitment outputs.

All integers are little endian. `SHA256` is single SHA-256 and `HASH256`
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

**The exact empty commitment** is the zero root with `h = 0` and salt 0: the
coinbase merge-mines no token. A zero root with any other `h` or salt, or
`h` above 16, is not a commitment; P2Pool refuses both
(`MergeMiningCommitment::validate`).

Pool-built jobs carry the empty commitment when the node has no tokens to
merge-mine, and otherwise a root of each miner's own token leaves
([below](#merge-mining-tokens-on-a-public-node)); a miner can also declare
one through [Job Declaration](bch-job-declaration.md). The share header
carries the root, `h` and salt in `pool_merge_mining` for a pool-built
share (tag `0x23`, only when the root is not the empty one) or in
`bch_coinbase` for a declared share, and the share commitment digests it,
so one proof of work stands behind one root. Every peer rebuilds output 0
byte for byte and checks only its format: no peer knows the tokens behind a
root, and share validity never depends on them. The rule tag
`mm2-roots-asert8-ticks` names this header format with the ASERT band
below (`8`: the test networks' eighth to
eight times; `-ticks`: the band follows the CHIP-2025-03 ticks and next
block interval of the block the share builds on, see
[Mining on the current chain](sharechain/current-chain.md#one-minute-blocks)).

A share is a **token share** when its commitment is anything but the exact
empty one (`share_has_token_commitment`), and a **no-token share**
otherwise. Their coinbase limits and payout rules are in
[Hybrid payouts](architecture/bch-hybrid-payouts.md#31-token-shares-and-no-token-shares).

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
preimage and the token's covenant pays the shares; how they are computed is
a token rule, not P2Pool's. Pickaxe's split leaf (token A with a 1% donation
to P2PKH(`d0` x 20) and a 1% fee to P2PKH(`fe` x 20): `ext`
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
canonical: exact lengths, nothing trailing. The `d ≤ 25` limit (2^25
transactions) belongs to the proof format. P2Pool's own coinbase proofs
between share-chain nodes are not limited by it: a BCH block deeper than 25
levels is still a valid share, it just cannot be token-claimed.

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
within the network's band:

- mainnet: `computed / 2 ≤ header_target ≤ computed × 1.05`;
- test networks: `computed / 8 ≤ header_target ≤ computed × 8`, or nBits
  exactly the proof-of-work limit's (`0x1d00ffff`, a minimum-difficulty
  block).

`computed` is ASERT's target for a parent at `height - 1` dated with the
header's own time, which stands in for the parent's (a proof has no parent
header). A header minted hours after its parent is harder than `computed`,
hence the wide hard side. A test-network header can also predate its parent
by hours ([BCHN timestamp checks](https://gitlab.com/bitcoin-cash-node/bitcoin-cash-node/-/blob/master/src/validation.cpp)),
and chipnet's one-hour half-life makes a 5% easy side too narrow, so the
test networks allow x8 on both sides. These are proof-format checks agreed
with Pickaxe, not BCH block validation.

P2Pool's `BchAsert::check_header_bits` and the SAFA covenant use the same
bands, number for number: mainnet `target × 100 ≤ computed × 105`, test
networks `target ≤ computed × 8`, and `computed / d` floored on the hard
side. A header dated two hours before its parent (BCHN accepts it when the
parent is dated ahead) makes the computed target 2^(7200 / 172800), about
1.029, times too hard on mainnet and 4 times on the test networks; both fit
their bands. A test runs real chipnet block 329,001 dated 200 s, 260 s,
10 minutes, one hour and two hours before its parent.

The arithmetic follows `CalculateASERT` and `GetNextASERTWorkRequired` in
BCHN's `src/pow.cpp` integer for integer, with the anchors and half-lives
of BCHN's chain parameters (`src/chainparams.cpp`):

| Network | Anchor height | Anchor nBits | Anchor parent time | Half-life |
|---|---|---|---|---|
| Mainnet | 661,647 | `0x1804dafe` | 1,605,447,844 | 172,800 s |
| Chipnet and testnet4 | 16,844 | `0x1d00ffff` | 1,605,451,779 | 3,600 s |
| Scalenet and regtest | no anchor | | | the check is skipped |

Its tests run the aserti3-2d specification's twelve vector runs (14,000
lines, BCHN's QA assets
[`test_vectors/aserti3-2d`](https://gitlab.com/bitcoin-cash-node/bchn-sw/qa-assets/-/tree/master/test_vectors/aserti3-2d),
MIT, vendored beside `shares/bch_asert.rs`) and nine real mainnet and nine
chipnet header pairs up to mainnet 974,001 and chipnet 329,001, with
chipnet's minimum-difficulty block 300,001.

**Where P2Pool runs it.** On every proof `AuxProof::verify` checks with a
network, and on every **token share's** bitcoin header, with the node's
`bitcoin_cash_network`: at the header gate peers run
(`validate_header_on_arrival`), and again in
`validate_share_block`, which a share the node mined itself reaches
without that gate. A node builds no token job on a template whose nBits
fail the band at its height and `curtime`; it mines that template without
tokens. Stratum V2 checks a declared token job's nBits and requested
minimum time before proposal validation or installation
(`invalid-token-asert`). No-token shares are not checked: BCHN alone judges
their blocks.

## Merge-mining tokens on a public node

On a public P2Pool node the operator's fee and the P2Pool donation apply to
merge-mined tokens as they apply to BCH. This is node behaviour, not a
share-chain rule: peers check only the format of a share's root.

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
Its covenant eases the state's target by the ticks since the state's block
(`min(dt, 288 x 600) / (threads x 600)`; on ten-minute blocks
`min(dh, 288) / threads`), so the node computes each job's leaf at the
job's coinbase height: `state_hash` is `HASH256` of the state and
`target_bits` is `safa_claim_target_bits`, integer for integer the SAFA
tool's `claim_target_bits`, counting ticks with the network's
`TickSchedule` pins. A template at or below the state's height is mined
without tokens.

At most 16 tokens, one per category; the set must fit one tree no taller
than the lowest limit among them, or the file is refused. An empty list
merge-mines nothing. A node does not start on a file it cannot read or that
is not valid (at most 1 MiB). It reads the file again whenever its
modification time changes, so the tool that claims a token can write the
new state hash; a changed file that is broken keeps the last good tokens
and logs why. No file ships with the node, since a vault's state changes
with every claim. tSAFA is deployed on chipnet; its parameters are in
[the SAFA tool's README](../p2poolv2_safa/README.md#chipnet-tsafa-deployment).

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

For Pickaxe's token A with a 1% donation and a 1% fee this is Pickaxe's
split leaf, byte for byte. The layout (height, salt and slots) depends only
on the categories, so it is searched once per file; miners differ only in
their root, which replaces a placeholder in the coinbase every miner on a
template shares. A job whose miner has no payout address carries the empty
commitment.

Every such share is a token share: its coinbase follows the token share's
limit ([Hybrid payouts](architecture/bch-hybrid-payouts.md#33-the-coinbase-limit-c)),
and its bitcoin header takes the ASERT check. A share wins a token when its
hash meets the token's target, whatever its miner's own target. P2Pool
does not ease a device's difficulty towards the easiest token target as
Pickaxe does, so a device whose difficulty is above a token's target
misses some of that token's wins.

## Disclosure to miners

A miner checks before hashing that their leaves pay them, and only the
donation and fee they agreed to, and claims their own wins. P2Pool
discloses both in Pickaxe's formats (`stratum::merge_disclosure`), only to
firmware that asks.

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
request_id | u16 n | n x u16`) after `SetupConnection`; the node grants it
in `RequestExtensions.Success` (`0x01`) on a connection with mining
channels, beside any other extension it grants there
([Miner options](miner-options.md#stratum-v2)). The connection then gets,
on the channel bit:

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
script, which a miner matches against their own. Bounded queues keep a
connection from waiting on the disk, and past 8 MiB a log moves to
`.jsonl.1` and starts again. The operator may publish the leaf log, and
hands a miner the lines of their wins.

**The proofs.** Each win's `AuxProof` v2 is built from the job's coinbase
and branch and checked as a covenant would check it, with the ASERT check
on mainnet and chipnet, before it is sent or logged; one that fails is
logged and dropped. A proof needs the block's coinbase branch to be at most
25 deep. P2Pool builds no claim transactions: claiming is the miner's, or
their tool's, with the token's covenant. An operator can run
[`safa claimer`](../p2poolv2_safa/README.md#claimer) separately to follow
one SAFA vault, verify pool wins and broadcast the claims.

## Job Declaration leaves

A Full-Template client whose declared output 0 is a token root discloses
its leaves as `CTMD` in `DeclareMiningJob.excess_data`, as Pickaxe's client
does, and each leaf must pay the channel's miner and carry the job's
donation and fee as its split. A Coinbase-Only job merge-mines no token.
The rules and error codes are in
[Declared token leaves](bch-job-declaration.md#declared-token-leaves).

## Offline CLI

No node configuration or wallet is required:

```sh
p2poolv2_cli bch-auxpow build tokens.json
p2poolv2_cli bch-auxpow verify proof.hex token.json --network chipnet
```

`tokens.json` lists leaves: each token is the token file's object above
with `max_height` left out and `payout_hash` and `ext` added
(`"payout_hash": "22650758a42ebfd7306e771ad24c442cad795ed9e549392233729f647a2b1be5"`
and a zero `ext` make Pickaxe's synthetic token A, not a real token).
Instead of `ext`, a token may give its split, from which `ext` is computed:
`"split": {"donation_bps": 100, "donation_script": "<hex>", "fee_bps": 100,
"fee_script": "<hex>"}`, scripts being locking bytecode (left out at a rate
of 0); without either, `ext` is 32 zero bytes. `build` prints the
commitment, output 0 in hex and each token's slot and branch. `verify`
takes the proof's bytes in hex and one token's leaf as JSON, with
`--max-height` for the token's tree limit (16 by default) and `--network`
for the ASERT check (regtest skips it). Each input file is limited to
1 MiB.

## The SAFA covenant

The experimental [SAFA AuxPoW covenant](../p2poolv2_safa/README.md)
(`p2poolv2_safa`, on libauth-rs) verifies v2 proofs inside the BCH 2026 VM.
It is a merge-mined variant of the
[author's SAFA draft](https://bitcoincashresearch.org/t/safas-a-sha256-asic-minable-automated-token-market/2123),
whose own mining check (`SHA256d(next NFT baton)` against a target) a BCH
commitment cannot satisfy; it is not the author's token. A claim rebuilds
the leaf from introspection (the vault's category, locking bytecode and
state, the claim's payout and target, and the split's `ext`), folds both
branches, checks the header's work against the token's target and its
nBits against ASERT at the coinbase's BIP34 height (the band above, in
CHIP-2025-03's ticks from the same pinned switch as the header gate, with a
fallback after 4,032 ten-minute blocks' worth of ticks without a claim),
eases the target by the ticks since the last claim, binds that height with
`OP_CHECKLOCKTIMEVERIFY` (the lock height's ticks in 600-tick units,
rounded up), and pays the split to scripts of at most 35 bytes. The vault
is the claim's only input: it pays the fee from its own cash, `S` to `3S`
satoshis for `S` bytes of claim bound, and keeps at least 800 satoshis.
Threads, parameters, tests and the chipnet tSAFA deployment are in its
README; a run against BCHN with a real P2Pool coinbase and an independent
review are open
([regtest evidence](../p2poolv2_safa/README.md#regtest-evidence-and-reproduction)).

Any token covenant must append a zero sign byte before converting a 32-byte
hash to an integer, and test the high-bit case (the draft's release
scenario passes Libauth `3.1.0-next.8`'s VM with a hash above its unsigned
target, through a direct `OP_BIN2NUM` comparison). It must also keep to the
standard budgets: the
[2026 P2S rules](https://github.com/bitjson/bch-p2s#unlocking-bytecode-length)
allow 10,000 bytes of standard unlocking bytecode, including the redeem
script, and a v2 proof carries the whole coinbase but output 0.
