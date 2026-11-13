// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use crate::accounting::payout::payout_levels::PplnsWeights;
use crate::accounting::payout::sharechain_pplns::pplns_window::MockPplnsWindow;
use crate::pool_difficulty::{AsertParent, MockPoolDifficulty};
use crate::shares::chain::chain_store_handle::MockChainStoreHandle;
use crate::shares::validation::FailureKind;
use crate::shares::weak::{GENESIS_WEAK_RATIO, WEAK_DEPTH, WeakSummary, next_weak_ratio};
use crate::store::block_tx_metadata::{BlockMetadata, ChainMembership, Status};
use crate::test_utils::{
    TestShareBlockBuilder, TestWeakShare, build_test_weak_share, build_test_weak_share_paying,
};
use bitcoin::hashes::Hash;
use bitcoin::{Address, CompactTarget, Network, PubkeyHash, Work};
use p2poolv2_wallet::payout_address::PayoutAddress;
use p2poolv2_wallet::share_owner::ShareOwner;

//* Validity tests. A weak share is mined at `HARD`, a target its hash
//* misses, on a parent whose minimum is `EASY`, the validator's ceiling and
//* a target nearly every hash meets: its parent's weak target is then
//* `EASY`, which its hash meets, so it is a weak share and not a full one.

/// A target almost no hash meets: about one in 2^32.
const HARD: u32 = 0x1d00_ffff;
/// A target almost every hash meets: all but one in 2^16.
const EASY: u32 = 0x2100_ffff;
/// A share time after the parent's and before the clock's limit.
const WEAK_TIME: u32 = 1_700_000_100;

/// A weak share for the body checks: only its header's proof-of-work hash
/// matters there, which `nonce` varies.
fn body_weak_share(nonce: u32) -> WeakShare {
    WeakShare {
        header: TestShareBlockBuilder::new()
            .share_owner(ShareOwner::CashP2pkh([0x11; 20]))
            .nonce(nonce)
            .build()
            .header,
        coinbase_script_sig: bitcoin::ScriptBuf::new(),
        coinbase_value: 1,
        merkle_branch: Vec::new(),
    }
}

#[test]
fn a_body_carrying_the_committed_weak_shares_matches_its_header() {
    let share = TestShareBlockBuilder::new()
        .share_owner(ShareOwner::CashP2pkh([0x22; 20]))
        .weak_shares(vec![body_weak_share(1), body_weak_share(2)])
        .build();

    assert!(check_weak_shares_match_header(&share).is_ok());
}

#[test]
fn a_body_dropping_a_committed_weak_share_is_a_bad_copy() {
    let mut share = TestShareBlockBuilder::new()
        .share_owner(ShareOwner::CashP2pkh([0x22; 20]))
        .weak_shares(vec![body_weak_share(1), body_weak_share(2)])
        .build();
    share.weak_shares.pop();

    let error = check_weak_shares_match_header(&share).unwrap_err();
    assert_eq!(error.kind, FailureKind::Consensus);
    assert!(error.message.contains("commits 2"), "{error}");
}

#[test]
fn a_body_swapping_a_committed_weak_share_does_not_match_the_root() {
    let mut share = TestShareBlockBuilder::new()
        .share_owner(ShareOwner::CashP2pkh([0x22; 20]))
        .weak_shares(vec![body_weak_share(1), body_weak_share(2)])
        .build();
    share.weak_shares[1] = body_weak_share(3);

    let error = check_weak_shares_match_header(&share).unwrap_err();
    assert!(error.message.contains("root"), "{error}");
}

/// A repeated last weak share keeps the merkle root, so the repeat itself
/// is refused.
#[test]
fn a_body_repeating_a_weak_share_is_refused_before_the_root() {
    let mut share = TestShareBlockBuilder::new()
        .share_owner(ShareOwner::CashP2pkh([0x22; 20]))
        .weak_shares(vec![
            body_weak_share(1),
            body_weak_share(2),
            body_weak_share(3),
        ])
        .build();
    share.header.weak_count = 4;
    share.weak_shares.push(body_weak_share(3));

    let error = check_weak_shares_match_header(&share).unwrap_err();
    assert!(error.message.contains("more than once"), "{error}");
}

