// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::node::messages::ShareHeaderBatch;
use crate::node::p2p_message_handlers::MAX_HEADERS_IN_RESPONSE;
use crate::node::p2p_message_handlers::senders::send_getheaders;
use crate::node::request_response_handler::block_fetcher::{BlockFetcherEvent, BlockFetcherHandle};
use crate::node::{SwarmSend, messages::Message};
use crate::pool_difficulty::AsertParent;
#[cfg(test)]
#[mockall_double::double]
use crate::pool_difficulty::PoolDifficulty;
#[cfg(not(test))]
use crate::pool_difficulty::PoolDifficulty;
#[cfg(test)]
#[mockall_double::double]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
#[cfg(not(test))]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
use crate::shares::share_block::{MIN_CUMULATIVE_CHAIN_WORK_MULTIPLIER, ShareHeader};
use crate::shares::validation::ShareValidator;
use crate::store::block_tx_metadata::BlockMetadata;
use crate::store::dag_store::MAX_BLOCKS_PER_HEIGHT;
use crate::store::writer::StoreError;
use bitcoin::hashes::Hash;
use bitcoin::{BlockHash, CompactTarget, Target, Work};
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;
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

impl From<crate::shares::validation::ValidationError> for HeaderSyncError {
    fn from(error: crate::shares::validation::ValidationError) -> Self {
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
///   - Each header's bits matches the ASERT-computed target from its parent
///   - Cumulative chain work exceeds minimum threshold
///
/// Phase 2 -- Organise:
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
/// that all declared uncle hashes are available, and that cumulative work
/// exceeds the minimum threshold.
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

    let (batch_hashes, cumulative_chain_work) = validate_dag_connectivity_and_difficulty(
        header_batch,
        share_validator,
        pool_difficulty,
        &anchor_hashes,
        chain_store_handle,
        lowest_anchor_height,
    )?;
    verify_all_uncles_available(
        share_headers,
        &batch_hashes,
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
/// Every header's parent must be either already processed in this batch
/// or present in the store. Every header must pass minimum difficulty,
/// its coinbase proof against the batch's branch for it, and ASERT target
/// validation.
///
/// Returns the set of all blockhashes in the batch and the cumulative
/// work across all headers.
fn validate_dag_connectivity_and_difficulty(
    header_batch: &ShareHeaderBatch,
    share_validator: &(dyn ShareValidator + Send + Sync),
    pool_difficulty: &PoolDifficulty,
    anchor_hashes: &HashSet<BlockHash>,
    chain_store_handle: &ChainStoreHandle,
    lowest_anchor_height: u32,
) -> Result<(HashSet<BlockHash>, Work), HeaderSyncError> {
    let share_headers = header_batch.headers();
    let mut known_hashes: HashSet<BlockHash> =
        HashSet::with_capacity(share_headers.len() + anchor_hashes.len());
    known_hashes.extend(anchor_hashes);

    let mut cumulative_work = Work::from_hex("0x00").unwrap();
    let mut asert_parent_cache: HashMap<BlockHash, AsertParent> =
        HashMap::with_capacity(share_headers.len() + 1);
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

        let parent =
            get_cached_asert_parent(&parent_hash, &mut asert_parent_cache, chain_store_handle)?;

        let expected_bits = pool_difficulty.calculate_target_clamped(parent);
        if header.bits != expected_bits {
            return Err(HeaderSyncError::AsertMismatch {
                block_hash: header.block_hash(),
                declared: header.bits.to_consensus(),
                expected: expected_bits.to_consensus(),
            });
        }

        let header_height = parent.height + 1;
        check_dense_height(&mut blocks_per_height, header_height)?;

        let header_hash = header.block_hash();
        asert_parent_cache.insert(
            header_hash,
            AsertParent::from_header(header, header_height, parent.time),
        );
        known_hashes.insert(header_hash);
        cumulative_work = cumulative_work + header.get_work();
    }

    Ok((known_hashes, cumulative_work))
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

/// Verify every uncle hash declared by any header in the batch exists
/// either as another header in this batch or in the store. Catches
/// phantom uncle references that no peer ever sent.
fn verify_all_uncles_available(
    share_headers: &[ShareHeader],
    batch_hashes: &HashSet<BlockHash>,
    chain_store_handle: &ChainStoreHandle,
    lowest_anchor_height: u32,
) -> Result<(), HeaderSyncError> {
    for header in share_headers {
        for uncle_hash in &header.uncles {
            if !batch_hashes.contains(uncle_hash)
                && chain_store_handle.get_block_metadata(uncle_hash).is_err()
            {
                return Err(HeaderSyncError::MissingUncle {
                    uncle_hash: *uncle_hash,
                    lowest_anchor_height,
                });
            }
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
