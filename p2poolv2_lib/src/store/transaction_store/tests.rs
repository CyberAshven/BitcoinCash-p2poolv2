use super::*;
use crate::store::block_tx_metadata::{BlockMetadata, ChainMembership, Status};
use crate::test_utils::TestShareBlockBuilder;
use bitcoin::hashes::Hash;
use bitcoin::pow::Work;
use tempfile::tempdir;

#[test]
fn test_outpoint_key_is_txid_hex_colon_index() {
    let txid = Txid::from_byte_array([0x01; 32]);

    assert_eq!(
        outpoint_key(&txid, 7),
        "0101010101010101010101010101010101010101010101010101010101010101:7"
    );
}

#[test]
fn test_txids_key_is_serialized_blockhash_then_suffix() {
    let blockhash = BlockHash::from_byte_array([0x02; 32]);
    let mut expected = vec![0x02; 32];
    expected.extend_from_slice(b"_txids");

    assert_eq!(txids_key(&blockhash, BLOCK_TXIDS_SUFFIX), expected);
}

#[test]
fn test_transaction_store_should_succeed() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
    let block = TestShareBlockBuilder::new().build();
    let mut batch = Store::get_write_batch();

    let metadata = store
        .add_sharechain_txs(&block.transactions, 1, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    assert_eq!(metadata.len(), block.transactions.len());
    for tx_meta in &metadata {
        let tx = store.get_tx(&tx_meta.txid).unwrap();
        assert_eq!(tx.compute_txid(), tx_meta.txid);
    }
}

#[test]
fn test_get_txs_returns_empty_for_empty_input() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
    assert!(store.get_txs(&[]).unwrap().is_empty());
}

#[test]
fn test_get_txs_returns_every_requested_transaction() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // Store three funding coinbase transactions so that spending inputs
    // can find their outputs in the Outputs CF.
    let funding_coinbase_input = bitcoin::TxIn {
        previous_output: bitcoin::OutPoint::new(bitcoin::Txid::all_zeros(), u32::MAX),
        sequence: bitcoin::Sequence::default(),
        witness: bitcoin::Witness::default(),
        script_sig: bitcoin::ScriptBuf::new(),
    };
    let funding_tx_1 = Transaction {
        version: bitcoin::transaction::Version(1),
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![funding_coinbase_input.clone()],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(1_000_000),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };
    let funding_txid_1 = funding_tx_1.compute_txid();

    // funding_tx_2 needs at least 6 outputs (index 5 is used by tx_b)
    let funding_tx_2 = Transaction {
        version: bitcoin::transaction::Version(1),
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn {
            previous_output: bitcoin::OutPoint::new(bitcoin::Txid::all_zeros(), u32::MAX),
            sequence: bitcoin::Sequence(1),
            witness: bitcoin::Witness::default(),
            script_sig: bitcoin::ScriptBuf::new(),
        }],
        output: vec![
            bitcoin::TxOut {
                value: bitcoin::Amount::from_sat(100_000),
                script_pubkey: bitcoin::ScriptBuf::new(),
            },
            bitcoin::TxOut {
                value: bitcoin::Amount::from_sat(100_000),
                script_pubkey: bitcoin::ScriptBuf::new(),
            },
            bitcoin::TxOut {
                value: bitcoin::Amount::from_sat(100_000),
                script_pubkey: bitcoin::ScriptBuf::new(),
            },
            bitcoin::TxOut {
                value: bitcoin::Amount::from_sat(100_000),
                script_pubkey: bitcoin::ScriptBuf::new(),
            },
            bitcoin::TxOut {
                value: bitcoin::Amount::from_sat(100_000),
                script_pubkey: bitcoin::ScriptBuf::new(),
            },
            bitcoin::TxOut {
                value: bitcoin::Amount::from_sat(100_000),
                script_pubkey: bitcoin::ScriptBuf::new(),
            },
        ],
    };
    let funding_txid_2 = funding_tx_2.compute_txid();

    // funding_tx_3 needs at least 2 outputs (index 1 is used by tx_b)
    let funding_tx_3 = Transaction {
        version: bitcoin::transaction::Version(1),
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn {
            previous_output: bitcoin::OutPoint::new(bitcoin::Txid::all_zeros(), u32::MAX),
            sequence: bitcoin::Sequence(2),
            witness: bitcoin::Witness::default(),
            script_sig: bitcoin::ScriptBuf::new(),
        }],
        output: vec![
            bitcoin::TxOut {
                value: bitcoin::Amount::from_sat(100_000),
                script_pubkey: bitcoin::ScriptBuf::new(),
            },
            bitcoin::TxOut {
                value: bitcoin::Amount::from_sat(100_000),
                script_pubkey: bitcoin::ScriptBuf::new(),
            },
        ],
    };
    let funding_txid_3 = funding_tx_3.compute_txid();

    let mut batch = Store::get_write_batch();
    store
        .add_sharechain_txs(
            &[
                ShareTransaction(funding_tx_1),
                ShareTransaction(funding_tx_2),
                ShareTransaction(funding_tx_3),
            ],
            1,
            &mut batch,
        )
        .unwrap();
    store.commit_batch(batch).unwrap();

    // Now create spending transactions that reference the funding outputs.
    let tx_a = Transaction {
        version: bitcoin::transaction::Version(1),
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn {
            previous_output: bitcoin::OutPoint::new(funding_txid_1, 0),
            ..Default::default()
        }],
        output: vec![
            bitcoin::TxOut {
                value: bitcoin::Amount::from_sat(100),
                script_pubkey: bitcoin::ScriptBuf::new(),
            },
            bitcoin::TxOut {
                value: bitcoin::Amount::from_sat(200),
                script_pubkey: bitcoin::ScriptBuf::new(),
            },
        ],
    };
    let tx_b = Transaction {
        version: bitcoin::transaction::Version(2),
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![
            bitcoin::TxIn {
                previous_output: bitcoin::OutPoint::new(funding_txid_2, 5),
                ..Default::default()
            },
            bitcoin::TxIn {
                previous_output: bitcoin::OutPoint::new(funding_txid_3, 1),
                ..Default::default()
            },
        ],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(300),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };
    let txid_a = tx_a.compute_txid();
    let txid_b = tx_b.compute_txid();

    let mut batch = Store::get_write_batch();
    store
        .add_sharechain_txs(
            &[
                ShareTransaction(tx_a.clone()),
                ShareTransaction(tx_b.clone()),
            ],
            2,
            &mut batch,
        )
        .unwrap();
    store.commit_batch(batch).unwrap();

    let loaded = store.get_txs(&[txid_a, txid_b]).unwrap();
    assert_eq!(loaded.len(), 2);
    assert_eq!(loaded[0], tx_a);
    assert_eq!(loaded[1], tx_b);

    // Order of the request drives the order of the result.
    let reversed = store.get_txs(&[txid_b, txid_a]).unwrap();
    assert_eq!(reversed[0], tx_b);
    assert_eq!(reversed[1], tx_a);
}

#[test]
fn test_get_txs_errors_when_any_txid_unknown() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let unknown_txid: bitcoin::Txid =
        bitcoin::hashes::sha256d::Hash::from_byte_array([0x99; 32]).into();
    let result = store.get_txs(&[unknown_txid]);
    assert!(result.is_err());
}