/// One proof of work carried twice under two encodings, the second with
/// another coinbase value: the leaves differ, and the header commits the
/// root over both, but the repeat check compares proof-of-work hashes and
/// refuses it.
#[test]
fn a_body_carrying_one_proof_of_work_under_two_encodings_is_a_repeat() {
    let first = body_weak_share(1);
    let mut second = body_weak_share(1);
    second.coinbase_value += 1;
    assert_ne!(first.committed_hash(), second.committed_hash());
    let share = TestShareBlockBuilder::new()
        .share_owner(ShareOwner::CashP2pkh([0x22; 20]))
        .weak_shares(vec![first, second])
        .build();

    let error = check_weak_shares_match_header(&share).unwrap_err();
    assert_eq!(error.kind, FailureKind::Consensus);
    assert!(error.message.contains("more than once"), "{error}");
}

/// Weak shares under the cap in count but past `WEAK_BYTES` in size are
/// refused. A branch longer than a decoded one may be makes each large.
#[test]
fn weak_shares_past_the_byte_limit_are_refused() {
    let mut large = body_weak_share(1);
    large.merkle_branch = vec![bitcoin::TxMerkleNode::all_zeros(); 100];
    let count = WEAK_BYTES / large.encoded_size() + 1;
    assert!(count <= WEAK_CAP);
    let weak_shares = vec![large; count];

    let error = check_weak_shares_size(&weak_shares).unwrap_err();
    assert!(error.message.contains("bytes"), "{error}");
}

#[test]
fn more_weak_shares_than_the_cap_are_refused() {
    let weak_shares = vec![body_weak_share(1); WEAK_CAP + 1];

    let error = check_weak_shares_size(&weak_shares).unwrap_err();
    assert!(error.message.contains("more than 64"), "{error}");
}

#[test]
fn a_weak_share_on_a_job_at_the_minimum_passes_the_header_rules() {
    let parent = TestShareBlockBuilder::new()
        .share_owner(ShareOwner::CashP2pkh([0x11; 20]))
        .bits(CompactTarget::from_consensus(EASY))
        .build()
        .header;
    let weak_share = build_test_weak_share(TestWeakShare {
        parent_hash: parent.block_hash(),
        bits: CompactTarget::from_consensus(HARD),
        minimum: CompactTarget::from_consensus(HARD),
        weak_ratio: next_weak_ratio(GENESIS_WEAK_RATIO, 0),
        distribution: &HashMap::from([(
            PayoutAddress::from(Address::p2pkh(
                PubkeyHash::from_byte_array([0x55; 20]),
                Network::Regtest,
            )),
            1u128,
        )]),
        time: WEAK_TIME,
        nonce: 1,
    });
    let mut pool_difficulty = MockPoolDifficulty::default();
    pool_difficulty
        .expect_calculate_target_clamped()
        .returning(|_| CompactTarget::from_consensus(HARD));
    let mut store = MockChainStoreHandle::default();
    let asert_parent = AsertParent::from_header(&parent, 0, 0);
    store
        .expect_get_asert_parent()
        .returning(move |_| Ok(asert_parent));
    store
        .expect_get_share_header()
        .returning(move |_| Ok(parent.clone()));
    store.expect_get_block_metadata().returning(|_| {
        Ok(BlockMetadata {
            expected_height: Some(0),
            chain_work: Work::from_le_bytes([0u8; 32]),
            status: Status::BlockValid,
            chain: ChainMembership::Confirmed,
        })
    });
    let validator = DefaultShareValidator::new(pool_difficulty)
        .for_chain(Chain::BitcoinCash)
        .with_target_ceiling(CompactTarget::from_consensus(EASY));

    let result = validator.validate_weak_share_header(&weak_share, &store);

    assert!(result.is_ok(), "{}", result.unwrap_err());
}

