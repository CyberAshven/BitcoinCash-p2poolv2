// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A whole Bitcoin Cash share trade on an in-process share chain.
//!
//! The share chain is the real store driven through its writer thread, and
//! every block carrying a trade transaction first passes the real Bitcoin
//! Cash rules: `validate_share_transactions` (scripts, values, outputs) and
//! `validate_prevouts` (existence, maturity, finality). Bitcoin Cash itself
//! is not running here: its leg is checked by the same interpreter, which
//! implements BCHN's rules for these scripts, and the counterparty reads the
//! secret from the BCH claim exactly as it would from a BCHN node.
//!
//! The protocol is the one in `docs/atomic-swap/bch-share-trading.md`: the
//! seller locks share units under the secret's hash, the buyer locks BCH
//! under the same hash, the seller claims the BCH and so reveals the secret,
//! and the buyer claims the shares with it.

use bitcoin::hashes::Hash;
use bitcoin::{Amount, NetworkKind, OutPoint, PrivateKey, TxOut, Txid};
use p2poolv2_lib::address::share_owner::ShareOwner;
use p2poolv2_lib::config::Chain;
use p2poolv2_lib::pool_difficulty::PoolDifficulty;
use p2poolv2_lib::shares::bch_htlc::{HtlcContract, secret_hash};
use p2poolv2_lib::shares::bch_script::{verify_outputs, verify_transaction};
use p2poolv2_lib::shares::bch_trade::{
    Coin, TradeKey, build_claim_transaction, build_lock_transaction, build_refund_transaction,
};
use p2poolv2_lib::shares::chain::chain_store_handle::ChainStoreHandle;
use p2poolv2_lib::shares::share_block::ShareBlock;
use p2poolv2_lib::shares::validation::{DefaultShareValidator, ShareValidator, ValidationError};
use p2poolv2_lib::test_utils::{
    TRADE_BUYER_SECRET_KEY, TRADE_SECRET, TRADE_SELLER_SECRET_KEY, TestShareBlockBuilder,
    setup_test_chain_store_handle,
};

/// Base timestamp for the share chain; each block advances it by one.
const FIRST_BLOCK_TIME: u32 = 1_700_000_000;
/// Share units in one share coinbase output.
const SHARE_UNITS: u64 = 100_000_000;

/// The Bitcoin Cash share chain's rules, with an arbitrary difficulty anchor:
/// none of the checks used here reads pool difficulty.
fn bitcoin_cash_validator() -> DefaultShareValidator {
    DefaultShareValidator::new(PoolDifficulty::new(
        bitcoin::CompactTarget::from_consensus(0x1d00ffff),
        FIRST_BLOCK_TIME,
        0,
    ))
    .for_chain(Chain::BitcoinCash)
}

/// Receive a block the way the node does once its parent is valid: run the
/// Bitcoin Cash transaction and prevout rules, and only if they pass, store
/// it, organise it onto the candidate chain, mark it valid and confirm it.
/// A rule failure is returned and nothing is stored.
async fn receive_if_valid(
    chain_store_handle: &ChainStoreHandle,
    block: &ShareBlock,
) -> Result<(), ValidationError> {
    let validator = bitcoin_cash_validator();
    validator.validate_share_transactions(block, chain_store_handle)?;
    validator.validate_prevouts(block, chain_store_handle)?;
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
    Ok(())
}

/// Extend the confirmed chain with empty shares until its tip is at
/// `tip_height`, returning the new tip.
async fn extend_to_height(
    chain_store_handle: &ChainStoreHandle,
    mut parent: ShareBlock,
    tip_height: u32,
) -> ShareBlock {
    let mut height = chain_store_handle.get_tip_height().unwrap().unwrap();
    while height < tip_height {
        height += 1;
        let block = TestShareBlockBuilder::new()
            .prev_share_blockhash(parent.block_hash().to_string())
            .nonce(0x7000_0000 + height)
            .time(FIRST_BLOCK_TIME + height)
            .build();
        receive_if_valid(chain_store_handle, &block).await.unwrap();
        parent = block;
    }
    parent
}

