// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Keeps a node at a target number of outgoing peers.
//!
//! Without a target, a node's outgoing peers are whatever its dial_peers and
//! Kademlia's queries leave connected. While the node has fewer outgoing peers
//! than `target_outgoing_peers`, it now dials one more peer a second from the
//! peers it has heard of. P2Pool v1 does the same for its `--outgoing-conns`
//! (`ClientFactory._think` in `p2pool/p2p.py`, an attempt about once a
//! second), and Bitcoin Cash Node for its 8 outbound peers
//! (`CConnman::ThreadOpenConnections` in `src/net.cpp`, every 500 ms).
//!
//! - **Known peers.** The node removes a peer from Kademlia's routing table
//!   when it disconnects, so the table holds little besides connected peers.
//!   The target keeps its own book of the globally routable TCP addresses that
//!   Kademlia's routing updates and Identify's listen addresses report; a peer
//!   dialed from a DNS seed arrives the same way once connected. The book
//!   holds at most `MAX_KNOWN_PEERS` peers. A peer the node has reached over an
//!   outgoing connection is verified, and a full book drops a random
//!   unverified peer for a new one, or refuses the new one when every peer is
//!   verified: peers that only claim addresses cannot push out peers that
//!   answered.
//! - **Spread.** The peer is chosen at random: a network group at random
//!   among those with a candidate, then a peer in it. As in Bitcoin Cash Node
//!   (`setConnected`), the node dials no peer in an IPv4 /16 or IPv6 /32
//!   that one of its outgoing peers already holds, so it adds at most one
//!   outgoing peer per group. Connections to peers the operator lists in
//!   dial_peers hold no group, and the target never dials those peers: the
//!   reconnector does.
//! - **Lookups.** Kademlia dials each peer a lookup asks that is not
//!   connected, and share relay would keep that connection busy for good. An
//!   outgoing connection none of this node's own dials opened (the target's,
//!   the reconnector's to a dial_peer, a DNS or Nostr seed's) is Kademlia's.
//!   It holds no group and does not count toward the target, and the node
//!   closes it once it has been open `LOOKUP_SETTLE_SECS` while no Kademlia
//!   query runs, or `KADEMLIA_QUERY_TIMEOUT_SECS`, the longest a query runs,
//!   whatever runs. The settle time lets Identify and Kademlia's protocol
//!   check run, so the peer reaches the book and the routing table, and it
//!   stays in both: the target may still choose it. Bitcoin Cash Node closes
//!   its feeler connections the same way once they have answered. A
//!   connection to an anchor is the exception: the target takes it as its
//!   own while it is below the target and the anchor's group is free.
//!   Inbound connections are never closed here.
//! - **Limits.** A dial never takes the node past `max_established_outgoing`,
//!   and the target's own dials in flight stay under half of
//!   `max_pending_outgoing`, leaving the other half to dial_peers and
//!   Kademlia. Blocked IPs are never dialed, and neither is an address that is
//!   not a globally routable IP address with a TCP port, so `/dns` addresses
//!   learned from peers never enter the book.
//! - **Backoff.** A peer the target dialed is not dialed again for 60
//!   seconds, doubled after each failure: a dial that fails, or a connection
//!   that closes within 60 seconds, as one to a peer that is full does. A
//!   connection that lasts resets it. After 5 failures in a row the peer is
//!   forgotten until Kademlia or Identify report it again.
//!
//! The target changes how many connections a node keeps. It does not change
//! which peers a share is relayed to, or how.

use super::address_filter::{NetworkGroup, network_group};
use super::behaviour::KADEMLIA_QUERY_TIMEOUT_SECS;
use super::connection_tracker::BlockedIps;
use super::dial_filter::{is_not_blocked, without_p2p_suffix};
use super::peer_book::{MAX_ANCHORS, SAVED_BOOK_VERSION, SavedBook, SavedPeer};
use crate::config::NetworkConfig;
use bitcoin::secp256k1::rand::seq::{IteratorRandom, SliceRandom};
use bitcoin::secp256k1::rand::thread_rng;
use libp2p::core::ConnectedPoint;
use libp2p::multiaddr::Protocol;
use libp2p::swarm::ConnectionId;
use libp2p::{Multiaddr, PeerId};
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use tokio::time::Instant;
use tracing::{debug, info, warn};

/// How often the node compares its outgoing peers with the target. At most
/// one dial starts at each check.
const TARGET_CHECK_INTERVAL_SECS: u64 = 1;

/// Wait before the target dials a peer again, doubled after each failure:
/// 60, 120, 240, 480 and 960 seconds.
const TARGET_DIAL_BACKOFF_SECS: u64 = 60;

/// A connection the target dialed that closes sooner than this counts as a
/// failed dial.
const STABLE_CONNECTION_SECS: u64 = 60;

/// Failed dials in a row after which a peer is dropped from the book.
const FORGET_AFTER_FAILURES: u32 = 5;

/// Most peers the book holds.
const MAX_KNOWN_PEERS: usize = 1_000;

/// Most addresses the book holds for one peer.
const MAX_ADDRESSES_PER_PEER: usize = 4;

/// A connection Kademlia opened for a lookup stays open at least this long,
/// so Identify and Kademlia's protocol check run on it.
const LOOKUP_SETTLE_SECS: u64 = 10;

/// A peer the target may dial.
struct KnownPeer {
    /// The peer's dialable addresses, without `/p2p`.
    addresses: Vec<Multiaddr>,
    /// Whether the node has reached the peer over an outgoing connection.
    verified: bool,
    /// Failed dials in a row.
    failures: u32,
    /// The target dials the peer no earlier than this.
    next_attempt_at: Instant,
    /// A peer this node held over an outgoing connection when it last saved
    /// its book: dialed before the others, once.
    anchor: bool,
}

/// An established outgoing connection.
struct OutgoingConnection {
    peer_id: PeerId,
    /// The network group the connection holds; None for a dial_peer or a
    /// lookup.
    group: Option<NetworkGroup>,
    /// When the connection was established, if the target dialed it.
    target_established_at: Option<Instant>,
    /// When the connection was established, if Kademlia opened it for a
    /// lookup and the target does not keep it.
    lookup_established_at: Option<Instant>,
}

/// A dial the target started, until it connects or fails.
struct PendingDial {
    peer_id: PeerId,
    group: NetworkGroup,
}

/// A peer the target chose, and the address to dial it at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TargetDial {
    pub(crate) peer_id: PeerId,
    pub(crate) address: Multiaddr,
    group: NetworkGroup,
}

/// The swarm's outgoing connection counts when the target is checked.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct OutgoingCounts {
    /// Outgoing connections being dialed, by anyone.
    pub(crate) pending: u32,
    /// Established outgoing connections.
    pub(crate) established: u32,
}

/// Dials known peers while the node has fewer outgoing peers than its
/// target. See the module documentation.
pub(crate) struct OutgoingPeerTarget {
    target: usize,
    max_established_outgoing: u32,
    max_pending_outgoing: u32,
    /// The configured dial_peers, without `/p2p`.
    dial_peer_addresses: HashSet<Multiaddr>,
    /// Peers reached at a dial_peer address or named by one. Their
    /// connections hold no group, and the target does not dial them.
    dial_peer_ids: HashSet<PeerId>,
    blocked_ips: BlockedIps,
    known_peers: HashMap<PeerId, KnownPeer>,
    known_peer_limit: usize,
    outgoing: HashMap<ConnectionId, OutgoingConnection>,
    pending: HashMap<ConnectionId, PendingDial>,
    /// Dials in flight to addresses a DNS or Nostr seed returned. Their
    /// connections are the node's own, not Kademlia's.
    seed_dials: HashSet<ConnectionId>,
    /// Lookup connections the node has asked the swarm to close.
    closing_lookups: HashSet<ConnectionId>,
    /// LOOKUP_SETTLE_SECS, shorter in tests.
    lookup_settle: Duration,
    /// Dials started toward the target since start, for P2P health metrics.
    dials_attempted_total: u64,
    /// Whether the node dials through Tor (`tor_proxy`, or Tor's default
    /// SOCKS port once Tor's control port made its onion service), so onion
    /// addresses are worth keeping and dialing.
    onion_reachable: bool,
}