#[test]
fn test_get_all_prevouts_succeeds_for_stored_outputs() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // Create a funding transaction with two outputs
    let funding_tx = Transaction {
        version: bitcoin::transaction::Version(1),
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![],
        output: vec![
            bitcoin::TxOut {
                value: bitcoin::Amount::from_sat(1_000_000),
                script_pubkey: bitcoin::ScriptBuf::new(),
            },
            bitcoin::TxOut {
                value: bitcoin::Amount::from_sat(2_000_000),
                script_pubkey: bitcoin::ScriptBuf::new(),
            },
        ],
    };
    let funding_txid = funding_tx.compute_txid();

    let mut batch = Store::get_write_batch();
    store
        .add_sharechain_txs(&[ShareTransaction(funding_tx)], 1, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    // Build a spending transaction that consumes both outputs
    let spending_tx = Transaction {
        version: bitcoin::transaction::Version(1),
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![
            bitcoin::TxIn {
                previous_output: bitcoin::OutPoint::new(funding_txid, 0),
                ..Default::default()
            },
            bitcoin::TxIn {
                previous_output: bitcoin::OutPoint::new(funding_txid, 1),
                ..Default::default()
            },
        ],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(2_900_000),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };

    let prevouts = store.get_all_prevouts(&spending_tx).unwrap();
    assert_eq!(prevouts.len(), 2);
    assert_eq!(prevouts[0].0, 0);
    assert_eq!(prevouts[0].1.value, bitcoin::Amount::from_sat(1_000_000));
    assert_eq!(prevouts[1].0, 1);
    assert_eq!(prevouts[1].1.value, bitcoin::Amount::from_sat(2_000_000));
}

#[test]
fn test_get_all_prevouts_fails_for_missing_output() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let spending_tx = Transaction {
        version: bitcoin::transaction::Version(1),
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn {
            previous_output: bitcoin::OutPoint::new(Txid::all_zeros(), 0),
            ..Default::default()
        }],
        output: vec![],
    };

    let result = store.get_all_prevouts(&spending_tx);
    assert!(result.is_err());
}

#[test]
fn test_check_prevouts_accepts_empty_input() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
    let result = store.check_prevouts(&[], 0, 0, 0).unwrap();
    assert_eq!(result, PrevoutCheck::Accepted);
}

#[test]
fn test_check_prevouts_accepts_when_all_present() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let funding_tx = Transaction {
        version: bitcoin::transaction::Version(1),
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![],
        output: vec![
            bitcoin::TxOut {
                value: bitcoin::Amount::from_sat(1_000_000),
                script_pubkey: bitcoin::ScriptBuf::new(),
            },
            bitcoin::TxOut {
                value: bitcoin::Amount::from_sat(2_000_000),
                script_pubkey: bitcoin::ScriptBuf::new(),
            },
        ],
    };
    let funding_txid = funding_tx.compute_txid();

    let mut batch = Store::get_write_batch();
    store
        .add_sharechain_txs(&[ShareTransaction(funding_tx)], 1, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let outpoints = vec![
        bitcoin::OutPoint::new(funding_txid, 0),
        bitcoin::OutPoint::new(funding_txid, 1),
    ];
    let result = store.check_prevouts(&outpoints, 1, 0, 0).unwrap();
    assert_eq!(result, PrevoutCheck::Accepted);
}

#[test]
fn test_check_prevouts_rejects_when_one_missing() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let funding_tx = Transaction {
        version: bitcoin::transaction::Version(1),
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(1_000_000),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };
    let funding_txid = funding_tx.compute_txid();

    let mut batch = Store::get_write_batch();
    store
        .add_sharechain_txs(&[ShareTransaction(funding_tx)], 1, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let unknown_txid: bitcoin::Txid =
        bitcoin::hashes::sha256d::Hash::from_byte_array([7u8; 32]).into();
    let outpoints = vec![
        bitcoin::OutPoint::new(funding_txid, 0),
        bitcoin::OutPoint::new(unknown_txid, 0),
    ];
    let result = store
        .check_prevouts(&outpoints, 1, 0, 0)
        .expect("a missing output is a rejection, not a store error");
    assert_eq!(
        result,
        PrevoutCheck::Rejected(PrevoutRejection::MissingOutput(bitcoin::OutPoint::new(
            unknown_txid,
            0
        )))
    );
}

#[test]
fn test_check_prevouts_applies_maturity_to_coinbase_outputs_only() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let coinbase_tx = Transaction {
        version: bitcoin::transaction::Version(1),
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn {
            previous_output: bitcoin::OutPoint::new(bitcoin::Txid::all_zeros(), u32::MAX),
            sequence: bitcoin::Sequence::MAX,
            witness: bitcoin::Witness::default(),
            script_sig: bitcoin::ScriptBuf::new(),
        }],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(5_000_000),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };
    let coinbase_txid = coinbase_tx.compute_txid();

    let regular_tx = Transaction {
        version: bitcoin::transaction::Version(1),
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(1_000_000),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };
    let regular_txid = regular_tx.compute_txid();

    let mut batch = Store::get_write_batch();
    store
        .add_sharechain_txs(
            &[ShareTransaction(coinbase_tx), ShareTransaction(regular_tx)],
            1,
            &mut batch,
        )
        .unwrap();
    store.commit_batch(batch).unwrap();

    let coinbase_outpoint = bitcoin::OutPoint::new(coinbase_txid, 0);
    let regular_outpoint = bitcoin::OutPoint::new(regular_txid, 0);

    // Both transactions were stored at height 1, so spending at height 10
    // leaves the coinbase 9 blocks deep, far short of the 6048 required.
    assert_eq!(
        store
            .check_prevouts(&[coinbase_outpoint], 10, 0, 6048)
            .unwrap(),
        PrevoutCheck::Rejected(PrevoutRejection::ImmatureCoinbase {
            outpoint: coinbase_outpoint,
            coinbase_root_height: 1,
            spending_height: 10,
            required_depth: 6048,
        })
    );

    // The same depth on a non-coinbase output is not a maturity question.
    assert_eq!(
        store
            .check_prevouts(&[regular_outpoint], 10, 0, 6048)
            .unwrap(),
        PrevoutCheck::Accepted
    );

    // Exactly the required depth is mature.
    assert_eq!(
        store
            .check_prevouts(&[coinbase_outpoint], 6049, 0, 6048)
            .unwrap(),
        PrevoutCheck::Accepted
    );
}

#[test]
fn test_check_prevouts_rejects_expired_coinbase_root_height() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // Store a coinbase tx at height 50
    let coinbase_tx = Transaction {
        version: bitcoin::transaction::Version(1),
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn {
            previous_output: bitcoin::OutPoint::new(bitcoin::Txid::all_zeros(), u32::MAX),
            sequence: bitcoin::Sequence::MAX,
            witness: bitcoin::Witness::default(),
            script_sig: bitcoin::ScriptBuf::new(),
        }],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(5_000_000),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };
    let coinbase_txid = coinbase_tx.compute_txid();

    let mut batch = Store::get_write_batch();
    store
        .add_sharechain_txs(&[ShareTransaction(coinbase_tx)], 50, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let outpoints = vec![bitcoin::OutPoint::new(coinbase_txid, 0)];

    // min_coinbase_root_height = 100: output at height 50 is expired.
    // Maturity is set to 0 throughout so only the root-height rule can fire.
    let result = store
        .check_prevouts(&outpoints, 100, 100, 0)
        .expect("an expired coinbase root is a rejection, not a store error");
    assert_eq!(
        result,
        PrevoutCheck::Rejected(PrevoutRejection::CoinbaseRootTooOld {
            outpoint: bitcoin::OutPoint::new(coinbase_txid, 0),
            coinbase_root_height: 50,
            minimum_height: 100,
        })
    );

    // min_coinbase_root_height = 50: output at height 50 is exactly at boundary
    let result = store.check_prevouts(&outpoints, 100, 50, 0);
    assert_eq!(result.unwrap(), PrevoutCheck::Accepted);

    // min_coinbase_root_height = 0: no filtering
    let result = store.check_prevouts(&outpoints, 100, 0, 0);
    assert_eq!(result.unwrap(), PrevoutCheck::Accepted);
}

