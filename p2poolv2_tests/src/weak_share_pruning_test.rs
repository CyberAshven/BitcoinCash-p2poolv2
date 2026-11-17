// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Whole weak shares pruned one payout depth below the tip, with their leaf
//! root and summary kept until the body is pruned, through the real store,
//! writer, getdata handler, message encoding, body check and PPLNS window.
//!
//! The store's own depths are Bitcoin Cash's, far longer than a test chain,
//! so each test passes the prune ranges itself. The window tests scale the
//! payout depth down with the window: a 2-share maturity skip, an 8-share
//! cap and an uncle's 3, 13 shares, sized as `BCH_PAYOUT_DEPTH` is from
//! `BCH_COINBASE_MATURITY`, `PPLNS_WINDOW_SHARES` and `MAX_UNCLES_DEPTH`.
//! Every bound is a count of shares, so the scaled chain has the shape of
//! the real one.

use bitcoin::consensus::encode::{deserialize, serialize};
use bitcoin::hashes::Hash;
use bitcoin::{Address, BlockHash, CompactTarget, PubkeyHash, ScriptBuf};
use p2poolv2_lib::accounting::payout::sharechain_pplns::PplnsWindow;
use p2poolv2_lib::address::share_owner::ShareOwner;
use p2poolv2_lib::config::{BitcoinCashNetwork, Chain, PayoutAddress};
use p2poolv2_lib::node::SwarmSend;
use p2poolv2_lib::node::messages::Message;
use p2poolv2_lib::node::p2p_message_handlers::receivers::getdata::handle_getdata_block;
use p2poolv2_lib::pool_difficulty::PoolDifficulty;
use p2poolv2_lib::shares::chain::chain_store_handle::ChainStoreHandle;
use p2poolv2_lib::shares::share_block::ShareBlock;
use p2poolv2_lib::shares::validation::{DefaultShareValidator, FailureKind, ShareValidator};
use p2poolv2_lib::shares::weak::{PrunedWeakShares, WeakShare, WeakSummary};
use p2poolv2_lib::store::Store;
use p2poolv2_lib::store::dag_store::MAX_UNCLES_DEPTH;
use p2poolv2_lib::store::writer::{StoreHandle, StoreWriter, write_channel};
use p2poolv2_lib::test_utils::TestShareBlockBuilder;
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::tempdir;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Network the test shares' payout addresses use.
const NETWORK: bitcoin::Network = bitcoin::Network::Regtest;
/// Base timestamp for the share chain; each share advances it by one.
const FIRST_SHARE_TIME: u32 = 1_700_000_000;
/// A weak ceiling far above every target here.
const CEILING: u32 = 0x207f_ffff;
/// A response channel stand-in; the handlers only pass it through.
const RESPONSE_CHANNEL: u32 = 1;

/// The scaled maturity skip, window cap and payout depth of the window
/// tests: 2 + 8 + `MAX_UNCLES_DEPTH`.
const MATURITY: usize = 2;
const WINDOW: usize = 8;
const PAYOUT_DEPTH: u32 = (MATURITY + WINDOW) as u32 + MAX_UNCLES_DEPTH as u32;
/// The tip of the pruned node, three scaled payout depths above genesis.
const TIP: u32 = 3 * PAYOUT_DEPTH;
/// Whole weak shares are pruned below one payout depth under the tip, and
/// bodies below two (the scaled chain leaves out the pruner's lag).
const WEAK_SHARE_PRUNE_TARGET: u32 = TIP - PAYOUT_DEPTH;
const BODY_PRUNE_TARGET: u32 = TIP - 2 * PAYOUT_DEPTH;
/// The deepest anchor a payout is built on: the bottom of a syncing node's
/// full-validation zone, which is also the reorg floor, one payout depth
/// below the tip.
const DEEPEST_ANCHOR: u32 = TIP - PAYOUT_DEPTH;

/// The payout address of the share at `height`.
fn share_payee(height: u32) -> PayoutAddress {
    Address::p2pkh(PubkeyHash::from_byte_array([height as u8; 20]), NETWORK).into()
}

/// The payout address of the weak share the share at `height` carries.
fn weak_payee(height: u32) -> PayoutAddress {
    Address::p2pkh(
        PubkeyHash::from_byte_array([0x80 | height as u8; 20]),
        NETWORK,
    )
    .into()
}

