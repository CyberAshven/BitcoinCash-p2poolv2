// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Compile the checked covenant source for one deployment.

use std::collections::BTreeMap;

use libauth::{
    AuthenticationProgramCommon, BytecodeGenerationResult, CompilationData, CompilerConfiguration,
    big_int_to_vm_number, create_compiler_bch, create_virtual_machine_bch_2026, encode_data_push,
    locking_bytecode_to_cash_address,
};

use crate::{
    BigInt, CLAIM_FIXED_BYTES, Input, MAX_THREADS, MIN_THREADS, Output, TransactionCommon, hash256,
    max_token_target, pow_limit, set_compact,
};

pub const SOURCE: &str = include_str!("../safa.cash");
const MAIN_MARKER: &str = "// == main ==";
/// `OP_INVOKE`, which runs a function `OP_DEFINE` stored (BCH 2026).
const OP_INVOKE: u8 = 0x8a;

/// The interval, in ticks (seconds of target time), of every Bitcoin Cash
/// block before a network's first interval switch, and of the ASERT anchor
/// block on every network: 600 (CHIP-2025-03).
pub const BCH_BLOCK_SPACING: u32 = 600;

/// A switch of a network's block interval pinned to a block (CHIP-2025-03):
/// every block after `height` has `interval` ticks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IntervalSwitch {
    /// The last block with the old interval.
    pub height: u32,
    /// The interval, in ticks, of every block after it.
    pub interval: u32,
}

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
    /// The network's switch from ten-minute blocks, pinned to the block it
    /// follows as P2Pool's header gate pins it (`TickSchedule` in
    /// `p2poolv2_lib`). `None` until the network has switched and a release
    /// pins the block: every block then counts 600 ticks.
    pub interval_switch: Option<IntervalSwitch>,
    pub cash_address_prefix: &'static str,
}

impl NetworkParameters {
    /// The switch the covenant is compiled with. Without a pinned switch it
    /// is the anchor block with an interval of 600, which counts every block
    /// as ten minutes.
    pub fn switch_constants(&self) -> IntervalSwitch {
        self.interval_switch.unwrap_or(IntervalSwitch {
            height: self.anchor_height,
            interval: BCH_BLOCK_SPACING,
        })
    }
}

