// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Sending a found Bitcoin Cash block to BCHN.
//!
//! A found block is worth a whole block reward, so it must not depend on one
//! RPC call. Each node it goes to gets it with `submitblock`, sent again after
//! a connection error, a time-out or a busy node until the node answers or the
//! retry window ends. An answer is final: BCHN's reject reason does not change
//! when the same block is sent again. The block also goes, at the same time,
//! to every extra node the operator lists in `[[submit_nodes]]`, each with its
//! own retries, so a node that is down or slow loses nothing. P2Pool v1 sent a
//! found block over two paths with retries (V9 in
//! `docs/p2pool-v1-bch-lessons.md`). A block found on a light template goes,
//! with the same retries, only to the node that made the template, with
//! `submitblocklight`.

use crate::accounting::stats::metrics::MetricsHandle;
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
use crate::store::found_block::FoundBlock;
use bitcoin::Block;
use bitcoindrpc::{BitcoindRpcClient, SubmitBlockOutcome};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::{Instant, sleep, timeout};
use tracing::{error, info, warn};

/// Metric label of the node in `[bitcoinrpc]`.
pub const PRIMARY_NODE_LABEL: &str = "primary";

/// How long and how often a found block is sent again to a node that gave
/// no answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubmitRetryPolicy {
    /// No new attempt starts after this long from the first.
    pub retry_window: Duration,
    /// An attempt with no answer after this long counts as a time-out. The
    /// node may still be validating the block; the next attempt then gets
    /// `duplicate` or `inconclusive`.
    pub attempt_timeout: Duration,
    /// Wait before the first retry. Each later wait doubles, up to
    /// `max_backoff`.
    pub first_backoff: Duration,
    /// Longest wait between two attempts.
    pub max_backoff: Duration,
}

/// Retry policy for found blocks: two minutes of attempts, each given 30
/// seconds, waits from half a second doubling up to 10 seconds. P2Pool v1
/// tried 10 times, 10 seconds apart.
pub const FOUND_BLOCK_RETRY_POLICY: SubmitRetryPolicy = SubmitRetryPolicy {
    retry_window: Duration::from_secs(120),
    attempt_timeout: Duration::from_secs(30),
    first_backoff: Duration::from_millis(500),
    max_backoff: Duration::from_secs(10),
};

/// One attempt per node, never sent again: for tests whose node is
/// unreachable on purpose, where retries would only make them slow.
pub const SINGLE_ATTEMPT_POLICY: SubmitRetryPolicy = SubmitRetryPolicy {
    retry_window: Duration::ZERO,
    attempt_timeout: Duration::from_secs(30),
    first_backoff: Duration::ZERO,
    max_backoff: Duration::ZERO,
};

/// Where a found block goes besides the node in `[bitcoinrpc]`, and how it
/// is sent again when a node gives no answer.
#[derive(Debug, Clone)]
pub struct FoundBlockSubmitter {
    /// Extra BCHN nodes from `[[submit_nodes]]`, in file order.
    pub extra_nodes: Arc<[BitcoindRpcClient]>,
    /// Retries for every node, [`FOUND_BLOCK_RETRY_POLICY`] on a running node.
    pub retry: SubmitRetryPolicy,
}

impl FoundBlockSubmitter {
    /// Send found blocks to `extra_nodes` too, with
    /// [`FOUND_BLOCK_RETRY_POLICY`].
    pub fn new(extra_nodes: Arc<[BitcoindRpcClient]>) -> Self {
        Self {
            extra_nodes,
            retry: FOUND_BLOCK_RETRY_POLICY,
        }
    }

    /// No extra nodes and [`SINGLE_ATTEMPT_POLICY`].
    pub fn single_attempt() -> Self {
        Self {
            extra_nodes: Arc::from([]),
            retry: SINGLE_ATTEMPT_POLICY,
        }
    }
}