#[test]
fn test_check_prevouts_rejects_expired_root_through_in_block_spend_chain() {
    // A coinbase at height 50 is spent by spend1 and spend2 within
    // the same block at height 60. The chain is:
    //   coinbase(h:50) -> spend1(h:60) -> spend2(h:60)
    // Both spend1 and spend2 inherit coinbase_root_height = 50.
    // Checking spend2's output with min = 100 should reject it.
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // Coinbase at height 50 (stored in a previous block)
    let coinbase_tx = Transaction {
        version: bitcoin::transaction::Version(1),
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn {
            previous_output: bitcoin::OutPoint::new(bitcoin::Txid::all_zeros(), u32::MAX),
            sequence: bitcoin::Sequence::MAX,
            witness: bitcoin::Witness::default(),
            script_sig: bitcoin::ScriptBuf::new(),
        }],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(5_000_000),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };
    let coinbase_txid = coinbase_tx.compute_txid();

    let mut batch = Store::get_write_batch();
    store
        .add_sharechain_txs(&[ShareTransaction(coinbase_tx)], 50, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    // spend1 and spend2 in the same block at height 60
    let spend1 = Transaction {
        version: bitcoin::transaction::Version(1),
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn {
            previous_output: bitcoin::OutPoint::new(coinbase_txid, 0),
            sequence: bitcoin::Sequence::MAX,
            witness: bitcoin::Witness::default(),
            script_sig: bitcoin::ScriptBuf::new(),
        }],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(4_000_000),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };
    let spend1_txid = spend1.compute_txid();

    let spend2 = Transaction {
        version: bitcoin::transaction::Version(1),
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn {
            previous_output: bitcoin::OutPoint::new(spend1_txid, 0),
            sequence: bitcoin::Sequence::MAX,
            witness: bitcoin::Witness::default(),
            script_sig: bitcoin::ScriptBuf::new(),
        }],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(3_000_000),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };
    let spend2_txid = spend2.compute_txid();

    // Store both in the same block at height 60
    let mut batch = Store::get_write_batch();
    store
        .add_sharechain_txs(
            &[ShareTransaction(spend1), ShareTransaction(spend2)],
            60,
            &mut batch,
        )
        .unwrap();
    store.commit_batch(batch).unwrap();

    // spend2's output inherits coinbase_root_height = 50 from the
    // chain: coinbase(50) -> spend1(inherits 50) -> spend2(inherits 50)
    let outpoints = vec![bitcoin::OutPoint::new(spend2_txid, 0)];

    // min = 100: root height 50 is expired
    let result = store
        .check_prevouts(&outpoints, 100, 100, 0)
        .expect("an expired coinbase root is a rejection, not a store error");
    assert_eq!(
        result,
        PrevoutCheck::Rejected(PrevoutRejection::CoinbaseRootTooOld {
            outpoint: bitcoin::OutPoint::new(spend2_txid, 0),
            coinbase_root_height: 50,
            minimum_height: 100,
        })
    );

    // min = 50: root height 50 is at boundary, passes
    let result = store.check_prevouts(&outpoints, 100, 50, 0);
    assert_eq!(result.unwrap(), PrevoutCheck::Accepted);
}

#[test]
fn test_is_any_prevout_spent_returns_false_for_empty_input() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
    assert!(!store.is_any_prevout_spent(&[]).unwrap());
}

#[test]
fn test_is_any_prevout_spent_reflects_spends_index_presence() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let funding_txid: bitcoin::Txid =
        bitcoin::hashes::sha256d::Hash::from_byte_array([41u8; 32]).into();
    let spending_txid: bitcoin::Txid =
        bitcoin::hashes::sha256d::Hash::from_byte_array([42u8; 32]).into();

    // No SpendsIndex entries yet.
    assert!(
        !store
            .is_any_prevout_spent(&[bitcoin::OutPoint::new(funding_txid, 0)])
            .unwrap()
    );

    // Record vout 0 as spent via the confirmation-path helper.
    let mut batch = rocksdb::WriteBatch::default();
    store
        .add_spend(&funding_txid, 0, &spending_txid, 0, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    assert!(
        store
            .is_any_prevout_spent(&[bitcoin::OutPoint::new(funding_txid, 0)])
            .unwrap()
    );
    assert!(
        !store
            .is_any_prevout_spent(&[bitcoin::OutPoint::new(funding_txid, 1)])
            .unwrap()
    );
    // Mixed batch with one present should report true.
    assert!(
        store
            .is_any_prevout_spent(&[
                bitcoin::OutPoint::new(funding_txid, 1),
                bitcoin::OutPoint::new(funding_txid, 0),
            ])
            .unwrap()
    );
}

#[test]
fn test_are_all_txids_confirmed_returns_true_for_empty_input() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
    assert!(store.are_all_txids_confirmed(&[]).unwrap());
}

#[test]
fn test_are_all_txids_confirmed_true_when_all_confirmed() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let txid_a: bitcoin::Txid = bitcoin::hashes::sha256d::Hash::from_byte_array([1u8; 32]).into();
    let txid_b: bitcoin::Txid = bitcoin::hashes::sha256d::Hash::from_byte_array([2u8; 32]).into();

    let blockhash = TestShareBlockBuilder::new().build().block_hash();
    let mut batch = Store::get_write_batch();
    store
        .update_block_metadata(
            &blockhash,
            &BlockMetadata {
                expected_height: Some(1),
                chain_work: Work::from_le_bytes([1u8; 32]),
                status: Status::BlockValid,
                chain: ChainMembership::Confirmed,
            },
            &mut batch,
        )
        .unwrap();
    store
        .add_txids_to_blocks_index(&blockhash, &Txids(vec![txid_a, txid_b]), &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    assert!(store.are_all_txids_confirmed(&[txid_a, txid_b]).unwrap());
}

#[test]
fn test_are_all_txids_confirmed_false_when_any_uncle_only() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let txid_confirmed: bitcoin::Txid =
        bitcoin::hashes::sha256d::Hash::from_byte_array([3u8; 32]).into();
    let txid_uncle_only: bitcoin::Txid =
        bitcoin::hashes::sha256d::Hash::from_byte_array([4u8; 32]).into();

    let confirmed_blockhash = TestShareBlockBuilder::new()
        .nonce(0xaaaa0001)
        .build()
        .block_hash();
    let uncle_blockhash = TestShareBlockBuilder::new()
        .nonce(0xaaaa0002)
        .build()
        .block_hash();

    let mut batch = Store::get_write_batch();
    store
        .update_block_metadata(
            &confirmed_blockhash,
            &BlockMetadata {
                expected_height: Some(1),
                chain_work: Work::from_le_bytes([1u8; 32]),
                status: Status::BlockValid,
                chain: ChainMembership::Confirmed,
            },
            &mut batch,
        )
        .unwrap();
    store
        .update_block_metadata(
            &uncle_blockhash,
            &BlockMetadata {
                expected_height: Some(1),
                chain_work: Work::from_le_bytes([1u8; 32]),
                status: Status::HeaderValid,
                chain: ChainMembership::Candidate,
            },
            &mut batch,
        )
        .unwrap();
    store
        .add_txids_to_blocks_index(
            &confirmed_blockhash,
            &Txids(vec![txid_confirmed]),
            &mut batch,
        )
        .unwrap();
    store
        .add_txids_to_blocks_index(&uncle_blockhash, &Txids(vec![txid_uncle_only]), &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    assert!(store.are_all_txids_confirmed(&[txid_confirmed]).unwrap());
    assert!(!store.are_all_txids_confirmed(&[txid_uncle_only]).unwrap());
    assert!(
        !store
            .are_all_txids_confirmed(&[txid_confirmed, txid_uncle_only])
            .unwrap()
    );
}

