// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::block_tx_metadata::{BlockMetadata, Status};
use super::{ColumnFamily, Store, writer::StoreError};
use crate::shares::chain::chain_store_handle::{COMMON_ANCESTOR_DEPTH, ConfirmedHeaderResult};
use crate::shares::share_block::{ShareBlock, ShareHeader, is_terminal_blockhash};
use crate::shares::validation::MAX_UNCLES;
use bitcoin::consensus::{self, Encodable, encode};
use bitcoin::hashes::Hash;
use bitcoin::{Address, BlockHash, CompactTarget, Work};
use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};
use tracing::{debug, warn};

/// Max depth to look for uncles when building new share blocks
pub const MAX_UNCLES_DEPTH: u8 = 3;

/// Number of blocks above which `find_best_block_valid_descendant` logs that
/// its walk was large. It never stops the walk.
///
/// The walk covers the validated, unconfirmed subtree above the confirmed tip
/// plus one metadata read per non-validated child. That is a handful of blocks
/// in normal operation; a walk past this threshold means confirmation has been
/// lagging for a while (about three hours of shares at 10s each).
const MINING_BASE_SEARCH_WARN_BLOCKS: usize = 1024;

/// Outcome of the mining base search, `find_best_block_valid_descendant`.
pub(crate) enum BlockValidSearch {
    /// A validated descendant of the confirmed tip to mine on.
    Found(BlockHash),
    /// No validated descendant was found. `visited` distinguishes "nothing
    /// above the confirmed tip yet" (zero) from "blocks are there but none of
    /// them is validated", which means confirmation is not keeping up.
    NotFound { visited: usize },
}

/// Maximum number of blocks included per height in getheaders responses.
///
/// In normal operation a height has at most 3-5 blocks. Even during
/// network partitions, each side extends its own chain at different
/// heights. Exceeding this cap indicates either a bug or an attack.
pub const MAX_BLOCKS_PER_HEIGHT: usize = 20;

/// Single confirmed share and its uncles.
#[derive(Clone, Debug, Serialize)]
pub struct ShareInfo {
    pub blockhash: BlockHash,
    pub prev_blockhash: BlockHash,
    pub height: u32,
    /// Payout address, as the header stores it.
    ///
    /// Not rendered to text here: a Bitcoin Cash payout is held under the
    /// legacy network whose base58 version bytes match, and only the API and
    /// CLI layers know to spell it as CashAddr. Serializes as the address
    /// string, as the header does.
    #[serde(with = "crate::shares::address_serde")]
    pub miner_bitcoin_address: Address,
    /// Miner address, as the witness program the header stores.
    ///
    /// Not the bech32m form: rendering that needs a network, and the store is
    /// network agnostic by design. The API and CLI layers hold the configured
    /// network and render it there.
    #[serde(with = "p2poolv2_wallet::share_owner::serde_hex")]
    pub miner_address: p2poolv2_wallet::share_owner::ShareOwner,
    pub timestamp: u32,
    pub bits: CompactTarget,
    pub uncles: Vec<UncleInfo>,
}

/// Uncle share referenced by a confirmed share.
#[derive(Clone, Debug, Serialize)]
pub struct UncleInfo {
    pub blockhash: BlockHash,
    pub prev_blockhash: BlockHash,
    /// Payout address, as the header stores it. See
    /// [`ShareInfo::miner_bitcoin_address`].
    #[serde(with = "crate::shares::address_serde")]
    pub miner_bitcoin_address: Address,
    /// Miner address, as the witness program the header stores. See
    /// [`ShareInfo::miner_address`].
    #[serde(with = "p2poolv2_wallet::share_owner::serde_hex")]
    pub miner_address: p2poolv2_wallet::share_owner::ShareOwner,
    pub timestamp: u32,
    pub height: Option<u32>,
}

/// A window of confirmed share chain data with uncle relationships.
///
/// Contains confirmed headers (newest-to-oldest), the mapping from
/// each nephew to its uncle blockhashes, and the fetched uncle headers
/// keyed by blockhash for random-access lookup.
#[derive(Clone)]
pub struct ShareDag {
    /// Confirmed share headers ordered newest-to-oldest with their heights.
    pub confirmed_headers: Vec<ConfirmedHeaderResult>,
    /// Map from confirmed share blockhash to its referenced uncle blockhashes.
    pub nephew_to_uncles: HashMap<BlockHash, Vec<BlockHash>>,
    /// Uncle headers keyed by blockhash for lookup during weighting.
    pub uncle_headers: HashMap<BlockHash, ShareHeader>,
}

