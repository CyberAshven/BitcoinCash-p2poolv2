// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! One Stratum V2 connection and the job registry it shares with the others.
//!
//! A Job Declaration connection and a Mining connection are separate TCP
//! sessions. The registry is what lets a token allocated on the first be
//! presented on the second.

use super::custom_job::declared_leaves;
use super::large_templates::{
    DECLARE_MINING_JOB_TRANSACTIONS, DeclareMiningJobTransactions, MAX_IDS_PER_MESSAGE,
    max_declared_transactions, positions_frame,
};
use super::payout_outputs::{
    INVALID_MINING_JOB_TOKEN, PAYOUT_OUTPUTS_UNAVAILABLE, PayoutIdentity, REQUEST_PAYOUT_OUTPUTS,
    RequestPayoutOutputs, VERIFY_RATE_LIMITED, VerifyPayoutOutputs, answer_frame,
    prepare_payout_outputs, stale_frame, verify_answer_frame, verify_payout_outputs,
};
use super::sort_ctor;
use crate::accounting::payout::AnchorWeights;
use crate::shares::auxpow::Disclosure;
use crate::shares::merge_mining::MergeMiningCommitment;
use crate::stratum::parse_password::{
    log_unknown_options, parse_miner_donation, parse_miner_payout_level, split_user_identity,
};
use crate::stratum::work::coinbase::declared_reservation_outputs;
use crate::stratum::work::gbt::{compute_merkle_branches, compute_merkle_root_from_branches};
use crate::stratum::work::prepared_notify::PreparedNotifyParams;
use crate::utils::time_provider::{SystemTimeProvider, TimeProvider};
use binary_sv2::{B064K, B0255, Deserialize, Seq064K, Str0255};
use bitcoin::block::{Header, Version};
use bitcoin::consensus::{deserialize, serialize};
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::rand::random;
use bitcoin::{Block, BlockHash, CompactTarget, Target, Transaction, TxMerkleNode, TxOut};
use codec_sv2::{Decoded, Decoder, Encoder};
use common_messages_sv2::{
    MESSAGE_TYPE_SETUP_CONNECTION, MESSAGE_TYPE_SETUP_CONNECTION_ERROR,
    MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS, Protocol, SetupConnection, SetupConnectionError,
    SetupConnectionSuccess, has_declare_tx_data, has_work_selection,
};
use framing_sv2::framing::MessageFrame;
#[cfg(test)]
use framing_sv2::framing::SerializedFrame;
use framing_sv2::header::Header as FrameHeader;
use ipnet::IpNet;
use job_declaration_sv2::{
    AllocateMiningJobToken, AllocateMiningJobTokenSuccess, DeclareMiningJob, DeclareMiningJobError,
    DeclareMiningJobSuccess, ERROR_CODE_DECLARE_MINING_JOB_INVALID_JOB,
    ERROR_CODE_DECLARE_MINING_JOB_INVALID_MINING_JOB_TOKEN, MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN,
    MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN_SUCCESS, MESSAGE_TYPE_DECLARE_MINING_JOB,
    MESSAGE_TYPE_DECLARE_MINING_JOB_ERROR, MESSAGE_TYPE_DECLARE_MINING_JOB_SUCCESS,
    MESSAGE_TYPE_PROVIDE_MISSING_TRANSACTIONS, MESSAGE_TYPE_PROVIDE_MISSING_TRANSACTIONS_SUCCESS,
    MESSAGE_TYPE_PUSH_SOLUTION, ProvideMissingTransactions, ProvideMissingTransactionsSuccess,
    PushSolution,
};
use mining_sv2::{
    MESSAGE_TYPE_CLOSE_CHANNEL, MESSAGE_TYPE_MINING_SET_NEW_PREV_HASH,
    MESSAGE_TYPE_NEW_EXTENDED_MINING_JOB, MESSAGE_TYPE_NEW_MINING_JOB,
    MESSAGE_TYPE_SET_CUSTOM_MINING_JOB, MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_ERROR,
    MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_SUCCESS, MESSAGE_TYPE_SET_TARGET,
    MESSAGE_TYPE_SUBMIT_SHARES_ERROR, MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED,
    MESSAGE_TYPE_SUBMIT_SHARES_STANDARD, MESSAGE_TYPE_SUBMIT_SHARES_SUCCESS,
    MESSAGE_TYPE_UPDATE_CHANNEL, MESSAGE_TYPE_UPDATE_CHANNEL_ERROR, SetCustomMiningJob,
    SetCustomMiningJobError,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::net::IpAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tracing::debug;

const PROTOCOL_VERSION: u16 = 2;
/// The error a declaration whose coinbase the node refuses gets, as
/// Pickaxe's pool sends it: here, token leaves that are missing, not
/// canonical, or do not make output 0's root.
const INVALID_COINBASE_TX: &str = "invalid-coinbase-tx";
const WORK_SELECTION_FLAG: u32 = 0b10;
/// `SetupConnection` flag `DECLARE_TX_DATA` of the Job Declaration
/// Protocol: the client reveals its transactions (Full-Template mode).
const DECLARE_TX_DATA_FLAG: u32 = 0b1;
const MAX_JOB_TOKENS: usize = 64;
const TOKEN_LIFETIME: Duration = Duration::from_secs(120);
/// The least declared transaction memory a registry allows: twice the
/// 32 MB floor of Bitcoin Cash's block size limit, to the next MiB power.
const DECLARED_TRANSACTION_BYTES_FLOOR: usize = 64 * 1024 * 1024;
/// Blocks' worth of declared transactions a registry may hold, at the
/// published template's size limit, when that is above the floor.
const DECLARED_TEMPLATES: usize = 2;
/// Request ids one connection may hold large-template pages for, as it may
/// hold eight pending declarations.
const MAX_PAGED_DECLARATIONS: usize = 8;
/// Open tokens one connection may hold requested payout outputs for, as it
/// may hold eight pending declarations. A client asks once per job, just
/// before declaring it, so a ninth is not following the extension.
const MAX_PAYOUT_SETS: usize = 8;
/// `VerifyPayoutOutputs` answers one connection may compute in one second.
///
/// A client verifies a job once, and may retry a few `unknown-anchor`
/// answers while a share propagates, so four a second is a short burst
/// rather than a walk it can keep the node on. One more is answered
/// `verify-rate-limited` and the connection stays up. The handler does not
/// await, so a second verify cannot be in progress; one that re-entered
/// would get the same answer. Unlike [`MAX_PAYOUT_SETS`], this does not
/// close the connection.
const MAX_VERIFY_PAYOUTS_PER_SECOND: u32 = 4;
/// Full-Template declarations a connection keeps for `PushSolution`, all
/// on the latest template's previous block.
const MAX_SOLVABLE_JOBS: usize = 4;

/// Which Job Declaration mode a connection negotiated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobDeclarationMode {
    /// `DECLARE_TX_DATA` is clear. Only the declarator knows the transactions.
    CoinbaseOnly,
    /// `DECLARE_TX_DATA` is set. The client reveals every transaction.
    FullTemplate,
}

/// The latest prepared template, which Job Declaration tokens are allocated
/// against. The template's transactions stay in the template; nothing here
/// decodes them.
///
/// The lock is the only lock this cache takes.
#[derive(Clone)]
pub struct PoolTransactionCache {
    prepared: Arc<RwLock<Option<Arc<PreparedNotifyParams>>>>,
}

impl PoolTransactionCache {
    /// Empty cache. The notifier replaces it when a template arrives.
    pub fn new() -> Self {
        Self {
            prepared: Arc::new(RwLock::new(None)),
        }
    }

