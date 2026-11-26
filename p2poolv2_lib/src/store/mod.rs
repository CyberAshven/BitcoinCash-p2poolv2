// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::accounting::payout::sharechain_pplns::pplns_window::ChainDepths;
use crate::shares::share_block::ShareBlock;
use crate::store::block_tx_metadata::{BlockMetadata, ChainMembership, Status};
use crate::store::column_families::ColumnFamily;
use crate::store::dag_store::{MAX_BLOCKS_PER_HEIGHT, MAX_UNCLES_DEPTH};
use crate::store::hashrate_samples::HashrateSample;
use crate::store::script_history::script_history_cf_options;
use bitcoin::consensus::Encodable;
use bitcoin::consensus::encode;
use bitcoin::{BlockHash, Work};
use rocksdb::{
    BlockBasedOptions, ColumnFamilyDescriptor, DB, DEFAULT_COLUMN_FAMILY_NAME,
    Options as RocksDbOptions,
};
use std::collections::HashMap;
use std::sync::atomic::AtomicU32;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, warn};
use writer::StoreError;

/// Key in the `Metadata` column family that holds the store format version.
const STORE_FORMAT_KEY: &[u8] = b"store_format";

/// Format version of the share-chain store this build writes. A store with
/// another version, or with none, was written under other rules: startup
/// refuses it, so the operator resyncs the share chain instead of mixing
/// old and new data in one database.
///
/// The version rises whenever what the store holds, or the rules a stored
/// chain was validated under, change: the share-chain rule flag days and
/// chain work that counts uncle work. `docs/sharechain/rule-history.md`
/// lists each version.
pub const STORE_FORMAT_VERSION: u32 = 12;

pub mod bch_headers;
pub mod block_tx_metadata;
pub mod column_families;
pub mod dag_store;
pub mod found_block;
pub mod hashrate_samples;
pub mod organise;
pub mod payout_tree;
mod pplns_shares;
pub mod prune_blocks;
mod prune_shares;
pub mod script_history;
pub mod share_store;
pub mod stored_user;
pub mod transaction_store;
pub mod user;
pub mod writer;

/// A store for share blocks.
/// RocksDB as is used as the underlying database.
/// We use column families to store different types of data, so that compactions are independent for each type.
/// Key Value stores in column families:
/// - block: share blocks
/// - block_txids: txids for a block, to get transactions for a block. A tx can appear in multiple blocks.
/// - inputs: inputs for a transaction, to get inputs for a tx.
/// - outputs: outputs for a transaction, to get outputs for a tx. These can be marked as spent. So these are updated.
#[allow(dead_code)]
pub struct Store {
    path: String,
    db: DB,
    // Thread-safe chain state for use by ChainStore
    genesis_blockhash: Arc<RwLock<Option<BlockHash>>>,
    /// The chain's share-chain depths, which size pruning and the reorg
    /// depth limit.
    depths: ChainDepths,
    /// Blocks whose candidate reorg was refused for forking below the reorg
    /// floor, each with the height its branch forks from. In memory only.
    refused_reorgs: Mutex<HashMap<BlockHash, u32>>,
    /// Script history rows below this height are dropped by the
    /// `ScriptHistory` compaction filter. Shared with that filter.
    script_history_gc_threshold: Arc<AtomicU32>,
    /// The first confirmed height whose script history this store holds: 0
    /// when it holds every height, `u32::MAX` while it writes none
    /// (`with_script_history`). Loaded from `Metadata` when the store opens.
    script_history_from: AtomicU32,
    /// Whether whole weak shares are kept until their share's body is
    /// pruned, as an explorer or archive wants, instead of one payout depth
    /// (`with_whole_weak_shares_kept`, `store.keep_whole_weak_shares`).
    keep_whole_weak_shares: bool,
}

/// Merge operator for appending BlockHashes to a Vec<BlockHash>.
///
/// This allows atomic append operations without read-modify-write cycles.
///
/// Registered via `set_merge_operator_associative`, so the same function
/// handles both full merge and partial merge:
/// - Full merge: `existing_val` is the base value from a prior Put/merge
///   result (a serialized `Vec<BlockHash>`) or `None`.
/// - Partial merge: `existing_val` is `Some(left_operand)` where the left
///   operand may be a raw 32-byte `BlockHash` OR a previously merged
///   `Vec<BlockHash>`. Operands may also be either format.
///
/// Each value passed in (existing_val or operand) is parsed by trying
/// `Vec<BlockHash>` first, then falling back to a single `BlockHash`.
fn blockhash_list_merge(
    _key: &[u8],
    existing_val: Option<&[u8]>,
    operands: &rocksdb::MergeOperands,
) -> Option<Vec<u8>> {
    let mut blockhashes: Vec<BlockHash> = match existing_val {
        Some(bytes) => parse_blockhash_bytes(bytes)?,
        None => Vec::new(),
    };

    for op in operands {
        for hash in parse_blockhash_bytes(op)? {
            if !blockhashes.contains(&hash) {
                blockhashes.push(hash);
            }
        }
    }

    let mut result = Vec::new();
    blockhashes.consensus_encode(&mut result).ok()?;
    Some(result)
}

/// Parse bytes as either a single raw 32-byte `BlockHash` or a
/// serialized `Vec<BlockHash>` (compact_size length prefix + N hashes).
///
/// Disambiguation is by length: a raw `BlockHash` is always exactly
/// 32 bytes, while a serialized `Vec<BlockHash>` is never 32 bytes
/// (0 elements = 1 byte, 1 element = 33 bytes, 2 elements = 65 bytes,
/// etc.). This avoids relying on `encode::deserialize` trial order,
/// which is unsafe because `deserialize` does not require full input
/// consumption -- a 32-byte hash starting with 0x00 would decode as
/// an empty Vec, silently losing the hash.
///
/// Returns `None` on unrecognised input so the merge operator can
/// propagate the failure to RocksDB instead of silently discarding data.
fn parse_blockhash_bytes(bytes: &[u8]) -> Option<Vec<BlockHash>> {
    const BLOCKHASH_LEN: usize = 32;
    if bytes.len() == BLOCKHASH_LEN {
        let hash = encode::deserialize::<BlockHash>(bytes).ok()?;
        return Some(vec![hash]);
    }
    let hashes = encode::deserialize::<Vec<BlockHash>>(bytes).ok()?;
    Some(hashes)
}

/// The column families in `column_families_on_disk` that are not in
/// `known_column_families` and are not the `default` one from RocksDB,
/// which every database has and an open adds itself.
fn unknown_column_families<'a>(
    column_families_on_disk: &'a [String],
    known_column_families: &[ColumnFamilyDescriptor],
) -> Vec<&'a str> {
    column_families_on_disk
        .iter()
        .map(String::as_str)
        .filter(|name| {
            *name != DEFAULT_COLUMN_FAMILY_NAME
                && !known_column_families
                    .iter()
                    .any(|descriptor| descriptor.name() == *name)
        })
        .collect()
}

/// A rocksdb based store for share blocks.
/// We use column families to store different types of data, so that compactions are independent for each type.
#[allow(dead_code)]
impl Store {
    /// Build CF options with a 10-bit bloom filter for point-lookup CFs.
    /// Bloom filters eliminate disk reads for keys that don't exist,
    /// which speeds up existence checks and negative lookups during sync.
    fn cf_options_with_bloom() -> RocksDbOptions {
        let mut block_opts = BlockBasedOptions::default();
        block_opts.set_bloom_filter(10.0, false);
        let mut opts = RocksDbOptions::default();
        opts.set_block_based_table_factory(&block_opts);
        opts
    }

    /// Create a new share store
    ///
    /// A read-only open refuses a store whose column families on disk are
    /// not exactly the ones this binary uses (`check_column_families_on_disk`),
    /// and opens every one with its own options, merge operators included.
    pub fn new(path: String, read_only: bool) -> Result<Self, StoreError> {
        let (db, script_history_gc_threshold) = Self::open_db(&path, read_only)?;
        let store = Self {
            path,
            db,
            // Initialise chain state fields
            genesis_blockhash: Arc::new(RwLock::new(None)),
            depths: ChainDepths::BITCOIN,
            refused_reorgs: Mutex::new(HashMap::new()),
            script_history_gc_threshold,
            script_history_from: AtomicU32::new(0),
            keep_whole_weak_shares: false,
        };
        store.load_script_history_from()?;
        Ok(store)
    }

