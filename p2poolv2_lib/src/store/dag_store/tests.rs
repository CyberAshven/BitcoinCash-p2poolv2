use super::*;
use crate::test_utils::TestShareBlockBuilder;
use crate::test_utils::make_test_share_program;
use crate::test_utils::parse_address_from_string;
use bitcoin::hashes::Hash;
use p2poolv2_wallet::witness_program_codec::to_hex;
use tempfile::tempdir;

#[test]
fn test_get_children() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // Create initial share (genesis)
    let share1 = TestShareBlockBuilder::new().build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&share1, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Create uncles for share2
    let uncle1_share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(100)
        .build();

    let uncle2_share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(200)
        .build();

    // Create share2 with uncles
    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .uncles(vec![uncle1_share2.block_hash(), uncle2_share2.block_hash()])
        .build();

    // Create share3
    let share3 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share2.block_hash().to_string())
        .build();

    // Add all shares to store with valid metadata
    store.store_with_valid_metadata(&uncle1_share2);
    store.store_with_valid_metadata(&uncle2_share2);
    store.store_with_valid_metadata(&share2);
    // Uncle block index updates are handled by organise_header, not
    // store_with_valid_metadata. Manually register uncle->nephew entries here
    // since this test exercises the block index directly.
    let mut batch = Store::get_write_batch();
    for uncle_blockhash in &share2.header.uncles {
        store
            .update_block_index(uncle_blockhash, &share2.block_hash(), &mut batch)
            .unwrap();
    }
    store.commit_batch(batch).unwrap();

    store.store_with_valid_metadata(&share3);

    // Verify children of share1
    let children_share1 = store
        .get_children_blockhashes(&share1.block_hash())
        .unwrap()
        .unwrap();
    assert_eq!(children_share1.len(), 3);
    assert!(children_share1.contains(&share2.block_hash()));
    assert!(children_share1.contains(&uncle1_share2.block_hash()));
    assert!(children_share1.contains(&uncle2_share2.block_hash()));

    // Verify children of share2
    let children_share2 = store
        .get_children_blockhashes(&share2.block_hash())
        .unwrap()
        .unwrap();
    assert_eq!(children_share2.len(), 1);
    assert!(children_share2.contains(&share3.block_hash()));

    // Verify children of share3
    let children_share3 = store
        .get_children_blockhashes(&share3.block_hash())
        .unwrap();
    assert!(children_share3.is_none());

    // Verify children of uncle1_share2
    let children_uncle1_share2 = store
        .get_children_blockhashes(&uncle1_share2.block_hash())
        .unwrap()
        .unwrap();
    assert!(children_uncle1_share2.contains(&share2.block_hash()));

    // Verify children of uncle2_share2
    let children_uncle2_share2 = store
        .get_children_blockhashes(&uncle2_share2.block_hash())
        .unwrap()
        .unwrap();
    assert!(children_uncle2_share2.contains(&share2.block_hash()));
}

#[test]
fn test_get_descendants() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // Create initial share (genesis)
    let share1 = TestShareBlockBuilder::new().build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&share1, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Create uncles for share2
    let uncle1_share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(100)
        .build();

    let uncle2_share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(200)
        .build();

    // Create share2 with uncles
    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .uncles(vec![uncle1_share2.block_hash(), uncle2_share2.block_hash()])
        .build();

    // Create share3
    let share3 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share2.block_hash().to_string())
        .build();

    // Add all shares to store with valid metadata
    store.store_with_valid_metadata(&uncle1_share2);
    store.store_with_valid_metadata(&uncle2_share2);
    store.store_with_valid_metadata(&share2);
    store.store_with_valid_metadata(&share3);

    // Verify descendants of share1
    let descendants_share1 = store
        .get_descendants(share1.block_hash(), &share3.block_hash(), 10)
        .unwrap();
    assert_eq!(descendants_share1.len(), 4);
    assert!(descendants_share1.contains(&share2.header));
    assert!(descendants_share1.contains(&share3.header));
    assert!(descendants_share1.contains(&uncle1_share2.header));
    assert!(descendants_share1.contains(&uncle2_share2.header));

    // Verify descendants of share2
    let descendants_share2 = store
        .get_descendants(share2.block_hash(), &share3.block_hash(), 10)
        .unwrap();
    assert_eq!(descendants_share2.len(), 1);
    assert_eq!(descendants_share2[0], share3.header);

    // Verify no descendants for share3
    let descendants_share3 = store
        .get_descendants(share3.block_hash(), &share3.block_hash(), 10)
        .unwrap();
    assert!(descendants_share3.is_empty());

    // Verify descendants with limit
    let descendants_with_limit = store
        .get_descendants(share1.block_hash(), &share3.block_hash(), 1)
        .unwrap();
    assert_eq!(descendants_with_limit.len(), 1);
    assert_eq!(descendants_with_limit[0], uncle1_share2.header);

    // Verify descendants with stop blockhash
    let descendants_with_limit = store
        .get_descendants(share1.block_hash(), &share2.block_hash(), 10)
        .unwrap();
    assert_eq!(descendants_with_limit.len(), 3);
    assert!(descendants_with_limit.contains(&share2.header));
    assert!(descendants_with_limit.contains(&uncle1_share2.header));
    assert!(descendants_with_limit.contains(&uncle2_share2.header));
}

#[test_log::test]
fn test_get_headers_for_block_locator_should_find_matching_blocks() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let num_blockhashes = 5;
    let mut hashes: Vec<BlockHash> = vec![];
    let mut blocks = Vec::with_capacity(num_blockhashes);
    let mut prev_blockhash = BlockHash::all_zeros();

    for nonce in 0..num_blockhashes {
        let block = TestShareBlockBuilder::new()
            .prev_share_blockhash(prev_blockhash.to_string())
            .nonce(nonce as u32)
            .build();
        prev_blockhash = block.block_hash();
        hashes.push(prev_blockhash);
        blocks.push(block);
    }

    // Setup genesis (height 0) and push remaining blocks to confirmed chain
    // so height-to-blockhash mapping is created by organise_header.
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&blocks[0], &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    for block in &blocks[1..] {
        store.push_to_confirmed_chain(block).unwrap();
    }

    let stop_block = store.get_blockhashes_for_height(2)[0];
    let locator = store.get_blockhashes_for_height(0);

    // Call handle_getblocks
    let result = store
        .get_headers_for_locator(locator.as_slice(), &stop_block, 10)
        .unwrap();

    assert_eq!(result.len(), 2);
    assert_eq!(result[0].block_hash(), hashes[1]);
    assert_eq!(result[1].block_hash(), hashes[2]);
}

#[test]
fn test_get_headers_for_block_locator_stop_block_not_found() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let num_blockhashes = 3;
    let mut blocks = Vec::with_capacity(num_blockhashes);
    let mut prev_blockhash = BlockHash::all_zeros();

    for nonce in 0..num_blockhashes {
        let block = TestShareBlockBuilder::new()
            .prev_share_blockhash(prev_blockhash.to_string())
            .nonce(nonce as u32)
            .build();
        prev_blockhash = block.block_hash();
        blocks.push(block);
    }

    // Setup genesis and push remaining to confirmed chain
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&blocks[0], &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    for block in &blocks[1..] {
        store.push_to_confirmed_chain(block).unwrap();
    }

    let locator = vec![blocks[0].block_hash()];

    // Use a stop block hash that doesn't exist in our chain
    let non_existent_stop_block =
        "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
            .parse::<BlockHash>()
            .unwrap();

    // Call get_headers_for_locator with non-existent stop block
    let result = store
        .get_headers_for_locator(&locator, &non_existent_stop_block, 10)
        .unwrap();
    assert_eq!(result.len(), 2);
    // start block not in response
    assert_eq!(result[0], blocks[1].header);
    assert_eq!(result[1], blocks[2].header);
}

#[test]
fn test_get_blockhashes_for_block_locator_should_find_matching_blocks() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let mut block_hashes = Vec::new();
    let mut blocks = Vec::new();

    let num_blockhashes = 5;

    let mut prev_blockhash = BlockHash::all_zeros();

    for nonce in 0..num_blockhashes {
        let block = TestShareBlockBuilder::new()
            .prev_share_blockhash(prev_blockhash.to_string())
            .nonce(nonce)
            .build();
        let blockhash = block.block_hash();
        block_hashes.push(blockhash);
        blocks.push(block);
        prev_blockhash = blockhash;
    }

    // Setup genesis (height 0) and push remaining blocks to confirmed chain
    // so height-to-blockhash mapping is created by organise_header.
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&blocks[0], &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    for block in &blocks[1..] {
        store.push_to_confirmed_chain(block).unwrap();
    }

    let stop_block = store.get_blockhashes_for_height(2)[0];

    let locator = store.get_blockhashes_for_height(0);

    // Call handle_getblocks
    let result = store
        .get_blockhashes_for_locator(locator.as_slice(), &stop_block, 10)
        .unwrap();

    assert_eq!(result.len(), 2);
    assert_eq!(result[0], block_hashes[1]);
    assert_eq!(result[1], block_hashes[2]);
}

/// Test that get_descendant_blockhashes gets all descendants from
/// height index and includes uncle blockhashes before the nephew
/// that references them.
///
/// Chain:
///   genesis(h:0) -> share_a(h:1) -> share_b(h:2, uncles=[uncle1])
///                \-> uncle1(h:1)
///
/// uncle1 is a child of genesis (same height as share_a) that lost
/// the candidate race. share_b at h:2 references uncle1 as an uncle.
#[test]
fn test_get_descendant_blockhashes_returns_confirmed_and_uncles() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // uncle1: child of genesis, same height as share_a, not confirmed
    let uncle1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(100)
        .build();
    store.store_with_valid_metadata(&uncle1);

    let share_a = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .work(2)
        .nonce(1)
        .build();
    store.push_to_confirmed_chain(&share_a).unwrap();

    let share_b = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_a.block_hash().to_string())
        .uncles(vec![uncle1.block_hash()])
        .nonce(2)
        .build();
    store.push_to_confirmed_chain(&share_b).unwrap();

    // Descendants from genesis: h:1 (share_a, uncle1 lex sorted), h:2 (share_b)
    let descendants = store
        .get_descendant_blockhashes(&genesis.block_hash(), &BlockHash::all_zeros(), 10)
        .unwrap();
    let mut height_1 = vec![share_a.block_hash(), uncle1.block_hash()];
    height_1.sort();
    let mut expected = height_1.clone();
    expected.push(share_b.block_hash());
    assert_eq!(descendants, expected);

    // Descendants from share_a (h:1): with MAX_UNCLES_DEPTH overlap,
    // starts at max(1-3,0)+1 = h:1. Includes h:1 (share_a, uncle1)
    // and h:2 (share_b).
    let descendants = store
        .get_descendant_blockhashes(&share_a.block_hash(), &BlockHash::all_zeros(), 10)
        .unwrap();
    assert_eq!(descendants, expected);

    // Test with limit: limit=2 completes h:1 (2 blocks) then stops
    let descendants = store
        .get_descendant_blockhashes(&genesis.block_hash(), &BlockHash::all_zeros(), 2)
        .unwrap();
    assert_eq!(descendants, height_1);

    // Test with stop_blockhash: stop hash at h:1, completes h:1
    let descendants = store
        .get_descendant_blockhashes(&genesis.block_hash(), &share_a.block_hash(), 10)
        .unwrap();
    assert_eq!(descendants, height_1);
}

/// Uncle-of-uncle test with valid ancestor relationships.
/// Height-based walk includes all blocks at each height.
///
/// Chain:
///   genesis(h:0) -> A(h:1) -> B(h:2) -> C(h:3, uncles=[uncle1])
///                \-> uncle2(h:1)
///                    A(h:1) -> uncle1(h:2, uncles=[uncle2])
///
/// uncle1 at h:2 is uncle of C at h:3 (ancestor height).
/// uncle2 at h:1 is uncle of uncle1 at h:2 (ancestor height).
#[test]
fn test_get_descendant_blockhashes_chases_uncle_of_uncle() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // A: confirmed at h:1
    let share_a = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .work(2)
        .nonce(1)
        .build();
    store.push_to_confirmed_chain(&share_a).unwrap();

    // uncle2: child of genesis at h:1, no uncles
    let uncle2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(200)
        .build();
    store.store_with_valid_metadata(&uncle2);

    // B: confirmed at h:2
    let share_b = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_a.block_hash().to_string())
        .work(2)
        .nonce(2)
        .build();
    store.push_to_confirmed_chain(&share_b).unwrap();

    // uncle1: child of A at h:2, references uncle2 at h:1 as uncle
    let uncle1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_a.block_hash().to_string())
        .uncles(vec![uncle2.block_hash()])
        .nonce(100)
        .build();
    store.store_with_valid_metadata(&uncle1);

    // C: confirmed at h:3, references uncle1 at h:2 as uncle
    let share_c = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_b.block_hash().to_string())
        .uncles(vec![uncle1.block_hash()])
        .work(2)
        .nonce(3)
        .build();
    store.push_to_confirmed_chain(&share_c).unwrap();

    let descendants = store
        .get_descendant_blockhashes(&genesis.block_hash(), &BlockHash::all_zeros(), 10)
        .unwrap();

    // h:1 has share_a, uncle2; h:2 has share_b, uncle1; h:3 has share_c
    let mut expected: Vec<BlockHash> = Vec::new();
    let mut height_1 = vec![share_a.block_hash(), uncle2.block_hash()];
    height_1.sort();
    expected.extend(height_1);
    let mut height_2 = vec![share_b.block_hash(), uncle1.block_hash()];
    height_2.sort();
    expected.extend(height_2);
    expected.push(share_c.block_hash());
    assert_eq!(descendants, expected);
}

