// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The coinbase tag this node writes, and the check of the tag in a job a
//! miner declares (`docs/stratum.md`, "The coinbase tag").
//!
//! The tag is the signature push of the coinbase scriptSig, after the
//! height, the aux flags, the extranonce and the nanosecond time pushes:
//! `P2Poolv2 0x0F <operator> 0x0F <node> 0x0F <chain>`. Every line is
//! written, empty or not, so each keeps its position: explorers that read
//! DATUM tags split the push at 0x0F and show line 1, the operator. The
//! operator and node lines may be empty; the chain line is always there.
//! Later versions may add lines after it, and a reader ignores the lines it
//! does not know.

use crate::config::coinbase_tag::{
    CASHADDR_CHARACTERS, COINBASE_TAG_MARKER, COINBASE_TAG_SEPARATOR, MAX_NODE_LABEL_LENGTH,
    MAX_OPERATOR_LENGTH, check_coinbase_name,
};
use crate::config::{Chain, Parsed, StratumConfig};
use crate::node::behaviour::request_response::network_prologue;
use crate::stratum::session::{EXTRANONCE1_SIZE, EXTRANONCE2_SIZE};
use crate::stratum::work::coinbase::MAX_COINBASE_SCRIPT_SIG_BYTES;
use bitcoin::BlockHash;
use bitcoin::hashes::{Hash, sha256};
use bitcoin::script::{Instruction, Script};
use bitcoindrpc::{BitcoinRpcConfig, BitcoindRpcClient};
use std::fmt;
use std::time::Duration;
use tracing::{info, warn};

/// The largest height push: a height from 8,388,608 up to the 500,000,000
/// that a lock time of the height less one allows is a four-byte script
/// number.
const MAX_HEIGHT_PUSH_BYTES: usize = 1 + 4;

/// The largest aux flags push from a Bitcoin Cash node P2Pool runs on. BCHD
/// sends the flags as the script that pushes `/bchd/`, 7 bytes; BCHN sends
/// empty flags and Knuth none, either pushed as one zero byte.
const MAX_AUX_FLAGS_PUSH_BYTES: usize = 1 + 1 + "/bchd/".len();

/// The extranonce push: enonce1, then enonce2.
const EXTRANONCE_PUSH_BYTES: usize = 1 + EXTRANONCE1_SIZE + EXTRANONCE2_SIZE;

/// The nanosecond time push.
const NSECS_PUSH_BYTES: usize = 1 + 8;

/// The opcode of the tag push: one byte for up to 75 bytes of data.
const TAG_PUSH_OPCODE_BYTES: usize = 1;

/// The most bytes of a coinbase tag: what the 100-byte scriptSig limit of
/// Bitcoin Cash leaves beside the largest of every other push, 64 bytes.
///
/// The scriptSig this node builds holds the height (at most 5 bytes), the
/// aux flags (8), the extranonce (13), the nanosecond time (9) and the
/// opcode of the tag push (1): 36 bytes. Merge mining adds nothing to the
/// scriptSig, as its commitment is output 0. A declared job writes the
/// height and its own tag in a prefix of at most 88 bytes, then the 12-byte
/// extranonce, so a tag within this limit fits there too.
pub const MAX_COINBASE_TAG_BYTES: usize = MAX_COINBASE_SCRIPT_SIG_BYTES
    - MAX_HEIGHT_PUSH_BYTES
    - MAX_AUX_FLAGS_PUSH_BYTES
    - EXTRANONCE_PUSH_BYTES
    - NSECS_PUSH_BYTES
    - TAG_PUSH_OPCODE_BYTES;

/// The characters of the family part of the chain line: 20 bits of the
/// digest of the genesis share hash.
pub const SHARE_CHAIN_FAMILY_LENGTH: usize = 4;

/// The characters of the fork part of the chain line: 10 bits of the
/// digest of the protocol string.
pub const SHARE_CHAIN_FORK_LENGTH: usize = 2;

/// The characters of the chain line, five bits each.
pub const SHARE_CHAIN_ID_LENGTH: usize = SHARE_CHAIN_FAMILY_LENGTH + SHARE_CHAIN_FORK_LENGTH;

/// The separators between the marker, operator, node and chain lines.
const TAG_SEPARATORS: usize = 3;

/// The most characters of the node line: what the tag leaves beside the
/// longest operator line and the chain line, 23. The node software takes
/// what the label (at most `MAX_NODE_LABEL_LENGTH`) and the space before it
/// leave.
pub const MAX_NODE_LINE_LENGTH: usize = MAX_COINBASE_TAG_BYTES
    - COINBASE_TAG_MARKER.len()
    - TAG_SEPARATORS
    - MAX_OPERATOR_LENGTH
    - SHARE_CHAIN_ID_LENGTH;

