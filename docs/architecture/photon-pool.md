# Proposal: a decentralized pool for PHOTON GPU miners

**Status: owner-approved minimum-payout design study, 2026-10-30.**
The main payout path is the direct, minimum-payout split in section 5.
The funded Merkle-sum tree and its complete phase 2a measurements are
retained there as a historical alternative, no longer the main path.
Parameter choices below are proposed PHOTON share-chain rules.
They change neither BCH consensus nor PHOTON v3.2. The independent
re-review of revision `4ecec7aa` clears only an isolated, test-only
payout-covenant prototype. Share-chain and node work stay blocked until
its findings F1 and F2 are fixed and re-reviewed (section 10). The owner
authorized isolated offline covenant experiments and a Rust payout
simulation for this study; no node integration, network calls or spending
are authorized. The earlier tree's measured results remain in section 5.

## 1. Goal and scope

Build a sister program in this Rust workspace. PHOTON miners contribute
verifiable work to a separate peer-to-peer share chain. Each winning claim
locks its reward to a direct payout commitment selected from the preceding
share-chain ledger. Won tokens credit that ledger by the PPLNS window;
unpaid balances carry between wins. Anyone can front the split's BCH and
receive the committed token reimbursement. No operator holds the reward key
or a private withdrawal ledger.
Pickaxe is one client. The protocol must permit independent clients.

The owner's decisions of **2026-10-30** distinguish two share-chain
families. The BCH share chain continues mining BCH, with tokens merge-mined
on that BCH work, SAFA style. Standalone token share chains serve tokens
with their own proof of work, PHOTON v3.2 style. One program is configured
per token; miners choose which chain to join. This study changes neither
the BCH P2Pool program nor its payout rules.

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
`src/reward.rs`. Additional transaction, key-rotation, signing and
tail-loop sources resolve the layout and hashing questions [P1-P10].
Links P1 to P10 pin revision `2f7cbb0493f2a46ef89d4f04c941074311a09ba0`,
which this document inspected. The Pickaxe review checked dev
`640e9d24b04f5cc2ce72bedc871f3e3f28422a92`. The template, redeem hex,
protocol constants, signer and CUDA tail loop are unchanged between those
revisions. Serialization, T2 and search code differ; the key rotation and
winner count cited as P9 are present at both [git diff of the two
revisions]. This is source verification, not a fresh query of mainnet or
chipnet UTXOs.

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
| Work and ownership | Share target, 32 bytes; miner's token payout locking bytecode with length; minimum multiple (at least 10, default 20); authenticated per-share fee policy, if approved |
| Baton anchor | Actual spent txid, 32 bytes; output index, 4 bytes; source value in satoshis, 8 bytes; source token amount in raw units, 8 bytes; source commitment, 100 bytes; source confirmation height in BCH blocks, 4 bytes, `0xffffffff` while unconfirmed; context BCH height, 4 bytes; vault-lineage digest, 32 bytes; transition count, 8 bytes; bounded path delta from the parent |
| Distribution | Canonical parent-window weights commitment, total integer weight, ledger/reservation digest, voted C, payout cap/profile and covenant version; direct payout list in R |
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
The first job needs no separate bootstrap rule. Under the payout rules the
owner adopted on 2026-10-30 (section 5), a win the earned balances cannot
cover is paid ahead, never beyond the expected credit of shares already in
the window. At launch every balance is empty, so the first list consists
only of such advances; no tokens are held in custody. The former proposal,
paying the finding miner alone, is not part of this design. The genesis
domain must still be visible to every miner.

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
   and the required share target. Reject the share if any referenced uncle
   is ineligible. Rebuild the prior-window weights.
3. Authenticate the spent baton against a validated BCH source transaction
   and its ancestry. Check lock, category, value, mutable capability,
   commitment and token amount. Apply the lineage/context eligibility rule
   below; an authentic historical source or a peer announcement is insufficient.
4. Check the claimed age against the source's committed confirmation
   height and the committed context height, then derive the PHOTON target.
   The pool profile sets sequence equal to the claimed age. Validate old
   shares against their committed historical context, not the current
   tip's age. For unconfirmed chaining, validate the entire predecessor
   claim path and use age 0 BCH blocks.
5. Verify the datasig, bind the complete proof seed in `R` as in section 4,
   then reconstruct the exact claim using the prescribed tail and expected
   reward lock. Check amount encoding, exact BCH values/fee, dust, and
   the selected tail profile's conservation or exact-burn rule. Check the
   signed positive target predicate. Verify one
   datasig and one HASH256. Reject altered templates.
6. If the hash meets the share target and its claim digest is not already
   credited on the selected ancestry, credit one chain share. If it also
   meets the contract target, immediately broadcast the claim. An eligible
   contract win must be forwarded even if it misses a harder share target.
   Record it separately; it does not acquire unearned share-chain credit.

Do not require every easier-target share to pass the contract's winning
PoW test. Validate the other claim rules with a proof-aware verifier.
For an actual win, use the unmodified covenant in the BCH 2026 VM and
BCHN transaction acceptance. These are distinct checks.

### Authenticated baton context and eligibility

This proposed rule starts at a designated deployment vault outpoint,
not any output with the same category and lock. Each share commits a vault
lineage: the ordered claim and oracle transitions from that outpoint to its
selected baton. Authenticate block inclusion, source data and intervening
spends using a validated BCH chain/UTXO view; a header or transaction
Merkle proof alone does not prove unspentness.

The lineage digest is a hash chain. It starts as SHA256 of a lineage domain
tag and the deployment outpoint. Each transition replaces it with SHA256 of
the tag, the previous digest and the new vault outpoint (36 bytes). The
share also commits the transition count, so lineages compare by length
and digest. Confirmation heights are not in the digest, so a transaction
that confirms, or returns to the mempool, keeps the same lineage.

Carry the path incrementally, as a delta from the parent [re-review F5].
For each transition after the parent's lineage tip, in order, the delta
holds the new vault outpoint and its confirmation height (40 bytes), or,
for an unconfirmed claim, the full transaction. Nodes check confirmed
entries against their own validated BCH view; the entries make that check
a lookup. Proposed bound: at most 256 confirmed entries and at most 8
unconfirmed claims of at most 1,000 bytes each, above every claim layout
in section 2, so at most 18,240 bytes of path evidence per share
[proposed profile; arithmetic]. A parent more than 256 transitions behind,
about 43 hours of claims at one per 600 seconds, cannot be extended.
Restarting after such an outage needs the outage-restart rule in
section 10.
A cold peer recomputes the lineage from the deployment outpoint with its
own BCH view and fetches carried claims.

A child's lineage must extend its parent's, and its context height must be
at least its parent's. Spent-baton lag is zero: in the committed context the
source must be the lineage's current unspent tip. A context must include
every transition confirmed at or below its context height, including oracle
moves. After those it may hold only single-input claims not confirmed at
or below that height (below). The claimed age must satisfy
`age <= context_height + 1 - source_confirmation_height`, in BCH blocks:
the claim could enter the next block [P1; proposed profile]. A context
height above the validating node's best
validated BCH height waits, within the peer queue bound, until that node
catches up. New live jobs and credited tip selection use the latest fully
validated BCH height; missing context pauses them. Historical shares are
checked against their own committed contexts, never today's age or local
first-seen time. Reorganization selects/rebuilds a compatible share branch
rather than rewriting old proofs [proposed profile; compare B8's
payout-validity rule].

Share compatibility is defined on vault lineage and anchor heights, not on
an exact BCH block hash [re-review F5]. Two contexts are compatible when
one lineage extends the other and each share's committed source
confirmation height matches the validating node's BCH view. A share
claiming age 0 BCH blocks needs only its source to be the lineage tip,
confirmed or not. Under the previous revision, which committed and
extended the exact block hash, every BCH orphan or brief tip disagreement
would fork the share chain. The losing side would roll back about 60
honest shares (600 seconds of BCH block interval at the 10-second share
interval) even when the vault lineage is unchanged [arithmetic]. Lineage
and heights are the only BCH data that PHOTON validation reads: which
baton is current and how much age it has. An orphan that leaves both
unchanged now moves no credit. One that changes them still rolls back the
dependent shares, as it must.

### Unconfirmed transitions

Speculative selection is limited to single-input claim successors
[re-review F1]. A node may select an unconfirmed transition only if it is
a claim with exactly one input, the vault, and every earlier unconfirmed
transition on the selected path is also such a claim. Validate each one's
token rules, finality and complete predecessor claim path. Use claimed age
0 BCH blocks for an unconfirmed source [P1]. No funding conflict can
invalidate such a claim: it spends nothing but the vault. Only a
conflicting claim can displace it, and that needs a real contract win,
not a fee.

Never select, mine on, or cancel jobs because of an unconfirmed oracle
transition, or any other unconfirmed transition with an input other than
the vault. Keep mining the current baton `V` until such a transition `O`
confirms; it then enters the lineage through the height rule above. A pool
contract win on `V` found while `O` is pending can lose to `O` at
confirmation. That risks the wins of about one BCH block interval, not
share credit. Price oracle disruption at the confirmed deposit: at least
8,000 satoshis plus fees (section 6).

The counterexample behind this rule comes from the re-review of
`4ecec7aa`. That revision let nodes cancel `V` jobs on any unconfirmed
oracle spend and select the spend speculatively:

1. The attacker broadcasts oracle `O`, spending `V` and a funding input
   `F`, and gets a conflicting spend of `F` mined. This needs no
   8,000-satoshi deposit.
2. Honest nodes stop mining `V`, mining `O`'s output speculatively or
   pausing, while the attacker privately mines `V` shares.
3. The `F` conflict confirms. `O` becomes invalid, and every share that
   selected it is dead.
4. The only compatible branch with work is the attacker's `V` branch, which
   becomes the mainline. Honest work is beyond the 6-share uncle depth, and
   cannot be an uncle anyway: uncles need the includer's baton.

The attacker then takes all window credit for up to about one BCH block,
about 60 shares, per success. It pays one conflicting transaction's fee,
or 8,000 satoshis if `O` confirms, and can repeat this every BCH block.
Ignoring unconfirmed oracle transitions removes step 2.

Distinct conflicting paths remain distinct; local mempool first-seen order
is not a share-chain rule. Once a parent selects a claim successor,
descendants cannot revert to its spent predecessor. Confirmation selects
the actual BCH path. A conflicting confirmed claim or a BCH reorg rolls
back the dependent speculative branch and jobs, then recomputes live
credit on a compatible branch. Historical storage does not imply current
credit or successful settlement.

### Credit eligibility

For a paying share, mainline PPLNS work must have been eligible in its
own context and be compatible with the paying share's vault lineage and
anchor heights. Pre-transition work can remain in the window. An uncle
must additionally use the includer's selected baton and compatible context.
A share that references an uncle failing these checks is invalid; the
uncle is not accepted at zero weight [re-review F5]. No uncle may introduce
old-baton work after a selected transition. Apply these checks before
assigning any credit, as well as at share acceptance.

If confirmed oracle transaction `O` spends `V`, a share whose context
contains `O` cannot mine against `V`: its source is not the lineage tip.
Neither a new timestamp nor authentic ancestry of `V` repairs that. An
unconfirmed `O` creates no competing context, because nodes ignore it.
A private old-context proof does not reveal its wall-clock creation time.
Depth rules and compatible-context payout filtering bound its credit
opportunity; they do not prove a deadline. Model these cases, including
the counterexample above, before profile activation.

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

