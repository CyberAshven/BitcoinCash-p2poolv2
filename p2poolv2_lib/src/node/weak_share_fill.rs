// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Shares relayed with their weak shares listed by hash
//! (`Message::ShareWithWeakHashes`) that wait for the weak shares this
//! node's weak pool lacked.
//!
//! The receiver asks the peer that relayed the share for them
//! (`Message::GetCarriedWeakShares`), and the answer
//! (`Message::CarriedWeakShares`) is handed back here by the share's hash.
//! One share waits for one peer at a time: a second peer relaying it
//! meanwhile is remembered, up to a bound, as a peer to ask for the share
//! whole if the first does not answer. Every map here is capped, and a wait
//! ends after [`WEAK_FILL_TIMEOUT`] whether answered or not.

use crate::shares::weak::WeakShare;
use bitcoin::BlockHash;
use libp2p::PeerId;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::sync::oneshot;

/// How long a share waits for the weak shares it asked its sender for.
///
/// Ten seconds is the block fetcher's request timeout, which it keeps equal
/// to libp2p's, and the weak share fetch's (`WEAK_FETCH_TIMEOUT`).
pub const WEAK_FILL_TIMEOUT: Duration = Duration::from_secs(10);

/// Shares waiting for weak shares at once, across every peer. Past it a
/// share is asked for whole.
pub const MAX_WEAK_FILLS: usize = 64;

/// Shares waiting for weak shares from one peer at once. A peer relays a
/// new share every few seconds, so a few cover a burst of uncles.
pub const MAX_WEAK_FILLS_PER_PEER: usize = 8;

/// Other peers remembered for one waiting share, as #105 remembers the
/// announcers of one weak share (`MAX_ANNOUNCERS_PER_HASH`).
pub const MAX_OTHER_RELAYERS: usize = 8;

/// What asking to wait for a share's weak shares did.
#[derive(Debug)]
pub(crate) enum FillStart {
    /// The share waits for `peer`'s answer, which arrives on this receiver.
    Started(oneshot::Receiver<Vec<WeakShare>>),
    /// The share already waits for another peer's answer. This peer is
    /// remembered as another that relayed it.
    AlreadyWaiting,
    /// Too many shares wait, in all or for this peer.
    Full,
}

/// One share waiting for its sender's answer.
struct WaitingFill {
    peer: PeerId,
    /// Taken when the answer arrives; the entry stays until the waiting task
    /// finishes, so a relay meanwhile is still remembered.
    reply: Option<oneshot::Sender<Vec<WeakShare>>>,
    others: Vec<PeerId>,
}

/// The shares waiting for weak shares, by share hash. Cloning shares the
/// book: the request handlers start waits, the response dispatcher answers
/// them.
#[derive(Clone, Default)]
pub struct WeakShareFill {
    waiting: Arc<Mutex<HashMap<BlockHash, WaitingFill>>>,
}

