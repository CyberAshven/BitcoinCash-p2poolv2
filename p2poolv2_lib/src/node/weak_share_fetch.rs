// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Which announced weak shares a node has asked a peer for.
//!
//! A hash is fetched from one peer at a time. Further announcements of it
//! are remembered, up to a bound, and asked only when the first peer does
//! not have the body, the body fails its check, or the ask goes unanswered.

use crate::shares::weak::WeakShare;
use crate::shares::weak_pool::{MAX_HELD_WEAK_SHARES, MAX_POOL_WEAK_SHARES};
use bitcoin::BlockHash;
use libp2p::PeerId;
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant};

/// Other peers remembered for one hash besides the peer being asked.
///
/// Eight failovers cover a mesh that announces the same hash. A further
/// announcement does not open another stream.
pub const MAX_ANNOUNCERS_PER_HASH: usize = 8;

/// Weak-share fetches open toward one peer at once.
///
/// The block fetcher uses the same cap under the default yamux stream
/// limit, so one inventory cannot fill a peer's streams.
pub const MAX_WEAK_FETCHES_PER_PEER: usize = 16;

/// Weak-share fetches open at once, across every peer.
///
/// 512 is 32 peers at the per-peer cap. Hashes past it wait instead of
/// opening a stream each.
pub const MAX_WEAK_FETCHES_IN_FLIGHT: usize = 512;

/// Hashes announced but not yet asked for.
///
/// The pool holds at most [`MAX_POOL_WEAK_SHARES`]. Remembering more
/// announcements than that cannot be served for long, so a further hash
/// is ignored until a later inventory offers it.
pub const MAX_WANTED_WEAK_SHARES: usize = MAX_POOL_WEAK_SHARES;

/// How long one fetch may stay unanswered before the next announcer is asked.
///
/// Ten seconds is the block fetcher's request timeout, which it keeps equal
/// to libp2p's. The worker polls this on its one-second tick.
pub const WEAK_FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// Bodies the worker is holding, so a second announcement does not fetch
/// another copy.
///
/// The pool itself holds at most [`MAX_HELD_WEAK_SHARES`] while it waits
/// for parents. The oldest record is dropped past that.
pub const MAX_HELD_FETCHES: usize = MAX_HELD_WEAK_SHARES;

/// A weak share to ask one peer for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WeakShareFetchSend {
    pub peer: PeerId,
    pub hash: BlockHash,
}

/// What the weak-share worker hears about a fetch, apart from the bodies
/// peers push on the existing weak-share channel.
///
/// Public because `handle_inventory` is public and forwards an announcement
/// on this type. The fields are public so the worker, the inventory handler
/// and the response dispatcher can build and read them.
// The body is the weak share. Boxing it would allocate on every fetched
// body, and the channel that carries this enum is already bounded.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum WeakFetchEvent {
    /// `peer` named these proof-of-work hashes.
    Announced {
        peer: PeerId,
        hashes: Vec<BlockHash>,
    },
    /// `peer` returned the body of a fetch.
    Body { peer: PeerId, weak_share: WeakShare },
    /// The peer that was asked does not have `hash`.
    NotFound { hash: BlockHash },
}

struct Wanted {
    next: PeerId,
    others: VecDeque<PeerId>,
}

struct InFlight {
    peer: PeerId,
    asked_at: Instant,
    others: VecDeque<PeerId>,
}

/// The fetch book. One task owns it. Every map here is capped.
pub(crate) struct WeakShareFetch {
    wanted: HashMap<BlockHash, Wanted>,
    wanted_order: VecDeque<BlockHash>,
    in_flight: HashMap<BlockHash, InFlight>,
    per_peer: HashMap<PeerId, usize>,
    /// Announcers kept after a body arrives, until the check says whether
    /// to ask the next peer. At most one entry per in-flight fetch.
    parked: HashMap<BlockHash, VecDeque<PeerId>>,
    parked_order: VecDeque<BlockHash>,
    holding: HashSet<BlockHash>,
    holding_order: VecDeque<BlockHash>,
}

