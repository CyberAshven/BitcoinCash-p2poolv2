// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A Bitcoin Cash script machine for tests, run opcode by opcode over the
//! actual bytes.
//!
//! No BCHN runs in the unit tests, so contracts the share chain or the payout
//! builds are executed here the way BCHN executes them, and tests check that
//! honest spends pass and dishonest ones fail. The machine follows BCHN's
//! rules (2026 upgrade, standard relay flags) for the opcodes it implements:
//!
//! - push-only unlocking bytecode of at most 10,000 bytes (P2S, 2026), with
//!   minimal pushes and minimally encoded numbers;
//! - P2SH32 evaluation of the redeem script and a clean stack;
//! - minimal-if, BIP65 lock time and null-fail;
//! - loops (`OP_BEGIN`/`OP_UNTIL`, 2026), `OP_CAT`, `OP_SPLIT`, `OP_NUM2BIN`,
//!   `OP_BIN2NUM`, arithmetic, introspection (2022) and token introspection
//!   (2023), reading CashTokens prefixes from locking bytecode;
//! - the VM limits (2025): every evaluated instruction costs 100, every push
//!   to the stack its length (twice for `OP_ADD`-like results), every SHA-256
//!   block 192 at relay (standard) validation, a signature check 26,000; an
//!   input may spend `(41 + unlocking length) * 800` operation cost and
//!   `(41 + unlocking length) / 2` hash iterations; stack items hold at most
//!   10,000 bytes and the stacks at most 1,000 items.
//!
//! Any other opcode is an error. Sources: the VM Limits CHIP
//! (github.com/bitjson/bch-vm-limits), the Loops CHIP
//! (github.com/bitjson/bch-loops), the introspection and CashTokens upgrade
//! specifications (upgradespecs.bitcoincashnode.org) and BCHN's
//! `src/script/interpreter.cpp`.
//!
//! Numbers are limited to 16 bytes here (BCH allows 10,000): every script
//! tested stays far below.

use crate::shares::bch_htlc::p2sh32_hash;
use crate::shares::bch_script::verify_signature;
use bitcoin::hashes::{Hash, hash160, sha256, sha256d};
use bitcoin::{Sequence, Transaction, TxOut};

const OP_0: u8 = 0x00;
const OP_PUSHDATA1: u8 = 0x4c;
const OP_PUSHDATA2: u8 = 0x4d;
const OP_PUSHDATA4: u8 = 0x4e;
const OP_1NEGATE: u8 = 0x4f;
const OP_RESERVED: u8 = 0x50;
const OP_1: u8 = 0x51;
const OP_16: u8 = 0x60;
const OP_IF: u8 = 0x63;
const OP_NOTIF: u8 = 0x64;
const OP_BEGIN: u8 = 0x65;
const OP_UNTIL: u8 = 0x66;
const OP_ELSE: u8 = 0x67;
const OP_ENDIF: u8 = 0x68;
const OP_VERIFY: u8 = 0x69;
const OP_2DUP: u8 = 0x6e;
const OP_DROP: u8 = 0x75;
const OP_DUP: u8 = 0x76;
const OP_NIP: u8 = 0x77;
const OP_OVER: u8 = 0x78;
const OP_PICK: u8 = 0x79;
const OP_ROT: u8 = 0x7b;
const OP_SWAP: u8 = 0x7c;
const OP_TUCK: u8 = 0x7d;
const OP_CAT: u8 = 0x7e;
const OP_SPLIT: u8 = 0x7f;
const OP_NUM2BIN: u8 = 0x80;
const OP_BIN2NUM: u8 = 0x81;
const OP_SIZE: u8 = 0x82;
const OP_EQUAL: u8 = 0x87;
const OP_EQUALVERIFY: u8 = 0x88;
const OP_1ADD: u8 = 0x8b;
const OP_1SUB: u8 = 0x8c;
const OP_NOT: u8 = 0x91;
const OP_ADD: u8 = 0x93;
const OP_SUB: u8 = 0x94;
const OP_BOOLAND: u8 = 0x9a;
const OP_BOOLOR: u8 = 0x9b;
const OP_NUMEQUAL: u8 = 0x9c;
const OP_NUMEQUALVERIFY: u8 = 0x9d;
const OP_LESSTHAN: u8 = 0x9f;
const OP_GREATERTHAN: u8 = 0xa0;
const OP_SHA256: u8 = 0xa8;
const OP_HASH160: u8 = 0xa9;
const OP_HASH256: u8 = 0xaa;
const OP_CHECKSIG: u8 = 0xac;
const OP_CHECKLOCKTIMEVERIFY: u8 = 0xb1;
const OP_INPUTINDEX: u8 = 0xc0;
const OP_ACTIVEBYTECODE: u8 = 0xc1;
const OP_TXVERSION: u8 = 0xc2;
const OP_TXINPUTCOUNT: u8 = 0xc3;
const OP_TXOUTPUTCOUNT: u8 = 0xc4;
const OP_TXLOCKTIME: u8 = 0xc5;
const OP_UTXOVALUE: u8 = 0xc6;
const OP_UTXOBYTECODE: u8 = 0xc7;
const OP_OUTPOINTTXHASH: u8 = 0xc8;
const OP_OUTPOINTINDEX: u8 = 0xc9;
const OP_INPUTBYTECODE: u8 = 0xca;
const OP_INPUTSEQUENCENUMBER: u8 = 0xcb;
const OP_OUTPUTVALUE: u8 = 0xcc;
const OP_OUTPUTBYTECODE: u8 = 0xcd;
const OP_UTXOTOKENCATEGORY: u8 = 0xce;
const OP_OUTPUTTOKENCATEGORY: u8 = 0xd1;

