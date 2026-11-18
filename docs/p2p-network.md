# P2P network of miners

> Ported from upstream P2Poolv2's documentation: `p2p_network_of_miners.adoc`
> in [p2poolv2/docs](https://github.com/p2poolv2/docs) at commit `623e4c1`,
> Copyright 2024-2026 P2Poolv2 Developers, MIT or Apache-2.0. Adapted to
> Bitcoin Cash by this fork; see the [port map](upstream-port-map.md).

P2Poolv2 is a peer-to-peer network in which every peer is a P2Poolv2 mining
node. Each node serves one or more mining rigs on its own Stratum ports and
is their gateway to the network.

## Two difficulties

- **Each rig's own difficulty (vardiff).** The node's Stratum server adjusts
  each rig's difficulty so that it submits about one share every 3.3
  seconds, whatever its hashrate, which lets the node measure each rig's
  hashrate closely. See [Stratum server and vardiff](stratum.md).
- **The pool difficulty.** Only shares that also meet the share chain's
  target enter the share chain. ASERT (aserti3-2d, Bitcoin Cash's own block
  algorithm) sets it for one share every 10 seconds. See
  [Share difficulty: ASERT](difficulty_adjustment/asert.md).

## One share chain

Bitcoin Cash has one share chain per network, with the former mini chain's
target ceiling (difficulty about 4.55) for small miners; see
[Small miners](small-miners.md) and
[Share chain redesign](sharechain/small-miners-design.md#1-one-share-chain).
Mainnet and chipnet run the same rules; only the network setting differs.

Nodes only talk to nodes on the same chain and rules. Every libp2p protocol
string carries the network, the share chain's genesis, a tag for each
share-chain rule and last a tag for the P2P messages,
`weak-by-hash/without-payouts` today (`protocol_string` in
`p2poolv2_lib/src/node/behaviour/request_response.rs`). A node on other
rules or messages fails the handshake instead of exchanging shares or
messages the other side cannot use.

## Gossip

Nodes relay the shares that meet the pool difficulty, and every node
validates a share before relaying it further (see
[Share validation](sharechain/validation.md)). The network runs on libp2p:
Kademlia for peer discovery, Identify, Ping, and a request-response protocol
for share announcements, headers, blocks and share transactions.

A new share whose coinbase outputs take at least 50,000 bytes, about 1,400
payees, is relayed without them (`ShareWithoutPayouts`). The receiver
rebuilds them from its own PPLNS window, as validation does anyway, and goes
on only with a coinbase whose txid matches the header's coinbase proof. A
share it cannot rebuild yet (parent or window missing) is fetched whole,
never refused. Sync and block requests send shares whole.

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

### Weak shares by hash

On Bitcoin Cash a share also carries weak shares, which travel ahead of it on
their own: a node pushes the ones its miners find to every peer, and
announces the ones it takes from peers by hash, each fetched once. A new
share is relayed with its weak shares listed by proof-of-work hash
(`ShareWithWeakHashes`), 32 bytes each against about 800 whole. Those still
spreading (in the weak pool less than 400 ms, or not held) go whole beside
the list, prefilled as in BIP152. The receiver fills the list from its weak
pool, asks the sender for any it lacks (`GetCarriedWeakShares`), and checks
the rebuilt share as one sent whole, weak root included. A share it cannot
fill within ten seconds, or whose weak shares do not rebuild the weak root,
is fetched whole, from another peer if one relayed it; it is never invalid
for that. A peer catching up gets old shares with their weak shares whole.

Stale shares on 1,000 nodes at 10-second shares, from the
[network simulation](simulation/network-sim.md) (`cargo netsim run --nodes
1000 --duration 10800 --weak-shares N --weak-share-carriage whole`, or
`gossip-only`; the runs are recorded with the weak share proposal, #117):

| Weak shares a share | Carried whole, 1-minute blocks | Gossiped only, 1-minute blocks | Carried whole, 10-minute blocks | Gossiped only, 10-minute blocks |
|---:|---:|---:|---:|---:|
| 32 | 5.53% | 4.91% | 5.27% | 4.20% |
| 160 | 9.99% | 4.91% | 8.66% | 4.20% |
| 320 | 14.99% | 4.91% | 14.11% | 4.20% |

The prefill is there because listing alone, on a 12-node swarm, cut a
relayed share message from 23.4 KB to 4.7 KB but the 2.4% of weak shares a
receiver then asked for added a round trip and raised the shares that missed
the chain from 9.69% to 12.37%.

## Finding peers

Bitcoin Cash has no P2Poolv2 DNS seeds. A new node finds its first peers in
`[network] dial_peers` or through Nostr. A `dial_peers` entry is an IP
address (`/ip4/`, `/ip6/`) or host name (`/dns4/`, `/dns6/`, `/dns/`), port
6884 by default ([Requirements](requirements.md#ports)). A host name is
resolved through the system's DNS on every dial, reconnects included. Only
host names in `dial_peers` are dialed, never a `/dns` address learned from a
peer, and `blocked_ips` is checked after a name resolves.

A node keeps its peer ID across restarts: its libp2p key is made on the first
start and kept in `p2p_node_key` beside the store
(`[network] node_key_path`). With `ephemeral_node_key = true` it gets a new
peer ID every start, which nothing links across restarts.

### Finding peers through Nostr

Reachable nodes announce themselves on public Nostr relays, and a node short
of peers asks those relays. No single party holds the list: the relays have
different operators, a node asks all of its relays, and a relay can drop an
announcement but not forge one.

- **Who announces:** a node with an `external_address` and `nostr_announce`
  on (the default). It publishes that address, ending in its peer ID, every
  hour.
- **What:** one NIP-78 app-data event (kind 30078) per node and network,
  signed with a key derived from the node key. Its `d` tag is
  `p2poolv2/<chain>-<network>/<hash>`, the hash of the node's protocol
  string, so a node only finds peers on its own rules. It carries a NIP-40
  `expiration` three hours ahead, 16 bits of NIP-13 proof of work, and a
  NIP-31 `alt` text.
- **Who looks:** a node with fewer than 8 peers, when DNS seeds would be
  asked (at once without `dial_peers`, otherwise after 10 seconds), then
  every 10 minutes while still short.
- **What a reader checks:** each event's id, signature, proof of work, tag
  and age. It keeps the newest per author, skips itself and peers it has,
  and dials at most 8 routable addresses (on regtest, loopback and private
  ones too). The noise handshake with the protocol prologue still decides
  whether a peer is on our network.
- **Relays:** `[network] nostr_relays`, by default six public relays with
  different operators. An empty list keeps the node off Nostr.

Announcing publishes the node's address and its being a P2Pool node, which
`external_address` already offers to anyone who dials it. A node that only
looks tells its relays it is a P2Pool node on that network.

## Outgoing peers

A node keeps at least `[network] target_outgoing_peers` outgoing peers (16
by default): connected peers it dialed, from `dial_peers`, a Kademlia query
or the target. While short, it dials one more peer a second
(`p2poolv2_lib/src/node/outgoing_peer_target.rs`):

- **Peers it has heard of.** A book of up to 1,000 peers and their
  addresses, from Kademlia's routing updates and the listen addresses peers
  report through Identify (a DNS seed peer, Bitcoin only, enters the same
  way). Only globally routable IPv4 and IPv6 addresses with a TCP port are
  kept, so nodes on a LAN or one machine find each other through
  `dial_peers`. A peer reached over an outgoing connection is verified; a
  full book drops a random unverified peer for a new one.
- **Spread over network groups.** The target picks a random network group
  with a candidate, then a peer in it, and never dials into an IPv4 /16 or
  IPv6 /32 that already holds an outgoing connection, as Bitcoin Cash Node
  does (`CConnman::ThreadOpenConnections`, `CNetAddr::GetGroup`).
  `dial_peers` connections hold no group and are left to the reconnector.
- **Within the limits.** Dials in flight count toward the target, no dial
  passes `max_established_outgoing`, and the target's own dials in flight
  stay below half of `max_pending_outgoing`. Blocked IPs are never dialed.
- **With backoff.** A peer is not dialed again for 60 seconds, doubled after
  each failure (a failed dial, or a connection closed within 60 seconds). A
  lasting connection resets it; five failures in a row drop the peer.

The share chain's P2P network runs on TCP over IPv4 and IPv6 only, with no
onion or I2P addresses. The target sets how many connections a node keeps,
not how shares are relayed. `/metrics` reports `p2p_outgoing_peers`,
`p2p_incoming_peers` and `p2p_outgoing_target_dials_total`
([Metrics](architecture/metrics.md)).

Why 16: P2Pool v1 kept 6 (`--outgoing-conns`) and Bitcoin Cash Node keeps 8.
In the network simulation, 1,000 nodes sending shares without payouts and
relaying after the header check carry a share every 4.46 seconds on 16 peers
against 5.30 on 8, and stale shares fall from 2.30% to 2.00% at 10-second
shares; with whole shares, more peers alone made propagation slower
([Network simulation](simulation/network-sim.md), "Where each idea reaches
the limit"). With every node dialing 16, the average node has about 32
links.

Each outgoing peer is another node's inbound peer: 16 inbound on average if
every node accepts inbound connections, 32 if half do, 64 if a quarter do,
and more for a node alone in its /16. The sample configs set
`max_established_incoming` to 128: with 1,000 nodes, a quarter accepting
inbound and 30% of those in ten cloud /16s, the busiest takes about 110.

The book survives a restart: it is written to `p2p_peers.json` beside
the store every ten minutes and at shutdown, each peer with whether the
node reached it. The peers held over outgoing connections at the save are
anchors, dialed once before any other on the next start, so a restarted node
is back among its peers before anyone fills its slots, as Bitcoin Core's
`anchors.dat` does; they still pass the rules above. A missing or broken
file means an empty book; a node with `ephemeral_node_key` keeps none,
since redialing its anchors would link its restarts.

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
