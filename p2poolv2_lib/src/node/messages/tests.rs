use super::*;
use crate::accounting::payout::coinbase_budget::BCH_MAX_TX_SIZE;
use crate::shares::bch_coinbase::BchCoinbase;
use crate::shares::merge_mining::MergeMiningCommitment;
use crate::shares::share_block::ShareTransaction;
use crate::shares::validation::{BCH_SHARE_BODY_SIZE_LIMIT, BLOCK_TXS_SIZE_LIMIT, MAX_UNCLES};
use crate::test_utils::TestShareBlockBuilder;
use bitcoin::consensus::encode;
use bitcoin::script::Builder;
use p2poolv2_wallet::share_owner::ShareOwner;
use std::str::FromStr;

#[test]
fn test_raw_message_roundtrip() {
    let block_hashes = vec![
        BlockHash::from_str("0000000086704a35f17580d06f76d4c02d2b1f68774800675fb45f0411205bb5")
            .unwrap(),
    ];
    let msg = Message::Inventory(InventoryMessage::BlockHashes(block_hashes.clone()));
    let raw = RawMessage::new(msg.clone());

    // Test encoding
    let mut encoded = Vec::new();
    raw.consensus_encode(&mut encoded).unwrap();

    // Test decoding
    let decoded = RawMessage::consensus_decode(&mut &encoded[..]).unwrap();

    assert_eq!(decoded.payload, msg);
    assert_eq!(decoded.payload_len, raw.payload_len);
    assert_eq!(decoded.checksum, raw.checksum);
}

#[test]
fn test_raw_message_checksum_verification() {
    let msg = Message::NotFound(GetData::Block(BlockHash::all_zeros()));
    let raw = RawMessage::new(msg);

    let mut encoded = Vec::new();
    raw.consensus_encode(&mut encoded).unwrap();

    // Corrupt the checksum. The header is now payload_len (4 bytes) followed
    // by the checksum (4 bytes), so the checksum starts at byte 4.
    encoded[4] ^= 0xFF;

    // Decoding should fail
    let result = RawMessage::consensus_decode(&mut &encoded[..]);
    assert!(result.is_err());
}

/// The largest legitimate message -- a full `ShareHeaders` response of
/// worst-case headers -- must fit under `MAX_P2P_MESSAGE_SIZE`, or header
/// sync would be rejected by our own codec. Worst case per header: the
/// maximum uncles, the longest address script (P2WSH) in every address
/// field, the bitcoin merkle root included, and a distinct coinbase branch of the maximum
/// length -- no two headers sharing a block template. The sender completes
/// whole heights, so a response can overshoot `MAX_HEADERS_IN_RESPONSE` by
/// up to one dense height.
#[test]
fn test_full_share_headers_response_fits_max_message_size() {
    let longest_address =
        bitcoin::Address::p2wsh(&bitcoin::ScriptBuf::new(), bitcoin::Network::Regtest);
    let mut header = TestShareBlockBuilder::new()
        .uncles(vec![BlockHash::all_zeros(); MAX_UNCLES])
        .build()
        .header;
    // Changing committed fields after the build leaves the proof unable to
    // derive the root, so the encoder includes it: the larger encoding.
    header.miner_bitcoin_address = longest_address.clone();
    header.donation_address = Some(longest_address.clone());
    header.donation = Some(u16::MAX);
    header.fee_address = Some(longest_address);
    header.fee = Some(u16::MAX);

    header.miner_address = ShareOwner::CashP2pkh([0x11; 20]);
    header.bch_coinbase = Some(BchCoinbase {
        prefix: vec![0xff; 88],
        merge_mining: MergeMiningCommitment::empty(),
    });

    let header_count = MAX_SHARE_HEADER_BATCH_LENGTH;
    let mut entries = Vec::with_capacity(header_count);
    for index in 0..header_count as u32 {
        let mut node = [0u8; 32];
        node[..4].copy_from_slice(&index.to_le_bytes());
        let branch =
            vec![bitcoin::TxMerkleNode::from_byte_array(node); MAX_COINBASE_MERKLE_BRANCH_LENGTH];
        entries.push((header.clone(), branch));
    }
    let message = Message::ShareHeaders(ShareHeaderBatch::from_headers_with_branches(entries));
    let encoded = encode::serialize(&message);

    assert!(
        encoded.len() < MAX_P2P_MESSAGE_SIZE,
        "a full ShareHeaders response is {} bytes, over the {} byte cap",
        encoded.len(),
        MAX_P2P_MESSAGE_SIZE
    );
}