/// CashTokens prefix opening a token output's locking bytecode field.
pub const PREFIX_TOKEN: u8 = 0xef;
const TOKEN_HAS_COMMITMENT_LENGTH: u8 = 0x40;
const TOKEN_HAS_NFT: u8 = 0x20;
const TOKEN_HAS_AMOUNT: u8 = 0x10;
const TOKEN_CAPABILITY_MASK: u8 = 0x0f;

const LOCKTIME_THRESHOLD: u64 = 500_000_000;
const MAX_SCRIPT_ELEMENT_SIZE: usize = 10_000;
const MAX_STACK_SIZE: usize = 1_000;
const MAX_UNLOCKING_BYTECODE_LENGTH: usize = 10_000;
const MAX_NUMBER_BYTES: usize = 16;
const DENSITY_CONTROL_BASE: u64 = 41;
const OPERATION_COST_PER_DENSITY_BYTE: u64 = 800;
const BASE_INSTRUCTION_COST: u64 = 100;
const STANDARD_HASH_ITERATION_COST: u64 = 192;
const SIGNATURE_CHECK_COST: u64 = 26_000;

/// What an input's evaluation used, against what the VM limits allow it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpendMetrics {
    /// Operation cost spent across unlocking, locking and redeem bytecode.
    pub operation_cost: u64,
    /// `(41 + unlocking length) * 800`.
    pub operation_cost_limit: u64,
    /// SHA-256 compression blocks hashed.
    pub hash_iterations: u64,
    /// `(41 + unlocking length) / 2`, the relay (standard) limit.
    pub hash_iteration_limit: u64,
}

/// What the machine reads of the spending transaction.
#[derive(Clone, Copy)]
struct Context<'a> {
    transaction: &'a Transaction,
    input_index: usize,
    spent_outputs: &'a [TxOut],
    script_code: &'a [u8],
}

/// One entry of the control stack: a conditional branch or an open loop.
#[derive(Debug, Clone, Copy)]
enum Control {
    Branch(bool),
    /// The position right after the loop's `OP_BEGIN`.
    Loop(usize),
}

/// Token data a CashTokens prefix carries.
struct Token {
    category: [u8; 32],
    capability: Option<u8>,
}

struct Machine<'a> {
    context: Context<'a>,
    metrics: SpendMetrics,
}

/// Run input `input_index` of `transaction` against the P2SH32 output it
/// spends, every other input spending the same output. `Ok` means BCHN would
/// accept the spend. For contracts that read no other input.
pub fn run_p2sh32_spend(
    transaction: &Transaction,
    input_index: usize,
    spent_output: &TxOut,
) -> Result<(), String> {
    let spent_outputs = vec![spent_output.clone(); transaction.input.len()];
    run_p2sh32_spend_with_utxos(transaction, input_index, &spent_outputs).map(|_| ())
}

