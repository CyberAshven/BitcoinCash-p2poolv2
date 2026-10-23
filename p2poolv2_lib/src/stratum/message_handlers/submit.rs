// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::accounting::payout::simple_pplns::SimplePplnsShare;
use crate::config::PoolMode;
use crate::shares::coinbase_proof::CoinbaseProof;
use crate::stratum::{
    difficulty_adjuster::DifficultyAdjusterTrait,
    emission::Emission,
    error::{Error, StratumErrorCode},
    messages::{Message, Response, SetDifficultyNotification, SimpleRequest},
    server::StratumContext,
    session::Session,
    work::{
        block_template::BlockTemplate,
        difficulty::validate::validate_bitcoin_difficulty,
        error::WorkError,
        tracker::{JobDetails, JobId},
    },
};
use bitcoin::{BlockHash, block::Header, blockdata::block::Block, hashes::Hash};
use bitcoindrpc::BitcoindRpcClient;
use serde_json::json;
use std::sync::Arc;
use std::time::SystemTime;
use tracing::{debug, error, info, warn};

/// Handle the "mining.submit" message
/// This function is called when a miner submits a share to the Stratum server.
/// It sends a response with the submission status.
///
/// Message format:
///
/// {"id": 1, "method": "mining.submit", "params": ["username", "jobid", "extranonce2", "nTime", "nonce"]}
/// Example message:
/// {"id": 1, "method": "mining.submit", "params": ["username", "4f", "fe36a31b", "504e86ed", "e9695791"]}
/// We can also receive messages with version_mask as the last parameter
/// {"id": 1, "method": "mining.submit", "params": ["username", "jobid", "extranonce2", "nTime", "nonce", "version_mask"]}
/// Example message:
/// {"id": 1, "method": "mining.submit", "params": ["username", "4f", "fe36a31b", "504e86ed", "e9695791", "1fffe000"]}
///
/// Handling version mask, we check mask is valid and then apply it to the block header
///
/// Where an accepted share goes depends on the pool mode. In P2Poolv2 mode a
/// share meeting the pool target is emitted to the share chain. In Hydrapool
/// mode a share whose hash meets the session difficulty is queued for the PPLNS
/// share worker; it is never emitted.
pub(crate) async fn handle_submit<'a, D: DifficultyAdjusterTrait>(
    message: SimpleRequest<'a>,
    session: &mut Session<D>,
    stratum_context: StratumContext,
) -> Result<Vec<Message<'a>>, Error> {
    debug!("Handling mining.submit message");
    if !session.subscribed {
        return Ok(vec![Message::Response(Response::new_error(
            message.id,
            StratumErrorCode::NotSubscribed,
        ))]);
    }
    if session.user_id.is_none() {
        return Ok(vec![Message::Response(
            Response::new_error(message.id, StratumErrorCode::UnauthorizedWorker)
                .with_message("Not authorized".to_string()),
        )]);
    }
    if message.params.len() < 5 {
        return Err(Error::InvalidParams("Missing parameters".into()));
    }

    let id = message.params[1]
        .as_ref()
        .ok_or_else(|| Error::InvalidParams("Missing job_id".into()))?;

    let job_id =
        u64::from_str_radix(id, 16).map_err(|_| Error::InvalidParams("Invalid job_id".into()))?;

    let job = match stratum_context.tracker_handle.get_job(JobId(job_id)) {
        Some(job) => job,
        None => {
            debug!("Job not found for job_id: {job_id}");
            return Ok(vec![Message::Response(Response::new_error(
                message.id,
                StratumErrorCode::JobNotFound,
            ))]);
        }
    };

    let current_difficulty = session.difficulty_adjuster.get_current_difficulty();

    // version mask from the session - we ignore different version mask sent in a submit message
    let version_mask = session.version_mask;

    // Validate the difficulty of the submitted share
    let validation_result =
        match validate_bitcoin_difficulty(&job, &message, &session.enonce1_hex, version_mask) {
            Ok(result) => result,
            Err(e) => {
                debug!("Share validation failed: {}", e);
                let response = Response::new_error(message.id, StratumErrorCode::OtherUnknown)
                    .with_message(e.to_string());
                return Ok(vec![Message::Response(response)]);
            }
        };

    // Compute the block hash once; it is reused for dedup, the pool-target
    // check, and the true-difficulty calculation on this hot path.
    let block_hash = validation_result.header.block_hash();

    let is_new_share = stratum_context
        .tracker_handle
        .add_share(JobId(job_id), block_hash);

    if !is_new_share {
        return Ok(vec![Message::Response(Response::new_error(
            message.id,
            StratumErrorCode::DuplicateShare,
        ))]);
    }

    if validation_result.meets_bitcoin_difficulty {
        submit_found_block(
            &stratum_context,
            validation_result.header,
            validation_result.coinbase.clone(),
            &job,
        )
        .await?;
    }

    // Mining difficulties are tracked as `truediffone`, i.e. difficulty is computed relative to mainnet
    let truediff = get_true_difficulty(&block_hash);
    debug!("True difficulty: {}", truediff);

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    // params[2] (extranonce2) and params[4] (nonce) are guaranteed present
    // because validate_bitcoin_difficulty already validated them above.
    let extranonce2 = message.params[2]
        .as_ref()
        .expect("validated by validate_bitcoin_difficulty");
    let nonce = message.params[4]
        .as_ref()
        .expect("validated by validate_bitcoin_difficulty");
    let stratum_share = SimplePplnsShare::new(
        session.user_id.unwrap(),
        current_difficulty,
        session.btcaddress.clone().unwrap_or_default(),
        session.workername.clone().unwrap_or_default(),
        timestamp,
        id.to_string(),
        extranonce2.to_string(),
        nonce.to_string(),
    );

    // The hash proves work only up to `truediff`. A share is credited at the
    // session difficulty, so it may only be stored when its hash meets that
    // difficulty. `ignore_difficulty` is a testing aid for the miner response
    // and metrics below; it must never open this gate.
    let hash_meets_session_difficulty = truediff >= current_difficulty as u128;

    emit_pool_share(
        &stratum_context,
        PoolShare {
            job_id: JobId(job_id),
            job: &job,
            header: validation_result.header,
            block_hash,
            coinbase: &validation_result.coinbase,
            share: stratum_share.clone(),
            meets_credited_difficulty: hash_meets_session_difficulty,
        },
    )
    .await?;

    session.last_share_time = Some(SystemTime::now());

    // If we are in testing mode and ignoring difficulty, accept share as meeting current difficulty
    let meets_session_difficulty =
        stratum_context.ignore_difficulty || hash_meets_session_difficulty;

    if meets_session_difficulty {
        let _ = stratum_context
            .metrics
            .record_share_accepted(stratum_share, truediff as u64)
            .await;
    } else {
        let _ = stratum_context.metrics.record_share_rejected().await;
    }

    let (new_difficulty, _is_first_share) = session.difficulty_adjuster.record_share_submission(
        truediff,
        job_id,
        session.suggested_difficulty,
        SystemTime::now(),
    );

    let mut response = vec![Message::Response(Response::new_ok(
        message.id,
        json!(meets_session_difficulty),
    ))];
    match new_difficulty {
        Some(difficulty) => {
            response.push(Message::SetDifficulty(SetDifficultyNotification::new(
                difficulty,
            )));
            Ok(response)
        }
        None => Ok(response),
    }
}

