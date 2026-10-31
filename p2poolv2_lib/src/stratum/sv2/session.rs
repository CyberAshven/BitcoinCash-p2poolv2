// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! One Stratum V2 connection and the job registry it shares with the others.
//!
//! A Job Declaration connection and a Mining connection are separate TCP
//! sessions. The registry is what lets a token allocated on the first be
//! presented on the second.

use super::custom_job::declared_leaves;
use super::sort_ctor;
use crate::shares::auxpow::Disclosure;
use crate::shares::merge_mining::MergeMiningCommitment;
use crate::stratum::parse_password::{
    log_unknown_options, parse_miner_donation, split_user_identity,
};
use crate::stratum::work::prepared_notify::PreparedNotifyParams;
use crate::utils::time_provider::{SystemTimeProvider, TimeProvider};
use binary_sv2::{B064K, B0255, Deserialize, Seq064K, Str0255};
use bitcoin::consensus::{deserialize, serialize};
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::rand::random;
use bitcoin::{Transaction, TxOut};
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
use job_declaration_sv2::{
    AllocateMiningJobToken, AllocateMiningJobTokenSuccess, DeclareMiningJob, DeclareMiningJobError,
    DeclareMiningJobSuccess, ERROR_CODE_DECLARE_MINING_JOB_INVALID_JOB,
    ERROR_CODE_DECLARE_MINING_JOB_INVALID_MINING_JOB_TOKEN, MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN,
    MESSAGE_TYPE_ALLOCATE_MINING_JOB_TOKEN_SUCCESS, MESSAGE_TYPE_DECLARE_MINING_JOB,
    MESSAGE_TYPE_DECLARE_MINING_JOB_ERROR, MESSAGE_TYPE_DECLARE_MINING_JOB_SUCCESS,
    MESSAGE_TYPE_PROVIDE_MISSING_TRANSACTIONS, MESSAGE_TYPE_PROVIDE_MISSING_TRANSACTIONS_SUCCESS,
    ProvideMissingTransactions, ProvideMissingTransactionsSuccess,
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
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

const PROTOCOL_VERSION: u16 = 2;
/// The error a declaration whose coinbase the node refuses gets, as
/// Pickaxe's pool sends it: here, token leaves that are missing, not
/// canonical, or do not make output 0's root.
const INVALID_COINBASE_TX: &str = "invalid-coinbase-tx";
const WORK_SELECTION_FLAG: u32 = 0b10;
const MAX_JOB_TOKENS: usize = 64;
const TOKEN_LIFETIME: Duration = Duration::from_secs(120);
const MAX_DECLARED_TRANSACTION_BYTES: usize = 64 * 1024 * 1024;

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
    state: TokenState,
}

/// A claimed declaration, removed from the registry before asynchronous validation.
pub(crate) struct DeclaredJob {
    pub prepared: Arc<PreparedNotifyParams>,
    pub transactions: Option<Arc<Vec<Transaction>>>,
    pub declaration: Option<Declaration>,
}

