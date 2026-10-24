// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::node::p2p_message_handlers::MAX_HEADERS_IN_RESPONSE;
use crate::shares::coinbase_proof::MAX_COINBASE_MERKLE_BRANCH_LENGTH;
use crate::shares::share_block::{ShareBlock, ShareHeader, Txids};
use crate::store::dag_store::MAX_BLOCKS_PER_HEIGHT;
use bitcoin::consensus::{Decodable, Encodable, encode};
use bitcoin::hashes::{Hash, sha256d};
use bitcoin::io::{Read, Write};
use bitcoin::{BlockHash, TxMerkleNode, Txid, VarInt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Display;

/// Largest P2P message payload this node will decode, in bytes.
///
/// The codec rejects a larger advertised length before allocating, so this
/// bounds the memory one peer message can pin -- including responses queued for
/// the response worker. Sized for P2Pool's own messages rather than bitcoin's
/// 5 MB block-relay limit. The largest legitimate messages are a Bitcoin Cash
/// `ShareBlock`, whose body holds a 1,000,000-byte coinbase and 200 KiB of
/// share transactions (`BCH_SHARE_BODY_SIZE_LIMIT`, 1,204,800 bytes) beside
/// its header and coinbase merkle branch, and a `ShareHeaders` batch of
/// `MAX_HEADERS_IN_RESPONSE` plus up to one height of overshoot (~1 MB of
/// worst-case headers and branches). 1.25 MiB holds either with about 100 KB
/// to spare; `test_full_share_headers_response_fits_max_message_size` and
/// `test_largest_bch_share_block_fits_max_message_size` guard the margin.
pub const MAX_P2P_MESSAGE_SIZE: usize = 1_280 * 1024;

/// Most headers, and most distinct branches, a `ShareHeaderBatch` may carry:
/// a full response plus the one dense height the sender completes past it.
/// An honest batch never has more branches than headers.
pub(crate) const MAX_SHARE_HEADER_BATCH_LENGTH: usize =
    MAX_HEADERS_IN_RESPONSE + MAX_BLOCKS_PER_HEIGHT;

/// Message type discriminants for determining the message type
/// We use a single byte integer instead of bitcoin's 12 byte string
mod message_discriminants {
    pub const INVENTORY: u8 = 0;
    pub const NOT_FOUND: u8 = 1;
    pub const GET_SHARE_HEADERS: u8 = 2;
    pub const GET_SHARE_BLOCKS: u8 = 3;
    pub const SHARE_HEADERS: u8 = 4;
    pub const SHARE_BLOCK: u8 = 5;
    pub const GET_DATA: u8 = 6;
    pub const TRANSACTION: u8 = 7;
    pub const HANDSHAKE: u8 = 8;
    pub const ACK: u8 = 9;
}

/// InventoryMessage discriminants to determine the type of inventory message
mod inventory_discriminants {
    pub const BLOCK_HASHES: u8 = 0;
    pub const TRANSACTION_HASHES: u8 = 1;
}

/// GetData discriminants to determine the type of get data message
mod getdata_discriminants {
    pub const BLOCK: u8 = 0;
    pub const TXID: u8 = 1;
}

/// P2P network messages, encoded using bitcoin consensus_encode
#[derive(Debug, Clone, PartialEq, Eq)]
// Boxing the large variants would ripple through every match site and every
// consensus encode/decode impl for marginal benefit.
#[allow(clippy::large_enum_variant)]
pub enum Message {
    Inventory(InventoryMessage),
    NotFound(GetData),
    GetShareHeaders(Vec<BlockHash>, BlockHash),
    GetShareBlocks(Vec<BlockHash>, BlockHash),
    ShareHeaders(ShareHeaderBatch),
    ShareBlock(ShareBlock),
    GetData(GetData),
    Transaction(bitcoin::Transaction),
    Handshake(HandshakeData),
    /// Acknowledgment response for request-response messages that
    /// sometimes do not need to send a meaningful return payload
    /// (e.g. Handshake, Inventory). This is a stop gap solution to
    /// avoiding timeout errors from libp2p and timeouts filling up
    /// queues. Ideally we need to build our own stream protocol for
    /// libp2p. Something, we don't want to take on now.
    Ack,
}

/// Handshake data exchanged when a connection is established.
/// Both peers send their confirmed tip height and hash so each
/// side can determine whether it needs to fetch headers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandshakeData {
    pub tip_height: u32,
    pub tip_hash: BlockHash,
}