/// Submit a block whose header met the bitcoin target.
///
/// Under the `sim` feature this does nothing: on regtest a random header
/// meets the bitcoin target about half the time, so submitting would spam
/// spurious blocks and (via the confirmation -> re-notify loop) distort the
/// measured uncle rate. Real block-finds in the load test go only through the
/// statistical block-find path. See docs/simulation/load-test-plan.md.
///
/// A block on a template from a Stratum V2 Template Provider goes back to
/// that provider; when the provider's connection cannot take it, it goes to
/// the node over RPC like every other block.
pub(crate) async fn submit_found_block(
    stratum_context: &StratumContext,
    header: Header,
    coinbase: bitcoin::Transaction,
    job: &JobDetails,
) -> Result<(), Error> {
    if !job.block_available {
        info!(
            "Private declared block {} must be broadcast by its declarator",
            header.block_hash()
        );
        return Ok(());
    }
    #[cfg(not(feature = "sim"))]
    {
        // Decode the template transactions only for this rare case.
        let block = build_full_block(header, coinbase, &job.blocktemplate)
            .map_err(|error| Error::SubmitFailure(error.to_string()))?;
        let unsent = match &job.blocktemplate.provided {
            Some(provided) => provided.submit(block).err(),
            None => Some(block),
        };
        if let Some(block) = unsent
            && submit_block(&block, &stratum_context.bitcoindrpc_client).await
        {
            request_template_refresh(stratum_context);
        }
    }
    #[cfg(feature = "sim")]
    {
        let _ = (stratum_context, header, coinbase, job);
    }
    Ok(())
}

