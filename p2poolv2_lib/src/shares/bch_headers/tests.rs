// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use crate::shares::bch_asert::tests::{CHIPNET_PAIRS, MAINNET_PAIRS, header};
use bitcoin::consensus::{deserialize, serialize};

/// A known parent from a real pair, at its real height.
fn known(height: u32, hex_header: &str) -> KnownBchHeader {
    KnownBchHeader {
        height,
        header: header(hex_header),
    }
}

/// Just after the child's own time, as a node would check it on arrival.
fn now_for(child: &Header) -> u32 {
    child.time + 1
}

#[test]
fn real_mainnet_children_pass_the_exact_check() {
    let rules = BchHeaderRules::for_network(BitcoinCashNetwork::Mainnet);
    for (parent_height, parent_hex, child_hex) in MAINNET_PAIRS {
        let parent = known(parent_height, parent_hex);
        let child = header(child_hex);
        let checked = rules.check(&child, &parent, now_for(&child)).unwrap();
        assert_eq!(checked.height, parent_height + 1);
        assert_eq!(checked.header, child);
    }
}

/// Chipnet includes block 300,001, dated more than 20 minutes after its
/// parent, which must carry the proof-of-work limit's nBits.
#[test]
fn real_chipnet_children_pass_the_exact_check_including_a_minimum_difficulty_block() {
    let rules = BchHeaderRules::for_network(BitcoinCashNetwork::Chipnet);
    let mut minimum_difficulty = 0;
    for (parent_height, parent_hex, child_hex) in CHIPNET_PAIRS {
        let parent = known(parent_height, parent_hex);
        let child = header(child_hex);
        // The anchor-era blocks are at the limit by ASERT itself; count the
        // ones the 20-minute rule puts there.
        if child.time > parent.header.time + MIN_DIFFICULTY_GAP {
            assert_eq!(child.bits.to_consensus(), BCH_POW_LIMIT_BITS);
            minimum_difficulty += 1;
        }
        rules.check(&child, &parent, now_for(&child)).unwrap();
    }
    assert_eq!(minimum_difficulty, 1);
}

/// A header with the band check's tolerance but not the exact nBits fails:
/// with its parent known, nothing but the required nBits is allowed.
#[test]
fn a_header_one_step_off_the_required_bits_fails() {
    let rules = BchHeaderRules::for_network(BitcoinCashNetwork::Mainnet);
    let (parent_height, parent_hex, child_hex) = MAINNET_PAIRS[2];
    let parent = known(parent_height, parent_hex);
    let mut child = header(child_hex);
    child.bits = CompactTarget::from_consensus(child.bits.to_consensus() + 1);
    let error = rules.check(&child, &parent, now_for(&child)).unwrap_err();
    assert!(matches!(error, BchHeaderError::Bits { .. }), "{error}");
}

/// On chipnet a block within 20 minutes of its parent may not take the
/// minimum difficulty.
#[test]
fn a_test_network_block_within_twenty_minutes_may_not_use_the_limit() {
    let rules = BchHeaderRules::for_network(BitcoinCashNetwork::Chipnet);
    let (parent_height, parent_hex, child_hex) = CHIPNET_PAIRS[2];
    let parent = known(parent_height, parent_hex);
    let mut child = header(child_hex);
    assert!(child.time <= parent.header.time + MIN_DIFFICULTY_GAP);
    child.bits = CompactTarget::from_consensus(BCH_POW_LIMIT_BITS);
    let error = rules.check(&child, &parent, now_for(&child)).unwrap_err();
    assert!(matches!(error, BchHeaderError::Bits { .. }), "{error}");
}

/// Mainnet never allows the minimum difficulty, whatever the gap.
#[test]
fn mainnet_requires_asert_bits_even_after_a_long_gap() {
    let rules = BchHeaderRules::for_network(BitcoinCashNetwork::Mainnet);
    let (parent_height, parent_hex, _) = MAINNET_PAIRS[4];
    let parent = known(parent_height, parent_hex);
    let late = parent.header.time + 3 * 60 * 60;
    assert_eq!(
        rules.required_bits(&parent, late),
        BchAsert::MAINNET.next_bits(parent_height, parent.header.time)
    );
}