/// The share at `height` on `parent`, carrying one weak share mined on the
/// same parent and paying its own payee.
fn carrier(parent: &ShareBlock, height: u32) -> ShareBlock {
    let weak_share = WeakShare {
        header: TestShareBlockBuilder::new()
            .share_owner(ShareOwner::CashP2pkh([0x33; 20]))
            .payout_address(weak_payee(height))
            .prev_share_blockhash(parent.block_hash().to_string())
            .nonce(0x7700_0000 + height)
            .time(FIRST_SHARE_TIME + height)
            .build()
            .header,
        coinbase_script_sig: ScriptBuf::new(),
        coinbase_value: 1,
        merkle_branch: Vec::new(),
    };
    TestShareBlockBuilder::new()
        .share_owner(ShareOwner::CashP2pkh([0x22; 20]))
        .payout_address(share_payee(height))
        .prev_share_blockhash(parent.block_hash().to_string())
        .nonce(0x6600_0000 + height)
        .time(FIRST_SHARE_TIME + height)
        .weak_shares(vec![weak_share])
        .build()
}

/// Genesis and `length` carriers on it, genesis first.
fn chain(length: u32) -> Vec<ShareBlock> {
    let genesis = TestShareBlockBuilder::new()
        .share_owner(ShareOwner::CashP2pkh([0x22; 20]))
        .payout_address(share_payee(0))
        .nonce(0x6600_0000)
        .time(FIRST_SHARE_TIME)
        .build();
    let mut blocks = Vec::with_capacity(length as usize + 1);
    blocks.push(genesis);
    for height in 1..=length {
        let parent = blocks.last().unwrap();
        blocks.push(carrier(parent, height));
    }
    blocks
}

/// A Bitcoin Cash node's store at `path`, pruning whole weak shares as a
/// node does by default, and its running writer.
fn open_node(path: &str) -> (ChainStoreHandle, JoinHandle<()>) {
    let store = Arc::new(
        Store::new(path.to_string(), false)
            .unwrap()
            .with_chain(Chain::BitcoinCash),
    );
    let (write_tx, write_rx) = write_channel();
    let store_writer = StoreWriter::new(store.clone(), write_rx);
    let writer = tokio::task::spawn_blocking(move || store_writer.run());
    let chain_store_handle = ChainStoreHandle::new(StoreHandle::new(store, write_tx), NETWORK)
        .with_bitcoin_cash_network(Some(BitcoinCashNetwork::Regtest));
    (chain_store_handle, writer)
}

/// Store, organise, mark valid and confirm `blocks`, genesis first.
async fn confirm(chain_store_handle: &ChainStoreHandle, blocks: &[ShareBlock]) {
    chain_store_handle
        .init_or_setup_genesis(blocks[0].clone())
        .await
        .unwrap();
    for block in &blocks[1..] {
        chain_store_handle
            .add_share_block(block.clone())
            .await
            .unwrap();
        chain_store_handle
            .organise_header(block.header.clone())
            .await
            .unwrap();
        chain_store_handle
            .mark_block_valid(block.block_hash())
            .await
            .unwrap();
        chain_store_handle.organise_block().await.unwrap();
    }
}

/// A node holding the whole chain to `TIP`, its whole weak shares pruned
/// below `WEAK_SHARE_PRUNE_TARGET` and its bodies below `BODY_PRUNE_TARGET`,
/// as the pruner leaves a node at that tip.
async fn pruned_node(path: &str) -> (ChainStoreHandle, JoinHandle<()>, Vec<ShareBlock>) {
    let blocks = chain(TIP);
    let (chain_store_handle, writer) = open_node(path);
    confirm(&chain_store_handle, &blocks).await;
    chain_store_handle
        .prune_block_bodies(1, BODY_PRUNE_TARGET)
        .await
        .unwrap();
    chain_store_handle
        .prune_weak_shares(1, WEAK_SHARE_PRUNE_TARGET)
        .await
        .unwrap();
    (chain_store_handle, writer, blocks)
}

