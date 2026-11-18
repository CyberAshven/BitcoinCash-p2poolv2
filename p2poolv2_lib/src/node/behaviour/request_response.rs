// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use bitcoin::consensus::Encodable;
use bitcoin::hashes::{Hash, sha256d};
use libp2p::futures::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use libp2p::request_response::{Codec, OutboundFailure};
use std::io;

use crate::accounting::payout::coinbase_budget::bch_payout_rule_tag;
use crate::accounting::payout::payout_levels::BCH_PAYOUT_LEVELS_RULE_TAG;
use crate::accounting::payout::sharechain_pplns::payout_rights::BCH_PAYOUT_HOLDER_RULE_TAG;
use crate::accounting::payout::sharechain_pplns::pplns_window::{
    BCH_COINBASE_MATURITY, BCH_GENESIS_LAST_RESORT_RULE_TAG, BCH_PER_SHARE_CUTS_RULE_TAG,
    BCH_PPLNS_WINDOW_BLOCKS, BCH_WORK_WEIGHT_RULE_TAG, bch_depth_rule_tag,
};
use crate::config::Chain;
use crate::node::messages::{MAX_P2P_MESSAGE_SIZE, Message, RawMessage};
use crate::pool_difficulty::BCH_ASERT_ANCHOR_TAG;
use crate::shares::band::BCH_BAND_RULE_TAG;
use crate::shares::bch_headers::BCH_CURRENT_CHAIN_RULE_TAG;
use crate::shares::bch_script::BCH_SHARE_TRADE_RULE_TAG;
use crate::shares::coinbase_proof::bch_merkle_rule_tag;
use crate::shares::merge_mining::BCH_MERGE_MINING_RULE_TAG;
use crate::shares::share_version::BCH_SHARE_VERSION_RULE_TAG;
use crate::shares::validation::{
    BCH_NO_SEGWIT_RULE_TAG, BCH_PAYOUT_SCRIPT_RULE_TAG, BCH_SHARE_BODY_RULE_TAG,
    BCH_UNCLE_ONCE_RULE_TAG, bch_sig_checks_rule_tag,
};
use crate::shares::weak::BCH_WEAK_SHARE_RULE_TAG;
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
/// From that batch on, a rule change was to be a new share version that
/// activates by a vote of the miners' work (`shares::share_version`),
/// keeping this tag so nodes on the old and the new version peer for the
/// vote to see both. Mining on the current chain
/// (`BCH_CURRENT_CHAIN_RULE_TAG`, 2026-10-30) is the owner's exception for
/// the alpha: it changes the share messages themselves, which now carry
/// Bitcoin Cash headers a node before it cannot read, so it ships as a flag
/// day with its own last segment. Later rule changes go through the vote.
///
/// Paying the genesis share only as the last resort
/// (`BCH_GENESIS_LAST_RESORT_RULE_TAG`, 2026-11-05) is the owner's second
/// exception, also a flag day with its own segment. It changes a coinbase
/// only while genesis is in the window, in a new chain's first hours, and a
/// vote cannot switch a chain before its parent is
/// `SHARE_VERSION_VOTE_DEPTH` shares high, three days, by when genesis has
/// left every window.
///
/// The difficulty band (`BCH_BAND_RULE_TAG`, 2026-11-09, issue #79 row 3) is
/// the third flag day, shipped with the merge of the main and mini chains
/// into one chain at the mini chain's genesis. A share may be up to `C_MAX`
/// (64) times harder than the minimum, with its minimum, the target its node
/// wanted and its band cap in its header (`shares::band`), and the next
/// share's ASERT anchors at the minimum. A node before it cannot read the
/// longer header and would refuse every heavier share, and the merged chain
/// starts from a new genesis, so every node upgrades together on a fresh
/// store rather than through the vote.
///
/// Payout levels (`BCH_PAYOUT_LEVELS_RULE_TAG`, 2026-11-09, issue #79 row 3,
/// part 4) are the fourth flag day, on the same restart. Each share header
/// carries its miner's payout level, and a slice of a found block below its
/// payee's level `546 * 2^k` satoshis is paid by a draw among the slices at
/// that level, seeded from the share's parent (`payout_levels`). A node
/// before it cannot read the longer header and would rebuild another
/// coinbase whenever a payee is below its level, and the header change gives
/// the chain a new genesis, so every node upgrades together on a fresh store
/// rather than through the vote.
///
/// Weak shares (`BCH_WEAK_SHARE_RULE_TAG`, 2026-11-09, issue #79 row 5) ship
/// on the same flag day: a share carries up to `WEAK_CAP` easier proofs
/// of work on jobs at the minimum, each with its parent at most `WEAK_DEPTH`
/// shares below it, and the PPLNS window credits every proof from a job
/// at the minimum at the weak work, with `INCLUDER_CUT` (100 basis points) of
/// each weak share's credit to the share carrying it (`shares::weak`). The
/// share header carries a weak ratio and commits the carried weak shares,
/// and weak shares travel in a new message, none of which a node before it
/// can read, so it cannot go through the vote.
///
/// The segment became `weak2` on 2026-11-12, a flag day of its own: a
/// share's weak root now commits each carried weak share's whole encoding
/// rather than its proof-of-work hash alone (`shares::weak::weak_root`). The
/// old root left a carried weak share's coinbase value, branch, scriptSig and
/// coinbase proof free for a relaying peer to change, and the change made an
/// honest share invalid after its body was stored. A node on `weak` computes
/// another root for every share carrying weak shares and refuses it, so the
/// two cannot share a chain, and the vote cannot carry a fix to what a body
/// check binds while the old rule lets any peer split a node off.
///
/// Its numbers changed on 2026-11-12, another flag day (study #85):
/// `INCLUDER_CUT` from 500 to 100 basis points, `WEAK_DEPTH` from 6 to 48 and
/// `R_MAX` from 4096 to 256, now in the segment. Nodes on the two credit weak
/// shares differently and start from different genesis shares, so they
/// cannot share a chain.
///
/// Payout levels' lowest rung changed on 2026-11-12, another flag day
/// (`BCH_PAYOUT_LEVELS_RULE_TAG`, now `levels-546x2-k1`). Level 0 is not a
/// valid payout level: it paid 546 satoshis, which is below dust once the
/// payout tree's fee is taken, so a level 0 win that landed in the tree went
/// to the tree's largest payee. A node on `levels-546x2` still accepts that
/// header and pays the win, and the genesis share's level byte changed, so
/// the two cannot share a chain.
///
/// The weak segment became `weak3` on 2026-11-12, another flag day: a weak
/// share a share carries is held to its carrier's time, not to each node's
/// clock (`check_carried_weak_time`). On `weak2` a share carrying a weak
/// share stamped two minutes ahead was valid on a node whose clock had
/// passed that time and invalid for good on one whose clock had not.
///
/// The weak segment became `weak4-640-100-48-256-320-64-4096-64-32` on 2026-11-15,
/// another flag day (issue #79, part 6 of the design page): the weak ratio
/// follows the weak shares that ask. A weak share asks when its
/// `wanted_bits` is easier than the weak target it met, each share header
/// counts the asking weak shares it carries as `weak_asking`, and while more
/// than one in `WEAK_ASK_DIVISOR` (64) of them ask the weak ratio rises, past
/// `R_MAX` as far as `R_ASK_MAX` (4096), until a share carries more than
/// `WEAK_FILL_MAX` (320); a share may carry `WEAK_CAP` (640) weak shares,
/// where it carried 64. A node on `weak3` cannot read the longer header,
/// refuses a share whose `wanted_bits` is easier than its own target and
/// steps the weak ratio without the asks, and the header change gives the
/// chain a new genesis, so every node upgrades together on a fresh store
/// rather than through the vote.
///
/// The weak segment gained `-summary` on 2026-11-16, before any node ran
/// `weak4`, another flag day: a share's weak root also commits the summary
/// of the weak shares it carries, so a node that pruned them one payout
/// depth below its tip serves the body with their leaf root and summary, and
/// a syncing peer checks those against the header. Every share carrying
/// weak shares has another root under the two, and a node without it cannot
/// read a body served that way.
///
/// The current-chain and merge-mining segments gained `-ticks` on
/// 2026-11-15, another flag day (`bch-prev-1off-ticks`,
/// `mm2-roots-asert8-ticks`): each known Bitcoin Cash header carries its
/// CHIP-2025-03 ticks and next block interval, worked out from its own
/// branch's median time past as BCHN does, and the nBits a peer's header
/// must carry, the nBits that size a payout window and a token share's
/// ASERT band follow them. Before, each node learned one-minute blocks
/// while running from its own Bitcoin Cash node, so two nodes could require
/// different nBits of one header; the two rules cannot share a chain.
fn share_chain_rule_tag(chain: Chain) -> Option<String> {
    match chain {
        Chain::Bitcoin => None,
        Chain::BitcoinCash => Some(format!(
            "pplns-k{BCH_PPLNS_WINDOW_BLOCKS}-m{BCH_COINBASE_MATURITY}/{BCH_ASERT_ANCHOR_TAG}/{BCH_SHARE_TRADE_RULE_TAG}/{BCH_PAYOUT_HOLDER_RULE_TAG}/{}/{BCH_PER_SHARE_CUTS_RULE_TAG}/{BCH_UNCLE_ONCE_RULE_TAG}/{BCH_SHARE_BODY_RULE_TAG}/{BCH_PAYOUT_SCRIPT_RULE_TAG}/{}/{}/{}/{BCH_NO_SEGWIT_RULE_TAG}/{BCH_WORK_WEIGHT_RULE_TAG}/{BCH_MERGE_MINING_RULE_TAG}/{BCH_SHARE_VERSION_RULE_TAG}/{BCH_CURRENT_CHAIN_RULE_TAG}/{BCH_GENESIS_LAST_RESORT_RULE_TAG}/{BCH_BAND_RULE_TAG}/{BCH_PAYOUT_LEVELS_RULE_TAG}/{BCH_WEAK_SHARE_RULE_TAG}",
            bch_payout_rule_tag(),
            bch_merkle_rule_tag(),
            bch_depth_rule_tag(),
            bch_sig_checks_rule_tag(),
        )),
    }
}

