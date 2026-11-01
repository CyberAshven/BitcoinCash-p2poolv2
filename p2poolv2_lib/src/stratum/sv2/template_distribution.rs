// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Stratum V2 Template Distribution client: block templates from a Template
//! Provider instead of the node's `getblocktemplate`.
//!
//! The node connects to the configured provider, over Noise when the
//! provider's authority key is pinned, sets up the Template Distribution
//! Protocol and declares in `CoinbaseOutputConstraints` the most coinbase
//! output bytes it can ever add. The provider then sends `NewTemplate` and
//! `SetNewPrevHash`, and the node asks for each template's transactions with
//! `RequestTransactionData`. Once a template's parent and transactions are
//! both known it becomes a [`BlockTemplate`], passes the checks a BCHN
//! template passes (CTOR, no witness data, target and bits), and goes to the
//! notifier exactly as a template fetched over RPC does. SV1 and SV2 jobs,
//! share emission and PPLNS payouts therefore do not change.
//!
//! A block found on a provided template goes back with `SubmitSolution`.
//! The protocol has no reply to it, so whenever the provider cannot take the
//! solution (its connection is gone, or the coinbase does not fit the
//! message) the block goes to the node over RPC instead.
//!
//! The coinbase is the share chain's: version 2, input sequence `0xfffffffe`
//! and lock time height - 1, which peers check when they validate a share. A
//! provider's own values for these fields are only logged; Bitcoin Cash
//! accepts any. The provider's `coinbase_prefix` is checked: it must be the
//! minimal BIP34 height push this coinbase starts with, written as the push
//! (the specification's form) or as the pushed bytes alone (the form
//! bchn-sv2-bridge sends, which checks submissions against the push). The
//! height comes from the node over RPC, as the parent's height plus one, so a
//! provider on another chain than the node's gets no work mined.
//!
//! The protocol carries no block size limit either. BCHN decides it: the same
//! `getblockheader` call reports the limit for a block on the parent
//! (`ablastate.nextblocksizelimit`), a 32 MB floor that ABLA raises with no
//! fixed maximum. Templates are checked against that, never a fixed size.
//!
//! Only Bitcoin Cash templates are mapped: a provider that requires coinbase
//! outputs (a Bitcoin provider's segwit commitment) is refused.

