// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Offline payout transaction construction. Funding signatures are external.

use libauth::runtime::bigint::BigInt;
use libauth::{Input, Output, Token, TransactionCommon, encode_data_push, encode_transaction_bch};

use crate::reference::{
    MAX_MONEY, NodeKind, TreeNode, allocation, committed_minimum, root_depth, valid_payout_script,
};
use crate::state::{RootLock, State, script_hash_lock};

/// A funder's complete input and authenticated source output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FundingInput {
    pub input: Input,
    pub source: Output,
}

/// Explicit funding and source choices for one offline payout spend.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpendOptions {
    pub funding: Vec<FundingInput>,
    pub change_script: Option<Vec<u8>>,
    /// Only this input's outpoint is used; sequence and unlocking are canonical.
    pub active_outpoint: Input,
    pub active_value: u64,
    pub root_lock: RootLock,
}

/// The canonical state and subtree controlled by one positive child output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildOutput {
    pub output_index: usize,
    pub state: State,
    pub node: TreeNode,
}

/// Fully serialized payout, its resolved sources and recovery data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuiltSpend {
    pub transaction: TransactionCommon,
    pub source_outputs: Vec<Output>,
    pub state: State,
    pub node: TreeNode,
    pub reward: u64,
    pub children: Vec<ChildOutput>,
    pub fee: u64,
    pub size: usize,
    /// Actual net outside BCH consumed, including fees and allocation top-ups.
    pub external_satoshis: u64,
}

/// Public fixture/outpoint helper; no keys or entropy are generated.
pub fn unsigned_input(txid: [u8; 32], output_index: u32) -> Input {
    Input {
        outpoint_transaction_hash: txid.to_vec(),
        outpoint_index: output_index,
        sequence_number: u32::MAX,
        unlocking_bytecode: Vec::with_capacity(0),
    }
}

/// Resolve a state to its fungible-only source output.
pub fn distribution_source(
    state: &State,
    reward: u64,
    value_satoshis: u64,
    root_lock: RootLock,
) -> Result<Output, String> {
    money_range(value_satoshis)?;
    let amount = state.amount(reward)?;
    if amount == 0 {
        return Err("a zero-allocation node has no distribution UTXO".into());
    }
    let lock = if state.mode == 0 {
        root_lock
    } else {
        RootLock::P2sh32
    };
    Ok(Output {
        locking_bytecode: script_hash_lock(&state.redeem()?, lock),
        value_satoshis,
        token: Some(Token {
            category: state.category.iter().rev().copied().collect(),
            amount,
            nft: None,
        }),
    })
}

fn money_range(value: u64) -> Result<(), String> {
    if value > MAX_MONEY {
        Err("BCH value exceeds MoneyRange".into())
    } else {
        Ok(())
    }
}

fn add_money(sum: u64, value: u64) -> Result<u64, String> {
    money_range(value)?;
    let sum = sum.checked_add(value).ok_or("BCH sum overflow")?;
    money_range(sum)?;
    Ok(sum)
}

fn allocation_output(state: &State, script: Vec<u8>, amount: u64) -> Result<Output, String> {
    Ok(Output {
        value_satoshis: committed_minimum(&script)?,
        locking_bytecode: script,
        token: Some(Token {
            category: state.category.iter().rev().copied().collect(),
            amount,
            nft: None,
        }),
    })
}

fn allocation_outputs(
    state: &State,
    node: &TreeNode,
    reward: u64,
) -> Result<(Vec<Output>, Vec<ChildOutput>), String> {
    if node.hash != state.subtree_hash || node.weight != state.upper.sub(&state.lower) {
        return Err("node does not match immutable state commitment or interval".into());
    }
    if usize::from(node.leaf_count) > (8usize << state.depth) {
        return Err("node does not fit remaining distribution depth".into());
    }
    if state.mode == 0 && state.depth != root_depth(node.leaf_count)? {
        return Err("root depth is not canonical for its leaf count".into());
    }
    let mut outputs = Vec::with_capacity(8);
    let mut children = Vec::with_capacity(2);
    if node.leaf_count > 8 {
        let NodeKind::Branch(left, right) = &node.kind else {
            return Err("internal node has no child pair".into());
        };
        if left.leaf_count != node.leaf_count / 2 || right.leaf_count != node.leaf_count.div_ceil(2)
        {
            return Err("children are not the deterministic midpoint split".into());
        }
        let mut lower = state.lower.clone();
        for child_node in [left, right] {
            let child = state.child(child_node, &lower, reward)?;
            let amount = child.amount(reward)?;
            if amount > 0 {
                let script = script_hash_lock(&child.redeem()?, RootLock::P2sh32);
                children.push(ChildOutput {
                    output_index: outputs.len(),
                    state: child,
                    node: child_node.as_ref().clone(),
                });
                outputs.push(allocation_output(state, script, amount)?);
            }
            lower = lower.add(&child_node.weight);
        }
        if lower != state.upper {
            return Err("child weights do not partition parent interval".into());
        }
    } else {
        let mut lower = state.lower.clone();
        for leaf in node.leaves() {
            let upper = lower.add(&leaf.weight);
            let amount = allocation(reward, &lower, &upper, &state.total_weight)?;
            if amount > 0 {
                outputs.push(allocation_output(state, leaf.script.clone(), amount)?);
            }
            lower = upper;
        }
        if lower != state.upper {
            return Err("terminal leaves do not partition parent interval".into());
        }
    }
    Ok((outputs, children))
}

