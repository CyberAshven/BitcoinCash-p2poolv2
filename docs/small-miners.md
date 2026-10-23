# Small miners: shares, windows and payouts on Bitcoin Cash

How often a small miner finds shares, how often the pool finds blocks, and
what a miner can expect from each block, with the Bitcoin Cash rules of this
fork: 10-second shares, a PPLNS window of `K = 3` Bitcoin Cash blocks of work
(plan decision 11), a 100-share maturity skip, and the main and mini share
chains. Upstream's wiki pages "Shares For Small Miners" and "Expected Number
of Shares" do the same arithmetic for Bitcoin; this page redoes it for BCH.

The rules themselves are in
[architecture/payout-distribution.md](architecture/payout-distribution.md).
Everything below is expectation; real luck varies around it.

## Inputs

| Quantity | Value | Source |
|---|---|---|
| BCH network hashrate `H` | 3.7 EH/s | Blockchair, 24-hour average, 2026-10-23 (difficulty 508.5 G) |
| Block interval | 600 s, 144 blocks a day | BCH consensus (aserti3-2d) |
| Block reward `R` | 3.125 BCH plus fees (fees ignored here) | BCH consensus, since the 2024 halving |
| Donation | 1.5% of each block, so miners share 98.5% | plan decision 9 |
| Share period | 10 s, 8,640 shares a day on each share chain | share chain ASERT |
| Window | shares worth `K = 3` blocks of work, at most `W` = 120,960 shares | plan decision 11 |
| Maturity skip | 100 shares (about 17 minutes) | plan decision 11 |
| Share target ceiling | main: difficulty about 1,164 (5e12 hashes); mini: about 4.55 (2e10 hashes) | `MAX_POOL_TARGET`, `MINI_ANCHOR_TARGET` |
| Dust limit for a P2PKH output | 546 satoshis | BCHN policy |

Plug in current numbers: they move with BCH's hashrate and price, the
formulas do not.

## Formulas

For a share chain holding a fraction `f` of BCH's hashrate (`P = f * H`) and
a miner with hashrate `m` on it:

- **Pool blocks:** `144 * f` a day, one every `600 / f` seconds on average.
- **Share difficulty:** `P * 10 / 2^32`, so the chain finds one share every
  10 seconds; never below the chain's ceiling difficulty (1,164 on main, 4.55
  on mini). The ceiling only binds below 500 GH/s on main and 2 GH/s on
  mini, where shares come slower than one per 10 seconds.
- **Window:** `min(K * 60 / f, W)` = `min(180 / f, 120,960)` shares, that many
  times 10 seconds long. The cap binds below `f` = 0.149%.
- **Miner's shares:** `8,640 * m / P` a day; `window * m / P` in the window
  at any moment.
- **Chance of no share in the window when a block is found:**
  `exp(-window * m / P)`. Such a block pays the miner nothing.
- **Expected income:** `144 * (m / H) * R * 0.985` BCH a day, on every chain
  and at every pool size. PPLNS is fair on average: the window size and the
  chain change how lumpy payouts are, not their mean.
- **Each share's slice of a block:** `R * 0.985 * d / (window work)`, where
  `d` is the share's difficulty: `R * 0.985 * f / 180` while the window is
  below the cap, `R * 0.985 / 120,960` at the cap. A share is paid by every
  block found while it is in the window: 3 blocks on average below the cap
  (a Poisson count: 5% of shares are paid by none), fewer at the cap, where
  the window is shorter than the time between pool blocks.

## Pool size: windows and block intervals

10-minute blocks, `K = 3`:

| Share chain's part of BCH | Hashrate | Pool blocks | Window | Slice per share per block |
|---|---|---|---|---|
| 30% | 1.1 EH/s | 43 a day, one per 33 min | 600 shares, 100 min | 513,000 sat |
| 10% | 370 PH/s | 14 a day, one per 100 min | 1,800 shares, 5 h | 171,000 sat |
| 1% | 37 PH/s | 1.4 a day, one per 16.7 h | 18,000 shares, 50 h | 17,100 sat |
| 0.1% | 3.7 PH/s | one per 6.9 days | 120,960 (cap), 14 days | 2,540 sat |
| 0.05% | 1.85 PH/s | one per 13.9 days | 120,960 (cap), 14 days | 2,540 sat |
| 0.01% | 370 TH/s | one per 69 days | 120,960 (cap), 14 days | 2,540 sat |

