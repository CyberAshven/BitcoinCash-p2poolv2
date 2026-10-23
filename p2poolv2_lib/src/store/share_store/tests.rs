use super::*;
use crate::shares::bch_coinbase::BchCoinbase;
use crate::shares::merge_mining::MergeMiningCommitment;
use crate::store::block_tx_metadata::{BlockMetadata, ChainMembership, Status};
use crate::test_utils::TestShareBlockBuilder;
use bitcoin::TxMerkleNode;
use bitcoin::Work;
use bitcoin::hashes::Hash;
use p2poolv2_wallet::share_owner::ShareOwner;
use tempfile::tempdir;

/// mark_invalid downgrades a not-yet-validated (HeaderValid) block.
#[test]
fn test_mark_invalid_marks_header_valid_block() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let share = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    store.store_with_valid_metadata(&share);
    assert_eq!(
        store
            .get_block_metadata(&share.block_hash())
            .unwrap()
            .status,
        Status::HeaderValid
    );

    let mut batch = Store::get_write_batch();
    store
        .mark_invalid(&share.block_hash(), None, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    assert_eq!(
        store
            .get_block_metadata(&share.block_hash())
            .unwrap()
            .status,
        Status::Invalid
    );
}

/// mark_invalid must not downgrade a block on the confirmed chain on a
/// later (transient) failure. Chain membership, not status, is the guard.
#[test]
fn test_mark_invalid_is_noop_for_confirmed() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let share = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    store.store_with_valid_metadata(&share);
    let mut metadata = store.get_block_metadata(&share.block_hash()).unwrap();
    metadata.status = Status::BlockValid;
    metadata.chain = ChainMembership::Confirmed;
    let mut batch = Store::get_write_batch();
    store
        .update_block_metadata(&share.block_hash(), &metadata, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let mut batch = Store::get_write_batch();
    store
        .mark_invalid(&share.block_hash(), None, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    assert_eq!(
        store
            .get_block_metadata(&share.block_hash())
            .unwrap()
            .status,
        Status::BlockValid,
        "mark_invalid must be a no-op for a confirmed block"
    );
}

/// mark_invalid takes effect on a non-confirmed block even if it was
/// previously BlockValid: validation state is tracked independently of
/// chain membership, so a candidate that later fails re-validation can be
/// quarantined instead of being silently confirmed.
#[test]
fn test_mark_invalid_takes_effect_for_off_chain_block() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // An off-chain (chain=None), previously-BlockValid block is still
    // quarantined by mark_invalid. (The candidate-chain case is covered by
    // the invalidation-reorg tests.)
    let share = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    store.store_with_valid_metadata(&share);
    let mut metadata = store.get_block_metadata(&share.block_hash()).unwrap();
    metadata.status = Status::BlockValid;
    metadata.chain = ChainMembership::None;
    let mut batch = Store::get_write_batch();
    store
        .update_block_metadata(&share.block_hash(), &metadata, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let mut batch = Store::get_write_batch();
    store
        .mark_invalid(&share.block_hash(), None, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    assert_eq!(
        store
            .get_block_metadata(&share.block_hash())
            .unwrap()
            .status,
        Status::Invalid
    );
}

/// mark_block_valid upgrades HeaderValid to BlockValid.
#[test]
fn test_mark_block_valid_upgrades_header_valid() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let share = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    store.store_with_valid_metadata(&share);

    let mut batch = Store::get_write_batch();
    store
        .mark_block_valid(&share.block_hash(), &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    assert_eq!(
        store
            .get_block_metadata(&share.block_hash())
            .unwrap()
            .status,
        Status::BlockValid
    );
}

/// mark_block_valid is an idempotent no-op for an already-BlockValid
/// block (the re-validation cascade may re-check a valid descendant).
#[test]
fn test_mark_block_valid_is_noop_for_block_valid() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let share = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    store.store_with_valid_metadata(&share);
    let mut metadata = store.get_block_metadata(&share.block_hash()).unwrap();
    metadata.status = Status::BlockValid;
    let mut batch = Store::get_write_batch();
    store
        .update_block_metadata(&share.block_hash(), &metadata, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let mut batch = Store::get_write_batch();
    store
        .mark_block_valid(&share.block_hash(), &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    assert_eq!(
        store
            .get_block_metadata(&share.block_hash())
            .unwrap()
            .status,
        Status::BlockValid
    );
}

/// mark_block_valid is a precondition violation for a Pending or Invalid
/// block -- block validation must run on a header-validated,
/// not-yet-rejected block -- so it returns InvalidState rather than
/// silently succeeding.
#[test]
fn test_mark_block_valid_errors_for_pending_or_invalid() {
    for status in [Status::Pending, Status::Invalid] {
        let temp_dir = tempdir().unwrap();
        let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
        let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
        let mut batch = Store::get_write_batch();
        store.setup_genesis(&genesis, &mut batch).unwrap();
        store.commit_batch(batch).unwrap();

        let share = TestShareBlockBuilder::new()
            .prev_share_blockhash(genesis.block_hash().to_string())
            .nonce(0xe9695792)
            .build();
        store.store_with_valid_metadata(&share);
        let mut metadata = store.get_block_metadata(&share.block_hash()).unwrap();
        metadata.status = status;
        let mut batch = Store::get_write_batch();
        store
            .update_block_metadata(&share.block_hash(), &metadata, &mut batch)
            .unwrap();
        store.commit_batch(batch).unwrap();

        let mut batch = Store::get_write_batch();
        let result = store.mark_block_valid(&share.block_hash(), &mut batch);
        assert!(
            matches!(result, Err(StoreError::InvalidStatusTransition(_))),
            "mark_block_valid must error for {status:?}, got {result:?}"
        );

        // The status must be left unchanged.
        assert_eq!(
            store
                .get_block_metadata(&share.block_hash())
                .unwrap()
                .status,
            status,
            "mark_block_valid must not change status for {status:?}"
        );
    }
}

#[test]
fn test_setup_genesis() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
    let genesis_block = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis_block, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();
    assert_eq!(
        store.get_genesis_blockhash().unwrap(),
        genesis_block.block_hash()
    );

    // verify we can get the share header for the genesis block
    let header = store.get_share_header(&genesis_block.block_hash()).unwrap();
    assert_eq!(header, Some(genesis_block.header.clone()));

    // verify there is nothing in the chain index for the genesis block
    let children = store
        .get_children_blockhashes(&genesis_block.block_hash())
        .unwrap();
    assert!(children.is_none());
}

#[test]
fn test_add_share_header_and_exists() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let block = TestShareBlockBuilder::new().build();
    let blockhash = block.block_hash();

    assert!(!store.share_header_exists(&blockhash));

    let mut batch = Store::get_write_batch();
    store.add_share_header(&block.header, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    assert!(store.share_header_exists(&blockhash));

    // Verify round-trip: read back and compare
    let header_cf = store.db.cf_handle(&ColumnFamily::Header).unwrap();
    let raw = store
        .db
        .get_cf::<&[u8]>(&header_cf, blockhash.as_ref())
        .unwrap()
        .unwrap();
    let stored_header: ShareHeader = consensus::encode::deserialize(&raw).unwrap();
    assert_eq!(stored_header, block.header);
}

#[test]
fn test_organise_header_stores_header_in_header_cf() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Create a child share and organise its header (without storing the full block)
    let child = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();

    assert!(!store.share_header_exists(&child.block_hash()));

    let mut batch = Store::get_write_batch();
    store.organise_header(&child.header, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Header should now be in the Header CF
    assert!(store.share_header_exists(&child.block_hash()));
}

#[test]
fn test_setup_genesis_stores_header_in_header_cf() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    assert!(store.share_header_exists(&genesis.block_hash()));
}

#[test]
fn test_chain_with_uncles() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // Create genesis block with a specific nonce to avoid duplicate issues
    let genesis_block = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis_block, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Create first share (child of genesis)
    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis_block.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    store.store_with_valid_metadata(&share1);

    // Create uncle (also child of genesis)
    let uncle1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis_block.block_hash().to_string())
        .nonce(0xe9695793) // Different nonce to get different hash
        .build();
    store.store_with_valid_metadata(&uncle1);

    // Create share2 referencing uncle1
    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .uncles(vec![uncle1.block_hash()])
        .nonce(0xe9695794)
        .build();
    store.store_with_valid_metadata(&share2);
    // Uncle block index updates are handled by organise_header, not
    // store_with_valid_metadata. Manually register uncle->nephew entries here.
    let mut batch = Store::get_write_batch();
    for uncle_blockhash in &share2.header.uncles {
        store
            .update_block_index(uncle_blockhash, &share2.block_hash(), &mut batch)
            .unwrap();
    }
    store.commit_batch(batch).unwrap();

    // Verify chain structure
    // Genesis should have 2 children (share1 and uncle1)
    let genesis_children = store
        .get_children_blockhashes(&genesis_block.block_hash())
        .unwrap()
        .unwrap();
    assert_eq!(genesis_children.len(), 2);
    assert!(genesis_children.contains(&share1.block_hash()));
    assert!(genesis_children.contains(&uncle1.block_hash()));

    // Share1 should have 1 child (share2)
    let share1_children = store
        .get_children_blockhashes(&share1.block_hash())
        .unwrap()
        .unwrap();
    assert_eq!(share1_children.len(), 1);
    assert!(share1_children.contains(&share2.block_hash()));

    // Uncle1 should also have share2 as child (since share2 references it as uncle)
    let uncle1_children = store
        .get_children_blockhashes(&uncle1.block_hash())
        .unwrap()
        .unwrap();
    assert_eq!(uncle1_children.len(), 1);
    assert!(uncle1_children.contains(&share2.block_hash()));

    // Verify we can retrieve all shares
    assert!(store.get_share(&genesis_block.block_hash()).is_some());
    assert!(store.get_share(&share1.block_hash()).is_some());
    assert!(store.get_share(&uncle1.block_hash()).is_some());
    assert!(store.get_share(&share2.block_hash()).is_some());
}

