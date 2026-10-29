// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Stratum V2 Mining Protocol channels for a local miner.
//!
//! A channel reuses the SV1 machinery: its [`Session`] holds the extranonce
//! prefix, the miner identity and vardiff; each template becomes a tracker
//! job built by [`insert_miner_job`]; and a valid submission goes through the
//! same block submission and share-chain emission as an SV1 `mining.submit`.
//!
//! The coinbase this pool builds is `coinbase1 | enonce1 (4) | enonce2 (8) |
//! coinbase2`. An extended channel is given enonce1 as its extranonce prefix
//! and rolls the 8 enonce2 bytes itself; a standard channel is given all 12
//! bytes and mines the merkle root the pool computes from them.
//!
//! The channel's `user_identity` carries what an SV1 miner sends as username
//! and password: `<address>.<worker>`, then optionally a separator (comma,
//! space, tab or semicolon) and options in the SV1 password format. The
//! address is the miner's payout address. `p2p=<share address>` names the
//! owner of the channel's shares, with the SV1 rules: a share address the
//! node configures owns every share and a different `p2p=` is refused, and
//! without either a P2Poolv2 node refuses the channel. For example
//! `bchtest:qq...,p2p=tp2pool...` on chipnet.

use super::custom_job::validate_custom_job;
use super::session::DeclaredJob;
use super::session::{encode_extension_frame, encode_frame, string_field};
use crate::accounting::payout::simple_pplns::SimplePplnsShare;
use crate::config::{BitcoinCashNetwork, Chain, PoolMode};
use crate::stratum::difficulty_adjuster::{
    DifficultyAdjuster, DifficultyAdjusterTrait, TARGET_DRR, share_difficulty_ceiling,
};
use crate::stratum::merge_disclosure::{
    JOB_LEAVES, MERGE_DISCLOSURE_EXTENSION, WIN_PROOF, job_leaves_payload, win_proof_payload,
};
use crate::stratum::message_handlers::authorize_response::{
    resolve_cash_share, resolve_share_address,
};
use crate::stratum::message_handlers::submit::{
    PoolShare, emit_pool_share, get_true_difficulty, submit_found_block,
};
use crate::stratum::parse_password::{
    donation_percent, extract_string_value, log_unknown_options, parse_password,
    resolve_miner_donation, split_user_identity,
};
use crate::stratum::server::StratumContext;
use crate::stratum::session::Session;
use crate::stratum::validate_username;
use crate::stratum::work::difficulty::validate::build_coinbase_from_components;
use crate::stratum::work::gbt::compute_merkle_root_from_branches;
use crate::stratum::work::merge_tokens::MinerTokenJob;
use crate::stratum::work::prepared_notify::{PreparedNotifyParams, insert_miner_job};
use crate::stratum::work::tracker::{JobDetails, JobId};
use binary_sv2::{B032, B064K, Deserialize, Seq0255, Sv2Option, U256};
use bitcoin::block::{Header, Version};
use bitcoin::hashes::Hash;
use bitcoin::{BlockHash, CompactTarget, Network, Target, TxMerkleNode};
use mining_sv2::{
    CloseChannel, ERROR_CODE_OPEN_MINING_CHANNEL_CHANNEL_CAPACITY_EXHAUSTED,
    ERROR_CODE_OPEN_MINING_CHANNEL_MAX_TARGET_OUT_OF_RANGE,
    ERROR_CODE_SET_CUSTOM_MINING_JOB_STALE_CHAIN_TIP, ERROR_CODE_SUBMIT_SHARES_BAD_EXTRANONCE_SIZE,
    ERROR_CODE_SUBMIT_SHARES_INVALID_NON_ROLLABLE_VERSION_BIT,
    ERROR_CODE_UPDATE_CHANNEL_MAX_TARGET_OUT_OF_RANGE, MESSAGE_TYPE_CLOSE_CHANNEL,
    MESSAGE_TYPE_MINING_SET_NEW_PREV_HASH, MESSAGE_TYPE_NEW_EXTENDED_MINING_JOB,
    MESSAGE_TYPE_NEW_MINING_JOB, MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL,
    MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL_SUCCESS, MESSAGE_TYPE_OPEN_MINING_CHANNEL_ERROR,
    MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL, MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL_SUCCESS,
    MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_ERROR, MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_SUCCESS,
    MESSAGE_TYPE_SET_TARGET, MESSAGE_TYPE_SUBMIT_SHARES_ERROR, MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED,
    MESSAGE_TYPE_SUBMIT_SHARES_STANDARD, MESSAGE_TYPE_SUBMIT_SHARES_SUCCESS,
    MESSAGE_TYPE_UPDATE_CHANNEL, MESSAGE_TYPE_UPDATE_CHANNEL_ERROR, NewExtendedMiningJob,
    NewMiningJob, OpenExtendedMiningChannel, OpenExtendedMiningChannelSuccess,
    OpenMiningChannelError, OpenStandardMiningChannel, OpenStandardMiningChannelSuccess,
    SetCustomMiningJob, SetCustomMiningJobError, SetCustomMiningJobSuccess, SetNewPrevHash,
    SetTarget, SubmitSharesError, SubmitSharesExtended, SubmitSharesStandard, SubmitSharesSuccess,
    UpdateChannel, UpdateChannelError,
};
use p2poolv2_wallet::share_owner::ShareOwner;
use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::time::timeout;
use tracing::{debug, error, info};

/// Bytes of extranonce a miner rolls on an extended channel (SV1 enonce2).
pub const EXTRANONCE_SIZE: u16 = 8;

/// Jobs a channel remembers. Older ones are dropped; the tracker keeps a job
/// for 15 minutes, far longer than this many templates take to arrive.
const MAX_CHANNEL_JOBS: usize = 32;

