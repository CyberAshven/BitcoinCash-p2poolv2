// SPDX-License-Identifier: AGPL-3.0-or-later

//! Independent reference vectors and manifest rejection boundaries.

#![cfg(feature = "test-utils")]

use libauth::runtime::bigint::BigInt;

use p2poolv2_photon_payout::reference::{
    Beneficiary, Manifest, allocation, maximum_window_weight, root_depth, weight_bytes,
};
use p2poolv2_photon_payout::state::State;
use p2poolv2_photon_payout::test_utils::{beneficiary_script, equal_tree};

#[test]
fn canonicalization_aggregates_sorts_and_excludes_zero_totals() {
    let manifest = Manifest::canonicalize(vec![
        Beneficiary {
            script: beneficiary_script(2),
            weight: BigInt::from_u64(3),
        },
        Beneficiary {
            script: beneficiary_script(1),
            weight: BigInt::from_u64(5),
        },
        Beneficiary {
            script: beneficiary_script(2),
            weight: BigInt::from_u64(7),
        },
        Beneficiary {
            script: beneficiary_script(3),
            weight: BigInt::zero(),
        },
    ])
    .unwrap();
    assert_eq!(manifest.leaves.len(), 2);
    assert_eq!(manifest.leaves[0].script, beneficiary_script(1));
    assert_eq!(manifest.leaves[0].weight, BigInt::from_u64(5));
    assert_eq!(manifest.leaves[1].script, beneficiary_script(2));
    assert_eq!(manifest.leaves[1].weight, BigInt::from_u64(10));
    assert_eq!(manifest.total_weight, BigInt::from_u64(15));
}

#[test]
fn canonical_manifest_rejects_zero_weight() {
    assert!(
        Manifest::from_canonical(vec![Beneficiary {
            script: beneficiary_script(1),
            weight: BigInt::zero(),
        }])
        .is_err()
    );
}

#[test]
fn canonical_manifest_rejects_duplicate_script() {
    assert!(
        Manifest::from_canonical(vec![
            Beneficiary {
                script: beneficiary_script(1),
                weight: BigInt::from_u64(1)
            },
            Beneficiary {
                script: beneficiary_script(1),
                weight: BigInt::from_u64(2)
            },
        ])
        .is_err()
    );
}

#[test]
fn canonical_manifest_rejects_unsorted_scripts() {
    assert!(
        Manifest::from_canonical(vec![
            Beneficiary {
                script: beneficiary_script(2),
                weight: BigInt::from_u64(1)
            },
            Beneficiary {
                script: beneficiary_script(1),
                weight: BigInt::from_u64(2)
            },
        ])
        .is_err()
    );
}

#[test]
fn manifest_rejects_negative_weight() {
    assert!(
        Manifest::canonicalize(vec![Beneficiary {
            script: beneficiary_script(1),
            weight: BigInt::from_i64(-1),
        }])
        .is_err()
    );
}

#[test]
fn manifest_rejects_nonstandard_beneficiary() {
    assert!(
        Manifest::canonicalize(vec![Beneficiary {
            script: vec![0x51],
            weight: BigInt::from_u64(1),
        }])
        .is_err()
    );
}

#[test]
fn manifest_rejects_empty_distribution() {
    assert!(Manifest::canonicalize(Vec::with_capacity(0)).is_err());
}

#[test]
fn manifest_rejects_more_than_4096_leaves() {
    assert!(equal_tree(4_097).is_err());
}

#[test]
fn manifest_accepts_exact_maximum_scaled_window() {
    let manifest = Manifest::canonicalize(vec![Beneficiary {
        script: beneficiary_script(1),
        weight: maximum_window_weight(),
    }])
    .unwrap();
    assert_eq!(manifest.total_weight, maximum_window_weight());
    assert_eq!(manifest.total_weight.bit_length(), 272);
}

#[test]
fn manifest_rejects_one_over_maximum_scaled_window() {
    assert!(
        Manifest::canonicalize(vec![Beneficiary {
            script: beneficiary_script(1),
            weight: maximum_window_weight().add(&BigInt::from_u64(1)),
        }])
        .is_err()
    );
}