/// Run input `input_index` of `transaction` against the P2SH32 output it
/// spends, with `spent_outputs` the outputs every input spends, in input
/// order. Returns what the input used of the VM limits.
pub fn run_p2sh32_spend_with_utxos(
    transaction: &Transaction,
    input_index: usize,
    spent_outputs: &[TxOut],
) -> Result<SpendMetrics, String> {
    if spent_outputs.len() != transaction.input.len() {
        return Err("one spent output per input is required".to_string());
    }
    let input = transaction
        .input
        .get(input_index)
        .ok_or("input out of range")?;
    let unlocking_script = input.script_sig.as_bytes();
    if unlocking_script.len() > MAX_UNLOCKING_BYTECODE_LENGTH {
        return Err(format!(
            "unlocking bytecode of {} bytes exceeds {MAX_UNLOCKING_BYTECODE_LENGTH}",
            unlocking_script.len()
        ));
    }
    let (_, locking_script) =
        split_token_prefix(spent_outputs[input_index].script_pubkey.as_bytes())?;
    if p2sh32_hash(locking_script).is_none() {
        return Err("the spent output is not P2SH32".to_string());
    }
    let density = DENSITY_CONTROL_BASE + unlocking_script.len() as u64;
    let context = Context {
        transaction,
        input_index,
        spent_outputs,
        script_code: unlocking_script,
    };
    let mut machine = Machine {
        context,
        metrics: SpendMetrics {
            operation_cost: 0,
            operation_cost_limit: density * OPERATION_COST_PER_DENSITY_BYTE,
            hash_iterations: 0,
            hash_iteration_limit: density / 2,
        },
    };
    let mut stack: Vec<Vec<u8>> = Vec::with_capacity(8);
    machine.execute(unlocking_script, &mut stack, true)?;
    let mut redeem_stack = stack.clone();
    machine.context.script_code = locking_script;
    machine.execute(locking_script, &mut stack, false)?;
    if !stack.last().is_some_and(|top| cast_to_bool(top)) {
        return Err("the P2SH32 hash does not match".to_string());
    }
    let redeem_script = redeem_stack.pop().ok_or("no redeem script")?;
    machine.context.script_code = &redeem_script;
    machine.execute(&redeem_script, &mut redeem_stack, false)?;
    if redeem_stack.len() != 1 || !cast_to_bool(&redeem_stack[0]) {
        return Err(format!(
            "the redeem script left {} items, not one true item",
            redeem_stack.len()
        ));
    }
    Ok(machine.metrics)
}

