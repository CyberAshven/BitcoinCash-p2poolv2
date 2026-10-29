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
layout and hashing questions [P1-P10]. The original review inspected revision
`2f7cbb0493f2a46ef89d4f04c941074311a09ba0`; this revision checks Pickaxe dev
`640e9d24b04f5cc2ce72bedc871f3e3f28422a92`. The template, redeem hex and
protocol constants are unchanged between those revisions. This is source verification,
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
forms. The proposed fast profile uses a 23-byte P2SH20 reward lock;
P2SH32 remains the owner-selectable fallback, subject to section 4's audit.

Pickaxe's reward constant is 700 satoshis [P6]. For a current P2PKH claim,
fees of 629 to 631 satoshis at 1 satoshi per byte leave a vault decrease
of 1,329 to 1,331 satoshis. A reward of 850 satoshis also fits that
allowance at that fee rate. Thus 700 to 850 satoshis is a feasible
example range, not a contract promise. A P2SH20 token reward output is
75 bytes with the 9-byte amount prefix, giving 669 satoshis of default
dust [S3; serialization arithmetic]. Its 700-satoshi value plus a
627-to-630-satoshi fee at 1 satoshi per byte fits the 1,500-satoshi
allowance for all four age layouts described below [P7].
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
Later emission epochs need separate layouts and VM vectors. Independently,
the initial pool profile restricts claimed age to 0 through 32,767 BCH
blocks. Greater available age does not force a greater claimed age [P1].

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
11 first-pass blocks [P4, P7; serialization arithmetic]. The further age
range 32,768 through 65,534 BCH blocks uses a 4-byte age push and a
3-byte shift: P2SH20 is 630 bytes and still needs 10 first-pass blocks.
P2PKH is 632 bytes and needs 11. Pickaxe's current P2PKH layout rejection
therefore does not prove that P2SH20 needs another block [P7]. This range
is excluded initially; adding it requires its own layouts and vectors.

### Conserved and burning tail profiles

Let `balance` be vault raw units, `base = floor(balance / 420,000) - 1`,
and `tail` range from 0 through 65,535 raw units [P1, P7; proposed bound].
Pickaxe's conserved profile uses reward `base - tail` and vault
`balance - base + tail`. Its `t2_parent_reward_amount` enforces their
sum, but the PHOTON covenant only enforces the vault lower bound above.
BCH consensus permits implicit fungible-token burns [S1, S6]. Conservation
is a miner-profile rule, not an immutable PHOTON requirement.

For conserved tails, amount changes touch zero-based 64-byte blocks 7
and 9; carry can also reach block 8. Keep Pickaxe's `supports_t2_window`
guard: every candidate must leave vault bytes at and after byte 512
unchanged, or use a validated recomputation/fallback for block 8 [P7].
For example, at age 128 BCH blocks, vault amount
2,089,072,092,774,399 raw units plus reward 4,973,993,015 raw units is a
valid conserved starting sum. Tail 1 changes vault bytes
`ff ff ff ff ff 6b 07 00` to `00 00 00 00 00 6c 07 00`, including byte
512. Both amounts exceed the encoding floor. This arithmetic counterexample
explains the guard; block 8 is not unconditionally invariant [P7].

T2 resumes block 7 at word 14, then processes blocks 8 and 9 and the
second hash [P8]. Counting remaining rounds as equal-cost compression
equivalents gives `(50 + 64 + 64 + 64) / 64 = 3.78125`, about 3.8 per
candidate. P2SH32 adds one block, about 4.8 equivalents, or about
21 percent fewer candidates at equal cost [P8; arithmetic]. There are
four stages; that integer count alone misses partial-round savings.

The owner must decide whether to permit a burning profile: vault
`balance - base`, reward `base - tail`, burning exactly `tail` raw units.
The covenant permits this amount relation; blocks 7 and 8 are fixed per
signature. Block 9 resumes at word 4, followed by the second hash:
`(60 + 64) / 64 = 1.9375`, about 1.9 compression equivalents. P2SH32
would cost about 2.9, about 34 percent fewer candidates at equal cost.
Burning could yield about `3.78125 / 1.9375 = 1.95` times the candidate
rate for solo or pool miners if SHA-bound [P1, P7, P8; derived model].
No burning kernel or either P2SH32 comparison has been benchmarked here.
Partial-round instructions, early filtering, signing and root hashing
can change all of these estimates.