impl ShareDag {
    /// Create an empty ShareDag with no confirmed headers or uncles.
    pub fn empty() -> Self {
        Self {
            confirmed_headers: Vec::new(),
            nephew_to_uncles: HashMap::new(),
            uncle_headers: HashMap::new(),
        }
    }

    /// Remove confirmed headers with timestamps before the given cutoff.
    ///
    /// Retains all headers with time >= earliest_allowed_time regardless
    /// of position, since timestamp monotonicity is not enforced by
    /// validation and out-of-order timestamps are possible.
    pub fn filter_confirmed_by_time(&mut self, earliest_allowed_time: u32) {
        self.confirmed_headers
            .retain(|result| result.header.time >= earliest_allowed_time);
    }

    /// Build uncle references from confirmed headers without hitting the store.
    ///
    /// Extracts all uncle blockhashes and builds the nephew-to-uncles mapping.
    /// Returns (all_uncle_hashes, nephew_to_uncles).
    pub fn collect_uncle_references(
        confirmed_headers: &[ConfirmedHeaderResult],
    ) -> (Vec<BlockHash>, HashMap<BlockHash, Vec<BlockHash>>) {
        let mut seen_uncles: HashSet<BlockHash> = HashSet::new();
        let mut all_uncle_hashes = Vec::with_capacity(confirmed_headers.len());
        let mut nephew_to_uncles: HashMap<BlockHash, Vec<BlockHash>> =
            HashMap::with_capacity(confirmed_headers.len());

        for result in confirmed_headers {
            if !result.header.uncles.is_empty() {
                nephew_to_uncles.insert(result.blockhash, result.header.uncles.clone());
                for uncle_hash in &result.header.uncles {
                    if seen_uncles.insert(*uncle_hash) {
                        all_uncle_hashes.push(*uncle_hash);
                    }
                }
            }
        }

        (all_uncle_hashes, nephew_to_uncles)
    }
}

impl Store {
    /// Update the block index so that we can easily find all the children of a block
    /// We store the next blockhashes for a block in a separate column family
    /// Uses merge operator for atomic append without read-modify-write
    pub(crate) fn update_block_index(
        &self,
        prev_blockhash: &BlockHash,
        next_blockhash: &BlockHash,
        batch: &mut rocksdb::WriteBatch,
    ) -> Result<(), StoreError> {
        debug!(
            "Updating block index {} to {}",
            prev_blockhash, next_blockhash
        );

        let block_index_cf = self.db.cf_handle(&ColumnFamily::BlockIndex).unwrap();
        let mut prev_blockhash_bytes = consensus::serialize(prev_blockhash);
        prev_blockhash_bytes.extend_from_slice(b"_bi");

        // Serialize the single BlockHash to merge
        let mut serialized = Vec::new();
        next_blockhash.consensus_encode(&mut serialized)?;

        // Use merge operator to atomically append
        batch.merge_cf(&block_index_cf, prev_blockhash_bytes, serialized);

        debug!(
            "Block index from parent to child: {} -> {}",
            prev_blockhash, next_blockhash
        );
        Ok(())
    }

    /// Load children BlockHashes for a blockhash from the block index
    /// These are tracked in a separate index in rocksdb as relations from
    /// blockhash -> next blockhashes
    pub fn get_children_blockhashes(
        &self,
        blockhash: &BlockHash,
    ) -> Result<Option<Vec<BlockHash>>, StoreError> {
        let block_index_cf = self.db.cf_handle(&ColumnFamily::BlockIndex).unwrap();
        let mut blockhash_bytes = consensus::serialize(blockhash);
        blockhash_bytes.extend_from_slice(b"_bi");

        match self
            .db
            .get_cf::<&[u8]>(&block_index_cf, blockhash_bytes.as_ref())?
        {
            Some(existing) => {
                let existing_blockhashes = encode::deserialize::<Vec<BlockHash>>(&existing)?;
                Ok(Some(existing_blockhashes))
            }
            None => Ok(None),
        }
    }