/// Long chain test: locator match deep in the chain. Height-based
/// walk starts at locator_height - MAX_UNCLES_DEPTH + 1 and
/// includes all blocks at each height up to top confirmed. The
/// overlap ensures fork block parents near the boundary are
/// included.
///
/// Chain:
///   genesis(h:0) -> h:1 -> ... -> h:5 -> h:6 -> ... -> h:8 -> h:9(uncles=[uncle]) -> h:10
///                                      \-> uncle(h:6, parent=h:5)
///
/// Locator at h:8. Walk starts at max(8-3,0)+1 = h:6. Uncle at
/// h:6 is included in the overlap.
#[test]
fn test_get_descendant_blockhashes_long_chain_starts_at_uncle_depth() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Build h:1 through h:8 without uncle references
    let mut chain: Vec<ShareBlock> = Vec::with_capacity(10);
    let mut prev_hash = genesis.block_hash().to_string();
    for nonce in 1..=8u32 {
        let block = TestShareBlockBuilder::new()
            .prev_share_blockhash(prev_hash)
            .work(2)
            .nonce(nonce)
            .build();
        store.push_to_confirmed_chain(&block).unwrap();
        prev_hash = block.block_hash().to_string();
        chain.push(block);
    }

    // Uncle at h:6 (parent is chain[4] at h:5) -- deepest possible
    // uncle reachable from h:8 anchor (parent at h:8 - 3 = h:5)
    let uncle = TestShareBlockBuilder::new()
        .prev_share_blockhash(chain[4].block_hash().to_string())
        .nonce(200)
        .build();
    store.store_with_valid_metadata(&uncle);

    // h:9 references the uncle
    let block_h9 = TestShareBlockBuilder::new()
        .prev_share_blockhash(chain[7].block_hash().to_string())
        .uncles(vec![uncle.block_hash()])
        .work(2)
        .nonce(9)
        .build();
    store.push_to_confirmed_chain(&block_h9).unwrap();
    chain.push(block_h9);

    // h:10
    let block_h10 = TestShareBlockBuilder::new()
        .prev_share_blockhash(chain[8].block_hash().to_string())
        .work(2)
        .nonce(10)
        .build();
    store.push_to_confirmed_chain(&block_h10).unwrap();
    chain.push(block_h10);

    // Locator at h:8 (chain[7]). With MAX_UNCLES_DEPTH=3 overlap,
    // walk starts at max(8-3,0)+1 = h:6. Includes h:6 (chain[5] +
    // uncle), h:7 (chain[6]), h:8 (chain[7]), h:9 (chain[8]),
    // h:10 (chain[9]).
    let locator_block = &chain[7];
    let descendants = store
        .get_descendant_blockhashes(&locator_block.block_hash(), &BlockHash::all_zeros(), 100)
        .unwrap();

    let mut expected: Vec<BlockHash> = Vec::new();
    // h:6: chain[5] + uncle
    let mut height_6 = vec![chain[5].block_hash(), uncle.block_hash()];
    height_6.sort();
    expected.extend(height_6);
    // h:7, h:8, h:9, h:10
    expected.push(chain[6].block_hash());
    expected.push(chain[7].block_hash());
    expected.push(chain[8].block_hash());
    expected.push(chain[9].block_hash());
    assert_eq!(descendants, expected);
}

/// A parallel fork chain runs alongside the confirmed chain.
/// Height-based walking includes all blocks at each height, so
/// fork blocks appear naturally without chasing uncle references.
///
/// DAG structure:
///
/// Confirmed: genesis -> C1(h:1) -> C2(h:2) -> C3(h:3) -> C4(h:4) -> C5(h:5)
/// Fork:                 C1(h:1) -> F2(h:2) -> F3(h:3) -> F4(h:4) -> F5(h:5)
/// Fork2:                                       F3(h:3) -> G4(h:4) -> G5(h:5)
///
/// All blocks at each height are included: confirmed, fork, and
/// fork2 blocks all appear in the response sorted lexicographically
/// within each height.
#[test]
fn test_get_descendant_blockhashes_includes_fork_chain_ancestry() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Confirmed chain: genesis -> C1 -> C2 -> C3 -> C4 -> C5
    let confirmed_1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .work(2)
        .nonce(10)
        .build();
    store.push_to_confirmed_chain(&confirmed_1).unwrap();

    let confirmed_2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(confirmed_1.block_hash().to_string())
        .work(2)
        .nonce(20)
        .build();
    store.push_to_confirmed_chain(&confirmed_2).unwrap();

    let confirmed_3 = TestShareBlockBuilder::new()
        .prev_share_blockhash(confirmed_2.block_hash().to_string())
        .work(2)
        .nonce(30)
        .build();
    store.push_to_confirmed_chain(&confirmed_3).unwrap();

    let confirmed_4 = TestShareBlockBuilder::new()
        .prev_share_blockhash(confirmed_3.block_hash().to_string())
        .work(2)
        .nonce(40)
        .build();
    store.push_to_confirmed_chain(&confirmed_4).unwrap();

    // Fork chain: C1 -> F2 -> F3 -> F4 -> F5
    let fork_2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(confirmed_1.block_hash().to_string())
        .nonce(200)
        .build();
    store.store_with_valid_metadata(&fork_2);

    let fork_3 = TestShareBlockBuilder::new()
        .prev_share_blockhash(fork_2.block_hash().to_string())
        .uncles(vec![confirmed_2.block_hash()])
        .nonce(300)
        .build();
    store.store_with_valid_metadata(&fork_3);

    let fork_4 = TestShareBlockBuilder::new()
        .prev_share_blockhash(fork_3.block_hash().to_string())
        .nonce(400)
        .build();
    store.store_with_valid_metadata(&fork_4);

    let fork_5 = TestShareBlockBuilder::new()
        .prev_share_blockhash(fork_4.block_hash().to_string())
        .uncles(vec![confirmed_4.block_hash()])
        .nonce(500)
        .build();
    store.store_with_valid_metadata(&fork_5);

    // Second fork chain from F3: F3 -> G4 -> G5 (unreferenced)
    let fork2_4 = TestShareBlockBuilder::new()
        .prev_share_blockhash(fork_3.block_hash().to_string())
        .nonce(410)
        .build();
    store.store_with_valid_metadata(&fork2_4);

    let fork2_5 = TestShareBlockBuilder::new()
        .prev_share_blockhash(fork2_4.block_hash().to_string())
        .nonce(510)
        .build();
    store.store_with_valid_metadata(&fork2_5);

    // C5 references F2 as uncle, pulling the fork into the DAG
    let confirmed_5 = TestShareBlockBuilder::new()
        .prev_share_blockhash(confirmed_4.block_hash().to_string())
        .uncles(vec![fork_2.block_hash()])
        .work(2)
        .nonce(50)
        .build();
    store.push_to_confirmed_chain(&confirmed_5).unwrap();

    let descendants = store
        .get_descendant_blockhashes(&genesis.block_hash(), &BlockHash::all_zeros(), 100)
        .unwrap();

    // Build expected: all blocks per height, lex sorted within height
    let mut expected: Vec<BlockHash> = Vec::new();
    // h:1
    let mut height_1 = vec![confirmed_1.block_hash()];
    height_1.sort();
    expected.extend(height_1);
    // h:2
    let mut height_2 = vec![confirmed_2.block_hash(), fork_2.block_hash()];
    height_2.sort();
    expected.extend(height_2);
    // h:3
    let mut height_3 = vec![confirmed_3.block_hash(), fork_3.block_hash()];
    height_3.sort();
    expected.extend(height_3);
    // h:4
    let mut height_4 = vec![
        confirmed_4.block_hash(),
        fork_4.block_hash(),
        fork2_4.block_hash(),
    ];
    height_4.sort();
    expected.extend(height_4);
    // h:5
    let mut height_5 = vec![
        confirmed_5.block_hash(),
        fork_5.block_hash(),
        fork2_5.block_hash(),
    ];
    height_5.sort();
    expected.extend(height_5);

    assert_eq!(descendants, expected);
}

/// Same DAG as the previous test but F5 also references G4 as
/// uncle. With height-based walking all blocks appear regardless
/// of uncle references.
///
/// Confirmed: genesis -> C1(h:1) -> C2(h:2) -> C3(h:3) -> C4(h:4) -> C5(h:5)
/// Fork:                 C1(h:1) -> F2(h:2) -> F3(h:3) -> F4(h:4) -> F5(h:5)
/// Fork2:                                       F3(h:3) -> G4(h:4) -> G5(h:5)
#[test]
fn test_get_descendant_blockhashes_includes_fork_uncle_of_fork() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Confirmed chain: genesis -> C1 -> C2 -> C3 -> C4 -> C5
    let confirmed_1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .work(2)
        .nonce(10)
        .build();
    store.push_to_confirmed_chain(&confirmed_1).unwrap();

    let confirmed_2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(confirmed_1.block_hash().to_string())
        .work(2)
        .nonce(20)
        .build();
    store.push_to_confirmed_chain(&confirmed_2).unwrap();

    let confirmed_3 = TestShareBlockBuilder::new()
        .prev_share_blockhash(confirmed_2.block_hash().to_string())
        .work(2)
        .nonce(30)
        .build();
    store.push_to_confirmed_chain(&confirmed_3).unwrap();

    let confirmed_4 = TestShareBlockBuilder::new()
        .prev_share_blockhash(confirmed_3.block_hash().to_string())
        .work(2)
        .nonce(40)
        .build();
    store.push_to_confirmed_chain(&confirmed_4).unwrap();

    // Fork chain: C1 -> F2 -> F3 -> F4 -> F5
    let fork_2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(confirmed_1.block_hash().to_string())
        .nonce(200)
        .build();
    store.store_with_valid_metadata(&fork_2);

    let fork_3 = TestShareBlockBuilder::new()
        .prev_share_blockhash(fork_2.block_hash().to_string())
        .uncles(vec![confirmed_2.block_hash()])
        .nonce(300)
        .build();
    store.store_with_valid_metadata(&fork_3);

    let fork_4 = TestShareBlockBuilder::new()
        .prev_share_blockhash(fork_3.block_hash().to_string())
        .nonce(400)
        .build();
    store.store_with_valid_metadata(&fork_4);

    // Second fork from F3: F3 -> G4 -> G5
    let fork2_4 = TestShareBlockBuilder::new()
        .prev_share_blockhash(fork_3.block_hash().to_string())
        .nonce(410)
        .build();
    store.store_with_valid_metadata(&fork2_4);

    let fork2_5 = TestShareBlockBuilder::new()
        .prev_share_blockhash(fork2_4.block_hash().to_string())
        .nonce(510)
        .build();
    store.store_with_valid_metadata(&fork2_5);

    // F5 references both C4 and G4 as uncles
    let fork_5 = TestShareBlockBuilder::new()
        .prev_share_blockhash(fork_4.block_hash().to_string())
        .uncles(vec![confirmed_4.block_hash(), fork2_4.block_hash()])
        .nonce(500)
        .build();
    store.store_with_valid_metadata(&fork_5);

    // C5 references F2 as uncle
    let confirmed_5 = TestShareBlockBuilder::new()
        .prev_share_blockhash(confirmed_4.block_hash().to_string())
        .uncles(vec![fork_2.block_hash()])
        .work(2)
        .nonce(50)
        .build();
    store.push_to_confirmed_chain(&confirmed_5).unwrap();

    let descendants = store
        .get_descendant_blockhashes(&genesis.block_hash(), &BlockHash::all_zeros(), 100)
        .unwrap();

    // Confirmed chain
    assert!(
        descendants.contains(&confirmed_1.block_hash()),
        "C1 missing"
    );
    assert!(
        descendants.contains(&confirmed_2.block_hash()),
        "C2 missing"
    );
    assert!(
        descendants.contains(&confirmed_3.block_hash()),
        "C3 missing"
    );
    assert!(
        descendants.contains(&confirmed_4.block_hash()),
        "C4 missing"
    );
    assert!(
        descendants.contains(&confirmed_5.block_hash()),
        "C5 missing"
    );

    // All fork blocks included via height-based walk
    assert!(descendants.contains(&fork_2.block_hash()), "F2 missing");
    assert!(descendants.contains(&fork_3.block_hash()), "F3 missing");
    assert!(descendants.contains(&fork_4.block_hash()), "F4 missing");
    assert!(descendants.contains(&fork_5.block_hash()), "F5 missing");
    assert!(descendants.contains(&fork2_4.block_hash()), "G4 missing");
    assert!(
        descendants.contains(&fork2_5.block_hash()),
        "G5 should appear (height-based walk includes all blocks at each height)"
    );
}

