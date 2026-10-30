// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Atomic operator state and incremental public win-log reads.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use super::config::VaultPoint;
use super::wins::Win;
use crate::cli::read_json;
use crate::from_hex;

const MAX_JOURNAL_BYTES: u64 = 64 * 1024 * 1024;
const MAX_WIN_LINE_BYTES: usize = 1024 * 1024;
const MAX_WIN_PASS_BYTES: u64 = 8 * 1024 * 1024;

/// Lock a stable sidecar, because replacing the state file changes its inode.
/// Journal locks are held before token locks; no code takes them in reverse.
pub(super) fn lock_state_file(path: &Path) -> Result<File, String> {
    let mut name = path.as_os_str().to_os_string();
    name.push(".lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(PathBuf::from(name))
        .map_err(|_| "could not open the state file lock")?;
    file.try_lock()
        .map_err(|_| "state file is locked by another claimer")?;
    Ok(file)
}

/// An exact prepared transaction persisted before attempting a broadcast.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PendingClaim {
    pub win: String,
    pub txid: String,
    pub hex: String,
    pub next_state: String,
    pub previous: VaultPoint,
}

/// Deployment and starting vault permanently associated with this journal.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DeploymentIdentity {
    pub network: String,
    pub category: String,
    pub threads: u32,
    pub virtual_sats: u64,
    pub origin: VaultPoint,
}

/// Durable progress, including wins already consumed from the append-only log.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Journal {
    pub vault: VaultPoint,
    #[serde(default)]
    pub deployment: Option<DeploymentIdentity>,
    #[serde(default)]
    pub commitment: Option<String>,
    #[serde(default)]
    pub thread: Option<u32>,
    #[serde(default)]
    pub wins_offset: u64,
    #[serde(default)]
    pub scan_height: Option<u32>,
    #[serde(default)]
    pub scan_hash: Option<String>,
    #[serde(default)]
    pub claimed: Vec<String>,
    #[serde(default)]
    pub dropped: Vec<String>,
    #[serde(default)]
    pub pending: Vec<PendingClaim>,
    #[serde(default)]
    pub wins: Vec<Win>,
    #[serde(default)]
    pub delays: BTreeMap<String, u64>,
}

/// Flush bytes before atomically replacing the destination in its directory.
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temporary = NamedTempFile::new_in(directory).map_err(|error| error.to_string())?;
    temporary
        .write_all(bytes)
        .map_err(|error| error.to_string())?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    temporary
        .persist(path)
        .map_err(|error| error.error.to_string())?;
    Ok(())
}

impl Journal {
    /// Resume an existing journal, or initialize only when the file is absent.
    pub fn load(path: &Path, start: &VaultPoint) -> Result<Self, String> {
        let file = match File::open(path) {
            Ok(file) => Some(file),
            Err(error) if error.kind() == ErrorKind::NotFound => None,
            Err(error) => return Err(error.to_string()),
        };
        if let Some(file) = file {
            if !file
                .metadata()
                .map_err(|error| error.to_string())?
                .is_file()
            {
                return Err("journal must be a regular file".into());
            }
            let mut bytes = Vec::with_capacity(1024);
            file.take(MAX_JOURNAL_BYTES + 1)
                .read_to_end(&mut bytes)
                .map_err(|error| error.to_string())?;
            if bytes.len() as u64 > MAX_JOURNAL_BYTES {
                return Err("journal exceeds 64 MiB".into());
            }
            let journal: Self =
                serde_json::from_slice(&bytes).map_err(|_| "invalid claimer journal")?;
            journal.vault.validate()?;
            if let Some(commitment) = &journal.commitment {
                from_hex(commitment, Some(16), "journal commitment")?;
            }
            Ok(journal)
        } else {
            start.validate()?;
            Ok(Self {
                vault: start.clone(),
                deployment: None,
                commitment: None,
                thread: None,
                wins_offset: 0,
                scan_height: None,
                scan_hash: None,
                claimed: Vec::with_capacity(16),
                dropped: Vec::with_capacity(16),
                pending: Vec::with_capacity(1),
                wins: Vec::with_capacity(16),
                delays: BTreeMap::new(),
            })
        }
    }

    /// Persist the full transition and offset together by atomic replacement.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(self).map_err(|error| error.to_string())?;
        if bytes.len() as u64 > MAX_JOURNAL_BYTES {
            return Err("journal exceeds 64 MiB".into());
        }
        atomic_write(path, &bytes)
    }
}

