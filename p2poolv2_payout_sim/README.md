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
`weak-depth` and `r-max`. Every run is a pure function of its inputs and
seed (default 1), drawn from `p2poolv2_netsim`'s seeded xoshiro256**
generator, so the tables below come out the same on any machine. The whole
default run, 1,000 simulated months per scenario, took 26 seconds on the
32-thread machine it was run on.

## The model

### Bitcoin Cash and the pool

- Bitcoin Cash's hashrate `H` is 3.7 EH/s (`docs/small-miners.md`), or 3,
  10 or 270 times that; blocks every 600 seconds; 3.125 BCH a block, fees
  left out; 1% donation on every share (`cuts-per-share`, so a payee gets
  99% of its slice).
- The pool holds a fraction `f` of `H` (1, 5, 10, 20 or 30%) and finds
  Bitcoin Cash blocks as a Poisson process at `f / 600` a second. Under
  `mini` each chain finds blocks at its own part of `H`.
- A month is 30 days (4,320 Bitcoin Cash blocks). A miner's fair income is
  its share of `H` of a month's blocks, less the donation: 433,459 sat for
  the Bitaxe today.

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
  any other period. In steady state that is a fixed span of time.
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
   times the minimum, sized to make that many. The table also reports where
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
   shares at 1% of BCH, the Bitaxe gets 1.80 times its fair income and the
   farms and the ASIC 0.91; at 10% of BCH, where the ASIC is at the minimum
   too, both get 1.28 and the 10 PH/s farms pay for it. The window also
   shrinks, to 34 hours instead of 50 at 1%. It makes ignoring the node
   default profitable (attack a below): with the farms at the minimum, an
   ASIC that follows the default gets 0.51 of fair. Crediting a share on a
   minimum job the weak work of its parent's weak target, like a weak
   share (`weak work` rows), makes every hash count once: every miner is
   within noise of fair, the window is 50 hours again, and the Bitaxe's
   monthly swing at 10% and 30% of BCH falls from 0.67 and 1.33 to 0.25 and
   0.39, because its income no longer comes half in rare full-share lumps.
   This is a share-chain rule change the page does not make; it is the
   owner's decision.
2. **Bitaxe, 50% industrial** (swing / payments a month / fee loss):

   | Design | 1% of BCH | 10% of BCH | 30% of BCH |
   |---|---|---|---|
   | mini | 0.34 / 19.7 / 0.67% | 0.91 / 4.88 / 0.17% | 1.58 / 1.59 / 0.06% |
   | one-chain | 0.41 / 19.5 / 0.67% | 1.31 / 2.41 / 0.09% | 2.39 / 0.76 / 0.03% |
   | band | 0.32 / 29.8 / 1.02% | 1.09 / 3.21 / 0.11% | 2.39 / 0.76 / 0.03% |
   | vardiff-cd | 0.31 / 30.1 / 1.03% | 0.90 / 4.84 / 0.17% | 1.61 / 1.60 / 0.06% |
   | band-weak | 0.23 / 43.4 / 1.11% | 0.62 / 64.2 / 1.95% | 1.36 / 14.9 / 0.54% |
   | band-weak-levels | 0.23 / 28.5 / 0.74% | 0.67 / 15.6 / 0.46% | 1.33 / 12.1 / 0.42% |
   | band-weak-levels, weak work | 0.20 / 24.9 / 0.85% | 0.25 / 24.6 / 0.85% | 0.39 / 23.6 / 0.83% |

   Without weak shares a Bitaxe at 30% of BCH has no payment in 78% of
   months on one chain, 60% on mini and 61% under vardiff-cd. With weak
   shares and levels, and the weak-work credit, it is paid about 24 times
   every month at every pool size, each payment about the 17,472-sat level,
   with 0.85% fee loss, which is 148 over the level.
3. **ASIC, 50% industrial** (same columns): one-chain 0.16 / 43 / 0.02% at
   1%, 0.14 / 170 / 0.07% at 10%, 0.23 / 70 / 0.03% at 30%; band-weak-levels
   with the weak-work credit 0.16 / 43 / 0.02%, 0.05 / 431 / 0.18%,
   0.05 / 1,039 / 0.43%. Under vardiff-cd every miner makes shares at the
   same rate, so the ASIC gets about 19 payments a month at 5% and above and
   its swing rises to 0.46 at 10% and 30%. Under `mini` in the `home` mix
   the ASIC is alone on the main chain and paid only when it finds a block
   itself: 0.12 payments a month.
4. **The band alone** helps only where heavy miners would make more than
   the node default's target: at 1% of BCH with 90% industrial the minimum
   falls to 0.11 of the uniform difficulty and the Bitaxe's swing from 0.41
   to 0.19; at 30% every miner is under the target and the band changes
   nothing. **The page's ABLA-like rule for `C` leaves 1 only when more than
   half of all chain shares want heavier shares** (at `C` = 1 every share
   that does not want more pulls `C` down as much as a capped one pushes it
   up). It settles at the limit with 90% industrial at 1% of BCH, and stays
   at 1.00 in the 50% industrial mix at every size and with 90% industrial at
   10% and 30%, where a fixed cap of 8 lowers the minimum to 0.56 (50%
   industrial, 1%), 0.80 (50%, 10%) and 0.62 (90%, 10%) of uniform.
5. **Weak shares cost about 2.9 KB/s** of messages at `WEAK_CAP` 64 and
   `WEAK_FILL` 32, against 0.1 KB/s of shares, at any pool size: the weak
   ratio settles at about 34.5 weak shares per share (the rule tracks the
   median carried count, which sits a little above `WEAK_FILL`), and a
   quarter of shares carry the full 64 with the rest carried later; 0.01 to
   0.02% expire.
6. **Shorter share periods** help one chain (Bitaxe swing at 10% of BCH 1.31
   at 10 s, 0.36 at 1 s, with 1.2 shares and 0.19 uncles a second) but add
   little once weak shares carry the credit (0.25 at 10 s, 0.21 at 1 s with
   the weak-work credit, at ten times the bytes).
7. **Attack (a), the farms' node at the minimum:** under the band the
   minimum returns to uniform and the Bitaxe gets one-chain's numbers, no
   worse than today, as the page says. With weak shares and the page's
   credit the farms gain and the ASIC that follows the default falls to
   0.51 of fair; with the weak-work credit nobody's income changes and the
   Bitaxe's swing rises only from 0.19 to 0.21 (1%) and 0.24 to 0.27 (10%).
8. **Attack (b), non-carrying nodes:** with half the hashrate on them 4.8 to
   5.0% of honest weak shares expire; with 90%, 56%. Under the weak-work
   credit the honest Bitaxe then gets 0.97 and 0.48 of fair: weak shares are
   most of its credit, so losing them costs it income, not only variance.
   A lone non-carrying ASIC loses its includer income (0.98 of fair at 10%
   of BCH and a 2% cut, 0.95 at 5%). A coalition of half the hashrate gains
   about 1% by not carrying at 1% of BCH whatever the cut (its ASIC is
   heavy and carries little), and at 10% gains 2% with no cut, 1% at 1 to
   2% and nothing at 5%.
9. **Attack (c), weak share spam** (one miner at the minimum with 1x or 10x
   the honest home hashrate, added to the pool): `R` hardly moves (35 to
   44), since chain shares already came mostly from miners at the minimum,
   but the spam raises the minimum itself, so the honest Bitaxe makes fewer
   full shares and so fewer weak shares: they fall 11 times (581 to 53 a
   month at 1% of BCH, 50% industrial). With the weak-work credit its
   payments stay at 25 a month, its swing goes from 0.20 to 0.23 and its
   income stays fair.
   The spam is paid for its work and takes nothing else.

### Recommended constants

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
  the attack search.
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
above), `INCLUDER_CUT` 5% unless a table sweeps it, 50% industrial unless a
table says otherwise.

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
- **Constants**. `weak-depth` sweeps `WEAK_DEPTH` (12, 24, 48) and
  `INCLUDER_CUT` (0, 1, 5%) against 50% and 90% refusing coalitions;
  `r-max` sweeps `R_MAX` (256, 1,024, 4,096) from genesis.

### What the study tables show

1. **Weak work in fork choice changes little through races alone.**
   Refusing shares now lose their races to carrying ones (stale 3.06%
   against 1.15% for honest shares, 1% of BCH, half refusing), but races
   are 2% of shares, so honest weak shares lost go only from 4.92% to 4.65%
   (half refusing) and from 56.2% to 55.9% (90%). A coalition of half the
   hashrate still gains 1.1 to 1.7% with no cut and 0.2 to 0.3% at 5%; one of
   90% gains 2.6 to 4.3% at every cut: at most 0.12 points less than under
   today's fork choice.
2. **It bites when carrying nodes mine siblings of light tips.** A
   half-hashrate coalition then loses 5.4 to 9.6% at every cut, 0% included:
   its light tips are replaced at 0.28 to 0.30 per share. A 90% coalition
   still gains 2.2 to 3.8%, since carriers find the next share only a tenth
   of the time. Shares heavier than the minimum are never replaced, so a
   coalition member on heavy shares (the refusing ASIC at 1% of BCH) gains 6%
   while its coalition loses.
3. **`WEAK_DEPTH` is the lever that stops a 90% coalition.** With weak work
   in races and a 1% cut, the coalition's gain falls from +2.7 / +4.2% at
   depth 6 (1% / 10% of BCH) to +1.4 / +2.2% at 12, +0.4 / +0.7% at 24 and
   +0.03 / +0.04% at 48, where honest weak shares lost fall from 56% to 1.2%
   and the Bitaxe gets 0.99 of fair instead of 0.48. From depth 12 a
   half-hashrate coalition gains at most 0.08%. At depth 48 a 5% cut makes
   a 90% coalition lose 0.1%; with carriers mining siblings it loses 0.3 to
   0.6% at every cut. A lone refuser loses its includer income: nothing at 0%, 0.2 /
   0.9% at 1%, 1.0 / 4.6% at 5%. Every miner's income is fair at every cut
   when everyone carries (the cut a home miner pays on its weak shares comes
   back on the weak shares its own shares carry), and the Bitaxe's swing
   does not move with the cut.
4. **Miner mixes.** In the Zipf spread 30% of the hashrate is at the minimum
   at 1% of BCH and 82% at 30%, `R` 49 to 38, and the Bitaxe's swing is
   0.20 / 0.26 / 0.35 at 1 / 10 / 30% (one chain: 0.41 / 1.31 / 2.39). With
   mostly medium miners everyone but the home miners mines heavier shares at
   1% (minimum 0.22 of uniform, `R` 75), and everyone is at the minimum at
   10 and 30% (`R` 34.7); the Bitaxe's swing is 0.21 / 0.28 / 0.40. In every
   mix and size the Bitaxe gets 23 to 25 payments a month and 0.97 to 1.00
   of fair.
5. **A big miner joining or leaving.** The minimum and `R` are within 10% of
   their new levels within an hour in every case, and `R` hardly moves (35
   to 42): it follows the share of chain shares at the minimum. `C`, under
   the page's `wanted_bits` rule, is 45 at 1% of BCH and 1.5 at 10% with the
   50% industrial mix; a big miner on the node default lifts it to 61 / 55 within
   2 hours, and when it leaves `C` is back within 1 / 13 hours; a big miner
   at the minimum joining at 1% brings `C` from 45 to 12 in 8 hours. (The
   `C settles at` column of the band cap table uses the earlier
   bits-driven step and gives 1.00 for this mix.) Home miners' pay swings
   with the window, not with `R` or `C`: when the pool doubles they get 2.0
   times fair in the first hour, 1.9 to 6 hours and 1.4 to a day at 1% of BCH
   (1.9 and 1.1 at 10%), and when it halves 0.5, 0.55, 0.66 and 0.86 until
   day 2 (back by 6 hours at 10%), the same whether the big miner is on the
   node default or at the minimum. Weak shares expiring meanwhile: 0.03% at
   most.
6. **Tiny pool.** At the ceiling `R` is 1.00 and no weak share is found, and
   the page's design with every miner at the minimum pays exactly as one
   chain, as does the band alone. But the node default still gives every
   miner 64 times the ceiling (each would make far more than 300 shares a
   month), so shares fall from 0.051 to 0.0008 a second; payments stay at
   117 a month and the swing goes from 0.09 to 0.10. The rows with levels
   differ from one chain only because estimating the level pots draws from
   the random stream: every slice there is far above every level.
7. **Floods do not hurt small miners' income.** The flood takes 91 to
   99.9% of the weak shares and the Bitaxe's fall from 9,718 to 9.5 a month
   (1,000 home miners, 1,000 times), but the flood brings blocks: the
   Bitaxe's payments rise from 1.45 to 14 (10 times) and 22 to 25 a month,
   its swing falls from 0.81 to between 0.22 and 0.39, and its income stays
   fair. At 90% industrial its swing goes from 0.20 to 0.27 to 0.29 at 100
   times. The flood is paid fair, and its cuts paid less cuts received are
   at most 0.14% of its income: it carries nearly all weak shares, so it
   gets its cuts back.
8. **Steering the draw does not pay.** Between racing shares it wins at
   most 28 sat a month (the ASIC at level 7 at 30% of BCH), because a miner
   steers only the blocks it finds (0.0014 a month for the Bitaxe). Picking
   the tip's parent every time wins at most 1,308 sat a month and loses 0.5
   to 1.2% of income to the stale shares it finds there.
9. **Losses.** Weak shares on a parent that leaves the chain, at half the
   stale rate, lose 1.05% of weak shares at 10 s and 4.35% at 2 s; the
   Bitaxe gets 0.99 to 1.00 and 0.98 of fair (the loss goes to heavier
   shares) with unchanged payments and swing, since `R` rises to fill the
   slots again.
   A race's loser dropping its weak shares loses none when every node
   carries: the winner carried the same oldest ones.

### Recommended constants (study)

- `WEAK_DEPTH` = 48 (8 minutes at 10 s): the only tested change that stops
  a 90% refusing coalition; its gain falls from +2.7 / +4.2% at 6 to
  +0.03 / +0.04% at a 1% cut, and the Bitaxe keeps 0.99 of fair instead of
  0.48.
- `INCLUDER_CUT` = 1%: at `WEAK_DEPTH` 48 refusing gains a 90% coalition at
  most 0.04%, costs a half-hashrate coalition 0.2 to 0.3% and a lone refuser
  0.2 to 0.9%; with races alone only 5% makes refusing never pay (a 90%
  coalition loses 0.1%), and with carriers mining siblings 0% does.
- `R_MAX` = 256: with `WEAK_FILL` 32, `R` settles between 34 and 75 in every
  table, and from genesis 256 brings `R` within 10% in 2 hours instead of 4,
  with 11 weak shares a second in the first hour (9 KB/s, half expiring)
  instead of 174 (139 KB/s, 95% expiring).
- Weak work in fork choice: worth the header-sync work only together with
  carriers mining siblings of light tips; through races alone it moves a
  coalition's gain by 0.12 points at most.

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
- What a deep `WEAK_DEPTH` costs a node: memory for about 48 times the weak
  shares per share, and weak shares whose Bitcoin Cash prev goes stale while
  they wait.
- `R_MAX` in a pool with almost no hashrate at the minimum, where it would
  bind; `WEAK_BYTES` and the band step.

## Code

| What | Where |
|---|---|
| Constants and their sources | `src/constants.rs` |
| Miner classes, mixes, tracked miners | `src/miners.rs` |
| Steady-state difficulties: uniform, band, ABLA settle, vardiff | `src/difficulty.rs` |
| The window sized by work | `src/window.rs` |
| The weak share queue and the weak ratio | `src/weak.rs` |
| Payout levels and the draw | `src/levels.rs` |
| Tracked miners' payouts block by block | `src/payouts.rs` |
| Scenarios, credit, reports | `src/scenario.rs` |
| Share races, fork choice and siblings of light tips (study) | `src/races.rs` |
| A big miner joining or leaving, and genesis, share by share (study) | `src/transition.rs` |
| The tables | `src/tables.rs`, `src/study.rs`, `src/report.rs` |
| Command line | `src/bin/payout_sim.rs` |

The crate is new code for the Bitcoin Cash fork, under AGPL-3.0-or-later
(see `LICENSE`), like `p2poolv2_netsim`, whose generator, number formats and
share chain constants it reuses. It does not link `p2poolv2_lib`.

## Tables from the default run

### Designs at each pool size

#### Mix: home

