// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Compile the checked covenant source for one deployment.

use std::collections::BTreeMap;

use libauth::{
    BytecodeGenerationResult, CompilationData, CompilerConfiguration, create_compiler_bch,
    create_virtual_machine_bch_2026, locking_bytecode_to_cash_address,
};

use crate::{CLAIM_FIXED_BYTES, MAX_THREADS, hash256, max_token_target, pow_limit, set_compact};

pub const SOURCE: &str = include_str!("../safa.cash");
const MAIN_MARKER: &str = "// == main ==";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetworkParameters {
    pub anchor_height: u32,
    pub anchor_bits: u32,
    pub anchor_parent_time: u32,
    pub half_life: u32,
    pub allow_pow_limit: bool,
    pub hard_divisor: u32,
    pub easy_numerator: u32,
    pub easy_denominator: u32,
    pub cash_address_prefix: &'static str,
}

/// Per-network parameters fixed in the covenant's source template.
pub fn network_parameters(network: &str) -> Result<NetworkParameters, String> {
    match network {
        "mainnet" => Ok(NetworkParameters {
            anchor_height: 661_647,
            anchor_bits: 0x1804dafe,
            anchor_parent_time: 1_605_447_844,
            half_life: 172_800,
            allow_pow_limit: false,
            hard_divisor: 2,
            easy_numerator: 105,
            easy_denominator: 100,
            cash_address_prefix: "bitcoincash",
        }),
        "chipnet" => Ok(NetworkParameters {
            anchor_height: 16_844,
            anchor_bits: 0x1d00ffff,
            anchor_parent_time: 1_605_451_779,
            half_life: 3_600,
            allow_pow_limit: true,
            hard_divisor: 8,
            easy_numerator: 8,
            easy_denominator: 1,
            cash_address_prefix: "bchtest",
        }),
        _ => Err(format!("unknown network {network}: mainnet or chipnet")),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Covenant {
    pub network: String,
    pub threads: u32,
    pub virtual_sats: u64,
    pub fee_k: usize,
    pub redeem_script: Vec<u8>,
    pub locking_bytecode: Vec<u8>,
    pub lock_hash: Vec<u8>,
}

/// Compile CashAssembly using libauth-rs's BCH compiler and BCH 2026 VM.
pub fn compile_cash_asm(text: &str) -> Result<Vec<u8>, String> {
    let compiler = create_compiler_bch(CompilerConfiguration {
        scripts: BTreeMap::from([("lock".into(), text.into())]),
        vm: Some(create_virtual_machine_bch_2026(Some(false))),
        ..Default::default()
    });
    match compiler.generate_bytecode(&CompilationData::default(), "lock") {
        BytecodeGenerationResult::Success { bytecode } => Ok(bytecode),
        BytecodeGenerationResult::Error(error) => {
            Err(format!("CashAssembly compilation failed: {error:?}"))
        }
    }
}

fn substitute(text: &str, values: &[(&str, String)]) -> Result<String, String> {
    let mut replaced = text.to_string();
    for (name, value) in values {
        replaced = replaced.replace(&format!("{{{{{name}}}}}"), value);
    }
    if replaced.contains("{{") {
        return Err("unresolved covenant parameter".into());
    }
    Ok(replaced)
}

fn library_source(network: &str) -> Result<String, String> {
    let parameters = network_parameters(network)?;
    let (library, _) = SOURCE
        .split_once(MAIN_MARKER)
        .ok_or("safa.cash has no main marker")?;
    substitute(
        library,
        &[
            ("POW_LIMIT", pow_limit().to_string()),
            (
                "ALLOW_POW_LIMIT",
                u8::from(parameters.allow_pow_limit).to_string(),
            ),
            ("ANCHOR_HEIGHT", parameters.anchor_height.to_string()),
            (
                "ANCHOR_PARENT_TIME",
                parameters.anchor_parent_time.to_string(),
            ),
            (
                "ANCHOR_TARGET",
                set_compact(parameters.anchor_bits)
                    .ok_or("invalid ASERT anchor")?
                    .to_string(),
            ),
            ("HALF_LIFE", parameters.half_life.to_string()),
            ("HARD_DIVISOR", parameters.hard_divisor.to_string()),
            ("EASY_NUMERATOR", parameters.easy_numerator.to_string()),
            ("EASY_DENOMINATOR", parameters.easy_denominator.to_string()),
        ],
    )
}

/// Compile the covenant's reusable functions followed by a CashASM body.
pub fn compile_library(network: &str, body: &str) -> Result<Vec<u8>, String> {
    compile_cash_asm(&format!("{}\n{body}", library_source(network)?))
}

fn push_prefix_length(length: usize) -> usize {
    match length {
        0..=75 => 1,
        76..=255 => 2,
        256..=65_535 => 3,
        _ => 5,
    }
}

/// Compile a deployment, iterating until its size-dependent fee constant settles.
pub fn create_covenant(network: &str, threads: u32, virtual_sats: u64) -> Result<Covenant, String> {
    let library = library_source(network)?;
    if !(1..=MAX_THREADS).contains(&threads) {
        return Err(format!(
            "threads must be an integer from 1 to {MAX_THREADS}"
        ));
    }
    if virtual_sats > (1u64 << 62) {
        return Err("virtual_sats must be from 0 to 2^62".into());
    }
    let (_, main) = SOURCE
        .split_once(MAIN_MARKER)
        .ok_or("safa.cash has no main marker")?;
    let mut fee_k = CLAIM_FIXED_BYTES + 3 + 2_000;
    for _round in 0..8 {
        let main = substitute(
            main,
            &[
                ("FEE_K", fee_k.to_string()),
                ("THREADS", threads.to_string()),
                ("VIRTUAL_SATS", virtual_sats.to_string()),
                ("MAX_TOKEN_TARGET", max_token_target().to_string()),
            ],
        )?;
        let redeem_script = compile_cash_asm(&format!("{library}\n{main}"))?;
        let next =
            CLAIM_FIXED_BYTES + push_prefix_length(redeem_script.len()) + redeem_script.len();
        if next == fee_k {
            let mut locking_bytecode = Vec::with_capacity(35);
            locking_bytecode.extend_from_slice(&[0xaa, 0x20]);
            locking_bytecode.extend_from_slice(&hash256(&redeem_script));
            locking_bytecode.push(0x87);
            return Ok(Covenant {
                network: network.into(),
                threads,
                virtual_sats,
                fee_k,
                redeem_script,
                lock_hash: hash256(&locking_bytecode),
                locking_bytecode,
            });
        }
        fee_k = next;
    }
    Err("the fee constant did not converge".into())
}

/// Encode the token-aware P2SH32 vault address for this deployment.
pub fn vault_address(covenant: &Covenant) -> Result<String, String> {
    let parameters = network_parameters(&covenant.network)?;
    locking_bytecode_to_cash_address(
        Some(parameters.cash_address_prefix),
        &covenant.locking_bytecode,
        Some(true),
    )
    .map(|result| result.address)
}
