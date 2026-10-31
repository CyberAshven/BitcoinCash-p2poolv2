// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use crate::config::BitcoinCashNetwork;
use crate::shares::bch_asert::tests::{MAINNET_PAIRS, header};
use bitcoin::block::Version;
use bitcoin::{CompactTarget, TxMerkleNode};
use std::collections::BTreeMap;

/// A block tree a sender knows, with its best chain. Headers here need no
/// proof of work: `required_path` and `merge_for_message` only read links.
#[derive(Default)]
struct Tree {
    headers: HashMap<BlockHash, KnownBchHeader>,
    best: BTreeMap<u32, BlockHash>,
}

impl Tree {
    /// A child of `parent` (the all-zero hash for a first block); `tag`
    /// makes siblings distinct.
    fn add(&mut self, parent: BlockHash, tag: u8) -> KnownBchHeader {
        let height = self
            .headers
            .get(&parent)
            .map_or(0, |parent| parent.height + 1);
        let known = KnownBchHeader {
            height,
            header: Header {
                version: Version::from_consensus(0x2000_0000),
                prev_blockhash: parent,
                merkle_root: TxMerkleNode::from_byte_array([tag; 32]),
                time: 1_700_000_000 + height * 600,
                bits: CompactTarget::from_consensus(0x207f_ffff),
                nonce: height,
            },
        };
        self.headers.insert(known.block_hash(), known);
        known
    }

    /// A best chain of `length` blocks from a first block.
    fn with_best_chain(length: u32) -> (Self, Vec<KnownBchHeader>) {
        let mut tree = Self::default();
        let mut chain = Vec::new();
        let mut parent = BlockHash::all_zeros();
        for _ in 0..length {
            let block = tree.add(parent, 0);
            tree.best.insert(block.height, block.block_hash());
            parent = block.block_hash();
            chain.push(block);
        }
        (tree, chain)
    }

    fn path(&self, prev: BlockHash, parent_prev: Option<BlockHash>) -> Vec<KnownBchHeader> {
        required_path(
            prev,
            parent_prev,
            |hash| self.headers.get(hash).copied(),
            |height| self.best.get(&height).copied(),
        )
    }
}

#[test]
fn a_share_on_its_parents_block_carries_nothing() {
    let (tree, chain) = Tree::with_best_chain(5);
    let prev = chain[4].block_hash();
    assert!(tree.path(prev, Some(prev)).is_empty());
}

#[test]
fn the_first_share_on_a_new_block_carries_that_block() {
    let (tree, chain) = Tree::with_best_chain(5);
    assert_eq!(
        tree.path(chain[4].block_hash(), Some(chain[3].block_hash())),
        vec![chain[4]]
    );
}

/// A block that lost an orphan race is off the best chain, so every share
/// on it carries it, even one whose parent built on it too.
#[test]
fn a_share_on_an_orphan_always_carries_the_orphan() {
    let (mut tree, chain) = Tree::with_best_chain(5);
    let orphan = tree.add(chain[3].block_hash(), 9);
    assert_eq!(
        tree.path(orphan.block_hash(), Some(orphan.block_hash())),
        vec![orphan]
    );
    assert_eq!(
        tree.path(orphan.block_hash(), Some(chain[3].block_hash())),
        vec![orphan]
    );
}

/// After a reorg the sender's best chain is a branch its peers may not
/// have: a share on it carries the branch back to the fork point.
#[test]
fn after_a_reorg_a_share_carries_the_branch_back_to_the_fork_point() {
    let (mut tree, chain) = Tree::with_best_chain(4);
    // The old tip 3 loses to a branch 3', 4' on block 2.
    let branch_3 = tree.add(chain[2].block_hash(), 7);
    let branch_4 = tree.add(branch_3.block_hash(), 7);
    tree.best.insert(3, branch_3.block_hash());
    tree.best.insert(4, branch_4.block_hash());
    assert_eq!(
        tree.path(branch_4.block_hash(), Some(chain[3].block_hash())),
        vec![branch_3, branch_4]
    );
}

/// A hostile fork's blocks are all carried, so every receiver can place the
/// share and agree it is not paid; a fork longer than the bound carries
/// nothing.
#[test]
fn a_share_on_a_hostile_fork_carries_the_fork_up_to_the_bound() {
    let (mut tree, chain) = Tree::with_best_chain(3);
    let mut fork = Vec::new();
    let mut parent = chain[2].block_hash();
    for _ in 0..=MAX_PATH_HEADERS {
        let block = tree.add(parent, 5);
        parent = block.block_hash();
        fork.push(block);
    }
    assert_eq!(
        tree.path(fork[2].block_hash(), Some(chain[2].block_hash())),
        fork[..3].to_vec()
    );
    assert!(
        tree.path(
            fork[MAX_PATH_HEADERS].block_hash(),
            Some(chain[2].block_hash())
        )
        .is_empty()
    );
}

