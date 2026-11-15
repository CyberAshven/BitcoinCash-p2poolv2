// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::node::messages::{Message, ShareWithWeakHashes};
use crate::node::request_response_handler::peer_block_knowledge::PeerBlockKnowledge;
use crate::node::request_sender::RequestSender;
use crate::shares::share_block::ShareBlock;
use bitcoin::block::Header;
use libp2p::PeerId;
use tracing::debug;

/// Share messages a node relayed to peers, their payload bytes, and the
/// bytes they would have taken with every weak share sent whole: what one
/// broadcast sent, and the totals since start for P2P health metrics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ShareRelayBytes {
    pub messages: u64,
    pub bytes: u64,
    pub whole_bytes: u64,
}

impl std::ops::AddAssign for ShareRelayBytes {
    fn add_assign(&mut self, other: Self) {
        self.messages += other.messages;
        self.bytes += other.bytes;
        self.whole_bytes += other.whole_bytes;
    }
}

/// Broadcast a new ShareBlock to all connected peers.
///
/// A share that carries weak shares goes as `ShareWithWeakHashes`, listing
/// them by proof-of-work hash: its peers have them from the weak share
/// gossip, and fill them from their weak pools. One that carries none goes
/// as `ShareBlock`, as before. Sync still sends weak shares whole
/// (`GetData::Block`, `GetShareBlocks`): a peer catching up was never sent
/// old weak shares.
///
/// Peers that are already known to have the block (tracked in
/// `peer_block_knowledge`) are skipped. Each recipient is recorded
/// in `peer_block_knowledge` so that subsequent broadcasts of the
/// same block are suppressed.
///
/// Sends directly via the swarm to avoid deadlocking the actor's
/// select loop (which would happen if we awaited on the bounded
/// swarm_tx channel that the actor also drains).
pub fn send_share_block_broadcast(
    share_block: ShareBlock,
    bch_headers: Vec<Header>,
    connected_peers: &[PeerId],
    peer_block_knowledge: &mut PeerBlockKnowledge,
    request_sender: &mut impl RequestSender,
) -> ShareRelayBytes {
    let block_hash = share_block.block_hash();
    let whole = Message::ShareBlock(share_block, bch_headers);
    let whole_bytes = whole.encoded_size() as u64;
    let message = relay_message(whole);
    let bytes = message.encoded_size() as u64;
    let message_type = message.message_type();
    let mut sent = ShareRelayBytes::default();

    for peer_id in connected_peers {
        if peer_block_knowledge.peer_knows_block(peer_id, &block_hash) {
            debug!("Skipping broadcast to peer {peer_id}: already knows block {block_hash}");
        } else {
            debug!("Broadcasting {message_type} {block_hash} to peer {peer_id}");
            request_sender.send_request(peer_id, message.clone());
            peer_block_knowledge.record_block_known(peer_id, block_hash);
            sent += ShareRelayBytes {
                messages: 1,
                bytes,
                whole_bytes,
            };
        }
    }
    sent
}