/// A hash that meets its own share target is a full share and must travel
/// as one, or one proof of work would be credited twice.
/// Proof-of-work checks are off under the `sim` feature.
#[cfg(not(feature = "sim"))]
#[test]
fn a_weak_share_meeting_its_own_bits_is_refused() {
    let parent = TestShareBlockBuilder::new()
        .share_owner(ShareOwner::CashP2pkh([0x11; 20]))
        .bits(CompactTarget::from_consensus(EASY))
        .build()
        .header;
    let weak_share = build_test_weak_share(TestWeakShare {
        parent_hash: parent.block_hash(),
        bits: CompactTarget::from_consensus(EASY),
        minimum: CompactTarget::from_consensus(EASY),
        weak_ratio: next_weak_ratio(GENESIS_WEAK_RATIO, 0),
        distribution: &HashMap::from([(
            PayoutAddress::from(Address::p2pkh(
                PubkeyHash::from_byte_array([0x55; 20]),
                Network::Regtest,
            )),
            1u128,
        )]),
        time: WEAK_TIME,
        nonce: 1,
    });
    let mut pool_difficulty = MockPoolDifficulty::default();
    pool_difficulty
        .expect_calculate_target_clamped()
        .returning(|_| CompactTarget::from_consensus(EASY));
    let mut store = MockChainStoreHandle::default();
    let asert_parent = AsertParent::from_header(&parent, 0, 0);
    store
        .expect_get_asert_parent()
        .returning(move |_| Ok(asert_parent));
    store
        .expect_get_share_header()
        .returning(move |_| Ok(parent.clone()));
    let validator = DefaultShareValidator::new(pool_difficulty)
        .for_chain(Chain::BitcoinCash)
        .with_target_ceiling(CompactTarget::from_consensus(EASY));

    let error = validator
        .validate_weak_share_header(&weak_share, &store)
        .unwrap_err();

    assert_eq!(error.kind, FailureKind::Consensus);
    assert!(error.message.contains("it is a full share"), "{error}");
}

/// With a weak ratio of one the parent's weak target is its minimum, `HARD`,
/// which the weak share's hash misses.
/// Proof-of-work checks are off under the `sim` feature.
#[cfg(not(feature = "sim"))]
#[test]
fn a_weak_share_missing_its_parents_weak_target_is_refused() {
    use crate::shares::weak::WEAK_RATIO_UNIT;

    let parent = TestShareBlockBuilder::new()
        .share_owner(ShareOwner::CashP2pkh([0x11; 20]))
        .bits(CompactTarget::from_consensus(HARD))
        .weak_ratio(WEAK_RATIO_UNIT)
        .build()
        .header;
    let weak_share = build_test_weak_share(TestWeakShare {
        parent_hash: parent.block_hash(),
        bits: CompactTarget::from_consensus(HARD),
        minimum: CompactTarget::from_consensus(HARD),
        weak_ratio: next_weak_ratio(WEAK_RATIO_UNIT, 0),
        distribution: &HashMap::from([(
            PayoutAddress::from(Address::p2pkh(
                PubkeyHash::from_byte_array([0x55; 20]),
                Network::Regtest,
            )),
            1u128,
        )]),
        time: WEAK_TIME,
        nonce: 1,
    });
    let mut pool_difficulty = MockPoolDifficulty::default();
    pool_difficulty
        .expect_calculate_target_clamped()
        .returning(|_| CompactTarget::from_consensus(HARD));
    let mut store = MockChainStoreHandle::default();
    let asert_parent = AsertParent::from_header(&parent, 0, 0);
    store
        .expect_get_asert_parent()
        .returning(move |_| Ok(asert_parent));
    store
        .expect_get_share_header()
        .returning(move |_| Ok(parent.clone()));
    let validator = DefaultShareValidator::new(pool_difficulty)
        .for_chain(Chain::BitcoinCash)
        .with_target_ceiling(CompactTarget::from_consensus(EASY));

    let error = validator
        .validate_weak_share_header(&weak_share, &store)
        .unwrap_err();

    assert_eq!(error.kind, FailureKind::Consensus);
    assert!(
        error.message.contains("misses its parent's weak target"),
        "{error}"
    );
}

