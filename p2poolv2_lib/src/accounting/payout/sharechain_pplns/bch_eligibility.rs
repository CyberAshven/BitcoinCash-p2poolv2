// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Mining on the current chain: which window shares a Bitcoin Cash block
//! pays.
//!
//! A block pays a window share only for work on its own BCH chain, give or
//! take one block. The paying block's chain is the one ending at its BCH
//! prev, the block its header builds on. A share is paid when its own BCH
//! prev is on that chain, or is a child of a block on it: a block that lost
//! an orphan race, or the block after the paying block's prev when the paying
//! share's template lagged. A share on a fork two or more blocks off the chain
//! earns nothing, though it keeps its place and its work in the window, so
//! the window's size does not depend on which shares are paid (V16,
//! `docs/sharechain/current-chain.md`).
//!
//! Shares are placed with the BCH headers a node knows. A share's body is
//! stored only once its BCH prev is known, and the headers a share needs
//! travel with it (`shares::bch_carried_headers`), so every node that
//! validated a window knows the same places and pays the same shares.

use bitcoin::BlockHash;

/// Where a known BCH block sits: its height and its parent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct BchPlace {
    pub(super) height: u32,
    pub(super) parent: BlockHash,
}

/// The BCH chain a paying block builds on, from its BCH prev down to the
/// lowest height a window needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PayingChain {
    /// Height of the paying block's BCH prev, `hashes[0]`.
    top_height: u32,
    /// The lowest height the walk was asked to reach. The chain stops above
    /// it only at a block whose parent is not known.
    walked_to: u32,
    /// One block per height, from the top down.
    hashes: Vec<BlockHash>,
}

impl PayingChain {
    /// Walk down from `top`, the paying block's BCH prev at `top_place`, to
    /// `low_height`. `place` gives a known block's place, or `None` for a
    /// block not known: the walk then ends at that block, which a known
    /// child names, so the chain still holds its hash. The walk also ends at
    /// the chain's first block.
    pub(super) fn walk<E>(
        top: BlockHash,
        top_place: BchPlace,
        low_height: u32,
        mut place: impl FnMut(&BlockHash) -> Result<Option<BchPlace>, E>,
    ) -> Result<Self, E> {
        let mut hashes = vec![top];
        let mut current = top_place;
        while current.height > low_height && current.height > 0 {
            hashes.push(current.parent);
            match place(&current.parent)? {
                Some(parent) => current = parent,
                None => break,
            }
        }
        Ok(Self {
            top_height: top_place.height,
            walked_to: low_height,
            hashes,
        })
    }

    /// Whether this chain, walked down to `low_height` or lower, is the one
    /// a block on `top` pays from.
    pub(super) fn serves(&self, top: &BlockHash, low_height: u32) -> bool {
        self.hashes[0] == *top && self.walked_to <= low_height
    }

    /// The block on this chain at `height`, if the chain reaches it.
    fn hash_at(&self, height: u32) -> Option<&BlockHash> {
        let depth = self.top_height.checked_sub(height)?;
        self.hashes.get(usize::try_from(depth).ok()?)
    }

    /// Whether this chain pays a share building on `prev`, which sits at
    /// `place`: `prev` is on the chain, or its parent is.
    pub(super) fn pays(&self, prev: &BlockHash, place: &BchPlace) -> bool {
        self.hash_at(place.height) == Some(prev)
            || place
                .height
                .checked_sub(1)
                .and_then(|height| self.hash_at(height))
                == Some(&place.parent)
    }

    /// The blocks on this chain, top first.
    pub(super) fn hashes(&self) -> &[BlockHash] {
        &self.hashes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::hashes::Hash;
    use std::collections::HashMap;

    fn hash(tag: u8) -> BlockHash {
        BlockHash::from_byte_array([tag; 32])
    }

    /// The paying chain 1 <- 2 <- 3 <- 4, heights 0 to 3, paid from 4, with
    /// blocks off it: 12 a sibling of 3, 13 a child of 12, 15 a child of 4
    /// and 16 a child of 15.
    fn tree() -> HashMap<BlockHash, BchPlace> {
        let place = |height, parent| BchPlace {
            height,
            parent: hash(parent),
        };
        HashMap::from([
            (hash(1), place(0, 0)),
            (hash(2), place(1, 1)),
            (hash(3), place(2, 2)),
            (hash(4), place(3, 3)),
            (hash(12), place(2, 2)),
            (hash(13), place(3, 12)),
            (hash(15), place(4, 4)),
            (hash(16), place(5, 15)),
        ])
    }

    fn walk(tree: &HashMap<BlockHash, BchPlace>, top: u8, low_height: u32) -> PayingChain {
        PayingChain::walk(hash(top), tree[&hash(top)], low_height, |block| {
            Ok::<_, ()>(tree.get(block).copied())
        })
        .unwrap()
    }

    #[test]
    fn a_block_pays_its_chain_and_one_block_off_it() {
        let tree = tree();
        let chain = walk(&tree, 4, 0);
        let pays = |tag| chain.pays(&hash(tag), &tree[&hash(tag)]);

        assert!(pays(1) && pays(2) && pays(3) && pays(4), "on the chain");
        assert!(pays(12), "a sibling of a block on the chain");
        assert!(pays(15), "the block after the paying prev");
        assert!(!pays(13), "two blocks off the chain");
        assert!(!pays(16), "two blocks past the paying prev");
    }

    #[test]
    fn the_walk_stops_at_the_lowest_height_asked_for() {
        let tree = tree();
        let chain = walk(&tree, 4, 2);

        assert_eq!(chain.hashes(), [hash(4), hash(3)]);
        assert!(chain.serves(&hash(4), 2));
        assert!(chain.serves(&hash(4), 3));
        assert!(!chain.serves(&hash(4), 1));
        assert!(!chain.serves(&hash(3), 2));
    }

    /// A chain whose oldest known block's parent is unknown still names that
    /// parent, as its known child does, and ends there.
    #[test]
    fn the_walk_ends_at_the_oldest_known_block() {
        let mut tree = tree();
        tree.remove(&hash(2));
        let chain = walk(&tree, 4, 0);

        assert_eq!(chain.hashes(), [hash(4), hash(3), hash(2)]);
        assert!(chain.serves(&hash(4), 0));
        assert!(chain.pays(&hash(3), &tree[&hash(3)]));
        assert!(chain.pays(&hash(12), &tree[&hash(12)]));
    }

    #[test]
    fn the_walk_ends_at_the_first_block() {
        let tree = tree();
        let chain = walk(&tree, 2, 0);

        assert_eq!(chain.hashes(), [hash(2), hash(1)]);
    }

    #[test]
    fn a_failed_read_fails_the_walk() {
        let tree = tree();
        let failed = PayingChain::walk(hash(4), tree[&hash(4)], 0, |_| Err("read failed"));

        assert_eq!(failed, Err("read failed"));
    }
}
