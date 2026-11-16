# p2poolv2_payout_sim

An offline Monte Carlo of payouts under the Bitcoin Cash share chain designs
of [Share chain redesign for payout variance and fees](../docs/sharechain/small-miners-design.md)
(issue #79, row 2). It measures, for a 1.2 TH/s Bitaxe and a 100 TH/s ASIC,
how often they are paid, how large a typical payment is, how much their
monthly income swings, how often a month brings nothing and what spending
the payments costs, under each design, pool size, miner mix and Bitcoin Cash
hashrate. It also reports the pool's share, uncle and weak share rates and
bytes a second, runs three attacks, and runs study #85 (weak work in fork
choice, miner mixes, arrivals, a tiny pool, floods, draw steering, losses
and the constants `WEAK_DEPTH`, `INCLUDER_CUT` and `R_MAX`).

It is not a network simulation: stale rates come from
[the network simulation](../docs/simulation/network-sim.md), and nothing
here sends messages.

## Run it

```sh
cargo payout-sim                           # every table, in Markdown
cargo payout-sim --scenario attack-spam    # one table
cargo payout-sim --months 200 --seed 7     # fewer months, another seed
cargo payout-sim run --design band --pool-fraction 0.1 --mix 90-industrial
cargo payout-sim run --help                # every input of one run
```

`cargo payout-sim` is an alias (in `.cargo/config.toml`) for
`cargo run --release -p p2poolv2_payout_sim --bin payout-sim --`. The tables
are `designs`, `hashrate`, `pool`, `weak-credit`, `band-cap`,
`weak-constants`, `includer-cut`, `levels`, `period`, `attack-minimum`,
`attack-carriers` and `attack-spam`, and study #85's `refusing-races`,
`mixes`, `arrivals`, `tiny-pool`, `floods`, `draw-steering`, `losses`,
`weak-depth`, `r-max` and `r-max-mixes`, and `scale`, `scale-levers`,
`band-step`, `window-cap` and `storage`, and the proposal's `weak-asks`,
`weak-asks-mixes`, `weak-asks-constants` and `weak-asks-attacks`, and the
study of its path, `weak-asks-genesis`, `weak-asks-big-miner`,
`weak-asks-rules`, `weak-asks-rule-mixes` and `weak-asks-rule-attacks`.
Every run is a pure function of its inputs and seed (default 1), drawn from
`p2poolv2_netsim`'s seeded xoshiro256** generator, so the tables below come
out the same on any machine. With one-minute blocks a simulated month holds
ten times the blocks it did, and a tracked miner paid in every block keeps
every payment for its median, so one scenario can hold a few hundred
megabytes: the whole default run, 1,000 simulated months per scenario, took
RUNTIME on the 32-thread machine it was run on (27 seconds with ten-minute
blocks), and needs several gigabytes of memory.

Since 2026-11-15 the model runs on one-minute Bitcoin Cash blocks
(CHIP-2025-03): a block every minute, paying a tenth of a ten-minute
block. A month's reward and a miner's fair income are unchanged; the window
is a tenth as long, the pool finds ten times the blocks, and the node
default aims a heavy miner at ten times the shares a month. The tables and
the prose under "What the tables show", "What the study tables show",
"Scale" and "Scale levers" are from that model; the ten-minute tables are
in this file at commit 05de7fa5. Since 2026-11-12 the model runs with study
#85's constants, which became the share-chain rules that day: `WEAK_DEPTH`
48, `INCLUDER_CUT` 1% and `R_MAX` 256. "Recommended constants" keeps the
first study's reasoning, from ten-minute blocks and the first constants
(`WEAK_DEPTH` 6, `INCLUDER_CUT` 2% or 5%, `R_MAX` 4,096), whose tables are
in this file at alpha eb9b1ee3.

## The model

### Bitcoin Cash and the pool

- Bitcoin Cash's hashrate `H` is 3.7 EH/s (`docs/small-miners.md`), or 3,
  10 or 270 times that; a block every minute (CHIP-2025-03, on chipnet since
  2026-11-15 and on mainnet from 2027-05-15; `BCH_BLOCK_INTERVAL_SECONDS`);
  0.3125 BCH a block, a tenth of a ten-minute block's 3.125 BCH, fees left
  out; 1% donation on every share (`cuts-per-share`, so a payee gets 99% of
  its slice).
- The pool holds a fraction `f` of `H` (1, 5, 10, 20 or 30%) and finds
  Bitcoin Cash blocks as a Poisson process at `f / 60` a second. Under
  `mini` each chain finds blocks at its own part of `H`.
- A month is 30 days (43,200 one-minute Bitcoin Cash blocks). A miner's fair
  income is its share of `H` of a month's blocks, less the donation: 433,459
  sat for the Bitaxe today, the same as with ten-minute blocks.

### Miners

- Mixes: `home` (every miner a 1.2 TH/s home miner), `50% industrial` and
  `90% industrial`. The industrial part is split equally by hashrate between
  1 PH/s and 10 PH/s machines, each its own miner (so its own address and
  node default). Counts may be fractional: a class stands for its share of
  the hashrate.
- Two tracked miners are always in the pool, inside its hashrate: the
  Bitaxe (1.2 TH/s) and the ASIC (100 TH/s), both at payout level 5
  (17,472 sat) unless a table says otherwise. The carrying-node tables add
  a third, the ASIC on a node that never carries others' weak shares.

### Share chain, in steady state

- ASERT holds the chain at one chain share per share period (10 s by
  default): the minimum difficulty is solved so that the chain shares all
  miners make at their own difficulties add up to one per period, never
  easier than the chain's ceiling (`MINI_ANCHOR_TARGET`, about 4.55; main
  chain `MAX_POOL_TARGET`, about 1,164).
- A fraction `s` of the shares found go stale and become uncles, paid 9/10
  to their miner and 1/10 to the share that names them. `s` is the network
  simulation's for small coinbases: 2.1% at 10 s, 3.8% at 5 s, 8.7% at 2 s,
  15.9% at 1 s. A stale share does not grow the chain, so `1 / (1 - s)`
  shares are found per chain share.
- The window: walk back 100 chain shares (maturity), then take shares until
  their work, uncles' and weak shares' included, reaches 3 Bitcoin Cash
  blocks of work, at most 120,960 shares at 10 s and 14 days of shares at
  any other period. In steady state that is a fixed span of time: three
  minutes over `f`, 5 hours at 1% of BCH (50 hours with ten-minute blocks).
  Only pools under about 0.015% of BCH reach the cap.
- A block pays each payee its credited work in the window over the
  window's work, rounded down to a satoshi, after its 1% donation.
  Payments below dust are paid as they are, as the hybrid payout rule does.

### The designs

1. `mini`: today's rule. Every share on a chain at that chain's one
   difficulty; miners under 10 TH/s on the mini chain, the rest on main, each
   chain with its own blocks and window.
2. `one-chain`: one chain at one difficulty.
3. `band`: one chain with the difficulty band at a fixed cap `C` (64 unless
   the band cap table says otherwise). The node default gives a miner the
   minimum unless it would then make more than
   `max(4 shares per pool block, 300 a month)`; then it gets `min(C, ratio)`
   times the minimum, sized to make that many. With one-minute blocks 4
   shares a pool block passes 300 a month from 0.17% of BCH (1,728 a month
   at 1%, 17,280 at 10%), so the band leaves far fewer miners above the
   minimum than with ten-minute blocks. The table also reports where
   the page's ABLA-like rule for `C` would settle: scanning `C` up from 1,
   the first `C` where the expected step per share,
   `p * floor(C) - (1 - p)` with `p` the share of chain shares from miners
   that wanted more than the cap, is no longer upwards.
4. `vardiff-cd`: Cash Dragon's chain (`docs/sharechain/vardiff-sharechain.md`
   on `alpha-vardiff-v2`). Each miner's tier is `w_i * D_asert`, its share of
   the hashrate times the ASERT difficulty, never easier than the ceiling, so
   every miner makes shares at the same rate. The floor,
   `W_min / W_total * D_asert` over the 120,960-entry floor window with
   presence `N` = 3, is the smallest miner's own tier and binds for nobody,
   unless miners are so many (more than about 40,000) that the smallest holds
   fewer than 3 entries. Then the floor is set by a small miner that happens
   to hold exactly 3, which holds only where `D_asert` is the floor window's
   work over 3; the floor settles where the chain makes one share per period.
5. `band-weak`: the band and weak shares, every payment direct.
6. `band-weak-levels`: the band, weak shares and payout levels.

`band-weak-levels, weak work` is design 6 with one change the page does not
make, explained under the first result below.

### Weak shares

Only miners on jobs at the minimum make weak shares: hashes that meet the
weak target `T_asert * R` but not the share target, `R - 1` of them per full
share. The pool's weak shares are simulated share by share (400,000 chain
shares after 20,000 to settle): `R` moves by 1/256 per share as the page's
fixed-point rule moves it, from `R_MAX` = 4096 at genesis; weak shares are
found during the exponential time between two chain shares; a new share
carries up to `WEAK_CAP` of them, oldest parent first, and a weak share whose
parent is more than `WEAK_DEPTH` = 6 shares below expires uncredited. A
share from a non-carrying node carries only its own node's weak shares. A
weak share is credited its weak work, `INCLUDER_CUT` (2%) of it to the
carrying share's miner; weak work counts in the window's work and takes no
window slots.

### Payout levels

The draw is section 4 of the page exactly (`levels.rs`): levels
`546 * 2^k`; the payees below their level grouped by level; `n = S / L`
parts of `D = S / n`; points `seed mod D + j * D`; the first point's payee
also gets `S - n * D`; with `n` = 0 one point `seed mod S` takes the pot.
Each block's seed is a fresh 64-bit draw from the seeded generator. The
other payees in a tracked miner's group are represented by the satoshis in
front of and behind it in the group's order: the draw only sees running
totals, so the tracked miner's outcome is exactly what it would be among
the real payees. Their pot is estimated once per scenario from each class's
amounts in a window (Poisson counts of shares, uncles and weak shares).

### What is simulated and what is taken at its expectation

The tracked miners' shares, uncles and weak shares are Poisson processes,
simulated one by one, and so are the blocks. The rest of the pool is taken
at its expected work in each window: a block pays a tracked miner its
actual work over the rest's expected work plus the tracked miners' actual
work. Where a tracked miner would hold more than 2,000 entries of one kind
in a window (the ASIC alone on the main chain in the `home` mix), those
entries are credited at their expected work. Expected bonuses (a share's
1/10 of the uncles it names, its includer cut) are credited per chain share
at their mean. Fees: each payment costs `spend_cost` = 148 sat to spend;
fee loss is payments times 148 over income.

### Left out

- Network effects beyond the stale rate: the share chain's own races
  (except in the study tables that race shares), orphans, propagation and
  the uncle depth limit.
- The ABLA-like cap and the weak ratio as dynamics over time: `C` is fixed
  per run; `R` is simulated in the weak share queue but the tracked miners
  use its time average. The study's `arrivals` and `r-max` tables run both
  share by share.
- `WEAK_BYTES` and the band step were not swept.
- Transaction fees, payout trees, share trading, operator fees.

## Results

All tables are `cargo payout-sim` with the defaults above, 1,000 months per
scenario, seed 1. Columns: payments a month (blocks paying the miner an
output); median payment; monthly swing, the standard deviation of monthly
income over its mean; the share of months with no payment; fee loss; and
income over fair income (`vs fair`, simulated unless a column says
expected). A difference of a few percent in `vs fair` between rows is noise
where payments are rare (the larger pools and the x270 rows).

### What the tables show

1. **The page's weak share credit pays a miner at the minimum about twice
   per hash.** A full share is credited its full work and each weak share
   its weak work besides, so a hash on a minimum job earns `2 - 1/R` times
   what a hash on a heavier job earns. With 90% of the hashrate on heavier
   shares at 1% of BCH, the Bitaxe gets 1.81 times its fair income and the
   farms and the ASIC 0.91; at 5% of BCH, where the ASIC is at the minimum
   too, both get 1.28 and the 10 PH/s farms pay for it. From 10% of BCH no
   miner of any mix mines above the minimum, so the double credit moves
   nothing and every miner gets fair (with ten-minute blocks it still did
   at 10%). The window also shrinks, to 3.4 hours instead of 5 at 1% (50%
   industrial). It makes ignoring the node default profitable (attack a
   below): a miner on heavier shares, at 0.91 to 0.92 of fair, is paid fair
   again at the minimum. Crediting a share on a minimum job the weak work
   of its parent's weak target, like a weak share (`weak work` rows), makes
   every hash count once: every miner is within noise of fair, the window
   is 5 hours again, and the Bitaxe's monthly swing at 10% and 30% of BCH
   falls from 0.72 and 1.19 to 0.27 and 0.39, because its income no longer
   comes half in rare full-share lumps. This is a share-chain rule change
   the page does not make; it is the owner's decision.
