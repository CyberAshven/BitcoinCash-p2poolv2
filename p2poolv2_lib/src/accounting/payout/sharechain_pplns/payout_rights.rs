// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Payout rights on Bitcoin Cash: who holds each share's PPLNS payout.
//!
//! Every share carries one payout right, which starts on the share's
//! coinbase output, held by the share's own `miner_address`. A share
//! transaction that spends an output carrying a right through input `i`
//! passes the right to its output `i`; a transaction with no output `i`
//! leaves the right with its holder, where it can no longer move. A right on
//! a P2PKH output is held by that output's key hash; a right on a P2SH32
//! output (the share-trading contract) keeps the holder it had, so a seller
//! holds a locked share until the claim or the refund.
//!
//! [`PayoutRights`] is the index the PPLNS window keeps of these rights for
//! the shares it caches. A right still on its coinbase output has no entry;
//! the index records only rights that moved, so it holds at most one entry
//! per cached share. See `docs/atomic-swap/bch-share-trading.md`, "Payout
//! rights", for the rule and why it pairs inputs with outputs.

use crate::shares::bch_script::p2pkh_hash;
use crate::shares::share_block::{ShareHeader, ShareTransaction};
use crate::shares::transactions::coinbase::{
    build_sharechain_coinbase_for_witness_root, compute_witness_root,
};
use bitcoin::{BlockHash, OutPoint, Transaction, TxOut, Txid};
use p2poolv2_wallet::share_owner::ShareOwner;
use std::collections::{HashMap, VecDeque};

/// Rule tag Bitcoin Cash protocol strings carry for paying a share's PPLNS
/// weight to the holder of its payout right, so nodes on the rule and nodes
/// paying `miner_bitcoin_address` fail the handshake instead of rebuilding
/// different coinbases for the same share.
pub const BCH_PAYOUT_HOLDER_RULE_TAG: &str = "payout-holder";

/// Most newest confirmed shares whose changes to the index can be undone.
///
/// A confirmed reorg within the undo log restores the index share by share;
/// a deeper one reloads it from the store. A payout anchor forking deeper
/// below the cached tip than the log reaches cannot be resolved. 1,210 is the
/// PPLNS window's eviction buffer at production sizes, the deepest anchor the
/// window itself serves. A local resource choice, not a consensus rule.
pub(crate) const RIGHTS_UNDO_DEPTH: usize = 1_210;

/// Most changed entries the undo log keeps across all its shares. The log
/// drops its oldest shares past this, so it costs at most about 10 MB however
/// many rights the newest shares move; while shares move few rights it
/// reaches the full `RIGHTS_UNDO_DEPTH`.
pub(crate) const RIGHTS_UNDO_MAX_CHANGES: usize = 100_000;

/// Where a right that left its share's coinbase output is, and who holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MovedRight {
    /// The output carrying the right. Spent when the right is frozen.
    pub(crate) location: OutPoint,
    /// Who the share's weight is paid to.
    pub(crate) holder: ShareOwner,
    /// The share's own `miner_address`, the right's first holder.
    pub(crate) owner: ShareOwner,
}

/// A share on the chain the index answers for, found by its coinbase txid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChainShare {
    pub(crate) blockhash: BlockHash,
    pub(crate) height: u32,
    /// The share's own `miner_address`.
    pub(crate) owner: ShareOwner,
}

impl ChainShare {
    /// The share a header describes, at `height`.
    pub(crate) fn from_header(blockhash: BlockHash, height: u32, header: &ShareHeader) -> Self {
        Self {
            blockhash,
            height,
            owner: header.miner_address,
        }
    }
}

/// One right a transaction moved, with its entry before the move.
#[derive(Debug, Clone, Copy)]
struct RightChange {
    share: BlockHash,
    previous: Option<MovedRight>,
}

/// What applying one confirmed share did to the index, so a reorg can undo
/// it.
#[derive(Debug)]
struct ShareUndo {
    blockhash: BlockHash,
    coinbase_txid: Txid,
    changes: Vec<RightChange>,
}

/// The overlay or index could not be built back to an anchor's fork point:
/// its undo log does not reach that deep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct UndoTooShallow {
    pub(crate) requested: usize,
    pub(crate) available: usize,
}

/// The txid of the coinbase a share header implies.
///
/// A share's coinbase is a function of its header: the owner, the bitcoin
/// header's hash (the weak block hash in its input) and the share witness
/// root (`build_sharechain_coinbase_for_witness_root`, which body validation
/// also rebuilds). So the index can recognise a spend of a share's coinbase
/// output without reading the share's body.
pub(crate) fn share_coinbase_txid(header: &ShareHeader) -> Txid {
    build_sharechain_coinbase_for_witness_root(
        &header.miner_address,
        header.bitcoin_header.block_hash(),
        header.coinbase_proof.share_witness_root,
    )
    .compute_txid()
}