impl OutgoingPeerTarget {
    pub(crate) fn new(config: &NetworkConfig, blocked_ips: BlockedIps) -> Self {
        if config.target_outgoing_peers > config.max_established_outgoing {
            warn!(
                "target_outgoing_peers {} is above max_established_outgoing {}; the node keeps at most {}",
                config.target_outgoing_peers,
                config.max_established_outgoing,
                config.max_established_outgoing
            );
        }
        if config.target_outgoing_peers > 0 {
            info!(
                "Keeping at least {} outgoing peers",
                config.target_outgoing_peers
            );
        }
        let mut dial_peer_addresses = HashSet::with_capacity(config.dial_peers.len());
        let mut dial_peer_ids = HashSet::with_capacity(config.dial_peers.len());
        for dial_peer in &config.dial_peers {
            if let Ok(address) = dial_peer.parse::<Multiaddr>() {
                if let Some(Protocol::P2p(peer_id)) = address.iter().last() {
                    dial_peer_ids.insert(peer_id);
                }
                dial_peer_addresses.insert(without_p2p_suffix(address));
            }
        }
        Self {
            target: config.target_outgoing_peers as usize,
            max_established_outgoing: config.max_established_outgoing,
            max_pending_outgoing: config.max_pending_outgoing,
            dial_peer_addresses,
            dial_peer_ids,
            blocked_ips,
            known_peers: HashMap::with_capacity(MAX_KNOWN_PEERS),
            known_peer_limit: MAX_KNOWN_PEERS,
            outgoing: HashMap::with_capacity(config.max_established_outgoing as usize),
            pending: HashMap::with_capacity(config.max_pending_outgoing as usize),
            seed_dials: HashSet::new(),
            closing_lookups: HashSet::new(),
            lookup_settle: Duration::from_secs(LOOKUP_SETTLE_SECS),
            dials_attempted_total: 0,
            onion_reachable: config.tor_proxy.is_some(),
        }
    }

    /// How often the actor checks the target.
    pub(crate) fn check_interval() -> Duration {
        Duration::from_secs(TARGET_CHECK_INTERVAL_SECS)
    }

    /// False when `target_outgoing_peers` is 0, which turns the target off.
    pub(crate) fn has_target(&self) -> bool {
        self.target > 0
    }

    /// Whether the node dials through Tor, so onion addresses are reachable.
    pub(crate) fn onion_reachable(&self) -> bool {
        self.onion_reachable
    }

    /// The node dials through Tor from now on: without a `tor_proxy`, Tor's
    /// control port has made its onion service and Tor's default SOCKS port
    /// is used. Onion addresses learned before were not kept.
    pub(crate) fn set_onion_reachable(&mut self) {
        self.onion_reachable = true;
    }

    /// Peers with at least one established outgoing connection, lookups
    /// included.
    pub(crate) fn outgoing_peer_count(&self) -> u64 {
        let peers: HashSet<&PeerId> = self
            .outgoing
            .values()
            .map(|connection| &connection.peer_id)
            .collect();
        peers.len() as u64
    }

    /// Peers with an outgoing connection that is not a lookup: the ones the
    /// target counts.
    fn kept_peer_count(&self) -> usize {
        let peers: HashSet<&PeerId> = self
            .outgoing
            .values()
            .filter(|connection| connection.lookup_established_at.is_none())
            .map(|connection| &connection.peer_id)
            .collect();
        peers.len()
    }

    /// Dials started toward the target since start.
    pub(crate) fn dials_attempted_total(&self) -> u64 {
        self.dials_attempted_total
    }

