// SPDX-License-Identifier: AGPL-3.0-or-later

//! Lean split assembled using the existing named-slot CashAssembly builder.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use libauth::{
    BytecodeGenerationResult, CompilationData, CompilerConfiguration, create_compiler_bch,
    create_virtual_machine_bch_2026,
};

use crate::covenant::{Assembly, validate_script};
use crate::lean::reference::ListOrder;

trait ReplaceSlot {
    fn replace_slot(&mut self, name: &str);
}

impl ReplaceSlot for Assembly {
    fn replace_slot(&mut self, name: &str) {
        self.remove(name);
        *self.stack.last_mut().expect("replacement value") = name.into();
    }
}

fn output(code: &mut Assembly, script: &str, amount: &str, index: &str) {
    code.copy(index);
    code.raw("OP_OUTPUTBYTECODE", 1, &["actual"]);
    code.equal("actual", script);
    code.remove("actual");
    code.copy(index);
    code.raw("OP_OUTPUTTOKENCATEGORY", 1, &["actual"]);
    code.equal("actual", "category");
    code.remove("actual");
    code.copy(index);
    code.raw("OP_OUTPUTTOKENCOMMITMENT OP_0 OP_EQUALVERIFY", 1, &[]);
    code.copy(index);
    code.raw("OP_OUTPUTTOKENAMOUNT", 1, &["actual"]);
    code.equal("actual", amount);
    code.remove("actual");
    code.copy(index);
    code.raw("OP_OUTPUTVALUE", 1, &["value"]);
    code.copy(script);
    code.raw(
        "OP_SIZE OP_NIP <200> OP_ADD <3> OP_MUL",
        1,
        &["minimum_bch"],
    );
    code.copy("value");
    code.raw("OP_LESSTHANOREQUAL OP_VERIFY", 2, &[]);
    code.compare_integer("value", 2_100_000_000_000_000, "OP_LESSTHANOREQUAL");
    code.take("value");
    code.copy("output_bch");
    code.raw("OP_ADD", 2, &["sum_bch"]);
    code.replace_slot("output_bch");
}