/// Name of the P2P messages a Bitcoin Cash node speaks beyond upstream's,
/// carried after the rule tag.
///
/// Not a share-chain rule: the shares, their headers, their weak roots and
/// what a node stores are the same either side of it. But two nodes that
/// cannot read each other's messages must not peer either, or one decodes
/// the other's message as malformed and the share in it is lost, so a
/// change to the messages changes this segment and the Noise prologue keeps
/// the two apart, as it does for a rule. The network is in alpha: every node
/// restarts on the new binary.
///
/// `weak-by-hash` since 2026-11-15: a new share relayed to peers lists the
/// weak shares it carries by proof-of-work hash (`ShareWithWeakHashes`),
/// sending whole beside the list only the ones its peers probably lack, and
/// a peer whose weak pool still lacks some asks the sender for them
/// (`GetCarriedWeakShares`, answered with `CarriedWeakShares`). A node
/// before it cannot read any of the three. Sync still sends weak shares
/// whole.
///
/// `without-payouts` since 2026-11-16: a new share whose coinbase outputs
/// are large (`RELAY_WITHOUT_PAYOUTS_MIN_BYTES`) is relayed without them
/// (`ShareWithoutPayouts`, which also lists the weak shares by hash), and the
/// receiver rebuilds them from its PPLNS window as validation does. A
/// smaller share still goes whole, as `ShareWithWeakHashes` or `ShareBlock`.
/// A node before it cannot read the new message. Sync still sends shares
/// whole.
///
/// `headers-resume` since 2026-11-18, with upstream's request-response
/// protocol 1.1.0 (`ProtocolVersion`): `GetShareHeaders` carries the last
/// header the requester got (`resume_after`), and the answer serves every
/// share at a height, a page ending inside a height if it fills, where 1.0.0
/// cuts each height at `MAX_BLOCKS_PER_HEIGHT` shares. Upstream picks the
/// version per stream, so without the segment a node before it would peer on
/// 1.0.0 and get cut heights, which can leave out the share the chain goes
/// through. With it, Bitcoin Cash nodes always speak 1.1.0 to each other;
/// 1.0.0 stays for a Bitcoin node and upstream's nodes.
const BCH_MESSAGES_TAG: &str = "weak-by-hash/without-payouts/headers-resume";

/// The share chain part of every protocol string: the genesis tag, followed
/// on Bitcoin Cash by the rule tag and the message tag.
fn share_chain_segment(chain: Chain, genesis_hash: BlockHash) -> String {
    match share_chain_rule_tag(chain) {
        Some(rule_tag) => format!(
            "{}/{rule_tag}/{BCH_MESSAGES_TAG}",
            genesis_tag(genesis_hash)
        ),
        None => genesis_tag(genesis_hash),
    }
}

