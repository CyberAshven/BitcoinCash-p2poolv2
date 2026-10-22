// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The hash time locked contract that trades Bitcoin Cash shares for BCH.
//!
//! One redeem script serves both legs of a trade. On the share chain it locks
//! share units the buyer claims with the secret; on Bitcoin Cash it locks the
//! BCH the seller claims with the same secret. Both chains see it behind a
//! P2SH32 output (`OP_HASH256 <32-byte hash> OP_EQUAL`), so BCHN executes the
//! bytes below and the share-chain interpreter (`bch_script`) accepts exactly
//! these bytes and nothing else:
//!
//! ```text
//! OP_IF
//!     OP_SIZE <32> OP_EQUALVERIFY
//!     OP_SHA256 <secret hash> OP_EQUALVERIFY
//!     OP_DUP OP_HASH160 <claim key hash>
//! OP_ELSE
//!     <refund height> OP_CHECKLOCKTIMEVERIFY OP_DROP
//!     OP_DUP OP_HASH160 <refund key hash>
//! OP_ENDIF
//! OP_EQUALVERIFY OP_CHECKSIG
//! ```
//!
//! - The claim path takes `<signature> <public key> <secret> OP_1`. The size
//!   check pins the secret to 32 bytes, so a secret cannot be valid on one
//!   chain and too large to push on the other.
//! - The refund path takes `<signature> <public key> OP_0` in a transaction
//!   whose lock time is a block height at or above the refund height, with a
//!   non-final input sequence (BIP65). The refund height is a share height on
//!   the share chain and a BCH block height on Bitcoin Cash.
//!
//! A contract is identified by its canonical bytes: [`HtlcContract::from_redeem_script`]
//! rebuilds the script from the parameters it reads and accepts the input only
//! if the two are identical, so a non-minimal push or any changed opcode is
//! rejected rather than interpreted.

use bitcoin::hashes::{Hash, hash160, sha256, sha256d};
use bitcoin::{ScriptBuf, Transaction};

/// Length of the secret the claim path reveals.
pub const SECRET_LENGTH: usize = 32;
/// Lock times at or above this value are UNIX times rather than heights.
pub const LOCKTIME_THRESHOLD: u32 = 500_000_000;
/// Smallest refund height the template accepts. Minimal encoding would turn
/// heights 1 to 16 into `OP_1` to `OP_16`, which the template does not carry,
/// and no trade has a reason to refund that early on either chain.
pub const MIN_REFUND_HEIGHT: u32 = 17;
/// Length of a P2SH32 locking script: `OP_HASH256`, a 32-byte push, `OP_EQUAL`.
pub const P2SH32_SCRIPT_LENGTH: usize = 35;

const OP_0: u8 = 0x00;
const OP_PUSHDATA1: u8 = 0x4c;
const OP_1: u8 = 0x51;
const OP_16: u8 = 0x60;
const OP_IF: u8 = 0x63;
const OP_ELSE: u8 = 0x67;
const OP_ENDIF: u8 = 0x68;
const OP_DROP: u8 = 0x75;
const OP_DUP: u8 = 0x76;
const OP_EQUAL: u8 = 0x87;
const OP_EQUALVERIFY: u8 = 0x88;
const OP_SIZE: u8 = 0x82;
const OP_SHA256: u8 = 0xa8;
const OP_HASH160: u8 = 0xa9;
const OP_HASH256: u8 = 0xaa;
const OP_CHECKSIG: u8 = 0xac;
const OP_CHECKLOCKTIMEVERIFY: u8 = 0xb1;
/// Largest push one length opcode makes; longer pushes need `OP_PUSHDATA1`.
const MAX_DIRECT_PUSH: usize = 0x4b;
/// The values `OP_1` to `OP_16` push, so a parsed push can borrow them.
static SMALL_NUMBERS: [u8; 16] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];

/// Byte offsets of the parameters inside the canonical redeem script.
const SECRET_HASH_START: usize = 7;
const CLAIM_HASH_START: usize = 43;
const REFUND_HEIGHT_PUSH: usize = 64;
/// Bytes after the refund height's number bytes and before the refund key
/// hash: `OP_CHECKLOCKTIMEVERIFY OP_DROP OP_DUP OP_HASH160 0x14`.
const REFUND_HASH_OFFSET: usize = 5;
/// Redeem script length without the refund height's number bytes.
const FIXED_SCRIPT_LENGTH: usize = 93;
/// Largest number of bytes a refund height below [`LOCKTIME_THRESHOLD`] needs.
const MAX_HEIGHT_BYTES: usize = 4;