#[test]
fn test_store_share_block_with_transactions_should_retreive_txs() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
    let block = TestShareBlockBuilder::new().build();
    let num_txs = block.transactions.len();
    let txs = block.transactions.clone();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&block, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let blockhash = block.block_hash();

    let result = store.get_share(&blockhash);
    assert!(result.is_some());
    let share = result.unwrap();
    assert_eq!(share.transactions.len(), num_txs);
    assert_eq!(share.transactions, txs);
}

#[test]
fn test_get_share_header() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let block = TestShareBlockBuilder::new().build();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&block, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let header = store.get_share_header(&block.block_hash()).unwrap();
    assert_eq!(header, Some(block.header.clone()));
}

#[test]
fn test_get_share_header_nonexistent() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let nonexistent_hash = BlockHash::all_zeros();
    let header = store.get_share_header(&nonexistent_hash).unwrap();
    assert!(header.is_none());
}

#[test]
fn test_block_status_for_nonexistent_block() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let nonexistent_hash = BlockHash::all_zeros();
    let share = store.get_share(&nonexistent_hash);
    assert!(share.is_none());
}

#[test]
fn test_share_block_exists_returns_false_for_missing_block() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let nonexistent_hash = BlockHash::all_zeros();
    assert!(!store.share_block_exists(&nonexistent_hash));
}

