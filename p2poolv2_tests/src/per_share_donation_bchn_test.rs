// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The Bitcoin Cash per-share donation against a real BCHN regtest node.
//!
//! Each window share's own donation comes off its own weight, and the
//! donation key is a payee like any other (`BCH_PER_SHARE_CUTS_RULE_TAG`).
//! These tests build a real share chain on the regtest mini genesis share,
//! size its window for BCHN's next block, split that block's coinbase as the
//! producer and every validator do (`build_pplns_outputs`), mine it on BCHN
//! and read the coinbase back from BCHN. Each payee's amount is checked to
//! the satoshi against amounts the test derives itself from what each share
//! committed.
//!
//! Every regtest block is at the proof-of-work limit, so its window falls
//! back to the share cap and holds every mature share, genesis included
//! (`MIN_SIZING_BLOCK_DIFFICULTY`), and the first 100 shares of a chain are
//! always mature. Genesis commits no donation, so a block that pays genesis
//! beside miner shares donates less than 1%, short by genesis's weight. A
//! window without genesis is the unit test
//! `test_bch_default_donation_is_paid_from_each_window_share`.
//!
//! The window is Bitcoin Cash's (`for_chain`) without the BCH chain rule
//! (`with_bch_header_rules`) a node's window has: these shares carry no real
//! BCH headers to check it against, as in `share_payout_rights_test`. That
//! rule decides which window shares a block pays, genesis among them, not
//! how a paid share's cut is split, so the shortfall above holds only where
//! the rule pays genesis beside other shares.
//!
//! They need a BCHN regtest node and are ignored by default; the nightly
//! sim workflow runs them with the payout tree tests:
//!
//! ```text
//! P2POOL_TEST_BCHN_RPC_URL=http://127.0.0.1:18443 \
//! P2POOL_TEST_BCHN_RPC_USER=user P2POOL_TEST_BCHN_RPC_PASSWORD=password \
//! cargo nextest run -p p2poolv2_tests --run-ignored only -E 'test(bchn_regtest)' -j 1
//! ```

use crate::payout_tree_bchn_test::{NextBlock, bchn, call, mine_coinbase, next_block};
use bitcoin::consensus::encode::deserialize_hex;
use bitcoin::hashes::Hash;
use bitcoin::{Address, Block, PubkeyHash, ScriptBuf, Target, Transaction};
use bitcoindrpc::BitcoindRpcClient;
use p2poolv2_lib::accounting::CoinbasePayouts;
use p2poolv2_lib::accounting::payout::coinbase_budget::PayoutSplit;
use p2poolv2_lib::accounting::payout::payout_distribution::build_pplns_outputs;
use p2poolv2_lib::accounting::payout::sharechain_pplns::PplnsWindow;
use p2poolv2_lib::accounting::payout::sharechain_pplns::pplns_window::{
    PayingBlock, bitcoin_block_difficulty,
};
use p2poolv2_lib::address::bch_share::{BchShareNetwork, encode_payout_address};
use p2poolv2_lib::config::{
    BchShareChain, BitcoinCashNetwork, Chain, DEFAULT_BITCOIN_CASH_DONATION, PayoutAddress,
    bitcoin_cash_donation_address,
};
use p2poolv2_lib::shares::bch_genesis;
use p2poolv2_lib::shares::chain::chain_store_handle::ChainStoreHandle;
use p2poolv2_lib::shares::share_block::ShareBlock;
use p2poolv2_lib::test_utils::{TestShareBlockBuilder, setup_test_chain_store_handle};
use serde_json::json;
use std::collections::HashMap;
use tempfile::TempDir;

/// The legacy network Bitcoin Cash regtest payout addresses and difficulty
/// use, `BitcoinCashNetwork::Regtest.legacy_network()`.
const NETWORK: bitcoin::Network = bitcoin::Network::Regtest;
/// Basis points in one.
const BASIS_POINTS: u128 = 10_000;
/// The finding node's own fee, 1%: on Bitcoin Cash only a share's own
/// committed fee is paid, so no test block pays it.
const NODE_FEE: u16 = 100;