/// The most bytes `NewExtendedMiningJob` carries in its coinbase prefix and
/// in its suffix: each is a `B0_64K` field in the SV2 specification.
const MAX_EXTENDED_JOB_COINBASE_PART: usize = 65_535;

/// Refuse a job whose coinbase an extended channel cannot carry.
///
/// A Bitcoin Cash coinbase that pays every miner directly may reach BCH's
/// 1,000,000-byte transaction limit, but an extended job carries the bytes
/// after the extranonce in one `B0_64K` field, about 1,900 P2PKH payouts.
/// Such a job is refused for the channel with this error rather than sent
/// truncated or malformed. Standard channels are header-only and never carry
/// the coinbase, so they keep working; a share-chain rule is not involved.
fn check_extended_job_fits(prefix_length: usize, suffix_length: usize) -> Result<(), String> {
    if prefix_length > MAX_EXTENDED_JOB_COINBASE_PART
        || suffix_length > MAX_EXTENDED_JOB_COINBASE_PART
    {
        return Err(format!(
            "the coinbase does not fit an SV2 extended job: its prefix is {prefix_length} and its \
             suffix {suffix_length} bytes, at most {MAX_EXTENDED_JOB_COINBASE_PART} each; a \
             standard channel can mine it"
        ));
    }
    Ok(())
}

/// Channels one connection may open. Every template builds a job per
/// channel, so an unbounded count would let one client make the node do
/// unbounded work; a proxy aggregating many devices uses extended channels.
pub const MAX_CHANNELS_PER_CONNECTION: usize = 32;

