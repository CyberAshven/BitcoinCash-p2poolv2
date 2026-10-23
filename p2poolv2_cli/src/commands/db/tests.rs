use super::*;
use p2poolv2_lib::test_utils::{
    TestShareBlockBuilder, genesis_for_tests, setup_test_chain_store_handle,
};

#[tokio::test]
async fn test_cleanup_dense_heights_invalidates_unreferenced_header_valid() {
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let genesis = genesis_for_tests();
    chain_store_handle
        .init_or_setup_genesis(genesis.clone())
        .await
        .unwrap();

    // Confirmed block at height 1
    let confirmed = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(1)
        .work(2)
        .build();
    chain_store_handle
        .add_share_block(confirmed.clone())
        .await
        .unwrap();
    chain_store_handle
        .organise_header(confirmed.header.clone())
        .await
        .unwrap();
    chain_store_handle
        .mark_block_valid(confirmed.block_hash())
        .await
        .unwrap();
    chain_store_handle.organise_block().await.unwrap();

    // Uncle at height 1 (organise header stores as HeaderValid)
    let uncle = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(100)
        .build();
    chain_store_handle
        .add_share_block(uncle.clone())
        .await
        .unwrap();
    chain_store_handle
        .organise_header(uncle.header.clone())
        .await
        .unwrap();

    // Confirmed block at height 2 referencing uncle
    let confirmed_with_uncle = TestShareBlockBuilder::new()
        .prev_share_blockhash(confirmed.block_hash().to_string())
        .uncles(vec![uncle.block_hash()])
        .nonce(2)
        .work(2)
        .build();
    chain_store_handle
        .add_share_block(confirmed_with_uncle.clone())
        .await
        .unwrap();
    chain_store_handle
        .organise_header(confirmed_with_uncle.header.clone())
        .await
        .unwrap();
    chain_store_handle
        .mark_block_valid(confirmed_with_uncle.block_hash())
        .await
        .unwrap();
    chain_store_handle.organise_block().await.unwrap();

    // 25 unreferenced HeaderValid blocks at height 1
    for nonce in 200..225 {
        let spam = TestShareBlockBuilder::new()
            .prev_share_blockhash(genesis.block_hash().to_string())
            .nonce(nonce)
            .build();
        chain_store_handle
            .add_share_block(spam.clone())
            .await
            .unwrap();
        chain_store_handle
            .organise_header(spam.header.clone())
            .await
            .unwrap();
    }

    let store = chain_store_handle.store_handle().store();
    let blocks_before = store.get_blockhashes_for_height(1);
    assert_eq!(blocks_before.len(), 27);

    cleanup_dense_heights(store).unwrap();

    // Uncle and confirmed should survive, 25 spam blocks invalidated
    let metadata_results = store.get_block_metadata_batch(&blocks_before).unwrap();
    let valid_count = metadata_results
        .iter()
        .filter(|(_, metadata)| metadata.status != Status::Invalid)
        .count();
    // 1 confirmed + 1 uncle-referenced HeaderValid = 2 valid
    assert_eq!(valid_count, 2);
}

#[tokio::test]
async fn test_cleanup_dense_heights_invalidates_offchain_block_valid() {
    // Reproduces the case where dense height is full of
    // BlockValid siblings this node mined onto a losing fork. They must be
    // invalidated; the confirmed block and a BlockValid uncle survive.
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let genesis = genesis_for_tests();
    chain_store_handle
        .init_or_setup_genesis(genesis.clone())
        .await
        .unwrap();

    // Confirmed block at height 1
    let confirmed = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(1)
        .work(2)
        .build();
    chain_store_handle
        .add_share_block(confirmed.clone())
        .await
        .unwrap();
    chain_store_handle
        .organise_header(confirmed.header.clone())
        .await
        .unwrap();
    chain_store_handle
        .mark_block_valid(confirmed.block_hash())
        .await
        .unwrap();
    chain_store_handle.organise_block().await.unwrap();

    // Off-chain BlockValid uncle at height 1, referenced by a confirmed
    // block at height 2. A BlockValid uncle must be kept.
    let uncle = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(100)
        .build();
    chain_store_handle
        .add_share_block(uncle.clone())
        .await
        .unwrap();
    chain_store_handle
        .organise_header(uncle.header.clone())
        .await
        .unwrap();
    chain_store_handle
        .mark_block_valid(uncle.block_hash())
        .await
        .unwrap();

    let confirmed_with_uncle = TestShareBlockBuilder::new()
        .prev_share_blockhash(confirmed.block_hash().to_string())
        .uncles(vec![uncle.block_hash()])
        .nonce(2)
        .work(2)
        .build();
    chain_store_handle
        .add_share_block(confirmed_with_uncle.clone())
        .await
        .unwrap();
    chain_store_handle
        .organise_header(confirmed_with_uncle.header.clone())
        .await
        .unwrap();
    chain_store_handle
        .mark_block_valid(confirmed_with_uncle.block_hash())
        .await
        .unwrap();
    chain_store_handle.organise_block().await.unwrap();

    // 25 off-chain BlockValid siblings at height 1 (the mined-fork flood)
    for nonce in 200..225 {
        let spam = TestShareBlockBuilder::new()
            .prev_share_blockhash(genesis.block_hash().to_string())
            .nonce(nonce)
            .build();
        chain_store_handle
            .add_share_block(spam.clone())
            .await
            .unwrap();
        chain_store_handle
            .organise_header(spam.header.clone())
            .await
            .unwrap();
        chain_store_handle
            .mark_block_valid(spam.block_hash())
            .await
            .unwrap();
    }

    let store = chain_store_handle.store_handle().store();
    let blocks_before = store.get_blockhashes_for_height(1);
    assert_eq!(blocks_before.len(), 27);

    cleanup_dense_heights(store).unwrap();

    // Confirmed + BlockValid uncle survive; 25 BlockValid spam invalidated.
    let metadata_results = store.get_block_metadata_batch(&blocks_before).unwrap();
    let valid_count = metadata_results
        .iter()
        .filter(|(_, metadata)| metadata.status != Status::Invalid)
        .count();
    assert_eq!(valid_count, 2);
}

