// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The Bitcoin Cash hybrid payout against a real BCHN regtest node.
//!
//! BCHN is the authority on both consensus and relay policy, so the payout
//! tree must pass a real BCHN mempool, not only the test script machine.
//! These tests mine a block whose coinbase the hybrid payout splits, mature
//! it, run the node's own payout tree executor against BCHN and check every
//! payee's output to the satoshi. They need a BCHN regtest node and are
//! ignored by default; run them with
//!
//! ```text
//! P2POOL_TEST_BCHN_RPC_URL=http://127.0.0.1:18443 \
//! P2POOL_TEST_BCHN_RPC_USER=user P2POOL_TEST_BCHN_RPC_PASSWORD=password \
//! cargo nextest run -p p2poolv2_tests --run-ignored only -E 'test(bchn_regtest)'
//! ```
//!
//! BCHN regtest requires standard transactions (`fRequireStandard`), as
//! mainnet does, so the node transactions are checked against relay policy
//! too.

use bitcoin::absolute::LockTime;
use bitcoin::block::{Header, Version as BlockVersion};
use bitcoin::consensus::encode::{deserialize_hex, serialize_hex};
use bitcoin::hashes::Hash;
use bitcoin::script::Builder;
use bitcoin::transaction::Version;
use bitcoin::{
    Address, Amount, Block, BlockHash, CompactTarget, Network, OutPoint, PubkeyHash, Sequence,
    Transaction, TxIn, TxMerkleNode, TxOut, Txid, Witness,
};
use bitcoindrpc::BitcoindRpcClient;
use p2poolv2_lib::accounting::CoinbasePayouts;
use p2poolv2_lib::accounting::payout::coinbase_budget::PayoutSplit;
use p2poolv2_lib::accounting::payout::payout_distribution::build_pplns_outputs;
use p2poolv2_lib::config::Chain;
use p2poolv2_lib::node::payout_tree_executor::{
    PayoutTreeExecutor, SETTLED_TIPS, payout_tree_channel,
};
use p2poolv2_lib::store::payout_tree::StoredPayoutTree;
use p2poolv2_lib::test_utils::setup_test_chain_store_handle;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::str::FromStr;

/// The node the tests mine on, from the environment.
fn bchn() -> BitcoindRpcClient {
    let url = std::env::var("P2POOL_TEST_BCHN_RPC_URL")
        .expect("set P2POOL_TEST_BCHN_RPC_URL to a BCHN regtest node's RPC URL");
    let user = std::env::var("P2POOL_TEST_BCHN_RPC_USER").unwrap_or_default();
    let password = std::env::var("P2POOL_TEST_BCHN_RPC_PASSWORD").unwrap_or_default();
    BitcoindRpcClient::new(&url, &user, &password).unwrap()
}

async fn call(node: &BitcoindRpcClient, method: &str, params: Vec<Value>) -> Value {
    node.request(method, params)
        .await
        .unwrap_or_else(|error| panic!("{method} failed: {error}"))
}

/// An address no test spends from, for blocks mined only to move the tip.
fn burn_address() -> String {
    Address::p2pkh(PubkeyHash::from_byte_array([0xee; 20]), Network::Regtest).to_string()
}

async fn mine(node: &BitcoindRpcClient, blocks: u64) {
    call(
        node,
        "generatetoaddress",
        vec![json!(blocks), json!(burn_address())],
    )
    .await;
}

/// `count` P2PKH payees weighing their number.
fn weights(count: u32, seed: u8) -> HashMap<Address, u128> {
    (1..=count)
        .map(|number| {
            let mut hash = [seed; 20];
            hash[..4].copy_from_slice(&number.to_le_bytes());
            (
                Address::p2pkh(PubkeyHash::from_byte_array(hash), Network::Regtest),
                u128::from(number),
            )
        })
        .collect()
}

/// What the next block on BCHN's tip needs: its height, the tip's hash,
/// time and bits, and the coinbase value. BCHN serves `getblocktemplate`
/// only to a connected node, so this reads the tip instead: regtest never
/// retargets, halves the 50-coin subsidy every 150 blocks, and the block
/// carries no transaction, so no fees.
struct NextBlock {
    height: u64,
    previous: BlockHash,
    time: u32,
    bits: CompactTarget,
    coinbase_value: u64,
}

