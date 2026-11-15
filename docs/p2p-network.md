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
peer in `[network] dial_peers`, given by IP address (`/ip4/`, `/ip6/`) or
host name (`/dns4/`, `/dns6/`, `/dns/`), and listens on port 6884 by default
([Requirements](requirements.md#ports)). The transport resolves a host name
through the system's DNS configuration on every dial, reconnects included.
Only host names listed in `dial_peers` are dialed, never a `/dns` address
learned from a peer, and `blocked_ips` is checked after a name resolves.

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

## Outgoing peers

A node keeps at least `[network] target_outgoing_peers` outgoing peers, 16
by default. An outgoing peer is a connected peer this node dialed: a
`dial_peers` entry, a peer Kademlia reached in a query, or one dialed for
the target. While the node has fewer, it dials one more peer a second
(`p2poolv2_lib/src/node/outgoing_peer_target.rs`):

- **Peers it has heard of.** The node keeps a book of up to 1,000 peers and
  their addresses, from Kademlia's routing updates and the listen addresses
  each peer reports through Identify. A peer reached through a DNS seed
  (Bitcoin only) enters it the same way once connected. Kademlia's own
  table cannot serve: the node removes a peer from it on disconnect. Only
  globally routable IPv4 and IPv6 addresses with a TCP port are kept, never
  a `/dns` name, a loopback or a private address, so nodes on a LAN or on
  one machine find each other through `dial_peers`. A peer the node has
  reached over an outgoing connection is verified, and a full book drops a
  random unverified peer for a new one.
- **Spread over network groups.** The target picks a network group at
  random among those with a candidate, then a peer in it. It never dials a
  peer in an IPv4 /16 or IPv6 /32 where the node already has an outgoing
  connection, as Bitcoin Cash Node does for its automatic peers
  (`CConnman::ThreadOpenConnections` and `CNetAddr::GetGroup`). Connections
  to `dial_peers` hold no group, and the target leaves those peers to the
  reconnector.
- **Within the limits.** Outgoing dials in flight count toward the target,
  and no dial takes the node past `max_established_outgoing`. The target's
  own dials in flight stay below half of `max_pending_outgoing`, so
  `dial_peers` and Kademlia keep the other half. Blocked IPs are never
  dialed.
- **With backoff.** A peer the target dialed is not dialed again for 60
  seconds, doubled after each failure. A failure is a dial that fails or a
  connection that closes within 60 seconds, as a full peer's does. A
  connection that lasts resets the backoff, and five failures in a row drop
  the peer from the book.

P2Pool v1 kept 6 outgoing peers by default (`--outgoing-conns`), one per
IPv4 /16, and Bitcoin Cash Node keeps 8. The share chain's P2P network runs
on TCP over IPv4 and IPv6 only; it has no onion or I2P addresses. The
target changes how many connections a node keeps, not which peers a share
is relayed to or how. `/metrics` reports `p2p_outgoing_peers`,
`p2p_incoming_peers` and `p2p_outgoing_target_dials_total`
([Metrics](architecture/metrics.md)).

Why 16: in the network simulation, 1,000 nodes that send shares without
their payouts and relay them after the header check carry a share every
4.21 seconds on 16 peers per node against 5.01 seconds on 8, within
today's stale rate, and at 10-second shares stale shares fall from 2.32%
to 1.99%. With whole shares, as relayed today, more peers alone made
propagation slower ([Network simulation](simulation/network-sim.md),
"Where each idea reaches the limit"). The simulation counts each node's
links in both directions: when every node dials 16, the average node has
about 32.

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
