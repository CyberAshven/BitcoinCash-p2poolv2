// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The requested payout outputs extension (`0x5052`), section 7 of
//! `docs/architecture/sv2-payout-extension.md`: a standard Job Declaration
//! client asks its Job Declaration connection for the exact coinbase outputs
//! of the job it is about to declare, instead of calling `/jd_prepare`.
//!
//! The client asks for the extension with `RequestExtensions` on a Job
//! Declaration connection. `RequestPayoutOutputs` names a token, the revenue
//! the client's template pays out, the template's height, previous block hash
//! and bits, and the scriptSig prefix the job will declare. The node answers
//! with the bytes `/jd_prepare` builds for them, through the same builder and
//! from the token's PPLNS snapshot: the exact empty merge-mining commitment
//! as output 0, the PPLNS outputs, padding, and the share commitment last.
//! `EXACT_OUTPUTS` is always set: the coinbase carries these outputs and no
//! others, with the version, input sequence and lock time the reply gives.
//! The custom job is then validated as any other (`validate_custom_job`).
//!
//! ```text
//! RequestPayoutOutputs         (0x00): request_id u32 | mining_job_token B0_255
//!                                      | available_payout_value u64 | height u32
//!                                      | prev_hash U256 | nbits u32
//!                                      | coinbase_prefix B0_255
//! RequestPayoutOutputs.Success (0x01): request_id u32 | flags u32
//!                                      | coinbase_tx_version u32
//!                                      | coinbase_tx_input_nSequence u32
//!                                      | coinbase_tx_locktime u32
//!                                      | coinbase_tx_outputs B0_64K
//! RequestPayoutOutputs.Error   (0x02): request_id u32 | error_code STR0_255
//! PayoutOutputsStale           (0x03): no fields
//! VerifyPayoutOutputs          (0x04): request_id u32 | mining_job_token B0_255
//!                                      | user_identifier STR0_255
//!                                      | available_payout_value u64 | height u32
//!                                      | prev_hash U256 | nbits u32
//!                                      | coinbase_prefix B0_255
//! VerifyPayoutOutputs.Success  (0x05): request_id u32 | flags u32
//!                                      | coinbase_tx_version u32
//!                                      | coinbase_tx_input_nSequence u32
//!                                      | coinbase_tx_locktime u32
//!                                      | coinbase_tx_outputs B0_64K
//! VerifyPayoutOutputs.Error    (0x06): request_id u32 | error_code STR0_255
//! ```
//!
//! Every frame carries extension type `0x5052` with the channel bit clear.
//! Integers are little-endian, and `prev_hash` is in the byte order of
//! `SetCustomMiningJob.prev_hash`. A connection that negotiated the extension
//! gets `PayoutOutputsStale` whenever the share new tokens are anchored on
//! changes; sets for tokens allocated before it are stale.
//! `VerifyPayoutOutputs` asks a second node, which did not issue the token,
//! to recompute those same output bytes.

use super::custom_job::{DECLARED_COINBASE_SEQUENCE, DECLARED_COINBASE_VERSION};
use super::preparation::{PreparationError, PreparedCoinbase, prepare_declared_coinbase};
use super::session::{encode_extension_frame, string_field};
use crate::accounting::payout::payout_levels::PplnsWeights;
use crate::accounting::payout::sharechain_pplns::payout::AnchorWeights;
use crate::accounting::payout::sharechain_pplns::pplns_window::WindowError;
use crate::shares::bch_coinbase::BchCoinbase;
use crate::shares::merge_mining::MergeMiningCommitment;
use crate::shares::share_commitment::ShareCommitment;
use crate::stratum::work::block_template::BlockTemplate;
use crate::stratum::work::prepared_notify::PreparedNotifyParams;
use binary_sv2::{B064K, B0255, Deserialize, Serialize, Str0255, U256};
use bitcoin::hashes::Hash;
use bitcoin::{BlockHash, CompactTarget};
use p2poolv2_wallet::payout_address::PayoutAddress;
use p2poolv2_wallet::share_owner::ShareOwner;
use std::str::FromStr;
use tracing::debug;