impl WeakShareFetch {
    pub(crate) fn new() -> Self {
        Self {
            wanted: HashMap::new(),
            wanted_order: VecDeque::new(),
            in_flight: HashMap::new(),
            per_peer: HashMap::new(),
            parked: HashMap::new(),
            parked_order: VecDeque::new(),
            holding: HashSet::new(),
            holding_order: VecDeque::new(),
        }
    }

    /// Record that `peer` announced `hash`.
    ///
    /// `has_seen` means the pool already checked a copy, so nothing is
    /// fetched. A hash already in flight or waiting only gains `peer` as
    /// a later announcer.
    pub(crate) fn note(&mut self, peer: PeerId, hash: BlockHash, has_seen: bool) {
        if has_seen || self.holding.contains(&hash) {
            return;
        }
        if let Some(inflight) = self.in_flight.get_mut(&hash) {
            remember_announcer(&mut inflight.others, peer, Some(inflight.peer));
            return;
        }
        if let Some(wanted) = self.wanted.get_mut(&hash) {
            remember_announcer(&mut wanted.others, peer, Some(wanted.next));
            return;
        }
        if let Some(others) = self.parked.get_mut(&hash) {
            remember_announcer(others, peer, None);
            return;
        }
        if self.wanted_order.len() >= MAX_WANTED_WEAK_SHARES {
            return;
        }
        self.wanted.insert(
            hash,
            Wanted {
                next: peer,
                others: VecDeque::new(),
            },
        );
        self.wanted_order.push_back(hash);
    }

    /// Asks to send, for hashes that are waiting and whose peer has a free
    /// slot. Never more than the free in-flight slots.
    pub(crate) fn take_next(&mut self, now: Instant) -> Vec<WeakShareFetchSend> {
        let mut sends = Vec::new();
        let mut deferred = VecDeque::new();
        while self.in_flight.len() < MAX_WEAK_FETCHES_IN_FLIGHT {
            let pending = self.wanted_order.len();
            if pending == 0 {
                break;
            }
            let mut started = 0usize;
            for _ in 0..pending {
                let Some(hash) = self.wanted_order.pop_front() else {
                    break;
                };
                let Some(mut wanted) = self.wanted.remove(&hash) else {
                    continue;
                };
                if self.in_flight.len() >= MAX_WEAK_FETCHES_IN_FLIGHT
                    || !self.ask_a_free_announcer(&mut wanted)
                {
                    deferred.push_back((hash, wanted));
                    continue;
                }
                self.start(hash, wanted, now, &mut sends);
                started += 1;
            }
            while let Some((hash, wanted)) = deferred.pop_front() {
                self.wanted.insert(hash, wanted);
                self.wanted_order.push_back(hash);
            }
            if started == 0 {
                break;
            }
        }
        sends
    }

    /// The body arrived. The in-flight slot is freed before the pool admits
    /// the share. Announcers stay parked so a failed check can ask the next
    /// one. A hash that was only waiting is forgotten: the body is here.
    pub(crate) fn body_arrived(&mut self, hash: BlockHash) {
        if let Some(inflight) = self.in_flight.remove(&hash) {
            self.release_peer(inflight.peer);
            self.park(hash, inflight.others);
        }
        self.remove_wanted(hash);
    }

    /// The peer we asked does not have the body. Ask the next announcer.
    pub(crate) fn not_found(&mut self, hash: BlockHash, now: Instant) -> Vec<WeakShareFetchSend> {
        let Some(inflight) = self.in_flight.remove(&hash) else {
            return Vec::new();
        };
        self.release_peer(inflight.peer);
        self.requeue(hash, inflight.others);
        self.take_next(now)
    }

    /// The body failed its consensus check. Ask the next announcer. The
    /// pool does not mark that hash seen, so another copy can still pass.
    pub(crate) fn invalid(&mut self, hash: BlockHash, now: Instant) -> Vec<WeakShareFetchSend> {
        let others = self.take_parked(hash);
        if let Some(inflight) = self.in_flight.remove(&hash) {
            self.release_peer(inflight.peer);
            self.requeue(hash, inflight.others);
        } else {
            self.requeue(hash, others);
        }
        self.take_next(now)
    }