Use a fixed work budget for the PPLNS window: 17,280 times bootstrap-share
work, nominally 48 hours at the proposed 10-second interval. The owner
chose 48 hours on 2026-10-30, after the section 5 simulation: a 12-hour
window (4,320 shares) left too little expected credit to pay ahead for
10,000 miners, and wins could not be listed. It is not a rolling promise
of 48 hours after hashrate changes. Clip the oldest included work to the
remaining budget. Aggregate by payout locking bytecode. No BCH coinbase
maturity skip, traded payout rights, BCH-difficulty multiplier or
BCH-denominated dust merging applies. An alternative window tied to PHOTON
expected claim work needs explicit pool-hopping analysis before selection.

Propose at most 2 uncle references per share and a 6-share ancestry bound.
These are depth rules, not v3.2 limits or seconds [proposed profile]. Use
greatest cumulative valid mainline work, breaking ties by ascending raw
share identifier. Keep the existing ancestor-off-chain uncle meaning [B2].
An uncle already canonical or already included anywhere on the selected
ancestry cannot earn credit again. Deduplicate by the reconstructed claim
digest: the HASH256 of the claim reconstruction with locktime 0, which
includes `HASH160(R)` in output 1 [P1; re-review F6]. That digest already
covers actual locktime variants. Do not key on the proof seed and tail
alone: the same seed and tail legitimately recur under a different `R`
after a timestamp or parent change, and each is a fresh PoW trial. Keep
a reorg-aware index so moving outside the window cannot renew credit.

Window arithmetic is exact and ordered. Walk mainline entries newest
first; consume each entry's own raw work, then its eligible-position
uncles in ascending claim digest. For each unique work item set
`clipped = min(work, remaining_budget)` and subtract it from the raw
budget. Own work earns `10 * clipped` weight. An eligible uncle earns
`9 * clipped` for its beneficiary and `clipped` for the eligible includer.
Thus each paid item conserves `10 * clipped` integer weight; never round
90 percent and 10 percent independently [B2; proposed clipping order].
The boundary is ordered, not proportional. Context-ineligible mainline
items consume their raw budget position but receive zero weight; do not
walk farther to replace them. Ineligible uncles never reach this walk:
their includer is invalid (above). Check positional validity and
deduplication before this walk.

The walk has no cap on distinct payout scripts. The historical funded tree
needed one at its 4,096 leaves [re-review F2]; the adopted direct split
credits a balance ledger instead, so every item in the budget earns its
weight. The owner removed the cap on 2026-10-30. Golden vectors must cover
ties and uncles.

Aggregate the resulting weights by payout script and drop scripts whose
total is zero; only positive weights earn credit.

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

`R` is the full payout redeem bytecode. With the P2SH20 candidate profile,
the claim's output index 1 is exactly
`OP_HASH160 <HASH160(R)> OP_EQUAL`; P2SH32 remains an open owner choice.
Nodes rebuild `R`; they never accept
a miner-supplied script hash without its expected preimage.

The canonical constructor contains these immutable values:

- A PHOTON-pool domain and covenant version, BCH network, token category
  and deployment lock hash.
- The expected parent share identifier, spent baton outpoint,
  `metadata_hash`, canonical parent-window weights and ledger state digest.
- For the main path: the direct ordered payee scripts, gross owed amounts,
  each miner's minimum multiple, `C`, cap/profile version, the reviewed
  program identifier and exact output/funding rules from section 5.
  The digest binds the complete preceding share-chain ledger, including
  outstanding reservations; BCH script execution does not authenticate
  that ledger. The historical tree instead binds its Merkle-sum root,
  total weight, allocation order, tree shape and depth bounds.
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

Pickaxe has no RIPEMD-160 on the GPU today; it exists only on the host,
in wallet and retired-v0 reward code [Pickaxe review of 5373473b]. Seed
binding therefore needs either a new SHA-256-of-`R` plus RIPEMD-160 seed
path in the Rust, CUDA, HIP and wgpu engines, or a GPU-to-host-to-GPU round
trip per batch. At about 1.6 GH/s per device, the host path is about
24,000 HASH160 of `R` per second (about 48,000 under tail burning): cheap
for a CPU, but an extra pipeline stage before each tail window. Measure
both options; the tail cost itself is unchanged.

On the main path, `R` directly commits ordered gross owed amounts rather
than a Merkle tree. It reads the actual reward and derives each gross
payment as `min(committed_owed, remaining_reward)`. It must consume that
reward exactly while meeting every paid miner's minimum. Tail-dependent
clipping can make a particular commitment unspendable; section 5 records
the coverage and final-minimum conditions. Different miners on one parent
generally have different `R` because their proof seed and metadata differ.
The historical tree below commits weights rather than final token amounts.

The security discussion that follows was reviewed for the historical tree.
Its binding requirements continue to apply, but its script lengths and
offset-dependent cost margins are not measurements of the lean program.
The final lean `R` needs its own byte-exact review. References below to a
bootstrap paying the finder describe the earlier proposal; the adopted
rules pay the first win ahead against expected credit instead (section 5).

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

Each found pair is worth at most one PHOTON reward, and only if its honest
side also contains a contract-winning tail. Let `p` be the win probability
of one honest `(R, seed)` candidate over its bounded tail range:
`p = 1 - (1 - (contract_target - 1) / 2^256)^candidate_count`, about
`candidate_count * (contract_target - 1) / 2^256` when small, under
independent digests. This is per seed, not per accepted share.

**Use the square-root estimate, not `2^80 / p`.** A single large generic
search yields many collisions, and the attacker needs only one whose honest
side wins. The cheap per-element variable is the attacker's own metadata,
for example its payout script, with the seed fixed. Each change gives a new
`R` and HASH160, hence a fresh trial of the tail range. With `n1` honest and
`n2` malicious candidates, success needs about `n1 * n2 * p = 2^160`. Total
HASH160 work is minimized near `2 * 2^80 / sqrt(p)`. Testing the tails of
the roughly `1 / p` collided honest seeds adds about one PHOTON win's work
[Pickaxe review of 5373473b; S2].

**This is an attainable estimate, not a conservative bound** [re-review F3].
The previous revision called it conservative because bounded-memory
golden-collision search costs more. That does not apply. A golden-collision
search needs one particular collision; here any cross collision whose
honest side wins will do, so the attacker needs about `1 / p` of them.
Parallel collision search finds `m` collisions in a range of `n` values
with about `sqrt(n * m)` evaluations and memory for about `m`
distinguished points [S8]. About half of all collisions pair an honest and
a malicious script. With `m` about `2 / p`, about `3 * 10^8` points or
roughly 10 GB at `p = 6.5 * 10^-9`, it reaches about
`2^80 / sqrt(p)` HASH160 evaluations with modest memory [re-review F3;
arithmetic].

**Price `p` at the attacker's best case.** Age easing can raise the
contract target toward `2^255 - 1`, so `p` can approach 1 [P1; section 2].
The floor is then about `2 * 2^80`, or `2.4 * 10^24` HASH160 evaluations
per stolen reward. That is about `1.5 * 10^7` GPU-years at the assumed
`5 * 10^9` HASH160 evaluations per second per GPU, or about 40 minutes for
a hypothetical ASIC fleet computing `10^21` HASH160 evaluations per second
[arithmetic]. At `p = 6.5 * 10^-9` (one PHOTON win per about `10^13`
candidates, 65,536 tails per seed), the same fleet needs about 1 year and
GPUs about `1.9 * 10^11` GPU-years, against `1.2 * 10^15` GPU-years under
the superseded `2^80 / p` model. No HASH160 ASIC fleet is known to this
document; the `10^21` rate is a stress assumption, not a measurement.

Weigh the `p -> 1` floor against the value of one PHOTON reward: at most
`floor(balance / 420,000)` raw units, for example about
`4.97 * 10^9` raw units in section 2's carry example [P1; arithmetic].
This document has no market price for PHOTON. The owner must compare that
value with about `1.5 * 10^7` GPU-years, and with the stress fleet's
40 minutes.

Two margins are not counted above. First, the honest `R` keeps its
variable 32-byte proof-seed/context digest at `R` offsets 43 to 74
(section 5 state layout), starting in the first SHA-256 block. No midstate
can be shared between honest candidates. Each honest HASH160 of a
4,435-byte `R` costs about 70 SHA-256 compressions, against about 2 for a
short malicious script. At the unequal-cost optimum the attack then costs
roughly `sqrt(70 * 2) / 2`, about 6, times more compression work than
equal-cost HASH160 counting [re-review F3; arithmetic]. Keep the digest
early in `R`: moving it to the end would allow midstate reuse and lose this
margin. Second, the search must finish within one job or baton lifetime:
the honest `R` binds the parent share and the spent baton, neither known
in advance. When `p` approaches 1 the contract target is so easy that the
baton moves quickly. The cost figures rely on neither margin, and P2SH20
security still uses no wall-clock window.

**Winner prefiltering cannot happen in the dangerous order.** Output 1
is `OP_HASH160 <HASH160(R)> OP_EQUAL`, and those 20 bytes are inside the
hashed claim (block 9, offsets 600 to 622). A tail can be tested only after
`R`, and so its HASH160, is fixed. If the attacker first finds a winning
`(seed, R1)`, a malicious `R2` must match an already fixed hash: a second
preimage. Prefiltering to a set `W` of known winners turns this into a
multi-target second preimage costing about `2^160 / |W|`. Each element of
`W` costs a real PHOTON win that the attacker could simply keep solo, so
`|W|` stays tiny and the cost exceeds `2^120` [Pickaxe review; serialization
in section 2].

These figures are estimates, not a proved attack lower bound. Binding
removes reuse across seeds; it does not control the attacker's own search
strategy. The re-review's check of other strategies is summarized below.

A precomputed malicious table of `2^50` entries leaves order `2^110`
online trials to hit it in a 160-bit range under the random-function
model. Each hit is a collision whose honest side wins with probability
`p`, so the cost per stolen reward is about `2^110 / p` trials
[re-review F3]. Grinding a
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
availability. P2SH32 remains the fallback. Its estimated kernel costs are
about 3.8 to 4.8 compressions for conserved tails and 1.9 to 2.9 for
burning tails (section 2); actual costs under both profiles must be
measured before the owner chooses. No P2SH20 deployment approval or
measured P2SH32 result is supplied by this document.

### Re-review verdict on P2SH20

The independent re-review of `4ecec7aa` found the binding sound. The
covenant hashes only its reconstruction. It copies version, outpoint,
unlocking bytecode, sequence, values, categories, commitment, amounts and
output-1 bytecode from the transaction; bitfields, commitment length and
locktime 0 are constants; the proof is compared with the next target
[P1, lock script lines 128 to 176]. So one `R` admits exactly one seed with
at most 65,536 tails. The re-review found no cheaper generic route than
section 4's estimate:

- Honest-side multicollisions on one seed share one hash, so they add no
  independent trials.
- A lookup table of `M` entries costs about `2^160 / (M * p)` per reward.
- Honest scripts differ in a free 32-byte digest, so their fixed
  structure offers no known shortcut.
- RIPEMD-160's input is a SHA-256 digest, so differential RIPEMD-160
  attacks do not transfer.