/// Per-network parameters fixed in the covenant's source template.
pub fn network_parameters(network: &str) -> Result<NetworkParameters, String> {
    match network {
        // One-minute blocks from median time past 1,810,382,400 (2027-05-15
        // 12:00 UTC). The block that reaches it is not known yet, so it is
        // not pinned.
        "mainnet" => Ok(NetworkParameters {
            anchor_height: 661_647,
            anchor_bits: 0x1804dafe,
            anchor_parent_time: 1_605_447_844,
            half_life: 172_800,
            allow_pow_limit: false,
            hard_divisor: 2,
            easy_numerator: 105,
            easy_denominator: 100,
            interval_switch: None,
            cash_address_prefix: "bitcoincash",
        }),
        // One-minute blocks after block 332,450, whose median time past was
        // the first at or after 1,794,744,000 (2026-11-15 12:00 UTC).
        "chipnet" => Ok(NetworkParameters {
            anchor_height: 16_844,
            anchor_bits: 0x1d00ffff,
            anchor_parent_time: 1_605_451_779,
            half_life: 3_600,
            allow_pow_limit: true,
            hard_divisor: 8,
            easy_numerator: 8,
            easy_denominator: 1,
            interval_switch: Some(IntervalSwitch {
                height: 332_450,
                interval: 60,
            }),
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

fn library_source(parameters: &NetworkParameters) -> Result<String, String> {
    let switch = parameters.switch_constants();
    // CHIP-2025-03 allows intervals from 1 to 600 ticks that divide 600, so
    // the anchor block's 600 is a whole multiple of every block's interval.
    // 600 is no multiple of 0.
    if !BCH_BLOCK_SPACING.is_multiple_of(switch.interval) {
        return Err(format!(
            "block interval {} does not divide {BCH_BLOCK_SPACING}",
            switch.interval
        ));
    }
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
            ("ANCHOR_INTERVAL", BCH_BLOCK_SPACING.to_string()),
            ("SWITCH_HEIGHT", switch.height.to_string()),
            ("SWITCH_INTERVAL", switch.interval.to_string()),
            ("HARD_DIVISOR", parameters.hard_divisor.to_string()),
            ("EASY_NUMERATOR", parameters.easy_numerator.to_string()),
            ("EASY_DENOMINATOR", parameters.easy_denominator.to_string()),
        ],
    )
}

/// Compile the covenant's reusable functions followed by a CashASM body.
pub fn compile_library(network: &str, body: &str) -> Result<Vec<u8>, String> {
    compile_library_for(&network_parameters(network)?, body)
}

/// [`compile_library`] for `parameters`, such as a network's with another
/// pinned switch.
pub fn compile_library_for(parameters: &NetworkParameters, body: &str) -> Result<Vec<u8>, String> {
    compile_cash_asm(&format!("{}\n{body}", library_source(parameters)?))
}

/// Run one of the covenant's functions alone in the BCH 2026 VM: the
/// `library` ([`compile_library`] with an empty body), then `arguments`
/// pushed in order and function `identifier` invoked. Returns the stack the
/// function leaves, or the VM's error when it fails.
pub fn evaluate_function(
    library: &[u8],
    identifier: u64,
    arguments: &[Vec<u8>],
) -> Result<Vec<Vec<u8>>, String> {
    let mut script = library.to_vec();
    for argument in arguments {
        script.extend(encode_data_push(argument));
    }
    script.extend(encode_data_push(&big_int_to_vm_number(&BigInt::from_u64(
        identifier,
    ))));
    script.push(OP_INVOKE);
    let state = create_virtual_machine_bch_2026(Some(false)).evaluate(
        &AuthenticationProgramCommon {
            input_index: 0,
            transaction: TransactionCommon {
                version: 2,
                locktime: 0,
                inputs: vec![Input {
                    outpoint_transaction_hash: vec![0; 32],
                    outpoint_index: 0,
                    sequence_number: 0,
                    unlocking_bytecode: Vec::new(),
                }],
                outputs: vec![Output {
                    locking_bytecode: [&[0x6a][..], &[0; 29]].concat(),
                    value_satoshis: 0,
                    token: None,
                }],
            },
            source_outputs: vec![Output {
                locking_bytecode: script,
                value_satoshis: 0,
                token: None,
            }],
        },
        None,
    );
    match state.error {
        // A function may leave several items; only a whole spend must leave one.
        Some(error) if !error.contains("unexpected number of items on the stack") => Err(error),
        _ => Ok(state.stack),
    }
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
    let library = library_source(&network_parameters(network)?)?;
    if !(MIN_THREADS..=MAX_THREADS).contains(&threads) {
        return Err(format!(
            "threads must be an integer from {MIN_THREADS} to {MAX_THREADS}"
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
        let next = fee_constant(&redeem_script);
        if next == fee_k {
            return Ok(covenant_from_redeem_script(
                network,
                threads,
                virtual_sats,
                redeem_script,
            ));
        }
        fee_k = next;
    }
    Err("the fee constant did not converge".into())
}

/// The fee constant K a redeem script was compiled with: the claim's fixed
/// bytes, the script and its push.
fn fee_constant(redeem_script: &[u8]) -> usize {
    CLAIM_FIXED_BYTES + push_prefix_length(redeem_script.len()) + redeem_script.len()
}

/// The covenant of a deployment from its redeem script, as
/// [`create_covenant`] compiled it for these parameters. Vaults on chain stay
/// locked to the script they were created with, whatever `safa.cash` says
/// later, so a recorded deployment is read back this way.
pub fn covenant_from_redeem_script(
    network: &str,
    threads: u32,
    virtual_sats: u64,
    redeem_script: Vec<u8>,
) -> Covenant {
    let mut locking_bytecode = Vec::with_capacity(35);
    locking_bytecode.extend_from_slice(&[0xaa, 0x20]);
    locking_bytecode.extend_from_slice(&hash256(&redeem_script));
    locking_bytecode.push(0x87);
    Covenant {
        network: network.into(),
        threads,
        virtual_sats,
        fee_k: fee_constant(&redeem_script),
        redeem_script,
        lock_hash: hash256(&locking_bytecode),
        locking_bytecode,
    }
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