    /// Get blockhashes to satisfy the locator query.
    /// Returns a list of blockhashes from the earliest block from the block hashes
    /// We assume the list of blocks in the locator is ordered by height, so we stop when we find the first block in the locator
    /// Find blockhashes up to the stop blockhash, or the limit provided
    pub fn get_blockhashes_for_locator(
        &self,
        locator: &[BlockHash],
        stop_blockhash: &BlockHash,
        limit: usize,
    ) -> Result<Vec<BlockHash>, StoreError> {
        let start_blockhash = self.first_known_for_locator(locator)?;
        // If no blockhash found, return vector with genesis block
        let start_blockhash = match start_blockhash {
            Some(hash) => hash,
            None => match self.get_genesis_blockhash() {
                Some(hash) => return Ok(vec![hash]),
                None => return Ok(vec![]),
            },
        };

        self.get_descendant_blockhashes(&start_blockhash, stop_blockhash, limit)
    }

    /// Find the first locator hash that we know about.
    ///
    /// Uses a single batch metadata lookup and then walks the locator
    /// in order to return the first hash with a valid status
    /// (HeaderValid, Candidate, Confirmed, or BlockValid). Pending
    /// and Invalid blocks are skipped.
    ///
    /// With height-based walking, get_descendant_blockhashes sends all
    /// blocks at each height regardless of status, so matching any
    /// valid block is correct. The locator is ordered newest-first, so
    /// the first match gives the highest known height.
    fn first_known_for_locator(
        &self,
        locator: &[BlockHash],
    ) -> Result<Option<BlockHash>, StoreError> {
        let metadata_results: HashMap<BlockHash, BlockMetadata> = self
            .get_block_metadata_batch(locator)?
            .into_iter()
            .collect();
        for blockhash in locator {
            if let Some(metadata) = metadata_results.get(blockhash)
                && metadata.status != Status::Pending
                && metadata.status != Status::Invalid
            {
                return Ok(Some(*blockhash));
            }
        }
        Ok(None)
    }

    /// Get headers to satisfy the locator query.
    pub fn get_headers_for_locator(
        &self,
        locator: &[BlockHash],
        stop_blockhash: &BlockHash,
        limit: usize,
    ) -> Result<Vec<ShareHeader>, StoreError> {
        let blockhashes = self.get_blockhashes_for_locator(locator, stop_blockhash, limit)?;
        let headers = self.get_share_headers(&blockhashes)?;
        let ordered: Vec<ShareHeader> = headers.into_iter().map(|(_, header)| header).collect();
        Ok(ordered)
    }

    /// Get descendants headers of a share
    /// We stop looking after we have found limit number of descendants or have hit stop blockhash
    pub fn get_descendants(
        &self,
        share: BlockHash,
        stop_blockhash: &BlockHash,
        limit: usize,
    ) -> Result<Vec<ShareHeader>, StoreError> {
        let mut descendants = Vec::with_capacity(limit);

        let mut next_children = vec![];
        let mut current_blockhash = share;
        while descendants.len() < limit && current_blockhash != *stop_blockhash {
            if let Ok(Some(children)) = self.get_children_blockhashes(&current_blockhash) {
                for child in children {
                    if descendants.len() < limit {
                        descendants.push(child);
                        next_children.push(child);
                    }
                }
            }
            current_blockhash = match next_children.pop() {
                Some(hash) => hash,
                None => break,
            };
        }
        let headers = self.get_share_headers(&descendants)?;
        let ordered: Vec<ShareHeader> = headers.into_iter().map(|(_, header)| header).collect();
        Ok(ordered)
    }

    /// Get the parent of a share as a ShareBlock
    pub fn get_parent(&self, blockhash: &BlockHash) -> Option<ShareBlock> {
        let share = self.get_share(blockhash)?;
        let parent_blockhash = share.header.prev_share_blockhash;
        self.get_share(&parent_blockhash)
    }

    /// Get the uncles of a share as pointed to by the ShareHeader
    /// Returns an error if an uncle hash is not found in the store
    ///
    /// This is not used to find uncles from the chain that should be
    /// included in the ShareHeader. For that look at
    /// ChainStore::find_uncles
    pub fn get_uncles(&self, blockhash: &BlockHash) -> Result<Vec<ShareBlock>, StoreError> {
        let share = self.get_share(blockhash);
        if share.is_none() {
            return Ok(vec![]);
        }
        let share = share.unwrap();
        let uncle_blocks = self.get_shares(&share.header.uncles)?;
        Ok(uncle_blocks.into_values().collect())
    }

