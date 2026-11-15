// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use crate::config::Chain;
use crate::node::p2p_message_handlers::receivers::block_receiver::{
    BlockReceiverEvent, BlockReceiverReceiver, create_block_receiver_channel,
};
use crate::node::p2p_message_handlers::receivers::getdata::handle_getdata_block;
use crate::node::request_response_handler::block_fetcher::{
    BlockFetcherReceiver, create_block_fetcher_channel,
};
use crate::node::validation_worker::{self, ValidationReceiver};
use crate::node::weak_share_worker::WeakShareNetwork;
use crate::pool_difficulty::MockPoolDifficulty;
use crate::shares::share_block::ShareBlock;
use crate::shares::validation::DefaultShareValidator;
use crate::shares::weak_pool::WeakShareSource;
use crate::test_utils::{TestShareBlockBuilder, setup_test_chain_store_handle};
use bitcoin::hashes::Hash;
use bitcoin::{Address, CompactTarget, Network, PubkeyHash, ScriptBuf, TxMerkleNode};
use mockall::predicate::eq;
use p2poolv2_wallet::payout_address::PayoutAddress;
use p2poolv2_wallet::share_owner::ShareOwner;
use std::time::Duration;

/// A weak share carried by the test share, told apart by `nonce`.
fn carried_weak_share(nonce: u32) -> WeakShare {
    WeakShare {
        header: TestShareBlockBuilder::new()
            .share_owner(ShareOwner::CashP2pkh([0x11; 20]))
            .nonce(nonce)
            .build()
            .header,
        coinbase_script_sig: ScriptBuf::from_bytes(vec![0x51; 40]),
        coinbase_value: 625_000_000,
        merkle_branch: vec![TxMerkleNode::from_byte_array([0x33; 32])],
    }
}

/// An honest Bitcoin Cash share carrying two weak shares, which the real
/// validator's gate passes, as the share block tests build it.
fn honest_share() -> ShareBlock {
    TestShareBlockBuilder::new()
        .share_owner(ShareOwner::CashP2pkh([0x22; 20]))
        .payout_address(PayoutAddress::from(Address::p2pkh(
            PubkeyHash::from_byte_array([0x22; 20]),
            Network::Regtest,
        )))
        .bits(CompactTarget::from_consensus(0x2100_ffff))
        .weak_shares(vec![carried_weak_share(1), carried_weak_share(2)])
        .build()
}

/// A chain store that has not stored the share, nor has the copy a share
/// waiting for its weak shares takes.
fn store_without_the_share() -> ChainStoreHandle {
    let mut store = ChainStoreHandle::default();
    store.expect_share_block_exists().returning(|_| false);
    store.expect_clone().returning(|| {
        let mut clone = ChainStoreHandle::default();
        clone.expect_share_block_exists().returning(|_| false);
        clone
    });
    store
}

/// A receiving node: its weak pool and fill book, the channels a relayed
/// share goes on to, and the real validator.
struct Receiver {
    _temp_dir: tempfile::TempDir,
    weak_share_tx: WeakShareEventSender,
    pool: WeakPool,
    validation_tx: ValidationSender,
    validation_rx: ValidationReceiver,
    block_receiver_handle: BlockReceiverHandle,
    block_receiver_rx: BlockReceiverReceiver,
    block_fetcher_handle: BlockFetcherHandle,
    block_fetcher_rx: BlockFetcherReceiver,
    swarm_tx: mpsc::Sender<SwarmSend<u32>>,
    swarm_rx: mpsc::Receiver<SwarmSend<u32>>,
    share_validator: Arc<dyn ShareValidator + Send + Sync>,
}

