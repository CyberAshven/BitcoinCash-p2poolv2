// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Bitcoin Cash script checks for share-chain transactions.
//!
//! The share chain accepts two spends and nothing else, which is stricter
//! than Bitcoin Cash and never looser:
//!
//! - **P2PKH.** A share coinbase pays its owner's 25-byte
//!   `OP_DUP OP_HASH160 <20 bytes> OP_EQUALVERIFY OP_CHECKSIG` script, spent
//!   by a `<signature> <public key>` input of two direct pushes.
//! - **The share-trading contract.** A P2SH32 output
//!   (`OP_HASH256 <32 bytes> OP_EQUAL`) whose redeem script is exactly the
//!   hash time locked contract of [`bch_htlc`]. The input is push-only with
//!   minimal pushes and ends with the redeem script. It claims with
//!   `<signature> <public key> <32-byte secret> OP_1` or refunds with
//!   `<signature> <public key> OP_0` once the transaction's lock time
//!   reaches the contract's refund height (BIP65: a height, at least the
//!   refund height, and a non-final input sequence). These are the checks
//!   BCHN's script interpreter makes for the same bytes; any other redeem
//!   script, P2SH20, multisig and token-prefixed outputs are rejected.
//!
//! Outputs are restricted too ([`verify_outputs`]): every output a share
//! transaction creates is P2PKH or P2SH32 and holds at least the value BCHN's
//! dust formula gives for that output. BCHN applies that formula to
//! satoshis, as relay policy; the share chain reuses it as its own floor on
//! share units (10^8 to a share), a share-chain rule BCHN never sees, so a
//! share cannot be split into crumbs. A transaction is also final at the height
//! of the share that includes it ([`verify_final`]), so the refund path
//! cannot be mined before its height.
//!
//! Signature rules follow BCHN's standard script flags:
//!
//! - The sighash type must set `SIGHASH_FORKID` (see [`bch_sighash`]).
//! - A 64-byte signature body is a Bitcoin Cash Schnorr signature (2019
//!   upgrade). It is not BIP340: the challenge hashes the compressed public
//!   key, and the nonce point must have a quadratic-residue Y coordinate.
//! - Any other body is ECDSA in strict DER (BIP66) with a low S value.
//! - The public key is a compressed or uncompressed SEC1 key; hybrid keys
//!   are rejected.
//! - The script code a contract signature commits to is the redeem script.
//! - Inputs carry no witness, because Bitcoin Cash has no witness data.

use crate::shares::bch_htlc::{
    HtlcContract, LOCKTIME_THRESHOLD, SECRET_LENGTH, minimal_pushes, p2sh32_hash,
};
use crate::shares::bch_sighash::{self, SighashError, SighashType};
use bitcoin::consensus::encode::VarInt;
use bitcoin::hashes::{Hash, hash160, sha256, sha256d};
use bitcoin::secp256k1::{self, Message, Secp256k1};
use bitcoin::{Sequence, Transaction, TxOut};
use k256::elliptic_curve::ops::Reduce;
use k256::elliptic_curve::sec1::{FromEncodedPoint, ToEncodedPoint};
use k256::elliptic_curve::{Group, PrimeField};
use k256::{AffinePoint, EncodedPoint, FieldBytes, FieldElement, ProjectivePoint, Scalar, U256};
use sha2::{Digest, Sha256};

const OP_DUP: u8 = 0x76;
const OP_HASH160: u8 = 0xa9;
const OP_EQUALVERIFY: u8 = 0x88;
const OP_CHECKSIG: u8 = 0xac;
const OP_EQUAL: u8 = 0x87;
const PUBKEY_HASH_LENGTH: u8 = 20;
const P2PKH_SCRIPT_LENGTH: usize = 25;
const P2SH20_SCRIPT_LENGTH: usize = 23;
/// Largest push a single opcode byte can make. Larger pushes need
/// `OP_PUSHDATA`, which minimal-push rules forbid for signatures and keys.
const MAX_DIRECT_PUSH: u8 = 0x4b;
/// Length of a Bitcoin Cash Schnorr signature without its sighash byte.
const SCHNORR_SIGNATURE_LENGTH: usize = 64;
const COMPRESSED_KEY_LENGTH: usize = 33;
const UNCOMPRESSED_KEY_LENGTH: usize = 65;

/// Name of these share transaction rules in Bitcoin Cash protocol strings:
/// the share-trading contract, the P2PKH or P2SH32 dust-floored outputs and
/// height finality. A node without them rejects shares this node accepts
/// (a contract spend) and accepts shares it rejects (a dust or `OP_RETURN`
/// output), so the two must not peer.
pub const BCH_SHARE_TRADE_RULE_TAG: &str = "trade-htlc";

/// Most SigChecks one transaction may execute: BCHN's `MAX_TX_SIGCHECKS`
/// (`src/consensus/consensus.h`), BCH consensus since the 2020-05-15
/// upgrade ([SigChecks](https://upgradespecs.bitcoincashnode.org/2020-05-15-sigchecks/)).
/// The share chain holds its transactions to the same limit.
pub const MAX_TX_SIGCHECKS: usize = 3_000;

/// Bytes of block per SigCheck a block may execute: BCHN's
/// `BLOCK_MAXBYTES_MAXSIGCHECKS_RATIO`, so a block of `n` bytes may execute
/// `n / 141` SigChecks. The share chain applies the same density to the
/// share transactions a share carries.
pub const BYTES_PER_SIGCHECK: usize = 141;

/// The SigChecks a share transaction the share chain accepts executes, as
/// BCH counts them (the 2020-05-15 SigChecks specification): every accepted
/// input is a P2PKH spend or a claim or refund of the share-trading
/// contract, each running exactly one `OP_CHECKSIG` with a non-empty
/// signature that must verify, which counts one; outputs count none.
/// Meaningful once [`verify_transaction`] has accepted the transaction.
pub fn sig_checks(transaction: &Transaction) -> usize {
    transaction.input.len()
}

/// BCHN's default `-dustrelayfee`, `DUST_RELAY_TX_FEE` in `policy.h`, in
/// satoshis per kilobyte.
pub const DUST_RELAY_FEE_PER_KILOBYTE: u64 = 1_000;
/// Bytes BCHN assumes an input spending an output takes: outpoint (36),
/// script length (1), a P2PKH signature and key (107) and sequence (4).
const DUST_SPENDING_INPUT_SIZE: u64 = 32 + 4 + 1 + 107 + 4;
/// BCHN calls an output dust when spending it would cost more than a third
/// of its value at the dust relay fee.
const DUST_FEE_MULTIPLE: u64 = 3;

/// Why a Bitcoin Cash share transaction failed its script checks.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BchScriptError {
    /// The spent outputs are not one per input in input order.
    #[error("spent outputs do not match the transaction inputs")]
    SpentOutputsMismatch,
    /// An input carries witness data, which Bitcoin Cash does not have.
    #[error("input {input_index} carries witness data")]
    WitnessPresent { input_index: usize },
    /// The spent output or the input script is not a supported spend.
    #[error(
        "input {input_index} is not a <signature> <public key> spend of a P2PKH output or a claim or refund of the share-trading contract"
    )]
    UnsupportedScript { input_index: usize },
    /// The public key is not a compressed or uncompressed key on the curve.
    #[error("input {input_index} public key is not a valid compressed or uncompressed key")]
    InvalidPublicKey { input_index: usize },
    /// The public key does not hash to the key hash the output names.
    #[error("input {input_index} public key does not match the spent output's key hash")]
    PublicKeyHashMismatch { input_index: usize },
    /// The sighash type is not one Bitcoin Cash accepts here.
    #[error("input {input_index}: {source}")]
    Sighash {
        input_index: usize,
        source: SighashError,
    },
    /// The ECDSA signature is not strict DER.
    #[error("input {input_index} ECDSA signature is not strict DER")]
    NonStrictDer { input_index: usize },
    /// The ECDSA signature has an S value above half the group order.
    #[error("input {input_index} ECDSA signature has a high S value")]
    HighS { input_index: usize },
    /// The signature does not verify against the key and the digest.
    #[error("input {input_index} signature does not verify")]
    SignatureRejected { input_index: usize },
    /// The redeem script does not hash to the spent P2SH32 output.
    #[error("input {input_index} redeem script does not match the P2SH32 output")]
    ScriptHashMismatch { input_index: usize },
    /// The claimed secret is not 32 bytes (`OP_SIZE <32> OP_EQUALVERIFY`).
    #[error("input {input_index} secret is not {SECRET_LENGTH} bytes")]
    SecretSize { input_index: usize },
    /// The claimed secret does not hash to the contract's secret hash.
    #[error("input {input_index} secret does not match the contract's secret hash")]
    SecretMismatch { input_index: usize },
    /// A refund whose transaction lock time is not a block height.
    #[error(
        "input {input_index} refund needs a lock time below {LOCKTIME_THRESHOLD}, got {lock_time}"
    )]
    LockTimeNotHeight { input_index: usize, lock_time: u32 },
    /// A refund whose transaction lock time is below the refund height.
    #[error(
        "input {input_index} refund lock time {lock_time} is below the contract's refund height {refund_height}"
    )]
    RefundTooEarly {
        input_index: usize,
        lock_time: u32,
        refund_height: u32,
    },
    /// A refund whose input sequence is final, which disables its lock time.
    #[error("input {input_index} refund has a final sequence, which disables the lock time")]
    FinalSequence { input_index: usize },
    /// An output that is neither P2PKH nor P2SH32.
    #[error("output {output_index} is neither P2PKH nor P2SH32")]
    UnsupportedOutput { output_index: usize },
    /// An output below BCHN's dust threshold for it.
    #[error("output {output_index} holds {value}, below the dust threshold {threshold}")]
    DustOutput {
        output_index: usize,
        value: u64,
        threshold: u64,
    },
    /// The transaction's lock time is not yet reached at the including height.
    #[error("transaction lock time {lock_time} is not final at share height {block_height}")]
    NotFinal { lock_time: u32, block_height: u32 },
}

/// Check every input of a non-coinbase Bitcoin Cash share transaction.
///
/// `spent_outputs` holds one `(input_index, output)` pair per input, in
/// input order, as `ChainStoreHandle::get_all_prevouts` returns them.
pub fn verify_transaction(
    transaction: &Transaction,
    spent_outputs: &[(usize, TxOut)],
) -> Result<(), BchScriptError> {
    if spent_outputs.len() != transaction.input.len() {
        return Err(BchScriptError::SpentOutputsMismatch);
    }
    for (position, (input_index, spent_output)) in spent_outputs.iter().enumerate() {
        if *input_index != position {
            return Err(BchScriptError::SpentOutputsMismatch);
        }
        verify_input(transaction, position, spent_output)?;
    }
    Ok(())
}

