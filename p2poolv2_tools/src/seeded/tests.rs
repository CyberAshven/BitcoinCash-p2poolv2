// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;

#[test]
fn first_value_matches_the_splitmix64_reference() {
    // Reference output for seed 0 from the published algorithm.
    assert_eq!(SeededSequence::new(0).next_u64(), 0xe220_a839_7b1d_cdaf);
}

#[test]
fn same_seed_gives_same_sequence() {
    let mut first = SeededSequence::new(42);
    let mut second = SeededSequence::new(42);
    assert_eq!(first.next_u64(), second.next_u64());
    assert_eq!(first.next_u64(), second.next_u64());
}

#[test]
fn next_unit_is_below_one() {
    let mut sequence = SeededSequence::new(u64::MAX);
    assert!(sequence.next_unit() < 1.0);
}

#[test]
fn next_below_zero_bound_is_zero() {
    assert_eq!(SeededSequence::new(7).next_below(0), 0);
}