use super::noise::NoiseTransport;
use super::session::{decode_plain_frames, encode_frame};
use crate::config::{BitcoinCashNetwork, TemplateProvider};
use crate::stratum::block_submit::submit_block;
use crate::stratum::work::block_template::{BlockTemplate, TemplateTransaction};
use crate::stratum::work::error::WorkError;
use crate::stratum::work::gbt::build_merkle_branches_for_template;
use crate::stratum::work::notify::{NotifyCmd, NotifySender};
use crate::utils::time_provider::{SystemTimeProvider, TimeProvider};
use binary_sv2::{B064K, Deserialize};
use bitcoin::blockdata::script::Builder;
use bitcoin::consensus::{deserialize, serialize};
use bitcoin::hashes::Hash;
use bitcoin::{Block, BlockHash, Target, Transaction};
use bitcoindrpc::{BitcoinRpcConfig, BitcoindRpcClient, BlockHeaderSummary};
use codec_sv2::Decoder;
use common_messages_sv2::{
    MESSAGE_TYPE_RECONNECT, MESSAGE_TYPE_SETUP_CONNECTION, MESSAGE_TYPE_SETUP_CONNECTION_ERROR,
    MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS, Protocol, Reconnect, SetupConnection,
    SetupConnectionError,
};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;
use template_distribution_sv2::{
    CoinbaseOutputConstraints, MESSAGE_TYPE_COINBASE_OUTPUT_CONSTRAINTS, MESSAGE_TYPE_NEW_TEMPLATE,
    MESSAGE_TYPE_REQUEST_TRANSACTION_DATA, MESSAGE_TYPE_REQUEST_TRANSACTION_DATA_ERROR,
    MESSAGE_TYPE_REQUEST_TRANSACTION_DATA_SUCCESS, MESSAGE_TYPE_SET_NEW_PREV_HASH,
    MESSAGE_TYPE_SUBMIT_SOLUTION, NewTemplate, RequestTransactionData, RequestTransactionDataError,
    RequestTransactionDataSuccess, SetNewPrevHash, SubmitSolution,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::mpsc;
use tokio::time::{sleep, timeout};
use tracing::{debug, error, info, warn};

/// Template Distribution Protocol version this client speaks.
const PROTOCOL_VERSION: u16 = 2;

/// Largest coinbase `SubmitSolution` can carry: its `coinbase_tx` is a B0_64K.
const MAX_SUBMITTED_COINBASE: u32 = 65_535;

/// Coinbase bytes outside its outputs when its script is the 100 bytes a
/// provider must assume: version 4, input count 1, previous output 36,
/// script length 1, script 100, sequence 4 and lock time 4.
const COINBASE_WITHOUT_OUTPUTS: u32 = 150;

/// Bytes of the largest output count the coinbase can need, a CompactSize.
/// The provider accounts for it itself, so it is not part of the reserve.
const OUTPUT_COUNT_SIZE: u32 = 3;

/// `coinbase_output_max_additional_size`: every output byte of the largest
/// coinbase `SubmitSolution` can carry. The PPLNS payouts grow with the
/// miners in the window, so the node declares this bound once instead of a
/// size that a later template could outgrow.
pub(crate) const COINBASE_OUTPUT_RESERVE: u32 =
    MAX_SUBMITTED_COINBASE - COINBASE_WITHOUT_OUTPUTS - OUTPUT_COUNT_SIZE;

/// BCH SigChecks reserve, carried in the protocol's legacy-named
/// `coinbase_output_max_additional_sigops` field. A coinbase executes no
/// scripts, so its SigChecks count is zero regardless of its outputs.
/// See <https://upgradespecs.bitcoincashnode.org/2020-05-15-sigchecks/#counting-rule>.
pub(crate) const COINBASE_OUTPUT_SIGCHECKS: u16 = 0;

/// Block bytes per SigCheck a Bitcoin Cash block may use.
const BYTES_PER_SIGCHECK: u32 = 141;

/// Block header bytes plus the largest transaction count CompactSize.
const HEADER_AND_COUNT_SIZE: u32 = 80 + 9;

/// Templates kept while they wait for their transactions or their
/// `SetNewPrevHash`. Template ids only grow, so the oldest go first.
const MAX_PENDING_TEMPLATES: usize = 16;

/// Found blocks queued for one provider connection.
const SOLUTION_QUEUE: usize = 16;

/// How long the provider has to answer `SetupConnection`.
const SETUP_TIMEOUT: Duration = Duration::from_secs(10);

/// First wait before reconnecting to the provider; it doubles up to
/// [`MAX_RECONNECT_DELAY`] while connections keep failing.
const RECONNECT_DELAY: Duration = Duration::from_secs(1);
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(30);

/// A provider can hear of a block a moment before the node does, so the
/// parent's height is asked for this many times, this far apart.
const HEIGHT_ATTEMPTS: u32 = 10;
const HEIGHT_RETRY_DELAY: Duration = Duration::from_millis(500);

/// Bytes read from the provider socket at a time.
const READ_BUFFER_SIZE: usize = 64 * 1024;

/// A block found on a provided template, on its way back to the provider.
#[derive(Debug)]
pub(crate) struct Solution {
    template_id: u64,
    /// Height of the block, the provided template's.
    height: u64,
    block: Block,
}

/// A template that came from a Template Provider, and the way back to the
/// provider connection that sent it.
#[derive(Debug, Clone)]
pub struct ProvidedTemplate {
    template_id: u64,
    height: u64,
    solutions: mpsc::Sender<Solution>,
}

impl ProvidedTemplate {
    /// Queue `block` for the provider connection that sent this template.
    ///
    /// Gives the block back when that connection has closed or its queue is
    /// full, so the caller can submit it over RPC instead.
    #[cfg_attr(feature = "sim", allow(dead_code))]
    pub(crate) fn submit(&self, block: Block) -> Result<(), Block> {
        self.solutions
            .try_send(Solution {
                template_id: self.template_id,
                height: self.height,
                block,
            })
            .map_err(|refused| refused.into_inner().block)
    }
}

/// The provider's latest `SetNewPrevHash`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProviderTip {
    template_id: u64,
    prev_hash: BlockHash,
    header_timestamp: u32,
    bits: u32,
    /// Little-endian, as the message carries it.
    target: [u8; 32],
}

/// A `NewTemplate` kept until it can be mined.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AnnouncedTemplate {
    template_id: u64,
    future: bool,
    version: u32,
    coinbase_tx_version: u32,
    coinbase_prefix: Vec<u8>,
    coinbase_tx_input_sequence: u32,
    coinbase_value: u64,
    coinbase_outputs_count: u32,
    coinbase_outputs: Vec<u8>,
    coinbase_tx_locktime: u32,
    merkle_path: Vec<[u8; 32]>,
    /// `None` until `RequestTransactionData.Success` arrives.
    transactions: Option<Vec<Transaction>>,
}

impl AnnouncedTemplate {
    fn from_message(message: &NewTemplate<'_>) -> Self {
        let merkle_path = message
            .merkle_path
            .iter()
            .map(|branch| {
                let mut bytes = [0u8; 32];
                bytes.copy_from_slice(branch.as_bytes());
                bytes
            })
            .collect::<Vec<_>>();
        // A template with no merkle path has no transactions to ask for.
        let transactions = merkle_path.is_empty().then(Vec::new);
        Self {
            template_id: message.template_id,
            future: message.future_template,
            version: message.version,
            coinbase_tx_version: message.coinbase_tx_version,
            coinbase_prefix: message.coinbase_prefix.as_bytes().to_vec(),
            coinbase_tx_input_sequence: message.coinbase_tx_input_sequence,
            coinbase_value: message.coinbase_tx_value_remaining,
            coinbase_outputs_count: message.coinbase_tx_outputs_count,
            coinbase_outputs: message.coinbase_tx_outputs.as_bytes().to_vec(),
            coinbase_tx_locktime: message.coinbase_tx_locktime,
            merkle_path,
            transactions,
        }
    }
}

/// A template whose parent and transactions are both known.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReadyTemplate {
    announced: AnnouncedTemplate,
    tip: ProviderTip,
}

