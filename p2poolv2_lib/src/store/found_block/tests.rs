use super::*;
use crate::test_utils::{TestShareBlockBuilder, setup_test_chain_store_handle};
use bitcoin::blockdata::constants::genesis_block;
use bitcoin::hashes::Hash;
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
            effort: None,
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
fn test_get_latest_found_blocks_empty_store() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let latest_found_blocks = store.get_latest_found_blocks(10, 1000).unwrap();

    assert_eq!(latest_found_blocks.counted, 0);
    assert!(!latest_found_blocks.scan_limit_reached);
    assert!(latest_found_blocks.found_blocks.is_empty());
}

#[test]
fn test_get_latest_found_blocks_returns_newest_first_and_bounds_count_by_scan_limit() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    //* Three accepted blocks at bitcoin heights 100, 101 and 102; the
    //* height prefix orders their keys. Each has its own blockhash, so an
    //* effort snapshot joins onto one block only.
    let mut accepted = Vec::new();
    for (nonce, bitcoin_height) in [(0xe9695792, 100), (0xe9695793, 101), (0xe9695794, 102)] {
        let share = TestShareBlockBuilder::new().nonce(nonce).build();
        let found_block = FoundBlock {
            bitcoin_height,
            bitcoin_blockhash: BlockHash::from_byte_array([bitcoin_height as u8; 32]),
            share_blockhash: share.block_hash(),
            bitcoin_time: share.header.bitcoin_header.time,
            miner_bitcoin_address: share.header.miner_bitcoin_address.to_string(),
            effort: None,
        };
        assert!(store.record_accepted_found_block(&found_block).unwrap());
        accepted.push(found_block);
    }
    let snapshot = EffortSnapshot {
        work: 460.5,
        network_difficulty: Some(447_000.0),
    };
    store
        .set_accepted_found_block_effort(&accepted[2].bitcoin_blockhash, snapshot)
        .unwrap();

    let latest_found_blocks = store.get_latest_found_blocks(2, 1000).unwrap();

    //* The count includes the undecoded oldest entry too; the list
    //* holds only the two newest, highest height first, each joined to
    //* the effort kept for it.
    assert_eq!(latest_found_blocks.counted, 3);
    assert!(!latest_found_blocks.scan_limit_reached);
    let mut newest = accepted[2].clone();
    newest.effort = Some(snapshot);
    assert_eq!(
        latest_found_blocks.found_blocks,
        vec![newest, accepted[1].clone()]
    );

    //* A scan limit below the entry count stops the walk early and
    //* flags that more entries remain.
    let capped = store.get_latest_found_blocks(2, 2).unwrap();
    assert_eq!(capped.counted, 2);
    assert!(capped.scan_limit_reached);
    assert_eq!(capped.found_blocks, latest_found_blocks.found_blocks);

    //* A scan limit equal to the entry count walks every entry, so
    //* nothing remains and the count is exact.
    let exact = store.get_latest_found_blocks(2, 3).unwrap();
    assert_eq!(exact.counted, 3);
    assert!(!exact.scan_limit_reached);
}

/// The bounded read, like `get_found_blocks`, returns accepted blocks only:
/// a candidate awaiting BCHN verification is neither returned nor counted.
#[test]
fn test_get_latest_found_blocks_leaves_out_candidates() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
    let mut share = TestShareBlockBuilder::new().nonce(0xe9695792).build();
    share.header.bitcoin_header = genesis_block(bitcoin::Network::Regtest).header;
    let mut batch = Store::get_write_batch();
    store.put_found_block_if_bitcoin_block(&share.header, &mut batch);
    store.commit_batch(batch).unwrap();
    assert_eq!(store.get_candidate_found_blocks().unwrap().len(), 1);

    let latest_found_blocks = store.get_latest_found_blocks(10, 1000).unwrap();

    assert_eq!(latest_found_blocks.counted, 0);
    assert!(latest_found_blocks.found_blocks.is_empty());
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
        effort: None,
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

