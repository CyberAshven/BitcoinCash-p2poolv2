// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use crate::accounting::payout::payout_tree::{PayoutTree, TreePayee};
use bitcoin::hashes::Hash;
use bitcoin::{PubkeyHash, ScriptBuf};
use std::sync::{Arc, Mutex};

/// BCHN as the executor sees it, scripted by each test.
#[derive(Clone)]
struct FakeNode {
    tip: Arc<Mutex<BlockHash>>,
    confirmations: Arc<Mutex<Option<i64>>>,
    confirmation_error: Arc<Mutex<Option<String>>>,
    outcome: Arc<Mutex<BroadcastOutcome>>,
    transaction_outcomes: Arc<Mutex<HashMap<Txid, BroadcastOutcome>>>,
    root_unspent: Arc<Mutex<bool>>,
    initial_block_download: Arc<Mutex<bool>>,
    sent: Arc<Mutex<Vec<Txid>>>,
}

impl FakeNode {
    fn new(confirmations: i64) -> Self {
        Self {
            tip: Arc::new(Mutex::new(BlockHash::from_byte_array([1; 32]))),
            confirmations: Arc::new(Mutex::new(Some(confirmations))),
            confirmation_error: Arc::new(Mutex::new(None)),
            outcome: Arc::new(Mutex::new(BroadcastOutcome::Accepted)),
            transaction_outcomes: Arc::new(Mutex::new(HashMap::with_capacity(8))),
            root_unspent: Arc::new(Mutex::new(true)),
            initial_block_download: Arc::new(Mutex::new(false)),
            sent: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn advance_tip(&self, byte: u8) {
        *self.tip.lock().unwrap() = BlockHash::from_byte_array([byte; 32]);
    }

    fn sent(&self) -> Vec<Txid> {
        self.sent.lock().unwrap().clone()
    }
}

#[async_trait]
impl TreeExecutionNode for FakeNode {
    async fn best_block_hash(&self) -> Result<BlockHash, String> {
        Ok(*self.tip.lock().unwrap())
    }

    async fn in_initial_block_download(&self) -> Result<bool, String> {
        Ok(*self.initial_block_download.lock().unwrap())
    }

    async fn block_confirmations(&self, _blockhash: BlockHash) -> Result<Option<i64>, String> {
        match self.confirmation_error.lock().unwrap().clone() {
            Some(error) => Err(error),
            None => Ok(*self.confirmations.lock().unwrap()),
        }
    }

    async fn send_raw_transaction(&self, transaction: &Transaction) -> BroadcastOutcome {
        self.sent.lock().unwrap().push(transaction.compute_txid());
        self.transaction_outcomes
            .lock()
            .unwrap()
            .get(&transaction.compute_txid())
            .cloned()
            .unwrap_or_else(|| self.outcome.lock().unwrap().clone())
    }

    async fn unspent_on_chain(&self, outpoint: OutPoint) -> Result<bool, String> {
        Ok(
            outpoint.txid == Txid::from_byte_array([0x44; 32])
                && *self.root_unspent.lock().unwrap(),
        )
    }
}

/// A kept tree of `count` P2PKH payees.
fn stored_tree(count: u32) -> StoredPayoutTree {
    let payees: Vec<TreePayee> = (0..count)
        .map(|number| {
            let mut hash = [0x11u8; 20];
            hash[..4].copy_from_slice(&number.to_le_bytes());
            TreePayee {
                script_pubkey: ScriptBuf::new_p2pkh(&PubkeyHash::from_byte_array(hash)),
                amount: 100_000,
            }
        })
        .collect();
    StoredPayoutTree::new(
        &PayoutTree::build(payees).unwrap(),
        Txid::from_byte_array([0x44; 32]),
        3,
        BlockHash::from_byte_array([0x55; 32]),
        900_000,
        BlockHash::from_byte_array([0x66; 32]),
    )
}

fn store_with(tree: StoredPayoutTree) -> ChainStoreHandle {
    let mut store = ChainStoreHandle::default();
    store
        .expect_get_payout_trees()
        .returning(move || Ok(vec![tree.clone()]));
    store
}

#[tokio::test]
async fn an_immature_tree_is_not_broadcast() {
    let node = FakeNode::new(99);
    let mut executor = PayoutTreeExecutor::new(
        node.clone(),
        store_with(stored_tree(3)),
        payout_tree_channel().1,
    );

    executor.execute_round().await;

    assert!(node.sent().is_empty());
}

/// At 100 confirmations the root goes first, then its children, at
/// 1 sat/byte; the children of the other rates are tried too, never their
/// roots.
#[tokio::test]
async fn a_mature_tree_is_broadcast_root_first_at_one_sat_per_byte() {
    let stored = stored_tree(501);
    let tree = stored.tree().unwrap();
    let at_one: Vec<Txid> = tree
        .node_transactions(1, stored.root_outpoint())
        .unwrap()
        .iter()
        .map(Transaction::compute_txid)
        .collect();
    let at_two_root = tree.node_transactions(2, stored.root_outpoint()).unwrap()[0].compute_txid();
    let node = FakeNode::new(100);
    let mut executor =
        PayoutTreeExecutor::new(node.clone(), store_with(stored), payout_tree_channel().1);

    executor.execute_round().await;

    let sent = node.sent();
    assert_eq!(sent[..3], at_one[..]);
    assert_eq!(sent.len(), 3 + 2 + 2);
    assert!(!sent.contains(&at_two_root));
}

#[tokio::test]
async fn nothing_is_broadcast_again_on_the_same_tip() {
    let node = FakeNode::new(100);
    let mut executor = PayoutTreeExecutor::new(
        node.clone(),
        store_with(stored_tree(3)),
        payout_tree_channel().1,
    );

    executor.execute_round().await;
    executor.execute_round().await;

    assert_eq!(node.sent().len(), 1);
}

#[tokio::test]
async fn a_tree_is_rebroadcast_on_each_new_tip_until_it_settles() {
    let node = FakeNode::new(100);
    let mut executor = PayoutTreeExecutor::new(
        node.clone(),
        store_with(stored_tree(3)),
        payout_tree_channel().1,
    );

    executor.execute_round().await;
    node.advance_tip(2);
    executor.execute_round().await;

    assert_eq!(node.sent().len(), 2);
}

/// Once every transaction is confirmed and the coinbase output spent for
/// six tips in a row, the tree is marked executed, once.
#[tokio::test]
async fn a_settled_tree_is_marked_executed_after_six_tips() {
    let stored = stored_tree(3);
    let coinbase_txid = stored.coinbase_txid;
    let mut store = store_with(stored);
    store
        .expect_mark_payout_tree_executed()
        .withf(move |txid| *txid == coinbase_txid)
        .times(1)
        .returning(|_| Ok(()));
    let node = FakeNode::new(150);
    *node.outcome.lock().unwrap() = BroadcastOutcome::InChain;
    *node.root_unspent.lock().unwrap() = false;
    let mut executor = PayoutTreeExecutor::new(node.clone(), store, payout_tree_channel().1);

    for byte in 1..=SETTLED_TIPS as u8 {
        node.advance_tip(byte);
        executor.execute_round().await;
    }
}

/// A transaction still waiting in the mempool keeps the tree open.
#[tokio::test]
async fn a_pending_tree_is_not_marked_executed() {
    let mut store = store_with(stored_tree(3));
    store.expect_mark_payout_tree_executed().never();
    let node = FakeNode::new(150);
    *node.root_unspent.lock().unwrap() = false;
    let mut executor = PayoutTreeExecutor::new(node.clone(), store, payout_tree_channel().1);

    for byte in 1..=(SETTLED_TIPS as u8 + 2) {
        node.advance_tip(byte);
        executor.execute_round().await;
    }
}

/// A tree BCHN refuses (here for its fee) stays kept and is tried again.
#[tokio::test]
async fn a_refused_tree_is_not_marked_executed() {
    let mut store = store_with(stored_tree(3));
    store.expect_mark_payout_tree_executed().never();
    let node = FakeNode::new(150);
    *node.outcome.lock().unwrap() = BroadcastOutcome::Rejected("min relay fee not met".to_string());
    let mut executor = PayoutTreeExecutor::new(node.clone(), store, payout_tree_channel().1);

    for byte in 1..=(SETTLED_TIPS as u8 + 2) {
        node.advance_tip(byte);
        executor.execute_round().await;
    }

    assert_eq!(node.sent().len(), SETTLED_TIPS as usize + 2);
}

/// The review's case: 501 payees make a root and two children. The root is
/// confirmed and its outputs spent, but BCHN refuses both children (here for
/// their fee). The tree is never marked executed and is broadcast again on
/// every tip: a spent root says nothing about its children.
#[tokio::test]
async fn a_501_payee_tree_with_confirmed_root_and_rejected_children_stays_pending() {
    let stored = stored_tree(501);
    let transactions = stored
        .tree()
        .unwrap()
        .node_transactions(1, stored.root_outpoint())
        .unwrap();
    let mut store = store_with(stored);
    store.expect_mark_payout_tree_executed().never();
    let node = FakeNode::new(150);
    *node.root_unspent.lock().unwrap() = false;
    *node.outcome.lock().unwrap() = BroadcastOutcome::Rejected("min relay fee not met".to_string());
    node.transaction_outcomes
        .lock()
        .unwrap()
        .insert(transactions[0].compute_txid(), BroadcastOutcome::InChain);
    let mut executor = PayoutTreeExecutor::new(node.clone(), store, payout_tree_channel().1);
    let rounds = SETTLED_TIPS as u8 + 2;

    for byte in 1..=rounds {
        node.advance_tip(byte);
        executor.execute_round().await;
    }

    // Each round: the 1 sat/byte root and its two children, and the two
    // children of each other rate.
    assert_eq!(node.sent().len(), usize::from(rounds) * (3 + 2 + 2));
    assert_eq!(
        executor.settled_tips.values().copied().collect::<Vec<_>>(),
        vec![0]
    );
}

/// A confirmed root does not turn missing children into confirmed payouts.
#[tokio::test]
async fn a_501_payee_tree_with_missing_children_stays_pending() {
    let stored = stored_tree(501);
    let transactions = stored
        .tree()
        .unwrap()
        .node_transactions(1, stored.root_outpoint())
        .unwrap();
    let mut store = store_with(stored);
    store.expect_mark_payout_tree_executed().never();
    let node = FakeNode::new(150);
    *node.outcome.lock().unwrap() = BroadcastOutcome::MissingInputs;
    node.transaction_outcomes
        .lock()
        .unwrap()
        .insert(transactions[0].compute_txid(), BroadcastOutcome::InChain);
    let mut executor = PayoutTreeExecutor::new(node.clone(), store, payout_tree_channel().1);

    executor.execute_round().await;

    assert_eq!(
        executor.settled_tips.values().copied().collect::<Vec<_>>(),
        vec![0]
    );
}

/// An RPC failure cannot count as another settled tip, or as evidence
/// that a found block is unknown and should expire.
#[tokio::test]
async fn a_confirmation_rpc_error_resets_settlement_without_aging_the_record() {
    let mut store = store_with(stored_tree(501));
    store.expect_mark_payout_tree_executed().never();
    store.expect_record_unknown_payout_tree_tip().never();
    let node = FakeNode::new(150);
    *node.outcome.lock().unwrap() = BroadcastOutcome::InChain;
    let mut executor = PayoutTreeExecutor::new(node.clone(), store, payout_tree_channel().1);

    executor.execute_round().await;
    assert_eq!(
        executor.settled_tips.values().copied().collect::<Vec<_>>(),
        vec![1]
    );
    let sent_before_error = node.sent().len();
    *node.confirmation_error.lock().unwrap() = Some("BCHN RPC connection closed".to_string());
    node.advance_tip(2);
    executor.execute_round().await;

    assert!(executor.settled_tips.is_empty());
    assert_eq!(node.sent().len(), sent_before_error);

    *node.confirmation_error.lock().unwrap() = None;
    node.advance_tip(3);
    executor.execute_round().await;
    assert_eq!(
        executor.settled_tips.values().copied().collect::<Vec<_>>(),
        vec![1]
    );
}

/// Rejections belonging to conflicting fee variants do not keep a fully
/// confirmed variant open. Its confirmed children prove the spent root.
#[tokio::test]
async fn a_501_payee_tree_settles_only_after_one_whole_variant_is_confirmed() {
    let stored = stored_tree(501);
    let coinbase_txid = stored.coinbase_txid;
    let transactions = stored
        .tree()
        .unwrap()
        .node_transactions(2, stored.root_outpoint())
        .unwrap();
    let mut store = store_with(stored);
    store
        .expect_mark_payout_tree_executed()
        .withf(move |txid| *txid == coinbase_txid)
        .times(1)
        .returning(|_| Ok(()));
    let node = FakeNode::new(150);
    *node.root_unspent.lock().unwrap() = false;
    *node.outcome.lock().unwrap() = BroadcastOutcome::MissingInputs;
    node.transaction_outcomes
        .lock()
        .unwrap()
        .insert(transactions[1].compute_txid(), BroadcastOutcome::InChain);
    node.transaction_outcomes
        .lock()
        .unwrap()
        .insert(transactions[2].compute_txid(), BroadcastOutcome::InChain);
    let mut executor = PayoutTreeExecutor::new(node.clone(), store, payout_tree_channel().1);

    for byte in 1..=SETTLED_TIPS as u8 {
        node.advance_tip(byte);
        executor.execute_round().await;
    }
}

#[tokio::test]
async fn an_unknown_block_is_aged_once_per_new_bch_tip() {
    let stored = stored_tree(3);
    let coinbase_txid = stored.coinbase_txid;
    let mut store = store_with(stored);
    store
        .expect_record_unknown_payout_tree_tip()
        .withf(move |txid, tip| {
            *txid == coinbase_txid && *tip == BlockHash::from_byte_array([1; 32])
        })
        .times(1)
        .returning(|_, _| Ok(false));
    store.expect_mark_payout_tree_executed().never();
    let node = FakeNode::new(0);
    *node.confirmations.lock().unwrap() = None;
    let mut executor = PayoutTreeExecutor::new(node.clone(), store, payout_tree_channel().1);

    executor.execute_round().await;
    executor.execute_round().await;

    assert!(node.sent().is_empty());
}

/// A BCHN still downloading the chain does not know blocks it will soon
/// have, so its new tips never count towards removing a kept tree.
#[tokio::test]
async fn an_unknown_block_is_not_aged_during_initial_block_download() {
    let mut store = store_with(stored_tree(3));
    store.expect_record_unknown_payout_tree_tip().never();
    let node = FakeNode::new(0);
    *node.confirmations.lock().unwrap() = None;
    *node.initial_block_download.lock().unwrap() = true;
    let mut executor = PayoutTreeExecutor::new(node.clone(), store, payout_tree_channel().1);

    for byte in 2..=9 {
        node.advance_tip(byte);
        executor.execute_round().await;
    }

    assert!(node.sent().is_empty());
}

#[tokio::test]
async fn a_found_tree_is_kept() {
    let stored = stored_tree(3);
    let expected = stored.clone();
    let mut store = ChainStoreHandle::default();
    store
        .expect_put_payout_tree()
        .withf(move |tree| *tree == expected)
        .times(1)
        .returning(|_| Ok(()));
    let executor = PayoutTreeExecutor::new(FakeNode::new(0), store, payout_tree_channel().1);

    executor.keep(stored).await;
}

#[test]
fn bchn_rejections_are_classified() {
    assert_eq!(
        classify_rejection(-27, "Transaction already in block chain"),
        BroadcastOutcome::InChain
    );
    assert_eq!(
        classify_rejection(-25, "Missing inputs"),
        BroadcastOutcome::MissingInputs
    );
    assert_eq!(
        classify_rejection(-26, "txn-mempool-conflict"),
        BroadcastOutcome::MissingInputs
    );
    assert_eq!(
        classify_rejection(-26, "min relay fee not met"),
        BroadcastOutcome::Rejected("RPC error -26: min relay fee not met".to_string())
    );
}