impl Receiver {
    /// A receiver whose weak pool holds `pooled`. The pool never writes to
    /// its store here, so no writer task runs, and paused time can advance.
    async fn with_pooled(pooled: &[WeakShare]) -> Self {
        let (pool_store, temp_dir) = setup_test_chain_store_handle(false).await;
        let (pool, relay_rx) = WeakPool::new(pool_store);
        for weak_share in pooled {
            pool.insert(weak_share.clone(), 1, WeakShareSource::Local);
        }
        let network = WeakShareNetwork::new(pool.clone(), relay_rx);
        let (validation_tx, validation_rx) = validation_worker::create_validation_channel();
        let (block_receiver_handle, block_receiver_rx) = create_block_receiver_channel();
        let (block_fetcher_handle, block_fetcher_rx) = create_block_fetcher_channel();
        let (swarm_tx, swarm_rx) = mpsc::channel(8);
        Self {
            _temp_dir: temp_dir,
            weak_share_tx: network.events_tx,
            pool,
            validation_tx,
            validation_rx,
            block_receiver_handle,
            block_receiver_rx,
            block_fetcher_handle,
            block_fetcher_rx,
            swarm_tx,
            swarm_rx,
            share_validator: Arc::new(
                DefaultShareValidator::new(MockPoolDifficulty::default())
                    .for_chain(Chain::BitcoinCash)
                    .with_target_ceiling(CompactTarget::from_consensus(0x2100_ffff)),
            ),
        }
    }

    /// `peer` relays `share` to this node.
    async fn relay(
        &self,
        share: ShareWithWeakHashes,
        peer: PeerId,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        handle_share_with_weak_hashes(
            share,
            peer,
            &store_without_the_share(),
            self.validation_tx.clone(),
            &self.block_receiver_handle,
            &self.block_fetcher_handle,
            &self.share_validator,
            Some(&self.weak_share_tx),
            self.swarm_tx.clone(),
        )
        .await
    }

    /// Accept the next share the block receiver is handed, in a task that
    /// returns it.
    fn accept_next_share(&mut self) -> tokio::task::JoinHandle<ShareBlock> {
        let mut block_receiver_rx = std::mem::replace(
            &mut self.block_receiver_rx,
            create_block_receiver_channel().1,
        );
        tokio::spawn(async move {
            match block_receiver_rx.recv().await {
                Some(BlockReceiverEvent::ShareBlockReceived {
                    share_block,
                    result_tx,
                }) => {
                    let _ = result_tx.send(Ok(()));
                    share_block
                }
                _ => panic!("Expected a share at the BlockReceiver"),
            }
        })
    }
}

/// The share asked for whole from the block fetcher, and from whom.
fn fetched_whole(block_fetcher_rx: &mut BlockFetcherReceiver) -> (Vec<BlockHash>, PeerId, bool) {
    match block_fetcher_rx
        .try_recv()
        .expect("Expected the share to be asked for whole")
    {
        BlockFetcherEvent::FetchBlocks {
            blockhashes,
            peer_id,
            use_peer,
        } => (blockhashes, peer_id, use_peer),
        other => panic!("Expected FetchBlocks, got {other}"),
    }
}

/// A share whose weak shares are all in the receiver's pool is rebuilt from
/// it and handed on whole, exactly the share the miner committed, without
/// asking the sender for anything.
#[tokio::test]
async fn a_share_relayed_by_hash_is_rebuilt_from_the_pool_and_handed_on_whole() {
    let honest = honest_share();
    let mut receiver = Receiver::with_pooled(&honest.weak_shares).await;
    let accepted = receiver.accept_next_share();

    let result = receiver
        .relay(ShareWithWeakHashes::new(honest.clone()), PeerId::random())
        .await;

    assert!(result.is_ok(), "{}", result.unwrap_err());
    assert_eq!(accepted.await.unwrap(), honest);
    assert!(
        receiver.swarm_rx.try_recv().is_err(),
        "nothing is asked of the sender"
    );
    let counts = receiver.pool.counts();
    assert_eq!(counts.filled_from_pool, 2);
    assert_eq!(counts.fetched_for_shares, 0);
    assert_eq!(counts.fills_fetched_whole, 0);
}

/// A weak share the pool lacks is asked from the sender by hash, and the
/// share is handed on whole once the sender's answer fills it.
#[tokio::test]
async fn a_missing_weak_share_is_fetched_from_the_sender() {
    let honest = honest_share();
    let block_hash = honest.block_hash();
    let missing = honest.weak_shares[1].clone();
    let mut receiver = Receiver::with_pooled(&honest.weak_shares[..1]).await;
    let accepted = receiver.accept_next_share();
    let sender = PeerId::random();

    let result = receiver
        .relay(ShareWithWeakHashes::new(honest.clone()), sender)
        .await;

    assert!(result.is_ok(), "{}", result.unwrap_err());
    match receiver.swarm_rx.try_recv() {
        Ok(SwarmSend::Request(peer, Message::GetCarriedWeakShares(share_hash, weak_hashes))) => {
            assert_eq!(peer, sender);
            assert_eq!(share_hash, block_hash);
            assert_eq!(weak_hashes, vec![missing.pow_hash()]);
        }
        other => panic!("Expected GetCarriedWeakShares to the sender, got {other:?}"),
    }
    assert!(
        receiver
            .weak_share_tx
            .fill()
            .answer(sender, block_hash, vec![missing])
    );

    assert_eq!(accepted.await.unwrap(), honest);
    let counts = receiver.pool.counts();
    assert_eq!(counts.filled_from_pool, 1);
    assert_eq!(counts.fetched_for_shares, 1);
    assert_eq!(counts.fills_fetched_whole, 0);
}