#[test]
fn test_are_all_txids_confirmed_false_when_unknown() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
    let unknown_txid: bitcoin::Txid =
        bitcoin::hashes::sha256d::Hash::from_byte_array([9u8; 32]).into();
    assert!(!store.are_all_txids_confirmed(&[unknown_txid]).unwrap());
}

fn confirmed_metadata(height: u32) -> BlockMetadata {
    BlockMetadata {
        expected_height: Some(height),
        chain_work: Work::from_le_bytes([1u8; 32]),
        status: Status::BlockValid,
        chain: ChainMembership::Confirmed,
    }
}

#[test]
fn test_are_all_txids_confirmed_excluding_drops_only_confirming_block() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let source_txid: bitcoin::Txid =
        bitcoin::hashes::sha256d::Hash::from_byte_array([11u8; 32]).into();
    let blockhash = TestShareBlockBuilder::new().build().block_hash();

    let mut batch = Store::get_write_batch();
    store
        .update_block_metadata(&blockhash, &confirmed_metadata(1), &mut batch)
        .unwrap();
    store
        .add_txids_to_blocks_index(&blockhash, &Txids(vec![source_txid]), &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    // Not excluded: confirmed.
    assert!(
        store
            .are_all_txids_confirmed_with_overlay(&[source_txid], &HashSet::new(), &HashSet::new())
            .unwrap()
    );
    // The only confirming block excluded: no longer confirmed.
    let excluded = HashSet::from([blockhash]);
    assert!(
        !store
            .are_all_txids_confirmed_with_overlay(&[source_txid], &excluded, &HashSet::new())
            .unwrap()
    );
}

#[test]
fn test_are_all_txids_confirmed_excluding_keeps_surviving_block() {
    // The same txid is included in two confirmed blocks; excluding one still
    // leaves it confirmed via the surviving block (block-level exclusion).
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let source_txid: bitcoin::Txid =
        bitcoin::hashes::sha256d::Hash::from_byte_array([12u8; 32]).into();
    let reorged_out = TestShareBlockBuilder::new()
        .nonce(0xbb01)
        .build()
        .block_hash();
    let surviving = TestShareBlockBuilder::new()
        .nonce(0xbb02)
        .build()
        .block_hash();

    let mut batch = Store::get_write_batch();
    store
        .update_block_metadata(&reorged_out, &confirmed_metadata(1), &mut batch)
        .unwrap();
    store
        .update_block_metadata(&surviving, &confirmed_metadata(1), &mut batch)
        .unwrap();
    store
        .add_txids_to_blocks_index(&reorged_out, &Txids(vec![source_txid]), &mut batch)
        .unwrap();
    store
        .add_txids_to_blocks_index(&surviving, &Txids(vec![source_txid]), &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let excluded = HashSet::from([reorged_out]);
    assert!(
        store
            .are_all_txids_confirmed_with_overlay(&[source_txid], &excluded, &HashSet::new())
            .unwrap()
    );
}

#[test]
fn test_is_any_prevout_spent_with_overlay_masks_and_adds() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let funding_txid: bitcoin::Txid =
        bitcoin::hashes::sha256d::Hash::from_byte_array([21u8; 32]).into();
    let spending_txid: bitcoin::Txid =
        bitcoin::hashes::sha256d::Hash::from_byte_array([22u8; 32]).into();
    let committed = bitcoin::OutPoint::new(funding_txid, 0);
    let batch_only = bitcoin::OutPoint::new(funding_txid, 1);

    let mut batch = Store::get_write_batch();
    store
        .add_spend(&funding_txid, 0, &spending_txid, 0, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let empty = HashSet::new();
    // Committed spend, no overlay: spent.
    assert!(
        store
            .is_any_prevout_spent_with_overlay(&[committed], &empty, &empty)
            .unwrap()
    );
    // removed_spends masks the committed spend: unspent.
    let removed = HashSet::from([committed]);
    assert!(
        !store
            .is_any_prevout_spent_with_overlay(&[committed], &removed, &empty)
            .unwrap()
    );
    // spent_in_batch reports an otherwise-unspent outpoint as spent.
    let in_batch = HashSet::from([batch_only]);
    assert!(
        store
            .is_any_prevout_spent_with_overlay(&[batch_only], &empty, &in_batch)
            .unwrap()
    );
    // Neither committed, removed, nor in-batch: unspent.
    assert!(
        !store
            .is_any_prevout_spent_with_overlay(&[batch_only], &empty, &empty)
            .unwrap()
    );
}

#[test]
fn test_accumulate_spent_outpoints_records_non_coinbase_inputs() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // Fund a coinbase output so the spender's prevout exists in the store.
    let funding_tx = Transaction {
        version: bitcoin::transaction::Version::ONE,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn {
            previous_output: bitcoin::OutPoint::null(),
            script_sig: bitcoin::ScriptBuf::new(),
            sequence: bitcoin::Sequence::MAX,
            witness: bitcoin::Witness::new(),
        }],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(50_000),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };
    let funding_txid = funding_tx.compute_txid();
    let mut batch = Store::get_write_batch();
    store
        .add_sharechain_txs(&[ShareTransaction(funding_tx)], 0, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let prevout = bitcoin::OutPoint::new(funding_txid, 0);
    let spending_tx = Transaction {
        version: bitcoin::transaction::Version::ONE,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn {
            previous_output: prevout,
            script_sig: bitcoin::ScriptBuf::new(),
            sequence: bitcoin::Sequence::MAX,
            witness: bitcoin::Witness::new(),
        }],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(10_000),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };
    let block = TestShareBlockBuilder::new()
        .add_transaction(spending_tx)
        .build();
    let blockhash = block.block_hash();

    let mut batch = Store::get_write_batch();
    store.add_share_block(&block, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let mut spent = HashSet::new();
    store
        .accumulate_spent_outpoints(&blockhash, &mut spent)
        .unwrap();

    // The spender's prevout is recorded; the coinbase's null input is not.
    assert_eq!(spent, HashSet::from([prevout]));
}

#[test]
fn test_accumulate_spent_outpoints_noop_for_missing_body() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
    let missing = TestShareBlockBuilder::new().build().block_hash();

    let mut spent = HashSet::new();
    store
        .accumulate_spent_outpoints(&missing, &mut spent)
        .unwrap();

    assert!(spent.is_empty());
}

#[test]
fn test_recheck_block_prevouts_true_for_missing_body() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
    let header_only = TestShareBlockBuilder::new().build().block_hash();

    // A prune-zone header-only block has no body / no prevouts to check.
    assert!(
        store
            .recheck_block_prevouts_with_overlay(&header_only, &ConfirmationOverlay::default())
            .unwrap()
    );
}