    /// The peer who delivered the body was over its rate, so the body was
    /// dropped unchecked. Ask the next announcer: its copy counts against
    /// its own rate, as a body it relayed whole would.
    pub(crate) fn rate_limited(
        &mut self,
        hash: BlockHash,
        now: Instant,
    ) -> Vec<WeakShareFetchSend> {
        let others = self.take_parked(hash);
        if let Some(inflight) = self.in_flight.remove(&hash) {
            self.release_peer(inflight.peer);
            self.requeue(hash, inflight.others);
        } else {
            self.requeue(hash, others);
        }
        self.take_next(now)
    }

    /// The check passed, or the body will not be asked for again. Drop any
    /// parked announcers.
    pub(crate) fn settled(&mut self, hash: BlockHash) {
        self.take_parked(hash);
        if let Some(inflight) = self.in_flight.remove(&hash) {
            self.release_peer(inflight.peer);
        }
    }

    /// The body is held for its parent. Do not fetch another copy. When the
    /// hold expires, [`Self::retain_holds`] forgets it and a later
    /// announcement may fetch.
    pub(crate) fn held(&mut self, hash: BlockHash) {
        self.settled(hash);
        if self.holding.contains(&hash) {
            return;
        }
        while self.holding_order.len() >= MAX_HELD_FETCHES {
            let Some(oldest) = self.holding_order.pop_front() else {
                break;
            };
            self.holding.remove(&oldest);
        }
        self.holding.insert(hash);
        self.holding_order.push_back(hash);
    }

    /// Forget held hashes for which `still_held` is false. Those holds
    /// expired, or the share was pooled, so a later announcement may fetch
    /// one that is no longer in hand.
    pub(crate) fn retain_holds(&mut self, mut still_held: impl FnMut(&BlockHash) -> bool) {
        let expired: Vec<BlockHash> = self
            .holding_order
            .iter()
            .filter(|hash| !still_held(hash))
            .copied()
            .collect();
        for hash in expired {
            self.forget_hold(hash);
        }
    }

    /// Move fetches that have waited longer than [`WEAK_FETCH_TIMEOUT`] to
    /// the next announcer. A fetch asked exactly that long ago is still in
    /// time; the next tick, a nanosecond past, is not.
    pub(crate) fn poll(&mut self, now: Instant) -> Vec<WeakShareFetchSend> {
        let timed_out: Vec<BlockHash> = self
            .in_flight
            .iter()
            .filter(|(_, inflight)| {
                now.saturating_duration_since(inflight.asked_at) > WEAK_FETCH_TIMEOUT
            })
            .map(|(hash, _)| *hash)
            .collect();
        for hash in timed_out {
            let Some(inflight) = self.in_flight.remove(&hash) else {
                continue;
            };
            self.release_peer(inflight.peer);
            self.requeue(hash, inflight.others);
        }
        self.take_next(now)
    }

    /// How many announcers are stored for `hash` behind the one in flight.
    #[cfg(test)]
    pub(crate) fn waiting_announcers(&self, hash: &BlockHash) -> usize {
        self.in_flight
            .get(hash)
            .map(|inflight| inflight.others.len())
            .unwrap_or(0)
    }

    /// Make `wanted.next` a peer with a free fetch slot: the peer it names,
    /// or else the first later announcer that has one, so hashes do not queue
    /// behind one peer that answers slowly or not at all. False when every
    /// announcer is at [`MAX_WEAK_FETCHES_PER_PEER`].
    fn ask_a_free_announcer(&self, wanted: &mut Wanted) -> bool {
        if self.has_free_slot(&wanted.next) {
            return true;
        }
        let Some(position) = wanted
            .others
            .iter()
            .position(|peer| self.has_free_slot(peer))
        else {
            return false;
        };
        let Some(free) = wanted.others.remove(position) else {
            return false;
        };
        let busy = std::mem::replace(&mut wanted.next, free);
        wanted.others.push_back(busy);
        true
    }

    fn has_free_slot(&self, peer: &PeerId) -> bool {
        self.per_peer.get(peer).copied().unwrap_or(0) < MAX_WEAK_FETCHES_PER_PEER
    }