/// The largest Bitcoin Cash share a peer may send -- a 1,000,000-byte
/// coinbase, 200 KiB of share transactions, a 32-level coinbase branch and a
/// header with every field at its largest -- fits one message.
#[test]
fn test_largest_bch_share_block_fits_max_message_size() {
    let mut share = TestShareBlockBuilder::new()
        .uncles(vec![BlockHash::all_zeros(); MAX_UNCLES])
        .build();
    share.header.miner_address = ShareOwner::CashP2pkh([0x11; 20]);
    share.header.bch_coinbase = Some(BchCoinbase {
        prefix: vec![0xff; 88],
        merge_mining: MergeMiningCommitment::empty(),
    });
    let coinbase_padding = BCH_MAX_TX_SIZE - share.bitcoin_coinbase.total_size();
    share.bitcoin_coinbase.output[0].script_pubkey =
        bitcoin::ScriptBuf::from_bytes(vec![0; coinbase_padding]);
    let mut share_transaction = share.transactions[0].0.clone();
    share_transaction.output[0].script_pubkey =
        bitcoin::ScriptBuf::from_bytes(vec![0; BLOCK_TXS_SIZE_LIMIT as usize]);
    share.transactions = vec![ShareTransaction(share_transaction)];
    share.template_merkle_branches = vec![TxMerkleNode::all_zeros(); 32];

    let encoded = encode::serialize(&Message::ShareBlock(share));

    assert!(encoded.len() > BCH_SHARE_BODY_SIZE_LIMIT);
    assert!(
        encoded.len() < MAX_P2P_MESSAGE_SIZE,
        "the largest share is {} bytes, over the {} byte cap",
        encoded.len(),
        MAX_P2P_MESSAGE_SIZE
    );
}

#[test]
fn declared_bch_header_batch_round_trips_a_compressed_32_level_proof() {
    let mut header = TestShareBlockBuilder::new().build().header;
    header.miner_address = ShareOwner::CashP2pkh([0x11; 20]);
    header.bch_coinbase = Some(BchCoinbase {
        prefix: Builder::new()
            .push_int(header.bitcoin_height as i64)
            .into_bytes(),
        merge_mining: MergeMiningCommitment::empty(),
    });
    let branch = vec![TxMerkleNode::all_zeros(); 32];
    header.bitcoin_header.merkle_root =
        header.coinbase_proof.merkle_root(&header, &branch).unwrap();
    let batch = ShareHeaderBatch::from_headers_with_branches(vec![(header, branch)]);
    let decoded: ShareHeaderBatch = encode::deserialize(&encode::serialize(&batch)).unwrap();
    assert_eq!(decoded, batch);
}

#[test]
fn test_raw_message_rejects_oversized_payload_len() {
    // A malicious peer advertises a payload length larger than the maximum
    // message size. Decoding must reject it without allocating the buffer.
    let payload_len = (MAX_P2P_MESSAGE_SIZE as u32) + 1;
    let mut encoded = Vec::new();
    encoded.extend_from_slice(&payload_len.to_le_bytes());
    encoded.extend_from_slice(&[0u8; 4]); // checksum placeholder

    let result = RawMessage::consensus_decode(&mut &encoded[..]);
    assert!(result.is_err());
}

#[test]
fn test_message_not_found_roundtrip() {
    let msg = Message::NotFound(GetData::Block(BlockHash::all_zeros()));
    let mut encoded = Vec::new();
    msg.consensus_encode(&mut encoded).unwrap();

    let decoded = Message::consensus_decode(&mut &encoded[..]).unwrap();
    assert_eq!(decoded, msg);
}