#[test]
fn test_recheck_block_prevouts_with_overlay_conditions() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // A confirmed source output, indexed to a confirmed producing block.
    let funding_tx = Transaction {
        version: bitcoin::transaction::Version::ONE,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn {
            previous_output: bitcoin::OutPoint::null(),
            script_sig: bitcoin::ScriptBuf::new(),
            sequence: bitcoin::Sequence::MAX,
            witness: bitcoin::Witness::new(),
        }],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(50_000),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };
    let source_txid = funding_tx.compute_txid();
    let source_outpoint = bitcoin::OutPoint::new(source_txid, 0);
    let producing_hash = TestShareBlockBuilder::new()
        .nonce(0xdd01)
        .build()
        .block_hash();
    let mut batch = Store::get_write_batch();
    store
        .add_sharechain_txs(&[ShareTransaction(funding_tx)], 0, &mut batch)
        .unwrap();
    store
        .update_block_metadata(&producing_hash, &confirmed_metadata(1), &mut batch)
        .unwrap();
    store
        .add_txids_to_blocks_index(&producing_hash, &Txids(vec![source_txid]), &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    // A stored (unconfirmed) spender block that spends the source. Storing a
    // block does not touch SpendsIndex, so the source is still unspent.
    let spending_tx = Transaction {
        version: bitcoin::transaction::Version::ONE,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn {
            previous_output: source_outpoint,
            script_sig: bitcoin::ScriptBuf::new(),
            sequence: bitcoin::Sequence::MAX,
            witness: bitcoin::Witness::new(),
        }],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(10_000),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };
    let spender_block = TestShareBlockBuilder::new()
        .nonce(0xdd02)
        .add_transaction(spending_tx)
        .build();
    let spender_hash = spender_block.block_hash();
    let mut batch = Store::get_write_batch();
    store.add_share_block(&spender_block, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Source confirmed and unspent: confirmable.
    assert!(
        store
            .recheck_block_prevouts_with_overlay(&spender_hash, &ConfirmationOverlay::default())
            .unwrap()
    );

    // The source's only confirmed block is reorged out this batch: its
    // source txid is no longer confirmed.
    let overlay = ConfirmationOverlay {
        removed_blockhashes: HashSet::from([producing_hash]),
        ..Default::default()
    };
    assert!(
        !store
            .recheck_block_prevouts_with_overlay(&spender_hash, &overlay)
            .unwrap()
    );

    // The source was spent by an earlier block in this batch: double-spend.
    let overlay = ConfirmationOverlay {
        spent_in_batch: HashSet::from([source_outpoint]),
        ..Default::default()
    };
    assert!(
        !store
            .recheck_block_prevouts_with_overlay(&spender_hash, &overlay)
            .unwrap()
    );

    // Record a committed spend of the source (as if a confirmed block spent
    // it). With no overlay it is now rejected.
    let other_spender: bitcoin::Txid =
        bitcoin::hashes::sha256d::Hash::from_byte_array([55u8; 32]).into();
    let mut batch = Store::get_write_batch();
    store
        .add_spend(&source_txid, 0, &other_spender, 0, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();
    assert!(
        !store
            .recheck_block_prevouts_with_overlay(&spender_hash, &ConfirmationOverlay::default())
            .unwrap()
    );

    // If the rewind removes that committed spend, the source is free again
    // and the spender is confirmable.
    let overlay = ConfirmationOverlay {
        removed_spends: HashSet::from([source_outpoint]),
        ..Default::default()
    };
    assert!(
        store
            .recheck_block_prevouts_with_overlay(&spender_hash, &overlay)
            .unwrap()
    );
}

/// A block may spend an output introduced by a block confirmed earlier in
/// the same batch. `confirm_blocks` runs after the prefix walk, so the
/// source block still carries its committed membership while the walk is
/// in progress; without the `confirmed_in_batch` side of the overlay the
/// spender is rejected and, on the success path, marked Invalid.
#[test]
fn test_confirmable_prefix_accepts_spend_of_source_confirmed_in_same_batch() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xd000).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let funding_tx = Transaction {
        version: bitcoin::transaction::Version::ONE,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn {
            previous_output: bitcoin::OutPoint::null(),
            script_sig: bitcoin::ScriptBuf::new(),
            sequence: bitcoin::Sequence::MAX,
            witness: bitcoin::Witness::new(),
        }],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(50_000),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };
    let funding_txid = funding_tx.compute_txid();
    let source_block = TestShareBlockBuilder::new()
        .nonce(0xdd01)
        .prev_share_blockhash(genesis.block_hash().to_string())
        .add_transaction(funding_tx)
        .build();

    let spending_tx = Transaction {
        version: bitcoin::transaction::Version::ONE,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn {
            previous_output: bitcoin::OutPoint::new(funding_txid, 0),
            script_sig: bitcoin::ScriptBuf::new(),
            sequence: bitcoin::Sequence::MAX,
            witness: bitcoin::Witness::new(),
        }],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(10_000),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };
    let spender_block = TestShareBlockBuilder::new()
        .nonce(0xdd02)
        .prev_share_blockhash(source_block.block_hash().to_string())
        .add_transaction(spending_tx)
        .build();

    // Both blocks have bodies but neither is confirmed yet: this is the
    // state a reorg back onto a previously-confirmed branch leaves.
    store.store_with_valid_metadata(&source_block);
    store.store_with_valid_metadata(&spender_block);

    let blocks = vec![
        (1u32, source_block.block_hash()),
        (2u32, spender_block.block_hash()),
    ];
    let mut overlay = ConfirmationOverlay::default();
    let (prefix_len, first_invalid) = store.confirmable_prefix(&blocks, &mut overlay).unwrap();

    assert_eq!(
        prefix_len, 2,
        "the spender's source is confirmed by this batch"
    );
    assert!(first_invalid.is_none());
    assert!(
        overlay
            .confirmed_in_batch
            .contains(&source_block.block_hash())
    );
}

/// The confirmed-in-batch set is built as blocks are accepted, so a block
/// may only spend from blocks *earlier* in the batch. Presented in the
/// wrong order the spender is still rejected -- seeding the set from the
/// whole branch up front would wrongly accept it.
#[test]
fn test_confirmable_prefix_rejects_spend_of_source_later_in_batch() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let genesis = TestShareBlockBuilder::new().nonce(0xd000).build();
    let mut batch = Store::get_write_batch();
    store.setup_genesis(&genesis, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let funding_tx = Transaction {
        version: bitcoin::transaction::Version::ONE,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn {
            previous_output: bitcoin::OutPoint::null(),
            script_sig: bitcoin::ScriptBuf::new(),
            sequence: bitcoin::Sequence::MAX,
            witness: bitcoin::Witness::new(),
        }],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(50_000),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };
    let funding_txid = funding_tx.compute_txid();
    let source_block = TestShareBlockBuilder::new()
        .nonce(0xde01)
        .prev_share_blockhash(genesis.block_hash().to_string())
        .add_transaction(funding_tx)
        .build();

    let spending_tx = Transaction {
        version: bitcoin::transaction::Version::ONE,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn {
            previous_output: bitcoin::OutPoint::new(funding_txid, 0),
            script_sig: bitcoin::ScriptBuf::new(),
            sequence: bitcoin::Sequence::MAX,
            witness: bitcoin::Witness::new(),
        }],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(10_000),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };
    let spender_block = TestShareBlockBuilder::new()
        .nonce(0xde02)
        .prev_share_blockhash(source_block.block_hash().to_string())
        .add_transaction(spending_tx)
        .build();

    store.store_with_valid_metadata(&source_block);
    store.store_with_valid_metadata(&spender_block);

    // Spender first: its source has not been accepted yet.
    let blocks = vec![
        (1u32, spender_block.block_hash()),
        (2u32, source_block.block_hash()),
    ];
    let mut overlay = ConfirmationOverlay::default();
    let (prefix_len, first_invalid) = store.confirmable_prefix(&blocks, &mut overlay).unwrap();

    assert_eq!(prefix_len, 0);
    assert_eq!(first_invalid, Some(spender_block.block_hash()));
}

#[test]
fn test_confirmable_prefix_accepts_all_when_no_bodies() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
    let blocks = vec![
        (
            1u32,
            TestShareBlockBuilder::new()
                .nonce(0xcc01)
                .build()
                .block_hash(),
        ),
        (
            2u32,
            TestShareBlockBuilder::new()
                .nonce(0xcc02)
                .build()
                .block_hash(),
        ),
    ];

    let mut overlay = ConfirmationOverlay::default();
    let (prefix_len, first_invalid) = store.confirmable_prefix(&blocks, &mut overlay).unwrap();

    // Header-only blocks all pass the prevout re-check.
    assert_eq!(prefix_len, 2);
    assert!(first_invalid.is_none());
    assert!(overlay.spent_in_batch.is_empty());
}