/// The requested payout outputs extension: `0x5052`, "PR", from SV2's
/// experimental range `0x4000` to `0x7fff`.
pub const PAYOUT_OUTPUTS_EXTENSION: u16 = 0x5052;
/// `RequestPayoutOutputs`, client to pool.
pub const REQUEST_PAYOUT_OUTPUTS: u8 = 0x00;
/// `RequestPayoutOutputs.Success`, pool to client.
pub const REQUEST_PAYOUT_OUTPUTS_SUCCESS: u8 = 0x01;
/// `RequestPayoutOutputs.Error`, pool to client.
pub const REQUEST_PAYOUT_OUTPUTS_ERROR: u8 = 0x02;
/// `PayoutOutputsStale`, pool to client, with no fields.
pub const PAYOUT_OUTPUTS_STALE: u8 = 0x03;
/// `VerifyPayoutOutputs`, client to a node that did not issue the token.
pub const VERIFY_PAYOUT_OUTPUTS: u8 = 0x04;
/// `VerifyPayoutOutputs.Success`, pool to client.
pub const VERIFY_PAYOUT_OUTPUTS_SUCCESS: u8 = 0x05;
/// `VerifyPayoutOutputs.Error`, pool to client.
pub const VERIFY_PAYOUT_OUTPUTS_ERROR: u8 = 0x06;

/// `RequestPayoutOutputs.Success.flags` bit 0: the coinbase's outputs are
/// exactly these, in this order, with no others, and its version, input
/// sequence and lock time are the ones given.
pub const EXACT_OUTPUTS: u32 = 0b1;

/// The token is unknown, expired, or already declared or used.
pub const INVALID_MINING_JOB_TOKEN: &str = "invalid-mining-job-token";
/// The token's anchor, or the template, no longer matches what the node pays.
pub const STALE_PAYOUT_OUTPUTS: &str = "stale-payout-outputs";
/// The identity does not resolve, the height is zero, or the builders refuse
/// the request: its revenue, scriptSig prefix or payouts.
pub const INVALID_REQUEST: &str = "invalid-request";
/// The outputs do not fit the coinbase or the `B0_64K` field that carries them.
pub const PAYOUT_OUTPUTS_TOO_LARGE: &str = "payout-outputs-too-large";
/// There are no PPLNS weights to split at the anchor.
pub const PAYOUT_OUTPUTS_UNAVAILABLE: &str = "payout-outputs-unavailable";
/// This node does not hold the token's anchor share, or the anchor is deeper
/// than its PPLNS window reaches. The client retries, or asks another node.
pub const UNKNOWN_ANCHOR: &str = "unknown-anchor";
/// This connection's verify budget is spent. The connection stays up.
pub const VERIFY_RATE_LIMITED: &str = "verify-rate-limited";

/// Deepest anchor, in shares below the confirmed tip, a `VerifyPayoutOutputs`
/// recomputes. A token anchors on its issuer's tip, so a verify names a share
/// at or near this node's tip; 100 shares is many minutes of share-chain
/// time. A deeper anchor is `unknown-anchor` without touching the window,
/// whose walk from an anchor it has not cached has no step cap of its own.
pub const MAX_VERIFY_ANCHOR_DEPTH: u32 = 100;

/// `RequestPayoutOutputs`: the outputs of the job a client is about to
/// declare on `mining_job_token`.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct RequestPayoutOutputs<'decoder> {
    /// Echoed in the reply.
    pub request_id: u32,
    /// From `AllocateMiningJobToken.Success`.
    pub mining_job_token: B0255<'decoder>,
    /// Satoshis the outputs pay: the template's whole coinbase revenue.
    pub available_payout_value: u64,
    /// The template's block height.
    pub height: u32,
    /// The template's previous block hash.
    pub prev_hash: U256<'decoder>,
    /// The template's compact target.
    pub nbits: u32,
    /// The coinbase scriptSig before the extranonce, exactly as the job will
    /// declare it.
    pub coinbase_prefix: B0255<'decoder>,
}