impl WeakShareFill {
    fn lock(&self) -> MutexGuard<'_, HashMap<BlockHash, WaitingFill>> {
        self.waiting.lock().expect("weak share fill lock poisoned")
    }

    /// Wait for `peer`'s answer about the share `share_hash`, unless it
    /// already waits for another peer or the caps are reached.
    pub(crate) fn start(&self, share_hash: BlockHash, peer: PeerId) -> FillStart {
        let mut waiting = self.lock();
        if let Some(fill) = waiting.get_mut(&share_hash) {
            if fill.peer != peer
                && !fill.others.contains(&peer)
                && fill.others.len() < MAX_OTHER_RELAYERS
            {
                fill.others.push(peer);
            }
            return FillStart::AlreadyWaiting;
        }
        let from_peer = waiting.values().filter(|fill| fill.peer == peer).count();
        if waiting.len() >= MAX_WEAK_FILLS || from_peer >= MAX_WEAK_FILLS_PER_PEER {
            return FillStart::Full;
        }
        let (reply, answer) = oneshot::channel();
        waiting.insert(
            share_hash,
            WaitingFill {
                peer,
                reply: Some(reply),
                others: Vec::new(),
            },
        );
        FillStart::Started(answer)
    }

    /// `peer` answered with `weak_shares` for the share `share_hash`. False
    /// when no share waits for that peer's answer: a late answer, a second
    /// one, or one nobody asked for, which is dropped.
    pub(crate) fn answer(
        &self,
        peer: PeerId,
        share_hash: BlockHash,
        weak_shares: Vec<WeakShare>,
    ) -> bool {
        let reply = match self.lock().get_mut(&share_hash) {
            Some(fill) if fill.peer == peer => fill.reply.take(),
            _ => None,
        };
        match reply {
            Some(reply) => reply.send(weak_shares).is_ok(),
            None => false,
        }
    }

    /// End the wait for `share_hash`, answered or not, and return the other
    /// peers that relayed the share meanwhile.
    pub(crate) fn finish(&self, share_hash: BlockHash) -> Vec<PeerId> {
        self.lock()
            .remove(&share_hash)
            .map(|fill| fill.others)
            .unwrap_or_default()
    }

    /// Shares waiting now.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.lock().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::hashes::Hash;

    fn share_hash(byte: u8) -> BlockHash {
        BlockHash::from_byte_array([byte; 32])
    }

    /// A share waits for one peer. A second relayer is remembered behind it,
    /// once, and comes back when the wait ends; only the asked peer's answer
    /// is taken, and only once.
    #[tokio::test]
    async fn a_share_waits_for_the_peer_it_asked_and_remembers_the_others() {
        let book = WeakShareFill::default();
        let asked = PeerId::random();
        let other = PeerId::random();
        let FillStart::Started(answer) = book.start(share_hash(1), asked) else {
            panic!("the first relay starts a wait");
        };
        assert!(matches!(
            book.start(share_hash(1), other),
            FillStart::AlreadyWaiting
        ));
        assert!(matches!(
            book.start(share_hash(1), other),
            FillStart::AlreadyWaiting
        ));

        assert!(
            !book.answer(other, share_hash(1), Vec::new()),
            "a peer that was not asked cannot answer"
        );
        assert!(book.answer(asked, share_hash(1), Vec::new()));
        assert!(
            !book.answer(asked, share_hash(1), Vec::new()),
            "a second answer is dropped"
        );
        assert_eq!(answer.await.unwrap(), Vec::new());
        assert_eq!(book.finish(share_hash(1)), vec![other]);
        assert_eq!(book.len(), 0);
    }

    /// The caps: eight waits a peer, sixty-four in all.
    #[test]
    fn waits_stop_at_eight_a_peer_and_sixty_four_in_all() {
        let book = WeakShareFill::default();
        let peer = PeerId::random();
        let mut answers = Vec::new();
        for byte in 0..MAX_WEAK_FILLS_PER_PEER as u8 {
            match book.start(share_hash(byte), peer) {
                FillStart::Started(answer) => answers.push(answer),
                other => panic!("wait {byte} for one peer should start, got {other:?}"),
            }
        }
        assert!(matches!(
            book.start(share_hash(0xf0), peer),
            FillStart::Full
        ));

        for byte in MAX_WEAK_FILLS_PER_PEER as u8..MAX_WEAK_FILLS as u8 {
            match book.start(share_hash(byte), PeerId::random()) {
                FillStart::Started(answer) => answers.push(answer),
                other => panic!("wait {byte} should start, got {other:?}"),
            }
        }
        assert!(matches!(
            book.start(share_hash(0xf1), PeerId::random()),
            FillStart::Full
        ));

        book.finish(share_hash(0));
        assert!(matches!(
            book.start(share_hash(0xf0), peer),
            FillStart::Started(_)
        ));
    }

    /// An answer that arrives after its wait ended finds nothing to answer.
    #[test]
    fn an_answer_after_the_wait_ended_is_dropped() {
        let book = WeakShareFill::default();
        let peer = PeerId::random();
        let _answer = book.start(share_hash(1), peer);
        assert!(book.finish(share_hash(1)).is_empty());

        assert!(!book.answer(peer, share_hash(1), Vec::new()));
    }
}