/// A miner on a heavier job inside the band gets no credit for near misses:
/// a weak share's bits must be its minimum.
#[test]
fn a_weak_share_from_a_job_above_the_minimum_is_refused() {
    let parent = TestShareBlockBuilder::new()
        .share_owner(ShareOwner::CashP2pkh([0x11; 20]))
        .bits(CompactTarget::from_consensus(EASY))
        .build()
        .header;
    let weak_share = build_test_weak_share(TestWeakShare {
        parent_hash: parent.block_hash(),
        bits: CompactTarget::from_consensus(0x1c00_ffff),
        minimum: CompactTarget::from_consensus(HARD),
        weak_ratio: next_weak_ratio(GENESIS_WEAK_RATIO, 0),
        distribution: &HashMap::from([(
            PayoutAddress::from(Address::p2pkh(
                PubkeyHash::from_byte_array([0x55; 20]),
                Network::Regtest,
            )),
            1u128,
        )]),
        time: WEAK_TIME,
        nonce: 1,
    });
    let mut pool_difficulty = MockPoolDifficulty::default();
    pool_difficulty
        .expect_calculate_target_clamped()
        .returning(|_| CompactTarget::from_consensus(HARD));
    let mut store = MockChainStoreHandle::default();
    let asert_parent = AsertParent::from_header(&parent, 0, 0);
    store
        .expect_get_asert_parent()
        .returning(move |_| Ok(asert_parent));
    let validator = DefaultShareValidator::new(pool_difficulty)
        .for_chain(Chain::BitcoinCash)
        .with_target_ceiling(CompactTarget::from_consensus(EASY));

    let error = validator
        .validate_weak_share_header(&weak_share, &store)
        .unwrap_err();

    assert_eq!(error.kind, FailureKind::Consensus);
    assert!(error.message.contains("jobs at the minimum"), "{error}");
}

#[test]
fn a_weak_share_whose_coinbase_pays_its_window_passes_the_coinbase_rule() {
    let payee = PayoutAddress::from(Address::p2pkh(
        PubkeyHash::from_byte_array([0x55; 20]),
        Network::Regtest,
    ));
    let distribution = HashMap::from([(payee, 1_000u128)]);
    let weak_share = build_test_weak_share(TestWeakShare {
        parent_hash: BlockHash::from_byte_array([0x66; 32]),
        bits: CompactTarget::from_consensus(HARD),
        minimum: CompactTarget::from_consensus(HARD),
        weak_ratio: GENESIS_WEAK_RATIO,
        distribution: &distribution,
        time: WEAK_TIME,
        nonce: 1,
    });
    let mut window = MockPplnsWindow::default();
    window.expect_network().return_const(Network::Regtest);
    window
        .expect_get_weights_from_start_hash()
        .returning(move |_, _, _| Ok(PplnsWeights::without_levels(distribution.clone())));
    let validator =
        DefaultShareValidator::new(MockPoolDifficulty::default()).for_chain(Chain::BitcoinCash);

    let result = validator.validate_weak_share_coinbase(
        &weak_share,
        &MockChainStoreHandle::default(),
        &mut window,
        &mut WeakPayoutCache::new(),
    );

    assert!(result.is_ok(), "{}", result.unwrap_err());
}

/// The window's payouts are built once for the weak shares of one parent
/// and template; each weak share then costs one coinbase hash.
#[test]
fn weak_shares_on_one_template_share_one_window_walk() {
    let payee = PayoutAddress::from(Address::p2pkh(
        PubkeyHash::from_byte_array([0x55; 20]),
        Network::Regtest,
    ));
    let distribution = HashMap::from([(payee, 1_000u128)]);
    let first = build_test_weak_share(TestWeakShare {
        parent_hash: BlockHash::from_byte_array([0x66; 32]),
        bits: CompactTarget::from_consensus(HARD),
        minimum: CompactTarget::from_consensus(HARD),
        weak_ratio: GENESIS_WEAK_RATIO,
        distribution: &distribution,
        time: WEAK_TIME,
        nonce: 1,
    });
    let second = build_test_weak_share(TestWeakShare {
        parent_hash: BlockHash::from_byte_array([0x66; 32]),
        bits: CompactTarget::from_consensus(HARD),
        minimum: CompactTarget::from_consensus(HARD),
        weak_ratio: GENESIS_WEAK_RATIO,
        distribution: &distribution,
        time: WEAK_TIME + 1,
        nonce: 2,
    });
    let mut window = MockPplnsWindow::default();
    window.expect_network().return_const(Network::Regtest);
    window
        .expect_get_weights_from_start_hash()
        .times(1)
        .returning(move |_, _, _| Ok(PplnsWeights::without_levels(distribution.clone())));
    let validator =
        DefaultShareValidator::new(MockPoolDifficulty::default()).for_chain(Chain::BitcoinCash);
    let mut cache = WeakPayoutCache::new();

    let first_result = validator.validate_weak_share_coinbase(
        &first,
        &MockChainStoreHandle::default(),
        &mut window,
        &mut cache,
    );
    let second_result = validator.validate_weak_share_coinbase(
        &second,
        &MockChainStoreHandle::default(),
        &mut window,
        &mut cache,
    );

    assert!(first_result.is_ok(), "{}", first_result.unwrap_err());
    assert!(second_result.is_ok(), "{}", second_result.unwrap_err());
}