    /// Find the shares from the given share up to depth from that share
    ///
    /// Returns a chain of blockhashes starting from start and going
    /// backward up to depth main chain ancestors (newest to oldest).
    /// Includes all uncles referenced by those main chain blocks.
    /// Uncles do not count toward the depth limit.
    pub(crate) fn get_dag_for_depth(
        &self,
        start: &BlockHash,
        depth: usize,
    ) -> Result<Vec<BlockHash>, StoreError> {
        let mut to_visit: VecDeque<(BlockHash, bool)> = VecDeque::with_capacity(depth);
        to_visit.push_back((*start, true)); // (blockhash, is_main_chain)

        let mut results = Vec::with_capacity(depth);
        let mut visited = HashSet::new();
        let mut remaining_depth = depth;

        // Walk backward through parents and uncles
        while let Some((next, is_main_chain)) = to_visit.pop_front() {
            // Skip already visited blocks to avoid duplicates
            if visited.contains(&next) {
                continue;
            }

            // Get the share to find its parent
            let Ok(Some(next_share_header)) = self.get_share_header(&next) else {
                // Can't find share, stop here
                break;
            };

            visited.insert(next);
            results.push(next_share_header.block_hash());

            if is_main_chain {
                remaining_depth -= 1;

                // Only continue main chain if depth not exhausted
                if remaining_depth > 0 {
                    to_visit.push_back((next_share_header.prev_share_blockhash, true));
                }

                // Always include uncles of main chain blocks we've processed
                for uncle in next_share_header.uncles.iter() {
                    to_visit.push_back((*uncle, false));
                }
            }
        }

        Ok(results)
    }

    /// Get common ancestor of two blockhashes
    /// We first find chain from each blockhashes provided and then find the common ancestor
    ///
    /// If one of the blockhashes is an ancestor of the other, it is
    /// returned as the common ancestor
    pub fn get_common_ancestor(
        &self,
        blockhash1: &BlockHash,
        blockhash2: &BlockHash,
    ) -> Result<Option<BlockHash>, StoreError> {
        debug!("Looking for common ancestor between {blockhash1} and {blockhash2}");
        // Get chains up to COMMON_ANCESTOR_DEPTH (ordered from newest to oldest)
        let chain1 = self.get_dag_for_depth(blockhash1, COMMON_ANCESTOR_DEPTH)?;
        let chain2 = self.get_dag_for_depth(blockhash2, COMMON_ANCESTOR_DEPTH)?;

        // Build a set from chain1 for O(1) lookup
        let chain1_set: HashSet<BlockHash> = chain1.into_iter().collect();

        // Find first common blockhash by iterating chain2
        // chain2 is ordered from newest to oldest, so first match is the most recent common ancestor
        for blockhash in chain2 {
            if chain1_set.contains(&blockhash) {
                return Ok(Some(blockhash));
            }
        }

        Ok(None)
    }

    /// Add a blockhash to the uncles index, marking it as used as an uncle
    /// by the given nephew blockhash.
    ///
    /// Uses merge operator to support multiple nephews including the same uncle.
    pub(crate) fn add_to_uncles_index(
        &self,
        uncle: &BlockHash,
        nephew: &BlockHash,
        batch: &mut rocksdb::WriteBatch,
    ) -> Result<(), StoreError> {
        let uncles_cf = self.db.cf_handle(&ColumnFamily::Uncles).unwrap();
        let mut serialized_nephew = Vec::new();
        nephew.consensus_encode(&mut serialized_nephew)?;
        batch.merge_cf(&uncles_cf, AsRef::<[u8]>::as_ref(uncle), serialized_nephew);
        Ok(())
    }

    /// Get the nephews that have included a blockhash as an uncle
    pub fn get_nephews(&self, uncle: &BlockHash) -> Option<Vec<BlockHash>> {
        let uncles_cf = self.db.cf_handle(&ColumnFamily::Uncles).unwrap();
        match self.db.get_cf::<&[u8]>(&uncles_cf, uncle.as_ref()) {
            Ok(Some(bytes)) => encode::deserialize(&bytes).ok(),
            Ok(None) | Err(_) => None,
        }
    }