/// Height-based walking includes all blocks at each height,
/// covering fork blocks that the old reference-chasing approach
/// missed.
///
/// Confirmed: genesis -> C1(h:1) -> C2(h:2) -> C3(h:3) -> C4(h:4) -> C5(h:5) -> C6(h:6) -> C7(h:7)
/// Fork:                 C1(h:1) -> F2(h:2) -> F3(h:3) -> F4(h:4) -> F5(h:5)
/// Fork2:                                       F3(h:3) -> G4(h:4) -> G5(h:5)
/// Uncle:                                                              U6(h:6)
#[test]
fn test_get_descendant_blockhashes_chases_uncle_parent_chain() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Confirmed chain: genesis -> C1 -> C2 -> C3 -> C4 -> C5 -> C6
    let confirmed_1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .work(2)
        .nonce(10)
        .build();
    store.push_to_confirmed_chain(&confirmed_1).unwrap();

    let confirmed_2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(confirmed_1.block_hash().to_string())
        .work(2)
        .nonce(20)
        .build();
    store.push_to_confirmed_chain(&confirmed_2).unwrap();

    let confirmed_3 = TestShareBlockBuilder::new()
        .prev_share_blockhash(confirmed_2.block_hash().to_string())
        .work(2)
        .nonce(30)
        .build();
    store.push_to_confirmed_chain(&confirmed_3).unwrap();

    let confirmed_4 = TestShareBlockBuilder::new()
        .prev_share_blockhash(confirmed_3.block_hash().to_string())
        .work(2)
        .nonce(40)
        .build();
    store.push_to_confirmed_chain(&confirmed_4).unwrap();

    // Fork chain: C1 -> F2 -> F3 -> F4 -> F5
    let fork_2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(confirmed_1.block_hash().to_string())
        .nonce(200)
        .build();
    store.store_with_valid_metadata(&fork_2);

    let fork_3 = TestShareBlockBuilder::new()
        .prev_share_blockhash(fork_2.block_hash().to_string())
        .nonce(300)
        .build();
    store.store_with_valid_metadata(&fork_3);

    let fork_4 = TestShareBlockBuilder::new()
        .prev_share_blockhash(fork_3.block_hash().to_string())
        .nonce(400)
        .build();
    store.store_with_valid_metadata(&fork_4);

    let fork_5 = TestShareBlockBuilder::new()
        .prev_share_blockhash(fork_4.block_hash().to_string())
        .nonce(500)
        .build();
    store.store_with_valid_metadata(&fork_5);

    // Second fork from F3: F3 -> G4 -> G5 (unreferenced)
    let fork2_4 = TestShareBlockBuilder::new()
        .prev_share_blockhash(fork_3.block_hash().to_string())
        .nonce(410)
        .build();
    store.store_with_valid_metadata(&fork2_4);

    let fork2_5 = TestShareBlockBuilder::new()
        .prev_share_blockhash(fork2_4.block_hash().to_string())
        .nonce(510)
        .build();
    store.store_with_valid_metadata(&fork2_5);

    // C5 references F2 (fork base) as uncle
    let confirmed_5 = TestShareBlockBuilder::new()
        .prev_share_blockhash(confirmed_4.block_hash().to_string())
        .uncles(vec![fork_2.block_hash()])
        .work(2)
        .nonce(50)
        .build();
    store.push_to_confirmed_chain(&confirmed_5).unwrap();

    let confirmed_6 = TestShareBlockBuilder::new()
        .prev_share_blockhash(confirmed_5.block_hash().to_string())
        .work(2)
        .nonce(60)
        .build();
    store.push_to_confirmed_chain(&confirmed_6).unwrap();

    // U6: uncle at h:6 that references F5 (fork tip) as its uncle
    let uncle_6 = TestShareBlockBuilder::new()
        .prev_share_blockhash(confirmed_5.block_hash().to_string())
        .uncles(vec![fork_5.block_hash()])
        .nonce(600)
        .build();
    store.store_with_valid_metadata(&uncle_6);

    // C7 references U6 as uncle
    let confirmed_7 = TestShareBlockBuilder::new()
        .prev_share_blockhash(confirmed_6.block_hash().to_string())
        .uncles(vec![uncle_6.block_hash()])
        .work(2)
        .nonce(70)
        .build();
    store.push_to_confirmed_chain(&confirmed_7).unwrap();

    let descendants = store
        .get_descendant_blockhashes(&genesis.block_hash(), &BlockHash::all_zeros(), 100)
        .unwrap();

    let mut expected: Vec<BlockHash> = Vec::new();
    // h:1
    expected.push(confirmed_1.block_hash());
    // h:2
    let mut height_2 = vec![confirmed_2.block_hash(), fork_2.block_hash()];
    height_2.sort();
    expected.extend(height_2);
    // h:3
    let mut height_3 = vec![confirmed_3.block_hash(), fork_3.block_hash()];
    height_3.sort();
    expected.extend(height_3);
    // h:4
    let mut height_4 = vec![
        confirmed_4.block_hash(),
        fork_4.block_hash(),
        fork2_4.block_hash(),
    ];
    height_4.sort();
    expected.extend(height_4);
    // h:5
    let mut height_5 = vec![
        confirmed_5.block_hash(),
        fork_5.block_hash(),
        fork2_5.block_hash(),
    ];
    height_5.sort();
    expected.extend(height_5);
    // h:6
    let mut height_6 = vec![confirmed_6.block_hash(), uncle_6.block_hash()];
    height_6.sort();
    expected.extend(height_6);
    // h:7
    expected.push(confirmed_7.block_hash());

    assert_eq!(descendants, expected);
}

/// Same DAG as above but F5 also references G4 as uncle. With
/// height-based walking all blocks appear regardless of uncle
/// references.
///
/// Confirmed: genesis -> C1 -> C2 -> C3 -> C4 -> C5 -> C6 -> C7
/// Fork:                 C1 -> F2 -> F3 -> F4 -> F5
/// Fork2:                            F3 -> G4 -> G5
/// Uncle:                                          U6
#[test]
fn test_get_descendant_blockhashes_chases_uncle_parent_chain_with_fork_uncle() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Confirmed chain: genesis -> C1 -> C2 -> C3 -> C4
    let confirmed_1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .work(2)
        .nonce(10)
        .build();
    store.push_to_confirmed_chain(&confirmed_1).unwrap();

    let confirmed_2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(confirmed_1.block_hash().to_string())
        .work(2)
        .nonce(20)
        .build();
    store.push_to_confirmed_chain(&confirmed_2).unwrap();

    let confirmed_3 = TestShareBlockBuilder::new()
        .prev_share_blockhash(confirmed_2.block_hash().to_string())
        .work(2)
        .nonce(30)
        .build();
    store.push_to_confirmed_chain(&confirmed_3).unwrap();

    let confirmed_4 = TestShareBlockBuilder::new()
        .prev_share_blockhash(confirmed_3.block_hash().to_string())
        .work(2)
        .nonce(40)
        .build();
    store.push_to_confirmed_chain(&confirmed_4).unwrap();

    // Fork chain: C1 -> F2 -> F3 -> F4
    let fork_2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(confirmed_1.block_hash().to_string())
        .nonce(200)
        .build();
    store.store_with_valid_metadata(&fork_2);

    let fork_3 = TestShareBlockBuilder::new()
        .prev_share_blockhash(fork_2.block_hash().to_string())
        .nonce(300)
        .build();
    store.store_with_valid_metadata(&fork_3);

    let fork_4 = TestShareBlockBuilder::new()
        .prev_share_blockhash(fork_3.block_hash().to_string())
        .nonce(400)
        .build();
    store.store_with_valid_metadata(&fork_4);

    // Second fork from F3: F3 -> G4 -> G5
    let fork2_4 = TestShareBlockBuilder::new()
        .prev_share_blockhash(fork_3.block_hash().to_string())
        .nonce(410)
        .build();
    store.store_with_valid_metadata(&fork2_4);

    let fork2_5 = TestShareBlockBuilder::new()
        .prev_share_blockhash(fork2_4.block_hash().to_string())
        .nonce(510)
        .build();
    store.store_with_valid_metadata(&fork2_5);

    // F5 references G4 as uncle
    let fork_5 = TestShareBlockBuilder::new()
        .prev_share_blockhash(fork_4.block_hash().to_string())
        .uncles(vec![fork2_4.block_hash()])
        .nonce(500)
        .build();
    store.store_with_valid_metadata(&fork_5);

    // C5 references F2 as uncle
    let confirmed_5 = TestShareBlockBuilder::new()
        .prev_share_blockhash(confirmed_4.block_hash().to_string())
        .uncles(vec![fork_2.block_hash()])
        .work(2)
        .nonce(50)
        .build();
    store.push_to_confirmed_chain(&confirmed_5).unwrap();

    let confirmed_6 = TestShareBlockBuilder::new()
        .prev_share_blockhash(confirmed_5.block_hash().to_string())
        .work(2)
        .nonce(60)
        .build();
    store.push_to_confirmed_chain(&confirmed_6).unwrap();

    // U6 references F5 as uncle
    let uncle_6 = TestShareBlockBuilder::new()
        .prev_share_blockhash(confirmed_5.block_hash().to_string())
        .uncles(vec![fork_5.block_hash()])
        .nonce(600)
        .build();
    store.store_with_valid_metadata(&uncle_6);

    // C7 references U6 as uncle
    let confirmed_7 = TestShareBlockBuilder::new()
        .prev_share_blockhash(confirmed_6.block_hash().to_string())
        .uncles(vec![uncle_6.block_hash()])
        .work(2)
        .nonce(70)
        .build();
    store.push_to_confirmed_chain(&confirmed_7).unwrap();

    let descendants = store
        .get_descendant_blockhashes(&genesis.block_hash(), &BlockHash::all_zeros(), 100)
        .unwrap();

    let mut expected: Vec<BlockHash> = Vec::new();
    // h:1
    expected.push(confirmed_1.block_hash());
    // h:2
    let mut height_2 = vec![confirmed_2.block_hash(), fork_2.block_hash()];
    height_2.sort();
    expected.extend(height_2);
    // h:3
    let mut height_3 = vec![confirmed_3.block_hash(), fork_3.block_hash()];
    height_3.sort();
    expected.extend(height_3);
    // h:4
    let mut height_4 = vec![
        confirmed_4.block_hash(),
        fork_4.block_hash(),
        fork2_4.block_hash(),
    ];
    height_4.sort();
    expected.extend(height_4);
    // h:5
    let mut height_5 = vec![
        confirmed_5.block_hash(),
        fork_5.block_hash(),
        fork2_5.block_hash(),
    ];
    height_5.sort();
    expected.extend(height_5);
    // h:6
    let mut height_6 = vec![confirmed_6.block_hash(), uncle_6.block_hash()];
    height_6.sort();
    expected.extend(height_6);
    // h:7
    expected.push(confirmed_7.block_hash());

    assert_eq!(descendants, expected);
}

#[test]
fn test_find_chain_for_depth_linear_chain() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // Create a linear chain of 10 blocks
    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let mut prev_hash = genesis.block_hash();
    let mut blocks = vec![genesis.block_hash()];

    for i in 1..10 {
        let share = TestShareBlockBuilder::new()
            .prev_share_blockhash(prev_hash.to_string())
            .nonce(0xe9695791 + i)
            .build();

        store.store_with_valid_metadata(&share);

        blocks.push(share.block_hash());
        prev_hash = share.block_hash();
    }

    // Test finding chain from tip with depth 5
    let chain = store.get_dag_for_depth(&blocks[9], 5).unwrap();

    // Should return blocks 9, 8, 7, 6, 5 (from newest to oldest)
    assert_eq!(chain.len(), 5);
    assert_eq!(chain[0], blocks[9]);
    assert_eq!(chain[1], blocks[8]);
    assert_eq!(chain[2], blocks[7]);
    assert_eq!(chain[3], blocks[6]);
    assert_eq!(chain[4], blocks[5]);

    // Test finding chain with depth greater than chain length
    let chain = store.get_dag_for_depth(&blocks[5], 10).unwrap();

    // Should return blocks 5, 4, 3, 2, 1, 0 (6 blocks total)
    assert_eq!(chain.len(), 6);
    assert_eq!(chain[0], blocks[5]);
    assert_eq!(chain[5], blocks[0]);

    // Test finding chain from genesis
    let chain = store.get_dag_for_depth(&blocks[0], 5).unwrap();

    // Should return only genesis (height 0, depth 0)
    assert_eq!(chain.len(), 1);
    assert_eq!(chain[0], blocks[0]);
}

#[test]
fn test_get_common_ancestor_linear_chain() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // Create a linear chain: genesis -> share1 -> share2 -> share3
    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    store.store_with_valid_metadata(&share1);

    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(0xe9695793)
        .build();
    store.store_with_valid_metadata(&share2);

    let share3 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share2.block_hash().to_string())
        .nonce(0xe9695794)
        .build();
    store.store_with_valid_metadata(&share3);

    // Test common ancestor of share3 and share2
    let ancestor = store
        .get_common_ancestor(&share3.block_hash(), &share2.block_hash())
        .unwrap();
    assert_eq!(ancestor, Some(share2.block_hash()));

    // Test common ancestor of share3 and share1
    let ancestor = store
        .get_common_ancestor(&share3.block_hash(), &share1.block_hash())
        .unwrap();
    assert_eq!(ancestor, Some(share1.block_hash()));

    // Test common ancestor of share3 and genesis
    let ancestor = store
        .get_common_ancestor(&share3.block_hash(), &genesis.block_hash())
        .unwrap();
    assert_eq!(ancestor, Some(genesis.block_hash()));

    // Test common ancestor of share2 and share1
    let ancestor = store
        .get_common_ancestor(&share2.block_hash(), &share1.block_hash())
        .unwrap();
    assert_eq!(ancestor, Some(share1.block_hash()));
}

