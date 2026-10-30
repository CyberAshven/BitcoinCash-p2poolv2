// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Operator-side vault following and verified claim broadcasting.

use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bitcoindrpc::{BitcoindRpcClient, BitcoindRpcError};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::cli::{covenant_of, read_json};
use crate::{Covenant, Vault, decode_state, from_hex};

mod chain;
pub mod config;
pub mod files;
pub mod wins;

use chain::{ChainRpc, follow_vault};
use config::{Config, VaultPoint};
use files::{
    DeploymentIdentity, Journal, PendingClaim, lock_state_file, read_wins, rewrite_token_state,
};
use wins::{ClaimError, Win, build_win_claim, candidates, parse_win};

const USAGE: &str = "usage: safa claimer claimer.json [--once] [--dry-run]";

/// Process options; a dry run leaves the journal and token file unchanged.
#[derive(Debug, PartialEq, Eq)]
pub struct Options {
    pub config: PathBuf,
    pub once: bool,
    pub dry_run: bool,
}

impl Options {
    /// Parse the config path and optional flags, rejecting unknown arguments.
    pub fn parse(arguments: &[String]) -> Result<Self, String> {
        let mut options = Self {
            config: PathBuf::new(),
            once: false,
            dry_run: false,
        };
        for argument in arguments {
            match argument.as_str() {
                "--once" if !options.once => options.once = true,
                "--dry-run" if !options.dry_run => options.dry_run = true,
                path if !path.starts_with('-') && options.config.as_os_str().is_empty() => {
                    options.config = PathBuf::from(path);
                }
                _ => return Err(USAGE.into()),
            }
        }
        if options.config.as_os_str().is_empty() {
            return Err(USAGE.into());
        }
        Ok(options)
    }
}

struct Claimer<Rpc = BitcoindRpcClient> {
    config: Config,
    deployment: Value,
    covenant: Covenant,
    category_display: String,
    category_wire: String,
    client: Rpc,
    journal: Journal,
    dry_run: bool,
}

/// Follow a single vault and run one pass or poll until interrupted.
pub async fn run_claimer(options: Options) -> Result<(), String> {
    let config = Config::load(&options.config)?;
    let deployment = read_json(&config.deployment)?;
    let covenant = covenant_of(&deployment)?;
    let category = from_hex(
        deployment["category"]
            .as_str()
            .ok_or("missing deployment category")?,
        Some(32),
        "category",
    )?;
    let category_display = hex::encode(&category);
    let category_wire = hex::encode(category.into_iter().rev().collect::<Vec<_>>());
    let client = config.rpc_client()?;
    let _journal_lock = if options.dry_run {
        None
    } else {
        Some(lock_state_file(&config.journal)?)
    };
    let mut journal = Journal::load(&config.journal, &config.vault)?;
    let identity = DeploymentIdentity {
        network: covenant.network.clone(),
        category: category_display.clone(),
        threads: covenant.threads,
        virtual_sats: covenant.virtual_sats,
        origin: config.vault.clone(),
    };
    match &journal.deployment {
        Some(known) if known == &identity => {}
        None if journal.commitment.is_none()
            && journal.wins_offset == 0
            && journal.pending.is_empty() =>
        {
            journal.deployment = Some(identity)
        }
        _ => {
            return Err(
                "journal deployment or starting vault does not match this configuration".into(),
            );
        }
    }
    let mut claimer = Claimer {
        config,
        deployment,
        covenant,
        category_display,
        category_wire,
        client,
        journal,
        dry_run: options.dry_run,
    };
    claimer.check_network().await?;
    loop {
        if let Err(error) = claimer.pass().await {
            emit(json!({"event":"error", "reason":error}))?;
            if options.once {
                return Err(error);
            }
        }
        if options.once {
            return Ok(());
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(claimer.config.poll_seconds)) => {},
            signal = tokio::signal::ctrl_c() => {
                signal.map_err(|_| "could not listen for interruption")?;
                return Ok(());
            }
        }
    }
}

