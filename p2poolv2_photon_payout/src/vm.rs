// SPDX-License-Identifier: AGPL-3.0-or-later

//! Offline BCH 2026 verification and resource measurements from pinned libauth.

use libauth::{
    AuthenticationProgramCommon, Output, ResolvedTransactionCommon, TransactionCommon,
    create_virtual_machine_bch_2026,
};

/// Resource usage observed while evaluating a single transaction input.
#[derive(Clone, Debug, PartialEq)]
pub struct VmMeasurement {
    pub standard: bool,
    pub unlocking_length: usize,
    pub operation_cost: f64,
    pub operation_cost_limit: f64,
    pub hash_iterations: f64,
    pub hash_iteration_limit: f64,
    pub peak_memory_slots: usize,
    pub peak_control_depth: usize,
    pub maximum_stack_item: usize,
    pub evaluated_instructions: f64,
}

/// Verify every input, BCH values, CashTokens and libauth admission checks.
///
/// This performs no network access and establishes no BCHN acceptance result.
pub fn verify_transaction(
    transaction: &TransactionCommon,
    source_outputs: &[Output],
    standard: bool,
) -> Result<(), String> {
    if transaction.inputs.len() != source_outputs.len() {
        return Err("the source-output count must equal the input count".into());
    }
    create_virtual_machine_bch_2026(Some(standard)).verify(&ResolvedTransactionCommon {
        transaction: transaction.clone(),
        source_outputs: source_outputs.to_vec(),
    })
}

/// Evaluate one input independently of whole-transaction admission checks.
///
/// Negative covenant tests use this to prove failure occurs in the covenant,
/// rather than only through a funding signature or transaction token check.
pub fn evaluate_input(
    transaction: &TransactionCommon,
    source_outputs: &[Output],
    input_index: usize,
    standard: bool,
) -> Result<(), String> {
    if transaction.inputs.len() != source_outputs.len() || input_index >= transaction.inputs.len() {
        return Err("the input or source-output index is invalid".into());
    }
    let machine = create_virtual_machine_bch_2026(Some(standard));
    let state = machine.evaluate(
        &AuthenticationProgramCommon {
            input_index,
            transaction: transaction.clone(),
            source_outputs: source_outputs.to_vec(),
        },
        None,
    );
    machine.state_success(&state)
}

/// Record VM counters and maxima across every post-opcode debug state.
///
/// The trace includes unlocking, locking and redeem evaluations. Memory slots
/// use stack length plus altstack length plus `function_count`, matching the
/// pinned VM's resource check. Maximum item size includes live stacks and
/// function bodies. Operation and hash costs and limits come directly from
/// the final VM metrics, rather than from an opcode-count approximation.
pub fn measure_input(
    transaction: &TransactionCommon,
    source_outputs: &[Output],
    input_index: usize,
    standard: bool,
) -> Result<VmMeasurement, String> {
    if transaction.inputs.len() != source_outputs.len() || input_index >= transaction.inputs.len() {
        return Err("the input or source-output index is invalid".into());
    }
    let machine = create_virtual_machine_bch_2026(Some(standard));
    let trace = machine.debug(
        &AuthenticationProgramCommon {
            input_index,
            transaction: transaction.clone(),
            source_outputs: source_outputs.to_vec(),
        },
        None,
    );
    let final_state = trace.last().ok_or("the VM returned an empty trace")?;
    machine.state_success(final_state)?;
    let peak_memory_slots = trace
        .iter()
        .map(|state| state.stack.len() + state.alternate_stack.len() + state.function_count)
        .max()
        .ok_or("the VM returned an empty trace")?;
    let peak_control_depth = trace
        .iter()
        .map(|state| state.control_stack.len())
        .max()
        .ok_or("the VM returned an empty trace")?;
    let maximum_stack_item = trace
        .iter()
        .flat_map(|state| {
            state
                .stack
                .iter()
                .chain(state.alternate_stack.iter())
                .chain(state.function_table.values())
        })
        .map(Vec::len)
        .max()
        .unwrap_or(0);
    Ok(VmMeasurement {
        standard,
        unlocking_length: transaction.inputs[input_index].unlocking_bytecode.len(),
        operation_cost: final_state.metrics.operation_cost,
        operation_cost_limit: final_state.metrics.maximum_operation_cost,
        hash_iterations: final_state.metrics.hash_digest_iterations,
        hash_iteration_limit: final_state.metrics.maximum_hash_digest_iterations,
        peak_memory_slots,
        peak_control_depth,
        maximum_stack_item,
        evaluated_instructions: final_state.metrics.evaluated_instruction_count,
    })
}