    /// Open the RocksDB database at `path` with every column family and its
    /// options. Also returns the script history GC threshold its compaction
    /// filter reads.
    ///
    /// A writable open creates the store and any missing column family. A
    /// read-only open first checks the column families on disk are exactly
    /// the ones this binary uses (`check_column_families_on_disk`): RocksDB
    /// lets a read-only open leave some out (and some RocksDB 11 releases
    /// before 11.6 deleted their files when such an open closed), and a
    /// column family opened without its merge operator fails reads of values
    /// not yet merged.
    fn open_db(path: &str, read_only: bool) -> Result<(DB, Arc<AtomicU32>), StoreError> {
        let (column_families, script_history_gc_threshold) = Self::column_family_descriptors();
        let db = if read_only {
            Self::check_column_families_on_disk(path, &column_families)?;
            DB::open_cf_descriptors_read_only(&Self::db_options(), path, column_families, false)?
        } else {
            DB::open_cf_descriptors(&Self::db_options(), path, column_families)?
        };
        Ok((db, script_history_gc_threshold))
    }

    /// Check the column families on disk at `path` are exactly `known`.
    ///
    /// Refuses one this binary does not know, which a newer or a different
    /// build wrote, and a missing one, which a writable open would create.
    /// Fails if `path` holds no store.
    fn check_column_families_on_disk(
        path: &str,
        known: &[ColumnFamilyDescriptor],
    ) -> Result<(), StoreError> {
        let on_disk = DB::list_cf(&RocksDbOptions::default(), path)?;
        let unknown = unknown_column_families(&on_disk, known);
        if !unknown.is_empty() {
            return Err(StoreError::ColumnFamilyMismatch(format!(
                "store has column families {} that this binary does not know; it was written \
                 by a newer or a different P2Poolv2 build, use a binary for that store",
                unknown.join(", ")
            )));
        }
        let missing: Vec<&str> = known
            .iter()
            .map(|descriptor| descriptor.name())
            .filter(|name| !on_disk.iter().any(|on_disk_name| on_disk_name == name))
            .collect();
        if !missing.is_empty() {
            return Err(StoreError::ColumnFamilyMismatch(format!(
                "store lacks column families {}; start the node once to create them",
                missing.join(", ")
            )));
        }
        Ok(())
    }

    /// The database options for every open: create the store and any missing
    /// column family when opened writable.
    fn db_options() -> RocksDbOptions {
        let mut db_options = RocksDbOptions::default();
        db_options.create_missing_column_families(true);
        db_options.create_if_missing(true);
        db_options
    }

    /// Every column family this binary uses, with its options. Also returns
    /// the script history GC threshold its compaction filter reads.
    fn column_family_descriptors() -> (Vec<ColumnFamilyDescriptor>, Arc<AtomicU32>) {
        // Point-lookup CFs get bloom filters to speed up existence checks
        // and reject negative lookups without disk I/O.
        // Merge/iterator CFs (BlockIndex, BlockHeight, TxidsBlocks, Uncles)
        // skip bloom filters since they primarily use range scans or merges.
        let block_metadata_cf_descriptor =
            ColumnFamilyDescriptor::new(ColumnFamily::BlockMetadata, Self::cf_options_with_bloom());
        let block_txids_cf =
            ColumnFamilyDescriptor::new(ColumnFamily::BlockTxids, RocksDbOptions::default());
        let inputs_cf =
            ColumnFamilyDescriptor::new(ColumnFamily::Inputs, Self::cf_options_with_bloom());
        let outputs_cf =
            ColumnFamilyDescriptor::new(ColumnFamily::Outputs, Self::cf_options_with_bloom());
        let tx_cf = ColumnFamilyDescriptor::new(ColumnFamily::Tx, Self::cf_options_with_bloom());

        // Configure BlockIndex column family with merge operator for efficient appends.
        // Bloom filter helps point lookups in get_children_blockhashes.
        let mut block_index_opts = Self::cf_options_with_bloom();
        block_index_opts
            .set_merge_operator_associative("blockhash_list_merge", blockhash_list_merge);
        let block_index_cf =
            ColumnFamilyDescriptor::new(ColumnFamily::BlockIndex, block_index_opts);

        // Configure BlockHeight column family with merge operator for efficient appends.
        // Bloom filter helps point lookups for TOP_CANDIDATE/TOP_CONFIRMED keys.
        let mut block_height_opts = Self::cf_options_with_bloom();
        block_height_opts
            .set_merge_operator_associative("blockhash_list_merge", blockhash_list_merge);
        let block_height_cf =
            ColumnFamilyDescriptor::new(ColumnFamily::BlockHeight, block_height_opts);

        // Configure TxidsBlocks column family with merge operator for efficient appends
        // Each txid can be in multiple blocks - only one confirmed,
        // but other valid PoW blocks could have the txids
        let mut txids_blocks_opts = RocksDbOptions::default();
        txids_blocks_opts
            .set_merge_operator_associative("blockhash_list_merge", blockhash_list_merge);
        let txids_blocks_cf =
            ColumnFamilyDescriptor::new(ColumnFamily::TxidsBlocks, txids_blocks_opts);

        // Configure Uncles column family with merge operator for efficient appends.
        // Each Uncle can be included by multiple nephews.
        let mut uncles_opts = RocksDbOptions::default();
        uncles_opts.set_merge_operator_associative("blockhash_list_merge", blockhash_list_merge);
        let uncles_cf = ColumnFamilyDescriptor::new(ColumnFamily::Uncles, uncles_opts);

        let bitcoin_txids_cf =
            ColumnFamilyDescriptor::new(ColumnFamily::BitcoinTxids, RocksDbOptions::default());

        let share_cf = ColumnFamilyDescriptor::new(ColumnFamily::Share, RocksDbOptions::default());
        let user_cf =
            ColumnFamilyDescriptor::new(ColumnFamily::User, Self::cf_options_with_bloom());
        let user_index_cf =
            ColumnFamilyDescriptor::new(ColumnFamily::UserIndex, RocksDbOptions::default());
        let metadata_cf =
            ColumnFamilyDescriptor::new(ColumnFamily::Metadata, RocksDbOptions::default());

        let spends_index_cf =
            ColumnFamilyDescriptor::new(ColumnFamily::SpendsIndex, Self::cf_options_with_bloom());

        let header_cf =
            ColumnFamilyDescriptor::new(ColumnFamily::Header, Self::cf_options_with_bloom());

        let template_merkle_branches_cf = ColumnFamilyDescriptor::new(
            ColumnFamily::TemplateMerkleBranches,
            RocksDbOptions::default(),
        );

        let bitcoin_coinbase_cf =
            ColumnFamilyDescriptor::new(ColumnFamily::BitcoinCoinbase, RocksDbOptions::default());
        let found_blocks_cf =
            ColumnFamilyDescriptor::new(ColumnFamily::FoundBlocks, RocksDbOptions::default());
        let hashrate_samples_cf =
            ColumnFamilyDescriptor::new(ColumnFamily::HashrateSamples, RocksDbOptions::default());
        let accepted_found_blocks_cf = ColumnFamilyDescriptor::new(
            ColumnFamily::AcceptedFoundBlocks,
            RocksDbOptions::default(),
        );
        let accepted_block_effort_cf = ColumnFamilyDescriptor::new(
            ColumnFamily::AcceptedBlockEffort,
            RocksDbOptions::default(),
        );
        let payout_trees_cf =
            ColumnFamilyDescriptor::new(ColumnFamily::PayoutTrees, RocksDbOptions::default());
        let bch_headers_cf =
            ColumnFamilyDescriptor::new(ColumnFamily::BchHeaders, Self::cf_options_with_bloom());
        let bch_best_chain_cf =
            ColumnFamilyDescriptor::new(ColumnFamily::BchBestChain, RocksDbOptions::default());
        let weak_shares_cf =
            ColumnFamilyDescriptor::new(ColumnFamily::WeakShares, RocksDbOptions::default());
        let weak_summaries_cf =
            ColumnFamilyDescriptor::new(ColumnFamily::WeakSummaries, RocksDbOptions::default());

        let script_history_gc_threshold = Arc::new(AtomicU32::new(0));
        let script_history_cf = ColumnFamilyDescriptor::new(
            ColumnFamily::ScriptHistory,
            script_history_cf_options(Arc::clone(&script_history_gc_threshold)),
        );

        let column_families = vec![
            block_metadata_cf_descriptor,
            block_txids_cf,
            txids_blocks_cf,
            uncles_cf,
            inputs_cf,
            outputs_cf,
            tx_cf,
            block_index_cf,
            block_height_cf,
            bitcoin_txids_cf,
            share_cf,
            user_cf,
            user_index_cf,
            metadata_cf,
            spends_index_cf,
            header_cf,
            template_merkle_branches_cf,
            bitcoin_coinbase_cf,
            found_blocks_cf,
            hashrate_samples_cf,
            accepted_found_blocks_cf,
            accepted_block_effort_cf,
            payout_trees_cf,
            bch_headers_cf,
            bch_best_chain_cf,
            weak_shares_cf,
            weak_summaries_cf,
            script_history_cf,
        ];
        (column_families, script_history_gc_threshold)
    }

