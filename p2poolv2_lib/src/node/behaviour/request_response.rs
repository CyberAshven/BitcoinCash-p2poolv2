// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use async_trait::async_trait;
use bitcoin::consensus::{Decodable, Encodable};
use bitcoin::hashes::{Hash, sha256d};
use libp2p::futures::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use libp2p::request_response::{Codec, OutboundFailure};
use std::io;

use crate::accounting::payout::coinbase_budget::bch_payout_rule_tag;
use crate::accounting::payout::sharechain_pplns::payout_rights::BCH_PAYOUT_HOLDER_RULE_TAG;
use crate::accounting::payout::sharechain_pplns::pplns_window::{
    BCH_COINBASE_MATURITY, BCH_PER_SHARE_CUTS_RULE_TAG, BCH_PPLNS_WINDOW_BLOCKS,
    BCH_WORK_WEIGHT_RULE_TAG, bch_depth_rule_tag,
};
use crate::config::Chain;
use crate::node::messages::{MAX_P2P_MESSAGE_SIZE, Message, RawMessage};
use crate::pool_difficulty::BCH_ASERT_ANCHOR_TAG;
use crate::shares::bch_script::BCH_SHARE_TRADE_RULE_TAG;
use crate::shares::coinbase_proof::bch_merkle_rule_tag;
use crate::shares::merge_mining::BCH_MERGE_MINING_RULE_TAG;
use crate::shares::share_version::BCH_SHARE_VERSION_RULE_TAG;
use crate::shares::validation::{
    BCH_NO_SEGWIT_RULE_TAG, BCH_PAYOUT_SCRIPT_RULE_TAG, BCH_SHARE_BODY_RULE_TAG,
    BCH_UNCLE_ONCE_RULE_TAG, bch_sig_checks_rule_tag,
};
use bitcoin::BlockHash;

/// Hex characters of the share chain genesis hash carried in the protocol
/// string. Eight is ample: this separates share chains that were never meant
/// to meet, it is not a security boundary, and the Noise handshake the string
/// feeds provides the cryptographic part.
const GENESIS_TAG_LEN: usize = 8;

/// Short, stable tag identifying the share chain a node is built for.
///
/// A share chain is reset by re-anchoring its genesis, which changes this tag
/// without anyone having to remember to bump a version. Two nodes built for
/// different chains therefore cannot derive the same protocol string.
fn genesis_tag(genesis_hash: BlockHash) -> String {
    let mut tag = genesis_hash.to_string();
    tag.truncate(GENESIS_TAG_LEN);
    tag
}