Burning loses at most 65,535 raw units per win. A uniformly selected
winning tail would lose 32,767.5 raw units on average; that is an
assumption about winners, not a measured distribution [tail-range arithmetic].
BCHN v29.2.0 rejects output token totals above input totals, but accepts
a positive unused balance; its standardness and mempool paths add no
implicit-burn prohibition [S6]. This is source evidence, not a live
relay test of a PHOTON burning claim. Fees, dust, finality, covenant
execution and all other admission rules still apply. Distribution spends
in section 5 always conserve the reward they actually receive.

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
| Baton anchor | Actual spent txid, 32 bytes; output index, 4 bytes; source value in satoshis, 8 bytes; source token amount in raw units, 8 bytes; source commitment, 100 bytes; authenticated BCH block hash, 32 bytes; ordered vault-transition path and dependency evidence |
| Distribution | Canonical parent-window weights root, 32 bytes; total integer weight; tree parameters and covenant version |
| Proof | Compressed public key, 33 bytes; nonce, 4 bytes; datasig, 64 bytes; tail decrement in raw units, 2 bytes; age in BCH blocks, 2 bytes |

Proposed work and weight fields use unsigned 64-byte little-endian integers.
All accumulators and arithmetic are checked; section 5 freezes their bounds.
The last group alone is 105 bytes. It is not the whole share or a complete
baton proof. Peers cache authenticated baton state, job context and parent
data. A cold peer must fetch them. Target integers and weights require
the bounds below; overflow is an error, never saturation.

Let `metadata_hash` be SHA256 of all canonical metadata above except the
proof group. Include every field that can change identity, credit, fork
choice or validation. Bind it in `R` before tail search starts. A share identifier
is HASH256 of a separate domain tag, canonical metadata and canonical proof.
Do not include actual winning txid in that identifier: its locktime variant
is not known when work starts. The spent baton outpoint is included.

The payout vector is calculated from the parent, excluding the current
share and its newly referenced uncles. This avoids a self-reference:
current proof determines future credit, not its own reward commitment.
The first job uses an explicit bootstrap distribution with the miner as
the sole beneficiary until credited history exists. That exception and
the genesis domain must be visible to every miner.

A server job supplies a reconstruction specification, not a final complete
claim template. The claim public key is the miner's transient datasig key,
rotated on job installation (and potentially again on nonce exhaustion)
[P9]. The miner fills transaction bytes 45 through 77 in the cached
prefix and computes its own midstate. It then fills nonce and datasig,
constructs its seed-bound `R` and reward lock, and prepares the tail loop.
The initial BCH value rule proposes exactly 700 satoshis for the reward
and a fee equal to canonical claim length at 1 satoshi per byte, deducted
from vault BCH [P6; proposed profile]. Pin those exact values in the seed;
unavailable dust/fee conditions pause mining rather than allow extra choices.
The timestamp is metadata bound by `R`, so timestamp/job changes change
`R`. Its output HASH160 occupies block 9 in these layouts; that particular
change does not alter the prefix. Key changes do alter the prefix [P7].

### Validation order

1. Bound lengths and queue work before expensive processing. Check domain,
   version, category, canonical encoding and duplicate share identifier.
2. Resolve the parent and uncles. Check height, time rules, uncle eligibility
   and the required share target. Rebuild the prior-window weights.
3. Authenticate the spent baton against a validated BCH source transaction
   and its ancestry. Check lock, category, value, mutable capability,
   commitment and token amount. Apply the lineage/context eligibility rule
   below; an authentic historical source or a peer announcement is insufficient.
4. Check the claimed age against that source's authenticated BCH anchor,
   then derive the PHOTON target. The pool profile sets sequence equal to
   the claimed age. Validate old shares against their committed historical
   context, not the current tip's age. For unconfirmed chaining, validate the
   entire predecessor claim path and use age 0 BCH blocks.
5. Verify the datasig, bind the complete proof seed in `R` as in section 4,
   then reconstruct the exact claim using the prescribed tail and expected
   reward lock. Check amount encoding, exact BCH values/fee, dust, and
   the selected tail profile's conservation or exact-burn rule. Check the
   signed positive target predicate. Verify one
   datasig and one HASH256. Reject altered templates.
6. If the hash meets the share target, credit one chain share. If it also
   meets the contract target, immediately broadcast the claim. An eligible
   contract win must be forwarded even if it misses a harder share target.
   Record it separately; it does not acquire unearned share-chain credit.