#[test]
fn test_get_output_succeeds_for_stored_output() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let tx = Transaction {
        version: bitcoin::transaction::Version(1),
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![],
        output: vec![
            bitcoin::TxOut {
                value: bitcoin::Amount::from_sat(1_000_000),
                script_pubkey: bitcoin::ScriptBuf::new(),
            },
            bitcoin::TxOut {
                value: bitcoin::Amount::from_sat(2_000_000),
                script_pubkey: bitcoin::ScriptBuf::new(),
            },
        ],
    };

    let txid = tx.compute_txid();
    let mut batch = Store::get_write_batch();
    store
        .add_sharechain_txs(&[ShareTransaction(tx)], 1, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let output_0 = store.get_output(&txid, 0).unwrap();
    assert_eq!(output_0.value, bitcoin::Amount::from_sat(1_000_000));

    let output_1 = store.get_output(&txid, 1).unwrap();
    assert_eq!(output_1.value, bitcoin::Amount::from_sat(2_000_000));

    let missing = store.get_output(&txid, 99);
    assert!(missing.is_err());
}

#[test]
fn test_transaction_store_for_nonexistent_transaction_should_fail() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let nonexistent_txid = Txid::all_zeros();
    let result = store.get_tx(&nonexistent_txid);
    assert!(result.is_err());
}

#[test]
fn test_store_retrieve_txids_by_blockhash_index() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
    let block = TestShareBlockBuilder::new().build();
    let blockhash = block.block_hash();

    let mut batch = Store::get_write_batch();
    store.add_share_block(&block, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Verify block -> txids index (forward lookup)
    let txids = store.get_txids_for_blockhash(&blockhash);
    assert_eq!(txids.0.len(), block.transactions.len());

    // Verify txid -> block index (reverse lookup via add_txids_to_blocks_index)
    for tx in &block.transactions {
        let txid = tx.compute_txid();
        let retrieved_blockhashes = store.get_blockhashes_for_txid(&txid).unwrap();
        assert_eq!(retrieved_blockhashes.len(), 1);
        assert_eq!(
            retrieved_blockhashes[0], blockhash,
            "txid {txid} should map to blockhash {blockhash}"
        );
    }

    // Verify a non-existent txid returns empty vector
    let nonexistent_txid = Txid::all_zeros();
    let result = store.get_blockhashes_for_txid(&nonexistent_txid).unwrap();
    assert!(result.is_empty());
}

#[test]
fn test_txid_to_blocks_index_supports_multiple_blockhashes() {
    // Test that a txid can be associated with multiple blockhashes,
    // simulating the scenario where competing miners include the same
    // transaction in different blocks.
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // Create a transaction that will be included in multiple blocks
    let shared_tx = Transaction {
        version: bitcoin::transaction::Version(1),
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(1000000),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };
    let shared_txid = shared_tx.compute_txid();

    // Create first block containing the shared transaction
    let block1 = TestShareBlockBuilder::new()
        .nonce(0xe9695791)
        .add_transaction(shared_tx.clone())
        .build();
    let blockhash1 = block1.block_hash();

    let mut batch = Store::get_write_batch();
    store.add_share_block(&block1, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Verify the txid maps to the first blockhash
    let blockhashes = store.get_blockhashes_for_txid(&shared_txid).unwrap();
    assert_eq!(blockhashes.len(), 1);
    assert!(blockhashes.contains(&blockhash1));

    // Create second block (different nonce) containing the same transaction
    // This simulates a competing miner including the same tx
    let block2 = TestShareBlockBuilder::new()
        .nonce(0xe9695792)
        .add_transaction(shared_tx.clone())
        .build();
    let blockhash2 = block2.block_hash();
    assert_ne!(
        blockhash1, blockhash2,
        "blocks should have different hashes"
    );

    let mut batch = Store::get_write_batch();
    store.add_share_block(&block2, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Verify the txid now maps to both blockhashes
    let blockhashes = store.get_blockhashes_for_txid(&shared_txid).unwrap();
    assert_eq!(blockhashes.len(), 2, "txid should map to 2 blockhashes");
    assert!(
        blockhashes.contains(&blockhash1),
        "should contain first blockhash"
    );
    assert!(
        blockhashes.contains(&blockhash2),
        "should contain second blockhash"
    );

    // Create third block with the same transaction
    let block3 = TestShareBlockBuilder::new()
        .nonce(0xe9695793)
        .add_transaction(shared_tx.clone())
        .build();
    let blockhash3 = block3.block_hash();

    let mut batch = Store::get_write_batch();
    store.add_share_block(&block3, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Verify the txid now maps to all three blockhashes
    let blockhashes = store.get_blockhashes_for_txid(&shared_txid).unwrap();
    assert_eq!(blockhashes.len(), 3, "txid should map to 3 blockhashes");
    assert!(blockhashes.contains(&blockhash1));
    assert!(blockhashes.contains(&blockhash2));
    assert!(blockhashes.contains(&blockhash3));
}

#[test]
fn test_txid_to_blocks_index_deduplicates_blockhashes() {
    // Test that adding the same blockhash for a txid multiple times
    // doesn't create duplicates (merge operator should handle this)
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let tx = Transaction {
        version: bitcoin::transaction::Version(1),
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![],
        output: vec![],
    };
    let txid = tx.compute_txid();
    let txids = Txids(vec![txid]);

    let blockhash = BlockHash::from_byte_array([1u8; 32]);

    // Add the same blockhash for the txid multiple times
    let mut batch = Store::get_write_batch();
    store
        .add_txids_to_blocks_index(&blockhash, &txids, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let mut batch = Store::get_write_batch();
    store
        .add_txids_to_blocks_index(&blockhash, &txids, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    let mut batch = Store::get_write_batch();
    store
        .add_txids_to_blocks_index(&blockhash, &txids, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    // Should still only have 1 blockhash (no duplicates)
    let blockhashes = store.get_blockhashes_for_txid(&txid).unwrap();
    assert_eq!(
        blockhashes.len(),
        1,
        "should have only 1 blockhash (deduplication)"
    );
    assert_eq!(blockhashes[0], blockhash);
}

#[test]
fn test_add_tx_metadata_with_no_inputs_or_outputs_should_succeed() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let tx = Transaction {
        version: bitcoin::transaction::Version::ONE,
        lock_time: bitcoin::locktime::absolute::LockTime::ZERO,
        input: vec![],
        output: vec![],
    };

    let mut batch = Store::get_write_batch();
    let txid = tx.compute_txid();
    let result = store.add_tx_metadata(txid, &tx, false, &mut batch);
    store.commit_batch(batch).unwrap();

    assert!(result.is_ok());
    let metadata = result.unwrap();
    assert_eq!(metadata.txid, txid);
    assert_eq!(metadata.input_count, 0);
    assert_eq!(metadata.output_count, 0);
    assert!(!metadata.validated);
}

#[test]
fn test_mark_transaction_valid() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let tx = Transaction {
        version: bitcoin::transaction::Version::ONE,
        lock_time: bitcoin::locktime::absolute::LockTime::ZERO,
        input: vec![],
        output: vec![],
    };

    let txid = tx.compute_txid();

    // Add tx metadata first
    let mut batch = Store::get_write_batch();
    store.add_tx_metadata(txid, &tx, false, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Verify it's not validated
    let metadata = store.get_tx_metadata(&txid).unwrap();
    assert!(!metadata.validated);

    // Mark as valid
    let mut batch = Store::get_write_batch();
    store.mark_transaction_valid(&txid, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    // Verify it's now validated
    let metadata = store.get_tx_metadata(&txid).unwrap();
    assert!(metadata.validated);
}

#[test]
fn test_add_txs_should_succeed() {
    // Create a new store with a temporary path
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // Create test transactions
    let tx1 = Transaction {
        version: bitcoin::transaction::Version(1),
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![],
        output: vec![],
    };

    let tx2 = Transaction {
        version: bitcoin::transaction::Version(1),
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![],
        output: vec![],
    };

    let transactions = vec![ShareTransaction(tx1.clone()), ShareTransaction(tx2.clone())];

    // Add transactions to store
    let mut batch = rocksdb::WriteBatch::default();
    store
        .add_sharechain_txs(&transactions, 1, &mut batch)
        .unwrap();
    store.db.write(batch).unwrap();

    // Verify transactions were stored correctly by retrieving them by txid
    let tx1_id = tx1.compute_txid();
    let tx2_id = tx2.compute_txid();

    assert_eq!(store.get_tx(&tx1_id).unwrap(), tx1);
    assert_eq!(store.get_tx(&tx2_id).unwrap(), tx2);
}

#[test]
fn test_add_txs_with_inputs_or_outputs_should_succeed() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let script_true = bitcoin::Script::builder()
        .push_opcode(bitcoin::opcodes::all::OP_PUSHNUM_1)
        .into_script();

    // Store a funding coinbase tx so the spending tx can find its input
    let funding_tx = Transaction {
        version: bitcoin::transaction::Version(1),
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn {
            previous_output: bitcoin::OutPoint::new(bitcoin::Txid::all_zeros(), u32::MAX),
            sequence: bitcoin::Sequence::default(),
            witness: bitcoin::Witness::default(),
            script_sig: script_true.clone(),
        }],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(1000000),
            script_pubkey: script_true.clone(),
        }],
    };
    let funding_txid = funding_tx.compute_txid();
    let mut batch = rocksdb::WriteBatch::default();
    store
        .add_sharechain_txs(&[ShareTransaction(funding_tx)], 1, &mut batch)
        .unwrap();
    store.db.write(batch).unwrap();

    let tx = Transaction {
        version: bitcoin::transaction::Version(1),
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn {
            previous_output: bitcoin::OutPoint::new(funding_txid, 0),
            sequence: bitcoin::Sequence::default(),
            witness: bitcoin::Witness::default(),
            script_sig: script_true.clone(),
        }],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(1000000),
            script_pubkey: script_true.clone(),
        }],
    };

    let txid = tx.compute_txid();
    let mut batch = rocksdb::WriteBatch::default();
    let res = store
        .add_sharechain_txs(&[ShareTransaction(tx.clone())], 2, &mut batch)
        .unwrap();
    store.db.write(batch).unwrap();

    assert_eq!(res[0].txid, txid);

    let tx = store.get_tx(&txid).unwrap();
    assert_eq!(tx.version, tx.version);
    assert_eq!(tx.lock_time, tx.lock_time);
    assert_eq!(tx.input.len(), 1);
    assert_eq!(
        tx.input[0].previous_output,
        bitcoin::OutPoint::new(funding_txid, 0)
    );
    assert_eq!(tx.input[0].script_sig, script_true);
    assert_eq!(tx.output.len(), 1);
    assert_eq!(tx.output[0].value, bitcoin::Amount::from_sat(1000000));
    assert_eq!(tx.output[0].script_pubkey, script_true);
}

