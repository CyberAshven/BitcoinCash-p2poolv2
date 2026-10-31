// SPDX-License-Identifier: AGPL-3.0-or-later

//! Deterministic CashAssembly for the isolated PHOTON payout prototype.
//!
//! The small assembler tracks named stack slots while emitting ordinary BCH
//! operations. It does not execute covenant policy on the host. The generated
//! annotated assembly is checked against the committed artifact by the tests.

use std::collections::BTreeMap;
use std::fmt::Write;
use std::sync::OnceLock;

use libauth::{
    BytecodeGenerationResult, CompilationData, CompilerConfiguration, create_compiler_bch,
    create_virtual_machine_bch_2026, instantiate_sha256,
};

const MONEY_MAX: i128 = 2_100_000_000_000_000;

#[derive(Clone)]
pub(crate) struct Assembly {
    pub(crate) text: String,
    pub(crate) stack: Vec<String>,
}

impl Assembly {
    pub(crate) fn new(inputs: &[&str]) -> Self {
        Self {
            text: String::new(),
            stack: inputs.iter().map(|name| (*name).to_owned()).collect(),
        }
    }

    pub(crate) fn line(&mut self, text: &str) {
        writeln!(self.text, "{text}").expect("writing to a string");
    }

    pub(crate) fn comment(&mut self, text: &str) {
        self.line(&format!("// {text}"));
    }

    pub(crate) fn raw(&mut self, code: &str, consumed: usize, outputs: &[&str]) {
        assert!(self.stack.len() >= consumed, "stack underflow: {code}");
        self.stack.truncate(self.stack.len() - consumed);
        self.stack
            .extend(outputs.iter().map(|name| (*name).to_owned()));
        self.line(code);
    }

    pub(crate) fn integer(&mut self, value: i128, name: &str) {
        self.raw(&format!("<{value}>"), 0, &[name]);
    }

    pub(crate) fn bytes(&mut self, value: &str, name: &str) {
        self.raw(&format!("<0x{value}>"), 0, &[name]);
    }

    pub(crate) fn copy(&mut self, name: &str) {
        let position = self
            .stack
            .iter()
            .rposition(|slot| slot == name)
            .unwrap_or_else(|| panic!("unknown stack slot {name}: {:?}", self.stack));
        let depth = self.stack.len() - 1 - position;
        self.raw(&format!("<{depth}> OP_PICK // copy {name}"), 0, &[name]);
    }

    pub(crate) fn take(&mut self, name: &str) {
        let position = self
            .stack
            .iter()
            .rposition(|slot| slot == name)
            .unwrap_or_else(|| panic!("unknown stack slot {name}: {:?}", self.stack));
        let depth = self.stack.len() - 1 - position;
        self.stack.remove(position);
        self.stack.push(name.to_owned());
        self.line(&format!("<{depth}> OP_ROLL // take {name}"));
    }

    pub(crate) fn remove(&mut self, name: &str) {
        self.take(name);
        self.raw("OP_DROP", 1, &[]);
    }

    pub(crate) fn assign(&mut self, name: &str) {
        assert_ne!(self.stack.last().map(String::as_str), Some(name));
        let position = self
            .stack
            .iter()
            .rposition(|slot| slot == name)
            .expect("existing assignment slot");
        let following = self.stack[position + 1..self.stack.len() - 1].to_vec();
        self.remove(name);
        *self.stack.last_mut().expect("assignment value") = name.to_owned();
        for slot in following {
            self.take(&slot);
        }
    }

    pub(crate) fn unsigned(&mut self, name: &str) {
        self.raw("<0x00> OP_CAT OP_BIN2NUM", 1, &[name]);
    }

    pub(crate) fn slice(&mut self, source: &str, start: i128, length: i128, name: &str) {
        self.copy(source);
        self.raw(
            &format!("<{start}> OP_SPLIT OP_NIP <{length}> OP_SPLIT OP_DROP"),
            1,
            &[name],
        );
    }

    pub(crate) fn field(&mut self, source: &str, start: i128, length: i128, name: &str) {
        self.slice(source, start, length, name);
        self.unsigned(name);
    }

    pub(crate) fn compare_integer(&mut self, name: &str, value: i128, operation: &str) {
        self.copy(name);
        self.integer(value, "constant");
        self.raw(&format!("{operation} OP_VERIFY"), 2, &[]);
    }

    pub(crate) fn equal(&mut self, left: &str, right: &str) {
        self.copy(left);
        self.copy(right);
        self.raw("OP_EQUALVERIFY", 2, &[]);
    }

