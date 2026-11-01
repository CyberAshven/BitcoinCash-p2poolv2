use super::*;
use crate::accounting::payout::sharechain_pplns::pplns_window::{PRUNE_DEPTH, PRUNE_INTERVAL};
use crate::store::organise::TopResult;
use crate::store::prune_blocks::MAX_REORG_DEPTH;
use crate::test_utils::TestShareBlockBuilder;
use bitcoin::Work;
use tempfile::tempdir;

#[test]
fn test_increment_top_candidate_sets_initial_top_from_height() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    assert!(store.get_top_candidate_height().is_err());

    let mut batch = Store::get_write_batch();
    let result = store.increment_top_candidate(5, &mut batch);
    store.commit_batch(batch).unwrap();

    assert_eq!(result.unwrap(), 5);
    assert_eq!(store.get_top_candidate_height().unwrap(), 5);
}

#[test]
fn test_increment_top_candidate_increments_consecutive_height() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // Bootstrap top to 3
    let mut batch = Store::get_write_batch();
    store.increment_top_candidate(3, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Height 4 is exactly 1 more than current top (3)
    let mut batch = Store::get_write_batch();
    let result = store.increment_top_candidate(4, &mut batch);
    store.commit_batch(batch).unwrap();

    assert_eq!(result.unwrap(), 4);
    assert_eq!(store.get_top_candidate_height().unwrap(), 4);
}

#[test]
fn test_increment_top_candidate_errors_on_skipped_height() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let mut batch = Store::get_write_batch();
    store.increment_top_candidate(1, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Height 3 skips over 2
    let mut batch = Store::get_write_batch();
    let result = store.increment_top_candidate(3, &mut batch);

    assert!(result.is_err());
    assert_eq!(store.get_top_candidate_height().unwrap(), 1);
}

#[test]
fn test_increment_top_candidate_errors_on_same_height() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let mut batch = Store::get_write_batch();
    store.increment_top_candidate(2, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // 2 - 2 = 0, not 1
    let mut batch = Store::get_write_batch();
    let result = store.increment_top_candidate(2, &mut batch);

    assert!(result.is_err());
    assert_eq!(store.get_top_candidate_height().unwrap(), 2);
}

// -- append_to_candidate tests -----------------------------------------

#[test]
fn test_append_to_candidate() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695790).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695791)
        .work(1)
        .build();
    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(0xe9695792)
        .work(2)
        .build();

    // Make share1 candidate at height 1
    store.push_to_candidate_chain(&share1).unwrap();

    // Verify we can retrieve it
    let candidate = store.get_candidate_at_height(1).unwrap();
    assert_eq!(candidate, share1.block_hash());

    // Make share2 candidate at height 2
    store.push_to_candidate_chain(&share2).unwrap();

    // Verify both heights
    assert_eq!(
        store.get_candidate_at_height(1).unwrap(),
        share1.block_hash()
    );
    assert_eq!(
        store.get_candidate_at_height(2).unwrap(),
        share2.block_hash()
    );

    // Non-existent height should return None
    assert!(store.get_candidate_at_height(999).is_err());

    // Top candidate height is changed
    assert_eq!(store.get_top_candidate_height().unwrap(), 2);

    // Top candidate is changed
    let top = store.get_top_candidate().unwrap();
    assert_eq!(top.hash, share2.block_hash());
    assert_eq!(top.height, 2);
}

#[test]
fn test_append_to_candidate_on_overwrite_previous_should_not_extend() {
    // Two sibling shares at the same height: push_to_candidate_chain should not
    // extend the candidate chain for the second share (equal work, same
    // height as existing candidate).
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695790).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695791)
        .build();
    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();

    // Organise share1 -- becomes candidate at height 1
    store.push_to_candidate_chain(&share1).unwrap();

    assert_eq!(
        store.get_candidate_at_height(1).unwrap(),
        share1.block_hash()
    );

    // Organise share2 at same height with equal work -- should not extend or reorg
    let result = store.push_to_candidate_chain(&share2).unwrap();
    assert!(result.is_none());
}

#[test]
fn test_push_to_candidate_chain_does_not_extend_when_parent_is_not_top_candidate() {
    // A share whose parent is not the current top candidate and
    // whose cumulative work is not greater should not extend or
    // reorg the candidate chain.
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695790).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // share1 extends genesis to height 1
    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695791)
        .build();
    store.push_to_candidate_chain(&share1).unwrap();

    // share2 extends share1 to height 2
    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    store.push_to_candidate_chain(&share2).unwrap();

    assert_eq!(store.get_top_candidate_height().unwrap(), 2);

    // orphan_share has an unknown parent (not in store), so
    // organise_header returns an error for missing parent.
    let orphan_share = TestShareBlockBuilder::new().nonce(0xe9695793).build();
    let result = store.push_to_candidate_chain(&orphan_share);

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not found"));
    assert_eq!(store.get_top_candidate_height().unwrap(), 2);
}

// -- get_candidates / get_candidates_chain tests -------------------

#[test]
fn test_get_candidates_returns_blockhashes_in_range() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695790).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695791)
        .build();
    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    let share3 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share2.block_hash().to_string())
        .nonce(0xe9695793)
        .build();

    // Add and organise each share sequentially
    for share in [&share1, &share2, &share3] {
        store.push_to_candidate_chain(share).unwrap();
    }

    // Full range (heights 1, 2, 3)
    let result = store.get_candidates(1, 3).unwrap();
    assert_eq!(result.len(), 3);
    assert_eq!(result[0], (1, share1.block_hash()));
    assert_eq!(result[1], (2, share2.block_hash()));
    assert_eq!(result[2], (3, share3.block_hash()));

    // Sub-range
    let result = store.get_candidates(2, 3).unwrap();
    assert_eq!(result.len(), 2);
    assert_eq!(result[0], (2, share2.block_hash()));
    assert_eq!(result[1], (3, share3.block_hash()));

    // Single height
    let result = store.get_candidates(2, 2).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0], (2, share2.block_hash()));
}

#[test]
fn test_get_candidates_returns_empty_when_from_greater_than_to() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let result = store.get_candidates(5, 3).unwrap();
    assert!(result.is_empty());
}

#[test]
fn test_get_candidates_chain_returns_candidates_from_blockhash_to_top() {
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

    // Add and organise each share sequentially
    for share in [&share1, &share2] {
        store.push_to_candidate_chain(share).unwrap();
    }

    let top = store.get_top_candidate().ok();
    let result = store
        .get_candidates_chain(&share1.block_hash(), top.as_ref())
        .unwrap();
    assert_eq!(result.len(), 2);
    assert_eq!(result[0], (1, share1.block_hash()));
    assert_eq!(result[1], (2, share2.block_hash()));
}

#[test]
fn test_get_candidates_chain_errors_when_no_top_candidate() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let result = store.get_candidates_chain(&genesis.block_hash(), None);
    assert!(result.is_err());
}

// -- is_candidate tests --------------------------------------------

#[test]
fn test_is_candidate_returns_true_when_candidate() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Add share and make it a candidate
    let share = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    store.push_to_candidate_chain(&share).unwrap();

    assert!(store.is_candidate(&share.block_hash()));
}

