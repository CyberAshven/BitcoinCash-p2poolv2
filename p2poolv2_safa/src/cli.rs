// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! JSON adapters for the SAFA command line, preserving the reference output.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use libauth::encode_transaction_bch;
use libauth::runtime::number::number_to_string;
use serde_json::{Value, json};

use crate::{
    BuiltClaim, Covenant, Funding, GenesisOptions, MAX_THREADS, Outpoint, Split, Vault,
    build_claim, build_genesis, claim_leaf, create_covenant, from_hex, hash256, vault_address,
    verify_transaction,
};

pub const USAGE: &str = "usage: safa params deployment.json | leaf deployment.json job.json | work deployment.json job.json | unlock deployment.json claim.json | claim deployment.json claim.json | genesis genesis.json | verify deployment.json claim.json | claimer claimer.json [--once] [--dry-run]";
const MAX_JSON_BYTES: u64 = 1024 * 1024;
const MAX_SAFE_INTEGER: u64 = (1_u64 << 53) - 1;

/// Read at most 1 MiB of JSON from a regular file.
pub fn read_json(path: impl AsRef<Path>) -> Result<Value, String> {
    let file = File::open(path).map_err(|error| error.to_string())?;
    if !file
        .metadata()
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Err("input must be a regular file".into());
    }
    let mut bytes = Vec::with_capacity(MAX_JSON_BYTES as usize + 1);
    file.take(MAX_JSON_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > MAX_JSON_BYTES as usize {
        return Err("input JSON exceeds 1 MiB".into());
    }
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}

/// Run a command on already parsed JSON, without file or network access.
pub fn run(command: &str, inputs: &[Value]) -> Result<Value, String> {
    match (command, inputs) {
        ("params", [deployment]) => params(deployment),
        ("leaf" | "work", [deployment, job]) => leaf(deployment, job),
        ("claim", [deployment, input]) => claim(deployment, input),
        ("genesis", [input]) => genesis(input),
        ("unlock", [deployment, input]) => unlock(deployment, input),
        ("verify", [deployment, input]) => verify(deployment, input),
        _ => Err(USAGE.into()),
    }
}

fn amount(value: &Value, name: &str) -> Result<u64, String> {
    let error =
        || format!("{name} must be a whole number of base units, as a number or a decimal string");
    match value {
        Value::Number(_) => safe_number(value).ok_or_else(error),
        Value::String(text)
            if !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            text.parse().map_err(|_| error())
        }
        _ => Err(error()),
    }
}

fn integer(value: &Value, name: &str) -> Result<u32, String> {
    safe_number(value)
        .filter(|integer| *integer <= u64::from(u32::MAX))
        .map(|integer| integer as u32)
        .ok_or_else(|| format!("{name} must be a non-negative integer"))
}

fn safe_number(value: &Value) -> Option<u64> {
    value
        .as_f64()
        .filter(|number| {
            number.is_finite()
                && *number >= 0.0
                && *number <= MAX_SAFE_INTEGER as f64
                && number.fract() == 0.0
        })
        .map(|number| number as u64)
}

