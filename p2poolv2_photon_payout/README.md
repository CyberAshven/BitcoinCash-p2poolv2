# PHOTON minimum-payout study and retained tree prototype

The main proposed path is the owner's **2026-10-30** minimum-payout design:
won tokens create backed gross balances, miners choose multiples of a
voted token cost `C` (floor 10, default 20), and a single direct split pays
the most owed eligible miners. Each actual payee contributes `C` tokens
to the submitter, who fronts BCH fees and all output dust. `R` still binds
the complete proof seed, parent and context. There is no sponsor or price
oracle. This crate is an offline study, with no node, share-chain, wallet,
RPC, network or spending integration.

`lean` adds an independent reference, direct-list redeem constructor and
transaction builder. `lean-payout.cash` is checked against its Rust
assembler. The existing assembler's visibility is widened for reuse; the
funded tree's program bytes, source artifact and tests remain unchanged.
`.cash` files use LF on Windows because the artifact tests compare exact
text. This fixes checkout portability without changing program bytes.

The exact lean encoding starts with a minimally pushed 82-byte header:
version/reserved (4 bytes), network (4), raw category (32), context digest
(32), `C` (8), and entry count (2). The context begins at `R` byte 42.
An empty altstack sentinel follows. Each immutable entry is a minimal push
of script length (1), script (23/25/35), gross owed (8) and multiple (2),
then `OP_TOALTSTACK`. Entries are pushed in reverse priority order and
popped most-owed first. The fixed lean program follows; there is no tree.
The program checks every entry, including an unpaid suffix, and repays
only actual positive payees. The reference also rejects duplicate scripts
globally; the VM's adjacent tie ordering alone is not a global uniqueness
proof. Authentic share-chain construction must supply that invariant.

`ProofBinding::digest` hashes `PHOTON-LEAN-CONTEXT-v1`, parent (32), baton
outpoint (36), metadata hash (32), window commitment (32), ledger commitment
(32), deployment lock hash (32), then seed length (u32le) and exact seed
bytes using HASH256. Its seed is an opaque caller-supplied serialization
of all section 4 non-tail choices. Mutation tests check field binding;
this is not PHOTON claim verification or a frozen share wire format.

The ordinary rule requires complete coverage and a clipped last gross
payment at least its own minimum. An inadequate list, an empty initial
ledger or a below-minimum final remainder fails closed. No bootstrap,
advance, token change, remainder donation or later rewrite of an old `R`
is invented. Pending obligations remain owed but reserved. An aggregate
token accounting identity does not make inaccessible locked roots payable.
The architecture document lists these blockers and all open owner choices.

Reproduce the lean measurements with:

```text
cargo photon-lean-measure
```

The measurements use complete signed synthetic transactions in both pinned
libauth BCH 2026 modes. No BCHN admission or market-value result is claimed.
Full artifact: [lean-measurements.md](lean-measurements.md).

### Lean direct split measurements

Parentheses identify beneficiary script length. Operation cost pairs are
standard / non-standard; hash pairs give usage / limit in the same order.
Fees are exact serialized bytes at 1 sat/byte; fee/payee is display rounding.

Pinned libauth be3ed16396c9eb9c0e57343474d36079a37c8dae, BCH 2026. Program 885 bytes. All listed spends verify both complete VMs. Equal gross debt 20*C, C=2^32 raw tokens, multiplier 20, maximum 35-byte P2SH32 payee scripts, P2PKH submitter/change, one 100-byte signed P2PKH funding unlock, 700 inherited satoshis, P2SH20 root. All payee/submitter token amounts use nine-byte CompactSize. Fee=serialized bytes at 1 sat/byte.

