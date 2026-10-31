# Node components

> Ported from upstream P2Poolv2's documentation: `node_components.adoc` in
> [p2poolv2/docs](https://github.com/p2poolv2/docs) at commit `623e4c1`,
> Copyright 2024-2026 P2Poolv2 Developers, MIT or Apache-2.0. Adapted to
> Bitcoin Cash by this fork; see the [port map](upstream-port-map.md).

A P2Poolv2 node has two parts: a **Stratum server** that miners connect to,
and a **controller** that keeps the share chain, does the share accounting
and talks to the other nodes. On Bitcoin Cash its block templates come from
Bitcoin Cash Node (BCHN), or from a Stratum V2 Template Provider.

```text
   Miner rigs (Stratum V1 or V2)
          |        ^
   shares |        | work
          v        |
  +------------------+    pool shares      +--------------+   pool shares
  |  Stratum server  | ------------------> |  Controller  | <-------------> Other P2Poolv2 nodes
  |                  | <------------------ |              |
  +------------------+  coinbase payouts,  +--------------+
          ^             pool difficulty           |
          | block templates                       v
  BCHN, or a Stratum V2                      Share store
  Template Provider                          (RocksDB)
```

"Shares" are what a rig finds at its own vardiff difficulty, every few
seconds. "Pool shares" are the ones that also meet the pool difficulty, about
one every 10 seconds on the whole share chain. Only pool shares go to the
controller and to other nodes; the rest still count towards each rig's
measured hashrate and vardiff.

## Stratum server

1. Fetches block templates: `getblocktemplate` from BCHN on every ZMQ
   `hashblock` event, on every block BCHN announces on its P2P port when
   `node_p2p_address` is set, and every 10 seconds in between, or templates
   pushed by a Stratum V2 Template Provider (`template_source = "sv2"`).
2. Builds work for the mining devices from the template and from what the
   controller supplies: the coinbase payouts and the pool difficulty. It
   serves Stratum V1 and Stratum V2 (standard and extended channels, and Job
   Declaration).
3. Receives the shares the devices find, checks each against the device's
   difficulty, and hands the ones that meet the pool difficulty to the
   controller. A share that also meets the Bitcoin Cash block target is
   submitted as a block straight away: to BCHN with `submitblock`, or back
   to the Template Provider that supplied its template.

## Controller

1. Accepts pool shares from the Stratum server and from peers.
2. Runs the share accounting: the PPLNS window over the share chain decides
   how much each miner is paid.
3. Builds the Bitcoin Cash coinbase from that accounting, after the
   donation and the operator fee, paying payees past the coinbase's limit
   through a payout tree ([Coinbase payouts](payouts/coinbase.md)).
4. Gives the Stratum server the current pool difficulty and coinbase.
5. Validates every share a peer sends: its proof of work against the pool
   difficulty, and its coinbase against the payouts the share chain implies
   ([Share validation](sharechain/validation.md)).
6. Relays valid shares, and signed share transactions, to its peers.
7. Keeps the payout tree of each block the pool finds and broadcasts it to
   BCHN once the coinbase has 100 confirmations
   (`node/payout_tree_executor.rs`).

## In the source

Both parts live in one crate, `p2poolv2_lib`: the Stratum server under
`stratum/`, the controller under `node/`, `shares/`, `store/` and
`accounting/`. The `p2poolv2_node` binary starts them together with the API
and dashboard (`p2poolv2_api`). The internal flow, from a stratum submit to
a stored and relayed share, is in
[Share processing pipeline](architecture/share-processing-pipeline.md) and
[Async flow](architecture/async-flow.md).
