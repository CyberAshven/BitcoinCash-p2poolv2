// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::{ColumnFamily, Store, writer::StoreError};
use crate::shares::share_block::{
    ShareTransaction, SpendingPrevouts, Txids, extract_spending_prevouts,
};
use crate::store::block_tx_metadata::{ChainMembership, TxMetadata};
use bitcoin::consensus::{self, Decodable, Encodable, encode};
use bitcoin::{BlockHash, OutPoint, Transaction, Txid};
use rocksdb::WriteBatch;
use std::collections::{HashMap, HashSet};
use std::fmt;
use tracing::debug;

/// Serialized outpoint size: 32B for Txid hash, 4B for index
const OUTPOINT_SIZE: usize = 36;

/// Key suffix of a block's sharechain txids in the `BlockTxids` CF.
pub(crate) const BLOCK_TXIDS_SUFFIX: &[u8] = b"_txids";

/// Key suffix of a block's bitcoin txids in the `BitcoinTxids` CF. Nothing
/// writes this CF yet.
pub(crate) const BITCOIN_TXIDS_SUFFIX: &[u8] = b"_bitcoin_txids";

/// Key of `blockhash` in a txids CF: the consensus-serialized blockhash
/// followed by `suffix`.
pub(crate) fn txids_key(blockhash: &BlockHash, suffix: &[u8]) -> Vec<u8> {
    let mut key = consensus::serialize(blockhash);
    key.extend_from_slice(suffix);
    key
}

/// Key of input or output `index` of `txid` in the `Inputs`, `Outputs` and
/// `SpendsIndex` CFs.
pub(crate) fn outpoint_key(txid: &Txid, index: u32) -> String {
    format!("{txid}:{index}")
}

/// A TxOut stored in the Outputs CF together with a flag indicating
/// whether it belongs to a coinbase transaction and the height of the
/// oldest coinbase ancestor in the output's spending chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredTxOut {
    pub tx_out: bitcoin::TxOut,
    pub is_coinbase: bool,
    /// Height of the oldest coinbase in this output's ancestry chain.
    /// For coinbase outputs, this is the block height where the coinbase
    /// was mined. For spending tx outputs, this is the minimum
    /// coinbase_root_height across all inputs. Used to enforce the rule
    /// that outputs whose coinbase root is deeper than PPLNS_DEPTH from
    /// tip cannot be spent.
    pub coinbase_root_height: u32,
}

impl Encodable for StoredTxOut {
    fn consensus_encode<W: bitcoin::io::Write + ?Sized>(
        &self,
        writer: &mut W,
    ) -> Result<usize, bitcoin::io::Error> {
        let mut length = self.tx_out.consensus_encode(writer)?;
        length += self.is_coinbase.consensus_encode(writer)?;
        length += self.coinbase_root_height.consensus_encode(writer)?;
        Ok(length)
    }
}

impl Decodable for StoredTxOut {
    fn consensus_decode<R: bitcoin::io::Read + ?Sized>(
        reader: &mut R,
    ) -> Result<Self, bitcoin::consensus::encode::Error> {
        let tx_out = bitcoin::TxOut::consensus_decode(reader)?;
        let is_coinbase = bool::consensus_decode(reader)?;
        let coinbase_root_height = u32::consensus_decode(reader)?;
        Ok(StoredTxOut {
            tx_out,
            is_coinbase,
            coinbase_root_height,
        })
    }
}

/// A TxIn stored in the Inputs CF together with its witness.
///
/// `bitcoin::TxIn::consensus_encode` does not include the witness
/// (witness data lives at the Transaction level under BIP144).
/// Storing the witness alongside the TxIn ensures segwit share
/// transactions survive a store roundtrip and can be relayed to peers
/// with their witness intact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredTxIn {
    pub tx_in: bitcoin::TxIn,
}

impl Encodable for StoredTxIn {
    fn consensus_encode<W: bitcoin::io::Write + ?Sized>(
        &self,
        writer: &mut W,
    ) -> Result<usize, bitcoin::io::Error> {
        let mut length = self.tx_in.consensus_encode(writer)?;
        length += self.tx_in.witness.consensus_encode(writer)?;
        Ok(length)
    }
}

impl Decodable for StoredTxIn {
    fn consensus_decode<R: bitcoin::io::Read + ?Sized>(
        reader: &mut R,
    ) -> Result<Self, bitcoin::consensus::encode::Error> {
        let mut tx_in = bitcoin::TxIn::consensus_decode(reader)?;
        tx_in.witness = bitcoin::Witness::consensus_decode(reader)?;
        Ok(StoredTxIn { tx_in })
    }
}

/// The in-memory deltas that a single confirmation `WriteBatch` applies. It is
/// used to re-check prevouts against the confirmed state the batch itself is
/// building.
///
/// A `WriteBatch` is opaque to reads until committed, so the committed-state
/// queries cannot see the batch's own writes mid-flight. This overlay carries
/// those deltas. It tracks block/outpoint, not just txids, because a
/// transaction can appear in more than one block across forks or an extend
/// chain control flow.
#[derive(Debug, Default)]
pub(crate) struct ConfirmationOverlay {
    /// Blocks leaving the confirmed chain in this batch (reorged-out suffix;
    /// empty for an extend). A source txid is not counted as confirmed when its
    /// only confirmed block is in this set.
    pub removed_blockhashes: HashSet<BlockHash>,
    /// Outpoints whose committed spend the rewind removes. Their committed
    /// spend is ignored, so a fork block may re-spend an output the reorged-out
    /// branch had spent.
    pub removed_spends: HashSet<OutPoint>,
    /// Outpoints spent by blocks confirmed earlier in this batch, so a later
    /// block that double-spends one of them is rejected.
    pub spent_in_batch: HashSet<OutPoint>,
    /// Blocks confirmed earlier in this batch, so a later block spending an
    /// output one of them introduces is accepted.
    pub confirmed_in_batch: HashSet<BlockHash>,
}

