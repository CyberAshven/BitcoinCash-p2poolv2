# Share chain redesign for payout variance and fees

Status: parts 1 and 2 implemented on branch feat/one-chain-band; part 4
implemented on branch feat/payout-levels, on top of it (store format 4);
part 3 on branch feat/weak-shares, on top of both (store format 5); part 5
is a proposal. Tracked in
[issue #79](https://github.com/CyberAshven/BitcoinCash-p2poolv2/issues/79).
Values marked *(sim)* are first proposals that the simulation in row 2 of the
issue confirms or replaces.

This page gives the exact rules for four changes to the Bitcoin Cash share
chain and the outline of a fifth:

1. one share chain instead of main and mini;
2. a difficulty band with a dynamic cap;
3. weak shares;
4. payout levels, a payout threshold paid by a draw;
5. a dynamic share period (outline only).

Parts 1 to 4 change share-chain rules. None of them changes Bitcoin Cash's
own rules: the blocks P2Pool finds stay ordinary Bitcoin Cash blocks, and
what a Bitcoin Cash node sees is only a different set of coinbase payees.
Why these changes, and the numbers behind them, are in the issue and on
[Small miners](../small-miners.md).

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

Main and mini merge into one chain, so every miner shares one PPLNS window
and the whole pool's hashrate finds blocks for everyone.

- The chain keeps the mini chain's anchor, ceiling and genesis
  (`MINI_ANCHOR_TARGET`, `0x1c384bd7`, difficulty about 4). A small pool then
  still makes a share every 10 seconds; ASERT raises the difficulty as the
  pool grows, as it does on mini today.
- The main chain (`MAIN_ANCHOR_TARGET`, ceiling `MAX_POOL_TARGET`) is
  removed for Bitcoin Cash. The `bch_share_chain` setting goes away; a
  config that still sets it to `main` is refused at start-up with a message
  saying the chains merged.
- Unchanged: the window of 3 BCH blocks of work behind a 100-share maturity
  skip and at most 120,960 shares, ASERT anchored at the parent with a
  600-second half-life, uncles, direct coinbase payouts, the hybrid payout
  tree, and each share's own donation and fee.

## 2. Difficulty band

### The rule

Today a share's target must equal the target ASERT computes from its parent.
With the band, a share's target `T` must satisfy

```text
T_asert / C(parent)  <=  T  <=  T_asert
```

where `T_asert` is the ASERT target from the parent
(`calculate_target_clamped`) and `C(parent)` is the band cap carried from
the parent (below). A share may therefore be up to `C` times harder than the
minimum, never easier. The comparison uses exact 256-bit targets; a target
written in compact form is rounded so it is never easier than the target it
stands for.

ASERT on Bitcoin Cash is anchored at the parent, and its anchor target is the
parent's own minimum, not the parent's `bits`. Anchored at a heavier parent's
`bits`, the next minimum would be as heavy as that parent, and every share
after a heavy one would have to be heavy too. Each share therefore carries its
minimum as `asert_bits` (below), which must equal `T_asert`; for a share at
the minimum it equals `bits`, and ASERT then runs exactly as before.

The check replaces the equality at the three places that make it today:

- full validation, `validate_with_pool_difficulty` in
  `shares/validation/mod.rs`;
- header sync, `validate_dag_connectivity_and_difficulty` in
  `node/p2p_message_handlers/receivers/share_headers.rs`;
- block receipt, `validate_asert` in
  `node/p2p_message_handlers/receivers/block_receiver.rs`.

A share's weight in the window and its work in fork choice are already its
exact work, `2^256 / (T + 1)`, so a heavier share counts for more without any
change there. ASERT counts shares by height and time, so when big miners mine
heavier shares the pool makes fewer shares, and ASERT lowers the minimum for
everyone else.

### What a share carries

Each Bitcoin Cash share header gains three fields:

| Field | Contents |
|---|---|
| `wanted_bits` | The target the share's node wanted for this miner before the cap: equal to `bits`, or harder when the cap stopped it |
| `band_cap` | This share's cap `C` in units of 1/1024, which must equal one step from its parent's `band_cap` (below) |
| `asert_bits` | This share's minimum `T_asert`, which must equal the ASERT target from the parent; the next share's ASERT is anchored at it |

All three are committed in the share commitment after the share version (tag
byte `0x24`, then the 4-byte compact `wanted_bits`, the 4-byte `band_cap` and
the 4-byte compact `asert_bits`), so the proof of work fixes them. `wanted_bits` must be at least as hard as `bits`; it
is a signal only and never makes a share valid or invalid beyond that. Carrying
`band_cap` in the header lets a node check it from the parent's header alone,
during header sync as well as full validation, with no walk back to genesis.
In the rule above, `C(parent)` is therefore read as the share's own `band_cap`,
which is one step from the parent's.

### The cap `C`

`C` is a fixed-point number carried from share to share, the way ABLA
carries Bitcoin Cash's block size limit from block to block. It starts at 1,
which is today's rule, and moves by at most a small step per share:

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

| Constant | Value |
|---|---|
| `C_MAX` | 64 *(sim)* |
| step | 1/512 per share, so `C` doubles in about 355 heavy shares and halves in about 355 others *(sim)* |

A share that wanted more pushes `C` up in proportion to the work its node
wanted, even while the cap holds the share itself at the minimum, and any
share that did not want more pulls it down a little. `u` comes from
`wanted_bits`, not `bits`: while `C` is 1 every share's own work equals the
minimum, so a step driven by `bits` could open the cap only when more than
half of all shares wanted heavier ones, which the simulation showed never
happens in a mixed pool. `C` therefore grows
while a large part of the pool's work is held back by the cap and settles
when the nodes' wishes fit inside it. A miner pushes `C` up only by wanting
heavier shares, which, once the band opens, it then mines, and that only
lowers the minimum for small miners; a miner that wants the minimum pulls `C`
down towards today's rule.

### Node default (policy, not a rule)

A node gives each miner the minimum target unless the miner would then make
more than `max(4 shares per pool block, 300 shares a month)`. Such a miner
gets a harder target, up to `C` times the minimum, chosen so it makes about
that many shares; `wanted_bits` records the target it would have had without
the cap. The node estimates the pool's block rate from the pool's hashrate,
the work of its newest 60 confirmed shares (ten minutes) over their time
span, against the Bitcoin Cash difficulty of the template; each miner's
hashrate is its accepted submissions over the last ten minutes, all its
connections together (`stratum/work/band_policy.rs`).

Heavier shares are there so ASERT can ease the minimum, and the minimum
cannot ease past the chain's ceiling. So no miner gets more than the room
left, the minimum's work over the ceiling's, and that bound is also what the
node wants: `wanted_bits` does not ask for more, so the band cap does not grow
for nothing. At the ceiling the room is one and every miner is at the
minimum. A pool too small for 10-second shares at the ceiling then keeps
every share it can make; without the bound its node default gave every miner
64 times the ceiling's work and cut its shares from 0.051 to 0.0008 a second
(study #85, payout simulator, tiny pool). The room shrinks smoothly as a
growing pool's minimum nears the ceiling, so ASERT settles on one minimum
rather than switching heavier jobs on and off around it.

The rule allows heavier shares; nothing forces them. A node that gives every
miner the minimum makes ASERT bring the minimum back to today's level: no
worse than now. Cash Dragon's `alpha-vardiff-v2` branch already passes a
per-miner target through jobs (`insert_miner_job`, the job's tier bits) and
checks a band at the three sites above; that plumbing is reused, and its
floor derived from the window is replaced by the ASERT minimum.

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

```text
Share 100 <-- Share 101 <-- Share 102 <-- Share 103
                ^   ^           ^   ^         ^
                |   |           |   |         |
             weak  weak       weak  uncle    weak
              a     b          c              d
```

### What a weak share is

A weak share is a proof of work on an ordinary job: its share header and its
Bitcoin Cash coinbase are exactly what a full share on that job would carry.
It is valid as a weak share when:

1. its `bits` is exactly `T_asert` of its parent: weak shares come only from
   jobs at the minimum, so a miner on a heavier band target gets no credit
   for near misses;
2. its proof-of-work hash meets the weak target of its parent (below) and
   does **not** meet its own `bits`: a hash that meets `bits` is a full share
   and must travel as one;
3. its Bitcoin Cash coinbase pays the PPLNS window anchored at its parent,
   exactly as a full share's must (checked as below);
4. it passes every header rule a full share passes except the share target:
   timestamps, the current-chain rule for its Bitcoin Cash prev, donation
   and fee caps, share version;
5. its proof-of-work hash is not the hash of any share, uncle or weak share
   already credited on the including share's chain.

A weak share is identified by its proof-of-work hash, the hash of the
Bitcoin Cash header it carries, not by its share header hash. One proof of
work is credited once, as a share or as a weak share, never both. The share
that carries it commits more than that identity: its whole encoding (below).

### Where weak shares come from

A node already accepts submissions at each connection's own difficulty
(vardiff) and turns the ones that meet the share target into shares. With weak
shares it also turns a submission that meets the weak target, on a job at the
minimum, into a weak share. For that a connection's difficulty must not be
above the weak difficulty: the cap that keeps a session's difficulty at or
below the share difficulty (lessons V8, `share_difficulty_ceiling`) becomes the
weak difficulty for miners at the minimum. SV1 and SV2 work the same way;
miners change nothing.

### The weak target

```text
T_weak(parent) = min(T_asert(parent) * R(parent), T_ceiling)
```

`R` is a second fixed-point state that each share header carries as
`weak_ratio`, committed with the weak-share root (below), and that must equal
one step from its parent's:

```text
weak_ratio:  R in units of 1/1024, from 1024 (R = 1) to R_MAX * 1024

weak_ratio(genesis) = R_MAX * 1024
weak_ratio(child)   = parent.weak_ratio - parent.weak_ratio / 256
                        if the parent carried more than WEAK_FILL weak shares
weak_ratio(child)   = parent.weak_ratio + parent.weak_ratio / 256
                        if it carried fewer
weak_ratio(child)   = parent.weak_ratio   if exactly WEAK_FILL
then clamped to 1024 .. R_MAX * 1024
```

In the formula above, `R(parent)` is the parent's `weak_ratio`: a weak share
mined on a parent meets that parent's weak target.

When home miners fill the weak slots, `R` falls and weak shares get harder;
when the slots run empty, `R` rises and they get easier. The weak difficulty
follows the home miners who use it, not industrial hashrate, and never
becomes easier than the chain's ceiling: in a small pool, where shares are
already at the ceiling, weak shares are simply never needed.

| Constant | Value |
|---|---|
| `R_MAX` | 256 (4096 until study #85) |
| `WEAK_CAP` | 64 weak shares per share *(sim)* |
| `WEAK_FILL` | 32, half the cap *(sim)* |
| `WEAK_BYTES` | 96 KiB of weak shares per share *(sim)* |
| `WEAK_DEPTH` | 48: a weak share's parent at most 48 shares below the share that carries it, 8 minutes at 10-second shares (6 until study #85) |
| `INCLUDER_CUT` | 100 basis points (1%; 500 until study #85): at `WEAK_DEPTH` 48, refusing to carry others' weak shares gains a coalition of 90% of the hashrate at most 0.06%, and costs a half-hashrate coalition 0.2 to 0.3% of its income and a lone refusing node 0.2 to 0.9% |

`WEAK_DEPTH` 48 costs a node memory and some reading. Its weak pool holds at
most 4 × 64 × 48 = 12,288 weak shares, about 18 MB at the 1.5 KiB average
`WEAK_BYTES` allows, and at most 3,072 (4.5 MiB) waiting for parents it does
not have; a share's carried-once check reads the weak shares of up to 47
ancestors. In steady state the next share carries almost every weak share,
so the pool holds far fewer. A weak share, like a share, needs only a known
Bitcoin Cash prev, so waiting does not make it stale.

### What a weak share message holds

| Field | Size |
|---|---|
| The share header, as mined | about 300 to 450 bytes |
| The Bitcoin Cash coinbase scriptSig | at most 100 bytes |
| The coinbase's total value | 8 bytes |
| The coinbase merkle branch | 32 bytes per level, 17 levels for a block of 100,000 transactions |

That is about 0.5 to 1.2 KB. The payout outputs are not carried: every node
already computes them for that parent.

### How a node checks one

1. Read the weak share's parent; it must be known and within `WEAK_DEPTH` of
   the node's tip (the node holds it in its weak pool until then, or drops
   it).
2. Rebuild the Bitcoin Cash coinbase as the node would for a share on that
   parent: the scriptSig from the message, then output 0 from the header's
   merge-mining fields, the window's payouts for the message's total value,
   the payout tree output if any, the padding, the share commitment rebuilt
   from the header, and the locktime from the header's height.
3. Hash the coinbase, fold the merkle branch, and require the header's merkle
   root.
4. Check the proof of work against `T_weak(parent)` and against rule 2.
5. Check rule 5 against the node's best chain and its weak pool.

The window's payouts for a parent and a total value are the same for every
weak share on that parent and template, so a node caches them; what remains
per weak share is hashing its coinbase, at most 1,000,000 bytes, a few
milliseconds. The simulation measures this at `WEAK_CAP` per share.

### How a share carries them

A share may carry up to `WEAK_CAP` weak shares and `WEAK_BYTES` of them in its
body. Its header commits to them (tag byte `0x26` in the share commitment, then
the 4-byte `weak_ratio`, a 2-byte count and the 32-byte merkle root of the
carried weak shares' whole encodings: each leaf is the SHA-256d of one weak
share as it is sent, header, scriptSig, total value and branch), so the proof
of work fixes the list and every byte of each weak share in it. A carried weak
share must be
valid as above, its parent at most `WEAK_DEPTH` shares below the carrying share,
and not carried by any of the carrying share's ancestors. Those checks use the
weak shares' proof-of-work hashes, read from the carried weak shares.

Why the whole encoding: the share block hash covers the header, not the body,
so a peer can relay an honest share with an altered body. The node refuses a
body that does not match its header as a bad copy and fetches the share again.
A root of the proof-of-work hashes alone left a carried weak share's other
fields unbound: a peer could raise one weak share's coinbase value by a
satoshi, or change its branch, scriptSig or coinbase proof, and the body still
matched. The node stored it, the weak share's coinbase or header check then
failed, and the node marked the honest share invalid and split from the chain,
at no cost in work to the peer. With the whole encoding committed, any such
change is a bad copy, and a weak share check that fails after the body is
stored judges what the miner committed. This changed on 2026-11-12 (protocol
segment `weak2-64-500-6`, store format 6; study #85's constants followed the
same day as `weak2-64-100-48-256`, store format 7).

A node fills its own shares from its weak pool, oldest parent first so none
expire, up to the cap. It relays a weak share once it has checked it, and
forgets it once its parent is more than `WEAK_DEPTH` below the tip or once the
best chain carries it.

### How weak shares are paid

Every proof of work from a job at the minimum is credited at the weak work,
`2^256 / (T_weak(parent) + 1)`, whether it became a full share, an uncle or a
weak share. Only a share heavier than the minimum is credited at its own work.

| Kind | Extends the chain | Credited weight | Carrying share gets |
|---|---|---|---|
| Full share at the minimum | yes | the weak work | |
| Full share heavier than the minimum | yes | its own work | |
| Uncle | no | 9/10 of the work above for its kind | 1/10 of it |
| Weak share | no | the weak work, less `INCLUDER_CUT` | `INCLUDER_CUT` of it |

Why: a hash on a job at the minimum earns credit whenever it meets the weak
target, and the hashes that meet the weak target are a fair sample of the
miner's work at the weak difficulty. Crediting a full share at its own work on
top would pay those hashes twice: per unit of work, `1` for the full shares
plus `1 - 1/R` for the weak shares. The simulation measured a Bitaxe at 1.8
times its fair pay, and industrial miners at 0.91 times, with that double
credit; with every minimum proof at the weak work, every miner is paid in
proportion to its work. Fork choice is unchanged: a full share still adds the
work of its own target to its chain. When `R` is 1, the weak work is the
minimum's work and nothing changes.

- A weak share's work is the exact work of the weak target it met,
  `2^256 / (T_weak + 1)`, not the work of its hash.
- Its weight goes to its own payout address, like an uncle's; it has no share
  coinbase and no payout right to sell. Its own donation and fee come off its
  own weight (`cuts-per-share`).
- The carrying share's part follows that share's own weight, as an uncle
  bonus does.
- For sizing the window, a share's work is its credited weight plus its
  uncles' and its weak shares' credited weights, so the window still holds 3
  BCH blocks of actual work. Weak shares take no window slots.
- Weak shares do not count in fork choice. Counting them, as uncles count
  since 2026-10-30, would make a share that drops others' weak shares lighter
  and so lose races (Sunlight's idea); the attack search in row 2 decides
  whether that is worth the extra header-sync work.

A node stores, with each share, the per-payee sums of its weak shares' credited
weight, so the window walk reads one entry per payee and share rather than
every weak share. Weak share bodies are pruned with their carrying share's body
([Pruning](../architecture/pruning.md)).

### What it costs

At `WEAK_CAP` = 64 per 10-second share, at most 6.4 weak shares a second, about
5 to 8 KB/s of relay and the same of storage while the shares are within the
pruning depth. Spam is bounded by the cap and costs real work at the weak
difficulty, which is paid like anyone's work and raises the weak difficulty
through `R`.

## 4. Payout levels

A payout level is the smallest payment a miner wants. A slice below it is paid
by a draw among the miners at the same level, within the same block, so
nothing is held and nothing is owed.

```text
slice of a payee ---> at or above its level? --- yes ---> paid directly
                          |
                          no
                          v
             pooled with the slices at the same level
                          |
                          v
             draw seeded from the parent share's hash
                          |
                          v
             winners get the pot in equal parts of at least the level
```

### Levels

```text
L_k = 546 * 2^k satoshis,  k = 1 .. 7
    = 1,092, 2,184, 4,368, 8,736, 17,472, 34,944, 69,888
```

546 is Bitcoin Cash's P2PKH dust limit, the floor. 69,888 is the cap, which
keeps large amounts out of any draw. Each Bitcoin Cash share header gains a
field, encoded after the band and committed after the band (tag byte `0x25`,
then one byte); a header above 7 does not decode:

| Field | Contents |
|---|---|
| `payout_level` | `k`, 1 to 7 |

A payee's level in a block is the `payout_level` of its newest credited entry in
the window: the newest share, uncle or weak share paying that address. The
walk visits window shares newest first; an uncle is placed at the share that
includes it, as the window credits it, and a share is newer than the uncles
it includes, which are its ancestors. A weak share is placed at the share
that carries it, after that share's uncles, and has its own header's
level. The donation key and fee addresses, every recipient of a donation or
fee cut in the window, are never drawn, even when they also mine.

### The draw

After the window's weights, cuts and the existing rounding give each payee an
amount `a` in satoshis:

1. Every payee whose amount is at least its level, the donation key and every
   fee address keep their amount.
2. The others are grouped by level. Within a group at level `L`, payees are
   ordered by locking script bytes, and `S` is the sum of their amounts.
3. `n = S / L`, rounded down.
4. The seed is the first 8 bytes, as a little-endian integer, of
   `SHA-256("p2pool-payout-level" || parent share hash || k)`: the parent
   share hash is the 32 bytes of the paying share's `prev_share_blockhash` as
   its header serializes them, and `k` one byte.
5. If `n` is 0, one point `p = seed mod S` picks the winner: the payee whose
   running total of amounts first exceeds `p`. It gets `S`.
6. Otherwise `D = S / n` rounded down, points `p_j = (seed mod D) + j * D` for
   `j = 0 .. n-1`, and each point pays `D` to the payee whose running total
   first exceeds it. The first point's payee also gets `S - n * D`, which is
   less than `n` satoshis.
7. Each payee's winnings replace its amount; a payee that wins nothing is not
   paid in this block. The outputs are then ordered and built as today,
   including the payout tree: by weight, as before the draw.

Every payee's expected payment equals its amount, up to the at most `n`
satoshis of step 6; the draw changes only how a group's pot is split, never
its size. Every node computes the same draw, because the seed comes from the
parent share, which every share built on that parent shares.

### Choosing a level

A miner sets its level at login, in the password or after a comma, like
`donation=`:

| Option | Meaning |
|---|---|
| `min-payout=<satoshis>` | The smallest level at or above this amount |
| `fee-tolerance=<percent>` | The level that keeps the cost of spending one payment at or below this share of it, using the node's `spend_cost` (148 satoshis by default, a P2PKH input at 1 sat/byte) |

Without either, the node's default applies: level 5, 17,472 satoshis, about
0.85% fee loss. A node warns a miner, and lowers its level, when its window
work makes fewer than 3 payments a month likely at the level it asked for
(node policy, not a rule). The dashboard shows each miner its expected
income, payments a month and monthly swing.

As built: `min-payout=` wins when both are given; a `fee-tolerance=` no level
meets gives level 7. The node estimates a miner's payments a month from its
hashrate, the pool's (from its newest 60 confirmed shares) and the
template's coinbase value, as the band's node policy sizes heavier jobs, and
logs the warning once per connection. A Job Declaration token carries the
level its identity chose, or the node's, unlowered. The `/miner` API and the
dashboard's miner page show the miner's level and its expected income,
payments a month at its level and monthly swing. Income is its payout
fraction of the window times the template's coinbase value on each of the
pool's blocks a month. The swing is
`sqrt(1 / credits a month + 1 / pool blocks a month)`, with
`1 / payments a month` added under the root when its slice of a block is
below its level; its credits a month are its shares, uncles and weak shares
in the window over the window's time span.

### Steering the seed

The seed comes from the parent's hash, which the share that pays cannot
change. A miner could prefer one parent over another to get a better draw,
but only by mining away from the best tip, at the risk of being orphaned. The
attack search in row 2 measures what that is worth.

## 5. Dynamic share period (outline)

The share period moves with the uncle rate: shorter while the network keeps
up, longer when uncles rise. It is specified after row 6 measures the uncle
rate on chipnet. What it must settle:

- `IDEAL_BLOCK_TIME` becomes a value carried from share to share, bounded,
  for example 1 to 10 seconds, moved by the uncle rate over a span of
  shares;
- the window's cap and the maturity skip become spans of time, not share
  counts, so they keep their meaning at any period;
- `WEAK_DEPTH` and the uncle depth scale with the period;
- it ships through the share-version vote, version 2.

### What the uncle rate already shows

The live chipnet pool, measured from node1's API on 2026-11-08 over the
8,700 heights below share 64,454 (about 24 hours): 8,700 confirmed shares, 257
uncles (2.95% of confirmed shares), every one named, none orphaned, at an
average interval of 9.99 seconds. All of it is one miner on two nodes on one
PC, so it measures local races, not network latency.

The network simulation ([Network simulation](../simulation/network-sim.md))
measured the stale rate, uncles and orphans together, against the share
interval on 100 nodes:

| Share interval | Stale, 3 KB coinbase | Orphaned | Stale, 200 KiB coinbase | Orphaned |
|---|---|---|---|---|
| 1 s | 15.9% | 0.1% | 45.8% | 9.8% |
| 2 s | 8.7% | 0.0% | 29.5% | 1.1% |
| 5 s | 3.8% | 0.0% | 14.1% | 0.1% |
| 10 s | 2.1% | 0.0% | 7.8% | 0.0% |
| 30 s | 0.5% | 0.0% | 2.7% | 0.0% |

The live 2.95% at 10 seconds sits where the simulation puts it. So by lost
work alone the share period's lower bound is set by the coinbase size: about 1
second for small coinbases (0.1% orphaned there), about 5 for large ones (1.1%
at 2 seconds). No simulated share ever had a fourth uncle to name, so the
three-uncle limit loses nothing. Holding the stale rate at today's on 1,000
nodes is stricter: 8.9 seconds today, 5.0 to 3.5 with the scaling ideas
combined ([Network simulation](../simulation/network-sim.md), "Scaling
propagation instead of capping it"). Below that, weak shares carry the extra
credit instead: gossiped beside shares they never race, so they add credit
without adding stale shares.

## Rollout

| Part | Protocol tag segment | Store format | How it ships |
|---|---|---|---|
| 1 One chain | the mini chain's genesis tag only | 3 | flag day: every node restarts on a fresh store |
| 2 Band | `band-c64` | 3 | same flag day |
| 3 Weak shares | `weak-64-500-6`, then `weak2-64-500-6`, then `weak2-64-100-48-256` | 5, then 6, then 7 | same flag day; old nodes cannot read the new header or the weak share message; the weak root's leaves then became whole encodings, a second flag day, and study #85's cut, depth and `R_MAX` a third |
| 4 Payout levels | `levels-546x2` | 4 | same flag day; old nodes cannot read the longer header |
| 5 Share period | none; share version 2 | 3 | share-version vote |

A part that changes what a stored share holds raises the store format when
it merges, so the numbers follow the merge order: part 4 raised it to 4
(2026-11-09), because its header carries the payout level, and part 3 to 5
(2026-11-09), because its header carries the weak ratio and the store keeps
the weak shares. The weak root's binding of each carried weak share's whole
encoding raised it to 6 (2026-11-12): stored shares were checked against the
old root, which every node now computes differently.

Each part is its own pull request, with its plain-words explanation and the
maintainer's agreement before merging. Chipnet restarts once per merged part
until mainnet launches; mainnet starts with all four. The share version stays
1 on the merged chain, so the vote stays free for part 5.

## Simulation and attack search (row 2)

An offline Rust model, the crate `p2poolv2_payout_sim` (`cargo payout-sim`;
its README has every table), that mines shares, uncles and weak shares with
Poisson timing, sizes the window by work, pays found blocks by weight, runs the
draw of section 4, and reports each miner's payments a month, typical payment,
monthly swing and fee loss, and the pool's shares, uncles and weak shares per
second. It runs:

- the mini chain as today, the band, Cash Dragon's vardiff chain, and the
  band with weak shares and payout levels;
- pools at 1, 5, 10, 20 and 30% of Bitcoin Cash's hashrate, and Bitcoin
  Cash's hashrate at 1, 3, 10 and 270 times today's;
- miner mixes from all home miners to 90% industrial.

Attacks:

- a big node that gives its miners the minimum;
- a node that never carries others' weak shares;
- weak-share spam at the weak difficulty;
- a miner that picks parents for a better draw.

It sets `C_MAX`, the band step, `R_MAX`, `WEAK_CAP`, `WEAK_FILL`,
`WEAK_DEPTH`, `INCLUDER_CUT`, `WEAK_BYTES` and the default level.

### What it found

A Bitaxe (1.2 TH/s) in a pool where half the hashrate is industrial. Each cell
is monthly swing / payments a month / fee loss:

| Design | Pool at 1% of BCH | 10% | 30% |
|---|---|---|---|
| Main and mini today | 0.34 / 19.7 / 0.67% | 0.91 / 4.9 / 0.17% | 1.58 / 1.6 / 0.06% |
| One chain, no band | 0.41 / 19.5 / 0.67% | 1.31 / 2.4 / 0.09% | 2.39 / 0.76 / 0.03% |
| Band | 0.32 / 29.8 / 1.02% | 1.09 / 3.2 / 0.11% | 2.39 / 0.76 / 0.03% |
| Cash Dragon's vardiff chain | 0.31 / 30.1 / 1.03% | 0.90 / 4.8 / 0.17% | 1.61 / 1.6 / 0.06% |
| Band, weak shares and payout levels | 0.20 / 24.9 / 0.85% | 0.25 / 24.6 / 0.85% | 0.39 / 23.6 / 0.83% |

The last row credits every minimum proof at the weak work, as above. The
simulation found the double credit that rule removes.

Attacks:

- **Big nodes giving their miners the minimum.** Under the band alone, the
  Bitaxe falls back to the one-chain numbers. With weak shares and the
  weak-work credit, nobody's income changes.
- **Nodes that never carry others' weak shares.** At the first constants
  (`WEAK_DEPTH` 6, a 5% cut), with half the hashrate refusing, 4.8% of honest
  weak shares expired and the Bitaxe got 0.97 of its fair pay; with 90%
  refusing, 56% expired and it got 0.48, and no tested cut stopped that
  coalition. Study #85 found that a deeper `WEAK_DEPTH` does: at 48, with a 1%
  cut, 1.4% expire with 90% refusing, the Bitaxe gets 0.99 of its fair pay,
  and the coalition gains at most 0.06%. Those became the rules on
  2026-11-12 (`weak2-64-100-48-256`, store format 7).
- **Weak-share spam at 10 times the honest home hashrate.** The Bitaxe's weak
  shares fall from 581 to 53 a month, but its payments stay at about 25 a
  month and its swing only goes from 0.20 to 0.23.

Measured at scale (2026-11-12; `docs/simulation/network-sim.md`, "Limits
study", and the payout simulator's `scale` table):

- **Checking a share** at a 120,960 share window and the default level costs
  a node 6.4 ms with 1,000 miners in the window, 38 ms with 10,000 and 362
  ms with 100,000; a weak share 0.3, 3.8 and 45 ms. At 10-second shares
  100,000 miners keep a node 18% busy.
- **A window full of weak shares** costs little more to walk: 14 ms for
  1,800 shares carrying 32 weak shares each with 5,100 payees, 174 ms with
  50,100 payees, and 160 ms at the cap of 120,960 shares with 6,000 payees.
  The time follows the number of payees, about 3.5 microseconds each, not
  the number of credits.
- **How often a small miner is paid** falls with the number of home miners
  in a pool, not with its hashrate: a pool makes 3.2 weak shares a second,
  whatever its size. A bigger pool also pays in bigger pieces, because its
  window is shorter: one weak share is worth 24,009 satoshis a block at 50%
  of Bitcoin Cash.

Still open:

- **Level 0 is gone (2026-11-12).** A level 0 win, 546 satoshis, that
  landed in the payout tree was below dust once the tree's fee was taken,
  so the tree gave it to its largest payee
  (`payout_tree::merge_dust_payees`). On a 12-node swarm with 1,200 miners
  (`docs/simulation/RUNBOOK.md`, Test J) the miners at level 0 were paid
  0.79 to 0.82 of their part of the hashrate on token shares, the tree's
  largest payee eleven times what its work earned, and the same miners 1.00
  with level 1 in place of level 0. The lowest level a share may carry is
  now 1, 1,092 satoshis (`MIN_PAYOUT_LEVEL`, protocol segment
  `levels-546x2-k1`): it stays above dust after the tree's fee at 3
  satoshis a byte. What remains is small: a level's pot below one level
  payment is paid whole to one payee, and a miner alone below its level
  keeps its own slice, each under 1,092 satoshis a block, paid as it is in
  the coinbase and merged in the tree.
- **Every weak share is sent whole to every peer.** A node sends each weak
  share it accepts, as its own request, to every peer but the one it came
  from, so a node with eleven peers receives it eleven times: the 12-node
  swarm took 719,915 weak shares from peers and refused 7,122,362
  duplicates. At the start of a chain, where `R` starts at `R_MAX` and the
  share difficulty is still below the hashrate, that was more requests than
  the streams hold: 136,129 inbound streams were dropped in the first
  minute, and none after. Announcing a weak share by its hash, as
  transactions are, sends each one once. Node behaviour, not a rule.
- **A carried weak share's timestamp is checked against the node's own
  clock.** `validate_carried_weak_headers` applies the 120-second future
  limit of a relayed weak share to the ones a share carries, and a failure
  marks the share invalid for good, so two nodes whose clocks differ can
  disagree about one share for ever. The limit for a carried weak share has
  to come from chain data, the carrying share's own time. A rule change.
- **A weak share that passes proof of work and fails a later check can be
  sent again and again**, each time costing a window walk: the failure is
  not remembered, the peer is not dropped, and the walk cache empties
  itself when it is full. Node behaviour.
- **Not yet simulated:** the band step.

## References

- [Issue #79](https://github.com/CyberAshven/BitcoinCash-p2poolv2/issues/79),
  with the numbers and the research behind this design.
- [Share validation](validation.md), [Share commitments](share-commitments.md),
  [PPLNS accounting](pplns-accounting.md),
  [Coinbase payouts](../payouts/coinbase.md),
  [Share difficulty: ASERT](../difficulty_adjustment/asert.md).
- [Lessons from P2Pool v1 on BCH](../p2pool-v1-bch-lessons.md), V6 and V7.
- [Small miners](../small-miners.md).
