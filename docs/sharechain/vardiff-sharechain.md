# Vardiff share chain

Status: proposed (not implemented).

This document specifies the **vardiff share chain**, an opt-in share chain
selected by `--vardiff-sharechain` (default off) or the configuration key
`bch_share_chain = "vardiff"`. It replaces the ASERT equality pin with a
validation band `[floor, infinity)`, derives that floor from the miners
actually on the chain (no hand-configured difficulty), and adjusts it
dynamically against spam. The goal is the outline's: small miners hold shares
in the PPLNS window at any pool hashrate, payouts stay work-proportional, and
the floor is consensus-bound rather than a per-node setting. Vardiff is a
separate share chain -- its own genesis, protocol tag, and store -- and main,
mini, and Bitcoin are untouched while the flag is off.

## 1. Why a separate chain

A function-derived floor varies while the chain runs, so no protocol tag can
pin it, and changing an existing chain's validation would fork every operator
who does not opt in. The outline's constraints therefore force one shape:

- **New chain, not a mode.** `BchShareChain::Vardiff` gets its own anchor
  target, so its genesis hash differs from main's and mini's and the chains
  never reorg into each other (the same isolation main and mini already have,
  `bch_genesis.rs`).
- **Own protocol tag.** The rule tag gets the `vardiff` segment instead of
  `floor-<value>`, so vardiff nodes peer only with vardiff nodes. Main and
  mini protocol strings stay byte-identical to their pre-vardiff form (no
  `vardiff` segment).
- **Own store.** Store isolation stays the operator's convention, as it
  already is for main and mini: one store per share chain. The genesis guard
  in `init_or_setup_genesis` turns a wrong store into a clear error
  (`GenesisMismatch`) instead of two chains in one database.
- **Flag precedence.** The runtime flag wins over configuration and logs a
  notice, so a node started with `--vardiff-sharechain` on a config that says
  `main` joins vardiff. The flag exists for operators and the live chipnet
  test (V4); the config key exists for deployments (Umbrel, Docker) that
  cannot pass arguments. Default off, both.

The existing band code (`share_chain_rule_tag(chain, floor)` and
`share_chain_segment(chain, floor, genesis)`) already threads a floor
argument through protocol construction; V1 reuses that seam, extending the
builder with the share-chain variant (section 5), rather than adding a
parallel path.

## 2. Build increments

V1, V2, and V3 are build increments of the one vardiff share chain: each is a
development step that keeps the work reviewable and testable, and none is a
separate chain. The chain is one thing throughout -- one genesis, one
`vardiff` protocol tag, one store -- and a live network runs one completed
build, so incremental builds are never mixed across nodes of a live chain.
The flag exists from the first increment so isolation can be exercised; the
feature is complete when V3 lands.

| Increment | Build scope | Validation rule after it lands |
|---|---|---|
| **V1** | Variant, genesis, tag, flag | Exact ASERT (the equality pin) |
| **V2** | Function-derived floor, per-miner tier, `sharechain_tier_mode` deleted | Validation band, floor derived |
| **V3** | Dynamic anti-spam floor on top of V2's mechanism | Validation band, floor state machine |

### V1: chain, flag, and tag; validation unchanged

V1 is deliberately small: it proves isolation and the flag before any
consensus arithmetic changes. The share chain's validation rule **stays the
equality pin** (`share_header.bits == calculate_target_clamped(...)` at the
three sites: full validation `validate_with_pool_difficulty`, header sync
`p2p_message_handlers/receivers/share_headers.rs`, and block receipt
`p2p_message_handlers/receivers/block_receiver.rs`), and jobs carry
the pool-wide `prepared.bits` exactly as on main today.

- `BchShareChain::Vardiff` is added to the config enum, with serde name
  `vardiff` and the CLI flag `--vardiff-sharechain` overriding it (notice
  logged). The existing validation that rejects `mini` on `Chain::Bitcoin`
  is extended to reject `vardiff` the same way: a vardiff chain is a
  Bitcoin Cash share chain, and every `BchShareChain` match must handle the
  new arm.