/// The body a node serves for `block_hash` through the real getdata
/// handler, sent over the wire and read back.
async fn served(chain_store_handle: &ChainStoreHandle, block_hash: BlockHash) -> ShareBlock {
    let (swarm_tx, mut swarm_rx) = mpsc::channel::<SwarmSend<u32>>(1);
    handle_getdata_block(
        block_hash,
        chain_store_handle.clone(),
        RESPONSE_CHANNEL,
        swarm_tx,
    )
    .await
    .unwrap();
    let Some(SwarmSend::Response(_, message)) = swarm_rx.recv().await else {
        panic!("expected a response");
    };
    let Message::ShareBlock(share_block, _) = deserialize::<Message>(&serialize(&message)).unwrap()
    else {
        panic!("expected a ShareBlock response");
    };
    share_block
}

fn bitcoin_cash_validator() -> DefaultShareValidator {
    DefaultShareValidator::new(PoolDifficulty::new(
        CompactTarget::from_consensus(0x1d00ffff),
        FIRST_SHARE_TIME,
        0,
    ))
    .for_chain(Chain::BitcoinCash)
}

/// A Bitcoin Cash window crediting weak work, at the scaled maturity skip
/// and window cap.
fn scaled_window() -> PplnsWindow {
    PplnsWindow::new(NETWORK)
        .for_chain(Chain::BitcoinCash)
        .with_weak_ceiling(CompactTarget::from_consensus(CEILING))
        .with_maturity_skip(MATURITY)
        .with_max_window_shares(WINDOW)
}

/// The weights a share on `anchor` divides, for a paying block claiming
/// `claimed_difficulty`, read through a fresh window over the node.
fn payout(
    chain_store_handle: &ChainStoreHandle,
    anchor: &ShareBlock,
    claimed_difficulty: u128,
) -> HashMap<PayoutAddress, u128> {
    let mut window = scaled_window();
    window.update(chain_store_handle).unwrap();
    window
        .get_distribution_from_start_hash(
            anchor.block_hash(),
            claimed_difficulty.into(),
            chain_store_handle,
        )
        .unwrap()
}

/// A node syncing from the pruned node: its confirmed tip at
/// `DEEPEST_ANCHOR`, the bottom of its full-validation zone. It holds no
/// weak share data below `BODY_PRUNE_TARGET`, whose bodies it never fetched
/// (stored here without their weak part, so a read of them would find no
/// summary), and above it whatever the pruned node serves: summaries below
/// `WEAK_SHARE_PRUNE_TARGET`, whole weak shares from it up.
async fn syncing_node(
    path: &str,
    pruned_node: &ChainStoreHandle,
    blocks: &[ShareBlock],
) -> (ChainStoreHandle, JoinHandle<()>) {
    let mut received = Vec::with_capacity(DEEPEST_ANCHOR as usize + 1);
    received.push(blocks[0].clone());
    for height in 1..=DEEPEST_ANCHOR {
        let block = if height < BODY_PRUNE_TARGET {
            ShareBlock {
                weak_shares: Vec::new(),
                pruned_weak_shares: None,
                ..blocks[height as usize].clone()
            }
        } else {
            served(pruned_node, blocks[height as usize].block_hash()).await
        };
        received.push(block);
    }
    let (chain_store_handle, writer) = open_node(path);
    confirm(&chain_store_handle, &received).await;
    (chain_store_handle, writer)
}

/// A node that pruned the whole weak shares of a share serves its body with
/// their leaf root and summary. A syncing node reads it off the wire, checks
/// it against the header and stores the summary, which matches the one the
/// whole weak shares give.
#[tokio::test]
async fn a_pruned_node_serves_the_summary_and_a_syncing_node_checks_and_stores_it() {
    let temp_dir = tempdir().unwrap();
    let (node, _writer, blocks) = pruned_node(temp_dir.path().to_str().unwrap()).await;
    let pruned = &blocks[WEAK_SHARE_PRUNE_TARGET as usize - 1];

    let body = served(&node, pruned.block_hash()).await;

    assert!(body.weak_shares.is_empty());
    assert_eq!(
        body.pruned_weak_shares,
        Some(PrunedWeakShares::of(&pruned.weak_shares))
    );
    bitcoin_cash_validator()
        .validate_body_matches_header(&body)
        .unwrap();

    let sync_dir = tempdir().unwrap();
    let (syncing, _sync_writer) =
        syncing_node(sync_dir.path().to_str().unwrap(), &node, &blocks).await;
    let summaries = syncing.get_weak_summaries(&[pruned.block_hash()]).unwrap();
    assert_eq!(
        summaries[&pruned.block_hash()],
        WeakSummary::of(&pruned.weak_shares)
    );
    let stored = syncing.get_share(&pruned.block_hash()).unwrap();
    assert_eq!(stored, body, "the syncing node serves it on as it got it");
}