/// Why a prevout check rejected a block.
///
/// These are facts about the block's inputs, established with all the data
/// present, not failures to read the store: a block carrying either of them
/// breaks a consensus rule and must be marked Invalid, never treated as
/// corruption.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrevoutRejection {
    /// The outpoint has no entry in the `Outputs` column family, so the
    /// output it spends does not exist on this node's view of the chain.
    MissingOutput(OutPoint),
    /// The output's coinbase root is older than the payout window allows.
    CoinbaseRootTooOld {
        outpoint: OutPoint,
        coinbase_root_height: u32,
        minimum_height: u32,
    },
    /// A coinbase output spent before enough blocks have passed since the
    /// block that mined it. Depth is measured from the stored
    /// `coinbase_root_height` to the spending block's own height.
    ImmatureCoinbase {
        outpoint: OutPoint,
        coinbase_root_height: u32,
        spending_height: u32,
        required_depth: usize,
    },
}

impl fmt::Display for PrevoutRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PrevoutRejection::MissingOutput(outpoint) => write!(
                f,
                "Output not found for {}:{}",
                outpoint.txid, outpoint.vout
            ),
            PrevoutRejection::CoinbaseRootTooOld {
                outpoint,
                coinbase_root_height,
                minimum_height,
            } => write!(
                f,
                "Output {}:{} has coinbase_root_height {} below minimum {}",
                outpoint.txid, outpoint.vout, coinbase_root_height, minimum_height
            ),
            PrevoutRejection::ImmatureCoinbase {
                outpoint,
                coinbase_root_height,
                spending_height,
                required_depth,
            } => write!(
                f,
                "Coinbase output {}:{} mined at height {} is not mature at height {} (requires at least {} blocks of depth)",
                outpoint.txid, outpoint.vout, coinbase_root_height, spending_height, required_depth
            ),
        }
    }
}

/// Outcome of checking a block's prevouts against the `Outputs` column family.
///
/// Separates a consensus verdict from a store failure: the enum carries the
/// verdict, while `Err(StoreError)` from the check is reserved for reads that
/// actually failed (RocksDB errors, undecodable rows).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrevoutCheck {
    /// Every outpoint exists, is within the window, and is mature.
    Accepted,
    /// A consensus rule about the prevouts is broken.
    Rejected(PrevoutRejection),
}

#[allow(dead_code)]
impl Store {
    /// Retrieve all previous outputs being spent by a transaction's inputs.
    ///
    /// Uses a single batch query to fetch all spent outputs, avoiding
    /// N+1 lookups. Returns a vector of (input_index, TxOut) pairs in
    /// input order.
    pub(crate) fn get_all_prevouts(
        &self,
        transaction: &bitcoin::Transaction,
    ) -> Result<Vec<(usize, bitcoin::TxOut)>, StoreError> {
        let outputs_cf = self.db.cf_handle(&ColumnFamily::Outputs).unwrap();
        let keys: Vec<(_, String)> = transaction
            .input
            .iter()
            .map(|input| {
                let key = format!(
                    "{}:{}",
                    input.previous_output.txid, input.previous_output.vout
                );
                (&outputs_cf, key)
            })
            .collect();

        let cf_keys: Vec<(_, &[u8])> = keys.iter().map(|(cf, key)| (*cf, key.as_bytes())).collect();

        // results are in the same order as keys
        let results = self.db.multi_get_cf(cf_keys);
        let mut prevouts = Vec::with_capacity(results.len());
        for (input_index, result) in results.into_iter().enumerate() {
            let data = result?.ok_or_else(|| {
                let outpoint = &transaction.input[input_index].previous_output;
                StoreError::NotFound(format!(
                    "Output not found for {}:{}",
                    outpoint.txid, outpoint.vout
                ))
            })?;
            let stored: StoredTxOut = encode::deserialize(&data).map_err(|_| {
                StoreError::Serialization("Failed to deserialize output".to_string())
            })?;
            prevouts.push((input_index, stored.tx_out));
        }
        Ok(prevouts)
    }

    /// Batch-read all outpoints from the Outputs CF in a single multi_get and
    /// check every rule that depends only on the spending block itself.
    ///
    /// Three rejections are possible: the outpoint has no entry, its
    /// `coinbase_root_height` is below `min_coinbase_root_height`, or it is a
    /// coinbase output spent less than `coinbase_maturity` blocks after the
    /// block that mined it. Each is a fact about the block's inputs, not a
    /// store failure, so it comes back as `Ok(PrevoutCheck::Rejected)` and the
    /// caller can mark the block Invalid. `Err` is reserved for reads that
    /// genuinely failed: a RocksDB error or an undecodable row.
    ///
    /// Maturity is measured from `StoredTxOut::coinbase_root_height` -- the
    /// height of the block that mined the coinbase, stamped on the output when
    /// it was written -- to `spending_height`, the spending block's own height.
    /// Both are properties of blocks, not of this node's confirmed chain, so
    /// the verdict is the same on every node and survives a reorg. Whether the
    /// prevout's source is confirmed, and whether it is already spent, are not
    /// checked here: those answers change with a reorg, so they belong to
    /// `recheck_block_prevouts_with_overlay` at confirmation time.
    pub(crate) fn check_prevouts(
        &self,
        outpoints: &[OutPoint],
        spending_height: u32,
        min_coinbase_root_height: u32,
        coinbase_maturity: usize,
    ) -> Result<PrevoutCheck, StoreError> {
        if outpoints.is_empty() {
            return Ok(PrevoutCheck::Accepted);
        }
        let outputs_cf = self.db.cf_handle(&ColumnFamily::Outputs).unwrap();
        let keys: Vec<String> = outpoints
            .iter()
            .map(|outpoint| outpoint_key(&outpoint.txid, outpoint.vout))
            .collect();
        let cf_keys: Vec<(_, &[u8])> = keys
            .iter()
            .map(|key| (&outputs_cf, key.as_bytes()))
            .collect();
        for (index, result) in self.db.multi_get_cf(cf_keys).into_iter().enumerate() {
            let Some(data) = result? else {
                return Ok(PrevoutCheck::Rejected(PrevoutRejection::MissingOutput(
                    outpoints[index],
                )));
            };
            let stored: StoredTxOut = encode::deserialize(&data).map_err(|_| {
                StoreError::Serialization("Failed to deserialize output".to_string())
            })?;
            if stored.coinbase_root_height < min_coinbase_root_height {
                return Ok(PrevoutCheck::Rejected(
                    PrevoutRejection::CoinbaseRootTooOld {
                        outpoint: outpoints[index],
                        coinbase_root_height: stored.coinbase_root_height,
                        minimum_height: min_coinbase_root_height,
                    },
                ));
            }
            let depth = spending_height.saturating_sub(stored.coinbase_root_height) as usize;
            if stored.is_coinbase && depth < coinbase_maturity {
                return Ok(PrevoutCheck::Rejected(PrevoutRejection::ImmatureCoinbase {
                    outpoint: outpoints[index],
                    coinbase_root_height: stored.coinbase_root_height,
                    spending_height,
                    required_depth: coinbase_maturity,
                }));
            }
        }
        Ok(PrevoutCheck::Accepted)
    }