/// A complete P2P network message with protocol framing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawMessage {
    /// The actual message payload
    pub payload: Message,
    /// Length of the payload in bytes
    pub payload_len: u32,
    /// Checksum: first 4 bytes of SHA256d(payload)
    pub checksum: [u8; 4],
}

impl RawMessage {
    /// Create a new RawMessage from a Message
    /// Automatically computes payload_len and checksum
    pub fn new(payload: Message) -> Self {
        // Encode payload to calculate length and checksum
        let mut engine = sha256d::Hash::engine();
        let payload_len = payload
            .consensus_encode(&mut engine)
            .expect("engine doesn't error");
        let payload_len = u32::try_from(payload_len).expect("payload length fits in u32");

        // Get checksum from hash
        let hash = sha256d::Hash::from_engine(engine);
        let checksum = [hash[0], hash[1], hash[2], hash[3]];

        Self {
            payload,
            payload_len,
            checksum,
        }
    }

    /// Consume RawMessage and return the inner payload
    pub fn into_payload(self) -> Message {
        self.payload
    }

    /// Get reference to the payload
    pub fn payload(&self) -> &Message {
        &self.payload
    }
}

impl Display for RawMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RawMessage({})", self.payload)
    }
}

impl Message {
    /// Returns the variant name as a static string slice.
    ///
    /// We need to avoid allocation for debug logging, when we don't
    /// want to clone message.
    pub fn message_type(&self) -> &'static str {
        match self {
            Message::Inventory(_) => "Inventory",
            Message::NotFound(_) => "NotFound",
            Message::GetShareHeaders(_, _) => "GetShareHeaders",
            Message::GetShareBlocks(_, _) => "GetShareBlocks",
            Message::ShareHeaders(_) => "ShareHeaders",
            Message::ShareBlock(_) => "ShareBlock",
            Message::GetData(_) => "GetData",
            Message::Transaction(_) => "Transaction",
            Message::Handshake(_) => "Handshake",
            Message::Ack => "Ack",
        }
    }
}

impl Display for Message {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message_type())
    }
}

/// A `ShareHeaders` response: headers, each with the coinbase merkle branch
/// its `CoinbaseProof` is checked against.
///
/// Each header leaves out its bitcoin merkle root when its proof and branch
/// give it back, which they do for every share but genesis: the receiver
/// derives the root, and a forged proof derives a root whose bitcoin header
/// fails its proof-of-work check.
///
/// Shares mined on one block template share a branch, so each distinct branch
/// is sent once in `branches` and every header names its branch by index. The
/// table belongs to this message alone: a header never refers to another
/// message's table, so dropped, reordered or retried responses cannot leave a
/// header without its branch. Decoding rejects an index outside the table, so
/// `branch` never fails on a decoded batch.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ShareHeaderBatch {
    headers: Vec<ShareHeader>,
    /// Index into `branches` for the header at the same position.
    branch_indexes: Vec<u16>,
    branches: Vec<Vec<TxMerkleNode>>,
}

impl ShareHeaderBatch {
    /// Build a batch from headers and their coinbase merkle branches,
    /// storing each distinct branch once.
    pub fn from_headers_with_branches(entries: Vec<(ShareHeader, Vec<TxMerkleNode>)>) -> Self {
        let mut headers = Vec::with_capacity(entries.len());
        let mut branch_indexes = Vec::with_capacity(entries.len());
        let mut branches: Vec<Vec<TxMerkleNode>> = Vec::new();
        let mut index_of_branch: HashMap<Vec<TxMerkleNode>, u16> =
            HashMap::with_capacity(entries.len());
        for (header, branch) in entries {
            let index = match index_of_branch.get(&branch) {
                Some(index) => *index,
                None => {
                    let index = branches.len() as u16;
                    index_of_branch.insert(branch.clone(), index);
                    branches.push(branch);
                    index
                }
            };
            headers.push(header);
            branch_indexes.push(index);
        }
        Self {
            headers,
            branch_indexes,
            branches,
        }
    }

    /// The headers, in message order.
    pub fn headers(&self) -> &[ShareHeader] {
        &self.headers
    }