    /// Finds uncles for a share built on `base_hash`, sorted by
    /// chain_work descending.
    ///
    /// Algorithm: walk the base's ancestry up to `MAX_UNCLES_DEPTH` parent
    /// pointers, collecting the uncles listed by the base and its ancestors.
    /// The children of those ancestors that are not themselves on the base's
    /// ancestry, not on the confirmed chain, not already listed as an uncle on
    /// this ancestry, and have their block body stored are the uncle
    /// candidates. Return the top `MAX_UNCLES` by chain_work.
    ///
    /// The listed-uncle exclusion matches the validation rule that an uncle
    /// appears at most once per chain. It looks only at the base's ancestry,
    /// so an uncle listed by a nephew on a losing branch is still available.
    /// Ported from pool2win's upstream P2Poolv2 commit d14d8a00
    /// (MIT OR Apache-2.0).
    ///
    /// Anchoring on `base_hash` (the nephew's parent) rather than the
    /// confirmed tip keeps every returned uncle within `MAX_UNCLES_DEPTH` of
    /// the nephew even when the base leads the confirmed chain on a
    /// `BlockValid` fork -- so the produced share passes the uncle-depth
    /// check that validation applies. The body check prevents selecting
    /// header-only blocks (received via header sync without a body), which
    /// would create shares that reference uncle data no node can serve.
    ///
    /// Selected uncles must also be at least header-validated, which skips
    /// `Invalid` and `Pending` blocks.
    pub fn find_uncles(&self, base_hash: &BlockHash) -> Result<Vec<BlockHash>, StoreError> {
        let mut ancestors: Vec<BlockHash> = Vec::with_capacity(MAX_UNCLES_DEPTH as usize);
        let mut ancestry: HashSet<BlockHash> =
            HashSet::with_capacity(MAX_UNCLES_DEPTH as usize + 1);
        let mut uncles_listed_on_ancestry: HashSet<BlockHash> =
            HashSet::with_capacity(MAX_UNCLES_DEPTH as usize * MAX_UNCLES);
        ancestry.insert(*base_hash);
        let mut current = *base_hash;
        let mut steps = 0;
        while steps < MAX_UNCLES_DEPTH as usize && !is_terminal_blockhash(&current) {
            let parent = match self.get_share_header(&current)? {
                Some(header) => {
                    uncles_listed_on_ancestry.extend(header.uncles.iter().copied());
                    header.prev_share_blockhash
                }
                None => return Ok(Vec::new()),
            };
            if !is_terminal_blockhash(&parent) {
                ancestors.push(parent);
                ancestry.insert(parent);
            }
            current = parent;
            steps += 1;
        }

        let children = ancestors.iter().flat_map(|ancestor| {
            self.get_children_blockhashes(ancestor)
                .ok()
                .flatten()
                .into_iter()
                .flatten()
                .map(move |child| (*ancestor, child))
        });

        let uncles_with_work: HashSet<(BlockHash, Work)> = children
            .filter(|(_, blockhash)| {
                !ancestry.contains(blockhash)
                    && !self.is_confirmed(blockhash)
                    && !uncles_listed_on_ancestry.contains(blockhash)
                    && self.share_block_exists(blockhash)
            })
            .filter(|(ancestor, blockhash)| self.is_child_of(blockhash, ancestor))
            .filter_map(|(_, blockhash)| {
                self.get_block_metadata(&blockhash)
                    .ok()
                    .filter(|metadata| {
                        matches!(metadata.status, Status::HeaderValid | Status::BlockValid)
                    })
                    .map(|metadata| (blockhash, metadata.chain_work))
            })
            .collect();

        // Sort by chain_work descending then blockhash ascending and take top MAX_UNCLES
        let mut uncles_with_work: Vec<(BlockHash, Work)> = uncles_with_work.into_iter().collect();
        uncles_with_work.sort_by_key(|(blockhash, work)| (std::cmp::Reverse(*work), *blockhash));

        let uncles = uncles_with_work
            .into_iter()
            .take(MAX_UNCLES)
            .map(|(hash, _)| hash)
            .collect();

        Ok(uncles)
    }

