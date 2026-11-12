// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//* Weak-work credit tests. Each loads a short Bitcoin Cash chain into the
//* window through `update` and reads the distribution anchored at its tip,
//* with no maturity skip and a paying difficulty that sizes nothing, so every
//* loaded share is paid. The minimum is `2^222`, the parent's weak ratio 4,
//* so its weak target is `2^224`, below the ceiling the window is given.

use super::super::{Chain, ConfirmedHeaderResult, DIFFICULTY_SCALE, PplnsWindow, ShareCuts};
use super::WeakEntry;
use crate::shares::chain::chain_store_handle::MockChainStoreHandle;
use crate::shares::weak::{WEAK_RATIO_UNIT, WeakShare, WeakSummary, includer_part, target_work};
use crate::store::block_tx_metadata::{BlockMetadata, ChainMembership, Status};
use crate::test_utils::TestShareBlockBuilder;
use bitcoin::hashes::Hash;
use bitcoin::{Address, BlockHash, CompactTarget, Network, PubkeyHash, ScriptBuf, Target, Work};
use p2poolv2_wallet::payout_address::PayoutAddress;
use p2poolv2_wallet::share_owner::ShareOwner;
use std::collections::HashMap;

/// The minimum every share here is mined at: `2^222`.
const MINIMUM: u32 = 0x1c40_0000;
/// Four times the minimum, a share heavier than it: `2^220`.
const FOUR_TIMES_HARDER: u32 = 0x1c10_0000;
/// The parent's weak target at a weak ratio of 4: `2^224`.
const WEAK_TARGET: u32 = 0x1d01_0000;
/// A ceiling far above every target here.
const CEILING: u32 = 0x1e00_ffff;

#[test]
fn a_share_at_the_minimum_is_credited_at_its_parents_weak_work() {
    let parent_payout = PayoutAddress::from(Address::p2pkh(
        PubkeyHash::from_byte_array([0x11; 20]),
        Network::Regtest,
    ));
    let share_payout = PayoutAddress::from(Address::p2pkh(
        PubkeyHash::from_byte_array([0x22; 20]),
        Network::Regtest,
    ));
    let parent = TestShareBlockBuilder::new()
        .share_owner(ShareOwner::CashP2pkh([0x11; 20]))
        .payout_address(parent_payout.clone())
        .bits(CompactTarget::from_consensus(MINIMUM))
        .weak_ratio(4 * WEAK_RATIO_UNIT)
        .build()
        .header;
    let share = TestShareBlockBuilder::new()
        .share_owner(ShareOwner::CashP2pkh([0x22; 20]))
        .payout_address(share_payout.clone())
        .prev_share_blockhash(parent.block_hash().to_string())
        .bits(CompactTarget::from_consensus(MINIMUM))
        .build()
        .header;
    let tip = share.block_hash();
    let headers = vec![
        ConfirmedHeaderResult {
            height: 1,
            blockhash: tip,
            header: share,
        },
        ConfirmedHeaderResult {
            height: 0,
            blockhash: parent.block_hash(),
            header: parent,
        },
    ];
    let mut store = MockChainStoreHandle::default();
    store.expect_get_chain_tip().returning(move || Ok(tip));
    store.expect_get_block_metadata().returning(|_| {
        Ok(BlockMetadata {
            expected_height: Some(1),
            chain_work: Work::from_le_bytes([0u8; 32]),
            status: Status::BlockValid,
            chain: ChainMembership::Confirmed,
        })
    });
    store
        .expect_get_confirmed_headers_in_range()
        .returning(move |_, _| Ok(headers.clone()));
    let mut window = PplnsWindow::new(Network::Testnet)
        .for_chain(Chain::BitcoinCash)
        .with_weak_ceiling(CompactTarget::from_consensus(CEILING))
        .with_window_sizes(0, 10);
    window.update(&store).unwrap();

    let distribution = window
        .get_distribution_from_start_hash(tip, 0_u128.into(), &MockChainStoreHandle::default())
        .unwrap();

    //* The share is credited at the parent's weak work, a quarter of its
    //* own; the parent, the chain's first share, at its own work.
    let weak_work = target_work(Target::from_compact(CompactTarget::from_consensus(
        WEAK_TARGET,
    )));
    let minimum_work = target_work(Target::from_compact(CompactTarget::from_consensus(MINIMUM)));
    assert_eq!(distribution[&share_payout], weak_work * DIFFICULTY_SCALE);
    assert_eq!(
        distribution[&parent_payout],
        minimum_work * DIFFICULTY_SCALE
    );
    assert_eq!(weak_work, (minimum_work + 1) / 4 - 1);
}

