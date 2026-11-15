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

## One share chain

On Bitcoin Cash there is one share chain per network. Its target ceiling is
the former mini chain's, difficulty about 4.55, for small miners; see
[Small miners](small-miners.md). The main and mini chains merged on
2026-11-09 ([Share chain redesign](sharechain/small-miners-design.md#1-one-share-chain)).
Mainnet and chipnet run the same rules; only the network setting differs.

Nodes only talk to nodes on the same chain and rules. Every libp2p protocol
string carries the Bitcoin Cash network, the share chain's genesis and, on
Bitcoin Cash, a tag for each share-chain rule and last a tag for the P2P
messages, `weak-by-hash` today (`protocol_string` in
`p2poolv2_lib/src/node/behaviour/request_response.rs`). A node on another
chain or rule fails the handshake instead of exchanging shares the other
side would reject, and a node speaking other messages fails it instead of
reading the other's messages as malformed.

## Gossip

Nodes relay the shares that meet the pool difficulty, and every node that
receives one validates it before relaying it further (see
[Share validation](sharechain/validation.md)). The network runs on libp2p:
Kademlia for peer discovery, Identify, Ping, and a request-response protocol
for share announcements, headers, blocks and share transactions.

Bitcoin Cash has no P2Poolv2 DNS seeds yet: a new node needs at least one
peer in `[network] dial_peers`, given by IP address (`/ip4/`, `/ip6/`) or
host name (`/dns4/`, `/dns6/`, `/dns/`), and listens on port 6884 by default
([Requirements](requirements.md#ports)). The transport resolves a host name
through the system's DNS configuration on every dial, reconnects included.

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

On Bitcoin Cash a share also carries weak shares, which travel ahead of it
on their own: a node pushes the ones its miners find to every peer, and
announces the ones it takes from peers by hash, each fetched once. So a node
relays a new share with its weak shares listed by proof-of-work hash
(`ShareWithWeakHashes`), not whole, except the ones still spreading: those
its weak pool took less than 400 ms ago, or does not hold, go whole beside
the list (prefilled, as in BIP152), so a peer rarely has to ask. The
receiver fills the list from its weak pool and the prefilled ones and asks
the sender for any it still lacks (`GetCarriedWeakShares`), then checks the
rebuilt share exactly as a share sent whole, weak root included. A share it
cannot fill within ten seconds, or whose weak shares do not rebuild the weak
root, is not complete yet, never invalid: it is fetched whole, from another
peer that relayed it if one did. A peer catching up still gets old shares
with their weak shares whole: it was never sent them on their own.

| Weak shares a share | Carried whole: stale shares on 1,000 nodes | Gossiped only |
|---:|---:|---:|
| 32 | 5.27% | 4.20% |
| 160 | 8.66% | 4.20% |
| 320 | 14.11% | 4.20% |

The [network simulation](simulation/network-sim.md) gave these at
10-second shares (`cargo netsim run --nodes 1000 --duration 10800
--weak-shares N --weak-share-carriage whole`, or `gossip-only`; the runs are
recorded with the weak share proposal, #117). Listing a share's weak shares
by hash costs it 32 bytes each, against about 800 whole. On a 12-node swarm
listing alone cut a relayed share message from 23.4 KB to 4.7 KB, but the
2.4% of weak shares a receiver then asked for added a round trip and raised
the shares that did not make the chain from 9.69% to 12.37%; the 400 ms of
prefilled weak shares are there to take that round trip out.

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