/// Whether a share header commits to share transactions besides its
/// coinbase, so its body must be read to apply them.
pub(crate) fn has_share_transactions(header: &ShareHeader) -> bool {
    header.coinbase_proof.share_witness_root != compute_witness_root(&[])
}

/// The holder a right gets on `output`: the key hash of a P2PKH output, or
/// `None` for any other output, where the right keeps its holder.
fn output_holder(output: &TxOut) -> Option<ShareOwner> {
    p2pkh_hash(output.script_pubkey.as_bytes()).map(|hash| ShareOwner::CashP2pkh(*hash))
}

/// Read and write access to a set of rights, shared by the index and the
/// per-anchor overlay so both move rights by the same code.
trait RightsState {
    /// The moved right of `share`, or `None` while it is on its coinbase
    /// output.
    fn moved_right(&self, share: &BlockHash) -> Option<MovedRight>;
    /// The share whose moved right sits on `outpoint`, if any.
    fn moved_right_at(&self, outpoint: &OutPoint) -> Option<(BlockHash, MovedRight)>;
    /// The chain share whose coinbase has `txid`.
    fn coinbase_share(&self, txid: &Txid) -> Option<ChainShare>;
    /// Record `right` as `share`'s moved right.
    fn set_moved_right(&mut self, share: BlockHash, right: MovedRight);
}

/// The right `outpoint` carries: a moved right sitting there, or the right
/// of the share whose coinbase output it is, while that right has not left.
fn right_at(state: &impl RightsState, outpoint: &OutPoint) -> Option<(BlockHash, MovedRight)> {
    if let Some(moved) = state.moved_right_at(outpoint) {
        return Some(moved);
    }
    if outpoint.vout != 0 {
        return None;
    }
    let chain_share = state.coinbase_share(&outpoint.txid)?;
    match state.moved_right(&chain_share.blockhash) {
        // The coinbase output was spent already, on a branch whose later
        // block spends it again: that branch is invalid, and the right
        // stays where the first spend put it.
        Some(_) => None,
        None => Some((
            chain_share.blockhash,
            MovedRight {
                location: *outpoint,
                holder: chain_share.owner,
                owner: chain_share.owner,
            },
        )),
    }
}

/// Move the rights `transaction` spends, input `i` to output `i`, and return
/// what changed.
///
/// A right whose input has no matching output stays where it is: on a spent
/// output, with its holder, frozen.
fn apply_transaction(state: &mut impl RightsState, transaction: &Transaction) -> Vec<RightChange> {
    let mut changes = Vec::new();
    let mut txid: Option<Txid> = None;
    for (input_index, input) in transaction.input.iter().enumerate() {
        if let Some((share, current)) = right_at(state, &input.previous_output)
            && let Some(output) = transaction.output.get(input_index)
        {
            let spending_txid = *txid.get_or_insert_with(|| transaction.compute_txid());
            let next = MovedRight {
                location: OutPoint::new(spending_txid, input_index as u32),
                holder: output_holder(output).unwrap_or(current.holder),
                owner: current.owner,
            };
            changes.push(RightChange {
                share,
                previous: state.moved_right(&share),
            });
            state.set_moved_right(share, next);
        }
    }
    changes
}

/// Move the rights every non-coinbase transaction of a share spends, in
/// block order.
fn apply_share_transactions(
    state: &mut impl RightsState,
    transactions: &[ShareTransaction],
) -> Vec<RightChange> {
    let mut changes = Vec::new();
    for transaction in transactions {
        if !transaction.is_coinbase() {
            changes.extend(apply_transaction(state, transaction));
        }
    }
    changes
}

/// How much undo history the index keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct UndoLimits {
    /// Most shares with an undo record.
    pub(crate) shares: usize,
    /// Most changed entries across those records.
    pub(crate) changes: usize,
}

impl Default for UndoLimits {
    fn default() -> Self {
        Self {
            shares: RIGHTS_UNDO_DEPTH,
            changes: RIGHTS_UNDO_MAX_CHANGES,
        }
    }
}

