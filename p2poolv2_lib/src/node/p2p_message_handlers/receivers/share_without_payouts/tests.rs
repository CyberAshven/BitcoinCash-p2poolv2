// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use crate::accounting::payout::payout_levels::PplnsWeights;
use crate::accounting::payout::sharechain_pplns::pplns_window::WindowError;
use crate::config::Chain;
use crate::node::messages::ShareWithWeakHashes;
use crate::node::p2p_message_handlers::receivers::block_receiver::{
    BlockReceiverEvent, BlockReceiverReceiver, create_block_receiver_channel,
};
use crate::node::request_response_handler::block_fetcher::{
    BlockFetcherEvent, BlockFetcherReceiver, create_block_fetcher_channel,
};
use crate::node::validation_worker::{self, ValidationReceiver};
use crate::pool_difficulty::MockPoolDifficulty;
use crate::shares::share_block::ShareBlock;
use crate::shares::validation::DefaultShareValidator;
use crate::test_utils::{TestShareBlockBuilder, build_test_share_paying};
use bitcoin::hashes::Hash;
use bitcoin::{Address, BlockHash, CompactTarget, Network, PubkeyHash};
use p2poolv2_wallet::payout_address::PayoutAddress;
use p2poolv2_wallet::share_owner::ShareOwner;
use std::collections::HashMap;

/// The regtest P2PKH address of key hash `byte` x 20.
fn payee(byte: u8) -> PayoutAddress {
    PayoutAddress::from(Address::p2pkh(
        PubkeyHash::from_byte_array([byte; 20]),
        Network::Regtest,
    ))
}

/// Window weights paying the 100 payees from key hash `first` up, by their
/// key hash byte.
fn weights_from(first: u8) -> PplnsWeights {
    PplnsWeights::without_levels(
        (first..first + 100)
            .map(|byte| (payee(byte), u128::from(byte) * 1_000))
            .collect::<HashMap<_, _>>(),
    )
}

/// An honest Bitcoin Cash share whose coinbase pays `weights` of 3.125 BCH,
/// which the real validator's gate passes.
fn honest_share(weights: &PplnsWeights) -> ShareBlock {
    build_test_share_paying(
        TestShareBlockBuilder::new()
            .share_owner(ShareOwner::CashP2pkh([0x22; 20]))
            .payout_address(payee(0x22))
            .bits(CompactTarget::from_consensus(0x2100_ffff))
            .build(),
        weights,
        312_500_000,
        None,
    )
}

/// `share` as a peer relays it: without its coinbase's outputs.
fn without_payouts(share: &ShareBlock) -> ShareWithoutPayouts {
    ShareWithoutPayouts::new(ShareWithWeakHashes::new(share.clone()))
}

/// A window that gives `weights` at any anchor, up to `walks` times.
fn window_giving(weights: PplnsWeights, walks: usize) -> PplnsWindow {
    let mut window = PplnsWindow::default();
    window.expect_network().return_const(Network::Regtest);
    window
        .expect_get_weights_from_start_hash()
        .times(walks)
        .returning(move |_, _, _| Ok(weights.clone()));
    window
}

/// A chain store that has not stored the share, nor has the copy the
/// rebuild takes to the blocking pool.
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

/// A receiving node: the channels a relayed share goes on to, the real
/// validator and a coinbase rebuilder over `window`.
struct Receiver {
    validation_tx: ValidationSender,
    _validation_rx: ValidationReceiver,
    block_receiver_handle: BlockReceiverHandle,
    block_receiver_rx: BlockReceiverReceiver,
    block_fetcher_handle: BlockFetcherHandle,
    block_fetcher_rx: BlockFetcherReceiver,
    swarm_tx: mpsc::Sender<SwarmSend<u32>>,
    swarm_rx: mpsc::Receiver<SwarmSend<u32>>,
    share_validator: Arc<dyn ShareValidator + Send + Sync>,
    coinbase_rebuilder: CoinbaseRebuilder,
}

impl Receiver {
    fn with_window(window: PplnsWindow) -> Self {
        let (validation_tx, validation_rx) = validation_worker::create_validation_channel();
        let (block_receiver_handle, block_receiver_rx) = create_block_receiver_channel();
        let (block_fetcher_handle, block_fetcher_rx) = create_block_fetcher_channel();
        let (swarm_tx, swarm_rx) = mpsc::channel(8);
        Self {
            validation_tx,
            _validation_rx: validation_rx,
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
            coinbase_rebuilder: CoinbaseRebuilder::new(Arc::new(RwLock::new(window))),
        }
    }

