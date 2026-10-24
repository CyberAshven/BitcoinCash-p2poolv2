// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Telling miners about the tokens a node merge-mines for them: the leaves
//! behind each of their jobs, and the proofs of the tokens their shares win.
//!
//! A miner on someone else's pool checks before hashing that their leaves
//! pay them, and only the donation and fee they agreed to, and claims their
//! own wins. The formats are Pickaxe's (pickaxe-miner PR #70 at a92fef7,
//! `docs/merge-mining.md`, "Disclosure"), so miners and tools work at both
//! kinds of pool. Disclosure is opt-in: firmware that does not ask sees
//! nothing new.
//!
//! - **Stratum V2**: the extension `0x4D4D` ("MM"), asked for through the
//!   extensions negotiation (type `0x0001`, `RequestExtensions` `0x00`,
//!   answered by `RequestExtensions.Success` `0x01` with the granted types).
//!   A connection that negotiated it gets, on the channel bit, a `JobLeaves`
//!   frame after each job that commits a root, and a `WinProof` frame after
//!   each share that wins a token.
//! - **Stratum V1**: the `mining.configure` extension `pickaxe-merge`, then
//!   `pickaxe.merge_leaves` after each job and `pickaxe.merge_proof` on a
//!   win.
//! - **Standard channels and every win**: owner-only JSON line logs beside
//!   the token file ([`MergeMiningLogs`]), which the operator may publish
//!   or hand to miners. A standard channel's device never sees its coinbase,
//!   so its leaves are a trust assumption the log lets anyone audit.
//!
//! ```text
//! JobLeaves (0x00):  channel u32 | job u32 | output 0 (53) | u16 n | CTMD (n)
//!                    | each leaf's aux branch, in CTMD order (32 x h, leaf to root)
//! WinProof  (0x01):  channel u32 | job u32 | category (32) | u16 n | AuxProof v2 (n)
//!                    | u8 m (0 or 69) | the split preimage (m)
//! ```

use crate::shares::auxpow::TOKEN_SPLIT_PREIMAGE_LENGTH;
use crate::stratum::work::merge_tokens::{MinerTokenJob, TokenWin};
use serde_json::{Value, json};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{error, warn};

/// The Stratum V2 extension that discloses merge-mined leaves and wins:
/// `0x4D4D`, "MM", Pickaxe's choice while SV2 has no registry of private
/// extension types.
pub const MERGE_DISCLOSURE_EXTENSION: u16 = 0x4D4D;
/// The Stratum V2 extensions negotiation's extension type.
pub const EXTENSIONS_NEGOTIATION: u16 = 0x0001;
/// `RequestExtensions`: `u16 request_id | u16 n | n x u16`.
pub const REQUEST_EXTENSIONS: u8 = 0x00;
/// `RequestExtensions.Success`: the request id and the granted types.
pub const REQUEST_EXTENSIONS_SUCCESS: u8 = 0x01;
/// The disclosure extension's message of a job's leaves.
pub const JOB_LEAVES: u8 = 0x00;
/// The disclosure extension's message of a share's token win.
pub const WIN_PROOF: u8 = 0x01;

/// The `mining.configure` extension an SV1 miner asks for the disclosure
/// with.
pub const SV1_MERGE_EXTENSION: &str = "pickaxe-merge";
/// The SV1 notification of a job's leaves.
pub const SV1_MERGE_LEAVES: &str = "pickaxe.merge_leaves";
/// The SV1 notification of a token win's proof.
pub const SV1_MERGE_PROOF: &str = "pickaxe.merge_proof";

/// A log moves to `.1` and starts again past this many bytes.
pub const MAX_LOG_BYTES: u64 = 8 * 1024 * 1024;

/// Lines waiting for a log's writer before more are dropped, so a miner's
/// connection never waits on the disk.
const LOG_QUEUE_LINES: usize = 1024;

/// The extensions a `RequestExtensions` payload asks for: its request id
/// and the extension types. `None` when the payload is not exactly one.
pub fn requested_extensions(payload: &[u8]) -> Option<(u16, Vec<u16>)> {
    let request = u16::from_le_bytes(payload.get(..2)?.try_into().ok()?);
    let count = usize::from(u16::from_le_bytes(payload.get(2..4)?.try_into().ok()?));
    let list = payload.get(4..)?;
    if list.len() != 2 * count {
        return None;
    }
    Some((
        request,
        list.as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect(),
    ))
}

/// A `RequestExtensions` or `RequestExtensions.Success` payload.
pub fn extensions_payload(request: u16, extensions: &[u16]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(4 + 2 * extensions.len());
    bytes.extend_from_slice(&request.to_le_bytes());
    bytes.extend_from_slice(&(extensions.len() as u16).to_le_bytes());
    extensions
        .iter()
        .for_each(|extension| bytes.extend_from_slice(&extension.to_le_bytes()));
    bytes
}