#[test]
fn test_get_common_ancestor_with_fork() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // Create a chain with a fork:
    //        genesis
    //         /  \
    //    share1  uncle1
    //      |
    //    share2
    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    store.store_with_valid_metadata(&share1);

    let uncle1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695793)
        .build();
    store.store_with_valid_metadata(&uncle1);

    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(0xe9695794)
        .build();
    store.store_with_valid_metadata(&share2);

    // Test common ancestor of share2 and uncle1 (should be genesis)
    let ancestor = store
        .get_common_ancestor(&share2.block_hash(), &uncle1.block_hash())
        .unwrap();
    assert_eq!(ancestor, Some(genesis.block_hash()));

    // Test common ancestor of share1 and uncle1 (should be genesis)
    let ancestor = store
        .get_common_ancestor(&share1.block_hash(), &uncle1.block_hash())
        .unwrap();
    assert_eq!(ancestor, Some(genesis.block_hash()));

    // Test common ancestor of share2 and share1 (should be share1)
    let ancestor = store
        .get_common_ancestor(&share2.block_hash(), &share1.block_hash())
        .unwrap();
    assert_eq!(ancestor, Some(share1.block_hash()));
}

#[test]
fn test_get_common_ancestor_no_common_within_depth() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // Create two separate chains (only for testing - wouldn't happen in real usage)
    let genesis1 = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&genesis1, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let genesis2 = TestShareBlockBuilder::new().nonce(0xe9695792).build();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&genesis2, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Test common ancestor of two different genesis blocks (should be None)
    let ancestor = store
        .get_common_ancestor(&genesis1.block_hash(), &genesis2.block_hash())
        .unwrap();
    assert_eq!(ancestor, None);
}

#[test]
fn test_get_dag_for_depth_no_duplicates_with_uncle() {
    // This test verifies that get_dag_for_depth does not return duplicate blocks
    // when uncles and main chain blocks share a common parent.
    //
    // DAG structure:
    //       share1
    //      /      \
    //   uncle1    share2
    //              |
    //            share3 (uncles=[uncle1])
    //
    // uncle1 is a sibling of share2 (both have share1 as parent).
    // share3 references uncle1 as its uncle.
    // When traversing from share3, both share2's parent (share1) and
    // uncle1's parent (share1) point to the same block. Without visited
    // tracking, share1 would appear twice in the results.

    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // Create genesis
    let share1 = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&share1, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Create uncle1 - sibling of share2
    let uncle1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(100)
        .build();
    store.store_with_valid_metadata(&uncle1);

    // Create share2 - sibling of uncle1
    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .build();
    store.store_with_valid_metadata(&share2);

    // Create share3 with uncle1 as uncle (uncle1 is sibling of share3's parent)
    let share3 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share2.block_hash().to_string())
        .uncles(vec![uncle1.block_hash()])
        .build();
    store.store_with_valid_metadata(&share3);

    // Get DAG from share3 with depth 10 (more than enough to include all blocks)
    let chain = store.get_dag_for_depth(&share3.block_hash(), 10).unwrap();

    // Verify no duplicates by checking that all blockhashes are unique
    let unique_hashes: HashSet<BlockHash> = chain.iter().cloned().collect();
    assert_eq!(
        chain.len(),
        unique_hashes.len(),
        "get_dag_for_depth returned duplicate blocks"
    );

    // Should contain exactly 4 unique blocks: share3, share2, uncle1, share1
    assert_eq!(chain.len(), 4);
    assert!(unique_hashes.contains(&share3.block_hash()));
    assert!(unique_hashes.contains(&share2.block_hash()));
    assert!(unique_hashes.contains(&uncle1.block_hash()));
    assert!(unique_hashes.contains(&share1.block_hash()));
}

#[test]
fn test_get_dag_for_depth_no_duplicates_with_multiple_uncles() {
    // More complex test with multiple uncles
    //
    // DAG structure:
    //              share1
    //           /    |    \
    //      uncle1  uncle2  share2
    //                        |
    //                      share3 (uncles=[uncle1, uncle2])
    //
    // uncle1, uncle2, and share2 are all siblings (share1 as parent).
    // share3 references both uncle1 and uncle2 as uncles.
    // Without visited tracking, share1 would be added to the queue 3 times.

    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let share1 = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&share1, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let uncle1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(100)
        .build();
    store.store_with_valid_metadata(&uncle1);

    let uncle2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(200)
        .build();
    store.store_with_valid_metadata(&uncle2);

    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .build();
    store.store_with_valid_metadata(&share2);

    let share3 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share2.block_hash().to_string())
        .uncles(vec![uncle1.block_hash(), uncle2.block_hash()])
        .build();
    store.store_with_valid_metadata(&share3);

    let chain = store.get_dag_for_depth(&share3.block_hash(), 10).unwrap();

    // Verify no duplicates
    let unique_hashes: HashSet<BlockHash> = chain.iter().cloned().collect();
    assert_eq!(
        chain.len(),
        unique_hashes.len(),
        "get_dag_for_depth returned duplicate blocks"
    );

    // Should contain exactly 5 unique blocks
    assert_eq!(chain.len(), 5);
    assert!(unique_hashes.contains(&share3.block_hash()));
    assert!(unique_hashes.contains(&share2.block_hash()));
    assert!(unique_hashes.contains(&uncle1.block_hash()));
    assert!(unique_hashes.contains(&uncle2.block_hash()));
    assert!(unique_hashes.contains(&share1.block_hash()));
}

#[test]
fn test_get_dag_for_depth_no_duplicates_deep_uncle_chain() {
    // Test with uncles at multiple levels of the chain
    //
    // DAG structure:
    //       share1
    //      /      \
    //   uncle1    share2
    //            /      \
    //         uncle2    share3 (uncles=[uncle1])
    //                     |
    //                   share4 (uncles=[uncle2])
    //
    // This creates multiple opportunities for duplicate parent references.

    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let share1 = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&share1, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // uncle1 is sibling of share2
    let uncle1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(100)
        .build();
    store.store_with_valid_metadata(&uncle1);

    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .build();
    store.store_with_valid_metadata(&share2);

    // uncle2 is sibling of share3
    let uncle2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share2.block_hash().to_string())
        .nonce(200)
        .build();
    store.store_with_valid_metadata(&uncle2);

    // share3 has uncle1 as uncle (sibling of its parent share2)
    let share3 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share2.block_hash().to_string())
        .uncles(vec![uncle1.block_hash()])
        .build();
    store.store_with_valid_metadata(&share3);

    // share4 has uncle2 as uncle (sibling of its parent share3)
    let share4 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share3.block_hash().to_string())
        .uncles(vec![uncle2.block_hash()])
        .build();
    store.store_with_valid_metadata(&share4);

    let chain = store.get_dag_for_depth(&share4.block_hash(), 10).unwrap();

    // Verify no duplicates
    let unique_hashes: HashSet<BlockHash> = chain.iter().cloned().collect();
    assert_eq!(
        chain.len(),
        unique_hashes.len(),
        "get_dag_for_depth returned duplicate blocks"
    );

    // Should contain exactly 6 unique blocks
    assert_eq!(chain.len(), 6);
    assert!(unique_hashes.contains(&share4.block_hash()));
    assert!(unique_hashes.contains(&share3.block_hash()));
    assert!(unique_hashes.contains(&share2.block_hash()));
    assert!(unique_hashes.contains(&share1.block_hash()));
    assert!(unique_hashes.contains(&uncle1.block_hash()));
    assert!(unique_hashes.contains(&uncle2.block_hash()));
}

#[test]
fn test_get_dag_for_depth_uncles_do_not_count_toward_depth() {
    // This test verifies that uncles do not count toward the main chain depth.
    // With depth=2, we should get 2 main chain blocks plus all their uncles.
    //
    // DAG structure:
    //       share1
    //      /      \
    //   uncle1    share2
    //              |
    //            share3 (uncles=[uncle1])
    //
    // With depth=2 starting from share3:
    // - Main chain blocks: share3, share2 (2 blocks = depth)
    // - Uncles: uncle1 (referenced by share3)
    // Result should be [share3, share2, uncle1] - NOT missing share2 due to uncle

    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let share1 = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&share1, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let uncle1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(100)
        .build();
    store.store_with_valid_metadata(&uncle1);

    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .build();
    store.store_with_valid_metadata(&share2);

    let share3 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share2.block_hash().to_string())
        .uncles(vec![uncle1.block_hash()])
        .build();
    store.store_with_valid_metadata(&share3);

    // With depth=2, we should get exactly 2 main chain blocks + uncles
    let chain = store.get_dag_for_depth(&share3.block_hash(), 2).unwrap();

    let chain_hashes: HashSet<BlockHash> = chain.iter().cloned().collect();

    // Must include both main chain blocks (share3 and share2)
    assert!(
        chain_hashes.contains(&share3.block_hash()),
        "share3 should be in result"
    );
    assert!(
        chain_hashes.contains(&share2.block_hash()),
        "share2 should be in result - uncle should not consume depth"
    );

    // Must include the uncle
    assert!(
        chain_hashes.contains(&uncle1.block_hash()),
        "uncle1 should be in result"
    );

    // Should NOT include share1 (beyond depth=2)
    assert!(
        !chain_hashes.contains(&share1.block_hash()),
        "share1 should NOT be in result - beyond depth"
    );

    // Total: 3 blocks (2 main chain + 1 uncle)
    assert_eq!(chain.len(), 3);
}

#[test]
fn test_get_dag_for_depth_multiple_uncles_do_not_affect_main_chain_depth() {
    // Test that even with many uncles, we still get the correct number of main chain blocks.
    //
    // DAG structure:
    //              share1
    //           /    |    \
    //      uncle1  uncle2  share2
    //                        |
    //                      share3 (uncles=[uncle1, uncle2])
    //                        |
    //                      share4
    //
    // With depth=2 starting from share4:
    // - Main chain: share4, share3
    // - Uncles of share3: uncle1, uncle2
    // Result: [share4, share3, uncle1, uncle2]

    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let share1 = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&share1, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let uncle1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(100)
        .build();
    store.store_with_valid_metadata(&uncle1);

    let uncle2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(200)
        .build();
    store.store_with_valid_metadata(&uncle2);

    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .build();
    store.store_with_valid_metadata(&share2);

    let share3 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share2.block_hash().to_string())
        .uncles(vec![uncle1.block_hash(), uncle2.block_hash()])
        .build();
    store.store_with_valid_metadata(&share3);

    let share4 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share3.block_hash().to_string())
        .build();
    store.store_with_valid_metadata(&share4);

    // With depth=2, we should get share4, share3 (main chain) + uncle1, uncle2
    let chain = store.get_dag_for_depth(&share4.block_hash(), 2).unwrap();

    let chain_hashes: HashSet<BlockHash> = chain.iter().cloned().collect();

    // Main chain blocks
    assert!(chain_hashes.contains(&share4.block_hash()));
    assert!(chain_hashes.contains(&share3.block_hash()));

    // Uncles of share3
    assert!(chain_hashes.contains(&uncle1.block_hash()));
    assert!(chain_hashes.contains(&uncle2.block_hash()));

    // Should NOT include share2 or share1 (beyond depth)
    assert!(!chain_hashes.contains(&share2.block_hash()));
    assert!(!chain_hashes.contains(&share1.block_hash()));

    // Total: 4 blocks (2 main chain + 2 uncles)
    assert_eq!(chain.len(), 4);
}

#[test]
fn test_get_dag_for_depth_uncles_at_multiple_levels() {
    // Test with uncles at each level of the main chain.
    //
    // DAG structure:
    //       share1
    //      /      \
    //   uncle1    share2
    //            /      \
    //         uncle2    share3 (uncles=[uncle1])
    //                     |
    //                   share4 (uncles=[uncle2])
    //
    // With depth=3 starting from share4:
    // - Main chain: share4, share3, share2 (3 blocks)
    // - Uncles: uncle2 (from share4), uncle1 (from share3)
    // Result: [share4, share3, share2, uncle2, uncle1]

    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let share1 = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&share1, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let uncle1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(100)
        .build();
    store.store_with_valid_metadata(&uncle1);

    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .build();
    store.store_with_valid_metadata(&share2);

    let uncle2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share2.block_hash().to_string())
        .nonce(200)
        .build();
    store.store_with_valid_metadata(&uncle2);

    let share3 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share2.block_hash().to_string())
        .uncles(vec![uncle1.block_hash()])
        .build();
    store.store_with_valid_metadata(&share3);

    let share4 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share3.block_hash().to_string())
        .uncles(vec![uncle2.block_hash()])
        .build();
    store.store_with_valid_metadata(&share4);

    // With depth=3, we should get share4, share3, share2 (main chain) + uncle2, uncle1
    let chain = store.get_dag_for_depth(&share4.block_hash(), 3).unwrap();

    let chain_hashes: HashSet<BlockHash> = chain.iter().cloned().collect();

    // Main chain blocks (exactly 3)
    assert!(chain_hashes.contains(&share4.block_hash()));
    assert!(chain_hashes.contains(&share3.block_hash()));
    assert!(chain_hashes.contains(&share2.block_hash()));

    // Uncles from the processed main chain blocks
    assert!(chain_hashes.contains(&uncle1.block_hash())); // uncle of share3
    assert!(chain_hashes.contains(&uncle2.block_hash())); // uncle of share4

    // Should NOT include share1 (beyond depth)
    assert!(!chain_hashes.contains(&share1.block_hash()));

    // Total: 5 blocks (3 main chain + 2 uncles)
    assert_eq!(chain.len(), 5);
}