/// A share rebuilt from a tracker job, ready for the share chain or the
/// Hydrapool PPLNS share worker.
pub(crate) struct PoolShare<'a> {
    pub job_id: JobId,
    pub job: &'a JobDetails,
    pub header: Header,
    pub block_hash: BlockHash,
    pub coinbase: &'a bitcoin::Transaction,
    pub share: SimplePplnsShare,
    /// Whether the share's hash meets the difficulty it is credited at: the
    /// SV1 session difficulty or the SV2 channel target. Never opened by
    /// `ignore_difficulty`, which only changes the miner's reply and metrics.
    pub meets_credited_difficulty: bool,
}

/// Send an accepted share where the pool mode stores it.
///
/// P2Poolv2 emits it to the share chain when it meets the pool target
/// (`emit_share_chain_share`). Hydrapool queues it for the PPLNS share worker
/// when its hash meets the difficulty it is credited at, and never emits it.
/// Callers keep hashrate and vardiff accounting on the full share stream
/// either way. Shared by SV1 and SV2. Returns whether the share was sent.
pub(crate) async fn emit_pool_share(
    stratum_context: &StratumContext,
    pool_share: PoolShare<'_>,
) -> Result<bool, Error> {
    match stratum_context.mode {
        PoolMode::P2poolv2 => emit_share_chain_share(stratum_context, pool_share).await,
        PoolMode::Hydrapool => {
            if !pool_share.meets_credited_difficulty {
                return Ok(false);
            }
            // The send only enqueues, so the connection returns to the miner
            // without waiting on the store.
            stratum_context
                .pplns_shares_tx
                .send(pool_share.share)
                .await
                .map_err(|e| {
                    Error::SubmitFailure(format!("Failed to send PPLNS share to store: {e}"))
                })?;
            Ok(true)
        }
    }
}