/// What handling one provider message asks the connection to do.
#[derive(Debug, PartialEq, Eq)]
enum SessionAction {
    /// Send this unencrypted frame to the provider.
    Send(Vec<u8>),
    /// Map this template and hand it to the notifier.
    Ready(ReadyTemplate),
    /// The provider's `Reconnect`: an empty host or a zero port mean its own.
    Reconnect { host: String, port: u16 },
}

/// The Template Distribution state of one provider connection, without I/O.
#[derive(Default)]
struct ProviderSession {
    tip: Option<ProviderTip>,
    pending: BTreeMap<u64, AnnouncedTemplate>,
}

impl ProviderSession {
    /// Handle one message from the provider.
    ///
    /// A malformed message is an error, which ends the connection. Messages
    /// this client does not use are ignored.
    fn on_message(
        &mut self,
        message_type: u8,
        payload: &mut [u8],
    ) -> Result<Vec<SessionAction>, String> {
        match message_type {
            MESSAGE_TYPE_NEW_TEMPLATE => {
                let message =
                    NewTemplate::from_bytes(payload).map_err(|error| format!("{error:?}"))?;
                self.on_new_template(&message)
            }
            MESSAGE_TYPE_SET_NEW_PREV_HASH => {
                let message =
                    SetNewPrevHash::from_bytes(payload).map_err(|error| format!("{error:?}"))?;
                Ok(self.on_set_new_prev_hash(&message))
            }
            MESSAGE_TYPE_REQUEST_TRANSACTION_DATA_SUCCESS => {
                let message = RequestTransactionDataSuccess::from_bytes(payload)
                    .map_err(|error| format!("{error:?}"))?;
                Ok(self.on_transactions(&message))
            }
            MESSAGE_TYPE_REQUEST_TRANSACTION_DATA_ERROR => {
                let message = RequestTransactionDataError::from_bytes(payload)
                    .map_err(|error| format!("{error:?}"))?;
                self.on_transaction_error(&message);
                Ok(Vec::new())
            }
            MESSAGE_TYPE_RECONNECT => {
                let message =
                    Reconnect::from_bytes(payload).map_err(|error| format!("{error:?}"))?;
                Ok(vec![SessionAction::Reconnect {
                    host: message.new_host.as_utf8_or_hex(),
                    port: message.new_port,
                }])
            }
            other => {
                debug!("Ignoring Template Distribution message type {other:#04x}");
                Ok(Vec::new())
            }
        }
    }

    /// Keep the template and ask for its transactions.
    ///
    /// A template for the current parent is refused before any
    /// `SetNewPrevHash`, which it would relate to.
    fn on_new_template(&mut self, message: &NewTemplate<'_>) -> Result<Vec<SessionAction>, String> {
        let announced = AnnouncedTemplate::from_message(message);
        if !announced.future && self.tip.is_none() {
            warn!(
                "Template Provider sent current template {} before any SetNewPrevHash",
                announced.template_id
            );
            return Ok(Vec::new());
        }
        let template_id = announced.template_id;
        let needs_transactions = announced.transactions.is_none();
        self.pending.insert(template_id, announced);
        while self.pending.len() > MAX_PENDING_TEMPLATES {
            self.pending.pop_first();
        }
        let mut actions = Vec::with_capacity(1);
        if needs_transactions {
            actions.push(SessionAction::Send(encode_frame(
                RequestTransactionData { template_id },
                MESSAGE_TYPE_REQUEST_TRANSACTION_DATA,
            )?));
        }
        actions.extend(self.take_ready(template_id));
        Ok(actions)
    }

    /// Move to the new parent. Templates for the old one, and future
    /// templates this message did not activate, can no longer be mined.
    fn on_set_new_prev_hash(&mut self, message: &SetNewPrevHash<'_>) -> Vec<SessionAction> {
        let mut prev_hash = [0u8; 32];
        prev_hash.copy_from_slice(message.prev_hash.as_bytes());
        let mut target = [0u8; 32];
        target.copy_from_slice(message.target.as_bytes());
        let tip = ProviderTip {
            template_id: message.template_id,
            prev_hash: BlockHash::from_byte_array(prev_hash),
            header_timestamp: message.header_timestamp,
            bits: message.n_bits,
            target,
        };
        self.tip = Some(tip);
        self.pending
            .retain(|template_id, _| *template_id == tip.template_id);
        if self.pending.is_empty() {
            warn!(
                "Template Provider activated template {}, which it never announced",
                tip.template_id
            );
        }
        self.take_ready(tip.template_id).into_iter().collect()
    }

    fn on_transactions(
        &mut self,
        message: &RequestTransactionDataSuccess<'_>,
    ) -> Vec<SessionAction> {
        let template_id = message.template_id;
        let Some(announced) = self.pending.get_mut(&template_id) else {
            debug!("Transactions for Template Provider template {template_id}, no longer pending");
            return Vec::new();
        };
        let decoded = message
            .transaction_list
            .iter()
            .map(|raw| deserialize::<Transaction>(raw.as_bytes()))
            .collect::<Result<Vec<_>, _>>();
        match decoded {
            Ok(transactions) => {
                announced.transactions = Some(transactions);
                self.take_ready(template_id).into_iter().collect()
            }
            Err(decode_error) => {
                error!(
                    "Template Provider template {template_id} has a malformed transaction: {decode_error}"
                );
                self.pending.remove(&template_id);
                Vec::new()
            }
        }
    }