/// A summary a peer changed, here the payout level of the weak share's
/// credit, fails the weak root after the trip over the wire: a bad copy.
#[tokio::test]
async fn a_summary_tampered_with_by_the_serving_peer_is_a_bad_copy() {
    let temp_dir = tempdir().unwrap();
    let (node, _writer, blocks) = pruned_node(temp_dir.path().to_str().unwrap()).await;
    let mut body = served(
        &node,
        blocks[WEAK_SHARE_PRUNE_TARGET as usize - 1].block_hash(),
    )
    .await;
    body.pruned_weak_shares.as_mut().unwrap().summary.groups[0].payout_level += 1;
    let body: ShareBlock = deserialize(&serialize(&body)).unwrap();

    let error = bitcoin_cash_validator()
        .validate_body_matches_header(&body)
        .unwrap_err();

    assert_eq!(error.kind(), FailureKind::Consensus);
    assert!(error.to_string().contains("root"), "{error}");
}

/// The pruner deletes whole weak shares below its target and not one share
/// sooner, keeps every summary down to the body prune target, and a rerun
/// finds nothing more to delete.
#[tokio::test]
async fn whole_weak_shares_go_below_the_target_and_not_one_share_sooner() {
    let temp_dir = tempdir().unwrap();
    let (node, _writer, blocks) = pruned_node(temp_dir.path().to_str().unwrap()).await;
    let store = node.store_handle().store();
    let last_pruned = &blocks[WEAK_SHARE_PRUNE_TARGET as usize - 1];
    let first_kept = &blocks[WEAK_SHARE_PRUNE_TARGET as usize];
    let deepest_summary = &blocks[BODY_PRUNE_TARGET as usize];

    assert!(
        store
            .get_weak_shares(&last_pruned.block_hash())
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store.get_weak_shares(&first_kept.block_hash()).unwrap(),
        first_kept.weak_shares
    );
    assert_eq!(
        store
            .get_pruned_weak_shares(&deepest_summary.block_hash())
            .unwrap(),
        Some(PrunedWeakShares::of(&deepest_summary.weak_shares))
    );
    assert_eq!(
        node.prune_weak_shares(1, WEAK_SHARE_PRUNE_TARGET)
            .await
            .unwrap(),
        0
    );
}

/// After a restart the node still serves the pruned form, reads the
/// summaries and holds the whole weak shares it kept.
#[tokio::test]
async fn a_restarted_node_serves_and_reads_its_pruned_weak_shares() {
    let temp_dir = tempdir().unwrap();
    let path = temp_dir.path().to_str().unwrap().to_string();
    let (node, writer, blocks) = pruned_node(&path).await;
    drop(node);
    writer.await.unwrap();

    let (reopened, _writer) = open_node(&path);
    reopened
        .init_or_setup_genesis(blocks[0].clone())
        .await
        .unwrap();
    let pruned = &blocks[WEAK_SHARE_PRUNE_TARGET as usize - 1];
    let kept = &blocks[WEAK_SHARE_PRUNE_TARGET as usize];

    let body = served(&reopened, pruned.block_hash()).await;
    assert_eq!(
        body.pruned_weak_shares,
        Some(PrunedWeakShares::of(&pruned.weak_shares))
    );
    bitcoin_cash_validator()
        .validate_body_matches_header(&body)
        .unwrap();
    assert_eq!(served(&reopened, kept.block_hash()).await, *kept);
    assert_eq!(
        reopened.get_weak_summaries(&[pruned.block_hash()]).unwrap()[&pruned.block_hash()],
        WeakSummary::of(&pruned.weak_shares)
    );
}