/// 64 weak shares on 64 templates of one Bitcoin Cash block, each paying
/// another coinbase value, share one walk of the window: the weights depend
/// on the parent and the block, not on the value. Each one's payouts are
/// still built for its own value, so its rebuilt coinbase matches the one it
/// was mined on and every one passes.
#[test]
fn weak_shares_on_many_templates_of_one_block_share_one_window_walk() {
    let payee = PayoutAddress::from(Address::p2pkh(
        PubkeyHash::from_byte_array([0x55; 20]),
        Network::Regtest,
    ));
    let distribution = HashMap::from([(payee, 1_000u128)]);
    let mut window = MockPplnsWindow::default();
    window.expect_network().return_const(Network::Regtest);
    let walked = distribution.clone();
    window
        .expect_get_weights_from_start_hash()
        .times(1)
        .returning(move |_, _, _| Ok(PplnsWeights::without_levels(walked.clone())));
    let validator =
        DefaultShareValidator::new(MockPoolDifficulty::default()).for_chain(Chain::BitcoinCash);
    let mut cache = WeakPayoutCache::new();

    let passed = (0..64u32)
        .filter(|template| {
            let weak_share = build_test_weak_share_paying(
                TestWeakShare {
                    parent_hash: BlockHash::from_byte_array([0x66; 32]),
                    bits: CompactTarget::from_consensus(HARD),
                    minimum: CompactTarget::from_consensus(HARD),
                    weak_ratio: GENESIS_WEAK_RATIO,
                    distribution: &distribution,
                    time: WEAK_TIME,
                    nonce: *template,
                },
                5_000_000_000 + u64::from(*template),
            );
            validator
                .validate_weak_share_coinbase(
                    &weak_share,
                    &MockChainStoreHandle::default(),
                    &mut window,
                    &mut cache,
                )
                .is_ok()
        })
        .count();

    assert_eq!(passed, 64);
}

/// The cached weights are those of the weak share's own block: one paying
/// another Bitcoin Cash block walks the window again.
#[test]
fn a_weak_share_paying_another_block_walks_the_window_again() {
    let payee = PayoutAddress::from(Address::p2pkh(
        PubkeyHash::from_byte_array([0x55; 20]),
        Network::Regtest,
    ));
    let distribution = HashMap::from([(payee, 1_000u128)]);
    let first = build_test_weak_share(TestWeakShare {
        parent_hash: BlockHash::from_byte_array([0x66; 32]),
        bits: CompactTarget::from_consensus(HARD),
        minimum: CompactTarget::from_consensus(HARD),
        weak_ratio: GENESIS_WEAK_RATIO,
        distribution: &distribution,
        time: WEAK_TIME,
        nonce: 1,
    });
    let mut second = first.clone();
    second.header.bitcoin_header.prev_blockhash = BlockHash::from_byte_array([0x77; 32]);
    let mut window = MockPplnsWindow::default();
    window.expect_network().return_const(Network::Regtest);
    let walked = distribution.clone();
    window
        .expect_get_weights_from_start_hash()
        .times(2)
        .returning(move |_, _, _| Ok(PplnsWeights::without_levels(walked.clone())));
    let validator =
        DefaultShareValidator::new(MockPoolDifficulty::default()).for_chain(Chain::BitcoinCash);
    let mut cache = WeakPayoutCache::new();

    let first_result = validator.validate_weak_share_coinbase(
        &first,
        &MockChainStoreHandle::default(),
        &mut window,
        &mut cache,
    );
    let _ = validator.validate_weak_share_coinbase(
        &second,
        &MockChainStoreHandle::default(),
        &mut window,
        &mut cache,
    );

    assert!(first_result.is_ok(), "{}", first_result.unwrap_err());
}

