// SPDX-License-Identifier: AGPL-3.0-or-later

//! Reproducible offline VM and BCH cost measurements for the frozen profile.

use std::env::args;
use std::hint::black_box;
use std::time::Instant;

use libauth::runtime::bigint::BigInt;
use libauth::{Input, encode_transaction_bch, hash_transaction_ui_order, hash160};

use p2poolv2_photon_payout::covenant::{program, program_hash};
use p2poolv2_photon_payout::reference::{Beneficiary, Manifest, TreeNode, maximum_window_weight};
use p2poolv2_photon_payout::state::{RootLock, State};
use p2poolv2_photon_payout::test_utils::maximum::configure_maximum_funding;
use p2poolv2_photon_payout::test_utils::{
    beneficiary_script, execute_tree, sign_synthetic_funding, spend_options, verify_both,
};
use p2poolv2_photon_payout::transaction::{BuiltSpend, build_spend, output_dust, unsigned_input};
use p2poolv2_photon_payout::vm::measure_input;

const REWARD: u64 = i64::MAX as u64;

/// Choose maximum-length scripts and a total just below the largest window.
fn maximum_tree(count: usize) -> Result<TreeNode, String> {
    let total = maximum_window_weight().sub(&BigInt::from_u64(1));
    let (ordinary_weight, remainder) = total.div_mod(&BigInt::from_u64(count as u64));
    let entries = (0..count)
        .map(|index| Beneficiary {
            script: beneficiary_script(index as u32),
            weight: if index == count - 1 {
                ordinary_weight.add(&remainder)
            } else {
                ordinary_weight.clone()
            },
        })
        .collect();
    Manifest::canonicalize(entries)?.tree()
}

fn signed_spend(
    state: &State,
    node: &TreeNode,
    root_lock: RootLock,
    active_value: u64,
) -> Result<BuiltSpend, String> {
    let mut spend = build_spend(state, node, REWARD, &spend_options(root_lock, active_value))?;
    sign_synthetic_funding(&mut spend)?;
    verify_both(&spend)?;
    Ok(spend)
}

fn print_measurement(name: &str, spend: &BuiltSpend) -> Result<(), String> {
    let standard = measure_input(&spend.transaction, &spend.source_outputs, 0, true)?;
    let non_standard = measure_input(&spend.transaction, &spend.source_outputs, 0, false)?;
    let dust: Vec<String> = spend
        .transaction
        .outputs
        .iter()
        .map(|output| output_dust(output).to_string())
        .collect();
    println!(
        "| {name} | {} | {} | {} | {:.0}/{:.0}; {:.0}/{:.0} | {:.0}/{:.0}; {:.0}/{:.0} | {} | {} | {} | {} | {} |",
        standard.unlocking_length,
        spend.state.redeem()?.len(),
        program()?.len(),
        standard.operation_cost,
        standard.operation_cost_limit,
        non_standard.operation_cost,
        non_standard.operation_cost_limit,
        standard.hash_iterations,
        standard.hash_iteration_limit,
        non_standard.hash_iterations,
        non_standard.hash_iteration_limit,
        standard
            .peak_memory_slots
            .max(non_standard.peak_memory_slots),
        standard
            .peak_control_depth
            .max(non_standard.peak_control_depth),
        standard
            .maximum_stack_item
            .max(non_standard.maximum_stack_item),
        spend.size,
        dust.join(", "),
    );
    Ok(())
}

/// Construct only the first beneficiary's linked path, funding its siblings.
fn first_path_cost(
    state: &State,
    node: &TreeNode,
    active_value: u64,
    outpoint: Input,
) -> Result<u64, String> {
    let mut options = spend_options(RootLock::P2sh20, active_value);
    options.active_outpoint = outpoint;
    let mut spend = build_spend(state, node, REWARD, &options)?;
    sign_synthetic_funding(&mut spend)?;
    verify_both(&spend)?;
    let transaction_id: [u8; 32] =
        hash_transaction_ui_order(&encode_transaction_bch(&spend.transaction), None)
            .try_into()
            .map_err(|_| "HASH256 length")?;
    let mut cost = spend.external_satoshis;
    if let Some(child) = spend.children.first() {
        let child_cost = first_path_cost(
            &child.state,
            &child.node,
            spend.transaction.outputs[child.output_index].value_satoshis,
            unsigned_input(transaction_id, child.output_index as u32),
        )?;
        cost = cost.checked_add(child_cost).ok_or("path BCH overflow")?;
    }
    Ok(cost)
}

