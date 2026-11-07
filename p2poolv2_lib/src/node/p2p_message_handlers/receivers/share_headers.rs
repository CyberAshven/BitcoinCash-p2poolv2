// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::accounting::payout::sharechain_pplns::pplns_window::{
    BCH_COINBASE_MATURITY, PPLNS_WINDOW_SHARES, declared_share_difficulty,
};
use crate::node::messages::ShareHeaderBatch;
use crate::node::p2p_message_handlers::MAX_HEADERS_IN_RESPONSE;
use crate::node::p2p_message_handlers::receivers::bch_headers::learn_carried_bch_headers;
use crate::node::p2p_message_handlers::senders::send_getheaders;
use crate::node::request_response_handler::block_fetcher::{BlockFetcherEvent, BlockFetcherHandle};
use crate::node::{SwarmSend, messages::Message};
#[cfg(test)]
#[mockall_double::double]
use crate::pool_difficulty::PoolDifficulty;
#[cfg(not(test))]
use crate::pool_difficulty::PoolDifficulty;
use crate::pool_difficulty::{AsertParent, target_to_difficulty_rounded_up};
#[cfg(test)]
#[mockall_double::double]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
#[cfg(not(test))]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
use crate::shares::share_block::{
    MIN_CUMULATIVE_CHAIN_WORK_MULTIPLIER, ShareHeader, is_terminal_blockhash,
};
use crate::shares::validation::sharechain_floor::{
    FloorCredit, SHARECHAIN_FLOOR_PRESENCE_MINIMUM, derive_sharechain_floor,
};
use crate::shares::validation::{
    MTP_WINDOW, ShareValidator, ValidationError, check_sharechain_band,
};
use crate::store::block_tx_metadata::BlockMetadata;
use crate::store::dag_store::MAX_BLOCKS_PER_HEIGHT;
use crate::store::writer::StoreError;
use bitcoin::hashes::Hash;
use bitcoin::{BlockHash, CompactTarget, Target, Work};
use p2poolv2_wallet::payout_address::PayoutAddress;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;
use std::iter;
use tokio::sync::mpsc;
use tracing::{debug, error, warn};

/// Depth to go back when retrying getheaders after missing parents/uncles.
const DEEP_LOCATOR_DEPTH: u32 = 100;

/// Errors from share header batch validation. Retryable variants
/// indicate missing data that a deeper getheaders may resolve.
/// Non-retryable variants indicate invalid data from the peer.
/// Retryable variants carry the lowest anchor height so the retry
/// locator can start from that point minus a margin.
#[derive(Debug)]
enum HeaderSyncError {
    MissingExternalParents {
        missing: Vec<BlockHash>,
        lowest_anchor_height: u32,
    },
    MissingUncle {
        uncle_hash: BlockHash,
        lowest_anchor_height: u32,
    },
    MissingParentInBatch {
        header: BlockHash,
        parent: BlockHash,
        lowest_anchor_height: u32,
    },
    AsertMismatch {
        block_hash: BlockHash,
        declared: u32,
        expected: u32,
    },
    /// A header's declared target fell outside the validation band
    /// `[effective floor, infinity)` in its compact bits.
    BelowFloor {
        block_hash: BlockHash,
        declared: u32,
        floor: u64,
    },
    /// A header's timestamp breaks median time past or the future limit.
    InvalidTimestamp {
        block_hash: BlockHash,
        error: ValidationError,
    },
    InsufficientWork,
    DenseHeight {
        height: u32,
        count: usize,
    },
    Other(Box<dyn Error + Send + Sync>),
}

impl HeaderSyncError {
    fn is_retryable(&self) -> bool {
        matches!(
            self,
            HeaderSyncError::MissingExternalParents { .. }
                | HeaderSyncError::MissingUncle { .. }
                | HeaderSyncError::MissingParentInBatch { .. }
        )
    }

