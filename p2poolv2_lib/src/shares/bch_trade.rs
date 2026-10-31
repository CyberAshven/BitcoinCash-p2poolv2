// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Build and sign the transactions of a share trade.
//!
//! A trade has the same three transactions on each chain (see
//! `docs/atomic-swap/bch-share-trading.md`):
//!
//! - **lock** spends P2PKH coins into the contract's P2SH32 output, with
//!   change back to a P2PKH script;
//! - **claim** spends the contract with the secret to the claim key's
//!   choice of P2PKH script;
//! - **refund** spends the contract back after its refund height.
//!
//! The share chain and Bitcoin Cash sign the same replay-protected digest
//! ([`bch_sighash`]), so one builder serves both. Signatures are Bitcoin Cash
//! Schnorr ([`bch_schnorr`], the 2019-05-15 upgrade) with
//! `SIGHASH_ALL | SIGHASH_FORKID`, which BCHN and the share chain accept:
//! 65 bytes with the sighash byte, against 71 or 72 for ECDSA, and always the
//! same length, so the fee is sized exactly before signing.
//!
//! Every function takes a key its owner supplies. Nothing here creates a key
//! or a secret: keys come from the owner's wallet, and the seller picks the
//! secret.

use crate::shares::bch_htlc::{HtlcContract, SECRET_LENGTH, pubkey_hash, secret_hash};
use crate::shares::bch_schnorr::{self, SCHNORR_SIGNATURE_LENGTH};
use crate::shares::bch_script::dust_threshold;
use crate::shares::bch_sighash::{self, SighashError, SighashType};
use bitcoin::absolute::LockTime;
use bitcoin::consensus::encode::serialize;
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::Secp256k1;
use bitcoin::transaction::Version;
use bitcoin::{
    Amount, OutPoint, PrivateKey, PubkeyHash, ScriptBuf, Sequence, Transaction, TxIn, TxOut,
    Witness,
};

/// `SIGHASH_ALL | SIGHASH_FORKID`: the signature commits to every input and
/// output, as Bitcoin Cash wallets sign by default.
const SIGHASH_ALL_FORKID: u8 = 0x41;
/// A Bitcoin Cash Schnorr signature and its sighash byte. Fees are sized with
/// it before the real signature exists; a Schnorr signature always has this
/// length.
const SIGNATURE_LENGTH: usize = SCHNORR_SIGNATURE_LENGTH + 1;

/// Why a trade transaction could not be built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TradeError {
    /// A lock transaction was given no coins to spend.
    #[error("no coins to spend")]
    NoCoins,
    /// The coins do not cover the outputs and the fee.
    #[error("coins hold {available} satoshis but the outputs and fee need {required}")]
    InsufficientFunds { available: u64, required: u64 },
    /// An output would be dust on Bitcoin Cash and on the share chain.
    #[error("output of {value} is below its dust threshold {threshold}")]
    Dust { value: u64, threshold: u64 },
    /// The key is not the one the coin or contract path names.
    #[error("this key does not own the coin: the contract names another key")]
    WrongKey,
    /// The secret does not hash to the contract's secret hash.
    #[error("the secret does not match the contract's secret hash")]
    WrongSecret,
    /// Satoshi amounts overflowed.
    #[error("amounts overflow")]
    Overflow,
    /// The key is zero or not below the curve order, so it cannot sign.
    #[error("the key cannot sign")]
    InvalidKey,
    /// The signature digest could not be built.
    #[error("signature digest: {0}")]
    Sighash(#[from] SighashError),
}

/// A coin a trade transaction spends: where it is and what it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Coin {
    /// The output being spent.
    pub outpoint: OutPoint,
    /// Its value in satoshis (share units on the share chain).
    pub value: Amount,
}

/// A signing key its owner supplied, for example from a wallet's WIF export.
///
/// Deliberately not `Debug`, so it cannot end up in a log.
pub struct TradeKey {
    private_key: PrivateKey,
    public_key: Vec<u8>,
}

impl TradeKey {
    /// Wrap a key. A compressed WIF signs with a 33-byte public key and an
    /// uncompressed one with 65 bytes, so the key hash matches the wallet's.
    pub fn new(private_key: PrivateKey) -> Self {
        let public_key = private_key
            .public_key(&Secp256k1::signing_only())
            .to_bytes();
        Self {
            private_key,
            public_key,
        }
    }