#[test]
fn test_is_candidate_returns_false_when_not_candidate() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Add share but don't make it a candidate (no push_to_candidate_chain call)
    let share = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&share, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    assert!(!store.is_candidate(&share.block_hash()));
}

// -- extend_candidates_at unit tests ------------------------------

#[test]
fn test_extend_candidates_at_returns_share_expected_height_when_no_top_candidate() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let share = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let metadata = BlockMetadata {
        expected_height: Some(5),
        chain_work: share.header.get_work(),
        status: Status::Pending,
        chain: ChainMembership::None,
    };

    let result = store.should_extend_candidates(&share.header, &metadata, None);
    assert_eq!(result.unwrap(), metadata.expected_height);
}

#[test]
fn test_extend_candidates_at_returns_height_when_conditions_match() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let parent = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let parent_hash = parent.block_hash();

    let share = TestShareBlockBuilder::new()
        .prev_share_blockhash(parent_hash.to_string())
        .nonce(0xe9695792)
        .build();

    let metadata = BlockMetadata {
        expected_height: Some(6),
        chain_work: share.header.get_work(),
        status: Status::Pending,
        chain: ChainMembership::None,
    };

    // height == top candidate height + 1 -> 6 == 5 + 1
    let top_candidate = Some(TopResult {
        hash: parent_hash,
        height: 5,
        work: Work::from_hex("0x05").unwrap(),
    });

    let result = store.should_extend_candidates(&share.header, &metadata, top_candidate.as_ref());
    assert_eq!(result.unwrap(), Some(6));
}

#[test]
fn test_extend_candidates_at_returns_none_when_hash_mismatch() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let share = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let different_share = TestShareBlockBuilder::new().nonce(0xe9695792).build();
    let different_hash = different_share.block_hash();

    let metadata = BlockMetadata {
        expected_height: Some(6),
        chain_work: share.header.get_work(),
        status: Status::Pending,
        chain: ChainMembership::None,
    };

    // Height condition met (6 == 5+1), but hash differs from prev_share_blockhash
    let top_candidate = Some(TopResult {
        hash: different_hash,
        height: 5,
        work: Work::from_hex("0x05").unwrap(),
    });

    let result = store.should_extend_candidates(&share.header, &metadata, top_candidate.as_ref());
    assert_eq!(result.unwrap(), None);
}

#[test]
fn test_extend_candidates_at_returns_none_when_height_mismatch() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let parent = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let parent_hash = parent.block_hash();

    let share = TestShareBlockBuilder::new()
        .prev_share_blockhash(parent_hash.to_string())
        .nonce(0xe9695792)
        .build();

    let metadata = BlockMetadata {
        expected_height: Some(7),
        chain_work: share.header.get_work(),
        status: Status::Pending,
        chain: ChainMembership::None,
    };

    // Hash matches but height doesn't (7 != 5+1)
    let top_candidate = Some(TopResult {
        hash: parent_hash,
        height: 5,
        work: Work::from_hex("0x05").unwrap(),
    });

    let result = store.should_extend_candidates(&share.header, &metadata, top_candidate.as_ref());
    assert_eq!(result.unwrap(), None);
}

#[test]
fn test_extend_candidates_at_returns_none_when_both_mismatch() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let share = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let different_share = TestShareBlockBuilder::new().nonce(0xe9695792).build();
    let different_hash = different_share.block_hash();

    let metadata = BlockMetadata {
        expected_height: Some(5),
        chain_work: share.header.get_work(),
        status: Status::Pending,
        chain: ChainMembership::None,
    };

    // Neither hash nor height matches
    let top_candidate = Some(TopResult {
        hash: different_hash,
        height: 10,
        work: Work::from_hex("0x05").unwrap(),
    });

    let result = store.should_extend_candidates(&share.header, &metadata, top_candidate.as_ref());
    assert_eq!(result.unwrap(), None);
}

// -- should_reorg_candidate unit tests ------------------------------

#[test]
fn test_should_reorg_candidate_true_when_more_work_different_hash() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let share = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let top_share = TestShareBlockBuilder::new().nonce(0xe9695792).build();

    let metadata = BlockMetadata {
        expected_height: Some(2),
        chain_work: Work::from_hex("0x10").unwrap(),
        status: Status::HeaderValid,
        chain: ChainMembership::None,
    };

    let top_candidate = Some(TopResult {
        hash: top_share.block_hash(),
        height: 2,
        work: Work::from_hex("0x05").unwrap(),
    });

    assert!(store.should_reorg_candidate(&share.block_hash(), &metadata, top_candidate.as_ref()));
}

#[test]
fn test_should_reorg_candidate_false_when_less_work() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let share = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let top_share = TestShareBlockBuilder::new().nonce(0xe9695792).build();

    let metadata = BlockMetadata {
        expected_height: Some(2),
        chain_work: Work::from_hex("0x03").unwrap(),
        status: Status::HeaderValid,
        chain: ChainMembership::None,
    };

    let top_candidate = Some(TopResult {
        hash: top_share.block_hash(),
        height: 2,
        work: Work::from_hex("0x05").unwrap(),
    });

    assert!(!store.should_reorg_candidate(&share.block_hash(), &metadata, top_candidate.as_ref()));
}

#[test]
fn test_should_reorg_candidate_false_when_equal_work() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let share = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let top_share = TestShareBlockBuilder::new().nonce(0xe9695792).build();

    let metadata = BlockMetadata {
        expected_height: Some(2),
        chain_work: Work::from_hex("0x05").unwrap(),
        status: Status::HeaderValid,
        chain: ChainMembership::None,
    };

    let top_candidate = Some(TopResult {
        hash: top_share.block_hash(),
        height: 2,
        work: Work::from_hex("0x05").unwrap(),
    });

    assert!(!store.should_reorg_candidate(&share.block_hash(), &metadata, top_candidate.as_ref()));
}

#[test]
fn test_should_reorg_candidate_false_when_same_hash() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let share = TestShareBlockBuilder::new().nonce(0xe9695791).build();

    let metadata = BlockMetadata {
        expected_height: Some(2),
        chain_work: Work::from_hex("0x10").unwrap(),
        status: Status::HeaderValid,
        chain: ChainMembership::Candidate,
    };

    // Same blockhash as share -- should not reorg against itself
    let top_candidate = Some(TopResult {
        hash: share.block_hash(),
        height: 2,
        work: Work::from_hex("0x05").unwrap(),
    });

    assert!(!store.should_reorg_candidate(&share.block_hash(), &metadata, top_candidate.as_ref()));
}

#[test]
fn test_should_reorg_candidate_false_when_no_top_candidate() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let share = TestShareBlockBuilder::new().nonce(0xe9695791).build();

    let metadata = BlockMetadata {
        expected_height: Some(2),
        chain_work: Work::from_hex("0x10").unwrap(),
        status: Status::HeaderValid,
        chain: ChainMembership::None,
    };

    assert!(!store.should_reorg_candidate(&share.block_hash(), &metadata, None));
}

// -- pick_best_child unit tests ----------------------------------

#[test]
fn test_pick_best_child_returns_none_for_empty_children() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let result = store
        .pick_best_child(&[], &genesis.block_hash(), 1, None)
        .unwrap();
    assert!(result.is_none());
}