    pub(crate) fn clean_return(&mut self, names: &[&str]) {
        for name in names.iter().rev() {
            self.copy(name);
            self.raw("OP_TOALTSTACK", 1, &[]);
        }
        let count = self.stack.len();
        for _ in 0..count {
            self.raw("OP_DROP", 1, &[]);
        }
        for name in names {
            self.raw("OP_FROMALTSTACK", 0, &[name]);
        }
        self.stack = names.iter().map(|name| (*name).to_owned()).collect();
    }

    pub(crate) fn function(&mut self, identifier: i128, body: Assembly) {
        self.line("<");
        self.line(&body.text);
        self.line(&format!("> <{identifier}> OP_DEFINE"));
    }

    pub(crate) fn invoke(&mut self, identifier: i128, count: usize, names: &[&str]) {
        self.raw(&format!("<{identifier}> OP_INVOKE"), count, names);
    }

    pub(crate) fn branch(&mut self, yes: impl FnOnce(&mut Self), no: impl FnOnce(&mut Self)) {
        self.raw("OP_IF", 1, &[]);
        let saved = self.stack.clone();
        yes(self);
        let expected = self.stack.clone();
        self.line("OP_ELSE");
        self.stack = saved;
        no(self);
        assert_eq!(self.stack, expected, "branch stack mismatch");
        self.line("OP_ENDIF");
    }
}

fn descriptor_function() -> Assembly {
    let mut code = Assembly::new(&["descriptors", "count"]);
    code.comment(
        "Function 1: recursively hash a deterministic midpoint tree of 98-byte descriptors.",
    );
    code.copy("count");
    code.integer(1, "one");
    code.raw("OP_NUMEQUAL", 2, &["leaf"]);
    code.branch(
        |code| {
            code.copy("descriptors");
            code.raw("OP_SIZE OP_NIP", 1, &["length"]);
            code.compare_integer("length", 98, "OP_NUMEQUAL");
            code.remove("length");
            code.remove("count");
        },
        |code| {
            code.copy("count");
            code.integer(2, "two");
            code.raw("OP_DIV", 2, &["left_count"]);
            code.copy("descriptors");
            code.copy("left_count");
            code.raw("<98> OP_MUL OP_SPLIT", 2, &["left_bytes", "right_bytes"]);
            code.copy("left_bytes");
            code.copy("left_count");
            code.invoke(1, 2, &["left"]);
            code.copy("right_bytes");
            code.copy("count");
            code.copy("left_count");
            code.raw("OP_SUB", 2, &["right_count"]);
            code.invoke(1, 2, &["right"]);
            combine_descriptors(code, "left", "right", "count", "combined");
            code.clean_return(&["combined"]);
            code.stack = vec!["descriptors".into()];
        },
    );
    code
}

fn combine_descriptors(code: &mut Assembly, left: &str, right: &str, count: &str, name: &str) {
    code.bytes("01", "domain");
    code.copy(left);
    code.copy(right);
    code.raw("OP_CAT OP_CAT OP_SHA256", 3, &["node_hash"]);
    code.field(left, 32, 64, "left_weight");
    code.field(right, 32, 64, "right_weight");
    code.raw(
        "OP_ADD <65> OP_NUM2BIN <64> OP_SPLIT OP_SWAP OP_DROP <0x00> OP_EQUALVERIFY",
        2,
        &[],
    );
    // The preceding conversion proves the sum fits unsigned 512 bits. Re-read
    // it for the canonical fixed-width descriptor (no host arithmetic).
    code.field(left, 32, 64, "left_weight");
    code.field(right, 32, 64, "right_weight");
    code.raw(
        "OP_ADD <65> OP_NUM2BIN <64> OP_SPLIT OP_DROP",
        2,
        &["weight_bytes"],
    );
    code.copy(count);
    code.raw("<2> OP_NUM2BIN", 1, &["count_bytes"]);
    code.raw("OP_CAT", 2, &["weight_count"]);
    code.take("node_hash");
    code.raw("OP_SWAP OP_CAT", 2, &[name]);
}

fn allocation(code: &mut Assembly, lower: &str, upper: &str, output: &str) {
    code.copy("reward");
    code.copy(upper);
    code.raw("OP_MUL", 2, &["product"]);
    code.copy("total");
    code.raw("OP_DIV", 2, &["upper_allocation"]);
    code.copy("reward");
    code.copy(lower);
    code.raw("OP_MUL", 2, &["product"]);
    code.copy("total");
    code.raw("OP_DIV OP_SUB", 3, &[output]);
}

