// SPDX-License-Identifier: AGPL-3.0-or-later

//! Profile maxima must exercise input zero's own introspection budget.

#![cfg(feature = "test-utils")]

use p2poolv2_photon_payout::test_utils::maximum::configure_maximum_funding;
use p2poolv2_photon_payout::test_utils::{internal_fixture, terminal_fixture, verify_both};
use p2poolv2_photon_payout::vm::measure_input;

#[test]
fn four_exact_10000_byte_signed_funding_inputs_internal_spend() {
    let mut spend = internal_fixture().unwrap();
    configure_maximum_funding(&mut spend).unwrap();
    assert_eq!(spend.transaction.inputs[1].unlocking_bytecode.len(), 10_000);
    assert_eq!(spend.transaction.inputs[2].unlocking_bytecode.len(), 10_000);
    assert_eq!(spend.transaction.inputs[3].unlocking_bytecode.len(), 10_000);
    assert_eq!(spend.transaction.inputs[4].unlocking_bytecode.len(), 10_000);
    assert_eq!(spend.fee, spend.size as u64);
    verify_both(&spend).unwrap();
    let measurement = measure_input(&spend.transaction, &spend.source_outputs, 0, true).unwrap();
    assert_eq!(measurement.maximum_stack_item, 10_000);
    assert_eq!(
        measurement.unlocking_length,
        spend.transaction.inputs[0].unlocking_bytecode.len()
    );
}

#[test]
fn four_exact_10000_byte_signed_funding_inputs_terminal_spend() {
    let mut spend = terminal_fixture().unwrap();
    configure_maximum_funding(&mut spend).unwrap();
    assert_eq!(spend.transaction.inputs[1].unlocking_bytecode.len(), 10_000);
    assert_eq!(spend.transaction.inputs[2].unlocking_bytecode.len(), 10_000);
    assert_eq!(spend.transaction.inputs[3].unlocking_bytecode.len(), 10_000);
    assert_eq!(spend.transaction.inputs[4].unlocking_bytecode.len(), 10_000);
    assert_eq!(spend.fee, spend.size as u64);
    verify_both(&spend).unwrap();
    let measurement = measure_input(&spend.transaction, &spend.source_outputs, 0, true).unwrap();
    assert_eq!(measurement.maximum_stack_item, 10_000);
}
