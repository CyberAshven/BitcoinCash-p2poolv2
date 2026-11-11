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
use rocksdb::{BlockBasedOptions, ColumnFamilyDescriptor, DB, Options as RocksDbOptions};
use std::collections::{HashMap, HashSet};
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
/// - 1 (2026-10-30): chain work counts the work of each uncle a share
///   names. Stores written before this have no version.
/// - 2 (2026-11-05): Bitcoin Cash pays the genesis share only as the last
///   resort (`BCH_GENESIS_LAST_RESORT_RULE_TAG`). A chain's first hours
///   were validated under the old rule, so a node could not sync them from
///   peers under the new one; the chain starts again.
/// - 3 (2026-11-09): Bitcoin Cash has one share chain, the mini chain's,
///   and its share headers carry a difficulty band (`BCH_BAND_RULE_TAG`):
///   the share's minimum, the target its node wanted and its band cap. A
///   stored header has none of these and would not decode, and the merged
///   chain starts from a new genesis.
/// - 4 (2026-11-09): Bitcoin Cash share headers carry their miner's payout
///   level, and a slice below its payee's level is paid by a draw
///   (`BCH_PAYOUT_LEVELS_RULE_TAG`). A stored header has no level and would
///   not decode, its coinbase was validated without the draw, and the chain
///   starts from a new genesis.
/// - 5 (2026-11-09): Bitcoin Cash share headers carry a weak ratio and
///   commit the weak shares their bodies carry (`BCH_WEAK_SHARE_RULE_TAG`),
///   the store keeps those weak shares and a summary of their credit, and
///   the PPLNS window credits proofs from jobs at the minimum at the weak
///   work. A stored header has no weak ratio and would not decode, and its
///   chain was paid under the old credit; the chain starts again.
pub const STORE_FORMAT_VERSION: u32 = 5;

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
    pub fn new(path: String, read_only: bool) -> Result<Self, StoreError> {
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

        let cfs = vec![
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
            payout_trees_cf,
            bch_headers_cf,
            bch_best_chain_cf,
            weak_shares_cf,
            weak_summaries_cf,
            script_history_cf,
        ];

        // for the db too, we use default options for now
        let mut db_options = RocksDbOptions::default();
        db_options.create_missing_column_families(true);
        db_options.create_if_missing(true);
        let db = if read_only {
            // Rebuild a descriptor list for the filtered set: the sample
            // family is write-only on this open so `verify_chain`,
            // `db_query` and other read-only tools never request it.
            let read_only_cfs: Vec<ColumnFamilyDescriptor> = cfs
                .iter()
                .filter(|descriptor| descriptor.name() != ColumnFamily::HashrateSamples.as_str())
                .map(|descriptor| {
                    ColumnFamilyDescriptor::new(descriptor.name(), RocksDbOptions::default())
                })
                .collect();
            DB::open_cf_descriptors_read_only(&db_options, path.clone(), read_only_cfs, false)?
        } else {
            DB::open_cf_descriptors(&db_options, path.clone(), cfs)?
        };
        let store = Self {
            path,
            db,
            // Initialise chain state fields
            genesis_blockhash: Arc::new(RwLock::new(None)),
            depths: ChainDepths::BITCOIN,
            refused_reorgs: Mutex::new(HashMap::new()),
            script_history_gc_threshold,
        };
        Ok(store)
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
    /// A height with more than MAX_BLOCKS_PER_HEIGHT blocks, as a burst
    /// of hashrate on a small pool makes, keeps MAX_BLOCKS_PER_HEIGHT of
    /// them: first the blocks on the confirmed or candidate chain and the
    /// uncles those chains name, then the rest by blockhash. Sorting by
    /// blockhash alone could leave out the confirmed block, and a peer
    /// syncing from this node then never connected the headers above it.
    /// A block whose parent or uncle was left out is left out too: a peer
    /// could not connect it either.
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
            Err(_) => return Ok(blockhashes),
        };

        let end_height = std::cmp::min(
            start_height.saturating_add(limit as u32),
            top_confirmed_height,
        );
        let height_entries = self.get_blockhashes_for_height_range(start_height, end_height);
        let mut left_out: HashSet<BlockHash> = HashSet::new();

        for (height, raw_hashes) in height_entries {
            let mut blocks_at_height: Vec<(BlockHash, ChainMembership)> = self
                .get_block_metadata_batch(&raw_hashes)?
                .into_iter()
                .filter(|(_, metadata)| {
                    metadata.status != Status::Pending && metadata.status != Status::Invalid
                })
                .map(|(hash, metadata)| (hash, metadata.chain))
                .collect();
            if !left_out.is_empty() {
                self.leave_out_unconnectable(&mut blocks_at_height, &mut left_out)?;
            }
            blocks_at_height.sort_by_key(|(hash, _)| *hash);
            if blocks_at_height.len() > MAX_BLOCKS_PER_HEIGHT {
                let chain_uncles = self.uncles_named_by_chains_above(height)?;
                blocks_at_height.sort_by_key(|(hash, chain)| {
                    let on_a_chain = *chain != ChainMembership::None || chain_uncles.contains(hash);
                    (!on_a_chain, *hash)
                });
                left_out.extend(
                    blocks_at_height
                        .drain(MAX_BLOCKS_PER_HEIGHT..)
                        .map(|(hash, _)| hash),
                );
            }
            let hashes_at_height: Vec<BlockHash> =
                blocks_at_height.into_iter().map(|(hash, _)| hash).collect();

            let found_stop = hashes_at_height.contains(stop_blockhash);
            blockhashes.extend(hashes_at_height);

            if found_stop || blockhashes.len() >= limit {
                return Ok(blockhashes);
            }
        }

        Ok(blockhashes)
    }

    /// The uncles that the confirmed and candidate blocks up to
    /// MAX_UNCLES_DEPTH heights above `height` name.
    fn uncles_named_by_chains_above(&self, height: u32) -> Result<HashSet<BlockHash>, StoreError> {
        let mut uncles = HashSet::new();
        for above in height + 1..=height.saturating_add(MAX_UNCLES_DEPTH as u32) {
            let chain_blocks = [
                self.get_confirmed_at_height(above).ok(),
                self.get_candidate_at_height(above).ok(),
            ];
            for chain_block in chain_blocks.into_iter().flatten() {
                if let Some(header) = self.get_share_header(&chain_block)? {
                    uncles.extend(header.uncles);
                }
            }
        }
        Ok(uncles)
    }

    /// Remove from `blocks` each block whose parent or an uncle is in
    /// `left_out`, and add it there.
    fn leave_out_unconnectable(
        &self,
        blocks: &mut Vec<(BlockHash, ChainMembership)>,
        left_out: &mut HashSet<BlockHash>,
    ) -> Result<(), StoreError> {
        let hashes: Vec<BlockHash> = blocks.iter().map(|(hash, _)| *hash).collect();
        for (hash, header) in self.get_share_headers(&hashes)? {
            let unconnectable = left_out.contains(&header.prev_share_blockhash)
                || header.uncles.iter().any(|uncle| left_out.contains(uncle));
            if unconnectable {
                left_out.insert(hash);
            }
        }
        blocks.retain(|(hash, _)| !left_out.contains(hash));
        Ok(())
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
             every proof from a job at the minimum paid at the weak work. Delete the store \
             directory and restart to resync the share chain from peers.",
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
        // The read-only descriptor list does not request the column
        // family, so `load_hashrate_samples` finds no handle and starts
        // empty instead of failing the open.
        let read_only = Store::new(path, true).unwrap();
        assert!(read_only.load_hashrate_samples(0).is_empty());
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