#[test]
fn test_remove_spend_clears_spends_index_entry() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let prev_txid: bitcoin::Txid =
        bitcoin::hashes::sha256d::Hash::from_byte_array([11u8; 32]).into();
    let spending_txid: bitcoin::Txid =
        bitcoin::hashes::sha256d::Hash::from_byte_array([12u8; 32]).into();

    let mut batch = rocksdb::WriteBatch::default();
    store
        .add_spend(&prev_txid, 0, &spending_txid, 0, &mut batch)
        .unwrap();
    store.db.write(batch).unwrap();
    assert!(store.is_spent(&prev_txid, 0).unwrap().is_some());

    let mut batch = rocksdb::WriteBatch::default();
    store.remove_spend(&prev_txid, 0, &mut batch);
    store.db.write(batch).unwrap();
    assert!(store.is_spent(&prev_txid, 0).unwrap().is_none());
}

#[test]
fn test_add_spends_for_block_populates_non_coinbase_inputs() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let funding_txid: bitcoin::Txid =
        bitcoin::hashes::sha256d::Hash::from_byte_array([21u8; 32]).into();
    let other_txid: bitcoin::Txid =
        bitcoin::hashes::sha256d::Hash::from_byte_array([22u8; 32]).into();

    let coinbase_tx = Transaction {
        version: bitcoin::transaction::Version(1),
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn {
            previous_output: bitcoin::OutPoint::null(),
            ..Default::default()
        }],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(5_000_000_000),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };

    let spending_tx = Transaction {
        version: bitcoin::transaction::Version(1),
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![
            bitcoin::TxIn {
                previous_output: bitcoin::OutPoint::new(funding_txid, 0),
                ..Default::default()
            },
            bitcoin::TxIn {
                previous_output: bitcoin::OutPoint::new(other_txid, 3),
                ..Default::default()
            },
        ],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(900_000),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };
    let spending_txid = spending_tx.compute_txid();

    let mut batch = rocksdb::WriteBatch::default();
    store
        .add_spends_for_block(
            &[ShareTransaction(coinbase_tx), ShareTransaction(spending_tx)],
            &mut batch,
        )
        .unwrap();
    store.db.write(batch).unwrap();

    let spent_a = store.is_spent(&funding_txid, 0).unwrap().unwrap();
    assert_eq!(spent_a.txid, spending_txid);
    assert_eq!(spent_a.vout, 0);

    let spent_b = store.is_spent(&other_txid, 3).unwrap().unwrap();
    assert_eq!(spent_b.txid, spending_txid);
    assert_eq!(spent_b.vout, 1);

    // Coinbase input's null prevout must not have been written.
    assert!(
        store
            .is_spent(&bitcoin::Txid::all_zeros(), u32::MAX)
            .unwrap()
            .is_none()
    );
}

#[test]
fn test_remove_spends_for_block_clears_every_non_coinbase_input() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let funding_txid: bitcoin::Txid =
        bitcoin::hashes::sha256d::Hash::from_byte_array([31u8; 32]).into();

    let spending_tx = Transaction {
        version: bitcoin::transaction::Version(1),
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![
            bitcoin::TxIn {
                previous_output: bitcoin::OutPoint::new(funding_txid, 0),
                ..Default::default()
            },
            bitcoin::TxIn {
                previous_output: bitcoin::OutPoint::new(funding_txid, 1),
                ..Default::default()
            },
        ],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(900_000),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };
    let txs = vec![ShareTransaction(spending_tx)];

    let mut batch = rocksdb::WriteBatch::default();
    store.add_spends_for_block(&txs, &mut batch).unwrap();
    store.db.write(batch).unwrap();
    assert!(store.is_spent(&funding_txid, 0).unwrap().is_some());
    assert!(store.is_spent(&funding_txid, 1).unwrap().is_some());

    let mut batch = rocksdb::WriteBatch::default();
    store.remove_spends_for_block(&txs, &mut batch);
    store.db.write(batch).unwrap();
    assert!(store.is_spent(&funding_txid, 0).unwrap().is_none());
    assert!(store.is_spent(&funding_txid, 1).unwrap().is_none());
}

#[test]
fn test_add_spends_for_block_is_noop_for_coinbase_only() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let coinbase_tx = Transaction {
        version: bitcoin::transaction::Version(1),
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn {
            previous_output: bitcoin::OutPoint::null(),
            ..Default::default()
        }],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(5_000_000_000),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };

    let mut batch = rocksdb::WriteBatch::default();
    store
        .add_spends_for_block(&[ShareTransaction(coinbase_tx)], &mut batch)
        .unwrap();
    store.db.write(batch).unwrap();

    assert!(
        store
            .is_spent(&bitcoin::Txid::all_zeros(), u32::MAX)
            .unwrap()
            .is_none()
    );
}

