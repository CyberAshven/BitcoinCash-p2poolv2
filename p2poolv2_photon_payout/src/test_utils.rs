// SPDX-License-Identifier: AGPL-3.0-or-later

//! Public synthetic fixtures shared by offline tests and measurement tooling.
//!
//! The fixed scalar one is publicly known and must never receive real funds.
//! This module supplies no key generation and reads no wallet material.

pub mod maximum;

use std::collections::VecDeque;

use libauth::{
    AuthenticationProgramCommon, Input, Output, TransactionCommon, encode_data_push,
    encode_transaction_bch, generate_signing_serialization_bch, hash_transaction_ui_order, hash160,
    hash256, secp256k1,
};

use libauth::runtime::bigint::BigInt;

use crate::covenant::redeem_script;
use crate::reference::{Beneficiary, Manifest, TreeNode, allocation, weight_bytes};
use crate::state::{RootLock, State, script_hash_lock};
use crate::transaction::{BuiltSpend, FundingInput, SpendOptions, build_spend, unsigned_input};
use crate::vm::{evaluate_input, verify_transaction};

pub const FIXTURE_PUBLIC_KEY: [u8; 33] = [
    0x02, 0x79, 0xbe, 0x66, 0x7e, 0xf9, 0xdc, 0xbb, 0xac, 0x55, 0xa0, 0x62, 0x95, 0xce, 0x87, 0x0b,
    0x07, 0x02, 0x9b, 0xfc, 0xdb, 0x2d, 0xce, 0x28, 0xd9, 0x59, 0xf2, 0x81, 0x5b, 0x16, 0xf8, 0x17,
    0x98,
];

/// Standard P2PKH lock for the publicly known test scalar one.
pub fn funding_script() -> Vec<u8> {
    let mut script = Vec::with_capacity(25);
    script.extend_from_slice(&[0x76, 0xa9, 0x14]);
    script.extend_from_slice(&hash160(&FIXTURE_PUBLIC_KEY, None));
    script.extend_from_slice(&[0x88, 0xac]);
    script
}

/// Make a token-free funding fixture with a fixed-length signature placeholder.
pub fn funding_input(index: u32, value_satoshis: u64) -> FundingInput {
    let mut transaction_hash = vec![0x55; 32];
    transaction_hash[..4].copy_from_slice(&index.to_le_bytes());
    let mut unlocking = encode_data_push(&[0; 65]);
    unlocking.extend_from_slice(&encode_data_push(&FIXTURE_PUBLIC_KEY));
    FundingInput {
        input: Input {
            outpoint_transaction_hash: transaction_hash,
            outpoint_index: 1,
            sequence_number: u32::MAX,
            unlocking_bytecode: unlocking,
        },
        source: Output {
            locking_bytecode: funding_script(),
            value_satoshis,
            token: None,
        },
    }
}

/// Sign every synthetic P2PKH funding input, preserving its 100-byte length.
///
/// The signatures commit all outputs and all source outputs using byte 0x61:
/// SIGHASH_ALL, SIGHASH_FORKID and SIGHASH_UTXOS. The fixture is publicly
/// spendable by design; it must never be used for live transaction signing.
pub fn sign_synthetic_funding(spend: &mut BuiltSpend) -> Result<(), String> {
    sign_funding_transaction(&mut spend.transaction, &spend.source_outputs)
}

/// Re-sign synthetic funding after an adversarial mutation of the transaction.
pub fn sign_funding_transaction(
    transaction: &mut TransactionCommon,
    source_outputs: &[Output],
) -> Result<(), String> {
    if source_outputs.len() != transaction.inputs.len() {
        return Err("the fixture source-output count differs from the input count".into());
    }
    let mut private_scalar = [0; 32];
    private_scalar[31] = 1;
    let expected_script = funding_script();
    for input_index in 1..transaction.inputs.len() {
        if source_outputs[input_index].locking_bytecode == expected_script {
            let serialization = generate_signing_serialization_bch(
                &AuthenticationProgramCommon {
                    input_index,
                    transaction: transaction.clone(),
                    source_outputs: source_outputs.to_vec(),
                },
                &expected_script,
                &[0x61],
                None,
            );
            let digest = hash256(&serialization, None);
            let mut signature = secp256k1.sign_message_hash_schnorr(&private_scalar, &digest)?;
            signature.push(0x61);
            let mut unlocking = encode_data_push(&signature);
            unlocking.extend_from_slice(&encode_data_push(&FIXTURE_PUBLIC_KEY));
            transaction.inputs[input_index].unlocking_bytecode = unlocking;
        }
    }
    Ok(())
}