/// A sender listing the share's weak shares in another order lists hashes
/// that do not rebuild the header's weak root. The share is not checked, so
/// never marked invalid, and the sender is not dropped for it: the share is
/// asked for whole, from that sender, as a share announced by its header.
#[tokio::test]
async fn hashes_that_do_not_rebuild_the_weak_root_get_the_share_asked_for_whole() {
    let honest = honest_share();
    let block_hash = honest.block_hash();
    let mut reordered = honest.clone();
    reordered.weak_shares.swap(0, 1);
    let mut receiver = Receiver::with_pooled(&honest.weak_shares).await;
    let sender = PeerId::random();

    let result = receiver
        .relay(ShareWithWeakHashes::new(reordered), sender)
        .await;

    assert!(result.is_ok(), "{}", result.unwrap_err());
    assert!(
        receiver.block_receiver_rx.try_recv().is_err(),
        "a share that does not rebuild is never buffered or stored"
    );
    assert!(
        receiver.validation_rx.try_recv().is_err(),
        "a share that does not rebuild is never validated, so never marked Invalid"
    );
    assert_eq!(
        fetched_whole(&mut receiver.block_fetcher_rx),
        (vec![block_hash], sender, true)
    );
    assert_eq!(receiver.pool.counts().fills_fetched_whole, 1);
}

/// A sender that answers with an altered copy of the missing weak share --
/// the same proof-of-work hash, a coinbase value one satoshi higher -- does
/// not complete the share: it is asked for whole, never checked.
#[tokio::test]
async fn an_altered_weak_share_from_the_sender_gets_the_share_asked_for_whole() {
    let honest = honest_share();
    let block_hash = honest.block_hash();
    let mut altered = honest.weak_shares[1].clone();
    altered.coinbase_value += 1;
    let mut receiver = Receiver::with_pooled(&honest.weak_shares[..1]).await;
    let sender = PeerId::random();

    receiver
        .relay(ShareWithWeakHashes::new(honest), sender)
        .await
        .unwrap();
    assert!(matches!(
        receiver.swarm_rx.try_recv(),
        Ok(SwarmSend::Request(_, Message::GetCarriedWeakShares(..)))
    ));
    assert!(
        receiver
            .weak_share_tx
            .fill()
            .answer(sender, block_hash, vec![altered])
    );

    let event = tokio::time::timeout(Duration::from_secs(5), receiver.block_fetcher_rx.recv())
        .await
        .expect("the share is asked for whole")
        .unwrap();
    assert!(matches!(
        event,
        BlockFetcherEvent::FetchBlocks { ref blockhashes, peer_id, use_peer: true }
            if *blockhashes == vec![block_hash] && peer_id == sender
    ));
    assert!(receiver.block_receiver_rx.try_recv().is_err());
    assert!(receiver.validation_rx.try_recv().is_err());
}

/// A sender that does not answer within `WEAK_FILL_TIMEOUT` is treated as a
/// slow peer: the share is asked for whole, from another peer that relayed
/// it meanwhile, whose own relay started no second wait.
#[tokio::test(start_paused = true)]
async fn a_sender_that_does_not_answer_in_time_gets_the_share_asked_from_another() {
    let honest = honest_share();
    let block_hash = honest.block_hash();
    let mut receiver = Receiver::with_pooled(&honest.weak_shares[..1]).await;
    let slow = PeerId::random();
    let other = PeerId::random();

    receiver
        .relay(ShareWithWeakHashes::new(honest.clone()), slow)
        .await
        .unwrap();
    receiver
        .relay(ShareWithWeakHashes::new(honest), other)
        .await
        .unwrap();
    assert!(matches!(
        receiver.swarm_rx.try_recv(),
        Ok(SwarmSend::Request(peer, Message::GetCarriedWeakShares(..))) if peer == slow
    ));
    assert!(
        receiver.swarm_rx.try_recv().is_err(),
        "the second relay waits on the first"
    );

    let event = receiver.block_fetcher_rx.recv().await.unwrap();
    assert!(matches!(
        event,
        BlockFetcherEvent::FetchBlocks { ref blockhashes, peer_id, use_peer: true }
            if *blockhashes == vec![block_hash] && peer_id == other
    ));
    assert_eq!(receiver.weak_share_tx.fill().len(), 0);
    assert!(receiver.validation_rx.try_recv().is_err());
}