/// Why contract parameters cannot form a contract.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HtlcError {
    /// The refund height is below [`MIN_REFUND_HEIGHT`].
    #[error("refund height {0} is below the smallest accepted height {MIN_REFUND_HEIGHT}")]
    RefundHeightTooLow(u32),
    /// The refund height would be read as a UNIX time.
    #[error("refund height {0} is not below {LOCKTIME_THRESHOLD}, so it would be a time")]
    RefundHeightIsATime(u32),
}

/// The parameters of one hash time locked contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HtlcContract {
    /// SHA256 of the 32-byte secret.
    pub secret_hash: [u8; 32],
    /// HASH160 of the key that claims with the secret.
    pub claim_pubkey_hash: [u8; 20],
    /// HASH160 of the key that takes the coins back after the refund height.
    pub refund_pubkey_hash: [u8; 20],
    /// Block height (share height on the share chain) from which the refund
    /// path's lock time is satisfied.
    pub refund_height: u32,
}

impl HtlcContract {
    /// Build a contract, rejecting a refund height the template cannot carry.
    pub fn new(
        secret_hash: [u8; 32],
        claim_pubkey_hash: [u8; 20],
        refund_pubkey_hash: [u8; 20],
        refund_height: u32,
    ) -> Result<Self, HtlcError> {
        if refund_height < MIN_REFUND_HEIGHT {
            return Err(HtlcError::RefundHeightTooLow(refund_height));
        }
        if refund_height >= LOCKTIME_THRESHOLD {
            return Err(HtlcError::RefundHeightIsATime(refund_height));
        }
        Ok(Self {
            secret_hash,
            claim_pubkey_hash,
            refund_pubkey_hash,
            refund_height,
        })
    }

    /// The canonical redeem script.
    pub fn redeem_script(&self) -> Vec<u8> {
        let height = encode_script_number(self.refund_height);
        let mut script = Vec::with_capacity(FIXED_SCRIPT_LENGTH + height.len());
        script.extend_from_slice(&[OP_IF, OP_SIZE, 0x01, SECRET_LENGTH as u8, OP_EQUALVERIFY]);
        script.push(OP_SHA256);
        push_data(&mut script, &self.secret_hash);
        script.extend_from_slice(&[OP_EQUALVERIFY, OP_DUP, OP_HASH160]);
        push_data(&mut script, &self.claim_pubkey_hash);
        script.push(OP_ELSE);
        push_data(&mut script, &height);
        script.extend_from_slice(&[OP_CHECKLOCKTIMEVERIFY, OP_DROP, OP_DUP, OP_HASH160]);
        push_data(&mut script, &self.refund_pubkey_hash);
        script.extend_from_slice(&[OP_ENDIF, OP_EQUALVERIFY, OP_CHECKSIG]);
        script
    }

    /// HASH256 of the redeem script, the hash a P2SH32 output commits to.
    pub fn script_hash(&self) -> [u8; 32] {
        sha256d::Hash::hash(&self.redeem_script()).to_byte_array()
    }

    /// The P2SH32 locking script that holds coins under this contract.
    pub fn locking_script(&self) -> ScriptBuf {
        p2sh32_script(&self.script_hash())
    }

    /// Read a contract from its redeem script.
    ///
    /// Returns `None` unless `script` is byte for byte the canonical script of
    /// the parameters it carries.
    pub fn from_redeem_script(script: &[u8]) -> Option<Self> {
        if script.len() <= FIXED_SCRIPT_LENGTH
            || script.len() > FIXED_SCRIPT_LENGTH + MAX_HEIGHT_BYTES
        {
            return None;
        }
        let height_length = usize::from(script[REFUND_HEIGHT_PUSH]);
        if height_length != script.len() - FIXED_SCRIPT_LENGTH {
            return None;
        }
        let height_start = REFUND_HEIGHT_PUSH + 1;
        let refund_height =
            decode_script_number(&script[height_start..height_start + height_length])?;
        let refund_hash_start = height_start + height_length + REFUND_HASH_OFFSET;
        let contract = Self::new(
            script[SECRET_HASH_START..SECRET_HASH_START + 32]
                .try_into()
                .ok()?,
            script[CLAIM_HASH_START..CLAIM_HASH_START + 20]
                .try_into()
                .ok()?,
            script[refund_hash_start..refund_hash_start + 20]
                .try_into()
                .ok()?,
            refund_height,
        )
        .ok()?;
        (contract.redeem_script() == script).then_some(contract)
    }