    /// The SEC1 public key this key signs with.
    pub fn public_key(&self) -> &[u8] {
        &self.public_key
    }

    /// HASH160 of the public key: the hash its P2PKH script and a contract
    /// name it by.
    pub fn pubkey_hash(&self) -> [u8; 20] {
        pubkey_hash(&self.public_key)
    }

    /// The P2PKH script this key spends.
    pub fn p2pkh_script(&self) -> ScriptBuf {
        ScriptBuf::new_p2pkh(&PubkeyHash::from_byte_array(self.pubkey_hash()))
    }

    /// Sign one input with `SIGHASH_ALL | SIGHASH_FORKID`, as a Bitcoin Cash
    /// Schnorr signature.
    ///
    /// `script_code` is what the signature commits to: the P2PKH script for a
    /// P2PKH coin, the redeem script for a contract coin. Returns the 64-byte
    /// signature followed by the sighash byte, ready to push.
    pub fn sign_input(
        &self,
        transaction: &Transaction,
        input_index: usize,
        script_code: &[u8],
        value: Amount,
    ) -> Result<Vec<u8>, TradeError> {
        let sighash_type = SighashType::from_byte(SIGHASH_ALL_FORKID)?;
        let digest = bch_sighash::signature_hash(
            transaction,
            input_index,
            script_code,
            value.to_sat(),
            sighash_type,
        )?;
        let signature = bch_schnorr::sign(&self.private_key.inner.secret_bytes(), &digest)
            .ok_or(TradeError::InvalidKey)?;
        let mut bytes = Vec::with_capacity(SIGNATURE_LENGTH);
        bytes.extend_from_slice(&signature);
        bytes.push(SIGHASH_ALL_FORKID);
        Ok(bytes)
    }
}

