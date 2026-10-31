# Comparison with SV2 and DATUM

> Ported from upstream P2Poolv2's documentation: `compare_datum_sv2.adoc` in
> [p2poolv2/docs](https://github.com/p2poolv2/docs) at commit `623e4c1`,
> Copyright 2024-2026 P2Poolv2 Developers, MIT or Apache-2.0. Adapted to
> Bitcoin Cash by this fork; see the [port map](upstream-port-map.md).

Stratum V2 with Job Declaration, and OCEAN's DATUM, both let a pool operator
hand transaction selection to its miners: miners build their own block
templates. Both still leave the accounting and the payouts with the operator.
This page compares the designs; it does not survey which pools mine Bitcoin
Cash. DATUM is OCEAN's protocol for Bitcoin. Stratum V2 is chain-neutral, and
this fork speaks it itself (see the end of this page).

## At a glance

| | DATUM (OCEAN) | SV2 Job Declaration pool | P2Poolv2 on Bitcoin Cash |
|---|---|---|---|
| Who builds block templates | the miner, through the DATUM Gateway | the miner | the miner, from its own BCHN or a Template Provider it chose |
| Who keeps the share accounting | the pool operator | the pool operator | every node, through the share chain |
| How miners are paid | coinbase outputs the pool sets; small balances held by the pool | from the pool's wallet (custodial) | coinbase outputs every node checks; a share's payout right can be sold for BCH |
| Can the pool reject work for its transactions? | yes, it validates every share | yes in Full-Template mode; no in Coinbase-Only mode | there is no operator; leaving a transaction out of every block would take most of the share chain's hashrate |
| Can the pool withhold payment? | yes, by leaving a miner out of later coinbases | yes, it holds the balances | no, payouts follow the share chain |
| Permission to mine | granted by the operator | granted by the operator | none; anyone can join the peer-to-peer network |

## Miners build templates

With DATUM, the operator sends each miner the coinbase outputs (the payout
split, miners included) and the miner builds the template from its own node.
With SV2 Job Declaration, the operator sends its own coinbase output (the
reward goes to the pool's wallet), the miner builds a template from its own
node, declares the job, and the operator accepts it.

```text
DATUM:    operator -> miner: coinbase outputs (the payout split)
          miner: builds a template from its own node
SV2 JD:   operator -> miner: the pool's coinbase output
          miner: builds a template, declares the job; operator accepts it
Both:     miner -> operator: shares; operator validates and credits them
```

## A central operator means permissioned mining

A miner in a central pool needs the operator's permission to join, to get
work and to be paid. Many operators are companies that must follow the rules
where they operate. P2Poolv2 aims to stay permissionless: anyone can join the
peer-to-peer network and mine, with accounting and payouts anyone can audit.

## The operator controls the payouts

The DATUM server tells each gateway which coinbase outputs to include, and an
SV2 Job Declaration pool pays from its own wallet. Either way the operator
decides the payout split, which gives it a lever over its miners' transaction
selection. An operator under pressure from a regulator can be made to stop
paying any miner whose blocks include transactions it wants suppressed:

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

A DATUM server validates the shares it receives, so it decides which
templates count as work. In SV2 Job Declaration's Full-Template mode the
operator sees the transactions and can reject a job outright. In Coinbase-Only
mode the miner does not reveal its transactions, so the operator cannot reject
the work for them. That is the most censorship-resistant mode an SV2 pool
offers, but the operator must trust miners about their fees, and a regulator
can make it drop the mode.

```text
regulator -> operator: suppress transactions involving address X
miner A: builds a template with a transaction paying X
  DATUM or SV2 Full-Template: transactions visible; operator rejects the job,
                              and A's work earns nothing unless it drops X
  SV2 Coinbase-Only:          no transaction list; operator accepts the work
                              and has to answer to the regulator
```

## P2Poolv2 keeps the accounting on a share chain

P2Poolv2 keeps the accounting on a share chain of weak Bitcoin Cash blocks.
Nakamoto consensus on the share chain brings every node to the same view of
the shares, so each node builds coinbases that every other node can check,
continuously and independently. See [Share chain](sharechain/README.md).

## P2Poolv2 pays without custody

Miners are paid in the coinbase of every block the pool finds, and can sell a
share's payout right for BCH through a contract on each chain. They never hand
their shares or rewards to an operator. See [Payouts](payouts/README.md).

## This fork and Stratum V2 Job Declaration

This fork uses Stratum V2 between a node and its own miners, Job Declaration
included (plan decisions 2, 3 and 7). The node acts as the SV2 pool and job
declarator, but it is not an operator's server: a declared job's coinbase must
carry exactly the PPLNS payouts every other node will rebuild, so declaring a
job changes what the block contains, never who is paid. In Full-Template mode
the node checks the declared block with its own BCHN; in Coinbase-Only mode it
cannot see the transactions, as above. See
[Job Declaration](bch-job-declaration.md) and plan decision 8
([bch-plan.md](bch-plan.md)), which rules out DATUM-style designs.