#[test]
fn test_get_dag_for_depth_exact_depth_boundary() {
    // Test that depth is exact - we get exactly N main chain blocks.
    //
    // Chain: share1 -> share2 -> share3 -> share4 -> share5
    //
    // With depth=3 from share5: should get share5, share4, share3 (exactly 3)

    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let share1 = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&share1, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .build();
    store.store_with_valid_metadata(&share2);

    let share3 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share2.block_hash().to_string())
        .build();
    store.store_with_valid_metadata(&share3);

    let share4 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share3.block_hash().to_string())
        .build();
    store.store_with_valid_metadata(&share4);

    let share5 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share4.block_hash().to_string())
        .build();
    store.store_with_valid_metadata(&share5);

    // With depth=3, should get exactly 3 main chain blocks
    let chain = store.get_dag_for_depth(&share5.block_hash(), 3).unwrap();

    assert_eq!(chain.len(), 3);
    assert_eq!(chain[0], share5.block_hash());
    assert_eq!(chain[1], share4.block_hash());
    assert_eq!(chain[2], share3.block_hash());

    // With depth=1, should get exactly 1 main chain block
    let chain = store.get_dag_for_depth(&share5.block_hash(), 1).unwrap();

    assert_eq!(chain.len(), 1);
    assert_eq!(chain[0], share5.block_hash());
}

#[test]
fn test_get_nephews_returns_none_for_unused_uncle() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let share = TestShareBlockBuilder::new().build();
    let mut batch = rocksdb::WriteBatch::default();
    store.add_share_block(&share, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // A share that has not been used as an uncle should return None
    assert!(store.get_nephews(&share.block_hash()).is_none());
}

#[test]
fn test_get_nephews_returns_nephews_for_used_uncle() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let uncle = TestShareBlockBuilder::new().nonce(100).build();
    let nephew1 = TestShareBlockBuilder::new().nonce(200).build();
    let nephew2 = TestShareBlockBuilder::new().nonce(300).build();

    let mut batch = rocksdb::WriteBatch::default();
    store.add_share_block(&uncle, &mut batch).unwrap();
    store.add_share_block(&nephew1, &mut batch).unwrap();
    store.add_share_block(&nephew2, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Add uncle to uncles index with two nephews
    let mut batch = rocksdb::WriteBatch::default();
    store
        .add_to_uncles_index(&uncle.block_hash(), &nephew1.block_hash(), &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let mut batch = rocksdb::WriteBatch::default();
    store
        .add_to_uncles_index(&uncle.block_hash(), &nephew2.block_hash(), &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    // Get nephews
    let nephews = store.get_nephews(&uncle.block_hash()).unwrap();
    assert_eq!(nephews.len(), 2);
    assert!(nephews.contains(&nephew1.block_hash()));
    assert!(nephews.contains(&nephew2.block_hash()));
}

#[test]
fn test_find_uncles_returns_error_when_no_confirmed_chain() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let share = TestShareBlockBuilder::new().build();
    let mut batch = rocksdb::WriteBatch::default();
    store.add_share_block(&share, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // The share has no ancestors, so there are no uncle candidates.
    let result = store.find_uncles(&share.block_hash()).unwrap();
    assert!(result.is_empty());
}

#[test]
fn test_find_uncles_returns_empty_when_no_uncles_available() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // Create a linear chain of confirmed blocks with no forks
    let share0 = TestShareBlockBuilder::new().nonce(0).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&share0, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share0.block_hash().to_string())
        .nonce(1)
        .build();
    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(2)
        .build();

    // Confirm all using push_to_confirmed_chain
    store.push_to_confirmed_chain(&share1).unwrap();
    store.push_to_confirmed_chain(&share2).unwrap();

    // No unconfirmed children exist, so find_uncles should return empty
    let uncles = store.find_uncles(&store.get_chain_tip().unwrap()).unwrap();
    assert!(uncles.is_empty());
}

#[test]
fn test_find_uncles_finds_single_uncle() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // Build chain:
    //   share0 (confirmed)
    //   /    \
    // share1  uncle1 (not confirmed)
    // (confirmed)

    let share0 = TestShareBlockBuilder::new().nonce(0).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&share0, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share0.block_hash().to_string())
        .nonce(1)
        .build();
    let uncle1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share0.block_hash().to_string())
        .nonce(100)
        .build();

    // Store uncle with Valid metadata so find_uncles can read its metadata
    store.store_with_valid_metadata(&uncle1);

    // Confirm share1 using push_to_confirmed_chain
    store.push_to_confirmed_chain(&share1).unwrap();

    // find_uncles should find uncle1
    let uncles = store.find_uncles(&store.get_chain_tip().unwrap()).unwrap();
    assert_eq!(uncles.len(), 1);
    assert!(uncles.contains(&uncle1.block_hash()));
}

#[test]
fn test_find_uncles_excludes_header_only_blocks_without_body() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // Build chain:
    //   share0 (confirmed)
    //   /    \
    // share1  uncle_no_body (header only, no block body)
    // (confirmed)

    let share0 = TestShareBlockBuilder::new().nonce(0).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&share0, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share0.block_hash().to_string())
        .nonce(1)
        .build();
    let uncle_no_body = TestShareBlockBuilder::new()
        .prev_share_blockhash(share0.block_hash().to_string())
        .nonce(100)
        .build();

    // Store uncle with header and metadata only, no block body.
    // Also write the parent->child block index so find_uncles
    // discovers it via get_children_blockhashes.
    store.create_valid_metadata_only(&uncle_no_body);
    let mut batch = Store::get_write_batch();
    store
        .update_block_index(
            &share0.block_hash(),
            &uncle_no_body.block_hash(),
            &mut batch,
        )
        .unwrap();
    store.commit_batch(batch).unwrap();
    assert!(!store.share_block_exists(&uncle_no_body.block_hash()));

    store.push_to_confirmed_chain(&share1).unwrap();

    // find_uncles should NOT include the header-only uncle
    let uncles = store.find_uncles(&store.get_chain_tip().unwrap()).unwrap();
    assert!(
        uncles.is_empty(),
        "Header-only uncle without block body should not be selected"
    );
}

#[test]
fn test_find_uncles_finds_multiple_uncles_at_different_heights() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // Build chain:
    //   share0 (confirmed, height 0)
    //   /    \
    // share1  uncle0 (not confirmed, height 1)
    // (confirmed, height 1)
    //   |    \
    // share2  uncle1 (not confirmed, height 2)
    // (confirmed, height 2)
    //   |    \
    // share3  uncle2 (not confirmed, height 3)
    // (confirmed, height 3)

    let share0 = TestShareBlockBuilder::new().nonce(0).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&share0, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share0.block_hash().to_string())
        .nonce(1)
        .build();
    let uncle0 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share0.block_hash().to_string())
        .nonce(100)
        .build();
    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(2)
        .build();
    let uncle1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .work(2)
        .nonce(101)
        .build();
    let share3 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share2.block_hash().to_string())
        .nonce(3)
        .build();
    let uncle2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share2.block_hash().to_string())
        .work(3)
        .nonce(102)
        .build();

    // Store uncles with Valid metadata before confirming their parent levels.
    // Uncle parents must have metadata for cumulative chain_work to be correct.
    store.store_with_valid_metadata(&uncle0);
    store.push_to_confirmed_chain(&share1).unwrap();

    store.store_with_valid_metadata(&uncle1);
    store.push_to_confirmed_chain(&share2).unwrap();

    store.store_with_valid_metadata(&uncle2);
    store.push_to_confirmed_chain(&share3).unwrap();

    // find_uncles should find uncle0, uncle1, uncle2
    // Sorted by chain_work descending: uncle2 (work=3), uncle1 (work=2), uncle0 (work=1)
    let uncles = store.find_uncles(&store.get_chain_tip().unwrap()).unwrap();
    assert_eq!(uncles.len(), 3);
    // Verify order - highest chain_work first
    assert_eq!(uncles[0], uncle2.block_hash());
    assert_eq!(uncles[1], uncle1.block_hash());
    assert_eq!(uncles[2], uncle0.block_hash());
}

#[test]
fn test_find_uncles_excludes_uncles_beyond_depth_3() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // Build chain with 6 blocks (heights 0-5) and uncles at various depths
    // Uncle at height 1 will be beyond depth 3 when looking from height 5
    //
    // share0 (confirmed, height 0)
    //   |    \
    // share1  uncle_deep (height 1, beyond depth 3 from height 5)
    //   |
    // share2 (confirmed, height 2)
    //   |    \
    // share3  uncle_within (height 3, within depth 3 from height 5)
    //   |
    // share4 (confirmed, height 4)
    //   |
    // share5 (confirmed, height 5)

    let share0 = TestShareBlockBuilder::new().nonce(0).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&share0, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share0.block_hash().to_string())
        .nonce(1)
        .build();
    let uncle_deep = TestShareBlockBuilder::new()
        .prev_share_blockhash(share0.block_hash().to_string())
        .nonce(100)
        .build();
    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(2)
        .build();
    let share3 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share2.block_hash().to_string())
        .nonce(3)
        .build();
    let uncle_within = TestShareBlockBuilder::new()
        .prev_share_blockhash(share2.block_hash().to_string())
        .nonce(101)
        .build();
    let share4 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share3.block_hash().to_string())
        .nonce(4)
        .build();
    let share5 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share4.block_hash().to_string())
        .nonce(5)
        .build();

    // Confirm main chain up to each uncle's parent before storing uncles
    store.push_to_confirmed_chain(&share1).unwrap();
    store.store_with_valid_metadata(&uncle_deep);
    store.push_to_confirmed_chain(&share2).unwrap();
    store.push_to_confirmed_chain(&share3).unwrap();
    store.store_with_valid_metadata(&uncle_within);
    store.push_to_confirmed_chain(&share4).unwrap();
    store.push_to_confirmed_chain(&share5).unwrap();

    // find_uncles from share5 (height 5)
    // Should only find uncle_within (at height 3, within depth 3: heights 2,3,4)
    // Should NOT find uncle_deep (at height 1, beyond the range we look at)
    let uncles = store.find_uncles(&store.get_chain_tip().unwrap()).unwrap();

    assert_eq!(uncles.len(), 1);
    assert!(uncles.contains(&uncle_within.block_hash()));
    assert!(!uncles.contains(&uncle_deep.block_hash()));
}

/// A block this node marked `Invalid` is never selected as an uncle: it
/// would pay PPLNS weight to a block we judged to break the rules. This is
/// selection only -- `validate_uncles` still accepts a peer's nephew that
/// references such a block, because uncle acceptance must not depend on
/// per-node validation progress.
#[test]
fn test_find_uncles_excludes_invalid_and_keeps_header_valid() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let base = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(1)
        .build();
    store.push_to_confirmed_chain(&base).unwrap();

    // Three siblings of base, off the confirmed chain: one left at
    // HeaderValid, one marked BlockValid, one marked Invalid.
    let header_valid = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(100)
        .build();
    store.store_with_valid_metadata(&header_valid);

    let block_valid = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(101)
        .build();
    store.store_with_valid_metadata(&block_valid);

    let invalid = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(102)
        .build();
    store.store_with_valid_metadata(&invalid);

    let mut batch = Store::get_write_batch();
    store
        .mark_block_valid(&block_valid.block_hash(), &mut batch)
        .unwrap();
    store
        .mark_invalid(&invalid.block_hash(), None, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let uncles = store.find_uncles(&base.block_hash()).unwrap();

    assert!(
        !uncles.contains(&invalid.block_hash()),
        "an Invalid block must not be paid uncle weight"
    );
    assert!(
        uncles.contains(&block_valid.block_hash()),
        "a validated sibling is still an uncle"
    );
    assert!(
        uncles.contains(&header_valid.block_hash()),
        "an uncle needs a body, not chain-context validation"
    );
}