// A tag push stays one opcode and its data.
const _: () = assert!(MAX_COINBASE_TAG_BYTES <= 75);
// The longest label leaves the node software room on the node line.
const _: () = assert!(MAX_NODE_LABEL_LENGTH + 1 < MAX_NODE_LINE_LENGTH);

/// How long the node has to answer `getnetworkinfo` at start.
const NETWORK_INFO_TIMEOUT: Duration = Duration::from_secs(10);

/// The chain line of the coinbase tag: six CashAddr characters, five bits
/// a character, most significant first, in two parts.
///
/// - The family, 4 characters: the first 20 bits of SHA-256 over the
///   genesis share hash, its 32 bytes in the order the share chain
///   serializes them. Every fork of a share chain keeps it, so explorers
///   group forks by it.
/// - The fork, 2 characters: the first 10 bits of SHA-256 over the P2P
///   protocol string the node connects with and logs at start
///   (`network_prologue`). A share chain that splits off with other rules,
///   or another implementation, gets another.
///
/// Every node on one share chain writes the same line. It is computed,
/// never configured. A rule change that also re-pins the genesis share, as
/// the Bitcoin Cash flag days of this project usually do, changes the
/// family too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShareChainId([u8; SHARE_CHAIN_ID_LENGTH]);

impl ShareChainId {
    /// The ID of the share chain with `genesis_hash` on `network` and
    /// `chain`, from the same protocol string the node connects with.
    pub fn new(network: bitcoin::Network, chain: Chain, genesis_hash: BlockHash) -> Self {
        Self::from_parts(
            genesis_hash,
            &network_prologue(network, chain, genesis_hash),
        )
    }

    /// The ID of the genesis share hash and the protocol string `protocol`.
    fn from_parts(genesis_hash: BlockHash, protocol: &str) -> Self {
        let family = sha256::Hash::hash(genesis_hash.as_byte_array()).to_byte_array();
        let fork = sha256::Hash::hash(protocol.as_bytes()).to_byte_array();
        Self::from_digest_starts([family[0], family[1], family[2]], [fork[0], fork[1]])
    }

    /// The ID of the first 20 bits of `family`, the first three bytes of the
    /// genesis digest, and the first 10 bits of `fork`, the first two bytes
    /// of the protocol digest.
    pub(crate) fn from_digest_starts(family: [u8; 3], fork: [u8; 2]) -> Self {
        let family_bits = (u32::from(family[0]) << 12)
            | (u32::from(family[1]) << 4)
            | (u32::from(family[2]) >> 4);
        let fork_bits = (u32::from(fork[0]) << 2) | (u32::from(fork[1]) >> 6);
        let mut characters = [0u8; SHARE_CHAIN_ID_LENGTH];
        let (family_characters, fork_characters) =
            characters.split_at_mut(SHARE_CHAIN_FAMILY_LENGTH);
        write_cashaddr_characters(family_characters, family_bits);
        write_cashaddr_characters(fork_characters, fork_bits);
        Self(characters)
    }

    /// The chain line, as ASCII bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// The family part, the first four characters, as ASCII bytes.
    pub fn family(&self) -> &[u8] {
        &self.0[..SHARE_CHAIN_FAMILY_LENGTH]
    }
}

/// Write the low `5 * characters.len()` bits of `bits` into `characters`,
/// five bits a CashAddr character, most significant first.
fn write_cashaddr_characters(characters: &mut [u8], bits: u32) {
    let count = characters.len();
    for (position, character) in characters.iter_mut().enumerate() {
        let shift = 5 * (count - 1 - position);
        *character = CASHADDR_CHARACTERS[((bits >> shift) & 0x1f) as usize];
    }
}

impl fmt::Display for ShareChainId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&String::from_utf8_lossy(&self.0))
    }
}

/// The coinbase tag of this node: the signature push of every coinbase it
/// builds, and the share chain its chain line names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeCoinbaseTag {
    bytes: Vec<u8>,
    share_chain_id: ShareChainId,
}