fn emit(event: Value) -> Result<(), String> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    serde_json::to_writer(&mut output, &event).map_err(|error| error.to_string())?;
    writeln!(output).map_err(|error| error.to_string())
}

fn rpc_error(method: &str, error: &BitcoindRpcError) -> String {
    match error {
        BitcoindRpcError::RpcError { code, .. } => format!("{method} failed (RPC code {code})"),
        _ => format!("{method} failed; check the node connection and credentials"),
    }
}

fn unix_time() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| "system clock precedes the Unix epoch".into())
}

impl<Rpc: ChainRpc> Claimer<Rpc> {
    async fn request<ResultValue: DeserializeOwned>(
        &self,
        method: &str,
        parameters: Vec<Value>,
    ) -> Result<ResultValue, BitcoindRpcError> {
        let result = self.client.call(method, parameters).await?;
        serde_json::from_value(result).map_err(|_| BitcoindRpcError::ParseError {
            message: "invalid RPC result".into(),
        })
    }
    async fn check_network(&self) -> Result<(), String> {
        let info: Value = self
            .request("getblockchaininfo", vec![])
            .await
            .map_err(|error| rpc_error("getblockchaininfo", &error))?;
        let expected = match self.covenant.network.as_str() {
            "mainnet" => "main",
            "chipnet" => "chip",
            _ => return Err("unsupported deployment network".into()),
        };
        if info["chain"] != expected {
            return Err("RPC network does not match the deployment".into());
        }
        Ok(())
    }

    fn save(&self) -> Result<(), String> {
        if self.dry_run {
            Ok(())
        } else {
            self.journal.save(&self.config.journal)
        }
    }

    fn write_state(&self) -> Result<(), String> {
        if !self.dry_run
            && let Some(state) = &self.journal.commitment
            && rewrite_token_state(&self.config.token_file, &self.category_wire, state)?
        {
            emit(json!({"event":"state_written", "state":state}))?;
        }
        Ok(())
    }

    fn ingest_wins(&mut self) -> Result<(), String> {
        let (offset, lines, rotated) = read_wins(&self.config.wins_log, self.journal.wins_offset)?;
        if rotated {
            emit(json!({"event":"wins_rotated", "offset":0}))?;
        }
        for line in lines {
            match parse_win(&line, &self.category_wire) {
                Ok(Some(win)) => {
                    let identity = win.id();
                    if !self.journal.claimed.contains(&identity)
                        && !self.journal.dropped.contains(&identity)
                        && !self.journal.wins.iter().any(|known| known.id() == identity)
                    {
                        let [minimum, maximum] = self.config.delay_seconds;
                        let delay = rand::random_range(minimum..=maximum);
                        let ready = win
                            .time
                            .checked_add(delay)
                            .ok_or("win delay overflows time")?;
                        self.journal.delays.insert(identity, ready);
                        self.journal.wins.push(win);
                    }
                }
                Ok(None) => {}
                Err(reason) => emit(json!({"event":"win_skipped", "reason":reason}))?,
            }
        }
        self.journal.wins_offset = offset;
        self.save()
    }

    async fn pass(&mut self) -> Result<(), String> {
        self.ingest_wins()?;
        let tip: u32 = self
            .request("getblockcount", vec![])
            .await
            .map_err(|error| rpc_error("getblockcount", &error))?;
        let old_point = self.journal.vault.clone();
        let old_state = self.journal.commitment.clone();
        let followed = follow_vault(
            &self.client,
            &self.covenant,
            &self.category_display,
            &mut self.journal,
            tip,
        )
        .await?;
        if old_point != self.journal.vault || old_state != self.journal.commitment {
            emit(json!({"event":"vault_followed", "vault":self.journal.vault,
                "commitment":self.journal.commitment}))?;
        }
        self.save()?;
        self.write_state()?;
        if !self.journal.pending.is_empty() {
            self.recover_pending(followed.vault.as_ref(), &followed.transactions)
                .await?;
        } else if let Some(vault) = followed.vault {
            self.claim_next(&vault, tip).await?;
        }
        Ok(())
    }