#[test]
fn test_share_block_exists_returns_true_for_stored_block() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let block = TestShareBlockBuilder::new().build();
    let blockhash = block.block_hash();

    assert!(!store.share_block_exists(&blockhash));

    let mut batch = Store::get_write_batch();
    store.add_share_block(&block, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    assert!(store.share_block_exists(&blockhash));
}

/// A block spending an output this node does not hold is stored, not
/// refused. The store cannot tell an output that never existed from one
/// below its prune height that it never fetched, so it records an unknown
/// root and leaves the verdict to validation. Refusing the block left a
/// valid block on the best chain unstorable, and confirmation stalled.
#[test]
fn test_add_share_block_stores_a_block_spending_an_output_it_does_not_hold() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let spend_of_unknown_output = Transaction {
        version: bitcoin::transaction::Version::ONE,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn {
            previous_output: bitcoin::OutPoint::new(bitcoin::Txid::from_byte_array([0x42; 32]), 0),
            ..Default::default()
        }],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(1_000),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };
    let block = TestShareBlockBuilder::new()
        .add_transaction(spend_of_unknown_output)
        .build();

    let mut batch = Store::get_write_batch();
    store
        .add_share_block(&block, &mut batch)
        .expect("a missing input is recorded, not an error");
    store.commit_batch(batch).unwrap();

    assert!(store.share_block_exists(&block.block_hash()));
}