| N | R bytes | Unlock bytes | Op cost/limit | Hash std; nonstd | Tx bytes/fee | Fee/payee | Dust payee/submitter/change | Gross front sats | Net external sats | Max item | Peak slots |
|---:|---:|---:|---:|---|---:|---:|---|---:|---:|---:|---:|
| 1 (35) | 1019 | 1048 | 94060/871200; 91756/871200 | 18/544; 18/3811 | 1440 | 1440.000 | 705 / 675 / 546 | 2820 | 2120 | 1048 | 21 |
| 8 (35) | 1355 | 1384 | 363361/1140000; 360417/1140000 | 23/712; 23/4987 | 2385 | 298.125 | 705 / 675 / 546 | 8700 | 8000 | 1384 | 28 |
| 32 (35) | 2507 | 2536 | 1286977/2061600; 1281729/2061600 | 41/1288; 41/9019 | 5625 | 175.781 | 705 / 675 / 546 | 28860 | 28160 | 2536 | 52 |
| 64 (35) | 4043 | 4072 | 2518576/3290400; 2510256/3290400 | 65/2056; 65/14395 | 9945 | 155.391 | 705 / 675 / 546 | 55740 | 55040 | 4072 | 84 |
| 100 (35) | 5771 | 5800 | 3904216/4672800; 3892440/4672800 | 92/2920; 92/20443 | 14805 | 148.050 | 705 / 675 / 546 | 85980 | 85280 | 5800 | 120 |
| 187 (35) | 9947 | 9976 | 7254457/8013600; 7234361/8013600 | 157/5008; 157/35059 | 26550 | 141.979 | 705 / 675 / 546 | 159060 | 158360 | 9976 | 207 |

Largest N with 35-byte payees: 187. N=188 fails: lean redeem or unlocking bytecode exceeds 10000 bytes.

| N | R bytes | Unlock bytes | Op cost/limit | Hash std; nonstd | Tx bytes/fee | Fee/payee | Dust payee/submitter/change | Gross front sats | Net external sats | Max item | Peak slots |
|---:|---:|---:|---:|---|---:|---:|---|---:|---:|---:|---:|
| 99 (25) | 4733 | 4762 | 3838633/3842400; 3828905/3842400 | 76/2401; 76/16810 | 12690 | 128.182 | 675 / 675 / 546 | 80190 | 79490 | 4762 | 119 |

Largest N with 25-byte payees: 99. N=100 fails: Unable to verify transaction: error in evaluating input index 0: Program attempted an operation that would exceed the operation cost density limit. Maximum operation cost: 3872800 (density control length: 4841); operation cost following operation: 3872819..

| N | R bytes | Unlock bytes | Op cost/limit | Hash std; nonstd | Tx bytes/fee | Fee/payee | Dust payee/submitter/change | Gross front sats | Net external sats | Max item | Peak slots |
|---:|---:|---:|---:|---|---:|---:|---|---:|---:|---:|---:|
| 83 (23) | 3959 | 3988 | 3223099/3223200; 3215035/3223200 | 63/2014; 63/14101 | 10518 | 126.723 | 669 / 675 / 546 | 66720 | 66020 | 3988 | 103 |

Largest N with 23-byte payees: 83. N=84 fails: Unable to verify transaction: error in evaluating input index 0: Program attempted an operation that would exceed the operation cost density limit. Maximum operation cost: 3252000 (density control length: 4065); operation cost following operation: 3252022..


Densest mixed standard-script profile maximum N=207: R=9971, unlock=10000, op=7999555/8032800, hash=157/5020, tx/fee=27378, fee/payee=132.261, grossfront=171180, netexternal=170480. Next N fails Unable to verify transaction: error in evaluating input index 0: Program attempted an operation that would exceed the operation cost density limit. Maximum operation cost: 8032800 (density control length: 10041); operation cost following operation: 8032865..

Method: lengths from actual R and input unlocking; final libauth counters and limits plus full opcode-debug trace maxima via existing vm::measure_input; transaction bytes from libauth BCH serializer after fixed-length Schnorr funding signature; dust=3*(serialized output bytes+148) via existing output_dust. Gross fronting=sum token-output BCH minima+fee; net external subtracts inherited 700, returned BCH change is excluded. The covenant minima conservatively reserve nine-byte token amounts (705 sats for P2SH32, 675 P2PKH); BCH-only change dust 546. 100000-byte standard transaction cap is checked by builder, 10000-byte unlock/redeem item caps checked before construction and by VMs. BCHN and market value are untested.