fn donation_key() -> PayoutAddress {
    bitcoin_cash_donation_address(BitcoinCashNetwork::Regtest)
}

fn miner(seed: u8) -> PayoutAddress {
    Address::p2pkh(PubkeyHash::from_byte_array([seed; 20]), NETWORK).into()
}

fn node_fee_address() -> PayoutAddress {
    miner(0xfe)
}

/// A share's exact work, as Bitcoin Cash weighs it.
fn work(share: &ShareBlock) -> u128 {
    let work = Target::from_compact(share.header.bits)
        .to_work()
        .to_le_bytes();
    u128::from_le_bytes(work[..16].try_into().unwrap())
}

/// A share chain on the regtest mini genesis, each share stored and confirmed
/// the way the node does once it is valid. Shares are at genesis's own bits,
/// the mini chain's ceiling: the easiest share a regtest pool mines. A chain
/// of fewer than 100 shares has every share mature, so a block's window holds
/// every share.
struct ShareChain {
    store: ChainStoreHandle,
    _store_dir: TempDir,
    genesis: ShareBlock,
    tip: ShareBlock,
    /// Every confirmed share, oldest first, genesis included: the window.
    shares: Vec<ShareBlock>,
}

impl ShareChain {
    async fn new() -> Self {
        let (store, store_dir) = setup_test_chain_store_handle(true).await;
        let genesis = bch_genesis::build(BchShareNetwork::Regtest, BchShareChain::Mini).unwrap();
        store.init_or_setup_genesis(genesis.clone()).await.unwrap();
        Self {
            store,
            _store_dir: store_dir,
            tip: genesis.clone(),
            shares: vec![genesis.clone()],
            genesis,
        }
    }

    /// Confirm a share on the tip paying `payout` and committing `donation`
    /// basis points to the donation key, or no donation for `None`.
    async fn mine(&mut self, payout: &PayoutAddress, donation: Option<u16>) {
        let height = self.shares.len() as u32;
        let mut builder = TestShareBlockBuilder::new()
            .payout_address(payout.clone())
            .bits(self.genesis.header.bits)
            .prev_share_blockhash(self.tip.block_hash().to_string())
            .nonce(0x6400_0000 + height)
            .time(self.genesis.header.time + height);
        if let Some(basis_points) = donation {
            builder = builder.donation(donation_key(), basis_points);
        }
        let share = builder.build();
        self.store.add_share_block(share.clone()).await.unwrap();
        self.store
            .organise_header(share.header.clone())
            .await
            .unwrap();
        self.store
            .mark_block_valid(share.block_hash())
            .await
            .unwrap();
        self.store.organise_block().await.unwrap();
        assert_eq!(
            self.store.get_tip_height().unwrap(),
            Some(height),
            "share {height} did not become the confirmed tip"
        );
        self.tip = share.clone();
        self.shares.push(share);
    }
}

/// The payouts of BCHN's next block for a share on `chain`'s tip: the
/// window's weights for that block, split as the producer and every
/// validator split them. The finding node's own donation and fee are passed,
/// as the notifier passes them, and Bitcoin Cash takes neither.
fn next_payouts(chain: &ShareChain, next: &NextBlock) -> CoinbasePayouts {
    let mut window = PplnsWindow::new(NETWORK).for_chain(Chain::BitcoinCash);
    window.update(&chain.store).unwrap();
    let paying = PayingBlock::new(bitcoin_block_difficulty(next.bits, NETWORK), next.previous);
    let weights = window
        .get_distribution_from_start_hash(chain.tip.block_hash(), paying, &chain.store)
        .unwrap();
    build_pplns_outputs(
        next.coinbase_value,
        &weights,
        &Some(donation_key()),
        Some(DEFAULT_BITCOIN_CASH_DONATION),
        &Some(node_fee_address()),
        Some(NODE_FEE),
        PayoutSplit::for_chain(Chain::BitcoinCash, false),
    )
    .unwrap()
}

