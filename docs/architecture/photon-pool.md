# Proposal: a decentralized pool for PHOTON GPU miners

**Status: proposal, 2026-10-29. No implementation is authorized by this
document.** The Pickaxe session and an independent covenant auditor must
review it before code. Parameter choices below are proposed PHOTON
share-chain rules. They change neither BCH consensus nor PHOTON v3.2.

## 1. Goal and scope

Build a sister program in this Rust workspace. PHOTON miners contribute
verifiable work to a separate peer-to-peer share chain. Each winning claim
locks its reward to the preceding PPLNS window. Anyone can fund and execute
the distribution. No operator holds the reward key or a withdrawal ledger.
Pickaxe is one client. The protocol must permit independent clients.

The owner chose this approach over a hosted Pickaxe pool on 2026-10-29.
The goal is lower individual payout variance without a payout custodian.
It does not create more PHOTON, eliminate the pool's own variance, promise
frequent settlement, or fund BCH transaction fees automatically.

The contract aims at roughly one emission per BCH block [P1]. Assuming
one emission per 600 seconds and a miner with 1/10,000 of PHOTON work, the
mean solo wait is 6,000,000 seconds, or 69.4 days. These are an owner-supplied
scenario and arithmetic, not a measured emission rate. Under an independent
Poisson approximation, the chance of no win during that mean wait is
about 36.8 percent. A pool with fraction `pool_fraction` of network work
has mean win spacing `600 seconds / pool_fraction` under the same assumption.
PPLNS spreads each pool win; it cannot make an unsuccessful pool pay.

