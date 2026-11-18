# Share Difficulty Adjustment on the Bitcoin Cash Share Chain

This page is the study behind the share-difficulty rule on
[Share difficulty: ASERT](asert.md). It reruns upstream's comparison of
difficulty adjustment algorithms for the share chain
([`p2poolv2/pool_difficulty_adjustment_analysis`](https://github.com/p2poolv2/pool_difficulty_adjustment_analysis))
with this fork's own share-difficulty code, and adds the cases that matter on
Bitcoin Cash: miners switching between BTC and BCH, a pool growing from one
BitAxe to exahashes, and long quiet periods. The simulator is the Rust crate
`p2poolv2_daa` (binary `daa_sim`, run as `cargo daa-sim`).

Every number on this page comes out of a simulation: it is an estimate from
the model described under [Method](#method), not a measurement of a live
network. Each table averages 20 seeded runs per rule and scenario.

## In short

- **The production rule holds up.** ASERT anchored at each share's parent
  ([decision 12](../bch-plan.md#decisions-so-far), 600-second half-life)
  ties with the genesis anchor upstream chose and beats every window
  algorithm upstream compared, in upstream's own scenarios and scores:
  composite 942.7 against 942.4 for the genesis anchor. Upstream published
  939.4 for its ASERT, and the window algorithms land within 1% of
  upstream's published scores too.
- **Only the parent anchor survives outages.** After a two-day outage on
  the mini chain, the genesis anchor held the ceiling for about 24 hours and
  two thirds of all work earned no credit; a 100 MH/s CPU miner was paid
  2.9 times its work. The parent anchor settled in 74 minutes with 3.7% of
  work uncredited. After the chipnet-style late start the genesis anchor
  never left the ceiling and the CPU miner was paid 671 times its work,
  1.7% of all credit, close to the 2.4% of chipnet block 328,554 a CPU miner
  really got (see [asert.md](asert.md)).
- **Window algorithms stall when hashrate leaves.** LWMA, DGW, DigiShield
  and SMA shrug off a pause (they recover in one to four minutes, ASERT in
  about 35), but when a 1 PH/s farm left one BitAxe behind, LWMA-45 had not
  reached the BitAxe's difficulty 12 hours later. ASERT got there in about
  2.7 hours, 2.3 of them spent waiting for the first share at the farm's
  difficulty: one slow share is enough for ASERT to drop.
- **600 seconds is slow for BTC/BCH switching.** A 2 EH/s farm, about half
  of Bitcoin Cash's hashrate, joining and leaving a 50 PH/s pool every
  4 hours: the production rule took 32 minutes to settle within 10% after
  the farm arrived and 82 after it left; with a 300-second half-life 15 and
  40, with 150 seconds 7 and 20. The burst of shares when the farm arrives
  peaked at 776, 426 and 229 shares a minute. Switching every 30 minutes,
  the production rule never settled before the farm came back after
  leaving.
- **The cost of a shorter half-life is noise.** At constant hashrate the
  difficulty sits on average 0.093 doublings (about 7%) from where it should
  be at 600 s, 0.128 (9%) at 300 s and 0.181 (13%) at 150 s. Share intervals
  barely change (99th percentile 46, 47 and 48 seconds). Because PPLNS
  credits each share with its difficulty, the noise does not change what a
  miner earns on average.
- **One BitAxe could hold the former main chain.** Its steady difficulty,
  about 2,800, is 2.4 times the main ceiling, so a lone BitAxe got 10-second
  shares at its real difficulty. The Bitcoin Cash share chain is now the
  former mini chain, with a far lower ceiling
  ([One share chain](../sharechain/small-miners-design.md#1-one-share-chain)).

## Recommendations and decisions

| Recommendation | Decision |
|---|---|
| 1. Keep the parent anchor. It matches the genesis anchor wherever the genesis anchor works and is the only ASERT form here that recovers from outages and late starts. | Adopted: [decision 12](../bch-plan.md#decisions-so-far), in force on Bitcoin Cash. |
| 2. Keep ASERT rather than a window algorithm. The window algorithms recover faster from pauses and cap the bursts when a big miner arrives, but they fall slowly, and can stall for many hours, when hashrate really leaves; upstream's own scores still rank them below ASERT. | In force: share targets are ASERT. |
| 3. Consider a 300-second half-life (30 share intervals) before mainnet. | Not taken: `HALFLIFE` is still 600 s. Open with item 5. |
| 4. Do not cap the interval ASERT counts. | Followed: the rule counts the parent's whole interval. |
| 5. Revisit the half-life with the share interval. | Open. The plan keeps 10-second shares under one-minute blocks ([decision 11](../bch-plan.md#decisions-so-far)). |

**The 300-second half-life** halves every settling time measured here except
after a large drop, where the wait for the first share dominates; it halves
the share bursts when a big miner arrives and cuts the work left uncredited
after restarts by up to half, for about 40% more difficulty noise at steady
state. It needs a check in the sim swarm first, where propagation delay and
uncles exist, and one effect this model leaves out: a share stamped at the
120-second future limit makes the next share `2^(110 / half-life)` times
easier, 1.14 at 600 s, 1.29 at 300 s and 1.66 at 150 s. That share is
credited at its lower difficulty, so its finder gains nothing on average, but
it adds noise. 150 seconds measured better still on responsiveness, but it is
a bigger step from what upstream (600 s) and BCH's own aserti3-2d (two days
for 10-minute blocks) were tested with, and it doubles the noise.

**The interval cap.** Capping a parent's interval at two half-lives sped
recovery from the two-day outage (27 instead of 74 minutes) but slowed
recovery when hashrate really left (3.8 instead of 2.7 hours). A shorter
half-life gets most of the first without the second
([table](#capping-the-interval-asert-counts)).

**The share interval.** The plan keeps 10-second shares under
CHIP-2025-03's one-minute blocks. Farms switch on wall-clock schedules, so for
switching the half-life matters in seconds; for noise it matters in share
intervals (`half-life / interval`).

## Method

### The rules compared

Every rule reads only what share headers carry, whole-second times and
compact targets, and returns the next share's compact target, never easier
than the chain's ceiling.

| Rule | How it is computed |
|---|---|
| ASERT parent, 600 s (production) | `PoolDifficulty::calculate_target_clamped` on `Chain::BitcoinCash`: the node's own code. |
| ASERT parent, other half-lives | `pool_difficulty::asert_target_with_easy_clamp`, the integer function the production rule calls, with the parent's bits and interval and another half-life. A lib test pins it to the production rule at 600 s. |
| ASERT genesis, 600 s (upstream) | `PoolDifficulty` on `Chain::Bitcoin`: upstream's choice, anchored at share-chain genesis. |
| ASERT parent, interval cap | Exploratory, not a rule anywhere: the parent anchor with the parent's interval counted as at most the cap. |
| SMA-72, DigiShield v3, LWMA-45, LWMA-20, DGW v3 | As upstream's `daa_simulation.py` wrote them, in floating point, then rounded to a compact target. |
| SMA-72 (window mean) | Added: the window's mean difficulty times expected over actual window time. |

Upstream's "SMA-72" and "DigiShield v3" multiply the tip's difficulty by a
window ratio at every share, so the window's error compounds share after
share; the window-mean SMA shows what a moving average does without that.
Both are kept in upstream's form so the scores compare.

### The mining model

- **Shares arrive at random** (a Poisson process). Group `i` of miners finds
  shares at `hashrate_i / (max(D, d_i) * 2^32)` per second, where `D` is the
  share difficulty and `d_i` the stratum difficulty of one of its
  connections. The node's vardiff settles each connection at one submission
  per 3.33 seconds (`TARGET_DRR` = 0.3), between 1 and 2^32. When `D` is
  below `d_i`, every submission is a share and earns `D`, not `d_i`: the
  group is paid for only `D / d_i` of its work. That is how the chipnet
  ASIC was underpaid and the CPU miner overpaid at the mini ceiling
  ([asert.md](asert.md#why-bitcoin-cash-does-not-anchor-at-genesis)).
  Upstream's model had no such bound; its scenarios keep that.
- **Hashrate changes exactly.** The work a share needs is one exponential
  draw, spent across hashrate changes to the second.
- **Header times are whole seconds,** as a node stamps them.
- **Credit is the average credit:** a group's share rate times the share
  difficulty, integrated over time. It leaves out the lottery of which group
  finds the next share, so it shows what the difficulty rule does to
  fairness.
- **Seeds.** Run `n` uses seed `42 + 1000 n`, as upstream seeded its runs,
  with SplitMix64 (`p2poolv2_tools::seeded`). The same arguments give the
  same tables on the same platform.

### Measurements

Upstream's, computed as upstream computed them: mean, median, standard
deviation, 95th and 99th percentile of share intervals (percentile as
`sorted[floor(n * p)]`), percent under 2 seconds and over 30, and deviation
of the mean from 10 seconds. The composite score adds, per scenario,
deviation % + `10 * stddev / 10` + `5 * (>30 s %)` + `2 * (<2 s %)`, doubled
for the attack scenarios; the attack score adds deviation % +
`10 * (>30 s %)` + `20 * stddev / 10` over the oscillation and drop
scenarios. Lower is better. The first 100 shares are left out.

Added for Bitcoin Cash:

| Measure | Meaning |
|---|---|
| Tracking error | How far the share difficulty is from the steady difficulty of the hashrate mining it, in doublings (`abs(log2(D / D*))`), averaged over shares weighted by the work each took. 0.1 is about 7% off. |
| Settle up, settle down | After a rise or a fall in hashrate, the time until the chain first mines at a difficulty within 10% of the new steady difficulty. Settled % counts the changes that settled before the next one. |
| Peak shares/min | Most shares found in any minute, over all runs. On a real network a burst of shares turns into uncles and orphans. |
| Uncredited work % | Work done while the share difficulty was below a group's stratum difficulty, in percent of all work. |
| Credit/work | A group's share of the credit over its share of the work. 1.000 is fair. |

### Inputs

| Input | Value | Source |
|---|---|---|
| Ideal share interval, half-life | 10 s, 600 s | `IDEAL_BLOCK_TIME`, `HALFLIFE` in `sim_overrides.rs` |
| Former main chain: ceiling and genesis | `0x1b384bd7` (difficulty 1,164); genesis `0x1b4188f5` (1,000), easier than the ceiling, so the first share is at the ceiling | `MAX_POOL_TARGET`, `MAIN_ANCHOR_TARGET` in `p2poolv2_daa` |
| Mini chain (today's Bitcoin Cash chain): ceiling and genesis | `0x1c384bd7` (difficulty 4.55) | `MINI_ANCHOR_TARGET` |
| Bitcoin Cash hashrate | 3.7 EH/s | Blockchair 24-hour average, 2026-10-23, in [small-miners.md](../small-miners.md) |
| BitAxe | 1.2 TH/s | vendor listing (BitAxe Gamma) |
| Farm machine | 200 TH/s | a current large SHA-256 ASIC, estimated |
| Chipnet ASIC | 4 TH/s | [asert.md](asert.md) |
| CPU miner | 100 MH/s | illustrative |

### Reproducing

```sh
cargo daa-sim list                                  # rules and scenarios
cargo daa-sim upstream --runs 20                    # upstream's six scenarios and scores
cargo daa-sim upstream --runs 20 --rule asert-parent --rule asert-parent-h150 \
    --rule asert-parent-h300 --rule asert-parent-h1200 --rule asert-parent-h2400 \
    --rule asert-parent-h3600                       # the half-life sweep
cargo daa-sim bch --runs 20 --charts <directory>    # the Bitcoin Cash scenarios
cargo daa-sim bch --runs 20 --rule asert-parent --rule asert-parent-h300 \
    --rule asert-parent-h600-cap1200 --rule asert-parent-h300-cap600 \
    --rule lwma-45                                  # the interval cap
```

Each command takes a few seconds and prints the full table of every
scenario, all columns and rules, of which this page keeps the columns that
tell the rules apart. `--output <file>` writes the tables to a file;
`--charts` writes one SVG per scenario, four of which are below.

## Upstream's comparison, rerun

Upstream's six scenarios (5,000 shares from a chain already at the right
difficulty, the first 100 left out), with upstream's published scores beside
this run's. Upstream's ASERT is the genesis anchor.

| Rule | Composite | Upstream's composite | Attack | Upstream's attack |
|---|---|---|---|---|
| ASERT genesis, 600 s (upstream) | 942.4 | 939.4 | 329.3 | 328.1 |
| ASERT parent, 600 s (production) | 942.7 | | 328.4 | |
| SMA-72 (window mean) | 1,009.2 | | 369.2 | |
| DGW v3 (24) | 1,057.7 | 1,059.7 | 439.6 | 441.9 |
| LWMA-45 | 1,123.4 | 1,125.7 | 489.0 | 491.7 |
| LWMA-20 | 1,125.1 | 1,124.4 | 491.5 | 491.9 |
| DigiShield v3 (upstream form) | 1,273.6 | 1,280.7 | 431.9 | 432.8 |
| SMA-72 (upstream form) | 6,584.1 | 5,230.6 | 2,782.7 | 2,130.3 |

The ranking is upstream's, and the scores agree within 1% except for
upstream's SMA-72, whose compounding makes it unstable and its score depend
on the random draws. Both ASERT anchors give the same targets until the
ceiling binds, which it never does here.

The oscillation attack (a miner with nine times the base hashrate mining 50
shares and leaving for 50) is where the rules differ most:

| Rule | Mean s | P99 s | >30 s % | Deviation % |
|---|---|---|---|---|
| ASERT parent, 600 s (production) | 9.95 | 73.0 | 9.4 | 0.5 |
| ASERT genesis, 600 s (upstream) | 9.98 | 73.2 | 9.4 | 0.2 |
| SMA-72 (window mean) | 10.85 | 84.1 | 11.1 | 8.5 |
| DigiShield v3 (upstream form) | 10.83 | 97.9 | 9.2 | 8.3 |
| DGW v3 (24) | 14.38 | 132.6 | 11.7 | 43.8 |
| LWMA-45 | 15.43 | 126.5 | 14.9 | 54.3 |

The half-life sweep on upstream's scenarios favours shorter half-lives
mildly, mostly through gradual growth, where a long half-life lags:

| Half-life | Composite | Attack | Mean s, gradual 10x growth |
|---|---|---|---|
| 150 s | 913.2 | 314.7 | 9.89 |
| 300 s | 932.4 | 324.5 | 9.80 |
| 600 s (production) | 942.7 | 328.4 | 9.60 |
| 1,200 s | 957.5 | 332.1 | 9.21 |
| 2,400 s | 985.2 | 336.4 | 8.44 |
| 3,600 s | 1,006.3 | 336.0 | 7.70 |

## Bitcoin Cash scenarios

All run from the real genesis bits and ceiling of the former main chain or
the mini chain, with every miner bounded by its stratum connections.

| Scenario | What happens |
|---|---|
| One BitAxe, main chain | A 1.2 TH/s BitAxe mines alone for a day. |
| BitAxe to 10 EH/s | A BitAxe alone for 6 hours, then farms join and the pool grows exponentially to 10 EH/s in 7 days, doubling about every 7 hours: past today's whole BCH network. |
| 5 EH/s farm joins a BitAxe | After 2 hours a 5 EH/s farm of 200 TH/s machines points at a one-BitAxe pool at once: a four-million-fold step. |
| BTC/BCH switch, 4 h on, 8 h off | A 50 PH/s pool; a 2 EH/s farm (about half of BCH's hashrate) mines on it 4 hours in every 12 and on BTC otherwise, for 3 days. |
| BTC/BCH switch, 30 min on, 30 min off | The same farm switching every 30 minutes, as profit-switching services can, for a day. |
| Farm leaves a BitAxe for 12 hours | A 1 PH/s farm, 99.9% of the pool, switches away and returns 12 hours later; one BitAxe keeps mining. |
| Pool quiet for 1 hour, for 12 hours | A 1 PH/s pool stops and comes back. |
| Two-day outage, mini chain | The 4 TH/s chipnet ASIC and a 100 MH/s CPU miner stop for two days and come back. |
| Late start, mini chain | The same miners start the mini chain 180 days after its genesis time, as the chipnet restart did ([asert.md](asert.md#why-bitcoin-cash-does-not-anchor-at-genesis)). |

### The half-life across the scenarios

ASERT anchored at the parent, at each half-life. These are the figures
behind recommendations 3 and 5.

| Half-life | One BitAxe: tracking error | 4 h switch: settle up / down | 4 h switch: peak shares/min | 30 min switch: settled % / tracking error | 5 EH/s joins: settle / peak shares/min | 1 h quiet: settle | 2-day outage: settle / uncredited % | Late start: settle / uncredited % | Farm leaves: settle down |
|---|---|---|---|---|---|---|---|---|---|
| 150 s | 0.181 | 7.1 min / 19.9 min | 229 | 96 / 0.445 | 6.9 min / 314 | 8.3 min | 18.6 min / 0.9 | 17.2 min / 1.4 | 2.9 h |
| 300 s | 0.128 | 15.2 min / 40.2 min | 426 | 51 / 0.628 | 14.7 min / 589 | 17.4 min | 34.2 min / 1.8 | 35.0 min / 2.7 | 3.2 h |
| 600 s (production) | 0.093 | 31.6 min / 81.8 min | 776 | 28 / 0.878 | 31.3 min / 1,102 | 34.6 min | 74.3 min / 3.7 | 71.3 min / 5.5 | 2.7 h |
| 1,200 s | 0.074 | 63.5 min / 2.8 h | 1,415 | 0 / 1.032 | 63.3 min / 2,088 | 59.9 min | 2.4 h / 7.4 | 2.4 h / 11.0 | 4.6 h |
| 2,400 s | 0.074 | 2.2 h / 5.6 h | 2,481 | 0 / 1.117 | 2.2 h / 3,937 | 118.8 min | 4.9 h / 14.7 | 5.0 h / 22.1 | 8.3 h |

Halving the half-life roughly halves every settling time, every burst and
the uncredited work, except after the farm leaves, where the wait for the
first slow share dominates ([below](#quiet-periods-and-outages)).

### Switching between BTC and BCH

| Rule | 4 h cycle: settle up | settle down | peak shares/min | 30 min cycle: settled % | tracking error | P99 s | mean s |
|---|---|---|---|---|---|---|---|
| ASERT parent, 600 s (production) | 31.6 min | 81.8 min | 776 | 28 | 0.878 | 122.2 | 8.98 |
| ASERT genesis, 600 s | 31.4 min | 82.5 min | 786 | 29 | 0.876 | 124.7 | 8.99 |
| DigiShield v3 (upstream form) | 57 s | 31.1 min | 61 | 76 | 0.931 | 131.5 | 14.30 |
| LWMA-45 | 4.3 min | 99.1 min | 95 | 50 | 0.382 | 263.5 | 16.53 |
| DGW v3 (24) | 95 s | 53.4 min | 122 | 50 | 0.422 | 208.6 | 15.96 |

When the farm arrives, a rule that has not caught up yet pays out a burst of
cheap shares; when it leaves, shares come slowly until the difficulty falls.
ASERT falls by at most one doubling per half-life of waiting, so a 41-fold
drop (5.4 doublings) takes at least 54 minutes at 600 s. The window
algorithms rise within a few minutes and so cap the bursts, but they fall
slowly: LWMA-45 never settled after a departure in the 30-minute cycle, and
their 99th percentile interval there is three to four minutes, their mean
interval 14 to 17 seconds.

![Share difficulty, BTC/BCH switch every 30 minutes, first run](bch-daa/switch-30m.svg)

![Share difficulty, BTC/BCH switch 4 hours in 12, first run](bch-daa/switch-4h.svg)

The production rule's line lies under the genesis anchor's: the two give the
same targets away from the ceiling.

### From one BitAxe to exahashes

One BitAxe holds the former main chain at its own difficulty under every
rule (nothing at the ceiling except under DigiShield's noise). Steady
exponential growth to 10 EH/s is easy for all of them: tracking error 0.09
doublings for the production rule. The sudden four-million-fold step tells
them apart:

| Rule | Settle | Settle max | Shares while settling | Peak shares/min |
|---|---|---|---|---|
| ASERT parent, 600 s (production) | 31.3 min | 45.0 min | 1,502 | 1,102 |
| DigiShield v3 (upstream form) | 70 s | 5.9 min | 66 | 74 |
| LWMA-45 | 3.4 min | 15.0 min | 97 | 88 |
| DGW v3 (24) | 2.1 min | 6.2 min | 148 | 149 |

No work went uncredited under any rule: the farm's 25,000 machines submit
7,500 times a second, so the difficulty passes their stratum difficulty in
well under a second. ASERT can raise the difficulty by at most 1.2% per
share at 600 s, and near the target it closes the gap with a time constant
of half-life / ln 2, 14 minutes at 600 s.

### Quiet periods and outages

| Rule | 1 h quiet: settle | uncredited % | 2-day outage (mini): settle | uncredited % | CPU credit/work | Late start (mini): settle | uncredited % | CPU credit/work |
|---|---|---|---|---|---|---|---|---|
| ASERT parent, 600 s (production) | 34.6 min | 1.0 | 74.3 min | 3.7 | 1.038 | 71.3 min | 5.5 | 1.059 |
| ASERT genesis, 600 s | 34.1 min | 1.0 | 23.7 h | 66.1 | 2.947 | never | 99.9 | 671.252 |
| DigiShield v3 (upstream form) | 91 s | 0.1 | 2.7 min | 1.7 | 1.017 | 3.2 min | 1.8 | 1.018 |
| LWMA-45 | 84 s | 0.1 | 2.2 min | 0.3 | 1.003 | 11.8 min | 0.9 | 1.009 |
| DGW v3 (24) | 2.5 min | 0.1 | 3.6 min | 0.2 | 1.002 | 5.3 min | 0.5 | 1.005 |

ASERT anchored at the parent forgets a pause: the first share after it is
measured from a long interval, so the next target drops (to the ceiling
after a pause of more than a few half-lives) and the pool climbs back at
ASERT's rate, underpaying any miner whose stratum difficulty is above the
share difficulty meanwhile. The genesis anchor also keeps the pause as a
debt: after 12 quiet hours on the main chain, half of its shares in the run
were at the ceiling and 10.7% of the work went uncredited, and on the mini
chain it held the ceiling for about a day after the outage. The window
algorithms limit each solve time (LWMA to six intervals), so a pause barely
moves them.

![Share difficulty, two-day outage on the mini chain, first run](bch-daa/outage-2d-mini.svg)

The other side of that limit is a farm that leaves for good:

| Rule | Settle after it leaves | Settle after it returns | Settled % | Longest interval | Mean s | Uncredited % |
|---|---|---|---|---|---|---|
| ASERT parent, 600 s (production) | 2.7 h | 36.0 min | 100 | 4.7 h | 9.51 | 1.4 |
| ASERT genesis, 600 s | 3.3 h | 35.3 min | 98 | 9.1 h | 9.26 | 1.6 |
| DigiShield v3 (upstream form) | 5.2 h | 62 s | 82 | 12.0 h | 18.49 | 0.1 |
| LWMA-45 | never | 4.5 min | 50 | 6.8 h | 25.14 | 0.1 |
| DGW v3 (24) | 11.8 h | 100 s | 52 | 8.6 h | 25.24 | 0.1 |

Every rule sets a share's target from its ancestors only, as BCH's
aserti3-2d does for blocks, so the first share after the farm leaves is
mined at the farm's difficulty: a BitAxe needs 2.3 hours for it on average,
and the longest wait in 20 runs was 4.7 to 10.2 hours depending on the
draws. That wait dominates, so the ASERT half-lives up to 600 s settle
within noise of each other here. ASERT then drops in one step; LWMA-45 moves
at most a factor of 1.5 per share, each of them slow, and none of its
departures settled within the 12 hours.

![Share difficulty, a 1 PH/s farm leaves a BitAxe for 12 hours, first run](bch-daa/farm-leaves.svg)

### Capping the interval ASERT counts

An exploratory variant, not a rule: the parent anchor counting the parent's
interval as at most a cap, so one pause lowers the difficulty by at most
`(cap - 10) / half-life` doublings.

| Rule | 1 h quiet: settle | 2-day outage: settle | uncredited % | Farm leaves: settle after it leaves |
|---|---|---|---|---|
| ASERT parent, 600 s (production) | 34.6 min | 74.3 min | 3.7 | 2.7 h |
| ASERT parent, 600 s, cap 1,200 s | 27.6 min | 26.7 min | 1.8 | 3.8 h |
| ASERT parent, 300 s | 17.4 min | 34.2 min | 1.8 | 3.2 h |
| ASERT parent, 300 s, cap 600 s | 12.9 min | 14.8 min | 0.9 | 4.5 h |

The cap helps after pauses and hurts when hashrate really leaves, and it
does not touch the late start (the chain starts at the ceiling anyway). A
shorter half-life gets most of the first benefit without the second cost.

## Limits of this analysis

- It is a model of one node and one chain: no propagation delay, uncles or
  orphans, so fast shares cost nothing here; peak shares per minute shows
  where they would. Real pools also have stale work after a new block and
  miners that do not switch instantly.
- Vardiff's own settling is left out: a new connection starts at the
  configured start difficulty and needs minutes to reach its own.
- Timestamp manipulation and the median-time-past and future-time limits
  are left out.
- Block finding, the PPLNS window and payouts are left out. Credit is what
  PPLNS weights shares by.
- The window algorithms are upstream's versions, not the reference
  implementations of the coins they come from.
- The scenarios are chosen, not observed. Profit-switching patterns on BCH
  vary; the 30-minute and 4-hour cycles bracket them rather than measure
  them.
- Results are averages over 20 runs; columns that take a maximum (settle
  max, longest interval, peak shares per minute) are single draws and move
  with the seeds, and so do the settling times after a large drop.

## Credit and licence

The scenarios, the six algorithms as written, the metrics and the scores
are upstream's, from `p2poolv2/pool_difficulty_adjustment_analysis` at
commit `6e6ce1a` (pool2win, 2026-09-27; its README says the simulations
were run with an LLM). That repository has no licence file, so
`p2poolv2_daa` is a new Rust implementation of its model with credit, not a
copy of its code, and this page quotes only its published scores. ASERT is
Bitcoin Cash's aserti3-2d
([specification](https://upgradespecs.bitcoincashnode.org/2020-11-15-asert/):
600-second blocks, two-day half-life), as implemented for the share chain in
`p2poolv2_lib/src/pool_difficulty.rs`.
