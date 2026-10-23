// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Database maintenance commands that operate directly on the RocksDB store.
//!
//! These commands require `--db-path` and open the store in read-write mode.
//! The node must be stopped before running these commands.

use super::DbCommands;
use bitcoin::BlockHash;
use p2poolv2_lib::store::Store;
use p2poolv2_lib::store::block_tx_metadata::{ChainMembership, Status};
use p2poolv2_lib::store::dag_store::MAX_BLOCKS_PER_HEIGHT;
use p2poolv2_lib::store::writer::StoreError;
use std::error::Error;

/// Open a RocksDB store in read-write mode for maintenance operations.
fn open_store_readwrite(db_path: &str) -> Result<Store, Box<dyn Error>> {
    Store::new(db_path.to_string(), false)
        .map_err(|error| format!("Failed to open database at {db_path}: {error}").into())
}

/// Dispatch a db subcommand.
pub fn execute(command: &DbCommands, db_path: &str) -> Result<(), Box<dyn Error>> {
    let store = open_store_readwrite(db_path)?;
    match command {
        DbCommands::CleanupDenseHeights => cleanup_dense_heights(&store),
    }
}

/// Whether any block that references `blockhash` as an uncle is still one the
/// sync sender would serve (status not `Invalid`, not `Pending`).
///
/// A genuine uncle -- one declared by a served block -- must be kept: the sender
/// filters `Invalid`/`Pending` blocks, so invalidating a served block's uncle
/// would leave the receiver unable to satisfy `verify_all_uncles_available`.
/// A flood sibling referenced only by other dead (Invalid) siblings has no live
/// nephew and is safe to invalidate. Because uncles are strictly lower-height
/// ancestors of their nephews, processing heights top-down means every nephew
/// has already been finalised when its uncle is examined, so this single check
/// yields the full transitive closure of uncles reachable from served blocks.
fn has_live_nephew(store: &Store, blockhash: &BlockHash) -> Result<bool, Box<dyn Error>> {
    let Some(nephews) = store.get_nephews(blockhash) else {
        return Ok(false);
    };
    for nephew in nephews {
        let metadata = store.get_block_metadata(&nephew)?;
        if metadata.status != Status::Invalid && metadata.status != Status::Pending {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Clean up dense heights by marking excess off-chain blocks as Invalid.
///
/// Walks the height index top-down and for each height with more than
/// MAX_BLOCKS_PER_HEIGHT blocks, keeps every block on the candidate or
/// confirmed chain and every block still referenced as an uncle by a served
/// block. Marks the remaining off-chain blocks -- whether `HeaderValid`
/// (received via header sync) or `BlockValid` (mined locally onto a losing
/// fork, the dense-height flood) -- as Invalid, so the sync sender's
/// Pending/Invalid filter stops serving them and the height collapses to its
/// real chain block(s).
///
/// Heights are processed top-down so that flood siblings which only reference
/// each other as uncles collapse: the highest are invalidated first, which
/// then leaves the ones below them with no live nephew.
fn cleanup_dense_heights(store: &Store) -> Result<(), Box<dyn Error>> {
    let top_height = match store.get_top_confirmed_height() {
        Ok(height) => height,
        Err(StoreError::NotFound(_)) => {
            println!("No confirmed chain found, nothing to clean up");
            return Ok(());
        }
        Err(error) => return Err(format!("Failed to get top height: {error}").into()),
    };

    let mut total_invalidated = 0usize;
    let mut dense_heights = 0usize;

    let height_entries = store.get_blockhashes_for_height_range(0, top_height);

    // Top-down: a block's nephews are strictly higher, so they are finalised
    // before the block is examined.
    for (height, blockhashes) in height_entries.into_iter().rev() {
        if blockhashes.len() <= MAX_BLOCKS_PER_HEIGHT {
            continue;
        }

        let metadata_results = store.get_block_metadata_batch(&blockhashes)?;

        // Off-chain blocks the sender would serve (HeaderValid or BlockValid,
        // and not on the candidate or confirmed chain) are eligible for
        // invalidation. Blocks on the candidate or confirmed chain are always
        // kept; blocks still referenced as an uncle by a served block are kept
        // below. A BlockValid off-chain block is a share this node mined onto
        // a losing fork.
        let off_chain_removable: Vec<BlockHash> = metadata_results
            .iter()
            .filter(|(_, metadata)| {
                metadata.chain == ChainMembership::None
                    && matches!(metadata.status, Status::HeaderValid | Status::BlockValid)
            })
            .map(|(hash, _)| *hash)
            .collect();

        if off_chain_removable.is_empty() {
            continue;
        }

        dense_heights += 1;
        let kept_count = blockhashes.len() - off_chain_removable.len();
        println!(
            "Height {height}: {total} blocks ({kept} on-chain, {removable} off-chain removable)",
            total = blockhashes.len(),
            kept = kept_count,
            removable = off_chain_removable.len(),
        );

        let mut batch = Store::get_write_batch();
        let mut height_invalidated = 0usize;

        for blockhash in &off_chain_removable {
            if has_live_nephew(store, blockhash)? {
                continue;
            }
            let mut metadata = store.get_block_metadata(blockhash)?;
            metadata.status = Status::Invalid;
            store.update_block_metadata(blockhash, &metadata, &mut batch)?;
            height_invalidated += 1;
        }

        if height_invalidated > 0 {
            store.commit_batch(batch)?;
            println!("  Invalidated {height_invalidated} off-chain blocks with no live nephew");
            total_invalidated += height_invalidated;
        }
    }

    println!(
        "Cleanup complete: {dense_heights} dense heights found, {total_invalidated} blocks invalidated"
    );
    Ok(())
}

#[cfg(test)]
mod tests;
