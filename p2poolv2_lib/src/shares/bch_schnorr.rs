// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Bitcoin Cash Schnorr signing (the 2019-05-15 upgrade).
//!
//! Bitcoin Cash Schnorr is not BIP340. Following the specification
//! (<https://upgradespecs.bitcoincashnode.org/2019-05-15-schnorr/>) and
//! BCHN's libsecp256k1 (`src/secp256k1/src/modules/schnorr/schnorr_impl.h`):
//!
//! - the nonce `k` comes from RFC6979 HMAC-SHA256 over the private key and
//!   the message, with the 16-byte algorithm tag `"Schnorr+SHA256  "`
//!   appended (RFC6979 section 3.6), so it never repeats an ECDSA nonce for
//!   the same key and message;
//! - `R = kG`, and `k` is negated when `R`'s Y coordinate is not a quadratic
//!   residue (Jacobi symbol 1);
//! - `e = SHA256(x(R) || compressed(P) || m) mod n`, always with the 33-byte
//!   compressed public key, also for an uncompressed key;
//! - the signature is `x(R) || (k + e * x) mod n`, 64 bytes.
//!
//! The nonce is deterministic, as BCHN's wallet makes it, so the same key
//! and message always give the same signature. Nothing here makes a key.
//! [`crate::shares::bch_script`] verifies these signatures.

use hmac::{Hmac, Mac};
use k256::elliptic_curve::PrimeField;
use k256::elliptic_curve::ops::Reduce;
use k256::elliptic_curve::sec1::ToEncodedPoint;
use k256::{FieldBytes, FieldElement, ProjectivePoint, Scalar, U256};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

/// RFC6979 section 3.6 additional data BCHN's libsecp256k1 appends for
/// Schnorr nonces (`secp256k1_schnorr_algo16`).
const SCHNORR_ALGORITHM_TAG: &[u8; 16] = b"Schnorr+SHA256  ";

/// Length of a Bitcoin Cash Schnorr signature, without its sighash byte.
pub const SCHNORR_SIGNATURE_LENGTH: usize = 64;

/// Sign the 32-byte `digest` with the 32-byte big-endian private key
/// `secret_key`, as BCHN's `CKey::SignSchnorr` does.
///
/// `None` when `secret_key` is zero or not below the curve order, which no
/// valid key is.
pub fn sign(secret_key: &[u8; 32], digest: &[u8; 32]) -> Option<[u8; SCHNORR_SIGNATURE_LENGTH]> {
    let secret =
        Option::<Scalar>::from(Scalar::from_repr(FieldBytes::clone_from_slice(secret_key)))?;
    if bool::from(secret.is_zero()) {
        return None;
    }
    let nonce = schnorr_nonce(secret_key, digest);
    sign_with_nonce(&secret, digest, nonce)
}

/// The signature for `secret` over `digest` with the nonce `nonce`. `None`
/// only if `nonce` is zero, whose point has no coordinates.
fn sign_with_nonce(
    secret: &Scalar,
    digest: &[u8; 32],
    nonce: Scalar,
) -> Option<[u8; SCHNORR_SIGNATURE_LENGTH]> {
    let nonce_point = (ProjectivePoint::GENERATOR * nonce)
        .to_affine()
        .to_encoded_point(false);
    let (Some(nonce_x), Some(nonce_y)) = (nonce_point.x(), nonce_point.y()) else {
        return None;
    };
    let y_is_residue = Option::<FieldElement>::from(FieldElement::from_bytes(nonce_y))
        .is_some_and(|nonce_y| bool::from(nonce_y.sqrt().is_some()));
    let nonce = if y_is_residue { nonce } else { -nonce };

    let public_key = (ProjectivePoint::GENERATOR * secret)
        .to_affine()
        .to_encoded_point(true);
    let mut challenge = Sha256::new();
    challenge.update(nonce_x);
    challenge.update(public_key.as_bytes());
    challenge.update(digest);
    let challenge_scalar = <Scalar as Reduce<U256>>::reduce_bytes(&challenge.finalize());

    let signature_scalar = nonce + challenge_scalar * secret;
    let mut signature = [0u8; SCHNORR_SIGNATURE_LENGTH];
    signature[..32].copy_from_slice(nonce_x);
    signature[32..].copy_from_slice(&signature_scalar.to_bytes());
    Some(signature)
}

/// libsecp256k1's `nonce_function_rfc6979` with the Schnorr algorithm tag:
/// the first RFC6979 output that is a valid scalar.
fn schnorr_nonce(secret_key: &[u8; 32], digest: &[u8; 32]) -> Scalar {
    let mut seed = [0u8; 80];
    seed[..32].copy_from_slice(secret_key);
    seed[32..64].copy_from_slice(digest);
    seed[64..].copy_from_slice(SCHNORR_ALGORITHM_TAG);
    let mut generator = Rfc6979::new(&seed);
    loop {
        let candidate = generator.generate();
        let nonce = Option::<Scalar>::from(Scalar::from_repr(FieldBytes::from(candidate)));
        if let Some(nonce) = nonce
            && !bool::from(nonce.is_zero())
        {
            return nonce;
        }
    }
}