#### Additional capacity probes and scope

`cargo photon-lean-measure --small-capacity` scans the same near-10,000-byte
script mix with C=1 and equal debt 20, multiplier 10: standard capacity 208.
`cargo photon-lean-measure --unequal-capacity` uses strictly descending debts
N-index+10: standard capacity 209. These two generic covenant probes have
rewards below the current PHOTON claim encoding range and are not mineable
claim vectors.

`cargo photon-lean-measure --claim-capacity` sets the largest debt to 2^32,
with C=1, multiplier 10 and the remaining debts descending 218..11 at N = 209.
The total reward is 4,294,991,112 raw units, above the current2^32 amount floor.
This checks split amount compatibility only; it does not execute a claim.
All 209 payees pass both complete VMs. R 9,971, unlock 10,000; operation cost
8,023,215/8,032,800 standard and 8,003,119/8,032,800 non-standard; hash iterations
157/5020 standard and 157/35143 non-standard. Transaction/fee 25,780 sats,
123.349 sats/payee; gross front 170,704 sats, net external 170,004 after inherited 700.
Peak stack+altstack slots 229; maximum observed item 10,000 bytes. N = 210 in this
same maximal-byte script mix fails operation cost in both modes.

The 209 records are 86 P2SH20 scripts followed by 123 P2SH32 scripts. Exact dust
is 657 sats for the large first output,645 each for the other 85 P2SH20 payees,
681 each for 123 P2SH32 payees,651 for submitter and546 for BCH change. The
builder/covenant deliberately retain conservative nine-byte-amount minima:
669/705 for payees and675 for submitter, so these small-amount outputs receive
more BCH than exact dust. All of it remains with its recipient.

The fixed nine-byte-amount mix at N = 208 fails standard operation density but
passes non-standard mode; it is not a standard candidate. N = 207 is its largest
measured standard profile. 209 is the largest measured standard construction
in the amount-compatible boundary probes, not a proof of an absolute maximum
over every possible debt, multiplier, script mix, submitter, program
optimization or unresolved owner parameter. The exact global extremum remains
open. The mix maximizes R length under the 10,000-byte unlock limit with as many
35-byte scripts as fit, 25-byte scripts for remaining even-byte room, then 23-byte
scripts. The scanner increments N until the next whole transaction fails.
The 100,000-byte standard transaction ceiling is inactive in all reported rows.

The approximately 110 sats/payee claim is not confirmed: the shortest uniform
standard profile here costs 126.723 sats/payee, the amount-compatible209-payee
probe 123.349, and the all 35-byte benchmark at 100 payees 148.050. A no-overhead
asymptote is 111 bytes for 23-byte scripts (36 bytes in R plus a 75-byte output);
shared contract/funding/submitter/change overhead and operation density still
apply. BCH dust fronting is additional to this fee.

## Simulation of the minimum-payout rules

`cargo photon-payout-sim --summary` reproduces the compact tables in
[simulation-summary.md](simulation-summary.md); the full inputs, integer
checks and methods are in [simulation-results.md](simulation-results.md).
The runs use the rules exactly as written in section 5 of the design
document, and under them small miners are not paid within the 5,000-win
horizon. The causes and candidate changes, none adopted, are in the design
document's "Simulation findings" subsection.

## Retained funded Merkle-sum tree (phase 2a)

The remaining tree description and measurements are historical evidence,
no longer the main payout path. Its sponsorship and per-win weight rules
do not replace the main path's balance ledger and token reimbursement.

This developer-only crate implements the funded fungible-token payout
covenant proposed in [the architecture document](../docs/architecture/photon-pool.md).
It has no node integration, RPC client, network transport, wallet, private-key
generator or broadcast path. All transactions and funding fixtures are offline.
The crate is AGPL-3.0-or-later, unpublished and excluded from distribution.