/// Tokens and the transaction set each one mines.
pub struct JobRegistry {
    cache: PoolTransactionCache,
    merge_mining: Option<MergeMiningCommitment>,
    tokens: HashMap<Vec<u8>, JobToken>,
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
                    ..
                } => wtxids.len() * 32 + declaration.prefix.len() + declaration.suffix.len(),
                _ => 0,
            })
            .sum()
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
    /// Full-Template declarations made on this connection that still wait
    /// for `ProvideMissingTransactionsSuccess`, by the client's request id.
    /// Request ids are chosen by each client, so two connections can use the
    /// same one; the token is looked up here, never across the registry.
    pending_declarations: HashMap<u32, Vec<u8>>,
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
            pending_declarations: HashMap::with_capacity(4),
        }
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
    /// The Job Declaration Protocol has no error reply to this message, so a
    /// bad `donation=` value, or one on a node without a Bitcoin Cash share
    /// chain, closes the connection with the reason in the node's log.
    fn on_allocate(
        &mut self,
        request_id: u32,
        user_identifier: &str,
    ) -> Result<Vec<Vec<u8>>, String> {
        let mode = self.mode.ok_or("job token requested before setup")?;
        let (_, options) = split_user_identity(user_identifier);
        if let Some(options) = options {
            log_unknown_options(options);
        }
        let donation = parse_miner_donation(options)?;
        let mut registry = self.registry.lock().expect("job registry lock");
        registry.expire_tokens();
        if registry.tokens.len() >= MAX_JOB_TOKENS {
            return Err("job token limit reached".into());
        }
        if donation.is_some() && registry.merge_mining.is_none() {
            return Err("donation= is a Bitcoin Cash P2Pool option this node cannot honour".into());
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
            let outputs = payouts
                .outputs
                .iter()
                .map(|pair| pair.address.script_pubkey())
                .chain(tree_script)
                .map(|script_pubkey| TxOut {
                    value: bitcoin::Amount::ZERO,
                    script_pubkey,
                })
                .collect::<Vec<_>>();
            let token = prepared
                .mining_job_token(random(), SystemTimeProvider.seconds_since_epoch() as u32)
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
        let state = registry.tokens.get(&token).map(|token| &token.state);
        if !matches!(
            state,
            Some(TokenState::Open {
                mode: JobDeclarationMode::FullTemplate
            })
        ) {
            drop(registry);
            return self.declare_error(
                request.request_id,
                ERROR_CODE_DECLARE_MINING_JOB_INVALID_MINING_JOB_TOKEN,
            );
        }
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
        let wtxids = request
            .wtxid_list
            .iter()
            .map(|wtxid| {
                let mut bytes = [0u8; 32];
                bytes.copy_from_slice(wtxid.as_bytes());
                bytes
            })
            .collect::<Vec<_>>();
        // shortcut: one response reveals every transaction; add cache negotiation for larger frames.
        let positions = (0..wtxids.len())
            .map(|index| {
                u16::try_from(index)
                    .map_err(|_| "declaration has more transactions than u16".to_string())
            })
            .collect::<Result<Vec<_>, String>>()?;
        if self.pending_declarations.len() >= 8
            || registry.declared_bytes()
                + wtxids.len() * 32
                + request.coinbase_tx_prefix.as_bytes().len()
                + request.coinbase_tx_suffix.as_bytes().len()
                > MAX_DECLARED_TRANSACTION_BYTES
        {
            return Err("job declaration limit reached".into());
        }
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
        };
        drop(registry);
        self.pending_declarations.insert(request.request_id, token);
        self.respond(
            ProvideMissingTransactions {
                request_id: request.request_id,
                unknown_tx_position_list: Seq064K::new(positions)
                    .map_err(|error| error.to_string())?,
            },
            MESSAGE_TYPE_PROVIDE_MISSING_TRANSACTIONS,
        )
    }

    fn on_transactions(
        &mut self,
        request: &ProvideMissingTransactionsSuccess,
    ) -> Result<Vec<Vec<u8>>, String> {
        let token = self
            .pending_declarations
            .remove(&request.request_id)
            .ok_or("transactions arrived with no open declaration")?;
        let mut registry = self.registry.lock().expect("job registry lock");
        registry.expire_tokens();
        let (wtxids, declaration) = match registry.tokens.get(&token).map(|token| &token.state) {
            Some(TokenState::AwaitingTransactions {
                request_id,
                wtxids,
                declaration,
            }) if *request_id == request.request_id => (wtxids.clone(), declaration.clone()),
            _ => return Err("transactions arrived with no open declaration".to_string()),
        };
        if request.transaction_list.len() != wtxids.len() {
            return Err("transaction list length does not match the declaration".to_string());
        }
        let mut transactions = Vec::with_capacity(wtxids.len());
        for (index, raw) in request.transaction_list.iter().enumerate() {
            let transaction: Transaction =
                deserialize(raw.as_bytes()).map_err(|error| error.to_string())?;
            let wtxid = transaction.compute_wtxid().to_byte_array();
            if wtxid != wtxids[index] {
                return Err("revealed transaction does not match its declared wtxid".to_string());
            }
            transactions.push(transaction);
        }
        sort_ctor(&mut transactions);
        if registry.declared_bytes()
            + transactions
                .iter()
                .map(Transaction::total_size)
                .sum::<usize>()
            > MAX_DECLARED_TRANSACTION_BYTES
        {
            return Err("declared transaction memory limit reached".into());
        }
        let mut job_token = registry
            .tokens
            .remove(&token)
            .ok_or("expired-mining-job-token")?;
        let mut new_token_bytes = token;
        if registry.merge_mining.is_some() {
            new_token_bytes[1..33].copy_from_slice(&random::<[u8; 32]>());
        } else {
            new_token_bytes = random::<[u8; 32]>().to_vec();
        }
        job_token.state = TokenState::Ready {
            transactions: Arc::new(transactions),
            declaration,
        };
        registry.tokens.insert(new_token_bytes.clone(), job_token);
        drop(registry);
        self.respond(
            DeclareMiningJobSuccess {
                request_id: request.request_id,
                new_mining_job_token: bytes_255(&new_token_bytes)?,
            },
            MESSAGE_TYPE_DECLARE_MINING_JOB_SUCCESS,
        )
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