/// A coinbase paying someone other than the window anchored at the weak
/// share's parent rebuilds to another txid and another merkle root.
#[test]
fn a_weak_share_whose_coinbase_does_not_pay_its_window_is_refused() {
    let miner = PayoutAddress::from(Address::p2pkh(
        PubkeyHash::from_byte_array([0x55; 20]),
        Network::Regtest,
    ));
    let window_payee = PayoutAddress::from(Address::p2pkh(
        PubkeyHash::from_byte_array([0x77; 20]),
        Network::Regtest,
    ));
    let weak_share = build_test_weak_share(TestWeakShare {
        parent_hash: BlockHash::from_byte_array([0x66; 32]),
        bits: CompactTarget::from_consensus(HARD),
        minimum: CompactTarget::from_consensus(HARD),
        weak_ratio: GENESIS_WEAK_RATIO,
        distribution: &HashMap::from([(miner, 1_000u128)]),
        time: WEAK_TIME,
        nonce: 1,
    });
    let mut window = MockPplnsWindow::default();
    window.expect_network().return_const(Network::Regtest);
    window
        .expect_get_weights_from_start_hash()
        .returning(move |_, _, _| {
            Ok(PplnsWeights::without_levels(HashMap::from([(
                window_payee.clone(),
                1_000u128,
            )])))
        });
    let validator =
        DefaultShareValidator::new(MockPoolDifficulty::default()).for_chain(Chain::BitcoinCash);

    let error = validator
        .validate_weak_share_coinbase(
            &weak_share,
            &MockChainStoreHandle::default(),
            &mut window,
            &mut WeakPayoutCache::new(),
        )
        .unwrap_err();

    assert_eq!(error.kind, FailureKind::Consensus);
    assert!(
        error.message.contains("does not pay its PPLNS window"),
        "{error}"
    );
}

/// A branch that does not lead from the coinbase to the header's merkle root
/// is refused, though the coinbase itself pays the window.
#[test]
fn a_weak_share_whose_branch_does_not_reach_its_merkle_root_is_refused() {
    let payee = PayoutAddress::from(Address::p2pkh(
        PubkeyHash::from_byte_array([0x55; 20]),
        Network::Regtest,
    ));
    let distribution = HashMap::from([(payee, 1_000u128)]);
    let mut weak_share = build_test_weak_share(TestWeakShare {
        parent_hash: BlockHash::from_byte_array([0x66; 32]),
        bits: CompactTarget::from_consensus(HARD),
        minimum: CompactTarget::from_consensus(HARD),
        weak_ratio: GENESIS_WEAK_RATIO,
        distribution: &distribution,
        time: WEAK_TIME,
        nonce: 1,
    });
    weak_share.merkle_branch = vec![bitcoin::TxMerkleNode::from_byte_array([0x88; 32])];
    let mut window = MockPplnsWindow::default();
    window.expect_network().return_const(Network::Regtest);
    window
        .expect_get_weights_from_start_hash()
        .returning(move |_, _, _| Ok(PplnsWeights::without_levels(distribution.clone())));
    let validator =
        DefaultShareValidator::new(MockPoolDifficulty::default()).for_chain(Chain::BitcoinCash);

    let error = validator
        .validate_weak_share_coinbase(
            &weak_share,
            &MockChainStoreHandle::default(),
            &mut window,
            &mut WeakPayoutCache::new(),
        )
        .unwrap_err();

    assert!(error.message.contains("merkle root"), "{error}");
}

/// The carrier is at height 101, the weak share's parent at 53: exactly
/// `WEAK_DEPTH` shares below, the deepest a carried weak share may be.
#[test]
fn a_weak_share_carried_once_within_its_depth_is_placed() {
    let carrier_parent = BlockHash::from_byte_array([0x01; 32]);
    let weak_parent = BlockHash::from_byte_array([0x02; 32]);
    let weak_share = build_test_weak_share(TestWeakShare {
        parent_hash: weak_parent,
        bits: CompactTarget::from_consensus(HARD),
        minimum: CompactTarget::from_consensus(HARD),
        weak_ratio: GENESIS_WEAK_RATIO,
        distribution: &HashMap::from([(
            PayoutAddress::from(Address::p2pkh(
                PubkeyHash::from_byte_array([0x55; 20]),
                Network::Regtest,
            )),
            1u128,
        )]),
        time: WEAK_TIME,
        nonce: 1,
    });
    let carrier = TestShareBlockBuilder::new()
        .share_owner(ShareOwner::CashP2pkh([0x22; 20]))
        .prev_share_blockhash(carrier_parent.to_string())
        .weak_shares(vec![weak_share])
        .build();
    let ancestor = TestShareBlockBuilder::new()
        .share_owner(ShareOwner::CashP2pkh([0x22; 20]))
        .build()
        .header;
    let mut store = MockChainStoreHandle::default();
    store.expect_get_block_metadata_batch().returning(move |_| {
        let metadata = |height| BlockMetadata {
            expected_height: Some(height),
            chain_work: Work::from_le_bytes([0u8; 32]),
            status: Status::BlockValid,
            chain: ChainMembership::Confirmed,
        };
        Ok(vec![
            (carrier_parent, metadata(100)),
            (weak_parent, metadata(101 - WEAK_DEPTH)),
        ])
    });
    store
        .expect_get_share_header()
        .returning(move |_| Ok(ancestor.clone()));
    let validator =
        DefaultShareValidator::new(MockPoolDifficulty::default()).for_chain(Chain::BitcoinCash);

    let result = validator.validate_carried_weak_positions(&carrier, &store);

    assert!(result.is_ok(), "{}", result.unwrap_err());
}