/// A distinct syntactically standard 35-byte beneficiary script.
pub fn beneficiary_script(index: u32) -> Vec<u8> {
    let mut script = vec![0xaa, 0x20];
    let mut digest = [0x33; 32];
    digest[..4].copy_from_slice(&index.to_be_bytes());
    script.extend_from_slice(&digest);
    script.push(0x87);
    script
}

/// Construct a canonical equal-weight tree for offline fixture transactions.
pub fn equal_tree(leaf_count: usize) -> Result<TreeNode, String> {
    let entries = (0..leaf_count)
        .map(|index| Beneficiary {
            script: beneficiary_script(index as u32),
            weight: BigInt::from_u64(1),
        })
        .collect();
    Manifest::canonicalize(entries)?.tree()
}

/// Four fixed synthetic P2PKH sources and a P2PKH change output.
pub fn spend_options(root_lock: RootLock, active_value: u64) -> SpendOptions {
    SpendOptions {
        funding: vec![
            funding_input(1, 10_000_000),
            funding_input(2, 10_000_000),
            funding_input(3, 10_000_000),
            funding_input(4, 10_000_000),
        ],
        change_script: Some(funding_script()),
        active_outpoint: Input {
            outpoint_transaction_hash: vec![0x66; 32],
            outpoint_index: 1,
            sequence_number: u32::MAX,
            unlocking_bytecode: Vec::with_capacity(0),
        },
        active_value,
        root_lock,
    }
}

/// Construct and sign a root transaction for a canonical tree.
pub fn root_spend(tree: &TreeNode, reward: u64, root_lock: RootLock) -> Result<BuiltSpend, String> {
    let state = State::root(tree, 0, [0x22; 32], [0x44; 32])?;
    let mut spend = build_spend(&state, tree, reward, &spend_options(root_lock, 700))?;
    sign_synthetic_funding(&mut spend)?;
    Ok(spend)
}

/// Verify a complete fixture under both libauth standardness modes.
pub fn verify_both(spend: &BuiltSpend) -> Result<(), String> {
    verify_transaction(&spend.transaction, &spend.source_outputs, true)
        .map_err(|error| format!("standard VM: {error}"))?;
    verify_transaction(&spend.transaction, &spend.source_outputs, false)
        .map_err(|error| format!("non-standard VM: {error}"))
}

/// Require rejection both by the active covenant and by transaction verification.
///
/// Active-input evaluation avoids mistaking an invalid funding signature or a
/// policy rejection for proof that the covenant enforces a boundary.
pub fn reject_both(spend: &BuiltSpend) -> Result<(), String> {
    if evaluate_input(&spend.transaction, &spend.source_outputs, 0, true).is_ok() {
        return Err("the standard covenant accepted the mutation".into());
    }
    if evaluate_input(&spend.transaction, &spend.source_outputs, 0, false).is_ok() {
        return Err("the non-standard covenant accepted the mutation".into());
    }
    if verify_transaction(&spend.transaction, &spend.source_outputs, true).is_ok() {
        return Err("standard transaction verification accepted the mutation".into());
    }
    if verify_transaction(&spend.transaction, &spend.source_outputs, false).is_ok() {
        return Err("non-standard transaction verification accepted the mutation".into());
    }
    Ok(())
}

/// A signed nine-leaf, maximum-reward root for adversarial internal cases.
pub fn internal_fixture() -> Result<BuiltSpend, String> {
    root_spend(&equal_tree(9)?, i64::MAX as u64, RootLock::P2sh32)
}

/// A signed eight-leaf, maximum-reward root for adversarial terminal cases.
pub fn terminal_fixture() -> Result<BuiltSpend, String> {
    root_spend(&equal_tree(8)?, i64::MAX as u64, RootLock::P2sh32)
}

/// Replace the packed witness while retaining the authentic immutable redeem.
pub fn replace_witness(spend: &mut BuiltSpend, witness: &[u8]) -> Result<(), String> {
    replace_redeem(spend, witness, &spend.state.redeem()?)
}

