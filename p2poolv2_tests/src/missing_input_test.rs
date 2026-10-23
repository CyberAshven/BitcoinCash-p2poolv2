// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A block spending an output this node does not hold, through the real
//! store and validator (`docs/architecture/pruning.md`, "Missing Input at
//! Write Time").

use bitcoin::hashes::Hash;
use bitcoin::{
    Amount, CompactTarget, OutPoint, ScriptBuf, Transaction, TxIn, TxOut, Txid, absolute::LockTime,
    transaction::Version,
};
use p2poolv2_lib::pool_difficulty::PoolDifficulty;
use p2poolv2_lib::shares::validation::{DefaultShareValidator, FailureKind, ShareValidator};
use p2poolv2_lib::test_utils::{
    TestShareBlockBuilder, confirm_test_block, setup_test_chain_store_handle,
};

/// Base timestamp for the share chain; each block advances it by one.
const FIRST_BLOCK_TIME: u32 = 1_700_000_000;

/// The share chain's rules, with an arbitrary difficulty anchor: none of the
/// checks used here reads pool difficulty.
fn validator() -> DefaultShareValidator {
    DefaultShareValidator::new(PoolDifficulty::new(
        CompactTarget::from_consensus(0x1d00ffff),
        FIRST_BLOCK_TIME,
        0,
    ))
}

/// A transaction spending `outpoint` into one output of `value` units.
fn spend(outpoint: OutPoint, value: u64) -> Transaction {
    Transaction {
        version: Version::ONE,
        lock_time: LockTime::ZERO,
        input: vec![TxIn {
            previous_output: outpoint,
            ..Default::default()
        }],
        output: vec![TxOut {
            value: Amount::from_sat(value),
            script_pubkey: ScriptBuf::new(),
        }],
    }
}

/// Case (b): a block validated on proof of work alone spends an output below
/// this node's prune height, which it never fetched. The block is stored and
/// confirmed instead of stalling confirmation. Its outputs carry an unknown
/// coinbase root, and a fully validated block spending one is rejected: the
/// true root is below the prune height, so every node holding it rejects the
/// spend as too old.
#[tokio::test]
async fn test_a_block_spending_an_unfetched_output_is_confirmed_and_its_outputs_are_unspendable() {
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let genesis = TestShareBlockBuilder::new()
        .nonce(0x6100_0000)
        .time(FIRST_BLOCK_TIME)
        .build();
    chain_store_handle
        .init_or_setup_genesis(genesis.clone())
        .await
        .unwrap();

    let spend_of_unfetched_output =
        spend(OutPoint::new(Txid::from_byte_array([0x42; 32]), 0), 2_000);
    let unfetched_spend_txid = spend_of_unfetched_output.compute_txid();
    let block = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .add_transaction(spend_of_unfetched_output)
        .nonce(0x6100_0001)
        .time(FIRST_BLOCK_TIME + 1)
        .build();
    confirm_test_block(&chain_store_handle, &block).await;

    assert!(chain_store_handle.is_block_confirmed(&block.block_hash()));

    let spender = TestShareBlockBuilder::new()
        .prev_share_blockhash(block.block_hash().to_string())
        .add_transaction(spend(OutPoint::new(unfetched_spend_txid, 0), 1_000))
        .nonce(0x6100_0002)
        .time(FIRST_BLOCK_TIME + 2)
        .build();
    let error = validator()
        .validate_prevouts(&spender, &chain_store_handle)
        .unwrap_err();

    assert_eq!(error.kind(), FailureKind::Consensus);
    assert!(
        error.to_string().contains("unknown coinbase root"),
        "unexpected error: {error}"
    );
}

/// Case (a): in the full-validation zone a missing input means the output
/// never existed on the block's chain. The block is stored, so validation
/// can mark it Invalid, rather than left unstored and fetched for ever.
#[tokio::test]
async fn test_a_fully_validated_block_spending_a_missing_output_is_stored_and_invalid() {
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let genesis = TestShareBlockBuilder::new()
        .nonce(0x6200_0000)
        .time(FIRST_BLOCK_TIME)
        .build();
    chain_store_handle
        .init_or_setup_genesis(genesis.clone())
        .await
        .unwrap();

    let block = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .add_transaction(spend(
            OutPoint::new(Txid::from_byte_array([0x43; 32]), 0),
            2_000,
        ))
        .nonce(0x6200_0001)
        .time(FIRST_BLOCK_TIME + 1)
        .build();
    chain_store_handle
        .add_share_block(block.clone())
        .await
        .expect("a missing input is recorded, not a store error");
    assert!(chain_store_handle.share_block_exists(&block.block_hash()));

    let transactions_error = validator()
        .validate_share_transactions(&block, &chain_store_handle)
        .unwrap_err();
    assert_eq!(transactions_error.kind(), FailureKind::Consensus);

    let prevouts_error = validator()
        .validate_prevouts(&block, &chain_store_handle)
        .unwrap_err();
    assert_eq!(prevouts_error.kind(), FailureKind::Consensus);
    assert!(
        prevouts_error.to_string().contains("Output not found"),
        "unexpected error: {prevouts_error}"
    );
}