#[test]
fn a_share_heavier_than_the_minimum_is_credited_at_its_own_work() {
    let share_payout = PayoutAddress::from(Address::p2pkh(
        PubkeyHash::from_byte_array([0x22; 20]),
        Network::Regtest,
    ));
    let parent = TestShareBlockBuilder::new()
        .share_owner(ShareOwner::CashP2pkh([0x11; 20]))
        .bits(CompactTarget::from_consensus(MINIMUM))
        .weak_ratio(4 * WEAK_RATIO_UNIT)
        .build()
        .header;
    let share = TestShareBlockBuilder::new()
        .share_owner(ShareOwner::CashP2pkh([0x22; 20]))
        .payout_address(share_payout.clone())
        .prev_share_blockhash(parent.block_hash().to_string())
        .bits(CompactTarget::from_consensus(FOUR_TIMES_HARDER))
        .band(
            CompactTarget::from_consensus(MINIMUM),
            CompactTarget::from_consensus(FOUR_TIMES_HARDER),
            4096,
        )
        .build()
        .header;
    let tip = share.block_hash();
    let headers = vec![
        ConfirmedHeaderResult {
            height: 1,
            blockhash: tip,
            header: share,
        },
        ConfirmedHeaderResult {
            height: 0,
            blockhash: parent.block_hash(),
            header: parent,
        },
    ];
    let mut store = MockChainStoreHandle::default();
    store.expect_get_chain_tip().returning(move || Ok(tip));
    store.expect_get_block_metadata().returning(|_| {
        Ok(BlockMetadata {
            expected_height: Some(1),
            chain_work: Work::from_le_bytes([0u8; 32]),
            status: Status::BlockValid,
            chain: ChainMembership::Confirmed,
        })
    });
    store
        .expect_get_confirmed_headers_in_range()
        .returning(move |_, _| Ok(headers.clone()));
    let mut window = PplnsWindow::new(Network::Testnet)
        .for_chain(Chain::BitcoinCash)
        .with_weak_ceiling(CompactTarget::from_consensus(CEILING))
        .with_window_sizes(0, 10);
    window.update(&store).unwrap();

    let distribution = window
        .get_distribution_from_start_hash(tip, 0_u128.into(), &MockChainStoreHandle::default())
        .unwrap();

    let own_work = target_work(Target::from_compact(CompactTarget::from_consensus(
        FOUR_TIMES_HARDER,
    )));
    assert_eq!(distribution[&share_payout], own_work * DIFFICULTY_SCALE);
}