#[test]
fn test_add_share_block_stores_header_in_header_cf() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let block = TestShareBlockBuilder::new().build();
    let blockhash = block.block_hash();

    let mut batch = Store::get_write_batch();
    store.add_share_block(&block, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Verify the header was written to the Header CF
    let header_cf = store.db.cf_handle(&ColumnFamily::Header).unwrap();
    let raw = store
        .db
        .get_cf::<&[u8]>(&header_cf, blockhash.as_ref())
        .unwrap();
    assert!(raw.is_some(), "Header should exist in Header CF");

    // Deserialize and verify it matches the original header
    let stored_header: ShareHeader = consensus::encode::deserialize(&raw.unwrap()).unwrap();
    assert_eq!(stored_header, block.header);
}

#[test]
fn test_get_block_metadata_batch_returns_stored_metadata() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let block_a = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let block_b = TestShareBlockBuilder::new().nonce(0xe9695792).build();
    let hash_a = block_a.block_hash();
    let hash_b = block_b.block_hash();

    let metadata_a = BlockMetadata {
        expected_height: Some(1),
        chain_work: Work::from_le_bytes([1u8; 32]),
        status: Status::HeaderValid,
        chain: ChainMembership::Candidate,
    };
    let metadata_b = BlockMetadata {
        expected_height: Some(2),
        chain_work: Work::from_le_bytes([2u8; 32]),
        status: Status::BlockValid,
        chain: ChainMembership::Confirmed,
    };

    let mut batch = Store::get_write_batch();
    store
        .update_block_metadata(&hash_a, &metadata_a, &mut batch)
        .unwrap();
    store
        .update_block_metadata(&hash_b, &metadata_b, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let results = store.get_block_metadata_batch(&[hash_a, hash_b]).unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0], (hash_a, metadata_a));
    assert_eq!(results[1], (hash_b, metadata_b));
}

#[test]
fn test_get_block_metadata_batch_skips_missing_blockhashes() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let block = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let stored_hash = block.block_hash();
    let missing_hash = BlockHash::all_zeros();

    let metadata = BlockMetadata {
        expected_height: Some(5),
        chain_work: Work::from_le_bytes([3u8; 32]),
        status: Status::HeaderValid,
        chain: ChainMembership::None,
    };

    let mut batch = Store::get_write_batch();
    store
        .update_block_metadata(&stored_hash, &metadata, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let results = store
        .get_block_metadata_batch(&[missing_hash, stored_hash])
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], (stored_hash, metadata));
}

#[test]
fn test_get_block_metadata_batch_returns_empty_for_empty_input() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let results = store.get_block_metadata_batch(&[]).unwrap();
    assert!(results.is_empty());
}

#[test]
fn test_add_share_block_skips_duplicate() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let block = TestShareBlockBuilder::new().build();
    let blockhash = block.block_hash();

    // First add succeeds and stores the block
    let mut batch = Store::get_write_batch();
    store.add_share_block(&block, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();
    assert!(store.get_share(&blockhash).is_some());

    // Second add of the same block returns Ok without error
    let mut batch = Store::get_write_batch();
    let result = store.add_share_block(&block, &mut batch);
    assert!(result.is_ok());

    // The batch should be empty (no writes for duplicate)
    // We verify by checking the block is still retrievable and unchanged
    store.commit_batch(batch).unwrap();
    assert!(store.get_share(&blockhash).is_some());
}

#[test]
fn test_template_merkle_branches_round_trip() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let mut block = TestShareBlockBuilder::new().build();
    let branch_a = TxMerkleNode::from_byte_array([0xaa; 32]);
    let branch_b = TxMerkleNode::from_byte_array([0xbb; 32]);
    let branch_c = TxMerkleNode::from_byte_array([0xcc; 32]);
    block.template_merkle_branches = vec![branch_a, branch_b, branch_c];

    let blockhash = block.block_hash();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&block, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let share = store.get_share(&blockhash).unwrap();
    assert_eq!(share.template_merkle_branches.len(), 3);
    assert_eq!(share.template_merkle_branches[0], branch_a);
    assert_eq!(share.template_merkle_branches[1], branch_b);
    assert_eq!(share.template_merkle_branches[2], branch_c);
}

