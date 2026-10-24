// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Traffic a P2Pool node carries besides the share chain, estimated from
//! formulas rather than simulated: BCHN's own relay, the block templates
//! the node fetches from BCHN, and the Stratum jobs it sends its miners.
//!
//! These are averages. They show how each load grows with block size and
//! coinbase size; they are not measurements of BCHN or of a miner.

use crate::constants::{
    BCH_BLOCK_INTERVAL_SECONDS, COMPACT_BLOCK_FIXED_BYTES, COMPACT_BLOCK_SHORT_ID_BYTES,
    EXTRANONCE_BYTES, INVENTORY_ENTRY_BYTES, TEMPLATE_JSON_BYTES_PER_TRANSACTION,
    TEMPLATE_POLL_SECONDS,
};
use crate::sizes::block_transaction_count;
use serde::Serialize;

/// IPv4 and TCP headers per full segment, as in `sizes`.
const PACKET_HEADER_BYTES: u64 = 40;
const SEGMENT_PAYLOAD_BYTES: u64 = 1_460;

/// Fixed JSON of a `getblocktemplate` reply beside its transactions:
/// previous hash, target, bits, height, version, coinbase value and the
/// rest. An estimate.
const TEMPLATE_FIXED_JSON_BYTES: u64 = 1_000;

/// Fixed JSON of a Stratum V1 `mining.notify` beside the coinbase parts and
/// the branch: method, job id, previous hash, version, bits, time and the
/// clean-jobs flag. An estimate.
const STRATUM_V1_FIXED_BYTES: u64 = 200;

/// Bytes per merkle branch hash in a Stratum V1 job: 64 hex digits, two
/// quotes and a comma.
const STRATUM_V1_BYTES_PER_BRANCH_HASH: u64 = 67;

/// Fixed bytes of a Stratum V2 `NewExtendedMiningJob` beside the coinbase
/// parts and the path, with the 6-byte frame header and the 16-byte Noise
/// tag. An estimate.
const STRATUM_V2_EXTENDED_FIXED_BYTES: u64 = 60;

/// Bytes of a Stratum V2 `NewMiningJob` on a standard (header-only)
/// channel: channel id, job id, optional time, version and merkle root, with
/// the frame header and Noise tag.
const STRATUM_V2_STANDARD_JOB_BYTES: u64 = 70;

/// How miners get their work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum StratumProtocol {
    /// Stratum V1: the coinbase in hex in every job. Most devices today.
    V1,
    /// Stratum V2 extended channel: the coinbase in binary in every job.
    V2Extended,
    /// Stratum V2 standard channel: a merkle root only, no coinbase.
    V2Standard,
}

impl StratumProtocol {
    /// Parse `v1`, `v2-extended` or `v2-standard`.
    pub fn parse(text: &str) -> Result<Self, String> {
        match text.trim() {
            "v1" => Ok(StratumProtocol::V1),
            "v2-extended" => Ok(StratumProtocol::V2Extended),
            "v2-standard" => Ok(StratumProtocol::V2Standard),
            other => Err(format!(
                "expected v1, v2-extended or v2-standard, got `{other}`"
            )),
        }
    }

    /// The command-line name.
    pub fn name(&self) -> &'static str {
        match self {
            StratumProtocol::V1 => "v1",
            StratumProtocol::V2Extended => "v2-extended",
            StratumProtocol::V2Standard => "v2-standard",
        }
    }
}

/// Bytes on the wire for `payload_bytes` sent over TCP/IP.
fn tcp_wire_bytes(payload_bytes: u64) -> u64 {
    let segments = payload_bytes.div_ceil(SEGMENT_PAYLOAD_BYTES).max(1);
    payload_bytes + segments * PACKET_HEADER_BYTES
}