The pinned `libauth` revision is the workspace's
`be3ed16396c9eb9c0e57343474d36079a37c8dae`. Its BCH 2026 VM is the execution
authority for this prototype, in standard and non-standard mode. BCHN
transaction admission, relay policy and real funding signatures require
separate integration evidence; no BCHN was called for this prototype.

The root is parameterized as P2SH20 or P2SH32. Children are P2SH32. The
336-byte state is prefixed by minimal `OP_PUSHDATA2`; the context digest
starts at redeem byte 43. The root reads the reward from its active input;
descendants pin the original reward and use global cumulative intervals.
Merkle-sum commitments authenticate midpoint leaf counts and unsigned
64-byte little-endian weights. Terminal buckets have at most eight leaves.

The covenant enforces a singleton payout input at index zero, one through
four token-free funding inputs, ordered positive allocation outputs,
optional token-free change last, version 2, locktime zero and final
sequences. Token allocation uses exact telescoping division. Each token
output receives its committed BCH minimum; their aggregate retains all
inherited BCH. The executor may assign excess BCH among those outputs.
This aggregate-only rule is the frozen prototype policy, pending the
owner's deployment decision.

## Reproduce

Use the build environment specified by the repository coordinator and a
target directory dedicated to this worktree. No network or wallet setup
is needed by the prototype.

```text
cargo fmt --all -- --check
cargo clippy -p p2poolv2_photon_payout --all-targets --all-features -- -D warnings
cargo nextest run -p p2poolv2_photon_payout
cargo nextest run -p p2poolv2_photon_payout --release --profile photon-heavy --test heavy --run-ignored ignored-only --nocapture
cargo deny check
cargo photon-payout-measure
```

Heavy acceptance tests are ignored by default so routine CI does not run
the full 4,096-leaf execution or 100,000 seeded random manifests. Run them
explicitly using the command above. The named `photon-heavy` nextest profile
allows up to an hour for these explicit stress runs and leaves the default
30-second timeout unchanged. The measurements artifact and the
architecture document record the actual run results and their limits.

## Checked artifacts and reference

`src/covenant.rs` resolves named stack slots to ordinary CashAssembly.
`payout.cash` retains that annotated assembly, compiled by pinned libauth.
`program.hex` and `program.sha256` are checked byte-for-byte in unit tests.
Regenerate them with `cargo run -p p2poolv2_photon_payout --example covenant_artifacts`.
The fixed program is 2,075 bytes; the redeem is 2,414 bytes. The SHA256 is
`5b6e4ea2802307e6fce8b94d8ed6d65751fc54af6370989688e39aa79b628f3c`.
The constructor requires this hash; the script compares its executing
suffix with the state digest and replicates that suffix into children.
The authentic root address binds the complete redeem, including its
context and program. No witness-supplied program or child lock is trusted.

The packed witness starts with a two-byte little-endian leaf count. An
internal node then reveals exactly two 98-byte descriptors (hash, 64-byte
weight, two-byte count). A terminal reveals each leaf as one-byte canonical
CompactSize script length, script, and 64-byte weight. The covenant derives
intervals, allocation order and child locks. It hashes the deterministic
midpoint tree even inside terminal buckets, authenticating all counts.

`reference.rs` independently canonicalizes and validates manifests, builds
trees, and checks 512-bit weights and 575-bit allocation products.
`state.rs` serializes the exact state; category bytes are raw wire order,
while libauth token objects use reversed display order. `transaction.rs`
builds root, internal and terminal spends from supplied funding inputs.
It assigns inherited BCH excess to the first positive output as constructor
policy; the covenant enforces only the aggregate. The maximum window is
`43,200 * 2^256` scaled units; a selected bootstrap work value can impose a
tighter bound before accepting a manifest.