/// The happy path: lock shares, lock BCH, the seller claims the BCH and the
/// buyer claims the shares with the secret the BCH claim revealed.
#[tokio::test]
async fn test_a_share_trade_completes_on_both_chains() {
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let seller =
        TradeKey::new(PrivateKey::from_slice(&TRADE_SELLER_SECRET_KEY, NetworkKind::Test).unwrap());
    let buyer =
        TradeKey::new(PrivateKey::from_slice(&TRADE_BUYER_SECRET_KEY, NetworkKind::Test).unwrap());

    // The seller mines the share at height 1 and waits out the 100-share
    // coinbase maturity.
    let genesis = TestShareBlockBuilder::new()
        .nonce(0x6000_0000)
        .time(FIRST_BLOCK_TIME)
        .build();
    chain_store_handle
        .init_or_setup_genesis(genesis.clone())
        .await
        .unwrap();
    let mined = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .share_owner(ShareOwner::CashP2pkh(seller.pubkey_hash()))
        .nonce(0x6000_0001)
        .time(FIRST_BLOCK_TIME + 1)
        .build();
    receive_if_valid(&chain_store_handle, &mined).await.unwrap();
    let share_coin = Coin {
        outpoint: OutPoint::new(mined.transactions[0].compute_txid(), 0),
        value: Amount::from_sat(SHARE_UNITS),
    };
    let tip = extend_to_height(&chain_store_handle, mined, 101).await;

    // Step 1: the seller locks 0.4 of the share for the buyer, refundable
    // from share height 600.
    let share_contract = HtlcContract::new(
        secret_hash(&TRADE_SECRET),
        buyer.pubkey_hash(),
        seller.pubkey_hash(),
        600,
    )
    .unwrap();
    let share_lock = build_lock_transaction(
        &[share_coin],
        &seller,
        &share_contract,
        Amount::from_sat(40_000_000),
        &seller.p2pkh_script(),
        0,
    )
    .unwrap();
    let lock_block = TestShareBlockBuilder::new()
        .prev_share_blockhash(tip.block_hash().to_string())
        .nonce(0x6000_0102)
        .time(FIRST_BLOCK_TIME + 102)
        .add_transaction(share_lock.clone())
        .build();
    receive_if_valid(&chain_store_handle, &lock_block)
        .await
        .unwrap();
    assert!(chain_store_handle.is_block_confirmed(&lock_block.block_hash()));
    let share_lock_output = chain_store_handle
        .get_output(&share_lock.compute_txid(), 0)
        .unwrap();
    assert_eq!(
        share_lock_output.script_pubkey,
        share_contract.locking_script()
    );
    assert_eq!(share_lock_output.value, Amount::from_sat(40_000_000));

    // Step 2: the buyer checks the share-chain lock against the contract it
    // rebuilt from the agreed terms, then locks 0.0005 BCH for the seller
    // under the same hash, refundable from BCH height 950,000.
    let bch_contract = HtlcContract::new(
        secret_hash(&TRADE_SECRET),
        seller.pubkey_hash(),
        buyer.pubkey_hash(),
        950_000,
    )
    .unwrap();
    let buyer_bch_coin = Coin {
        outpoint: OutPoint::new(Txid::from_byte_array([0xbc; 32]), 0),
        value: Amount::from_sat(100_000),
    };
    let bch_lock = build_lock_transaction(
        &[buyer_bch_coin],
        &buyer,
        &bch_contract,
        Amount::from_sat(50_000),
        &buyer.p2pkh_script(),
        1,
    )
    .unwrap();
    assert_eq!(
        verify_transaction(
            &bch_lock,
            &[(
                0,
                TxOut {
                    value: buyer_bch_coin.value,
                    script_pubkey: buyer.p2pkh_script(),
                },
            )],
        ),
        Ok(())
    );
    assert_eq!(verify_outputs(&bch_lock), Ok(()));

    // Step 3: the seller claims the BCH, which publishes the secret.
    let bch_contract_coin = Coin {
        outpoint: OutPoint::new(bch_lock.compute_txid(), 0),
        value: Amount::from_sat(50_000),
    };
    let bch_claim = build_claim_transaction(
        &bch_contract_coin,
        &bch_contract,
        &seller,
        &TRADE_SECRET,
        &seller.p2pkh_script(),
        1,
    )
    .unwrap();
    assert_eq!(
        verify_transaction(
            &bch_claim,
            &[(
                0,
                TxOut {
                    value: bch_contract_coin.value,
                    script_pubkey: bch_contract.locking_script(),
                },
            )],
        ),
        Ok(())
    );
    assert_eq!(verify_outputs(&bch_claim), Ok(()));

    // Step 4: the buyer reads the secret from the BCH claim and claims the
    // shares with it.
    let revealed = bch_contract.find_secret(&bch_claim).unwrap();
    assert_eq!(revealed, TRADE_SECRET);
    let share_contract_coin = Coin {
        outpoint: OutPoint::new(share_lock.compute_txid(), 0),
        value: Amount::from_sat(40_000_000),
    };
    let share_claim = build_claim_transaction(
        &share_contract_coin,
        &share_contract,
        &buyer,
        &revealed,
        &buyer.p2pkh_script(),
        0,
    )
    .unwrap();
    let claim_block = TestShareBlockBuilder::new()
        .prev_share_blockhash(lock_block.block_hash().to_string())
        .nonce(0x6000_0103)
        .time(FIRST_BLOCK_TIME + 103)
        .add_transaction(share_claim.clone())
        .build();
    receive_if_valid(&chain_store_handle, &claim_block)
        .await
        .unwrap();

    assert_eq!(chain_store_handle.get_tip_height().unwrap(), Some(103));
    assert!(chain_store_handle.is_block_confirmed(&claim_block.block_hash()));
    let bought = chain_store_handle
        .get_output(&share_claim.compute_txid(), 0)
        .unwrap();
    assert_eq!(bought.script_pubkey, buyer.p2pkh_script());
    assert_eq!(bought.value, Amount::from_sat(40_000_000));
    let change = chain_store_handle
        .get_output(&share_lock.compute_txid(), 1)
        .unwrap();
    assert_eq!(change.script_pubkey, seller.p2pkh_script());
    assert_eq!(change.value, Amount::from_sat(60_000_000));
}

