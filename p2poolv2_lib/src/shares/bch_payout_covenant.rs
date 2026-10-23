// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The covenant that locks one node of a Bitcoin Cash payout tree.
//!
//! A payout tree pays the coinbase payees that do not fit the coinbase
//! (`docs/architecture/bch-hybrid-payouts.md`, section 5). Each node is a
//! P2SH32 output whose redeem script commits to the exact outputs the node
//! transaction must create. Nobody holds a key: anyone may spend a node, but
//! only with the one transaction the share chain decided.
//!
//! ```text
//! unlocking bytecode (push-only): <Z zero bytes> <redeem script>
//!
//! redeem script, with this node's value V, output count k, filler length Z
//! and allowed output hashes H written in:
//! OP_0 <Z> OP_NUM2BIN OP_EQUALVERIFY                       // the filler is exactly Z zeros
//! OP_TXVERSION OP_2 OP_NUMEQUALVERIFY                      // version 2
//! OP_TXLOCKTIME OP_0 OP_NUMEQUALVERIFY                     // lock time 0
//! OP_TXINPUTCOUNT OP_1 OP_NUMEQUALVERIFY                   // one input, no outside money
//! OP_0 OP_INPUTSEQUENCENUMBER <0xffffffff> OP_NUMEQUALVERIFY  // final sequence
//! OP_0 OP_UTXOVALUE OP_8 OP_NUM2BIN <V, 8 bytes> OP_EQUALVERIFY  // this node holds exactly V
//! OP_TXOUTPUTCOUNT <k> OP_NUMEQUALVERIFY                   // exactly k outputs
//! OP_0 OP_0                                                // acc = empty, i = 0
//! OP_BEGIN
//!   OP_DUP OP_OUTPUTTOKENCATEGORY OP_0 OP_EQUALVERIFY      // output i carries no token
//!   OP_DUP OP_OUTPUTVALUE OP_8 OP_NUM2BIN                  // its value, 8 bytes
//!   OP_OVER OP_OUTPUTBYTECODE OP_SIZE OP_SWAP OP_CAT       // size || script
//!   OP_CAT
//!   OP_ROT OP_SWAP OP_CAT OP_SHA256                        // acc = SHA256(acc || output i)
//!   OP_SWAP OP_1ADD
//!   OP_DUP <k> OP_NUMEQUAL
//! OP_UNTIL
//! OP_DROP
//! <H> OP_EQUAL                                             // one allowed hash, or for the root:
//! OP_DUP <H1> OP_EQUAL OP_OVER <H2> OP_EQUAL OP_BOOLOR OP_SWAP <H3> OP_EQUAL OP_BOOLOR
//! ```
//!
//! What each check stops:
//!
//! - The output hash and count: any changed payee, amount or script, and any
//!   extra output for whoever broadcasts the transaction.
//! - One input: an executor adding its own inputs and outputs.
//! - `V`: spending a look-alike output sent to the same address with another
//!   value.
//! - No token on an output: a node that spends output 0 of its parent is a
//!   token genesis input, so an executor could otherwise mint a token onto a
//!   payee's output.
//! - Version, lock time, sequence and the filler: every other degree of
//!   freedom of the transaction. Bitcoin Cash requires push-only unlocking
//!   bytecode with minimal pushes (consensus since 2018 and 2019), and the
//!   P2SH32 hash fixes the redeem script, so once the filler is fixed the
//!   whole transaction is fixed and every txid in a tree is known as soon as
//!   the block is found.
//!
//! The root of a tree accepts three output hashes, one per fee rate
//! (1, 2 and 3 satoshis per byte); every other node accepts one. The filler
//! only buys VM budget: the 2025 VM limits grant each input operation cost
//! and hash iterations in proportion to its unlocking bytecode length.
//!
//! Uses introspection (2022), P2SH32 and token introspection (2023), the VM
//! limits (2025) and loops (2026). No Bitcoin Cash consensus change.