/// Replace only the matching token's SAFA state; preserve every other JSON field.
pub fn rewrite_token_state(path: &Path, category_wire: &str, state: &str) -> Result<bool, String> {
    let _token_lock = lock_state_file(path)?;
    let category = from_hex(category_wire, Some(32), "token category")?;
    from_hex(state, Some(16), "token state")?;
    let mut document = read_json(path)?;
    let tokens = document
        .get_mut("tokens")
        .and_then(|tokens| tokens.as_array_mut())
        .ok_or("token file must contain a tokens array")?;
    let matches = tokens
        .iter()
        .enumerate()
        .filter_map(|(index, token)| {
            token
                .get("category")
                .and_then(|value| value.as_str())
                .and_then(|value| hex::decode(value).ok())
                .filter(|candidate| candidate == &category)
                .map(|_| index)
        })
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        return Err("token file must contain exactly one matching category".into());
    }
    let safa = tokens[matches[0]]
        .get_mut("safa")
        .and_then(|value| value.as_object_mut())
        .ok_or("matching token must contain a safa object")?;
    if safa.get("state").and_then(|value| value.as_str()) == Some(state) {
        Ok(false)
    } else {
        safa.insert("state".into(), state.into());
        let bytes = serde_json::to_vec_pretty(&document).map_err(|error| error.to_string())?;
        atomic_write(path, &bytes)?;
        Ok(true)
    }
}