/// Check that every output is P2PKH or P2SH32 and not dust.
///
/// Values are share units, and [`dust_threshold`], BCHN's satoshi formula,
/// serves as the share chain's floor on them. Applies to the outputs of
/// non-coinbase share transactions. A share
/// coinbase pays one whole share unit to its owner's P2PKH script, far above
/// the threshold.
pub fn verify_outputs(transaction: &Transaction) -> Result<(), BchScriptError> {
    for (output_index, output) in transaction.output.iter().enumerate() {
        let script = output.script_pubkey.as_bytes();
        if p2pkh_hash(script).is_none() && p2sh32_hash(script).is_none() {
            return Err(BchScriptError::UnsupportedOutput { output_index });
        }
        let threshold = dust_threshold(output);
        let value = output.value.to_sat();
        if value < threshold {
            return Err(BchScriptError::DustOutput {
                output_index,
                value,
                threshold,
            });
        }
    }
    Ok(())
}

/// BCHN's dust threshold for an output, `GetDustThreshold` in `policy.cpp`.
///
/// Three times the fee, at [`DUST_RELAY_FEE_PER_KILOBYTE`], for the output's
/// serialized size plus the input that would spend it: 546 for P2PKH
/// (34 + 148 bytes) and 576 for P2SH32 (44 + 148 bytes). The value of an
/// output at or above the threshold is not dust.
pub fn dust_threshold(output: &TxOut) -> u64 {
    dust_threshold_for_script_length(output.script_pubkey.len())
}

/// [`dust_threshold`] for an output whose locking script is
/// `script_length` bytes: the threshold depends on the output's size alone.
pub fn dust_threshold_for_script_length(script_length: usize) -> u64 {
    let spend_size = output_size_for_script_length(script_length) as u64 + DUST_SPENDING_INPUT_SIZE;
    DUST_FEE_MULTIPLE * (spend_size * DUST_RELAY_FEE_PER_KILOBYTE / 1_000)
}

/// Serialized bytes of an output whose locking script is `script_length`
/// bytes: the 8-byte value, the CompactSize length and the script.
pub fn output_size_for_script_length(script_length: usize) -> usize {
    8 + VarInt::from(script_length).size() + script_length
}

/// Check that `transaction` is final in a share at `block_height`.
///
/// Bitcoin's `IsFinalTx`: a zero lock time, or every input sequence final,
/// or a height lock time below the including height. A time lock time with a
/// non-final sequence is not final here, which is stricter than Bitcoin
/// Cash: no share transaction needs one, and a share's own time is a weaker
/// clock than a height.
pub fn verify_final(transaction: &Transaction, block_height: u32) -> Result<(), BchScriptError> {
    let lock_time = transaction.lock_time.to_consensus_u32();
    let sequences_final = transaction
        .input
        .iter()
        .all(|input| input.sequence == Sequence::MAX);
    if lock_time == 0
        || sequences_final
        || (lock_time < LOCKTIME_THRESHOLD && lock_time < block_height)
    {
        return Ok(());
    }
    Err(BchScriptError::NotFinal {
        lock_time,
        block_height,
    })
}

/// Check one input against the output it spends.
fn verify_input(
    transaction: &Transaction,
    input_index: usize,
    spent_output: &TxOut,
) -> Result<(), BchScriptError> {
    if !transaction.input[input_index].witness.is_empty() {
        return Err(BchScriptError::WitnessPresent { input_index });
    }
    let script_pubkey = spent_output.script_pubkey.as_bytes();
    if let Some(pubkey_hash) = p2pkh_hash(script_pubkey) {
        let (signature, public_key) =
            signature_and_key(transaction.input[input_index].script_sig.as_bytes())
                .ok_or(BchScriptError::UnsupportedScript { input_index })?;
        let spend = KeySpend {
            script_code: script_pubkey,
            pubkey_hash,
            signature,
            public_key,
        };
        verify_key_spend(transaction, input_index, spent_output, &spend)
    } else if let Some(script_hash) = p2sh32_hash(script_pubkey) {
        verify_contract_spend(transaction, input_index, spent_output, script_hash)
    } else {
        Err(BchScriptError::UnsupportedScript { input_index })
    }
}

/// A signature and key that must match a key hash, signed over `script_code`.
struct KeySpend<'a> {
    script_code: &'a [u8],
    pubkey_hash: &'a [u8; 20],
    signature: &'a [u8],
    public_key: &'a [u8],
}

/// Check a claim or refund of the share-trading contract.
fn verify_contract_spend(
    transaction: &Transaction,
    input_index: usize,
    spent_output: &TxOut,
    script_hash: &[u8; 32],
) -> Result<(), BchScriptError> {
    let unsupported = BchScriptError::UnsupportedScript { input_index };
    let pushes = minimal_pushes(transaction.input[input_index].script_sig.as_bytes())
        .ok_or(unsupported.clone())?;
    let (redeem_script, arguments) = pushes.split_last().ok_or(unsupported.clone())?;
    if sha256d::Hash::hash(redeem_script).as_byte_array() != script_hash {
        return Err(BchScriptError::ScriptHashMismatch { input_index });
    }
    let contract = HtlcContract::from_redeem_script(redeem_script).ok_or(unsupported.clone())?;
    match arguments {
        [signature, public_key, secret, [1]] => {
            if secret.len() != SECRET_LENGTH {
                return Err(BchScriptError::SecretSize { input_index });
            }
            if sha256::Hash::hash(secret).to_byte_array() != contract.secret_hash {
                return Err(BchScriptError::SecretMismatch { input_index });
            }
            let spend = KeySpend {
                script_code: redeem_script,
                pubkey_hash: &contract.claim_pubkey_hash,
                signature,
                public_key,
            };
            verify_key_spend(transaction, input_index, spent_output, &spend)
        }
        [signature, public_key, []] => {
            verify_refund_lock_time(transaction, input_index, contract.refund_height)?;
            let spend = KeySpend {
                script_code: redeem_script,
                pubkey_hash: &contract.refund_pubkey_hash,
                signature,
                public_key,
            };
            verify_key_spend(transaction, input_index, spent_output, &spend)
        }
        _ => Err(unsupported),
    }
}

/// `<refund height> OP_CHECKLOCKTIMEVERIFY` as BIP65 defines it for a height.
fn verify_refund_lock_time(
    transaction: &Transaction,
    input_index: usize,
    refund_height: u32,
) -> Result<(), BchScriptError> {
    let lock_time = transaction.lock_time.to_consensus_u32();
    if lock_time >= LOCKTIME_THRESHOLD {
        return Err(BchScriptError::LockTimeNotHeight {
            input_index,
            lock_time,
        });
    }
    if lock_time < refund_height {
        return Err(BchScriptError::RefundTooEarly {
            input_index,
            lock_time,
            refund_height,
        });
    }
    if transaction.input[input_index].sequence == Sequence::MAX {
        return Err(BchScriptError::FinalSequence { input_index });
    }
    Ok(())
}

/// `OP_DUP OP_HASH160 <key hash> OP_EQUALVERIFY OP_CHECKSIG` for one input.
fn verify_key_spend(
    transaction: &Transaction,
    input_index: usize,
    spent_output: &TxOut,
    spend: &KeySpend,
) -> Result<(), BchScriptError> {
    if !is_compressed_or_uncompressed(spend.public_key) {
        return Err(BchScriptError::InvalidPublicKey { input_index });
    }
    if hash160::Hash::hash(spend.public_key).as_byte_array() != spend.pubkey_hash {
        return Err(BchScriptError::PublicKeyHashMismatch { input_index });
    }
    verify_signature(
        transaction,
        input_index,
        spend.script_code,
        spent_output.value.to_sat(),
        spend.signature,
        spend.public_key,
    )
}

/// `OP_CHECKSIG` for one input: a sighash-typed signature over the digest
/// that commits to `script_code` and the spent `amount`.
pub(crate) fn verify_signature(
    transaction: &Transaction,
    input_index: usize,
    script_code: &[u8],
    amount: u64,
    signature: &[u8],
    public_key: &[u8],
) -> Result<(), BchScriptError> {
    let (body, sighash_byte) = signature
        .split_last()
        .map(|(last, body)| (body, *last))
        .ok_or(BchScriptError::UnsupportedScript { input_index })?;
    let sighash_type =
        SighashType::from_byte(sighash_byte).map_err(|source| BchScriptError::Sighash {
            input_index,
            source,
        })?;
    let digest =
        bch_sighash::signature_hash(transaction, input_index, script_code, amount, sighash_type)
            .map_err(|source| BchScriptError::Sighash {
                input_index,
                source,
            })?;

    if body.len() == SCHNORR_SIGNATURE_LENGTH {
        verify_schnorr(body, public_key, &digest, input_index)
    } else {
        verify_ecdsa(body, public_key, &digest, input_index)
    }
}

/// The 20-byte hash of a P2SH20 locking script
/// (`OP_HASH160 <20 bytes> OP_EQUAL`), or `None` for any other script.
pub fn p2sh20_hash(script: &[u8]) -> Option<&[u8; 20]> {
    if script.len() != P2SH20_SCRIPT_LENGTH
        || script[0] != OP_HASH160
        || script[1] != PUBKEY_HASH_LENGTH
        || script[22] != OP_EQUAL
    {
        return None;
    }
    script[2..22].try_into().ok()
}

/// Whether `script` is a Bitcoin Cash payout script: P2PKH, P2SH20 or
/// P2SH32.
///
/// A share-chain rule for every address a Bitcoin Cash share header names
/// (the payee, the donation and the fee). These are the locking scripts
/// CashAddr encodes (types 0 and 1, 20 and 32 bytes; token-aware types 2 and
/// 3 lock with the same scripts). A witness program, which a share header
/// can otherwise carry, is no Bitcoin Cash script: BCH has no SegWit, and
/// such an output can never be spent (push-only unlocking bytecode leaving
/// one item, consensus since November 2018), so it would only burn its
/// payout.
pub fn is_bch_payout_script(script: &[u8]) -> bool {
    p2pkh_hash(script).is_some() || p2sh20_hash(script).is_some() || p2sh32_hash(script).is_some()
}

/// The 20-byte key hash of a P2PKH script, or `None` for any other script.
pub fn p2pkh_hash(script: &[u8]) -> Option<&[u8; 20]> {
    if script.len() != P2PKH_SCRIPT_LENGTH
        || script[0] != OP_DUP
        || script[1] != OP_HASH160
        || script[2] != PUBKEY_HASH_LENGTH
        || script[23] != OP_EQUALVERIFY
        || script[24] != OP_CHECKSIG
    {
        return None;
    }
    script[3..23].try_into().ok()
}

/// Split an input script made of exactly two direct pushes.
fn signature_and_key(script_sig: &[u8]) -> Option<(&[u8], &[u8])> {
    let (signature, rest) = direct_push(script_sig)?;
    let (public_key, rest) = direct_push(rest)?;
    if rest.is_empty() {
        Some((signature, public_key))
    } else {
        None
    }
}