#[test]
fn a_carried_weak_share_is_credited_at_the_weak_work_less_the_includer_cut() {
    let carrier_payout = PayoutAddress::from(Address::p2pkh(
        PubkeyHash::from_byte_array([0x22; 20]),
        Network::Regtest,
    ));
    let weak_payout = PayoutAddress::from(Address::p2pkh(
        PubkeyHash::from_byte_array([0x33; 20]),
        Network::Regtest,
    ));
    let parent = TestShareBlockBuilder::new()
        .share_owner(ShareOwner::CashP2pkh([0x11; 20]))
        .bits(CompactTarget::from_consensus(MINIMUM))
        .weak_ratio(4 * WEAK_RATIO_UNIT)
        .build()
        .header;
    let weak_share = WeakShare {
        header: TestShareBlockBuilder::new()
            .share_owner(ShareOwner::CashP2pkh([0x33; 20]))
            .payout_address(weak_payout.clone())
            .prev_share_blockhash(parent.block_hash().to_string())
            .bits(CompactTarget::from_consensus(MINIMUM))
            .build()
            .header,
        coinbase_script_sig: ScriptBuf::new(),
        coinbase_value: 1,
        merkle_branch: Vec::new(),
    };
    let carrier = TestShareBlockBuilder::new()
        .share_owner(ShareOwner::CashP2pkh([0x22; 20]))
        .payout_address(carrier_payout.clone())
        .prev_share_blockhash(parent.block_hash().to_string())
        .bits(CompactTarget::from_consensus(MINIMUM))
        .weak_shares(vec![weak_share.clone()])
        .build()
        .header;
    let tip = carrier.block_hash();
    let summary = WeakSummary::of(&[weak_share]);
    let headers = vec![
        ConfirmedHeaderResult {
            height: 1,
            blockhash: tip,
            header: carrier,
        },
        ConfirmedHeaderResult {
            height: 0,
            blockhash: parent.block_hash(),
            header: parent,
        },
    ];
    let mut store = MockChainStoreHandle::default();
    store.expect_get_chain_tip().returning(move || Ok(tip));
    store.expect_get_block_metadata().returning(|_| {
        Ok(BlockMetadata {
            expected_height: Some(1),
            chain_work: Work::from_le_bytes([0u8; 32]),
            status: Status::BlockValid,
            chain: ChainMembership::Confirmed,
        })
    });
    store
        .expect_get_confirmed_headers_in_range()
        .returning(move |_, _| Ok(headers.clone()));
    store
        .expect_get_weak_summaries()
        .returning(move |_| Ok(HashMap::from([(tip, summary.clone())])));
    let mut window = PplnsWindow::new(Network::Testnet)
        .for_chain(Chain::BitcoinCash)
        .with_weak_ceiling(CompactTarget::from_consensus(CEILING))
        .with_window_sizes(0, 10);
    window.update(&store).unwrap();

    let distribution = window
        .get_distribution_from_start_hash(tip, 0_u128.into(), &MockChainStoreHandle::default())
        .unwrap();

    //* The weak share earns 99% of the weak work; the carrier its own weak
    //* work and the other 1%.
    let scaled_weak_work = target_work(Target::from_compact(CompactTarget::from_consensus(
        WEAK_TARGET,
    ))) * DIFFICULTY_SCALE;
    let includer = includer_part(scaled_weak_work);
    assert_eq!(includer, scaled_weak_work / 100);
    assert_eq!(distribution[&weak_payout], scaled_weak_work - includer);
    assert_eq!(distribution[&carrier_payout], scaled_weak_work + includer);
}

