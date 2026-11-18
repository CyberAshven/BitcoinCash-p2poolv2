# p2poolv2_payout_sim

An offline Monte Carlo of payouts under the Bitcoin Cash share chain designs
of [Share chain redesign for payout variance and fees](../docs/sharechain/small-miners-design.md)
(issue #79, row 2). For a 1.2 TH/s Bitaxe and a 100 TH/s ASIC it measures
how often they are paid, how large a typical payment is, how much their
monthly income swings, how often a month brings nothing and what spending
the payments costs, under each design, pool size, miner mix and Bitcoin Cash
hashrate. It also reports the pool's share, uncle and weak share rates and
bytes a second, and runs the attacks and studies listed under "Results".

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
`cargo run --release -p p2poolv2_payout_sim --bin payout-sim --`.
`--scenario` takes `all` or one table name; each study under "Results" names
its tables. Every run is a pure function of its inputs and seed (default 1),
drawn from `p2poolv2_netsim`'s seeded xoshiro256** generator, so the tables
come out the same on any machine. The whole default run, 1,000 simulated
months per scenario, takes about 10 minutes on 32 threads, one table at a
time, and needs several gigabytes of memory.

The tables are for one-minute Bitcoin Cash blocks (CHIP-2025-03). The
ten-minute tables are in this file at commit 2972cfd2, and those of the
first constants (`WEAK_DEPTH` 6, `INCLUDER_CUT` 5%, `R_MAX` 4,096) at
eb9b1ee3.

## The model

### Bitcoin Cash and the pool

- Bitcoin Cash's hashrate `H` is 3.7 EH/s (`docs/small-miners.md`), or 3,
  10, 100, 270 or 1,000 times that; a block every minute
  (`BCH_BLOCK_INTERVAL_SECONDS`), 0.3125 BCH a block, fees left out; 1%
  donation on every share (`cuts-per-share`, so a payee gets 99% of its
  slice).
- The pool holds a fraction `f` of `H` (1, 5, 10, 20 or 30%, more in the
  scale tables) and finds Bitcoin Cash blocks as a Poisson process at
  `f / 60` a second. Under `mini` each chain finds blocks at its own part of
  `H`.
- A month is 30 days (43,200 blocks). A miner's fair income is its share of
  `H` of a month's blocks, less the donation: 433,459 sat for the Bitaxe
  today.

### Miners

- Mixes: `home` (every miner a 1.2 TH/s home miner), `50% industrial` and
  `90% industrial`, the industrial part split equally by hashrate between
  1 PH/s and 10 PH/s machines, each its own miner (own address and node
  default). The studies add `99% industrial`; `zipf` (the `k`-th largest
  miner has `1/k` of the largest one's hashrate, the smallest a home
  miner's); `medium` (90% on 30 to 300 TH/s miners, 10% home); and
  `medium, few big` (80% medium, 10% home, 10% 1 PH/s farms). Counts may be
  fractional: a class stands for its share of the hashrate.
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
  any other period. In steady state that is three minutes over `f`: 5 hours
  at 1% of BCH. Only pools under about 0.015% of BCH reach the cap.
- A block pays each payee its credited work in the window over the
  window's work, rounded down to a satoshi, after its 1% donation.

### The designs

1. `mini`: the rule before issue #79. Every share on a chain at that
   chain's one difficulty; miners under 10 TH/s on the mini chain, the rest
   on main, each chain with its own blocks and window.
2. `one-chain`: one chain at one difficulty.
3. `band`: one chain with the difficulty band at a fixed cap `C` (64 unless
   the band cap table says otherwise). The node default gives a miner the
   minimum unless it would then make more than
   `max(4 shares per pool block, 300 a month)`; then it gets `min(C, ratio)`
   times the minimum, sized to make that many. 4 shares a pool block passes
   300 a month from 0.17% of BCH (1,728 a month at 1%, 17,280 at 10%).
4. `vardiff-cd`: Cash Dragon's chain (`docs/sharechain/vardiff-sharechain.md`
   on `alpha-vardiff-v2`). Each miner's tier is `w_i * D_asert`, its share of
   the hashrate times the ASERT difficulty, never easier than the ceiling, so
   every miner makes shares at the same rate. The floor,
   `W_min / W_total * D_asert` over the 120,960-entry floor window with
   presence `N` = 3, binds for nobody unless miners number more than about
   40,000.
5. `band-weak`: the band and weak shares, every payment direct.
6. `band-weak-levels`: the band, weak shares and payout levels.

`band-weak-levels, weak work` is design 6 with every proof on a job at the
minimum credited at the weak work, which the weak share credit study below
explains. The studies from #85 on use it.

### Weak shares

Only miners on jobs at the minimum make weak shares: hashes that meet the
weak target `T_asert * R` but not the share target, `R - 1` of them per full
share. The pool's weak shares are simulated share by share (400,000 chain
shares after 20,000 to settle): `R` moves by 1/256 per share as the page's
fixed-point rule moves it, from `R_MAX` at the start; weak shares are found
during the exponential time between two chain shares; a new share carries up
to `WEAK_CAP` of them, oldest parent first, and a weak share whose parent is
more than `WEAK_DEPTH` shares below expires uncredited. A share from a
non-carrying node carries only its own node's weak shares. A weak share is
credited its weak work, `INCLUDER_CUT` of it to the carrying share's miner;
weak work counts in the window's work and takes no window slots.

