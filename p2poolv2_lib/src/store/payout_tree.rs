// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Bitcoin Cash payout trees of the blocks the pool found.
//!
//! A found block whose coinbase pays some payees through a payout tree is
//! worth nothing to them until the tree's node transactions are broadcast
//! after maturity. Every node keeps each such tree, keyed by the coinbase
//! txid, before submitting the block, with validation as a second source,
//! whatever the share's later status: the tree is fixed in the BCH block. The
//! payees and their amounts are enough to rebuild every node transaction
//! (`PayoutTree::build`); the API serves them so payees can keep their own
//! copy. A record of a block BCHN still does not know after
//! `UNKNOWN_BLOCK_TIP_LIMIT` new tips, counted only while BCHN is out of
//! initial block download, is removed: on a test network a share can claim a
//! found block with a minimum-difficulty header no BCHN ever accepts. Known
//! blocks and executed trees are retained (about 45 bytes per payee).

use super::{ColumnFamily, Store, writer::StoreError};
use crate::accounting::payout::payout_tree::{PayoutTree, PayoutTreeError, TreePayee};
use bitcoin::consensus::encode::{self, Decodable, Encodable, VarInt};
use bitcoin::hashes::Hash;
use bitcoin::{BlockHash, OutPoint, ScriptBuf, Txid};
use rocksdb::WriteOptions;

/// New BCH tips, outside initial block download, at which BCHN may not
/// know a kept tree's block before the record is removed. A real block
/// reaches BCHN well within them: the finder submits it before it relays the
/// share, and BCH relays a block in seconds.
pub const UNKNOWN_BLOCK_TIP_LIMIT: u32 = 6;

/// A payout tree of a found block, as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredPayoutTree {
    /// Txid of the coinbase holding the tree's root output.
    pub coinbase_txid: Txid,
    /// Index of the root output in the coinbase.
    pub root_vout: u32,
    /// Hash of the BCH block the coinbase is in.
    pub bitcoin_blockhash: BlockHash,
    /// Height of that block.
    pub bitcoin_height: u64,
    /// The share whose header carries the block.
    pub share_blockhash: BlockHash,
    /// The tree's payees after dust merging, in canonical order.
    pub payees: Vec<TreePayee>,
    /// Whether this node has seen every node transaction confirmed and
    /// stopped broadcasting it.
    pub executed: bool,
    /// New BCH tips at which BCHN did not know the purported found block.
    unknown_block_tips: u32,
    /// Last tip counted as unknown, so restarting at that tip counts once.
    last_unknown_tip: BlockHash,
}

impl StoredPayoutTree {
    /// The record of `tree`, whose root is output `root_vout` of the coinbase
    /// `coinbase_txid` in block `bitcoin_blockhash` at `bitcoin_height`,
    /// carried by share `share_blockhash`.
    pub fn new(
        tree: &PayoutTree,
        coinbase_txid: Txid,
        root_vout: u32,
        bitcoin_blockhash: BlockHash,
        bitcoin_height: u64,
        share_blockhash: BlockHash,
    ) -> Self {
        Self {
            coinbase_txid,
            root_vout,
            bitcoin_blockhash,
            bitcoin_height,
            share_blockhash,
            payees: tree.payees().to_vec(),
            executed: false,
            unknown_block_tips: 0,
            last_unknown_tip: BlockHash::all_zeros(),
        }
    }

    /// Rebuild the tree. Its payees are already merged, so this is the tree
    /// the coinbase committed to.
    pub fn tree(&self) -> Result<PayoutTree, PayoutTreeError> {
        PayoutTree::build(self.payees.clone())
    }

    /// The coinbase output the tree's root spends.
    pub fn root_outpoint(&self) -> OutPoint {
        OutPoint::new(self.coinbase_txid, self.root_vout)
    }
}

impl Encodable for StoredPayoutTree {
    fn consensus_encode<W: bitcoin::io::Write + ?Sized>(
        &self,
        writer: &mut W,
    ) -> Result<usize, bitcoin::io::Error> {
        let mut length = 0;
        length += self.coinbase_txid.consensus_encode(writer)?;
        length += self.root_vout.consensus_encode(writer)?;
        length += self.bitcoin_blockhash.consensus_encode(writer)?;
        length += self.bitcoin_height.consensus_encode(writer)?;
        length += self.share_blockhash.consensus_encode(writer)?;
        length += VarInt::from(self.payees.len()).consensus_encode(writer)?;
        for payee in &self.payees {
            length += payee.script_pubkey.consensus_encode(writer)?;
            length += payee.amount.consensus_encode(writer)?;
        }
        length += self.executed.consensus_encode(writer)?;
        length += self.unknown_block_tips.consensus_encode(writer)?;
        length += self.last_unknown_tip.consensus_encode(writer)?;
        Ok(length)
    }
}