#[test]
fn test_message_get_share_headers_roundtrip() {
    let hashes = vec![
        BlockHash::from_str("0000000086704a35f17580d06f76d4c02d2b1f68774800675fb45f0411205bb5")
            .unwrap(),
        BlockHash::from_str("0000000086704a35f17580d06f76d4c02d2b1f68774800675fb45f0411205bb6")
            .unwrap(),
    ];
    let stop =
        BlockHash::from_str("0000000086704a35f17580d06f76d4c02d2b1f68774800675fb45f0411205bb7")
            .unwrap();

    let msg = Message::GetShareHeaders(hashes.clone(), stop);
    let mut encoded = Vec::new();
    msg.consensus_encode(&mut encoded).unwrap();

    let decoded = Message::consensus_decode(&mut &encoded[..]).unwrap();
    match decoded {
        Message::GetShareHeaders(decoded_hashes, decoded_stop) => {
            assert_eq!(decoded_hashes, hashes);
            assert_eq!(decoded_stop, stop);
        }
        _ => panic!("Expected GetShareHeaders variant"),
    }
}

#[test]
fn test_message_get_share_blocks_roundtrip() {
    let hashes = vec![
        BlockHash::from_str("0000000086704a35f17580d06f76d4c02d2b1f68774800675fb45f0411205bb5")
            .unwrap(),
    ];
    let stop = BlockHash::all_zeros();

    let msg = Message::GetShareBlocks(hashes.clone(), stop);
    let mut encoded = Vec::new();
    msg.consensus_encode(&mut encoded).unwrap();

    let decoded = Message::consensus_decode(&mut &encoded[..]).unwrap();
    match decoded {
        Message::GetShareBlocks(decoded_hashes, decoded_stop) => {
            assert_eq!(decoded_hashes, hashes);
            assert_eq!(decoded_stop, stop);
        }
        _ => panic!("Expected GetShareBlocks variant"),
    }
}

#[test]
fn test_inventory_message_block_hashes_roundtrip() {
    let hashes = vec![
        BlockHash::from_str("0000000086704a35f17580d06f76d4c02d2b1f68774800675fb45f0411205bb5")
            .unwrap(),
        BlockHash::from_str("0000000086704a35f17580d06f76d4c02d2b1f68774800675fb45f0411205bb6")
            .unwrap(),
    ];

    let inv = InventoryMessage::BlockHashes(hashes.clone());
    let mut encoded = Vec::new();
    inv.consensus_encode(&mut encoded).unwrap();

    let decoded = InventoryMessage::consensus_decode(&mut &encoded[..]).unwrap();
    match decoded {
        InventoryMessage::BlockHashes(decoded_hashes) => {
            assert_eq!(decoded_hashes, hashes);
        }
        _ => panic!("Expected BlockHashes variant"),
    }
}

#[test]
fn test_get_data_block_roundtrip() {
    let hash =
        BlockHash::from_str("0000000086704a35f17580d06f76d4c02d2b1f68774800675fb45f0411205bb5")
            .unwrap();

    let get_data = GetData::Block(hash);
    let mut encoded = Vec::new();
    get_data.consensus_encode(&mut encoded).unwrap();

    let decoded = GetData::consensus_decode(&mut &encoded[..]).unwrap();
    assert_eq!(decoded, get_data);
}

#[test]
fn test_get_data_txid_roundtrip() {
    let txid =
        Txid::from_str("d2528fc2d7a4f95ace97860f157c895b6098667df0e43912b027cfe58edf304e").unwrap();

    let get_data = GetData::Txid(txid);
    let mut encoded = Vec::new();
    get_data.consensus_encode(&mut encoded).unwrap();

    let decoded = GetData::consensus_decode(&mut &encoded[..]).unwrap();
    assert_eq!(decoded, get_data);
}