Non-goals are changing the token category or vault contract, BCH block
mining, merge mining, tradable share outputs, a hosted service, and an
exchange or token-to-BCH fee conversion service. PHOTON's author is
[2qx](https://github.com/2qx/vox). A replacement vault would be a different
deployment. The owner's requirement is the existing v3.2 token.

## 2. Verified PHOTON v3.2 constraints

### Source boundary

The inspected Pickaxe files are the v3.2 template and redeem script,
`src/protocol.rs`, `docs/protocol-profile.md`, `docs/architecture.md`, and
`src/reward.rs`. Additional transaction and tail-loop sources resolve the
layout and hashing questions [P1-P8]. The named files match Pickaxe revision
`2f7cbb0493f2a46ef89d4f04c941074311a09ba0`. This is source verification,
not a fresh query of mainnet or chipnet UTXOs.

The decoded redeem script is 273 bytes. Its HASH256 is
`0ab476a6dab00ba11118a11c078f871c5060a66e18140368425202772d0e80c2`.
The lock is
`aa200ab476a6dab00ba11118a11c078f871c5060a66e18140368425202772d0e80c287`.
Pickaxe's deployment constants identify that same lock on mainnet and
chipnet, with different categories [P1-P3]. There is no administrator
branch. Every spend preserves the active vault's lock and category.

### Claim and oracle branches

For a claim, the covenant requires transaction version 2, exactly one
input and exactly two outputs [P1]. Output index 0 recreates the vault.
It preserves the category and mutable NFT capability. The new commitment
has exactly 100 bytes: a 4-byte nonce, a 32-byte next target and a 64-byte
datasig. Vault tokens must be at least
`balance - floor(balance / 420,000)`, in raw token units. Vault BCH must
be at least `input_value - 1,500 satoshis`.

The pool profile requires output index 1 to use the PHOTON category.
The covenant copies that output's category into its proof preimage;
it does not compare it to the vault category. The reconstructed prefix
hard-codes bitfield `0x10`, so the canonical output carries fungible tokens
without an NFT. For this same-category profile, BCH token conservation
limits the reward to `floor(balance / 420,000)` raw units.
Pickaxe chooses one raw unit less before its tail decrement
[P1, P3]. The reward's BCH value and the transaction fee share the
1,500-satoshi vault allowance. No funding input can be added to a claim:
that would select the oracle branch instead.

The locking bytecode of the reward is unconstrained by purpose. In the
current 9-byte reward-amount encoding, its length can be at most 84 bytes
before the covenant's reconstructed length byte differs from transaction
serialization [P1, P7]. P2PKH, P2SH20 and P2SH32 are the useful standard
recipient forms. This does not mean they are the only BCH standard script
forms. This proposal uses a 23-byte P2SH20 reward lock.

Pickaxe's reward constant is 700 satoshis [P6]. For a current P2PKH claim,
fees of 629 to 631 satoshis at 1 satoshi per byte leave a vault decrease
of 1,329 to 1,331 satoshis. A reward of 850 satoshis also fits that
allowance at that fee rate. Thus 700 to 850 satoshis is a feasible
example range, not a contract promise. A P2SH20 claim is slightly smaller.
If relay fees and required reward dust do not fit, mining must stop or
wait for different inclusion conditions; it cannot add a claim input.

The oracle branch is different. With more than one input it requires
the active vault output to gain at least 8,000 satoshis and retain its
token amount and commitment [P1]. The caller also pays transaction fees.
It moves the vault outpoint without a mining proof. It does not grant
the pool an 8,000-satoshi withdrawal. `reward.rs` retains settlement
machinery for the retired v0 contract; its reserve-funded split is not
a v3.2 payout solution [P6].

### Encodings and the proof

The covenant always reconstructs the vault amount as marker `0xff`
followed by 8 bytes. Canonical serialization therefore requires at least
`2^32` raw units remaining. Its reward branch uses ScriptNum length to
choose a 4-byte or 8-byte amount. The range `[2^31, 2^32)` raw units is
incompatible. Rewards below `2^16` raw units also do not match that
reconstruction [P1, P7]. The initial pool profile follows Pickaxe's
narrower supported case: both amounts stay at least `2^32` raw units,
including the entire configured tail range. It fails closed outside it.
Later emission epochs need separate layouts and VM vectors.

The datasig authenticates `nonce || next_target` under a supplied compressed
public key. It does not authenticate a payout owner. The claimed nonnegative
age passes CSV and is less than 65,535 BCH blocks. It can be less than
the available age; the contract does not require the maximum. For an
unconfirmed predecessor, age 0 BCH blocks is allowed. The target is
`floor(previous_target * (age + 143) / 144)`; multiply before divide
[P1, P3]. The target and proof digest are positive little-endian ScriptNum
values. The predicate is strictly `0 < digest_number < target`.
This differs from an unsigned 256-bit comparison and from retired v0's
absolute-value comparison.

The covenant hashes a reconstruction of the claim with locktime fixed to
0. Other claim fields are copied from the transaction or reconstructed
constants. It never reads actual transaction locktime [P1]. With the
canonical locktime, that proof hash equals the transaction's HASH256.
Section 6 explains the other case. Changing the reward script or amount
changes the checked hash. A recipient cannot be replaced while retaining
the same proof.

### Byte layout and GPU implications

Offsets below are zero-based and inclusive, in bytes. They describe the
current P2PKH layout with a 9-byte reward amount and age 0 to 16 BCH
blocks [P7]. Add the age shift to fields after the age push: 0 bytes for
ages 0 to 16 BCH blocks, 1 byte for ages 17 to 127 BCH blocks, and 2 bytes
for ages 128 to 32,767 BCH blocks.

| Field | Byte offsets | Meaning |
|---|---|---|
| Public key | 45 to 77 | 33-byte compressed key |
| Age push | 78 | Small-integer opcode in this layout |
| Vault redeem script | 82 to 354 | 273 bytes |
| Input sequence | 355 to 358 | CSV sequence, 4 bytes |
| Vault satoshis | 360 to 367 | 8 bytes |
| Commitment nonce | 404 to 407 | 4 bytes |
| Commitment target | 408 to 439 | 32 bytes |
| Commitment datasig | 440 to 503 | 64 bytes |
| Vault amount marker and amount | 504; 505 to 512 | `0xff`; 8 bytes |
| Reward satoshis | 548 to 555 | 8 bytes |
| Reward category | 558 to 589 | 32 bytes |
| Reward bitfield | 590 | `0x10` |
| Reward amount marker and amount | 591; 592 to 599 | `0xff`; 8 bytes |
| Reward locking bytecode | 600 to 624 | P2PKH, 25 bytes |
| Actual locktime | 625 to 628 | 4 bytes |

P2SH20 replaces the last script with 23 bytes at offsets 600 to 622 and
moves actual locktime to offsets 623 to 626. Earlier offsets stay fixed.
Claims are then 627, 628 or 629 bytes for those age ranges. P2PKH claims
are 629, 630 or 631 bytes. Both need 10 first-pass SHA-256 blocks after
padding. P2SH32 adds 10 bytes relative to P2PKH: 639 to 641 bytes and
11 first-pass blocks [P4, P7; serialization arithmetic].

The supplied hashing estimate needs correction. The conserved tail search
does not only decrease reward bytes 592 to 599. It also increases vault
bytes 505 to 512 by the same tail value, less than 65,536 raw units [P7].
With zero-based 64-byte block numbering, those changes touch blocks 7
and 9. Current T2 resumes block 7, processes blocks 8 and 9, and does the
second SHA-256: four compression stages per candidate, with partial-round
optimizations. A baseline candidate from the cached 384-byte prefix
needs five stages plus its signing work; T2 prepares block 6 once per
nonce [P8]. The brief's two-stage tail model does not describe this code.
An extra block could turn four stages into five stages, suggesting a
20-percent candidate-rate reduction in a simple equal-cost model. That
is not a benchmark. Neither that model nor the brief's one-third slowdown
is a measured P2SH32 cost. P2SH20 keeps the block count; its actual speed
still needs measurement and new Rust, CUDA, HIP and wgpu layouts.

## 3. PHOTON shares and share-chain rules

Use new share types, a new genesis identifier, a separate database, and
distinct P2P protocol identifiers. Never put a PHOTON claim into a BCH
`ShareHeader.bitcoin_header`. Never credit one proof on both chains.

### Canonical fields

The proposed wire version uses fixed-width little-endian integers,
raw hash bytes, and minimally encoded CompactSize lengths for byte
strings and lists. Hash display order is not wire order. Reject duplicate
fields, trailing data, nonminimal encodings and unknown versions. This
is a format proposal; golden byte vectors must precede implementation.

| Group | Proposed fields and widths |
|---|---|
| Domain | Wire version, 2 bytes; chain identifier, 32 bytes; BCH network identifier, 4 bytes; category, 32 bytes; vault lock hash, 32 bytes |
| DAG metadata | Parent share identifier, 32 bytes; height, 8 bytes; timestamp in Unix seconds, 8 bytes; ordered uncle identifiers, each 32 bytes |
| Work and ownership | Share target, 32 bytes; miner's token payout locking bytecode with length; authenticated per-share fee policy, if approved |
| Baton anchor | Actual spent txid, 32 bytes; output index, 4 bytes; source value in satoshis, 8 bytes; source token amount in raw units, 8 bytes; source commitment, 100 bytes; observed BCH block hash or unconfirmed-parent reference |
| Distribution | Canonical parent-window weights root, 32 bytes; total integer weight; tree parameters and covenant version |
| Proof | Compressed public key, 33 bytes; nonce, 4 bytes; datasig, 64 bytes; tail decrement in raw units, 2 bytes; age in BCH blocks, 2 bytes |

Proposed weight fields use unsigned 64-byte little-endian integers.
Their range and arithmetic bounds must be checked, not truncated.
The last group alone is 105 bytes. It is not the whole share or a complete
baton proof. Peers cache authenticated baton state, job context and parent
data. A cold peer must fetch them. Target integers and weights require
explicit overflow bounds in the final format.

Let `metadata_hash` be SHA256 of all canonical metadata above except the
proof group. Include every field that can change identity, credit, fork
choice or validation. Bind it in `R` before work starts. A share identifier
is HASH256 of a separate domain tag, canonical metadata and canonical proof.
Do not include actual winning txid in that identifier: its locktime variant
is not known when work starts. The spent baton outpoint is included.

The payout vector is calculated from the parent, excluding the current
share and its newly referenced uncles. This avoids a self-reference:
current proof determines future credit, not its own reward commitment.
The first job uses an explicit bootstrap distribution with the miner as
the sole beneficiary until credited history exists. That exception and
the genesis domain must be visible to every miner.

### Validation order

1. Bound lengths and queue work before expensive processing. Check domain,
   version, category, canonical encoding and duplicate share identifier.
2. Resolve the parent and uncles. Check height, time rules, uncle eligibility
   and the required share target. Rebuild the prior-window weights and `R`.
3. Authenticate the spent baton against a validated BCH source transaction
   and its ancestry. Check lock, category, value, mutable capability,
   commitment and token amount. A peer announcement alone is insufficient.
4. Check the claimed age against that source's authenticated BCH anchor,
   then derive the PHOTON target. The pool profile sets sequence equal to
   the claimed age. Validate old shares against their committed historical
   context, not the current tip's age. For unconfirmed chaining, validate the
   entire predecessor claim path and use age 0 BCH blocks.
5. Reconstruct the exact claim using the public key, nonce, datasig, tail
   value and expected P2SH20 output. Check amount encoding, fee, dust,
   conservation and the signed positive target predicate. Verify one
   datasig and one HASH256. Reject altered templates.
6. If the hash meets the share target, credit one chain share. If it also
   meets the contract target, immediately broadcast the claim. An eligible
   contract win must be forwarded even if it misses a harder share target.
   Record it separately; it does not acquire unearned share-chain credit.

Do not require every easier-target share to pass the contract's winning
PoW test. Validate the other claim rules with a proof-aware verifier.
For an actual win, use the unmodified covenant in the BCH 2026 VM and
BCHN transaction acceptance. These are distinct checks.

### Targets, PPLNS and uncles

Keep three targets separate: the PHOTON contract target, the deterministic
share-chain target, and the connection's statistical vardiff target.
Only the share-chain target earns PPLNS work. Vardiff submissions below
chain difficulty are local measurements and never enter the DAG.

Proposed starting parameters are a 10-second mean chain-share interval
and a 600-second ASERT half-life, reusing the existing integer ASERT core
[B1]. Re-anchor from the parent as this fork does. Replace BCH target
ceilings and compact-header assumptions. Fix the PHOTON share-target
ceiling and bootstrap target in a new chain profile after simulation.
All targets must be positive and at most `2^255 - 1`, the positive
32-byte ScriptNum maximum.
The share target is not derived from claimed hashrate or connection vardiff.

For a strict positive target `T`, valid random digests number `T - 1`
out of `2^256` digests, when `T <= 2^255 - 1`. Define credited integer work
as `floor(2^256 / (T - 1))`, with `T > 1`; reject other targets.
This definition must be shared by fork choice and PPLNS. Reusing an
unsigned inclusive BCH target-to-work conversion would be wrong.

Use a fixed work budget for the PPLNS window, proposed as 4,320 times
bootstrap-share work. That is nominally 12 hours at the proposed
10-second interval, not a rolling promise of 12 hours after hashrate
changes. Clip the oldest included work to the remaining budget. Aggregate
by payout locking bytecode. No BCH coinbase maturity skip, traded payout
rights, BCH-difficulty multiplier or BCH-denominated dust merging applies.
An alternative window tied to PHOTON expected claim work needs explicit
pool-hopping analysis before selection. The owner must select the budget.

Propose at most 2 uncle references per share and a 6-share ancestry bound,
alongside a proposed 60-second stale-parent credit window. These are
design parameters, not v3.2 limits. Reuse the existing ancestor-off-chain
uncle meaning, deduplication and proposed 90-percent uncle / 10-percent
includer work split [B2]. Credit each proof once. A share on an old baton
can remain eligible for uncle credit if it met the freshness rule when
published. It cannot win against an already spent baton.

Timestamps must exceed the median of the preceding 11 share timestamps;
a proposed future allowance is 15 seconds. Both are proposed PHOTON
share-chain rules. Late jobs and old parents stop earning credit. However,
timestamp checks and a 6-share bound alone do not prove a 60-second global
wall-clock bound, especially during outages or private forks. Local
first-seen times must not silently become divergent share-chain rules.
Specifying deterministic freshness, delayed sync and fork behavior is a
launch blocker, also for the security claim in section 4.

## 4. The payout commitment R

`R` is the full root redeem bytecode. The claim's output index 1 is exactly
`OP_HASH160 <HASH160(R)> OP_EQUAL`. Nodes rebuild `R`; they never accept
a miner-supplied script hash without its expected preimage.

The canonical constructor contains these immutable values:

- A PHOTON-pool domain and covenant version, BCH network, token category
  and deployment lock hash.
- The expected parent share identifier, spent baton outpoint,
  `metadata_hash`, and canonical parent-window Merkle-sum root and weight.
- Allocation order, tree shape, depth bounds, the reviewed program identifier,
  and output/funding rules from section 5.
- An optional commitment to the claim datasig public key and nonce, if the
  auditor requires it. This changes `R` per nonce, not per tail candidate.

The context digest is a pushed constant consumed by the script, even
where it does not affect payout arithmetic. It binds the PoW to the new
share's payee, time, target and uncles. Omitting it would let someone
relabel a public proof to collect future credit. Do not put the current
share identifier, datasig or realized tail reward into `R`: those would
create a circular dependency or rebuild it for every tail candidate.

`R` commits weights, not final token amounts. The root reads the actual
reward amount when spent. Different miners on the same parent therefore
commit the same beneficiary weights, but generally different redeem
scripts because their metadata differs. The brief's "same payout" is an
economic statement, not necessarily an identical output script.

### Conditional P2SH20 argument

P2SH20 has a 160-bit hash. A generic chosen-pair collision takes work on
the order of `2^80` HASH160 evaluations, not `2^160` [S2]. Here a malicious
redeem script and an acceptable `R` must collide while that exact parent
can still earn credit. Every accepted `R` includes a fresh parent identifier
and changing payout root. An old collision cannot be moved to a new parent.

If the usable interval really is 60 seconds, `2^80 / 60 seconds` is about
`2.0 * 10^22` HASH160 evaluations per second. This is arithmetic under a
generic-hash model, not a measured attack rate. The brief's comparison to
"20 times the Bitcoin network" is unsupported by these sources and mixes
HASH160 with ASIC SHA256d work. It is not used as evidence here.

A precomputed malicious table of `2^50` entries leaves order `2^110`
online trials to hit it in a 160-bit range under the random-function
model. Finding an alternative script for an already fixed honest reward
is a second-preimage problem of order `2^160` evaluations. Grinding a
payout address in advance does not predict future parent-bound roots.
These statements assume sound SHA-256/RIPEMD-160 composition and no
special attack on the restricted script family; they are not proofs of it.

Required conditions are strict script reconstruction, binding of every
credit field, fresh unpredictable parent context, and a short enforceable
eligibility interval. A root that repeats across windows is unacceptable.
Privately mined parents, eclipse attacks, outages and bootstrap must be
included in the audit: a miner can predict its own parent, so "a parent
exists" alone is insufficient. Optional pubkey/nonce binding costs about
one root HASH160 per nonce, amortized over up to 65,536 tail candidates
[P7; design choice]. It does not repair an unlimited freshness interval.

After confirmation the output may wait indefinitely. That does not extend
a chosen-pair search against a new honest script: its hash is already
fixed. It does require second-preimage security and permanent data
availability. P2SH20 acceptance remains conditional on the auditor's
freshness analysis. If it fails, the owner must accept a measured P2SH32
kernel cost or stop this design; no live deployment should assume the
60-second claim has been proved.

## 5. A funded fungible-token payout covenant

### Allocation and tree shape

Use a balanced binary tree of weight commitments. Proposed terminal
buckets contain at most 8 beneficiary scripts; internal nodes have at
most 2 children. These are conservative prototype parameters. Increase
them only after measuring unlocking size, hash budget and execution cost.
Do not inherit the BCH tree's 500-output fan-out.

Aggregate duplicate scripts and sort by locking bytecode. Each beneficiary
has a positive integer weight and a cumulative interval `[lower, upper)`.
Proposed leaf hashes are SHA256 of a leaf-domain byte, canonical script
length and script, and its 64-byte weight. Internal hashes are SHA256 of
a distinct node-domain byte, left hash and weight, then right hash and
weight. Hashes are 32 bytes; weights are 64-byte little-endian integers.
Use domain byte `0x00` for leaves and `0x01` for internal nodes. Terminal
buckets reveal their individual leaves and the deterministic subtree
needed to recompute the committed hash. Split the sorted leaf list at
its midpoint, with the larger half on the right for odd leaf counts.
Let `total_weight` be the root weight and `reward` the actual root token
amount, both in raw integer units. Allocate

`floor(reward * upper / total_weight) - floor(reward * lower / total_weight)`

raw PHOTON units. Adjacent intervals telescope, so allocations sum exactly
to `reward`. Use wide integer intermediates with reviewed bounds. This
avoids independently rounded splits losing tokens, and avoids a new root
hash for each tail amount. Zero allocations produce no token output.
An entitlement smaller than one raw token unit cannot be paid in that
win; the window offers future opportunities, not an accrual ledger.
No positive entitlement is donated to a larger miner to solve BCH dust.

Root state lives in `R`, not an NFT. After the root observes `reward`,
child redeem scripts pin that reward, the global denominator, interval,
subtree commitment, category and remaining depth. Children use P2SH32:
their locks are outside the mining preimage and incur no PHOTON kernel
size penalty. Terminal buckets pay token-aware P2PKH, P2SH20 or P2SH32
addresses supplied by members.

Each internal spend reveals child subtree hashes, weights and intervals.
The covenant recomputes the committed Merkle-sum node and verifies exact
partitioning. Proposed redeem encoding is a minimal push of canonical
immutable state followed by a fixed program. The program uses
`OP_ACTIVEBYTECODE` to extract its own exact program suffix, constructs
the canonical child state, appends that same suffix, then hashes the child
redeem to form its P2SH32 lock. No `OP_CODESEPARATOR` is permitted. Root
state reads the reward dynamically; child state fixes it. This avoids
embedding a program recursively inside itself. It must not trust
witness-supplied child locks or program bytes. Exact state encoding,
bytecode extraction and maximum depth need VM proof and budget measurement;
no executable covenant is supplied by this proposal.

### Enforced transaction boundary

The proposed payout spend has one active distribution input, always at
input index 0. It has at least one additional funding input. Enforce
that all additional inputs contain no tokens. Never combine two payout
nodes in one transaction in this first design. This prevents one output
set satisfying multiple independent covenant obligations.
Require transaction version 2, locktime 0 and final sequences on every
input. Funding must not add a new timelock or another malleable locktime.

The payout covenant enforces all of the following, even when an adversary
constructs the transaction:

- The active input has the expected category, exact prescribed token
  amount for a child, and the expected script. The authentic root starts
  fungible-only. Token-free funding inputs cannot introduce an NFT of
  that existing category under BCH token validation [S1]. Category and
  empty commitment introspection alone cannot distinguish an immutable
  NFT with an empty commitment; do not use that as a no-NFT test.
- Outputs start with the exact ordered positive child or beneficiary
  allocations. Their token prefixes and scripts match the committed
  allocation. Their total token amount equals the entire active input
  amount. No burn, token change, token fee, minting authority or alternate
  recipient is permitted.
- Output count is exactly that allocation count, optionally followed by
  one token-free BCH change output. No unvalidated extra outputs are
  permitted. Funding signatures authorize that change and the fee.
- Each allocation output receives at least its committed BCH minimum.
  The sum of allocation-output satoshis is at least
  the active input's satoshis. Thus existing reward BCH cannot be peeled
  off to a change address or fees. The external funder pays missing dust
  and the entire fee.
- There is no owner key, timeout confiscation, privileged executor,
  requirement to spend the vault again, or expiry of token entitlements.

The covenant cannot read relay policy. The proposed profile commits base
minima using the default rate and largest supported amount prefix listed
below. Funding nodes choose `max(local_dust_threshold, committed_minimum)`
per output and assign inherited excess to the first positive output.
The covenant permits larger BCH donations while enforcing those minima
and aggregate preservation. Child scripts depend on token state, not
donated BCH amounts. All token amounts and locks remain exact.

Funder inputs use ordinary valid BCH signatures, normally
`SIGHASH_ALL | SIGHASH_FORKID`, committing input outpoints and all outputs.
Prefer the additional `SIGHASH_UTXOS` binding where supported; the ordinary
ALL mode does not authenticate every other input's source value and script.
The covenant needs no signature. Anyone can choose their own token-free
inputs, pay the shortfall and fee, and receive their own BCH change.
The covenant does not authenticate a particular funder. Multiple funders
can cooperate by signing the same complete transaction. Nodes must never
sign an unknown input or offer a public unrestricted BCH wallet.

### Dust, fees and liveness

At BCHN's default dust relay rate of 1,000 satoshis per kilobyte,
the threshold is `3 * (serialized_output_bytes + 148)` satoshis.
It includes the CashTokens prefix [S1, S3]. For a fungible amount encoded
in 9 bytes, thresholds are 675 satoshis for P2PKH, 669 satoshis for
P2SH20 and 705 satoshis for P2SH32. For a 5-byte amount they are
663, 657 and 693 satoshis respectively. These are relay-policy values,
not token denominations or immutable BCH consensus rules.

For example, splitting a 700-satoshi root into two P2SH32 token children
with 9-byte amounts needs at least 1,410 satoshis in allocation outputs.
Outside funding supplies at least 710 satoshis plus the transaction fee.
Terminal outputs need their own token-aware dust. In general, paying
`member_count` members costs roughly `member_count` dust allocations
plus all tree transaction fees; exact cost depends on amount encodings,
script sizes, inherited BCH and tree shape. Do not quote a fixed fee
without constructing the transactions.

Default executors use local BCHN relay policy and construct a signed
funding transaction only within an operator-approved BCH budget. A node
operator may sponsor it. A beneficiary may fund the branches leading to
their own allocation, creating sibling outputs without executing them.
Users may cooperate to fund it. Their incentive is access to the tokens;
there is no assumed token sale, reimbursable BCH claim or automatic
executor bounty. A different incentive would require owner approval and
new covenant rules.

If nobody funds execution, the tokens wait under the committed script.
There is no fee deduction from token entitlements and no expiry. This
means the protocol does not destroy or forfeit tokens, not that recovery
is possible after everyone loses the redeem scripts and proofs. Replicate
the full allocation manifest, root and child scripts, weights, program
versions and proofs with each share. Keep winning manifests independently
of ordinary DAG pruning. Serve and export them so members can retain
offline recovery copies. A share's data must be available before credit.

### What the existing tree actually does

The existing tag `payout-hybrid-t3072-f500-r1to3-z8-cov1` names a BCH
payout rule used on token-bearing BCH mining jobs. It does not describe
a fungible-token distribution contract [B3-B6]. Its covenant permits
one input, pays its fees from that input, fixes BCH output variants,
and rejects token outputs. It uses P2SH32 and no NFT. The amount fields
are satoshis; dust merging can move a small payee to the largest payee.
It cannot spend the proposed PHOTON reward unchanged.

Reuse canonical manifest storage, deterministic tree construction
patterns, hash-domain separation, VM budget measurements and executor
recovery ideas. Replace the covenant, allocation units, amount-dependent
tree builder, fee model and output validation. The PHOTON profile needs
its own tag. Its root is P2SH20; its children can retain P2SH32. The lack
of an NFT is handled by immutable redeem-script state, not by changing
the PHOTON claim prefix.

## 6. Wins, the baton race and transaction malleability

Every claim spends the same current baton. Two wins against that outpoint
conflict. BCHN's first-seen policy is local mempool behavior, not a global
winner election. BCH inclusion and subsequent reorgs determine settlement.
The pool cannot reserve the baton or prevent solo miners winning it.

Age 0 BCH blocks permits an unconfirmed successor to be mined. Several
claims can form a mempool chain and land in one BCH block [P1, P4]. On
every successor or oracle spend, cancel old jobs immediately. Nodes may
speculatively mine a validated unconfirmed successor, but must label and
track its dependencies. Reorgs or replaced ancestors cancel the whole
dependent branch. A child referring to one txid variant does not spend
another variant's output.

Anyone holding the public key, age, commitment and transaction bytes can
broadcast a win. No recipient signature is needed. Nodes validate and
relay a win immediately, ahead of ordinary share gossip. They persist
its proof, spent baton outpoint, payout manifest, canonical transaction,
all observed txid variants and confirmed successor. Record submission,
mempool acceptance and BCH confirmation as different states.

Actual locktime is malleable because the proof preimage ends in a literal
0 locktime [P1]. A third party can change actual `nLockTime` and rebroadcast
without changing the checked PoW. The variant must still be final under
BCH rules. A future locktime can delay acceptance; it cannot redirect the
fixed outputs. Version, outpoint, input bytecode, sequence and outputs
are bound by the reconstruction; changing them generally needs new PoW.
Pool shares require canonical minimal pushes and locktime 0, but tracking
must recognize valid on-chain variants that do not meet that pool policy.

Track conflicts first by the baton outpoint spent, then by normalized
proof and outputs. One spent outpoint can have several distinct valid
winning proofs as well as locktime variants. Do not collapse their payout
manifests into one. Select the actual accepted transaction before building
a payout spend, whose input must name its real output-index-1 outpoint.
Share-chain reorganization after a win cannot rewrite its locked payout.

An oracle user can stale everyone's jobs by depositing at least
8,000 satoshis per move plus fees [P1]. This affects solo and pool miners.
Follow valid oracle transitions, invalidate work promptly, and rate-limit
peer processing without ignoring real chain changes. The pool cannot ban
the oracle path or guarantee a useful bound on an attacker's budget.

## 7. Sister node and reuse plan

Propose a new `p2poolv2_photon` binary and PHOTON modules or crate, behind
interfaces extracted only where reuse is real. Preserve the BCH program's
wire format and behavior. Architecture sources [B1-B9] and direct module
inspection give the following boundaries. Estimates are initial engineering
effort in engineer-weeks, including focused tests; they are not measured
delivery dates and do not include external audit waiting time.

| Module or seam | Carries over | Must change | Estimate |
|---|---|---|---|
| `node/mod.rs`, `node/behaviour`, `node/connection_tracker.rs` | Noise transport, connection limits, reconnect, address filtering | PHOTON identifiers, seeds and independent network configuration | 1 to 2 engineer-weeks |
| `node/actor.rs`, workers and channels | Bounded queues and validation/organization worker pattern | Concrete BCH actor wiring; proof, job and chain interfaces | 2 to 4 engineer-weeks |
| `shares/share_block/mod.rs`, `node/messages.rs`, `stratum/emission.rs` | DAG concepts and header-sync patterns | New PHOTON header/body, canonical serializer and proof; BCH header, coinbase and merkle branches are replaced | 2 to 4 engineer-weeks |
| `store`, `store/writer`, `store/organise`, chain handle | RocksDB lifecycle, serialized writes, atomic batches, locator and chain-index algorithms | Generic validated-work/header access, codecs, new database/genesis and PHOTON pruning depths | 2 to 4 engineer-weeks |
| `pool_difficulty.rs` | Integer ASERT arithmetic | Positive strict-target work, PHOTON ceiling, anchors and parameters | 1 to 2 engineer-weeks |
| `accounting/payout/sharechain_pplns` | Incremental window cache, work aggregation and uncle bookkeeping | PHOTON window, credit fields and rounding; remove BCH maturity, block-work sizing and traded payout rights | 2 to 4 engineer-weeks |
| `shares/bch_payout_covenant`, `accounting/payout/payout_tree` | Test-harness and canonical-manifest patterns | New funded fungible covenant and weighted token tree | 3 to 6 engineer-weeks |
| `node/payout_tree_executor`, `store/payout_tree`, API | Durable recovery, manifest serving, rebroadcast/reorg concepts | Sponsor budgets, dynamic funding/change, actual outpoint tracking; remove coinbase-maturity and fixed-txid assumptions | 2 to 4 engineer-weeks |
| Miner-facing protocol and job source | Rate-estimation concepts and BCHN RPC methods | PHOTON messages, baton watcher, job generation and submission | 2 to 4 engineer-weeks |
| Cross-node verification and independent interoperability | Existing multi-node harness approach | Forks, funding adversaries, wins, reorgs, malicious jobs and external-client vectors | 3 to 6 engineer-weeks |

`ShareHeader.bitcoin_header` is a concrete BCH header. Its serializer also
carries BCH coinbase and auxiliary-proof fields [B7]. `Emission`, P2P
header batches, writer commands and `NodeActor` use those concrete types.
They cannot carry over unchanged. Extract a small DAG header/work interface
and a separate proof validator; retain distinct BCH and PHOTON messages.
Do not force PHOTON into the existing share transaction or coinbase model.

The storage mechanisms can be reused with different typed records. The
existing PPLNS walker is a useful seam, but its BCH block-difficulty window,
100-share payout skip and payout-right transfer logic are not PHOTON
rules. The executor's separate 100-BCH-block coinbase maturity does not
apply either. Full source files should not be copied and renamed merely
to claim reuse. Integration work may overlap; the table is not a summed
calendar schedule. Pickaxe changes and independent audit are additional.

## 8. Miner protocol and Pickaxe work

Publish a versioned PHOTON protocol specification and byte vectors before
miners depend on it. Pickaxe already prefers Stratum V2 toward pool nodes
[P5]. Prefer a negotiated PHOTON extension over authenticated Noise channels.
It is not compatible with ordinary BCH SV2 jobs just because it shares a
transport. Reserve message identifiers and capability negotiation through
the applicable protocol process; do not silently overload BCH messages.

The protocol needs these operations:

- Open a PHOTON channel with chain/deployment identifier, worker label,
  payout script, supported layouts and covenant versions. No withdrawal
  key or wallet seed is sent.
- Deliver a job identifier and generation, complete baton anchor and
  source evidence, parent and uncle context, PPLNS manifest/root, canonical
  metadata and `R`, age, amounts, fee/value policy, template, and the
  contract, chain and vardiff targets. Include enough data for the client
  to rebuild the template and check its beneficiary vector.
- Update only the statistical target with a defined activation job.
  A proposed starting aim is one statistical submission per 5 seconds
  per connection, bounded by server capacity. Keep that target at least
  as easy as the chain target. Strong miners may exceed that submission
  rate because every chain share must be sent.
- Submit job identifier/generation, public key, nonce, datasig, tail value
  and any explicitly negotiated reconstruction fields. Return accepted
  statistical work, accepted chain share, stale, duplicate or invalid
  with a precise reason. A claimed hashrate earns nothing.
- Deliver immediate job cancellation on baton movement and new work on
  share-tip changes. Support parent/manifest fetch, win announcements,
  payout status and recovery-manifest export.

Pickaxe already passes a comparison target separately from transaction
bytes [P8]. It needs a share mode in that path, with contract wins
never dropped by vardiff. It needs P2SH20 age layouts and conservation
vectors in Rust, CUDA, HIP and wgpu. A root change affects the hashed
template even if the byte count stays constant. Verify cached midstates,
tail offsets, sign-bit filtering, target changes, CPU/GPU equivalence and
winning covenant execution for each layout. Do not publish a throughput
claim from block counts alone.

Jobs may come from a local sister node or an authenticated remote node.
Local validation gives the miner its own view of the DAG. Remote-only
operation trusts that node for freshness and availability, so clients
should be able to fetch history or check another peer and switch nodes.
Pin or verify the remote Noise identity. A remote node cannot be allowed
to disguise a solo-to-operator job as a pool job. The client displays the
category, pool chain, payout script and any fee policy it actually verifies.

Use separate work and wallet responsibilities. Existing external signing
or miner facilities supply transient datasig keys; the sister node does
not add a general private-key generator. Any client can implement the
public messages and vectors. No Pickaxe donation or operator fee becomes
a pool-wide rule by accident. Such cuts, if approved, belong to each
share's authenticated credit policy, not the finding miner's whole window.

## 9. Attacks and limitations

| Attack | Answer and remaining limitation |
|---|---|
| Withhold a full win while submitting easier shares | A node reconstructs every submitted share and broadcasts any win itself. A miner can still discard winning candidates before submission. Public shares do not prove that all candidates were reported; simulation must quantify withholding incentives. |
| P2SH20 chosen-pair collision | Parent-bound reconstruction and short eligibility constrain online work only under section 4's conditions. Freshness/private-fork review is a blocker. P2SH32 is the fallback with measured kernel work. |
| Oracle griefing or ordinary solo claims | Follow real baton moves and cancel jobs. Oracle moves cost at least 8,000 satoshis plus fees; neither mode can exclude them. |
| Share spam and forged state | Size bounds, per-peer budgets, dependency deduplication, cheap checks before crypto, deterministic chain difficulty, and authenticated baton sources. Weak vardiff work stays local. |
| Selfish share mining, timestamp grinding, stale-parent replay | Greatest cumulative valid work, metadata bound into `R`, unique proof credit and bounded uncle inclusion. Freshness and time rules need adversarial simulation; no claim of eliminated selfish-mining advantage. |
| A pool miner broadcasts a win to its own address | Changing output index 1 changes the checked HASH256. Reusing the same datasig does not preserve the PoW. A miner can mine a separate solo template, but that proof fails the pool's expected-`R` check and earns no pool credit. |
| Duplicate payout inputs or reused output obligations | Exactly one distribution input at index 0; all funding inputs token-free. Check every input and output, not just the active input. |
| Token burn, fake child lock or altered rounding | Exact token sum, authenticated weighted intervals, approved child-program reconstruction and no extra token outputs. BCH token conservation alone permits burns, so it is insufficient. |
| Funding fee griefing | No inherited payout BCH can go to change or fees. Funders sign complete transactions and fund all fees. Never assume child-pays-for-parent will rescue a low-fee claim. No free public funding wallet. Competing funded spends may delay each other under local first-seen policy. |
| Nobody sponsors distribution | Tokens wait with no deadline. Settlement liveness depends on a willing funder and retained manifests. Small entitlements may not justify their BCH cost. |
| Missing payout data or eclipse | Require data before credit, replicate and export winning manifests, retain them beyond DAG pruning, use independent peers. Hashes alone cannot recover missing scripts. |
| Reorg or locktime malleability | Persist conflict sets by spent baton and proof, resolve actual txids, roll back dependent work and payout tracking. Never treat RPC acceptance as confirmation. |

## 10. Open decisions and phases

The owner must decide the share/PPLNS parameters, uncle policy, bootstrap
beneficiary rule, optional per-share fees, sponsor budget and who is
expected to fund small payouts. The proposal does not assume sponsors
will recover BCH from PHOTON. The owner must also approve any P2SH32
fallback after measured performance, rather than accept a false security
or speed guarantee.

Review and implementation gates, in order:

1. **Design review only.** Pickaxe reviews actual template and kernel
   implications. The independent auditor reviews parent unpredictability,
   private forks, freshness/sync rules, proof binding, no-NFT state,
   funding isolation and child-script construction. Resolve work bounds,
   tree depth, zero allocations and the integer window policy.
2. **Covenant feasibility, after approval.** Build a Rust-only prototype
   in the existing BCH 2026 VM harness. Measure bytecode, unlocking length,
   hash/operation budgets and token-aware dust for the largest supported
   tree. Mutate token amounts, templates, multiple input groups, funding
   order, change, NFTs, burns and all rounding boundaries. Validate whole
   transactions with BCHN, not only individual script invocations.
3. **Protocol and simulation.** Publish golden share/job/payout vectors.
   Simulate honest latency, hash changes, partitions, private forks,
   withholding, oracle moves, long outages and startup. Freeze a separate
   profile and tag only when those results support the chosen parameters.
4. **Sister node and independent clients.** Extract the minimum shared
   interfaces. Implement DAG/store/proof/manifest paths and Pickaxe layouts.
   Require another client to submit shares and independently reconstruct
   a win and payout from the public specification.
5. **Chipnet before mainnet.** Exercise real multi-node mining, competing
   wins, mempool chaining, locktime variants, oracle moves, funded tree
   execution, sponsor refusal, restart and BCH reorg recovery. Use separate
   authorization for any spending. Mainnet needs the owner's approval
   after the audit and measured results.

Source checks contradicted or qualified these parts of the initial brief:
the two-compression conserved tail model and its P2SH32 slowdown estimate;
an always-unsigned transaction hash; amount-encoding limits as a complete
future profile; the reward-category check; an existing token-distribution
covenant; identical scripts
for all miners on one tip; and a proved global 60-second collision window.
The v3.2 oracle deposit rule is confirmed. Actual locktime malleability is
confirmed by the reconstruction, subject to BCH finality. No current
network-rate claim or live-chain compatibility is established by this doc.

## Sources

PHOTON sources were inspected read-only in `D:/pickaxe-electric`. Links
pin the inspected revision; byte offsets above are derived from its
serializer, not from comments in the template.

- **P1:** [v3.2 template](https://github.com/CyberAshven/pickaxe-miner/blob/2f7cbb0493f2a46ef89d4f04c941074311a09ba0/reference/photon_v32_template.json), `scripts.lock.script`; opcode source is authoritative where comments disagree.
- **P2:** [v3.2 redeem hex](https://github.com/CyberAshven/pickaxe-miner/blob/2f7cbb0493f2a46ef89d4f04c941074311a09ba0/reference/photon_v32_redeem.hex); decoded length and HASH256 checked against P3.
- **P3:** [protocol constants and derivation](https://github.com/CyberAshven/pickaxe-miner/blob/2f7cbb0493f2a46ef89d4f04c941074311a09ba0/src/protocol.rs), `PhotonDeployment`, `ProofRule::Positive`, `derive_photon_state`.
- **P4:** [protocol profile](https://github.com/CyberAshven/pickaxe-miner/blob/2f7cbb0493f2a46ef89d4f04c941074311a09ba0/docs/protocol-profile.md), layout shifts and supported ages.
- **P5:** [Pickaxe architecture](https://github.com/CyberAshven/pickaxe-miner/blob/2f7cbb0493f2a46ef89d4f04c941074311a09ba0/docs/architecture.md), mining destinations and GPU engines.
- **P6:** [reward handling](https://github.com/CyberAshven/pickaxe-miner/blob/2f7cbb0493f2a46ef89d4f04c941074311a09ba0/src/reward.rs), `TOKEN_OUTPUT_SATS`, retired-v0 constants and settlement paths.
- **P7:** [transaction serialization](https://github.com/CyberAshven/pickaxe-miner/blob/2f7cbb0493f2a46ef89d4f04c941074311a09ba0/src/tx.rs), `PhotonLayout`, conserved T2 amount updates and encoding guards.
- **P8:** [Rust T2](https://github.com/CyberAshven/pickaxe-miner/blob/2f7cbb0493f2a46ef89d4f04c941074311a09ba0/rust-engine/src/t2.rs) and [CUDA tail loop](https://github.com/CyberAshven/pickaxe-miner/blob/2f7cbb0493f2a46ef89d4f04c941074311a09ba0/cuda/photon_t2_tail.cu), block preparation and conserved tail hashing.
- **B1:** [ASERT design](../difficulty_adjustment/asert.md) and [integer target implementation](https://github.com/CyberAshven/BitcoinCash-p2poolv2/blob/044391e3966539f26d6b471d64334cbe600f602b/p2poolv2_lib/src/pool_difficulty.rs).
- **B2:** [Payout distribution](payout-distribution.md) and [PPLNS implementation](https://github.com/CyberAshven/BitcoinCash-p2poolv2/blob/044391e3966539f26d6b471d64334cbe600f602b/p2poolv2_lib/src/accounting/payout/sharechain_pplns/pplns_window.rs).
- **B3:** [Hybrid BCH payout design](bch-hybrid-payouts.md).
- **B4:** [BCH payout covenant](https://github.com/CyberAshven/BitcoinCash-p2poolv2/blob/044391e3966539f26d6b471d64334cbe600f602b/p2poolv2_lib/src/shares/bch_payout_covenant.rs), module contract and `redeem_script`.
- **B5:** [BCH tree builder](https://github.com/CyberAshven/BitcoinCash-p2poolv2/blob/044391e3966539f26d6b471d64334cbe600f602b/p2poolv2_lib/src/accounting/payout/payout_tree.rs), `TreePayee` and dust/fee allocation.
- **B6:** [Tree executor](https://github.com/CyberAshven/BitcoinCash-p2poolv2/blob/044391e3966539f26d6b471d64334cbe600f602b/p2poolv2_lib/src/node/payout_tree_executor.rs) and [manifest store](https://github.com/CyberAshven/BitcoinCash-p2poolv2/blob/044391e3966539f26d6b471d64334cbe600f602b/p2poolv2_lib/src/store/payout_tree.rs).
- **B7:** [Share types and serialization](https://github.com/CyberAshven/BitcoinCash-p2poolv2/blob/044391e3966539f26d6b471d64334cbe600f602b/p2poolv2_lib/src/shares/share_block/mod.rs), [P2P messages](https://github.com/CyberAshven/BitcoinCash-p2poolv2/blob/044391e3966539f26d6b471d64334cbe600f602b/p2poolv2_lib/src/node/messages.rs), and [emission](https://github.com/CyberAshven/BitcoinCash-p2poolv2/blob/044391e3966539f26d6b471d64334cbe600f602b/p2poolv2_lib/src/stratum/emission.rs).
- **B8:** [Share pipeline](share-processing-pipeline.md), [async flow](async-flow.md), [store schema](store-schema.md), [store architecture](store-architecture.md), [pruning](pruning.md), and [BCH anchor rules](mine-on-bitcoin-chain.md).
- **B9:** [Node actor](https://github.com/CyberAshven/BitcoinCash-p2poolv2/blob/044391e3966539f26d6b471d64334cbe600f602b/p2poolv2_lib/src/node/actor.rs), [store writer](https://github.com/CyberAshven/BitcoinCash-p2poolv2/blob/044391e3966539f26d6b471d64334cbe600f602b/p2poolv2_lib/src/store/writer/mod.rs), and [chain handle](https://github.com/CyberAshven/BitcoinCash-p2poolv2/blob/044391e3966539f26d6b471d64334cbe600f602b/p2poolv2_lib/src/shares/chain/chain_store_handle.rs).
- **S1:** [CashTokens specification](https://cashtokens.org/docs/spec/chip/), prefix serialization, introspection and token-validation rules.
- **S2:** [BCHN P2SH security bulletin](https://bitcoincashnode.org/en/newsroom/bchn-technical-bulletin-2022-06-09), script-hash collision and second-preimage distinction. The time/memory calculations here are explicitly conditional design analysis.
- **S3:** [BCHN dust and standardness policy](https://github.com/bitcoin-cash-node/bitcoin-cash-node/blob/master/src/policy/policy.cpp), `GetDustThreshold` and `IsStandardTx`, and [default rates](https://github.com/bitcoin-cash-node/bitcoin-cash-node/blob/master/src/policy/policy.h); checked 2026-10-29. Deployment must recheck the target node's policy.
