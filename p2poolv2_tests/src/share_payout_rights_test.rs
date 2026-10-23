// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A Bitcoin Cash share trade moves the share's PPLNS payout.
//!
//! The share chain is the real store driven through its writer thread, every
//! trade transaction passes the real Bitcoin Cash rules before its share is
//! confirmed, and the payout is the coinbase the notifier and the validator
//! both build: `PplnsWindow::get_distribution_from_start_hash` for the next
//! share's anchor, split by `build_pplns_outputs`. The window is Bitcoin
//! Cash's (`for_chain`), so it pays each window share's weight to whoever
//! holds the share's payout right (docs/atomic-swap/bch-share-trading.md,
//! "Payout rights").

use bitcoin::hashes::Hash;
use bitcoin::{Address, Amount, BlockHash, NetworkKind, OutPoint, PrivateKey, PubkeyHash};
use p2poolv2_lib::accounting::OutputPair;
use p2poolv2_lib::accounting::payout::payout_distribution::build_pplns_outputs;
use p2poolv2_lib::accounting::payout::sharechain_pplns::PplnsWindow;
use p2poolv2_lib::accounting::payout::sharechain_pplns::pplns_window::DIFFICULTY_SCALE;
use p2poolv2_lib::address::share_owner::ShareOwner;
use p2poolv2_lib::config::Chain;
use p2poolv2_lib::pool_difficulty::PoolDifficulty;
use p2poolv2_lib::shares::bch_htlc::{HtlcContract, secret_hash};
use p2poolv2_lib::shares::bch_trade::{
    Coin, TradeKey, build_claim_transaction, build_lock_transaction, build_refund_transaction,
};
use p2poolv2_lib::shares::chain::chain_store_handle::ChainStoreHandle;
use p2poolv2_lib::shares::share_block::ShareBlock;
use p2poolv2_lib::shares::validation::{DefaultShareValidator, ShareValidator};
use p2poolv2_lib::test_utils::{
    PUBKEY_2G, TRADE_BUYER_SECRET_KEY, TRADE_SECRET, TRADE_SELLER_SECRET_KEY,
    TestShareBlockBuilder, setup_test_chain_store_handle,
};
use std::collections::HashMap;
use std::slice;

/// Base timestamp for the share chain; each block advances it by one.
const FIRST_BLOCK_TIME: u32 = 1_700_000_000;
/// Share units in one share coinbase output.
const SHARE_UNITS: u64 = 100_000_000;
/// Network the test shares' payout addresses are on.
const NETWORK: bitcoin::Network = bitcoin::Network::Signet;
/// Paying block difficulty at the proof-of-work limit, so the Bitcoin Cash
/// window pays its share cap: every mature share of this short chain.
const MINIMUM_PAYING_DIFFICULTY: u128 = 1;
/// Value the next coinbase pays out.
const COINBASE_VALUE: u64 = 5_000_000_000;

fn seller() -> TradeKey {
    TradeKey::new(PrivateKey::from_slice(&TRADE_SELLER_SECRET_KEY, NetworkKind::Test).unwrap())
}

fn buyer() -> TradeKey {
    TradeKey::new(PrivateKey::from_slice(&TRADE_BUYER_SECRET_KEY, NetworkKind::Test).unwrap())
}

/// The BCH address a share's right pays when its holder is `key`.
fn holder_address(key: &TradeKey) -> Address {
    Address::p2pkh(PubkeyHash::from_byte_array(key.pubkey_hash()), NETWORK)
}