/// Version of the request-response protocol negotiated on a stream.
///
/// The version selects how version-dependent messages are serialized and how
/// a request is served, so a node can keep talking to peers that only know an
/// older version.
///
/// The version is independent of share chain isolation. A chain reset
/// re-anchors genesis, which changes the genesis tag in every protocol string
/// (see [`network_prologue`]); it does not need, and must not rely on, a
/// version bump.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolVersion {
    /// Original protocol. Header responses truncate each height to
    /// `MAX_BLOCKS_PER_HEIGHT` blocks.
    V1_0,
    /// Header responses serve every block at a height and page with a resume
    /// point inside a height.
    V1_1,
}

impl ProtocolVersion {
    /// Version segment of the protocol string.
    fn as_str(self) -> &'static str {
        match self {
            ProtocolVersion::V1_0 => "1.0.0",
            ProtocolVersion::V1_1 => "1.1.0",
        }
    }
}

/// Request-response versions this node supports, most preferred first.
///
/// Outbound streams offer the versions in this order and multistream-select
/// takes the first one the remote accepts, so two current nodes negotiate the
/// newest version and a current node falls back to 1.0.0 with an older peer.
pub const SUPPORTED_PROTOCOL_VERSIONS: [ProtocolVersion; 2] =
    [ProtocolVersion::V1_1, ProtocolVersion::V1_0];

/// Build the network identifier for a bitcoin network and share chain.
///
/// Used as the Noise prologue and the Identify protocol version. Network
/// isolation is enforced through the network and share chain segments: nodes
/// on different networks, on share chains with different genesis blocks, or
/// on Bitcoin Cash with different rules or messages derive different
/// prologues and therefore fail the Noise handshake. The same segments are
/// carried by [`request_response_protocol_string`] and
/// [`kad_protocol_string`], so such nodes also fail to negotiate any shared
/// protocol. Centralizing construction here keeps the segments consistent
/// across the Noise prologue, Identify, request-response and Kademlia.
///
/// The string keeps its `1.0.0` suffix and does not follow the
/// request-response protocol version. Noise requires byte-identical
/// prologues, so changing it would stop old and new nodes from connecting at
/// all and leave nothing to negotiate a version over. On Bitcoin it is the
/// string v0.18 nodes use; on Bitcoin Cash the rule and message tags in its
/// share chain segment are what keep nodes apart.
pub fn network_prologue(
    network: bitcoin::Network,
    chain: Chain,
    genesis_hash: BlockHash,
) -> String {
    format!(
        "/p2pool/{}/{}/1.0.0",
        network.to_core_arg(),
        share_chain_segment(chain, genesis_hash)
    )
}

/// Build the request-response protocol string for a bitcoin network, share
/// chain and protocol version.
///
/// Carries the same network and share chain segments as
/// [`network_prologue`], so nodes on different networks, or on share chains
/// with different genesis blocks or rules, fail to negotiate a stream
/// whatever version they support. Only the last segment varies with
/// `version`. The 1.0.0 string equals the one v0.18 nodes offer.
///
/// The network and share chain segments are not strictly required for 1.1.0,
/// since the Noise prologue already isolates chains, but 1.0.0 needs them for
/// compatibility and keeping them helps debugging.
pub fn request_response_protocol_string(
    network: bitcoin::Network,
    chain: Chain,
    genesis_hash: BlockHash,
    version: ProtocolVersion,
) -> String {
    format!(
        "/p2pool/{}/{}/{}",
        network.to_core_arg(),
        share_chain_segment(chain, genesis_hash),
        version.as_str()
    )
}

/// Build the Kademlia protocol string for a bitcoin network and share chain.
///
/// Kademlia uses a distinct protocol name from [`network_prologue`] but shares
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
pub struct P2PoolRequestResponseProtocol {
    name: String,
    version: ProtocolVersion,
}

impl P2PoolRequestResponseProtocol {
    /// Protocol for `version` on the given network and share chain.
    pub fn new(
        network: bitcoin::Network,
        chain: Chain,
        genesis_hash: BlockHash,
        version: ProtocolVersion,
    ) -> Self {
        Self {
            name: request_response_protocol_string(network, chain, genesis_hash, version),
            version,
        }
    }

    /// Version this protocol negotiates.
    pub fn version(&self) -> ProtocolVersion {
        self.version
    }
}

impl AsRef<str> for P2PoolRequestResponseProtocol {
    fn as_ref(&self) -> &str {
        &self.name
    }
}

// Consensus codec implementation using RawMessage for request-response protocols
#[derive(Clone)]
pub struct ConsensusCodec;

impl ConsensusCodec {
    /// Read one framed message, decoding the payload in the layout of the
    /// stream's protocol `version`.
    async fn read_message<T>(&self, version: ProtocolVersion, io: &mut T) -> io::Result<Message>
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

        let message = Message::consensus_decode_with_version(version, &mut &payload_bytes[..])
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

        Ok(message)
    }

    /// Write one framed message, encoding the payload in the layout of the
    /// stream's protocol `version`.
    async fn write_message<T>(
        &self,
        version: ProtocolVersion,
        io: &mut T,
        msg: Message,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        let raw_msg = RawMessage::new_with_version(version, msg);
        let mut bytes = Vec::new();
        raw_msg
            .consensus_encode(&mut bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        io.write_all(&bytes).await
    }
}

impl Codec for ConsensusCodec {
    type Protocol = P2PoolRequestResponseProtocol;
    type Request = Message;
    type Response = Message;

    async fn read_request<T>(
        &mut self,
        protocol: &Self::Protocol,
        io: &mut T,
    ) -> io::Result<Self::Request>
    where
        T: AsyncRead + Unpin + Send,
    {
        self.read_message(protocol.version(), io).await
    }

    async fn read_response<T>(
        &mut self,
        protocol: &Self::Protocol,
        io: &mut T,
    ) -> io::Result<Self::Response>
    where
        T: AsyncRead + Unpin + Send,
    {
        self.read_message(protocol.version(), io).await
    }

    async fn write_request<T>(
        &mut self,
        protocol: &Self::Protocol,
        io: &mut T,
        req: Self::Request,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        self.write_message(protocol.version(), io, req).await
    }

    async fn write_response<T>(
        &mut self,
        protocol: &Self::Protocol,
        io: &mut T,
        res: Self::Response,
    ) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        self.write_message(protocol.version(), io, res).await
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
    use super::{
        BCH_MESSAGES_TAG, ConsensusCodec, GENESIS_TAG_LEN, P2PoolRequestResponseProtocol,
        ProtocolVersion, kad_protocol_string, network_prologue, request_response_protocol_string,
    };
    use crate::config::Chain;
    use crate::node::messages::{GetShareHeaders, Message};
    use crate::shares::bch_genesis;
    use crate::shares::share_block::ShareBlock;
    use bitcoin::BlockHash;
    use bitcoin::hashes::Hash;
    use libp2p::futures::io::Cursor;
    use libp2p::request_response::Codec;
    use p2poolv2_wallet::bch_share::BchShareNetwork;
    use std::str::FromStr;

