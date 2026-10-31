use super::*;
use crate::test_utils::{TestShareBlockBuilder, setup_test_chain_store_handle};
use bitcoin::blockdata::constants::genesis_block;
use tempfile::tempdir;

#[test]
fn test_confirmed_share_meeting_claimed_target_remains_unverified_candidate() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Meeting a header's claimed target does not establish BCHN acceptance.
    let mut share = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    share.header.bitcoin_header = genesis_block(bitcoin::Network::Regtest).header;
    store.push_to_confirmed_chain(&share).unwrap();

    assert!(store.get_found_blocks().unwrap().is_empty());
    let found_blocks = store.get_candidate_found_blocks().unwrap();
    assert_eq!(
        found_blocks,
        vec![FoundBlock {
            bitcoin_height: share.header.bitcoin_height,
            bitcoin_blockhash: share.header.bitcoin_header.block_hash(),
            share_blockhash: share.block_hash(),
            bitcoin_time: share.header.bitcoin_header.time,
            miner_bitcoin_address: share.header.miner_bitcoin_address.to_string(),
        }]
    );
}

#[test]
fn test_confirmed_share_missing_bitcoin_target_is_not_recorded() {
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
    assert!(!share.header.meets_bitcoin_difficulty());
    store.push_to_confirmed_chain(&share).unwrap();

    assert_eq!(
        store.get_confirmed_at_height(1).unwrap(),
        share.block_hash()
    );
    assert!(store.get_found_blocks().unwrap().is_empty());
    assert!(store.get_candidate_found_blocks().unwrap().is_empty());
}

#[test]
fn test_uncle_meeting_claimed_target_becomes_candidate_when_nephew_confirms() {
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
    store.push_to_confirmed_chain(&share1).unwrap();

    // The uncle is a sibling of share1 that lost the share chain race
    // and meets its claimed BCH target. It arrives after share1 with equal
    // work, so it stays off the candidate and confirmed chains.
    let mut uncle = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695793)
        .build();
    uncle.header.bitcoin_header = genesis_block(bitcoin::Network::Regtest).header;
    store.push_to_candidate_chain(&uncle).unwrap();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&uncle, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();
    assert_eq!(
        store.get_confirmed_at_height(1).unwrap(),
        share1.block_hash()
    );
    assert!(store.get_found_blocks().unwrap().is_empty());

    let nephew = TestShareBlockBuilder::new()
        .prev_share_blockhash(share1.block_hash().to_string())
        .uncles(vec![uncle.block_hash()])
        .nonce(0xe9695794)
        .build();
    store.push_to_confirmed_chain(&nephew).unwrap();

    assert_eq!(
        store.get_confirmed_at_height(2).unwrap(),
        nephew.block_hash()
    );
    assert!(store.get_found_blocks().unwrap().is_empty());
    let found_blocks = store.get_candidate_found_blocks().unwrap();
    assert_eq!(found_blocks.len(), 1);
    assert_eq!(found_blocks[0].share_blockhash, uncle.block_hash());
    assert_eq!(
        found_blocks[0].bitcoin_blockhash,
        uncle.header.bitcoin_header.block_hash()
    );
}

#[test]
fn test_header_only_share_meeting_claimed_target_becomes_unverified_candidate() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Below the prune height a syncing node promotes shares header-only,
    // so recording must not need the share body.
    let mut share = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    share.header.bitcoin_header = genesis_block(bitcoin::Network::Regtest).header;
    store.push_to_candidate_chain(&share).unwrap();
    assert!(!store.share_block_exists(&share.block_hash()));

    let mut batch = Store::get_write_batch();
    store
        .record_found_blocks(&share.block_hash(), &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    assert!(store.get_found_blocks().unwrap().is_empty());
    let found_blocks = store.get_candidate_found_blocks().unwrap();
    assert_eq!(found_blocks.len(), 1);
    assert_eq!(found_blocks[0].share_blockhash, share.block_hash());
}

#[test]
fn test_record_found_blocks_errors_when_header_missing() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
    let share = TestShareBlockBuilder::new().nonce(0xe9695792).build();

    let mut batch = Store::get_write_batch();
    let result = store.record_found_blocks(&share.block_hash(), &mut batch);

    assert!(matches!(result, Err(StoreError::NotFound(_))));
}

/// A candidate is not a found block. Once accepted it is one, it survives a
/// reopen, it is recorded only once and it is no longer a candidate.
#[test]
fn test_accepted_candidate_is_recorded_once_and_survives_reopen() {
    let temp_dir = tempdir().unwrap();
    let path = temp_dir.path().to_str().unwrap().to_string();
    let store = Store::new(path.clone(), false).unwrap();
    let mut share = TestShareBlockBuilder::new().nonce(0xe9695792).build();
    share.header.bitcoin_header = genesis_block(bitcoin::Network::Regtest).header;
    let mut batch = Store::get_write_batch();
    store.put_found_block_if_bitcoin_block(&share.header, &mut batch);
    store.commit_batch(batch).unwrap();

    assert!(store.get_found_blocks().unwrap().is_empty());
    let candidate = store.get_candidate_found_blocks().unwrap().remove(0);
    assert_eq!(candidate.share_blockhash, share.block_hash());
    assert!(store.record_accepted_found_block(&candidate).unwrap());
    assert_eq!(store.get_found_blocks().unwrap(), vec![candidate.clone()]);
    assert!(store.get_candidate_found_blocks().unwrap().is_empty());
    drop(store);

    let store = Store::new(path, false).unwrap();
    assert_eq!(store.get_found_blocks().unwrap(), vec![candidate.clone()]);
    assert!(!store.record_accepted_found_block(&candidate).unwrap());
    assert!(store.get_candidate_found_blocks().unwrap().is_empty());
}

#[tokio::test]
async fn test_accepted_block_is_serialized_and_recorded_once_before_share_persists() {
    let (chain_store, _temp_dir) = setup_test_chain_store_handle(true).await;
    let share = TestShareBlockBuilder::new().nonce(0xe9695792).build();
    let accepted = FoundBlock {
        bitcoin_height: share.header.bitcoin_height,
        bitcoin_blockhash: share.header.bitcoin_header.block_hash(),
        share_blockhash: share.block_hash(),
        bitcoin_time: share.header.bitcoin_header.time,
        miner_bitcoin_address: share.header.miner_bitcoin_address.to_string(),
    };
    assert!(!chain_store.share_block_exists(&share.block_hash()));
    assert!(
        chain_store
            .record_accepted_found_block(accepted.clone())
            .await
            .unwrap()
    );
    let mut duplicate = accepted.clone();
    duplicate.bitcoin_time += 1;
    assert!(
        !chain_store
            .record_accepted_found_block(duplicate)
            .await
            .unwrap()
    );
    assert_eq!(chain_store.get_found_blocks().unwrap(), vec![accepted]);
    assert!(chain_store.get_candidate_found_blocks().unwrap().is_empty());
    assert!(!chain_store.share_block_exists(&share.block_hash()));
}