fn payment_function() -> Assembly {
    let mut code = Assembly::new(&[
        "script", "amount", "minimum", "category", "index", "satoshis",
    ]);
    code.comment("Function 2: exact ordered positive allocation, or omit a zero allocation.");
    code.copy("amount");
    code.integer(0, "zero");
    code.raw("OP_GREATERTHAN", 2, &["positive"]);
    code.branch(
        |code| {
            code.copy("index");
            code.raw("OP_OUTPUTBYTECODE", 1, &["actual_script"]);
            code.equal("actual_script", "script");
            code.remove("actual_script");
            code.copy("index");
            code.raw("OP_OUTPUTTOKENCATEGORY", 1, &["actual_category"]);
            code.equal("actual_category", "category");
            code.remove("actual_category");
            code.copy("index");
            code.raw("OP_OUTPUTTOKENCOMMITMENT <0x> OP_EQUALVERIFY", 1, &[]);
            code.copy("index");
            code.raw("OP_OUTPUTTOKENAMOUNT", 1, &["actual_amount"]);
            code.equal("actual_amount", "amount");
            code.remove("actual_amount");
            code.copy("index");
            code.raw("OP_OUTPUTVALUE", 1, &["value"]);
            code.copy("value");
            code.copy("minimum");
            code.raw("OP_GREATERTHANOREQUAL OP_VERIFY", 2, &[]);
            code.compare_integer("value", MONEY_MAX, "OP_LESSTHANOREQUAL");
            code.copy("satoshis");
            code.take("value");
            code.raw("OP_ADD", 2, &["new_satoshis"]);
            code.assign("satoshis");
            code.compare_integer("satoshis", MONEY_MAX, "OP_LESSTHANOREQUAL");
            code.copy("index");
            code.raw("OP_1ADD", 1, &["new_index"]);
            code.assign("index");
        },
        |_| {},
    );
    code.clean_return(&["index", "satoshis"]);
    code
}

pub(crate) fn validate_script(code: &mut Assembly) {
    code.comment(
        "The only beneficiary scripts are exact standard P2PKH, P2SH20 or P2SH32 templates.",
    );
    code.copy("script");
    code.raw("OP_SIZE OP_NIP", 1, &["script_length"]);
    code.copy("script_length");
    code.integer(25, "length");
    code.raw("OP_NUMEQUAL", 2, &["p2pkh"]);
    code.branch(
        |code| {
            code.slice("script", 0, 3, "prefix");
            code.bytes("76a914", "expected");
            code.raw("OP_EQUALVERIFY", 2, &[]);
            code.slice("script", 23, 2, "suffix");
            code.bytes("88ac", "expected");
            code.raw("OP_EQUALVERIFY", 2, &[]);
        },
        |code| {
            code.copy("script_length");
            code.integer(23, "length");
            code.raw("OP_NUMEQUAL", 2, &["p2sh20"]);
            code.branch(
                |code| {
                    code.slice("script", 0, 2, "prefix");
                    code.bytes("a914", "expected");
                    code.raw("OP_EQUALVERIFY", 2, &[]);
                    code.slice("script", 22, 1, "suffix");
                    code.bytes("87", "expected");
                    code.raw("OP_EQUALVERIFY", 2, &[]);
                },
                |code| {
                    code.compare_integer("script_length", 35, "OP_NUMEQUAL");
                    code.slice("script", 0, 2, "prefix");
                    code.bytes("aa20", "expected");
                    code.raw("OP_EQUALVERIFY", 2, &[]);
                    code.slice("script", 34, 1, "suffix");
                    code.bytes("87", "expected");
                    code.raw("OP_EQUALVERIFY", 2, &[]);
                },
            );
        },
    );
}

fn make_child(code: &mut Assembly, descriptor: &str, lower: &str, upper: &str) {
    code.comment("Canonical child state retains network, category, context and program digest, and pins original reward.");
    code.bytes("010001", "header");
    code.copy("depth");
    code.raw("OP_1SUB <1> OP_NUM2BIN OP_CAT", 2, &["header"]);
    code.slice("state", 4, 100, "immutable");
    code.raw("OP_CAT", 2, &["child_state"]);
    code.slice(descriptor, 0, 32, "subtree");
    code.raw("OP_CAT", 2, &["child_state"]);
    code.slice("state", 136, 64, "total_bytes");
    code.raw("OP_CAT", 2, &["child_state"]);
    code.copy(lower);
    code.raw(
        "<65> OP_NUM2BIN <64> OP_SPLIT OP_DROP OP_CAT",
        2,
        &["child_state"],
    );
    code.copy(upper);
    code.raw(
        "<65> OP_NUM2BIN <64> OP_SPLIT OP_DROP OP_CAT",
        2,
        &["child_state"],
    );
    code.copy("reward");
    code.raw("<8> OP_NUM2BIN OP_CAT", 2, &["child_state"]);
    code.bytes("4d5001", "push_header");
    code.take("child_state");
    code.raw("OP_CAT", 2, &["child_redeem"]);
    code.copy("program");
    code.raw("OP_CAT OP_HASH256", 2, &["child_hash"]);
    code.bytes("aa20", "p2sh32_prefix");
    code.take("child_hash");
    code.raw("OP_CAT <0x87> OP_CAT", 2, &["child_script"]);
}

