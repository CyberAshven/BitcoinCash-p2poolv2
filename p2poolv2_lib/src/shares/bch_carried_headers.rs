// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Which Bitcoin Cash headers travel with a share, and how a receiver learns
//! them. Part of mining on the current chain (V16,
//! `docs/sharechain/current-chain.md`).
//!
//! A share's **BCH prev** is the block its BCH header builds on. A receiver
//! must know that block's header before it stores the share's body, so a
//! sender attaches the headers a receiver may lack:
//!
//! - **Which.** Walk back from the share's BCH prev, collecting headers,
//!   until the first header on the sender's BCHN best chain that is also the
//!   share parent's BCH prev or below it. A share on the same block as its
//!   parent carries nothing; the first share on a new block carries that
//!   block; a share on a block that lost an orphan race carries the orphan;
//!   after a reorg a share carries the path back to the fork point; a share
//!   on a hostile fork's `k`-th block carries all `k`. Best-chain blocks the
//!   receiver's own BCHN holds too, so they need not travel.
//! - **Bounds.** A share whose path is longer than [`MAX_PATH_HEADERS`]
//!   carries nothing: its prev is too far off any chain to be paid. One
//!   message carries one table of distinct headers, oldest first, at most
//!   [`MAX_CARRIED_BCH_HEADERS`].
//! - **Receiving.** Headers already known are skipped. Each other header
//!   needs its parent, known or earlier in the table, and must pass
//!   `BchHeaderRules::check`. The first failure stops the table; a header
//!   whose parent is unknown is set aside for a later try.
//!
//! The headers are not part of a share's hash: the proof of work commits to
//! the BCH prev, and each header's own hash commits to its parent, so a
//! sender can only leave headers out or send wrong ones, and a wrong one
//! fails its check.

use super::bch_headers::{BchHeaderError, BchHeaderRules, KnownBchHeader};
use bitcoin::BlockHash;
use bitcoin::block::Header;
use bitcoin::hashes::Hash;
use std::collections::{HashMap, HashSet};

/// The longest path back a single share carries. A hostile fork's blocks
/// past the first are never paid, and BCHN refuses a reorg deeper than 10
/// blocks, so 32 covers every honest share.
pub const MAX_PATH_HEADERS: usize = 32;

/// The most BCH headers one message carries.
pub const MAX_CARRIED_BCH_HEADERS: usize = 128;

/// The headers a share building on `prev` carries, oldest first, given its
/// parent's BCH prev (`None` when the sender does not know the parent).
///
/// `header` looks up a known header; `best` gives the hash at a height on
/// the sender's BCHN best chain.
pub fn required_path(
    prev: BlockHash,
    parent_prev: Option<BlockHash>,
    header: impl Fn(&BlockHash) -> Option<KnownBchHeader>,
    best: impl Fn(u32) -> Option<BlockHash>,
) -> Vec<KnownBchHeader> {
    if prev == BlockHash::all_zeros() {
        return Vec::new();
    }
    let parent_prev = parent_prev.and_then(|hash| header(&hash));
    let mut path = Vec::new();
    let mut current = header(&prev);
    while let Some(known) = current {
        let on_best_chain = best(known.height) == Some(known.block_hash());
        let below_parent = parent_prev
            .as_ref()
            .is_none_or(|parent| is_ancestor_or_equal(&known, parent, &header));
        if on_best_chain && below_parent {
            path.reverse();
            return path;
        }
        if path.len() == MAX_PATH_HEADERS {
            return Vec::new();
        }
        path.push(known);
        current = header(&known.header.prev_blockhash);
    }
    // The walk left the known headers without reaching the best chain.
    Vec::new()
}

/// True when `ancestor` is `descendant` or below it on its chain, judged
/// within [`MAX_PATH_HEADERS`] steps.
fn is_ancestor_or_equal(
    ancestor: &KnownBchHeader,
    descendant: &KnownBchHeader,
    header: &impl Fn(&BlockHash) -> Option<KnownBchHeader>,
) -> bool {
    if ancestor.height > descendant.height {
        return false;
    }
    let mut current = *descendant;
    for _ in 0..=MAX_PATH_HEADERS {
        if current.height == ancestor.height {
            return current.block_hash() == ancestor.block_hash();
        }
        match header(&current.header.prev_blockhash) {
            Some(parent) => current = parent,
            None => return false,
        }
    }
    false
}

/// One message's table: the distinct headers of `paths`, oldest first (by
/// height, then hash), at most [`MAX_CARRIED_BCH_HEADERS`]. Past the cap the
/// newest are left out; a later message carries them.
pub fn merge_for_message(paths: impl IntoIterator<Item = Vec<KnownBchHeader>>) -> Vec<Header> {
    let mut seen = HashSet::new();
    let mut headers: Vec<KnownBchHeader> = paths
        .into_iter()
        .flatten()
        .filter(|known| seen.insert(known.block_hash()))
        .collect();
    headers.sort_by_key(|known| (known.height, known.block_hash()));
    headers.truncate(MAX_CARRIED_BCH_HEADERS);
    headers.into_iter().map(|known| known.header).collect()
}

/// What a receiver made of a message's headers.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct CarriedOutcome {
    /// Headers that passed, in table order, to store.
    pub accepted: Vec<KnownBchHeader>,
    /// Headers whose parent is not known yet, to try again later.
    pub unlinked: Vec<Header>,
    /// The check that stopped the table, if one failed.
    pub failure: Option<BchHeaderError>,
}

/// Check a message's headers in order against what is `known`, at `now`.
pub fn accept_carried(
    headers: &[Header],
    known: impl Fn(&BlockHash) -> Option<KnownBchHeader>,
    rules: &BchHeaderRules,
    now: u32,
) -> CarriedOutcome {
    let mut outcome = CarriedOutcome::default();
    let mut learned: HashMap<BlockHash, KnownBchHeader> = HashMap::new();
    for header in headers {
        let hash = header.block_hash();
        if learned.contains_key(&hash) || known(&hash).is_some() {
            continue;
        }
        let parent = learned
            .get(&header.prev_blockhash)
            .copied()
            .or_else(|| known(&header.prev_blockhash));
        let Some(parent) = parent else {
            outcome.unlinked.push(*header);
            continue;
        };
        match rules.check(header, &parent, now) {
            Ok(checked) => {
                learned.insert(hash, checked);
                outcome.accepted.push(checked);
            }
            Err(error) => {
                outcome.failure = Some(error);
                break;
            }
        }
    }
    outcome
}

#[cfg(test)]
mod tests;