/// Mine `payouts` on BCHN and return the coinbase of the block BCHN stored.
async fn mine_and_read_back(node: &BitcoindRpcClient, payouts: &CoinbasePayouts) -> Transaction {
    let (coinbase, hash, _) = mine_coinbase(node, payouts).await;
    assert_eq!(
        call(node, "getbestblockhash", Vec::new()).await,
        json!(hash.to_string()),
        "BCHN did not make the block its tip"
    );
    let raw = call(node, "getblock", vec![json!(hash.to_string()), json!(0)]).await;
    let block: Block = deserialize_hex(raw.as_str().unwrap()).unwrap();
    assert_eq!(block.block_hash(), hash);
    assert_eq!(block.txdata[0], coinbase);
    block.txdata[0].clone()
}

/// The coinbase's payouts, by locking script.
fn paid(coinbase: &Transaction) -> HashMap<ScriptBuf, u64> {
    let mut paid = HashMap::new();
    for output in &coinbase.output {
        *paid.entry(output.script_pubkey.clone()).or_insert(0) += output.value.to_sat();
    }
    paid
}

/// What a coinbase of `value` pays for a window of `shares`, from the
/// per-share rule and the canonical split as documented, not from the
/// window's code. A share of work `w` committing donation `d` (basis points)
/// credits its payee `w * (10^4 - d) * 10^4` and the donation key
/// `w * d * 10^4`; genesis commits none. Each payee gets `value * weight /
/// total` rounded down, and the remainder goes to the last payee in
/// canonical order: ascending weight, ties by locking script, then by
/// address text.
fn expected_payouts(value: u64, shares: &[ShareBlock]) -> HashMap<ScriptBuf, u64> {
    let mut weights: HashMap<PayoutAddress, u128> = HashMap::new();
    for share in shares {
        let work = work(share);
        let donation = share.header.donation.map_or(0, u128::from);
        *weights
            .entry(share.header.miner_bitcoin_address.clone())
            .or_insert(0) += work * (BASIS_POINTS - donation) * BASIS_POINTS;
        if let Some(address) = &share.header.donation_address {
            *weights.entry(address.clone()).or_insert(0) += work * donation * BASIS_POINTS;
        }
    }
    let total: u128 = weights.values().sum();
    let mut payees: Vec<(u128, ScriptBuf, String, u64)> = weights
        .iter()
        .map(|(address, weight)| {
            (
                *weight,
                address.script_pubkey(),
                address.to_string(),
                (u128::from(value) * weight / total) as u64,
            )
        })
        .collect();
    payees.sort();
    let remainder = value - payees.iter().map(|payee| payee.3).sum::<u64>();
    payees.last_mut().unwrap().3 += remainder;
    payees
        .into_iter()
        .map(|(_, script, _, amount)| (script, amount))
        .collect()
}

/// The donation key's part: each share's own cut of its own part of
/// `value`, summed, rounded down. The key is never the largest payee here,
/// so it takes no rounding remainder.
fn expected_donation(value: u64, shares: &[ShareBlock]) -> u64 {
    let committed: u128 = shares
        .iter()
        .map(|share| work(share) * share.header.donation.map_or(0, u128::from))
        .sum();
    let total: u128 = shares.iter().map(work).sum();
    (u128::from(value) * committed / (BASIS_POINTS * total)) as u64
}

/// The case a regtest pool meets first, seen in Knuth's end-to-end runs:
/// blocks found before any share-chain share, whose window holds only the
/// genesis share. Genesis commits no donation, so each coinbase pays the
/// genesis key, a NUMS key no one can spend from, the whole value, and pays
/// neither the donation key nor the finding node's fee address, though the
/// node donates 1%.
#[tokio::test]
#[ignore = "needs a BCHN regtest node: set P2POOL_TEST_BCHN_RPC_URL"]
async fn bchn_regtest_a_window_of_only_genesis_pays_genesis_alone() {
    let node = bchn();
    let chain = ShareChain::new().await;
    let genesis_payee = chain.genesis.header.miner_bitcoin_address.clone();
    assert!(
        encode_payout_address(genesis_payee.clone(), BchShareNetwork::Regtest)
            .unwrap()
            .starts_with("bchreg:qqmt9aztcr"),
        "the genesis payee is not the NUMS key Knuth's runs saw"
    );

    for _ in 0..2 {
        let next = next_block(&node).await;
        let payouts = next_payouts(&chain, &next);
        let coinbase = mine_and_read_back(&node, &payouts).await;

        assert_eq!(
            paid(&coinbase),
            HashMap::from([(genesis_payee.script_pubkey(), next.coinbase_value)])
        );
        assert_eq!(
            paid(&coinbase),
            expected_payouts(next.coinbase_value, &chain.shares)
        );
        assert!(payouts.tree.is_none());
    }
}