impl Decodable for StoredPayoutTree {
    fn consensus_decode<R: bitcoin::io::Read + ?Sized>(
        reader: &mut R,
    ) -> Result<Self, encode::Error> {
        let coinbase_txid = Txid::consensus_decode(reader)?;
        let root_vout = u32::consensus_decode(reader)?;
        let bitcoin_blockhash = BlockHash::consensus_decode(reader)?;
        let bitcoin_height = u64::consensus_decode(reader)?;
        let share_blockhash = BlockHash::consensus_decode(reader)?;
        let count = VarInt::consensus_decode(reader)?.0;
        let mut payees = Vec::with_capacity(count.min(1 << 16) as usize);
        for _ in 0..count {
            payees.push(TreePayee {
                script_pubkey: ScriptBuf::consensus_decode(reader)?,
                amount: u64::consensus_decode(reader)?,
            });
        }
        let executed = bool::consensus_decode(reader)?;
        let unknown_block_tips = u32::consensus_decode(reader)?;
        let last_unknown_tip = BlockHash::consensus_decode(reader)?;
        Ok(Self {
            coinbase_txid,
            root_vout,
            bitcoin_blockhash,
            bitcoin_height,
            share_blockhash,
            payees,
            executed,
            unknown_block_tips,
            last_unknown_tip,
        })
    }
}

impl Store {
    /// Count a new BCH tip at which the block of a kept payout tree is
    /// unknown. Remove the record at the limit, returning whether removed.
    /// Counts and the last observed tip persist across executor restarts.
    pub(crate) fn record_unknown_payout_tree_tip(
        &self,
        coinbase_txid: &Txid,
        bitcoin_tip: BlockHash,
    ) -> Result<bool, StoreError> {
        let Some(mut tree) = self.get_payout_tree(coinbase_txid)? else {
            return Ok(false);
        };
        if tree.executed || tree.last_unknown_tip == bitcoin_tip {
            return Ok(false);
        }
        tree.last_unknown_tip = bitcoin_tip;
        tree.unknown_block_tips = tree.unknown_block_tips.saturating_add(1);
        let column_family = self.db.cf_handle(&ColumnFamily::PayoutTrees).unwrap();
        let key = coinbase_txid.to_byte_array();
        if tree.unknown_block_tips >= UNKNOWN_BLOCK_TIP_LIMIT {
            self.db.delete_cf(&column_family, key)?;
            Ok(true)
        } else {
            self.db
                .put_cf(&column_family, key, encode::serialize(&tree))?;
            Ok(false)
        }
    }

    /// Keep a found block's payout tree. A tree already kept, perhaps marked
    /// executed, is left as it is: the same coinbase has the same tree. Sync
    /// the WAL before acknowledging a new record, before block submission.
    pub(crate) fn put_payout_tree(&self, tree: &StoredPayoutTree) -> Result<(), StoreError> {
        let column_family = self.db.cf_handle(&ColumnFamily::PayoutTrees).unwrap();
        let key = tree.coinbase_txid.to_byte_array();
        if self.db.get_cf(&column_family, key)?.is_some() {
            return Ok(());
        }
        let mut options = WriteOptions::default();
        options.set_sync(true);
        self.db
            .put_cf_opt(&column_family, key, encode::serialize(tree), &options)?;
        Ok(())
    }

    /// Mark the tree of `coinbase_txid` executed, so the executor stops
    /// broadcasting it. A tree not kept is an error.
    pub(crate) fn mark_payout_tree_executed(&self, coinbase_txid: &Txid) -> Result<(), StoreError> {
        let mut tree = self.get_payout_tree(coinbase_txid)?.ok_or_else(|| {
            StoreError::NotFound(format!("No payout tree for coinbase {coinbase_txid}"))
        })?;
        tree.executed = true;
        let column_family = self.db.cf_handle(&ColumnFamily::PayoutTrees).unwrap();
        self.db.put_cf(
            &column_family,
            coinbase_txid.to_byte_array(),
            encode::serialize(&tree),
        )?;
        Ok(())
    }

    /// The payout tree of the coinbase `coinbase_txid`, if kept.
    pub fn get_payout_tree(
        &self,
        coinbase_txid: &Txid,
    ) -> Result<Option<StoredPayoutTree>, StoreError> {
        let column_family = self.db.cf_handle(&ColumnFamily::PayoutTrees).unwrap();
        self.db
            .get_cf(&column_family, coinbase_txid.to_byte_array())?
            .map(|value| encode::deserialize(&value).map_err(StoreError::from))
            .transpose()
    }