#[test]
fn test_pick_best_child_returns_none_when_no_metadata() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // A blockhash that was never added to the store
    let fake_hash = TestShareBlockBuilder::new()
        .nonce(0xe9695799)
        .build()
        .block_hash();
    let parent = TestShareBlockBuilder::new().nonce(0xe9695791).build();

    let result = store
        .pick_best_child(&[fake_hash], &parent.block_hash(), 1, None)
        .unwrap();
    assert!(result.is_none());
}

#[test]
fn test_pick_best_child_skips_invalid_status() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let child = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    store.store_with_valid_metadata(&child);

    // pick_best_child only picks HeaderValid children, so an Invalid
    // child is skipped even though it exists at the right height.
    let mut metadata = store.get_block_metadata(&child.block_hash()).unwrap();
    metadata.status = Status::Invalid;
    let mut batch = Store::get_write_batch();
    store
        .update_block_metadata(&child.block_hash(), &metadata, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let result = store
        .pick_best_child(&[child.block_hash()], &genesis.block_hash(), 1, None)
        .unwrap();
    assert!(result.is_none());
}

#[test]
fn test_pick_best_child_skips_wrong_height() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let child = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    // Use push_to_candidate_chain to set metadata (height 1, status Candidate)
    // then manually reset status to Valid so pick_best_child sees it
    store.push_to_candidate_chain(&child).unwrap();
    let mut metadata = store.get_block_metadata(&child.block_hash()).unwrap();
    metadata.status = Status::HeaderValid;
    let mut batch = Store::get_write_batch();
    store
        .update_block_metadata(&child.block_hash(), &metadata, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    // Ask for height 5 -- child is at height 1
    let result = store
        .pick_best_child(&[child.block_hash()], &genesis.block_hash(), 5, None)
        .unwrap();
    assert!(result.is_none());
}

#[test]
fn test_pick_best_child_skips_wrong_parent() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // child's prev_share_blockhash is genesis, not some_other
    let child = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    // Use push_to_candidate_chain to set metadata, then reset status to Valid
    store.push_to_candidate_chain(&child).unwrap();
    let mut metadata = store.get_block_metadata(&child.block_hash()).unwrap();
    metadata.status = Status::HeaderValid;
    let mut batch = Store::get_write_batch();
    store
        .update_block_metadata(&child.block_hash(), &metadata, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let wrong_parent = TestShareBlockBuilder::new().nonce(0xe9695799).build();
    let result = store
        .pick_best_child(&[child.block_hash()], &wrong_parent.block_hash(), 1, None)
        .unwrap();
    assert!(result.is_none());
}

#[test]
fn test_pick_best_child_returns_valid_child() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let child = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    // Store block data and create Valid metadata directly
    store.store_with_valid_metadata(&child);

    let result = store
        .pick_best_child(&[child.block_hash()], &genesis.block_hash(), 1, None)
        .unwrap();
    assert!(result.is_some());
    let (hash, result_metadata) = result.unwrap();
    assert_eq!(hash, child.block_hash());
    assert_eq!(result_metadata.expected_height, Some(1));
}

#[test]
fn test_pick_best_child_selects_heaviest_among_multiple() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let light = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .work(1)
        .nonce(0xe9695792)
        .build();
    let heavy = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .work(3)
        .nonce(0xe9695793)
        .build();

    // Store block data and create Valid metadata directly (avoids
    // candidate chain side effects from push_to_candidate_chain).
    store.store_with_valid_metadata(&light);
    store.store_with_valid_metadata(&heavy);

    // Both are valid children at height 1 -- heavy has more work
    let result = store
        .pick_best_child(
            &[light.block_hash(), heavy.block_hash()],
            &genesis.block_hash(),
            1,
            None,
        )
        .unwrap();
    assert_eq!(result.unwrap().0, heavy.block_hash());

    // Order should not matter -- reverse the input
    let result = store
        .pick_best_child(
            &[heavy.block_hash(), light.block_hash()],
            &genesis.block_hash(),
            1,
            None,
        )
        .unwrap();
    assert_eq!(result.unwrap().0, heavy.block_hash());
}

#[test]
fn test_pick_best_child_honours_exclusion() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Two valid children of genesis; the heavier one is excluded, so the
    // lighter one is chosen instead of being the overall best.
    let heavy = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .work(3)
        .build();
    store.store_with_valid_metadata(&heavy);
    let light = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695793)
        .work(1)
        .build();
    store.store_with_valid_metadata(&light);

    let result = store
        .pick_best_child(
            &[heavy.block_hash(), light.block_hash()],
            &genesis.block_hash(),
            1,
            Some(&heavy.block_hash()),
        )
        .unwrap();
    assert_eq!(result.unwrap().0, light.block_hash());
}

// -- get_candidate_blocks_missing_data tests --

#[test]
fn test_missing_data_returns_empty_when_no_candidates() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let result = store.get_candidate_blocks_missing_data(None).unwrap();
    assert!(result.is_empty());
}

#[test]
fn test_missing_data_returns_all_candidates_when_none_confirmed() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695790).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695791)
        .build();
    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(0xe9695792)
        .build();

    store.push_to_candidate_chain(&share1).unwrap();
    store.push_to_candidate_chain(&share2).unwrap();

    // Both candidates have Candidate status (not BlockValid), so both are missing data
    let missing = store.get_candidate_blocks_missing_data(None).unwrap();
    assert_eq!(missing.len(), 2);
    assert_eq!(missing[0], share1.block_hash());
    assert_eq!(missing[1], share2.block_hash());
}

#[test]
fn test_missing_data_skips_block_valid_candidates() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695790).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695791)
        .build();
    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(0xe9695792)
        .build();

    store.push_to_candidate_chain(&share1).unwrap();
    store.push_to_candidate_chain(&share2).unwrap();

    // Mark share1 as BlockValid
    let mut metadata = store.get_block_metadata(&share1.block_hash()).unwrap();
    metadata.status = Status::BlockValid;
    let mut batch = Store::get_write_batch();
    store
        .update_block_metadata(&share1.block_hash(), &metadata, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let missing = store.get_candidate_blocks_missing_data(None).unwrap();
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0], share2.block_hash());
}

#[test]
fn test_missing_data_returns_empty_when_all_block_valid() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695790).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695791)
        .build();
    store.push_to_candidate_chain(&share1).unwrap();

    // Mark as BlockValid
    let mut metadata = store.get_block_metadata(&share1.block_hash()).unwrap();
    metadata.status = Status::BlockValid;
    let mut batch = Store::get_write_batch();
    store
        .update_block_metadata(&share1.block_hash(), &metadata, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let missing = store.get_candidate_blocks_missing_data(None).unwrap();
    assert!(missing.is_empty());
}

#[test]
fn test_missing_data_returns_empty_when_candidate_at_or_below_confirmed() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695790).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695791)
        .build();
    store.push_to_candidate_chain(&share1).unwrap();

    // Set confirmed height to match candidate height (both at 1)
    let mut batch = Store::get_write_batch();
    store.set_top_confirmed_height(1, &mut batch);
    store.commit_batch(batch).unwrap();

    let missing = store.get_candidate_blocks_missing_data(None).unwrap();
    assert!(missing.is_empty());
}

