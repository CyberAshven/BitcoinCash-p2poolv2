use super::*;
use crate::shares::share_block::{ShareTransaction, Txids};
use crate::store::block_tx_metadata::Status;
use crate::test_utils::TestShareBlockBuilder;
use bitcoin::hashes::Hash;
use tempfile::tempdir;

// ── append_to_confirmed tests ─────────────────────────────────────────

#[test]
fn test_make_confirmed() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let share1 = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let share2 = TestShareBlockBuilder::new().nonce(0xe9695792).build();

    let mut batch = Store::get_write_batch();
    store.add_share_block(&share1, &mut batch).unwrap();
    let mut metadata1 = BlockMetadata {
        expected_height: Some(0),
        chain_work: share1.header.get_work(),
        status: Status::HeaderValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(&share1.block_hash(), &metadata1, &mut batch)
        .unwrap();
    store
        .append_to_confirmed(&share1.block_hash(), 0, &mut metadata1, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    // Verify we can retrieve it
    let confirmed = store.get_confirmed_at_height(0).unwrap();
    assert_eq!(confirmed, share1.block_hash());

    let mut batch = Store::get_write_batch();
    store.add_share_block(&share2, &mut batch).unwrap();
    let mut metadata2 = BlockMetadata {
        expected_height: Some(1),
        chain_work: share2.header.get_work(),
        status: Status::HeaderValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(&share2.block_hash(), &metadata2, &mut batch)
        .unwrap();
    store
        .append_to_confirmed(&share2.block_hash(), 1, &mut metadata2, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    // Verify both heights
    assert_eq!(
        store.get_confirmed_at_height(0).unwrap(),
        share1.block_hash()
    );
    assert_eq!(
        store.get_confirmed_at_height(1).unwrap(),
        share2.block_hash()
    );

    // Non-existent height should return None
    assert!(store.get_confirmed_at_height(999).is_err());

    // Top confirmed height is changed
    assert_eq!(store.get_top_confirmed_height().unwrap(), 1);

    // Top confirmed is changed
    let top_confirmed = store.get_top_confirmed().unwrap();
    assert_eq!(top_confirmed.hash, share2.block_hash());
    assert_eq!(top_confirmed.height, 1);
    assert_eq!(top_confirmed.work, share2.header.get_work());
}

#[test]
fn test_make_confirmed_ignores_same_height() {
    // Confirming at the same height as current top is ignored
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let share1 = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let share2 = TestShareBlockBuilder::new().nonce(0xe9695792).build();
    let share3 = TestShareBlockBuilder::new().nonce(0xe9695793).build();

    // Add all shares first
    let mut batch = Store::get_write_batch();
    store.add_share_block(&share1, &mut batch).unwrap();
    store.add_share_block(&share2, &mut batch).unwrap();
    store.add_share_block(&share3, &mut batch).unwrap();
    let mut metadata1 = BlockMetadata {
        expected_height: Some(0),
        chain_work: share1.header.get_work(),
        status: Status::HeaderValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(&share1.block_hash(), &metadata1, &mut batch)
        .unwrap();
    let mut metadata2 = BlockMetadata {
        expected_height: Some(1),
        chain_work: share2.header.get_work(),
        status: Status::HeaderValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(&share2.block_hash(), &metadata2, &mut batch)
        .unwrap();
    let mut metadata3 = BlockMetadata {
        expected_height: Some(1),
        chain_work: share3.header.get_work(),
        status: Status::HeaderValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(&share3.block_hash(), &metadata3, &mut batch)
        .unwrap();
    store
        .append_to_confirmed(&share1.block_hash(), 0, &mut metadata1, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    assert_eq!(
        store.get_confirmed_at_height(0).unwrap(),
        share1.block_hash()
    );

    // Confirm share2
    let mut batch = Store::get_write_batch();
    store
        .append_to_confirmed(&share2.block_hash(), 1, &mut metadata2, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    // Confirm share3 at same height as share2, should error.
    let mut batch = Store::get_write_batch();
    assert!(
        store
            .append_to_confirmed(&share3.block_hash(), 1, &mut metadata3, &mut batch)
            .is_err()
    );
}

#[test]
fn test_make_confirmed_does_not_update_top_when_height_skips() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let share0 = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let share2 = TestShareBlockBuilder::new().nonce(0xe9695792).build();

    // Add shares first
    let mut batch = Store::get_write_batch();
    store.add_share_block(&share0, &mut batch).unwrap();
    store.add_share_block(&share2, &mut batch).unwrap();
    let mut metadata0 = BlockMetadata {
        expected_height: Some(0),
        chain_work: share0.header.get_work(),
        status: Status::HeaderValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(&share0.block_hash(), &metadata0, &mut batch)
        .unwrap();
    let mut metadata2 = BlockMetadata {
        expected_height: Some(2),
        chain_work: share2.header.get_work(),
        status: Status::HeaderValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(&share2.block_hash(), &metadata2, &mut batch)
        .unwrap();
    store
        .append_to_confirmed(&share0.block_hash(), 0, &mut metadata0, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    // Verify top confirmed height is 0
    assert_eq!(store.get_top_confirmed_height().unwrap(), 0);

    // Make share2 confirmed at height 2 (skipping height 1) should error
    let mut batch = Store::get_write_batch();
    assert!(
        store
            .append_to_confirmed(&share2.block_hash(), 2, &mut metadata2, &mut batch)
            .is_err()
    );
}

// ── candidate_and_confirmed_are_independent ──────────────────────

#[test]
fn test_candidate_and_confirmed_are_independent() {
    // Candidate and confirmed should be stored separately at the same height
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let candidate_share = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let confirmed_share = TestShareBlockBuilder::new().nonce(0xe9695792).build();

    // Add shares first
    let mut batch = Store::get_write_batch();
    store.add_share_block(&candidate_share, &mut batch).unwrap();
    store.add_share_block(&confirmed_share, &mut batch).unwrap();
    let mut candidate_metadata = BlockMetadata {
        expected_height: Some(0),
        chain_work: candidate_share.header.get_work(),
        status: Status::HeaderValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(
            &candidate_share.block_hash(),
            &candidate_metadata,
            &mut batch,
        )
        .unwrap();
    let mut confirmed_metadata = BlockMetadata {
        expected_height: Some(0),
        chain_work: confirmed_share.header.get_work(),
        status: Status::HeaderValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(
            &confirmed_share.block_hash(),
            &confirmed_metadata,
            &mut batch,
        )
        .unwrap();
    store
        .append_to_candidates(
            &candidate_share.block_hash(),
            0,
            &mut candidate_metadata,
            &mut batch,
        )
        .unwrap();
    store.commit_batch(batch).unwrap();

    let mut batch = Store::get_write_batch();
    store
        .append_to_confirmed(
            &confirmed_share.block_hash(),
            0,
            &mut confirmed_metadata,
            &mut batch,
        )
        .unwrap();
    store.commit_batch(batch).unwrap();

    // Both should be retrievable independently
    assert_eq!(
        store.get_candidate_at_height(0).unwrap(),
        candidate_share.block_hash()
    );
    assert_eq!(
        store.get_confirmed_at_height(0).unwrap(),
        confirmed_share.block_hash()
    );
}

// ── is_confirmed tests ────────────────────────────────────────────

#[test]
fn test_is_confirmed_returns_true_when_confirmed() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // Create and add a share to the store (this sets up block metadata)
    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Add a child share and mark it confirmed
    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    store.push_to_candidate_chain(&share1).unwrap();
    let mut batch = Store::get_write_batch();
    let mut metadata = store.get_block_metadata(&share1.block_hash()).unwrap();
    store
        .append_to_confirmed(
            &share1.block_hash(),
            metadata.expected_height.unwrap(),
            &mut metadata,
            &mut batch,
        )
        .unwrap();
    store.commit_batch(batch).unwrap();

    // is_confirmed should return true
    assert!(store.is_confirmed(&share1.block_hash()));
}

#[test]
fn test_is_confirmed_returns_true_for_genesis_after_setup() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // Create and add a share to the store (this sets up block metadata)
    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Genesis is marked confirmed at setup
    assert!(store.is_confirmed(&genesis.block_hash()));
}

#[test]
fn test_is_confirmed_returns_false_when_not_confirmed() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // Create and add a share to the store (this sets up block metadata)
    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Create another share
    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&share2, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Don't mark it as confirmed - is_confirmed should return false
    assert!(!store.is_confirmed(&share2.block_hash()));
}

#[test]
fn test_is_confirmed_returns_false_when_different_block_confirmed() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // Create genesis and add to store
    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Create another share at the same height
    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    store.push_to_candidate_chain(&share2).unwrap();

    // Create a third share at the same height as share2
    let share3 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695793)
        .build();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&share3, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Mark share2 as confirmed at height 1
    let mut batch = Store::get_write_batch();
    let mut metadata2 = store.get_block_metadata(&share2.block_hash()).unwrap();
    store
        .append_to_confirmed(
            &share2.block_hash(),
            metadata2.expected_height.unwrap(),
            &mut metadata2,
            &mut batch,
        )
        .unwrap();
    store.commit_batch(batch).unwrap();

    // share2 should be confirmed, share3 should not
    assert!(store.is_confirmed(&share2.block_hash()));
    assert!(!store.is_confirmed(&share3.block_hash()));
}

#[test]
fn test_is_confirmed_returns_false_when_no_metadata() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // Create a blockhash that doesn't exist in the store
    let fake_blockhash = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
        .parse::<BlockHash>()
        .unwrap();

    // is_confirmed should return false because there's no metadata
    assert!(!store.is_confirmed(&fake_blockhash));
}

// ── get_confirmed / get_confirmed_chain tests ───────────────────

#[test]
fn test_get_confirmed_returns_blockhashes_in_range() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let share0 = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let share1 = TestShareBlockBuilder::new().nonce(0xe9695792).build();
    let share2 = TestShareBlockBuilder::new().nonce(0xe9695793).build();

    // append_to_confirmed checks top height against DB, so commit between each
    let mut batch = Store::get_write_batch();
    store.add_share_block(&share0, &mut batch).unwrap();
    let mut m0 = BlockMetadata {
        expected_height: Some(0),
        chain_work: share0.header.get_work(),
        status: Status::HeaderValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(&share0.block_hash(), &m0, &mut batch)
        .unwrap();
    store
        .append_to_confirmed(&share0.block_hash(), 0, &mut m0, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let mut batch = Store::get_write_batch();
    store.add_share_block(&share1, &mut batch).unwrap();
    let mut m1 = BlockMetadata {
        expected_height: Some(1),
        chain_work: share1.header.get_work(),
        status: Status::HeaderValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(&share1.block_hash(), &m1, &mut batch)
        .unwrap();
    store
        .append_to_confirmed(&share1.block_hash(), 1, &mut m1, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let mut batch = Store::get_write_batch();
    store.add_share_block(&share2, &mut batch).unwrap();
    let mut m2 = BlockMetadata {
        expected_height: Some(2),
        chain_work: share2.header.get_work(),
        status: Status::HeaderValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(&share2.block_hash(), &m2, &mut batch)
        .unwrap();
    store
        .append_to_confirmed(&share2.block_hash(), 2, &mut m2, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    // Full range
    let result = store.get_confirmed(0, 2).unwrap();
    assert_eq!(result.len(), 3);
    assert_eq!(result[0], (0, share0.block_hash()));
    assert_eq!(result[1], (1, share1.block_hash()));
    assert_eq!(result[2], (2, share2.block_hash()));

    // Sub-range
    let result = store.get_confirmed(1, 2).unwrap();
    assert_eq!(result.len(), 2);
    assert_eq!(result[0], (1, share1.block_hash()));
    assert_eq!(result[1], (2, share2.block_hash()));

    // Single height
    let result = store.get_confirmed(1, 1).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0], (1, share1.block_hash()));
}

#[test]
fn test_get_confirmed_returns_empty_when_from_greater_than_to() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let result = store.get_confirmed(5, 3).unwrap();
    assert!(result.is_empty());
}

#[test]
fn test_get_confirmed_chain_returns_confirmed_from_blockhash_to_top() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(0xe9695793)
        .build();

    // append_to_confirmed checks top height against DB, so commit between each
    store.push_to_candidate_chain(&share1).unwrap();
    let mut batch = Store::get_write_batch();
    let mut m1 = store.get_block_metadata(&share1.block_hash()).unwrap();
    store
        .append_to_confirmed(
            &share1.block_hash(),
            m1.expected_height.unwrap(),
            &mut m1,
            &mut batch,
        )
        .unwrap();
    store.commit_batch(batch).unwrap();

    store.push_to_candidate_chain(&share2).unwrap();
    let mut batch = Store::get_write_batch();
    let mut m2 = store.get_block_metadata(&share2.block_hash()).unwrap();
    store
        .append_to_confirmed(
            &share2.block_hash(),
            m2.expected_height.unwrap(),
            &mut m2,
            &mut batch,
        )
        .unwrap();
    store.commit_batch(batch).unwrap();

    let top = store.get_top_confirmed().ok();
    let result = store
        .get_confirmed_chain(&share1.block_hash(), top.as_ref())
        .unwrap();
    assert_eq!(result.len(), 2);
    assert_eq!(result[0], (1, share1.block_hash()));
    assert_eq!(result[1], (2, share2.block_hash()));
}

#[test]
fn test_get_confirmed_chain_errors_when_no_top_confirmed() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let result = store.get_confirmed_chain(&genesis.block_hash(), None);
    assert!(result.is_err());
}

// ── should_reorg_confirmed tests ─────────────────────────────────

#[test]
fn test_should_reorg_confirmed_false_when_candidates_empty() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let top_confirmed = store.get_top_confirmed().unwrap();
    let candidates: Chain = Vec::new();

    assert!(!store.should_reorg_confirmed(&top_confirmed, &candidates));
}

#[test]
fn test_should_reorg_confirmed_false_when_less_work() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Confirm share1 with more work at h:1
    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .work(3)
        .nonce(0xe9695792)
        .build();
    store.push_to_candidate_chain(&share1).unwrap();
    let mut batch = Store::get_write_batch();
    let mut m1 = store.get_block_metadata(&share1.block_hash()).unwrap();
    store
        .append_to_confirmed(
            &share1.block_hash(),
            m1.expected_height.unwrap(),
            &mut m1,
            &mut batch,
        )
        .unwrap();
    store.commit_batch(batch).unwrap();

    let top_confirmed = store.get_top_confirmed().unwrap();

    // Candidate fork from genesis with less work
    let fork = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .work(2)
        .nonce(0xe9695793)
        .build();
    store.push_to_candidate_chain(&fork).unwrap();

    let candidates = vec![(1, fork.block_hash())];
    assert!(!store.should_reorg_confirmed(&top_confirmed, &candidates));
}

#[test]
fn test_should_reorg_confirmed_false_when_equal_work() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Confirm share1 at h:1
    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .work(5)
        .nonce(0xe9695792)
        .build();
    store.push_to_candidate_chain(&share1).unwrap();
    let mut batch = Store::get_write_batch();
    let mut m1 = store.get_block_metadata(&share1.block_hash()).unwrap();
    store
        .append_to_confirmed(
            &share1.block_hash(),
            m1.expected_height.unwrap(),
            &mut m1,
            &mut batch,
        )
        .unwrap();
    store.commit_batch(batch).unwrap();

    let top_confirmed = store.get_top_confirmed().unwrap();

    // Candidate fork with equal work
    let fork = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .work(5)
        .nonce(0xe9695793)
        .build();
    store.push_to_candidate_chain(&fork).unwrap();

    let candidates = vec![(1, fork.block_hash())];
    assert!(!store.should_reorg_confirmed(&top_confirmed, &candidates));
}

#[test]
fn test_should_reorg_confirmed_false_when_extends_confirmed() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let top_confirmed = store.get_top_confirmed().unwrap();

    // Candidate is a child of confirmed at height+1 (extends, not reorg)
    let child = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .work(5)
        .nonce(0xe9695792)
        .build();
    store.push_to_candidate_chain(&child).unwrap();

    let candidates = vec![(1, child.block_hash())];
    assert!(!store.should_reorg_confirmed(&top_confirmed, &candidates));
}

/// Candidate fork branches off genesis (not extending confirmed tip share1).
///
/// Confirmed: genesis(h:0) → share1(h:1)
/// Candidates: fork(h:1, parent=genesis, more work)
#[test]
fn test_should_reorg_confirmed_true_when_more_work_and_not_extending() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Confirm share1 at h:1
    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    store.push_to_candidate_chain(&share1).unwrap();
    let mut batch = Store::get_write_batch();
    let mut m1 = store.get_block_metadata(&share1.block_hash()).unwrap();
    store
        .append_to_confirmed(
            &share1.block_hash(),
            m1.expected_height.unwrap(),
            &mut m1,
            &mut batch,
        )
        .unwrap();
    store.commit_batch(batch).unwrap();

    let top_confirmed = store.get_top_confirmed().unwrap();

    // Fork branches from genesis (NOT from share1), with more work
    let fork = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .work(4)
        .nonce(0xe9695793)
        .build();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&fork, &mut batch).unwrap();
    let fork_metadata = BlockMetadata {
        expected_height: Some(1),
        chain_work: fork.header.get_work(),
        status: Status::HeaderValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(&fork.block_hash(), &fork_metadata, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    // fork's parent is genesis, not share1 (top confirmed), so not extending
    let candidates = vec![(1, fork.block_hash())];
    assert!(store.should_reorg_confirmed(&top_confirmed, &candidates));
}

/// Multi-entry candidate chain with last entry having more work.
///
/// Confirmed: genesis(h:0) → share1(h:1) → share2(h:2)
/// Candidates: fork1(h:1, parent=genesis) → fork2(h:2, more work)
#[test]
fn test_should_reorg_confirmed_true_for_deeper_fork() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Build confirmed: genesis -> share1(h:1) -> share2(h:2)
    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    store.push_to_candidate_chain(&share1).unwrap();
    let mut batch = Store::get_write_batch();
    let mut m1 = store.get_block_metadata(&share1.block_hash()).unwrap();
    store
        .append_to_confirmed(
            &share1.block_hash(),
            m1.expected_height.unwrap(),
            &mut m1,
            &mut batch,
        )
        .unwrap();
    store.commit_batch(batch).unwrap();

    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(0xe9695793)
        .build();
    store.push_to_candidate_chain(&share2).unwrap();
    let mut batch = Store::get_write_batch();
    let mut m2 = store.get_block_metadata(&share2.block_hash()).unwrap();
    store
        .append_to_confirmed(
            &share2.block_hash(),
            m2.expected_height.unwrap(),
            &mut m2,
            &mut batch,
        )
        .unwrap();
    store.commit_batch(batch).unwrap();

    let top_confirmed = store.get_top_confirmed().unwrap();

    // Build fork: genesis -> fork1(h:1) -> fork2(h:2, more work)
    let fork1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695794)
        .build();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&fork1, &mut batch).unwrap();
    let fork1_metadata = BlockMetadata {
        expected_height: Some(1),
        chain_work: fork1.header.get_work(),
        status: Status::HeaderValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(&fork1.block_hash(), &fork1_metadata, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let fork2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(fork1.block_hash().to_string())
        .work(4)
        .nonce(0xe9695795)
        .build();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&fork2, &mut batch).unwrap();
    let fork2_metadata = BlockMetadata {
        expected_height: Some(2),
        chain_work: fork1_metadata.chain_work + fork2.header.get_work(),
        status: Status::HeaderValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(&fork2.block_hash(), &fork2_metadata, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    // fork1's parent is genesis, not share2 (top confirmed)
    let candidates = vec![(1, fork1.block_hash()), (2, fork2.block_hash())];
    assert!(store.should_reorg_confirmed(&top_confirmed, &candidates));
}

// ── reorg_confirmed tests ───────────────────────────────────────

/// Simple reorg: fork replaces a single confirmed entry.
///
/// Before: genesis(h:0, confirmed) → A(h:1, confirmed)
/// Fork:   genesis → F(h:1, candidate, more work)
/// After:  genesis(h:0, confirmed) → F(h:1, confirmed)
///         A has Valid status, candidate chain still present
#[test]
fn test_reorg_confirmed_replaces_single_entry() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Confirm A at h:1
    let a = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&a, &mut batch).unwrap();
    let mut metadata_a = BlockMetadata {
        expected_height: Some(1),
        chain_work: a.header.get_work(),
        status: Status::HeaderValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(&a.block_hash(), &metadata_a, &mut batch)
        .unwrap();
    store
        .append_to_confirmed(&a.block_hash(), 1, &mut metadata_a, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let top_confirmed = store.get_top_confirmed().unwrap();

    // Fork F from genesis with more work, stored as candidate.
    // BlockValid because reorg_confirmed only confirms validated in-zone blocks.
    let fork_share = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .work(3)
        .nonce(0xe9695793)
        .build();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&fork_share, &mut batch).unwrap();
    let mut fork_metadata = BlockMetadata {
        expected_height: Some(1),
        chain_work: fork_share.header.get_work(),
        status: Status::BlockValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(&fork_share.block_hash(), &fork_metadata, &mut batch)
        .unwrap();
    store
        .append_to_candidates(&fork_share.block_hash(), 1, &mut fork_metadata, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    // Reorg confirmed
    let candidates = vec![(1, fork_share.block_hash())];
    let mut batch = Store::get_write_batch();
    let result = store
        .reorg_confirmed(&top_confirmed, &candidates, 0, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    // New top confirmed is at h:1
    assert_eq!(result, Some(1));
    assert_eq!(store.get_top_confirmed_height().unwrap(), 1);
    assert_eq!(
        store.get_confirmed_at_height(1).unwrap(),
        fork_share.block_hash()
    );

    // Genesis still confirmed at h:0
    assert_eq!(
        store.get_confirmed_at_height(0).unwrap(),
        genesis.block_hash()
    );

    // A is reorged out with Valid status
    let updated_metadata_a = store.get_block_metadata(&a.block_hash()).unwrap();
    assert_eq!(updated_metadata_a.status, Status::HeaderValid);

    // F has Confirmed status
    assert!(store.is_confirmed(&fork_share.block_hash()));

    // Candidate chain coexists with confirmed
    assert!(store.get_top_candidate().is_ok());
    assert_eq!(
        store.get_candidate_at_height(1).unwrap(),
        fork_share.block_hash()
    );
}

/// Deeper reorg: fork replaces multiple confirmed entries.
///
/// Before: genesis(h:0) → A(h:1) → B(h:2)  [all confirmed]
/// Fork:   genesis → F1(h:1) → F2(h:2, more work)  [candidates]
/// After:  genesis(h:0) → F1(h:1) → F2(h:2)  [all confirmed]
///         A, B have Valid status, candidate chain still present
#[test]
fn test_reorg_confirmed_replaces_multiple_entries() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Build confirmed: genesis -> A(h:1) -> B(h:2)
    let share_a = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&share_a, &mut batch).unwrap();
    let mut metadata_a = BlockMetadata {
        expected_height: Some(1),
        chain_work: share_a.header.get_work(),
        status: Status::HeaderValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(&share_a.block_hash(), &metadata_a, &mut batch)
        .unwrap();
    store
        .append_to_confirmed(&share_a.block_hash(), 1, &mut metadata_a, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let share_b = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_a.block_hash().to_string())
        .nonce(0xe9695793)
        .build();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&share_b, &mut batch).unwrap();
    let mut metadata_b = BlockMetadata {
        expected_height: Some(2),
        chain_work: metadata_a.chain_work + share_b.header.get_work(),
        status: Status::HeaderValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(&share_b.block_hash(), &metadata_b, &mut batch)
        .unwrap();
    store
        .append_to_confirmed(&share_b.block_hash(), 2, &mut metadata_b, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let top_confirmed = store.get_top_confirmed().unwrap();

    // Build fork: genesis -> F1(h:1) -> F2(h:2, more work)
    let fork_1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695794)
        .build();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&fork_1, &mut batch).unwrap();
    // BlockValid because reorg_confirmed only confirms validated in-zone blocks.
    let mut fork_1_metadata = BlockMetadata {
        expected_height: Some(1),
        chain_work: fork_1.header.get_work(),
        status: Status::BlockValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(&fork_1.block_hash(), &fork_1_metadata, &mut batch)
        .unwrap();
    store
        .append_to_candidates(&fork_1.block_hash(), 1, &mut fork_1_metadata, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let fork_2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(fork_1.block_hash().to_string())
        .work(4)
        .nonce(0xe9695795)
        .build();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&fork_2, &mut batch).unwrap();
    let mut fork_2_metadata = BlockMetadata {
        expected_height: Some(2),
        chain_work: fork_1_metadata.chain_work + fork_2.header.get_work(),
        status: Status::BlockValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(&fork_2.block_hash(), &fork_2_metadata, &mut batch)
        .unwrap();
    store
        .append_to_candidates(&fork_2.block_hash(), 2, &mut fork_2_metadata, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    // Reorg confirmed
    let candidates = vec![(1, fork_1.block_hash()), (2, fork_2.block_hash())];
    let mut batch = Store::get_write_batch();
    let result = store
        .reorg_confirmed(&top_confirmed, &candidates, 0, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    assert_eq!(result, Some(2));
    assert_eq!(store.get_top_confirmed_height().unwrap(), 2);
    assert_eq!(
        store.get_confirmed_at_height(1).unwrap(),
        fork_1.block_hash()
    );
    assert_eq!(
        store.get_confirmed_at_height(2).unwrap(),
        fork_2.block_hash()
    );

    // Genesis still confirmed
    assert_eq!(
        store.get_confirmed_at_height(0).unwrap(),
        genesis.block_hash()
    );

    // A and B reorged out with Valid status
    let reloaded_metadata_a = store.get_block_metadata(&share_a.block_hash()).unwrap();
    assert_eq!(reloaded_metadata_a.status, Status::HeaderValid);
    let reloaded_metadata_b = store.get_block_metadata(&share_b.block_hash()).unwrap();
    assert_eq!(reloaded_metadata_b.status, Status::HeaderValid);

    // F1 and F2 confirmed
    assert!(store.is_confirmed(&fork_1.block_hash()));
    assert!(store.is_confirmed(&fork_2.block_hash()));

    // Candidate chain coexists with confirmed
    assert!(store.get_top_candidate().is_ok());
    assert_eq!(
        store.get_candidate_at_height(1).unwrap(),
        fork_1.block_hash()
    );
    assert_eq!(
        store.get_candidate_at_height(2).unwrap(),
        fork_2.block_hash()
    );
}

/// Reorg to a shorter fork with more work.
///
/// Before: genesis(h:0) → A(h:1) → B(h:2) → C(h:3)  [confirmed]
/// Fork:   genesis → F(h:1, much more work)  [candidate]
/// After:  genesis(h:0) → F(h:1)  [confirmed]
///         A, B, C have Valid status, candidate chain still present
#[test]
fn test_reorg_confirmed_to_shorter_chain() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Build confirmed: genesis -> A -> B -> C
    let share_a = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&share_a, &mut batch).unwrap();
    let mut metadata_a = BlockMetadata {
        expected_height: Some(1),
        chain_work: share_a.header.get_work(),
        status: Status::HeaderValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(&share_a.block_hash(), &metadata_a, &mut batch)
        .unwrap();
    store
        .append_to_confirmed(&share_a.block_hash(), 1, &mut metadata_a, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let share_b = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_a.block_hash().to_string())
        .nonce(0xe9695793)
        .build();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&share_b, &mut batch).unwrap();
    let mut metadata_b = BlockMetadata {
        expected_height: Some(2),
        chain_work: metadata_a.chain_work + share_b.header.get_work(),
        status: Status::HeaderValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(&share_b.block_hash(), &metadata_b, &mut batch)
        .unwrap();
    store
        .append_to_confirmed(&share_b.block_hash(), 2, &mut metadata_b, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let share_c = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_b.block_hash().to_string())
        .nonce(0xe9695794)
        .build();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&share_c, &mut batch).unwrap();
    let mut metadata_c = BlockMetadata {
        expected_height: Some(3),
        chain_work: metadata_b.chain_work + share_c.header.get_work(),
        status: Status::HeaderValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(&share_c.block_hash(), &metadata_c, &mut batch)
        .unwrap();
    store
        .append_to_confirmed(&share_c.block_hash(), 3, &mut metadata_c, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let top_confirmed = store.get_top_confirmed().unwrap();

    // Fork F from genesis with much more work.
    // BlockValid because reorg_confirmed only confirms validated in-zone blocks.
    let fork_share = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .work(4)
        .nonce(0xe9695795)
        .build();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&fork_share, &mut batch).unwrap();
    let mut fork_metadata = BlockMetadata {
        expected_height: Some(1),
        chain_work: fork_share.header.get_work(),
        status: Status::BlockValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(&fork_share.block_hash(), &fork_metadata, &mut batch)
        .unwrap();
    store
        .append_to_candidates(&fork_share.block_hash(), 1, &mut fork_metadata, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let candidates = vec![(1, fork_share.block_hash())];
    let mut batch = Store::get_write_batch();
    let result = store
        .reorg_confirmed(&top_confirmed, &candidates, 0, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    // New confirmed chain is shorter: genesis → F
    assert_eq!(result, Some(1));
    assert_eq!(store.get_top_confirmed_height().unwrap(), 1);
    assert_eq!(
        store.get_confirmed_at_height(1).unwrap(),
        fork_share.block_hash()
    );

    // Old entries at h:2 and h:3 are gone
    assert!(store.get_confirmed_at_height(2).is_err());
    assert!(store.get_confirmed_at_height(3).is_err());

    // A, B, C reorged out with Valid status
    for hash in [
        share_a.block_hash(),
        share_b.block_hash(),
        share_c.block_hash(),
    ] {
        let meta = store.get_block_metadata(&hash).unwrap();
        assert_eq!(meta.status, Status::HeaderValid);
    }

    // Candidate chain coexists with confirmed
    assert!(store.get_top_candidate().is_ok());
    assert_eq!(
        store.get_candidate_at_height(1).unwrap(),
        fork_share.block_hash()
    );
}

/// Partial reorg: fork branches from a middle confirmed share.
///
/// Before: genesis(h:0) → A(h:1) → B(h:2)  [confirmed]
/// Fork:   A → F(h:2, more work)  [candidate]
/// After:  genesis(h:0) → A(h:1) → F(h:2)  [confirmed]
///         B has Valid status, A stays Confirmed, candidate chain still present
#[test]
fn test_reorg_confirmed_partial_from_mid_chain() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Confirmed: genesis -> A(h:1) -> B(h:2)
    let share_a = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&share_a, &mut batch).unwrap();
    let mut metadata_a = BlockMetadata {
        expected_height: Some(1),
        chain_work: share_a.header.get_work(),
        status: Status::HeaderValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(&share_a.block_hash(), &metadata_a, &mut batch)
        .unwrap();
    store
        .append_to_confirmed(&share_a.block_hash(), 1, &mut metadata_a, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let share_b = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_a.block_hash().to_string())
        .nonce(0xe9695793)
        .build();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&share_b, &mut batch).unwrap();
    let mut metadata_b = BlockMetadata {
        expected_height: Some(2),
        chain_work: metadata_a.chain_work + share_b.header.get_work(),
        status: Status::HeaderValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(&share_b.block_hash(), &metadata_b, &mut batch)
        .unwrap();
    store
        .append_to_confirmed(&share_b.block_hash(), 2, &mut metadata_b, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let top_confirmed = store.get_top_confirmed().unwrap();

    // Fork F from A (h:1, confirmed) with more work
    let fork_share = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_a.block_hash().to_string())
        .work(3)
        .nonce(0xe9695794)
        .build();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&fork_share, &mut batch).unwrap();
    let mut fork_metadata = BlockMetadata {
        expected_height: Some(2),
        chain_work: metadata_a.chain_work + fork_share.header.get_work(),
        status: Status::BlockValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(&fork_share.block_hash(), &fork_metadata, &mut batch)
        .unwrap();
    store
        .append_to_candidates(&fork_share.block_hash(), 2, &mut fork_metadata, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let candidates = vec![(2, fork_share.block_hash())];
    let mut batch = Store::get_write_batch();
    let result = store
        .reorg_confirmed(&top_confirmed, &candidates, 0, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    assert_eq!(result, Some(2));
    assert_eq!(store.get_top_confirmed_height().unwrap(), 2);

    // A stays confirmed (fork_point's parent is still on confirmed)
    assert!(store.is_confirmed(&share_a.block_hash()));
    assert_eq!(
        store.get_confirmed_at_height(1).unwrap(),
        share_a.block_hash()
    );

    // F replaces B
    assert_eq!(
        store.get_confirmed_at_height(2).unwrap(),
        fork_share.block_hash()
    );
    assert!(store.is_confirmed(&fork_share.block_hash()));

    // B reorged out
    let reloaded_metadata_b = store.get_block_metadata(&share_b.block_hash()).unwrap();
    assert_eq!(reloaded_metadata_b.status, Status::HeaderValid);

    // Candidate chain coexists with confirmed
    assert!(store.get_top_candidate().is_ok());
    assert_eq!(
        store.get_candidate_at_height(2).unwrap(),
        fork_share.block_hash()
    );
}

/// Reorg confirmed with empty candidates returns error.
#[test]
fn test_reorg_confirmed_errors_on_empty_candidates() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let top_confirmed = store.get_top_confirmed().unwrap();
    let candidates: Chain = Vec::new();

    let mut batch = Store::get_write_batch();
    let result = store.reorg_confirmed(&top_confirmed, &candidates, 0, &mut batch);
    assert!(result.is_err());
}

// -- SpendsIndex confirmation-gating integration test --

/// End-to-end regression for the "SpendsIndex is gated on
/// confirmation" change: confirming a share writes spends, reorging
/// it out removes them, re-confirming a replacement re-adds them.
#[test]
fn test_spends_index_follows_confirmation_state() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Build a funding coinbase tx and store it so its output exists
    // in the Outputs CF for coinbase_root_height computation.
    let funding_tx = bitcoin::Transaction {
        version: bitcoin::transaction::Version::ONE,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn {
            previous_output: bitcoin::OutPoint::new(bitcoin::Txid::all_zeros(), u32::MAX),
            script_sig: bitcoin::ScriptBuf::new(),
            sequence: bitcoin::Sequence::MAX,
            witness: bitcoin::Witness::new(),
        }],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(1_000),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };
    let funding_txid = funding_tx.compute_txid();
    let mut batch = Store::get_write_batch();
    store
        .add_sharechain_txs(&[ShareTransaction(funding_tx)], 0, &mut batch)
        .unwrap();
    // Index the funding tx into the confirmed genesis block so its output
    // has a confirmed source. Confirming a spender re-checks that its
    // prevout sources are on the confirmed chain.
    store
        .add_txids_to_blocks_index(
            &genesis.block_hash(),
            &Txids(vec![funding_txid]),
            &mut batch,
        )
        .unwrap();
    store.commit_batch(batch).unwrap();

    let prevout = bitcoin::OutPoint::new(funding_txid, 0);
    let spending_tx = bitcoin::Transaction {
        version: bitcoin::transaction::Version::ONE,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn {
            previous_output: prevout,
            script_sig: bitcoin::ScriptBuf::new(),
            sequence: bitcoin::Sequence::MAX,
            witness: bitcoin::Witness::new(),
        }],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(1_000),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };

    // Block A at height 1 containing the spender.
    let share_a = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .add_transaction(spending_tx.clone())
        .build();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&share_a, &mut batch).unwrap();
    let mut metadata_a = BlockMetadata {
        expected_height: Some(1),
        chain_work: share_a.header.get_work(),
        status: Status::HeaderValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(&share_a.block_hash(), &metadata_a, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    // Pre-confirmation: no SpendsIndex entry.
    assert!(!store.is_any_prevout_spent(&[prevout]).unwrap());

    // Confirm A. put_confirmed_entry should populate SpendsIndex.
    let mut batch = Store::get_write_batch();
    store
        .append_to_confirmed(&share_a.block_hash(), 1, &mut metadata_a, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();
    assert!(store.is_any_prevout_spent(&[prevout]).unwrap());

    // Build a heavier fork at height 1 that does NOT contain the
    // spender. Reorg confirms the fork and unconfirms A; the
    // reorg-out path should clear the SpendsIndex entry.
    let fork_1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .work(4)
        .nonce(0xe9695793)
        .build();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&fork_1, &mut batch).unwrap();
    // BlockValid because reorg_confirmed only confirms validated in-zone blocks.
    let mut fork_1_metadata = BlockMetadata {
        expected_height: Some(1),
        chain_work: fork_1.header.get_work(),
        status: Status::BlockValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(&fork_1.block_hash(), &fork_1_metadata, &mut batch)
        .unwrap();
    store
        .append_to_candidates(&fork_1.block_hash(), 1, &mut fork_1_metadata, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let top_confirmed = store.get_top_confirmed().unwrap();
    let candidates = vec![(1, fork_1.block_hash())];
    let mut batch = Store::get_write_batch();
    store
        .reorg_confirmed(&top_confirmed, &candidates, 0, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    assert_eq!(
        store.get_confirmed_at_height(1).unwrap(),
        fork_1.block_hash()
    );
    assert!(!store.is_any_prevout_spent(&[prevout]).unwrap());

    // Build a second fork that extends the new confirmed chain with
    // a block containing the same spender tx, and reorg again so it
    // becomes confirmed.
    let fork_2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(fork_1.block_hash().to_string())
        .work(8)
        .nonce(0xe9695794)
        .add_transaction(spending_tx)
        .build();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&fork_2, &mut batch).unwrap();
    let mut fork_2_metadata = BlockMetadata {
        expected_height: Some(2),
        chain_work: fork_1_metadata.chain_work + fork_2.header.get_work(),
        status: Status::HeaderValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(&fork_2.block_hash(), &fork_2_metadata, &mut batch)
        .unwrap();
    store
        .append_to_candidates(&fork_2.block_hash(), 2, &mut fork_2_metadata, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    // Extend confirmed by promoting fork_2.
    let mut batch = Store::get_write_batch();
    store
        .extend_confirmed(&[(2, fork_2.block_hash())], &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    assert_eq!(
        store.get_confirmed_at_height(2).unwrap(),
        fork_2.block_hash()
    );
    assert!(store.is_any_prevout_spent(&[prevout]).unwrap());
}

/// Two candidates on the same chain that spend the same confirmed output
/// must not both be confirmed.
///
/// Each passed ingest prevout validation because, at its own validation
/// time, the other spender was still an unconfirmed candidate and so had no
/// SpendsIndex entry. Confirming them in one batch would record both
/// spends, the second silently overwriting the first.
///
/// Setup: genesis(confirmed h:0), a confirmed funding output
///        spender_1(h:1, BlockValid, candidate) spends the output
///        spender_2(h:2, BlockValid, candidate) spends the SAME output
/// Action: extend_confirmed over both candidates
/// After:  only spender_1 is confirmed, spender_2 is Invalid, top is 1
#[test]
fn test_extend_confirmed_rejects_same_chain_double_spend() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xf0000001).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // A funding output whose source tx is on the confirmed chain (genesis).
    let funding_tx = bitcoin::Transaction {
        version: bitcoin::transaction::Version::ONE,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn {
            previous_output: bitcoin::OutPoint::new(bitcoin::Txid::all_zeros(), u32::MAX),
            script_sig: bitcoin::ScriptBuf::new(),
            sequence: bitcoin::Sequence::MAX,
            witness: bitcoin::Witness::new(),
        }],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(1_000),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };
    let funding_txid = funding_tx.compute_txid();
    let mut batch = Store::get_write_batch();
    store
        .add_sharechain_txs(&[ShareTransaction(funding_tx)], 0, &mut batch)
        .unwrap();
    store
        .add_txids_to_blocks_index(
            &genesis.block_hash(),
            &Txids(vec![funding_txid]),
            &mut batch,
        )
        .unwrap();
    store.commit_batch(batch).unwrap();

    // Two distinct transactions (different output values, so different
    // txids) that spend the same prevout.
    let prevout = bitcoin::OutPoint::new(funding_txid, 0);
    let spend_of = |value: u64| bitcoin::Transaction {
        version: bitcoin::transaction::Version::ONE,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn {
            previous_output: prevout,
            script_sig: bitcoin::ScriptBuf::new(),
            sequence: bitcoin::Sequence::MAX,
            witness: bitcoin::Witness::new(),
        }],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(value),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };

    let spender_1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xf0000002)
        .add_transaction(spend_of(1_000))
        .build();
    let spender_2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(spender_1.block_hash().to_string())
        .nonce(0xf0000003)
        .add_transaction(spend_of(900))
        .build();

    // Both reached the candidate chain as BlockValid: neither saw the
    // other's spend at ingest. Commit each separately because storing a
    // block reads its parent's committed metadata.
    for (height, block) in [(1u32, &spender_1), (2u32, &spender_2)] {
        let mut batch = Store::get_write_batch();
        store.add_share_block(block, &mut batch).unwrap();
        let mut metadata = BlockMetadata {
            expected_height: Some(height),
            chain_work: block.header.get_work(),
            status: Status::BlockValid,
            chain: ChainMembership::None,
        };
        store
            .update_block_metadata(&block.block_hash(), &metadata, &mut batch)
            .unwrap();
        store
            .append_to_candidates(&block.block_hash(), height, &mut metadata, &mut batch)
            .unwrap();
        store.commit_batch(batch).unwrap();
    }

    let candidates = vec![(1, spender_1.block_hash()), (2, spender_2.block_hash())];
    let mut batch = Store::get_write_batch();
    let new_top = store.extend_confirmed(&candidates, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    assert_eq!(new_top, Some(1), "only the first spender may be confirmed");
    assert_eq!(
        store.get_confirmed_at_height(1).unwrap(),
        spender_1.block_hash()
    );
    assert!(
        store.get_confirmed_at_height(2).is_err(),
        "the double-spending block must not be confirmed"
    );
    assert_eq!(store.get_top_confirmed_height().unwrap(), 1);
    assert_eq!(
        store
            .get_block_metadata(&spender_2.block_hash())
            .unwrap()
            .status,
        Status::Invalid,
        "the double-spending block must be marked Invalid"
    );
}

/// A fork block that spends an output whose source leaves the confirmed
/// chain in this very reorg must not be confirmed.
///
/// Setup: genesis(confirmed h:0) -> share_a(confirmed h:1)
///        fork_1(h:1, more work, parent=genesis)
///        fork_2(h:2, parent=fork_1) spends share_a's coinbase output
/// Action: reorg_confirmed with fork_2 as the candidate tip
/// After:  fork_1 is confirmed, fork_2 is Invalid and not confirmed
#[test]
fn test_reorg_confirmed_rejects_fork_block_spending_reorged_out_source() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xf1000001).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // share_a is confirmed at height 1 and carries a funding transaction
    // that exists only in it, so the reorg takes that source off the
    // confirmed chain. (Every builder block shares one coinbase txid, so a
    // coinbase cannot serve as a source unique to one block.)
    let funding_tx = bitcoin::Transaction {
        version: bitcoin::transaction::Version::ONE,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn {
            previous_output: bitcoin::OutPoint::new(bitcoin::Txid::all_zeros(), u32::MAX),
            script_sig: bitcoin::ScriptBuf::new(),
            sequence: bitcoin::Sequence::MAX,
            witness: bitcoin::Witness::new(),
        }],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(4_242),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };
    let source_outpoint = bitcoin::OutPoint::new(funding_tx.compute_txid(), 0);
    let share_a = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xf1000002)
        .add_transaction(funding_tx)
        .build();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&share_a, &mut batch).unwrap();
    let mut metadata_a = BlockMetadata {
        expected_height: Some(1),
        chain_work: share_a.header.get_work(),
        status: Status::BlockValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(&share_a.block_hash(), &metadata_a, &mut batch)
        .unwrap();
    store
        .append_to_confirmed(&share_a.block_hash(), 1, &mut metadata_a, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let fork_1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .work(4)
        .nonce(0xf1000003)
        .build();
    assert!(
        fork_1.header.get_work() > share_a.header.get_work(),
        "test precondition: the fork must outweigh the confirmed chain"
    );
    let mut batch = Store::get_write_batch();
    store.add_share_block(&fork_1, &mut batch).unwrap();
    let mut fork_1_metadata = BlockMetadata {
        expected_height: Some(1),
        chain_work: fork_1.header.get_work(),
        status: Status::BlockValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(&fork_1.block_hash(), &fork_1_metadata, &mut batch)
        .unwrap();
    store
        .append_to_candidates(&fork_1.block_hash(), 1, &mut fork_1_metadata, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    // fork_2 spends share_a's coinbase, which this reorg unconfirms.
    let spending_tx = bitcoin::Transaction {
        version: bitcoin::transaction::Version::ONE,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn {
            previous_output: source_outpoint,
            script_sig: bitcoin::ScriptBuf::new(),
            sequence: bitcoin::Sequence::MAX,
            witness: bitcoin::Witness::new(),
        }],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(1_000),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };
    let fork_2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(fork_1.block_hash().to_string())
        .work(4)
        .nonce(0xf1000004)
        .add_transaction(spending_tx)
        .build();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&fork_2, &mut batch).unwrap();
    let mut fork_2_metadata = BlockMetadata {
        expected_height: Some(2),
        chain_work: fork_1_metadata.chain_work + fork_2.header.get_work(),
        status: Status::BlockValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(&fork_2.block_hash(), &fork_2_metadata, &mut batch)
        .unwrap();
    store
        .append_to_candidates(&fork_2.block_hash(), 2, &mut fork_2_metadata, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let top_confirmed = store.get_top_confirmed().unwrap();
    let candidates = vec![(2, fork_2.block_hash())];
    let mut batch = Store::get_write_batch();
    let new_top = store
        .reorg_confirmed(&top_confirmed, &candidates, 0, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    assert_eq!(new_top, Some(1), "the reorg must stop below fork_2");
    assert_eq!(
        store.get_confirmed_at_height(1).unwrap(),
        fork_1.block_hash()
    );
    assert!(
        store.get_confirmed_at_height(2).is_err(),
        "fork_2 spends a source this reorg unconfirmed, so it must not confirm"
    );
    assert_eq!(
        store
            .get_block_metadata(&fork_2.block_hash())
            .unwrap()
            .status,
        Status::Invalid
    );
}

/// When the fork blocks that survive the prevout re-check no longer
/// outweigh the confirmed chain, the reorg is abandoned rather than
/// regressing the confirmed tip onto a weaker branch.
///
/// Setup: genesis(confirmed h:0) -> share_a(confirmed h:1, heavy)
///        fork_1(h:1, lighter than share_a, parent=genesis)
///        fork_2(h:2, parent=fork_1) spends share_a's coinbase output,
///        so the full fork outweighs share_a but fork_1 alone does not
/// Action: reorg_confirmed with fork_2 as the candidate tip
/// After:  no reorg, share_a stays confirmed, and fork_2's status is
///         untouched -- it only failed the re-check because the overlay
///         assumed share_a was reorged out, which never happened
#[test]
fn test_reorg_confirmed_aborts_when_validated_prefix_lacks_work() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xf2000001).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // A funding transaction that exists only in share_a, so the reorg takes
    // its source off the confirmed chain.
    let funding_tx = bitcoin::Transaction {
        version: bitcoin::transaction::Version::ONE,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn {
            previous_output: bitcoin::OutPoint::new(bitcoin::Txid::all_zeros(), u32::MAX),
            script_sig: bitcoin::ScriptBuf::new(),
            sequence: bitcoin::Sequence::MAX,
            witness: bitcoin::Witness::new(),
        }],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(5_353),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };
    let source_outpoint = bitcoin::OutPoint::new(funding_tx.compute_txid(), 0);
    let share_a = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .work(4)
        .nonce(0xf2000002)
        .add_transaction(funding_tx)
        .build();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&share_a, &mut batch).unwrap();
    let mut metadata_a = BlockMetadata {
        expected_height: Some(1),
        chain_work: share_a.header.get_work(),
        status: Status::BlockValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(&share_a.block_hash(), &metadata_a, &mut batch)
        .unwrap();
    store
        .append_to_confirmed(&share_a.block_hash(), 1, &mut metadata_a, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let fork_1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xf2000003)
        .build();
    assert!(
        fork_1.header.get_work() < share_a.header.get_work(),
        "test precondition: fork_1 alone must not outweigh share_a"
    );
    let mut batch = Store::get_write_batch();
    store.add_share_block(&fork_1, &mut batch).unwrap();
    let mut fork_1_metadata = BlockMetadata {
        expected_height: Some(1),
        chain_work: fork_1.header.get_work(),
        status: Status::BlockValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(&fork_1.block_hash(), &fork_1_metadata, &mut batch)
        .unwrap();
    store
        .append_to_candidates(&fork_1.block_hash(), 1, &mut fork_1_metadata, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let spending_tx = bitcoin::Transaction {
        version: bitcoin::transaction::Version::ONE,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn {
            previous_output: source_outpoint,
            script_sig: bitcoin::ScriptBuf::new(),
            sequence: bitcoin::Sequence::MAX,
            witness: bitcoin::Witness::new(),
        }],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(1_000),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };
    let fork_2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(fork_1.block_hash().to_string())
        .work(8)
        .nonce(0xf2000004)
        .add_transaction(spending_tx)
        .build();
    let fork_2_work = fork_1_metadata.chain_work + fork_2.header.get_work();
    assert!(
        fork_2_work > share_a.header.get_work(),
        "test precondition: the full fork must outweigh share_a"
    );
    let mut batch = Store::get_write_batch();
    store.add_share_block(&fork_2, &mut batch).unwrap();
    let mut fork_2_metadata = BlockMetadata {
        expected_height: Some(2),
        chain_work: fork_2_work,
        status: Status::BlockValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(&fork_2.block_hash(), &fork_2_metadata, &mut batch)
        .unwrap();
    store
        .append_to_candidates(&fork_2.block_hash(), 2, &mut fork_2_metadata, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let top_confirmed = store.get_top_confirmed().unwrap();
    let candidates = vec![(2, fork_2.block_hash())];
    let mut batch = Store::get_write_batch();
    let new_top = store
        .reorg_confirmed(&top_confirmed, &candidates, 0, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    assert_eq!(new_top, None, "the reorg must be abandoned");
    assert_eq!(
        store.get_confirmed_at_height(1).unwrap(),
        share_a.block_hash(),
        "the confirmed chain must be left untouched"
    );
    assert_eq!(store.get_top_confirmed_height().unwrap(), 1);
    assert_eq!(
        store
            .get_block_metadata(&share_a.block_hash())
            .unwrap()
            .chain,
        ChainMembership::Confirmed
    );
    // fork_2 spends an output of share_a, which is still confirmed, so
    // fork_2 is perfectly valid. Marking it Invalid here would be terminal:
    // no later reorg could adopt that branch even once it wins on work.
    let fork_2_metadata = store.get_block_metadata(&fork_2.block_hash()).unwrap();
    assert_eq!(fork_2_metadata.status, Status::BlockValid);
    assert_eq!(fork_2_metadata.chain, ChainMembership::Candidate);
}

/// A reorg may confirm a block together with the block holding the output
/// it spends. `confirm_blocks` runs after the prefix walk, so the source
/// still carries its committed `Candidate` membership while the walk is in
/// progress: without the confirmed-in-batch side of the overlay the
/// spender is truncated out of the prefix and marked Invalid, even though
/// the same batch confirms its source.
///
/// Setup: genesis(confirmed h:0) -> share_a(confirmed h:1, light)
///        fork_1(h:1, holds the funding tx), fork_2(h:2, spends it)
/// Action: reorg_confirmed with fork_2 as the candidate tip
/// After:  both fork blocks confirmed, fork_2 still BlockValid
#[test]
fn test_reorg_confirmed_accepts_spend_of_source_confirmed_in_same_reorg() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xf4000001).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // The confirmed branch this reorg replaces, deliberately light.
    let share_a = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xf4000002)
        .build();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&share_a, &mut batch).unwrap();
    let mut metadata_a = BlockMetadata {
        expected_height: Some(1),
        chain_work: share_a.header.get_work(),
        status: Status::BlockValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(&share_a.block_hash(), &metadata_a, &mut batch)
        .unwrap();
    store
        .append_to_confirmed(&share_a.block_hash(), 1, &mut metadata_a, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    // fork_1 carries the funding output; fork_2 spends it. Both were
    // confirmed together before being reorged out, which is why fork_2
    // passed validate_prevouts at ingest.
    let funding_tx = bitcoin::Transaction {
        version: bitcoin::transaction::Version::ONE,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn {
            previous_output: bitcoin::OutPoint::new(bitcoin::Txid::all_zeros(), u32::MAX),
            script_sig: bitcoin::ScriptBuf::new(),
            sequence: bitcoin::Sequence::MAX,
            witness: bitcoin::Witness::new(),
        }],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(7_777),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };
    let source_outpoint = bitcoin::OutPoint::new(funding_tx.compute_txid(), 0);
    let fork_1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .work(4)
        .nonce(0xf4000003)
        .add_transaction(funding_tx)
        .build();
    assert!(
        fork_1.header.get_work() > share_a.header.get_work(),
        "test precondition: fork_1 alone must outweigh share_a, so the \
             reorg is not abandoned for lack of work"
    );
    let mut batch = Store::get_write_batch();
    store.add_share_block(&fork_1, &mut batch).unwrap();
    let mut fork_1_metadata = BlockMetadata {
        expected_height: Some(1),
        chain_work: fork_1.header.get_work(),
        status: Status::BlockValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(&fork_1.block_hash(), &fork_1_metadata, &mut batch)
        .unwrap();
    store
        .append_to_candidates(&fork_1.block_hash(), 1, &mut fork_1_metadata, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let spending_tx = bitcoin::Transaction {
        version: bitcoin::transaction::Version::ONE,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn {
            previous_output: source_outpoint,
            script_sig: bitcoin::ScriptBuf::new(),
            sequence: bitcoin::Sequence::MAX,
            witness: bitcoin::Witness::new(),
        }],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(1_000),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };
    let fork_2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(fork_1.block_hash().to_string())
        .work(4)
        .nonce(0xf4000004)
        .add_transaction(spending_tx)
        .build();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&fork_2, &mut batch).unwrap();
    let mut fork_2_metadata = BlockMetadata {
        expected_height: Some(2),
        chain_work: fork_1_metadata.chain_work + fork_2.header.get_work(),
        status: Status::BlockValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(&fork_2.block_hash(), &fork_2_metadata, &mut batch)
        .unwrap();
    store
        .append_to_candidates(&fork_2.block_hash(), 2, &mut fork_2_metadata, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let top_confirmed = store.get_top_confirmed().unwrap();
    let candidates = vec![(2, fork_2.block_hash())];
    let mut batch = Store::get_write_batch();
    let new_top = store
        .reorg_confirmed(&top_confirmed, &candidates, 0, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    assert_eq!(new_top, Some(2), "both fork blocks must be confirmed");
    assert_eq!(
        store.get_confirmed_at_height(2).unwrap(),
        fork_2.block_hash()
    );
    assert_eq!(
        store
            .get_block_metadata(&fork_2.block_hash())
            .unwrap()
            .status,
        Status::BlockValid,
        "fork_2's source is confirmed by this same batch, so it is valid"
    );
}

/// A fork block may re-spend an output that the reorged-out branch had
/// spent: the rewind releases that spend, so the output is free again.
/// Guards against rejecting a valid fork block by consulting the committed
/// SpendsIndex without accounting for the spends this reorg removes.
///
/// Setup: genesis(confirmed h:0) holds the funding coinbase output
///        share_a(confirmed h:1) spends it
///        fork_1(h:1, more work, parent=genesis) spends the SAME output
/// Action: reorg_confirmed with fork_1 as the candidate tip
/// After:  fork_1 is confirmed and stays BlockValid
#[test]
fn test_reorg_confirmed_allows_respend_of_reorged_out_spend() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xf3000001).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // The funding output lives in genesis, which is the fork point and so
    // stays confirmed across the reorg.
    let source_outpoint = bitcoin::OutPoint::new(genesis.transactions[0].0.compute_txid(), 0);
    let spend_of = |value: u64| bitcoin::Transaction {
        version: bitcoin::transaction::Version::ONE,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn {
            previous_output: source_outpoint,
            script_sig: bitcoin::ScriptBuf::new(),
            sequence: bitcoin::Sequence::MAX,
            witness: bitcoin::Witness::new(),
        }],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(value),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };

    let share_a = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xf3000002)
        .add_transaction(spend_of(1_000))
        .build();
    // Commit the block before confirming it: put_confirmed_entry reads the
    // committed body to record its spends.
    let mut batch = Store::get_write_batch();
    store.add_share_block(&share_a, &mut batch).unwrap();
    let mut metadata_a = BlockMetadata {
        expected_height: Some(1),
        chain_work: share_a.header.get_work(),
        status: Status::BlockValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(&share_a.block_hash(), &metadata_a, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let mut batch = Store::get_write_batch();
    store
        .append_to_confirmed(&share_a.block_hash(), 1, &mut metadata_a, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();
    assert!(
        store.is_any_prevout_spent(&[source_outpoint]).unwrap(),
        "test precondition: the confirmed chain spends the output"
    );

    // fork_1 spends the same output with a different transaction.
    let fork_1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .work(4)
        .nonce(0xf3000003)
        .add_transaction(spend_of(900))
        .build();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&fork_1, &mut batch).unwrap();
    let mut fork_1_metadata = BlockMetadata {
        expected_height: Some(1),
        chain_work: fork_1.header.get_work(),
        status: Status::BlockValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(&fork_1.block_hash(), &fork_1_metadata, &mut batch)
        .unwrap();
    store
        .append_to_candidates(&fork_1.block_hash(), 1, &mut fork_1_metadata, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let top_confirmed = store.get_top_confirmed().unwrap();
    let candidates = vec![(1, fork_1.block_hash())];
    let mut batch = Store::get_write_batch();
    let new_top = store
        .reorg_confirmed(&top_confirmed, &candidates, 0, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    assert_eq!(new_top, Some(1));
    assert_eq!(
        store.get_confirmed_at_height(1).unwrap(),
        fork_1.block_hash(),
        "the re-spending fork block is valid once the old spend is released"
    );
    assert_eq!(
        store
            .get_block_metadata(&fork_1.block_hash())
            .unwrap()
            .status,
        Status::BlockValid,
        "a valid re-spend must not be marked Invalid"
    );
    assert!(
        store.is_any_prevout_spent(&[source_outpoint]).unwrap(),
        "the output is now spent by the fork block"
    );
}

/// When the very first candidate fails the prevout re-check, nothing is
/// confirmed and the confirmed chain is left exactly as it was. Guards
/// against resetting the top confirmed height when the confirmable prefix
/// is empty.
///
/// Setup: genesis(confirmed h:0) -> share_a(confirmed h:1) spends an output
///        spender(h:2, BlockValid, candidate) spends the SAME output
/// Action: extend_confirmed over the single double-spending candidate
/// After:  nothing confirmed, top stays at 1, the spender is Invalid
#[test]
fn test_extend_confirmed_keeps_chain_when_first_candidate_fails() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xf6000001).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let source_outpoint = bitcoin::OutPoint::new(genesis.transactions[0].0.compute_txid(), 0);
    let spend_of = |value: u64| bitcoin::Transaction {
        version: bitcoin::transaction::Version::ONE,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn {
            previous_output: source_outpoint,
            script_sig: bitcoin::ScriptBuf::new(),
            sequence: bitcoin::Sequence::MAX,
            witness: bitcoin::Witness::new(),
        }],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(value),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };

    // share_a is confirmed at height 1 and spends the output.
    let share_a = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xf6000002)
        .add_transaction(spend_of(1_000))
        .build();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&share_a, &mut batch).unwrap();
    let mut metadata_a = BlockMetadata {
        expected_height: Some(1),
        chain_work: share_a.header.get_work(),
        status: Status::BlockValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(&share_a.block_hash(), &metadata_a, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();
    let mut batch = Store::get_write_batch();
    store
        .append_to_confirmed(&share_a.block_hash(), 1, &mut metadata_a, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    // The only candidate spends the same output, so the confirmable prefix
    // is empty.
    let spender = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_a.block_hash().to_string())
        .nonce(0xf6000003)
        .add_transaction(spend_of(900))
        .build();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&spender, &mut batch).unwrap();
    let mut spender_metadata = BlockMetadata {
        expected_height: Some(2),
        chain_work: spender.header.get_work(),
        status: Status::BlockValid,
        chain: ChainMembership::None,
    };
    store
        .update_block_metadata(&spender.block_hash(), &spender_metadata, &mut batch)
        .unwrap();
    store
        .append_to_candidates(&spender.block_hash(), 2, &mut spender_metadata, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let candidates = vec![(2, spender.block_hash())];
    let mut batch = Store::get_write_batch();
    let new_top = store.extend_confirmed(&candidates, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    assert_eq!(new_top, None, "nothing may be confirmed");
    assert_eq!(
        store.get_top_confirmed_height().unwrap(),
        1,
        "the confirmed tip must not move"
    );
    assert_eq!(
        store.get_confirmed_at_height(1).unwrap(),
        share_a.block_hash()
    );
    assert!(store.get_confirmed_at_height(2).is_err());
    assert_eq!(
        store
            .get_block_metadata(&spender.block_hash())
            .unwrap()
            .status,
        Status::Invalid
    );
}

/// When a block in the fork branch lacks block data, the reorg
/// should not happen.
///
/// Setup: genesis(confirmed h:0) -> share1(confirmed h:1)
///        fork(h:1, more work, parent=genesis) -- header only, no block data
/// Action: call reorg_confirmed with fork as the candidate tip
/// After:  reorg does not happen, confirmed chain unchanged
#[test]
fn test_reorg_confirmed_skipped_when_fork_branch_missing_block_data() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // share1: confirmed at h:1 (has block data)
    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    store.push_to_confirmed_chain(&share1).unwrap();
    assert_eq!(store.get_top_confirmed_height().unwrap(), 1);

    // fork: child of genesis at h:1 with more work, header only (no block data)
    let fork = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .work(2)
        .nonce(0xe9695793)
        .build();
    // Only store header and metadata, not block data
    store.create_valid_metadata_only(&fork);

    assert!(!store.share_block_exists(&fork.block_hash()));

    let top_confirmed = store.get_top_confirmed().unwrap();
    let candidates = vec![(1u32, fork.block_hash())];

    let mut batch = Store::get_write_batch();
    let result = store
        .reorg_confirmed(&top_confirmed, &candidates, 0, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    // Reorg should not have happened
    assert_eq!(result, None);
    assert_eq!(store.get_top_confirmed_height().unwrap(), 1);
    assert_eq!(
        store.get_confirmed_at_height(1).unwrap(),
        share1.block_hash()
    );
}

/// A reorg must not confirm an in-zone fork block that is not `BlockValid`,
/// even when its body is present. This mirrors, on the reorg path, the
/// extend path's promotion gate (`contiguous_candidates_with_block_data`).
///
/// Setup: genesis(confirmed h:0) -> share1(confirmed h:1)
///        fork(h:1, more work, parent=genesis) -- body present but HeaderValid
/// Action: call reorg_confirmed with fork as the candidate tip
/// After:  reorg does not happen, confirmed chain unchanged
///
/// Fail-first: before the status gate the fork's present body passed
/// all_block_and_uncle_data_available and the reorg confirmed an unvalidated
/// block.
#[test]
fn test_reorg_confirmed_skipped_when_fork_block_not_block_valid() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // share1: confirmed at h:1.
    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    store.push_to_confirmed_chain(&share1).unwrap();
    assert_eq!(store.get_top_confirmed_height().unwrap(), 1);

    // fork: child of genesis at h:1 with more work, body present but only
    // HeaderValid (never chain-context validated).
    let fork = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .work(2)
        .nonce(0xe9695793)
        .build();
    store.store_with_valid_metadata(&fork);
    assert!(store.share_block_exists(&fork.block_hash()));
    assert_eq!(
        store.get_block_metadata(&fork.block_hash()).unwrap().status,
        Status::HeaderValid
    );

    let top_confirmed = store.get_top_confirmed().unwrap();
    let candidates = vec![(1u32, fork.block_hash())];

    let mut batch = Store::get_write_batch();
    let result = store
        .reorg_confirmed(&top_confirmed, &candidates, 0, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    // Reorg refused: the fork is not BlockValid.
    assert_eq!(result, None);
    assert_eq!(store.get_top_confirmed_height().unwrap(), 1);
    assert_eq!(
        store.get_confirmed_at_height(1).unwrap(),
        share1.block_hash()
    );
}

// -- all_block_and_uncle_data_available prune_height tests --

#[test]
fn test_block_below_prune_height_passes_without_body() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Header-only block at height 1 (no body)
    let share = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    store.push_to_candidate_chain(&share).unwrap();
    assert!(!store.share_block_exists(&share.block_hash()));

    // prune_height = 2: block at height 1 is below, body not required
    let result = store
        .all_block_and_uncle_data_available(&[share.block_hash()], 2)
        .unwrap();
    assert!(result, "Block below prune_height should pass without body");
}

#[test]
fn test_block_at_prune_height_fails_without_body() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Header-only block at height 1 (no body)
    let share = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    store.push_to_candidate_chain(&share).unwrap();

    // prune_height = 1: block at height 1 is at boundary, body required
    let result = store
        .all_block_and_uncle_data_available(&[share.block_hash()], 1)
        .unwrap();
    assert!(!result, "Block at prune_height should fail without body");
}

#[test]
fn test_uncle_below_prune_height_passes_without_body() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Uncle at height 1 (header-only, no body)
    let uncle = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695793)
        .build();
    store.push_to_candidate_chain(&uncle).unwrap();

    // Main block at height 1 referencing the uncle, with full body
    let main_block = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .uncles(vec![uncle.block_hash()])
        .nonce(0xe9695792)
        .build();
    store.store_with_valid_metadata(&main_block);

    // prune_height = 2: both blocks at height 1 are below boundary
    let result = store
        .all_block_and_uncle_data_available(&[main_block.block_hash()], 2)
        .unwrap();
    assert!(result, "Uncle below prune_height should pass without body");
}

#[test]
fn test_uncle_at_prune_height_fails_without_body() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Uncle at height 1 (header-only, no body)
    let uncle = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695793)
        .build();
    store.push_to_candidate_chain(&uncle).unwrap();

    // Main block at height 1 referencing the uncle, with full body
    let main_block = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .uncles(vec![uncle.block_hash()])
        .nonce(0xe9695792)
        .build();
    store.store_with_valid_metadata(&main_block);

    // prune_height = 1: uncle at height 1 is at boundary, body required
    let result = store
        .all_block_and_uncle_data_available(&[main_block.block_hash()], 1)
        .unwrap();
    assert!(!result, "Uncle at prune_height should fail without body");
}