/// The RFC6979 HMAC-SHA256 generator (section 3.2), as libsecp256k1's
/// `secp256k1_rfc6979_hmac_sha256` runs it.
struct Rfc6979 {
    key: [u8; 32],
    value: [u8; 32],
    retry: bool,
}

impl Rfc6979 {
    /// Steps b to g of section 3.2, seeded with `seed`.
    fn new(seed: &[u8]) -> Self {
        let mut generator = Self {
            key: [0x00; 32],
            value: [0x01; 32],
            retry: false,
        };
        generator.key = generator.mac(&[&generator.value, &[0x00], seed]);
        generator.value = generator.mac(&[&generator.value]);
        generator.key = generator.mac(&[&generator.value, &[0x01], seed]);
        generator.value = generator.mac(&[&generator.value]);
        generator
    }

    /// Step h: the next 32 bytes.
    fn generate(&mut self) -> [u8; 32] {
        if self.retry {
            self.key = self.mac(&[&self.value, &[0x00]]);
            self.value = self.mac(&[&self.value]);
        }
        self.value = self.mac(&[&self.value]);
        self.retry = true;
        self.value
    }

    fn mac(&self, parts: &[&[u8]]) -> [u8; 32] {
        let mut mac = HmacSha256::new_from_slice(&self.key).expect("HMAC takes any key length");
        for part in parts {
            mac.update(part);
        }
        mac.finalize().into_bytes().into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shares::bch_script::verify_schnorr;
    use bitcoin::PrivateKey;
    use bitcoin::hashes::{Hash, sha256d};
    use bitcoin::secp256k1::Secp256k1;

    /// `Hash("Very deterministic message")` from BCHN's
    /// `src/test/key_tests.cpp`, `key_test1`: the message its deterministic
    /// signature vectors sign.
    fn deterministic_message() -> [u8; 32] {
        sha256d::Hash::hash(b"Very deterministic message").to_byte_array()
    }

    fn secret_bytes(wif: &str) -> [u8; 32] {
        PrivateKey::from_wif(wif).unwrap().inner.secret_bytes()
    }

    /// BCHN `key_tests.cpp`: `key1.SignSchnorr(hashMsg)`, `strSecret1`.
    #[test]
    fn signs_bchn_key1_deterministic_vector() {
        let signature = sign(
            &secret_bytes("5HxWvvfubhXpYYpS3tJkw6fq9jE9j18THftkZjHHfmFiWtmAbrj"),
            &deterministic_message(),
        )
        .unwrap();
        assert_eq!(
            hex::encode(signature),
            "2c56731ac2f7a7e7f11518fc7722a166b02438924ca9d8b4d111347b81d07175\
             71846de67ad3d913a8fdf9d8f3f73161a4c48ae81cb183b214765feb86e255ce"
        );
    }

    /// BCHN `key_tests.cpp`: `key1C.SignSchnorr(hashMsg)`. The compressed form
    /// of the same key signs the same bytes: the challenge always hashes the
    /// compressed public key.
    #[test]
    fn signs_bchn_key1_compressed_deterministic_vector() {
        let signature = sign(
            &secret_bytes("Kwr371tjA9u2rFSMZjTNun2PXXP3WPZu2afRHTcta6KxEUdm1vEw"),
            &deterministic_message(),
        )
        .unwrap();
        assert_eq!(
            hex::encode(signature),
            "2c56731ac2f7a7e7f11518fc7722a166b02438924ca9d8b4d111347b81d07175\
             71846de67ad3d913a8fdf9d8f3f73161a4c48ae81cb183b214765feb86e255ce"
        );
    }

    /// BCHN `key_tests.cpp`: `key2.SignSchnorr(hashMsg)`, `strSecret2`.
    #[test]
    fn signs_bchn_key2_deterministic_vector() {
        let signature = sign(
            &secret_bytes("5KC4ejrDjv152FGwP386VD1i2NYc5KkfSMyv1nGy1VGDxGHqVY3"),
            &deterministic_message(),
        )
        .unwrap();
        assert_eq!(
            hex::encode(signature),
            "e7167ae0afbba6019b4c7fcfe6de79165d555e8295bd72da1b8aa1a5b5430588\
             0517cace1bcb0cb515e2eeaffd49f1e4dd49fd72826b4b1573c84da49a38405d"
        );
    }

    #[test]
    fn a_signature_verifies_against_its_public_key() {
        let private_key =
            PrivateKey::from_wif("L3Hq7a8FEQwJkW1M2GNKDW28546Vp5miewcCzSqUD9kCAXrJdS3g").unwrap();
        let digest = deterministic_message();
        let signature = sign(&private_key.inner.secret_bytes(), &digest).unwrap();
        let public_key = private_key
            .public_key(&Secp256k1::signing_only())
            .to_bytes();

        assert!(verify_schnorr(&signature, &public_key, &digest, 0).is_ok());
    }

    #[test]
    fn a_zero_key_cannot_sign() {
        assert_eq!(sign(&[0u8; 32], &deterministic_message()), None);
    }

    #[test]
    fn a_key_at_the_curve_order_cannot_sign() {
        let order = hex::decode("fffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141")
            .unwrap();
        assert_eq!(
            sign(&order.try_into().unwrap(), &deterministic_message()),
            None
        );
    }
}