fn pay_current(code: &mut Assembly, script: &str, amount: &str, minimum: i128) {
    code.copy(script);
    code.copy(amount);
    code.integer(minimum, "minimum");
    code.copy("category");
    code.copy("index");
    code.copy("satoshis");
    code.invoke(2, 6, &["new_index", "new_satoshis"]);
    code.assign("satoshis");
    code.assign("index");
}

fn internal(code: &mut Assembly) {
    code.comment("Internal witness is two authenticated hash/weight/count descriptors; intervals and locks are never witness choices.");
    code.compare_integer("depth", 0, "OP_GREATERTHAN");
    code.copy("witness");
    code.raw("OP_SIZE OP_NIP <196> OP_NUMEQUALVERIFY", 1, &[]);
    code.slice("witness", 0, 98, "left");
    code.slice("witness", 98, 98, "right");
    code.field("left", 96, 2, "left_count");
    code.copy("count");
    code.raw("<2> OP_DIV", 1, &["expected_left"]);
    code.equal("left_count", "expected_left");
    code.remove("expected_left");
    code.field("right", 96, 2, "right_count");
    code.copy("left_count");
    code.copy("right_count");
    code.raw("OP_ADD", 2, &["sum_count"]);
    code.equal("sum_count", "count");
    code.remove("sum_count");
    code.field("left", 32, 64, "left_weight");
    code.field("right", 32, 64, "right_weight");
    code.compare_integer("left_weight", 0, "OP_GREATERTHAN");
    code.compare_integer("right_weight", 0, "OP_GREATERTHAN");
    code.copy("lower");
    code.copy("left_weight");
    code.raw("OP_ADD", 2, &["middle"]);
    code.copy("middle");
    code.copy("right_weight");
    code.raw("OP_ADD", 2, &["sum_upper"]);
    code.equal("sum_upper", "upper");
    code.remove("sum_upper");
    combine_descriptors(code, "left", "right", "count", "node");
    code.slice("node", 0, 32, "hash");
    code.equal("hash", "subtree");
    code.remove("hash");
    make_child(code, "left", "lower", "middle");
    allocation(code, "lower", "middle", "amount");
    pay_current(code, "child_script", "amount", 705);
    code.remove("child_script");
    code.remove("amount");
    make_child(code, "right", "middle", "upper");
    allocation(code, "middle", "upper", "amount");
    pay_current(code, "child_script", "amount", 705);
    code.remove("child_script");
    code.remove("amount");
    for name in [
        "left",
        "right",
        "left_count",
        "right_count",
        "left_weight",
        "right_weight",
        "middle",
        "node",
    ] {
        code.remove(name);
    }
}