#[test]
fn test_missing_data_only_returns_candidates_above_confirmed() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695790).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695791)
        .build();
    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    let share3 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share2.block_hash().to_string())
        .nonce(0xe9695793)
        .build();

    store.push_to_candidate_chain(&share1).unwrap();
    store.push_to_candidate_chain(&share2).unwrap();
    store.push_to_candidate_chain(&share3).unwrap();

    // Confirmed height is 1, so only candidates at height 2 and 3 are checked
    let mut batch = Store::get_write_batch();
    store.set_top_confirmed_height(1, &mut batch);
    store.commit_batch(batch).unwrap();

    let missing = store.get_candidate_blocks_missing_data(None).unwrap();
    assert_eq!(missing.len(), 2);
    assert_eq!(missing[0], share2.block_hash());
    assert_eq!(missing[1], share3.block_hash());
}

#[test]
fn test_missing_data_includes_uncle_blocks() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695790).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // share1: candidate at h:1
    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695791)
        .build();
    store.push_to_candidate_chain(&share1).unwrap();

    // uncle_block: fork child of genesis at h:1, not on candidate chain.
    // organise_header stores it in BlockHeight at h:1 with HeaderValid
    // status but does not put it on the candidate chain since share1
    // already occupies that slot and has equal or greater work.
    let uncle_block = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695799)
        .build();
    store.create_valid_metadata_only(&uncle_block);

    // share2: candidate at h:2, includes uncle_block as uncle
    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .uncles(vec![uncle_block.block_hash()])
        .nonce(0xe9695792)
        .build();
    store.push_to_candidate_chain(&share2).unwrap();

    let missing = store.get_candidate_blocks_missing_data(None).unwrap();

    // All three should be missing: share1 and uncle_block at h:1, share2 at h:2
    assert_eq!(missing.len(), 3);
    assert!(missing.contains(&share1.block_hash()));
    assert!(missing.contains(&uncle_block.block_hash()));
    assert!(missing.contains(&share2.block_hash()));
}

/// Uncle at a confirmed height is missed by the height scan but
/// should be included because a candidate header references it.
///
/// Scenario: genesis -> share1(h:1, confirmed) -> share2(h:2, candidate).
/// uncle_block is a fork child of genesis at h:1 (same height as
/// the confirmed share1). share2 declares uncle_block as an uncle.
/// Only the uncle's header exists, not its block data.
/// get_candidate_blocks_missing_data should return uncle_block.
#[test]
fn test_missing_data_includes_uncle_at_confirmed_height() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695790).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // share1: confirmed at h:1
    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695791)
        .build();
    store.push_to_confirmed_chain(&share1).unwrap();

    // uncle_block: fork child of genesis at h:1, only header stored
    let uncle_block = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695799)
        .build();
    let mut batch = Store::get_write_batch();
    store
        .add_share_header(&uncle_block.header, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    // share2: candidate at h:2, references uncle_block
    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .uncles(vec![uncle_block.block_hash()])
        .nonce(0xe9695792)
        .build();
    store.push_to_candidate_chain(&share2).unwrap();

    let missing = store.get_candidate_blocks_missing_data(None).unwrap();

    // share2 is missing block data (candidate above confirmed),
    // uncle_block is missing block data (referenced by share2's header)
    assert!(
        missing.contains(&share2.block_hash()),
        "share2 should be in missing list"
    );
    assert!(
        missing.contains(&uncle_block.block_hash()),
        "uncle_block at confirmed height should be in missing list"
    );
}

/// When fork_height is below the confirmed height, blocks at
/// that height should be included in the missing data result.
///
/// Scenario: genesis -> share1(h:1, confirmed) -> share2(h:2, candidate).
/// fork_block is a competing block at h:1 with Candidate status.
/// Without fork_height, the scan starts at h:2 and misses
/// fork_block. With fork_height=1, fork_block is found.
#[test]
fn test_missing_data_scans_from_min_height_when_below_confirmed() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695790).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // share1: confirmed at h:1
    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695791)
        .build();
    store.push_to_confirmed_chain(&share1).unwrap();

    // fork_block: competing block at h:1, only header stored with Candidate status
    let fork_block = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695799)
        .build();
    store.create_valid_metadata_only(&fork_block);

    // share2: candidate at h:2, extends share1
    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    store.push_to_candidate_chain(&share2).unwrap();

    // Without fork_height: fork_block at h:1 is skipped
    let missing = store.get_candidate_blocks_missing_data(None).unwrap();
    assert!(
        !missing.contains(&fork_block.block_hash()),
        "fork_block should NOT be returned without fork_height"
    );
    assert!(missing.contains(&share2.block_hash()));

    // With fork_height=1: fork_block at h:1 is found
    let missing = store.get_candidate_blocks_missing_data(Some(1)).unwrap();
    assert!(
        missing.contains(&fork_block.block_hash()),
        "fork_block should be returned with fork_height=1"
    );
    assert!(missing.contains(&share2.block_hash()));
}

#[test]
fn test_pick_best_child_skips_invalid_among_valid() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // valid_child: stored with Status::Valid
    let valid_child = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .work(1)
        .nonce(0xe9695792)
        .build();
    store.store_with_valid_metadata(&valid_child);

    // invalid_child: stored HeaderValid initially, then marked Invalid
    let invalid_child = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .work(3)
        .nonce(0xe9695793)
        .build();
    store.store_with_valid_metadata(&invalid_child);

    // Override status to Invalid (more work but ineligible for the chain)
    let mut metadata = store
        .get_block_metadata(&invalid_child.block_hash())
        .unwrap();
    metadata.status = Status::Invalid;
    let mut batch = Store::get_write_batch();
    store
        .update_block_metadata(&invalid_child.block_hash(), &metadata, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    // invalid_child has more work but is Invalid -- should be skipped
    let result = store
        .pick_best_child(
            &[invalid_child.block_hash(), valid_child.block_hash()],
            &genesis.block_hash(),
            1,
            None,
        )
        .unwrap();
    assert_eq!(result.unwrap().0, valid_child.block_hash());
}

/// A candidate that has passed chain-context validation is BlockValid
/// while still on the candidate chain. pick_best_child must keep picking
/// it so extend_candidates_with_children can rebuild the chain through an
/// already-validated block (e.g. after a reorg).
#[test]
fn test_pick_best_child_selects_block_valid_child() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // header_valid_child: lower work, plain HeaderValid
    let header_valid_child = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .work(1)
        .nonce(0xe9695792)
        .build();
    store.store_with_valid_metadata(&header_valid_child);

    // block_valid_child: higher work, upgraded to BlockValid
    let block_valid_child = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .work(3)
        .nonce(0xe9695793)
        .build();
    store.store_with_valid_metadata(&block_valid_child);
    let mut metadata = store
        .get_block_metadata(&block_valid_child.block_hash())
        .unwrap();
    metadata.status = Status::BlockValid;
    let mut batch = Store::get_write_batch();
    store
        .update_block_metadata(&block_valid_child.block_hash(), &metadata, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    // block_valid_child has more work and is eligible -- it must win.
    let result = store
        .pick_best_child(
            &[
                block_valid_child.block_hash(),
                header_valid_child.block_hash(),
            ],
            &genesis.block_hash(),
            1,
            None,
        )
        .unwrap();
    assert_eq!(result.unwrap().0, block_valid_child.block_hash());
}