/// Per unit of work, a miner at the minimum is paid what a heavier miner
/// is. At a weak ratio of 4 a miner at the minimum finds, on average, four
/// proofs at the weak target per proof at its share target: here one full
/// share carrying three of its own weak shares, from as many hashes as one
/// share at the minimum. A heavier miner four times harder needs four times
/// those hashes for its one share. Crediting the full share at its own work
/// on top of its weak shares would pay the first miner 1.75 times its due.
#[test]
fn per_unit_of_work_a_miner_at_the_minimum_earns_what_a_heavier_miner_earns() {
    let small_payout = PayoutAddress::from(Address::p2pkh(
        PubkeyHash::from_byte_array([0x22; 20]),
        Network::Regtest,
    ));
    let heavy_payout = PayoutAddress::from(Address::p2pkh(
        PubkeyHash::from_byte_array([0x44; 20]),
        Network::Regtest,
    ));
    let parent = TestShareBlockBuilder::new()
        .share_owner(ShareOwner::CashP2pkh([0x11; 20]))
        .bits(CompactTarget::from_consensus(MINIMUM))
        .weak_ratio(4 * WEAK_RATIO_UNIT)
        .build()
        .header;
    let small_weak_share = |nonce: u32| WeakShare {
        header: TestShareBlockBuilder::new()
            .share_owner(ShareOwner::CashP2pkh([0x22; 20]))
            .payout_address(small_payout.clone())
            .prev_share_blockhash(parent.block_hash().to_string())
            .bits(CompactTarget::from_consensus(MINIMUM))
            .nonce(nonce)
            .build()
            .header,
        coinbase_script_sig: ScriptBuf::new(),
        coinbase_value: 1,
        merkle_branch: Vec::new(),
    };
    let weak_shares = vec![
        small_weak_share(1),
        small_weak_share(2),
        small_weak_share(3),
    ];
    let small_share = TestShareBlockBuilder::new()
        .share_owner(ShareOwner::CashP2pkh([0x22; 20]))
        .payout_address(small_payout.clone())
        .prev_share_blockhash(parent.block_hash().to_string())
        .bits(CompactTarget::from_consensus(MINIMUM))
        .weak_shares(weak_shares.clone())
        .build()
        .header;
    let heavy_share = TestShareBlockBuilder::new()
        .share_owner(ShareOwner::CashP2pkh([0x44; 20]))
        .payout_address(heavy_payout.clone())
        .prev_share_blockhash(small_share.block_hash().to_string())
        .bits(CompactTarget::from_consensus(FOUR_TIMES_HARDER))
        .band(
            CompactTarget::from_consensus(MINIMUM),
            CompactTarget::from_consensus(FOUR_TIMES_HARDER),
            4096,
        )
        .build()
        .header;
    let tip = heavy_share.block_hash();
    let small_hash = small_share.block_hash();
    let summary = WeakSummary::of(&weak_shares);
    let headers = vec![
        ConfirmedHeaderResult {
            height: 2,
            blockhash: tip,
            header: heavy_share,
        },
        ConfirmedHeaderResult {
            height: 1,
            blockhash: small_hash,
            header: small_share,
        },
        ConfirmedHeaderResult {
            height: 0,
            blockhash: parent.block_hash(),
            header: parent,
        },
    ];
    let mut store = MockChainStoreHandle::default();
    store.expect_get_chain_tip().returning(move || Ok(tip));
    store.expect_get_block_metadata().returning(|_| {
        Ok(BlockMetadata {
            expected_height: Some(2),
            chain_work: Work::from_le_bytes([0u8; 32]),
            status: Status::BlockValid,
            chain: ChainMembership::Confirmed,
        })
    });
    store
        .expect_get_confirmed_headers_in_range()
        .returning(move |_, _| Ok(headers.clone()));
    store
        .expect_get_weak_summaries()
        .returning(move |_| Ok(HashMap::from([(small_hash, summary.clone())])));
    let mut window = PplnsWindow::new(Network::Testnet)
        .for_chain(Chain::BitcoinCash)
        .with_weak_ceiling(CompactTarget::from_consensus(CEILING))
        .with_window_sizes(0, 10);
    window.update(&store).unwrap();

    let distribution = window
        .get_distribution_from_start_hash(tip, 0_u128.into(), &MockChainStoreHandle::default())
        .unwrap();

    //* Expected hashes: 2^256 / (T + 1) for the small miner's four proofs at
    //* the weak target, which is one minimum's work, and four minimums' work
    //* for the heavy share.
    let minimum_work = target_work(Target::from_compact(CompactTarget::from_consensus(MINIMUM)));
    let small_per_hash = distribution[&small_payout] as f64 / minimum_work as f64;
    let heavy_per_hash = distribution[&heavy_payout] as f64 / (4 * minimum_work) as f64;
    assert!(
        (small_per_hash / heavy_per_hash - 1.0).abs() < 1e-6,
        "small {small_per_hash}, heavy {heavy_per_hash}"
    );
}

/// A weak entry of `miner` at `payout_level`, built as the window builds one
/// from a carried weak share's credit group.
fn weak_entry_at_level(
    window: &mut PplnsWindow,
    miner: PayoutAddress,
    payout_level: u8,
) -> WeakEntry {
    WeakEntry {
        cuts: ShareCuts::default(),
        internal_key: window.address_keys.key_for(miner),
        owner_key: window.owner_keys.key_for(ShareOwner::CashP2pkh([0x44; 20])),
        bch_prev: BlockHash::all_zeros(),
        payout_level,
        count: 1,
        work: 2,
    }
}