/// `RequestPayoutOutputs.Success`: the outputs and the coinbase fields a
/// job built on them must have.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct RequestPayoutOutputsSuccess<'decoder> {
    /// From the request.
    pub request_id: u32,
    /// [`EXACT_OUTPUTS`] or not; other bits are zero.
    pub flags: u32,
    /// The coinbase's version when [`EXACT_OUTPUTS`] is set.
    pub coinbase_tx_version: u32,
    /// The coinbase input's sequence when [`EXACT_OUTPUTS`] is set.
    pub coinbase_tx_input_n_sequence: u32,
    /// The coinbase's lock time when [`EXACT_OUTPUTS`] is set.
    pub coinbase_tx_locktime: u32,
    /// A CompactSize count, then that many serialized outputs, as in
    /// `SetCustomMiningJob.coinbase_tx_outputs`.
    pub coinbase_tx_outputs: B064K<'decoder>,
}

/// `RequestPayoutOutputs.Error`: why the request gets no outputs.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct RequestPayoutOutputsError<'decoder> {
    /// From the request.
    pub request_id: u32,
    /// One of the error codes of this module.
    pub error_code: Str0255<'decoder>,
}

/// `VerifyPayoutOutputs`: recompute the outputs of a token another node
/// issued, for `user_identifier`, the identity the client allocated it with.
///
/// The other fields are the ones the client sent in `RequestPayoutOutputs`.
/// The success and error payloads are those structs: a client compares them
/// byte for byte. This node keeps no registry entry for the token.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct VerifyPayoutOutputs<'decoder> {
    /// Echoed in the reply.
    pub request_id: u32,
    /// A token another node issued.
    pub mining_job_token: B0255<'decoder>,
    /// Whom the client allocated the token for, as
    /// `AllocateMiningJobToken.user_identifier`.
    pub user_identifier: Str0255<'decoder>,
    /// Satoshis the outputs pay: the template's whole coinbase revenue.
    pub available_payout_value: u64,
    /// The template's block height.
    pub height: u32,
    /// The template's previous block hash.
    pub prev_hash: U256<'decoder>,
    /// The template's compact target.
    pub nbits: u32,
    /// The coinbase scriptSig before the extranonce, exactly as the job will
    /// declare it.
    pub coinbase_prefix: B0255<'decoder>,
}

/// Whom a token's requested payout outputs pay: the payout address and share
/// owner its `AllocateMiningJobToken.user_identifier` names, resolved as a
/// mining channel's identity (`MiningChannels::payout_identity`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PayoutIdentity {
    pub payout: PayoutAddress,
    pub owner: ShareOwner,
}

/// The coinbase `request` declares on its token, which was allocated on
/// `prepared` for `identity`, while the node anchors new tokens on `anchor`.
///
/// These are the bytes `/jd_prepare` builds, from the same builder
/// (`prepare_declared_coinbase`) and the token's PPLNS snapshot, with the
/// exact empty merge-mining commitment. An error is the code
/// `RequestPayoutOutputs.Error` carries: no identity is `invalid-request`; a
/// token on an earlier anchor, or a request naming another template than the
/// token's, is `stale-payout-outputs`; no weights is
/// `payout-outputs-unavailable`; and a builder's refusal is `invalid-request`,
/// or `payout-outputs-too-large` for a coinbase past its limits.
pub(crate) fn prepare_payout_outputs(
    request: &RequestPayoutOutputs,
    prepared: &PreparedNotifyParams,
    identity: Option<&PayoutIdentity>,
    anchor: Option<BlockHash>,
) -> Result<PreparedCoinbase, &'static str> {
    let identity = identity.ok_or(INVALID_REQUEST)?;
    if anchor != Some(prepared.prev_share_blockhash())
        || !names_template(request, prepared.template())
    {
        return Err(STALE_PAYOUT_OUTPUTS);
    }
    let Some(weights) = prepared
        .pplns_weights()
        .filter(|weights| !weights.is_empty())
    else {
        return Err(PAYOUT_OUTPUTS_UNAVAILABLE);
    };
    build_declared_payout_outputs(
        request.mining_job_token.as_bytes(),
        identity,
        weights,
        request.available_payout_value,
        request.height,
        request.coinbase_prefix.as_bytes(),
    )
}