/// Read one push of 1 to 75 bytes made by its own length opcode.
fn direct_push(script: &[u8]) -> Option<(&[u8], &[u8])> {
    let (&opcode, rest) = script.split_first()?;
    if opcode == 0 || opcode > MAX_DIRECT_PUSH {
        return None;
    }
    let length = usize::from(opcode);
    if rest.len() < length {
        return None;
    }
    Some(rest.split_at(length))
}

/// Bitcoin Cash strict encoding: 33 bytes starting 02 or 03, or 65 bytes
/// starting 04. Hybrid keys (06, 07) are rejected.
fn is_compressed_or_uncompressed(public_key: &[u8]) -> bool {
    match public_key.len() {
        COMPRESSED_KEY_LENGTH => public_key[0] == 0x02 || public_key[0] == 0x03,
        UNCOMPRESSED_KEY_LENGTH => public_key[0] == 0x04,
        _ => false,
    }
}

/// Verify a strict-DER, low-S ECDSA signature with libsecp256k1.
fn verify_ecdsa(
    der: &[u8],
    public_key: &[u8],
    digest: &[u8; 32],
    input_index: usize,
) -> Result<(), BchScriptError> {
    if !is_strict_der(der) {
        return Err(BchScriptError::NonStrictDer { input_index });
    }
    let signature = secp256k1::ecdsa::Signature::from_der(der)
        .map_err(|_| BchScriptError::NonStrictDer { input_index })?;
    let mut normalized = signature;
    normalized.normalize_s();
    if normalized != signature {
        return Err(BchScriptError::HighS { input_index });
    }
    let key = secp256k1::PublicKey::from_slice(public_key)
        .map_err(|_| BchScriptError::InvalidPublicKey { input_index })?;
    Secp256k1::verification_only()
        .verify_ecdsa(&Message::from_digest(*digest), &signature, &key)
        .map_err(|_| BchScriptError::SignatureRejected { input_index })
}

/// BIP66 strict DER for a signature body without its sighash byte.
///
/// The same checks as BCHN's `IsValidDERSignatureEncoding`: a compound of
/// two positive integers with no excess padding, 8 to 72 bytes in all.
fn is_strict_der(der: &[u8]) -> bool {
    let size = der.len();
    if !(8..=72).contains(&size) {
        return false;
    }
    if der[0] != 0x30 || usize::from(der[1]) != size - 2 {
        return false;
    }
    let r_length = usize::from(der[3]);
    if 5 + r_length >= size {
        return false;
    }
    let s_length = usize::from(der[5 + r_length]);
    if r_length + s_length + 6 != size {
        return false;
    }
    if der[2] != 0x02 || r_length == 0 || der[4] & 0x80 != 0 {
        return false;
    }
    if r_length > 1 && der[4] == 0x00 && der[5] & 0x80 == 0 {
        return false;
    }
    if der[r_length + 4] != 0x02 || s_length == 0 || der[r_length + 6] & 0x80 != 0 {
        return false;
    }
    if s_length > 1 && der[r_length + 6] == 0x00 && der[r_length + 7] & 0x80 == 0 {
        return false;
    }
    true
}