/// What became of a found block at one node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockSubmission {
    /// The node accepted the block.
    Accepted,
    /// The node already had the block and found it valid, for example from
    /// an attempt whose answer was lost.
    Duplicate,
    /// The node stored the block without a verdict, for example because
    /// another block at this height arrived first and stays its best chain
    /// (first seen). Sending it again does not change that.
    Inconclusive,
    /// The node rejected the block, with its reason.
    Rejected(String),
    /// The node gave no answer within the retry window, or refused the
    /// request itself (for example a wrong RPC password).
    Failed(String),
}

impl BlockSubmission {
    /// Only a positive BCHN verdict proves acceptance. Merely storing a block
    /// with an inconclusive verdict does not count as finding one.
    pub fn is_accepted(&self) -> bool {
        matches!(self, Self::Accepted | Self::Duplicate)
    }

    /// Metric label of this result.
    pub fn label(&self) -> &'static str {
        match self {
            BlockSubmission::Accepted => "accepted",
            BlockSubmission::Duplicate => "duplicate",
            BlockSubmission::Inconclusive => "inconclusive",
            BlockSubmission::Rejected(_) => "rejected",
            BlockSubmission::Failed(_) => "failed",
        }
    }

    /// Whether the node now holds the block: accepted, a duplicate of a
    /// valid block, or stored without a verdict.
    pub fn node_has_block(&self) -> bool {
        matches!(
            self,
            BlockSubmission::Accepted | BlockSubmission::Duplicate | BlockSubmission::Inconclusive
        )
    }
}

/// The pool share behind a locally submitted block, recorded only after BCHN
/// accepts it. Copies share one serialized store writer, so several BCHN
/// replies for the same block reset effort only once.
#[derive(Clone)]
pub struct FoundBlockAcceptance {
    pub store: ChainStoreHandle,
    pub found: FoundBlock,
}

impl FoundBlockAcceptance {
    /// Persist a positive node verdict before resetting the effort metric.
    pub async fn record(&self, result: &BlockSubmission, metrics: &MetricsHandle) {
        if result.is_accepted() {
            match self
                .store
                .record_accepted_found_block(self.found.clone())
                .await
            {
                Ok(true) => {
                    let _ = metrics.reset_block_effort().await;
                }
                Ok(false) => {}
                Err(error) => error!(
                    "Cannot record accepted BCH block {}: {error}",
                    self.found.bitcoin_blockhash
                ),
            }
        }
    }
}

impl From<SubmitBlockOutcome> for BlockSubmission {
    fn from(outcome: SubmitBlockOutcome) -> Self {
        match outcome {
            SubmitBlockOutcome::Accepted => BlockSubmission::Accepted,
            SubmitBlockOutcome::Duplicate => BlockSubmission::Duplicate,
            SubmitBlockOutcome::Inconclusive => BlockSubmission::Inconclusive,
            SubmitBlockOutcome::Rejected(reason) => BlockSubmission::Rejected(reason),
        }
    }
}

/// Send `block` to one node with `submitblock`, again after each error that
/// may pass ([`bitcoindrpc::BitcoindRpcError::is_transient`]) or time-out,
/// until the node answers or `policy.retry_window` ends.
///
/// A block found on a light template, whose job is `light_job_id`, holds
/// only its header and coinbase and goes with `submitblocklight` instead.
///
/// Returns the result and how many times the block was sent again.
pub async fn submit_with_retry(
    block: &Block,
    light_job_id: Option<&str>,
    client: &BitcoindRpcClient,
    policy: &SubmitRetryPolicy,
) -> (BlockSubmission, u32) {
    let started = Instant::now();
    let mut backoff = policy.first_backoff;
    let mut retries = 0;
    loop {
        let attempt = async {
            match light_job_id {
                Some(job_id) => client.submit_block_light(block, job_id).await,
                None => client.submit_block(block).await,
            }
        };
        let failure = match timeout(policy.attempt_timeout, attempt).await {
            Ok(Ok(outcome)) => return (outcome.into(), retries),
            Ok(Err(error)) if !error.is_transient() => {
                return (BlockSubmission::Failed(error.to_string()), retries);
            }
            Ok(Err(error)) => error.to_string(),
            Err(_) => format!(
                "no answer within {} seconds",
                policy.attempt_timeout.as_secs_f64()
            ),
        };
        if started.elapsed() + backoff > policy.retry_window {
            return (BlockSubmission::Failed(failure), retries);
        }
        warn!(
            "Block {} not submitted yet ({failure}); sending it again in {} ms",
            block.block_hash(),
            backoff.as_millis()
        );
        sleep(backoff).await;
        retries += 1;
        backoff = (backoff * 2).min(policy.max_backoff);
    }
}