/// Without the parent's prev, as for a batch's first share whose parent the
/// sender lacks, the walk stops at the first best-chain block.
#[test]
fn without_the_parents_prev_the_walk_stops_on_the_best_chain() {
    let (mut tree, chain) = Tree::with_best_chain(5);
    let orphan = tree.add(chain[3].block_hash(), 9);
    assert_eq!(tree.path(orphan.block_hash(), None), vec![orphan]);
    assert!(tree.path(chain[4].block_hash(), None).is_empty());
}

#[test]
fn a_first_block_or_an_unknown_prev_carries_nothing() {
    let (tree, _) = Tree::with_best_chain(3);
    assert!(tree.path(BlockHash::all_zeros(), None).is_empty());
    assert!(
        tree.path(BlockHash::from_byte_array([0xee; 32]), None)
            .is_empty()
    );
}

#[test]
fn a_message_carries_each_header_once_oldest_first_within_the_cap() {
    let (mut tree, chain) = Tree::with_best_chain(3);
    let orphan = tree.add(chain[1].block_hash(), 9);
    let merged = merge_for_message([vec![chain[2]], vec![orphan, chain[2]], vec![chain[1]]]);
    assert_eq!(merged.len(), 3);
    assert_eq!(merged[0], chain[1].header);
    let heights: Vec<u32> = merged
        .iter()
        .map(|header| tree.headers[&header.block_hash()].height)
        .collect();
    assert_eq!(heights, vec![1, 2, 2]);

    let mut many = Vec::new();
    let mut parent = chain[2].block_hash();
    for _ in 0..MAX_CARRIED_BCH_HEADERS + 5 {
        let block = tree.add(parent, 3);
        parent = block.block_hash();
        many.push(block);
    }
    let merged = merge_for_message([many.clone()]);
    assert_eq!(merged.len(), MAX_CARRIED_BCH_HEADERS);
    assert_eq!(merged[0], many[0].header);
}

/// Real mainnet blocks 661,647 to 661,649 for the receiving side, which
/// runs the full header check.
fn real_chain() -> [KnownBchHeader; 3] {
    [
        KnownBchHeader {
            height: MAINNET_PAIRS[0].0,
            header: header(MAINNET_PAIRS[0].1),
        },
        KnownBchHeader {
            height: MAINNET_PAIRS[1].0,
            header: header(MAINNET_PAIRS[1].1),
        },
        KnownBchHeader {
            height: MAINNET_PAIRS[1].0 + 1,
            header: header(MAINNET_PAIRS[1].2),
        },
    ]
}

fn receive(headers: &[Header], known: &[KnownBchHeader]) -> CarriedOutcome {
    let known: HashMap<BlockHash, KnownBchHeader> = known
        .iter()
        .map(|known| (known.block_hash(), *known))
        .collect();
    accept_carried(
        headers,
        |hash| known.get(hash).copied(),
        &BchHeaderRules::for_network(BitcoinCashNetwork::Mainnet),
        u32::MAX / 2,
    )
}

/// A header links to one known or to one earlier in the same table.
#[test]
fn a_receiver_learns_a_chain_of_headers_linked_through_the_table() {
    let [first, second, third] = real_chain();
    let outcome = receive(&[second.header, third.header], &[first]);
    assert_eq!(outcome.accepted, vec![second, third]);
    assert!(outcome.unlinked.is_empty());
    assert_eq!(outcome.failure, None);
}

#[test]
fn a_receiver_skips_known_headers_and_sets_aside_unlinked_ones() {
    let [first, second, third] = real_chain();
    let outcome = receive(&[second.header], &[first, second]);
    assert_eq!(outcome, CarriedOutcome::default());

    let outcome = receive(&[third.header], &[first]);
    assert!(outcome.accepted.is_empty());
    assert_eq!(outcome.unlinked, vec![third.header]);
}

/// The first header that fails stops the table: nothing after it is
/// learned.
#[test]
fn a_failing_header_stops_the_table() {
    let [first, second, third] = real_chain();
    let mut forged = second.header;
    forged.nonce = forged.nonce.wrapping_add(1);
    let outcome = receive(&[forged, third.header], &[first]);
    assert!(outcome.accepted.is_empty());
    assert!(matches!(
        outcome.failure,
        Some(BchHeaderError::ProofOfWork(_))
    ));
}