async fn next_block(node: &BitcoindRpcClient) -> NextBlock {
    let tip = call(node, "getbestblockhash", Vec::new()).await;
    let header = call(node, "getblockheader", vec![tip.clone(), json!(true)]).await;
    let height = header["height"].as_u64().unwrap() + 1;
    NextBlock {
        height,
        previous: BlockHash::from_str(tip.as_str().unwrap()).unwrap(),
        time: header["time"].as_u64().unwrap() as u32 + 1,
        bits: CompactTarget::from_unprefixed_hex(header["bits"].as_str().unwrap()).unwrap(),
        coinbase_value: (50 * 100_000_000u64) >> (height / 150),
    }
}

/// Mine one block whose coinbase pays `payouts` on top of BCHN's tip,
/// returning the coinbase. Asserts BCHN accepts the block.
async fn mine_coinbase(
    node: &BitcoindRpcClient,
    payouts: &CoinbasePayouts,
) -> (Transaction, BlockHash, u64) {
    let next = next_block(node).await;
    let height = next.height;
    let mut outputs: Vec<TxOut> = payouts
        .outputs
        .iter()
        .map(|pair| TxOut {
            value: pair.amount,
            script_pubkey: pair.address.script_pubkey(),
        })
        .collect();
    outputs.extend(payouts.tree_output());
    let coinbase = Transaction {
        version: Version::TWO,
        lock_time: LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::null(),
            script_sig: Builder::new()
                .push_int(height as i64)
                .push_slice(b"p2pool-tree-test")
                .into_script(),
            sequence: Sequence::MAX,
            witness: Witness::new(),
        }],
        output: outputs,
    };
    let mut header = Header {
        version: BlockVersion::from_consensus(0x2000_0000),
        prev_blockhash: next.previous,
        merkle_root: TxMerkleNode::from_raw_hash(coinbase.compute_txid().to_raw_hash()),
        time: next.time,
        bits: next.bits,
        nonce: 0,
    };
    while header.validate_pow(header.target()).is_err() {
        header.nonce += 1;
    }
    let block = Block {
        header,
        txdata: vec![coinbase.clone()],
    };
    let verdict = call(node, "submitblock", vec![json!(serialize_hex(&block))]).await;
    assert!(verdict.is_null(), "BCHN refused the block: {verdict}");
    (coinbase, block.block_hash(), height)
}

/// The value of output `vout` of `txid` in BCHN's UTXO set, mempool
/// excluded.
async fn utxo_value(node: &BitcoindRpcClient, txid: Txid, vout: u32) -> Option<u64> {
    let output = call(
        node,
        "gettxout",
        vec![json!(txid.to_string()), json!(vout), json!(false)],
    )
    .await;
    output["value"]
        .as_f64()
        .map(|value| Amount::from_btc(value).unwrap().to_sat())
}