/// The `JobLeaves` payload of `tokens`, the leaves behind job `job` of
/// channel `channel`.
pub fn job_leaves_payload(channel: u32, job: u32, tokens: &MinerTokenJob) -> Vec<u8> {
    let disclosure = tokens.disclosure().encode();
    let mut bytes = Vec::with_capacity(8 + 53 + 2 + disclosure.len() + 32 * 16 * 16);
    bytes.extend_from_slice(&channel.to_le_bytes());
    bytes.extend_from_slice(&job.to_le_bytes());
    bytes.extend_from_slice(&tokens.output());
    bytes.extend_from_slice(&(disclosure.len() as u16).to_le_bytes());
    bytes.extend_from_slice(&disclosure);
    for branch in tokens.branches() {
        branch.iter().for_each(|hash| bytes.extend_from_slice(hash));
    }
    bytes
}

/// The `WinProof` payload of `win`, won by a share on job `job` of channel
/// `channel`.
pub fn win_proof_payload(channel: u32, job: u32, win: &TokenWin) -> Vec<u8> {
    let mut bytes =
        Vec::with_capacity(8 + 32 + 2 + win.proof.len() + 1 + TOKEN_SPLIT_PREIMAGE_LENGTH);
    bytes.extend_from_slice(&channel.to_le_bytes());
    bytes.extend_from_slice(&job.to_le_bytes());
    bytes.extend_from_slice(&win.category);
    bytes.extend_from_slice(&(win.proof.len() as u16).to_le_bytes());
    bytes.extend_from_slice(&win.proof);
    if win.split.is_none() {
        bytes.push(0);
    } else {
        bytes.push(TOKEN_SPLIT_PREIMAGE_LENGTH as u8);
        bytes.extend_from_slice(&win.split.preimage());
    }
    bytes
}

/// The split preimage a claim reveals, in hex, or empty without a split.
fn split_hex(win: &TokenWin) -> String {
    if win.split.is_none() {
        String::new()
    } else {
        hex::encode(win.split.preimage())
    }
}

/// The params of `pickaxe.merge_leaves` for SV1 job `job_id`: the job id,
/// output 0, the `CTMD` disclosure and each leaf's aux branch, all hex.
pub fn sv1_merge_leaves_params(job_id: &str, tokens: &MinerTokenJob) -> Vec<Value> {
    let branches: Vec<String> = tokens
        .branches()
        .iter()
        .map(|branch| hex::encode(branch.concat()))
        .collect();
    vec![
        json!(job_id),
        json!(hex::encode(tokens.output())),
        json!(hex::encode(tokens.disclosure().encode())),
        json!(branches),
    ]
}

