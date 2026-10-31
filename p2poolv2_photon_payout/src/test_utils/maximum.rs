// SPDX-License-Identifier: AGPL-3.0-or-later

//! Signed public funding fixtures at the profile's exact unlocking-size limit.

use libauth::{
    AuthenticationProgramCommon, encode_data_push, encode_transaction_bch,
    generate_signing_serialization_bch, hash256, secp256k1,
};

use crate::state::{RootLock, script_hash_lock};
use crate::test_utils::{FIXTURE_PUBLIC_KEY, funding_script};
use crate::transaction::BuiltSpend;

/// The exact funding redeem: a 9,868-byte data push, OP_DROP and fixed P2PKH.
///
/// The push has a three-byte OP_PUSHDATA2 header, yielding a 9,897-byte redeem.
/// A 66-byte signature push, 34-byte public-key push and three-byte redeem
/// push header make the complete unlocking bytecode exactly 10,000 bytes.
pub fn maximum_funding_redeem() -> Vec<u8> {
    let mut redeem = encode_data_push(&vec![0; 9_868]);
    redeem.push(0x75);
    redeem.extend_from_slice(&funding_script());
    redeem
}

/// Replace every funder with an exact 10,000-byte, valid signed P2SH32 fixture.
///
/// The public scalar one signs the complete transaction with SIGHASH_ALL,
/// SIGHASH_FORKID and SIGHASH_UTXOS. The final token-free change is adjusted
/// to retain an exact fee of one satoshi per byte. No private key is generated.
pub fn configure_maximum_funding(spend: &mut BuiltSpend) -> Result<(), String> {
    if spend.source_outputs.len() != spend.transaction.inputs.len()
        || !(2..=5).contains(&spend.transaction.inputs.len())
    {
        return Err("maximum-funding fixture requires one through four resolved funders".into());
    }
    if spend
        .transaction
        .outputs
        .last()
        .is_none_or(|output| output.token.is_some())
    {
        return Err("maximum-funding fixture requires a final token-free change output".into());
    }
    let redeem = maximum_funding_redeem();
    let locking_bytecode = script_hash_lock(&redeem, RootLock::P2sh32);
    let mut unlocking = encode_data_push(&[0; 65]);
    unlocking.extend_from_slice(&encode_data_push(&FIXTURE_PUBLIC_KEY));
    unlocking.extend_from_slice(&encode_data_push(&redeem));
    if redeem.len() != 9_897 || unlocking.len() != 10_000 {
        return Err("maximum-funding fixture length calculation changed".into());
    }
    for funding_index in 1..spend.transaction.inputs.len() {
        if spend.source_outputs[funding_index].token.is_some() {
            return Err("maximum-funding fixture cannot replace a token-bearing source".into());
        }
        spend.source_outputs[funding_index].locking_bytecode = locking_bytecode.clone();
        spend.transaction.inputs[funding_index].unlocking_bytecode = unlocking.clone();
    }
    let size = encode_transaction_bch(&spend.transaction).len();
    let fee = u64::try_from(size).map_err(|_| "maximum-funding transaction size overflow")?;
    let change = spend
        .transaction
        .outputs
        .last_mut()
        .expect("validated change");
    if fee >= spend.fee {
        let additional_fee = fee - spend.fee;
        change.value_satoshis = change
            .value_satoshis
            .checked_sub(additional_fee)
            .ok_or("maximum-funding fixture has insufficient BCH change")?;
        spend.external_satoshis = spend
            .external_satoshis
            .checked_add(additional_fee)
            .ok_or("maximum-funding fixture cost overflow")?;
    } else {
        let released_fee = spend.fee - fee;
        change.value_satoshis = change
            .value_satoshis
            .checked_add(released_fee)
            .ok_or("maximum-funding fixture change overflow")?;
        spend.external_satoshis = spend
            .external_satoshis
            .checked_sub(released_fee)
            .ok_or("maximum-funding fixture cost underflow")?;
    }
    let mut public_test_scalar = [0; 32];
    public_test_scalar[31] = 1;
    for funding_index in 1..spend.transaction.inputs.len() {
        let serialization = generate_signing_serialization_bch(
            &AuthenticationProgramCommon {
                input_index: funding_index,
                transaction: spend.transaction.clone(),
                source_outputs: spend.source_outputs.clone(),
            },
            &redeem,
            &[0x61],
            None,
        );
        let digest = hash256(&serialization, None);
        let mut signature = secp256k1.sign_message_hash_schnorr(&public_test_scalar, &digest)?;
        signature.push(0x61);
        let mut signed_unlocking = encode_data_push(&signature);
        signed_unlocking.extend_from_slice(&encode_data_push(&FIXTURE_PUBLIC_KEY));
        signed_unlocking.extend_from_slice(&encode_data_push(&redeem));
        if signed_unlocking.len() != 10_000 {
            return Err("maximum-funding signature changed the required unlocking length".into());
        }
        spend.transaction.inputs[funding_index].unlocking_bytecode = signed_unlocking;
    }
    spend.size = size;
    spend.fee = fee;
    Ok(())
}
