// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use temp_env::with_vars;

/// The Stratum V2 reference implementation's key-utils vectors.
const SECRET_KEY: &str = "zmBEmPhqo3A92FkiLVvyCz6htc3e53ph3ZbD4ASqGaLjwnFLi";
const PUBLIC_KEY: &str = "9bDuixKmZqAJnrmP746n8zU1wyAQRrus7th9dxnkPg6RzQvCnan";

fn bitcoin_cash_config() -> StratumConfig<Raw> {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.chain = Chain::BitcoinCash;
    config.bitcoin_cash_network = Some(BitcoinCashNetwork::Regtest);
    config.bootstrap_address = "bchreg:qqzldt8u5mafprrw6uljppyqg30uhl97ey890dqm34".to_string();
    config
}

#[test]
fn absent_template_source_takes_templates_over_rpc() {
    let parsed = bitcoin_cash_config().parse().unwrap();
    assert_eq!(parsed.template_source, TemplateSource::Rpc);
    assert!(parsed.template_provider().is_none());
}

#[test]
fn the_sample_configuration_takes_templates_over_rpc() {
    let config = Config::load("../config.sample.toml").unwrap();
    assert_eq!(config.stratum.template_source, TemplateSource::Rpc);
    assert_eq!(config.stratum.template_provider_address, None);
}

#[test]
fn template_source_sv2_deserializes_with_its_provider_address() {
    with_vars(
        [
            ("P2POOL_STRATUM__TEMPLATE_SOURCE", Some("sv2")),
            (
                "P2POOL_STRATUM__TEMPLATE_PROVIDER_ADDRESS",
                Some("127.0.0.1:8442"),
            ),
        ],
        || {
            let config = Config::load("../config.sample.toml").unwrap();
            assert_eq!(config.stratum.template_source, TemplateSource::Sv2);
            assert_eq!(
                config.stratum.template_provider_address.as_deref(),
                Some("127.0.0.1:8442")
            );
        },
    );
}

#[test]
fn a_loopback_provider_without_a_key_is_spoken_to_unencrypted() {
    let mut config = bitcoin_cash_config();
    config.template_source = TemplateSource::Sv2;
    config.template_provider_address = Some("127.0.0.1:8442".to_string());
    let parsed = config.parse().unwrap();
    assert_eq!(
        parsed.template_provider(),
        Some(&TemplateProvider {
            address: "127.0.0.1:8442".to_string(),
            authority_public_key: None,
            network: BitcoinCashNetwork::Regtest,
            proxy: None,
        })
    );
}

#[test]
fn localhost_counts_as_a_loopback_provider() {
    let mut config = bitcoin_cash_config();
    config.template_source = TemplateSource::Sv2;
    config.template_provider_address = Some("localhost:8442".to_string());
    assert!(config.parse().unwrap().template_provider().is_some());
}

#[test]
fn a_bracketed_ipv6_loopback_provider_needs_no_key() {
    let mut config = bitcoin_cash_config();
    config.template_source = TemplateSource::Sv2;
    config.template_provider_address = Some("[::1]:8442".to_string());
    assert!(config.parse().unwrap().template_provider().is_some());
}

#[test]
fn a_remote_provider_with_its_key_pins_that_key() {
    let mut config = bitcoin_cash_config();
    config.template_source = TemplateSource::Sv2;
    config.template_provider_address = Some("node.example:8442".to_string());
    config.template_provider_authority_public_key = Some(PUBLIC_KEY.to_string());
    let parsed = config.parse().unwrap();
    let secret = SecretKey::from_slice(&base58::decode_check(SECRET_KEY).unwrap()).unwrap();
    let expected = Keypair::from_secret_key(&Secp256k1::signing_only(), &secret)
        .x_only_public_key()
        .0
        .serialize();
    assert_eq!(
        parsed.template_provider().unwrap().authority_public_key,
        Some(expected)
    );
}

#[test]
fn a_remote_provider_without_a_key_is_rejected() {
    let mut config = bitcoin_cash_config();
    config.template_source = TemplateSource::Sv2;
    config.template_provider_address = Some("192.0.2.7:8442".to_string());
    let error = config.parse().unwrap_err();
    assert!(
        error.message.contains("another computer"),
        "{}",
        error.message
    );
}