    /// Size the prune depth, prune target and reorg depth limit by `chain`'s
    /// own depths ([`ChainDepths::for_chain`]). A new store uses Bitcoin's;
    /// a Bitcoin Cash node sets its own before anything reads them.
    pub fn with_chain(mut self, chain: crate::config::Chain) -> Self {
        self.depths = ChainDepths::for_chain(chain);
        self
    }

    /// The share-chain depths this store prunes and reorgs by.
    pub fn depths(&self) -> ChainDepths {
        self.depths
    }

    /// Keep whole weak shares until their share's body is pruned, two payout
    /// depths below the tip, when `keep` is set (`store.keep_whole_weak_shares`,
    /// for an explorer or an archive). By default they are pruned one payout
    /// depth below the confirmed tip, and their leaf root and summary stay with
    /// the body (`Store::weak_share_prune_target`). Either way the node serves
    /// every body a syncing peer asks for.
    pub fn with_whole_weak_shares_kept(mut self, keep: bool) -> Self {
        self.keep_whole_weak_shares = keep;
        self
    }

    /// The chain a stored share chain belongs to, read from its genesis
    /// share's owner: a Bitcoin Cash genesis pays a P2PKH share owner, a
    /// Bitcoin one a taproot program. `None` for a store without a genesis.
    /// For tools that open a store without a node config.
    pub fn stored_chain(&self) -> Option<crate::config::Chain> {
        let genesis = self.get_confirmed_at_height(0).ok()?;
        let header = self.get_share_header(&genesis).ok()??;
        Some(match header.miner_address {
            p2poolv2_wallet::share_owner::ShareOwner::CashP2pkh(_) => {
                crate::config::Chain::BitcoinCash
            }
            p2poolv2_wallet::share_owner::ShareOwner::Taproot(_) => crate::config::Chain::Bitcoin,
        })
    }

    /// Get a rocksb write batch
    /// An associated function as batch is not obtained from db
    pub fn get_write_batch() -> rocksdb::WriteBatch {
        rocksdb::WriteBatch::default()
    }

    /// Commit a write batch earlier obtained using get batch
    pub fn commit_batch(&self, batch: rocksdb::WriteBatch) -> Result<(), rocksdb::Error> {
        self.db.write(batch)
    }

    /// Get all blockhashes from locator height up to top confirmed
    /// height.
    ///
    /// Walks the height index and collects all valid blocks at each
    /// height (confirmed, candidate, header-valid, block-valid).
    /// Pending and invalid blocks are excluded. Within each height,
    /// blocks are sorted lexicographically by blockhash for
    /// deterministic ordering. Heights are never split across batches
    /// -- all blocks at a height are included atomically.
    ///
    /// Starts MAX_UNCLES_DEPTH heights before the locator match so
    /// that fork blocks near the boundary have their parents included
    /// in the batch. The overlap is small and duplicate headers are
    /// handled as no-ops by organise_header on the receiver. This
    /// prevents in the common case starting to fetch from a locator
    /// much deeper down.
    ///
    /// This produces a topologically sorted DAG subgraph because every
    /// block's parent is at height H-1, which is either in this batch
    /// or was sent in a previous batch.
    ///
    /// Returns an empty list when the store has no confirmed chain yet, since
    /// there is nothing to serve. Any other store error is returned, so a
    /// requester never mistakes a storage failure for the end of the chain.
    fn get_descendant_blockhashes(
        &self,
        blockhash: &BlockHash,
        stop_blockhash: &BlockHash,
        limit: usize,
    ) -> Result<Vec<BlockHash>, StoreError> {
        let mut blockhashes = Vec::with_capacity(limit);

        let locator_height = match self.get_block_metadata(blockhash) {
            Ok(metadata) => metadata.expected_height.unwrap_or(0),
            Err(_) => 0,
        };
        let start_height = locator_height.saturating_sub(MAX_UNCLES_DEPTH as u32) + 1;

        let top_confirmed_height = match self.get_top_confirmed_height() {
            Ok(height) => height,
            Err(StoreError::NotFound(_)) => return Ok(blockhashes),
            Err(error) => return Err(error),
        };

        let end_height = std::cmp::min(
            start_height.saturating_add(limit as u32),
            top_confirmed_height,
        );
        let height_entries = self.get_blockhashes_for_height_range(start_height, end_height);

        for (_height, raw_hashes) in height_entries {
            let mut hashes_at_height = self.servable_blockhashes_sorted(&raw_hashes)?;
            hashes_at_height.truncate(MAX_BLOCKS_PER_HEIGHT);

            let found_stop = hashes_at_height.contains(stop_blockhash);
            blockhashes.extend(hashes_at_height);

            if found_stop || blockhashes.len() >= limit {
                return Ok(blockhashes);
            }
        }

        Ok(blockhashes)
    }

    /// Serve every servable block from `start_height` upward, for protocol
    /// 1.1.0 header sync.
    ///
    /// Walks heights in ascending order and, within a height, blockhashes in
    /// ascending order -- the same order `get_descendant_blockhashes` uses, but
    /// with no per-height cap, so a block on the chain can never be left out
    /// of a dense height. Pending and invalid blocks are excluded.
    ///
    /// `resume_after` skips the blocks at `start_height` up to and including
    /// that hash, continuing a previous page from inside a height. The walk
    /// stops after exactly `limit` blocks, even mid-height, so a response never
    /// overshoots; the next request resumes after the last block served. It
    /// also stops after `stop_blockhash` and at the top confirmed height.
    ///
    /// Every block's parent is at the height below, either earlier in this
    /// page or served by a previous one, so pages arrive topologically sorted.
    ///
    /// Returns an empty list when the store has no confirmed chain yet, since
    /// there is nothing to serve. Any other store error is returned, so a
    /// requester never mistakes a storage failure for the end of the chain.
    fn get_blockhashes_from_height(
        &self,
        start_height: u32,
        resume_after: Option<BlockHash>,
        stop_blockhash: &BlockHash,
        limit: usize,
    ) -> Result<Vec<BlockHash>, StoreError> {
        let mut blockhashes = Vec::with_capacity(limit);
        if limit == 0 {
            return Ok(blockhashes);
        }

        let top_confirmed_height = match self.get_top_confirmed_height() {
            Ok(height) => height,
            Err(StoreError::NotFound(_)) => return Ok(blockhashes),
            Err(error) => return Err(error),
        };

        // Every height up to the confirmed tip holds at least its confirmed
        // block, so `limit` heights always hold at least `limit` blocks.
        let end_height = std::cmp::min(
            start_height.saturating_add(limit as u32),
            top_confirmed_height,
        );
        let height_entries = self.get_blockhashes_for_height_range(start_height, end_height);

        for (height, raw_hashes) in height_entries {
            let hashes_at_height = self.servable_blockhashes_sorted(&raw_hashes)?;
            //* Hashes are sorted, so the resume point is found by binary
            //* search: serving starts at the first hash after it.
            let first_unserved = match resume_after {
                Some(resume_hash) if height == start_height => {
                    hashes_at_height.partition_point(|hash| *hash <= resume_hash)
                }
                _ => 0,
            };

            //* Serve hashes in order, ending the page once it is full or the
            //* stop hash has been served.
            for blockhash in &hashes_at_height[first_unserved..] {
                blockhashes.push(*blockhash);
                if blockhashes.len() == limit || blockhash == stop_blockhash {
                    return Ok(blockhashes);
                }
            }
        }

        Ok(blockhashes)
    }