use crate::shares::bch_htlc::{encode_script_number, p2sh32_script};
use bitcoin::hashes::{Hash, HashEngine, sha256, sha256d};
use bitcoin::{ScriptBuf, TxOut};

/// Version every node transaction must have.
pub const NODE_TRANSACTION_VERSION: i32 = 2;

/// Lock time every node transaction must have.
pub const NODE_LOCK_TIME: u32 = 0;

/// Sequence of the one input of every node transaction.
pub const NODE_SEQUENCE: u32 = 0xffff_ffff;

/// Filler bytes per node output in the unlocking bytecode.
///
/// The VM limits grant `(41 + unlocking length) * 800` operation cost and
/// `(41 + unlocking length) / 2` hash iterations (relay limit) per input. One
/// pass of the loop costs about 3,040 operation cost and two hash iterations
/// (measured with `test_utils::bch_vm`), which 3.8 and 4 bytes of unlocking
/// bytecode buy. Eight bytes leave a margin of two.
pub const FILLER_BYTES_PER_OUTPUT: usize = 8;

/// Version of this covenant's layout, carried in the protocol tag. Any
/// change to the redeem script is another version: every node must build the
/// same tree to agree on a coinbase.
pub const NODE_COVENANT_VERSION: u8 = 1;

const OP_0: u8 = 0x00;
const OP_PUSHDATA1: u8 = 0x4c;
const OP_PUSHDATA2: u8 = 0x4d;
const OP_1: u8 = 0x51;
const OP_2: u8 = 0x52;
const OP_8: u8 = 0x58;
const OP_BEGIN: u8 = 0x65;
const OP_UNTIL: u8 = 0x66;
const OP_DROP: u8 = 0x75;
const OP_DUP: u8 = 0x76;
const OP_OVER: u8 = 0x78;
const OP_ROT: u8 = 0x7b;
const OP_SWAP: u8 = 0x7c;
const OP_CAT: u8 = 0x7e;
const OP_NUM2BIN: u8 = 0x80;
const OP_SIZE: u8 = 0x82;
const OP_EQUAL: u8 = 0x87;
const OP_EQUALVERIFY: u8 = 0x88;
const OP_1ADD: u8 = 0x8b;
const OP_BOOLOR: u8 = 0x9b;
const OP_NUMEQUAL: u8 = 0x9c;
const OP_NUMEQUALVERIFY: u8 = 0x9d;
const OP_SHA256: u8 = 0xa8;
const OP_TXVERSION: u8 = 0xc2;
const OP_TXINPUTCOUNT: u8 = 0xc3;
const OP_TXOUTPUTCOUNT: u8 = 0xc4;
const OP_TXLOCKTIME: u8 = 0xc5;
const OP_UTXOVALUE: u8 = 0xc6;
const OP_INPUTSEQUENCENUMBER: u8 = 0xcb;
const OP_OUTPUTVALUE: u8 = 0xcc;
const OP_OUTPUTBYTECODE: u8 = 0xcd;
const OP_OUTPUTTOKENCATEGORY: u8 = 0xd1;

/// Largest data a single length opcode pushes.
const MAX_DIRECT_PUSH: usize = 0x4b;

/// The covenant of one payout tree node: what the node holds and which
/// outputs may spend it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeCovenant {
    value: u64,
    output_count: usize,
    output_hashes: Vec<[u8; 32]>,
}

impl NodeCovenant {
    /// A node holding `value` satoshis that pays `output_count` outputs whose
    /// [`outputs_hash`] is one of `output_hashes`.
    ///
    /// # Panics
    ///
    /// If `output_count` is zero or `output_hashes` is empty: a node always
    /// pays something, and a covenant accepting nothing is a bug.
    pub fn new(value: u64, output_count: usize, output_hashes: Vec<[u8; 32]>) -> Self {
        assert!(
            output_count > 0,
            "a payout tree node pays at least one output"
        );
        assert!(
            !output_hashes.is_empty(),
            "a payout tree node accepts at least one output set"
        );
        Self {
            value,
            output_count,
            output_hashes,
        }
    }