#[tokio::test]
async fn test_cleanup_dense_heights_invalidates_uncle_of_dead_sibling() {
    // flood siblings reference each other as
    // uncles across consecutive dense heights, so none is "unreferenced".
    // Processing top-down, the higher flood is invalidated first, which
    // leaves the lower flood with no live nephew, so it too is removed.
    // A naive "kept if any nephew exists" check would leave the lower
    // height dense and still walling sync.
    let (chain_store_handle, _temp_dir) = setup_test_chain_store_handle(true).await;
    let genesis = genesis_for_tests();
    chain_store_handle
        .init_or_setup_genesis(genesis.clone())
        .await
        .unwrap();

    // Confirmed chain: genesis -> C1 (h1) -> C2 (h2)
    let c1 = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(1)
        .work(2)
        .build();
    chain_store_handle
        .add_share_block(c1.clone())
        .await
        .unwrap();
    chain_store_handle
        .organise_header(c1.header.clone())
        .await
        .unwrap();
    chain_store_handle
        .mark_block_valid(c1.block_hash())
        .await
        .unwrap();
    chain_store_handle.organise_block().await.unwrap();

    let c2 = TestShareBlockBuilder::new()
        .prev_share_blockhash(c1.block_hash().to_string())
        .nonce(2)
        .work(2)
        .build();
    chain_store_handle
        .add_share_block(c2.clone())
        .await
        .unwrap();
    chain_store_handle
        .organise_header(c2.header.clone())
        .await
        .unwrap();
    chain_store_handle
        .mark_block_valid(c2.block_hash())
        .await
        .unwrap();
    chain_store_handle.organise_block().await.unwrap();

    // 25 off-chain flood siblings at h1
    let mut lower_flood = Vec::with_capacity(25);
    for nonce in 200..225 {
        let f1 = TestShareBlockBuilder::new()
            .prev_share_blockhash(genesis.block_hash().to_string())
            .nonce(nonce)
            .build();
        chain_store_handle
            .add_share_block(f1.clone())
            .await
            .unwrap();
        chain_store_handle
            .organise_header(f1.header.clone())
            .await
            .unwrap();
        chain_store_handle
            .mark_block_valid(f1.block_hash())
            .await
            .unwrap();
        lower_flood.push(f1.block_hash());
    }

    // 25 off-chain flood siblings at h2, each declaring a distinct h1
    // flood block as an uncle -- so every h1 flood block has a nephew,
    // but only a dead (off-chain) one.
    for (index, uncle) in lower_flood.iter().enumerate() {
        let f2 = TestShareBlockBuilder::new()
            .prev_share_blockhash(c1.block_hash().to_string())
            .uncles(vec![*uncle])
            .nonce(300 + index as u32)
            .build();
        chain_store_handle
            .add_share_block(f2.clone())
            .await
            .unwrap();
        chain_store_handle
            .organise_header(f2.header.clone())
            .await
            .unwrap();
        chain_store_handle
            .mark_block_valid(f2.block_hash())
            .await
            .unwrap();
    }

    let store = chain_store_handle.store_handle().store();
    assert_eq!(store.get_blockhashes_for_height(1).len(), 26); // C1 + 25 flood
    assert_eq!(store.get_blockhashes_for_height(2).len(), 26); // C2 + 25 flood

    cleanup_dense_heights(store).unwrap();

    // Both heights collapse to their single confirmed block: the h1 flood
    // is invalidated even though every one had a nephew, because those
    // nephews (the h2 flood) were invalidated first.
    let after_h1 = store.get_block_metadata_batch(&lower_flood).unwrap();
    let live_h1 = after_h1
        .iter()
        .filter(|(_, metadata)| metadata.status != Status::Invalid)
        .count();
    assert_eq!(live_h1, 0, "all h1 flood siblings should be invalidated");
}