    /// A Bitcoin Cash protocol string without its message tag: the rule
    /// tests below take the rule segments off it one flag day at a time.
    fn rules_only(protocol_string: String) -> String {
        protocol_string.replace(&format!("/{BCH_MESSAGES_TAG}/"), "/")
    }

    /// The prologue must stay byte-identical to the string v0.18 nodes use,
    /// or the Noise handshake between old and new nodes fails.
    #[test]
    fn test_network_prologue_carries_network_and_genesis_tag() {
        let genesis_hash = ShareBlock::build_genesis_for_network(bitcoin::Network::Signet)
            .unwrap()
            .block_hash();
        let expected_tag = &genesis_hash.to_string()[..GENESIS_TAG_LEN];

        assert_eq!(
            network_prologue(bitcoin::Network::Signet, Chain::Bitcoin, genesis_hash),
            format!("/p2pool/signet/{expected_tag}/1.0.0")
        );
    }

    /// The isolation property a chain reset relies on: re-anchoring genesis
    /// changes the prologue even though the bitcoin network is unchanged, so
    /// nodes on the old chain cannot connect to new ones.
    #[test]
    fn test_network_prologue_differs_when_genesis_differs_on_same_network() {
        let genesis_hash = ShareBlock::build_genesis_for_network(bitcoin::Network::Testnet4)
            .unwrap()
            .block_hash();
        let other_genesis_hash = ShareBlock::build_genesis_for_network(bitcoin::Network::Signet)
            .unwrap()
            .block_hash();
        assert_ne!(genesis_hash, other_genesis_hash);

        assert_ne!(
            network_prologue(bitcoin::Network::Testnet4, Chain::Bitcoin, genesis_hash),
            network_prologue(
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
            network_prologue(bitcoin::Network::Signet, Chain::Bitcoin, genesis_hash)
        );
    }

    /// A Bitcoin Cash node names its payout rule after the genesis tag. The
    /// string feeds the Noise prologue, Identify and request-response, so a
    /// peer on another rule fails the handshake.
    #[test]
    fn test_bitcoin_cash_protocol_string_carries_the_payout_rule() {
        let genesis_hash = bch_genesis::build(BchShareNetwork::Chipnet)
            .unwrap()
            .block_hash();
        let expected_tag = &genesis_hash.to_string()[..GENESIS_TAG_LEN];

        assert_eq!(
            network_prologue(bitcoin::Network::Testnet, Chain::BitcoinCash, genesis_hash),
            format!(
                "/p2pool/test/{expected_tag}/pplns-k3-m100/asert-parent/trade-htlc/payout-holder/payout-hybrid-t3072-f500-r1to3-z8-cov1/cuts-per-share/uncle-once/body-1000000-204800/payee-p2pkh-p2sh20-p2sh32/merkle32/depths-m100/sigchecks3000-1452/no-segwit/weight-work/mm2-roots-asert8-ticks/version-vote/bch-prev-1off-ticks/genesis-last-resort/band-c64/levels-546x2-k1/weak4-640-100-48-256-320-64-4096-64-32-summary/weak-by-hash/without-payouts/headers-resume/1.0.0"
            )
        );
    }

    /// From 2026-11-15 a new share is relayed with the weak shares it
    /// carries listed by hash. A node before it derived the same string
    /// without `/weak-by-hash`: it cannot read the new share message or the
    /// request for carried weak shares, so the two strings differ and the
    /// nodes fail the handshake rather than decode each other's messages as
    /// malformed. Kademlia's string changes with it, as with every segment.
    #[test]
    fn test_bitcoin_cash_protocol_string_carries_the_weak_by_hash_messages() {
        let genesis_hash = bch_genesis::build(BchShareNetwork::Chipnet)
            .unwrap()
            .block_hash();
        let current = network_prologue(bitcoin::Network::Testnet, Chain::BitcoinCash, genesis_hash);
        let before = rules_only(current.clone());

        assert_ne!(current, before, "weak shares by hash change the messages");
        assert!(
            current.ends_with(
                "/weak4-640-100-48-256-320-64-4096-64-32-summary/weak-by-hash/without-payouts/headers-resume/1.0.0"
            ),
            "{current}"
        );
        assert!(
            before.ends_with("/weak4-640-100-48-256-320-64-4096-64-32-summary/1.0.0"),
            "{before}"
        );
        let kad = kad_protocol_string(bitcoin::Network::Testnet, Chain::BitcoinCash, genesis_hash);
        assert!(
            kad.ends_with("/weak-by-hash/without-payouts/headers-resume/kad/1.0.0"),
            "{kad}"
        );
    }

    /// From 2026-11-16 a new share with large coinbase outputs is relayed
    /// without them (`ShareWithoutPayouts`). A node on `weak-by-hash` alone
    /// derived the string without `/without-payouts`: it cannot read the new
    /// share message, so the two fail the handshake rather than drop large
    /// shares as malformed.
    #[test]
    fn test_bitcoin_cash_protocol_string_carries_the_without_payouts_message() {
        let genesis_hash = bch_genesis::build(BchShareNetwork::Chipnet)
            .unwrap()
            .block_hash();
        let current = network_prologue(bitcoin::Network::Testnet, Chain::BitcoinCash, genesis_hash);
        let weak_by_hash_only = current
            .replace("/headers-resume/", "/")
            .replace("/without-payouts/", "/");

        assert_ne!(current, weak_by_hash_only);
        assert!(
            weak_by_hash_only.ends_with("/weak-by-hash/1.0.0"),
            "{weak_by_hash_only}"
        );
        let kad = kad_protocol_string(bitcoin::Network::Testnet, Chain::BitcoinCash, genesis_hash);
        assert_ne!(kad, kad.replace("/without-payouts/", "/"));
    }

    /// From 2026-11-18 a header request carries the last header the
    /// requester got and the answer serves every share at a height
    /// (request-response 1.1.0). A node on `without-payouts` derived the
    /// string without `/headers-resume` and would peer on 1.0.0, where heights
    /// are cut, so the two strings differ and the nodes fail the handshake
    /// instead. Kademlia's string changes with it.
    #[test]
    fn test_bitcoin_cash_protocol_string_carries_the_headers_resume_messages() {
        let genesis_hash = bch_genesis::build(BchShareNetwork::Chipnet)
            .unwrap()
            .block_hash();
        let current = network_prologue(bitcoin::Network::Testnet, Chain::BitcoinCash, genesis_hash);
        let before = current.replace("/headers-resume/", "/");

        assert_ne!(current, before);
        assert!(
            current.ends_with("/without-payouts/headers-resume/1.0.0"),
            "{current}"
        );
        assert!(
            before.ends_with("/weak-by-hash/without-payouts/1.0.0"),
            "{before}"
        );
        let kad = kad_protocol_string(bitcoin::Network::Testnet, Chain::BitcoinCash, genesis_hash);
        assert_ne!(kad, kad.replace("/headers-resume/", "/"));
    }

    /// On Bitcoin Cash both request-response versions carry the prologue's
    /// rule and message tags: 1.0.0 is the prologue itself and 1.1.0 differs
    /// only in its last segment.
    #[test]
    fn test_bitcoin_cash_request_response_protocols_carry_the_share_chain_segment() {
        let genesis_hash = bch_genesis::build(BchShareNetwork::Chipnet)
            .unwrap()
            .block_hash();
        let prologue =
            network_prologue(bitcoin::Network::Testnet, Chain::BitcoinCash, genesis_hash);
        let share_chain_part = prologue.strip_suffix("/1.0.0").unwrap();

        assert_eq!(
            request_response_protocol_string(
                bitcoin::Network::Testnet,
                Chain::BitcoinCash,
                genesis_hash,
                ProtocolVersion::V1_0
            ),
            prologue
        );
        assert_eq!(
            request_response_protocol_string(
                bitcoin::Network::Testnet,
                Chain::BitcoinCash,
                genesis_hash,
                ProtocolVersion::V1_1
            ),
            format!("{share_chain_part}/1.1.0")
        );
    }

    /// Bitcoin shares carry no weak shares, so a Bitcoin node's messages did
    /// not change and its protocol strings stay upstream's.
    #[test]
    fn test_bitcoin_protocol_string_has_no_message_tag() {
        let genesis_hash = ShareBlock::build_genesis_for_network(bitcoin::Network::Signet)
            .unwrap()
            .block_hash();

        assert!(
            !network_prologue(bitcoin::Network::Signet, Chain::Bitcoin, genesis_hash)
                .contains(BCH_MESSAGES_TAG)
        );
        assert!(
            !kad_protocol_string(bitcoin::Network::Signet, Chain::Bitcoin, genesis_hash)
                .contains(BCH_MESSAGES_TAG)
        );
    }

    /// A chipnet node from before the parent anchor derived
    /// `/p2pool/test/<genesis tag>/pplns-k3-m100/1.0.0` from the same
    /// genesis. It computes other ASERT targets, so the new string differs
    /// and the two cannot negotiate.
    #[test]
    fn test_bitcoin_cash_protocol_string_differs_from_the_genesis_anchor_string() {
        let genesis_hash = bch_genesis::build(BchShareNetwork::Chipnet)
            .unwrap()
            .block_hash();
        let expected_tag = &genesis_hash.to_string()[..GENESIS_TAG_LEN];
        let genesis_anchor_string = format!("/p2pool/test/{expected_tag}/pplns-k3-m100/1.0.0");

        assert_ne!(
            network_prologue(bitcoin::Network::Testnet, Chain::BitcoinCash, genesis_hash),
            genesis_anchor_string
        );
    }

    /// A chipnet node from before the share-trading rules derived
    /// `/p2pool/test/<genesis tag>/pplns-k3-m100/asert-parent/1.0.0`. It
    /// rejects contract spends and accepts dust outputs, so the new string
    /// differs and the two cannot negotiate.
    #[test]
    fn test_bitcoin_cash_protocol_string_differs_from_the_string_before_share_trading() {
        let genesis_hash = bch_genesis::build(BchShareNetwork::Chipnet)
            .unwrap()
            .block_hash();
        let expected_tag = &genesis_hash.to_string()[..GENESIS_TAG_LEN];
        let before_trading_string =
            format!("/p2pool/test/{expected_tag}/pplns-k3-m100/asert-parent/1.0.0");

        assert_ne!(
            network_prologue(bitcoin::Network::Testnet, Chain::BitcoinCash, genesis_hash),
            before_trading_string
        );
    }

    /// A chipnet node from before the coinbase budget derived
    /// `/p2pool/test/<genesis tag>/pplns-k3-m100/asert-parent/trade-htlc/payout-holder/1.0.0`.
    /// It pays every payee, so it builds another coinbase for any window
    /// past the cap; the new string differs and the two cannot negotiate.
    #[test]
    fn test_bitcoin_cash_protocol_string_differs_from_the_string_before_the_coinbase_budget() {
        let genesis_hash = bch_genesis::build(BchShareNetwork::Chipnet)
            .unwrap()
            .block_hash();
        let expected_tag = &genesis_hash.to_string()[..GENESIS_TAG_LEN];
        let before_budget_string = format!(
            "/p2pool/test/{expected_tag}/pplns-k3-m100/asert-parent/trade-htlc/payout-holder/1.0.0"
        );

        assert_ne!(
            network_prologue(bitcoin::Network::Testnet, Chain::BitcoinCash, genesis_hash),
            before_budget_string
        );
    }

    /// A chipnet node from before mining on the current chain derived the
    /// string ending `.../version-vote/1.0.0`. It cannot read share messages
    /// carrying Bitcoin Cash headers, so the new string differs and the two
    /// cannot negotiate.
    #[test]
    fn test_bitcoin_cash_protocol_string_differs_from_the_string_before_the_current_chain_rule() {
        let genesis_hash = bch_genesis::build(BchShareNetwork::Chipnet)
            .unwrap()
            .block_hash();
        let current = rules_only(network_prologue(
            bitcoin::Network::Testnet,
            Chain::BitcoinCash,
            genesis_hash,
        ));
        let before = current.replace(
            "/bch-prev-1off-ticks/genesis-last-resort/band-c64/levels-546x2-k1/weak4-640-100-48-256-320-64-4096-64-32-summary/",
            "/",
        );

        assert_ne!(current, before);
        assert!(before.ends_with("/version-vote/1.0.0"), "{before}");
    }

    /// A chipnet node from before the genesis share became the payee of last
    /// resort derived the string ending `.../bch-prev-1off/1.0.0`. It pays
    /// genesis beside mined work while genesis is in the window, so the new
    /// string differs and the two cannot negotiate.
    #[test]
    fn test_bitcoin_cash_protocol_string_differs_from_the_string_before_genesis_last_resort() {
        let genesis_hash = bch_genesis::build(BchShareNetwork::Chipnet)
            .unwrap()
            .block_hash();
        let current = rules_only(network_prologue(
            bitcoin::Network::Testnet,
            Chain::BitcoinCash,
            genesis_hash,
        ));
        let before = current.replace(
            "/genesis-last-resort/band-c64/levels-546x2-k1/weak4-640-100-48-256-320-64-4096-64-32-summary/",
            "/",
        );

        assert_ne!(current, before);
        assert!(before.ends_with("/bch-prev-1off-ticks/1.0.0"), "{before}");
    }

    /// A chipnet node from before the difficulty band derived the string
    /// ending `.../genesis-last-resort/1.0.0`. It cannot read a header with a
    /// band and refuses every heavier share, so the new string differs and
    /// the two cannot negotiate.
    #[test]
    fn test_bitcoin_cash_protocol_string_differs_from_the_string_before_the_band() {
        let genesis_hash = bch_genesis::build(BchShareNetwork::Chipnet)
            .unwrap()
            .block_hash();
        let current = rules_only(network_prologue(
            bitcoin::Network::Testnet,
            Chain::BitcoinCash,
            genesis_hash,
        ));
        let before = current.replace(
            "/band-c64/levels-546x2-k1/weak4-640-100-48-256-320-64-4096-64-32-summary/",
            "/",
        );

        assert_ne!(current, before);
        assert!(before.ends_with("/genesis-last-resort/1.0.0"), "{before}");
    }

    /// A chipnet node from before payout levels derived the string ending
    /// `.../band-c64/1.0.0`. It cannot read a header with a payout level and
    /// pays every payee its exact slice, so the new string differs and the
    /// two cannot negotiate.
    #[test]
    fn test_bitcoin_cash_protocol_string_differs_from_the_string_before_payout_levels() {
        let genesis_hash = bch_genesis::build(BchShareNetwork::Chipnet)
            .unwrap()
            .block_hash();
        let current = rules_only(network_prologue(
            bitcoin::Network::Testnet,
            Chain::BitcoinCash,
            genesis_hash,
        ));
        let before = current.replace(
            "/levels-546x2-k1/weak4-640-100-48-256-320-64-4096-64-32-summary/",
            "/",
        );

        assert_ne!(current, before);
        assert!(before.ends_with("/band-c64/1.0.0"), "{before}");
    }

    /// A chipnet node from before weak shares derived the string ending
    /// `.../levels-546x2/1.0.0`. It cannot read a header with a weak ratio and
    /// carried weak shares, nor the weak share message, and credits shares at
    /// their own work, so the new string differs and the two cannot negotiate.
    #[test]
    fn test_bitcoin_cash_protocol_string_differs_from_the_string_before_weak_shares() {
        let genesis_hash = bch_genesis::build(BchShareNetwork::Chipnet)
            .unwrap()
            .block_hash();
        let current = rules_only(network_prologue(
            bitcoin::Network::Testnet,
            Chain::BitcoinCash,
            genesis_hash,
        ));
        let before = current.replace(
            "/levels-546x2-k1/weak4-640-100-48-256-320-64-4096-64-32-summary/",
            "/levels-546x2/",
        );

        assert_ne!(current, before);
        assert!(before.ends_with("/levels-546x2/1.0.0"), "{before}");
    }

    /// A chipnet node from before the weak root bound whole weak shares
    /// derived the string ending `.../levels-546x2/weak-64-500-6/1.0.0`. Its
    /// weak root covers only the carried weak shares' proof-of-work hashes,
    /// so it refuses every share carrying weak shares under the new root:
    /// the new string differs and the two cannot negotiate.
    #[test]
    fn test_bitcoin_cash_protocol_string_differs_from_the_string_before_the_whole_weak_share_root()
    {
        let genesis_hash = bch_genesis::build(BchShareNetwork::Chipnet)
            .unwrap()
            .block_hash();
        let current = rules_only(network_prologue(
            bitcoin::Network::Testnet,
            Chain::BitcoinCash,
            genesis_hash,
        ));
        let before = current.replace(
            "/levels-546x2-k1/weak4-640-100-48-256-320-64-4096-64-32-summary/",
            "/levels-546x2/weak-64-500-6/",
        );

        assert_ne!(current, before);
        assert!(
            before.ends_with("/levels-546x2/weak-64-500-6/1.0.0"),
            "{before}"
        );
    }

    /// A chipnet node from before study #85's constants derived the string
    /// ending `.../levels-546x2/weak2-64-500-6/1.0.0`. It takes a 5% includer
    /// cut, lets weak shares wait 6 shares and starts from a genesis whose
    /// weak ratio is 4096, so it credits weak shares differently and the two
    /// cannot negotiate.
    #[test]
    fn test_bitcoin_cash_protocol_string_differs_from_the_string_before_the_study_constants() {
        let genesis_hash = bch_genesis::build(BchShareNetwork::Chipnet)
            .unwrap()
            .block_hash();
        let current = rules_only(network_prologue(
            bitcoin::Network::Testnet,
            Chain::BitcoinCash,
            genesis_hash,
        ));
        let before = current.replace(
            "/levels-546x2-k1/weak4-640-100-48-256-320-64-4096-64-32-summary/",
            "/levels-546x2/weak2-64-500-6/",
        );

        assert_ne!(current, before);
        assert!(
            before.ends_with("/levels-546x2/weak2-64-500-6/1.0.0"),
            "{before}"
        );
    }

    /// From 2026-11-15 each known Bitcoin Cash header carries its ticks and
    /// next interval from its own branch, and token shares' ASERT band
    /// follows them. A node on `bch-prev-1off` and `mm2-roots-asert8` learned
    /// one-minute blocks while running and could require other nBits, so the
    /// new string differs and the two cannot negotiate.
    #[test]
    fn test_bitcoin_cash_protocol_string_carries_the_tick_rules() {
        let genesis_hash = bch_genesis::build(BchShareNetwork::Chipnet)
            .unwrap()
            .block_hash();
        let current = network_prologue(bitcoin::Network::Testnet, Chain::BitcoinCash, genesis_hash);
        let before = current
            .replace("/bch-prev-1off-ticks/", "/bch-prev-1off/")
            .replace("/mm2-roots-asert8-ticks/", "/mm2-roots-asert8/");

        assert_ne!(current, before, "tick rules flag day");
        assert!(
            current.contains("/mm2-roots-asert8-ticks/version-vote/bch-prev-1off-ticks/"),
            "{current}"
        );
    }

    /// From 2026-11-12 a carried weak share is held to its carrier's time. A
    /// node on `weak2` judges it by its own clock, so the new string differs
    /// and the two cannot negotiate.
    #[test]
    fn test_bitcoin_cash_protocol_string_carries_the_carried_weak_time_rule() {
        let genesis_hash = bch_genesis::build(BchShareNetwork::Chipnet)
            .unwrap()
            .block_hash();
        let current = rules_only(network_prologue(
            bitcoin::Network::Testnet,
            Chain::BitcoinCash,
            genesis_hash,
        ));
        let before = current.replace(
            "/weak4-640-100-48-256-320-64-4096-64-32-summary/",
            "/weak2-64-100-48-256/",
        );

        assert_ne!(current, before, "carried weak time flag day");
        assert!(
            current
                .ends_with("/levels-546x2-k1/weak4-640-100-48-256-320-64-4096-64-32-summary/1.0.0"),
            "{current}"
        );
    }

    /// A chipnet node from before the weak ratio followed the asks derived
    /// the string ending `.../levels-546x2-k1/weak3-64-100-48-256/1.0.0`. It
    /// cannot read a header that counts its asking weak shares, refuses a
    /// share wanting a target easier than its own and steps the weak ratio
    /// without the asks, so the new string differs and the two cannot
    /// negotiate.
    #[test]
    fn test_bitcoin_cash_protocol_string_differs_from_the_string_before_the_asks() {
        let genesis_hash = bch_genesis::build(BchShareNetwork::Chipnet)
            .unwrap()
            .block_hash();
        let current = rules_only(network_prologue(
            bitcoin::Network::Testnet,
            Chain::BitcoinCash,
            genesis_hash,
        ));
        let before = current.replace(
            "/weak4-640-100-48-256-320-64-4096-64-32-summary/",
            "/weak3-64-100-48-256/",
        );

        assert_ne!(current, before, "weak asks flag day");
        assert!(
            before.ends_with("/levels-546x2-k1/weak3-64-100-48-256/1.0.0"),
            "{before}"
        );
    }

    /// A node from before the weak root committed the weak shares' summary
    /// derived the string ending `.../weak4-640-100-48-256-320-64-4096-64-32/1.0.0`.
    /// It computes another root for every share carrying weak shares and
    /// cannot read a body served with its weak shares pruned, so the new
    /// string differs and the two cannot negotiate.
    #[test]
    fn test_bitcoin_cash_protocol_string_differs_from_the_string_before_the_weak_summary() {
        let genesis_hash = bch_genesis::build(BchShareNetwork::Chipnet)
            .unwrap()
            .block_hash();
        let current = rules_only(network_prologue(
            bitcoin::Network::Testnet,
            Chain::BitcoinCash,
            genesis_hash,
        ));
        let before = current.replace(
            "/weak4-640-100-48-256-320-64-4096-64-32-summary/",
            "/weak4-640-100-48-256-320-64-4096-64-32/",
        );

        assert_ne!(current, before, "weak summary flag day");
        assert!(
            before.ends_with("/levels-546x2-k1/weak4-640-100-48-256-320-64-4096-64-32/1.0.0"),
            "{before}"
        );
    }

    /// From 2026-11-12 a Bitcoin Cash payout level starts at the lowest. A
    /// node on `levels-546x2` still accepts level 0, so the new string differs
    /// and the two cannot negotiate.
    #[test]
    fn test_bitcoin_cash_protocol_string_carries_the_level_floor() {
        let genesis_hash = bch_genesis::build(BchShareNetwork::Chipnet)
            .unwrap()
            .block_hash();
        let current = rules_only(network_prologue(
            bitcoin::Network::Testnet,
            Chain::BitcoinCash,
            genesis_hash,
        ));
        let before = current.replace("/levels-546x2-k1/", "/levels-546x2/");

        assert_ne!(current, before, "level floor flag day");
        assert!(
            current.contains("levels-546x2-k1"),
            "protocol string holds levels-546x2-k1: {current}"
        );
        assert!(
            before.ends_with("/levels-546x2/weak4-640-100-48-256-320-64-4096-64-32-summary/1.0.0"),
            "{before}"
        );
    }

    /// A chipnet node from before the one-nephew uncle rule derived
    /// `.../trade-htlc/payout-holder/payout-hybrid-t3072-f500-r1to3-z8-cov1/1.0.0`. It accepts a share
    /// naming an uncle an ancestor already names, so the new string differs
    /// and the two cannot negotiate.
    #[test]
    fn test_bitcoin_cash_protocol_string_differs_from_the_string_before_uncle_once() {
        let genesis_hash = bch_genesis::build(BchShareNetwork::Chipnet)
            .unwrap()
            .block_hash();
        let expected_tag = &genesis_hash.to_string()[..GENESIS_TAG_LEN];
        let before_uncle_once_string = format!(
            "/p2pool/test/{expected_tag}/pplns-k3-m100/asert-parent/trade-htlc/payout-holder/payout-hybrid-t3072-f500-r1to3-z8-cov1/1.0.0"
        );

        assert_ne!(
            network_prologue(bitcoin::Network::Testnet, Chain::BitcoinCash, genesis_hash),
            before_uncle_once_string
        );
    }

    /// A chipnet node from before the 2026-10-24 rule batch derived
    /// `.../payout-holder/payout-cap11/uncle-once/1.0.0`. It runs none of
    /// the batch's rules, so the new string differs and the two cannot
    /// negotiate; the batch also starts a fresh share chain.
    #[test]
    fn test_bitcoin_cash_protocol_string_differs_from_the_string_before_the_rule_batch() {
        let genesis_hash = bch_genesis::build(BchShareNetwork::Chipnet)
            .unwrap()
            .block_hash();
        let expected_tag = &genesis_hash.to_string()[..GENESIS_TAG_LEN];
        let before_batch_string = format!(
            "/p2pool/test/{expected_tag}/pplns-k3-m100/asert-parent/trade-htlc/payout-holder/payout-cap11/uncle-once/1.0.0"
        );
        let string = network_prologue(bitcoin::Network::Testnet, Chain::BitcoinCash, genesis_hash);
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
        let genesis_hash = bch_genesis::build(BchShareNetwork::Chipnet)
            .unwrap()
            .block_hash();
        let expected_tag = &genesis_hash.to_string()[..GENESIS_TAG_LEN];
        let before_fixes_string = format!(
            "/p2pool/test/{expected_tag}/pplns-k3-m100/asert-parent/trade-htlc/payout-holder/payout-hybrid-t3072-f500-r1to3-z8-cov1/uncle-once/body-1000000-204800/payee-p2pkh-p2sh20-p2sh32/merkle32/depths-m100/sigchecks3000-1452/no-segwit/weight-work/mm2-roots-asert/version-vote/1.0.0"
        );
        let string = network_prologue(bitcoin::Network::Testnet, Chain::BitcoinCash, genesis_hash);
        assert_ne!(string, before_fixes_string);
        assert!(string.contains("/cuts-per-share/"));
        assert!(string.contains("/mm2-roots-asert8-ticks/"));
    }

    /// The isolation the window rule relies on: a chipnet node from before
    /// the rule derived `/p2pool/test/<genesis tag>/1.0.0` from the same
    /// genesis. The new string differs, so the two cannot negotiate and never
    /// exchange shares the other would reject.
    #[test]
    fn test_bitcoin_cash_protocol_string_differs_from_the_fixed_window_string() {
        let genesis_hash = bch_genesis::build(BchShareNetwork::Chipnet)
            .unwrap()
            .block_hash();
        let expected_tag = &genesis_hash.to_string()[..GENESIS_TAG_LEN];
        let fixed_window_string = format!("/p2pool/test/{expected_tag}/1.0.0");

        assert_ne!(
            network_prologue(bitcoin::Network::Testnet, Chain::BitcoinCash, genesis_hash),
            fixed_window_string
        );
    }

    /// Kademlia carries the same payout rule, so a fixed-window node cannot
    /// join a sized-window node's DHT either.
    #[test]
    fn test_bitcoin_cash_kad_protocol_string_carries_the_payout_rule() {
        let genesis_hash = bch_genesis::build(BchShareNetwork::Chipnet)
            .unwrap()
            .block_hash();
        let expected_tag = &genesis_hash.to_string()[..GENESIS_TAG_LEN];

        assert_eq!(
            kad_protocol_string(bitcoin::Network::Testnet, Chain::BitcoinCash, genesis_hash),
            format!(
                "/p2pool/test/{expected_tag}/pplns-k3-m100/asert-parent/trade-htlc/payout-holder/payout-hybrid-t3072-f500-r1to3-z8-cov1/cuts-per-share/uncle-once/body-1000000-204800/payee-p2pkh-p2sh20-p2sh32/merkle32/depths-m100/sigchecks3000-1452/no-segwit/weight-work/mm2-roots-asert8-ticks/version-vote/bch-prev-1off-ticks/genesis-last-resort/band-c64/levels-546x2-k1/weak4-640-100-48-256-320-64-4096-64-32-summary/weak-by-hash/without-payouts/headers-resume/kad/1.0.0"
            )
        );
    }

    /// A v0.18 node offers exactly this string for request-response, so the
    /// 1.0.0 protocol must keep it for the two to negotiate a stream.
    #[test]
    fn test_v1_0_request_response_protocol_matches_legacy_string() {
        let genesis_hash = ShareBlock::build_genesis_for_network(bitcoin::Network::Signet)
            .unwrap()
            .block_hash();
        let expected_tag = &genesis_hash.to_string()[..GENESIS_TAG_LEN];

        assert_eq!(
            request_response_protocol_string(
                bitcoin::Network::Signet,
                Chain::Bitcoin,
                genesis_hash,
                ProtocolVersion::V1_0
            ),
            format!("/p2pool/signet/{expected_tag}/1.0.0")
        );
    }

    #[test]
    fn test_v1_1_request_response_protocol_carries_version() {
        let genesis_hash = ShareBlock::build_genesis_for_network(bitcoin::Network::Signet)
            .unwrap()
            .block_hash();
        let expected_tag = &genesis_hash.to_string()[..GENESIS_TAG_LEN];

        assert_eq!(
            request_response_protocol_string(
                bitcoin::Network::Signet,
                Chain::Bitcoin,
                genesis_hash,
                ProtocolVersion::V1_1
            ),
            format!("/p2pool/signet/{expected_tag}/1.1.0")
        );
    }

    /// On a 1.0.0 stream the codec writes the legacy request layout, so the
    /// resume point is not carried, and the reading side records the 1.0.0
    /// stream on the decoded request.
    #[tokio::test]
    async fn test_codec_v1_0_stream_drops_resume_point_and_records_version() {
        let genesis_hash = ShareBlock::build_genesis_for_network(bitcoin::Network::Signet)
            .unwrap()
            .block_hash();
        let protocol = P2PoolRequestResponseProtocol::new(
            bitcoin::Network::Signet,
            Chain::Bitcoin,
            genesis_hash,
            ProtocolVersion::V1_0,
        );
        let locator_hash =
            BlockHash::from_str("0000000086704a35f17580d06f76d4c02d2b1f68774800675fb45f0411205bb5")
                .unwrap();
        let request = GetShareHeaders::new(
            vec![locator_hash],
            BlockHash::all_zeros(),
            Some(
                BlockHash::from_str(
                    "a3fbd2a0e62c42446e4e6d18dab3a479ebc84c279757450e019eafc89dfc1a62",
                )
                .unwrap(),
            ),
        );
        let mut codec = ConsensusCodec;
        let mut stream = Cursor::new(Vec::new());

        codec
            .write_request(&protocol, &mut stream, Message::GetShareHeaders(request))
            .await
            .unwrap();
        stream.set_position(0);
        let decoded = codec.read_request(&protocol, &mut stream).await.unwrap();

        assert_eq!(
            decoded,
            Message::GetShareHeaders(GetShareHeaders {
                locator: vec![locator_hash],
                stop: BlockHash::all_zeros(),
                resume_after: None,
                protocol_version: ProtocolVersion::V1_0,
            })
        );
    }

    /// On a 1.1.0 stream the resume point survives the round trip.
    #[tokio::test]
    async fn test_codec_v1_1_stream_carries_resume_point() {
        let genesis_hash = ShareBlock::build_genesis_for_network(bitcoin::Network::Signet)
            .unwrap()
            .block_hash();
        let protocol = P2PoolRequestResponseProtocol::new(
            bitcoin::Network::Signet,
            Chain::Bitcoin,
            genesis_hash,
            ProtocolVersion::V1_1,
        );
        let locator_hash =
            BlockHash::from_str("0000000086704a35f17580d06f76d4c02d2b1f68774800675fb45f0411205bb5")
                .unwrap();
        let request = GetShareHeaders::new(
            vec![locator_hash],
            BlockHash::all_zeros(),
            Some(
                BlockHash::from_str(
                    "a3fbd2a0e62c42446e4e6d18dab3a479ebc84c279757450e019eafc89dfc1a62",
                )
                .unwrap(),
            ),
        );
        let message = Message::GetShareHeaders(request);
        let mut codec = ConsensusCodec;
        let mut stream = Cursor::new(Vec::new());

        codec
            .write_request(&protocol, &mut stream, message.clone())
            .await
            .unwrap();
        stream.set_position(0);
        let decoded = codec.read_request(&protocol, &mut stream).await.unwrap();

        assert_eq!(decoded, message);
    }
}