/// Spend `key`'s P2PKH `coins` into `amount` locked under `contract`.
///
/// Output 0 is the contract's P2SH32 output; output 1, when there is one,
/// returns the change to `change_script`. The fee is `fee_per_byte` times the
/// signed size (0 by default on the share chain, which needs no fee). Change below
/// its dust threshold is left to the fee, as Bitcoin Cash wallets do; the
/// share chain needs no fee, so pick `amount` to leave no change there.
pub fn build_lock_transaction(
    coins: &[Coin],
    key: &TradeKey,
    contract: &HtlcContract,
    amount: Amount,
    change_script: &ScriptBuf,
    fee_per_byte: u64,
) -> Result<Transaction, TradeError> {
    if coins.is_empty() {
        return Err(TradeError::NoCoins);
    }
    let available = coins
        .iter()
        .try_fold(Amount::ZERO, |total, coin| total.checked_add(coin.value))
        .ok_or(TradeError::Overflow)?;
    let contract_output = TxOut {
        value: amount,
        script_pubkey: contract.locking_script(),
    };
    check_dust(&contract_output)?;

    let mut transaction = Transaction {
        version: Version::TWO,
        lock_time: LockTime::ZERO,
        input: coins
            .iter()
            .map(|coin| unsigned_input(coin.outpoint, Sequence::MAX))
            .collect(),
        output: vec![
            contract_output,
            TxOut {
                value: Amount::ZERO,
                script_pubkey: change_script.clone(),
            },
        ],
    };
    let dummy_unlock = p2pkh_unlocking_script(&[0u8; SIGNATURE_LENGTH], key.public_key());
    for input in &mut transaction.input {
        input.script_sig = dummy_unlock.clone();
    }

    let fee_with_change = fee_for(&transaction, fee_per_byte)?;
    let change = available
        .to_sat()
        .checked_sub(amount.to_sat())
        .and_then(|rest| rest.checked_sub(fee_with_change));
    let change_threshold = dust_threshold(&transaction.output[1]);
    match change {
        Some(change) if change >= change_threshold => {
            transaction.output[1].value = Amount::from_sat(change);
        }
        _ => {
            transaction.output.truncate(1);
            let fee_without_change = fee_for(&transaction, fee_per_byte)?;
            let required = amount
                .to_sat()
                .checked_add(fee_without_change)
                .ok_or(TradeError::Overflow)?;
            if available.to_sat() < required {
                return Err(TradeError::InsufficientFunds {
                    available: available.to_sat(),
                    required,
                });
            }
        }
    }

    let script_code = key.p2pkh_script();
    let signatures = coins
        .iter()
        .enumerate()
        .map(|(input_index, coin)| {
            key.sign_input(
                &transaction,
                input_index,
                script_code.as_bytes(),
                coin.value,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    for (input, signature) in transaction.input.iter_mut().zip(signatures) {
        input.script_sig = p2pkh_unlocking_script(&signature, key.public_key());
    }
    Ok(transaction)
}

/// Claim `coin`, locked under `contract`, with `secret` to `destination`.
///
/// `key` must be the contract's claim key. The input sequence is final and
/// the lock time zero, so the claim is valid at once.
pub fn build_claim_transaction(
    coin: &Coin,
    contract: &HtlcContract,
    key: &TradeKey,
    secret: &[u8; SECRET_LENGTH],
    destination: &ScriptBuf,
    fee_per_byte: u64,
) -> Result<Transaction, TradeError> {
    if key.pubkey_hash() != contract.claim_pubkey_hash {
        return Err(TradeError::WrongKey);
    }
    if secret_hash(secret) != contract.secret_hash {
        return Err(TradeError::WrongSecret);
    }
    let mut transaction = contract_spend(coin, destination, LockTime::ZERO, Sequence::MAX);
    transaction.input[0].script_sig =
        contract.claim_unlocking_script(&[0u8; SIGNATURE_LENGTH], key.public_key(), secret);
    pay_destination_after_fee(&mut transaction, coin.value, fee_per_byte)?;
    let signature = key.sign_input(&transaction, 0, &contract.redeem_script(), coin.value)?;
    transaction.input[0].script_sig =
        contract.claim_unlocking_script(&signature, key.public_key(), secret);
    Ok(transaction)
}

/// Take `coin`, locked under `contract`, back to `destination`.
///
/// `key` must be the contract's refund key. The lock time is the refund
/// height and the input sequence is not final, so the transaction is valid in
/// a block or share above the refund height and not before.
pub fn build_refund_transaction(
    coin: &Coin,
    contract: &HtlcContract,
    key: &TradeKey,
    destination: &ScriptBuf,
    fee_per_byte: u64,
) -> Result<Transaction, TradeError> {
    if key.pubkey_hash() != contract.refund_pubkey_hash {
        return Err(TradeError::WrongKey);
    }
    let lock_time = LockTime::from_consensus(contract.refund_height);
    let mut transaction = contract_spend(
        coin,
        destination,
        lock_time,
        Sequence::ENABLE_LOCKTIME_NO_RBF,
    );
    transaction.input[0].script_sig =
        contract.refund_unlocking_script(&[0u8; SIGNATURE_LENGTH], key.public_key());
    pay_destination_after_fee(&mut transaction, coin.value, fee_per_byte)?;
    let signature = key.sign_input(&transaction, 0, &contract.redeem_script(), coin.value)?;
    transaction.input[0].script_sig =
        contract.refund_unlocking_script(&signature, key.public_key());
    Ok(transaction)
}

/// `<signature> <public key>`.
fn p2pkh_unlocking_script(signature: &[u8], public_key: &[u8]) -> ScriptBuf {
    let mut script = Vec::with_capacity(signature.len() + public_key.len() + 2);
    script.push(signature.len() as u8);
    script.extend_from_slice(signature);
    script.push(public_key.len() as u8);
    script.extend_from_slice(public_key);
    ScriptBuf::from_bytes(script)
}

fn unsigned_input(outpoint: OutPoint, sequence: Sequence) -> TxIn {
    TxIn {
        previous_output: outpoint,
        script_sig: ScriptBuf::new(),
        sequence,
        witness: Witness::new(),
    }
}

/// One contract input paying one output, not yet signed or sized.
fn contract_spend(
    coin: &Coin,
    destination: &ScriptBuf,
    lock_time: LockTime,
    sequence: Sequence,
) -> Transaction {
    Transaction {
        version: Version::TWO,
        lock_time,
        input: vec![unsigned_input(coin.outpoint, sequence)],
        output: vec![TxOut {
            value: Amount::ZERO,
            script_pubkey: destination.clone(),
        }],
    }
}

/// Set the single output to the coin's value less the fee for the
/// transaction's current, placeholder-signed size.
fn pay_destination_after_fee(
    transaction: &mut Transaction,
    value: Amount,
    fee_per_byte: u64,
) -> Result<(), TradeError> {
    let fee = fee_for(transaction, fee_per_byte)?;
    let paid = value
        .to_sat()
        .checked_sub(fee)
        .ok_or(TradeError::InsufficientFunds {
            available: value.to_sat(),
            required: fee,
        })?;
    transaction.output[0].value = Amount::from_sat(paid);
    check_dust(&transaction.output[0])
}

fn fee_for(transaction: &Transaction, fee_per_byte: u64) -> Result<u64, TradeError> {
    (serialize(transaction).len() as u64)
        .checked_mul(fee_per_byte)
        .ok_or(TradeError::Overflow)
}

fn check_dust(output: &TxOut) -> Result<(), TradeError> {
    let threshold = dust_threshold(output);
    let value = output.value.to_sat();
    if value < threshold {
        return Err(TradeError::Dust { value, threshold });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shares::bch_script::{verify_outputs, verify_transaction};
    use crate::test_utils::{TRADE_BUYER_SECRET_KEY, TRADE_SECRET, TRADE_SELLER_SECRET_KEY};
    use bitcoin::{NetworkKind, Txid};

    fn seller() -> TradeKey {
        TradeKey::new(PrivateKey::from_slice(&TRADE_SELLER_SECRET_KEY, NetworkKind::Test).unwrap())
    }

    fn buyer() -> TradeKey {
        TradeKey::new(PrivateKey::from_slice(&TRADE_BUYER_SECRET_KEY, NetworkKind::Test).unwrap())
    }

    #[test]
    fn a_lock_pays_the_contract_first_and_returns_the_change() {
        let seller = seller();
        let contract = HtlcContract::new(
            secret_hash(&TRADE_SECRET),
            buyer().pubkey_hash(),
            seller.pubkey_hash(),
            5_000,
        )
        .unwrap();
        let coin = Coin {
            outpoint: OutPoint::new(Txid::from_byte_array([0x01; 32]), 0),
            value: Amount::from_int_btc(1),
        };
        let transaction = build_lock_transaction(
            &[coin],
            &seller,
            &contract,
            Amount::from_sat(40_000_000),
            &seller.p2pkh_script(),
            0,
        )
        .unwrap();

        assert_eq!(transaction.output.len(), 2);
        assert_eq!(
            transaction.output[0].script_pubkey,
            contract.locking_script()
        );
        assert_eq!(transaction.output[0].value, Amount::from_sat(40_000_000));
        assert_eq!(transaction.output[1].script_pubkey, seller.p2pkh_script());
        assert_eq!(transaction.output[1].value, Amount::from_sat(60_000_000));
        let spent = [(
            0,
            TxOut {
                value: coin.value,
                script_pubkey: seller.p2pkh_script(),
            },
        )];
        assert_eq!(verify_transaction(&transaction, &spent), Ok(()));
        assert_eq!(verify_outputs(&transaction), Ok(()));
    }

    #[test]
    fn a_lock_fee_is_the_rate_times_the_signed_size() {
        let buyer = buyer();
        let contract = HtlcContract::new(
            secret_hash(&TRADE_SECRET),
            seller().pubkey_hash(),
            buyer.pubkey_hash(),
            950_000,
        )
        .unwrap();
        let coin = Coin {
            outpoint: OutPoint::new(Txid::from_byte_array([0x02; 32]), 1),
            value: Amount::from_sat(100_000),
        };
        let transaction = build_lock_transaction(
            &[coin],
            &buyer,
            &contract,
            Amount::from_sat(50_000),
            &buyer.p2pkh_script(),
            1,
        )
        .unwrap();

        let fee =
            100_000 - transaction.output[0].value.to_sat() - transaction.output[1].value.to_sat();
        let size = serialize(&transaction).len() as u64;
        // A Schnorr signature is always 65 bytes with its sighash byte, so the
        // fee sized before signing pays for exactly the signed size.
        assert_eq!(fee, size);
    }

    #[test]
    fn change_below_dust_is_left_to_the_fee() {
        let buyer = buyer();
        let contract = HtlcContract::new(
            secret_hash(&TRADE_SECRET),
            seller().pubkey_hash(),
            buyer.pubkey_hash(),
            950_000,
        )
        .unwrap();
        let coin = Coin {
            outpoint: OutPoint::new(Txid::from_byte_array([0x03; 32]), 0),
            value: Amount::from_sat(10_500),
        };
        let transaction = build_lock_transaction(
            &[coin],
            &buyer,
            &contract,
            Amount::from_sat(10_000),
            &buyer.p2pkh_script(),
            1,
        )
        .unwrap();
        assert_eq!(transaction.output.len(), 1);
        assert_eq!(transaction.output[0].value, Amount::from_sat(10_000));
    }

    #[test]
    fn a_lock_below_the_p2sh32_dust_threshold_is_refused() {
        let seller = seller();
        let contract = HtlcContract::new(
            secret_hash(&TRADE_SECRET),
            buyer().pubkey_hash(),
            seller.pubkey_hash(),
            5_000,
        )
        .unwrap();
        let coin = Coin {
            outpoint: OutPoint::new(Txid::from_byte_array([0x04; 32]), 0),
            value: Amount::from_int_btc(1),
        };
        assert_eq!(
            build_lock_transaction(
                &[coin],
                &seller,
                &contract,
                Amount::from_sat(575),
                &seller.p2pkh_script(),
                0,
            ),
            Err(TradeError::Dust {
                value: 575,
                threshold: 576,
            })
        );
    }

    #[test]
    fn a_lock_without_enough_coins_is_refused() {
        let seller = seller();
        let contract = HtlcContract::new(
            secret_hash(&TRADE_SECRET),
            buyer().pubkey_hash(),
            seller.pubkey_hash(),
            5_000,
        )
        .unwrap();
        let coin = Coin {
            outpoint: OutPoint::new(Txid::from_byte_array([0x05; 32]), 0),
            value: Amount::from_sat(1_000),
        };
        assert_eq!(
            build_lock_transaction(
                &[coin],
                &seller,
                &contract,
                Amount::from_sat(2_000),
                &seller.p2pkh_script(),
                0,
            ),
            Err(TradeError::InsufficientFunds {
                available: 1_000,
                required: 2_000,
            })
        );
    }

    #[test]
    fn a_claim_with_the_wrong_secret_is_refused() {
        let buyer = buyer();
        let contract = HtlcContract::new(
            secret_hash(&TRADE_SECRET),
            buyer.pubkey_hash(),
            seller().pubkey_hash(),
            5_000,
        )
        .unwrap();
        let coin = Coin {
            outpoint: OutPoint::new(Txid::from_byte_array([0x06; 32]), 0),
            value: Amount::from_sat(40_000_000),
        };
        assert_eq!(
            build_claim_transaction(
                &coin,
                &contract,
                &buyer,
                &[0x00; SECRET_LENGTH],
                &buyer.p2pkh_script(),
                0,
            ),
            Err(TradeError::WrongSecret)
        );
    }

    #[test]
    fn a_claim_with_the_refund_key_is_refused() {
        let seller = seller();
        let contract = HtlcContract::new(
            secret_hash(&TRADE_SECRET),
            buyer().pubkey_hash(),
            seller.pubkey_hash(),
            5_000,
        )
        .unwrap();
        let coin = Coin {
            outpoint: OutPoint::new(Txid::from_byte_array([0x07; 32]), 0),
            value: Amount::from_sat(40_000_000),
        };
        assert_eq!(
            build_claim_transaction(
                &coin,
                &contract,
                &seller,
                &TRADE_SECRET,
                &seller.p2pkh_script(),
                0,
            ),
            Err(TradeError::WrongKey)
        );
    }

    #[test]
    fn a_refund_carries_the_refund_height_and_a_non_final_sequence() {
        let seller = seller();
        let contract = HtlcContract::new(
            secret_hash(&TRADE_SECRET),
            buyer().pubkey_hash(),
            seller.pubkey_hash(),
            5_000,
        )
        .unwrap();
        let coin = Coin {
            outpoint: OutPoint::new(Txid::from_byte_array([0x08; 32]), 0),
            value: Amount::from_sat(40_000_000),
        };
        let transaction =
            build_refund_transaction(&coin, &contract, &seller, &seller.p2pkh_script(), 0).unwrap();
        assert_eq!(transaction.lock_time.to_consensus_u32(), 5_000);
        assert_eq!(
            transaction.input[0].sequence,
            Sequence::ENABLE_LOCKTIME_NO_RBF
        );
        assert_eq!(transaction.output[0].value, coin.value);
        let spent = [(
            0,
            TxOut {
                value: coin.value,
                script_pubkey: contract.locking_script(),
            },
        )];
        assert_eq!(verify_transaction(&transaction, &spent), Ok(()));
    }

    #[test]
    fn a_refund_with_the_claim_key_is_refused() {
        let buyer = buyer();
        let contract = HtlcContract::new(
            secret_hash(&TRADE_SECRET),
            buyer.pubkey_hash(),
            seller().pubkey_hash(),
            5_000,
        )
        .unwrap();
        let coin = Coin {
            outpoint: OutPoint::new(Txid::from_byte_array([0x09; 32]), 0),
            value: Amount::from_sat(40_000_000),
        };
        assert_eq!(
            build_refund_transaction(&coin, &contract, &buyer, &buyer.p2pkh_script(), 0),
            Err(TradeError::WrongKey)
        );
    }

    #[test]
    fn a_claim_pays_the_fee_from_the_contract_coin() {
        let seller = seller();
        let contract = HtlcContract::new(
            secret_hash(&TRADE_SECRET),
            seller.pubkey_hash(),
            buyer().pubkey_hash(),
            950_000,
        )
        .unwrap();
        let coin = Coin {
            outpoint: OutPoint::new(Txid::from_byte_array([0x0a; 32]), 0),
            value: Amount::from_sat(50_000),
        };
        let transaction = build_claim_transaction(
            &coin,
            &contract,
            &seller,
            &TRADE_SECRET,
            &seller.p2pkh_script(),
            1,
        )
        .unwrap();
        let fee = 50_000 - transaction.output[0].value.to_sat();
        let size = serialize(&transaction).len() as u64;
        assert!(fee >= size && fee <= size + 2, "fee {fee} for size {size}");
    }

    #[test]
    fn a_claim_that_leaves_dust_is_refused() {
        let seller = seller();
        let contract = HtlcContract::new(
            secret_hash(&TRADE_SECRET),
            seller.pubkey_hash(),
            buyer().pubkey_hash(),
            950_000,
        )
        .unwrap();
        let coin = Coin {
            outpoint: OutPoint::new(Txid::from_byte_array([0x0b; 32]), 0),
            value: Amount::from_sat(700),
        };
        let error = build_claim_transaction(
            &coin,
            &contract,
            &seller,
            &TRADE_SECRET,
            &seller.p2pkh_script(),
            1,
        )
        .unwrap_err();
        assert!(
            matches!(error, TradeError::Dust { threshold: 546, .. }),
            "{error}"
        );
    }

    #[test]
    fn the_counterparty_finds_the_secret_in_a_claim() {
        let seller = seller();
        let contract = HtlcContract::new(
            secret_hash(&TRADE_SECRET),
            seller.pubkey_hash(),
            buyer().pubkey_hash(),
            950_000,
        )
        .unwrap();
        let coin = Coin {
            outpoint: OutPoint::new(Txid::from_byte_array([0x0c; 32]), 0),
            value: Amount::from_sat(50_000),
        };
        let claim = build_claim_transaction(
            &coin,
            &contract,
            &seller,
            &TRADE_SECRET,
            &seller.p2pkh_script(),
            1,
        )
        .unwrap();
        assert_eq!(contract.find_secret(&claim), Some(TRADE_SECRET));
    }

    #[test]
    fn a_refund_reveals_no_secret() {
        let buyer = buyer();
        let contract = HtlcContract::new(
            secret_hash(&TRADE_SECRET),
            seller().pubkey_hash(),
            buyer.pubkey_hash(),
            950_000,
        )
        .unwrap();
        let coin = Coin {
            outpoint: OutPoint::new(Txid::from_byte_array([0x0d; 32]), 0),
            value: Amount::from_sat(50_000),
        };
        let refund =
            build_refund_transaction(&coin, &contract, &buyer, &buyer.p2pkh_script(), 1).unwrap();
        assert_eq!(contract.find_secret(&refund), None);
    }
}