/// Receive a block the way the node does once its parent is valid: run the
/// Bitcoin Cash transaction and prevout rules, then store, organise, mark
/// valid and confirm it.
async fn receive(chain_store_handle: &ChainStoreHandle, block: &ShareBlock) {
    let validator = DefaultShareValidator::new(PoolDifficulty::new(
        bitcoin::CompactTarget::from_consensus(0x1d00ffff),
        FIRST_BLOCK_TIME,
        0,
    ))
    .for_chain(Chain::BitcoinCash);
    validator
        .validate_share_transactions(block, chain_store_handle)
        .unwrap();
    validator
        .validate_prevouts(block, chain_store_handle)
        .unwrap();
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

/// A share on `parent` at `height`, carrying `transactions`. `salt` keeps
/// shares at the same height on different branches apart.
fn share(
    parent: &ShareBlock,
    height: u32,
    salt: u32,
    transactions: &[bitcoin::Transaction],
) -> ShareBlock {
    let builder = TestShareBlockBuilder::new()
        .prev_share_blockhash(parent.block_hash().to_string())
        .nonce(0x6300_0000 + salt * 0x1000 + height)
        .time(FIRST_BLOCK_TIME + salt * 1_000 + height);
    transactions
        .iter()
        .fold(builder, |builder, transaction| {
            builder.add_transaction(transaction.clone())
        })
        .build()
}

/// Confirm empty shares on `parent` until the tip is at `tip_height`.
async fn extend(
    chain_store_handle: &ChainStoreHandle,
    mut parent: ShareBlock,
    tip_height: u32,
) -> ShareBlock {
    let mut height = chain_store_handle.get_tip_height().unwrap().unwrap();
    while height < tip_height {
        height += 1;
        let block = share(&parent, height, 0, &[]);
        receive(chain_store_handle, &block).await;
        parent = block;
    }
    parent
}

/// A chain whose share at height 1 the seller mined, paying out to
/// `PUBKEY_2G`'s address, with every other share paying the builder's
/// default address, confirmed to height 101 so the seller's share coinbase
/// is mature. Returns the seller's share and the tip.
async fn chain_with_a_mature_seller_share(
    chain_store_handle: &ChainStoreHandle,
) -> (ShareBlock, ShareBlock) {
    let genesis = TestShareBlockBuilder::new()
        .nonce(0x6300_0000)
        .time(FIRST_BLOCK_TIME)
        .build();
    chain_store_handle
        .init_or_setup_genesis(genesis.clone())
        .await
        .unwrap();
    let mined = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .miner_pubkey(PUBKEY_2G)
        .share_owner(ShareOwner::CashP2pkh(seller().pubkey_hash()))
        .nonce(0x6300_0001)
        .time(FIRST_BLOCK_TIME + 1)
        .build();
    receive(chain_store_handle, &mined).await;
    let tip = extend(chain_store_handle, mined.clone(), 101).await;
    (mined, tip)
}

/// The share contract: the buyer claims with the secret, the seller refunds
/// from `refund_height`.
fn share_contract(refund_height: u32) -> HtlcContract {
    HtlcContract::new(
        secret_hash(&TRADE_SECRET),
        buyer().pubkey_hash(),
        seller().pubkey_hash(),
        refund_height,
    )
    .unwrap()
}

/// The seller's lock of the whole share into the contract.
fn lock(mined: &ShareBlock, contract: &HtlcContract) -> bitcoin::Transaction {
    build_lock_transaction(
        &[Coin {
            outpoint: OutPoint::new(mined.transactions[0].compute_txid(), 0),
            value: Amount::from_sat(SHARE_UNITS),
        }],
        &seller(),
        contract,
        Amount::from_sat(SHARE_UNITS),
        &seller().p2pkh_script(),
        0,
    )
    .unwrap()
}

fn contract_coin(lock: &bitcoin::Transaction) -> Coin {
    Coin {
        outpoint: OutPoint::new(lock.compute_txid(), 0),
        value: Amount::from_sat(SHARE_UNITS),
    }
}

/// The weights the next share's coinbase divides, for a share on `anchor`.
fn weights(
    window: &mut PplnsWindow,
    anchor: &ShareBlock,
    chain_store_handle: &ChainStoreHandle,
) -> HashMap<Address, u128> {
    window
        .get_distribution_from_start_hash(
            anchor.block_hash(),
            MINIMUM_PAYING_DIFFICULTY,
            chain_store_handle,
        )
        .unwrap()
}

/// The outputs of the next share's coinbase, for a share on `anchor`.
fn next_coinbase(
    window: &mut PplnsWindow,
    anchor: &ShareBlock,
    chain_store_handle: &ChainStoreHandle,
) -> Vec<OutputPair> {
    build_pplns_outputs(
        COINBASE_VALUE,
        &weights(window, anchor, chain_store_handle),
        &None,
        None,
        &None,
        None,
    )
    .unwrap()
}

fn pays(outputs: &[OutputPair], address: &Address) -> bool {
    outputs.iter().any(|output| output.address == *address)
}

fn bitcoin_cash_window() -> PplnsWindow {
    PplnsWindow::new(NETWORK).for_chain(Chain::BitcoinCash)
}

/// The weight of the seller's share: its difficulty, scaled.
fn seller_share_weight(mined: &ShareBlock) -> u128 {
    mined.get_difficulty(NETWORK) * DIFFICULTY_SCALE
}

/// A claim of the share contract moves the sold share's weight to the
/// buyer's key hash in the next coinbase; until then the seller, who mined
/// it, is paid at the address it mined with, the contract lock included.
#[tokio::test]
async fn test_a_claim_moves_the_share_payout_to_the_buyer_in_the_next_coinbase() {
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let (mined, tip) = chain_with_a_mature_seller_share(&chain_store_handle).await;
    let seller_payout = mined.header.miner_bitcoin_address.clone();
    let mut window = bitcoin_cash_window();
    window.update(&chain_store_handle).unwrap();

    let before = next_coinbase(&mut window, &tip, &chain_store_handle);
    assert!(pays(&before, &seller_payout));
    assert!(!pays(&before, &holder_address(&buyer())));

    let contract = share_contract(600);
    let share_lock = lock(&mined, &contract);
    let lock_block = share(&tip, 102, 0, slice::from_ref(&share_lock));
    receive(&chain_store_handle, &lock_block).await;
    window.update(&chain_store_handle).unwrap();

    let locked = weights(&mut window, &lock_block, &chain_store_handle);
    assert_eq!(
        locked.get(&seller_payout),
        Some(&seller_share_weight(&mined))
    );
    assert_eq!(locked.get(&holder_address(&buyer())), None);

    let claim = build_claim_transaction(
        &contract_coin(&share_lock),
        &contract,
        &buyer(),
        &TRADE_SECRET,
        &buyer().p2pkh_script(),
        0,
    )
    .unwrap();
    let claim_block = share(&lock_block, 103, 0, slice::from_ref(&claim));
    receive(&chain_store_handle, &claim_block).await;
    window.update(&chain_store_handle).unwrap();

    let claimed = weights(&mut window, &claim_block, &chain_store_handle);
    assert_eq!(
        claimed.get(&holder_address(&buyer())),
        Some(&seller_share_weight(&mined))
    );
    assert_eq!(claimed.get(&seller_payout), None);
    let after = next_coinbase(&mut window, &claim_block, &chain_store_handle);
    assert!(pays(&after, &holder_address(&buyer())));
    assert!(!pays(&after, &seller_payout));
    assert_eq!(
        after.iter().map(|output| output.amount).sum::<Amount>(),
        Amount::from_sat(COINBASE_VALUE)
    );
}

/// Bitcoin keeps upstream's rule: the same claim leaves a Bitcoin window
/// paying the share's `miner_bitcoin_address`.
#[tokio::test]
async fn test_a_bitcoin_window_keeps_paying_the_miner_after_a_claim() {
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let (mined, tip) = chain_with_a_mature_seller_share(&chain_store_handle).await;
    let contract = share_contract(600);
    let share_lock = lock(&mined, &contract);
    let lock_block = share(&tip, 102, 0, slice::from_ref(&share_lock));
    receive(&chain_store_handle, &lock_block).await;
    let claim = build_claim_transaction(
        &contract_coin(&share_lock),
        &contract,
        &buyer(),
        &TRADE_SECRET,
        &buyer().p2pkh_script(),
        0,
    )
    .unwrap();
    let claim_block = share(&lock_block, 103, 0, slice::from_ref(&claim));
    receive(&chain_store_handle, &claim_block).await;
    let mut bitcoin_window = PplnsWindow::new(NETWORK);
    bitcoin_window.update(&chain_store_handle).unwrap();

    let after_the_claim = weights(&mut bitcoin_window, &claim_block, &chain_store_handle);

    assert_eq!(
        after_the_claim.get(&mined.header.miner_bitcoin_address),
        Some(&seller_share_weight(&mined))
    );
    assert_eq!(after_the_claim.get(&holder_address(&buyer())), None);
}

/// A refund after the refund height passes the right back to the seller's
/// own key hash, so the seller is paid at the address it mined with, as if
/// the share had never been offered.
#[tokio::test]
async fn test_a_refund_keeps_the_share_payout_with_the_seller() {
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let (mined, tip) = chain_with_a_mature_seller_share(&chain_store_handle).await;
    let seller_payout = mined.header.miner_bitcoin_address.clone();
    let mut window = bitcoin_cash_window();

    let contract = share_contract(104);
    let share_lock = lock(&mined, &contract);
    let lock_block = share(&tip, 102, 0, slice::from_ref(&share_lock));
    receive(&chain_store_handle, &lock_block).await;
    let tip = extend(&chain_store_handle, lock_block, 104).await;
    let refund = build_refund_transaction(
        &contract_coin(&share_lock),
        &contract,
        &seller(),
        &seller().p2pkh_script(),
        0,
    )
    .unwrap();
    let refund_block = share(&tip, 105, 0, slice::from_ref(&refund));
    receive(&chain_store_handle, &refund_block).await;
    window.update(&chain_store_handle).unwrap();

    let refunded = weights(&mut window, &refund_block, &chain_store_handle);
    assert_eq!(
        refunded.get(&seller_payout),
        Some(&seller_share_weight(&mined))
    );
    assert_eq!(refunded.get(&holder_address(&seller())), None);
    assert_eq!(refunded.get(&holder_address(&buyer())), None);
    let after = next_coinbase(&mut window, &refund_block, &chain_store_handle);
    assert!(pays(&after, &seller_payout));
    assert!(!pays(&after, &holder_address(&buyer())));
}

/// A reorg that takes the claim off the confirmed chain gives the payout back
/// to the seller: the window undoes the claim's share when it rolls back.
#[tokio::test]
async fn test_a_reorg_that_removes_the_claim_returns_the_payout_to_the_seller() {
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let (mined, tip) = chain_with_a_mature_seller_share(&chain_store_handle).await;
    let seller_payout = mined.header.miner_bitcoin_address.clone();
    let mut window = bitcoin_cash_window();
    let contract = share_contract(600);
    let share_lock = lock(&mined, &contract);
    let lock_block = share(&tip, 102, 0, slice::from_ref(&share_lock));
    receive(&chain_store_handle, &lock_block).await;
    let claim = build_claim_transaction(
        &contract_coin(&share_lock),
        &contract,
        &buyer(),
        &TRADE_SECRET,
        &buyer().p2pkh_script(),
        0,
    )
    .unwrap();
    let claim_block = share(&lock_block, 103, 0, slice::from_ref(&claim));
    receive(&chain_store_handle, &claim_block).await;
    window.update(&chain_store_handle).unwrap();
    assert!(
        weights(&mut window, &claim_block, &chain_store_handle)
            .contains_key(&holder_address(&buyer()))
    );

    // Two shares on the lock block, without the claim, outweigh the claim.
    let fork_first = share(&lock_block, 103, 1, &[]);
    let fork_second = share(&fork_first, 104, 1, &[]);
    receive(&chain_store_handle, &fork_first).await;
    receive(&chain_store_handle, &fork_second).await;
    assert!(!chain_store_handle.is_block_confirmed(&claim_block.block_hash()));
    window.update(&chain_store_handle).unwrap();

    let on_the_fork = weights(&mut window, &fork_second, &chain_store_handle);
    assert_eq!(
        on_the_fork.get(&seller_payout),
        Some(&seller_share_weight(&mined))
    );
    assert_eq!(on_the_fork.get(&holder_address(&buyer())), None);
}

/// Every node pays the same holder for the same anchor whatever its own
/// cached tip: an anchor above the cache sees the claim in its branch, and
/// an anchor below a cached claim does not.
#[tokio::test]
async fn test_the_holder_depends_on_the_anchor_not_on_the_cached_tip() {
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let (mined, tip) = chain_with_a_mature_seller_share(&chain_store_handle).await;
    let seller_payout = mined.header.miner_bitcoin_address.clone();
    let contract = share_contract(600);
    let share_lock = lock(&mined, &contract);
    let lock_block = share(&tip, 102, 0, slice::from_ref(&share_lock));
    receive(&chain_store_handle, &lock_block).await;

    // This window's cache stops at the lock block.
    let mut lagging_window = bitcoin_cash_window();
    lagging_window.update(&chain_store_handle).unwrap();
    let claim = build_claim_transaction(
        &contract_coin(&share_lock),
        &contract,
        &buyer(),
        &TRADE_SECRET,
        &buyer().p2pkh_script(),
        0,
    )
    .unwrap();
    let claim_block = share(&lock_block, 103, 0, slice::from_ref(&claim));
    receive(&chain_store_handle, &claim_block).await;
    let mut current_window = bitcoin_cash_window();
    current_window.update(&chain_store_handle).unwrap();

    let above_the_cache = weights(&mut lagging_window, &claim_block, &chain_store_handle);
    let at_the_cache = weights(&mut current_window, &claim_block, &chain_store_handle);
    assert_eq!(above_the_cache, at_the_cache);
    assert!(above_the_cache.contains_key(&holder_address(&buyer())));

    let below_the_claim = weights(&mut current_window, &lock_block, &chain_store_handle);
    assert_eq!(
        below_the_claim.get(&seller_payout),
        Some(&seller_share_weight(&mined))
    );
    assert_eq!(below_the_claim.get(&holder_address(&buyer())), None);
    let anchor_hash: BlockHash = lock_block.block_hash();
    assert_eq!(
        below_the_claim,
        weights(&mut lagging_window, &lock_block, &chain_store_handle),
        "anchor {anchor_hash}"
    );
}
