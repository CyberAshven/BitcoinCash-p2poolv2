# Stratum server and vardiff

> Ported from upstream P2Poolv2's documentation: `stratum.adoc` and
> `pool_difficulty_adjustment.adoc` in
> [p2poolv2/docs](https://github.com/p2poolv2/docs) at commit `623e4c1`,
> Copyright 2024-2026 P2Poolv2 Developers, MIT or Apache-2.0. Adapted to
> Bitcoin Cash by this fork; see the [port map](upstream-port-map.md).

Each P2Poolv2 node has its own Stratum server. It turns Bitcoin Cash block
templates into work for the mining rigs and keeps each rig at a target share
rate.

The server follows CKPool's parameters: one share about every 3.3 seconds
per rig, and CKPool's vardiff algorithm, which years of use have tested. It
is written in Rust in `p2poolv2_lib/src/stratum/`. Stratum V1 is served on
`[stratum] port`, and Stratum V2 on `sv2_port` when it is set.

## Components

```text
BCHN or Template Provider --templates----------> Template fetcher --> Job builder
Share chain ---------------payouts, tip--------> Job builder
Job builder <--------------difficulty----------> Vardiff
Job builder ---------------mining.notify-------> Miners
Miners --------------------new share-----------> Share validator
Share validator -----------pool shares---------> Share chain
Share validator -----------blocks--------------> BCHN or Template Provider
```

**Template fetcher.** Asks BCHN for a block template with
`getblocktemplate`, without Bitcoin's SegWit rules. It fetches one as soon as
BCHN announces a new block over ZMQ (`zmqpubhashblock`), right after this node
submits a block of its own, and otherwise every 10 seconds
(`GBT_POLL_INTERVAL` in `p2poolv2_node/src/lib.rs`), so that new transactions
make it into the work. With `template_source = "sv2"` the node takes
templates from a Stratum V2 Template Provider instead, checks them as it
checks BCHN's, and still asks BCHN for the height each template builds on
(`stratum/sv2/template_distribution.rs`).

**Job builder.** Turns each template into a mining job. The coinbase pays
miners by the PPLNS window over the share chain, after the donation and the
operator fee. It also carries the merge-mining output, a padding output and
the share commitment, and every job commits to the current share chain tip
(see [Share commitments](sharechain/share-commitments.md)). Bitcoin Cash
limits a coinbase scriptSig to 100 bytes (`MAX_COINBASE_SCRIPTSIG_SIZE` in
BCHN's `src/consensus/consensus.h`). Today the builder also holds every
coinbase to an 11-payout, 784-byte budget and refuses a larger one; that
budget is being redesigned (see [Coinbase payouts](payouts/coinbase.md)).
Every connected Stratum V1 miner gets the job with `mining.notify`; Stratum
V2 channels get the equivalent messages.

**Vardiff.** Adjusts each miner's difficulty so that it submits about one
share every 3.33 seconds (`TARGET_DRR` 0.3 in
`stratum/difficulty_adjuster/mod.rs`), whatever its hashrate, within the
node's `minimum_difficulty` and `maximum_difficulty`. It follows CKPool's
algorithm and waits for enough shares, or enough time, before changing
difficulty, so it does not overreact to luck. See
[Difficulty adjustment in CKPool](difficulty_adjustment/ckpool.md).

**Share difficulty cap.** In P2Poolv2 mode a miner's difficulty never goes
above the share difficulty of the job it mines (`share_difficulty_ceiling`
in `stratum/difficulty_adjuster/mod.rs`), whatever vardiff, the node's
`minimum_difficulty`, `d=`, `th=` or `mining.suggest_difficulty` ask for. A
miner submits only hashes that meet its own difficulty; a difficulty above
the share difficulty would make it throw away hashes that are shares, and
it would be paid for fewer hashes than it did. Before each job the node
lowers a session that is above the job's share difficulty and sends
`mining.set_difficulty` (SV1) or `SetTarget` (SV2) ahead of the job. The cap
is the share difficulty rounded down, so its target is never harder than the
share target. P2Pool v1 did the same (see
[What P2Pool v1 knew about Bitcoin Cash](p2pool-v1-bch-lessons.md), V8).
Hydrapool mode has no share chain and no cap. Once the share chain's ASERT
has settled, vardiff's own choice stays below the share difficulty (an
estimate: one submission every 3.3 s per connection against one share every
10 s for the whole pool), so the cap binds mainly on explicit settings.

**Share validator.** Checks every submitted share against the miner's
current difficulty and rejects duplicates. Then:

- a share that meets the **pool target** goes to the share chain, which
  stores it and relays it to peers;
- a share that also meets the **Bitcoin Cash target** is a block, and is
  submitted straight away: with `submitblock` to BCHN, or back to the
  Template Provider whose template it used.

**Found blocks** (`stratum/block_submit.rs`). A block is worth the whole
block reward, so it never depends on one RPC call. BCHN gets it again after
a connection error, a time-out (30 seconds per attempt) or a busy node
(HTTP 503), with waits from half a second doubling to 10 seconds, for up to
two minutes. BCHN's answer is final and is not sent again: `null` (accepted),
`duplicate` (BCHN already has the valid block, for example from an attempt
whose answer was lost), `inconclusive` (BCHN stored the block without a
verdict, for example because another block at this height arrived first and
stays its best chain) or a reject reason. Every node listed in
`[[submit_nodes]]` also gets the block, from its own task with its own
retries, also when the block went to a Template Provider. Each result is
logged and counted in `found_block_submissions_total{node,outcome}`, and
each retry in `found_block_submit_retries_total{node}` (see
[Metrics](architecture/metrics.md)). The Template Provider's RPC fallback
and the simulation retry the same way but count nothing.

Shares below the pool target still count towards the miner's measured
hashrate and vardiff, but do not enter the share chain.

Version rolling: the node offers the BIP 320 general-purpose bits
(`version_mask = "1fffe000"`), which ASIC firmware rolls on Bitcoin Cash as
on Bitcoin.

## The pool difficulty

The share chain's own target is set by ASERT. Upstream chose it after
simulations of several difficulty algorithms against oscillating hashrate,
a sudden 90% drop and noisy hashrate
([methodology](https://github.com/p2poolv2/pool_difficulty_adjustment_analysis/blob/6e6ce1a8ac50e0257a9839a9744fc3e52b0f3ecd/daa_evaluation.md),
[results](https://github.com/p2poolv2/pool_difficulty_adjustment_analysis/blob/6e6ce1a8ac50e0257a9839a9744fc3e52b0f3ecd/daa_simulation_report.md)),
with a 10-second target interval, uncles at most 3 deep and PPLNS
accounting. Upstream says those simulations were run with LLMs; they are
modelled results, not measurements on a live pool.

ASERT is also Bitcoin Cash's own block difficulty algorithm since November
2020 ([aserti3-2d](https://upgradespecs.bitcoincashnode.org/2020-11-15-asert/)).
This fork computes share targets with BCH's integer formula, a 10-second
interval and a 600-second half-life, anchored at each share's parent rather
than at genesis. Why, and how it behaved on chipnet, is in
[Share difficulty: ASERT](difficulty_adjustment/asert.md).

## Miner options

Usernames, the `p2p=`, `d=` and `th=` password options, and Stratum V2
settings are on [Miner options](miner-options.md).