/// Replace witness and redeem while retaining the source locking bytecode.
pub fn replace_redeem(spend: &mut BuiltSpend, witness: &[u8], redeem: &[u8]) -> Result<(), String> {
    let mut unlocking = encode_data_push(witness);
    unlocking.extend_from_slice(&encode_data_push(redeem));
    spend.transaction.inputs[0].unlocking_bytecode = unlocking;
    Ok(())
}

/// Mutate one immutable field without changing the funded source address.
pub fn mutate_state_byte(spend: &mut BuiltSpend, offset: usize) -> Result<(), String> {
    let mut redeem = spend.state.redeem()?;
    *redeem.get_mut(3 + offset).ok_or("invalid state offset")? ^= 1;
    replace_redeem(spend, &spend.node.witness()?, &redeem)
}

/// Re-lock a deliberately malformed raw state so P2SH binding cannot mask it.
pub fn rebind_raw_state(spend: &mut BuiltSpend, state: &[u8]) -> Result<(), String> {
    let redeem = redeem_script(state)?;
    spend.source_outputs[0].locking_bytecode = script_hash_lock(&redeem, RootLock::P2sh32);
    replace_redeem(spend, &spend.node.witness()?, &redeem)
}

/// Encode a leaf reveal even for an oversized, intentionally invalid bucket.
pub fn terminal_reveal(node: &TreeNode) -> Result<Vec<u8>, String> {
    let mut witness = Vec::with_capacity(2 + usize::from(node.leaf_count) * 100);
    witness.extend_from_slice(&node.leaf_count.to_le_bytes());
    for leaf in node.leaves() {
        witness.push(leaf.script.len() as u8);
        witness.extend_from_slice(&leaf.script);
        witness.extend_from_slice(&weight_bytes(&leaf.weight)?);
    }
    Ok(witness)
}

/// Aggregate exact costs and token checks for a complete synthetic execution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeExecution {
    pub spends: usize,
    pub internal_spends: usize,
    pub terminal_spends: usize,
    pub paid_tokens: u64,
    pub total_fees: u64,
    pub total_external_satoshis: u64,
    pub leaf_satoshis: u64,
    /// Bit d is set after an internal spend at remaining depth d succeeds.
    pub internal_depths: u16,
}

/// Build and verify every linked payout transaction under both VM modes.
///
/// Each transaction gets different public synthetic funding outpoints. Children
/// spend their parent's actual transaction identifier and exact output, so the
/// full execution checks linked UTXO state rather than independent shadow roots.
pub fn execute_tree(
    tree: &TreeNode,
    reward: u64,
    root_lock: RootLock,
) -> Result<TreeExecution, String> {
    let root = root_spend(tree, reward, root_lock)?;
    let mut pending = VecDeque::with_capacity(usize::from(tree.leaf_count));
    pending.push_back(root);
    let mut result = TreeExecution {
        spends: 0,
        internal_spends: 0,
        terminal_spends: 0,
        paid_tokens: 0,
        total_fees: 0,
        total_external_satoshis: 0,
        leaf_satoshis: 0,
        internal_depths: 0,
    };
    while let Some(spend) = pending.pop_front() {
        verify_both(&spend)?;
        let source_amount = spend.source_outputs[0]
            .token
            .as_ref()
            .ok_or("distribution source lacks its token")?
            .amount;
        let output_amount = spend
            .transaction
            .outputs
            .iter()
            .try_fold(0u64, |sum, output| {
                sum.checked_add(output.token.as_ref().map_or(0, |token| token.amount))
                    .ok_or("output token sum overflow")
            })?;
        if source_amount != output_amount {
            return Err("a payout transaction did not conserve its entire input".into());
        }
        result.spends += 1;
        result.total_fees = result
            .total_fees
            .checked_add(spend.fee)
            .ok_or("fee sum overflow")?;
        result.total_external_satoshis = result
            .total_external_satoshis
            .checked_add(spend.external_satoshis)
            .ok_or("funding sum overflow")?;
        if spend.children.is_empty() {
            result.terminal_spends += 1;
            result.paid_tokens = result
                .paid_tokens
                .checked_add(output_amount)
                .ok_or("paid token sum overflow")?;
            result.leaf_satoshis = spend
                .transaction
                .outputs
                .iter()
                .filter(|output| output.token.is_some())
                .try_fold(result.leaf_satoshis, |sum, output| {
                    sum.checked_add(output.value_satoshis)
                        .ok_or("leaf BCH sum overflow")
                })?;
        } else {
            result.internal_spends += 1;
            result.internal_depths |= 1u16 << spend.state.depth;
            let txid: [u8; 32] =
                hash_transaction_ui_order(&encode_transaction_bch(&spend.transaction), None)
                    .try_into()
                    .map_err(|_| "transaction identifier has wrong length")?;
            for child in &spend.children {
                let source = &spend.transaction.outputs[child.output_index];
                let mut options = spend_options(RootLock::P2sh32, source.value_satoshis);
                let next_index = 100 + ((result.spends + pending.len()) * 4) as u32;
                options.funding = vec![
                    funding_input(next_index, 10_000_000),
                    funding_input(next_index + 1, 10_000_000),
                    funding_input(next_index + 2, 10_000_000),
                    funding_input(next_index + 3, 10_000_000),
                ];
                options.active_outpoint = unsigned_input(txid, child.output_index as u32);
                let mut next = build_spend(&child.state, &child.node, reward, &options)?;
                if next.source_outputs[0] != *source {
                    return Err("child source differs from the exact parent output".into());
                }
                sign_synthetic_funding(&mut next)?;
                pending.push_back(next);
            }
        }
    }
    if result.paid_tokens != reward {
        return Err("the full tree did not pay the entire original reward".into());
    }
    Ok(result)
}