Do not require every easier-target share to pass the contract's winning
PoW test. Validate the other claim rules with a proof-aware verifier.
For an actual win, use the unmodified covenant in the BCH 2026 VM and
BCHN transaction acceptance. These are distinct checks.

### Authenticated baton context and eligibility

This proposed rule starts at a designated deployment vault outpoint,
not any output with the same category and lock. Each share commits a BCH
block context and the complete validated claim/oracle transition path
from that outpoint to its selected baton. Authenticate block inclusion,
source data and intervening spends using a validated BCH chain/UTXO view;
a header or transaction Merkle proof alone does not prove unspentness.
Carry or fetch the evidence needed for reproducible historical validation.

A child context must extend its parent's BCH branch and selected vault
path. Allowable backward block lag and spent-baton lag are both zero in
that committed context: the source must be the path's current unspent tip.
A BCH block advance must incorporate every confirmed transition on the
designated lineage, including oracle moves. New live jobs and credited
tip selection use the latest fully validated BCH block context, with
zero-block lag; missing context pauses them. Historical shares are checked
against their own committed contexts, never today's age or local first-seen
time. Reorganization selects/rebuilds a compatible share branch rather
than rewriting old proofs [proposed profile; compare B8's payout-validity rule].

Unconfirmed predecessors are permitted only on an explicit selected path.
Validate every predecessor claim or oracle transaction, its funding-input
prevouts, dependency spends, token rules and finality. Use claimed age
0 BCH blocks for an unconfirmed source [P1]. Distinct conflicting paths
remain distinct; local mempool first-seen order is not a share-chain rule.
Once a parent selects a successor, descendants cannot revert to its spent
predecessor. Confirmation selects the actual BCH path. A funding conflict
or BCH reorg rolls back the dependent speculative branch and jobs, then
recomputes live credit on a compatible branch. Historical storage does
not imply current credit or successful settlement.

For a paying share, mainline PPLNS work must have been eligible in its
own context and be compatible with the paying BCH branch and ordered
vault lineage. Pre-transition work can remain in the window. An uncle
must additionally use the includer's selected baton and compatible context;
both uncle credit and the includer bonus fail together otherwise. No
uncle may introduce old-baton work after a selected transition. Apply
these checks before assigning any credit, as well as at share acceptance.

For the review's counterexample, if oracle transaction `O` spends `V`,
a current-parent share whose context contains `O` cannot mine against `V`:
its source is not the selected tip. Neither a new timestamp nor authentic
ancestry of `V` repairs that. If `O` is unconfirmed and not yet selected,
competing speculative contexts remain possible until resolved. A private
old-context proof does not reveal its wall-clock creation time. Depth
rules and compatible-context payout filtering bound its credit opportunity;
they do not prove a deadline. Model these cases before profile activation.

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
At `T = 2`, work is `2^256`, requiring 257 bits. Use checked 512-bit
work, cumulative chainwork, window accumulators and weights throughout.
Do not inherit BCH's 256-bit work type or the existing saturating
`work_to_u128` conversion [B2]. At most `2^64 - 1` shares in the proposed
height field bound ordinary cumulative chainwork below `2^320` work units.
Uncle work is paid once but does not add to fork-choice chainwork.

Use a fixed work budget for the PPLNS window, proposed as 4,320 times
bootstrap-share work. That is nominally 12 hours at the proposed
10-second interval, not a rolling promise of 12 hours after hashrate
changes. Clip the oldest included work to the remaining budget. Aggregate
by payout locking bytecode. No BCH coinbase maturity skip, traded payout
rights, BCH-difficulty multiplier or BCH-denominated dust merging applies.
An alternative window tied to PHOTON expected claim work needs explicit
pool-hopping analysis before selection. The owner must select the budget.

Propose at most 2 uncle references per share and a 6-share ancestry bound.
These are depth rules, not v3.2 limits or seconds [proposed profile]. Use
greatest cumulative valid mainline work, breaking ties by ascending raw
share identifier. Keep the existing ancestor-off-chain uncle meaning [B2].
An uncle already canonical or already included anywhere on the selected
ancestry cannot earn credit again. Deduplicate normalized claim proof
and tail, not merely share identifiers or actual locktime variants; keep
a reorg-aware index so moving outside the window cannot renew credit.

Window arithmetic is exact and ordered. Walk mainline entries newest
first; consume each entry's own raw work, then its eligible-position
uncles in ascending normalized proof identifier. For each unique work
item set `clipped = min(work, remaining_budget)` and subtract it from the
raw budget. Own work earns `10 * clipped` weight. An eligible uncle earns
`9 * clipped` for its beneficiary and `clipped` for the eligible includer.
Thus each paid item conserves `10 * clipped` integer weight; never round
90 percent and 10 percent independently [B2; proposed clipping order].
The boundary is ordered, not proportional. Context-ineligible items
consume their raw budget position but receive zero weight, including zero
uncle bonus; do not walk farther to replace them. Check positional validity
and deduplication before this walk. Aggregate the resulting weights by
payout script. An all-zero non-bootstrap window cannot build a job.

Timestamps must exceed the median of the preceding 11 share timestamps;
a proposed future allowance is 15 seconds. Both are proposed PHOTON
share-chain rules. Late jobs and old parents stop earning credit. However,
timestamp checks and a 6-share bound do not prove a 60-second global
wall-clock bound. Own unpublished parents, future-dated parents, outages,
eclipse and predictable bootstrap all defeat that inference. Local
first-seen expiry would make delayed peers disagree. Pickaxe proposes
depth plus cumulative-work fork choice; this proposal adopts that basis
for credit freshness while retaining the explicit context rules above.
Its deterministic sync/reorg behavior still needs adversarial validation.
P2SH20 security in section 4 does not rely on any wall-clock window.

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
- The complete proof seed: compressed claim public key, message nonce,
  exact datasig bytes, claimed age, sequence, derived contract target,
  and every other non-tail claim choice. Pin the amount layout, base
  reward, conserved/burning profile and bounded tail range. Pin actual
  canonical locktime 0, exact vault and reward BCH values and fee rule;
  no uncommitted value or script choice may vary inside one seed.

The context digest is a pushed constant consumed by the script, even
where it does not affect payout arithmetic. It binds the PoW to the new
share's payee, time, target and uncles. Omitting it would let someone
relabel a public proof to collect future credit. Do not put the current
share identifier or realized tail reward into `R`. Bind the rule deriving
the reward from the tail instead. Datasig binding is not circular:
choose key and message nonce, sign `nonce || target`, construct `R`, then
search tails. The datasig does not sign `R` [P1, P7]. A signer can vary
its Schnorr signing nonce `k` to make different valid datasigs for one
key/message nonce; RFC6979 determinism is client policy, not a covenant
restriction [P10]. Key/nonce binding alone is insufficient.

One `R` therefore describes exactly one proof seed, allowing at most
65,536 prescribed tail candidates [proposed profile]. It cannot serve
other seeds on siblings, uncles, branches or parents. Multiple winning
tails for that seed still spend one baton and yield at most one settled
reward. Locktime variants do not create another credited proof or reward.
The miner rebuilds `R` and computes HASH160 once per nonce/signature seed,
amortized over the tail search: one root HASH160 per 65,536 candidates
at a full window, or `1 / candidate_count` per candidate. That evaluation
hashes the full redeem script, not one SHA-256 compression; benchmark
its actual length and cost. It does not lengthen the 23-byte output.

`R` commits weights, not final token amounts. The root reads the actual
reward amount when spent. Different miners on the same parent therefore
commit the same beneficiary weights, but generally different redeem
scripts because their metadata differs. The brief's "same payout" is an
economic statement, not necessarily an identical output script.

### Cost-based P2SH20 decision

P2SH20 has a 160-bit hash. A generic chosen-pair collision takes work on
the order of `2^80` HASH160 evaluations, not `2^160` [S2]. Searching
over seed-derived legitimate scripts and malicious scripts can find a
pair, but the resulting pair serves only its one specific seed. Finding
an alternative script for an already fixed honest `R` is instead an
order-`2^160` second-preimage problem. The proposal uses the cheaper
chosen-pair model conservatively; it does not call a fixed-root search
an 80-bit second-preimage attack.

Pickaxe argues that collision cost, rather than a clock, makes freshness
an avoidable launch blocker for P2SH20. At an assumed
`5 * 10^9 HASH160 evaluations / second / GPU`, and
31,557,600 seconds per year, `2^80` evaluations take approximately
`2^80 / (5 * 10^9 * 31,557,600) = 7.66 million GPU-years` [S2;
Pickaxe review assumptions and arithmetic]. Neither that rate nor an
attack on full-length root scripts has been measured. Memory, hardware,
energy and search constants are omitted; Bitcoin SHA256d ASIC rates are
not HASH160 evidence.

Each found pair is worth at most one PHOTON reward, and only if its seed
also contains a contract-winning tail. For a random accepted share,
`probability = min(1, (contract_target - 1) / (share_target - 1))`.
The familiar `contract_target / share_target` approximation applies when
the contract target is harder and both targets are large [section 3;
uniform-digest arithmetic]. Under an independent repeated-search model
yielding one random accepted share per collision, expected cost per
stolen reward is approximately `2^80 / probability` HASH160 evaluations,
or `7.66 million / probability` GPU-years under the supplied rate.

That multiplier is conditional, not a proved attack lower bound. A seed
with `candidate_count` tails has win probability
`1 - (1 - (contract_target - 1) / 2^256)^candidate_count` under independent
digests; it is not itself a single accepted share. An attacker can test
tails before selecting seed scripts for its collision search. The auditor
must analyze winner prefiltering, unequal search populations, time/memory
tradeoffs and restricted script families before accepting the economic
argument. Binding removes reuse across seeds; it does not enforce the
attacker's order of computation.

A precomputed malicious table of `2^50` entries leaves order `2^110`
online trials to hit it in a 160-bit range under the random-function
model. Grinding a
payout address in advance does not predict future parent-bound roots.
These statements assume sound SHA-256/RIPEMD-160 composition and no
special attack on the restricted script family; they are not proofs of it.

Required conditions are strict reconstruction and complete seed/credit
binding. There is no assumed entropy-generation-to-expiry deadline.
The independent review correctly rejects the earlier 60-second premise
and optional key/nonce binding. Its requirement to use P2SH32 if relying
only on that premise remains sound. Pickaxe's cost argument is a revised
proposal for audit, not evidence that the earlier premise was correct.
Keep section 3's baton eligibility, depth freshness and cumulative-work
rules regardless: they govern fair credit even if collisions are costly.

After confirmation the output may wait indefinitely. That does not extend
a chosen-pair search against a new honest script: its hash is already
fixed. It does require second-preimage security and permanent data
availability. The auditor must re-review this complete-binding/cost
argument. P2SH32 remains the fallback. Its estimated kernel costs are
about 3.8 to 4.8 compressions for conserved tails and 1.9 to 2.9 for
burning tails (section 2); actual costs under both profiles must be
measured before the owner chooses. No P2SH20 deployment approval or
measured P2SH32 result is supplied by this document.

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
The initial limits below are fixed proposal parameters, not measurements
of an executable covenant. A larger profile requires a new version.
Let `total_weight` be the root weight and `reward` the actual root token
amount, both in raw integer units. Allocate

`floor(reward * upper / total_weight) - floor(reward * lower / total_weight)`

raw PHOTON units. Adjacent intervals telescope, so allocations sum exactly
to `reward`. Use checked wide integer intermediates within the bounds below. This
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
witness-supplied child locks or program bytes. The proposed encoding and
caps below are fixed for feasibility testing; bytecode extraction and
enforcement still need VM proof. No executable covenant is supplied here.

### Frozen initial execution profile

All values in this table are proposed profile bounds. BCH limits in the
following table are separately sourced. Reject excess rather than truncate.

| Item | Initial profile bound |
|---|---|
| Beneficiaries after script aggregation | At most 4,096 leaves |
| Tree | Binary midpoint split; at most 12 Merkle edges from root to leaf; at most 9 distribution splits before a bucket of at most 8 leaves |
| Additional funding inputs | At least 1 and at most 4; at most 5 total inputs including distribution input 0 |
| Beneficiary script | Exactly standard P2PKH, P2SH20 or P2SH32, at most 35 bytes |
| State | Exactly 336 bytes, layout below; minimal `OP_PUSHDATA2` push followed by fixed program |
| Program | At most 4,096 bytes; no `OP_CODESEPARATOR`; version identifies exact audited program hash |
| Redeem bytecode | At most 4,435 bytes: 3-byte push header + 336-byte state + program |
| Unlocking bytecode | At most 10,000 bytes for every input; complete input-0 witness must fit this bound |
| Work/weights/intervals | Checked unsigned 512-bit integers, serialized in 64 little-endian bytes; positive total and `0 <= lower < upper <= total` |
| Reward and individual token amounts | From 1 through `2^63 - 1` raw units, also bounded by the authenticated claim; zero allocation omits an output [S1] |
| Arithmetic intermediates | At least 575 unsigned bits for generic 512-bit weight times 63-bit reward; use checked arithmetic and canonical positive ScriptNum conversion |

The state concatenates: version (2 bytes), mode (1 byte: root 0 or child 1),
remaining distribution depth (1 byte, at most 9), BCH network (4 bytes),
category (32 bytes), proof-seed/context digest (32 bytes), audited program
hash (32 bytes), subtree hash (32 bytes), total weight (64 bytes), interval
lower (64 bytes), interval upper (64 bytes), and reward (8 bytes).
Integers are fixed-width little-endian; hashes/category use raw wire bytes.
The sizes sum to 336 bytes [proposed encoding; arithmetic]. Root reward
is the zero sentinel and is read from the active input; every child pins
the positive original reward. Root interval is `[0, total)`; child intervals
partition it. Remaining depth decrements exactly; depth 0 permits only a
terminal bucket. State version and mode select the exact field semantics;
unknown values fail. The context digest includes all section 4 commitments.
`OP_ACTIVEBYTECODE` returns the executing bytecode; with no separator the
program removes the fixed 339-byte state-push prefix to obtain the suffix.
It must verify the version's program hash before reconstructing children.

For `window_budget = 4,320 * bootstrap_work`, scaled total payout weight
is at most `10 * window_budget < 2^272` weight units, even at target 2
[section 3; arithmetic]. Thus this profile's reward product is below
`2^335`; the 575-bit bound also covers the full serialized weight range.
Enforce the tighter window bound when accepting manifests. Funding BCH
values and their sums must pass BCH MoneyRange and checked arithmetic,
never wrap or default. Enforce exact tree counts, intervals and terminal
leaf lengths rather than trusting witness-supplied counts.

Under BCHN v29.2.0, let `L` be the active input's unlocking length in bytes
[S4, S5]. These are BCH VM/admission limits, not extra funding capacity:

| Limit | Bound |
|---|---|
| Executed script and stack item length | 10,000 bytes |
| Combined stack, altstack and function-table entries | 1,000 entries |
| Control depth | 100 levels |
| Operation cost | `800 * (41 + L)` cost units |
| Standard hash iterations | `floor((41 + L) / 2)` iterations |
| BCH consensus hash iterations | `floor(7 * (41 + L) / 2)` iterations |
| Standard unlocking length after P2S activation | Up to 10,000 bytes |

Inspecting funding inputs spends input 0's budget; those inputs do not
increase it. With the specified maximum scripts, a leaf preimage is at
most 101 bytes and an internal preimage 193 bytes, requiring respectively
2 and 4 SHA-256 iterations. An eight-leaf bucket costs
`8 * 2 + 7 * 4 = 44` iterations for its tree alone [state/hash layout;
SHA-256 padding arithmetic]. Program verification, child-script hashing,
arithmetic, token introspection and all other execution are additional.
Measure full internal and terminal transactions, maximum funding and
script sizes, zero allocations, all rounding boundaries and the maximum
tree under standardness and BCH consensus budgets. Old 520-byte item
or 1,650-byte standard unlocking objections do not apply after these
upgrades. Constructibility remains **plausible, not demonstrated**.

### Enforced transaction boundary

The proposed payout spend has one active distribution input, always at
input index 0. It has at least one additional funding input. Enforce
that all additional inputs contain no tokens: for each one compare
`OP_UTXOTOKENCATEGORY` byte-for-byte with the empty vector using equality.
Token amount zero or an empty NFT commitment is insufficient; an immutable
NFT with an empty commitment still has a nonempty category [S1, S7].
Never combine two payout
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
per output. The proposed BCH excess rule gives beneficiaries **no individual
BCH entitlement** beyond the committed minimum: all inherited BCH must
remain among the exact allocation outputs, but an executor may distribute
that excess among those beneficiaries arbitrarily. Constructor policy
assigns it to the first positive output; this is not covenant enforcement.
This keeps children independent of donations and local dust increases.
The owner must approve that policy or require a new deterministic BCH
allocation rule. For example, an input with 1,000,000 satoshis and two
positive P2PKH allocations could assign 999,325 and 675 satoshis while
external inputs fund the fee [S3; arithmetic]. That redirects inherited
excess without violating this proposed policy. A funder's complete
`SIGHASH_ALL` signatures still protect the particular outputs it authorizes.
Token amounts, beneficiaries and locks remain exact.

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
its own tag. Its proposed fast root is P2SH20, pending the owner's decision;
its children retain P2SH32. The lack
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

Each **confirmed** oracle move adds at least 8,000 satoshis to the vault,
plus fees paid by its funder [P1]. An unconfirmed announcement can disrupt
speculative mining without that guaranteed deposit: oracle `O` spends
baton `V` and funding input `F`, but a conflicting spend of `F` alone can
confirm elsewhere, invalidating `O` while leaving `V` unspent. No local
BCHN replacement of `O` is required. Such disruption can cost only the
conflicting transaction's fee, with no 8,000-satoshi vault deposit. Do not
price every announcement as a confirmed oracle move.

An oracle move preserves commitment/target but resets the age origin to
the new output's confirmation context (or age 0 while unconfirmed).
Repeated confirmed moves can suppress age-based target easing [P1].
Follow validated transitions, apply section 3's credit rules, roll back
funding conflicts and rate-limit peer processing without ignoring real
chain changes. These risks affect solo and pool miners. The pool cannot
ban the oracle path or infer an attacker's budget from observed messages.

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
The effort rows sum to 20 to 40 engineer-weeks before that additional
work [table arithmetic]. Unmeasured covenant feasibility and unresolved
profile decisions prevent a reliable delivery estimate.

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
  metadata, the `R` constructor, age, amounts, fee/value policy, layout, and the
  contract, chain and vardiff targets. Include enough data for the client
  to rebuild the template and check its beneficiary vector. The miner
  supplies its own transient key, nonce and datasig before constructing
  final `R` and midstates; the server cannot send the final complete template.
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
bytes [P8]. Share mode must use the easier (numerically greater) of the
statistical target and contract target for the GPU comparison, retaining
the positive ScriptNum predicate. Age-based contract easing can make the
contract target easier than the chain or vardiff target after quiet
periods [P1]. Filter the returned candidates separately for statistical
work, chain credit and actual contract wins; forward every valid win.

T2 returns only `min(total_winners, winner_cap)` stored hits, discarding
the excess; `total_winners` is observable in its result [P9]. Share mode
must detect `total_winners > winner_cap` and rescan that exact batch at
the contract target, even if ordinary share hits are discarded. If the
contract-target rescan also overflows, subdivide and retry until every
contract winner is recovered. Keep the original job generation/data until
recovery completes and deduplicate rescans. No contract win may be lost
to an easier share filter or a full result buffer.

Pickaxe needs P2SH20 age layouts and profile-dependent conservation/burn
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
| P2SH20 chosen-pair collision | Complete seed binding limits a colliding pair to at most one settled reward from at most 65,536 tails. Section 4 gives a conditional cost model, not a deadline or proved economic lower bound. Auditor re-review and owner choice remain required; measure P2SH32 under both tail profiles. |
| Oracle griefing or ordinary solo claims | Confirmed oracle moves deposit at least 8,000 satoshis plus funder fees. Unconfirmed moves may disrupt and then lose to a funding-input conflict without any deposit. Oracle moves reset the age origin and suppress target easing. Follow authenticated paths and roll back dependencies. |
| Share spam and forged state | Size bounds, per-peer budgets, dependency deduplication, cheap checks before crypto, deterministic chain difficulty, and authenticated baton sources. Weak vardiff work stays local. |
| Selfish share mining, timestamp grinding, stale-parent replay | Greatest cumulative valid work, complete metadata/seed binding, unique proof credit and depth-bounded uncles. Authenticated context forbids reverting to a spent predecessor. Old private context does not prove creation time; delayed-sync and fork incentives still need simulation. |
| Historical baton after oracle spend | Authentic `V` is insufficient when selected context includes `O` spending it. Reject that share and any later old-baton uncle or includer bonus; filter PPLNS by paying lineage. Speculative conflicting contexts remain a risk until BCH resolves them. |
| A pool miner broadcasts a win to its own address | Changing output index 1 changes the checked HASH256. Reusing the same datasig does not preserve the PoW. A miner can mine a separate solo template, but that proof fails the pool's expected-`R` check and earns no pool credit. |
| Duplicate payout inputs or reused output obligations | Exactly one distribution input at index 0; all funding inputs token-free. Check every input and output, not just the active input. |
| Token burn, fake child lock or altered rounding | Distribution always conserves its actual input reward using exact sums and intervals. Claim-tail burning is a separate owner-selected profile with an exact bounded loss. Funding category must equal the empty vector; token amount zero cannot exclude NFTs. |
| Cached schedule carry or result-buffer overflow | Retain the conserved-tail carry guard or recompute block 8. Compare at the easier statistical/contract target, detect total hits above capacity, and rescan/subdivide at contract target until every win is recovered. |
| Work overflow or uncle rounding/replay | Checked 512-bit work and weights, bounded wider products, raw-work clipping before conserved 9/1 uncle accounting, and ancestry-wide proof deduplication. Existing saturating u128 PPLNS cannot be inherited. |
| Redirect inherited BCH excess | Proposed beneficiaries have token entitlements and minimum BCH only. Aggregate excess can be assigned among them arbitrarily; funder signatures bind actual outputs. Owner must approve or require deterministic BCH allocation. |
| Funding fee griefing | No inherited payout BCH can go to change or fees. Funders sign complete transactions and fund all fees. Never assume child-pays-for-parent will rescue a low-fee claim. No free public funding wallet. Competing funded spends may delay each other under local first-seen policy. |
| Nobody sponsors distribution | Tokens wait with no deadline. Settlement liveness depends on a willing funder and retained manifests. Small entitlements may not justify their BCH cost. |
| Missing payout data or eclipse | Require data before credit, replicate and export winning manifests, retain them beyond DAG pruning, use independent peers. Hashes alone cannot recover missing scripts. |
| Reorg or locktime malleability | Persist conflict sets by spent baton and proof, resolve actual txids, roll back dependent work and payout tracking. Never treat RPC acceptance as confirmation. |

## 10. Open decisions and phases

The owner must decide the following before a profile is approved:

- Share/PPLNS parameters, depth-based uncle policy, bootstrap beneficiary
  rule, authenticated-context lag policy, optional per-share fees, sponsor
  budget and who is expected to fund small payouts.
- Conserved tail or bounded tail burning. Burning trades token supply
  for a possible SHA-bound candidate-rate gain in solo and pool mining;
  it is not assumed merely because BCHN permits implicit burns.
- P2SH20 with the complete-binding/cost argument after auditor re-review,
  or P2SH32 after measurements under both tail profiles.
- Initial age range through 32,767 BCH blocks, or separately validated
  layouts/vectors through 65,534 BCH blocks. The latter still fits ten
  first-pass blocks with P2SH20.
- The proposed aggregate-only BCH excess rule, or a deterministic
  individual BCH allocation enforced by a revised covenant.

The initial tree/state/funding/arithmetic caps in section 5 are fixed
prototype bounds; changes require explicit profile revision and new
measurements. No sponsor reimbursement from PHOTON is assumed.

Review and implementation gates, in order:

1. **Design review only.** Pickaxe reviews actual template and kernel
   implications under both tail profiles. The independent auditor re-reviews
   complete proof binding, collision cost and winning-seed prefiltering,
   private forks, context eligibility, freshness/sync rules, no-NFT state,
   funding isolation, BCH excess and child-script construction. Confirm
   the frozen bounds, zero allocations and exact integer window policy.
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
covenant; identical scripts for all miners on one tip; datasig circularity;
and a proved global 60-second collision window. The v3.2 oracle deposit
rule is confirmed for confirmed moves, not every speculative disruption.
Actual locktime malleability is
confirmed by the reconstruction, subject to BCH finality. No current
network-rate claim or live-chain compatibility is established by this doc.

## Review history

- **2026-10-29 Pickaxe review**, against pickaxe-miner dev `640e9d2`:
  refined conserved-tail cost to about 3.8 compression equivalents;
  added the burning decision, BCHN burn-policy source check, age-layout
  distinction, miner-owned transient key/midstate, and lossless contract-win
  comparison/overflow recovery. Depth plus cumulative work replaces the
  clock premise. The claimed collision economics remain conditional and
  need independent re-review; an unmeasured kernel model is not throughput.
- **2026-10-29 independent review**, against `85a8afec`: addressed all
  nine findings with complete seed/datasig binding, authenticated baton
  contexts, frozen execution bounds/VM budgets, widened work and conserved
  clipping, confirmed versus speculative oracle cost, explicit BCH excess
  policy, cached-block carry guard and the initial age cap. Funding inputs
  must have exactly empty category introspection. The 60-second argument
  is withdrawn. Its 575-bit product and 44-iteration bucket estimates are
  confirmed by the bounded encoding; full covenant feasibility is unmeasured.

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