- Uncles and forks add honest candidates, not more `p` per `R`.
- A bootstrap `R` pays the finding miner alone; there is nothing to steal.

Its verdict: **acceptable with changes for an audit-gated P2SH20
profile**, under five conditions:

1. The section 4 cost text is corrected as above (F3).
2. `R` is byte-exact and canonical, with golden vectors for `R` and
   `HASH160(R)`. Nodes reject any non-canonical `R`.
3. Credit is deduplicated by claim digest (section 3, F6).
4. The profile is versioned and carries a tripwire. Any on-chain spend of a
   pool root by a redeem script other than its canonical `R` is a public
   HASH160 collision. Once such a confirmed spend is in a share's context,
   that share and its descendants must use P2SH32 roots. The tripwire does
   not prevent the first theft; it stops new P2SH20 roots once one is public.
5. A measured P2SH32 fallback is kept ready.

The re-review does not require P2SH32. It estimates that P2SH32 would cost
about 21 to 34 percent of candidate rate, section 2's conserved and burning
kernel models, and in its view that would remove the pool's
competitiveness. Those percentages are unmeasured models. The auditor's
sign-off on the final `R` bytes and the owner's choice remain open.

## 5. Minimum payouts with a lean direct split

### The owner's design, in plain words

These are the owner's decisions of **2026-10-30**, not independently chosen
implementation defaults. The owner rejected making the funded payout tree
the main path: the measured 4,096-leaf win needs 0.06985170 BCH externally,
including 4,098,190 satoshis in fees, about 1,001 fee satoshis per payee.
That can cost more than the small token payments are worth. Paying whole
wins to miners in turn was also rejected: a very small miner could wait
years. The tree, its code and its measurements remain available unchanged.

Instead, each miner accumulates a token balance on the PHOTON share chain.
Only actual won tokens create credit, allocated by the same PPLNS window
described above. A balance carries forward between wins. Miners choose how
large a payment must become before it is worth settling.

One lean transaction splits each funded win. On **2026-10-30**, after the
simulation below, the owner adopted these payout rules for the PHOTON pool
only; BCH P2Pool's own payouts and share chain are unchanged:

1. **First come, first served.** A miner joins the queue when their earned
   balance first reaches their own minimum, and the queue is paid in that
   order. A miner paid in full rejoins at the back once they have earned
   their minimum again; one whose payment was clipped keeps their place.
2. **The rest is paid ahead, within a bound.** If the queue does not use
   the whole win, the rest goes to the miners whose shares already in the
   window are expected to earn the most. Nobody is paid ahead of more than
   the expected PPLNS credit of the shares they have already found, so
   those same shares repay the advance. No tokens are held in custody,
   and the first win at launch is paid the same way.
3. **No small leftovers.** If what remains after an entry would be too
   small to pay the next miner's minimum, the entry before absorbs it: a
   small extra advance below one further minimum plus the tail span.
4. **No script cap, and a 48-hour window.** Every share in the window earns
   credit (section 3).

Every payment still meets its miner's minimum, the split pays out its
entire reward, and there is no token change output or donated remainder.
A list that cannot meet these conditions within `N` payees cannot be
mined; the simulation found none with a 48-hour window and `N = 187`.

`C` is one payout's cost expressed in raw token units, a single share-chain
parameter. Each miner records a multiple with their payout script in their
shares: at least 10, with 20 as the default. Their minimum gross payment is
`multiple * C`. The submitter fronts the BCH fee and output dust, receives
`C` tokens from each payment, and sends the rest to its miner. Thus a miner
paid exactly their minimum gives up `1 / multiple` of that gross token
payment; larger payments lose less. Each miner keeps the BCH dust on their
output. If nobody funds a split, the tokens stay locked and the miner's
unpaid balance is retained.

The owner requires that payouts never cost more BCH than the tokens are
worth, with no sponsor. This is an execution condition, not a price oracle:
an independent submitter chooses whether the token reimbursement covers
its actual BCH outlay. `C` can be changed only through the share chain's
upgrade vote. No automatic price update is part of this design. An on-chain
Cauldron price averaged over about a week is a possible later option only.
Without a price source the protocol cannot prove that token reimbursement
is worth the BCH spent; no willing submitter means no settlement guarantee.

### Exact ledger and selection rules

1. Identify a miner by their authenticated payout script, aggregating that
   script's window credit. Bind their multiple in their shares as with the
   script. An omitted multiple means 20; values below 10 are invalid.
   Multiple changes and conflicting multiples for one aggregated script
   need a deterministic, authenticated activation rule before a profile is
   frozen. A third party naming another miner's public script must not
   acquire the right to change that miner's minimum. No latest/min/max
   rule is selected here; changes must not rewrite an already locked `R`.
2. Credit only the actual reward of a settled pool claim on the selected
   BCH history. Use the parent PPLNS weights and exact telescoping integer
   allocation: `floor(reward * upper / total) - floor(reward * lower / total)`.
   This is accounting arithmetic, not a payout tree. A tail burn, if later
   approved, is never credited. A conflicting lost claim creates no credit.
3. Maintain signed balances (won credit minus confirmed gross debits; a
   balance is negative while paid ahead), reserved amounts and confirmed
   gross debits separately. A split consumes gross balance; its payee
   receives `gross - C`, and the submitter receives `C`. A payment that is
   merely proposed or unfunded is not a confirmed debit. The backing
   identity is `total won = confirmed gross debits + sum of signed
   balances`, with reorg-aware rollback and exact integer arithmetic.
   Available balance is the signed balance minus reservations.
4. Take the parent ledger snapshot before proof-seed/tail search. A miner is
   queued from the first ledger update at which their available balance
   reaches `multiple * C`; ties at one update go by ascending raw locking
   bytecode. Leave the queue when the available balance drops below the
   minimum: a miner paid in full, then credited back above it, requeues at
   the back. A queued entry's cap is its available balance. Every other
   miner may be offered at cap `available + E`, where `E` is the expected
   credit of its shares in the parent window, if that reaches its minimum;
   offers go largest cap first, ties by bytecode. For a share at position
   `j` (oldest 0) of a window holding `L` of `W` shares,
   `E = q * reward * (H(W-1) - H(L-1) + (j+1)/W)`, where `q` is the chance
   a share is also a claim and `H` the harmonic number. A deployed profile
   needs this in exact integers over the work budget; the study used
   floating point. Take as many queued entries, in order, as still let at
   most `N` entries cover the largest possible reward, then offers. Walk
   the list: an entry is followed by another only if the smallest possible
   reward still leaves the next one its minimum; otherwise that entry's
   cap becomes whatever covers the largest reward, and the list ends. Bind
   the scripts, caps and multiples directly in `R`, in that order. If no
   list fits, no job can be built. Lean version 2 pays in `R`'s order and
   checks no ordering itself; the share chain validates the order.
5. Let actual input tokens be `A`, `remaining = A`, and `paid_count = 0`.
   Visit committed entries in order while `remaining > 0`. Set
   `gross = min(committed_cap, remaining)`. Require
   `gross >= multiple * C`, subtract `gross` from `remaining`, and emit
   exactly `gross - C` tokens to that entry's exact script. Since
   `multiple >= 10` and `C > 0`, every emitted amount is positive.
   Increment `paid_count`. Reject unless `remaining == 0` and
   `1 <= paid_count <= N`. Unused committed suffix entries receive no output.
6. Emit exactly one submitter token output after the payees, with
   `paid_count * C` tokens in the same category, then optionally one
   token-free BCH change output. The submitter chooses its own standard
   payout script. The payee outputs plus submitter tokens equal `A` exactly.
   No other token output, burn, NFT, category change or token-bearing funding
   is permitted on an authentic fungible-only payout path.
7. Use input 0 as the sole payout input and one through four token-free
   BCH funding inputs for the measured prototype. Require version 2,
   locktime 0 and final sequences on all inputs. Bind output order, script,
   token category and exact amounts. Each token output receives at least
   the profile's dust minimum. Keep inherited payout BCH among the token
   outputs; outside funding pays the fee and missing dust. The submitter's
   BCH change carries no tokens. Funding signatures bind the actual complete
   transaction. The covenant has no privileged executor or owner key.
8. Reserve every immutable live win's gross allocations before selecting
   another win's list; competing sibling jobs reserve only on the selected
   settled branch. The selected baton lineage's accepted wins must be
   reflected even if a share-parent ledger snapshot is older. Retain
   winning `R`, source outpoints and allocation data
   beyond normal share pruning. Funding refusal retains the reservation and
   debt. Confirmation applies its debit once; reorgs undo dependent credits,
   reservations and debits without relabeling old locked payout outputs.
   Exact durable replay/finality rules are an integration gate, not code
   supplied by this offline study.

### Coverage conditions and open contradictions

For a fixed pre-win list with caps `cap_i`, define prefix sum
`S_j = sum(cap_i for i < j)`. A reward `A` is distributable exactly when
some prefix ends at entry `j <= N`, `S_j < A <= S_j + cap_j`, and its last
gross payment `A - S_j >= multiple_j * C`; each preceding full payment
must also meet its minimum. Merely having a list sum at least `A` is
insufficient: a small last remainder violates the minimum. The adopted
list walk (rule 4) builds every list so that each tail is distributable.

- **Launch.** Resolved by the adopted rules: the first win is paid ahead
  against expected credit, with no custody. The former finder-only rule is
  not part of this design.
- **Immutable list and tail gaps.** The supported tail changes `A` by up to
  65,535 raw units. Resolved by the leftover rule, which leaves the last
  entry its minimum at the smallest reward. Later earnings, a new `C`, or a
  new multiple still cannot repair an old `R`.
- **Cap and coverage.** If the queue and bounded offers cannot cover a
  reward within `N` entries, no job can be built for that win. With a
  12-hour window this blocked many wins at 10,000 miners; with the adopted
  48-hour window and `N = 187` the simulation found none. A smaller `N`, a
  shorter window or a much larger population can bring it back.
- **Advances.** Paid-ahead balances are recovered from the credit of shares
  already found; pool luck can leave a small shortfall. A miner who leaves
  while paid ahead takes the unrecovered amount with them: at most 0.04
  mean rewards across a whole 48-hour-window run.
- **Timing and reservations.** Current-win credit is unknown until the
  tail is found; it cannot be inserted into a list already hashed for that
  proof. Earlier won balances and immutable pending lists must not be
  promised twice. The aggregate backing identity alone does not guarantee
  that every inaccessible old root can ever pay its remaining debt.
- **Value and funding.** `C` is a token unit count, not proof of BCH value.
  Fee-only estimates omit payee dust and the submitter's token-output dust.
  The no-sponsor condition can stop payouts indefinitely when reimbursement
  is unattractive. Vote timing and stale committed `C` can extend that stop.
  A single `C` cannot exactly match every transaction's different amortized
  fee, amount encoding and script lengths; it is a voted reimbursement
  parameter. `C / gross` is the token deduction for fee plus dust, not a
  measured pure BCH transaction-fee fraction.

The isolated lean covenant below measures these rules, and the seeded
simulation after it tests their economics. Neither resolves the owner
decisions or approves a deployment.

### Lean direct split measurements