    fn on_transaction_error(&mut self, message: &RequestTransactionDataError<'_>) {
        warn!(
            "Template Provider refused transactions for template {}: {}",
            message.template_id,
            message.error_code.as_utf8_or_hex()
        );
        self.pending.remove(&message.template_id);
    }

    /// The template, if it can be mined now: its transactions are known and
    /// it is either the template the latest `SetNewPrevHash` activated or a
    /// template for the current parent. Older pending templates go with it.
    fn take_ready(&mut self, template_id: u64) -> Option<SessionAction> {
        let tip = self.tip?;
        let announced = self.pending.get(&template_id)?;
        let active = !announced.future || tip.template_id == template_id;
        if !active || announced.transactions.is_none() {
            return None;
        }
        let announced = self.pending.remove(&template_id)?;
        self.pending
            .retain(|pending_id, _| *pending_id > template_id);
        Some(SessionAction::Ready(ReadyTemplate { announced, tip }))
    }
}

/// The floor of the network's adaptive block size limit (ABLA): 32 MB on
/// mainnet and regtest, 2 MB on chipnet, 256 MB on scalenet
/// (`nDefaultConsensusBlockSize` in BCHN's `src/chainparams.cpp`). The limit
/// never goes below it, so a block within the floor is never too large.
fn block_size_floor(network: BitcoinCashNetwork) -> u64 {
    match network {
        BitcoinCashNetwork::Mainnet | BitcoinCashNetwork::Regtest => 32_000_000,
        BitcoinCashNetwork::Chipnet => 2_000_000,
        BitcoinCashNetwork::Scalenet => 256_000_000,
    }
}

/// Block size limit a provided template is checked against.
///
/// `reported` is the latest limit BCHN reported for a block on a template's
/// parent (`ablastate.nextblocksizelimit`), the same limit its own
/// `getblocktemplate` reports as `sizelimit`. BCHN manages the limit, so it is
/// used whatever its size. Only while BCHN has reported none, because it is
/// unreachable or keeps no ABLA state, does the network's floor stand in.
fn template_size_limit(reported: Option<u64>, network: BitcoinCashNetwork) -> u64 {
    reported.unwrap_or_else(|| block_size_floor(network))
}

/// The scriptSig start of every coinbase this pool builds at `height`: the
/// minimal BIP34 height push.
fn height_push(height: u64) -> Result<Vec<u8>, WorkError> {
    let height = i64::try_from(height).map_err(|_| WorkError {
        message: format!("block height {height} does not fit a script number"),
    })?;
    Ok(Builder::new().push_int(height).into_script().into_bytes())
}

/// Whether a coinbase starting with `push` satisfies the provider's
/// `prefix`: as the specification writes it, or as the pushed bytes that
/// bchn-sv2-bridge sends and expects behind their length.
fn prefix_matches_height_push(prefix: &[u8], push: &[u8]) -> bool {
    let length_prefixed = u8::try_from(prefix.len())
        .is_ok_and(|length| push.split_first() == Some((&length, prefix)));
    prefix == push || length_prefixed
}

/// Build the [`BlockTemplate`] for `ready` at `height` and check it as a BCHN
/// template is checked, against the block size limit `size_limit`.
///
/// `now` (seconds since the epoch) refreshes the time of a template that came
/// after its `SetNewPrevHash`. `solutions` is the queue of the connection
/// the template came on.
fn provided_block_template(
    ready: ReadyTemplate,
    height: u64,
    size_limit: u64,
    now: u32,
    solutions: mpsc::Sender<Solution>,
) -> Result<BlockTemplate, WorkError> {
    let template_id = ready.announced.template_id;
    let invalid = |message: &str| WorkError {
        message: format!("Template Provider template {template_id}: {message}"),
    };
    let announced = ready.announced;
    let tip = ready.tip;
    if announced.coinbase_outputs_count != 0 || !announced.coinbase_outputs.is_empty() {
        return Err(invalid(
            "requires coinbase outputs, which a Bitcoin Cash share chain coinbase cannot carry",
        ));
    }
    if !prefix_matches_height_push(&announced.coinbase_prefix, &height_push(height)?) {
        return Err(invalid(&format!(
            "coinbase prefix {} is not the height push for height {height}",
            hex::encode(&announced.coinbase_prefix)
        )));
    }
    if announced.coinbase_tx_version != 2
        || announced.coinbase_tx_input_sequence != 0xffff_fffe
        || u64::from(announced.coinbase_tx_locktime) + 1 != height
    {
        debug!(
            "Template Provider template {} asks for coinbase version {}, sequence {:#x} and lock time {}; the share chain coinbase keeps version 2, sequence 0xfffffffe and lock time height - 1",
            template_id,
            announced.coinbase_tx_version,
            announced.coinbase_tx_input_sequence,
            announced.coinbase_tx_locktime
        );
    }
    let transactions = announced.transactions.unwrap_or_default();
    let transaction_bytes = transactions
        .iter()
        .map(|transaction| transaction.total_size() as u64)
        .sum::<u64>();
    if transaction_bytes + u64::from(HEADER_AND_COUNT_SIZE) + u64::from(MAX_SUBMITTED_COINBASE)
        > size_limit
    {
        return Err(invalid("transactions leave no room for the coinbase"));
    }
    let target = Target::from_le_bytes(tip.target);
    let template = BlockTemplate {
        version: announced.version as i32,
        rules: Vec::new(),
        vbavailable: HashMap::with_capacity(0),
        vbrequired: 0,
        previousblockhash: tip.prev_hash.to_string(),
        transactions: transactions
            .iter()
            .map(|transaction| TemplateTransaction {
                data: hex::encode(serialize(transaction)),
                txid: transaction.compute_txid().to_string(),
                hash: transaction.compute_txid().to_string(),
                depends: Vec::new(),
                fee: 0,
                sigops: 0,
                weight: 0,
            })
            .collect(),
        coinbaseaux: HashMap::with_capacity(0),
        coinbasevalue: announced.coinbase_value,
        longpollid: template_id.to_string(),
        target: hex::encode(target.to_be_bytes()),
        mintime: tip.header_timestamp,
        mutable: Vec::new(),
        noncerange: "00000000ffffffff".to_string(),
        sigoplimit: size_limit / u64::from(BYTES_PER_SIGCHECK),
        sizelimit: size_limit,
        weightlimit: 0,
        curtime: tip.header_timestamp.max(now),
        bits: format!("{:08x}", tip.bits),
        height: u32::try_from(height).map_err(|_| invalid("height does not fit 32 bits"))?,
        default_witness_commitment: None,
        provided: Some(ProvidedTemplate {
            template_id,
            height,
            solutions,
        }),
        light: None,
    };
    template.validate_bch()?;
    let branches = build_merkle_branches_for_template(&template)?
        .into_iter()
        .map(|branch| branch.to_byte_array())
        .collect::<Vec<_>>();
    if branches != announced.merkle_path {
        return Err(invalid("merkle path does not match its transactions"));
    }
    Ok(template)
}