#[test]
fn test_message_discriminants_unique() {
    use message_discriminants::*;
    let discriminants = [
        INVENTORY,
        NOT_FOUND,
        GET_SHARE_HEADERS,
        GET_SHARE_BLOCKS,
        SHARE_HEADERS,
        SHARE_BLOCK,
        GET_DATA,
        TRANSACTION,
        HANDSHAKE,
        ACK,
    ];

    // Check all discriminants are unique
    for i in 0..discriminants.len() {
        for j in (i + 1)..discriminants.len() {
            assert_ne!(
                discriminants[i], discriminants[j],
                "Discriminants at positions {i} and {j} are not unique"
            );
        }
    }
}

#[test]
fn test_inventory_message_serde() {
    let have_shares = vec![
        "0000000086704a35f17580d06f76d4c02d2b1f68774800675fb45f0411205bb5"
            .parse::<BlockHash>()
            .unwrap(),
        "0000000086704a35f17580d06f76d4c02d2b1f68774800675fb45f0411205bb6"
            .parse::<BlockHash>()
            .unwrap(),
        "0000000086704a35f17580d06f76d4c02d2b1f68774800675fb45f0411205bb7"
            .parse::<BlockHash>()
            .unwrap(),
    ];

    let msg = Message::Inventory(InventoryMessage::BlockHashes(have_shares.clone()));

    // Test serialization
    let mut serialized = Vec::new();
    msg.consensus_encode(&mut serialized).unwrap();

    // Test deserialization
    let deserialized = encode::deserialize::<Message>(&serialized).unwrap();

    let deserialized: Vec<BlockHash> = match deserialized {
        Message::Inventory(InventoryMessage::BlockHashes(have_shares)) => have_shares,
        _ => panic!("Expected Inventory variant"),
    };

    // Verify the deserialized message matches original
    assert_eq!(deserialized.len(), 3);
    assert!(deserialized.contains(&have_shares[0]));
    assert!(deserialized.contains(&have_shares[1]));
    assert!(deserialized.contains(&have_shares[2]));
}

#[test]
fn test_get_data_message_serde() {
    // Test BlockHash variant
    let block_msg = Message::GetData(GetData::Block(
        "0000000086704a35f17580d06f76d4c02d2b1f68774800675fb45f0411205bb5"
            .parse::<BlockHash>()
            .unwrap(),
    ));
    let mut serialized = Vec::new();
    block_msg.consensus_encode(&mut serialized).unwrap();

    // Test deserialization
    let deserialized = encode::deserialize::<Message>(&serialized).unwrap();

    match deserialized {
        Message::GetData(GetData::Block(hash)) => {
            assert_eq!(
                hash.to_string(),
                "0000000086704a35f17580d06f76d4c02d2b1f68774800675fb45f0411205bb5"
            )
        }
        _ => panic!("Expected BlockHash variant"),
    }

    // Test Txid variant
    let tx_msg = Message::GetData(GetData::Txid(
        Txid::from_str("d2528fc2d7a4f95ace97860f157c895b6098667df0e43912b027cfe58edf304e").unwrap(),
    ));
    let mut serialized = Vec::new();
    tx_msg.consensus_encode(&mut serialized).unwrap();

    let deserialized = encode::deserialize::<Message>(&serialized).unwrap();
    match deserialized {
        Message::GetData(GetData::Txid(hash)) => {
            assert_eq!(
                hash,
                Txid::from_str("d2528fc2d7a4f95ace97860f157c895b6098667df0e43912b027cfe58edf304e")
                    .unwrap()
            )
        }
        _ => panic!("Expected Txid variant"),
    }
}

#[test]
fn test_handshake_message_roundtrip() {
    let tip_hash =
        BlockHash::from_str("0000000086704a35f17580d06f76d4c02d2b1f68774800675fb45f0411205bb5")
            .unwrap();
    let handshake_data = HandshakeData {
        tip_height: 42,
        tip_hash,
    };

    let msg = Message::Handshake(handshake_data.clone());
    let mut encoded = Vec::new();
    msg.consensus_encode(&mut encoded).unwrap();

    let decoded = Message::consensus_decode(&mut &encoded[..]).unwrap();
    match decoded {
        Message::Handshake(decoded_data) => {
            assert_eq!(decoded_data.tip_height, 42);
            assert_eq!(decoded_data.tip_hash, tip_hash);
        }
        _ => panic!("Expected Handshake variant"),
    }
}