/// `BlockIndex` stores uncle -> nephew edges under the same key as
/// parent -> child ones, so a nephew can be read back as a "child" of an
/// ancestor. A nephew sits at or above the new share's own height, and
/// `validate_uncle_positions` rejects such an uncle -- meaning the node
/// would mine a share its own organise worker marks Invalid.
/// An invalidated block hides its whole subtree: those descendants can
/// never be confirmed, because the candidate chain cannot reorg through an
/// Invalid ancestor. The cached pointer this search replaced kept naming
/// such a descendant, so the pool mined on a dead branch.
#[test]
fn test_find_best_block_valid_descendant_skips_invalid_subtree() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // c1 is validated, and x1 above it has more work.
    let c1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(1)
        .build();
    store.store_with_valid_metadata(&c1);
    let x1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(c1.block_hash().to_string())
        .nonce(2)
        .build();
    store.store_with_valid_metadata(&x1);
    let mut batch = Store::get_write_batch();
    store
        .mark_block_valid(&c1.block_hash(), &mut batch)
        .unwrap();
    store
        .mark_block_valid(&x1.block_hash(), &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    assert!(matches!(
        store
            .find_best_block_valid_descendant(&genesis.block_hash())
            .unwrap(),
        BlockValidSearch::Found(hash) if hash == x1.block_hash()
    ));

    // Invalidating c1 takes x1 out of reach even though x1 is untouched.
    let mut batch = Store::get_write_batch();
    store
        .mark_invalid(&c1.block_hash(), None, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    assert_eq!(
        store.get_block_metadata(&x1.block_hash()).unwrap().status,
        Status::BlockValid,
        "x1 itself is still BlockValid; only its ancestor was invalidated"
    );
    assert!(matches!(
        store
            .find_best_block_valid_descendant(&genesis.block_hash())
            .unwrap(),
        BlockValidSearch::NotFound { visited: 1 }
    ));
}

/// However many validated blocks sit above the confirmed tip, the search
/// returns the highest-work one, not the best of the blocks it happened to
/// reach first.
///
/// A search that stops part-way returns the same shallow block on every
/// job refresh, so every share lands on one parent as a sibling, adds no
/// work, and builds a dense height.
#[test]
fn test_find_best_block_valid_descendant_finds_tip_beyond_many_validated_siblings() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // A validated chain c1 -> c2 -> c3 above genesis.
    let c1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(1)
        .build();
    store.store_with_valid_metadata(&c1);
    let c2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(c1.block_hash().to_string())
        .nonce(2)
        .build();
    store.store_with_valid_metadata(&c2);
    let c3 = TestShareBlockBuilder::new()
        .prev_share_blockhash(c2.block_hash().to_string())
        .nonce(3)
        .build();
    store.store_with_valid_metadata(&c3);
    let mut batch = Store::get_write_batch();
    store
        .mark_block_valid(&c1.block_hash(), &mut batch)
        .unwrap();
    store
        .mark_block_valid(&c2.block_hash(), &mut batch)
        .unwrap();
    store
        .mark_block_valid(&c3.block_hash(), &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    // 1024 validated siblings of c1: with c1 that is 1025 blocks at the
    // first level, more than a 1024-block search reaches before c2.
    let mut batch = Store::get_write_batch();
    for nonce in 4..1028 {
        let sibling = TestShareBlockBuilder::new()
            .prev_share_blockhash(genesis.block_hash().to_string())
            .nonce(nonce)
            .build();
        store.store_with_valid_metadata(&sibling);
        store
            .mark_block_valid(&sibling.block_hash(), &mut batch)
            .unwrap();
    }
    store.commit_batch(batch).unwrap();

    assert!(matches!(
        store
            .find_best_block_valid_descendant(&genesis.block_hash())
            .unwrap(),
        BlockValidSearch::Found(hash) if hash == c3.block_hash()
    ));
}

/// Header-only blocks cannot hide a validated block from the search.
///
/// Validation is parent-gated, so no block below a header-only one is
/// validated and the search does not walk into it. A batch of synced
/// headers above the confirmed tip (a full response at a time) must not
/// stop the search from reaching our own validated chain.
#[test]
fn test_find_best_block_valid_descendant_skips_header_only_subtree() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Our validated chain v1 -> v2.
    let v1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(1)
        .build();
    store.store_with_valid_metadata(&v1);
    let v2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(v1.block_hash().to_string())
        .nonce(2)
        .build();
    store.store_with_valid_metadata(&v2);
    let mut batch = Store::get_write_batch();
    store
        .mark_block_valid(&v1.block_hash(), &mut batch)
        .unwrap();
    store
        .mark_block_valid(&v2.block_hash(), &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    // 1024 HeaderValid siblings of v1, never validated.
    for nonce in 3..1027 {
        let header_only = TestShareBlockBuilder::new()
            .prev_share_blockhash(genesis.block_hash().to_string())
            .nonce(nonce)
            .build();
        store.store_with_valid_metadata(&header_only);
    }

    assert!(matches!(
        store
            .find_best_block_valid_descendant(&genesis.block_hash())
            .unwrap(),
        BlockValidSearch::Found(hash) if hash == v2.block_hash()
    ));
}

/// A validated block not on the candidate chain is still a mining base: this
/// is the normal state when the candidate chain has reorged onto a peer's
/// header-only fork and our own mined blocks are the only validated ones
/// above the confirmed tip.
#[test]
fn test_find_best_block_valid_descendant_finds_off_candidate_chain_block() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // A header-only block with more work, as header sync would leave it.
    let header_only = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(1)
        .work(8)
        .build();
    store.create_valid_metadata_only(&header_only);
    let mut batch = Store::get_write_batch();
    store
        .update_block_index(&genesis.block_hash(), &header_only.block_hash(), &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    // Our own validated block, off the candidate chain and with less work.
    let mined = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(2)
        .work(1)
        .build();
    store.store_with_valid_metadata(&mined);
    let mut batch = Store::get_write_batch();
    store
        .mark_block_valid(&mined.block_hash(), &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    assert!(matches!(
        store
            .find_best_block_valid_descendant(&genesis.block_hash())
            .unwrap(),
        BlockValidSearch::Found(hash) if hash == mined.block_hash()
    ));
}

#[test]
fn test_find_uncles_excludes_nephew_read_as_child() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // Confirmed chain, with a fork at height 2:
    //
    //   genesis(0) - a(1) - b(2) --- base(3)
    //                  |        \
    //                  |         sibling(3)
    //                  |
    //                  \ b_fork(2) - p(3) - nephew(4)
    //                                          uncles b(2)
    //
    // nephew is a valid block: b is two below it and is not on its own
    // ancestry (p, b_fork, a, genesis). But b IS an ancestor of base, so
    // organising nephew's header files it under b in the block index, and
    // a share mined on base would pick it up as an uncle at its own
    // height.
    let genesis = TestShareBlockBuilder::new().nonce(0).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let a = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(1)
        .build();
    store.push_to_confirmed_chain(&a).unwrap();

    let b = TestShareBlockBuilder::new()
        .prev_share_blockhash(a.block_hash().to_string())
        .nonce(2)
        .build();
    store.push_to_confirmed_chain(&b).unwrap();

    let base = TestShareBlockBuilder::new()
        .prev_share_blockhash(b.block_hash().to_string())
        .nonce(3)
        .build();
    store.push_to_confirmed_chain(&base).unwrap();

    // Two genuine uncle candidates for a share mined on base: a sibling of
    // base, and the head of the fork branch at height 2.
    let sibling = TestShareBlockBuilder::new()
        .prev_share_blockhash(b.block_hash().to_string())
        .nonce(100)
        .build();
    store.store_with_valid_metadata(&sibling);

    let b_fork = TestShareBlockBuilder::new()
        .prev_share_blockhash(a.block_hash().to_string())
        .nonce(101)
        .build();
    store.store_with_valid_metadata(&b_fork);

    let p = TestShareBlockBuilder::new()
        .prev_share_blockhash(b_fork.block_hash().to_string())
        .nonce(102)
        .build();
    store.store_with_valid_metadata(&p);

    let nephew = TestShareBlockBuilder::new()
        .prev_share_blockhash(p.block_hash().to_string())
        .uncles(vec![b.block_hash()])
        .nonce(103)
        .build();
    store.store_with_valid_metadata(&nephew);

    // Mirror what organise_header writes for a declared uncle.
    let mut batch = Store::get_write_batch();
    store
        .update_block_index(&b.block_hash(), &nephew.block_hash(), &mut batch)
        .unwrap();
    store
        .add_to_uncles_index(&b.block_hash(), &nephew.block_hash(), &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    // The nephew is reachable as a "child" of b and passes every other
    // filter: off base's ancestry, unconfirmed, never itself used as an
    // uncle, body present.
    let children = store
        .get_children_blockhashes(&b.block_hash())
        .unwrap()
        .unwrap();
    assert!(children.contains(&nephew.block_hash()));

    let uncles = store.find_uncles(&base.block_hash()).unwrap();
    assert!(
        !uncles.contains(&nephew.block_hash()),
        "A nephew must never be selected as an uncle: it is not below the new share"
    );
    assert!(uncles.contains(&sibling.block_hash()));
    assert!(uncles.contains(&b_fork.block_hash()));
    assert_eq!(uncles.len(), 2);
}

// Regression cases ported from pool2win's upstream P2Poolv2 commit
// d14d8a00b9acffe26149bb39585881461e09c228 (MIT OR Apache-2.0).
#[test]
fn test_find_uncles_excludes_uncles_listed_by_ancestors() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // Build chain:
    //   share0 (confirmed)
    //   /    \
    // share1  uncle1 (not confirmed, listed as uncle by share2)
    // (confirmed)
    //   |    \
    // share2  uncle2 (not confirmed, available)
    // (confirmed, lists uncle1)

    let share0 = TestShareBlockBuilder::new().nonce(0).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&share0, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share0.block_hash().to_string())
        .nonce(1)
        .build();
    let uncle1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share0.block_hash().to_string())
        .nonce(100)
        .build();
    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .uncles(vec![uncle1.block_hash()])
        .nonce(2)
        .build();
    let uncle2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(101)
        .build();

    // Confirm share1 first so uncle1 and uncle2 can find their parents
    store.push_to_confirmed_chain(&share1).unwrap();
    store.store_with_valid_metadata(&uncle1);
    store.store_with_valid_metadata(&uncle2);

    // Confirm share2
    store.push_to_confirmed_chain(&share2).unwrap();

    // find_uncles should only find uncle2: share2, an ancestor of the
    // new share, already lists uncle1.
    let uncles = store.find_uncles(&store.get_chain_tip().unwrap()).unwrap();
    assert_eq!(uncles.len(), 1);
    assert!(uncles.contains(&uncle2.block_hash()));
    assert!(!uncles.contains(&uncle1.block_hash()));
}

/// An uncle listed only by a losing sibling nephew, off the base's
/// ancestry, is still available to the winning chain.
#[test]
fn test_find_uncles_includes_uncle_listed_only_off_chain() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // Build chain:
    //   share0 (confirmed)
    //   /    \
    // share1  uncle1 (not confirmed)
    // (confirmed)
    //   |    \
    // share2  losing_nephew (not confirmed, lists uncle1)
    // (confirmed)

    let share0 = TestShareBlockBuilder::new().nonce(0).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&share0, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share0.block_hash().to_string())
        .nonce(1)
        .build();
    let uncle1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share0.block_hash().to_string())
        .nonce(100)
        .build();
    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(2)
        .build();
    let losing_nephew = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .uncles(vec![uncle1.block_hash()])
        .nonce(101)
        .build();

    store.push_to_confirmed_chain(&share1).unwrap();
    store.store_with_valid_metadata(&uncle1);
    store.store_with_valid_metadata(&losing_nephew);
    store.push_to_confirmed_chain(&share2).unwrap();

    // organise_header records every listed uncle in the uncles index,
    // whichever branch the nephew is on.
    let mut batch = rocksdb::WriteBatch::default();
    store
        .add_to_uncles_index(
            &uncle1.block_hash(),
            &losing_nephew.block_hash(),
            &mut batch,
        )
        .unwrap();
    store.commit_batch(batch).unwrap();

    let uncles = store.find_uncles(&store.get_chain_tip().unwrap()).unwrap();
    assert!(uncles.contains(&uncle1.block_hash()));
    assert!(uncles.contains(&losing_nephew.block_hash()));
}

#[test]
fn test_find_uncles_excludes_confirmed_blocks() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // Build chain where all children are confirmed (no uncles)
    //   share0 (confirmed)
    //   /
    // share1 (confirmed)
    //   |
    // share2 (confirmed)

    let share0 = TestShareBlockBuilder::new().nonce(0).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&share0, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share0.block_hash().to_string())
        .nonce(1)
        .build();
    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(2)
        .build();

    // Confirm all using push_to_confirmed_chain
    store.push_to_confirmed_chain(&share1).unwrap();
    store.push_to_confirmed_chain(&share2).unwrap();

    // find_uncles should return empty - share1 is child of share0 but is confirmed
    let uncles = store.find_uncles(&store.get_chain_tip().unwrap()).unwrap();
    assert!(uncles.is_empty());
}

