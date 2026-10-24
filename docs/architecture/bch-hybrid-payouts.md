**The architecture files are LLM generated. The goal is make it easier
for new comers to the code base and other LLMs to quickly understand
the design and remain consistent with the design choices.**

# Hybrid payouts on Bitcoin Cash

**Status: Approved 2026-10-23 and implemented on PR #33.** The payout split,
tree covenant and builder, storage, API and maturity executor are implemented;
the tree has an end-to-end BCHN regtest test. Chipnet, ASIC firmware limits
and large-pool propagation remain unmeasured. Sections 7 and 8.1 describe
future claim and transport improvements separately from the implemented rule.

This rule replaces the former rule 8 of
[payout-distribution.md](payout-distribution.md) (the 11-payout cap and the
weighted tail draw, which the owner rejected as a Bitcoin assumption). It
changes only P2Pool's own share-chain rules and the coinbase P2Pool builds.
BCHN never sees share-chain rules, and nothing here changes BCH consensus.

## In plain words

When the pool finds a Bitcoin Cash block, the block's first
transaction (the coinbase) pays the miners. Every P2Pool node rebuilds that
coinbase from the share chain and rejects a share whose payouts differ. The
rule pays every payee directly within the share's coinbase limit and pays
the rest through a payout tree. The former 11-payout cap and lottery, based
on Bitcoin pool firmware evidence, have been removed.

**Two kinds of share.** Each BCH coinbase built by P2Pool starts with a small
merge-mining record (output 0, 53 bytes). It is either *empty* (no tokens are
being merge-mined) or it names token work.

1. **No tokens (the normal case).** The coinbase pays every miner directly.
   There is no P2Pool cap and no lottery. The only bound is Bitcoin Cash's
   own: one transaction, the coinbase included, may be at most 1,000,000
   bytes (BCH consensus, `MAX_TX_SIZE`). That is about 29,400 miners. If a
   window ever has more, the rest are paid through the payout tree below,
   with the same rule as token shares. Block size is never the limit: ABLA
   gives at least 32 MB and grows, and BCHN decides it.
2. **Tokens.** A token claim has to re-hash the whole coinbase inside the
   BCH script machine, which takes at most 10,000 bytes per input. So a token
   share keeps its coinbase within **T = 3,072 bytes**. That fits about 77
   miners paid directly. Everyone past that point is paid through a
   **payout tree**.

**Who is paid directly.** Payees are lined up from the **smallest** payout to
the **largest**. They are paid directly in that order until the next one
would not fit under the limit. That payee, the one that straddles the line,
and everyone after it go into the tree. Nobody is split in two, and the line
always rounds down: a few unused bytes may stay below the limit. Small
miners come first because they are the ones a fee would hurt; the tree's
fee is a few dozen satoshis, which matters little on a large payout.

**The payout tree.** It is one output in the coinbase that locks the tree
payees' money with a script (a "covenant"). Nobody holds a key to it. After
the coinbase's normal 100-block wait, anyone can spend it, but only in one
exact way: into exactly the outputs the share chain decided, to the satoshi.
A tree of up to 500 payees is one transaction; more payees take a few
transactions in a row. Every P2Pool node broadcasts them automatically, and
so can any payee. The executor can steal nothing and pays nothing.

**What it costs.** The tree pays its own BCH fee out of the tree's money:
every node executes at 1 satoshi per byte. The root also permits the
precomputed 2 and 3 sat/byte variants. Each payee funds its own output and
an equal share of the remaining transaction bytes; amounts depend on the
tree's shape and script sizes. Only token shares, or windows past about
29,400 miners, use the tree. Direct payouts cost nothing extra.

**Another limit of P2Pool's own.** Each share carries its coinbase between
P2Pool nodes. The old 200 KiB combined body limit came from upstream, a
Bitcoin pool, and would have capped direct payouts at about 6,000 miners.
The implemented share limit allows a full 1,000,000-byte coinbase
(BCH's own transaction limit; a coinbase is never relayed by BCHN, so the
100 KB relay limit does not apply) plus the share's own 200 KiB of share
transactions and its header. That stays one fixed share-chain constant. The
cost is bandwidth: a share with 29,400 payees is about 1 MB, made about every
10 seconds, and on a slow uplink it reaches peers late, so more shares end
up as uncles. The fix is in how shares travel, not in the limit: every node
already rebuilds the payouts itself, so shares can be sent without them, a
few hundred bytes each, with the full coinbase fetched only when needed.

**What can go wrong.**
- *Fees rise above 1 sat/byte.* The default executor keeps rebroadcasting the
  1 sat/byte variant; anyone may broadcast the 2 or 3 sat/byte variant before
  another variant is accepted. Above 3 sat/byte, the tree waits for relay or
  direct inclusion in a block. The money stays locked to the payees.
- *Every P2Pool node goes offline.* The money waits in the tree, with no
  deadline. Anyone with the tree's data can run it later. Nodes keep that
  data for every block, and the API serves it so payees can save their own
  copy.
- *A miner commits a token record only to force the tree on others.* It costs
  each tree payee its share of the fee and a short wait after maturity.
  It cannot redirect money.
- *Reorganisations.* A BCH reorg deeper than 100 blocks would hurt every
  coinbase alike. Share-chain reorgs do not matter once the BCH block exists:
  the tree is fixed in that block.

**What BCHN sees.** Ordinary coinbases, sometimes with one P2SH32 output, and
later ordinary standard transactions with no signatures. All of it uses
features BCH already activated: introspection (2022), P2SH32 (2023), the VM
limits (2025) and loops with 10,000-byte unlocking scripts (2026). No BCH
consensus change.

**Growing T later.** The owner asked whether claim data could be spread
across several 10 KB inputs in one 100 KB transaction. Data can, but hashing
is the catch: BCH's SHA-256 opcode hashes one stack item of at most 10,000
bytes and cannot continue a half-finished hash. So the cheap path stops near
10 KB of coinbase. Beyond that, SHA-256 would have to be written by hand in
script; our estimate is that each 10 KB input then hashes only about half a
kilobyte, so a 100 KB transaction adds only about 5 KB (a prototype must
measure this). A pool that finds a block may also put a claim of up to 1 MB
into it itself (BCH only relays 100 KB, but accepts 1 MB inside a block);
that helps only when this pool finds the block. Better long-term options:
move the token record to the end of the coinbase, where P2Pool's existing
midstate trick makes claims independent of coinbase size, or a future BCH
opcode that can continue a hash. T stays 3,072 now. Any raise is a new
protocol tag.