The additive `lean` module reuses the existing Rust assembler and VM harness.
The tree's source artifact, program bytes and tests are unchanged. Lean `R`
pushes an 82-byte header (version/reserved, network, category, context, C,
entry count), an altstack sentinel, then individual script/owed/multiple
records in reverse priority order and the fixed program. The context starts
at redeem byte 42, versus byte 43 in the historical tree. Exact field widths
and `ProofBinding` serialization are in the crate README. Canonical Rust
construction checks global beneficiary uniqueness; VM-local adjacent tie
ordering alone does not check duplicates at different debts. The caller must
also authenticate the ledger and all opaque non-tail seed bytes. This is
not a claim verifier or a share-chain serializer.

In the tables, parentheses identify beneficiary script length. Operation
cost pairs are standard / non-standard; hash pairs show usage / limit in
those same two modes. Sizes and fees are integers; fee/payee is rounded
only for display. These are offline libauth measurements, not BCHN relay.

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
standard profile here costs 126.723 sats/payee, the amount-compatible 209-payee
probe 123.349, and the all 35-byte benchmark at 100 payees 148.050. A no-overhead
asymptote is 111 bytes for 23-byte scripts (36 bytes in R plus a 75-byte output);
shared contract/funding/submitter/change overhead and operation density still
apply. BCH dust fronting is additional to this fee.

### Simulation findings: the earlier rules did not deliver short waits

`cargo photon-payout-sim` runs the earlier rules (most owed first, never
paid ahead, a 4,096-script window cap and launch custody) for 1,000 and
10,000 miners: one giant holding 70 percent, a log-uniform mix over eight
decades of hash rate, and a heavy-tailed mix, with multiples of 10, 20, 30
and 50, churn, delayed funding and `N = 64`. Every number is a scenario
assumption, not an owner decision. The compact tables are in
[`simulation-summary.md`](../../p2poolv2_photon_payout/simulation-summary.md)
and the full inputs and methods in
[`simulation-results.md`](../../p2poolv2_photon_payout/simulation-results.md).

In plain words: big and medium miners were paid regularly, but small and
smaller miners were not paid at all within the 5,000-win horizon, and most
wins paid only one to thirteen miners although up to 64 fit. The expected
wait `multiple * C / earning rate` did not hold: a fixed probe miner whose
formula predicts about 1.15 days waited 15 to 77 days, or was never paid.
It is not independent of the number of miners under these rules.

| Population | Miners | Payouts per won reward (after bootstrap) | Eligible miners beyond `N`, mean |
|---|---:|---:|---:|
| Giant 70 percent | 1,000 | 1.008 | 334 |
| Log-uniform | 1,000 | 1.924 | 398 |
| Heavy-tailed | 1,000 | 3.359 | 662 |
| Giant 70 percent | 10,000 | 3.732 | 2,615 |
| Log-uniform | 10,000 | 7.831 | 3,280 |
| Heavy-tailed | 10,000 | 13.010 | 6,719 |

The causes, in order of effect:

1. **Most-owed priority.** The list is sorted by owed amount, so large old
   debts always go first and use up each reward. A miner who has just
   reached their minimum waits behind every larger balance, which brings
   back the whole-win waiting the owner rejected.
2. **A permanent backlog.** A list is fixed before its win is found, so it
   can only pay debts that already existed. With no earlier won tokens the
   strict rules cannot pay the first win at all (the strict genesis probe
   rejects all 100). The conditional runs therefore assume 100 earlier
   rewards held under an unspecified bootstrap custody. Each later win adds
   one reward of credit and pays one reward out, so that 100-reward backlog
   never shrinks, and everyone queues behind it.
3. **Sparse shares.** The smallest miners seldom have a share inside the
   4,320-share window, and the 4,096-script window cap (kept from the tree)
   drops credit for the oldest distinct miners when more than 4,096 are
   present. With 10,000 miners that alone excludes many of them.

Candidate changes: pay in the order miners became eligible (first come,
first served) instead of most owed; remove the window's script cap, which
only the tree needed; avoid any launch custody by letting a win's
remainder go to the next miners in line, even ahead of what they have
earned (bounded, and recovered from their later credit), so no tokens wait
in custody and the backlog stays near zero; and size the share rate so a
target smallest miner has a share in most windows. Each changes a PHOTON
share-chain rule. The re-run below simulates them; the owner adopted the
first three on 2026-10-30, with a 48-hour window instead of a faster share
rate.

### Candidate rules re-run: first come, first served with bounded pay-ahead

On **2026-10-30** the owner asked for the four candidate changes above to be
simulated. After seeing the results the owner adopted them, with a 48-hour
window, for the PHOTON pool only (rules at the start of this section). In
plain words, the simulated rules are:

1. **First come, first served.** A miner joins the queue when their earned
   balance first reaches their own minimum (`multiple * C`). The queue is
   paid in that order, ties broken by script bytes. A miner paid in full
   rejoins at the back once they have earned their minimum again. A miner
   whose payment was clipped keeps their place for the rest.
2. **No tokens held at launch.** There is no launch history and nothing is
   held in custody; the first win pays at once. After the queue, the rest
   of each win is paid ahead to the miners whose shares already in the
   window are expected to earn the most. Nobody is paid ahead of more than
   the expected PPLNS credit of shares they have already found, so those
   same shares repay the advance. Its expected loss is zero; only pool luck
   makes the real credit differ. If what remains after an entry is too
   small to pay the next miner's minimum at the smallest possible reward,
   the entry before absorbs it: a small extra advance, below one further
   minimum plus the 65,535-unit tail span. If no list of at most `N`
   payees can cover the reward, the pool cannot build a job for that win,
   and the study counts it as blocked.
3. **No 4,096-script window cap.** Every share in the window earns credit.
4. **Share rate and window length.** The study compares 10-second and
   1-second pool shares with a 12-hour window, and a 48-hour window at
   10-second shares.

The expected credit of a share at position `j` (oldest 0) in a window holding
`L` of `W` shares is `q * reward * (H(W-1) - H(L-1) + (j+1)/W)`. Here `q` is
the chance that a pool share is also a win and `H` is the harmonic number. The
first term covers a window that is still filling, as at launch. The study
computes this in floating point; a deployed rule needs exact integer
arithmetic, for example from the ratio of the share and claim targets.

**Covenant change.** First come, first served cannot use the version 1
lean covenant, which checks that `R` lists debts most-owed first. That
check adds no security: a dishonest `R` could name anyone, and the share
chain must validate `R` against its ledger in any case. Version 2 (header
version 2) drops it and pays entries in the order `R` commits; every other
check is unchanged. Version 1's bytes, artifact and measurements are
unchanged. Version 2 is 818 program bytes instead of 885. Its largest
measured standard lists are 188 payees with 35-byte scripts, 238 with
25-byte P2PKH and 198 with 23-byte P2SH20 (versus 187, 99 and 83), and 244
in the densest mix (versus 207). The ordering check was most of the
operation cost for short scripts. The fee is 117 to 154 sats per payee for
lists of 64 or more. Full table:
[`lean-share-order-measurements.md`](../../p2poolv2_photon_payout/lean-share-order-measurements.md).

`cargo photon-payout-sim --candidate-summary` reproduces the tables in
[`candidate-summary.md`](../../p2poolv2_photon_payout/candidate-summary.md);
the per-period tables are in
[`candidate-results.md`](../../p2poolv2_photon_payout/candidate-results.md).
The seed, populations, multiples, churn, delayed funding, `C` and 5,000-win
horizon match the written-rules study. Costs serialize every list through
the version 2 builder. The fixed probe miner has 0.0001 of the original
pool's hash and multiple 20; its threshold wait `multiple * C / earning rate`
is about 1.15 days in every run.

| Miners | Mix | N | Share interval | Window | Blocked wins of 5,000 (by N alone) | Payouts per won reward | Probe interval mean / P95 days | Peak paid ahead, rewards |
|---:|---|---:|---:|---:|---:|---:|---:|---:|
| 1,000 | Giant 70 percent | 64 | 10 s | 12 h | 0 (0) | 44.338 | 1.162 / 3.492 | 1.805 |
| 1,000 | Log-uniform | 64 | 10 s | 12 h | 0 (0) | 63.941 | 0.987 / 3.026 | 2.311 |
| 1,000 | Heavy-tailed | 64 | 10 s | 12 h | 0 (0) | 63.933 | 1.123 / 3.492 | 3.438 |
| 10,000 | Giant 70 percent | 64 | 10 s | 12 h | 80 (6) | 46.930 | 1.538 / 4.043 | 7.440 |
| 10,000 | Log-uniform | 64 | 10 s | 12 h | 4,927 (4,927) | 63.219 | censored / censored | 8.023 |
| 10,000 | Heavy-tailed | 64 | 10 s | 12 h | 4,980 (4,980) | 61.050 | censored / censored | 6.836 |
| 1,000 | Giant 70 percent | 187 | 10 s | 12 h | 0 (0) | 44.338 | 1.162 / 3.492 | 1.805 |
| 1,000 | Log-uniform | 187 | 10 s | 12 h | 0 (0) | 83.567 | 0.987 / 3.026 | 2.126 |
| 1,000 | Heavy-tailed | 187 | 10 s | 12 h | 0 (0) | 109.883 | 0.992 / 2.974 | 2.905 |
| 10,000 | Giant 70 percent | 187 | 10 s | 12 h | 57 (0) | 47.114 | 1.538 / 3.920 | 7.422 |
| 10,000 | Log-uniform | 187 | 10 s | 12 h | 380 (50) | 121.722 | 1.477 / 3.713 | 7.033 |
| 10,000 | Heavy-tailed | 187 | 10 s | 12 h | 1,680 (140) | 120.727 | 2.047 / 4.745 | 10.653 |
| 10,000 | Giant 70 percent | 187 | 1 s | 12 h | 3,280 (0) | 58.410 | 2.400 / 5.778 | 6.897 |
| 10,000 | Log-uniform | 187 | 1 s | 12 h | 4,094 (1,092) | 168.599 | 4.651 / 8.732 | 6.332 |
| 10,000 | Heavy-tailed | 187 | 1 s | 12 h | 4,984 (86) | 128.125 | censored / censored | 5.438 |
| 10,000 | Giant 70 percent | 187 | 10 s | 48 h | 0 (0) | 63.635 | 1.216 / 3.040 | 11.098 |
| 10,000 | Log-uniform | 187 | 10 s | 48 h | 0 (0) | 129.071 | 0.984 / 2.180 | 12.086 |
| 10,000 | Heavy-tailed | 187 | 10 s | 48 h | 0 (0) | 119.600 | 1.241 / 3.054 | 19.887 |

What the re-run shows:

- **At 1,000 miners the four changes work.** No win was blocked at
  `N = 64` or `N = 187`. The probe was paid every 0.99 to 1.16 days on
  average, matching its 1.15-day threshold; under the written rules it
  waited 15 to 77 days. Medium miners were paid every 0.3 to 0.6 days.
- **At 10,000 miners with a 12-hour window the pool often cannot mine.** At
  `N = 64`, almost every win in the log-uniform and heavy-tailed mixes had
  no valid list, all because 64 payees were too few. At `N = 187`, 380 and
  1,680 of 5,000 wins were blocked, mostly because the balances of miners
  still below their minimum added up to more than the bounded pay-ahead
  could cover.