    /// Publish the immutable payout and template snapshot used by allocated tokens.
    pub(crate) fn set_prepared(&self, prepared: Arc<PreparedNotifyParams>) {
        *self.prepared.write().expect("pool prepared template lock") = Some(prepared);
    }

    /// Withdraw the template so new Job Declaration tokens cannot use stale work.
    pub(crate) fn clear_prepared(&self) {
        *self.prepared.write().expect("pool prepared template lock") = None;
    }

    /// The share new tokens are anchored on, while a template is published.
    fn anchor(&self) -> Option<BlockHash> {
        self.prepared
            .read()
            .expect("pool prepared template lock")
            .as_ref()
            .map(|prepared| prepared.prev_share_blockhash())
    }

    /// The size limit of the block the published template builds, in bytes.
    fn size_limit(&self) -> Option<u64> {
        self.prepared
            .read()
            .expect("pool prepared template lock")
            .as_ref()
            .map(|prepared| prepared.template().sizelimit)
    }

    /// What new tokens' payouts rest on, while a template is published
    /// (`payout_basis`).
    fn payout_basis(&self) -> Option<PayoutBasis> {
        self.prepared
            .read()
            .expect("pool prepared template lock")
            .as_deref()
            .map(payout_basis)
    }
}

/// The share and the BCH block a token's payouts rest on. A token allocated
/// on another pair answers `stale-payout-outputs`, so a change of either
/// sends `PayoutOutputsStale`.
type PayoutBasis = (BlockHash, String);

fn payout_basis(prepared: &PreparedNotifyParams) -> PayoutBasis {
    (
        prepared.prev_share_blockhash(),
        prepared.template().previousblockhash.clone(),
    )
}

impl Default for PoolTransactionCache {
    fn default() -> Self {
        Self::new()
    }
}

enum TokenState {
    /// Allocated, and not yet given its transactions.
    Open { mode: JobDeclarationMode },
    /// Full-template declaration waiting for the raw transactions.
    AwaitingTransactions {
        request_id: u32,
        wtxids: Vec<[u8; 32]>,
        declaration: Declaration,
        /// The transactions revealed so far, in declared order: the first
        /// `received.len()` of `wtxids`.
        received: Vec<Transaction>,
        /// How many transactions, from `received.len()`, the outstanding
        /// request asks for. There is one request at a time.
        requested: usize,
    },
    /// Transactions in CTOR, ready to mine.
    Ready {
        transactions: Arc<Vec<Transaction>>,
        declaration: Declaration,
    },
}

#[derive(Clone)]
pub(crate) struct Declaration {
    pub version: u32,
    pub prefix: Vec<u8>,
    pub suffix: Vec<u8>,
    /// The token leaves behind the declared coinbase's output 0, from
    /// `DeclareMiningJob.excess_data` (`declared_leaves`); none for the
    /// empty commitment.
    pub disclosure: Disclosure,
}

struct JobToken {
    allocated: Instant,
    prepared: Option<Arc<PreparedNotifyParams>>,
    /// Whom the token's requested payout outputs pay; `None` when its
    /// `user_identifier` names no payout address and share owner.
    identity: Option<PayoutIdentity>,
    state: TokenState,
}

/// A claimed declaration, removed from the registry before asynchronous validation.
pub(crate) struct DeclaredJob {
    pub prepared: Arc<PreparedNotifyParams>,
    pub transactions: Option<Arc<Vec<Transaction>>>,
    pub declaration: Option<Declaration>,
}

/// A Full-Template declaration a connection made, kept after
/// `SetCustomMiningJob` takes its token, so the block a `PushSolution`
/// reports can be rebuilt from it.
struct SolvableJob {
    /// The previous block of the template the token was allocated against,
    /// which every declared job builds on.
    prev_hash: BlockHash,
    /// That template's target. A pushed header must meet it, so a client
    /// cannot make the node submit blocks without network-difficulty work.
    target: Target,
    prefix: Vec<u8>,
    suffix: Vec<u8>,
    /// The declared transactions, in CTOR.
    transactions: Arc<Vec<Transaction>>,
    /// The coinbase's merkle branch over `transactions`.
    merkle_branches: Vec<TxMerkleNode>,
    /// This job's share of the kept-declaration budget, given back when it is
    /// dropped.
    _reserved: SolvableBytes,
}

/// Bytes reserved from a registry's budget for declarations kept for
/// `PushSolution`, given back on drop. The budget is as large as the
/// tokens' own (`JobRegistry::declared_byte_limit`): a kept declaration
/// outlives its token.
struct SolvableBytes {
    budget: Arc<AtomicUsize>,
    bytes: usize,
}

impl SolvableBytes {
    /// Reserve `bytes` from `budget`, or `None` when they would take it past
    /// `limit`. Callers reserve under the registry lock; releases may run at
    /// any time, and only make room.
    fn reserve(budget: &Arc<AtomicUsize>, bytes: usize, limit: usize) -> Option<Self> {
        let used = budget.fetch_add(bytes, Ordering::SeqCst);
        if used.saturating_add(bytes) > limit {
            budget.fetch_sub(bytes, Ordering::SeqCst);
            return None;
        }
        Some(Self {
            budget: Arc::clone(budget),
            bytes,
        })
    }
}

impl Drop for SolvableBytes {
    fn drop(&mut self) {
        self.budget.fetch_sub(self.bytes, Ordering::SeqCst);
    }
}

impl SolvableJob {
    /// The job for `declaration` and `transactions` on `prepared`'s
    /// template, or `None` when the template's previous block or bits do
    /// not parse, or `budget` has no room for it within `limit` bytes.
    fn new(
        prepared: &PreparedNotifyParams,
        declaration: &Declaration,
        transactions: Arc<Vec<Transaction>>,
        budget: &Arc<AtomicUsize>,
        limit: usize,
    ) -> Option<Self> {
        let template = prepared.template();
        let prev_hash = template.previousblockhash.parse().ok()?;
        let bits = CompactTarget::from_unprefixed_hex(&template.bits).ok()?;
        let bytes = transactions
            .iter()
            .map(Transaction::total_size)
            .sum::<usize>()
            + declaration.prefix.len()
            + declaration.suffix.len();
        let reserved = SolvableBytes::reserve(budget, bytes, limit)?;
        let txids = transactions
            .iter()
            .map(|transaction| transaction.compute_txid().to_raw_hash())
            .collect();
        let merkle_branches = compute_merkle_branches(txids)
            .into_iter()
            .map(TxMerkleNode::from_raw_hash)
            .collect();
        Some(Self {
            prev_hash,
            target: Target::from_compact(bits),
            prefix: declaration.prefix.clone(),
            suffix: declaration.suffix.clone(),
            transactions,
            merkle_branches,
            _reserved: reserved,
        })
    }

    /// The block `push` reports for this job, when its coinbase decodes and
    /// its header meets the template's target.
    fn block(&self, push: &PushSolution) -> Option<Block> {
        let extranonce = push.extranonce.as_bytes();
        let mut raw = Vec::with_capacity(self.prefix.len() + extranonce.len() + self.suffix.len());
        raw.extend_from_slice(&self.prefix);
        raw.extend_from_slice(extranonce);
        raw.extend_from_slice(&self.suffix);
        let coinbase: Transaction = deserialize(&raw).ok()?;
        let header = Header {
            version: Version::from_consensus(push.version as i32),
            prev_blockhash: self.prev_hash,
            merkle_root: compute_merkle_root_from_branches(
                coinbase.compute_txid(),
                &self.merkle_branches,
            ),
            time: push.ntime,
            bits: CompactTarget::from_consensus(push.nbits),
            nonce: push.nonce,
        };
        if !self.target.is_met_by(header.block_hash()) {
            return None;
        }
        let mut txdata = Vec::with_capacity(self.transactions.len() + 1);
        txdata.push(coinbase);
        txdata.extend(self.transactions.iter().cloned());
        Some(Block { header, txdata })
    }
}