fn main() -> Result<(), String> {
    let arguments: Vec<String> = args().skip(1).collect();
    let spends_only = arguments.as_slice() == ["--spends-only"];
    if !arguments.is_empty() && !spends_only {
        return Err("usage: photon-payout-measure [--spends-only]".into());
    }
    println!("# Prototype measurements (phase 2a)");
    println!();
    println!("Pinned libauth: be3ed16396c9eb9c0e57343474d36079a37c8dae; BCH 2026.");
    println!("Program SHA256: `{}`.", hex::encode(program_hash()?));
    println!();
    println!(
        "All weights total 43,200 * 2^256 - 1, reward 2^63 - 1, scripts 35 bytes, four signed synthetic P2PKH funding inputs, P2PKH change. All transactions verify in both VM modes. Cost/hash pairs are standard; non-standard. Tx bytes equal fee in satoshis at 1 sat/byte. Dust is per ordered allocation output then change, using actual encoded amounts."
    );
    println!();
    println!(
        "| Spend | L | Redeem | Program | Op cost/limit (std; nonstd) | Hash iterations/limit (std; nonstd) | Peak slots | Control depth | Max item | Tx bytes/fee | Dust sats |"
    );
    println!("|---|---:|---:|---:|---|---|---:|---:|---:|---:|---|");
    let tree = maximum_tree(4_096)?;
    let root = State::root(&tree, 0, [0x22; 32], [0x44; 32])?;
    let root20 = signed_spend(&root, &tree, RootLock::P2sh20, 700)?;
    print_measurement("Root P2SH20", &root20)?;
    print_measurement(
        "Root P2SH32",
        &signed_spend(&root, &tree, RootLock::P2sh32, 700)?,
    )?;
    let internal = root20.children.first().ok_or("root has no first child")?;
    let internal_spend = signed_spend(&internal.state, &internal.node, RootLock::P2sh32, 705)?;
    print_measurement("Internal P2SH32", &internal_spend)?;
    let mut terminal_state = internal.state.clone();
    let mut terminal_node = internal.node.clone();
    while terminal_node.leaf_count > 8 {
        let split = signed_spend(&terminal_state, &terminal_node, RootLock::P2sh32, 705)?;
        let child = split
            .children
            .first()
            .ok_or("internal has no first child")?;
        terminal_state = child.state.clone();
        terminal_node = child.node.clone();
    }
    let terminal_spend = signed_spend(&terminal_state, &terminal_node, RootLock::P2sh32, 705)?;
    print_measurement("Terminal 8 P2SH32", &terminal_spend)?;
    let mut maximum_root = root20.clone();
    configure_maximum_funding(&mut maximum_root)?;
    verify_both(&maximum_root)?;
    print_measurement("Root P2SH20, 10,000-byte funders", &maximum_root)?;
    let mut maximum_internal = internal_spend.clone();
    configure_maximum_funding(&mut maximum_internal)?;
    verify_both(&maximum_internal)?;
    print_measurement("Internal P2SH32, 10,000-byte funders", &maximum_internal)?;
    let mut maximum_terminal = terminal_spend.clone();
    configure_maximum_funding(&mut maximum_terminal)?;
    verify_both(&maximum_terminal)?;
    print_measurement("Terminal 8 P2SH32, 10,000-byte funders", &maximum_terminal)?;
    if spends_only {
        return Ok(());
    }
    println!();
    println!(
        "| Leaves | Spends | Fees (sats) | External BCH (sats) | External BCH | First beneficiary path (sats) |"
    );
    println!("|---:|---:|---:|---:|---:|---:|");
    for count in [2, 8, 9, 64, 512, 4_096] {
        let tree = maximum_tree(count)?;
        let state = State::root(&tree, 0, [0x22; 32], [0x44; 32])?;
        let cost = execute_tree(&tree, REWARD, RootLock::P2sh20)?;
        let path = first_path_cost(&state, &tree, 700, unsigned_input([0x66; 32], 1))?;
        println!(
            "| {count} | {} | {} | {} | {:.8} | {} |",
            cost.spends,
            cost.total_fees,
            cost.total_external_satoshis,
            cost.total_external_satoshis as f64 / 100_000_000.0,
            path
        );
    }
    let redeem = root.redeem()?;
    let started = Instant::now();
    for _iteration in 0..100_000 {
        black_box(hash160(black_box(&redeem), None));
    }
    let elapsed = started.elapsed();
    println!();
    println!(
        "CPU HASH160(R): 100,000 hashes in {:.6} s ({:.3} microseconds/seed), this build on this host; no GPU implementation or GPU measurement.",
        elapsed.as_secs_f64(),
        elapsed.as_secs_f64() * 10.0
    );
    println!();
    println!(
        "Method: VM final metrics provide operation cost, hash iterations and their limits. Opcode debug traces provide maxima of stack+altstack+function_count, control_stack.len(), and byte length across both stacks and function bodies, including unlocking and redeem phases. Serialized BCH transactions provide bytes and actual token-prefix dust, 3*(output_bytes+148). Full-tree costs sum each constructed transaction's fee and allocation shortfall; inherited child BCH is passed into the next spend with distinct synthetic funding outpoints. Net outside cost equals final leaf BCH minus root's 700 sats plus fees. Path cost funds only the first canonical beneficiary's ancestors, including sibling outputs and the complete terminal bucket, with four P2PKH funding inputs and change per transaction. BCHN acceptance and group C are not tested."
    );
    Ok(())
}