impl Machine<'_> {
    fn execute(
        &mut self,
        script: &[u8],
        stack: &mut Vec<Vec<u8>>,
        push_only: bool,
    ) -> Result<(), String> {
        let mut control: Vec<Control> = Vec::with_capacity(4);
        let mut position = 0;
        while position < script.len() {
            let opcode = script[position];
            position += 1;
            self.metrics.operation_cost += BASE_INSTRUCTION_COST;
            let executing = control.iter().all(|entry| match entry {
                Control::Branch(taken) => *taken,
                Control::Loop(_) => true,
            });
            if opcode <= OP_PUSHDATA4 {
                let data = read_push(script, opcode, &mut position)?;
                if executing {
                    if !is_minimal_push(opcode, &data) {
                        return Err("a push is not minimal".to_string());
                    }
                    self.push(stack, data, 1)?;
                }
            } else if push_only && opcode > OP_16 {
                return Err(format!("opcode {opcode:#04x} in a push-only script"));
            } else {
                self.execute_opcode(opcode, executing, stack, &mut control, &mut position)?;
            }
            if stack.len() > MAX_STACK_SIZE {
                return Err("stack overflow".to_string());
            }
            self.check_limits()?;
        }
        if control.is_empty() {
            Ok(())
        } else {
            Err("unbalanced OP_IF or OP_BEGIN".to_string())
        }
    }

    fn check_limits(&self) -> Result<(), String> {
        if self.metrics.operation_cost > self.metrics.operation_cost_limit {
            return Err(format!(
                "operation cost {} exceeds the limit {}",
                self.metrics.operation_cost, self.metrics.operation_cost_limit
            ));
        }
        if self.metrics.hash_iterations > self.metrics.hash_iteration_limit {
            return Err(format!(
                "hash iterations {} exceed the standard limit {}",
                self.metrics.hash_iterations, self.metrics.hash_iteration_limit
            ));
        }
        Ok(())
    }

    /// Push `item`, tallying its length `factor` times.
    fn push(&mut self, stack: &mut Vec<Vec<u8>>, item: Vec<u8>, factor: u64) -> Result<(), String> {
        if item.len() > MAX_SCRIPT_ELEMENT_SIZE {
            return Err(format!(
                "a stack item of {} bytes exceeds {MAX_SCRIPT_ELEMENT_SIZE}",
                item.len()
            ));
        }
        self.metrics.operation_cost += factor * item.len() as u64;
        stack.push(item);
        Ok(())
    }

    /// Count the SHA-256 blocks hashing `length` bytes takes, once or twice.
    fn tally_hash(&mut self, length: usize, double: bool) {
        let iterations = 1 + (length as u64 + 8) / 64 + u64::from(double);
        self.metrics.hash_iterations += iterations;
        self.metrics.operation_cost += iterations * STANDARD_HASH_ITERATION_COST;
    }

    fn execute_opcode(
        &mut self,
        opcode: u8,
        executing: bool,
        stack: &mut Vec<Vec<u8>>,
        control: &mut Vec<Control>,
        position: &mut usize,
    ) -> Result<(), String> {
        match opcode {
            OP_IF | OP_NOTIF => {
                let taken = if executing {
                    let condition = pop(stack)?;
                    if !(condition.is_empty() || condition == [1]) {
                        return Err("OP_IF argument is not minimal".to_string());
                    }
                    cast_to_bool(&condition) == (opcode == OP_IF)
                } else {
                    false
                };
                control.push(Control::Branch(taken));
                Ok(())
            }
            OP_ELSE => match control.last_mut() {
                Some(Control::Branch(taken)) => {
                    *taken = !*taken;
                    Ok(())
                }
                _ => Err("OP_ELSE without OP_IF".to_string()),
            },
            OP_ENDIF => match control.last() {
                Some(Control::Branch(_)) => {
                    control.pop();
                    Ok(())
                }
                _ => Err("OP_ENDIF without OP_IF".to_string()),
            },
            OP_BEGIN => {
                control.push(Control::Loop(*position));
                Ok(())
            }
            OP_UNTIL => {
                let start = match control.last() {
                    Some(Control::Loop(start)) => *start,
                    _ => return Err("OP_UNTIL without OP_BEGIN".to_string()),
                };
                if executing && !cast_to_bool(&pop(stack)?) {
                    *position = start;
                } else {
                    control.pop();
                }
                Ok(())
            }
            _ if !executing => Ok(()),
            OP_1NEGATE => self.push(stack, vec![0x81], 1),
            OP_RESERVED => Err("OP_RESERVED".to_string()),
            OP_1..=OP_16 => self.push(stack, vec![opcode - OP_1 + 1], 1),
            OP_VERIFY => {
                if cast_to_bool(&pop(stack)?) {
                    Ok(())
                } else {
                    Err("OP_VERIFY failed".to_string())
                }
            }
            _ => self.execute_data_opcode(opcode, stack),
        }
    }

    fn execute_data_opcode(&mut self, opcode: u8, stack: &mut Vec<Vec<u8>>) -> Result<(), String> {
        match opcode {
            OP_DROP => pop(stack).map(|_| ()),
            OP_DUP => {
                let item = peek(stack, 0)?.clone();
                self.push(stack, item, 1)
            }
            OP_2DUP => {
                let first = peek(stack, 1)?.clone();
                let second = peek(stack, 0)?.clone();
                self.push(stack, first, 1)?;
                self.push(stack, second, 1)
            }
            OP_NIP => {
                let top = pop(stack)?;
                pop(stack)?;
                stack.push(top);
                Ok(())
            }
            OP_OVER => {
                let item = peek(stack, 1)?.clone();
                self.push(stack, item, 1)
            }
            OP_PICK => {
                let depth = usize::try_from(decode_number(&pop(stack)?)?)
                    .map_err(|_| "OP_PICK depth is negative".to_string())?;
                let item = peek(stack, depth)?.clone();
                self.push(stack, item, 1)
            }
            OP_ROT => {
                if stack.len() < 3 {
                    return Err("stack too short for OP_ROT".to_string());
                }
                let item = stack.remove(stack.len() - 3);
                stack.push(item);
                Ok(())
            }
            OP_SWAP => {
                let length = stack.len();
                if length < 2 {
                    return Err("stack too short for OP_SWAP".to_string());
                }
                stack.swap(length - 1, length - 2);
                Ok(())
            }
            OP_TUCK => {
                let top = peek(stack, 0)?.clone();
                let length = stack.len();
                if length < 2 {
                    return Err("stack too short for OP_TUCK".to_string());
                }
                self.metrics.operation_cost += top.len() as u64;
                stack.insert(length - 2, top);
                Ok(())
            }
            OP_CAT => {
                let right = pop(stack)?;
                let mut left = pop(stack)?;
                left.extend_from_slice(&right);
                self.push(stack, left, 1)
            }
            OP_SPLIT => {
                let split_at = usize::try_from(decode_number(&pop(stack)?)?)
                    .map_err(|_| "OP_SPLIT position is negative".to_string())?;
                let mut data = pop(stack)?;
                if split_at > data.len() {
                    return Err("OP_SPLIT position out of range".to_string());
                }
                let right = data.split_off(split_at);
                self.push(stack, data, 1)?;
                self.push(stack, right, 1)
            }
            OP_NUM2BIN => {
                let size = usize::try_from(decode_number(&pop(stack)?)?)
                    .map_err(|_| "OP_NUM2BIN size is negative".to_string())?;
                let raw = pop(stack)?;
                let item = num_to_bin(&raw, size)?;
                self.push(stack, item, 1)
            }
            OP_BIN2NUM => {
                let raw = pop(stack)?;
                let number = minimally_encode(&raw);
                if number.len() > MAX_NUMBER_BYTES {
                    return Err("OP_BIN2NUM result is too large".to_string());
                }
                self.push(stack, number, 1)
            }
            OP_SIZE => {
                let length = peek(stack, 0)?.len();
                self.push(stack, encode_number(length as i128), 1)
            }
            OP_EQUAL | OP_EQUALVERIFY => {
                let right = pop(stack)?;
                let left = pop(stack)?;
                self.push(stack, encode_bool(left == right), 1)?;
                if opcode == OP_EQUALVERIFY {
                    return verify_top(stack, "OP_EQUALVERIFY");
                }
                Ok(())
            }
            OP_1ADD | OP_1SUB | OP_NOT => {
                let value = decode_number(&pop(stack)?)?;
                let (result, factor) = match opcode {
                    OP_1ADD => (value + 1, 2),
                    OP_1SUB => (value - 1, 2),
                    _ => (i128::from(value == 0), 1),
                };
                self.push(stack, encode_number(result), factor)
            }
            OP_ADD | OP_SUB | OP_BOOLAND | OP_BOOLOR | OP_NUMEQUAL | OP_NUMEQUALVERIFY
            | OP_LESSTHAN | OP_GREATERTHAN => {
                let right = decode_number(&pop(stack)?)?;
                let left = decode_number(&pop(stack)?)?;
                let (result, factor) = match opcode {
                    OP_ADD => (left + right, 2),
                    OP_SUB => (left - right, 2),
                    OP_BOOLAND => (i128::from(left != 0 && right != 0), 1),
                    OP_BOOLOR => (i128::from(left != 0 || right != 0), 1),
                    OP_LESSTHAN => (i128::from(left < right), 1),
                    OP_GREATERTHAN => (i128::from(left > right), 1),
                    _ => (i128::from(left == right), 1),
                };
                self.push(stack, encode_number(result), factor)?;
                if opcode == OP_NUMEQUALVERIFY {
                    return verify_top(stack, "OP_NUMEQUALVERIFY");
                }
                Ok(())
            }
            OP_SHA256 => {
                let data = pop(stack)?;
                self.tally_hash(data.len(), false);
                self.push(stack, sha256::Hash::hash(&data).to_byte_array().to_vec(), 1)
            }
            OP_HASH160 => {
                let data = pop(stack)?;
                self.tally_hash(data.len(), true);
                self.push(
                    stack,
                    hash160::Hash::hash(&data).to_byte_array().to_vec(),
                    1,
                )
            }
            OP_HASH256 => {
                let data = pop(stack)?;
                self.tally_hash(data.len(), true);
                self.push(
                    stack,
                    sha256d::Hash::hash(&data).to_byte_array().to_vec(),
                    1,
                )
            }
            OP_CHECKSIG => self.check_signature(stack),
            OP_CHECKLOCKTIMEVERIFY => check_lock_time(stack, self.context),
            _ => self.execute_introspection(opcode, stack),
        }
    }

    fn execute_introspection(
        &mut self,
        opcode: u8,
        stack: &mut Vec<Vec<u8>>,
    ) -> Result<(), String> {
        let transaction = self.context.transaction;
        let item = match opcode {
            OP_INPUTINDEX => encode_number(self.context.input_index as i128),
            OP_ACTIVEBYTECODE => self.context.script_code.to_vec(),
            OP_TXVERSION => encode_number(i128::from(transaction.version.0)),
            OP_TXINPUTCOUNT => encode_number(transaction.input.len() as i128),
            OP_TXOUTPUTCOUNT => encode_number(transaction.output.len() as i128),
            OP_TXLOCKTIME => encode_number(i128::from(transaction.lock_time.to_consensus_u32())),
            OP_UTXOVALUE
            | OP_UTXOBYTECODE
            | OP_OUTPOINTTXHASH
            | OP_OUTPOINTINDEX
            | OP_INPUTBYTECODE
            | OP_INPUTSEQUENCENUMBER
            | OP_UTXOTOKENCATEGORY => {
                let index = pop_index(stack, transaction.input.len())?;
                let input = &transaction.input[index];
                let spent = &self.context.spent_outputs[index];
                match opcode {
                    OP_UTXOVALUE => encode_number(i128::from(spent.value.to_sat())),
                    OP_UTXOBYTECODE => split_token_prefix(spent.script_pubkey.as_bytes())?
                        .1
                        .to_vec(),
                    OP_OUTPOINTTXHASH => input.previous_output.txid.to_byte_array().to_vec(),
                    OP_OUTPOINTINDEX => encode_number(i128::from(input.previous_output.vout)),
                    OP_INPUTBYTECODE => input.script_sig.as_bytes().to_vec(),
                    OP_INPUTSEQUENCENUMBER => encode_number(i128::from(input.sequence.0)),
                    _ => token_category(spent)?,
                }
            }
            OP_OUTPUTVALUE | OP_OUTPUTBYTECODE | OP_OUTPUTTOKENCATEGORY => {
                let index = pop_index(stack, transaction.output.len())?;
                let output = &transaction.output[index];
                match opcode {
                    OP_OUTPUTVALUE => encode_number(i128::from(output.value.to_sat())),
                    OP_OUTPUTBYTECODE => split_token_prefix(output.script_pubkey.as_bytes())?
                        .1
                        .to_vec(),
                    _ => token_category(output)?,
                }
            }
            other => return Err(format!("unsupported opcode {other:#04x}")),
        };
        self.push(stack, item, 1)
    }

    /// `OP_CHECKSIG` with null-fail: a failing non-empty signature is an
    /// error rather than a false result.
    fn check_signature(&mut self, stack: &mut Vec<Vec<u8>>) -> Result<(), String> {
        let public_key = pop(stack)?;
        let signature = pop(stack)?;
        let valid = !signature.is_empty()
            && verify_signature(
                self.context.transaction,
                self.context.input_index,
                self.context.script_code,
                self.context.spent_outputs[self.context.input_index]
                    .value
                    .to_sat(),
                &signature,
                &public_key,
            )
            .is_ok();
        if !valid && !signature.is_empty() {
            return Err("OP_CHECKSIG failed with a non-empty signature".to_string());
        }
        if !signature.is_empty() {
            self.metrics.operation_cost += SIGNATURE_CHECK_COST;
        }
        self.push(stack, encode_bool(valid), 1)
    }
}