2. **Bitaxe, 50% industrial** (swing / payments a month / fee loss):

   | Design | 1% of BCH | 10% of BCH | 30% of BCH |
   |---|---|---|---|
   | mini | 0.29 / 45.8 / 1.57% | 0.92 / 4.68 / 0.18% | 1.57 / 1.54 / 0.06% |
   | one-chain | 0.39 / 24.9 / 0.86% | 1.28 / 2.37 / 0.09% | 2.28 / 0.80 / 0.03% |
   | band | 0.30 / 45.1 / 1.55% | 1.28 / 2.37 / 0.09% | 2.28 / 0.80 / 0.03% |
   | vardiff-cd | 0.30 / 48.2 / 1.66% | 0.89 / 4.92 / 0.18% | 1.55 / 1.57 / 0.06% |
   | band-weak | 0.17 / 405 / 10.44% | 0.73 / 45.3 / 1.59% | 1.27 / 15.1 / 0.54% |
   | band-weak-levels | 0.22 / 32.6 / 0.84% | 0.72 / 13.4 / 0.47% | 1.19 / 12.1 / 0.44% |
   | band-weak-levels, weak work | 0.20 / 25.2 / 0.85% | 0.27 / 24.8 / 0.85% | 0.39 / 24.0 / 0.84% |

   Without weak shares a Bitaxe at 30% of BCH has no payment in 78% of
   months on one chain, 60% on mini and 61% under vardiff-cd. With weak
   shares and levels, and the weak-work credit, it is paid about 24 times
   every month at every pool size, each payment about the 17,472-sat level,
   with 0.85% fee loss, which is 148 over the level. With one-minute blocks
   a share stays in the window a tenth as long but meets as many blocks,
   three on average, each paying it the same slice (a tenth of the reward
   over a tenth of the window's work), so a small miner's shares share a
   block less often: on one chain the Bitaxe is paid 24.9 times a month at
   1% of BCH instead of 19.5, in smaller payments, at the same swing.
3. **ASIC, 50% industrial** (same columns): one-chain 0.06 / 430 / 0.18% at
   1%, 0.13 / 210 / 0.09% at 10%, 0.23 / 71 / 0.03% at 30%; band-weak-levels
   with the weak-work credit 0.05 / 432 / 0.18%, 0.03 / 1,967 / 0.81%,
   0.04 / 1,906 / 0.78%. With one-minute blocks the ASIC's slice of a block
   falls below its 17,472-sat level once the pool passes about 5% of BCH,
   so the level draw pays it about 1,900 to 2,000 times a month, each about
   the level, at 0.8% fee loss, where ten-minute blocks paid it once a block
   (431 and 1,039 a month at 0.18% and 0.43%). Under vardiff-cd every miner
   makes shares at the same rate, so the ASIC gets about 19 payments a month
   at 5% and above and its swing rises to 0.46 at 10% and 30%. Under `mini`
   in the `home` mix the ASIC is alone on the main chain and paid only when
   it finds a block itself: 1.17 payments a month.
4. **The band alone** helps only where heavy miners would make more than
   the node default's target, and with one-minute blocks that target is ten
   times higher: 4 shares a pool block, 1,728 a month at 1% of BCH and
   17,280 at 10%. At 1% of BCH with 90% industrial the minimum falls to 0.12
   of the uniform difficulty and the Bitaxe's swing from 0.39 to 0.14; with
   50% industrial to 0.54 and from 0.39 to 0.30. At 10% and 30% no miner is
   above the target and the band changes nothing (with ten-minute blocks it
   still lowered the minimum at 10%). **The page's ABLA-like rule for `C`
   leaves 1 only when more than half of all chain shares want heavier
   shares** (at `C` = 1 every share that does not want more pulls `C` down
   as much as a capped one pushes it up). It settles at the limit with 90%
   industrial at 1% of BCH, and stays at 1.00 in the 50% industrial mix at
   every size, where a fixed cap of 8 lowers the minimum to 0.57 of uniform
   at 1%; at 10% and 30% no cap changes anything.
5. **Weak shares cost about 2.9 KB/s** of messages at `WEAK_CAP` 64 and
   `WEAK_FILL` 32, against 0.1 KB/s of shares, at any pool size: the weak
   ratio settles at about 34.5 weak shares per share (the rule tracks the
   median carried count, which sits a little above `WEAK_FILL`), and a
   quarter of shares carry the full 64 with the rest carried later; none
   expire.
6. **Shorter share periods** help one chain (Bitaxe swing at 10% of BCH 1.28
   at 10 s, 0.36 at 1 s, with 1.2 shares and 0.19 uncles a second) but add
   little once weak shares carry the credit (0.27 at 10 s, 0.20 at 1 s with
   the weak-work credit, at ten times the bytes).
7. **Attack (a), the farms' node at the minimum:** under the band the
   minimum returns to uniform and the Bitaxe gets one-chain's numbers, no
   worse than today, as the page says. With weak shares and the page's
   credit the farms and the ASIC, on heavier shares at 0.91 of fair, are
   paid fair once the farms mine at the minimum (with ten-minute blocks the
   ASIC was still heavier and fell to 0.51); with the weak-work credit
   nobody's income changes and the Bitaxe's swing rises only from 0.20 to
   0.21 (1%). At 10% and 30% nobody is above the minimum and the attack
   changes nothing.
8. **Attack (b), non-carrying nodes**, at the rules' `WEAK_DEPTH` 48 and 1%
   cut: with half the hashrate on them no honest weak share expires; with
   90%, 1.2% (1% of BCH) and 1.4% (10%), and the honest Bitaxe gets 1.00
   and 0.99 of fair under the weak-work credit. At the first `WEAK_DEPTH` of
   6 the `weak-depth` table shows 4.7% and 55% expiring and the Bitaxe at
   0.97 and 0.49: weak shares are most of its credit, so losing them costs
   it income, not only variance. A lone non-carrying ASIC loses its
   includer income (0.99 of fair at a 1% cut, 0.98 at 2%, 0.95 at 5%, at 1%
   and at 10% of BCH). A coalition of half the hashrate loses 0.18% (1% of
   BCH) and 0.35% (10%) by not carrying at the 1% cut, one of 90% gains
   0.03% and 0.09% (`refusing-races`, share work).
9. **Attack (c), weak share spam** (one miner at the minimum with 1x or 10x
   the honest home hashrate, added to the pool): `R` hardly moves (36 to 39
   with 50% industrial, 41 to 45 with 90%), since chain shares already came
   mostly from miners at the minimum, but the spam raises the minimum
   itself, so the honest Bitaxe makes fewer full shares and so fewer weak
   shares: they fall 11 times (579 to 51 a month at 1% of BCH, 50%
   industrial). With the weak-work credit its payments go from 25 to 24 a
   month, its swing from 0.20 to 0.23 and its income stays fair.
   The spam is paid for its work and takes nothing else.

### Recommended constants

The first study's reasoning, from ten-minute blocks and the first
constants. The one-minute tables agree where they repeat it: from `C_MAX`
30 to 64 the minimum falls from 0.13 to 0.12 of uniform and the swing from
0.15 to 0.14; `WEAK_CAP` 16/8 leaves 7.5% of months empty at 30% of BCH,
128/64 lowers the swing to 0.31 and a fill of 48 gives 0.36; level 6
halves the fee loss and the payments and raises the swing from 0.27 to
0.34 at 10%. With one-minute blocks the cap matters only below about 5% of
BCH, where heavy miners still want heavier shares.

- `C_MAX` = 64: from 30 to 64 the minimum falls from 0.13 to 0.11 of
  uniform (1% of BCH, 90% industrial) and the Bitaxe's swing from 0.20 to
  0.19, and 256 adds 0.01 more. The cap rule must change before the cap
  matters: as written it stays at 1 unless more than half of the chain
  shares want heavier shares.
- `WEAK_CAP` = 64, `WEAK_FILL` = 32: with the weak-work credit the Bitaxe
  is paid every month at 30% of BCH (swing 0.39) for 2.9 KB/s; 16/8 leaves
  8.5% of months empty and 128/64 lowers the swing to 0.31 for 5.6 KB/s; a
  fill near the cap (48 of 64) puts 40% of shares at the cap for a swing of
  0.36.
- `INCLUDER_CUT` = 5%: home miners at the minimum get it back as includers
  (the Bitaxe stays at 1.00 of fair at every cut), a lone non-carrying
  node loses 5%, and it is the smallest tested cut at which a half-hashrate
  coalition at 10% of BCH gains nothing by not carrying. No cut stops a
  heavy coalition at 1%, so counting weak shares in fork choice is worth
  the attack search. Study #85 later
  moved the cut to 1% with `WEAK_DEPTH` 48 (below).
- Default level 5 (17,472 sat): 25 payments a month and 0.85% fee loss at
  every pool size with the weak-work credit; level 6 halves the fee loss
  but halves the payments and raises the swing from 0.25 to 0.32 at 10%.
- The weak-work credit for shares on minimum jobs, without which the
  constants above do not reach their numbers and every miner prefers the
  minimum.

## Study #85: weak work in fork choice, with a smaller includer cut

The tables named `Study` below answer the study on the design page
(`docs/sharechain/small-miners-design.md`, "Study: weak work in fork choice,
with a smaller includer cut", on `docs/weak-work-fork-choice`). They run the
design as the page now has it: the band, weak shares and payout levels,
every proof on a job at the minimum credited at the weak work (`weak work`
above), `INCLUDER_CUT` 1% (5% before the rules changed) unless a table
sweeps it, 50% industrial unless a table says otherwise.

### What the study adds to the model

- **Share races** (`refusing-races`, `weak-depth`, `losses`; `src/races.rs`).
  The weak share queue races shares instead of taking a flat stale rate: at
  each share, with chance `s / (1 - s)`, a second share is found on the same
  parent, each racer drawn by the shares its class finds. Both carry what
  their nodes carry from the same queue. Fork choice picks the winner by
  chain work: the share's own work (`share work`, today's rule), plus the
  work of the weak shares it carries, each once, at the weak ratio
  (`weak work`, the study's rule). Equal work is a coin toss. The loser is
  an uncle at 9/10 of its credit; its weak shares stay waiting, as the page
  has nodes keep a weak share until the best chain carries it. Each class's
  stale rate, the weak shares each kind of node's shares carry and the
  weak shares lost then feed the credit.
- **Carriers mining siblings** (`weak work, siblings`). Weak work in fork
  choice also lets carrying nodes act on it: when the tip comes from a
  non-carrying node and carries less weak work than a carrying node's share
  at the minimum would on the same parent, carrying nodes keep mining on
  that parent. If a carrying node finds the next share, it is a sibling that
  outweighs the tip and replaces it, and the weak shares the refusing
  miners found on the tip are lost; if a refusing node finds it first, it is
  the tip's child. This is a node policy the page does not state.
- **Coalition gain.** The refusing (non-carrying) classes' expected income
  over what the same classes earn, per unit of fair income, when everyone
  carries. "Refusing ASIC gains" is the tracked non-carrying ASIC against the
  carrying ASIC of the all-carrying run; in that run it is a lone refuser.
- **Miner mixes** (`mixes`; `src/miners.rs`). `zipf`: the `k`-th largest
  miner has `1/k` of the largest one's hashrate, the smallest a home miner's
  (3,520 miners at 1% of BCH, 78,101 at 30%), the largest 32 ranks a class
  each and above them ranks grouped by a factor of 1.25. `medium`: 90% on
  30, 53, 95, 169 and 300 TH/s miners (equal hashrate each), 10% home.
  `medium, few big`: 80% medium, 10% home, 10% 1 PH/s farms.
- **Arrivals and departures** (`arrivals`, `r-max`; `src/transition.rs`).
  A share-by-share chain with no steady state assumed: ASERT anchored at the
  parent with its 600-second half-life; `band_cap` stepped from the parent's
  `band_cap` and `wanted_bits` exactly as section 2 of the page has it (up
  by `C * u / 512`, `u` the wanted multiple as a whole number up to `C_MAX`,
  when the parent wanted more than its cap allowed, else down by `C / 512`);
  `weak_ratio` stepped as section 3 has it; the node default inside the cap;
  weak shares found, carried oldest first and expired; and every share's
  credit in a running window of 3 BCH blocks of work behind 100 shares.
  Home miners' pay is their part of that window times the blocks the pool
  finds at its hashrate. A big miner with as much hashrate as the 50%
  industrial pool joins or leaves at day 7 (so it is half the pool when
  present), on the node default or at the minimum; 8 runs of 21 days per
  case, summed. "Within 10%" is the first hour after the event whose mean is
  within 10% of the mean of the last 72 hours: `C` wanders around where it
  settles, so the hour it first gets there is the measure. The `r-max`
  table runs the same chain from genesis for two days.
- **Tiny pool** (`tiny-pool`). Two miners of 0.25 and 0.75 GH/s on a 37 GH/s
  test network (`H` times 10^-8, so the pool finds blocks at all): the
  ceiling is 4.55, which a 10-second chain reaches below about 2 GH/s. The
  last column names an earlier row that paid every miner exactly the same.
- **Floods** (`floods`). Attack (c)'s spammer, a miner at the minimum with a
  multiple of the honest home hashrate, added to the pool (the network
  stays the same), at 10, 100 and 1,000 times. Bases: a pool of 1,000 home
  miners (0.03% of BCH, so 1,000 times is 30%) and 90% industrial at 1%
  (1,000 times would be more than BCH).
- **Draw steering** (`draw-steering`; `src/payouts.rs`). A tracked miner,
  with every payee at its level, picks of two parents the one whose seed
  pays it more. It can only steer a block it finds itself, so every pool
  block's draw is also run with a second seed and the difference counted at
  the chance the block is its own and it has a choice: `races only` while two
  shares race (`s / (1 - s)`, no orphan risk added, since a race goes either
  way); `tip or its parent` always, but a share it finds on the parent is a
  sibling of the tip and goes stale, losing its tenth and its includer cut,
  for the part of the time the parent's seed is the better one.
- **Losses** (`losses`). A weak share mined on a parent that leaves the chain
  (a race's loser, which is also what an unknown parent becomes) is lost: at
  half the stale rate, since about half the hashrate mines on a race's loser
  for about one share period. A race's loser can also drop the weak shares
  it carried (`uncle carrier drops`), instead of their staying in the queue.
- **Constants**. `weak-depth` sweeps `WEAK_DEPTH` (6, 12, 24; the rules'
  48 is in `refusing-races`) and `INCLUDER_CUT` (0, 1, 5%) against 50% and
  90% refusing coalitions; `r-max` sweeps `R_MAX` (256, 1,024, 4,096) from
  genesis.

### What the study tables show

1. **Weak work in fork choice changes little through races alone.**
   Refusing shares now lose their races to carrying ones (stale 2.93%
   against 1.25% for honest shares, 1% of BCH, half refusing), but races
   are 2% of shares, and at the rules' `WEAK_DEPTH` 48 there is little left
   to change: no honest weak share is lost with half the hashrate refusing,
   and with 90% 1.26% instead of 1.20% at 1% of BCH and 1.14% instead of
   1.54% at 10%. A coalition of half the hashrate gains nothing with no cut
   and loses 0.2 to 0.4% at the 1% cut; one of 90% gains at most 0.11% (no
   cut, 10%) and loses 0.1 to 0.2% at 5%: within 0.04 points of today's
   fork choice.
2. **It bites when carrying nodes mine siblings of light tips.** A
   half-hashrate coalition then loses 5.6 to 6.7% at 1% of BCH and 12.8 to
   15.2% at 10% at every cut, 0% included: its light tips are replaced at
   0.28 and 0.31 per share. A 90% coalition loses 0.3 to 0.9%, since
   carriers find the next share only a tenth of the time. Shares heavier
   than the minimum are never replaced; with one-minute blocks the ASIC is
   at the minimum in this mix, so the refusing ASIC loses with its
   coalition (17% at 1% of BCH), where on heavy shares with ten-minute
   blocks it gained 6%.
3. **`WEAK_DEPTH` is the lever that stops a 90% coalition.** With weak work
   in races and a 1% cut, the coalition's gain falls from +2.8 / +5.6% at
   depth 6 (1% / 10% of BCH) to +1.4 / +2.9% at 12, +0.4 / +0.8% at 24 and
   +0.03 / +0.05% at 48, where honest weak shares lost fall from 55% to
   1.3% and the Bitaxe gets 0.99 of fair instead of 0.49. From depth 12 a
   half-hashrate coalition gains at most 0.12%. At depth 48 a 5% cut makes
   a 90% coalition lose 0.1 to 0.2%; with carriers mining siblings it loses
   0.3 to 0.9% at every cut. A lone refuser loses its includer income:
   nothing at 0%, 0.9 / 1.0% at 1%, 4.6 / 4.9% at 5%. Every miner's income
   is fair at every cut when everyone carries (the cut a home miner pays on
   its weak shares comes back on the weak shares its own shares carry), and
   the Bitaxe's swing does not move with the cut.
4. **Miner mixes.** In the Zipf spread 56% of the hashrate is at the
   minimum at 1% of BCH, 91% at 10% and all of it at 30%, `R` 41 to 35, and
   the Bitaxe's swing is 0.20 / 0.26 / 0.40 at 1 / 10 / 30% (one chain:
   0.39 / 1.28 / 2.28). With mostly medium miners 82% of the hashrate is at
   the minimum at 1% (minimum 0.96 of uniform, `R` 40; with ten-minute
   blocks every medium miner mined heavier shares there), and everyone is
   at the minimum at 10 and 30% (`R` 34.7); the Bitaxe's swing is
   0.20 / 0.27 / 0.39. In every mix and size the Bitaxe gets 24 to 25
   payments a month and 0.97 to 1.01 of fair.
5. **A big miner joining or leaving.** The minimum and `R` are within 10% of
   their new levels within an hour in every case but a big miner on the
   node default joining at 10% (2 and 3 hours), and `R` hardly moves (35 to
   43): it follows the share of chain shares at the minimum. `C`, under the
   page's `wanted_bits` rule, is 30 at 1% of BCH and 1.0 at 10% with the 50%
   industrial mix; a big miner on the node default lifts it to 59 / 5.8
   within 1 / 4 hours, and when it leaves `C` is back within 3 hours; a big
   miner at the minimum joining at 1% brings `C` from 30 to 1.8 in 8 hours.
   (The `C settles at` column of the band cap table uses the earlier
   bits-driven step and gives 1.00 for this mix.) Home miners' pay swings
   with the window, not with `R` or `C`, and the window is a tenth as long
   as with ten-minute blocks: when the pool doubles they get 1.9 times fair
   in the first hour, 1.15 to 6 hours and fair after that at 1% of BCH (1.35
   in the first hour and fair after at 10%), and when it halves 0.5 and 0.8,
   fair from 6 hours (0.7 in the first hour at 10%), the same whether the
   big miner is on the node default or at the minimum. With ten-minute
   blocks the same swings lasted to day 2. No weak share expires meanwhile.
6. **Tiny pool.** At the ceiling `R` is 1.00 and no weak share is found, and
   the page's design with every miner at the minimum pays exactly as one
   chain, as does the band alone. But the node default still makes shares
   heavier: the 1 GH/s pool finds 1,168 one-minute blocks a month, so 4
   shares a pool block are 4,670 a month, and it gives the small miner 7.1
   times the ceiling and the large one 21.3 times (with ten-minute blocks,
   64 times for both). Shares fall from 0.051 to 0.0036 a second; payments
   stay at about 1,168 a month, ten times the ten-minute 117, and the swing
   at 0.03. The small miner gets 1.04 of fair and the large one 0.99: the
   lighter shares name most uncles and take the nephew's tenth of the
   heavier ones. The rows with levels differ from one chain only because
   estimating the level pots draws from the random stream: every slice
   there is far above every level.
7. **Floods do not hurt small miners' income.** The flood takes 91 to
   99.9% of the weak shares and the Bitaxe's fall from 9,753 to 9.5 a month
   (1,000 home miners, 1,000 times), but the flood brings blocks: the
   Bitaxe's payments rise from 14.1 to 24.8 (10 times) and stay at 18.5 to
   24 a month, its swing goes from 0.26 to between 0.20 and 0.40, and its
   income stays fair. At 90% industrial its swing goes from 0.20 to 0.21
   to 0.28 at 100 times. The flood is paid fair, and its cuts paid less cuts
   received are at most 0.23% of its income at the 1% cut (1.2% at 5%): it
   carries nearly all weak shares, so it gets its cuts back.
8. **Steering the draw does not pay.** Between racing shares it wins at
   most 170 sat a month (the ASIC at level 7 at 10% of BCH), because a miner
   steers only the blocks it finds (0.014 a month for the Bitaxe, ten times
   the ten-minute figure). Picking the tip's parent every time wins at most
   7,918 sat a month and loses more to the stale shares it finds there: 0.01
   to 0.25% of its income, net.
9. **Losses.** Weak shares on a parent that leaves the chain, at half the
   stale rate, lose 1.05% of weak shares at 10 s and 4.35% at 2 s; the
   Bitaxe gets 0.99 to 1.01 and 0.97 to 0.98 of fair (the loss goes to
   heavier shares) with payments and swing within noise, since `R` rises to
   fill the slots again.
   A race's loser dropping its weak shares loses none when every node
   carries: the winner carried the same oldest ones.

### Constants (study), the rules since 2026-11-12

The rerun with them as the model's defaults (the tables below) agrees: with
90% of the hashrate refusing and share work deciding forks, as on the live
chain, 1.2% (1% of BCH) and 1.5% (10%) of honest weak shares are lost, the
Bitaxe gets 0.99 of fair, and the coalition gains +0.03% at 1% of BCH and
+0.09% at 10%; weak shares expire in no steady-state table. `r-max-mixes`
runs `R_MAX` 256 against 4,096 in every mix and pool size: `R` settles at 35
to 88 wherever any hashrate mines at the minimum, and the two caps give the
same results within noise. With ten-minute blocks the 99% industrial pool
at 1% of BCH pinned `R` at its cap; with one-minute blocks 1% of its
hashrate mines at the minimum and `R` settles at 87.5 under both caps.

- `WEAK_DEPTH` = 48 (8 minutes at 10 s): the only tested change that stops
  a 90% refusing coalition; its gain falls from +2.8 / +5.6% at 6 to
  +0.03 / +0.05% at a 1% cut, and the Bitaxe keeps 0.99 of fair instead of
  0.49.
- `INCLUDER_CUT` = 1%: at `WEAK_DEPTH` 48 refusing gains a 90% coalition at
  most 0.09%, costs a half-hashrate coalition 0.2 to 0.4% and a lone refuser
  0.9 to 1.0%; with races alone only 5% makes refusing never pay (a 90%
  coalition loses 0.1 to 0.2%), and with carriers mining siblings 0% does.
- `R_MAX` = 256: with `WEAK_FILL` 32, `R` settles between 35 and 88 in every
  table, and from genesis 256 brings `R` within 10% in 2 hours instead of 4,
  with 10 to 14 weak shares a second in the first hour (8 to 11 KB/s, a
  third to half expiring) instead of 169 to 228 (135 to 183 KB/s, 86 to 91%
  expiring).
- Weak work in fork choice: worth the header-sync work only together with
  carriers mining siblings of light tips; through races alone it moves a
  coalition's gain by 0.04 points at most.

### Approximations of the study

- Races: two shares per race, at `s / (1 - s)` per share; both see the same
  weak queue; first-seen propagation is not modelled (equal work is a coin
  toss); a race's loser is always named as an uncle.
- Siblings: the sibling always carries everything waiting and wins; no race
  at a replacement; replaced tips are not fed back into ASERT (the weak
  ratio absorbs the change); every weak share's work is counted at the
  current weak ratio.
- Coalition and miner incomes in the refusing and depth tables are
  expected from the credit (400,000 queued shares), not simulated payments.
- Arrivals: the node's ten-minute hashrate estimates are exact; ASERT in
  floating point; stale shares lower the share rate but are not raced.
- Losses: the stale-parent rate (half the stale rate) is an estimate, not a
  measurement.
- Steering: the miner's place in its level group is uniform in every block.
- The tiny pool's network is hypothetical.

### Not done

- Header-sync and validation work of counting weak work in fork choice,
  relay latency of weak-share traffic, compact coinbases and the other
  propagation ideas (they belong in `p2poolv2_netsim`), and tokens and SAFA.
- `WEAK_BYTES` and the band step. (A deep `WEAK_DEPTH`'s cost to a node is
  on the design page: a weak share needs only a known Bitcoin Cash prev, as
  a share does, so waiting does not make it stale.)

## Scale: bigger pools on a bigger Bitcoin Cash

`cargo payout-sim --scenario scale` runs the adopted design at 10, 30, 50
and 90% of Bitcoin Cash and at 1, 10, 100, 270 and 1,000 times today's
hashrate (270 times is about Bitcoin's), 50% industrial, with every home
miner's payout level, the Bitaxe's and the ASIC's set as a node sets them
(`level_policy::affordable_payout_level`: the highest level up to 5 that
still expects three payments a month). The model keeps home miners at 1.2
TH/s and half the pool's hashrate, so ten times the hashrate is ten times
the home miners. Each row gives Bitcoin Cash's difficulty at that hashrate
for one-minute blocks: 5.17e10 today, 1.40e13 at 270 times and 5.17e13 at
1,000 times, a tenth of the ten-minute difficulty. Bitcoin's own
difficulty, at about the 270 times rows' hashrate with ten-minute blocks,
is ten times the x270 rows', about 1.4e14. The table is at the end of this
file.

The smallest miner that still finds three weak shares a month, the node's
payment floor, in TH/s:

| Bitcoin Cash difficulty | Pool at 10% | 30% | 50% | 90% |
|---|---:|---:|---:|---:|
| 5.17e10 (today) | 0.12 | 0.36 | 0.60 | 1.1 |
| 5.17e11 (10 times) | 1.2 | 3.6 | 6.0 | 11 |
| 5.17e12 (100 times) | 12 | 36 | 60 | 109 |
| 1.40e13 (270 times, Bitcoin's hashrate) | 33 | 98 | 163 | 294 |
| 5.17e13 (1,000 times) | 121 | 363 | 604 | 1,088 |

These are the ten-minute figures but for the pool at 10% today (0.09 TH/s
then): the floor follows the hashrate at the minimum, not the block
interval, and at 10% of Bitcoin Cash the node default no longer gives the
farms heavier shares, so more of the pool's hashrate makes weak shares.

It is `3 * (minimum difficulty / R) * 2^32 / a month`: the weak difficulty
follows the hashrate at the minimum, so the floor rises with the pool's
hashrate, whether the pool grew or Bitcoin Cash did. A miner below it is
still paid fairly, in rarer and larger pieces, until it is upgraded or
leaves.

What it shows:

1. **Payouts stay fair at every size.** A miner's income over its fair part
   stays within the noise of the months simulated, from a tenth to nine
   tenths of Bitcoin Cash and up to 1,000 times today's hashrate.
2. **How often a small miner is paid falls with the number of home miners
   in the pool.** A pool makes `WEAK_FILL` weak shares a share, 3.2 a second
   at 10-second shares, about 8.3 million a month, whatever its size. They
   are split among the miners at the minimum. Today a Bitaxe finds 29 a
   month in a pool at 10% of Bitcoin Cash, 9.4 at 30%, 5.6 at 50% and 3.1 at
   90%; at ten times the hashrate, 2.8 down to 0.3.
3. **A bigger pool pays in bigger pieces.** The window is three blocks of
   work, so at 50% of Bitcoin Cash it is six minutes, 36 shares, and one
   weak share is worth 23,994 satoshis of each block; at 90%, 43,162: the
   same slice as with ten-minute blocks, from a tenth of the reward and a
   tenth of the window. The payout level cannot split a weak share's
   credit, so above about 30% of Bitcoin Cash a Bitaxe is paid by the weak
   share, not by its level: 17 payments a month at 50% and 9.5 at 90%
   today, with 0.5% and 4.7% of months empty. The ASIC's slice of a block is
   below its level from 10% of Bitcoin Cash today, so it is paid about 800
   to 2,000 times a month in pieces of about the level or one weak share.
4. **A 1.2 TH/s miner ages out as hashrate grows, a 100 TH/s miner much
   later.** At ten times today's hashrate a Bitaxe is paid 4.7 times a month
   at 10% of Bitcoin Cash (10% of months empty) and under once a month at
   90%. At 270 times, Bitcoin's hashrate, it is paid once every three months
   to once every three years. A 100 TH/s miner at 270 times is still paid
   7.6 times a month at 10%, 7.3 at 30% (7% of months empty), 5.2 at 50%
   (19% empty) and 2.9 at 90% (40% empty).
5. **The lever is weak shares a second.** More weak shares a share, or a
   shorter share period, gives small miners more, smaller credits. The next
   section compares the two.

## Scale levers: a shorter share period against more weak shares a share

`cargo payout-sim --scenario scale-levers` asks when section 5 of the design
page, the dynamic share period, is needed, and whether it is the right
lever. It runs the scale table's pools at 10 and 30% of Bitcoin Cash and at
1, 10, 100 and 270 times today's hashrate, each seven ways:

- the rules as they are: 10-second shares, `WEAK_FILL` 32;
- a share period of 5, 2 and 1 seconds, each share carrying as many weak
  shares as today, at the stale rate the network simulation measured for
  that period;
- 10-second shares carrying 2, 5 and 10 times the weak shares (`WEAK_CAP`,
  `WEAK_FILL` and `R_MAX` times that), which is as many weak shares a second
  as the 5, 2 and 1-second periods give.

The table is at the end of this file. What it shows:

1. **Today the share period changes nothing for a small miner.** At today's
   hashrate a Bitaxe is paid 24 to 25 times a month at every period and
   every weak share rate, with no empty month. Weak shares already credit it
   29 times a month in a pool at 10% of Bitcoin Cash and 9.4 times at 30%.
2. **More credit is first needed when the pool itself is about as large as
   all of Bitcoin Cash is today.** A pool makes 3.45 weak shares a second
   whatever its size, split among its miners at the minimum. The smallest
   miner that still finds three a month is about 0.33 TH/s for each EH/s of
   pool hashrate when half of it is home miners. That reaches a Bitaxe's 1.2
   TH/s at 3.7 EH/s of pool hashrate, for example 10% of a Bitcoin Cash ten
   times today's, and a 100 TH/s miner's at about 300 EH/s, 30% of a Bitcoin
   Cash at Bitcoin's hashrate.
3. **Past that point both levers help, and equally.** At ten times today's
   hashrate in a pool at 30%, a Bitaxe has 41% of its months empty. Twice
   the weak shares a second leave 19%, five times 4 to 6% and ten times 2%
   (the same as with ten-minute blocks),
   whether they come from a shorter period or from more weak shares a share:
   the two columns of payments, empty months and swing agree within the
   noise of the months simulated in every row.
4. **The shorter period pays for it in stale shares, more weak shares a
   share do not.** The 5, 2 and 1-second periods run at 3.8%, 8.7% and 15.9%
   stale shares; the 10-second period stays at 2.1% whatever its shares
   carry. Both relay the same weak shares, 6.9, 17 and 35 a second.
5. **Neither lever saves a 1.2 TH/s miner at Bitcoin's hashrate.** At 270
   times today's hashrate its fair income is 1,605 satoshis a month, under
   two payments at the lowest level. With ten times the weak shares a
   second it still has about 45% of its months empty in a pool at 10% and
   about 70% at 30%. A 100 TH/s miner there is paid 7 to 8 times a month by
   today's rules; its 7% of empty months at 30% go to 1.3% or below with
   twice the weak shares a second.
6. **On 1,000 nodes the network simulation says the same.** The stale
   rates above are its 100-node figures. A day of shares on 1,000 nodes
   (`cargo netsim run --nodes 1000 --share-interval S --weak-shares N
   --weak-share-carriage gossip-only`, 8,640 share intervals each, from
   `cargo netsim sweep scaling-weak-shares`):

   | Share period | Weak shares a share | Weak shares a second | Stale | Upload a node, mean (Mbit/s) |
   |---|---:|---:|---:|---:|
   | 10 s | 32 | 3.2 | 3.83% | 0.35 |
   | 5 s | 32 | 6.4 | 8.01% | 0.75 |
   | 2 s | 32 | 16 | 17.89% | 1.88 |
   | 1 s | 32 | 32 | 30.43% | 3.78 |
   | 10 s | 64 | 6.4 | 3.83% | 0.52 |

   Twice the weak shares a second cost 0.18 Mbit/s of upload and no stale
   share when a 10-second share carries them, and 0.40 Mbit/s and 4.2 points
   of stale shares when a 5-second period brings them. With one-minute
   blocks part of that comes from the coinbase: the 1% pool's window holds
   3,600 shares at 5 seconds against 1,800 at 10, so a thousand miners make
   974 payees instead of 836 and a 35 KB share instead of 31 KB. That is
   with weak shares travelling beside the shares. A share's body carries
   them whole today, and so 32 of them a share already put the stale rate at
   4.80% (`--weak-share-carriage whole`): a node that has a weak share from
   gossip downloads it again inside the share.

So the dynamic share period is not the lever to build. What the design may
need, once a pool passes about 3.7 EH/s, is more weak shares a share: a
larger `WEAK_FILL` and `WEAK_CAP`, or the budget issue #79 describes that
grows while its slots stay full, under a ceiling in bytes a second. The page
has `WEAK_FILL` fixed at 32.

## Proposal: weak shares that follow the asks

Section 6 of the design page proposes how the number of weak shares should
grow: a weak share asks for an easier weak target when its node wanted one
for its miner, and while more than one in `WEAK_ASK_DIVISOR` (64) of the
weak shares a share carried ask, the weak ratio rises instead of steering
towards `WEAK_FILL`, until a share carries more than `WEAK_FILL_MAX` (320).
It is not a rule of the chain. Four tables test it, at the end of this file:

```sh
cargo payout-sim --scenario weak-asks            # where the count settles
cargo payout-sim --scenario weak-asks-mixes      # every miner mix
cargo payout-sim --scenario weak-asks-constants  # the divisor, the node default, the ceiling
cargo payout-sim --scenario weak-asks-attacks    # false asks, silence, censoring nodes
cargo payout-sim run --asks --hashrate-multiple 10 --pool-fraction 0.3
```

### The model

`src/asks.rs` runs the weak share queue share by share, as `src/weak.rs`
does, with the proposed step. Only counts are simulated. A class of miners
at the minimum asks while the weak ratio in force is below the ratio at
which one of its miners finds the node default's 16 weak shares a month,
`16 * D_asert * 2^32 / (hashrate * month)`, and never at or above the ratio
that puts the weak target at the chain's ceiling. A share carries up to 640
weak shares, the oldest first, those that ask and those that do not in
proportion. An attack splits each class: a part that asks whatever its
miners find, a part that never asks, or a part of the chain's shares from
nodes that carry no weak share that asks. Each row then pays the Bitaxe and
the ASIC block by block, as every other table does, with every class at the
payout level a node would leave it at. "fill 320" is the rules as they are
with `WEAK_FILL` 320, `WEAK_CAP` 640 and `R_MAX` 2,560.

### What the tables show

1. **The count follows the need.** At today's hashrate a pool at 10% of
   Bitcoin Cash stays at the floor and one at 30% opens to 57 weak shares a
   share, where a Bitaxe finds 16 a month. At ten times today's hashrate a
   pool at 10% settles at 189 and one at 30% at the ceiling, 344.
2. **It pays as `WEAK_FILL` 320 does where the ceiling is reached, and
   relays less where it is not.** At ten times today's hashrate in a pool at
   10%, a Bitaxe has 1.4% of its months empty at 15 KB/s of relay; with
   `WEAK_FILL` 320 it has 1.3% at 28 KB/s; by today's rules, 9.3% at 2.9
   KB/s. At today's hashrate the asks relay 3.8 to 4.7 KB/s against 28.
3. **Without asks the mean is 46 weak shares a share, not 34.5.** The step
   holds the median at `WEAK_FILL`; with a cap of 640 a share found after a
   long wait carries every weak share found on its parent.
4. **Every mix opens.** At ten times today's hashrate the count settles at
   115 to 345 a share in the home, 50% and 90% industrial, Zipf and medium
   mixes. In the 99% industrial mix the home miners are 1% of the hashrate
   at the minimum, under one in 64, and open it to 82.
5. **The divisor matters only in a spread of sizes.** With half the
   hashrate on home miners of one size, 16, 64 and 256 give the same count.
   In the Zipf mix they give 220, 299 and 312 a share.
6. **The node default sets where the count stops.** Asking below 4 weak
   shares a month leaves 142 a share (50% industrial) and a Bitaxe with 6.3%
   of its months empty; below 16, the ceiling and 2.7%; below 64, the same.
7. **The ceiling sets what the smallest miners get.** `WEAK_FILL_MAX` 64,
   160 and 320 leave a Bitaxe in a pool at 30% of ten times today's Bitcoin
   Cash with 17.5%, 5.2% and 2.7% of its months empty.
8. **Attacks reach the ceiling or the floor, no further.** In a pool that
   needs no more, 1% of the hashrate at the minimum asking whatever it finds
   raises the count from 46 to 82 a share, 2% to 221 and 5% to 316. In a
   pool that needs more, it takes 97% of that hashrate never asking to bring
   the count from 344 to 143, and 99% to bring it to 57. Nodes that carry no
   asking weak share lower it with their part of the shares: 250, 168, 107
   and 83 a share at 25, 50, 75 and 90%.

### Not modelled

- The step from one count to another. The ratio moves by 1/256 a share, so
  ten times the count takes about 590 shares, 1.6 hours at 10-second shares.
  The next section runs it.
- The band's own reading of `wanted_bits`, which the proposal leaves as it
  is.
- What a node pays to check and store the weak shares; the design page
  takes those from the network simulation.

## Study: the weak ratio's path from genesis and after a big miner, with the asks

PR #118 builds section 6 with a `WEAK_CAP` of 640. A 12-node swarm on that
branch ran out of memory: from genesis, at a weak ratio of 256, every share
carried 640 weak shares, while alpha's cap of 64 bounded the same start and
the ratio settled near 32 after about 500 shares. The tables above measured
steady states, not the path to them. Five tables run the path share by
share, and each candidate fix in steady state and under the attacks again:

```sh
cargo payout-sim --scenario weak-asks-genesis       # from genesis
cargo payout-sim --scenario weak-asks-big-miner     # a big miner joins or leaves at the minimum
cargo payout-sim --scenario weak-asks-rules         # where each rule settles
cargo payout-sim --scenario weak-asks-rule-mixes    # every miner mix
cargo payout-sim --scenario weak-asks-rule-attacks  # the attacks under each rule
```

The rules:

| Rule | What changes from section 6 as written |
|---|---|
| today | Alpha: `WEAK_CAP` 64, no asks |
| v0 as written | Nothing: PR #118 |
| v1 down to R/2 (R/4) | A share that was not asked and carried more than 32 takes the next ratio to `R * 32 / count` at once, never below `R / 2` (`R / 4`) |
| v2 cap follows asks | A share may carry more than 64 weak shares only when more than one in 64 of those it carries ask |
| v3 genesis 32 | The genesis share's weak ratio is 32, not 256 |
| v4 = v2 + v3 | Both |
| v5 = v4, one ask opens | v4, but one asking weak share is enough for more than 64 |

### The model

`src/transition.rs` runs the chain share by share with ASERT, the band cap
and the weak ratio moving as the rules move them. It now runs the asks too:
each class at the minimum asks while the ratio in force is below the ratio
at which one of its miners finds 16 weak shares a month at the minimum of
the moment, and a share carries what `carry_plan` in `src/asks.rs` lets it,
the oldest first. Under v2, v4 and v5 a share whose weak shares ask too
seldom leaves its newest silent ones waiting until it is down to 64 or
enough of what it carries ask; they expire if no later share takes them
within `WEAK_DEPTH`. Each case is eight runs of the 50% industrial mix,
averaged: two days from genesis, or a big miner with as much hashrate as
the pool, at the minimum, joining or leaving a day after genesis and two
days after that. A run makes about 2,100 shares in six hours. "Count stays
within 25%" is the first hour from which every hourly count is within 25%
of the last day's. The test network pool is 10 GH/s of 10 MH/s miners on a
3.7 TH/s network, so small that the chain's ceiling holds the weak ratio in
force near 5; its big miner is 1 TH/s, and its last rows add 1% of the
hashrate at the minimum asking whatever it finds. The steady-state tables
run the queue of `src/asks.rs` under each rule and pay the Bitaxe and the
ASIC as the tables above do. The earlier tables come out byte for byte as
before.

### What the tables show

1. **The flaw is the start, and the model finds it.** From genesis in a pool
   at 10% of today's Bitcoin Cash, where nobody needs to ask, the rule as
   written carries 132 weak shares a share in the first hour against
   today's 64; 769 of a run's first 2,100 shares carry more than 64 and 41
   more than 320, the largest 640, and the count stays within 25% of where
   it settles only after 3 hours. The model has no checking cost, so it
   does not reproduce the swarm's slowdown, in which shares came slower
   under the load and so carried still more; for the rule as written its
   counts are a lower bound.
2. **A big miner brings the flaw back only where the ceiling held the
   ratio.** A share carries `R - 1` times the part of the chain's shares
   from jobs at the minimum, whatever the hashrate, so a big miner arriving
   at the minimum changes the count little: 46.6 a share before, 49.6 at
   the peak after, in the pool at 10% of today's. At ten times today's
   hashrate it doubles the minimum, the home miners ask again and the count
   opens from 175 to 190 a share to 335 to 342 within an hour under every
   rule with asks but v1, as it should. On the test network the ratio in force sits near 5 while the
   ratio itself climbs to 256; when 1 TH/s joins, the rule as written
   carries more than 64 in 860 of the next 2,100 shares and more than 320
   in 51, and a genesis ratio of 32 changes nothing (870 and 53).
3. **v1 breaks the steady state.** It settles at 10.7 weak shares a share in
   the pool at 10% of today's (46 as written), and at 54 in both pools at
   ten times today's hashrate (189 and 344); with R/4 at 10 to 40. A Bitaxe
   in a pool at 30% of ten times today's Bitcoin Cash has 25.4% of its
   months without a payment (34.3% with R/4), against 2.7% as written. One
   share's count is a noisy draw, the weak shares found in an exponential
   wait, so a rule that falls to `R * 32 / count` on one share and rises by
   1/256 holds the median far below 32. The attacks get much worse: with 90%
   of the hashrate never asking a Bitaxe has 66.5% of its months empty
   (2.1% as written), with 25% of the shares from nodes that carry no asking
   weak share 57.7% (3.0%).
4. **v2 bounds every start where nobody asks, and keeps the asks.** From
   genesis in the pool at 10% of today's no share carries more than 64, but
   12.7% of the weak shares found in the first six hours expire (13.1%
   today). On the test network no share carries more than 64. In steady
   state the pool at 10% of today's settles at today's 34.5 a share, not
   46: the 12 more were shares after a long wait carrying all the weak
   shares found on their parent, which nobody asked for, and the Bitaxe has
   no month without a payment either way. At ten times today's hashrate the
   pools settle at 187 and 344 (189 and 344 as written).
5. **v3 fixes genesis only.** Its start peaks at 48, near the steady 46, but
   471 shares in six hours still carry more than 64: that is the rule as
   written in its steady state, where 22% of the shares carry more than 64.
   On the test network it does what v0 does (point 2).
6. **v4 fixes both and costs nothing at genesis.** From genesis in the pool
   at 10% of today's it peaks at 36, no share carries more than 64 and no
   weak share expires; on the test network no share carries more than 64.
   In the pool at 30% of ten times today's the count takes two hours to open
   from 32 to 344, the one cost of the low start.
7. **Where fewer than one weak share in 64 asks, v2 and v4 open less than
   v0.** (After genesis v4 is v2.) In the 99% industrial mix at ten times
   today's hashrate v4 settles at 49.5 and 49.7 a share in pools at 10 and
   30% instead of 82, and a Bitaxe has 7.7% and 25.3% of its months without
   a payment instead of 2.6% and 14.0% (9.0% and 41% today). Every other mix
   settles within three shares of the rule as written. The attacks that
   dilute the asks bite harder under v2: with 97% and 99% of the hashrate
   never asking the count falls to 95 and 39 instead of 143 and 57, and
   with 25, 50, 75 and 90% of the shares from nodes that carry no asking
   weak share to 231, 139, 86 and 68 instead of 250, 168, 107 and 83. None
   goes below today's rule. Hashrate that asks whatever it finds is no
   stronger: 241 and 317 a share at 2% and 5% (221 and 316 as written).
8. **When the asks stop, v4 cuts at once and loses a little.** When the big
   miner leaves the pool at 10% of ten times today's, the home miners stop
   asking; the rule as written comes down from 343 to 190 within the hour,
   v4 carries 64 until the ratio falls and 7.1% of the weak shares found in
   the next six hours expire. Share by share, with the asks going on and
   off, that pool settles at 175 under v4 with 1.5% of its weak shares
   expiring, against 190 and none. Home miners' pay over fair is the same
   under every rule (0.94 on the first day, the window turning over).
9. **v5 wins back half of point 7 and gives the start to an asker.** In the
   99% industrial mix it settles at 66 a share (a Bitaxe has 3.2% and 19.5%
   of its months empty). But on the test network with 1% of the hashrate
   asking whatever it finds, 66 shares in six hours after the join carry
   more than 320 under v5, against 7 under v4 and 122 as written.

So v4 is the variant that fixes the flaw: where nobody asks, no start
carries more than today's 64 and the count is today's, at genesis no weak
share is lost, and where more than one weak share in 64 asks every steady
state of section 6 holds, the Bitaxe's and the ASIC's pay with it.
It gives up part of section 6's gain where the askers are fewer than one in
64 (point 7) and loses 1.5% of the weak shares where the asks switch on and
off (point 8), never falling below today's rule, and opens no attack that
section 6 did not have. v1 fails in steady state and under every attack;
v3 alone and v0 leave the test network's start open; v5 trades a little of
point 7 back for a start that 1% of the hashrate can open. The cap of v4 is
checked from the header alone: a share with more than 64 weak shares is
valid only if `weak_asking * 64 > weak_count`.

### Not modelled

- What checking the weak shares costs a node, and the slower shares that
  load brings; the swarm has it.
- Gossip: every node sees every weak share at once.
- A node that, under v2, v4 or v5, picks newer asking weak shares over older
  silent ones to carry more; the model keeps the oldest first and leaves the
  newest silent ones.

## Window cap and storage

`cargo payout-sim --scenario window-cap` asks whether the PPLNS window's cap
should be longer than 14 days. On Bitcoin Cash the window holds three blocks
of the pool's work, at most 120,960 shares; a pool that finds fewer than
three blocks in 14 days reaches the cap. The table runs pools of medium
miners at 0.01 to 1% of today's Bitcoin Cash, levels as a node sets them,
with the cap at 14, 30 and 60 days. `cargo payout-sim --scenario storage`
is arithmetic, no simulation: what a node keeps, by payees in the window,
payout level and cap. Both tables are at the end of this file.

What the window cap table shows:

1. **The cap binds only in pools under about 0.15% of Bitcoin Cash**, 5.5
   PH/s today, which find fewer than three blocks in 14 days. From 0.2% up
   the window holds three blocks well inside 14 days (10.4 days at 0.2%,
   2.1 at 1%), and every cap gives the same row.
2. **A longer cap does not help a miner that keeps mining.** In a pool at
   0.01% (370 TH/s, a block about every 70 days) the Bitaxe has 63%, 66%
   and 65% of its months without a payment at 14, 30 and 60 days; at 0.02%
   40%, 39% and 42%; at 0.05% 11%, 10% and 11%. Each block pays a steady
   miner its part of the window's work, whatever the window's length, so
   its income follows the blocks the pool finds. Pay stays fair in every row
   (0.99 to 1.08).
3. **What it changes is the single share.** At 0.05% a share is paid by 1.0
   block on average with 14 days and 2.2 with 30, so 36% of shares against
   12% are paid by none. That matters to a miner that mines a few days and
   leaves, and its expected pay is fair either way.
4. **It costs every node.** The cap sets the payout depth, and with it the
   reorg limit, the expiry of share chain outputs and the bodies a node
   keeps: 60 days of them instead of 28 with a 30-day cap. A miner that
   stops is paid for up to 30 days.

So the 14-day cap stays.

What the storage table shows:

1. **The payout level, not the payees, bounds a coinbase.** Below its level
   a payee's part is drawn in whole levels, so a block pays at most the
   reward over the level: 17,885 outputs at the default level 5 (608 KB),
   with 100,000 payees or 1,000,000. Only at level 1 (1,092 satoshis) does
   a block with 100,000 or more payees fill Bitcoin Cash's 1,000,000-byte
   transaction limit and pay the rest through its payout tree: 70,600 tree
   payees (3.2 MB kept for ever) with 100,000 payees, 256,772 (11.6 MB) with
   1,000,000. A pool at 30% of Bitcoin Cash finds about 1,300 blocks a
   month, so with a million payees all at level 1 it would keep about 15 GB
   of trees a month.
2. **The coinbase is most of what a node keeps:** 8.3 GB for 28 days with
   1,000 payees, 82.5 GB with 10,000, 148 GB at the default level's ceiling
   and 242 GB when every coinbase is a full 1 MB. A 30-day cap doubles each.
   Never pruned, they would grow by 108 GB to 3.2 TB a year.
3. **Stored without its payouts it takes 367 bytes,** 0.09 GB for 28 days.
   Every node can rebuild the payouts from the chain, as it already does to
   check a share.
4. **Weak shares take 6.2 GB for 28 days at 32 a share and 62 GB at part
   6's ceiling of 320.** Kept for one payout depth instead of two, which
   they allow because they hold no share chain outputs, 3.1 and 31 GB.
5. **Headers, kept for ever, take 3.15 GB a year.**

With coinbases stored without their payouts and weak shares kept for one
payout depth, a node at part 6's ceiling with a million payees keeps about
31 GB and 3 GB more a year, against 62 GB of weak shares and 148 to 242 GB
of coinbases now. Fees are left out: at level 5 each 1 BCH of fees adds
about 5,700 outputs.

## Code

| What | Where |
|---|---|
| Constants and their sources | `src/constants.rs` |
| Miner classes, mixes, tracked miners | `src/miners.rs` |
| Steady-state difficulties: uniform, band, ABLA settle, vardiff | `src/difficulty.rs` |
| The window sized by work | `src/window.rs` |
| The weak share queue and the weak ratio | `src/weak.rs` |
| The weak share queue under the proposed asks and its variants | `src/asks.rs` |
| Payout levels and the draw | `src/levels.rs` |
| Tracked miners' payouts block by block | `src/payouts.rs` |
| Scenarios, credit, reports | `src/scenario.rs` |
| Share races, fork choice and siblings of light tips (study) | `src/races.rs` |
| A big miner joining or leaving, and genesis, share by share, with or without the asks (study) | `src/transition.rs` |
| The tables | `src/tables.rs`, `src/study.rs`, `src/ask_study.rs`, `src/ask_paths.rs`, `src/report.rs` |
| Command line | `src/bin/payout_sim.rs` |

The crate is new code for the Bitcoin Cash fork, under AGPL-3.0-or-later
(see `LICENSE`), like `p2poolv2_netsim`, whose generator, number formats and
share chain constants it reuses. It does not link `p2poolv2_lib`.

## Tables from the default run

### Designs at each pool size

#### Mix: home

| Design | Pool | Bitaxe pay/mo | Bitaxe median sat | Bitaxe swing | Bitaxe empty months | Bitaxe fee loss | Bitaxe vs fair | ASIC pay/mo | ASIC median sat | ASIC swing | ASIC empty months | ASIC fee loss | ASIC vs fair |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| mini | 1% | 24.9 | 16,808 | 0.38 | 0.00% | 0.86% | 0.99 | 1.17 | 30,937,500 | 0.91 | 30.70% | 0.00% | 1.01 |
| mini | 5% | 4.87 | 84,039 | 0.94 | 23.20% | 0.18% | 0.94 | 1.17 | 30,937,500 | 0.91 | 30.70% | 0.00% | 1.01 |
| mini | 10% | 2.36 | 167,667 | 1.27 | 46.20% | 0.09% | 0.91 | 1.17 | 30,937,500 | 0.91 | 30.70% | 0.00% | 1.01 |
| mini | 20% | 1.18 | 333,572 | 1.78 | 68.80% | 0.04% | 0.91 | 1.17 | 30,937,500 | 0.91 | 30.70% | 0.00% | 1.01 |
| mini | 30% | 0.781 | 497,697 | 2.28 | 78.40% | 0.03% | 0.89 | 1.17 | 30,937,500 | 0.91 | 30.70% | 0.00% | 1.01 |
| one-chain | 1% | 24.9 | 16,853 | 0.39 | 0.00% | 0.86% | 0.99 | 430 | 84,314 | 0.06 | 0.00% | 0.18% | 1.00 |
| one-chain | 5% | 4.95 | 84,130 | 0.89 | 21.40% | 0.18% | 0.96 | 391 | 84,130 | 0.10 | 0.00% | 0.16% | 1.00 |
| one-chain | 10% | 2.37 | 167,758 | 1.28 | 46.30% | 0.09% | 0.91 | 210 | 167,758 | 0.13 | 0.00% | 0.09% | 1.00 |
| one-chain | 20% | 1.12 | 333,661 | 1.89 | 69.30% | 0.04% | 0.86 | 107 | 333,661 | 0.20 | 0.00% | 0.04% | 0.99 |
| one-chain | 30% | 0.803 | 497,785 | 2.28 | 78.10% | 0.03% | 0.92 | 71.4 | 497,785 | 0.23 | 0.00% | 0.03% | 0.99 |
| band | 1% | 24.9 | 16,853 | 0.39 | 0.00% | 0.86% | 0.99 | 430 | 84,314 | 0.06 | 0.00% | 0.18% | 1.00 |
| band | 5% | 4.95 | 84,130 | 0.89 | 21.40% | 0.18% | 0.96 | 391 | 84,130 | 0.10 | 0.00% | 0.16% | 1.00 |
| band | 10% | 2.37 | 167,758 | 1.28 | 46.30% | 0.09% | 0.91 | 210 | 167,758 | 0.13 | 0.00% | 0.09% | 1.00 |
| band | 20% | 1.12 | 333,661 | 1.89 | 69.30% | 0.04% | 0.86 | 107 | 333,661 | 0.20 | 0.00% | 0.04% | 0.99 |
| band | 30% | 0.803 | 497,785 | 2.28 | 78.10% | 0.03% | 0.92 | 71.4 | 497,785 | 0.23 | 0.00% | 0.03% | 0.99 |
| vardiff-cd | 1% | 25.2 | 16,854 | 0.39 | 0.00% | 0.86% | 1.00 | 25.1 | 1,341,524 | 0.39 | 0.20% | 0.01% | 0.96 |
| vardiff-cd | 5% | 4.88 | 84,086 | 0.90 | 22.20% | 0.18% | 0.94 | 19.4 | 1,767,125 | 0.45 | 0.00% | 0.01% | 0.95 |
| vardiff-cd | 10% | 2.38 | 167,717 | 1.24 | 45.90% | 0.09% | 0.92 | 19.3 | 1,766,830 | 0.44 | 0.10% | 0.01% | 0.94 |
| vardiff-cd | 20% | 1.20 | 333,624 | 1.79 | 68.50% | 0.04% | 0.92 | 19.2 | 1,766,924 | 0.47 | 0.30% | 0.01% | 0.94 |
| vardiff-cd | 30% | 0.756 | 497,753 | 2.38 | 78.80% | 0.03% | 0.86 | 19.2 | 1,767,169 | 0.45 | 0.50% | 0.01% | 0.94 |
| band-weak | 1% | 281 | 480 | 0.23 | 0.00% | 9.66% | 0.99 | 432 | 79,214 | 0.05 | 0.00% | 0.18% | 1.00 |
| band-weak | 5% | 89.7 | 2,401 | 0.51 | 0.00% | 3.11% | 0.98 | 2,095 | 9,606 | 0.06 | 0.00% | 0.86% | 1.00 |
| band-weak | 10% | 45.3 | 4,802 | 0.73 | 0.00% | 1.59% | 0.97 | 2,518 | 4,802 | 0.08 | 0.00% | 1.03% | 1.00 |
| band-weak | 20% | 22.8 | 9,603 | 1.01 | 0.00% | 0.83% | 0.94 | 1,697 | 9,603 | 0.11 | 0.00% | 0.70% | 1.00 |
| band-weak | 30% | 15.1 | 14,401 | 1.27 | 0.00% | 0.54% | 0.96 | 1,198 | 14,401 | 0.14 | 0.00% | 0.49% | 1.00 |
| band-weak-levels | 1% | 24.1 | 17,481 | 0.27 | 0.00% | 0.84% | 0.99 | 432 | 79,215 | 0.05 | 0.00% | 0.18% | 1.00 |
| band-weak-levels | 5% | 14.5 | 17,486 | 0.54 | 0.00% | 0.51% | 0.98 | 1,110 | 17,490 | 0.06 | 0.00% | 0.46% | 1.00 |
| band-weak-levels | 10% | 13.4 | 17,489 | 0.73 | 0.00% | 0.47% | 0.97 | 1,082 | 17,489 | 0.08 | 0.00% | 0.45% | 1.00 |
| band-weak-levels | 20% | 12.5 | 17,489 | 1.07 | 0.00% | 0.43% | 0.98 | 1,032 | 17,489 | 0.11 | 0.00% | 0.42% | 1.00 |
| band-weak-levels | 30% | 12.2 | 17,483 | 1.19 | 0.20% | 0.44% | 0.94 | 1,005 | 17,483 | 0.13 | 0.00% | 0.41% | 1.00 |
| band-weak-levels, weak work | 1% | 24.9 | 17,476 | 0.21 | 0.00% | 0.85% | 1.00 | 432 | 83,507 | 0.05 | 0.00% | 0.18% | 1.00 |
| band-weak-levels, weak work | 5% | 24.4 | 17,479 | 0.24 | 0.00% | 0.85% | 0.99 | 1,792 | 17,481 | 0.03 | 0.00% | 0.74% | 1.00 |
| band-weak-levels, weak work | 10% | 25.0 | 17,476 | 0.26 | 0.00% | 0.85% | 1.01 | 1,966 | 17,476 | 0.03 | 0.00% | 0.81% | 1.00 |
| band-weak-levels, weak work | 20% | 24.4 | 17,480 | 0.33 | 0.00% | 0.85% | 0.98 | 1,947 | 17,480 | 0.04 | 0.00% | 0.80% | 1.00 |
| band-weak-levels, weak work | 30% | 24.0 | 17,476 | 0.39 | 0.00% | 0.84% | 0.97 | 1,908 | 17,476 | 0.04 | 0.00% | 0.78% | 1.00 |

#### Mix: 50% industrial

| Design | Pool | Bitaxe pay/mo | Bitaxe median sat | Bitaxe swing | Bitaxe empty months | Bitaxe fee loss | Bitaxe vs fair | ASIC pay/mo | ASIC median sat | ASIC swing | ASIC empty months | ASIC fee loss | ASIC vs fair |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| mini | 1% | 45.8 | 8,407 | 0.29 | 0.00% | 1.57% | 1.00 | 217 | 168,207 | 0.08 | 0.00% | 0.09% | 1.00 |
| mini | 5% | 10.1 | 42,077 | 0.62 | 4.60% | 0.35% | 0.98 | 593 | 42,167 | 0.07 | 0.00% | 0.24% | 1.00 |
| mini | 10% | 4.68 | 84,062 | 0.92 | 21.60% | 0.18% | 0.91 | 390 | 84,151 | 0.10 | 0.00% | 0.16% | 1.00 |
| mini | 20% | 2.39 | 167,690 | 1.26 | 46.40% | 0.09% | 0.92 | 210 | 167,779 | 0.14 | 0.00% | 0.09% | 1.00 |
| mini | 30% | 1.54 | 250,867 | 1.57 | 60.30% | 0.06% | 0.89 | 143 | 250,955 | 0.17 | 0.00% | 0.06% | 1.00 |
| one-chain | 1% | 24.9 | 16,853 | 0.39 | 0.00% | 0.86% | 0.99 | 430 | 84,314 | 0.06 | 0.00% | 0.18% | 1.00 |
| one-chain | 5% | 4.95 | 84,130 | 0.89 | 21.40% | 0.18% | 0.96 | 391 | 84,130 | 0.10 | 0.00% | 0.16% | 1.00 |
| one-chain | 10% | 2.37 | 167,758 | 1.28 | 46.30% | 0.09% | 0.91 | 210 | 167,758 | 0.13 | 0.00% | 0.09% | 1.00 |
| one-chain | 20% | 1.12 | 333,661 | 1.89 | 69.30% | 0.04% | 0.86 | 107 | 333,661 | 0.20 | 0.00% | 0.04% | 0.99 |
| one-chain | 30% | 0.803 | 497,785 | 2.28 | 78.10% | 0.03% | 0.92 | 71.4 | 497,785 | 0.23 | 0.00% | 0.03% | 0.99 |
| band | 1% | 45.1 | 9,080 | 0.30 | 0.00% | 1.55% | 0.99 | 432 | 81,751 | 0.06 | 0.00% | 0.18% | 1.00 |
| band | 5% | 5.50 | 74,364 | 0.85 | 17.50% | 0.20% | 0.94 | 438 | 74,364 | 0.09 | 0.00% | 0.18% | 1.01 |
| band | 10% | 2.37 | 167,758 | 1.28 | 46.30% | 0.09% | 0.91 | 210 | 167,758 | 0.13 | 0.00% | 0.09% | 1.00 |
| band | 20% | 1.12 | 333,661 | 1.89 | 69.30% | 0.04% | 0.86 | 107 | 333,661 | 0.20 | 0.00% | 0.04% | 0.99 |
| band | 30% | 0.803 | 497,785 | 2.28 | 78.10% | 0.03% | 0.92 | 71.4 | 497,785 | 0.23 | 0.00% | 0.03% | 0.99 |
| vardiff-cd | 1% | 48.2 | 8,453 | 0.30 | 0.00% | 1.66% | 0.99 | 49.0 | 686,130 | 0.27 | 0.00% | 0.02% | 0.98 |
| vardiff-cd | 5% | 9.99 | 42,242 | 0.63 | 3.80% | 0.35% | 0.97 | 19.2 | 1,767,125 | 0.44 | 0.10% | 0.01% | 0.94 |
| vardiff-cd | 10% | 4.92 | 84,473 | 0.89 | 20.30% | 0.18% | 0.96 | 19.1 | 1,766,830 | 0.46 | 0.20% | 0.01% | 0.94 |
| vardiff-cd | 20% | 2.34 | 168,900 | 1.26 | 46.30% | 0.09% | 0.91 | 19.3 | 1,766,924 | 0.46 | 0.20% | 0.01% | 0.94 |
| vardiff-cd | 30% | 1.57 | 253,281 | 1.55 | 60.50% | 0.06% | 0.92 | 19.3 | 1,767,169 | 0.46 | 0.30% | 0.01% | 0.94 |
| band-weak | 1% | 405 | 721 | 0.17 | 0.00% | 10.44% | 1.32 | 432 | 109,345 | 0.05 | 0.00% | 0.13% | 1.33 |
| band-weak | 5% | 133 | 1,810 | 0.47 | 0.00% | 4.03% | 1.13 | 2,149 | 9,050 | 0.05 | 0.00% | 0.77% | 1.14 |
| band-weak | 10% | 45.3 | 4,802 | 0.73 | 0.00% | 1.59% | 0.97 | 2,518 | 4,802 | 0.08 | 0.00% | 1.03% | 1.00 |
| band-weak | 20% | 22.8 | 9,603 | 1.01 | 0.00% | 0.83% | 0.94 | 1,697 | 9,603 | 0.11 | 0.00% | 0.70% | 1.00 |
| band-weak | 30% | 15.1 | 14,401 | 1.27 | 0.00% | 0.54% | 0.96 | 1,198 | 14,401 | 0.14 | 0.00% | 0.49% | 1.00 |
| band-weak-levels | 1% | 32.6 | 17,479 | 0.22 | 0.00% | 0.84% | 1.32 | 432 | 109,345 | 0.05 | 0.00% | 0.13% | 1.33 |
| band-weak-levels | 5% | 17.1 | 17,497 | 0.47 | 0.00% | 0.52% | 1.11 | 1,266 | 17,493 | 0.06 | 0.00% | 0.46% | 1.14 |
| band-weak-levels | 10% | 13.4 | 17,487 | 0.72 | 0.00% | 0.47% | 0.97 | 1,081 | 17,487 | 0.08 | 0.00% | 0.45% | 1.00 |
| band-weak-levels | 20% | 12.5 | 17,494 | 1.07 | 0.00% | 0.44% | 0.98 | 1,031 | 17,494 | 0.11 | 0.00% | 0.42% | 1.00 |
| band-weak-levels | 30% | 12.1 | 17,506 | 1.19 | 0.30% | 0.44% | 0.94 | 1,003 | 17,506 | 0.13 | 0.00% | 0.41% | 1.00 |
| band-weak-levels, weak work | 1% | 25.2 | 17,481 | 0.20 | 0.00% | 0.85% | 1.02 | 432 | 83,681 | 0.05 | 0.00% | 0.18% | 1.00 |
| band-weak-levels, weak work | 5% | 24.8 | 17,480 | 0.23 | 0.00% | 0.85% | 1.00 | 1,833 | 18,093 | 0.03 | 0.00% | 0.75% | 1.00 |
| band-weak-levels, weak work | 10% | 24.8 | 17,485 | 0.27 | 0.00% | 0.85% | 1.00 | 1,967 | 17,491 | 0.03 | 0.00% | 0.81% | 1.00 |
| band-weak-levels, weak work | 20% | 24.4 | 17,487 | 0.34 | 0.00% | 0.85% | 0.98 | 1,946 | 17,487 | 0.04 | 0.00% | 0.80% | 1.00 |
| band-weak-levels, weak work | 30% | 24.0 | 17,487 | 0.39 | 0.00% | 0.84% | 0.97 | 1,906 | 17,487 | 0.04 | 0.00% | 0.78% | 1.00 |

#### Mix: 90% industrial

| Design | Pool | Bitaxe pay/mo | Bitaxe median sat | Bitaxe swing | Bitaxe empty months | Bitaxe fee loss | Bitaxe vs fair | ASIC pay/mo | ASIC median sat | ASIC swing | ASIC empty months | ASIC fee loss | ASIC vs fair |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| mini | 1% | 42.8 | 10,093 | 0.19 | 0.00% | 1.46% | 1.00 | 388 | 91,088 | 0.06 | 0.00% | 0.16% | 1.00 |
| mini | 5% | 46.1 | 8,425 | 0.28 | 0.00% | 1.57% | 1.00 | 423 | 75,745 | 0.10 | 0.00% | 0.17% | 1.00 |
| mini | 10% | 24.9 | 16,849 | 0.39 | 0.00% | 0.86% | 0.99 | 232 | 151,072 | 0.13 | 0.00% | 0.10% | 1.00 |
| mini | 20% | 12.8 | 33,685 | 0.55 | 2.00% | 0.44% | 1.00 | 120 | 300,627 | 0.19 | 0.00% | 0.05% | 1.00 |
| mini | 30% | 8.47 | 50,501 | 0.66 | 6.00% | 0.29% | 0.99 | 79.7 | 448,736 | 0.22 | 0.00% | 0.03% | 0.99 |
| one-chain | 1% | 24.9 | 16,853 | 0.39 | 0.00% | 0.86% | 0.99 | 430 | 84,314 | 0.06 | 0.00% | 0.18% | 1.00 |
| one-chain | 5% | 4.95 | 84,130 | 0.89 | 21.40% | 0.18% | 0.96 | 391 | 84,130 | 0.10 | 0.00% | 0.16% | 1.00 |
| one-chain | 10% | 2.37 | 167,758 | 1.28 | 46.30% | 0.09% | 0.91 | 210 | 167,758 | 0.13 | 0.00% | 0.09% | 1.00 |
| one-chain | 20% | 1.12 | 333,661 | 1.89 | 69.30% | 0.04% | 0.86 | 107 | 333,661 | 0.20 | 0.00% | 0.04% | 0.99 |
| one-chain | 30% | 0.803 | 497,785 | 2.28 | 78.10% | 0.03% | 0.92 | 71.4 | 497,785 | 0.23 | 0.00% | 0.03% | 0.99 |
| band | 1% | 169 | 2,066 | 0.14 | 0.00% | 5.66% | 1.02 | 433 | 84,044 | 0.05 | 0.00% | 0.18% | 1.01 |
| band | 5% | 6.54 | 63,632 | 0.78 | 13.50% | 0.23% | 0.96 | 501 | 63,632 | 0.08 | 0.00% | 0.20% | 1.00 |
| band | 10% | 2.37 | 167,758 | 1.28 | 46.30% | 0.09% | 0.91 | 210 | 167,758 | 0.13 | 0.00% | 0.09% | 1.00 |
| band | 20% | 1.12 | 333,661 | 1.89 | 69.30% | 0.04% | 0.86 | 107 | 333,661 | 0.20 | 0.00% | 0.04% | 0.99 |
| band | 30% | 0.803 | 497,785 | 2.28 | 78.10% | 0.03% | 0.92 | 71.4 | 497,785 | 0.23 | 0.00% | 0.03% | 0.99 |
| vardiff-cd | 1% | 193 | 1,729 | 0.13 | 0.00% | 6.49% | 1.02 | 193 | 140,539 | 0.13 | 0.00% | 0.08% | 0.99 |
| vardiff-cd | 5% | 50.5 | 8,642 | 0.28 | 0.00% | 1.70% | 1.02 | 51.2 | 689,810 | 0.27 | 0.00% | 0.02% | 0.99 |
| vardiff-cd | 10% | 25.2 | 17,278 | 0.39 | 0.10% | 0.86% | 1.01 | 25.8 | 1,349,448 | 0.39 | 0.10% | 0.01% | 0.96 |
| vardiff-cd | 20% | 12.7 | 34,639 | 0.55 | 2.00% | 0.43% | 1.02 | 19.3 | 1,766,924 | 0.45 | 0.20% | 0.01% | 0.95 |
| vardiff-cd | 30% | 8.31 | 52,158 | 0.69 | 6.60% | 0.28% | 1.00 | 19.2 | 1,767,169 | 0.46 | 0.00% | 0.01% | 0.94 |
| band-weak | 1% | 431 | 1,055 | 0.08 | 0.00% | 8.11% | 1.82 | 431 | 77,222 | 0.05 | 0.00% | 0.19% | 0.91 |
| band-weak | 5% | 200 | 1,327 | 0.43 | 0.00% | 5.39% | 1.27 | 2,159 | 10,622 | 0.05 | 0.00% | 0.69% | 1.28 |
| band-weak | 10% | 45.3 | 4,802 | 0.73 | 0.00% | 1.59% | 0.97 | 2,518 | 4,802 | 0.08 | 0.00% | 1.03% | 1.00 |
| band-weak | 20% | 22.8 | 9,603 | 1.01 | 0.00% | 0.83% | 0.94 | 1,697 | 9,603 | 0.11 | 0.00% | 0.70% | 1.00 |
| band-weak | 30% | 15.1 | 14,401 | 1.27 | 0.00% | 0.54% | 0.96 | 1,198 | 14,401 | 0.14 | 0.00% | 0.49% | 1.00 |
| band-weak-levels | 1% | 44.7 | 17,500 | 0.16 | 0.00% | 0.85% | 1.81 | 432 | 77,218 | 0.05 | 0.00% | 0.19% | 0.91 |
| band-weak-levels | 5% | 19.9 | 17,585 | 0.46 | 0.00% | 0.53% | 1.28 | 1,422 | 17,597 | 0.05 | 0.00% | 0.45% | 1.28 |
| band-weak-levels | 10% | 13.3 | 17,601 | 0.72 | 0.00% | 0.47% | 0.97 | 1,075 | 17,601 | 0.08 | 0.00% | 0.44% | 0.99 |
| band-weak-levels | 20% | 12.6 | 17,598 | 1.07 | 0.00% | 0.44% | 0.99 | 1,026 | 17,598 | 0.11 | 0.00% | 0.42% | 1.00 |
| band-weak-levels | 30% | 12.1 | 17,508 | 1.19 | 0.20% | 0.44% | 0.94 | 1,003 | 17,508 | 0.13 | 0.00% | 0.41% | 1.00 |
| band-weak-levels, weak work | 1% | 25.3 | 17,481 | 0.20 | 0.00% | 0.85% | 1.02 | 432 | 84,205 | 0.05 | 0.00% | 0.18% | 1.00 |
| band-weak-levels, weak work | 5% | 24.5 | 17,502 | 0.22 | 0.00% | 0.85% | 0.99 | 1,871 | 17,576 | 0.03 | 0.00% | 0.77% | 1.00 |
| band-weak-levels, weak work | 10% | 24.9 | 17,534 | 0.26 | 0.00% | 0.84% | 1.01 | 1,964 | 17,567 | 0.03 | 0.00% | 0.81% | 1.00 |
| band-weak-levels, weak work | 20% | 24.4 | 17,565 | 0.33 | 0.00% | 0.84% | 0.99 | 1,941 | 17,565 | 0.04 | 0.00% | 0.80% | 1.00 |
| band-weak-levels, weak work | 30% | 23.9 | 17,474 | 0.39 | 0.00% | 0.84% | 0.97 | 1,908 | 17,474 | 0.04 | 0.00% | 0.78% | 1.00 |


### Designs as Bitcoin Cash's hashrate grows (pool 10%, 50% industrial)

| Design | BCH hashrate | Bitaxe fair sat/mo | Bitaxe pay/mo | Bitaxe median sat | Bitaxe swing | Bitaxe empty months | Bitaxe fee loss | Bitaxe vs fair | ASIC pay/mo | ASIC median sat | ASIC swing | ASIC empty months | ASIC fee loss | ASIC vs fair |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| mini | x1 | 433,459 | 4.68 | 84,062 | 0.92 | 21.60% | 0.18% | 0.91 | 390 | 84,151 | 0.10 | 0.00% | 0.16% | 1.00 |
| mini | x3 | 144,486 | 1.55 | 84,076 | 1.57 | 60.40% | 0.18% | 0.90 | 140 | 84,106 | 0.17 | 0.00% | 0.17% | 1.01 |
| mini | x10 | 43,346 | 0.494 | 84,081 | 2.90 | 85.90% | 0.18% | 0.96 | 42.5 | 84,090 | 0.30 | 0.00% | 0.17% | 1.00 |
| mini | x270 | 1,605 | 0.023 | 84,084 | 12.54 | 99.30% | 0.18% | 1.19 | 1.61 | 84,084 | 1.57 | 60.80% | 0.18% | 1.01 |
| one-chain | x1 | 433,459 | 2.37 | 167,758 | 1.28 | 46.30% | 0.09% | 0.91 | 210 | 167,758 | 0.13 | 0.00% | 0.09% | 1.00 |
| one-chain | x3 | 144,486 | 0.766 | 167,727 | 2.30 | 77.90% | 0.09% | 0.89 | 71.5 | 167,727 | 0.24 | 0.00% | 0.09% | 1.00 |
| one-chain | x10 | 43,346 | 0.265 | 167,716 | 3.79 | 92.10% | 0.09% | 1.02 | 21.3 | 167,716 | 0.43 | 0.20% | 0.09% | 0.99 |
| one-chain | x270 | 1,605 | 0.030 | 167,712 | 13.69 | 99.40% | 0.09% | 3.10 | 0.727 | 167,712 | 2.40 | 79.60% | 0.09% | 0.91 |
| band | x1 | 433,459 | 2.37 | 167,758 | 1.28 | 46.30% | 0.09% | 0.91 | 210 | 167,758 | 0.13 | 0.00% | 0.09% | 1.00 |
| band | x3 | 144,486 | 0.766 | 167,727 | 2.30 | 77.90% | 0.09% | 0.89 | 71.5 | 167,727 | 0.24 | 0.00% | 0.09% | 1.00 |
| band | x10 | 43,346 | 0.265 | 167,716 | 3.79 | 92.10% | 0.09% | 1.02 | 21.3 | 167,716 | 0.43 | 0.20% | 0.09% | 0.99 |
| band | x270 | 1,605 | 0.030 | 167,712 | 13.69 | 99.40% | 0.09% | 3.10 | 0.727 | 167,712 | 2.40 | 79.60% | 0.09% | 0.91 |
| vardiff-cd | x1 | 433,459 | 4.92 | 84,473 | 0.89 | 20.30% | 0.18% | 0.96 | 19.1 | 1,766,830 | 0.46 | 0.20% | 0.01% | 0.94 |
| vardiff-cd | x3 | 144,486 | 1.59 | 84,890 | 1.55 | 60.50% | 0.17% | 0.93 | 19.1 | 612,376 | 0.45 | 0.40% | 0.02% | 0.97 |
| vardiff-cd | x10 | 43,346 | 0.485 | 86,385 | 2.91 | 86.30% | 0.17% | 0.96 | 18.9 | 186,531 | 0.46 | 0.30% | 0.08% | 0.98 |
| vardiff-cd | x270 | 1,605 | 9.0e-3 | 134,123 | 14.17 | 99.40% | 0.11% | 0.73 | 0.898 | 134,123 | 2.16 | 75.10% | 0.11% | 0.90 |
| band-weak | x1 | 433,459 | 45.3 | 4,802 | 0.73 | 0.00% | 1.59% | 0.97 | 2,518 | 4,802 | 0.08 | 0.00% | 1.03% | 1.00 |
| band-weak | x3 | 144,486 | 14.9 | 4,802 | 1.24 | 0.00% | 1.65% | 0.93 | 1,093 | 4,802 | 0.14 | 0.00% | 1.34% | 1.00 |
| band-weak | x10 | 43,346 | 4.44 | 4,801 | 2.43 | 9.70% | 1.43% | 1.06 | 363 | 4,801 | 0.25 | 0.00% | 1.49% | 1.00 |
| band-weak | x270 | 1,605 | 0.186 | 4,801 | 10.70 | 92.00% | 0.96% | 1.79 | 13.8 | 4,801 | 1.24 | 0.00% | 1.69% | 0.91 |
| band-weak-levels | x1 | 433,459 | 13.4 | 17,487 | 0.72 | 0.00% | 0.47% | 0.97 | 1,081 | 17,487 | 0.08 | 0.00% | 0.45% | 1.00 |
| band-weak-levels | x3 | 144,486 | 4.26 | 17,477 | 1.27 | 3.40% | 0.44% | 0.99 | 369 | 17,477 | 0.14 | 0.00% | 0.45% | 1.00 |
| band-weak-levels | x10 | 43,346 | 1.24 | 17,492 | 2.23 | 37.00% | 0.49% | 0.87 | 111 | 17,492 | 0.26 | 0.00% | 0.46% | 0.98 |
| band-weak-levels | x270 | 1,605 | 0.043 | 17,504 | 11.66 | 96.60% | 0.32% | 1.23 | 4.11 | 17,504 | 1.38 | 4.20% | 0.43% | 1.07 |
| band-weak-levels, weak work | x1 | 433,459 | 24.8 | 17,485 | 0.27 | 0.00% | 0.85% | 1.00 | 1,967 | 17,491 | 0.03 | 0.00% | 0.81% | 1.00 |
| band-weak-levels, weak work | x3 | 144,486 | 8.07 | 17,473 | 0.47 | 0.40% | 0.85% | 0.98 | 688 | 17,473 | 0.06 | 0.00% | 0.84% | 1.00 |
| band-weak-levels, weak work | x10 | 43,346 | 2.36 | 17,479 | 0.89 | 19.70% | 0.85% | 0.95 | 207 | 17,479 | 0.09 | 0.00% | 0.85% | 1.00 |
| band-weak-levels, weak work | x270 | 1,605 | 0.096 | 17,483 | 4.62 | 94.50% | 0.85% | 1.05 | 7.58 | 17,483 | 0.49 | 0.40% | 0.85% | 0.99 |


### Pool load (50% industrial)

| Design | Pool | ASERT difficulty | Easiest share | C settles at | R | Weak per share | Weak expired | Window (Bitaxe's chain) | Shares/s | Uncles/s | Weak/s | KB/s |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| mini | 1% | 42,056,519 | 42,056,519 | - | - | - | - | 10.0 h | 0.204 | 4.3e-3 | 0 | 0.204 |
| mini | 5% | 210,733,007 | 210,733,007 | - | - | - | - | 2.0 h | 0.204 | 4.3e-3 | 0 | 0.204 |
| mini | 10% | 421,578,617 | 421,578,617 | - | - | - | - | 1.0 h | 0.204 | 4.3e-3 | 0 | 0.204 |
| mini | 20% | 843,269,838 | 843,269,838 | - | - | - | - | 0.5 h | 0.204 | 4.3e-3 | 0 | 0.204 |
| mini | 30% | 1,264,961,058 | 1,264,961,058 | - | - | - | - | 0.3 h | 0.204 | 4.3e-3 | 0 | 0.204 |
| one-chain | 1% | 84,338,244 | 84,338,244 | - | - | - | - | 5.0 h | 0.102 | 2.1e-3 | 0 | 0.102 |
| one-chain | 5% | 421,691,220 | 421,691,220 | - | - | - | - | 1.0 h | 0.102 | 2.1e-3 | 0 | 0.102 |
| one-chain | 10% | 843,382,441 | 843,382,441 | - | - | - | - | 0.5 h | 0.102 | 2.1e-3 | 0 | 0.102 |
| one-chain | 20% | 1,686,764,881 | 1,686,764,881 | - | - | - | - | 0.2 h | 0.102 | 2.1e-3 | 0 | 0.102 |
| one-chain | 30% | 2,530,147,322 | 2,530,147,322 | - | - | - | - | 0.2 h | 0.102 | 2.1e-3 | 0 | 0.102 |
| band | 1% | 45,342,948 | 45,342,948 | 1.00 | - | - | - | 5.0 h | 0.102 | 2.1e-3 | 0 | 0.102 |
| band | 5% | 372,519,366 | 372,519,366 | 1.00 | - | - | - | 1.0 h | 0.102 | 2.1e-3 | 0 | 0.102 |
| band | 10% | 843,382,441 | 843,382,441 | 1.00 | - | - | - | 0.5 h | 0.102 | 2.1e-3 | 0 | 0.102 |
| band | 20% | 1,686,764,881 | 1,686,764,881 | 1.00 | - | - | - | 0.2 h | 0.102 | 2.1e-3 | 0 | 0.102 |
| band | 30% | 2,530,147,322 | 2,530,147,322 | 1.00 | - | - | - | 0.2 h | 0.102 | 2.1e-3 | 0 | 0.102 |
| vardiff-cd | 1% | 1,297,682,804,146 | 42,087,010 | - | - | - | - | 5.0 h | 0.102 | 2.1e-3 | 0 | 0.102 |
| vardiff-cd | 5% | 17,367,303,371,429 | 210,998,631 | - | - | - | - | 1.0 h | 0.102 | 2.1e-3 | 0 | 0.102 |
| vardiff-cd | 10% | 34,734,606,742,859 | 422,632,736 | - | - | - | - | 0.5 h | 0.102 | 2.1e-3 | 0 | 0.102 |
| vardiff-cd | 20% | 69,469,213,485,718 | 847,477,341 | - | - | - | - | 0.2 h | 0.102 | 2.1e-3 | 0 | 0.102 |
| vardiff-cd | 30% | 104,203,820,228,577 | 1,274,436,885 | - | - | - | - | 0.2 h | 0.102 | 2.1e-3 | 0 | 0.102 |
| band-weak | 1% | 45,342,948 | 45,342,948 | 1.00 | 37.2 | 34.5 | 0.00% | 3.4 h | 0.102 | 2.1e-3 | 3.45 | 2.86 |
| band-weak | 5% | 372,519,366 | 372,519,366 | 1.00 | 40.7 | 34.5 | 0.00% | 0.6 h | 0.102 | 2.1e-3 | 3.45 | 2.86 |
| band-weak | 10% | 843,382,441 | 843,382,441 | 1.00 | 34.7 | 34.5 | 0.00% | 0.3 h | 0.102 | 2.1e-3 | 3.45 | 2.86 |
| band-weak | 20% | 1,686,764,881 | 1,686,764,881 | 1.00 | 34.7 | 34.5 | 0.00% | 0.1 h | 0.102 | 2.1e-3 | 3.45 | 2.86 |
| band-weak | 30% | 2,530,147,322 | 2,530,147,322 | 1.00 | 34.7 | 34.5 | 0.00% | 0.1 h | 0.102 | 2.1e-3 | 3.45 | 2.86 |
| band-weak-levels | 1% | 45,342,948 | 45,342,948 | 1.00 | 37.2 | 34.5 | 0.00% | 3.4 h | 0.102 | 2.1e-3 | 3.45 | 2.86 |
| band-weak-levels | 5% | 372,519,366 | 372,519,366 | 1.00 | 40.7 | 34.5 | 0.00% | 0.6 h | 0.102 | 2.1e-3 | 3.45 | 2.86 |
| band-weak-levels | 10% | 843,382,441 | 843,382,441 | 1.00 | 34.7 | 34.5 | 0.00% | 0.3 h | 0.102 | 2.1e-3 | 3.45 | 2.86 |
| band-weak-levels | 20% | 1,686,764,881 | 1,686,764,881 | 1.00 | 34.7 | 34.5 | 0.00% | 0.1 h | 0.102 | 2.1e-3 | 3.45 | 2.86 |
| band-weak-levels | 30% | 2,530,147,322 | 2,530,147,322 | 1.00 | 34.7 | 34.5 | 0.00% | 0.1 h | 0.102 | 2.1e-3 | 3.45 | 2.86 |
| band-weak-levels, weak work | 1% | 45,342,948 | 45,342,948 | 1.00 | 37.2 | 34.5 | 0.00% | 5.0 h | 0.102 | 2.1e-3 | 3.45 | 2.86 |
| band-weak-levels, weak work | 5% | 372,519,366 | 372,519,366 | 1.00 | 40.7 | 34.5 | 0.00% | 1.0 h | 0.102 | 2.1e-3 | 3.45 | 2.86 |
| band-weak-levels, weak work | 10% | 843,382,441 | 843,382,441 | 1.00 | 34.7 | 34.5 | 0.00% | 0.5 h | 0.102 | 2.1e-3 | 3.45 | 2.86 |
| band-weak-levels, weak work | 20% | 1,686,764,881 | 1,686,764,881 | 1.00 | 34.7 | 34.5 | 0.00% | 0.2 h | 0.102 | 2.1e-3 | 3.45 | 2.86 |
| band-weak-levels, weak work | 30% | 2,530,147,322 | 2,530,147,322 | 1.00 | 34.7 | 34.5 | 0.00% | 0.2 h | 0.102 | 2.1e-3 | 3.45 | 2.86 |


### Weak share credit: as the page writes it and credited once (band-weak-levels)

| Mix | Pool | Credit | Window | Bitaxe expected vs fair | Bitaxe simulated vs fair | Bitaxe swing | ASIC share multiple | ASIC expected vs fair | ASIC simulated vs fair |
|---|---|---|---|---|---|---|---|---|---|
| home | 1% | page | 2.5 h | 1.00 | 0.99 | 0.27 | 1.00 | 1.00 | 1.00 |
| home | 1% | weak work | 5.0 h | 1.00 | 1.00 | 0.21 | 1.00 | 1.00 | 1.00 |
| home | 10% | page | 0.3 h | 1.00 | 0.97 | 0.73 | 1.00 | 1.00 | 1.00 |
| home | 10% | weak work | 0.5 h | 1.00 | 1.01 | 0.26 | 1.00 | 1.00 | 1.00 |
| home | 30% | page | 0.1 h | 1.00 | 0.94 | 1.19 | 1.00 | 1.00 | 1.00 |
| home | 30% | weak work | 0.2 h | 1.00 | 0.97 | 0.39 | 1.00 | 1.00 | 1.00 |
| 50% industrial | 1% | page | 3.4 h | 1.33 | 1.32 | 0.22 | 1.00 | 1.33 | 1.33 |
| 50% industrial | 1% | weak work | 5.0 h | 1.00 | 1.02 | 0.20 | 1.00 | 1.00 | 1.00 |
| 50% industrial | 10% | page | 0.3 h | 1.00 | 0.97 | 0.72 | 1.00 | 1.00 | 1.00 |
| 50% industrial | 10% | weak work | 0.5 h | 1.00 | 1.00 | 0.27 | 1.00 | 1.00 | 1.00 |
| 50% industrial | 30% | page | 0.1 h | 1.00 | 0.94 | 1.19 | 1.00 | 1.00 | 1.00 |
| 50% industrial | 30% | weak work | 0.2 h | 1.00 | 0.97 | 0.39 | 1.00 | 1.00 | 1.00 |
| 90% industrial | 1% | page | 4.6 h | 1.81 | 1.81 | 0.16 | 3.43 | 0.92 | 0.91 |
| 90% industrial | 1% | weak work | 5.0 h | 1.01 | 1.02 | 0.20 | 3.43 | 1.00 | 1.00 |
| 90% industrial | 10% | page | 0.3 h | 1.00 | 0.97 | 0.72 | 1.00 | 1.00 | 0.99 |
| 90% industrial | 10% | weak work | 0.5 h | 1.00 | 1.01 | 0.26 | 1.00 | 1.00 | 1.00 |
| 90% industrial | 30% | page | 0.1 h | 1.00 | 0.94 | 1.19 | 1.00 | 1.00 | 1.00 |
| 90% industrial | 30% | weak work | 0.2 h | 1.00 | 0.97 | 0.39 | 1.00 | 1.00 | 1.00 |


### Band cap C (design band)

| Mix | Pool | C | C settles at | Minimum / uniform | ASIC share multiple | Bitaxe shares/mo | Bitaxe pay/mo | Bitaxe swing | Bitaxe empty months | ASIC pay/mo | ASIC swing |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 50% industrial | 1% | 1.00 | 1.00 | 1.00 | 1.00 | 8.53 | 24.9 | 0.39 | 0.00% | 430 | 0.06 |
| 50% industrial | 1% | 8.00 | 1.00 | 0.57 | 1.00 | 15.1 | 43.2 | 0.29 | 0.00% | 431 | 0.06 |
| 50% industrial | 1% | 30.0 | 1.00 | 0.54 | 1.00 | 15.8 | 45.3 | 0.30 | 0.00% | 432 | 0.06 |
| 50% industrial | 1% | 64.0 | 1.00 | 0.54 | 1.00 | 15.9 | 45.1 | 0.30 | 0.00% | 432 | 0.06 |
| 50% industrial | 1% | 256 | 1.00 | 0.54 | 1.00 | 16.0 | 45.6 | 0.29 | 0.00% | 432 | 0.06 |
| 50% industrial | 10% | 1.00 | 1.00 | 1.00 | 1.00 | 0.795 | 2.37 | 1.28 | 46.30% | 210 | 0.13 |
| 50% industrial | 10% | 8.00 | 1.00 | 1.00 | 1.00 | 0.795 | 2.37 | 1.28 | 46.30% | 210 | 0.13 |
| 50% industrial | 10% | 30.0 | 1.00 | 1.00 | 1.00 | 0.795 | 2.37 | 1.28 | 46.30% | 210 | 0.13 |
| 50% industrial | 10% | 64.0 | 1.00 | 1.00 | 1.00 | 0.795 | 2.37 | 1.28 | 46.30% | 210 | 0.13 |
| 50% industrial | 10% | 256 | 1.00 | 1.00 | 1.00 | 0.795 | 2.37 | 1.28 | 46.30% | 210 | 0.13 |
| 50% industrial | 30% | 1.00 | 1.00 | 1.00 | 1.00 | 0.256 | 0.803 | 2.28 | 78.10% | 71.4 | 0.23 |
| 50% industrial | 30% | 8.00 | 1.00 | 1.00 | 1.00 | 0.256 | 0.803 | 2.28 | 78.10% | 71.4 | 0.23 |
| 50% industrial | 30% | 30.0 | 1.00 | 1.00 | 1.00 | 0.256 | 0.803 | 2.28 | 78.10% | 71.4 | 0.23 |
| 50% industrial | 30% | 64.0 | 1.00 | 1.00 | 1.00 | 0.256 | 0.803 | 2.28 | 78.10% | 71.4 | 0.23 |
| 50% industrial | 30% | 256 | 1.00 | 1.00 | 1.00 | 0.256 | 0.803 | 2.28 | 78.10% | 71.4 | 0.23 |
| 90% industrial | 1% | 1.00 | 1.00 | 1.00 | 1.00 | 8.53 | 24.9 | 0.39 | 0.00% | 430 | 0.06 |
| 90% industrial | 1% | 8.00 | 8.00 | 0.21 | 1.94 | 40.4 | 106 | 0.19 | 0.00% | 433 | 0.06 |
| 90% industrial | 1% | 30.0 | 30.0 | 0.13 | 3.17 | 66.0 | 160 | 0.15 | 0.00% | 433 | 0.05 |
| 90% industrial | 1% | 64.0 | 64.0 | 0.12 | 3.43 | 71.5 | 169 | 0.14 | 0.00% | 433 | 0.05 |
| 90% industrial | 1% | 256 | 256 | 0.11 | 3.61 | 75.2 | 176 | 0.14 | 0.00% | 433 | 0.05 |
| 90% industrial | 10% | 1.00 | 1.00 | 1.00 | 1.00 | 0.795 | 2.37 | 1.28 | 46.30% | 210 | 0.13 |
| 90% industrial | 10% | 8.00 | 1.00 | 1.00 | 1.00 | 0.795 | 2.37 | 1.28 | 46.30% | 210 | 0.13 |
| 90% industrial | 10% | 30.0 | 1.00 | 1.00 | 1.00 | 0.795 | 2.37 | 1.28 | 46.30% | 210 | 0.13 |
| 90% industrial | 10% | 64.0 | 1.00 | 1.00 | 1.00 | 0.795 | 2.37 | 1.28 | 46.30% | 210 | 0.13 |
| 90% industrial | 10% | 256 | 1.00 | 1.00 | 1.00 | 0.795 | 2.37 | 1.28 | 46.30% | 210 | 0.13 |
| 90% industrial | 30% | 1.00 | 1.00 | 1.00 | 1.00 | 0.256 | 0.803 | 2.28 | 78.10% | 71.4 | 0.23 |
| 90% industrial | 30% | 8.00 | 1.00 | 1.00 | 1.00 | 0.256 | 0.803 | 2.28 | 78.10% | 71.4 | 0.23 |
| 90% industrial | 30% | 30.0 | 1.00 | 1.00 | 1.00 | 0.256 | 0.803 | 2.28 | 78.10% | 71.4 | 0.23 |
| 90% industrial | 30% | 64.0 | 1.00 | 1.00 | 1.00 | 0.256 | 0.803 | 2.28 | 78.10% | 71.4 | 0.23 |
| 90% industrial | 30% | 256 | 1.00 | 1.00 | 1.00 | 0.256 | 0.803 | 2.28 | 78.10% | 71.4 | 0.23 |


### WEAK_CAP and WEAK_FILL (band-weak-levels, 50% industrial)

| Credit | Pool | WEAK_CAP | WEAK_FILL | R | Weak found per share | Carried per share | Shares at the cap | Weak expired | KB/s | Bitaxe weak/mo | Bitaxe pay/mo | Bitaxe swing | Bitaxe empty months |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| page | 1% | 16 | 8 | 10.00 | 8.56 | 8.56 | 26.98% | 0.00% | 0.787 | 144 | 32.2 | 0.22 | 0.00% |
| page | 1% | 32 | 16 | 19.1 | 17.2 | 17.2 | 26.09% | 0.00% | 1.48 | 289 | 32.2 | 0.21 | 0.00% |
| page | 1% | 64 | 32 | 37.2 | 34.5 | 34.5 | 25.41% | 0.00% | 2.86 | 579 | 32.6 | 0.22 | 0.00% |
| page | 1% | 128 | 64 | 73.5 | 69.0 | 69.0 | 25.21% | 0.00% | 5.62 | 1,157 | 32.9 | 0.21 | 0.00% |
| page | 1% | 64 | 16 | 23.7 | 21.7 | 21.7 | 6.78% | 0.00% | 1.84 | 364 | 32.7 | 0.22 | 0.00% |
| page | 1% | 64 | 48 | 44.6 | 41.6 | 41.6 | 40.03% | 0.00% | 3.43 | 696 | 32.4 | 0.22 | 0.00% |
| page | 10% | 16 | 8 | 9.39 | 8.56 | 8.56 | 27.02% | 0.00% | 0.787 | 7.04 | 12.5 | 0.76 | 0.20% |
| page | 10% | 32 | 16 | 17.9 | 17.2 | 17.2 | 26.12% | 0.00% | 1.48 | 14.4 | 13.3 | 0.74 | 0.00% |
| page | 10% | 64 | 32 | 34.7 | 34.5 | 34.5 | 25.52% | 0.00% | 2.86 | 29.0 | 13.4 | 0.72 | 0.00% |
| page | 10% | 128 | 64 | 68.6 | 69.1 | 69.1 | 25.33% | 0.00% | 5.63 | 58.0 | 13.5 | 0.74 | 0.00% |
| page | 10% | 64 | 16 | 22.3 | 21.7 | 21.7 | 6.90% | 0.00% | 1.84 | 18.4 | 13.4 | 0.75 | 0.00% |
| page | 10% | 64 | 48 | 41.7 | 41.6 | 41.6 | 39.94% | 0.00% | 3.43 | 35.0 | 13.6 | 0.75 | 0.00% |
| page | 30% | 16 | 8 | 9.39 | 8.56 | 8.56 | 27.02% | 0.00% | 0.787 | 2.27 | 4.12 | 1.33 | 12.40% |
| page | 30% | 32 | 16 | 17.9 | 17.2 | 17.2 | 26.12% | 0.00% | 1.48 | 4.62 | 7.62 | 1.29 | 2.00% |
| page | 30% | 64 | 32 | 34.7 | 34.5 | 34.5 | 25.52% | 0.00% | 2.86 | 9.40 | 12.1 | 1.19 | 0.30% |
| page | 30% | 128 | 64 | 68.6 | 69.1 | 69.1 | 25.33% | 0.00% | 5.63 | 19.4 | 12.4 | 1.19 | 0.00% |
| page | 30% | 64 | 16 | 22.3 | 21.7 | 21.7 | 6.90% | 0.00% | 1.84 | 5.92 | 9.40 | 1.21 | 0.90% |
| page | 30% | 64 | 48 | 41.7 | 41.6 | 41.6 | 39.94% | 0.00% | 3.43 | 11.5 | 12.3 | 1.21 | 0.00% |
| weak work | 1% | 16 | 8 | 10.00 | 8.56 | 8.56 | 26.98% | 0.00% | 0.787 | 144 | 24.8 | 0.22 | 0.00% |
| weak work | 1% | 32 | 16 | 19.1 | 17.2 | 17.2 | 26.09% | 0.00% | 1.48 | 289 | 24.8 | 0.21 | 0.00% |
| weak work | 1% | 64 | 32 | 37.2 | 34.5 | 34.5 | 25.41% | 0.00% | 2.86 | 579 | 25.2 | 0.20 | 0.00% |
| weak work | 1% | 128 | 64 | 73.5 | 69.0 | 69.0 | 25.21% | 0.00% | 5.62 | 1,157 | 24.5 | 0.19 | 0.00% |
| weak work | 1% | 64 | 16 | 23.7 | 21.7 | 21.7 | 6.78% | 0.00% | 1.84 | 364 | 24.7 | 0.21 | 0.00% |
| weak work | 1% | 64 | 48 | 44.6 | 41.6 | 41.6 | 40.03% | 0.00% | 3.43 | 696 | 24.9 | 0.21 | 0.00% |
| weak work | 10% | 16 | 8 | 9.39 | 8.56 | 8.56 | 27.02% | 0.00% | 0.787 | 7.04 | 23.7 | 0.41 | 0.00% |
| weak work | 10% | 32 | 16 | 17.9 | 17.2 | 17.2 | 26.12% | 0.00% | 1.48 | 14.4 | 24.5 | 0.32 | 0.00% |
| weak work | 10% | 64 | 32 | 34.7 | 34.5 | 34.5 | 25.52% | 0.00% | 2.86 | 29.0 | 24.8 | 0.27 | 0.00% |
| weak work | 10% | 128 | 64 | 68.6 | 69.1 | 69.1 | 25.33% | 0.00% | 5.63 | 58.0 | 25.0 | 0.24 | 0.00% |
| weak work | 10% | 64 | 16 | 22.3 | 21.7 | 21.7 | 6.90% | 0.00% | 1.84 | 18.4 | 25.0 | 0.31 | 0.00% |
| weak work | 10% | 64 | 48 | 41.7 | 41.6 | 41.6 | 39.94% | 0.00% | 3.43 | 35.0 | 24.8 | 0.26 | 0.00% |
| weak work | 30% | 16 | 8 | 9.39 | 8.56 | 8.56 | 27.02% | 0.00% | 0.787 | 2.27 | 7.63 | 0.71 | 7.50% |
| weak work | 30% | 32 | 16 | 17.9 | 17.2 | 17.2 | 26.12% | 0.00% | 1.48 | 4.62 | 14.9 | 0.51 | 0.80% |
| weak work | 30% | 64 | 32 | 34.7 | 34.5 | 34.5 | 25.52% | 0.00% | 2.86 | 9.40 | 24.0 | 0.39 | 0.00% |
| weak work | 30% | 128 | 64 | 68.6 | 69.1 | 69.1 | 25.33% | 0.00% | 5.63 | 19.4 | 24.8 | 0.31 | 0.00% |
| weak work | 30% | 64 | 16 | 22.3 | 21.7 | 21.7 | 6.90% | 0.00% | 1.84 | 5.92 | 18.4 | 0.47 | 0.40% |
| weak work | 30% | 64 | 48 | 41.7 | 41.6 | 41.6 | 39.94% | 0.00% | 3.43 | 11.5 | 24.4 | 0.36 | 0.00% |


### INCLUDER_CUT (band-weak-levels)

| Pool | Credit | INCLUDER_CUT | Bitaxe vs fair | Bitaxe bonus share | Carrying ASIC vs fair | Carrying ASIC bonus share | Lone non-carrying ASIC vs fair | Honest weak expired, 50% non-carrying | Bitaxe vs fair, 50% non-carrying | Non-carrying ASIC vs fair, 50% non-carrying |
|---|---|---|---|---|---|---|---|---|---|---|
| 1% | page | 0.00% | 1.33 | 0.20% | 1.33 | 0.20% | 1.33 | 0.00% | 1.33 | 1.33 |
| 1% | page | 1.00% | 1.33 | 0.66% | 1.33 | 0.66% | 1.32 | 0.00% | 1.33 | 1.32 |
| 1% | page | 2.00% | 1.33 | 1.12% | 1.33 | 1.12% | 1.31 | 0.00% | 1.33 | 1.32 |
| 1% | page | 5.00% | 1.32 | 2.51% | 1.32 | 2.51% | 1.29 | 0.00% | 1.33 | 1.31 |
| 1% | weak work | 0.00% | 1.00 | 0.20% | 1.00 | 0.20% | 1.00 | 0.00% | 1.00 | 1.00 |
| 1% | weak work | 1.00% | 1.00 | 1.11% | 1.00 | 1.11% | 0.99 | 0.00% | 1.00 | 1.00 |
| 1% | weak work | 2.00% | 1.00 | 2.02% | 1.00 | 2.02% | 0.98 | 0.00% | 1.01 | 0.99 |
| 1% | weak work | 5.00% | 1.00 | 4.76% | 1.00 | 4.76% | 0.95 | 0.00% | 1.01 | 0.98 |
| 10% | page | 0.00% | 1.00 | 0.11% | 1.00 | 0.11% | 1.00 | 0.00% | 1.00 | 1.00 |
| 10% | page | 1.00% | 1.00 | 0.60% | 1.00 | 0.60% | 1.00 | 0.00% | 1.00 | 1.00 |
| 10% | page | 2.00% | 1.00 | 1.09% | 1.00 | 1.09% | 0.99 | 0.00% | 1.00 | 1.00 |
| 10% | page | 5.00% | 1.00 | 2.57% | 1.00 | 2.57% | 0.98 | 0.00% | 1.01 | 0.99 |
| 10% | weak work | 0.00% | 1.00 | 0.01% | 1.00 | 0.01% | 1.00 | 0.00% | 1.00 | 1.00 |
| 10% | weak work | 1.00% | 1.00 | 0.98% | 1.00 | 0.98% | 0.99 | 0.00% | 1.00 | 1.00 |
| 10% | weak work | 2.00% | 1.00 | 1.95% | 1.00 | 1.95% | 0.98 | 0.00% | 1.01 | 0.99 |
| 10% | weak work | 5.00% | 1.00 | 4.87% | 1.00 | 4.87% | 0.95 | 0.00% | 1.02 | 0.98 |


## Default payout level (band-weak-levels, 50% industrial)

| Credit | Pool | Level | Bitaxe pay/mo | Bitaxe median sat | Bitaxe swing | Bitaxe empty months | Bitaxe fee loss | ASIC pay/mo | ASIC median sat | ASIC fee loss |
|---|---|---|---|---|---|---|---|---|---|---|
| page | 1% | none | 405 | 721 | 0.17 | 0.00% | 10.44% | 432 | 109,345 | 0.13% |
| page | 1% | 3 (4,368 sat) | 90.1 | 4,368 | 0.18 | 0.00% | 2.34% | 432 | 109,345 | 0.13% |
| page | 1% | 4 (8,736 sat) | 60.2 | 9,108 | 0.19 | 0.00% | 1.56% | 432 | 109,345 | 0.13% |
| page | 1% | 5 (17,472 sat) | 32.6 | 17,479 | 0.22 | 0.00% | 0.84% | 432 | 109,345 | 0.13% |
| page | 1% | 6 (34,944 sat) | 16.5 | 34,967 | 0.28 | 0.00% | 0.42% | 432 | 109,345 | 0.13% |
| page | 1% | 7 (69,888 sat) | 8.08 | 70,066 | 0.36 | 0.00% | 0.21% | 431 | 109,518 | 0.13% |
| page | 10% | none | 45.3 | 4,802 | 0.73 | 0.00% | 1.59% | 2,518 | 4,802 | 1.03% |
| page | 10% | 3 (4,368 sat) | 45.0 | 4,802 | 0.72 | 0.00% | 1.58% | 2,516 | 4,802 | 1.04% |
| page | 10% | 4 (8,736 sat) | 25.3 | 8,736 | 0.72 | 0.00% | 0.89% | 1,828 | 9,604 | 0.75% |
| page | 10% | 5 (17,472 sat) | 13.4 | 17,487 | 0.72 | 0.00% | 0.47% | 1,081 | 17,487 | 0.45% |
| page | 10% | 6 (34,944 sat) | 7.20 | 35,089 | 0.75 | 0.10% | 0.25% | 597 | 35,089 | 0.25% |
| page | 10% | 7 (69,888 sat) | 4.25 | 70,394 | 0.77 | 3.30% | 0.15% | 352 | 70,394 | 0.14% |
| page | 30% | none | 15.1 | 14,401 | 1.27 | 0.00% | 0.54% | 1,198 | 14,401 | 0.49% |
| page | 30% | 3 (4,368 sat) | 14.7 | 14,401 | 1.19 | 0.10% | 0.53% | 1,200 | 14,401 | 0.49% |
| page | 30% | 4 (8,736 sat) | 14.7 | 14,401 | 1.19 | 0.10% | 0.53% | 1,200 | 14,401 | 0.49% |
| page | 30% | 5 (17,472 sat) | 12.1 | 17,506 | 1.19 | 0.30% | 0.44% | 1,003 | 17,506 | 0.41% |
| page | 30% | 6 (34,944 sat) | 6.27 | 34,980 | 1.20 | 1.00% | 0.23% | 539 | 34,980 | 0.22% |
| page | 30% | 7 (69,888 sat) | 3.34 | 70,099 | 1.23 | 7.20% | 0.12% | 287 | 70,099 | 0.12% |
| weak work | 1% | none | 425 | 961 | 0.07 | 0.00% | 14.48% | 432 | 83,684 | 0.18% |
| weak work | 1% | 3 (4,368 sat) | 99.3 | 4,369 | 0.11 | 0.00% | 3.39% | 432 | 83,681 | 0.18% |
| weak work | 1% | 4 (8,736 sat) | 49.4 | 8,740 | 0.15 | 0.00% | 1.69% | 432 | 83,681 | 0.18% |
| weak work | 1% | 5 (17,472 sat) | 25.2 | 17,481 | 0.20 | 0.00% | 0.85% | 432 | 83,681 | 0.18% |
| weak work | 1% | 6 (34,944 sat) | 12.3 | 35,003 | 0.27 | 0.00% | 0.42% | 432 | 83,681 | 0.18% |
| weak work | 1% | 7 (69,888 sat) | 6.22 | 70,007 | 0.40 | 0.10% | 0.21% | 432 | 83,681 | 0.18% |
| weak work | 10% | none | 88.8 | 4,801 | 0.22 | 0.00% | 3.02% | 3,548 | 9,604 | 1.45% |
| weak work | 10% | 3 (4,368 sat) | 88.9 | 4,802 | 0.20 | 0.00% | 3.02% | 3,545 | 9,604 | 1.45% |
| weak work | 10% | 4 (8,736 sat) | 49.4 | 8,738 | 0.22 | 0.00% | 1.69% | 2,956 | 9,604 | 1.21% |
| weak work | 10% | 5 (17,472 sat) | 24.8 | 17,485 | 0.27 | 0.00% | 0.85% | 1,967 | 17,491 | 0.81% |
| weak work | 10% | 6 (34,944 sat) | 12.5 | 34,981 | 0.34 | 0.00% | 0.42% | 1,031 | 34,981 | 0.42% |
| weak work | 10% | 7 (69,888 sat) | 6.25 | 70,038 | 0.44 | 0.70% | 0.21% | 514 | 70,039 | 0.21% |
| weak work | 30% | none | 29.4 | 14,401 | 0.37 | 0.00% | 1.02% | 2,255 | 14,401 | 0.93% |
| weak work | 30% | 3 (4,368 sat) | 28.9 | 14,401 | 0.38 | 0.00% | 1.02% | 2,258 | 14,401 | 0.93% |
| weak work | 30% | 4 (8,736 sat) | 28.9 | 14,401 | 0.38 | 0.00% | 1.02% | 2,258 | 14,401 | 0.93% |
| weak work | 30% | 5 (17,472 sat) | 24.0 | 17,487 | 0.39 | 0.00% | 0.84% | 1,906 | 17,487 | 0.78% |
| weak work | 30% | 6 (34,944 sat) | 12.0 | 34,980 | 0.44 | 0.10% | 0.42% | 1,027 | 34,980 | 0.42% |
| weak work | 30% | 7 (69,888 sat) | 6.01 | 69,918 | 0.55 | 0.50% | 0.21% | 516 | 69,918 | 0.21% |


### Share period (pool 10%, 50% industrial)

| Design | Period | Stale | Window | Shares/s | Uncles/s | Weak/s | KB/s | Bitaxe pay/mo | Bitaxe median sat | Bitaxe swing | Bitaxe empty months | Bitaxe fee loss | Bitaxe vs fair |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| one-chain | 10 s | 2.10% | 0.5 h | 0.102 | 2.1e-3 | 0 | 0.102 | 2.37 | 167,758 | 1.28 | 46.30% | 0.09% | 0.91 |
| one-chain | 5 s | 3.80% | 0.5 h | 0.208 | 7.9e-3 | 0 | 0.208 | 5.00 | 82,798 | 0.91 | 20.80% | 0.18% | 0.95 |
| one-chain | 2 s | 8.70% | 0.5 h | 0.548 | 0.048 | 0 | 0.548 | 13.7 | 31,659 | 0.52 | 1.40% | 0.47% | 0.99 |
| one-chain | 1 s | 15.90% | 0.5 h | 1.19 | 0.189 | 0 | 1.19 | 29.7 | 14,718 | 0.36 | 0.00% | 1.02% | 0.99 |
| band-weak-levels | 10 s | 2.10% | 0.3 h | 0.102 | 2.1e-3 | 3.45 | 2.86 | 13.4 | 17,487 | 0.72 | 0.00% | 0.47% | 0.97 |
| band-weak-levels | 5 s | 3.80% | 0.3 h | 0.208 | 7.9e-3 | 6.89 | 5.72 | 14.4 | 17,492 | 0.54 | 0.00% | 0.51% | 0.96 |
| band-weak-levels | 2 s | 8.70% | 0.3 h | 0.548 | 0.048 | 17.2 | 14.3 | 22.8 | 17,501 | 0.31 | 0.00% | 0.70% | 1.11 |
| band-weak-levels | 1 s | 15.90% | 0.3 h | 1.19 | 0.189 | 34.5 | 28.8 | 27.9 | 17,478 | 0.24 | 0.00% | 0.85% | 1.13 |
| band-weak-levels, weak work | 10 s | 2.10% | 0.5 h | 0.102 | 2.1e-3 | 3.45 | 2.86 | 24.8 | 17,485 | 0.27 | 0.00% | 0.85% | 1.00 |
| band-weak-levels, weak work | 5 s | 3.80% | 0.5 h | 0.208 | 7.9e-3 | 6.89 | 5.72 | 24.7 | 17,480 | 0.24 | 0.00% | 0.85% | 1.00 |
| band-weak-levels, weak work | 2 s | 8.70% | 0.5 h | 0.548 | 0.048 | 17.2 | 14.3 | 24.9 | 17,482 | 0.21 | 0.00% | 0.85% | 1.00 |
| band-weak-levels, weak work | 1 s | 15.90% | 0.5 h | 1.19 | 0.189 | 34.5 | 28.8 | 24.7 | 17,476 | 0.20 | 0.00% | 0.85% | 1.00 |


### Attack (a): the industrial node mines at the minimum (90% industrial)

| Design | Credit | Pool | Industrial node | Minimum / uniform | R | Bitaxe pay/mo | Bitaxe median sat | Bitaxe swing | Bitaxe empty months | Bitaxe fee loss | Bitaxe vs fair | ASIC pay/mo | ASIC median sat | ASIC swing | ASIC empty months | ASIC fee loss | ASIC vs fair |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| one-chain | page | 1% | node default | 1.00 | - | 24.9 | 16,853 | 0.39 | 0.00% | 0.86% | 0.99 | 430 | 84,314 | 0.06 | 0.00% | 0.18% | 1.00 |
| one-chain | page | 10% | node default | 1.00 | - | 2.37 | 167,758 | 1.28 | 46.30% | 0.09% | 0.91 | 210 | 167,758 | 0.13 | 0.00% | 0.09% | 1.00 |
| one-chain | page | 30% | node default | 1.00 | - | 0.803 | 497,785 | 2.28 | 78.10% | 0.03% | 0.92 | 71.4 | 497,785 | 0.23 | 0.00% | 0.03% | 0.99 |
| band | page | 1% | node default | 0.12 | - | 169 | 2,066 | 0.14 | 0.00% | 5.66% | 1.02 | 433 | 84,044 | 0.05 | 0.00% | 0.18% | 1.01 |
| band | page | 1% | at the minimum | 1.00 | - | 24.9 | 16,853 | 0.39 | 0.00% | 0.86% | 0.99 | 430 | 84,314 | 0.06 | 0.00% | 0.18% | 1.00 |
| band | page | 10% | node default | 1.00 | - | 2.37 | 167,758 | 1.28 | 46.30% | 0.09% | 0.91 | 210 | 167,758 | 0.13 | 0.00% | 0.09% | 1.00 |
| band | page | 10% | at the minimum | 1.00 | - | 2.37 | 167,758 | 1.28 | 46.30% | 0.09% | 0.91 | 210 | 167,758 | 0.13 | 0.00% | 0.09% | 1.00 |
| band | page | 30% | node default | 1.00 | - | 0.803 | 497,785 | 2.28 | 78.10% | 0.03% | 0.92 | 71.4 | 497,785 | 0.23 | 0.00% | 0.03% | 0.99 |
| band | page | 30% | at the minimum | 1.00 | - | 0.803 | 497,785 | 2.28 | 78.10% | 0.03% | 0.92 | 71.4 | 497,785 | 0.23 | 0.00% | 0.03% | 0.99 |
| band-weak-levels | page | 1% | node default | 0.12 | 41.9 | 44.7 | 17,500 | 0.16 | 0.00% | 0.85% | 1.81 | 432 | 77,218 | 0.05 | 0.00% | 0.19% | 0.91 |
| band-weak-levels | page | 1% | at the minimum | 1.00 | 34.7 | 24.1 | 17,555 | 0.27 | 0.00% | 0.83% | 0.99 | 432 | 79,215 | 0.05 | 0.00% | 0.18% | 1.00 |
| band-weak-levels | page | 10% | node default | 1.00 | 34.7 | 13.3 | 17,601 | 0.72 | 0.00% | 0.47% | 0.97 | 1,075 | 17,601 | 0.08 | 0.00% | 0.44% | 0.99 |
| band-weak-levels | page | 10% | at the minimum | 1.00 | 34.7 | 13.3 | 17,601 | 0.72 | 0.00% | 0.47% | 0.97 | 1,075 | 17,601 | 0.08 | 0.00% | 0.44% | 0.99 |
| band-weak-levels | page | 30% | node default | 1.00 | 34.7 | 12.1 | 17,508 | 1.19 | 0.20% | 0.44% | 0.94 | 1,003 | 17,508 | 0.13 | 0.00% | 0.41% | 1.00 |
| band-weak-levels | page | 30% | at the minimum | 1.00 | 34.7 | 12.1 | 17,508 | 1.19 | 0.20% | 0.44% | 0.94 | 1,003 | 17,508 | 0.13 | 0.00% | 0.41% | 1.00 |
| band-weak-levels | weak work | 1% | node default | 0.12 | 41.9 | 25.3 | 17,481 | 0.20 | 0.00% | 0.85% | 1.02 | 432 | 84,205 | 0.05 | 0.00% | 0.18% | 1.00 |
| band-weak-levels | weak work | 1% | at the minimum | 1.00 | 34.7 | 25.1 | 17,484 | 0.21 | 0.00% | 0.85% | 1.01 | 432 | 83,507 | 0.05 | 0.00% | 0.18% | 1.00 |
| band-weak-levels | weak work | 10% | node default | 1.00 | 34.7 | 24.9 | 17,534 | 0.26 | 0.00% | 0.84% | 1.01 | 1,964 | 17,567 | 0.03 | 0.00% | 0.81% | 1.00 |
| band-weak-levels | weak work | 10% | at the minimum | 1.00 | 34.7 | 24.9 | 17,534 | 0.26 | 0.00% | 0.84% | 1.01 | 1,964 | 17,567 | 0.03 | 0.00% | 0.81% | 1.00 |
| band-weak-levels | weak work | 30% | node default | 1.00 | 34.7 | 23.9 | 17,474 | 0.39 | 0.00% | 0.84% | 0.97 | 1,908 | 17,474 | 0.04 | 0.00% | 0.78% | 1.00 |
| band-weak-levels | weak work | 30% | at the minimum | 1.00 | 34.7 | 23.9 | 17,474 | 0.39 | 0.00% | 0.84% | 0.97 | 1,908 | 17,474 | 0.04 | 0.00% | 0.78% | 1.00 |


### Attack (b): nodes that never carry others' weak shares (band-weak-levels)

| Mix | Pool | Credit | Non-carrying | R | Honest weak expired | Bitaxe weak/mo | Bitaxe pay/mo | Bitaxe swing | Bitaxe empty months | Bitaxe vs fair | Carrying ASIC vs fair | Non-carrying ASIC vs fair |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 50% industrial | 1% | page | 0.00% | 37.2 | 0.00% | 577 | 32.2 | 0.22 | 0.00% | 1.33 | 1.33 | 1.32 |
| 50% industrial | 1% | page | 50.00% | 37.6 | 0.00% | 583 | 32.7 | 0.22 | 0.00% | 1.33 | 1.33 | 1.32 |
| 50% industrial | 1% | page | 90.00% | 37.2 | 1.17% | 569 | 32.2 | 0.22 | 0.00% | 1.32 | 1.32 | 1.33 |
| 50% industrial | 1% | weak work | 0.00% | 37.2 | 0.00% | 577 | 25.0 | 0.21 | 0.00% | 1.00 | 1.00 | 0.99 |
| 50% industrial | 1% | weak work | 50.00% | 37.6 | 0.00% | 583 | 25.0 | 0.20 | 0.00% | 1.00 | 1.00 | 1.00 |
| 50% industrial | 1% | weak work | 90.00% | 37.2 | 1.17% | 569 | 24.6 | 0.20 | 0.00% | 1.00 | 1.00 | 1.00 |
| 50% industrial | 10% | page | 0.00% | 34.7 | 0.00% | 29.0 | 13.4 | 0.74 | 0.00% | 1.00 | 1.00 | 1.00 |
| 50% industrial | 10% | page | 50.00% | 34.9 | 0.00% | 29.1 | 13.4 | 0.71 | 0.00% | 1.00 | 1.00 | 1.00 |
| 50% industrial | 10% | page | 90.00% | 34.8 | 1.36% | 28.6 | 13.3 | 0.71 | 0.00% | 1.00 | 1.00 | 1.00 |
| 50% industrial | 10% | weak work | 0.00% | 34.7 | 0.00% | 29.0 | 24.8 | 0.27 | 0.00% | 1.00 | 1.00 | 0.99 |
| 50% industrial | 10% | weak work | 50.00% | 34.9 | 0.00% | 29.1 | 24.7 | 0.27 | 0.00% | 1.00 | 1.00 | 1.00 |
| 50% industrial | 10% | weak work | 90.00% | 34.8 | 1.36% | 28.6 | 24.7 | 0.28 | 0.00% | 0.99 | 0.99 | 1.00 |
| 90% industrial | 1% | page | 0.00% | 42.0 | 0.00% | 2,905 | 45.2 | 0.16 | 0.00% | 1.81 | 0.92 | 0.91 |
| 90% industrial | 1% | page | 50.00% | 42.1 | 0.00% | 2,915 | 45.2 | 0.16 | 0.00% | 1.82 | 0.92 | 0.92 |
| 90% industrial | 1% | page | 90.00% | 42.0 | 0.92% | 2,881 | 44.9 | 0.16 | 0.00% | 1.81 | 0.92 | 0.92 |
| 90% industrial | 1% | weak work | 0.00% | 42.0 | 0.00% | 2,905 | 25.0 | 0.20 | 0.00% | 1.01 | 1.00 | 1.00 |
| 90% industrial | 1% | weak work | 50.00% | 42.1 | 0.00% | 2,915 | 25.3 | 0.20 | 0.00% | 1.02 | 1.01 | 1.00 |
| 90% industrial | 1% | weak work | 90.00% | 42.0 | 0.92% | 2,881 | 25.1 | 0.20 | 0.00% | 1.01 | 1.01 | 1.00 |
| 90% industrial | 10% | page | 0.00% | 34.7 | 0.00% | 29.0 | 13.3 | 0.74 | 0.00% | 1.00 | 1.00 | 1.00 |
| 90% industrial | 10% | page | 50.00% | 34.9 | 0.00% | 29.1 | 13.3 | 0.72 | 0.00% | 1.00 | 1.00 | 1.00 |
| 90% industrial | 10% | page | 90.00% | 34.8 | 1.36% | 28.6 | 13.2 | 0.71 | 0.00% | 1.00 | 1.00 | 1.00 |
| 90% industrial | 10% | weak work | 0.00% | 34.7 | 0.00% | 29.0 | 24.5 | 0.28 | 0.00% | 1.00 | 1.00 | 0.99 |
| 90% industrial | 10% | weak work | 50.00% | 34.9 | 0.00% | 29.1 | 24.6 | 0.27 | 0.00% | 1.00 | 1.00 | 1.00 |
| 90% industrial | 10% | weak work | 90.00% | 34.8 | 1.36% | 28.6 | 24.5 | 0.27 | 0.00% | 0.99 | 0.99 | 1.00 |


### Attack (c): weak share spam (band-weak-levels)

| Mix | Pool before spam | Spam | Pool with spam | Credit | R | Bitaxe weak/mo | Bitaxe pay/mo | Bitaxe median sat | Bitaxe swing | Bitaxe empty months | Bitaxe expected vs fair | Bitaxe simulated vs fair |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 50% industrial | 1% | 0x | 1% | page | 37.2 | 579 | 32.6 | 17,479 | 0.22 | 0.00% | 1.33 | 1.32 |
| 50% industrial | 1% | 1x | 1.50% | page | 38.6 | 291 | 27.6 | 17,504 | 0.26 | 0.00% | 1.20 | 1.18 |
| 50% industrial | 1% | 10x | 5.99% | page | 36.0 | 50.6 | 14.2 | 17,679 | 0.57 | 0.00% | 1.02 | 1.00 |
| 50% industrial | 1% | 0x | 1% | weak work | 37.2 | 579 | 25.2 | 17,481 | 0.20 | 0.00% | 1.00 | 1.02 |
| 50% industrial | 1% | 1x | 1.50% | weak work | 38.6 | 291 | 24.7 | 17,485 | 0.22 | 0.00% | 1.00 | 1.00 |
| 50% industrial | 1% | 10x | 5.99% | weak work | 36.0 | 50.6 | 24.4 | 17,523 | 0.23 | 0.00% | 1.00 | 0.99 |
| 90% industrial | 1% | 0x | 1% | page | 41.9 | 2,908 | 44.7 | 17,500 | 0.16 | 0.00% | 1.81 | 1.81 |
| 90% industrial | 1% | 1x | 1.10% | page | 40.9 | 1,449 | 41.5 | 17,492 | 0.18 | 0.00% | 1.68 | 1.67 |
| 90% industrial | 1% | 10x | 2.00% | page | 45.4 | 265 | 26.2 | 17,644 | 0.29 | 0.00% | 1.28 | 1.26 |
| 90% industrial | 1% | 0x | 1% | weak work | 41.9 | 2,908 | 25.3 | 17,481 | 0.20 | 0.00% | 1.01 | 1.02 |
| 90% industrial | 1% | 1x | 1.10% | weak work | 40.9 | 1,449 | 25.0 | 17,515 | 0.21 | 0.00% | 1.01 | 1.01 |
| 90% industrial | 1% | 10x | 2.00% | weak work | 45.4 | 265 | 25.2 | 17,535 | 0.21 | 0.00% | 1.00 | 1.02 |
| 90% industrial | 10% | 0x | 10% | page | 34.7 | 29.0 | 13.3 | 17,601 | 0.72 | 0.00% | 1.00 | 0.97 |
| 90% industrial | 10% | 1x | 11.00% | page | 34.7 | 26.3 | 13.2 | 17,642 | 0.75 | 0.00% | 1.00 | 1.00 |
| 90% industrial | 10% | 10x | 20.00% | page | 34.7 | 14.2 | 12.3 | 17,668 | 1.02 | 0.20% | 1.00 | 0.98 |
| 90% industrial | 10% | 0x | 10% | weak work | 34.7 | 29.0 | 24.9 | 17,534 | 0.26 | 0.00% | 1.00 | 1.01 |
| 90% industrial | 10% | 1x | 11.00% | weak work | 34.7 | 26.3 | 25.0 | 17,519 | 0.27 | 0.00% | 1.00 | 1.01 |
| 90% industrial | 10% | 10x | 20.00% | weak work | 34.7 | 14.2 | 24.0 | 17,619 | 0.34 | 0.00% | 1.00 | 0.97 |


### Study: a refusing coalition with share races (band-weak-levels, weak work, 50% industrial)

| Pool | Fork choice | INCLUDER_CUT | Refusing | Honest weak lost | Honest stale | Refusing stale | Tips replaced per share | Bitaxe vs fair | Carrying ASIC vs fair | Refusing ASIC vs fair | Coalition gains | Refusing ASIC gains |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1% | share work | 0.00% | 0.00% | 0.00% | 2.12% | - | 0 | 1.00 | 1.00 | 1.00 | - | -0.00% |
| 1% | share work | 0.00% | 50.00% | 0.00% | 2.12% | 2.07% | 0 | 1.00 | 1.00 | 1.00 | -0.00% | +0.00% |
| 1% | share work | 0.00% | 90.00% | 1.20% | 2.18% | 2.08% | 0 | 0.99 | 0.99 | 1.00 | +0.06% | +0.06% |
| 1% | share work | 1.00% | 0.00% | 0.00% | 2.12% | - | 0 | 1.00 | 1.00 | 0.99 | - | -0.90% |
| 1% | share work | 1.00% | 50.00% | 0.00% | 2.12% | 2.07% | 0 | 1.00 | 1.00 | 1.00 | -0.18% | -0.33% |
| 1% | share work | 1.00% | 90.00% | 1.20% | 2.18% | 2.08% | 0 | 0.99 | 0.99 | 1.00 | +0.03% | -0.00% |
| 1% | share work | 2.00% | 0.00% | 0.00% | 2.12% | - | 0 | 1.00 | 1.00 | 0.98 | - | -1.81% |
| 1% | share work | 2.00% | 50.00% | 0.00% | 2.12% | 2.07% | 0 | 1.01 | 1.01 | 0.99 | -0.35% | -0.66% |
| 1% | share work | 2.00% | 90.00% | 1.20% | 2.18% | 2.08% | 0 | 1.00 | 1.00 | 1.00 | -0.00% | -0.07% |
| 1% | share work | 5.00% | 0.00% | 0.00% | 2.12% | - | 0 | 0.99 | 1.00 | 0.95 | - | -4.53% |
| 1% | share work | 5.00% | 50.00% | 0.00% | 2.12% | 2.07% | 0 | 1.01 | 1.01 | 0.98 | -0.89% | -1.66% |
| 1% | share work | 5.00% | 90.00% | 1.20% | 2.18% | 2.08% | 0 | 1.01 | 1.01 | 0.99 | -0.10% | -0.26% |
| 1% | weak work | 0.00% | 0.00% | 0.00% | 2.08% | - | 0 | 1.00 | 1.00 | 1.00 | - | -0.01% |
| 1% | weak work | 0.00% | 50.00% | 0.00% | 1.25% | 2.93% | 0 | 1.00 | 1.00 | 1.00 | -0.00% | -0.00% |
| 1% | weak work | 0.00% | 90.00% | 1.26% | 0.85% | 2.22% | 0 | 0.99 | 0.99 | 1.00 | +0.06% | +0.06% |
| 1% | weak work | 1.00% | 0.00% | 0.00% | 2.08% | - | 0 | 1.00 | 1.00 | 0.99 | - | -0.92% |
| 1% | weak work | 1.00% | 50.00% | 0.00% | 1.25% | 2.93% | 0 | 1.00 | 1.00 | 1.00 | -0.18% | -0.34% |
| 1% | weak work | 1.00% | 90.00% | 1.26% | 0.85% | 2.22% | 0 | 0.99 | 0.99 | 1.00 | +0.03% | -0.01% |
| 1% | weak work | 2.00% | 0.00% | 0.00% | 2.08% | - | 0 | 1.00 | 1.00 | 0.98 | - | -1.83% |
| 1% | weak work | 2.00% | 50.00% | 0.00% | 1.25% | 2.93% | 0 | 1.01 | 1.01 | 0.99 | -0.36% | -0.69% |
| 1% | weak work | 2.00% | 90.00% | 1.26% | 0.85% | 2.22% | 0 | 1.00 | 1.00 | 1.00 | -0.00% | -0.07% |
| 1% | weak work | 5.00% | 0.00% | 0.00% | 2.08% | - | 0 | 1.00 | 1.00 | 0.95 | - | -4.56% |
| 1% | weak work | 5.00% | 50.00% | 0.00% | 1.25% | 2.93% | 0 | 1.01 | 1.01 | 0.98 | -0.90% | -1.71% |
| 1% | weak work | 5.00% | 90.00% | 1.26% | 0.85% | 2.22% | 0 | 1.01 | 1.01 | 0.99 | -0.10% | -0.27% |
| 1% | weak work, siblings | 0.00% | 0.00% | 0.00% | 2.03% | - | 4.8e-3 | 1.00 | 1.00 | 0.99 | - | -0.74% |
| 1% | weak work, siblings | 0.00% | 50.00% | 0.00% | 0.77% | 45.29% | 0.277 | 1.06 | 1.06 | 0.83 | -5.59% | -16.78% |
| 1% | weak work, siblings | 0.00% | 90.00% | 0.99% | 0.58% | 9.56% | 0.073 | 1.02 | 1.02 | 0.96 | -0.30% | -3.77% |
| 1% | weak work, siblings | 1.00% | 0.00% | 0.00% | 2.03% | - | 4.8e-3 | 1.00 | 1.00 | 0.98 | - | -1.65% |
| 1% | weak work, siblings | 1.00% | 50.00% | 0.00% | 0.77% | 45.29% | 0.277 | 1.06 | 1.06 | 0.83 | -5.81% | -17.21% |
| 1% | weak work, siblings | 1.00% | 90.00% | 0.99% | 0.58% | 9.56% | 0.073 | 1.03 | 1.03 | 0.96 | -0.33% | -3.82% |
| 1% | weak work, siblings | 2.00% | 0.00% | 0.00% | 2.03% | - | 4.8e-3 | 1.00 | 1.00 | 0.97 | - | -2.56% |
| 1% | weak work, siblings | 2.00% | 50.00% | 0.00% | 0.77% | 45.29% | 0.277 | 1.06 | 1.06 | 0.82 | -6.04% | -17.63% |
| 1% | weak work, siblings | 2.00% | 90.00% | 0.99% | 0.58% | 9.56% | 0.073 | 1.03 | 1.03 | 0.96 | -0.36% | -3.88% |
| 1% | weak work, siblings | 5.00% | 0.00% | 0.00% | 2.03% | - | 4.8e-3 | 1.00 | 1.00 | 0.94 | - | -5.29% |
| 1% | weak work, siblings | 5.00% | 50.00% | 0.00% | 0.77% | 45.29% | 0.277 | 1.07 | 1.07 | 0.81 | -6.70% | -18.92% |
| 1% | weak work, siblings | 5.00% | 90.00% | 0.99% | 0.58% | 9.56% | 0.073 | 1.04 | 1.04 | 0.96 | -0.46% | -4.04% |
| 10% | share work | 0.00% | 0.00% | 0.00% | 2.07% | - | 0 | 1.00 | 1.00 | 1.00 | - | -0.00% |
| 10% | share work | 0.00% | 50.00% | 0.00% | 2.11% | 2.10% | 0 | 1.00 | 1.00 | 1.00 | +0.00% | +0.00% |
| 10% | share work | 0.00% | 90.00% | 1.54% | 2.03% | 2.13% | 0 | 0.99 | 0.99 | 1.00 | +0.15% | +0.14% |
| 10% | share work | 1.00% | 0.00% | 0.00% | 2.07% | - | 0 | 1.00 | 1.00 | 0.99 | - | -0.97% |
| 10% | share work | 1.00% | 50.00% | 0.00% | 2.11% | 2.10% | 0 | 1.00 | 1.00 | 1.00 | -0.35% | -0.34% |
| 10% | share work | 1.00% | 90.00% | 1.54% | 2.03% | 2.13% | 0 | 0.99 | 0.99 | 1.00 | +0.09% | +0.07% |
| 10% | share work | 2.00% | 0.00% | 0.00% | 2.07% | - | 0 | 1.00 | 1.00 | 0.98 | - | -1.94% |
| 10% | share work | 2.00% | 50.00% | 0.00% | 2.11% | 2.10% | 0 | 1.01 | 1.01 | 0.99 | -0.70% | -0.69% |
| 10% | share work | 2.00% | 90.00% | 1.54% | 2.03% | 2.13% | 0 | 1.00 | 1.00 | 1.00 | +0.04% | -0.01% |
| 10% | share work | 5.00% | 0.00% | 0.00% | 2.07% | - | 0 | 1.00 | 1.00 | 0.95 | - | -4.85% |
| 10% | share work | 5.00% | 50.00% | 0.00% | 2.11% | 2.10% | 0 | 1.02 | 1.02 | 0.98 | -1.74% | -1.72% |
| 10% | share work | 5.00% | 90.00% | 1.54% | 2.03% | 2.13% | 0 | 1.01 | 1.01 | 1.00 | -0.14% | -0.23% |
| 10% | weak work | 0.00% | 0.00% | 0.00% | 2.07% | - | 0 | 1.00 | 1.00 | 1.00 | - | -0.01% |
| 10% | weak work | 0.00% | 50.00% | 0.00% | 1.15% | 3.02% | 0 | 1.00 | 1.00 | 1.00 | -0.00% | -0.01% |
| 10% | weak work | 0.00% | 90.00% | 1.14% | 0.65% | 2.27% | 0 | 0.99 | 0.99 | 1.00 | +0.11% | +0.11% |
| 10% | weak work | 1.00% | 0.00% | 0.00% | 2.07% | - | 0 | 1.00 | 1.00 | 0.99 | - | -0.99% |
| 10% | weak work | 1.00% | 50.00% | 0.00% | 1.15% | 3.02% | 0 | 1.00 | 1.00 | 1.00 | -0.36% | -0.38% |
| 10% | weak work | 1.00% | 90.00% | 1.14% | 0.65% | 2.27% | 0 | 1.00 | 0.99 | 1.00 | +0.05% | +0.05% |
| 10% | weak work | 2.00% | 0.00% | 0.00% | 2.07% | - | 0 | 1.00 | 1.00 | 0.98 | - | -1.97% |
| 10% | weak work | 2.00% | 50.00% | 0.00% | 1.15% | 3.02% | 0 | 1.01 | 1.01 | 0.99 | -0.72% | -0.75% |
| 10% | weak work | 2.00% | 90.00% | 1.14% | 0.65% | 2.27% | 0 | 1.00 | 1.00 | 1.00 | -0.00% | -0.01% |
| 10% | weak work | 5.00% | 0.00% | 0.00% | 2.07% | - | 0 | 1.00 | 1.00 | 0.95 | - | -4.92% |
| 10% | weak work | 5.00% | 50.00% | 0.00% | 1.15% | 3.02% | 0 | 1.02 | 1.02 | 0.98 | -1.78% | -1.87% |
| 10% | weak work | 5.00% | 90.00% | 1.14% | 0.65% | 2.27% | 0 | 1.02 | 1.01 | 1.00 | -0.17% | -0.20% |
| 10% | weak work, siblings | 0.00% | 0.00% | 0.00% | 2.07% | - | 2.4e-4 | 1.00 | 1.00 | 1.00 | - | -0.33% |
| 10% | weak work, siblings | 0.00% | 50.00% | 0.00% | 0.59% | 48.72% | 0.307 | 1.13 | 1.13 | 0.87 | -12.84% | -12.83% |
| 10% | weak work, siblings | 0.00% | 90.00% | 1.23% | 0.38% | 9.73% | 0.075 | 1.05 | 1.05 | 0.99 | -0.60% | -0.61% |
| 10% | weak work, siblings | 1.00% | 0.00% | 0.00% | 2.07% | - | 2.4e-4 | 1.00 | 1.00 | 0.99 | - | -1.29% |
| 10% | weak work, siblings | 1.00% | 50.00% | 0.00% | 0.59% | 48.72% | 0.307 | 1.13 | 1.13 | 0.87 | -13.32% | -13.28% |
| 10% | weak work, siblings | 1.00% | 90.00% | 1.23% | 0.38% | 9.73% | 0.075 | 1.06 | 1.06 | 0.99 | -0.66% | -0.67% |
| 10% | weak work, siblings | 2.00% | 0.00% | 0.00% | 2.07% | - | 2.4e-4 | 1.00 | 1.00 | 0.98 | - | -2.24% |
| 10% | weak work, siblings | 2.00% | 50.00% | 0.00% | 0.59% | 48.72% | 0.307 | 1.14 | 1.14 | 0.86 | -13.79% | -13.74% |
| 10% | weak work, siblings | 2.00% | 90.00% | 1.23% | 0.38% | 9.73% | 0.075 | 1.07 | 1.07 | 0.99 | -0.72% | -0.74% |
| 10% | weak work, siblings | 5.00% | 0.00% | 0.00% | 2.07% | - | 2.4e-4 | 1.00 | 1.00 | 0.95 | - | -5.09% |
| 10% | weak work, siblings | 5.00% | 50.00% | 0.00% | 0.59% | 48.72% | 0.307 | 1.15 | 1.15 | 0.85 | -15.22% | -15.09% |
| 10% | weak work, siblings | 5.00% | 90.00% | 1.23% | 0.38% | 9.73% | 0.075 | 1.08 | 1.08 | 0.99 | -0.90% | -0.93% |


### Study: miner mixes (INCLUDER_CUT 1%)

| Mix | Pool | Design | Miners | Largest miner | Hashrate at the minimum | Minimum / uniform | R | Weak expired | Bitaxe pay/mo | Bitaxe median sat | Bitaxe swing | Bitaxe empty months | Bitaxe fee loss | Bitaxe vs fair | ASIC pay/mo | ASIC median sat | ASIC swing | ASIC empty months | ASIC fee loss | ASIC vs fair |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| zipf | 1% | one-chain | 3,520 | 11.41% | 100.00% | 1.00 | - | - | 24.9 | 16,853 | 0.39 | 0.00% | 0.86% | 0.99 | 430 | 84,314 | 0.06 | 0.00% | 0.18% | 1.00 |
| zipf | 1% | band-weak-levels, weak work | 3,520 | 11.41% | 56.47% | 0.67 | 41.4 | 0.00% | 25.1 | 17,479 | 0.20 | 0.00% | 0.85% | 1.01 | 432 | 83,537 | 0.05 | 0.00% | 0.18% | 1.00 |
| zipf | 10% | one-chain | 28,456 | 9.23% | 100.00% | 1.00 | - | - | 2.37 | 167,758 | 1.28 | 46.30% | 0.09% | 0.91 | 210 | 167,758 | 0.13 | 0.00% | 0.09% | 1.00 |
| zipf | 10% | band-weak-levels, weak work | 28,456 | 9.23% | 90.77% | 0.97 | 37.2 | 0.00% | 24.7 | 17,478 | 0.26 | 0.00% | 0.85% | 1.00 | 1,990 | 17,483 | 0.03 | 0.00% | 0.82% | 1.00 |
| zipf | 30% | one-chain | 78,101 | 8.44% | 100.00% | 1.00 | - | - | 0.803 | 497,785 | 2.28 | 78.10% | 0.03% | 0.92 | 71.4 | 497,785 | 0.23 | 0.00% | 0.03% | 0.99 |
| zipf | 30% | band-weak-levels, weak work | 78,101 | 8.44% | 100.00% | 1.00 | 34.7 | 0.00% | 24.0 | 17,491 | 0.40 | 0.00% | 0.84% | 0.97 | 1,906 | 17,491 | 0.04 | 0.00% | 0.78% | 1.00 |
| medium | 1% | one-chain | 3,554 | 0.81% | 100.00% | 1.00 | - | - | 24.9 | 16,853 | 0.39 | 0.00% | 0.86% | 0.99 | 430 | 84,314 | 0.06 | 0.00% | 0.18% | 1.00 |
| medium | 1% | band-weak-levels, weak work | 3,554 | 0.81% | 82.05% | 0.96 | 40.4 | 0.00% | 24.6 | 17,540 | 0.20 | 0.00% | 0.84% | 0.99 | 432 | 83,431 | 0.05 | 0.00% | 0.18% | 1.00 |
| medium | 10% | one-chain | 35,614 | 0.08% | 100.00% | 1.00 | - | - | 2.37 | 167,758 | 1.28 | 46.30% | 0.09% | 0.91 | 210 | 167,758 | 0.13 | 0.00% | 0.09% | 1.00 |
| medium | 10% | band-weak-levels, weak work | 35,614 | 0.08% | 100.00% | 1.00 | 34.7 | 0.00% | 24.5 | 17,479 | 0.27 | 0.00% | 0.85% | 0.99 | 1,967 | 17,481 | 0.03 | 0.00% | 0.81% | 1.00 |
| medium | 30% | one-chain | 106,857 | 0.03% | 100.00% | 1.00 | - | - | 0.803 | 497,785 | 2.28 | 78.10% | 0.03% | 0.92 | 71.4 | 497,785 | 0.23 | 0.00% | 0.03% | 0.99 |
| medium | 30% | band-weak-levels, weak work | 106,857 | 0.03% | 100.00% | 1.00 | 34.7 | 0.00% | 23.9 | 17,477 | 0.39 | 0.00% | 0.84% | 0.97 | 1,907 | 17,477 | 0.04 | 0.00% | 0.78% | 1.00 |
| medium, few big | 1% | one-chain | 3,505 | 2.70% | 100.00% | 1.00 | - | - | 24.9 | 16,853 | 0.39 | 0.00% | 0.86% | 0.99 | 430 | 84,314 | 0.06 | 0.00% | 0.18% | 1.00 |
| medium, few big | 1% | band-weak-levels, weak work | 3,505 | 2.70% | 74.07% | 0.87 | 40.9 | 0.00% | 24.8 | 17,511 | 0.19 | 0.00% | 0.85% | 1.00 | 432 | 83,492 | 0.05 | 0.00% | 0.18% | 1.00 |
| medium, few big | 10% | one-chain | 35,119 | 0.27% | 100.00% | 1.00 | - | - | 2.37 | 167,758 | 1.28 | 46.30% | 0.09% | 0.91 | 210 | 167,758 | 0.13 | 0.00% | 0.09% | 1.00 |
| medium, few big | 10% | band-weak-levels, weak work | 35,119 | 0.27% | 100.00% | 1.00 | 34.7 | 0.00% | 24.9 | 17,478 | 0.26 | 0.00% | 0.85% | 1.00 | 1,969 | 17,478 | 0.03 | 0.00% | 0.81% | 1.00 |
| medium, few big | 30% | one-chain | 105,372 | 0.09% | 100.00% | 1.00 | - | - | 0.803 | 497,785 | 2.28 | 78.10% | 0.03% | 0.92 | 71.4 | 497,785 | 0.23 | 0.00% | 0.03% | 0.99 |
| medium, few big | 30% | band-weak-levels, weak work | 105,372 | 0.09% | 100.00% | 1.00 | 34.7 | 0.00% | 24.0 | 17,481 | 0.40 | 0.00% | 0.84% | 0.97 | 1,906 | 17,481 | 0.04 | 0.00% | 0.78% | 1.00 |


### Study: a big miner joining or leaving at once (50% industrial)

#### Settling

| Pool | Big miner | Its jobs | Minimum after / before | C before | C after | R before | R after | Minimum within 10% | C within 10% | R within 10% | Weak expired, first 6 h |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 1% | joins | node default | 1.09 | 29.7 | 59.2 | 37.9 | 41.3 | < 1 h | 1 h | < 1 h | 0.00% |
| 1% | joins | at the minimum | 3.45 | 29.9 | 1.80 | 37.7 | 42.4 | 1 h | 8 h | < 1 h | 0.00% |
| 1% | leaves | node default | 0.91 | 59.1 | 30.1 | 41.5 | 37.8 | < 1 h | 3 h | < 1 h | 0.00% |
| 1% | leaves | at the minimum | 0.29 | 1.73 | 30.7 | 42.6 | 37.7 | 1 h | 3 h | 1 h | 0.00% |
| 10% | joins | node default | 1.18 | 1.00 | 5.80 | 34.9 | 40.8 | 2 h | 4 h | 3 h | 0.00% |
| 10% | joins | at the minimum | 2.00 | 1.00 | 1.00 | 34.9 | 34.9 | 1 h | < 1 h | < 1 h | 0.00% |
| 10% | leaves | node default | 0.85 | 5.82 | 1.00 | 40.9 | 34.9 | < 1 h | 3 h | < 1 h | 0.00% |
| 10% | leaves | at the minimum | 0.50 | 1.00 | 1.00 | 35.0 | 35.0 | 1 h | < 1 h | < 1 h | 0.00% |

#### Home miners' expected pay over fair, after the event

| Pool | Big miner | Its jobs | Before | 0-1 h | 1-6 h | 6-24 h | 1-2 d | 2-4 d | 4-8 d | 8-14 d |
|---|---|---|---|---|---|---|---|---|---|---|
| 1% | joins | node default | 1.01 | 1.91 | 1.15 | 1.00 | 1.02 | 1.00 | 1.01 | 1.01 |
| 1% | joins | at the minimum | 1.00 | 1.91 | 1.14 | 1.00 | 1.00 | 1.00 | 1.00 | 1.00 |
| 1% | leaves | node default | 1.01 | 0.54 | 0.83 | 1.02 | 1.02 | 1.00 | 1.00 | 1.01 |
| 1% | leaves | at the minimum | 1.00 | 0.52 | 0.82 | 1.01 | 1.01 | 1.02 | 1.01 | 1.01 |
| 10% | joins | node default | 1.00 | 1.37 | 1.00 | 1.02 | 1.01 | 1.01 | 1.01 | 1.01 |
| 10% | joins | at the minimum | 1.00 | 1.34 | 1.00 | 1.00 | 1.00 | 1.00 | 1.00 | 1.00 |
| 10% | leaves | node default | 1.00 | 0.75 | 1.00 | 1.00 | 1.00 | 1.00 | 1.00 | 1.00 |
| 10% | leaves | at the minimum | 1.00 | 0.71 | 1.00 | 1.00 | 1.00 | 1.00 | 1.00 | 1.00 |


### Study: a tiny pool at the ceiling (1 GH/s on a 37 GH/s test network)

| Design | Minimum / ceiling | Small share multiple | Large share multiple | R | Weak/s | Shares/s | Small pay/mo | Small median sat | Small swing | Small empty months | Small fee loss | Small vs fair | Large pay/mo | Large median sat | Large swing | Large empty months | Large fee loss | Large vs fair | Pays exactly as |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| one-chain, node default | 1.00 | 1.00 | 1.00 | - | 0 | 0.051 | 1,167 | 7,727,481 | 0.03 | 0.00% | 0.00% | 1.00 | 1,167 | 23,210,018 | 0.03 | 0.00% | 0.00% | 1.00 | - |
| band, node default | 1.00 | 7.10 | 21.3 | - | 0 | 3.6e-3 | 1,168 | 7,750,930 | 0.03 | 0.00% | 0.00% | 1.04 | 1,168 | 23,186,569 | 0.03 | 0.00% | 0.00% | 0.99 | - |
| band-weak, weak work, node default | 1.00 | 7.10 | 21.3 | 1.00 | 0 | 3.6e-3 | 1,168 | 7,750,930 | 0.03 | 0.00% | 0.00% | 1.04 | 1,168 | 23,186,569 | 0.03 | 0.00% | 0.00% | 0.99 | band, node default |
| band-weak-levels, weak work, node default | 1.00 | 7.10 | 21.3 | 1.00 | 0 | 3.6e-3 | 1,169 | 7,750,930 | 0.03 | 0.00% | 0.00% | 1.04 | 1,169 | 23,186,569 | 0.03 | 0.00% | 0.00% | 0.99 | - |
| band, all at the minimum | 1.00 | 1.00 | 1.00 | - | 0 | 0.051 | 1,167 | 7,727,481 | 0.03 | 0.00% | 0.00% | 1.00 | 1,167 | 23,210,018 | 0.03 | 0.00% | 0.00% | 1.00 | one-chain, node default |
| band-weak, weak work, all at the minimum | 1.00 | 1.00 | 1.00 | 1.00 | 0 | 0.051 | 1,167 | 7,727,481 | 0.03 | 0.00% | 0.00% | 1.00 | 1,167 | 23,210,018 | 0.03 | 0.00% | 0.00% | 1.00 | one-chain, node default |
| band-weak-levels, weak work, all at the minimum | 1.00 | 1.00 | 1.00 | 1.00 | 0 | 0.051 | 1,167 | 7,726,972 | 0.03 | 0.00% | 0.00% | 1.00 | 1,167 | 23,210,527 | 0.03 | 0.00% | 0.00% | 1.00 | - |


### Study: large miners filling the weak slots (band-weak-levels, weak work)

| Pool | Size | Flood | Pool with flood | INCLUDER_CUT | R | Bitaxe weak/mo | Bitaxe pay/mo | Bitaxe median sat | Bitaxe swing | Bitaxe empty months | Bitaxe expected vs fair | Bitaxe simulated vs fair | Flood's part of weak shares | Flood vs fair | Flood's cuts paid less received, of its fair income |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1,000 home miners | 0.03% | 0x | 0.03% | 1.00% | 34.8 | 9,753 | 14.1 | 30,949 | 0.26 | 0.00% | 1.00 | 1.00 | - | - | - |
| 1,000 home miners | 0.03% | 10x | 0.33% | 1.00% | 34.8 | 885 | 24.8 | 17,478 | 0.20 | 0.00% | 1.00 | 1.00 | 90.91% | 1.00 | 0.00% |
| 1,000 home miners | 0.03% | 100x | 3.01% | 1.00% | 34.7 | 96.4 | 23.9 | 18,136 | 0.24 | 0.00% | 1.00 | 1.00 | 98.92% | 1.00 | -0.00% |
| 1,000 home miners | 0.03% | 1000x | 29.76% | 1.00% | 34.7 | 9.48 | 18.5 | 23,502 | 0.39 | 0.10% | 1.00 | 0.98 | 99.89% | 1.00 | -0.00% |
| 1,000 home miners | 0.03% | 0x | 0.03% | 5.00% | 34.8 | 9,753 | 14.1 | 30,950 | 0.26 | 0.00% | 1.00 | 1.00 | - | - | - |
| 1,000 home miners | 0.03% | 10x | 0.33% | 5.00% | 34.8 | 885 | 24.9 | 17,574 | 0.21 | 0.00% | 1.00 | 1.01 | 90.91% | 1.00 | 0.00% |
| 1,000 home miners | 0.03% | 100x | 3.01% | 5.00% | 34.7 | 96.4 | 23.7 | 18,233 | 0.24 | 0.00% | 1.00 | 1.00 | 98.92% | 1.00 | -0.01% |
| 1,000 home miners | 0.03% | 1000x | 29.76% | 5.00% | 34.7 | 9.48 | 18.5 | 22,553 | 0.40 | 0.00% | 1.00 | 0.97 | 99.89% | 1.00 | -0.01% |
| 90% industrial | 1% | 0x | 1% | 1.00% | 41.9 | 2,908 | 25.3 | 17,481 | 0.20 | 0.00% | 1.01 | 1.02 | - | - | - |
| 90% industrial | 1% | 10x | 2.00% | 1.00% | 45.4 | 265 | 25.2 | 17,535 | 0.21 | 0.00% | 1.00 | 1.02 | 90.69% | 1.00 | 0.23% |
| 90% industrial | 1% | 100x | 10.98% | 1.00% | 34.7 | 26.4 | 24.1 | 17,991 | 0.28 | 0.00% | 1.00 | 1.00 | 90.89% | 1.00 | -0.00% |
| 90% industrial | 1% | 0x | 1% | 5.00% | 41.9 | 2,908 | 24.8 | 17,543 | 0.20 | 0.00% | 1.01 | 1.00 | - | - | - |
| 90% industrial | 1% | 10x | 2.00% | 5.00% | 45.4 | 265 | 24.3 | 17,579 | 0.22 | 0.00% | 0.99 | 0.98 | 90.69% | 0.99 | 1.17% |
| 90% industrial | 1% | 100x | 10.98% | 5.00% | 34.7 | 26.4 | 24.0 | 18,136 | 0.29 | 0.00% | 1.00 | 1.00 | 90.89% | 1.00 | -0.01% |


### Study: steering the level draw (band-weak-levels, weak work, 50% industrial)

| Pool | Steering miner | Level | Picks between | Blocks it finds a month | Draws where the other parent is better | Gain sat/mo | Orphan cost sat/mo | Net sat/mo | Net of its income |
|---|---|---|---|---|---|---|---|---|---|
| 1% | Bitaxe 1.2 TH/s | 5 | races only | 0.014 | 5.49% | 0.284 | 0 | +0.284 | +0.00% |
| 1% | Bitaxe 1.2 TH/s | 5 | tip or its parent | 0.014 | 5.49% | 13.2 | 326 | -312 | -0.07% |
| 1% | ASIC 100 TH/s | 7 | races only | 1.17 | 0.86% | 8.1e-3 | 0 | +8.1e-3 | +0.00% |
| 1% | ASIC 100 TH/s | 7 | tip or its parent | 1.17 | 0.86% | 0.379 | 4,274 | -4,274 | -0.01% |
| 10% | Bitaxe 1.2 TH/s | 5 | races only | 0.014 | 20.19% | 0.022 | 0 | +0.022 | +0.00% |
| 10% | Bitaxe 1.2 TH/s | 5 | tip or its parent | 0.014 | 20.19% | 1.02 | 1,104 | -1,103 | -0.25% |
| 10% | ASIC 100 TH/s | 7 | races only | 1.17 | 11.80% | 170 | 0 | +170 | +0.00% |
| 10% | ASIC 100 TH/s | 7 | tip or its parent | 1.17 | 11.80% | 7,918 | 53,782 | -45,864 | -0.13% |
| 30% | Bitaxe 1.2 TH/s | 5 | races only | 0.014 | 14.81% | 1.7e-3 | 0 | +1.7e-3 | +0.00% |
| 30% | Bitaxe 1.2 TH/s | 5 | tip or its parent | 0.014 | 14.81% | 0.079 | 810 | -810 | -0.19% |
| 30% | ASIC 100 TH/s | 7 | races only | 1.17 | 17.17% | 52.4 | 0 | +52.4 | +0.00% |
| 30% | ASIC 100 TH/s | 7 | tip or its parent | 1.17 | 17.17% | 2,441 | 78,231 | -75,789 | -0.21% |


### Study: weak shares lost to stale parents and uncle carriers (band-weak-levels, weak work, 50% industrial)

| Pool | Period | Stale | Losses | Weak lost | Weak expired | Bitaxe weak/mo | Bitaxe pay/mo | Bitaxe swing | Bitaxe empty months | Bitaxe expected vs fair | Bitaxe simulated vs fair | ASIC expected vs fair |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1% | 10 s | 2.10% | none | 0.00% | 0.00% | 580 | 24.6 | 0.20 | 0.00% | 1.00 | 0.99 | 1.00 |
| 1% | 10 s | 2.10% | stale parent | 1.05% | 0.00% | 579 | 24.6 | 0.21 | 0.00% | 0.99 | 0.99 | 0.99 |
| 1% | 10 s | 2.10% | uncle carrier drops | 0.00% | 0.00% | 580 | 24.6 | 0.20 | 0.00% | 1.00 | 0.99 | 1.00 |
| 1% | 10 s | 2.10% | both | 1.05% | 0.00% | 579 | 24.6 | 0.21 | 0.00% | 0.99 | 0.99 | 0.99 |
| 1% | 2 s | 8.70% | none | 0.00% | 0.00% | 2,915 | 24.7 | 0.20 | 0.00% | 1.00 | 1.00 | 1.00 |
| 1% | 2 s | 8.70% | stale parent | 4.36% | 0.00% | 2,905 | 24.0 | 0.20 | 0.00% | 0.98 | 0.97 | 1.02 |
| 1% | 2 s | 8.70% | uncle carrier drops | 0.00% | 0.00% | 2,915 | 24.7 | 0.20 | 0.00% | 1.00 | 1.00 | 1.00 |
| 1% | 2 s | 8.70% | both | 4.36% | 0.00% | 2,905 | 24.0 | 0.20 | 0.00% | 0.98 | 0.97 | 1.02 |
| 10% | 10 s | 2.10% | none | 0.00% | 0.00% | 29.1 | 24.8 | 0.27 | 0.00% | 1.00 | 1.00 | 1.00 |
| 10% | 10 s | 2.10% | stale parent | 1.05% | 0.00% | 29.0 | 25.1 | 0.28 | 0.00% | 1.00 | 1.01 | 1.00 |
| 10% | 10 s | 2.10% | uncle carrier drops | 0.00% | 0.00% | 29.1 | 24.8 | 0.27 | 0.00% | 1.00 | 1.00 | 1.00 |
| 10% | 10 s | 2.10% | both | 1.05% | 0.00% | 29.0 | 25.1 | 0.28 | 0.00% | 1.00 | 1.01 | 1.00 |
| 10% | 2 s | 8.70% | none | 0.00% | 0.00% | 194 | 25.0 | 0.22 | 0.00% | 1.00 | 1.01 | 1.00 |
| 10% | 2 s | 8.70% | stale parent | 4.35% | 0.00% | 194 | 24.4 | 0.21 | 0.00% | 0.99 | 0.98 | 0.99 |
| 10% | 2 s | 8.70% | uncle carrier drops | 0.00% | 0.00% | 194 | 25.0 | 0.22 | 0.00% | 1.00 | 1.01 | 1.00 |
| 10% | 2 s | 8.70% | both | 4.35% | 0.00% | 194 | 24.4 | 0.21 | 0.00% | 0.99 | 0.98 | 0.99 |


### Study constants: WEAK_DEPTH and INCLUDER_CUT against a refusing coalition (band-weak-levels, weak work, 50% industrial)

| Pool | Fork choice | WEAK_DEPTH | INCLUDER_CUT | Refusing | Honest weak lost | Bitaxe vs fair | Bitaxe swing | Coalition gains | Refusing ASIC gains | Lone refuser gains |
|---|---|---|---|---|---|---|---|---|---|---|
| 1% | weak work | 6 | 0.00% | 50.00% | 4.69% | 0.97 | 0.21 | +1.16% | +1.16% | -0.00% |
| 1% | weak work | 6 | 0.00% | 90.00% | 54.51% | 0.48 | 0.29 | +2.87% | +2.86% | -0.00% |
| 1% | weak work | 6 | 1.00% | 50.00% | 4.69% | 0.97 | 0.20 | +0.97% | +0.81% | -0.91% |
| 1% | weak work | 6 | 1.00% | 90.00% | 54.51% | 0.49 | 0.29 | +2.82% | +2.77% | -0.91% |
| 1% | weak work | 6 | 5.00% | 50.00% | 4.69% | 0.98 | 0.20 | +0.22% | -0.60% | -4.56% |
| 1% | weak work | 6 | 5.00% | 90.00% | 54.51% | 0.52 | 0.29 | +2.63% | +2.39% | -4.56% |
| 1% | weak work | 12 | 0.00% | 50.00% | 0.24% | 1.00 | 0.20 | +0.06% | +0.05% | -0.01% |
| 1% | weak work | 12 | 0.00% | 90.00% | 28.60% | 0.73 | 0.24 | +1.48% | +1.48% | -0.01% |
| 1% | weak work | 12 | 1.00% | 50.00% | 0.24% | 1.00 | 0.20 | -0.12% | -0.28% | -0.91% |
| 1% | weak work | 12 | 1.00% | 90.00% | 28.60% | 0.74 | 0.24 | +1.44% | +1.40% | -0.91% |
| 1% | weak work | 12 | 5.00% | 50.00% | 0.24% | 1.01 | 0.20 | -0.85% | -1.62% | -4.55% |
| 1% | weak work | 12 | 5.00% | 90.00% | 28.60% | 0.76 | 0.23 | +1.28% | +1.09% | -4.55% |
| 1% | weak work | 24 | 0.00% | 50.00% | 0.00% | 1.00 | 0.20 | -0.00% | -0.00% | -0.01% |
| 1% | weak work | 24 | 0.00% | 90.00% | 8.85% | 0.92 | 0.21 | +0.45% | +0.45% | -0.01% |
| 1% | weak work | 24 | 1.00% | 50.00% | 0.00% | 1.00 | 0.21 | -0.18% | -0.34% | -0.91% |
| 1% | weak work | 24 | 1.00% | 90.00% | 8.85% | 0.92 | 0.22 | +0.42% | +0.39% | -0.91% |
| 1% | weak work | 24 | 5.00% | 50.00% | 0.00% | 1.01 | 0.20 | -0.90% | -1.70% | -4.56% |
| 1% | weak work | 24 | 5.00% | 90.00% | 8.85% | 0.94 | 0.22 | +0.28% | +0.13% | -4.56% |
| 1% | weak work, siblings | 6 | 0.00% | 50.00% | 2.31% | 1.04 | 0.21 | -5.04% | -16.30% | -0.73% |
| 1% | weak work, siblings | 6 | 0.00% | 90.00% | 53.02% | 0.51 | 0.30 | +2.53% | -0.85% | -0.73% |
| 1% | weak work, siblings | 6 | 1.00% | 50.00% | 2.31% | 1.04 | 0.20 | -5.26% | -16.72% | -1.63% |
| 1% | weak work, siblings | 6 | 1.00% | 90.00% | 53.02% | 0.52 | 0.29 | +2.48% | -0.93% | -1.63% |
| 1% | weak work, siblings | 6 | 5.00% | 50.00% | 2.31% | 1.06 | 0.20 | -6.16% | -18.41% | -5.26% |
| 1% | weak work, siblings | 6 | 5.00% | 90.00% | 53.02% | 0.55 | 0.29 | +2.30% | -1.24% | -5.26% |
| 1% | weak work, siblings | 12 | 0.00% | 50.00% | 0.09% | 1.06 | 0.21 | -5.62% | -16.90% | -0.72% |
| 1% | weak work, siblings | 12 | 0.00% | 90.00% | 27.45% | 0.77 | 0.23 | +1.12% | -2.31% | -0.72% |
| 1% | weak work, siblings | 12 | 1.00% | 50.00% | 0.09% | 1.06 | 0.21 | -5.84% | -17.33% | -1.63% |
| 1% | weak work, siblings | 12 | 1.00% | 90.00% | 27.45% | 0.77 | 0.24 | +1.08% | -2.39% | -1.63% |
| 1% | weak work, siblings | 12 | 5.00% | 50.00% | 0.09% | 1.07 | 0.20 | -6.72% | -19.03% | -5.26% |
| 1% | weak work, siblings | 12 | 5.00% | 90.00% | 27.45% | 0.79 | 0.23 | +0.92% | -2.70% | -5.26% |
| 1% | weak work, siblings | 24 | 0.00% | 50.00% | 0.00% | 1.06 | 0.20 | -5.59% | -16.78% | -0.74% |
| 1% | weak work, siblings | 24 | 0.00% | 90.00% | 8.32% | 0.95 | 0.21 | +0.10% | -3.31% | -0.74% |
| 1% | weak work, siblings | 24 | 1.00% | 50.00% | 0.00% | 1.06 | 0.20 | -5.81% | -17.20% | -1.65% |
| 1% | weak work, siblings | 24 | 1.00% | 90.00% | 8.32% | 0.96 | 0.21 | +0.06% | -3.38% | -1.65% |
| 1% | weak work, siblings | 24 | 5.00% | 50.00% | 0.00% | 1.07 | 0.19 | -6.70% | -18.92% | -5.29% |
| 1% | weak work, siblings | 24 | 5.00% | 90.00% | 8.32% | 0.97 | 0.21 | -0.08% | -3.65% | -5.29% |
| 10% | weak work | 6 | 0.00% | 50.00% | 4.73% | 0.98 | 0.27 | +2.35% | +2.34% | -0.05% |
| 10% | weak work | 6 | 0.00% | 90.00% | 55.60% | 0.49 | 0.39 | +5.73% | +5.73% | -0.05% |
| 10% | weak work | 6 | 1.00% | 50.00% | 4.73% | 0.98 | 0.26 | +1.97% | +1.95% | -1.03% |
| 10% | weak work | 6 | 1.00% | 90.00% | 55.60% | 0.49 | 0.40 | +5.64% | +5.64% | -1.03% |
| 10% | weak work | 6 | 5.00% | 50.00% | 4.73% | 1.00 | 0.27 | +0.48% | +0.41% | -4.95% |
| 10% | weak work | 6 | 5.00% | 90.00% | 55.60% | 0.53 | 0.42 | +5.29% | +5.27% | -4.95% |
| 10% | weak work | 12 | 0.00% | 50.00% | 0.24% | 1.00 | 0.26 | +0.12% | +0.11% | -0.01% |
| 10% | weak work | 12 | 0.00% | 90.00% | 29.41% | 0.74 | 0.31 | +2.95% | +2.95% | -0.01% |
| 10% | weak work | 12 | 1.00% | 50.00% | 0.24% | 1.00 | 0.26 | -0.24% | -0.25% | -0.99% |
| 10% | weak work | 12 | 1.00% | 90.00% | 29.41% | 0.74 | 0.32 | +2.87% | +2.89% | -0.99% |
| 10% | weak work | 12 | 5.00% | 50.00% | 0.24% | 1.02 | 0.28 | -1.67% | -1.71% | -4.92% |
| 10% | weak work | 12 | 5.00% | 90.00% | 29.41% | 0.77 | 0.32 | +2.57% | +2.63% | -4.92% |
| 10% | weak work | 24 | 0.00% | 50.00% | 0.00% | 1.00 | 0.27 | -0.00% | -0.01% | -0.01% |
| 10% | weak work | 24 | 0.00% | 90.00% | 9.15% | 0.92 | 0.29 | +0.90% | +0.89% | -0.01% |
| 10% | weak work | 24 | 1.00% | 50.00% | 0.00% | 1.00 | 0.28 | -0.36% | -0.38% | -0.99% |
| 10% | weak work | 24 | 1.00% | 90.00% | 9.15% | 0.93 | 0.28 | +0.84% | +0.79% | -0.99% |
| 10% | weak work | 24 | 5.00% | 50.00% | 0.00% | 1.02 | 0.27 | -1.78% | -1.85% | -4.92% |
| 10% | weak work | 24 | 5.00% | 90.00% | 9.15% | 0.95 | 0.30 | +0.58% | +0.41% | -4.92% |
| 10% | weak work, siblings | 6 | 0.00% | 50.00% | 2.23% | 1.12 | 0.27 | -11.72% | -11.72% | -0.31% |
| 10% | weak work, siblings | 6 | 0.00% | 90.00% | 54.00% | 0.54 | 0.37 | +5.18% | +5.18% | -0.31% |
| 10% | weak work, siblings | 6 | 1.00% | 50.00% | 2.23% | 1.12 | 0.27 | -12.20% | -12.20% | -1.28% |
| 10% | weak work, siblings | 6 | 1.00% | 90.00% | 54.00% | 0.54 | 0.38 | +5.09% | +5.09% | -1.28% |
| 10% | weak work, siblings | 6 | 5.00% | 50.00% | 2.23% | 1.14 | 0.28 | -14.14% | -14.14% | -5.19% |
| 10% | weak work, siblings | 6 | 5.00% | 90.00% | 54.00% | 0.58 | 0.39 | +4.73% | +4.74% | -5.19% |
| 10% | weak work, siblings | 12 | 0.00% | 50.00% | 0.08% | 1.13 | 0.27 | -12.82% | -12.79% | -0.33% |
| 10% | weak work, siblings | 12 | 0.00% | 90.00% | 29.03% | 0.79 | 0.31 | +2.37% | +2.38% | -0.33% |
| 10% | weak work, siblings | 12 | 1.00% | 50.00% | 0.08% | 1.13 | 0.26 | -13.30% | -13.21% | -1.28% |
| 10% | weak work, siblings | 12 | 1.00% | 90.00% | 29.03% | 0.79 | 0.31 | +2.29% | +2.33% | -1.28% |
| 10% | weak work, siblings | 12 | 5.00% | 50.00% | 0.08% | 1.15 | 0.27 | -15.21% | -14.90% | -5.08% |
| 10% | weak work, siblings | 12 | 5.00% | 90.00% | 29.03% | 0.82 | 0.33 | +1.98% | +2.15% | -5.08% |
| 10% | weak work, siblings | 24 | 0.00% | 50.00% | 0.00% | 1.13 | 0.27 | -12.81% | -12.79% | -0.33% |
| 10% | weak work, siblings | 24 | 0.00% | 90.00% | 8.98% | 0.98 | 0.27 | +0.20% | +0.20% | -0.33% |
| 10% | weak work, siblings | 24 | 1.00% | 50.00% | 0.00% | 1.13 | 0.27 | -13.29% | -13.22% | -1.28% |
| 10% | weak work, siblings | 24 | 1.00% | 90.00% | 8.98% | 0.99 | 0.27 | +0.13% | +0.15% | -1.28% |
| 10% | weak work, siblings | 24 | 5.00% | 50.00% | 0.00% | 1.15 | 0.27 | -15.18% | -14.95% | -5.09% |
| 10% | weak work, siblings | 24 | 5.00% | 90.00% | 8.98% | 1.01 | 0.30 | -0.12% | -0.06% | -5.09% |


### Study constants: R_MAX and the start from genesis (50% industrial)

| Pool | R_MAX | R settles at | R gets within 10% | Weak found/s, first hour | Weak KB/s, first hour | Weak expired, first hour |
|---|---|---|---|---|---|---|
| 1% | 256 | 37.7 | 2 h | 9.78 | 7.82 | 32.96% |
| 1% | 1024 | 38.0 | 3 h | 41.2 | 33.0 | 77.00% |
| 1% | 4096 | 37.8 | 4 h | 169 | 135 | 85.60% |
| 10% | 256 | 35.0 | 2 h | 14.2 | 11.4 | 48.99% |
| 10% | 1024 | 35.0 | 3 h | 55.5 | 44.4 | 81.49% |
| 10% | 4096 | 34.9 | 4 h | 228 | 183 | 90.69% |


### Study constants: R_MAX 256 and 4,096 in every miner mix (band-weak-levels, weak work)

| Mix | Pool | R_MAX | Hashrate at the minimum | R | Weak expired | Bitaxe pay/mo | Bitaxe median sat | Bitaxe swing | Bitaxe empty months | Bitaxe fee loss | Bitaxe vs fair | ASIC pay/mo | ASIC median sat | ASIC swing | ASIC empty months | ASIC fee loss | ASIC vs fair |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| home | 1% | 256 | 100.00% | 34.7 | 0.00% | 24.9 | 17,476 | 0.21 | 0.00% | 0.85% | 1.00 | 432 | 83,507 | 0.05 | 0.00% | 0.18% | 1.00 |
| home | 1% | 4096 | 100.00% | 34.7 | 0.00% | 24.7 | 17,481 | 0.21 | 0.00% | 0.85% | 1.00 | 432 | 83,512 | 0.05 | 0.00% | 0.18% | 1.00 |
| home | 10% | 256 | 100.00% | 34.7 | 0.00% | 25.0 | 17,476 | 0.26 | 0.00% | 0.85% | 1.01 | 1,966 | 17,476 | 0.03 | 0.00% | 0.81% | 1.00 |
| home | 10% | 4096 | 100.00% | 34.7 | 0.00% | 24.6 | 17,477 | 0.28 | 0.00% | 0.85% | 0.99 | 1,966 | 17,480 | 0.03 | 0.00% | 0.81% | 1.00 |
| home | 30% | 256 | 100.00% | 34.7 | 0.00% | 24.0 | 17,476 | 0.39 | 0.00% | 0.84% | 0.97 | 1,908 | 17,476 | 0.04 | 0.00% | 0.78% | 1.00 |
| home | 30% | 4096 | 100.00% | 34.7 | 0.00% | 24.0 | 17,475 | 0.38 | 0.10% | 0.84% | 0.97 | 1,908 | 17,475 | 0.04 | 0.00% | 0.78% | 1.00 |
| 50% industrial | 1% | 256 | 50.14% | 37.2 | 0.00% | 25.2 | 17,481 | 0.20 | 0.00% | 0.85% | 1.02 | 432 | 83,681 | 0.05 | 0.00% | 0.18% | 1.00 |
| 50% industrial | 1% | 4096 | 50.14% | 37.3 | 0.00% | 25.0 | 17,477 | 0.20 | 0.00% | 0.85% | 1.01 | 432 | 83,681 | 0.05 | 0.00% | 0.18% | 1.00 |
| 50% industrial | 10% | 256 | 100.00% | 34.7 | 0.00% | 24.8 | 17,485 | 0.27 | 0.00% | 0.85% | 1.00 | 1,967 | 17,491 | 0.03 | 0.00% | 0.81% | 1.00 |
| 50% industrial | 10% | 4096 | 100.00% | 34.7 | 0.00% | 24.7 | 17,482 | 0.27 | 0.00% | 0.85% | 1.00 | 1,967 | 17,482 | 0.03 | 0.00% | 0.81% | 1.00 |
| 50% industrial | 30% | 256 | 100.00% | 34.7 | 0.00% | 24.0 | 17,487 | 0.39 | 0.00% | 0.84% | 0.97 | 1,906 | 17,487 | 0.04 | 0.00% | 0.78% | 1.00 |
| 50% industrial | 30% | 4096 | 100.00% | 34.7 | 0.00% | 24.0 | 17,476 | 0.38 | 0.10% | 0.84% | 0.97 | 1,909 | 17,476 | 0.04 | 0.00% | 0.78% | 1.00 |
| 90% industrial | 1% | 256 | 9.98% | 41.9 | 0.00% | 25.3 | 17,481 | 0.20 | 0.00% | 0.85% | 1.02 | 432 | 84,205 | 0.05 | 0.00% | 0.18% | 1.00 |
| 90% industrial | 1% | 4096 | 9.98% | 41.9 | 0.00% | 25.0 | 17,492 | 0.20 | 0.00% | 0.85% | 1.01 | 432 | 84,205 | 0.05 | 0.00% | 0.18% | 1.00 |
| 90% industrial | 10% | 256 | 100.00% | 34.7 | 0.00% | 24.9 | 17,534 | 0.26 | 0.00% | 0.84% | 1.01 | 1,964 | 17,567 | 0.03 | 0.00% | 0.81% | 1.00 |
| 90% industrial | 10% | 4096 | 100.00% | 34.7 | 0.00% | 24.7 | 17,542 | 0.27 | 0.00% | 0.84% | 1.00 | 1,960 | 17,574 | 0.03 | 0.00% | 0.81% | 1.00 |
| 90% industrial | 30% | 256 | 100.00% | 34.7 | 0.00% | 23.9 | 17,474 | 0.39 | 0.00% | 0.84% | 0.97 | 1,908 | 17,474 | 0.04 | 0.00% | 0.78% | 1.00 |
| 90% industrial | 30% | 4096 | 100.00% | 34.7 | 0.00% | 24.0 | 17,528 | 0.38 | 0.10% | 0.84% | 0.98 | 1,903 | 17,528 | 0.04 | 0.00% | 0.78% | 1.00 |
| zipf | 1% | 256 | 56.47% | 41.4 | 0.00% | 25.1 | 17,479 | 0.20 | 0.00% | 0.85% | 1.01 | 432 | 83,537 | 0.05 | 0.00% | 0.18% | 1.00 |
| zipf | 1% | 4096 | 56.47% | 41.3 | 0.00% | 24.9 | 17,487 | 0.19 | 0.00% | 0.85% | 1.00 | 432 | 83,544 | 0.05 | 0.00% | 0.18% | 1.00 |
| zipf | 10% | 256 | 90.77% | 37.2 | 0.00% | 24.7 | 17,478 | 0.26 | 0.00% | 0.85% | 1.00 | 1,990 | 17,483 | 0.03 | 0.00% | 0.82% | 1.00 |
| zipf | 10% | 4096 | 90.77% | 37.1 | 0.00% | 24.9 | 17,480 | 0.27 | 0.00% | 0.85% | 1.01 | 1,990 | 17,486 | 0.03 | 0.00% | 0.82% | 1.00 |
| zipf | 30% | 256 | 100.00% | 34.7 | 0.00% | 24.0 | 17,491 | 0.40 | 0.00% | 0.84% | 0.97 | 1,906 | 17,491 | 0.04 | 0.00% | 0.78% | 1.00 |
| zipf | 30% | 4096 | 100.00% | 34.7 | 0.00% | 24.1 | 17,482 | 0.38 | 0.10% | 0.84% | 0.98 | 1,908 | 17,482 | 0.04 | 0.00% | 0.78% | 1.00 |
| medium | 1% | 256 | 82.05% | 40.4 | 0.00% | 24.6 | 17,540 | 0.20 | 0.00% | 0.84% | 0.99 | 432 | 83,431 | 0.05 | 0.00% | 0.18% | 1.00 |
| medium | 1% | 4096 | 82.05% | 40.4 | 0.00% | 24.9 | 17,552 | 0.21 | 0.00% | 0.84% | 1.01 | 433 | 83,446 | 0.05 | 0.00% | 0.18% | 1.00 |
| medium | 10% | 256 | 100.00% | 34.7 | 0.00% | 24.5 | 17,479 | 0.27 | 0.00% | 0.85% | 0.99 | 1,967 | 17,481 | 0.03 | 0.00% | 0.81% | 1.00 |
| medium | 10% | 4096 | 100.00% | 34.7 | 0.00% | 24.7 | 17,476 | 0.27 | 0.00% | 0.85% | 1.00 | 1,969 | 17,478 | 0.03 | 0.00% | 0.81% | 1.00 |
| medium | 30% | 256 | 100.00% | 34.7 | 0.00% | 23.9 | 17,477 | 0.39 | 0.00% | 0.84% | 0.97 | 1,907 | 17,477 | 0.04 | 0.00% | 0.78% | 1.00 |
| medium | 30% | 4096 | 100.00% | 34.7 | 0.00% | 24.0 | 17,478 | 0.38 | 0.10% | 0.84% | 0.97 | 1,908 | 17,478 | 0.04 | 0.00% | 0.78% | 1.00 |
| medium, few big | 1% | 256 | 74.07% | 40.9 | 0.00% | 24.8 | 17,511 | 0.19 | 0.00% | 0.85% | 1.00 | 432 | 83,492 | 0.05 | 0.00% | 0.18% | 1.00 |
| medium, few big | 1% | 4096 | 74.07% | 40.8 | 0.00% | 24.7 | 17,513 | 0.20 | 0.00% | 0.85% | 1.00 | 433 | 83,482 | 0.05 | 0.00% | 0.18% | 1.00 |
| medium, few big | 10% | 256 | 100.00% | 34.7 | 0.00% | 24.9 | 17,478 | 0.26 | 0.00% | 0.85% | 1.00 | 1,969 | 17,478 | 0.03 | 0.00% | 0.81% | 1.00 |
| medium, few big | 10% | 4096 | 100.00% | 34.7 | 0.00% | 25.0 | 17,481 | 0.27 | 0.00% | 0.85% | 1.01 | 1,966 | 17,485 | 0.03 | 0.00% | 0.81% | 1.00 |
| medium, few big | 30% | 256 | 100.00% | 34.7 | 0.00% | 24.0 | 17,481 | 0.40 | 0.00% | 0.84% | 0.97 | 1,906 | 17,481 | 0.04 | 0.00% | 0.78% | 1.00 |
| medium, few big | 30% | 4096 | 100.00% | 34.7 | 0.00% | 24.1 | 17,473 | 0.38 | 0.20% | 0.84% | 0.98 | 1,909 | 17,473 | 0.04 | 0.00% | 0.78% | 1.00 |
| 99% industrial | 1% | 256 | 1.00% | 87.5 | 0.00% | 26.6 | 17,539 | 0.20 | 0.00% | 0.84% | 1.07 | 433 | 84,063 | 0.06 | 0.00% | 0.18% | 1.00 |
| 99% industrial | 1% | 4096 | 1.00% | 87.5 | 0.00% | 26.7 | 17,510 | 0.19 | 0.00% | 0.85% | 1.08 | 433 | 84,063 | 0.06 | 0.00% | 0.18% | 1.00 |
| 99% industrial | 10% | 256 | 100.00% | 34.7 | 0.00% | 24.1 | 17,909 | 0.26 | 0.00% | 0.82% | 1.00 | 1,927 | 18,024 | 0.03 | 0.00% | 0.79% | 1.00 |
| 99% industrial | 10% | 4096 | 100.00% | 34.7 | 0.00% | 24.3 | 17,916 | 0.27 | 0.00% | 0.82% | 1.01 | 1,925 | 18,032 | 0.03 | 0.00% | 0.79% | 1.00 |
| 99% industrial | 30% | 256 | 100.00% | 34.7 | 0.00% | 24.0 | 17,483 | 0.40 | 0.00% | 0.84% | 0.97 | 1,907 | 17,483 | 0.04 | 0.00% | 0.78% | 1.00 |
| 99% industrial | 30% | 4096 | 100.00% | 34.7 | 0.00% | 24.0 | 17,583 | 0.38 | 0.10% | 0.84% | 0.98 | 1,898 | 17,583 | 0.04 | 0.00% | 0.78% | 1.00 |



## Scale: bigger pools on a bigger Bitcoin Cash, levels as a node sets them (band-weak-levels, weak work, 50% industrial)

| BCH hashrate | BCH difficulty | Pool | Minimum difficulty | R | Smallest miner with 3 weak shares a month (TH/s) | Bitaxe level | Bitaxe weak/mo | ASIC level | ASIC weak/mo | Bitaxe pay/mo | Bitaxe median sat | Bitaxe swing | Bitaxe empty months | Bitaxe fee loss | Bitaxe vs fair | ASIC pay/mo | ASIC median sat | ASIC swing | ASIC empty months | ASIC fee loss | ASIC vs fair |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| x1 | 5.17e10 | 10% | 843,382,441 | 34.7 | 0.121 | 5 | 29.0 | 5 | 2,410 | 24.8 | 17,485 | 0.27 | 0.00% | 0.85% | 1.00 | 1,967 | 17,491 | 0.03 | 0.00% | 0.81% | 1.00 |
| x1 | 5.17e10 | 30% | 2,530,147,322 | 34.7 | 0.363 | 5 | 9.40 | 5 | 804 | 24.0 | 17,487 | 0.39 | 0.00% | 0.84% | 0.97 | 1,906 | 17,487 | 0.04 | 0.00% | 0.78% | 1.00 |
| x1 | 5.17e10 | 50% | 4,216,912,203 | 34.7 | 0.604 | 5 | 5.59 | 5 | 482 | 17.1 | 23,994 | 0.48 | 0.50% | 0.61% | 0.96 | 1,439 | 23,994 | 0.05 | 0.00% | 0.59% | 1.00 |
| x1 | 5.17e10 | 90% | 7,590,441,965 | 34.7 | 1.09 | 5 | 3.06 | 5 | 268 | 9.45 | 43,162 | 0.65 | 4.70% | 0.34% | 0.95 | 820 | 43,162 | 0.07 | 0.00% | 0.34% | 1.00 |
| x10 | 5.17e11 | 10% | 8,433,824,405 | 34.7 | 1.21 | 4 | 2.76 | 5 | 242 | 4.74 | 8,736 | 0.75 | 10.00% | 1.69% | 0.96 | 206 | 17,489 | 0.09 | 0.00% | 0.85% | 1.00 |
| x10 | 5.17e11 | 30% | 25,301,473,215 | 34.7 | 3.63 | 4 | 0.962 | 5 | 80.6 | 2.96 | 14,400 | 1.18 | 40.50% | 1.02% | 0.99 | 204 | 17,494 | 0.13 | 0.00% | 0.84% | 1.00 |
| x10 | 5.17e11 | 50% | 42,169,122,025 | 34.7 | 6.04 | 4 | 0.573 | 5 | 48.3 | 1.75 | 23,993 | 1.49 | 57.10% | 0.61% | 0.98 | 148 | 23,993 | 0.16 | 0.00% | 0.61% | 1.00 |
| x10 | 5.17e11 | 90% | 75,904,419,646 | 34.7 | 10.9 | 4 | 0.309 | 5 | 26.8 | 0.961 | 43,160 | 1.98 | 73.70% | 0.34% | 0.96 | 82.6 | 43,160 | 0.21 | 0.00% | 0.34% | 1.00 |
| x100 | 5.17e12 | 10% | 84,338,244,051 | 34.7 | 12.1 | 1 | 0.284 | 5 | 24.2 | 0.877 | 4,801 | 2.06 | 75.40% | 3.05% | 0.98 | 20.9 | 17,481 | 0.30 | 0.00% | 0.85% | 1.01 |
| x100 | 5.17e12 | 30% | 253,014,732,152 | 34.7 | 36.3 | 1 | 0.089 | 5 | 7.81 | 0.285 | 14,400 | 3.74 | 91.10% | 1.01% | 0.97 | 19.7 | 17,476 | 0.43 | 0.20% | 0.84% | 0.96 |
| x100 | 5.17e12 | 50% | 421,691,220,254 | 34.7 | 60.4 | 1 | 0.052 | 5 | 4.62 | 0.157 | 23,993 | 4.86 | 95.00% | 0.60% | 0.90 | 14.3 | 23,993 | 0.51 | 1.20% | 0.61% | 0.96 |
| x100 | 5.17e12 | 90% | 759,044,196,457 | 34.7 | 109 | 1 | 0.024 | 5 | 2.55 | 0.072 | 43,160 | 7.06 | 97.70% | 0.33% | 0.73 | 7.90 | 43,160 | 0.71 | 8.60% | 0.34% | 0.95 |
| x270 | 1.40e13 | 10% | 227,713,258,937 | 34.7 | 32.6 | 1 | 0.097 | 5 | 8.68 | 0.309 | 4,801 | 3.55 | 90.70% | 3.04% | 0.94 | 7.58 | 17,483 | 0.49 | 0.50% | 0.85% | 0.99 |
| x270 | 1.40e13 | 30% | 683,139,776,811 | 34.7 | 97.9 | 1 | 0.028 | 5 | 2.83 | 0.114 | 14,400 | 6.18 | 97.00% | 1.01% | 1.04 | 7.31 | 17,484 | 0.70 | 6.90% | 0.84% | 0.96 |
| x270 | 1.40e13 | 50% | 1,138,566,294,685 | 34.7 | 163 | 1 | 0.015 | 5 | 1.72 | 0.049 | 23,993 | 9.20 | 98.50% | 0.60% | 0.75 | 5.24 | 23,993 | 0.87 | 18.90% | 0.61% | 0.95 |
| x270 | 1.40e13 | 90% | 2,049,419,330,433 | 34.7 | 294 | 1 | 0.011 | 5 | 0.978 | 0.031 | 43,160 | 10.62 | 98.90% | 0.34% | 0.84 | 2.94 | 43,160 | 1.21 | 40.40% | 0.34% | 0.96 |
| x1000 | 5.17e13 | 10% | 843,382,440,507 | 34.7 | 121 | 1 | 0.021 | 4 | 2.30 | 0.068 | 4,801 | 7.43 | 97.80% | 2.97% | 0.78 | 7.14 | 4,801 | 0.74 | 9.30% | 3.05% | 0.96 |
| x1000 | 5.17e13 | 30% | 2,530,147,321,522 | 34.7 | 363 | 1 | 5.0e-3 | 4 | 0.813 | 0.020 | 14,400 | 15.11 | 99.40% | 0.93% | 0.74 | 2.50 | 14,400 | 1.32 | 46.80% | 1.02% | 1.01 |
| x1000 | 5.17e13 | 50% | 4,216,912,202,537 | 34.7 | 604 | 1 | 5.0e-3 | 4 | 0.488 | 0.014 | 23,993 | 14.60 | 99.50% | 0.62% | 0.77 | 1.51 | 23,993 | 1.63 | 62.00% | 0.61% | 1.01 |
| x1000 | 5.17e13 | 90% | 7,590,441,964,567 | 34.7 | 1,088 | 1 | 5.0e-3 | 4 | 0.269 | 0.011 | 43,160 | 15.97 | 99.50% | 0.34% | 1.10 | 0.818 | 43,160 | 2.18 | 76.70% | 0.34% | 0.99 |



## Band step: how fast the cap moves, and hashrate that overstates its wish

`payout-sim --scenario band-step`: a chain from genesis for two days, 50%
industrial, 8 seeds a row. The cap `C` starts at 1 and moves one step a
share: up by `C * u / divisor` when the parent's node wanted heavier shares
than the cap allowed, down by `C / divisor` otherwise. The rule's divisor is
512. In the last two rows of each pool, part of the pool's hashrate mines at
the minimum while every share it finds records the largest wanted
difficulty, to push the cap up.

| Pool | Step divisor | Hashrate overstating | C settles at | C gets within 10% | Home pay over fair, first 6 hours | Home pay over fair, last day |
|---|---|---|---|---|---|---|
| 1% | 128 | 0.00% | 27.93 | 1 h | 0.81 | 1.02 |
| 1% | 512 | 0.00% | 31.31 | 4 h | 0.84 | 1.00 |
| 1% | 2048 | 0.00% | 30.80 | 12 h | 0.88 | 1.01 |
| 1% | 512 | 2.00% | 61.29 | 2 h | 0.80 | 1.02 |
| 1% | 512 | 10.00% | 63.41 | 1 h | 0.81 | 1.01 |
| 10% | 128 | 0.00% | 1.00 | < 1 h | 0.95 | 1.00 |
| 10% | 512 | 0.00% | 1.00 | < 1 h | 0.95 | 1.00 |
| 10% | 2048 | 0.00% | 1.00 | < 1 h | 0.95 | 1.00 |
| 10% | 512 | 2.00% | 48.11 | 17 h | 0.95 | 1.00 |
| 10% | 512 | 10.00% | 62.90 | 1 h | 0.95 | 1.00 |

1. **Where `C` settles does not depend on the step.** At 1% of Bitcoin Cash
   it rests at 28 to 31 for divisors from 128 to 2,048 (42 to 46 with
   ten-minute blocks, whose node default asked for heavier jobs); at 10% at
   1.00, where no miner wants a job above the minimum and the cap is not
   used. The divisor sets only the speed: within 10% of the resting cap
   after 1 hour at 128, 4 hours at 512 and 12 hours at 2,048.
2. **Overstating moves the cap and nothing else.** 2% of the hashrate
   recording the largest wanted difficulty takes `C` to 48 to 61, and 10%
   to 63, the maximum being 64. Home miners are paid 1.00 to 1.02 of fair in
   the last day either way, and the same in the first six hours as on an
   honest chain. The cap only bounds how heavy a job a node may give; an
   honest node still gives each miner the job its own rate calls for.
3. **The first hours of a new chain pay home miners less, whatever the
   step:** 0.80 to 0.88 of fair in the first six hours at 1% of BCH and
   0.95 at 10%, 1.00 to 1.02 in the last day. That is the weak ratio
   settling from `R_MAX`, where weak shares are found faster than shares
   can carry them, not the cap.

## Scale levers: a shorter share period against more weak shares a share (band-weak-levels, weak work, 50% industrial)

| BCH hashrate | Pool | Period | WEAK_FILL | Stale | Weak/s | KB/s | R | Smallest miner with 3 weak shares a month (TH/s) | Bitaxe level | Bitaxe weak/mo | Bitaxe pay/mo | Bitaxe empty months | Bitaxe swing | Bitaxe vs fair | ASIC level | ASIC weak/mo | ASIC pay/mo | ASIC empty months | ASIC swing | ASIC vs fair |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| x1 | 10% | 10 s | 32 | 2.10% | 3.45 | 2.86 | 34.7 | 0.121 | 5 | 29.0 | 24.8 | 0.00% | 0.27 | 1.00 | 5 | 2,410 | 1,967 | 0.00% | 0.03 | 1.00 |
| x1 | 10% | 5 s | 32 | 3.80% | 6.89 | 5.72 | 34.2 | 0.060 | 5 | 58.0 | 24.7 | 0.00% | 0.24 | 1.00 | 5 | 4,829 | 2,046 | 0.00% | 0.03 | 1.00 |
| x1 | 10% | 2 s | 32 | 8.70% | 17.2 | 14.3 | 36.6 | 0.018 | 5 | 194 | 24.9 | 0.00% | 0.21 | 1.00 | 5 | 16,136 | 2,071 | 0.00% | 0.02 | 1.00 |
| x1 | 10% | 1 s | 32 | 15.90% | 34.5 | 28.8 | 31.6 | 9.0e-3 | 5 | 388 | 24.7 | 0.00% | 0.20 | 1.00 | 5 | 32,258 | 2,078 | 0.00% | 0.02 | 1.01 |
| x1 | 10% | 10 s | 64 | 2.10% | 6.91 | 5.63 | 68.7 | 0.061 | 5 | 58.1 | 24.6 | 0.00% | 0.23 | 0.99 | 5 | 4,844 | 2,043 | 0.00% | 0.03 | 1.00 |
| x1 | 10% | 10 s | 160 | 2.10% | 17.3 | 13.9 | 170 | 0.025 | 5 | 146 | 24.9 | 0.00% | 0.22 | 1.00 | 5 | 12,107 | 2,065 | 0.00% | 0.02 | 1.00 |
| x1 | 10% | 10 s | 320 | 2.10% | 34.6 | 27.8 | 339 | 0.012 | 5 | 291 | 24.8 | 0.00% | 0.21 | 1.00 | 5 | 24,175 | 2,067 | 0.00% | 0.02 | 1.00 |
| x1 | 30% | 10 s | 32 | 2.10% | 3.45 | 2.86 | 34.7 | 0.363 | 5 | 9.40 | 24.0 | 0.00% | 0.39 | 0.97 | 5 | 804 | 1,906 | 0.00% | 0.04 | 1.00 |
| x1 | 30% | 5 s | 32 | 3.80% | 6.89 | 5.72 | 34.2 | 0.181 | 5 | 19.4 | 24.7 | 0.00% | 0.30 | 0.99 | 5 | 1,609 | 2,041 | 0.00% | 0.03 | 1.00 |
| x1 | 30% | 2 s | 32 | 8.70% | 17.2 | 14.3 | 32.5 | 0.072 | 5 | 48.4 | 25.0 | 0.00% | 0.24 | 1.01 | 5 | 4,026 | 2,062 | 0.00% | 0.03 | 1.00 |
| x1 | 30% | 1 s | 32 | 15.90% | 34.5 | 28.8 | 30.0 | 0.036 | 5 | 96.7 | 25.1 | 0.00% | 0.22 | 1.01 | 5 | 8,050 | 2,066 | 0.00% | 0.03 | 1.00 |
| x1 | 30% | 10 s | 64 | 2.10% | 6.91 | 5.63 | 68.7 | 0.183 | 5 | 19.4 | 24.7 | 0.00% | 0.31 | 1.00 | 5 | 1,615 | 2,034 | 0.00% | 0.03 | 1.00 |
| x1 | 30% | 10 s | 160 | 2.10% | 17.3 | 13.9 | 170 | 0.074 | 5 | 48.4 | 24.9 | 0.00% | 0.25 | 1.00 | 5 | 4,034 | 2,064 | 0.00% | 0.03 | 1.00 |
| x1 | 30% | 10 s | 320 | 2.10% | 34.6 | 27.8 | 339 | 0.037 | 5 | 96.6 | 25.0 | 0.00% | 0.22 | 1.01 | 5 | 8,053 | 2,066 | 0.00% | 0.02 | 1.00 |
| x10 | 10% | 10 s | 32 | 2.10% | 3.45 | 2.86 | 34.7 | 1.21 | 4 | 2.76 | 4.74 | 10.00% | 0.75 | 0.96 | 5 | 242 | 206 | 0.00% | 0.09 | 1.00 |
| x10 | 10% | 5 s | 32 | 3.80% | 6.89 | 5.72 | 34.2 | 0.603 | 4 | 5.60 | 4.80 | 4.30% | 0.61 | 0.97 | 5 | 483 | 207 | 0.00% | 0.08 | 1.00 |
| x10 | 10% | 2 s | 32 | 8.70% | 17.2 | 14.3 | 32.5 | 0.241 | 4 | 14.3 | 4.86 | 1.20% | 0.51 | 0.98 | 5 | 1,209 | 206 | 0.00% | 0.07 | 1.00 |
| x10 | 10% | 1 s | 32 | 15.90% | 34.5 | 28.8 | 30.0 | 0.120 | 4 | 29.1 | 5.02 | 0.90% | 0.47 | 1.01 | 5 | 2,416 | 207 | 0.00% | 0.07 | 1.00 |
| x10 | 10% | 10 s | 64 | 2.10% | 6.91 | 5.63 | 68.7 | 0.610 | 4 | 5.62 | 4.85 | 4.10% | 0.60 | 0.98 | 5 | 485 | 207 | 0.00% | 0.09 | 1.00 |
| x10 | 10% | 10 s | 160 | 2.10% | 17.3 | 13.9 | 170 | 0.246 | 4 | 14.4 | 4.79 | 1.70% | 0.53 | 0.97 | 5 | 1,211 | 207 | 0.00% | 0.08 | 1.00 |
| x10 | 10% | 10 s | 320 | 2.10% | 34.6 | 27.8 | 339 | 0.124 | 4 | 29.0 | 4.97 | 1.30% | 0.49 | 1.00 | 5 | 2,417 | 207 | 0.00% | 0.07 | 1.00 |
| x10 | 30% | 10 s | 32 | 2.10% | 3.45 | 2.86 | 34.7 | 3.63 | 4 | 0.962 | 2.96 | 40.50% | 1.18 | 0.99 | 5 | 80.6 | 204 | 0.00% | 0.13 | 1.00 |
| x10 | 30% | 5 s | 32 | 3.80% | 6.89 | 5.72 | 34.2 | 1.81 | 4 | 1.86 | 4.71 | 17.50% | 0.85 | 0.95 | 5 | 162 | 207 | 0.00% | 0.10 | 1.00 |
| x10 | 30% | 2 s | 32 | 8.70% | 17.2 | 14.3 | 32.5 | 0.722 | 4 | 4.63 | 4.66 | 5.20% | 0.66 | 0.94 | 5 | 403 | 207 | 0.00% | 0.09 | 1.00 |
| x10 | 30% | 1 s | 32 | 15.90% | 34.5 | 28.8 | 30.0 | 0.360 | 4 | 9.41 | 4.81 | 2.90% | 0.56 | 0.97 | 5 | 805 | 208 | 0.00% | 0.08 | 1.01 |
| x10 | 30% | 10 s | 64 | 2.10% | 6.91 | 5.63 | 68.7 | 1.83 | 4 | 1.86 | 4.68 | 19.20% | 0.88 | 0.95 | 5 | 162 | 207 | 0.00% | 0.11 | 1.00 |
| x10 | 30% | 10 s | 160 | 2.10% | 17.3 | 13.9 | 170 | 0.739 | 4 | 4.64 | 4.77 | 4.90% | 0.65 | 0.96 | 5 | 404 | 208 | 0.00% | 0.08 | 1.00 |
| x10 | 30% | 10 s | 320 | 2.10% | 34.6 | 27.8 | 339 | 0.371 | 4 | 9.42 | 4.78 | 2.60% | 0.59 | 0.96 | 5 | 806 | 207 | 0.00% | 0.08 | 1.00 |
| x100 | 10% | 10 s | 32 | 2.10% | 3.45 | 2.86 | 34.7 | 12.1 | 1 | 0.284 | 0.877 | 75.40% | 2.06 | 0.98 | 5 | 24.2 | 20.9 | 0.00% | 0.30 | 1.01 |
| x100 | 10% | 5 s | 32 | 3.80% | 6.89 | 5.72 | 34.2 | 6.03 | 1 | 0.573 | 1.69 | 57.60% | 1.47 | 0.95 | 5 | 48.4 | 20.4 | 0.00% | 0.26 | 0.99 |
| x100 | 10% | 2 s | 32 | 8.70% | 17.2 | 14.3 | 32.5 | 2.41 | 1 | 1.41 | 3.68 | 26.00% | 0.97 | 0.93 | 5 | 121 | 20.6 | 0.00% | 0.23 | 1.00 |
| x100 | 10% | 1 s | 32 | 15.90% | 34.5 | 28.8 | 30.0 | 1.20 | 1 | 2.76 | 3.76 | 13.10% | 0.80 | 0.95 | 5 | 242 | 20.5 | 0.00% | 0.23 | 0.99 |
| x100 | 10% | 10 s | 64 | 2.10% | 6.91 | 5.63 | 68.7 | 6.10 | 1 | 0.576 | 1.76 | 57.30% | 1.52 | 1.00 | 5 | 48.5 | 20.6 | 0.00% | 0.26 | 1.00 |
| x100 | 10% | 10 s | 160 | 2.10% | 17.3 | 13.9 | 170 | 2.46 | 1 | 1.42 | 3.78 | 26.50% | 0.97 | 0.96 | 5 | 121 | 20.7 | 0.00% | 0.23 | 1.00 |
| x100 | 10% | 10 s | 320 | 2.10% | 34.6 | 27.8 | 339 | 1.24 | 1 | 2.76 | 3.71 | 12.30% | 0.78 | 0.94 | 5 | 242 | 20.8 | 0.00% | 0.24 | 1.01 |
| x100 | 30% | 10 s | 32 | 2.10% | 3.45 | 2.86 | 34.7 | 36.3 | 1 | 0.089 | 0.285 | 91.10% | 3.74 | 0.97 | 5 | 7.81 | 19.7 | 0.20% | 0.43 | 0.96 |
| x100 | 30% | 5 s | 32 | 3.80% | 6.89 | 5.72 | 34.2 | 18.1 | 1 | 0.177 | 0.554 | 84.20% | 2.63 | 0.93 | 5 | 16.0 | 20.5 | 0.00% | 0.34 | 0.99 |
| x100 | 30% | 2 s | 32 | 8.70% | 17.2 | 14.3 | 32.5 | 7.22 | 1 | 0.488 | 1.51 | 62.40% | 1.70 | 1.01 | 5 | 40.4 | 20.6 | 0.00% | 0.27 | 1.00 |
| x100 | 30% | 1 s | 32 | 15.90% | 34.5 | 28.8 | 30.0 | 3.60 | 1 | 0.962 | 2.87 | 40.00% | 1.17 | 0.96 | 5 | 80.7 | 20.9 | 0.00% | 0.26 | 1.01 |
| x100 | 30% | 10 s | 64 | 2.10% | 6.91 | 5.63 | 68.7 | 18.3 | 1 | 0.181 | 0.610 | 83.30% | 2.60 | 1.04 | 5 | 16.0 | 20.2 | 0.00% | 0.33 | 0.98 |
| x100 | 30% | 10 s | 160 | 2.10% | 17.3 | 13.9 | 170 | 7.39 | 1 | 0.485 | 1.51 | 62.30% | 1.62 | 1.03 | 5 | 40.4 | 20.5 | 0.00% | 0.27 | 0.99 |
| x100 | 30% | 10 s | 320 | 2.10% | 34.6 | 27.8 | 339 | 3.71 | 1 | 0.964 | 2.95 | 39.10% | 1.15 | 1.02 | 5 | 80.6 | 20.6 | 0.00% | 0.24 | 1.00 |
| x270 | 10% | 10 s | 32 | 2.10% | 3.45 | 2.86 | 34.7 | 32.6 | 1 | 0.097 | 0.309 | 90.70% | 3.55 | 0.94 | 5 | 8.68 | 7.58 | 0.50% | 0.49 | 0.99 |
| x270 | 10% | 5 s | 32 | 3.80% | 6.89 | 5.72 | 34.2 | 16.3 | 1 | 0.207 | 0.663 | 81.80% | 2.49 | 1.00 | 5 | 17.9 | 7.52 | 0.40% | 0.42 | 0.98 |
| x270 | 10% | 2 s | 32 | 8.70% | 17.2 | 14.3 | 32.5 | 6.50 | 1 | 0.532 | 1.42 | 59.50% | 1.62 | 0.97 | 5 | 44.9 | 7.63 | 0.00% | 0.37 | 1.00 |
| x270 | 10% | 1 s | 32 | 15.90% | 34.5 | 28.8 | 30.0 | 3.24 | 1 | 1.07 | 1.32 | 47.40% | 1.35 | 0.90 | 5 | 89.6 | 7.77 | 0.00% | 0.37 | 1.02 |
| x270 | 10% | 10 s | 64 | 2.10% | 6.91 | 5.63 | 68.7 | 16.5 | 1 | 0.209 | 0.668 | 81.30% | 2.45 | 1.02 | 5 | 17.9 | 7.62 | 0.60% | 0.43 | 1.00 |
| x270 | 10% | 10 s | 160 | 2.10% | 17.3 | 13.9 | 170 | 6.65 | 1 | 0.535 | 1.52 | 61.10% | 1.60 | 1.04 | 5 | 44.8 | 7.68 | 0.00% | 0.40 | 1.00 |
| x270 | 10% | 10 s | 320 | 2.10% | 34.6 | 27.8 | 339 | 3.34 | 1 | 1.06 | 1.47 | 44.90% | 1.32 | 1.01 | 5 | 89.5 | 7.67 | 0.00% | 0.38 | 1.00 |
| x270 | 30% | 10 s | 32 | 2.10% | 3.45 | 2.86 | 34.7 | 97.9 | 1 | 0.028 | 0.114 | 97.00% | 6.18 | 1.04 | 5 | 2.83 | 7.31 | 6.90% | 0.70 | 0.96 |
| x270 | 30% | 5 s | 32 | 3.80% | 6.89 | 5.72 | 34.2 | 48.8 | 1 | 0.065 | 0.200 | 93.90% | 4.50 | 0.91 | 5 | 5.79 | 7.50 | 1.30% | 0.53 | 0.98 |
| x270 | 30% | 2 s | 32 | 8.70% | 17.2 | 14.3 | 32.5 | 19.5 | 1 | 0.169 | 0.557 | 84.00% | 2.64 | 1.01 | 5 | 14.7 | 7.60 | 0.20% | 0.45 | 0.99 |
| x270 | 30% | 1 s | 32 | 15.90% | 34.5 | 28.8 | 30.0 | 9.72 | 1 | 0.360 | 1.15 | 69.60% | 1.85 | 1.04 | 5 | 29.9 | 7.60 | 0.10% | 0.40 | 0.99 |
| x270 | 30% | 10 s | 64 | 2.10% | 6.91 | 5.63 | 68.7 | 49.4 | 1 | 0.062 | 0.192 | 94.00% | 4.48 | 0.89 | 5 | 5.79 | 7.32 | 1.20% | 0.53 | 0.96 |
| x270 | 30% | 10 s | 160 | 2.10% | 17.3 | 13.9 | 170 | 19.9 | 1 | 0.162 | 0.487 | 85.60% | 2.86 | 0.90 | 5 | 14.7 | 7.42 | 0.10% | 0.44 | 0.97 |
| x270 | 30% | 10 s | 320 | 2.10% | 34.6 | 27.8 | 339 | 10.0 | 1 | 0.353 | 1.11 | 70.80% | 2.03 | 1.04 | 5 | 29.8 | 7.59 | 0.00% | 0.41 | 0.99 |

## Window cap: 14 days against 30 and 60 in small pools (band-weak-levels, weak work, medium miners, levels as a node sets them)

| Pool | PH/s | Blocks a month | Cap | Window | Blocks a share is paid by | Shares paid by no block | Paid after stopping, at most | Bitaxe level | Bitaxe pay/mo | Bitaxe empty months | Bitaxe swing | Bitaxe vs fair | ASIC level | ASIC pay/mo | ASIC empty months | ASIC swing | ASIC vs fair |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 0.01% | 0.370 | 0.432 | 14 days | 14.0 days | 0.20 | 81.74% | 14.0 days | 1 | 0.451 | 63.00% | 1.46 | 1.04 | 1 | 0.451 | 63.00% | 1.46 | 1.04 |
| 0.01% | 0.370 | 0.432 | 30 days | 30.0 days | 0.43 | 64.92% | 30.0 days | 1 | 0.427 | 66.30% | 1.59 | 0.99 | 1 | 0.427 | 66.30% | 1.59 | 0.99 |
| 0.01% | 0.370 | 0.432 | 60 days | 60.0 days | 0.86 | 42.15% | 60.0 days | 1 | 0.440 | 64.80% | 1.53 | 1.02 | 1 | 0.440 | 64.80% | 1.53 | 1.02 |
| 0.02% | 0.740 | 0.864 | 14 days | 14.0 days | 0.40 | 66.82% | 14.0 days | 1 | 0.935 | 39.60% | 1.02 | 1.08 | 1 | 0.935 | 39.60% | 1.02 | 1.08 |
| 0.02% | 0.740 | 0.864 | 30 days | 30.0 days | 0.86 | 42.15% | 30.0 days | 1 | 0.901 | 38.80% | 0.99 | 1.04 | 1 | 0.901 | 38.80% | 0.99 | 1.04 |
| 0.02% | 0.740 | 0.864 | 60 days | 60.0 days | 1.73 | 17.76% | 60.0 days | 1 | 0.861 | 41.60% | 1.05 | 1.00 | 1 | 0.861 | 41.60% | 1.05 | 1.00 |
| 0.05% | 1.85 | 2.16 | 14 days | 14.0 days | 1.01 | 36.49% | 14.0 days | 1 | 2.19 | 11.20% | 0.68 | 1.02 | 1 | 2.19 | 11.20% | 0.68 | 1.01 |
| 0.05% | 1.85 | 2.16 | 30 days | 30.0 days | 2.16 | 11.53% | 30.0 days | 1 | 2.20 | 10.00% | 0.66 | 1.02 | 1 | 2.20 | 10.00% | 0.66 | 1.02 |
| 0.05% | 1.85 | 2.16 | 60 days | 41.7 days | 3.00 | 4.98% | 41.7 days | 1 | 2.17 | 11.10% | 0.67 | 1.00 | 1 | 2.17 | 11.10% | 0.68 | 1.00 |
| 0.10% | 3.70 | 4.32 | 14 days | 14.0 days | 2.02 | 13.32% | 14.0 days | 5 | 4.36 | 0.80% | 0.48 | 1.02 | 5 | 4.36 | 0.80% | 0.48 | 1.01 |
| 0.10% | 3.70 | 4.32 | 30 days | 20.8 days | 3.00 | 4.98% | 20.8 days | 5 | 4.35 | 0.90% | 0.48 | 1.01 | 5 | 4.35 | 0.90% | 0.47 | 1.01 |
| 0.10% | 3.70 | 4.32 | 60 days | 20.8 days | 3.00 | 4.98% | 20.8 days | 5 | 4.35 | 0.90% | 0.48 | 1.01 | 5 | 4.35 | 0.90% | 0.47 | 1.01 |
| 0.20% | 7.40 | 8.64 | 14 days | 10.4 days | 3.00 | 4.98% | 10.4 days | 5 | 8.62 | 0.00% | 0.34 | 1.01 | 5 | 8.62 | 0.00% | 0.34 | 1.00 |
| 0.20% | 7.40 | 8.64 | 30 days | 10.4 days | 3.00 | 4.98% | 10.4 days | 5 | 8.62 | 0.00% | 0.34 | 1.01 | 5 | 8.62 | 0.00% | 0.34 | 1.00 |
| 0.20% | 7.40 | 8.64 | 60 days | 10.4 days | 3.00 | 4.98% | 10.4 days | 5 | 8.62 | 0.00% | 0.34 | 1.01 | 5 | 8.62 | 0.00% | 0.34 | 1.00 |
| 0.50% | 18.5 | 21.6 | 14 days | 4.2 days | 3.00 | 4.98% | 4.2 days | 5 | 21.6 | 0.00% | 0.21 | 1.01 | 5 | 21.6 | 0.00% | 0.22 | 0.99 |
| 0.50% | 18.5 | 21.6 | 30 days | 4.2 days | 3.00 | 4.98% | 4.2 days | 5 | 21.6 | 0.00% | 0.21 | 1.01 | 5 | 21.6 | 0.00% | 0.22 | 0.99 |
| 0.50% | 18.5 | 21.6 | 60 days | 4.2 days | 3.00 | 4.98% | 4.2 days | 5 | 21.6 | 0.00% | 0.21 | 1.01 | 5 | 21.6 | 0.00% | 0.22 | 0.99 |
| 1% | 37.0 | 43.2 | 14 days | 2.1 days | 3.00 | 4.98% | 2.1 days | 5 | 24.9 | 0.00% | 0.19 | 1.00 | 5 | 43.2 | 0.00% | 0.15 | 1.00 |
| 1% | 37.0 | 43.2 | 30 days | 2.1 days | 3.00 | 4.98% | 2.1 days | 5 | 24.9 | 0.00% | 0.19 | 1.00 | 5 | 43.2 | 0.00% | 0.15 | 1.00 |
| 1% | 37.0 | 43.2 | 60 days | 2.1 days | 3.00 | 4.98% | 2.1 days | 5 | 24.9 | 0.00% | 0.19 | 1.00 | 5 | 43.2 | 0.00% | 0.15 | 1.00 |

## Storage: what a node keeps, by payees, payout level and window cap (arithmetic, no fees)

| Payees in the window | Payout level | Coinbase outputs | In the payout tree | Coinbase KB | GB kept, 14-day cap (28 days) | GB kept, 30-day cap (60 days) | GB a year if never pruned | Payout tree MB a found block, kept for ever |
|---|---|---|---|---|---|---|---|---|
| 10 | 5 (17,472 sat) | 10 | 0 | 0.707 | 0.171 | 0.367 | 2.23 | 0 |
| 10 | 1 (1,092 sat) | 10 | 0 | 0.707 | 0.171 | 0.367 | 2.23 | 0 |
| 100 | 5 (17,472 sat) | 100 | 0 | 3.77 | 0.913 | 1.95 | 11.9 | 0 |
| 100 | 1 (1,092 sat) | 100 | 0 | 3.77 | 0.913 | 1.95 | 11.9 | 0 |
| 1,000 | 5 (17,472 sat) | 1,000 | 0 | 34.4 | 8.33 | 17.8 | 108 | 0 |
| 1,000 | 1 (1,092 sat) | 1,000 | 0 | 34.4 | 8.33 | 17.8 | 108 | 0 |
| 10,000 | 5 (17,472 sat) | 10,000 | 0 | 340 | 82.5 | 177 | 1,073 | 0 |
| 10,000 | 1 (1,092 sat) | 10,000 | 0 | 340 | 82.5 | 177 | 1,073 | 0 |
| 100,000 | 5 (17,472 sat) | 17,885 | 0 | 608 | 148 | 316 | 1,919 | 0 |
| 100,000 | 1 (1,092 sat) | 100,000 | 70,600 | 1,000 | 242 | 519 | 3,153 | 3.18 |
| 1,000,000 | 5 (17,472 sat) | 17,885 | 0 | 608 | 148 | 316 | 1,919 | 0 |
| 1,000,000 | 1 (1,092 sat) | 286,172 | 256,772 | 1,000 | 242 | 519 | 3,153 | 11.6 |

| Stored | GB for two payout depths, 14-day cap | GB for two payout depths, 30-day cap | GB for one payout depth, 14-day cap | GB for one payout depth, 30-day cap | GB a year |
|---|---|---|---|---|---|
| 32 weak shares a share | 6.21 | 13.3 | 3.10 | 6.64 | 80.7 |
| 320 weak shares a share | 62.1 | 133 | 31.0 | 66.4 | 807 |
| A coinbase stored without its payouts | 0.089 | 0.190 | 0.044 | 0.095 | 1.16 |
| Share headers, kept for ever | - | - | - | - | 3.15 |

## Proposal: weak shares that follow the asks, where they settle (band-weak-levels, weak work, 50% industrial)

| BCH hashrate | Pool | Rule | Weak a share | R | Asking | Weak/s | KB/s | Smallest miner with 3 weak shares a month (TH/s) | Bitaxe weak/mo | Bitaxe pay/mo | Bitaxe empty months | Bitaxe swing | Bitaxe vs fair | ASIC weak/mo | ASIC pay/mo | ASIC empty months | ASIC swing | ASIC vs fair |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| x1 | 10% | today | 34.5 | 36.8 | 0.00% | 3.45 | 2.86 | 0.091 | 38.6 | 24.7 | 0.00% | 0.25 | 0.99 | 3,209 | 431 | 0.00% | 0.05 | 1.00 |
| x1 | 10% | asks | 46.3 | 49.3 | 0.00% | 4.63 | 3.81 | 0.068 | 52.0 | 25.1 | 0.00% | 0.25 | 1.01 | 4,327 | 433 | 0.00% | 0.05 | 1.00 |
| x1 | 10% | fill 320 | 346 | 361 | 0.00% | 34.6 | 27.8 | 9.3e-3 | 388 | 25.2 | 0.00% | 0.21 | 1.01 | 32,299 | 433 | 0.00% | 0.05 | 1.00 |
| x1 | 30% | today | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 0.363 | 9.40 | 24.0 | 0.00% | 0.38 | 0.98 | 804 | 1,041 | 0.00% | 0.05 | 1.00 |
| x1 | 30% | asks | 56.8 | 56.6 | 6.00% | 5.68 | 4.65 | 0.222 | 15.7 | 24.6 | 0.00% | 0.32 | 0.99 | 1,327 | 1,151 | 0.00% | 0.04 | 1.00 |
| x1 | 30% | fill 320 | 346 | 339 | 0.00% | 34.6 | 27.8 | 0.037 | 96.6 | 24.7 | 0.00% | 0.22 | 0.99 | 8,053 | 1,288 | 0.00% | 0.03 | 1.00 |
| x10 | 10% | today | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 1.21 | 2.76 | 4.73 | 9.30% | 0.76 | 0.96 | 242 | 355 | 0.00% | 0.09 | 1.00 |
| x10 | 10% | asks | 189 | 186 | 26.62% | 18.9 | 15.3 | 0.225 | 15.7 | 4.87 | 1.40% | 0.51 | 0.98 | 1,325 | 431 | 0.00% | 0.06 | 1.00 |
| x10 | 10% | fill 320 | 346 | 339 | 0.00% | 34.6 | 27.8 | 0.124 | 29.0 | 4.96 | 1.30% | 0.47 | 1.00 | 2,417 | 431 | 0.00% | 0.05 | 1.00 |
| x10 | 30% | today | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 3.63 | 0.962 | 2.95 | 41.00% | 1.18 | 0.99 | 80.6 | 191 | 0.00% | 0.13 | 1.00 |
| x10 | 30% | asks | 344 | 337 | 49.99% | 34.4 | 27.6 | 0.373 | 9.38 | 4.91 | 2.70% | 0.56 | 0.99 | 802 | 206 | 0.00% | 0.08 | 1.00 |
| x10 | 30% | fill 320 | 346 | 339 | 0.00% | 34.6 | 27.8 | 0.371 | 9.42 | 4.82 | 2.40% | 0.58 | 0.97 | 806 | 205 | 0.00% | 0.08 | 1.00 |
| x100 | 10% | today | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 12.1 | 0.284 | 0.846 | 75.60% | 2.10 | 0.95 | 24.2 | 20.9 | 0.00% | 0.30 | 1.01 |
| x100 | 10% | asks | 344 | 338 | 49.99% | 34.4 | 27.6 | 1.24 | 2.75 | 3.75 | 12.90% | 0.81 | 0.95 | 241 | 20.8 | 0.00% | 0.22 | 1.00 |
| x100 | 10% | fill 320 | 346 | 339 | 0.00% | 34.6 | 27.8 | 1.24 | 2.76 | 3.59 | 14.30% | 0.83 | 0.91 | 242 | 20.6 | 0.00% | 0.24 | 1.00 |
| x100 | 30% | today | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 36.3 | 0.089 | 0.259 | 91.20% | 3.78 | 0.87 | 7.81 | 19.7 | 0.20% | 0.43 | 0.97 |
| x100 | 30% | asks | 344 | 338 | 50.00% | 34.4 | 27.6 | 3.72 | 0.959 | 2.86 | 40.00% | 1.23 | 0.99 | 80.5 | 20.7 | 0.00% | 0.24 | 1.00 |
| x100 | 30% | fill 320 | 346 | 339 | 0.00% | 34.6 | 27.8 | 3.71 | 0.964 | 2.91 | 39.70% | 1.20 | 1.01 | 80.6 | 20.6 | 0.00% | 0.24 | 1.00 |
| x270 | 10% | today | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 32.6 | 0.097 | 0.304 | 90.70% | 3.64 | 0.92 | 8.68 | 7.48 | 0.30% | 0.48 | 0.98 |
| x270 | 10% | asks | 344 | 338 | 49.99% | 34.4 | 27.6 | 3.35 | 1.06 | 1.48 | 44.70% | 1.34 | 1.02 | 89.2 | 7.59 | 0.00% | 0.37 | 0.99 |
| x270 | 10% | fill 320 | 346 | 339 | 0.00% | 34.6 | 27.8 | 3.34 | 1.06 | 1.51 | 45.30% | 1.29 | 1.04 | 89.5 | 7.61 | 0.00% | 0.38 | 0.99 |
| x270 | 30% | today | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 97.9 | 0.028 | 0.091 | 97.00% | 6.42 | 0.84 | 2.83 | 7.22 | 7.20% | 0.69 | 0.95 |
| x270 | 30% | asks | 344 | 337 | 50.00% | 34.4 | 27.6 | 10.1 | 0.351 | 1.05 | 72.00% | 2.04 | 0.99 | 29.7 | 7.60 | 0.00% | 0.41 | 0.99 |
| x270 | 30% | fill 320 | 346 | 339 | 0.00% | 34.6 | 27.8 | 10.0 | 0.353 | 1.02 | 71.70% | 1.93 | 0.96 | 29.8 | 7.48 | 0.10% | 0.41 | 0.98 |



## Proposal: the asks in every miner mix, at ten times today's hashrate (band-weak-levels, weak work)

| Mix | Pool | Rule | Weak a share | R | Asking | Weak/s | KB/s | Smallest miner with 3 weak shares a month (TH/s) | Bitaxe weak/mo | Bitaxe pay/mo | Bitaxe empty months | Bitaxe swing | Bitaxe vs fair | ASIC weak/mo | ASIC pay/mo | ASIC empty months | ASIC swing | ASIC vs fair |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| home | 10% | today | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 1.21 | 2.76 | 4.62 | 10.30% | 0.75 | 0.93 | 242 | 355 | 0.00% | 0.09 | 1.00 |
| home | 10% | asks | 189 | 186 | 52.82% | 18.9 | 15.2 | 0.225 | 15.7 | 4.79 | 1.50% | 0.52 | 0.97 | 1,326 | 431 | 0.00% | 0.06 | 1.00 |
| home | 30% | today | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 3.63 | 0.962 | 2.95 | 41.00% | 1.18 | 0.99 | 80.6 | 226 | 0.00% | 0.13 | 1.00 |
| home | 30% | asks | 345 | 338 | 100.00% | 34.5 | 27.7 | 0.372 | 9.39 | 4.87 | 2.10% | 0.55 | 0.98 | 804 | 1,094 | 0.00% | 0.05 | 1.00 |
| 50% industrial | 10% | today | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 1.21 | 2.76 | 4.73 | 9.30% | 0.76 | 0.96 | 242 | 355 | 0.00% | 0.09 | 1.00 |
| 50% industrial | 10% | asks | 189 | 186 | 26.62% | 18.9 | 15.3 | 0.225 | 15.7 | 4.87 | 1.40% | 0.51 | 0.98 | 1,325 | 431 | 0.00% | 0.06 | 1.00 |
| 50% industrial | 30% | today | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 3.63 | 0.962 | 2.95 | 41.00% | 1.18 | 0.99 | 80.6 | 191 | 0.00% | 0.13 | 1.00 |
| 50% industrial | 30% | asks | 344 | 337 | 49.99% | 34.4 | 27.6 | 0.373 | 9.38 | 4.91 | 2.70% | 0.56 | 0.99 | 802 | 206 | 0.00% | 0.08 | 1.00 |
| 90% industrial | 10% | today | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 1.21 | 2.76 | 4.72 | 9.80% | 0.74 | 0.95 | 242 | 355 | 0.00% | 0.09 | 1.00 |
| 90% industrial | 10% | asks | 189 | 186 | 5.34% | 18.9 | 15.2 | 0.225 | 15.7 | 4.82 | 1.40% | 0.53 | 0.97 | 1,325 | 431 | 0.00% | 0.06 | 1.00 |
| 90% industrial | 30% | today | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 3.63 | 0.962 | 2.95 | 41.00% | 1.18 | 0.99 | 80.6 | 190 | 0.00% | 0.13 | 1.00 |
| 90% industrial | 30% | asks | 334 | 327 | 10.00% | 33.4 | 26.8 | 0.385 | 9.11 | 4.91 | 2.80% | 0.58 | 0.99 | 778 | 207 | 0.00% | 0.08 | 1.00 |
| 99% industrial | 10% | today | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 1.21 | 2.76 | 4.78 | 9.00% | 0.74 | 0.97 | 242 | 355 | 0.00% | 0.09 | 1.00 |
| 99% industrial | 10% | asks | 81.7 | 81.1 | 1.00% | 8.17 | 6.64 | 0.517 | 6.74 | 4.97 | 2.60% | 0.57 | 1.00 | 573 | 424 | 0.00% | 0.07 | 1.00 |
| 99% industrial | 30% | today | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 3.63 | 0.962 | 2.95 | 41.00% | 1.18 | 0.99 | 80.6 | 191 | 0.00% | 0.13 | 1.00 |
| 99% industrial | 30% | asks | 81.5 | 80.7 | 1.00% | 8.15 | 6.62 | 1.56 | 2.20 | 4.67 | 14.00% | 0.79 | 0.95 | 190 | 205 | 0.00% | 0.10 | 1.00 |
| zipf | 10% | today | 34.5 | 38.1 | 0.00% | 3.45 | 2.86 | 0.907 | 3.70 | 4.68 | 6.60% | 0.69 | 0.95 | 322 | 202 | 0.00% | 0.09 | 1.00 |
| zipf | 10% | asks | 115 | 126 | 1.42% | 11.5 | 9.34 | 0.275 | 12.8 | 4.86 | 2.30% | 0.56 | 0.98 | 1,081 | 207 | 0.00% | 0.08 | 1.00 |
| zipf | 30% | today | 34.5 | 36.8 | 0.00% | 3.45 | 2.86 | 3.15 | 1.10 | 3.39 | 33.90% | 1.07 | 0.99 | 92.7 | 193 | 0.00% | 0.13 | 1.00 |
| zipf | 30% | asks | 299 | 312 | 3.93% | 29.9 | 24.0 | 0.372 | 9.39 | 4.86 | 2.40% | 0.56 | 0.98 | 803 | 207 | 0.00% | 0.08 | 1.00 |
| medium | 10% | today | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 1.21 | 2.76 | 4.72 | 9.80% | 0.74 | 0.95 | 242 | 197 | 0.00% | 0.10 | 1.00 |
| medium | 10% | asks | 190 | 186 | 5.36% | 19.0 | 15.3 | 0.225 | 15.7 | 4.88 | 1.40% | 0.51 | 0.98 | 1,325 | 206 | 0.00% | 0.08 | 1.00 |
| medium | 30% | today | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 3.63 | 0.962 | 2.95 | 41.00% | 1.18 | 0.99 | 80.6 | 191 | 0.00% | 0.13 | 1.00 |
| medium | 30% | asks | 334 | 328 | 10.00% | 33.4 | 26.8 | 0.384 | 9.12 | 4.93 | 1.60% | 0.56 | 0.99 | 779 | 207 | 0.00% | 0.08 | 1.00 |
| medium, few big | 10% | today | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 1.21 | 2.76 | 4.72 | 9.80% | 0.74 | 0.95 | 242 | 197 | 0.00% | 0.10 | 1.00 |
| medium, few big | 10% | asks | 189 | 186 | 5.33% | 18.9 | 15.2 | 0.225 | 15.7 | 4.84 | 1.60% | 0.53 | 0.98 | 1,325 | 206 | 0.00% | 0.07 | 1.00 |
| medium, few big | 30% | today | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 3.63 | 0.962 | 2.95 | 41.00% | 1.18 | 0.99 | 80.6 | 191 | 0.00% | 0.13 | 1.00 |
| medium, few big | 30% | asks | 334 | 329 | 10.00% | 33.4 | 26.8 | 0.383 | 9.15 | 4.93 | 2.70% | 0.56 | 0.99 | 782 | 207 | 0.00% | 0.08 | 1.00 |



## Proposal: the asks' constants, each varied alone (pool 30% of ten times today's hashrate)

| Mix | WEAK_ASK_DIVISOR | Node default (weak shares a month) | WEAK_FILL_MAX | Weak a share | R | Asking | Weak/s | KB/s | Smallest miner with 3 weak shares a month (TH/s) | Bitaxe weak/mo | Bitaxe pay/mo | Bitaxe empty months | Bitaxe swing | Bitaxe vs fair | ASIC weak/mo | ASIC pay/mo | ASIC empty months | ASIC swing | ASIC vs fair |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 50% industrial | 16 | 16 | 320 | 344 | 337 | 49.99% | 34.4 | 27.6 | 0.373 | 9.38 | 4.91 | 2.70% | 0.56 | 0.99 | 802 | 206 | 0.00% | 0.08 | 1.00 |
| 50% industrial | 64 | 16 | 320 | 344 | 337 | 49.99% | 34.4 | 27.6 | 0.373 | 9.38 | 4.91 | 2.70% | 0.56 | 0.99 | 802 | 206 | 0.00% | 0.08 | 1.00 |
| 50% industrial | 256 | 16 | 320 | 344 | 337 | 49.99% | 34.4 | 27.6 | 0.373 | 9.38 | 4.91 | 2.70% | 0.56 | 0.99 | 802 | 206 | 0.00% | 0.08 | 1.00 |
| 50% industrial | 64 | 4 | 320 | 142 | 140 | 21.42% | 14.2 | 11.4 | 0.899 | 3.81 | 4.67 | 6.30% | 0.66 | 0.94 | 332 | 207 | 0.00% | 0.09 | 1.00 |
| 50% industrial | 64 | 16 | 320 | 344 | 337 | 49.99% | 34.4 | 27.6 | 0.373 | 9.38 | 4.91 | 2.70% | 0.56 | 0.99 | 802 | 206 | 0.00% | 0.08 | 1.00 |
| 50% industrial | 64 | 64 | 320 | 344 | 337 | 49.99% | 34.4 | 27.6 | 0.373 | 9.38 | 4.91 | 2.70% | 0.56 | 0.99 | 802 | 206 | 0.00% | 0.08 | 1.00 |
| 50% industrial | 64 | 16 | 64 | 69.3 | 68.8 | 50.00% | 6.93 | 5.65 | 1.83 | 1.86 | 4.70 | 17.50% | 0.85 | 0.95 | 162 | 203 | 0.00% | 0.11 | 1.00 |
| 50% industrial | 64 | 16 | 160 | 173 | 171 | 49.99% | 17.3 | 14.0 | 0.738 | 4.64 | 4.74 | 5.20% | 0.64 | 0.95 | 405 | 206 | 0.00% | 0.09 | 1.00 |
| 50% industrial | 64 | 16 | 320 | 344 | 337 | 49.99% | 34.4 | 27.6 | 0.373 | 9.38 | 4.91 | 2.70% | 0.56 | 0.99 | 802 | 206 | 0.00% | 0.08 | 1.00 |
| zipf | 16 | 16 | 320 | 220 | 230 | 7.02% | 22.0 | 17.7 | 0.504 | 6.95 | 4.73 | 3.30% | 0.60 | 0.95 | 592 | 207 | 0.00% | 0.08 | 1.00 |
| zipf | 64 | 16 | 320 | 299 | 312 | 3.93% | 29.9 | 24.0 | 0.372 | 9.39 | 4.86 | 2.40% | 0.56 | 0.98 | 803 | 207 | 0.00% | 0.08 | 1.00 |
| zipf | 256 | 16 | 320 | 312 | 326 | 3.80% | 31.2 | 25.1 | 0.356 | 9.83 | 4.72 | 1.50% | 0.55 | 0.95 | 839 | 207 | 0.00% | 0.08 | 1.00 |
| zipf | 64 | 4 | 320 | 108 | 114 | 1.22% | 10.8 | 8.78 | 1.02 | 3.35 | 4.85 | 7.60% | 0.71 | 0.98 | 292 | 206 | 0.00% | 0.09 | 1.00 |
| zipf | 64 | 16 | 320 | 299 | 312 | 3.93% | 29.9 | 24.0 | 0.372 | 9.39 | 4.86 | 2.40% | 0.56 | 0.98 | 803 | 207 | 0.00% | 0.08 | 1.00 |
| zipf | 64 | 64 | 320 | 338 | 352 | 14.71% | 33.8 | 27.1 | 0.330 | 10.6 | 4.88 | 1.30% | 0.54 | 0.98 | 908 | 207 | 0.00% | 0.08 | 1.00 |
| zipf | 64 | 16 | 64 | 69.3 | 72.9 | 16.31% | 6.93 | 5.65 | 1.59 | 2.15 | 4.75 | 15.50% | 0.82 | 0.96 | 186 | 205 | 0.00% | 0.10 | 1.00 |
| zipf | 64 | 16 | 160 | 172 | 180 | 8.82% | 17.2 | 13.9 | 0.644 | 5.34 | 4.81 | 4.60% | 0.65 | 0.97 | 464 | 207 | 0.00% | 0.08 | 1.00 |
| zipf | 64 | 16 | 320 | 299 | 312 | 3.93% | 29.9 | 24.0 | 0.372 | 9.39 | 4.86 | 2.40% | 0.56 | 0.98 | 803 | 207 | 0.00% | 0.08 | 1.00 |



## Proposal: attacks on the asks (band-weak-levels, weak work, 50% industrial)

| Attack | Its part | BCH hashrate, pool | Weak a share | R | Asking | Weak/s | KB/s | Smallest miner with 3 weak shares a month (TH/s) | Bitaxe weak/mo | Bitaxe pay/mo | Bitaxe empty months | Bitaxe swing | Bitaxe vs fair | ASIC weak/mo | ASIC pay/mo | ASIC empty months | ASIC swing | ASIC vs fair |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| asks whatever it finds | 0.00% | x1, 10% | 46.3 | 49.3 | 0.00% | 4.63 | 3.81 | 0.068 | 52.0 | 25.1 | 0.00% | 0.25 | 1.01 | 4,327 | 433 | 0.00% | 0.05 | 1.00 |
| asks whatever it finds | 1.00% | x1, 10% | 82.1 | 86.5 | 1.00% | 8.21 | 6.67 | 0.039 | 92.0 | 24.8 | 0.00% | 0.23 | 1.00 | 7,661 | 432 | 0.00% | 0.05 | 1.00 |
| asks whatever it finds | 2.00% | x1, 10% | 221 | 232 | 2.00% | 22.1 | 17.8 | 0.014 | 249 | 25.0 | 0.00% | 0.21 | 1.01 | 20,720 | 432 | 0.00% | 0.05 | 1.00 |
| asks whatever it finds | 5.00% | x1, 10% | 316 | 330 | 5.00% | 31.6 | 25.4 | 0.010 | 355 | 24.8 | 0.00% | 0.21 | 1.00 | 29,517 | 431 | 0.00% | 0.05 | 1.00 |
| asks whatever it finds | 25.00% | x1, 10% | 341 | 357 | 25.00% | 34.1 | 27.4 | 9.4e-3 | 383 | 24.9 | 0.00% | 0.21 | 1.00 | 31,878 | 432 | 0.00% | 0.05 | 1.00 |
| none, today's rule | - | x10, 30% | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 3.63 | 0.962 | 2.95 | 41.00% | 1.18 | 0.99 | 80.6 | 191 | 0.00% | 0.13 | 1.00 |
| none | - | x10, 30% | 344 | 337 | 49.99% | 34.4 | 27.6 | 0.373 | 9.38 | 4.91 | 2.70% | 0.56 | 0.99 | 802 | 206 | 0.00% | 0.08 | 1.00 |
| never asks | 50.00% | x10, 30% | 341 | 334 | 25.00% | 34.1 | 27.4 | 0.377 | 9.29 | 4.75 | 2.70% | 0.56 | 0.96 | 795 | 206 | 0.00% | 0.08 | 1.00 |
| never asks | 90.00% | x10, 30% | 316 | 311 | 5.00% | 31.6 | 25.4 | 0.404 | 8.64 | 4.75 | 2.10% | 0.56 | 0.96 | 740 | 206 | 0.00% | 0.08 | 1.00 |
| never asks | 97.00% | x10, 30% | 143 | 140 | 1.50% | 14.3 | 11.5 | 0.895 | 3.83 | 4.81 | 7.10% | 0.70 | 0.97 | 333 | 206 | 0.00% | 0.09 | 1.00 |
| never asks | 99.00% | x10, 30% | 57.3 | 57.0 | 0.50% | 5.73 | 4.69 | 2.20 | 1.56 | 4.68 | 22.10% | 0.95 | 0.96 | 134 | 202 | 0.00% | 0.11 | 1.00 |
| industrial miners at the minimum | today | x10, 30% | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 3.63 | 0.962 | 2.95 | 41.00% | 1.18 | 0.99 | 80.6 | 191 | 0.00% | 0.13 | 1.00 |
| industrial miners at the minimum | asks | x10, 30% | 344 | 337 | 49.99% | 34.4 | 27.6 | 0.373 | 9.38 | 4.91 | 2.70% | 0.56 | 0.99 | 802 | 206 | 0.00% | 0.08 | 1.00 |
| carries no asking weak share | 25.00% | x10, 30% | 250 | 246 | 50.00% | 25.0 | 20.1 | 0.511 | 6.87 | 4.75 | 3.00% | 0.58 | 0.96 | 585 | 206 | 0.00% | 0.08 | 1.00 |
| carries no asking weak share | 50.00% | x10, 30% | 168 | 165 | 50.00% | 16.8 | 13.5 | 0.762 | 4.50 | 4.71 | 5.80% | 0.67 | 0.95 | 392 | 207 | 0.00% | 0.09 | 1.00 |
| carries no asking weak share | 75.00% | x10, 30% | 107 | 106 | 50.01% | 10.7 | 8.66 | 1.19 | 2.87 | 4.73 | 11.10% | 0.73 | 0.96 | 251 | 205 | 0.00% | 0.10 | 1.00 |
| carries no asking weak share | 90.00% | x10, 30% | 83.1 | 83.1 | 49.42% | 8.41 | 6.83 | 1.51 | 2.23 | 4.71 | 14.80% | 0.83 | 0.95 | 194 | 206 | 0.00% | 0.10 | 1.00 |



## Study: the weak ratio's path from genesis, by rule (band-weak-levels, weak work, 50% industrial)

| BCH hashrate, pool | Rule | Peak weak a share (hourly) | Largest share | Shares above 64, first 6 h | Shares above 320, first 6 h | Count stays within 25% | Weak expired, first 6 h | Weak a share, last day | Weak expired, last day |
|---|---|---|---|---|---|---|---|---|---|
| x1, 10% | today | 64.0 | 64.0 | 0 | 0 | 1 h | 13.10% | 34.6 | 0.00% |
| x1, 10% | v0 as written | 132 | 640 | 769 | 40.9 | 3 h | 0.00% | 46.5 | 0.00% |
| x1, 10% | v1 down to R/2 | 13.0 | 543 | 13.9 | 0.625 | < 1 h | 0.00% | 10.9 | 0.00% |
| x1, 10% | v1 down to R/4 | 11.1 | 640 | 11.6 | 0.625 | < 1 h | 0.00% | 10.5 | 0.00% |
| x1, 10% | v2 cap follows asks | 63.9 | 64.0 | 0 | 0 | 1 h | 12.66% | 34.5 | 0.00% |
| x1, 10% | v3 genesis 32 | 48.4 | 556 | 471 | 1.88 | 1 h | 0.00% | 46.4 | 0.00% |
| x1, 10% | v4 = v2 + v3 | 36.3 | 64.0 | 0 | 0 | < 1 h | 0.00% | 34.6 | 0.00% |
| x1, 10% | v5 = v4, one ask opens | 35.7 | 64.0 | 0 | 0 | < 1 h | 0.00% | 34.4 | 0.00% |
| x10, 10% | today | 64.0 | 64.0 | 0 | 0 | 2 h | 21.99% | 34.4 | 0.00% |
| x10, 10% | v0 as written | 200 | 640 | 1,562 | 435 | < 1 h | 0.00% | 190 | 0.00% |
| x10, 10% | v1 down to R/2 | 58.9 | 640 | 624 | 13.0 | < 1 h | 0.00% | 54.4 | 0.00% |
| x10, 10% | v1 down to R/4 | 44.6 | 640 | 377 | 7.00 | < 1 h | 0.00% | 39.6 | 0.00% |
| x10, 10% | v2 cap follows asks | 181 | 640 | 1,038 | 380 | < 1 h | 3.98% | 175 | 1.58% |
| x10, 10% | v3 genesis 32 | 195 | 640 | 1,420 | 349 | 1 h | 0.00% | 190 | 0.00% |
| x10, 10% | v4 = v2 + v3 | 181 | 640 | 1,050 | 324 | 1 h | 1.45% | 175 | 1.54% |
| x10, 10% | v5 = v4, one ask opens | 183 | 640 | 1,055 | 326 | 1 h | 1.11% | 175 | 1.63% |
| x10, 30% | today | 64.0 | 64.0 | 0 | 0 | 2 h | 22.15% | 34.5 | 0.00% |
| x10, 30% | v0 as written | 352 | 640 | 1,861 | 1,033 | < 1 h | 0.00% | 344 | 0.00% |
| x10, 30% | v1 down to R/2 | 59.9 | 640 | 660 | 14.2 | < 1 h | 0.00% | 53.8 | 0.00% |
| x10, 30% | v1 down to R/4 | 48.5 | 640 | 391 | 7.62 | < 1 h | 0.00% | 38.9 | 0.00% |
| x10, 30% | v2 cap follows asks | 353 | 640 | 1,854 | 1,035 | < 1 h | 0.00% | 344 | 0.00% |
| x10, 30% | v3 genesis 32 | 353 | 640 | 1,623 | 769 | 2 h | 0.00% | 344 | 0.00% |
| x10, 30% | v4 = v2 + v3 | 354 | 640 | 1,627 | 770 | 2 h | 0.00% | 344 | 0.00% |
| x10, 30% | v5 = v4, one ask opens | 354 | 640 | 1,642 | 770 | 2 h | 0.00% | 345 | 0.00% |



## Study: a big miner joining or leaving at the minimum, by rule (band-weak-levels, weak work)

| BCH hashrate, pool | Big miner | Rule | Weak a share, 6 h before | Peak weak a share (hourly) | Largest share | Shares above 64, 6 h after | Shares above 320, 6 h after | Count stays within 25% | Weak expired, 6 h after | Weak a share, last day | Weak expired, last day | Home pay over fair, first day |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| x1, 10% | joins | today | 34.3 | 36.7 | 64.0 | 0 | 0 | < 1 h | 0.00% | 34.5 | 0.00% | 1.07 |
| x1, 10% | joins | v0 as written | 46.6 | 49.6 | 546 | 578 | 2.88 | < 1 h | 0.00% | 46.6 | 0.00% | 1.06 |
| x1, 10% | joins | v1 down to R/2 | 11.0 | 26.5 | 345 | 178 | 0.125 | < 1 h | 0.00% | 24.6 | 0.00% | 1.07 |
| x1, 10% | joins | v1 down to R/4 | 10.5 | 24.9 | 364 | 146 | 0 | < 1 h | 0.00% | 21.3 | 0.00% | 1.07 |
| x1, 10% | joins | v2 cap follows asks | 34.4 | 38.8 | 361 | 40.2 | 0 | < 1 h | 0.00% | 37.9 | 0.00% | 1.06 |
| x1, 10% | joins | v3 genesis 32 | 46.4 | 50.6 | 546 | 591 | 4.38 | < 1 h | 0.00% | 46.4 | 0.00% | 1.06 |
| x1, 10% | joins | v4 = v2 + v3 | 34.6 | 38.6 | 425 | 38.5 | 0.125 | < 1 h | 0.00% | 37.9 | 0.00% | 1.07 |
| x1, 10% | joins | v5 = v4, one ask opens | 34.5 | 39.0 | 365 | 37.6 | 0 | < 1 h | 0.00% | 37.9 | 0.00% | 1.06 |
| x1, 10% | leaves | today | 34.3 | 35.5 | 64.0 | 0 | 0 | < 1 h | 0.00% | 34.5 | 0.00% | 0.94 |
| x1, 10% | leaves | v0 as written | 45.7 | 48.3 | 640 | 498 | 2.12 | < 1 h | 0.00% | 46.3 | 0.00% | 0.94 |
| x1, 10% | leaves | v1 down to R/2 | 24.5 | 13.4 | 139 | 11.9 | 0 | < 1 h | 0.00% | 11.0 | 0.00% | 0.94 |
| x1, 10% | leaves | v1 down to R/4 | 20.1 | 11.8 | 167 | 11.6 | 0 | < 1 h | 0.00% | 10.6 | 0.00% | 0.95 |
| x1, 10% | leaves | v2 cap follows asks | 37.9 | 35.8 | 64.0 | 0 | 0 | < 1 h | 0.00% | 34.5 | 0.00% | 0.94 |
| x1, 10% | leaves | v3 genesis 32 | 46.5 | 48.7 | 519 | 500 | 0.875 | < 1 h | 0.00% | 46.4 | 0.00% | 0.94 |
| x1, 10% | leaves | v4 = v2 + v3 | 37.8 | 35.7 | 64.0 | 0 | 0 | < 1 h | 0.00% | 34.5 | 0.00% | 0.94 |
| x1, 10% | leaves | v5 = v4, one ask opens | 37.8 | 35.7 | 64.0 | 0 | 0 | < 1 h | 0.00% | 34.4 | 0.00% | 0.95 |
| x10, 10% | joins | today | 34.4 | 35.9 | 64.0 | 0 | 0 | < 1 h | 0.00% | 34.6 | 0.00% | 1.06 |
| x10, 10% | joins | v0 as written | 190 | 350 | 640 | 1,898 | 1,017 | < 1 h | 0.00% | 342 | 0.00% | 1.06 |
| x10, 10% | joins | v1 down to R/2 | 53.6 | 58.9 | 640 | 642 | 12.4 | < 1 h | 0.00% | 53.7 | 0.00% | 1.06 |
| x10, 10% | joins | v1 down to R/4 | 38.6 | 48.9 | 640 | 401 | 6.25 | < 1 h | 0.00% | 41.2 | 0.00% | 1.06 |
| x10, 10% | joins | v2 cap follows asks | 175 | 345 | 640 | 1,851 | 974 | 1 h | 0.05% | 336 | 0.02% | 1.06 |
| x10, 10% | joins | v3 genesis 32 | 190 | 350 | 640 | 1,887 | 1,013 | < 1 h | 0.00% | 341 | 0.00% | 1.06 |
| x10, 10% | joins | v4 = v2 + v3 | 175 | 345 | 640 | 1,860 | 976 | 1 h | 0.09% | 335 | 0.14% | 1.06 |
| x10, 10% | joins | v5 = v4, one ask opens | 177 | 350 | 640 | 1,868 | 998 | < 1 h | 0.00% | 336 | 0.07% | 1.06 |
| x10, 10% | leaves | today | 34.3 | 35.8 | 64.0 | 0 | 0 | < 1 h | 0.00% | 34.5 | 0.00% | 0.94 |
| x10, 10% | leaves | v0 as written | 343 | 241 | 640 | 1,529 | 447 | 1 h | 0.00% | 190 | 0.00% | 0.94 |
| x10, 10% | leaves | v1 down to R/2 | 53.8 | 61.1 | 640 | 613 | 11.4 | < 1 h | 0.00% | 53.4 | 0.00% | 0.94 |
| x10, 10% | leaves | v1 down to R/4 | 39.1 | 47.1 | 640 | 380 | 5.62 | < 1 h | 0.00% | 40.2 | 0.00% | 0.94 |
| x10, 10% | leaves | v2 cap follows asks | 334 | 181 | 640 | 1,006 | 381 | < 1 h | 5.74% | 175 | 1.69% | 0.94 |
| x10, 10% | leaves | v3 genesis 32 | 339 | 244 | 640 | 1,523 | 449 | 1 h | 0.00% | 190 | 0.00% | 0.94 |
| x10, 10% | leaves | v4 = v2 + v3 | 337 | 184 | 640 | 991 | 365 | < 1 h | 7.14% | 175 | 1.53% | 0.94 |
| x10, 10% | leaves | v5 = v4, one ask opens | 338 | 185 | 640 | 987 | 367 | < 1 h | 7.50% | 175 | 1.64% | 0.94 |
| x10, 30% | joins | today | 34.7 | 35.9 | 64.0 | 0 | 0 | < 1 h | 0.00% | 34.5 | 0.00% | 1.03 |
| x10, 30% | joins | v0 as written | 345 | 353 | 640 | 1,916 | 1,087 | < 1 h | 0.00% | 340 | 0.00% | 1.03 |
| x10, 30% | joins | v1 down to R/2 | 54.0 | 59.3 | 640 | 639 | 12.5 | < 1 h | 0.00% | 53.8 | 0.00% | 1.03 |
| x10, 30% | joins | v1 down to R/4 | 42.3 | 44.2 | 640 | 410 | 6.38 | < 1 h | 0.00% | 40.0 | 0.00% | 1.03 |
| x10, 30% | joins | v2 cap follows asks | 344 | 350 | 640 | 1,924 | 1,084 | < 1 h | 0.00% | 341 | 0.00% | 1.03 |
| x10, 30% | joins | v3 genesis 32 | 344 | 352 | 640 | 1,927 | 1,089 | < 1 h | 0.00% | 342 | 0.00% | 1.03 |
| x10, 30% | joins | v4 = v2 + v3 | 343 | 360 | 640 | 1,925 | 1,091 | < 1 h | 0.00% | 342 | 0.00% | 1.03 |
| x10, 30% | joins | v5 = v4, one ask opens | 344 | 351 | 640 | 1,925 | 1,088 | < 1 h | 0.00% | 340 | 0.00% | 1.03 |
| x10, 30% | leaves | today | 34.5 | 35.6 | 64.0 | 0 | 0 | < 1 h | 0.00% | 34.5 | 0.00% | 0.98 |
| x10, 30% | leaves | v0 as written | 342 | 351 | 640 | 1,815 | 1,035 | < 1 h | 0.00% | 343 | 0.00% | 0.98 |
| x10, 30% | leaves | v1 down to R/2 | 55.5 | 59.5 | 640 | 612 | 11.6 | < 1 h | 0.00% | 53.7 | 0.00% | 0.98 |
| x10, 30% | leaves | v1 down to R/4 | 38.9 | 48.2 | 640 | 407 | 5.75 | < 1 h | 0.00% | 40.8 | 0.00% | 0.98 |
| x10, 30% | leaves | v2 cap follows asks | 343 | 354 | 640 | 1,825 | 1,040 | < 1 h | 0.00% | 343 | 0.00% | 0.98 |
| x10, 30% | leaves | v3 genesis 32 | 341 | 357 | 640 | 1,818 | 1,038 | < 1 h | 0.00% | 344 | 0.00% | 0.98 |
| x10, 30% | leaves | v4 = v2 + v3 | 342 | 357 | 640 | 1,810 | 1,041 | < 1 h | 0.00% | 342 | 0.00% | 0.98 |
| x10, 30% | leaves | v5 = v4, one ask opens | 342 | 353 | 640 | 1,818 | 1,038 | < 1 h | 0.00% | 345 | 0.00% | 0.98 |
| test network, 10 GH/s | joins | today | 3.95 | 49.8 | 64.0 | 0 | 0 | 1 h | 10.09% | 34.5 | 0.00% | 4.50 |
| test network, 10 GH/s | joins | v0 as written | 3.82 | 93.4 | 640 | 860 | 50.9 | 2 h | 0.00% | 46.5 | 0.00% | 3.87 |
| test network, 10 GH/s | joins | v1 down to R/2 | 3.91 | 7.89 | 142 | 3.00 | 0 | < 1 h | 0.00% | 7.13 | 0.00% | 3.87 |
| test network, 10 GH/s | joins | v1 down to R/4 | 3.87 | 7.84 | 112 | 2.88 | 0 | < 1 h | 0.00% | 6.99 | 0.00% | 3.83 |
| test network, 10 GH/s | joins | v2 cap follows asks | 4.03 | 49.8 | 64.0 | 0 | 0 | 1 h | 10.36% | 34.5 | 0.00% | 4.50 |
| test network, 10 GH/s | joins | v3 genesis 32 | 3.87 | 94.9 | 640 | 870 | 53.4 | 2 h | 0.00% | 46.6 | 0.00% | 3.88 |
| test network, 10 GH/s | joins | v4 = v2 + v3 | 3.86 | 49.4 | 64.0 | 0 | 0 | 1 h | 10.25% | 34.5 | 0.00% | 4.48 |
| test network, 10 GH/s | joins | v5 = v4, one ask opens | 3.91 | 49.5 | 64.0 | 0 | 0 | 1 h | 10.22% | 34.5 | 0.00% | 4.47 |
| test network, 10 GH/s | leaves | today | 34.6 | 23.3 | 64.0 | 0 | 0 | 2 h | 0.00% | 3.94 | 0.00% | 0.07 |
| test network, 10 GH/s | leaves | v0 as written | 45.4 | 25.9 | 250 | 2.62 | 0 | 2 h | 0.00% | 3.88 | 0.00% | 0.07 |
| test network, 10 GH/s | leaves | v1 down to R/2 | 7.21 | 7.00 | 58.0 | 0 | 0 | 2 h | 0.00% | 3.84 | 0.00% | 0.07 |
| test network, 10 GH/s | leaves | v1 down to R/4 | 7.07 | 6.76 | 75.0 | 0.125 | 0 | 2 h | 0.00% | 3.93 | 0.00% | 0.07 |
| test network, 10 GH/s | leaves | v2 cap follows asks | 34.4 | 21.3 | 64.0 | 0 | 0 | 2 h | 0.00% | 3.90 | 0.00% | 0.07 |
| test network, 10 GH/s | leaves | v3 genesis 32 | 46.8 | 25.5 | 252 | 3.75 | 0 | 2 h | 0.00% | 3.93 | 0.00% | 0.07 |
| test network, 10 GH/s | leaves | v4 = v2 + v3 | 34.4 | 23.1 | 64.0 | 0 | 0 | 2 h | 0.00% | 3.92 | 0.00% | 0.07 |
| test network, 10 GH/s | leaves | v5 = v4, one ask opens | 34.4 | 25.3 | 64.0 | 0 | 0 | 2 h | 0.00% | 3.86 | 0.00% | 0.07 |
| test network, 10 GH/s, 1% always asks | joins | today | 3.93 | 49.9 | 64.0 | 0 | 0 | 1 h | 10.71% | 34.5 | 0.00% | 4.53 |
| test network, 10 GH/s, 1% always asks | joins | v0 as written | 3.88 | 110 | 640 | 1,245 | 122 | 2 h | 0.00% | 82.5 | 0.00% | 3.86 |
| test network, 10 GH/s, 1% always asks | joins | v1 down to R/2 | 3.99 | 8.69 | 130 | 3.50 | 0 | < 1 h | 0.00% | 7.84 | 0.00% | 3.87 |
| test network, 10 GH/s, 1% always asks | joins | v1 down to R/4 | 3.84 | 8.60 | 134 | 4.12 | 0 | < 1 h | 0.00% | 7.78 | 0.00% | 3.87 |
| test network, 10 GH/s, 1% always asks | joins | v2 cap follows asks | 3.83 | 63.5 | 640 | 259 | 5.12 | 1 h | 4.89% | 49.5 | 0.00% | 4.33 |
| test network, 10 GH/s, 1% always asks | joins | v3 genesis 32 | 3.93 | 112 | 640 | 1,271 | 130 | 2 h | 0.00% | 82.0 | 0.00% | 3.86 |
| test network, 10 GH/s, 1% always asks | joins | v4 = v2 + v3 | 3.90 | 67.0 | 640 | 253 | 6.75 | 1 h | 5.81% | 49.8 | 0.00% | 4.38 |
| test network, 10 GH/s, 1% always asks | joins | v5 = v4, one ask opens | 3.90 | 85.5 | 640 | 695 | 66.0 | 2 h | 1.02% | 66.0 | 0.00% | 3.95 |
| test network, 10 GH/s, 1% always asks | leaves | today | 34.7 | 22.9 | 64.0 | 0 | 0 | 2 h | 0.00% | 3.95 | 0.00% | 0.07 |
| test network, 10 GH/s, 1% always asks | leaves | v0 as written | 80.7 | 31.8 | 427 | 4.38 | 0.375 | 2 h | 0.00% | 3.86 | 0.00% | 0.07 |
| test network, 10 GH/s, 1% always asks | leaves | v1 down to R/2 | 7.81 | 7.61 | 64.0 | 0 | 0 | 2 h | 0.00% | 3.94 | 0.00% | 0.07 |
| test network, 10 GH/s, 1% always asks | leaves | v1 down to R/4 | 7.56 | 6.67 | 77.0 | 0.125 | 0 | 2 h | 0.00% | 3.97 | 0.00% | 0.07 |
| test network, 10 GH/s, 1% always asks | leaves | v2 cap follows asks | 49.4 | 25.8 | 127 | 0.500 | 0 | 2 h | 0.00% | 3.95 | 0.00% | 0.07 |
| test network, 10 GH/s, 1% always asks | leaves | v3 genesis 32 | 83.8 | 29.8 | 320 | 4.62 | 0 | 2 h | 0.00% | 3.90 | 0.00% | 0.07 |
| test network, 10 GH/s, 1% always asks | leaves | v4 = v2 + v3 | 49.0 | 25.4 | 127 | 0.500 | 0 | 2 h | 0.00% | 3.95 | 0.00% | 0.07 |
| test network, 10 GH/s, 1% always asks | leaves | v5 = v4, one ask opens | 67.1 | 33.5 | 294 | 1.88 | 0 | 2 h | 0.00% | 3.93 | 0.00% | 0.07 |



## Study: where each rule settles (band-weak-levels, weak work, 50% industrial)

| BCH hashrate, pool | Rule | Weak expired | Weak a share | R | Asking | Weak/s | KB/s | Smallest miner with 3 weak shares a month (TH/s) | Bitaxe weak/mo | Bitaxe pay/mo | Bitaxe empty months | Bitaxe swing | Bitaxe vs fair | ASIC weak/mo | ASIC pay/mo | ASIC empty months | ASIC swing | ASIC vs fair |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| x1, 10% | today | 0.00% | 34.5 | 36.8 | 0.00% | 3.45 | 2.86 | 0.091 | 38.6 | 24.7 | 0.00% | 0.25 | 0.99 | 3,209 | 431 | 0.00% | 0.05 | 1.00 |
| x1, 10% | v0 as written | 0.00% | 46.3 | 49.3 | 0.00% | 4.63 | 3.81 | 0.068 | 52.0 | 25.1 | 0.00% | 0.25 | 1.01 | 4,327 | 433 | 0.00% | 0.05 | 1.00 |
| x1, 10% | v1 down to R/2 | 0.00% | 10.7 | 12.2 | 56.53% | 1.07 | 0.961 | 0.275 | 11.9 | 24.1 | 0.00% | 0.34 | 0.99 | 1,000 | 431 | 0.00% | 0.06 | 1.00 |
| x1, 10% | v1 down to R/4 | 0.00% | 10.3 | 11.8 | 56.91% | 1.03 | 0.929 | 0.284 | 11.5 | 24.1 | 0.00% | 0.33 | 0.99 | 966 | 430 | 0.00% | 0.06 | 1.00 |
| x1, 10% | v2 cap follows asks | 0.00% | 34.5 | 36.9 | 0.00% | 3.45 | 2.86 | 0.091 | 38.7 | 24.7 | 0.00% | 0.26 | 1.00 | 3,219 | 431 | 0.00% | 0.05 | 1.00 |
| x1, 10% | v3 genesis 32 | 0.00% | 46.3 | 49.3 | 0.00% | 4.63 | 3.81 | 0.068 | 52.0 | 24.9 | 0.00% | 0.25 | 1.00 | 4,330 | 433 | 0.00% | 0.05 | 1.00 |
| x1, 10% | v4 = v2 + v3 | 0.00% | 34.5 | 36.9 | 0.00% | 3.45 | 2.86 | 0.091 | 38.7 | 24.9 | 0.00% | 0.25 | 1.00 | 3,218 | 431 | 0.00% | 0.05 | 1.00 |
| x1, 10% | v5 = v4, one ask opens | 0.00% | 34.5 | 36.9 | 0.00% | 3.45 | 2.86 | 0.091 | 38.7 | 24.9 | 0.00% | 0.25 | 1.00 | 3,218 | 431 | 0.00% | 0.05 | 1.00 |
| x10, 10% | today | 0.00% | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 1.21 | 2.76 | 4.73 | 9.30% | 0.76 | 0.96 | 242 | 355 | 0.00% | 0.09 | 1.00 |
| x10, 10% | v0 as written | 0.00% | 189 | 186 | 26.62% | 18.9 | 15.3 | 0.225 | 15.7 | 4.87 | 1.40% | 0.51 | 0.98 | 1,325 | 431 | 0.00% | 0.06 | 1.00 |
| x10, 10% | v1 down to R/2 | 0.00% | 54.1 | 53.9 | 50.01% | 5.41 | 4.43 | 0.778 | 4.35 | 4.74 | 5.20% | 0.63 | 0.96 | 379 | 402 | 0.00% | 0.08 | 1.00 |
| x10, 10% | v1 down to R/4 | 0.00% | 38.9 | 39.1 | 50.01% | 3.89 | 3.21 | 1.07 | 3.13 | 4.79 | 7.60% | 0.72 | 0.97 | 273 | 369 | 0.00% | 0.08 | 1.00 |
| x10, 10% | v2 cap follows asks | 0.00% | 187 | 184 | 38.32% | 18.7 | 15.1 | 0.228 | 15.5 | 4.92 | 1.50% | 0.53 | 0.99 | 1,310 | 431 | 0.00% | 0.06 | 1.00 |
| x10, 10% | v3 genesis 32 | 0.00% | 190 | 186 | 26.61% | 19.0 | 15.3 | 0.225 | 15.7 | 4.86 | 1.10% | 0.53 | 0.98 | 1,325 | 431 | 0.00% | 0.06 | 1.00 |
| x10, 10% | v4 = v2 + v3 | 0.00% | 187 | 184 | 38.38% | 18.7 | 15.1 | 0.228 | 15.5 | 4.99 | 0.80% | 0.51 | 1.01 | 1,309 | 431 | 0.00% | 0.06 | 1.00 |
| x10, 10% | v5 = v4, one ask opens | 0.00% | 187 | 184 | 38.24% | 18.7 | 15.1 | 0.228 | 15.5 | 4.87 | 1.20% | 0.50 | 0.98 | 1,310 | 431 | 0.00% | 0.06 | 1.00 |
| x10, 30% | today | 0.00% | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 3.63 | 0.962 | 2.95 | 41.00% | 1.18 | 0.99 | 80.6 | 191 | 0.00% | 0.13 | 1.00 |
| x10, 30% | v0 as written | 0.00% | 344 | 337 | 49.99% | 34.4 | 27.6 | 0.373 | 9.38 | 4.91 | 2.70% | 0.56 | 0.99 | 802 | 206 | 0.00% | 0.08 | 1.00 |
| x10, 30% | v1 down to R/2 | 0.00% | 53.9 | 53.6 | 50.02% | 5.39 | 4.41 | 2.35 | 1.47 | 4.45 | 25.40% | 0.95 | 0.97 | 126 | 201 | 0.00% | 0.12 | 1.00 |
| x10, 30% | v1 down to R/4 | 0.00% | 39.9 | 40.0 | 50.01% | 3.99 | 3.30 | 3.15 | 1.11 | 3.44 | 34.30% | 1.10 | 1.01 | 93.1 | 193 | 0.00% | 0.12 | 1.00 |
| x10, 30% | v2 cap follows asks | 0.00% | 344 | 337 | 49.99% | 34.4 | 27.6 | 0.373 | 9.38 | 4.91 | 2.70% | 0.56 | 0.99 | 802 | 206 | 0.00% | 0.08 | 1.00 |
| x10, 30% | v3 genesis 32 | 0.00% | 343 | 337 | 50.00% | 34.3 | 27.6 | 0.373 | 9.37 | 4.89 | 1.80% | 0.56 | 0.99 | 801 | 205 | 0.00% | 0.08 | 1.00 |
| x10, 30% | v4 = v2 + v3 | 0.00% | 343 | 337 | 50.00% | 34.3 | 27.6 | 0.373 | 9.37 | 4.89 | 1.80% | 0.56 | 0.99 | 801 | 205 | 0.00% | 0.08 | 1.00 |
| x10, 30% | v5 = v4, one ask opens | 0.00% | 343 | 337 | 50.00% | 34.3 | 27.6 | 0.373 | 9.37 | 4.89 | 1.80% | 0.56 | 0.99 | 801 | 205 | 0.00% | 0.08 | 1.00 |



## Study: the rules that bound the start in every miner mix, at ten times today's hashrate (band-weak-levels, weak work)

| Mix | Pool | Rule | Weak expired | Weak a share | R | Asking | Weak/s | KB/s | Smallest miner with 3 weak shares a month (TH/s) | Bitaxe weak/mo | Bitaxe pay/mo | Bitaxe empty months | Bitaxe swing | Bitaxe vs fair | ASIC weak/mo | ASIC pay/mo | ASIC empty months | ASIC swing | ASIC vs fair |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| home | 10% | today | 0.00% | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 1.21 | 2.76 | 4.62 | 10.30% | 0.75 | 0.93 | 242 | 355 | 0.00% | 0.09 | 1.00 |
| home | 10% | v0 as written | 0.00% | 189 | 186 | 52.82% | 18.9 | 15.2 | 0.225 | 15.7 | 4.79 | 1.50% | 0.52 | 0.97 | 1,326 | 431 | 0.00% | 0.06 | 1.00 |
| home | 10% | v4 = v2 + v3 | 0.00% | 187 | 184 | 76.39% | 18.7 | 15.0 | 0.228 | 15.5 | 4.94 | 1.20% | 0.51 | 1.00 | 1,310 | 431 | 0.00% | 0.06 | 1.00 |
| home | 10% | v5 = v4, one ask opens | 0.00% | 187 | 184 | 76.59% | 18.7 | 15.1 | 0.228 | 15.5 | 4.96 | 1.40% | 0.52 | 1.00 | 1,310 | 431 | 0.00% | 0.06 | 1.00 |
| home | 30% | today | 0.00% | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 3.63 | 0.962 | 2.95 | 41.00% | 1.18 | 0.99 | 80.6 | 226 | 0.00% | 0.13 | 1.00 |
| home | 30% | v0 as written | 0.00% | 345 | 338 | 100.00% | 34.5 | 27.7 | 0.372 | 9.39 | 4.87 | 2.10% | 0.55 | 0.98 | 804 | 1,094 | 0.00% | 0.05 | 1.00 |
| home | 30% | v4 = v2 + v3 | 0.00% | 345 | 339 | 100.00% | 34.5 | 27.7 | 0.371 | 9.42 | 4.77 | 1.70% | 0.54 | 0.96 | 806 | 1,095 | 0.00% | 0.05 | 1.00 |
| home | 30% | v5 = v4, one ask opens | 0.00% | 345 | 339 | 100.00% | 34.5 | 27.7 | 0.371 | 9.42 | 4.77 | 1.70% | 0.54 | 0.96 | 806 | 1,095 | 0.00% | 0.05 | 1.00 |
| 50% industrial | 10% | today | 0.00% | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 1.21 | 2.76 | 4.73 | 9.30% | 0.76 | 0.96 | 242 | 355 | 0.00% | 0.09 | 1.00 |
| 50% industrial | 10% | v0 as written | 0.00% | 189 | 186 | 26.62% | 18.9 | 15.3 | 0.225 | 15.7 | 4.87 | 1.40% | 0.51 | 0.98 | 1,325 | 431 | 0.00% | 0.06 | 1.00 |
| 50% industrial | 10% | v4 = v2 + v3 | 0.00% | 187 | 184 | 38.38% | 18.7 | 15.1 | 0.228 | 15.5 | 4.99 | 0.80% | 0.51 | 1.01 | 1,309 | 431 | 0.00% | 0.06 | 1.00 |
| 50% industrial | 10% | v5 = v4, one ask opens | 0.00% | 187 | 184 | 38.24% | 18.7 | 15.1 | 0.228 | 15.5 | 4.87 | 1.20% | 0.50 | 0.98 | 1,310 | 431 | 0.00% | 0.06 | 1.00 |
| 50% industrial | 30% | today | 0.00% | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 3.63 | 0.962 | 2.95 | 41.00% | 1.18 | 0.99 | 80.6 | 191 | 0.00% | 0.13 | 1.00 |
| 50% industrial | 30% | v0 as written | 0.00% | 344 | 337 | 49.99% | 34.4 | 27.6 | 0.373 | 9.38 | 4.91 | 2.70% | 0.56 | 0.99 | 802 | 206 | 0.00% | 0.08 | 1.00 |
| 50% industrial | 30% | v4 = v2 + v3 | 0.00% | 343 | 337 | 50.00% | 34.3 | 27.6 | 0.373 | 9.37 | 4.89 | 1.80% | 0.56 | 0.99 | 801 | 205 | 0.00% | 0.08 | 1.00 |
| 50% industrial | 30% | v5 = v4, one ask opens | 0.00% | 343 | 337 | 50.00% | 34.3 | 27.6 | 0.373 | 9.37 | 4.89 | 1.80% | 0.56 | 0.99 | 801 | 205 | 0.00% | 0.08 | 1.00 |
| 90% industrial | 10% | today | 0.00% | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 1.21 | 2.76 | 4.72 | 9.80% | 0.74 | 0.95 | 242 | 355 | 0.00% | 0.09 | 1.00 |
| 90% industrial | 10% | v0 as written | 0.00% | 189 | 186 | 5.34% | 18.9 | 15.2 | 0.225 | 15.7 | 4.82 | 1.40% | 0.53 | 0.97 | 1,325 | 431 | 0.00% | 0.06 | 1.00 |
| 90% industrial | 10% | v4 = v2 + v3 | 0.00% | 187 | 184 | 7.71% | 18.7 | 15.1 | 0.228 | 15.5 | 4.99 | 1.90% | 0.52 | 1.01 | 1,310 | 431 | 0.00% | 0.06 | 1.00 |
| 90% industrial | 10% | v5 = v4, one ask opens | 0.00% | 186 | 184 | 7.68% | 18.6 | 15.0 | 0.228 | 15.5 | 4.96 | 1.30% | 0.52 | 1.00 | 1,310 | 431 | 0.00% | 0.06 | 1.00 |
| 90% industrial | 30% | today | 0.00% | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 3.63 | 0.962 | 2.95 | 41.00% | 1.18 | 0.99 | 80.6 | 190 | 0.00% | 0.13 | 1.00 |
| 90% industrial | 30% | v0 as written | 0.00% | 334 | 327 | 10.00% | 33.4 | 26.8 | 0.385 | 9.11 | 4.91 | 2.80% | 0.58 | 0.99 | 778 | 207 | 0.00% | 0.08 | 1.00 |
| 90% industrial | 30% | v4 = v2 + v3 | 0.00% | 334 | 329 | 10.00% | 33.4 | 26.8 | 0.383 | 9.15 | 4.76 | 2.10% | 0.55 | 0.96 | 782 | 206 | 0.00% | 0.08 | 1.00 |
| 90% industrial | 30% | v5 = v4, one ask opens | 0.00% | 334 | 328 | 10.00% | 33.4 | 26.8 | 0.383 | 9.14 | 4.89 | 2.20% | 0.57 | 0.99 | 781 | 207 | 0.00% | 0.08 | 1.00 |
| 99% industrial | 10% | today | 0.00% | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 1.21 | 2.76 | 4.78 | 9.00% | 0.74 | 0.97 | 242 | 355 | 0.00% | 0.09 | 1.00 |
| 99% industrial | 10% | v0 as written | 0.00% | 81.7 | 81.1 | 1.00% | 8.17 | 6.64 | 0.517 | 6.74 | 4.97 | 2.60% | 0.57 | 1.00 | 573 | 424 | 0.00% | 0.07 | 1.00 |
| 99% industrial | 10% | v4 = v2 + v3 | 0.00% | 49.5 | 49.4 | 1.00% | 4.95 | 4.06 | 0.849 | 3.97 | 4.78 | 7.70% | 0.67 | 0.96 | 347 | 395 | 0.00% | 0.08 | 1.00 |
| 99% industrial | 10% | v5 = v4, one ask opens | 0.00% | 66.8 | 66.4 | 1.00% | 6.68 | 5.44 | 0.631 | 5.41 | 4.82 | 3.20% | 0.60 | 0.97 | 468 | 416 | 0.00% | 0.07 | 1.00 |
| 99% industrial | 30% | today | 0.00% | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 3.63 | 0.962 | 2.95 | 41.00% | 1.18 | 0.99 | 80.6 | 191 | 0.00% | 0.13 | 1.00 |
| 99% industrial | 30% | v0 as written | 0.00% | 81.5 | 80.7 | 1.00% | 8.15 | 6.62 | 1.56 | 2.20 | 4.67 | 14.00% | 0.79 | 0.95 | 190 | 205 | 0.00% | 0.10 | 1.00 |
| 99% industrial | 30% | v4 = v2 + v3 | 0.00% | 49.7 | 49.6 | 1.00% | 4.97 | 4.08 | 2.53 | 1.37 | 4.10 | 25.30% | 0.93 | 0.96 | 116 | 197 | 0.00% | 0.12 | 1.00 |
| 99% industrial | 30% | v5 = v4, one ask opens | 0.00% | 66.2 | 65.9 | 1.00% | 6.62 | 5.40 | 1.91 | 1.78 | 4.67 | 19.50% | 0.87 | 0.95 | 155 | 204 | 0.00% | 0.11 | 1.00 |
| zipf | 10% | today | 0.00% | 34.5 | 38.1 | 0.00% | 3.45 | 2.86 | 0.907 | 3.70 | 4.68 | 6.60% | 0.69 | 0.95 | 322 | 202 | 0.00% | 0.09 | 1.00 |
| zipf | 10% | v0 as written | 0.00% | 115 | 126 | 1.42% | 11.5 | 9.34 | 0.275 | 12.8 | 4.86 | 2.30% | 0.56 | 0.98 | 1,081 | 207 | 0.00% | 0.08 | 1.00 |
| zipf | 10% | v4 = v2 + v3 | 0.00% | 114 | 124 | 1.95% | 11.4 | 9.24 | 0.279 | 12.6 | 4.88 | 1.40% | 0.51 | 0.99 | 1,066 | 207 | 0.00% | 0.08 | 1.00 |
| zipf | 10% | v5 = v4, one ask opens | 0.00% | 115 | 125 | 1.61% | 11.5 | 9.29 | 0.277 | 12.7 | 4.86 | 2.30% | 0.53 | 0.98 | 1,073 | 206 | 0.00% | 0.08 | 1.00 |
| zipf | 30% | today | 0.00% | 34.5 | 36.8 | 0.00% | 3.45 | 2.86 | 3.15 | 1.10 | 3.39 | 33.90% | 1.07 | 0.99 | 92.7 | 193 | 0.00% | 0.13 | 1.00 |
| zipf | 30% | v0 as written | 0.00% | 299 | 312 | 3.93% | 29.9 | 24.0 | 0.372 | 9.39 | 4.86 | 2.40% | 0.56 | 0.98 | 803 | 207 | 0.00% | 0.08 | 1.00 |
| zipf | 30% | v4 = v2 + v3 | 0.00% | 302 | 315 | 3.89% | 30.2 | 24.3 | 0.368 | 9.49 | 4.83 | 2.40% | 0.55 | 0.97 | 812 | 206 | 0.00% | 0.08 | 1.00 |
| zipf | 30% | v5 = v4, one ask opens | 0.00% | 299 | 311 | 3.94% | 29.9 | 24.0 | 0.374 | 9.36 | 4.65 | 2.20% | 0.55 | 0.94 | 800 | 206 | 0.00% | 0.08 | 1.00 |
| medium | 10% | today | 0.00% | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 1.21 | 2.76 | 4.72 | 9.80% | 0.74 | 0.95 | 242 | 197 | 0.00% | 0.10 | 1.00 |
| medium | 10% | v0 as written | 0.00% | 190 | 186 | 5.36% | 19.0 | 15.3 | 0.225 | 15.7 | 4.88 | 1.40% | 0.51 | 0.98 | 1,325 | 206 | 0.00% | 0.08 | 1.00 |
| medium | 10% | v4 = v2 + v3 | 0.00% | 187 | 184 | 7.72% | 18.7 | 15.0 | 0.228 | 15.5 | 4.89 | 1.30% | 0.52 | 0.99 | 1,310 | 207 | 0.00% | 0.08 | 1.00 |
| medium | 10% | v5 = v4, one ask opens | 0.00% | 187 | 184 | 7.71% | 18.7 | 15.1 | 0.228 | 15.5 | 4.94 | 1.40% | 0.50 | 1.00 | 1,310 | 207 | 0.00% | 0.08 | 1.00 |
| medium | 30% | today | 0.00% | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 3.63 | 0.962 | 2.95 | 41.00% | 1.18 | 0.99 | 80.6 | 191 | 0.00% | 0.13 | 1.00 |
| medium | 30% | v0 as written | 0.00% | 334 | 328 | 10.00% | 33.4 | 26.8 | 0.384 | 9.12 | 4.93 | 1.60% | 0.56 | 0.99 | 779 | 207 | 0.00% | 0.08 | 1.00 |
| medium | 30% | v4 = v2 + v3 | 0.00% | 334 | 328 | 10.00% | 33.4 | 26.8 | 0.384 | 9.13 | 4.76 | 2.20% | 0.56 | 0.96 | 780 | 206 | 0.00% | 0.08 | 1.00 |
| medium | 30% | v5 = v4, one ask opens | 0.00% | 334 | 327 | 10.00% | 33.4 | 26.8 | 0.385 | 9.10 | 4.85 | 3.10% | 0.55 | 0.98 | 777 | 206 | 0.00% | 0.08 | 1.00 |
| medium, few big | 10% | today | 0.00% | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 1.21 | 2.76 | 4.72 | 9.80% | 0.74 | 0.95 | 242 | 197 | 0.00% | 0.10 | 1.00 |
| medium, few big | 10% | v0 as written | 0.00% | 189 | 186 | 5.33% | 18.9 | 15.2 | 0.225 | 15.7 | 4.84 | 1.60% | 0.53 | 0.98 | 1,325 | 206 | 0.00% | 0.07 | 1.00 |
| medium, few big | 10% | v4 = v2 + v3 | 0.00% | 187 | 184 | 7.71% | 18.7 | 15.0 | 0.228 | 15.5 | 4.81 | 1.40% | 0.54 | 0.97 | 1,310 | 207 | 0.00% | 0.08 | 1.00 |
| medium, few big | 10% | v5 = v4, one ask opens | 0.00% | 186 | 184 | 7.70% | 18.6 | 15.0 | 0.228 | 15.5 | 4.99 | 1.40% | 0.53 | 1.01 | 1,310 | 206 | 0.00% | 0.08 | 1.00 |
| medium, few big | 30% | today | 0.00% | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 3.63 | 0.962 | 2.95 | 41.00% | 1.18 | 0.99 | 80.6 | 191 | 0.00% | 0.13 | 1.00 |
| medium, few big | 30% | v0 as written | 0.00% | 334 | 329 | 10.00% | 33.4 | 26.8 | 0.383 | 9.15 | 4.93 | 2.70% | 0.56 | 0.99 | 782 | 207 | 0.00% | 0.08 | 1.00 |
| medium, few big | 30% | v4 = v2 + v3 | 0.00% | 334 | 328 | 10.00% | 33.4 | 26.8 | 0.384 | 9.12 | 4.80 | 3.10% | 0.60 | 0.97 | 779 | 207 | 0.00% | 0.08 | 1.00 |
| medium, few big | 30% | v5 = v4, one ask opens | 0.00% | 334 | 327 | 10.00% | 33.4 | 26.8 | 0.384 | 9.12 | 4.81 | 2.50% | 0.58 | 0.97 | 779 | 206 | 0.00% | 0.08 | 1.00 |



## Study: the attacks on the asks under each rule (band-weak-levels, weak work, 50% industrial)

| Attack | Its part | BCH hashrate, pool | Rule | Weak expired | Weak a share | R | Asking | Weak/s | KB/s | Smallest miner with 3 weak shares a month (TH/s) | Bitaxe weak/mo | Bitaxe pay/mo | Bitaxe empty months | Bitaxe swing | Bitaxe vs fair | ASIC weak/mo | ASIC pay/mo | ASIC empty months | ASIC swing | ASIC vs fair |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| asks whatever it finds | 0.00% | x1, 10% | v0 as written | 0.00% | 46.3 | 49.3 | 0.00% | 4.63 | 3.81 | 0.068 | 52.0 | 25.1 | 0.00% | 0.25 | 1.01 | 4,327 | 433 | 0.00% | 0.05 | 1.00 |
| asks whatever it finds | 0.00% | x1, 10% | v1 down to R/2 | 0.00% | 10.7 | 12.2 | 56.53% | 1.07 | 0.961 | 0.275 | 11.9 | 24.1 | 0.00% | 0.34 | 0.99 | 1,000 | 431 | 0.00% | 0.06 | 1.00 |
| asks whatever it finds | 0.00% | x1, 10% | v2 cap follows asks | 0.00% | 34.5 | 36.9 | 0.00% | 3.45 | 2.86 | 0.091 | 38.7 | 24.7 | 0.00% | 0.26 | 1.00 | 3,219 | 431 | 0.00% | 0.05 | 1.00 |
| asks whatever it finds | 0.00% | x1, 10% | v5 = v4, one ask opens | 0.00% | 34.5 | 36.9 | 0.00% | 3.45 | 2.86 | 0.091 | 38.7 | 24.9 | 0.00% | 0.25 | 1.00 | 3,218 | 431 | 0.00% | 0.05 | 1.00 |
| asks whatever it finds | 1.00% | x1, 10% | v0 as written | 0.00% | 82.1 | 86.5 | 1.00% | 8.21 | 6.67 | 0.039 | 92.0 | 24.8 | 0.00% | 0.23 | 1.00 | 7,661 | 432 | 0.00% | 0.05 | 1.00 |
| asks whatever it finds | 1.00% | x1, 10% | v1 down to R/2 | 0.00% | 11.0 | 12.5 | 52.66% | 1.10 | 0.983 | 0.268 | 12.2 | 24.1 | 0.00% | 0.35 | 0.99 | 1,027 | 431 | 0.00% | 0.06 | 1.00 |
| asks whatever it finds | 1.00% | x1, 10% | v2 cap follows asks | 0.00% | 49.5 | 52.5 | 1.00% | 4.95 | 4.06 | 0.064 | 55.4 | 25.1 | 0.00% | 0.24 | 1.01 | 4,612 | 432 | 0.00% | 0.05 | 1.00 |
| asks whatever it finds | 1.00% | x1, 10% | v5 = v4, one ask opens | 0.00% | 66.7 | 70.5 | 1.00% | 6.67 | 5.44 | 0.047 | 74.8 | 24.8 | 0.00% | 0.23 | 1.00 | 6,224 | 432 | 0.00% | 0.05 | 1.00 |
| asks whatever it finds | 2.00% | x1, 10% | v0 as written | 0.00% | 221 | 232 | 2.00% | 22.1 | 17.8 | 0.014 | 249 | 25.0 | 0.00% | 0.21 | 1.01 | 20,720 | 432 | 0.00% | 0.05 | 1.00 |
| asks whatever it finds | 2.00% | x1, 10% | v1 down to R/2 | 0.00% | 11.5 | 12.9 | 47.39% | 1.15 | 1.02 | 0.259 | 12.7 | 23.9 | 0.00% | 0.35 | 0.98 | 1,068 | 431 | 0.00% | 0.06 | 1.00 |
| asks whatever it finds | 2.00% | x1, 10% | v2 cap follows asks | 0.00% | 241 | 252 | 2.00% | 24.1 | 19.4 | 0.013 | 270 | 24.9 | 0.00% | 0.20 | 1.01 | 22,485 | 432 | 0.00% | 0.05 | 1.00 |
| asks whatever it finds | 2.00% | x1, 10% | v5 = v4, one ask opens | 0.00% | 217 | 227 | 2.00% | 21.7 | 17.5 | 0.015 | 244 | 24.7 | 0.00% | 0.21 | 0.99 | 20,260 | 433 | 0.00% | 0.05 | 1.00 |
| asks whatever it finds | 5.00% | x1, 10% | v0 as written | 0.00% | 316 | 330 | 5.00% | 31.6 | 25.4 | 0.010 | 355 | 24.8 | 0.00% | 0.21 | 1.00 | 29,517 | 431 | 0.00% | 0.05 | 1.00 |
| asks whatever it finds | 5.00% | x1, 10% | v1 down to R/2 | 0.00% | 15.7 | 17.3 | 20.04% | 1.57 | 1.35 | 0.194 | 17.6 | 24.7 | 0.00% | 0.32 | 1.00 | 1,456 | 432 | 0.00% | 0.06 | 1.00 |
| asks whatever it finds | 5.00% | x1, 10% | v2 cap follows asks | 0.00% | 317 | 331 | 5.00% | 31.7 | 25.4 | 0.010 | 355 | 24.7 | 0.00% | 0.20 | 1.00 | 29,545 | 431 | 0.00% | 0.05 | 1.00 |
| asks whatever it finds | 5.00% | x1, 10% | v5 = v4, one ask opens | 0.00% | 316 | 330 | 5.00% | 31.6 | 25.4 | 0.010 | 354 | 25.2 | 0.00% | 0.20 | 1.02 | 29,480 | 432 | 0.00% | 0.05 | 1.00 |
| asks whatever it finds | 25.00% | x1, 10% | v0 as written | 0.00% | 341 | 357 | 25.00% | 34.1 | 27.4 | 9.4e-3 | 383 | 24.9 | 0.00% | 0.21 | 1.00 | 31,878 | 432 | 0.00% | 0.05 | 1.00 |
| asks whatever it finds | 25.00% | x1, 10% | v1 down to R/2 | 0.00% | 53.6 | 56.8 | 24.98% | 5.36 | 4.39 | 0.059 | 59.9 | 24.6 | 0.00% | 0.24 | 0.99 | 5,000 | 431 | 0.00% | 0.05 | 1.00 |
| asks whatever it finds | 25.00% | x1, 10% | v2 cap follows asks | 0.00% | 341 | 357 | 25.00% | 34.1 | 27.4 | 9.4e-3 | 383 | 24.9 | 0.00% | 0.21 | 1.00 | 31,878 | 432 | 0.00% | 0.05 | 1.00 |
| asks whatever it finds | 25.00% | x1, 10% | v5 = v4, one ask opens | 0.00% | 341 | 356 | 25.00% | 34.1 | 27.4 | 9.4e-3 | 383 | 24.9 | 0.00% | 0.20 | 1.01 | 31,841 | 432 | 0.00% | 0.05 | 1.00 |
| never asks | 50.00% | x10, 30% | v0 as written | 0.00% | 341 | 334 | 25.00% | 34.1 | 27.4 | 0.377 | 9.29 | 4.75 | 2.70% | 0.56 | 0.96 | 795 | 206 | 0.00% | 0.08 | 1.00 |
| never asks | 50.00% | x10, 30% | v1 down to R/2 | 0.00% | 53.7 | 53.4 | 25.02% | 5.37 | 4.40 | 2.35 | 1.47 | 4.48 | 24.30% | 0.90 | 0.98 | 125 | 200 | 0.00% | 0.11 | 1.00 |
| never asks | 50.00% | x10, 30% | v2 cap follows asks | 0.00% | 341 | 334 | 25.00% | 34.1 | 27.4 | 0.377 | 9.29 | 4.75 | 2.70% | 0.56 | 0.96 | 795 | 206 | 0.00% | 0.08 | 1.00 |
| never asks | 50.00% | x10, 30% | v5 = v4, one ask opens | 0.00% | 342 | 336 | 25.00% | 34.2 | 27.4 | 0.375 | 9.34 | 4.71 | 3.80% | 0.58 | 0.95 | 799 | 205 | 0.00% | 0.08 | 1.00 |
| never asks | 90.00% | x10, 30% | v0 as written | 0.00% | 316 | 311 | 5.00% | 31.6 | 25.4 | 0.404 | 8.64 | 4.75 | 2.10% | 0.56 | 0.96 | 740 | 206 | 0.00% | 0.08 | 1.00 |
| never asks | 90.00% | x10, 30% | v1 down to R/2 | 0.00% | 14.7 | 15.4 | 5.00% | 1.47 | 1.28 | 8.18 | 0.409 | 1.30 | 66.50% | 1.74 | 0.99 | 34.6 | 106 | 0.00% | 0.19 | 1.00 |
| never asks | 90.00% | x10, 30% | v2 cap follows asks | 0.00% | 317 | 311 | 5.00% | 31.7 | 25.5 | 0.404 | 8.64 | 4.78 | 2.30% | 0.56 | 0.96 | 741 | 205 | 0.00% | 0.08 | 1.00 |
| never asks | 90.00% | x10, 30% | v5 = v4, one ask opens | 0.00% | 316 | 310 | 5.00% | 31.6 | 25.3 | 0.406 | 8.59 | 4.76 | 2.90% | 0.57 | 0.96 | 736 | 206 | 0.00% | 0.08 | 1.00 |
| never asks | 97.00% | x10, 30% | v0 as written | 0.00% | 143 | 140 | 1.50% | 14.3 | 11.5 | 0.895 | 3.83 | 4.81 | 7.10% | 0.70 | 0.97 | 333 | 206 | 0.00% | 0.09 | 1.00 |
| never asks | 97.00% | x10, 30% | v1 down to R/2 | 0.00% | 8.29 | 9.13 | 1.49% | 0.829 | 0.766 | 13.8 | 0.239 | 0.813 | 77.70% | 2.22 | 1.05 | 19.6 | 63.8 | 0.00% | 0.24 | 1.00 |
| never asks | 97.00% | x10, 30% | v2 cap follows asks | 0.00% | 95.1 | 94.2 | 1.50% | 9.51 | 7.71 | 1.34 | 2.55 | 4.62 | 12.30% | 0.77 | 0.94 | 223 | 206 | 0.00% | 0.10 | 1.00 |
| never asks | 97.00% | x10, 30% | v5 = v4, one ask opens | 0.00% | 127 | 125 | 1.50% | 12.7 | 10.3 | 1.00 | 3.41 | 4.67 | 7.50% | 0.70 | 0.94 | 297 | 207 | 0.00% | 0.09 | 1.00 |
| never asks | 99.00% | x10, 30% | v0 as written | 0.00% | 57.3 | 57.0 | 0.50% | 5.73 | 4.69 | 2.20 | 1.56 | 4.68 | 22.10% | 0.95 | 0.96 | 134 | 202 | 0.00% | 0.11 | 1.00 |
| never asks | 99.00% | x10, 30% | v1 down to R/2 | 0.00% | 7.44 | 8.29 | 0.50% | 0.744 | 0.698 | 15.2 | 0.214 | 0.688 | 79.80% | 2.43 | 0.97 | 17.6 | 58.8 | 0.00% | 0.26 | 1.01 |
| never asks | 99.00% | x10, 30% | v2 cap follows asks | 0.00% | 39.1 | 39.2 | 0.50% | 3.91 | 3.23 | 3.21 | 1.09 | 3.38 | 36.20% | 1.16 | 1.00 | 91.3 | 193 | 0.00% | 0.12 | 1.00 |
| never asks | 99.00% | x10, 30% | v5 = v4, one ask opens | 0.00% | 44.9 | 44.8 | 0.50% | 4.49 | 3.69 | 2.81 | 1.22 | 3.62 | 30.80% | 1.01 | 0.94 | 105 | 195 | 0.00% | 0.12 | 1.00 |
| industrial miners at the minimum | - | x10, 30% | today | 0.00% | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 3.63 | 0.962 | 2.95 | 41.00% | 1.18 | 0.99 | 80.6 | 191 | 0.00% | 0.13 | 1.00 |
| industrial miners at the minimum | - | x10, 30% | v0 as written | 0.00% | 344 | 337 | 49.99% | 34.4 | 27.6 | 0.373 | 9.38 | 4.91 | 2.70% | 0.56 | 0.99 | 802 | 206 | 0.00% | 0.08 | 1.00 |
| industrial miners at the minimum | - | x10, 30% | v1 down to R/2 | 0.00% | 53.9 | 53.6 | 50.02% | 5.39 | 4.41 | 2.35 | 1.47 | 4.45 | 25.40% | 0.95 | 0.97 | 126 | 201 | 0.00% | 0.12 | 1.00 |
| industrial miners at the minimum | - | x10, 30% | v2 cap follows asks | 0.00% | 344 | 337 | 49.99% | 34.4 | 27.6 | 0.373 | 9.38 | 4.91 | 2.70% | 0.56 | 0.99 | 802 | 206 | 0.00% | 0.08 | 1.00 |
| industrial miners at the minimum | - | x10, 30% | v5 = v4, one ask opens | 0.00% | 343 | 337 | 50.00% | 34.3 | 27.6 | 0.373 | 9.37 | 4.89 | 1.80% | 0.56 | 0.99 | 801 | 205 | 0.00% | 0.08 | 1.00 |
| carries no asking weak share | 25.00% | x10, 30% | v0 as written | 0.00% | 250 | 246 | 50.00% | 25.0 | 20.1 | 0.511 | 6.87 | 4.75 | 3.00% | 0.58 | 0.96 | 585 | 206 | 0.00% | 0.08 | 1.00 |
| carries no asking weak share | 25.00% | x10, 30% | v1 down to R/2 | 0.00% | 20.5 | 21.1 | 49.99% | 2.05 | 1.74 | 5.97 | 0.571 | 1.79 | 57.70% | 1.52 | 0.99 | 48.1 | 143 | 0.00% | 0.17 | 1.00 |
| carries no asking weak share | 25.00% | x10, 30% | v2 cap follows asks | 0.00% | 231 | 227 | 50.01% | 23.1 | 18.6 | 0.554 | 6.30 | 4.76 | 3.70% | 0.63 | 0.96 | 539 | 205 | 0.00% | 0.08 | 1.00 |
| carries no asking weak share | 25.00% | x10, 30% | v5 = v4, one ask opens | 0.00% | 231 | 227 | 50.01% | 23.1 | 18.5 | 0.555 | 6.29 | 4.72 | 4.00% | 0.61 | 0.95 | 538 | 206 | 0.00% | 0.08 | 1.00 |
| carries no asking weak share | 50.00% | x10, 30% | v0 as written | 0.00% | 168 | 165 | 50.00% | 16.8 | 13.5 | 0.762 | 4.50 | 4.71 | 5.80% | 0.67 | 0.95 | 392 | 207 | 0.00% | 0.09 | 1.00 |
| carries no asking weak share | 50.00% | x10, 30% | v1 down to R/2 | 0.00% | 16.8 | 17.4 | 50.02% | 1.68 | 1.45 | 7.21 | 0.476 | 1.51 | 61.30% | 1.61 | 1.01 | 39.3 | 119 | 0.00% | 0.18 | 1.00 |
| carries no asking weak share | 50.00% | x10, 30% | v2 cap follows asks | 0.00% | 139 | 137 | 50.00% | 13.9 | 11.2 | 0.917 | 3.73 | 4.69 | 7.00% | 0.70 | 0.95 | 325 | 206 | 0.00% | 0.09 | 1.00 |
| carries no asking weak share | 50.00% | x10, 30% | v5 = v4, one ask opens | 0.00% | 140 | 138 | 49.99% | 14.0 | 11.3 | 0.913 | 3.75 | 4.79 | 6.10% | 0.70 | 0.97 | 327 | 207 | 0.00% | 0.09 | 1.00 |
| carries no asking weak share | 75.00% | x10, 30% | v0 as written | 0.00% | 107 | 106 | 50.01% | 10.7 | 8.66 | 1.19 | 2.87 | 4.73 | 11.10% | 0.73 | 0.96 | 251 | 205 | 0.00% | 0.10 | 1.00 |
| carries no asking weak share | 75.00% | x10, 30% | v1 down to R/2 | 0.00% | 15.3 | 15.9 | 50.00% | 1.53 | 1.33 | 7.89 | 0.432 | 1.39 | 65.60% | 1.79 | 1.01 | 35.9 | 110 | 0.00% | 0.19 | 1.00 |
| carries no asking weak share | 75.00% | x10, 30% | v2 cap follows asks | 0.00% | 85.8 | 85.0 | 50.00% | 8.58 | 6.96 | 1.48 | 2.29 | 4.57 | 13.40% | 0.81 | 0.93 | 201 | 206 | 0.00% | 0.10 | 1.00 |
| carries no asking weak share | 75.00% | x10, 30% | v5 = v4, one ask opens | 0.00% | 85.9 | 85.1 | 50.01% | 8.59 | 6.97 | 1.48 | 2.30 | 4.74 | 14.30% | 0.81 | 0.96 | 201 | 206 | 0.00% | 0.10 | 1.00 |
| carries no asking weak share | 90.00% | x10, 30% | v0 as written | 1.15% | 83.1 | 83.1 | 49.42% | 8.41 | 6.83 | 1.51 | 2.23 | 4.71 | 14.80% | 0.83 | 0.95 | 194 | 206 | 0.00% | 0.10 | 1.00 |
| carries no asking weak share | 90.00% | x10, 30% | v1 down to R/2 | 0.36% | 13.6 | 14.3 | 49.79% | 1.36 | 1.19 | 8.79 | 0.380 | 1.18 | 68.70% | 1.86 | 0.96 | 31.9 | 98.7 | 0.00% | 0.20 | 1.00 |
| carries no asking weak share | 90.00% | x10, 30% | v2 cap follows asks | 0.59% | 67.8 | 67.8 | 49.71% | 6.82 | 5.56 | 1.85 | 1.83 | 4.81 | 17.40% | 0.84 | 0.98 | 159 | 204 | 0.00% | 0.10 | 1.00 |
| carries no asking weak share | 90.00% | x10, 30% | v5 = v4, one ask opens | 0.58% | 67.9 | 67.8 | 49.72% | 6.83 | 5.56 | 1.85 | 1.83 | 4.66 | 18.90% | 0.85 | 0.95 | 159 | 204 | 0.00% | 0.10 | 1.00 |