    /// Satoshis the node holds.
    pub fn value(&self) -> u64 {
        self.value
    }

    /// Outputs the node transaction pays.
    pub fn output_count(&self) -> usize {
        self.output_count
    }

    /// The output hashes the covenant accepts.
    pub fn output_hashes(&self) -> &[[u8; 32]] {
        &self.output_hashes
    }

    /// Zero bytes the unlocking bytecode carries before the redeem script.
    pub fn filler_length(&self) -> usize {
        FILLER_BYTES_PER_OUTPUT * self.output_count
    }

    /// The redeem script (see the module documentation).
    pub fn redeem_script(&self) -> Vec<u8> {
        let mut script = Vec::with_capacity(48 + 35 * self.output_hashes.len());
        script.push(OP_0);
        push_number(&mut script, self.filler_length());
        script.extend_from_slice(&[OP_NUM2BIN, OP_EQUALVERIFY]);
        script.extend_from_slice(&[OP_TXVERSION, OP_2, OP_NUMEQUALVERIFY]);
        script.extend_from_slice(&[OP_TXLOCKTIME, OP_0, OP_NUMEQUALVERIFY]);
        script.extend_from_slice(&[OP_TXINPUTCOUNT, OP_1, OP_NUMEQUALVERIFY]);
        script.extend_from_slice(&[OP_0, OP_INPUTSEQUENCENUMBER]);
        push_data(&mut script, &encode_script_number(NODE_SEQUENCE));
        script.push(OP_NUMEQUALVERIFY);
        script.extend_from_slice(&[OP_0, OP_UTXOVALUE, OP_8, OP_NUM2BIN]);
        push_data(&mut script, &self.value.to_le_bytes());
        script.push(OP_EQUALVERIFY);
        script.push(OP_TXOUTPUTCOUNT);
        push_number(&mut script, self.output_count);
        script.push(OP_NUMEQUALVERIFY);
        script.extend_from_slice(&[OP_0, OP_0, OP_BEGIN]);
        script.extend_from_slice(&[OP_DUP, OP_OUTPUTTOKENCATEGORY, OP_0, OP_EQUALVERIFY]);
        script.extend_from_slice(&[OP_DUP, OP_OUTPUTVALUE, OP_8, OP_NUM2BIN]);
        script.extend_from_slice(&[OP_OVER, OP_OUTPUTBYTECODE, OP_SIZE, OP_SWAP, OP_CAT]);
        script.push(OP_CAT);
        script.extend_from_slice(&[OP_ROT, OP_SWAP, OP_CAT, OP_SHA256]);
        script.extend_from_slice(&[OP_SWAP, OP_1ADD]);
        script.push(OP_DUP);
        push_number(&mut script, self.output_count);
        script.extend_from_slice(&[OP_NUMEQUAL, OP_UNTIL, OP_DROP]);
        self.push_hash_check(&mut script);
        script
    }

    /// Compare the accumulated hash on the stack with the allowed hashes,
    /// leaving one boolean.
    fn push_hash_check(&self, script: &mut Vec<u8>) {
        let (last, earlier) = self
            .output_hashes
            .split_last()
            .expect("NodeCovenant::new keeps at least one hash");
        if earlier.is_empty() {
            push_data(script, last);
            script.push(OP_EQUAL);
            return;
        }
        // Stack: acc. Each earlier hash: compare a copy, folding the
        // results into one boolean kept under acc; the last one consumes acc.
        let (first, middle) = earlier
            .split_first()
            .expect("earlier holds at least one hash");
        script.push(OP_DUP);
        push_data(script, first);
        script.push(OP_EQUAL);
        for hash in middle {
            script.push(OP_OVER);
            push_data(script, hash);
            script.extend_from_slice(&[OP_EQUAL, OP_BOOLOR]);
        }
        script.push(OP_SWAP);
        push_data(script, last);
        script.extend_from_slice(&[OP_EQUAL, OP_BOOLOR]);
    }