/// Build root, internal or terminal spending with ordered exact allocations.
///
/// Funding unlocking bytecode is supplied by the caller and may be signed
/// after construction with a fixed-length signature. This function reads no
/// wallet, contacts no network and generates no private keys. The fee is
/// serialized size at one satoshi per byte when change is requested. Without
/// change, all unused funding becomes fee; it must still cover that minimum.
pub fn build_spend(
    state: &State,
    node: &TreeNode,
    reward: u64,
    options: &SpendOptions,
) -> Result<BuiltSpend, String> {
    state.validate()?;
    node.validate()?;
    if !(1..=4).contains(&options.funding.len()) {
        return Err("payout needs 1 through 4 external funding inputs".into());
    }
    if options.active_outpoint.outpoint_transaction_hash.len() != 32 {
        return Err("active outpoint hash must contain 32 bytes".into());
    }
    if options
        .change_script
        .as_ref()
        .is_some_and(|script| !valid_payout_script(script))
    {
        return Err("change script must use a supported standard template".into());
    }
    let source = distribution_source(state, reward, options.active_value, options.root_lock)?;
    let active_amount = source.token.as_ref().expect("distribution token").amount;
    let (mut outputs, children) = allocation_outputs(state, node, reward)?;
    let mut output_tokens = 0u64;
    let mut allocation_satoshis = 0u64;
    for output in &outputs {
        output_tokens = output_tokens
            .checked_add(output.token.as_ref().expect("allocation token").amount)
            .ok_or("token allocation sum overflow")?;
        allocation_satoshis = add_money(allocation_satoshis, output.value_satoshis)?;
    }
    if output_tokens != active_amount || outputs.is_empty() {
        return Err("allocation outputs do not conserve the active input's tokens".into());
    }
    if source.value_satoshis > allocation_satoshis {
        let excess = source.value_satoshis - allocation_satoshis;
        outputs[0].value_satoshis = add_money(outputs[0].value_satoshis, excess)?;
        allocation_satoshis = source.value_satoshis;
    }
    let mut unlocking = encode_data_push(&node.witness()?);
    unlocking.extend_from_slice(&encode_data_push(&state.redeem()?));
    if unlocking.len() > 10_000 {
        return Err("active unlocking bytecode exceeds 10000 bytes".into());
    }
    let mut active_input = options.active_outpoint.clone();
    active_input.sequence_number = u32::MAX;
    active_input.unlocking_bytecode = unlocking;
    let mut inputs = Vec::with_capacity(1 + options.funding.len());
    inputs.push(active_input);
    let mut source_outputs = Vec::with_capacity(1 + options.funding.len());
    source_outputs.push(source);
    let mut input_satoshis = options.active_value;
    let mut funding_satoshis = 0;
    for funding in &options.funding {
        if funding.source.token.is_some()
            || funding.input.sequence_number != u32::MAX
            || funding.input.outpoint_transaction_hash.len() != 32
            || funding.input.unlocking_bytecode.len() > 10_000
        {
            return Err("funding must be token-free, final and bounded".into());
        }
        funding_satoshis = add_money(funding_satoshis, funding.source.value_satoshis)?;
        input_satoshis = add_money(input_satoshis, funding.source.value_satoshis)?;
        inputs.push(funding.input.clone());
        source_outputs.push(funding.source.clone());
    }
    if let Some(script) = &options.change_script {
        outputs.push(Output {
            locking_bytecode: script.clone(),
            value_satoshis: 0,
            token: None,
        });
    }
    let mut transaction = TransactionCommon {
        version: 2,
        locktime: 0,
        inputs,
        outputs,
    };
    let size = encode_transaction_bch(&transaction).len();
    let minimum_fee = u64::try_from(size).map_err(|_| "transaction size overflow")?;
    let surplus = input_satoshis
        .checked_sub(allocation_satoshis)
        .ok_or("external funding does not cover output minima")?;
    if surplus < minimum_fee {
        return Err("external funding does not cover serialized one-sat/byte fee".into());
    }
    let fee = if options.change_script.is_some() {
        let change = transaction.outputs.last_mut().expect("change output");
        change.value_satoshis = surplus - minimum_fee;
        let dust = 3 * (8 + 1 + change.locking_bytecode.len() as u64 + 148);
        if change.value_satoshis < dust {
            return Err("requested BCH change would be dust".into());
        }
        minimum_fee
    } else {
        surplus
    };
    let external_satoshis = allocation_satoshis
        .checked_sub(options.active_value)
        .and_then(|shortfall| shortfall.checked_add(fee))
        .ok_or("external funding cost overflow")?;
    if external_satoshis > funding_satoshis {
        return Err("external funding does not cover total execution cost".into());
    }
    Ok(BuiltSpend {
        transaction,
        source_outputs,
        state: state.clone(),
        node: node.clone(),
        reward,
        children,
        fee,
        size,
        external_satoshis,
    })
}

/// Exact output dust at the default rate, including its serialized token prefix.
pub fn output_dust(output: &Output) -> u64 {
    let transaction = TransactionCommon {
        version: 2,
        locktime: 0,
        inputs: Vec::with_capacity(0),
        outputs: vec![output.clone()],
    };
    // Version, input/output counts and locktime occupy exactly ten bytes.
    3 * (encode_transaction_bch(&transaction).len() as u64 - 10 + 148)
}

/// The first child's upper boundary, useful when walking an execution path.
pub fn left_upper(state: &State, node: &TreeNode) -> Result<BigInt, String> {
    match &node.kind {
        NodeKind::Branch(left, _) => Ok(state.lower.add(&left.weight)),
        NodeKind::Leaf(_) => Err("a leaf has no child boundary".into()),
    }
}
