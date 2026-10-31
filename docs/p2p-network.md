# P2P network of miners

> Ported from upstream P2Poolv2's documentation: `p2p_network_of_miners.adoc`
> in [p2poolv2/docs](https://github.com/p2poolv2/docs) at commit `623e4c1`,
> Copyright 2024-2026 P2Poolv2 Developers, MIT or Apache-2.0. Adapted to
> Bitcoin Cash by this fork; see the [port map](upstream-port-map.md).

P2Poolv2 is a peer-to-peer network in which every peer is a P2Poolv2 mining
node. Each node serves one or more mining rigs on its own Stratum ports and
is their gateway to the network.

## Two difficulties

- **Each rig's own difficulty (vardiff).** The node's Stratum server sends
  work to its rigs and adjusts each rig's difficulty so that it submits about
  one share every 3.3 seconds, whatever its hashrate. That rate lets the node
  measure each rig's hashrate closely. See
  [Stratum server and vardiff](stratum.md).
- **The pool difficulty.** Only shares that also meet the share chain's
  target enter the share chain. The target is set by ASERT, the integer
  algorithm Bitcoin Cash uses for its own blocks (aserti3-2d), tuned to one
  share every 10 seconds on each share chain. See
  [Share difficulty: ASERT](difficulty_adjustment/asert.md).

## Main and mini share chains

On Bitcoin Cash there are two share chains, each with its own genesis share.
The mini chain's target ceiling is 256 times easier (difficulty about 4.55
against about 1,164 on main), for small miners; see
[Small miners](small-miners.md). A node mines one of them
(`bch_share_chain` in its config). Mainnet and chipnet run the same rules;
only the network setting differs.

Nodes only talk to nodes on the same chain and rules. Every libp2p protocol
string carries the Bitcoin Cash network, the share chain's genesis and, on
Bitcoin Cash, a tag for each share-chain rule (`protocol_string` in
`p2poolv2_lib/src/node/behaviour/request_response.rs`). A node on another
chain or rule fails the handshake instead of exchanging shares the other
side would reject.

## Gossip

Nodes relay the shares that meet the pool difficulty, and every node that
receives one validates it before relaying it further (see
[Share validation](sharechain/validation.md)). The network runs on libp2p:
Kademlia for peer discovery, Identify, Ping, and a request-response protocol
for share announcements, headers, blocks and share transactions.

Bitcoin Cash has no P2Poolv2 DNS seeds yet: a new node needs at least one
peer in `[network] dial_peers`, and listens on port 6884 by default
([Requirements](requirements.md#ports)).

Take seven nodes, A to G, connected in a mesh. Several links carry shares
at once, because more than one node can find a share at the same time:

```text
Node A finds share a1 and sends it to Node B and Node D
Node B finds share b1 and sends it to Node C and Node E
Node G finds share g1 and sends it to Node D, Node E and Node F
Links idle at this moment: C-E, C-F, D-E, E-F
```

Each receiving node validates the share and passes it on to its own peers
that have not seen it, so every share reaches every node.

## One view of the shares

Every node keeps a replica of the share chain, and the nodes agree on it by
Nakamoto consensus on the share chain: the chain with the most work wins.
Through that agreement every node knows how many shares each miner found,
and at which difficulty.

When a share arrives, the receiving node rebuilds the Bitcoin Cash coinbase
the share should carry from its own copy of the PPLNS window and compares it
with the coinbase the share actually carries. A share that pays anyone a
different amount is invalid. See [PPLNS accounting](sharechain/pplns-accounting.md)
for how the window is counted on Bitcoin Cash.