fn bits(value: &Value, name: &str) -> Result<u32, String> {
    value
        .as_str()
        .filter(|text| text.len() == 8 && text.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .and_then(|text| u32::from_str_radix(text, 16).ok())
        .ok_or_else(|| format!("{name} must be eight hexadecimal digits, such as \"1d00ffff\""))
}

fn bytes(value: &Value, length: Option<usize>, name: &str) -> Result<Vec<u8>, String> {
    let text = value
        .as_str()
        .ok_or_else(|| format!("{name} must be hexadecimal bytes"))?;
    from_hex(text, length, name)
}

fn optional_script(value: &Value, name: &str) -> Result<Vec<u8>, String> {
    if value.is_null() {
        Ok(Vec::new())
    } else {
        bytes(value, None, name)
    }
}

fn fee_rate(input: &Value) -> Result<u32, String> {
    if input["fee_rate"].is_null() {
        Ok(1)
    } else {
        integer(&input["fee_rate"], "fee_rate")
    }
}

pub(crate) fn covenant_of(deployment: &Value) -> Result<Covenant, String> {
    let network = deployment["network"]
        .as_str()
        .ok_or("network must be mainnet or chipnet")?;
    let threads = integer(&deployment["threads"], "threads")?;
    create_covenant(
        network,
        threads,
        amount(&deployment["virtual_sats"], "virtual_sats")?,
    )
}

fn category_of(deployment: &Value) -> Result<Vec<u8>, String> {
    bytes(&deployment["category"], Some(32), "category")
}

fn split_of(value: &Value) -> Result<Option<Split>, String> {
    if value.is_null() {
        return Ok(None);
    }
    let donation_bps = integer(&value["donation_bps"], "donation_bps")?;
    let fee_bps = integer(&value["fee_bps"], "fee_bps")?;
    Ok(Some(Split {
        donation_bps: donation_bps
            .try_into()
            .map_err(|_| "donation_bps must be an unsigned 16-bit integer")?,
        donation_locking_bytecode: optional_script(&value["donation_script"], "donation_script")?,
        fee_bps: fee_bps
            .try_into()
            .map_err(|_| "fee_bps must be an unsigned 16-bit integer")?,
        fee_locking_bytecode: optional_script(&value["fee_script"], "fee_script")?,
    }))
}

fn params(deployment: &Value) -> Result<Value, String> {
    let covenant = covenant_of(deployment)?;
    Ok(json!({
        "network": covenant.network,
        "threads": covenant.threads,
        "virtual_sats": covenant.virtual_sats.to_string(),
        "redeem_script": hex::encode(&covenant.redeem_script),
        "redeem_script_bytes": covenant.redeem_script.len(),
        "fee_k": covenant.fee_k,
        "locking_bytecode": hex::encode(&covenant.locking_bytecode),
        "lock_hash": hex::encode(&covenant.lock_hash),
        "vault_address": vault_address(&covenant)?,
    }))
}

fn leaf(deployment: &Value, job: &Value) -> Result<Value, String> {
    let covenant = covenant_of(deployment)?;
    let split = split_of(&job["split"])?;
    let vault = Vault {
        category: category_of(deployment)?,
        value: 0,
        token_amount: 0,
        commitment: bytes(&job["vault_commitment"], Some(16), "vault_commitment")?,
    };
    let info = claim_leaf(
        &covenant,
        &vault,
        &bytes(&job["payout_script"], None, "payout_script")?,
        split.as_ref(),
        integer(&job["height"], "height")?,
    )?;
    let mut token = json!({
        "category": hex::encode(&info.leaf.category),
        "lock_hash": hex::encode(&info.leaf.lock_hash),
        "state_hash": hex::encode(&info.leaf.state_hash),
        "payout_hash": hex::encode(&info.leaf.payout_hash),
        "target_bits": format!("{:08x}", info.target_bits),
    });
    if let Some(split) = split {
        token["split"] = json!({
            "donation_bps": split.donation_bps,
            "donation_script": hex::encode(split.donation_locking_bytecode),
            "fee_bps": split.fee_bps,
            "fee_script": hex::encode(split.fee_locking_bytecode),
        });
    }
    Ok(json!({
        "token": token,
        "leaf_hash": hex::encode(info.leaf_hash),
        "ext": hex::encode(info.leaf.ext),
        "split_preimage": hex::encode(info.preimage),
        "height_delta": info.height_delta,
    }))
}

fn built_claim(deployment: &Value, input: &Value) -> Result<BuiltClaim, String> {
    let covenant = covenant_of(deployment)?;
    let vault = Vault {
        category: category_of(deployment)?,
        value: amount(&input["vault"]["value_sats"], "vault.value_sats")?,
        token_amount: amount(&input["vault"]["token_amount"], "vault.token_amount")?,
        commitment: bytes(&input["vault"]["commitment"], Some(16), "vault.commitment")?,
    };
    let outpoint = Outpoint {
        txid: bytes(&input["outpoint"]["txid"], Some(32), "outpoint.txid")?,
        vout: integer(&input["outpoint"]["vout"], "outpoint.vout")?,
    };
    let split = split_of(&input["split"])?;
    let locktime = if input["locktime"].is_null() {
        None
    } else {
        Some(integer(&input["locktime"], "locktime")?)
    };
    build_claim(
        &covenant,
        &outpoint,
        &vault,
        &bytes(&input["payout_script"], None, "payout_script")?,
        split.as_ref(),
        &bytes(&input["proof"], None, "proof")?,
        fee_rate(input)?,
        locktime,
    )
}

fn claim(deployment: &Value, input: &Value) -> Result<Value, String> {
    let built = built_claim(deployment, input)?;
    verify_transaction(&built.transaction, &built.source_outputs)
        .map_err(|error| format!("the covenant refuses this claim: {error}"))?;
    let encoded = encode_transaction_bch(&built.transaction);
    let mut transaction_hash = hash256(&encoded);
    transaction_hash.reverse();
    Ok(json!({
        "txid": hex::encode(transaction_hash),
        "hex": hex::encode(&encoded),
        "size": encoded.len(),
        "size_bound": built.plan.size_bound,
        "fee_sats": built.fee.to_string(),
        "coinbase_height": built.plan.height,
        "target_bits": format!("{:08x}", built.plan.target_bits),
        "reward": built.plan.reward.to_string(),
        "shares": {
            "miner": built.plan.shares.miner.to_string(),
            "donation": built.plan.shares.donation.to_string(),
            "fee": built.plan.shares.fee.to_string(),
        },
        "next_state": hex::encode(built.plan.next_state),
    }))
}

fn genesis(input: &Value) -> Result<Value, String> {
    if input["network"] != "chipnet" {
        return Err("genesis is prepared for chipnet only".into());
    }
    let threads = integer(&input["threads"], "threads")?;
    if threads == 0 || threads > MAX_THREADS {
        return Err(format!(
            "threads must be an integer from 1 to {MAX_THREADS}"
        ));
    }
    let covenant = covenant_of(input)?;
    let funding = Funding {
        txid: bytes(&input["funding"]["txid"], Some(32), "funding.txid")?,
        vout: integer(&input["funding"]["vout"], "funding.vout")?,
        value_satoshis: amount(&input["funding"]["value_sats"], "funding.value_sats")?,
        locking_bytecode: bytes(&input["funding"]["script"], None, "funding.script")?,
    };
    let built = build_genesis(
        &covenant,
        &GenesisOptions {
            funding: funding.clone(),
            change_locking_bytecode: bytes(&input["change_script"], None, "change_script")?,
            supply_per_thread: amount(&input["supply_per_thread"], "supply_per_thread")?,
            seed_sats: amount(&input["seed_sats"], "seed_sats")?,
            start_height: integer(&input["start_height"], "start_height")?,
            target_bits: bits(&input["target_bits"], "target_bits")?,
            ref_bits: bits(&input["ref_bits"], "ref_bits")?,
            fee_rate: fee_rate(input)?,
        },
    )?;
    let unsigned = hex::encode(encode_transaction_bch(&built.transaction));
    let txid =
        serde_json::to_string(&input["funding"]["txid"]).map_err(|error| error.to_string())?;
    let script =
        serde_json::to_string(&input["funding"]["script"]).map_err(|error| error.to_string())?;
    let funding_amount = number_to_string(funding.value_satoshis as f64 / 100_000_000.0);
    let previous = format!(
        "[{{\"txid\":{txid},\"vout\":0,\"scriptPubKey\":{script},\"amount\":{funding_amount}}}]"
    );
    let vaults = built
        .vaults
        .iter()
        .enumerate()
        .map(|(thread, vault)| {
            let token = vault.token.as_ref().ok_or("genesis vault has no token")?;
            let nft = token.nft.as_ref().ok_or("genesis vault has no NFT")?;
            Ok(json!({
                "thread": thread,
                "value_sats": vault.value_satoshis.to_string(),
                "token_amount": token.amount.to_string(),
                "commitment": hex::encode(&nft.commitment),
            }))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(json!({
        "deployment": {
            "network": covenant.network,
            "threads": covenant.threads,
            "virtual_sats": covenant.virtual_sats.to_string(),
            "category": input["funding"]["txid"],
        },
        "vault_address": vault_address(&covenant)?,
        "redeem_script_bytes": covenant.redeem_script.len(),
        "fee_k": covenant.fee_k,
        "vaults": vaults,
        "genesis_fee_sats": built.fee.to_string(),
        "genesis_size_estimate": built.size,
        "unsigned_genesis": unsigned,
        "sign_with": format!("signrawtransactionwithwallet {unsigned} '{previous}'"),
        "note": "Not broadcast. The owner signs the funding input with their own wallet and decides whether to send it.",
    }))
}

fn unlock(deployment: &Value, input: &Value) -> Result<Value, String> {
    let built = built_claim(deployment, input)?;
    Ok(json!({
        "unlocking_bytecode": hex::encode(&built.transaction.inputs[0].unlocking_bytecode),
    }))
}

fn verify(deployment: &Value, input: &Value) -> Result<Value, String> {
    let built = built_claim(deployment, input)?;
    verify_transaction(&built.transaction, &built.source_outputs)
        .map_err(|error| format!("the covenant refuses this claim: {error}"))?;
    Ok(json!({ "verified": true }))
}