/// Read the data a push opcode pushes.
fn read_push(script: &[u8], opcode: u8, position: &mut usize) -> Result<Vec<u8>, String> {
    let length_bytes = match opcode {
        OP_PUSHDATA1 => 1,
        OP_PUSHDATA2 => 2,
        OP_PUSHDATA4 => 4,
        _ => 0,
    };
    let length = if length_bytes == 0 {
        usize::from(opcode)
    } else {
        let bytes = script
            .get(*position..*position + length_bytes)
            .ok_or("truncated push length")?;
        *position += length_bytes;
        bytes
            .iter()
            .rev()
            .fold(0usize, |length, byte| (length << 8) | usize::from(*byte))
    };
    let data = script
        .get(*position..*position + length)
        .ok_or("truncated push")?
        .to_vec();
    *position += length;
    Ok(data)
}

/// BCHN's `CheckMinimalPush`: the shortest opcode that pushes `data`.
fn is_minimal_push(opcode: u8, data: &[u8]) -> bool {
    match data.len() {
        0 => opcode == OP_0,
        1 if (1..=16).contains(&data[0]) || data[0] == 0x81 => false,
        length if length <= 75 => usize::from(opcode) == length,
        length if length <= 255 => opcode == OP_PUSHDATA1,
        length if length <= 65_535 => opcode == OP_PUSHDATA2,
        _ => true,
    }
}