/// The index of payout rights for the shares a PPLNS window caches, at the
/// window's cached confirmed tip.
///
/// Bounded by the cache: one coinbase entry per cached share, at most one
/// moved right per cached share and its reverse entry, and undo records for
/// the newest shares, at most [`RIGHTS_UNDO_DEPTH`] of them and
/// [`RIGHTS_UNDO_MAX_CHANGES`] changed entries in all (one share moves at
/// most one right per input).
#[derive(Debug, Default)]
pub(crate) struct PayoutRights {
    /// Rights that left their share's coinbase output.
    moved: HashMap<BlockHash, MovedRight>,
    /// The share whose moved right sits on each outpoint.
    by_location: HashMap<OutPoint, BlockHash>,
    /// Cached shares by coinbase txid.
    coinbases: HashMap<Txid, ChainShare>,
    /// Undo records of the newest applied shares, newest first.
    undo: VecDeque<ShareUndo>,
    /// Changed entries across `undo`.
    undo_changes: usize,
    undo_limits: UndoLimits,
}

impl RightsState for PayoutRights {
    fn moved_right(&self, share: &BlockHash) -> Option<MovedRight> {
        self.moved.get(share).copied()
    }

    fn moved_right_at(&self, outpoint: &OutPoint) -> Option<(BlockHash, MovedRight)> {
        let share = self.by_location.get(outpoint)?;
        self.moved
            .get(share)
            .filter(|right| right.location == *outpoint)
            .map(|right| (*share, *right))
    }

    fn coinbase_share(&self, txid: &Txid) -> Option<ChainShare> {
        self.coinbases.get(txid).copied()
    }

    fn set_moved_right(&mut self, share: BlockHash, right: MovedRight) {
        self.restore(share, Some(right));
    }
}

impl PayoutRights {
    /// An empty index keeping other undo history than the default, for tests.
    #[cfg(test)]
    pub(crate) fn with_undo_limits(undo_limits: UndoLimits) -> Self {
        Self {
            undo_limits,
            ..Self::default()
        }
    }

    /// Apply one newly confirmed share on top of the index: register its
    /// coinbase, then move the rights its transactions spend. Records how to
    /// undo it, dropping the oldest undo records past the limits.
    pub(crate) fn apply_share(
        &mut self,
        share: ChainShare,
        coinbase_txid: Txid,
        transactions: &[ShareTransaction],
    ) {
        self.coinbases.insert(coinbase_txid, share);
        let changes = apply_share_transactions(self, transactions);
        self.undo_changes += changes.len();
        self.undo.push_front(ShareUndo {
            blockhash: share.blockhash,
            coinbase_txid,
            changes,
        });
        self.limit_undo(self.undo_limits.shares);
    }

    /// Undo the newest applied share, which must be `blockhash`. Returns
    /// false, changing nothing, when its undo record is gone.
    pub(crate) fn undo_share(&mut self, blockhash: &BlockHash) -> bool {
        match self.undo.front() {
            Some(record) if record.blockhash == *blockhash => {}
            _ => return false,
        }
        let Some(record) = self.undo.pop_front() else {
            return false;
        };
        self.undo_changes -= record.changes.len();
        for change in record.changes.iter().rev() {
            self.restore(change.share, change.previous);
        }
        self.coinbases.remove(&record.coinbase_txid);
        true
    }

    /// How many of the newest shares can be undone.
    pub(crate) fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    /// Drop a share leaving the cache: its coinbase entry and its right.
    pub(crate) fn evict_share(&mut self, blockhash: &BlockHash, coinbase_txid: &Txid) {
        self.coinbases.remove(coinbase_txid);
        self.restore(*blockhash, None);
    }

    /// Keep undo records for at most `shares` shares, and within the limits,
    /// dropping the oldest. The window passes its cached share count, so no
    /// record refers to a share the cache has evicted.
    pub(crate) fn limit_undo(&mut self, shares: usize) {
        let most_shares = shares.min(self.undo_limits.shares);
        while self.undo.len() > most_shares
            || (self.undo.len() > 1 && self.undo_changes > self.undo_limits.changes)
        {
            if let Some(oldest) = self.undo.pop_back() {
                self.undo_changes -= oldest.changes.len();
            }
        }
    }

    /// Forget everything, for a reload from the store.
    pub(crate) fn clear(&mut self) {
        self.moved.clear();
        self.by_location.clear();
        self.coinbases.clear();
        self.undo.clear();
        self.undo_changes = 0;
    }