/// The bitcoin coinbase is not in the header, so it is stored beside the
/// block and comes back with it.
#[test]
fn test_bitcoin_coinbase_round_trip() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let block = TestShareBlockBuilder::new().build();
    let blockhash = block.block_hash();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&block, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let share = store.get_share(&blockhash).unwrap();
    assert_eq!(share.bitcoin_coinbase, block.bitcoin_coinbase);
    assert_eq!(share, block);
}

#[test]
fn test_get_bitcoin_coinbase_returns_none_for_missing() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    assert!(
        store
            .get_bitcoin_coinbase(&BlockHash::all_zeros())
            .unwrap()
            .is_none()
    );
}

#[test]
fn declared_bch_coinbase_metadata_survives_store_reconstruction() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
    let mut share = TestShareBlockBuilder::new().build();
    share.header.miner_address = ShareOwner::CashP2pkh([0x22; 20]);
    share.header.bch_coinbase = Some(BchCoinbase {
        prefix: vec![0x51],
        merge_mining: MergeMiningCommitment::reserved(),
    });
    let blockhash = share.block_hash();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&share, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();
    assert_eq!(store.get_share(&blockhash).unwrap(), share);
}

#[test]
fn test_get_template_merkle_branches_returns_empty_for_missing() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let missing_hash = BlockHash::all_zeros();
    let branches = store.get_template_merkle_branches(&missing_hash).unwrap();
    assert!(branches.is_empty());
}

#[test]
fn test_first_existing_share_header_returns_none_for_empty_slice() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    assert!(store.first_existing_share_header(&[]).is_none());
}

#[test]
fn test_first_existing_share_header_returns_none_when_no_match() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let hash_a = TestShareBlockBuilder::new()
        .nonce(0xe9695791)
        .build()
        .block_hash();
    let hash_b = TestShareBlockBuilder::new()
        .nonce(0xe9695792)
        .build()
        .block_hash();

    assert!(
        store
            .first_existing_share_header(&[hash_a, hash_b])
            .is_none()
    );
}

#[test]
fn test_first_existing_share_header_returns_first_match() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let block_a = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let block_b = TestShareBlockBuilder::new().nonce(0xe9695792).build();
    let block_c = TestShareBlockBuilder::new().nonce(0xe9695793).build();

    // Store headers for block_b and block_c but not block_a
    let mut batch = Store::get_write_batch();
    store.add_share_header(&block_b.header, &mut batch).unwrap();
    store.add_share_header(&block_c.header, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // block_a is not in store, block_b is -- block_b should be returned
    let result = store.first_existing_share_header(&[
        block_a.block_hash(),
        block_b.block_hash(),
        block_c.block_hash(),
    ]);
    assert_eq!(result, Some(block_b.block_hash()));
}

#[test]
fn test_first_existing_share_header_respects_slice_order() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let block_a = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let block_b = TestShareBlockBuilder::new().nonce(0xe9695792).build();

    // Store both headers
    let mut batch = Store::get_write_batch();
    store.add_share_header(&block_a.header, &mut batch).unwrap();
    store.add_share_header(&block_b.header, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // block_a appears first in the slice, so it should be returned
    let result_a_first =
        store.first_existing_share_header(&[block_a.block_hash(), block_b.block_hash()]);
    assert_eq!(result_a_first, Some(block_a.block_hash()));

    // block_b appears first in the slice, so it should be returned
    let result_b_first =
        store.first_existing_share_header(&[block_b.block_hash(), block_a.block_hash()]);
    assert_eq!(result_b_first, Some(block_b.block_hash()));
}

#[test]
fn test_missing_share_blocks_returns_all_when_none_stored() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let block_a = TestShareBlockBuilder::new().nonce(1).build();
    let block_b = TestShareBlockBuilder::new().nonce(2).build();
    let hashes = vec![block_a.block_hash(), block_b.block_hash()];

    let missing = store.missing_share_blocks(&hashes);
    assert_eq!(missing.len(), 2);
    assert_eq!(missing, hashes);
}

