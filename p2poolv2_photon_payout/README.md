# PHOTON minimum-payout study and retained tree prototype

The main proposed path is the owner's **2026-10-30** minimum-payout design:
won tokens create balances, miners choose a multiple (at least 10, default
20) of a voted token cost `C`, and a single direct split pays the queue first
come, first served, then pays the rest ahead within the expected credit of
shares already found, over a 48-hour window. The owner adopted those rules
on 2026-10-30 for the PHOTON pool only; they do not change BCH P2Pool, and
PHOTON is GPU-mined, not merge-mined with BCH. Each actual payee contributes `C` tokens
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

`ListOrder::ShareChain` selects lean version 2 for the adopted
first-come, first-served rule. Its header version is 2, and it pays entries
in the order `R` commits, without the most-owed ordering check; all other
checks are unchanged. `lean-payout-share-order.cash` is its checked source.
Version 1's bytes and artifact are unchanged. Version 2 is 818 program bytes
and fits 188, 238 and 198 payees with 35-, 25- and 23-byte scripts, or 244
in the densest mix. Reproduce with `cargo photon-lean-measure --share-order`;
full artifact: [lean-share-order-measurements.md](lean-share-order-measurements.md).

### Lean direct split measurements

Version 1 is 885 program bytes. Its largest standard lists are 187 payees
with 35-byte scripts, 99 with 25-byte and 83 with 23-byte scripts, and 207
in the densest standard mix; the amount-compatible claim probe
(`--claim-capacity`) fits 209. The fee is 141.979 sats a payee at 187
payees with 35-byte scripts and 126.723 at the shortest uniform profile, so
the earlier estimate of about 110 sats a payee is not confirmed; BCH dust
fronting comes on top. Every listed spend verifies in both libauth BCH 2026
modes. Tables, methods and the other capacity probes (`--small-capacity`,
`--unequal-capacity`): [lean-measurements.md](lean-measurements.md).

## Simulation of the minimum-payout rules

`cargo photon-payout-sim --summary` reproduces the compact tables in
[simulation-summary.md](simulation-summary.md); the full inputs, integer
checks and methods are in [simulation-results.md](simulation-results.md).
The runs use the rules as first written (most owed first, never paid
ahead, a 4,096-script window cap and 100 launch wins of history), and under
them small miners are not paid within the 5,000-win horizon. The causes are
in the design document's "Simulation findings" subsection.

`cargo photon-payout-sim --candidate-summary` re-runs the same scenarios
under the four changes the owner adopted, with the 48-hour window: first
come, first served; pay-ahead bounded by the expected credit of shares
already found; no window script cap; and no launch history. The runs
compare 10-second and 1-second shares and 12-hour and 48-hour windows.
Tables:
[candidate-summary.md](candidate-summary.md); per-period detail:
[candidate-results.md](candidate-results.md) (`--candidate`). At 1,000
miners no win is blocked and the fixed probe is paid about every 1.0 to
1.2 days, matching its threshold formula. At 10,000 miners a 12-hour
window often leaves no valid list, so those wins cannot be mined; a
48-hour window at `N = 187` removes every blocked win, with the same probe
waits. The design document's "Candidate rules re-run" subsection explains
the results.

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

No network or wallet setup is needed.

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
The tree's regular tests have 130 passing cases, and with the lean and
simulation tests the crate has 207; the two ignored heavy tests were run
separately and both passed. No valid-case or VM budget failure remains.

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