    fn start(
        &mut self,
        hash: BlockHash,
        wanted: Wanted,
        now: Instant,
        sends: &mut Vec<WeakShareFetchSend>,
    ) {
        *self.per_peer.entry(wanted.next).or_default() += 1;
        sends.push(WeakShareFetchSend {
            peer: wanted.next,
            hash,
        });
        self.in_flight.insert(
            hash,
            InFlight {
                peer: wanted.next,
                asked_at: now,
                others: wanted.others,
            },
        );
    }

    fn requeue(&mut self, hash: BlockHash, mut others: VecDeque<PeerId>) {
        let Some(next) = others.pop_front() else {
            return;
        };
        if self.wanted.contains_key(&hash) {
            return;
        }
        if self.wanted_order.len() >= MAX_WANTED_WEAK_SHARES
            && let Some(oldest) = self.wanted_order.pop_front()
        {
            self.wanted.remove(&oldest);
        }
        if self.wanted_order.len() >= MAX_WANTED_WEAK_SHARES {
            return;
        }
        self.wanted.insert(hash, Wanted { next, others });
        self.wanted_order.push_back(hash);
    }

    fn park(&mut self, hash: BlockHash, others: VecDeque<PeerId>) {
        if let Some(parked) = self.parked.get_mut(&hash) {
            *parked = others;
            return;
        }
        while self.parked_order.len() >= MAX_WEAK_FETCHES_IN_FLIGHT {
            let Some(oldest) = self.parked_order.pop_front() else {
                break;
            };
            self.parked.remove(&oldest);
        }
        self.parked.insert(hash, others);
        self.parked_order.push_back(hash);
    }

    fn take_parked(&mut self, hash: BlockHash) -> VecDeque<PeerId> {
        self.parked_order.retain(|parked| *parked != hash);
        self.parked.remove(&hash).unwrap_or_default()
    }

    fn remove_wanted(&mut self, hash: BlockHash) {
        if self.wanted.remove(&hash).is_some() {
            self.wanted_order.retain(|wanted| *wanted != hash);
        }
    }

    fn forget_hold(&mut self, hash: BlockHash) {
        if self.holding.remove(&hash) {
            self.holding_order.retain(|held| *held != hash);
        }
    }

    fn release_peer(&mut self, peer: PeerId) {
        // `remove` needs the map. The `get_mut` borrow ends before that.
        let finished = match self.per_peer.get_mut(&peer) {
            Some(count) => {
                *count = count.saturating_sub(1);
                *count == 0
            }
            None => return,
        };
        if finished {
            self.per_peer.remove(&peer);
        }
    }
}