/// A weak share is an entry for a payee's level: a miner whose newest
/// credited entry is its weak share in a newer share by another miner is
/// drawn at the weak share's own level.
#[test]
fn a_weak_share_newer_than_its_miners_share_sets_its_level() {
    let mut window = PplnsWindow::new(Network::Regtest).for_chain(Chain::BitcoinCash);
    let miner = PayoutAddress::from(Address::p2pkh(
        PubkeyHash::from_byte_array([1; 20]),
        Network::Regtest,
    ));
    let other = PayoutAddress::from(Address::p2pkh(
        PubkeyHash::from_byte_array([9; 20]),
        Network::Regtest,
    ));
    let anchor = BlockHash::from_byte_array([2; 32]);
    let mut carrier =
        window.build_confirmed_entry(anchor, 1, other, ShareOwner::CashP2pkh([9; 20]), 2, vec![]);
    carrier.payout_level = 1;
    carrier.weak_entries = vec![weak_entry_at_level(&mut window, miner.clone(), 6)];
    let mut older = window.build_confirmed_entry(
        BlockHash::from_byte_array([1; 32]),
        0,
        miner.clone(),
        ShareOwner::CashP2pkh([1; 20]),
        2,
        vec![],
    );
    older.payout_level = 3;
    window.confirmed_entries.push_back(carrier);
    window.confirmed_entries.push_back(older);
    let resolved = window.resolved_window(Vec::new(), 0, 1, HashMap::new());

    let levels = window
        .distribution_for_resolved(anchor, &resolved)
        .unwrap()
        .levels
        .unwrap();

    assert_eq!(levels.level_of(&miner), Some(6));
}

/// A share is newer than the weak shares it carries: its own level counts
/// when both pay one miner.
#[test]
fn a_share_is_newer_than_the_weak_shares_it_carries() {
    let mut window = PplnsWindow::new(Network::Regtest).for_chain(Chain::BitcoinCash);
    let miner = PayoutAddress::from(Address::p2pkh(
        PubkeyHash::from_byte_array([1; 20]),
        Network::Regtest,
    ));
    let anchor = BlockHash::from_byte_array([2; 32]);
    let mut share = window.build_confirmed_entry(
        anchor,
        0,
        miner.clone(),
        ShareOwner::CashP2pkh([1; 20]),
        2,
        vec![],
    );
    share.payout_level = 2;
    share.weak_entries = vec![weak_entry_at_level(&mut window, miner.clone(), 7)];
    window.confirmed_entries.push_back(share);
    let resolved = window.resolved_window(Vec::new(), 0, 1, HashMap::new());

    let levels = window
        .distribution_for_resolved(anchor, &resolved)
        .unwrap()
        .levels
        .unwrap();

    assert_eq!(levels.level_of(&miner), Some(2));
}

/// An uncle is newer than the weak shares the same share carries: the
/// uncle's level counts when both pay one miner.
#[test]
fn an_uncle_is_newer_than_the_weak_shares_beside_it() {
    let mut window = PplnsWindow::new(Network::Regtest).for_chain(Chain::BitcoinCash);
    let miner = PayoutAddress::from(Address::p2pkh(
        PubkeyHash::from_byte_array([1; 20]),
        Network::Regtest,
    ));
    let other = PayoutAddress::from(Address::p2pkh(
        PubkeyHash::from_byte_array([9; 20]),
        Network::Regtest,
    ));
    let mut uncle = window.build_uncle_entry(
        BlockHash::from_byte_array([3; 32]),
        miner.clone(),
        ShareOwner::CashP2pkh([1; 20]),
        2,
    );
    uncle.payout_level = 4;
    let anchor = BlockHash::from_byte_array([2; 32]);
    let mut carrier = window.build_confirmed_entry(
        anchor,
        0,
        other,
        ShareOwner::CashP2pkh([9; 20]),
        2,
        vec![uncle],
    );
    carrier.weak_entries = vec![weak_entry_at_level(&mut window, miner.clone(), 1)];
    window.confirmed_entries.push_back(carrier);
    let resolved = window.resolved_window(Vec::new(), 0, 1, HashMap::new());

    let levels = window
        .distribution_for_resolved(anchor, &resolved)
        .unwrap()
        .levels
        .unwrap();

    assert_eq!(levels.level_of(&miner), Some(4));
}