/// Split a locking bytecode field into its CashTokens data, if any, and the
/// locking bytecode itself.
fn split_token_prefix(field: &[u8]) -> Result<(Option<Token>, &[u8]), String> {
    if field.first() != Some(&PREFIX_TOKEN) {
        return Ok((None, field));
    }
    let category: [u8; 32] = field
        .get(1..33)
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or("truncated token category")?;
    let bitfield = *field.get(33).ok_or("truncated token bitfield")?;
    let mut position = 34;
    if bitfield & TOKEN_HAS_COMMITMENT_LENGTH != 0 {
        let length = usize::from(*field.get(position).ok_or("truncated commitment length")?);
        position += 1 + length;
    }
    if bitfield & TOKEN_HAS_AMOUNT != 0 {
        let first = *field.get(position).ok_or("truncated token amount")?;
        position += match first {
            0xfd => 3,
            0xfe => 5,
            0xff => 9,
            _ => 1,
        };
    }
    let rest = field.get(position..).ok_or("truncated token prefix")?;
    let capability = (bitfield & TOKEN_HAS_NFT != 0).then_some(bitfield & TOKEN_CAPABILITY_MASK);
    Ok((
        Some(Token {
            category,
            capability,
        }),
        rest,
    ))
}

/// What `OP_OUTPUTTOKENCATEGORY` and `OP_UTXOTOKENCATEGORY` push: nothing
/// (the number 0) for an output without tokens, else the category, followed
/// by the capability byte for a mutable or minting NFT.
fn token_category(output: &TxOut) -> Result<Vec<u8>, String> {
    let (token, _) = split_token_prefix(output.script_pubkey.as_bytes())?;
    Ok(match token {
        None => Vec::new(),
        Some(token) => {
            let mut category = token.category.to_vec();
            if let Some(capability) = token.capability.filter(|capability| *capability != 0) {
                category.push(capability);
            }
            category
        }
    })
}