fn terminal(code: &mut Assembly) {
    code.comment("Terminal buckets reveal one to eight positive leaves; hash the same midpoint tree, including all zero allocations.");
    code.bytes("", "descriptors");
    code.integer(0, "leaf_index");
    code.integer(0, "previous_script");
    code.copy("lower");
    code.raw("OP_DUP OP_DROP", 1, &["cursor"]);
    code.line("OP_BEGIN");
    let loop_stack = code.stack.clone();
    code.take("witness");
    code.raw(
        "<1> OP_SPLIT OP_SWAP <0x00> OP_CAT OP_BIN2NUM OP_SPLIT",
        1,
        &["script", "tail"],
    );
    code.take("tail");
    code.raw("<64> OP_SPLIT", 1, &["weight_bytes", "new_witness"]);
    *code.stack.last_mut().expect("remaining witness") = "witness".into();
    validate_script(code);
    code.copy("script");
    code.raw(
        "OP_REVERSEBYTES <0x00> OP_CAT OP_BIN2NUM",
        1,
        &["script_number"],
    );
    // Equal-length scripts compare lexically after reversal. Different standard
    // templates have distinct first bytes, so right-pad to 35 bytes first.
    code.copy("script_length");
    code.raw(
        "<35> OP_SWAP OP_SUB <8> OP_MUL OP_LSHIFTNUM",
        2,
        &["sort_key"],
    );
    code.copy("sort_key");
    code.copy("previous_script");
    code.raw("OP_GREATERTHAN OP_VERIFY", 2, &[]);
    code.take("sort_key");
    code.assign("previous_script");
    code.copy("weight_bytes");
    code.unsigned("weight");
    code.compare_integer("weight", 0, "OP_GREATERTHAN");
    code.copy("cursor");
    code.copy("weight");
    code.raw("OP_ADD", 2, &["next_cursor"]);
    code.copy("next_cursor");
    code.copy("upper");
    code.raw("OP_LESSTHANOREQUAL OP_VERIFY", 2, &[]);
    allocation(code, "cursor", "next_cursor", "amount");
    code.copy("script");
    code.copy("amount");
    code.copy("script_length");
    code.raw("<200> OP_ADD <3> OP_MUL", 1, &["minimum"]);
    code.copy("category");
    code.copy("index");
    code.copy("satoshis");
    code.invoke(2, 6, &["new_index", "new_satoshis"]);
    code.assign("satoshis");
    code.assign("index");
    code.bytes("00", "leaf_domain");
    code.copy("script_length");
    code.raw("<1> OP_NUM2BIN OP_CAT", 2, &["leaf_preimage"]);
    code.copy("script");
    code.raw("OP_CAT", 2, &["leaf_preimage"]);
    code.copy("weight_bytes");
    code.raw("OP_CAT OP_SHA256", 2, &["leaf_hash"]);
    code.copy("weight_bytes");
    code.raw("OP_CAT <0x0100> OP_CAT", 2, &["descriptor"]);
    code.copy("descriptors");
    code.take("descriptor");
    code.raw("OP_CAT", 2, &["new_descriptors"]);
    code.assign("descriptors");
    code.take("next_cursor");
    code.assign("cursor");
    for name in [
        "script",
        "weight_bytes",
        "script_length",
        "weight",
        "amount",
    ] {
        code.remove(name);
    }
    code.copy("leaf_index");
    code.raw("OP_1ADD", 1, &["new_leaf_index"]);
    code.assign("leaf_index");
    // Restore the declared slot order before the next loop iteration.
    for name in &loop_stack {
        code.take(name);
    }
    assert_eq!(code.stack, loop_stack);
    code.copy("leaf_index");
    code.copy("count");
    code.raw("OP_NUMEQUAL OP_UNTIL", 2, &[]);
    code.copy("witness");
    code.bytes("", "empty");
    code.raw("OP_EQUALVERIFY", 2, &[]);
    code.equal("cursor", "upper");
    code.copy("descriptors");
    code.copy("count");
    code.invoke(1, 2, &["root_descriptor"]);
    code.slice("root_descriptor", 0, 32, "hash");
    code.equal("hash", "subtree");
    for name in [
        "descriptors",
        "leaf_index",
        "previous_script",
        "cursor",
        "root_descriptor",
        "hash",
    ] {
        code.remove(name);
    }
}