/// Tokens and the transaction set each one mines.
pub struct JobRegistry {
    cache: PoolTransactionCache,
    merge_mining: Option<MergeMiningCommitment>,
    tokens: HashMap<Vec<u8>, JobToken>,
    /// Client addresses Coinbase-Only declarations are taken from; `None`
    /// takes them from every client.
    coinbase_only_from: Option<Vec<IpNet>>,
    /// Bytes of declarations the connections keep for `PushSolution`.
    solvable_bytes: Arc<AtomicUsize>,
}

impl JobRegistry {
    /// Registry for `cache`. `merge_mining` is included in every allocated
    /// token's coinbase outputs. `None` leaves that field empty, which is
    /// what a Bitcoin node sends.
    pub fn new(cache: PoolTransactionCache, merge_mining: Option<MergeMiningCommitment>) -> Self {
        Self {
            cache,
            merge_mining,
            tokens: HashMap::with_capacity(8),
            coinbase_only_from: None,
            solvable_bytes: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Take Coinbase-Only declarations only from clients in `networks`.
    /// A Coinbase-Only declarator's shares are credited on transactions the
    /// node never sees, so a node offers it only to clients it trusts.
    pub(crate) fn with_coinbase_only_from(mut self, networks: Vec<IpNet>) -> Self {
        self.coinbase_only_from = Some(networks);
        self
    }

    /// Whether a client at `peer` may declare Coinbase-Only jobs. An
    /// IPv4-mapped IPv6 address counts as its IPv4 address. Without a known
    /// address only a node with no allowlist takes it.
    fn takes_coinbase_only_from(&self, peer: Option<IpAddr>) -> bool {
        match (&self.coinbase_only_from, peer) {
            (None, _) => true,
            (Some(networks), Some(peer)) => {
                let peer = peer.to_canonical();
                networks.iter().any(|network| network.contains(&peer))
            }
            (Some(_), None) => false,
        }
    }

    /// Transactions a ready token will mine, in CTOR.
    pub fn transactions_for_token(&self, token: &[u8]) -> Option<Vec<Transaction>> {
        match self.tokens.get(token).map(|token| &token.state) {
            Some(TokenState::Ready { transactions, .. }) => Some(transactions.as_ref().clone()),
            _ => None,
        }
    }

    fn expire_tokens(&mut self) {
        self.tokens
            .retain(|_, token| token.allocated.elapsed() < TOKEN_LIFETIME);
    }

    /// `token` while it is open: allocated, and neither declared nor used.
    fn open_token(&self, token: &[u8]) -> Option<&JobToken> {
        self.tokens
            .get(token)
            .filter(|token| matches!(token.state, TokenState::Open { .. }))
    }

    /// Claim a token exactly once; an invalid custom job needs a new allocation.
    pub(crate) fn take_job(&mut self, token: &[u8]) -> Result<DeclaredJob, String> {
        self.expire_tokens();
        let token = self
            .tokens
            .remove(token)
            .ok_or("invalid-mining-job-token")?;
        let prepared = token.prepared.ok_or("template-unavailable")?;
        match token.state {
            TokenState::Open {
                mode: JobDeclarationMode::CoinbaseOnly,
            } => Ok(DeclaredJob {
                prepared,
                transactions: None,
                declaration: None,
            }),
            TokenState::Ready {
                transactions,
                declaration,
            } => Ok(DeclaredJob {
                prepared,
                transactions: Some(transactions),
                declaration: Some(declaration),
            }),
            _ => Err("invalid-mining-job-token".into()),
        }
    }

    fn declared_bytes(&self) -> usize {
        self.tokens
            .values()
            .map(|token| match &token.state {
                TokenState::Ready {
                    transactions,
                    declaration,
                } => {
                    transactions
                        .iter()
                        .map(Transaction::total_size)
                        .sum::<usize>()
                        + declaration.prefix.len()
                        + declaration.suffix.len()
                }
                TokenState::AwaitingTransactions {
                    wtxids,
                    declaration,
                    received,
                    ..
                } => {
                    wtxids.len() * 32
                        + declaration.prefix.len()
                        + declaration.suffix.len()
                        + received.iter().map(Transaction::total_size).sum::<usize>()
                }
                _ => 0,
            })
            .sum()
    }

    /// The declared transaction memory this registry allows:
    /// `DECLARED_TEMPLATES` blocks at the published template's size limit,
    /// and at least `DECLARED_TRANSACTION_BYTES_FLOOR`. It follows the block
    /// size limit, as ABLA raises it.
    fn declared_byte_limit(&self) -> usize {
        let blocks = self
            .cache
            .size_limit()
            .and_then(|limit| usize::try_from(limit).ok())
            .map_or(0, |limit| limit.saturating_mul(DECLARED_TEMPLATES));
        blocks.max(DECLARED_TRANSACTION_BYTES_FLOOR)
    }
}

/// Bytes of one Stratum V2 connection.
pub struct Sv2Connection {
    registry: Arc<Mutex<JobRegistry>>,
    decoder: Decoder,
    inbox: Vec<u8>,
    mode: Option<JobDeclarationMode>,
    mining: bool,
    work_selection: bool,
    /// The client's address, which decides whether it may declare
    /// Coinbase-Only jobs.
    peer: Option<IpAddr>,
    /// Full-Template declarations made on this connection that still wait
    /// for `ProvideMissingTransactionsSuccess`, by the client's request id.
    /// Request ids are chosen by each client, so two connections can use the
    /// same one; the token is looked up here, never across the registry.
    pending_declarations: HashMap<u32, Vec<u8>>,
    /// The connection negotiated the requested payout outputs extension
    /// (`0x5052`, `super::payout_outputs`).
    payout_outputs: bool,
    /// The share and BCH block new tokens rested on when this connection was
    /// last told, by the grant or by `PayoutOutputsStale`.
    payout_basis: Option<PayoutBasis>,
    /// Open tokens this connection was given payout outputs for since then,
    /// at most [`MAX_PAYOUT_SETS`].
    payout_sets: HashSet<Vec<u8>>,
    /// `VerifyPayoutOutputs` answers admitted since [`Self::verify_window`].
    verify_count: u32,
    /// Start of the one-second verify budget.
    verify_window: Instant,
    /// A verify is inside [`Self::handle_verify_payout_outputs`].
    verify_outstanding: bool,
    /// This connection's latest Full-Template declarations, newest last,
    /// for `PushSolution`.
    solvable_jobs: VecDeque<SolvableJob>,
    /// The connection negotiated the large templates extension (`0x4C54`,
    /// `super::large_templates`).
    large_templates: bool,
    /// Transaction ids paged with `DeclareMiningJob.Transactions` ahead of
    /// their `DeclareMiningJob`, by request id.
    declared_pages: HashMap<u32, Vec<[u8; 32]>>,
}

impl Sv2Connection {
    /// Connection that shares `registry` with the node's other SV2 sessions.
    pub fn new(registry: Arc<Mutex<JobRegistry>>) -> Self {
        Self {
            registry,
            decoder: Decoder::new(),
            inbox: Vec::new(),
            mode: None,
            mining: false,
            work_selection: false,
            peer: None,
            pending_declarations: HashMap::with_capacity(4),
            payout_outputs: false,
            payout_basis: None,
            payout_sets: HashSet::with_capacity(MAX_PAYOUT_SETS),
            verify_count: 0,
            verify_window: Instant::now(),
            verify_outstanding: false,
            solvable_jobs: VecDeque::with_capacity(MAX_SOLVABLE_JOBS),
            large_templates: false,
            declared_pages: HashMap::new(),
        }
    }

    /// The connection of a client at `peer`.
    pub(crate) fn with_peer(mut self, peer: IpAddr) -> Self {
        self.peer = Some(peer);
        self
    }

    /// Accept `incoming` and return the response frames it completed.
    ///
    /// Every message goes to [`Self::handle_message`]. The listener decodes
    /// with [`Self::decode`] instead, so it can route Mining Protocol channel
    /// messages to [`super::mining::MiningChannels`].
    pub fn accept(&mut self, incoming: &[u8]) -> Result<Vec<Vec<u8>>, String> {
        let messages = self.decode(incoming)?;
        let mut responses = Vec::with_capacity(messages.len());
        for mut message in messages {
            if message.extension_type != 0 {
                return Err("unsupported Stratum V2 extension".to_string());
            }
            responses.extend(self.handle_message(message.message_type, &mut message.payload)?);
        }
        Ok(responses)
    }

    /// Decode `incoming` into complete messages.
    pub fn decode(&mut self, incoming: &[u8]) -> Result<Vec<Sv2Message>, String> {
        decode_plain_frames(&mut self.decoder, &mut self.inbox, incoming)
    }

    /// Whether this connection has set up a protocol, as a request for
    /// extensions needs.
    pub fn is_set_up(&self) -> bool {
        self.mining || self.mode.is_some()
    }

    /// Whether this connection set up the Mining Protocol.
    pub fn is_mining(&self) -> bool {
        self.mining
    }

    pub(crate) fn can_select_work(&self) -> bool {
        self.mining && self.work_selection
    }

    pub(crate) fn take_declared_job(&self, token: &[u8]) -> Result<DeclaredJob, String> {
        self.registry
            .lock()
            .expect("job registry lock")
            .take_job(token)
    }

    /// Grant the large templates extension (`0x4C54`) to a Full-Template Job
    /// Declaration connection on a Bitcoin Cash node, whose block size limit
    /// standard Job Declaration cannot carry. Returns whether it was
    /// granted.
    pub(crate) fn grant_large_templates(&mut self) -> bool {
        let bitcoin_cash = self
            .registry
            .lock()
            .expect("job registry lock")
            .merge_mining
            .is_some();
        if self.mode != Some(JobDeclarationMode::FullTemplate) || !bitcoin_cash {
            return false;
        }
        self.large_templates = true;
        true
    }

    /// Whether this connection negotiated the large templates extension.
    pub(crate) fn has_large_templates(&self) -> bool {
        self.large_templates
    }

    /// Take a large templates message on a connection that negotiated the
    /// extension. A client sends only `DeclareMiningJob.Transactions`, which
    /// has no answer; any other message of the extension closes the
    /// connection, and so does a page past the bounds.
    pub(crate) fn handle_large_templates(
        &mut self,
        message_type: u8,
        payload: &mut [u8],
    ) -> Result<Vec<Vec<u8>>, String> {
        if message_type != DECLARE_MINING_JOB_TRANSACTIONS {
            return Err(format!(
                "unsupported large templates message type {message_type}"
            ));
        }
        let page = DeclareMiningJobTransactions::from_bytes(payload)
            .map_err(|error| format!("{error:?}"))?;
        self.on_transactions_page(&page)?;
        Ok(Vec::new())
    }

    /// Buffer a page of declared transaction ids for the `DeclareMiningJob`
    /// with the same request id. A page for a request id already declared,
    /// for a ninth request id, or that takes the list past the most
    /// transactions a block at the published template's size limit can
    /// hold, closes the connection.
    fn on_transactions_page(&mut self, page: &DeclareMiningJobTransactions) -> Result<(), String> {
        if self.pending_declarations.contains_key(&page.request_id) {
            return Err("transaction page after its DeclareMiningJob".to_string());
        }
        let size_limit = self
            .registry
            .lock()
            .expect("job registry lock")
            .cache
            .size_limit()
            .ok_or("transaction page with no template")?;
        if !self.declared_pages.contains_key(&page.request_id)
            && self.declared_pages.len() >= MAX_PAGED_DECLARATIONS
        {
            return Err("transaction page limit reached".to_string());
        }
        let ids = self.declared_pages.entry(page.request_id).or_default();
        if ids.len() + page.txid_list.len() > max_declared_transactions(size_limit) {
            return Err("declared transactions past the block size limit".to_string());
        }
        ids.extend(page.txid_list.iter().map(|txid| {
            let mut bytes = [0u8; 32];
            bytes.copy_from_slice(txid.as_bytes());
            bytes
        }));
        Ok(())
    }

    /// The request for the `count` declared transactions from `start` of
    /// the declaration `request_id`: the core `ProvideMissingTransactions`
    /// when every position fits a `u16`, else
    /// `ProvideMissingTransactions.Positions`.
    fn missing_transactions_request(
        &mut self,
        request_id: u32,
        start: usize,
        count: usize,
    ) -> Result<Vec<Vec<u8>>, String> {
        let end = start + count;
        if end <= usize::from(u16::MAX) + 1 {
            let positions = (start..end)
                .map(|position| u16::try_from(position).expect("checked above"))
                .collect::<Vec<_>>();
            return self.respond(
                ProvideMissingTransactions {
                    request_id,
                    unknown_tx_position_list: Seq064K::new(positions)
                        .map_err(|error| error.to_string())?,
                },
                MESSAGE_TYPE_PROVIDE_MISSING_TRANSACTIONS,
            );
        }
        Ok(vec![positions_frame(request_id, start, count)?])
    }

    /// Grant the requested payout outputs extension (`0x5052`) to a Job
    /// Declaration connection on a Bitcoin Cash node, the only one whose
    /// tokens carry P2Pool job terms. Returns whether it was granted.
    pub(crate) fn grant_payout_outputs(&mut self) -> bool {
        let registry = self.registry.lock().expect("job registry lock");
        if self.mode.is_none() || registry.merge_mining.is_none() {
            return false;
        }
        self.payout_basis = registry.cache.payout_basis();
        drop(registry);
        self.payout_outputs = true;
        true
    }

    /// Whether this connection negotiated the requested payout outputs.
    pub(crate) fn has_payout_outputs(&self) -> bool {
        self.payout_outputs
    }

    /// Answer a requested payout outputs message on a connection that
    /// negotiated the extension. A client sends `RequestPayoutOutputs`;
    /// `VerifyPayoutOutputs` is [`Self::handle_verify_payout_outputs`]. Any
    /// other message of the extension closes the connection, and so does
    /// a request for a token past [`MAX_PAYOUT_SETS`], as a declaration past
    /// its limit does. Every other refusal is a `RequestPayoutOutputs.Error`.
    pub(crate) fn handle_payout_outputs(
        &mut self,
        message_type: u8,
        payload: &mut [u8],
    ) -> Result<Vec<Vec<u8>>, String> {
        if message_type != REQUEST_PAYOUT_OUTPUTS {
            return Err(format!(
                "unsupported requested payout outputs message type {message_type}"
            ));
        }
        let request =
            RequestPayoutOutputs::from_bytes(payload).map_err(|error| error.to_string())?;
        let token = request.mining_job_token.as_bytes().to_vec();
        let mut registry = self.registry.lock().expect("job registry lock");
        registry.expire_tokens();
        self.payout_sets
            .retain(|token| registry.open_token(token).is_some());
        if self.payout_sets.len() >= MAX_PAYOUT_SETS && !self.payout_sets.contains(&token) {
            return Err("payout output request limit reached".into());
        }
        let terms = registry
            .open_token(&token)
            .map(|open| (open.prepared.clone(), open.identity.clone()));
        let anchor = registry.cache.anchor();
        drop(registry);
        // The build runs outside the registry lock: a large window's payouts
        // must not hold up other connections' tokens.
        let answer = match terms {
            None => Err(INVALID_MINING_JOB_TOKEN),
            Some((None, _)) => Err(PAYOUT_OUTPUTS_UNAVAILABLE),
            Some((Some(prepared), identity)) => {
                prepare_payout_outputs(&request, &prepared, identity.as_ref(), anchor)
            }
        };
        if answer.is_ok() {
            self.payout_sets.insert(token);
        }
        Ok(vec![answer_frame(&request, answer)?])
    }

    /// Answer `VerifyPayoutOutputs` (`0x04`).
    ///
    /// The token is another node's. Nothing is allocated and the job
    /// registry is not taken. `identity_of` resolves `user_identifier` the
    /// way `AllocateMiningJobToken` does. `weights` is this node's window;
    /// `None` (a node that does not pay from the share chain) is
    /// `payout-outputs-unavailable`. A malformed payload still closes the
    /// connection. A verify past [`MAX_VERIFY_PAYOUTS_PER_SECOND`], or one
    /// started while another is in this function, is `verify-rate-limited`
    /// and does not close the connection.
    pub(crate) fn handle_verify_payout_outputs(
        &mut self,
        payload: &mut [u8],
        identity_of: impl Fn(&str) -> Option<PayoutIdentity>,
        weights: Option<&dyn AnchorWeights>,
    ) -> Result<Vec<Vec<u8>>, String> {
        let request =
            VerifyPayoutOutputs::from_bytes(payload).map_err(|error| error.to_string())?;
        if !self.admit_verify() {
            return Ok(vec![verify_answer_frame(
                request.request_id,
                request.height,
                Err(VERIFY_RATE_LIMITED),
            )?]);
        }
        let identity = identity_of(&request.user_identifier.as_utf8_or_hex());
        let answer = verify_payout_outputs(&request, identity.as_ref(), weights);
        self.verify_outstanding = false;
        Ok(vec![verify_answer_frame(
            request.request_id,
            request.height,
            answer,
        )?])
    }

    /// Whether this connection may run a verify now.
    ///
    /// Admits at most one at a time and at most
    /// [`MAX_VERIFY_PAYOUTS_PER_SECOND`] in the current second. An admission
    /// sets [`Self::verify_outstanding`]; the caller clears it when the
    /// verify returns. A refusal spends nothing.
    fn admit_verify(&mut self) -> bool {
        if self.verify_outstanding {
            return false;
        }
        let now = Instant::now();
        if now.duration_since(self.verify_window) >= Duration::from_secs(1) {
            self.verify_window = now;
            self.verify_count = 0;
        }
        if self.verify_count >= MAX_VERIFY_PAYOUTS_PER_SECOND {
            return false;
        }
        self.verify_count += 1;
        self.verify_outstanding = true;
        true
    }

    /// What a newly published template sends on this connection:
    /// `PayoutOutputsStale` when it negotiated the requested payout outputs
    /// and new tokens now rest on another share or BCH block. The sets given
    /// for earlier tokens are stale from then on, and no longer count towards
    /// [`MAX_PAYOUT_SETS`].
    pub(crate) fn on_template(
        &mut self,
        prepared: &PreparedNotifyParams,
    ) -> Result<Vec<Vec<u8>>, String> {
        let basis = Some(payout_basis(prepared));
        if !self.payout_outputs || self.payout_basis == basis {
            return Ok(Vec::new());
        }
        self.payout_basis = basis;
        self.payout_sets.clear();
        Ok(vec![stale_frame()?])
    }

    /// Handle one setup, Job Declaration, or custom-job message.
    pub fn handle_message(
        &mut self,
        message_type: u8,
        payload: &mut [u8],
    ) -> Result<Vec<Vec<u8>>, String> {
        match message_type {
            MESSAGE_TYPE_SETUP_CONNECTION => {
                let setup =
                    SetupConnection::from_bytes(payload).map_err(|error| error.to_string())?;
                self.on_setup(&setup)
            }
            MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN => {
                let request = AllocateMiningJobToken::from_bytes(payload)
                    .map_err(|error| error.to_string())?;
                self.on_allocate(
                    request.request_id,
                    &request.user_identifier.as_utf8_or_hex(),
                    None,
                )
            }
            MESSAGE_TYPE_DECLARE_MINING_JOB => {
                let request =
                    DeclareMiningJob::from_bytes(payload).map_err(|error| error.to_string())?;
                self.on_declare(&request)
            }
            MESSAGE_TYPE_PROVIDE_MISSING_TRANSACTIONS_SUCCESS => {
                let request = ProvideMissingTransactionsSuccess::from_bytes(payload)
                    .map_err(|error| error.to_string())?;
                self.on_transactions(&request)
            }
            MESSAGE_TYPE_SET_CUSTOM_MINING_JOB => {
                let request =
                    SetCustomMiningJob::from_bytes(payload).map_err(|error| error.to_string())?;
                self.on_custom_job(&request)
            }
            // The listener submits the block (`server::handle_messages`);
            // here it is only rebuilt.
            MESSAGE_TYPE_PUSH_SOLUTION => self.push_solution(payload).map(|_| Vec::new()),
            _ => Err(format!(
                "unsupported Stratum V2 message type {message_type}"
            )),
        }
    }

    fn on_setup(&mut self, setup: &SetupConnection) -> Result<Vec<Vec<u8>>, String> {
        let version = setup.get_version(PROTOCOL_VERSION, PROTOCOL_VERSION);
        let Some(used_version) = version else {
            return self.respond(
                SetupConnectionError {
                    flags: 0,
                    error_code: string_field("protocol-version-mismatch")?,
                },
                MESSAGE_TYPE_SETUP_CONNECTION_ERROR,
            );
        };
        match setup.protocol {
            Protocol::JobDeclarationProtocol => {
                let mode = if has_declare_tx_data(setup.flags) {
                    JobDeclarationMode::FullTemplate
                } else {
                    JobDeclarationMode::CoinbaseOnly
                };
                // Coinbase-Only only from clients the operator lists; the
                // error names the flag the client must set (sv2-spec 3.x).
                if mode == JobDeclarationMode::CoinbaseOnly
                    && !self
                        .registry
                        .lock()
                        .expect("job registry lock")
                        .takes_coinbase_only_from(self.peer)
                {
                    self.mode = None;
                    self.mining = false;
                    return self.respond(
                        SetupConnectionError {
                            flags: DECLARE_TX_DATA_FLAG,
                            error_code: string_field("unsupported-feature-flags")?,
                        },
                        MESSAGE_TYPE_SETUP_CONNECTION_ERROR,
                    );
                }
                self.mode = Some(mode);
                self.mining = false;
                self.work_selection = false;
                self.respond(
                    SetupConnectionSuccess {
                        used_version,
                        flags: 0,
                    },
                    MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS,
                )
            }
            Protocol::MiningProtocol => {
                if has_work_selection(setup.flags)
                    && self
                        .registry
                        .lock()
                        .expect("job registry lock")
                        .merge_mining
                        .is_none()
                {
                    self.mining = false;
                    self.mode = None;
                    return self.respond(
                        SetupConnectionError {
                            flags: WORK_SELECTION_FLAG,
                            error_code: string_field("unsupported-feature-flags")?,
                        },
                        MESSAGE_TYPE_SETUP_CONNECTION_ERROR,
                    );
                }
                self.mining = true;
                self.work_selection = has_work_selection(setup.flags);
                self.mode = None;
                self.payout_outputs = false;
                self.respond(
                    SetupConnectionSuccess {
                        used_version,
                        flags: 0,
                    },
                    MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS,
                )
            }
            Protocol::TemplateDistributionProtocol => self.respond(
                SetupConnectionError {
                    flags: 0,
                    error_code: string_field("unsupported-protocol")?,
                },
                MESSAGE_TYPE_SETUP_CONNECTION_ERROR,
            ),
        }
    }

    /// Allocate a token whose terms carry the donation `user_identifier`
    /// chooses with `donation=`, as a mining channel's identity does, or the
    /// node's own without one. The channel that later mines the job must
    /// choose the same donation (`MiningChannels::install_custom_job`).
    ///
    /// The token remembers `identity`, the payout address and share owner
    /// `user_identifier` names as a mining channel's identity, which its
    /// requested payout outputs pay (`super::payout_outputs`). Without one
    /// the token is allocated all the same, and a request for its outputs
    /// is answered `invalid-request`.
    ///
    /// The terms carry the payout level `user_identifier` chooses with
    /// `min-payout=` or `fee-tolerance=`, or the node's own without one. A
    /// declared job's level is not lowered by the node's level policy: the
    /// node knows the declarator's hashrate only once it mines.
    ///
    /// The Job Declaration Protocol has no error reply to this message, so a
    /// bad `donation=`, `min-payout=` or `fee-tolerance=` value, or one on a
    /// node without a Bitcoin Cash share chain, closes the connection with
    /// the reason in the node's log.
    pub(crate) fn on_allocate(
        &mut self,
        request_id: u32,
        user_identifier: &str,
        identity: Option<PayoutIdentity>,
    ) -> Result<Vec<Vec<u8>>, String> {
        let mode = self.mode.ok_or("job token requested before setup")?;
        let (_, options) = split_user_identity(user_identifier);
        if let Some(options) = options {
            log_unknown_options(options);
        }
        let donation = parse_miner_donation(options)?;
        let payout_level = parse_miner_payout_level(options)?;
        let mut registry = self.registry.lock().expect("job registry lock");
        registry.expire_tokens();
        if registry.tokens.len() >= MAX_JOB_TOKENS {
            return Err("job token limit reached".into());
        }
        if donation.is_some() && registry.merge_mining.is_none() {
            return Err("donation= is a Bitcoin Cash P2Pool option this node cannot honour".into());
        }
        if payout_level.is_some() && registry.merge_mining.is_none() {
            return Err(
                "min-payout= and fee-tolerance= are Bitcoin Cash P2Pool options this node \
                 cannot honour"
                    .into(),
            );
        }
        let mut prepared = registry
            .cache
            .prepared
            .read()
            .expect("pool prepared template lock")
            .clone();
        if let Some(template) = &prepared
            && let Some(miner_prepared) = template
                .for_miner_donation(donation)
                .map_err(|error| error.to_string())?
        {
            prepared = Some(miner_prepared);
        }
        let (token, coinbase_outputs) = if registry.merge_mining.is_some() {
            let prepared = prepared.as_ref().ok_or("template-unavailable")?;
            // The payout scripts of a declared job carrying the exact empty
            // commitment; a token commitment's job may pay more of them
            // through the payout tree.
            let payouts = prepared
                .declared_outputs(
                    prepared.template().coinbasevalue,
                    &MergeMiningCommitment::empty(),
                )
                .map_err(|error| error.to_string())?;
            let tree_script = payouts.tree_output().map(|output| output.script_pubkey);
            let mut outputs = payouts
                .outputs
                .iter()
                .map(|pair| pair.address.script_pubkey())
                .chain(tree_script)
                .map(|script_pubkey| TxOut {
                    value: bitcoin::Amount::ZERO,
                    script_pubkey,
                })
                .collect::<Vec<_>>();
            if self.payout_outputs {
                // The requested payout outputs answer the whole output list,
                // so its reservation holds output 0, the padding and the
                // commitment as well, each at its largest.
                let (output_zero, tail) = declared_reservation_outputs();
                outputs.insert(0, output_zero);
                outputs.extend(tail);
            }
            let level = payout_level.map_or(prepared.payout_level(), |choice| {
                choice.level(prepared.spend_cost())
            });
            let token = prepared
                .mining_job_token_at_level(
                    random(),
                    SystemTimeProvider.seconds_since_epoch() as u32,
                    level,
                )
                .map_err(|error| error.to_string())?;
            (token, serialize(&outputs))
        } else {
            (random::<[u8; 32]>().to_vec(), empty_output_list())
        };
        registry.tokens.insert(
            token.clone(),
            JobToken {
                allocated: Instant::now(),
                prepared,
                identity,
                state: TokenState::Open { mode },
            },
        );
        drop(registry);
        self.respond(
            AllocateMiningJobTokenSuccess {
                request_id,
                mining_job_token: bytes_255(&token)?,
                coinbase_outputs: bytes_field(&coinbase_outputs)?,
            },
            MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN_SUCCESS,
        )
    }

    fn on_declare(&mut self, request: &DeclareMiningJob) -> Result<Vec<Vec<u8>>, String> {
        // Pages of a large template come before their DeclareMiningJob and
        // end with it, whatever the answer.
        let paged = self
            .declared_pages
            .remove(&request.request_id)
            .unwrap_or_default();
        if self.mode != Some(JobDeclarationMode::FullTemplate) {
            return self.declare_error(
                request.request_id,
                ERROR_CODE_DECLARE_MINING_JOB_INVALID_JOB,
            );
        }
        let token = request.mining_job_token.as_bytes().to_vec();
        let mut registry = self.registry.lock().expect("job registry lock");
        registry.expire_tokens();
        self.pending_declarations
            .retain(|_, token| registry.tokens.contains_key(token));
        if self.pending_declarations.contains_key(&request.request_id) {
            drop(registry);
            return self.declare_error(
                request.request_id,
                ERROR_CODE_DECLARE_MINING_JOB_INVALID_JOB,
            );
        }
        let Some(job_token) = registry.tokens.get(&token).filter(|token| {
            matches!(
                token.state,
                TokenState::Open {
                    mode: JobDeclarationMode::FullTemplate
                }
            )
        }) else {
            drop(registry);
            return self.declare_error(
                request.request_id,
                ERROR_CODE_DECLARE_MINING_JOB_INVALID_MINING_JOB_TOKEN,
            );
        };
        let size_limit = job_token
            .prepared
            .as_ref()
            .map(|prepared| prepared.template().sizelimit);
        // A Bitcoin Cash declaration discloses the token leaves behind its
        // output 0 in excess_data, so the node can check, when the job is
        // installed, that they pay its fee and the miner's donation.
        let disclosure = if registry.merge_mining.is_some() {
            match declared_leaves(
                request.coinbase_tx_suffix.as_bytes(),
                request.excess_data.as_bytes(),
            ) {
                Ok(disclosure) => disclosure,
                Err(reason) => {
                    drop(registry);
                    return self.declare_error_with_details(
                        request.request_id,
                        INVALID_COINBASE_TX,
                        reason.as_bytes(),
                    );
                }
            }
        } else {
            Disclosure::default()
        };
        let mut wtxids = paged;
        wtxids.extend(request.wtxid_list.iter().map(|wtxid| {
            let mut bytes = [0u8; 32];
            bytes.copy_from_slice(wtxid.as_bytes());
            bytes
        }));
        if size_limit.is_some_and(|limit| wtxids.len() > max_declared_transactions(limit)) {
            return Err("declared transactions past the block size limit".into());
        }
        if self.pending_declarations.len() >= 8
            || registry.declared_bytes()
                + wtxids.len() * 32
                + request.coinbase_tx_prefix.as_bytes().len()
                + request.coinbase_tx_suffix.as_bytes().len()
                > registry.declared_byte_limit()
        {
            return Err("job declaration limit reached".into());
        }
        // The node holds no transactions of its own, so it asks for all of
        // them, a batch at a time.
        let first = wtxids.len().min(MAX_IDS_PER_MESSAGE);
        registry
            .tokens
            .get_mut(&token)
            .expect("token checked above")
            .state = TokenState::AwaitingTransactions {
            request_id: request.request_id,
            wtxids,
            declaration: Declaration {
                version: request.version,
                prefix: request.coinbase_tx_prefix.as_bytes().to_vec(),
                suffix: request.coinbase_tx_suffix.as_bytes().to_vec(),
                disclosure,
            },
            received: Vec::new(),
            requested: first,
        };
        drop(registry);
        self.pending_declarations.insert(request.request_id, token);
        self.missing_transactions_request(request.request_id, 0, first)
    }

    /// Take a `ProvideMissingTransactions.Success`.
    ///
    /// It must hold the transactions the outstanding request asked for, in
    /// order. On a connection that negotiated large templates it may hold
    /// only a prefix of them, the longest that fit one frame, but at least
    /// one; the node then asks for the rest. Once every declared
    /// transaction has arrived, they are sorted into CTOR and the token is
    /// ready.
    fn on_transactions(
        &mut self,
        request: &ProvideMissingTransactionsSuccess,
    ) -> Result<Vec<Vec<u8>>, String> {
        let token = self
            .pending_declarations
            .remove(&request.request_id)
            .ok_or("transactions arrived with no open declaration")?;
        let large_templates = self.large_templates;
        let mut registry = self.registry.lock().expect("job registry lock");
        registry.expire_tokens();
        let byte_limit = registry.declared_byte_limit();
        let declared_elsewhere = registry.declared_bytes();
        let job_token = registry
            .tokens
            .get_mut(&token)
            .ok_or("transactions arrived with no open declaration")?;
        let size_limit = job_token
            .prepared
            .as_ref()
            .map(|prepared| prepared.template().sizelimit);
        let TokenState::AwaitingTransactions {
            request_id,
            wtxids,
            received,
            requested,
            ..
        } = &mut job_token.state
        else {
            return Err("transactions arrived with no open declaration".to_string());
        };
        if *request_id != request.request_id {
            return Err("transactions arrived with no open declaration".to_string());
        }
        let count = request.transaction_list.len();
        let partial = count < *requested;
        if count > *requested || (partial && (!large_templates || count == 0)) {
            return Err("transaction list length does not match the request".to_string());
        }
        let mut revealed_bytes = 0usize;
        for raw in request.transaction_list.iter() {
            let transaction: Transaction =
                deserialize(raw.as_bytes()).map_err(|error| error.to_string())?;
            let wtxid = transaction.compute_wtxid().to_byte_array();
            if wtxid != wtxids[received.len()] {
                return Err("revealed transaction does not match its declared wtxid".to_string());
            }
            revealed_bytes += transaction.total_size();
            received.push(transaction);
        }
        let received_bytes = received.iter().map(Transaction::total_size).sum::<usize>();
        if declared_elsewhere + revealed_bytes > byte_limit
            || size_limit
                .and_then(|limit| usize::try_from(limit).ok())
                .is_some_and(|limit| received_bytes > limit)
        {
            return Err("declared transaction memory limit reached".into());
        }
        if received.len() < wtxids.len() {
            let start = received.len();
            let next = (wtxids.len() - start).min(MAX_IDS_PER_MESSAGE);
            *requested = next;
            let request_id = *request_id;
            drop(registry);
            self.pending_declarations.insert(request_id, token);
            return self.missing_transactions_request(request_id, start, next);
        }
        let mut job_token = registry
            .tokens
            .remove(&token)
            .ok_or("expired-mining-job-token")?;
        let TokenState::AwaitingTransactions {
            received: mut transactions,
            declaration,
            ..
        } = std::mem::replace(
            &mut job_token.state,
            TokenState::Open {
                mode: JobDeclarationMode::FullTemplate,
            },
        )
        else {
            unreachable!("the token was checked to await transactions above");
        };
        sort_ctor(&mut transactions);
        let mut new_token_bytes = token;
        if registry.merge_mining.is_some() {
            new_token_bytes[1..33].copy_from_slice(&random::<[u8; 32]>());
        } else {
            new_token_bytes = random::<[u8; 32]>().to_vec();
        }
        let transactions = Arc::new(transactions);
        let solvable_limit = registry.declared_byte_limit();
        let solvable = job_token.prepared.as_deref().and_then(|prepared| {
            SolvableJob::new(
                prepared,
                &declaration,
                Arc::clone(&transactions),
                &registry.solvable_bytes,
                solvable_limit,
            )
        });
        job_token.state = TokenState::Ready {
            transactions,
            declaration,
        };
        registry.tokens.insert(new_token_bytes.clone(), job_token);
        drop(registry);
        if let Some(job) = solvable {
            self.keep_solvable(job);
        }
        self.respond(
            DeclareMiningJobSuccess {
                request_id: request.request_id,
                new_mining_job_token: bytes_255(&new_token_bytes)?,
            },
            MESSAGE_TYPE_DECLARE_MINING_JOB_SUCCESS,
        )
    }

    /// Keep `job` for `PushSolution`: only jobs on its previous block, at
    /// most `MAX_SOLVABLE_JOBS`, the oldest dropped first.
    fn keep_solvable(&mut self, job: SolvableJob) {
        self.solvable_jobs
            .retain(|kept| kept.prev_hash == job.prev_hash);
        if self.solvable_jobs.len() == MAX_SOLVABLE_JOBS {
            self.solvable_jobs.pop_front();
        }
        self.solvable_jobs.push_back(job);
    }

    /// The block a `PushSolution` in `payload` reports (sv2-spec 6.4.9),
    /// rebuilt from the newest of this connection's Full-Template
    /// declarations it solves.
    ///
    /// `None`, with the connection kept, when none does: a Coinbase-Only
    /// declarator, whose transactions the node never saw, a job on another
    /// previous block, or a header short of the template's target. The
    /// message has no answer. A connection that never set up Job
    /// Declaration, or a payload that does not decode, is an error.
    pub(crate) fn push_solution(&mut self, payload: &mut [u8]) -> Result<Option<Block>, String> {
        if self.mode.is_none() {
            return Err("PushSolution requires a Job Declaration connection".to_string());
        }
        let push = PushSolution::from_bytes(payload).map_err(|error| error.to_string())?;
        let block = self
            .solvable_jobs
            .iter()
            .rev()
            .filter(|job| job.prev_hash.as_byte_array().as_slice() == push.prev_hash.as_bytes())
            .find_map(|job| job.block(&push));
        if block.is_none() {
            debug!("PushSolution solves none of this connection's Full-Template declarations");
        }
        Ok(block)
    }

    fn on_custom_job(&mut self, request: &SetCustomMiningJob) -> Result<Vec<Vec<u8>>, String> {
        if !self.mining {
            return Err("SetCustomMiningJob requires a mining connection".to_string());
        }
        let token = request.token.as_bytes().to_vec();
        let registry = self.registry.lock().expect("job registry lock");
        let ready = matches!(
            registry.tokens.get(&token).map(|token| &token.state),
            Some(TokenState::Ready { .. })
                | Some(TokenState::Open {
                    mode: JobDeclarationMode::CoinbaseOnly,
                })
        );
        if !ready {
            drop(registry);
            return self.respond(
                SetCustomMiningJobError {
                    channel_id: request.channel_id,
                    request_id: request.request_id,
                    error_code: string_field("invalid-mining-job-token")?,
                },
                MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_ERROR,
            );
        }
        drop(registry);
        // A token is not a validated tracker job; never promise rewards for it.
        self.respond(
            SetCustomMiningJobError {
                channel_id: request.channel_id,
                request_id: request.request_id,
                error_code: string_field("job-not-yet-validated")?,
            },
            MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_ERROR,
        )
    }

    fn declare_error(
        &mut self,
        request_id: u32,
        error_code: &'static str,
    ) -> Result<Vec<Vec<u8>>, String> {
        self.declare_error_with_details(request_id, error_code, &[])
    }

    /// A `DeclareMiningJob.Error` saying why in `details`.
    fn declare_error_with_details(
        &mut self,
        request_id: u32,
        error_code: &'static str,
        details: &[u8],
    ) -> Result<Vec<Vec<u8>>, String> {
        self.respond(
            DeclareMiningJobError {
                request_id,
                error_code: string_field(error_code)?,
                error_details: bytes_field(details)?,
            },
            MESSAGE_TYPE_DECLARE_MINING_JOB_ERROR,
        )
    }

    fn respond<T>(&mut self, message: T, message_type: u8) -> Result<Vec<Vec<u8>>, String>
    where
        T: binary_sv2::Serialize + binary_sv2::GetSize,
    {
        Ok(vec![encode_frame(message, message_type)?])
    }
}

/// One complete Stratum V2 message: its extension type with the channel bit
/// cleared (0 for the base protocol), its message type and its payload.
/// The extension type tells an extension's message from a base one of the
/// same type, such as `RequestExtensions` (`0x0001`, `0x00`) from
/// `SetupConnection` (`0x0000`, `0x00`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sv2Message {
    pub extension_type: u16,
    pub message_type: u8,
    pub payload: Vec<u8>,
}

impl Sv2Message {
    /// The message read from a frame's header and payload.
    pub(crate) fn from_frame(header: &FrameHeader, payload: &[u8]) -> Self {
        Self {
            extension_type: header.ext_type_without_channel_msg(),
            message_type: header.msg_type(),
            payload: payload.to_vec(),
        }
    }
}

/// Decode unencrypted `incoming` bytes into complete messages. `inbox`
/// keeps the bytes of a frame that is not complete yet, for the next call
/// with the same `decoder`.
pub(crate) fn decode_plain_frames(
    decoder: &mut Decoder,
    inbox: &mut Vec<u8>,
    incoming: &[u8],
) -> Result<Vec<Sv2Message>, String> {
    inbox.extend_from_slice(incoming);
    let mut messages = Vec::new();
    let mut reading = true;
    while reading && inbox.len() >= decoder.writable_len() {
        let room = decoder.writable_len();
        decoder.writable().copy_from_slice(&inbox[..room]);
        inbox.drain(..room);
        messages.extend(read_plain_frames(decoder)?);
        reading = decoder.writable_len() > 0;
    }
    Ok(messages)
}

fn read_plain_frames(decoder: &mut Decoder) -> Result<Vec<Sv2Message>, String> {
    let mut messages = Vec::new();
    let mut reading = true;
    while reading {
        match decoder.next_frame().map_err(|error| error.to_string())? {
            Decoded::Incomplete(_) => reading = false,
            Decoded::Frame(mut frame) => {
                let header = frame.header();
                messages.push(Sv2Message::from_frame(&header, frame.payload()));
            }
        }
    }
    Ok(messages)
}

/// Encode one unencrypted frame of extension `extension_type`'s message
/// `message_type`, with the channel bit when the message is for one channel.
/// The header is the base protocol's: the extension type (`u16`, the
/// channel bit its top bit), the message type and the payload length
/// (`u24`), all little-endian.
pub(crate) fn encode_extension_frame(
    extension_type: u16,
    message_type: u8,
    channel_message: bool,
    payload: &[u8],
) -> Result<Vec<u8>, String> {
    let length = u32::try_from(payload.len())
        .ok()
        .filter(|length| *length < 1 << 24)
        .ok_or("Stratum V2 extension message exceeds the frame size")?;
    let extension_type = extension_type | if channel_message { 0x8000 } else { 0 };
    let mut frame = Vec::with_capacity(6 + payload.len());
    frame.extend_from_slice(&extension_type.to_le_bytes());
    frame.push(message_type);
    frame.extend_from_slice(&length.to_le_bytes()[..3]);
    frame.extend_from_slice(payload);
    Ok(frame)
}

/// Whether the specification sets the channel bit on frames of this type.
///
/// Messages addressed to one channel carry the bit in their extension type;
/// a client that dispatches by it would misroute a frame without it.
fn is_channel_message(message_type: u8) -> bool {
    matches!(
        message_type,
        MESSAGE_TYPE_NEW_MINING_JOB
            | MESSAGE_TYPE_NEW_EXTENDED_MINING_JOB
            | MESSAGE_TYPE_MINING_SET_NEW_PREV_HASH
            | MESSAGE_TYPE_SET_TARGET
            | MESSAGE_TYPE_SUBMIT_SHARES_STANDARD
            | MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED
            | MESSAGE_TYPE_SUBMIT_SHARES_SUCCESS
            | MESSAGE_TYPE_SUBMIT_SHARES_ERROR
            | MESSAGE_TYPE_SET_CUSTOM_MINING_JOB
            | MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_SUCCESS
            | MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_ERROR
            | MESSAGE_TYPE_UPDATE_CHANNEL
            | MESSAGE_TYPE_UPDATE_CHANNEL_ERROR
            | MESSAGE_TYPE_CLOSE_CHANNEL
    )
}

/// Encode `message` as one unencrypted Stratum V2 frame, setting the channel
/// bit where the specification requires it.
pub(crate) fn encode_frame<T>(message: T, message_type: u8) -> Result<Vec<u8>, String>
where
    T: binary_sv2::Serialize + binary_sv2::GetSize,
{
    let frame =
        MessageFrame::from_message(message, message_type, 0, is_channel_message(message_type))
            .map_err(|error| error.to_string())?;
    Encoder::new()
        .encode(frame)
        .map_err(|error| error.to_string())
}

pub(crate) fn string_field(value: &'static str) -> Result<Str0255<'static>, String> {
    value
        .try_into()
        .map_err(|error: binary_sv2::Error| error.to_string())
}

fn bytes_field(value: &[u8]) -> Result<B064K<'_>, String> {
    B064K::new(value).map_err(|error| error.to_string())
}

fn bytes_255(value: &[u8]) -> Result<B0255<'_>, String> {
    B0255::new(value).map_err(|error| error.to_string())
}

/// A CompactSize-prefixed output list with no outputs: the single byte zero.
fn empty_output_list() -> Vec<u8> {
    serialize(&Vec::<TxOut>::with_capacity(0))
}

/// Encode `message` as one unencrypted Stratum V2 frame.
#[cfg(test)]
pub fn encode_message<T>(message: T, message_type: u8) -> Result<Vec<u8>, String>
where
    T: binary_sv2::Serialize + binary_sv2::GetSize,
{
    encode_frame(message, message_type)
}

#[cfg(test)]
mod tests;
