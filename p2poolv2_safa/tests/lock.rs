// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The claim's lock height under CHIP-2025-03's ticks, in the BCH 2026 VM.
//!
//! A Bitcoin Cash lock height `L` counts units of 600 ticks: a transaction
//! locked to it is final in the first block whose ticks pass `600 * L`. The
//! covenant locks a claim to its coinbase block's ticks in those units,
//! rounded up (Function 21).
mod test_utils;

use std::collections::BTreeSet;

use p2poolv2_safa::{self as safa, NetworkParameters};
use test_utils::*;

/// Chipnet's last ten-minute block.
const SWITCH: u32 = 332_450;

fn lock_in_vm(caller: &FunctionCaller, height: u32) -> u32 {
    stack_number(&ok(caller.call(21, &[numeric(i64::from(height))]))[0])
        .to_u64()
        .unwrap() as u32
}

/// The first block a transaction locked to `lock` is final in: the first
/// whose ticks pass `600 * lock`.
fn first_final_block(parameters: &NetworkParameters, lock: u32) -> u32 {
    (0..)
        .map(|offset| lock + offset)
        .find(|height| parameters.ticks(*height) > 600 * u64::from(lock))
        .unwrap()
}

/// Heights around chipnet's switch, the tens after it and far past it.
fn heights() -> impl Iterator<Item = u32> {
    (SWITCH - 20..=SWITCH + 40).chain([333_800, 340_000, 432_451])
}

#[test]
fn lock_heights_follow_the_pinned_switch() {
    let chipnet = safa::network_parameters("chipnet").unwrap();
    let caller = FunctionCaller::for_parameters(&chipnet);
    for (height, lock) in [
        (1, 1),
        (332_449, 332_449),
        (332_450, 332_450),
        // One unit covers ten one-minute blocks.
        (332_451, 332_451),
        (332_452, 332_451),
        (332_460, 332_451),
        (332_461, 332_452),
        (332_470, 332_452),
        (333_800, 332_585),
    ] {
        assert_eq!(lock_in_vm(&caller, height), lock, "height {height}");
        assert_eq!(chipnet.lock_height(height), lock, "height {height}");
    }
    // Mainnet has no pinned switch: the lock height is the block height.
    let mainnet = safa::network_parameters("mainnet").unwrap();
    let caller = FunctionCaller::for_parameters(&mainnet);
    for height in [1, 661_647, 661_648, 974_001, 2_000_000] {
        assert_eq!(lock_in_vm(&caller, height), height);
        assert_eq!(mainnet.lock_height(height), height);
    }
}

/// The lock height is the coinbase block's ticks in 600-tick units, rounded
/// up, in the VM as in the model.
#[test]
fn a_lock_height_is_the_coinbase_blocks_ticks_rounded_up() {
    let chipnet = safa::network_parameters("chipnet").unwrap();
    let caller = FunctionCaller::for_parameters(&chipnet);
    for height in heights() {
        let lock = lock_in_vm(&caller, height);
        assert_eq!(lock, chipnet.lock_height(height), "height {height}");
        let ticks = chipnet.ticks(height);
        assert!(600 * u64::from(lock) >= ticks, "height {height}");
        assert!(600 * u64::from(lock - 1) < ticks, "height {height}");
    }
    // Knuth's own ticks for chipnet's blocks 332,450 and 332,460.
    assert_eq!(chipnet.ticks(332_450), 199_470_000);
    assert_eq!(chipnet.ticks(332_460), 199_470_600);
}

/// A claim is never final in its coinbase's block or before it, and is
/// final at most ten one-minute blocks after it: in the very next block
/// while blocks are ten minutes, and one to ten blocks later after the
/// switch, depending on where the block falls in its 600-tick unit.
#[test]
fn a_claim_is_final_after_its_coinbases_block_and_within_ten_blocks() {
    let chipnet = safa::network_parameters("chipnet").unwrap();
    let mut waits_after_the_switch = BTreeSet::new();
    for height in heights() {
        let first = first_final_block(&chipnet, chipnet.lock_height(height));
        assert!(first > height, "height {height}");
        assert!(first <= height + 10, "height {height}");
        if height <= SWITCH {
            assert_eq!(first, height + 1, "height {height}");
        } else {
            waits_after_the_switch.insert(first - height);
        }
    }
    assert_eq!(waits_after_the_switch, (1..=10).collect());
}

/// Before: the covenant locked a claim to its coinbase's block height. After
/// the switch that lock is not final until nine more blocks have passed for
/// every block past the switch, without bound. The lock in ticks stays
/// within ten blocks.
#[test]
fn locking_to_the_block_height_waits_nine_more_blocks_per_block() {
    let chipnet = safa::network_parameters("chipnet").unwrap();
    eprintln!("coinbase  lock before, first block  lock after, first block");
    for (height, before, after) in [
        (332_450, 332_451, 332_451),
        (332_451, 332_461, 332_461),
        (332_452, 332_471, 332_461),
        (332_460, 332_551, 332_461),
        (332_461, 332_561, 332_471),
        (333_800, 345_951, 333_801),
        (340_000, 407_951, 340_001),
    ] {
        let lock = chipnet.lock_height(height);
        eprintln!(
            "{height}    {height}, {}       {lock}, {}",
            first_final_block(&chipnet, height),
            first_final_block(&chipnet, lock)
        );
        assert_eq!(
            first_final_block(&chipnet, height),
            before,
            "height {height}"
        );
        assert_eq!(first_final_block(&chipnet, lock), after, "height {height}");
        if height > SWITCH {
            assert_eq!(before, SWITCH + 10 * (height - SWITCH) + 1);
        }
    }
}

/// The claimer tries a win once its lock is final in the next block: the
/// highest claimable coinbase height is the tip while blocks are ten minutes
/// and at most nine blocks below it after the switch.
#[test]
fn the_highest_claimable_height_is_the_last_whose_lock_is_final_in_the_next_block() {
    let chipnet = safa::network_parameters("chipnet").unwrap();
    let mut behind = BTreeSet::new();
    for tip in (SWITCH - 20..=SWITCH + 60).chain([333_800, 333_801, 340_005]) {
        let claimable = chipnet.last_claimable_height(tip);
        let is_final =
            |height: u32| first_final_block(&chipnet, chipnet.lock_height(height)) <= tip + 1;
        assert!(claimable <= tip, "tip {tip}");
        assert!(is_final(claimable), "tip {tip}");
        assert!(!is_final(claimable + 1), "tip {tip}");
        if tip <= SWITCH {
            assert_eq!(claimable, tip);
        } else {
            behind.insert(tip - claimable);
        }
    }
    assert_eq!(behind, (0..=9).collect());
    let mainnet = safa::network_parameters("mainnet").unwrap();
    for tip in [661_647, 974_000, 2_000_000] {
        assert_eq!(mainnet.last_claimable_height(tip), tip);
    }
}