#[test]
fn test_missing_share_blocks_excludes_stored_blocks() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let block_a = TestShareBlockBuilder::new().nonce(1).build();
    let block_b = TestShareBlockBuilder::new().nonce(2).build();

    let mut batch = Store::get_write_batch();
    store.add_share_block(&block_a, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let hashes = vec![block_a.block_hash(), block_b.block_hash()];
    let missing = store.missing_share_blocks(&hashes);
    assert_eq!(missing, vec![block_b.block_hash()]);
}

#[test]
fn test_missing_share_blocks_returns_empty_when_all_stored() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let block_a = TestShareBlockBuilder::new().nonce(1).build();
    let block_b = TestShareBlockBuilder::new().nonce(2).build();

    let mut batch = Store::get_write_batch();
    store.add_share_block(&block_a, &mut batch).unwrap();
    store.add_share_block(&block_b, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let hashes = vec![block_a.block_hash(), block_b.block_hash()];
    let missing = store.missing_share_blocks(&hashes);
    assert!(missing.is_empty());
}

#[test]
fn test_missing_share_blocks_empty_input() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let missing = store.missing_share_blocks(&[]);
    assert!(missing.is_empty());
}

#[test]
fn test_get_blockhashes_for_height_range_returns_all_heights() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let block_a = TestShareBlockBuilder::new().nonce(1).build();
    let block_b = TestShareBlockBuilder::new().nonce(2).build();
    let block_c = TestShareBlockBuilder::new().nonce(3).build();

    let mut batch = Store::get_write_batch();
    store
        .set_height_to_blockhash(&block_a.block_hash(), 10, &mut batch)
        .unwrap();
    store
        .set_height_to_blockhash(&block_b.block_hash(), 11, &mut batch)
        .unwrap();
    store
        .set_height_to_blockhash(&block_c.block_hash(), 12, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let results = store.get_blockhashes_for_height_range(10, 12);
    assert_eq!(results.len(), 3);
    assert_eq!(results[0], (10, vec![block_a.block_hash()]));
    assert_eq!(results[1], (11, vec![block_b.block_hash()]));
    assert_eq!(results[2], (12, vec![block_c.block_hash()]));
}

#[test]
fn test_get_blockhashes_for_height_range_multiple_blocks_per_height() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let block_a = TestShareBlockBuilder::new().nonce(1).build();
    let block_b = TestShareBlockBuilder::new().nonce(2).build();

    let mut batch = Store::get_write_batch();
    store
        .set_height_to_blockhash(&block_a.block_hash(), 5, &mut batch)
        .unwrap();
    store
        .set_height_to_blockhash(&block_b.block_hash(), 5, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let results = store.get_blockhashes_for_height_range(5, 5);
    assert_eq!(results.len(), 1);
    let (height, blockhashes) = &results[0];
    assert_eq!(*height, 5);
    assert_eq!(blockhashes.len(), 2);
    assert!(blockhashes.contains(&block_a.block_hash()));
    assert!(blockhashes.contains(&block_b.block_hash()));
}

#[test]
fn test_get_blockhashes_for_height_range_empty_when_reversed() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let results = store.get_blockhashes_for_height_range(10, 5);
    assert!(results.is_empty());
}

#[test]
fn test_get_blockhashes_for_height_range_skips_empty_heights() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let block_a = TestShareBlockBuilder::new().nonce(1).build();
    let block_b = TestShareBlockBuilder::new().nonce(2).build();

    let mut batch = Store::get_write_batch();
    store
        .set_height_to_blockhash(&block_a.block_hash(), 10, &mut batch)
        .unwrap();
    store
        .set_height_to_blockhash(&block_b.block_hash(), 15, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let results = store.get_blockhashes_for_height_range(10, 15);
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].0, 10);
    assert_eq!(results[1].0, 15);
}

#[test]
fn test_get_blockhashes_for_height_range_no_entries() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let results = store.get_blockhashes_for_height_range(100, 200);
    assert!(results.is_empty());
}