#[test]
fn manifest_checks_selected_bootstrap_budget() {
    let manifest = Manifest::canonicalize(vec![Beneficiary {
        script: beneficiary_script(1),
        weight: BigInt::from_u64(43_201),
    }])
    .unwrap();
    assert!(
        manifest
            .validate_bootstrap_work(&BigInt::from_u64(1))
            .is_err()
    );
    assert!(
        manifest
            .validate_bootstrap_work(&BigInt::from_u64(2))
            .is_ok()
    );
}

#[test]
fn root_depth_golden_boundaries() {
    assert_eq!(root_depth(1).unwrap(), 0);
    assert_eq!(root_depth(8).unwrap(), 0);
    assert_eq!(root_depth(9).unwrap(), 1);
    assert_eq!(root_depth(16).unwrap(), 1);
    assert_eq!(root_depth(17).unwrap(), 2);
    assert_eq!(root_depth(4_096).unwrap(), 9);
}

#[test]
fn exact_integer_allocation() {
    assert_eq!(
        allocation(
            10,
            &BigInt::from_u64(2),
            &BigInt::from_u64(4),
            &BigInt::from_u64(10)
        )
        .unwrap(),
        2
    );
}

#[test]
fn off_by_one_allocation() {
    assert_eq!(
        allocation(
            10,
            &BigInt::from_u64(1),
            &BigInt::from_u64(2),
            &BigInt::from_u64(3)
        )
        .unwrap(),
        3
    );
    assert_eq!(
        allocation(
            10,
            &BigInt::from_u64(2),
            &BigInt::from_u64(3),
            &BigInt::from_u64(3)
        )
        .unwrap(),
        4
    );
}

#[test]
fn allocation_handles_full_unsigned_512bit_weights_and_575bit_product() {
    let total = BigInt::from_u64(1).shl(512).sub(&BigInt::from_u64(1));
    assert_eq!(
        total.mul(&BigInt::from_u64(i64::MAX as u64)).bit_length(),
        575
    );
    assert_eq!(
        allocation(i64::MAX as u64, &BigInt::zero(), &total, &total).unwrap(),
        i64::MAX as u64
    );
    assert_eq!(weight_bytes(&total).unwrap(), [255; 64]);
}

#[test]
fn allocation_rejects_unsigned_512bit_overflow() {
    let total = BigInt::from_u64(1).shl(512);
    assert!(allocation(1, &BigInt::zero(), &total, &total).is_err());
}

#[test]
fn allocation_rejects_zero_total() {
    assert!(allocation(1, &BigInt::zero(), &BigInt::from_u64(1), &BigInt::zero()).is_err());
}

#[test]
fn allocation_rejects_empty_interval() {
    assert!(
        allocation(
            1,
            &BigInt::from_u64(1),
            &BigInt::from_u64(1),
            &BigInt::from_u64(2)
        )
        .is_err()
    );
}

#[test]
fn state_roundtrips_exactly_336_bytes_with_context_early_in_redeem() {
    let node = equal_tree(9).unwrap();
    let state = State::root(&node, 3, [0x22; 32], [0x44; 32]).unwrap();
    let encoded = state.encode().unwrap();
    assert_eq!(encoded.len(), 336);
    assert_eq!(State::decode(&encoded).unwrap(), state);
    let redeem = state.redeem().unwrap();
    assert_eq!(&redeem[..3], &[0x4d, 0x50, 0x01]);
    assert_eq!(&redeem[43..75], &[0x44; 32]);
}

#[test]
fn state_rejects_unknown_network() {
    assert!(State::root(&equal_tree(1).unwrap(), 5, [0x22; 32], [0x44; 32]).is_err());
}

#[test]
fn state_rejects_trailing_bytes() {
    let state = State::root(&equal_tree(1).unwrap(), 0, [0x22; 32], [0x44; 32]).unwrap();
    let mut encoded = state.encode().unwrap().to_vec();
    encoded.push(0);
    assert!(State::decode(&encoded).is_err());
}

#[test]
fn tree_rejects_an_altered_public_descriptor() {
    let mut node = equal_tree(9).unwrap();
    node.leaf_count = 8;
    assert!(node.validate().is_err());
}