#[test]
fn a_corrupted_provider_key_is_rejected() {
    let mut config = bitcoin_cash_config();
    config.template_source = TemplateSource::Sv2;
    config.template_provider_address = Some("127.0.0.1:8442".to_string());
    // The reference implementation's bad-checksum vector.
    config.template_provider_authority_public_key =
        Some("9bDuixKmZqAJnrmP746n8zU1wyAQRrus7th9dxnkPg6RzQvCnam".to_string());
    let error = config.parse().unwrap_err();
    assert!(
        error
            .message
            .contains("template_provider_authority_public_key"),
        "{}",
        error.message
    );
}

#[test]
fn sv2_templates_without_a_provider_address_are_rejected() {
    let mut config = bitcoin_cash_config();
    config.template_source = TemplateSource::Sv2;
    let error = config.parse().unwrap_err();
    assert!(
        error.message.contains("needs template_provider_address"),
        "{}",
        error.message
    );
}

#[test]
fn a_provider_address_without_a_port_is_rejected() {
    let mut config = bitcoin_cash_config();
    config.template_source = TemplateSource::Sv2;
    config.template_provider_address = Some("127.0.0.1".to_string());
    let error = config.parse().unwrap_err();
    assert!(error.message.contains("HOST:PORT"), "{}", error.message);
}

#[test]
fn sv2_templates_on_bitcoin_are_rejected() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.template_source = TemplateSource::Sv2;
    config.template_provider_address = Some("127.0.0.1:8442".to_string());
    let error = config.parse().unwrap_err();
    assert!(error.message.contains("bitcoin-cash"), "{}", error.message);
}

#[test]
fn a_reconnect_without_host_or_port_keeps_the_provider() {
    let provider = TemplateProvider {
        address: "127.0.0.1:8442".to_string(),
        authority_public_key: None,
        network: BitcoinCashNetwork::Regtest,
        proxy: None,
    };
    assert_eq!(provider.redirected("", 0).unwrap(), provider);
}

#[test]
fn an_unencrypted_provider_may_redirect_to_another_port_on_this_computer() {
    let provider = TemplateProvider {
        address: "127.0.0.1:8442".to_string(),
        authority_public_key: None,
        network: BitcoinCashNetwork::Regtest,
        proxy: None,
    };
    assert_eq!(
        provider.redirected("::1", 8443).unwrap().address,
        "[::1]:8443"
    );
}

#[test]
fn an_unencrypted_provider_may_not_redirect_to_another_computer() {
    let provider = TemplateProvider {
        address: "127.0.0.1:8442".to_string(),
        authority_public_key: None,
        network: BitcoinCashNetwork::Regtest,
        proxy: None,
    };
    let error = provider.redirected("192.0.2.7", 8442).unwrap_err();
    assert!(
        error.message.contains("another computer"),
        "{}",
        error.message
    );
}

#[test]
fn a_pinned_provider_keeps_its_key_when_it_redirects_to_another_computer() {
    let provider = TemplateProvider {
        address: "127.0.0.1:8442".to_string(),
        authority_public_key: Some([7; 32]),
        network: BitcoinCashNetwork::Regtest,
        proxy: None,
    };
    assert_eq!(
        provider.redirected("node.example", 0).unwrap(),
        TemplateProvider {
            address: "node.example:8442".to_string(),
            authority_public_key: Some([7; 32]),
            network: BitcoinCashNetwork::Regtest,
            proxy: None,
        }
    );
}

const ONION_PROVIDER: &str = "abcdefghijklmnopqrstuvwxyz234567abcdefghijklmnopqrstuvwx.onion:3335";

#[test]
fn a_tor_provider_is_reached_through_the_proxy_with_its_key() {
    let mut config = bitcoin_cash_config();
    config.template_source = TemplateSource::Sv2;
    config.template_provider_address = Some(ONION_PROVIDER.to_string());
    config.template_provider_authority_public_key = Some(PUBLIC_KEY.to_string());
    config.template_provider_proxy = Some("127.0.0.1:9050".to_string());
    let parsed = config.parse().unwrap();
    let provider = parsed.template_provider().unwrap();
    assert_eq!(provider.address, ONION_PROVIDER);
    assert_eq!(provider.proxy.as_deref(), Some("127.0.0.1:9050"));
}

#[test]
fn a_tor_or_i2p_provider_without_a_proxy_is_rejected() {
    for address in [ONION_PROVIDER, "abcdefgh.b32.i2p:3335"] {
        let mut config = bitcoin_cash_config();
        config.template_source = TemplateSource::Sv2;
        config.template_provider_address = Some(address.to_string());
        config.template_provider_authority_public_key = Some(PUBLIC_KEY.to_string());
        let error = config.parse().unwrap_err();
        assert!(
            error
                .message
                .contains("template_provider_proxy is required"),
            "{}",
            error.message
        );
    }
}

