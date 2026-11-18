// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The weak-work credit of the Bitcoin Cash PPLNS window
//! (`docs/sharechain/small-miners-design.md`, part 3, "How weak shares are
//! paid").
//!
//! Every proof of work from a job at the minimum is credited at the weak
//! work of its parent's weak target, `2^256 / (T_weak(parent) + 1)`: a full
//! share at the minimum, an uncle at the minimum (its 9/10, the nephew's
//! 1/10), and each weak share a share carries. A share heavier than the
//! minimum is credited at its own work. A weak share's credit goes, less
//! [`INCLUDER_CUT`], to its own payout address after its own donation and
//! fee; the cut goes to the share carrying it, with that share's weight.
//! For sizing the window a share's work is its credited weight plus its
//! uncles' and its weak shares'.
//!
//! Kept apart from the walk in `pplns_window.rs`, which calls in at three
//! places: building an entry ([`PplnsWindow::credited_weight`],
//! [`PplnsWindow::weak_entries`]), sizing it (`ConfirmedEntry::work`) and
//! crediting it ([`PplnsWindow::credit_weak_entries`]).

use super::{
    ChainStoreHandle, ConfirmedEntry, Credit, DIFFICULTY_SCALE, PplnsWindow, ResolvedWindow,
    ShareCuts, StoreError,
};
use crate::shares::share_block::{ShareHeader, is_terminal_blockhash};
#[cfg(doc)]
use crate::shares::weak::INCLUDER_CUT;
use crate::shares::weak::{WeakSummary, includer_part, is_at_minimum, weak_work};
use bitcoin::BlockHash;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use tracing::debug;

#[cfg(test)]
mod tests;

/// The weak shares one window share carries that pay one payee alike
/// (`shares::weak::WeakCreditGroup`), resolved to the window's keys and to
/// their weak work.
pub(super) struct WeakEntry {
    /// The weak shares' own donation and fee.
    pub(super) cuts: ShareCuts,
    /// Payout address key their credit is paid to.
    pub(super) internal_key: usize,
    /// Share owner key of their miner.
    pub(super) owner_key: usize,
    /// The block they build on, which places their work on a Bitcoin Cash
    /// chain.
    pub(super) bch_prev: BlockHash,
    /// Their payout level (`WeakCreditGroup::payout_level`).
    pub(super) payout_level: u8,
    /// How many weak shares the entry holds.
    pub(super) count: u32,
    /// Their weak work together: `count` times the weak work of their
    /// parent's weak target.
    pub(super) work: u128,
}

impl PplnsWindow {
    /// Whether this window credits proofs from jobs at the minimum at the
    /// weak work: on Bitcoin Cash, set by `for_chain`.
    pub(super) fn credits_weak_work(&self) -> bool {
        self.weak_ceiling.is_some()
    }

    /// The weak work of `header`'s own weak target: what each proof mined on
    /// it as a parent is credited at. `None` on Bitcoin.
    pub(super) fn child_weak_work(&self, header: &ShareHeader) -> Option<u128> {
        self.weak_ceiling
            .filter(|_| header.carries_band())
            .map(|ceiling| weak_work(header, ceiling))
    }