Every slice is above the 546-satoshi dust limit, and a miner's slices of one
block add up into one coinbase output.

## Worked examples

Three miners: a Bitaxe-class device (1.2 TH/s), one ASIC (100 TH/s) and a
small farm (1 PH/s). Expected income is the same on any chain:

| Miner | Expected income |
|---|---|
| 1.2 TH/s | 0.000144 BCH (14,400 sat) a day |
| 100 TH/s | 0.012 BCH a day |
| 1 PH/s | 0.12 BCH a day |

How it arrives differs.

**Main chain at 1% of BCH (37 PH/s, share difficulty about 86 million).**

| Miner | Shares a day | Shares in the window | No share in the window at a block | Paid per block when in it |
|---|---|---|---|---|
| 1.2 TH/s | 0.28 | 0.58 | 56% | about 17,100 sat per share |
| 100 TH/s | 23 | 49 | never | 0.0083 BCH |
| 1 PH/s | 234 | 486 | never | 0.083 BCH |

The Bitaxe finds a share about every 3.6 days. It is in the window for 44%
of the pool's blocks, and each share it finds is paid by three blocks on
average. It has at least one share in a day 24% of the time, in a week 86%,
in two weeks 98%.

**Mini chain at 0.05% of BCH (1.85 PH/s, share difficulty about 4.3
million).**

| Miner | Shares a day | Shares in the window | Paid per pool block |
|---|---|---|---|
| 1.2 TH/s | 5.6 | 79 | 0.0020 BCH |
| 100 TH/s | 467 | 6,500 | 0.17 BCH |

The Bitaxe sees several shares a day and is always in the 14-day window, but
the pool finds a block about every two weeks, and that block pays the whole
window at once.

**Mini chain at 0.01% of BCH (370 TH/s).** The Bitaxe finds 28 shares a day,
but the pool finds a block about every 69 days. Each block pays the last 14
days of work: 0.0100 BCH to a Bitaxe that mined through them. A 14-day stretch
of mining sees a pool block 18% of the time.

So the main chain's variance is in whether a small miner has a share in the
window; the mini chain's variance is in when the pool finds a block. The mini
chain shows a small miner its work counting every day, and the 14-day cap
bounds how long anyone is paid after stopping.

## How small can a miner be?

Upstream asked which hashrate finds at least one share in two weeks with 99%
probability: a mean of 4.6 shares in 120,960 share periods.

| Chain | Hashrate needed |
|---|---|
| Main at 1% of BCH (37 PH/s) | 1.4 TH/s, about one Bitaxe |
| Main at 10% of BCH (370 PH/s) | 14 TH/s |
| Mini at 0.05% of BCH (1.85 PH/s) | 70 GH/s |

Uncles, which the share chain pays at 9/10 of their difficulty, add more
chances; this count leaves them out.

## Timing

- A share is paid only once it is 100 shares deep, about 17 minutes.
- A coinbase output can be spent 100 BCH blocks after its block, about 17
  hours.
- After a miner stops, its shares stay in the window for the window's
  length: 100 minutes at 30% of BCH, 50 hours at 1%, 14 days at the cap.

## If BCH moves to 1-minute blocks (CHIP-2025-03)

The window is sized by the paying block's difficulty, so it shrinks with the
block interval on its own: `min(18 / f, 120,960)` shares. At 1% of BCH it is
1,800 shares (5 hours), with a pool block every 1.7 hours. Assuming the same
issuance per day (0.3125 BCH a block), each share's slice is unchanged below
the cap (17,100 sat at 1%). The cap would bind below 0.0149% of BCH, and a
share's slice at the cap would fall to about 250 sat, below the dust limit.
Tuning `K`, the cap and the share period for faster blocks is an open
question in [bch-plan.md](bch-plan.md); the 10-second share period is kept
for now (decision 11).

## Network size

Upstream sized P2Poolv2 for about 1,000 nodes. On BCH, 1% of the network is
37 PH/s: about 370 machines of 100 TH/s, or 31,000 Bitaxes. Each share is
about 1 KB to store (see [architecture/pruning.md](architecture/pruning.md),
"Growth of Data Kept Forever"), about 8.6 MB a day per share chain, whatever
the number of miners. Each block's coinbase pays every miner in the window
one output, which is where many small miners cost space; see
[known-attacks.md](known-attacks.md) and [requirements.md](requirements.md)
for the coinbase size limits of stock ASIC firmware.
