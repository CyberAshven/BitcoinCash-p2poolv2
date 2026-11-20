**The architecture files are LLM generated. The goal is make it easier
for new comers to the code base and other LLMs to quickly understand
the design and remain consistent with the design choices.**

# Hybrid payouts on Bitcoin Cash

How the Bitcoin Cash coinbase that P2Pool builds pays the PPLNS window: which
payees are paid directly, which through a payout tree, and how the tree pays
them. The window, weights and per-share cuts that feed this rule are in
[Payout distribution](payout-distribution.md); what a miner sees is on
[Coinbase payouts](../payouts/coinbase.md).

Everything here is a share-chain rule, except the tree's executor, storage
and API, which are node behaviour. BCHN never sees share-chain rules, and
nothing here changes Bitcoin Cash consensus. The rule replaced an 11-payout
cap with a weighted tail draw, which took a Bitcoin firmware report as a
Bitcoin Cash limit ([Rule history](../sharechain/rule-history.md)).

## In plain words

- **No-token share** (the exact empty merge-mining commitment): every payee
  is paid directly, up to BCH's 1,000,000-byte transaction limit, about
  29,400 P2PKH payees. Block size (ABLA, at least 32 MB) is never the limit.
- **Token share:** a token claim re-hashes the coinbase in one script input
  of at most 10,000 bytes, so the coinbase stays within
  **`T` = 3,072 bytes**, about 79 P2PKH payees.
- **Past the limit**, payees go smallest first. The first that does not fit
  and everyone after it are paid through one **payout tree** output: a
  keyless covenant anyone can spend after the 100-block maturity, but only
  into exactly the outputs the share chain decided, less each payee's share
  of a 1 to 3 sat/byte fee. Every node broadcasts the trees of found blocks.
- **Donation and fee** are per share and paid like any payee.
- **BCHN sees** ordinary coinbases, sometimes with one P2SH32 output, and
  later standard transactions with no signatures, using introspection
  (2022), P2SH32 (2023), the VM limits (2025) and loops with 10,000-byte
  unlocking scripts (2026).

## 1. Decisions this design follows

Plan decision 14 ([bch-plan.md](../bch-plan.md)): the exact empty commitment
pays every miner directly with no P2Pool cap or lottery; a token share keeps
its coinbase within `T`, one fixed share-chain constant in the protocol tag,
and pays the payees past it through a covenant tree whose payees fund their
own fees; a deterministic rule places the payee that straddles the limit;
no-token shares follow the same rule past BCH's 1,000,000-byte limit, and
the share body allows a full 1,000,000-byte coinbase.

### Format v2 in brief

Output 0 of every P2Pool coinbase is the merge-mining record of common
format v2, agreed with Pickaxe (`shares/merge_mining.rs`; tree and
`AuxProof` v2 in `shares/auxpow.rs`):

```text
output 0, 53 bytes: value 0 (8) | 2c | 6a 2a | "CTMM" | 02 | root (32) | h (1) | salt (4, LE)
empty commitment:   root = 32 zero bytes, h = 0, salt = 0
```

Slots, the 169-byte leaf, the proof (aux tree height `h <= 16`, coinbase
branch depth `d <= 25`) and how jobs and declared coinbases fill the root
are on [Auxiliary work proofs](../bch-auxpow.md). A covenant rebuilds
output 0 and the leaf itself; the proof never carries them. The share
header carries root, `h` and salt, so every peer rebuilds output 0.

## 2. Facts it rests on

From BCHN master and the BCH upgrade specifications (Sources, at the end).

