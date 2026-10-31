// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Read public pool wins, resolve committed scripts, and verify claims.

use std::collections::HashSet;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::config::VaultPoint;
use crate::cli::run;
use crate::{
    Vault, coinbase_height, coinbase_merkle_root, commitment_output, decode_proof, fold_aux,
    from_hex, hash256, parse_split, slot,
};

/// One public AuxPoW win. Hashes use the pool's hashed (wire) byte order.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Win {
    pub category: String,
    #[serde(default)]
    pub channel: Value,
    pub job: Value,
    pub payout_hash: String,
    pub proof: String,
    pub split: String,
    pub time: u64,
}

impl Win {
    /// Identify a proof and its committed recipients independently of log position.
    pub fn id(&self) -> String {
        let identity = format!(
            "{}:{}:{}:{}",
            self.category, self.payout_hash, self.proof, self.split
        );
        hex::encode(hash256(identity.as_bytes()))
    }

    /// Read the proof's BIP34 coinbase height, never the pool's clock or job id.
    pub fn height(&self) -> Result<u32, String> {
        let proof = decode_proof(&from_hex(&self.proof, None, "proof")?)?;
        coinbase_height(&proof.head)
    }
}

/// Parse a matching win, rejecting malformed proof and split encodings.
/// Other categories need no proof parsing and are ignored.
pub fn parse_win(line: &[u8], category_wire: &str) -> Result<Option<Win>, String> {
    let value: Value = serde_json::from_slice(line).map_err(|error| error.to_string())?;
    let category = value["category"].as_str().ok_or("win has no category")?;
    from_hex(category, Some(32), "win category")?;
    if !category.eq_ignore_ascii_case(category_wire) {
        return Ok(None);
    }
    let mut win: Win = serde_json::from_value(value).map_err(|error| error.to_string())?;
    win.category = hex::encode(from_hex(&win.category, Some(32), "win category")?);
    win.payout_hash = hex::encode(from_hex(&win.payout_hash, Some(32), "payout_hash")?);
    win.proof = hex::encode(from_hex(&win.proof, None, "proof")?);
    let split = from_hex(&win.split, None, "split")?;
    parse_split(&split)?;
    win.split = hex::encode(split);
    win.height()?;
    Ok(Some(win))
}

/// Select distinct, unattempted wins above the vault state and at the tip or below.
/// Height is the primary ordering, with time and identity breaking ties.
pub fn candidates(
    wins: &[Win],
    state_height: u32,
    tip: u32,
    claimed: &[String],
    dropped: &[String],
) -> Vec<Win> {
    let mut distinct = HashSet::with_capacity(wins.len());
    let mut selected = wins
        .iter()
        .filter_map(|win| win.height().ok().map(|height| (height, win)))
        .filter(|(height, win)| {
            let identity = win.id();
            *height > state_height
                && *height <= tip
                && !claimed.contains(&identity)
                && !dropped.contains(&identity)
                && distinct.insert(identity)
        })
        .collect::<Vec<_>>();
    selected.sort_by_cached_key(|(height, win)| (*height, win.time, win.id()));
    selected.into_iter().map(|(_, win)| win.clone()).collect()
}

/// Distinguish missing recipients, temporary vault funding, and invalid claims.
#[derive(Debug, PartialEq, Eq)]
pub enum ClaimError {
    Unresolved(String),
    Deferred(String),
    Invalid(String),
}

impl fmt::Display for ClaimError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unresolved(message) | Self::Deferred(message) | Self::Invalid(message) => {
                formatter.write_str(message)
            }
        }
    }
}

/// Resolve a HASH256 commitment against the configured public locking bytecodes.
pub fn resolve_script(expected: &[u8], scripts: &[String]) -> Result<String, ClaimError> {
    for script in scripts {
        let bytecode = from_hex(script, None, "locking bytecode").map_err(ClaimError::Invalid)?;
        if hash256(&bytecode) == expected {
            return Ok(hex::encode(bytecode));
        }
    }
    Err(ClaimError::Unresolved(format!(
        "no configured locking bytecode matches HASH256 {}",
        hex::encode(expected)
    )))
}