/// Read complete new lines, retaining a partial final line for the next pass.
///
/// A shrinking file resets the cursor. Each pass reads up to 8 MiB and each
/// line retains at most 1 MiB; an oversized complete line becomes an invalid
/// empty record so callers log and skip it. Oversized lines are drained to the
/// next newline even beyond the pass budget. A missing log has no new records.
pub fn read_wins(path: &Path, offset: u64) -> Result<(u64, Vec<Vec<u8>>, bool), String> {
    let file = match File::open(path) {
        Ok(file) => Some(file),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => return Err(error.to_string()),
    };
    if let Some(mut file) = file {
        let metadata = file.metadata().map_err(|error| error.to_string())?;
        if !metadata.is_file() {
            return Err("wins log must be a regular file".into());
        }
        let rotated = metadata.len() < offset;
        let start = if rotated { 0 } else { offset };
        file.seek(SeekFrom::Start(start))
            .map_err(|error| error.to_string())?;
        let mut reader = BufReader::new(file);
        let mut lines = Vec::with_capacity(64);
        let mut line = Vec::with_capacity(4096);
        let mut consumed = start;
        let mut complete_offset = start;
        let mut oversized = false;
        let mut available = reader.fill_buf().map_err(|error| error.to_string())?;
        while !available.is_empty() && (consumed - start < MAX_WIN_PASS_BYTES || oversized) {
            let end = available.iter().position(|byte| *byte == b'\n');
            let count = end.map_or(available.len(), |position| position + 1);
            if !oversized {
                if line.len() + count > MAX_WIN_LINE_BYTES {
                    oversized = true;
                    line.clear();
                } else {
                    line.extend_from_slice(&available[..count]);
                }
            }
            reader.consume(count);
            consumed += count as u64;
            if end.is_some() {
                if !oversized {
                    line.pop();
                }
                lines.push(line);
                line = Vec::with_capacity(4096);
                oversized = false;
                complete_offset = consumed;
            }
            available = reader.fill_buf().map_err(|error| error.to_string())?;
        }
        Ok((complete_offset, lines, rotated))
    } else {
        Ok((offset, Vec::with_capacity(0), false))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::{Value, json};
    use tempfile::tempdir;

    use super::{
        DeploymentIdentity, Journal, PendingClaim, lock_state_file, read_wins, rewrite_token_state,
    };
    use crate::claimer::config::VaultPoint;

    #[test]
    fn token_rewrite_preserves_other_tokens_and_all_unrelated_fields() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("tokens.json");
        let category = "11".repeat(32);
        let original = json!({"owner":"operator","tokens":[
            {"category":category,"lock_hash":"unchanged","safa":{"threads":4,"state":"00".repeat(16),"custom":42},"unknown":[1,2]},
            {"category":"22".repeat(32),"safa":{"state":"33".repeat(16),"threads":63},"keep":true}
        ]});
        fs::write(&path, original.to_string()).unwrap();
        assert!(rewrite_token_state(&path, &category, &"44".repeat(16)).unwrap());
        let changed: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let mut expected = original;
        expected["tokens"][0]["safa"]["state"] = json!("44".repeat(16));
        assert_eq!(changed, expected);
        assert!(!rewrite_token_state(&path, &category, &"44".repeat(16)).unwrap());
    }

    #[test]
    fn token_rewrite_rejects_duplicate_categories_without_mutating_file() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("tokens.json");
        let category = "11".repeat(32);
        let bytes = json!({"tokens":[
            {"category":category,"safa":{"state":"00".repeat(16)}},
            {"category":category,"safa":{"state":"22".repeat(16)}}
        ]})
        .to_string();
        fs::write(&path, &bytes).unwrap();
        assert!(rewrite_token_state(&path, &category, &"44".repeat(16)).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), bytes);
    }

    #[test]
    fn token_lock_prevents_another_writer_from_replacing_other_category_updates() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("tokens.json");
        let category = "11".repeat(32);
        let original =
            json!({"tokens":[{"category":category,"safa":{"state":"00".repeat(16)}}]}).to_string();
        fs::write(&path, &original).unwrap();
        let first_writer = lock_state_file(&path).unwrap();
        assert!(rewrite_token_state(&path, &category, &"22".repeat(16)).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        drop(first_writer);
        assert!(rewrite_token_state(&path, &category, &"22".repeat(16)).unwrap());
    }

    #[test]
    fn malformed_line_larger_than_pass_budget_does_not_stall_later_wins() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("wins.jsonl");
        let mut log = vec![b'x'; 8 * 1024 * 1024 + 32];
        log.extend_from_slice(b"\nvalid\n");
        fs::write(&path, &log).unwrap();
        let (offset, lines, rotated) = read_wins(&path, 0).unwrap();
        assert_eq!(offset, 8 * 1024 * 1024 + 33);
        assert_eq!(lines, vec![Vec::<u8>::new()]);
        assert!(!rotated);
        let (next_offset, lines, rotated) = read_wins(&path, offset).unwrap();
        assert_eq!(next_offset, log.len() as u64);
        assert_eq!(lines, vec![b"valid".to_vec()]);
        assert!(!rotated);
    }

    #[test]
    fn wins_cursor_advances_only_over_complete_lines() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("wins.jsonl");
        fs::write(&path, b"first\nsecond").unwrap();
        let (offset, lines, rotated) = read_wins(&path, 0).unwrap();
        assert_eq!(offset, 6);
        assert_eq!(lines, vec![b"first".to_vec()]);
        assert!(!rotated);
        fs::write(&path, b"first\nsecond\nthird\n").unwrap();
        let (offset, lines, rotated) = read_wins(&path, offset).unwrap();
        assert_eq!(offset, 19);
        assert_eq!(lines, vec![b"second".to_vec(), b"third".to_vec()]);
        assert!(!rotated);
    }

    #[test]
    fn wins_cursor_restarts_after_rotation_shrinks_file() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("wins.jsonl");
        fs::write(&path, b"new\n").unwrap();
        let (offset, lines, rotated) = read_wins(&path, 9000).unwrap();
        assert_eq!(offset, 4);
        assert_eq!(lines, vec![b"new".to_vec()]);
        assert!(rotated);
    }

    #[test]
    fn oversized_complete_win_line_is_skipped_without_losing_next_line() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("wins.jsonl");
        let mut bytes = vec![b'x'; 1024 * 1024 + 1];
        bytes.extend_from_slice(b"\nvalid\n");
        fs::write(&path, &bytes).unwrap();
        let (offset, lines, rotated) = read_wins(&path, 0).unwrap();
        assert_eq!(offset, bytes.len() as u64);
        assert_eq!(lines, vec![Vec::<u8>::new(), b"valid".to_vec()]);
        assert!(!rotated);
    }

    #[test]
    fn journal_resumes_vault_cursor_outcomes_schedule_and_pending_claim() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("journal.json");
        let start = VaultPoint {
            txid: "11".repeat(32),
            vout: 3,
        };
        let mut journal = Journal::load(&path, &start).unwrap();
        let identity = DeploymentIdentity {
            network: "chipnet".into(),
            category: "77".repeat(32),
            threads: 4,
            virtual_sats: 84_000_000_000,
            origin: start.clone(),
        };
        journal.deployment = Some(identity.clone());
        journal.vault = VaultPoint {
            txid: "22".repeat(32),
            vout: 0,
        };
        journal.commitment = Some("33".repeat(16));
        journal.thread = Some(3);
        journal.wins_offset = 12345;
        journal.scan_height = Some(330100);
        journal.scan_hash = Some("44".repeat(32));
        journal.claimed.push("claimed-win".into());
        journal.dropped.push("dropped-win".into());
        journal.delays.insert("pending-win".into(), 2000);
        journal.pending.push(PendingClaim {
            win: "pending-win".into(),
            txid: "55".repeat(32),
            hex: "00".into(),
            next_state: "66".repeat(16),
            previous: start.clone(),
        });
        journal.save(&path).unwrap();
        let resumed = Journal::load(&path, &start).unwrap();
        assert_eq!(resumed.deployment, Some(identity));
        assert_eq!(resumed.vault.txid, "22".repeat(32));
        assert_eq!(resumed.commitment, Some("33".repeat(16)));
        assert_eq!(resumed.thread, Some(3));
        assert_eq!(resumed.wins_offset, 12345);
        assert_eq!(resumed.scan_height, Some(330100));
        assert_eq!(resumed.scan_hash, Some("44".repeat(32)));
        assert_eq!(resumed.claimed, vec!["claimed-win"]);
        assert_eq!(resumed.dropped, vec!["dropped-win"]);
        assert_eq!(resumed.delays.get("pending-win"), Some(&2000));
        assert_eq!(resumed.pending[0].previous, start);
        assert_eq!(resumed.pending[0].txid, "55".repeat(32));
        assert_eq!(resumed.pending[0].hex, "00");
        assert_eq!(resumed.pending[0].next_state, "66".repeat(16));
    }

    #[test]
    fn malformed_journal_is_not_reinitialized() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("journal.json");
        fs::write(&path, "broken").unwrap();
        let start = VaultPoint {
            txid: "11".repeat(32),
            vout: 3,
        };
        assert!(
            Journal::load(&path, &start)
                .unwrap_err()
                .contains("invalid claimer journal")
        );
        assert_eq!(fs::read_to_string(path).unwrap(), "broken");
    }
}