    /// The coinbase merkle branch of the header at `position`.
    pub fn branch(&self, position: usize) -> &[TxMerkleNode] {
        &self.branches[self.branch_indexes[position] as usize]
    }

    /// Number of distinct branches carried.
    pub fn branch_count(&self) -> usize {
        self.branches.len()
    }

    /// Each header's hash with its branch, for storing the branches of
    /// headers held without their bodies.
    pub fn blockhashes_with_branches(&self) -> Vec<(BlockHash, Vec<TxMerkleNode>)> {
        self.headers
            .iter()
            .enumerate()
            .map(|(position, header)| (header.block_hash(), self.branch(position).to_vec()))
            .collect()
    }

    pub fn len(&self) -> usize {
        self.headers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.headers.is_empty()
    }
}

impl Encodable for ShareHeaderBatch {
    #[inline]
    fn consensus_encode<W: Write + ?Sized>(&self, w: &mut W) -> Result<usize, bitcoin::io::Error> {
        let mut len = VarInt::from(self.branches.len()).consensus_encode(w)?;
        for branch in &self.branches {
            len += VarInt::from(branch.len()).consensus_encode(w)?;
            for node in branch {
                len += node.consensus_encode(w)?;
            }
        }
        len += VarInt::from(self.headers.len()).consensus_encode(w)?;
        for (header, branch_index) in self.headers.iter().zip(&self.branch_indexes) {
            let branch = &self.branches[*branch_index as usize];
            let root_is_derivable = header.coinbase_proof.merkle_root(header, branch).ok()
                == Some(header.bitcoin_header.merkle_root);
            len += branch_index.consensus_encode(w)?;
            len += (!root_is_derivable).consensus_encode(w)?;
            len += header.consensus_encode_with(w, !root_is_derivable)?;
        }
        Ok(len)
    }
}

impl Decodable for ShareHeaderBatch {
    #[inline]
    fn consensus_decode_from_finite_reader<R: Read + ?Sized>(
        r: &mut R,
    ) -> Result<Self, encode::Error> {
        // The reader is bounded by MAX_P2P_MESSAGE_SIZE, but an empty branch
        // costs one byte there and a Vec here, so both counts are capped
        // before anything is allocated.
        let branch_count = VarInt::consensus_decode(r)?.0 as usize;
        if branch_count > MAX_SHARE_HEADER_BATCH_LENGTH {
            return Err(encode::Error::ParseFailed(
                "Share header batch has too many branches",
            ));
        }
        let mut branches = Vec::with_capacity(branch_count);
        for _ in 0..branch_count {
            let node_count = VarInt::consensus_decode(r)?.0 as usize;
            if node_count > MAX_COINBASE_MERKLE_BRANCH_LENGTH {
                return Err(encode::Error::ParseFailed(
                    "Coinbase merkle branch too long",
                ));
            }
            let mut branch = Vec::with_capacity(node_count);
            for _ in 0..node_count {
                branch.push(TxMerkleNode::consensus_decode(r)?);
            }
            branches.push(branch);
        }
        let header_count = VarInt::consensus_decode(r)?.0 as usize;
        if header_count > MAX_SHARE_HEADER_BATCH_LENGTH {
            return Err(encode::Error::ParseFailed(
                "Share header batch has too many headers",
            ));
        }
        let mut headers = Vec::with_capacity(header_count);
        let mut branch_indexes = Vec::with_capacity(header_count);
        for _ in 0..header_count {
            let branch_index = u16::consensus_decode(r)?;
            let branch: &Vec<TxMerkleNode> =
                branches
                    .get(branch_index as usize)
                    .ok_or(encode::Error::ParseFailed(
                        "Share header names a branch outside the batch",
                    ))?;
            let includes_bitcoin_merkle_root = bool::consensus_decode(r)?;
            let mut header = ShareHeader::consensus_decode_with(r, includes_bitcoin_merkle_root)?;
            if !includes_bitcoin_merkle_root {
                header.bitcoin_header.merkle_root = header
                    .coinbase_proof
                    .merkle_root(&header, branch)
                    .map_err(|_| {
                        encode::Error::ParseFailed("Cannot derive share header bitcoin merkle root")
                    })?;
            }
            headers.push(header);
            branch_indexes.push(branch_index);
        }
        Ok(Self {
            headers,
            branch_indexes,
            branches,
        })
    }