- **One-second shares made that worse.** With the same 12-hour window, more
  small miners hold balances below their minimum, and each share's expected
  credit is a tenth as large, so less of it reaches anyone's minimum. They
  did let more small miners find shares: in the giant mix, small miners with
  no share fell from 1,022 to 148.
- **A 48-hour window removed every blocked win at 10,000 miners** in all
  three mixes. The probe was paid every 0.98 to 1.24 days, the same as at
  1,000 miners and as its formula, so under these rules its wait does not
  depend on the number of miners. Medium miners were paid every 0.6 to 2.2
  days, sooner than their 3.4 to 4.1-day thresholds because pay-ahead
  reaches them before they have earned it all.
- **Costs.** Fees were 138 to 159 sats per payee across all runs. In the
  48-hour runs, tokens paid ahead and not yet recovered peaked at 11 to 20
  mean rewards; miners who had left held at most 0.04 rewards of advances,
  recovered only if they return; and payments above a miner's own bound,
  from absorbing small leftovers, totalled at most 7.9 rewards out of 5,000.
- **The smallest miners still find no shares.** At 10-second shares a miner
  below about one ten-millionth of the pool almost never finds one: 2,405
  of 2,460 tiny miners in the 10,000-miner giant mix found none in about
  139 days. This
  is the smallest-miner floor the owner raised. A faster share rate
  includes them, but adds share-chain load and, with a 12-hour window, blocks
  wins.

The owner adopted first come, first served, the pay-ahead bound, the
leftover rule, no script cap and the 48-hour window. Still open: `N` (187
studied; version 2 allows up to 188, 238, 198 or 244 by script mix), the
share rate and smallest-miner floor, and what happens to advances held by
miners who leave. None of these runs touched a node, chipnet or mainnet.

### Historical funded Merkle-sum tree: no longer the main path

Everything below through the end of section 5 retains the earlier tree
proposal and its phase 2a evidence for comparison. Its sponsor model,
per-win allocations without accrual, tree state and caps apply to that
alternative only. They do not override the minimum-payout rules above.

### Allocation and tree shape

Use a balanced binary tree of weight commitments. Proposed terminal
buckets contain at most 8 beneficiary scripts; internal nodes have at
most 2 children. These are conservative prototype parameters. Increase
them only after measuring unlocking size, hash budget and execution cost.
Do not inherit the BCH tree's 500-output fan-out.

Aggregate duplicate scripts and sort by locking bytecode. Each beneficiary
has a positive integer weight and a cumulative interval `[lower, upper)`.
Exclude scripts whose aggregated weight is zero, for example those whose
only window items were context-ineligible, before building the tree. Nodes
reject any manifest that contains a zero-weight leaf: its interval would
have `lower = upper`, which the `lower < upper` bound below forbids, and
its whole bucket could never be spent [re-review F4].

Proposed leaf hashes are SHA256 of a leaf-domain byte, canonical script
length and script, and its 64-byte weight. Internal hashes are SHA256 of
a distinct node-domain byte, then the left child's hash, weight and leaf
count, then the right child's. Hashes are 32 bytes; weights are 64-byte
little-endian integers; leaf counts are 2-byte little-endian integers from
1 through 4,096. Use domain byte `0x00` for leaves and `0x01` for internal
nodes. Split the sorted leaf list at its midpoint: `floor(m / 2)` leaves on
the left and `ceil(m / 2)` on the right for a node of `m` leaves.

A node is terminal exactly when its leaf count is at most 8. A terminal
spend reveals the node's leaves and the deterministic subtree needed to
recompute the committed hash, so its leaf count is revealed and
hash-verified. An internal spend reveals both children's hashes, weights,
intervals and leaf counts; the covenant recomputes the committed hash, so
the node's leaf count `m` is hash-verified too. It requires `m > 8`, the
midpoint child counts, and remaining depth at least 1, and gives each child
remaining depth one less.

The root's remaining depth is exactly `ceil(log2(ceil(n / 8)))` for `n`
leaves: 0 for 1 to 8 leaves, so the root acts as a terminal bucket, and 9
for 4,096 leaves. A node with `m` leaves and remaining depth `d` then
always satisfies `m <= 8 * 2^d`. The root does by this choice, and a
midpoint split keeps `ceil(m / 2) <= 8 * 2^(d - 1)`. So every node with
more than 8 leaves has remaining depth at least 1, and no bucket can be
stranded at depth 0. A terminal node may have remaining depth above 0, for
example the 8-leaf left child of a 17-leaf root at depth 2. Nodes reject an
`R` or manifest whose root depth differs from this formula [re-review F4;
arithmetic]. Golden vectors cover 1, 8, 9, 16, 17 and 4,096 leaves.

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

Each internal spend reveals child subtree hashes, weights, intervals and
leaf counts. The covenant recomputes the committed Merkle-sum node and
verifies exact partitioning. Proposed redeem encoding is a minimal push of
canonical immutable state followed by a fixed program. The program uses
`OP_ACTIVEBYTECODE` to extract its own exact program suffix, constructs
the canonical child state, appends that same suffix, then hashes the child
redeem to form its P2SH32 lock. Read `OP_ACTIVEBYTECODE` only in the
top-level redeem script, never inside an `OP_INVOKE`d function: there it
returns the function body from its last code separator, not the redeem
script [S9, OP_INVOKE clarification 3.3; S7]. No `OP_CODESEPARATOR` is
permitted anywhere, including function bodies defined with `OP_DEFINE`
[re-review F4]. Root
state reads the reward dynamically; child state fixes it. This avoids
embedding a program recursively inside itself. It must not trust
witness-supplied child locks or program bytes. The proposed encoding and
caps below are fixed for feasibility testing; bytecode extraction and
enforcement are demonstrated by the isolated libauth prototype below.
This supplies no node or share-chain implementation.

### Frozen initial execution profile

All values in this table are proposed profile bounds. BCH limits in the
following table are separately sourced. Reject excess rather than truncate.
The window walk's script cap (section 3) is part of the credit rule, not
truncation of a finished manifest.

| Item | Initial profile bound |
|---|---|
| Beneficiaries after script aggregation | At most 4,096 leaves, each with positive weight; the section 3 walk stops before a 4,097th script, so an honest window never exceeds this; reject any manifest that does |
| Tree | Binary midpoint split; terminal exactly at 8 or fewer leaves; root depth `ceil(log2(ceil(n / 8)))`; at most 12 Merkle edges from root to leaf; at most 9 distribution splits before a bucket of at most 8 leaves |
| Additional funding inputs | At least 1 and at most 4; at most 5 total inputs including distribution input 0 |
| Beneficiary script | Exactly standard P2PKH, P2SH20 or P2SH32, at most 35 bytes |
| State | Exactly 336 bytes, layout below; minimal `OP_PUSHDATA2` push followed by fixed program |
| Program | At most 4,096 bytes; no `OP_CODESEPARATOR` in the program or any function body; `OP_ACTIVEBYTECODE` only at top level; version identifies exact audited program hash |
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
At top level `OP_ACTIVEBYTECODE` returns the executing redeem script; with
no separator the program removes the fixed 339-byte state-push prefix to
obtain the suffix. It must verify the version's program hash before
reconstructing children.

For `window_budget = 4,320 * bootstrap_work`, scaled total payout weight
is at most `10 * window_budget < 2^272` weight units, even at target 2
[section 3; arithmetic]. Thus this profile's reward product is below
`2^335`; the 575-bit bound also covers the full serialized weight range.
Enforce the tighter window bound when accepting manifests. Funding BCH
values and their sums must pass BCH MoneyRange and checked arithmetic,
never wrap or default. Enforce exact tree counts, intervals and terminal
leaf lengths rather than trusting unverified witness-supplied counts.

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
most 101 bytes and an internal preimage 197 bytes, including the two
2-byte leaf counts, requiring respectively 2 and 4 SHA-256 iterations
[S4, `CalcHashIters`]. An eight-leaf bucket costs
`8 * 2 + 7 * 4 = 44` iterations for its tree alone [state/hash layout;
SHA-256 padding arithmetic]. Program verification, child-script hashing,
arithmetic, token introspection and all other execution are additional.
Measure full internal and terminal transactions, maximum funding and
script sizes, zero allocations, all rounding boundaries and the maximum
tree under standardness and BCH consensus budgets. Old 520-byte item
or 1,650-byte standard unlocking objections do not apply after these
upgrades. Constructibility in the pinned libauth VM is demonstrated below;
BCHN admission remains untested.

### Re-review constructibility estimates

The re-review projected these figures from the frozen bounds. They are
**estimates, not measurements**; section 10's phase 2 must measure them.

| Spend | Unlocking length `L` | Hash iterations | Standard hash limit `floor((41 + L) / 2)` | Operation-cost limit `800 * (41 + L)` |
|---|---|---|---|---|
| Internal | About 4,650 bytes | About 211: program hash 65, node 4, two child P2SH32 locks 71 each | 2,345 iterations | 3,752,800 cost units |
| Terminal, 8 leaves | About 5,330 bytes | About 109: program hash 65, bucket 44 | 2,685 iterations | 4,296,800 cost units |

The limits follow from `L` by BCHN's formulas [S4; arithmetic]. Hashing
is not binding: 211 iterations at 192 cost units each at standard relay
are 40,512 cost units, about 1 percent of the internal budget. The
575-bit arithmetic fits natively: BCH numbers may hold up to 10,000 bytes
since the May 2025 upgrade [S7]. A 4,435-byte child redeem script is
under the 10,000-byte item limit, and BCHN v29.2.0 relays unlocking
bytecode up to 10,000 bytes after the May 2026 upgrade [S5].

The re-review found no way to burn, misroute or double-count tokens or to
inject an NFT. The empty-category funding check excludes every token,
because `OP_UTXOTOKENCATEGORY` pushes an empty item only for a token-free
input [S7]. Tokens can be stranded only through the tree-rule gaps closed
above, or as unrelated UTXOs donated to a payout address, which no
covenant path spends. Funder griefing is limited to first-seen races and
cost inflation.

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

The re-review's estimates, not constructed transactions, give the scale.
Each spend is about 5 KB, so about 5,000 satoshis at 1 satoshi per byte,
because its redeem script alone can reach 4,435 bytes. A full 4,096-leaf
tree needs 1,023 spends (511 internal, 512 terminal): about 5.3 million
satoshis of fees plus about 2.8 million satoshis of leaf dust, about
0.08 BCH in total. One beneficiary funding only its own path, 9 internal
spends and 1 terminal spend, pays about 60,000 satoshis [re-review
constructibility; arithmetic].

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

### Prototype measurements (phase 2a)

The owner authorized the isolated Rust crate
[`p2poolv2_photon_payout`](../../p2poolv2_photon_payout/README.md).
Its annotated `payout.cash`, exact `program.hex` and SHA256 artifact are
checked against the Rust assembler and pinned libauth compiler by tests.
The program is **2,075 bytes**, and every redeem is **2,414 bytes**, including
the minimal three-byte push header and 336-byte state. Its SHA256 is
`5b6e4ea2802307e6fce8b94d8ed6d65751fc54af6370989688e39aa79b628f3c`.
The context digest remains at redeem offsets 43 through 74. There is exactly
one top-level `OP_ACTIVEBYTECODE` and no `OP_CODESEPARATOR` in the source.
Children reconstruct the same suffix rather than trusting witness bytes.