#[test]
fn test_find_uncles_returns_max_3_uncles_by_chain_work() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // Build chain with 4 uncles - should only return top 3 by chain_work
    //   share0 (confirmed, height 0)
    //   / | \ \
    // share1 uncle_a uncle_b uncle_c (height 1)
    // (confirmed)
    //   |    \
    // share2  uncle_d (height 2)
    // (confirmed)

    let share0 = TestShareBlockBuilder::new().nonce(0).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&share0, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share0.block_hash().to_string())
        .nonce(1)
        .build();
    let uncle_a = TestShareBlockBuilder::new()
        .prev_share_blockhash(share0.block_hash().to_string())
        .nonce(100)
        .build();
    let uncle_b = TestShareBlockBuilder::new()
        .prev_share_blockhash(share0.block_hash().to_string())
        .nonce(101)
        .build();
    let uncle_c = TestShareBlockBuilder::new()
        .prev_share_blockhash(share0.block_hash().to_string())
        .nonce(102)
        .build();
    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(2)
        .build();
    let uncle_d = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .work(2)
        .nonce(103)
        .build();

    // Store uncle shares with Valid metadata so find_uncles can read their metadata
    store.store_with_valid_metadata(&uncle_a);
    store.store_with_valid_metadata(&uncle_b);
    store.store_with_valid_metadata(&uncle_c);

    // Confirm share1 first, so uncle_d's parent has metadata
    store.push_to_confirmed_chain(&share1).unwrap();

    // Now store uncle_d (its parent share1 now has metadata)
    store.store_with_valid_metadata(&uncle_d);

    // Confirm share2
    store.push_to_confirmed_chain(&share2).unwrap();

    // find_uncles should return exactly 3 uncles, prioritizing higher chain_work
    // uncle_d has work=2, uncle_a/b/c have default work=1
    let uncles = store.find_uncles(&store.get_chain_tip().unwrap()).unwrap();

    // uncle_d leads on chain_work. uncle_a/b/c tie behind it and only two
    // of the three fit, so the blockhash tiebreak decides both which pair
    // is kept and the order it is kept in.
    let mut tied = [
        uncle_a.block_hash(),
        uncle_b.block_hash(),
        uncle_c.block_hash(),
    ];
    tied.sort();
    assert_eq!(uncles, vec![uncle_d.block_hash(), tied[0], tied[1]]);
}

/// Uncles of equal chain_work are ordered by blockhash ascending.
#[test]
fn test_find_uncles_orders_equal_work_uncles_by_blockhash() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // share0 (confirmed, height 0)
    //   /   |     |       \
    // share1 uncle_a uncle_b uncle_c (height 1, all default work)
    // (confirmed)
    let share0 = TestShareBlockBuilder::new().nonce(0).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&share0, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share0.block_hash().to_string())
        .nonce(1)
        .build();
    let uncle_a = TestShareBlockBuilder::new()
        .prev_share_blockhash(share0.block_hash().to_string())
        .nonce(100)
        .build();
    let uncle_b = TestShareBlockBuilder::new()
        .prev_share_blockhash(share0.block_hash().to_string())
        .nonce(101)
        .build();
    let uncle_c = TestShareBlockBuilder::new()
        .prev_share_blockhash(share0.block_hash().to_string())
        .nonce(102)
        .build();

    store.store_with_valid_metadata(&uncle_a);
    store.store_with_valid_metadata(&uncle_b);
    store.store_with_valid_metadata(&uncle_c);
    store.push_to_confirmed_chain(&share1).unwrap();

    let uncles = store.find_uncles(&share1.block_hash()).unwrap();

    // Same parent and same work, so chain_work ties and only the blockhash
    // separates them.
    let mut expected = vec![
        uncle_a.block_hash(),
        uncle_b.block_hash(),
        uncle_c.block_hash(),
    ];
    expected.sort();
    assert_eq!(uncles, expected);
}

/// When more equal-work candidates exist than MAX_UNCLES, the blockhash
/// order decides which are kept, not just how the kept ones are arranged.
/// Without the tiebreak this selection differs between nodes.
#[test]
fn test_find_uncles_keeps_lowest_blockhashes_when_equal_work_exceeds_max() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // share0 (confirmed, height 0)
    //   /   |      |      |      \
    // share1 uncle_a uncle_b uncle_c uncle_d (height 1, all default work)
    // (confirmed)
    let share0 = TestShareBlockBuilder::new().nonce(0).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&share0, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share0.block_hash().to_string())
        .nonce(1)
        .build();
    let uncle_a = TestShareBlockBuilder::new()
        .prev_share_blockhash(share0.block_hash().to_string())
        .nonce(100)
        .build();
    let uncle_b = TestShareBlockBuilder::new()
        .prev_share_blockhash(share0.block_hash().to_string())
        .nonce(101)
        .build();
    let uncle_c = TestShareBlockBuilder::new()
        .prev_share_blockhash(share0.block_hash().to_string())
        .nonce(102)
        .build();
    let uncle_d = TestShareBlockBuilder::new()
        .prev_share_blockhash(share0.block_hash().to_string())
        .nonce(103)
        .build();

    store.store_with_valid_metadata(&uncle_a);
    store.store_with_valid_metadata(&uncle_b);
    store.store_with_valid_metadata(&uncle_c);
    store.store_with_valid_metadata(&uncle_d);
    store.push_to_confirmed_chain(&share1).unwrap();

    let uncles = store.find_uncles(&share1.block_hash()).unwrap();

    let mut candidates = [
        uncle_a.block_hash(),
        uncle_b.block_hash(),
        uncle_c.block_hash(),
        uncle_d.block_hash(),
    ];
    candidates.sort();
    assert_eq!(uncles.as_slice(), &candidates[..MAX_UNCLES]);
}

#[test]
fn test_find_uncles_with_deep_chain_only_looks_at_last_3_heights() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // Build a chain of 7 blocks (heights 0-6)
    // with uncles at heights 1, 2, 3, 4, 5
    // find_uncles from height 6 should only look at heights 3, 4, 5 (last 3)

    let share0 = TestShareBlockBuilder::new().nonce(0).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&share0, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share0.block_hash().to_string())
        .nonce(1)
        .build();
    let uncle1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share0.block_hash().to_string())
        .nonce(101)
        .build();
    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(2)
        .build();
    let uncle2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(102)
        .build();
    let share3 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share2.block_hash().to_string())
        .nonce(3)
        .build();
    let uncle3 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share2.block_hash().to_string())
        .nonce(103)
        .build();
    let share4 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share3.block_hash().to_string())
        .nonce(4)
        .build();
    let uncle4 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share3.block_hash().to_string())
        .nonce(104)
        .build();
    let share5 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share4.block_hash().to_string())
        .nonce(5)
        .build();
    let uncle5 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share4.block_hash().to_string())
        .work(2)
        .nonce(105)
        .build();
    let share6 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share5.block_hash().to_string())
        .nonce(6)
        .build();

    // Store uncle shares with Valid metadata so find_uncles can discover them.
    // Uncle parents must be confirmed first so metadata chain_work is correct.
    store.store_with_valid_metadata(&uncle1);
    store.push_to_confirmed_chain(&share1).unwrap();

    store.store_with_valid_metadata(&uncle2);
    store.push_to_confirmed_chain(&share2).unwrap();

    store.store_with_valid_metadata(&uncle3);
    store.push_to_confirmed_chain(&share3).unwrap();

    store.store_with_valid_metadata(&uncle4);
    store.push_to_confirmed_chain(&share4).unwrap();

    store.store_with_valid_metadata(&uncle5);
    store.push_to_confirmed_chain(&share5).unwrap();

    store.push_to_confirmed_chain(&share6).unwrap();

    // find_uncles from share6 (height 6) looks at confirmed blocks at heights 3, 4, 5
    // and finds their non-confirmed children.
    // - share3 (height 3) has children: share4, uncle4 -> uncle4 found
    // - share4 (height 4) has children: share5, uncle5 -> uncle5 found
    // - share5 (height 5) has children: share6 only -> no uncles
    // uncle3 is NOT found because it's a child of share2 (height 2), which is outside the range
    let uncles = store.find_uncles(&store.get_chain_tip().unwrap()).unwrap();

    assert_eq!(uncles.len(), 2);
    // Should be sorted by chain_work descending: uncle5 (work=2), uncle4 (work=1)
    assert_eq!(uncles[0], uncle5.block_hash());
    assert_eq!(uncles[1], uncle4.block_hash());

    // Verify uncle1, uncle2, and uncle3 are NOT included (parents outside depth range)
    assert!(!uncles.contains(&uncle1.block_hash()));
    assert!(!uncles.contains(&uncle2.block_hash()));
    assert!(!uncles.contains(&uncle3.block_hash()));
}

#[test]
fn test_find_uncles_anchors_on_leading_base_not_confirmed_tip() {
    // Regression for the mining-base / uncle-altitude mismatch: when the
    // mining base leads the confirmed tip on a BlockValid fork, uncles must
    // be selected relative to the base (the nephew's parent), not the
    // confirmed tip. Otherwise find_uncles returns blocks too deep for the
    // nephew and the node's own share fails the uncle-depth check.
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // Confirmed chain g(0) -> c1(1) -> c2(2) -> c3(3).
    let g = TestShareBlockBuilder::new().nonce(0).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&g, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let c1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(g.block_hash().to_string())
        .nonce(1)
        .build();
    // A sibling of c2 (child of c1) at height 2. Depth 4 from a nephew built
    // on f5, so it must be excluded -- but confirmed-tip anchoring (tip c3)
    // would have selected it.
    let deep_uncle = TestShareBlockBuilder::new()
        .prev_share_blockhash(c1.block_hash().to_string())
        .nonce(102)
        .build();
    let c2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(c1.block_hash().to_string())
        .nonce(2)
        .build();
    let c3 = TestShareBlockBuilder::new()
        .prev_share_blockhash(c2.block_hash().to_string())
        .nonce(3)
        .build();

    store.push_to_confirmed_chain(&c1).unwrap();
    store.store_with_valid_metadata(&deep_uncle);
    store.push_to_confirmed_chain(&c2).unwrap();
    store.push_to_confirmed_chain(&c3).unwrap();

    // BlockValid fork leading the confirmed tip by 2: c3 -> f4(4) -> f5(5).
    let f4 = TestShareBlockBuilder::new()
        .prev_share_blockhash(c3.block_hash().to_string())
        .nonce(4)
        .build();
    // A sibling of f4 (child of c3) at height 4. Depth 2 from a nephew built
    // on f5, so it is a valid uncle.
    let near_uncle = TestShareBlockBuilder::new()
        .prev_share_blockhash(c3.block_hash().to_string())
        .nonce(104)
        .build();
    let f5 = TestShareBlockBuilder::new()
        .prev_share_blockhash(f4.block_hash().to_string())
        .nonce(5)
        .build();
    store.store_with_valid_metadata(&f4);
    store.store_with_valid_metadata(&near_uncle);
    store.store_with_valid_metadata(&f5);

    // Anchored on the leading base f5 (the nephew would be at height 6).
    let uncles = store.find_uncles(&f5.block_hash()).unwrap();

    // The near sibling (depth 2) is a valid uncle.
    assert!(
        uncles.contains(&near_uncle.block_hash()),
        "near sibling within MAX_UNCLES_DEPTH must be a uncle"
    );
    // The deep sibling (depth 4) is outside the base's window and must be
    // excluded; confirmed-tip anchoring would have wrongly included it.
    assert!(
        !uncles.contains(&deep_uncle.block_hash()),
        "sibling deeper than MAX_UNCLES_DEPTH from the base must be excluded"
    );
}

/// Higher chain_work uncles at a lower height should be preferred
/// over lower chain_work uncles at a higher height.
///
///   share0 (confirmed, height 0)
///     /        |           \
///   share1     uncle_mid_a  uncle_mid_b
///   (confirmed, h:1)  (h:1, work=2)  (h:1, work=2)
///     /    |         \
///   share2  uncle_high  uncle_low
///   (confirmed, h:2)  (h:2, work=3)  (h:2, work=1)
///
/// Result: uncle_high(work=3), uncle_mid_a(work=2), uncle_mid_b(work=2)
/// uncle_low(work=1) is excluded despite being at higher height
#[test]
fn test_find_uncles_prefers_higher_chain_work_over_height() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let share0 = TestShareBlockBuilder::new().nonce(0).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&share0, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share0.block_hash().to_string())
        .nonce(1)
        .build();
    let uncle_mid_a = TestShareBlockBuilder::new()
        .prev_share_blockhash(share0.block_hash().to_string())
        .work(2)
        .nonce(100)
        .build();
    let uncle_mid_b = TestShareBlockBuilder::new()
        .prev_share_blockhash(share0.block_hash().to_string())
        .work(2)
        .nonce(101)
        .build();
    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(2)
        .build();
    let uncle_high = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .work(3)
        .nonce(200)
        .build();
    let uncle_low = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(201)
        .build();

    // Store uncle shares at height 1 with Valid metadata (parent share0 has metadata from genesis)
    store.store_with_valid_metadata(&uncle_mid_a);
    store.store_with_valid_metadata(&uncle_mid_b);

    // Confirm share1 so uncles at height 2 can compute their metadata
    store.push_to_confirmed_chain(&share1).unwrap();

    // Store uncle shares at height 2 with Valid metadata
    store.store_with_valid_metadata(&uncle_high);
    store.store_with_valid_metadata(&uncle_low);

    // Confirm share2
    store.push_to_confirmed_chain(&share2).unwrap();

    let uncles = store.find_uncles(&store.get_chain_tip().unwrap()).unwrap();

    // uncle_high (work=3) comes first, then uncle_mid_a and uncle_mid_b
    // (both work=2) in blockhash order.
    let mut tied = [uncle_mid_a.block_hash(), uncle_mid_b.block_hash()];
    tied.sort();
    assert_eq!(uncles, vec![uncle_high.block_hash(), tied[0], tied[1]]);

    // uncle_low at height 2 is excluded despite being at higher height than the mids
    assert!(!uncles.contains(&uncle_low.block_hash()));
}

