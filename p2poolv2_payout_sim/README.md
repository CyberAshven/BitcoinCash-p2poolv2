# p2poolv2_payout_sim

An offline Monte Carlo of payouts under the Bitcoin Cash share chain designs
of [Share chain redesign for payout variance and fees](../docs/sharechain/small-miners-design.md)
(issue #79, row 2). It measures, for a 1.2 TH/s Bitaxe and a 100 TH/s ASIC,
how often they are paid, how large a typical payment is, how much their
monthly income swings, how often a month brings nothing and what spending
the payments costs, under each design, pool size, miner mix and Bitcoin Cash
hashrate. It also reports the pool's share, uncle and weak share rates and
bytes a second, and runs three attacks.

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
`attack-carriers` and `attack-spam`. Every run is a pure function of its
inputs and seed (default 1), drawn from `p2poolv2_netsim`'s seeded
xoshiro256** generator, so the tables below come out the same on any
machine. The whole default run, 1,000 simulated months per scenario, took
11 seconds on the 32-thread machine it was run on.

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

- Network effects beyond the stale rate: the share chain's own races,
  orphans, propagation and the uncle depth limit.
- The ABLA-like cap and the weak ratio as dynamics over time: `C` is fixed
  per run; `R` is simulated in the weak share queue but the tracked miners
  use its time average.
- The fourth attack on the page, a miner picking parents for a better draw;
  `R_MAX`, `WEAK_DEPTH`, `WEAK_BYTES` and the band step were not swept.
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
| The tables | `src/tables.rs`, `src/report.rs` |
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