/// BIP65: the top item is a non-negative number of at most five bytes, of
/// the same kind (height or time) as the transaction's lock time and no
/// greater than it, and the input's sequence is not final.
fn check_lock_time(stack: &[Vec<u8>], context: Context) -> Result<(), String> {
    let top = stack.last().ok_or("empty stack")?;
    if top.len() > 5 {
        return Err("lock time is longer than five bytes".to_string());
    }
    let required = decode_number(top)?;
    if required < 0 {
        return Err("lock time is negative".to_string());
    }
    let required = required as u64;
    let lock_time = u64::from(context.transaction.lock_time.to_consensus_u32());
    if (required < LOCKTIME_THRESHOLD) != (lock_time < LOCKTIME_THRESHOLD) {
        return Err("lock time kinds differ".to_string());
    }
    if required > lock_time {
        return Err("lock time not reached".to_string());
    }
    if context.transaction.input[context.input_index].sequence == Sequence::MAX {
        return Err("final sequence".to_string());
    }
    Ok(())
}

fn pop(stack: &mut Vec<Vec<u8>>) -> Result<Vec<u8>, String> {
    stack.pop().ok_or_else(|| "empty stack".to_string())
}

/// The item `depth` below the top.
fn peek(stack: &[Vec<u8>], depth: usize) -> Result<&Vec<u8>, String> {
    stack
        .len()
        .checked_sub(depth + 1)
        .map(|index| &stack[index])
        .ok_or_else(|| "stack too short".to_string())
}

/// Pop an input or output index, which must be below `count`.
fn pop_index(stack: &mut Vec<Vec<u8>>, count: usize) -> Result<usize, String> {
    let index = decode_number(&pop(stack)?)?;
    usize::try_from(index)
        .ok()
        .filter(|index| *index < count)
        .ok_or_else(|| format!("introspection index {index} out of range"))
}

/// Pop the result a `...VERIFY` opcode pushed, failing if it is false.
fn verify_top(stack: &mut Vec<Vec<u8>>, opcode_name: &str) -> Result<(), String> {
    if cast_to_bool(&pop(stack)?) {
        Ok(())
    } else {
        Err(format!("{opcode_name} failed"))
    }
}

fn encode_bool(value: bool) -> Vec<u8> {
    if value { vec![1] } else { Vec::new() }
}