/// Send `block` to the node in `[bitcoinrpc]` with retries and log what it
/// said. Used where no metrics handle exists: the Template Provider's RPC
/// fallback and the simulation's block find.
pub async fn submit_block(block: &Block, client: &BitcoindRpcClient) -> BlockSubmission {
    let (result, retries) = submit_with_retry(block, None, client, &FOUND_BLOCK_RETRY_POLICY).await;
    log_submission(block, PRIMARY_NODE_LABEL, &result, retries);
    result
}

/// Send a found block to every node in `[[submit_nodes]]` (`extra_nodes`),
/// each from its own background task with its own retries, so a slow or
/// unreachable extra node never delays the node in `[bitcoinrpc]`. Each
/// result is logged and counted in `found_block_submissions_total`.
pub fn spawn_extra_node_submissions(
    block: &Arc<Block>,
    extra_nodes: &[BitcoindRpcClient],
    metrics: &MetricsHandle,
    policy: SubmitRetryPolicy,
    acceptance: Option<FoundBlockAcceptance>,
) {
    for (index, client) in extra_nodes.iter().enumerate() {
        let block = Arc::clone(block);
        let client = client.clone();
        let metrics = metrics.clone();
        let label = extra_node_label(index);
        let acceptance = acceptance.clone();
        tokio::spawn(async move {
            let (result, retries) = submit_with_retry(&block, None, &client, &policy).await;
            log_submission(&block, &label, &result, retries);
            record_submission(&metrics, label, &result, retries).await;
            if let Some(acceptance) = acceptance {
                acceptance.record(&result, &metrics).await;
            }
        });
    }
}

/// Send a found block to the node in `[bitcoinrpc]` with retries, log the
/// result and count it in `found_block_submissions_total` as `primary`.
/// `light_job_id` names the light template a block was found on
/// ([`submit_with_retry`]).
pub async fn submit_to_primary_node(
    block: &Block,
    light_job_id: Option<&str>,
    client: &BitcoindRpcClient,
    metrics: &MetricsHandle,
    policy: &SubmitRetryPolicy,
) -> BlockSubmission {
    let (result, retries) = submit_with_retry(block, light_job_id, client, policy).await;
    log_submission(block, PRIMARY_NODE_LABEL, &result, retries);
    record_submission(metrics, PRIMARY_NODE_LABEL.to_string(), &result, retries).await;
    result
}

/// Metric label of the extra node at `index` in `[[submit_nodes]]`:
/// `extra1` for the first. Labels name the position, never the URL, which
/// may carry credentials.
pub fn extra_node_label(index: usize) -> String {
    format!("extra{}", index + 1)
}

async fn record_submission(
    metrics: &MetricsHandle,
    node: String,
    result: &BlockSubmission,
    retries: u32,
) {
    if metrics
        .record_block_submission(node, result.label(), retries)
        .await
        .is_err()
    {
        warn!("Metrics actor stopped; found block submission not counted");
    }
}

fn log_submission(block: &Block, node: &str, result: &BlockSubmission, retries: u32) {
    let block_hash = block.block_hash();
    match result {
        BlockSubmission::Accepted => {
            info!("Node {node} accepted block {block_hash} (retries: {retries})")
        }
        BlockSubmission::Duplicate => {
            info!("Node {node} already has block {block_hash} and found it valid")
        }
        BlockSubmission::Inconclusive => warn!(
            "Node {node} stored block {block_hash} without a verdict; another block at this \
             height may have arrived first"
        ),
        BlockSubmission::Rejected(reason) => {
            error!("Node {node} rejected block {block_hash}: {reason}")
        }
        BlockSubmission::Failed(failure) => error!(
            "Block {block_hash} could not be submitted to node {node} after {retries} retries: \
             {failure}"
        ),
    }
}

#[cfg(test)]
mod tests;