The approved choices and remaining follow-ups are listed in
[section 10](#10-approved-choices-and-follow-ups).

## 1. Decisions this design follows

From the owner, 2026-10-23:

1. A share whose merge-mining commitment is the exact empty one pays every
   miner directly in the coinbase. No P2Pool cap, no lottery. ABLA handles
   block size and BCHN is the authority.
2. A share that carries token commitments keeps its coinbase within a
   threshold T so its token claim fits in the BCH VM. Payees past T are paid
   through a covenant payout tree: "tree only for tokens after the
   threshold".
3. A deterministic rule decides whether the payee that straddles the
   threshold is paid directly or through the tree.
4. T is one fixed share-chain constant carried in the protocol tag. The
   auditor recommends 3,072 bytes. Tree-paid payees fund their own
   withdrawal fees.
5. Check what limits really bind a claim spread across inputs, and how T
   could grow.
6. Merge-mining common format v2, as approved: a 53-byte `OP_RETURN`
   `"CTMM" 02 root h salt` commitment at coinbase output 0, salted category
   slots, the `0x00`/`0x01` SHA-256 tree, a 169-byte leaf, a proof with
   coinbase branch depth `d <= 25`, and an exact empty commitment when no
   tokens are mined.

Added in review (2026-10-23): BCH's 1,000,000-byte transaction limit also
bounds shares without tokens, so they need the same rule past that point.
The owner directed that P2Pool's share body limit allow a full 1,000,000-byte
coinbase plus the share's own overhead, instead of a fixed 200 KiB.

### Format v2 in brief

Agreed between the P2Pool and Pickaxe sessions and approved by the owner on
2026-10-23. Implemented in `shares/merge_mining.rs` (output 0) and
`shares/auxpow.rs` (the tree and `AuxProof` v2), with the ASERT header check
in `shares/bch_asert.rs`; P2Pool matches Pickaxe's golden vectors byte for
byte. See [Auxiliary work proofs](../bch-auxpow.md).

```text
output 0, 53 bytes: value 0 (8) | 2c | 6a 2a | "CTMM" | 02 | root (32) | h (1) | salt (4, LE)
empty commitment:   root = 32 zero bytes, h = 0, salt = 0
slot:               u32le(SHA256(0x02 || salt || category)[0..4]) mod 2^h, h <= 16
tree:               leaf = SHA256(0x00 || preimage); node = SHA256(0x01 || left || right)
leaf preimage:      "CTML" ver category lock_hash state_hash payout_hash target_bits(4) ext  (169 bytes)
proof:              header (80) | h | salt | aux branch (32h) | coinbase head (47 + L, L <= 100)
                    | coinbase tail | d | coinbase branch (32d), d <= 25
```

A covenant rebuilds output 0 and the leaf itself; the proof never carries
them. The coinbase head is everything before output 0 (version, the single
null input with its scriptSig, the one-byte output count) and the tail is
everything after it. Pool-built jobs (Stratum V1 and Stratum V2 standard
channels) carry the empty commitment; only a miner that declares its own
coinbase through Job Declaration commits token work, and the share header
carries its root, `h` and salt so every peer rebuilds output 0.

## 2. Facts it rests on

Checked against BCHN master and the BCH upgrade specifications on
2026-10-23. Sources are listed at the end.

| Fact | Value | Kind | Source |
|---|---|---|---|
| Largest transaction, coinbase included | 1,000,000 bytes (`MAX_TX_SIZE`) | BCH consensus | `consensus.h`; `tx_check.cpp` (`CheckCoinbase` calls `CheckTransactionCommon`, `bad-txns-oversize`) |
| Limit on coinbase outputs | none, apart from the size limit | BCH consensus | `tx_check.cpp` (only `bad-txns-vout-empty`) |
| Coinbase scriptSig | 2 to 100 bytes | BCH consensus | `consensus.h`, `tx_check.cpp` (`bad-cb-length`) |
| Coinbase maturity | 100 blocks | BCH consensus | `consensus.h` (`COINBASE_MATURITY`) |
| Tokens on coinbase outputs | never | BCH consensus | CashTokens spec; `validation.cpp` |
| Block size | ABLA, at least 32,000,000 bytes on mainnet; BCHN's 2 GB guard is temporary | BCH consensus | `consensus.h`, CHIP-2023-04 |
| Largest standard transaction | 100,000 bytes (`MAX_STANDARD_TX_SIZE`) | relay policy | `policy.h` |
| Unlocking bytecode per input | 10,000 bytes, standard and consensus since May 2026 | both | CHIP-2024-12 P2S; `policy.cpp`; `vm_limits.h` |
| Largest stack item | 10,000 bytes | BCH consensus | CHIP-2021-05 VM Limits; `vm_limits.h` |
| Operation cost per input | at most `(41 + unlocking length) * 800` | both | VM Limits CHIP |
| Hash digest iterations per input | at most `(41 + L) / 2` standard, `(41 + L) * 7 / 2` consensus; a message of `M` bytes costs `1 + (M + 8) / 64`, plus 1 if double-hashed | both | VM Limits CHIP |
| Opcode cost | 100 per opcode, plus pushed bytes; arithmetic `+ 2 * result length`; `OP_MUL`/`DIV`/`MOD` also `+ a.length * b.length`; 192 per hash iteration standard, 64 consensus | both | VM Limits CHIP |
| Loops, functions, bitwise shifts | `OP_BEGIN`/`OP_UNTIL`, `OP_DEFINE`/`OP_INVOKE`, `OP_LSHIFTBIN`/`RSHIFTBIN`/`LSHIFTNUM`/`RSHIFTNUM`, `OP_INVERT` since 2026-05-15 (mainnet from block 951,145) | BCH consensus | 2026-05-15 upgrade spec; `chainparams.cpp` |
| `OP_AND`/`OR`/`XOR` | operands must be the same length | BCH consensus | May 2018 opcode spec |
| SHA-256 that resumes from a midstate | no such opcode | BCH consensus | opcode list in `script.h`; no CHIP locked in |
| Dust threshold | `3 * dustrelayfee * (output size + 148)`: 546 P2PKH, 540 P2SH20, 576 P2SH32 | relay policy, for outputs a relayed transaction creates | `policy.cpp`, `policy.h` |
| Dust in the coinbase | allowed: the coinbase is never relayed, and blocks are not checked for standardness | BCH consensus | `validation.cpp` (`IsStandardTx` only in mempool acceptance) |
| Minimum relay fee | 1 sat/byte by default; size counts as `max(size, SigChecks * 50)` | relay policy | `bitcoind` options; SigChecks spec |
| Unconfirmed chains | no length limit since 2021-05-15; BCHN removed child-pays-for-parent mining in v23.0.0 | relay and mining policy | BCHN 23.0.0 release notes |
| Non-standard transactions in the mempool | refused on mainnet (`-acceptnonstdtxn` is test chains only) | node option | `init.cpp`, `chainparams.cpp` |
| P2SH32 output | 44 bytes (35-byte locking bytecode) | | CHIP-2022-05 |
| Next scheduled upgrade time | 2027-05-15 mainnet, 2026-11-15 chipnet; no CHIP is locked in for it | | `chainparams.cpp` (`upgrade2027ActivationTime`) |

P2Pool's own limits that this design touches:

| Limit | Value | Where | Origin |
|---|---|---|---|
| Share transactions, including their share coinbase | 200 KiB | `BLOCK_TXS_SIZE_LIMIT`, `shares/validation/mod.rs` | Retained for the share chain's own transactions |
| BCH share body, including the carried BCH coinbase | 1,204,800 bytes | `BCH_SHARE_BODY_SIZE_LIMIT`, `shares/validation/mod.rs` | 1,000,000-byte coinbase plus 200 KiB of share transactions |
| BCH P2P message | 1.25 MiB | `MAX_P2P_MESSAGE_SIZE`, `node/messages.rs` | Holds the full share body, header and merkle branch |
| Share spacing | about 10 seconds | `IDEAL_BLOCK_TIME`, `sim_overrides.rs` | Upstream |

## 3. The payout rule

Everything here is a share-chain rule: the producer and every validator
build the same coinbase byte for byte (`validate_bitcoin_payout`). The
window, payout rights and cuts remain as described in
`payout-distribution.md`; this batch also changes BCH weights from rounded
difficulty to exact work (audit A17). The canonical split and tree replace
the former rules 7 and 8.

### 3.1 Token shares and no-token shares

A share is a **no-token share** when its merge-mining commitment is exactly
the empty one: root of 32 zero bytes, `h = 0` and salt 0. Any other value,
including a zero root with a non-zero salt, is non-empty for payout
classification. The merge-mining format validator independently rejects a
zero root unless both `h` and salt are zero.

### 3.2 Payees and their order

After rules 5 to 7, each payee `i` has a locking bytecode, a weight `w_i`,
an amount `a_i = floor(A * w_i / W)` and an output of `s_i` bytes: 34 for
P2PKH, 32 for P2SH20, 44 for P2SH32. On BCH a payee must be one of these
three; every other payout script in a share header is invalid. This includes
Bitcoin witness programs, which cannot be spent on BCH.

**Canonical order:** ascending weight, ties by locking bytecode compared as
bytes. The smallest payee comes first.

Why smallest first:

| Order | Effect | Verdict |
|---|---|---|
| Largest first | The tree holds the small payees: its fixed per-payee fee is the largest fraction of their money, and small tree outputs can fall below dust and cannot be relayed. | Rejected |
| Address or script order | Unrelated to size; a payee can grind an address to land in the direct part. | Rejected |
| Smallest first | Small payees are paid in the coinbase with no fee. Tree payees are the largest, so the fee is a tiny fraction and tree outputs stay far above dust. A large miner splitting into many small addresses to jump the line saves under 100 satoshis per address and costs others about the same. | Adopted |

### 3.3 The coinbase limit `C`

| Share | `C` | Why |
|---|---|---|
| No-token share | `D` = 1,000,000 bytes | BCH consensus `MAX_TX_SIZE`. Not a P2Pool number. |
| Token share | `T` = 3,072 bytes | The token claim must hash the coinbase in the VM (section 4). |

`S` = 445 bytes is the most a P2Pool coinbase can take besides its PPLNS
payouts. It uses the worst case of every field, so `C - S` is always safe:

| Part | Bytes |
|---|---|
| Version, input count, outpoint, scriptSig length, sequence, one-byte output count, lock time | 51 |
| scriptSig (BCH's maximum) | 100 |
| Output count taking 3 bytes from 253 outputs | 2 |
| Output 0, the CTMM commitment | 53 |
| Donation (P2PKH) | 34 |
| Operator fee (largest BCH output, P2SH32) | 44 |
| Tree output (P2SH32) | 44 |
| Padding before the share commitment (at most 63 bytes of padding) | 74 |
| Share commitment output | 43 |
| **Total** | **445** |

The payout budget is `P = C - S`: 2,627 bytes for a token share (77 P2PKH
payees) and 999,555 bytes for a no-token share (29,398 P2PKH payees).

### 3.4 The straddle rule (exact)

Walk the payees in canonical order.

1. **Everyone fits.** If the payees' outputs total at most `P + 44` bytes
   (the budget without a tree output), every payee is paid directly and the
   coinbase has no tree output.
2. **Otherwise** payee `j` is paid directly when the outputs of payees `1`
   to `j` total at most `P`. The first payee for which that fails, the
   straddler, and every payee after it go to the tree.
3. **Round down, never split.** The straddler always goes to the tree, even
   if it is over by one byte. A payee is never split between a direct output
   and a tree leaf. No later payee is moved back into the direct part, even
   if its smaller output would fit the bytes left over (at most 43). Those
   bytes stay unused.
4. **Token shares** also need fewer than 253 coinbase outputs, so the output
   count is one byte where the covenant expects it. With `T` = 3,072 a coinbase
   has at most 88 outputs (83 P2SH20 payouts and 5 others, or 82 and 6 with
   the tree output), so this always holds; validators check it anyway.

So the coinbase never exceeds `C`, and which payees are direct is a pure
function of the weights, the amount and the payees' scripts.

### 3.5 Remainder and dust

- **Remainder.** `A - sum(a_i)` (fewer satoshis than there are payees) is
  added to the last payee in canonical order, the largest, wherever it is
  paid.
- **Direct payouts below dust.** Paid as they are. BCH consensus accepts a
  coinbase output of any value, and dust is relay policy for outputs that a
  relayed transaction creates. Such an output can still be spent later as
  an input. At today's subsidy one share in a 120,960-share window earns
  about 2,500 satoshis per block, so this needs about three more halvings
  (around 2036) unless fees grow, or a share far below the window's average
  difficulty.
- **Tree leaves below dust.** A tree output must meet dust or its
  transaction is not relayed. A tree payee whose net amount (section 5.3) at
  3 sat/byte, the highest permitted rate, is below its dust threshold is
  removed from the tree and its `a_i` is added to the last payee. Repeat
  until no leaf is below dust; the count only falls, so this ends. Since tree
  payees are the largest, this needs every payee below about 700 satoshis.

### 3.6 Coinbase layout

Output 0: the CTMM commitment. Then the donation, the operator fee, the
direct payouts in canonical order, the tree output if any, the padding and
the share commitment last, as today. BCH sets no rule on output order inside
a transaction (CTOR orders transactions, not outputs), so this order is the
share chain's own choice.

## 4. Why T = 3,072 bytes

The claim is the token transaction that proves a share's work. Its unlocking
bytecode carries everything in the proof except output 0, which the covenant
rebuilds, plus the token's redeem script.

The auditor's worst case for the parts besides the coinbase is about 2,535
bytes: header 80, aux branch at `h = 16` (512), coinbase branch at `d = 25`
(800), a redeem script of about 1.1 KB, `h`, salt, `d` and push opcodes.

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
variant's growth.

VM cost at `T` (estimate): budget `(41 + 5,554) * 800` = 4.48M operation
cost. Hashing is about 175 digest iterations: the coinbase about 50, the
coinbase branch 75, the aux branch 32, the leaf, header and state hashes about
18. The standard limit is `(41 + 5,554) / 2` = 2,797. The claim transaction
is about 6 KB, about 6,000 satoshis at 1 sat/byte. For scale, the SAFA
variant's regtest withdrawal had a 1,266-byte scriptSig and passed BCHN 29.2.0
(`tools/safa-auxpow/README.md`).

A deployed token covenant cannot change. **T must never exceed what the
deployed token covenants can claim.** That is why any raise is a new protocol
tag and stays at or below 6,500 unless every deployed token was written for
more.

## 5. The payout tree

### 5.1 Shape

- **Leaves** are the tree payees in canonical order.
- **Fan-out** `F` = 500 outputs per node transaction (checked by covenant
  VM tests; large-pool measurements remain open, section 5.6).
- Leaves are grouped in order, `F` per node; the last group may be smaller.
  If there is more than one node, the nodes are grouped `F` per parent the
  same way, until one root remains.
- Up to 500 tree payees: one transaction. Up to 250,000: two levels
  (`1 + ceil(n / 500)` transactions).
- Each node is a P2SH32 output whose redeem script commits to that node's
  exact outputs. The coinbase holds the root.

The tree is built bottom-up, like a merkle tree, so the root's P2SH32 hash
commits to every output below it. Validators rebuild the whole tree to check
the coinbase's root output.

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

What each check stops:

| Check | Stops |
|---|---|
| Exact outputs (`H`, `k`) | Any change to a payee, an amount, or an extra output for the executor |
| One input | An executor adding inputs and outputs to skim the fee allowance |
| `V` | Spending a look-alike UTXO someone sent to the same address with another value |
| No token on any output | An executor minting a token (a node that spends output 0 of its parent is a token genesis input) and attaching it to payees' outputs |
| Version, lock time, sequence, exact zero filler | Variants of the same transaction. Minimal pushes fix the unlocking form, so every txid at each permitted fee rate is known as soon as the block is found |

There are no signatures and no keys. The root permits three precomputed
transactions, one for each fee rate; after a root variant is selected, its
descendant transactions are fixed. BCH is first-seen with no replace-by-fee,
so another variant cannot replace a root already accepted by a node.

This is the "congestion control tree" idea (BIP119 on Bitcoin, which needs a
new opcode there). On BCH the 2022 introspection opcodes and the 2026 loop
opcodes already do it.

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
  the rest: `R` times its size plus a rounding excess below `n` satoshis.
- So the tree's total fee is at most `R * B + n - 1` satoshis, it comes only
  from the tree's own value, and the executor neither pays nor receives
  anything.

### 5.4 Dust

- **Coinbase outputs:** BCH consensus allows any value, and dust rules never
  apply to a block's transactions (section 2).
- **Tree outputs:** each node transaction is relayed, so each output must be
  at least its dust threshold: 546 (P2PKH), 540 (P2SH20), 576 (P2SH32), and
  a child node's P2SH32 output too. Section 3.5 removes leaves that would fall
  below at the highest permitted rate, 3 sat/byte, before building all three
  variants. The final payee receives any merged value. Child nodes hold the
  sum of their leaves.

### 5.5 Who executes it, and when

- The root is a coinbase output, so BCH consensus lets it be spent only in a
  block at least 100 blocks after the block that created it
  (`bad-txns-premature-spend-of-coinbase`). Direct payouts wait the same 100
  blocks, so the tree adds only the time to confirm its own transactions.
- After that anyone may broadcast the node transactions, in any order once
  each parent is known. BCH has no limit on unconfirmed chains since May
  2021, so a whole two-level tree can be broadcast at once.
- **Every P2Pool node runs an executor by default.** It needs no keys and no
  money. It asks BCHN for its tip every 30 seconds; on each new tip it
  broadcasts the 1 sat/byte transactions of every tree whose coinbase has
  100 confirmations, root first, finishes the children of a 2 or 3 sat/byte
  root someone else broadcast, and rebroadcasts until the tree has looked
  settled for 6 tips in a row. All nodes produce identical transactions, so
  duplicates are harmless.
- **Data.** When a node accepts a share that is a BCH block, it stores that
  block's tree payees and the block's coinbase txid, root output, height and
  hash, independent of the share's later share-chain status, and rebuilds
  the node transactions from them. It keeps the data after execution too
  and never prunes it. The data is about 50 bytes per tree payee.
- **API.** The node serves each tree's transactions, so a payee or a wallet
  can save them and broadcast them without P2Pool.

### 5.6 Sizes and VM cost (estimates)

The filler `Z` is 8 zero bytes per output. It buys operation-cost and hashing
budget, since the budget grows with the unlocking length. The covenant
checks the filler's exact bytes, preventing transaction-ID changes through
otherwise unused unlocking data. Rust VM tests measure both budgets at
500 outputs; BCHN regtest checks the resulting transactions' relay and
execution behavior.

The loop uses about 3,000 operation cost and two hash iterations per output.
For 500 P2PKH outputs `test_utils::bch_vm` measures 1.51M of the 3.32M
operation cost allowed and 1,002 of the 2,076 standard hash iterations, 45%
and 48%. Each output's 8 filler bytes buy 6,400 operation cost and 4 hash
iterations. A 500-output P2PKH transaction is
about 21 KB, below the 100,000-byte standard size; its filler is 4,000
bytes, below the 10,000-byte unlocking limit after the redeem script and
pushes are added. SigChecks are zero. Exact serialized sizes and fees are
computed by the builder, rather than taken from these estimates.

The BCH block carries one more output for the tree: 44 bytes.

## 6. Failure modes

| Case | What happens | Mitigation |
|---|---|---|
| Fees rise above 1 sat/byte | The default executor keeps the 1 sat/byte variant; a root not already accepted may instead be spent with its 2 or 3 sat/byte variant. Above 3 sat/byte, ordinary relay may wait. | BCH consensus has no minimum fee: a miner may include a tree directly. BCH first-seen policy prevents replacing a root already accepted at another rate. A later share-chain rule can alter the rates for new trees. |
| Partial execution | Some nodes confirmed, others not | Each node is independent once its parent exists. Anyone can finish later. Nothing expires. |
| A miner forces the tree (a format-valid token commitment) | Tree payees fund their share of the fee and wait for tree confirmation after maturity | Cannot redirect money. Only declared coinbases can carry a commitment (pool-built jobs carry the empty one). |
| Executor misbehaves | Cannot change outputs, add inputs, attach tokens or vary the transaction | Covenant checks (5.2) |
| Dust sent to a node's address | A second UTXO with the same script | Spendable only with exactly `V`, so a different value is stuck and harms nothing; an exact copy pays the payees twice, a gift |
| BCH reorg below 100 blocks | Before maturity the block may be orphaned, as for direct payouts. After execution, reorged node transactions return to the mempool. | Executor rebroadcasts. Txids do not change. |
| Share-chain reorg | The share that became the BCH block may later leave the share chain | The tree is fixed in the BCH block. Nodes store it by coinbase txid, not by share-chain status. |
| All P2Pool nodes offline or lose data | Money waits in the tree | No deadline. Any copy of the tree data is enough: every node keeps it, the API serves it, payees can keep their own. An archival node can also rebuild it from the share chain. |
| BCH miners censor tree transactions | Same as censoring any transaction | Another miner includes them. |

## 7. Can claims grow past T?

### 7.1 What binds

- **Unlocking bytecode: 10,000 bytes per input**, standard and consensus.
  A claim's data can be spread over several inputs: an input can read
  another input's unlocking bytecode with `OP_INPUTBYTECODE`.
- **Stack item: 10,000 bytes.** `OP_SHA256` and `OP_HASH256` hash one stack
  item, and BCH has no opcode that resumes SHA-256 from a midstate. So the
  whole coinbase must be one item of at most 10,000 bytes to hash it with the
  native opcode. This is the binding limit.
- **Operation cost: `(41 + L) * 800` per input**, so each input's work grows
  with its own unlocking length, never with the transaction's.
- **Transaction size:** 100,000 bytes to relay, 1,000,000 bytes in a block.

### 7.2 The ladder

| Step | Coinbase limit | How | Cost |
|---|---|---|---|
| Now | `T` = 3,072 | One-input claim, native hashing | none |
| 1 | up to 6,500 | Same claims, new protocol tag | Only if every deployed token accepts it |
| 2 | about 9,900 | Two-input claim: the coinbase tail sits in a second input, input 0 reads it with `OP_INPUTBYTECODE` and hashes the whole coinbase as one 10,000-byte item | The token covenant must be written this way from the start |
| 3a | tens of KB | SHA-256 written in script, continuing a midstate across inputs (7.3) | About half a kilobyte per 10 KB input (estimate) |
| 3b | no limit | Format v3: put the CTMM output in P2Pool's 64-byte-aligned tail beside the share commitment; the claim carries a midstate and about 100 bytes of tail and compresses only 2 blocks in script | One new merge-mining format version |
| 3c | no limit | A BCH upgrade with a resumable SHA-256 opcode (Elements has `OP_SHA256INITIALIZE`/`UPDATE`/`FINALIZE`) | Up to BCH, not P2Pool |

Step 3b is the strongest. P2Pool's own share proof already works this way
(`shares/coinbase_proof.rs`: a SHA-256 midstate of a 64-byte-aligned prefix
plus a fixed tail), as do classic p2pool's hash link and RSK. For a claim it
is sound because the claim only proves SHA-256 work on a header that commits
to the root, which is all Case A ever proves (the VM cannot see whether a
header is a real BCH block). The fixed-length tail ending at the end of the
transaction pins the commitment's position, so a pool cannot slip a second
root into the prefix. Two compression blocks cost about 1.4M to 2.4M
operation cost (7.3), within one input's budget at about 3 to 4 KB of
unlocking (a little filler if needed). Claims would then
no longer depend on coinbase size, and token shares could pay every miner
directly: `T` and the tree would go away. It changes the approved v2 format,
so it needs the owner's and Pickaxe's agreement and a prototype.

The last approved review of v2 assumed the commitment at output 0, where a
midstate would only skip the head, and so recommended revisiting a midstate
only if coinbase ceilings grew past about 3 KB. `T` = 3,072 is at that
point.

### 7.3 SHA-256 in script: an estimate

An earlier estimate in review put one 64-byte compression block at about
0.2M operation cost. Counting opcodes for one round gives about five times
that. Words are
4-byte big-endian strings for the bitwise opcodes and are turned into
numbers for addition (`OP_REVERSEBYTES`, append `00`, `OP_BIN2NUM`) and back
(`2^32 OP_MOD`, `OP_NUM2BIN`, `OP_SPLIT`, `OP_DROP`, `OP_REVERSEBYTES`).
A rotation doubles the word (`OP_DUP OP_CAT`) and shifts it with
`OP_RSHIFTBIN`.

| Part of one round | Opcodes (estimate) |
|---|---|
| `Sigma1(e)`: pick, double, three shifts, two XORs, keep the low word | 17 |
| `Ch(e,f,g) = g ^ (e & (f ^ g))` | 11 |
| `Sigma0(a)` | 17 |
| `Maj(a,b,c) = (a & b) \| (c & (a \| b))` | 14 |
| Seven words to numbers (`h`, `Sigma1`, `Ch`, `W_t`, `Sigma0`, `Maj`, `d`) | 28 |
| Seven additions, `K_t` pushed as a number before `OP_INVOKE` | 10 |
| Two numbers back to words (`a'`, `e'`) | 16 |
| Picks and reshuffling the eight state words | 12 |
| **One round** | **about 125** |

| Per 64-byte block | Opcodes |
|---|---|
| 64 rounds | 8,000 |
| 48 message-schedule words at about 70 each (`sigma0`, `sigma1` with a shift, four conversions, three additions) | 3,360 |
| Splitting the block and the final eight additions | 170 |
| **Total** | **about 11,500** |

At 100 base plus about 5 pushed bytes per opcode that is about 1.2M
operation cost per block. A careful implementation might reach 0.7M. Even
counting only the operations themselves, with no stack moves or type
conversions (six rotations at three opcodes each, seven additions, about
eleven logic opcodes: about 36 per round, and about 21 per schedule word),
a block takes about 3,300 opcodes, 0.33M. So 0.2M is below the floor. The
auditor's earlier review also put it near 1M. **This is an estimate; a
prototype must measure it.**

What follows, at 0.7M to 1.2M per block and an input budget of 8.03M (10,000
bytes of unlocking) less about 0.3M of linking checks:

| Claim transaction | Inputs | Coinbase hashed in script |
|---|---|---|
| 100,000 bytes (standard) | about 10 | 4 to 7 KB |
| 1,000,000 bytes (consensus, not relayed) | about 100 | 40 to 70 KB |
| Same, if 0.2M per block held | | about 25 KB and 250 KB |

So at standard size, chaining in script hashes less than native hashing
(7.5 to 10 KB) and costs a 100 KB fee (about 100,000 satoshis per claim). It
pays off only for a 1 MB claim a pool mines itself, and step 3b does better
at any size.

### 7.4 Option: P2Pool puts its own large claim into the block it finds

The miner that finds a BCH block decides what it contains. Neither the
coinbase nor a transaction the winner adds itself is ever relayed on its
own, so only BCH consensus binds them: 1,000,000 bytes per transaction.
That lets a P2Pool node mine a claim larger than the 100,000-byte relay
limit, for example a 1 MB chained claim (7.3).

What BCHN allows (BCHN master):

- BCHN has no option to raise the 100,000-byte standard size
  (`MAX_STANDARD_TX_SIZE` is a constant in `policy.h`).
- `-acceptnonstdtxn` is refused on mainnet; only test chains allow it
  (`init.cpp`). It is on by default on testnet3 and scalenet, but off by
  default on chipnet, testnet4 and regtest (`fRequireStandard = true` in
  `chainparams.cpp`), so a chipnet node needs `-acceptnonstdtxn=1` to hold
  and template such a transaction.
- `getblocktemplate` is built from BCHN's own mempool, so on mainnet it never
  includes a non-standard claim.
- `submitblock` checks consensus only (`ConnectBlock` never calls
  `IsStandardTx`), so a block that carries one is valid.

So on mainnet P2Pool itself would add the claim to each template it gets
from BCHN:

1. Insert the transaction in CTOR position (ascending txid), keeping the
   template's own strict-order check (`validate_bch` in
   `stratum/work/block_template.rs`) satisfied.
2. Recompute the coinbase merkle branch carried by every job and share
   (`template_merkle_branches`).
3. Count its size against the template's `sizelimit` together with the
   coinbase (`validate_bch_block_size`), and its SigChecks against the
   block's `sizelimit / 141`.
4. Pay it no fee, or add its fee to the coinbase value the payout rule
   splits.

**Cost.** Every template refresh rebuilds the merkle branch and resends
jobs to all miners, as a new template does today; the claim's bytes ride in
every block template until a block is found. **Limits.** It confirms only
when this node's pool finds a block, which for a small pool may take days,
and the claim must stay valid until then (the token's own state may move
on). It needs no share-chain rule: peers check only the coinbase and its
merkle branch, never which other BCH transactions a job carries, so it is a
node-local option. Job Declaration's Full-Template mode already lets a miner
choose its own transactions. Not built; nothing in P2Pool does this today.

### 7.5 Binding rules for multi-input claims

From the multi-input on-chain verifier work the owner has studied and forked
(0zkbrewer's BCH-FRI-STARK-Verifier: a 28-input, 92,191-byte chipnet spend in
block 314,510, and the audits that found forged accepts in it), for steps 2
and 3a:

1. **Bind the locking script that runs, not only the bytes read.** Input 0
   checks `OP_UTXOBYTECODE` of every helper input against the helper
   covenant's P2SH32. Bytes in an unlocking script prove nothing about the
   code that ran.
2. **Bind every input another input reads,** anchored at input 0: the first
   chunk starts from the SHA-256 initial value and contains the head and
   output 0 that input 0 rebuilt; each helper reads its predecessor's
   midstate and checks that predecessor's locking bytecode; input 0 reads the
   final digest from the last helper and folds it as the coinbase txid.
3. **Test soundness by proving a false statement from scratch,** for example
   a claim for a coinbase that does not contain the committed root, built
   honestly end to end. Breaking one hash in an honest proof is not a
   soundness test.
4. Budget the binding overhead before claiming a size: in that work the sound
   version was about 30% larger than the demo.

## 8. What changes where

| Item | P2Pool share-chain rules (BCHN never sees them) | BCH block and transactions (what BCHN sees) |
|---|---|---|
| Payout split | Section 3 replaces rule 8: no cap, no draw, the straddle rule and the tree | No-token coinbases may have thousands of outputs; token coinbases have one P2SH32 tree output |
| Constants | `T` 3,072, `D` 1,000,000 (BCH's own), `F` 500, fee rates 1 through 3, `Z` 8 per output and covenant version 1; tag segment `payout-hybrid-t3072-f500-r1to3-z8-cov1` replaces `payout-cap11` | none |
| Merge-mining commitment | From v2: header fields root, `h`, salt; the exact empty commitment | Output 0, 53 bytes, value 0 |
| Payee scripts | P2PKH, P2SH20 and P2SH32 only on BCH | none |
| Token shares | Fewer than 253 coinbase outputs; coinbase at most `T` | none |
| Share body | A fixed constant: a 1,000,000-byte coinbase (BCH's `MAX_TX_SIZE`) plus 200 KiB of share transactions plus the header and merkle branch (8.1); P2P message and buffer sizes re-derived | none |
| Byte budget checks | `BCH_COINBASE_BYTE_BUDGET` (784) and `check_bch_coinbase_budget` go away; the job builder checks `C` instead. (The audit found, by reading the code, that the 784-byte check would stop a BCH Hydrapool node at about 15 miners.) | none |
| Tree execution | Node-local executor, storage and API; not a share-chain rule | Standard transactions after maturity: no signatures, push-only unlocking, standard outputs above dust |
| BCH consensus | unchanged | unchanged. Uses introspection (2022), P2SH32 and CashTokens checks (2023), VM limits (2025), loops and 10,000-byte unlocking (2026) |

### 8.1 The share body limit

**Implemented.** The earlier combined 200 KiB limit came from upstream,
a Bitcoin pool, and would have become a payout cap at about 6,000 P2PKH
payees. `BLOCK_TXS_SIZE_LIMIT` now bounds the share chain's own transactions
separately, while `BCH_SHARE_BODY_SIZE_LIMIT` allows:

```text
share body limit = 1,000,000   the coinbase: BCH consensus MAX_TX_SIZE
                 +   204,800   share transactions (200 KiB, as today)
                 = 1,204,800 bytes, plus the header and coinbase merkle branch,
                               which their decoders bound as today
```

A coinbase is never relayed by BCHN on its own (it reaches BCHN only inside
a block through `submitblock`), so BCHN's 100,000-byte relay limit never
applies to it; only the 1,000,000-byte consensus limit does. Derived sizes
change with it: `MAX_P2P_MESSAGE_SIZE` is 1.25 MiB, and the block
receiver's pending buffer, bounded at 32 MiB, holds at least 26 of the
largest shares. The organise worker's channel (512 shares) and pending
buffer (1,024 shares), bounded by count alone, would hold about 620 MB and
1.2 GB full of the largest shares, which have passed validation and so
carry valid proof of work. They are bounded in bytes too: the channel at
64 MiB, where a validation task waits for room, and the buffer at 128 MiB,
which evicts its oldest blocks
([share processing pipeline](share-processing-pipeline.md#channel-configuration)).
Node behaviour, not a share-chain rule. Measuring propagation at the
maximum body size is a follow-up.

**Bandwidth and propagation (arithmetic estimates).** P2Pool makes a share
about every 10 seconds (`IDEAL_BLOCK_TIME`), plus uncles. Each node receives
each share once and uploads it to every peer that asks for it.

| Payees | Share | Download per node | One hop at 10 Mbit/s | One hop at 100 Mbit/s |
|---|---|---|---|---|
| 100 | about 4 KB | 0.4 KB/s | 3 ms | 0.3 ms |
| 1,000 | about 35 KB | 3.5 KB/s | 28 ms | 3 ms |
| 6,000 (the former limit) | about 205 KB | 20 KB/s | 0.16 s | 16 ms |
| 29,398 (BCH's limit) | about 1 MB | 100 KB/s (0.8 Mbit/s, about 260 GB a month) | 0.8 s | 80 ms |

A node that sends a 1 MB share to 8 peers over one 10 Mbit/s uplink needs
6.4 seconds for the last peer (0.64 s at 100 Mbit/s), and a share crosses
several hops. A share that arrives `t` seconds late is built on by others
for those seconds, so about `1 - e^(-t/10)` more shares go stale: 1 second
costs about 10%, 6.4 seconds about 47%. Stale shares become uncles (9/10 of
their weight) or orphans, which hurts small miners on slow links most.
Storage grows the same way: shares are kept for `BCH_PRUNE_DEPTH` (242,126
shares), about 8 GB of coinbases at 1,000 payees and 242 GB at 29,398. These
are estimates from sizes and link speeds; the network sim being built in
parallel should measure propagation delay and stale rate against coinbase
size from 1 KB to 1 MB.

**Keeping the limit fixed while the cost stays small.** The limit says what
is valid; how shares travel is a separate choice that peers can make
efficient without touching the limit:

1. **Send shares without their payouts.** Every validator already rebuilds
   the whole coinbase from its own window (`validate_bitcoin_payout`), so the
   payout outputs carry no information a peer lacks. A compact share carries
   the header, the coinbase proof and the fields the miner chose (the
   scriptSig and the CTMM fields): a few hundred bytes whatever the payee
   count. The receiver rebuilds the coinbase and checks it against the proof.
2. **Check work before the body.** The coinbase proof (a SHA-256 midstate of
   the 64-byte-aligned prefix plus the fixed tail, `shares/coinbase_proof.rs`)
   already lets a peer check a share's proof of work from its header alone,
   so headers spread in milliseconds whatever the body size.
3. **Fetch full bodies only when needed,** for example by a node still
   syncing that cannot rebuild the window yet, and from several peers at
   once, so no single peer's uplink carries the whole load.
4. **Store compactly too,** rebuilding a coinbase on demand for the API or
   for `submitblock`.

The full 1,204,800-byte form stays the rule, so a node can always ask for
and check a complete share. Items 1 to 4 change the share wire and store
format, not the payout rule, and need their own protocol tag.

### 8.2 Limits outside P2Pool that a large coinbase meets

These are not share-chain rules and this design does not cap them, but they
decide what works in practice:

- **Stratum V1 devices** hash the whole coinbase, and some firmware has
  limits. The only evidence so far is from other pools (OCEAN's DATUM gateway
  sets about 755 bytes for older stock S19 firmware, 2,250 for S21, 6,500 for
  Whatsminer and Bitaxe, 16,000 for ePIC and vnish). The chipnet test with
  the owner's Avalon measures ours.
- **Stratum V2 standard channels** never see the coinbase. **Extended
  channels and Job Declaration** carry the coinbase prefix and suffix in
  fields of at most 65,535 bytes each, so a coinbase past about 1,900 payees
  does not fit them.
- **BCHN's templates** leave room for the coinbase only below
  `-percentblockmaxsize` (50% on mainnet, 100% on chipnet by default);
  `bch-quickstart.md` already tells operators to keep it below 100%.

## 9. How it is tested

### Unit tests (Rust, `cargo nextest`)

- Split: everyone fits; the straddler exactly at `P` (direct) and one byte
  over (tree); no back-filling with a smaller P2SH20 output; ties broken by
  script bytes; `T` and `D` both; under 253 outputs; empty against non-empty
  commitments (a zero root with a non-zero salt is a token share).
- Tree: shapes for 1, 500, 501, 250,000 and 250,001 leaves; outputs plus fees
  equal the tree's value exactly; the fee bound `R * B + n - 1`; dust removal
  converging; the remainder going to the largest payee.
- Producer and validator build identical coinbases for both kinds of share,
  past both limits (extending `test_bch_notifier_and_validator_build_identical_outputs`).
- Covenant: extend the Rust stack machine in `test_utils::bch_vm` with
  introspection, loops, `OP_CAT`, `OP_SPLIT`, `OP_NUM2BIN` and token
  introspection, and run every node transaction, counting operation cost and
  hash iterations as the VM Limits CHIP does. Negative tests are built from
  scratch as false statements: an extra or missing output, a changed value or
  script, an extra input, a token on an output, another version, lock time or
  sequence, a UTXO with another value. The measurements set `Z` and confirm
  `F`.
- Claim fit: the largest token-share coinbase at `T` with `h = 16` and
  `d = 25` gives an unlocking of at most 10,000 bytes.
- Executor: maturity timing, idempotent rebroadcast, BCH and share-chain
  reorgs, against `p2poolv2_tools`'s mock bitcoind.

### The sim on BCHN regtest

BCHN is the authority on both consensus and relay policy, so every node
transaction must pass a real BCHN mempool, not only our stack machine. A new
RUNBOOK test next to Test E (payout split) and Test G (maturity):

1. A swarm with more than 77 payees in the window.
2. One node declares a non-empty commitment through Job Declaration until its
   share becomes a regtest block.
3. `generatetoaddress 100`; the executor broadcasts; BCHN accepts every node
   transaction into its mempool and a block.
4. Every payee's UTXO matches the expected amount to the satoshi.
5. Again with the empty commitment and a few thousand payees: BCHN accepts the
   large coinbase.

Regtest does not enforce BIP34, which does not matter here.

### Chipnet with the owner's Avalon

Run by the owner after approval; agents do not touch the running chipnet
pool.

1. Coinbases of increasing size (about 1, 2, 4, 8 and 16 KB) on no-token
   shares, recording coinb1 and coinb2 and whether the Avalon's shares stay
   valid.
2. A token share declared by a Job Declaration client committing a test SAFA
   root; after 100 chipnet blocks the tree executes and every payee is paid.
3. The test token claimed from a coinbase at `T`.
4. The pool's BCHN with `-percentblockmaxsize` below 100% (chipnet's default
   is 100%).

## 10. Approved choices and follow-ups

The owner approved `T = 3,072`, smallest-first ordering, the straddle rule,
the tree beyond either coinbase budget, direct payouts below dust, fan-out
500, fee variants 1 through 3 with node execution at 1 sat/byte, and a
1,000,000-byte coinbase beside 200 KiB of share transactions. These are
implemented in this batch.

Compact share transport, measured propagation at scale, chipnet tree
execution and device firmware measurements remain follow-ups. Format v3
(section 7.2, step 3b) is deferred and needs agreement with Pickaxe before
implementation. It is not part of this batch.

## Sources

BCHN (master, read 2026-10-23):

- [`src/consensus/consensus.h`](https://gitlab.com/bitcoin-cash-node/bitcoin-cash-node/-/blob/master/src/consensus/consensus.h): `MAX_TX_SIZE`, `COINBASE_MATURITY`, `MAX_COINBASE_SCRIPTSIG_SIZE`, `MAX_CONSENSUS_BLOCK_SIZE`
- [`src/consensus/tx_check.cpp`](https://gitlab.com/bitcoin-cash-node/bitcoin-cash-node/-/blob/master/src/consensus/tx_check.cpp): `CheckTransactionCommon`, `CheckCoinbase`
- [`src/validation.cpp`](https://gitlab.com/bitcoin-cash-node/bitcoin-cash-node/-/blob/master/src/validation.cpp): standardness only in mempool acceptance; token checks on the coinbase
- [`src/policy/policy.h`](https://gitlab.com/bitcoin-cash-node/bitcoin-cash-node/-/blob/master/src/policy/policy.h) and [`policy.cpp`](https://gitlab.com/bitcoin-cash-node/bitcoin-cash-node/-/blob/master/src/policy/policy.cpp): `MAX_STANDARD_TX_SIZE`, dust threshold, scriptSig size
- [`src/script/vm_limits.h`](https://gitlab.com/bitcoin-cash-node/bitcoin-cash-node/-/blob/master/src/script/vm_limits.h), [`script.h`](https://gitlab.com/bitcoin-cash-node/bitcoin-cash-node/-/blob/master/src/script/script.h), [`standard.h`](https://gitlab.com/bitcoin-cash-node/bitcoin-cash-node/-/blob/master/src/script/standard.h)
- [`src/init.cpp`](https://gitlab.com/bitcoin-cash-node/bitcoin-cash-node/-/blob/master/src/init.cpp) (`-acceptnonstdtxn`) and [`src/chainparams.cpp`](https://gitlab.com/bitcoin-cash-node/bitcoin-cash-node/-/blob/master/src/chainparams.cpp) (`fRequireStandard`, upgrade heights, `upgrade2027ActivationTime`)
- [Release notes 23.0.0](https://docs.bitcoincashnode.org/doc/release-notes/release-notes-23.0.0/): unconfirmed chain limit and CPFP removed
- [`bitcoind` options](https://docs.bitcoincashnode.org/doc/cli/bitcoind/): `-minrelaytxfee`, `-dustrelayfee`, `-percentblockmaxsize`

Upgrade specifications and CHIPs:

- [2018-05 re-enabled opcodes](https://upgradespecs.bitcoincashnode.org/may-2018-reenabled-opcodes/), [2018-11 CTOR](https://upgradespecs.bitcoincashnode.org/2018-nov-upgrade/), [2020-05 SigChecks](https://upgradespecs.bitcoincashnode.org/2020-05-15-sigchecks/), [2021-05 upgrade](https://upgradespecs.bitcoincashnode.org/2021-05-15-upgrade/)
- [2022-05 introspection](https://upgradespecs.bitcoincashnode.org/2022-05-15-upgrade/), [2023-05 CashTokens and P2SH32](https://upgradespecs.bitcoincashnode.org/2023-05-15-upgrade/), [CashTokens spec](https://github.com/cashtokens/cashtokens)
- [2025-05 VM limits and BigInt](https://upgradespecs.bitcoincashnode.org/2025-05-15-upgrade/): [VM Limits CHIP](https://github.com/bitjson/bch-vm-limits)
- [2026-05 upgrade](https://upgradespecs.bitcoincashnode.org/2026-05-15-upgrade/): [P2S](https://github.com/bitjson/bch-p2s), [Loops](https://github.com/bitjson/bch-loops), [Functions](https://github.com/bitjson/bch-functions), [Bitwise](https://github.com/bitjson/bch-bitwise)
- [ABLA, CHIP-2023-04](https://gitlab.com/0353F40E/ebaa)

Other:

- [Elements opcodes](https://github.com/ElementsProject/elements/blob/master/doc/tapscript_opcodes.md): `OP_SHA256INITIALIZE`, `UPDATE`, `FINALIZE` (precedent for a resumable hash)
- [BIP119](https://github.com/bitcoin/bips/blob/master/bip-0119.mediawiki): congestion control trees (the idea; BCH needs no new opcode)
- [0zkbrewer BCH-FRI-STARK-Verifier](https://github.com/0zkbrewer/BCH-FRI-STARK-Verifier): multi-input verifier topology and its binding audits
- [OCEAN DATUM gateway](https://github.com/OCEAN-xyz/datum_gateway): per-device coinbase sizes (`src/datum_coinbaser.c`, `src/datum_stratum.c`)
- In this repository: `accounting/payout/coinbase_budget.rs`, `stratum/work/coinbase.rs`, `shares/validation/mod.rs`, `shares/coinbase_proof.rs`, `tools/safa-auxpow/README.md`, `docs/bch-auxpow.md`
