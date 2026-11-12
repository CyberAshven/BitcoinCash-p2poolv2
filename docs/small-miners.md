# Small miners: shares, windows and payouts on Bitcoin Cash

How often a small miner gets credited work, how often the pool finds blocks,
and what a miner can expect from each block, with the Bitcoin Cash rules of
this fork:

- one share chain with 10-second shares, at the former mini chain's anchor and
  ceiling (the main and mini chains merged on 2026-11-09);
- a difficulty band, so big miners can mine heavier shares;
- weak shares, so work at the minimum is credited many times a share period;
- payout levels, so no payment is dust;
- a PPLNS window of `K = 3` Bitcoin Cash blocks of work (plan decision 11),
  with a 100-share maturity skip.

Upstream's wiki pages "Shares For Small Miners" and "Expected Number of
Shares" do the same arithmetic for Bitcoin; this page redoes it for BCH.

The rules are in
[Share chain redesign for payout variance and fees](sharechain/small-miners-design.md)
and [architecture/payout-distribution.md](architecture/payout-distribution.md).
The numbers under "Worked examples" are the payout simulator's
(`p2poolv2_payout_sim`, its README's "Results" and "Tables from the default
run", 1,000 simulated months per row). Everything is expectation; real luck
varies around it.

## Inputs

| Quantity | Value | Source |
|---|---|---|
| BCH network hashrate `H` | 3.7 EH/s | Blockchair, 24-hour average, 2026-10-23 (difficulty 508.5 G) |
| Block interval | 600 s, 144 blocks a day | BCH consensus (aserti3-2d) |
| Block reward `R` | 3.125 BCH plus fees (fees ignored here) | BCH consensus, since the 2024 halving |
| Donation | 1% of each share's slice by default, so miners share 99% | plan decision 9, `cuts-per-share` |
| Share period | 10 s, 8,640 chain shares a day | share chain ASERT |
| Share target ceiling | difficulty about 4.55 (2e10 hashes) | `MINI_ANCHOR_TARGET` |
| Band cap `C` | heavier shares up to 64 times the minimum's work | `BAND_CAP_MAX_MULTIPLE` |
| Weak shares | up to 64 carried a share; the weak ratio `R_w` settles near 34.5 weak shares a share | `WEAK_CAP`, `WEAK_FILL`, payout simulator |
| Includer cut | 1% of a weak share's credit, to the share that carries it | `INCLUDER_CUT` |
| Window | shares worth `K = 3` blocks of work, at most `W` = 120,960 shares; weak work counts in the work and takes no slots | plan decision 11 |
| Maturity skip | 100 shares (about 17 minutes) | plan decision 11 |
| Payout levels | `546 * 2^k` satoshis, `k` 1 to 7; level 5 (17,472 sat) by default | `DEFAULT_PAYOUT_LEVEL` |
| Spending a payment | 148 satoshis (one P2PKH input at 1 sat/byte) | `DEFAULT_SPEND_COST` |
| Dust limit for a P2PKH output | 546 satoshis | BCHN policy |

Plug in current numbers: they move with BCH's hashrate and price, the
formulas do not.

## Formulas

For a pool holding a fraction `f` of BCH's hashrate (`P = f * H`) and a
miner with hashrate `m` in it:

- **Pool blocks:** `144 * f` a day, one every `600 / f` seconds on average.
- **The minimum:** ASERT sets the minimum share difficulty `D` so that the
  chain makes one share every 10 seconds. With every miner at the minimum,
  `D = P * 10 / 2^32`. A miner that would make more than
  `max(4 shares per pool block, 300 a month)` at the minimum gets heavier
  shares (the node default), so the chain makes fewer of its shares and
  ASERT eases `D` for everyone else. The minimum is never easier than the
  ceiling (4.55). The ceiling binds below about 2 GH/s, where shares come
  slower than one per 10 seconds, and there the node default keeps every
  miner at the minimum.
- **Weak shares:** a miner on a job at the minimum also finds weak shares,
  at `D / R_w`, about `R_w - 1` of them for each full share. Every proof from
  a job at the minimum, full share or weak share, is credited the weak work.
  So a small miner's credited work arrives in units about 35 times smaller
  than a share, `86,400 * m * R_w / (D * 2^32)` of them a day.
- **Window:** `min(K * 60 / f, W)` = `min(180 / f, 120,960)` shares, that many
  times 10 seconds long. The cap binds below `f` = 0.149%.
- **Expected income:** `144 * (m / H) * R * 0.99` BCH a day, at every pool
  size and with every rule on this page. PPLNS is fair on average: the
  window, the band, weak shares and levels change how lumpy payouts are, not
  their mean.
- **Payout levels:** a payee whose slice of a block is below its level `L`
  does not get a sub-level output. The payees below their level share one
  pot per level, and a draw over the pot pays `L` to about `pot / L` of
  them, each with a chance in proportion to its slice. A payee at or above
  its level is paid its slice directly. Either way a payment is never below
  546 satoshis, and the expected payment equals the slice.
- **Payments a month:** about `income / L` for a miner below its level every
  block, one each pool block for a miner above it. A node lowers a miner's
  level, from its choice or the default, until the miner can expect at least
  3 payments a month (`MIN_PAYMENTS_PER_MONTH`).
- **Fee loss:** `148 / payment`: 0.85% at level 5, 14% at level 1, the lowest.

## Pool size: windows and block intervals

10-minute blocks, `K = 3`:

| Pool's part of BCH | Hashrate | Pool blocks | Window |
|---|---|---|---|
| 30% | 1.1 EH/s | 43 a day, one per 33 min | 600 shares, 100 min |
| 10% | 370 PH/s | 14 a day, one per 100 min | 1,800 shares, 5 h |
| 1% | 37 PH/s | 1.4 a day, one per 16.7 h | 18,000 shares, 50 h |
| 0.1% | 3.7 PH/s | one per 6.9 days | 120,960 (cap), 14 days |
| 0.01% | 370 TH/s | one per 69 days | 120,960 (cap), 14 days |

A miner's credited work in the window adds up into one slice per block.

## Worked examples

Two tracked miners, a Bitaxe-class device (1.2 TH/s) and one ASIC
(100 TH/s), in a pool where half the hashrate is industrial (1 PH/s and
10 PH/s machines) and half is home miners. Both stay at payout level 5.
Expected income is the same under every design:

| Miner | Expected income |
|---|---|
| 1.2 TH/s | 433,459 sat a month (14,449 a day) |
| 100 TH/s | 0.36 BCH a month |

How it arrives, with the band, weak shares and levels (the swing is the
standard deviation of monthly income over its mean):

| Pool's part of BCH | Bitaxe payments a month | Bitaxe median payment | Bitaxe swing | Bitaxe fee loss | ASIC payments a month | ASIC median payment | ASIC swing |
|---|---:|---:|---:|---:|---:|---:|---:|
| 1% | 24.9 | 17,473 sat | 0.20 | 0.85% | 43.2 | 842,516 sat | 0.16 |
| 10% | 24.6 | 17,473 sat | 0.25 | 0.85% | 431 | 81,921 sat | 0.05 |
| 30% | 23.6 | 17,472 sat | 0.39 | 0.83% | 1,039 | 28,485 sat | 0.05 |

The same Bitaxe on one chain at one difficulty, without the band, weak
shares or levels:

| Pool's part of BCH | Payments a month | Swing | Months with no payment |
|---|---:|---:|---:|
| 1% | 19.5 | 0.41 | 0% |
| 10% | 2.41 | 1.31 | 46% |
| 30% | 0.76 | 2.39 | 78% |

So a home miner is paid about 25 times a month whatever the pool's size,
each payment about the level, and loses 0.85% of it to spending. Without
weak shares a larger pool pays the Bitaxe in rarer, bigger lumps: at 30% of
BCH it goes most months with nothing. The ASIC is above its level in nearly
every block and is paid each one.

### As BCH's hashrate grows

Pool at 10% of BCH, the same mix. The simulator holds the Bitaxe at level 5;
the last column is the level a node's default lowers it to so it can expect
at least 3 payments a month (computed, not simulated):

| BCH hashrate | Bitaxe income a month | Payments a month at level 5 | Swing at level 5 | Months with no payment | Level the node default gives |
|---|---:|---:|---:|---:|---|
| today (3.7 EH/s) | 433,459 sat | 24.6 | 0.25 | 0% | 5 (17,472 sat), about 25 a month |
| 3 times | 144,486 sat | 8.05 | 0.45 | 0% | 5, about 8 a month |
| 10 times | 43,346 sat | 2.44 | 0.87 | 20% | 4 (8,736 sat), about 5 a month, 1.7% to fees |
| 270 times (about Bitcoin's) | 1,605 sat | 0.10 | 4.51 | 94% | 0 (546 sat), about 3 a month, 27% to fees |

At Bitcoin's hashrate a Bitaxe still gets paid about every ten days, but a
quarter of each payment goes to spending it: the cost of a payment is fixed
while the income shrinks.

## How small can a miner be?

Every miner is paid its fair income on average, however small. How often it
is paid depends on two things: its income over its level, and how often it
finds a weak share, because a credit is never smaller than one weak share.
The second binds first. A pool makes about 8.3 million weak shares a month
(32 a share), split among its miners at the minimum, so the smallest miner
that still finds three a month is:

| BCH difficulty | Pool at 10% of BCH | 30% | 50% | 90% |
|---|---:|---:|---:|---:|
| 5.17e11 (today, 3.7 EH/s) | 0.09 TH/s | 0.36 | 0.60 | 1.1 |
| 5.17e12 (10 times) | 1.2 TH/s | 3.6 | 6.0 | 11 |
| 1.40e14 (270 times, Bitcoin's) | 33 TH/s | 98 | 163 | 294 |
| 5.17e14 (1,000 times) | 121 TH/s | 363 | 604 | 1,088 |

These are from the payout simulator's `scale` table, for a pool with half
its hashrate from home miners. A smaller miner is still paid fairly, in
rarer and larger pieces: at today's difficulty a 1.2 TH/s miner is paid 25
times a month in a pool at 10% of BCH and 9 times at 90%; at ten times the
difficulty, 5 times and once. So a device ages out as the difficulty grows,
unless it is upgraded: what counts is its part of the hashrate, not its
TH/s. The level formula alone (3 payments a month from 1,638 sat a month at
level 0, about 4.5 GH/s today) is a lower floor that the weak-share rate
reaches first.

Uncles, which the share chain pays at 9/10 of their difficulty, and the
includer cut on weak shares add a little to a miner's credit; the simulator
counts both.

## Timing

- A share is paid only once it is 100 shares deep, about 17 minutes.
- A weak share is credited only if a share carries it within 48 shares of its
  parent (`WEAK_DEPTH`), about 8 minutes; the simulator loses none that way.
- A coinbase output can be spent 100 BCH blocks after its block, about 17
  hours.
- After a miner stops, its work stays in the window for the window's length:
  100 minutes at 30% of BCH, 50 hours at 1%, 14 days at the cap.

## If BCH moves to 1-minute blocks (CHIP-2025-03)

The window is sized by the paying block's difficulty, so it shrinks with the
block interval on its own: `min(18 / f, 120,960)` shares. At 1% of BCH it is
1,800 shares (5 hours), with a pool block every 1.7 hours. Assuming the same
issuance per day (0.3125 BCH a block), each block pays a tenth as much and
there are ten times as many blocks, so a miner's slices are a tenth as large
and payout levels put more miners in the draw: income and payments a month
stay about the same. Tuning `K`, the cap and the share period for faster
blocks is an open question in [bch-plan.md](bch-plan.md); the 10-second share
period is kept for now (decision 11).

## Network size

Upstream sized P2Poolv2 for about 1,000 nodes. On BCH, 1% of the network is
37 PH/s: about 370 machines of 100 TH/s, or 31,000 Bitaxes. Each share is
about 1 KB to store (see [architecture/pruning.md](architecture/pruning.md),
"Growth of Data Kept Forever"), about 8.6 MB a day, whatever the number of
miners. Weak shares cost about 2.9 KB/s of messages at `WEAK_CAP` 64 and
`WEAK_FILL` 32, against 0.1 KB/s of shares, at any pool size (payout
simulator, "Results", item 5). Each block's coinbase pays every payee at or
above its level one output and the draw's winners one each, which is where
many small miners cost space; see [known-attacks.md](known-attacks.md) and
[requirements.md](requirements.md) for the coinbase size limits of stock
ASIC firmware.