/// Tag naming the share chain rules a Bitcoin Cash node enforces beyond
/// upstream's, or `None` on Bitcoin, whose protocol strings stay upstream's.
///
/// Bitcoin Cash sizes the PPLNS window by the paying block's difficulty
/// (`BCH_PPLNS_WINDOW_BLOCKS`) behind its own maturity
/// (`BCH_COINBASE_MATURITY`). A node on the earlier fixed window, or with
/// another number of blocks or another maturity, rebuilds a different
/// coinbase for every share, so the two must not peer: they would only
/// exchange shares the other rejects. The tag is derived from the constants,
/// so changing either rule changes the tag without anyone having to remember
/// a version bump, as re-anchoring genesis does for `genesis_tag`.
///
/// Bitcoin Cash also anchors ASERT at each share's parent
/// (`BCH_ASERT_ANCHOR_TAG`). A node anchoring at genesis computes another
/// target for every share once the chain has been behind schedule, so it
/// is kept apart the same way.
///
/// Then come the share transaction rules (`BCH_SHARE_TRADE_RULE_TAG`): the
/// share-trading contract, dust-floored P2PKH or P2SH32 outputs and height
/// finality. A node without them disagrees on shares carrying a contract
/// spend, a dust output or an `OP_RETURN` output.
///
/// Then comes the payout to the holder of each share's payout right
/// (`BCH_PAYOUT_HOLDER_RULE_TAG`). A node paying `miner_bitcoin_address`
/// rebuilds another coinbase once any right has moved.
///
/// Then the hybrid payout (`bch_payout_rule_tag`, built from the token
/// share's coinbase limit and the payout tree's fan-out, fee rates, filler
/// and covenant version). A node on other values rebuilds another coinbase
/// whenever payees go to the tree.
///
/// Then the donation and fee taken from each share's own weight at its own
/// settings (`BCH_PER_SHARE_CUTS_RULE_TAG`). A node taking them off the
/// whole coinbase at the paying share's settings rebuilds another coinbase
/// whenever window shares differ in their cuts.
///
/// Then comes the rule that an uncle may have only one nephew on a chain
/// (`BCH_UNCLE_ONCE_RULE_TAG`). A node without it accepts a share naming an
/// uncle an ancestor already names, and pays that uncle twice.
///
/// Last come the other rules of the 2026-10-24 share-chain rule batch, one
/// segment each, in the batch's item order: the share body limit
/// (`BCH_SHARE_BODY_RULE_TAG`, item 2), payout scripts
/// (`BCH_PAYOUT_SCRIPT_RULE_TAG`, 3), the 32-level merkle branch
/// (`bch_merkle_rule_tag`, 4), depths from the 100-share maturity
/// (`bch_depth_rule_tag`, 5), SigChecks (`bch_sig_checks_rule_tag`, 6), no
/// SegWit commitment (`BCH_NO_SEGWIT_RULE_TAG`, 7), weights by work
/// (`BCH_WORK_WEIGHT_RULE_TAG`, 8), merge-mining format v2 with the ASERT
/// check (`BCH_MERGE_MINING_RULE_TAG`, 9) and the share-version vote
/// (`BCH_SHARE_VERSION_RULE_TAG`, 10). The batch's item 1, the hybrid
/// payout, took over the payout cap's slot above.
///
/// From this batch on, a rule change is a new share version that activates
/// by a vote of the miners' work (`shares::share_version`). It must not
/// change this tag: nodes on the old and the new version have to peer for
/// the vote to see both.
fn share_chain_rule_tag(chain: Chain) -> Option<String> {
    match chain {
        Chain::Bitcoin => None,
        Chain::BitcoinCash => Some(format!(
            "pplns-k{BCH_PPLNS_WINDOW_BLOCKS}-m{BCH_COINBASE_MATURITY}/{BCH_ASERT_ANCHOR_TAG}/{BCH_SHARE_TRADE_RULE_TAG}/{BCH_PAYOUT_HOLDER_RULE_TAG}/{}/{BCH_PER_SHARE_CUTS_RULE_TAG}/{BCH_UNCLE_ONCE_RULE_TAG}/{BCH_SHARE_BODY_RULE_TAG}/{BCH_PAYOUT_SCRIPT_RULE_TAG}/{}/{}/{}/{BCH_NO_SEGWIT_RULE_TAG}/{BCH_WORK_WEIGHT_RULE_TAG}/{BCH_MERGE_MINING_RULE_TAG}/{BCH_SHARE_VERSION_RULE_TAG}",
            bch_payout_rule_tag(),
            bch_merkle_rule_tag(),
            bch_depth_rule_tag(),
            bch_sig_checks_rule_tag(),
        )),
    }
}

/// The share chain part of every protocol string: the genesis tag, followed
/// on Bitcoin Cash by the rule tag.
fn share_chain_segment(chain: Chain, genesis_hash: BlockHash) -> String {
    match share_chain_rule_tag(chain) {
        Some(rule_tag) => format!("{}/{rule_tag}", genesis_tag(genesis_hash)),
        None => genesis_tag(genesis_hash),
    }
}

/// Build the libp2p protocol string for a bitcoin network and share chain.
///
/// Network isolation is enforced through protocol negotiation: nodes on
/// different networks, on share chains with different genesis blocks, or on
/// Bitcoin Cash with different payout rules derive different protocol strings
/// and therefore fail to negotiate a shared protocol. Centralizing
/// construction here keeps the string consistent across the request-response
/// protocol, Identify, and the Noise prologue.
pub fn protocol_string(network: bitcoin::Network, chain: Chain, genesis_hash: BlockHash) -> String {
    format!(
        "/p2pool/{}/{}/1.0.0",
        network.to_core_arg(),
        share_chain_segment(chain, genesis_hash)
    )
}

