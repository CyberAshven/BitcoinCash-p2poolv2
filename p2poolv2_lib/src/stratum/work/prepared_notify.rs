// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::block_template::{BlockTemplate, parse_flags};
#[cfg(test)]
use super::coinbase::build_bitcoin_coinbase_transaction;
use super::coinbase::{LOCKTIME_LENGTH, build_coinbase, get_timestamp_bytes, split_coinbase};
use super::error::WorkError;
use super::gbt::build_merkle_branches_for_template;
use super::tracker::{JobDetails, JobId, JobTracker};
use crate::accounting::OutputPair;
use crate::accounting::payout::payout_distribution::build_pplns_outputs;
use crate::shares::share_block::{ShareTransaction, encode_optional_address};
use crate::shares::share_commitment::{
    ShareCommitment, build_commitment_prefix, build_commitment_suffix, commitment_digest,
};
use crate::shares::transactions::coinbase::compute_witness_root;
use crate::shares::validation::BLOCK_TXS_SIZE_LIMIT;
use crate::shares::witness_commitment::WitnessCommitment;
use crate::stratum::util::{reverse_four_byte_chunks, to_be_hex};
use crate::utils::time_provider::{SystemTimeProvider, TimeProvider};
use bitcoin::consensus::Encodable;
use bitcoin::hashes::{self, Hash};
use bitcoin::transaction::Version;
use bitcoin::{Address, BlockHash, CompactTarget, WitnessMerkleNode};
use std::collections::HashMap;
use std::sync::Arc;

/// Hex characters per byte.
const HEX_PER_BYTE: usize = 2;

/// Hex of the opcode starting coinbase2: the nsecs push, OP_PUSHBYTES_8.
const NSECS_PUSH_OPCODE_HEX: &str = "08";

/// Hex length of the nsecs push at the front of coinbase2: the
/// OP_PUSHBYTES_8 opcode (1 byte) and the 8-byte little-endian nsecs.
const NSECS_PUSH_HEX_LENGTH: usize = HEX_PER_BYTE * (1 + 8);

/// Hex of the commitment output's script opcodes preceding the hash:
/// OP_RETURN (0x6a) and OP_PUSHBYTES_32 (0x20).
const COMMITMENT_SCRIPT_OPCODES_HEX: &str = "6a20";

/// Hex length of the 32-byte commitment hash in the commitment output.
const COMMITMENT_HASH_HEX_LENGTH: usize = HEX_PER_BYTE * 32;

/// Hex length of the 4-byte locktime ending coinbase2, which follows the
/// commitment hash.
const LOCKTIME_HEX_LENGTH: usize = HEX_PER_BYTE * LOCKTIME_LENGTH;

/// Split coinbase2 around its per-miner parts -- the nsecs push at the front
/// and the commitment hash before the locktime -- into the static middle and
/// the locktime.
///
/// Checks the layout before slicing: the nsecs push opens coinbase2, and the
/// commitment output's opcodes and `dummy_commitment_hex` sit right before the
/// locktime. A coinbase built any other way would place each miner's
/// commitment in the wrong bytes, and every share would then fail validation
/// with nothing pointing back here.
fn split_coinbase2(
    coinbase2: &str,
    dummy_commitment_hex: &str,
) -> Result<(String, String), WorkError> {
    let minimum_length = NSECS_PUSH_HEX_LENGTH
        + COMMITMENT_SCRIPT_OPCODES_HEX.len()
        + COMMITMENT_HASH_HEX_LENGTH
        + LOCKTIME_HEX_LENGTH;
    if coinbase2.len() < minimum_length {
        return Err(WorkError {
            message: format!("coinbase2 of {} hex chars is too short", coinbase2.len()),
        });
    }
    let locktime_start = coinbase2.len() - LOCKTIME_HEX_LENGTH;
    let commitment_start = locktime_start - COMMITMENT_HASH_HEX_LENGTH;
    let opcodes_start = commitment_start - COMMITMENT_SCRIPT_OPCODES_HEX.len();

    let has_expected_layout = coinbase2.starts_with(NSECS_PUSH_OPCODE_HEX)
        && &coinbase2[opcodes_start..commitment_start] == COMMITMENT_SCRIPT_OPCODES_HEX
        && &coinbase2[commitment_start..locktime_start] == dummy_commitment_hex;
    if !has_expected_layout {
        return Err(WorkError {
            message: "coinbase2 does not start with the nsecs push and end with the commitment output and locktime".to_string(),
        });
    }

    Ok((
        coinbase2[NSECS_PUSH_HEX_LENGTH..commitment_start].to_string(),
        coinbase2[locktime_start..].to_string(),
    ))
}

