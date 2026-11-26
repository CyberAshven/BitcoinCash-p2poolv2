# Proposal: a decentralized pool for PHOTON GPU miners

**Status: design for an unbuilt feature. Whether to build it is the
owner's decision.** The payout path is the lean direct split (section 5).
The parameters are proposed PHOTON share-chain rules; they change neither
Bitcoin Cash consensus nor PHOTON v3.2. Only offline work exists: the
payout covenant prototype and simulator in
[`p2poolv2_photon_payout`](../../p2poolv2_photon_payout/README.md). No node
integration, network calls or spending are authorized; share-chain and
node work wait on the gates in section 10.

## 1. Goal and scope

A sister program in this workspace runs a separate peer-to-peer share chain
for PHOTON miners. Each winning claim locks its reward to a payout
commitment `R` built from the preceding share-chain ledger. Won tokens
credit miners by the PPLNS window, and unpaid balances carry between wins.
Anyone can front a split's BCH for a token reimbursement. No operator holds
the reward key or a private ledger. Pickaxe is one client; the protocol
permits others. The owner preferred this to a hosted Pickaxe pool. It is a
standalone token share chain, one program per token on that token's own
proof of work; the BCH share chain keeps mining BCH with tokens merge-mined
on its work (SAFA), and BCH P2Pool's payout rules are unchanged.

The aim is lower payout variance without a custodian. PHOTON aims at about
one emission per BCH block [P1]. At one emission per 600 seconds a miner
with 1/10,000 of the work waits 6,000,000 seconds (69.4 days) on average
solo, with a 36.8 percent chance of no win in that time; a pool with
fraction `pool_fraction` of the work wins every
`600 seconds / pool_fraction`. PPLNS spreads the pool's wins; it does not
create PHOTON, promise frequent settlement or fund BCH fees. With one-minute
BCH blocks (CHIP-2025-03: chipnet from block 332,451, mainnet on
2027-05-15), spans counted in BCH blocks, such as claim age, cover a tenth
of the time.