    /// Unlocking script that claims with the secret.
    pub fn claim_unlocking_script(
        &self,
        signature: &[u8],
        public_key: &[u8],
        secret: &[u8; SECRET_LENGTH],
    ) -> ScriptBuf {
        let redeem_script = self.redeem_script();
        let mut script = Vec::with_capacity(
            signature.len() + public_key.len() + SECRET_LENGTH + redeem_script.len() + 6,
        );
        push_data(&mut script, signature);
        push_data(&mut script, public_key);
        push_data(&mut script, secret);
        script.push(OP_1);
        push_data(&mut script, &redeem_script);
        ScriptBuf::from_bytes(script)
    }

    /// Unlocking script that takes the coins back after the refund height.
    pub fn refund_unlocking_script(&self, signature: &[u8], public_key: &[u8]) -> ScriptBuf {
        let redeem_script = self.redeem_script();
        let mut script =
            Vec::with_capacity(signature.len() + public_key.len() + redeem_script.len() + 5);
        push_data(&mut script, signature);
        push_data(&mut script, public_key);
        script.push(OP_0);
        push_data(&mut script, &redeem_script);
        ScriptBuf::from_bytes(script)
    }

    /// The secret revealed by a transaction that claims this contract.
    ///
    /// Looks at every input and returns the first pushed secret whose SHA256
    /// is this contract's secret hash. The counterparty reads it from the
    /// claim on one chain to claim on the other.
    pub fn find_secret(&self, transaction: &Transaction) -> Option<[u8; SECRET_LENGTH]> {
        let redeem_script = self.redeem_script();
        transaction.input.iter().find_map(|input| {
            let pushes = minimal_pushes(input.script_sig.as_bytes())?;
            match pushes.as_slice() {
                [_signature, _public_key, secret, [1], redeem]
                    if *redeem == redeem_script.as_slice() =>
                {
                    let secret: [u8; SECRET_LENGTH] = (*secret).try_into().ok()?;
                    (sha256::Hash::hash(&secret).to_byte_array() == self.secret_hash)
                        .then_some(secret)
                }
                _ => None,
            }
        })
    }
}

/// SHA256 of a secret, the value a contract commits to.
pub fn secret_hash(secret: &[u8; SECRET_LENGTH]) -> [u8; 32] {
    sha256::Hash::hash(secret).to_byte_array()
}

/// HASH160 of a public key, the value a contract names an owner by.
pub fn pubkey_hash(public_key: &[u8]) -> [u8; 20] {
    hash160::Hash::hash(public_key).to_byte_array()
}

/// `OP_HASH256 <hash> OP_EQUAL`.
pub fn p2sh32_script(script_hash: &[u8; 32]) -> ScriptBuf {
    let mut script = Vec::with_capacity(P2SH32_SCRIPT_LENGTH);
    script.push(OP_HASH256);
    push_data(&mut script, script_hash);
    script.push(OP_EQUAL);
    ScriptBuf::from_bytes(script)
}

/// The 32-byte hash of a P2SH32 locking script, or `None` for any other script.
pub fn p2sh32_hash(script: &[u8]) -> Option<&[u8; 32]> {
    if script.len() != P2SH32_SCRIPT_LENGTH
        || script[0] != OP_HASH256
        || script[1] != 32
        || script[34] != OP_EQUAL
    {
        return None;
    }
    script[2..34].try_into().ok()
}