    /// The blocks among `raw_hashes` a header sync server may serve -- not
    /// `Pending` and not `Invalid` -- sorted ascending by blockhash, the order
    /// both serving walks use within a height.
    fn servable_blockhashes_sorted(
        &self,
        raw_hashes: &[BlockHash],
    ) -> Result<Vec<BlockHash>, StoreError> {
        let mut servable: Vec<BlockHash> = self
            .get_block_metadata_batch(raw_hashes)?
            .into_iter()
            .filter(|(_, metadata)| {
                metadata.status != Status::Pending && metadata.status != Status::Invalid
            })
            .map(|(hash, _)| hash)
            .collect();
        servable.sort();
        Ok(servable)
    }

    /// Get genesis block hash from chain state
    pub fn get_genesis_blockhash(&self) -> Option<BlockHash> {
        *self.genesis_blockhash.read().unwrap()
    }

    /// Set genesis block hash in chain state
    pub fn set_genesis_blockhash(&self, hash: BlockHash) {
        *self.genesis_blockhash.write().unwrap() = Some(hash);
    }

    /// Get chain tip from the confirmed chain index.
    ///
    /// Returns the blockhash at the top confirmed height.
    pub fn get_chain_tip(&self) -> Result<BlockHash, StoreError> {
        let height = self.get_top_confirmed_height()?;
        self.get_confirmed_at_height(height)
    }

    /// Get total work of the confirmed chain tip
    pub fn get_total_work(&self) -> Result<Work, StoreError> {
        let tip = self.get_chain_tip()?;
        let metadata = self.get_block_metadata(&tip)?;
        Ok(metadata.chain_work)
    }

    /// Setup genesis block for the store, including the script history of
    /// its coinbase.
    /// Returns an error, if store already has even a single block
    pub fn setup_genesis(
        &self,
        genesis: &ShareBlock,
        batch: &mut rocksdb::WriteBatch,
    ) -> Result<(), StoreError> {
        let blockhash = genesis.block_hash();
        let genesis_work = genesis.header.get_work();
        self.add_share_header(&genesis.header, batch)?;
        self.add_share_block(genesis, batch)?;

        self.set_height_to_blockhash(&blockhash, 0, batch)?;
        let mut metadata = BlockMetadata {
            expected_height: Some(0),
            chain_work: genesis_work,
            status: Status::HeaderValid,
            chain: ChainMembership::None,
        };
        self.update_block_metadata(&blockhash, &metadata, batch)?;
        let metadata_cf = self.db.cf_handle(&ColumnFamily::Metadata).unwrap();
        batch.put_cf(
            &metadata_cf,
            STORE_FORMAT_KEY,
            STORE_FORMAT_VERSION.to_le_bytes(),
        );

        *self.genesis_blockhash.write().unwrap() = Some(blockhash);

        // Genesis is coinbase-only. No need to make sure to call
        // `add_spends_for_block`.
        assert_eq!(
            genesis.transactions.iter().len(),
            1,
            "Genesis should only have one transaction, the coinbase."
        );
        self.append_to_confirmed(&blockhash, 0, &mut metadata, batch)?;
        // `put_confirmed_entry` skips the history of a block whose body is
        // not yet committed, and the genesis body is in this batch. Its
        // coinbase has no prevouts, so its history is written directly.
        self.add_script_history_for_block(0, &genesis.transactions, batch)?;
        Ok(())
    }

    /// Refuse a store written in another format than this build's.
    fn check_store_format(&self) -> Result<(), StoreError> {
        let metadata_cf = self.db.cf_handle(&ColumnFamily::Metadata).unwrap();
        let stored = self
            .db
            .get_cf(&metadata_cf, STORE_FORMAT_KEY)
            .map_err(|error| StoreError::Database(error.to_string()))?
            .map(|bytes| <[u8; 4]>::try_from(bytes.as_slice()).map(u32::from_le_bytes));
        let found = match stored {
            Some(Ok(version)) if version == STORE_FORMAT_VERSION => return Ok(()),
            Some(Ok(version)) => format!("format {version}"),
            Some(Err(_)) => "an unreadable format version".to_string(),
            None => "no format version (it was written before 2026-10-30)".to_string(),
        };
        Err(StoreError::OutdatedStore(format!(
            "the share-chain store at {} has {found}, and this node writes format \
             {STORE_FORMAT_VERSION}, where chain work counts uncle work, Bitcoin Cash pays \
             the genesis share only as the last resort, and Bitcoin Cash has one share chain \
             (the main and mini chains merged) whose headers carry a difficulty band, a \
             payout level and weak shares, with payees below their level paid by a draw and \
             every proof from a job at the minimum paid at the weak work, whose weak root \
             commits each carried weak share's whole encoding and their summary, whose weak \
             ratio follows the weak shares that ask for an easier weak target, and where each \
             known Bitcoin Cash header carries its ticks and next block interval (CHIP-2025-03). \
             Delete the store directory and restart to resync the share chain from peers.",
            self.path
        )))
    }

    /// Initialise chain state from existing data in the store.
    /// Sets the genesis blockhash so chain tip and total work can be read from the confirmed chain index.
    ///
    /// Returns `StoreError::OutdatedStore` when the store was written in
    /// another format (`STORE_FORMAT_VERSION`).
    pub fn init_chain_state_from_store(&self, genesis_hash: BlockHash) -> Result<(), StoreError> {
        self.check_store_format()?;
        self.set_genesis_blockhash(genesis_hash);
        debug!(
            "Initialised chain state: tip={}, height={}, work={}",
            self.get_chain_tip()?,
            self.get_top_confirmed_height()?,
            self.get_total_work()?,
        );
        Ok(())
    }

    /// Record one hashrate sample under its big-endian timestamp key.
    ///
    /// Same timestamp overwrites: the ring carries at most one sample per
    /// minute and a replayed timestamp must not duplicate.
    pub(crate) fn add_hashrate_sample(&self, sample: &HashrateSample) -> Result<(), StoreError> {
        let column_family = self
            .db
            .cf_handle(&ColumnFamily::HashrateSamples)
            .expect("hashrate_samples CF registered on read-write open");
        self.db
            .put_cf(&column_family, sample.key(), encode::serialize(sample))?;
        Ok(())
    }

    /// Delete every hashrate sample with a timestamp strictly before
    /// `before_secs`, one range delete.
    pub fn evict_hashrate_samples_before(&self, before_secs: u64) -> Result<(), StoreError> {
        let column_family = self
            .db
            .cf_handle(&ColumnFamily::HashrateSamples)
            .expect("hashrate_samples CF registered on read-write open");
        let start = HashrateSample::key_at(0);
        let end = HashrateSample::key_at(before_secs);
        self.db.delete_range_cf(&column_family, start, end)?;
        Ok(())
    }

    /// Load hashrate samples with timestamps at or after `since_secs`,
    /// oldest first. A corrupt or future-dated entry is skipped with a
    /// warning rather than failing startup: one bad row must not take the
    /// dashboard down.
    pub fn load_hashrate_samples(&self, since_secs: u64) -> Vec<HashrateSample> {
        let Some(column_family) = self.db.cf_handle(&ColumnFamily::HashrateSamples) else {
            warn!("hashrate_samples CF absent; dashboard history starts empty");
            return Vec::new();
        };
        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.fill_cache(false);
        read_opts.set_iterate_lower_bound(HashrateSample::key_at(since_secs));
        let mode =
            rocksdb::IteratorMode::From(&HashrateSample::key_at(0), rocksdb::Direction::Forward);
        let mut samples = Vec::with_capacity(43_200);
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or_default();
        // Two sample intervals of clock skew: a sample stamped further into
        // the future is bogus and is dropped instead of pinning the chart.
        let future_cutoff = now_secs.saturating_add(120);
        for item in self.db.iterator_cf_opt(&column_family, read_opts, mode) {
            let Ok((key, value)) = item else {
                warn!("Unreadable hashrate sample entry; skipping it");
                continue;
            };
            match encode::deserialize::<HashrateSample>(&value) {
                Ok(sample) => {
                    if sample.time > future_cutoff {
                        warn!("Dropping future-dated hashrate sample at {}", sample.time);
                    } else if u64::from_be_bytes([
                        key[0], key[1], key[2], key[3], key[4], key[5], key[6], key[7],
                    ]) != sample.time
                    {
                        warn!("Hashrate sample key/value timestamp mismatch; skipping it");
                    } else {
                        samples.push(sample);
                    }
                }
                Err(error) => warn!("Corrupt hashrate sample at {key:?}: {error}; skipping it"),
            }
        }
        samples
    }
}

