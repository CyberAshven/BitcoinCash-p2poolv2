# Node components

> Ported from upstream P2Poolv2's documentation: `node_components.adoc` in
> [p2poolv2/docs](https://github.com/p2poolv2/docs) at commit `623e4c1`,
> Copyright 2024-2026 P2Poolv2 Developers, MIT or Apache-2.0. Adapted to
> Bitcoin Cash by this fork; see the [port map](upstream-port-map.md).

A P2Poolv2 node has two parts: a **Stratum server** that miners connect to,
and a **controller** that keeps the share chain, does the share accounting
and talks to the other nodes. Block templates come from Bitcoin Cash Node
(BCHN) or a Stratum V2 Template Provider.

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
seconds. "Pool shares" also meet the pool difficulty, about one every 10
seconds on the whole share chain; only they go to the controller and to
other nodes.

## Stratum server

1. Fetches templates: `getblocktemplate` from BCHN on every ZMQ `hashblock`
   event (and on BCHN's P2P block announcements with `node_p2p_address`)
   and every 10 seconds, or from a Template Provider
   (`template_source = "sv2"`). See [Stratum server and vardiff](stratum.md).
2. Builds work from the template and the controller's coinbase and pool
   difficulty, over Stratum V1 and V2 (channels and Job Declaration).
3. Hands shares that meet the pool difficulty to the controller, and sends a
   share that meets the Bitcoin Cash target as a block at once: `submitblock`
   to BCHN, or back to the Template Provider.

## Controller

1. Validates pool shares from the Stratum server and from peers: proof of
   work, and the coinbase against the payouts the share chain implies
   ([Share validation](sharechain/validation.md)).
2. Builds the coinbase from the PPLNS accounting, after the donation and the
   operator fee, with a payout tree past the coinbase's limit
   ([Coinbase payouts](payouts/coinbase.md)), and gives it and the pool
   difficulty to the Stratum server.
3. Relays valid shares, and signed share transactions, to its peers.
4. Broadcasts each found block's payout tree once the coinbase has 100
   confirmations (`node/payout_tree_executor.rs`).

## In the source

Both parts live in `p2poolv2_lib`: the Stratum server under `stratum/`, the
controller under `node/`, `shares/`, `store/` and `accounting/`. The
`p2poolv2_node` binary starts them with the API and dashboard
(`p2poolv2_api`). The flow from a stratum submit to a stored and relayed
share: [Share processing pipeline](architecture/share-processing-pipeline.md)
and [Async flow](architecture/async-flow.md).