/// Build the Kademlia protocol string for a bitcoin network and share chain.
///
/// Kademlia uses a distinct protocol name from [`protocol_string`] but shares
/// the same network and share chain segments, so it is derived here to keep
/// both in step.
pub fn kad_protocol_string(
    network: bitcoin::Network,
    chain: Chain,
    genesis_hash: BlockHash,
) -> String {
    format!(
        "/p2pool/{}/{}/kad/1.0.0",
        network.to_core_arg(),
        share_chain_segment(chain, genesis_hash)
    )
}

// Protocol name for our request-response protocol
#[derive(Debug, Clone)]
pub struct P2PoolRequestResponseProtocol(String);

impl P2PoolRequestResponseProtocol {
    pub fn new(network: bitcoin::Network, chain: Chain, genesis_hash: BlockHash) -> Self {
        Self(protocol_string(network, chain, genesis_hash))
    }
}

impl AsRef<str> for P2PoolRequestResponseProtocol {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

// Consensus codec implementation using RawMessage for request-response protocols
#[derive(Clone)]
pub struct ConsensusCodec;

impl ConsensusCodec {
    async fn read_message<T>(&self, io: &mut T) -> io::Result<Message>
    where
        T: AsyncRead + Unpin + Send,
    {
        // Read header: payload_len (4) + checksum (4) = 8 bytes
        let mut header_bytes = [0u8; 8];
        io.read_exact(&mut header_bytes).await?;

        // Parse payload length and checksum from header
        let payload_len = u32::from_le_bytes([
            header_bytes[0],
            header_bytes[1],
            header_bytes[2],
            header_bytes[3],
        ]);
        let expected_checksum = [
            header_bytes[4],
            header_bytes[5],
            header_bytes[6],
            header_bytes[7],
        ];

        // Reject an oversized advertised length before allocating, so a
        // malicious peer cannot trigger a multi-gigabyte allocation / OOM.
        let payload_len = payload_len as usize;
        if payload_len > MAX_P2P_MESSAGE_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Payload length exceeds maximum message size",
            ));
        }

        // Read exactly payload_len bytes
        let mut payload_bytes = vec![0u8; payload_len];
        io.read_exact(&mut payload_bytes).await?;

        // Verify the payload checksum before decoding
        let hash = sha256d::Hash::hash(&payload_bytes);
        let actual_checksum = [hash[0], hash[1], hash[2], hash[3]];
        if actual_checksum != expected_checksum {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Checksum mismatch",
            ));
        }

        let message = Message::consensus_decode(&mut &payload_bytes[..])
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

        Ok(message)
    }

    async fn write_message<T>(&self, io: &mut T, msg: Message) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        let raw_msg = RawMessage::new(msg);
        let mut bytes = Vec::new();
        raw_msg
            .consensus_encode(&mut bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        io.write_all(&bytes).await
    }
}

#[async_trait]
impl Codec for ConsensusCodec {
    type Protocol = P2PoolRequestResponseProtocol;
    type Request = Message;
    type Response = Message;

    async fn read_request<T>(&mut self, _: &Self::Protocol, io: &mut T) -> io::Result<Self::Request>
    where
        T: AsyncRead + Unpin + Send,
    {
        self.read_message(io).await
    }

    async fn read_response<T>(
        &mut self,
        _: &Self::Protocol,
        io: &mut T,
    ) -> io::Result<Self::Response>
    where
        T: AsyncRead + Unpin + Send,
    {
        self.read_message(io).await
    }

    async fn write_request<T>(
        &mut self,
        _: &Self::Protocol,
        io: &mut T,
        req: Self::Request,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        self.write_message(io, req).await
    }

    async fn write_response<T>(
        &mut self,
        _: &Self::Protocol,
        io: &mut T,
        res: Self::Response,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        self.write_message(io, res).await
    }
}

// Helper type aliases for the request-response behavior
pub type RequestResponseBehaviour = libp2p::request_response::Behaviour<ConsensusCodec>;
pub type RequestResponseEvent = libp2p::request_response::Event<Message, Message>;

