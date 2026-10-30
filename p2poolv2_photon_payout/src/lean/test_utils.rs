// SPDX-License-Identifier: AGPL-3.0-or-later

//! Reusable public synthetic fixtures for lean VM tests and measurements.

use crate::lean::{BuiltSplit, Entry, Manifest, ProofBinding, SplitOptions, build_split};
use crate::state::{RootLock, script_hash_lock};
use crate::test_utils::{
    beneficiary_script, funding_input, funding_script, sign_funding_transaction, spend_options,
};
use crate::vm::{evaluate_input, verify_transaction};
use libauth::encode_data_push;

pub const FIXTURE_COST: u64 = 1 << 32;

/// Public placeholder fields for digest binding tests, never a real claim seed.
pub fn proof_binding() -> ProofBinding {
    ProofBinding {
        parent: [1; 32],
        baton_outpoint: [2; 36],
        metadata_hash: [3; 32],
        window_commitment: [4; 32],
        ledger_commitment: [5; 32],
        deployment_lock_hash: [6; 32],
        proof_seed: vec![7; 250],
    }
}

/// Equal debts in canonical script order; all token amounts use nine bytes.
pub fn manifest(count: usize) -> Manifest {
    Manifest {
        network: 0,
        category: [0x22; 32],
        context: [0x44; 32],
        cost: FIXTURE_COST,
        entries: (0..count)
            .map(|index| Entry {
                script: beneficiary_script(index as u32),
                owed: 20 * FIXTURE_COST,
                multiple: 20,
            })
            .collect(),
    }
}

/// One signed P2PKH funder, one P2PKH submitter and one BCH change output.
pub fn fixture(count: usize, lock: RootLock) -> Result<BuiltSplit, String> {
    let manifest = manifest(count);
    build_fixture(&manifest, count as u64 * 20 * FIXTURE_COST, lock)
}

/// Reusable complete signed fixture, including clipped and malformed-state tests.
pub fn build_fixture(
    manifest: &Manifest,
    reward: u64,
    lock: RootLock,
) -> Result<BuiltSplit, String> {
    let mut spend = spend_options(lock, 700);
    spend.funding = vec![funding_input(1, 10_000_000)];
    let mut result = build_split(
        manifest,
        reward,
        &SplitOptions {
            spend,
            submitter_script: funding_script(),
        },
    )?;
    sign_funding_transaction(&mut result.transaction, &result.source_outputs)?;
    Ok(result)
}

/// Synthetic shortest P2SH20 or P2PKH recipients, sorted by bytecode.
pub fn script_profile(count: usize, script_length: usize) -> Result<Manifest, String> {
    let mut result = manifest(count);
    for (index, entry) in result.entries.iter_mut().enumerate() {
        let mut digest = [0x33; 20];
        digest[..4].copy_from_slice(&(index as u32).to_be_bytes());
        entry.script = match script_length {
            23 => {
                let mut script = vec![0xa9, 0x14];
                script.extend_from_slice(&digest);
                script.push(0x87);
                script
            }
            25 => {
                let mut script = vec![0x76, 0xa9, 0x14];
                script.extend_from_slice(&digest);
                script.extend_from_slice(&[0x88, 0xac]);
                script
            }
            35 => entry.script.clone(),
            _ => return Err("unsupported synthetic script profile".into()),
        };
    }
    Ok(result)
}

/// Signed profile fixture with k=L and maximum token amount encodings.
pub fn profile_fixture(count: usize, length: usize) -> Result<BuiltSplit, String> {
    build_fixture(
        &script_profile(count, length)?,
        count as u64 * 20 * FIXTURE_COST,
        RootLock::P2sh20,
    )
}

/// Fill available R bytes with standard script lengths, maximizing VM budget.
pub fn mixed_profile(count: usize) -> Result<Manifest, String> {
    let mut result = script_profile(count, 23)?;
    let mut extra_bytes = 9_000usize
        .checked_sub(36 * count)
        .ok_or("too many shortest records")?;
    let long_count = count.min(extra_bytes / 12);
    extra_bytes -= long_count * 12;
    let middle_count = (count - long_count).min(extra_bytes / 2);
    let longest = manifest(count);
    let middle = script_profile(count, 25)?;
    for (index, entry) in result.entries.iter_mut().enumerate() {
        if index < long_count {
            entry.script = longest.entries[index].script.clone();
        } else if index < long_count + middle_count {
            entry.script = middle.entries[index].script.clone();
        }
    }
    result
        .entries
        .sort_by(|left, right| left.script.cmp(&right.script));
    Ok(result)
}

/// Low-cost boundary probe; parameter choices are fixtures, not owner decisions.
pub fn small_cost_profile(count: usize, unequal: bool) -> Result<Manifest, String> {
    let mut manifest = mixed_profile(count)?;
    manifest.cost = 1;
    for (index, entry) in manifest.entries.iter_mut().enumerate() {
        entry.owed = if unequal {
            (count - index) as u64 + 10
        } else {
            20
        };
        entry.multiple = 10;
    }
    Ok(manifest)
}

/// Build and sign a low-cost capacity probe at its exact full debt sum.
pub fn small_cost_fixture(count: usize, unequal: bool) -> Result<BuiltSplit, String> {
    let manifest = small_cost_profile(count, unequal)?;
    let reward = manifest.entries.iter().map(|entry| entry.owed).sum();
    build_fixture(&manifest, reward, RootLock::P2sh20)
}

/// Small debts after one large priority debt keep root reward above 2^32.
/// This is an amount-layout probe, not a PHOTON claim execution result.
pub fn claim_amount_fixture(count: usize) -> Result<BuiltSplit, String> {
    let mut manifest = small_cost_profile(count, true)?;
    manifest.entries[0].owed = 1 << 32;
    let reward = manifest.entries.iter().map(|entry| entry.owed).sum();
    build_fixture(&manifest, reward, RootLock::P2sh20)
}

/// Replace a raw redeem, optionally re-lock it to probe script semantics.
pub fn replace_redeem(split: &mut BuiltSplit, redeem: Vec<u8>, relock: bool) {
    let submitter_index = split.gross_payments.len();
    let mut unlocking =
        encode_data_push(&split.transaction.outputs[submitter_index].locking_bytecode);
    unlocking.extend_from_slice(&encode_data_push(&redeem));
    split.transaction.inputs[0].unlocking_bytecode = unlocking;
    if relock {
        split.source_outputs[0].locking_bytecode = script_hash_lock(&redeem, RootLock::P2sh20);
    }
    split.redeem = redeem;
}

/// Check the complete transaction in standard and non-standard BCH 2026 VMs.
pub fn verify_both(split: &BuiltSplit) -> Result<(), String> {
    verify_transaction(&split.transaction, &split.source_outputs, true)?;
    verify_transaction(&split.transaction, &split.source_outputs, false)
}

/// Re-sign funders and require active covenant plus complete-transaction failure.
pub fn reject_both(split: &mut BuiltSplit) -> Result<(), String> {
    sign_funding_transaction(&mut split.transaction, &split.source_outputs)?;
    if evaluate_input(&split.transaction, &split.source_outputs, 0, true).is_ok()
        || evaluate_input(&split.transaction, &split.source_outputs, 0, false).is_ok()
        || verify_transaction(&split.transaction, &split.source_outputs, true).is_ok()
        || verify_transaction(&split.transaction, &split.source_outputs, false).is_ok()
    {
        return Err("a mutated lean split was accepted".into());
    }
    Ok(())
}