#[test]
fn test_missing_data_scan_start_clamps_to_prune_floor() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // confirmed tip at 0 (genesis), candidate tip above PRUNE_DEPTH
    // so prune_floor is meaningful.
    // next_confirmed_height = 0 + 1 = 1
    // prune_floor = candidate_tip - PRUNE_DEPTH = 1000
    let candidate_tip = PRUNE_DEPTH as u32 + 1000;
    let prune_floor = candidate_tip - PRUNE_DEPTH as u32;
    let mut batch = Store::get_write_batch();
    store
        .increment_top_candidate(candidate_tip, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    // No fork_height: min_fork_and_confirmed_height = 1.
    // Result = max(1, prune_floor=1000) = 1000.
    let result = store.missing_data_scan_start(None).unwrap();
    assert_eq!(result, prune_floor);

    // fork_height below prune_floor: min(500, 1) = 1.
    // Result = max(1, 1000) = 1000.
    let result = store.missing_data_scan_start(Some(500)).unwrap();
    assert_eq!(result, prune_floor);

    // fork_height above prune_floor: min(2000, 1) = 1.
    // Result = max(1, 1000) = 1000.
    let result = store.missing_data_scan_start(Some(2000)).unwrap();
    assert_eq!(result, prune_floor);
}

#[test]
fn test_missing_data_scan_start_no_clamp_on_short_chain() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // candidate tip below PRUNE_DEPTH: prune_floor = 0.
    // next_confirmed_height = 0 + 1 = 1.
    // Result = max(1, 0) = 1.
    let mut batch = Store::get_write_batch();
    store.increment_top_candidate(100, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let result = store.missing_data_scan_start(None).unwrap();
    assert_eq!(result, 1);
}

// -- invalidation reorg tests -----------------------------------------

/// Invalidating a mid-candidate block removes it and its descendants from
/// the candidate chain and rebuilds the best surviving branch from the
/// invalid block's parent.
#[test]
fn test_mark_invalid_reorgs_mid_candidate() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Candidate chain: genesis -> c1 -> c2 -> c3.
    let c1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .work(1)
        .build();
    store.push_to_candidate_chain(&c1).unwrap();
    let c2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(c1.block_hash().to_string())
        .nonce(0xe9695793)
        .work(3)
        .build();
    store.push_to_candidate_chain(&c2).unwrap();
    let c3 = TestShareBlockBuilder::new()
        .prev_share_blockhash(c2.block_hash().to_string())
        .nonce(0xe9695794)
        .work(1)
        .build();
    store.push_to_candidate_chain(&c3).unwrap();

    // c2b: a lower-work sibling of c2 (also a child of c1), off-chain.
    let c2b = TestShareBlockBuilder::new()
        .prev_share_blockhash(c1.block_hash().to_string())
        .nonce(0xe9695795)
        .work(2)
        .build();
    store.store_with_valid_metadata(&c2b);

    assert_eq!(store.get_top_candidate_height().unwrap(), 3);

    // Invalidate c2 (mid-candidate).
    let mut batch = Store::get_write_batch();
    store
        .mark_invalid(&c2.block_hash(), None, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    // c2 is Invalid and off the chain; c3 left the chain too.
    let c2_meta = store.get_block_metadata(&c2.block_hash()).unwrap();
    assert_eq!(c2_meta.status, Status::Invalid);
    assert_eq!(c2_meta.chain, ChainMembership::None);
    assert_eq!(
        store.get_block_metadata(&c3.block_hash()).unwrap().chain,
        ChainMembership::None
    );

    // The candidate chain was rebuilt onto the surviving sibling c2b.
    assert_eq!(store.get_top_candidate_height().unwrap(), 2);
    assert_eq!(store.get_candidate_at_height(1).unwrap(), c1.block_hash());
    assert_eq!(store.get_candidate_at_height(2).unwrap(), c2b.block_hash());
    assert!(store.get_candidate_at_height(3).is_err());
    assert_eq!(
        store.get_block_metadata(&c2b.block_hash()).unwrap().chain,
        ChainMembership::Candidate
    );
}

/// Invalidating the only candidate above the confirmed tip, with no
/// surviving sibling, leaves the candidate top at the confirmed tip rather
/// than clearing it.
#[test]
fn test_mark_invalid_drops_candidate_top_to_confirmed_tip_when_no_sibling() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let c1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .work(1)
        .build();
    store.push_to_candidate_chain(&c1).unwrap();
    assert_eq!(store.get_top_candidate_height().unwrap(), 1);

    let mut batch = Store::get_write_batch();
    store
        .mark_invalid(&c1.block_hash(), None, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let c1_meta = store.get_block_metadata(&c1.block_hash()).unwrap();
    assert_eq!(c1_meta.status, Status::Invalid);
    assert_eq!(c1_meta.chain, ChainMembership::None);
    // No candidate remains above the confirmed tip, but the top candidate
    // height still resolves: an absent key is the pre-genesis bootstrap
    // state, in which organise_block stops promoting and
    // should_extend_candidates accepts any header as the new tip.
    assert_eq!(store.get_top_candidate_height().unwrap(), 0);
    assert!(store.get_candidate_at_height(1).is_err());
}

// -- invalidation reorg helper unit tests -----------------------------

/// detach_candidate_branch removes the branch from `from_height` up,
/// and clears chain membership for every removed block except the invalid
/// one, whose metadata the caller owns.
#[test]
fn test_detach_candidate_branch() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let c1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    store.push_to_candidate_chain(&c1).unwrap();
    let c2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(c1.block_hash().to_string())
        .nonce(0xe9695793)
        .build();
    store.push_to_candidate_chain(&c2).unwrap();
    let c3 = TestShareBlockBuilder::new()
        .prev_share_blockhash(c2.block_hash().to_string())
        .nonce(0xe9695794)
        .build();
    store.push_to_candidate_chain(&c3).unwrap();

    // Detach from height 2 treating c2 as the invalid block.
    let mut batch = Store::get_write_batch();
    store
        .detach_candidate_branch(&c2.block_hash(), 2, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    // Index entries for the detached heights are gone; c1 stays.
    assert!(store.get_candidate_at_height(2).is_err());
    assert!(store.get_candidate_at_height(3).is_err());
    assert_eq!(store.get_candidate_at_height(1).unwrap(), c1.block_hash());
    // c3 (not the invalid block) left the candidate chain.
    assert_eq!(
        store.get_block_metadata(&c3.block_hash()).unwrap().chain,
        ChainMembership::None
    );
    // c2's metadata is untouched by detach (the caller owns it).
    assert_eq!(
        store.get_block_metadata(&c2.block_hash()).unwrap().chain,
        ChainMembership::Candidate
    );
}