| Design | Pool | Bitaxe pay/mo | Bitaxe median sat | Bitaxe swing | Bitaxe empty months | Bitaxe fee loss | Bitaxe vs fair | ASIC pay/mo | ASIC median sat | ASIC swing | ASIC empty months | ASIC fee loss | ASIC vs fair |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| mini | 1% | 19.4 | 16,816 | 0.41 | 0.00% | 0.67% | 0.99 | 0.117 | 309,375,000 | 2.90 | 88.90% | 0.00% | 1.00 |
| mini | 5% | 4.89 | 84,245 | 0.87 | 21.80% | 0.17% | 0.95 | 0.117 | 309,375,000 | 2.90 | 88.90% | 0.00% | 1.00 |
| mini | 10% | 2.44 | 168,489 | 1.26 | 45.90% | 0.09% | 0.95 | 0.117 | 309,375,000 | 2.90 | 88.90% | 0.00% | 1.00 |
| mini | 20% | 1.14 | 336,840 | 1.84 | 68.70% | 0.04% | 0.89 | 0.117 | 309,375,000 | 2.90 | 88.90% | 0.00% | 1.00 |
| mini | 30% | 0.763 | 505,008 | 2.30 | 79.00% | 0.03% | 0.89 | 0.117 | 309,375,000 | 2.90 | 88.90% | 0.00% | 1.00 |
| one-chain | 1% | 19.5 | 16,864 | 0.41 | 0.00% | 0.67% | 1.00 | 43.4 | 826,323 | 0.16 | 0.00% | 0.02% | 1.00 |
| one-chain | 5% | 4.90 | 84,290 | 0.92 | 20.90% | 0.17% | 0.96 | 187 | 168,626 | 0.12 | 0.00% | 0.08% | 1.01 |
| one-chain | 10% | 2.41 | 168,580 | 1.31 | 45.60% | 0.09% | 0.94 | 170 | 168,580 | 0.14 | 0.00% | 0.07% | 1.00 |
| one-chain | 20% | 1.14 | 336,931 | 1.87 | 69.20% | 0.04% | 0.89 | 102 | 336,931 | 0.20 | 0.00% | 0.04% | 1.00 |
| one-chain | 30% | 0.755 | 505,099 | 2.39 | 78.30% | 0.03% | 0.88 | 70.0 | 505,099 | 0.23 | 0.00% | 0.03% | 1.00 |
| band | 1% | 19.3 | 16,839 | 0.42 | 0.00% | 0.67% | 0.99 | 43.1 | 839,515 | 0.16 | 0.00% | 0.02% | 1.00 |
| band | 5% | 4.90 | 84,290 | 0.92 | 20.90% | 0.17% | 0.96 | 187 | 168,626 | 0.12 | 0.00% | 0.08% | 1.01 |
| band | 10% | 2.41 | 168,580 | 1.31 | 45.60% | 0.09% | 0.94 | 170 | 168,580 | 0.14 | 0.00% | 0.07% | 1.00 |
| band | 20% | 1.14 | 336,931 | 1.87 | 69.20% | 0.04% | 0.89 | 102 | 336,931 | 0.20 | 0.00% | 0.04% | 1.00 |
| band | 30% | 0.755 | 505,099 | 2.39 | 78.30% | 0.03% | 0.88 | 70.0 | 505,099 | 0.23 | 0.00% | 0.03% | 1.00 |
| vardiff-cd | 1% | 19.6 | 16,862 | 0.40 | 0.00% | 0.67% | 1.00 | 19.8 | 1,396,005 | 0.41 | 0.10% | 0.01% | 1.01 |
| vardiff-cd | 5% | 4.93 | 84,292 | 0.90 | 21.80% | 0.17% | 0.96 | 18.5 | 1,862,891 | 0.46 | 0.00% | 0.01% | 0.99 |
| vardiff-cd | 10% | 2.36 | 168,539 | 1.27 | 45.50% | 0.09% | 0.92 | 18.9 | 1,862,564 | 0.45 | 0.10% | 0.01% | 1.00 |
| vardiff-cd | 20% | 1.20 | 336,894 | 1.83 | 68.50% | 0.04% | 0.93 | 18.9 | 1,862,668 | 0.45 | 0.40% | 0.01% | 0.98 |
| vardiff-cd | 30% | 0.766 | 505,066 | 2.26 | 78.60% | 0.03% | 0.89 | 19.0 | 1,862,940 | 0.45 | 0.40% | 0.01% | 0.99 |
| band-weak | 1% | 43.3 | 5,667 | 0.28 | 0.00% | 1.48% | 1.00 | 43.3 | 405,076 | 0.17 | 0.00% | 0.03% | 0.51 |
| band-weak | 5% | 74.3 | 2,374 | 0.52 | 0.00% | 2.65% | 0.96 | 216 | 161,946 | 0.09 | 0.00% | 0.09% | 1.00 |
| band-weak | 10% | 43.0 | 4,748 | 0.70 | 0.00% | 1.54% | 0.95 | 432 | 42,733 | 0.09 | 0.00% | 0.18% | 1.00 |
| band-weak | 20% | 22.5 | 9,495 | 1.07 | 0.00% | 0.81% | 0.95 | 767 | 18,991 | 0.12 | 0.00% | 0.31% | 1.00 |
| band-weak | 30% | 14.9 | 14,242 | 1.36 | 0.00% | 0.54% | 0.95 | 807 | 14,243 | 0.14 | 0.00% | 0.33% | 1.00 |
| band-weak-levels | 1% | 19.9 | 20,009 | 0.30 | 0.00% | 0.69% | 0.99 | 43.1 | 405,076 | 0.17 | 0.00% | 0.03% | 0.51 |
| band-weak-levels | 5% | 14.4 | 17,472 | 0.54 | 0.00% | 0.50% | 0.99 | 215 | 161,947 | 0.09 | 0.00% | 0.09% | 1.00 |
| band-weak-levels | 10% | 12.9 | 17,473 | 0.77 | 0.10% | 0.45% | 0.99 | 429 | 42,733 | 0.09 | 0.00% | 0.18% | 1.00 |
| band-weak-levels | 20% | 12.3 | 17,473 | 1.04 | 0.00% | 0.43% | 0.98 | 673 | 28,486 | 0.11 | 0.00% | 0.28% | 1.00 |
| band-weak-levels | 30% | 12.2 | 17,474 | 1.32 | 0.00% | 0.43% | 0.98 | 720 | 17,474 | 0.14 | 0.00% | 0.29% | 1.01 |
| band-weak-levels, weak work | 1% | 25.0 | 17,472 | 0.21 | 0.00% | 0.85% | 1.01 | 43.1 | 845,480 | 0.16 | 0.00% | 0.02% | 1.00 |
| band-weak-levels, weak work | 5% | 24.5 | 17,472 | 0.23 | 0.00% | 0.85% | 0.99 | 215 | 167,012 | 0.07 | 0.00% | 0.09% | 1.00 |
| band-weak-levels, weak work | 10% | 24.6 | 17,472 | 0.27 | 0.00% | 0.85% | 0.99 | 432 | 82,857 | 0.05 | 0.00% | 0.18% | 1.00 |
| band-weak-levels, weak work | 20% | 24.2 | 17,472 | 0.33 | 0.00% | 0.84% | 0.98 | 830 | 37,980 | 0.05 | 0.00% | 0.34% | 1.00 |
| band-weak-levels, weak work | 30% | 23.5 | 17,472 | 0.38 | 0.00% | 0.83% | 0.97 | 1,038 | 28,485 | 0.05 | 0.00% | 0.43% | 1.00 |

#### Mix: 50% industrial

| Design | Pool | Bitaxe pay/mo | Bitaxe median sat | Bitaxe swing | Bitaxe empty months | Bitaxe fee loss | Bitaxe vs fair | ASIC pay/mo | ASIC median sat | ASIC swing | ASIC empty months | ASIC fee loss | ASIC vs fair |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| mini | 1% | 19.7 | 16,817 | 0.34 | 0.00% | 0.67% | 1.00 | 21.6 | 1,670,400 | 0.22 | 0.00% | 0.01% | 1.00 |
| mini | 5% | 9.69 | 42,128 | 0.62 | 4.10% | 0.34% | 0.98 | 108 | 337,429 | 0.12 | 0.00% | 0.04% | 1.00 |
| mini | 10% | 4.88 | 84,268 | 0.91 | 20.70% | 0.17% | 0.95 | 187 | 168,670 | 0.12 | 0.00% | 0.08% | 1.00 |
| mini | 20% | 2.43 | 168,512 | 1.24 | 46.40% | 0.09% | 0.94 | 169 | 168,602 | 0.15 | 0.00% | 0.07% | 1.00 |
| mini | 30% | 1.59 | 252,711 | 1.58 | 59.80% | 0.06% | 0.93 | 129 | 252,801 | 0.18 | 0.00% | 0.05% | 1.01 |
| one-chain | 1% | 19.5 | 16,864 | 0.41 | 0.00% | 0.67% | 1.00 | 43.4 | 826,323 | 0.16 | 0.00% | 0.02% | 1.00 |
| one-chain | 5% | 4.90 | 84,290 | 0.92 | 20.90% | 0.17% | 0.96 | 187 | 168,626 | 0.12 | 0.00% | 0.08% | 1.01 |
| one-chain | 10% | 2.41 | 168,580 | 1.31 | 45.60% | 0.09% | 0.94 | 170 | 168,580 | 0.14 | 0.00% | 0.07% | 1.00 |
| one-chain | 20% | 1.14 | 336,931 | 1.87 | 69.20% | 0.04% | 0.89 | 102 | 336,931 | 0.20 | 0.00% | 0.04% | 1.00 |
| one-chain | 30% | 0.755 | 505,099 | 2.39 | 78.30% | 0.03% | 0.88 | 70.0 | 505,099 | 0.23 | 0.00% | 0.03% | 1.00 |
| band | 1% | 29.8 | 8,599 | 0.32 | 0.00% | 1.02% | 1.00 | 43.1 | 839,538 | 0.16 | 0.00% | 0.02% | 1.00 |
| band | 5% | 8.06 | 50,637 | 0.68 | 5.50% | 0.29% | 0.95 | 208 | 151,938 | 0.10 | 0.00% | 0.09% | 1.00 |
| band | 10% | 3.21 | 134,656 | 1.09 | 36.80% | 0.11% | 1.00 | 201 | 134,656 | 0.13 | 0.00% | 0.08% | 1.01 |
| band | 20% | 1.16 | 333,163 | 1.86 | 68.80% | 0.04% | 0.89 | 102 | 333,163 | 0.20 | 0.00% | 0.04% | 1.00 |
| band | 30% | 0.755 | 505,099 | 2.39 | 78.30% | 0.03% | 0.88 | 70.0 | 505,099 | 0.23 | 0.00% | 0.03% | 1.00 |
| vardiff-cd | 1% | 30.1 | 8,455 | 0.31 | 0.00% | 1.03% | 1.00 | 30.1 | 700,105 | 0.30 | 0.00% | 0.01% | 0.99 |
| vardiff-cd | 5% | 10.2 | 42,294 | 0.63 | 3.80% | 0.34% | 1.01 | 18.4 | 1,862,891 | 0.46 | 0.10% | 0.01% | 0.99 |
| vardiff-cd | 10% | 4.84 | 84,681 | 0.90 | 20.60% | 0.17% | 0.95 | 18.8 | 1,862,564 | 0.46 | 0.10% | 0.01% | 0.99 |
| vardiff-cd | 20% | 2.32 | 169,734 | 1.27 | 46.50% | 0.09% | 0.91 | 19.1 | 1,862,668 | 0.44 | 0.20% | 0.01% | 0.99 |
| vardiff-cd | 30% | 1.60 | 255,161 | 1.61 | 60.90% | 0.06% | 0.94 | 18.7 | 1,862,940 | 0.46 | 0.20% | 0.01% | 0.97 |
| band-weak | 1% | 43.4 | 13,737 | 0.23 | 0.00% | 1.11% | 1.33 | 43.4 | 564,712 | 0.17 | 0.00% | 0.03% | 0.68 |
| band-weak | 5% | 145 | 1,194 | 0.37 | 0.00% | 3.79% | 1.31 | 216 | 211,637 | 0.08 | 0.00% | 0.07% | 1.33 |
| band-weak | 10% | 64.2 | 3,562 | 0.62 | 0.00% | 1.95% | 1.13 | 433 | 53,433 | 0.08 | 0.00% | 0.15% | 1.14 |
| band-weak | 20% | 33.3 | 7,181 | 1.00 | 0.00% | 1.06% | 1.08 | 832 | 21,544 | 0.11 | 0.00% | 0.30% | 1.14 |
| band-weak | 30% | 14.9 | 14,242 | 1.36 | 0.00% | 0.54% | 0.95 | 807 | 14,243 | 0.14 | 0.00% | 0.33% | 1.00 |
| band-weak-levels | 1% | 28.5 | 17,473 | 0.23 | 0.00% | 0.74% | 1.32 | 43.2 | 564,715 | 0.17 | 0.00% | 0.03% | 0.67 |
| band-weak-levels | 5% | 20.8 | 17,474 | 0.38 | 0.00% | 0.56% | 1.28 | 216 | 211,638 | 0.08 | 0.00% | 0.07% | 1.33 |
| band-weak-levels | 10% | 15.6 | 17,472 | 0.67 | 0.00% | 0.46% | 1.16 | 431 | 53,433 | 0.09 | 0.00% | 0.15% | 1.14 |
| band-weak-levels | 20% | 14.5 | 17,473 | 0.97 | 0.00% | 0.46% | 1.08 | 742 | 28,725 | 0.11 | 0.00% | 0.27% | 1.14 |
| band-weak-levels | 30% | 12.1 | 17,475 | 1.33 | 0.00% | 0.42% | 0.97 | 720 | 17,475 | 0.14 | 0.00% | 0.29% | 1.01 |
| band-weak-levels, weak work | 1% | 24.9 | 17,473 | 0.20 | 0.00% | 0.85% | 1.00 | 43.2 | 842,516 | 0.16 | 0.00% | 0.02% | 1.00 |
| band-weak-levels, weak work | 5% | 24.6 | 17,472 | 0.23 | 0.00% | 0.85% | 0.99 | 216 | 166,698 | 0.07 | 0.00% | 0.09% | 1.00 |
| band-weak-levels, weak work | 10% | 24.6 | 17,473 | 0.25 | 0.00% | 0.85% | 0.99 | 431 | 81,921 | 0.05 | 0.00% | 0.18% | 1.00 |
| band-weak-levels, weak work | 20% | 24.6 | 17,472 | 0.29 | 0.00% | 0.85% | 0.99 | 844 | 43,086 | 0.05 | 0.00% | 0.35% | 1.00 |
| band-weak-levels, weak work | 30% | 23.6 | 17,472 | 0.39 | 0.00% | 0.83% | 0.97 | 1,039 | 28,485 | 0.05 | 0.00% | 0.43% | 1.00 |

#### Mix: 90% industrial