#[test]
fn a_header_on_another_parent_fails() {
    let rules = BchHeaderRules::for_network(BitcoinCashNetwork::Mainnet);
    let (parent_height, parent_hex, _) = MAINNET_PAIRS[0];
    let (_, _, unrelated_hex) = MAINNET_PAIRS[3];
    let parent = known(parent_height, parent_hex);
    let unrelated = header(unrelated_hex);
    let error = rules
        .check(&unrelated, &parent, now_for(&unrelated))
        .unwrap_err();
    assert!(matches!(error, BchHeaderError::NotChild { .. }), "{error}");
}

/// A header whose hash misses its own target fails, even with the right
/// nBits: changing the nonce keeps everything else.
#[test]
fn a_header_without_its_proof_of_work_fails() {
    let rules = BchHeaderRules::for_network(BitcoinCashNetwork::Mainnet);
    let (parent_height, parent_hex, child_hex) = MAINNET_PAIRS[1];
    let parent = known(parent_height, parent_hex);
    let mut child = header(child_hex);
    child.nonce = child.nonce.wrapping_add(1);
    let error = rules.check(&child, &parent, now_for(&child)).unwrap_err();
    assert!(matches!(error, BchHeaderError::ProofOfWork(_)), "{error}");
}

/// BCHN's two-hour future limit: one second past it fails, at it passes.
#[test]
fn a_header_more_than_two_hours_ahead_of_the_clock_fails() {
    let rules = BchHeaderRules::for_network(BitcoinCashNetwork::Mainnet);
    let (parent_height, parent_hex, child_hex) = MAINNET_PAIRS[5];
    let parent = known(parent_height, parent_hex);
    let child = header(child_hex);
    let at_the_limit = child.time - MAX_FUTURE_BLOCK_TIME;
    rules.check(&child, &parent, at_the_limit).unwrap();
    let error = rules.check(&child, &parent, at_the_limit - 1).unwrap_err();
    assert!(
        matches!(error, BchHeaderError::TooFarAhead { .. }),
        "{error}"
    );
}

/// Regtest never retargets: the child repeats its parent's nBits.
#[test]
fn regtest_requires_the_parents_bits() {
    let rules = BchHeaderRules::for_network(BitcoinCashNetwork::Regtest);
    let parent = known(10, MAINNET_PAIRS[0].1);
    assert_eq!(
        rules.required_bits(&parent, parent.header.time + 600),
        Some(parent.header.bits)
    );
}

/// The window's nBits is the child's required nBits, except that a late
/// test network child does not drop it to the limit.
#[test]
fn sizing_bits_are_the_asert_bits_whatever_the_paying_blocks_time() {
    let chipnet = BchHeaderRules::for_network(BitcoinCashNetwork::Chipnet);
    let mut late = 0;
    for (parent_height, parent_hex, child_hex) in CHIPNET_PAIRS {
        let parent = known(parent_height, parent_hex);
        let child = header(child_hex);
        let asert = BchAsert::for_network(BitcoinCashNetwork::Chipnet)
            .unwrap()
            .next_bits(parent_height, parent.header.time);
        assert_eq!(chipnet.sizing_bits(&parent), asert);
        if child.time > parent.header.time + MIN_DIFFICULTY_GAP {
            assert_ne!(chipnet.sizing_bits(&parent), Some(child.bits));
            late += 1;
        } else {
            assert_eq!(chipnet.sizing_bits(&parent), Some(child.bits));
        }
    }
    assert_eq!(late, 1);
    let mainnet = BchHeaderRules::for_network(BitcoinCashNetwork::Mainnet);
    for (parent_height, parent_hex, child_hex) in MAINNET_PAIRS {
        let parent = known(parent_height, parent_hex);
        assert_eq!(mainnet.sizing_bits(&parent), Some(header(child_hex).bits));
    }
}

/// Regtest sizes by the parent's nBits; scalenet, unchecked, by none.
#[test]
fn sizing_bits_follow_the_network_bits_rule() {
    let parent = known(10, MAINNET_PAIRS[0].1);
    assert_eq!(
        BchHeaderRules::for_network(BitcoinCashNetwork::Regtest).sizing_bits(&parent),
        Some(parent.header.bits)
    );
    assert_eq!(
        BchHeaderRules::for_network(BitcoinCashNetwork::Scalenet).sizing_bits(&parent),
        None
    );
}

#[test]
fn a_known_header_round_trips_its_encoding() {
    let parent = known(MAINNET_PAIRS[6].0, MAINNET_PAIRS[6].1);
    let bytes = serialize(&parent);
    assert_eq!(bytes.len(), 84);
    assert_eq!(deserialize::<KnownBchHeader>(&bytes).unwrap(), parent);
}