    /// Returns the retry depth if this error is retryable.
    /// The depth is calculated so the locator starts 100 blocks
    /// before the lowest anchor height from the failed batch.
    fn retry_depth(&self, tip_height: u32) -> Option<u32> {
        let lowest_anchor_height = match self {
            HeaderSyncError::MissingExternalParents {
                lowest_anchor_height,
                ..
            } => *lowest_anchor_height,
            HeaderSyncError::MissingUncle {
                lowest_anchor_height,
                ..
            } => *lowest_anchor_height,
            HeaderSyncError::MissingParentInBatch {
                lowest_anchor_height,
                ..
            } => *lowest_anchor_height,
            _ => return None,
        };
        let target_height = lowest_anchor_height.saturating_sub(DEEP_LOCATOR_DEPTH);
        Some(tip_height.saturating_sub(target_height))
    }
}

impl fmt::Display for HeaderSyncError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HeaderSyncError::MissingExternalParents { missing, .. } => {
                write!(
                    formatter,
                    "External parent(s) not found in store: {missing:?}"
                )
            }
            HeaderSyncError::MissingUncle { uncle_hash, .. } => {
                write!(
                    formatter,
                    "Declared uncle {uncle_hash} not delivered in batch and not in store"
                )
            }
            HeaderSyncError::MissingParentInBatch { header, parent, .. } => {
                write!(
                    formatter,
                    "Header {header} has parent {parent} which is not in batch or store"
                )
            }
            HeaderSyncError::AsertMismatch {
                block_hash,
                declared,
                expected,
            } => {
                write!(
                    formatter,
                    "ASERT mismatch for {block_hash}: declared bits {declared:#010x}, expected {expected:#010x}"
                )
            }
            HeaderSyncError::BelowFloor {
                block_hash,
                declared,
                floor,
            } => {
                write!(
                    formatter,
                    "Header {block_hash} declared bits {declared:#010x} below the sharechain difficulty floor {floor}"
                )
            }
            HeaderSyncError::InvalidTimestamp { block_hash, error } => {
                write!(formatter, "Invalid timestamp for {block_hash}: {error}")
            }
            HeaderSyncError::InsufficientWork => {
                write!(formatter, "Cumulative chain work below minimum")
            }
            HeaderSyncError::DenseHeight { height, count } => {
                write!(
                    formatter,
                    "Height {height} has {count} blocks, exceeds MAX_BLOCKS_PER_HEIGHT"
                )
            }
            HeaderSyncError::Other(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for HeaderSyncError {}

impl From<Box<dyn Error + Send + Sync>> for HeaderSyncError {
    fn from(error: Box<dyn Error + Send + Sync>) -> Self {
        HeaderSyncError::Other(error)
    }
}

impl From<StoreError> for HeaderSyncError {
    fn from(error: StoreError) -> Self {
        HeaderSyncError::Other(error.into())
    }
}

impl From<ValidationError> for HeaderSyncError {
    fn from(error: ValidationError) -> Self {
        HeaderSyncError::Other(error.into())
    }
}

/// Handle ShareHeaders received from a peer.
///
/// The peer_id identifies the peer that sent the headers, allowing
/// follow-up requests to be directed back to the same peer.
///
/// Phase 1 -- Validate:
///   - Each header meets minimum pool difficulty (uncle count + target floor)
///   - Headers form a single chain with uncles correctly interleaved
///   - Each header's timestamp is after its ancestors' median time past and
///     at most `MAX_FUTURE_TIME_SECS` ahead of the local clock, the rules
///     full validation applies
///   - Each header's bits matches the ASERT-computed target from its parent
///   - Cumulative chain work exceeds minimum threshold
///
/// Phase 2 -- Organise:
///   - Before organising each header, validate its uncles (count, duplicates,
///     positions, reuse by an ancestor). Organising credits uncle work to chain
///     work, so this cannot wait for the body. Earlier headers in the batch are
///     already organised, so the check sees them.
///   - Store validated headers via organise_header (persists metadata for later
///     validate_with_pool_difficulty in validate_share_block)
///
/// Then either request more headers or trigger block fetch.
pub async fn handle_share_headers<C: Send + Sync>(
    peer_id: libp2p::PeerId,
    header_batch: ShareHeaderBatch,
    chain_store_handle: ChainStoreHandle,
    swarm_tx: mpsc::Sender<SwarmSend<C>>,
    block_fetcher_handle: BlockFetcherHandle,
    share_validator: &(dyn ShareValidator + Send + Sync),
) -> Result<(), Box<dyn Error + Send + Sync>> {
    learn_carried_bch_headers(
        header_batch.bch_headers(),
        &chain_store_handle,
        share_validator,
    )
    .await?;
    let share_headers = header_batch.headers();
    debug!("Received {} ShareHeaders", share_headers.len());
    if !share_headers.is_empty() {
        let first = &share_headers[0];
        let last = share_headers.last().unwrap();
        debug!(
            "ShareHeaders batch: first={} (parent={}), last={} (parent={})",
            first.block_hash(),
            first.prev_share_blockhash,
            last.block_hash(),
            last.prev_share_blockhash,
        );
    }

    // No new headers, start fetching any block data for received headers
    if share_headers.is_empty() {
        return trigger_block_fetch(peer_id, &chain_store_handle, &block_fetcher_handle, None)
            .await;
    }

    // Phase 1: Validate the header chain in memory
    if let Err(sync_error) =
        validate_header_chain(&header_batch, &chain_store_handle, share_validator)
    {
        if sync_error.is_retryable() {
            // The depth is subtracted from build_locator's start height, which
            // is the candidate tip, so it must be measured against the same
            // base. Measuring it against the confirmed tip would anchor the
            // retry higher than intended -- a shallower locator, defeating a
            // retry whose purpose is to reach further back.
            let tip_height = chain_store_handle
                .get_candidate_tip_height()
                .ok()
                .flatten()
                .unwrap_or(0);
            let depth = sync_error.retry_depth(tip_height).unwrap_or(0);
            debug!(
                "Retryable header sync error, sending getheaders with depth {depth}: {sync_error}"
            );
            send_getheaders(peer_id, chain_store_handle, swarm_tx, depth).await?;
            return Ok(());
        }
        if matches!(sync_error, HeaderSyncError::DenseHeight { .. }) {
            error!("Disconnecting peer {peer_id}: {sync_error}");
            let _ = swarm_tx.send(SwarmSend::Disconnect(peer_id)).await;
        }
        return Err(sync_error.into());
    }

    // Phase 2: Store each header's coinbase merkle branch, so headers held
    // without their bodies can be served on with the branch their proof needs,
    // then organise the validated headers into the candidate chain.
    chain_store_handle
        .add_header_template_merkle_branches(header_batch.blockhashes_with_branches())
        .await?;
    for header in share_headers {
        if let Err(error) = share_validator.validate_uncles(header, &chain_store_handle) {
            error!(
                "Rejecting header {} from peer {peer_id}: {error}",
                header.block_hash()
            );
            return Err(error.into());
        }
        chain_store_handle.organise_header(header.clone()).await?;
    }

    let first_blockhash = share_headers[0].block_hash();

    trigger_or_request(
        peer_id,
        share_headers,
        &first_blockhash,
        &chain_store_handle,
        &swarm_tx,
        &block_fetcher_handle,
    )
    .await
}

/// Validate the received header batch as a connected DAG.
///
/// The batch contains a mix of main chain and uncle headers. Validation
/// checks that the batch forms a connected DAG (every header's parent is
/// either earlier in the batch or in the store), that every header is bound
/// to its proof of work by its coinbase proof and meets ASERT difficulty,
/// that every declared uncle is earlier in the batch or in the store, and
/// that cumulative work exceeds the minimum threshold.
fn validate_header_chain(
    header_batch: &ShareHeaderBatch,
    chain_store_handle: &ChainStoreHandle,
    share_validator: &(dyn ShareValidator + Send + Sync),
) -> Result<(), HeaderSyncError> {
    let share_headers = header_batch.headers();
    let pool_difficulty = share_validator.pool_difficulty();

    let anchors = find_chain_anchors(share_headers, chain_store_handle)?;
    let anchor_hashes: HashSet<BlockHash> = anchors.iter().map(|(hash, _)| *hash).collect();
    debug!("Found {} anchor(s) for header batch", anchors.len());

    let lowest_anchor_height = anchors
        .iter()
        .filter_map(|(_, metadata)| metadata.expected_height)
        .min()
        .unwrap_or(0);

    let best_anchor_work = anchors
        .iter()
        .map(|(_, metadata)| metadata.chain_work)
        .max()
        .unwrap_or(Work::from_hex("0x00").unwrap());

    let cumulative_chain_work = validate_dag_connectivity_and_difficulty(
        header_batch,
        share_validator,
        pool_difficulty,
        &anchor_hashes,
        chain_store_handle,
        lowest_anchor_height,
    )?;
    validate_cumulative_work(
        best_anchor_work,
        cumulative_chain_work,
        pool_difficulty.target_ceiling(),
    )?;

    debug!(
        "Validated {} headers, cumulative chain work: {cumulative_chain_work}",
        share_headers.len(),
    );
    Ok(())
}

/// Get what ASERT needs from the blockhash as a parent, checking the
/// batch-local cache first and falling back to the store. Store results
/// are cached for subsequent lookups within the same batch.
fn get_cached_asert_parent(
    blockhash: &BlockHash,
    cache: &mut HashMap<BlockHash, AsertParent>,
    chain_store_handle: &ChainStoreHandle,
) -> Result<AsertParent, Box<dyn Error + Send + Sync>> {
    if let Some(&cached) = cache.get(blockhash) {
        return Ok(cached);
    }
    let parent = chain_store_handle.get_asert_parent(blockhash)?;
    cache.insert(*blockhash, parent);
    Ok(parent)
}

/// Validate that the batch forms a connected DAG with valid difficulty.
///
/// Every header's parent and every uncle it declares must be either
/// already processed in this batch or present in the store. Headers are
/// organised in batch order, and organising a header reads its uncles'
/// headers to add their work, so an uncle delivered after its nephew is
/// missing. Every header must pass minimum difficulty,
/// its coinbase proof against the batch's branch for it, the timestamp rules
/// (`ShareValidator::validate_header_timestamp`, against the times of up to
/// `MTP_WINDOW` ancestors from the batch and the store), and ASERT target
/// validation. The timestamp check comes before ASERT because each header's
/// target is computed from its parent's time: blocks below the
/// full-validation zone never run `validate_timestamp`, so this is the only
/// place their times are checked.
///
/// Returns the cumulative work across all headers.
fn validate_dag_connectivity_and_difficulty(
    header_batch: &ShareHeaderBatch,
    share_validator: &(dyn ShareValidator + Send + Sync),
    pool_difficulty: &PoolDifficulty,
    anchor_hashes: &HashSet<BlockHash>,
    chain_store_handle: &ChainStoreHandle,
    lowest_anchor_height: u32,
) -> Result<Work, HeaderSyncError> {
    let share_headers = header_batch.headers();
    let mut known_hashes: HashSet<BlockHash> =
        HashSet::with_capacity(share_headers.len() + anchor_hashes.len());
    known_hashes.extend(anchor_hashes);
    let mut validated_headers: HashMap<BlockHash, &ShareHeader> =
        HashMap::with_capacity(share_headers.len());

    let mut cumulative_work = Work::from_hex("0x00").unwrap();
    let mut asert_parent_cache: HashMap<BlockHash, AsertParent> =
        HashMap::with_capacity(share_headers.len() + 1);
    let mut time_links: HashMap<BlockHash, TimeLink> =
        HashMap::with_capacity(share_headers.len() + MTP_WINDOW);
    let mut blocks_per_height: HashMap<u32, usize> = HashMap::with_capacity(64);

    for (position, header) in share_headers.iter().enumerate() {
        share_validator.validate_header_minimum_difficulty(header)?;
        share_validator.validate_coinbase_proof(header, header_batch.branch(position))?;

        let parent_hash = header.prev_share_blockhash;
        if !known_hashes.contains(&parent_hash)
            && chain_store_handle.get_block_metadata(&parent_hash).is_err()
        {
            return Err(HeaderSyncError::MissingParentInBatch {
                header: header.block_hash(),
                parent: parent_hash,
                lowest_anchor_height,
            });
        }

        let ancestor_times =
            collect_ancestor_times(parent_hash, &mut time_links, chain_store_handle)?;
        share_validator
            .validate_header_timestamp(header, &ancestor_times)
            .map_err(|error| HeaderSyncError::InvalidTimestamp {
                block_hash: header.block_hash(),
                error,
            })?;

        let parent =
            get_cached_asert_parent(&parent_hash, &mut asert_parent_cache, chain_store_handle)?;

        if share_validator.derives_sharechain_floor() {
            let d_asert_capped = target_to_difficulty_rounded_up(Target::from_compact(
                pool_difficulty.calculate_target_clamped(parent),
            ));
            let floor = derive_sync_sharechain_floor(
                SyncFloorWalkStart {
                    parent_hash,
                    header_hash: header.block_hash(),
                    parent_height: parent.height,
                },
                &validated_headers,
                chain_store_handle,
                d_asert_capped,
                BCH_COINBASE_MATURITY,
                lowest_anchor_height,
            )?;
            check_sharechain_band(header.bits, floor, pool_difficulty.target_ceiling()).map_err(
                |_rejection| HeaderSyncError::BelowFloor {
                    block_hash: header.block_hash(),
                    declared: header.bits.to_consensus(),
                    floor,
                },
            )?;
        } else {
            let expected_bits = pool_difficulty.calculate_target_clamped(parent);
            if header.bits != expected_bits {
                return Err(HeaderSyncError::AsertMismatch {
                    block_hash: header.block_hash(),
                    declared: header.bits.to_consensus(),
                    expected: expected_bits.to_consensus(),
                });
            }
        }

        let header_height = parent.height + 1;
        check_dense_height(&mut blocks_per_height, header_height)?;
        check_uncles_known(
            header,
            &known_hashes,
            chain_store_handle,
            lowest_anchor_height,
        )?;

        let header_hash = header.block_hash();
        asert_parent_cache.insert(
            header_hash,
            AsertParent::from_header(header, header_height, parent.time),
        );
        time_links.insert(
            header_hash,
            TimeLink {
                time: header.time,
                parent: parent_hash,
            },
        );
        known_hashes.insert(header_hash);
        validated_headers.insert(header_hash, header);
        cumulative_work = cumulative_work + header.get_work();
    }

    Ok(cumulative_work)
}

/// One floor-window entry as the header-sync walk reads it: the fields the
/// floor arithmetic needs, without the payout window cache's payout fields,
/// which header sync does not track.
struct SyncFloorEntry {
    declared_difficulty: u128,
    miner: PayoutAddress,
    uncles: Vec<BlockHash>,
}

/// Where a header-sync floor walk starts: the validated header's parent
/// (the first window entry) and the validated header itself, whose hash is
/// the child reported when that first entry is missing.
struct SyncFloorWalkStart {
    parent_hash: BlockHash,
    header_hash: BlockHash,
    parent_height: u32,
}

/// Derive the PPLNS window floor for the header whose parent is
/// `start.parent_hash`, as the vardiff share chain checks it at header sync
/// (see docs/sharechain/vardiff-sharechain.md).
///
/// The walk mirrors `PplnsWindow::get_sharechain_floor_from_start_hash`:
/// the same `PPLNS_WINDOW_SHARES` window walked down from the validated
/// header's parent, the same `BCH_COINBASE_MATURITY` skip counted from that
/// parent, and the same credits (each share's own `d` to its miner, each
/// uncle's `d` to the uncle's miner). It walks naively, one store read per
/// ancestor below the batch -- the batch is small and this runs only on the
/// vardiff chain -- and refuses (`MissingParentInBatch`, `MissingUncle`)
/// rather than deriving a floor over a partial window.
///
/// `validated_headers` holds the batch headers earlier in the parent-before-
/// child order, so every ancestor is either there or in the store. Uncle
/// headers are looked for in the batch first, then read from the store in
/// one batch, and a miss fails closed.
fn derive_sync_sharechain_floor(
    start: SyncFloorWalkStart,
    validated_headers: &HashMap<BlockHash, &ShareHeader>,
    chain_store_handle: &ChainStoreHandle,
    d_asert_capped: u128,
    maturity_skip: usize,
    lowest_anchor_height: u32,
) -> Result<u64, HeaderSyncError> {
    let paying_height = start.parent_height as usize + 1;
    let immature_share_count = maturity_skip
        .saturating_sub(1)
        .min(paying_height.saturating_sub(maturity_skip));

    let mut window_entries: Vec<SyncFloorEntry> = Vec::with_capacity(PPLNS_WINDOW_SHARES);
    let mut entry_hash = start.parent_hash;
    let mut entry_height = start.parent_height;
    let mut child_hash = start.header_hash;
    let mut skipped_entries = 0usize;
    let mut reached_chain_start = false;
    while window_entries.len() < PPLNS_WINDOW_SHARES && !reached_chain_start {
        let (declared_difficulty, miner, uncles, prev_hash) =
            if let Some(header) = validated_headers.get(&entry_hash) {
                (
                    declared_share_difficulty(header),
                    header.miner_bitcoin_address.clone(),
                    header.uncles.clone(),
                    header.prev_share_blockhash,
                )
            } else {
                let header = chain_store_handle
                    .get_share_header(&entry_hash)
                    .map_err(|error| match error {
                        StoreError::NotFound(_message) => HeaderSyncError::MissingParentInBatch {
                            header: child_hash,
                            parent: entry_hash,
                            lowest_anchor_height,
                        },
                        error => HeaderSyncError::Other(Box::new(error)),
                    })?;
                (
                    declared_share_difficulty(&header),
                    header.miner_bitcoin_address.clone(),
                    header.uncles.clone(),
                    header.prev_share_blockhash,
                )
            };

        if skipped_entries < immature_share_count {
            skipped_entries += 1;
        } else {
            window_entries.push(SyncFloorEntry {
                declared_difficulty,
                miner,
                uncles,
            });
        }

        reached_chain_start = entry_height == 0;
        if !reached_chain_start {
            child_hash = entry_hash;
            entry_hash = prev_hash;
            entry_height -= 1;
        }
    }

    let mut seen_uncles: HashSet<BlockHash> = HashSet::new();
    let mut unmaterialized_uncles: Vec<BlockHash> = Vec::new();
    for entry in &window_entries {
        for uncle_hash in &entry.uncles {
            if seen_uncles.insert(*uncle_hash) {
                unmaterialized_uncles.push(*uncle_hash);
            }
        }
    }

    let mut resolved_uncles: HashMap<BlockHash, (u128, PayoutAddress)> =
        HashMap::with_capacity(unmaterialized_uncles.len());
    for uncle_hash in &unmaterialized_uncles {
        if let Some(header) = validated_headers.get(uncle_hash) {
            resolved_uncles.insert(
                *uncle_hash,
                (
                    declared_share_difficulty(header),
                    header.miner_bitcoin_address.clone(),
                ),
            );
        }
    }
    if !unmaterialized_uncles.is_empty() {
        let uncle_headers = chain_store_handle
            .get_share_headers(&unmaterialized_uncles)
            .map_err(|error| HeaderSyncError::Other(Box::new(error)))?;
        for (uncle_hash, header) in uncle_headers {
            resolved_uncles.insert(
                uncle_hash,
                (
                    declared_share_difficulty(&header),
                    header.miner_bitcoin_address.clone(),
                ),
            );
        }
    }
    for uncle_hash in &unmaterialized_uncles {
        if !resolved_uncles.contains_key(uncle_hash) {
            return Err(HeaderSyncError::MissingUncle {
                uncle_hash: *uncle_hash,
                lowest_anchor_height,
            });
        }
    }

    let credits = window_entries.iter().flat_map(|entry| {
        iter::once(FloorCredit {
            miner: entry.miner.clone(),
            declared_difficulty: entry.declared_difficulty,
            counts_for_presence: true,
        })
        .chain(entry.uncles.iter().map(|uncle_hash| {
            let (declared_difficulty, miner) = resolved_uncles
                .get(uncle_hash)
                .expect("every window uncle is resolved before any credit is built");
            FloorCredit {
                miner: miner.clone(),
                declared_difficulty: *declared_difficulty,
                counts_for_presence: false,
            }
        }))
    });
    derive_sharechain_floor(credits, d_asert_capped, SHARECHAIN_FLOOR_PRESENCE_MINIMUM)
        .map_err(|error| HeaderSyncError::Other(Box::new(error)))
}

/// A header's time and parent, all the median time past walk reads.
#[derive(Debug, Clone, Copy)]
struct TimeLink {
    time: u32,
    parent: BlockHash,
}

/// The times of up to `MTP_WINDOW` ancestors ending at `parent_hash`, newest
/// first, as `ShareValidator::validate_header_timestamp` takes them.
///
/// Headers earlier in the batch are in `time_links` already; ancestors in the
/// store are read once and cached there, so a batch reads at most
/// `MTP_WINDOW` headers from the store per anchor. The walk stops at genesis.
/// An ancestor the store does not hold fails the batch: connectivity has been
/// checked, so every ancestor is in the batch or the store.
fn collect_ancestor_times(
    parent_hash: BlockHash,
    time_links: &mut HashMap<BlockHash, TimeLink>,
    chain_store_handle: &ChainStoreHandle,
) -> Result<Vec<u32>, HeaderSyncError> {
    let mut ancestor_times = Vec::with_capacity(MTP_WINDOW);
    let mut current = parent_hash;
    while ancestor_times.len() < MTP_WINDOW && !is_terminal_blockhash(&current) {
        let link = match time_links.get(&current) {
            Some(link) => *link,
            None => {
                let header = chain_store_handle.get_share_header(&current)?;
                let link = TimeLink {
                    time: header.time,
                    parent: header.prev_share_blockhash,
                };
                time_links.insert(current, link);
                link
            }
        };
        ancestor_times.push(link.time);
        current = link.parent;
    }
    Ok(ancestor_times)
}

/// Reject batches where a single height has too many blocks.
///
/// In normal operation a height has at most a few blocks (confirmed +
/// uncles). A dense height indicates a misbehaving or attacking peer.
fn check_dense_height(
    blocks_per_height: &mut HashMap<u32, usize>,
    height: u32,
) -> Result<(), HeaderSyncError> {
    let count = blocks_per_height.entry(height).or_insert(0);
    *count += 1;
    if *count > MAX_BLOCKS_PER_HEIGHT {
        return Err(HeaderSyncError::DenseHeight {
            height,
            count: *count,
        });
    }
    Ok(())
}

/// Verify every uncle declared by `header` is a header already processed
/// in this batch or is in the store. Catches phantom uncle references that
/// no peer ever sent, and uncles delivered after their nephew.
///
/// Ported from pool2win's upstream P2Poolv2 commit 604fe3ce
/// (MIT OR Apache-2.0).
fn check_uncles_known(
    header: &ShareHeader,
    known_hashes: &HashSet<BlockHash>,
    chain_store_handle: &ChainStoreHandle,
    lowest_anchor_height: u32,
) -> Result<(), HeaderSyncError> {
    for uncle_hash in &header.uncles {
        if !known_hashes.contains(uncle_hash)
            && chain_store_handle.get_block_metadata(uncle_hash).is_err()
        {
            return Err(HeaderSyncError::MissingUncle {
                uncle_hash: *uncle_hash,
                lowest_anchor_height,
            });
        }
    }
    Ok(())
}

/// Find all chain anchors: external parents from the batch that exist
/// in our store.
///
/// Collects all `prev_share_blockhash` values that are NOT themselves
/// hashes of headers in this batch (i.e., external parents from the
/// store). With height-based batches, multiple blocks may have
/// external parents at the previous batch boundary on different
/// branches.
fn find_chain_anchors(
    share_headers: &[ShareHeader],
    chain_store_handle: &ChainStoreHandle,
) -> Result<Vec<(BlockHash, BlockMetadata)>, HeaderSyncError> {
    let batch_hashes: HashSet<BlockHash> = share_headers
        .iter()
        .map(|header| header.block_hash())
        .collect();

    let external_parents: Vec<BlockHash> = share_headers
        .iter()
        .map(|header| header.prev_share_blockhash)
        .filter(|parent| !batch_hashes.contains(parent))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();

    let metadata_results = chain_store_handle
        .get_block_metadata_batch(&external_parents)
        .map_err(|error| HeaderSyncError::Other(Box::new(error)))?;

    if metadata_results.len() != external_parents.len() {
        let found: HashSet<BlockHash> = metadata_results.iter().map(|(hash, _)| *hash).collect();
        let missing: Vec<BlockHash> = external_parents
            .into_iter()
            .filter(|hash| !found.contains(hash))
            .collect();
        let lowest_anchor_height = metadata_results
            .iter()
            .filter_map(|(_, metadata)| metadata.expected_height)
            .min()
            .unwrap_or(0);
        return Err(HeaderSyncError::MissingExternalParents {
            missing,
            lowest_anchor_height,
        });
    }

    Ok(metadata_results)
}

/// Verify cumulative chain work exceeds the minimum threshold.
///
/// The threshold is `MIN_CUMULATIVE_CHAIN_WORK_MULTIPLIER` shares at the
/// chain's easiest target, `target_ceiling`. A Bitcoin Cash mini share is
/// 256 times easier than a main share, so measuring it against
/// `MAX_POOL_TARGET` rejected every header batch from a mini chain with
/// fewer than 256 shares, and a new node could not sync one.
fn validate_cumulative_work(
    anchor_chain_work: Work,
    batch_chain_work: Work,
    target_ceiling: CompactTarget,
) -> Result<(), HeaderSyncError> {
    let single_share_work = Target::from_compact(target_ceiling).to_work();
    let zero_work = Work::from_hex("0x00").unwrap();
    let min_cumulative_work = (0..MIN_CUMULATIVE_CHAIN_WORK_MULTIPLIER)
        .fold(zero_work, |accumulated, _| accumulated + single_share_work);
    let candidate_work = anchor_chain_work + batch_chain_work;

    if candidate_work < min_cumulative_work {
        return Err(HeaderSyncError::InsufficientWork);
    }
    Ok(())
}

/// Either trigger block fetch or request next batch of headers.
async fn trigger_or_request<C: Send + Sync>(
    peer_id: libp2p::PeerId,
    share_headers: &[ShareHeader],
    first_blockhash: &BlockHash,
    chain_store_handle: &ChainStoreHandle,
    swarm_tx: &mpsc::Sender<SwarmSend<C>>,
    block_fetcher_handle: &BlockFetcherHandle,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    if share_headers.len() < MAX_HEADERS_IN_RESPONSE {
        let fork_height = chain_store_handle
            .find_fork_point_height(first_blockhash)
            .unwrap_or_else(|error| {
                warn!(
                    "Failed to find fork point for {}: {}",
                    first_blockhash, error
                );
                None
            });
        trigger_block_fetch(
            peer_id,
            chain_store_handle,
            block_fetcher_handle,
            fork_height,
        )
        .await
    } else {
        request_next_headers(peer_id, share_headers, swarm_tx).await
    }
}

/// Header sync is complete -- query for candidate blocks missing full
/// block data and send them to the block fetcher for download.
///
/// When `scan_start_height` is provided, the scan extends down to
/// that height so fork blocks at or below the confirmed tip are
/// included in the fetch request.
async fn trigger_block_fetch(
    peer_id: libp2p::PeerId,
    chain_store_handle: &ChainStoreHandle,
    block_fetcher_handle: &BlockFetcherHandle,
    fork_height: Option<u32>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    debug!("Header sync complete, triggering block fetch for missing data");
    let missing_blockhashes = chain_store_handle.get_candidate_blocks_missing_data(fork_height)?;
    if !missing_blockhashes.is_empty() {
        debug!(
            "Requesting {} blocks from block fetcher",
            missing_blockhashes.len()
        );
        let use_peer = chain_store_handle.is_current();
        if let Err(send_error) = block_fetcher_handle
            .send(BlockFetcherEvent::FetchBlocks {
                blockhashes: missing_blockhashes,
                peer_id,
                use_peer,
            })
            .await
        {
            error!(
                "Failed to send FetchBlocks to block fetcher: {}",
                send_error
            );
        }
    } else {
        debug!("No candidate blocks missing data");
    }
    Ok(())
}

/// More headers are available -- send a follow-up GetShareHeaders
/// request to the same peer starting from the last received header.
async fn request_next_headers<C: Send + Sync>(
    peer_id: libp2p::PeerId,
    share_headers: &[ShareHeader],
    swarm_tx: &mpsc::Sender<SwarmSend<C>>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let stop_block_hash = BlockHash::all_zeros();
    let last_block_hash = share_headers.last().unwrap().block_hash();
    debug!(
        "Requesting more share headers, locator={last_block_hash}, batch_size={}",
        share_headers.len(),
    );
    let getheaders_request = Message::GetShareHeaders(vec![last_block_hash], stop_block_hash);
    if let Err(send_error) = swarm_tx
        .send(SwarmSend::Request(peer_id, getheaders_request))
        .await
    {
        error!("Failed to send getheaders request: {}", send_error);
        return Err(format!("Failed to send getheaders request: {send_error}").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
