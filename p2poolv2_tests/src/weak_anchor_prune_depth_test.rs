// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A syncing node pays a weak share carried at the bottom of its
//! full-validation zone from summaries it fetched.
//!
//! A weak share's parent may be `WEAK_DEPTH` shares below the share carrying
//! it, and the weak share is paid from a window anchored at that parent. A
//! syncing node fetches bodies, and with them the weak share summaries the
//! window reads, only down to its prune height; a share it pays without a
//! summary it credits without its weak shares, and the weak share then fails
//! its payout check. The prune depth adds `WEAK_DEPTH`
//! (`ChainDepths::weak_anchor_depth`) so that every share such a window pays
//! is fetched.
//!
//! The chain is scaled down with the window: a 2-share maturity skip, an
//! 8-share cap and an uncle's 3, a 13-share payout depth sized as
//! `BCH_PAYOUT_DEPTH` is, with the real `WEAK_DEPTH`. Every bound is a count
//! of shares, so the scaled chain has the shape of the real one.

use bitcoin::hashes::Hash;
use bitcoin::{Address, CompactTarget, PubkeyHash, ScriptBuf};
use p2poolv2_lib::accounting::payout::sharechain_pplns::PplnsWindow;
use p2poolv2_lib::accounting::payout::sharechain_pplns::pplns_window::ChainDepths;
use p2poolv2_lib::address::share_owner::ShareOwner;
use p2poolv2_lib::config::{Chain, PayoutAddress};
use p2poolv2_lib::shares::chain::chain_store_handle::ChainStoreHandle;
use p2poolv2_lib::shares::share_block::ShareBlock;
use p2poolv2_lib::shares::weak::{WEAK_DEPTH, WeakShare};
use p2poolv2_lib::store::dag_store::MAX_UNCLES_DEPTH;
use p2poolv2_lib::store::prune_blocks::prune_height_for_tip;
use p2poolv2_lib::test_utils::{
    TestShareBlockBuilder, confirm_test_block, setup_test_chain_store_handle,
};
use std::collections::HashMap;

/// Network the test shares' payout addresses use.
const NETWORK: bitcoin::Network = bitcoin::Network::Regtest;
/// Base timestamp for the share chain; each share advances it by one.
const FIRST_SHARE_TIME: u32 = 1_700_000_000;
/// A weak ceiling far above every target here.
const CEILING: u32 = 0x207f_ffff;
/// The scaled maturity skip and window cap.
const MATURITY: usize = 2;
const WINDOW: usize = 8;

/// The scaled depths: a payout depth of 2 + 8 + `MAX_UNCLES_DEPTH`, and the
/// real `WEAK_DEPTH`, as `ChainDepths::BITCOIN_CASH` holds them.
const DEPTHS: ChainDepths = ChainDepths {
    coinbase_maturity: MATURITY,
    payout_depth: MATURITY + WINDOW + MAX_UNCLES_DEPTH as usize,
    weak_anchor_depth: WEAK_DEPTH as usize,
};

/// The same depths before the prune depth covered the weak anchor.
const DEPTHS_BEFORE: ChainDepths = ChainDepths {
    weak_anchor_depth: 0,
    ..DEPTHS
};

/// The syncing node's candidate tip, 20 shares above its prune depth.
const TIP: u32 = DEPTHS.prune_depth() as u32 + 20;

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

/// Genesis and `length` shares on it, genesis first, each carrying one weak
/// share mined on its parent and paying its own payee.
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
        blocks.push(
            TestShareBlockBuilder::new()
                .share_owner(ShareOwner::CashP2pkh([0x22; 20]))
                .payout_address(share_payee(height))
                .prev_share_blockhash(parent.block_hash().to_string())
                .nonce(0x6600_0000 + height)
                .time(FIRST_SHARE_TIME + height)
                .weak_shares(vec![weak_share])
                .build(),
        );
    }
    blocks
}

/// A node holding `blocks`, with the bodies below `fetched_from` stored
/// without their weak shares, as a syncing node holds the shares below its
/// prune height: no summary to read.
async fn node_fetching_from(
    blocks: &[ShareBlock],
    fetched_from: u32,
) -> (ChainStoreHandle, tempfile::TempDir) {
    let (chain_store_handle, temp_dir) = setup_test_chain_store_handle(true).await;
    chain_store_handle
        .init_or_setup_genesis(blocks[0].clone())
        .await
        .unwrap();
    for (height, block) in blocks.iter().enumerate().skip(1) {
        let stored = if (height as u32) < fetched_from {
            ShareBlock {
                weak_shares: Vec::new(),
                ..block.clone()
            }
        } else {
            block.clone()
        };
        confirm_test_block(&chain_store_handle, &stored).await;
    }
    (chain_store_handle, temp_dir)
}

/// The weights a share on `anchor` divides, at the scaled cap, through a
/// fresh Bitcoin Cash window over the node.
fn payout(
    chain_store_handle: &ChainStoreHandle,
    anchor: &ShareBlock,
) -> HashMap<PayoutAddress, u128> {
    let mut window = PplnsWindow::new(NETWORK)
        .for_chain(Chain::BitcoinCash)
        .with_weak_ceiling(CompactTarget::from_consensus(CEILING))
        .with_maturity_skip(MATURITY)
        .with_max_window_shares(WINDOW);
    window.update(chain_store_handle).unwrap();
    window
        .get_distribution_from_start_hash(anchor.block_hash(), 1_u128.into(), chain_store_handle)
        .unwrap()
}

/// The deepest share a syncing node fully checks is one payout depth below
/// its tip; a weak share it carries may have its parent `WEAK_DEPTH` lower.
/// Paid from that parent, the weak share's window reads shares a node
/// fetching from the prune height holds with their summaries, and pays what
/// the whole chain pays. Fetching from two payout depths, as before, it
/// reads shares held without summaries and pays otherwise.
#[tokio::test]
async fn a_weak_share_carried_at_the_bottom_of_the_zone_is_paid_from_fetched_summaries() {
    let deepest_checked = TIP - DEPTHS.payout_depth as u32 + 1;
    let weak_parent = deepest_checked - WEAK_DEPTH;
    let blocks = chain(weak_parent);
    let anchor = &blocks[weak_parent as usize];
    let deepest_paid = weak_parent - WINDOW as u32;
    let (whole, _whole_dir) = node_fetching_from(&blocks, 0).await;
    let (fixed, _fixed_dir) = node_fetching_from(&blocks, prune_height_for_tip(DEPTHS, TIP)).await;
    let (before, _before_dir) =
        node_fetching_from(&blocks, prune_height_for_tip(DEPTHS_BEFORE, TIP)).await;

    let paid = payout(&fixed, anchor);

    assert!(deepest_paid >= prune_height_for_tip(DEPTHS, TIP));
    assert_eq!(paid, payout(&whole, anchor));
    assert!(paid.contains_key(&weak_payee(deepest_paid)));
    assert!(deepest_paid < prune_height_for_tip(DEPTHS_BEFORE, TIP));
    assert_ne!(payout(&before, anchor), paid);
}