    /// Find the highest-work `BlockValid` descendant of `from`.
    ///
    /// This is the mining base search. It walks forward from the confirmed tip
    /// over the descendant subtree, so it finds blocks we have fully validated
    /// even when they sit off the candidate chain -- which is the normal state
    /// when the candidate chain has reorged onto a peer's header-only fork and
    /// our own mined blocks are the only validated ones above the confirmed
    /// tip. Reading the answer from the DAG on demand, rather than caching a
    /// pointer, means there is no invariant to maintain across invalidation,
    /// reorg and restart.
    ///
    /// Only `BlockValid` blocks are walked into. Validation is parent-gated --
    /// a block is validated only once its parent is `BlockValid` or confirmed
    /// -- so no block below a non-`BlockValid` one is validated, and skipping
    /// its subtree loses no answer. In particular an `Invalid` block hides its
    /// whole subtree, and header-only blocks, which header sync stores a full
    /// response at a time, cost the search one metadata read each.
    ///
    /// The walk is not bounded. What remains is the validated, unconfirmed
    /// subtree, which grows only at real hashrate and only while confirmation
    /// lags. A bound would end the walk at the same shallow block on every job
    /// refresh, pinning every share onto one parent as a sibling that adds no
    /// work. A walk larger than `MINING_BASE_SEARCH_WARN_BLOCKS` is logged.
    ///
    /// Ranking is by cumulative work, ties broken by the lexicographically
    /// smallest hash, so every node picks the same base. A `BlockValid` block
    /// with a `BlockValid` child always loses to that child on work, so the
    /// winner is a tip of the validated subtree without needing a separate
    /// check for it.
    pub(crate) fn find_best_block_valid_descendant(
        &self,
        from: &BlockHash,
    ) -> Result<BlockValidSearch, StoreError> {
        let mut queue: VecDeque<BlockHash> = self.children_blockhashes(from).into();
        let mut visited = 0;
        let mut best: Option<(BlockHash, Work)> = None;

        while let Some(blockhash) = queue.pop_front() {
            visited += 1;

            // A block whose metadata cannot be read is skipped like any
            // non-BlockValid block, which also drops its subtree. That is not
            // propagated: this search is best-effort with a documented
            // fallback to the confirmed tip, and `children_blockhashes` already
            // treats read errors as no children, whereas an error here would
            // travel through `get_mining_base` into the notify build and stop
            // the node. It is logged so the pruning is not silent.
            let metadata = match self.get_block_metadata(&blockhash) {
                Ok(metadata) => Some(metadata),
                Err(StoreError::NotFound(_)) => None,
                Err(error) => {
                    warn!(
                        "Skipping {blockhash} and its subtree in the mining base search: {error}"
                    );
                    None
                }
            };
            if let Some(metadata) =
                metadata.filter(|metadata| metadata.status == Status::BlockValid)
            {
                let outranks_best = match best {
                    Some((best_hash, best_work)) => {
                        metadata.chain_work > best_work
                            || (metadata.chain_work == best_work && blockhash < best_hash)
                    }
                    None => true,
                };
                if outranks_best {
                    best = Some((blockhash, metadata.chain_work));
                }
                queue.extend(self.children_blockhashes(&blockhash));
            }
        }

        if visited > MINING_BASE_SEARCH_WARN_BLOCKS {
            warn!(
                "Mining base search visited {visited} blocks above {from}: confirmation is lagging"
            );
        }

        match best {
            Some((blockhash, _)) => Ok(BlockValidSearch::Found(blockhash)),
            None => Ok(BlockValidSearch::NotFound { visited }),
        }
    }

    /// Children that actually name `blockhash` as their parent.
    ///
    /// `BlockIndex` also holds uncle -> nephew edges under the same key, so
    /// the raw entry cannot be walked as a descendant list (see
    /// `find_uncles`). Read errors yield no children.
    fn children_blockhashes(&self, blockhash: &BlockHash) -> Vec<BlockHash> {
        self.get_children_blockhashes(blockhash)
            .ok()
            .flatten()
            .into_iter()
            .flatten()
            .filter(|child| self.is_child_of(child, blockhash))
            .collect()
    }

    /// Whether `blockhash` names `parent_hash` as its parent.
    ///
    /// Distinguishes a real parent -> child edge from the uncle -> nephew edge
    /// stored under the same `BlockIndex` key. A block whose header cannot be
    /// read is not treated as a child.
    fn is_child_of(&self, blockhash: &BlockHash, parent_hash: &BlockHash) -> bool {
        self.get_share_header(blockhash)
            .ok()
            .flatten()
            .is_some_and(|header| header.prev_share_blockhash == *parent_hash)
    }