/// The coinbase `token` declares for `identity` and `weights`.
///
/// The same builder as [`prepare_payout_outputs`] (`prepare_declared_coinbase`)
/// and the exact empty merge-mining commitment. `VerifyPayoutOutputs` passes
/// the weights it just computed at the token's anchor; `RequestPayoutOutputs`
/// passes the token's stored snapshot. An empty map is
/// `payout-outputs-unavailable`. A token that does not decode is
/// `invalid-mining-job-token`. A builder's refusal is `invalid-request`, or
/// `payout-outputs-too-large` for a coinbase past its limits.
pub(crate) fn build_declared_payout_outputs(
    token: &[u8],
    identity: &PayoutIdentity,
    weights: &PplnsWeights,
    available_payout_value: u64,
    height: u32,
    coinbase_prefix: &[u8],
) -> Result<PreparedCoinbase, &'static str> {
    if weights.is_empty() {
        return Err(PAYOUT_OUTPUTS_UNAVAILABLE);
    }
    let commitment =
        ShareCommitment::from_mining_job_token(token, identity.payout.clone(), identity.owner)
            .map_err(|_| INVALID_MINING_JOB_TOKEN)?;
    let descriptor = BchCoinbase {
        prefix: coinbase_prefix.to_vec(),
        merge_mining: MergeMiningCommitment::empty(),
    };
    prepare_declared_coinbase(
        token,
        commitment,
        weights,
        available_payout_value,
        height,
        descriptor,
    )
    .map_err(|error| {
        debug!("Refused requested payout outputs: {error}");
        match error {
            PreparationError::Invalid(_) => INVALID_REQUEST,
            PreparationError::TooLarge(_) => PAYOUT_OUTPUTS_TOO_LARGE,
        }
    })
}

/// Outputs for `request` at the token's own anchor.
///
/// Stateless: nothing is stored, and the job registry is not taken. The
/// token is decoded, then `identity` resolved, then the weights computed,
/// then [`build_declared_payout_outputs`] runs outside the window lock
/// (`weights_at` holds that lock only for the distribution). No identity is
/// `invalid-request`, as is a height of zero. No window is
/// `payout-outputs-unavailable`. An anchor this node does not hold, one more
/// than [`MAX_VERIFY_ANCHOR_DEPTH`] shares below its confirmed tip, or one
/// whose window entries it no longer keeps, is `unknown-anchor`; the depth
/// is checked before the window is taken, so an old anchor costs no walk.
pub(crate) fn verify_payout_outputs(
    request: &VerifyPayoutOutputs,
    identity: Option<&PayoutIdentity>,
    weights: Option<&dyn AnchorWeights>,
) -> Result<PreparedCoinbase, &'static str> {
    let token = request.mining_job_token.as_bytes();
    let anchor =
        ShareCommitment::mining_job_token_anchor(token).map_err(|_| INVALID_MINING_JOB_TOKEN)?;
    let identity = identity.ok_or(INVALID_REQUEST)?;
    if request.height == 0 {
        return Err(INVALID_REQUEST);
    }
    let weights = weights.ok_or(PAYOUT_OUTPUTS_UNAVAILABLE)?;
    if !weights
        .anchor_depth(anchor)
        .is_some_and(|depth| depth <= MAX_VERIFY_ANCHOR_DEPTH)
    {
        return Err(UNKNOWN_ANCHOR);
    }
    let prev_hash = block_hash_of(&request.prev_hash)?;
    let distribution = weights
        .weights_at(anchor, request.nbits, prev_hash)
        .map_err(|error| {
            debug!("Verify payout outputs could not use anchor {anchor}: {error}");
            anchor_error_code(&error)
        })?;
    build_declared_payout_outputs(
        token,
        identity,
        &distribution,
        request.available_payout_value,
        request.height,
        request.coinbase_prefix.as_bytes(),
    )
}

/// `prev_hash` as a block hash. The field is 32 bytes, in the byte order of
/// `SetCustomMiningJob.prev_hash`.
fn block_hash_of(prev_hash: &U256) -> Result<BlockHash, &'static str> {
    let bytes: [u8; 32] = prev_hash
        .as_bytes()
        .try_into()
        .map_err(|_| INVALID_REQUEST)?;
    Ok(BlockHash::from_byte_array(bytes))
}