/// Split a push-only script into the data each opcode pushes, or `None` if
/// it holds anything else or any push is not minimal.
///
/// The rules are Bitcoin Cash's push-only and minimal-data rules: an empty
/// push is `OP_0`, a one-byte value from 1 to 16 is `OP_1` to `OP_16`, data
/// up to 75 bytes uses its length opcode and up to 255 bytes `OP_PUSHDATA1`.
/// `OP_1NEGATE`, `OP_PUSHDATA2`, `OP_PUSHDATA4` and every other opcode are
/// rejected because no accepted spend needs them.
pub(crate) fn minimal_pushes(script: &[u8]) -> Option<Vec<&[u8]>> {
    let mut pushes = Vec::with_capacity(6);
    let mut rest = script;
    while let Some((&opcode, after_opcode)) = rest.split_first() {
        let (data, remaining): (&[u8], &[u8]) = match opcode {
            OP_0 => (&[], after_opcode),
            OP_1..=OP_16 => (
                std::slice::from_ref(&SMALL_NUMBERS[usize::from(opcode - OP_1)]),
                after_opcode,
            ),
            0x01..=0x4b => {
                let length = usize::from(opcode);
                if after_opcode.len() < length {
                    return None;
                }
                let (data, remaining) = after_opcode.split_at(length);
                if length == 1 && (1..=16).contains(&data[0]) {
                    return None;
                }
                if length == 1 && data[0] == 0x81 {
                    return None;
                }
                (data, remaining)
            }
            OP_PUSHDATA1 => {
                let (&length, after_length) = after_opcode.split_first()?;
                let length = usize::from(length);
                if length <= MAX_DIRECT_PUSH || after_length.len() < length {
                    return None;
                }
                after_length.split_at(length)
            }
            _ => return None,
        };
        pushes.push(data);
        rest = remaining;
    }
    Some(pushes)
}

/// Append a minimal push of `data`, which must be 1 to 255 bytes.
fn push_data(script: &mut Vec<u8>, data: &[u8]) {
    debug_assert!(!data.is_empty() && data.len() <= usize::from(u8::MAX));
    if data.len() > MAX_DIRECT_PUSH {
        script.push(OP_PUSHDATA1);
    }
    script.push(data.len() as u8);
    script.extend_from_slice(data);
}

/// Minimal little-endian script number for a positive value.
///
/// A value whose top byte has its high bit set gets an extra zero byte, so
/// the number stays positive.
pub(crate) fn encode_script_number(value: u32) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(MAX_HEIGHT_BYTES + 1);
    let mut remaining = value;
    while remaining > 0 {
        bytes.push((remaining & 0xff) as u8);
        remaining >>= 8;
    }
    if bytes.last().is_some_and(|top| top & 0x80 != 0) {
        bytes.push(0x00);
    }
    bytes
}

