# Share chain redesign for payout variance and fees

The rules and constants of the Bitcoin Cash share chain's design for small
miners
([issue #79](https://github.com/CyberAshven/BitcoinCash-p2poolv2/issues/79)):

1. one share chain instead of main and mini;
2. a difficulty band with a dynamic cap;
3. weak shares;
4. payout levels, a payout threshold paid by a draw;
5. a dynamic share period (not planned);
6. weak shares that follow the asks (PR #118).

Parts 1 to 4 and 6 are share-chain rules in force. None changes Bitcoin
Cash's own rules: the blocks P2Pool finds stay ordinary Bitcoin Cash blocks,
with a different set of coinbase payees. What they mean for a miner, in
numbers, is on [Small miners](../small-miners.md); how the window credits
each kind of work, on [PPLNS accounting](pplns-accounting.md).

In force: the protocol tag segments `band-c64`, `levels-546x2-k1` and
`weak4-640-100-48-256-320-64-4096-64-32-summary`, and store format 12.
Earlier tags and formats: [Rule history](rule-history.md).

```text
 home miners ----- full and weak shares ----.
                                             v
 ASICs and farms -- full shares, heavier ---> one share chain
                    inside the band               |
                                                  v
                          PPLNS window: 3 BCH blocks of work
                                                  |
                                                  v
                          coinbase of a found BCH block
                           |                      |
            slice >= the miner's level    slice < the level
                           |                      |
                     paid directly        draw within the level,
                                          paid out in the same block
```

## 1. One share chain

Bitcoin Cash has one share chain, so every miner shares one PPLNS window and
the whole pool's hashrate finds blocks for everyone.

- The chain has the former mini chain's anchor, ceiling and genesis
  (`MINI_ANCHOR_TARGET`, `0x1c384bd7`, difficulty about 4.55), so a small
  pool still makes a share every 10 seconds; ASERT raises the difficulty as
  the pool grows. The main chain (`MAIN_ANCHOR_TARGET`, ceiling
  `MAX_POOL_TARGET`) is gone on Bitcoin Cash.
- A config that still sets `bch_share_chain` is refused at start-up with a
  message saying the chains merged.
- The rest is the mini chain's: the window of 3 BCH blocks of work behind a
  100-share maturity skip and at most 120,960 shares, ASERT anchored at the
  parent with a 600-second half-life, uncles, direct coinbase payouts, the
  hybrid payout tree, and each share's own donation and fee.

## 2. Difficulty band

### The rule

A share's target `T`, its `bits`, must satisfy

```text
T_asert / C(parent)  <=  T  <=  T_asert
```

where `T_asert` is the ASERT target from the parent
(`calculate_target_clamped`) and `C(parent)` the band cap carried from the
parent (below): a share may be up to `C` times harder than the minimum,
never easier. The comparison uses exact 256-bit targets; a compact target is
rounded so it is never easier than the target it stands for. Full
validation (`validate_with_pool_difficulty`), header sync
(`validate_dag_connectivity_and_difficulty`) and block receipt
(`validate_asert`) all check it.

ASERT is anchored at the parent's minimum, not at its `bits`: anchored at a
heavier parent's `bits`, every share after a heavy one would have to be
heavy too. A share's weight in the window and its work in fork choice are
its exact work, `2^256 / (T + 1)`. ASERT counts shares by height and time,
so when big miners mine heavier shares the pool makes fewer shares, and
ASERT lowers the minimum for everyone else.

### What a share carries

| Field | Contents |
|---|---|
| `wanted_bits` | The target the share's node wanted for this miner before the cap: `bits`, harder when the cap stopped it, or easier when a weak share asks (part 6). A signal only: it never makes a share valid or invalid |
| `band_cap` | The share's cap `C` in units of 1/1024, which must equal one step from its parent's (below) |
| `asert_bits` | The share's minimum `T_asert`, which must equal the ASERT target from the parent; the next share's ASERT is anchored at it. At the minimum it equals `bits` |

All three are committed after the share version (tag byte `0x24`, then the
4-byte compact `wanted_bits`, the 4-byte `band_cap` and the 4-byte compact
`asert_bits`). Carrying `band_cap` lets a node check it from the parent's
header alone, in header sync too.

### The cap `C`

`C` is carried from share to share, the way ABLA carries Bitcoin Cash's
block size limit, starting at 1:

```text
C_fp:  C in units of 1/1024, an integer from 1024 (C = 1) to C_MAX * 1024

band_cap(genesis) = 1024
u                 = min(work(parent.wanted_bits) / work(the parent's own T_asert),
                        C_MAX), an integer, at least 1
wanted more       = parent.wanted_bits is harder than
                    (the parent's own T_asert) * 1024 / parent.band_cap

band_cap(child) = parent.band_cap + parent.band_cap * u / 512   if the parent wanted more
band_cap(child) = parent.band_cap - parent.band_cap / 512       otherwise
then clamped to 1024 .. C_MAX * 1024
```

| Constant | Value | Why |
|---|---|---|
| `C_MAX` (`BAND_CAP_MAX_MULTIPLE`) | 64 | At 1% of Bitcoin Cash with 90% industrial, from 30 to 64 the minimum falls from 0.13 to 0.12 of uniform and the Bitaxe's swing from 0.15 to 0.14; 256 adds 0.01 more |
| step (`BAND_STEP_DIVISOR`) | 1/512 a share: `C` doubles in about 355 heavy shares and halves in about 355 others | Where `C` settles does not depend on the step, only how fast: within 10% after 1 hour at a divisor of 128, 4 at 512, 12 at 2,048 (`payout-sim --scenario band-step`) |

`u` comes from `wanted_bits`, not `bits`: while `C` is 1 every share's own
work is the minimum's, so a step driven by `bits` could open the cap only
when more than half of all shares wanted heavier ones, which never happens
in a mixed pool. A miner pushes `C` up only by wanting heavier shares, which
it then mines, and that only lowers the minimum for small miners. Hashrate
that mines at the minimum while recording the largest wanted difficulty
moves `C` to 48 to 61 with 2% of the pool and to 64 with 10%, and changes
nothing home miners are paid: the cap only bounds how heavy a job a node may
give.

### Node default (policy, not a rule)

A node gives each miner the minimum target unless the miner would then make
more than `max(4 shares per pool block, 300 shares a month)`
(`SHARES_PER_POOL_BLOCK`, `SHARES_PER_MONTH`). Such a miner gets a harder
target, up to `C` times the minimum, so it makes about that many;
`wanted_bits` records the target it would have had without the cap, and the
miner's job carries both (`insert_miner_job`). The pool's block rate comes
from the work of its newest 60 confirmed shares (`POOL_HASHRATE_SHARES`)
over their time span, against the template's Bitcoin Cash difficulty; a
miner's hashrate is its accepted submissions over the last ten minutes, all
connections together (`stratum/work/band_policy.rs`).

With one-minute blocks, 4 shares a pool block pass 300 a month from 0.17% of
Bitcoin Cash, so few miners get heavier shares: a 100 TH/s ASIC at 1% of
Bitcoin Cash with 90% industrial wants 3.4 times the minimum, and from 10%
no simulated miner wants more.

No miner gets more than the room left, the minimum's work over the
ceiling's, since ASERT cannot ease the minimum past the ceiling. So a pool
too small for 10-second shares at the ceiling keeps every share it can make
(without the bound a tiny pool's shares fell to 0.0036 a second), and ASERT
settles on one minimum as a growing pool nears the ceiling. Nothing forces
heavier shares: a node that gives every miner the minimum brings the minimum
back to that of a chain with one difficulty.

## 3. Weak shares

A weak share is an easier proof of work that a later share carries, the way
it carries uncles. It lets a home miner's work count often even in a pool of
several EH/s, without a second chain and without trusting an operator.

```text
a miner's hash
   |
   +-- meets the BCH block target? ---- yes --> BCH block (also a full share)
   |
   +-- meets its share target? -------- yes --> full share: extends the chain
   |
   +-- meets the weak target? --------- yes --> weak share: relayed, then
   |                                            carried by one of the next
   |                                            few shares
   +-- otherwise: counted only by its own node, for hashrate statistics
```

### What a weak share is

A weak share is a proof of work on an ordinary job: its share header and
Bitcoin Cash coinbase are what a full share on that job would carry. It is
valid when:

1. its `bits` is exactly `T_asert` of its parent: weak shares come only from
   jobs at the minimum, so a miner on a heavier band target gets no credit
   for near misses;
2. its proof-of-work hash meets its parent's weak target (below) and does
   **not** meet its own `bits`: a hash that meets `bits` is a full share and
   travels as one;
3. its Bitcoin Cash coinbase pays the PPLNS window anchored at its parent,
   exactly as a full share's must;
4. it passes every header rule a full share passes except the share target.
   A carried weak share's time is at most 120 seconds after its carrier's
   (`check_carried_weak_time`), so nodes whose clocks differ cannot disagree
   about a share for good; a relayed one ahead of the local clock is held
   and checked again;
5. its proof-of-work hash is not that of any share, uncle or weak share
   already credited on the carrying share's chain.

A weak share is known by its proof-of-work hash, the hash of the Bitcoin
Cash header it carries. One proof of work is credited once, as a share or as
a weak share.

A node turns a submission that meets the weak target on a job at the
minimum into a weak share. For that a connection's vardiff must not exceed
the weak difficulty: the cap that keeps a session's difficulty at or below
the share difficulty (lessons V8, `share_difficulty_ceiling`) is the weak
difficulty for miners at the minimum. SV1 and SV2 work the same way; miners
change nothing.

### The weak target

```text
T_weak(parent) = min(T_asert(parent) * R(parent), T_ceiling)
```

`R(parent)` is the parent's `weak_ratio`, in units of 1/1024, never below 1024
(`R` = 1), 32 at genesis. Each share's must equal one step of 1/256 from its
parent's: down when the parent carried more than `WEAK_FILL` weak shares,
up when fewer ([part 6](#6-weak-shares-that-follow-the-asks) has the full
step, with the asks). So the weak difficulty follows the home miners who
use it, not industrial hashrate, and is never easier than the ceiling: a
small pool at the ceiling never needs weak shares.

| Constant | Value | Why |
|---|---|---|
| `R_MAX` (`WEAK_RATIO_MAX_MULTIPLE`) | 256: the largest ratio while nobody asks | `R` settles at 35 to 88 wherever any hashrate mines at the minimum; 256 and 4,096 pay the same in every mix (Study #85) |
| `WEAK_FILL` | 32: the count `R` steers towards while nobody asks | At 30% of Bitcoin Cash a Bitaxe is paid every month (swing 0.39) for 2.9 KB/s; a fill of 8 (cap 16) leaves 7.5% of its months empty, 64 (cap 128) lowers the swing to 0.31 for 5.6 KB/s |
| `WEAK_IDLE_CAP` | 64: the most a share carries unless more than one in 64 of them ask (part 6) | Twice the fill, room for the luck of share times: a quarter of shares reach it |
| `WEAK_CAP` | 640: the most a share carries while enough ask | Twice `WEAK_FILL_MAX` (part 6) |
| `WEAK_BYTES` | 960 KiB of weak shares a share | 640 at a 1.5 KiB average |
| `WEAK_DEPTH` | 48 shares, 8 minutes: the furthest a carried weak share's parent may be below its carrier | The only tested change that stops a coalition of 90% of the hashrate gaining by refusing to carry others' weak shares ([Study #85](#study-85-weak-work-in-fork-choice)) |
| `INCLUDER_CUT` | 100 basis points (1%) of a weak share's credit, to its carrier | At depth 48 refusing to carry others' weak shares gains no coalition anything worth having ([Study #85](#study-85-weak-work-in-fork-choice)) |

A node's weak pool holds at most 4 × 640 × 48 = 122,880 weak shares
(`MAX_POOL_WEAK_SHARES`, about 180 MB), and at most 30,720 (45 MiB) waiting
for parents it does not have; it starts with room for 12,288 and grows only
while the asks open the count. A carried-once check reads the weak shares
of up to 47 ancestors. A weak share needs only a known Bitcoin Cash prev, so
waiting does not make it stale.

### What a weak share message holds

| Field | Size |
|---|---|
| The share header, as mined | about 300 to 450 bytes |
| The Bitcoin Cash coinbase scriptSig | at most 100 bytes |
| The coinbase's total value | 8 bytes |
| The coinbase merkle branch | 32 bytes per level, 17 levels for a block of 100,000 transactions |

About 0.5 to 1.2 KB. The payout outputs are not carried: every node already
computes them for that parent.

### How a node checks one

The parent must be known and within `WEAK_DEPTH` of the node's tip (until
then the node holds the weak share in its weak pool, or drops it). The node
rebuilds the Bitcoin Cash coinbase as for a share on that parent (the
scriptSig from the message, output 0 from the header's merge-mining fields,
the window's payouts for the message's total value, the payout tree output,
the padding, the share commitment from the header and the locktime from its
height), hashes it, folds the branch, and requires the header's merkle root.
It then checks the proof of work against `T_weak(parent)` and rule 2, and
rule 5 against its best chain and weak pool. The window's payouts are cached
per parent and template, so what remains per weak share is hashing its
coinbase, at most 1,000,000 bytes, a few milliseconds.

Only a weak share that passes is remembered as seen, so an altered copy
relayed first never keeps the real one out. A node takes at most 192 weak
shares a second from one peer (`PEER_WEAK_SHARES_PER_SECOND`, three times
what a full pool makes, in bursts of up to 1,280), which bounds the checks a
peer can cause (node behaviour).

### How a share carries them

A share carries up to `WEAK_CAP` weak shares and `WEAK_BYTES` of them in its
body, more than `WEAK_IDLE_CAP` only while enough ask. Its header commits
them under tag byte `0x26`: the 4-byte `weak_ratio`, a 2-byte count, the
2-byte `weak_asking` (part 6) and the 32-byte weak root,
`SHA-256d(merkle root || SHA-256d(summary))`. The merkle root's leaves are
the SHA-256d of each weak share as sent (header, scriptSig, total value and
branch); the summary is what the window and the carried-once rule read. So
the proof of work fixes every byte of every carried weak share. It has to:
the share block hash covers the header, not the body, and with a root of
proof-of-work hashes alone a peer could alter a weak share's other fields in
an honest share's body and get the share marked invalid. Any such change is
now a bad copy, fetched again. A carried weak share must be valid as above,
its parent at most `WEAK_DEPTH` below the carrier, and not carried by any of
the carrier's ancestors.

A node fills its own shares from its weak pool, oldest parent first so none
expire, up to the cap. It relays a weak share once checked, and forgets it
once its parent is more than `WEAK_DEPTH` below the tip or the best chain
carries it. A weak share travels once; a new share lists the ones it carries
by proof-of-work hash (`ShareWithWeakHashes`, protocol message segment
`weak-by-hash`; [P2P network](../p2p-network.md#gossip)). Whole weak shares are pruned one
payout depth below the confirmed tip, and their summary stays with the share
body ([Pruning](../architecture/pruning.md)).

### How weak shares are paid

Every proof of work from a job at the minimum, full share, uncle or weak
share, is credited at the weak work, `2^256 / (T_weak(parent) + 1)`; only a
share heavier than the minimum is credited at its own work. A weak share's
credit, less `INCLUDER_CUT` for its carrier, goes to its own payout address
([PPLNS accounting](pplns-accounting.md#weak-shares-and-the-weak-work-credit-bitcoin-cash)
has the table).

Why: the hashes that meet the weak target are a fair sample of a miner's
work. Crediting a full share at its own work on top would pay those hashes
twice, `1` per unit of work for the full shares plus `1 - 1/R` for the weak
shares; the simulation then paid a Bitaxe 1.8 times its fair pay and
industrial miners 0.91 times. When `R` is 1, nothing changes.

Weak work does not count in fork choice. Counting it would make a share
that drops others' weak shares lose races, but through races alone it moves
a refusing coalition's gain by 0.12 points at most, not worth its
header-sync work ([Study #85](#study-85-weak-work-in-fork-choice)). Walking
a window full of weak shares costs about 3.5 microseconds a payee, not a
credit: 160 ms at the 120,960-share cap with 6,000 payees.

## 4. Payout levels

A payout level is the smallest payment a miner wants. A slice below it is
paid by a draw among the miners at the same level, within the same block,
so nothing is held and nothing is owed.

### Levels

```text
L_k = 546 * 2^k satoshis,  k = 1 .. 7
    = 1,092, 2,184, 4,368, 8,736, 17,472, 34,944, 69,888
```

546 is Bitcoin Cash's P2PKH dust limit (`PAYOUT_LEVEL_FLOOR`); 69,888, the
cap, keeps large amounts out of any draw. Each Bitcoin Cash share header
carries `payout_level`, `k` from 1 (`MIN_PAYOUT_LEVEL`) to 7, one byte
committed after the band (tag byte `0x25`); a header outside that range does
not decode. Level 0 is not allowed: a 546-satoshi win in the payout tree was
below dust after the tree's fee and went to the tree's largest payee
(`payout_tree::merge_dust_payees`), so level-0 miners were paid 0.79 to 0.82
of fair on a 12-node swarm (`docs/simulation/RUNBOOK.md`, Test J). What
remains is small: a pot below one level payment is paid whole to one payee,
and a miner alone below its level keeps its own slice, each under 1,092
satoshis a block.

A payee's level in a block is the `payout_level` of its newest credited
entry in the window: the newest share, uncle or weak share paying that
address. The walk visits window shares newest first; an uncle is placed at
the share that includes it, and a weak share at the share that carries it,
after that share's uncles. The donation key and fee addresses are never
drawn, even when they also mine.

### The draw

After the window's weights, cuts and rounding give each payee an amount in
satoshis:

1. Every payee whose amount is at least its level, the donation key and every
   fee address keep their amount.
2. The others are grouped by level. Within a group at level `L`, payees are
   ordered by locking script bytes, and `S` is the sum of their amounts.
3. `n = S / L`, rounded down.
4. The seed is the first 8 bytes, as a little-endian integer, of
   `SHA-256("p2pool-payout-level" || parent share hash || k)`: the 32 bytes
   of the paying share's `prev_share_blockhash` as its header serializes
   them, and `k` one byte (`draw_seed`).
5. If `n` is 0, one point `p = seed mod S` picks the winner: the payee whose
   running total of amounts first exceeds `p`. It gets `S`.
6. Otherwise `D = S / n` rounded down, points `p_j = (seed mod D) + j * D` for
   `j = 0 .. n-1`, and each point pays `D` to the payee whose running total
   first exceeds it. The first point's payee also gets `S - n * D`, less
   than `n` satoshis.
7. Each payee's winnings replace its amount; a payee that wins nothing is not
   paid in this block. The outputs, payout tree included, are then ordered
   by weight, as before the draw.

Every payee's expected payment equals its amount, up to the at most `n`
satoshis of step 6. Every node computes the same draw, because the seed
comes from the parent share. The share that pays cannot change it; a miner
could prefer one parent for a better draw only by mining away from the best
tip, and in the payout simulator that costs more in orphaned shares than it
gains (0.01 to 0.25% of its income).

### Choosing a level

A miner sets its level at login, in the password or after a comma, like
`donation=`:

| Option | Meaning |
|---|---|
| `min-payout=<satoshis>` | The smallest level at or above this amount |
| `fee-tolerance=<percent>` | The smallest level that keeps the cost of spending one payment at or below this share of it, using the node's `spend_cost` (148 satoshis by default, a P2PKH input at 1 sat/byte) |

`min-payout=` wins when both are given; a `fee-tolerance=` no level meets
gives level 7. Without either, the node's `payout_level` applies, by default
level 5 (`DEFAULT_PAYOUT_LEVEL`, 17,472 satoshis): 25 payments a month and
0.85% fee loss at every pool size, where level 6 halves the fee loss but
also the payments and raises the swing from 0.27 to 0.34 at 10% of Bitcoin
Cash. A Job Declaration token carries the level its identity chose, or the
node's, unlowered.

A node lowers a miner's level, and logs a warning once per connection, when
the miner can expect fewer than 3 payments a month at it
(`MIN_PAYMENTS_PER_MONTH`; node policy), estimated from the miner's
hashrate, the pool's and the template's coinbase value. The `/miner` API and
the dashboard show the level, expected income, payments a month and monthly
swing, `sqrt(1 / credits a month + 1 / pool blocks a month)`, plus
`1 / payments a month` under the root when the miner's slice of a block is
below its level.

## 5. Dynamic share period (not planned)

A share period that moves with the uncle rate is not planned
([PR #116](https://github.com/CyberAshven/BitcoinCash-p2poolv2/pull/116)):
in the payout simulator a shorter period and more weak shares a share pay a
small miner the same, and only the shorter period adds stale shares, 8.0%,
17.9% and 30.4% at 5, 2 and 1 seconds on 1,000 nodes against 3.8% at 10
seconds (`cargo payout-sim --scenario scale-levers`; its README's "Scale
levers"). At today's hashrate the period changes nothing for a small miner,
and at Bitcoin's neither lever saves a 1.2 TH/s miner, whose fair income is
1,605 satoshis a month. In the network simulation no share ever had a fourth
uncle to name, so the three-uncle limit loses nothing. The share period
stays 10 seconds; part 6 grows the weak shares a share instead.

## 6. Weak shares that follow the asks

### Why

At `WEAK_FILL` a share carries about 32 weak shares whatever the pool's
size, about 3.5 a second split among the miners at the minimum, so a
miner's weak shares a month fall as the pool grows. The smallest miner that
still finds three a month is about 0.33 TH/s for each EH/s of pool hashrate:
a Bitaxe's 1.2 TH/s once the pool holds 3.7 EH/s. More weak shares a share
fix that, but every weak share costs every node relay, validation and
storage, so a share carries more only while the miners that need them ask.
The slots cannot be the signal, since the weak ratio holds them at the fill
whatever the demand.

### The rule

1. **`wanted_bits` may be easier than `bits`.** It stays a signal; the band
   reads it as not wanting more.
2. **A weak share asks** when its `wanted_bits` is easier than the weak
   target it met (`weak_share_asks`). A share header commits how many of the
   weak shares it carries ask, `weak_asking` (2 bytes under tag `0x26`, after
   the count, never more than it); a share whose `weak_asking` is not the
   number of asking weak shares in its body is invalid.
3. **The weak ratio's step reads the asks** (`next_weak_ratio`):

```text
asked = parent.weak_count <= WEAK_FILL_MAX
        and parent.weak_asking * WEAK_ASK_DIVISOR > parent.weak_count

if asked:
weak_ratio(child) = parent.weak_ratio + parent.weak_ratio / 256
                        at most R_ASK_MAX * 1024
otherwise:
weak_ratio(child) = parent.weak_ratio - parent.weak_ratio / 256
                        if the parent carried more than WEAK_FILL weak shares,
                        or its ratio is above R_MAX * 1024
weak_ratio(child) = parent.weak_ratio + parent.weak_ratio / 256
                        if it carried fewer, at most R_MAX * 1024
weak_ratio(child) = parent.weak_ratio   if exactly WEAK_FILL
never below 1024
```

4. **The cap follows the asks** (`weak_count_allowed`), read from the header
   alone, so a header breaking it does not decode:

```text
weak_count <= WEAK_CAP
and (weak_count <= WEAK_IDLE_CAP or weak_asking * WEAK_ASK_DIVISOR > weak_count)
```

5. **A new chain starts at a weak ratio of 32** (`GENESIS_WEAK_RATIO`), where
   a chain carrying `WEAK_FILL` settles in a pool at a tenth of today's
   Bitcoin Cash.

| Constant | Value |
|---|---|
| `WEAK_FILL_MAX` | 320: asks stop counting once a share carries more; what every node must be able to carry |
| `WEAK_ASK_DIVISOR` | 64: more than one carried weak share in 64 must ask, so 640 needs 11 |
| `R_ASK_MAX` (`WEAK_RATIO_ASK_MAX_MULTIPLE`) | 4,096: the largest ratio asks may reach |

Without asks the count settles at a median of 32 and a mean of 34.5 a
share. A node builds its shares to the cap: oldest parent first, as many as
fit `WEAK_CAP` and `WEAK_BYTES`, leaving the newest silent ones for a later
share when the asking ones allow no more.

Why the idle cap and the genesis ratio of 32: with a cap of 640 whatever the
asks and a genesis ratio of 256, every share from genesis carried 640 weak
shares, the chain slowed under the checking load and the test swarm ran out
of memory. With both, where nobody asks no share carries more than 64, from
genesis or after a big miner joins. The cost (payout simulator,
`weak-asks-genesis`, `weak-asks-big-miner`, `weak-asks-rules`,
`weak-asks-rule-mixes`, `weak-asks-rule-attacks`, rule v4): in a pool at 30%
of ten times today's hashrate the count takes two hours to open from 32 to
344, and when the asks stop, up to 7.1% of the weak shares found in the next
six hours expire while the ratio falls.

### Node default (policy, not a rule)

A node asks for a miner at the minimum while it would find fewer than 16
weak shares a month at its job's weak target (`ASK_WEAK_SHARES_PER_MONTH`):
`wanted_bits` is then the target at which it would find 16, never easier
than the ceiling. A miner on a heavier band target makes no weak shares and
never asks. Past about 16 a month more weak shares barely help: at ten times
today's hashrate a Bitaxe with 2.8 a month has 9% of its months without a
payment, with 5.6 it has 4%, and with 14 to 29, 1.1 to 1.4%. The miners that
find the fewest weak shares need them most, which is why one in 64 asking is
enough.

### What it does

Payout simulator (`cargo payout-sim --scenario weak-asks`, and
`weak-asks-mixes`, `weak-asks-constants`, `weak-asks-attacks`), one-minute
blocks, half the pool's hashrate industrial. Without asks every pool carries
34.5 weak shares a share, 2.9 KB/s.

| Bitcoin Cash hashrate, pool | Weak shares a share with asks | Relay, KB/s | Bitaxe weak shares a month, without / with asks | Bitaxe months without a payment, without / with asks |
|---|---:|---:|---:|---:|
| today's, 10% | 46 | 3.8 | 29 / 39 | 0% / 0% |
| today's, 30% | 57 | 4.7 | 9.4 / 15.7 | 0% / 0% |
| 10 times, 10% | 189 | 15.3 | 2.8 / 15.7 | 10% / 1.6% |
| 10 times, 30% | 344 | 27.6 | 0.96 / 9.4 | 40.5% / 2.8% |
| 100 times, 10% | 344 | 27.6 | 0.28 / 2.8 | 75% / 14% |
| Bitcoin's (270 times), 10% | 344 | 27.6 | 0.10 / 1.1 | 91% / 46% |

Raising `WEAK_FILL` to 320 instead pays the same where it is needed (2.6% at
30% of ten times today's) but makes every pool relay 346 weak shares a
share, 27.8 KB/s. Every miner's income stays within the noise of its fair
part. At ten times today's hashrate the asks open the count in every miner
mix tested (141 to 345 a share; a Bitaxe has 0.9 to 2.9% of its months
without a payment, against 8 to 41% without asks), except where home miners
are under one weak share in 64: in the 99% industrial mix it opens only to
about 50, and a Bitaxe at 30% has 25.2% of its months without a payment.

### Attacks

| Attack | Result |
|---|---|
| Hashrate that asks whatever its miners find, in a pool that needs no more (10% of today's) | 1% of the hashrate at the minimum raises the count from 46 to 82 a share, 2% to 222, 5% to 315: free, but never more than the ceiling every node carries anyway, and it pays the asker nothing |
| Hashrate that never asks, in a pool that needs more (30% of ten times today's) | With half the hashrate never asking the count stays at the ceiling, with 90% at 316; 97% brings it to 143 and 99% to 57 |
| Industrial miners mining at the minimum, same pool | No change: 344 a share |
| Nodes that carry no weak share that asks, same pool | 250 a share at 25% of the shares, 168 at 50%, 107 at 75%, 83 at 90% (a Bitaxe's months without a payment 3%, 6%, 12%, 15%); they give up the includer cut on what they drop |

The worst an attacker can do is hold the count at the ceiling, which nodes
are sized for, or at the floor, the rule without asks. Weak-share spam costs
real work at the weak difficulty, is paid like anyone's work, and raises the
weak difficulty through `R`.

### What it costs a node

| | At the floor (32 a share) | At the ceiling (320 a share) |
|---|---:|---:|
| Weak shares a second | 3.5 | 35 |
| Relay, at 0.8 KB each | 2.8 KB/s | 28 KB/s |
| Stored: whole for 121,423 shares (14 days), summary for 242,534 (28 days) | 4.3 GB | 43 GB |
| Weak pool, at most | 12,288 at the start | 122,880 |

Validation, not bandwidth, is the ceiling. A weak share costs one coinbase
build per parent and template: 0.3 ms with 1,000 miners in the window,
3.8 ms with 10,000 and 45 ms with 100,000
([Network simulation](../simulation/network-sim.md), "Share period and each
node's load"). At the ceiling that keeps a node about 1% busy with 1,000
miners and 12% with 10,000; with 100,000 it needs 1.3 times a node, unless
the pool's nodes build one template (16%). A node checks the header rules of
the weak shares a share carries on helper threads, one fewer than its CPUs
(`utils/parallel.rs`, PR #127), and their coinbases on one thread, where
batches on several measured about 15% slower at 1,200 payees. So 320 a share
suits pools paying tens of thousands of miners, not 100,000. These costs
were measured with ten-minute blocks, whose coinbase paid 17,885 outputs
with 100,000 miners against at most about 1,790 now, so they overstate the
check.

## Simulation

The payout simulator, the crate `p2poolv2_payout_sim` (`cargo payout-sim`),
mines shares, uncles and weak shares with Poisson timing, sizes the window
by work and pays found blocks by weight with the draw, for pools at 1 to
30% of Bitcoin Cash, Bitcoin Cash at 1 to 270 times today's hashrate and
miner mixes from all home miners to 99% industrial. It set `C_MAX`, the
band step, `R_MAX`, `WEAK_CAP`, `WEAK_FILL`, `WEAK_DEPTH`, `INCLUDER_CUT`,
`WEAK_BYTES` and the default level; every table is in its
[README](../../p2poolv2_payout_sim/README.md).

### The designs compared

A Bitaxe (1.2 TH/s) in a pool where half the hashrate is industrial, with
one-minute blocks. Each cell is monthly swing / payments a month / fee loss:

| Design | Pool at 1% of BCH | 10% | 30% |
|---|---|---|---|
| Main and mini, before the merge | 0.29 / 45.8 / 1.57% | 0.92 / 4.7 / 0.18% | 1.57 / 1.5 / 0.06% |
| One chain, no band | 0.39 / 24.9 / 0.86% | 1.28 / 2.4 / 0.09% | 2.28 / 0.80 / 0.03% |
| Band | 0.30 / 45.1 / 1.55% | 1.28 / 2.4 / 0.09% | 2.28 / 0.80 / 0.03% |
| Cash Dragon's vardiff chain | 0.30 / 48.2 / 1.66% | 0.89 / 4.9 / 0.18% | 1.55 / 1.6 / 0.06% |
| Band, weak shares and payout levels | 0.20 / 25.2 / 0.85% | 0.27 / 24.8 / 0.85% | 0.39 / 24.0 / 0.84% |

Under the band alone, big nodes that give their miners the minimum put the
Bitaxe back at the one-chain numbers; with weak shares and the weak-work
credit nobody's income changes. Weak-share spam at 10 times the honest home
hashrate cuts the Bitaxe's weak shares from 579 to 51 a month, but its
payments stay at about 25 a month and its swing goes from 0.20 to 0.23.

### Study #85: weak work in fork choice

The study tested counting carried weak work in fork choice, as uncle work
counts, against nodes refusing to carry others' weak shares. At the first
constants (`WEAK_DEPTH` 6, a 5% cut, ten-minute blocks), with 90% of the
hashrate refusing, 56% of honest weak shares expired and the Bitaxe got 0.48
of its fair pay, whatever the cut.

| Finding | Result |
|---|---|
| Weak work in races alone | Races are about 2% of shares: honest weak shares lost move from 4.92% to 4.65% with half the hashrate refusing, and a 90% coalition still gains 2.6 to 4.3% at every cut |
| What stops a 90% coalition | `WEAK_DEPTH` 48: its gain falls from +2.7 to +4.2% at depth 6 to +0.03 to +0.04%, honest losses from 56% to 1.2%, and the Bitaxe keeps 0.99 of its fair pay |
| Includer cut | 1% at depth 48: a 90% coalition gains at most 0.04 to 0.06%; refusing costs a half-hashrate coalition 0.2 to 0.3% of its income and a lone refusing node 0.2 to 0.9% |

Decided: `WEAK_DEPTH` 48, a 1% cut and `R_MAX` 256; weak work stays out of
fork choice. The study's scaling ideas were built as #121 (weak shares by
hash), #128 (relay after the header check), #131 (shares relayed without
payouts) and #122 (16 outgoing peers).

## References

- [Issue #79](https://github.com/CyberAshven/BitcoinCash-p2poolv2/issues/79),
  with the numbers and the research behind this design.
- [Share validation](validation.md), [Share commitments](share-commitments.md),
  [PPLNS accounting](pplns-accounting.md),
  [Coinbase payouts](../payouts/coinbase.md),
  [Share difficulty: ASERT](../difficulty_adjustment/asert.md).
- [Lessons from P2Pool v1 on BCH](../p2pool-v1-bch-lessons.md), V6 and V7.
- [Small miners](../small-miners.md).