    /// Every kept payout tree, ordered by coinbase txid.
    pub fn get_payout_trees(&self) -> Result<Vec<StoredPayoutTree>, StoreError> {
        let column_family = self.db.cf_handle(&ColumnFamily::PayoutTrees).unwrap();
        let mut trees = Vec::new();
        for item in self
            .db
            .iterator_cf(&column_family, rocksdb::IteratorMode::Start)
        {
            let (_, value) = item?;
            trees.push(encode::deserialize(&value)?);
        }
        Ok(trees)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::PubkeyHash;
    use tempfile::tempdir;

    fn stored(byte: u8) -> StoredPayoutTree {
        let payees: Vec<TreePayee> = (1..=3u8)
            .map(|number| TreePayee {
                script_pubkey: ScriptBuf::new_p2pkh(&PubkeyHash::from_byte_array([number; 20])),
                amount: 100_000 * u64::from(number),
            })
            .collect();
        let tree = PayoutTree::build(payees).unwrap();
        StoredPayoutTree::new(
            &tree,
            Txid::from_byte_array([byte; 32]),
            4,
            BlockHash::from_byte_array([0x22; 32]),
            900_000,
            BlockHash::from_byte_array([0x33; 32]),
        )
    }

    #[test]
    fn a_stored_tree_round_trips_and_rebuilds_its_tree() {
        let tree = stored(1);
        let decoded: StoredPayoutTree = encode::deserialize(&encode::serialize(&tree)).unwrap();
        assert_eq!(decoded, tree);
        assert_eq!(decoded.tree().unwrap().payees(), tree.payees.as_slice());
        assert_eq!(decoded.root_outpoint().vout, 4);
    }

    #[test]
    fn a_truncated_record_is_a_store_error() {
        let mut encoded = encode::serialize(&stored(1));
        encoded.pop();

        assert!(encode::deserialize::<StoredPayoutTree>(&encoded).is_err());
    }

    #[test]
    fn an_unknown_block_record_expires_after_six_distinct_tips_across_restarts() {
        let directory = tempdir().unwrap();
        let path = directory.path().to_str().unwrap().to_string();
        let store = Store::new(path.clone(), false).unwrap();
        let tree = stored(1);
        let coinbase_txid = tree.coinbase_txid;
        store.put_payout_tree(&tree).unwrap();
        assert!(
            !store
                .record_unknown_payout_tree_tip(&coinbase_txid, BlockHash::from_byte_array([1; 32]))
                .unwrap()
        );
        // Another copy of the same tree cannot reset its expiry metadata.
        store.put_payout_tree(&tree).unwrap();
        drop(store);
        let store = Store::new(path, false).unwrap();
        assert!(
            !store
                .record_unknown_payout_tree_tip(&coinbase_txid, BlockHash::from_byte_array([1; 32]))
                .unwrap()
        );
        assert_eq!(
            store
                .get_payout_tree(&coinbase_txid)
                .unwrap()
                .unwrap()
                .unknown_block_tips,
            1
        );
        assert!(
            !store
                .record_unknown_payout_tree_tip(&coinbase_txid, BlockHash::from_byte_array([2; 32]))
                .unwrap()
        );
        assert!(
            !store
                .record_unknown_payout_tree_tip(&coinbase_txid, BlockHash::from_byte_array([3; 32]))
                .unwrap()
        );
        assert!(
            !store
                .record_unknown_payout_tree_tip(&coinbase_txid, BlockHash::from_byte_array([4; 32]))
                .unwrap()
        );
        assert!(
            !store
                .record_unknown_payout_tree_tip(&coinbase_txid, BlockHash::from_byte_array([5; 32]))
                .unwrap()
        );
        assert!(
            store
                .record_unknown_payout_tree_tip(&coinbase_txid, BlockHash::from_byte_array([6; 32]))
                .unwrap()
        );
        assert!(store.get_payout_tree(&coinbase_txid).unwrap().is_none());
        assert!(store.get_payout_trees().unwrap().is_empty());
    }

    #[test]
    fn trees_are_kept_once_and_marked_executed() {
        let directory = tempdir().unwrap();
        let store = Store::new(directory.path().to_str().unwrap().to_string(), false).unwrap();
        let first = stored(1);
        let second = stored(2);

        store.put_payout_tree(&first).unwrap();
        store.put_payout_tree(&second).unwrap();
        store
            .mark_payout_tree_executed(&first.coinbase_txid)
            .unwrap();
        store.put_payout_tree(&first).unwrap();

        let kept = store.get_payout_trees().unwrap();
        assert_eq!(kept.len(), 2);
        assert!(
            store
                .get_payout_tree(&first.coinbase_txid)
                .unwrap()
                .unwrap()
                .executed
        );
        assert!(!kept[1].executed);
        assert!(
            store
                .mark_payout_tree_executed(&Txid::from_byte_array([9; 32]))
                .is_err()
        );
    }
}