impl NodeCoinbaseTag {
    /// The tag with `operator` and `node_line`, which passed the name
    /// rules, and the chain line of `share_chain_id`. Every line is written,
    /// empty or not.
    pub fn new(operator: &str, node_line: &str, share_chain_id: ShareChainId) -> Self {
        let mut bytes = Vec::with_capacity(
            COINBASE_TAG_MARKER.len()
                + TAG_SEPARATORS
                + operator.len()
                + node_line.len()
                + SHARE_CHAIN_ID_LENGTH,
        );
        bytes.extend_from_slice(COINBASE_TAG_MARKER.as_bytes());
        bytes.push(COINBASE_TAG_SEPARATOR);
        bytes.extend_from_slice(operator.as_bytes());
        bytes.push(COINBASE_TAG_SEPARATOR);
        bytes.extend_from_slice(node_line.as_bytes());
        bytes.push(COINBASE_TAG_SEPARATOR);
        bytes.extend_from_slice(share_chain_id.as_bytes());
        Self {
            bytes,
            share_chain_id,
        }
    }

    /// The bytes of the signature push.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The share chain the chain line names.
    pub fn share_chain_id(&self) -> ShareChainId {
        self.share_chain_id
    }
}

/// The lines joined by ` | `, for the log.
impl fmt::Display for NodeCoinbaseTag {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = String::from_utf8_lossy(&self.bytes);
        formatter.write_str(&text.replace(char::from(COINBASE_TAG_SEPARATOR), " | "))
    }
}

/// The node line: the node software, then the label after a space. Either
/// may be empty.
pub fn node_line(software: &str, label: &str) -> String {
    match (software.is_empty(), label.is_empty()) {
        (false, false) => format!("{software} {label}"),
        (true, _) => label.to_string(),
        (false, true) => software.to_string(),
    }
}

/// The characters the node software may take on the node line beside
/// `label` and the space before it.
fn software_room(label: &str) -> usize {
    if label.is_empty() {
        MAX_NODE_LINE_LENGTH
    } else {
        MAX_NODE_LINE_LENGTH.saturating_sub(label.len() + 1)
    }
}

/// The node software and version for the node line, from the `subversion`
/// of `getnetworkinfo` (BIP 14, `/Name:Version(comments)/`): the first user
/// agent without its comments, named as operators know it. BCHN
/// (`/Bitcoin Cash Node:29.2.1(EB32.0)/`) becomes `BCHN 29.2.1`, Knuth
/// `Knuth 1.7.0` and BCHD `BCHD 0.22.4`; other software keeps its own name.
///
/// The line must pass the name rules within `room` characters; otherwise
/// the reason is returned, and the line is left out.
pub fn node_software_line(subversion: &str, room: usize) -> Result<String, String> {
    //* Take the first user agent, without its slashes and its comments.
    let agent = subversion
        .trim()
        .trim_start_matches('/')
        .split('/')
        .next()
        .unwrap_or_default();
    let agent = agent.split('(').next().unwrap_or_default().trim();
    //* Name the software as operators know it, followed by its version.
    let named = match agent.split_once(':') {
        Some((name, version)) => format!("{} {}", software_name(name.trim()), version.trim()),
        None => software_name(agent).to_string(),
    };
    let line = named.trim();
    if line.is_empty() {
        return Err(format!("the subversion {subversion:?} names no software"));
    }
    check_coinbase_name(line, room).map_err(|reason| format!("the node software {reason}"))?;
    Ok(line.to_string())
}

/// The short name operators know a node software by.
fn software_name(name: &str) -> &str {
    if name.eq_ignore_ascii_case("Bitcoin Cash Node") {
        "BCHN"
    } else if name.eq_ignore_ascii_case("Knuth") {
        "Knuth"
    } else if name.eq_ignore_ascii_case("BCHD") {
        "BCHD"
    } else {
        name
    }
}

/// The `subversion` the node in `bitcoinrpc` reports, or why there is none:
/// a node without `getnetworkinfo`, an answer without a subversion, or no
/// answer within `NETWORK_INFO_TIMEOUT`.
async fn read_subversion(bitcoinrpc: &BitcoinRpcConfig) -> Result<String, String> {
    let client =
        BitcoindRpcClient::new(&bitcoinrpc.url, &bitcoinrpc.username, &bitcoinrpc.password)
            .map_err(|error| format!("no RPC client for the node: {error}"))?;
    match tokio::time::timeout(NETWORK_INFO_TIMEOUT, client.getnetworkinfo()).await {
        Ok(Ok(info)) => info
            .subversion
            .ok_or_else(|| "the node answers getnetworkinfo without a subversion".to_string()),
        Ok(Err(error)) if error.is_method_not_found() => {
            Err("the node has no getnetworkinfo".to_string())
        }
        Ok(Err(error)) => Err(format!("getnetworkinfo failed: {error}")),
        Err(_) => Err(format!(
            "the node did not answer getnetworkinfo within {NETWORK_INFO_TIMEOUT:?}"
        )),
    }
}