/// Pre-serialized notify message with placeholders for per-miner fields.
///
/// coinbase1 is static (same for all miners): [height][aux_flags][EXTRANONCE_SEPARATOR]
/// coinbase2 is built per-miner:
/// [nsecs][pool_sig][sequence][outputs..][padding output][commitment output][locktime]
/// where the commitment output is `OP_RETURN OP_PUSHBYTES_32 <commitment_hash>`.
/// Only the nsecs push and the 32 commitment bytes differ between miners, and
/// both are fixed size, so the padding that block-aligns the prefix is the same
/// for every miner on a template.
///
/// The JSON template has fixed-size placeholders for job_id and the full coinbase2.
/// The commitment encoding is split into a prefix (fields before time) and a
/// suffix (fields after time), so that per-share hashing inserts a fresh 4-byte
/// timestamp between them without rebuilding the whole commitment.
pub struct PreparedNotifyParams {
    /// Exact difficulty weights at this job's share-chain anchor.
    pplns_weights: Option<HashMap<Address, u128>>,
    /// Pre-serialized JSON notify string with placeholder job_id and coinbase2
    json_template: String,
    /// Byte offset of the 16-char job_id placeholder in json_template
    job_id_offset: usize,
    /// Byte offset of the coinbase2 placeholder in json_template
    coinbase2_offset: usize,
    /// Length of the coinbase2 placeholder in json_template
    coinbase2_placeholder_len: usize,
    /// Pre-serialized commitment fields before time:
    /// prev_share_blockhash + uncles + bits
    commitment_prefix: Vec<u8>,
    /// Pre-serialized commitment fields after time (before miner address):
    /// donation_address + donation + fee_address + fee
    commitment_suffix: Vec<u8>,
    /// Previous share block hash (for building ShareCommitment struct)
    prev_share_blockhash: BlockHash,
    /// Uncle block hashes (for building ShareCommitment struct)
    uncles: Vec<BlockHash>,
    /// Share chain difficulty target (for building ShareCommitment struct)
    bits: CompactTarget,
    /// Donation address for developers
    donation_address: Option<Address>,
    /// Donation in basis points
    donation: Option<u16>,
    /// Fee address for the pool operator
    fee_address: Option<Address>,
    /// Fee in basis points
    fee: Option<u16>,
    /// Shared block template
    template: Arc<BlockTemplate>,
    /// Static coinbase1 hex (identical for all miners)
    coinbase1: String,
    /// Coinbase2 hex between the nsecs push and the commitment hash bytes:
    /// [pool_sig][sequence][outputs..][padding output][commitment output header]
    coinbase2_middle: String,
    /// Coinbase2 hex after the commitment hash bytes: the locktime.
    coinbase2_locktime: String,
    /// Merkle branches for the template transactions (excluding coinbase).
    /// Passed through to JobDetails so validators can verify the bitcoin merkle root.
    merkle_branches: Vec<bitcoin::TxMerkleNode>,
    /// Share transactions every share on these params carries after its
    /// coinbase, shared with each job.
    share_transactions: Arc<[ShareTransaction]>,
    /// BIP141 witness root of `share_transactions`, committed by every
    /// miner's share commitment.
    share_witness_root: WitnessMerkleNode,
}