/// Verify a Bitcoin Cash Schnorr signature.
///
/// From the 2019-05-15 Schnorr specification: fail if `r >= p` or `s >= n`;
/// `e = SHA256(r || compressed(P) || m) mod n`; `R = sG - eP`; fail if `R` is
/// the point at infinity, if `x(R) != r`, or if `y(R)` is not a quadratic
/// residue modulo `p`.
pub(crate) fn verify_schnorr(
    signature: &[u8],
    public_key: &[u8],
    digest: &[u8; 32],
    input_index: usize,
) -> Result<(), BchScriptError> {
    let rejected = BchScriptError::SignatureRejected { input_index };
    let (r_bytes, s_bytes) = signature.split_at(32);
    let r_bytes = FieldBytes::clone_from_slice(r_bytes);
    if bool::from(FieldElement::from_bytes(&r_bytes).is_none()) {
        return Err(rejected);
    }
    let signature_scalar =
        Option::<Scalar>::from(Scalar::from_repr(FieldBytes::clone_from_slice(s_bytes)))
            .ok_or(rejected.clone())?;
    let encoded_key = EncodedPoint::from_bytes(public_key)
        .map_err(|_| BchScriptError::InvalidPublicKey { input_index })?;
    let key_point = Option::<AffinePoint>::from(AffinePoint::from_encoded_point(&encoded_key))
        .ok_or(BchScriptError::InvalidPublicKey { input_index })?;

    let mut challenge = Sha256::new();
    challenge.update(r_bytes);
    challenge.update(key_point.to_encoded_point(true).as_bytes());
    challenge.update(digest);
    let challenge_scalar = <Scalar as Reduce<U256>>::reduce_bytes(&challenge.finalize());

    let nonce_point = ProjectivePoint::GENERATOR * signature_scalar
        - ProjectivePoint::from(key_point) * challenge_scalar;
    if bool::from(nonce_point.is_identity()) {
        return Err(rejected);
    }
    let nonce_encoded = nonce_point.to_affine().to_encoded_point(false);
    let (Some(nonce_x), Some(nonce_y)) = (nonce_encoded.x(), nonce_encoded.y()) else {
        return Err(rejected);
    };
    if *nonce_x != r_bytes {
        return Err(rejected);
    }
    let y_is_residue = Option::<FieldElement>::from(FieldElement::from_bytes(nonce_y))
        .is_some_and(|nonce_y| bool::from(nonce_y.sqrt().is_some()));
    if !y_is_residue {
        return Err(rejected);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::consensus::encode::deserialize_hex;
    use bitcoin::{Amount, ScriptBuf, Witness};
    use k256::elliptic_curve::ops::Neg;

    use crate::shares::bch_htlc::{p2sh32_script, secret_hash};
    use crate::shares::bch_trade::{
        Coin, TradeKey, build_claim_transaction, build_refund_transaction,
    };
    use crate::test_utils::bch_vm::run_p2sh32_spend;
    use crate::test_utils::{
        BCH_TRANSFER_ECDSA_ALL, BCH_TRANSFER_ECDSA_ALL_UNCOMPRESSED,
        BCH_TRANSFER_ECDSA_ALL_WITHOUT_FORK_ID, BCH_TRANSFER_ECDSA_NONE, BCH_TRANSFER_SCHNORR_ALL,
        BCH_TRANSFER_SCHNORR_ALL_ANYONE_CAN_PAY, BCH_TRANSFER_SCHNORR_ALL_UNCOMPRESSED,
        BCH_TRANSFER_SCHNORR_SINGLE, BCH_TRANSFER_SCRIPT_PUBKEY,
        BCH_TRANSFER_UNCOMPRESSED_SCRIPT_PUBKEY, TRADE_BUYER_SECRET_KEY, TRADE_SECRET,
        TRADE_SELLER_SECRET_KEY,
    };
    use bitcoin::absolute::LockTime;
    use bitcoin::{NetworkKind, OutPoint, PrivateKey, Txid};

    #[test]
    fn schnorr_all_signed_by_bchn_verifies() {
        let transaction: Transaction = deserialize_hex(BCH_TRANSFER_SCHNORR_ALL).unwrap();
        let script = ScriptBuf::from_hex(BCH_TRANSFER_SCRIPT_PUBKEY).unwrap();
        let spent = [
            (
                0,
                TxOut {
                    value: Amount::from_sat(50_000),
                    script_pubkey: script.clone(),
                },
            ),
            (
                1,
                TxOut {
                    value: Amount::from_sat(70_000),
                    script_pubkey: script,
                },
            ),
        ];
        assert_eq!(verify_transaction(&transaction, &spent), Ok(()));
    }

    #[test]
    fn ecdsa_all_signed_by_bchn_verifies() {
        let transaction: Transaction = deserialize_hex(BCH_TRANSFER_ECDSA_ALL).unwrap();
        let script = ScriptBuf::from_hex(BCH_TRANSFER_SCRIPT_PUBKEY).unwrap();
        let spent = [
            (
                0,
                TxOut {
                    value: Amount::from_sat(50_000),
                    script_pubkey: script.clone(),
                },
            ),
            (
                1,
                TxOut {
                    value: Amount::from_sat(70_000),
                    script_pubkey: script,
                },
            ),
        ];
        assert_eq!(verify_transaction(&transaction, &spent), Ok(()));
    }

    #[test]
    fn ecdsa_with_an_uncompressed_key_signed_by_bchn_verifies() {
        let transaction: Transaction =
            deserialize_hex(BCH_TRANSFER_ECDSA_ALL_UNCOMPRESSED).unwrap();
        let script = ScriptBuf::from_hex(BCH_TRANSFER_UNCOMPRESSED_SCRIPT_PUBKEY).unwrap();
        let spent = [
            (
                0,
                TxOut {
                    value: Amount::from_sat(50_000),
                    script_pubkey: script.clone(),
                },
            ),
            (
                1,
                TxOut {
                    value: Amount::from_sat(70_000),
                    script_pubkey: script,
                },
            ),
        ];
        assert_eq!(verify_transaction(&transaction, &spent), Ok(()));
    }

    #[test]
    fn ecdsa_none_signed_by_bchn_verifies_and_ignores_the_outputs() {
        let mut transaction: Transaction = deserialize_hex(BCH_TRANSFER_ECDSA_NONE).unwrap();
        let script = ScriptBuf::from_hex(BCH_TRANSFER_SCRIPT_PUBKEY).unwrap();
        let spent = [
            (
                0,
                TxOut {
                    value: Amount::from_sat(50_000),
                    script_pubkey: script.clone(),
                },
            ),
            (
                1,
                TxOut {
                    value: Amount::from_sat(70_000),
                    script_pubkey: script,
                },
            ),
        ];
        assert_eq!(verify_transaction(&transaction, &spent), Ok(()));

        transaction.output[0].value = Amount::from_sat(1);
        assert_eq!(verify_transaction(&transaction, &spent), Ok(()));
    }

    #[test]
    fn schnorr_single_signed_by_bchn_commits_each_input_to_its_own_output() {
        let mut transaction: Transaction = deserialize_hex(BCH_TRANSFER_SCHNORR_SINGLE).unwrap();
        let script = ScriptBuf::from_hex(BCH_TRANSFER_SCRIPT_PUBKEY).unwrap();
        let spent = [
            (
                0,
                TxOut {
                    value: Amount::from_sat(50_000),
                    script_pubkey: script.clone(),
                },
            ),
            (
                1,
                TxOut {
                    value: Amount::from_sat(70_000),
                    script_pubkey: script,
                },
            ),
        ];
        assert_eq!(verify_transaction(&transaction, &spent), Ok(()));

        transaction.output[1].value = Amount::from_sat(4_000);
        assert_eq!(
            verify_transaction(&transaction, &spent),
            Err(BchScriptError::SignatureRejected { input_index: 1 })
        );
    }

    #[test]
    fn schnorr_anyone_can_pay_signed_by_bchn_verifies_without_the_other_input() {
        let mut transaction: Transaction =
            deserialize_hex(BCH_TRANSFER_SCHNORR_ALL_ANYONE_CAN_PAY).unwrap();
        let script = ScriptBuf::from_hex(BCH_TRANSFER_SCRIPT_PUBKEY).unwrap();
        let spent = [
            (
                0,
                TxOut {
                    value: Amount::from_sat(50_000),
                    script_pubkey: script.clone(),
                },
            ),
            (
                1,
                TxOut {
                    value: Amount::from_sat(70_000),
                    script_pubkey: script.clone(),
                },
            ),
        ];
        assert_eq!(verify_transaction(&transaction, &spent), Ok(()));

        transaction.input.truncate(1);
        let first_only = [(
            0,
            TxOut {
                value: Amount::from_sat(50_000),
                script_pubkey: script,
            },
        )];
        assert_eq!(verify_transaction(&transaction, &first_only), Ok(()));
    }

    #[test]
    fn a_bitcoin_signature_without_fork_id_is_rejected() {
        let transaction: Transaction =
            deserialize_hex(BCH_TRANSFER_ECDSA_ALL_WITHOUT_FORK_ID).unwrap();
        let script = ScriptBuf::from_hex(BCH_TRANSFER_SCRIPT_PUBKEY).unwrap();
        let spent = [
            (
                0,
                TxOut {
                    value: Amount::from_sat(50_000),
                    script_pubkey: script.clone(),
                },
            ),
            (
                1,
                TxOut {
                    value: Amount::from_sat(70_000),
                    script_pubkey: script,
                },
            ),
        ];
        assert_eq!(
            verify_transaction(&transaction, &spent),
            Err(BchScriptError::Sighash {
                input_index: 0,
                source: SighashError::MissingForkId(0x01),
            })
        );
    }

    #[test]
    fn schnorr_rejects_a_different_spent_amount() {
        let transaction: Transaction = deserialize_hex(BCH_TRANSFER_SCHNORR_ALL).unwrap();
        let script = ScriptBuf::from_hex(BCH_TRANSFER_SCRIPT_PUBKEY).unwrap();
        let spent = [
            (
                0,
                TxOut {
                    value: Amount::from_sat(50_001),
                    script_pubkey: script.clone(),
                },
            ),
            (
                1,
                TxOut {
                    value: Amount::from_sat(70_000),
                    script_pubkey: script,
                },
            ),
        ];
        assert_eq!(
            verify_transaction(&transaction, &spent),
            Err(BchScriptError::SignatureRejected { input_index: 0 })
        );
    }

    #[test]
    fn ecdsa_all_rejects_a_changed_output() {
        let mut transaction: Transaction = deserialize_hex(BCH_TRANSFER_ECDSA_ALL).unwrap();
        transaction.output[1].value = Amount::from_sat(4_000);
        let script = ScriptBuf::from_hex(BCH_TRANSFER_SCRIPT_PUBKEY).unwrap();
        let spent = [
            (
                0,
                TxOut {
                    value: Amount::from_sat(50_000),
                    script_pubkey: script.clone(),
                },
            ),
            (
                1,
                TxOut {
                    value: Amount::from_sat(70_000),
                    script_pubkey: script,
                },
            ),
        ];
        assert_eq!(
            verify_transaction(&transaction, &spent),
            Err(BchScriptError::SignatureRejected { input_index: 0 })
        );
    }

    #[test]
    fn schnorr_rejects_a_changed_nonce() {
        let mut transaction: Transaction = deserialize_hex(BCH_TRANSFER_SCHNORR_ALL).unwrap();
        let mut script_sig = transaction.input[0].script_sig.to_bytes();
        script_sig[1] ^= 0x01;
        transaction.input[0].script_sig = ScriptBuf::from_bytes(script_sig);
        let script = ScriptBuf::from_hex(BCH_TRANSFER_SCRIPT_PUBKEY).unwrap();
        let spent = [
            (
                0,
                TxOut {
                    value: Amount::from_sat(50_000),
                    script_pubkey: script.clone(),
                },
            ),
            (
                1,
                TxOut {
                    value: Amount::from_sat(70_000),
                    script_pubkey: script,
                },
            ),
        ];
        assert_eq!(
            verify_transaction(&transaction, &spent),
            Err(BchScriptError::SignatureRejected { input_index: 0 })
        );
    }

    #[test]
    fn ecdsa_rejects_a_high_s_signature() {
        let mut transaction: Transaction = deserialize_hex(BCH_TRANSFER_ECDSA_ALL).unwrap();
        // Input 0 pushes 0x47 bytes: DER 30 44 02 20 <r> 02 20 <s>, then 0x41.
        let script_sig = transaction.input[0].script_sig.to_bytes();
        let der = &script_sig[1..71];
        let low_s = Scalar::from_repr(FieldBytes::clone_from_slice(&der[38..70])).unwrap();
        let high_s = low_s.neg().to_repr();
        let mut high_der = Vec::with_capacity(72);
        high_der.extend_from_slice(&[0x30, 0x45]);
        high_der.extend_from_slice(&der[2..36]);
        high_der.extend_from_slice(&[0x02, 0x21, 0x00]);
        high_der.extend_from_slice(&high_s);
        let mut high_script_sig = Vec::with_capacity(script_sig.len() + 1);
        high_script_sig.push(0x48);
        high_script_sig.extend_from_slice(&high_der);
        high_script_sig.push(0x41);
        high_script_sig.extend_from_slice(&script_sig[72..]);
        transaction.input[0].script_sig = ScriptBuf::from_bytes(high_script_sig);
        let script = ScriptBuf::from_hex(BCH_TRANSFER_SCRIPT_PUBKEY).unwrap();
        let spent = [
            (
                0,
                TxOut {
                    value: Amount::from_sat(50_000),
                    script_pubkey: script.clone(),
                },
            ),
            (
                1,
                TxOut {
                    value: Amount::from_sat(70_000),
                    script_pubkey: script,
                },
            ),
        ];
        assert_eq!(
            verify_transaction(&transaction, &spent),
            Err(BchScriptError::HighS { input_index: 0 })
        );
    }

    #[test]
    fn ecdsa_rejects_padding_that_strict_der_forbids() {
        let mut transaction: Transaction = deserialize_hex(BCH_TRANSFER_ECDSA_ALL).unwrap();
        let script_sig = transaction.input[0].script_sig.to_bytes();
        let der = &script_sig[1..71];
        let mut padded_der = Vec::with_capacity(72);
        padded_der.extend_from_slice(&[0x30, 0x45, 0x02, 0x21, 0x00]);
        padded_der.extend_from_slice(&der[4..]);
        let mut padded_script_sig = Vec::with_capacity(script_sig.len() + 1);
        padded_script_sig.push(0x48);
        padded_script_sig.extend_from_slice(&padded_der);
        padded_script_sig.push(0x41);
        padded_script_sig.extend_from_slice(&script_sig[72..]);
        transaction.input[0].script_sig = ScriptBuf::from_bytes(padded_script_sig);
        let script = ScriptBuf::from_hex(BCH_TRANSFER_SCRIPT_PUBKEY).unwrap();
        let spent = [
            (
                0,
                TxOut {
                    value: Amount::from_sat(50_000),
                    script_pubkey: script.clone(),
                },
            ),
            (
                1,
                TxOut {
                    value: Amount::from_sat(70_000),
                    script_pubkey: script,
                },
            ),
        ];
        assert_eq!(
            verify_transaction(&transaction, &spent),
            Err(BchScriptError::NonStrictDer { input_index: 0 })
        );
    }

    #[test]
    fn a_key_that_does_not_hash_to_the_output_is_rejected() {
        let transaction: Transaction = deserialize_hex(BCH_TRANSFER_SCHNORR_ALL).unwrap();
        let other = ScriptBuf::from_hex(BCH_TRANSFER_UNCOMPRESSED_SCRIPT_PUBKEY).unwrap();
        let script = ScriptBuf::from_hex(BCH_TRANSFER_SCRIPT_PUBKEY).unwrap();
        let spent = [
            (
                0,
                TxOut {
                    value: Amount::from_sat(50_000),
                    script_pubkey: other,
                },
            ),
            (
                1,
                TxOut {
                    value: Amount::from_sat(70_000),
                    script_pubkey: script,
                },
            ),
        ];
        assert_eq!(
            verify_transaction(&transaction, &spent),
            Err(BchScriptError::PublicKeyHashMismatch { input_index: 0 })
        );
    }

    #[test]
    fn a_p2sh_output_is_not_supported() {
        let transaction: Transaction = deserialize_hex(BCH_TRANSFER_SCHNORR_ALL).unwrap();
        let p2sh = ScriptBuf::from_hex("a9147b00ca78ec9935f4794bf1b1af2c08f2e33dfc7487").unwrap();
        let script = ScriptBuf::from_hex(BCH_TRANSFER_SCRIPT_PUBKEY).unwrap();
        let spent = [
            (
                0,
                TxOut {
                    value: Amount::from_sat(50_000),
                    script_pubkey: p2sh,
                },
            ),
            (
                1,
                TxOut {
                    value: Amount::from_sat(70_000),
                    script_pubkey: script,
                },
            ),
        ];
        assert_eq!(
            verify_transaction(&transaction, &spent),
            Err(BchScriptError::UnsupportedScript { input_index: 0 })
        );
    }

    #[test]
    fn witness_data_is_rejected() {
        let mut transaction: Transaction = deserialize_hex(BCH_TRANSFER_SCHNORR_ALL).unwrap();
        transaction.input[1].witness = Witness::from_slice(&[[0x01u8]]);
        let script = ScriptBuf::from_hex(BCH_TRANSFER_SCRIPT_PUBKEY).unwrap();
        let spent = [
            (
                0,
                TxOut {
                    value: Amount::from_sat(50_000),
                    script_pubkey: script.clone(),
                },
            ),
            (
                1,
                TxOut {
                    value: Amount::from_sat(70_000),
                    script_pubkey: script,
                },
            ),
        ];
        assert_eq!(
            verify_transaction(&transaction, &spent),
            Err(BchScriptError::WitnessPresent { input_index: 1 })
        );
    }

    #[test]
    fn spent_outputs_out_of_input_order_are_rejected() {
        let transaction: Transaction = deserialize_hex(BCH_TRANSFER_SCHNORR_ALL).unwrap();
        let script = ScriptBuf::from_hex(BCH_TRANSFER_SCRIPT_PUBKEY).unwrap();
        let swapped = [
            (
                1,
                TxOut {
                    value: Amount::from_sat(70_000),
                    script_pubkey: script.clone(),
                },
            ),
            (
                0,
                TxOut {
                    value: Amount::from_sat(50_000),
                    script_pubkey: script.clone(),
                },
            ),
        ];
        assert_eq!(
            verify_transaction(&transaction, &swapped),
            Err(BchScriptError::SpentOutputsMismatch)
        );
        let one = [(
            0,
            TxOut {
                value: Amount::from_sat(50_000),
                script_pubkey: script,
            },
        )];
        assert_eq!(
            verify_transaction(&transaction, &one),
            Err(BchScriptError::SpentOutputsMismatch)
        );
    }

    #[test]
    fn schnorr_with_an_uncompressed_key_signed_by_bchn_verifies() {
        let transaction: Transaction =
            deserialize_hex(BCH_TRANSFER_SCHNORR_ALL_UNCOMPRESSED).unwrap();
        let script = ScriptBuf::from_hex(BCH_TRANSFER_UNCOMPRESSED_SCRIPT_PUBKEY).unwrap();
        let spent = [
            (
                0,
                TxOut {
                    value: Amount::from_sat(50_000),
                    script_pubkey: script.clone(),
                },
            ),
            (
                1,
                TxOut {
                    value: Amount::from_sat(70_000),
                    script_pubkey: script,
                },
            ),
        ];
        assert_eq!(verify_transaction(&transaction, &spent), Ok(()));
    }

    /// Nonce 3G has a Y coordinate that is not a square modulo p; nonce -3G
    /// has the same X and a square Y. Signatures over both share `r`, so only
    /// the quadratic-residue check tells them apart.
    #[test]
    fn schnorr_accepts_only_the_nonce_with_a_square_y() {
        let secret = Scalar::from(7u64);
        let public_key = (ProjectivePoint::GENERATOR * secret)
            .to_affine()
            .to_encoded_point(true);
        let digest = [0x5a; 32];
        let nonce = Scalar::from(3u64);
        let nonce_encoded = (ProjectivePoint::GENERATOR * nonce)
            .to_affine()
            .to_encoded_point(false);
        let nonce_x = nonce_encoded.x().unwrap();
        let mut challenge = Sha256::new();
        challenge.update(nonce_x);
        challenge.update(public_key.as_bytes());
        challenge.update(digest);
        let challenge_scalar = <Scalar as Reduce<U256>>::reduce_bytes(&challenge.finalize());

        let mut non_square_signature = Vec::with_capacity(64);
        non_square_signature.extend_from_slice(nonce_x);
        non_square_signature.extend_from_slice(&(nonce + challenge_scalar * secret).to_repr());
        let mut square_signature = Vec::with_capacity(64);
        square_signature.extend_from_slice(nonce_x);
        square_signature.extend_from_slice(&(nonce.neg() + challenge_scalar * secret).to_repr());

        assert_eq!(
            verify_schnorr(&non_square_signature, public_key.as_bytes(), &digest, 0),
            Err(BchScriptError::SignatureRejected { input_index: 0 })
        );
        assert_eq!(
            verify_schnorr(&square_signature, public_key.as_bytes(), &digest, 0),
            Ok(())
        );
    }

    #[test]
    fn schnorr_rejects_an_r_not_below_the_field_size() {
        let public_key = (ProjectivePoint::GENERATOR * Scalar::from(7u64))
            .to_affine()
            .to_encoded_point(true);
        let mut signature = [0x01u8; 64];
        signature[..32].copy_from_slice(&[0xff; 32]);
        assert_eq!(
            verify_schnorr(&signature, public_key.as_bytes(), &[0x5a; 32], 0),
            Err(BchScriptError::SignatureRejected { input_index: 0 })
        );
    }

    #[test]
    fn hybrid_public_keys_are_rejected() {
        assert!(!is_compressed_or_uncompressed(&[0x06; 65]));
        assert!(!is_compressed_or_uncompressed(&[0x07; 65]));
        assert!(is_compressed_or_uncompressed(&[0x04; 65]));
        assert!(is_compressed_or_uncompressed(&[0x02; 33]));
    }

    // The share-trading contract. Each test signs a real Bitcoin Cash
    // transaction with the fixed trade keys: the buyer claims shares with the
    // secret, the seller refunds them after the refund height.

    #[test]
    fn a_claim_with_the_secret_verifies() {
        let buyer = TradeKey::new(
            PrivateKey::from_slice(&TRADE_BUYER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let seller = TradeKey::new(
            PrivateKey::from_slice(&TRADE_SELLER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let contract = HtlcContract::new(
            secret_hash(&TRADE_SECRET),
            buyer.pubkey_hash(),
            seller.pubkey_hash(),
            5_000,
        )
        .unwrap();
        let coin = Coin {
            outpoint: OutPoint::new(Txid::from_byte_array([0x31; 32]), 0),
            value: Amount::from_sat(40_000_000),
        };
        let claim = build_claim_transaction(
            &coin,
            &contract,
            &buyer,
            &TRADE_SECRET,
            &buyer.p2pkh_script(),
            0,
        )
        .unwrap();
        let spent = [(
            0,
            TxOut {
                value: coin.value,
                script_pubkey: contract.locking_script(),
            },
        )];
        assert_eq!(verify_transaction(&claim, &spent), Ok(()));
        assert_eq!(verify_outputs(&claim), Ok(()));
    }

    /// A Bitcoin Cash Schnorr signature over the same digest, as a wallet
    /// signing with `-signschnorr` would produce. The nonce is negated when
    /// its point's Y is not a square, as the Schnorr specification requires
    /// of signers.
    #[test]
    fn a_schnorr_signed_claim_verifies() {
        let buyer = TradeKey::new(
            PrivateKey::from_slice(&TRADE_BUYER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let seller = TradeKey::new(
            PrivateKey::from_slice(&TRADE_SELLER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let contract = HtlcContract::new(
            secret_hash(&TRADE_SECRET),
            buyer.pubkey_hash(),
            seller.pubkey_hash(),
            5_000,
        )
        .unwrap();
        let coin = Coin {
            outpoint: OutPoint::new(Txid::from_byte_array([0x32; 32]), 0),
            value: Amount::from_sat(40_000_000),
        };
        let mut claim = build_claim_transaction(
            &coin,
            &contract,
            &buyer,
            &TRADE_SECRET,
            &buyer.p2pkh_script(),
            0,
        )
        .unwrap();
        let digest = bch_sighash::signature_hash(
            &claim,
            0,
            &contract.redeem_script(),
            coin.value.to_sat(),
            SighashType::from_byte(0x41).unwrap(),
        )
        .unwrap();
        let secret_scalar =
            Scalar::from_repr(FieldBytes::clone_from_slice(&TRADE_BUYER_SECRET_KEY)).unwrap();
        let first_nonce = Scalar::from(0x1234_5678u64);
        let first_point = (ProjectivePoint::GENERATOR * first_nonce)
            .to_affine()
            .to_encoded_point(false);
        let first_y = FieldElement::from_bytes(first_point.y().unwrap()).unwrap();
        let nonce = if bool::from(first_y.sqrt().is_some()) {
            first_nonce
        } else {
            first_nonce.neg()
        };
        let nonce_x = *(ProjectivePoint::GENERATOR * nonce)
            .to_affine()
            .to_encoded_point(false)
            .x()
            .unwrap();
        let mut challenge = Sha256::new();
        challenge.update(nonce_x);
        challenge.update(buyer.public_key());
        challenge.update(digest);
        let challenge_scalar = <Scalar as Reduce<U256>>::reduce_bytes(&challenge.finalize());
        let mut signature = Vec::with_capacity(65);
        signature.extend_from_slice(&nonce_x);
        signature.extend_from_slice(&(nonce + challenge_scalar * secret_scalar).to_repr());
        signature.push(0x41);
        claim.input[0].script_sig =
            contract.claim_unlocking_script(&signature, buyer.public_key(), &TRADE_SECRET);

        let spent = [(
            0,
            TxOut {
                value: coin.value,
                script_pubkey: contract.locking_script(),
            },
        )];
        assert_eq!(verify_transaction(&claim, &spent), Ok(()));
    }

    #[test]
    fn a_claim_with_the_wrong_secret_is_rejected() {
        let buyer = TradeKey::new(
            PrivateKey::from_slice(&TRADE_BUYER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let seller = TradeKey::new(
            PrivateKey::from_slice(&TRADE_SELLER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let contract = HtlcContract::new(
            secret_hash(&TRADE_SECRET),
            buyer.pubkey_hash(),
            seller.pubkey_hash(),
            5_000,
        )
        .unwrap();
        let coin = Coin {
            outpoint: OutPoint::new(Txid::from_byte_array([0x33; 32]), 0),
            value: Amount::from_sat(40_000_000),
        };
        let mut claim = build_claim_transaction(
            &coin,
            &contract,
            &buyer,
            &TRADE_SECRET,
            &buyer.p2pkh_script(),
            0,
        )
        .unwrap();
        // The signature does not cover the unlocking script, so it stays
        // valid and only the secret is wrong.
        let signature = minimal_pushes(claim.input[0].script_sig.as_bytes()).unwrap()[0].to_vec();
        claim.input[0].script_sig =
            contract.claim_unlocking_script(&signature, buyer.public_key(), &[0x43; 32]);
        let spent = [(
            0,
            TxOut {
                value: coin.value,
                script_pubkey: contract.locking_script(),
            },
        )];
        assert_eq!(
            verify_transaction(&claim, &spent),
            Err(BchScriptError::SecretMismatch { input_index: 0 })
        );
    }

    /// `OP_SIZE <32> OP_EQUALVERIFY` refuses a secret of any other length,
    /// even one that hashes to the contract's secret hash.
    #[test]
    fn a_claim_with_a_31_byte_secret_is_rejected() {
        let buyer = TradeKey::new(
            PrivateKey::from_slice(&TRADE_BUYER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let seller = TradeKey::new(
            PrivateKey::from_slice(&TRADE_SELLER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let short_secret = [0x42u8; 31];
        let contract = HtlcContract::new(
            sha256::Hash::hash(&short_secret).to_byte_array(),
            buyer.pubkey_hash(),
            seller.pubkey_hash(),
            5_000,
        )
        .unwrap();
        let redeem_script = contract.redeem_script();
        let mut claim = Transaction {
            version: bitcoin::transaction::Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![bitcoin::TxIn {
                previous_output: OutPoint::new(Txid::from_byte_array([0x34; 32]), 0),
                script_sig: ScriptBuf::new(),
                sequence: Sequence::MAX,
                witness: Witness::new(),
            }],
            output: vec![TxOut {
                value: Amount::from_sat(40_000_000),
                script_pubkey: buyer.p2pkh_script(),
            }],
        };
        let signature = buyer
            .sign_input(&claim, 0, &redeem_script, Amount::from_sat(40_000_000))
            .unwrap();
        let mut script_sig = Vec::with_capacity(250);
        script_sig.push(signature.len() as u8);
        script_sig.extend_from_slice(&signature);
        script_sig.push(buyer.public_key().len() as u8);
        script_sig.extend_from_slice(buyer.public_key());
        script_sig.push(31);
        script_sig.extend_from_slice(&short_secret);
        script_sig.push(0x51);
        script_sig.push(0x4c);
        script_sig.push(redeem_script.len() as u8);
        script_sig.extend_from_slice(&redeem_script);
        claim.input[0].script_sig = ScriptBuf::from_bytes(script_sig);
        let spent = [(
            0,
            TxOut {
                value: Amount::from_sat(40_000_000),
                script_pubkey: contract.locking_script(),
            },
        )];
        assert_eq!(
            verify_transaction(&claim, &spent),
            Err(BchScriptError::SecretSize { input_index: 0 })
        );
    }

    #[test]
    fn a_claim_signed_by_the_refund_key_is_rejected() {
        let buyer = TradeKey::new(
            PrivateKey::from_slice(&TRADE_BUYER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let seller = TradeKey::new(
            PrivateKey::from_slice(&TRADE_SELLER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let contract = HtlcContract::new(
            secret_hash(&TRADE_SECRET),
            buyer.pubkey_hash(),
            seller.pubkey_hash(),
            5_000,
        )
        .unwrap();
        let coin = Coin {
            outpoint: OutPoint::new(Txid::from_byte_array([0x35; 32]), 0),
            value: Amount::from_sat(40_000_000),
        };
        // The seller knows the secret too, but the claim path names the
        // buyer's key.
        let mut claim = build_claim_transaction(
            &coin,
            &contract,
            &buyer,
            &TRADE_SECRET,
            &seller.p2pkh_script(),
            0,
        )
        .unwrap();
        let signature = seller
            .sign_input(&claim, 0, &contract.redeem_script(), coin.value)
            .unwrap();
        claim.input[0].script_sig =
            contract.claim_unlocking_script(&signature, seller.public_key(), &TRADE_SECRET);
        let spent = [(
            0,
            TxOut {
                value: coin.value,
                script_pubkey: contract.locking_script(),
            },
        )];
        assert_eq!(
            verify_transaction(&claim, &spent),
            Err(BchScriptError::PublicKeyHashMismatch { input_index: 0 })
        );
    }

    #[test]
    fn a_claim_with_the_claim_key_but_another_signature_is_rejected() {
        let buyer = TradeKey::new(
            PrivateKey::from_slice(&TRADE_BUYER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let seller = TradeKey::new(
            PrivateKey::from_slice(&TRADE_SELLER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let contract = HtlcContract::new(
            secret_hash(&TRADE_SECRET),
            buyer.pubkey_hash(),
            seller.pubkey_hash(),
            5_000,
        )
        .unwrap();
        let coin = Coin {
            outpoint: OutPoint::new(Txid::from_byte_array([0x36; 32]), 0),
            value: Amount::from_sat(40_000_000),
        };
        let mut claim = build_claim_transaction(
            &coin,
            &contract,
            &buyer,
            &TRADE_SECRET,
            &seller.p2pkh_script(),
            0,
        )
        .unwrap();
        let signature = seller
            .sign_input(&claim, 0, &contract.redeem_script(), coin.value)
            .unwrap();
        claim.input[0].script_sig =
            contract.claim_unlocking_script(&signature, buyer.public_key(), &TRADE_SECRET);
        let spent = [(
            0,
            TxOut {
                value: coin.value,
                script_pubkey: contract.locking_script(),
            },
        )];
        assert_eq!(
            verify_transaction(&claim, &spent),
            Err(BchScriptError::SignatureRejected { input_index: 0 })
        );
    }

    #[test]
    fn a_claim_signature_commits_to_the_outputs() {
        let buyer = TradeKey::new(
            PrivateKey::from_slice(&TRADE_BUYER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let seller = TradeKey::new(
            PrivateKey::from_slice(&TRADE_SELLER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let contract = HtlcContract::new(
            secret_hash(&TRADE_SECRET),
            buyer.pubkey_hash(),
            seller.pubkey_hash(),
            5_000,
        )
        .unwrap();
        let coin = Coin {
            outpoint: OutPoint::new(Txid::from_byte_array([0x37; 32]), 0),
            value: Amount::from_sat(40_000_000),
        };
        let mut claim = build_claim_transaction(
            &coin,
            &contract,
            &buyer,
            &TRADE_SECRET,
            &buyer.p2pkh_script(),
            0,
        )
        .unwrap();
        // Anyone who sees the secret could try to redirect the claim.
        claim.output[0].script_pubkey = seller.p2pkh_script();
        let spent = [(
            0,
            TxOut {
                value: coin.value,
                script_pubkey: contract.locking_script(),
            },
        )];
        assert_eq!(
            verify_transaction(&claim, &spent),
            Err(BchScriptError::SignatureRejected { input_index: 0 })
        );
    }

    #[test]
    fn a_refund_at_the_refund_height_verifies() {
        let buyer = TradeKey::new(
            PrivateKey::from_slice(&TRADE_BUYER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let seller = TradeKey::new(
            PrivateKey::from_slice(&TRADE_SELLER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let contract = HtlcContract::new(
            secret_hash(&TRADE_SECRET),
            buyer.pubkey_hash(),
            seller.pubkey_hash(),
            5_000,
        )
        .unwrap();
        let coin = Coin {
            outpoint: OutPoint::new(Txid::from_byte_array([0x38; 32]), 0),
            value: Amount::from_sat(40_000_000),
        };
        let refund =
            build_refund_transaction(&coin, &contract, &seller, &seller.p2pkh_script(), 0).unwrap();
        let spent = [(
            0,
            TxOut {
                value: coin.value,
                script_pubkey: contract.locking_script(),
            },
        )];
        assert_eq!(verify_transaction(&refund, &spent), Ok(()));
        assert_eq!(verify_final(&refund, 5_001), Ok(()));
    }

    #[test]
    fn an_early_refund_is_rejected() {
        let buyer = TradeKey::new(
            PrivateKey::from_slice(&TRADE_BUYER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let seller = TradeKey::new(
            PrivateKey::from_slice(&TRADE_SELLER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let contract = HtlcContract::new(
            secret_hash(&TRADE_SECRET),
            buyer.pubkey_hash(),
            seller.pubkey_hash(),
            5_000,
        )
        .unwrap();
        let coin = Coin {
            outpoint: OutPoint::new(Txid::from_byte_array([0x39; 32]), 0),
            value: Amount::from_sat(40_000_000),
        };
        let mut refund =
            build_refund_transaction(&coin, &contract, &seller, &seller.p2pkh_script(), 0).unwrap();
        refund.lock_time = LockTime::from_consensus(4_999);
        let signature = seller
            .sign_input(&refund, 0, &contract.redeem_script(), coin.value)
            .unwrap();
        refund.input[0].script_sig =
            contract.refund_unlocking_script(&signature, seller.public_key());
        let spent = [(
            0,
            TxOut {
                value: coin.value,
                script_pubkey: contract.locking_script(),
            },
        )];
        assert_eq!(
            verify_transaction(&refund, &spent),
            Err(BchScriptError::RefundTooEarly {
                input_index: 0,
                lock_time: 4_999,
                refund_height: 5_000,
            })
        );
    }

    /// A final sequence turns the lock time off, so BIP65 fails the
    /// `OP_CHECKLOCKTIMEVERIFY` instead of letting the refund through early.
    #[test]
    fn a_refund_with_a_final_sequence_is_rejected() {
        let buyer = TradeKey::new(
            PrivateKey::from_slice(&TRADE_BUYER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let seller = TradeKey::new(
            PrivateKey::from_slice(&TRADE_SELLER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let contract = HtlcContract::new(
            secret_hash(&TRADE_SECRET),
            buyer.pubkey_hash(),
            seller.pubkey_hash(),
            5_000,
        )
        .unwrap();
        let coin = Coin {
            outpoint: OutPoint::new(Txid::from_byte_array([0x3a; 32]), 0),
            value: Amount::from_sat(40_000_000),
        };
        let mut refund =
            build_refund_transaction(&coin, &contract, &seller, &seller.p2pkh_script(), 0).unwrap();
        refund.input[0].sequence = Sequence::MAX;
        let signature = seller
            .sign_input(&refund, 0, &contract.redeem_script(), coin.value)
            .unwrap();
        refund.input[0].script_sig =
            contract.refund_unlocking_script(&signature, seller.public_key());
        let spent = [(
            0,
            TxOut {
                value: coin.value,
                script_pubkey: contract.locking_script(),
            },
        )];
        assert_eq!(
            verify_transaction(&refund, &spent),
            Err(BchScriptError::FinalSequence { input_index: 0 })
        );
    }

    #[test]
    fn a_refund_with_a_time_lock_is_rejected() {
        let buyer = TradeKey::new(
            PrivateKey::from_slice(&TRADE_BUYER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let seller = TradeKey::new(
            PrivateKey::from_slice(&TRADE_SELLER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let contract = HtlcContract::new(
            secret_hash(&TRADE_SECRET),
            buyer.pubkey_hash(),
            seller.pubkey_hash(),
            5_000,
        )
        .unwrap();
        let coin = Coin {
            outpoint: OutPoint::new(Txid::from_byte_array([0x3b; 32]), 0),
            value: Amount::from_sat(40_000_000),
        };
        let mut refund =
            build_refund_transaction(&coin, &contract, &seller, &seller.p2pkh_script(), 0).unwrap();
        refund.lock_time = LockTime::from_consensus(1_800_000_000);
        let signature = seller
            .sign_input(&refund, 0, &contract.redeem_script(), coin.value)
            .unwrap();
        refund.input[0].script_sig =
            contract.refund_unlocking_script(&signature, seller.public_key());
        let spent = [(
            0,
            TxOut {
                value: coin.value,
                script_pubkey: contract.locking_script(),
            },
        )];
        assert_eq!(
            verify_transaction(&refund, &spent),
            Err(BchScriptError::LockTimeNotHeight {
                input_index: 0,
                lock_time: 1_800_000_000,
            })
        );
    }

    #[test]
    fn a_refund_signed_by_the_claim_key_is_rejected() {
        let buyer = TradeKey::new(
            PrivateKey::from_slice(&TRADE_BUYER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let seller = TradeKey::new(
            PrivateKey::from_slice(&TRADE_SELLER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let contract = HtlcContract::new(
            secret_hash(&TRADE_SECRET),
            buyer.pubkey_hash(),
            seller.pubkey_hash(),
            5_000,
        )
        .unwrap();
        let coin = Coin {
            outpoint: OutPoint::new(Txid::from_byte_array([0x3c; 32]), 0),
            value: Amount::from_sat(40_000_000),
        };
        let mut refund =
            build_refund_transaction(&coin, &contract, &seller, &buyer.p2pkh_script(), 0).unwrap();
        let signature = buyer
            .sign_input(&refund, 0, &contract.redeem_script(), coin.value)
            .unwrap();
        refund.input[0].script_sig =
            contract.refund_unlocking_script(&signature, buyer.public_key());
        let spent = [(
            0,
            TxOut {
                value: coin.value,
                script_pubkey: contract.locking_script(),
            },
        )];
        assert_eq!(
            verify_transaction(&refund, &spent),
            Err(BchScriptError::PublicKeyHashMismatch { input_index: 0 })
        );
    }

    #[test]
    fn a_redeem_script_of_another_contract_is_rejected() {
        let buyer = TradeKey::new(
            PrivateKey::from_slice(&TRADE_BUYER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let seller = TradeKey::new(
            PrivateKey::from_slice(&TRADE_SELLER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let contract = HtlcContract::new(
            secret_hash(&TRADE_SECRET),
            buyer.pubkey_hash(),
            seller.pubkey_hash(),
            5_000,
        )
        .unwrap();
        let earlier_refund = HtlcContract::new(
            secret_hash(&TRADE_SECRET),
            buyer.pubkey_hash(),
            seller.pubkey_hash(),
            100,
        )
        .unwrap();
        let coin = Coin {
            outpoint: OutPoint::new(Txid::from_byte_array([0x3d; 32]), 0),
            value: Amount::from_sat(40_000_000),
        };
        let refund =
            build_refund_transaction(&coin, &earlier_refund, &seller, &seller.p2pkh_script(), 0)
                .unwrap();
        let spent = [(
            0,
            TxOut {
                value: coin.value,
                script_pubkey: contract.locking_script(),
            },
        )];
        assert_eq!(
            verify_transaction(&refund, &spent),
            Err(BchScriptError::ScriptHashMismatch { input_index: 0 })
        );
    }

    /// Any other script behind P2SH32 is refused even when its hash matches,
    /// here a redeem script of `OP_1` that anyone could spend on Bitcoin Cash.
    #[test]
    fn a_redeem_script_outside_the_template_is_rejected() {
        let anyone_can_spend = [0x51u8];
        let spend = Transaction {
            version: bitcoin::transaction::Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![bitcoin::TxIn {
                previous_output: OutPoint::new(Txid::from_byte_array([0x3e; 32]), 0),
                // A minimal push of the one-byte redeem script: 0x51 is 81,
                // not a small number with an opcode of its own.
                script_sig: ScriptBuf::from_bytes(vec![0x01, 0x51]),
                sequence: Sequence::MAX,
                witness: Witness::new(),
            }],
            output: vec![TxOut {
                value: Amount::from_sat(40_000_000),
                script_pubkey: ScriptBuf::from_hex(BCH_TRANSFER_SCRIPT_PUBKEY).unwrap(),
            }],
        };
        let spent = [(
            0,
            TxOut {
                value: Amount::from_sat(40_000_000),
                script_pubkey: p2sh32_script(
                    &sha256d::Hash::hash(&anyone_can_spend).to_byte_array(),
                ),
            },
        )];
        assert_eq!(
            verify_transaction(&spend, &spent),
            Err(BchScriptError::UnsupportedScript { input_index: 0 })
        );
    }

    #[test]
    fn a_claim_selector_other_than_one_is_rejected() {
        let buyer = TradeKey::new(
            PrivateKey::from_slice(&TRADE_BUYER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let seller = TradeKey::new(
            PrivateKey::from_slice(&TRADE_SELLER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let contract = HtlcContract::new(
            secret_hash(&TRADE_SECRET),
            buyer.pubkey_hash(),
            seller.pubkey_hash(),
            5_000,
        )
        .unwrap();
        let coin = Coin {
            outpoint: OutPoint::new(Txid::from_byte_array([0x3f; 32]), 0),
            value: Amount::from_sat(40_000_000),
        };
        let mut claim = build_claim_transaction(
            &coin,
            &contract,
            &buyer,
            &TRADE_SECRET,
            &buyer.p2pkh_script(),
            0,
        )
        .unwrap();
        // OP_1 becomes OP_2: truthy in a bare OP_IF, but not Bitcoin Cash's
        // minimal-if form, and not the template.
        let mut script_sig = claim.input[0].script_sig.to_bytes();
        let selector_index = script_sig.len() - contract.redeem_script().len() - 3;
        assert_eq!(script_sig[selector_index], 0x51);
        script_sig[selector_index] = 0x52;
        claim.input[0].script_sig = ScriptBuf::from_bytes(script_sig);
        let spent = [(
            0,
            TxOut {
                value: coin.value,
                script_pubkey: contract.locking_script(),
            },
        )];
        assert_eq!(
            verify_transaction(&claim, &spent),
            Err(BchScriptError::UnsupportedScript { input_index: 0 })
        );
    }

    #[test]
    fn a_claim_with_an_extra_push_is_rejected() {
        let buyer = TradeKey::new(
            PrivateKey::from_slice(&TRADE_BUYER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let seller = TradeKey::new(
            PrivateKey::from_slice(&TRADE_SELLER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let contract = HtlcContract::new(
            secret_hash(&TRADE_SECRET),
            buyer.pubkey_hash(),
            seller.pubkey_hash(),
            5_000,
        )
        .unwrap();
        let coin = Coin {
            outpoint: OutPoint::new(Txid::from_byte_array([0x40; 32]), 0),
            value: Amount::from_sat(40_000_000),
        };
        let mut claim = build_claim_transaction(
            &coin,
            &contract,
            &buyer,
            &TRADE_SECRET,
            &buyer.p2pkh_script(),
            0,
        )
        .unwrap();
        // An extra element would be left on the stack, which Bitcoin Cash's
        // clean-stack rule rejects.
        let mut script_sig = vec![0x00];
        script_sig.extend_from_slice(claim.input[0].script_sig.as_bytes());
        claim.input[0].script_sig = ScriptBuf::from_bytes(script_sig);
        let spent = [(
            0,
            TxOut {
                value: coin.value,
                script_pubkey: contract.locking_script(),
            },
        )];
        assert_eq!(
            verify_transaction(&claim, &spent),
            Err(BchScriptError::UnsupportedScript { input_index: 0 })
        );
    }

    #[test]
    fn a_contract_behind_p2sh20_is_rejected() {
        let buyer = TradeKey::new(
            PrivateKey::from_slice(&TRADE_BUYER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let seller = TradeKey::new(
            PrivateKey::from_slice(&TRADE_SELLER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let contract = HtlcContract::new(
            secret_hash(&TRADE_SECRET),
            buyer.pubkey_hash(),
            seller.pubkey_hash(),
            5_000,
        )
        .unwrap();
        let coin = Coin {
            outpoint: OutPoint::new(Txid::from_byte_array([0x41; 32]), 0),
            value: Amount::from_sat(40_000_000),
        };
        let claim = build_claim_transaction(
            &coin,
            &contract,
            &buyer,
            &TRADE_SECRET,
            &buyer.p2pkh_script(),
            0,
        )
        .unwrap();
        let p2sh20 = ScriptBuf::new_p2sh(&bitcoin::ScriptHash::hash(&contract.redeem_script()));
        let spent = [(
            0,
            TxOut {
                value: coin.value,
                script_pubkey: p2sh20,
            },
        )];
        assert_eq!(
            verify_transaction(&claim, &spent),
            Err(BchScriptError::UnsupportedScript { input_index: 0 })
        );
    }

    #[test]
    fn dust_thresholds_follow_bchn() {
        let p2pkh = TxOut {
            value: Amount::ZERO,
            script_pubkey: ScriptBuf::from_hex(BCH_TRANSFER_SCRIPT_PUBKEY).unwrap(),
        };
        let p2sh32 = TxOut {
            value: Amount::ZERO,
            script_pubkey: p2sh32_script(&[0x11; 32]),
        };
        let p2sh20 = TxOut {
            value: Amount::ZERO,
            script_pubkey: ScriptBuf::from_hex("a9147b00ca78ec9935f4794bf1b1af2c08f2e33dfc7487")
                .unwrap(),
        };
        assert_eq!(dust_threshold(&p2pkh), 546);
        assert_eq!(dust_threshold(&p2sh32), 576);
        assert_eq!(dust_threshold(&p2sh20), 540);
    }

    #[test]
    fn a_p2pkh_output_at_the_dust_threshold_is_accepted_and_one_below_is_not() {
        let mut transaction = Transaction {
            version: bitcoin::transaction::Version::TWO,
            lock_time: LockTime::ZERO,
            input: Vec::new(),
            output: vec![TxOut {
                value: Amount::from_sat(546),
                script_pubkey: ScriptBuf::from_hex(BCH_TRANSFER_SCRIPT_PUBKEY).unwrap(),
            }],
        };
        assert_eq!(verify_outputs(&transaction), Ok(()));
        transaction.output[0].value = Amount::from_sat(545);
        assert_eq!(
            verify_outputs(&transaction),
            Err(BchScriptError::DustOutput {
                output_index: 0,
                value: 545,
                threshold: 546,
            })
        );
    }

    #[test]
    fn a_p2sh32_output_below_576_is_dust() {
        let transaction = Transaction {
            version: bitcoin::transaction::Version::TWO,
            lock_time: LockTime::ZERO,
            input: Vec::new(),
            output: vec![
                TxOut {
                    value: Amount::from_sat(576),
                    script_pubkey: p2sh32_script(&[0x11; 32]),
                },
                TxOut {
                    value: Amount::from_sat(575),
                    script_pubkey: p2sh32_script(&[0x22; 32]),
                },
            ],
        };
        assert_eq!(
            verify_outputs(&transaction),
            Err(BchScriptError::DustOutput {
                output_index: 1,
                value: 575,
                threshold: 576,
            })
        );
    }

    #[test]
    fn an_op_return_output_is_rejected() {
        let transaction: Transaction = deserialize_hex(BCH_TRANSFER_SCHNORR_ALL).unwrap();
        assert_eq!(
            verify_outputs(&transaction),
            Err(BchScriptError::UnsupportedOutput { output_index: 1 })
        );
    }

    #[test]
    fn a_p2sh20_output_is_rejected() {
        let transaction = Transaction {
            version: bitcoin::transaction::Version::TWO,
            lock_time: LockTime::ZERO,
            input: Vec::new(),
            output: vec![TxOut {
                value: Amount::from_sat(100_000),
                script_pubkey: ScriptBuf::from_hex(
                    "a9147b00ca78ec9935f4794bf1b1af2c08f2e33dfc7487",
                )
                .unwrap(),
            }],
        };
        assert_eq!(
            verify_outputs(&transaction),
            Err(BchScriptError::UnsupportedOutput { output_index: 0 })
        );
    }

    #[test]
    fn a_zero_lock_time_is_final() {
        let transaction: Transaction = deserialize_hex(BCH_TRANSFER_SCHNORR_ALL).unwrap();
        assert_eq!(transaction.lock_time, LockTime::ZERO);
        assert_eq!(verify_final(&transaction, 1), Ok(()));
    }

    #[test]
    fn a_height_lock_time_is_final_only_above_it() {
        let mut transaction: Transaction = deserialize_hex(BCH_TRANSFER_SCHNORR_ALL).unwrap();
        // Input 1 has sequence 0xfffffffe, so the lock time applies.
        transaction.lock_time = LockTime::from_consensus(5_000);
        assert_eq!(
            verify_final(&transaction, 5_000),
            Err(BchScriptError::NotFinal {
                lock_time: 5_000,
                block_height: 5_000,
            })
        );
        assert_eq!(verify_final(&transaction, 5_001), Ok(()));
    }

    #[test]
    fn final_sequences_make_any_lock_time_final() {
        let mut transaction: Transaction = deserialize_hex(BCH_TRANSFER_SCHNORR_ALL).unwrap();
        transaction.lock_time = LockTime::from_consensus(5_000);
        transaction.input[1].sequence = Sequence::MAX;
        assert_eq!(verify_final(&transaction, 10), Ok(()));
    }

    #[test]
    fn a_time_lock_time_is_never_final_on_the_share_chain() {
        let mut transaction: Transaction = deserialize_hex(BCH_TRANSFER_SCHNORR_ALL).unwrap();
        transaction.lock_time = LockTime::from_consensus(1_700_000_000);
        assert_eq!(
            verify_final(&transaction, 4_000_000),
            Err(BchScriptError::NotFinal {
                lock_time: 1_700_000_000,
                block_height: 4_000_000,
            })
        );
    }

    #[test]
    fn p2pkh_p2sh20_and_p2sh32_are_bitcoin_cash_payout_scripts() {
        let p2pkh = ScriptBuf::new_p2pkh(&bitcoin::PubkeyHash::from_byte_array([1; 20]));
        let p2sh20 = ScriptBuf::new_p2sh(&bitcoin::ScriptHash::from_byte_array([2; 20]));
        let p2sh32 = p2sh32_script(&[3; 32]);
        assert!(is_bch_payout_script(p2pkh.as_bytes()));
        assert!(is_bch_payout_script(p2sh20.as_bytes()));
        assert!(is_bch_payout_script(p2sh32.as_bytes()));
    }

    #[test]
    fn witness_programs_and_data_outputs_are_not_bitcoin_cash_payout_scripts() {
        let p2wpkh = ScriptBuf::new_p2wpkh(&bitcoin::WPubkeyHash::from_byte_array([1; 20]));
        let p2wsh = ScriptBuf::new_p2wsh(&bitcoin::WScriptHash::from_byte_array([1; 32]));
        assert!(!is_bch_payout_script(p2wpkh.as_bytes()));
        assert!(!is_bch_payout_script(p2wsh.as_bytes()));
        assert!(!is_bch_payout_script(&[0x6a, 0x00]));
        assert!(!is_bch_payout_script(&[]));
    }

    #[test]
    fn dust_thresholds_follow_the_output_size() {
        assert_eq!(dust_threshold_for_script_length(25), 546);
        assert_eq!(dust_threshold_for_script_length(23), 540);
        assert_eq!(dust_threshold_for_script_length(35), 576);
        assert_eq!(output_size_for_script_length(25), 34);
        assert_eq!(output_size_for_script_length(23), 32);
        assert_eq!(output_size_for_script_length(35), 44);
    }

    // The same spends run opcode by opcode on `test_utils::bch_vm`, which
    // executes the contract's bytes the way BCHN does instead of matching
    // them as a template. Each case must get the same verdict from both.

    #[test]
    fn the_stack_machine_accepts_the_claim_the_interpreter_accepts() {
        let buyer = TradeKey::new(
            PrivateKey::from_slice(&TRADE_BUYER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let seller = TradeKey::new(
            PrivateKey::from_slice(&TRADE_SELLER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let contract = HtlcContract::new(
            secret_hash(&TRADE_SECRET),
            buyer.pubkey_hash(),
            seller.pubkey_hash(),
            5_000,
        )
        .unwrap();
        let coin = Coin {
            outpoint: OutPoint::new(Txid::from_byte_array([0x61; 32]), 0),
            value: Amount::from_sat(40_000_000),
        };
        let claim = build_claim_transaction(
            &coin,
            &contract,
            &buyer,
            &TRADE_SECRET,
            &buyer.p2pkh_script(),
            0,
        )
        .unwrap();
        let spent = TxOut {
            value: coin.value,
            script_pubkey: contract.locking_script(),
        };
        assert_eq!(run_p2sh32_spend(&claim, 0, &spent), Ok(()));
        assert_eq!(verify_transaction(&claim, &[(0, spent)]), Ok(()));
    }

    #[test]
    fn the_stack_machine_accepts_the_refund_the_interpreter_accepts() {
        let buyer = TradeKey::new(
            PrivateKey::from_slice(&TRADE_BUYER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let seller = TradeKey::new(
            PrivateKey::from_slice(&TRADE_SELLER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let contract = HtlcContract::new(
            secret_hash(&TRADE_SECRET),
            buyer.pubkey_hash(),
            seller.pubkey_hash(),
            950_000,
        )
        .unwrap();
        let coin = Coin {
            outpoint: OutPoint::new(Txid::from_byte_array([0x62; 32]), 0),
            value: Amount::from_sat(50_000),
        };
        let refund =
            build_refund_transaction(&coin, &contract, &seller, &seller.p2pkh_script(), 1).unwrap();
        let spent = TxOut {
            value: coin.value,
            script_pubkey: contract.locking_script(),
        };
        assert_eq!(run_p2sh32_spend(&refund, 0, &spent), Ok(()));
        assert_eq!(verify_transaction(&refund, &[(0, spent)]), Ok(()));
    }

    #[test]
    fn the_stack_machine_rejects_a_wrong_secret_like_the_interpreter() {
        let buyer = TradeKey::new(
            PrivateKey::from_slice(&TRADE_BUYER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let seller = TradeKey::new(
            PrivateKey::from_slice(&TRADE_SELLER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let contract = HtlcContract::new(
            secret_hash(&TRADE_SECRET),
            buyer.pubkey_hash(),
            seller.pubkey_hash(),
            5_000,
        )
        .unwrap();
        let coin = Coin {
            outpoint: OutPoint::new(Txid::from_byte_array([0x63; 32]), 0),
            value: Amount::from_sat(40_000_000),
        };
        let mut claim = build_claim_transaction(
            &coin,
            &contract,
            &buyer,
            &TRADE_SECRET,
            &buyer.p2pkh_script(),
            0,
        )
        .unwrap();
        let signature = minimal_pushes(claim.input[0].script_sig.as_bytes()).unwrap()[0].to_vec();
        claim.input[0].script_sig =
            contract.claim_unlocking_script(&signature, buyer.public_key(), &[0x43; 32]);
        let spent = TxOut {
            value: coin.value,
            script_pubkey: contract.locking_script(),
        };
        assert_eq!(
            run_p2sh32_spend(&claim, 0, &spent),
            Err("OP_EQUALVERIFY failed".to_string())
        );
        assert!(verify_transaction(&claim, &[(0, spent)]).is_err());
    }

    #[test]
    fn the_stack_machine_rejects_an_early_refund_like_the_interpreter() {
        let buyer = TradeKey::new(
            PrivateKey::from_slice(&TRADE_BUYER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let seller = TradeKey::new(
            PrivateKey::from_slice(&TRADE_SELLER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let contract = HtlcContract::new(
            secret_hash(&TRADE_SECRET),
            buyer.pubkey_hash(),
            seller.pubkey_hash(),
            5_000,
        )
        .unwrap();
        let coin = Coin {
            outpoint: OutPoint::new(Txid::from_byte_array([0x64; 32]), 0),
            value: Amount::from_sat(40_000_000),
        };
        let mut refund =
            build_refund_transaction(&coin, &contract, &seller, &seller.p2pkh_script(), 0).unwrap();
        refund.lock_time = LockTime::from_consensus(4_999);
        let signature = seller
            .sign_input(&refund, 0, &contract.redeem_script(), coin.value)
            .unwrap();
        refund.input[0].script_sig =
            contract.refund_unlocking_script(&signature, seller.public_key());
        let spent = TxOut {
            value: coin.value,
            script_pubkey: contract.locking_script(),
        };
        assert_eq!(
            run_p2sh32_spend(&refund, 0, &spent),
            Err("lock time not reached".to_string())
        );
        assert!(verify_transaction(&refund, &[(0, spent)]).is_err());
    }

    #[test]
    fn the_stack_machine_rejects_a_claim_by_the_refund_key_like_the_interpreter() {
        let buyer = TradeKey::new(
            PrivateKey::from_slice(&TRADE_BUYER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let seller = TradeKey::new(
            PrivateKey::from_slice(&TRADE_SELLER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let contract = HtlcContract::new(
            secret_hash(&TRADE_SECRET),
            buyer.pubkey_hash(),
            seller.pubkey_hash(),
            5_000,
        )
        .unwrap();
        let coin = Coin {
            outpoint: OutPoint::new(Txid::from_byte_array([0x65; 32]), 0),
            value: Amount::from_sat(40_000_000),
        };
        let mut claim = build_claim_transaction(
            &coin,
            &contract,
            &buyer,
            &TRADE_SECRET,
            &seller.p2pkh_script(),
            0,
        )
        .unwrap();
        let signature = seller
            .sign_input(&claim, 0, &contract.redeem_script(), coin.value)
            .unwrap();
        claim.input[0].script_sig =
            contract.claim_unlocking_script(&signature, seller.public_key(), &TRADE_SECRET);
        let spent = TxOut {
            value: coin.value,
            script_pubkey: contract.locking_script(),
        };
        assert_eq!(
            run_p2sh32_spend(&claim, 0, &spent),
            Err("OP_EQUALVERIFY failed".to_string())
        );
        assert!(verify_transaction(&claim, &[(0, spent)]).is_err());
    }

    #[test]
    fn the_stack_machine_rejects_a_refund_with_a_final_sequence_like_the_interpreter() {
        let buyer = TradeKey::new(
            PrivateKey::from_slice(&TRADE_BUYER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let seller = TradeKey::new(
            PrivateKey::from_slice(&TRADE_SELLER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let contract = HtlcContract::new(
            secret_hash(&TRADE_SECRET),
            buyer.pubkey_hash(),
            seller.pubkey_hash(),
            5_000,
        )
        .unwrap();
        let coin = Coin {
            outpoint: OutPoint::new(Txid::from_byte_array([0x66; 32]), 0),
            value: Amount::from_sat(40_000_000),
        };
        let mut refund =
            build_refund_transaction(&coin, &contract, &seller, &seller.p2pkh_script(), 0).unwrap();
        refund.input[0].sequence = Sequence::MAX;
        let signature = seller
            .sign_input(&refund, 0, &contract.redeem_script(), coin.value)
            .unwrap();
        refund.input[0].script_sig =
            contract.refund_unlocking_script(&signature, seller.public_key());
        let spent = TxOut {
            value: coin.value,
            script_pubkey: contract.locking_script(),
        };
        assert_eq!(
            run_p2sh32_spend(&refund, 0, &spent),
            Err("final sequence".to_string())
        );
        assert!(verify_transaction(&refund, &[(0, spent)]).is_err());
    }
}