    #[inline]
    fn consensus_decode<R: Read + ?Sized>(r: &mut R) -> Result<Self, encode::Error> {
        Self::consensus_decode_from_finite_reader(&mut r.take(MAX_P2P_MESSAGE_SIZE as u64))
    }
}

impl Encodable for Message {
    fn consensus_encode<W: Write + ?Sized>(&self, w: &mut W) -> Result<usize, bitcoin::io::Error> {
        use message_discriminants::*;
        match self {
            Message::Inventory(inv) => {
                let mut len = INVENTORY.consensus_encode(w)?;
                len += inv.consensus_encode(w)?;
                Ok(len)
            }
            Message::NotFound(get_data) => {
                let mut len = NOT_FOUND.consensus_encode(w)?;
                len += get_data.consensus_encode(w)?;
                Ok(len)
            }
            Message::GetShareHeaders(hashes, stop) => {
                let mut len = GET_SHARE_HEADERS.consensus_encode(w)?;
                len += hashes.consensus_encode(w)?;
                len += stop.consensus_encode(w)?;
                Ok(len)
            }
            Message::GetShareBlocks(hashes, stop) => {
                let mut len = GET_SHARE_BLOCKS.consensus_encode(w)?;
                len += hashes.consensus_encode(w)?;
                len += stop.consensus_encode(w)?;
                Ok(len)
            }
            Message::ShareHeaders(headers) => {
                let mut len = SHARE_HEADERS.consensus_encode(w)?;
                len += headers.consensus_encode(w)?;
                Ok(len)
            }
            Message::ShareBlock(block) => {
                let mut len = SHARE_BLOCK.consensus_encode(w)?;
                len += block.consensus_encode(w)?;
                Ok(len)
            }
            Message::GetData(data) => {
                let mut len = GET_DATA.consensus_encode(w)?;
                len += data.consensus_encode(w)?;
                Ok(len)
            }
            Message::Transaction(tx) => {
                let mut len = TRANSACTION.consensus_encode(w)?;
                len += tx.consensus_encode(w)?;
                Ok(len)
            }
            Message::Handshake(handshake_data) => {
                let mut len = HANDSHAKE.consensus_encode(w)?;
                len += handshake_data.tip_height.consensus_encode(w)?;
                len += handshake_data.tip_hash.consensus_encode(w)?;
                Ok(len)
            }
            Message::Ack => {
                let len = ACK.consensus_encode(w)?;
                Ok(len)
            }
        }
    }
}

impl Decodable for Message {
    fn consensus_decode<R: Read + ?Sized>(r: &mut R) -> Result<Self, encode::Error> {
        use message_discriminants::*;
        let disc = u8::consensus_decode(r)?;
        match disc {
            INVENTORY => Ok(Message::Inventory(InventoryMessage::consensus_decode(r)?)),
            NOT_FOUND => Ok(Message::NotFound(GetData::consensus_decode(r)?)),
            GET_SHARE_HEADERS => Ok(Message::GetShareHeaders(
                Vec::<BlockHash>::consensus_decode(r)?,
                BlockHash::consensus_decode(r)?,
            )),
            GET_SHARE_BLOCKS => Ok(Message::GetShareBlocks(
                Vec::<BlockHash>::consensus_decode(r)?,
                BlockHash::consensus_decode(r)?,
            )),
            SHARE_HEADERS => Ok(Message::ShareHeaders(ShareHeaderBatch::consensus_decode(
                r,
            )?)),
            SHARE_BLOCK => Ok(Message::ShareBlock(ShareBlock::consensus_decode(r)?)),
            GET_DATA => Ok(Message::GetData(GetData::consensus_decode(r)?)),
            TRANSACTION => Ok(Message::Transaction(
                bitcoin::Transaction::consensus_decode(r)?,
            )),
            HANDSHAKE => Ok(Message::Handshake(HandshakeData {
                tip_height: u32::consensus_decode(r)?,
                tip_hash: BlockHash::consensus_decode(r)?,
            })),
            ACK => Ok(Message::Ack),
            _ => Err(encode::Error::ParseFailed("Invalid Message discriminant")),
        }
    }
}