    /// Batch fetches uncle headers and metadata using multi_get_cf.
    ///
    /// Silently skips any uncle whose header is not found in the store.
    pub fn get_uncle_infos(&self, uncle_hashes: &[BlockHash]) -> Vec<UncleInfo> {
        if uncle_hashes.is_empty() {
            return Vec::new();
        }

        let headers = self.get_share_headers(uncle_hashes).unwrap_or_default();
        // Uncle info only decorates a monitoring event, so a store failure
        // costs display detail rather than correctness -- the same trade the
        // header read above already makes. Logged so it is not silent.
        let metadata_pairs = self
            .get_block_metadata_batch(uncle_hashes)
            .unwrap_or_else(|error| {
                warn!("Could not read uncle metadata for monitoring: {error}");
                Vec::new()
            });
        let metadata_map: HashMap<BlockHash, _> = metadata_pairs
            .into_iter()
            .filter_map(|(hash, metadata)| Some((hash, metadata.expected_height?)))
            .collect();

        headers
            .into_iter()
            .map(|(blockhash, header)| UncleInfo {
                blockhash,
                prev_blockhash: header.prev_share_blockhash,
                miner_bitcoin_address: header.miner_bitcoin_address,
                miner_address: header.miner_address,
                timestamp: header.time,
                height: metadata_map.get(&blockhash).copied(),
            })
            .collect()
    }

    /// Batch fetches share headers for a chain, returning a map keyed by blockhash.
    ///
    /// Returns an error if any blockhash in the chain has no matching header.
    fn fetch_header_map(
        &self,
        chain: &[(u32, BlockHash)],
    ) -> Result<HashMap<BlockHash, ShareHeader>, StoreError> {
        let blockhashes: Vec<BlockHash> = chain.iter().map(|(_, hash)| *hash).collect();
        let found_headers = self.get_share_headers(&blockhashes)?;

        if found_headers.len() != chain.len() {
            return Err(StoreError::NotFound("Some share headers not found".into()));
        }

        Ok(HashMap::from_iter(found_headers))
    }

    /// Builds ShareInfo entries from a chain and its header map.
    ///
    /// Collects all uncle hashes across headers, batch resolves them,
    /// and returns entries ordered from highest height to lowest.
    fn assemble_share_infos(
        &self,
        chain: &[(u32, BlockHash)],
        header_map: &HashMap<BlockHash, ShareHeader>,
    ) -> Vec<ShareInfo> {
        let all_uncle_hashes: Vec<BlockHash> = header_map
            .values()
            .flat_map(|header| header.uncles.iter())
            .copied()
            .collect();
        let mut uncle_map: HashMap<BlockHash, UncleInfo> = self
            .get_uncle_infos(&all_uncle_hashes)
            .into_iter()
            .map(|info| (info.blockhash, info))
            .collect();

        let mut shares = Vec::with_capacity(chain.len());
        for (height, blockhash) in chain.iter().rev() {
            let header = header_map.get(blockhash).unwrap();
            let uncles: Vec<UncleInfo> = header
                .uncles
                .iter()
                .filter_map(|uncle_hash| uncle_map.remove(uncle_hash))
                .collect();

            shares.push(ShareInfo {
                blockhash: *blockhash,
                prev_blockhash: header.prev_share_blockhash,
                height: *height,
                miner_bitcoin_address: header.miner_bitcoin_address.clone(),
                miner_address: header.miner_address,
                timestamp: header.time,
                bits: header.bits,
                uncles,
            });
        }

        shares
    }

    /// Batch fetches confirmed share headers and returns ShareInfo entries
    /// ordered from highest height to lowest.
    pub fn query_shares(
        &self,
        from_height: u32,
        to_height: u32,
    ) -> Result<Vec<ShareInfo>, StoreError> {
        let confirmed_chain = self.get_confirmed(from_height, to_height)?;
        let header_map = self.fetch_header_map(&confirmed_chain)?;
        Ok(self.assemble_share_infos(&confirmed_chain, &header_map))
    }

    /// Batch fetches confirmed share headers ordered from lowest height to highest.
    pub fn query_share_headers(
        &self,
        from_height: u32,
        to_height: u32,
    ) -> Result<Vec<ShareHeader>, StoreError> {
        let confirmed_chain = self.get_confirmed(from_height, to_height)?;
        let blockhashes: Vec<BlockHash> = confirmed_chain.iter().map(|(_, hash)| *hash).collect();
        let found_headers = self.get_share_headers(&blockhashes)?;

        if found_headers.len() != confirmed_chain.len() {
            return Err(StoreError::NotFound("Some share headers not found".into()));
        }

        Ok(found_headers
            .into_iter()
            .map(|(_, header)| header)
            .collect())
    }