/// rebuild_candidate_from_parent picks the best surviving child of the
/// branch point (excluding the invalid block) and returns the new top.
#[test]
fn test_rebuild_candidate_from_parent() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let c1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    store.push_to_candidate_chain(&c1).unwrap();

    // Two children of c1: the invalid one and a surviving sibling.
    let invalid = TestShareBlockBuilder::new()
        .prev_share_blockhash(c1.block_hash().to_string())
        .nonce(0xe9695793)
        .work(1)
        .build();
    store.store_with_valid_metadata(&invalid);
    let surviving = TestShareBlockBuilder::new()
        .prev_share_blockhash(c1.block_hash().to_string())
        .nonce(0xe9695794)
        .work(2)
        .build();
    store.store_with_valid_metadata(&surviving);

    let mut batch = Store::get_write_batch();
    let final_top = store
        .rebuild_candidate_from_parent(&invalid.block_hash(), 2, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    assert_eq!(final_top, 2);
    assert_eq!(
        store.get_candidate_at_height(2).unwrap(),
        surviving.block_hash()
    );
    assert_eq!(
        store
            .get_block_metadata(&surviving.block_hash())
            .unwrap()
            .chain,
        ChainMembership::Candidate
    );
}

/// `mark_invalid` runs after `confirm_blocks` has queued a *lower* confirmed
/// top into the same batch, which is what a reorg to a shorter validated
/// prefix does. Reading the confirmed top from the store here would see the
/// pre-batch value, decide the rebuilt candidate top no longer reaches
/// above it, and erase TOP_CANDIDATE_KEY while candidate entries exist --
/// leaving `get_top_candidate_height` permanently NotFound.
#[test]
fn test_mark_invalid_keeps_candidate_top_against_pending_confirmed_top() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Candidate chain c1 -> c2 -> c3, with a surviving sibling of c2 so the
    // rebuild produces a candidate top above the invalidated height.
    let c1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695802)
        .work(1)
        .build();
    store.push_to_candidate_chain(&c1).unwrap();
    let c2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(c1.block_hash().to_string())
        .nonce(0xe9695803)
        .work(3)
        .build();
    store.push_to_candidate_chain(&c2).unwrap();
    let c3 = TestShareBlockBuilder::new()
        .prev_share_blockhash(c2.block_hash().to_string())
        .nonce(0xe9695804)
        .work(1)
        .build();
    store.push_to_candidate_chain(&c3).unwrap();

    let c2b = TestShareBlockBuilder::new()
        .prev_share_blockhash(c1.block_hash().to_string())
        .nonce(0xe9695805)
        .work(2)
        .build();
    store.store_with_valid_metadata(&c2b);

    // Committed confirmed top is deliberately high: 5, above the candidate
    // heights, standing in for the branch a reorg is rewinding away.
    let mut batch = Store::get_write_batch();
    store.set_top_confirmed_height(5, &mut batch);
    store.commit_batch(batch).unwrap();

    // The batch has queued confirmed top 1, as confirm_blocks would when
    // the validated prefix ends at c1.
    let mut batch = Store::get_write_batch();
    store
        .mark_invalid(&c2.block_hash(), Some(1), &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    // The rebuild put c2b at height 2, which is above the batch's confirmed
    // top of 1, so the candidate top must survive.
    assert_eq!(store.get_candidate_at_height(2).unwrap(), c2b.block_hash());
    assert_eq!(
        store.get_top_candidate_height().unwrap(),
        2,
        "candidate top erased against the pre-batch confirmed top"
    );
}

/// When the rebuild finds no surviving child, the candidate top falls back
/// to the invalidated block's parent. That parent is itself a candidate
/// here, so candidate entries still exist and the top must be set to it,
/// not cleared.
#[test]
fn test_mark_invalid_sets_candidate_top_to_parent_when_no_sibling_survives() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let c1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695812)
        .work(1)
        .build();
    store.push_to_candidate_chain(&c1).unwrap();
    let c2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(c1.block_hash().to_string())
        .nonce(0xe9695813)
        .work(1)
        .build();
    store.push_to_candidate_chain(&c2).unwrap();

    let mut batch = Store::get_write_batch();
    store
        .mark_invalid(&c2.block_hash(), None, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    assert_eq!(store.get_candidate_at_height(1).unwrap(), c1.block_hash());
    assert_eq!(store.get_top_candidate_height().unwrap(), 1);
}

/// The rebuilt candidate top is recorded above the confirmed tip, and falls
/// back to the confirmed tip -- never to absent -- when no branch survives.
#[test]
fn test_set_top_candidate_height_after_rebuild() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Confirmed tip is genesis (height 0). A top above it is set.
    let mut batch = Store::get_write_batch();
    store.set_top_candidate_height_after_rebuild(5, 0, &mut batch);
    store.commit_batch(batch).unwrap();
    assert_eq!(store.get_top_candidate_height().unwrap(), 5);

    // Nothing survives above the confirmed tip: the top becomes the
    // confirmed tip. Deleting the key here would put the store back into
    // the pre-genesis bootstrap state, where organise_block stops
    // promoting and any header becomes the candidate tip unchecked.
    let mut batch = Store::get_write_batch();
    store.set_top_candidate_height_after_rebuild(0, 0, &mut batch);
    store.commit_batch(batch).unwrap();
    assert_eq!(store.get_top_candidate_height().unwrap(), 0);
}

/// With share transactions a reorg must not fork more than
/// `MAX_REORG_DEPTH` (`PAYOUT_DEPTH`) below the confirmed tip, far above
/// the prune target: a block on such a branch may spend an output this
/// node stamped with an unknown root or never fetched.
#[test]
fn test_reorg_branch_forking_more_than_payout_depth_below_confirmed_tip_is_refused() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();
    let share_1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    store.push_to_candidate_chain(&share_1).unwrap();
    let share_2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_1.block_hash().to_string())
        .nonce(0xe9695793)
        .build();
    store.push_to_candidate_chain(&share_2).unwrap();
    let fork = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_2.block_hash().to_string())
        .nonce(0xe9695799)
        .build();
    store.store_with_valid_metadata(&fork);
    // The branch point, share_2, is at height 2.
    let mut batch = Store::get_write_batch();
    store.set_top_confirmed_height(2 + MAX_REORG_DEPTH as u32 + 1, &mut batch);
    store.commit_batch(batch).unwrap();

    assert!(
        store
            .reorg_branch_forks_below_reorg_floor(&fork.block_hash())
            .unwrap()
    );
}

/// A branch forking exactly `MAX_REORG_DEPTH` below the confirmed tip is
/// followed: every output its blocks may spend is above the prune height.
#[test]
fn test_reorg_branch_forking_exactly_payout_depth_below_confirmed_tip_is_followed() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();
    let share_1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    store.push_to_candidate_chain(&share_1).unwrap();
    let share_2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_1.block_hash().to_string())
        .nonce(0xe9695793)
        .build();
    store.push_to_candidate_chain(&share_2).unwrap();
    let fork = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_2.block_hash().to_string())
        .nonce(0xe9695799)
        .build();
    store.store_with_valid_metadata(&fork);
    let mut batch = Store::get_write_batch();
    store.set_top_confirmed_height(2 + MAX_REORG_DEPTH as u32, &mut batch);
    store.commit_batch(batch).unwrap();

    assert!(
        !store
            .reorg_branch_forks_below_reorg_floor(&fork.block_hash())
            .unwrap()
    );
}