#[cfg(test)]
impl Store {
    /// Organise a share header into the candidate chain.
    ///
    /// Computes height and chain_work from parent metadata, creates
    /// BlockMetadata, and updates the candidate chain.
    /// Returns the new candidate height, or None.
    pub fn push_to_candidate_chain(&self, share: &ShareBlock) -> Result<Option<u32>, StoreError> {
        let mut batch = Store::get_write_batch();
        let result = self.organise_header(&share.header, &mut batch)?;
        self.commit_batch(batch)?;
        Ok(result)
    }

    /// Push a share to the confirmed chain: organise header, store
    /// the full block, then promote candidates to confirmed.
    /// Returns the new confirmed height if changed, or None.
    ///
    /// In-PPLNS-zone promotion requires the candidate prefix to be
    /// BlockValid, so this marks the whole candidate chain validated before
    /// promoting -- simulating candidates that have completed chain-context
    /// validation.
    pub fn push_to_confirmed_chain(&self, share: &ShareBlock) -> Result<Option<u32>, StoreError> {
        self.push_to_candidate_chain(share)?;
        let mut batch = Store::get_write_batch();
        self.add_share_block(share, &mut batch)?;
        self.commit_batch(batch)?;
        self.mark_candidate_chain_block_valid();
        let mut batch = Store::get_write_batch();
        let result = self.organise_block(&mut batch)?;
        self.commit_batch(batch)?;
        Ok(result)
    }

    /// Mark every block currently on the candidate chain BlockValid.
    ///
    /// Test helper for promotion scenarios: in-PPLNS-zone confirmation
    /// requires `is_candidate_and_block_valid`, so tests that build a
    /// candidate chain and expect it confirmed must first mark it validated.
    /// Already-BlockValid entries are left unchanged.
    pub fn mark_candidate_chain_block_valid(&self) {
        let top = match self.get_top_candidate_height() {
            Ok(top) => top,
            Err(_) => return,
        };
        let mut batch = Store::get_write_batch();
        for height in 1..=top {
            if let Ok(hash) = self.get_candidate_at_height(height) {
                let _ = self.mark_block_valid(&hash, &mut batch);
            }
        }
        self.commit_batch(batch).unwrap();
    }

    /// Store a share block and create Valid metadata for it.
    ///
    /// Used in tests for shares that need to be discoverable by
    /// forward walks and uncle lookups. The metadata height and
    /// chain_work are computed from the parent, with chain_work adding
    /// uncle work as `initialise_new_header` does. Panics if the parent
    /// or an uncle header is not in the store -- missing data in test
    /// setup is a bug. Does NOT go through organise_header, so it avoids
    /// candidate chain side effects.
    pub fn store_with_valid_metadata(&self, share: &ShareBlock) {
        let blockhash = share.block_hash();
        let share_work = share.header.get_work();
        let parent_metadata = self
            .get_block_metadata(&share.header.prev_share_blockhash)
            .unwrap_or_else(|_| {
                panic!(
                    "Parent {} not found for block {blockhash} in test setup",
                    share.header.prev_share_blockhash
                )
            });
        let parent_height = parent_metadata.expected_height.unwrap_or_default();
        let height = parent_height + 1;
        let uncles_work = self.sum_uncles_work(&share.header, &blockhash).unwrap();
        let chain_work = parent_metadata.chain_work + share_work + uncles_work;
        let mut batch = Store::get_write_batch();
        self.add_share_block(share, &mut batch).unwrap();
        self.set_height_to_blockhash(&blockhash, height, &mut batch)
            .unwrap();
        let metadata = BlockMetadata {
            expected_height: Some(height),
            chain_work,
            status: Status::HeaderValid,
            chain: ChainMembership::None,
        };
        self.update_block_metadata(&blockhash, &metadata, &mut batch)
            .unwrap();
        self.commit_batch(batch).unwrap();
    }

    /// `store_with_valid_metadata`, then mark the share `BlockValid`, as
    /// full validation does: for shares a test needs selectable as uncles
    /// (`find_uncles` takes only fully validated shares).
    pub fn store_with_block_valid_metadata(&self, share: &ShareBlock) {
        self.store_with_valid_metadata(share);
        let mut batch = Store::get_write_batch();
        self.mark_block_valid(&share.block_hash(), &mut batch)
            .unwrap();
        self.commit_batch(batch).unwrap();
    }