| Design | Pool | Bitaxe pay/mo | Bitaxe median sat | Bitaxe swing | Bitaxe empty months | Bitaxe fee loss | Bitaxe vs fair | ASIC pay/mo | ASIC median sat | ASIC swing | ASIC empty months | ASIC fee loss | ASIC vs fair |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| mini | 1% | 4.27 | 100,373 | 0.51 | 1.40% | 0.15% | 0.99 | 39.0 | 924,473 | 0.16 | 0.00% | 0.02% | 1.00 |
| mini | 5% | 19.8 | 16,854 | 0.34 | 0.00% | 0.67% | 1.01 | 178 | 151,788 | 0.12 | 0.00% | 0.07% | 1.00 |
| mini | 10% | 19.4 | 16,858 | 0.42 | 0.00% | 0.67% | 0.99 | 178 | 151,739 | 0.14 | 0.00% | 0.07% | 1.00 |
| mini | 20% | 11.9 | 33,718 | 0.55 | 1.70% | 0.41% | 0.99 | 112 | 303,279 | 0.19 | 0.00% | 0.05% | 1.01 |
| mini | 30% | 8.15 | 50,576 | 0.68 | 5.80% | 0.29% | 0.98 | 77.7 | 454,672 | 0.23 | 0.00% | 0.03% | 1.01 |
| one-chain | 1% | 19.5 | 16,864 | 0.41 | 0.00% | 0.67% | 1.00 | 43.4 | 826,323 | 0.16 | 0.00% | 0.02% | 1.00 |
| one-chain | 5% | 4.90 | 84,290 | 0.92 | 20.90% | 0.17% | 0.96 | 187 | 168,626 | 0.12 | 0.00% | 0.08% | 1.01 |
| one-chain | 10% | 2.41 | 168,580 | 1.31 | 45.60% | 0.09% | 0.94 | 170 | 168,580 | 0.14 | 0.00% | 0.07% | 1.00 |
| one-chain | 20% | 1.14 | 336,931 | 1.87 | 69.20% | 0.04% | 0.89 | 102 | 336,931 | 0.20 | 0.00% | 0.04% | 1.00 |
| one-chain | 30% | 0.755 | 505,099 | 2.39 | 78.30% | 0.03% | 0.88 | 70.0 | 505,099 | 0.23 | 0.00% | 0.03% | 1.00 |
| band | 1% | 42.9 | 9,765 | 0.19 | 0.00% | 1.44% | 1.02 | 43.1 | 839,535 | 0.17 | 0.00% | 0.02% | 1.00 |
| band | 5% | 32.3 | 12,590 | 0.33 | 0.00% | 1.09% | 1.01 | 216 | 169,389 | 0.08 | 0.00% | 0.09% | 1.01 |
| band | 10% | 4.01 | 104,168 | 0.98 | 27.00% | 0.14% | 0.96 | 239 | 104,203 | 0.12 | 0.00% | 0.10% | 1.00 |
| band | 20% | 1.21 | 327,830 | 1.86 | 68.60% | 0.04% | 0.92 | 104 | 327,830 | 0.20 | 0.00% | 0.04% | 1.00 |
| band | 30% | 0.755 | 505,099 | 2.39 | 78.30% | 0.03% | 0.88 | 70.0 | 505,099 | 0.23 | 0.00% | 0.03% | 1.00 |
| vardiff-cd | 1% | 42.9 | 10,341 | 0.20 | 0.00% | 1.44% | 1.02 | 42.9 | 844,735 | 0.20 | 0.00% | 0.02% | 0.99 |
| vardiff-cd | 5% | 45.8 | 8,644 | 0.28 | 0.00% | 1.54% | 1.02 | 46.1 | 703,936 | 0.28 | 0.00% | 0.02% | 1.01 |
| vardiff-cd | 10% | 24.7 | 17,287 | 0.38 | 0.00% | 0.84% | 1.01 | 25.3 | 1,404,588 | 0.38 | 0.10% | 0.01% | 1.01 |
| vardiff-cd | 20% | 12.9 | 34,674 | 0.53 | 1.70% | 0.42% | 1.04 | 18.7 | 1,862,668 | 0.48 | 0.20% | 0.01% | 0.98 |
| vardiff-cd | 30% | 8.07 | 52,238 | 0.69 | 6.70% | 0.28% | 0.97 | 19.0 | 1,862,940 | 0.44 | 0.00% | 0.01% | 0.99 |
| band-weak | 1% | 43.3 | 18,016 | 0.17 | 0.00% | 0.81% | 1.82 | 43.3 | 759,818 | 0.16 | 0.00% | 0.02% | 0.91 |
| band-weak | 5% | 216 | 1,917 | 0.19 | 0.00% | 4.10% | 1.80 | 216 | 157,129 | 0.08 | 0.00% | 0.10% | 0.93 |
| band-weak | 10% | 94.3 | 2,614 | 0.53 | 0.00% | 2.56% | 1.26 | 431 | 62,755 | 0.08 | 0.00% | 0.14% | 1.28 |
| band-weak | 20% | 50.7 | 5,280 | 0.98 | 0.00% | 1.37% | 1.26 | 858 | 26,403 | 0.11 | 0.00% | 0.27% | 1.28 |
| band-weak | 30% | 14.9 | 14,242 | 1.36 | 0.00% | 0.54% | 0.95 | 807 | 14,243 | 0.14 | 0.00% | 0.33% | 1.00 |
| band-weak-levels | 1% | 39.6 | 18,361 | 0.17 | 0.00% | 0.75% | 1.80 | 43.0 | 759,821 | 0.16 | 0.00% | 0.02% | 0.91 |
| band-weak-levels | 5% | 43.0 | 17,473 | 0.21 | 0.00% | 0.82% | 1.79 | 216 | 157,129 | 0.08 | 0.00% | 0.10% | 0.93 |
| band-weak-levels | 10% | 18.1 | 17,474 | 0.54 | 0.00% | 0.48% | 1.28 | 433 | 62,755 | 0.08 | 0.00% | 0.14% | 1.29 |
| band-weak-levels | 20% | 16.3 | 17,475 | 0.95 | 0.00% | 0.44% | 1.26 | 800 | 26,403 | 0.11 | 0.00% | 0.26% | 1.28 |
| band-weak-levels | 30% | 12.1 | 17,488 | 1.34 | 0.00% | 0.42% | 0.97 | 719 | 17,488 | 0.14 | 0.00% | 0.29% | 1.01 |
| band-weak-levels, weak work | 1% | 25.0 | 17,475 | 0.19 | 0.00% | 0.85% | 1.01 | 43.0 | 840,132 | 0.17 | 0.00% | 0.02% | 0.99 |
| band-weak-levels, weak work | 5% | 24.8 | 17,473 | 0.21 | 0.00% | 0.85% | 1.00 | 216 | 171,208 | 0.08 | 0.00% | 0.09% | 1.02 |
| band-weak-levels, weak work | 10% | 24.8 | 17,474 | 0.24 | 0.00% | 0.85% | 1.00 | 433 | 83,108 | 0.05 | 0.00% | 0.18% | 1.00 |
| band-weak-levels, weak work | 20% | 24.6 | 17,478 | 0.28 | 0.00% | 0.85% | 0.99 | 854 | 42,241 | 0.04 | 0.00% | 0.35% | 0.99 |
| band-weak-levels, weak work | 30% | 23.5 | 17,483 | 0.38 | 0.00% | 0.83% | 0.97 | 1,039 | 28,485 | 0.05 | 0.00% | 0.43% | 1.00 |

### Designs as Bitcoin Cash's hashrate grows (pool 10%, 50% industrial)

| Design | BCH hashrate | Bitaxe fair sat/mo | Bitaxe pay/mo | Bitaxe median sat | Bitaxe swing | Bitaxe empty months | Bitaxe fee loss | Bitaxe vs fair | ASIC pay/mo | ASIC median sat | ASIC swing | ASIC empty months | ASIC fee loss | ASIC vs fair |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| mini | x1 | 433,459 | 4.88 | 84,268 | 0.91 | 20.70% | 0.17% | 0.95 | 187 | 168,670 | 0.12 | 0.00% | 0.08% | 1.00 |
| mini | x3 | 144,486 | 1.55 | 84,282 | 1.61 | 59.90% | 0.18% | 0.90 | 105 | 84,312 | 0.19 | 0.00% | 0.13% | 1.01 |
| mini | x10 | 43,346 | 0.486 | 84,288 | 2.95 | 86.10% | 0.18% | 0.94 | 38.5 | 84,297 | 0.32 | 0.00% | 0.16% | 0.99 |
| mini | x270 | 1,605 | 0.026 | 84,290 | 12.49 | 99.30% | 0.18% | 1.34 | 1.54 | 84,290 | 1.56 | 60.70% | 0.18% | 0.97 |
| one-chain | x1 | 433,459 | 2.41 | 168,580 | 1.31 | 45.60% | 0.09% | 0.94 | 170 | 168,580 | 0.14 | 0.00% | 0.07% | 1.00 |
| one-chain | x3 | 144,486 | 0.759 | 168,550 | 2.29 | 77.90% | 0.09% | 0.89 | 66.4 | 168,550 | 0.24 | 0.00% | 0.08% | 1.01 |
| one-chain | x10 | 43,346 | 0.266 | 168,539 | 3.91 | 91.90% | 0.09% | 1.03 | 20.8 | 168,539 | 0.44 | 0.20% | 0.09% | 0.99 |
| one-chain | x270 | 1,605 | 0.034 | 168,534 | 12.90 | 99.30% | 0.09% | 3.53 | 0.710 | 168,534 | 2.29 | 79.70% | 0.09% | 0.89 |
| band | x1 | 433,459 | 3.21 | 134,656 | 1.09 | 36.80% | 0.11% | 1.00 | 201 | 134,656 | 0.13 | 0.00% | 0.08% | 1.01 |
| band | x3 | 144,486 | 0.839 | 154,410 | 2.20 | 76.10% | 0.10% | 0.90 | 71.6 | 154,410 | 0.24 | 0.00% | 0.09% | 1.00 |
| band | x10 | 43,346 | 0.266 | 168,539 | 3.91 | 91.90% | 0.09% | 1.03 | 20.8 | 168,539 | 0.44 | 0.20% | 0.09% | 0.99 |
| band | x270 | 1,605 | 0.034 | 168,534 | 12.90 | 99.30% | 0.09% | 3.53 | 0.710 | 168,534 | 2.29 | 79.70% | 0.09% | 0.89 |
| vardiff-cd | x1 | 433,459 | 4.84 | 84,681 | 0.90 | 20.60% | 0.17% | 0.95 | 18.8 | 1,862,564 | 0.46 | 0.10% | 0.01% | 0.99 |
| vardiff-cd | x3 | 144,486 | 1.53 | 85,100 | 1.60 | 61.40% | 0.17% | 0.90 | 18.8 | 623,483 | 0.46 | 0.30% | 0.02% | 0.99 |
| vardiff-cd | x10 | 43,346 | 0.490 | 86,602 | 2.91 | 86.20% | 0.17% | 0.98 | 18.8 | 187,549 | 0.46 | 0.40% | 0.08% | 1.00 |
| vardiff-cd | x270 | 1,605 | 0.023 | 134,648 | 14.84 | 99.40% | 0.11% | 1.93 | 0.903 | 134,648 | 2.12 | 74.80% | 0.11% | 0.91 |
| band-weak | x1 | 433,459 | 64.2 | 3,562 | 0.62 | 0.00% | 1.95% | 1.13 | 433 | 53,433 | 0.08 | 0.00% | 0.15% | 1.14 |
| band-weak | x3 | 144,486 | 22.3 | 3,579 | 1.11 | 0.00% | 2.16% | 1.06 | 427 | 14,318 | 0.14 | 0.00% | 0.46% | 1.15 |
| band-weak | x10 | 43,346 | 4.45 | 4,747 | 2.20 | 10.10% | 1.54% | 0.98 | 252 | 4,747 | 0.25 | 0.00% | 1.04% | 0.99 |
| band-weak | x270 | 1,605 | 0.144 | 4,747 | 11.37 | 92.20% | 0.91% | 1.45 | 13.8 | 4,747 | 1.31 | 0.10% | 1.59% | 0.96 |
| band-weak-levels | x1 | 433,459 | 15.6 | 17,472 | 0.67 | 0.00% | 0.46% | 1.16 | 431 | 53,433 | 0.09 | 0.00% | 0.15% | 1.14 |
| band-weak-levels | x3 | 144,486 | 5.00 | 17,475 | 1.20 | 1.60% | 0.45% | 1.13 | 339 | 17,897 | 0.14 | 0.00% | 0.36% | 1.14 |
| band-weak-levels | x10 | 43,346 | 1.27 | 17,474 | 2.35 | 35.40% | 0.46% | 0.95 | 107 | 17,474 | 0.25 | 0.00% | 0.44% | 1.00 |
| band-weak-levels | x270 | 1,605 | 0.043 | 17,472 | 10.37 | 96.60% | 0.32% | 1.24 | 4.08 | 17,472 | 1.32 | 4.20% | 0.44% | 1.03 |
| band-weak-levels, weak work | x1 | 433,459 | 24.6 | 17,473 | 0.25 | 0.00% | 0.85% | 0.99 | 431 | 81,921 | 0.05 | 0.00% | 0.18% | 1.00 |
| band-weak-levels, weak work | x3 | 144,486 | 8.05 | 17,472 | 0.45 | 0.00% | 0.85% | 0.97 | 416 | 28,634 | 0.06 | 0.00% | 0.51% | 1.00 |
| band-weak-levels, weak work | x10 | 43,346 | 2.44 | 17,472 | 0.87 | 20.00% | 0.85% | 0.98 | 197 | 17,473 | 0.10 | 0.00% | 0.81% | 1.00 |
| band-weak-levels, weak work | x270 | 1,605 | 0.102 | 17,472 | 4.51 | 93.90% | 0.85% | 1.11 | 7.49 | 17,472 | 0.48 | 0.70% | 0.85% | 0.98 |

### Pool load (50% industrial)

