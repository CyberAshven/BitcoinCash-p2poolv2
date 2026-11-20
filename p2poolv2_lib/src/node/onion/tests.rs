// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use libp2p::core::Endpoint;
use libp2p::core::transport::PortUse;

/// The Tor Project's own v3 onion service.
const TOR_PROJECT_ONION: &str = "2gzyxa5ihm7nsggfxnu52rck2vv4rvmdlkiu3zzui5du4xyclen53wid";

#[test]
fn base32_matches_rfc_4648() {
    for (input, expected) in [
        ("", ""),
        ("f", "my"),
        ("fo", "mzxq"),
        ("foo", "mzxw6"),
        ("foob", "mzxw6yq"),
        ("fooba", "mzxw6ytb"),
        ("foobar", "mzxw6ytboi"),
    ] {
        assert_eq!(base32_lower(input.as_bytes()), expected, "{input}");
    }
}

/// The host name is the address as written, with `.onion`, whether or not a
/// `/p2p` follows.
#[test]
fn an_onion3_address_gives_its_host_and_port() {
    let expected = Some((format!("{TOR_PROJECT_ONION}.onion"), 6884));
    let bare: Multiaddr = format!("/onion3/{TOR_PROJECT_ONION}:6884").parse().unwrap();
    assert_eq!(onion3_target(&bare), expected);

    let with_peer = bare.with(Protocol::P2p(libp2p::PeerId::random()));
    assert_eq!(onion3_target(&with_peer), expected);
}

#[test]
fn an_uppercase_onion3_address_gives_a_lowercase_host() {
    let address: Multiaddr = format!("/onion3/{}:6884", TOR_PROJECT_ONION.to_uppercase())
        .parse()
        .unwrap();
    assert_eq!(
        onion3_target(&address),
        Some((format!("{TOR_PROJECT_ONION}.onion"), 6884))
    );
}

#[test]
fn other_addresses_have_no_onion_target() {
    for address in [
        "/ip4/203.0.113.1/tcp/6884",
        "/dns4/pool.example.com/tcp/6884",
        "/ip6/2001:db8::1/tcp/6884",
    ] {
        let address: Multiaddr = address.parse().unwrap();
        assert_eq!(onion3_target(&address), None, "{address}");
    }
    let onion_then_tcp: Multiaddr = format!("/onion3/{TOR_PROJECT_ONION}:6884/tcp/1")
        .parse()
        .unwrap();
    assert_eq!(onion3_target(&onion_then_tcp), None);
}

/// Only version 3 is an onion address Tor dials today.
#[test]
fn an_onion3_address_of_another_version_is_refused() {
    let mut bytes = [7u8; 35];
    bytes[34] = 2;
    let old = Multiaddr::empty().with(Protocol::Onion3((bytes, 6884).into()));
    assert_eq!(onion3_target(&old), None);

    bytes[34] = ONION3_VERSION;
    let current = Multiaddr::empty().with(Protocol::Onion3((bytes, 6884).into()));
    assert!(onion3_target(&current).is_some());
}

/// A non-onion address goes to the transport beside it, through
/// `MultiaddrNotSupported`.
#[test]
fn the_onion_transport_refuses_other_addresses() {
    let mut transport = OnionTransport::new("127.0.0.1:9050".to_string());
    let address: Multiaddr = "/ip4/203.0.113.1/tcp/6884".parse().unwrap();
    let result = transport.dial(
        address.clone(),
        DialOpts {
            role: Endpoint::Dialer,
            port_use: PortUse::New,
        },
    );
    assert!(
        matches!(result, Err(TransportError::MultiaddrNotSupported(refused)) if refused == address)
    );
}
