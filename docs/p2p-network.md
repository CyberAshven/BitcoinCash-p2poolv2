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
`weak-by-hash/without-payouts/headers-resume` today (`network_prologue` in
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

A new node finds its first peers in `[network] dial_peers`, through Nostr,
from DNS seeds once its network has some, and after a restart from the peers
it saved. A `dial_peers` entry is an IP
address (`/ip4/`, `/ip6/`) or host name (`/dns4/`, `/dns6/`, `/dns/`), port
6884 by default ([Requirements](requirements.md#ports)). A host name is
resolved through the system's DNS on every dial, reconnects included. Only
host names in `dial_peers` are dialed, never a `/dns` address learned from a
peer, and `blocked_ips` is checked after a name resolves.

A node keeps its peer ID across restarts: its libp2p key is made on the first
start and kept in `p2p_node_key` beside the store
(`[network] node_key_path`). With `ephemeral_node_key = true` it gets a new
peer ID every start, which nothing links across restarts. A running node
holds `p2p_node_key.lock` and `p2p_peers.json.lock`, so a second node whose
store sits in the same directory stops at start instead of taking the same
peer ID, which would keep the two from ever connecting. Give each node its
own directory.

### Onion peers

With `[network] tor_proxy` set to a Tor daemon's SOCKS5 port (Tor's default
is `127.0.0.1:9050`), the node dials `/onion3/<address>:<port>` peers through
it, as Bitcoin Cash Node's `-onion` does (`node/onion.rs`). The onion name
goes to the proxy unresolved. Every other address is dialed directly,
unless `proxy_clearnet` is on (below). An onion address can be a
`dial_peers` entry; one a peer reports through Identify enters Kademlia and
the target's book, and one announced on Nostr is dialed, only on a node with
a Tor proxy, `tor_proxy` or the one an onion service brings (below), so a
node without Tor never tries one. The network group of an onion address is
the first 4 bits of its key, as in Bitcoin Cash Node.

A node on a computer running Tor makes itself an onion service through Tor's
control port, as Bitcoin Cash Node's `-listenonion` does
(`node/tor_control.rs`). `[network] listen_onion` is on by default; the node
connects to `tor_control` (`127.0.0.1:9051` by default) and:

1. asks `PROTOCOLINFO 1` and authenticates with `tor_password`
   (`HASHEDPASSWORD`), with no password, or with the cookie file of Tor
   (`COOKIE`). A password is the stronger choice: `COOKIE` sends the cookie
   itself to whatever answers on `tor_control`. `SAFECOOKIE`, which Bitcoin
   Cash Node uses, is not used here: it needs two HMAC keys that the control
   spec fixes as constant strings;
2. sends `ADD_ONION` with its key, or `NEW:ED25519-V3` the first time, and
   `Port=6884,<listen address>`: peers dial port 6884, the default, whatever
   the node listens on, and Tor forwards to the node's `listen_address`
   (`127.0.0.1` for `0.0.0.0`);
3. saves the new key as `ED25519-V3:<base64>` in `onion_v3_private_key`
   beside the store, readable by its owner only, and sends it again on every
   later start, so the onion address stays. With `ephemeral_node_key` Tor
   keeps no key (`Flags=DiscardPK`) and the address is new on every start;
4. keeps the control connection open, since Tor removes the service when it
   closes. While it is up, Identify sends `/onion3/<address>:6884`, pushed to
   connected peers at once, and the node announces it on Nostr. When the
   connection closes the node stops advertising it and connects again after
   1 second, 1.5 times longer each try, at most 10 minutes.

Once the service is up, a node without a `tor_proxy` dials onion peers
through `127.0.0.1:9050`, Tor's default SOCKS port, as Bitcoin Cash Node
does. With no Tor running, or no control port, the node logs each try at
debug only. Tor's defaults leave the control port off; a torrc with
`ControlPort 9051` and `CookieAuthentication 1` turns it on, and the node's
user must be able to read the cookie file (on Debian, be in the
`debian-tor` group). `listen_onion` is off when the node does not listen,
its `external_address` is an onion address already, or `onlynet` (below)
leaves onion out.

An onion service can be set up by hand instead, in the torrc, as Bitcoin
Cash Node's `doc/tor.md` describes:

```
HiddenServiceDir /var/lib/tor/p2pool/
HiddenServicePort 6884 127.0.0.1:6884
```

Tor writes the service's name to `hostname` in that directory and keeps its
key there. Set `external_address = "/onion3/<the name without .onion>:6884"`:
Identify sends it to every peer, and peers with a `tor_proxy` keep and dial
it. The first port, the one peers dial, stays 6884 whatever the node listens
on, as Bitcoin Cash Node keeps its default port for every onion service, so
the port tells nothing about the node. The second address is the node's
`listen_address`; `127.0.0.1` reaches a node listening on `0.0.0.0`. A node
that has an IP address as well announces both, and its peer ID links them.

With `onion_only = true` the node announces only its onion address, the one
set up by hand or the one the control port made:

- An `external_address` that is not `/onion3` stops it at start.
- It takes no external address from what peers observe, and never makes a
  routable listen address its external one, as Bitcoin Cash Node with
  `-discover=0`.
- Identify sends no listen addresses, only the onion one.
- It announces on Nostr only when it reaches the relays through Tor, as a
  node that opens no clearnet connection of its own does (`onlynet` without
  IPv4 and IPv6, or `proxy_clearnet`, below). A relay it reached directly
  would see its IP beside its onion address, so then it does not announce.

Listen on `127.0.0.1`, so only Tor reaches the node. `onion_only` governs what
the node announces, not what it dials: without `onlynet` or
`proxy_clearnet`, dials to IP addresses, from `dial_peers`, the outgoing
target or Kademlia, still go out directly and show those peers its IP and
peer ID. A node that was reachable by IP before needs
a new node key and an empty peer list, or peers that knew it can link its IP
to the onion. Inbound onion connections all come from the Tor daemon, so
blocking `127.0.0.1` cuts every onion peer.

**Onion-only.** `onlynet = ["onion"]`, as Bitcoin Cash Node's
`-onlynet=onion`, has the node dial onion peers alone. Every other address
is refused before anything is dialed or resolved (`base_transport` in
`node/mod.rs`): `OrTransport` would hand a refused address on to TCP, so the
refusal comes first. The node's own TCP only listens, IP addresses from
peers and announcements are not kept, DNS seeds are skipped, and the Nostr
relays are reached through Tor, the name of a relay unresolved. Such a node
also announces only onion addresses, as with `onion_only` above. Listen on
`127.0.0.1` (`listen_address = "/ip4/127.0.0.1/tcp/6884"`), so the node
listens on no IP address peers could reach. `onlynet` takes `"ipv4"`,
`"ipv6"` and `"onion"` in any mix; a host name is clearnet (`/dns4` IPv4,
`/dns6` IPv6, `/dns` either). `"onion"` needs `tor_proxy`, or `listen_onion`:
without `tor_proxy` the node dials no peer and reaches no relay until the
control port has made its onion service, and from then on uses
`127.0.0.1:9050`. Onion peers saved in `p2p_peers.json` are loaded at start
only on a node with `tor_proxy`, so an onion-only node keeps its peer list
across restarts with `tor_proxy = "127.0.0.1:9050"` set.

**Everything through Tor.** `proxy_clearnet = true`, as Bitcoin Cash Node's
`-proxy`, dials IPv4, IPv6 and host-name peers through `tor_proxy` too, a
name unresolved for Tor's exit to resolve. The node's own TCP then only
listens and nothing reaches the system's resolver: DNS seeds are skipped and
the Nostr relays are reached through Tor. `blocked_ips` still refuses an IP
dial; the address behind a host name is Tor's to find, so it is not checked.
Loading refuses `proxy_clearnet` without `tor_proxy`. The peers the node
dials see a Tor exit, so it takes no external address from what they
observe, as `-proxy` sets `-discover=0` in Bitcoin Cash Node. Listen on
`127.0.0.1` here too, unless peers should reach the node at its own address.

**How the settings combine.** Each follows a setting of Bitcoin Cash Node:

| Setting | Bitcoin Cash Node | What it decides |
|---|---|---|
| `tor_proxy` | `-onion` | the proxy for onion peers |
| `listen_onion` | `-listenonion` | the onion service, and `127.0.0.1:9050` for onion peers without `tor_proxy` |
| `onion_only` | `-discover=0` with an onion `-externalip` | announcing only the onion address |
| `onlynet` | `-onlynet` | the networks the node dials, and so the ones it announces |
| `proxy_clearnet` | `-proxy` | IPv4, IPv6 and host names dialed through Tor, and no address from what peers observe |

- `onlynet` without `"ipv4"` and `"ipv6"` turns `onion_only` on: Bitcoin
  Cash Node advertises no address on a network it does not reach (`AddLocal`
  in `src/net.cpp`). `onion_only` alone limits no dial.
- A node that opens no clearnet connection of its own reaches its Nostr
  relays through Tor, so an onion-only one announces its onion address
  there: the relay sees a Tor exit. An `onion_only` node that dials
  clearnet directly also reaches the relays directly, and announces nothing
  on Nostr.
- Onion peers become reachable once the control port has made the service,
  through `127.0.0.1:9050`, only if `onlynet` allows onion, and a node whose
  `onlynet` leaves onion out makes no onion service. Here the node differs
  from Bitcoin Cash Node, whose control port code makes onion reachable
  whatever `-onlynet` says.
- With `-proxy`, Bitcoin Cash Node also stops listening, and so makes no
  onion service, unless `-listen=1` is given. `proxy_clearnet` keeps the
  node listening on `listen_address` and making its onion service.

### DNS seeds

A seed is a host name whose A and AAAA records list long-running nodes. The
node asks the seeds once: at start without `dial_peers`, or 10 seconds in
when no `dial_peers` entry connected. It dials at most 16 addresses from any
one seed, so no seed operator can fill a node's connections.

- **Which seeds:** a Bitcoin Cash node takes the seeds of its
  `bitcoin_cash_network` (`node/dns_seeds.rs`), never Bitcoin's. Chipnet's
  list has one seed node, `p2poolv2-chipnet.realbitcoin.cash:6886`; mainnet's
  is empty until operators run seeds; regtest and scalenet have none.
  `[network] dns_seed_hosts` replaces the built-in list, so a new seed works
  before a release names it. `dns_seed = false` skips seeds.
- **Running a seed:** point a host name's A/AAAA records at long-running
  nodes of one network that listen on port 6884. No crawler is needed; keep
  the records to nodes that stay up. Each seed should have its own operator,
  so one person's mistake or outage leaves the others. A seed whose nodes
  listen on another port is written `host:port`, in the built-in lists and
  in `dns_seed_hosts`.
- **Names outside DNS:** the node resolves seeds through the system resolver.
  A `.fn` name from [Freedom Names](https://github.com/FreedomNames/FreedomNames)
  works when the system resolver asks a Freedom Names node, as its README
  describes; the node itself needs no change for that.
- **Yggdrasil:** a node's Yggdrasil address (`200::/7`) counts as routable,
  so it can be the node's `external_address`, a seed's AAAA record or a
  `dial_peers` entry. Only nodes on Yggdrasil can reach it.

### Finding peers through Nostr

Reachable nodes announce themselves on public Nostr relays, and a node short
of peers asks those relays. No single party holds the list: the relays have
different operators, a node asks all of its relays, and a relay can drop an
announcement but not forge one.

- **Who announces:** a node with an `external_address`, or an onion service
  made through the Tor control port, and `nostr_announce` on (the default).
  An onion-only node announces only when it reaches the relays through Tor,
  as a node that opens no clearnet connection of its own does. It publishes
  those addresses, each ending in its peer ID, every hour, and again, at
  most once a minute, when its onion service comes up, or goes away while
  it has another address to announce.
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
  different operators. An empty list keeps the node off Nostr. A node that
  opens no clearnet connection of its own reaches them through Tor
  ([Onion peers](#onion-peers)), and not at all while it has no Tor proxy.

Announcing publishes the node's address and its being a P2Pool node, which
`external_address` already offers to anyone who dials it. A node that only
looks tells its relays it is a P2Pool node on that network.

## Outgoing peers

A node keeps at least `[network] target_outgoing_peers` outgoing peers (16
by default): connected peers it dialed and keeps, from `dial_peers`, the
target, or a DNS or Nostr seed. While short, it dials one more peer a second
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
  IPv6 /32 that already holds an outgoing peer, as Bitcoin Cash Node
  does (`CConnman::ThreadOpenConnections`, `CNetAddr::GetGroup`).
  `dial_peers` connections hold no group and are left to the reconnector.
- **Lookups close.** Kademlia dials each unconnected peer a lookup asks, and
  share relay kept those connections open, so lookups, not the target,
  picked a node's peers. An outgoing connection none of the node's own dials
  opened is Kademlia's: it holds no group, does not count, and closes after
  10 seconds once no query runs, or after 60, Kademlia's query timeout. Like
  Bitcoin Cash Node's feeler connections, a lookup only discovers addresses:
  the node sends no handshake on it and answers the peer's without a sync,
  and shares, weak shares and transactions go only to peers that sent a
  handshake, so closing a lookup cuts nothing in flight. The peer stays in
  the routing table and the book. A connection that is not a lookup, opened
  while the lookup is, makes the peer a share-chain peer. A connection to an
  anchor is the target's; inbound connections stay;
  `target_outgoing_peers = 0` keeps lookups open as full peers.
- **Gone peers are forgotten.** Kademlia keeps an unreachable peer at its
  last address, so a node that left, or came back under another peer ID,
  would be handed to every lookup and dialed by every node. When a dial to a
  peer the node has no connection to fails, the node removes it from the
  routing table; a live peer is learned again from the next lookup that
  finds it.
- **Within the limits.** Dials in flight count toward the target, no dial
  passes `max_established_outgoing`, and the target's own dials in flight
  stay below half of `max_pending_outgoing`. Blocked IPs are never dialed.
- **With backoff.** A peer is not dialed again for 60 seconds, doubled after
  each failure (a failed dial, or a connection closed within 60 seconds). A
  lasting connection resets it; five failures in a row drop the peer.

The share chain's P2P network runs on TCP over IPv4 and IPv6, and through
Tor to onion addresses ([Onion peers](#onion-peers)); there are no I2P
addresses. The target sets how many connections a node keeps, not how
shares are relayed. `/metrics` reports `p2p_outgoing_peers`,
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