/// Annotated exact program; no tree opcode or artifact is modified.
pub fn generated_source(order: ListOrder) -> String {
    let mut code = Assembly::new(&["submitter", "state"]);
    match order {
        ListOrder::MostOwed => {
            code.comment("PHOTON lean direct split v1. R commits ordered debts, not a Merkle tree.")
        }
        ListOrder::ShareChain => code.comment(
            "PHOTON lean direct split v2. The share chain orders R; this pays in that order.",
        ),
    }
    code.comment("The early context digest binds every section4 proof-seed choice externally.");
    code.field("state", 0, 4, "version");
    code.compare_integer("version", i128::from(order.version()), "OP_NUMEQUAL");
    code.remove("version");
    code.field("state", 4, 4, "network");
    code.compare_integer("network", 4, "OP_LESSTHANOREQUAL");
    code.remove("network");
    code.slice("state", 8, 32, "category");
    code.field("state", 72, 8, "cost");
    code.compare_integer("cost", 0, "OP_GREATERTHAN");
    code.compare_integer("cost", i64::MAX as i128 / 10, "OP_LESSTHANOREQUAL");
    code.field("state", 80, 2, "count");
    code.compare_integer("count", 0, "OP_GREATERTHAN");
    code.copy("state");
    code.raw("OP_SIZE OP_NIP <82> OP_NUMEQUALVERIFY", 1, &[]);
    code.remove("state");
    code.raw("OP_INPUTINDEX OP_0 OP_NUMEQUALVERIFY OP_TXVERSION OP_2 OP_NUMEQUALVERIFY OP_TXLOCKTIME OP_0 OP_NUMEQUALVERIFY", 0, &[]);
    code.raw("OP_0 OP_UTXOTOKENCATEGORY", 0, &["actual_category"]);
    code.equal("actual_category", "category");
    code.remove("actual_category");
    code.raw(
        "OP_0 OP_UTXOTOKENCOMMITMENT OP_0 OP_EQUALVERIFY OP_0 OP_UTXOTOKENAMOUNT",
        0,
        &["remaining"],
    );
    code.compare_integer("remaining", 0, "OP_GREATERTHAN");
    code.compare_integer("remaining", i64::MAX as i128, "OP_LESSTHANOREQUAL");
    code.raw("OP_TXINPUTCOUNT", 0, &["inputs"]);
    code.compare_integer("inputs", 2, "OP_GREATERTHANOREQUAL");
    code.compare_integer("inputs", 5, "OP_LESSTHANOREQUAL");
    code.integer(0, "input_index");
    code.line("OP_BEGIN");
    let input_stack = code.stack.clone();
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
            code.raw("OP_UTXOTOKENCATEGORY OP_0 OP_EQUALVERIFY", 1, &[]);
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
    code.raw("OP_1ADD", 1, &["next"]);
    code.replace_slot("input_index");
    for name in &input_stack {
        code.take(name);
    }
    code.copy("input_index");
    code.copy("inputs");
    code.raw("OP_NUMEQUAL OP_UNTIL", 2, &[]);
    code.remove("input_index");
    code.remove("inputs");
    code.copy("submitter");
    *code.stack.last_mut().expect("script") = "script".into();
    validate_script(&mut code);
    code.remove("script_length");
    code.remove("script");
    code.integer(0, "entry_index");
    code.integer(0, "paid");
    code.integer(0, "output_bch");
    if order == ListOrder::MostOwed {
        code.integer(i64::MAX as i128, "previous_owed");
        code.integer(0, "previous_script");
    }
    code.line("OP_BEGIN");
    let loop_stack = code.stack.clone();
    code.raw("OP_FROMALTSTACK", 0, &["entries"]);
    code.take("entries");
    code.raw(
        "OP_1 OP_SPLIT OP_SWAP <0x00> OP_CAT OP_BIN2NUM OP_SPLIT",
        1,
        &["script", "rest"],
    );
    code.take("rest");
    code.raw("<8> OP_SPLIT", 1, &["owed_bytes", "rest"]);
    code.take("rest");
    code.raw("OP_2 OP_SPLIT OP_0 OP_EQUALVERIFY", 1, &["multiple_bytes"]);
    code.take("owed_bytes");
    code.unsigned("owed");
    code.take("multiple_bytes");
    code.unsigned("multiple");
    code.compare_integer("multiple", 10, "OP_GREATERTHANOREQUAL");
    code.compare_integer("owed", i64::MAX as i128, "OP_LESSTHANOREQUAL");
    code.copy("multiple");
    code.copy("cost");
    code.raw("OP_MUL", 2, &["minimum"]);
    code.copy("owed");
    code.copy("minimum");
    code.raw("OP_GREATERTHANOREQUAL OP_VERIFY", 2, &[]);
    validate_script(&mut code);
    if order == ListOrder::MostOwed {
        code.copy("script");
        code.raw(
            "OP_REVERSEBYTES <0x00> OP_CAT OP_BIN2NUM",
            1,
            &["script_number"],
        );
        code.copy("script_length");
        code.raw(
            "<35> OP_SWAP OP_SUB <8> OP_MUL OP_LSHIFTNUM",
            2,
            &["sort_key"],
        );
        code.copy("owed");
        code.copy("previous_owed");
        code.raw("OP_LESSTHANOREQUAL OP_VERIFY", 2, &[]);
        code.copy("owed");
        code.copy("previous_owed");
        code.raw("OP_NUMEQUAL", 2, &["tie"]);
        code.branch(
            |code| {
                code.copy("sort_key");
                code.copy("previous_script");
                code.raw("OP_GREATERTHAN OP_VERIFY", 2, &[]);
            },
            |_| {},
        );
        code.take("sort_key");
        code.replace_slot("previous_script");
        code.copy("owed");
        code.replace_slot("previous_owed");
    }
    let payment_stack = code.stack.clone();
    code.copy("remaining");
    code.branch(
        |code| {
            code.copy("owed");
            code.copy("remaining");
            code.raw("OP_MIN", 2, &["gross"]);
            code.copy("gross");
            code.copy("minimum");
            code.raw("OP_GREATERTHANOREQUAL OP_VERIFY", 2, &[]);
            code.copy("gross");
            code.copy("cost");
            code.raw("OP_SUB", 2, &["net"]);
            output(code, "script", "net", "paid");
            code.copy("remaining");
            code.take("gross");
            code.raw("OP_SUB", 2, &["new_remaining"]);
            code.replace_slot("remaining");
            code.copy("paid");
            code.raw("OP_1ADD", 1, &["new_paid"]);
            code.replace_slot("paid");
            code.remove("net");
            for name in &payment_stack {
                code.take(name);
            }
        },
        |_| {},
    );
    for name in ["script", "script_length", "owed", "multiple", "minimum"] {
        code.remove(name);
    }
    code.copy("entry_index");
    code.raw("OP_1ADD", 1, &["next"]);
    code.replace_slot("entry_index");
    for name in &loop_stack {
        code.take(name);
    }
    code.copy("entry_index");
    code.copy("count");
    code.raw("OP_NUMEQUAL OP_UNTIL", 2, &[]);
    code.raw("OP_FROMALTSTACK OP_0 OP_EQUALVERIFY", 0, &[]);
    code.compare_integer("remaining", 0, "OP_NUMEQUAL");
    code.copy("paid");
    code.copy("cost");
    code.raw("OP_MUL", 2, &["submitter_tokens"]);
    output(&mut code, "submitter", "submitter_tokens", "paid");
    code.copy("paid");
    code.raw("OP_1ADD", 1, &["change_index"]);
    code.raw("OP_TXOUTPUTCOUNT", 0, &["output_count"]);
    code.copy("output_count");
    code.copy("change_index");
    code.raw("OP_SUB", 2, &["extra"]);
    code.compare_integer("extra", 0, "OP_GREATERTHANOREQUAL");
    code.compare_integer("extra", 1, "OP_LESSTHANOREQUAL");
    code.copy("extra");
    code.branch(
        |code| {
            code.copy("change_index");
            code.raw("OP_OUTPUTTOKENCATEGORY OP_0 OP_EQUALVERIFY", 1, &[]);
        },
        |_| {},
    );
    code.copy("output_bch");
    code.raw("OP_0 OP_UTXOVALUE OP_GREATERTHANOREQUAL OP_VERIFY", 1, &[]);
    code.clean_return(&[]);
    code.raw("OP_1", 0, &["success"]);
    code.text.replace("<0x>", "OP_0")
}

/// Compile once with pinned libauth; every byte belongs to the new covenant.
pub fn compile_program(order: ListOrder) -> Result<Vec<u8>, String> {
    static MOST_OWED: OnceLock<Result<Vec<u8>, String>> = OnceLock::new();
    static SHARE_CHAIN: OnceLock<Result<Vec<u8>, String>> = OnceLock::new();
    match order {
        ListOrder::MostOwed => &MOST_OWED,
        ListOrder::ShareChain => &SHARE_CHAIN,
    }
    .get_or_init(|| {
        let compiler = create_compiler_bch(CompilerConfiguration {
            scripts: BTreeMap::from([("lean".into(), generated_source(order))]),
            vm: Some(create_virtual_machine_bch_2026(Some(false))),
            ..Default::default()
        });
        match compiler.generate_bytecode(&CompilationData::default(), "lean") {
            BytecodeGenerationResult::Success { bytecode } => Ok(bytecode),
            BytecodeGenerationResult::Error(error) => Err(format!("lean CashAssembly: {error:?}")),
        }
    })
    .clone()
}