/// The carrier is at height 101, the weak share's parent at 52: one more
/// than `WEAK_DEPTH` shares below.
#[test]
fn a_weak_share_whose_parent_is_too_deep_is_refused() {
    let carrier_parent = BlockHash::from_byte_array([0x01; 32]);
    let weak_parent = BlockHash::from_byte_array([0x02; 32]);
    let weak_share = build_test_weak_share(TestWeakShare {
        parent_hash: weak_parent,
        bits: CompactTarget::from_consensus(HARD),
        minimum: CompactTarget::from_consensus(HARD),
        weak_ratio: GENESIS_WEAK_RATIO,
        distribution: &HashMap::from([(
            PayoutAddress::from(Address::p2pkh(
                PubkeyHash::from_byte_array([0x55; 20]),
                Network::Regtest,
            )),
            1u128,
        )]),
        time: WEAK_TIME,
        nonce: 1,
    });
    let carrier = TestShareBlockBuilder::new()
        .share_owner(ShareOwner::CashP2pkh([0x22; 20]))
        .prev_share_blockhash(carrier_parent.to_string())
        .weak_shares(vec![weak_share])
        .build();
    let mut store = MockChainStoreHandle::default();
    store.expect_get_block_metadata_batch().returning(move |_| {
        let metadata = |height| BlockMetadata {
            expected_height: Some(height),
            chain_work: Work::from_le_bytes([0u8; 32]),
            status: Status::BlockValid,
            chain: ChainMembership::Confirmed,
        };
        Ok(vec![
            (carrier_parent, metadata(100)),
            (weak_parent, metadata(100 - WEAK_DEPTH)),
        ])
    });
    let validator =
        DefaultShareValidator::new(MockPoolDifficulty::default()).for_chain(Chain::BitcoinCash);

    let error = validator
        .validate_carried_weak_positions(&carrier, &store)
        .unwrap_err();

    assert_eq!(error.kind, FailureKind::Consensus);
    assert!(
        error
            .message
            .contains(&format!("{} shares below its carrier", WEAK_DEPTH + 1)),
        "{error}"
    );
}