impl PreparedNotifyParams {
    /// Template whose share-chain payout snapshot this job commits to.
    pub(crate) fn template(&self) -> &Arc<BlockTemplate> {
        &self.template
    }

    /// Versioned BCH job terms, preceded by an unpredictable connection nonce.
    pub(crate) fn mining_job_token(
        &self,
        nonce: [u8; 32],
        time: u32,
    ) -> Result<Vec<u8>, WorkError> {
        let mut token = Vec::with_capacity(255);
        token.push(1);
        token.extend_from_slice(&nonce);
        token.extend_from_slice(&self.commitment_prefix);
        time.consensus_encode(&mut token).expect("vector writer");
        encode_optional_address(&self.donation_address, &mut token).expect("vector writer");
        self.donation
            .unwrap_or(0)
            .consensus_encode(&mut token)
            .expect("vector writer");
        encode_optional_address(&self.fee_address, &mut token).expect("vector writer");
        self.fee
            .unwrap_or(0)
            .consensus_encode(&mut token)
            .expect("vector writer");
        if token.len() > 255 {
            return Err(WorkError {
                message: "BCH job terms exceed the SV2 token budget".into(),
            });
        }
        Ok(token)
    }

    /// Recompute payouts when a declared transaction set changes fee revenue.
    pub(crate) fn declared_outputs(&self, value: u64) -> Result<Vec<OutputPair>, WorkError> {
        let weights = self.pplns_weights.as_ref().ok_or_else(|| WorkError {
            message: "PPLNS snapshot is unavailable for declared work".into(),
        })?;
        build_pplns_outputs(
            value,
            weights,
            &self.donation_address,
            self.donation,
            &self.fee_address,
            self.fee,
        )
        .map_err(|error| WorkError {
            message: error.to_string(),
        })
    }

    /// The ASERT-computed pool target for shares built on these params.
    pub fn bits(&self) -> bitcoin::CompactTarget {
        self.bits
    }

    /// The share transactions shares built on these params carry.
    pub fn share_transactions(&self) -> &[ShareTransaction] {
        &self.share_transactions
    }
}

/// Serialize the merkle branches array as a JSON array string.
fn serialize_merkle_branches_json(branches: &[String]) -> String {
    let mut result = String::with_capacity(branches.len() * 68);
    result.push('[');
    for (index, branch) in branches.iter().enumerate() {
        if index > 0 {
            result.push(',');
        }
        result.push('"');
        result.push_str(branch);
        result.push('"');
    }
    result.push(']');
    result
}