/// The error code a window error becomes for `VerifyPayoutOutputs`.
///
/// A missing anchor share, a store read that failed while looking for it,
/// an anchor deeper than this window, and a Bitcoin Cash header the window
/// does not have yet are `unknown-anchor`: the client retries or asks
/// another node. A window that pays no one is `payout-outputs-unavailable`.
fn anchor_error_code(error: &WindowError) -> &'static str {
    match error {
        WindowError::InsufficientEntries { .. }
        | WindowError::RightsHistoryTooShallow { .. }
        | WindowError::ReadFailure(_)
        | WindowError::UnknownBchPrev { .. } => UNKNOWN_ANCHOR,
        WindowError::NothingEligible { .. } => PAYOUT_OUTPUTS_UNAVAILABLE,
    }
}

/// Whether `request` names `template`'s height, previous block hash and
/// bits, as `SetCustomMiningJob` must (`validate_custom_job`).
fn names_template(request: &RequestPayoutOutputs, template: &BlockTemplate) -> bool {
    let prev_hash = BlockHash::from_str(&template.previousblockhash).ok();
    let bits = CompactTarget::from_unprefixed_hex(&template.bits).ok();
    request.height == template.height
        && prev_hash.is_some_and(|hash| request.prev_hash.as_bytes() == hash.as_byte_array())
        && bits.is_some_and(|bits| request.nbits == bits.to_consensus())
}

/// The frame answering `request`: its outputs with [`EXACT_OUTPUTS`], the
/// declared coinbase's version and sequence and a lock time of the height
/// less one, or the error code.
pub(crate) fn answer_frame(
    request: &RequestPayoutOutputs,
    answer: Result<PreparedCoinbase, &'static str>,
) -> Result<Vec<u8>, String> {
    payout_answer_frame(
        request.request_id,
        request.height,
        answer,
        REQUEST_PAYOUT_OUTPUTS_SUCCESS,
        REQUEST_PAYOUT_OUTPUTS_ERROR,
    )
}

/// [`answer_frame`] for `VerifyPayoutOutputs`: the same payload bytes, with
/// message types [`VERIFY_PAYOUT_OUTPUTS_SUCCESS`] and
/// [`VERIFY_PAYOUT_OUTPUTS_ERROR`].
pub(crate) fn verify_answer_frame(
    request_id: u32,
    height: u32,
    answer: Result<PreparedCoinbase, &'static str>,
) -> Result<Vec<u8>, String> {
    payout_answer_frame(
        request_id,
        height,
        answer,
        VERIFY_PAYOUT_OUTPUTS_SUCCESS,
        VERIFY_PAYOUT_OUTPUTS_ERROR,
    )
}

/// One success or error payload, framed as `success_type` or `error_type`.
///
/// A height of zero has no lock time of height minus one, so a coinbase
/// that was built becomes `invalid-request`. The success and error structs
/// are the request's: verify and request replies differ only by message type.
fn payout_answer_frame(
    request_id: u32,
    height: u32,
    answer: Result<PreparedCoinbase, &'static str>,
    success_type: u8,
    error_type: u8,
) -> Result<Vec<u8>, String> {
    let answer = answer.and_then(|coinbase| {
        let locktime = height.checked_sub(1).ok_or(INVALID_REQUEST)?;
        Ok((coinbase, locktime))
    });
    let (message_type, payload) = match answer {
        Ok((coinbase, locktime)) => (
            success_type,
            binary_sv2::to_bytes(RequestPayoutOutputsSuccess {
                request_id,
                flags: EXACT_OUTPUTS,
                coinbase_tx_version: DECLARED_COINBASE_VERSION,
                coinbase_tx_input_n_sequence: DECLARED_COINBASE_SEQUENCE,
                coinbase_tx_locktime: locktime,
                coinbase_tx_outputs: B064K::new(coinbase.outputs())
                    .map_err(|error| error.to_string())?,
            }),
        ),
        Err(error_code) => (
            error_type,
            binary_sv2::to_bytes(RequestPayoutOutputsError {
                request_id,
                error_code: string_field(error_code)?,
            }),
        ),
    };
    let payload = payload.map_err(|error| error.to_string())?;
    encode_extension_frame(PAYOUT_OUTPUTS_EXTENSION, message_type, false, &payload)
}

/// The `PayoutOutputsStale` frame, which has no fields.
pub(crate) fn stale_frame() -> Result<Vec<u8>, String> {
    encode_extension_frame(PAYOUT_OUTPUTS_EXTENSION, PAYOUT_OUTPUTS_STALE, false, &[])
}

#[cfg(test)]
mod tests;