The defaults are the share-chain rules while no weak share asks:
`WEAK_CAP` 64 (the rules' `WEAK_IDLE_CAP`), `WEAK_FILL` 32, `WEAK_DEPTH` 48,
`INCLUDER_CUT` 1% and `R_MAX` 256 (`src/constants.rs`). Only the tables of
the last two studies run the asks.

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
  (except in the study tables that race shares), orphans, propagation, the
  uncle depth limit, and what checking and relaying weak shares costs a
  node (`p2poolv2_netsim` and the swarm have those).
- The band cap and the weak ratio as dynamics over time, except in the
  tables that run the chain share by share (`arrivals`, `r-max`,
  `band-step` and the path tables of the asks).
- `WEAK_BYTES`, transaction fees, payout trees, share trading, operator
  fees, tokens and SAFA.
- In the study tables: two shares per race, a coin toss at equal work (no
  first-seen propagation), and a race's loser always named as an uncle; a
  sibling of a light tip always carries everything waiting and wins;
  coalition incomes in the refusing and depth tables are expected from the
  credit, not simulated payments; arrivals take the nodes' hashrate
  estimates as exact; the stale-parent rate of the losses table is an
  estimate; the tiny pool's network is hypothetical; the asks' tables
  simulate counts only and let every node see every weak share at once.

## Results

All tables are `cargo payout-sim` with the defaults above, 1,000 months per
scenario, seed 1, and are under "Tables from the default run". Columns:
payments a month (blocks paying the miner an output); median payment;
monthly swing, the standard deviation of monthly income over its mean; the
share of months with no payment; fee loss; and income over fair income
(`vs fair`, simulated unless a column says expected). A difference of a few
percent in `vs fair` between rows is noise where payments are rare (the
larger pools and the x270 rows).

### Designs and pool size

`--scenario designs`, `hashrate`, `pool`. How often is each miner paid, and
how much does its income swing, under each design, pool size and mix?

Without weak shares a Bitaxe in a pool at 30% of BCH (50% industrial) has
no payment in 78% of months on one chain (60% on `mini`, 61% under
`vardiff-cd`). With the
band, weak shares, payout levels and the weak-work credit it is paid about
24 times every month at every pool size, each payment about its
17,472-sat level, at 0.85% fee loss; the ASIC is paid 430 to 1,970 times a
month at 0.2 to 0.8% fee loss. Weak shares cost about 2.9 KB/s of messages
at any pool size, against 0.1 KB/s of shares.

### Weak share credit

`--scenario weak-credit`. Does the page's credit pay every hash once?

No. A share on a minimum job is credited its full work and each weak share
its weak work besides, so a hash at the minimum earns `2 - 1/R` times what a
hash on a heavier job earns: at 1% of BCH with 90% industrial the Bitaxe
gets 1.81 of fair and the heavier miners 0.91, and ignoring the node default
pays. Crediting a share on a minimum job only the weak work of its parent's
weak target (`weak work`) puts every miner within noise of fair, and lowers
the Bitaxe's swing at 10% and 30% of BCH from 0.72 and 1.19 to 0.27 and
0.39.

### Band cap

`--scenario band-cap`. What does the difficulty band alone do?

It helps only where heavy miners would make more than the node default's 4
shares a pool block: at 1% of BCH with 90% industrial it lowers the minimum
to 0.12 of the uniform difficulty and the Bitaxe's swing from 0.39 to 0.14.
At 10% and 30% no miner is above that target and the band changes nothing.
The `C settles at` column uses the first, bits-driven cap rule, which stays
at 1 unless more than half of all chain shares want heavier shares; the
band step study has the rule as built.

### WEAK_CAP and WEAK_FILL

`--scenario weak-constants`. How many weak shares should a share carry?

With the weak-work credit, 64 and 32 pay the Bitaxe every month at 30% of
BCH (swing 0.39) for 2.9 KB/s. 16 and 8 leave 7.5% of its months empty;
128 and 64 lower the swing to 0.31 for twice the bytes; a fill of 48 of 64
gives 0.36.

### INCLUDER_CUT

`--scenario includer-cut`. What should a share earn for the weak shares it
carries?

Home miners at the minimum get the cut back as includers, so the Bitaxe
stays at fair at every cut; a lone non-carrying node loses its includer
income, 0.99 of fair at a 1% cut, 0.98 at 2% and 0.95 at 5%. Study #85 set
the rule at 1%.

### Default payout level

`--scenario levels`. Which payout level should a node default to?

Level 5 (17,472 sat): about 25 payments a month and 0.85% fee loss at every
pool size with the weak-work credit. Level 6 halves the fee loss and the
payments and raises the swing from 0.27 to 0.34 at 10% of BCH.

### Share period

`--scenario period`. Do shorter shares help small miners?

On one chain, yes: the Bitaxe's swing at 10% of BCH falls from 1.28 at
10 s to 0.36 at 1 s. Once weak shares carry the credit, little: 0.27 at
10 s and 0.20 at 1 s, at ten times the bytes. "Scale levers" compares the
period with more weak shares a share.

### Attacks

`--scenario attack-minimum`, `attack-carriers`, `attack-spam`.

- **(a) The farms' node gives them the minimum.** With the weak-work credit
  nobody's income changes and the Bitaxe's swing rises from 0.20 to 0.21 at
  1% of BCH; at 10% and 30% nobody is above the minimum anyway. Under the
  band alone the minimum returns to uniform and the Bitaxe gets one chain's
  numbers, no worse than before the band.
- **(b) Nodes that never carry others' weak shares**, at `WEAK_DEPTH` 48 and
  a 1% cut: with half the hashrate on them no honest weak share expires;
  with 90%, 1.2% (1% of BCH) and 1.4% (10%), and the honest Bitaxe gets 1.00
  and 0.99 of fair.
- **(c) Weak share spam** at 1 or 10 times the honest home hashrate: the
  spam raises the minimum, so the honest Bitaxe finds 11 times fewer weak
  shares, but its payments go only from 25 to 24 a month, its swing from
  0.20 to 0.23, and its income stays fair. The spam is paid for its work and
  takes nothing else.

### Study #85: weak work in fork choice, with a smaller includer cut

`--scenario refusing-races`, `weak-depth`, `mixes`, `arrivals`,
`tiny-pool`, `floods`, `draw-steering`, `losses`, `r-max`, `r-max-mixes`.
The study on the design page: should weak work count in fork choice, and
which `WEAK_DEPTH`, `INCLUDER_CUT` and `R_MAX` stop a coalition that refuses
to carry others' weak shares?

`WEAK_DEPTH` is the lever. With a 1% cut a 90% refusing coalition gains
+2.8% / +5.6% (1% / 10% of BCH) at depth 6 and +0.03% / +0.05% at 48, where
the Bitaxe keeps 0.99 of fair instead of 0.49. Weak work in fork choice
moves a coalition's gain by at most 0.04 points through races alone; it
bites only if carrying nodes also mine siblings of light tips. Depth 48, a
1% cut and `R_MAX` 256 are the share-chain rules since PR #98 and this
model's defaults.

How the study tables are made: `src/races.rs` races shares at
`s / (1 - s)` per share and lets fork choice pick by share work (`share
work`) or by share work plus the carried weak work (`weak work`);
`weak work, siblings` adds carrying nodes that keep mining on the parent of
a light tip from a non-carrying node. A coalition's gain is the refusing
classes' expected income over what the same classes earn when everyone
carries. `src/transition.rs` runs `arrivals` and `r-max` share by share:
ASERT anchored at the parent, the band cap and the weak ratio stepped as the
page has them, 8 runs of 21 days per case.

| Table | Question | Answer |
|---|---|---|
| `refusing-races` | Does weak work in races change what refusing earns? | Refusing shares lose their races to carrying ones, but races are 2% of shares: a half-hashrate coalition loses 0.2 to 0.4% at the 1% cut, a 90% one gains at most 0.11%. With siblings of light tips, a half-hashrate coalition loses 5.6 to 15.2% at every cut |
| `weak-depth` | Which `WEAK_DEPTH` and `INCLUDER_CUT` stop a coalition? | Depth: honest weak shares lost fall from 55% at 6 to 1.3% at 48. A lone refuser loses 0.9 to 1.0% at a 1% cut and 4.6 to 4.9% at 5% |
| `mixes` | Does the design hold in other miner mixes? | In every mix and size the Bitaxe gets 24 to 25 payments a month and 0.97 to 1.01 of fair |
| `arrivals` | What happens when a miner with half the pool joins or leaves at once? | The minimum and `R` are within 10% of their new levels within an hour in most cases (2 to 3 hours for a miner on the node default joining at 10% of BCH). Home miners' pay follows the window: 1.9 times fair in the first hour after the pool doubles at 1% of BCH, fair after 6 hours |
| `tiny-pool` | Does a 1 GH/s pool at the ceiling work? | `R` is 1 and no weak share is found; the design pays as one chain does, the small miner 1.04 of fair and the large one 0.99 |
| `floods` | Do weak share floods of 10 to 1,000 times the home hashrate hurt small miners? | No: the flood takes up to 99.9% of the weak shares but brings blocks, and the Bitaxe is paid 18.5 to 24.8 times a month at fair |
| `draw-steering` | Can a miner steer the level draw by picking its parent? | It wins at most 170 sat a month between racing shares; always picking the tip's parent loses more to stale shares than it wins |
| `losses` | What do weak shares on parents that leave the chain cost? | 1.05% of weak shares at 10 s, 4.35% at 2 s; the Bitaxe gets 0.97 to 1.01 of fair |
| `r-max`, `r-max-mixes` | Which `R_MAX`? | `R` settles at 35 to 88 wherever any hashrate mines at the minimum, under 256 and 4,096 alike. From genesis 256 brings `R` within 10% in 2 hours instead of 4, with 10 to 14 weak shares a second in the first hour instead of 169 to 228 |

### Scale

`--scenario scale`. Does pay stay fair in larger pools on a larger Bitcoin
Cash, and when does a small miner stop being paid every month?

The pool runs at 10, 30, 50 and 90% of Bitcoin Cash and at 1 to 1,000 times
today's hashrate, every payout level as a node sets it
(`level_policy::affordable_payout_level`: the highest level up to 5 that
still expects three payments a month). Pay stays fair in every row. A pool
makes `WEAK_FILL` weak shares a share whatever its size, split among its
miners at the minimum, so a small miner is paid less often as the home
miners grow: at ten times today's hashrate a Bitaxe is paid 4.7 times a
month at 10% of Bitcoin Cash and under once at 90%; at 270 times (Bitcoin's
hashrate) a 100 TH/s miner is still paid 2.9 to 7.6 times a month. The
table's floor column is the smallest miner that still finds three weak
shares a month: `3 * (minimum difficulty / R) * 2^32 / a month`.

### Scale levers

`--scenario scale-levers`. Once small miners need more credit, is a shorter
share period (section 5 of the design page) the lever, or more weak shares
a share?

More weak shares a share. Each pool at 10 and 30% of Bitcoin Cash runs at
10-second shares, at 5, 2 and 1-second shares, and at 10-second shares
carrying 2, 5 and 10 times the weak shares. Today neither lever changes
anything for a small miner. More credit is first needed at about 3.7 EH/s
of pool hashrate; then both levers pay alike (at ten times today's hashrate
in a pool at 30%, a Bitaxe's empty months fall from 41% to 19%, 4 to 6% and
2%), but the shorter periods run at 3.8%, 8.7% and 15.9% stale shares where
10-second shares stay at 2.1%. The asks study builds on this.

The network simulation agrees on 1,000 nodes (`cargo netsim run --nodes
1000 --share-interval S --weak-shares N --weak-share-carriage gossip-only`,
8,640 share intervals each, from `cargo netsim sweep scaling-weak-shares`):

| Share period | Weak shares a share | Weak shares a second | Stale | Upload a node, mean (Mbit/s) |
|---|---:|---:|---:|---:|
| 10 s | 32 | 3.2 | 3.83% | 0.35 |
| 5 s | 32 | 6.4 | 8.01% | 0.75 |
| 2 s | 32 | 16 | 17.89% | 1.88 |
| 1 s | 32 | 32 | 30.43% | 3.78 |
| 10 s | 64 | 6.4 | 3.83% | 0.52 |

### Band step

`--scenario band-step`. How fast should the band cap move, and can
hashrate that overstates the difficulty it wants push it?

A chain from genesis for two days, 50% industrial, 8 seeds a row. The cap
`C` starts at 1 and moves one step a share: up by `C * u / divisor` when
the parent's node wanted heavier shares than the cap allowed, down by
`C / divisor` otherwise; the rule's divisor is 512. In the last two rows of
each pool, part of the hashrate mines at the minimum while every share it
finds records the largest wanted difficulty.

Where `C` settles does not depend on the step: 28 to 31 at 1% of Bitcoin
Cash, 1.00 at 10%. The divisor sets only the speed: within 10% after 1, 4
and 12 hours at 128, 512 and 2,048. Overstating moves the cap (to 48 to 63)
and nothing else: home miners stay at 1.00 to 1.02 of fair. The first six
hours of a new chain pay home miners 0.80 to 0.88 of fair at 1% of BCH
whatever the step, from the weak ratio settling from `R_MAX`, not from the
cap.

### Window cap

`--scenario window-cap`. Should the PPLNS window's 14-day cap be longer?

No. Pools of medium miners at 0.01 to 1% of today's Bitcoin Cash run with
caps of 14, 30 and 60 days. The cap binds only below about 0.015% of BCH
(0.55 PH/s today), and even there a miner that keeps mining is paid the
same under every cap (the Bitaxe has 0.9% of its months empty at 0.01%) and
fairly. A longer cap only changes the single share: at 0.01%, 13% of shares
are paid by no block with 14 days and 5% with 30, which matters to a miner
that mines a few days and leaves. It costs every node, since the cap sets
the payout depth, the reorg limit and the bodies kept.

### Storage

`--scenario storage`, arithmetic with no simulation and no fees. What does
a node keep, by payees, payout level and window cap?

The payout level, not the payees, bounds a coinbase: a block pays at most
the reward over the level, 1,788 outputs (61 KB) at level 5 and 28,617
(973 KB) at level 1, inside BCH's 1,000,000-byte transaction limit, so the
reward alone never reaches the payout tree. Coinbases are most of what a
node keeps: 8.3 GB for 28 days with 1,000 payees, 14.8 GB at level 5's
ceiling, 236 GB at level 1's. Weak shares take 4.3 GB for 28 days at 32 a
share and 43 GB at 320; headers 3.15 GB a year. Each 1 BCH of fees adds
about 5,700 outputs at level 5.

### Weak shares that follow the asks

`--scenario weak-asks`, `weak-asks-mixes`, `weak-asks-constants`,
`weak-asks-attacks`, or `cargo payout-sim run --asks`. Section 6 of the
design page, built in PR #118: a weak share asks for an easier weak target
when its node wants one for its miner (fewer than 16 weak shares a month),
and while more than one in `WEAK_ASK_DIVISOR` (64) of the weak shares a
share carried ask, the weak ratio rises instead of steering towards
`WEAK_FILL`, until a share carries more than `WEAK_FILL_MAX` (320). Does the
count follow the need, and can attacks abuse it?

It follows the need. At today's hashrate a pool at 10% of Bitcoin Cash stays
at the floor and one at 30% opens to 57 weak shares a share; at ten times
today's hashrate they settle at 189 and the ceiling, 344. Where the ceiling
is reached the asks pay as `WEAK_FILL` 320 does (a Bitaxe at 10% of ten
times today's has 1.6% of its months empty at 15 KB/s, against 1.3% at
28 KB/s and 10% at 2.9 KB/s without asks), and where it is not they relay
less. Every mix opens. Attacks reach the ceiling or the floor and no
further: 5% of the hashrate asking whatever it finds takes a pool that needs
nothing to 315 a share, and 99% never asking brings one that needs more to
57.

`src/asks.rs` runs the weak share queue share by share with the asks,
counts only. A share carries up to 640 weak shares, the oldest first. The
`fill 320` rows are the rules without asks at `WEAK_FILL` 320, `WEAK_CAP`
640 and `R_MAX` 2,560.

### The weak ratio's path with the asks

`--scenario weak-asks-genesis`, `weak-asks-big-miner`, `weak-asks-rules`,
`weak-asks-rule-mixes`, `weak-asks-rule-attacks`. The first build of PR #118
ran a 12-node swarm out of memory: from genesis, at a weak ratio of 256,
every share carried 640 weak shares. Does the rule bound the path from
genesis and after a big miner, and which change fixes it?

The flaw is the start: from genesis in a pool at 10% of today's Bitcoin
Cash the rule as written peaks at 162 weak shares a share, and 819 of the
first 2,100 shares carry more than 64. v4 fixes it: from genesis no share
carries more than 64 and no weak share expires, and where more than one
weak share in 64 asks every steady state of the asks holds. It gives up
part of the gain where fewer than one in 64 ask (49.5 a share instead of 82
in the 99% industrial mix), loses 1.5% of the weak shares where the asks
switch on and off, and opens no new attack. v4 is the rule PR #118
built: a share with more than 64 weak shares is valid only if
`weak_asking * 64 > weak_count`, and the genesis weak ratio is 32.

| Rule | What changes from section 6 as written |
|---|---|
| today | `WEAK_CAP` 64, no asks |
| v0 as written | Nothing |
| v1 down to R/2 (R/4) | A share that was not asked and carried more than 32 takes the next ratio to `R * 32 / count` at once, never below `R / 2` (`R / 4`) |
| v2 cap follows asks | A share may carry more than 64 weak shares only when more than one in 64 of those it carries ask |
| v3 genesis 32 | The genesis share's weak ratio is 32, not 256 |
| v4 = v2 + v3 | Both |
| v5 = v4, one ask opens | v4, but one asking weak share is enough for more than 64 |

`src/transition.rs` runs the chain share by share under each rule; each
case averages eight runs of the 50% industrial mix, two days from genesis or
a big miner at the minimum with the pool's hashrate joining or leaving a
day after genesis. "Count stays within 25%" is the first hour from which
every hourly count is within 25% of the last day's. The test network pool
is 10 GH/s of 10 MH/s miners on 3.7 TH/s, where the chain's ceiling holds
the weak ratio in force near 5; its big miner is 1 TH/s, and its last rows
add 1% of the hashrate asking whatever it finds.

## Code

| What | Where |
|---|---|
| Constants and their sources | `src/constants.rs` |
| Miner classes, mixes, tracked miners | `src/miners.rs` |
| Steady-state difficulties: uniform, band, ABLA settle, vardiff | `src/difficulty.rs` |
| The window sized by work | `src/window.rs` |
| The weak share queue and the weak ratio | `src/weak.rs` |
| The weak share queue under the asks and its variants | `src/asks.rs` |
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
| 0.01% | 0.370 | 4.32 | 14 days | 14.0 days | 2.02 | 13.32% | 14.0 days | 5 | 4.36 | 0.90% | 0.49 | 1.01 | 5 | 4.36 | 0.90% | 0.49 | 1.01 |
| 0.01% | 0.370 | 4.32 | 30 days | 20.8 days | 3.00 | 4.98% | 20.8 days | 5 | 4.36 | 0.90% | 0.48 | 1.01 | 5 | 4.36 | 0.90% | 0.48 | 1.01 |
| 0.01% | 0.370 | 4.32 | 60 days | 20.8 days | 3.00 | 4.98% | 20.8 days | 5 | 4.36 | 0.90% | 0.48 | 1.01 | 5 | 4.36 | 0.90% | 0.48 | 1.01 |
| 0.02% | 0.740 | 8.64 | 14 days | 10.4 days | 3.00 | 4.98% | 10.4 days | 5 | 8.64 | 0.00% | 0.34 | 1.00 | 5 | 8.64 | 0.00% | 0.34 | 1.00 |
| 0.02% | 0.740 | 8.64 | 30 days | 10.4 days | 3.00 | 4.98% | 10.4 days | 5 | 8.64 | 0.00% | 0.34 | 1.00 | 5 | 8.64 | 0.00% | 0.34 | 1.00 |
| 0.02% | 0.740 | 8.64 | 60 days | 10.4 days | 3.00 | 4.98% | 10.4 days | 5 | 8.64 | 0.00% | 0.34 | 1.00 | 5 | 8.64 | 0.00% | 0.34 | 1.00 |
| 0.05% | 1.85 | 21.6 | 14 days | 4.2 days | 3.00 | 4.98% | 4.2 days | 5 | 21.1 | 0.00% | 0.23 | 1.00 | 5 | 21.7 | 0.00% | 0.21 | 1.00 |
| 0.05% | 1.85 | 21.6 | 30 days | 4.2 days | 3.00 | 4.98% | 4.2 days | 5 | 21.1 | 0.00% | 0.23 | 1.00 | 5 | 21.7 | 0.00% | 0.21 | 1.00 |
| 0.05% | 1.85 | 21.6 | 60 days | 4.2 days | 3.00 | 4.98% | 4.2 days | 5 | 21.1 | 0.00% | 0.23 | 1.00 | 5 | 21.7 | 0.00% | 0.21 | 1.00 |
| 0.10% | 3.70 | 43.2 | 14 days | 2.1 days | 3.00 | 4.98% | 2.1 days | 5 | 24.9 | 0.00% | 0.21 | 1.01 | 5 | 43.0 | 0.00% | 0.16 | 0.99 |
| 0.10% | 3.70 | 43.2 | 30 days | 2.1 days | 3.00 | 4.98% | 2.1 days | 5 | 24.9 | 0.00% | 0.21 | 1.01 | 5 | 43.0 | 0.00% | 0.16 | 0.99 |
| 0.10% | 3.70 | 43.2 | 60 days | 2.1 days | 3.00 | 4.98% | 2.1 days | 5 | 24.9 | 0.00% | 0.21 | 1.01 | 5 | 43.0 | 0.00% | 0.16 | 0.99 |
| 0.20% | 7.40 | 86.4 | 14 days | 1.0 days | 3.00 | 4.98% | 1.1 days | 5 | 25.5 | 0.00% | 0.20 | 1.03 | 5 | 86.9 | 0.00% | 0.12 | 1.00 |
| 0.20% | 7.40 | 86.4 | 30 days | 1.0 days | 3.00 | 4.98% | 1.1 days | 5 | 25.5 | 0.00% | 0.20 | 1.03 | 5 | 86.9 | 0.00% | 0.12 | 1.00 |
| 0.20% | 7.40 | 86.4 | 60 days | 1.0 days | 3.00 | 4.98% | 1.1 days | 5 | 25.5 | 0.00% | 0.20 | 1.03 | 5 | 86.9 | 0.00% | 0.12 | 1.00 |
| 0.50% | 18.5 | 216 | 14 days | 0.4 days | 3.00 | 4.98% | 0.4 days | 5 | 24.7 | 0.00% | 0.20 | 1.00 | 5 | 216 | 0.00% | 0.08 | 0.99 |
| 0.50% | 18.5 | 216 | 30 days | 0.4 days | 3.00 | 4.98% | 0.4 days | 5 | 24.7 | 0.00% | 0.20 | 1.00 | 5 | 216 | 0.00% | 0.08 | 0.99 |
| 0.50% | 18.5 | 216 | 60 days | 0.4 days | 3.00 | 4.98% | 0.4 days | 5 | 24.7 | 0.00% | 0.20 | 1.00 | 5 | 216 | 0.00% | 0.08 | 0.99 |
| 1% | 37.0 | 432 | 14 days | 0.2 days | 3.00 | 4.98% | 0.2 days | 5 | 24.6 | 0.00% | 0.20 | 0.99 | 5 | 432 | 0.00% | 0.05 | 1.00 |
| 1% | 37.0 | 432 | 30 days | 0.2 days | 3.00 | 4.98% | 0.2 days | 5 | 24.6 | 0.00% | 0.20 | 0.99 | 5 | 432 | 0.00% | 0.05 | 1.00 |
| 1% | 37.0 | 432 | 60 days | 0.2 days | 3.00 | 4.98% | 0.2 days | 5 | 24.6 | 0.00% | 0.20 | 0.99 | 5 | 432 | 0.00% | 0.05 | 1.00 |

## Storage: what a node keeps, by payees, payout level and window cap (arithmetic, no fees)

| Payees in the window | Payout level | Coinbase outputs | In the payout tree | Coinbase KB | GB kept, 14-day cap (28 days) | GB kept, 30-day cap (60 days) | GB a year if never pruned | Payout tree MB a found block, kept for ever |
|---|---|---|---|---|---|---|---|---|
| 10 | 5 (17,472 sat) | 10 | 0 | 0.707 | 0.171 | 0.367 | 2.23 | 0 |
| 10 | 1 (1,092 sat) | 10 | 0 | 0.707 | 0.171 | 0.367 | 2.23 | 0 |
| 100 | 5 (17,472 sat) | 100 | 0 | 3.77 | 0.914 | 1.96 | 11.9 | 0 |
| 100 | 1 (1,092 sat) | 100 | 0 | 3.77 | 0.914 | 1.96 | 11.9 | 0 |
| 1,000 | 5 (17,472 sat) | 1,000 | 0 | 34.4 | 8.34 | 17.8 | 108 | 0 |
| 1,000 | 1 (1,092 sat) | 1,000 | 0 | 34.4 | 8.34 | 17.8 | 108 | 0 |
| 10,000 | 5 (17,472 sat) | 1,788 | 0 | 61.2 | 14.8 | 31.7 | 193 | 0 |
| 10,000 | 1 (1,092 sat) | 10,000 | 0 | 340 | 82.6 | 177 | 1,073 | 0 |
| 100,000 | 5 (17,472 sat) | 1,788 | 0 | 61.2 | 14.8 | 31.7 | 193 | 0 |
| 100,000 | 1 (1,092 sat) | 28,617 | 0 | 973 | 236 | 505 | 3,070 | 0 |
| 1,000,000 | 5 (17,472 sat) | 1,788 | 0 | 61.2 | 14.8 | 31.7 | 193 | 0 |
| 1,000,000 | 1 (1,092 sat) | 28,617 | 0 | 973 | 236 | 505 | 3,070 | 0 |

| Stored | GB kept: whole one payout depth, summary two, 14-day cap | GB kept: whole one payout depth, summary two, 30-day cap | GB kept with keep_whole_weak_shares: both two payout depths, 14-day cap | GB kept with keep_whole_weak_shares: both two payout depths, 30-day cap | GB a year if never pruned |
|---|---|---|---|---|---|
| 32 weak shares a share | 4.33 | 9.26 | 7.43 | 15.9 | 96.6 |
| 320 weak shares a share | 43.2 | 92.4 | 74.2 | 159 | 965 |
| Share headers, kept for ever | - | - | - | - | 3.15 |

## Proposal: weak shares that follow the asks, where they settle (band-weak-levels, weak work, 50% industrial)

| BCH hashrate | Pool | Rule | Weak a share | R | Asking | Weak/s | KB/s | Smallest miner with 3 weak shares a month (TH/s) | Bitaxe weak/mo | Bitaxe pay/mo | Bitaxe empty months | Bitaxe swing | Bitaxe vs fair | ASIC weak/mo | ASIC pay/mo | ASIC empty months | ASIC swing | ASIC vs fair |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| x1 | 10% | today | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 0.121 | 29.0 | 24.8 | 0.00% | 0.27 | 1.00 | 2,410 | 1,967 | 0.00% | 0.03 | 1.00 |
| x1 | 10% | asks | 46.3 | 46.4 | 0.00% | 4.63 | 3.81 | 0.090 | 39.0 | 24.8 | 0.00% | 0.26 | 1.00 | 3,244 | 2,013 | 0.00% | 0.03 | 1.00 |
| x1 | 10% | fill 320 | 346 | 339 | 0.00% | 34.6 | 27.8 | 0.012 | 291 | 24.8 | 0.00% | 0.21 | 1.00 | 24,175 | 2,067 | 0.00% | 0.02 | 1.00 |
| x1 | 30% | today | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 0.363 | 9.40 | 24.0 | 0.00% | 0.39 | 0.97 | 804 | 1,906 | 0.00% | 0.04 | 1.00 |
| x1 | 30% | asks | 56.8 | 56.6 | 6.00% | 5.68 | 4.65 | 0.222 | 15.7 | 24.4 | 0.00% | 0.32 | 0.99 | 1,327 | 2,026 | 0.00% | 0.04 | 1.00 |
| x1 | 30% | fill 320 | 346 | 339 | 0.00% | 34.6 | 27.8 | 0.037 | 96.6 | 25.0 | 0.00% | 0.22 | 1.01 | 8,053 | 2,066 | 0.00% | 0.02 | 1.00 |
| x10 | 10% | today | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 1.21 | 2.76 | 4.74 | 10.00% | 0.75 | 0.96 | 242 | 206 | 0.00% | 0.09 | 1.00 |
| x10 | 10% | asks | 189 | 186 | 26.62% | 18.9 | 15.3 | 0.225 | 15.7 | 4.89 | 1.60% | 0.53 | 0.99 | 1,325 | 206 | 0.00% | 0.07 | 1.00 |
| x10 | 10% | fill 320 | 346 | 339 | 0.00% | 34.6 | 27.8 | 0.124 | 29.0 | 4.97 | 1.30% | 0.49 | 1.00 | 2,417 | 207 | 0.00% | 0.07 | 1.00 |
| x10 | 30% | today | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 3.63 | 0.962 | 2.96 | 40.50% | 1.18 | 0.99 | 80.6 | 204 | 0.00% | 0.13 | 1.00 |
| x10 | 30% | asks | 344 | 337 | 49.99% | 34.4 | 27.6 | 0.373 | 9.38 | 4.84 | 2.80% | 0.56 | 0.98 | 802 | 206 | 0.00% | 0.08 | 1.00 |
| x10 | 30% | fill 320 | 346 | 339 | 0.00% | 34.6 | 27.8 | 0.371 | 9.42 | 4.78 | 2.60% | 0.59 | 0.96 | 806 | 207 | 0.00% | 0.08 | 1.00 |
| x100 | 10% | today | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 12.1 | 0.284 | 0.877 | 75.40% | 2.06 | 0.98 | 24.2 | 20.9 | 0.00% | 0.30 | 1.01 |
| x100 | 10% | asks | 344 | 338 | 49.99% | 34.4 | 27.6 | 1.24 | 2.75 | 3.77 | 14.00% | 0.83 | 0.96 | 241 | 20.5 | 0.00% | 0.24 | 0.99 |
| x100 | 10% | fill 320 | 346 | 339 | 0.00% | 34.6 | 27.8 | 1.24 | 2.76 | 3.71 | 12.30% | 0.78 | 0.94 | 242 | 20.8 | 0.00% | 0.24 | 1.01 |
| x100 | 30% | today | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 36.3 | 0.089 | 0.285 | 91.10% | 3.74 | 0.97 | 7.81 | 19.7 | 0.20% | 0.43 | 0.96 |
| x100 | 30% | asks | 344 | 338 | 50.00% | 34.4 | 27.6 | 3.72 | 0.959 | 2.88 | 41.20% | 1.24 | 1.00 | 80.5 | 20.6 | 0.00% | 0.25 | 1.00 |
| x100 | 30% | fill 320 | 346 | 339 | 0.00% | 34.6 | 27.8 | 3.71 | 0.964 | 2.95 | 39.10% | 1.15 | 1.02 | 80.6 | 20.6 | 0.00% | 0.24 | 1.00 |
| x270 | 10% | today | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 32.6 | 0.097 | 0.309 | 90.70% | 3.55 | 0.94 | 8.68 | 7.58 | 0.50% | 0.49 | 0.99 |
| x270 | 10% | asks | 344 | 338 | 49.99% | 34.4 | 27.6 | 3.35 | 1.06 | 1.40 | 45.80% | 1.33 | 0.96 | 89.2 | 7.66 | 0.20% | 0.38 | 1.00 |
| x270 | 10% | fill 320 | 346 | 339 | 0.00% | 34.6 | 27.8 | 3.34 | 1.06 | 1.47 | 44.90% | 1.32 | 1.01 | 89.5 | 7.67 | 0.00% | 0.38 | 1.00 |
| x270 | 30% | today | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 97.9 | 0.028 | 0.114 | 97.00% | 6.18 | 1.04 | 2.83 | 7.31 | 6.90% | 0.70 | 0.96 |
| x270 | 30% | asks | 344 | 337 | 50.00% | 34.4 | 27.6 | 10.1 | 0.351 | 1.03 | 71.80% | 2.08 | 0.98 | 29.7 | 7.65 | 0.00% | 0.41 | 1.00 |
| x270 | 30% | fill 320 | 346 | 339 | 0.00% | 34.6 | 27.8 | 10.0 | 0.353 | 1.11 | 70.80% | 2.03 | 1.04 | 29.8 | 7.59 | 0.00% | 0.41 | 0.99 |



## Proposal: the asks in every miner mix, at ten times today's hashrate (band-weak-levels, weak work)

| Mix | Pool | Rule | Weak a share | R | Asking | Weak/s | KB/s | Smallest miner with 3 weak shares a month (TH/s) | Bitaxe weak/mo | Bitaxe pay/mo | Bitaxe empty months | Bitaxe swing | Bitaxe vs fair | ASIC weak/mo | ASIC pay/mo | ASIC empty months | ASIC swing | ASIC vs fair |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| home | 10% | today | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 1.21 | 2.76 | 4.62 | 11.00% | 0.76 | 0.93 | 242 | 685 | 0.00% | 0.07 | 1.00 |
| home | 10% | asks | 189 | 186 | 52.82% | 18.9 | 15.2 | 0.225 | 15.7 | 4.82 | 1.10% | 0.52 | 0.97 | 1,326 | 2,606 | 0.00% | 0.04 | 1.00 |
| home | 30% | today | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 3.63 | 0.962 | 2.96 | 40.50% | 1.18 | 0.99 | 80.6 | 246 | 0.00% | 0.13 | 1.00 |
| home | 30% | asks | 345 | 338 | 100.00% | 34.5 | 27.7 | 0.372 | 9.40 | 4.86 | 1.90% | 0.56 | 0.98 | 804 | 2,206 | 0.00% | 0.04 | 1.00 |
| 50% industrial | 10% | today | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 1.21 | 2.76 | 4.74 | 10.00% | 0.75 | 0.96 | 242 | 206 | 0.00% | 0.09 | 1.00 |
| 50% industrial | 10% | asks | 189 | 186 | 26.62% | 18.9 | 15.3 | 0.225 | 15.7 | 4.89 | 1.60% | 0.53 | 0.99 | 1,325 | 206 | 0.00% | 0.07 | 1.00 |
| 50% industrial | 30% | today | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 3.63 | 0.962 | 2.96 | 40.50% | 1.18 | 0.99 | 80.6 | 204 | 0.00% | 0.13 | 1.00 |
| 50% industrial | 30% | asks | 344 | 337 | 49.99% | 34.4 | 27.6 | 0.373 | 9.38 | 4.84 | 2.80% | 0.56 | 0.98 | 802 | 206 | 0.00% | 0.08 | 1.00 |
| 90% industrial | 10% | today | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 1.21 | 2.76 | 4.65 | 9.90% | 0.76 | 0.94 | 242 | 207 | 0.00% | 0.09 | 1.00 |
| 90% industrial | 10% | asks | 189 | 186 | 5.34% | 18.9 | 15.2 | 0.225 | 15.7 | 4.84 | 1.20% | 0.52 | 0.98 | 1,325 | 206 | 0.00% | 0.08 | 0.99 |
| 90% industrial | 30% | today | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 3.63 | 0.962 | 2.96 | 40.50% | 1.18 | 0.99 | 80.6 | 204 | 0.00% | 0.13 | 1.00 |
| 90% industrial | 30% | asks | 334 | 327 | 10.00% | 33.4 | 26.8 | 0.385 | 9.11 | 4.86 | 1.90% | 0.57 | 0.98 | 778 | 206 | 0.00% | 0.08 | 1.00 |
| 99% industrial | 10% | today | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 1.21 | 2.76 | 4.64 | 9.60% | 0.76 | 0.95 | 242 | 206 | 0.00% | 0.09 | 1.00 |
| 99% industrial | 10% | asks | 81.7 | 81.1 | 1.00% | 8.17 | 6.64 | 0.517 | 6.74 | 4.76 | 3.20% | 0.61 | 0.98 | 573 | 207 | 0.00% | 0.08 | 1.00 |
| 99% industrial | 30% | today | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 3.63 | 0.962 | 2.96 | 40.50% | 1.18 | 0.99 | 80.6 | 205 | 0.00% | 0.13 | 1.00 |
| 99% industrial | 30% | asks | 81.5 | 80.7 | 1.00% | 8.15 | 6.62 | 1.56 | 2.20 | 4.74 | 14.00% | 0.80 | 0.97 | 190 | 207 | 0.00% | 0.10 | 1.00 |
| zipf | 10% | today | 34.5 | 37.2 | 0.00% | 3.45 | 2.86 | 1.11 | 3.00 | 4.68 | 8.40% | 0.72 | 0.96 | 263 | 207 | 0.00% | 0.09 | 1.00 |
| zipf | 10% | asks | 141 | 148 | 1.62% | 14.1 | 11.4 | 0.279 | 12.6 | 4.78 | 1.60% | 0.52 | 0.97 | 1,067 | 206 | 0.00% | 0.07 | 1.00 |
| zipf | 30% | today | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 3.63 | 0.962 | 2.96 | 40.50% | 1.18 | 0.99 | 80.6 | 204 | 0.00% | 0.13 | 1.00 |
| zipf | 30% | asks | 309 | 304 | 4.57% | 30.9 | 24.9 | 0.414 | 8.44 | 4.73 | 2.40% | 0.58 | 0.96 | 723 | 207 | 0.00% | 0.08 | 1.00 |
| medium | 10% | today | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 1.21 | 2.76 | 4.65 | 9.90% | 0.76 | 0.94 | 242 | 207 | 0.00% | 0.09 | 1.00 |
| medium | 10% | asks | 190 | 186 | 5.36% | 19.0 | 15.3 | 0.225 | 15.7 | 4.87 | 0.90% | 0.52 | 0.98 | 1,325 | 206 | 0.00% | 0.08 | 1.00 |
| medium | 30% | today | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 3.63 | 0.962 | 2.96 | 40.50% | 1.18 | 0.99 | 80.6 | 204 | 0.00% | 0.13 | 1.00 |
| medium | 30% | asks | 334 | 328 | 10.00% | 33.4 | 26.8 | 0.384 | 9.12 | 4.78 | 2.90% | 0.58 | 0.96 | 779 | 208 | 0.00% | 0.08 | 1.01 |
| medium, few big | 10% | today | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 1.21 | 2.76 | 4.65 | 9.90% | 0.76 | 0.94 | 242 | 207 | 0.00% | 0.09 | 1.00 |
| medium, few big | 10% | asks | 189 | 186 | 5.33% | 18.9 | 15.2 | 0.225 | 15.7 | 4.79 | 1.50% | 0.52 | 0.97 | 1,325 | 207 | 0.00% | 0.08 | 1.00 |
| medium, few big | 30% | today | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 3.63 | 0.962 | 2.96 | 40.50% | 1.18 | 0.99 | 80.6 | 204 | 0.00% | 0.13 | 1.00 |
| medium, few big | 30% | asks | 334 | 329 | 10.00% | 33.4 | 26.8 | 0.383 | 9.15 | 4.83 | 2.80% | 0.59 | 0.98 | 782 | 207 | 0.00% | 0.08 | 1.00 |



## Proposal: the asks' constants, each varied alone (pool 30% of ten times today's hashrate)

| Mix | WEAK_ASK_DIVISOR | Node default (weak shares a month) | WEAK_FILL_MAX | Weak a share | R | Asking | Weak/s | KB/s | Smallest miner with 3 weak shares a month (TH/s) | Bitaxe weak/mo | Bitaxe pay/mo | Bitaxe empty months | Bitaxe swing | Bitaxe vs fair | ASIC weak/mo | ASIC pay/mo | ASIC empty months | ASIC swing | ASIC vs fair |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 50% industrial | 16 | 16 | 320 | 344 | 337 | 49.99% | 34.4 | 27.6 | 0.373 | 9.38 | 4.84 | 2.80% | 0.56 | 0.98 | 802 | 206 | 0.00% | 0.08 | 1.00 |
| 50% industrial | 64 | 16 | 320 | 344 | 337 | 49.99% | 34.4 | 27.6 | 0.373 | 9.38 | 4.84 | 2.80% | 0.56 | 0.98 | 802 | 206 | 0.00% | 0.08 | 1.00 |
| 50% industrial | 256 | 16 | 320 | 344 | 337 | 49.99% | 34.4 | 27.6 | 0.373 | 9.38 | 4.84 | 2.80% | 0.56 | 0.98 | 802 | 206 | 0.00% | 0.08 | 1.00 |
| 50% industrial | 64 | 4 | 320 | 142 | 140 | 21.42% | 14.2 | 11.4 | 0.899 | 3.81 | 4.84 | 6.30% | 0.69 | 0.98 | 332 | 206 | 0.00% | 0.09 | 1.00 |
| 50% industrial | 64 | 16 | 320 | 344 | 337 | 49.99% | 34.4 | 27.6 | 0.373 | 9.38 | 4.84 | 2.80% | 0.56 | 0.98 | 802 | 206 | 0.00% | 0.08 | 1.00 |
| 50% industrial | 64 | 64 | 320 | 344 | 337 | 49.99% | 34.4 | 27.6 | 0.373 | 9.38 | 4.84 | 2.80% | 0.56 | 0.98 | 802 | 206 | 0.00% | 0.08 | 1.00 |
| 50% industrial | 64 | 16 | 64 | 69.3 | 68.8 | 50.00% | 6.93 | 5.65 | 1.83 | 1.86 | 4.65 | 17.30% | 0.86 | 0.94 | 162 | 207 | 0.00% | 0.11 | 1.00 |
| 50% industrial | 64 | 16 | 160 | 173 | 171 | 49.99% | 17.3 | 14.0 | 0.738 | 4.64 | 4.69 | 4.60% | 0.68 | 0.95 | 405 | 206 | 0.00% | 0.09 | 1.00 |
| 50% industrial | 64 | 16 | 320 | 344 | 337 | 49.99% | 34.4 | 27.6 | 0.373 | 9.38 | 4.84 | 2.80% | 0.56 | 0.98 | 802 | 206 | 0.00% | 0.08 | 1.00 |
| zipf | 16 | 16 | 320 | 209 | 205 | 6.87% | 20.9 | 16.8 | 0.613 | 5.63 | 4.74 | 3.30% | 0.62 | 0.97 | 488 | 207 | 0.00% | 0.08 | 1.00 |
| zipf | 64 | 16 | 320 | 309 | 304 | 4.57% | 30.9 | 24.9 | 0.414 | 8.44 | 4.73 | 2.40% | 0.58 | 0.96 | 723 | 207 | 0.00% | 0.08 | 1.00 |
| zipf | 256 | 16 | 320 | 316 | 311 | 4.30% | 31.6 | 25.4 | 0.405 | 8.62 | 4.63 | 2.60% | 0.55 | 0.94 | 739 | 207 | 0.00% | 0.08 | 1.00 |
| zipf | 64 | 4 | 320 | 124 | 122 | 1.33% | 12.4 | 10.0 | 1.03 | 3.33 | 10.0 | 5.10% | 0.64 | 0.95 | 290 | 206 | 0.00% | 0.09 | 0.99 |
| zipf | 64 | 16 | 320 | 309 | 304 | 4.57% | 30.9 | 24.9 | 0.414 | 8.44 | 4.73 | 2.40% | 0.58 | 0.96 | 723 | 207 | 0.00% | 0.08 | 1.00 |
| zipf | 64 | 64 | 320 | 338 | 331 | 13.67% | 33.8 | 27.1 | 0.380 | 9.21 | 4.94 | 2.60% | 0.56 | 1.00 | 788 | 208 | 0.00% | 0.08 | 1.00 |
| zipf | 64 | 16 | 64 | 69.3 | 69.2 | 14.78% | 6.93 | 5.65 | 1.82 | 1.87 | 4.66 | 18.60% | 0.84 | 0.95 | 163 | 206 | 0.00% | 0.10 | 1.00 |
| zipf | 64 | 16 | 160 | 172 | 170 | 8.36% | 17.2 | 13.9 | 0.741 | 4.63 | 14.0 | 1.50% | 0.53 | 0.96 | 403 | 207 | 0.00% | 0.08 | 1.00 |
| zipf | 64 | 16 | 320 | 309 | 304 | 4.57% | 30.9 | 24.9 | 0.414 | 8.44 | 4.73 | 2.40% | 0.58 | 0.96 | 723 | 207 | 0.00% | 0.08 | 1.00 |



## Proposal: attacks on the asks (band-weak-levels, weak work, 50% industrial)

| Attack | Its part | BCH hashrate, pool | Weak a share | R | Asking | Weak/s | KB/s | Smallest miner with 3 weak shares a month (TH/s) | Bitaxe weak/mo | Bitaxe pay/mo | Bitaxe empty months | Bitaxe swing | Bitaxe vs fair | ASIC weak/mo | ASIC pay/mo | ASIC empty months | ASIC swing | ASIC vs fair |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| asks whatever it finds | 0.00% | x1, 10% | 46.3 | 46.4 | 0.00% | 4.63 | 3.81 | 0.090 | 39.0 | 24.8 | 0.00% | 0.26 | 1.00 | 3,244 | 2,013 | 0.00% | 0.03 | 1.00 |
| asks whatever it finds | 1.00% | x1, 10% | 82.1 | 81.3 | 1.00% | 8.21 | 6.67 | 0.052 | 69.1 | 24.7 | 0.00% | 0.23 | 1.00 | 5,742 | 2,050 | 0.00% | 0.03 | 1.00 |
| asks whatever it finds | 2.00% | x1, 10% | 222 | 220 | 2.00% | 22.2 | 17.9 | 0.019 | 188 | 25.0 | 0.00% | 0.21 | 1.01 | 15,647 | 2,068 | 0.00% | 0.02 | 1.00 |
| asks whatever it finds | 5.00% | x1, 10% | 315 | 310 | 5.00% | 31.5 | 25.3 | 0.014 | 265 | 24.9 | 0.00% | 0.20 | 1.00 | 22,077 | 2,069 | 0.00% | 0.02 | 1.00 |
| asks whatever it finds | 25.00% | x1, 10% | 342 | 336 | 24.99% | 34.2 | 27.4 | 0.012 | 288 | 24.7 | 0.00% | 0.21 | 0.99 | 23,961 | 2,066 | 0.00% | 0.02 | 1.00 |
| none, today's rule | - | x10, 30% | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 3.63 | 0.962 | 2.96 | 40.50% | 1.18 | 0.99 | 80.6 | 204 | 0.00% | 0.13 | 1.00 |
| none | - | x10, 30% | 344 | 337 | 49.99% | 34.4 | 27.6 | 0.373 | 9.38 | 4.84 | 2.80% | 0.56 | 0.98 | 802 | 206 | 0.00% | 0.08 | 1.00 |
| never asks | 50.00% | x10, 30% | 341 | 334 | 25.00% | 34.1 | 27.4 | 0.377 | 9.29 | 4.88 | 2.40% | 0.55 | 0.98 | 795 | 207 | 0.00% | 0.08 | 1.00 |
| never asks | 90.00% | x10, 30% | 316 | 311 | 5.00% | 31.6 | 25.4 | 0.404 | 8.64 | 4.73 | 3.00% | 0.57 | 0.95 | 740 | 207 | 0.00% | 0.08 | 1.00 |
| never asks | 97.00% | x10, 30% | 143 | 140 | 1.50% | 14.3 | 11.5 | 0.895 | 3.83 | 4.71 | 6.00% | 0.70 | 0.95 | 333 | 207 | 0.00% | 0.09 | 1.00 |
| never asks | 99.00% | x10, 30% | 57.3 | 57.0 | 0.50% | 5.73 | 4.69 | 2.20 | 1.56 | 4.73 | 21.90% | 0.92 | 0.96 | 134 | 206 | 0.00% | 0.11 | 1.00 |
| industrial miners at the minimum | today | x10, 30% | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 3.63 | 0.962 | 2.96 | 40.50% | 1.18 | 0.99 | 80.6 | 204 | 0.00% | 0.13 | 1.00 |
| industrial miners at the minimum | asks | x10, 30% | 344 | 337 | 49.99% | 34.4 | 27.6 | 0.373 | 9.38 | 4.84 | 2.80% | 0.56 | 0.98 | 802 | 206 | 0.00% | 0.08 | 1.00 |
| carries no asking weak share | 25.00% | x10, 30% | 250 | 246 | 50.00% | 25.0 | 20.1 | 0.511 | 6.87 | 4.88 | 2.90% | 0.59 | 0.98 | 585 | 207 | 0.00% | 0.08 | 1.00 |
| carries no asking weak share | 50.00% | x10, 30% | 168 | 165 | 50.00% | 16.8 | 13.5 | 0.762 | 4.50 | 4.69 | 6.10% | 0.65 | 0.95 | 392 | 207 | 0.00% | 0.09 | 1.00 |
| carries no asking weak share | 75.00% | x10, 30% | 107 | 106 | 50.01% | 10.7 | 8.66 | 1.19 | 2.87 | 4.73 | 11.50% | 0.74 | 0.95 | 251 | 207 | 0.00% | 0.10 | 1.00 |
| carries no asking weak share | 90.00% | x10, 30% | 83.1 | 83.1 | 49.42% | 8.41 | 6.83 | 1.51 | 2.23 | 4.74 | 14.60% | 0.80 | 0.96 | 194 | 206 | 0.00% | 0.10 | 1.00 |



## Study: the weak ratio's path from genesis, by rule (band-weak-levels, weak work, 50% industrial)

| BCH hashrate, pool | Rule | Peak weak a share (hourly) | Largest share | Shares above 64, first 6 h | Shares above 320, first 6 h | Count stays within 25% | Weak expired, first 6 h | Weak a share, last day | Weak expired, last day |
|---|---|---|---|---|---|---|---|---|---|
| x1, 10% | today | 64.0 | 64.0 | 0 | 0 | 2 h | 20.95% | 34.4 | 0.00% |
| x1, 10% | v0 as written | 162 | 640 | 819 | 67.0 | 3 h | 0.00% | 46.3 | 0.00% |
| x1, 10% | v1 down to R/2 | 14.9 | 640 | 27.6 | 0.625 | < 1 h | 0.00% | 13.8 | 0.00% |
| x1, 10% | v1 down to R/4 | 14.2 | 640 | 28.2 | 0.750 | < 1 h | 0.00% | 12.8 | 0.00% |
| x1, 10% | v2 cap follows asks | 63.9 | 64.0 | 0 | 0 | 2 h | 19.73% | 34.5 | 0.00% |
| x1, 10% | v3 genesis 32 | 47.8 | 596 | 495 | 1.50 | < 1 h | 0.00% | 46.3 | 0.00% |
| x1, 10% | v4 = v2 + v3 | 35.9 | 64.0 | 0 | 0 | < 1 h | 0.00% | 34.5 | 0.00% |
| x1, 10% | v5 = v4, one ask opens | 35.3 | 64.0 | 0 | 0 | < 1 h | 0.00% | 34.5 | 0.00% |
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
| x1, 10% | joins | today | 34.6 | 35.7 | 64.0 | 0 | 0 | < 1 h | 0.00% | 34.5 | 0.00% | 1.01 |
| x1, 10% | joins | v0 as written | 47.2 | 47.7 | 621 | 564 | 2.00 | < 1 h | 0.00% | 46.4 | 0.00% | 1.02 |
| x1, 10% | joins | v1 down to R/2 | 13.8 | 27.4 | 319 | 174 | 0 | < 1 h | 0.00% | 24.8 | 0.00% | 1.01 |
| x1, 10% | joins | v1 down to R/4 | 12.9 | 25.3 | 332 | 147 | 0.125 | < 1 h | 0.00% | 21.8 | 0.00% | 1.02 |
| x1, 10% | joins | v2 cap follows asks | 34.5 | 39.1 | 378 | 49.0 | 0 | < 1 h | 0.00% | 38.3 | 0.00% | 1.02 |
| x1, 10% | joins | v3 genesis 32 | 45.9 | 48.1 | 558 | 541 | 1.88 | < 1 h | 0.00% | 46.0 | 0.00% | 1.02 |
| x1, 10% | joins | v4 = v2 + v3 | 34.5 | 39.0 | 369 | 47.0 | 0.125 | < 1 h | 0.00% | 38.3 | 0.00% | 1.01 |
| x1, 10% | joins | v5 = v4, one ask opens | 34.7 | 39.2 | 346 | 50.1 | 0 | < 1 h | 0.00% | 38.2 | 0.00% | 1.01 |
| x1, 10% | leaves | today | 34.7 | 35.3 | 64.0 | 0 | 0 | < 1 h | 0.00% | 34.5 | 0.00% | 0.99 |
| x1, 10% | leaves | v0 as written | 46.3 | 48.3 | 619 | 534 | 2.12 | < 1 h | 0.00% | 46.5 | 0.00% | 0.99 |
| x1, 10% | leaves | v1 down to R/2 | 25.2 | 15.0 | 212 | 31.6 | 0 | < 1 h | 0.00% | 13.8 | 0.00% | 0.99 |
| x1, 10% | leaves | v1 down to R/4 | 22.3 | 15.2 | 171 | 24.2 | 0 | < 1 h | 0.00% | 13.0 | 0.00% | 0.99 |
| x1, 10% | leaves | v2 cap follows asks | 38.3 | 36.0 | 161 | 0.125 | 0 | < 1 h | 0.00% | 34.5 | 0.00% | 0.99 |
| x1, 10% | leaves | v3 genesis 32 | 45.7 | 49.1 | 637 | 525 | 2.88 | < 1 h | 0.00% | 46.4 | 0.00% | 0.99 |
| x1, 10% | leaves | v4 = v2 + v3 | 38.3 | 36.3 | 64.0 | 0 | 0 | < 1 h | 0.00% | 34.6 | 0.00% | 0.99 |
| x1, 10% | leaves | v5 = v4, one ask opens | 38.3 | 36.7 | 64.0 | 0 | 0 | < 1 h | 0.00% | 34.5 | 0.00% | 0.99 |
| x10, 10% | joins | today | 34.4 | 35.9 | 64.0 | 0 | 0 | < 1 h | 0.00% | 34.6 | 0.00% | 1.01 |
| x10, 10% | joins | v0 as written | 190 | 350 | 640 | 1,898 | 1,017 | < 1 h | 0.00% | 342 | 0.00% | 1.02 |
| x10, 10% | joins | v1 down to R/2 | 53.6 | 58.9 | 640 | 642 | 12.4 | < 1 h | 0.00% | 53.7 | 0.00% | 1.01 |
| x10, 10% | joins | v1 down to R/4 | 38.6 | 48.9 | 640 | 401 | 6.25 | < 1 h | 0.00% | 41.2 | 0.00% | 1.02 |
| x10, 10% | joins | v2 cap follows asks | 175 | 345 | 640 | 1,851 | 974 | 1 h | 0.05% | 336 | 0.02% | 1.01 |
| x10, 10% | joins | v3 genesis 32 | 190 | 350 | 640 | 1,887 | 1,013 | < 1 h | 0.00% | 341 | 0.00% | 1.01 |
| x10, 10% | joins | v4 = v2 + v3 | 175 | 345 | 640 | 1,860 | 976 | 1 h | 0.09% | 335 | 0.14% | 1.02 |
| x10, 10% | joins | v5 = v4, one ask opens | 177 | 350 | 640 | 1,868 | 998 | < 1 h | 0.00% | 336 | 0.07% | 1.01 |
| x10, 10% | leaves | today | 34.3 | 35.8 | 64.0 | 0 | 0 | < 1 h | 0.00% | 34.5 | 0.00% | 0.99 |
| x10, 10% | leaves | v0 as written | 343 | 241 | 640 | 1,529 | 447 | 1 h | 0.00% | 190 | 0.00% | 0.99 |
| x10, 10% | leaves | v1 down to R/2 | 53.8 | 61.1 | 640 | 613 | 11.4 | < 1 h | 0.00% | 53.4 | 0.00% | 0.99 |
| x10, 10% | leaves | v1 down to R/4 | 39.1 | 47.1 | 640 | 380 | 5.62 | < 1 h | 0.00% | 40.2 | 0.00% | 0.99 |
| x10, 10% | leaves | v2 cap follows asks | 334 | 181 | 640 | 1,006 | 381 | < 1 h | 5.74% | 175 | 1.69% | 0.98 |
| x10, 10% | leaves | v3 genesis 32 | 339 | 244 | 640 | 1,523 | 449 | 1 h | 0.00% | 190 | 0.00% | 0.99 |
| x10, 10% | leaves | v4 = v2 + v3 | 337 | 184 | 640 | 991 | 365 | < 1 h | 7.14% | 175 | 1.53% | 0.98 |
| x10, 10% | leaves | v5 = v4, one ask opens | 338 | 185 | 640 | 987 | 367 | < 1 h | 7.50% | 175 | 1.64% | 0.98 |
| x10, 30% | joins | today | 34.7 | 35.9 | 64.0 | 0 | 0 | < 1 h | 0.00% | 34.5 | 0.00% | 1.01 |
| x10, 30% | joins | v0 as written | 345 | 353 | 640 | 1,916 | 1,087 | < 1 h | 0.00% | 340 | 0.00% | 1.01 |
| x10, 30% | joins | v1 down to R/2 | 54.0 | 59.3 | 640 | 639 | 12.5 | < 1 h | 0.00% | 53.8 | 0.00% | 1.01 |
| x10, 30% | joins | v1 down to R/4 | 42.3 | 44.2 | 640 | 410 | 6.38 | < 1 h | 0.00% | 40.0 | 0.00% | 1.01 |
| x10, 30% | joins | v2 cap follows asks | 344 | 350 | 640 | 1,924 | 1,084 | < 1 h | 0.00% | 341 | 0.00% | 1.01 |
| x10, 30% | joins | v3 genesis 32 | 344 | 352 | 640 | 1,927 | 1,089 | < 1 h | 0.00% | 342 | 0.00% | 1.01 |
| x10, 30% | joins | v4 = v2 + v3 | 343 | 360 | 640 | 1,925 | 1,091 | < 1 h | 0.00% | 342 | 0.00% | 1.01 |
| x10, 30% | joins | v5 = v4, one ask opens | 344 | 351 | 640 | 1,925 | 1,088 | < 1 h | 0.00% | 340 | 0.00% | 1.01 |
| x10, 30% | leaves | today | 34.5 | 35.6 | 64.0 | 0 | 0 | < 1 h | 0.00% | 34.5 | 0.00% | 0.99 |
| x10, 30% | leaves | v0 as written | 342 | 351 | 640 | 1,815 | 1,035 | < 1 h | 0.00% | 343 | 0.00% | 0.99 |
| x10, 30% | leaves | v1 down to R/2 | 55.5 | 59.5 | 640 | 612 | 11.6 | < 1 h | 0.00% | 53.7 | 0.00% | 0.99 |
| x10, 30% | leaves | v1 down to R/4 | 38.9 | 48.2 | 640 | 407 | 5.75 | < 1 h | 0.00% | 40.8 | 0.00% | 0.99 |
| x10, 30% | leaves | v2 cap follows asks | 343 | 354 | 640 | 1,825 | 1,040 | < 1 h | 0.00% | 343 | 0.00% | 0.99 |
| x10, 30% | leaves | v3 genesis 32 | 341 | 357 | 640 | 1,818 | 1,038 | < 1 h | 0.00% | 344 | 0.00% | 0.99 |
| x10, 30% | leaves | v4 = v2 + v3 | 342 | 357 | 640 | 1,810 | 1,041 | < 1 h | 0.00% | 342 | 0.00% | 0.99 |
| x10, 30% | leaves | v5 = v4, one ask opens | 342 | 353 | 640 | 1,818 | 1,038 | < 1 h | 0.00% | 345 | 0.00% | 0.99 |
| test network, 10 GH/s | joins | today | 4.13 | 50.7 | 64.0 | 0 | 0 | 1 h | 9.49% | 34.5 | 0.00% | 2.33 |
| test network, 10 GH/s | joins | v0 as written | 4.10 | 95.4 | 640 | 859 | 52.5 | 2 h | 0.00% | 46.4 | 0.00% | 1.89 |
| test network, 10 GH/s | joins | v1 down to R/2 | 4.10 | 8.09 | 134 | 2.75 | 0 | < 1 h | 0.00% | 7.15 | 0.00% | 1.88 |
| test network, 10 GH/s | joins | v1 down to R/4 | 4.08 | 7.84 | 192 | 1.50 | 0 | < 1 h | 0.00% | 7.02 | 0.00% | 1.87 |
| test network, 10 GH/s | joins | v2 cap follows asks | 4.10 | 49.8 | 64.0 | 0 | 0 | 1 h | 11.37% | 34.5 | 0.00% | 2.40 |
| test network, 10 GH/s | joins | v3 genesis 32 | 4.06 | 94.2 | 640 | 865 | 52.4 | 2 h | 0.00% | 46.5 | 0.00% | 1.89 |
| test network, 10 GH/s | joins | v4 = v2 + v3 | 4.09 | 49.6 | 64.0 | 0 | 0 | 1 h | 10.22% | 34.5 | 0.00% | 2.37 |
| test network, 10 GH/s | joins | v5 = v4, one ask opens | 4.12 | 50.1 | 64.0 | 0 | 0 | 1 h | 8.90% | 34.5 | 0.00% | 2.34 |
| test network, 10 GH/s | leaves | today | 34.6 | 23.3 | 64.0 | 0 | 0 | 2 h | 0.00% | 4.10 | 0.00% | 0.60 |
| test network, 10 GH/s | leaves | v0 as written | 45.4 | 25.9 | 250 | 2.62 | 0 | 2 h | 0.00% | 4.09 | 0.00% | 0.60 |
| test network, 10 GH/s | leaves | v1 down to R/2 | 7.21 | 7.00 | 58.0 | 0 | 0 | 2 h | 0.00% | 4.09 | 0.00% | 0.60 |
| test network, 10 GH/s | leaves | v1 down to R/4 | 7.07 | 6.76 | 75.0 | 0.125 | 0 | 1 h | 0.00% | 4.09 | 0.00% | 0.60 |
| test network, 10 GH/s | leaves | v2 cap follows asks | 34.4 | 21.3 | 64.0 | 0 | 0 | 2 h | 0.00% | 4.10 | 0.00% | 0.60 |
| test network, 10 GH/s | leaves | v3 genesis 32 | 46.8 | 25.5 | 252 | 3.75 | 0 | 2 h | 0.00% | 4.09 | 0.00% | 0.60 |
| test network, 10 GH/s | leaves | v4 = v2 + v3 | 34.4 | 23.1 | 64.0 | 0 | 0 | 2 h | 0.00% | 4.10 | 0.00% | 0.60 |
| test network, 10 GH/s | leaves | v5 = v4, one ask opens | 34.4 | 25.3 | 64.0 | 0 | 0 | 2 h | 0.00% | 4.09 | 0.00% | 0.60 |
| test network, 10 GH/s, 1% always asks | joins | today | 4.11 | 50.2 | 64.0 | 0 | 0 | 1 h | 9.65% | 34.5 | 0.00% | 2.35 |
| test network, 10 GH/s, 1% always asks | joins | v0 as written | 4.12 | 116 | 640 | 1,253 | 127 | 2 h | 0.00% | 82.4 | 0.00% | 1.86 |
| test network, 10 GH/s, 1% always asks | joins | v1 down to R/2 | 4.11 | 8.67 | 130 | 4.00 | 0 | < 1 h | 0.00% | 7.90 | 0.00% | 1.90 |
| test network, 10 GH/s, 1% always asks | joins | v1 down to R/4 | 4.08 | 8.58 | 121 | 5.00 | 0 | < 1 h | 0.00% | 7.83 | 0.00% | 1.90 |
| test network, 10 GH/s, 1% always asks | joins | v2 cap follows asks | 4.08 | 66.1 | 612 | 270 | 5.50 | 1 h | 4.96% | 49.6 | 0.00% | 2.24 |
| test network, 10 GH/s, 1% always asks | joins | v3 genesis 32 | 4.11 | 112 | 640 | 1,255 | 120 | 2 h | 0.00% | 82.0 | 0.00% | 1.86 |
| test network, 10 GH/s, 1% always asks | joins | v4 = v2 + v3 | 4.14 | 65.5 | 639 | 262 | 5.00 | 1 h | 4.78% | 49.6 | 0.00% | 2.20 |
| test network, 10 GH/s, 1% always asks | joins | v5 = v4, one ask opens | 4.10 | 87.3 | 640 | 729 | 70.2 | 2 h | 0.83% | 66.5 | 0.00% | 2.00 |
| test network, 10 GH/s, 1% always asks | leaves | today | 34.7 | 22.9 | 64.0 | 0 | 0 | 2 h | 0.00% | 4.09 | 0.00% | 0.60 |
| test network, 10 GH/s, 1% always asks | leaves | v0 as written | 80.7 | 31.8 | 427 | 4.38 | 0.375 | 2 h | 0.00% | 4.10 | 0.00% | 0.60 |
| test network, 10 GH/s, 1% always asks | leaves | v1 down to R/2 | 7.81 | 7.61 | 64.0 | 0 | 0 | 1 h | 0.00% | 4.10 | 0.00% | 0.60 |
| test network, 10 GH/s, 1% always asks | leaves | v1 down to R/4 | 7.56 | 6.67 | 77.0 | 0.125 | 0 | 1 h | 0.00% | 4.11 | 0.00% | 0.60 |
| test network, 10 GH/s, 1% always asks | leaves | v2 cap follows asks | 49.4 | 25.8 | 127 | 0.500 | 0 | 2 h | 0.00% | 4.10 | 0.00% | 0.60 |
| test network, 10 GH/s, 1% always asks | leaves | v3 genesis 32 | 83.8 | 29.8 | 320 | 4.62 | 0 | 2 h | 0.00% | 4.10 | 0.00% | 0.60 |
| test network, 10 GH/s, 1% always asks | leaves | v4 = v2 + v3 | 49.0 | 25.4 | 127 | 0.500 | 0 | 2 h | 0.00% | 4.10 | 0.00% | 0.60 |
| test network, 10 GH/s, 1% always asks | leaves | v5 = v4, one ask opens | 67.1 | 33.5 | 294 | 1.88 | 0 | 2 h | 0.00% | 4.11 | 0.00% | 0.60 |



## Study: where each rule settles (band-weak-levels, weak work, 50% industrial)

| BCH hashrate, pool | Rule | Weak expired | Weak a share | R | Asking | Weak/s | KB/s | Smallest miner with 3 weak shares a month (TH/s) | Bitaxe weak/mo | Bitaxe pay/mo | Bitaxe empty months | Bitaxe swing | Bitaxe vs fair | ASIC weak/mo | ASIC pay/mo | ASIC empty months | ASIC swing | ASIC vs fair |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| x1, 10% | today | 0.00% | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 0.121 | 29.0 | 24.8 | 0.00% | 0.27 | 1.00 | 2,410 | 1,967 | 0.00% | 0.03 | 1.00 |
| x1, 10% | v0 as written | 0.00% | 46.3 | 46.4 | 0.00% | 4.63 | 3.81 | 0.090 | 39.0 | 24.8 | 0.00% | 0.26 | 1.00 | 3,244 | 2,013 | 0.00% | 0.03 | 1.00 |
| x1, 10% | v1 down to R/2 | 0.00% | 13.7 | 14.4 | 46.07% | 1.37 | 1.20 | 0.290 | 11.4 | 24.5 | 0.00% | 0.34 | 0.99 | 961 | 1,705 | 0.00% | 0.04 | 1.00 |
| x1, 10% | v1 down to R/4 | 0.00% | 12.8 | 13.6 | 46.32% | 1.28 | 1.13 | 0.309 | 10.7 | 24.5 | 0.00% | 0.37 | 0.99 | 899 | 1,686 | 0.00% | 0.04 | 1.00 |
| x1, 10% | v2 cap follows asks | 0.00% | 34.5 | 34.8 | 0.00% | 3.45 | 2.86 | 0.120 | 29.1 | 24.9 | 0.00% | 0.27 | 1.00 | 2,421 | 1,967 | 0.00% | 0.03 | 1.00 |
| x1, 10% | v3 genesis 32 | 0.00% | 46.3 | 46.4 | 0.00% | 4.63 | 3.81 | 0.090 | 39.0 | 24.8 | 0.00% | 0.25 | 1.00 | 3,244 | 2,010 | 0.00% | 0.03 | 1.00 |
| x1, 10% | v4 = v2 + v3 | 0.00% | 34.5 | 34.9 | 0.00% | 3.45 | 2.86 | 0.120 | 29.1 | 24.7 | 0.00% | 0.28 | 0.99 | 2,422 | 1,969 | 0.00% | 0.03 | 1.00 |
| x1, 10% | v5 = v4, one ask opens | 0.00% | 34.5 | 34.9 | 0.00% | 3.45 | 2.86 | 0.120 | 29.1 | 24.7 | 0.00% | 0.28 | 0.99 | 2,422 | 1,969 | 0.00% | 0.03 | 1.00 |
| x10, 10% | today | 0.00% | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 1.21 | 2.76 | 4.74 | 10.00% | 0.75 | 0.96 | 242 | 206 | 0.00% | 0.09 | 1.00 |
| x10, 10% | v0 as written | 0.00% | 189 | 186 | 26.62% | 18.9 | 15.3 | 0.225 | 15.7 | 4.89 | 1.60% | 0.53 | 0.99 | 1,325 | 206 | 0.00% | 0.07 | 1.00 |
| x10, 10% | v1 down to R/2 | 0.00% | 54.1 | 53.9 | 50.01% | 5.41 | 4.43 | 0.778 | 4.35 | 4.60 | 6.00% | 0.66 | 0.93 | 379 | 207 | 0.00% | 0.09 | 1.00 |
| x10, 10% | v1 down to R/4 | 0.00% | 38.9 | 39.1 | 50.01% | 3.89 | 3.21 | 1.07 | 3.13 | 4.63 | 8.10% | 0.72 | 0.93 | 273 | 206 | 0.00% | 0.10 | 1.00 |
| x10, 10% | v2 cap follows asks | 0.00% | 187 | 184 | 38.32% | 18.7 | 15.1 | 0.228 | 15.5 | 4.91 | 1.40% | 0.50 | 0.99 | 1,310 | 206 | 0.00% | 0.07 | 1.00 |
| x10, 10% | v3 genesis 32 | 0.00% | 190 | 186 | 26.61% | 19.0 | 15.3 | 0.225 | 15.7 | 4.82 | 2.40% | 0.55 | 0.97 | 1,325 | 206 | 0.00% | 0.07 | 1.00 |
| x10, 10% | v4 = v2 + v3 | 0.00% | 187 | 184 | 38.38% | 18.7 | 15.1 | 0.228 | 15.5 | 4.89 | 1.50% | 0.54 | 0.99 | 1,309 | 207 | 0.00% | 0.08 | 1.00 |
| x10, 10% | v5 = v4, one ask opens | 0.00% | 187 | 184 | 38.24% | 18.7 | 15.1 | 0.228 | 15.5 | 4.90 | 1.90% | 0.51 | 0.99 | 1,310 | 207 | 0.00% | 0.08 | 1.00 |
| x10, 30% | today | 0.00% | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 3.63 | 0.962 | 2.96 | 40.50% | 1.18 | 0.99 | 80.6 | 204 | 0.00% | 0.13 | 1.00 |
| x10, 30% | v0 as written | 0.00% | 344 | 337 | 49.99% | 34.4 | 27.6 | 0.373 | 9.38 | 4.84 | 2.80% | 0.56 | 0.98 | 802 | 206 | 0.00% | 0.08 | 1.00 |
| x10, 30% | v1 down to R/2 | 0.00% | 53.9 | 53.6 | 50.02% | 5.39 | 4.41 | 2.35 | 1.47 | 4.42 | 26.00% | 0.97 | 0.96 | 126 | 206 | 0.00% | 0.12 | 1.00 |
| x10, 30% | v1 down to R/4 | 0.00% | 39.9 | 40.0 | 50.01% | 3.99 | 3.30 | 3.15 | 1.11 | 3.37 | 35.00% | 1.11 | 0.98 | 93.1 | 204 | 0.00% | 0.12 | 1.00 |
| x10, 30% | v2 cap follows asks | 0.00% | 344 | 337 | 49.99% | 34.4 | 27.6 | 0.373 | 9.38 | 4.84 | 2.80% | 0.56 | 0.98 | 802 | 206 | 0.00% | 0.08 | 1.00 |
| x10, 30% | v3 genesis 32 | 0.00% | 343 | 337 | 50.00% | 34.3 | 27.6 | 0.373 | 9.37 | 4.81 | 2.10% | 0.56 | 0.97 | 801 | 206 | 0.00% | 0.08 | 1.00 |
| x10, 30% | v4 = v2 + v3 | 0.00% | 343 | 337 | 50.00% | 34.3 | 27.6 | 0.373 | 9.37 | 4.81 | 2.10% | 0.56 | 0.97 | 801 | 206 | 0.00% | 0.08 | 1.00 |
| x10, 30% | v5 = v4, one ask opens | 0.00% | 343 | 337 | 50.00% | 34.3 | 27.6 | 0.373 | 9.37 | 4.81 | 2.10% | 0.56 | 0.97 | 801 | 206 | 0.00% | 0.08 | 1.00 |



## Study: the rules that bound the start in every miner mix, at ten times today's hashrate (band-weak-levels, weak work)

| Mix | Pool | Rule | Weak expired | Weak a share | R | Asking | Weak/s | KB/s | Smallest miner with 3 weak shares a month (TH/s) | Bitaxe weak/mo | Bitaxe pay/mo | Bitaxe empty months | Bitaxe swing | Bitaxe vs fair | ASIC weak/mo | ASIC pay/mo | ASIC empty months | ASIC swing | ASIC vs fair |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| home | 10% | today | 0.00% | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 1.21 | 2.76 | 4.62 | 11.00% | 0.76 | 0.93 | 242 | 685 | 0.00% | 0.07 | 1.00 |
| home | 10% | v0 as written | 0.00% | 189 | 186 | 52.82% | 18.9 | 15.2 | 0.225 | 15.7 | 4.82 | 1.10% | 0.52 | 0.97 | 1,326 | 2,606 | 0.00% | 0.04 | 1.00 |
| home | 10% | v4 = v2 + v3 | 0.00% | 187 | 184 | 76.39% | 18.7 | 15.0 | 0.228 | 15.5 | 4.96 | 2.10% | 0.52 | 1.00 | 1,310 | 2,590 | 0.00% | 0.04 | 1.00 |
| home | 10% | v5 = v4, one ask opens | 0.00% | 187 | 184 | 76.59% | 18.7 | 15.1 | 0.228 | 15.5 | 5.00 | 1.20% | 0.51 | 1.01 | 1,310 | 2,587 | 0.00% | 0.04 | 1.00 |
| home | 30% | today | 0.00% | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 3.63 | 0.962 | 2.96 | 40.50% | 1.18 | 0.99 | 80.6 | 246 | 0.00% | 0.13 | 1.00 |
| home | 30% | v0 as written | 0.00% | 345 | 338 | 100.00% | 34.5 | 27.7 | 0.372 | 9.40 | 4.86 | 1.90% | 0.56 | 0.98 | 804 | 2,206 | 0.00% | 0.04 | 1.00 |
| home | 30% | v4 = v2 + v3 | 0.00% | 345 | 339 | 100.00% | 34.5 | 27.7 | 0.371 | 9.42 | 4.80 | 1.50% | 0.57 | 0.97 | 806 | 2,210 | 0.00% | 0.04 | 1.00 |
| home | 30% | v5 = v4, one ask opens | 0.00% | 345 | 339 | 100.00% | 34.5 | 27.7 | 0.371 | 9.42 | 4.80 | 1.50% | 0.57 | 0.97 | 806 | 2,210 | 0.00% | 0.04 | 1.00 |
| 50% industrial | 10% | today | 0.00% | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 1.21 | 2.76 | 4.74 | 10.00% | 0.75 | 0.96 | 242 | 206 | 0.00% | 0.09 | 1.00 |
| 50% industrial | 10% | v0 as written | 0.00% | 189 | 186 | 26.62% | 18.9 | 15.3 | 0.225 | 15.7 | 4.89 | 1.60% | 0.53 | 0.99 | 1,325 | 206 | 0.00% | 0.07 | 1.00 |
| 50% industrial | 10% | v4 = v2 + v3 | 0.00% | 187 | 184 | 38.38% | 18.7 | 15.1 | 0.228 | 15.5 | 4.89 | 1.50% | 0.54 | 0.99 | 1,309 | 207 | 0.00% | 0.08 | 1.00 |
| 50% industrial | 10% | v5 = v4, one ask opens | 0.00% | 187 | 184 | 38.24% | 18.7 | 15.1 | 0.228 | 15.5 | 4.90 | 1.90% | 0.51 | 0.99 | 1,310 | 207 | 0.00% | 0.08 | 1.00 |
| 50% industrial | 30% | today | 0.00% | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 3.63 | 0.962 | 2.96 | 40.50% | 1.18 | 0.99 | 80.6 | 204 | 0.00% | 0.13 | 1.00 |
| 50% industrial | 30% | v0 as written | 0.00% | 344 | 337 | 49.99% | 34.4 | 27.6 | 0.373 | 9.38 | 4.84 | 2.80% | 0.56 | 0.98 | 802 | 206 | 0.00% | 0.08 | 1.00 |
| 50% industrial | 30% | v4 = v2 + v3 | 0.00% | 343 | 337 | 50.00% | 34.3 | 27.6 | 0.373 | 9.37 | 4.81 | 2.10% | 0.56 | 0.97 | 801 | 206 | 0.00% | 0.08 | 1.00 |
| 50% industrial | 30% | v5 = v4, one ask opens | 0.00% | 343 | 337 | 50.00% | 34.3 | 27.6 | 0.373 | 9.37 | 4.81 | 2.10% | 0.56 | 0.97 | 801 | 206 | 0.00% | 0.08 | 1.00 |
| 90% industrial | 10% | today | 0.00% | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 1.21 | 2.76 | 4.65 | 9.90% | 0.76 | 0.94 | 242 | 207 | 0.00% | 0.09 | 1.00 |
| 90% industrial | 10% | v0 as written | 0.00% | 189 | 186 | 5.34% | 18.9 | 15.2 | 0.225 | 15.7 | 4.84 | 1.20% | 0.52 | 0.98 | 1,325 | 206 | 0.00% | 0.08 | 0.99 |
| 90% industrial | 10% | v4 = v2 + v3 | 0.00% | 187 | 184 | 7.71% | 18.7 | 15.1 | 0.228 | 15.5 | 4.92 | 1.20% | 0.53 | 0.99 | 1,310 | 207 | 0.00% | 0.07 | 1.00 |
| 90% industrial | 10% | v5 = v4, one ask opens | 0.00% | 186 | 184 | 7.68% | 18.6 | 15.0 | 0.228 | 15.5 | 4.92 | 1.60% | 0.53 | 0.99 | 1,310 | 207 | 0.00% | 0.07 | 1.00 |
| 90% industrial | 30% | today | 0.00% | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 3.63 | 0.962 | 2.96 | 40.50% | 1.18 | 0.99 | 80.6 | 204 | 0.00% | 0.13 | 1.00 |
| 90% industrial | 30% | v0 as written | 0.00% | 334 | 327 | 10.00% | 33.4 | 26.8 | 0.385 | 9.11 | 4.86 | 1.90% | 0.57 | 0.98 | 778 | 206 | 0.00% | 0.08 | 1.00 |
| 90% industrial | 30% | v4 = v2 + v3 | 0.00% | 334 | 329 | 10.00% | 33.4 | 26.8 | 0.383 | 9.16 | 4.89 | 1.90% | 0.57 | 0.99 | 782 | 207 | 0.00% | 0.08 | 1.00 |
| 90% industrial | 30% | v5 = v4, one ask opens | 0.00% | 334 | 328 | 10.00% | 33.4 | 26.8 | 0.383 | 9.14 | 4.65 | 2.40% | 0.57 | 0.94 | 781 | 207 | 0.00% | 0.07 | 1.00 |
| 99% industrial | 10% | today | 0.00% | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 1.21 | 2.76 | 4.64 | 9.60% | 0.76 | 0.95 | 242 | 206 | 0.00% | 0.09 | 1.00 |
| 99% industrial | 10% | v0 as written | 0.00% | 81.7 | 81.1 | 1.00% | 8.17 | 6.64 | 0.517 | 6.74 | 4.76 | 3.20% | 0.61 | 0.98 | 573 | 207 | 0.00% | 0.08 | 1.00 |
| 99% industrial | 10% | v4 = v2 + v3 | 0.00% | 49.5 | 49.4 | 1.00% | 4.95 | 4.06 | 0.849 | 3.97 | 4.61 | 7.00% | 0.68 | 0.93 | 347 | 206 | 0.00% | 0.09 | 1.00 |
| 99% industrial | 10% | v5 = v4, one ask opens | 0.00% | 66.8 | 66.4 | 1.00% | 6.68 | 5.44 | 0.631 | 5.41 | 4.72 | 3.90% | 0.64 | 0.98 | 468 | 207 | 0.00% | 0.08 | 1.00 |
| 99% industrial | 30% | today | 0.00% | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 3.63 | 0.962 | 2.96 | 40.50% | 1.18 | 0.99 | 80.6 | 205 | 0.00% | 0.13 | 1.00 |
| 99% industrial | 30% | v0 as written | 0.00% | 81.5 | 80.7 | 1.00% | 8.15 | 6.62 | 1.56 | 2.20 | 4.74 | 14.00% | 0.80 | 0.97 | 190 | 207 | 0.00% | 0.10 | 1.00 |
| 99% industrial | 30% | v4 = v2 + v3 | 0.00% | 49.7 | 49.6 | 1.00% | 4.97 | 4.08 | 2.53 | 1.37 | 4.12 | 25.20% | 0.95 | 0.96 | 116 | 206 | 0.00% | 0.11 | 1.00 |
| 99% industrial | 30% | v5 = v4, one ask opens | 0.00% | 66.2 | 65.9 | 1.00% | 6.62 | 5.40 | 1.91 | 1.78 | 4.68 | 20.40% | 0.89 | 0.95 | 155 | 207 | 0.00% | 0.11 | 1.00 |
| zipf | 10% | today | 0.00% | 34.5 | 37.2 | 0.00% | 3.45 | 2.86 | 1.11 | 3.00 | 4.68 | 8.40% | 0.72 | 0.96 | 263 | 207 | 0.00% | 0.09 | 1.00 |
| zipf | 10% | v0 as written | 0.00% | 141 | 148 | 1.62% | 14.1 | 11.4 | 0.279 | 12.6 | 4.78 | 1.60% | 0.52 | 0.97 | 1,067 | 206 | 0.00% | 0.07 | 1.00 |
| zipf | 10% | v4 = v2 + v3 | 0.00% | 139 | 146 | 2.06% | 13.9 | 11.2 | 0.283 | 12.5 | 4.99 | 1.40% | 0.54 | 1.01 | 1,054 | 206 | 0.00% | 0.08 | 1.00 |
| zipf | 10% | v5 = v4, one ask opens | 0.00% | 141 | 148 | 1.75% | 14.1 | 11.4 | 0.280 | 12.6 | 4.64 | 1.50% | 0.54 | 0.98 | 1,065 | 206 | 0.00% | 0.08 | 1.00 |
| zipf | 30% | today | 0.00% | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 3.63 | 0.962 | 2.96 | 40.50% | 1.18 | 0.99 | 80.6 | 204 | 0.00% | 0.13 | 1.00 |
| zipf | 30% | v0 as written | 0.00% | 309 | 304 | 4.57% | 30.9 | 24.9 | 0.414 | 8.44 | 4.73 | 2.40% | 0.58 | 0.96 | 723 | 207 | 0.00% | 0.08 | 1.00 |
| zipf | 30% | v4 = v2 + v3 | 0.00% | 311 | 305 | 4.55% | 31.1 | 25.0 | 0.413 | 8.46 | 4.71 | 3.10% | 0.57 | 0.97 | 724 | 207 | 0.00% | 0.08 | 1.00 |
| zipf | 30% | v5 = v4, one ask opens | 0.00% | 310 | 304 | 4.59% | 31.0 | 24.9 | 0.414 | 8.44 | 4.68 | 2.20% | 0.56 | 0.96 | 723 | 207 | 0.00% | 0.08 | 1.00 |
| medium | 10% | today | 0.00% | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 1.21 | 2.76 | 4.65 | 9.90% | 0.76 | 0.94 | 242 | 207 | 0.00% | 0.09 | 1.00 |
| medium | 10% | v0 as written | 0.00% | 190 | 186 | 5.36% | 19.0 | 15.3 | 0.225 | 15.7 | 4.87 | 0.90% | 0.52 | 0.98 | 1,325 | 206 | 0.00% | 0.08 | 1.00 |
| medium | 10% | v4 = v2 + v3 | 0.00% | 187 | 184 | 7.72% | 18.7 | 15.0 | 0.228 | 15.5 | 4.87 | 1.70% | 0.52 | 0.98 | 1,310 | 207 | 0.00% | 0.07 | 1.00 |
| medium | 10% | v5 = v4, one ask opens | 0.00% | 187 | 184 | 7.71% | 18.7 | 15.1 | 0.228 | 15.5 | 4.85 | 1.70% | 0.53 | 0.98 | 1,310 | 207 | 0.00% | 0.07 | 1.00 |
| medium | 30% | today | 0.00% | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 3.63 | 0.962 | 2.96 | 40.50% | 1.18 | 0.99 | 80.6 | 204 | 0.00% | 0.13 | 1.00 |
| medium | 30% | v0 as written | 0.00% | 334 | 328 | 10.00% | 33.4 | 26.8 | 0.384 | 9.12 | 4.78 | 2.90% | 0.58 | 0.96 | 779 | 208 | 0.00% | 0.08 | 1.01 |
| medium | 30% | v4 = v2 + v3 | 0.00% | 334 | 328 | 10.00% | 33.4 | 26.8 | 0.384 | 9.13 | 4.85 | 1.90% | 0.56 | 0.98 | 780 | 207 | 0.00% | 0.08 | 1.00 |
| medium | 30% | v5 = v4, one ask opens | 0.00% | 334 | 327 | 10.00% | 33.4 | 26.8 | 0.385 | 9.10 | 4.83 | 2.20% | 0.58 | 0.98 | 777 | 206 | 0.00% | 0.08 | 1.00 |
| medium, few big | 10% | today | 0.00% | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 1.21 | 2.76 | 4.65 | 9.90% | 0.76 | 0.94 | 242 | 207 | 0.00% | 0.09 | 1.00 |
| medium, few big | 10% | v0 as written | 0.00% | 189 | 186 | 5.33% | 18.9 | 15.2 | 0.225 | 15.7 | 4.79 | 1.50% | 0.52 | 0.97 | 1,325 | 207 | 0.00% | 0.08 | 1.00 |
| medium, few big | 10% | v4 = v2 + v3 | 0.00% | 187 | 184 | 7.71% | 18.7 | 15.0 | 0.228 | 15.5 | 4.91 | 1.30% | 0.52 | 0.99 | 1,310 | 206 | 0.00% | 0.08 | 1.00 |
| medium, few big | 10% | v5 = v4, one ask opens | 0.00% | 186 | 184 | 7.70% | 18.6 | 15.0 | 0.228 | 15.5 | 4.97 | 0.90% | 0.51 | 1.00 | 1,310 | 206 | 0.00% | 0.08 | 1.00 |
| medium, few big | 30% | today | 0.00% | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 3.63 | 0.962 | 2.96 | 40.50% | 1.18 | 0.99 | 80.6 | 204 | 0.00% | 0.13 | 1.00 |
| medium, few big | 30% | v0 as written | 0.00% | 334 | 329 | 10.00% | 33.4 | 26.8 | 0.383 | 9.15 | 4.83 | 2.80% | 0.59 | 0.98 | 782 | 207 | 0.00% | 0.08 | 1.00 |
| medium, few big | 30% | v4 = v2 + v3 | 0.00% | 334 | 328 | 10.00% | 33.4 | 26.8 | 0.384 | 9.12 | 4.82 | 2.20% | 0.57 | 0.97 | 779 | 207 | 0.00% | 0.08 | 1.00 |
| medium, few big | 30% | v5 = v4, one ask opens | 0.00% | 334 | 327 | 10.00% | 33.4 | 26.8 | 0.384 | 9.12 | 4.85 | 2.10% | 0.57 | 0.98 | 779 | 206 | 0.00% | 0.08 | 1.00 |



## Study: the attacks on the asks under each rule (band-weak-levels, weak work, 50% industrial)

| Attack | Its part | BCH hashrate, pool | Rule | Weak expired | Weak a share | R | Asking | Weak/s | KB/s | Smallest miner with 3 weak shares a month (TH/s) | Bitaxe weak/mo | Bitaxe pay/mo | Bitaxe empty months | Bitaxe swing | Bitaxe vs fair | ASIC weak/mo | ASIC pay/mo | ASIC empty months | ASIC swing | ASIC vs fair |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| asks whatever it finds | 0.00% | x1, 10% | v0 as written | 0.00% | 46.3 | 46.4 | 0.00% | 4.63 | 3.81 | 0.090 | 39.0 | 24.8 | 0.00% | 0.26 | 1.00 | 3,244 | 2,013 | 0.00% | 0.03 | 1.00 |
| asks whatever it finds | 0.00% | x1, 10% | v1 down to R/2 | 0.00% | 13.7 | 14.4 | 46.07% | 1.37 | 1.20 | 0.290 | 11.4 | 24.5 | 0.00% | 0.34 | 0.99 | 961 | 1,705 | 0.00% | 0.04 | 1.00 |
| asks whatever it finds | 0.00% | x1, 10% | v2 cap follows asks | 0.00% | 34.5 | 34.8 | 0.00% | 3.45 | 2.86 | 0.120 | 29.1 | 24.9 | 0.00% | 0.27 | 1.00 | 2,421 | 1,967 | 0.00% | 0.03 | 1.00 |
| asks whatever it finds | 0.00% | x1, 10% | v5 = v4, one ask opens | 0.00% | 34.5 | 34.9 | 0.00% | 3.45 | 2.86 | 0.120 | 29.1 | 24.7 | 0.00% | 0.28 | 0.99 | 2,422 | 1,969 | 0.00% | 0.03 | 1.00 |
| asks whatever it finds | 1.00% | x1, 10% | v0 as written | 0.00% | 82.1 | 81.3 | 1.00% | 8.21 | 6.67 | 0.052 | 69.1 | 24.7 | 0.00% | 0.23 | 1.00 | 5,742 | 2,050 | 0.00% | 0.03 | 1.00 |
| asks whatever it finds | 1.00% | x1, 10% | v1 down to R/2 | 0.00% | 14.0 | 14.7 | 45.03% | 1.40 | 1.22 | 0.286 | 11.6 | 24.5 | 0.00% | 0.34 | 0.99 | 977 | 1,712 | 0.00% | 0.04 | 1.00 |
| asks whatever it finds | 1.00% | x1, 10% | v2 cap follows asks | 0.00% | 49.6 | 49.6 | 1.00% | 4.96 | 4.07 | 0.085 | 41.7 | 25.0 | 0.00% | 0.24 | 1.01 | 3,474 | 2,019 | 0.00% | 0.03 | 1.00 |
| asks whatever it finds | 1.00% | x1, 10% | v5 = v4, one ask opens | 0.00% | 66.6 | 66.1 | 1.00% | 6.66 | 5.43 | 0.063 | 55.9 | 24.6 | 0.00% | 0.24 | 0.99 | 4,657 | 2,041 | 0.00% | 0.03 | 1.00 |
| asks whatever it finds | 2.00% | x1, 10% | v0 as written | 0.00% | 222 | 220 | 2.00% | 22.2 | 17.9 | 0.019 | 188 | 25.0 | 0.00% | 0.21 | 1.01 | 15,647 | 2,068 | 0.00% | 0.02 | 1.00 |
| asks whatever it finds | 2.00% | x1, 10% | v1 down to R/2 | 0.00% | 14.4 | 15.1 | 42.36% | 1.44 | 1.25 | 0.278 | 12.0 | 24.7 | 0.00% | 0.36 | 1.00 | 1,009 | 1,724 | 0.00% | 0.04 | 1.00 |
| asks whatever it finds | 2.00% | x1, 10% | v2 cap follows asks | 0.00% | 249 | 245 | 2.00% | 24.9 | 20.1 | 0.017 | 210 | 25.2 | 0.00% | 0.21 | 1.01 | 17,455 | 2,068 | 0.00% | 0.02 | 1.00 |
| asks whatever it finds | 2.00% | x1, 10% | v5 = v4, one ask opens | 0.00% | 217 | 213 | 2.00% | 21.7 | 17.4 | 0.020 | 182 | 24.7 | 0.00% | 0.21 | 1.00 | 15,172 | 2,066 | 0.00% | 0.02 | 1.00 |
| asks whatever it finds | 5.00% | x1, 10% | v0 as written | 0.00% | 315 | 310 | 5.00% | 31.5 | 25.3 | 0.014 | 265 | 24.9 | 0.00% | 0.20 | 1.00 | 22,077 | 2,069 | 0.00% | 0.02 | 1.00 |
| asks whatever it finds | 5.00% | x1, 10% | v1 down to R/2 | 0.00% | 18.2 | 18.8 | 24.95% | 1.82 | 1.56 | 0.223 | 15.2 | 24.8 | 0.00% | 0.31 | 1.00 | 1,275 | 1,850 | 0.00% | 0.04 | 1.00 |
| asks whatever it finds | 5.00% | x1, 10% | v2 cap follows asks | 0.00% | 316 | 311 | 5.00% | 31.6 | 25.4 | 0.013 | 267 | 24.9 | 0.00% | 0.21 | 1.00 | 22,166 | 2,069 | 0.00% | 0.02 | 1.00 |
| asks whatever it finds | 5.00% | x1, 10% | v5 = v4, one ask opens | 0.00% | 316 | 310 | 5.00% | 31.6 | 25.4 | 0.014 | 266 | 24.9 | 0.00% | 0.20 | 1.00 | 22,142 | 2,068 | 0.00% | 0.02 | 1.00 |
| asks whatever it finds | 25.00% | x1, 10% | v0 as written | 0.00% | 342 | 336 | 24.99% | 34.2 | 27.4 | 0.012 | 288 | 24.7 | 0.00% | 0.21 | 0.99 | 23,961 | 2,066 | 0.00% | 0.02 | 1.00 |
| asks whatever it finds | 25.00% | x1, 10% | v1 down to R/2 | 0.00% | 53.8 | 53.6 | 25.01% | 5.38 | 4.41 | 0.078 | 45.1 | 24.8 | 0.00% | 0.25 | 1.00 | 3,763 | 2,023 | 0.00% | 0.03 | 1.00 |
| asks whatever it finds | 25.00% | x1, 10% | v2 cap follows asks | 0.00% | 342 | 336 | 24.99% | 34.2 | 27.4 | 0.012 | 288 | 24.7 | 0.00% | 0.21 | 0.99 | 23,961 | 2,066 | 0.00% | 0.02 | 1.00 |
| asks whatever it finds | 25.00% | x1, 10% | v5 = v4, one ask opens | 0.00% | 341 | 334 | 25.00% | 34.1 | 27.4 | 0.013 | 287 | 24.7 | 0.00% | 0.21 | 1.00 | 23,863 | 2,068 | 0.00% | 0.02 | 1.00 |
| never asks | 50.00% | x10, 30% | v0 as written | 0.00% | 341 | 334 | 25.00% | 34.1 | 27.4 | 0.377 | 9.29 | 4.88 | 2.40% | 0.55 | 0.98 | 795 | 207 | 0.00% | 0.08 | 1.00 |
| never asks | 50.00% | x10, 30% | v1 down to R/2 | 0.00% | 53.7 | 53.4 | 25.02% | 5.37 | 4.40 | 2.35 | 1.47 | 4.52 | 24.00% | 0.90 | 0.98 | 125 | 205 | 0.00% | 0.11 | 1.00 |
| never asks | 50.00% | x10, 30% | v2 cap follows asks | 0.00% | 341 | 334 | 25.00% | 34.1 | 27.4 | 0.377 | 9.29 | 4.88 | 2.40% | 0.55 | 0.98 | 795 | 207 | 0.00% | 0.08 | 1.00 |
| never asks | 50.00% | x10, 30% | v5 = v4, one ask opens | 0.00% | 342 | 336 | 25.00% | 34.2 | 27.4 | 0.375 | 9.34 | 4.85 | 2.00% | 0.54 | 0.98 | 799 | 206 | 0.00% | 0.08 | 1.00 |
| never asks | 90.00% | x10, 30% | v0 as written | 0.00% | 316 | 311 | 5.00% | 31.6 | 25.4 | 0.404 | 8.64 | 4.73 | 3.00% | 0.57 | 0.95 | 740 | 207 | 0.00% | 0.08 | 1.00 |
| never asks | 90.00% | x10, 30% | v1 down to R/2 | 0.00% | 14.7 | 15.4 | 5.00% | 1.47 | 1.28 | 8.18 | 0.409 | 1.27 | 65.70% | 1.72 | 0.96 | 34.6 | 110 | 0.00% | 0.18 | 1.01 |
| never asks | 90.00% | x10, 30% | v2 cap follows asks | 0.00% | 317 | 311 | 5.00% | 31.7 | 25.5 | 0.404 | 8.65 | 4.86 | 1.50% | 0.55 | 0.98 | 741 | 206 | 0.00% | 0.08 | 1.00 |
| never asks | 90.00% | x10, 30% | v5 = v4, one ask opens | 0.00% | 316 | 310 | 5.00% | 31.6 | 25.3 | 0.406 | 8.60 | 4.76 | 2.30% | 0.56 | 0.96 | 736 | 206 | 0.00% | 0.08 | 1.00 |
| never asks | 97.00% | x10, 30% | v0 as written | 0.00% | 143 | 140 | 1.50% | 14.3 | 11.5 | 0.895 | 3.83 | 4.71 | 6.00% | 0.70 | 0.95 | 333 | 207 | 0.00% | 0.09 | 1.00 |
| never asks | 97.00% | x10, 30% | v1 down to R/2 | 0.00% | 8.29 | 9.13 | 1.49% | 0.829 | 0.766 | 13.8 | 0.239 | 0.758 | 77.50% | 2.20 | 0.96 | 19.6 | 65.5 | 0.00% | 0.25 | 1.00 |
| never asks | 97.00% | x10, 30% | v2 cap follows asks | 0.00% | 95.1 | 94.2 | 1.50% | 9.51 | 7.71 | 1.34 | 2.55 | 4.72 | 11.90% | 0.78 | 0.96 | 223 | 206 | 0.00% | 0.10 | 1.00 |
| never asks | 97.00% | x10, 30% | v5 = v4, one ask opens | 0.00% | 127 | 125 | 1.50% | 12.7 | 10.3 | 1.00 | 3.41 | 4.78 | 8.00% | 0.69 | 0.96 | 297 | 207 | 0.00% | 0.09 | 1.00 |
| never asks | 99.00% | x10, 30% | v0 as written | 0.00% | 57.3 | 57.0 | 0.50% | 5.73 | 4.69 | 2.20 | 1.56 | 4.73 | 21.90% | 0.92 | 0.96 | 134 | 206 | 0.00% | 0.11 | 1.00 |
| never asks | 99.00% | x10, 30% | v1 down to R/2 | 0.00% | 7.44 | 8.29 | 0.50% | 0.744 | 0.698 | 15.2 | 0.214 | 0.735 | 79.20% | 2.34 | 1.03 | 17.6 | 59.3 | 0.00% | 0.26 | 1.00 |
| never asks | 99.00% | x10, 30% | v2 cap follows asks | 0.00% | 39.1 | 39.2 | 0.50% | 3.91 | 3.23 | 3.21 | 1.09 | 3.35 | 36.10% | 1.11 | 0.99 | 91.3 | 204 | 0.00% | 0.13 | 1.00 |
| never asks | 99.00% | x10, 30% | v5 = v4, one ask opens | 0.00% | 44.9 | 44.8 | 0.50% | 4.49 | 3.69 | 2.81 | 1.22 | 3.70 | 31.00% | 1.03 | 0.96 | 105 | 205 | 0.00% | 0.12 | 1.00 |
| industrial miners at the minimum | - | x10, 30% | today | 0.00% | 34.5 | 34.7 | 0.00% | 3.45 | 2.86 | 3.63 | 0.962 | 2.96 | 40.50% | 1.18 | 0.99 | 80.6 | 204 | 0.00% | 0.13 | 1.00 |
| industrial miners at the minimum | - | x10, 30% | v0 as written | 0.00% | 344 | 337 | 49.99% | 34.4 | 27.6 | 0.373 | 9.38 | 4.84 | 2.80% | 0.56 | 0.98 | 802 | 206 | 0.00% | 0.08 | 1.00 |
| industrial miners at the minimum | - | x10, 30% | v1 down to R/2 | 0.00% | 53.9 | 53.6 | 50.02% | 5.39 | 4.41 | 2.35 | 1.47 | 4.42 | 26.00% | 0.97 | 0.96 | 126 | 206 | 0.00% | 0.12 | 1.00 |
| industrial miners at the minimum | - | x10, 30% | v2 cap follows asks | 0.00% | 344 | 337 | 49.99% | 34.4 | 27.6 | 0.373 | 9.38 | 4.84 | 2.80% | 0.56 | 0.98 | 802 | 206 | 0.00% | 0.08 | 1.00 |
| industrial miners at the minimum | - | x10, 30% | v5 = v4, one ask opens | 0.00% | 343 | 337 | 50.00% | 34.3 | 27.6 | 0.373 | 9.37 | 4.81 | 2.10% | 0.56 | 0.97 | 801 | 206 | 0.00% | 0.08 | 1.00 |
| carries no asking weak share | 25.00% | x10, 30% | v0 as written | 0.00% | 250 | 246 | 50.00% | 25.0 | 20.1 | 0.511 | 6.87 | 4.88 | 2.90% | 0.59 | 0.98 | 585 | 207 | 0.00% | 0.08 | 1.00 |
| carries no asking weak share | 25.00% | x10, 30% | v1 down to R/2 | 0.00% | 20.5 | 21.1 | 49.99% | 2.05 | 1.74 | 5.97 | 0.571 | 1.73 | 57.10% | 1.52 | 0.96 | 48.1 | 150 | 0.00% | 0.17 | 1.00 |
| carries no asking weak share | 25.00% | x10, 30% | v2 cap follows asks | 0.00% | 231 | 227 | 50.01% | 23.1 | 18.6 | 0.554 | 6.30 | 4.76 | 2.70% | 0.61 | 0.96 | 539 | 206 | 0.00% | 0.08 | 1.00 |
| carries no asking weak share | 25.00% | x10, 30% | v5 = v4, one ask opens | 0.00% | 231 | 227 | 50.01% | 23.1 | 18.5 | 0.555 | 6.29 | 4.84 | 4.20% | 0.60 | 0.98 | 538 | 206 | 0.00% | 0.08 | 1.00 |
| carries no asking weak share | 50.00% | x10, 30% | v0 as written | 0.00% | 168 | 165 | 50.00% | 16.8 | 13.5 | 0.762 | 4.50 | 4.69 | 6.10% | 0.65 | 0.95 | 392 | 207 | 0.00% | 0.09 | 1.00 |
| carries no asking weak share | 50.00% | x10, 30% | v1 down to R/2 | 0.00% | 16.8 | 17.4 | 50.02% | 1.68 | 1.45 | 7.21 | 0.476 | 1.47 | 61.60% | 1.57 | 0.97 | 39.3 | 124 | 0.00% | 0.18 | 1.00 |
| carries no asking weak share | 50.00% | x10, 30% | v2 cap follows asks | 0.00% | 139 | 137 | 50.00% | 13.9 | 11.2 | 0.917 | 3.73 | 4.80 | 7.30% | 0.69 | 0.97 | 325 | 206 | 0.00% | 0.09 | 1.00 |
| carries no asking weak share | 50.00% | x10, 30% | v5 = v4, one ask opens | 0.00% | 140 | 138 | 49.99% | 14.0 | 11.3 | 0.913 | 3.75 | 4.75 | 6.50% | 0.68 | 0.96 | 327 | 206 | 0.00% | 0.09 | 1.00 |
| carries no asking weak share | 75.00% | x10, 30% | v0 as written | 0.00% | 107 | 106 | 50.01% | 10.7 | 8.66 | 1.19 | 2.87 | 4.73 | 11.50% | 0.74 | 0.95 | 251 | 207 | 0.00% | 0.10 | 1.00 |
| carries no asking weak share | 75.00% | x10, 30% | v1 down to R/2 | 0.00% | 15.3 | 15.9 | 50.00% | 1.53 | 1.33 | 7.89 | 0.432 | 1.41 | 64.90% | 1.77 | 1.03 | 35.9 | 114 | 0.00% | 0.19 | 1.01 |
| carries no asking weak share | 75.00% | x10, 30% | v2 cap follows asks | 0.00% | 85.8 | 85.0 | 50.00% | 8.58 | 6.96 | 1.48 | 2.29 | 4.65 | 13.50% | 0.82 | 0.94 | 201 | 207 | 0.00% | 0.10 | 1.00 |
| carries no asking weak share | 75.00% | x10, 30% | v5 = v4, one ask opens | 0.00% | 85.9 | 85.1 | 50.01% | 8.59 | 6.97 | 1.48 | 2.30 | 4.71 | 13.20% | 0.78 | 0.95 | 201 | 208 | 0.00% | 0.10 | 1.00 |
| carries no asking weak share | 90.00% | x10, 30% | v0 as written | 1.15% | 83.1 | 83.1 | 49.42% | 8.41 | 6.83 | 1.51 | 2.23 | 4.74 | 14.60% | 0.80 | 0.96 | 194 | 206 | 0.00% | 0.10 | 1.00 |
| carries no asking weak share | 90.00% | x10, 30% | v1 down to R/2 | 0.36% | 13.6 | 14.3 | 49.79% | 1.36 | 1.19 | 8.79 | 0.380 | 1.23 | 68.70% | 1.84 | 1.00 | 31.9 | 102 | 0.00% | 0.20 | 1.00 |
| carries no asking weak share | 90.00% | x10, 30% | v2 cap follows asks | 0.59% | 67.8 | 67.8 | 49.71% | 6.82 | 5.56 | 1.85 | 1.83 | 4.70 | 18.40% | 0.86 | 0.95 | 159 | 206 | 0.00% | 0.11 | 1.00 |
| carries no asking weak share | 90.00% | x10, 30% | v5 = v4, one ask opens | 0.58% | 67.9 | 67.8 | 49.72% | 6.83 | 5.56 | 1.85 | 1.83 | 4.65 | 19.80% | 0.89 | 0.94 | 159 | 207 | 0.00% | 0.10 | 1.00 |