    /// Create Valid metadata for a share without storing its block data.
    ///
    /// Also stores the header in the Header CF so that downstream
    /// children can look up parent timestamps and heights. chain_work
    /// adds uncle work as `initialise_new_header` does. Panics if the
    /// parent or an uncle header is not in the store -- missing data in
    /// test setup is a bug.
    pub fn create_valid_metadata_only(&self, share: &ShareBlock) {
        let blockhash = share.block_hash();
        let share_work = share.header.get_work();
        let parent_metadata = self
            .get_block_metadata(&share.header.prev_share_blockhash)
            .unwrap_or_else(|_| {
                panic!(
                    "Parent {} not found for block {blockhash} in test setup",
                    share.header.prev_share_blockhash
                )
            });
        let parent_height = parent_metadata.expected_height.unwrap_or_default();
        let height = parent_height + 1;
        let uncles_work = self.sum_uncles_work(&share.header, &blockhash).unwrap();
        let chain_work = parent_metadata.chain_work + share_work + uncles_work;
        let mut batch = Store::get_write_batch();
        self.add_share_header(&share.header, &mut batch).unwrap();
        self.set_height_to_blockhash(&blockhash, height, &mut batch)
            .unwrap();
        let metadata = BlockMetadata {
            expected_height: Some(height),
            chain_work,
            status: Status::HeaderValid,
            chain: ChainMembership::None,
        };
        self.update_block_metadata(&blockhash, &metadata, &mut batch)
            .unwrap();
        self.commit_batch(batch).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::TestShareBlockBuilder;
    use crate::test_utils::multiplied_compact_target_as_work;
    use bitcoin::consensus;
    use bitcoin::hashes::Hash;
    use tempfile::tempdir;

    /// A store set up by this build reopens: `setup_genesis` writes the
    /// store format version that `init_chain_state_from_store` checks.
    #[test]
    fn test_store_from_this_build_reopens() {
        let temp_dir = tempdir().unwrap();
        let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
        let genesis = TestShareBlockBuilder::new().build();
        let mut batch = Store::get_write_batch();
        store.setup_genesis(&genesis, &mut batch).unwrap();
        store.commit_batch(batch).unwrap();

        store
            .init_chain_state_from_store(genesis.block_hash())
            .unwrap();
    }

    /// A store written before chain work counted uncle work has no format
    /// version. Startup refuses it and tells the operator to resync.
    #[test]
    fn test_store_without_format_version_is_refused() {
        let temp_dir = tempdir().unwrap();
        let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
        let genesis = TestShareBlockBuilder::new().build();
        let mut batch = Store::get_write_batch();
        store.setup_genesis(&genesis, &mut batch).unwrap();
        store.commit_batch(batch).unwrap();
        let metadata_cf = store.db.cf_handle(&ColumnFamily::Metadata).unwrap();
        store.db.delete_cf(&metadata_cf, STORE_FORMAT_KEY).unwrap();

        let error = store
            .init_chain_state_from_store(genesis.block_hash())
            .unwrap_err();
        assert!(matches!(error, StoreError::OutdatedStore(_)), "{error:?}");
        assert!(error.to_string().contains("Delete the store directory"));
    }

    /// A store from another format version is refused too.
    #[test]
    fn test_store_with_another_format_version_is_refused() {
        let temp_dir = tempdir().unwrap();
        let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
        let genesis = TestShareBlockBuilder::new().build();
        let mut batch = Store::get_write_batch();
        store.setup_genesis(&genesis, &mut batch).unwrap();
        store.commit_batch(batch).unwrap();
        let metadata_cf = store.db.cf_handle(&ColumnFamily::Metadata).unwrap();
        store
            .db
            .put_cf(
                &metadata_cf,
                STORE_FORMAT_KEY,
                (STORE_FORMAT_VERSION + 1).to_le_bytes(),
            )
            .unwrap();

        let error = store
            .init_chain_state_from_store(genesis.block_hash())
            .unwrap_err();
        let expected = format!("has format {}", STORE_FORMAT_VERSION + 1);
        assert!(error.to_string().contains(&expected), "{error}");
    }

    /// A store from before the genesis last-resort rule, format 1, is
    /// refused: its chain's first hours were validated under the old rule.
    #[test]
    fn test_store_from_before_genesis_last_resort_is_refused() {
        let temp_dir = tempdir().unwrap();
        let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
        let genesis = TestShareBlockBuilder::new().build();
        let mut batch = Store::get_write_batch();
        store.setup_genesis(&genesis, &mut batch).unwrap();
        store.commit_batch(batch).unwrap();
        let metadata_cf = store.db.cf_handle(&ColumnFamily::Metadata).unwrap();
        store
            .db
            .put_cf(&metadata_cf, STORE_FORMAT_KEY, 1u32.to_le_bytes())
            .unwrap();

        let error = store
            .init_chain_state_from_store(genesis.block_hash())
            .unwrap_err();
        assert!(matches!(error, StoreError::OutdatedStore(_)), "{error:?}");
        assert!(error.to_string().contains("has format 1"), "{error}");
    }

    /// A store from before the one share chain and its band, format 2, is
    /// refused: its headers carry no band, and its chain is another genesis's.
    #[test]
    fn test_store_from_before_the_band_is_refused() {
        let temp_dir = tempdir().unwrap();
        let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
        let genesis = TestShareBlockBuilder::new().build();
        let mut batch = Store::get_write_batch();
        store.setup_genesis(&genesis, &mut batch).unwrap();
        store.commit_batch(batch).unwrap();
        let metadata_cf = store.db.cf_handle(&ColumnFamily::Metadata).unwrap();
        store
            .db
            .put_cf(&metadata_cf, STORE_FORMAT_KEY, 2u32.to_le_bytes())
            .unwrap();

        let error = store
            .init_chain_state_from_store(genesis.block_hash())
            .unwrap_err();
        assert!(matches!(error, StoreError::OutdatedStore(_)), "{error:?}");
        assert!(error.to_string().contains("has format 2"), "{error}");
        assert!(error.to_string().contains("difficulty band"), "{error}");
    }

    /// A store from before payout levels, format 3, is refused: its headers
    /// carry no payout level, and its coinbases were validated without the
    /// draw.
    #[test]
    fn test_store_from_before_payout_levels_is_refused() {
        let temp_dir = tempdir().unwrap();
        let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
        let genesis = TestShareBlockBuilder::new().build();
        let mut batch = Store::get_write_batch();
        store.setup_genesis(&genesis, &mut batch).unwrap();
        store.commit_batch(batch).unwrap();
        let metadata_cf = store.db.cf_handle(&ColumnFamily::Metadata).unwrap();
        store
            .db
            .put_cf(&metadata_cf, STORE_FORMAT_KEY, 3u32.to_le_bytes())
            .unwrap();

        let error = store
            .init_chain_state_from_store(genesis.block_hash())
            .unwrap_err();
        assert!(matches!(error, StoreError::OutdatedStore(_)), "{error:?}");
        assert!(error.to_string().contains("has format 3"), "{error}");
        assert!(error.to_string().contains("payout level"), "{error}");
    }

    /// A store from before weak shares, format 4, is refused: its headers
    /// carry no weak ratio, and its shares were credited at their own work.
    #[test]
    fn test_store_from_before_weak_shares_is_refused() {
        let temp_dir = tempdir().unwrap();
        let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
        let genesis = TestShareBlockBuilder::new().build();
        let mut batch = Store::get_write_batch();
        store.setup_genesis(&genesis, &mut batch).unwrap();
        store.commit_batch(batch).unwrap();
        let metadata_cf = store.db.cf_handle(&ColumnFamily::Metadata).unwrap();
        store
            .db
            .put_cf(&metadata_cf, STORE_FORMAT_KEY, 4u32.to_le_bytes())
            .unwrap();

        let error = store
            .init_chain_state_from_store(genesis.block_hash())
            .unwrap_err();
        assert!(matches!(error, StoreError::OutdatedStore(_)), "{error:?}");
        assert!(error.to_string().contains("has format 4"), "{error}");
        assert!(error.to_string().contains("weak work"), "{error}");
    }

    /// A store from before the weak root bound whole weak shares, format 5,
    /// is refused: its shares carrying weak shares commit the old root.
    #[test]
    fn test_store_from_before_the_whole_weak_share_root_is_refused() {
        let temp_dir = tempdir().unwrap();
        let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
        let genesis = TestShareBlockBuilder::new().build();
        let mut batch = Store::get_write_batch();
        store.setup_genesis(&genesis, &mut batch).unwrap();
        store.commit_batch(batch).unwrap();
        let metadata_cf = store.db.cf_handle(&ColumnFamily::Metadata).unwrap();
        store
            .db
            .put_cf(&metadata_cf, STORE_FORMAT_KEY, 5u32.to_le_bytes())
            .unwrap();

        let error = store
            .init_chain_state_from_store(genesis.block_hash())
            .unwrap_err();
        assert!(matches!(error, StoreError::OutdatedStore(_)), "{error:?}");
        assert!(error.to_string().contains("has format 5"), "{error}");
        assert!(error.to_string().contains("whole encoding"), "{error}");
    }

    /// A store from before the payout-level floor, format 7, is refused: its
    /// chain was validated under the rule that allowed level 0.
    #[test]
    fn test_store_from_before_the_level_floor_is_refused() {
        let temp_dir = tempdir().unwrap();
        let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
        let genesis = TestShareBlockBuilder::new().build();
        let mut batch = Store::get_write_batch();
        store.setup_genesis(&genesis, &mut batch).unwrap();
        store.commit_batch(batch).unwrap();
        let metadata_cf = store.db.cf_handle(&ColumnFamily::Metadata).unwrap();
        store
            .db
            .put_cf(&metadata_cf, STORE_FORMAT_KEY, 7u32.to_le_bytes())
            .unwrap();

        let error = store
            .init_chain_state_from_store(genesis.block_hash())
            .unwrap_err();
        assert!(matches!(error, StoreError::OutdatedStore(_)), "{error:?}");
        assert!(error.to_string().contains("has format 7"), "{error}");
    }

    /// A store from before the weak ratio followed the asks, format 9, is
    /// refused: its headers do not count their asking weak shares.
    #[test]
    fn test_store_from_before_the_weak_asks_is_refused() {
        let temp_dir = tempdir().unwrap();
        let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
        let genesis = TestShareBlockBuilder::new().build();
        let mut batch = Store::get_write_batch();
        store.setup_genesis(&genesis, &mut batch).unwrap();
        store.commit_batch(batch).unwrap();
        let metadata_cf = store.db.cf_handle(&ColumnFamily::Metadata).unwrap();
        store
            .db
            .put_cf(&metadata_cf, STORE_FORMAT_KEY, 9u32.to_le_bytes())
            .unwrap();

        let error = store
            .init_chain_state_from_store(genesis.block_hash())
            .unwrap_err();
        assert!(matches!(error, StoreError::OutdatedStore(_)), "{error:?}");
        assert!(error.to_string().contains("has format 9"), "{error}");
        assert!(
            error.to_string().contains("weak shares that ask"),
            "{error}"
        );
    }

    #[test_log::test]
    fn test_chain_state_management() {
        let temp_dir = tempdir().unwrap();
        let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

        // Create test shares in a linear chain
        let share1 = TestShareBlockBuilder::new().build();

        let share2 = TestShareBlockBuilder::new()
            .prev_share_blockhash(share1.block_hash().to_string())
            .work(2)
            .build();

        let share3 = TestShareBlockBuilder::new()
            .prev_share_blockhash(share2.block_hash().to_string())
            .work(3)
            .build();

        let genesis_hash = share1.block_hash();

        // Setup genesis (share1 confirmed at height 0)
        let mut batch = Store::get_write_batch();
        store.setup_genesis(&share1, &mut batch).unwrap();
        store.commit_batch(batch).unwrap();

        assert_eq!(store.get_genesis_blockhash(), Some(genesis_hash));
        assert_eq!(store.get_chain_tip().unwrap(), genesis_hash);

        // Push share2 to confirmed (extends confirmed to height 1)
        store.push_to_confirmed_chain(&share2).unwrap();

        // Push share3 to confirmed (extends confirmed to height 2)
        store.push_to_confirmed_chain(&share3).unwrap();

        // Test initialization from store
        store.init_chain_state_from_store(genesis_hash).unwrap();

        // After initialization, tip and genesis should be readable from confirmed chain
        assert_eq!(store.get_chain_tip().unwrap(), share3.block_hash());
        assert_eq!(store.get_genesis_blockhash(), Some(genesis_hash));

        // Total work should reflect sum of work from confirmed chain
        assert_eq!(
            store.get_total_work().unwrap(),
            multiplied_compact_target_as_work(0x01e0377ae, 1)
                + multiplied_compact_target_as_work(0x01e0377ae, 2)
                + multiplied_compact_target_as_work(0x01e0377ae, 3)
        );
    }

    #[test]
    fn a_sample_round_trips_through_the_store() {
        let temp_dir = tempdir().unwrap();
        let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
        let original = HashrateSample {
            time: 1_792_672_402,
            pool: Some(1.5),
            local: None,
            network: Some(2.5),
        };
        store.add_hashrate_sample(&original).unwrap();
        let loaded = store.load_hashrate_samples(original.time);
        assert_eq!(loaded, vec![original]);
    }

    #[test]
    fn eviction_drops_only_older_samples() {
        let temp_dir = tempdir().unwrap();
        let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
        store
            .add_hashrate_sample(&HashrateSample {
                time: 1_000,
                pool: Some(1.0),
                local: None,
                network: None,
            })
            .unwrap();
        store
            .add_hashrate_sample(&HashrateSample {
                time: 2_000,
                pool: Some(2.0),
                local: None,
                network: None,
            })
            .unwrap();
        store.evict_hashrate_samples_before(2_000).unwrap();
        let loaded = store.load_hashrate_samples(0);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].time, 2_000);
    }

    #[test]
    fn history_survives_a_store_reopen() {
        let temp_dir = tempdir().unwrap();
        let path = temp_dir.path().to_str().unwrap().to_string();
        let sample = HashrateSample {
            time: 1_792_672_402,
            pool: Some(1.0),
            local: None,
            network: Some(2.0),
        };
        {
            let store = Store::new(path.clone(), false).unwrap();
            store.add_hashrate_sample(&sample).unwrap();
        }
        let reopened = Store::new(path, false).unwrap();
        assert_eq!(reopened.load_hashrate_samples(0), vec![sample]);
    }

    #[test]
    fn a_read_only_open_sees_the_persisted_history() {
        let temp_dir = tempdir().unwrap();
        let path = temp_dir.path().to_str().unwrap().to_string();
        let sample = HashrateSample {
            time: 1_792_672_402,
            pool: Some(1.0),
            local: None,
            network: Some(2.0),
        };
        {
            let store = Store::new(path.clone(), false).unwrap();
            store.add_hashrate_sample(&sample).unwrap();
        }
        // A read-only open opens every column family on disk, the samples
        // too, so read-only tools see the persisted history.
        let read_only = Store::new(path, true).unwrap();
        assert_eq!(read_only.load_hashrate_samples(0), vec![sample]);
    }

    /// A read-only `Store::new` refuses a store with a column family this
    /// binary does not know, saying a newer or a different build wrote it,
    /// and leaves that column family intact.
    #[test]
    fn test_read_only_open_refuses_an_unknown_column_family() {
        let temp_dir = tempdir().unwrap();
        let path = temp_dir.path().to_str().unwrap().to_string();
        drop(Store::new(path.clone(), false).unwrap());

        //* Add a column family this binary does not know, with a row, flushed to disk
        let mut all_column_families = DB::list_cf(&RocksDbOptions::default(), &path).unwrap();
        all_column_families.push("from_a_newer_binary".to_string());
        let mut options = RocksDbOptions::default();
        options.create_missing_column_families(true);
        let db = DB::open_cf(&options, &path, &all_column_families).unwrap();
        let unknown_cf = db.cf_handle("from_a_newer_binary").unwrap();
        db.put_cf(&unknown_cf, b"key", b"value").unwrap();
        db.flush_cf(&unknown_cf).unwrap();
        drop(unknown_cf);
        drop(db);

        let error = Store::new(path.clone(), true).err().unwrap();
        assert!(
            matches!(error, StoreError::ColumnFamilyMismatch(_)),
            "{error}"
        );
        assert!(error.to_string().contains("from_a_newer_binary"), "{error}");
        assert!(error.to_string().contains("newer"), "{error}");

        //* The store still opens, and the unknown column family keeps its row
        let db = DB::open_cf(&RocksDbOptions::default(), &path, &all_column_families).unwrap();
        let unknown_cf = db.cf_handle("from_a_newer_binary").unwrap();
        assert_eq!(
            db.iterator_cf(&unknown_cf, rocksdb::IteratorMode::Start)
                .count(),
            1
        );
    }

    /// A read-only `Store::new` refuses a store missing one of the column
    /// families this binary uses, naming it, instead of the bare message
    /// from RocksDB.
    #[test]
    fn test_read_only_open_refuses_a_store_missing_a_column_family() {
        let temp_dir = tempdir().unwrap();
        let path = temp_dir.path().to_str().unwrap().to_string();
        drop(Store::new(path.clone(), false).unwrap());

        //* Drop a column family this binary uses
        let all_column_families = DB::list_cf(&RocksDbOptions::default(), &path).unwrap();
        let db = DB::open_cf(&RocksDbOptions::default(), &path, &all_column_families).unwrap();
        db.drop_cf(ColumnFamily::FoundBlocks.as_str()).unwrap();
        drop(db);

        let error = Store::new(path, true).err().unwrap();
        assert!(
            matches!(error, StoreError::ColumnFamilyMismatch(_)),
            "{error}"
        );
        assert!(error.to_string().contains("found_blocks"), "{error}");
        assert!(error.to_string().contains("start the node"), "{error}");
    }

    /// A store last opened by a build without one of the later column
    /// families, such as the effort snapshots, is refused read-only until a
    /// writable open, as the node makes at start, has created it.
    #[test]
    fn test_read_only_open_accepts_a_store_once_a_writable_open_created_its_column_families() {
        let temp_dir = tempdir().unwrap();
        let path = temp_dir.path().to_str().unwrap().to_string();
        drop(Store::new(path.clone(), false).unwrap());
        let all_column_families = DB::list_cf(&RocksDbOptions::default(), &path).unwrap();
        let db = DB::open_cf(&RocksDbOptions::default(), &path, &all_column_families).unwrap();
        db.drop_cf(ColumnFamily::AcceptedBlockEffort.as_str())
            .unwrap();
        drop(db);
        assert!(matches!(
            Store::new(path.clone(), true).err(),
            Some(StoreError::ColumnFamilyMismatch(_))
        ));

        drop(Store::new(path.clone(), false).unwrap());

        assert!(Store::new(path, true).is_ok());
    }

    /// A read-only open reads a column family with its merge operator. The
    /// height index appends by merge; opened without the operator, a read
    /// of operands not yet merged fails, and `get_blockhashes_for_height`
    /// would report no blocks at that height.
    #[test]
    fn test_read_only_open_reads_merged_values() {
        let temp_dir = tempdir().unwrap();
        let path = temp_dir.path().to_str().unwrap().to_string();
        let height = 200u32;
        let hash1 = BlockHash::from_byte_array([10u8; 32]);
        let hash2 = BlockHash::from_byte_array([20u8; 32]);
        {
            let store = Store::new(path.clone(), false).unwrap();
            let mut batch = Store::get_write_batch();
            store
                .set_height_to_blockhash(&hash1, height, &mut batch)
                .unwrap();
            store
                .set_height_to_blockhash(&hash2, height, &mut batch)
                .unwrap();
            store.commit_batch(batch).unwrap();
        }

        let read_only = Store::new(path, true).unwrap();
        let hashes = read_only.get_blockhashes_for_height(height);
        assert_eq!(hashes.len(), 2);
        assert!(hashes.contains(&hash1));
        assert!(hashes.contains(&hash2));
    }

    #[test]
    fn test_merge_operator_for_block_height() {
        let temp_dir = tempdir().unwrap();
        let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

        let height = 100u32;

        // Create three different blockhashes
        let hash1 = BlockHash::from_byte_array([1u8; 32]);
        let hash2 = BlockHash::from_byte_array([2u8; 32]);
        let hash3 = BlockHash::from_byte_array([3u8; 32]);

        // Add hash1 at height 100
        let mut batch = Store::get_write_batch();
        store
            .set_height_to_blockhash(&hash1, height, &mut batch)
            .unwrap();
        store.commit_batch(batch).unwrap();

        // Verify hash1 is stored
        let hashes = store.get_blockhashes_for_height(height);
        assert_eq!(hashes.len(), 1);
        assert!(hashes.contains(&hash1));

        // Add hash2 at the same height using merge operator
        let mut batch = Store::get_write_batch();
        store
            .set_height_to_blockhash(&hash2, height, &mut batch)
            .unwrap();
        store.commit_batch(batch).unwrap();

        // Verify both hash1 and hash2 are stored
        let hashes = store.get_blockhashes_for_height(height);
        assert_eq!(hashes.len(), 2);
        assert!(hashes.contains(&hash1));
        assert!(hashes.contains(&hash2));

        // Add hash3 at the same height
        let mut batch = Store::get_write_batch();
        store
            .set_height_to_blockhash(&hash3, height, &mut batch)
            .unwrap();
        store.commit_batch(batch).unwrap();

        // Verify all three hashes are stored
        let hashes = store.get_blockhashes_for_height(height);
        assert_eq!(hashes.len(), 3);
        assert!(hashes.contains(&hash1));
        assert!(hashes.contains(&hash2));
        assert!(hashes.contains(&hash3));

        // Test deduplication - add hash1 again
        let mut batch = Store::get_write_batch();
        store
            .set_height_to_blockhash(&hash1, height, &mut batch)
            .unwrap();
        store.commit_batch(batch).unwrap();

        // Should still have only 3 hashes (no duplicate)
        let hashes = store.get_blockhashes_for_height(height);
        assert_eq!(hashes.len(), 3);
        assert!(hashes.contains(&hash1));
        assert!(hashes.contains(&hash2));
        assert!(hashes.contains(&hash3));
    }

    #[test]
    fn test_merge_operator_multiple_batches() {
        let temp_dir = tempdir().unwrap();
        let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

        let height = 200u32;

        let hash1 = BlockHash::from_byte_array([10u8; 32]);
        let hash2 = BlockHash::from_byte_array([20u8; 32]);
        let hash3 = BlockHash::from_byte_array([30u8; 32]);

        // Add multiple hashes in a single batch
        let mut batch = Store::get_write_batch();
        store
            .set_height_to_blockhash(&hash1, height, &mut batch)
            .unwrap();
        store
            .set_height_to_blockhash(&hash2, height, &mut batch)
            .unwrap();
        store
            .set_height_to_blockhash(&hash3, height, &mut batch)
            .unwrap();
        store.commit_batch(batch).unwrap();

        // Verify all hashes are stored correctly
        let hashes = store.get_blockhashes_for_height(height);
        assert_eq!(hashes.len(), 3);
        assert!(hashes.contains(&hash1));
        assert!(hashes.contains(&hash2));
        assert!(hashes.contains(&hash3));
    }

    /// Tests parse_blockhash_bytes handles both formats correctly.
    ///
    /// In partial merge mode, RocksDB passes raw 32-byte BlockHash
    /// operands. In full merge mode, existing_val is a serialized
    /// Vec<BlockHash>. The parse function must handle both.
    #[test]
    fn test_parse_blockhash_bytes_handles_raw_and_vec_formats() {
        let hash1 = BlockHash::from_byte_array([0xAAu8; 32]);
        let hash2 = BlockHash::from_byte_array([0xBBu8; 32]);

        // Raw 32-byte operand (as seen in partial merge)
        let raw_operand = consensus::serialize(&hash1);
        assert_eq!(raw_operand.len(), 32);
        let parsed = parse_blockhash_bytes(&raw_operand).expect("raw 32-byte operand should parse");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0], hash1);

        // Serialized Vec<BlockHash> (as seen in full merge existing_val)
        let vec_value = consensus::serialize(&vec![hash1, hash2]);
        assert_eq!(vec_value.len(), 65); // compact_size(2) + 32 + 32
        let parsed = parse_blockhash_bytes(&vec_value).expect("serialized Vec should parse");
        assert_eq!(parsed.len(), 2);
        assert!(parsed.contains(&hash1));
        assert!(parsed.contains(&hash2));

        // Hash with 0x00 first byte must NOT be misread as empty Vec
        let mut zero_prefix_bytes = [0x00u8; 32];
        zero_prefix_bytes[31] = 0x42;
        let zero_prefix_hash = BlockHash::from_byte_array(zero_prefix_bytes);
        let raw_zero = consensus::serialize(&zero_prefix_hash);
        let parsed = parse_blockhash_bytes(&raw_zero).expect("0x00-prefixed hash should parse");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0], zero_prefix_hash);

        // Empty input returns None (unrecognised format)
        assert!(parse_blockhash_bytes(&[]).is_none());

        // Garbage input returns None
        assert!(parse_blockhash_bytes(&[0xFF, 0xFF, 0xFF]).is_none());
    }

    /// Tests the merge operator with compaction forcing SST-level merges.
    ///
    /// Writes two hashes at the same height in separate flushes, then
    /// compacts. In a fresh DB compaction reaches the bottommost level
    /// (full merge with existing_val=None), so both hashes survive.
    /// This test verifies the baseline full-merge path works.
    #[test]
    fn test_merge_operator_survives_compaction() {
        let temp_dir = tempdir().unwrap();
        let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

        let height = 3224u32;
        let hash1 = BlockHash::from_byte_array([0xAAu8; 32]);
        let hash2 = BlockHash::from_byte_array([0xBBu8; 32]);

        // Write hash1 in its own batch and flush to an SST file
        let mut batch = Store::get_write_batch();
        store
            .set_height_to_blockhash(&hash1, height, &mut batch)
            .unwrap();
        store.commit_batch(batch).unwrap();
        let block_height_cf = store.db.cf_handle(&ColumnFamily::BlockHeight).unwrap();
        store
            .db
            .flush_cf(&block_height_cf)
            .expect("flush should succeed");

        // Write hash2 in a separate batch and flush to a second SST file
        let mut batch = Store::get_write_batch();
        store
            .set_height_to_blockhash(&hash2, height, &mut batch)
            .unwrap();
        store.commit_batch(batch).unwrap();
        store
            .db
            .flush_cf(&block_height_cf)
            .expect("flush should succeed");

        // Force compaction -- this triggers partial merge on the two
        // merge operands sitting in separate SST files
        store
            .db
            .compact_range_cf(&block_height_cf, None::<&[u8]>, None::<&[u8]>);

        // After compaction, both hashes must still be present
        let hashes = store.get_blockhashes_for_height(height);
        assert_eq!(
            hashes.len(),
            2,
            "Both hashes should survive compaction, got: {hashes:?}"
        );
        assert!(hashes.contains(&hash1), "hash1 lost after compaction");
        assert!(hashes.contains(&hash2), "hash2 lost after compaction");
    }

    /// Verifies that parse_blockhash_bytes correctly handles a
    /// partial-merge result (serialized Vec) appearing as an operand.
    #[test]
    fn test_parse_blockhash_bytes_handles_partial_merge_result_as_operand() {
        let hash1 = BlockHash::from_byte_array([0xCCu8; 32]);
        let hash2 = BlockHash::from_byte_array([0xDDu8; 32]);

        // A partial merge result is a serialized Vec<BlockHash>
        let partial_merge_result = consensus::serialize(&vec![hash1, hash2]);
        assert_eq!(partial_merge_result.len(), 65);

        // parse_blockhash_bytes must extract both hashes
        let parsed = parse_blockhash_bytes(&partial_merge_result)
            .expect("partial merge result should parse");
        assert_eq!(parsed.len(), 2);
        assert!(parsed.contains(&hash1));
        assert!(parsed.contains(&hash2));
    }

    #[test]
    fn test_merge_operator_different_heights() {
        let temp_dir = tempdir().unwrap();
        let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

        let hash1 = BlockHash::from_byte_array([100u8; 32]);
        let hash2 = BlockHash::from_byte_array([200u8; 32]);

        // Add hash1 at height 100 and hash2 at height 200
        let mut batch = Store::get_write_batch();
        store
            .set_height_to_blockhash(&hash1, 100, &mut batch)
            .unwrap();
        store
            .set_height_to_blockhash(&hash2, 200, &mut batch)
            .unwrap();
        store.commit_batch(batch).unwrap();

        // Verify hashes are stored at correct heights
        let hashes_100 = store.get_blockhashes_for_height(100);
        assert_eq!(hashes_100.len(), 1);
        assert!(hashes_100.contains(&hash1));

        let hashes_200 = store.get_blockhashes_for_height(200);
        assert_eq!(hashes_200.len(), 1);
        assert!(hashes_200.contains(&hash2));
    }
}