/// Produce readable, annotated CashAssembly with all stack indexes resolved.
pub fn generated_source() -> String {
    let mut code = Assembly::new(&["witness", "state"]);
    code.comment("PHOTON phase 2a prototype v1. Witness: count:u16 then descriptors or leaves.");
    code.comment("Read active redeem bytecode HERE, outside every function invocation.");
    code.raw("OP_ACTIVEBYTECODE", 0, &["redeem"]);
    code.copy("state");
    code.raw("OP_SIZE OP_NIP <336> OP_NUMEQUALVERIFY", 1, &[]);
    code.slice("redeem", 0, 3, "prefix");
    code.bytes("4d5001", "canonical_prefix");
    code.raw("OP_EQUALVERIFY", 2, &[]);
    code.slice("redeem", 3, 336, "embedded_state");
    code.equal("embedded_state", "state");
    code.remove("embedded_state");
    code.copy("redeem");
    code.raw("<339> OP_SPLIT OP_NIP", 1, &["program"]);
    code.copy("program");
    code.raw("OP_SIZE OP_NIP <4096> OP_LESSTHANOREQUAL OP_VERIFY", 1, &[]);
    code.copy("program");
    code.raw("OP_SHA256", 1, &["program_digest"]);
    code.slice("state", 72, 32, "pinned_digest");
    code.equal("program_digest", "pinned_digest");
    code.remove("program_digest");
    code.remove("pinned_digest");
    code.field("state", 0, 2, "version");
    code.compare_integer("version", 1, "OP_NUMEQUAL");
    code.remove("version");
    code.field("state", 2, 1, "mode");
    code.compare_integer("mode", 2, "OP_LESSTHAN");
    code.field("state", 3, 1, "depth");
    code.compare_integer("depth", 9, "OP_LESSTHANOREQUAL");
    code.field("state", 4, 4, "network");
    code.compare_integer("network", 4, "OP_LESSTHANOREQUAL");
    code.remove("network");
    code.slice("state", 8, 32, "category");
    code.slice("state", 104, 32, "subtree");
    code.field("state", 136, 64, "total");
    code.compare_integer("total", 0, "OP_GREATERTHAN");
    code.field("state", 200, 64, "lower");
    code.field("state", 264, 64, "upper");
    code.copy("lower");
    code.copy("upper");
    code.raw("OP_LESSTHAN OP_VERIFY", 2, &[]);
    code.copy("upper");
    code.copy("total");
    code.raw("OP_LESSTHANOREQUAL OP_VERIFY", 2, &[]);
    code.field("state", 328, 8, "reward");
    code.raw("OP_INPUTINDEX <0> OP_NUMEQUALVERIFY OP_TXVERSION <2> OP_NUMEQUALVERIFY OP_TXLOCKTIME <0> OP_NUMEQUALVERIFY", 0, &[]);
    code.raw("<0> OP_UTXOTOKENCATEGORY", 0, &["actual_category"]);
    code.equal("actual_category", "category");
    code.remove("actual_category");
    code.raw("<0> OP_UTXOTOKENCOMMITMENT <0x> OP_EQUALVERIFY", 0, &[]);
    code.copy("mode");
    code.branch(
        |code| {
            code.compare_integer("reward", 0, "OP_GREATERTHAN");
            code.raw("<0> OP_UTXOBYTECODE", 0, &["active_lock"]);
            code.copy("redeem");
            code.raw(
                "OP_HASH256 <0xaa20> OP_SWAP OP_CAT <0x87> OP_CAT",
                1,
                &["expected_lock"],
            );
            code.equal("active_lock", "expected_lock");
            code.remove("active_lock");
            code.remove("expected_lock");
        },
        |code| {
            code.compare_integer("reward", 0, "OP_NUMEQUAL");
            code.compare_integer("lower", 0, "OP_NUMEQUAL");
            code.equal("upper", "total");
            code.raw("<0> OP_UTXOTOKENAMOUNT", 0, &["actual_reward"]);
            code.assign("reward");
            code.raw("<0> OP_UTXOBYTECODE", 0, &["active_lock"]);
            code.copy("active_lock");
            code.raw("OP_SIZE OP_NIP <23> OP_NUMEQUAL", 1, &["p2sh20"]);
            code.branch(
                |code| {
                    code.copy("redeem");
                    code.raw(
                        "OP_HASH160 <0xa914> OP_SWAP OP_CAT <0x87> OP_CAT",
                        1,
                        &["expected_lock"],
                    );
                },
                |code| {
                    code.copy("redeem");
                    code.raw(
                        "OP_HASH256 <0xaa20> OP_SWAP OP_CAT <0x87> OP_CAT",
                        1,
                        &["expected_lock"],
                    );
                },
            );
            code.equal("active_lock", "expected_lock");
            code.remove("active_lock");
            code.remove("expected_lock");
        },
    );
    code.compare_integer("reward", 0, "OP_GREATERTHAN");
    code.compare_integer("reward", i64::MAX as i128, "OP_LESSTHANOREQUAL");
    allocation(&mut code, "lower", "upper", "expected_amount");
    code.raw("<0> OP_UTXOTOKENAMOUNT", 0, &["active_amount"]);
    code.equal("active_amount", "expected_amount");
    code.remove("active_amount");
    code.remove("expected_amount");
    code.comment("Exactly one distribution input and 1..4 token-free funding inputs; all sequences final; MoneyRange checked.");
    code.raw("OP_TXINPUTCOUNT", 0, &["input_count"]);
    code.compare_integer("input_count", 2, "OP_GREATERTHANOREQUAL");
    code.compare_integer("input_count", 5, "OP_LESSTHANOREQUAL");
    code.integer(0, "input_index");
    code.integer(0, "input_satoshis");
    code.line("OP_BEGIN");
    let loop_stack = code.stack.clone();
    code.copy("input_index");
    code.raw(
        "OP_INPUTSEQUENCENUMBER <4294967295> OP_NUMEQUALVERIFY",
        1,
        &[],
    );
    code.copy("input_index");
    code.branch(
        |code| {
            code.copy("input_index");
            code.raw("OP_UTXOTOKENCATEGORY <0x> OP_EQUALVERIFY", 1, &[]);
        },
        |_| {},
    );
    code.copy("input_index");
    code.raw(
        "OP_INPUTBYTECODE OP_SIZE OP_NIP <10000> OP_LESSTHANOREQUAL OP_VERIFY",
        1,
        &[],
    );
    code.copy("input_index");
    code.raw("OP_UTXOVALUE", 1, &["input_value"]);
    code.compare_integer("input_value", 0, "OP_GREATERTHANOREQUAL");
    code.compare_integer("input_value", MONEY_MAX, "OP_LESSTHANOREQUAL");
    code.copy("input_satoshis");
    code.take("input_value");
    code.raw("OP_ADD", 2, &["new_input_satoshis"]);
    code.assign("input_satoshis");
    code.compare_integer("input_satoshis", MONEY_MAX, "OP_LESSTHANOREQUAL");
    code.copy("input_index");
    code.raw("OP_1ADD", 1, &["new_input_index"]);
    code.assign("input_index");
    for name in &loop_stack {
        code.take(name);
    }
    code.copy("input_index");
    code.copy("input_count");
    code.raw("OP_NUMEQUAL OP_UNTIL", 2, &[]);
    code.remove("input_index");
    code.remove("input_count");
    code.remove("input_satoshis");
    code.function(1, descriptor_function());
    code.function(2, payment_function());
    code.take("witness");
    code.raw(
        "<2> OP_SPLIT OP_SWAP <0x00> OP_CAT OP_BIN2NUM",
        1,
        &["witness", "count"],
    );
    code.compare_integer("count", 1, "OP_GREATERTHANOREQUAL");
    code.compare_integer("count", 4096, "OP_LESSTHANOREQUAL");
    code.copy("count");
    code.integer(8, "capacity");
    code.copy("depth");
    code.raw("OP_LSHIFTNUM OP_LESSTHANOREQUAL OP_VERIFY", 3, &[]);
    code.copy("mode");
    code.raw("OP_NOT", 1, &["root"]);
    code.branch(
        |code| {
            code.copy("depth");
            code.branch(
                |code| {
                    code.copy("count");
                    code.integer(8, "capacity");
                    code.copy("depth");
                    code.raw("OP_1SUB OP_LSHIFTNUM OP_GREATERTHAN OP_VERIFY", 3, &[]);
                },
                |_| {},
            );
        },
        |_| {},
    );
    code.integer(0, "index");
    code.integer(0, "satoshis");
    code.copy("count");
    code.integer(8, "terminal_max");
    code.raw("OP_GREATERTHAN", 2, &["internal"]);
    code.branch(internal, terminal);
    code.comment(
        "Only an optional final token-free change output may follow all exact allocations.",
    );
    code.raw("OP_TXOUTPUTCOUNT", 0, &["output_count"]);
    code.copy("output_count");
    code.copy("index");
    code.raw("OP_SUB", 2, &["extra"]);
    code.compare_integer("extra", 0, "OP_GREATERTHANOREQUAL");
    code.compare_integer("extra", 1, "OP_LESSTHANOREQUAL");
    code.copy("extra");
    code.branch(
        |code| {
            code.copy("index");
            code.raw("OP_OUTPUTTOKENCATEGORY <0x> OP_EQUALVERIFY", 1, &[]);
            code.copy("index");
            code.raw("OP_OUTPUTVALUE", 1, &["change_value"]);
            code.compare_integer("change_value", 0, "OP_GREATERTHANOREQUAL");
            code.compare_integer("change_value", MONEY_MAX, "OP_LESSTHANOREQUAL");
            code.remove("change_value");
        },
        |_| {},
    );
    code.copy("satoshis");
    code.raw("<0> OP_UTXOVALUE OP_GREATERTHANOREQUAL OP_VERIFY", 1, &[]);
    code.clean_return(&[]);
    code.raw("OP_1", 0, &["success"]);
    code.text.replace("<0x>", "OP_0")
}