/// Emit a P2Poolv2 share to the share chain when it meets the pool target.
///
/// The share commitment carries the ASERT-computed pool target (bits), and on
/// Bitcoin Cash the declared coinbase layout. A job without a commitment (no
/// miner share address) has nothing to emit. Returns whether the share was
/// emitted.
async fn emit_share_chain_share(
    stratum_context: &StratumContext,
    pool_share: PoolShare<'_>,
) -> Result<bool, Error> {
    let job = pool_share.job;
    let Some(commitment) = &job.share_commitment else {
        return Ok(false);
    };
    let pool_target = bitcoin::Target::from_compact(commitment.bits);
    if !pool_target.is_met_by(pool_share.block_hash) {
        debug!(
            "Share does not meet pool difficulty, not emitting to share chain: hash {} target {:?}",
            pool_share.block_hash, commitment.bits
        );
        return Ok(false);
    }
    // The full coinbase exists only here, so the proof is built now. Every
    // coinbase this pool builds ends with the commitment, so a failure is a
    // malformed job, not the miner's fault: log it and keep the share off the
    // share chain. Hashrate and vardiff accounting still run in the caller.
    let coinbase_proof = match CoinbaseProof::from_coinbase(
        pool_share.coinbase,
        commitment.share_witness_root,
    ) {
        Ok(coinbase_proof) => coinbase_proof,
        Err(error) => {
            error!(
                "Failed to build coinbase proof for job {}, not emitting to share chain: {error}",
                pool_share.job_id.0
            );
            return Ok(false);
        }
    };
    stratum_context
        .emissions_tx
        .send(Emission {
            header: pool_share.header,
            blocktemplate: job.blocktemplate.clone(),
            share_commitment: commitment.clone(),
            template_merkle_branches: job.template_merkle_branches.clone(),
            bitcoin_coinbase: pool_share.coinbase.clone(),
            coinbase_proof,
            share_transactions: Arc::clone(&job.share_transactions),
        })
        .await
        .map_err(|e| Error::SubmitFailure(format!("Failed to send share to store: {e}")))?;
    Ok(true)
}

/// Submit block to bitcoind using the shared RPC client.
///
/// Returns whether bitcoind accepted the submission.
pub async fn submit_block(block: &Block, bitcoindrpc_client: &BitcoindRpcClient) -> bool {
    tracing::info!(
        "Submitting block to bitcoind: {:?}",
        block.header.block_hash()
    );
    match bitcoindrpc_client.submit_block(block).await {
        // submitblock answers null when it accepts the block, and a reason
        // such as "duplicate" or "high-hash" when it does not.
        Ok(result) if result == "null" => {
            info!("Block submitted successfully");
            true
        }
        Ok(reason) => {
            warn!("bitcoind did not accept the block: {reason}");
            false
        }
        Err(e) => {
            error!("Failed to submit block: {}", e);
            false
        }
    }
}

/// Ask the template task for a new template now, so miners move off the
/// parent of the block this node just found instead of finding same-height
/// siblings until the next ZMQ event or poll. A full channel means a refresh
/// is already pending, which is all this needs.
#[cfg(not(feature = "sim"))]
fn request_template_refresh(stratum_context: &StratumContext) {
    if let Some(template_refresh_tx) = &stratum_context.template_refresh_tx {
        let _ = template_refresh_tx.try_send(());
    }
}

/// Build full block from header, coinbase and blocktemplate
/// Return an error for malformed template transactions rather than panicking.
/// Only called for the rare case of finding a bitcoin block (real auto-submit
/// in non-sim builds; the sim statistical block-find under `sim`).
pub(crate) fn build_full_block(
    header: Header,
    coinbase: bitcoin::Transaction,
    blocktemplate: &BlockTemplate,
) -> Result<Block, WorkError> {
    let mut all_transactions = Vec::with_capacity(blocktemplate.transactions.len() + 1);
    all_transactions.push(coinbase);
    all_transactions.extend(blocktemplate.decode_transactions()?);

    Ok(Block {
        header,
        txdata: all_transactions,
    })
}

/// Use bitcoin mainnet max attainable target to convert the hash into difficulty
/// This global difficulty to used to track difficult adjustment by the pool, independent of the chain that is being mined.
pub(crate) fn get_true_difficulty(hash: &bitcoin::BlockHash) -> u128 {
    let mut bytes = hash.to_byte_array();
    bytes.reverse();
    let diff = u128::from_str_radix(&hex::encode(&bytes[..16]), 16).unwrap();
    (0xFFFF_u128 << (208 - 128)) / diff
}

#[cfg(test)]
mod handle_submit_tests;