// Error type for request-response failures
#[derive(Debug, thiserror::Error)]
pub enum RequestResponseError {
    #[error("Outbound request failed: {0}")]
    OutboundFailure(#[from] OutboundFailure),

    #[error("IO error: {0}")]
    Io(#[from] io::Error),
}

#[cfg(test)]
mod tests {
    use super::{GENESIS_TAG_LEN, kad_protocol_string, protocol_string};
    use crate::config::{BchShareChain, Chain};
    use crate::shares::bch_genesis;
    use crate::shares::share_block::ShareBlock;
    use p2poolv2_wallet::bch_share::BchShareNetwork;

    #[test]
    fn test_protocol_string_carries_network_and_genesis_tag() {
        let genesis_hash = ShareBlock::build_genesis_for_network(bitcoin::Network::Signet)
            .unwrap()
            .block_hash();
        let expected_tag = &genesis_hash.to_string()[..GENESIS_TAG_LEN];

        assert_eq!(
            protocol_string(bitcoin::Network::Signet, Chain::Bitcoin, genesis_hash),
            format!("/p2pool/signet/{expected_tag}/1.0.0")
        );
    }

    /// The isolation property a chain reset relies on: re-anchoring genesis
    /// changes the protocol string even though the bitcoin network is
    /// unchanged, so nodes on the old chain cannot negotiate with new ones.
    #[test]
    fn test_protocol_string_differs_when_genesis_differs_on_same_network() {
        let genesis_hash = ShareBlock::build_genesis_for_network(bitcoin::Network::Testnet4)
            .unwrap()
            .block_hash();
        let other_genesis_hash = ShareBlock::build_genesis_for_network(bitcoin::Network::Signet)
            .unwrap()
            .block_hash();
        assert_ne!(genesis_hash, other_genesis_hash);

        assert_ne!(
            protocol_string(bitcoin::Network::Testnet4, Chain::Bitcoin, genesis_hash),
            protocol_string(
                bitcoin::Network::Testnet4,
                Chain::Bitcoin,
                other_genesis_hash
            )
        );
    }

    /// Kademlia negotiates a distinct protocol name, but must isolate on the
    /// same genesis so a peer cannot join the DHT of a chain it is not on.
    #[test]
    fn test_kad_protocol_string_is_distinct_but_carries_same_genesis_tag() {
        let genesis_hash = ShareBlock::build_genesis_for_network(bitcoin::Network::Signet)
            .unwrap()
            .block_hash();
        let expected_tag = &genesis_hash.to_string()[..GENESIS_TAG_LEN];

        let kad = kad_protocol_string(bitcoin::Network::Signet, Chain::Bitcoin, genesis_hash);
        assert_eq!(kad, format!("/p2pool/signet/{expected_tag}/kad/1.0.0"));
        assert_ne!(
            kad,
            protocol_string(bitcoin::Network::Signet, Chain::Bitcoin, genesis_hash)
        );
    }

    /// A Bitcoin Cash node names its payout rule after the genesis tag. The
    /// string feeds the Noise prologue, Identify and request-response, so a
    /// peer on another rule fails the handshake.
    #[test]
    fn test_bitcoin_cash_protocol_string_carries_the_payout_rule() {
        let genesis_hash = bch_genesis::build(BchShareNetwork::Chipnet, BchShareChain::Main)
            .unwrap()
            .block_hash();
        let expected_tag = &genesis_hash.to_string()[..GENESIS_TAG_LEN];

        assert_eq!(
            protocol_string(bitcoin::Network::Testnet, Chain::BitcoinCash, genesis_hash),
            format!(
                "/p2pool/test/{expected_tag}/pplns-k3-m100/asert-parent/trade-htlc/payout-holder/payout-hybrid-t3072-f500-r1to3-z8-cov1/cuts-per-share/uncle-once/body-1000000-204800/payee-p2pkh-p2sh20-p2sh32/merkle32/depths-m100/sigchecks3000-1452/no-segwit/weight-work/mm2-roots-asert8/version-vote/1.0.0"
            )
        );
    }

    /// A chipnet node from before the parent anchor derived
    /// `/p2pool/test/<genesis tag>/pplns-k3-m100/1.0.0` from the same
    /// genesis. It computes other ASERT targets, so the new string differs
    /// and the two cannot negotiate.
    #[test]
    fn test_bitcoin_cash_protocol_string_differs_from_the_genesis_anchor_string() {
        let genesis_hash = bch_genesis::build(BchShareNetwork::Chipnet, BchShareChain::Mini)
            .unwrap()
            .block_hash();
        let expected_tag = &genesis_hash.to_string()[..GENESIS_TAG_LEN];
        let genesis_anchor_string = format!("/p2pool/test/{expected_tag}/pplns-k3-m100/1.0.0");

        assert_ne!(
            protocol_string(bitcoin::Network::Testnet, Chain::BitcoinCash, genesis_hash),
            genesis_anchor_string
        );
    }

    /// A chipnet node from before the share-trading rules derived
    /// `/p2pool/test/<genesis tag>/pplns-k3-m100/asert-parent/1.0.0`. It
    /// rejects contract spends and accepts dust outputs, so the new string
    /// differs and the two cannot negotiate.
    #[test]
    fn test_bitcoin_cash_protocol_string_differs_from_the_string_before_share_trading() {
        let genesis_hash = bch_genesis::build(BchShareNetwork::Chipnet, BchShareChain::Mini)
            .unwrap()
            .block_hash();
        let expected_tag = &genesis_hash.to_string()[..GENESIS_TAG_LEN];
        let before_trading_string =
            format!("/p2pool/test/{expected_tag}/pplns-k3-m100/asert-parent/1.0.0");

        assert_ne!(
            protocol_string(bitcoin::Network::Testnet, Chain::BitcoinCash, genesis_hash),
            before_trading_string
        );
    }

    /// A chipnet node from before the coinbase budget derived
    /// `/p2pool/test/<genesis tag>/pplns-k3-m100/asert-parent/trade-htlc/payout-holder/1.0.0`.
    /// It pays every payee, so it builds another coinbase for any window
    /// past the cap; the new string differs and the two cannot negotiate.
    #[test]
    fn test_bitcoin_cash_protocol_string_differs_from_the_string_before_the_coinbase_budget() {
        let genesis_hash = bch_genesis::build(BchShareNetwork::Chipnet, BchShareChain::Mini)
            .unwrap()
            .block_hash();
        let expected_tag = &genesis_hash.to_string()[..GENESIS_TAG_LEN];
        let before_budget_string = format!(
            "/p2pool/test/{expected_tag}/pplns-k3-m100/asert-parent/trade-htlc/payout-holder/1.0.0"
        );

        assert_ne!(
            protocol_string(bitcoin::Network::Testnet, Chain::BitcoinCash, genesis_hash),
            before_budget_string
        );
    }

    /// A chipnet node from before the one-nephew uncle rule derived
    /// `.../trade-htlc/payout-holder/payout-hybrid-t3072-f500-r1to3-z8-cov1/1.0.0`. It accepts a share
    /// naming an uncle an ancestor already names, so the new string differs
    /// and the two cannot negotiate.
    #[test]
    fn test_bitcoin_cash_protocol_string_differs_from_the_string_before_uncle_once() {
        let genesis_hash = bch_genesis::build(BchShareNetwork::Chipnet, BchShareChain::Main)
            .unwrap()
            .block_hash();
        let expected_tag = &genesis_hash.to_string()[..GENESIS_TAG_LEN];
        let before_uncle_once_string = format!(
            "/p2pool/test/{expected_tag}/pplns-k3-m100/asert-parent/trade-htlc/payout-holder/payout-hybrid-t3072-f500-r1to3-z8-cov1/1.0.0"
        );

        assert_ne!(
            protocol_string(bitcoin::Network::Testnet, Chain::BitcoinCash, genesis_hash),
            before_uncle_once_string
        );
    }

    /// A chipnet node from before the 2026-10-24 rule batch derived
    /// `.../payout-holder/payout-cap11/uncle-once/1.0.0`. It runs none of
    /// the batch's rules, so the new string differs and the two cannot
    /// negotiate; the batch also starts a fresh share chain.
    #[test]
    fn test_bitcoin_cash_protocol_string_differs_from_the_string_before_the_rule_batch() {
        let genesis_hash = bch_genesis::build(BchShareNetwork::Chipnet, BchShareChain::Main)
            .unwrap()
            .block_hash();
        let expected_tag = &genesis_hash.to_string()[..GENESIS_TAG_LEN];
        let before_batch_string = format!(
            "/p2pool/test/{expected_tag}/pplns-k3-m100/asert-parent/trade-htlc/payout-holder/payout-cap11/uncle-once/1.0.0"
        );
        let string = protocol_string(bitcoin::Network::Testnet, Chain::BitcoinCash, genesis_hash);
        assert_ne!(string, before_batch_string);
        assert!(!string.contains("payout-cap11"));
    }

    /// The batch's tag as first pushed had neither per-share cuts nor the
    /// test networks' x8 ASERT easy side (`.../payout-hybrid-.../uncle-once/
    /// .../mm2-roots-asert/version-vote`). A node on it takes the cuts off
    /// the whole coinbase and refuses chipnet token shares dated a few
    /// minutes before their parent, so the final tag differs from it.
    #[test]
    fn test_bitcoin_cash_protocol_string_differs_from_the_batch_before_its_review_fixes() {
        let genesis_hash = bch_genesis::build(BchShareNetwork::Chipnet, BchShareChain::Main)
            .unwrap()
            .block_hash();
        let expected_tag = &genesis_hash.to_string()[..GENESIS_TAG_LEN];
        let before_fixes_string = format!(
            "/p2pool/test/{expected_tag}/pplns-k3-m100/asert-parent/trade-htlc/payout-holder/payout-hybrid-t3072-f500-r1to3-z8-cov1/uncle-once/body-1000000-204800/payee-p2pkh-p2sh20-p2sh32/merkle32/depths-m100/sigchecks3000-1452/no-segwit/weight-work/mm2-roots-asert/version-vote/1.0.0"
        );
        let string = protocol_string(bitcoin::Network::Testnet, Chain::BitcoinCash, genesis_hash);
        assert_ne!(string, before_fixes_string);
        assert!(string.contains("/cuts-per-share/"));
        assert!(string.contains("/mm2-roots-asert8/"));
    }

    /// The isolation the window rule relies on: a chipnet node from before
    /// the rule derived `/p2pool/test/<genesis tag>/1.0.0` from the same
    /// genesis. The new string differs, so the two cannot negotiate and never
    /// exchange shares the other would reject.
    #[test]
    fn test_bitcoin_cash_protocol_string_differs_from_the_fixed_window_string() {
        let genesis_hash = bch_genesis::build(BchShareNetwork::Chipnet, BchShareChain::Main)
            .unwrap()
            .block_hash();
        let expected_tag = &genesis_hash.to_string()[..GENESIS_TAG_LEN];
        let fixed_window_string = format!("/p2pool/test/{expected_tag}/1.0.0");

        assert_ne!(
            protocol_string(bitcoin::Network::Testnet, Chain::BitcoinCash, genesis_hash),
            fixed_window_string
        );
    }

    /// Kademlia carries the same payout rule, so a fixed-window node cannot
    /// join a sized-window node's DHT either.
    #[test]
    fn test_bitcoin_cash_kad_protocol_string_carries_the_payout_rule() {
        let genesis_hash = bch_genesis::build(BchShareNetwork::Chipnet, BchShareChain::Main)
            .unwrap()
            .block_hash();
        let expected_tag = &genesis_hash.to_string()[..GENESIS_TAG_LEN];

        assert_eq!(
            kad_protocol_string(bitcoin::Network::Testnet, Chain::BitcoinCash, genesis_hash),
            format!(
                "/p2pool/test/{expected_tag}/pplns-k3-m100/asert-parent/trade-htlc/payout-holder/payout-hybrid-t3072-f500-r1to3-z8-cov1/cuts-per-share/uncle-once/body-1000000-204800/payee-p2pkh-p2sh20-p2sh32/merkle32/depths-m100/sigchecks3000-1452/no-segwit/weight-work/mm2-roots-asert8/version-vote/kad/1.0.0"
            )
        );
    }
}