Non-goals: a new token category or vault (the pool serves the existing
v3.2 token by [2qx](https://github.com/2qx/vox)), BCH block mining, merge
mining, tradable shares, a hosted service, and token-to-BCH exchange.

## 2. Verified PHOTON v3.2 constraints

### Source boundary

Sources P1 to P10 pin Pickaxe revision
`2f7cbb0493f2a46ef89d4f04c941074311a09ba0`; the template, redeem script,
constants, signer and CUDA tail loop are the same in dev
`640e9d24b04f5cc2ce72bedc871f3e3f28422a92`. Offsets come from Pickaxe's
serializer. This is source verification, not a query of live UTXOs. The
redeem script is 273 bytes, HASH256
`0ab476a6dab00ba11118a11c078f871c5060a66e18140368425202772d0e80c2`. The
lock `aa200ab476a6dab00ba11118a11c078f871c5060a66e18140368425202772d0e80c287`
is the same on mainnet and chipnet, with different categories [P1-P3].
There is no administrator branch; every spend keeps the lock and category.

### Claim and oracle branches

- A claim is version 2 with exactly one input and two outputs [P1].
  Output 0 recreates the vault: category, mutable NFT capability and a new
  100-byte commitment (4-byte nonce, 32-byte next target, 64-byte datasig).
  The vault keeps at least `balance - floor(balance / 420,000)` raw tokens
  and `input_value - 1,500` satoshis.
- Output 1 is the reward: PHOTON category (copied into the proof preimage,
  not compared with the vault's), bitfield `0x10` (fungible, no NFT), at
  most `floor(balance / 420,000)` raw units; Pickaxe takes one unit less
  before its tail decrement [P1, P3]. Its BCH and the fee share the
  1,500-satoshi allowance; a funding input would select the oracle branch.
- The reward lock is free up to 84 bytes [P1, P7]; P2PKH, P2SH20 and
  P2SH32 are the useful forms. Pickaxe pays 700 satoshis [P6]. A P2SH20
  token output is 75 bytes with 669 satoshis of dust [S3]; 700 satoshis
  plus a 627-to-630-satoshi fee at 1 satoshi per byte fits every age layout
  (P2PKH: 629 to 631). If fees and dust do not fit, mining waits.
- The oracle branch takes more than one input, adds at least 8,000
  satoshis to the vault and keeps its tokens and commitment; the caller
  pays fees [P1]. It moves the vault without a proof and withdraws nothing.
  `reward.rs`'s reserve-funded split serves retired v0, not v3.2 [P6].

### Encodings and the proof

- The vault amount is always rebuilt as `0xff` plus 8 bytes, so at least
  `2^32` raw units must remain. Rewards in `[2^31, 2^32)` or below `2^16`
  do not match the reward encoding [P1, P7]. The initial profile keeps both
  amounts at least `2^32` across the tail range and claimed age within 0 to
  32,767 BCH blocks, and fails closed outside. Later emission epochs need
  their own layouts and vectors.
- The datasig signs `nonce || next_target` under a supplied key; it does not
  authenticate a payout owner. The age passes CSV, is below 65,535, may be
  less than the available age, and is 0 for an unconfirmed predecessor. The
  target is `floor(previous_target * (age + 143) / 144)`. Target and digest
  are positive little-endian ScriptNums, `0 < digest_number < target`, not
  an unsigned 256-bit comparison [P1, P3].
- The covenant hashes the claim rebuilt with locktime 0 and never reads the
  actual locktime. Changing the reward script or amount changes the hash,
  so a win cannot be redirected.

### Byte layout and GPU implications

In the P2PKH layout with ages 0 to 16 [P7], the key is at bytes 45 to 77,
the age push at 78, the new commitment at 404 to 503, the vault amount at
504 to 512, the reward amount at 591 to 599, the reward lock at 600 to 624
(64-byte block 9) and the locktime at 625 to 628; a P2SH20 lock ends at
622. Fields after the age push shift by 1 byte for ages 17 to 127 and 2 for
128 to 32,767. P2SH20 claims are 627 to 629 bytes and P2PKH 629 to 631, both 10
first-pass SHA-256 blocks; P2SH32 claims are 639 to 641 bytes, 11 blocks
[P4, P7]. Ages 32,768 to 65,534, excluded initially, shift by 3: P2SH20 is
then 630 bytes, still 10 blocks; P2PKH 632, 11 blocks.

### Conserved and burning tail profiles

With `base = floor(balance / 420,000) - 1` and `tail` from 0 to 65,535 raw
units, the conserved profile pays `base - tail` and leaves
`balance - base + tail` in the vault (`t2_parent_reward_amount`). The
covenant enforces only the vault floor, and Bitcoin Cash consensus allows
implicit token burns [S1, S6], so conservation is a miner-profile rule. Its
tail changes blocks 7 and 9 and can carry into block 8, so Pickaxe's
`supports_t2_window` guard stays (at age 128, vault 2,089,072,092,774,399
plus reward 4,973,993,015 raw units, tail 1 changes byte 512). A candidate
costs `(50 + 64 + 64 + 64) / 64 = 3.78125` compressions [P8]; P2SH32 about
4.8, 21 percent fewer candidates.

A burning profile (open decision) leaves `balance - base`, pays
`base - tail` and burns `tail`. Only block 9 and the second hash change:
`(60 + 64) / 64 = 1.9375`, about 1.95 times the candidate rate if SHA-bound
(P2SH32 about 2.9, 34 percent fewer). It burns at most 65,535 raw units per
win, 32,767.5 on average if winning tails are uniform. BCHN v29.2.0 accepts
an unused token balance and has no implicit-burn policy [S6]. None of this
is benchmarked or relay-tested. Payout spends always conserve.

## 3. PHOTON shares and share-chain rules

PHOTON has its own share types, genesis, database and P2P protocol
identifiers. A claim never goes into a BCH `ShareHeader.bitcoin_header`,
and one proof never earns credit on both chains.

### Canonical fields

| Group | Proposed fields and widths |
|---|---|
| Domain | Wire version, 2 bytes; chain identifier, 32; BCH network, 4; category, 32; vault lock hash, 32 |
| DAG metadata | Parent share identifier, 32; height, 8; Unix timestamp, 8; ordered uncle identifiers, 32 each |
| Work and ownership | Share target, 32; payout locking bytecode with length; minimum multiple (at least 10, default 20); per-share fee policy, if approved |
| Baton anchor | Spent txid, 32; output index, 4; source satoshis, 8; source raw tokens, 8; source commitment, 100; source confirmation height, 4 (`0xffffffff` while unconfirmed); context BCH height, 4; vault-lineage digest, 32; transition count, 8; bounded path delta from the parent |
| Distribution | Parent-window weights commitment, total weight, ledger/reservation digest, voted `C`, payout cap/profile and covenant version; direct payout list in `R` |
| Proof | Compressed public key, 33; nonce, 4; datasig, 64; tail, 2; age in BCH blocks, 2 |

Integers are fixed-width little-endian, hashes raw, lengths minimal
CompactSize. Duplicate fields, trailing data, nonminimal encodings and
unknown versions are rejected; golden vectors come before code. Work and
weights are checked unsigned 64-byte integers; overflow is an error.
`metadata_hash` is SHA256 of everything except the 105-byte proof group and
is bound in `R` before the tail search. The share identifier is HASH256 of
a domain tag, metadata and proof; it holds the spent baton outpoint, not
the winning txid.

The payout list comes from the parent, without the share and its new
uncles. At launch every balance is empty, so the first list holds bounded
advances only (section 5): no bootstrap rule, no custody.

A job is a reconstruction specification, not a final template. The miner
uses its own transient datasig key, rotated per job and on nonce exhaustion
[P9]; fills bytes 45 to 77; computes its midstate; signs; builds `R` and the
reward lock; and runs the tail loop. The reward is exactly 700 satoshis and
the fee the claim length at 1 satoshi per byte [P6], pinned in the seed.
A new timestamp or job changes `R`, and so only block 9; a new key changes
the prefix [P7].

### Validation order

1. Bound sizes and queue before expensive work. Check domain, version,
   category, encoding and duplicate identifier.
2. Resolve parent and uncles. Check height, time, uncle eligibility (one
   ineligible uncle invalidates the share) and the share target. Rebuild
   the window weights.
3. Authenticate the spent baton against a validated BCH source and its
   ancestry (lock, category, value, capability, commitment, amount), then
   apply the lineage rule below.
4. Check the claimed age against the committed source confirmation and
   context heights and derive the contract target; sequence equals age.
   Old shares use their own committed context. Unconfirmed chaining
   validates the whole predecessor path at age 0.
5. Verify the datasig, bind the seed in `R`, rebuild the exact claim with
   the prescribed tail and reward lock, and check encodings, BCH values,
   fee, dust, the tail profile and the target predicate: one datasig check
   and one HASH256.
6. A hash under the share target whose claim digest is uncredited on the
   ancestry earns one share. A hash under the contract target is broadcast
   at once, even if it misses the share target; it earns no extra credit.

An actual win is also checked against the unmodified covenant in the BCH
2026 VM and by BCHN transaction acceptance.

### Authenticated baton context and eligibility

The chain starts at one designated deployment vault outpoint. Each share
commits a vault lineage: the claim and oracle transitions from that outpoint
to its baton, checked against a validated BCH chain and UTXO view. The
lineage digest starts as SHA256 of a domain tag and the deployment outpoint;
each transition replaces it with SHA256 of the tag, the previous digest and
the new 36-byte outpoint. Shares also commit the transition count.
Confirmation heights are not in the digest.

Each share carries a path delta from its parent: per transition the new
outpoint and its confirmation height (40 bytes), or the whole transaction
for an unconfirmed claim. At most 256 confirmed entries and 8 unconfirmed
claims of at most 1,000 bytes, so at most 18,240 bytes. A parent more than
256 transitions behind (about 43 hours of claims) cannot be extended;
restart after such an outage is an open decision.

- A child's lineage extends its parent's, and its context height is at
  least its parent's. The spent baton is the lineage's unspent tip.
- A context holds every transition confirmed at or below its height,
  oracle moves included, then only unconfirmed single-input claims.
- `age <= context_height + 1 - source_confirmation_height` [P1].
- A context above the node's best validated BCH height waits, within the
  peer queue bound; new jobs use the latest validated height. A reorg
  rebuilds a compatible branch; old proofs are never rewritten.

Two contexts are compatible when one lineage extends the other and each
share's source confirmation height matches the node's BCH view; an age-0
share only needs its source to be the tip. Binding an exact BCH block hash
instead would make every BCH orphan roll back about 60 honest shares (one
600-second block of 10-second shares) even with the lineage unchanged.

### Unconfirmed transitions

A node may build on an unconfirmed transition only if it, and every
earlier unconfirmed one on the path, is a claim whose sole input is the
vault, validated at age 0. Only a conflicting claim, a real contract win,
can displace it.

Nodes never select, mine on or cancel jobs for an unconfirmed oracle
transition `O` or any transition with a non-vault input; they keep mining
the baton `V` until `O` confirms. Otherwise an attacker could broadcast `O`
spending `V` and a funding input `F`, get a conflicting spend of `F` mined
(no 8,000-satoshi deposit), and take all window credit on a private `V`
branch, about 60 shares, for one fee per BCH block. The cost of the rule: a
pool win on `V` can lose to `O`, about one BCH block of wins.

Mempool first-seen order is not a share-chain rule. Descendants cannot
revert to a spent predecessor. A conflicting confirmed claim or a BCH reorg
rolls back the dependent branch and its jobs.

### Credit eligibility

PPLNS work must have been eligible in its own context and be compatible
with the paying share's lineage and heights; pre-transition work can stay
in the window. An uncle must also use the includer's baton. A share whose
context holds a confirmed `O` spending `V` cannot mine on `V`. Depth rules
and lineage filtering bound the credit of a private old-context proof;
they do not prove when it was made.

### Targets, PPLNS and uncles

The contract, share-chain and vardiff targets stay separate. Only the
share-chain target earns PPLNS work; vardiff work never enters the DAG.

- **Share target.** 10-second shares and a 600-second ASERT half-life on
  the existing integer ASERT core, re-anchored from the parent [B1]. Ceiling
  and bootstrap target come after simulation. Targets are positive, at most
  `2^255 - 1`, and never derived from claimed hashrate.
- **Work.** `floor(2^256 / (T - 1))` for target `T > 1`, used by fork choice
  and PPLNS alike. At `T = 2` that is `2^256`, so work, chainwork and
  weights are checked 512-bit integers, not BCH's 256-bit type or the
  saturating `work_to_u128` [B2]. At most `2^64 - 1` shares keep chainwork
  below `2^320`. Uncle work is paid once and adds no chainwork.
- **Window.** A fixed work budget of 17,280 bootstrap shares, nominally 48
  hours at 10-second shares, not a rolling 48 hours; a 12-hour window
  (4,320 shares) blocked wins at 10,000 miners (section 5). A window tied
  to expected claim work would need pool-hopping analysis first. Credit
  aggregates by payout script. No coinbase maturity skip, traded payout
  rights, BCH-difficulty multiplier or BCH dust merging applies.
- **Uncles.** At most 2 per share within 6 shares of ancestry [B2]. Fork
  choice is greatest cumulative valid work, ties by ascending share
  identifier. Credit is deduplicated across the ancestry by claim digest,
  HASH256 of the claim rebuilt with locktime 0, which covers `HASH160(R)`
  and all locktime variants; the index is reorg-aware.
- **Walk.** Newest first: each entry's own work, then its eligible uncles
  by ascending claim digest, each clipped to the remaining budget. Own work
  earns `10 * clipped`; an uncle `9 * clipped` for its finder and `clipped`
  for the includer. Context-ineligible items use budget at zero weight.
  There is no cap on distinct scripts; zero totals are dropped.
- **Time.** A timestamp must exceed the median of the previous 11, with 15
  seconds of future allowance. This proves no wall-clock bound; freshness
  rests on depth and cumulative work, which still need adversarial testing.

## 4. The payout commitment R

`R` is the payout redeem bytecode. Claim output 1 is
`OP_HASH160 <HASH160(R)> OP_EQUAL` in the proposed P2SH20 profile; P2SH32
is the fallback. Nodes rebuild `R` and never accept a script hash without
its expected preimage. `R` commits:

- the pool domain, covenant version, BCH network, category and deployment
  lock hash;
- the parent share, spent baton outpoint, `metadata_hash`, window weights
  and ledger digest with reservations (BCH script cannot check the ledger);
- the ordered payee scripts, caps and multiples, `C`, profile version and
  the output and funding rules of section 5;
- the complete proof seed: key, message nonce, exact datasig bytes, age,
  sequence, contract target, amount layout, tail profile and range,
  locktime 0, BCH values and fee rule.

The context binds the proof of work to the share's payee, time, target and
uncles; without it anyone could relabel a public proof. The miner signs
`nonce || target`, builds `R`, then searches tails, so the datasig never
signs `R`. Exact datasig bytes are bound because a signer can vary its
Schnorr nonce `k` [P10]. One `R` serves one seed and at most 65,536 tails,
and several winning tails still spend one baton for one reward.

`HASH160(R)` is computed once per seed. Pickaxe has RIPEMD-160 only on the
host, so it needs a GPU seed path in Rust, CUDA, HIP and wgpu, or a host
round trip per batch: about 24,000 per second at 1.6 GH/s per device
(48,000 when burning). Both are to be measured; a 2,414-byte `R` hashed in
1.547 microseconds on one CPU, with no GPU figure.

### P2SH20 or P2SH32

P2SH20 keeps claims within 10 SHA-256 blocks; P2SH32 costs about 21
(conserved) to 34 percent (burning) of the candidate rate, unmeasured. The
choice is open. The numbers [S2, S8]:

- A chosen-pair HASH160 collision costs about `2^80`; a second preimage of
  a fixed honest `R` about `2^160`. A pair pays only if its honest side
  holds a winning tail, with per-seed probability `p`, about
  `candidate_count * (contract_target - 1) / 2^256`.
- Parallel collision search for the `1 / p` collisions needed costs about
  `2 * 2^80 / sqrt(p)` with modest memory (about 10 GB at
  `p = 6.5 * 10^-9`): an attainable estimate, not a lower bound.
- With `p` near 1 (age easing), that is about `2.4 * 10^24` HASH160 per
  stolen reward: `1.5 * 10^7` GPU-years at `5 * 10^9` per second per GPU,
  or 40 minutes for a hypothetical `10^21`-per-second ASIC fleet (none is
  known). At `p = 6.5 * 10^-9` (a win per `10^13` candidates):
  `1.9 * 10^11` GPU-years, or 1 year for that fleet.
- The prize is one reward, at most `floor(balance / 420,000)`, about
  `4.97 * 10^9` raw units in section 2's example; there is no PHOTON price
  to weigh it against.
- Matching a known winner is a second preimage (above `2^120`), since
  `HASH160(R)` is inside the hashed claim. A table of `M` precomputed
  scripts costs about `2^160 / (M * p)` per reward; honest multicollisions,
  RIPEMD-160 differentials and extra uncles give no cheaper route.
- Uncounted margins: the context digest sits early in `R` (byte 42), so
  honest candidates share no midstate (keep it early), and a search must
  finish within one job. A root waiting unfunded needs only
  second-preimage resistance and data availability.

P2SH20 is acceptable only with: a byte-exact canonical `R` and golden
vectors for `R` and `HASH160(R)`; credit deduplicated by claim digest; a
versioned profile with a tripwire, where a confirmed spend of a pool root by
any script but its canonical `R` moves later shares to P2SH32 roots; and a
measured P2SH32 fallback. A 60-second collision window was the earlier
argument and was rejected; security rests on seed binding and cost. The
final lean `R` still needs its own security review.

## 5. Minimum payouts with a lean direct split

### The design in plain words

The first design, a funded Merkle-sum tree, passed its phase 2a prototype
in libauth, but a 4,096-leaf win needs 0.06985170 BCH from outside,
4,098,190 satoshis of it fees (about 1,001 per payee), more than small
payments are worth ([`measurements.md`](../../p2poolv2_photon_payout/measurements.md)).
Paying whole wins in turn was rejected too: a small miner could wait years.

Instead miners accumulate token balances on the share chain from won
tokens, by the PPLNS window, and one lean transaction splits each funded
win. For the PHOTON pool only, miners are paid first come, first served in
the order their balances reached their minimum; paid in full, a miner
rejoins at the back, and clipped, keeps their place. Leftover reward is paid
ahead to the miners whose shares already in the window are expected to earn
most, never beyond that expected credit, so no tokens are held in custody,
even at launch. A remainder too small for the next minimum goes to the
entry before. There is no script cap, and the window is 48 hours. Every
payment meets its minimum and the whole reward is paid, with no token
change.

`C`, one payout's cost in raw tokens, is a share-chain parameter changed
only by the upgrade vote. Each miner's shares carry a multiple, at least 10,
default 20; the minimum gross payment is `multiple * C`. The submitter
fronts the BCH fee and dust and takes `C` tokens from each payment, so a
miner paid exactly the minimum gives up `1 / multiple`; each miner keeps the
BCH dust on their output. Payouts must never cost more BCH than the tokens
are worth, with no sponsor: a submitter decides whether `C` covers its BCH.
Unfunded, the tokens stay locked and the balance is kept.

### Exact ledger and selection rules

1. A miner is their authenticated payout script, aggregating its credit.
   The multiple is bound in their shares: omitted means 20, below 10 is
   invalid. Changes need a deterministic, authenticated activation rule
   (open); a third party cannot change another miner's minimum, and nothing
   rewrites a locked `R`.
2. Only the actual reward of a settled pool claim on the selected BCH
   history is credited, by the parent weights with telescoping integers:
   `floor(reward * upper / total) - floor(reward * lower / total)`. Burns
   and lost claims create no credit.
3. The ledger keeps signed balances (credit minus confirmed gross debits,
   negative while paid ahead), reservations and confirmed debits apart. A
   payee receives `gross - C`, the submitter `C`. Unfunded payments are not
   debits. `total won = confirmed gross debits + sum of signed balances`,
   with reorg-aware rollback. Available balance is signed balance minus
   reservations.
4. From the parent snapshot, taken before the tail search: a miner is queued
   from the first update where their available balance reaches
   `multiple * C` (ties by ascending raw locking bytecode) and leaves when
   it drops below; paid in full, they requeue at the back. A queued entry's
   cap is its available balance. Any other miner is offered at cap
   `available + E` if that reaches its minimum, largest first, ties by
   bytecode. `E`, the expected credit of its window shares, is
   `q * reward * (H(W-1) - H(L-1) + (j+1)/W)` per share at position `j`
   (oldest 0) of a window holding `L` of `W` shares, with `q` the chance a
   share is also a claim and `H` the harmonic number (exact integers in a
   deployed profile). The list takes as many queued entries as still let at
   most `N` entries cover the largest possible reward, then offers. An entry
   is followed by another only if the smallest possible reward leaves the
   next its minimum; otherwise its cap grows to cover the largest reward
   (by less than one further minimum plus the tail span) and the list ends.
   If no list fits, no job is built. Scripts, caps and multiples go into
   `R` in order; the share chain validates the order.
5. With input tokens `A` and `remaining = A`, the covenant visits entries
   while `remaining > 0`: `gross = min(cap, remaining)`, requires
   `gross >= multiple * C`, and emits `gross - C` tokens to the script; with
   `C > 0` every amount is positive. It requires `remaining == 0` and
   `1 <= paid_count <= N`. Unused entries get no output.
6. One submitter output with `paid_count * C` tokens, to a script of its
   choice, follows the payees, then optionally one token-free BCH change
   output. No other token output, burn, NFT or category change.
7. Input 0 is the only payout input, followed by one to four funding inputs
   whose `OP_UTXOTOKENCATEGORY` equals the empty vector (a zero amount is
   not enough: an immutable NFT with an empty commitment has a category)
   [S1, S7]. Version 2, locktime 0, final sequences. Each token output gets
   its dust minimum; inherited payout BCH stays among the token outputs;
   funders pay the fee and missing dust. No owner key or privileged
   executor.
8. Every live win's allocations are reserved before another list is
   selected; siblings reserve only on the selected branch. Winning `R`,
   outpoints and allocations outlive share pruning. Refused funding keeps
   the reservation; confirmation debits once; reorgs undo dependent credits,
   reservations and debits. Durable replay rules are an integration gate.

### Coverage conditions and open contradictions

With caps `cap_i` and prefix sums `S_j = sum(cap_i for i < j)`, a reward
`A` is payable exactly when some `j <= N` has `S_j < A <= S_j + cap_j` and
`A - S_j >= multiple_j * C`. A list summing to at least `A` is not enough;
the list walk builds lists that satisfy this for every tail.

- **Tail gaps.** The tail moves `A` by up to 65,535 raw units; the leftover
  rule covers it. Later earnings or a new `C` cannot repair an old `R`.
- **Coverage within `N`.** Smaller `N`, a shorter window or many more
  miners can bring blocked wins back.
- **Advances.** Pool luck can leave a shortfall; a miner who leaves while
  paid ahead keeps at most 0.04 mean rewards per 48-hour-window run.
- **Reservations.** A win's own credit is unknown until its tail is found.
  The backing identity does not make every stuck old root payable.
- **Value.** `C` is a token count, not proof of BCH value; payouts stop
  while reimbursement is unattractive, and one `C` cannot match every
  transaction's fee and dust.

### Lean direct split measurements

The prototype's lean `R` is an 82-byte header (version, network, category,
context from byte 42, `C`, entry count), an altstack sentinel, one record
per payee and a fixed program. Version 1 (885 program bytes) checked that
debts were listed most-owed first, which adds no security since the share
chain validates `R` anyway. Version 2 (818 bytes) pays in `R`'s order.

Measured in libauth `be3ed16396c9eb9c0e57343474d36079a37c8dae` (BCH 2026,
both modes) with signed transactions: debt `20 * C`, `C = 2^32`, one P2PKH
funder, 700 inherited satoshis, P2SH20 root, 1 satoshi per byte, nine-byte
dust minima. BCHN v29.2.0 limits apply [S4, S5]:
10,000-byte unlocking bytecode and stack items, operation cost
`800 * (41 + L)`, standard hash iterations `floor((41 + L) / 2)`,
100,000-byte transactions.

| Payee scripts | Largest N, v1 | Largest N, v2 | v2 fee per payee, sats | v2 limit |
|---|---:|---:|---:|---|
| 35-byte P2SH32 | 187 | 188 | 141.585 | Unlocking size |
| 25-byte P2PKH | 99 | 238 | 120.202 | Unlocking size |
| 23-byte P2SH20 | 83 | 198 | 117.253 | Operation cost |
| Densest mix | 207 | 244 | 117.844 | Operation cost |

| v2, 35-byte payees: N | 8 | 64 | 188 |
|---|---:|---:|---:|
| Fee per payee, sats | 289.750 | 154.344 | 141.585 |
| BCH fronted (fee plus dust), sats | 8,633 | 55,673 | 159,833 |

Lists of 64 or more cost 117 to 154 satoshis of fee per payee.
`cargo photon-lean-measure --share-order` reproduces
[`lean-share-order-measurements.md`](../../p2poolv2_photon_payout/lean-share-order-measurements.md);
version 1 and capacity probes:
[`lean-measurements.md`](../../p2poolv2_photon_payout/lean-measurements.md).

### Simulation findings: the earlier rules did not deliver short waits

`cargo photon-payout-sim --summary` runs the first written rules (most owed
first, no pay-ahead, a 4,096-script cap, launch custody, 12-hour window):
1,000 and 10,000 miners in three mixes (one giant with 70 percent,
log-uniform over eight decades, heavy-tailed), multiples 10 to 50, churn,
delayed funding, `N = 64`, 5,000 wins. Small miners were never paid. A
probe whose wait `multiple * C / earning rate` is 1.15 days waited 15 to 77
days or forever.

| Mix | Miners | Payouts per won reward | Eligible miners beyond `N`, mean |
|---|---:|---:|---:|
| Giant 70 percent | 1,000 | 1.008 | 334 |
| Log-uniform | 1,000 | 1.924 | 398 |
| Heavy-tailed | 10,000 | 13.010 | 6,719 |

Causes: most-owed order put old large debts first; empty balances cannot
pay a first win, so the runs assumed a 100-reward launch backlog in
custody, which never shrank; and small miners rarely had a share in the
4,320-share window, while the cap dropped miners beyond 4,096. Tables:
[`simulation-summary.md`](../../p2poolv2_photon_payout/simulation-summary.md).

### Candidate rules re-run: first come, first served with bounded pay-ahead

`cargo photon-payout-sim --candidate-summary` repeats those runs under the
adopted rules, comparing share rates and windows. The probe has 0.0001 of
the pool's hash and multiple 20; its threshold wait is 1.15 days. A blocked
win has no valid list.

| Miners | N | Shares | Window | Blocked wins of 5,000, giant / log-uniform / heavy | Probe mean interval, days |
|---:|---:|---:|---:|---|---|
| 1,000 | 64, 187 | 10 s | 12 h | 0 / 0 / 0 | 0.99 to 1.16 |
| 10,000 | 64 | 10 s | 12 h | 80 / 4,927 / 4,980 | 1.538 / censored / censored |
| 10,000 | 187 | 10 s | 12 h | 57 / 380 / 1,680 | 1.538 / 1.477 / 2.047 |
| 10,000 | 187 | 1 s | 12 h | 3,280 / 4,094 / 4,984 | 2.400 / 4.651 / censored |
| 10,000 | 187 | 10 s | 48 h | 0 / 0 / 0 | 1.216 / 0.984 / 1.241 |

- At `N = 64` the log-uniform and heavy-tailed blocks are all for want of
  payees. At `N = 187` with a 12-hour window, balances below their minimum
  outgrow what bounded pay-ahead covers. 1-second shares cut each share's
  credit tenfold and block more, though fewer small miners find no share
  (giant mix: 148 instead of 1,022).
- The 48-hour window blocks nothing, and the probe's wait matches its
  formula whatever the miner count. Unrecovered advances peak at 11 to 20
  mean rewards; absorbed leftovers total at most 7.9 rewards of 5,000.
- A win pays 44 to 169 miners on average (1 to 13 before), at 138 to 159
  satoshis of fee per payee.
- At 10-second shares a miner below about one ten-millionth of the pool
  almost never finds a share: 2,405 of 2,460 tiny miners in the giant mix
  found none in about 139 days.

Tables: [`candidate-summary.md`](../../p2poolv2_photon_payout/candidate-summary.md).

### Dust, fees and liveness

Default dust is `3 * (serialized_output_bytes + 148)` satoshis, CashTokens
prefix included [S1, S3]: with a 9-byte token amount 675 (P2PKH), 669
(P2SH20) and 705 (P2SH32); with 5 bytes 663, 657 and 693; token-free
change 546. The covenant
commits these largest-prefix minima; funders pay
`max(local_dust_threshold, committed_minimum)`. It enforces inherited BCH
only in aggregate, so an executor may spread the excess among payees
arbitrarily; that rule is an open decision.

Funders sign complete transactions (`SIGHASH_ALL | SIGHASH_FORKID`, with
`SIGHASH_UTXOS` where supported) and may cooperate. Nodes never sign
unknown inputs or offer a public wallet. Unfunded tokens wait under their
`R` with no expiry. Winning `R`, lists and proofs are replicated, kept
beyond pruning and exportable; a share's data must be available before it
earns credit.

## 6. Wins, the baton race and transaction malleability

Every claim spends the current baton, so wins conflict and BCH inclusion
settles them; the pool cannot reserve the baton. Claims can chain at age 0
and land in one block [P1, P4]. A validated single-input claim successor
cancels old jobs at once. Anyone holding a win's bytes can broadcast it;
nodes relay wins ahead of shares and track submission, mempool acceptance
and confirmation separately.

The real locktime is malleable because the proof assumes 0 [P1]: a third
party can rebroadcast with another final locktime, delaying but not
redirecting the win. Pool shares use locktime 0; tracking still recognizes
valid variants. Conflicts are tracked by spent baton, then claim digest;
one baton can have several winning proofs, each with its own list. A
payout spend names the accepted transaction's real output, and a
share-chain reorg cannot rewrite a locked payout. Each confirmed oracle
move costs at least 8,000 satoshis plus fees and resets the age origin,
suppressing age easing for solo and pool miners alike [P1].

## 7. Sister node and reuse plan

A new `p2poolv2_photon` binary reuses, behind small extracted interfaces,
the Noise transport, bounded worker queues, RocksDB store and chain-index
algorithms, integer ASERT and the PPLNS window cache [B1-B9]. It needs its
own share header, serializer, proof validator, messages, genesis and
database, strict-target work, the ledger, the lean split and a submitter;
`ShareHeader.bitcoin_header`, `Emission`, `NodeActor` and the writer are
concrete BCH types. The BCH payout tree (tag
`payout-hybrid-t3072-f500-r1to3-z8-cov1`) pays BCH and rejects token outputs
[B3-B6]; the BCH-difficulty window, 100-share payout skip and coinbase
maturity are not PHOTON rules. Tree-era estimates were 20 to 40
engineer-weeks plus Pickaxe work and audit; the lean path has no estimate.

## 8. Miner protocol and Pickaxe work

A versioned PHOTON extension over Stratum V2's Noise channels [P5], with its
own message identifiers and byte vectors, carries:

- **Open channel**: chain, worker, payout script, multiple, layouts and
  covenant versions; no key or wallet seed.
- **Job**: baton anchor, lineage and path delta, parent and uncles, weights,
  ledger digest and payee list, `C`, the `R` constructor, age, amounts,
  layout and the contract, chain and vardiff targets.
- **Set target**: about one submission per 5 seconds, never harder than the
  chain target.
- **Submit**: key, nonce, datasig and tail; replies name the result or
  reason. Claimed hashrate earns nothing.
- **Cancel and fetch**: cancellation when the baton moves, new work on tip
  changes, parent and list fetch, win announcements and list export.

Share mode compares at the easier of the statistical and contract targets
[P8]. T2 keeps only `min(total_winners, winner_cap)` hits [P9], so when
`total_winners > winner_cap` the batch is rescanned at the contract target,
subdivided until no win is lost. Pickaxe needs P2SH20 age layouts, the
`HASH160(R)` seed path and tail-profile vectors in Rust, CUDA, HIP and wgpu.
Remote job sources are pinned by Noise identity, and the client shows the
category, chain, payout script and fee policy it verified. A donation or
operator fee, if approved, is a per-share credit policy, not a cut of the
finder's whole window.

## 9. Attacks and limitations

| Attack | Answer and remaining limitation |
|---|---|
| Withholding wins | Nodes rebuild every share and broadcast wins; a miner can still discard candidates before submitting. |
| P2SH20 collision | One reward per pair, about `2.4 * 10^24` HASH160 at best (an estimate); a public collision switches roots to P2SH32. |
| Oracle griefing | Confirmed moves cost at least 8,000 satoshis and reset age easing; unconfirmed ones move no jobs or credit, but a pool win can lose to a pending one. |
| Old baton after an oracle move | Shares and uncles on `V` are invalid once a context holds confirmed `O`. Conflicting claims stay a risk until BCH settles them; a one-share race after BCH block propagation is open. |
| Many small scripts | Each script earns its own work, but many balances below their minimum can leave a win with no list within `N`. |
| Share spam, forged state | Size bounds, per-peer budgets, cheap checks first, authenticated baton sources. |
| Selfish mining, stale parents | Cumulative-work fork choice, full seed binding, unique proof credit, depth-bounded uncles; fork incentives need simulation. |
| Token theft or burn in a split | Only input 0 holds tokens; the covenant conserves the reward exactly. |
| Funding fee griefing | Inherited BCH cannot go to fees or change; funders pay all fees; competing funded spends can delay each other. |
| Nobody funds a split | Tokens wait in `R`, balances stay owed; a blocked `R` is not repaired later. |
| Missing data, eclipse | Data before credit, replicated lists, independent peers. |
| Reorgs, locktime variants | Conflict sets by baton and claim digest; RPC acceptance is not confirmation. |

Section 2 covers redirecting a win and the schedule carry; section 3
unconfirmed-oracle credit capture (to be simulated), BCH orphans, work
overflow and uncle replay; section 8 result-buffer overflow. Inherited BCH
excess is an open decision.

## 10. Open decisions and phases

Adopted, for the PHOTON pool only: minimum payouts on a balance ledger
with the lean split; first come, first served; pay-ahead bounded by expected
credit, with no launch custody; no script cap; a 48-hour window; lean
covenant version 2.

### Open decisions

| Decision | Options and what is known |
|---|---|
| Build the pool | Owner's call; nothing below is authorized. |
| Payee cap `N` | 187 studied; version 2 fits 188 to 244 by script mix. |
| `C` | Initial raw-token value; how often it may change; the vote threshold. A week-averaged Cauldron price is a later option only. |
| Miners who leave below their minimum | Pay when a split has room, or expire after a long time. |
| Miners who leave paid ahead | No policy; at most 0.04 mean rewards unrecovered per run. |
| Share rate | Miners below about one ten-millionth of the pool find no shares at 10 seconds; faster shares add load and, with a 12-hour window, block wins. |
| Multiple changes | A deterministic, authenticated activation rule. |
| P2SH20 or P2SH32 | P2SH20 under section 4's conditions after review of the final `R`, weighing one reward against the cost estimate; or P2SH32 measured under both tail profiles. |
| Tail profile | Conserved, or burning (about 1.95 times the candidate rate, unmeasured; at most 65,535 raw units burned per win). |
| Age range | Through 32,767 BCH blocks, or through 65,534 with new layouts and vectors. |
| Inherited BCH excess | Aggregate-only (prototype default) or a per-payee rule in a revised covenant. |
| Other share parameters | Uncle depth policy, restart after an outage beyond 256 transitions, context lag, per-share fees, a window tied to claim work. |
| Coverage, reservations, no sponsor | Section 5's coverage list must be resolved before share-chain work. |

### Gate status

- **Payout covenant prototype: open** for isolated test work. Phase 2a
  passes in libauth; BCHN acceptance, phase 2b, GPU seed hashing and a
  security review of the final lean `R` remain.
- **Share-chain and node work: closed** until an independent review accepts
  section 3's oracle and window rules and the final `R`. Any chipnet or
  mainnet spending needs its own authorization.

### Phases

1. **Design review** of sections 3 and 4, the final lean `R`, private forks,
   context and sync rules, funding isolation and the BCH excess rule.
2. **Covenant feasibility.** Phase 2a done in libauth-rs `be3ed16`, as
   `p2poolv2_safa` uses. Phase 2b, not yet authorized, uses the unmodified
   v3.2 covenant: P2SH20 claims at ages 0, 17, 128 and 32,767; locktime
   variants; mutations of `R`, key, nonce, datasig, age, sequence and
   values; golden vectors for `R` and `HASH160(R)`; its cost on CPU and GPU;
   BCHN regtest `testmempoolaccept`. The test VM in
   `p2poolv2_lib/src/test_utils/bch_vm.rs` lacks `OP_DEFINE`, `OP_INVOKE`
   and numbers above 16 bytes.
3. **Protocol and simulation.** Golden vectors; simulate latency, hash
   changes, partitions, private forks, withholding, oracle moves, BCH
   orphans, long outages and startup; then freeze a profile and tag.
4. **Sister node and independent clients.** Another client must rebuild a
   win and payout from the public specification.
5. **Chipnet, then mainnet** after the audit, with the owner's approval.

## Sources

Pickaxe files at [revision 2f7cbb0](https://github.com/CyberAshven/pickaxe-miner/tree/2f7cbb0493f2a46ef89d4f04c941074311a09ba0):

- **P1:** `reference/photon_v32_template.json`, `scripts.lock.script`.
- **P2:** `reference/photon_v32_redeem.hex`.
- **P3:** `src/protocol.rs`: `PhotonDeployment`, `ProofRule::Positive`, `derive_photon_state`.
- **P4:** `docs/protocol-profile.md`: layouts and ages.
- **P5:** `docs/architecture.md`: mining destinations and GPU engines.
- **P6:** `src/reward.rs`: `TOKEN_OUTPUT_SATS`, retired v0.
- **P7:** `src/tx.rs`: `PhotonLayout`, T2 amount updates, encoding guards.
- **P8:** `rust-engine/src/t2.rs`, `cuda/photon_t2_tail.cu`.
- **P9:** `src/search.rs` (`rotate_search_identity`), `src/cuda_t2.rs` (`T2Batch::total_winners`).
- **P10:** `src/crypto.rs`: `bch_schnorr_sign`, `bch_schnorr_sign_search_candidate`.

This repository at [commit 044391e](https://github.com/CyberAshven/BitcoinCash-p2poolv2/tree/044391e3966539f26d6b471d64334cbe600f602b), paths under `p2poolv2_lib/src/`:

- **B1:** [ASERT](../difficulty_adjustment/asert.md), `pool_difficulty.rs`.
- **B2:** [Payout distribution](payout-distribution.md), `accounting/payout/sharechain_pplns/pplns_window.rs`.
- **B3 to B6:** [Hybrid BCH payouts](bch-hybrid-payouts.md), `shares/bch_payout_covenant.rs`, `accounting/payout/payout_tree.rs`, `node/payout_tree_executor.rs`, `store/payout_tree.rs`.
- **B7:** `shares/share_block/mod.rs`, `node/messages.rs`, `stratum/emission.rs`.
- **B8:** [Share pipeline](share-processing-pipeline.md), [async flow](async-flow.md), [store schema](store-schema.md), [store architecture](store-architecture.md), [pruning](pruning.md), [BCH anchor rules](mine-on-bitcoin-chain.md).
- **B9:** `node/actor.rs`, `node/mod.rs`, `node/behaviour`, `node/connection_tracker.rs`, `store/writer/mod.rs`, `store/organise`, `shares/chain/chain_store_handle.rs`.

Bitcoin Cash:

- **S1:** [CashTokens specification](https://cashtokens.org/docs/spec/chip/).
- **S2:** [BCHN P2SH security bulletin](https://bitcoincashnode.org/en/newsroom/bchn-technical-bulletin-2022-06-09): collision versus second preimage.
- **S3:** [BCHN policy](https://github.com/bitcoin-cash-node/bitcoin-cash-node/blob/master/src/policy/policy.cpp), `GetDustThreshold`; deployment rechecks the node's policy.
- **S4:** [BCHN v29.2.0 VM limits](https://github.com/bitcoin-cash-node/bitcoin-cash-node/blob/v29.2.0/src/script/vm_limits.h).
- **S5:** [BCHN v29.2.0 standardness](https://github.com/bitcoin-cash-node/bitcoin-cash-node/blob/v29.2.0/src/policy/policy.cpp): 10,000-byte unlocking bytecode after the May 2026 upgrade.
- **S6:** [BCHN v29.2.0 token validation](https://github.com/bitcoin-cash-node/bitcoin-cash-node/blob/v29.2.0/src/consensus/tokens.cpp), `CheckTxTokens`: outputs above inputs fail with `bad-txns-token-in-belowout`.
- **S7:** [BCHN v29.2.0 interpreter](https://github.com/bitcoin-cash-node/bitcoin-cash-node/blob/v29.2.0/src/script/interpreter.cpp): `OP_UTXOTOKENCATEGORY`, `OP_ACTIVEBYTECODE`.
- **S8:** van Oorschot and Wiener, [Parallel Collision Search with Cryptanalytic Applications](https://doi.org/10.1007/PL00003816), Journal of Cryptology, 1999.