/// Resolve the canonical split preimage; zero-rate slots require no script.
pub fn resolve_split(preimage: &str, scripts: &[String]) -> Result<Value, ClaimError> {
    let bytes = from_hex(preimage, None, "split").map_err(ClaimError::Invalid)?;
    let split = parse_split(&bytes).map_err(ClaimError::Invalid)?;
    match split {
        None => Ok(Value::Null),
        Some(split) => {
            let donation_script = if split.donation_bps == 0 {
                String::new()
            } else {
                resolve_script(&split.donation_hash, scripts)?
            };
            let fee_script = if split.fee_bps == 0 {
                String::new()
            } else {
                resolve_script(&split.fee_hash, scripts)?
            };
            Ok(json!({
                "donation_bps": split.donation_bps,
                "donation_script": donation_script,
                "fee_bps": split.fee_bps,
                "fee_script": fee_script,
            }))
        }
    }
}

/// Build and verify through the same BCH 2026 VM path as `safa claim`.
#[allow(clippy::too_many_arguments)]
pub fn build_win_claim(
    deployment: &Value,
    win: &Win,
    outpoint: &VaultPoint,
    vault: &Vault,
    payout_scripts: &[String],
    split_scripts: &[String],
    fee_rate: u32,
) -> Result<Value, ClaimError> {
    let category_wire = hex::encode(vault.category.iter().rev().copied().collect::<Vec<_>>());
    if !win.category.eq_ignore_ascii_case(&category_wire) {
        return Err(ClaimError::Invalid(
            "win category does not match the vault".into(),
        ));
    }
    let payout_hash =
        from_hex(&win.payout_hash, Some(32), "payout_hash").map_err(ClaimError::Invalid)?;
    let payout_script = resolve_script(&payout_hash, payout_scripts)?;
    let split = resolve_split(&win.split, split_scripts)?;
    let input = json!({
        "outpoint": {"txid": outpoint.txid, "vout": outpoint.vout},
        "vault": {
            "value_sats": vault.value.to_string(),
            "token_amount": vault.token_amount.to_string(),
            "commitment": hex::encode(&vault.commitment),
        },
        "payout_script": payout_script,
        "split": split,
        "proof": win.proof,
        "fee_rate": fee_rate,
        "locktime": win.height().map_err(ClaimError::Invalid)?,
    });
    let inputs = [deployment.clone(), input];
    match run("claim", &inputs) {
        Ok(claim) => Ok(claim),
        Err(message) => {
            if message == "the vault cannot pay the claim fee and outputs and keep 800 satoshis"
                || message == "the vault has too few tokens for a reward"
            {
                verify_state_commitment(deployment, &inputs[1]).map_err(ClaimError::Invalid)?;
                Err(ClaimError::Deferred(message))
            } else {
                Err(ClaimError::Invalid(message))
            }
        }
    }
}

/// A temporary funding failure must not hide work committed to another state.
/// This is only a deferral check; broadcast still requires the complete claim VM.
fn verify_state_commitment(deployment: &Value, input: &Value) -> Result<(), String> {
    let proof = decode_proof(&from_hex(
        input["proof"].as_str().ok_or("claim has no proof")?,
        None,
        "proof",
    )?)?;
    let job = json!({
        "vault_commitment":input["vault"]["commitment"],
        "height":coinbase_height(&proof.head)?,
        "payout_script":input["payout_script"],
        "split":input["split"],
    });
    let leaf = run("leaf", &[deployment.clone(), job])?;
    let leaf_hash = from_hex(
        leaf["leaf_hash"].as_str().ok_or("leaf has no hash")?,
        Some(32),
        "leaf hash",
    )?;
    let category = from_hex(
        leaf["token"]["category"]
            .as_str()
            .ok_or("leaf has no category")?,
        Some(32),
        "leaf category",
    )?;
    let root = fold_aux(
        &leaf_hash,
        slot(&category, proof.salt, proof.height),
        &proof.aux_branch,
    );
    let output = commitment_output(&root, proof.height, proof.salt);
    let mut coinbase = Vec::with_capacity(proof.head.len() + output.len() + proof.tail.len());
    coinbase.extend_from_slice(&proof.head);
    coinbase.extend_from_slice(&output);
    coinbase.extend_from_slice(&proof.tail);
    if coinbase_merkle_root(&coinbase, &proof.coinbase_branch) != proof.header[36..68] {
        return Err(
            "proof commitment does not match the current vault state and recipients".into(),
        );
    }
    Ok(())
}