/// A window at its share cap, anchored at the bottom of a syncing node's
/// full-validation zone, pays shares whose whole weak shares the network
/// pruned. Every one of them still has its summary, served by the pruned
/// node, and the payout is the one the whole weak shares give, down to the
/// weak payee of the deepest share paid.
#[tokio::test]
async fn a_capped_window_at_the_bottom_of_the_zone_pays_from_kept_summaries() {
    let temp_dir = tempdir().unwrap();
    let (node, _writer, blocks) = pruned_node(temp_dir.path().to_str().unwrap()).await;
    let sync_dir = tempdir().unwrap();
    let (syncing, _sync_writer) =
        syncing_node(sync_dir.path().to_str().unwrap(), &node, &blocks).await;
    let reference_dir = tempdir().unwrap();
    let (reference, _reference_writer) = open_node(reference_dir.path().to_str().unwrap());
    confirm(&reference, &blocks[..=DEEPEST_ANCHOR as usize]).await;
    let anchor = &blocks[DEEPEST_ANCHOR as usize];
    let deepest_paid = DEEPEST_ANCHOR - WINDOW as u32;

    let synced = payout(&syncing, anchor, 1);

    assert_eq!(synced, payout(&reference, anchor, 1));
    assert!((BODY_PRUNE_TARGET..WEAK_SHARE_PRUNE_TARGET).contains(&deepest_paid));
    assert!(synced.contains_key(&weak_payee(deepest_paid)));
    assert!(synced.contains_key(&weak_payee(WEAK_SHARE_PRUNE_TARGET - 1)));
    assert!(!synced.contains_key(&share_payee(deepest_paid - 1)));
}

/// The deepest fork a node follows branches at the reorg floor, one payout
/// depth below the tip. A share on it is paid from that anchor, whose capped
/// window and an uncle below its deepest share reach just above the body
/// prune target: every share there keeps its summary on the pruned node,
/// equal to the one its whole weak shares gave, though the whole weak shares
/// are gone.
#[tokio::test]
async fn the_deepest_reorg_reads_summaries_just_above_the_body_prune_target() {
    let temp_dir = tempdir().unwrap();
    let (node, _writer, blocks) = pruned_node(temp_dir.path().to_str().unwrap()).await;
    let reorg_floor = TIP - PAYOUT_DEPTH;
    let deepest_read = reorg_floor + 1 - PAYOUT_DEPTH;
    let read: Vec<&ShareBlock> = blocks[deepest_read as usize..=reorg_floor as usize]
        .iter()
        .collect();
    let hashes: Vec<BlockHash> = read.iter().map(|block| block.block_hash()).collect();

    let summaries = node.get_weak_summaries(&hashes).unwrap();

    assert_eq!(deepest_read, BODY_PRUNE_TARGET + 1);
    let whole: HashMap<BlockHash, WeakSummary> = read
        .iter()
        .map(|block| (block.block_hash(), WeakSummary::of(&block.weak_shares)))
        .collect();
    assert_eq!(summaries, whole);
    let store = node.store_handle().store();
    assert!(store.get_weak_shares(&hashes[0]).unwrap().is_empty());
    assert!(
        store
            .get_pruned_weak_shares(&blocks[BODY_PRUNE_TARGET as usize - 1].block_hash())
            .unwrap()
            .is_none(),
        "below the body prune target the summary goes with the body"
    );
}

/// Without header rules a window is sized by the difficulty its paying
/// header claims. A claim far above what Bitcoin Cash requires cannot move
/// it past its share cap: it pays what a window at the cap pays, and nothing
/// below its deepest share, so it never reads below the summaries.
#[tokio::test]
async fn a_paying_header_claiming_more_difficulty_is_still_capped() {
    let temp_dir = tempdir().unwrap();
    let (node, _writer, blocks) = pruned_node(temp_dir.path().to_str().unwrap()).await;
    let sync_dir = tempdir().unwrap();
    let (syncing, _sync_writer) =
        syncing_node(sync_dir.path().to_str().unwrap(), &node, &blocks).await;
    let anchor = &blocks[DEEPEST_ANCHOR as usize];
    let deepest_paid = DEEPEST_ANCHOR - WINDOW as u32;

    let claimed = payout(&syncing, anchor, u128::MAX);

    assert_eq!(claimed, payout(&syncing, anchor, 1));
    assert!(claimed.contains_key(&share_payee(deepest_paid)));
    assert!(!claimed.contains_key(&share_payee(deepest_paid - 1)));
    assert!(!claimed.contains_key(&weak_payee(deepest_paid - 1)));
}
