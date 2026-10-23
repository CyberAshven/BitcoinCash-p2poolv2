use super::*;
use crate::test_utils::TestShareBlockBuilder;
use tempfile::tempdir;

// ── get_branch_to_chain tests ─────────────────────────────────────────

#[test]
fn test_get_branch_to_chain_returns_single_entry_when_already_candidate() {
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

    // Branch should contain just the share since it is already a candidate
    let branch = store
        .get_branch_to_chain(&share.block_hash(), |h| store.is_candidate(h))
        .unwrap();
    assert!(branch.is_some());
    assert_eq!(branch, Some(VecDeque::from([share.block_hash()])));
}

#[test]
fn test_get_branch_to_chain_returns_branch_to_candidate_ancestor() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // Setup: genesis -> share1 (candidate) -> share2 -> share3
    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // share1 is a candidate at height 1
    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    store.push_to_candidate_chain(&share1).unwrap();

    // share2 extends share1 but is NOT on candidate chain
    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(0xe9695793)
        .build();
    store.store_with_valid_metadata(&share2);

    // share3 extends share2 and is NOT on candidate chain
    let share3 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share2.block_hash().to_string())
        .nonce(0xe9695794)
        .build();
    store.store_with_valid_metadata(&share3);

    // Branch from share3 should be [share1, share2, share3]
    let branch = store
        .get_branch_to_chain(&share3.block_hash(), |h| store.is_candidate(h))
        .unwrap()
        .unwrap();
    assert_eq!(branch.len(), 3);
    assert_eq!(branch[0], share1.block_hash());
    assert_eq!(branch[1], share2.block_hash());
    assert_eq!(branch[2], share3.block_hash());
}

#[test]
fn test_get_branch_to_chain_returns_single_share_branch() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // Setup: genesis -> share1 (candidate) -> share2 (not on candidate chain)
    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    store.push_to_candidate_chain(&share1).unwrap();

    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(0xe9695793)
        .build();
    store.store_with_valid_metadata(&share2);

    // Branch from share2 should be just [share1, share2]
    let branch = store
        .get_branch_to_chain(&share2.block_hash(), |h| store.is_candidate(h))
        .unwrap()
        .unwrap();
    assert_eq!(branch.len(), 2);
    assert_eq!(branch[0], share1.block_hash());
    assert_eq!(branch[1], share2.block_hash());
}