/// The buyer never pays: the seller's refund is rejected until the share
/// above the refund height, a claim without the secret is rejected, and the
/// refund then confirms.
#[tokio::test]
async fn test_unpaid_shares_return_to_the_seller_after_the_refund_height() {
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let seller =
        TradeKey::new(PrivateKey::from_slice(&TRADE_SELLER_SECRET_KEY, NetworkKind::Test).unwrap());
    let buyer =
        TradeKey::new(PrivateKey::from_slice(&TRADE_BUYER_SECRET_KEY, NetworkKind::Test).unwrap());

    let genesis = TestShareBlockBuilder::new()
        .nonce(0x6100_0000)
        .time(FIRST_BLOCK_TIME)
        .build();
    chain_store_handle
        .init_or_setup_genesis(genesis.clone())
        .await
        .unwrap();
    let mined = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .share_owner(ShareOwner::CashP2pkh(seller.pubkey_hash()))
        .nonce(0x6100_0001)
        .time(FIRST_BLOCK_TIME + 1)
        .build();
    receive_if_valid(&chain_store_handle, &mined).await.unwrap();
    let share_coin = Coin {
        outpoint: OutPoint::new(mined.transactions[0].compute_txid(), 0),
        value: Amount::from_sat(SHARE_UNITS),
    };
    let tip = extend_to_height(&chain_store_handle, mined, 101).await;

    // The whole share is offered, refundable from share height 110.
    let share_contract = HtlcContract::new(
        secret_hash(&TRADE_SECRET),
        buyer.pubkey_hash(),
        seller.pubkey_hash(),
        110,
    )
    .unwrap();
    let share_lock = build_lock_transaction(
        &[share_coin],
        &seller,
        &share_contract,
        Amount::from_sat(SHARE_UNITS),
        &seller.p2pkh_script(),
        0,
    )
    .unwrap();
    assert_eq!(
        share_lock.output.len(),
        1,
        "no change when the whole share is sold"
    );
    let lock_block = TestShareBlockBuilder::new()
        .prev_share_blockhash(tip.block_hash().to_string())
        .nonce(0x6100_0102)
        .time(FIRST_BLOCK_TIME + 102)
        .add_transaction(share_lock.clone())
        .build();
    receive_if_valid(&chain_store_handle, &lock_block)
        .await
        .unwrap();
    let contract_coin = Coin {
        outpoint: OutPoint::new(share_lock.compute_txid(), 0),
        value: Amount::from_sat(SHARE_UNITS),
    };
    let refund = build_refund_transaction(
        &contract_coin,
        &share_contract,
        &seller,
        &seller.p2pkh_script(),
        0,
    )
    .unwrap();

    // At height 103 the refund is not final yet.
    let early_refund_block = TestShareBlockBuilder::new()
        .prev_share_blockhash(lock_block.block_hash().to_string())
        .nonce(0x6100_0103)
        .time(FIRST_BLOCK_TIME + 103)
        .add_transaction(refund.clone())
        .build();
    let early = receive_if_valid(&chain_store_handle, &early_refund_block)
        .await
        .unwrap_err();
    assert!(
        early
            .to_string()
            .contains("lock time 110 is not final at share height 103"),
        "{early}"
    );

    // Without the secret the buyer cannot claim: a guessed secret fails the
    // hash lock even with the buyer's valid signature.
    let mut guessed_claim = build_claim_transaction(
        &contract_coin,
        &share_contract,
        &buyer,
        &TRADE_SECRET,
        &buyer.p2pkh_script(),
        0,
    )
    .unwrap();
    let signature = buyer
        .sign_input(
            &guessed_claim,
            0,
            &share_contract.redeem_script(),
            contract_coin.value,
        )
        .unwrap();
    guessed_claim.input[0].script_sig =
        share_contract.claim_unlocking_script(&signature, buyer.public_key(), &[0x43; 32]);
    let guessed_claim_block = TestShareBlockBuilder::new()
        .prev_share_blockhash(lock_block.block_hash().to_string())
        .nonce(0x6100_0203)
        .time(FIRST_BLOCK_TIME + 103)
        .add_transaction(guessed_claim)
        .build();
    let guessed = receive_if_valid(&chain_store_handle, &guessed_claim_block)
        .await
        .unwrap_err();
    assert!(
        guessed
            .to_string()
            .contains("secret does not match the contract's secret hash"),
        "{guessed}"
    );

    // At height 110, the refund height itself, the refund is still not final.
    let tip = extend_to_height(&chain_store_handle, lock_block, 109).await;
    let at_height_block = TestShareBlockBuilder::new()
        .prev_share_blockhash(tip.block_hash().to_string())
        .nonce(0x6100_0110)
        .time(FIRST_BLOCK_TIME + 110)
        .add_transaction(refund.clone())
        .build();
    assert!(
        receive_if_valid(&chain_store_handle, &at_height_block)
            .await
            .is_err()
    );

    // In the share above the refund height it is mined and confirmed.
    let tip = extend_to_height(&chain_store_handle, tip, 110).await;
    let refund_block = TestShareBlockBuilder::new()
        .prev_share_blockhash(tip.block_hash().to_string())
        .nonce(0x6100_0111)
        .time(FIRST_BLOCK_TIME + 111)
        .add_transaction(refund.clone())
        .build();
    receive_if_valid(&chain_store_handle, &refund_block)
        .await
        .unwrap();
    assert_eq!(chain_store_handle.get_tip_height().unwrap(), Some(111));
    assert!(chain_store_handle.is_block_confirmed(&refund_block.block_hash()));
    let returned = chain_store_handle
        .get_output(&refund.compute_txid(), 0)
        .unwrap();
    assert_eq!(returned.script_pubkey, seller.p2pkh_script());
    assert_eq!(returned.value, Amount::from_sat(SHARE_UNITS));
}