/// Reproducible, public-data-only generator for allocation stress tests.
///
/// SplitMix64 is used solely to choose synthetic manifest weights and rewards.
/// It is not a cryptographic random source and never generates key material.
pub struct ManifestGenerator {
    state: u64,
}

impl ManifestGenerator {
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut value = self.state;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^ (value >> 31)
    }

    /// Generate one through eight unequal, positive, occasionally wide leaves.
    pub fn next_manifest(&mut self) -> Result<(Manifest, u64), String> {
        let leaf_count = (self.next_u64() % 8 + 1) as usize;
        let reward = (self.next_u64() & i64::MAX as u64).max(1);
        let mut leaves = Vec::with_capacity(leaf_count);
        for leaf_index in 0..leaf_count {
            let weight = BigInt::from_u64(self.next_u64().max(1));
            let shift = self.next_u64() % 201;
            leaves.push(Beneficiary {
                script: beneficiary_script(leaf_index as u32),
                weight: weight.shl(shift),
            });
        }
        Ok((Manifest::canonicalize(leaves)?, reward))
    }
}

/// Run a seeded manifest stress property against both complete libauth VMs.
///
/// Each manifest must telescope exactly in the independent integer reference;
/// the VM must then accept precisely those output amounts. These stress-only
/// transactions use public P2SH20 OP_TRUE funding, avoiding a signature-speed
/// benchmark. Ordinary acceptance and maximum-size measurements use signed
/// P2PKH inputs. Both fixture types are valid complete resolved transactions.
pub fn random_manifest_stress(count: usize, seed: u64) -> Result<(), String> {
    let mut generator = ManifestGenerator::new(seed);
    for manifest_index in 0..count {
        let (manifest, reward) = generator.next_manifest()?;
        let mut lower = BigInt::zero();
        let mut total_allocated = 0u64;
        for leaf in &manifest.leaves {
            let upper = lower.add(&leaf.weight);
            total_allocated = total_allocated
                .checked_add(allocation(reward, &lower, &upper, &manifest.total_weight)?)
                .ok_or("random allocation sum overflow")?;
            lower = upper;
        }
        if total_allocated != reward || lower != manifest.total_weight {
            return Err(format!(
                "manifest {manifest_index} did not telescope exactly"
            ));
        }
        let tree = manifest.tree()?;
        let state = State::root(&tree, 0, [0x22; 32], [0x44; 32])?;
        let mut options = spend_options(RootLock::P2sh20, 700);
        let mut funding = funding_input(1, 10_000_000);
        funding.source.locking_bytecode = script_hash_lock(&[0x51], RootLock::P2sh20);
        funding.input.unlocking_bytecode = encode_data_push(&[0x51]);
        options.funding = vec![funding];
        let spend = build_spend(&state, &tree, reward, &options)?;
        verify_both(&spend).map_err(|error| format!("manifest {manifest_index}: {error}"))?;
        if (manifest_index + 1) % 10_000 == 0 {
            println!(
                "{} seeded manifests passed both VM modes",
                manifest_index + 1
            );
        }
    }
    Ok(())
}