fn remember_announcer(others: &mut VecDeque<PeerId>, peer: PeerId, current: Option<PeerId>) {
    if current == Some(peer) || others.contains(&peer) {
        return;
    }
    if others.len() >= MAX_ANNOUNCERS_PER_HASH {
        return;
    }
    others.push_back(peer);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::hashes::Hash;

    fn hash_h() -> BlockHash {
        BlockHash::from_byte_array([0x11; 32])
    }

    fn peer() -> PeerId {
        PeerId::random()
    }

    /// A peer with every fetch slot taken does not hold back a hash another
    /// announcer can serve: the hash is asked from that announcer, and the
    /// busy peer stays behind it as a later choice.
    #[test]
    fn a_hash_is_asked_from_an_announcer_with_a_free_slot() {
        let now = Instant::now();
        let busy = peer();
        let free = peer();
        let mut book = WeakShareFetch::new();
        for byte in 0..MAX_WEAK_FETCHES_PER_PEER as u8 {
            book.note(busy, BlockHash::from_byte_array([byte; 32]), false);
        }
        assert_eq!(
            book.take_next(now).len(),
            MAX_WEAK_FETCHES_PER_PEER,
            "the busy peer has every slot taken"
        );

        let extra = BlockHash::from_byte_array([0xee; 32]);
        book.note(busy, extra, false);
        assert!(
            book.take_next(now).is_empty(),
            "only the busy peer announced it: it waits"
        );
        book.note(free, extra, false);
        let sends = book.take_next(now);

        assert_eq!(
            sends,
            vec![WeakShareFetchSend {
                peer: free,
                hash: extra,
            }],
            "asked from the announcer with a free slot"
        );
        assert_eq!(
            book.waiting_announcers(&extra),
            1,
            "the busy peer stays as a later choice"
        );
    }

    #[test]
    fn two_announcers_fetch_once_and_the_list_stops_at_eight() {
        let now = Instant::now();
        let hash = hash_h();
        let peers: Vec<PeerId> = (0..10).map(|_| peer()).collect();
        let mut book = WeakShareFetch::new();

        book.note(peers[0], hash, false);
        book.note(peers[1], hash, false);
        let sends = book.take_next(now);

        assert_eq!(
            sends,
            vec![WeakShareFetchSend {
                peer: peers[0],
                hash,
            }],
            "two announcers of hash H produce one GetData, to the first"
        );
        assert_eq!(
            book.waiting_announcers(&hash),
            1,
            "the second announcer of hash H is stored"
        );

        for announcer in peers.iter().skip(2).take(7) {
            book.note(*announcer, hash, false);
        }
        assert_eq!(
            book.waiting_announcers(&hash),
            8,
            "announcers three through nine of hash H fill the list to 8"
        );
        book.note(peers[9], hash, false);
        assert_eq!(
            book.waiting_announcers(&hash),
            8,
            "a tenth announcer of hash H does not grow the list past 8"
        );

        let sends = book.not_found(hash, now);
        assert_eq!(
            sends,
            vec![WeakShareFetchSend {
                peer: peers[1],
                hash,
            }],
            "not_found from the first peer produces one GetData to the second"
        );

        let mut seen = WeakShareFetch::new();
        seen.note(peers[0], hash, true);
        assert!(
            seen.take_next(now).is_empty(),
            "has_seen produces no GetData for hash H"
        );
    }

    /// A body refused for its sender's rate is asked from the next announcer,
    /// and when none is known yet, from the next peer that announces it.
    #[test]
    fn a_body_refused_for_the_rate_is_asked_from_the_next_announcer() {
        let now = Instant::now();
        let hash = hash_h();
        let first = peer();
        let second = peer();
        let later = peer();
        let mut book = WeakShareFetch::new();
        book.note(first, hash, false);
        book.note(second, hash, false);
        assert_eq!(book.take_next(now).len(), 1, "hash H is asked once");

        book.body_arrived(hash);
        let sends = book.rate_limited(hash, now);
        assert_eq!(
            sends,
            vec![WeakShareFetchSend { peer: second, hash }],
            "hash H is asked from the second announcer"
        );

        book.body_arrived(hash);
        assert!(
            book.rate_limited(hash, now).is_empty(),
            "no announcer of hash H is left to ask"
        );
        book.note(later, hash, false);
        assert_eq!(
            book.take_next(now),
            vec![WeakShareFetchSend { peer: later, hash }],
            "a later announcement of hash H fetches"
        );
    }

    #[test]
    fn poll_fails_over_only_after_ten_seconds() {
        let now = Instant::now();
        let hash = hash_h();
        let first = peer();
        let second = peer();
        let mut book = WeakShareFetch::new();
        book.note(first, hash, false);
        book.note(second, hash, false);
        let _ = book.take_next(now);

        let at_timeout = now + WEAK_FETCH_TIMEOUT;
        assert!(
            book.poll(at_timeout).is_empty(),
            "poll at 10 seconds does not return hash H"
        );

        let past = at_timeout + Duration::from_nanos(1);
        let sends = book.poll(past);
        assert_eq!(
            sends,
            vec![WeakShareFetchSend { peer: second, hash }],
            "poll at 10 seconds plus one nanosecond asks the next announcer for hash H"
        );
    }

    #[test]
    fn body_arrived_then_poll_produces_nothing() {
        let now = Instant::now();
        let hash = hash_h();
        let mut book = WeakShareFetch::new();
        book.note(peer(), hash, false);
        book.note(peer(), hash, false);
        let _ = book.take_next(now);

        book.body_arrived(hash);
        let past = now + WEAK_FETCH_TIMEOUT + Duration::from_nanos(1);
        assert!(
            book.poll(past).is_empty(),
            "body_arrived then poll produces nothing for hash H"
        );
    }
}