impl Encodable for RawMessage {
    fn consensus_encode<W: Write + ?Sized>(&self, w: &mut W) -> Result<usize, bitcoin::io::Error> {
        let mut len = 0;
        len += self.payload_len.consensus_encode(w)?;
        len += self.checksum.consensus_encode(w)?;
        len += self.payload.consensus_encode(w)?;
        Ok(len)
    }
}

impl Decodable for RawMessage {
    fn consensus_decode<R: Read + ?Sized>(r: &mut R) -> Result<Self, encode::Error> {
        // Read header
        let payload_len: u32 = Decodable::consensus_decode(r)?;
        let expected_checksum: [u8; 4] = Decodable::consensus_decode(r)?;

        // Reject an oversized advertised length before allocating, so a
        // malicious peer cannot trigger a multi-gigabyte allocation / OOM.
        if payload_len as usize > MAX_P2P_MESSAGE_SIZE {
            return Err(encode::Error::ParseFailed(
                "Payload length exceeds maximum message size",
            ));
        }

        // Read payload into buffer
        let mut payload_bytes = vec![0u8; payload_len as usize];
        r.read_exact(&mut payload_bytes)?;

        // Verify checksum
        let hash = sha256d::Hash::hash(&payload_bytes);
        let actual_checksum = [hash[0], hash[1], hash[2], hash[3]];
        if actual_checksum != expected_checksum {
            return Err(encode::Error::ParseFailed("Checksum mismatch"));
        }

        let payload = Message::consensus_decode(&mut &payload_bytes[..])?;

        Ok(RawMessage {
            payload_len,
            checksum: expected_checksum,
            payload,
        })
    }
}

/// The inventory message used to tell a peer what we have in our inventory.
/// The message can be used to tell the peer about share headers, blocks, or transactions that this peer has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InventoryMessage {
    BlockHashes(Vec<BlockHash>),
    TransactionHashes(Txids),
}

impl Encodable for InventoryMessage {
    fn consensus_encode<W: Write + ?Sized>(&self, w: &mut W) -> Result<usize, bitcoin::io::Error> {
        use inventory_discriminants::*;
        match self {
            InventoryMessage::BlockHashes(hashes) => {
                let mut len = BLOCK_HASHES.consensus_encode(w)?;
                len += hashes.consensus_encode(w)?;
                Ok(len)
            }
            InventoryMessage::TransactionHashes(txids) => {
                let mut len = TRANSACTION_HASHES.consensus_encode(w)?;
                len += txids.consensus_encode(w)?;
                Ok(len)
            }
        }
    }
}

impl Decodable for InventoryMessage {
    fn consensus_decode<R: Read + ?Sized>(r: &mut R) -> Result<Self, encode::Error> {
        use inventory_discriminants::*;
        let disc = u8::consensus_decode(r)?;
        match disc {
            BLOCK_HASHES => Ok(InventoryMessage::BlockHashes(
                Vec::<BlockHash>::consensus_decode(r)?,
            )),
            TRANSACTION_HASHES => Ok(InventoryMessage::TransactionHashes(
                Txids::consensus_decode(r)?,
            )),
            _ => Err(encode::Error::ParseFailed(
                "Invalid InventoryMessage discriminant",
            )),
        }
    }
}

/// Message for requesting data from peers
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum GetData {
    Block(BlockHash),
    Txid(Txid),
}

impl Encodable for GetData {
    fn consensus_encode<W: Write + ?Sized>(&self, w: &mut W) -> Result<usize, bitcoin::io::Error> {
        use getdata_discriminants::*;
        match self {
            GetData::Block(hash) => {
                let mut len = BLOCK.consensus_encode(w)?;
                len += hash.consensus_encode(w)?;
                Ok(len)
            }
            GetData::Txid(txid) => {
                let mut len = TXID.consensus_encode(w)?;
                len += txid.consensus_encode(w)?;
                Ok(len)
            }
        }
    }
}

impl Decodable for GetData {
    fn consensus_decode<R: Read + ?Sized>(r: &mut R) -> Result<Self, encode::Error> {
        use getdata_discriminants::*;
        let disc = u8::consensus_decode(r)?;
        match disc {
            BLOCK => Ok(GetData::Block(BlockHash::consensus_decode(r)?)),
            TXID => Ok(GetData::Txid(Txid::consensus_decode(r)?)),
            _ => Err(encode::Error::ParseFailed("Invalid GetData discriminant")),
        }
    }
}

#[cfg(test)]
mod tests;