    /// The rights as of an anchor whose fork point is `undo_count` shares
    /// below the cached tip, at `fork_height`: the index with those shares
    /// undone. Apply the anchor's branch shares to the overlay next.
    pub(crate) fn overlay(
        &self,
        undo_count: usize,
        fork_height: u32,
    ) -> Result<RightsOverlay<'_>, UndoTooShallow> {
        if undo_count > self.undo.len() {
            return Err(UndoTooShallow {
                requested: undo_count,
                available: self.undo.len(),
            });
        }
        let mut overlay = RightsOverlay {
            base: self,
            fork_height,
            changed: HashMap::new(),
            located: HashMap::new(),
            branch_coinbases: HashMap::new(),
        };
        for record in self.undo.iter().take(undo_count) {
            for change in record.changes.iter().rev() {
                overlay.restore(change.share, change.previous);
            }
        }
        Ok(overlay)
    }

    /// Set `share`'s entry, keeping the reverse index in step.
    fn restore(&mut self, share: BlockHash, right: Option<MovedRight>) {
        let previous = match right {
            Some(right) => {
                self.by_location.insert(right.location, share);
                self.moved.insert(share, right)
            }
            None => self.moved.remove(&share),
        };
        if let Some(previous) = previous
            && right.is_none_or(|right| right.location != previous.location)
            && self.by_location.get(&previous.location) == Some(&share)
        {
            self.by_location.remove(&previous.location);
        }
    }
}

/// The payout rights as of one anchor: the index, with the cached shares
/// above the anchor's fork point undone and the anchor's branch shares
/// applied. Discarded after use; it never changes the index.
#[derive(Debug)]
pub(crate) struct RightsOverlay<'index> {
    base: &'index PayoutRights,
    /// Cached shares above this height are not the anchor's ancestors.
    fork_height: u32,
    /// Entries that differ from the index; `None` is a right back on its
    /// coinbase output.
    changed: HashMap<BlockHash, Option<MovedRight>>,
    /// Where the changed rights sit.
    located: HashMap<OutPoint, BlockHash>,
    /// The branch shares' coinbases.
    branch_coinbases: HashMap<Txid, ChainShare>,
}

impl RightsState for RightsOverlay<'_> {
    fn moved_right(&self, share: &BlockHash) -> Option<MovedRight> {
        match self.changed.get(share) {
            Some(entry) => *entry,
            None => self.base.moved_right(share),
        }
    }

    fn moved_right_at(&self, outpoint: &OutPoint) -> Option<(BlockHash, MovedRight)> {
        let located = self.located.get(outpoint).copied();
        let indexed = self.base.by_location.get(outpoint).copied();
        located.into_iter().chain(indexed).find_map(|share| {
            self.moved_right(&share)
                .filter(|right| right.location == *outpoint)
                .map(|right| (share, right))
        })
    }

    fn coinbase_share(&self, txid: &Txid) -> Option<ChainShare> {
        self.branch_coinbases.get(txid).copied().or_else(|| {
            self.base
                .coinbase_share(txid)
                .filter(|share| share.height <= self.fork_height)
        })
    }

    fn set_moved_right(&mut self, share: BlockHash, right: MovedRight) {
        self.restore(share, Some(right));
    }
}