    fn finish_win(&mut self, identity: &str, claimed: bool) {
        let history = if claimed {
            &mut self.journal.claimed
        } else {
            &mut self.journal.dropped
        };
        if !history.iter().any(|known| known == identity) {
            history.push(identity.to_string());
        }
        self.journal.wins.retain(|win| win.id() != identity);
        self.journal.delays.remove(identity);
    }

    async fn claim_next(&mut self, vault: &Vault, tip: u32) -> Result<(), String> {
        let height = decode_state(&vault.commitment)?.height;
        self.drop_stale_wins(height)?;
        let available = candidates(
            &self.journal.wins,
            height,
            tip,
            &self.journal.claimed,
            &self.journal.dropped,
        );
        let now = unix_time()?;
        for win in available {
            let identity = win.id();
            let built = match self.build(&win, vault) {
                Ok(claim) => Some(claim),
                Err(ClaimError::Unresolved(reason)) => {
                    emit(json!({"event":"win_skipped", "win":identity, "reason":reason}))?;
                    self.finish_win(&identity, false);
                    None
                }
                Err(ClaimError::Invalid(reason)) => {
                    emit(json!({"event":"win_dropped", "win":identity, "reason":reason}))?;
                    self.finish_win(&identity, false);
                    None
                }
                Err(ClaimError::Deferred(reason)) => {
                    emit(json!({"event":"win_deferred", "win":identity, "reason":reason}))?;
                    return self.save();
                }
            };
            if let Some(claim) = built {
                let ready = self
                    .journal
                    .delays
                    .get(&identity)
                    .copied()
                    .ok_or("journal is missing a win's scheduled delay")?;
                if self.dry_run || now >= ready {
                    emit(
                        json!({"event":"claim_built", "win":identity, "dry_run":self.dry_run,
                        "claim":claim}),
                    )?;
                    if !self.dry_run {
                        let pending = PendingClaim {
                            win: identity,
                            txid: claim_string(&claim, "txid")?,
                            hex: claim_string(&claim, "hex")?,
                            next_state: claim_string(&claim, "next_state")?,
                            previous: self.journal.vault.clone(),
                        };
                        self.journal.pending.push(pending.clone());
                        self.save()?;
                        self.broadcast(&pending).await?;
                        return Ok(());
                    }
                } else {
                    // The oldest valid win keeps its place until its delay expires.
                    return self.save();
                }
            }
        }
        self.save()
    }

    fn drop_stale_wins(&mut self, height: u32) -> Result<(), String> {
        let stale = self
            .journal
            .wins
            .iter()
            .filter_map(|win| {
                win.height()
                    .ok()
                    .filter(|win_height| *win_height <= height)
                    .map(|_| win.id())
            })
            .collect::<Vec<_>>();
        for identity in stale {
            emit(json!({"event":"win_dropped", "win":identity,
                "reason":"coinbase height is at or below the current vault state"}))?;
            self.finish_win(&identity, false);
        }
        Ok(())
    }

    fn build(&self, win: &Win, vault: &Vault) -> Result<Value, ClaimError> {
        build_win_claim(
            &self.deployment,
            win,
            &self.journal.vault,
            vault,
            &self.config.payout_scripts,
            &self.config.split_scripts,
            self.config.fee_rate,
        )
    }