- A new anchor constant gives the vardiff chain its own genesis. The value is
  chosen so the genesis hash differs from both existing chains (the
  isolation's whole point) and so the derived floor (V2) starts from a
  useful point; see section 4.
- The rule tag gains the `vardiff` segment in place of `floor-<value>`; the
  validator is constructed with `sharechain_diff_floor: None`, which the
  band code treats as the equality pin (the `None` arm lives at each of the
  three band call sites; `check_sharechain_band` itself always takes a
  floor). No validation change.
- The static `sharechain_diff_floor` key and its
  `DEFAULT_SHARECHAIN_DIFF_FLOOR*` constants are **removed** (config, samples,
  docs), because a configured floor is exactly what the outline forbids
  (section 3); that purge returns main/mini to the equality pin and removes
  their `floor-<value>` tag segment. `sharechain_tier_mode` stays `off` in
  V1; V2 deletes the switch (below). The validator's floor field, its
  builder `with_sharechain_diff_floor`, the tier computation, and the
  `floor-<value>` tag formatter stay dormant in the tree: they are V2's
  enforcement points, and V1 extends the rule-tag builder with the `vardiff`
  arm beside the floor segment.

V1 exists as its own commit so the isolation and flag plumbing is reviewable
without consensus arithmetic; a local same-build run exercises it before the
floor arithmetic lands.

### V2: function-derived floor and per-miner tiers

V2 turns the band on with a floor computed from the chain's own traffic. The
floor and the band are **integer-only and deterministic**: every node sees
the same shares (the share chain is the transport), so a floor defined over
accepted chain work is a consensus rule, not a node-local estimate.
Floating-point or wall-clock-derived values are forbidden in anything a peer
can disagree about; the tier is node-local and is the one exception (below).

The floor itself is one clean rule, straight from outline line 16 ("the
smallest miner sets the diff floor which is consensus bound"). Over a
trailing window of accepted shares, every share's declared difficulty `d` and
payout identity are already recorded; a miner's window work is the integer
sum of its shares' `d`. Define

```text
W_total = sum of d over all shares in the window
W_min   = min over miners with at least N shares in the window of
          (sum of d over that miner's shares in the window)
floor   = max(D_ceiling, W_min * D_asert_capped / W_total)
```

`floor` is the effective floor as this section reasons about it; the u64 the
band receives is `W_min * D_asert_capped / W_total`, computed with round-up
(never-easier), and the chain ceiling joins it as a separate target-space
input (conventions below). `N` is a small code-local constant (two or three
shares); a miner with fewer than N shares does not enter `W_min` (it still
adds to `W_total`), so a drive-by identity cannot set the floor. The
newcomer paragraph below explains what the presence rule buys.

The ratio is span-free: every miner's window work is `m_i * span / 2^32`, so
`W_min / W_total` is exactly the smallest eligible miner's hashrate share and
the window's length cancels. Multiplying by `D_asert_capped` (the parent
chain's ASERT difficulty, the same value the equality pin computes) turns
that share into a difficulty: the floor is the smallest eligible miner's own
fair tier under the tier formula below.

The identity key is the share's own `miner_bitcoin_address` (the payout
address its header commits to), never the share owner
`miner_address`: the owner varies with transfers and is not the entity the
payout window pays. The sum counts declared difficulty once per earner, the
same way the window walk counts work: each share's own `d` to its miner, each
uncle's `d` to the uncle's own `miner_bitcoin_address` (an uncle's coinbase
output can never be spent, so that key never moves). The nephew bonus a share
earns for including an uncle is a payout *weight*, not window work, and does
not enter `W_min`. This is a deliberately lighter key than the payout
window's `share_payee`, which resolves transferred payout rights: the floor
is a rate limit on *mining identities* and stays computable from share
headers alone, while the payout window continues to credit whoever holds the
right.

The window is **share-chain-local, not the payout PPLNS window**: a
fixed-count window of the trailing `PPLNS_WINDOW_SHARES` (120,960) share
entries, the fixed-count sizing Bitcoin's payout window uses, walked from the
share's parent by the same composition rules the payout walk uses (candidate
headers above the confirmed start, the `BCH_COINBASE_MATURITY` skip). An
uncle is not an entry and takes no window slot; its `d` still enters the
sums, credited to the uncle's own miner. Uncle headers are materialized for
the walk the way the payout walk materializes them, one batch lookup over the
window's uncle hashes, but the floor walk uses the opposite failure rule: a
lookup miss refuses the share (below) instead of skipping the uncle. The
payout window's own sizing cannot be reused here: it sizes itself from the
*paying block's* difficulty
(`WindowSizing::BlockWork`, `PayingBlock`), and no paying block exists when a
share is validated. A share-local work budget has no such anchor -- it would
need Bitcoin Cash difficulty, which header sync does not have, or scale with
the chain's own dynamic difficulty and ratchet. A fixed count reuses the
existing model and constant and keeps the rule a pure function of share-chain
state: no Bitcoin Cash header, no paying block, no miner-chosen bits (a
`claimed_difficulty` rule would let a spammer shrink the window), and the same
walk is computable at all three band sites. At full validation and block
receipt the parent chain is in the store. At header sync, where the validated
header's ancestors are in the batch and not yet stored, the walk runs over
the batch's already-validated headers and the store's confirmed entries
together, mirroring how the batch-local `asert_parent_cache` already serves
ASERT there; share headers are validated in batches of parent-before-child
order, so every ancestor is one or the other. Uncles join the same two
sources: a batch header's declared uncles were required known when the header
was validated, and a stored entry's uncles are in the store, so the lookup
resolves across the batch-store seam whenever the batch itself is sound; a
miss refuses rather than producing a partial sum.

