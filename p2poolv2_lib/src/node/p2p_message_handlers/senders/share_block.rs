// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::node::SwarmSend;
use crate::node::actor::WEAK_ANNOUNCE_INTERVAL;
use crate::node::messages::{Message, ShareWithWeakHashes};
use crate::node::request_response_handler::peer_block_knowledge::PeerBlockKnowledge;
use crate::node::request_sender::RequestSender;
#[cfg(test)]
#[mockall_double::double]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
#[cfg(not(test))]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
use crate::shares::share_block::ShareBlock;
use crate::shares::weak::WeakShare;
use crate::shares::weak_pool::WeakPool;
use bitcoin::BlockHash;
use bitcoin::block::Header;
use libp2p::PeerId;
use libp2p::request_response::ResponseChannel;
use std::collections::HashSet;
use tokio::sync::mpsc;
use tracing::debug;

/// How long this node must have held a carried weak share in its weak pool
/// before a share it relays lists that weak share by hash alone. A younger
/// one, or one not in the pool, is sent whole as well (prefilled), since its
/// peers may not have it yet.
///
/// 400 ms is two of the weak share announce intervals
/// (`WEAK_ANNOUNCE_INTERVAL`, 200 ms). A weak share a peer sent this node is
/// announced at the next interval, up to 200 ms after it was pooled; a peer
/// that lacks it then asks for it, one round trip, and checks it, which the
/// second interval covers on links of up to about 150 ms round trip. One
/// this node's miners found was pushed whole to every peer when it was
/// pooled, so it reaches them sooner. An older weak share a peer still lacks
/// is asked for, as before. The cost is the weak shares of the last 400 ms
/// sent whole: about 1.4 a share at chipnet's 3.5 weak shares a second, and
/// 14 at the 35 a second of 320 weak shares a share.
pub const PREFILL_WEAK_SHARE_MILLIS: u64 = 2 * WEAK_ANNOUNCE_INTERVAL.as_millis() as u64;

/// Share messages a node relayed to peers, their payload bytes, the bytes
/// they would have taken with every weak share sent whole, and the weak
/// shares they sent whole beside the hashes: what one broadcast sent, and the
/// totals since start for P2P health metrics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ShareRelayBytes {
    pub messages: u64,
    pub bytes: u64,
    pub whole_bytes: u64,
    pub prefilled: u64,
}

impl std::ops::AddAssign for ShareRelayBytes {
    fn add_assign(&mut self, other: Self) {
        self.messages += other.messages;
        self.bytes += other.bytes;
        self.whole_bytes += other.whole_bytes;
        self.prefilled += other.prefilled;
    }
}

/// The carried weak shares of `share_block` its peers may not have yet,
/// which a relay of it at `now_millis` sends whole: those `weak_pool` has
/// held for less than [`PREFILL_WEAK_SHARE_MILLIS`], and those it does not
/// hold, which this node met only inside a share. None without a weak pool.
pub fn weak_shares_to_prefill(
    share_block: &ShareBlock,
    weak_pool: Option<&WeakPool>,
    now_millis: u64,
) -> HashSet<BlockHash> {
    let Some(weak_pool) = weak_pool else {
        return HashSet::new();
    };
    let pow_hashes: Vec<BlockHash> = share_block
        .weak_shares
        .iter()
        .map(WeakShare::pow_hash)
        .collect();
    pow_hashes
        .iter()
        .zip(weak_pool.pooled_at_millis(&pow_hashes))
        .filter(|(_, pooled_at)| {
            pooled_at.is_none_or(|pooled_at| {
                now_millis.saturating_sub(pooled_at) < PREFILL_WEAK_SHARE_MILLIS
            })
        })
        .map(|(pow_hash, _)| *pow_hash)
        .collect()
}

/// Hand `share_block` to the node actor to relay to its peers
/// (`SwarmSend::BroadcastBlock`, sent by [`send_share_block_broadcast`]),
/// with the Bitcoin Cash headers it builds on that a peer may lack.
///
/// Called by the validation worker for a share this node mined, once it is
/// validated, and by the block receiver for a share a peer sent, once its
/// header checks out and before its body is validated. In the sim the
/// broadcast waits the simulated propagation delay first
/// (`sim_overrides::spawn_delayed_broadcast`).
pub async fn queue_share_block_broadcast(
    share_block: ShareBlock,
    chain_store_handle: &ChainStoreHandle,
    swarm_tx: &mpsc::Sender<SwarmSend<ResponseChannel<Message>>>,
) {
    #[cfg(not(feature = "sim"))]
    {
        let block_hash = share_block.block_hash();
        let bch_headers =
            chain_store_handle.carried_bch_headers(std::slice::from_ref(&share_block.header));
        if let Err(send_error) = swarm_tx
            .send(SwarmSend::BroadcastBlock(share_block, bch_headers))
            .await
        {
            tracing::error!("Failed to send broadcast for share block {block_hash}: {send_error}");
        }
    }
    #[cfg(feature = "sim")]
    {
        let _ = chain_store_handle;
        crate::sim_overrides::spawn_delayed_broadcast(swarm_tx.clone(), share_block);
    }
}

