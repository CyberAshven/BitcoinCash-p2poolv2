use super::*;
use crate::test_utils::TestShareBlockBuilder;
use bitcoin::blockdata::constants::genesis_block;
use tempfile::tempdir;

#[test]
fn test_confirmed_share_meeting_bitcoin_target_is_recorded() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
    let genesis = TestShareBlockBuilder::new().nonce(0xe9695791).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // The regtest genesis header meets its own target, so this share
    // carries a bitcoin block.
    let mut share = TestShareBlockBuilder::new()
        .prev_share_blockhash(genesis.block_hash().to_string())
        .nonce(0xe9695792)
        .build();
    share.header.bitcoin_header = genesis_block(bitcoin::Network::Regtest).header;
    store.push_to_confirmed_chain(&share).unwrap();

    let found_blocks = store.get_found_blocks().unwrap();
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
}

#[test]
fn test_uncle_meeting_bitcoin_target_is_recorded_when_nephew_confirms() {
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
    // but carries a bitcoin block. It arrives after share1 with equal
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
    let found_blocks = store.get_found_blocks().unwrap();
    assert_eq!(found_blocks.len(), 1);
    assert_eq!(found_blocks[0].share_blockhash, uncle.block_hash());
    assert_eq!(
        found_blocks[0].bitcoin_blockhash,
        uncle.header.bitcoin_header.block_hash()
    );
}

#[test]
fn test_header_only_share_meeting_bitcoin_target_is_recorded() {
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

    let found_blocks = store.get_found_blocks().unwrap();
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