Composition determinism: the floor window is one sequence of parent-linked
ancestors, and the seam between batch and store is not part of the rule.
Batch ancestors join the stored confirmed entries below the batch's oldest
ancestor; a batch header counts exactly like a stored one, ordered and
heighted by chain position. Two nodes that hold the same batch and the same
chain below it compose the same window, whichever of them has organized
(confirmed) more of that chain: the maturity skip counts shares down from the
share's parent by height (`min(maturity - 1, p - maturity)`, `p` the
validated share's own height -- the payout walk's paying height, one above
the window's anchor entry -- and `maturity` the chain's
`BCH_COINBASE_MATURITY`), which is chain shape, not any node's Bitcoin Cash
confirmation state. A node whose store cannot reach deep enough to complete
the window refuses the share before any floor is judged (above); no node
judges on a partial window. The V2 gate pins the equality: composing the
window from a validated batch and the stored chain below it derives the same
`W_min` as walking the same sequence once the batch is stored.

`W_min` and `W_total` are exact integer sums of integers, and the ratio
`W_min / W_total` is the smallest eligible miner's exact hashrate share:
every node that has the same window computes the same floor, with no floats,
no clocks, and no local state. The rule is the outline's line 16 in
arithmetic: the smallest miner's fair tier becomes the floor. The count
window's span feedback -- the defect that makes a raw window-work floor
untenable -- is cancelled here, because every miner's window work is
`m_i * span / 2^32` and the span divides out of the ratio: the floor is
`w_min * D_asert_capped` in ratio form. The floor's level still follows
window presence: which identities are eligible, how many entries each holds,
and their weights are all functions of the chain's emission history, so a
population change moves the floor with window-length latency -- the
equilibrium is approached as the window ages and is stable, not
instantaneous. The span-feedback path exists only for the payout window,
which this rule does not use for the floor.

The result on the chain is the outline's line 24 target in arithmetic: with
every tier at `w_i * D_asert_capped`, ASERT regulates total emission to one
share per 10 seconds, so each of the `k` miners on the chain emits at the
pool rate divided by `k` and holds `120,960 / k` entries in the floor window
-- every miner has shares in the window at all times for any `k` below the
cap: window counts pin participation, weights pay for work. A newcomer
smaller than the current `w_min` mines at the floor until it holds enough
shares (the presence minimum `N`) in the window to lower `w_min` to its own
share, exactly the `>= floor` entry the next paragraph describes. The
outline's line 14 shape holds at the share level: every tier is
`w_i * D_asert_capped`, so a small miner mines lighter shares and a large
miner heavier ones, while every miner emits at the same per-miner rate and
the window's *weights* -- and so payouts -- stay proportional to work. (The
miner-comparison model's figures -- 2, 201, and 2,012 shares per day at
1 TH/s, 100 TH/s, and 1 PH/s -- belong to its static 10M floor, not to this
rule.) When `w_min * D_asert_capped` sits below the chain ceiling the floor
stays at the ceiling: the chain is too small for the rule to ask for
anything easier.

Two cases are distinct and never overlap. (1) **Young chain**: the walk
reaches the chain start with fewer than the cap's post-maturity entries (or
none). That is a normal short window, never a refusal: the floor is
`max(D_ceiling, W_min)` over the entries that exist, and with no eligible
entry it is `D_ceiling` -- the band binds at the chain ceiling until shares
accumulate. The `reaches_chain_start` check is the discriminator between this
case and the next. (2) **Pruned boundary**: the walk runs out of cached
entries without reaching the chain start, so the oldest retained entry's
height depends on this node's own tip
(`WindowError::InsufficientEntries`). No retry can bring the missing entries
back, so this refuses, fail closed -- the same treatment
`validate_bitcoin_payout` gives the error -- rather than judging against a
guessed floor. Retained window history is therefore a stated requirement of
running a vardiff node, the same implicit requirement the payout walk already
carries: a node must retain at least the floor window (and its uncles) below
its tip to validate shares. An uncle header this node no longer retains
refuses the share the same way at any later band site, block receipt
included; the determinism detail below gives it the unresolvable
classification.