/// The params of `pickaxe.merge_proof` for a win on SV1 job `job_id`: the
/// job id, the token category, the `AuxProof` v2 and the split preimage
/// (empty without one), all hex.
pub fn sv1_merge_proof_params(job_id: &str, win: &TokenWin) -> Vec<Value> {
    vec![
        json!(job_id),
        json!(hex::encode(win.category)),
        json!(hex::encode(&win.proof)),
        json!(split_hex(win)),
    ]
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// The owner-only logs of a node that merge-mines tokens, beside its token
/// file: `<token file>.leaves.jsonl`, one line per standard-channel job
/// that commits a root, and `<token file>.wins.jsonl`, one line per token
/// a share wins.
///
/// ```text
/// {"time":<unix seconds>,"channel":<id>,"job":<id>,"output0":"<hex>","leaves":"<CTMD hex>"}
/// {"time":<unix seconds>,"channel":<id>,"job":<id>,"payout_hash":"<hex>","category":"<hex>","proof":"<hex>","split":"<hex or empty>"}
/// ```
///
/// An SV1 win names its `"job"` by the notify's hex job id and has no
/// channel. No line names an address: leaves carry `HASH256` of the
/// payout script, which a miner matches against their own. Writer threads
/// take the lines through bounded queues, so a connection never waits on
/// them; past [`MAX_LOG_BYTES`] a log moves to `.jsonl.1` and starts again.
#[derive(Clone, Debug)]
pub struct MergeMiningLogs {
    leaves: LineLog,
    wins: LineLog,
}

impl MergeMiningLogs {
    /// Start the logs beside `token_file`.
    pub fn start(token_file: &Path) -> Self {
        Self {
            leaves: LineLog::start(token_file.with_extension("leaves.jsonl"), MAX_LOG_BYTES).0,
            wins: LineLog::start(token_file.with_extension("wins.jsonl"), MAX_LOG_BYTES).0,
        }
    }

    /// Logs over the given line logs, for tests that join their writers.
    #[cfg(test)]
    pub(crate) fn from_logs(leaves: LineLog, wins: LineLog) -> Self {
        Self { leaves, wins }
    }

    /// Log the leaves of job `job` on standard channel `channel`.
    pub fn log_standard_job(&self, channel: u32, job: u32, tokens: &MinerTokenJob) {
        self.leaves.write(
            json!({
                "time": unix_seconds(),
                "channel": channel,
                "job": job,
                "output0": hex::encode(tokens.output()),
                "leaves": hex::encode(tokens.disclosure().encode()),
            })
            .to_string(),
        );
    }

    /// Log a token won by a share on SV2 job `job` of channel `channel`.
    pub fn log_sv2_win(&self, channel: u32, job: u32, tokens: &MinerTokenJob, win: &TokenWin) {
        self.wins.write(
            json!({
                "time": unix_seconds(),
                "channel": channel,
                "job": job,
                "payout_hash": payout_hash_hex(tokens),
                "category": hex::encode(win.category),
                "proof": hex::encode(&win.proof),
                "split": split_hex(win),
            })
            .to_string(),
        );
    }

    /// Log a token won by a share on SV1 job `job_id`.
    pub fn log_sv1_win(&self, job_id: &str, tokens: &MinerTokenJob, win: &TokenWin) {
        self.wins.write(
            json!({
                "time": unix_seconds(),
                "job": job_id,
                "payout_hash": payout_hash_hex(tokens),
                "category": hex::encode(win.category),
                "proof": hex::encode(&win.proof),
                "split": split_hex(win),
            })
            .to_string(),
        );
    }
}

/// The payout hash every leaf of a miner's job binds.
fn payout_hash_hex(tokens: &MinerTokenJob) -> String {
    tokens
        .entries
        .first()
        .map(|entry| hex::encode(entry.leaf.payout_hash))
        .unwrap_or_default()
}

/// A file a writer thread appends lines to, taking them from a bounded
/// queue, readable by its owner alone.
#[derive(Clone, Debug)]
pub struct LineLog {
    sender: SyncSender<String>,
}

impl LineLog {
    /// Start the writer of `path`, which moves to `<path>.1` and starts
    /// again past `max_bytes`. The handle ends once every `LineLog` clone
    /// is dropped and the queue is written.
    pub fn start(path: PathBuf, max_bytes: u64) -> (Self, JoinHandle<()>) {
        let (sender, receiver) = sync_channel(LOG_QUEUE_LINES);
        let writer = std::thread::Builder::new()
            .name("merge-mining-log".to_string())
            .spawn(move || write_lines(&path, max_bytes, receiver))
            .expect("the merge-mining log thread starts");
        (Self { sender }, writer)
    }

    /// Queue `line`; a full queue drops it and says so.
    pub fn write(&self, line: String) {
        match self.sender.try_send(line) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                warn!("Merge-mining log queue is full; a line was dropped")
            }
            Err(TrySendError::Disconnected(_)) => {
                error!("Merge-mining log writer has stopped; a line was dropped")
            }
        }
    }
}

/// Open `path` for appending, readable by its owner alone where the
/// platform has such modes.
fn open_owner_only(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// The writer thread: append each line, moving the file to `.1` past
/// `max_bytes`.
fn write_lines(path: &Path, max_bytes: u64, receiver: Receiver<String>) {
    let rotated = {
        let mut name = path.as_os_str().to_owned();
        name.push(".1");
        PathBuf::from(name)
    };
    let mut file = match open_owner_only(path) {
        Ok(file) => Some(file),
        Err(open_error) => {
            error!(
                "Cannot open merge-mining log {}: {open_error}",
                path.display()
            );
            None
        }
    };
    for line in receiver {
        let Some(open) = file.as_mut() else {
            continue;
        };
        if let Err(write_error) = writeln!(open, "{line}") {
            error!(
                "Cannot write merge-mining log {}: {write_error}",
                path.display()
            );
            continue;
        }
        let full = open
            .metadata()
            .map(|metadata| metadata.len() > max_bytes)
            .unwrap_or(false);
        if full {
            file = None;
            if let Err(rename_error) = std::fs::rename(path, &rotated) {
                error!(
                    "Cannot move merge-mining log {} aside: {rename_error}",
                    path.display()
                );
            }
            file = match open_owner_only(path) {
                Ok(file) => Some(file),
                Err(open_error) => {
                    error!(
                        "Cannot reopen merge-mining log {}: {open_error}",
                        path.display()
                    );
                    file
                }
            };
        }
    }
}

#[cfg(test)]
mod tests;