/// The `SetupConnection` for the Template Distribution Protocol.
fn setup_connection_frame(provider: &TemplateProvider) -> Result<Vec<u8>, String> {
    let (host, port) = provider
        .address
        .rsplit_once(':')
        .and_then(|(host, port)| Some((host, port.parse::<u16>().ok()?)))
        .ok_or("template provider address is not HOST:PORT")?;
    encode_frame(
        SetupConnection {
            protocol: Protocol::TemplateDistributionProtocol,
            min_version: PROTOCOL_VERSION,
            max_version: PROTOCOL_VERSION,
            flags: 0,
            endpoint_host: host
                .try_into()
                .map_err(|error| format!("endpoint host: {error:?}"))?,
            endpoint_port: port,
            vendor: "p2poolv2"
                .try_into()
                .map_err(|error| format!("vendor: {error:?}"))?,
            hardware_version: ""
                .try_into()
                .map_err(|error| format!("hardware version: {error:?}"))?,
            firmware: env!("CARGO_PKG_VERSION")
                .try_into()
                .map_err(|error| format!("firmware: {error:?}"))?,
            device_id: ""
                .try_into()
                .map_err(|error| format!("device id: {error:?}"))?,
        },
        MESSAGE_TYPE_SETUP_CONNECTION,
    )
}

/// Reserve bytes for the largest PPLNS coinbase and zero BCH SigChecks.
fn coinbase_output_constraints_frame() -> Result<Vec<u8>, String> {
    encode_frame(
        CoinbaseOutputConstraints {
            coinbase_output_max_additional_size: COINBASE_OUTPUT_RESERVE,
            coinbase_output_max_additional_sigops: COINBASE_OUTPUT_SIGCHECKS,
        },
        MESSAGE_TYPE_COINBASE_OUTPUT_CONSTRAINTS,
    )
}

/// `SubmitSolution` for `solution`, or `None` when its coinbase does not fit
/// the message.
fn submit_solution_frame(solution: &Solution) -> Option<Vec<u8>> {
    let coinbase = serialize(solution.block.txdata.first()?);
    encode_frame(
        SubmitSolution {
            template_id: solution.template_id,
            version: solution.block.header.version.to_consensus() as u32,
            header_timestamp: solution.block.header.time,
            header_nonce: solution.block.header.nonce,
            coinbase_tx: B064K::new(&coinbase).ok()?,
        },
        MESSAGE_TYPE_SUBMIT_SOLUTION,
    )
    .ok()
}

/// Frames to and from the provider, encrypted when its key is pinned.
enum Transport {
    Plain { decoder: Decoder, inbox: Vec<u8> },
    Noise(Box<NoiseTransport>),
}

impl Transport {
    /// Complete Template Distribution messages in `incoming`: type and
    /// payload. This node negotiates no extension with a provider, so a
    /// frame of one is left out.
    fn decode(&mut self, incoming: &[u8]) -> Result<Vec<(u8, Vec<u8>)>, String> {
        let messages = match self {
            Transport::Plain { decoder, inbox } => decode_plain_frames(decoder, inbox, incoming),
            Transport::Noise(noise) => noise.decode(incoming),
        }?;
        Ok(messages
            .into_iter()
            .filter(|message| {
                let base = message.extension_type == 0;
                if !base {
                    debug!(
                        "Ignoring a Template Provider message of extension {:#06x}",
                        message.extension_type
                    );
                }
                base
            })
            .map(|message| (message.message_type, message.payload))
            .collect())
    }

    /// Bytes to write for one unencrypted frame.
    fn wire(&mut self, frame: Vec<u8>) -> Result<Vec<u8>, String> {
        match self {
            Transport::Plain { .. } => Ok(frame),
            Transport::Noise(noise) => noise.encrypt(frame),
        }
    }
}