Determinism detail: the floor used to validate a given share is derived from
the window ending at **that share's parent**, not at the validating node's
tip. Two nodes that have the same parent chain and the same window entries
compute the same floor for the same share, and a share at exactly the floor
is accepted or rejected by all of them alike; a node's own tip window is used
only for its *own* job bits, which are node-local. The guarantee is
determinism under equal window composition, a wider dependency than ASERT
(which on this chain reads only the chain's epoch and the parent's height
and time) but not a wider failure envelope, because the floor walk fails
closed where the payout walk skips.
Uncle headers for every window entry -- the entries' own uncles, not only the
validated share's -- must be stored, or present and already validated in the
sync batch, before the share is judged. The payout walk skips a missing uncle
header with a warning; the floor is consensus and cannot: a miss is missing
window data, so the share is refused without a verdict -- recoverable when
the uncle can still arrive, unresolvable when this node no longer retains it
(eviction) -- never judged against a partial sum. Identities with no shares in
the window are not considered,
and a newcomer cannot lower `W_min`: its first accepted share counts at the
current floor, so it enters at `>= floor`; and its share does not enter
`W_min` until the identity holds the presence minimum `N` of them. A newcomer
therefore needs `N` accepted shares -- each at the floor in force when it was
mined -- before it can start moving `W_min` toward its own weight and the
floor down toward its own fair tier. The presence minimum is what makes
lowering the floor a deliberate cost rather than a drive-by act; the next
paragraph spells the attack out.

The presence rule earns its keep when the floor is derived from a *ratio*
rather than a raw sum: a sub-`N` identity still adds to `W_total`, so it
dilutes the ratio slightly, but it can never be `W_min` and can never be the
argument that pulls the floor down. The threat it guards against is the
drive-by identity: without presence, a fresh identity could register a single
negligible-hashrate share at the floor, then stop mining and coast while
honest miners age out of the trailing window; `W_min` would bind at the one
stale share's `d`, and the floor would slide toward `D_ceiling`. With
presence, the candidate must hold `N` or more shares that met a floor the
chain itself accepted, so moving the floor down is an episode, not a
drive-by: a one-time `N`-share buy holds the floor for up to one
window-length, and sustained pinning costs `N` shares per window-length.
The floor still follows the smallest work share among present identities: an
attacker who pays that `N`-share cost is indistinguishable from an honest
miner of that size, and the floor settles at the work share they bought. What
the coast attack loses by paying is the cheap version of the trick; the
residual is a bought-work pin holding `W_min` down plus spam that dilutes
`W_total`, both priced at floor difficulty and bounded by the ceiling clamp
(below); the dilution variant is V3's domain. The ceiling clamp bounds the
floor's fall: `max(D_ceiling, ...)` on the vardiff chain uses the vardiff
anchor's own ceiling (section 4), so a fully collapsed floor admits only
floor-difficulty spam -- it cannot lower any present miner's own tier below
its fair `w_i * D_asert_capped`.

On top of the floor, every miner gets a tier:
`D_asert` is the pool ASERT difficulty (that of the job target
`calculate_target_clamped` returns), and on the vardiff chain that target is
the epoch-anchor rule: a closed form from the chain's epoch (its first share
at height 1) over the parent's height and time, so neither the committed
tier bits nor a miner's weight can feed back into `D_asert`. The epoch
replaces genesis as the anchor because the share genesis predates the first
share by months on a fresh chain: framed at genesis, every on-schedule
interval leaves the debt unchanged and the chain clamps at the easy anchor
forever (the run-3 stall), while the epoch framing starts the schedule at
the first share and escapes immediately. Anchoring at the parent instead
would re-anchor the pool ASERT on `round(w_i * D_asert_capped)` every share,
which quantizes away ASERT's sub-integer growth and pins difficulty at an
integer (the dead-zone pin the live run proved), and would drag the anchor
toward a minority miner's `w_i < 1` tier. Thus

```text
D_asert_capped = max(D_asert, D_ceiling)
w_i            = miner i's share of the pool hashrate (node-local)
tier_i         = max(floor, D_ceiling, round(w_i * D_asert_capped))
```

`D_asert` is deliberately absent from the outer maximum: including it would
pin every job back at ASERT and the floor would never bind. `D_ceiling` stays
in it because the derived `floor` is already `max(D_ceiling, ...)`, so the
term is redundant today; it mirrors the tier helper's separate floor and
ceiling inputs and keeps each conversion conservative on its own. The
tier enters the job through the existing `tier_bits` path (`insert_miner_job`,
`build_notify_with_job_id`), written into the commitment, so the emission
gate (`submit.rs`) and the session clamp (`apply_difficulty_constraints`)
follow it with no further change. Two rules hold for every conversion:
difficulty and target are inverse (a smaller target is harder), and every
difficulty-to-bits conversion uses the truncating compact encoding so the
decoded difficulty is never below the source difficulty (the never-easier
rule).