    /// HASH256 of the redeem script, the hash the P2SH32 output commits to.
    pub fn script_hash(&self) -> [u8; 32] {
        sha256d::Hash::hash(&self.redeem_script()).to_byte_array()
    }

    /// The P2SH32 locking script that holds the node.
    pub fn locking_script(&self) -> ScriptBuf {
        p2sh32_script(&self.script_hash())
    }

    /// The output that holds the node in its parent transaction.
    pub fn output(&self) -> TxOut {
        TxOut {
            value: bitcoin::Amount::from_sat(self.value),
            script_pubkey: self.locking_script(),
        }
    }

    /// The unlocking bytecode that spends the node: the filler, then the
    /// redeem script, each pushed minimally.
    pub fn unlocking_script(&self) -> ScriptBuf {
        let redeem_script = self.redeem_script();
        let filler = vec![0u8; self.filler_length()];
        let mut script = Vec::with_capacity(filler.len() + redeem_script.len() + 6);
        push_data(&mut script, &filler);
        push_data(&mut script, &redeem_script);
        ScriptBuf::from_bytes(script)
    }
}

/// The hash a node covenant accumulates over its transaction's outputs.
///
/// `acc_0` is empty and `acc_(i+1) = SHA256(acc_i || value_i || size_i ||
/// script_i)`, where `value_i` is the 8-byte little-endian value and `size_i`
/// is the script length as `OP_SIZE` pushes it: a minimal script number, one
/// byte for every script a payout can have (at most 35 bytes). An empty
/// output list hashes to 32 zero bytes, which no covenant accepts: a node
/// always pays at least one output.
pub fn outputs_hash(outputs: &[TxOut]) -> [u8; 32] {
    let mut accumulator: Vec<u8> = Vec::new();
    for output in outputs {
        let script = output.script_pubkey.as_bytes();
        let length = u32::try_from(script.len()).expect("a script is far below 4 GB");
        let mut engine = sha256::Hash::engine();
        engine.input(&accumulator);
        engine.input(&output.value.to_sat().to_le_bytes());
        engine.input(&encode_script_number(length));
        engine.input(script);
        accumulator = sha256::Hash::from_engine(engine).to_byte_array().to_vec();
    }
    let mut hash = [0u8; 32];
    if accumulator.len() == 32 {
        hash.copy_from_slice(&accumulator);
    }
    hash
}

/// Append a minimal push of a non-negative number: `OP_0`, `OP_1` to
/// `OP_16`, or its script number bytes.
fn push_number(script: &mut Vec<u8>, value: usize) {
    match value {
        0 => script.push(OP_0),
        1..=16 => script.push(OP_1 + (value as u8) - 1),
        _ => {
            let value = u32::try_from(value).expect("covenant numbers are small");
            push_data(script, &encode_script_number(value));
        }
    }
}

/// Append a minimal push of `data`, at most 65,535 bytes and never a single
/// byte from 1 to 16 (which minimal pushes write as `OP_1` to `OP_16`).
fn push_data(script: &mut Vec<u8>, data: &[u8]) {
    let length = data.len();
    debug_assert!(length != 1 || !(1..=16).contains(&data[0]));
    if length == 0 {
        script.push(OP_0);
    } else if length <= MAX_DIRECT_PUSH {
        script.push(length as u8);
    } else if length <= usize::from(u8::MAX) {
        script.extend_from_slice(&[OP_PUSHDATA1, length as u8]);
    } else {
        let length = u16::try_from(length).expect("covenant pushes stay below 65,536 bytes");
        script.push(OP_PUSHDATA2);
        script.extend_from_slice(&length.to_le_bytes());
    }
    script.extend_from_slice(data);
}

#[cfg(test)]
mod tests;