    /// The weak work of each of `headers`' own weak targets, by share, for
    /// the shares among them that are parents of others in the same load.
    /// Empty on Bitcoin.
    pub(super) fn batch_child_weak_works<'a>(
        &self,
        headers: impl Iterator<Item = (&'a BlockHash, &'a ShareHeader)>,
    ) -> HashMap<BlockHash, u128> {
        if !self.credits_weak_work() {
            return HashMap::new();
        }
        headers
            .filter_map(|(blockhash, header)| Some((*blockhash, self.child_weak_work(header)?)))
            .collect()
    }

    /// The parents whose weak work crediting `headers` and the weak shares
    /// in `summaries` needs: the parent of each header at the minimum and of
    /// each weak share. Uncle headers are passed among `headers`.
    pub(super) fn weak_work_parents<'a>(
        &self,
        headers: impl Iterator<Item = &'a ShareHeader>,
        summaries: &HashMap<BlockHash, WeakSummary>,
    ) -> HashSet<BlockHash> {
        let mut parents = HashSet::new();
        if !self.credits_weak_work() {
            return parents;
        }
        parents.extend(
            headers
                .filter(|header| is_at_minimum(header))
                .map(|header| header.prev_share_blockhash)
                .filter(|parent| !is_terminal_blockhash(parent)),
        );
        parents.extend(
            summaries
                .values()
                .flat_map(|summary| summary.groups.iter().map(|group| group.parent)),
        );
        parents
    }

    /// The weak work of each of `parents`: from `batch`, the weak works of
    /// the headers being loaded with them, then from the cached entries, then
    /// from the store. A parent whose header is not stored is a read failure:
    /// a share is stored only once its parent is, and a weak share is valid
    /// only once its parent is.
    pub(super) fn resolve_parent_weak_works(
        &self,
        parents: &HashSet<BlockHash>,
        batch: &HashMap<BlockHash, u128>,
        chain_store_handle: &ChainStoreHandle,
    ) -> Result<HashMap<BlockHash, u128>, Box<dyn Error + Send + Sync>> {
        let mut weak_works = HashMap::with_capacity(parents.len());
        let mut missing = Vec::new();
        for parent in parents {
            match batch
                .get(parent)
                .or_else(|| self.child_weak_works.get(parent))
            {
                Some(work) => {
                    weak_works.insert(*parent, *work);
                }
                None => missing.push(*parent),
            }
        }
        if missing.is_empty() {
            return Ok(weak_works);
        }
        //* A stored parent without a band, a Bitcoin share header, has no
        //* weak target: its children are credited at their own work.
        let mut stored = HashSet::with_capacity(missing.len());
        for (blockhash, header) in chain_store_handle.get_share_headers(&missing)? {
            stored.insert(blockhash);
            if let Some(work) = self.child_weak_work(&header) {
                weak_works.insert(blockhash, work);
            }
        }
        match missing.iter().find(|parent| !stored.contains(*parent)) {
            Some(parent) => Err(StoreError::NotFound(format!(
                "Header of share {parent}, the parent of credited work, is not stored"
            ))
            .into()),
            None => Ok(weak_works),
        }
    }

    /// The weight `header` is credited at before the uncle and weak splits:
    /// the weak work of its parent's weak target when it was mined on a job
    /// at the minimum, else its own work (`base_weight`). The chain's first
    /// share has no parent and is credited at its own work.
    pub(super) fn credited_weight(
        &self,
        header: &ShareHeader,
        parent_weak_works: &HashMap<BlockHash, u128>,
    ) -> u128 {
        if self.credits_weak_work()
            && is_at_minimum(header)
            && let Some(work) = parent_weak_works.get(&header.prev_share_blockhash)
        {
            return *work;
        }
        self.base_weight(header)
    }

    /// The weak entries of a share carrying the weak shares `summary` sums,
    /// one per credit group, with their weak work from `parent_weak_works`.
    pub(super) fn weak_entries(
        &mut self,
        summary: Option<&WeakSummary>,
        parent_weak_works: &HashMap<BlockHash, u128>,
    ) -> Vec<WeakEntry> {
        let Some(summary) = summary else {
            return Vec::new();
        };
        let mut entries = Vec::with_capacity(summary.groups.len());
        for group in &summary.groups {
            let parent_work = parent_weak_works.get(&group.parent).copied().unwrap_or(0);
            let cuts = self.cuts_of(
                &group.donation_address,
                group.donation,
                &group.fee_address,
                group.fee,
            );
            entries.push(WeakEntry {
                cuts,
                internal_key: self.address_keys.key_for(group.payout_address.clone()),
                owner_key: self.owner_keys.key_for(group.share_owner),
                bch_prev: group.bch_prev,
                payout_level: group.payout_level,
                count: group.count,
                work: parent_work.saturating_mul(u128::from(group.count)),
            });
        }
        entries
    }

    /// The weak share summaries of the shares among `headers` that carry
    /// weak shares, read in one batch. Nothing is read when none does, nor
    /// on Bitcoin.
    ///
    /// A confirmed share whose summary is missing was confirmed from its
    /// header alone, far below the candidate tip during sync, where no
    /// window pays (`confirmed_share_transactions`); it is credited without
    /// its weak shares. `require_all` refuses that for a branch share above
    /// the cached chain, whose body a window may pay.
    pub(super) fn weak_summaries<'a>(
        &self,
        headers: impl Iterator<Item = (&'a BlockHash, &'a ShareHeader)>,
        chain_store_handle: &ChainStoreHandle,
        require_all: bool,
    ) -> Result<HashMap<BlockHash, WeakSummary>, Box<dyn Error + Send + Sync>> {
        if !self.credits_weak_work() {
            return Ok(HashMap::new());
        }
        let carriers: Vec<BlockHash> = headers
            .filter(|(_, header)| header.weak_count > 0)
            .map(|(blockhash, _)| *blockhash)
            .collect();
        if carriers.is_empty() {
            return Ok(HashMap::new());
        }
        let summaries = chain_store_handle.get_weak_summaries(&carriers)?;
        match carriers
            .iter()
            .find(|carrier| !summaries.contains_key(*carrier))
        {
            Some(carrier) if require_all => Err(StoreError::NotFound(format!(
                "The weak shares of branch share {carrier} are not stored"
            ))
            .into()),
            Some(carrier) => {
                debug!(
                    "Confirmed share {carrier} has no stored weak shares; it is credited without them here"
                );
                Ok(summaries)
            }
            None => Ok(summaries),
        }
    }

    /// Credit the weak shares `entry` carries: each weak entry the window
    /// pays earns its weak work, less [`INCLUDER_CUT`], for its own payee
    /// after its own cuts. Each credit carries its weak shares' own payout
    /// level and is placed after the share's own credit and its uncles, in
    /// the order its group first appears among the carried weak shares
    /// (`NewestLevel`). Returns the includer's part of the paid weak
    /// entries, which the carrying share adds to its own weight.
    pub(super) fn credit_weak_entries(
        entry: &ConfirmedEntry,
        resolved: &ResolvedWindow,
        credit: &mut impl FnMut(Credit),
    ) -> u128 {
        let mut includer_bonus: u128 = 0;
        for (group_index, weak_entry) in entry.weak_entries.iter().enumerate() {
            if !resolved.pays(&weak_entry.bch_prev) {
                continue;
            }
            let scaled = weak_entry.work.saturating_mul(DIFFICULTY_SCALE);
            let includer = includer_part(scaled);
            credit(Credit {
                cuts: weak_entry.cuts,
                internal_key: weak_entry.internal_key,
                owner_key: weak_entry.owner_key,
                blockhash: entry.blockhash,
                nephew: Some(entry.blockhash),
                difficulty: weak_entry.work,
                weight: scaled - includer,
                weak_shares: weak_entry.count,
                weak_count: 0,
                payout_level: weak_entry.payout_level,
                place_in_share: 1 + entry.uncle_entries.len() + group_index,
            });
            includer_bonus = includer_bonus.saturating_add(includer);
        }
        includer_bonus
    }

    /// The weak work the weak entries of `entry` add to a window's size.
    pub(super) fn weak_entries_work(entry: &ConfirmedEntry) -> u128 {
        entry.weak_entries.iter().fold(0, |work: u128, weak_entry| {
            work.saturating_add(weak_entry.work)
        })
    }
}