| Design | Pool | ASERT difficulty | Easiest share | C settles at | R | Weak per share | Weak expired | Window (Bitaxe's chain) | Shares/s | Uncles/s | Weak/s | KB/s |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| mini | 1% | 42,056,519 | 42,056,519 | - | - | - | - | 100.3 h | 0.204 | 4.3e-3 | 0 | 0.204 |
| mini | 5% | 210,733,007 | 210,733,007 | - | - | - | - | 20.0 h | 0.204 | 4.3e-3 | 0 | 0.204 |
| mini | 10% | 421,578,617 | 421,578,617 | - | - | - | - | 10.0 h | 0.204 | 4.3e-3 | 0 | 0.204 |
| mini | 20% | 843,269,838 | 843,269,838 | - | - | - | - | 5.0 h | 0.204 | 4.3e-3 | 0 | 0.204 |
| mini | 30% | 1,264,961,058 | 1,264,961,058 | - | - | - | - | 3.3 h | 0.204 | 4.3e-3 | 0 | 0.204 |
| one-chain | 1% | 84,338,244 | 84,338,244 | - | - | - | - | 50.0 h | 0.102 | 2.1e-3 | 0 | 0.102 |
| one-chain | 5% | 421,691,220 | 421,691,220 | - | - | - | - | 10.0 h | 0.102 | 2.1e-3 | 0 | 0.102 |
| one-chain | 10% | 843,382,441 | 843,382,441 | - | - | - | - | 5.0 h | 0.102 | 2.1e-3 | 0 | 0.102 |
| one-chain | 20% | 1,686,764,881 | 1,686,764,881 | - | - | - | - | 2.5 h | 0.102 | 2.1e-3 | 0 | 0.102 |
| one-chain | 30% | 2,530,147,322 | 2,530,147,322 | - | - | - | - | 1.7 h | 0.102 | 2.1e-3 | 0 | 0.102 |
| band | 1% | 42,881,878 | 42,881,878 | 1.01 | - | - | - | 50.0 h | 0.102 | 2.1e-3 | 0 | 0.102 |
| band | 5% | 252,929,932 | 252,929,932 | 1.00 | - | - | - | 10.0 h | 0.102 | 2.1e-3 | 0 | 0.102 |
| band | 10% | 673,227,233 | 673,227,233 | 1.00 | - | - | - | 5.0 h | 0.102 | 2.1e-3 | 0 | 0.102 |
| band | 20% | 1,667,836,535 | 1,667,836,535 | 1.00 | - | - | - | 2.5 h | 0.102 | 2.1e-3 | 0 | 0.102 |
| band | 30% | 2,530,147,322 | 2,530,147,322 | 1.00 | - | - | - | 1.7 h | 0.102 | 2.1e-3 | 0 | 0.102 |
| vardiff-cd | 1% | 1,297,682,804,146 | 42,087,010 | - | - | - | - | 50.0 h | 0.102 | 2.1e-3 | 0 | 0.102 |
| vardiff-cd | 5% | 17,367,303,371,429 | 210,998,631 | - | - | - | - | 10.0 h | 0.102 | 2.1e-3 | 0 | 0.102 |
| vardiff-cd | 10% | 34,734,606,742,859 | 422,632,736 | - | - | - | - | 5.0 h | 0.102 | 2.1e-3 | 0 | 0.102 |
| vardiff-cd | 20% | 69,469,213,485,718 | 847,477,341 | - | - | - | - | 2.5 h | 0.102 | 2.1e-3 | 0 | 0.102 |
| vardiff-cd | 30% | 104,203,820,228,577 | 1,274,436,885 | - | - | - | - | 1.7 h | 0.102 | 2.1e-3 | 0 | 0.102 |
| band-weak | 1% | 42,881,878 | 42,881,878 | 1.01 | 35.4 | 34.5 | 0.01% | 33.7 h | 0.102 | 2.1e-3 | 3.45 | 2.86 |
| band-weak | 5% | 252,929,932 | 252,929,932 | 1.00 | 41.4 | 34.5 | 0.01% | 6.7 h | 0.102 | 2.1e-3 | 3.45 | 2.86 |
| band-weak | 10% | 673,227,233 | 673,227,233 | 1.00 | 37.0 | 34.5 | 0.02% | 2.9 h | 0.102 | 2.1e-3 | 3.45 | 2.86 |
| band-weak | 20% | 1,667,836,535 | 1,667,836,535 | 1.00 | 45.4 | 34.5 | 0.02% | 1.4 h | 0.102 | 2.1e-3 | 3.45 | 2.86 |
| band-weak | 30% | 2,530,147,322 | 2,530,147,322 | 1.00 | 34.7 | 34.5 | 0.01% | 0.8 h | 0.102 | 2.1e-3 | 3.45 | 2.86 |
| band-weak-levels | 1% | 42,881,878 | 42,881,878 | 1.01 | 35.4 | 34.5 | 0.01% | 33.7 h | 0.102 | 2.1e-3 | 3.45 | 2.86 |
| band-weak-levels | 5% | 252,929,932 | 252,929,932 | 1.00 | 41.4 | 34.5 | 0.01% | 6.7 h | 0.102 | 2.1e-3 | 3.45 | 2.86 |
| band-weak-levels | 10% | 673,227,233 | 673,227,233 | 1.00 | 37.0 | 34.5 | 0.02% | 2.9 h | 0.102 | 2.1e-3 | 3.45 | 2.86 |
| band-weak-levels | 20% | 1,667,836,535 | 1,667,836,535 | 1.00 | 45.4 | 34.5 | 0.02% | 1.4 h | 0.102 | 2.1e-3 | 3.45 | 2.86 |
| band-weak-levels | 30% | 2,530,147,322 | 2,530,147,322 | 1.00 | 34.7 | 34.5 | 0.01% | 0.8 h | 0.102 | 2.1e-3 | 3.45 | 2.86 |
| band-weak-levels, weak work | 1% | 42,881,878 | 42,881,878 | 1.01 | 35.4 | 34.5 | 0.01% | 50.0 h | 0.102 | 2.1e-3 | 3.45 | 2.86 |
| band-weak-levels, weak work | 5% | 252,929,932 | 252,929,932 | 1.00 | 41.4 | 34.5 | 0.01% | 10.0 h | 0.102 | 2.1e-3 | 3.45 | 2.86 |
| band-weak-levels, weak work | 10% | 673,227,233 | 673,227,233 | 1.00 | 37.0 | 34.5 | 0.02% | 5.0 h | 0.102 | 2.1e-3 | 3.45 | 2.86 |
| band-weak-levels, weak work | 20% | 1,667,836,535 | 1,667,836,535 | 1.00 | 45.4 | 34.5 | 0.02% | 2.5 h | 0.102 | 2.1e-3 | 3.45 | 2.86 |
| band-weak-levels, weak work | 30% | 2,530,147,322 | 2,530,147,322 | 1.00 | 34.7 | 34.5 | 0.01% | 1.7 h | 0.102 | 2.1e-3 | 3.45 | 2.86 |

### Weak share credit: as the page writes it and credited once (band-weak-levels)

| Mix | Pool | Credit | Window | Bitaxe expected vs fair | Bitaxe simulated vs fair | Bitaxe swing | ASIC share multiple | ASIC expected vs fair | ASIC simulated vs fair |
|---|---|---|---|---|---|---|---|---|---|
| home | 1% | page | 25.4 h | 1.00 | 0.99 | 0.30 | 2.39 | 0.51 | 0.51 |
| home | 1% | weak work | 50.0 h | 1.00 | 1.01 | 0.21 | 2.39 | 1.01 | 1.00 |
| home | 10% | page | 2.5 h | 1.00 | 0.99 | 0.77 | 1.00 | 1.00 | 1.00 |
| home | 10% | weak work | 5.0 h | 1.00 | 0.99 | 0.27 | 1.00 | 1.00 | 1.00 |
| home | 30% | page | 0.8 h | 1.00 | 0.98 | 1.32 | 1.00 | 1.00 | 1.01 |
| home | 30% | weak work | 1.7 h | 1.00 | 0.97 | 0.38 | 1.00 | 1.00 | 1.00 |
| 50% industrial | 1% | page | 33.7 h | 1.33 | 1.32 | 0.23 | 4.69 | 0.68 | 0.67 |
| 50% industrial | 1% | weak work | 50.0 h | 1.00 | 1.00 | 0.20 | 4.69 | 1.00 | 1.00 |
| 50% industrial | 10% | page | 2.9 h | 1.14 | 1.16 | 0.67 | 1.00 | 1.14 | 1.14 |
| 50% industrial | 10% | weak work | 5.0 h | 1.00 | 0.99 | 0.25 | 1.00 | 1.00 | 1.00 |
| 50% industrial | 30% | page | 0.8 h | 1.00 | 0.97 | 1.33 | 1.00 | 1.00 | 1.01 |
| 50% industrial | 30% | weak work | 1.7 h | 1.00 | 0.97 | 0.39 | 1.00 | 1.00 | 1.00 |
| 90% industrial | 1% | page | 45.6 h | 1.81 | 1.80 | 0.17 | 20.9 | 0.91 | 0.91 |
| 90% industrial | 1% | weak work | 50.0 h | 1.01 | 1.01 | 0.19 | 20.9 | 1.00 | 0.99 |
| 90% industrial | 10% | page | 3.3 h | 1.28 | 1.28 | 0.54 | 1.00 | 1.28 | 1.29 |
| 90% industrial | 10% | weak work | 5.0 h | 1.00 | 1.00 | 0.24 | 1.00 | 1.00 | 1.00 |
| 90% industrial | 30% | page | 0.8 h | 1.00 | 0.97 | 1.34 | 1.00 | 1.00 | 1.01 |
| 90% industrial | 30% | weak work | 1.7 h | 1.00 | 0.97 | 0.38 | 1.00 | 1.00 | 1.00 |

### Band cap C (design band)

| Mix | Pool | C | C settles at | Minimum / uniform | ASIC share multiple | Bitaxe shares/mo | Bitaxe pay/mo | Bitaxe swing | Bitaxe empty months | ASIC pay/mo | ASIC swing |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 50% industrial | 1% | 1.00 | 1.00 | 1.00 | 1.00 | 8.53 | 19.5 | 0.41 | 0.00% | 43.4 | 0.16 |
| 50% industrial | 1% | 8.00 | 1.01 | 0.56 | 4.25 | 15.3 | 28.3 | 0.33 | 0.00% | 43.1 | 0.17 |
| 50% industrial | 1% | 30.0 | 1.01 | 0.52 | 4.62 | 16.6 | 29.5 | 0.32 | 0.00% | 43.1 | 0.17 |
| 50% industrial | 1% | 64.0 | 1.01 | 0.51 | 4.69 | 16.8 | 29.8 | 0.32 | 0.00% | 43.1 | 0.16 |
| 50% industrial | 1% | 256 | 1.01 | 0.51 | 4.72 | 16.9 | 29.9 | 0.31 | 0.00% | 43.1 | 0.17 |
| 50% industrial | 10% | 1.00 | 1.00 | 1.00 | 1.00 | 0.795 | 2.41 | 1.31 | 45.60% | 170 | 0.14 |
| 50% industrial | 10% | 8.00 | 1.00 | 0.80 | 1.00 | 1.04 | 3.21 | 1.09 | 36.80% | 201 | 0.13 |
| 50% industrial | 10% | 30.0 | 1.00 | 0.80 | 1.00 | 1.04 | 3.21 | 1.09 | 36.80% | 201 | 0.13 |
| 50% industrial | 10% | 64.0 | 1.00 | 0.80 | 1.00 | 1.04 | 3.21 | 1.09 | 36.80% | 201 | 0.13 |
| 50% industrial | 10% | 256 | 1.00 | 0.80 | 1.00 | 1.04 | 3.21 | 1.09 | 36.80% | 201 | 0.13 |
| 50% industrial | 30% | 1.00 | 1.00 | 1.00 | 1.00 | 0.256 | 0.755 | 2.39 | 78.30% | 70.0 | 0.23 |
| 50% industrial | 30% | 8.00 | 1.00 | 1.00 | 1.00 | 0.256 | 0.755 | 2.39 | 78.30% | 70.0 | 0.23 |
| 50% industrial | 30% | 30.0 | 1.00 | 1.00 | 1.00 | 0.256 | 0.755 | 2.39 | 78.30% | 70.0 | 0.23 |
| 50% industrial | 30% | 64.0 | 1.00 | 1.00 | 1.00 | 0.256 | 0.755 | 2.39 | 78.30% | 70.0 | 0.23 |
| 50% industrial | 30% | 256 | 1.00 | 1.00 | 1.00 | 0.256 | 0.755 | 2.39 | 78.30% | 70.0 | 0.23 |
| 90% industrial | 1% | 1.00 | 1.00 | 1.00 | 1.00 | 8.53 | 19.5 | 0.41 | 0.00% | 43.4 | 0.16 |
| 90% industrial | 1% | 8.00 | 8.00 | 0.21 | 8.00 | 40.6 | 40.7 | 0.23 | 0.00% | 43.2 | 0.16 |
| 90% industrial | 1% | 30.0 | 30.0 | 0.13 | 18.4 | 66.4 | 42.3 | 0.20 | 0.00% | 42.7 | 0.17 |
| 90% industrial | 1% | 64.0 | 64.0 | 0.11 | 20.9 | 75.7 | 42.9 | 0.19 | 0.00% | 43.1 | 0.17 |
| 90% industrial | 1% | 256 | 256 | 0.10 | 23.0 | 83.1 | 42.8 | 0.20 | 0.00% | 42.9 | 0.17 |
| 90% industrial | 10% | 1.00 | 1.00 | 1.00 | 1.00 | 0.795 | 2.41 | 1.31 | 45.60% | 170 | 0.14 |
| 90% industrial | 10% | 8.00 | 1.00 | 0.62 | 1.00 | 1.32 | 4.01 | 0.98 | 27.00% | 239 | 0.12 |
| 90% industrial | 10% | 30.0 | 1.00 | 0.62 | 1.00 | 1.32 | 4.01 | 0.98 | 27.00% | 239 | 0.12 |
| 90% industrial | 10% | 64.0 | 1.00 | 0.62 | 1.00 | 1.32 | 4.01 | 0.98 | 27.00% | 239 | 0.12 |
| 90% industrial | 10% | 256 | 1.00 | 0.62 | 1.00 | 1.32 | 4.01 | 0.98 | 27.00% | 239 | 0.12 |
| 90% industrial | 30% | 1.00 | 1.00 | 1.00 | 1.00 | 0.256 | 0.755 | 2.39 | 78.30% | 70.0 | 0.23 |
| 90% industrial | 30% | 8.00 | 1.00 | 1.00 | 1.00 | 0.256 | 0.755 | 2.39 | 78.30% | 70.0 | 0.23 |
| 90% industrial | 30% | 30.0 | 1.00 | 1.00 | 1.00 | 0.256 | 0.755 | 2.39 | 78.30% | 70.0 | 0.23 |
| 90% industrial | 30% | 64.0 | 1.00 | 1.00 | 1.00 | 0.256 | 0.755 | 2.39 | 78.30% | 70.0 | 0.23 |
| 90% industrial | 30% | 256 | 1.00 | 1.00 | 1.00 | 0.256 | 0.755 | 2.39 | 78.30% | 70.0 | 0.23 |

### WEAK_CAP and WEAK_FILL (band-weak-levels, 50% industrial)

| Credit | Pool | WEAK_CAP | WEAK_FILL | R | Weak found per share | Carried per share | Shares at the cap | Weak expired | KB/s | Bitaxe weak/mo | Bitaxe pay/mo | Bitaxe swing | Bitaxe empty months |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| page | 1% | 16 | 8 | 9.55 | 8.56 | 8.56 | 26.98% | 0.02% | 0.787 | 144 | 28.0 | 0.23 | 0.00% |
| page | 1% | 32 | 16 | 18.2 | 17.2 | 17.2 | 26.03% | 0.02% | 1.48 | 291 | 28.3 | 0.23 | 0.00% |
| page | 1% | 64 | 32 | 35.4 | 34.5 | 34.5 | 25.50% | 0.01% | 2.86 | 581 | 28.5 | 0.23 | 0.00% |
| page | 1% | 128 | 64 | 69.8 | 69.0 | 69.0 | 25.20% | 0.01% | 5.62 | 1,161 | 28.8 | 0.23 | 0.00% |
| page | 1% | 64 | 16 | 22.7 | 21.7 | 21.7 | 6.81% | 0.00% | 1.84 | 367 | 28.5 | 0.23 | 0.00% |
| page | 1% | 64 | 48 | 42.5 | 41.6 | 41.6 | 39.93% | 0.17% | 3.43 | 700 | 28.8 | 0.23 | 0.00% |
| page | 10% | 16 | 8 | 9.91 | 8.56 | 8.56 | 26.96% | 0.03% | 0.787 | 9.44 | 14.4 | 0.68 | 0.00% |
| page | 10% | 32 | 16 | 19.0 | 17.2 | 17.2 | 26.15% | 0.01% | 1.48 | 19.4 | 15.1 | 0.69 | 0.00% |
| page | 10% | 64 | 32 | 37.0 | 34.5 | 34.5 | 25.42% | 0.02% | 2.86 | 38.8 | 15.6 | 0.67 | 0.00% |
| page | 10% | 128 | 64 | 73.1 | 69.1 | 69.1 | 25.19% | 0.01% | 5.63 | 77.5 | 15.7 | 0.66 | 0.00% |
| page | 10% | 64 | 16 | 23.6 | 21.7 | 21.7 | 6.89% | 0.00% | 1.84 | 24.5 | 15.3 | 0.65 | 0.00% |
| page | 10% | 64 | 48 | 44.3 | 41.6 | 41.6 | 39.99% | 0.16% | 3.43 | 46.5 | 15.5 | 0.62 | 0.00% |
| page | 30% | 16 | 8 | 9.41 | 8.57 | 8.56 | 27.04% | 0.02% | 0.787 | 2.27 | 4.03 | 1.40 | 13.40% |
| page | 30% | 32 | 16 | 17.9 | 17.2 | 17.2 | 26.11% | 0.01% | 1.48 | 4.62 | 7.46 | 1.27 | 2.30% |
| page | 30% | 64 | 32 | 34.7 | 34.5 | 34.5 | 25.50% | 0.01% | 2.86 | 9.41 | 12.1 | 1.33 | 0.00% |
| page | 30% | 128 | 64 | 68.6 | 69.1 | 69.1 | 25.21% | 0.01% | 5.63 | 19.3 | 12.3 | 1.21 | 0.00% |
| page | 30% | 64 | 16 | 22.3 | 21.7 | 21.7 | 6.92% | 0.00% | 1.84 | 5.92 | 9.55 | 1.30 | 0.40% |
| page | 30% | 64 | 48 | 41.7 | 41.7 | 41.6 | 40.07% | 0.19% | 3.43 | 11.5 | 12.2 | 1.19 | 0.20% |
| weak work | 1% | 16 | 8 | 9.55 | 8.56 | 8.56 | 26.98% | 0.02% | 0.787 | 144 | 24.7 | 0.21 | 0.00% |
| weak work | 1% | 32 | 16 | 18.2 | 17.2 | 17.2 | 26.03% | 0.02% | 1.48 | 291 | 24.8 | 0.21 | 0.00% |
| weak work | 1% | 64 | 32 | 35.4 | 34.5 | 34.5 | 25.50% | 0.01% | 2.86 | 581 | 24.9 | 0.20 | 0.00% |
| weak work | 1% | 128 | 64 | 69.8 | 69.0 | 69.0 | 25.20% | 0.01% | 5.62 | 1,161 | 24.7 | 0.21 | 0.00% |
| weak work | 1% | 64 | 16 | 22.7 | 21.7 | 21.7 | 6.81% | 0.00% | 1.84 | 367 | 24.6 | 0.21 | 0.00% |
| weak work | 1% | 64 | 48 | 42.5 | 41.6 | 41.6 | 39.93% | 0.17% | 3.43 | 700 | 24.8 | 0.20 | 0.00% |
| weak work | 10% | 16 | 8 | 9.91 | 8.56 | 8.56 | 26.96% | 0.03% | 0.787 | 9.44 | 23.8 | 0.37 | 0.00% |
| weak work | 10% | 32 | 16 | 19.0 | 17.2 | 17.2 | 26.15% | 0.01% | 1.48 | 19.4 | 25.0 | 0.29 | 0.00% |
| weak work | 10% | 64 | 32 | 37.0 | 34.5 | 34.5 | 25.42% | 0.02% | 2.86 | 38.8 | 24.6 | 0.25 | 0.00% |
| weak work | 10% | 128 | 64 | 73.1 | 69.1 | 69.1 | 25.19% | 0.01% | 5.63 | 77.5 | 24.8 | 0.24 | 0.00% |
| weak work | 10% | 64 | 16 | 23.6 | 21.7 | 21.7 | 6.89% | 0.00% | 1.84 | 24.5 | 24.4 | 0.27 | 0.00% |
| weak work | 10% | 64 | 48 | 44.3 | 41.6 | 41.6 | 39.99% | 0.16% | 3.43 | 46.5 | 24.6 | 0.25 | 0.00% |
| weak work | 30% | 16 | 8 | 9.41 | 8.57 | 8.56 | 27.04% | 0.02% | 0.787 | 2.27 | 7.63 | 0.74 | 8.50% |
| weak work | 30% | 32 | 16 | 17.9 | 17.2 | 17.2 | 26.11% | 0.01% | 1.48 | 4.62 | 14.3 | 0.51 | 1.40% |
| weak work | 30% | 64 | 32 | 34.7 | 34.5 | 34.5 | 25.50% | 0.01% | 2.86 | 9.41 | 23.6 | 0.39 | 0.00% |
| weak work | 30% | 128 | 64 | 68.6 | 69.1 | 69.1 | 25.21% | 0.01% | 5.63 | 19.3 | 24.8 | 0.31 | 0.00% |
| weak work | 30% | 64 | 16 | 22.3 | 21.7 | 21.7 | 6.92% | 0.00% | 1.84 | 5.92 | 18.5 | 0.46 | 0.20% |
| weak work | 30% | 64 | 48 | 41.7 | 41.7 | 41.6 | 40.07% | 0.19% | 3.43 | 11.5 | 24.1 | 0.36 | 0.00% |

### INCLUDER_CUT (band-weak-levels)

| Pool | Credit | INCLUDER_CUT | Bitaxe vs fair | Bitaxe bonus share | Carrying ASIC vs fair | Carrying ASIC bonus share | Lone non-carrying ASIC vs fair | Honest weak expired, 50% non-carrying | Bitaxe vs fair, 50% non-carrying | Non-carrying ASIC vs fair, 50% non-carrying |
|---|---|---|---|---|---|---|---|---|---|---|
| 1% | page | 0.00% | 1.33 | 0.21% | 0.67 | 0.09% | 0.67 | 4.82% | 1.31 | 0.68 |
| 1% | page | 1.00% | 1.33 | 0.69% | 0.67 | 0.29% | 0.67 | 4.82% | 1.31 | 0.68 |
| 1% | page | 2.00% | 1.33 | 1.18% | 0.68 | 0.49% | 0.67 | 4.82% | 1.31 | 0.68 |
| 1% | page | 5.00% | 1.33 | 2.63% | 0.68 | 1.09% | 0.67 | 4.82% | 1.32 | 0.68 |
| 1% | weak work | 0.00% | 1.00 | 0.21% | 1.00 | 0.05% | 1.00 | 4.82% | 0.97 | 1.01 |
| 1% | weak work | 1.00% | 1.00 | 1.17% | 1.00 | 0.25% | 1.00 | 4.82% | 0.97 | 1.01 |
| 1% | weak work | 2.00% | 1.00 | 2.12% | 1.00 | 0.45% | 1.00 | 4.82% | 0.97 | 1.01 |
| 1% | weak work | 5.00% | 1.00 | 4.98% | 1.01 | 1.05% | 1.00 | 4.82% | 0.98 | 1.02 |
| 10% | page | 0.00% | 1.14 | 0.13% | 1.14 | 0.13% | 1.14 | 4.88% | 1.13 | 1.15 |
| 10% | page | 1.00% | 1.14 | 0.60% | 1.14 | 0.60% | 1.14 | 4.88% | 1.13 | 1.15 |
| 10% | page | 2.00% | 1.14 | 1.06% | 1.14 | 1.06% | 1.13 | 4.88% | 1.13 | 1.15 |
| 10% | page | 5.00% | 1.14 | 2.46% | 1.14 | 2.46% | 1.11 | 4.88% | 1.13 | 1.14 |
| 10% | weak work | 0.00% | 1.00 | 0.07% | 1.00 | 0.07% | 1.00 | 4.88% | 0.97 | 1.02 |
| 10% | weak work | 1.00% | 1.00 | 0.99% | 1.00 | 0.99% | 0.99 | 4.88% | 0.97 | 1.01 |
| 10% | weak work | 2.00% | 1.00 | 1.90% | 1.00 | 1.90% | 0.98 | 4.88% | 0.98 | 1.01 |
| 10% | weak work | 5.00% | 1.00 | 4.66% | 1.00 | 4.66% | 0.95 | 4.88% | 0.98 | 1.00 |

### Default payout level (band-weak-levels, 50% industrial)

| Credit | Pool | Level | Bitaxe pay/mo | Bitaxe median sat | Bitaxe swing | Bitaxe empty months | Bitaxe fee loss | ASIC pay/mo | ASIC median sat | ASIC fee loss |
|---|---|---|---|---|---|---|---|---|---|---|
| page | 1% | none | 43.4 | 13,737 | 0.23 | 0.00% | 1.11% | 43.4 | 564,712 | 0.03% |
| page | 1% | 3 (4,368 sat) | 43.1 | 13,735 | 0.21 | 0.00% | 1.11% | 43.2 | 564,715 | 0.03% |
| page | 1% | 4 (8,736 sat) | 37.9 | 14,446 | 0.22 | 0.00% | 0.98% | 43.2 | 564,715 | 0.03% |
| page | 1% | 5 (17,472 sat) | 28.5 | 17,473 | 0.23 | 0.00% | 0.74% | 43.2 | 564,715 | 0.03% |
| page | 1% | 6 (34,944 sat) | 16.3 | 34,947 | 0.28 | 0.00% | 0.42% | 43.2 | 564,715 | 0.03% |
| page | 1% | 7 (69,888 sat) | 8.11 | 69,891 | 0.37 | 0.10% | 0.21% | 43.2 | 564,715 | 0.03% |
| page | 10% | none | 64.2 | 3,562 | 0.62 | 0.00% | 1.95% | 433 | 53,433 | 0.15% |
| page | 10% | 3 (4,368 sat) | 53.4 | 4,368 | 0.66 | 0.00% | 1.58% | 431 | 53,433 | 0.16% |
| page | 10% | 4 (8,736 sat) | 29.2 | 8,736 | 0.67 | 0.00% | 0.87% | 431 | 53,433 | 0.16% |
| page | 10% | 5 (17,472 sat) | 15.6 | 17,472 | 0.67 | 0.00% | 0.46% | 431 | 53,433 | 0.15% |
| page | 10% | 6 (34,944 sat) | 8.77 | 34,951 | 0.68 | 0.00% | 0.26% | 421 | 53,433 | 0.15% |
| page | 10% | 7 (69,888 sat) | 5.37 | 69,939 | 0.70 | 1.40% | 0.16% | 328 | 69,942 | 0.12% |
| page | 30% | none | 14.9 | 14,242 | 1.36 | 0.00% | 0.54% | 807 | 14,243 | 0.33% |
| page | 30% | 3 (4,368 sat) | 14.7 | 14,242 | 1.33 | 0.00% | 0.52% | 805 | 14,243 | 0.33% |
| page | 30% | 4 (8,736 sat) | 14.7 | 14,242 | 1.33 | 0.00% | 0.52% | 805 | 14,243 | 0.33% |
| page | 30% | 5 (17,472 sat) | 12.1 | 17,475 | 1.33 | 0.00% | 0.42% | 720 | 17,475 | 0.29% |
| page | 30% | 6 (34,944 sat) | 6.30 | 34,950 | 1.33 | 1.00% | 0.22% | 491 | 34,950 | 0.20% |
| page | 30% | 7 (69,888 sat) | 3.28 | 69,921 | 1.37 | 7.50% | 0.12% | 278 | 69,921 | 0.11% |
| weak work | 1% | none | 43.4 | 9,956 | 0.16 | 0.00% | 1.47% | 43.4 | 842,519 | 0.02% |
| weak work | 1% | 3 (4,368 sat) | 43.2 | 9,956 | 0.16 | 0.00% | 1.47% | 43.2 | 842,516 | 0.02% |
| weak work | 1% | 4 (8,736 sat) | 42.3 | 10,085 | 0.16 | 0.00% | 1.44% | 43.2 | 842,516 | 0.02% |
| weak work | 1% | 5 (17,472 sat) | 24.9 | 17,473 | 0.20 | 0.00% | 0.85% | 43.2 | 842,516 | 0.02% |
| weak work | 1% | 6 (34,944 sat) | 12.4 | 34,947 | 0.29 | 0.00% | 0.42% | 43.2 | 842,516 | 0.02% |
| weak work | 1% | 7 (69,888 sat) | 6.20 | 69,911 | 0.40 | 0.20% | 0.21% | 43.2 | 842,516 | 0.02% |
| weak work | 10% | none | 105 | 3,561 | 0.19 | 0.00% | 3.57% | 433 | 81,921 | 0.18% |
| weak work | 10% | 3 (4,368 sat) | 87.8 | 4,368 | 0.20 | 0.00% | 3.01% | 431 | 81,921 | 0.18% |
| weak work | 10% | 4 (8,736 sat) | 49.2 | 8,736 | 0.22 | 0.00% | 1.68% | 431 | 81,921 | 0.18% |
| weak work | 10% | 5 (17,472 sat) | 24.6 | 17,473 | 0.25 | 0.00% | 0.85% | 431 | 81,921 | 0.18% |
| weak work | 10% | 6 (34,944 sat) | 12.4 | 34,951 | 0.32 | 0.00% | 0.42% | 431 | 81,921 | 0.18% |
| weak work | 10% | 7 (69,888 sat) | 6.27 | 69,918 | 0.43 | 0.00% | 0.21% | 418 | 83,708 | 0.17% |
| weak work | 30% | none | 29.3 | 14,241 | 0.36 | 0.00% | 1.01% | 1,106 | 28,485 | 0.45% |
| weak work | 30% | 3 (4,368 sat) | 28.7 | 14,241 | 0.38 | 0.00% | 1.01% | 1,104 | 28,485 | 0.45% |
| weak work | 30% | 4 (8,736 sat) | 28.7 | 14,241 | 0.38 | 0.00% | 1.01% | 1,104 | 28,485 | 0.45% |
| weak work | 30% | 5 (17,472 sat) | 23.6 | 17,472 | 0.39 | 0.00% | 0.83% | 1,039 | 28,485 | 0.43% |
| weak work | 30% | 6 (34,944 sat) | 12.1 | 34,949 | 0.43 | 0.00% | 0.42% | 830 | 34,953 | 0.34% |
| weak work | 30% | 7 (69,888 sat) | 6.00 | 69,911 | 0.53 | 1.50% | 0.21% | 509 | 69,918 | 0.21% |

### Share period (pool 10%, 50% industrial)

| Design | Period | Stale | Window | Shares/s | Uncles/s | Weak/s | KB/s | Bitaxe pay/mo | Bitaxe median sat | Bitaxe swing | Bitaxe empty months | Bitaxe fee loss | Bitaxe vs fair |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| one-chain | 10 s | 2.10% | 5.0 h | 0.102 | 2.1e-3 | 0 | 0.102 | 2.41 | 168,580 | 1.31 | 45.60% | 0.09% | 0.94 |
| one-chain | 5 s | 3.80% | 5.0 h | 0.208 | 7.9e-3 | 0 | 0.208 | 5.06 | 82,976 | 0.88 | 20.90% | 0.18% | 0.97 |
| one-chain | 2 s | 8.70% | 5.0 h | 0.548 | 0.048 | 0 | 0.548 | 13.5 | 31,679 | 0.53 | 1.40% | 0.46% | 1.00 |
| one-chain | 1 s | 15.90% | 5.0 h | 1.19 | 0.189 | 0 | 1.19 | 28.8 | 14,727 | 0.36 | 0.00% | 0.99% | 0.99 |
| band-weak-levels | 10 s | 2.10% | 2.9 h | 0.102 | 2.1e-3 | 3.45 | 2.86 | 15.6 | 17,472 | 0.67 | 0.00% | 0.46% | 1.16 |
| band-weak-levels | 5 s | 3.80% | 3.4 h | 0.208 | 7.9e-3 | 6.89 | 5.72 | 20.2 | 17,473 | 0.43 | 0.00% | 0.54% | 1.28 |
| band-weak-levels | 2 s | 8.70% | 3.4 h | 0.548 | 0.048 | 17.3 | 14.3 | 31.0 | 17,474 | 0.25 | 0.00% | 0.80% | 1.32 |
| band-weak-levels | 1 s | 15.90% | 3.4 h | 1.19 | 0.189 | 34.5 | 28.8 | 32.9 | 17,472 | 0.21 | 0.00% | 0.85% | 1.33 |
| band-weak-levels, weak work | 10 s | 2.10% | 5.0 h | 0.102 | 2.1e-3 | 3.45 | 2.86 | 24.6 | 17,473 | 0.25 | 0.00% | 0.85% | 0.99 |
| band-weak-levels, weak work | 5 s | 3.80% | 5.0 h | 0.208 | 7.9e-3 | 6.89 | 5.72 | 24.9 | 17,472 | 0.22 | 0.00% | 0.85% | 1.00 |
| band-weak-levels, weak work | 2 s | 8.70% | 5.0 h | 0.548 | 0.048 | 17.3 | 14.3 | 24.8 | 17,473 | 0.21 | 0.00% | 0.85% | 1.00 |
| band-weak-levels, weak work | 1 s | 15.90% | 5.0 h | 1.19 | 0.189 | 34.5 | 28.8 | 24.7 | 17,472 | 0.21 | 0.00% | 0.85% | 1.00 |

### Attack (a): the industrial node mines at the minimum (90% industrial)

| Design | Credit | Pool | Industrial node | Minimum / uniform | R | Bitaxe pay/mo | Bitaxe median sat | Bitaxe swing | Bitaxe empty months | Bitaxe fee loss | Bitaxe vs fair | ASIC pay/mo | ASIC median sat | ASIC swing | ASIC empty months | ASIC fee loss | ASIC vs fair |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| one-chain | page | 1% | node default | 1.00 | - | 19.5 | 16,864 | 0.41 | 0.00% | 0.67% | 1.00 | 43.4 | 826,323 | 0.16 | 0.00% | 0.02% | 1.00 |
| one-chain | page | 10% | node default | 1.00 | - | 2.41 | 168,580 | 1.31 | 45.60% | 0.09% | 0.94 | 170 | 168,580 | 0.14 | 0.00% | 0.07% | 1.00 |
| one-chain | page | 30% | node default | 1.00 | - | 0.755 | 505,099 | 2.39 | 78.30% | 0.03% | 0.88 | 70.0 | 505,099 | 0.23 | 0.00% | 0.03% | 1.00 |
| band | page | 1% | node default | 0.11 | - | 42.9 | 9,765 | 0.19 | 0.00% | 1.44% | 1.02 | 43.1 | 839,535 | 0.17 | 0.00% | 0.02% | 1.00 |
| band | page | 1% | at the minimum | 1.00 | - | 19.3 | 16,839 | 0.42 | 0.00% | 0.67% | 0.99 | 43.1 | 839,515 | 0.16 | 0.00% | 0.02% | 1.00 |
| band | page | 10% | node default | 0.62 | - | 4.01 | 104,168 | 0.98 | 27.00% | 0.14% | 0.96 | 239 | 104,203 | 0.12 | 0.00% | 0.10% | 1.00 |
| band | page | 10% | at the minimum | 1.00 | - | 2.41 | 168,580 | 1.31 | 45.60% | 0.09% | 0.94 | 170 | 168,580 | 0.14 | 0.00% | 0.07% | 1.00 |
| band | page | 30% | node default | 1.00 | - | 0.755 | 505,099 | 2.39 | 78.30% | 0.03% | 0.88 | 70.0 | 505,099 | 0.23 | 0.00% | 0.03% | 1.00 |
| band | page | 30% | at the minimum | 1.00 | - | 0.755 | 505,099 | 2.39 | 78.30% | 0.03% | 0.88 | 70.0 | 505,099 | 0.23 | 0.00% | 0.03% | 1.00 |
| band-weak-levels | page | 1% | node default | 0.11 | 39.5 | 39.6 | 18,361 | 0.17 | 0.00% | 0.75% | 1.80 | 43.0 | 759,821 | 0.16 | 0.00% | 0.02% | 0.91 |
| band-weak-levels | page | 1% | at the minimum | 1.00 | 34.8 | 20.0 | 20,007 | 0.29 | 0.00% | 0.69% | 0.99 | 43.1 | 405,076 | 0.17 | 0.00% | 0.03% | 0.51 |
| band-weak-levels | page | 10% | node default | 0.62 | 38.9 | 18.1 | 17,474 | 0.54 | 0.00% | 0.48% | 1.28 | 433 | 62,755 | 0.08 | 0.00% | 0.14% | 1.29 |
| band-weak-levels | page | 10% | at the minimum | 1.00 | 34.7 | 13.1 | 17,487 | 0.76 | 0.10% | 0.45% | 1.00 | 429 | 42,733 | 0.09 | 0.00% | 0.18% | 1.00 |
| band-weak-levels | page | 30% | node default | 1.00 | 34.7 | 12.1 | 17,488 | 1.34 | 0.00% | 0.42% | 0.97 | 719 | 17,488 | 0.14 | 0.00% | 0.29% | 1.01 |
| band-weak-levels | page | 30% | at the minimum | 1.00 | 34.7 | 12.1 | 17,488 | 1.34 | 0.00% | 0.42% | 0.97 | 719 | 17,488 | 0.14 | 0.00% | 0.29% | 1.01 |
| band-weak-levels | weak work | 1% | node default | 0.11 | 39.5 | 25.0 | 17,475 | 0.19 | 0.00% | 0.85% | 1.01 | 43.0 | 840,132 | 0.17 | 0.00% | 0.02% | 0.99 |
| band-weak-levels | weak work | 1% | at the minimum | 1.00 | 34.8 | 24.8 | 17,476 | 0.21 | 0.00% | 0.85% | 1.00 | 43.1 | 845,480 | 0.16 | 0.00% | 0.02% | 1.00 |
| band-weak-levels | weak work | 10% | node default | 0.62 | 38.9 | 24.8 | 17,474 | 0.24 | 0.00% | 0.85% | 1.00 | 433 | 83,108 | 0.05 | 0.00% | 0.18% | 1.00 |
| band-weak-levels | weak work | 10% | at the minimum | 1.00 | 34.7 | 24.6 | 17,481 | 0.27 | 0.00% | 0.85% | 0.99 | 432 | 82,857 | 0.05 | 0.00% | 0.18% | 1.00 |
| band-weak-levels | weak work | 30% | node default | 1.00 | 34.7 | 23.5 | 17,483 | 0.38 | 0.00% | 0.83% | 0.97 | 1,039 | 28,485 | 0.05 | 0.00% | 0.43% | 1.00 |
| band-weak-levels | weak work | 30% | at the minimum | 1.00 | 34.7 | 23.5 | 17,483 | 0.38 | 0.00% | 0.83% | 0.97 | 1,039 | 28,485 | 0.05 | 0.00% | 0.43% | 1.00 |

### Attack (b): nodes that never carry others' weak shares (band-weak-levels)

| Mix | Pool | Credit | Non-carrying | R | Honest weak expired | Bitaxe weak/mo | Bitaxe pay/mo | Bitaxe swing | Bitaxe empty months | Bitaxe vs fair | Carrying ASIC vs fair | Non-carrying ASIC vs fair |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 50% industrial | 1% | page | 0.00% | 35.4 | 0.02% | 582 | 28.6 | 0.23 | 0.00% | 1.33 | 0.68 | 0.67 |
| 50% industrial | 1% | page | 50.00% | 36.5 | 4.82% | 572 | 28.3 | 0.23 | 0.00% | 1.31 | 0.68 | 0.68 |
| 50% industrial | 1% | page | 90.00% | 37.5 | 56.27% | 270 | 21.5 | 0.28 | 0.00% | 0.99 | 0.69 | 0.69 |
| 50% industrial | 1% | weak work | 0.00% | 35.4 | 0.02% | 582 | 24.9 | 0.20 | 0.00% | 1.00 | 1.00 | 1.00 |
| 50% industrial | 1% | weak work | 50.00% | 36.5 | 4.82% | 572 | 24.2 | 0.20 | 0.00% | 0.97 | 1.02 | 1.01 |
| 50% industrial | 1% | weak work | 90.00% | 37.5 | 56.27% | 270 | 11.9 | 0.29 | 0.00% | 0.48 | 1.03 | 1.03 |
| 50% industrial | 10% | page | 0.00% | 36.9 | 0.02% | 38.7 | 15.3 | 0.67 | 0.00% | 1.14 | 1.14 | 1.13 |
| 50% industrial | 10% | page | 50.00% | 37.9 | 4.88% | 37.9 | 14.9 | 0.66 | 0.00% | 1.13 | 1.13 | 1.15 |
| 50% industrial | 10% | page | 90.00% | 39.1 | 56.39% | 17.9 | 7.91 | 0.87 | 0.10% | 0.85 | 0.85 | 1.17 |
| 50% industrial | 10% | weak work | 0.00% | 36.9 | 0.02% | 38.7 | 25.1 | 0.26 | 0.00% | 1.00 | 1.00 | 0.98 |
| 50% industrial | 10% | weak work | 50.00% | 37.9 | 4.88% | 37.9 | 24.2 | 0.25 | 0.00% | 0.98 | 0.98 | 1.01 |
| 50% industrial | 10% | weak work | 90.00% | 39.1 | 56.39% | 17.9 | 12.1 | 0.35 | 0.00% | 0.48 | 0.48 | 1.04 |
| 90% industrial | 1% | page | 0.00% | 39.5 | 0.01% | 2,904 | 39.7 | 0.17 | 0.00% | 1.81 | 0.91 | 0.91 |
| 90% industrial | 1% | page | 50.00% | 40.7 | 4.98% | 2,848 | 39.2 | 0.17 | 0.00% | 1.78 | 0.91 | 0.91 |
| 90% industrial | 1% | page | 90.00% | 41.9 | 56.21% | 1,353 | 31.5 | 0.20 | 0.00% | 1.33 | 0.92 | 0.92 |
| 90% industrial | 1% | weak work | 0.00% | 39.5 | 0.01% | 2,904 | 25.0 | 0.20 | 0.00% | 1.01 | 1.00 | 1.00 |
| 90% industrial | 1% | weak work | 50.00% | 40.7 | 4.98% | 2,848 | 23.8 | 0.21 | 0.00% | 0.97 | 1.00 | 1.00 |
| 90% industrial | 1% | weak work | 90.00% | 41.9 | 56.21% | 1,353 | 11.8 | 0.30 | 0.00% | 0.48 | 1.01 | 1.00 |
| 90% industrial | 10% | page | 0.00% | 39.0 | 0.01% | 53.0 | 17.9 | 0.54 | 0.00% | 1.28 | 1.28 | 1.27 |
| 90% industrial | 10% | page | 50.00% | 39.9 | 4.95% | 51.6 | 17.4 | 0.56 | 0.00% | 1.27 | 1.27 | 1.29 |
| 90% industrial | 10% | page | 90.00% | 41.1 | 56.13% | 24.6 | 9.64 | 0.74 | 0.30% | 0.96 | 0.96 | 1.31 |
| 90% industrial | 10% | weak work | 0.00% | 39.0 | 0.01% | 53.0 | 24.9 | 0.24 | 0.00% | 1.00 | 1.00 | 0.98 |
| 90% industrial | 10% | weak work | 50.00% | 39.9 | 4.95% | 51.6 | 24.2 | 0.24 | 0.00% | 0.97 | 0.97 | 1.01 |
| 90% industrial | 10% | weak work | 90.00% | 41.1 | 56.13% | 24.6 | 12.1 | 0.34 | 0.00% | 0.48 | 0.48 | 1.03 |

### Attack (c): weak share spam (band-weak-levels)

| Mix | Pool before spam | Spam | Pool with spam | Credit | R | Bitaxe weak/mo | Bitaxe pay/mo | Bitaxe median sat | Bitaxe swing | Bitaxe empty months | Bitaxe expected vs fair | Bitaxe simulated vs fair |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 50% industrial | 1% | 0x | 1% | page | 35.4 | 581 | 28.5 | 17,473 | 0.23 | 0.00% | 1.33 | 1.32 |
| 50% industrial | 1% | 1x | 1.50% | page | 35.2 | 291 | 24.8 | 19,335 | 0.28 | 0.00% | 1.20 | 1.18 |
| 50% industrial | 1% | 10x | 5.99% | page | 36.1 | 52.9 | 14.8 | 17,483 | 0.54 | 0.00% | 1.04 | 1.03 |
| 50% industrial | 1% | 0x | 1% | weak work | 35.4 | 581 | 24.9 | 17,473 | 0.20 | 0.00% | 1.00 | 1.00 |
| 50% industrial | 1% | 1x | 1.50% | weak work | 35.2 | 291 | 25.0 | 17,472 | 0.22 | 0.00% | 1.00 | 1.01 |
| 50% industrial | 1% | 10x | 5.99% | weak work | 36.1 | 52.9 | 24.9 | 17,481 | 0.23 | 0.00% | 1.00 | 1.00 |
| 90% industrial | 1% | 0x | 1% | page | 39.5 | 2,900 | 39.6 | 18,361 | 0.17 | 0.00% | 1.81 | 1.80 |
| 90% industrial | 1% | 1x | 1.10% | page | 37.1 | 1,451 | 38.4 | 17,483 | 0.19 | 0.00% | 1.68 | 1.70 |
| 90% industrial | 1% | 10x | 2.00% | page | 35.7 | 264 | 26.2 | 19,969 | 0.28 | 0.00% | 1.29 | 1.26 |
| 90% industrial | 1% | 0x | 1% | weak work | 39.5 | 2,900 | 25.0 | 17,475 | 0.19 | 0.00% | 1.01 | 1.01 |
| 90% industrial | 1% | 1x | 1.10% | weak work | 37.1 | 1,451 | 25.2 | 17,475 | 0.21 | 0.00% | 1.01 | 1.01 |
| 90% industrial | 1% | 10x | 2.00% | weak work | 35.7 | 264 | 24.9 | 17,479 | 0.21 | 0.00% | 1.00 | 1.00 |
| 90% industrial | 10% | 0x | 10% | page | 38.9 | 52.9 | 18.1 | 17,474 | 0.54 | 0.00% | 1.28 | 1.28 |
| 90% industrial | 10% | 1x | 11.00% | page | 39.3 | 44.6 | 17.0 | 17,488 | 0.62 | 0.00% | 1.25 | 1.23 |
| 90% industrial | 10% | 10x | 20.00% | page | 44.1 | 18.7 | 14.4 | 17,473 | 0.98 | 0.00% | 1.12 | 1.15 |
| 90% industrial | 10% | 0x | 10% | weak work | 38.9 | 52.9 | 24.8 | 17,474 | 0.24 | 0.00% | 1.00 | 1.00 |
| 90% industrial | 10% | 1x | 11.00% | weak work | 39.3 | 44.6 | 25.0 | 17,473 | 0.24 | 0.00% | 1.00 | 1.01 |
| 90% industrial | 10% | 10x | 20.00% | weak work | 44.1 | 18.7 | 24.6 | 17,478 | 0.31 | 0.00% | 1.00 | 0.99 |

### Study: a refusing coalition with share races (band-weak-levels, weak work, 50% industrial)

| Pool | Fork choice | INCLUDER_CUT | Refusing | Honest weak lost | Honest stale | Refusing stale | Tips replaced per share | Bitaxe vs fair | Carrying ASIC vs fair | Refusing ASIC vs fair | Coalition gains | Refusing ASIC gains |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1% | share work | 0.00% | 0.00% | 0.02% | 2.09% | - | 0 | 1.00 | 1.00 | 1.00 | - | -0.02% |
| 1% | share work | 0.00% | 50.00% | 4.92% | 2.10% | 2.13% | 0 | 0.96 | 1.01 | 1.01 | +1.20% | +1.20% |
| 1% | share work | 0.00% | 90.00% | 56.15% | 2.13% | 2.07% | 0 | 0.47 | 1.03 | 1.03 | +2.80% | +2.79% |
| 1% | share work | 1.00% | 0.00% | 0.02% | 2.09% | - | 0 | 1.00 | 1.00 | 1.00 | - | -0.23% |
| 1% | share work | 1.00% | 50.00% | 4.92% | 2.10% | 2.13% | 0 | 0.97 | 1.01 | 1.01 | +1.02% | +1.12% |
| 1% | share work | 1.00% | 90.00% | 56.15% | 2.13% | 2.07% | 0 | 0.47 | 1.03 | 1.03 | +2.75% | +2.78% |
| 1% | share work | 2.00% | 0.00% | 0.02% | 2.09% | - | 0 | 1.00 | 1.00 | 1.00 | - | -0.43% |
| 1% | share work | 2.00% | 50.00% | 4.92% | 2.10% | 2.13% | 0 | 0.97 | 1.02 | 1.01 | +0.84% | +1.04% |
| 1% | share work | 2.00% | 90.00% | 56.15% | 2.13% | 2.07% | 0 | 0.48 | 1.03 | 1.03 | +2.71% | +2.76% |
| 1% | share work | 5.00% | 0.00% | 0.02% | 2.09% | - | 0 | 1.00 | 1.01 | 1.00 | - | -1.05% |
| 1% | share work | 5.00% | 50.00% | 4.92% | 2.10% | 2.13% | 0 | 0.98 | 1.03 | 1.02 | +0.29% | +0.82% |
| 1% | share work | 5.00% | 90.00% | 56.15% | 2.13% | 2.07% | 0 | 0.50 | 1.04 | 1.04 | +2.58% | +2.71% |
| 1% | weak work | 0.00% | 0.00% | 0.02% | 2.09% | - | 0 | 1.00 | 1.00 | 1.00 | - | -0.02% |
| 1% | weak work | 0.00% | 50.00% | 4.65% | 1.15% | 3.06% | 0 | 0.97 | 1.01 | 1.01 | +1.14% | +1.11% |
| 1% | weak work | 0.00% | 90.00% | 55.85% | 0.78% | 2.26% | 0 | 0.47 | 1.03 | 1.03 | +2.78% | +2.75% |
| 1% | weak work | 1.00% | 0.00% | 0.02% | 2.09% | - | 0 | 1.00 | 1.00 | 1.00 | - | -0.23% |
| 1% | weak work | 1.00% | 50.00% | 4.65% | 1.15% | 3.06% | 0 | 0.97 | 1.01 | 1.01 | +0.95% | +1.03% |
| 1% | weak work | 1.00% | 90.00% | 55.85% | 0.78% | 2.26% | 0 | 0.48 | 1.03 | 1.03 | +2.74% | +2.73% |
| 1% | weak work | 2.00% | 0.00% | 0.02% | 2.09% | - | 0 | 1.00 | 1.00 | 1.00 | - | -0.43% |
| 1% | weak work | 2.00% | 50.00% | 4.65% | 1.15% | 3.06% | 0 | 0.97 | 1.02 | 1.01 | +0.77% | +0.96% |
| 1% | weak work | 2.00% | 90.00% | 55.85% | 0.78% | 2.26% | 0 | 0.48 | 1.03 | 1.03 | +2.69% | +2.72% |
| 1% | weak work | 5.00% | 0.00% | 0.02% | 2.09% | - | 0 | 1.00 | 1.01 | 1.00 | - | -1.05% |
| 1% | weak work | 5.00% | 50.00% | 4.65% | 1.15% | 3.06% | 0 | 0.98 | 1.03 | 1.02 | +0.22% | +0.73% |
| 1% | weak work | 5.00% | 90.00% | 55.85% | 0.78% | 2.26% | 0 | 0.50 | 1.04 | 1.04 | +2.56% | +2.67% |
| 1% | weak work, siblings | 0.00% | 0.00% | 0.02% | 2.09% | - | 0 | 1.00 | 1.00 | 1.00 | - | -0.02% |
| 1% | weak work, siblings | 0.00% | 50.00% | 2.31% | 0.65% | 47.63% | 0.297 | 1.04 | 1.06 | 1.06 | -5.38% | +6.46% |
| 1% | weak work, siblings | 0.00% | 90.00% | 53.92% | 0.54% | 9.46% | 0.072 | 0.50 | 1.06 | 1.06 | +2.40% | +5.85% |
| 1% | weak work, siblings | 1.00% | 0.00% | 0.02% | 2.09% | - | 0 | 1.00 | 1.00 | 1.00 | - | -0.23% |
| 1% | weak work, siblings | 1.00% | 50.00% | 2.31% | 0.65% | 47.63% | 0.297 | 1.05 | 1.07 | 1.07 | -5.61% | +6.38% |
| 1% | weak work, siblings | 1.00% | 90.00% | 53.92% | 0.54% | 9.46% | 0.072 | 0.51 | 1.06 | 1.06 | +2.36% | +5.83% |
| 1% | weak work, siblings | 2.00% | 0.00% | 0.02% | 2.09% | - | 0 | 1.00 | 1.00 | 1.00 | - | -0.43% |
| 1% | weak work, siblings | 2.00% | 50.00% | 2.31% | 0.65% | 47.63% | 0.297 | 1.05 | 1.07 | 1.07 | -5.84% | +6.29% |
| 1% | weak work, siblings | 2.00% | 90.00% | 53.92% | 0.54% | 9.46% | 0.072 | 0.52 | 1.06 | 1.06 | +2.31% | +5.81% |
| 1% | weak work, siblings | 5.00% | 0.00% | 0.02% | 2.09% | - | 0 | 1.00 | 1.01 | 1.00 | - | -1.05% |
| 1% | weak work, siblings | 5.00% | 50.00% | 2.31% | 0.65% | 47.63% | 0.297 | 1.06 | 1.08 | 1.07 | -6.51% | +6.04% |
| 1% | weak work, siblings | 5.00% | 90.00% | 53.92% | 0.54% | 9.46% | 0.072 | 0.54 | 1.07 | 1.07 | +2.19% | +5.76% |
| 10% | share work | 0.00% | 0.00% | 0.02% | 2.07% | - | 0 | 1.00 | 1.00 | 1.00 | - | -0.06% |
| 10% | share work | 0.00% | 50.00% | 4.93% | 2.11% | 2.10% | 0 | 0.97 | 0.97 | 1.02 | +1.83% | +1.83% |
| 10% | share work | 0.00% | 90.00% | 56.24% | 2.10% | 2.13% | 0 | 0.47 | 0.47 | 1.04 | +4.30% | +4.30% |
| 10% | share work | 1.00% | 0.00% | 0.02% | 2.07% | - | 0 | 1.00 | 1.00 | 0.99 | - | -0.97% |
| 10% | share work | 1.00% | 50.00% | 4.93% | 2.11% | 2.10% | 0 | 0.97 | 0.97 | 1.01 | +1.56% | +1.49% |
| 10% | share work | 1.00% | 90.00% | 56.24% | 2.10% | 2.13% | 0 | 0.48 | 0.48 | 1.04 | +4.23% | +4.22% |
| 10% | share work | 2.00% | 0.00% | 0.02% | 2.07% | - | 0 | 1.00 | 1.00 | 0.98 | - | -1.88% |
| 10% | share work | 2.00% | 50.00% | 4.93% | 2.11% | 2.10% | 0 | 0.98 | 0.98 | 1.01 | +1.28% | +1.14% |
| 10% | share work | 2.00% | 90.00% | 56.24% | 2.10% | 2.13% | 0 | 0.49 | 0.49 | 1.04 | +4.17% | +4.13% |
| 10% | share work | 5.00% | 0.00% | 0.02% | 2.07% | - | 0 | 1.00 | 1.00 | 0.95 | - | -4.62% |
| 10% | share work | 5.00% | 50.00% | 4.93% | 2.11% | 2.10% | 0 | 0.99 | 0.98 | 1.00 | +0.46% | +0.10% |
| 10% | share work | 5.00% | 90.00% | 56.24% | 2.10% | 2.13% | 0 | 0.51 | 0.51 | 1.04 | +3.97% | +3.88% |
| 10% | weak work | 0.00% | 0.00% | 0.02% | 2.07% | - | 0 | 1.00 | 1.00 | 1.00 | - | -0.00% |
| 10% | weak work | 0.00% | 50.00% | 4.68% | 1.25% | 2.99% | 0 | 0.97 | 0.97 | 1.02 | +1.73% | +1.74% |
| 10% | weak work | 0.00% | 90.00% | 55.72% | 0.78% | 2.23% | 0 | 0.48 | 0.48 | 1.04 | +4.26% | +4.26% |
| 10% | weak work | 1.00% | 0.00% | 0.02% | 2.07% | - | 0 | 1.00 | 1.00 | 0.99 | - | -0.91% |
| 10% | weak work | 1.00% | 50.00% | 4.68% | 1.25% | 2.99% | 0 | 0.97 | 0.97 | 1.01 | +1.45% | +1.40% |
| 10% | weak work | 1.00% | 90.00% | 55.72% | 0.78% | 2.23% | 0 | 0.48 | 0.48 | 1.04 | +4.19% | +4.17% |
| 10% | weak work | 2.00% | 0.00% | 0.02% | 2.07% | - | 0 | 1.00 | 1.00 | 0.98 | - | -1.82% |
| 10% | weak work | 2.00% | 50.00% | 4.68% | 1.25% | 2.99% | 0 | 0.98 | 0.98 | 1.01 | +1.17% | +1.06% |
| 10% | weak work | 2.00% | 90.00% | 55.72% | 0.78% | 2.23% | 0 | 0.49 | 0.49 | 1.04 | +4.13% | +4.09% |
| 10% | weak work | 5.00% | 0.00% | 0.02% | 2.07% | - | 0 | 1.00 | 1.00 | 0.95 | - | -4.56% |
| 10% | weak work | 5.00% | 50.00% | 4.68% | 1.25% | 2.99% | 0 | 0.99 | 0.99 | 1.00 | +0.34% | +0.03% |
| 10% | weak work | 5.00% | 90.00% | 55.72% | 0.78% | 2.23% | 0 | 0.51 | 0.51 | 1.04 | +3.93% | +3.84% |
| 10% | weak work, siblings | 0.00% | 0.00% | 0.02% | 2.09% | - | 2.9e-4 | 1.00 | 1.00 | 1.00 | - | -0.29% |
| 10% | weak work, siblings | 0.00% | 50.00% | 2.40% | 0.75% | 45.61% | 0.281 | 1.07 | 1.07 | 0.86 | -7.89% | -13.76% |
| 10% | weak work, siblings | 0.00% | 90.00% | 54.23% | 0.58% | 9.02% | 0.068 | 0.51 | 0.51 | 1.02 | +3.82% | +2.14% |
| 10% | weak work, siblings | 1.00% | 0.00% | 0.02% | 2.09% | - | 2.9e-4 | 1.00 | 1.00 | 0.99 | - | -1.20% |
| 10% | weak work, siblings | 1.00% | 50.00% | 2.40% | 0.75% | 45.61% | 0.281 | 1.08 | 1.08 | 0.86 | -8.23% | -14.20% |
| 10% | weak work, siblings | 1.00% | 90.00% | 54.23% | 0.58% | 9.02% | 0.068 | 0.52 | 0.52 | 1.02 | +3.76% | +2.05% |
| 10% | weak work, siblings | 2.00% | 0.00% | 0.02% | 2.09% | - | 2.9e-4 | 1.00 | 1.00 | 0.98 | - | -2.10% |
| 10% | weak work, siblings | 2.00% | 50.00% | 2.40% | 0.75% | 45.61% | 0.281 | 1.08 | 1.08 | 0.85 | -8.58% | -14.65% |
| 10% | weak work, siblings | 2.00% | 90.00% | 54.23% | 0.58% | 9.02% | 0.068 | 0.53 | 0.53 | 1.02 | +3.69% | +1.96% |
| 10% | weak work, siblings | 5.00% | 0.00% | 0.02% | 2.09% | - | 2.9e-4 | 1.00 | 1.00 | 0.95 | - | -4.84% |
| 10% | weak work, siblings | 5.00% | 50.00% | 2.40% | 0.75% | 45.61% | 0.281 | 1.09 | 1.09 | 0.84 | -9.62% | -15.98% |
| 10% | weak work, siblings | 5.00% | 90.00% | 54.23% | 0.58% | 9.02% | 0.068 | 0.55 | 0.55 | 1.01 | +3.50% | +1.69% |

### Study: miner mixes (INCLUDER_CUT 5%)

| Mix | Pool | Design | Miners | Largest miner | Hashrate at the minimum | Minimum / uniform | R | Weak expired | Bitaxe pay/mo | Bitaxe median sat | Bitaxe swing | Bitaxe empty months | Bitaxe fee loss | Bitaxe vs fair | ASIC pay/mo | ASIC median sat | ASIC swing | ASIC empty months | ASIC fee loss | ASIC vs fair |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| zipf | 1% | one-chain | 3,520 | 11.41% | 100.00% | 1.00 | - | - | 19.5 | 16,864 | 0.41 | 0.00% | 0.67% | 1.00 | 43.4 | 826,323 | 0.16 | 0.00% | 0.02% | 1.00 |
| zipf | 1% | band-weak-levels, weak work | 3,520 | 11.41% | 29.65% | 0.42 | 49.2 | 0.02% | 24.5 | 17,485 | 0.20 | 0.00% | 0.85% | 0.99 | 42.9 | 844,326 | 0.16 | 0.00% | 0.02% | 1.00 |
| zipf | 10% | one-chain | 28,456 | 9.23% | 100.00% | 1.00 | - | - | 2.41 | 168,580 | 1.31 | 45.60% | 0.09% | 0.94 | 170 | 168,580 | 0.14 | 0.00% | 0.07% | 1.00 |
| zipf | 10% | band-weak-levels, weak work | 28,456 | 9.23% | 67.75% | 0.77 | 39.3 | 0.01% | 24.8 | 17,475 | 0.26 | 0.00% | 0.85% | 1.00 | 432 | 81,283 | 0.05 | 0.00% | 0.18% | 0.99 |
| zipf | 30% | one-chain | 78,101 | 8.44% | 100.00% | 1.00 | - | - | 0.755 | 505,099 | 2.39 | 78.30% | 0.03% | 0.88 | 70.0 | 505,099 | 0.23 | 0.00% | 0.03% | 1.00 |
| zipf | 30% | band-weak-levels, weak work | 78,101 | 8.44% | 82.41% | 0.89 | 37.5 | 0.01% | 23.3 | 17,472 | 0.35 | 0.00% | 0.81% | 0.98 | 1,069 | 34,267 | 0.05 | 0.00% | 0.44% | 1.00 |
| medium | 1% | one-chain | 3,554 | 0.81% | 100.00% | 1.00 | - | - | 19.5 | 16,864 | 0.41 | 0.00% | 0.67% | 1.00 | 43.4 | 826,323 | 0.16 | 0.00% | 0.02% | 1.00 |
| medium | 1% | band-weak-levels, weak work | 3,554 | 0.81% | 9.98% | 0.22 | 74.8 | 0.02% | 24.1 | 17,476 | 0.21 | 0.00% | 0.85% | 0.97 | 43.1 | 841,153 | 0.17 | 0.00% | 0.02% | 1.00 |
| medium | 10% | one-chain | 35,614 | 0.08% | 100.00% | 1.00 | - | - | 2.41 | 168,580 | 1.31 | 45.60% | 0.09% | 0.94 | 170 | 168,580 | 0.14 | 0.00% | 0.07% | 1.00 |
| medium | 10% | band-weak-levels, weak work | 35,614 | 0.08% | 100.00% | 1.00 | 34.7 | 0.01% | 24.8 | 17,479 | 0.28 | 0.00% | 0.85% | 1.00 | 432 | 82,839 | 0.05 | 0.00% | 0.18% | 1.00 |
| medium | 30% | one-chain | 106,857 | 0.03% | 100.00% | 1.00 | - | - | 0.755 | 505,099 | 2.39 | 78.30% | 0.03% | 0.88 | 70.0 | 505,099 | 0.23 | 0.00% | 0.03% | 1.00 |
| medium | 30% | band-weak-levels, weak work | 106,857 | 0.03% | 100.00% | 1.00 | 34.7 | 0.01% | 23.0 | 17,475 | 0.40 | 0.00% | 0.80% | 0.97 | 1,030 | 27,613 | 0.05 | 0.00% | 0.42% | 1.00 |
| medium, few big | 1% | one-chain | 3,505 | 2.70% | 100.00% | 1.00 | - | - | 19.5 | 16,864 | 0.41 | 0.00% | 0.67% | 1.00 | 43.4 | 826,323 | 0.16 | 0.00% | 0.02% | 1.00 |
| medium, few big | 1% | band-weak-levels, weak work | 3,505 | 2.70% | 9.98% | 0.20 | 67.2 | 0.01% | 24.6 | 17,481 | 0.20 | 0.00% | 0.85% | 0.99 | 43.2 | 841,148 | 0.16 | 0.00% | 0.02% | 1.00 |
| medium, few big | 10% | one-chain | 35,119 | 0.27% | 100.00% | 1.00 | - | - | 2.41 | 168,580 | 1.31 | 45.60% | 0.09% | 0.94 | 170 | 168,580 | 0.14 | 0.00% | 0.07% | 1.00 |
| medium, few big | 10% | band-weak-levels, weak work | 35,119 | 0.27% | 100.00% | 1.00 | 34.7 | 0.01% | 24.4 | 17,472 | 0.29 | 0.00% | 0.85% | 0.98 | 432 | 82,839 | 0.05 | 0.00% | 0.18% | 1.00 |
| medium, few big | 30% | one-chain | 105,372 | 0.09% | 100.00% | 1.00 | - | - | 0.755 | 505,099 | 2.39 | 78.30% | 0.03% | 0.88 | 70.0 | 505,099 | 0.23 | 0.00% | 0.03% | 1.00 |
| medium, few big | 30% | band-weak-levels, weak work | 105,372 | 0.09% | 100.00% | 1.00 | 34.7 | 0.01% | 22.8 | 17,474 | 0.40 | 0.00% | 0.80% | 0.97 | 1,030 | 27,613 | 0.05 | 0.00% | 0.42% | 1.00 |

### Study: a big miner joining or leaving at once (50% industrial)

#### Settling

| Pool | Big miner | Its jobs | Minimum after / before | C before | C after | R before | R after | Minimum within 10% | C within 10% | R within 10% | Weak expired, first 6 h |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 1% | joins | node default | 1.03 | 45.5 | 60.9 | 35.7 | 36.7 | < 1 h | < 1 h | < 1 h | 0.00% |
| 1% | joins | at the minimum | 3.02 | 45.0 | 12.4 | 35.8 | 36.0 | 1 h | 8 h | < 1 h | 0.00% |
| 1% | leaves | node default | 0.97 | 60.9 | 44.6 | 36.6 | 35.7 | < 1 h | 1 h | < 1 h | 0.03% |
| 1% | leaves | at the minimum | 0.33 | 12.4 | 44.8 | 35.9 | 35.7 | 1 h | 2 h | < 1 h | 0.01% |
| 10% | joins | node default | 0.95 | 1.54 | 55.3 | 42.4 | 40.5 | 1 h | 2 h | 1 h | 0.01% |
| 10% | joins | at the minimum | 2.18 | 1.52 | 1.00 | 42.5 | 35.4 | 1 h | 1 h | 1 h | 0.03% |
| 10% | leaves | node default | 1.03 | 55.2 | 1.53 | 40.8 | 42.2 | < 1 h | 13 h | < 1 h | 0.02% |
| 10% | leaves | at the minimum | 0.46 | 1.00 | 1.53 | 35.3 | 42.3 | 1 h | 4 h | < 1 h | 0.03% |

#### Home miners' expected pay over fair, after the event

| Pool | Big miner | Its jobs | Before | 0-1 h | 1-6 h | 6-24 h | 1-2 d | 2-4 d | 4-8 d | 8-14 d |
|---|---|---|---|---|---|---|---|---|---|---|
| 1% | joins | node default | 1.02 | 2.01 | 1.89 | 1.41 | 1.01 | 1.01 | 1.01 | 1.02 |
| 1% | joins | at the minimum | 1.01 | 2.00 | 1.89 | 1.42 | 1.01 | 1.01 | 1.00 | 1.00 |
| 1% | leaves | node default | 1.02 | 0.52 | 0.55 | 0.66 | 0.86 | 1.00 | 1.01 | 1.01 |
| 1% | leaves | at the minimum | 1.00 | 0.50 | 0.53 | 0.65 | 0.86 | 1.01 | 1.01 | 1.01 |
| 10% | joins | node default | 1.00 | 1.88 | 1.14 | 1.01 | 1.00 | 1.01 | 1.00 | 1.01 |
| 10% | joins | at the minimum | 1.00 | 1.87 | 1.12 | 1.00 | 1.00 | 1.00 | 1.00 | 1.00 |
| 10% | leaves | node default | 1.01 | 0.51 | 0.80 | 0.99 | 0.99 | 0.99 | 1.00 | 1.00 |
| 10% | leaves | at the minimum | 1.00 | 0.52 | 0.81 | 1.00 | 0.99 | 1.00 | 1.00 | 1.00 |

### Study: a tiny pool at the ceiling (1 GH/s on a 37 GH/s test network)

| Design | Minimum / ceiling | Small share multiple | Large share multiple | R | Weak/s | Shares/s | Small pay/mo | Small median sat | Small swing | Small empty months | Small fee loss | Small vs fair | Large pay/mo | Large median sat | Large swing | Large empty months | Large fee loss | Large vs fair | Pays exactly as |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| one-chain, node default | 1.00 | 1.00 | 1.00 | - | 0 | 0.051 | 117 | 77,342,912 | 0.09 | 0.00% | 0.00% | 1.00 | 117 | 232,032,087 | 0.09 | 0.00% | 0.00% | 1.00 | - |
| band, node default | 1.00 | 64.0 | 64.0 | - | 0 | 8.0e-4 | 117 | 77,343,749 | 0.10 | 0.00% | 0.00% | 1.00 | 117 | 232,031,250 | 0.09 | 0.00% | 0.00% | 1.00 | - |
| band-weak, weak work, node default | 1.00 | 64.0 | 64.0 | 1.00 | 0 | 8.0e-4 | 117 | 77,343,749 | 0.10 | 0.00% | 0.00% | 1.00 | 117 | 232,031,250 | 0.09 | 0.00% | 0.00% | 1.00 | band, node default |
| band-weak-levels, weak work, node default | 1.00 | 64.0 | 64.0 | 1.00 | 0 | 8.0e-4 | 117 | 77,211,687 | 0.10 | 0.00% | 0.00% | 1.00 | 117 | 232,163,312 | 0.09 | 0.00% | 0.00% | 1.00 | - |
| band, all at the minimum | 1.00 | 1.00 | 1.00 | - | 0 | 0.051 | 117 | 77,342,912 | 0.09 | 0.00% | 0.00% | 1.00 | 117 | 232,032,087 | 0.09 | 0.00% | 0.00% | 1.00 | one-chain, node default |
| band-weak, weak work, all at the minimum | 1.00 | 1.00 | 1.00 | 1.00 | 0 | 0.051 | 117 | 77,342,912 | 0.09 | 0.00% | 0.00% | 1.00 | 117 | 232,032,087 | 0.09 | 0.00% | 0.00% | 1.00 | one-chain, node default |
| band-weak-levels, weak work, all at the minimum | 1.00 | 1.00 | 1.00 | 1.00 | 0 | 0.051 | 117 | 77,322,059 | 0.09 | 0.00% | 0.00% | 1.00 | 117 | 232,052,940 | 0.09 | 0.00% | 0.00% | 1.00 | - |

### Study: large miners filling the weak slots (band-weak-levels, weak work)

| Pool | Size | Flood | Pool with flood | INCLUDER_CUT | R | Bitaxe weak/mo | Bitaxe pay/mo | Bitaxe median sat | Bitaxe swing | Bitaxe empty months | Bitaxe expected vs fair | Bitaxe simulated vs fair | Flood's part of weak shares | Flood vs fair | Flood's cuts paid less received, of its fair income |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1,000 home miners | 0.03% | 0x | 0.03% | 1.00% | 34.7 | 9,718 | 1.45 | 309,549 | 0.81 | 22.70% | 1.00 | 1.04 | - | - | - |
| 1,000 home miners | 0.03% | 10x | 0.33% | 1.00% | 34.8 | 887 | 14.1 | 30,387 | 0.26 | 0.00% | 1.00 | 0.99 | 90.91% | 1.00 | 0.00% |
| 1,000 home miners | 0.03% | 100x | 3.01% | 1.00% | 34.7 | 96.5 | 24.7 | 17,513 | 0.23 | 0.00% | 1.00 | 1.00 | 98.92% | 1.00 | -0.00% |
| 1,000 home miners | 0.03% | 1000x | 29.76% | 1.00% | 34.7 | 9.49 | 23.5 | 17,822 | 0.38 | 0.00% | 1.00 | 0.97 | 99.89% | 1.00 | -0.00% |
| 1,000 home miners | 0.03% | 0x | 0.03% | 5.00% | 34.7 | 9,718 | 1.45 | 309,610 | 0.81 | 22.70% | 1.00 | 1.04 | - | - | - |
| 1,000 home miners | 0.03% | 10x | 0.33% | 5.00% | 34.8 | 887 | 14.1 | 30,388 | 0.26 | 0.00% | 1.00 | 0.99 | 90.91% | 1.00 | 0.01% |
| 1,000 home miners | 0.03% | 100x | 3.01% | 5.00% | 34.7 | 96.5 | 24.7 | 17,548 | 0.22 | 0.00% | 1.00 | 1.00 | 98.92% | 1.00 | -0.00% |
| 1,000 home miners | 0.03% | 1000x | 29.76% | 5.00% | 34.7 | 9.49 | 21.9 | 18,419 | 0.39 | 0.00% | 1.00 | 0.97 | 99.89% | 1.00 | -0.00% |
| 90% industrial | 1% | 0x | 1% | 1.00% | 39.5 | 2,900 | 24.9 | 17,476 | 0.20 | 0.00% | 1.02 | 1.01 | - | - | - |
| 90% industrial | 1% | 10x | 2.00% | 1.00% | 35.7 | 264 | 25.0 | 17,481 | 0.21 | 0.00% | 1.00 | 1.01 | 90.91% | 1.00 | 0.03% |
| 90% industrial | 1% | 100x | 10.98% | 1.00% | 35.1 | 27.5 | 24.6 | 17,543 | 0.27 | 0.00% | 1.00 | 0.99 | 94.76% | 1.00 | 0.01% |
| 90% industrial | 1% | 0x | 1% | 5.00% | 39.5 | 2,900 | 24.9 | 17,481 | 0.19 | 0.00% | 1.01 | 1.00 | - | - | - |
| 90% industrial | 1% | 10x | 2.00% | 5.00% | 35.7 | 264 | 24.8 | 17,474 | 0.21 | 0.00% | 1.00 | 1.00 | 90.91% | 1.00 | 0.14% |
| 90% industrial | 1% | 100x | 10.98% | 5.00% | 35.1 | 27.5 | 24.7 | 17,520 | 0.29 | 0.00% | 1.00 | 1.00 | 94.76% | 1.00 | 0.05% |

### Study: steering the level draw (band-weak-levels, weak work, 50% industrial)

| Pool | Steering miner | Level | Picks between | Blocks it finds a month | Draws where the other parent is better | Gain sat/mo | Orphan cost sat/mo | Net sat/mo | Net of its income |
|---|---|---|---|---|---|---|---|---|---|
| 1% | Bitaxe 1.2 TH/s | 5 | races only | 1.4e-3 | 23.30% | 0.122 | 0 | +0.122 | +0.00% |
| 1% | Bitaxe 1.2 TH/s | 5 | tip or its parent | 1.4e-3 | 23.30% | 5.70 | 5,312 | -5,306 | -1.22% |
| 1% | ASIC 100 TH/s | 7 | races only | 0.117 | 0.00% | 0 | 0 | +0 | +0.00% |
| 1% | ASIC 100 TH/s | 7 | tip or its parent | 0.117 | 0.00% | 0 | 0 | +0 | +0.00% |
| 10% | Bitaxe 1.2 TH/s | 5 | races only | 1.4e-3 | 16.94% | 0.021 | 0 | +0.021 | +0.00% |
| 10% | Bitaxe 1.2 TH/s | 5 | tip or its parent | 1.4e-3 | 16.94% | 0.998 | 3,598 | -3,597 | -0.83% |
| 10% | ASIC 100 TH/s | 7 | races only | 0.117 | 10.06% | 4.35 | 0 | +4.35 | +0.00% |
| 10% | ASIC 100 TH/s | 7 | tip or its parent | 0.117 | 10.06% | 203 | 178,144 | -177,941 | -0.49% |
| 30% | Bitaxe 1.2 TH/s | 5 | races only | 1.4e-3 | 16.42% | 1.8e-3 | 0 | +1.8e-3 | +0.00% |
| 30% | Bitaxe 1.2 TH/s | 5 | tip or its parent | 1.4e-3 | 16.42% | 0.085 | 3,664 | -3,664 | -0.85% |
| 30% | ASIC 100 TH/s | 7 | races only | 0.117 | 19.45% | 28.1 | 0 | +28.1 | +0.00% |
| 30% | ASIC 100 TH/s | 7 | tip or its parent | 0.117 | 19.45% | 1,308 | 361,729 | -360,422 | -1.00% |

### Study: weak shares lost to stale parents and uncle carriers (band-weak-levels, weak work, 50% industrial)

| Pool | Period | Stale | Losses | Weak lost | Weak expired | Bitaxe weak/mo | Bitaxe pay/mo | Bitaxe swing | Bitaxe empty months | Bitaxe expected vs fair | Bitaxe simulated vs fair | ASIC expected vs fair |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1% | 10 s | 2.10% | none | 0.00% | 0.02% | 581 | 24.7 | 0.20 | 0.00% | 1.00 | 0.99 | 1.01 |
| 1% | 10 s | 2.10% | stale parent | 1.05% | 0.02% | 582 | 24.7 | 0.20 | 0.00% | 0.99 | 1.00 | 1.02 |
| 1% | 10 s | 2.10% | uncle carrier drops | 0.00% | 0.02% | 581 | 24.7 | 0.20 | 0.00% | 1.00 | 0.99 | 1.01 |
| 1% | 10 s | 2.10% | both | 1.05% | 0.02% | 582 | 24.7 | 0.20 | 0.00% | 0.99 | 1.00 | 1.02 |
| 1% | 2 s | 8.70% | none | 0.00% | 0.02% | 2,901 | 24.5 | 0.20 | 0.00% | 1.00 | 0.99 | 1.00 |
| 1% | 2 s | 8.70% | stale parent | 4.35% | 0.01% | 2,915 | 24.3 | 0.21 | 0.00% | 0.98 | 0.98 | 1.02 |
| 1% | 2 s | 8.70% | uncle carrier drops | 0.00% | 0.02% | 2,901 | 24.5 | 0.20 | 0.00% | 1.00 | 0.99 | 1.00 |
| 1% | 2 s | 8.70% | both | 4.35% | 0.01% | 2,915 | 24.3 | 0.21 | 0.00% | 0.98 | 0.98 | 1.02 |
| 10% | 10 s | 2.10% | none | 0.00% | 0.02% | 38.7 | 24.8 | 0.25 | 0.00% | 1.00 | 1.00 | 1.00 |
| 10% | 10 s | 2.10% | stale parent | 1.05% | 0.01% | 38.7 | 24.8 | 0.26 | 0.00% | 1.00 | 1.00 | 0.99 |
| 10% | 10 s | 2.10% | uncle carrier drops | 0.00% | 0.02% | 38.7 | 24.8 | 0.25 | 0.00% | 1.00 | 1.00 | 1.00 |
| 10% | 10 s | 2.10% | both | 1.05% | 0.01% | 38.7 | 24.8 | 0.26 | 0.00% | 1.00 | 1.00 | 0.99 |
| 10% | 2 s | 8.70% | none | 0.00% | 0.02% | 291 | 24.6 | 0.21 | 0.00% | 1.00 | 0.99 | 0.99 |
| 10% | 2 s | 8.70% | stale parent | 4.35% | 0.01% | 292 | 24.3 | 0.21 | 0.00% | 0.98 | 0.98 | 0.97 |
| 10% | 2 s | 8.70% | uncle carrier drops | 0.00% | 0.02% | 291 | 24.6 | 0.21 | 0.00% | 1.00 | 0.99 | 0.99 |
| 10% | 2 s | 8.70% | both | 4.35% | 0.01% | 292 | 24.3 | 0.21 | 0.00% | 0.98 | 0.98 | 0.97 |

### Study constants: WEAK_DEPTH and INCLUDER_CUT against a refusing coalition (band-weak-levels, weak work, 50% industrial)

| Pool | Fork choice | WEAK_DEPTH | INCLUDER_CUT | Refusing | Honest weak lost | Bitaxe vs fair | Bitaxe swing | Coalition gains | Refusing ASIC gains | Lone refuser gains |
|---|---|---|---|---|---|---|---|---|---|---|
| 1% | weak work | 12 | 0.00% | 50.00% | 0.30% | 1.00 | 0.20 | +0.07% | +0.07% | -0.02% |
| 1% | weak work | 12 | 0.00% | 90.00% | 29.32% | 0.73 | 0.24 | +1.44% | +1.40% | -0.02% |
| 1% | weak work | 12 | 1.00% | 50.00% | 0.30% | 1.00 | 0.20 | -0.11% | -0.00% | -0.23% |
| 1% | weak work | 12 | 1.00% | 90.00% | 29.32% | 0.73 | 0.24 | +1.40% | +1.38% | -0.23% |
| 1% | weak work | 12 | 5.00% | 50.00% | 0.30% | 1.02 | 0.20 | -0.82% | -0.30% | -1.04% |
| 1% | weak work | 12 | 5.00% | 90.00% | 29.32% | 0.75 | 0.23 | +1.25% | +1.31% | -1.04% |
| 1% | weak work | 24 | 0.00% | 50.00% | 0.00% | 1.00 | 0.21 | -0.00% | -0.00% | +0.00% |
| 1% | weak work | 24 | 0.00% | 90.00% | 9.40% | 0.91 | 0.21 | +0.46% | +0.46% | +0.00% |
| 1% | weak work | 24 | 1.00% | 50.00% | 0.00% | 1.00 | 0.20 | -0.18% | -0.08% | -0.21% |
| 1% | weak work | 24 | 1.00% | 90.00% | 9.40% | 0.92 | 0.22 | +0.42% | +0.44% | -0.21% |
| 1% | weak work | 24 | 5.00% | 50.00% | 0.00% | 1.02 | 0.21 | -0.89% | -0.37% | -1.03% |
| 1% | weak work | 24 | 5.00% | 90.00% | 9.40% | 0.94 | 0.21 | +0.30% | +0.39% | -1.03% |
| 1% | weak work | 48 | 0.00% | 50.00% | 0.00% | 1.00 | 0.20 | -0.00% | -0.00% | +0.00% |
| 1% | weak work | 48 | 0.00% | 90.00% | 1.24% | 0.99 | 0.19 | +0.06% | +0.04% | +0.00% |
| 1% | weak work | 48 | 1.00% | 50.00% | 0.00% | 1.00 | 0.20 | -0.18% | -0.07% | -0.21% |
| 1% | weak work | 48 | 1.00% | 90.00% | 1.24% | 0.99 | 0.20 | +0.03% | +0.03% | -0.21% |
| 1% | weak work | 48 | 5.00% | 50.00% | 0.00% | 1.02 | 0.20 | -0.89% | -0.37% | -1.03% |
| 1% | weak work | 48 | 5.00% | 90.00% | 1.24% | 1.01 | 0.20 | -0.09% | -0.02% | -1.03% |
| 1% | weak work, siblings | 12 | 0.00% | 50.00% | 0.05% | 1.06 | 0.20 | -5.92% | +5.85% | -0.02% |
| 1% | weak work, siblings | 12 | 0.00% | 90.00% | 28.96% | 0.75 | 0.23 | +1.11% | +4.51% | -0.02% |
| 1% | weak work, siblings | 12 | 1.00% | 50.00% | 0.05% | 1.06 | 0.20 | -6.14% | +5.77% | -0.23% |
| 1% | weak work, siblings | 12 | 1.00% | 90.00% | 28.96% | 0.76 | 0.23 | +1.07% | +4.49% | -0.23% |
| 1% | weak work, siblings | 12 | 5.00% | 50.00% | 0.05% | 1.08 | 0.20 | -7.03% | +5.45% | -1.04% |
| 1% | weak work, siblings | 12 | 5.00% | 90.00% | 28.96% | 0.78 | 0.23 | +0.92% | +4.42% | -1.04% |
| 1% | weak work, siblings | 24 | 0.00% | 50.00% | 0.00% | 1.06 | 0.20 | -5.95% | +5.85% | +0.00% |
| 1% | weak work, siblings | 24 | 0.00% | 90.00% | 8.90% | 0.95 | 0.21 | +0.08% | +3.50% | +0.00% |
| 1% | weak work, siblings | 24 | 1.00% | 50.00% | 0.00% | 1.06 | 0.20 | -6.17% | +5.77% | -0.21% |
| 1% | weak work, siblings | 24 | 1.00% | 90.00% | 8.90% | 0.95 | 0.20 | +0.05% | +3.48% | -0.21% |
| 1% | weak work, siblings | 24 | 5.00% | 50.00% | 0.00% | 1.08 | 0.20 | -7.06% | +5.45% | -1.03% |
| 1% | weak work, siblings | 24 | 5.00% | 90.00% | 8.90% | 0.97 | 0.20 | -0.07% | +3.43% | -1.03% |
| 1% | weak work, siblings | 48 | 0.00% | 50.00% | 0.00% | 1.06 | 0.20 | -5.96% | +5.86% | +0.00% |
| 1% | weak work, siblings | 48 | 0.00% | 90.00% | 1.24% | 1.02 | 0.20 | -0.30% | +3.13% | +0.00% |
| 1% | weak work, siblings | 48 | 1.00% | 50.00% | 0.00% | 1.06 | 0.20 | -6.18% | +5.78% | -0.21% |
| 1% | weak work, siblings | 48 | 1.00% | 90.00% | 1.24% | 1.02 | 0.20 | -0.33% | +3.12% | -0.21% |
| 1% | weak work, siblings | 48 | 5.00% | 50.00% | 0.00% | 1.08 | 0.19 | -7.07% | +5.46% | -1.03% |
| 1% | weak work, siblings | 48 | 5.00% | 90.00% | 1.24% | 1.04 | 0.20 | -0.45% | +3.07% | -1.03% |
| 10% | weak work | 12 | 0.00% | 50.00% | 0.24% | 1.00 | 0.25 | +0.08% | +0.08% | -0.01% |
| 10% | weak work | 12 | 0.00% | 90.00% | 29.82% | 0.73 | 0.30 | +2.23% | +2.23% | -0.01% |
| 10% | weak work | 12 | 1.00% | 50.00% | 0.24% | 1.00 | 0.25 | -0.18% | -0.25% | -0.91% |
| 10% | weak work | 12 | 1.00% | 90.00% | 29.82% | 0.73 | 0.30 | +2.18% | +2.15% | -0.91% |
| 10% | weak work | 12 | 5.00% | 50.00% | 0.24% | 1.01 | 0.27 | -1.26% | -1.57% | -4.55% |
| 10% | weak work | 12 | 5.00% | 90.00% | 29.82% | 0.76 | 0.30 | +1.95% | +1.82% | -4.55% |
| 10% | weak work | 24 | 0.00% | 50.00% | 0.00% | 1.00 | 0.26 | -0.00% | -0.01% | -0.01% |
| 10% | weak work | 24 | 0.00% | 90.00% | 9.55% | 0.91 | 0.27 | +0.70% | +0.70% | -0.01% |
| 10% | weak work | 24 | 1.00% | 50.00% | 0.00% | 1.00 | 0.25 | -0.27% | -0.34% | -0.91% |
| 10% | weak work | 24 | 1.00% | 90.00% | 9.55% | 0.92 | 0.26 | +0.66% | +0.64% | -0.91% |
| 10% | weak work | 24 | 5.00% | 50.00% | 0.00% | 1.01 | 0.27 | -1.33% | -1.70% | -4.55% |
| 10% | weak work | 24 | 5.00% | 90.00% | 9.55% | 0.94 | 0.27 | +0.47% | +0.39% | -4.55% |
| 10% | weak work | 48 | 0.00% | 50.00% | 0.00% | 1.00 | 0.26 | -0.00% | -0.01% | -0.01% |
| 10% | weak work | 48 | 0.00% | 90.00% | 1.18% | 0.99 | 0.25 | +0.09% | +0.08% | -0.01% |
| 10% | weak work | 48 | 1.00% | 50.00% | 0.00% | 1.00 | 0.26 | -0.27% | -0.38% | -0.93% |
| 10% | weak work | 48 | 1.00% | 90.00% | 1.18% | 0.99 | 0.25 | +0.04% | -0.01% | -0.93% |
| 10% | weak work | 48 | 5.00% | 50.00% | 0.00% | 1.01 | 0.26 | -1.33% | -1.82% | -4.63% |
| 10% | weak work | 48 | 5.00% | 90.00% | 1.18% | 1.01 | 0.26 | -0.13% | -0.37% | -4.63% |
| 10% | weak work, siblings | 12 | 0.00% | 50.00% | 0.05% | 1.09 | 0.25 | -8.78% | -14.63% | -0.30% |
| 10% | weak work, siblings | 12 | 0.00% | 90.00% | 28.38% | 0.77 | 0.29 | +1.70% | +0.01% | -0.30% |
| 10% | weak work, siblings | 12 | 1.00% | 50.00% | 0.05% | 1.09 | 0.25 | -9.12% | -15.07% | -1.21% |
| 10% | weak work, siblings | 12 | 1.00% | 90.00% | 28.38% | 0.78 | 0.29 | +1.65% | -0.01% | -1.21% |
| 10% | weak work, siblings | 12 | 5.00% | 50.00% | 0.05% | 1.11 | 0.26 | -10.48% | -16.84% | -4.85% |
| 10% | weak work, siblings | 12 | 5.00% | 90.00% | 28.38% | 0.80 | 0.29 | +1.42% | -0.07% | -4.85% |
| 10% | weak work, siblings | 24 | 0.00% | 50.00% | 0.00% | 1.09 | 0.25 | -8.69% | -14.46% | -0.30% |
| 10% | weak work, siblings | 24 | 0.00% | 90.00% | 9.21% | 0.96 | 0.26 | +0.20% | -1.47% | -0.30% |
| 10% | weak work, siblings | 24 | 1.00% | 50.00% | 0.00% | 1.09 | 0.25 | -9.03% | -14.90% | -1.21% |
| 10% | weak work, siblings | 24 | 1.00% | 90.00% | 9.21% | 0.96 | 0.26 | +0.16% | -1.53% | -1.21% |
| 10% | weak work, siblings | 24 | 5.00% | 50.00% | 0.00% | 1.11 | 0.26 | -10.38% | -16.69% | -4.84% |
| 10% | weak work, siblings | 24 | 5.00% | 90.00% | 9.21% | 0.98 | 0.27 | -0.04% | -1.76% | -4.84% |
| 10% | weak work, siblings | 48 | 0.00% | 50.00% | 0.00% | 1.09 | 0.26 | -8.69% | -14.47% | -0.30% |
| 10% | weak work, siblings | 48 | 0.00% | 90.00% | 1.12% | 1.04 | 0.25 | -0.42% | -2.10% | -0.30% |
| 10% | weak work, siblings | 48 | 1.00% | 50.00% | 0.00% | 1.09 | 0.25 | -9.03% | -14.92% | -1.21% |
| 10% | weak work, siblings | 48 | 1.00% | 90.00% | 1.12% | 1.04 | 0.25 | -0.46% | -2.15% | -1.21% |
| 10% | weak work, siblings | 48 | 5.00% | 50.00% | 0.00% | 1.11 | 0.27 | -10.39% | -16.72% | -4.84% |
| 10% | weak work, siblings | 48 | 5.00% | 90.00% | 1.12% | 1.06 | 0.26 | -0.64% | -2.35% | -4.84% |

### Study constants: R_MAX and the start from genesis (50% industrial)

| Pool | R_MAX | R settles at | R gets within 10% | Weak found/s, first hour | Weak KB/s, first hour | Weak expired, first hour |
|---|---|---|---|---|---|---|
| 1% | 256 | 35.6 | 2 h | 11.1 | 8.85 | 50.08% |
| 1% | 1024 | 35.6 | 3 h | 45.8 | 36.6 | 86.94% |
| 1% | 4096 | 35.7 | 4 h | 174 | 139 | 95.47% |
| 10% | 256 | 42.2 | 2 h | 11.1 | 8.92 | 44.02% |
| 10% | 1024 | 42.1 | 3 h | 43.8 | 35.0 | 84.78% |
| 10% | 4096 | 42.3 | 4 h | 172 | 137 | 95.56% |