/// Compile the exact deterministic prototype program using pinned libauth BCH 2026.
pub fn program() -> Result<Vec<u8>, String> {
    static PROGRAM: OnceLock<Result<Vec<u8>, String>> = OnceLock::new();
    PROGRAM.get_or_init(compile_program).clone()
}

fn compile_program() -> Result<Vec<u8>, String> {
    compile_source(&generated_source())
}

fn compile_source(source: &str) -> Result<Vec<u8>, String> {
    let compiler = create_compiler_bch(CompilerConfiguration {
        scripts: BTreeMap::from([("program".into(), source.to_owned())]),
        vm: Some(create_virtual_machine_bch_2026(Some(false))),
        ..Default::default()
    });
    match compiler.generate_bytecode(&CompilationData::default(), "program") {
        BytecodeGenerationResult::Success { bytecode } => {
            if bytecode.len() > 4096 {
                Err(format!(
                    "prototype program {} bytes exceeds profile 4096",
                    bytecode.len()
                ))
            } else {
                Ok(bytecode)
            }
        }
        BytecodeGenerationResult::Error(error) => Err(format!("CashAssembly: {error:?}")),
    }
}

/// SHA256 digest pinned in canonical state and verified against the executing suffix.
pub fn program_hash() -> Result<[u8; 32], String> {
    Ok((instantiate_sha256().hash)(&program()?)
        .try_into()
        .expect("SHA256 is 32 bytes"))
}