    /// Batch fetches confirmed share blocks ordered from lowest height to highest.
    pub fn query_share_blocks(
        &self,
        from_height: u32,
        to_height: u32,
    ) -> Result<Vec<ShareBlock>, StoreError> {
        let confirmed_chain = self.get_confirmed(from_height, to_height)?;
        let blockhashes: Vec<BlockHash> = confirmed_chain.iter().map(|(_, hash)| *hash).collect();
        let mut found_shares = self.get_shares(&blockhashes)?;

        let share_blocks: Vec<ShareBlock> = confirmed_chain
            .iter()
            .filter_map(|(_, hash)| found_shares.remove(hash))
            .collect();

        if share_blocks.len() != confirmed_chain.len() {
            return Err(StoreError::NotFound("Some share blocks not found".into()));
        }

        Ok(share_blocks)
    }

    /// Batch fetches candidate share headers and returns ShareInfo entries
    /// ordered from highest height to lowest.
    ///
    /// Pruning deletes candidate index entries below the prune target, so
    /// heights in that range are omitted.
    pub fn query_candidates(
        &self,
        from_height: u32,
        to_height: u32,
    ) -> Result<Vec<ShareInfo>, StoreError> {
        let candidate_chain = self.get_candidates(from_height, to_height)?;
        let header_map = self.fetch_header_map(&candidate_chain)?;
        Ok(self.assemble_share_infos(&candidate_chain, &header_map))
    }

    /// Query ALL share headers in the height index for a range of heights.
    ///
    /// Unlike query_shares/query_candidates which only return confirmed or
    /// candidate chain entries, this returns every block at each height
    /// regardless of status (Confirmed, Candidate, HeaderValid, etc.).
    pub fn query_dag(&self, from_height: u32, to_height: u32) -> Vec<DagEntry> {
        // Callers pass the range, so cap the preallocation and let the Vec grow.
        let estimated_capacity = (to_height.saturating_sub(from_height) as usize)
            .saturating_add(1)
            .saturating_mul(2)
            .min(1024);
        let mut entries = Vec::with_capacity(estimated_capacity);

        let mut height = from_height;
        while height <= to_height {
            let blockhashes = self.get_blockhashes_for_height(height);
            for blockhash in &blockhashes {
                let (status, chain) = self
                    .get_block_metadata(blockhash)
                    .map(|metadata| (metadata.status.to_string(), metadata.chain.to_string()))
                    .unwrap_or_else(|_| ("Unknown".to_string(), "Unknown".to_string()));

                let (parent, uncles, miner_bitcoin_address, miner_address) =
                    match self.get_share_header(blockhash) {
                        Ok(Some(header)) => (
                            header.prev_share_blockhash,
                            header.uncles.clone(),
                            Some(header.miner_bitcoin_address),
                            Some(header.miner_address),
                        ),
                        _ => (BlockHash::all_zeros(), vec![], None, None),
                    };

                let has_block_data = self.share_block_exists(blockhash);

                entries.push(DagEntry {
                    blockhash: *blockhash,
                    height,
                    validation_status: status,
                    chain,
                    parent,
                    uncles,
                    miner_bitcoin_address,
                    miner_address,
                    has_block_data,
                });
            }
            height += 1;
        }

        entries
    }
}

/// Entry representing a single share header at a height in the DAG.
///
/// Includes all blocks at that height regardless of chain status,
/// useful for debugging fork structure and missing block data.
#[derive(Clone, Debug, Serialize)]
pub struct DagEntry {
    pub blockhash: BlockHash,
    pub height: u32,
    /// Validation state only. Chain position is reported separately in `chain`.
    pub validation_status: String,
    pub chain: String,
    pub parent: BlockHash,
    pub uncles: Vec<BlockHash>,
    /// Payout address, or `None` when the header could not be read. See
    /// [`ShareInfo::miner_bitcoin_address`].
    #[serde(with = "crate::shares::option_address_serde")]
    pub miner_bitcoin_address: Option<Address>,
    /// Miner address, or `None` when the header could not be read.
    ///
    /// The witness program rather than the bech32m form: the store is network
    /// agnostic, so the API and CLI layers render it.
    #[serde(with = "p2poolv2_wallet::share_owner::serde_hex_option")]
    pub miner_address: Option<p2poolv2_wallet::share_owner::ShareOwner>,
    pub has_block_data: bool,
}

#[cfg(test)]
mod tests;
