// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! What a window full of weak-share credits costs a node to walk: a timing
//! report, run by hand.

use super::{Chain, ConfirmedHeaderResult, PplnsWindow};

use crate::accounting::payout::coinbase_budget::PayoutSplit;
use crate::accounting::payout::payout_distribution::build_pplns_outputs;
use crate::accounting::payout::payout_levels::PplnsWeights;
use crate::shares::chain::chain_store_handle::MockChainStoreHandle;
use crate::shares::share_block::ShareHeader;
use crate::shares::weak::{WEAK_RATIO_UNIT, WeakCreditGroup, WeakSummary};
use crate::store::block_tx_metadata::{BlockMetadata, ChainMembership, Status};
use crate::test_utils::TestShareBlockBuilder;

use bitcoin::hashes::Hash;
use bitcoin::{Address, BlockHash, CompactTarget, Network, PubkeyHash, Work};

use p2poolv2_wallet::payout_address::PayoutAddress;
use p2poolv2_wallet::share_owner::ShareOwner;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

/// The minimum every share here is mined at: `2^222`.
const MINIMUM: u32 = 0x1c40_0000;
/// The window's weak ceiling, far above every target here.
const CEILING: u32 = 0x1e00_ffff;
/// Signet, the network these payout addresses are for.
const NETWORK: Network = Network::Signet;
/// The coinbase value the builds split, in satoshis.
const REWARD: u64 = 312_500_000;

/// Column names of the one line each case prints.
const REPORT_COLUMNS: &str = concat!(
    "shares,weak_per_share,share_miners,home_miners,payees,",
    "load_ms,walk_ms,build_ms,build_ms_token,",
    "coinbase_outputs,tree_payees",
);

/// One miner's payout address and share-chain owner.
///
/// `class` is the first byte of the key hash and `number` the next four, so
/// the share's own miner and the miners of the weak shares it carries never
/// land on the same address.
fn miner(class: u8, number: u32) -> (PayoutAddress, ShareOwner) {
    let mut key = [0u8; 20];
    key[0] = class;
    key[1..5].copy_from_slice(&number.to_le_bytes());
    let address = Address::p2pkh(PubkeyHash::from_byte_array(key), NETWORK);
    (PayoutAddress::from(address), ShareOwner::CashP2pkh(key))
}

/// The middle sample. One build's time moves from run to run, so the report
/// uses the median of seven.
fn median_ms(mut samples: Vec<f64>) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples[samples.len() / 2]
}

/// One carried weak share credited to home miner `number`: mined on `parent`,
/// building on `bch_prev`, at the share's payout level, with no donation or fee.
fn weak_group(parent: BlockHash, bch_prev: BlockHash, number: u32) -> WeakCreditGroup {
    let (payout_address, share_owner) = miner(2, number);
    WeakCreditGroup {
        parent,
        payout_address,
        share_owner,
        donation_address: None,
        donation: None,
        fee_address: None,
        fee: None,
        payout_level: 5,
        bch_prev,
        count: 1,
    }
}

/// The weak shares one share carries: `weak_per_share` groups of one, paid to
/// home miners in turn.
fn weak_summary(
    header: &ShareHeader,
    index: usize,
    weak_per_share: usize,
    home_miners: usize,
) -> WeakSummary {
    let parent = header.prev_share_blockhash;
    let bch_prev = header.bitcoin_header.prev_blockhash;
    let groups = (0..weak_per_share)
        .map(|group| {
            let number = ((index * weak_per_share + group) % home_miners) as u32;
            weak_group(parent, bch_prev, number)
        })
        .collect();
    //* Crediting reads `groups` only. The proof-of-work hashes are not read.
    WeakSummary {
        pow_hashes: Vec::new(),
        groups,
    }
}

/// Share headers, oldest built first then reversed to newest-first, and the
/// weak summaries of every share after the genesis share.
fn share_chain(
    shares: usize,
    weak_per_share: usize,
    share_miners: usize,
    home_miners: usize,
) -> (
    BlockHash,
    Vec<ConfirmedHeaderResult>,
    Arc<HashMap<BlockHash, WeakSummary>>,
) {
    let mut headers = Vec::with_capacity(shares);
    let capacity = if weak_per_share > 0 {
        shares.saturating_sub(1)
    } else {
        0
    };
    let mut summaries = HashMap::with_capacity(capacity);
    let mut prev_hash = BlockHash::all_zeros().to_string();
    for index in 0..shares {
        let (payout_address, share_owner) = miner(1, (index % share_miners) as u32);
        let mut header = TestShareBlockBuilder::new()
            .prev_share_blockhash(prev_hash)
            .share_owner(share_owner)
            .payout_address(payout_address)
            .bits(CompactTarget::from_consensus(MINIMUM))
            .weak_ratio(4 * WEAK_RATIO_UNIT)
            .payout_level(5)
            .build()
            .header;
        //* The window asks for a summary only when `weak_count` is above zero,
        //* so the count is what puts this share's credits into the load. It is
        //* set before the hash, which the next share links to.
        let carries_weak = index > 0 && weak_per_share > 0;
        if carries_weak {
            header.weak_count = weak_per_share as u16;
        }
        let blockhash = header.block_hash();
        if carries_weak {
            summaries.insert(
                blockhash,
                weak_summary(&header, index, weak_per_share, home_miners),
            );
        }
        prev_hash = blockhash.to_string();
        headers.push(ConfirmedHeaderResult {
            height: index as u32,
            blockhash,
            header,
        });
    }
    let tip_hash = headers
        .last()
        .expect("a case names at least one share")
        .blockhash;
    headers.reverse();
    (tip_hash, headers, Arc::new(summaries))
}