/// Prefix the program with the minimally pushed, exactly 336-byte immutable state.
pub fn redeem_script(state: &[u8]) -> Result<Vec<u8>, String> {
    if state.len() != 336 {
        return Err("state must be exactly 336 bytes".into());
    }
    let program = program()?;
    let mut redeem = Vec::with_capacity(339 + program.len());
    redeem.extend_from_slice(&[0x4d, 0x50, 0x01]);
    redeem.extend_from_slice(state);
    redeem.extend(program);
    Ok(redeem)
}

#[cfg(test)]
mod tests {
    use libauth::instantiate_sha256;

    use super::{compile_source, generated_source, program, program_hash};
    use crate::state::{RootLock, script_hash_lock};
    use crate::test_utils::{
        equal_tree, reject_both, replace_redeem, root_spend, sign_synthetic_funding, verify_both,
    };

    #[test]
    fn covenant_program_compiles_within_frozen_bound() {
        let bytecode = program().expect("the annotated program compiles");
        println!("program length: {}", bytecode.len());
        assert!(bytecode.len() <= 4096);
    }

    #[test]
    fn covenant_checked_artifacts_match() {
        let source = generated_source();
        assert_eq!(source.matches("OP_ACTIVEBYTECODE").count(), 1);
        assert!(!source.contains("OP_CODESEPARATOR"));
        assert_eq!(source, include_str!("../payout.cash"));
        assert_eq!(
            hex::encode(program().expect("compile")),
            include_str!("../program.hex").trim()
        );
        assert_eq!(
            hex::encode(program_hash().expect("hash")),
            include_str!("../program.sha256").trim()
        );
    }

    #[test]
    fn covenant_smoke_internal_root() {
        let tree = equal_tree(9).expect("tree");
        let spend = root_spend(&tree, 1 << 40, RootLock::P2sh20).expect("spend");
        verify_both(&spend).expect("whole VM verifies");
    }

    #[test]
    fn covenant_smoke_terminal_root() {
        let tree = equal_tree(8).expect("tree");
        let spend = root_spend(&tree, 1 << 40, RootLock::P2sh20).expect("spend");
        verify_both(&spend).expect("whole VM verifies");
    }

    #[test]
    fn active_bytecode_inside_invoke_cannot_extract_redeem_even_with_matching_source_lock() {
        let source = generated_source().replacen(
            "OP_ACTIVEBYTECODE\n",
            "<OP_ACTIVEBYTECODE> <3> OP_DEFINE <3> OP_INVOKE\n",
            1,
        );
        let altered_program = compile_source(&source).expect("altered script compiles");
        let mut spend = root_spend(&equal_tree(9).unwrap(), 1 << 40, RootLock::P2sh32).unwrap();
        let mut state = spend.state.encode().unwrap();
        state[72..104].copy_from_slice(&(instantiate_sha256().hash)(&altered_program));
        let mut redeem = vec![0x4d, 0x50, 0x01];
        redeem.extend_from_slice(&state);
        redeem.extend(altered_program);
        spend.source_outputs[0].locking_bytecode = script_hash_lock(&redeem, RootLock::P2sh32);
        let witness = spend.node.witness().unwrap();
        replace_redeem(&mut spend, &witness, &redeem).unwrap();
        sign_synthetic_funding(&mut spend).unwrap();
        reject_both(&spend).expect("invoked ACTIVEBYTECODE returns function body, not the redeem");
    }
}