/// The carrier's parent already carries the weak share, so carrying it
/// again would credit one proof of work twice on one chain.
#[test]
fn a_weak_share_an_ancestor_already_carries_is_refused() {
    let carrier_parent = BlockHash::from_byte_array([0x01; 32]);
    let weak_parent = BlockHash::from_byte_array([0x02; 32]);
    let weak_share = build_test_weak_share(TestWeakShare {
        parent_hash: weak_parent,
        bits: CompactTarget::from_consensus(HARD),
        minimum: CompactTarget::from_consensus(HARD),
        weak_ratio: GENESIS_WEAK_RATIO,
        distribution: &HashMap::from([(
            PayoutAddress::from(Address::p2pkh(
                PubkeyHash::from_byte_array([0x55; 20]),
                Network::Regtest,
            )),
            1u128,
        )]),
        time: WEAK_TIME,
        nonce: 1,
    });
    let earlier_carrier = TestShareBlockBuilder::new()
        .share_owner(ShareOwner::CashP2pkh([0x22; 20]))
        .weak_shares(vec![weak_share.clone()])
        .build();
    let summary = WeakSummary::of(&earlier_carrier.weak_shares);
    let earlier_header = earlier_carrier.header;
    let carrier = TestShareBlockBuilder::new()
        .share_owner(ShareOwner::CashP2pkh([0x22; 20]))
        .prev_share_blockhash(carrier_parent.to_string())
        .weak_shares(vec![weak_share])
        .build();
    let mut store = MockChainStoreHandle::default();
    store.expect_get_block_metadata_batch().returning(move |_| {
        let metadata = |height| BlockMetadata {
            expected_height: Some(height),
            chain_work: Work::from_le_bytes([0u8; 32]),
            status: Status::BlockValid,
            chain: ChainMembership::Confirmed,
        };
        Ok(vec![
            (carrier_parent, metadata(10)),
            (weak_parent, metadata(9)),
        ])
    });
    store
        .expect_get_share_header()
        .returning(move |_| Ok(earlier_header.clone()));
    store
        .expect_get_weak_summaries()
        .returning(move |_| Ok(HashMap::from([(carrier_parent, summary.clone())])));
    let validator =
        DefaultShareValidator::new(MockPoolDifficulty::default()).for_chain(Chain::BitcoinCash);

    let error = validator
        .validate_carried_weak_positions(&carrier, &store)
        .unwrap_err();

    assert_eq!(error.kind, FailureKind::Consensus);
    assert!(
        error.message.contains("already carried by an ancestor"),
        "{error}"
    );
}

/// The carrier's parent carries a weak share, and the carrier carries the
/// same proof of work with another coinbase value: another leaf of the weak
/// root, but the carried-once rule compares proof-of-work hashes, so it is
/// still one proof of work credited twice and refused.
#[test]
fn a_weak_share_an_ancestor_carries_is_refused_under_another_encoding() {
    let carrier_parent = BlockHash::from_byte_array([0x01; 32]);
    let weak_parent = BlockHash::from_byte_array([0x02; 32]);
    let weak_share = build_test_weak_share(TestWeakShare {
        parent_hash: weak_parent,
        bits: CompactTarget::from_consensus(HARD),
        minimum: CompactTarget::from_consensus(HARD),
        weak_ratio: GENESIS_WEAK_RATIO,
        distribution: &HashMap::from([(
            PayoutAddress::from(Address::p2pkh(
                PubkeyHash::from_byte_array([0x55; 20]),
                Network::Regtest,
            )),
            1u128,
        )]),
        time: WEAK_TIME,
        nonce: 1,
    });
    let mut reencoded = weak_share.clone();
    reencoded.coinbase_value += 1;
    assert_ne!(reencoded.committed_hash(), weak_share.committed_hash());
    let earlier_carrier = TestShareBlockBuilder::new()
        .share_owner(ShareOwner::CashP2pkh([0x22; 20]))
        .weak_shares(vec![weak_share])
        .build();
    let summary = WeakSummary::of(&earlier_carrier.weak_shares);
    let earlier_header = earlier_carrier.header;
    let carrier = TestShareBlockBuilder::new()
        .share_owner(ShareOwner::CashP2pkh([0x22; 20]))
        .prev_share_blockhash(carrier_parent.to_string())
        .weak_shares(vec![reencoded])
        .build();
    let mut store = MockChainStoreHandle::default();
    store.expect_get_block_metadata_batch().returning(move |_| {
        let metadata = |height| BlockMetadata {
            expected_height: Some(height),
            chain_work: Work::from_le_bytes([0u8; 32]),
            status: Status::BlockValid,
            chain: ChainMembership::Confirmed,
        };
        Ok(vec![
            (carrier_parent, metadata(10)),
            (weak_parent, metadata(9)),
        ])
    });
    store
        .expect_get_share_header()
        .returning(move |_| Ok(earlier_header.clone()));
    store
        .expect_get_weak_summaries()
        .returning(move |_| Ok(HashMap::from([(carrier_parent, summary.clone())])));
    let validator =
        DefaultShareValidator::new(MockPoolDifficulty::default()).for_chain(Chain::BitcoinCash);

    let error = validator
        .validate_carried_weak_positions(&carrier, &store)
        .unwrap_err();

    assert_eq!(error.kind, FailureKind::Consensus);
    assert!(
        error.message.contains("already carried by an ancestor"),
        "{error}"
    );
}