/// Miner shares at the node's default donation, 1%, after genesis, in a
/// window that pays genesis beside them. The donation key is a payee whose
/// amount is the sum of each share's own cut, short of 1% of the block by
/// genesis's weight: three equal shares and genesis donate 3 / 4 of 1%.
/// Each miner is paid its shares' work less
/// their cut, genesis its whole part, and the finding node's fee address
/// nothing.
#[tokio::test]
#[ignore = "needs a BCHN regtest node: set P2POOL_TEST_BCHN_RPC_URL"]
async fn bchn_regtest_default_donations_pay_the_key_each_shares_cut() {
    let node = bchn();
    let mut chain = ShareChain::new().await;
    for payout in [miner(1), miner(2), miner(1)] {
        chain
            .mine(&payout, Some(DEFAULT_BITCOIN_CASH_DONATION))
            .await;
    }
    assert_eq!(chain.shares.len(), 4);

    let next = next_block(&node).await;
    let payouts = next_payouts(&chain, &next);
    let coinbase = mine_and_read_back(&node, &payouts).await;
    let paid = paid(&coinbase);

    let value = next.coinbase_value;
    let donation = paid[&donation_key().script_pubkey()];
    assert_eq!(donation, expected_donation(value, &chain.shares));
    assert_eq!(donation, value * 3 / 400);
    assert!(donation < value / 100);
    assert_eq!(paid, expected_payouts(value, &chain.shares));
    assert!(!paid.contains_key(&node_fee_address().script_pubkey()));
    assert_eq!(paid.values().sum::<u64>(), value);
}

/// Each miner's own choice at login is its shares' cut, whatever the
/// finding node donates: a share mined with `donation=0.5` commits 50 basis
/// points, one mined with `donation=0` commits no donation, and one with no
/// choice the node's default, 1% (what `prepared_notify`'s donation tests
/// show each login commits). The key gets exactly the sum of those cuts, and
/// the miner that opted out is paid its whole part.
#[tokio::test]
#[ignore = "needs a BCHN regtest node: set P2POOL_TEST_BCHN_RPC_URL"]
async fn bchn_regtest_each_miners_chosen_donation_is_its_own_shares_cut() {
    let node = bchn();
    let mut chain = ShareChain::new().await;
    chain.mine(&miner(1), Some(50)).await;
    chain.mine(&miner(2), None).await;
    chain
        .mine(&miner(3), Some(DEFAULT_BITCOIN_CASH_DONATION))
        .await;

    let next = next_block(&node).await;
    let payouts = next_payouts(&chain, &next);
    let coinbase = mine_and_read_back(&node, &payouts).await;
    let paid = paid(&coinbase);

    let value = next.coinbase_value;
    let donation = paid[&donation_key().script_pubkey()];
    assert_eq!(donation, expected_donation(value, &chain.shares));
    assert_eq!(donation, value * 150 / 40_000);
    assert_eq!(paid, expected_payouts(value, &chain.shares));
    let quarter = value / 4;
    assert!(paid[&miner(2).script_pubkey()] >= quarter);
    assert_eq!(paid[&miner(1).script_pubkey()], value * 9_950 / 40_000);
    assert_eq!(paid[&miner(3).script_pubkey()], value * 9_900 / 40_000);
    assert!(!paid.contains_key(&node_fee_address().script_pubkey()));
}
