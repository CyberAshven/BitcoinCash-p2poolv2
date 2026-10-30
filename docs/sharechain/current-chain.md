# Mining on the current chain

> Ported from upstream P2Poolv2's documentation:
> `architecture/bitcoin_chain_validation.adoc` in
> [p2poolv2/docs](https://github.com/p2poolv2/docs) at commit `623e4c1`,
> Copyright 2024-2026 P2Poolv2 Developers, MIT or Apache-2.0. Adapted to
> Bitcoin Cash by this fork; see the [port map](../upstream-port-map.md).

**Status on Bitcoin Cash: in progress, not active.** Upstream designed this
defense for Bitcoin; this fork plans it before mainnet
([Requirements](../requirements.md#functional)). Built on 2026-10-30: the
header check (`p2poolv2_lib/src/shares/bch_headers.rs`), which requires
each header's exact aserti3-2d nBits from its known parent, and the store
of known headers (`p2poolv2_lib/src/store/bch_headers.rs`), which a worker
fills from the node's own BCHN (`p2poolv2_lib/src/node/bch_header_feeder.rs`):
every 5 seconds it walks back from BCHN's best block to a known header,
and a new node first learns 8,400 headers, about 58 days. Since the same
day, share messages carry the BCH headers a receiver needs
(`p2poolv2_lib/src/shares/bch_carried_headers.rs`), and the protocol
strings end in `bch-prev-1off`, so nodes from before cannot peer. Not built
yet: storing a share's body only once its BCH prev is known, and the payout
rule. Until all of it
is built, no share-chain rule has changed. This page explains the
attack, the design, and what it needs on Bitcoin Cash. The full design, with
its formal model, is
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
  On chipnet and the other test networks, a block whose time is more than 20
  minutes after its parent's uses the minimum difficulty. The node needs a
  Rust aserti3-2d header validator with each network's anchor; the share
  chain already uses the same integer formula for its own targets
  ([Share difficulty: ASERT](../difficulty_adjustment/asert.md)).
- **Its timestamp** is at most two hours ahead of the node's clock, as BCHN
  requires (`MAX_FUTURE_BLOCK_TIME` in BCHN's `src/chain.h`). Under
  aserti3-2d every block's bits depend on timestamps, so without this limit a
  fake branch could pick times that ease its bits.

A header that passes cost a full block of real proof of work. Upstream's node
keeps about six weeks of headers (roughly 6,048 blocks, which is also six
weeks on Bitcoin Cash), filled from its node's notifications and from carried
headers.

BCHN's finalization helps but does not replace the rule. By default BCHN
refuses a reorg deeper than 10 blocks (`DEFAULT_MAX_REORG_DEPTH` in its
`src/validation.h`), which limits how long a hostile fork can stay
competitive for the node's own BCHN. It says nothing about which block a peer's
share builds on.

## What it costs

- A share-chain rule change, shipped on Bitcoin Cash as a flag day with a new
  protocol tag, like the other rule changes.
- A hostile fork earns for about two block intervals of its hashrate each time
  it starts.
- Header-only work one block off the paying chain earns, at the cost of a
  full block of proof of work.
- Shares on the second block of a losing two-block orphan race earn nothing;
  such races are rare.
- Payout variance rises during a fork war.

Until it is built, run your own BCHN, or a Template Provider you trust, and
keep it well connected ([Known attacks](../known-attacks.md)).