#[test]
fn test_handshake_message_fresh_node_roundtrip() {
    let handshake_data = HandshakeData {
        tip_height: 0,
        tip_hash: BlockHash::all_zeros(),
    };

    let msg = Message::Handshake(handshake_data);
    let mut encoded = Vec::new();
    msg.consensus_encode(&mut encoded).unwrap();

    let decoded = Message::consensus_decode(&mut &encoded[..]).unwrap();
    match decoded {
        Message::Handshake(decoded_data) => {
            assert_eq!(decoded_data.tip_height, 0);
            assert_eq!(decoded_data.tip_hash, BlockHash::all_zeros());
        }
        _ => panic!("Expected Handshake variant"),
    }
}

#[test]
fn test_ack_message_roundtrip() {
    let msg = Message::Ack;
    let mut encoded = Vec::new();
    msg.consensus_encode(&mut encoded).unwrap();

    let decoded = Message::consensus_decode(&mut &encoded[..]).unwrap();
    assert_eq!(decoded, Message::Ack);
}

/// Shares mined on one template share a coinbase branch, so the batch
/// carries it once.
#[test]
fn test_share_header_batch_stores_shared_branch_once() {
    let first = TestShareBlockBuilder::new().nonce(1).build().header;
    let second = TestShareBlockBuilder::new().nonce(2).build().header;
    let branch = vec![bitcoin::TxMerkleNode::from_byte_array([0x42; 32])];

    let batch = ShareHeaderBatch::from_headers_with_branches(vec![
        (first, branch.clone()),
        (second, branch.clone()),
    ]);

    assert_eq!(batch.len(), 2);
    assert_eq!(batch.branch_count(), 1);
    assert_eq!(batch.branch(0), branch.as_slice());
    assert_eq!(batch.branch(1), branch.as_slice());
}

#[test]
fn test_share_header_batch_keeps_distinct_branches_apart() {
    let first = TestShareBlockBuilder::new().nonce(1).build().header;
    let second = TestShareBlockBuilder::new().nonce(2).build().header;
    let first_branch = vec![bitcoin::TxMerkleNode::from_byte_array([0x42; 32])];
    let second_branch = vec![bitcoin::TxMerkleNode::from_byte_array([0x43; 32])];

    let batch = ShareHeaderBatch::from_headers_with_branches(vec![
        (first, first_branch.clone()),
        (second, second_branch.clone()),
    ]);

    assert_eq!(batch.branch_count(), 2);
    assert_eq!(batch.branch(0), first_branch.as_slice());
    assert_eq!(batch.branch(1), second_branch.as_slice());
}

#[test]
fn test_share_headers_message_round_trips_with_branches() {
    let first = TestShareBlockBuilder::new().nonce(1).build().header;
    let second = TestShareBlockBuilder::new().nonce(2).build().header;
    let branch = vec![
        bitcoin::TxMerkleNode::from_byte_array([0x42; 32]),
        bitcoin::TxMerkleNode::from_byte_array([0x43; 32]),
    ];
    let message = Message::ShareHeaders(ShareHeaderBatch::from_headers_with_branches(vec![
        (first, branch.clone()),
        (second, Vec::new()),
    ]));

    let decoded: Message = encode::deserialize(&encode::serialize(&message)).unwrap();

    assert_eq!(decoded, message);
}

/// A header naming a branch the batch does not carry is a malformed
/// message, rejected at decode so a decoded batch always resolves.
#[test]
fn test_share_header_batch_decode_rejects_branch_index_outside_table() {
    let header = TestShareBlockBuilder::new().build().header;
    let mut bytes = Vec::new();
    VarInt::from(0usize).consensus_encode(&mut bytes).unwrap();
    VarInt::from(1usize).consensus_encode(&mut bytes).unwrap();
    header.consensus_encode(&mut bytes).unwrap();
    0u16.consensus_encode(&mut bytes).unwrap();

    let result: Result<ShareHeaderBatch, _> = encode::deserialize(&bytes);

    assert!(result.is_err());
}