| Fact | Value | Kind |
|---|---|---|
| Largest transaction, coinbase included | 1,000,000 bytes (`MAX_TX_SIZE`, `bad-txns-oversize`) | BCH consensus |
| Coinbase outputs | no count limit; never tokens | BCH consensus |
| Coinbase scriptSig | 2 to 100 bytes | BCH consensus |
| Coinbase maturity | 100 blocks (`COINBASE_MATURITY`) | BCH consensus |
| Block size | ABLA, at least 32,000,000 bytes on mainnet | BCH consensus |
| Largest standard transaction | 100,000 bytes (`MAX_STANDARD_TX_SIZE`) in BCHN; Knuth has no limit below 1,000,000 (Knuth PR #728) | node policy |
| Unlocking bytecode per input; largest stack item | 10,000 bytes each (P2S since May 2026; VM Limits) | both |
| Operation cost per input | at most `(41 + unlocking length) * 800` | both |
| Hash digest iterations per input | at most `(41 + L) / 2` standard, `(41 + L) * 7 / 2` consensus; a message of `M` bytes costs `1 + (M + 8) / 64`, plus 1 if double-hashed | both |
| Loops (`OP_BEGIN`/`OP_UNTIL`), functions, bitwise shifts | since 2026-05-15 (mainnet block 951,145) | BCH consensus |
| SHA-256 that resumes from a midstate | no such opcode | BCH consensus |
| Dust threshold | `3 * dustrelayfee * (output size + 148)`: 546 P2PKH, 540 P2SH20, 576 P2SH32; never checked in a block (`IsStandardTx` runs only in mempool acceptance) | relay policy |
| Minimum relay fee | 1 sat/byte by default; size counts as `max(size, SigChecks * 50)` | relay policy |
| Unconfirmed chains | no length limit since 2021-05-15 | relay policy |

P2Pool's own limits:

| Limit | Value | Where |
|---|---|---|
| Share transactions, including their share coinbase | 200 KiB | `BLOCK_TXS_SIZE_LIMIT` |
| BCH share body, including the carried BCH coinbase | 1,204,800 bytes | `BCH_SHARE_BODY_SIZE_LIMIT` (section 8) |
| BCH P2P message | 2,144 KiB: the full share body, up to 960 KiB of carried weak shares (`WEAK_BYTES`), header and merkle branch | `MAX_P2P_MESSAGE_SIZE`, `node/messages.rs` |
| Share spacing | about 10 seconds | `IDEAL_BLOCK_TIME` |

## 3. The payout rule

The producer and every validator build the same coinbase byte for byte
(`validate_bitcoin_payout`). This is rule 8 of
[Payout distribution](payout-distribution.md), which gives the window,
payout rights, exact-work weights and cuts.

### 3.1 Token shares and no-token shares

A share is a **no-token share** when its merge-mining commitment is exactly
the empty one: root of 32 zero bytes, `h = 0` and salt 0. Any other value,
including a zero root with a non-zero salt, is non-empty for payout
classification. The merge-mining format validator independently rejects a
zero root unless both `h` and salt are zero. Pool-built and declared
coinbases can both carry token commitments; the rule classifies the
commitment, not the channel type.

### 3.2 Payees and their order

The donation key and each share's fee address are ordinary payees beside the
share payees, all credits aggregated by locking bytecode before ordering or
splitting ([per-share cuts](payout-distribution.md#bitcoin-cash-per-share-cuts)).
Each payee `i` then has a locking bytecode, a weight `w_i`, an amount
`a_i = floor(A * w_i / W)` and an output of `s_i` bytes: 34 for P2PKH, 32
for P2SH20, 44 for P2SH32. (The payout-level draw, rule 9, may then replace
amounts; the order stays by weight.) On BCH a payee must be one of these
three; every other payout script in a share header is invalid, Bitcoin
witness programs included.

**Canonical order:** ascending weight, ties by locking bytecode compared as
bytes. The smallest payee comes first.

| Order | Effect | Verdict |
|---|---|---|
| Largest first | The tree holds the small payees: its fixed per-payee fee is the largest fraction of their money, and small tree outputs can fall below dust and cannot be relayed. | Rejected |
| Address or script order | Unrelated to size; a payee can grind an address to land in the direct part. | Rejected |
| Smallest first | Small payees are paid in the coinbase with no fee. Tree payees are the largest, so the fee is a tiny fraction and tree outputs stay far above dust. A large miner splitting into many small addresses to jump the line saves under 100 satoshis per address and costs others about the same. | Adopted |

### 3.3 The coinbase limit `C`

| Share | `C` | Why |
|---|---|---|
| No-token share | `D` = 1,000,000 bytes (`BCH_MAX_TX_SIZE`) | BCH consensus `MAX_TX_SIZE`. Not a P2Pool number. |
| Token share | `T` = 3,072 bytes (`BCH_TOKEN_SHARE_COINBASE_LIMIT`) | The token claim must hash the coinbase in the VM (section 4). |

`S` = 367 bytes (`BCH_COINBASE_BYTES_BESIDE_PAYOUTS`) is the most a P2Pool
coinbase can take besides its PPLNS payouts, every field at its worst case,
so `C - S` is always safe:

| Part | Bytes |
|---|---|
| Version, input count, outpoint, scriptSig length, sequence, one-byte output count, lock time | 51 |
| scriptSig (BCH's maximum) | 100 |
| Output count taking 3 bytes from 253 outputs | 2 |
| Output 0, the CTMM commitment | 53 |
| Tree output (P2SH32) | 44 |
| Padding before the share commitment (at most 63 bytes of padding) | 74 |
| Share commitment output | 43 |
| **Total** | **367** |

There is no donation or fee output. Outputs a coinbase carries before the
split, as Hydrapool's donation and fee do, take their own bytes from `C`.
The payout budget is `P = C - S`: 2,705 bytes for a token share (79 P2PKH
payees) and 999,633 bytes for a no-token share (29,400 P2PKH payees).

### 3.4 The straddle rule (exact)

Walk the payees in canonical order.

1. **Everyone fits.** If the payees' outputs total at most `P + 44` bytes
   (the budget without a tree output), every payee is paid directly and the
   coinbase has no tree output: 80 P2PKH payees for a token share, 29,402
   for a no-token share.
2. **Otherwise** payee `j` is paid directly when the outputs of payees `1`
   to `j` total at most `P`. The first payee for which that fails, the
   straddler, and every payee after it go to the tree.
3. **Round down, never split.** The straddler always goes to the tree, even
   if it is over by one byte. A payee is never split between a direct output
   and a tree leaf. No later payee is moved back into the direct part, even
   if its smaller output would fit the bytes left over (at most 43).
4. **Token shares** also need fewer than 253 coinbase outputs
   (`MAX_TOKEN_SHARE_COINBASE_OUTPUTS`), so the output count is one byte
   where the covenant expects it. With `T` = 3,072 a coinbase has at most 88
   outputs (85 P2SH20 payouts and 3 others, or 84 and 4 with the tree
   output), so this always holds; validators check it anyway.

So the coinbase never exceeds `C`, and which payees are direct is a pure
function of the weights, the amount and the payees' scripts.

### 3.5 Remainder and dust

- **Remainder.** `A - sum(a_i)` (fewer satoshis than there are payees) is
  added to the last payee in canonical order, the largest, wherever it is
  paid.
- **Direct payouts below dust** are paid as they are: BCH consensus accepts
  a coinbase output of any value, and such an output can still be spent
  later. At today's subsidy one share in a 120,960-share window earns about
  2,500 satoshis per block, so this needs about three more halvings (around
  2036) unless fees grow, or a share far below the window's average
  difficulty.
- **Tree leaves below dust.** A tree payee whose net amount (section 5.3) at
  3 sat/byte, the highest permitted rate, is below its dust threshold is
  removed from the tree and its `a_i` is added to the last payee. Repeat
  until no leaf is below dust; the count only falls, so this ends. Since tree
  payees are the largest, this needs every payee below about 700 satoshis.
  If the final merged payee still cannot fund its fee at 3 sat/byte and its
  dust threshold, the whole merged tail is paid directly to that payee in
  the tree output's place: the same value, and never a zero-value or
  below-dust tree output.

### 3.6 Coinbase layout

Output 0: the CTMM commitment. Then the direct payees in canonical order
(the donation key and fee addresses among them), the tree output if any, the
padding and the share commitment last. BCH sets no rule on output order
inside a transaction (CTOR orders transactions, not outputs), so this order
is the share chain's own choice.

## 4. Why T = 3,072 bytes

The token claim's unlocking bytecode carries everything in the proof except
output 0, which the covenant rebuilds, plus the token's redeem script. The
parts besides the coinbase take about 2,535 bytes at worst: header 80, aux
branch at `h = 16` (512), coinbase branch at `d = 25` (800), a redeem script
of about 1.1 KB, `h`, salt, `d` and push opcodes.

```text
unlocking = (coinbase - 53) + 2,535 <= 10,000
coinbase <= 7,518 bytes
```

| Coinbase | Unlocking | Margin to 10,000 | Note |
|---|---|---|---|
| 3,072 (`T`) | 5,554 | 4,446 | Implemented budget |
| 6,500 | 8,982 | 1,018 | Ceiling for any later raise |
| 7,518 | 10,000 | 0 | Absolute limit for a one-input claim |

The margin at 3,072 covers a larger redeem script, a coinbase branch of 32
levels if BCHN lifts its 2 GB block guard (224 more bytes), and the SAFA
variant's growth. At `T` the claim (estimate) has `(41 + 5,554) * 800` =
4.48M operation cost and about 175 digest iterations against a standard
limit of `(41 + 5,554) / 2` = 2,797; it is about 6 KB, about 6,000 satoshis
at 1 sat/byte. The SAFA variant's regtest withdrawal, with a 1,266-byte
scriptSig, passed BCHN 29.2.0 (`p2poolv2_safa/README.md`).

A deployed token covenant cannot change. **T must never exceed what the
deployed token covenants can claim.** So any raise is a new protocol tag and
stays at or below 6,500 unless every deployed token was written for more.

## 5. The payout tree

Code: `accounting/payout/payout_tree.rs`, `shares/bch_payout_covenant.rs`.

### 5.1 Shape

- **Leaves** are the tree payees in canonical order.
- **Fan-out** `F` = 500 outputs per node transaction
  (`BCH_PAYOUT_TREE_FAN_OUT`). Leaves are grouped in order, `F` per node,
  the last group possibly smaller; nodes are grouped `F` per parent the same
  way until one root remains.
- Up to 500 tree payees: one transaction. Up to 250,000: two levels
  (`1 + ceil(n / 500)` transactions).
- Each node is a P2SH32 output whose redeem script commits to that node's
  exact outputs. The coinbase holds the root. Built bottom-up, like a merkle
  tree, so the root's hash commits to every output below it. Validators
  rebuild the whole tree to check the root output.

### 5.2 Node covenant (opcode sketch)

```text
unlocking bytecode (push-only):  <filler: Z zero bytes>  <redeem script>

redeem script, with this node's V (its value), k (its output count), Z and H written in:
OP_0 <Z> OP_NUM2BIN OP_EQUALVERIFY          // exactly Z zero bytes of filler
OP_TXVERSION OP_2 OP_NUMEQUALVERIFY        // one canonical transaction:
OP_TXLOCKTIME OP_0 OP_NUMEQUALVERIFY       //   version 2, lock time 0,
OP_TXINPUTCOUNT OP_1 OP_NUMEQUALVERIFY     //   one input, no outside money,
OP_0 OP_INPUTSEQUENCENUMBER <0xffffffff> OP_NUMEQUALVERIFY  // final sequence
OP_0 OP_UTXOVALUE OP_8 OP_NUM2BIN <V, 8 bytes> OP_EQUALVERIFY  // exactly V
OP_TXOUTPUTCOUNT <k> OP_NUMEQUALVERIFY     // exactly k outputs, no change
OP_0 OP_0                                  // acc = empty, i = 0
OP_BEGIN                                   // stack: acc i
  OP_DUP OP_OUTPUTTOKENCATEGORY OP_0 OP_EQUALVERIFY    // output i carries no token
  OP_DUP OP_OUTPUTVALUE OP_8 OP_NUM2BIN                // value, 8 bytes LE
  OP_OVER OP_OUTPUTBYTECODE OP_SIZE OP_SWAP OP_CAT     // length || script
  OP_CAT                                               // the output as serialized
  OP_ROT OP_SWAP OP_CAT OP_SHA256                      // acc = SHA256(acc || output i)
  OP_SWAP OP_1ADD
  OP_DUP <k> OP_NUMEQUAL
OP_UNTIL
OP_DROP <H> OP_EQUAL                       // a child accepts its one output hash
// The root instead accepts H1, H2 or H3, one for each fee-rate variant.
```

`H` is computed off chain the same way: `acc_0` empty,
`acc_(i+1) = SHA256(acc_i || serialized output i)`. Scripts are at most 35
bytes, so `OP_SIZE` gives the same one byte as the serialized length.

| Check | Stops |
|---|---|
| Exact outputs (`H`, `k`) | Any change to a payee, an amount, or an extra output for the executor |
| One input | An executor adding inputs and outputs to skim the fee allowance |
| `V` | Spending a look-alike UTXO someone sent to the same address with another value |
| No token on any output | An executor minting a token (a node that spends output 0 of its parent is a token genesis input) and attaching it to payees' outputs |
| Version, lock time, sequence, exact zero filler | Variants of the same transaction. Minimal pushes fix the unlocking form, so every txid at each permitted fee rate is known as soon as the block is found |

There are no signatures and no keys. After a root variant is selected, its
descendants are fixed. BCH is first-seen with no replace-by-fee, so another
variant cannot replace a root a node already accepted. This is the
"congestion control tree" idea (BIP119 on Bitcoin, which needs a new opcode
there); on BCH the 2022 introspection and 2026 loop opcodes already do it.

### 5.3 Fees: exact, per byte, from the tree

- `R` is one of 1, 2 or 3 sat/byte (`BCH_TREE_FEE_RATES`). The root
  commits to all three output variants; child covenants are specific to
  their variant. Every P2Pool node executes at 1 sat/byte
  (`BCH_TREE_EXECUTION_FEE_RATE`). Tree transactions have no signatures,
  so their relay size is their byte size.
- All node transactions are fully determined, so their sizes are known when
  the coinbase is built. Let `B` be their total size, `n` the tree payees and
  `O = B - sum(s_i)` the bytes that are not payee outputs.
- Tree payee `i` pays `phi_i = R * s_i + ceil(R * O / n)` and receives
  `a_i - phi_i`.
- Every node except the root pays exactly `R` times its size. The root pays
  `R` times its size plus a rounding excess below `n` satoshis.
- So the tree's total fee is at most `R * B + n - 1` satoshis, from the
  tree's own value only; the executor neither pays nor receives anything.

### 5.4 Dust

Each node transaction is relayed, so each tree output, a child node's
P2SH32 output included, must be at least its dust threshold: 546 (P2PKH),
540 (P2SH20), 576 (P2SH32). Section 3.5 removes leaves that would fall below
at 3 sat/byte before all three variants are built. Child nodes hold the sum
of their leaves. Coinbase outputs have no dust rule (section 2).

### 5.5 Who executes it, and when

- The root is a coinbase output, spendable 100 blocks after its block
  (`bad-txns-premature-spend-of-coinbase`), like direct payouts. Then anyone
  may broadcast the node transactions; with no limit on unconfirmed chains,
  a two-level tree goes out at once.
- **Every P2Pool node runs an executor** (`node/payout_tree_executor.rs`),
  with no keys and no money. It asks BCHN for its tip every 30 seconds
  (`EXECUTOR_POLL_INTERVAL`). On each new tip it broadcasts the 1 sat/byte
  transactions of every tree whose coinbase has 100 confirmations, root
  first, finishes the children of a 2 or 3 sat/byte root someone else
  broadcast, and rebroadcasts until every transaction of one root variant
  has been confirmed for 6 tips in a row (`SETTLED_TIPS`). A spent root
  proves nothing about its children; a rejection, transport error or
  missing input keeps descendants pending. Duplicates are harmless.
- **Data** (`store/payout_tree.rs`). Before a found block goes out, over
  Stratum V1, V2 or Job Declaration, the finding node stores the job's tree
  payees with the coinbase txid, root output, block height and hash; if that
  write fails the block still goes out. Every node also stores the tree when
  it validates the share, so a share-chain reorg does not lose it. Records
  stay after execution, about 50 bytes per tree payee.
- **Unknown blocks.** A test-network share can claim a block with a
  minimum-difficulty header that no BCHN accepts. A record whose block BCHN
  does not know is removed after 6 distinct new tips that report it unknown
  (`UNKNOWN_BLOCK_TIP_LIMIT`), counted only while BCHN is out of initial
  block download and kept across restarts. Known immature blocks, off-chain
  blocks BCHN still knows, and RPC errors do not age records.
- **API.** `/payout_trees` and `/payout_tree?coinbase_txid=` serve each
  tree's payees and raw transactions at every permitted rate, so a payee can
  broadcast them without P2Pool.

### 5.6 Sizes and VM cost

The filler `Z` is 8 zero bytes per output (`FILLER_BYTES_PER_OUTPUT`). It
buys operation-cost and hashing budget, which grows with the unlocking
length; the covenant checks its exact bytes, so it cannot change the
transaction ID. The loop uses about 3,000 operation cost and two hash
iterations per output, and each output's filler buys 6,400 operation cost
and 4 hash iterations. For 500 P2PKH outputs `test_utils::bch_vm` measures
1.51M of the 3.32M operation cost allowed and 1,002 of the 2,076 standard
hash iterations, 45% and 48%. Such a transaction is about 21 KB, below the
100,000-byte standard size; its 4,000 filler bytes, the redeem script and
pushes stay below the 10,000-byte unlocking limit. SigChecks are zero. The
builder computes exact sizes and fees. The BCH block carries one more
output for the tree: 44 bytes.

## 6. Failure modes

| Case | What happens | Mitigation |
|---|---|---|
| Fees rise above 1 sat/byte | Anyone may broadcast the 2 or 3 sat/byte variant of a root not yet accepted; above 3, relay may wait | A miner may include a tree directly; a later rule can change the rates for new trees. The money stays locked to the payees. |
| A miner forces the tree (a format-valid token commitment) | Tree payees pay their fee share and wait for the tree | Cannot redirect money |
| Dust sent to a node's address | A second UTXO with the same script | Spendable only with exactly `V`; an exact copy pays the payees twice |
| BCH reorg | Before maturity as for direct payouts; after execution, node transactions return to the mempool | Executor rebroadcasts; txids do not change |
| Share-chain reorg | The share that became the block leaves the share chain | The tree is fixed in the BCH block, stored by coinbase txid |
| All P2Pool nodes offline or lose data | Money waits in the tree, with no deadline | Any copy of the tree data is enough: every node, the API, a payee's own, or a rebuild from the share chain |
| BCH miners censor tree transactions | As for any transaction | Another miner includes them |

## 7. Can claims grow past T?

**What binds.** `OP_SHA256` and `OP_HASH256` hash one stack item of at most
10,000 bytes, and no opcode resumes SHA-256 from a midstate, so native
hashing needs the whole coinbase as one item. More inputs (each with 10,000
bytes of unlocking, readable by others with `OP_INPUTBYTECODE`) do not lift
that by themselves.

**Chosen.** `T` stays 3,072. A raise up to 6,500 with the same one-input
claims needs a new protocol tag and every deployed token to accept it.

**Other options**, none built:

- *Two-input claim,* about 9,900 bytes: input 0 reads the coinbase tail
  from a second input; tokens must be written this way from the start.
- *SHA-256 in script* across inputs: 0.7M to 1.2M operation cost per
  64-byte block by an opcode count, so a 100,000-byte claim hashes only 4 to
  7 KB, a 1 MB claim 40 to 70 KB.
- *Format v3:* the CTMM output moves into P2Pool's 64-byte-aligned tail, so
  a claim carries a midstate and about 100 bytes of tail and compresses 2
  blocks in script, as the share proof does (`shares/coinbase_proof.rs`);
  `T` and the tree could go. The
  strongest option; it changes format v2, so it needs Pickaxe's agreement
  and a prototype.
- *A resumable SHA-256 opcode* in a BCH upgrade (Elements has
  `OP_SHA256INITIALIZE`/`UPDATE`/`FINALIZE`).
- *P2Pool mines its own claim of up to 1 MB* into its templates: node-local,
  confirmed only when this pool finds a block.

A multi-input claim must bind the locking script of every helper input it
reads (`OP_UTXOBYTECODE`), anchor its chain of midstates at input 0, and be
tested by proving a false statement from scratch (the 0zkbrewer verifier
audits in Sources).

## 8. What changes where

Share-chain rules (BCHN never sees them):

- **Payout split:** section 3; `T` 3,072, `D` 1,000,000, `F` 500, fee
  rates 1 to 3, `Z` 8 and covenant version 1 form the tag segment
  `payout-hybrid-t3072-f500-r1to3-z8-cov1` (`bch_payout_rule_tag`), which
  replaced `payout-cap11`.
- **Payee scripts:** P2PKH, P2SH20 and P2SH32 only
  (`payee-p2pkh-p2sh20-p2sh32`).
- **Share body** (`body-1000000-204800`): `BCH_SHARE_BODY_SIZE_LIMIT` =
  1,000,000 (the coinbase, BCH's `MAX_TX_SIZE`) + 204,800 (share
  transactions) = 1,204,800 bytes, plus the header and coinbase merkle
  branch. A coinbase reaches BCHN only inside a block, so the 100,000-byte
  standard size never applies to it. The job builder checks `C`
  (`check_bch_coinbase_limit`); the 784-byte `BCH_COINBASE_BYTE_BUDGET` is
  gone.

Node behaviour:

- **Buffers:** the block receiver's pending buffer is bounded at 32 MiB (at
  least 26 of the largest shares); the organise worker's channel (512
  shares) at 64 MiB, where a validation task waits for room, and its
  pending buffer (1,024 shares) at 128 MiB, evicting its oldest blocks
  ([share processing pipeline](share-processing-pipeline.md#channel-configuration)).
- **Relay:** a new share whose coinbase outputs take 50,000 bytes or more
  travels without them, and the receiver rebuilds them
  ([P2P network](../p2p-network.md)); measured propagation is in
  [Network simulation](../simulation/network-sim.md).
- **Tree execution:** executor, storage and API (5.5).

What BCHN sees: no-token coinbases with up to thousands of outputs, token
coinbases with one P2SH32 tree output, and after maturity standard
transactions with no signatures, push-only unlocking and outputs above dust.
Limits outside P2Pool (Stratum V1 firmware, the 65,535-byte SV2 fields,
about 1,900 payees, and BCHN's `-percentblockmaxsize`) are on
[Pool size limits](../sharechain/pool-size.md) and [Stratum](../stratum.md).

## 9. How it is tested

- **Unit tests** (`cargo nextest`): the split, straddler and dust in
  `coinbase_budget/tests.rs`; tree shapes (1, 500, 501, 250,000 and 250,001
  leaves) and the fee bound `R * B + n - 1` in `payout_tree/tests.rs`;
  every node transaction on `test_utils::bch_vm`, with negative tests built
  from scratch, in `bch_payout_covenant/tests.rs`; producer and validator
  agreement in `sharechain_pplns/payout.rs`; the executor in
  `payout_tree_executor/tests.rs`.
- **BCHN regtest** (`p2poolv2_tests/src/payout_tree_bchn_test.rs`, ignored by
  default, run nightly by the `payout-tree-bchn` job of `sim-nightly.yml`):
  a token-share coinbase with 700 payees (79 direct, a two-level tree for
  621) that the executor broadcasts only after maturity, each payee checked
  to the satoshi, and a no-token coinbase over 100 KB paying 3,000 miners
  directly. Start BCHN regtest with `-acceptnonstdtxn=0`, then:

  ```sh
  P2POOL_TEST_BCHN_RPC_URL=http://127.0.0.1:18443 \
  P2POOL_TEST_BCHN_RPC_USER=user P2POOL_TEST_BCHN_RPC_PASSWORD=password \
  cargo nextest run -p p2poolv2_tests --no-default-features \
    --run-ignored only -E 'test(bchn_regtest)' -j 1
  ```

- **Chipnet** (a follow-up): coinbases of about 1 to 16 KB mined by an
  Avalon over Stratum V1, a declared token share whose tree executes after
  100 blocks, and a token claimed from a coinbase at `T`.

## 10. Approved choices and follow-ups

- `T` = 3,072 bytes for token shares; `D` = 1,000,000 bytes otherwise.
- Smallest-first order, ties by locking bytecode.
- The straddle rule: round down, never split, never back-fill.
- The tree beyond either coinbase limit.
- Direct payouts below dust are paid; tree leaves below dust are merged.
- Fan-out 500.
- Fee variants 1 through 3 sat/byte; nodes execute at 1 sat/byte.
- A 1,000,000-byte coinbase beside 200 KiB of share transactions.

Follow-ups: chipnet tree execution, device firmware measurements, and
format v3 (section 7), which needs Pickaxe's agreement first.

## Sources

- BCHN master
  ([source](https://gitlab.com/bitcoin-cash-node/bitcoin-cash-node/-/tree/master/src)):
  `consensus/consensus.h` (`MAX_TX_SIZE`, `COINBASE_MATURITY`,
  `MAX_COINBASE_SCRIPTSIG_SIZE`, `MAX_CONSENSUS_BLOCK_SIZE`),
  `consensus/tx_check.cpp` (`CheckCoinbase` calls `CheckTransactionCommon`;
  `bad-cb-length`, only `bad-txns-vout-empty` on outputs), `validation.cpp`,
  `policy/policy.h` and
  `policy.cpp` (`MAX_STANDARD_TX_SIZE`, dust), `script/vm_limits.h`,
  `script.h`, `init.cpp` (`-acceptnonstdtxn`), `chainparams.cpp`
  (`fRequireStandard`, upgrade heights);
  [release notes 23.0.0](https://docs.bitcoincashnode.org/doc/release-notes/release-notes-23.0.0/);
  [`bitcoind` options](https://docs.bitcoincashnode.org/doc/cli/bitcoind/)
  (`-minrelaytxfee`, `-dustrelayfee`, `-percentblockmaxsize`).
- Upgrade specifications: [2018-05 opcodes](https://upgradespecs.bitcoincashnode.org/may-2018-reenabled-opcodes/),
  [2018-11 CTOR](https://upgradespecs.bitcoincashnode.org/2018-nov-upgrade/),
  [2020-05 SigChecks](https://upgradespecs.bitcoincashnode.org/2020-05-15-sigchecks/),
  [2021-05](https://upgradespecs.bitcoincashnode.org/2021-05-15-upgrade/),
  [2022-05 introspection](https://upgradespecs.bitcoincashnode.org/2022-05-15-upgrade/),
  [2023-05 CashTokens and P2SH32](https://upgradespecs.bitcoincashnode.org/2023-05-15-upgrade/),
  [2025-05 VM limits](https://upgradespecs.bitcoincashnode.org/2025-05-15-upgrade/)
  ([CHIP](https://github.com/bitjson/bch-vm-limits)),
  [2026-05](https://upgradespecs.bitcoincashnode.org/2026-05-15-upgrade/)
  ([P2S](https://github.com/bitjson/bch-p2s), [Loops](https://github.com/bitjson/bch-loops),
  [Functions](https://github.com/bitjson/bch-functions), [Bitwise](https://github.com/bitjson/bch-bitwise)),
  [CashTokens](https://github.com/cashtokens/cashtokens),
  [ABLA, CHIP-2023-04](https://gitlab.com/0353F40E/ebaa).
- Other: [Elements opcodes](https://github.com/ElementsProject/elements/blob/master/doc/tapscript_opcodes.md)
  (a resumable hash), [BIP119](https://github.com/bitcoin/bips/blob/master/bip-0119.mediawiki)
  (congestion control trees),
  [0zkbrewer BCH-FRI-STARK-Verifier](https://github.com/0zkbrewer/BCH-FRI-STARK-Verifier)
  (multi-input verifier and its binding audits),
  [OCEAN DATUM gateway](https://github.com/OCEAN-xyz/datum_gateway)
  (per-device coinbase sizes).