The default `test-utils` feature exposes public synthetic fixtures. Their
fixed scalar one is deliberately public; they generate no keys and read no
wallet. Ordinary fixtures sign P2PKH funding with
`SIGHASH_ALL | SIGHASH_FORKID | SIGHASH_UTXOS`. Maximum-funding fixtures sign
P2SH32 redeems and produce exactly 10,000-byte unlocks. The core can also be
built with `--no-default-features`; no fixture is a live funding wallet.

## Results

The final program passed groups A and B in both standard and non-standard
libauth BCH 2026 modes. Whole-transaction verification includes BCH values,
CashTokens and every funding script. Mutation helpers also require failure
of input 0 independently, preventing stale signatures from masking a
covenant acceptance bug.

| Acceptance | Result |
|---|---|
| A1 | Both root locks, two children, four funders and change pass |
| A2, A5 | All 1,023 linked spends of 4,096 leaves pass; 511 internal, 512 terminal, depths 9 through 1, token conservation at every step |
| A3 | Eight 35-byte scripts, nine-byte token amounts, near-272-bit total and four funders pass |
| A4 | Single-leaf and root-terminal cases pass; separate child-mode single-leaf VM vector passes; a 17-leaf tree covers a terminal child with remaining depth above zero |
| A6, A7 | Rewards 1, 2^32 and 2^63-1, zero child/leaf allocation and all-but-one zero pass |
| A8 | Exact and adjacent rounding boundaries pass; 100,000 seeded manifests telescope exactly and pass both complete VM modes |
| B | All listed mutation families reject; each allocation output has separate plus/minus-one probes; all 12 state fields have binding tests and malformed-state semantics have re-locked tests |
| D | VM counters, opcode-trace peaks, dust, fees, full trees and first-beneficiary paths recorded in [measurements.md](measurements.md) |

The final heavy run completed on 2026-10-30 (Asia/Riyadh): full-tree test
2.84 seconds; 100,000 manifests 156.76 seconds, SplitMix64 seed
`0x70686f746f6e3261`. Stress manifests have one through eight unequal positive
leaves, including wide weights, and use valid public P2SH20 `OP_TRUE`
funding to avoid benchmarking signatures. Full-tree execution uses distinct
signed funding outpoints and actual parent txids and source outputs.
Regular tests have 130 passing cases; the two ignored heavy tests were run
separately and both passed. No valid-case or VM budget failure remains.

The first stress attempt hit the existing 30-second nextest timeout; the
explicit heavy profile fixes only that run's time budget. Parallel Windows
runs also reported successful tests with nextest `LEAK` labels despite
these tests spawning no processes; the final serial run passed all 130
regular tests cleanly with no leak labels.
These harness observations are distinct from VM rejection results.

With four ordinary P2PKH funders per spend, the 4,096-leaf tree consumes
6,985,170 satoshis externally: 4,098,190 in fees plus 2,887,680 of final leaf
BCH minus the root's inherited 700. The first beneficiary's path consumes
46,832 satoshis, including sibling outputs and its full eight-leaf bucket.
The measured program is smaller than the earlier design estimates; no
profile limit was increased. BCHN admission, group C and GPU seed hashing
remain unverified. No node or share-chain behavior is implemented.

## Phase 2b TODO: group C

Use the unmodified PHOTON v3.2 covenant, in a separate claim-side change:

- Verify P2SH20 pool claims at ages 0, 17, 128 and 32,767 BCH blocks.
- Show locktime variants preserve the reconstructed proof.
- Mutate `R`, key, nonce, datasig through signing nonce, age, sequence and
  BCH values; require changed digest or rejection.
- Publish claim-side golden vectors for `R` and `HASH160(R)`.
- Measure `HASH160(R)` per seed on CPU and GPU; this crate has no GPU engine.

BCHN regtest `testmempoolaccept` and full relay/dust/finality testing remain
a separate gate. This phase does not authorize a share chain, node,
chipnet spend, mainnet spend or deployment.