/// Why a provider connection ended.
#[derive(Debug, PartialEq, Eq)]
enum ConnectionEnd {
    /// The notifier is gone, so the node is stopping.
    NotifierClosed,
    /// The provider sent `Reconnect`: the next connection goes there.
    Redirected(TemplateProvider),
    /// Anything else: the client reconnects.
    Failed(String),
}

/// Start the task that takes templates from `provider` and sends them to
/// the notifier, in place of `start_gbt`.
///
/// The task keeps reconnecting while the node runs, waiting longer after
/// each failed connection up to [`MAX_RECONNECT_DELAY`]; miners keep the
/// last template meanwhile, as they do when `getblocktemplate` fails. It
/// ends when the notifier's receiver is dropped. `bitcoin_config` is the
/// node templates are checked against and blocks fall back to.
pub async fn start_template_provider(
    provider: TemplateProvider,
    bitcoin_config: BitcoinRpcConfig,
    notify_tx: NotifySender,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let bitcoind = BitcoindRpcClient::new(
        &bitcoin_config.url,
        &bitcoin_config.username,
        &bitcoin_config.password,
    )
    .map_err(|error| WorkError {
        message: format!("Failed to create the bitcoind client: {error}"),
    })?;
    info!(
        "Taking block templates from the Stratum V2 Template Provider at {} ({})",
        provider.address,
        if provider.authority_public_key.is_some() {
            "Noise, pinned authority key"
        } else {
            "unencrypted, loopback"
        }
    );
    tokio::spawn(async move {
        keep_connected(&provider, &bitcoind, &notify_tx).await;
    });
    Ok(())
}

/// Run provider connections one after another until the notifier is gone.
///
/// A `Reconnect` sends the next connection to the address it names, after
/// [`RECONNECT_DELAY`]; any other end of a connection returns to the
/// configured provider, which may redirect again.
async fn keep_connected(
    provider: &TemplateProvider,
    bitcoind: &BitcoindRpcClient,
    notify_tx: &NotifySender,
) {
    let mut target = provider.clone();
    let mut delay = RECONNECT_DELAY;
    let mut running = true;
    while running {
        let (end, published) = serve_provider(&target, bitcoind, notify_tx).await;
        match end {
            ConnectionEnd::NotifierClosed => {
                info!("Notifier closed, stopping the Template Provider client");
                running = false;
            }
            ConnectionEnd::Redirected(next) => {
                info!(
                    "Template Provider at {} redirected the node to {}",
                    target.address, next.address
                );
                target = next;
                delay = RECONNECT_DELAY;
                sleep(RECONNECT_DELAY).await;
            }
            ConnectionEnd::Failed(reason) => {
                if published {
                    delay = RECONNECT_DELAY;
                }
                warn!(
                    "Template Provider connection to {} ended: {reason}; reconnecting to {} in {}s",
                    target.address,
                    provider.address,
                    delay.as_secs()
                );
                target = provider.clone();
                sleep(delay).await;
                delay = (delay * 2).min(MAX_RECONNECT_DELAY);
            }
        }
    }
}

/// Serve one provider connection until it ends.
///
/// Returns why it ended and whether it published a template. Solutions
/// still queued for it then go to the node over RPC.
async fn serve_provider(
    provider: &TemplateProvider,
    bitcoind: &BitcoindRpcClient,
    notify_tx: &NotifySender,
) -> (ConnectionEnd, bool) {
    let (solutions_tx, mut solutions_rx) = mpsc::channel(SOLUTION_QUEUE);
    let (end, published) = match open_connection(provider).await {
        Ok((transport, stream, early_messages)) => {
            let (mut reader, writer) = stream.into_split();
            let mut connection = ProviderConnection {
                provider,
                bitcoind,
                notify_tx,
                transport,
                writer,
                session: ProviderSession::default(),
                parent: None,
                reported_size_limit: None,
                found_block: None,
                solutions_tx,
                published: false,
            };
            let end = connection
                .run(&mut reader, &mut solutions_rx, early_messages)
                .await;
            (end, connection.published)
        }
        Err(reason) => (ConnectionEnd::Failed(reason), false),
    };
    solutions_rx.close();
    while let Ok(solution) = solutions_rx.try_recv() {
        submit_block(&solution.block, bitcoind).await;
    }
    (end, published)
}