    /// Record the addresses a peer reported (Identify) or Kademlia holds for
    /// it. Only dialable addresses are kept, and they replace the ones the
    /// book had. A dial_peer, or a peer with no dialable address, is ignored.
    pub(crate) fn learn_addresses<'a>(
        &mut self,
        peer_id: PeerId,
        addresses: impl IntoIterator<Item = &'a Multiaddr>,
    ) {
        let dialable = dialable_addresses(addresses, self.onion_reachable);
        if dialable.is_empty() || self.dial_peer_ids.contains(&peer_id) {
            return;
        }
        match self.known_peers.get_mut(&peer_id) {
            Some(known_peer) => known_peer.addresses = dialable,
            None => self.insert_known_peer(peer_id, dialable),
        }
    }

    /// Record an established connection. An outgoing one verifies the peer in
    /// the book. One the target, the reconnector or a seed dialed counts
    /// toward the target and holds the network group of its address, unless
    /// it goes to a dial_peer; so does one to an anchor while the target has
    /// room and the group is free. Any other outgoing connection is
    /// Kademlia's lookup. A dial the target started is no longer pending.
    /// Returns whether the connection is a lookup.
    pub(crate) fn record_connection_established(
        &mut self,
        peer_id: PeerId,
        connection_id: ConnectionId,
        endpoint: &ConnectedPoint,
    ) -> bool {
        let target_dial = self.pending.remove(&connection_id);
        let seed_dial = self.seed_dials.remove(&connection_id);
        let mut is_lookup = false;
        if let ConnectedPoint::Dialer { address, .. } = endpoint {
            if self
                .dial_peer_addresses
                .contains(&without_p2p_suffix(address.clone()))
            {
                self.dial_peer_ids.insert(peer_id);
                self.known_peers.remove(&peer_id);
            }
            let dial_peer = self.dial_peer_ids.contains(&peer_id);
            let group = network_group(address);
            let anchor = !dial_peer && self.takes_anchor(&peer_id, group);
            if let Some(known_peer) = self.known_peers.get_mut(&peer_id) {
                known_peer.verified = true;
                if anchor {
                    known_peer.anchor = false;
                }
            }
            let lookup = !(dial_peer || anchor || seed_dial || target_dial.is_some());
            self.outgoing.insert(
                connection_id,
                OutgoingConnection {
                    peer_id,
                    group: group.filter(|_| !dial_peer && !lookup),
                    target_established_at: target_dial.map(|_| Instant::now()),
                    lookup_established_at: lookup.then(Instant::now),
                },
            );
            is_lookup = lookup;
        }
        is_lookup
    }

    /// Whether the target takes a connection it did not dial to `peer_id` as
    /// its own: an anchor from the saved book, while the target has room and
    /// the anchor's group is free.
    fn takes_anchor(&self, peer_id: &PeerId, group: Option<NetworkGroup>) -> bool {
        self.known_peers
            .get(peer_id)
            .is_some_and(|known_peer| known_peer.anchor)
            && group.is_some_and(|group| !self.held_groups().contains(&group))
            && self.kept_peer_count() < self.target
    }

    /// Record that the node dialed an address a DNS or Nostr seed returned.
    pub(crate) fn record_seed_dial_started(&mut self, connection_id: ConnectionId) {
        self.seed_dials.insert(connection_id);
    }

    /// The lookup connections to close: those open `LOOKUP_SETTLE_SECS` while
    /// no Kademlia query runs, and those open `KADEMLIA_QUERY_TIMEOUT_SECS`
    /// whatever runs. A connection is listed until it is recorded closing.
    pub(crate) fn finished_lookups(&self, queries_running: bool) -> Vec<ConnectionId> {
        let query_timeout = Duration::from_secs(KADEMLIA_QUERY_TIMEOUT_SECS);
        self.outgoing
            .iter()
            .filter(|(connection_id, _)| !self.closing_lookups.contains(connection_id))
            .filter_map(|(connection_id, connection)| {
                let open = connection.lookup_established_at?.elapsed();
                (open >= query_timeout || (!queries_running && open >= self.lookup_settle))
                    .then_some(*connection_id)
            })
            .collect()
    }

    /// Record that the node asked the swarm to close a finished lookup.
    pub(crate) fn record_lookup_closing(&mut self, connection_id: ConnectionId) {
        self.closing_lookups.insert(connection_id);
    }

    /// Record a closed connection. One the target dialed counts as a failed
    /// dial when it closed within STABLE_CONNECTION_SECS, and resets the
    /// peer's backoff when it lasted. Returns whether the node closed it as a
    /// finished lookup.
    pub(crate) fn record_connection_closed(&mut self, connection_id: ConnectionId) -> bool {
        let closed_lookup = self.closing_lookups.remove(&connection_id);
        let target_connection = self.outgoing.remove(&connection_id).and_then(|connection| {
            connection
                .target_established_at
                .map(|established_at| (connection.peer_id, established_at))
        });
        match target_connection {
            Some((peer_id, established_at))
                if established_at.elapsed() < Duration::from_secs(STABLE_CONNECTION_SECS) =>
            {
                self.record_failure(&peer_id);
            }
            Some((peer_id, _)) => self.reset_backoff(&peer_id),
            None => {}
        }
        closed_lookup
    }

    /// Record a failed outgoing connection. Backs off only when it is one of
    /// the target's dials; other dials are ignored.
    pub(crate) fn record_connection_failure(&mut self, connection_id: ConnectionId) {
        self.seed_dials.remove(&connection_id);
        if let Some(dial) = self.pending.remove(&connection_id) {
            self.record_failure(&dial.peer_id);
        }
    }

    /// Record a dial the swarm refused synchronously, so no connection was
    /// ever pending.
    pub(crate) fn record_dial_failure(&mut self, peer_id: &PeerId) {
        self.record_failure(peer_id);
    }

    /// Record that the target's dial is in flight on `connection_id`.
    pub(crate) fn record_dial_started(&mut self, connection_id: ConnectionId, dial: TargetDial) {
        self.pending.insert(
            connection_id,
            PendingDial {
                peer_id: dial.peer_id,
                group: dial.group,
            },
        );
    }

    /// The next peer to dial toward the target, or None when the node has
    /// enough outgoing peers, no room for another dial or no candidate. A
    /// candidate is a known peer out of backoff that is not connected, not a
    /// dial_peer and not being dialed by the target, at an address that is
    /// not blocked and whose network group no outgoing connection or target
    /// dial holds. The chosen peer is not chosen again until its backoff
    /// passes, whatever the dial's outcome.
    pub(crate) fn next_dial(
        &mut self,
        counts: OutgoingCounts,
        is_connected: impl Fn(&PeerId) -> bool,
    ) -> Option<TargetDial> {
        if !self.has_room(counts) {
            return None;
        }
        let dial = self.choose_candidate(&is_connected)?;
        if let Some(known_peer) = self.known_peers.get_mut(&dial.peer_id) {
            known_peer.next_attempt_at = Instant::now() + backoff_delay(known_peer.failures);
            known_peer.anchor = false;
        }
        self.dials_attempted_total += 1;
        Some(dial)
    }

    /// Whether another dial fits: the outgoing peers other than lookups and
    /// every outgoing dial in flight stay below the target, all outgoing
    /// connections and dials below max_established_outgoing,
    /// and the target's own dials in flight below half of
    /// max_pending_outgoing.
    fn has_room(&self, counts: OutgoingCounts) -> bool {
        self.kept_peer_count() + (counts.pending as usize) < self.target
            && counts.established.saturating_add(counts.pending) < self.max_established_outgoing
            && counts.pending < self.max_pending_outgoing
            && self.pending.len() < (self.max_pending_outgoing / 2) as usize
    }

    /// Pick a random network group among the candidates' groups, then a
    /// random candidate in it, so a group with many peers is no likelier to
    /// be dialed than a group with one.
    fn choose_candidate(&self, is_connected: &impl Fn(&PeerId) -> bool) -> Option<TargetDial> {
        let now = Instant::now();
        let held_groups = self.held_groups();
        let pending_peers: HashSet<PeerId> =
            self.pending.values().map(|dial| dial.peer_id).collect();
        let is_ready = |(peer_id, known_peer): &(&PeerId, &KnownPeer)| {
            known_peer.next_attempt_at <= now
                && !self.dial_peer_ids.contains(peer_id)
                && !pending_peers.contains(peer_id)
                && !is_connected(peer_id)
        };
        // Anchors from the saved book go first, while any is ready.
        let anchors_ready = self
            .known_peers
            .iter()
            .any(|entry| entry.1.anchor && is_ready(&entry));
        let ready_peers = self
            .known_peers
            .iter()
            .filter(|entry| is_ready(entry) && (!anchors_ready || entry.1.anchor));
        let mut candidates: HashMap<NetworkGroup, Vec<(PeerId, &Multiaddr)>> =
            HashMap::with_capacity(self.known_peers.len());
        for (peer_id, known_peer) in ready_peers {
            for address in &known_peer.addresses {
                if let Some(group) = network_group(address)
                    && !held_groups.contains(&group)
                    && is_not_blocked(address, &self.blocked_ips)
                {
                    candidates
                        .entry(group)
                        .or_default()
                        .push((*peer_id, address));
                }
            }
        }
        let mut rng = thread_rng();
        let (group, peers) = candidates.iter().choose(&mut rng)?;
        let (peer_id, address) = peers.choose(&mut rng)?;
        Some(TargetDial {
            peer_id: *peer_id,
            address: (*address).clone(),
            group: *group,
        })
    }

    /// Groups held by outgoing connections and by the target's dials in
    /// flight.
    fn held_groups(&self) -> HashSet<NetworkGroup> {
        let mut held = HashSet::with_capacity(self.outgoing.len() + self.pending.len());
        held.extend(
            self.outgoing
                .values()
                .filter_map(|connection| connection.group),
        );
        held.extend(self.pending.values().map(|dial| dial.group));
        held
    }

    /// Add a peer to the book. A full book first drops a random unverified
    /// peer; when every peer is verified the new one is not added.
    fn insert_known_peer(&mut self, peer_id: PeerId, addresses: Vec<Multiaddr>) {
        if self.known_peers.len() >= self.known_peer_limit && !self.forget_random_unverified_peer()
        {
            debug!("Known peers full of verified peers, not adding {peer_id}");
            return;
        }
        self.known_peers.insert(
            peer_id,
            KnownPeer {
                addresses,
                verified: false,
                failures: 0,
                next_attempt_at: Instant::now(),
                anchor: false,
            },
        );
    }

    /// Drop a random unverified peer from the book. False when there is none.
    fn forget_random_unverified_peer(&mut self) -> bool {
        let unverified = self
            .known_peers
            .iter()
            .filter(|(_, known_peer)| !known_peer.verified)
            .map(|(peer_id, _)| *peer_id)
            .choose(&mut thread_rng());
        match unverified {
            Some(peer_id) => {
                self.known_peers.remove(&peer_id);
                true
            }
            None => false,
        }
    }

    /// Count a failed dial: double the peer's backoff, or forget the peer
    /// after FORGET_AFTER_FAILURES failures in a row.
    fn record_failure(&mut self, peer_id: &PeerId) {
        let forget = match self.known_peers.get_mut(peer_id) {
            Some(known_peer) => {
                known_peer.failures += 1;
                known_peer.next_attempt_at = Instant::now() + backoff_delay(known_peer.failures);
                known_peer.failures >= FORGET_AFTER_FAILURES
            }
            None => false,
        };
        if forget {
            debug!("Forgetting peer {peer_id} after {FORGET_AFTER_FAILURES} failed dials");
            self.known_peers.remove(peer_id);
        }
    }

    fn reset_backoff(&mut self, peer_id: &PeerId) {
        if let Some(known_peer) = self.known_peers.get_mut(peer_id) {
            known_peer.failures = 0;
        }
    }

    /// The book to save: every known peer with its addresses and whether the
    /// node reached it, verified first, and as anchors the peers the target
    /// holds over outgoing connections now, at most `MAX_ANCHORS`. Peers
    /// reached at a dial_peer address are the reconnector's and stay out.
    pub(crate) fn saved_book(&self) -> SavedBook {
        let mut peers: Vec<SavedPeer> = self
            .known_peers
            .iter()
            .map(|(peer_id, known_peer)| SavedPeer {
                peer_id: peer_id.to_string(),
                addresses: known_peer
                    .addresses
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
                verified: known_peer.verified,
            })
            .collect();
        peers.sort_by_key(|peer| !peer.verified);
        let mut anchors: Vec<String> = self
            .outgoing
            .values()
            .filter(|connection| connection.group.is_some())
            .map(|connection| connection.peer_id.to_string())
            .collect();
        anchors.sort();
        anchors.dedup();
        anchors.truncate(MAX_ANCHORS);
        SavedBook {
            version: SAVED_BOOK_VERSION,
            peers,
            anchors,
        }
    }

    /// Add the peers of a saved book, within the book's limit: only peers
    /// with a dialable address, never a dial_peer, each with the verified
    /// mark it had and no backoff. Its anchors are dialed before any other
    /// peer, once. Entries that do not parse are skipped.
    pub(crate) fn load_book(&mut self, book: SavedBook) {
        if book.version != SAVED_BOOK_VERSION {
            warn!(
                "Ignoring a saved peer book of version {}, this node writes {SAVED_BOOK_VERSION}",
                book.version
            );
            return;
        }
        let anchors: HashSet<PeerId> = book
            .anchors
            .iter()
            .filter_map(|peer_id| peer_id.parse().ok())
            .take(MAX_ANCHORS)
            .collect();
        let mut loaded = 0;
        for saved in book.peers {
            if self.known_peers.len() >= self.known_peer_limit {
                break;
            }
            let Ok(peer_id) = saved.peer_id.parse::<PeerId>() else {
                continue;
            };
            let parsed: Vec<Multiaddr> = saved
                .addresses
                .iter()
                .filter_map(|address| address.parse().ok())
                .collect();
            let addresses = dialable_addresses(&parsed, self.onion_reachable);
            if addresses.is_empty() || self.dial_peer_ids.contains(&peer_id) {
                continue;
            }
            self.known_peers.insert(
                peer_id,
                KnownPeer {
                    addresses,
                    verified: saved.verified,
                    failures: 0,
                    next_attempt_at: Instant::now(),
                    anchor: anchors.contains(&peer_id),
                },
            );
            loaded += 1;
        }
        if loaded > 0 {
            info!(
                "Loaded {loaded} known peers from the saved peer book, {} anchors",
                self.known_peers.values().filter(|peer| peer.anchor).count()
            );
        }
    }

    /// Whether the book holds `peer_id`, for tests.
    #[cfg(test)]
    fn knows(&self, peer_id: &PeerId) -> bool {
        self.known_peers.contains_key(peer_id)
    }

    /// Set a smaller book, for tests.
    #[cfg(test)]
    fn with_known_peer_limit(mut self, known_peer_limit: usize) -> Self {
        self.known_peer_limit = known_peer_limit;
        self
    }

    /// Set a shorter settle time for lookups, for tests on real connections.
    #[cfg(test)]
    pub(crate) fn with_lookup_settle(mut self, lookup_settle: Duration) -> Self {
        self.lookup_settle = lookup_settle;
        self
    }
}

