// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;

/// Test vectors from the Stratum V2 reference implementation's
/// key-utils crate: a secret key and the public key it derives.
const SECRET_KEY: &str = "zmBEmPhqo3A92FkiLVvyCz6htc3e53ph3ZbD4ASqGaLjwnFLi";
const PUBLIC_KEY: &str = "9bDuixKmZqAJnrmP746n8zU1wyAQRrus7th9dxnkPg6RzQvCnan";

#[test]
fn coinbase_only_is_taken_from_loopback_by_default() {
    let config = StratumConfig::<Raw>::new_for_test_default()
        .parse()
        .unwrap();
    let expected: Vec<IpNet> = vec!["127.0.0.0/8".parse().unwrap(), "::1/128".parse().unwrap()];
    assert_eq!(config.sv2_coinbase_only_networks, expected);
}

#[test]
fn coinbase_only_clients_may_be_addresses_or_ranges() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.sv2_coinbase_only_from = vec!["100.64.0.9".to_string(), "10.0.0.0/8".to_string()];
    let parsed = config.parse().unwrap();
    let expected: Vec<IpNet> = vec![
        "100.64.0.9/32".parse().unwrap(),
        "10.0.0.0/8".parse().unwrap(),
    ];
    assert_eq!(parsed.sv2_coinbase_only_networks, expected);
}

#[test]
fn a_coinbase_only_client_that_is_not_an_address_is_rejected() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.sv2_coinbase_only_from = vec!["my-kth.example".to_string()];
    let error = config.parse().unwrap_err();
    assert!(
        error.message.contains("sv2_coinbase_only_from"),
        "{}",
        error.message
    );
}

#[test]
fn reference_implementation_key_pair_parses() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.sv2_port = Some(3336);
    config.sv2_authority_public_key = Some(PUBLIC_KEY.to_string());
    config.sv2_authority_secret_key = Some(SECRET_KEY.to_string());
    let authority = config.parse().unwrap().sv2_authority().unwrap();
    assert_eq!(authority.cert_validity_secs, DEFAULT_SV2_CERT_VALIDITY_SECS);
    assert!(!format!("{authority:?}").contains(&format!("{:?}", authority.secret_key)));
}

#[test]
fn a_certificate_validity_that_does_not_fit_32_bits_is_rejected() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.sv2_port = Some(3336);
    config.sv2_authority_public_key = Some(PUBLIC_KEY.to_string());
    config.sv2_authority_secret_key = Some(SECRET_KEY.to_string());
    config.sv2_cert_validity_secs = Some(u64::from(u32::MAX) + 1);
    let error = config.parse().unwrap_err();
    assert!(
        error.message.contains("sv2_cert_validity_secs"),
        "{}",
        error.message
    );
}

#[test]
fn a_zero_certificate_validity_is_rejected() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.sv2_port = Some(3336);
    config.sv2_authority_public_key = Some(PUBLIC_KEY.to_string());
    config.sv2_authority_secret_key = Some(SECRET_KEY.to_string());
    config.sv2_cert_validity_secs = Some(0);
    let error = config.parse().unwrap_err();
    assert!(
        error.message.contains("sv2_cert_validity_secs"),
        "{}",
        error.message
    );
}

#[test]
fn no_keys_means_no_noise() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.sv2_port = Some(3336);
    assert!(config.parse().unwrap().sv2_authority().is_none());
}

#[test]
fn a_public_key_alone_is_rejected() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.sv2_port = Some(3336);
    config.sv2_authority_public_key = Some(PUBLIC_KEY.to_string());
    let error = config.parse().unwrap_err();
    assert!(error.message.contains("both be set"), "{}", error.message);
}

#[test]
fn keys_without_an_sv2_port_are_rejected() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.sv2_authority_public_key = Some(PUBLIC_KEY.to_string());
    config.sv2_authority_secret_key = Some(SECRET_KEY.to_string());
    let error = config.parse().unwrap_err();
    assert!(error.message.contains("need sv2_port"), "{}", error.message);
}

#[test]
fn a_corrupted_public_key_is_rejected() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.sv2_port = Some(3336);
    // The reference implementation's bad-checksum vector.
    config.sv2_authority_public_key =
        Some("9bDuixKmZqAJnrmP746n8zU1wyAQRrus7th9dxnkPg6RzQvCnam".to_string());
    config.sv2_authority_secret_key = Some(SECRET_KEY.to_string());
    assert!(config.parse().is_err());
}

#[test]
fn a_public_key_of_another_version_is_rejected() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.sv2_port = Some(3336);
    // The reference implementation's version-12 vector.
    config.sv2_authority_public_key =
        Some("2myPhc5vkPzuC5FXNK5tee79WmP7uoLh55SxezoF8iqwF3E3rnPY".to_string());
    config.sv2_authority_secret_key = Some(SECRET_KEY.to_string());
    let error = config.parse().unwrap_err();
    assert!(error.message.contains("version 1"), "{}", error.message);
}

#[test]
fn a_secret_key_for_another_public_key_is_rejected() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.sv2_port = Some(3336);
    config.sv2_authority_public_key = Some(PUBLIC_KEY.to_string());
    // Base58Check of 32 bytes of 0x01: a valid secret for another key.
    config.sv2_authority_secret_key = Some(base58::encode_check(&[1u8; 32]));
    let error = config.parse().unwrap_err();
    assert!(error.message.contains("derives"), "{}", error.message);
}