/// Wire bytes of one job for one miner whose coinbase is `coinbase_bytes`
/// with a merkle branch of `branch_hashes`.
pub fn stratum_job_bytes(
    protocol: StratumProtocol,
    coinbase_bytes: u64,
    branch_hashes: u64,
) -> u64 {
    let coinbase_parts = coinbase_bytes.saturating_sub(EXTRANONCE_BYTES);
    let payload = match protocol {
        StratumProtocol::V1 => {
            2 * coinbase_parts
                + STRATUM_V1_BYTES_PER_BRANCH_HASH * branch_hashes
                + STRATUM_V1_FIXED_BYTES
        }
        StratumProtocol::V2Extended => {
            coinbase_parts + 32 * branch_hashes + STRATUM_V2_EXTENDED_FIXED_BYTES
        }
        StratumProtocol::V2Standard => STRATUM_V2_STANDARD_JOB_BYTES,
    };
    tcp_wire_bytes(payload)
}

/// Jobs a miner gets per second: one for each new share-chain tip (the
/// payout changes), one for each template poll and one for each BCH block.
pub fn stratum_jobs_per_second(share_interval_seconds: f64) -> f64 {
    1.0 / share_interval_seconds + 1.0 / TEMPLATE_POLL_SECONDS + 1.0 / BCH_BLOCK_INTERVAL_SECONDS
}

/// What a node's BCHN carries, on average, for blocks of `block_bytes`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct BchnLoad {
    /// Transactions per block.
    pub transactions_per_block: u64,
    /// Bytes per second BCHN downloads, and about as much it uploads, to
    /// relay the network's transactions: each transaction once, an `inv`
    /// entry for it on half its peer links and one `getdata` entry.
    pub relay_bytes_per_second: f64,
    /// Bytes of a compact block carrying a coinbase of the given size.
    pub compact_block_bytes: u64,
    /// Bytes per second a node fetches from BCHN as full
    /// `getblocktemplate` JSON: every poll and every block.
    pub template_bytes_per_second: f64,
}

/// BCHN's load for blocks of `block_bytes` with transactions of
/// `average_transaction_bytes`, `bch_peers` peer links and a coinbase of
/// `coinbase_bytes`.
pub fn bchn_load(
    block_bytes: u64,
    average_transaction_bytes: u64,
    bch_peers: u64,
    coinbase_bytes: u64,
) -> BchnLoad {
    let transactions = block_transaction_count(block_bytes, average_transaction_bytes);
    let transactions_per_second = transactions as f64 / BCH_BLOCK_INTERVAL_SECONDS;
    let bytes_per_transaction = average_transaction_bytes as f64
        + (bch_peers as f64 / 2.0) * INVENTORY_ENTRY_BYTES as f64
        + INVENTORY_ENTRY_BYTES as f64;
    let compact_block_bytes =
        COMPACT_BLOCK_FIXED_BYTES + COMPACT_BLOCK_SHORT_ID_BYTES * transactions + coinbase_bytes;
    let template_bytes = 2 * block_bytes
        + TEMPLATE_JSON_BYTES_PER_TRANSACTION * transactions
        + TEMPLATE_FIXED_JSON_BYTES;
    let templates_per_second = 1.0 / TEMPLATE_POLL_SECONDS + 1.0 / BCH_BLOCK_INTERVAL_SECONDS;
    BchnLoad {
        transactions_per_block: transactions,
        relay_bytes_per_second: transactions_per_second * bytes_per_transaction,
        compact_block_bytes,
        template_bytes_per_second: template_bytes as f64 * templates_per_second,
    }
}

/// Seconds a coinbase of `coinbase_bytes` adds to relaying a block the pool
/// finds over `hops` links of `bytes_per_second`: the coinbase is the one
/// transaction no other node has, so a compact block carries it whole.
pub fn coinbase_relay_seconds(coinbase_bytes: u64, bytes_per_second: f64, hops: u64) -> f64 {
    hops as f64 * coinbase_bytes as f64 / bytes_per_second
}

/// Chance a block is orphaned because it reached the other miners
/// `delay_seconds` later, with BCH blocks every ten minutes.
pub fn orphan_risk(delay_seconds: f64) -> f64 {
    1.0 - (-delay_seconds / BCH_BLOCK_INTERVAL_SECONDS).exp()
}

/// Megabits per second for a rate in bytes per second.
pub fn megabits(bytes_per_second: f64) -> f64 {
    bytes_per_second * 8.0 / 1_000_000.0
}

#[cfg(test)]
mod tests;