/// The wait before the next dial after `failures` failures in a row.
fn backoff_delay(failures: u32) -> Duration {
    Duration::from_secs(TARGET_DIAL_BACKOFF_SECS << failures.min(FORGET_AFTER_FAILURES))
}

/// The addresses the target may dial, without `/p2p`, de-duplicated and at
/// most MAX_ADDRESSES_PER_PEER of them. Onion addresses count only when the
/// node dials through Tor (`onion`).
fn dialable_addresses<'a>(
    addresses: impl IntoIterator<Item = &'a Multiaddr>,
    onion: bool,
) -> Vec<Multiaddr> {
    let mut dialable = Vec::with_capacity(MAX_ADDRESSES_PER_PEER);
    for address in addresses {
        let address = without_p2p_suffix(address.clone());
        if dialable.len() < MAX_ADDRESSES_PER_PEER
            && is_dialable(&address, onion)
            && !dialable.contains(&address)
        {
            dialable.push(address);
        }
    }
    dialable
}

/// A globally routable IPv4 or IPv6 address followed by a TCP port and
/// nothing else, what the node's TCP transport dials; or, when the node dials
/// through Tor (`onion`), a v3 onion address alone.
fn is_dialable(address: &Multiaddr, onion: bool) -> bool {
    let mut protocols = address.iter();
    let dialable_form = match (protocols.next(), protocols.next(), protocols.next()) {
        (Some(Protocol::Ip4(_) | Protocol::Ip6(_)), Some(Protocol::Tcp(port)), None) => port > 0,
        (Some(Protocol::Onion3(onion3)), None, None) => onion && onion3.port() > 0,
        _ => false,
    };
    dialable_form && network_group(address).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use libp2p::core::Endpoint;
    use libp2p::core::transport::PortUse;
    use tokio::time::advance;

    fn dialer(address: &str) -> ConnectedPoint {
        ConnectedPoint::Dialer {
            address: address.parse().unwrap(),
            role_override: Endpoint::Dialer,
            port_use: PortUse::Reuse,
        }
    }

    /// Record an outgoing connection the node keeps: one a seed dialed,
    /// which counts and holds its group like the target's own.
    fn connect_kept(
        target: &mut OutgoingPeerTarget,
        peer_id: PeerId,
        connection: usize,
        address: &str,
    ) {
        let connection_id = ConnectionId::new_unchecked(connection);
        target.record_seed_dial_started(connection_id);
        target.record_connection_established(peer_id, connection_id, &dialer(address));
    }

    #[tokio::test(start_paused = true)]
    async fn test_dials_a_known_peer_below_the_target() {
        let config = NetworkConfig {
            target_outgoing_peers: 2,
            ..NetworkConfig::default()
        };
        let mut target = OutgoingPeerTarget::new(&config, BlockedIps::default());
        let peer = PeerId::random();
        let address: Multiaddr = "/ip4/1.2.3.4/tcp/6884".parse().unwrap();
        target.learn_addresses(peer, [&address]);

        let dial = target
            .next_dial(OutgoingCounts::default(), |_| false)
            .expect("a known peer is dialed below the target");

        assert_eq!(dial.peer_id, peer);
        assert_eq!(dial.address, address);
        assert_eq!(target.dials_attempted_total(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn test_stops_dialing_at_the_target() {
        let config = NetworkConfig {
            target_outgoing_peers: 2,
            ..NetworkConfig::default()
        };
        let mut target = OutgoingPeerTarget::new(&config, BlockedIps::default());
        connect_kept(&mut target, PeerId::random(), 1, "/ip4/5.6.7.8/tcp/6884");
        connect_kept(&mut target, PeerId::random(), 2, "/ip4/9.10.11.12/tcp/6884");
        let peer = PeerId::random();
        target.learn_addresses(peer, [&"/ip4/1.2.3.4/tcp/6884".parse().unwrap()]);

        assert_eq!(target.outgoing_peer_count(), 2);
        assert_eq!(target.next_dial(OutgoingCounts::default(), |_| false), None);

        target.record_connection_closed(ConnectionId::new_unchecked(2));
        let dial = target.next_dial(OutgoingCounts::default(), |_| false);
        assert_eq!(dial.map(|dial| dial.peer_id), Some(peer));
    }

    /// Dials in flight, the target's or Kademlia's, count toward the target
    /// so a burst of them does not overshoot it.
    #[tokio::test(start_paused = true)]
    async fn test_counts_dials_in_flight_toward_the_target() {
        let config = NetworkConfig {
            target_outgoing_peers: 1,
            ..NetworkConfig::default()
        };
        let mut target = OutgoingPeerTarget::new(&config, BlockedIps::default());
        target.learn_addresses(
            PeerId::random(),
            [&"/ip4/1.2.3.4/tcp/6884".parse().unwrap()],
        );
        let one_dial_in_flight = OutgoingCounts {
            pending: 1,
            established: 0,
        };

        assert_eq!(target.next_dial(one_dial_in_flight, |_| false), None);
    }

    #[tokio::test(start_paused = true)]
    async fn test_never_dials_past_max_established_outgoing() {
        let config = NetworkConfig {
            target_outgoing_peers: 16,
            max_established_outgoing: 2,
            ..NetworkConfig::default()
        };
        let mut target = OutgoingPeerTarget::new(&config, BlockedIps::default());
        target.learn_addresses(
            PeerId::random(),
            [&"/ip4/1.2.3.4/tcp/6884".parse().unwrap()],
        );
        let full = OutgoingCounts {
            pending: 1,
            established: 1,
        };

        assert_eq!(target.next_dial(full, |_| false), None);
    }

    /// The target keeps half of max_pending_outgoing free for dial_peers and
    /// Kademlia, whose dials the connection limits would otherwise deny.
    #[tokio::test(start_paused = true)]
    async fn test_leaves_half_of_max_pending_outgoing_to_others() {
        let config = NetworkConfig {
            target_outgoing_peers: 16,
            max_pending_outgoing: 2,
            ..NetworkConfig::default()
        };
        let mut target = OutgoingPeerTarget::new(&config, BlockedIps::default());
        target.learn_addresses(
            PeerId::random(),
            [&"/ip4/1.2.3.4/tcp/6884".parse().unwrap()],
        );
        target.learn_addresses(
            PeerId::random(),
            [&"/ip4/5.6.7.8/tcp/6884".parse().unwrap()],
        );
        let dial = target
            .next_dial(OutgoingCounts::default(), |_| false)
            .unwrap();
        target.record_dial_started(ConnectionId::new_unchecked(1), dial);

        let one_dial_in_flight = OutgoingCounts {
            pending: 1,
            established: 0,
        };
        assert_eq!(target.next_dial(one_dial_in_flight, |_| false), None);
    }

    /// The second peer of a /16 is not dialed while the first is being
    /// dialed, nor once it is connected; a peer in another /16 is.
    #[tokio::test(start_paused = true)]
    async fn test_never_dials_a_second_peer_in_the_same_ipv4_slash_16() {
        let config = NetworkConfig::default();
        let mut target = OutgoingPeerTarget::new(&config, BlockedIps::default());
        target.learn_addresses(
            PeerId::random(),
            [&"/ip4/1.2.3.4/tcp/6884".parse().unwrap()],
        );
        target.learn_addresses(
            PeerId::random(),
            [&"/ip4/1.2.200.9/tcp/6884".parse().unwrap()],
        );
        let first = target
            .next_dial(OutgoingCounts::default(), |_| false)
            .unwrap();
        let connection_id = ConnectionId::new_unchecked(1);
        let first_peer = first.peer_id;
        let first_address = first.address.to_string();
        target.record_dial_started(connection_id, first);

        assert_eq!(target.next_dial(OutgoingCounts::default(), |_| false), None);

        target.record_connection_established(first_peer, connection_id, &dialer(&first_address));
        assert_eq!(
            target.next_dial(OutgoingCounts::default(), |peer| *peer == first_peer),
            None
        );

        let other_group = PeerId::random();
        target.learn_addresses(other_group, [&"/ip4/1.3.0.1/tcp/6884".parse().unwrap()]);
        let dial = target.next_dial(OutgoingCounts::default(), |peer| *peer == first_peer);
        assert_eq!(dial.map(|dial| dial.peer_id), Some(other_group));
    }

    #[tokio::test(start_paused = true)]
    async fn test_never_dials_a_second_peer_in_the_same_ipv6_slash_32() {
        let config = NetworkConfig::default();
        let mut target = OutgoingPeerTarget::new(&config, BlockedIps::default());
        connect_kept(
            &mut target,
            PeerId::random(),
            1,
            "/ip6/2001:4860:4860::8888/tcp/6884",
        );
        target.learn_addresses(
            PeerId::random(),
            [&"/ip6/2001:4860:1::1/tcp/6884".parse().unwrap()],
        );

        assert_eq!(target.next_dial(OutgoingCounts::default(), |_| false), None);
    }

    /// A connection to a dial_peer counts as an outgoing peer but holds no
    /// group, so a peer in the dial_peer's /16 can still be dialed. The
    /// dial_peer itself is left to the reconnector.
    #[tokio::test(start_paused = true)]
    async fn test_dial_peers_hold_no_network_group() {
        let config = NetworkConfig {
            dial_peers: vec!["/ip4/1.2.3.4/tcp/6884".to_string()],
            ..NetworkConfig::default()
        };
        let mut target = OutgoingPeerTarget::new(&config, BlockedIps::default());
        let dial_peer = PeerId::random();
        target.record_connection_established(
            dial_peer,
            ConnectionId::new_unchecked(1),
            &dialer("/ip4/1.2.3.4/tcp/6884"),
        );
        target.learn_addresses(dial_peer, [&"/ip4/1.2.3.4/tcp/6884".parse().unwrap()]);
        let same_group = PeerId::random();
        target.learn_addresses(same_group, [&"/ip4/1.2.9.9/tcp/6884".parse().unwrap()]);

        assert_eq!(target.outgoing_peer_count(), 1);
        assert!(!target.knows(&dial_peer));
        let dial = target.next_dial(OutgoingCounts::default(), |_| false);
        assert_eq!(dial.map(|dial| dial.peer_id), Some(same_group));
    }

    /// A failed dial backs off 120 seconds, the next 240.
    #[tokio::test(start_paused = true)]
    async fn test_failed_dials_back_off_and_double() {
        let config = NetworkConfig::default();
        let mut target = OutgoingPeerTarget::new(&config, BlockedIps::default());
        let peer = PeerId::random();
        target.learn_addresses(peer, [&"/ip4/1.2.3.4/tcp/6884".parse().unwrap()]);
        let dial = target
            .next_dial(OutgoingCounts::default(), |_| false)
            .unwrap();
        target.record_dial_started(ConnectionId::new_unchecked(1), dial);
        target.record_connection_failure(ConnectionId::new_unchecked(1));

        advance(Duration::from_secs(119)).await;
        assert_eq!(target.next_dial(OutgoingCounts::default(), |_| false), None);
        advance(Duration::from_secs(2)).await;
        let dial = target
            .next_dial(OutgoingCounts::default(), |_| false)
            .expect("the backoff has passed");
        assert_eq!(dial.peer_id, peer);

        target.record_dial_started(ConnectionId::new_unchecked(2), dial);
        target.record_connection_failure(ConnectionId::new_unchecked(2));
        advance(Duration::from_secs(239)).await;
        assert_eq!(target.next_dial(OutgoingCounts::default(), |_| false), None);
        advance(Duration::from_secs(2)).await;
        let dial = target.next_dial(OutgoingCounts::default(), |_| false);
        assert_eq!(dial.map(|dial| dial.peer_id), Some(peer));
    }

    /// A peer that accepts the connection and closes it at once, as a full
    /// peer does, backs off like one that cannot be reached.
    #[tokio::test(start_paused = true)]
    async fn test_connection_that_closes_quickly_counts_as_a_failure() {
        let config = NetworkConfig::default();
        let mut target = OutgoingPeerTarget::new(&config, BlockedIps::default());
        let peer = PeerId::random();
        target.learn_addresses(peer, [&"/ip4/1.2.3.4/tcp/6884".parse().unwrap()]);
        let dial = target
            .next_dial(OutgoingCounts::default(), |_| false)
            .unwrap();
        target.record_dial_started(ConnectionId::new_unchecked(1), dial);
        target.record_connection_established(
            peer,
            ConnectionId::new_unchecked(1),
            &dialer("/ip4/1.2.3.4/tcp/6884"),
        );
        advance(Duration::from_secs(10)).await;
        target.record_connection_closed(ConnectionId::new_unchecked(1));

        advance(Duration::from_secs(119)).await;
        assert_eq!(target.next_dial(OutgoingCounts::default(), |_| false), None);
        advance(Duration::from_secs(2)).await;
        let dial = target.next_dial(OutgoingCounts::default(), |_| false);
        assert_eq!(dial.map(|dial| dial.peer_id), Some(peer));
    }

    /// After a connection that lasted, the next failure backs off 120
    /// seconds again, not 240.
    #[tokio::test(start_paused = true)]
    async fn test_connection_that_lasts_resets_the_backoff() {
        let config = NetworkConfig::default();
        let mut target = OutgoingPeerTarget::new(&config, BlockedIps::default());
        let peer = PeerId::random();
        target.learn_addresses(peer, [&"/ip4/1.2.3.4/tcp/6884".parse().unwrap()]);
        target.record_dial_failure(&peer);
        advance(Duration::from_secs(121)).await;
        let dial = target
            .next_dial(OutgoingCounts::default(), |_| false)
            .unwrap();
        target.record_dial_started(ConnectionId::new_unchecked(1), dial);
        target.record_connection_established(
            peer,
            ConnectionId::new_unchecked(1),
            &dialer("/ip4/1.2.3.4/tcp/6884"),
        );
        advance(Duration::from_secs(300)).await;
        target.record_connection_closed(ConnectionId::new_unchecked(1));

        let dial = target
            .next_dial(OutgoingCounts::default(), |_| false)
            .expect("a peer whose connection lasted is dialed again");
        target.record_dial_started(ConnectionId::new_unchecked(2), dial);
        target.record_connection_failure(ConnectionId::new_unchecked(2));
        advance(Duration::from_secs(119)).await;
        assert_eq!(target.next_dial(OutgoingCounts::default(), |_| false), None);
        advance(Duration::from_secs(2)).await;
        let dial = target.next_dial(OutgoingCounts::default(), |_| false);
        assert_eq!(dial.map(|dial| dial.peer_id), Some(peer));
    }

    #[tokio::test(start_paused = true)]
    async fn test_peer_is_forgotten_after_five_failed_dials() {
        let config = NetworkConfig::default();
        let mut target = OutgoingPeerTarget::new(&config, BlockedIps::default());
        let peer = PeerId::random();
        target.learn_addresses(peer, [&"/ip4/1.2.3.4/tcp/6884".parse().unwrap()]);

        target.record_dial_failure(&peer);
        target.record_dial_failure(&peer);
        target.record_dial_failure(&peer);
        target.record_dial_failure(&peer);
        assert!(target.knows(&peer));
        target.record_dial_failure(&peer);
        assert!(!target.knows(&peer));
    }

    /// A blocked IP is never dialed, including one blocked after the peer
    /// was learned; unblocking it makes the peer a candidate again.
    #[tokio::test(start_paused = true)]
    async fn test_never_dials_a_blocked_ip() {
        let config = NetworkConfig::default();
        let blocked_ips = BlockedIps::default();
        let mut target = OutgoingPeerTarget::new(&config, blocked_ips.clone());
        let peer = PeerId::random();
        target.learn_addresses(peer, [&"/ip4/1.2.3.4/tcp/6884".parse().unwrap()]);
        blocked_ips.insert("1.2.3.4".parse().unwrap());

        assert_eq!(target.next_dial(OutgoingCounts::default(), |_| false), None);

        blocked_ips.remove(&"1.2.3.4".parse().unwrap());
        let dial = target.next_dial(OutgoingCounts::default(), |_| false);
        assert_eq!(dial.map(|dial| dial.peer_id), Some(peer));
    }

    /// Only a globally routable IP with a TCP port is kept: no loopback,
    /// private, `/dns` or QUIC address, and `/p2p` is stripped.
    #[tokio::test(start_paused = true)]
    async fn test_learns_only_dialable_addresses() {
        let config = NetworkConfig::default();
        let mut target = OutgoingPeerTarget::new(&config, BlockedIps::default());
        let undialable = PeerId::random();
        let loopback: Multiaddr = "/ip4/127.0.0.1/tcp/6884".parse().unwrap();
        let private: Multiaddr = "/ip4/192.168.1.1/tcp/6884".parse().unwrap();
        let dns: Multiaddr = "/dns4/pool.example.com/tcp/6884".parse().unwrap();
        let quic: Multiaddr = "/ip4/1.2.3.4/udp/6884/quic-v1".parse().unwrap();
        target.learn_addresses(undialable, [&loopback, &private, &dns, &quic]);
        assert!(!target.knows(&undialable));

        let peer = PeerId::random();
        let with_p2p: Multiaddr = "/ip4/1.2.3.4/tcp/6884"
            .parse::<Multiaddr>()
            .unwrap()
            .with(Protocol::P2p(peer));
        target.learn_addresses(peer, [&with_p2p]);
        let dial = target.next_dial(OutgoingCounts::default(), |_| false);
        assert_eq!(
            dial.map(|dial| dial.address),
            Some("/ip4/1.2.3.4/tcp/6884".parse().unwrap())
        );
    }

    /// An onion address is learned and dialed only by a node with a Tor
    /// proxy, and holds an onion network group.
    #[tokio::test(start_paused = true)]
    async fn test_learns_onion_addresses_only_with_tor() {
        let onion: Multiaddr =
            "/onion3/2gzyxa5ihm7nsggfxnu52rck2vv4rvmdlkiu3zzui5du4xyclen53wid:6884"
                .parse()
                .unwrap();
        let peer = PeerId::random();

        let mut without_tor =
            OutgoingPeerTarget::new(&NetworkConfig::default(), BlockedIps::default());
        without_tor.learn_addresses(peer, [&onion]);
        assert!(!without_tor.knows(&peer));

        let config = NetworkConfig {
            tor_proxy: Some("127.0.0.1:9050".to_string()),
            ..NetworkConfig::default()
        };
        let mut with_tor = OutgoingPeerTarget::new(&config, BlockedIps::default());
        with_tor.learn_addresses(peer, [&onion]);
        let dial = with_tor.next_dial(OutgoingCounts::default(), |_| false);
        assert_eq!(dial.map(|dial| dial.address), Some(onion.clone()));
        assert!(matches!(
            network_group(&onion),
            Some(NetworkGroup::Onion(_))
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn test_never_dials_a_connected_peer() {
        let config = NetworkConfig::default();
        let mut target = OutgoingPeerTarget::new(&config, BlockedIps::default());
        let peer = PeerId::random();
        target.learn_addresses(peer, [&"/ip4/1.2.3.4/tcp/6884".parse().unwrap()]);

        assert_eq!(
            target.next_dial(OutgoingCounts::default(), |connected| *connected == peer),
            None
        );
    }

    #[tokio::test(start_paused = true)]
    async fn test_target_of_zero_never_dials() {
        let config = NetworkConfig {
            target_outgoing_peers: 0,
            ..NetworkConfig::default()
        };
        let mut target = OutgoingPeerTarget::new(&config, BlockedIps::default());
        target.learn_addresses(
            PeerId::random(),
            [&"/ip4/1.2.3.4/tcp/6884".parse().unwrap()],
        );

        assert!(!target.has_target());
        assert_eq!(target.next_dial(OutgoingCounts::default(), |_| false), None);
    }

    /// A full book drops an unverified peer for a new one and keeps the
    /// peer the node has reached.
    #[tokio::test(start_paused = true)]
    async fn test_full_book_drops_an_unverified_peer_for_a_new_one() {
        let config = NetworkConfig::default();
        let mut target =
            OutgoingPeerTarget::new(&config, BlockedIps::default()).with_known_peer_limit(2);
        let verified = PeerId::random();
        let unverified = PeerId::random();
        let newcomer = PeerId::random();
        target.learn_addresses(verified, [&"/ip4/1.2.3.4/tcp/6884".parse().unwrap()]);
        target.learn_addresses(unverified, [&"/ip4/5.6.7.8/tcp/6884".parse().unwrap()]);
        target.record_connection_established(
            verified,
            ConnectionId::new_unchecked(1),
            &dialer("/ip4/1.2.3.4/tcp/6884"),
        );

        target.learn_addresses(newcomer, [&"/ip4/9.10.11.12/tcp/6884".parse().unwrap()]);

        assert!(target.knows(&verified));
        assert!(!target.knows(&unverified));
        assert!(target.knows(&newcomer));
    }

    #[tokio::test(start_paused = true)]
    async fn test_full_book_of_verified_peers_refuses_new_ones() {
        let config = NetworkConfig::default();
        let mut target =
            OutgoingPeerTarget::new(&config, BlockedIps::default()).with_known_peer_limit(1);
        let verified = PeerId::random();
        let newcomer = PeerId::random();
        target.learn_addresses(verified, [&"/ip4/1.2.3.4/tcp/6884".parse().unwrap()]);
        target.record_connection_established(
            verified,
            ConnectionId::new_unchecked(1),
            &dialer("/ip4/1.2.3.4/tcp/6884"),
        );

        target.learn_addresses(newcomer, [&"/ip4/5.6.7.8/tcp/6884".parse().unwrap()]);

        assert!(target.knows(&verified));
        assert!(!target.knows(&newcomer));
    }

    /// A saved book brings back every known peer with its verified mark, and
    /// the peers held over outgoing connections as anchors; dial_peers and
    /// addresses that are not dialable stay out.
    #[tokio::test(start_paused = true)]
    async fn test_a_saved_book_brings_back_peers_and_anchors() {
        let config = NetworkConfig {
            target_outgoing_peers: 4,
            ..NetworkConfig::default()
        };
        let mut target = OutgoingPeerTarget::new(&config, BlockedIps::default());
        let held = PeerId::random();
        let heard = PeerId::random();
        target.learn_addresses(held, [&"/ip4/1.2.3.4/tcp/6884".parse().unwrap()]);
        target.learn_addresses(heard, [&"/ip4/5.6.7.8/tcp/6884".parse().unwrap()]);
        connect_kept(&mut target, held, 1, "/ip4/1.2.3.4/tcp/6884");

        let mut book = target.saved_book();
        assert_eq!(book.anchors, vec![held.to_string()]);
        assert_eq!(book.peers[0].peer_id, held.to_string(), "verified first");
        assert!(book.peers[0].verified);
        assert!(!book.peers[1].verified);
        book.peers.push(SavedPeer {
            peer_id: PeerId::random().to_string(),
            addresses: vec!["/ip4/192.168.1.9/tcp/6884".into()],
            verified: true,
        });
        book.peers.push(SavedPeer {
            peer_id: "not a peer id".into(),
            addresses: vec!["/ip4/9.9.9.9/tcp/6884".into()],
            verified: false,
        });

        let mut restarted = OutgoingPeerTarget::new(&config, BlockedIps::default());
        restarted.load_book(book);

        assert!(restarted.knows(&held) && restarted.knows(&heard));
        assert_eq!(restarted.known_peers.len(), 2);
        assert!(restarted.known_peers[&held].verified);
        assert!(restarted.known_peers[&held].anchor);
        assert!(!restarted.known_peers[&heard].anchor);
    }

    /// On the next start the anchors are dialed before any other known peer,
    /// once each.
    #[tokio::test(start_paused = true)]
    async fn test_anchors_are_dialed_first_once() {
        let config = NetworkConfig {
            target_outgoing_peers: 8,
            ..NetworkConfig::default()
        };
        let anchor = PeerId::random();
        let mut peers: Vec<SavedPeer> = (0..20)
            .map(|index| SavedPeer {
                peer_id: PeerId::random().to_string(),
                addresses: vec![format!("/ip4/{}.1.1.1/tcp/6884", index + 1)],
                verified: true,
            })
            .collect();
        peers.push(SavedPeer {
            peer_id: anchor.to_string(),
            addresses: vec!["/ip4/100.1.1.1/tcp/6884".into()],
            verified: true,
        });
        let mut target = OutgoingPeerTarget::new(&config, BlockedIps::default());
        target.load_book(SavedBook {
            version: SAVED_BOOK_VERSION,
            peers,
            anchors: vec![anchor.to_string()],
        });

        let first = target
            .next_dial(OutgoingCounts::default(), |_| false)
            .unwrap();
        assert_eq!(first.peer_id, anchor);
        assert!(
            !target.known_peers[&anchor].anchor,
            "an anchor goes first once"
        );
        let second = target
            .next_dial(
                OutgoingCounts {
                    pending: 1,
                    established: 0,
                },
                |_| false,
            )
            .unwrap();
        assert_ne!(second.peer_id, anchor);
    }

    fn listener() -> ConnectedPoint {
        ConnectedPoint::Listener {
            local_addr: "/ip4/0.0.0.0/tcp/6884".parse().unwrap(),
            send_back_addr: "/ip4/5.6.7.8/tcp/51234".parse().unwrap(),
        }
    }

    /// An outgoing connection no dial of this node opened is Kademlia's
    /// lookup. Once it has settled it closes when no query runs, and the
    /// close is reported as the node's own.
    #[tokio::test(start_paused = true)]
    async fn test_a_lookup_connection_closes_after_its_query() {
        let mut target = OutgoingPeerTarget::new(&NetworkConfig::default(), BlockedIps::default());
        let lookup = ConnectionId::new_unchecked(1);
        target.record_connection_established(
            PeerId::random(),
            lookup,
            &dialer("/ip4/1.2.3.4/tcp/6884"),
        );

        advance(Duration::from_secs(LOOKUP_SETTLE_SECS - 1)).await;
        assert!(target.finished_lookups(false).is_empty(), "still settling");
        advance(Duration::from_secs(1)).await;
        assert!(
            target.finished_lookups(true).is_empty(),
            "a query still runs"
        );
        assert_eq!(target.finished_lookups(false), vec![lookup]);

        target.record_lookup_closing(lookup);
        assert!(target.finished_lookups(false).is_empty(), "asked once");
        assert!(target.record_connection_closed(lookup));
        assert_eq!(target.outgoing_peer_count(), 0);
    }

    /// No query runs longer than KADEMLIA_QUERY_TIMEOUT_SECS, so a lookup
    /// closes then even while queries keep running.
    #[tokio::test(start_paused = true)]
    async fn test_a_lookup_connection_closes_after_the_query_timeout() {
        let mut target = OutgoingPeerTarget::new(&NetworkConfig::default(), BlockedIps::default());
        let lookup = ConnectionId::new_unchecked(1);
        target.record_connection_established(
            PeerId::random(),
            lookup,
            &dialer("/ip4/1.2.3.4/tcp/6884"),
        );

        advance(Duration::from_secs(KADEMLIA_QUERY_TIMEOUT_SECS - 1)).await;
        assert!(target.finished_lookups(true).is_empty());
        advance(Duration::from_secs(1)).await;
        assert_eq!(target.finished_lookups(true), vec![lookup]);
    }

    /// A lookup neither fills the target nor holds its group: the target
    /// still dials a peer in the lookup's /16.
    #[tokio::test(start_paused = true)]
    async fn test_a_lookup_connection_neither_counts_nor_holds_a_group() {
        let config = NetworkConfig {
            target_outgoing_peers: 1,
            ..NetworkConfig::default()
        };
        let mut target = OutgoingPeerTarget::new(&config, BlockedIps::default());
        let lookup_peer = PeerId::random();
        target.record_connection_established(
            lookup_peer,
            ConnectionId::new_unchecked(1),
            &dialer("/ip4/1.2.3.4/tcp/6884"),
        );
        let same_group = PeerId::random();
        target.learn_addresses(same_group, [&"/ip4/1.2.9.9/tcp/6884".parse().unwrap()]);

        assert_eq!(target.outgoing_peer_count(), 1);
        let dial = target.next_dial(OutgoingCounts::default(), |peer| *peer == lookup_peer);
        assert_eq!(dial.map(|dial| dial.peer_id), Some(same_group));
        assert!(target.saved_book().anchors.is_empty());
    }

    /// The target's own peers, inbound peers, dial_peers and seeds' peers
    /// are never closed as lookups; a close the node did not ask for is not
    /// reported as its own.
    #[tokio::test(start_paused = true)]
    async fn test_kept_and_inbound_connections_never_close_as_lookups() {
        let config = NetworkConfig {
            dial_peers: vec!["/ip4/9.9.9.9/tcp/6884".to_string()],
            ..NetworkConfig::default()
        };
        let mut target = OutgoingPeerTarget::new(&config, BlockedIps::default());
        let target_peer = PeerId::random();
        target.learn_addresses(target_peer, [&"/ip4/1.2.3.4/tcp/6884".parse().unwrap()]);
        let dial = target
            .next_dial(OutgoingCounts::default(), |_| false)
            .unwrap();
        target.record_dial_started(ConnectionId::new_unchecked(1), dial);
        target.record_connection_established(
            target_peer,
            ConnectionId::new_unchecked(1),
            &dialer("/ip4/1.2.3.4/tcp/6884"),
        );
        target.record_connection_established(
            PeerId::random(),
            ConnectionId::new_unchecked(2),
            &listener(),
        );
        target.record_connection_established(
            PeerId::random(),
            ConnectionId::new_unchecked(3),
            &dialer("/ip4/9.9.9.9/tcp/6884"),
        );
        target.record_seed_dial_started(ConnectionId::new_unchecked(4));
        target.record_connection_established(
            PeerId::random(),
            ConnectionId::new_unchecked(4),
            &dialer("/ip4/5.6.7.8/tcp/6884"),
        );

        advance(Duration::from_secs(KADEMLIA_QUERY_TIMEOUT_SECS * 10)).await;
        assert!(target.finished_lookups(false).is_empty());
        assert_eq!(target.kept_peer_count(), 3, "target, dial_peer and seed");
        for connection in 1..=4 {
            assert!(!target.record_connection_closed(ConnectionId::new_unchecked(connection)));
        }
    }

    /// A connection Kademlia opens to an anchor before the target dials it
    /// is the target's: it holds the anchor's group and is never closed. An
    /// anchor in a group the target already holds is a lookup like any
    /// other.
    #[tokio::test(start_paused = true)]
    async fn test_a_connection_kademlia_opens_to_an_anchor_stays() {
        let anchor = PeerId::random();
        let crowded_anchor = PeerId::random();
        let saved = |peer_id: PeerId, address: &str| SavedPeer {
            peer_id: peer_id.to_string(),
            addresses: vec![address.into()],
            verified: true,
        };
        let mut target = OutgoingPeerTarget::new(&NetworkConfig::default(), BlockedIps::default());
        target.load_book(SavedBook {
            version: SAVED_BOOK_VERSION,
            peers: vec![
                saved(anchor, "/ip4/1.2.3.4/tcp/6884"),
                saved(crowded_anchor, "/ip4/1.2.9.9/tcp/6884"),
            ],
            anchors: vec![anchor.to_string(), crowded_anchor.to_string()],
        });

        target.record_connection_established(
            anchor,
            ConnectionId::new_unchecked(1),
            &dialer("/ip4/1.2.3.4/tcp/6884"),
        );
        target.record_connection_established(
            crowded_anchor,
            ConnectionId::new_unchecked(2),
            &dialer("/ip4/1.2.9.9/tcp/6884"),
        );

        advance(Duration::from_secs(LOOKUP_SETTLE_SECS)).await;
        assert_eq!(
            target.finished_lookups(false),
            vec![ConnectionId::new_unchecked(2)]
        );
        assert_eq!(target.kept_peer_count(), 1);
        assert!(!target.known_peers[&anchor].anchor, "taken once");
        assert_eq!(target.saved_book().anchors, vec![anchor.to_string()]);
    }

    /// A book from another format version is ignored, not misread.
    #[tokio::test(start_paused = true)]
    async fn test_a_book_of_another_version_is_ignored() {
        let mut target = OutgoingPeerTarget::new(&NetworkConfig::default(), BlockedIps::default());
        target.load_book(SavedBook {
            version: SAVED_BOOK_VERSION + 1,
            peers: vec![SavedPeer {
                peer_id: PeerId::random().to_string(),
                addresses: vec!["/ip4/1.2.3.4/tcp/6884".into()],
                verified: true,
            }],
            anchors: vec![],
        });
        assert!(target.known_peers.is_empty());
    }
}
