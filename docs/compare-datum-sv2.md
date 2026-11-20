# Comparison with SV2 and DATUM

> Ported from upstream P2Poolv2's documentation: `compare_datum_sv2.adoc` in
> [p2poolv2/docs](https://github.com/p2poolv2/docs) at commit `623e4c1`,
> Copyright 2024-2026 P2Poolv2 Developers, MIT or Apache-2.0. Adapted to
> Bitcoin Cash by this fork; see the [port map](upstream-port-map.md).

Stratum V2 with Job Declaration, and OCEAN's DATUM (a Bitcoin protocol), both
let miners build their own block templates, but leave the accounting and the
payouts with a pool operator.

| | DATUM (OCEAN) | SV2 Job Declaration pool | P2Poolv2 on Bitcoin Cash |
|---|---|---|---|
| Who builds block templates | the miner, through the DATUM Gateway, with the coinbase outputs the operator sends | the miner, around the pool's coinbase output, then declares the job | the miner, from its own BCHN or a Template Provider it chose |
| Who keeps the share accounting | the pool operator | the pool operator | every node, through the share chain |
| How miners are paid | coinbase outputs the pool sets; small balances held by the pool | from the pool's wallet (custodial) | coinbase outputs every node checks; a share's payout right can be sold for BCH |
| Can the pool reject work for its transactions? | yes, it validates every share | yes in Full-Template mode; no in Coinbase-Only mode | there is no operator; leaving a transaction out of every block would take most of the share chain's hashrate |
| Can the pool withhold payment? | yes, by leaving a miner out of later coinbases | yes, it holds the balances | no, payouts follow the share chain |
| Permission to mine | granted by the operator | granted by the operator | none; anyone can join the peer-to-peer network |

In a central pool a miner needs the operator's permission to join, to get
work and to be paid, and many operators are companies that must follow the
rules where they operate. That gives a regulator two levers.

## The operator controls the payouts

The operator decides the payout split, so it can be made to stop paying a
miner whose blocks include transactions a regulator wants suppressed:

```text
regulator -> operator: suppress transactions involving address X
miner A: builds a template with a transaction paying X; submits shares
miner B: builds a template without X; submits shares
operator: computes payouts and leaves miner A out
  DATUM:   the next coinbase outputs leave A out
  SV2 JD:  A's balance is withheld; B is paid from the pool's wallet
           (in Coinbase-Only mode the operator sees X only when A finds a block)
miner A: drops the transaction or leaves the pool
```

## The operator can reject work

A DATUM server validates the shares it receives, and an SV2 pool in
Full-Template mode sees the transactions, so either can reject a job. SV2's
Coinbase-Only mode hides them, the most censorship-resistant mode an SV2
pool offers, but the operator must then trust miners about their fees, and a
regulator can make it drop the mode.

```text
regulator -> operator: suppress transactions involving address X
miner A: builds a template with a transaction paying X
  DATUM or SV2 Full-Template: transactions visible; operator rejects the job,
                              and A's work earns nothing unless it drops X
  SV2 Coinbase-Only:          no transaction list; operator accepts the work
                              and has to answer to the regulator
```

## P2Poolv2: a share chain instead of an operator

P2Poolv2 keeps the accounting on a share chain of weak Bitcoin Cash blocks.
Nakamoto consensus on the share chain brings every node to the same view of
the shares, so each node builds coinbases every other node can check
([Share chain](sharechain/README.md)). Miners are paid in the coinbase of
every block the pool finds, or sell a share's payout right for BCH, without
handing shares or rewards to anyone ([Payouts](payouts/README.md)).

This fork serves Stratum V2 to a node's own miners, Job Declaration included
(plan decisions 2, 3 and 7). The node acts as the SV2 pool and job
declarator, not as an operator's server: a declared job's coinbase must carry
exactly the PPLNS payouts every other node will rebuild, so declaring a job
changes what the block contains, never who is paid. In Full-Template mode
the node checks the declared block with its own BCHN. See
[Job Declaration](bch-job-declaration.md) and plan decision 8
([bch-plan.md](bch-plan.md)), which rules out DATUM-style designs.