/// A find's effort snapshot is written after the accepted block and read
/// back joined to it: the work since the previous find over the network
/// difficulty of the header that claimed it.
#[tokio::test]
async fn test_effort_snapshot_joins_onto_the_accepted_block() {
    let (chain_store, _temp_dir) = setup_test_chain_store_handle(true).await;
    let share = TestShareBlockBuilder::new().nonce(0xe9695792).build();
    let accepted = FoundBlock {
        bitcoin_height: share.header.bitcoin_height,
        bitcoin_blockhash: share.header.bitcoin_header.block_hash(),
        share_blockhash: share.block_hash(),
        bitcoin_time: share.header.bitcoin_header.time,
        miner_bitcoin_address: share.header.miner_bitcoin_address.to_string(),
        effort: None,
    };
    assert!(
        chain_store
            .record_accepted_found_block(accepted.clone())
            .await
            .unwrap()
    );
    let snapshot = EffortSnapshot {
        work: 460.5,
        network_difficulty: Some(447_000.0),
    };
    chain_store
        .set_accepted_found_block_effort(accepted.bitcoin_blockhash, snapshot)
        .await
        .unwrap();
    let mut expected = accepted;
    expected.effort = Some(snapshot);
    assert_eq!(chain_store.get_found_blocks().unwrap(), vec![expected]);
}

/// The snapshot lives in its own column family beside the accepted record
/// and survives reopening the store.
#[test]
fn test_effort_snapshot_survives_reopen() {
    let temp_dir = tempdir().unwrap();
    let path = temp_dir.path().to_str().unwrap().to_string();
    let share = TestShareBlockBuilder::new().nonce(0xe9695792).build();
    let accepted = FoundBlock {
        bitcoin_height: share.header.bitcoin_height,
        bitcoin_blockhash: share.header.bitcoin_header.block_hash(),
        share_blockhash: share.block_hash(),
        bitcoin_time: share.header.bitcoin_header.time,
        miner_bitcoin_address: share.header.miner_bitcoin_address.to_string(),
        effort: None,
    };
    let snapshot = EffortSnapshot {
        work: 33.0,
        network_difficulty: Some(447_000.0),
    };
    {
        let store = Store::new(path.clone(), false).unwrap();
        assert!(store.record_accepted_found_block(&accepted).unwrap());
        store
            .set_accepted_found_block_effort(&accepted.bitcoin_blockhash, snapshot)
            .unwrap();
    }
    let reopened = Store::new(path, false).unwrap();
    let mut expected = accepted;
    expected.effort = Some(snapshot);
    assert_eq!(reopened.get_found_blocks().unwrap(), vec![expected]);
}

/// Read-only tools never request the effort column family: they open a
/// store that has one, read the accepted finds, and report no effort.
#[test]
fn test_read_only_open_reads_found_blocks_without_effort() {
    let temp_dir = tempdir().unwrap();
    let path = temp_dir.path().to_str().unwrap().to_string();
    let share = TestShareBlockBuilder::new().nonce(0xe9695792).build();
    let accepted = FoundBlock {
        bitcoin_height: share.header.bitcoin_height,
        bitcoin_blockhash: share.header.bitcoin_header.block_hash(),
        share_blockhash: share.block_hash(),
        bitcoin_time: share.header.bitcoin_header.time,
        miner_bitcoin_address: share.header.miner_bitcoin_address.to_string(),
        effort: None,
    };
    {
        let store = Store::new(path.clone(), false).unwrap();
        assert!(store.record_accepted_found_block(&accepted).unwrap());
        store
            .set_accepted_found_block_effort(
                &accepted.bitcoin_blockhash,
                EffortSnapshot {
                    work: 460.5,
                    network_difficulty: Some(447_000.0),
                },
            )
            .unwrap();
    }
    let read_only = Store::new(path, true).unwrap();
    assert_eq!(read_only.get_found_blocks().unwrap(), vec![accepted]);
}