These are measurements from libauth-rs `be3ed16`'s BCH 2026 VM, with both
standard and non-standard execution and complete resolved-transaction
verification. They are **not BCHN acceptance or relay results**. Run
`cargo photon-payout-measure` to reproduce the tables; its complete output
and method are retained in
[`measurements.md`](../../p2poolv2_photon_payout/measurements.md).

All rows use 35-byte P2SH32 beneficiaries, total weight
`43,200 * 2^256 - 1`, original reward `2^63 - 1`, four signed public synthetic
funding inputs and one final P2PKH change output. The internal example is
the 2,048-leaf child of a 4,096-leaf root; the terminal example is its
eight-leaf descendant. Root input BCH is 700 satoshis; children inherit 705.
Ordinary funders have 100-byte P2PKH unlocks. The last three rows replace
each funder with a signed P2SH32 fixture whose unlock is exactly 10,000
bytes, testing the funding-size ceiling against input 0's own budget.
Each transaction verifies in both VM modes. Cost pairs below are
standard / non-standard; `L` always means input 0's unlock length.

| Spend | L | Operation cost | Operation limit | Hash iterations | Hash limits | Peak slots | Control depth | Largest item | Tx bytes and fee (sats) |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Root P2SH20 | 2,617 | 363,534 / 338,830 | 2,126,400 | 193 | 1,329 / 9,303 | 36 | 4 | 2,617 | 3,442 |
| Root P2SH32 | 2,617 | 363,665 / 338,961 | 2,126,400 | 193 | 1,329 / 9,303 | 36 | 4 | 2,617 | 3,442 |
| Internal P2SH32 | 2,617 | 363,519 / 338,815 | 2,126,400 | 193 | 1,329 / 9,303 | 36 | 4 | 2,617 | 3,442 |
| Terminal 8 P2SH32 | 3,222 | 1,038,866 / 1,019,026 | 2,610,400 | 155 | 1,631 / 11,420 | 44 | 9 | 3,222 | 4,569 |
| Root P2SH20, maximum funder unlocks | 2,617 | 403,138 / 378,434 | 2,126,400 | 193 | 1,329 / 9,303 | 36 | 4 | 10,000 | 43,050 |
| Internal, maximum funder unlocks | 2,617 | 403,123 / 378,419 | 2,126,400 | 193 | 1,329 / 9,303 | 36 | 4 | 10,000 | 43,050 |
| Terminal 8, maximum funder unlocks | 3,222 | 1,078,470 / 1,058,630 | 2,610,400 | 155 | 1,631 / 11,420 | 44 | 9 | 10,000 | 44,177 |

Operation limits are `800 * (41 + L)`, and hash limits are respectively
`floor((41 + L) / 2)` and `floor(7 * (41 + L) / 2)`. These costs, iterations
and limits are read directly from the final VM state's metrics. Peak slots
are the maximum of stack length plus altstack length plus `function_count`
over the complete opcode debug trace, including unlocking, locking and
redeem execution. Control depth is the maximum `control_stack.len()` in
that trace. Largest item includes both stacks and function bodies. Thus
the 10,000-byte maximum above includes introspected funding unlocks.
Funders do not enlarge input 0's operation or hash budget.

Fees equal the exact serialized BCH transaction size at 1 satoshi per
byte. Default dust is calculated from each serialized output, including
its actual CashTokens prefix, as `3 * (output_bytes + 148)`. Allocation
outputs above are each 705 satoshis of dust; token-free P2PKH change is
546. The committed largest-prefix minima are 675 for P2PKH, 669 for P2SH20
and 705 for P2SH32. Mixed-script positive tests exercise all three.
Maximum-size funders add exactly 39,608 transaction bytes and satoshis
relative to four ordinary P2PKH funders, including CompactSize growth.

The following full-tree measurements use four ordinary signed P2PKH
funders and change per spend, the same maximum-length beneficiary scripts
and weights/reward above, and a 700-satoshi P2SH20 root. Each child spends
its parent's actual txid and exact output; every spend conserves tokens
and passes both complete VM checks. External BCH is fees plus final leaf
BCH minus the inherited 700 satoshis. It is net consumed funding; temporary
funding UTXO sizes and returned change are not counted as costs.

| Leaves | Spends | Fees (sats) | External BCH (sats) | External BCH | First beneficiary's path (sats) |
|---:|---:|---:|---:|---:|---:|
| 2 | 1 | 3,446 | 4,156 | 0.00004156 | 4,156 |
| 8 | 1 | 4,569 | 9,509 | 0.00009509 | 9,509 |
| 9 | 3 | 11,271 | 16,916 | 0.00016916 | 10,088 |
| 64 | 15 | 60,646 | 105,066 | 0.00105066 | 21,950 |
| 512 | 127 | 509,262 | 869,522 | 0.00869522 | 34,391 |
| 4,096 | 1,023 | 4,098,190 | 6,985,170 | 0.06985170 | 46,832 |

Path funding creates both children at every ancestor and pays the complete
terminal bucket, but executes only the first canonical beneficiary's
branch. For 4,096 leaves this is nine internal spends and one terminal
spend. No token entitlement is taken to pay BCH costs. Using the maximum
10,000-byte funders instead would add `39,608 * spends` satoshis to full
execution and `39,608 * path_spends` to the path under these fixtures.
These are constructed costs, replacing the earlier roughly 0.08 BCH and
60,000-satoshi estimates; they are not fee-rate or sponsor guarantees.

A release-build host benchmark hashed the 2,414-byte `R` 100,000 times in
0.154738 seconds, or 1.547 microseconds per seed, on an AMD Ryzen 9 9955HX3D.
This is one CPU timing under concurrent local work, not a GPU benchmark or
claim-mining throughput result. No GPU engine exists in this crate. The
earlier 4,435-byte `R` hashing model describes the profile maximum, not
these actual bytes; audit the P2SH20 collision-cost margins against the
final program rather than treating the maximum-length estimate as measured.

Positive groups A1 through A8 and all listed group B mutation families
are exercised in both VM modes, including malformed states re-locked to
matching source addresses, a real new genesis category on change, and
`OP_ACTIVEBYTECODE` inside `OP_INVOKE`. Rejection helpers require failure
of the active covenant as well as complete transaction verification, so
invalid funding signatures cannot hide a missing covenant check.
The 4,096-leaf run has 511 internal and 512 terminal spends, exercises
remaining depths 9 through 1, and pays all `2^63 - 1` raw units.
The 100,000-manifest stress test is separately ignored by default and uses
seed `0x70686f746f6e3261`. On 2026-10-30 it passed both complete VM modes
and exact reference telescoping in 156.76 seconds. The full-tree test
passed in 2.84 seconds; the 130 regular tests passed in a serial release
run. Completion and reproduction details are recorded in the crate README.
It uses valid public P2SH20 `OP_TRUE` funding for speed; normal acceptance,
tree execution and measurements use real synthetic funding signatures.

No design contradiction or VM budget failure was found in this prototype.
The initial default 30-second nextest timeout stopped the first stress
attempt; the explicit `photon-heavy` profile leaves normal timeouts intact.
**Open gates:** BCHN regtest admission/relay and group C against the
unmodified PHOTON v3.2 claim covenant are not run in phase 2a. GPU seed
hashing and final `R` security review are also pending. The authentic root
must originate fungible-only: introspection cannot distinguish an immutable
NFT with an empty commitment from a fungible-only category. Token-free
funding and whole-transaction token validation preserve that invariant on
the authentic payout path; arbitrary donated UTXOs do not authenticate a
PHOTON claim. No node code, network access or spending is included.

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
claims can form a mempool chain and land in one BCH block [P1, P4]. On a
validated single-input claim successor, cancel old jobs immediately. Nodes
may speculatively mine on it, but must label and track its dependencies.
On an unconfirmed oracle spend, or any transition with an input other than
the vault, keep mining the current baton; cancel only when it confirms
(section 3, F1). Reorgs or replaced ancestors cancel the whole dependent
branch. A child referring to one txid variant does not spend another
variant's output.

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

Track conflicts first by the baton outpoint spent, then by reconstructed
claim digest, which covers the outputs. One spent outpoint can have
several distinct valid winning proofs as well as locktime variants. Do
not collapse their payout manifests into one. Select the actual accepted transaction before building
a payout spend, whose input must name its real output-index-1 outpoint.
Share-chain reorganization after a win cannot rewrite its locked payout.

Each **confirmed** oracle move adds at least 8,000 satoshis to the vault,
plus fees paid by its funder [P1]. An unconfirmed announcement carries no
such deposit: oracle `O` spends baton `V` and funding input `F`, but a
conflicting spend of `F` alone can confirm elsewhere, invalidating `O`
while leaving `V` unspent. No local BCHN replacement of `O` is required,
and the attempt costs only the conflicting transaction's fee. Section 3
therefore never selects or acts on an unconfirmed oracle transition, so
such an announcement moves no jobs and no credit (F1). The only way to
disrupt pool jobs through the oracle path is a confirmed move. Price
oracle disruption at that confirmed deposit: at least 8,000 satoshis plus
fees. An unconfirmed `O` can still make a pool win on `V` lose to `O`
when `O` confirms.

An oracle move preserves commitment/target but resets the age origin to
the new output's confirmation height (age 0 while unconfirmed, a state
the pool does not mine on). Repeated confirmed moves can suppress
age-based target easing [P1]. Follow validated transitions,
apply section 3's credit rules, roll back conflicting claims and BCH
reorgs, and rate-limit peer processing without ignoring real chain
changes. These risks affect solo and pool miners. The pool cannot ban the
oracle path or infer an attacker's budget from observed messages.

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
| `shares/bch_payout_covenant`, `accounting/payout/payout_tree` | Test-harness and canonical-manifest patterns | Direct funded split, gross balance/reservation ledger and voted minimum parameters; historical tree retained separately | Earlier tree estimate 3 to 6 engineer-weeks; lean integration unestimated |
| `node/payout_tree_executor`, `store/payout_tree`, API | Durable recovery, manifest serving, rebroadcast/reorg concepts | Submitter economic limits and token reimbursement, dynamic funding/change, actual outpoint tracking; remove coinbase-maturity and fixed-txid assumptions | Earlier tree estimate 2 to 4 engineer-weeks; lean integration unestimated |
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
The original tree effort rows summed to 20 to 40 engineer-weeks before
that additional work. They are not a delivery estimate for the new ledger
and lean payout path. Unresolved profile decisions prevent a reliable
delivery estimate.

## 8. Miner protocol and Pickaxe work

Publish a versioned PHOTON protocol specification and byte vectors before
miners depend on it. Pickaxe already prefers Stratum V2 toward pool nodes
[P5]. Prefer a negotiated PHOTON extension over authenticated Noise channels.
It is not compatible with ordinary BCH SV2 jobs just because it shares a
transport. Reserve message identifiers and capability negotiation through
the applicable protocol process; do not silently overload BCH messages.

The protocol needs these operations:

- Open a PHOTON channel with chain/deployment identifier, worker label,
  payout script, minimum multiple, supported layouts and covenant versions. No withdrawal
  key or wallet seed is sent.