/// Broadcast a new ShareBlock to all connected peers.
///
/// A share that carries weak shares goes as `ShareWithWeakHashes`, listing
/// them by proof-of-work hash: its peers have most of them from the weak
/// share gossip, and fill them from their weak pools. The ones in `prefill`
/// (`weak_shares_to_prefill`) go whole beside the list. One that carries
/// none goes as `ShareBlock`, as before. Sync still sends weak shares whole
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
    prefill: &HashSet<BlockHash>,
    connected_peers: &[PeerId],
    peer_block_knowledge: &mut PeerBlockKnowledge,
    request_sender: &mut impl RequestSender,
) -> ShareRelayBytes {
    let block_hash = share_block.block_hash();
    let whole = Message::ShareBlock(share_block, bch_headers);
    let whole_bytes = whole.encoded_size() as u64;
    let message = relay_message(whole, prefill);
    let bytes = message.encoded_size() as u64;
    let prefilled = match &message {
        Message::ShareWithWeakHashes(share, _) => share.prefilled().len() as u64,
        _ => 0,
    };
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
                prefilled,
            };
        }
    }
    sent
}

/// The message a new share is relayed in: `ShareWithWeakHashes`, with the
/// weak shares in `prefill` whole, when it carries weak shares; `ShareBlock`
/// as it was otherwise.
fn relay_message(whole: Message, prefill: &HashSet<BlockHash>) -> Message {
    match whole {
        Message::ShareBlock(share_block, bch_headers)
            if !share_block.weak_shares.is_empty()
                && crate::sim_overrides::relays_weak_shares_by_hash(&share_block) =>
        {
            Message::ShareWithWeakHashes(
                ShareWithWeakHashes::prefilling(share_block, |weak_share| {
                    prefill.contains(&weak_share.pow_hash())
                }),
                bch_headers,
            )
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
            &HashSet::new(),
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
            &HashSet::new(),
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
            &HashSet::new(),
            &connected,
            &mut knowledge,
            &mut mock_sender,
        );
    }

    /// A Bitcoin Cash share carrying two weak shares.
    fn share_carrying_weak_shares() -> ShareBlock {
        share_carrying_weak_shares_of(2)
    }

    /// A Bitcoin Cash share carrying `count` weak shares.
    fn share_carrying_weak_shares_of(count: u32) -> ShareBlock {
        let weak_shares = (1..=count)
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
            &HashSet::new(),
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
                prefilled: 0,
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
            &HashSet::new(),
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
                prefilled: 0,
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
            &HashSet::new(),
            &[],
            &mut knowledge,
            &mut mock_sender,
        );
    }

    /// A relay prefills the carried weak shares its peers may lack: those
    /// the pool took less than 400 ms ago, and those it does not hold. One
    /// held exactly 400 ms is listed by hash alone.
    #[tokio::test]
    async fn test_weak_shares_to_prefill_are_the_young_and_the_unpooled() {
        let (store, _temp_dir) = crate::test_utils::setup_test_chain_store_handle(false).await;
        let (pool, _relay_rx) = WeakPool::new(store);
        let share_block = share_carrying_weak_shares_of(3);
        let [old, young, unpooled] = [0, 1, 2].map(|i| share_block.weak_shares[i].clone());
        let now = 1_000_000;
        pool.insert_at(
            old.clone(),
            1,
            crate::shares::weak_pool::WeakShareSource::Local,
            now - PREFILL_WEAK_SHARE_MILLIS,
        );
        pool.insert_at(
            young.clone(),
            1,
            crate::shares::weak_pool::WeakShareSource::Local,
            now - PREFILL_WEAK_SHARE_MILLIS + 1,
        );

        let prefill = weak_shares_to_prefill(&share_block, Some(&pool), now);

        assert_eq!(
            prefill,
            HashSet::from([young.pow_hash(), unpooled.pow_hash()])
        );
        assert!(!prefill.contains(&old.pow_hash()));
        assert!(weak_shares_to_prefill(&share_block, None, now).is_empty());
    }

    /// The prefilled weak shares go whole beside the full list of hashes, and
    /// the broadcast counts them for each peer it sent to.
    #[test]
    fn test_broadcast_prefills_the_weak_shares_it_is_given() {
        let share_block = share_carrying_weak_shares();
        let prefilled = share_block.weak_shares[1].clone();
        let prefill = HashSet::from([prefilled.pow_hash()]);
        let expected = ShareWithWeakHashes::prefilling(share_block.clone(), |weak_share| {
            weak_share.pow_hash() == prefilled.pow_hash()
        });
        assert_eq!(expected.prefilled(), [prefilled]);
        assert_eq!(expected.weak_hashes().len(), 2);
        let relayed = Message::ShareWithWeakHashes(expected, Vec::new());
        let bytes = relayed.encoded_size() as u64;
        let mut knowledge = PeerBlockKnowledge::default();

        let mut mock_sender = MockRequestSender::new();
        mock_sender
            .expect_send_request()
            .withf(move |_, message| *message == relayed)
            .times(2)
            .returning(|_, _| ());

        let sent = send_share_block_broadcast(
            share_block.clone(),
            Vec::new(),
            &prefill,
            &[PeerId::random(), PeerId::random()],
            &mut knowledge,
            &mut mock_sender,
        );

        assert_eq!(sent.messages, 2);
        assert_eq!(sent.bytes, 2 * bytes);
        assert_eq!(sent.prefilled, 2);
        assert!(sent.bytes < sent.whole_bytes);
    }
}