#[test]
fn test_get_blockhashes_for_all_txids_returns_empty_for_empty_input() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();
    let result = store.get_blockhashes_for_all_txids(&[]).unwrap();
    assert!(result.is_empty());
}

#[test]
fn test_get_blockhashes_for_all_txids_returns_blockhashes_for_known_txids() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let block = TestShareBlockBuilder::new().build();
    let blockhash = block.block_hash();

    let mut batch = Store::get_write_batch();
    store.add_share_block(&block, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let txids: Vec<Txid> = block
        .transactions
        .iter()
        .map(|tx| tx.compute_txid())
        .collect();
    let result = store.get_blockhashes_for_all_txids(&txids).unwrap();

    assert_eq!(result.len(), txids.len());
    for blockhashes in &result {
        assert_eq!(blockhashes.len(), 1);
        assert_eq!(blockhashes[0], blockhash);
    }
}

#[test]
fn test_get_blockhashes_for_all_txids_returns_empty_vec_for_unknown_txids() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let unknown_txid_a = Txid::all_zeros();
    let unknown_txid_b = Txid::from_byte_array([1u8; 32]);
    let result = store
        .get_blockhashes_for_all_txids(&[unknown_txid_a, unknown_txid_b])
        .unwrap();

    assert_eq!(result.len(), 2);
    assert!(result[0].is_empty());
    assert!(result[1].is_empty());
}

#[test]
fn test_get_blockhashes_for_all_txids_mixed_known_and_unknown() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let block = TestShareBlockBuilder::new().build();
    let blockhash = block.block_hash();
    let known_txid = block.transactions[0].compute_txid();

    let mut batch = Store::get_write_batch();
    store.add_share_block(&block, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let unknown_txid = Txid::from_byte_array([0xffu8; 32]);
    let result = store
        .get_blockhashes_for_all_txids(&[known_txid, unknown_txid])
        .unwrap();

    assert_eq!(result.len(), 2);
    assert_eq!(result[0].len(), 1);
    assert_eq!(result[0][0], blockhash);
    assert!(result[1].is_empty());
}

#[test]
fn test_get_blockhashes_for_all_txids_txid_in_multiple_blocks() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    let shared_tx = Transaction {
        version: bitcoin::transaction::Version(1),
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(1000000),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };
    let shared_txid = shared_tx.compute_txid();

    let block1 = TestShareBlockBuilder::new()
        .nonce(0xe9695791)
        .add_transaction(shared_tx.clone())
        .build();
    let blockhash1 = block1.block_hash();

    let block2 = TestShareBlockBuilder::new()
        .nonce(0xe9695792)
        .add_transaction(shared_tx.clone())
        .build();
    let blockhash2 = block2.block_hash();

    let mut batch = Store::get_write_batch();
    store.add_share_block(&block1, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let mut batch = Store::get_write_batch();
    store.add_share_block(&block2, &mut batch).unwrap();
    store.commit_batch(batch).unwrap();

    let result = store.get_blockhashes_for_all_txids(&[shared_txid]).unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].len(), 2);
    assert!(result[0].contains(&blockhash1));
    assert!(result[0].contains(&blockhash2));
}

#[test]
fn test_min_coinbase_root_height_returns_min_across_inputs() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // Store two funding coinbase txs at different heights
    let funding_tx_a = Transaction {
        version: bitcoin::transaction::Version(1),
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn {
            previous_output: bitcoin::OutPoint::null(),
            ..Default::default()
        }],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(1_000_000),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };
    let txid_a = funding_tx_a.compute_txid();

    let funding_tx_b = Transaction {
        version: bitcoin::transaction::Version(1),
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn {
            previous_output: bitcoin::OutPoint::null(),
            script_sig: bitcoin::ScriptBuf::from_bytes(vec![0x01]),
            ..Default::default()
        }],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(2_000_000),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };
    let txid_b = funding_tx_b.compute_txid();

    // Store at heights 100 and 500
    let mut batch = Store::get_write_batch();
    store
        .add_sharechain_txs(&[ShareTransaction(funding_tx_a)], 100, &mut batch)
        .unwrap();
    store
        .add_sharechain_txs(&[ShareTransaction(funding_tx_b)], 500, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    // Spending tx consumes one output from each funding tx
    let spending_tx = Transaction {
        version: bitcoin::transaction::Version(1),
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![
            bitcoin::TxIn {
                previous_output: bitcoin::OutPoint::new(txid_a, 0),
                ..Default::default()
            },
            bitcoin::TxIn {
                previous_output: bitcoin::OutPoint::new(txid_b, 0),
                ..Default::default()
            },
        ],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(2_900_000),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };

    let outputs_cf = store.db.cf_handle(&ColumnFamily::Outputs).unwrap();
    let empty_cache: HashMap<String, u32> = HashMap::new();
    let result = store
        .min_coinbase_root_height_for_inputs(
            &ShareTransaction(spending_tx),
            &outputs_cf,
            &empty_cache,
        )
        .unwrap();

    assert_eq!(
        result, 100,
        "Should return the minimum height across inputs"
    );
}

#[test]
fn test_min_coinbase_root_height_errors_when_input_outputs_missing() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // Spending tx references outputs not in the store
    let spending_tx = Transaction {
        version: bitcoin::transaction::Version(1),
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn {
            previous_output: bitcoin::OutPoint::new(Txid::all_zeros(), 0),
            ..Default::default()
        }],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(1_000_000),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };

    let outputs_cf = store.db.cf_handle(&ColumnFamily::Outputs).unwrap();
    let empty_cache: HashMap<String, u32> = HashMap::new();
    let result = store.min_coinbase_root_height_for_inputs(
        &ShareTransaction(spending_tx),
        &outputs_cf,
        &empty_cache,
    );

    assert!(
        result.is_err(),
        "Should return error when input outputs not found in store"
    );
}

#[test]
fn test_coinbase_root_height_propagates_through_spending_chain() {
    let temp_dir = tempdir().unwrap();
    let store = Store::new(temp_dir.path().to_str().unwrap().to_string(), false).unwrap();

    // Store a coinbase tx at height 200
    let coinbase_tx = Transaction {
        version: bitcoin::transaction::Version(1),
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn {
            previous_output: bitcoin::OutPoint::null(),
            ..Default::default()
        }],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(5_000_000),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };
    let coinbase_txid = coinbase_tx.compute_txid();

    let mut batch = Store::get_write_batch();
    store
        .add_sharechain_txs(&[ShareTransaction(coinbase_tx)], 200, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    // Store a spending tx at height 300 that spends the coinbase
    let spending_tx = Transaction {
        version: bitcoin::transaction::Version(1),
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn {
            previous_output: bitcoin::OutPoint::new(coinbase_txid, 0),
            ..Default::default()
        }],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(4_000_000),
            script_pubkey: bitcoin::ScriptBuf::new(),
        }],
    };
    let spending_txid = spending_tx.compute_txid();

    let mut batch = Store::get_write_batch();
    store
        .add_sharechain_txs(&[ShareTransaction(spending_tx)], 300, &mut batch)
        .unwrap();
    store.commit_batch(batch).unwrap();

    // Verify the spending tx's output inherited coinbase_root_height = 200
    let output_key = outpoint_key(&spending_txid, 0);
    let outputs_cf = store.db.cf_handle(&ColumnFamily::Outputs).unwrap();
    let bytes = store
        .db
        .get_cf(&outputs_cf, output_key.as_bytes())
        .unwrap()
        .unwrap();
    let stored_out = consensus::deserialize::<StoredTxOut>(&bytes).unwrap();

    assert_eq!(
        stored_out.coinbase_root_height, 200,
        "Spending tx output should inherit coinbase_root_height from input"
    );
}