    /// `peer` relays `share` to this node.
    async fn relay(
        &self,
        share: ShareWithoutPayouts,
        peer: PeerId,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        handle_share_without_payouts(
            share,
            peer,
            &store_without_the_share(),
            self.validation_tx.clone(),
            &self.block_receiver_handle,
            &self.block_fetcher_handle,
            &self.share_validator,
            None,
            Some(&self.coinbase_rebuilder),
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

    /// The share asked for whole from the block fetcher, and from whom.
    fn fetched_whole(&mut self) -> (Vec<BlockHash>, PeerId, bool) {
        match self
            .block_fetcher_rx
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
}

/// A share relayed without its payouts, to a node whose window gives the
/// weights its miner paid, gets its coinbase back and is handed on exactly
/// as its miner built it; nothing is fetched, and the rebuild is counted.
#[tokio::test]
async fn a_share_without_payouts_is_rebuilt_and_handed_on_whole() {
    let weights = weights_from(1);
    let honest = honest_share(&weights);
    let mut receiver = Receiver::with_window(window_giving(weights, 1));
    let accepted = receiver.accept_next_share();

    let result = receiver
        .relay(without_payouts(&honest), PeerId::random())
        .await;

    assert!(result.is_ok(), "{}", result.unwrap_err());
    assert_eq!(accepted.await.unwrap(), honest);
    assert!(matches!(
        receiver.block_fetcher_rx.try_recv(),
        Ok(BlockFetcherEvent::BlockRequestCompleted(hash)) if hash == honest.block_hash()
    ));
    assert!(receiver.block_fetcher_rx.try_recv().is_err());
    let counts = receiver.coinbase_rebuilder.counts();
    assert_eq!((counts.rebuilt, counts.fetched_whole), (1, 0));
    assert!(counts.max_micros <= counts.micros);
}

/// A node whose window gives other weights than the miner's rebuilds
/// another coinbase. The share is not refused and the peer kept: the share
/// is asked for whole from the peer that relayed it, never handed on.
#[tokio::test]
async fn a_share_whose_window_differs_here_is_fetched_whole_not_refused() {
    let honest = honest_share(&weights_from(1));
    let mut receiver = Receiver::with_window(window_giving(weights_from(101), 1));
    let peer = PeerId::random();

    let result = receiver.relay(without_payouts(&honest), peer).await;

    assert!(result.is_ok(), "{}", result.unwrap_err());
    assert_eq!(
        receiver.fetched_whole(),
        (vec![honest.block_hash()], peer, true)
    );
    assert!(receiver.block_receiver_rx.try_recv().is_err());
    assert!(receiver.swarm_rx.try_recv().is_err(), "no peer is dropped");
    let counts = receiver.coinbase_rebuilder.counts();
    assert_eq!((counts.rebuilt, counts.fetched_whole), (0, 1));
}

/// A node whose window cannot reach the share's parent yet asks for the
/// share whole as well.
#[tokio::test]
async fn a_share_whose_parent_the_window_cannot_reach_is_fetched_whole() {
    let honest = honest_share(&weights_from(1));
    let mut window = PplnsWindow::default();
    window.expect_network().return_const(Network::Regtest);
    window
        .expect_get_weights_from_start_hash()
        .returning(|anchor, _, _| {
            Err(WindowError::ReadFailure(
                format!("anchor {anchor} is not stored").into(),
            ))
        });
    let mut receiver = Receiver::with_window(window);
    let peer = PeerId::random();

    let result = receiver.relay(without_payouts(&honest), peer).await;

    assert!(result.is_ok(), "{}", result.unwrap_err());
    assert_eq!(
        receiver.fetched_whole(),
        (vec![honest.block_hash()], peer, true)
    );
    assert!(receiver.block_receiver_rx.try_recv().is_err());
    assert!(receiver.swarm_rx.try_recv().is_err(), "no peer is dropped");
}

/// A copy whose outputs' total or scriptSig a peer changed rebuilds a
/// coinbase with another txid than the header commits, so it is never
/// handed on or stored. The share is asked for whole instead: the receiver
/// cannot tell a changed copy from a window of its own that differs.
#[tokio::test]
async fn a_share_with_a_tampered_coinbase_is_never_handed_on() {
    let weights = weights_from(1);
    let honest = honest_share(&weights);
    let mut more_paid = honest.clone();
    more_paid.bitcoin_coinbase.output[1].value += bitcoin::Amount::from_sat(1);
    let mut other_extranonce = honest.clone();
    let mut script_sig = other_extranonce.bitcoin_coinbase.input[0]
        .script_sig
        .to_bytes();
    let last = script_sig.len() - 1;
    script_sig[last] ^= 1;
    other_extranonce.bitcoin_coinbase.input[0].script_sig = bitcoin::ScriptBuf::from(script_sig);
    let mut receiver = Receiver::with_window(window_giving(weights, 1));

    let first = PeerId::random();
    let second = PeerId::random();
    let more_paid_result = receiver.relay(without_payouts(&more_paid), first).await;
    let other_extranonce_result = receiver
        .relay(without_payouts(&other_extranonce), second)
        .await;

    assert!(more_paid_result.is_ok() && other_extranonce_result.is_ok());
    assert_eq!(
        receiver.fetched_whole(),
        (vec![honest.block_hash()], first, true)
    );
    assert_eq!(
        receiver.fetched_whole(),
        (vec![honest.block_hash()], second, true)
    );
    assert!(receiver.block_receiver_rx.try_recv().is_err());
    assert_eq!(receiver.coinbase_rebuilder.counts().fetched_whole, 2);
}

/// A copy whose header a peer changed fails the coinbase proof, as a share
/// sent whole would: it is refused before anything is rebuilt or fetched
/// for it, and the error disconnects the peer.
#[tokio::test]
async fn a_share_with_a_tampered_header_is_refused() {
    let weights = weights_from(1);
    let mut tampered = honest_share(&weights);
    tampered.header.time += 1;
    let mut receiver = Receiver::with_window(window_giving(weights, 0));

    let result = receiver
        .relay(without_payouts(&tampered), PeerId::random())
        .await;

    assert!(result.is_err());
    assert!(matches!(
        receiver.block_fetcher_rx.try_recv(),
        Ok(BlockFetcherEvent::BlockRequestCompleted(_))
    ));
    assert!(
        receiver.block_fetcher_rx.try_recv().is_err(),
        "nothing fetched"
    );
    assert!(receiver.block_receiver_rx.try_recv().is_err());
    assert_eq!(
        receiver.coinbase_rebuilder.counts(),
        RebuildCounts::default()
    );
}
