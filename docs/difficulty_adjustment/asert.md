# Share Difficulty: ASERT

Every share's target is set by ASERT, the integer algorithm Bitcoin Cash
uses for its own blocks (aserti3-2d), with a 10-second ideal share interval
and a 600-second half-life instead of BCH's 10 minutes and 2 days
(`IDEAL_BLOCK_TIME` and `HALFLIFE` in `sim_overrides.rs`). The code is
`p2poolv2_lib/src/pool_difficulty.rs`.

## Formula

```
next_target = anchor_target * 2^((time_delta - ideal * (height_delta + 1)) / half_life)
```

computed with BCH's fixed-point exponent, cubic approximation of `2^x` and
512-bit intermediate, then clamped so it is never easier than the chain's
ceiling: `MAX_POOL_TARGET` (`0x1b384bd7`, difficulty about 1,160) on Bitcoin
and the Bitcoin Cash main chain, `0x1c384bd7` (difficulty about 4) on mini.

Every caller passes the parent of the share being validated or mined as an
`AsertParent`: its height, time and bits, and the time of its own parent
(its own time for genesis, which has no parent).
`ChainStoreHandle::get_asert_parent` reads it from the parent's header and
metadata and its parent's header. Share validation, header sync (which
carries it from header to header in the batch), the block receiver and the
work notifier all use it, so they compute the same target from the same
share.

## Where it is anchored

| Chain | Anchor | `anchor_target` | `time_delta` | `height_delta` |
|---|---|---|---|---|
| Bitcoin | genesis | genesis bits | parent time - genesis time | parent height |
| Bitcoin Cash | the parent | parent bits | parent time - its parent's time | 0 |

Bitcoin keeps upstream's rule: the algorithm is absolute, so a share's
target follows from genesis and its parent's height and time alone.

Bitcoin Cash moves BCH's anchor convention to each share's parent: the
parent is the anchor block, its bits are the anchor target, and its own
parent's time is the anchor parent time. The first share builds on genesis,
whose interval is zero, so it gets the genesis rule's target. Without the
ceiling the two rules give the same targets, because the exponents add up
along the chain: two steps of the parent rule are one step of the genesis
rule over both intervals. They differ only at the ceiling.

## Why Bitcoin Cash does not anchor at genesis

The genesis anchor counts every second since genesis. A chain that spends
time behind schedule keeps that time as a debt: its target stays clamped at
the ceiling until enough fast shares have been mined to catch up with the
schedule, and while it is clamped every submission that meets the ceiling
is a share at the floor difficulty.

The Bitcoin Cash genesis (`bch_genesis::build`) takes its timestamp from
the regtest genesis data, 1776855600 (2026-04-22 11:00 UTC), on every
network. The chipnet mini chain restarted on 2026-10-19 13:24 UTC on a
fresh store, 180 days later: about 1.55 million shares behind schedule.
With the ASIC's submissions arriving every 3 seconds, each share made up 7
seconds, so the chain would have stayed at the ceiling for roughly two and
a half months. Every share had difficulty 4 at the mini ceiling, where a 4
TH/s ASIC should hold about 9,300 at the 10-second target. Shares that all
weigh the same make PPLNS pay by submission count, not work: a CPU miner
got 2.4% of chipnet block 328,554 and 31% of a block on 2026-10-18 beside
the ASIC. And the Bitcoin Cash PPLNS window, sized by work, always hit its
120,960-share cap.

The same happens after any outage. Two days without shares leave the chain
two days behind schedule, which at 3-second submissions is about 21 hours
at the floor. A small share chain is stopped and restarted far more often
than BCH's own chain, so this is a property of the rule, not of one start.

The parent anchor measures each step from the parent alone. When a long
interval drives a target to the ceiling, the next share starts from the
ceiling instead of from a schedule the chain has fallen behind, and ASERT
climbs from there at its normal rate.

### Options weighed

