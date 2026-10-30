// SPDX-License-Identifier: AGPL-3.0-or-later

//! Transaction builder for an externally funded, single-win lean split.

use libauth::{Output, Token, TransactionCommon, encode_data_push, encode_transaction_bch};

use crate::lean::reference::{MAX_UNLOCK_BYTES, Manifest};
use crate::reference::{MAX_MONEY, committed_minimum, valid_payout_script};
use crate::state::script_hash_lock;
use crate::transaction::SpendOptions;

/// External BCH funding and a spend-time submitter's repayment destination.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplitOptions {
    pub spend: SpendOptions,
    pub submitter_script: Vec<u8>,
}

/// Complete offline resolved split; funding signatures remain caller supplied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuiltSplit {
    pub transaction: TransactionCommon,
    pub source_outputs: Vec<Output>,
    pub redeem: Vec<u8>,
    pub gross_payments: Vec<u64>,
    pub size: usize,
    pub fee: u64,
    /// Net external BCH: output minima plus fee minus inherited root BCH.
    pub external_satoshis: u64,
    /// All token-output BCH plus fee, before inherited root BCH is credited.
    pub gross_fronting: u64,
}

fn token_output(manifest: &Manifest, script: Vec<u8>, amount: u64) -> Result<Output, String> {
    Ok(Output {
        value_satoshis: committed_minimum(&script)?,
        locking_bytecode: script,
        token: Some(Token {
            category: manifest.category.iter().rev().copied().collect(),
            amount,
            nft: None,
        }),
    })
}

/// Construct exact gross-minus-C payments and C per actual payee to submitter.
///
/// This assumes the manifest comes from an authenticated, reserved share-chain
/// ledger. The covenant cannot prove that an off-chain debt exists. An invalid
/// final clip or inadequate capped coverage returns an error and leaves the
/// root unspent. No partial remainder UTXO or bootstrap credit is invented.
pub fn build_split(
    manifest: &Manifest,
    reward: u64,
    options: &SplitOptions,
) -> Result<BuiltSplit, String> {
    let gross_payments = manifest.allocate(reward)?;
    if !(1..=4).contains(&options.spend.funding.len())
        || !valid_payout_script(&options.submitter_script)
    {
        return Err("need 1..4 token-free funders and a standard submitter script".into());
    }
    if options
        .spend
        .active_outpoint
        .outpoint_transaction_hash
        .len()
        != 32
        || options.spend.active_value > MAX_MONEY
    {
        return Err("invalid active outpoint or BCH value".into());
    }
    if options
        .spend
        .change_script
        .as_ref()
        .is_some_and(|script| !valid_payout_script(script))
    {
        return Err("unsupported BCH change script".into());
    }
    let redeem = manifest.redeem()?;
    let mut unlocking = encode_data_push(&options.submitter_script);
    unlocking.extend_from_slice(&encode_data_push(&redeem));
    if redeem.len() > MAX_UNLOCK_BYTES || unlocking.len() > MAX_UNLOCK_BYTES {
        return Err("lean redeem or unlocking bytecode exceeds 10000 bytes".into());
    }
    let mut active = options.spend.active_outpoint.clone();
    active.sequence_number = u32::MAX;
    active.unlocking_bytecode = unlocking;
    let source = token_output(
        manifest,
        script_hash_lock(&redeem, options.spend.root_lock),
        reward,
    )?;
    let source = Output {
        value_satoshis: options.spend.active_value,
        ..source
    };
    let mut inputs = Vec::with_capacity(1 + options.spend.funding.len());
    inputs.push(active);
    let mut source_outputs = Vec::with_capacity(1 + options.spend.funding.len());
    source_outputs.push(source);
    let mut input_value = options.spend.active_value;
    for funding in &options.spend.funding {
        if funding.source.token.is_some()
            || funding.input.sequence_number != u32::MAX
            || funding.input.outpoint_transaction_hash.len() != 32
            || funding.input.unlocking_bytecode.len() > MAX_UNLOCK_BYTES
        {
            return Err("funding must be token-free, final and bounded".into());
        }
        input_value = input_value
            .checked_add(funding.source.value_satoshis)
            .filter(|value| *value <= MAX_MONEY)
            .ok_or("funding BCH outside MoneyRange")?;
        inputs.push(funding.input.clone());
        source_outputs.push(funding.source.clone());
    }
    let mut outputs = Vec::with_capacity(gross_payments.len() + 2);
    for (entry, gross) in manifest.entries.iter().zip(&gross_payments) {
        outputs.push(token_output(
            manifest,
            entry.script.clone(),
            gross - manifest.cost,
        )?);
    }
    let repayment = manifest
        .cost
        .checked_mul(gross_payments.len() as u64)
        .ok_or("submitter repayment overflow")?;
    outputs.push(token_output(
        manifest,
        options.submitter_script.clone(),
        repayment,
    )?);
    let mut allocation_bch = outputs.iter().try_fold(0u64, |total, output| {
        total
            .checked_add(output.value_satoshis)
            .ok_or("output BCH overflow")
    })?;
    if allocation_bch < options.spend.active_value {
        outputs[0].value_satoshis += options.spend.active_value - allocation_bch;
        allocation_bch = options.spend.active_value;
    }
    if let Some(script) = &options.spend.change_script {
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
    if size > 100_000 {
        return Err("lean split exceeds 100000-byte standard transaction limit".into());
    }
    let available_fee = input_value
        .checked_sub(allocation_bch)
        .ok_or("funding does not cover BCH outputs")?;
    if available_fee < size as u64 {
        return Err("funding does not cover one-sat/byte fee".into());
    }
    let fee = if options.spend.change_script.is_some() {
        let change = transaction.outputs.last_mut().expect("change exists");
        change.value_satoshis = available_fee - size as u64;
        let minimum = 3 * (157 + change.locking_bytecode.len() as u64);
        if change.value_satoshis < minimum {
            return Err("BCH change is dust".into());
        }
        size as u64
    } else {
        available_fee
    };
    let gross_fronting = allocation_bch.checked_add(fee).ok_or("fronting overflow")?;
    Ok(BuiltSplit {
        transaction,
        source_outputs,
        redeem,
        gross_payments,
        size,
        fee,
        external_satoshis: gross_fronting - options.spend.active_value,
        gross_fronting,
    })
}
