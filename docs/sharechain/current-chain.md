# Mining on the current chain

> Ported from upstream P2Poolv2's documentation:
> `architecture/bitcoin_chain_validation.adoc` in
> [p2poolv2/docs](https://github.com/p2poolv2/docs) at commit `623e4c1`,
> Copyright 2024-2026 P2Poolv2 Developers, MIT or Apache-2.0. Adapted to
> Bitcoin Cash by this fork; see the [port map](../upstream-port-map.md).

Upstream designed this defense for Bitcoin; this fork builds it for Bitcoin
Cash ([Requirements](../requirements.md#functional)). It is a share-chain
rule, carried in the protocol strings as `bch-prev-1off-ticks` (known headers
carry their one-minute-block ticks, see
[One-minute blocks](#one-minute-blocks)); earlier segments are in
[Rule history](rule-history.md). The parts:

- the header check (`p2poolv2_lib/src/shares/bch_headers.rs`), which
  requires each header's exact aserti3-2d nBits from its known parent;
- the store of known headers (`p2poolv2_lib/src/store/bch_headers.rs`),
  which a worker fills from the node's own BCHN
  (`p2poolv2_lib/src/node/bch_header_feeder.rs`): every 5 seconds it walks
  back from BCHN's best block to a known header, a new node first learns
  about 58 days of headers counted in ticks (8,400 ten-minute or 84,000
  one-minute blocks, stored in chunks from the tip down, before the node
  dials peers), and each new template's previous block is learned before
  jobs are built on it;
- carried headers (`p2poolv2_lib/src/shares/bch_carried_headers.rs`): share
  messages carry the BCH headers a receiver needs;
- the known BCH prev: a node stores a peer's share body only once it knows
  the share's BCH prev, and a share in the payout zone with an unknown BCH
  prev is `Recoverable` until its header arrives. Share headers, and so fork
  choice, never wait on it;
- the payout rule
  (`p2poolv2_lib/src/accounting/payout/sharechain_pplns/bch_eligibility.rs`).

This page explains the attack, the design, and what it needs on Bitcoin
Cash. The full design, with its formal model and this fork's deviations, is
[Mine on the current Bitcoin chain](../architecture/mine-on-bitcoin-chain.md).

## The hostile fork attack

A hostile fork starts when a group of miners splits from the Bitcoin Cash
chain and mines its own branch. If those miners also run P2Poolv2, they can
point each share's header at a block on their fork. Unless a rule withholds
payment for such shares, they join the share chain like any other and gain
PPLNS weight.

The attack then pays twice. When an honest share finds a Bitcoin Cash block,
its coinbase pays every share in its window, the fork's shares included. The
fork's miners earn BCH on the main chain for work that attacks it, and keep
whatever their fork earns. P2Poolv2 would subsidize the attack.

```text
Bitcoin Cash:  100 <- 101 <- 102 <- 103 <- 104 <- 105 (found by share S)
                       ^
Hostile fork:          +---- H102 <- H103 <- H104

Share chain:   1  <- 2  <- 3    <- 4  <- 5    <- 6  <- 7    <- S
builds on:     101   102   H102    103   H103    104   H104    104

Without the rule, S's coinbase pays shares 3, 5 and 7 for hostile work.
```

## The defense

A coinbase pays a share only for work on the Bitcoin Cash chain that the paying
block extends, give or take one block. Work on a made-up block earns nothing,
work on a block that lost an orphan race still earns, and miners on a hostile
fork earn on the main chain for at most the fork's first two blocks.

Every share names the block it builds on: the previous block hash in its
Bitcoin Cash header, here called the share's **BCH prev**. The design has
three parts:

- **Payout rule.** A coinbase pays a share in its window only if that share's
  BCH prev is on the paying share's chain (the chain ending at the paying
  share's own BCH prev) or one block off it. Every node rebuilds every
  coinbase, so this is a share validity rule.
- **Carried headers.** A share carries the Bitcoin Cash headers a receiver
  needs to place its BCH prev: none when it builds on the same block as its
  parent, one after a new block, and the path back to the fork point after a
  reorg. They travel beside the share header, and the proof of work already
  commits to them through the BCH prev.
- **Known BCH prev.** A node accepts a share only once it knows the header of
  the share's BCH prev; until then it waits for the headers. A share on a
  made-up block never gets a header, so no node accepts it.

| BCH prev, relative to the paying chain | Paid | Examples |
|---|---|---|
| On it | yes | ordinary shares |
| One block off it | yes | a block that lost an orphan race; a hostile fork's first block |
| Two or more blocks off it | no | a hostile fork's later blocks |
| Unknown | never in a window | the node never accepts the share |

Eligibility is measured from the paying share's BCH prev, never from a node's
own BCHN tip, so every node that knows the same headers rebuilds the same
coinbase. An ineligible share keeps its place in the window and earns
nothing. An ineligible uncle earns nothing and its nephew no bonus for it.

On Bitcoin Cash the paying share's BCH prev also sizes the window. The window
holds three expected blocks' worth of work
([PPLNS accounting](pplns-accounting.md)), and one block's worth is the
difficulty Bitcoin Cash requires of the block after the paying share's BCH
prev, which every node computes from that known header. Before this rule it
was the nBits the paying share's header claims, which only a found block has
to get right. The test networks' minimum-difficulty rule is left out, so a late
chipnet block does not shrink the window.

A share whose window pays no one, because none of its window's work is on its
BCH chain or one block off it, is invalid: it builds on an old or foreign
block, and anyone can make one for one share's proof of work. When this
node's own template would pay no one, the job builder withdraws the work, as
for any payout it cannot build, and tries again with the next template. Only
the share chain's first share, built from the network's first block, builds
on the all-zero hash; any other share that does is invalid.

That first share, genesis, mines nothing and pays a NUMS key no one can
spend. A window pays it only when it pays no other share or uncle
(`genesis-last-resort`): the payout of the share built right on it, and of a
window with no other work on the paying chain. So no mined work's payout is
ever burned to that key.

## Why not reject stale work instead

Tying share validity to the local node's tip fails: nodes see new blocks at
different times and their share chains would split; a reorg would invalidate
honest shares mined on the losing block; a share mined just before a new
block looks the same as one mined on an old block on purpose; and accepting
shares within a few blocks of the tip pays a hostile fork for those blocks.
The payout rule moves the decision from accepting a share to paying it, so all
nodes hold the same shares.

## What Bitcoin Cash needs

Each node checks carried headers itself, without asking BCHN:

- **The header links** to a header the node already knows.
- **Its hash meets its bits, and its bits are exactly what Bitcoin Cash
  requires.** Upstream checks Bitcoin's 2,016-block retarget with
  rust-bitcoin. Bitcoin Cash sets every block's bits with aserti3-2d, from a
  fixed anchor block and the parent's height and timestamp, with a two-day
  half-life on mainnet
  ([specification](https://upgradespecs.bitcoincashnode.org/2020-11-15-asert/)).
  On chipnet and the other test networks, a block whose time is more than
  twice its interval after its parent's (20 minutes for a ten-minute block,
  2 for a one-minute block) uses the minimum difficulty. This fork checks it
  with its own Rust aserti3-2d validator and each network's anchor, the same
  integer formula the share chain uses for its own targets
  ([Share difficulty: ASERT](../difficulty_adjustment/asert.md)). Scalenet
  has no anchor here, so its headers are checked for links, proof of work and
  time only, and its windows are sized by the claimed nBits.
- **Its timestamp** is at most two hours ahead of the node's clock, as BCHN
  requires (`MAX_FUTURE_BLOCK_TIME` in BCHN's `src/chain.h`). Under
  aserti3-2d every block's bits depend on timestamps, so without this limit a
  fake branch could pick times that ease its bits.

A header that passes cost a full block of real proof of work. Upstream's
design keeps about 8,400 headers; this fork's node keeps about 58 days,
counted in ticks (above), and adds the carried headers that pass.

BCHN's finalization helps but does not replace the rule. By default BCHN
refuses a reorg deeper than 6,000 ticks (`-maxreorgticks`, 10 ten-minute or
100 one-minute blocks), which limits how long a hostile fork can stay
competitive for the node's own BCHN. It says nothing about which block a peer's
share builds on.

## One-minute blocks

Bitcoin Cash moves to one-minute blocks with CHIP-2025-03: chipnet on
2026-11-15 (block 332,450 is its last ten-minute block), mainnet, testnet,
scalenet and regtest at median time past 1810382400 (2027-05-15 12:00 UTC).
A block's **ticks** are the sum of the target intervals up to it. ASERT
counts ticks from its anchor and gives a one-minute block a target ten times
easier, and the test networks' minimum-difficulty gap is twice the new
block's interval.

Each known header carries its ticks and the interval of a block built on it,
as BCHN keeps them in its block index:

- its ticks are its parent's ticks plus its parent's next interval;
- its next interval is 60 once its own median time past (it and its ten
  ancestors) has reached the network's activation time, else 600. The
  networks' switches are a list of timed switches, each an interval the CHIP
  allows.

A header gets them when it becomes known: from the node's own BCHN, or from a
peer after the header check, from its known parent and ancestors. So every
node that knows a branch gives its blocks the same values, and the nBits it
requires of a header, the nBits that size a window and a token share's ASERT
band follow the branch, also across a reorg onto a branch that switched at
another block. Where fewer than ten ancestors are known, as for the oldest
headers of a new store, the network's pinned activation block answers by
height (chipnet's is 332,450, and the tests check that the median-time-past
rule switches there); mainnet's has no pin yet.

The node's own BCHN reports `ticks`, `target_time` and `next_target_time`
for its blocks. A new store's first walk counts ticks down from the tip's
reported ticks; everywhere else the reports only cross-check P2Pool's own
values, and a disagreement is logged as an error, never followed. The values
live in the store (format 12) rather than being learned from BCHN while
running, so nodes that started at different times, restarted or reorged
cannot require different nBits of one header and split the share chain.

## What it costs

- A hostile fork earns for about two block intervals of its hashrate each time
  it starts.
- Header-only work one block off the paying chain earns, at the cost of a
  full block of proof of work.
- Shares on the second block of a losing two-block orphan race earn nothing;
  such races are rare.
- Payout variance rises during a fork war.

Still run your own BCHN, or a Template Provider you trust, and keep it well
connected ([Known attacks](../known-attacks.md)): your templates choose the
chain your shares and coinbases pay.