    async fn recover_pending(
        &mut self,
        vault: Option<&Vault>,
        transactions: &[String],
    ) -> Result<(), String> {
        let pending = self
            .journal
            .pending
            .first()
            .ok_or("missing pending claim")?
            .clone();
        if transactions.contains(&pending.txid) || self.journal.vault.txid == pending.txid {
            return self.accepted(&pending, true);
        }
        let known = self
            .request::<Value>("getrawtransaction", vec![json!(pending.txid), json!(true)])
            .await;
        match known {
            Ok(transaction) if transaction["confirmations"].as_i64().unwrap_or(0) >= 0 => {
                self.accepted(&pending, true)
            }
            Ok(_) | Err(BitcoindRpcError::RpcError { code: -5, .. }) => {
                if self.journal.vault != pending.previous {
                    self.journal.pending.remove(0);
                    emit(json!({"event":"rejected", "txid":pending.txid,
                        "reason":"vault moved before the pending claim was accepted"}))?;
                    self.save()
                } else if let Some(vault) = vault {
                    let win = self
                        .journal
                        .wins
                        .iter()
                        .find(|win| win.id() == pending.win)
                        .ok_or("pending claim has no journaled win")?;
                    let verified = self.build(win, vault).map_err(|error| error.to_string())?;
                    if verified["txid"] != pending.txid
                        || verified["hex"] != pending.hex
                        || verified["next_state"] != pending.next_state
                    {
                        return Err("pending claim does not match the verified transaction".into());
                    }
                    if self.dry_run {
                        emit(json!({"event":"claim_built", "win":pending.win,
                            "dry_run":true, "claim":verified}))
                    } else {
                        self.broadcast(&pending).await
                    }
                } else {
                    Ok(())
                }
            }
            Err(error) => Err(rpc_error("getrawtransaction", &error)),
        }
    }

    async fn broadcast(&mut self, pending: &PendingClaim) -> Result<(), String> {
        let result = self
            .request::<String>("sendrawtransaction", vec![json!(pending.hex)])
            .await;
        match result {
            Ok(txid) if txid == pending.txid => self.accepted(pending, false),
            Ok(_) => Err("sendrawtransaction returned an unexpected transaction id".into()),
            Err(error) => {
                emit(json!({"event":"rejected", "txid":pending.txid,
                    "reason":rpc_error("sendrawtransaction", &error)}))?;
                // Keep uncertain sends for lookup. A definite rejection is followed
                // again next pass before considering another claim.
                if matches!(error, BitcoindRpcError::RpcError {code, ..} if code != -27) {
                    self.journal
                        .pending
                        .retain(|known| known.txid != pending.txid);
                }
                self.save()
            }
        }
    }

    fn accepted(&mut self, pending: &PendingClaim, recovered: bool) -> Result<(), String> {
        self.finish_win(&pending.win, true);
        self.journal
            .pending
            .retain(|known| known.txid != pending.txid);
        if self.journal.vault == pending.previous {
            self.journal.vault = VaultPoint {
                txid: pending.txid.clone(),
                vout: 0,
            };
            self.journal.commitment = Some(pending.next_state.clone());
        }
        self.save()?;
        self.write_state()?;
        emit(
            json!({"event":"broadcast", "txid":pending.txid, "recovered":recovered,
            "dry_run":self.dry_run}),
        )
    }
}

fn claim_string(claim: &Value, field: &str) -> Result<String, String> {
    claim[field]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| format!("claim has no {field}"))
}

#[cfg(test)]
mod runtime_tests;
#[cfg(test)]
mod test_utils;

#[cfg(test)]
mod tests {
    use super::{Options, lock_state_file};
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[test]
    fn claimer_flags_accept_both_orders() {
        let options =
            Options::parse(&["--once".into(), "claimer.json".into(), "--dry-run".into()]).unwrap();
        assert_eq!(
            options,
            Options {
                config: PathBuf::from("claimer.json"),
                once: true,
                dry_run: true
            }
        );
    }

    #[test]
    fn claimer_rejects_unknown_flags() {
        assert!(Options::parse(&["claimer.json".into(), "--broadcast".into()]).is_err());
    }

    #[test]
    fn claimer_requires_config() {
        assert!(Options::parse(&["--once".into()]).is_err());
    }

    #[test]
    fn journal_lock_prevents_two_broadcasters_and_releases_on_close() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("journal.json");
        let first = lock_state_file(&path).unwrap();
        assert!(lock_state_file(&path).is_err());
        drop(first);
        assert!(lock_state_file(&path).is_ok());
    }
}