/// A token share's coinbase with 700 payees: 77 are paid directly and 623
/// through a two-level payout tree. After 100 blocks the node's executor
/// broadcasts the tree at 1 sat/byte, BCHN accepts every node transaction
/// into its mempool and a block, every payee holds its amount to the
/// satoshi, and the executor marks the tree executed once it settles.
#[tokio::test]
#[ignore = "needs a BCHN regtest node: set P2POOL_TEST_BCHN_RPC_URL"]
async fn bchn_regtest_executes_a_two_level_payout_tree() {
    let node = bchn();
    let info = call(&node, "getblockchaininfo", Vec::new()).await;
    assert_eq!(info["chain"], "regtest");
    mine(&node, 1).await;
    let next = next_block(&node).await;
    let value = next.coinbase_value;
    let payouts = build_pplns_outputs(
        value,
        &weights(700, 0x31),
        &None,
        None,
        &None,
        None,
        PayoutSplit::for_chain(Chain::BitcoinCash, true),
    )
    .unwrap();
    let tree = payouts.tree.clone().unwrap();
    assert_eq!(payouts.outputs.len(), 77);
    assert_eq!(tree.node_count(), 3);

    let (coinbase, blockhash, height) = mine_coinbase(&node, &payouts).await;
    let root_vout = coinbase
        .output
        .iter()
        .position(|output| *output == tree.root_output())
        .unwrap() as u32;
    let stored = StoredPayoutTree::new(
        &tree,
        coinbase.compute_txid(),
        root_vout,
        blockhash,
        height,
        BlockHash::all_zeros(),
    );
    let (chain_store_handle, _store_dir) = setup_test_chain_store_handle(true).await;
    let mut executor = PayoutTreeExecutor::new(
        node.clone(),
        chain_store_handle.clone(),
        payout_tree_channel().1,
    );
    executor.keep(stored.clone()).await;

    // 99 blocks on top: the coinbase cannot be spent in the next block yet.
    mine(&node, 98).await;
    executor.execute_round().await;
    let mempool = call(&node, "getrawmempool", Vec::new()).await;
    assert!(mempool.as_array().unwrap().is_empty());

    mine(&node, 1).await;
    executor.execute_round().await;
    let transactions = tree.node_transactions(1, stored.root_outpoint()).unwrap();
    let mempool = call(&node, "getrawmempool", Vec::new()).await;
    let mempool: Vec<&str> = mempool
        .as_array()
        .unwrap()
        .iter()
        .map(|txid| txid.as_str().unwrap())
        .collect();
    assert_eq!(mempool.len(), transactions.len());
    assert!(
        transactions
            .iter()
            .all(|transaction| mempool.contains(&transaction.compute_txid().to_string().as_str()))
    );

    mine(&node, 1).await;
    // Every payee output of the two nodes paying the payees, to the satoshi,
    // as BCHN's UTXO set holds it.
    let expected: Vec<(Txid, u32, u64)> = transactions[1..]
        .iter()
        .flat_map(|transaction| {
            let txid = transaction.compute_txid();
            transaction
                .output
                .iter()
                .enumerate()
                .map(move |(vout, output)| (txid, vout as u32, output.value.to_sat()))
        })
        .collect();
    let mut held = Vec::with_capacity(expected.len());
    for (txid, vout, _) in &expected {
        held.push((
            *txid,
            *vout,
            utxo_value(&node, *txid, *vout).await.unwrap_or(0),
        ));
    }
    assert_eq!(held, expected);
    assert_eq!(expected.len(), 623);
    let paid: u64 = expected.iter().map(|(_, _, value)| value).sum();
    assert_eq!(paid + tree.total_fee(1).unwrap(), tree.value());

    for _ in 0..SETTLED_TIPS {
        executor.execute_round().await;
        mine(&node, 1).await;
    }
    executor.execute_round().await;
    assert!(
        chain_store_handle
            .get_payout_tree(&stored.coinbase_txid)
            .unwrap()
            .unwrap()
            .executed
    );
}

/// A no-token share pays every miner directly: BCHN accepts a coinbase
/// paying 3,000 payees, about 102 KB, past the 100 KB relay limit, which a
/// coinbase never meets.
#[tokio::test]
#[ignore = "needs a BCHN regtest node: set P2POOL_TEST_BCHN_RPC_URL"]
async fn bchn_regtest_accepts_a_no_token_coinbase_paying_every_miner() {
    let node = bchn();
    let info = call(&node, "getblockchaininfo", Vec::new()).await;
    assert_eq!(info["chain"], "regtest");
    let next = next_block(&node).await;
    let value = next.coinbase_value;
    let payouts = build_pplns_outputs(
        value,
        &weights(3_000, 0x32),
        &None,
        None,
        &None,
        None,
        PayoutSplit::for_chain(Chain::BitcoinCash, false),
    )
    .unwrap();
    assert!(payouts.tree.is_none());

    let (coinbase, blockhash, _) = mine_coinbase(&node, &payouts).await;

    assert!(coinbase.total_size() > 100_000);
    let best = call(&node, "getbestblockhash", Vec::new()).await;
    assert_eq!(best.as_str().unwrap(), blockhash.to_string());
    let raw = call(
        &node,
        "getblock",
        vec![json!(blockhash.to_string()), json!(0)],
    )
    .await;
    let block: Block = deserialize_hex(raw.as_str().unwrap()).unwrap();
    assert_eq!(block.txdata[0], coinbase);
}