/// The message a new share is relayed in: `ShareWithWeakHashes` when it
/// carries weak shares, `ShareBlock` as it was otherwise.
fn relay_message(whole: Message) -> Message {
    match whole {
        Message::ShareBlock(share_block, bch_headers)
            if !share_block.weak_shares.is_empty()
                && crate::sim_overrides::relays_weak_shares_by_hash(&share_block) =>
        {
            Message::ShareWithWeakHashes(ShareWithWeakHashes::new(share_block), bch_headers)
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::request_sender::MockRequestSender;
    use crate::test_utils::TestShareBlockBuilder;
    use bitcoin::hashes::Hash;
    use p2poolv2_wallet::share_owner::ShareOwner;

    #[test]
    fn test_broadcast_to_all_peers() {
        let peer_a = PeerId::random();
        let peer_b = PeerId::random();
        let peer_c = PeerId::random();
        let share_block = TestShareBlockBuilder::new().build();
        let block_hash = share_block.block_hash();
        let mut knowledge = PeerBlockKnowledge::default();

        let mut mock_sender = MockRequestSender::new();
        mock_sender
            .expect_send_request()
            .times(3)
            .returning(|_, _| ());

        let connected = vec![peer_a, peer_b, peer_c];
        send_share_block_broadcast(
            share_block,
            Vec::new(),
            &connected,
            &mut knowledge,
            &mut mock_sender,
        );

        assert!(knowledge.peer_knows_block(&peer_a, &block_hash));
        assert!(knowledge.peer_knows_block(&peer_b, &block_hash));
        assert!(knowledge.peer_knows_block(&peer_c, &block_hash));
    }

    #[test]
    fn test_broadcast_filters_known_peers() {
        let peer_a = PeerId::random();
        let peer_b = PeerId::random();
        let share_block = TestShareBlockBuilder::new().build();
        let block_hash = share_block.block_hash();

        let mut knowledge = PeerBlockKnowledge::default();
        knowledge.record_block_known(&peer_a, block_hash);

        let mut mock_sender = MockRequestSender::new();
        mock_sender
            .expect_send_request()
            .withf(move |sent_peer_id, _| *sent_peer_id == peer_b)
            .times(1)
            .returning(|_, _| ());

        let connected = vec![peer_a, peer_b];
        send_share_block_broadcast(
            share_block,
            Vec::new(),
            &connected,
            &mut knowledge,
            &mut mock_sender,
        );

        assert!(knowledge.peer_knows_block(&peer_b, &block_hash));
    }

    #[test]
    fn test_broadcast_skips_all_known_peers() {
        let peer_a = PeerId::random();
        let peer_b = PeerId::random();
        let share_block = TestShareBlockBuilder::new().build();
        let block_hash = share_block.block_hash();

        let mut knowledge = PeerBlockKnowledge::default();
        knowledge.record_block_known(&peer_a, block_hash);
        knowledge.record_block_known(&peer_b, block_hash);

        let mut mock_sender = MockRequestSender::new();
        mock_sender.expect_send_request().never();

        let connected = vec![peer_a, peer_b];
        send_share_block_broadcast(
            share_block,
            Vec::new(),
            &connected,
            &mut knowledge,
            &mut mock_sender,
        );
    }

    /// A Bitcoin Cash share carrying two weak shares.
    fn share_carrying_weak_shares() -> ShareBlock {
        let weak_shares = (1..=2)
            .map(|nonce| crate::shares::weak::WeakShare {
                header: TestShareBlockBuilder::new()
                    .share_owner(ShareOwner::CashP2pkh([0x11; 20]))
                    .nonce(nonce)
                    .build()
                    .header,
                coinbase_script_sig: bitcoin::ScriptBuf::from_bytes(vec![0x51; 40]),
                coinbase_value: 625_000_000,
                merkle_branch: vec![bitcoin::TxMerkleNode::all_zeros(); 12],
            })
            .collect();
        TestShareBlockBuilder::new()
            .share_owner(ShareOwner::CashP2pkh([0x22; 20]))
            .weak_shares(weak_shares)
            .build()
    }

    /// A new share carrying weak shares is relayed with them listed by hash,
    /// and the broadcast reports the bytes it sent beside the bytes the
    /// whole share would have taken.
    #[test]
    fn test_broadcast_lists_carried_weak_shares_by_hash() {
        let peer_a = PeerId::random();
        let peer_b = PeerId::random();
        let share_block = share_carrying_weak_shares();
        let relayed =
            Message::ShareWithWeakHashes(ShareWithWeakHashes::new(share_block.clone()), Vec::new());
        let whole_bytes =
            Message::ShareBlock(share_block.clone(), Vec::new()).encoded_size() as u64;
        let bytes = relayed.encoded_size() as u64;
        let mut knowledge = PeerBlockKnowledge::default();

        let mut mock_sender = MockRequestSender::new();
        mock_sender
            .expect_send_request()
            .withf(move |_, message| *message == relayed)
            .times(2)
            .returning(|_, _| ());

        let sent = send_share_block_broadcast(
            share_block,
            Vec::new(),
            &[peer_a, peer_b],
            &mut knowledge,
            &mut mock_sender,
        );

        assert_eq!(
            sent,
            ShareRelayBytes {
                messages: 2,
                bytes: 2 * bytes,
                whole_bytes: 2 * whole_bytes,
            }
        );
        assert!(bytes < whole_bytes);
    }

    /// A share carrying no weak shares is relayed as it always was, and its
    /// bytes are its whole bytes.
    #[test]
    fn test_broadcast_sends_a_share_without_weak_shares_as_it_was() {
        let share_block = TestShareBlockBuilder::new().build();
        let whole = Message::ShareBlock(share_block.clone(), Vec::new());
        let whole_bytes = whole.encoded_size() as u64;
        let mut knowledge = PeerBlockKnowledge::default();

        let mut mock_sender = MockRequestSender::new();
        mock_sender
            .expect_send_request()
            .withf(move |_, message| *message == whole)
            .times(1)
            .returning(|_, _| ());

        let sent = send_share_block_broadcast(
            share_block,
            Vec::new(),
            &[PeerId::random()],
            &mut knowledge,
            &mut mock_sender,
        );

        assert_eq!(
            sent,
            ShareRelayBytes {
                messages: 1,
                bytes: whole_bytes,
                whole_bytes,
            }
        );
    }

    #[test]
    fn test_broadcast_no_connected_peers() {
        let share_block = TestShareBlockBuilder::new().build();
        let mut knowledge = PeerBlockKnowledge::default();

        let mut mock_sender = MockRequestSender::new();
        mock_sender.expect_send_request().never();

        send_share_block_broadcast(
            share_block,
            Vec::new(),
            &[],
            &mut knowledge,
            &mut mock_sender,
        );
    }
}