    /// Batch check the SpendsIndex column family: returns true if any
    /// outpoint has a stored spend entry.
    ///
    /// `SpendsIndex` is written only from the confirmation path
    /// (`put_confirmed_entry`) and cleared on reorg-out, so presence in
    /// this column family means "spent by a transaction on the confirmed
    /// sharechain". No per-hit confirmation lookup is needed.
    pub(crate) fn is_any_prevout_spent(&self, outpoints: &[OutPoint]) -> Result<bool, StoreError> {
        if outpoints.is_empty() {
            return Ok(false);
        }
        let spends_index_cf = self.db.cf_handle(&ColumnFamily::SpendsIndex).unwrap();
        let keys: Vec<String> = outpoints
            .iter()
            .map(|outpoint| outpoint_key(&outpoint.txid, outpoint.vout))
            .collect();
        let cf_keys: Vec<(_, &[u8])> = keys
            .iter()
            .map(|key| (&spends_index_cf, key.as_bytes()))
            .collect();
        for result in self.db.multi_get_cf(cf_keys) {
            if result?.is_some() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// `is_any_prevout_spent` adjusted by a confirmation batch's deltas.
    ///
    /// An outpoint is spent when it is spent by a block confirmed earlier in
    /// this batch (`spent_in_batch`), OR its committed spend still stands after
    /// the batch's reorg-out removals (committed and not in `removed_spends`).
    pub(crate) fn is_any_prevout_spent_with_overlay(
        &self,
        outpoints: &[OutPoint],
        removed_spends_in_batch: &HashSet<OutPoint>,
        already_spent_in_batch: &HashSet<OutPoint>,
    ) -> Result<bool, StoreError> {
        if outpoints
            .iter()
            .any(|outpoint| already_spent_in_batch.contains(outpoint))
        {
            return Ok(true);
        }
        let still_being_spent_in_batch: Vec<OutPoint> = outpoints
            .iter()
            .filter(|outpoint| !removed_spends_in_batch.contains(*outpoint))
            .copied()
            .collect();
        self.is_any_prevout_spent(&still_being_spent_in_batch)
    }

    /// Returns true if every provided txid is included in at least one
    /// block on the confirmed chain (`BlockMetadata::chain` is
    /// `ChainMembership::Confirmed`). Returns false on the first txid that
    /// has no confirmed blockhash.
    pub(crate) fn are_all_txids_confirmed(&self, txids: &[Txid]) -> Result<bool, StoreError> {
        self.are_all_txids_confirmed_with_overlay(txids, &HashSet::new(), &HashSet::new())
    }

    /// As `are_all_txids_confirmed`, but adjusted by what the current batch
    /// does to the confirmed chain: blockhashes in `removed` are treated as no
    /// longer confirmed, and those in `added` as already confirmed.
    ///
    /// Both directions are needed because a batch both rewinds and confirms.
    /// Without `removed`, a source whose only confirmed block is being reorged
    /// out would still count as confirmed. Without `added`, a source confirmed
    /// by an earlier block of this same batch would count as unconfirmed --
    /// `confirm_blocks` runs after the prefix walk, so those blocks still carry
    /// their committed `Candidate` membership while the walk is in progress.
    /// This mirrors `is_any_prevout_spent_with_overlay`, which also
    /// consults both directions for spends.
    ///
    /// Both sets are block-level, not txid-level, because a txid can appear in
    /// more than one block: a source still counts as confirmed if any
    /// surviving confirmed block includes it.
    pub(crate) fn are_all_txids_confirmed_with_overlay(
        &self,
        txids: &[Txid],
        removed: &HashSet<BlockHash>,
        added: &HashSet<BlockHash>,
    ) -> Result<bool, StoreError> {
        let per_txid_blockhashes = self.get_blockhashes_for_all_txids(txids)?;

        let all_blockhashes: Vec<BlockHash> = per_txid_blockhashes
            .iter()
            .flat_map(|blockhashes| blockhashes.iter().copied())
            .collect();

        let metadata_pairs = self.get_block_metadata_batch(&all_blockhashes)?;
        let confirmed_set: HashSet<BlockHash> = metadata_pairs
            .into_iter()
            .filter(|(blockhash, metadata)| {
                metadata.chain == ChainMembership::Confirmed && !removed.contains(blockhash)
            })
            .map(|(blockhash, _)| blockhash)
            .collect();

        for blockhashes in &per_txid_blockhashes {
            let has_confirmed = blockhashes
                .iter()
                .any(|blockhash| confirmed_set.contains(blockhash) || added.contains(blockhash));
            if !has_confirmed {
                return Ok(false);
            }
        }

        Ok(true)
    }

    /// Retrieve a single transaction output by txid and output index.
    ///
    /// Looks up the output in the Outputs column family using the
    /// key format `{txid}:{vout}`.
    pub(crate) fn get_output(&self, txid: &Txid, vout: u32) -> Result<bitcoin::TxOut, StoreError> {
        let outputs_cf = self.db.cf_handle(&ColumnFamily::Outputs).unwrap();
        let output_key = outpoint_key(txid, vout);
        match self.db.get_cf::<&[u8]>(&outputs_cf, output_key.as_ref())? {
            Some(data) => {
                let stored: StoredTxOut = encode::deserialize(&data).map_err(|_| {
                    StoreError::Serialization("Failed to deserialize output".to_string())
                })?;
                Ok(stored.tx_out)
            }
            None => Err(StoreError::NotFound(format!(
                "Output not found for {txid}:{vout}"
            ))),
        }
    }

    /// Store share chain transactions in the store.
    ///
    /// Writes transaction metadata, each input, and each output into
    /// their respective column families. This function is pure tx
    /// storage and does not touch the `SpendsIndex` -- chain-state
    /// bookkeeping of spends is the responsibility of the confirmation
    /// path (`put_confirmed_entry` / `remove_spends_for_block`).
    ///
    /// The block -> txids association is stored separately via
    /// `add_txids_to_blocks_index`, allowing this function to persist
    /// transactions outside of a block context.
    ///
    /// `block_height` is used to set `coinbase_root_height` on outputs:
    /// coinbase outputs get the block height directly, spending tx
    /// outputs get the minimum coinbase_root_height across their inputs.
    pub(crate) fn add_sharechain_txs(
        &self,
        transactions: &[ShareTransaction],
        block_height: u32,
        batch: &mut rocksdb::WriteBatch,
    ) -> Result<Vec<TxMetadata>, StoreError> {
        let inputs_cf = self.db.cf_handle(&ColumnFamily::Inputs).unwrap();
        let outputs_cf = self.db.cf_handle(&ColumnFamily::Outputs).unwrap();
        let mut txs_metadata = Vec::new();
        // Track coinbase_root_height for outputs written in this batch
        // so in-block spending chains can look up earlier outputs.
        let mut batch_heights: HashMap<String, u32> =
            HashMap::with_capacity(transactions.len() * 2);

        for tx in transactions {
            let txid = tx.compute_txid();
            let metadata = self.add_tx_metadata(txid, tx, false, batch)?;
            txs_metadata.push(metadata);

            for (i, input) in tx.input.iter().enumerate() {
                let input_key = outpoint_key(&txid, i as u32);
                let stored = StoredTxIn {
                    tx_in: input.clone(),
                };
                let serialized = consensus::serialize(&stored);
                batch.put_cf::<&[u8], Vec<u8>>(&inputs_cf, input_key.as_ref(), serialized);
            }

            let is_coinbase = tx.0.is_coinbase();
            let coinbase_root_height = if is_coinbase {
                block_height
            } else {
                self.min_coinbase_root_height_for_inputs(tx, &outputs_cf, &batch_heights)?
            };

            for (i, output) in tx.output.iter().enumerate() {
                let output_key = outpoint_key(&txid, i as u32);
                batch_heights.insert(output_key.clone(), coinbase_root_height);
                let stored = StoredTxOut {
                    tx_out: output.clone(),
                    is_coinbase,
                    coinbase_root_height,
                };
                let serialized = consensus::serialize(&stored);
                batch.put_cf::<&[u8], Vec<u8>>(&outputs_cf, output_key.as_ref(), serialized);
            }
        }
        Ok(txs_metadata)
    }

    /// Compute the minimum coinbase_root_height across all inputs of a
    /// non-coinbase transaction. Checks the in-batch cache first (for
    /// in-block spending chains), then falls back to the Outputs CF.
    /// Returns an error if any input's output is missing or cannot be
    /// deserialized.
    fn min_coinbase_root_height_for_inputs(
        &self,
        tx: &ShareTransaction,
        outputs_cf: &impl rocksdb::AsColumnFamilyRef,
        batch_heights: &HashMap<String, u32>,
    ) -> Result<u32, StoreError> {
        let mut min_height = u32::MAX;
        for input in &tx.0.input {
            let outpoint_key =
                outpoint_key(&input.previous_output.txid, input.previous_output.vout);

            let height = if let Some(cached_height) = batch_heights.get(&outpoint_key) {
                *cached_height
            } else {
                let bytes = self
                    .db
                    .get_cf(outputs_cf, outpoint_key.as_bytes())?
                    .ok_or_else(|| {
                        StoreError::NotFound(format!(
                            "Input output not found for coinbase_root_height: {outpoint_key}"
                        ))
                    })?;
                let stored_out: StoredTxOut = consensus::deserialize(&bytes).map_err(|_| {
                    StoreError::Serialization(format!(
                        "Failed to deserialize output for coinbase_root_height: {outpoint_key}"
                    ))
                })?;
                stored_out.coinbase_root_height
            };
            min_height = std::cmp::min(min_height, height);
        }
        Ok(min_height)
    }

    /// Marks the transaction as successfully validated, this prevents us validating it again.
    /// We only validate what is dependent on the chain state. Once valid, the txid is never made invalid.
    pub(crate) fn mark_transaction_valid(
        &self,
        txid: &Txid,
        batch: &mut rocksdb::WriteBatch,
    ) -> Result<TxMetadata, StoreError> {
        // mark tx metadata as validated
        let mut tx_metadata = self.get_tx_metadata(txid)?;
        tx_metadata.validated = true;

        let mut serialized = Vec::new();
        tx_metadata.consensus_encode(&mut serialized)?;
        let tx_cf = self.db.cf_handle(&ColumnFamily::Tx).unwrap();
        batch.put_cf::<&[u8], Vec<u8>>(&tx_cf, txid.as_ref(), serialized);

        Ok(tx_metadata)
    }

    /// Add spend index entry from output -> input. This helps us
    /// track which outputs have been spent, and quickly check if an
    /// output is spent - by checking for presence in this index.
    ///
    /// Using this index instead of an UTXO set we can concurrently
    /// update this index.
    ///
    /// To check if a spend is a part of a confirmed block, we need to
    /// look at the spending txid and check if it is included in a
    /// confirmed block.
    ///
    /// We need to add a transaction id -> blockhash index
    pub(crate) fn add_spend(
        &self,
        prevout_txid: &Txid,
        prevout_index: u32,
        spending_txid: &Txid,
        spending_index: u32,
        batch: &mut WriteBatch,
    ) -> Result<(), StoreError> {
        let key = outpoint_key(prevout_txid, prevout_index);
        let spends_index_cf = self.db.cf_handle(&ColumnFamily::SpendsIndex).unwrap();
        let mut serialized = Vec::with_capacity(OUTPOINT_SIZE);
        spending_txid.consensus_encode(&mut serialized)?;
        spending_index.consensus_encode(&mut serialized)?;
        batch.put_cf::<&[u8], Vec<u8>>(&spends_index_cf, key.as_ref(), serialized);
        Ok(())
    }

    /// Delete a `SpendsIndex` entry for `(prevout_txid, prevout_index)`.
    ///
    /// Used by the reorg-out path when a block that previously recorded
    /// the spend is no longer on the confirmed sharechain. `delete_cf`
    /// leaves a RocksDB tombstone; this is acceptable at `SpendsIndex`
    /// cardinality (bounded by inputs in reorged blocks).
    pub(crate) fn remove_spend(
        &self,
        prevout_txid: &Txid,
        prevout_index: u32,
        batch: &mut WriteBatch,
    ) {
        let key = outpoint_key(prevout_txid, prevout_index);
        let spends_index_cf = self.db.cf_handle(&ColumnFamily::SpendsIndex).unwrap();
        batch.delete_cf::<&[u8]>(&spends_index_cf, key.as_ref());
    }

    /// Write `SpendsIndex` entries for every non-coinbase input of every
    /// transaction in `txs`. Intended to be called from the confirmation
    /// path so that `SpendsIndex` presence means "spent on the confirmed
    /// sharechain".
    pub(crate) fn add_spends_for_block(
        &self,
        txs: &[ShareTransaction],
        batch: &mut WriteBatch,
    ) -> Result<(), StoreError> {
        for share_transaction in txs {
            let transaction = &share_transaction.0;
            if !transaction.is_coinbase() {
                let spending_txid = transaction.compute_txid();
                for (spending_index, input) in transaction.input.iter().enumerate() {
                    self.add_spend(
                        &input.previous_output.txid,
                        input.previous_output.vout,
                        &spending_txid,
                        spending_index as u32,
                        batch,
                    )?;
                }
            }
        }
        Ok(())
    }

    /// Delete every `SpendsIndex` entry that `add_spends_for_block` would
    /// have written for `txs`. Used by the reorg-out path.
    pub(crate) fn remove_spends_for_block(&self, txs: &[ShareTransaction], batch: &mut WriteBatch) {
        for share_transaction in txs {
            let transaction = &share_transaction.0;
            if !transaction.is_coinbase() {
                for input in &transaction.input {
                    self.remove_spend(
                        &input.previous_output.txid,
                        input.previous_output.vout,
                        batch,
                    );
                }
            }
        }
    }

    /// Check if txid:vout outpoint is spent
    ///
    /// Checks if the outpoint is in the index and return the spending
    /// input txid:index as outpoint if present
    ///
    /// Caller should check if the spending outpoint is in a confirmed
    /// block or not, if they need to.
    pub(crate) fn is_spent(&self, txid: &Txid, vout: u32) -> Result<Option<OutPoint>, StoreError> {
        let key = outpoint_key(txid, vout);
        let spends_index_cf = self.db.cf_handle(&ColumnFamily::SpendsIndex).unwrap();

        match self.db.get_cf::<&[u8]>(&spends_index_cf, key.as_ref())? {
            Some(outpoint) => {
                let (txid, consumed) = encode::deserialize_partial(&outpoint)?;
                let (vout, _consumed) = encode::deserialize_partial(&outpoint[consumed..])?;
                Ok(Some(OutPoint { txid, vout }))
            }
            None => Ok(None),
        }
    }

    /// Store transaction metadata
    pub(crate) fn add_tx_metadata(
        &self,
        txid: bitcoin::Txid,
        tx: &Transaction,
        validated: bool,
        batch: &mut rocksdb::WriteBatch,
    ) -> Result<TxMetadata, StoreError> {
        debug!("Adding tx metadata for txid {txid}");
        let tx_metadata = TxMetadata {
            txid,
            version: tx.version,
            lock_time: tx.lock_time,
            input_count: tx.input.len() as u32,
            output_count: tx.output.len() as u32,
            validated,
        };

        let tx_cf = self.db.cf_handle(&ColumnFamily::Tx).unwrap();
        let mut tx_metadata_serialized = Vec::new();
        tx_metadata.consensus_encode(&mut tx_metadata_serialized)?;
        batch.put_cf::<&[u8], Vec<u8>>(&tx_cf, txid.as_ref(), tx_metadata_serialized);
        Ok(tx_metadata)
    }

    /// Add the given blockhash to the txids -> blocks index for all txids.
    ///
    /// This index maps each txid to one or more blockhashes, allowing
    /// callers to look up which blocks contain a given transaction.
    /// Whether those blocks (and thus the transactions) are confirmed
    /// depends on the chain state and is determined by callers outside
    /// this function.
    ///
    /// Uses merge operator to append blockhashes, since a txid can be
    /// included in multiple blocks (e.g., competing miners extending
    /// different chain tips).
    pub(crate) fn add_txids_to_blocks_index(
        &self,
        blockhash: &BlockHash,
        txids: &Txids,
        batch: &mut WriteBatch,
    ) -> Result<(), StoreError> {
        let txids_blocks_cf = self.db.cf_handle(&ColumnFamily::TxidsBlocks).unwrap();
        let serialized_blockhash = consensus::serialize(blockhash);

        for txid in &txids.0 {
            batch.merge_cf(
                &txids_blocks_cf,
                AsRef::<[u8]>::as_ref(txid),
                &serialized_blockhash,
            );
        }
        Ok(())
    }

    /// Get all blockhashes for a given txid from the txids_blocks index
    ///
    /// Returns a vector of blockhashes that contain this txid.
    /// A txid can be in multiple blocks when competing miners include
    /// the same transaction in different blocks.
    /// This is the reverse lookup of add_txids_to_blocks_index.
    pub(crate) fn get_blockhashes_for_txid(
        &self,
        txid: &Txid,
    ) -> Result<Vec<BlockHash>, StoreError> {
        let txids_blocks_cf = self.db.cf_handle(&ColumnFamily::TxidsBlocks).unwrap();

        match self.db.get_cf::<&[u8]>(&txids_blocks_cf, txid.as_ref())? {
            Some(blockhash_bytes) => {
                let blockhashes: Vec<BlockHash> =
                    encode::deserialize(&blockhash_bytes).map_err(|_| {
                        StoreError::Serialization(
                            "Failed to deserialize blockhashes from txids_blocks index".to_string(),
                        )
                    })?;
                Ok(blockhashes)
            }
            None => Ok(Vec::new()),
        }
    }

    /// Batch-fetch blockhashes for all provided txids in a single multi_get_cf call.
    ///
    /// Returns a Vec parallel to the input: each entry is the list of
    /// blockhashes that contain that txid (empty if none found).
    pub(crate) fn get_blockhashes_for_all_txids(
        &self,
        txids: &[Txid],
    ) -> Result<Vec<Vec<BlockHash>>, StoreError> {
        if txids.is_empty() {
            return Ok(Vec::new());
        }
        let txids_blocks_cf = self.db.cf_handle(&ColumnFamily::TxidsBlocks).unwrap();
        let cf_keys: Vec<(_, &[u8])> = txids
            .iter()
            .map(|txid| (&txids_blocks_cf, txid.as_ref()))
            .collect();
        let results = self.db.multi_get_cf(cf_keys);
        let mut all_blockhashes: Vec<Vec<BlockHash>> = Vec::with_capacity(txids.len());
        for result in results {
            match result? {
                Some(blockhash_bytes) => {
                    let blockhashes: Vec<BlockHash> = encode::deserialize(&blockhash_bytes)
                        .map_err(|_| {
                            StoreError::Serialization(
                                "Failed to deserialize blockhashes from txids_blocks index"
                                    .to_string(),
                            )
                        })?;
                    all_blockhashes.push(blockhashes);
                }
                None => {
                    all_blockhashes.push(Vec::new());
                }
            }
        }
        Ok(all_blockhashes)
    }

    /// Add the list of transaction IDs to the batch
    /// Transactions themselves are stored in add_txs, here we just store the association between block and txids
    pub(crate) fn add_block_to_txids_index(
        &self,
        blockhash: &BlockHash,
        txids: &Txids,
        batch: &mut rocksdb::WriteBatch,
        bytes_suffix: &[u8],
        column_family: ColumnFamily,
    ) -> Result<(), StoreError> {
        let blockhash_bytes = txids_key(blockhash, bytes_suffix);

        let mut serialized_txids = Vec::new();
        txids.consensus_encode(&mut serialized_txids)?;
        let block_txids_cf = self.db.cf_handle(&column_family).unwrap();
        batch.put_cf::<&[u8], Vec<u8>>(&block_txids_cf, blockhash_bytes.as_ref(), serialized_txids);
        Ok(())
    }

    /// Get all transaction IDs for a given block hash from the
    /// BlockTxids column family.
    pub fn get_txids_for_blockhash(&self, blockhash: &BlockHash) -> Txids {
        let blockhash_bytes = txids_key(blockhash, BLOCK_TXIDS_SUFFIX);

        let block_txids_cf = self.db.cf_handle(&ColumnFamily::BlockTxids).unwrap();
        match self
            .db
            .get_cf::<&[u8]>(&block_txids_cf, blockhash_bytes.as_ref())
        {
            Ok(Some(serialized_txids)) => match encode::deserialize(&serialized_txids) {
                Ok(t) => t,
                Err(_) => {
                    tracing::warn!("Error reading txids for blockhash");
                    Txids(Vec::new())
                }
            },
            _ => Txids(Vec::new()),
        }
    }

    /// Get the validation status of a transaction from the store
    pub(crate) fn get_tx_metadata(&self, txid: &bitcoin::Txid) -> Result<TxMetadata, StoreError> {
        let tx_cf = self.db.cf_handle(&ColumnFamily::Tx).unwrap();
        match self.db.get_cf::<&[u8]>(&tx_cf, txid.as_ref())? {
            Some(tx_metadata) => encode::deserialize(&tx_metadata).map_err(|_| {
                StoreError::Serialization("Failed to deserialize tx metadata".to_string())
            }),
            None => Err(StoreError::NotFound(format!(
                "Transaction metadata not found for txid: {txid}"
            ))),
        }
    }

    /// Get transactions for a blockhash from the BlockTxids column
    /// family. Looks up the txids index, then reconstructs each
    /// transaction from the Tx/Inputs/Outputs CFs.
    pub fn get_txs_for_blockhash(&self, blockhash: &BlockHash) -> Vec<Transaction> {
        let txids = self.get_txids_for_blockhash(blockhash);
        txids.0.iter().flat_map(|txid| self.get_tx(txid)).collect()
    }

    /// Get a transaction from the store using a provided txid
    /// - Load tx metadata
    /// - Load inputs
    /// - Load outputs
    /// - Deserialize inputs and outputs
    /// - Return transaction
    pub fn get_tx(&self, txid: &bitcoin::Txid) -> Result<Transaction, StoreError> {
        let tx_metadata = self.get_tx_metadata(txid)?;

        debug!("Transaction metadata: {:?}", tx_metadata);

        let inputs_cf = self.db.cf_handle(&ColumnFamily::Inputs).unwrap();
        let outputs_cf = self.db.cf_handle(&ColumnFamily::Outputs).unwrap();
        let mut inputs = Vec::new();
        let mut outputs = Vec::new();

        for i in 0..tx_metadata.input_count {
            let input_key = outpoint_key(txid, i);
            let input = self
                .db
                .get_cf::<&[u8]>(&inputs_cf, input_key.as_ref())
                .unwrap()
                .unwrap();
            let input: bitcoin::TxIn = match encode::deserialize::<StoredTxIn>(&input) {
                Ok(stored) => stored.tx_in,
                Err(e) => {
                    tracing::error!("Error deserializing input: {e:?}");
                    return Err(e.into());
                }
            };
            inputs.push(input);
        }
        for i in 0..tx_metadata.output_count {
            let output_key = outpoint_key(txid, i);
            let output = self
                .db
                .get_cf::<&[u8]>(&outputs_cf, output_key.as_ref())
                .unwrap()
                .unwrap();
            let output = match encode::deserialize::<StoredTxOut>(&output) {
                Ok(stored) => stored.tx_out,
                Err(e) => {
                    tracing::error!("Error deserializing output: {e:?}");
                    return Err(e.into());
                }
            };
            outputs.push(output);
        }
        let transaction = Transaction {
            version: tx_metadata.version,
            lock_time: tx_metadata.lock_time,
            input: inputs,
            output: outputs,
        };
        Ok(transaction)
    }

    /// Batch load `TxMetadata` for a list of txids via a single
    /// `multi_get_cf` on the `Tx` column family. Result order matches
    /// the input order. Errors on any missing txid.
    pub(crate) fn get_metadata_for_txids(
        &self,
        txids: &[Txid],
    ) -> Result<Vec<TxMetadata>, StoreError> {
        if txids.is_empty() {
            return Ok(Vec::new());
        }
        let tx_cf = self.db.cf_handle(&ColumnFamily::Tx).unwrap();
        let keys: Vec<(_, &[u8])> = txids.iter().map(|txid| (&tx_cf, txid.as_ref())).collect();
        let results = self.db.multi_get_cf(keys);
        let mut metadatas: Vec<TxMetadata> = Vec::with_capacity(txids.len());
        for (index, result) in results.into_iter().enumerate() {
            let bytes = result?.ok_or_else(|| {
                StoreError::NotFound(format!(
                    "Transaction metadata not found for txid: {}",
                    txids[index]
                ))
            })?;
            let metadata: TxMetadata = encode::deserialize(&bytes).map_err(|_| {
                StoreError::Serialization("Failed to deserialize tx metadata".to_string())
            })?;
            metadatas.push(metadata);
        }
        Ok(metadatas)
    }

    /// Batch load transaction inputs for every `(txid, metadata)` pair
    /// via a single `multi_get_cf` on the `Inputs` column family. Returns
    /// a per-tx `Vec<TxIn>` in the same order as `txids`. `txids` and
    /// `metadatas` must have the same length.
    pub(crate) fn get_inputs(
        &self,
        txids: &[Txid],
        metadatas: &[TxMetadata],
    ) -> Result<Vec<Vec<bitcoin::TxIn>>, StoreError> {
        let inputs_cf = self.db.cf_handle(&ColumnFamily::Inputs).unwrap();
        let mut keys: Vec<String> = Vec::new();
        // Use offsets to track inputs for various txids to avoid using hashmap here
        let mut offsets: Vec<usize> = Vec::with_capacity(txids.len() + 1);
        offsets.push(0);
        for (index, metadata) in metadatas.iter().enumerate() {
            let txid = &txids[index];
            for input_index in 0..metadata.input_count {
                keys.push(outpoint_key(txid, input_index));
            }
            offsets.push(keys.len());
        }

        let cf_keys: Vec<(_, &[u8])> = keys
            .iter()
            .map(|key| (&inputs_cf, key.as_bytes()))
            .collect();
        let results = self.db.multi_get_cf(cf_keys);
        let mut decoded: Vec<bitcoin::TxIn> = Vec::with_capacity(keys.len());
        for (index, result) in results.into_iter().enumerate() {
            let bytes = result?.ok_or_else(|| {
                StoreError::NotFound(format!("Input not found for {}", keys[index]))
            })?;
            let stored: StoredTxIn = encode::deserialize(&bytes).map_err(|_| {
                StoreError::Serialization("Failed to deserialize input".to_string())
            })?;
            decoded.push(stored.tx_in);
        }

        // group the offset based inputs for each txid
        let mut grouped: Vec<Vec<bitcoin::TxIn>> = Vec::with_capacity(txids.len());
        for index in 0..txids.len() {
            grouped.push(decoded[offsets[index]..offsets[index + 1]].to_vec());
        }
        Ok(grouped)
    }

    /// Batch load transaction outputs for every `(txid, metadata)` pair
    /// via a single `multi_get_cf` on the `Outputs` column family.
    /// Returns a per-tx `Vec<TxOut>` in the same order as `txids`.
    /// `txids` and `metadatas` must have the same length.
    pub(crate) fn get_outputs(
        &self,
        txids: &[Txid],
        metadatas: &[TxMetadata],
    ) -> Result<Vec<Vec<bitcoin::TxOut>>, StoreError> {
        let outputs_cf = self.db.cf_handle(&ColumnFamily::Outputs).unwrap();
        let mut keys: Vec<String> = Vec::new();
        // Use offsets to track ouputs for various txids to avoid using hashmap here
        let mut offsets: Vec<usize> = Vec::with_capacity(txids.len() + 1);
        offsets.push(0);
        for (index, metadata) in metadatas.iter().enumerate() {
            let txid = &txids[index];
            for output_index in 0..metadata.output_count {
                keys.push(outpoint_key(txid, output_index));
            }
            offsets.push(keys.len());
        }

        let cf_keys: Vec<(_, &[u8])> = keys
            .iter()
            .map(|key| (&outputs_cf, key.as_bytes()))
            .collect();
        let results = self.db.multi_get_cf(cf_keys);
        let mut decoded: Vec<bitcoin::TxOut> = Vec::with_capacity(keys.len());
        for (index, result) in results.into_iter().enumerate() {
            let bytes = result?.ok_or_else(|| {
                StoreError::NotFound(format!("Output not found for {}", keys[index]))
            })?;
            let stored: StoredTxOut = encode::deserialize(&bytes).map_err(|_| {
                StoreError::Serialization("Failed to deserialize output".to_string())
            })?;
            decoded.push(stored.tx_out);
        }

        // group the offset based outputs for each txid
        let mut grouped: Vec<Vec<bitcoin::TxOut>> = Vec::with_capacity(txids.len());
        for index in 0..txids.len() {
            grouped.push(decoded[offsets[index]..offsets[index + 1]].to_vec());
        }
        Ok(grouped)
    }

    /// Batch load transactions for a list of txids.
    ///
    /// Composes `get_metadata_for_txids`, `get_inputs`, and
    /// `get_outputs`: three `multi_get_cf` calls total, one per column
    /// family, independent of how many transactions are requested.
    pub(crate) fn get_txs(&self, txids: &[Txid]) -> Result<Vec<Transaction>, StoreError> {
        if txids.is_empty() {
            return Ok(Vec::new());
        }
        let metadatas = self.get_metadata_for_txids(txids)?;
        let inputs = self.get_inputs(txids, &metadatas)?;
        let outputs = self.get_outputs(txids, &metadatas)?;
        let mut txs = Vec::with_capacity(txids.len());
        for (metadata, (tx_inputs, tx_outputs)) in
            metadatas.into_iter().zip(inputs.into_iter().zip(outputs))
        {
            txs.push(Transaction {
                version: metadata.version,
                lock_time: metadata.lock_time,
                input: tx_inputs,
                output: tx_outputs,
            });
        }
        Ok(txs)
    }

    /// Get transactions by blockhash index for sharechain transactions
    pub(crate) fn get_txs_by_blockhash_index(
        &self,
        blockhash: &BlockHash,
    ) -> Result<Vec<ShareTransaction>, StoreError> {
        let txids = self.get_txids_for_blockhash(blockhash);
        let txs = self.get_txs(&txids.0)?;
        let mut share_txs = Vec::with_capacity(txs.len());
        for tx in txs {
            share_txs.push(ShareTransaction(tx));
        }
        Ok(share_txs)
    }

    /// Re-validate one about-to-be-confirmed block's prevouts against committed
    /// state adjusted by `overlay`.
    ///
    /// Returns `Ok(true)` if the block may be confirmed, `Ok(false)` if a
    /// prevout's source has left the confirmed chain or the prevout is now
    /// double-spent. A prune-zone block without body data has no prevouts to
    /// check and returns `Ok(true)` (mirrors `put_confirmed_entry`). Coinbase
    /// maturity is not re-checked here: it is made deterministic at ingest
    /// against the block's own height (see `validate_prevouts`).
    pub(crate) fn recheck_block_prevouts_with_overlay(
        &self,
        blockhash: &BlockHash,
        overlay: &ConfirmationOverlay,
    ) -> Result<bool, StoreError> {
        if !self.share_block_exists(blockhash) {
            return Ok(true);
        }
        let transactions = self.get_txs_by_blockhash_index(blockhash)?;
        let SpendingPrevouts {
            all_outpoints,
            external_source_txids,
        } = extract_spending_prevouts(&transactions).map_err(|duplicate| {
            StoreError::Database(format!("Confirming block {blockhash} has {duplicate}"))
        })?;
        if !self.are_all_txids_confirmed_with_overlay(
            &external_source_txids,
            &overlay.removed_blockhashes,
            &overlay.confirmed_in_batch,
        )? {
            return Ok(false);
        }
        if self.is_any_prevout_spent_with_overlay(
            &all_outpoints,
            &overlay.removed_spends,
            &overlay.spent_in_batch,
        )? {
            return Ok(false);
        }
        Ok(true)
    }

    /// Record every outpoint spent by `blockhash`'s non-coinbase inputs into
    /// `spent_in_batch`, mirroring `add_spends_for_block` so the overlay tracks
    /// the same spends the batch will write.
    pub(crate) fn accumulate_spent_outpoints(
        &self,
        blockhash: &BlockHash,
        spent_in_batch: &mut HashSet<OutPoint>,
    ) -> Result<(), StoreError> {
        if !self.share_block_exists(blockhash) {
            return Ok(());
        }
        for share_transaction in self.get_txs_by_blockhash_index(blockhash)? {
            let transaction = &share_transaction.0;
            if !transaction.is_coinbase() {
                for input in &transaction.input {
                    spent_in_batch.insert(input.previous_output);
                }
            }
        }
        Ok(())
    }

    /// Walk `blocks` in confirmation order, re-checking each block's prevouts
    /// against the running `overlay`, and return `(prefix_len, first_invalid)`.
    ///
    /// The prefix is the leading run of blocks that pass the re-check; the walk
    /// stops at the first block that fails and reports its hash. Each accepted
    /// block's spends are folded into `overlay.spent_in_batch` so a later block
    /// double-spending an earlier one is caught.
    pub(crate) fn confirmable_prefix(
        &self,
        blocks: &[(u32, BlockHash)],
        overlay: &mut ConfirmationOverlay,
    ) -> Result<(usize, Option<BlockHash>), StoreError> {
        let mut prefix_len = 0;
        let mut first_invalid = None;
        while prefix_len < blocks.len() && first_invalid.is_none() {
            let (_, blockhash) = &blocks[prefix_len];
            if self.recheck_block_prevouts_with_overlay(blockhash, overlay)? {
                self.accumulate_spent_outpoints(blockhash, &mut overlay.spent_in_batch)?;
                overlay.confirmed_in_batch.insert(*blockhash);
                prefix_len += 1;
            } else {
                first_invalid = Some(*blockhash);
            }
        }
        Ok((prefix_len, first_invalid))
    }
}

#[cfg(test)]
mod tests;