A floor rise can never invalidate an issued job. The floor a share is judged
against is derived from the window ending at **that share's parent** (the
determinism rule above), and the job's parent is fixed in its commitment
(`prev_share_blockhash`) before the job is issued: a share that would raise
the floor is a descendant of that parent, so it is not in the parent's window
and cannot change the floor the job's share is judged against. The node
computes the tier's floor over that same window -- its tip is the parent it
embeds in the job it is about to issue -- so `tier_i >= floor` holds by
construction at issuance and at validation alike. Under V3 the same anchoring
applies to the raised floor: an attack detection step moves the floor for
shares that build after the attack, never for shares already in flight whose
parents predate it, so honest miners do not churn jobs during an attack. The
remaining gap is only a node's own freshness in refreshing jobs to new tips,
and each stale job's share is still judged against the floor of the parent it
carries.

V2 also deletes `sharechain_tier_mode`: with no static floor left to gate, a
node computes tier bits exactly when a derived floor exists, and that is the
vardiff chain alone. Main and mini have no derived floor, so their jobs keep
`prepared.bits` and no code path reaches the tier for them.

Arithmetic conventions, fixed here so V2's consensus arithmetic needs no
fixed-point unit:

- `d`, `W_min`, and `W_total` are exact integers: `d` is a share's declared
  difficulty measured against the **mainnet** proof-of-work limit
  (`Target::difficulty(Network::Bitcoin)`, the same unit as
  `target_for_difficulty` and `share_difficulty_ceiling`), and the window
  sums accumulate in u128 with `saturating_add`. The floor window is
  deliberately *not* summed in the payout window's `ExactWork` unit: the band
  compares in mainnet difficulty space, and the two units are not
  interchangeable. A floor window built from chain state must never call
  `bitcoin_block_difficulty` with the node's `bitcoin::Network` for this:
  `payout_network` maps chipnet, scalenet, and regtest to `Network::Regtest`,
  whose proof-of-work limit is about `2^31` times easier, so the resulting
  difficulty would be off by that factor. The u128 sum narrows to the u64
  `floor` argument of `check_sharechain_band` and is refused, never
  saturated, when it exceeds `u64::MAX`: saturation would not land at the
  ceiling band -- `target_for_difficulty(u64::MAX)` is a far stricter target
  than the ceiling's, so the band's target-space `min` would pick it and
  reject every honest share. The narrowing fails closed: a window sum outside
  the representable range leaves the rule's validated domain, so the share is
  refused without a verdict rather than judged against a floor the arithmetic
  cannot produce.
- The ratio `W_min * D_asert_capped / W_total` is integer division with
  round-up (never-easier), computed in u128 before the u64 narrowing above.
  `D_asert_capped` is the same `calculate_target_clamped` value the equality
  pin computes, in mainnet difficulty space: derived from the parent header's
  height and time against the chain's epoch, so it is available
  wherever the pin is, including header sync, where the batch-local
  `asert_parent_cache` already serves it -- the floor computation walks the
  window, not a second ASERT chain. The ceiling's
  fractional difficulty is never itself a `floor` value: it is enforced as a
  separate ceiling input, the target-space `min` in the band (below) and the
  `D_ceiling` term in the tier formula above, so the u64 floor is always an
  integer; the tier's `w_i` is the one non-integer, the node's own smoothed
  observation (the metrics actor's 600-second window with its publish-time
  EMA), and float arithmetic is safe there because peers never check it.
- The band compares in target space, not difficulty space:
  `threshold = min(target_for_difficulty(floor), ceiling_target)` with
  `target_for_difficulty`'s exact division, so the ceiling's fractional
  difficulty (3.90625) is ordered exactly and needs no rounding.
- Rounding is always conservative: difficulty-to-bits uses
  `to_compact_lossy` (never easier), target-to-difficulty for a lower bound
  rounds up, and target-to-difficulty for a session cap
  (`share_difficulty_ceiling`) rounds down.
- Only the floor and the band are consensus; the floor is an integer and the
  ceiling side of the band is ordered exactly in target space. Every
  tier input, `w_i` included, is node-local observation; peers check the band
  alone.