/// The proxy carries the connection, so it is never spoken to without
/// the provider's key, a provider on this computer included.
#[test]
fn a_provider_behind_a_proxy_without_a_key_is_rejected() {
    let mut config = bitcoin_cash_config();
    config.template_source = TemplateSource::Sv2;
    config.template_provider_address = Some("127.0.0.1:8442".to_string());
    config.template_provider_proxy = Some("127.0.0.1:9050".to_string());
    let error = config.parse().unwrap_err();
    assert!(
        error.message.contains("reached through a proxy"),
        "{}",
        error.message
    );
}

#[test]
fn a_proxy_without_a_port_or_without_sv2_templates_is_rejected() {
    let mut config = bitcoin_cash_config();
    config.template_source = TemplateSource::Sv2;
    config.template_provider_address = Some(ONION_PROVIDER.to_string());
    config.template_provider_authority_public_key = Some(PUBLIC_KEY.to_string());
    config.template_provider_proxy = Some("127.0.0.1".to_string());
    let error = config.parse().unwrap_err();
    assert!(error.message.contains("HOST:PORT"), "{}", error.message);

    let mut config = bitcoin_cash_config();
    config.template_provider_proxy = Some("127.0.0.1:9050".to_string());
    let error = config.parse().unwrap_err();
    assert!(
        error.message.contains("template_source = \"sv2\""),
        "{}",
        error.message
    );
}

/// A redirect stays behind the proxy, and a provider reached directly
/// cannot send the node to a name only a proxy reaches.
#[test]
fn a_redirect_keeps_the_proxy_and_needs_one_for_a_tor_name() {
    let behind_proxy = TemplateProvider {
        address: ONION_PROVIDER.to_string(),
        authority_public_key: Some([7; 32]),
        network: BitcoinCashNetwork::Regtest,
        proxy: Some("127.0.0.1:9050".to_string()),
    };
    let redirected = behind_proxy.redirected("node.example", 8442).unwrap();
    assert_eq!(redirected.address, "node.example:8442");
    assert_eq!(redirected.proxy.as_deref(), Some("127.0.0.1:9050"));

    let direct = TemplateProvider {
        address: "node.example:8442".to_string(),
        authority_public_key: Some([7; 32]),
        network: BitcoinCashNetwork::Regtest,
        proxy: None,
    };
    let error = direct
        .redirected(ONION_PROVIDER.trim_end_matches(":3335"), 3335)
        .unwrap_err();
    assert!(
        error.message.contains("without a proxy"),
        "{}",
        error.message
    );
}

#[test]
fn template_source_rpc_light_deserializes_and_parses_on_bitcoin_cash() {
    with_vars(
        [("P2POOL_STRATUM__TEMPLATE_SOURCE", Some("rpc_light"))],
        || {
            let config = Config::load("../config.sample.toml").unwrap();
            assert_eq!(config.stratum.template_source, TemplateSource::RpcLight);
            assert!(
                config
                    .stratum
                    .parse()
                    .unwrap()
                    .template_provider()
                    .is_none()
            );
        },
    );
}

#[test]
fn rpc_light_templates_on_bitcoin_are_rejected() {
    let mut config = StratumConfig::<Raw>::new_for_test_default();
    config.template_source = TemplateSource::RpcLight;
    let error = config.parse().unwrap_err();
    assert!(error.message.contains("rpc_light"), "{}", error.message);
    assert!(error.message.contains("bitcoin-cash"), "{}", error.message);
}

#[test]
fn rpc_light_templates_with_submit_nodes_are_rejected() {
    let config: Config = config::Config::builder()
        .add_source(config::File::with_name("../config.sample.toml"))
        .add_source(config::File::from_str(
            r#"
                [stratum]
                template_source = "rpc_light"

                [[submit_nodes]]
                url = "http://bchn-a.example:8332"
                username = "a"
                password = "secret-a"
                "#,
            config::FileFormat::Toml,
        ))
        .build()
        .unwrap()
        .try_deserialize()
        .unwrap();

    let error = config.validate_submit_nodes().unwrap_err();
    assert!(error.to_string().contains("submit_nodes"), "{error}");
}

#[test]
fn a_provider_address_with_rpc_templates_is_rejected() {
    let mut config = bitcoin_cash_config();
    config.template_provider_address = Some("127.0.0.1:8442".to_string());
    let error = config.parse().unwrap_err();
    assert!(
        error.message.contains("need template_source"),
        "{}",
        error.message
    );
}