/// Connect, run the Noise handshake when the key is pinned, and set up the
/// Template Distribution Protocol with the coinbase output constraints.
///
/// Also returns any messages that arrived with the setup reply.
async fn open_connection(
    provider: &TemplateProvider,
) -> Result<(Transport, TcpStream, Vec<(u8, Vec<u8>)>), String> {
    let mut stream = timeout(SETUP_TIMEOUT, TcpStream::connect(&provider.address))
        .await
        .map_err(|_| "connect timed out".to_string())?
        .map_err(|error| format!("connect: {error}"))?;
    if let Err(nodelay_error) = stream.set_nodelay(true) {
        debug!("Failed to set TCP_NODELAY for the Template Provider: {nodelay_error}");
    }
    let mut transport = match provider.authority_public_key {
        Some(key) => Transport::Noise(Box::new(NoiseTransport::connect(&mut stream, key).await?)),
        None => Transport::Plain {
            decoder: Decoder::new(),
            inbox: Vec::with_capacity(READ_BUFFER_SIZE),
        },
    };
    let setup = transport.wire(setup_connection_frame(provider)?)?;
    stream
        .write_all(&setup)
        .await
        .map_err(|error| format!("write: {error}"))?;
    let early_messages = timeout(SETUP_TIMEOUT, read_setup_reply(&mut stream, &mut transport))
        .await
        .map_err(|_| "no SetupConnection reply".to_string())??;
    let constraints = transport.wire(coinbase_output_constraints_frame()?)?;
    stream
        .write_all(&constraints)
        .await
        .map_err(|error| format!("write: {error}"))?;
    info!(
        "Template Provider at {} set up; reserved {COINBASE_OUTPUT_RESERVE} coinbase output bytes and {COINBASE_OUTPUT_SIGCHECKS} BCH SigChecks",
        provider.address
    );
    Ok((transport, stream, early_messages))
}

/// Read until the provider answers `SetupConnection`, and return the
/// messages that came after the answer.
async fn read_setup_reply(
    stream: &mut TcpStream,
    transport: &mut Transport,
) -> Result<Vec<(u8, Vec<u8>)>, String> {
    let mut buffer = vec![0u8; 4096];
    let mut messages = Vec::new();
    while messages.is_empty() {
        let count = stream
            .read(&mut buffer)
            .await
            .map_err(|error| format!("read: {error}"))?;
        if count == 0 {
            return Err("closed before answering SetupConnection".to_string());
        }
        messages = transport.decode(&buffer[..count])?;
    }
    let (message_type, mut payload) = messages.remove(0);
    match message_type {
        MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS => Ok(messages),
        MESSAGE_TYPE_SETUP_CONNECTION_ERROR => {
            let refusal = SetupConnectionError::from_bytes(&mut payload)
                .map_err(|error| format!("{error:?}"))?;
            Err(format!(
                "SetupConnection refused: {}",
                refusal.error_code.as_utf8_or_hex()
            ))
        }
        other => Err(format!(
            "expected a SetupConnection reply, got message type {other:#04x}"
        )),
    }
}

/// The parent the provider's current templates build on.
#[derive(Clone, Copy, Debug)]
struct ParentBlock {
    hash: BlockHash,
    /// Height of a block on the parent.
    height: u64,
    /// Block size limit for a block on the parent (`template_size_limit`).
    size_limit: u64,
}

/// One set-up provider connection.
struct ProviderConnection<'a> {
    provider: &'a TemplateProvider,
    bitcoind: &'a BitcoindRpcClient,
    notify_tx: &'a NotifySender,
    transport: Transport,
    writer: OwnedWriteHalf,
    session: ProviderSession,
    /// The parent of the latest published template.
    parent: Option<ParentBlock>,
    /// The latest block size limit the node reported for a parent.
    reported_size_limit: Option<u64>,
    /// The last block this node sent the provider, and the height of a
    /// block on it: when the provider names it as the parent, the height is
    /// known without asking the node, which may not have the block yet.
    found_block: Option<(BlockHash, u64)>,
    /// Given to every template published from this connection.
    solutions_tx: mpsc::Sender<Solution>,
    /// Whether a template from this connection reached the notifier.
    published: bool,
}