/// A branch table longer than any legitimate response is rejected before
/// it is read: empty branches cost one byte each on the wire but a `Vec`
/// each in memory, so a bounded message could otherwise allocate tens of
/// megabytes.
#[test]
fn test_share_header_batch_decode_rejects_branch_table_over_limit() {
    let mut bytes = Vec::new();
    VarInt::from(MAX_SHARE_HEADER_BATCH_LENGTH + 1)
        .consensus_encode(&mut bytes)
        .unwrap();
    bytes.resize(bytes.len() + MAX_SHARE_HEADER_BATCH_LENGTH + 1, 0);
    VarInt::from(0usize).consensus_encode(&mut bytes).unwrap();

    let result: Result<ShareHeaderBatch, _> = encode::deserialize(&bytes);

    assert!(result.is_err());
}

#[test]
fn test_share_header_batch_decode_accepts_branch_table_at_limit() {
    let mut bytes = Vec::new();
    VarInt::from(MAX_SHARE_HEADER_BATCH_LENGTH)
        .consensus_encode(&mut bytes)
        .unwrap();
    bytes.resize(bytes.len() + MAX_SHARE_HEADER_BATCH_LENGTH, 0);
    VarInt::from(0usize).consensus_encode(&mut bytes).unwrap();

    let result: Result<ShareHeaderBatch, _> = encode::deserialize(&bytes);

    assert!(result.is_ok());
}

/// A header whose proof derives its bitcoin merkle root travels without
/// it, and the receiver restores it exactly.
#[test]
fn test_share_header_batch_omits_derivable_bitcoin_merkle_root() {
    let header = TestShareBlockBuilder::new()
        .prev_share_blockhash(
            "0000000086704a35f17580d06f76d4c02d2b1f68774800675fb45f0411205bb5".to_string(),
        )
        .build()
        .header;
    let derivable = ShareHeaderBatch::from_headers_with_branches(vec![(header.clone(), vec![])]);
    let underivable = ShareHeaderBatch::from_headers_with_branches(vec![(
        header.clone(),
        vec![bitcoin::TxMerkleNode::from_byte_array([0x42; 32])],
    )]);

    let derivable_bytes = encode::serialize(&derivable);
    let underivable_bytes = encode::serialize(&underivable);

    // The underivable batch carries one more branch node and the root.
    assert_eq!(underivable_bytes.len() - derivable_bytes.len(), 32 + 32);
    let decoded: ShareHeaderBatch = encode::deserialize(&derivable_bytes).unwrap();
    assert_eq!(decoded.headers(), [header].as_slice());
}

/// A sender that leaves the root out under a forged proof makes the
/// receiver derive a different root, so the bitcoin header the receiver
/// sees is not the one the proof of work was done on.
#[test]
fn test_share_header_batch_forged_proof_derives_a_different_root() {
    let original = TestShareBlockBuilder::new()
        .prev_share_blockhash(
            "0000000086704a35f17580d06f76d4c02d2b1f68774800675fb45f0411205bb5".to_string(),
        )
        .build()
        .header;
    let mut forged = original.clone();
    forged.coinbase_proof.midstate[0] ^= 1;

    let mut bytes = Vec::new();
    VarInt::from(1usize).consensus_encode(&mut bytes).unwrap();
    VarInt::from(0usize).consensus_encode(&mut bytes).unwrap();
    VarInt::from(1usize).consensus_encode(&mut bytes).unwrap();
    0u16.consensus_encode(&mut bytes).unwrap();
    false.consensus_encode(&mut bytes).unwrap();
    forged.consensus_encode_with(&mut bytes, false).unwrap();

    let decoded: ShareHeaderBatch = encode::deserialize(&bytes).unwrap();

    let decoded_header = &decoded.headers()[0];
    assert_ne!(
        decoded_header.bitcoin_header.merkle_root,
        original.bitcoin_header.merkle_root
    );
    assert_ne!(
        decoded_header.bitcoin_header.block_hash(),
        original.bitcoin_header.block_hash()
    );
}