- Deliver a job identifier and generation, complete baton anchor, lineage
  digest and path delta, parent and uncle context, PPLNS weights,
  ledger/reservation digest and direct payee list, canonical metadata,
  voted `C`, cap and multiples, the `R` constructor, age, amounts, fee/value
  policy, layout, and the contract, chain and vardiff targets. Include enough data for the client
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
- Deliver immediate job cancellation on selected baton movement (a
  confirmed transition or a validated single-input claim successor) and
  new work on share-tip changes. Support parent/manifest fetch, win
  announcements, payout status and recovery-manifest export.

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
| P2SH20 chosen-pair collision | Complete seed binding limits a colliding pair to at most one settled reward from at most 65,536 tails. Section 4's square-root figure is an attainable estimate, with a floor of about `2.4 * 10^24` HASH160 evaluations per stolen reward as `p` approaches 1, not a deadline or proved lower bound. A public collision on a pool root trips the switch to P2SH32. Auditor sign-off on the final `R` and owner choice remain required; measure P2SH32 under both tail profiles. |
| Oracle griefing or ordinary solo claims | Confirmed oracle moves deposit at least 8,000 satoshis plus funder fees. Nodes never select or act on an unconfirmed oracle transition, or any transition with a non-vault input, so an announcement later killed by a funding-input conflict moves no credit (section 3, F1). A pool win on `V` can still lose to a pending `O`. Oracle moves reset the age origin and suppress target easing. Follow authenticated paths and roll back dependencies. |
| Unconfirmed-oracle credit capture | The F1 counterexample in section 3: honest nodes leave `V` for a doomed `O` while the attacker mines `V` privately. Prevented by mining `V` until `O` confirms; simulate it in phase 3. |
| Payout-script spam in the window | The newest-first walk stops before a 4,097th script; older work gets zero weight and consumes no budget, so a job can always be built (section 3, F2). |
| BCH orphans and tip disagreement | Compatibility uses vault lineage and anchor heights, not an exact BCH block hash, so an orphan that moves no vault transition changes no credit (section 3, F5). |
| Share spam and forged state | Size bounds, per-peer budgets, dependency deduplication, cheap checks before crypto, deterministic chain difficulty, and authenticated baton sources. Weak vardiff work stays local. |
| Selfish share mining, timestamp grinding, stale-parent replay | Greatest cumulative valid work, complete metadata/seed binding, unique proof credit and depth-bounded uncles. Authenticated context forbids reverting to a spent predecessor. Old private context does not prove creation time; delayed-sync and fork incentives still need simulation. |
| Historical baton after oracle spend | Authentic `V` is insufficient when the context includes confirmed `O` spending it. Reject that share and any share that references an old-baton uncle; filter PPLNS by paying lineage. An unconfirmed `O` creates no competing context. Conflicting single-input claims remain a risk until BCH resolves them; each needs a real contract win. |
| A pool miner broadcasts a win to its own address | Changing output index 1 changes the checked HASH256. Reusing the same datasig does not preserve the PoW. A miner can mine a separate solo template, but that proof fails the pool's expected-`R` check and earns no pool credit. |
| Duplicate payout inputs or reused output obligations | Exactly one distribution input at index 0; all funding inputs token-free. Check every input and output, not just the active input. |
| Token burn, theft or altered amounts | The lean split conserves the actual input reward: gross-minus-C to the committed payees, C per actual payee to the submitter. The historical tree uses telescoping intervals. Claim-tail burning remains a separate open profile. Funding category must equal the empty vector; token amount zero cannot exclude NFTs. |
| Cached schedule carry or result-buffer overflow | Retain the conserved-tail carry guard or recompute block 8. Compare at the easier statistical/contract target, detect total hits above capacity, and rescan/subdivide at contract target until every win is recovered. |
| Work overflow or uncle rounding/replay | Checked 512-bit work and weights, bounded wider products, raw-work clipping before conserved 9/1 uncle accounting, and ancestry-wide deduplication by claim digest. Existing saturating u128 PPLNS cannot be inherited. |
| Redirect inherited BCH excess | Proposed beneficiaries have token entitlements and minimum BCH only. Aggregate excess can be assigned among them arbitrarily; funder signatures bind actual outputs. Owner must approve or require deterministic BCH allocation. |
| Funding fee griefing | No inherited payout BCH can go to change or fees. Funders sign complete transactions and fund all fees. Never assume child-pays-for-parent will rescue a low-fee claim. No free public funding wallet. Competing funded spends may delay each other under local first-seen policy. |
| Nobody funds distribution | The main path has no sponsor. Tokens wait in their original R, owed balances remain, and immutable reservations prevent double payment. Settlement needs willing token-reimbursed submitters, retained manifests and a payable tail/list combination. A blocked R is not repaired by later earnings. |
| Missing payout data or eclipse | Require data before credit, replicate and export winning manifests, retain them beyond DAG pruning, use independent peers. Hashes alone cannot recover missing scripts. |
| Reorg or locktime malleability | Persist conflict sets by spent baton and claim digest, resolve actual txids, roll back dependent work and payout tracking. Never treat RPC acceptance as confirmation. |

## 10. Open decisions and phases

The owner's 2026-10-30 minimum-payout choices above replace sponsorship
and per-win tree fan-out as the main path. Also on 2026-10-30 the owner
adopted, for the PHOTON pool only: first come, first served; pay-ahead
bounded by the expected credit of found shares, which replaces any launch
bootstrap or custody; no window script cap; a 48-hour window; and the
version 2 covenant that pays in the share chain's order. BCH P2Pool is
unchanged. The following remain **open owner decisions**, with no
default selected by this study:

- Miners who leave below their minimum: pay when a split has room, or
  expire after a long time. Neither policy is implemented or assumed.
- Miners who leave while paid ahead: the study measured at most 0.04 mean
  rewards unrecovered over a 48-hour-window run; no policy is selected.
- The payee cap `N` and the initial raw-token value of `C`.
- How often `C` may change and the upgrade vote's threshold.
- P2SH20 or P2SH32 for claim output index 1, after review of final lean `R`.
- The share rate needed by the smallest miners; a fee threshold cannot
  give credit to a miner who has not found a credited chain share. At
  10-second shares, miners below about one ten-millionth of the pool
  almost never find one.

Coverage gaps, fixed-list reservations, multiple-change activation and
the price/no-sponsor execution condition in section 5 also need explicit
resolution before share-chain implementation. A possible week-averaged
Cauldron price remains a later option, outside this design.

The owner must decide the following before a profile is approved:

- Share parameters other than the 48-hour window, depth-based uncle
  policy, restart after an outage longer than the path-delta bound,
  authenticated-context lag policy and optional per-share fees. Sponsor
  budgets belong only to the retained historical tree, not the main path.
- Conserved tail or bounded tail burning. Burning trades token supply
  for a possible SHA-bound candidate-rate gain in solo and pool mining;
  it is not assumed merely because BCHN permits implicit burns.
- P2SH20 under the re-review's five conditions (section 4), or P2SH32
  after measurements under both tail profiles.
- Initial age range through 32,767 BCH blocks, or separately validated
  layouts/vectors through 65,534 BCH blocks. The latter still fits ten
  first-pass blocks with P2SH20.
- The proposed aggregate-only BCH excess rule, or a deterministic
  individual BCH allocation enforced by a revised covenant. The prototype
  freezes the aggregate-only rule as its default; this decision stays
  pending.

The historical tree/state/funding/arithmetic caps in section 5 remain fixed
prototype bounds for that alternative. Its program and tests are retained.
The main path reimburses submitters in tokens as specified above and has
no sponsor; no initial `C` or cap `N` has been approved here.

### Gate status

The independent re-review of `4ecec7aa` sets these gates:

- **GO** for an isolated, test-only payout-covenant feasibility prototype:
  no node integration and no chipnet spending. It builds on section 5 with
  the F4 gaps closed (zero-weight exclusion, leaf-count terminal rule, root
  depth, top-level `OP_ACTIVEBYTECODE`), which this revision does. The
  aggregate-only BCH excess rule is its frozen default; the owner's
  decision on that rule stays pending. The root lock is a parameter,
  P2SH20 or P2SH32.
- **NO-GO** for share-chain or node work until F1 and F2 are fixed. This
  revision proposes fixes for both in section 3; they need re-review
  before the gate opens.

The owner subsequently authorized phase 2a only. **Phase 2a passes in pinned
libauth**: all A/B cases, the 1,023-spend execution and 100,000 seeded
manifests pass in both VM modes; group D measurements are in section 5.
The full phase 2 gate remains open for BCHN acceptance and phase 2b group C.
Its evidence and reproduction commands are in the prototype README.
This does not open the share-chain/node gate or authorize network spending.

Review and implementation gates, in order:

1. **Design review only.** Done for `4ecec7aa` by Pickaxe and the
   independent re-review. The auditor must re-review this revision's
   F1 to F7 changes, the final `R` bytes, private forks, context
   eligibility, freshness/sync rules, no-NFT state, funding isolation,
   BCH excess and child-script construction before share-chain work.
2. **Covenant feasibility, phase 2a authorized.** The Rust-only prototype
   now supplies program bytes, reference allocation/builders, adversarial
   VM tests and measured costs (section 5). Pinned libauth execution is
   demonstrated; BCHN acceptance and phase 2b group C remain open.
   Use libauth-rs at the workspace's pinned revision `be3ed16` with its
   BCH 2026 virtual machine, as `p2poolv2_safa` already does, together with
   BCHN regtest `testmempoolaccept` as the authority. The local test VM in
   `p2poolv2_lib/src/test_utils/bch_vm.rs` has loops and token
   introspection but only P2SH32 helpers, no `OP_DEFINE` or `OP_INVOKE`,
   and numbers of at most 16 bytes, below the 575-bit intermediates.
   Extend it or do not rely on it. Validate whole transactions with BCHN,
   not only individual script invocations. The checklist below is the
   re-review's acceptance tests.
3. **Protocol and simulation.** Publish golden share/job/payout vectors,
   including the window walk's script-cap stop point. Simulate honest
   latency, hash changes, partitions, private forks, withholding, oracle
   moves, the F1 unconfirmed-oracle counterexample, BCH orphans with an
   unchanged lineage, script spam against the cap, long outages and
   startup. Freeze a separate profile and tag only when those results
   support the chosen parameters.
4. **Sister node and independent clients.** Extract the minimum shared
   interfaces. Implement DAG/store/proof/manifest paths and Pickaxe layouts.
   Require another client to submit shares and independently reconstruct
   a win and payout from the public specification.
5. **Chipnet before mainnet.** Exercise real multi-node mining, competing
   wins, mempool chaining, locktime variants, oracle moves, funded tree
   execution, sponsor refusal, restart and BCH reorg recovery. Use separate
   authorization for any spending. Mainnet needs the owner's approval
   after the audit and measured results.

### Historical funded-tree phase 2 acceptance checklist

These are the re-review's tests for the retained funded tree, not a lean
split deployment approval. Every positive case must pass,
and every mutation must fail, in the standard and non-standard libauth-rs
VMs and in BCHN regtest.

**A. Positive cases.**

1. Root spend, P2SH20 and P2SH32, internal mode with 2 children, 4 funding
   inputs and change.
2. Internal spend at every remaining depth from 9 down to 1.
3. A terminal bucket of 8 leaves, all 35-byte P2SH32 scripts with 9-byte
   amounts, total weight near `2^272`, and 4 funding inputs.
