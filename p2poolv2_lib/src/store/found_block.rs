// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Claimed Bitcoin Cash block candidates and BCHN-accepted pool blocks.
//!
//! Share confirmation records headers meeting their own claimed targets in
//! the FoundBlocks column family. These are unverified candidates: they are
//! never exposed as found blocks until a trusted caller verifies BCHN
//! acceptance and records them separately in AcceptedFoundBlocks. Neither record changes share
//! confirmation, payout eligibility or any share-chain rule.

use super::{ColumnFamily, Store, writer::StoreError};
use crate::shares::share_block::ShareHeader;
use bitcoin::BlockHash;
use bitcoin::consensus::encode::{self, Decodable, Encodable};

/// Size of the big endian bitcoin height prefix in a block record key.
const HEIGHT_KEY_PREFIX_LENGTH: usize = 8;

/// A Bitcoin Cash block candidate, accepted only after BCHN verification.
///
/// Candidate and accepted column families use the same key and value
/// encoding. The key is the big endian Bitcoin Cash height followed by the
/// blockhash, so iteration is ordered by height and two blocks at the same
/// height do not collide. The value carries the remaining fields.
#[derive(Debug, Clone, PartialEq)]
pub struct FoundBlock {
    /// Bitcoin height of the found block
    pub bitcoin_height: u64,
    /// Hash of the found bitcoin block
    pub bitcoin_blockhash: BlockHash,
    /// Hash of the share whose header carries the bitcoin block
    pub share_blockhash: BlockHash,
    /// Bitcoin header time in seconds since the epoch
    pub bitcoin_time: u32,
    /// Bitcoin address of the miner that found the block
    pub miner_bitcoin_address: String,
}

/// The value half of a FoundBlocks entry, encoded with bitcoin consensus
/// encoding.
struct FoundBlockValue {
    share_blockhash: BlockHash,
    bitcoin_time: u32,
    miner_bitcoin_address: String,
}

impl Encodable for FoundBlockValue {
    fn consensus_encode<W: bitcoin::io::Write + ?Sized>(
        &self,
        writer: &mut W,
    ) -> Result<usize, bitcoin::io::Error> {
        let mut length = 0;
        length += self.share_blockhash.consensus_encode(writer)?;
        length += self.bitcoin_time.consensus_encode(writer)?;
        length += self.miner_bitcoin_address.consensus_encode(writer)?;
        Ok(length)
    }
}

impl Decodable for FoundBlockValue {
    fn consensus_decode<R: bitcoin::io::Read + ?Sized>(
        reader: &mut R,
    ) -> Result<Self, encode::Error> {
        Ok(FoundBlockValue {
            share_blockhash: BlockHash::consensus_decode(reader)?,
            bitcoin_time: u32::consensus_decode(reader)?,
            miner_bitcoin_address: String::consensus_decode(reader)?,
        })
    }
}

/// Build the FoundBlocks key: big endian bitcoin height, then blockhash.
fn found_block_key(bitcoin_height: u64, bitcoin_blockhash: &BlockHash) -> Vec<u8> {
    let mut key = Vec::with_capacity(HEIGHT_KEY_PREFIX_LENGTH + 32);
    key.extend_from_slice(&bitcoin_height.to_be_bytes());
    key.extend_from_slice(&encode::serialize(bitcoin_blockhash));
    key
}

impl Store {
    /// Record claimed Bitcoin Cash block candidates from a confirmed share.
    ///
    /// Checks the share's header and the headers of each of its uncles,
    /// and records every one meeting its header's claimed target. This does
    /// not establish BCHN acceptance or expose a found block. Only
    /// header data is read, so this works for blocks promoted header-only
    /// below the prune height.
    ///
    /// Headers are read from the committed store, so the share and its
    /// uncles must have been stored in an earlier batch. A missing header
    /// is an error because confirmation requires it.
    pub(crate) fn record_found_blocks(
        &self,
        blockhash: &BlockHash,
        batch: &mut rocksdb::WriteBatch,
    ) -> Result<(), StoreError> {
        let header = self.get_required_share_header(blockhash)?;
        for uncle_hash in &header.uncles {
            let uncle_header = self.get_required_share_header(uncle_hash)?;
            self.put_found_block_if_bitcoin_block(&uncle_header, batch);
        }
        self.put_found_block_if_bitcoin_block(&header, batch);
        Ok(())
    }

    /// Read a share header, returning NotFound if it is absent.
    fn get_required_share_header(&self, blockhash: &BlockHash) -> Result<ShareHeader, StoreError> {
        self.get_share_header(blockhash)?
            .ok_or_else(|| StoreError::NotFound(format!("Share header not found for {blockhash}")))
    }

