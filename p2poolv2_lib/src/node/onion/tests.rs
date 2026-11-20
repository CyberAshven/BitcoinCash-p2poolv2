// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use crate::config::{NetworkConfig, PeerNetwork};
use crate::node::base_transport;
use crate::node::connection_tracker::BlockedIps;
use crate::test_utils::TestSocksProxy;
use libp2p::core::Endpoint;
use libp2p::core::transport::PortUse;
use libp2p::tcp::Config as TcpConfig;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::time::timeout;

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

/// An onion-only node refuses an `/ip4` address and a configured `/dns4`
/// dial_peer before anything connects or resolves: neither the peer's
/// listener nor the Tor proxy's sees a connection. `OrTransport` passes a
/// refused address on to the next transport, so the refusal has to come
/// first. An onion address is still dialed.
#[tokio::test]
async fn onion_only_refuses_clearnet_addresses_before_connecting() {
    let peer = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = peer.local_addr().unwrap().port();
    let proxy = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let network = NetworkConfig {
        dial_peers: vec![format!("/dns4/localhost/tcp/{port}")],
        tor_proxy: Some(proxy.local_addr().unwrap().to_string()),
        onlynet: Some(vec![PeerNetwork::Onion]),
        ..NetworkConfig::default()
    };
    let mut transport = base_transport(TcpConfig::default(), &network, BlockedIps::default());
    let dial_opts = DialOpts {
        role: Endpoint::Dialer,
        port_use: PortUse::New,
    };

    let ip: Multiaddr = format!("/ip4/127.0.0.1/tcp/{port}").parse().unwrap();
    assert!(matches!(
        transport.dial(ip, dial_opts),
        Err(TransportError::MultiaddrNotSupported(_))
    ));
    let name: Multiaddr = format!("/dns4/localhost/tcp/{port}").parse().unwrap();
    assert!(matches!(
        transport.dial(name, dial_opts),
        Err(TransportError::MultiaddrNotSupported(_))
    ));
    let onion: Multiaddr = format!("/onion3/{TOR_PROJECT_ONION}:6884").parse().unwrap();
    assert!(transport.dial(onion, dial_opts).is_ok());

    assert!(
        timeout(Duration::from_millis(200), peer.accept())
            .await
            .is_err(),
        "no TCP connection reaches the peer"
    );
    assert!(
        timeout(Duration::from_millis(200), proxy.accept())
            .await
            .is_err(),
        "no clearnet dial reaches the proxy"
    );
}

/// With `proxy_clearnet`, IP addresses and a configured host name are
/// dialed through the Tor proxy: an IP as its bytes, an IPv6 one included,
/// and the name unresolved, for Tor's exit to resolve.
#[tokio::test]
async fn proxy_clearnet_sends_ip_and_name_dials_to_the_proxy() {
    let peer = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = peer.local_addr().unwrap().port();
    let mut proxy = TestSocksProxy::start(Some(peer.local_addr().unwrap()), true).await;
    let network = NetworkConfig {
        dial_peers: vec!["/dns4/peer.invalid/tcp/6884".to_string()],
        tor_proxy: Some(proxy.address.clone()),
        proxy_clearnet: true,
        ..NetworkConfig::default()
    };
    let mut transport = base_transport(TcpConfig::default(), &network, BlockedIps::default());
    let dial_opts = DialOpts {
        role: Endpoint::Dialer,
        port_use: PortUse::New,
    };

    let ip: Multiaddr = format!("/ip4/127.0.0.1/tcp/{port}").parse().unwrap();
    let (dialed, accepted) = tokio::join!(transport.dial(ip, dial_opts).unwrap(), peer.accept());
    dialed.unwrap();
    accepted.unwrap();
    let request = proxy.next_request().await;
    assert_eq!(request.address_type, 1, "an IPv4 address");
    assert_eq!(request.host, [127, 0, 0, 1]);
    assert_eq!(request.port, port);

    let ipv6: Multiaddr = "/ip6/2001:db8::1/tcp/6884".parse().unwrap();
    transport.dial(ipv6, dial_opts).unwrap().await.unwrap();
    let request = proxy.next_request().await;
    assert_eq!(request.address_type, 4, "an IPv6 address");
    assert_eq!(request.port, 6884);

    let name: Multiaddr = "/dns4/peer.invalid/tcp/6884".parse().unwrap();
    transport.dial(name, dial_opts).unwrap().await.unwrap();
    let request = proxy.next_request().await;
    assert_eq!(request.address_type, 3, "a name, not an address");
    assert_eq!(request.host, "peer.invalid".as_bytes());
    assert_eq!(request.port, 6884);
}