/// A refused deep fork is remembered, and a child of it is refused by
/// lookup: the child is not even in the store, so its branch cannot be
/// walked. A block on the chain is not refused.
#[test]
fn test_reorg_refused_before_refuses_a_refused_fork_and_its_children() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();
    let share_1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    store.push_to_candidate_chain(&share_1).unwrap();
    let share_2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_1.block_hash().to_string())
        .nonce(0xe9695793)
        .build();
    store.push_to_candidate_chain(&share_2).unwrap();
    let fork = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_2.block_hash().to_string())
        .nonce(0xe9695799)
        .build();
    store.store_with_valid_metadata(&fork);
    let fork_child = TestShareBlockBuilder::new()
        .prev_share_blockhash(fork.block_hash().to_string())
        .nonce(0xe969579a)
        .build();
    let mut batch = Store::get_write_batch();
    store.set_top_confirmed_height(2 + MAX_REORG_DEPTH as u32 + 1, &mut batch);
    store.commit_batch(batch).unwrap();

    assert!(
        !store
            .reorg_refused_before(&fork.block_hash(), &share_2.block_hash())
            .unwrap()
    );
    assert!(
        store
            .reorg_branch_forks_below_reorg_floor(&fork.block_hash())
            .unwrap()
    );

    assert!(
        store
            .reorg_refused_before(&fork.block_hash(), &share_2.block_hash())
            .unwrap()
    );
    assert!(
        store
            .reorg_refused_before(&fork_child.block_hash(), &fork.block_hash())
            .unwrap()
    );
    assert!(
        !store
            .reorg_refused_before(&share_2.block_hash(), &share_1.block_hash())
            .unwrap()
    );
}

/// A remembered refusal is not used once the confirmed tip falls back and
/// the floor no longer refuses the fork: every refusal is forgotten and the
/// branch is walked again, which now follows it.
#[test]
fn test_reorg_refused_before_forgets_refusals_once_the_floor_falls() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();
    let share_1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    store.push_to_candidate_chain(&share_1).unwrap();
    let share_2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_1.block_hash().to_string())
        .nonce(0xe9695793)
        .build();
    store.push_to_candidate_chain(&share_2).unwrap();
    let fork = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_2.block_hash().to_string())
        .nonce(0xe9695799)
        .build();
    store.store_with_valid_metadata(&fork);
    let mut batch = Store::get_write_batch();
    store.set_top_confirmed_height(2 + MAX_REORG_DEPTH as u32 + 1, &mut batch);
    store.commit_batch(batch).unwrap();
    assert!(
        store
            .reorg_branch_forks_below_reorg_floor(&fork.block_hash())
            .unwrap()
    );

    let mut batch = Store::get_write_batch();
    store.set_top_confirmed_height(2 + MAX_REORG_DEPTH as u32, &mut batch);
    store.commit_batch(batch).unwrap();

    assert!(
        !store
            .reorg_refused_before(&fork.block_hash(), &share_2.block_hash())
            .unwrap()
    );
    assert!(store.refused_reorgs.lock().unwrap().is_empty());
    assert!(
        !store
            .reorg_branch_forks_below_reorg_floor(&fork.block_hash())
            .unwrap()
    );
}

/// A branch forking `MAX_UNCLES_DEPTH` above the prune target was followed
/// before share transactions. It is refused now: it forks a whole
/// `PAYOUT_DEPTH` deeper than the floor.
#[test]
fn test_reorg_branch_forking_just_above_the_prune_target_is_refused() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();
    let share_1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    store.push_to_candidate_chain(&share_1).unwrap();
    let share_2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_1.block_hash().to_string())
        .nonce(0xe9695793)
        .build();
    store.push_to_candidate_chain(&share_2).unwrap();
    let share_3 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_2.block_hash().to_string())
        .nonce(0xe9695794)
        .build();
    store.push_to_candidate_chain(&share_3).unwrap();
    let share_4 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_3.block_hash().to_string())
        .nonce(0xe9695795)
        .build();
    store.push_to_candidate_chain(&share_4).unwrap();
    let fork = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_4.block_hash().to_string())
        .nonce(0xe9695799)
        .build();
    store.store_with_valid_metadata(&fork);
    // Prune target 1, so the branch point at 4 is MAX_UNCLES_DEPTH above it.
    let mut batch = Store::get_write_batch();
    store.set_top_confirmed_height((PRUNE_DEPTH + PRUNE_INTERVAL) as u32 + 1, &mut batch);
    store.commit_batch(batch).unwrap();

    assert!(
        store
            .reorg_branch_forks_below_reorg_floor(&fork.block_hash())
            .unwrap()
    );
}

/// The floor follows the confirmed tip, not the candidate tip: a candidate
/// chain running far ahead, as during a sync, refuses nothing the
/// confirmed tip allows.
#[test]
fn test_reorg_floor_follows_the_confirmed_tip_when_the_candidate_runs_ahead() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();
    let share_1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    store.push_to_candidate_chain(&share_1).unwrap();
    let share_2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_1.block_hash().to_string())
        .nonce(0xe9695793)
        .build();
    store.push_to_candidate_chain(&share_2).unwrap();
    let fork = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_2.block_hash().to_string())
        .nonce(0xe9695799)
        .build();
    store.store_with_valid_metadata(&fork);
    let mut batch = Store::get_write_batch();
    store.set_top_confirmed_height(2 + MAX_REORG_DEPTH as u32, &mut batch);
    store.set_top_candidate_height(2 + MAX_REORG_DEPTH as u32 + 1_000, &mut batch);
    store.commit_batch(batch).unwrap();

    assert!(
        !store
            .reorg_branch_forks_below_reorg_floor(&fork.block_hash())
            .unwrap()
    );
}

/// With a confirmed tip within `MAX_REORG_DEPTH` of genesis, no reorg is
/// refused.
#[test]
fn test_reorg_branch_with_confirmed_tip_below_the_reorg_depth_is_followed() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();
    let share_1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    store.push_to_candidate_chain(&share_1).unwrap();
    let fork = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695799)
        .build();
    store.store_with_valid_metadata(&fork);
    let mut batch = Store::get_write_batch();
    store.set_top_confirmed_height(MAX_REORG_DEPTH as u32, &mut batch);
    store.commit_batch(batch).unwrap();

    assert!(
        !store
            .reorg_branch_forks_below_reorg_floor(&fork.block_hash())
            .unwrap()
    );
}

#[test]
fn test_reorg_branch_has_invalid_false_when_all_valid() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Candidate branch point at h1.
    let share_p = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    store.push_to_candidate_chain(&share_p).unwrap();

    // share_a (h2) -> share_b (h3): HeaderValid, detached (chain = None).
    let share_a = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_p.block_hash().to_string())
        .nonce(0xe9695793)
        .build();
    store.store_with_valid_metadata(&share_a);
    let share_b = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_a.block_hash().to_string())
        .nonce(0xe9695794)
        .build();
    store.store_with_valid_metadata(&share_b);

    assert!(
        !store
            .reorg_branch_has_invalid(&share_b.block_hash())
            .unwrap()
    );
}