- The floor walk reuses the payout window's own cache and walk
  (`PplnsWindow`'s confirmed entries, invalidated as shares organize past the
  confirmed tip), with its own fail-closed uncle resolution (above). When the
  payout count cap binds, that cached walk already covers the floor window;
  when the payout work budget binds first (a small paying-block difficulty),
  the payout walk stops early and the floor walk extends past that stop with
  its own bounded walk, still capped at `PPLNS_WINDOW_SHARES`. Either way the
  per-job tier floor reads the cached floor result and adds no walk of its own.

Consensus split of responsibilities: the **band is consensus** (every node
validates `declared_difficulty >= floor` at all three sites), the **tier is
node-local** (a node chooses job bits from its own observation; peers only
check the band). The floor is consensus because it is a pure function of
accepted shares and their uncle headers; a node missing a window uncle header
refuses the share rather than deriving a different floor (above). The rule tag
pins the rule set, not a snapshot value; when the window or its uncles cannot
be materialized the share is refused rather than judged (above).

### V3: dynamic anti-spam floor

V3 adds the outline's attack response (lines 18-20) on top of V2's floor
mechanism: a running baseline of honest traffic that raises the floor when
the window's observed emission exceeds it and decays back when the attack
subsides. Like V2's
floor, every V3 quantity is a pure function of the share DAG ending at the
validating share's parent. The baseline, the period classification, and the
step index that sets the raised floor are all computed by folding over the
same parent-anchored trailing window in chain order, from the entries'
recorded work and timestamps, so any two nodes holding the same window compute
the same raised floor for the same share, and a share mined under one node's
raised floor is judged the same by every peer whose window matches. The folder
advances one step per entry of that window; a node's local arrival order and
clock never enter.

- **Baseline.** The parent-anchored window's observed emission: its summed
  declared work over the recorded span between its oldest and newest entry
  timestamps. The time base is share timestamps, which consensus already
  validates (median time past and the 120-second future bound), so no
  node-local clock enters the rule. The baseline is the fold's long-horizon
  average of that emission over windows already classified at-or-below
  threshold; it is the "appropriate level for small miners" to which the floor
  recovers.
- **Attack detection.** The window's emission materially above the baseline
  for a sustained run -- a run of consecutive entries in the same
  parent-anchored window, so "sustained" is DAG data, not wall-clock time at
  the receiving node. The raised floor makes each spam share cost
  proportionally more, buying back the validation and storage work. The raise
  is capped against the state at attack onset, not against any quantity
  observed during the attack: the cap is `C * floor_onset`, where `C` is a V3
  constant and `floor_onset` is V2's derived floor at the entry where the
  sustained run begins -- both DAG quantities, neither a function of the
  arriving traffic the cap is meant to police. `floor_onset` is bounded below
  by `D_ceiling`, so the cap can never request an easier floor than the
  anchor. During a sustained attack, floored miners are priced out by design
  until the recovery run completes; the cap's job is bounding the
  suppression depth, not preventing it.
- **Recovery.** Once the trailing entries show the emission back at or below
  baseline for a recovery run, the fold steps the floor back toward the
  baseline level rather than dropping at once, so an attacker cannot use the
  recovery ramp itself as a spam window. The step is an index in the fold, so
  which step applies to which share follows from the window's entries alone.
- **Poisoning guard (outline line 20).** The baseline update must not take
  the attack into the baseline. The statistic is window *work*, the summed
  declared `d`, not share count: a spam share at the floor costs real hashes
  toward the statistic, so sub-threshold campaigns cannot ratchet the baseline
  up on cheap shares the way a count-based classifier would -- an attacker
  must burn work at least proportional to the traffic it wants the baseline to
  absorb. On top of that, the fold's long-horizon average may not exceed its
  value at any past entry by more than a fixed drift factor absent a sustained
  regime change across an entire horizon, so repeated sub-threshold campaigns
  cannot accumulate without bound. The floor itself is clamped between the
  derived floor and the raise cap (above), so no baseline poisoning can move
  the floor outside the range honest miners can survive. The concrete constants
  (the detection multiple, the sustain and recovery runs, the baseline
  horizon, the drift factor) are fixed in the V3 increment from its unit-table
  runs; V4 verifies the behavior live rather than defining the constants.
  These are code constants like the existing ASERT and window constants, not
  operator configuration: outline line 5 forbids new static *directives*, and
  internal constants like these are unavoidable in a state machine and stay
  code-local.

V3 changes the state machine only; the band, the tier, and the job plumbing
from V2 are untouched. The consensus change is the fold: the floor's raise and
recovery are a pure function of the parent-anchored window, the same anchoring
rule V2's derived floor uses, never any node's local arrival order or clock.