4. Terminal buckets of 1 leaf, and a root acting as a terminal bucket.
5. Full 4,096-leaf execution: 1,023 spends, with token conservation
   checked at every step.
6. Rewards of 1, `2^32` and `2^63 - 1` raw units.
7. A zero-allocation child, a zero-allocation leaf, and all allocations
   but one zero.
8. Exact-integer and off-by-one rounding boundaries, plus at least `10^5`
   random manifests checked against a Rust reference for exact
   telescoping.

**B. Mutations.**

- Amounts and tokens: any output amount changed by 1 raw unit either way;
  a burn; token change; an extra token output; a wrong category; tokens of
  a new genesis category on change.
- Funding inputs: one carrying an immutable NFT with an empty commitment,
  a mutable NFT, a minting NFT, PHOTON fungible tokens or another
  category's fungible tokens; a second payout node as an input.
- Transaction shape: active input not at index 0; 1 or 6 inputs in total;
  extra outputs; change not last; two change outputs; version 1; nonzero
  locktime; a non-final sequence.
- BCH values: an output below its committed BCH minimum; inherited BCH
  peeled to change or fee.
- State and tree: a witness-supplied child lock; each of the 12 state
  fields mutated; a wrong depth decrement; an internal spend at depth 0;
  a 9-leaf terminal; a leaf/internal domain swap;
  `wl + wr != upper - lower`; an odd split with the larger half on the
  left; a zero-weight leaf.
- Program: one program bit flipped; `OP_CODESEPARATOR` inserted;
  `OP_ACTIVEBYTECODE` read inside `OP_INVOKE`.
- Addresses: a wrong-category or BCH-only UTXO at a root address; an
  amount other than the prescribed one at a child address.
- Added for this revision's leaf-count rule: a child leaf count other than
  the midpoint split; an internal spend of a node with at most 8 leaves.

**C. Claim side, with the unmodified v3.2 covenant.**

- P2SH20 pool claims at ages 0, 17, 128 and 32,767 BCH blocks pass.
- Locktime variants keep the same proof.
- Mutating any byte of `R`, or the key, nonce, datasig (through `k`), age,
  sequence or BCH values, changes the digest or causes rejection.
- Golden vectors for `R` and `HASH160(R)`.

**D. Numbers to record per spend type.**

- `L`, redeem length and program length.
- Operation cost against its limit; hash iterations against the standard
  `floor((41 + L) / 2)` and consensus `floor(7 * (41 + L) / 2)` limits.
- Peak stack plus altstack plus function-table count, peak control depth
  and maximum stack item size.
- Transaction size and fee at 1 satoshi per byte, and dust per output.
- Total BCH to execute trees of 2, 8, 9, 64, 512 and 4,096 leaves, and the
  cost for one beneficiary to fund its own path.
- `HASH160(R)` cost per seed on CPU and GPU.

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
- **2026-10-29 independent review**, against `85a8afec`: revisions
  `5373473b` and `4ecec7aa` responded to all nine findings with complete
  seed/datasig binding, authenticated baton contexts, frozen execution
  bounds/VM budgets, widened work and conserved clipping, confirmed versus
  speculative oracle cost, explicit BCH excess policy, cached-block carry
  guard and the initial age cap. Funding inputs must have exactly empty
  category introspection. The 60-second argument is withdrawn. Its 575-bit
  product and 44-iteration bucket estimates are confirmed by the bounded
  encoding; full covenant feasibility is unmeasured. Not every finding was
  fully resolved. The re-review below judged findings 1, 2, 5, 8 and 9
  resolved; finding 4 resolved as a specification only, unmeasured, with
  residual gaps F2 and F4; finding 6 resolved in text, with its credit
  consequence left as F1; finding 7 resolved as explicit policy, pending
  the owner; and finding 3, the historical baton, only partly resolved,
  with F1 and a one-share race after BCH block propagation left open.
- **2026-10-29 independent re-review**, against `4ecec7aa`: found the
  earlier blocker resolved and proof-seed binding complete, and P2SH20
  acceptable with changes. Two new major share-chain findings block node
  work: an unconfirmed oracle transition let an attacker capture window
  credit for one conflicting fee (F1), and the 4,096-leaf cap could halt
  job building (F2). This revision folds in confirmed-only oracle
  selection; a deterministic script cap in the window walk; the corrected
  collision wording, `p -> 1` floor and uncounted margins (F3);
  zero-weight exclusion, the leaf-count terminal rule, root depth
  `ceil(log2(ceil(n / 8)))` and top-level-only `OP_ACTIVEBYTECODE` (F4);
  lineage-and-height compatibility, bounded incremental path evidence and
  rejection of ineligible uncles (F5); claim-digest deduplication (F6);
  and source and history corrections (F7). It also records the P2SH20
  verdict, the constructibility and funder-cost estimates, the gate status
  and the phase 2 checklist. The re-review's claims checked here against
  BCHN v29.2.0, the functions CHIP and Pickaxe sources held; none was
  rejected. These changes are not yet re-reviewed.

## Sources

PHOTON sources were inspected read-only in `D:/pickaxe-electric`. Links
P1 to P10 pin revision `2f7cbb0`, which this document inspected; the
Pickaxe review checked dev `640e9d2` (section 2). Byte offsets above are
derived from its serializer, not from comments in the template.

- **P1:** [v3.2 template](https://github.com/CyberAshven/pickaxe-miner/blob/2f7cbb0493f2a46ef89d4f04c941074311a09ba0/reference/photon_v32_template.json), `scripts.lock.script`; opcode source is authoritative where comments disagree.
- **P2:** [v3.2 redeem hex](https://github.com/CyberAshven/pickaxe-miner/blob/2f7cbb0493f2a46ef89d4f04c941074311a09ba0/reference/photon_v32_redeem.hex); decoded length and HASH256 checked against P3.
- **P3:** [protocol constants and derivation](https://github.com/CyberAshven/pickaxe-miner/blob/2f7cbb0493f2a46ef89d4f04c941074311a09ba0/src/protocol.rs), `PhotonDeployment`, `ProofRule::Positive`, `derive_photon_state`.
- **P4:** [protocol profile](https://github.com/CyberAshven/pickaxe-miner/blob/2f7cbb0493f2a46ef89d4f04c941074311a09ba0/docs/protocol-profile.md), layout shifts and supported ages.
- **P5:** [Pickaxe architecture](https://github.com/CyberAshven/pickaxe-miner/blob/2f7cbb0493f2a46ef89d4f04c941074311a09ba0/docs/architecture.md), mining destinations and GPU engines.
- **P6:** [reward handling](https://github.com/CyberAshven/pickaxe-miner/blob/2f7cbb0493f2a46ef89d4f04c941074311a09ba0/src/reward.rs), `TOKEN_OUTPUT_SATS`, retired-v0 constants and settlement paths.
- **P7:** [transaction serialization](https://github.com/CyberAshven/pickaxe-miner/blob/2f7cbb0493f2a46ef89d4f04c941074311a09ba0/src/tx.rs), `PhotonLayout`, conserved T2 amount updates and encoding guards.
- **P8:** [Rust T2](https://github.com/CyberAshven/pickaxe-miner/blob/2f7cbb0493f2a46ef89d4f04c941074311a09ba0/rust-engine/src/t2.rs) and [CUDA tail loop](https://github.com/CyberAshven/pickaxe-miner/blob/2f7cbb0493f2a46ef89d4f04c941074311a09ba0/cuda/photon_t2_tail.cu), block preparation and conserved tail hashing.
- **P9:** [search worker](https://github.com/CyberAshven/pickaxe-miner/blob/2f7cbb0493f2a46ef89d4f04c941074311a09ba0/src/search.rs), `rotate_search_identity`: a fresh random datasig key on job installation and after a job's 2^32 nonces run out; and [CUDA T2 host](https://github.com/CyberAshven/pickaxe-miner/blob/2f7cbb0493f2a46ef89d4f04c941074311a09ba0/src/cuda_t2.rs), `T2Batch::total_winners` and `total_winners.min(winner_cap)` returned hits.
- **P10:** [BCH Schnorr signer](https://github.com/CyberAshven/pickaxe-miner/blob/2f7cbb0493f2a46ef89d4f04c941074311a09ba0/src/crypto.rs), `bch_schnorr_sign` with an RFC6979 nonce and `bch_schnorr_sign_search_candidate` with a caller-chosen `k`.
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
- **S4:** [BCHN v29.2.0 VM limits](https://github.com/bitcoin-cash-node/bitcoin-cash-node/blob/v29.2.0/src/script/vm_limits.h): `MAX_SCRIPT_SIZE`, `MAX_STACK_SIZE`, `MAX_CONTROL_STACK_DEPTH`, `GetInputOpCostLimit`, `GetInputHashItersLimit`, `GetHashIterOpCostFactor` and `CalcHashIters`.
- **S5:** [BCHN v29.2.0 standardness](https://github.com/bitcoin-cash-node/bitcoin-cash-node/blob/v29.2.0/src/policy/policy.cpp), `IsStandardTx`: unlocking bytecode up to `MAX_SCRIPT_SIZE`, 10,000 bytes, once the May 2026 upgrade is active, instead of 1,650 bytes; and [`MAX_STANDARD_TX_SIZE`](https://github.com/bitcoin-cash-node/bitcoin-cash-node/blob/v29.2.0/src/policy/policy.h), 100,000 bytes.
- **S6:** [BCHN v29.2.0 token validation](https://github.com/bitcoin-cash-node/bitcoin-cash-node/blob/v29.2.0/src/consensus/tokens.cpp), `CheckTxTokens`: outputs above inputs fail with `bad-txns-token-in-belowout`; a positive unused balance is not rejected. The [mempool path](https://github.com/bitcoin-cash-node/bitcoin-cash-node/blob/v29.2.0/src/validation.cpp) calls only `CheckTxTokens` for tokens, and S5's policy file adds no implicit-burn rule.
- **S7:** [BCHN v29.2.0 interpreter](https://github.com/bitcoin-cash-node/bitcoin-cash-node/blob/v29.2.0/src/script/interpreter.cpp): `OP_UTXOTOKENCATEGORY` pushes an empty item only for a token-free input, otherwise the 32-byte category plus a capability byte for mutable or minting NFTs; `OP_ACTIVEBYTECODE` and `OP_CODESEPARATOR` act on the current `OP_INVOKE` frame; and [script numbers](https://github.com/bitcoin-cash-node/bitcoin-cash-node/blob/v29.2.0/src/script/script.h), `MAXIMUM_ELEMENT_SIZE_BIG_INT` of 10,000 bytes.
- **S8:** P. C. van Oorschot and M. J. Wiener, [Parallel Collision Search with Cryptanalytic Applications](https://doi.org/10.1007/PL00003816), Journal of Cryptology 12(1), 1999, pages 1 to 28: distinguished-point search for many collisions and the separate golden-collision case.
- **S9:** [CHIP-2025-05 Functions](https://github.com/bitjson/bch-functions/blob/e36e9bd99dc8efb6417bf645aa9d5f37fb9824c7/readme.md), version 2.0.2, scheduled for the May 2026 upgrade (2026-05-15 on mainnet) and implemented in BCHN v29.2.0: OP_INVOKE clarifications 3.2 and 3.3, code separators and `OP_ACTIVEBYTECODE` inside an invoked function.