impl ProviderConnection<'_> {
    /// Exchange templates and solutions until the connection ends,
    /// starting with `early_messages`.
    async fn run(
        &mut self,
        reader: &mut OwnedReadHalf,
        solutions_rx: &mut mpsc::Receiver<Solution>,
        early_messages: Vec<(u8, Vec<u8>)>,
    ) -> ConnectionEnd {
        let mut end = self.handle_messages(early_messages).await.err();
        let mut buffer = vec![0u8; READ_BUFFER_SIZE];
        while end.is_none() {
            tokio::select! {
                read = reader.read(&mut buffer) => {
                    end = match read {
                        Ok(0) => Some(ConnectionEnd::Failed("closed by the provider".to_string())),
                        Err(read_error) => Some(ConnectionEnd::Failed(format!("read: {read_error}"))),
                        Ok(count) => match self.transport.decode(&buffer[..count]) {
                            Ok(messages) => self.handle_messages(messages).await.err(),
                            Err(reason) => Some(ConnectionEnd::Failed(reason)),
                        },
                    };
                }
                Some(solution) = solutions_rx.recv() => {
                    end = self.send_solution(solution).await.err();
                }
            }
        }
        end.unwrap_or_else(|| ConnectionEnd::Failed("ended".to_string()))
    }

    /// Answer the provider and publish the templates `messages` make ready.
    async fn handle_messages(&mut self, messages: Vec<(u8, Vec<u8>)>) -> Result<(), ConnectionEnd> {
        for (message_type, mut payload) in messages {
            let actions = self
                .session
                .on_message(message_type, &mut payload)
                .map_err(|reason| ConnectionEnd::Failed(format!("malformed message: {reason}")))?;
            for action in actions {
                match action {
                    SessionAction::Send(frame) => self.write_frame(frame).await?,
                    SessionAction::Ready(ready) => self.publish(ready).await?,
                    SessionAction::Reconnect { host, port } => {
                        return Err(match self.provider.redirected(&host, port) {
                            Ok(next) => ConnectionEnd::Redirected(next),
                            Err(refused) => ConnectionEnd::Failed(refused.message),
                        });
                    }
                }
            }
        }
        Ok(())
    }

    async fn write_frame(&mut self, frame: Vec<u8>) -> Result<(), ConnectionEnd> {
        let wire = self.transport.wire(frame).map_err(ConnectionEnd::Failed)?;
        self.writer
            .write_all(&wire)
            .await
            .map_err(|error| ConnectionEnd::Failed(format!("write: {error}")))
    }

    /// Make `hash`, with a block on it at `height`, the parent of the
    /// templates that follow, keeping `reported`, the size limit the node
    /// reported for a block on it, when there is one.
    fn enter_parent(&mut self, hash: BlockHash, height: u64, reported: Option<u64>) -> ParentBlock {
        if reported.is_some() {
            self.reported_size_limit = reported;
        }
        let size_limit = template_size_limit(self.reported_size_limit, self.provider.network);
        if self.reported_size_limit.is_none() {
            warn!(
                "The node reported no block size limit for parent {hash}; checking its templates against the network's {size_limit}-byte floor"
            );
        } else {
            debug!("Template Provider parent {hash}: block size limit {size_limit} bytes");
        }
        let parent = ParentBlock {
            hash,
            height,
            size_limit,
        };
        self.parent = Some(parent);
        parent
    }

    /// Map `ready` and send it to the notifier. A template that fails the
    /// checks is logged and skipped; the provider's next one may pass.
    async fn publish(&mut self, ready: ReadyTemplate) -> Result<(), ConnectionEnd> {
        let prev_hash = ready.tip.prev_hash;
        let parent = match (self.parent, self.found_block) {
            (Some(known), _) if known.hash == prev_hash => known,
            // The successor of a block this node found: the node may not
            // have that block yet, so it is not asked, and the last limit it
            // reported stands. ABLA moves the limit by a small fraction a
            // block.
            (_, Some((found, height))) if found == prev_hash => {
                self.enter_parent(prev_hash, height, None)
            }
            _ => match lookup_parent(self.bitcoind, prev_hash).await {
                Ok(summary) => {
                    self.enter_parent(prev_hash, summary.height + 1, summary.next_block_size_limit)
                }
                Err(reason) => {
                    error!("Template Provider template on parent {prev_hash} skipped: {reason}");
                    return Ok(());
                }
            },
        };
        let height = parent.height;
        let template_id = ready.announced.template_id;
        let now = SystemTimeProvider.seconds_since_epoch() as u32;
        match provided_block_template(
            ready,
            height,
            parent.size_limit,
            now,
            self.solutions_tx.clone(),
        ) {
            Ok(template) => {
                info!(
                    "Template Provider template {template_id} at height {height} with {} transactions",
                    template.transactions.len()
                );
                self.notify_tx
                    .send(NotifyCmd::SendToAll {
                        template: Arc::new(template),
                    })
                    .await
                    .map_err(|_| ConnectionEnd::NotifierClosed)?;
                self.published = true;
            }
            Err(invalid) => error!("{invalid}"),
        }
        Ok(())
    }

    /// Send `solution` to the provider, or to the node over RPC when it
    /// cannot go to the provider. A write that fails ends the connection.
    ///
    /// After the solution the coinbase output constraints are declared
    /// again. The specification lets a client send them at any time, and a
    /// provider that answers them with a fresh template (bchn-sv2-bridge
    /// does) announces the block's successor at once instead of at its next
    /// tip poll, so miners leave the old parent before they find a
    /// same-height sibling. Template Distribution has no message that asks
    /// for a template.
    async fn send_solution(&mut self, solution: Solution) -> Result<(), ConnectionEnd> {
        let block_hash = solution.block.block_hash();
        let Some(frame) = submit_solution_frame(&solution) else {
            warn!("Block {block_hash} does not fit SubmitSolution; submitting it over RPC");
            submit_block(&solution.block, self.bitcoind).await;
            return Ok(());
        };
        match self.write_frame(frame).await {
            Ok(()) => {
                info!(
                    "Sent block {block_hash} to the Template Provider for template {}",
                    solution.template_id
                );
                self.found_block = Some((block_hash, solution.height + 1));
                let constraints =
                    coinbase_output_constraints_frame().map_err(ConnectionEnd::Failed)?;
                self.write_frame(constraints).await
            }
            Err(failed) => {
                warn!(
                    "Block {block_hash} could not reach the Template Provider; submitting it over RPC"
                );
                submit_block(&solution.block, self.bitcoind).await;
                Err(failed)
            }
        }
    }
}

/// Height of `prev_hash` on the node and the size limit for a block on it,
/// retried while the node catches up.
async fn lookup_parent(
    bitcoind: &BitcoindRpcClient,
    prev_hash: BlockHash,
) -> Result<BlockHeaderSummary, String> {
    let hash = prev_hash.to_string();
    let mut result = bitcoind.get_block_header_summary(&hash).await;
    let mut attempt = 1;
    while result.is_err() && attempt < HEIGHT_ATTEMPTS {
        sleep(HEIGHT_RETRY_DELAY).await;
        result = bitcoind.get_block_header_summary(&hash).await;
        attempt += 1;
    }
    result.map_err(|error| format!("the node does not know the parent: {error}"))
}

#[cfg(test)]
mod tests;
