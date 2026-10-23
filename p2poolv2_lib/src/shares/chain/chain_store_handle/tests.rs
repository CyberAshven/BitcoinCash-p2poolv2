use crate::pool_difficulty::AsertParent;
use crate::store::Store;
use crate::store::writer::StoreError;
use crate::test_utils::{
    TestShareBlockBuilder, build_confirmed_test_chain, genesis_for_tests,
    setup_test_chain_store_handle,
};
use bitcoin::BlockHash;
use bitcoin::hashes::Hash;

/// A store already holding another share chain must be rejected, not
/// quietly given a second genesis. This is the stale-store case an
/// operator hits by upgrading across a chain reset without deleting the
/// store directory.
///
/// The restart is simulated by calling init again with a different
/// genesis: the in-memory genesis blockhash a real restart clears does
/// not gate this path, so the branch taken is the same one.
#[tokio::test]
async fn test_init_or_setup_genesis_rejects_store_built_on_another_chain() {
    let (chain_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let genesis = genesis_for_tests();
    chain_handle
        .init_or_setup_genesis(genesis.clone())
        .await
        .unwrap();

    let other_genesis = TestShareBlockBuilder::new().nonce(0xe9695792).build();
    assert_ne!(other_genesis.block_hash(), genesis.block_hash());

    let error = chain_handle
        .init_or_setup_genesis(other_genesis)
        .await
        .expect_err("store on another chain must be rejected");

    assert!(
        matches!(error, StoreError::GenesisMismatch(_)),
        "expected GenesisMismatch, got {error:?}"
    );
    assert!(
        error
            .to_string()
            .contains(&genesis.block_hash().to_string()),
        "error must name the genesis already in the store: {error}"
    );
}

#[tokio::test]
async fn test_chain_store_handle_creation() {
    let (chain_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    assert_eq!(chain_handle.network(), bitcoin::Network::Signet);
}

/// Genesis has no parent, so as an ASERT parent its previous time is
/// its own time.
#[tokio::test]
async fn test_get_asert_parent_of_genesis_uses_its_own_time() {
    let (chain_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let genesis = genesis_for_tests();
    chain_handle
        .init_or_setup_genesis(genesis.clone())
        .await
        .unwrap();

    let parent = chain_handle
        .get_asert_parent(&genesis.block_hash())
        .unwrap();

    assert_eq!(
        parent,
        AsertParent {
            height: 0,
            time: genesis.header.time,
            bits: genesis.header.bits,
            previous_time: genesis.header.time,
        }
    );
}

/// A share's ASERT parent carries its own height, time and bits and
/// its parent's time, the interval the Bitcoin Cash rule reads.
#[tokio::test]
async fn test_get_asert_parent_reads_the_share_and_its_parent_time() {
    let (chain_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let genesis = genesis_for_tests();
    chain_handle
        .init_or_setup_genesis(genesis.clone())
        .await
        .unwrap();
    let share = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .time(genesis.header.time + 7)
        .build();
    chain_handle
        .store_handle()
        .store()
        .store_with_valid_metadata(&share);

    let parent = chain_handle.get_asert_parent(&share.block_hash()).unwrap();

    assert_eq!(
        parent,
        AsertParent {
            height: 1,
            time: genesis.header.time + 7,
            bits: share.header.bits,
            previous_time: genesis.header.time,
        }
    );
}

#[tokio::test]
async fn test_get_mining_base_defaults_to_confirmed_tip() {
    let (chain_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let genesis = genesis_for_tests();
    chain_handle
        .init_or_setup_genesis(genesis.clone())
        .await
        .unwrap();

    // No BlockValid block yet -> mining base is the confirmed (genesis) tip.
    assert_eq!(
        chain_handle.get_mining_base().unwrap(),
        genesis.block_hash()
    );
}

#[tokio::test]
async fn test_get_mining_base_prefers_higher_work_block_valid() {
    let (chain_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let genesis = genesis_for_tests();
    chain_handle
        .init_or_setup_genesis(genesis.clone())
        .await
        .unwrap();

    // A real block validated on top of genesis out-works the confirmed
    // (genesis) tip, so it becomes the mining base.
    let store = chain_handle.store_handle().store();
    let block = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .work(2)
        .build();
    store.store_with_valid_metadata(&block);
    let mut batch = Store::get_write_batch();
    store
        .mark_block_valid(&block.block_hash(), &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    assert_eq!(chain_handle.get_mining_base().unwrap(), block.block_hash());
}

/// A validated block that forks off below the confirmed tip is never the
/// mining base, however much work it carries: the base must descend from
/// the confirmed tip. Adopting a heavier branch is `reorg_confirmed`'s job,
/// not the mining base's.
#[tokio::test]
async fn test_get_mining_base_ignores_validated_sibling_of_confirmed_tip() {
    let (chain_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let genesis = genesis_for_tests();
    chain_handle
        .init_or_setup_genesis(genesis.clone())
        .await
        .unwrap();

    let store = chain_handle.store_handle().store();
    let confirmed = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .work(2)
        .build();
    store.push_to_confirmed_chain(&confirmed).unwrap();

    // A validated sibling of the confirmed tip, with far more work.
    let sibling = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695799)
        .work(8)
        .build();
    store.store_with_valid_metadata(&sibling);
    let mut batch = Store::get_write_batch();
    store
        .mark_block_valid(&sibling.block_hash(), &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    assert_eq!(
        chain_handle.get_mining_base().unwrap(),
        confirmed.block_hash(),
        "a sibling of the confirmed tip is not a descendant of it"
    );
}

/// Two validated descendants of the confirmed tip with equal cumulative
/// work: the lexicographically smaller hash wins, so every node picks the
/// same base.
#[tokio::test]
async fn test_get_mining_base_tiebreak_smallest_hash_on_equal_work() {
    let (chain_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let genesis = genesis_for_tests();
    chain_handle
        .init_or_setup_genesis(genesis.clone())
        .await
        .unwrap();

    let store = chain_handle.store_handle().store();
    // Both build on the confirmed (genesis) tip with the same work, so
    // they tie on cumulative work.
    let child_a = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .work(3)
        .build();
    let child_b = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695793)
        .work(3)
        .build();
    store.store_with_valid_metadata(&child_a);
    store.store_with_valid_metadata(&child_b);
    let mut batch = Store::get_write_batch();
    store
        .mark_block_valid(&child_a.block_hash(), &mut batch)
        .unwrap();
    store
        .mark_block_valid(&child_b.block_hash(), &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let expected = std::cmp::min(child_a.block_hash(), child_b.block_hash());
    let other = std::cmp::max(child_a.block_hash(), child_b.block_hash());
    assert_ne!(expected, other, "fixture must produce two distinct hashes");
    assert_eq!(chain_handle.get_mining_base().unwrap(), expected);
}

/// Blocks sit above the confirmed tip but none is validated yet -- the
/// state header sync leaves behind. The base is the confirmed tip, and
/// get_mining_base warns because confirmation is not keeping up.
#[tokio::test]
async fn test_get_mining_base_returns_confirmed_tip_when_nothing_above_is_validated() {
    let (chain_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let genesis = genesis_for_tests();
    chain_handle
        .init_or_setup_genesis(genesis.clone())
        .await
        .unwrap();

    let store = chain_handle.store_handle().store();
    // Header-only, as header sync stores it: metadata but no BlockValid.
    let header_only = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .work(5)
        .build();
    store.create_valid_metadata_only(&header_only);
    let mut batch = Store::get_write_batch();
    store
        .update_block_index(&genesis.block_hash(), &header_only.block_hash(), &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    assert_eq!(
        chain_handle.get_mining_base().unwrap(),
        genesis.block_hash()
    );
}

#[tokio::test]
async fn test_get_mining_base_and_uncles_excludes_base() {
    let (chain_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let genesis = genesis_for_tests();
    chain_handle
        .init_or_setup_genesis(genesis.clone())
        .await
        .unwrap();

    // With only genesis, the mining base is genesis and it is never
    // listed as an uncle (its own-ancestry is excluded).
    let (base, uncles) = chain_handle.get_mining_base_and_uncles().unwrap();
    assert_eq!(base, genesis.block_hash());
    assert!(!uncles.contains(&genesis.block_hash()));
    assert!(uncles.is_empty());
}

#[tokio::test]
async fn test_get_mining_base_and_uncles_returns_sibling_uncle() {
    let (chain_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let genesis = genesis_for_tests();
    chain_handle
        .init_or_setup_genesis(genesis.clone())
        .await
        .unwrap();

    let store = chain_handle.store_handle().store();
    // Confirm a block at height 1.
    let confirmed = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .work(2)
        .build();
    store.push_to_confirmed_chain(&confirmed).unwrap();

    // A sibling of the confirmed block (also a child of genesis) with its
    // body stored is a valid uncle candidate.
    let sibling = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695793)
        .work(1)
        .build();
    store.store_with_valid_metadata(&sibling);

    // With no BlockValid block, the mining base is the confirmed tip. The
    // sibling is returned as an uncle, and the mining base is never listed
    // among the uncles.
    let (base, uncles) = chain_handle.get_mining_base_and_uncles().unwrap();
    assert_eq!(base, confirmed.block_hash());
    assert!(uncles.contains(&sibling.block_hash()));
    assert!(!uncles.contains(&confirmed.block_hash()));
}

#[tokio::test]
async fn test_get_mining_base_and_uncles_excludes_fork_ancestry() {
    let (chain_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let genesis = genesis_for_tests();
    chain_handle
        .init_or_setup_genesis(genesis.clone())
        .await
        .unwrap();

    let store = chain_handle.store_handle().store();
    // Confirm a block at height 1.
    let confirmed = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .work(2)
        .build();
    store.push_to_confirmed_chain(&confirmed).unwrap();

    // A BlockValid block above the confirmed tip, off the candidate chain,
    // becomes the mining base.
    let fork = TestShareBlockBuilder::new()
        .prev_share_blockhash(confirmed.block_hash().to_string())
        .nonce(0xe9695793)
        .work(5)
        .build();
    store.store_with_valid_metadata(&fork);
    let mut batch = Store::get_write_batch();
    store
        .mark_block_valid(&fork.block_hash(), &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    // Another sibling off genesis is a genuine uncle, outside the fork's
    // ancestry.
    let sibling = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695794)
        .work(1)
        .build();
    store.store_with_valid_metadata(&sibling);

    let (base, uncles) = chain_handle.get_mining_base_and_uncles().unwrap();
    // The mining base is the validated block above the confirmed tip; it
    // and its ancestry (down to genesis) are excluded from the uncle set.
    assert_eq!(base, fork.block_hash());
    assert!(!uncles.contains(&fork.block_hash()));
    assert!(!uncles.contains(&genesis.block_hash()));
    // The confirmed tip is never an uncle, but the unrelated sibling is.
    assert!(!uncles.contains(&confirmed.block_hash()));
    assert!(uncles.contains(&sibling.block_hash()));
}

#[tokio::test]
async fn test_chain_store_handle_init_genesis() {
    let (chain_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let genesis = genesis_for_tests();

    chain_handle
        .init_or_setup_genesis(genesis.clone())
        .await
        .unwrap();

    // Verify genesis is stored
    let stored_genesis = chain_handle.get_share(&genesis.block_hash());
    assert!(stored_genesis.is_some());
    assert_eq!(stored_genesis.unwrap().block_hash(), genesis.block_hash());
}

#[tokio::test]
async fn test_chain_store_handle_add_share_block() {
    let (chain_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let genesis = genesis_for_tests();

    chain_handle
        .init_or_setup_genesis(genesis.clone())
        .await
        .unwrap();

    // Add a share
    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .miner_pubkey("020202020202020202020202020202020202020202020202020202020202020202")
        .work(2)
        .build();

    chain_handle.add_share_block(share1.clone()).await.unwrap();

    // Verify share is stored
    let stored_share = chain_handle.get_share(&share1.block_hash());
    assert!(stored_share.is_some());
}

#[tokio::test]
async fn test_build_locator_genesis_only() {
    let (chain_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let genesis = genesis_for_tests();

    chain_handle
        .init_or_setup_genesis(genesis.clone())
        .await
        .unwrap();

    let locator = chain_handle.build_locator(0).unwrap();
    assert_eq!(locator.len(), 1, "Locator should contain exactly genesis");
    assert_eq!(
        locator[0],
        genesis.block_hash(),
        "Locator should contain the genesis blockhash"
    );
}

#[tokio::test]
async fn test_build_locator_empty_chain() {
    let (chain_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let locator = chain_handle.build_locator(0).unwrap();
    assert!(
        locator.is_empty(),
        "Locator for empty chain should be empty"
    );
}

#[tokio::test]
async fn test_build_locator_short_chain() {
    let (chain_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let genesis = genesis_for_tests();

    chain_handle
        .init_or_setup_genesis(genesis.clone())
        .await
        .unwrap();

    // Build a chain of 5 shares after genesis
    let mut prev_hash = genesis.block_hash();
    let mut shares = Vec::with_capacity(5);
    for _ in 0..5 {
        let share = TestShareBlockBuilder::new()
            .prev_share_blockhash(prev_hash.to_string())
            .miner_pubkey("020202020202020202020202020202020202020202020202020202020202020202")
            .work(2)
            .build();
        chain_handle.add_share_block(share.clone()).await.unwrap();
        chain_handle
            .organise_header(share.header.clone())
            .await
            .unwrap();
        chain_handle
            .mark_block_valid(share.block_hash())
            .await
            .unwrap();
        chain_handle.organise_block().await.unwrap();
        prev_hash = share.block_hash();
        shares.push(share);
    }

    let locator = chain_handle.build_locator(0).unwrap();
    // With tip at height 5, step=1 for all entries: heights 5,4,3,2,1,0
    assert_eq!(
        locator.len(),
        6,
        "Short chain locator should include all heights"
    );
    // First entry should be the tip (height 5)
    assert_eq!(locator[0], shares[4].block_hash());
    // Last entry should be genesis (height 0)
    assert_eq!(locator[locator.len() - 1], genesis.block_hash());

    // Validate get_confirmed_at_height returns the correct blockhash for each height
    let confirmed_genesis = chain_handle.get_confirmed_at_height(0).unwrap();
    assert_eq!(
        confirmed_genesis,
        genesis.block_hash(),
        "Confirmed at height 0 should be genesis"
    );
    for (index, share) in shares.iter().enumerate() {
        let height = (index + 1) as u32;
        let confirmed = chain_handle.get_confirmed_at_height(height).unwrap();
        assert_eq!(
            confirmed,
            share.block_hash(),
            "Confirmed at height {height} should match share {index}"
        );
    }

    // Querying beyond the tip should return an error
    assert!(
        chain_handle.get_confirmed_at_height(6).is_err(),
        "Querying beyond tip height should return an error"
    );
}

#[tokio::test]
async fn test_build_locator_long_chain_step_doubling() {
    let (chain_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let genesis = genesis_for_tests();

    chain_handle
        .init_or_setup_genesis(genesis.clone())
        .await
        .unwrap();

    // Build a chain of 20 shares
    let mut prev_hash = genesis.block_hash();
    let mut shares = Vec::with_capacity(20);
    for _ in 0..20 {
        let share = TestShareBlockBuilder::new()
            .prev_share_blockhash(prev_hash.to_string())
            .miner_pubkey("020202020202020202020202020202020202020202020202020202020202020202")
            .work(2)
            .build();
        chain_handle.add_share_block(share.clone()).await.unwrap();
        chain_handle
            .organise_header(share.header.clone())
            .await
            .unwrap();
        chain_handle
            .mark_block_valid(share.block_hash())
            .await
            .unwrap();
        chain_handle.organise_block().await.unwrap();
        prev_hash = share.block_hash();
        shares.push(share);
    }

    let locator = chain_handle.build_locator(0).unwrap();
    // The locator should have fewer entries than the chain length
    // due to step doubling after 10 entries
    assert!(
        locator.len() < 21,
        "Long chain locator should be shorter than total chain height + 1"
    );
    // First should be tip
    assert_eq!(locator[0], shares[19].block_hash());
    // Last should be genesis
    assert_eq!(locator[locator.len() - 1], genesis.block_hash());
}

#[tokio::test]
async fn test_chain_store_handle_get_depth() {
    let (chain_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let genesis = genesis_for_tests();

    chain_handle
        .init_or_setup_genesis(genesis.clone())
        .await
        .unwrap();

    // Add shares
    let share1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .miner_pubkey("020202020202020202020202020202020202020202020202020202020202020202")
        .work(1)
        .build();

    chain_handle
        .add_share_block_and_organise_header(share1.clone())
        .await
        .unwrap();

    let share2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .miner_pubkey("020202020202020202020202020202020202020202020202020202020202020202")
        .work(1)
        .build();

    chain_handle
        .add_share_block_and_organise_header(share2)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_get_confirmed_headers_in_range() {
    let (chain_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let genesis = genesis_for_tests();

    chain_handle
        .init_or_setup_genesis(genesis.clone())
        .await
        .unwrap();

    let mut prev_hash = genesis.block_hash();
    let mut shares = vec![genesis.clone()];
    for _ in 0..4 {
        let share = TestShareBlockBuilder::new()
            .prev_share_blockhash(prev_hash.to_string())
            .miner_pubkey("020202020202020202020202020202020202020202020202020202020202020202")
            .work(2)
            .build();
        chain_handle.add_share_block(share.clone()).await.unwrap();
        chain_handle
            .organise_header(share.header.clone())
            .await
            .unwrap();
        chain_handle
            .mark_block_valid(share.block_hash())
            .await
            .unwrap();
        chain_handle.organise_block().await.unwrap();
        prev_hash = share.block_hash();
        shares.push(share);
    }

    // Full range: heights 0..4, returned newest-to-oldest
    let headers = chain_handle.get_confirmed_headers_in_range(0, 4).unwrap();
    assert_eq!(headers.len(), 5);
    assert_eq!(headers[0].blockhash, shares[4].block_hash());
    assert_eq!(headers[4].blockhash, genesis.block_hash());

    // Partial range: heights 2..4, returned newest-to-oldest
    let headers = chain_handle.get_confirmed_headers_in_range(2, 4).unwrap();
    assert_eq!(headers.len(), 3);
    assert_eq!(headers[0].blockhash, shares[4].block_hash());
    assert_eq!(headers[2].blockhash, shares[2].block_hash());

    // Long range: heights 0..10, returned newest-to-oldest
    let headers = chain_handle.get_confirmed_headers_in_range(0, 10).unwrap();
    assert_eq!(headers.len(), 5);
    assert_eq!(headers[0].blockhash, shares[4].block_hash());
    assert_eq!(headers[4].blockhash, shares[0].block_hash());
}

#[tokio::test]
async fn test_get_confirmed_headers_in_range_empty() {
    let (chain_handle, _temp_dir) = setup_test_chain_store_handle(true).await;

    // No genesis, no confirmed shares -- range query returns empty
    let headers = chain_handle.get_confirmed_headers_in_range(0, 10).unwrap();
    assert!(headers.is_empty());
}

#[tokio::test]
async fn test_is_current_returns_false_when_no_chain_tip() {
    let (chain_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    // No genesis initialised, so get_chain_tip_header will fail
    assert!(!chain_handle.is_current());
}

#[tokio::test]
async fn test_is_current_returns_true_when_tip_is_recent() {
    let (chain_handle, _temp_dir) = setup_test_chain_store_handle(true).await;

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as u32;

    let genesis = TestShareBlockBuilder::new().time(now_secs).build();

    chain_handle.init_or_setup_genesis(genesis).await.unwrap();

    assert!(chain_handle.is_current());
}

#[tokio::test]
async fn test_is_current_returns_false_when_tip_is_stale() {
    let (chain_handle, _temp_dir) = setup_test_chain_store_handle(true).await;

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as u32;
    // Set the tip timestamp 600 seconds in the past, well beyond the 300s threshold
    let stale_time = now_secs.saturating_sub(600);

    let genesis = TestShareBlockBuilder::new().time(stale_time).build();

    chain_handle.init_or_setup_genesis(genesis).await.unwrap();

    assert!(!chain_handle.is_current());
}

/// The age rule is decided by `is_tip_current`, which `is_current` calls
/// after reading the tip and the clock. Driving it directly pins both
/// sides of the boundary and cannot race the clock -- the previous version
/// read `SystemTime::now()` once to build the tip and again inside
/// `is_current`, so a one-second tick between them flipped the result.
#[test]
fn test_is_tip_current_at_boundary() {
    let now_secs = 1_000_000u64;
    let max_age = super::MAX_TIP_AGE_SECS;

    assert!(
        super::is_tip_current((now_secs - max_age) as u32, now_secs),
        "a tip exactly MAX_TIP_AGE_SECS old is still current"
    );
    assert!(
        !super::is_tip_current((now_secs - max_age - 1) as u32, now_secs),
        "one second past the boundary is stale"
    );
    assert!(
        super::is_tip_current(now_secs as u32, now_secs),
        "a tip stamped now is current"
    );
}

/// When an off-chain block (uncle) exists at the same height as a
/// chain block, the locator must contain only the chain block. The
/// locator walks the candidate chain, which has one entry per height,
/// so siblings sitting at that height are never picked up.
#[tokio::test]
async fn test_build_locator_excludes_blocks_not_on_chain() {
    let (chain_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let genesis = genesis_for_tests();

    chain_handle
        .init_or_setup_genesis(genesis.clone())
        .await
        .unwrap();

    // Build confirmed chain: genesis -> share_a (h:1) -> share_b (h:2)
    let share_a = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .miner_pubkey("020202020202020202020202020202020202020202020202020202020202020202")
        .work(2)
        .build();
    chain_handle.add_share_block(share_a.clone()).await.unwrap();
    chain_handle
        .organise_header(share_a.header.clone())
        .await
        .unwrap();
    chain_handle
        .mark_block_valid(share_a.block_hash())
        .await
        .unwrap();
    chain_handle.organise_block().await.unwrap();

    let share_b = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_a.block_hash().to_string())
        .miner_pubkey("020202020202020202020202020202020202020202020202020202020202020202")
        .work(2)
        .build();
    chain_handle.add_share_block(share_b.clone()).await.unwrap();
    chain_handle
        .organise_header(share_b.header.clone())
        .await
        .unwrap();
    chain_handle
        .mark_block_valid(share_b.block_hash())
        .await
        .unwrap();
    chain_handle.organise_block().await.unwrap();

    // Store an uncle at height 1 (same height as share_a) via the
    // underlying Store, which puts it in the BlockHeight CF without
    // confirming it.
    let uncle = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(999)
        .build();
    chain_handle
        .store_handle()
        .store()
        .store_with_valid_metadata(&uncle);

    // Verify the uncle is indeed stored at height 1
    let blocks_at_height_1 = chain_handle.store_handle().get_blockhashes_for_height(1);
    assert!(
        blocks_at_height_1.contains(&uncle.block_hash()),
        "Uncle should be stored at height 1"
    );
    assert!(
        blocks_at_height_1.len() > 1,
        "Height 1 should have both confirmed share and uncle"
    );

    let locator = chain_handle.build_locator(0).unwrap();

    // Locator should contain only confirmed blocks
    assert!(
        !locator.contains(&uncle.block_hash()),
        "Locator must not contain the non-confirmed uncle"
    );
    assert_eq!(
        locator[0],
        share_b.block_hash(),
        "First entry should be tip"
    );
    assert_eq!(
        locator[locator.len() - 1],
        genesis.block_hash(),
        "Last entry should be genesis"
    );
    // Heights 2, 1, 0 -- all confirmed
    assert_eq!(locator.len(), 3);
    assert_eq!(locator[1], share_a.block_hash());
}

/// The locator anchors on the candidate tip, not the confirmed tip.
///
/// Confirmed lags candidate whenever bodies lag headers, so anchoring on
/// the confirmed tip made a restarting node re-request every header above
/// it.
#[tokio::test]
async fn test_build_locator_anchors_on_candidate_tip() {
    let (chain_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let genesis = genesis_for_tests();
    chain_handle
        .init_or_setup_genesis(genesis.clone())
        .await
        .unwrap();

    // Confirmed chain reaches height 1.
    let confirmed_share = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .miner_pubkey("020202020202020202020202020202020202020202020202020202020202020202")
        .work(2)
        .build();
    chain_handle
        .add_share_block(confirmed_share.clone())
        .await
        .unwrap();
    chain_handle
        .organise_header(confirmed_share.header.clone())
        .await
        .unwrap();
    chain_handle
        .mark_block_valid(confirmed_share.block_hash())
        .await
        .unwrap();
    chain_handle.organise_block().await.unwrap();

    // Height 2 is header-only: on the candidate chain, never confirmed.
    let candidate_share = TestShareBlockBuilder::new()
        .prev_share_blockhash(confirmed_share.block_hash().to_string())
        .miner_pubkey("020202020202020202020202020202020202020202020202020202020202020202")
        .work(2)
        .build();
    chain_handle
        .add_share_block(candidate_share.clone())
        .await
        .unwrap();
    chain_handle
        .organise_header(candidate_share.header.clone())
        .await
        .unwrap();

    let locator = chain_handle.build_locator(0).unwrap();
    assert_eq!(
        locator[0],
        candidate_share.block_hash(),
        "locator must anchor on the candidate tip, not the confirmed tip"
    );
}

/// The candidate walk keeps its genesis anchor.
///
/// Genesis is registered on the confirmed chain only, so resolving each
/// height must fall back to the confirmed index. Without that fallback the
/// locator loses genesis, the peer matches nothing, and it answers with
/// genesis -- a full header refetch.
#[tokio::test]
async fn test_build_locator_keeps_genesis_anchor_above_confirmed() {
    let (chain_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let genesis = genesis_for_tests();
    chain_handle
        .init_or_setup_genesis(genesis.clone())
        .await
        .unwrap();

    let confirmed_share = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .miner_pubkey("020202020202020202020202020202020202020202020202020202020202020202")
        .work(2)
        .build();
    chain_handle
        .add_share_block(confirmed_share.clone())
        .await
        .unwrap();
    chain_handle
        .organise_header(confirmed_share.header.clone())
        .await
        .unwrap();
    chain_handle
        .mark_block_valid(confirmed_share.block_hash())
        .await
        .unwrap();
    chain_handle.organise_block().await.unwrap();

    let candidate_share = TestShareBlockBuilder::new()
        .prev_share_blockhash(confirmed_share.block_hash().to_string())
        .miner_pubkey("020202020202020202020202020202020202020202020202020202020202020202")
        .work(2)
        .build();
    chain_handle
        .add_share_block(candidate_share.clone())
        .await
        .unwrap();
    chain_handle
        .organise_header(candidate_share.header.clone())
        .await
        .unwrap();

    let locator = chain_handle.build_locator(0).unwrap();
    assert_eq!(
        locator[locator.len() - 1],
        genesis.block_hash(),
        "locator must still end at genesis"
    );
    assert!(
        locator.contains(&confirmed_share.block_hash()),
        "locator must still cover the confirmed chain below the candidate tip"
    );
}

/// find_fork_point_height walks from a fork block back to the
/// confirmed chain and returns the confirmed ancestor's height.
///
/// Chain: genesis(h:0) -> A(h:1) -> B(h:2) -> C(h:3)  [confirmed]
/// Fork:                  A(h:1) -> D(h:2) -> E(h:3)   [header only]
///
/// find_fork_point_height(E) should return height 1 (A is the
/// confirmed ancestor where the fork meets the confirmed chain).
#[tokio::test]
async fn test_find_fork_point_height_returns_confirmed_ancestor_height() {
    let (chain_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let genesis = genesis_for_tests();

    chain_handle
        .init_or_setup_genesis(genesis.clone())
        .await
        .unwrap();

    // Build confirmed chain: genesis -> A -> B -> C
    let mut prev_hash = genesis.block_hash();
    let mut confirmed_shares = Vec::with_capacity(3);
    for index in 0..3 {
        let share = TestShareBlockBuilder::new()
            .prev_share_blockhash(prev_hash.to_string())
            .miner_pubkey("020202020202020202020202020202020202020202020202020202020202020202")
            .nonce(index)
            .work(2)
            .build();
        chain_handle.add_share_block(share.clone()).await.unwrap();
        chain_handle
            .organise_header(share.header.clone())
            .await
            .unwrap();
        chain_handle
            .mark_block_valid(share.block_hash())
            .await
            .unwrap();
        chain_handle.organise_block().await.unwrap();
        prev_hash = share.block_hash();
        confirmed_shares.push(share);
    }

    // Build fork: A(h:1) -> D(h:2) -> E(h:3), header only
    let share_a_hash = confirmed_shares[0].block_hash();
    let fork_d = TestShareBlockBuilder::new()
        .prev_share_blockhash(share_a_hash.to_string())
        .nonce(100)
        .work(1)
        .build();
    chain_handle
        .store_handle()
        .store()
        .store_with_valid_metadata(&fork_d);

    let fork_e = TestShareBlockBuilder::new()
        .prev_share_blockhash(fork_d.block_hash().to_string())
        .nonce(101)
        .work(1)
        .build();
    chain_handle
        .store_handle()
        .store()
        .store_with_valid_metadata(&fork_e);

    // Fork point for E should be A's height (1)
    let fork_height = chain_handle
        .find_fork_point_height(&fork_e.block_hash())
        .unwrap();
    assert_eq!(fork_height, Some(1));

    // Fork point for D should also be A's height (1)
    let fork_height_d = chain_handle
        .find_fork_point_height(&fork_d.block_hash())
        .unwrap();
    assert_eq!(fork_height_d, Some(1));

    // Fork point for a confirmed block (C) should be its own height (3)
    let confirmed_height = chain_handle
        .find_fork_point_height(&confirmed_shares[2].block_hash())
        .unwrap();
    assert_eq!(confirmed_height, Some(3));
}

/// find_fork_point_height returns None when the blockhash is
/// not in the store (ancestry cannot be walked).
#[tokio::test]
async fn test_find_fork_point_height_returns_none_for_unknown_block() {
    let (chain_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let genesis = genesis_for_tests();

    chain_handle
        .init_or_setup_genesis(genesis.clone())
        .await
        .unwrap();

    let unknown_hash = BlockHash::all_zeros();
    let result = chain_handle.find_fork_point_height(&unknown_hash).unwrap();
    assert_eq!(result, None);
}

/// A node that is not current never prunes, even with bodies far below
/// the target: its confirmed tip may lag the candidate tip.
#[tokio::test]
async fn test_prune_block_bodies_if_due_skips_when_not_current() {
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let blocks = build_confirmed_test_chain(&chain_store_handle, 4).await;
    let mut next_prune_start = None;

    chain_store_handle
        .prune_block_bodies_if_due(&mut next_prune_start)
        .await
        .unwrap();

    assert!(!chain_store_handle.is_current());
    assert_eq!(next_prune_start, None);
    assert!(
        chain_store_handle
            .store_handle()
            .store()
            .share_block_exists(&blocks[1].block_hash())
    );
}