/// The store `update` reads: the tip, its metadata, the headers, and the
/// weak summaries the headers' counts name.
fn mock_store(
    tip_hash: BlockHash,
    headers: Vec<ConfirmedHeaderResult>,
    summaries: Arc<HashMap<BlockHash, WeakSummary>>,
) -> MockChainStoreHandle {
    let tip_height = headers
        .first()
        .expect("a case names at least one share")
        .height;
    let mut store = MockChainStoreHandle::default();
    store.expect_get_chain_tip().returning(move || Ok(tip_hash));
    store.expect_get_block_metadata().returning(move |_| {
        Ok(BlockMetadata {
            expected_height: Some(tip_height),
            chain_work: Work::from_le_bytes([0u8; 32]),
            status: Status::BlockValid,
            chain: ChainMembership::Confirmed,
        })
    });
    store
        .expect_get_confirmed_headers_in_range()
        .returning(move |_, _| Ok(headers.clone()));
    //* A header with `weak_count` of zero is not asked about. The comparison
    //* case carries none, so there is no call to expect.
    if !summaries.is_empty() {
        let summaries = Arc::clone(&summaries);
        store
            .expect_get_weak_summaries()
            .returning(move |share_hashes| {
                let found: HashMap<BlockHash, WeakSummary> = share_hashes
                    .iter()
                    .filter_map(|hash| summaries.get(hash).map(|summary| (*hash, summary.clone())))
                    .collect();
                Ok(found)
            });
    }
    store
}

/// Median milliseconds of seven coinbase builds, and either the direct output
/// count or the token build's tree payee count.
fn median_build_ms(weights: &PplnsWeights, token_share: bool, case: &str) -> (f64, usize) {
    let mut samples = Vec::with_capacity(7);
    let mut built = None;
    for _ in 0..7 {
        let started = Instant::now();
        built = Some(
            build_pplns_outputs(
                REWARD,
                weights,
                &None,
                None,
                &None,
                None,
                PayoutSplit::for_chain(Chain::BitcoinCash, token_share),
            )
            .unwrap_or_else(|error| panic!("{case}, token share {token_share}: {error}")),
        );
        samples.push(started.elapsed().as_secs_f64() * 1_000.0);
    }
    let outputs = built.expect("seven builds ran");
    let count = if token_share {
        outputs.tree.as_ref().map_or(0, |tree| tree.payees().len())
    } else {
        outputs.outputs.len()
    };
    (median_ms(samples), count)
}

/// Timing report, not a check: what a Bitcoin Cash window costs to load,
/// walk and pay when its shares carry weak-share credits, and what the same
/// cap costs with none. The load finds each payee's key by scanning the
/// window's payees (`AddressKeys::key_for`), so its time grows with shares
/// times weak shares times payees; the cases keep the payees few enough to
/// finish in minutes. Run with
/// `cargo test --release -p p2poolv2_lib weak_share_credits -- --ignored --nocapture`.
#[test]
#[ignore = "timing report, run by hand"]
fn validation_cost_with_weak_share_credits() {
    println!("{REPORT_COLUMNS}");
    for (shares, weak_per_share, share_miners, home_miners) in [
        (1_800, 32, 100, 5_000),     // a pool at 10% of Bitcoin Cash
        (1_800, 32, 100, 50_000),    // the same with ten times the home miners
        (18_000, 32, 1_000, 50_000), // a pool at 1%
        (120_960, 32, 1_000, 5_000), // the window's cap, a pool below 0.15%
        (120_960, 0, 1_000, 0),      // the cap without weak shares, to compare
    ] {
        let case = format!("{shares} shares, {weak_per_share} weak each");
        let (tip_hash, headers, summaries) =
            share_chain(shares, weak_per_share, share_miners, home_miners);
        let store = mock_store(tip_hash, headers, summaries);
        let mut window = PplnsWindow::new(NETWORK)
            .for_chain(Chain::BitcoinCash)
            .with_weak_ceiling(CompactTarget::from_consensus(CEILING))
            .with_window_sizes(0, shares);

        let started = Instant::now();
        window
            .update(&store)
            .unwrap_or_else(|error| panic!("{case} load: {error}"));
        let load_ms = started.elapsed().as_secs_f64() * 1_000.0;

        //* The tip is already cached, and this paying block names no Bitcoin
        //* Cash prev, so the walk reads nothing from the store. Weak credits
        //* are already on the cached entries.
        let started = Instant::now();
        let weights = window
            .get_weights_from_start_hash(
                tip_hash,
                (1_u128 << 80).into(),
                &MockChainStoreHandle::default(),
            )
            .unwrap_or_else(|error| panic!("{case} walk: {error}"));
        let walk_ms = started.elapsed().as_secs_f64() * 1_000.0;
        let payees = weights.weights.len();
        //* Every home miner has a weak share in each case, so a walk that
        //* left the weak credits out, or credited them nothing, shows here.
        assert_eq!(payees, share_miners.min(shares) + home_miners, "{case}");
        assert!(weights.weights.values().all(|weight| *weight > 0), "{case}");

        let (build_ms, coinbase_outputs) = median_build_ms(&weights, false, &case);
        let (build_ms_token, tree_payees) = median_build_ms(&weights, true, &case);
        println!(
            "{shares},{weak_per_share},{share_miners},{home_miners},{payees},\
             {load_ms:.1},{walk_ms:.2},{build_ms:.2},{build_ms_token:.2},\
             {coinbase_outputs},{tree_payees}"
        );
    }
}