/// Read a positive script number of 1 to 4 bytes.
///
/// Returns `None` for a negative number. Minimality is not checked here:
/// callers compare against the canonical encoding instead.
fn decode_script_number(bytes: &[u8]) -> Option<u32> {
    let (&top, _) = bytes.split_last()?;
    if bytes.len() > MAX_HEIGHT_BYTES || top & 0x80 != 0 {
        return None;
    }
    Some(
        bytes
            .iter()
            .rev()
            .fold(0u32, |value, byte| (value << 8) | u32::from(*byte)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::hex::{DisplayHex, FromHex};

    const SECRET: [u8; 32] = [0x5e; 32];
    const CLAIM_HASH: [u8; 20] = [0xc1; 20];
    const REFUND_HASH: [u8; 20] = [0xf2; 20];

    #[test]
    fn redeem_script_has_the_documented_bytes() {
        let contract =
            HtlcContract::new(secret_hash(&SECRET), CLAIM_HASH, REFUND_HASH, 1_000_000).unwrap();
        let expected = format!(
            "6382012088a820{}8876a914{}67{}b17576a914{}6888ac",
            secret_hash(&SECRET).to_lower_hex_string(),
            CLAIM_HASH.to_lower_hex_string(),
            "0340420f",
            REFUND_HASH.to_lower_hex_string(),
        );
        assert_eq!(contract.redeem_script().to_lower_hex_string(), expected);
        assert_eq!(contract.redeem_script().len(), 96);
    }

    #[test]
    fn locking_script_is_p2sh32_of_the_double_sha256_of_the_redeem_script() {
        let contract =
            HtlcContract::new(secret_hash(&SECRET), CLAIM_HASH, REFUND_HASH, 1_000_000).unwrap();
        let script = contract.locking_script();
        assert_eq!(script.len(), P2SH32_SCRIPT_LENGTH);
        assert_eq!(script.as_bytes()[0], OP_HASH256);
        assert_eq!(script.as_bytes()[1], 32);
        assert_eq!(script.as_bytes()[34], OP_EQUAL);
        assert_eq!(
            p2sh32_hash(script.as_bytes()),
            Some(&sha256d::Hash::hash(&contract.redeem_script()).to_byte_array())
        );
    }

    #[test]
    fn a_redeem_script_reads_back_as_its_contract() {
        let contract =
            HtlcContract::new(secret_hash(&SECRET), CLAIM_HASH, REFUND_HASH, 8_640).unwrap();
        assert_eq!(
            HtlcContract::from_redeem_script(&contract.redeem_script()),
            Some(contract)
        );
    }

    #[test]
    fn the_smallest_and_largest_refund_heights_read_back() {
        let smallest = HtlcContract::new(
            secret_hash(&SECRET),
            CLAIM_HASH,
            REFUND_HASH,
            MIN_REFUND_HEIGHT,
        )
        .unwrap();
        assert_eq!(smallest.redeem_script().len(), 94);
        assert_eq!(
            HtlcContract::from_redeem_script(&smallest.redeem_script()),
            Some(smallest)
        );
        let largest = HtlcContract::new(
            secret_hash(&SECRET),
            CLAIM_HASH,
            REFUND_HASH,
            LOCKTIME_THRESHOLD - 1,
        )
        .unwrap();
        assert_eq!(largest.redeem_script().len(), 97);
        assert_eq!(
            HtlcContract::from_redeem_script(&largest.redeem_script()),
            Some(largest)
        );
    }

    #[test]
    fn a_refund_height_below_seventeen_is_rejected() {
        assert_eq!(
            HtlcContract::new(secret_hash(&SECRET), CLAIM_HASH, REFUND_HASH, 16),
            Err(HtlcError::RefundHeightTooLow(16))
        );
    }

    #[test]
    fn a_refund_time_is_rejected() {
        assert_eq!(
            HtlcContract::new(
                secret_hash(&SECRET),
                CLAIM_HASH,
                REFUND_HASH,
                LOCKTIME_THRESHOLD
            ),
            Err(HtlcError::RefundHeightIsATime(LOCKTIME_THRESHOLD))
        );
    }

    #[test]
    fn a_changed_opcode_is_not_a_contract() {
        let contract =
            HtlcContract::new(secret_hash(&SECRET), CLAIM_HASH, REFUND_HASH, 8_640).unwrap();
        let mut script = contract.redeem_script();
        // OP_SHA256 becomes OP_HASH256: a different hash lock.
        script[5] = OP_HASH256;
        assert_eq!(HtlcContract::from_redeem_script(&script), None);
    }

    #[test]
    fn a_trailing_byte_is_not_a_contract() {
        let contract =
            HtlcContract::new(secret_hash(&SECRET), CLAIM_HASH, REFUND_HASH, 8_640).unwrap();
        let mut script = contract.redeem_script();
        script.push(OP_DROP);
        assert_eq!(HtlcContract::from_redeem_script(&script), None);
    }

    #[test]
    fn a_non_minimal_refund_height_is_not_a_contract() {
        // 8,640 is 0x21c0, minimally two bytes c0 21. Pad it with a zero byte.
        let contract =
            HtlcContract::new(secret_hash(&SECRET), CLAIM_HASH, REFUND_HASH, 8_640).unwrap();
        let canonical = contract.redeem_script();
        let mut padded = Vec::with_capacity(canonical.len() + 1);
        padded.extend_from_slice(&canonical[..REFUND_HEIGHT_PUSH]);
        padded.extend_from_slice(&[0x03, 0xc0, 0x21, 0x00]);
        padded.extend_from_slice(&canonical[REFUND_HEIGHT_PUSH + 3..]);
        assert_eq!(HtlcContract::from_redeem_script(&padded), None);
    }

    #[test]
    fn a_negative_refund_height_is_not_a_contract() {
        let contract =
            HtlcContract::new(secret_hash(&SECRET), CLAIM_HASH, REFUND_HASH, 8_640).unwrap();
        let mut script = contract.redeem_script();
        // c0 21 becomes c0 a1: the sign bit makes it -8,640.
        script[REFUND_HEIGHT_PUSH + 2] = 0xa1;
        assert_eq!(HtlcContract::from_redeem_script(&script), None);
    }

    #[test]
    fn script_numbers_keep_their_sign_bit_clear() {
        assert_eq!(encode_script_number(17), vec![0x11]);
        assert_eq!(encode_script_number(127), vec![0x7f]);
        assert_eq!(encode_script_number(128), vec![0x80, 0x00]);
        assert_eq!(encode_script_number(255), vec![0xff, 0x00]);
        assert_eq!(encode_script_number(256), vec![0x00, 0x01]);
        assert_eq!(
            encode_script_number(LOCKTIME_THRESHOLD - 1),
            vec![0xff, 0x64, 0xcd, 0x1d]
        );
        assert_eq!(decode_script_number(&[0x80, 0x00]), Some(128));
        assert_eq!(
            decode_script_number(&[0xff, 0x64, 0xcd, 0x1d]),
            Some(499_999_999)
        );
        assert_eq!(decode_script_number(&[0x80]), None);
        assert_eq!(decode_script_number(&[]), None);
    }

    #[test]
    fn claim_unlocking_script_pushes_signature_key_secret_true_and_redeem_script() {
        let contract =
            HtlcContract::new(secret_hash(&SECRET), CLAIM_HASH, REFUND_HASH, 8_640).unwrap();
        let signature = [0x30; 71];
        let public_key = [0x02; 33];
        let script = contract.claim_unlocking_script(&signature, &public_key, &SECRET);
        let redeem_script = contract.redeem_script();
        let pushes = minimal_pushes(script.as_bytes()).unwrap();
        assert_eq!(
            pushes,
            vec![
                signature.as_slice(),
                public_key.as_slice(),
                SECRET.as_slice(),
                [1u8].as_slice(),
                redeem_script.as_slice(),
            ]
        );
        // The redeem script is longer than 75 bytes, so it uses OP_PUSHDATA1.
        let redeem_push = script.len() - redeem_script.len() - 2;
        assert_eq!(script.as_bytes()[redeem_push], OP_PUSHDATA1);
    }

    #[test]
    fn refund_unlocking_script_pushes_signature_key_false_and_redeem_script() {
        let contract =
            HtlcContract::new(secret_hash(&SECRET), CLAIM_HASH, REFUND_HASH, 8_640).unwrap();
        let signature = [0x30; 71];
        let public_key = [0x03; 33];
        let script = contract.refund_unlocking_script(&signature, &public_key);
        let redeem_script = contract.redeem_script();
        assert_eq!(
            minimal_pushes(script.as_bytes()).unwrap(),
            vec![
                signature.as_slice(),
                public_key.as_slice(),
                [].as_slice(),
                redeem_script.as_slice(),
            ]
        );
    }

    #[test]
    fn a_one_byte_push_of_one_is_not_minimal() {
        assert_eq!(minimal_pushes(&[0x01, 0x01]), None);
        assert_eq!(minimal_pushes(&[OP_1]), Some(vec![[1u8].as_slice()]));
    }

    #[test]
    fn pushdata1_for_short_data_is_not_minimal() {
        let mut script = vec![OP_PUSHDATA1, 75];
        script.extend_from_slice(&[0xaa; 75]);
        assert_eq!(minimal_pushes(&script), None);
    }

    #[test]
    fn pushdata2_is_rejected() {
        // OP_PUSHDATA2 (0x4d) with a two-byte length of 256.
        let mut script = vec![0x4d, 0x00, 0x01];
        script.extend_from_slice(&[0xaa; 256]);
        assert_eq!(minimal_pushes(&script), None);
    }

    #[test]
    fn a_non_push_opcode_is_rejected() {
        assert_eq!(minimal_pushes(&[OP_1, OP_DUP]), None);
    }

    #[test]
    fn a_truncated_push_is_rejected() {
        assert_eq!(minimal_pushes(&[0x05, 0x01, 0x02]), None);
    }

    #[test]
    fn p2sh20_is_not_p2sh32() {
        let p2sh20 = Vec::from_hex("a9147b00ca78ec9935f4794bf1b1af2c08f2e33dfc7487").unwrap();
        assert_eq!(p2sh32_hash(&p2sh20), None);
    }
}