/// The chain tip (parent of the next share) must never appear as
/// an uncle even if it is a non-confirmed child of a confirmed
/// ancestor within the depth window.
///
/// Scenario: genesis -> share1(h:1) -> share2(h:2, chain tip)
/// with fork_uncle as a sibling of share2 (also child of share1).
/// find_uncles should return fork_uncle but never the chain tip.
#[test]
fn test_find_uncles_excludes_chain_tip() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(1)
        .build();
    store.push_to_confirmed_chain(&share1).unwrap();

    // fork_uncle: sibling of share2, child of share1
    let fork_uncle = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(99)
        .build();
    store.store_with_valid_metadata(&fork_uncle);

    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .nonce(2)
        .build();
    store.push_to_confirmed_chain(&share2).unwrap();

    // Verify chain tip is share2
    assert_eq!(store.get_chain_tip().unwrap(), share2.block_hash());

    let uncles = store.find_uncles(&store.get_chain_tip().unwrap()).unwrap();

    // fork_uncle should be found
    assert_eq!(uncles.len(), 1);
    assert_eq!(uncles[0], fork_uncle.block_hash());

    // chain tip must never appear as an uncle
    assert!(
        !uncles.contains(&share2.block_hash()),
        "chain tip must never appear as an uncle"
    );
}

#[test]
fn test_share_info_serialization() {
    let share_info = ShareInfo {
        blockhash: BlockHash::all_zeros(),
        prev_blockhash: BlockHash::all_zeros(),
        height: 42,
        miner_bitcoin_address: parse_address_from_string(
            "tb1q4axuxtvt0q6x4r7g8qjqmzfhkkw4tjgvjrxe7q",
        ),
        miner_address: make_test_share_program(1).into(),
        timestamp: 1_700_000_000,
        bits: CompactTarget::from_consensus(0x1b4188f5),
        uncles: vec![],
    };

    let json = serde_json::to_string(&share_info).unwrap();
    assert!(json.contains("\"height\":42"));
    assert!(
        json.contains("\"miner_bitcoin_address\":\"tb1q4axuxtvt0q6x4r7g8qjqmzfhkkw4tjgvjrxe7q\"")
    );
    assert!(json.contains(&format!(
        "\"miner_address\":\"{}\"",
        to_hex(&make_test_share_program(1))
    )));
    assert!(json.contains("\"timestamp\":1700000000"));
}

#[test]
fn test_share_info_with_uncles_serialization() {
    let uncle = UncleInfo {
        blockhash: BlockHash::all_zeros(),
        prev_blockhash: BlockHash::all_zeros(),
        miner_bitcoin_address: parse_address_from_string(
            "tb1qyazxde6558qj6z3d9np5e6msmrspwpf6k0qggk",
        ),
        miner_address: make_test_share_program(2).into(),
        timestamp: 1_700_000_010,
        height: Some(41),
    };

    let share_info = ShareInfo {
        blockhash: BlockHash::all_zeros(),
        prev_blockhash: BlockHash::all_zeros(),
        height: 42,
        miner_bitcoin_address: parse_address_from_string(
            "tb1q4axuxtvt0q6x4r7g8qjqmzfhkkw4tjgvjrxe7q",
        ),
        miner_address: make_test_share_program(1).into(),
        timestamp: 1_700_000_020,
        bits: CompactTarget::from_consensus(0x1b4188f5),
        uncles: vec![uncle],
    };

    let json = serde_json::to_string(&share_info).unwrap();
    assert!(json.contains(&format!("\"{}\"", to_hex(&make_test_share_program(2)))));
    assert!(json.contains("\"height\":41"));
}

#[test]
fn test_uncle_info_serialization() {
    let uncle = UncleInfo {
        blockhash: BlockHash::all_zeros(),
        prev_blockhash: BlockHash::all_zeros(),
        miner_bitcoin_address: parse_address_from_string(
            "tb1q4axuxtvt0q6x4r7g8qjqmzfhkkw4tjgvjrxe7q",
        ),
        miner_address: make_test_share_program(2).into(),
        timestamp: 1_700_000_005,
        height: Some(10),
    };

    let json = serde_json::to_string(&uncle).unwrap();
    assert!(
        json.contains("\"miner_bitcoin_address\":\"tb1q4axuxtvt0q6x4r7g8qjqmzfhkkw4tjgvjrxe7q\"")
    );
    assert!(json.contains(&format!(
        "\"miner_address\":\"{}\"",
        to_hex(&make_test_share_program(2))
    )));
    assert!(json.contains("\"timestamp\":1700000005"));
    assert!(json.contains("\"height\":10"));
}

#[test]
fn test_uncle_info_with_no_height_serialization() {
    let uncle = UncleInfo {
        blockhash: BlockHash::all_zeros(),
        prev_blockhash: BlockHash::all_zeros(),
        miner_bitcoin_address: parse_address_from_string(
            "tb1qyazxde6558qj6z3d9np5e6msmrspwpf6k0qggk",
        ),
        miner_address: make_test_share_program(2).into(),
        timestamp: 1_700_000_005,
        height: None,
    };

    let json = serde_json::to_string(&uncle).unwrap();
    assert!(json.contains("\"height\":null"));
}

#[test]
fn test_query_share_blocks_returns_blocks_in_order() {
    use crate::store::block_tx_metadata::{BlockMetadata, ChainMembership, Status};

    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let share0 = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let share1 = TestShareBlockBuilder::new().nonce(0xe9695792).build();
    let share2 = TestShareBlockBuilder::new().nonce(0xe9695793).build();

    let shares = [&share0, &share1, &share2];
    for (height, share) in shares.iter().enumerate() {
        let mut batch = Store::get_write_batch();
        store.add_share_block(share, &mut batch).unwrap();
        let mut metadata = BlockMetadata {
            expected_height: Some(height as u32),
            chain_work: share.header.get_work(),
            status: Status::HeaderValid,
            chain: ChainMembership::None,
        };
        store
            .update_block_metadata(&share.block_hash(), &metadata, &mut batch)
            .unwrap();
        store
            .append_to_confirmed(
                &share.block_hash(),
                height as u32,
                &mut metadata,
                &mut batch,
            )
            .unwrap();
        store.commit_batch(batch).unwrap();
    }

    let result = store.query_share_blocks(0, 2).unwrap();
    assert_eq!(result.len(), 3);
    assert_eq!(result[0].block_hash(), share0.block_hash());
    assert_eq!(result[1].block_hash(), share1.block_hash());
    assert_eq!(result[2].block_hash(), share2.block_hash());
}

#[test]
fn test_query_share_blocks_returns_subset() {
    use crate::store::block_tx_metadata::{BlockMetadata, ChainMembership, Status};

    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let share0 = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let share1 = TestShareBlockBuilder::new().nonce(0xe9695792).build();
    let share2 = TestShareBlockBuilder::new().nonce(0xe9695793).build();

    let shares = [&share0, &share1, &share2];
    for (height, share) in shares.iter().enumerate() {
        let mut batch = Store::get_write_batch();
        store.add_share_block(share, &mut batch).unwrap();
        let mut metadata = BlockMetadata {
            expected_height: Some(height as u32),
            chain_work: share.header.get_work(),
            status: Status::HeaderValid,
            chain: ChainMembership::None,
        };
        store
            .update_block_metadata(&share.block_hash(), &metadata, &mut batch)
            .unwrap();
        store
            .append_to_confirmed(
                &share.block_hash(),
                height as u32,
                &mut metadata,
                &mut batch,
            )
            .unwrap();
        store.commit_batch(batch).unwrap();
    }

    let result = store.query_share_blocks(1, 2).unwrap();
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].block_hash(), share1.block_hash());
    assert_eq!(result[1].block_hash(), share2.block_hash());
}

#[test]
fn test_query_share_blocks_no_confirmed_shares() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let result = store.query_share_blocks(0, 0).unwrap();
    assert!(result.is_empty());
}

/// Locator containing a non-confirmed block (uncle) matches it.
/// With height-based walking, any valid block in the store is a
/// valid locator match. The uncle at h:1 matches, so descendants
/// start from h:2.
///
/// Chain:
///   genesis(h:0) -> share_a(h:1) -> share_b(h:2)
///                \-> uncle(h:1, HeaderValid)
///
/// Locator: [uncle_hash]
/// Expected: matches uncle at h:1, returns share_b at h:2.
#[test]
fn test_get_blockhashes_for_locator_matches_non_confirmed_block() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // uncle: child of genesis, stored as HeaderValid
    let uncle = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(100)
        .build();
    store.store_with_valid_metadata(&uncle);

    let share_a = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .work(2)
        .nonce(1)
        .build();
    store.push_to_confirmed_chain(&share_a).unwrap();

    let share_b = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_a.block_hash().to_string())
        .work(2)
        .nonce(2)
        .build();
    store.push_to_confirmed_chain(&share_b).unwrap();

    // Locator with only the uncle hash -- uncle is HeaderValid at
    // h:1, so it matches. With MAX_UNCLES_DEPTH overlap, start
    // height is max(1-3,0)+1 = 1. Returns all blocks at h:1
    // (uncle, share_a) and h:2 (share_b).
    let locator = vec![uncle.block_hash()];
    let result = store
        .get_blockhashes_for_locator(&locator, &BlockHash::all_zeros(), 10)
        .unwrap();

    assert_eq!(result.len(), 3);
    assert!(result.contains(&uncle.block_hash()));
    assert!(result.contains(&share_a.block_hash()));
    assert!(result.contains(&share_b.block_hash()));
}

/// first_known_for_locator returns the first known hash from the
/// locator, matching any valid status (HeaderValid, Candidate,
/// Confirmed, BlockValid). Only unknown and invalid hashes are
/// skipped.
///
/// Chain:
///   genesis(h:0) -> share_a(h:1) -> share_b(h:2)
///                \-> uncle(h:1, HeaderValid)
#[test]
fn test_first_known_for_locator_matches_any_valid_status() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let uncle = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(100)
        .build();
    store.store_with_valid_metadata(&uncle);

    let share_a = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .work(2)
        .nonce(1)
        .build();
    store.push_to_confirmed_chain(&share_a).unwrap();

    let share_b = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_a.block_hash().to_string())
        .work(2)
        .nonce(2)
        .build();
    store.push_to_confirmed_chain(&share_b).unwrap();

    let unknown_hash = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        .parse::<BlockHash>()
        .unwrap();

    // Locator: [unknown, uncle, share_b, genesis]
    // Skips unknown (not in store), matches uncle (HeaderValid).
    let locator = vec![
        unknown_hash,
        uncle.block_hash(),
        share_b.block_hash(),
        genesis.block_hash(),
    ];
    let result = store.first_known_for_locator(&locator).unwrap();
    assert_eq!(result, Some(uncle.block_hash()));

    // Locator with only unknown entries returns None
    let locator = vec![unknown_hash];
    let result = store.first_known_for_locator(&locator).unwrap();
    assert_eq!(result, None);

    // Empty locator returns None
    let result = store.first_known_for_locator(&[]).unwrap();
    assert_eq!(result, None);

    // Locator with HeaderValid entry matches it
    let locator = vec![uncle.block_hash(), genesis.block_hash()];
    let result = store.first_known_for_locator(&locator).unwrap();
    assert_eq!(result, Some(uncle.block_hash()));

    // Locator with only confirmed entries returns the first one
    let locator = vec![share_a.block_hash(), genesis.block_hash()];
    let result = store.first_known_for_locator(&locator).unwrap();
    assert_eq!(result, Some(share_a.block_hash()));
}

// --- query_dag tests ---

#[test]
fn test_query_dag_returns_genesis() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let entries = store.query_dag(0, 0);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].blockhash, genesis.block_hash());
    assert_eq!(entries[0].height, 0);
    assert_eq!(entries[0].validation_status, "HeaderValid");
    assert_eq!(entries[0].chain, "Confirmed");
    assert!(entries[0].has_block_data);
}

#[test]
fn test_query_dag_returns_multiple_blocks_at_same_height() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Two blocks at height 1 with same parent (genesis)
    let share_a = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    store.push_to_confirmed_chain(&share_a).unwrap();

    let share_b = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695793)
        .build();

    // Store block + organise header so it appears in height index
    let mut batch = Store::get_write_batch();
    store.add_share_block(&share_b, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();
    let mut batch = Store::get_write_batch();
    store.organise_header(&share_b.header, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let entries = store.query_dag(1, 1);
    assert_eq!(entries.len(), 2);

    let hashes: Vec<BlockHash> = entries.iter().map(|e| e.blockhash).collect();
    assert!(hashes.contains(&share_a.block_hash()));
    assert!(hashes.contains(&share_b.block_hash()));
}

#[test]
fn test_query_dag_returns_empty_for_unpopulated_heights() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Query heights above the chain -- should return empty
    let entries = store.query_dag(5, 10);
    assert!(entries.is_empty());
}

#[test]
fn test_query_dag_shows_has_block_data_false_for_header_only() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Organise header only (no block data stored)
    let share = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    let mut batch = Store::get_write_batch();
    store.organise_header(&share.header, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let entries = store.query_dag(1, 1);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].blockhash, share.block_hash());
    assert!(!entries[0].has_block_data);
}