/// Build the pre-serialized JSON notify string by concatenation.
///
/// coinbase1 is static. coinbase2 gets a placeholder that is replaced per-miner.
/// Returns the JSON string, the byte offset of the job_id placeholder,
/// the byte offset of the coinbase2 placeholder, and the placeholder length.
#[allow(clippy::too_many_arguments)] // wiring constructor: each parameter is a distinct collaborator, a params struct would only move the list
fn build_json_template(
    coinbase1: &str,
    coinbase2_placeholder: &str,
    prevhash_byte_swapped: &str,
    merkle_branches: &[String],
    version_hex: &str,
    nbits: &str,
    ntime_hex: &str,
    clean_jobs: bool,
) -> (String, usize, usize, usize) {
    let estimated_capacity =
        256 + coinbase1.len() + coinbase2_placeholder.len() + merkle_branches.len() * 68;
    let mut json = String::with_capacity(estimated_capacity);

    json.push_str(r#"{"method":"mining.notify","params":[""#);
    let job_id_offset = json.len();
    json.push_str("0000000000000000"); // 16-char placeholder for job_id
    json.push_str(r#"",""#);
    json.push_str(prevhash_byte_swapped);
    json.push_str(r#"",""#);
    json.push_str(coinbase1);
    json.push_str(r#"",""#);
    let coinbase2_offset = json.len();
    let coinbase2_placeholder_len = coinbase2_placeholder.len();
    json.push_str(coinbase2_placeholder);
    json.push_str(r#"","#);
    json.push_str(&serialize_merkle_branches_json(merkle_branches));
    json.push_str(r#",""#);
    json.push_str(version_hex);
    json.push_str(r#"",""#);
    json.push_str(nbits);
    json.push_str(r#"",""#);
    json.push_str(ntime_hex);
    json.push_str(r#"","#);
    if clean_jobs {
        json.push_str("true");
    } else {
        json.push_str("false");
    }
    json.push_str("]}");

    (
        json,
        job_id_offset,
        coinbase2_offset,
        coinbase2_placeholder_len,
    )
}

/// Builder for constructing PreparedNotifyParams with pre-computed shared fields.
///
/// By serializing notify params once, we avoid needing to serialize
/// for each stratum client. Instead, we pick up the prepared notify
/// params and use the miner address to build share commitment, thus
/// the coinbase1 for each individual client.
pub(crate) struct PreparedNotifyParamsBuilder {
    pplns_weights: Option<HashMap<Address, u128>>,
    template: Arc<BlockTemplate>,
    output_distribution: Vec<OutputPair>,
    pool_signature: Vec<u8>,
    clean_jobs: bool,
    prev_share_blockhash: BlockHash,
    uncles: Vec<BlockHash>,
    bits: CompactTarget,
    donation_address: Option<Address>,
    donation: Option<u16>,
    fee_address: Option<Address>,
    fee: Option<u16>,
    include_merge_mining: bool,
    share_transactions: Vec<ShareTransaction>,
}

impl PreparedNotifyParamsBuilder {
    /// Create a new builder with required parameters.
    pub fn new(
        template: Arc<BlockTemplate>,
        output_distribution: Vec<OutputPair>,
        pool_signature: &[u8],
        clean_jobs: bool,
    ) -> Self {
        Self {
            pplns_weights: None,
            template,
            output_distribution,
            pool_signature: pool_signature.to_vec(),
            clean_jobs,
            prev_share_blockhash: BlockHash::all_zeros(),
            uncles: Vec::new(),
            bits: CompactTarget::from_consensus(0),
            donation_address: None,
            donation: None,
            fee_address: None,
            fee: None,
            include_merge_mining: false,
            share_transactions: Vec::new(),
        }
    }

    /// Share transactions for every share on these params to carry after its
    /// coinbase, in order, already checked against the mining base (on
    /// Bitcoin Cash, `ShareTransactionPool::select_for_share`). `build` drops
    /// transactions from the end until the share fits `BLOCK_TXS_SIZE_LIMIT`
    /// with its coinbases.
    pub fn share_transactions(mut self, share_transactions: Vec<ShareTransaction>) -> Self {
        self.share_transactions = share_transactions;
        self
    }

    pub fn prev_share_blockhash(mut self, prev_share_blockhash: BlockHash) -> Self {
        self.prev_share_blockhash = prev_share_blockhash;
        self
    }

    pub fn pplns_weights(mut self, weights: Option<HashMap<Address, u128>>) -> Self {
        self.pplns_weights = weights;
        self
    }

    pub fn uncles(mut self, uncles: Vec<BlockHash>) -> Self {
        self.uncles = uncles;
        self
    }

    pub fn bits(mut self, bits: CompactTarget) -> Self {
        self.bits = bits;
        self
    }

    pub fn donation_address(mut self, donation_address: Option<Address>) -> Self {
        self.donation_address = donation_address;
        self
    }

    pub fn donation(mut self, donation: Option<u16>) -> Self {
        self.donation = donation;
        self
    }

    pub fn fee_address(mut self, fee_address: Option<Address>) -> Self {
        self.fee_address = fee_address;
        self
    }

    pub fn fee(mut self, fee: Option<u16>) -> Self {
        self.fee = fee;
        self
    }

    /// Append the reserved AuxPoW output and check final BCH block size.
    /// Only Bitcoin Cash sets this.
    pub fn include_merge_mining(mut self, include_merge_mining: bool) -> Self {
        self.include_merge_mining = include_merge_mining;
        self
    }

    /// Build the PreparedNotifyParams by constructing the coinbase transaction
    /// with a dummy commitment hash, splitting it, and constructing the
    /// pre-serialized JSON template.
    ///
    /// coinbase1 is static (same for all miners). coinbase2 is split around its
    /// two per-miner parts -- the nsecs push at the front and the commitment
    /// hash bytes before the locktime -- into a static middle and locktime.
    pub fn build(self) -> Result<PreparedNotifyParams, WorkError> {
        let coinbaseaux = parse_flags(self.template.coinbaseaux.get("flags").cloned())?;
        let witness_commitment = self
            .template
            .default_witness_commitment
            .as_deref()
            .map(WitnessCommitment::from_hex)
            .transpose()
            .map_err(|error| WorkError {
                message: format!("Invalid witness commitment: {error}"),
            })?;

        // Build coinbase with dummy commitment hash and dummy nsecs.
        // After split_coinbase, coinbase1 is fully static, coinbase2 starts
        // with [nsecs_push][pool_sig_push]... and ends with
        // [commitment_hash][locktime].
        let dummy_commitment_hash = hashes::sha256::Hash::from_byte_array([0xab_u8; 32]);

        let merge_mining = if self.include_merge_mining {
            Some(crate::shares::merge_mining::MergeMiningCommitment::reserved())
        } else {
            None
        };
        let coinbase = build_coinbase(
            Version::TWO,
            self.output_distribution.as_slice(),
            self.template.height as i64,
            coinbaseaux,
            witness_commitment.as_ref(),
            &self.pool_signature,
            Some(dummy_commitment_hash),
            0u64,
            None,
            merge_mining,
        )?;

        if self.include_merge_mining {
            self.template.validate_bch_block_size(&coinbase)?;
        }

        let share_transactions = fit_share_transactions(self.share_transactions, &coinbase);
        let share_witness_root = compute_witness_root(&share_transactions);

        let (coinbase1, coinbase2_full) = split_coinbase(&coinbase)?;

        let (coinbase2_middle, coinbase2_locktime) = split_coinbase2(
            &coinbase2_full,
            &hex::encode(dummy_commitment_hash.as_byte_array()),
        )?;

        // Pre-compute merkle branches
        let merkle_branches_raw = build_merkle_branches_for_template(&self.template)?;
        let merkle_branches_hex: Vec<String> = merkle_branches_raw
            .iter()
            .map(|branch| to_be_hex(&branch.to_string()))
            .collect();

        let prevhash_byte_swapped = reverse_four_byte_chunks(&self.template.previousblockhash)
            .map_err(|error| WorkError {
                message: format!("Failed to reverse previous block hash: {error}"),
            })?;

        let version_hex = hex::encode(self.template.version.to_be_bytes());
        let ntime_hex = hex::encode(self.template.curtime.to_be_bytes());

        // Use a zero-filled placeholder for coinbase2 in the JSON template.
        // It will be replaced per-miner with the actual coinbase2.
        let coinbase2_placeholder = "0".repeat(coinbase2_full.len());

        let (json_template, job_id_offset, coinbase2_offset, coinbase2_placeholder_len) =
            build_json_template(
                &coinbase1,
                &coinbase2_placeholder,
                &prevhash_byte_swapped,
                &merkle_branches_hex,
                &version_hex,
                &self.template.bits,
                &ntime_hex,
                self.clean_jobs,
            );

        let commitment_prefix =
            build_commitment_prefix(self.prev_share_blockhash, &self.uncles, self.bits);
        let commitment_suffix = build_commitment_suffix(
            &self.donation_address,
            self.donation,
            &self.fee_address,
            self.fee,
            share_witness_root,
        );

        Ok(PreparedNotifyParams {
            pplns_weights: self.pplns_weights,
            json_template,
            job_id_offset,
            coinbase2_offset,
            coinbase2_placeholder_len,
            commitment_prefix,
            commitment_suffix,
            prev_share_blockhash: self.prev_share_blockhash,
            uncles: self.uncles,
            bits: self.bits,
            donation_address: self.donation_address,
            donation: self.donation,
            fee_address: self.fee_address,
            fee: self.fee,
            template: self.template,
            coinbase1,
            coinbase2_middle,
            coinbase2_locktime,
            merkle_branches: merkle_branches_raw
                .into_iter()
                .map(bitcoin::TxMerkleNode::from_raw_hash)
                .collect(),
            share_transactions: share_transactions.into(),
            share_witness_root,
        })
    }
}

/// Bytes a miner's bitcoin coinbase may add to the one `build` sizes: the
/// extranonce and the nanosecond timestamp.
const BITCOIN_COINBASE_SIZE_ALLOWANCE: usize = 64;

/// Bytes kept for the share coinbase, which holds the weak block hash and so
/// cannot be built before the share is found.
const SHARE_COINBASE_SIZE_ALLOWANCE: usize = 256;

/// Drop share transactions from the end until they fit
/// `BLOCK_TXS_SIZE_LIMIT` beside the share coinbase and `bitcoin_coinbase`,
/// which a share block also carries.
fn fit_share_transactions(
    mut share_transactions: Vec<ShareTransaction>,
    bitcoin_coinbase: &bitcoin::Transaction,
) -> Vec<ShareTransaction> {
    let budget = (BLOCK_TXS_SIZE_LIMIT as usize).saturating_sub(
        bitcoin_coinbase.total_size()
            + BITCOIN_COINBASE_SIZE_ALLOWANCE
            + SHARE_COINBASE_SIZE_ALLOWANCE,
    );
    let mut total: usize = share_transactions
        .iter()
        .map(|transaction| transaction.total_size())
        .sum();
    while total > budget
        && let Some(dropped) = share_transactions.pop()
    {
        total -= dropped.total_size();
    }
    share_transactions
}

/// Hex of the commitment digest for one miner.
///
/// Uses the prefix and suffix pre-built once per template, so the per-miner
/// cost is a memcpy plus the tail append -- not a re-encode of the shared
/// fields. With thousands of workers these calls are serial, so the last miner
/// to be notified pays the sum of all of them; re-encoding the donation and fee
/// bech32 strings per miner is exactly what this avoids.
fn get_commitment_hex(
    commitment_prefix: &[u8],
    time: u32,
    commitment_suffix: &[u8],
    miner_bitcoin_address: Option<&Address>,
    miner_address: Option<p2poolv2_wallet::share_owner::ShareOwner>,
) -> String {
    let digest = commitment_digest(
        commitment_prefix,
        time,
        commitment_suffix,
        miner_bitcoin_address,
        miner_address,
    );
    hex::encode(digest.as_byte_array())
}

/// Build a per-miner coinbase2 hex from the commitment hash, a fresh
/// timestamp, and the static middle and locktime.
fn build_per_miner_coinbase2(
    commitment_hash_hex: &str,
    nsecs: u64,
    coinbase2_middle: &str,
    coinbase2_locktime: &str,
) -> String {
    // nsecs push: 0x08 opcode + 8 bytes LE
    let nsecs_bytes = nsecs.to_le_bytes();
    let mut coinbase2 = String::with_capacity(
        NSECS_PUSH_HEX_LENGTH
            + coinbase2_middle.len()
            + COMMITMENT_HASH_HEX_LENGTH
            + coinbase2_locktime.len(),
    );
    coinbase2.push_str("08");
    coinbase2.push_str(&hex::encode(nsecs_bytes));
    coinbase2.push_str(coinbase2_middle);
    coinbase2.push_str(commitment_hash_hex);
    coinbase2.push_str(coinbase2_locktime);
    coinbase2
}

/// Insert one miner's job for `prepared` into the tracker.
///
/// Computes the miner-specific commitment hash and coinbase2 and stores the
/// job, returning its id and coinbase2. SV1 wraps this in a notify message;
/// Stratum V2 reads the job back from the tracker to build its own messages.
/// When either address is None (solo or Hydrapool mode), a commitment hash is
/// still computed from whatever is available, but no `ShareCommitment` is
/// built: a share needs both a bitcoin payout identity and a share chain owner.
pub(crate) fn insert_miner_job(
    prepared: &PreparedNotifyParams,
    miner_bitcoin_address: Option<&Address>,
    miner_address: Option<p2poolv2_wallet::share_owner::ShareOwner>,
    tracker_handle: &JobTracker,
) -> (JobId, String) {
    let fresh_time = SystemTimeProvider.seconds_since_epoch() as u32;

    let nsecs = get_timestamp_bytes(&SystemTimeProvider);

    // Build ShareCommitment only when both addresses are available: the share
    // chain coinbase needs an owner and the bitcoin coinbase needs a payee.
    let share_commitment = match (miner_bitcoin_address, miner_address) {
        (Some(bitcoin_address), Some(share_address)) => Some(ShareCommitment {
            prev_share_blockhash: prepared.prev_share_blockhash,
            uncles: prepared.uncles.clone(),
            miner_bitcoin_address: bitcoin_address.clone(),
            miner_address: share_address,
            share_witness_root: prepared.share_witness_root,
            bits: prepared.bits,
            time: fresh_time,
            donation_address: prepared.donation_address.clone(),
            donation: prepared.donation,
            fee_address: prepared.fee_address.clone(),
            fee: prepared.fee,
            bch_coinbase: None,
        }),
        _ => None,
    };

    // The hash the miner embeds in its bitcoin coinbase must be the hash of the
    // commitment we store, so derive one from the other rather than building
    // the same digest twice.
    let commitment_hash_hex = get_commitment_hex(
        &prepared.commitment_prefix,
        fresh_time,
        &prepared.commitment_suffix,
        miner_bitcoin_address,
        miner_address,
    );

    // Build per-miner coinbase2
    let coinbase2 = build_per_miner_coinbase2(
        &commitment_hash_hex,
        nsecs,
        &prepared.coinbase2_middle,
        &prepared.coinbase2_locktime,
    );

    let job_id = tracker_handle.get_next_job_id();
    tracker_handle.insert_job_details(
        job_id,
        JobDetails {
            block_available: true,
            blocktemplate: Arc::clone(&prepared.template),
            coinbase1: prepared.coinbase1.clone(),
            coinbase2: coinbase2.clone(),
            generation_timestamp: fresh_time as u64,
            share_commitment,
            coinbase_nsecs: nsecs,
            template_merkle_branches: prepared.merkle_branches.clone(),
            share_transactions: Arc::clone(&prepared.share_transactions),
        },
    );
    (job_id, coinbase2)
}

/// Build a per-miner notify message from the prepared template.
///
/// Inserts the miner's job with [`insert_miner_job`] and overwrites the
/// job id and coinbase2 placeholders in the pre-built JSON.
pub(crate) fn build_notify_from_prepared(
    prepared: &PreparedNotifyParams,
    miner_bitcoin_address: Option<&Address>,
    miner_address: Option<p2poolv2_wallet::share_owner::ShareOwner>,
    tracker_handle: &JobTracker,
) -> Result<String, WorkError> {
    let (job_id, coinbase2) = insert_miner_job(
        prepared,
        miner_bitcoin_address,
        miner_address,
        tracker_handle,
    );
    let job_id_hex = format!("{job_id:016x}");

    // Clone the JSON template and overwrite the fixed-size placeholders.
    let mut notify_json = prepared.json_template.clone();
    notify_json.replace_range(
        prepared.job_id_offset..prepared.job_id_offset + 16,
        &job_id_hex,
    );
    notify_json.replace_range(
        prepared.coinbase2_offset..prepared.coinbase2_offset + prepared.coinbase2_placeholder_len,
        &coinbase2,
    );

    Ok(notify_json)
}

#[cfg(test)]
mod tests;