/// A list that does not hold exactly the weak shares the header counts is
/// from a bad peer: refused, and nothing is fetched for it.
#[tokio::test]
async fn a_list_that_does_not_match_the_header_count_is_refused() {
    let honest = honest_share();
    let mut short = honest.clone();
    short.weak_shares.truncate(1);
    let mut receiver = Receiver::with_pooled(&honest.weak_shares).await;

    let result = receiver
        .relay(ShareWithWeakHashes::new(short), PeerId::random())
        .await;

    let error = result.unwrap_err().to_string();
    assert!(error.contains("lists 1 weak shares"), "{error}");
    assert!(receiver.swarm_rx.try_recv().is_err());
    assert!(receiver.block_fetcher_rx.try_recv().is_err());
    assert!(receiver.block_receiver_rx.try_recv().is_err());
}

/// The sender answers from its store with the weak shares asked for, and
/// with none for a share it does not have.
#[tokio::test]
async fn the_sender_answers_with_the_weak_shares_asked_for() {
    let honest = honest_share();
    let block_hash = honest.block_hash();
    let unknown = BlockHash::from_byte_array([0x44; 32]);
    let wanted = honest.weak_shares[1].clone();
    let mut store = ChainStoreHandle::default();
    let stored = honest.clone();
    store
        .expect_get_share()
        .with(eq(block_hash))
        .returning(move |_| Some(stored.clone()));
    store
        .expect_get_share()
        .with(eq(unknown))
        .returning(|_| None);
    let (swarm_tx, mut swarm_rx) = mpsc::channel::<SwarmSend<u32>>(2);

    handle_get_carried_weak_shares(
        block_hash,
        vec![wanted.pow_hash()],
        store,
        7,
        swarm_tx.clone(),
    )
    .await
    .unwrap();
    assert!(matches!(
        swarm_rx.recv().await,
        Some(SwarmSend::Response(7, Message::CarriedWeakShares(share_hash, ref weak_shares)))
            if share_hash == block_hash && *weak_shares == vec![wanted.clone()]
    ));

    let mut store = ChainStoreHandle::default();
    store.expect_get_share().returning(|_| None);
    handle_get_carried_weak_shares(unknown, vec![wanted.pow_hash()], store, 8, swarm_tx)
        .await
        .unwrap();
    assert!(matches!(
        swarm_rx.recv().await,
        Some(SwarmSend::Response(8, Message::CarriedWeakShares(share_hash, ref weak_shares)))
            if share_hash == unknown && weak_shares.is_empty()
    ));
}

/// Sync is unchanged: a share asked for by hash, as a peer catching up asks
/// for it, is sent with its weak shares whole.
#[tokio::test]
async fn a_share_asked_for_in_sync_is_sent_with_its_weak_shares_whole() {
    let honest = honest_share();
    let block_hash = honest.block_hash();
    let mut store = ChainStoreHandle::default();
    let stored = honest.clone();
    store
        .expect_get_share()
        .returning(move |_| Some(stored.clone()));
    store.expect_carried_bch_headers().returning(|_| Vec::new());
    let (swarm_tx, mut swarm_rx) = mpsc::channel::<SwarmSend<u32>>(1);

    handle_getdata_block(block_hash, store, 1, swarm_tx)
        .await
        .unwrap();

    match swarm_rx.recv().await {
        Some(SwarmSend::Response(_, Message::ShareBlock(share_block, _))) => {
            assert_eq!(share_block.weak_shares.len(), 2);
            assert_eq!(share_block, honest);
        }
        other => panic!("Expected the share whole, got {other:?}"),
    }
}
