// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use crate::shares::bch_asert::tests::{
    CHIPNET_AROUND_ACTIVATION, CHIPNET_PAIRS, MAINNET_PAIRS, chipnet_times_around_activation,
    header,
};
use crate::shares::bch_asert::{U256, get_compact, median_time_past};
use bitcoin::TxMerkleNode;
use bitcoin::block::Version;
use bitcoin::consensus::{deserialize, serialize};
use bitcoin::hashes::Hash;
use std::collections::HashMap;

/// A known header on ten-minute blocks: ticks `600 * height` and a next
/// block of 600 ticks, as every header before a switch has.
pub(crate) fn ten_minute_header(height: u32, header: Header) -> KnownBchHeader {
    KnownBchHeader {
        height,
        header,
        ticks: i64::from(height) * 600,
        next_interval: 600,
    }
}

/// A known parent from a real pair, at its real height.
fn known(height: u32, hex_header: &str) -> KnownBchHeader {
    ten_minute_header(height, header(hex_header))
}

/// No ancestors known beyond the parent.
fn no_ancestors(_: &BlockHash) -> Option<KnownBchHeader> {
    None
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
        let checked = rules
            .check(&child, &parent, no_ancestors, now_for(&child))
            .unwrap();
        assert_eq!(checked.height, parent_height + 1);
        assert_eq!(checked.header, child);
        assert_eq!(checked.ticks, parent.ticks + 600);
        assert_eq!(checked.next_interval, 600);
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
        if child.time > parent.header.time + parent.tick_info().min_difficulty_gap() {
            assert_eq!(child.bits.to_consensus(), BCH_POW_LIMIT_BITS);
            minimum_difficulty += 1;
        }
        rules
            .check(&child, &parent, no_ancestors, now_for(&child))
            .unwrap();
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
    let error = rules
        .check(&child, &parent, no_ancestors, now_for(&child))
        .unwrap_err();
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
    assert!(child.time <= parent.header.time + parent.tick_info().min_difficulty_gap());
    child.bits = CompactTarget::from_consensus(BCH_POW_LIMIT_BITS);
    let error = rules
        .check(&child, &parent, no_ancestors, now_for(&child))
        .unwrap_err();
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
        BchAsert::MAINNET.next_bits(parent.tick_info(), parent.header.time)
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
        .check(&unrelated, &parent, no_ancestors, now_for(&unrelated))
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
    let error = rules
        .check(&child, &parent, no_ancestors, now_for(&child))
        .unwrap_err();
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
    rules
        .check(&child, &parent, no_ancestors, at_the_limit)
        .unwrap();
    let error = rules
        .check(&child, &parent, no_ancestors, at_the_limit - 1)
        .unwrap_err();
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
            .next_bits(parent.tick_info(), parent.header.time);
        assert_eq!(chipnet.sizing_bits(&parent), asert);
        if child.time > parent.header.time + parent.tick_info().min_difficulty_gap() {
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

/// A known header's encoding carries its ticks and next interval, so a
/// restart keeps them, and a stored entry with ticks or an interval the CHIP
/// does not allow is refused.
#[test]
fn a_known_header_round_trips_its_encoding() {
    let parent = KnownBchHeader {
        ticks: 199_470_060,
        next_interval: 60,
        ..known(MAINNET_PAIRS[6].0, MAINNET_PAIRS[6].1)
    };
    let bytes = serialize(&parent);
    assert_eq!(bytes.len(), 100);
    assert_eq!(deserialize::<KnownBchHeader>(&bytes).unwrap(), parent);
    let refused = serialize(&KnownBchHeader {
        next_interval: 7,
        ..parent
    });
    assert!(deserialize::<KnownBchHeader>(&refused).is_err());
    let negative = serialize(&KnownBchHeader {
        ticks: -1,
        ..parent
    });
    assert!(deserialize::<KnownBchHeader>(&negative).is_err());
}

/// Synthetic linked headers with the given `(time, bits)`, each on the one
/// before, the first on `prev`. `tag` changes every merkle root, for a
/// competing branch. They carry no real proof of work.
fn linked_headers(prev: BlockHash, blocks: &[(u32, u32)], tag: u8) -> Vec<Header> {
    let mut headers: Vec<Header> = Vec::with_capacity(blocks.len());
    for (index, (time, bits)) in blocks.iter().enumerate() {
        let mut root = [tag; 32];
        root[..8].copy_from_slice(&(index as u64).to_le_bytes());
        headers.push(Header {
            version: Version::from_consensus(0x2000_0000),
            prev_blockhash: headers.last().map_or(prev, Header::block_hash),
            merkle_root: TxMerkleNode::from_byte_array(root),
            time: *time,
            bits: CompactTarget::from_consensus(*bits),
            nonce: 0,
        });
    }
    headers
}

/// The headers a node knows, by hash, as its store holds them.
#[derive(Default)]
struct Known(HashMap<BlockHash, KnownBchHeader>);

impl Known {
    fn lookup(&self) -> impl Fn(&BlockHash) -> Option<KnownBchHeader> + '_ {
        |hash| self.0.get(hash).copied()
    }

    /// Learn `headers` in order on `parent` as a node does on arrival, and
    /// return them as known.
    fn learn(
        &mut self,
        rules: &BchHeaderRules,
        parent: KnownBchHeader,
        headers: &[Header],
    ) -> Vec<KnownBchHeader> {
        let mut parent = parent;
        let mut learned = Vec::with_capacity(headers.len());
        for header in headers {
            let known = rules.known_on(*header, &parent, self.lookup()).unwrap();
            self.0.insert(known.block_hash(), known);
            learned.push(known);
            parent = known;
        }
        learned
    }
}

/// The header at `height` in `branch`.
fn at(branch: &[KnownBchHeader], height: u32) -> KnownBchHeader {
    *branch.iter().find(|known| known.height == height).unwrap()
}

/// Real chipnet blocks 332,429 to 332,460 (heights, times and nBits; no
/// real hashes, so the headers carry no proof of work), learned in order on
/// a new store whose oldest header, 332,429, takes the pins' ticks. Each
/// header gets the ticks and next interval Knuth reports (332,450: ticks
/// 199,470,000 and a next block of 60; 332,451: 199,470,060), and from
/// 332,440 on the nBits Bitcoin Cash requires on each known parent is the
/// block's real nBits, the minimum-difficulty blocks included. The full
/// check passes everything but proof of work.
#[test]
fn chipnets_headers_around_the_activation_get_knuths_ticks_and_bits() {
    let rules = BchHeaderRules::for_network(BitcoinCashNetwork::Chipnet);
    let real_bits: HashMap<u32, u32> = CHIPNET_AROUND_ACTIVATION
        .iter()
        .map(|(height, _, bits)| (*height, *bits))
        .collect();
    let blocks: Vec<(u32, u32)> = chipnet_times_around_activation()
        .iter()
        .map(|(height, time)| (*time, real_bits.get(height).copied().unwrap_or(0x1a21_8c7e)))
        .collect();
    let headers = linked_headers(BlockHash::all_zeros(), &blocks, 0x11);
    let tick_info = rules.tick_schedule().tick_info(332_429);
    let oldest = KnownBchHeader {
        height: 332_429,
        header: headers[0],
        ticks: tick_info.ticks,
        next_interval: tick_info.next_interval,
    };
    let mut known = Known::default();
    known.0.insert(oldest.block_hash(), oldest);
    let learned = known.learn(&rules, oldest, &headers[1..]);
    assert_eq!(at(&learned, 332_449).ticks, 199_469_400);
    assert_eq!(at(&learned, 332_449).next_interval, 600);
    assert_eq!(at(&learned, 332_450).ticks, 199_470_000);
    assert_eq!(at(&learned, 332_450).next_interval, 60);
    assert_eq!(at(&learned, 332_451).ticks, 199_470_060);
    assert_eq!(at(&learned, 332_451).next_interval, 60);
    assert_eq!(at(&learned, 332_460).ticks, 199_470_600);
    // Every header's values are the pins' too.
    for known in &learned {
        assert_eq!(
            known.tick_info(),
            rules.tick_schedule().tick_info(known.height),
            "block {}",
            known.height
        );
    }
    let mut minimum_difficulty = Vec::with_capacity(4);
    for pair in learned.windows(2).filter(|pair| pair[1].height >= 332_440) {
        let [parent, child] = pair else {
            unreachable!()
        };
        assert_eq!(
            rules.required_bits(parent, child.header.time),
            Some(child.header.bits),
            "block {}",
            child.height
        );
        if child.header.bits.to_consensus() == BCH_POW_LIMIT_BITS {
            minimum_difficulty.push(child.height);
        }
        let error = rules
            .check(
                &child.header,
                parent,
                known.lookup(),
                now_for(&child.header),
            )
            .unwrap_err();
        assert!(
            matches!(error, BchHeaderError::ProofOfWork(_)),
            "block {}: {error}",
            child.height
        );
    }
    assert_eq!(minimum_difficulty, [332_441, 332_445, 332_451, 332_454]);
}

/// The height of the first block in `chain` whose own median time past,
/// over it and its ten ancestors in `times` (oldest first, aligned with
/// `chain`), reaches `activation_time`.
fn first_reaching(chain: &[KnownBchHeader], times: &[u32], activation_time: i64) -> Option<u32> {
    (10..times.len())
        .find(|&index| median_time_past(&times[index - 10..=index]).unwrap() >= activation_time)
        .map(|index| chain[index].height)
}

/// Two branches off chipnet block 332,444: the real one, whose median time
/// past reaches the activation time at 332,450, and one dated slower, which
/// reaches it at a later block. A node that knows both gives each block the
/// next interval its own branch's median time past decides: at 332,450 the
/// real branch's next block is 60 ticks and the slow branch's 600, and the
/// slow branch switches at its own block. A reorg from one onto the other
/// needs nothing learned again.
#[test]
fn a_reorg_across_the_activation_follows_each_branchs_median_time_past() {
    let rules = BchHeaderRules::for_network(BitcoinCashNetwork::Chipnet);
    let activation_time = 1_794_744_000;
    let times = chipnet_times_around_activation();
    // The shared chain, 332,429 to 332,444.
    let shared: Vec<(u32, u32)> = times[..16]
        .iter()
        .map(|(_, time)| (*time, 0x1a21_8c7e))
        .collect();
    let shared_headers = linked_headers(BlockHash::all_zeros(), &shared, 0x22);
    let oldest = ten_minute_header(332_429, shared_headers[0]);
    let mut known = Known::default();
    known.0.insert(oldest.block_hash(), oldest);
    let mut shared_known = vec![oldest];
    shared_known.extend(known.learn(&rules, oldest, &shared_headers[1..]));
    let fork = *shared_known.last().unwrap();
    assert_eq!(fork.height, 332_444);

    // The real branch, 332,445 to 332,460.
    let real: Vec<(u32, u32)> = times[16..]
        .iter()
        .map(|(_, time)| (*time, 0x1a21_8c7e))
        .collect();
    let real_branch = known.learn(
        &rules,
        fork,
        &linked_headers(fork.block_hash(), &real, 0x33),
    );
    // The slow branch: blocks 20 seconds apart from just after 332,444, so
    // its median time past stays below the activation time until its later
    // blocks, dated past it, outnumber the rest.
    let slow: Vec<(u32, u32)> = (0..16)
        .map(|index| {
            let time = if index < 8 {
                1_794_743_310 + 20 * index
            } else {
                1_794_750_000 + 60 * index
            };
            (time, 0x1a21_8c7e)
        })
        .collect();
    let slow_branch = known.learn(
        &rules,
        fork,
        &linked_headers(fork.block_hash(), &slow, 0x44),
    );

    let chain_of = |branch: &[KnownBchHeader]| -> (Vec<KnownBchHeader>, Vec<u32>) {
        let mut chain = shared_known.clone();
        chain.extend_from_slice(branch);
        let times = chain.iter().map(|known| known.header.time).collect();
        (chain, times)
    };
    let (real_chain, real_times) = chain_of(&real_branch);
    let (slow_chain, slow_times) = chain_of(&slow_branch);
    let real_switch = first_reaching(&real_chain, &real_times, activation_time).unwrap();
    let slow_switch = first_reaching(&slow_chain, &slow_times, activation_time).unwrap();
    assert_eq!(real_switch, 332_450);
    assert!(slow_switch > 332_450, "{slow_switch}");

    for (branch, switch) in [(&real_branch, real_switch), (&slow_branch, slow_switch)] {
        for known in branch {
            let expected = if known.height >= switch { 60 } else { 600 };
            assert_eq!(known.next_interval, expected, "block {}", known.height);
        }
    }
    assert_eq!(at(&real_branch, 332_450).next_interval, 60);
    assert_eq!(at(&slow_branch, 332_450).next_interval, 600);
    // The ticks part from the switch on: the slow branch's blocks are still
    // ten minutes.
    assert_eq!(at(&real_branch, 332_452).ticks, 199_470_120);
    assert_eq!(at(&slow_branch, 332_452).ticks, 332_452 * 600);
    // So the nBits each branch requires after the same height differ, each
    // ASERT on its own branch's ticks and interval.
    let real_parent = at(&real_branch, 332_451);
    let slow_parent = at(&slow_branch, 332_451);
    assert_eq!(
        rules.sizing_bits(&real_parent),
        BchAsert::CHIPNET.next_bits(
            TickInfo {
                ticks: 199_470_060,
                next_interval: 60,
            },
            real_parent.header.time
        )
    );
    assert_eq!(
        rules.sizing_bits(&slow_parent),
        BchAsert::CHIPNET.next_bits(
            TickInfo {
                ticks: 332_451 * 600,
                next_interval: 600,
            },
            slow_parent.header.time
        )
    );
    assert_ne!(
        rules.sizing_bits(&real_parent),
        rules.sizing_bits(&slow_parent)
    );
    // Knowing both branches changes neither: each header was worked out
    // from its own ancestors.
    for known_header in real_branch.iter().chain(&slow_branch) {
        assert_eq!(known.0[&known_header.block_hash()], *known_header);
    }
}

/// Mainnet has no block pinned: a chain whose median time past crosses
/// 2027-05-15 12:00 UTC switches to one-minute blocks by itself at the block
/// that crosses it, and the nBits required after that block are for a block
/// ten times as frequent, where the pins still say ten minutes.
#[test]
fn mainnet_switches_by_itself_where_median_time_past_crosses() {
    let rules = BchHeaderRules::for_network(BitcoinCashNetwork::Mainnet);
    let activation_time: u32 = 1_810_382_400;
    // Blocks 600 seconds apart from 15 intervals before the activation time:
    // the block at index i has median time past `first + 600 * (i - 5)`,
    // which reaches the activation time at index 20.
    let first = activation_time - 600 * 15;
    let blocks: Vec<(u32, u32)> = (0..40)
        .map(|index| (first + 600 * index, 0x1802_8000))
        .collect();
    let headers = linked_headers(BlockHash::all_zeros(), &blocks, 0x55);
    let oldest = ten_minute_header(999_990, headers[0]);
    let mut known = Known::default();
    known.0.insert(oldest.block_hash(), oldest);
    let mut chain = vec![oldest];
    chain.extend(known.learn(&rules, oldest, &headers[1..]));
    let activation = chain[20];
    assert_eq!(activation.height, 1_000_010);
    let window: Vec<u32> = blocks[10..=20].iter().map(|(time, _)| *time).collect();
    assert_eq!(median_time_past(&window), Some(i64::from(activation_time)));
    for known in &chain {
        let expected = if known.height >= activation.height {
            60
        } else {
            600
        };
        assert_eq!(known.next_interval, expected, "block {}", known.height);
        // The pins have no mainnet switch.
        assert_eq!(rules.tick_schedule().interval_after(known.height), 600);
    }
    assert_eq!(activation.ticks, 1_000_010 * 600);
    assert_eq!(chain[21].ticks, 1_000_010 * 600 + 60);
    assert_eq!(chain[39].ticks, 1_000_010 * 600 + 19 * 60);
    let asert = BchAsert::MAINNET;
    let ten_minutes = asert
        .next_target(
            TickInfo {
                ticks: activation.ticks,
                next_interval: 600,
            },
            activation.header.time,
        )
        .unwrap();
    assert_eq!(
        rules.required_bits(&activation, activation.header.time + 60),
        Some(get_compact(ten_minutes * U256::from(10u64)))
    );
    assert_ne!(
        rules.required_bits(&activation, activation.header.time + 60),
        asert.next_bits(
            asert.tick_schedule.tick_info(activation.height),
            activation.header.time
        )
    );
}
