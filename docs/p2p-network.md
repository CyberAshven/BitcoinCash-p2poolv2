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
messages, `weak-by-hash/without-payouts/headers-resume` today
(`network_prologue` in
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

A new Bitcoin Cash share whose coinbase outputs take at least 50,000 bytes,
about 1,400 payees, is relayed without them (`ShareWithoutPayouts`); a
smaller one goes whole. Every node rebuilds the outputs from its own PPLNS
window at the share's parent to validate the share anyway, so the receiver
rebuilds them first and goes on only with a coinbase whose txid is the one
the header's coinbase proof gives. A share it cannot rebuild that way, its parent or window not
here yet, is fetched whole, never refused. Sync and block requests still
send shares whole.

Bitcoin Cash has no P2Poolv2 DNS seeds yet. A new node finds its first
peers in `[network] dial_peers` or through Nostr (below). A peer in
`dial_peers` is given by IP address (`/ip4/`, `/ip6/`) or
host name (`/dns4/`, `/dns6/`, `/dns/`), and listens on port 6884 by default
([Requirements](requirements.md#ports)). The transport resolves a host name
through the system's DNS configuration on every dial, reconnects included.
Only host names listed in `dial_peers` are dialed, never a `/dns` address
learned from a peer, and `blocked_ips` is checked after a name resolves.

A node keeps its peer ID across restarts: its libp2p key is made on the
first start and kept in `p2p_node_key` beside the store
(`[network] node_key_path`). Peers and announcements keep finding it under
the same peer ID. With `ephemeral_node_key = true` a node gets a new peer
ID every start instead, which nothing links across restarts. The key and the peer
book (below) belong to one running node: a node holds `p2p_node_key.lock`
and `p2p_peers.json.lock` while it runs, so a second node whose store sits in
the same directory stops at start rather than taking the same peer ID, which
would keep the two from ever connecting. Give each node its own directory.

### Finding peers through Nostr

Nodes that say they are reachable announce themselves on public Nostr
relays, and a node short of peers asks those relays for them. No single
party holds this list: the relays are run by different operators, a node
asks all of its relays at once, and a relay can drop an announcement but
cannot forge one.

- **Who announces:** a node with an `external_address` (its operator says
  it is reachable) and `nostr_announce` on, the default. It publishes that
  address, ending in its peer ID, every hour.
- **What is published:** one NIP-78 app-data event (kind 30078) per node and
  network. It is signed with a key derived from the node key, so there is
  no second key file. Its `d` tag is `p2poolv2/<chain>-<network>/<hash>`,
  where the hash is of the node's network protocol string; every
  share-chain rule change alters it, so a node only finds peers on its own
  rules. It also carries a NIP-40 `expiration` three hours ahead, a NIP-13
  proof of work of 16 bits on its id, and a NIP-31 `alt` text for Nostr
  clients that show it.
- **Who looks:** a node with fewer than 8 peers asks every relay when DNS
  seeds would be asked (at once without `dial_peers`, otherwise after 10
  seconds), then every 10 minutes while still short.
- **What a reader checks:** the id, signature, proof of work, tag and age
  of every event. It keeps the newest announcement per author, skips itself
  and the peers it has, and dials at most 8 routable addresses. Announced
  addresses are only candidates: the noise handshake with the network
  protocol prologue still decides whether a peer is on our network. On
  regtest, loopback and private addresses are dialed too.
- **Relays:** `[network] nostr_relays`, by default six public relays run by
  different operators. An empty list keeps the node off Nostr.

Announcing publishes the node's address and its being a P2Pool node, which
`external_address` already offers to anyone who dials it. A node that only
looks reveals to the relays it asks that it is a P2Pool node on that
network.

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
| 32 | 5.53% | 4.91% |
| 160 | 9.99% | 4.91% |
| 320 | 14.99% | 4.91% |

The [network simulation](simulation/network-sim.md) gave these at
10-second shares and one-minute Bitcoin Cash blocks (`cargo netsim run
--nodes 1000 --duration 10800 --weak-shares N --weak-share-carriage whole`,
or `gossip-only`; the runs are recorded with the weak share proposal, #117).
With ten-minute blocks they were 5.27%, 8.66% and 14.11% against 4.20%: a
BCH block now crosses the links every minute, which adds stale shares
either way, and carrying the weak shares whole adds about what it did, 0.6,
5.1 and 10.1 points (1.1, 4.5 and 9.9). Listing a share's weak shares
by hash costs it 32 bytes each, against about 800 whole. On a 12-node swarm
listing alone cut a relayed share message from 23.4 KB to 4.7 KB, but the
2.4% of weak shares a receiver then asked for added a round trip and raised
the shares that did not make the chain from 9.69% to 12.37%; the 400 ms of
prefilled weak shares are there to take that round trip out.

## Outgoing peers

A node keeps at least `[network] target_outgoing_peers` outgoing peers, 16
by default. An outgoing peer is a connected peer this node dialed and
keeps: a `dial_peers` entry, one dialed for the target, or one a DNS or
Nostr seed returned. While the node has fewer, it dials one more peer a
second (`p2poolv2_lib/src/node/outgoing_peer_target.rs`):

- **Peers it has heard of.** The node keeps a book of up to 1,000 peers and
  their addresses, from Kademlia's routing updates and the listen addresses
  each peer reports through Identify. A peer reached through a DNS seed
  (Bitcoin only) enters it the same way once connected. Kademlia's own
  table cannot serve: the node removes a peer from it on disconnect, unless
  the node closed that peer's lookup connection itself, and when a dial to a
  peer it has no connection to fails. Only
  globally routable IPv4 and IPv6 addresses with a TCP port are kept, never
  a `/dns` name, a loopback or a private address, so nodes on a LAN or on
  one machine find each other through `dial_peers`. A peer the node has
  reached over an outgoing connection is verified, and a full book drops a
  random unverified peer for a new one.
- **Spread over network groups.** The target picks a network group at
  random among those with a candidate, then a peer in it. It never dials a
  peer in an IPv4 /16 or IPv6 /32 where the node already has an outgoing
  peer, as Bitcoin Cash Node does for its automatic peers
  (`CConnman::ThreadOpenConnections` and `CNetAddr::GetGroup`). Connections
  to `dial_peers` hold no group, and the target leaves those peers to the
  reconnector.
- **Lookups close.** Kademlia dials each unconnected peer a lookup asks, and
  share relay kept those connections open, so lookups, not the target,
  picked a node's peers. An outgoing connection none of the node's own dials
  opened is Kademlia's: it holds no group, does not count, and closes after
  10 seconds once no query runs, or after 60, Kademlia's query timeout, as
  Bitcoin Cash Node's feeler connections close. Like a feeler, a lookup only
  discovers addresses: the node sends no handshake on it and answers the
  peer's without a sync, and shares, weak shares and transactions go only to
  peers that sent a handshake, so the lookup's peer never gets them and
  closing the lookup cuts nothing in flight. The peer stays in the routing
  table and the book; a connection that is not a lookup, opened while the
  lookup is, makes it a share-chain peer. A connection to an anchor is the
  target's; inbound connections stay; `target_outgoing_peers = 0` keeps
  lookups open as full peers.
- **Gone peers are forgotten.** Kademlia keeps an unreachable peer at its
  last address, so a node that left, or came back under another peer ID,
  would be handed to every lookup and dialed again by every node. When a
  dial to a peer the node has no connection to fails, the node removes the
  peer from the routing table; a live one is learned again from the next
  lookup that finds it.
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
4.46 seconds on 16 peers per node against 5.30 seconds on 8, within
today's stale rate, and at 10-second shares stale shares fall from 2.30%
to 2.00%. With whole shares more peers alone made propagation slower
([Network simulation](simulation/network-sim.md), "Where each idea reaches
the limit"). The simulation counts each node's links in both directions:
when every node dials 16, the average node has about 32.

Each outgoing peer is another node's inbound peer. When every node keeps
16, a node takes 16 inbound peers on average if every node accepts inbound
connections, 32 if half do and 64 if a quarter do, and a node alone in its
/16 takes more than one in a crowded /16, since the target picks a group
before a peer. The sample configs set `max_established_incoming` to 128:
with 1,000 nodes, a quarter of them accepting inbound connections and 30%
of those in ten cloud /16s, the busiest node takes about 110.

The book of known peers survives a restart. It is written to
`p2p_peers.json` beside the store every ten minutes and when the node
stops, and read on the next start. Peers keep whether the node reached
them. The peers held over outgoing connections when it was saved are
anchors: dialed before any other known peer on the next start, once, so a
restarted node is back among its peers before anyone can fill its slots, as
Bitcoin Core's `anchors.dat` does. They still pass the rules above. A
missing or broken file means an empty book; a node with
`ephemeral_node_key` keeps none, since redialing its anchors would link its
restarts.

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