/// The coinbase tag of this node, built once at start: the operator line
/// and the label from `stratum`, the node software from `getnetworkinfo` on
/// the node in `bitcoinrpc`, and the chain line `share_chain_id`.
///
/// The node software is not the operator to fix, so it never stops the
/// start: a node without `getnetworkinfo`, or a software line that fails
/// the name rules or does not fit beside the label, leaves the software out
/// of the node line, with a warning.
pub async fn coinbase_tag_for_node(
    stratum: &StratumConfig<Parsed>,
    bitcoinrpc: &BitcoinRpcConfig,
    share_chain_id: ShareChainId,
) -> NodeCoinbaseTag {
    let label = stratum.coinbase_node_label();
    let software = read_subversion(bitcoinrpc)
        .await
        .and_then(|subversion| node_software_line(&subversion, software_room(label)))
        .unwrap_or_else(|reason| {
            warn!("The coinbase tag names no node software: {reason}");
            String::new()
        });
    let tag = NodeCoinbaseTag::new(
        stratum.coinbase_operator(),
        &node_line(&software, label),
        share_chain_id,
    );
    info!("Coinbase tag: {tag}");
    tag
}

/// Check the coinbase tag of a job a miner declares, in `script_prefix`,
/// its scriptSig before the extranonce. A declarator builds the template,
/// so the operator and node lines are its own, but the share lands on this
/// share chain: after the height push, a data push must start with the
/// `P2Poolv2` line, and every such push must be a tag whose chain line is
/// `share_chain_id`.
///
/// The error says why the tag is refused, for the log.
pub(crate) fn check_declared_coinbase_tag(
    script_prefix: &[u8],
    share_chain_id: ShareChainId,
) -> Result<(), String> {
    let mut tags = Script::from_bytes(script_prefix)
        .instructions()
        .skip(1)
        .filter_map(|instruction| match instruction {
            Ok(Instruction::PushBytes(bytes)) => Some(bytes.as_bytes()),
            _ => None,
        })
        .filter(|bytes| starts_with_marker_line(bytes))
        .peekable();
    if tags.peek().is_none() {
        return Err(format!(
            "its scriptSig prefix has no push that starts with the {COINBASE_TAG_MARKER} line"
        ));
    }
    tags.try_for_each(|tag| check_coinbase_tag(tag, share_chain_id))
}

/// Whether `bytes` start with the `P2Poolv2` line and its separator.
fn starts_with_marker_line(bytes: &[u8]) -> bool {
    bytes
        .strip_prefix(COINBASE_TAG_MARKER.as_bytes())
        .and_then(|rest| rest.first())
        == Some(&COINBASE_TAG_SEPARATOR)
}

/// Check a tag that starts with the `P2Poolv2` line: an operator line and a
/// node line under the name rules, the chain line `share_chain_id`, and any
/// later lines under the name rules, within `MAX_COINBASE_TAG_BYTES`.
fn check_coinbase_tag(tag: &[u8], share_chain_id: ShareChainId) -> Result<(), String> {
    if tag.len() > MAX_COINBASE_TAG_BYTES {
        return Err(format!(
            "its coinbase tag is {} bytes, more than {MAX_COINBASE_TAG_BYTES}",
            tag.len()
        ));
    }
    let text = std::str::from_utf8(tag).map_err(|_| "its coinbase tag is not text".to_string())?;
    let mut lines = text.split(char::from(COINBASE_TAG_SEPARATOR));
    let (Some(_marker), Some(operator), Some(node), Some(chain)) =
        (lines.next(), lines.next(), lines.next(), lines.next())
    else {
        return Err(
            "its coinbase tag lacks the operator, node or chain line after the P2Poolv2 line"
                .to_string(),
        );
    };
    check_coinbase_name(operator, MAX_OPERATOR_LENGTH)
        .map_err(|reason| format!("its operator line {reason}"))?;
    check_coinbase_name(node, MAX_NODE_LINE_LENGTH)
        .map_err(|reason| format!("its node line {reason}"))?;
    if chain.as_bytes() != share_chain_id.as_bytes() {
        return Err(format!(
            "its chain line {chain:?} names another share chain than this one, {share_chain_id}"
        ));
    }
    lines.try_for_each(|line| {
        check_coinbase_name(line, MAX_OPERATOR_LENGTH)
            .map_err(|reason| format!("a later line of its coinbase tag {reason}"))
    })
}

#[cfg(test)]
mod tests;