/// Decode a minimally encoded script number.
fn decode_number(bytes: &[u8]) -> Result<i128, String> {
    if bytes.len() > MAX_NUMBER_BYTES {
        return Err("number too large for the test machine".to_string());
    }
    let Some((&last, rest)) = bytes.split_last() else {
        return Ok(0);
    };
    if last & 0x7f == 0 && rest.last().is_none_or(|byte| byte & 0x80 == 0) {
        return Err("number is not minimally encoded".to_string());
    }
    let magnitude = bytes
        .iter()
        .enumerate()
        .fold(0i128, |value, (index, byte)| {
            let byte = if index == bytes.len() - 1 {
                byte & 0x7f
            } else {
                *byte
            };
            value | (i128::from(byte) << (8 * index))
        });
    Ok(if last & 0x80 != 0 {
        -magnitude
    } else {
        magnitude
    })
}

/// Minimal script number encoding.
fn encode_number(value: i128) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(9);
    let negative = value < 0;
    let mut magnitude = value.unsigned_abs();
    while magnitude > 0 {
        bytes.push((magnitude & 0xff) as u8);
        magnitude >>= 8;
    }
    if let Some(last) = bytes.last_mut() {
        if *last & 0x80 != 0 {
            bytes.push(if negative { 0x80 } else { 0x00 });
        } else if negative {
            *last |= 0x80;
        }
    }
    bytes
}

/// BCHN's `MinimallyEncode`: drop zero padding before the sign byte.
fn minimally_encode(raw: &[u8]) -> Vec<u8> {
    let mut bytes = raw.to_vec();
    let Some(&last) = bytes.last() else {
        return bytes;
    };
    let sign = last & 0x80;
    let Some(last_index) = bytes.len().checked_sub(1) else {
        return bytes;
    };
    bytes[last_index] &= 0x7f;
    while bytes.last() == Some(&0) {
        bytes.pop();
    }
    if let Some(top) = bytes.last_mut() {
        if *top & 0x80 != 0 {
            bytes.push(sign);
        } else {
            *top |= sign;
        }
    }
    bytes
}

/// `OP_NUM2BIN`: `raw` as a number padded to `size` bytes, the sign bit
/// moved to the last byte.
fn num_to_bin(raw: &[u8], size: usize) -> Result<Vec<u8>, String> {
    if size > MAX_SCRIPT_ELEMENT_SIZE {
        return Err("OP_NUM2BIN size exceeds the element limit".to_string());
    }
    let mut number = minimally_encode(raw);
    if number.len() > size {
        return Err("OP_NUM2BIN cannot fit the number".to_string());
    }
    let sign = number.last_mut().map_or(0, |last| {
        let sign = *last & 0x80;
        *last &= 0x7f;
        sign
    });
    number.resize(size, 0);
    if let Some(last) = number.last_mut() {
        *last |= sign;
    }
    Ok(number)
}

/// Script truth: any non-zero byte, except a lone sign bit in the last byte
/// (negative zero).
fn cast_to_bool(value: &[u8]) -> bool {
    match value.split_last() {
        None => false,
        Some((last, rest)) => rest.iter().any(|byte| *byte != 0) || (*last & 0x7f) != 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_encode_minimally_and_round_trip() {
        assert_eq!(encode_number(0), Vec::<u8>::new());
        assert_eq!(encode_number(127), vec![0x7f]);
        assert_eq!(encode_number(128), vec![0x80, 0x00]);
        assert_eq!(encode_number(-1), vec![0x81]);
        assert_eq!(
            encode_number(0xffff_ffff),
            vec![0xff, 0xff, 0xff, 0xff, 0x00]
        );
        assert_eq!(decode_number(&[0x80, 0x00]), Ok(128));
        assert_eq!(decode_number(&[0x81]), Ok(-1));
        assert!(decode_number(&[0x01, 0x00]).is_err());
    }

    #[test]
    fn num_to_bin_pads_with_the_sign_last() {
        assert_eq!(num_to_bin(&[], 3), Ok(vec![0, 0, 0]));
        assert_eq!(num_to_bin(&[0x05], 2), Ok(vec![0x05, 0x00]));
        assert_eq!(num_to_bin(&[0x85], 2), Ok(vec![0x05, 0x80]));
        assert!(num_to_bin(&[0x01, 0x02], 1).is_err());
    }

    #[test]
    fn minimal_pushes_follow_bchn() {
        assert!(is_minimal_push(OP_0, &[]));
        assert!(!is_minimal_push(0x01, &[0x05]));
        assert!(is_minimal_push(0x01, &[0x20]));
        assert!(!is_minimal_push(OP_PUSHDATA1, &[0u8; 20]));
        assert!(is_minimal_push(OP_PUSHDATA2, &[0u8; 300]));
    }
}