- **Re-anchor genesis at a later time.** Fixes one start and changes the
  genesis hash (`shares/genesis/` and its definitions are not to be touched
  without the maintainer's decision). Any later start or outage repeats the
  debt.
- **Anchor at the first share, as BCH anchors at its activation block.**
  Fixes the late start, not outages. The anchor would be the parent's
  ancestor at height 1, which a fork at height 1 makes ambiguous (shares at
  height 1 are cheap: they are mined at the ceiling), and finding it costs
  an ancestry walk or a new store index.
- **Anchor at the parent (chosen).** Local and constant cost: one more
  header read per target. Forgets debt only at the ceiling, and otherwise
  gives the genesis rule's targets.

## Properties

- **Deterministic.** Integer arithmetic only, and every input comes from
  the parent's header, its metadata and its own parent's header. Headers
  are never pruned (`pruning.md`), so every node reads the same values at
  any depth.
- **Timestamps.** A share's time sets only the next target, as on the
  genesis anchor, and the median-time-past (11 shares) and 120-second
  future limits bound it. The parent anchor forgets only at the easy
  ceiling, so compared with the genesis anchor it can make a later target
  harder, never easier.
- **Rounding.** Each step stores the target at compact precision (it rounds
  down by at most about 2^-15) and truncates the exponent, where the genesis
  anchor rounds once. ASERT's time term absorbs the bias: the steady share
  interval moves by about bias x half-life / ln 2, under 0.03 seconds.
- **Speed.** A share can make the target at most 2^(-10/600) harder, about
  1.2%, so climbing `n` doublings takes at least `60 n` shares. From the
  mini ceiling to a 4 TH/s miner's steady difficulty is eleven doublings.

Unit tests in `pool_difficulty.rs` mine a constant-hashrate miner against
both rules (`test_utils::mine_at_constant_hashrate`; a share takes the
miner's expected time at its target, at least the 3-second submission
interval):

| Case (4 TH/s miner) | Parent anchor (Bitcoin Cash) | Genesis anchor |
|---|---|---|
| Chipnet mini restarted 180 days after genesis | 0.95 of the steady difficulty after 8 half-lives (80 minutes, 1,139 shares) | at the ceiling |
| Main chain, same start | 0.94 after 4 half-lives | |
| Mini chain after a two-day outage | 0.95 eight half-lives after it resumes | at the ceiling |
| Genesis anchored at launch at the steady difficulty, as the sim does | 1.01 after 6 hours | 1.01, the same target |

The tests allow 15% (5% for the regulated start). Whole-second intervals
leave the model about 5% below the steady difficulty, where a share's
expected interval rounds to 10 seconds.

## Activation

The parent anchor is a consensus rule: a node on the genesis anchor
computes another target for every share once the chain has been behind
schedule, and rejects the other's shares. It ships as a flag day, as the
PPLNS window did (`docs/bch-plan.md`, decision 11): every chipnet and
regtest node upgrades in one release and starts from a new store path.
Bitcoin Cash protocol strings carry the rule as `asert-parent` after the
payout rule (`/p2pool/test/<genesis tag>/pplns-k3-m100/asert-parent/1.0.0`,
`BCH_ASERT_ANCHOR_TAG`; since the share-trading rules, followed by
`trade-htlc`, then `payout-holder`, then `payout-cap11`, then `uncle-once`), so nodes on the old rule fail the handshake
instead of trading shares the other rejects. Mainnet starts with the rule
from its first share.

## Simulation

`sim_overrides::genesis_timestamp` and `anchor_target` still anchor genesis
at the sim's launch time and at the steady difficulty of its network
hashrate. On Bitcoin nothing changes. On Bitcoin Cash the genesis time now
sets only the interval the second share is measured from, so anchoring at
launch still starts the chain regulated, and a run whose genesis time is far
in the past starts from the ceiling instead of from a debt.

## Bitcoin

Bitcoin's targets, protocol strings and fixtures are unchanged
(`test_asert_matches_fixture_share_headers`,
`test_share_sync_fixture_chain_validity`). The only change there is the
extra header read for the parent's own parent, which the genesis rule does
not use.