/// Message types this module handles.
pub fn is_mining_channel_message(message_type: u8) -> bool {
    matches!(
        message_type,
        MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL
            | MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL
            | MESSAGE_TYPE_SUBMIT_SHARES_STANDARD
            | MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED
            | MESSAGE_TYPE_UPDATE_CHANNEL
            | MESSAGE_TYPE_CLOSE_CHANNEL
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChannelKind {
    /// The pool fixes all 12 extranonce bytes and sends merkle roots.
    Standard,
    /// The miner rolls the 8 enonce2 bytes and builds the coinbase.
    Extended,
}

#[derive(Clone, Copy)]
struct ChannelJob {
    tracker_job_id: JobId,
    target: Target,
    difficulty: u64,
}

struct Channel {
    kind: ChannelKind,
    session: Session<DifficultyAdjuster>,
    /// enonce2 for a standard channel; unused on an extended one.
    standard_enonce2: [u8; 8],
    /// Easiest target the device accepts, from its latest open/update request.
    max_target: Target,
    /// Active jobs retain the target and credit assigned before hashing started.
    jobs: BTreeMap<u32, ChannelJob>,
    next_job_id: u32,
    /// Parent and nbits of the channel's latest `SetNewPrevHash`. The jobs
    /// carry no nbits, so new bits on the same parent need a new one too.
    last_tip: Option<(BlockHash, u32)>,
}

impl Channel {
    fn insert_job(&mut self, tracker_job_id: JobId) -> u32 {
        let job_id = self.next_job_id;
        self.next_job_id = self.next_job_id.wrapping_add(1);
        let target =
            target_for_difficulty(self.session.difficulty_adjuster.get_current_difficulty())
                .min(self.max_target);
        let difficulty = target
            .difficulty(Network::Bitcoin)
            .min(u128::from(u64::MAX)) as u64;
        self.jobs.insert(
            job_id,
            ChannelJob {
                tracker_job_id,
                target,
                difficulty,
            },
        );
        while self.jobs.len() > MAX_CHANNEL_JOBS {
            self.jobs.pop_first();
        }
        job_id
    }
}

/// The mining channels of one SV2 connection.
pub struct MiningChannels {
    ctx: StratumContext,
    version_mask: i32,
    channels: BTreeMap<u32, Channel>,
    next_channel_id: u32,
    /// The connection negotiated the merge-mining disclosure extension
    /// (`0x4D4D`): its channels get each job's token leaves and each win's
    /// proof (`stratum::merge_disclosure`).
    merge_disclosure: bool,
}

impl MiningChannels {
    /// Validate and install a declared BCH job before acknowledging reward
    /// eligibility. Its token must carry the channel's own donation.
    pub(crate) async fn install_custom_job(
        &mut self,
        request: &SetCustomMiningJob<'_>,
        declared: DeclaredJob,
        current: Option<Arc<PreparedNotifyParams>>,
    ) -> Result<Vec<Vec<u8>>, String> {
        if self.ctx.share_chain != Chain::BitcoinCash || self.ctx.mode != PoolMode::P2poolv2 {
            return custom_job_error(request, "unsupported-pool-mode");
        }
        let Some(channel) = self.channels.get_mut(&request.channel_id) else {
            return custom_job_error(request, "invalid-channel-id");
        };
        if channel.kind != ChannelKind::Extended {
            return custom_job_error(request, "invalid-channel-type");
        }
        let Some(current) = current else {
            return custom_job_error(request, "template-unavailable");
        };
        if current.template().previousblockhash != declared.prepared.template().previousblockhash {
            return custom_job_error(request, ERROR_CODE_SET_CUSTOM_MINING_JOB_STALE_CHAIN_TIP);
        }
        // The token's terms carry the donation of the identity that
        // allocated it; the job must pay what this channel's identity chose,
        // or the node's donation when it chose none.
        let chosen = channel.session.donation.or(current.donation()).unwrap_or(0);
        if declared.prepared.donation().unwrap_or(0) != chosen {
            debug!(
                "Rejected SV2 custom job: its token donates {} basis points, the channel {chosen}",
                declared.prepared.donation().unwrap_or(0)
            );
            return custom_job_error(request, "donation-mismatch");
        }
        let payout = channel
            .session
            .parsed_address
            .clone()
            .ok_or("authorized channel has no payout address")?;
        let owner = channel
            .session
            .share_owner
            .ok_or("authorized channel has no share owner")?;
        let (job, proposal) =
            match validate_custom_job(request, declared, payout, owner, self.version_mask) {
                Ok(validated) => validated,
                Err(reason) => {
                    debug!("Rejected SV2 custom job: {reason}");
                    return custom_job_error(request, "invalid-custom-job");
                }
            };
        if let Some(proposal) = proposal {
            match timeout(
                Duration::from_secs(10),
                self.ctx
                    .bitcoindrpc_client
                    .validate_block_proposal(&proposal),
            )
            .await
            {
                Ok(Ok(())) => (),
                rejected => {
                    debug!("Rejected declared BCH template: {rejected:?}");
                    return custom_job_error(request, "invalid-template");
                }
            }
        }
        let prev_hash = BlockHash::from_str(&job.blocktemplate.previousblockhash)
            .map_err(|error| error.to_string())?;
        let tip = (prev_hash, request.nbits);
        if channel.last_tip != Some(tip) {
            channel.jobs.clear();
        }
        channel.last_tip = Some(tip);
        let tracker_job_id = self.ctx.tracker_handle.get_next_job_id();
        self.ctx
            .tracker_handle
            .insert_job_details(tracker_job_id, job);
        let job_id = channel.insert_job(tracker_job_id);
        Ok(vec![encode_frame(
            SetCustomMiningJobSuccess {
                channel_id: request.channel_id,
                request_id: request.request_id,
                job_id,
            },
            MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_SUCCESS,
        )?])
    }

    /// Channels for one connection, sharing the SV1 stratum context.
    pub(crate) fn new(ctx: StratumContext, version_mask: i32) -> Self {
        Self {
            ctx,
            version_mask,
            channels: BTreeMap::new(),
            next_channel_id: 1,
            merge_disclosure: false,
        }
    }

    /// The connection negotiated the merge-mining disclosure extension:
    /// from now on its channels get each job's leaves (`JobLeaves`) and
    /// each win's proof (`WinProof`).
    pub(crate) fn grant_merge_disclosure(&mut self) {
        self.merge_disclosure = true;
    }

    /// What follows a job that commits a token root: its `JobLeaves` frame
    /// for a connection that negotiated the disclosure, and on a standard
    /// channel, whose device never sees its coinbase, a line in the node's
    /// leaf log.
    fn disclose_job(
        &self,
        channel_id: u32,
        kind: ChannelKind,
        job_id: u32,
        tokens: &MinerTokenJob,
    ) -> Result<Vec<Vec<u8>>, String> {
        if kind == ChannelKind::Standard
            && let Some(logs) = &self.ctx.merge_mining_logs
        {
            logs.log_standard_job(channel_id, job_id, tokens);
        }
        if !self.merge_disclosure {
            return Ok(Vec::new());
        }
        Ok(vec![encode_extension_frame(
            MERGE_DISCLOSURE_EXTENSION,
            JOB_LEAVES,
            true,
            &job_leaves_payload(channel_id, job_id, tokens),
        )?])
    }

    /// The proofs of the tokens a share on job `job_id` of `channel_id`
    /// wins: each logged with the node's wins, and sent as a `WinProof`
    /// frame to a connection that negotiated the disclosure. A share wins
    /// a token when its hash meets the token's target, whatever the
    /// channel's target.
    fn token_win_frames(
        &self,
        channel_id: u32,
        job_id: u32,
        job: &JobDetails,
        header: &Header,
        coinbase: &bitcoin::Transaction,
    ) -> Result<Vec<Vec<u8>>, String> {
        let Some(tokens) = &job.merge_tokens else {
            return Ok(Vec::new());
        };
        let wins = tokens.wins(
            header,
            coinbase,
            &job.template_merkle_branches,
            self.ctx.token_asert(),
        );
        let mut frames = Vec::with_capacity(wins.len());
        for win in &wins {
            info!(
                "Share {} on SV2 channel {channel_id} wins token {}",
                header.block_hash(),
                hex::encode(win.category)
            );
            if let Some(logs) = &self.ctx.merge_mining_logs {
                logs.log_sv2_win(channel_id, job_id, tokens, win);
            }
            if self.merge_disclosure {
                frames.push(encode_extension_frame(
                    MERGE_DISCLOSURE_EXTENSION,
                    WIN_PROOF,
                    true,
                    &win_proof_payload(channel_id, job_id, win),
                )?);
            }
        }
        Ok(frames)
    }

    /// Handle one mining channel message and return the response frames.
    ///
    /// `current` is the latest prepared template, used to send a newly
    /// opened channel its first job.
    pub async fn handle(
        &mut self,
        message_type: u8,
        payload: &mut [u8],
        current: Option<Arc<PreparedNotifyParams>>,
    ) -> Result<Vec<Vec<u8>>, String> {
        match message_type {
            MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL => {
                let request = OpenStandardMiningChannel::from_bytes(payload)
                    .map_err(|error| error.to_string())?;
                self.open(
                    OpenRequest {
                        kind: ChannelKind::Standard,
                        request_id: request.request_id,
                        user_identity: request.user_identity.as_utf8_or_hex(),
                        nominal_hash_rate: request.nominal_hash_rate,
                        max_target: request.max_target.as_bytes(),
                        min_extranonce_size: 0,
                    },
                    current,
                )
                .await
            }
            MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL => {
                let request = OpenExtendedMiningChannel::from_bytes(payload)
                    .map_err(|error| error.to_string())?;
                self.open(
                    OpenRequest {
                        kind: ChannelKind::Extended,
                        request_id: request.request_id,
                        user_identity: request.user_identity.as_utf8_or_hex(),
                        nominal_hash_rate: request.nominal_hash_rate,
                        max_target: request.max_target.as_bytes(),
                        min_extranonce_size: request.min_extranonce_size,
                    },
                    current,
                )
                .await
            }
            MESSAGE_TYPE_SUBMIT_SHARES_STANDARD => {
                let request =
                    SubmitSharesStandard::from_bytes(payload).map_err(|error| error.to_string())?;
                let enonce2 = match self.channels.get(&request.channel_id) {
                    Some(channel) => channel.standard_enonce2,
                    None => [0u8; 8],
                };
                self.submit(Submission {
                    channel_id: request.channel_id,
                    sequence_number: request.sequence_number,
                    job_id: request.job_id,
                    nonce: request.nonce,
                    ntime: request.ntime,
                    version: request.version,
                    enonce2: enonce2.to_vec(),
                })
                .await
            }
            MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED => {
                let request =
                    SubmitSharesExtended::from_bytes(payload).map_err(|error| error.to_string())?;
                self.submit(Submission {
                    channel_id: request.channel_id,
                    sequence_number: request.sequence_number,
                    job_id: request.job_id,
                    nonce: request.nonce,
                    ntime: request.ntime,
                    version: request.version,
                    enonce2: request.extranonce.as_bytes().to_vec(),
                })
                .await
            }
            MESSAGE_TYPE_UPDATE_CHANNEL => {
                let request =
                    UpdateChannel::from_bytes(payload).map_err(|error| error.to_string())?;
                self.update(request.channel_id, request.maximum_target.as_bytes())
            }
            MESSAGE_TYPE_CLOSE_CHANNEL => {
                let request =
                    CloseChannel::from_bytes(payload).map_err(|error| error.to_string())?;
                self.close_channel(request.channel_id).await;
                Ok(Vec::new())
            }
            _ => Err(format!("not a mining channel message: {message_type}")),
        }
    }

    async fn close_channel(&mut self, channel_id: u32) {
        if let Some(channel) = self.channels.remove(&channel_id)
            && let Err(metrics_error) = self
                .ctx
                .metrics
                .decrement_worker_count(
                    channel.session.btcaddress,
                    channel.session.workername.unwrap_or_default(),
                )
                .await
        {
            error!("Failed to record closed SV2 worker: {metrics_error}");
        }
    }

    /// Release every authorized worker when the connection ends.
    pub(crate) async fn close_all(&mut self) {
        while let Some(channel_id) = self.channels.keys().next().copied() {
            self.close_channel(channel_id).await;
        }
    }

    /// Apply a device's new `maximum_target` and answer with the channel's
    /// target, capped by it. Proxies such as the reference translator send
    /// `UpdateChannel` as their hashrate changes. The nominal hashrate is not
    /// used: vardiff sets the target from submitted shares, as on SV1.
    fn update(&mut self, channel_id: u32, maximum_target: &[u8]) -> Result<Vec<Vec<u8>>, String> {
        let Some(channel) = self.channels.get_mut(&channel_id) else {
            return Ok(vec![encode_frame(
                UpdateChannelError {
                    channel_id,
                    error_code: string_field("invalid-channel-id")?,
                },
                MESSAGE_TYPE_UPDATE_CHANNEL_ERROR,
            )?]);
        };
        let maximum_target = device_max_target(maximum_target);
        if maximum_target == Target::ZERO {
            return Ok(vec![encode_frame(
                UpdateChannelError {
                    channel_id,
                    error_code: string_field(ERROR_CODE_UPDATE_CHANNEL_MAX_TARGET_OUT_OF_RANGE)?,
                },
                MESSAGE_TYPE_UPDATE_CHANNEL_ERROR,
            )?]);
        }
        channel.max_target = maximum_target;
        let target =
            target_for_difficulty(channel.session.difficulty_adjuster.get_current_difficulty())
                .min(channel.max_target)
                .to_le_bytes();
        Ok(vec![encode_frame(
            SetTarget {
                channel_id,
                maximum_target: u256(&target)?,
            },
            MESSAGE_TYPE_SET_TARGET,
        )?])
    }

    /// Send every open channel a job for `prepared`.
    ///
    /// A coinbase that exceeds an extended channel's wire fields is logged
    /// and skipped only for that channel. Standard channels still receive
    /// their header jobs, and a later template can fit the extended channel.
    pub fn on_template(&mut self, prepared: &PreparedNotifyParams) -> Vec<Vec<u8>> {
        let channel_ids: Vec<u32> = self.channels.keys().copied().collect();
        let mut frames = Vec::with_capacity(channel_ids.len() * 2);
        for channel_id in channel_ids {
            match self.job_frames(channel_id, prepared) {
                Ok(job_frames) => frames.extend(job_frames),
                Err(job_error) => {
                    error!("Failed to build SV2 job for channel {channel_id}: {job_error}")
                }
            }
        }
        frames
    }

    async fn open(
        &mut self,
        request: OpenRequest<'_>,
        current: Option<Arc<PreparedNotifyParams>>,
    ) -> Result<Vec<Vec<u8>>, String> {
        let OpenRequest {
            kind,
            request_id,
            user_identity,
            nominal_hash_rate,
            max_target,
            min_extranonce_size,
        } = request;
        if self.channels.len() >= MAX_CHANNELS_PER_CONNECTION {
            return open_error(
                request_id,
                ERROR_CODE_OPEN_MINING_CHANNEL_CHANNEL_CAPACITY_EXHAUSTED,
            );
        }
        if kind == ChannelKind::Extended && min_extranonce_size > EXTRANONCE_SIZE {
            return open_error(request_id, "min-extranonce-size-too-large");
        }
        let device_max_target = device_max_target(max_target);
        if device_max_target == Target::ZERO {
            return open_error(
                request_id,
                ERROR_CODE_OPEN_MINING_CHANNEL_MAX_TARGET_OUT_OF_RANGE,
            );
        }
        let mut session = Session::<DifficultyAdjuster>::new(
            initial_difficulty(
                nominal_hash_rate,
                self.ctx.start_difficulty,
                self.ctx.minimum_difficulty,
                self.ctx.maximum_difficulty,
            ),
            self.ctx.minimum_difficulty,
            self.ctx.maximum_difficulty,
            self.version_mask,
        );
        let (username, options) = split_user_identity(&user_identity);
        let validated = match validate_username::validate_for_chain(
            username,
            self.ctx.validate_addresses,
            self.ctx.network,
            self.ctx.share_chain,
            self.ctx
                .bch_share_network
                .map(BitcoinCashNetwork::from_share_network),
        ) {
            Ok(validated) => validated,
            Err(_) => return open_error(request_id, "invalid-user-identity"),
        };
        let share_owner = match self.resolve_share_owner(options) {
            Ok(share_owner) => share_owner,
            Err(reason) => {
                debug!("Refusing SV2 channel for {username}: {reason}");
                let code = if options
                    .and_then(|options| extract_string_value(options, "p2p="))
                    .is_some()
                {
                    "invalid-share-address"
                } else {
                    "share-address-required"
                };
                return open_error(request_id, code);
            }
        };
        if let Some(options) = options {
            log_unknown_options(options);
        }
        let donation = match resolve_miner_donation(options, self.ctx.share_chain, self.ctx.mode) {
            Ok(donation) => donation,
            Err(reason) => {
                debug!("Refusing SV2 channel for {username}: {reason}");
                return open_error(request_id, "invalid-donation");
            }
        };
        if kind == ChannelKind::Extended
            && let Some(prepared) = &current
        {
            // The coinbase this channel mines carries its own donation.
            let miner_prepared = match prepared.for_miner_donation(donation) {
                Ok(miner_prepared) => miner_prepared,
                Err(reason) => {
                    error!("Refusing SV2 extended channel for {username}: {reason}");
                    return open_error(request_id, "invalid-donation");
                }
            };
            let (prefix_length, suffix_length) = miner_prepared
                .as_deref()
                .unwrap_or(prepared)
                .coinbase_part_lengths();
            if let Err(reason) = check_extended_job_fits(prefix_length, suffix_length) {
                error!("Refusing SV2 extended channel for {username}: {reason}");
                return open_error(request_id, "coinbase-too-large");
            }
        }
        let user_id = self
            .ctx
            .chain_store_handle
            .add_user(validated.user_key.to_string())
            .await
            .map_err(|error| format!("Failed to store user: {error}"))?;
        session.username = Some(username.to_string());
        session.btcaddress = Some(validated.user_key.to_string());
        session.workername = validated.worker_name.map(|name| name.to_string());
        session.parsed_address = validated.parsed_address;
        session.share_owner = share_owner;
        session.donation = donation;
        if let Some(basis_points) = donation {
            debug!(
                "SV2 channel for {username} mines with a {} P2Pool donation",
                donation_percent(basis_points)
            );
        }
        session.user_id = Some(user_id);
        session.subscribed = true;
        // Open at or below the share difficulty of the job about to be sent,
        // so the first job needs no `SetTarget` (see `job_frames`).
        if let Some(prepared) = &current {
            session
                .difficulty_adjuster
                .set_share_difficulty_ceiling(self.share_difficulty_ceiling(prepared));
        }
        let channel_target =
            target_for_difficulty(session.difficulty_adjuster.get_current_difficulty())
                .min(device_max_target);
        let enonce1 = session.enonce1.to_le_bytes();
        let standard_enonce2 = standard_enonce2(self.next_channel_id);
        let channel_id = self.next_channel_id;
        self.next_channel_id = self.next_channel_id.wrapping_add(1);
        let mut frames = Vec::with_capacity(3);
        let target_bytes = channel_target.to_le_bytes();
        match kind {
            ChannelKind::Standard => {
                let mut prefix = Vec::with_capacity(12);
                prefix.extend_from_slice(&enonce1);
                prefix.extend_from_slice(&standard_enonce2);
                frames.push(encode_frame(
                    OpenStandardMiningChannelSuccess {
                        request_id,
                        channel_id,
                        target: u256(&target_bytes)?,
                        extranonce_prefix: b032(&prefix)?,
                        group_channel_id: 0,
                    },
                    MESSAGE_TYPE_OPEN_STANDARD_MINING_CHANNEL_SUCCESS,
                )?);
            }
            ChannelKind::Extended => {
                frames.push(encode_frame(
                    OpenExtendedMiningChannelSuccess {
                        request_id,
                        channel_id,
                        target: u256(&target_bytes)?,
                        extranonce_size: EXTRANONCE_SIZE,
                        extranonce_prefix: b032(&enonce1)?,
                        group_channel_id: 0,
                    },
                    MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL_SUCCESS,
                )?);
            }
        }
        self.channels.insert(
            channel_id,
            Channel {
                kind,
                session,
                standard_enonce2,
                max_target: device_max_target,
                jobs: BTreeMap::new(),
                next_job_id: 1,
                last_tip: None,
            },
        );
        if let Err(metrics_error) = self
            .ctx
            .metrics
            .increment_worker_count(
                validated.user_key.to_string(),
                validated.worker_name.unwrap_or_default().to_string(),
            )
            .await
        {
            error!("Failed to send increment worker count message: {metrics_error}");
        }
        if let Some(prepared) = current {
            frames.extend(self.job_frames(channel_id, &prepared)?);
        }
        Ok(frames)
    }

    /// Owner of a channel's shares from the node's configured share address
    /// and the identity's `p2p=` option, by the SV1 authorize rules.
    ///
    /// `Ok(None)` only in Hydrapool mode, which builds no share commitment.
    fn resolve_share_owner(&self, options: Option<&str>) -> Result<Option<ShareOwner>, String> {
        match self.ctx.share_chain {
            Chain::Bitcoin => {
                let parsed = options.map(parse_password);
                resolve_share_address(
                    self.ctx.miner_address,
                    parsed
                        .as_ref()
                        .and_then(|parsed| parsed.miner_address.as_ref()),
                    self.ctx.network,
                    self.ctx.mode,
                )
                .map(|address| {
                    address.map(|address| ShareOwner::Taproot(address.witness_program()))
                })
            }
            Chain::BitcoinCash => {
                let network = self
                    .ctx
                    .bch_share_network
                    .ok_or("Bitcoin Cash share addresses need a network")?;
                resolve_cash_share(self.ctx.cash_share, options, network, self.ctx.mode)
            }
        }
    }

    /// Share difficulty ceiling of jobs built on `prepared`: the session
    /// difficulty a channel may have without losing share-chain shares (see
    /// [`share_difficulty_ceiling`]). `None` in Hydrapool mode, which has no
    /// share chain.
    fn share_difficulty_ceiling(&self, prepared: &PreparedNotifyParams) -> Option<u64> {
        match self.ctx.mode {
            PoolMode::Hydrapool => None,
            PoolMode::P2poolv2 => share_difficulty_ceiling(prepared.bits()),
        }
    }

    /// Insert a tracker job for `channel_id` and build its SV2 frames.
    ///
    /// When the channel's difficulty is above the job's share difficulty, it
    /// comes down to it first and a `SetTarget` precedes the job, so the
    /// device throws away no hash that is a share-chain share.
    /// Extended coinbase limits are checked before the channel or tracker
    /// changes; an unrepresentable job leaves the channel ready to retry.
    fn job_frames(
        &mut self,
        channel_id: u32,
        prepared: &PreparedNotifyParams,
    ) -> Result<Vec<Vec<u8>>, String> {
        let is_hydrapool = self.ctx.mode == PoolMode::Hydrapool;
        let ceiling = self.share_difficulty_ceiling(prepared);
        let channel = self
            .channels
            .get_mut(&channel_id)
            .ok_or("job for an unknown channel")?;
        // The channel's own donation, when its identity chose one.
        let miner_prepared = prepared
            .for_miner_donation(channel.session.donation)
            .map_err(|error| error.to_string())?;
        let prepared = miner_prepared.as_deref().unwrap_or(prepared);
        if channel.kind == ChannelKind::Extended {
            let (prefix_length, suffix_length) = prepared.coinbase_part_lengths();
            check_extended_job_fits(prefix_length, suffix_length)?;
        }
        let mut frames = Vec::with_capacity(3);
        if let Some(lowered) = channel
            .session
            .difficulty_adjuster
            .set_share_difficulty_ceiling(ceiling)
        {
            let target = target_for_difficulty(lowered)
                .min(channel.max_target)
                .to_le_bytes();
            frames.push(encode_frame(
                SetTarget {
                    channel_id,
                    maximum_target: u256(&target)?,
                },
                MESSAGE_TYPE_SET_TARGET,
            )?);
        }
        let (bitcoin_address, share_owner) = if is_hydrapool {
            (None, None)
        } else {
            (
                channel.session.parsed_address.as_ref(),
                channel.session.share_owner,
            )
        };
        let (tracker_job_id, _coinbase2) = insert_miner_job(
            prepared,
            bitcoin_address,
            share_owner,
            &self.ctx.tracker_handle,
        );
        let job = self
            .ctx
            .tracker_handle
            .get_job(tracker_job_id)
            .ok_or("tracker lost a job it just inserted")?;
        let template = &job.blocktemplate;
        let prev_hash =
            BlockHash::from_str(&template.previousblockhash).map_err(|error| error.to_string())?;
        let nbits = u32::from_str_radix(&template.bits, 16).map_err(|error| error.to_string())?;
        let version = template.version as u32;
        let new_prev_hash = channel.last_tip != Some((prev_hash, nbits));
        if new_prev_hash {
            channel.jobs.clear();
        }
        let channel_job_id = channel.insert_job(tracker_job_id);
        channel.last_tip = Some((prev_hash, nbits));
        let min_ntime = if new_prev_hash {
            Sv2Option::new(None)
        } else {
            Sv2Option::new(Some(template.curtime))
        };

        match channel.kind {
            ChannelKind::Standard => {
                let enonce1_hex = channel.session.enonce1_hex.clone();
                let merkle_root =
                    standard_merkle_root(&job, &enonce1_hex, &channel.standard_enonce2)?;
                frames.push(encode_frame(
                    NewMiningJob {
                        channel_id,
                        job_id: channel_job_id,
                        min_ntime,
                        version,
                        merkle_root: u256(merkle_root.as_byte_array())?,
                    },
                    MESSAGE_TYPE_NEW_MINING_JOB,
                )?);
            }
            ChannelKind::Extended => {
                let prefix = hex::decode(&job.coinbase1).map_err(|error| error.to_string())?;
                let suffix = hex::decode(&job.coinbase2).map_err(|error| error.to_string())?;
                let path = job
                    .template_merkle_branches
                    .iter()
                    .map(|branch| {
                        U256::new(branch.as_byte_array()).map_err(|error| error.to_string())
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                frames.push(encode_frame(
                    NewExtendedMiningJob {
                        channel_id,
                        job_id: channel_job_id,
                        min_ntime,
                        version,
                        version_rolling_allowed: self.version_mask != 0,
                        merkle_path: Seq0255::new(path).map_err(|error| error.to_string())?,
                        coinbase_tx_prefix: B064K::new(&prefix)
                            .map_err(|error| error.to_string())?,
                        coinbase_tx_suffix: B064K::new(&suffix)
                            .map_err(|error| error.to_string())?,
                    },
                    MESSAGE_TYPE_NEW_EXTENDED_MINING_JOB,
                )?);
            }
        }
        if new_prev_hash {
            frames.push(encode_frame(
                SetNewPrevHash {
                    channel_id,
                    job_id: channel_job_id,
                    prev_hash: u256(prev_hash.as_byte_array())?,
                    min_ntime: template.curtime,
                    nbits,
                },
                MESSAGE_TYPE_MINING_SET_NEW_PREV_HASH,
            )?);
        }
        let kind = channel.kind;
        if let Some(tokens) = &job.merge_tokens {
            frames.extend(self.disclose_job(channel_id, kind, channel_job_id, tokens)?);
        }
        Ok(frames)
    }

    async fn submit(&mut self, submission: Submission) -> Result<Vec<Vec<u8>>, String> {
        let Some(channel) = self.channels.get_mut(&submission.channel_id) else {
            return submit_error(&submission, "invalid-channel-id");
        };
        let Some(channel_job) = channel.jobs.get(&submission.job_id).copied() else {
            return submit_error(&submission, "invalid-job-id");
        };
        let tracker_job_id = channel_job.tracker_job_id;
        let Some(job) = self.ctx.tracker_handle.get_job(tracker_job_id) else {
            return submit_error(&submission, "stale-share");
        };
        if submission.enonce2.len() != usize::from(EXTRANONCE_SIZE) {
            return submit_error(&submission, ERROR_CODE_SUBMIT_SHARES_BAD_EXTRANONCE_SIZE);
        }
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_secs();
        if submission.ntime < job.blocktemplate.curtime || u64::from(submission.ntime) > now + 7200
        {
            return submit_error(&submission, "invalid-ntime");
        }
        let template_version = job.blocktemplate.version;
        let rolled = submission.version as i32;
        if (rolled ^ template_version) & !self.version_mask != 0 {
            return submit_error(
                &submission,
                ERROR_CODE_SUBMIT_SHARES_INVALID_NON_ROLLABLE_VERSION_BIT,
            );
        }
        let enonce2_hex = hex::encode(&submission.enonce2);
        let coinbase = build_coinbase_from_components(
            &job.coinbase1,
            &channel.session.enonce1_hex,
            &enonce2_hex,
            &job.coinbase2,
        )
        .map_err(|error| error.to_string())?;
        let header = Header {
            version: Version::from_consensus(rolled),
            prev_blockhash: BlockHash::from_str(&job.blocktemplate.previousblockhash)
                .map_err(|error| error.to_string())?,
            merkle_root: compute_merkle_root_from_branches(
                coinbase.compute_txid(),
                &job.template_merkle_branches,
            ),
            time: submission.ntime,
            bits: CompactTarget::from_unprefixed_hex(&job.blocktemplate.bits)
                .map_err(|error| error.to_string())?,
            nonce: submission.nonce,
        };
        let block_hash = header.block_hash();
        if !self
            .ctx
            .tracker_handle
            .add_share(tracker_job_id, block_hash)
        {
            return submit_error(&submission, "duplicate-share");
        }
        // As for SV1, a header that meets the network target is submitted
        // whatever the channel target, and a share that meets the pool target
        // goes to the share chain. The channel target only decides the reply
        // and the accounting below.
        if Target::from_compact(header.bits).is_met_by(block_hash) {
            submit_found_block(&self.ctx, header, coinbase.clone(), &job)
                .await
                .map_err(|error| error.to_string())?;
        }
        let difficulty = channel_job.difficulty;
        let share = SimplePplnsShare::new(
            channel.session.user_id.unwrap_or_default(),
            difficulty,
            channel.session.btcaddress.clone().unwrap_or_default(),
            channel.session.workername.clone().unwrap_or_default(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_secs())
                .unwrap_or_default(),
            format!("{:016x}", tracker_job_id.0),
            enonce2_hex.clone(),
            format!("{:08x}", submission.nonce),
        );
        // The share is credited at the job's channel difficulty, so Hydrapool
        // stores it only when its hash meets the channel target, as SV1 does
        // with the session difficulty. `ignore_difficulty` never opens this.
        let hash_meets_channel_target = channel_job.target.is_met_by(block_hash);
        emit_pool_share(
            &self.ctx,
            PoolShare {
                job_id: tracker_job_id,
                job: &job,
                header,
                block_hash,
                coinbase: &coinbase,
                share: share.clone(),
                meets_credited_difficulty: hash_meets_channel_target,
            },
        )
        .await
        .map_err(|error| error.to_string())?;
        channel.session.last_share_time = Some(SystemTime::now());
        let truediff = get_true_difficulty(&block_hash);
        let meets_channel_target = self.ctx.ignore_difficulty || hash_meets_channel_target;
        if meets_channel_target {
            let _ = self
                .ctx
                .metrics
                .record_share_accepted(share, truediff as u64)
                .await;
        } else {
            let _ = self.ctx.metrics.record_share_rejected().await;
        }
        let (new_difficulty, _is_first_share) = channel
            .session
            .difficulty_adjuster
            .record_share_submission(truediff, tracker_job_id.0, None, SystemTime::now());
        let mut frames = Vec::with_capacity(2);
        if meets_channel_target {
            frames.push(encode_frame(
                SubmitSharesSuccess {
                    channel_id: submission.channel_id,
                    last_sequence_number: submission.sequence_number,
                    new_submits_accepted_count: 1,
                    new_shares_sum: difficulty,
                },
                MESSAGE_TYPE_SUBMIT_SHARES_SUCCESS,
            )?);
        } else {
            frames.extend(submit_error(&submission, "difficulty-too-low")?);
        }
        if let Some(new_difficulty) = new_difficulty {
            let target = target_for_difficulty(new_difficulty)
                .min(channel.max_target)
                .to_le_bytes();
            frames.push(encode_frame(
                SetTarget {
                    channel_id: submission.channel_id,
                    maximum_target: u256(&target)?,
                },
                MESSAGE_TYPE_SET_TARGET,
            )?);
        }
        debug!(
            "SV2 share on channel {}: meets channel target {meets_channel_target}",
            submission.channel_id
        );
        frames.extend(self.token_win_frames(
            submission.channel_id,
            submission.job_id,
            &job,
            &header,
            &coinbase,
        )?);
        Ok(frames)
    }
}

pub(crate) fn custom_job_error(
    request: &SetCustomMiningJob<'_>,
    code: &'static str,
) -> Result<Vec<Vec<u8>>, String> {
    Ok(vec![encode_frame(
        SetCustomMiningJobError {
            channel_id: request.channel_id,
            request_id: request.request_id,
            error_code: string_field(code)?,
        },
        MESSAGE_TYPE_SET_CUSTOM_MINING_JOB_ERROR,
    )?])
}

struct Submission {
    channel_id: u32,
    sequence_number: u32,
    job_id: u32,
    nonce: u32,
    ntime: u32,
    version: u32,
    enonce2: Vec<u8>,
}

/// Target for a stratum difficulty, as SV1 `mining.set_difficulty` means it:
/// difficulty 1 is the mainnet difficulty-1 target, and difficulty 0 is
/// treated as 1.
pub(crate) fn target_for_difficulty(difficulty: u64) -> Target {
    let divisor = u128::from(difficulty.max(1));
    let dividend = Target::MAX_ATTAINABLE_MAINNET.to_be_bytes();
    let mut quotient = [0u8; 32];
    let mut remainder: u128 = 0;
    for (index, byte) in dividend.iter().enumerate() {
        remainder = (remainder << 8) | u128::from(*byte);
        quotient[index] = (remainder / divisor) as u8;
        remainder %= divisor;
    }
    Target::from_be_bytes(quotient)
}

/// The `max_target` of an open request: a little-endian U256. A malformed
/// one (not 32 bytes) leaves the channel uncapped.
fn device_max_target(max_target: &[u8]) -> Target {
    match <[u8; 32]>::try_from(max_target) {
        Ok(bytes) => Target::from_le_bytes(bytes),
        Err(_) => Target::MAX_ATTAINABLE_MAINNET,
    }
}

/// What an `OpenStandardMiningChannel` or `OpenExtendedMiningChannel`
/// asks for. A standard channel asks for no extranonce space.
struct OpenRequest<'a> {
    kind: ChannelKind,
    request_id: u32,
    user_identity: String,
    nominal_hash_rate: f32,
    max_target: &'a [u8],
    min_extranonce_size: u16,
}

/// Hashes a difficulty-1 share takes on average.
const HASHES_PER_DIFFICULTY: f64 = 4_294_967_296.0;

/// The first difficulty of a channel whose client declares
/// `nominal_hash_rate` hashes a second: the difficulty the adjuster settles
/// on for that rate, within the node's minimum and maximum. The SV2 spec has
/// the pool set a channel's first target from this rate, so a small device
/// is not left at a difficulty it needs days to meet. A client that declares
/// nothing usable starts at the node's `start_difficulty`.
fn initial_difficulty(
    nominal_hash_rate: f32,
    start_difficulty: u64,
    minimum_difficulty: u64,
    maximum_difficulty: Option<u64>,
) -> u64 {
    if !nominal_hash_rate.is_finite() || nominal_hash_rate <= 0.0 {
        return start_difficulty;
    }
    let shares_per_second = f64::from(nominal_hash_rate) / HASHES_PER_DIFFICULTY;
    let settled = (shares_per_second / TARGET_DRR).round();
    let difficulty = if settled >= u64::MAX as f64 {
        u64::MAX
    } else {
        settled as u64
    };
    let difficulty = difficulty.max(minimum_difficulty);
    maximum_difficulty.map_or(difficulty, |maximum| difficulty.min(maximum))
}

/// enonce2 the pool fixes for a standard channel. Each channel of a
/// connection has its own random enonce1, so this only has to differ between
/// channels on one connection.
fn standard_enonce2(channel_id: u32) -> [u8; 8] {
    u64::from(channel_id).to_le_bytes()
}

/// Merkle root a standard channel mines: the job's coinbase with the
/// channel's full 12-byte extranonce, folded with the template branches.
fn standard_merkle_root(
    job: &JobDetails,
    enonce1_hex: &str,
    enonce2: &[u8; 8],
) -> Result<TxMerkleNode, String> {
    let coinbase = build_coinbase_from_components(
        &job.coinbase1,
        enonce1_hex,
        &hex::encode(enonce2),
        &job.coinbase2,
    )
    .map_err(|error| error.to_string())?;
    Ok(compute_merkle_root_from_branches(
        coinbase.compute_txid(),
        &job.template_merkle_branches,
    ))
}

fn u256(bytes: &[u8]) -> Result<U256<'_>, String> {
    U256::new(bytes).map_err(|error| error.to_string())
}

fn b032(bytes: &[u8]) -> Result<B032<'_>, String> {
    B032::new(bytes).map_err(|error| error.to_string())
}

fn open_error(request_id: u32, error_code: &'static str) -> Result<Vec<Vec<u8>>, String> {
    Ok(vec![encode_frame(
        OpenMiningChannelError {
            request_id,
            error_code: string_field(error_code)?,
        },
        MESSAGE_TYPE_OPEN_MINING_CHANNEL_ERROR,
    )?])
}

fn submit_error(submission: &Submission, error_code: &'static str) -> Result<Vec<Vec<u8>>, String> {
    Ok(vec![encode_frame(
        SubmitSharesError {
            channel_id: submission.channel_id,
            sequence_number: submission.sequence_number,
            error_code: string_field(error_code)?,
        },
        MESSAGE_TYPE_SUBMIT_SHARES_ERROR,
    )?])
}

#[cfg(test)]
mod tests;
