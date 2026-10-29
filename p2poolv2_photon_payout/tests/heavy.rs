// SPDX-License-Identifier: AGPL-3.0-or-later

#![cfg(feature = "test-utils")]

use p2poolv2_photon_payout::covenant::{program, program_hash};
use p2poolv2_photon_payout::state::RootLock;
use p2poolv2_photon_payout::test_utils::{equal_tree, execute_tree, random_manifest_stress};

#[test]
#[ignore = "full linked execution: 1023 spends verified in each BCH 2026 VM mode"]
fn a2_a5_complete_4096_leaf_execution_covers_depths_nine_through_one() {
    println!(
        "program {} bytes, SHA256 {}",
        program().unwrap().len(),
        hex::encode(program_hash().unwrap())
    );
    let tree = equal_tree(4_096).unwrap();
    let execution = execute_tree(&tree, i64::MAX as u64, RootLock::P2sh20).unwrap();
    assert_eq!(execution.spends, 1_023);
    assert_eq!(execution.internal_spends, 511);
    assert_eq!(execution.terminal_spends, 512);
    assert_eq!(execution.paid_tokens, i64::MAX as u64);
    assert_eq!(execution.internal_depths, 0b11_1111_1110);
    println!("{execution:?}");
}

#[test]
#[ignore = "100000 seeded manifests, reference telescoping and both complete VM modes"]
fn a8_one_hundred_thousand_seeded_manifests_match_reference_in_both_vms() {
    println!(
        "program {} bytes, SHA256 {}",
        program().unwrap().len(),
        hex::encode(program_hash().unwrap())
    );
    random_manifest_stress(100_000, 0x7068_6f74_6f6e_3261).unwrap();
    println!("100000 manifests; SplitMix64 seed 0x70686f746f6e3261; both VM modes passed");
}