impl RightsOverlay<'_> {
    /// Apply one branch share above the fork point: register its coinbase,
    /// then move the rights its transactions spend.
    pub(crate) fn apply_share(
        &mut self,
        share: ChainShare,
        coinbase_txid: Txid,
        transactions: &[ShareTransaction],
    ) {
        self.branch_coinbases.insert(coinbase_txid, share);
        apply_share_transactions(self, transactions);
    }

    /// Every share whose right is held by someone other than its own miner,
    /// with that holder's key hash.
    pub(crate) fn transferred_holders(&self) -> Vec<(BlockHash, [u8; 20])> {
        let mut holders = Vec::with_capacity(self.base.moved.len() + self.changed.len());
        let indexed = self
            .base
            .moved
            .keys()
            .filter(|share| !self.changed.contains_key(*share));
        for share in indexed.chain(self.changed.keys()) {
            if let Some(right) = self.moved_right(share)
                && right.holder != right.owner
                && let ShareOwner::CashP2pkh(hash) = right.holder
            {
                holders.push((*share, hash));
            }
        }
        holders
    }

    fn restore(&mut self, share: BlockHash, right: Option<MovedRight>) {
        if let Some(right) = right {
            self.located.insert(right.location, share);
        }
        self.changed.insert(share, right);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{TestShareBlockBuilder, TestShareHeaderBuilder};
    use bitcoin::hashes::Hash;
    use bitcoin::{Amount, ScriptBuf, Sequence, TxIn, Witness, absolute, transaction};
    use std::slice;

    const MINER: [u8; 20] = [0x11; 20];
    const SELLER: [u8; 20] = [0x22; 20];
    const BUYER: [u8; 20] = [0x33; 20];
    const OTHER_MINER: [u8; 20] = [0x44; 20];

    fn blockhash(byte: u8) -> BlockHash {
        BlockHash::from_byte_array([byte; 32])
    }

    fn coinbase_txid(byte: u8) -> Txid {
        Txid::from_byte_array([byte; 32])
    }

    fn chain_share(byte: u8, height: u32, owner: [u8; 20]) -> ChainShare {
        ChainShare {
            blockhash: blockhash(byte),
            height,
            owner: ShareOwner::CashP2pkh(owner),
        }
    }

    fn p2pkh_output(hash: [u8; 20]) -> TxOut {
        TxOut {
            value: Amount::from_sat(1_000),
            script_pubkey: ScriptBuf::new_p2pkh(&bitcoin::PubkeyHash::from_byte_array(hash)),
        }
    }

    fn p2sh32_output() -> TxOut {
        let mut script = vec![0xaa, 0x20];
        script.extend_from_slice(&[0x55; 32]);
        script.push(0x87);
        TxOut {
            value: Amount::from_sat(1_000),
            script_pubkey: ScriptBuf::from_bytes(script),
        }
    }

    /// A transaction spending `inputs` into `outputs`; scripts are not
    /// checked by the index.
    fn spend(inputs: &[OutPoint], outputs: Vec<TxOut>) -> ShareTransaction {
        ShareTransaction(Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: inputs
                .iter()
                .map(|outpoint| TxIn {
                    previous_output: *outpoint,
                    script_sig: ScriptBuf::new(),
                    sequence: Sequence::MAX,
                    witness: Witness::new(),
                })
                .collect(),
            output: outputs,
        })
    }

    fn coinbase_outpoint(byte: u8) -> OutPoint {
        OutPoint::new(coinbase_txid(byte), 0)
    }

    fn rights_with_share(byte: u8, owner: [u8; 20]) -> PayoutRights {
        let mut rights = PayoutRights::default();
        rights.apply_share(chain_share(byte, 1, owner), coinbase_txid(byte), &[]);
        rights
    }

    #[test]
    fn a_right_on_its_coinbase_output_has_no_entry() {
        let rights = rights_with_share(1, MINER);

        assert_eq!(rights.moved_right(&blockhash(1)), None);
        let overlay = rights.overlay(0, 1).unwrap();
        assert!(overlay.transferred_holders().is_empty());
    }

    #[test]
    fn a_coinbase_spent_to_another_key_moves_the_right_to_that_key() {
        let mut rights = rights_with_share(1, MINER);
        let transfer = spend(&[coinbase_outpoint(1)], vec![p2pkh_output(BUYER)]);

        rights.apply_share(
            chain_share(2, 2, OTHER_MINER),
            coinbase_txid(2),
            slice::from_ref(&transfer),
        );

        assert_eq!(
            rights.moved_right(&blockhash(1)),
            Some(MovedRight {
                location: OutPoint::new(transfer.compute_txid(), 0),
                holder: ShareOwner::CashP2pkh(BUYER),
                owner: ShareOwner::CashP2pkh(MINER),
            })
        );
        let overlay = rights.overlay(0, 2).unwrap();
        assert_eq!(overlay.transferred_holders(), vec![(blockhash(1), BUYER)]);
    }

    #[test]
    fn a_lock_into_the_contract_keeps_the_seller_as_holder() {
        let mut rights = rights_with_share(1, SELLER);
        let lock = spend(
            &[coinbase_outpoint(1)],
            vec![p2sh32_output(), p2pkh_output(SELLER)],
        );

        rights.apply_share(
            chain_share(2, 2, OTHER_MINER),
            coinbase_txid(2),
            slice::from_ref(&lock),
        );

        let right = rights.moved_right(&blockhash(1)).unwrap();
        assert_eq!(right.location, OutPoint::new(lock.compute_txid(), 0));
        assert_eq!(right.holder, ShareOwner::CashP2pkh(SELLER));
        assert!(
            rights
                .overlay(0, 2)
                .unwrap()
                .transferred_holders()
                .is_empty()
        );
    }

    #[test]
    fn a_claim_of_the_contract_moves_the_right_to_the_buyer() {
        let mut rights = rights_with_share(1, SELLER);
        let lock = spend(&[coinbase_outpoint(1)], vec![p2sh32_output()]);
        let claim = spend(
            &[OutPoint::new(lock.compute_txid(), 0)],
            vec![p2pkh_output(BUYER)],
        );

        rights.apply_share(chain_share(2, 2, OTHER_MINER), coinbase_txid(2), &[lock]);
        rights.apply_share(chain_share(3, 3, OTHER_MINER), coinbase_txid(3), &[claim]);

        assert_eq!(
            rights.overlay(0, 3).unwrap().transferred_holders(),
            vec![(blockhash(1), BUYER)]
        );
    }

    #[test]
    fn a_refund_of_the_contract_returns_the_right_to_the_seller() {
        let mut rights = rights_with_share(1, SELLER);
        let lock = spend(&[coinbase_outpoint(1)], vec![p2sh32_output()]);
        let refund = spend(
            &[OutPoint::new(lock.compute_txid(), 0)],
            vec![p2pkh_output(SELLER)],
        );

        rights.apply_share(chain_share(2, 2, OTHER_MINER), coinbase_txid(2), &[lock]);
        rights.apply_share(chain_share(3, 3, OTHER_MINER), coinbase_txid(3), &[refund]);

        assert_eq!(
            rights.moved_right(&blockhash(1)).unwrap().holder,
            ShareOwner::CashP2pkh(SELLER)
        );
        assert!(
            rights
                .overlay(0, 3)
                .unwrap()
                .transferred_holders()
                .is_empty()
        );
    }

    #[test]
    fn the_right_on_input_one_passes_to_output_one() {
        let mut rights = rights_with_share(1, MINER);
        rights.apply_share(chain_share(2, 2, SELLER), coinbase_txid(2), &[]);
        let transfer = spend(
            &[coinbase_outpoint(1), coinbase_outpoint(2)],
            vec![p2pkh_output(BUYER), p2pkh_output(OTHER_MINER)],
        );

        rights.apply_share(
            chain_share(3, 3, OTHER_MINER),
            coinbase_txid(3),
            slice::from_ref(&transfer),
        );

        let first = rights.moved_right(&blockhash(1)).unwrap();
        let second = rights.moved_right(&blockhash(2)).unwrap();
        assert_eq!(first.location, OutPoint::new(transfer.compute_txid(), 0));
        assert_eq!(first.holder, ShareOwner::CashP2pkh(BUYER));
        assert_eq!(second.location, OutPoint::new(transfer.compute_txid(), 1));
        assert_eq!(second.holder, ShareOwner::CashP2pkh(OTHER_MINER));
    }

    #[test]
    fn a_right_with_no_matching_output_stays_with_its_holder() {
        let mut rights = rights_with_share(1, MINER);
        rights.apply_share(chain_share(2, 2, SELLER), coinbase_txid(2), &[]);
        let merge = spend(
            &[coinbase_outpoint(1), coinbase_outpoint(2)],
            vec![p2pkh_output(BUYER)],
        );

        rights.apply_share(chain_share(3, 3, OTHER_MINER), coinbase_txid(3), &[merge]);

        assert_eq!(
            rights.moved_right(&blockhash(1)).unwrap().holder,
            ShareOwner::CashP2pkh(BUYER)
        );
        assert_eq!(rights.moved_right(&blockhash(2)), None);
    }

    #[test]
    fn a_frozen_right_does_not_move_with_a_later_spend_of_another_output() {
        let mut rights = rights_with_share(1, MINER);
        rights.apply_share(chain_share(2, 2, SELLER), coinbase_txid(2), &[]);
        let merge = spend(
            &[coinbase_outpoint(1), coinbase_outpoint(2)],
            vec![p2pkh_output(BUYER)],
        );
        let onward = spend(
            &[OutPoint::new(merge.compute_txid(), 0)],
            vec![p2pkh_output(OTHER_MINER)],
        );

        rights.apply_share(chain_share(3, 3, OTHER_MINER), coinbase_txid(3), &[merge]);
        rights.apply_share(chain_share(4, 4, OTHER_MINER), coinbase_txid(4), &[onward]);

        assert_eq!(
            rights.moved_right(&blockhash(1)).unwrap().holder,
            ShareOwner::CashP2pkh(OTHER_MINER)
        );
        assert_eq!(rights.moved_right(&blockhash(2)), None);
    }

    #[test]
    fn a_spend_of_an_output_carrying_no_right_moves_nothing() {
        let mut rights = rights_with_share(1, MINER);
        let unrelated = spend(
            &[OutPoint::new(Txid::from_byte_array([0x99; 32]), 0)],
            vec![p2pkh_output(BUYER)],
        );

        rights.apply_share(
            chain_share(2, 2, OTHER_MINER),
            coinbase_txid(2),
            &[unrelated],
        );

        assert_eq!(rights.moved_right(&blockhash(1)), None);
    }

    #[test]
    fn a_second_spend_of_a_coinbase_on_an_invalid_branch_moves_nothing() {
        let mut rights = rights_with_share(1, MINER);
        let first = spend(&[coinbase_outpoint(1)], vec![p2pkh_output(BUYER)]);
        let double_spend = spend(&[coinbase_outpoint(1)], vec![p2pkh_output(OTHER_MINER)]);

        rights.apply_share(chain_share(2, 2, OTHER_MINER), coinbase_txid(2), &[first]);
        rights.apply_share(
            chain_share(3, 3, OTHER_MINER),
            coinbase_txid(3),
            &[double_spend],
        );

        assert_eq!(
            rights.moved_right(&blockhash(1)).unwrap().holder,
            ShareOwner::CashP2pkh(BUYER)
        );
    }

    #[test]
    fn undoing_a_share_restores_the_rights_it_moved() {
        let mut rights = rights_with_share(1, SELLER);
        let lock = spend(&[coinbase_outpoint(1)], vec![p2sh32_output()]);
        let claim = spend(
            &[OutPoint::new(lock.compute_txid(), 0)],
            vec![p2pkh_output(BUYER)],
        );
        rights.apply_share(
            chain_share(2, 2, OTHER_MINER),
            coinbase_txid(2),
            slice::from_ref(&lock),
        );
        rights.apply_share(chain_share(3, 3, OTHER_MINER), coinbase_txid(3), &[claim]);

        assert!(rights.undo_share(&blockhash(3)));

        let right = rights.moved_right(&blockhash(1)).unwrap();
        assert_eq!(right.location, OutPoint::new(lock.compute_txid(), 0));
        assert_eq!(right.holder, ShareOwner::CashP2pkh(SELLER));
        assert_eq!(rights.coinbase_share(&coinbase_txid(3)), None);
        assert!(rights.undo_share(&blockhash(2)));
        assert_eq!(rights.moved_right(&blockhash(1)), None);
        assert!(rights.by_location.is_empty());
    }

    #[test]
    fn undoing_a_share_that_is_not_the_newest_changes_nothing() {
        let mut rights = rights_with_share(1, MINER);
        rights.apply_share(chain_share(2, 2, OTHER_MINER), coinbase_txid(2), &[]);

        assert!(!rights.undo_share(&blockhash(1)));
        assert_eq!(rights.undo_depth(), 2);
    }

    #[test]
    fn the_undo_log_keeps_only_the_newest_shares() {
        let mut rights = PayoutRights::default();
        for height in 0..(RIGHTS_UNDO_DEPTH as u32 + 5) {
            let byte = (height % 250) as u8;
            rights.apply_share(
                ChainShare {
                    blockhash: BlockHash::from_byte_array([byte; 32]),
                    height,
                    owner: ShareOwner::CashP2pkh(MINER),
                },
                Txid::from_byte_array([byte; 32]),
                &[],
            );
        }

        assert_eq!(rights.undo_depth(), RIGHTS_UNDO_DEPTH);
        rights.limit_undo(10);
        assert_eq!(rights.undo_depth(), 10);
    }

    #[test]
    fn the_undo_log_drops_its_oldest_shares_past_the_change_limit() {
        let mut rights = PayoutRights::with_undo_limits(UndoLimits {
            shares: 10,
            changes: 2,
        });
        rights.apply_share(chain_share(1, 1, MINER), coinbase_txid(1), &[]);
        rights.apply_share(chain_share(2, 2, MINER), coinbase_txid(2), &[]);
        rights.apply_share(chain_share(3, 3, MINER), coinbase_txid(3), &[]);
        let first = spend(&[coinbase_outpoint(1)], vec![p2pkh_output(BUYER)]);
        let second = spend(&[coinbase_outpoint(2)], vec![p2pkh_output(BUYER)]);
        let third = spend(&[coinbase_outpoint(3)], vec![p2pkh_output(BUYER)]);

        rights.apply_share(chain_share(4, 4, MINER), coinbase_txid(4), &[first]);
        rights.apply_share(chain_share(5, 5, MINER), coinbase_txid(5), &[second]);
        assert_eq!(rights.undo_depth(), 5);
        rights.apply_share(chain_share(6, 6, MINER), coinbase_txid(6), &[third]);

        assert_eq!(rights.undo_depth(), 2);
        assert!(rights.undo_share(&blockhash(6)));
        assert!(rights.undo_share(&blockhash(5)));
        assert!(!rights.undo_share(&blockhash(4)));
        assert_eq!(rights.moved_right(&blockhash(3)), None);
        assert_eq!(
            rights.moved_right(&blockhash(1)).unwrap().holder,
            ShareOwner::CashP2pkh(BUYER)
        );
    }

    #[test]
    fn evicting_a_share_drops_its_coinbase_and_its_right() {
        let mut rights = rights_with_share(1, MINER);
        let transfer = spend(&[coinbase_outpoint(1)], vec![p2pkh_output(BUYER)]);
        rights.apply_share(
            chain_share(2, 2, OTHER_MINER),
            coinbase_txid(2),
            &[transfer],
        );

        rights.evict_share(&blockhash(1), &coinbase_txid(1));

        assert_eq!(rights.moved_right(&blockhash(1)), None);
        assert_eq!(rights.coinbase_share(&coinbase_txid(1)), None);
        assert!(rights.by_location.is_empty());
    }

    #[test]
    fn an_overlay_below_a_claim_sees_the_seller_and_leaves_the_index_alone() {
        let mut rights = rights_with_share(1, SELLER);
        let lock = spend(&[coinbase_outpoint(1)], vec![p2sh32_output()]);
        let claim = spend(
            &[OutPoint::new(lock.compute_txid(), 0)],
            vec![p2pkh_output(BUYER)],
        );
        rights.apply_share(chain_share(2, 2, OTHER_MINER), coinbase_txid(2), &[lock]);
        rights.apply_share(chain_share(3, 3, OTHER_MINER), coinbase_txid(3), &[claim]);

        let below_the_claim = rights.overlay(1, 2).unwrap();

        assert!(below_the_claim.transferred_holders().is_empty());
        assert_eq!(
            rights.overlay(0, 3).unwrap().transferred_holders(),
            vec![(blockhash(1), BUYER)]
        );
    }

    #[test]
    fn an_overlay_applies_the_branch_on_top_of_its_fork_point() {
        let mut rights = rights_with_share(1, SELLER);
        let lock = spend(&[coinbase_outpoint(1)], vec![p2sh32_output()]);
        rights.apply_share(
            chain_share(2, 2, OTHER_MINER),
            coinbase_txid(2),
            slice::from_ref(&lock),
        );
        let refund_on_the_index = spend(
            &[OutPoint::new(lock.compute_txid(), 0)],
            vec![p2pkh_output(SELLER)],
        );
        rights.apply_share(
            chain_share(3, 3, OTHER_MINER),
            coinbase_txid(3),
            &[refund_on_the_index],
        );
        let claim_on_the_branch = spend(
            &[OutPoint::new(lock.compute_txid(), 0)],
            vec![p2pkh_output(BUYER)],
        );

        let mut branch = rights.overlay(1, 2).unwrap();
        branch.apply_share(
            chain_share(9, 3, OTHER_MINER),
            coinbase_txid(9),
            &[claim_on_the_branch],
        );

        assert_eq!(branch.transferred_holders(), vec![(blockhash(1), BUYER)]);
        assert_eq!(
            rights.moved_right(&blockhash(1)).unwrap().holder,
            ShareOwner::CashP2pkh(SELLER)
        );
    }

    #[test]
    fn an_overlay_ignores_coinbases_of_cached_shares_above_its_fork_point() {
        let mut rights = rights_with_share(1, MINER);
        rights.apply_share(chain_share(2, 2, SELLER), coinbase_txid(2), &[]);
        let spend_of_a_share_not_on_the_branch =
            spend(&[coinbase_outpoint(2)], vec![p2pkh_output(BUYER)]);

        let mut branch = rights.overlay(1, 1).unwrap();
        branch.apply_share(
            chain_share(9, 2, OTHER_MINER),
            coinbase_txid(9),
            &[spend_of_a_share_not_on_the_branch],
        );

        assert!(branch.transferred_holders().is_empty());
    }

    #[test]
    fn an_overlay_deeper_than_the_undo_log_is_refused() {
        let rights = rights_with_share(1, MINER);

        assert_eq!(
            rights.overlay(2, 0).unwrap_err(),
            UndoTooShallow {
                requested: 2,
                available: 1
            }
        );
    }

    #[test]
    fn has_share_transactions_reads_the_header_witness_root() {
        let mut header = TestShareHeaderBuilder::new().build();
        header.coinbase_proof.share_witness_root = compute_witness_root(&[]);
        assert!(!has_share_transactions(&header));

        header.coinbase_proof.share_witness_root =
            compute_witness_root(&[spend(&[coinbase_outpoint(1)], vec![p2pkh_output(BUYER)])]);
        assert!(has_share_transactions(&header));
    }

    #[test]
    fn share_coinbase_txid_matches_the_coinbase_a_share_block_carries() {
        let block = TestShareBlockBuilder::new()
            .share_owner(ShareOwner::CashP2pkh(MINER))
            .add_transaction(spend(&[coinbase_outpoint(1)], vec![p2pkh_output(BUYER)]).0)
            .build();

        assert_eq!(
            share_coinbase_txid(&block.header),
            block.transactions[0].compute_txid()
        );
    }
}