## 3. Configuration

The outline (line 5) forbids any new static directive: "all values such as
value min/max should be function derived or existing defaults." Vardiff
follows that: the floor is derived from chain traffic (section 2), so there
is no floor key or difficulty bound to configure.

- **Added**: `vardiff` as a `BchShareChain` value and
  `--vardiff-sharechain` in `p2poolv2_node`. V2 and V3 add no keys.
- **Deleted with V1**: the static `sharechain_diff_floor` key, its sample
  entries, and the `DEFAULT_SHARECHAIN_DIFF_FLOOR`,
  `DEFAULT_SHARECHAIN_DIFF_FLOOR_TESTNET`,
  `DEFAULT_SHARECHAIN_DIFF_FLOOR_REGTEST` constants, together with the code
  comments that reference them. A configured floor is exactly what the
  outline forbids.
- **Deleted with V2**: `sharechain_tier_mode` (and its `UniformFloor` mode):
  with the static floor gone there is no switch left to gate, and the tier
  activates intrinsically whenever a derived floor exists (vardiff only).
  The validator's `sharechain_diff_floor` field and its
  `with_sharechain_diff_floor` builder evolve into the derived-floor source,
  the tier computation stays as vardiff's enforcement point, and the
  `floor-<value>` tag formatter is removed once no chain carries a static
  floor (section 5).

The store path convention needs no code: operators (and the sample compose
setups) already suffix the store by chain;
`P2POOL_STORE__PATH=/p2poolv2/data/${NETWORK}-${BCH_SHARE_CHAIN}` yields
`chipnet-vardiff`. The genesis guard catches a reused store with a clear
error, as it does today.

## 4. Anchor and ceiling choice

`MAIN_ANCHOR_TARGET` (`0x1b4188f5`, difficulty 1,000) anchors main with
ceiling `MAX_POOL_TARGET` (`0x1b384bd7`, difficulty 1,164); the mini anchor
`0x1c384bd7` (difficulty 4.55) doubles as mini's ceiling, and mini's ASERT
easy-clamp keys off the same constant (`asert_easy_clamp`) so a mini chain
never eases past its own anchor. Vardiff follows the mini pattern, for a
reason the outline makes explicit: the floor's whole point is to sit *below*
the ASERT difficulty for small miners, so a vardiff chain must have a ceiling
far easier than main's 1,164 -- otherwise the effective floor
`max(floor, D_ceiling) = 1,164` would bind above every small miner's tier
and the chain would behave exactly like main.

Vardiff therefore gets its own anchor/ceiling/easy-clamp constant. The
design choice is the mini exponent with the main mantissa:
`VARDIFF_ANCHOR_TARGET = 0x1c4188f5` (difficulty 3.90625). It differs from
both existing targets, so the genesis hash differs; it keeps the compact
mantissa rules (`0x4188f5` is encodable in this exponent); and being 256x
easier than main's anchor it gives the derived floor room to bind at small
pool sizes without an artificially hard start. The vardiff arms mirror
mini's:

- `anchor_bits(BchShareChain::Vardiff)` returns the constant (genesis bits);
- `target_ceiling` returns it (the chain's own ceiling);
- `asert_easy_clamp` returns it (ASERT never eases past the anchor).

Genesis hash pin tests (the `bch_genesis.rs` pattern) are added for the
vardiff chain; the existing main and mini pins do not move.

The anchor is a code constant, not configuration: the Vardiff arm returns
`CompactTarget::from_consensus(VARDIFF_ANCHOR_TARGET)`. It is isolated to
one line, one ceiling arm, one clamp arm, and one test.

V1 implementation note: `asert_easy_clamp` currently returns the main
ceiling's target for every anchor except mini, so the vardiff arm of that
match is required in the same commit; without it, vardiff ASERT would
silently ease only to main's difficulty-1,000 target, harder than the
vardiff anchor, and the easy clamp would never bind.

## 5. Protocol tag

`share_chain_rule_tag` gains a vardiff arm emitting the single `vardiff`
segment:

```text
.../pplns-k3-m100/asert-parent/vardiff/<other-rules>/.../1.0.0
```

in place of `floor-<value>`. The segment names the chain's rule set, not a
build counter: it is `vardiff` from the first increment through the completed
chain, because V1/V2/V3 are increments of one implementation and are not
meant to coexist on a libp2p network. The tag cannot pin the floor, since the
floor is derived and dynamic, and it does not need to: only the completed
chain is deployed, and nodes built from the same release enforce the same
rules. The `floor-<value>` segment stays dormant through V1 and is removed in
V2: after V1's purge of the static floor no chain carries one, so main and
mini emit their pre-floor strings and vardiff emits `vardiff`.

The segment's name lives beside the other `BCH_*_RULE_TAG` constants. The
rule-tag builder currently takes `(chain, floor)`; it grows the share-chain
variant (`BchShareChain`) so the vardiff arm can emit the `vardiff` segment,
and `protocol_string` / `kad_protocol_string` /
`P2PoolRequestResponseProtocol::new` thread it through their existing
callers (request-response, Identify, Kademlia, Noise prologue -- all already
consume the one builder). The full tag is the only handshake barrier: no
extra negotiation message, no feature bit, and nodes on a vardiff chain never
see main/mini peers at all (different genesis tag as well).

If the shipped vardiff rule set ever changes, that change gets its own fence
then -- a new segment or a re-anchored genesis -- following the codebase's
existing conventions; this document does not pre-commit one.

## 6. What stays exactly as it is

- **Main and mini, Bitcoin.** Protocol strings, validation, jobs, config:
  untouched. The new variant is additive; no code path switches on it unless
  configured.
- **The band code.** `sharechain_diff_floor: None` reproduces the equality
  pin. V1 purges the static floor and its default fill, so main and mini
  validate by the equality pin and their protocol strings lose the
  `floor-<value>` segment; the band goes live on vardiff only, with the
  derived floor in V2.
- **PPLNS.** Window (`K = 3`, 120,960 cap, 100-share maturity skip), tag
  `pplns-k3-m100`, and weight math unchanged. Larger miners hold more shares
  in the window and earn more; the floor changes which shares exist, not how
  they are paid.
- **Earnings comparable to main and mini.** The payout window pays per work
  unit and, on Bitcoin Cash, is a work budget (`K = 3` blocks of the paying
  share's difficulty, capped at the count): whichever bound binds, the window
  credits work at the chain's own rate, so a miner's mean income is the
  chain's emission times its hashrate share, independent of how many shares
  it emits. Vardiff changes share frequency and payout variance -- more,
  lighter shares for small miners -- but not the work credited per unit of
  hashrate, so total earnings stay comparable to main and mini (outline line
  26). The floor moves which shares exist and how often they arrive; the
  weight math still pays each share for the work it represents. The one
  dependency is uncle payout weight: the payout walk credits an uncle only
  when the uncle's header resolves at payout time and earns it nothing
  otherwise (a missing header is skipped with a warning), so payout weight is
  never granted on a reference alone, and an attacker cannot inflate payout
  weight without a stored header carrying real work.
- **Stratum pseudodiff.** The per-session CKPool adjuster is unchanged; it
  still targets the session's configured share rate. The tier only moves the
  base target the adjuster clamps against: the adjuster may not issue job
  bits easier than the tier, in the same way it may not ease past the pool
  ASERT today (the never-easier rule keeps every conversion conservative).
- **Hydrapool.** No share chain; never sees the tier.

## 7. Build order and gates

| Increment | Commit scope | Gate |
|---|---|---|
| **V1** | `BchShareChain::Vardiff`, anchor constant + genesis arm + pin tests, `vardiff` tag arm + tests, CLI flag with precedence, removal of `sharechain_diff_floor` key/constants/samples, code comments updated to this document | fmt, clippy, nextest; two-node local run on regtest with mixed flags proving isolation |
| **V2** | Integer floor derivation over window history, band activation on vardiff, per-miner tier wiring, fail-closed uncle materialization for the floor walk, `sharechain_tier_mode` deletion, `floor-<value>` formatter removal | same gates + tier/band unit tables, including a deliberately missing uncle header refusing the share, a window sum above `u64::MAX` refusing rather than saturating, and a sync-composed window deriving the same `W_min` as the stored-composed walk, including a maturity-boundary case; local two-miner run |
| **V3** | Baseline, detection, recovery, poisoning guard | same gates + state-machine tables, including a share mined on a job issued before a floor rise still validating at its parent-anchored floor and a floor-movement case with window-length latency when the eligible set changes; chipnet run |
| **V4** | Live verification on chipnet with the pooler-cpuminer scripts (`start1-vardiff1.sh`, `start1-vardiff2.sh`), two miners at different hashrates | observed shares in window for both miners, floor movement under a synthetic burst |

Each increment is its own commit, gated in order, and increments are not
separate chains: only the completed chain is deployed, and a live network
runs one build across all nodes. The anchor is fixed (section 4) before the
first increment, so the genesis holds across increments; if a
pre-completion change did force a new anchor, the runtime guard
(`GenesisMismatch`) makes the stale dev store loud instead of silent.