/// Returns true when an Invalid block lies between the target and the branch
/// point (the case that would otherwise re-admit it via write_branch_as_candidates).
#[test]
fn test_reorg_branch_has_invalid_true_for_invalid_ancestor() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let share_p = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    store.push_to_candidate_chain(&share_p).unwrap();

    // share_a is Invalid and detached; share_b (HeaderValid) descends from it.
    let share_a = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_p.block_hash().to_string())
        .nonce(0xe9695793)
        .build();
    store.store_with_valid_metadata(&share_a);
    let mut a_metadata = store.get_block_metadata(&share_a.block_hash()).unwrap();
    a_metadata.status = Status::Invalid;
    let mut batch = Store::get_write_batch();
    store
        .update_block_metadata(&share_a.block_hash(), &a_metadata, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let share_b = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_a.block_hash().to_string())
        .nonce(0xe9695794)
        .build();
    store.store_with_valid_metadata(&share_b);

    assert!(
        store
            .reorg_branch_has_invalid(&share_b.block_hash())
            .unwrap()
    );
}

/// Returns true when the target block itself is Invalid.
#[test]
fn test_reorg_branch_has_invalid_true_when_target_invalid() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let share_p = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    store.push_to_candidate_chain(&share_p).unwrap();

    let share_a = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_p.block_hash().to_string())
        .nonce(0xe9695793)
        .build();
    store.store_with_valid_metadata(&share_a);
    let mut a_metadata = store.get_block_metadata(&share_a.block_hash()).unwrap();
    a_metadata.status = Status::Invalid;
    let mut batch = Store::get_write_batch();
    store
        .update_block_metadata(&share_a.block_hash(), &a_metadata, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    assert!(
        store
            .reorg_branch_has_invalid(&share_a.block_hash())
            .unwrap()
    );
}

/// Returns false when the branch does not reach the candidate/confirmed
/// chain (get_branch_to_chain yields None because a parent header is missing).
#[test]
fn test_reorg_branch_has_invalid_false_when_branch_unresolved() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Orphan whose parent header is not stored, so the walk cannot reach the
    // candidate/confirmed chain.
    let orphan = TestShareBlockBuilder::new().nonce(0xe9695795).build();
    let mut batch = Store::get_write_batch();
    store.add_share_header(&orphan.header, &mut batch).unwrap();
    store
        .update_block_metadata(
            &orphan.block_hash(),
            &BlockMetadata {
                expected_height: Some(1),
                chain_work: orphan.header.get_work(),
                status: Status::HeaderValid,
                chain: ChainMembership::None,
            },
            &mut batch,
        )
        .unwrap();
    store.commit_batch(batch).unwrap();

    assert!(
        !store
            .reorg_branch_has_invalid(&orphan.block_hash())
            .unwrap()
    );
}

#[test]
fn test_needing_validation_returns_stranded_run_ascending() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695790).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695791)
        .build();
    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(0xe9695792)
        .build();

    // Both are HeaderValid candidates with their bodies stored: stranded.
    store.push_to_candidate_chain(&share1).unwrap();
    store.push_to_candidate_chain(&share2).unwrap();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&share1, &mut batch).unwrap();
    store.add_share_block(&share2, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let needing = store
        .get_candidate_blocks_needing_validation(1, 512)
        .unwrap();
    assert_eq!(needing, vec![share1.block_hash(), share2.block_hash()]);
}

#[test]
fn test_needing_validation_excludes_block_valid() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695790).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695791)
        .build();
    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(0xe9695792)
        .build();

    store.push_to_candidate_chain(&share1).unwrap();
    store.push_to_candidate_chain(&share2).unwrap();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&share1, &mut batch).unwrap();
    store.add_share_block(&share2, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // share1 is already validated: it is not stranded.
    let mut metadata = store.get_block_metadata(&share1.block_hash()).unwrap();
    metadata.status = Status::BlockValid;
    let mut batch = Store::get_write_batch();
    store
        .update_block_metadata(&share1.block_hash(), &metadata, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let needing = store
        .get_candidate_blocks_needing_validation(1, 512)
        .unwrap();
    assert_eq!(needing, vec![share2.block_hash()]);
}

#[test]
fn test_needing_validation_excludes_body_missing() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695790).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695791)
        .build();
    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(0xe9695792)
        .build();

    store.push_to_candidate_chain(&share1).unwrap();
    store.push_to_candidate_chain(&share2).unwrap();
    // Only share1's body is stored; share2 is still waiting on the fetcher.
    let mut batch = Store::get_write_batch();
    store.add_share_block(&share1, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let needing = store
        .get_candidate_blocks_needing_validation(1, 512)
        .unwrap();
    assert_eq!(needing, vec![share1.block_hash()]);
}

#[test]
fn test_needing_validation_excludes_non_candidate_membership() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695790).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695791)
        .build();
    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(0xe9695792)
        .build();

    store.push_to_candidate_chain(&share1).unwrap();
    store.push_to_candidate_chain(&share2).unwrap();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&share1, &mut batch).unwrap();
    store.add_share_block(&share2, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // share1 is HeaderValid with its body but off the candidate chain
    // (e.g. confirmed below the prune zone); it must not be re-driven.
    let mut metadata = store.get_block_metadata(&share1.block_hash()).unwrap();
    metadata.chain = ChainMembership::Confirmed;
    let mut batch = Store::get_write_batch();
    store
        .update_block_metadata(&share1.block_hash(), &metadata, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let needing = store
        .get_candidate_blocks_needing_validation(1, 512)
        .unwrap();
    assert_eq!(needing, vec![share2.block_hash()]);
}

#[test]
fn test_needing_validation_respects_limit() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695790).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695791)
        .build();
    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(0xe9695792)
        .build();

    store.push_to_candidate_chain(&share1).unwrap();
    store.push_to_candidate_chain(&share2).unwrap();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&share1, &mut batch).unwrap();
    store.add_share_block(&share2, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let needing = store.get_candidate_blocks_needing_validation(1, 1).unwrap();
    assert_eq!(needing, vec![share1.block_hash()]);
}

#[test]
fn test_needing_validation_stitches_across_batches() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695790).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695791)
        .build();
    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    let share3 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share2.block_hash().to_string())
        .nonce(0xe9695793)
        .build();

    store.push_to_candidate_chain(&share1).unwrap();
    store.push_to_candidate_chain(&share2).unwrap();
    store.push_to_candidate_chain(&share3).unwrap();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&share1, &mut batch).unwrap();
    store.add_share_block(&share2, &mut batch).unwrap();
    store.add_share_block(&share3, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // One height per batch forces the scan across three batches; the result
    // must stitch them in height order and honour the limit across batches.
    let all = store
        .scan_candidate_blocks_needing_validation(1, 512, 1)
        .unwrap();
    assert_eq!(
        all,
        vec![
            share1.block_hash(),
            share2.block_hash(),
            share3.block_hash()
        ]
    );

    let limited = store
        .scan_candidate_blocks_needing_validation(1, 2, 1)
        .unwrap();
    assert_eq!(limited, vec![share1.block_hash(), share2.block_hash()]);
}