/// The seller cannot lock a share before it matures, so a buyer never pays
/// for share units a share-chain reorg below the maturity depth could undo.
#[tokio::test]
async fn test_a_lock_of_an_immature_share_is_rejected() {
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let seller =
        TradeKey::new(PrivateKey::from_slice(&TRADE_SELLER_SECRET_KEY, NetworkKind::Test).unwrap());
    let buyer =
        TradeKey::new(PrivateKey::from_slice(&TRADE_BUYER_SECRET_KEY, NetworkKind::Test).unwrap());

    let genesis = TestShareBlockBuilder::new()
        .nonce(0x6200_0000)
        .time(FIRST_BLOCK_TIME)
        .build();
    chain_store_handle
        .init_or_setup_genesis(genesis.clone())
        .await
        .unwrap();
    let mined = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .share_owner(ShareOwner::CashP2pkh(seller.pubkey_hash()))
        .nonce(0x6200_0001)
        .time(FIRST_BLOCK_TIME + 1)
        .build();
    receive_if_valid(&chain_store_handle, &mined).await.unwrap();
    let share_coin = Coin {
        outpoint: OutPoint::new(mined.transactions[0].compute_txid(), 0),
        value: Amount::from_sat(SHARE_UNITS),
    };
    // The share at height 1 is 99 deep at height 100, one short.
    let tip = extend_to_height(&chain_store_handle, mined, 99).await;

    let share_contract = HtlcContract::new(
        secret_hash(&TRADE_SECRET),
        buyer.pubkey_hash(),
        seller.pubkey_hash(),
        600,
    )
    .unwrap();
    let share_lock = build_lock_transaction(
        &[share_coin],
        &seller,
        &share_contract,
        Amount::from_sat(40_000_000),
        &seller.p2pkh_script(),
        0,
    )
    .unwrap();
    let lock_block = TestShareBlockBuilder::new()
        .prev_share_blockhash(tip.block_hash().to_string())
        .nonce(0x6200_0100)
        .time(FIRST_BLOCK_TIME + 100)
        .add_transaction(share_lock)
        .build();
    let error = receive_if_valid(&chain_store_handle, &lock_block)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("is not mature at height 100"),
        "{error}"
    );
}