    /// Add an unverified FoundBlocks candidate when `header` meets the
    /// target claimed by its Bitcoin Cash header.
    ///
    /// The entry is derived from the header alone, so writing it again
    /// when a share is re-confirmed after a reorg writes identical bytes.
    fn put_found_block_if_bitcoin_block(
        &self,
        header: &ShareHeader,
        batch: &mut rocksdb::WriteBatch,
    ) {
        if !header.meets_bitcoin_difficulty() {
            return;
        }
        let column_family = self.db.cf_handle(&ColumnFamily::FoundBlocks).unwrap();
        let key = found_block_key(header.bitcoin_height, &header.bitcoin_header.block_hash());
        let value = FoundBlockValue {
            share_blockhash: header.block_hash(),
            bitcoin_time: header.bitcoin_header.time,
            miner_bitcoin_address: header.miner_bitcoin_address.to_string(),
        };
        batch.put_cf(&column_family, key, encode::serialize(&value));
    }

    /// Record a BCHN-accepted block, returning whether it was newly inserted.
    ///
    /// The caller must verify a successful BCHN submission or membership in
    /// BCHN's chain before calling. StoreWriter serializes the existence
    /// check and write so duplicate submissions cannot count a find twice.
    /// The share need not be persisted yet when our own submission succeeds.
    pub(crate) fn record_accepted_found_block(
        &self,
        found_block: &FoundBlock,
    ) -> Result<bool, StoreError> {
        let column_family = self
            .db
            .cf_handle(&ColumnFamily::AcceptedFoundBlocks)
            .unwrap();
        let key = found_block_key(found_block.bitcoin_height, &found_block.bitcoin_blockhash);
        if self.db.get_pinned_cf(&column_family, &key)?.is_some() {
            return Ok(false);
        }
        let value = FoundBlockValue {
            share_blockhash: found_block.share_blockhash,
            bitcoin_time: found_block.bitcoin_time,
            miner_bitcoin_address: found_block.miner_bitcoin_address.clone(),
        };
        let mut batch = Self::get_write_batch();
        batch.put_cf(&column_family, key, encode::serialize(&value));
        self.commit_batch(batch)?;
        Ok(true)
    }

    /// Return BCHN-accepted pool blocks, ordered by Bitcoin Cash height.
    pub fn get_found_blocks(&self) -> Result<Vec<FoundBlock>, StoreError> {
        let column_family = self
            .db
            .cf_handle(&ColumnFamily::AcceptedFoundBlocks)
            .unwrap();
        let mut found_blocks = Vec::new();
        for item in self
            .db
            .iterator_cf(&column_family, rocksdb::IteratorMode::Start)
        {
            let (key, value) = item?;
            found_blocks.push(decode_found_block(&key, &value)?);
        }
        Ok(found_blocks)
    }

    /// Return claimed block candidates still awaiting BCHN verification.
    ///
    /// A FoundBlocks entry never establishes BCHN acceptance. Accepted keys
    /// are skipped so periodic verification does not query them again.
    pub fn get_candidate_found_blocks(&self) -> Result<Vec<FoundBlock>, StoreError> {
        let candidates = self.db.cf_handle(&ColumnFamily::FoundBlocks).unwrap();
        let accepted = self
            .db
            .cf_handle(&ColumnFamily::AcceptedFoundBlocks)
            .unwrap();
        let mut found_blocks = Vec::new();
        for item in self
            .db
            .iterator_cf(&candidates, rocksdb::IteratorMode::Start)
        {
            let (key, value) = item?;
            let is_accepted = self.db.get_pinned_cf(&accepted, &key)?.is_some();
            if !is_accepted {
                found_blocks.push(decode_found_block(&key, &value)?);
            }
        }
        Ok(found_blocks)
    }
}

/// Decode a FoundBlocks entry from its key and value bytes.
fn decode_found_block(key: &[u8], value: &[u8]) -> Result<FoundBlock, StoreError> {
    let (height_bytes, blockhash_bytes) = key
        .split_at_checked(HEIGHT_KEY_PREFIX_LENGTH)
        .ok_or_else(|| StoreError::Serialization(format!("Found block key too short: {key:?}")))?;
    let bitcoin_height = u64::from_be_bytes(height_bytes.try_into().map_err(|_| {
        StoreError::Serialization(format!("Invalid found block height: {height_bytes:?}"))
    })?);
    let bitcoin_blockhash: BlockHash = encode::deserialize(blockhash_bytes)?;
    let decoded_value: FoundBlockValue = encode::deserialize(value)?;
    Ok(FoundBlock {
        bitcoin_height,
        bitcoin_blockhash,
        share_blockhash: decoded_value.share_blockhash,
        bitcoin_time: decoded_value.bitcoin_time,
        miner_bitcoin_address: decoded_value.miner_bitcoin_address,
    })
}

#[cfg(test)]
mod tests;
