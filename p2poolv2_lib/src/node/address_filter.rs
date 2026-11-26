// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::onion::is_valid_onion3;
use crate::config::{NetworkConfig, PeerNetwork};
use libp2p::Multiaddr;
use libp2p::multiaddr::Protocol;

/// Returns true if an IPv4 address is globally routable.
/// Rejects loopback, private (RFC 1918), link-local, unspecified,
/// broadcast, documentation, shared address space, multicast,
/// reserved, and benchmarking ranges.
fn is_ipv4_global(ip: &std::net::Ipv4Addr) -> bool {
    let octets = ip.octets();
    let first = octets[0];
    let second = octets[1];
    // Unspecified (0.0.0.0/8)
    if first == 0 {
        return false;
    }
    // Loopback (127.0.0.0/8)
    if ip.is_loopback() {
        return false;
    }
    // Private: 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16
    if ip.is_private() {
        return false;
    }
    // Link-local (169.254.0.0/16)
    if ip.is_link_local() {
        return false;
    }
    // Broadcast (255.255.255.255)
    if ip.is_broadcast() {
        return false;
    }
    // Documentation: 192.0.2.0/24, 198.51.100.0/24, 203.0.113.0/24
    if (first == 192 && second == 0 && octets[2] == 2)
        || (first == 198 && second == 51 && octets[2] == 100)
        || (first == 203 && second == 0 && octets[2] == 113)
    {
        return false;
    }
    // Shared address space (100.64.0.0/10)
    if first == 100 && (second & 0xC0) == 64 {
        return false;
    }
    // Multicast (224.0.0.0/4)
    if (224..=239).contains(&first) {
        return false;
    }
    // Reserved for future use (240.0.0.0/4, excluding broadcast)
    if first >= 240 {
        return false;
    }
    // Benchmarking (198.18.0.0/15)
    if first == 198 && (second == 18 || second == 19) {
        return false;
    }
    true
}

/// Returns true if an IPv6 address is globally routable.
/// Rejects loopback, unspecified, link-local, unique local,
/// documentation, discard-only, and IPv4-mapped/compatible ranges.
fn is_ipv6_global(ip: &std::net::Ipv6Addr) -> bool {
    if ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() {
        return false;
    }
    let segments = ip.segments();
    // Link-local (fe80::/10)
    if (segments[0] & 0xFFC0) == 0xFE80 {
        return false;
    }
    // Unique local (fc00::/7)
    if (segments[0] & 0xFE00) == 0xFC00 {
        return false;
    }
    // Documentation (2001:db8::/32)
    if segments[0] == 0x2001 && segments[1] == 0x0DB8 {
        return false;
    }
    // Discard-only (100::/64, RFC 6666)
    if segments[0] == 0x0100 && segments[1] == 0 && segments[2] == 0 && segments[3] == 0 {
        return false;
    }
    // IPv4-mapped (::ffff:0:0/96) and IPv4-compatible (deprecated, ::/96)
    if segments[0] == 0
        && segments[1] == 0
        && segments[2] == 0
        && segments[3] == 0
        && segments[4] == 0
    {
        return false;
    }
    true
}

/// Returns true if the multiaddr contains a globally routable IP address.
/// Rejects loopback, private, link-local, and unspecified addresses.
pub fn is_routable_multiaddr(address: &Multiaddr) -> bool {
    for protocol in address.iter() {
        match protocol {
            Protocol::Ip4(ip) => return is_ipv4_global(&ip),
            Protocol::Ip6(ip) => return is_ipv6_global(&ip),
            _ => {}
        }
    }
    false
}

/// The networks the node dials peers on (`NetworkConfig::dials`): those
/// `onlynet` lists, or IPv4, IPv6 and, through a Tor proxy, onion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Reachable {
    ipv4: bool,
    ipv6: bool,
    /// Whether `onlynet` allows onion, so onion becomes reachable once the
    /// node has a proxy for it (`with_onion_proxy`).
    onion_allowed: bool,
    onion: bool,
}

impl Reachable {
    /// The networks the node dials from the start: onion only with a
    /// `tor_proxy`.
    pub(crate) fn from_config(config: &NetworkConfig) -> Self {
        Self {
            ipv4: config.dials(PeerNetwork::Ipv4),
            ipv6: config.dials(PeerNetwork::Ipv6),
            onion_allowed: config.onlynet_allows(PeerNetwork::Onion),
            onion: config.dials(PeerNetwork::Onion),
        }
    }

    /// The same networks once the node has a proxy for onion peers, as when
    /// the Tor control port has made its onion service and the node dials
    /// through the default Tor SOCKS port: onion too, unless `onlynet`
    /// leaves it out.
    pub(crate) fn with_onion_proxy(self) -> Self {
        Self {
            onion: self.onion_allowed,
            ..self
        }
    }

    /// Whether `address` is on a network the node dials, by its first
    /// component. A host name is clearnet: `/dns4` is IPv4, `/dns6` IPv6,
    /// and `/dns` or `/dnsaddr` either. No other address is dialable.
    pub(crate) fn allows(self, address: &Multiaddr) -> bool {
        match address.iter().next() {
            Some(Protocol::Ip4(_) | Protocol::Dns4(_)) => self.ipv4,
            Some(Protocol::Ip6(_) | Protocol::Dns6(_)) => self.ipv6,
            Some(Protocol::Dns(_) | Protocol::Dnsaddr(_)) => self.ipv4 || self.ipv6,
            Some(Protocol::Onion3(_)) => self.onion,
            _ => false,
        }
    }
}

/// Whether the node can reach `address`: one on a network it dials
/// (`Reachable::allows`), at a globally routable IP or a v3 onion address.
/// A node without Tor neither learns nor dials onion addresses, and an
/// onion-only node no IP.
pub(crate) fn is_reachable(address: &Multiaddr, reachable: Reachable) -> bool {
    reachable.allows(address)
        && (is_routable_multiaddr(address)
            || address.iter().any(
                |protocol| matches!(protocol, Protocol::Onion3(onion3) if is_valid_onion3(onion3.hash())),
            ))
}

/// A network group as Bitcoin Cash Node draws one (`CNetAddr::GetGroup` in
/// `src/netaddress.cpp`): the first 16 bits of an IPv4 address or the first
/// 32 bits of an IPv6 address. Addresses in one group are cheap to get in
/// bulk; many groups are not, so outgoing peers are spread over groups. An
/// onion address's group is the first 4 bits of its key, as in Bitcoin Cash
/// Node; onion keys are cheap to make, so these groups spread peers less.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum NetworkGroup {
    Ipv4([u8; 2]),
    Ipv6([u16; 2]),
    Onion(u8),
}

/// The network group of the globally routable IP address or v3 onion
/// address in `address`, or None when it has neither.
pub(crate) fn network_group(address: &Multiaddr) -> Option<NetworkGroup> {
    for protocol in address.iter() {
        match protocol {
            Protocol::Ip4(ip) => {
                let octets = ip.octets();
                return is_ipv4_global(&ip).then_some(NetworkGroup::Ipv4([octets[0], octets[1]]));
            }
            Protocol::Ip6(ip) => {
                let segments = ip.segments();
                return is_ipv6_global(&ip)
                    .then_some(NetworkGroup::Ipv6([segments[0], segments[1]]));
            }
            Protocol::Onion3(onion) => {
                let key = onion.hash();
                return is_valid_onion3(key).then_some(NetworkGroup::Onion(key[0] >> 4));
            }
            _ => {}
        }
    }
    None
}

/// Extracts the TCP port from a multiaddr string (e.g. "/ip4/0.0.0.0/tcp/6884").
/// Returns None if the string is not a valid multiaddr, has no TCP component,
/// or the port is 0 (ephemeral).
pub fn extract_listen_port(listen_address: &str) -> Option<u16> {
    let multiaddr: Multiaddr = listen_address.parse().ok()?;
    extract_tcp_port(&multiaddr)
}

/// Extracts a non-zero TCP port from a parsed multiaddr.
/// Returns None if no TCP component is present or the port is 0.
pub fn extract_tcp_port(address: &Multiaddr) -> Option<u16> {
    for protocol in address.iter() {
        if let Protocol::Tcp(port) = protocol {
            if port > 0 {
                return Some(port);
            }
            return None;
        }
    }
    None
}

/// Builds an external multiaddr by taking the IP from `observed_addr` and
/// replacing the port with `listen_port`. Returns None if the observed address
/// has no IP component or the IP is not globally routable.
pub fn build_external_address(observed_addr: &Multiaddr, listen_port: u16) -> Option<Multiaddr> {
    for protocol in observed_addr.iter() {
        match protocol {
            Protocol::Ip4(ip) => {
                if is_ipv4_global(&ip) {
                    let mut addr = Multiaddr::empty();
                    addr.push(Protocol::Ip4(ip));
                    addr.push(Protocol::Tcp(listen_port));
                    return Some(addr);
                }
                return None;
            }
            Protocol::Ip6(ip) => {
                if is_ipv6_global(&ip) {
                    let mut addr = Multiaddr::empty();
                    addr.push(Protocol::Ip6(ip));
                    addr.push(Protocol::Tcp(listen_port));
                    return Some(addr);
                }
                return None;
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rejects_loopback() {
        let addr: Multiaddr = "/ip4/127.0.0.1/tcp/6884".parse().unwrap();
        assert!(!is_routable_multiaddr(&addr));
    }

    #[test]
    fn test_rejects_private_10() {
        let addr: Multiaddr = "/ip4/10.0.0.1/tcp/6884".parse().unwrap();
        assert!(!is_routable_multiaddr(&addr));
    }

    #[test]
    fn test_rejects_private_172() {
        let addr: Multiaddr = "/ip4/172.16.0.1/tcp/6884".parse().unwrap();
        assert!(!is_routable_multiaddr(&addr));
    }

    #[test]
    fn test_rejects_private_192() {
        let addr: Multiaddr = "/ip4/192.168.1.1/tcp/6884".parse().unwrap();
        assert!(!is_routable_multiaddr(&addr));
    }

    #[test]
    fn test_rejects_unspecified() {
        let addr: Multiaddr = "/ip4/0.0.0.0/tcp/6884".parse().unwrap();
        assert!(!is_routable_multiaddr(&addr));
    }

    #[test]
    fn test_rejects_ipv6_loopback() {
        let addr: Multiaddr = "/ip6/::1/tcp/6884".parse().unwrap();
        assert!(!is_routable_multiaddr(&addr));
    }

    #[test]
    fn test_rejects_ipv6_link_local() {
        let addr: Multiaddr = "/ip6/fe80::1/tcp/6884".parse().unwrap();
        assert!(!is_routable_multiaddr(&addr));
    }

    #[test]
    fn test_rejects_ipv6_unique_local() {
        let addr: Multiaddr = "/ip6/fd00::1/tcp/6884".parse().unwrap();
        assert!(!is_routable_multiaddr(&addr));
    }

    #[test]
    fn test_rejects_ipv6_documentation() {
        let addr: Multiaddr = "/ip6/2001:db8::1/tcp/6884".parse().unwrap();
        assert!(!is_routable_multiaddr(&addr));
    }

    #[test]
    fn test_rejects_ipv6_discard_only() {
        let addr: Multiaddr = "/ip6/100::1/tcp/6884".parse().unwrap();
        assert!(!is_routable_multiaddr(&addr));
    }

    #[test]
    fn test_rejects_ipv4_mapped_ipv6() {
        let addr: Multiaddr = "/ip6/::ffff:192.0.2.1/tcp/6884".parse().unwrap();
        assert!(!is_routable_multiaddr(&addr));
    }

    #[test]
    fn test_accepts_public_ipv4() {
        let addr: Multiaddr = "/ip4/8.8.8.8/tcp/6884".parse().unwrap();
        assert!(is_routable_multiaddr(&addr));
    }

    #[test]
    fn test_accepts_public_ipv6() {
        let addr: Multiaddr = "/ip6/2001:4860:4860::8888/tcp/6884".parse().unwrap();
        assert!(is_routable_multiaddr(&addr));
    }

    #[test]
    fn test_rejects_no_ip() {
        let addr: Multiaddr = "/tcp/6884".parse().unwrap();
        assert!(!is_routable_multiaddr(&addr));
    }

    #[test]
    fn test_rejects_shared_address_space() {
        let addr: Multiaddr = "/ip4/100.64.0.1/tcp/6884".parse().unwrap();
        assert!(!is_routable_multiaddr(&addr));
    }

    #[test]
    fn test_rejects_documentation_range() {
        let addr: Multiaddr = "/ip4/192.0.2.1/tcp/6884".parse().unwrap();
        assert!(!is_routable_multiaddr(&addr));
    }

    #[test]
    fn test_rejects_multicast() {
        let addr: Multiaddr = "/ip4/224.0.0.1/tcp/6884".parse().unwrap();
        assert!(!is_routable_multiaddr(&addr));
    }

    #[test]
    fn test_rejects_reserved() {
        let addr: Multiaddr = "/ip4/240.0.0.1/tcp/6884".parse().unwrap();
        assert!(!is_routable_multiaddr(&addr));
    }

    #[test]
    fn test_rejects_benchmarking() {
        let addr: Multiaddr = "/ip4/198.18.0.1/tcp/6884".parse().unwrap();
        assert!(!is_routable_multiaddr(&addr));
    }

    #[test]
    fn test_extract_listen_port_from_valid_multiaddr() {
        assert_eq!(extract_listen_port("/ip4/0.0.0.0/tcp/6884"), Some(6884));
    }

    #[test]
    fn test_extract_listen_port_from_ipv6() {
        assert_eq!(extract_listen_port("/ip6/::0/tcp/9999"), Some(9999));
    }

    #[test]
    fn test_extract_listen_port_returns_none_for_no_tcp() {
        assert_eq!(extract_listen_port("/ip4/0.0.0.0/udp/6884"), None);
    }

    #[test]
    fn test_extract_listen_port_returns_none_for_invalid() {
        assert_eq!(extract_listen_port("not-a-multiaddr"), None);
    }

    #[test]
    fn test_extract_listen_port_returns_none_for_ephemeral() {
        assert_eq!(extract_listen_port("/ip4/0.0.0.0/tcp/0"), None);
    }

    #[test]
    fn test_extract_tcp_port_from_resolved_address() {
        let addr: Multiaddr = "/ip4/127.0.0.1/tcp/45678".parse().unwrap();
        assert_eq!(extract_tcp_port(&addr), Some(45678));
    }

    #[test]
    fn test_extract_tcp_port_returns_none_for_zero() {
        let addr: Multiaddr = "/ip4/127.0.0.1/tcp/0".parse().unwrap();
        assert_eq!(extract_tcp_port(&addr), None);
    }

    #[test]
    fn test_build_external_address_replaces_port() {
        let observed: Multiaddr = "/ip4/8.8.8.8/tcp/54321".parse().unwrap();
        let result = build_external_address(&observed, 6884);
        let expected: Multiaddr = "/ip4/8.8.8.8/tcp/6884".parse().unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn test_build_external_address_with_ipv6() {
        let observed: Multiaddr = "/ip6/2001:4860:4860::8888/tcp/54321".parse().unwrap();
        let result = build_external_address(&observed, 6884);
        let expected: Multiaddr = "/ip6/2001:4860:4860::8888/tcp/6884".parse().unwrap();
        assert_eq!(result, Some(expected));
    }

    #[test]
    fn test_build_external_address_rejects_private() {
        let observed: Multiaddr = "/ip4/192.168.1.1/tcp/54321".parse().unwrap();
        assert_eq!(build_external_address(&observed, 6884), None);
    }

    #[test]
    fn test_build_external_address_rejects_loopback() {
        let observed: Multiaddr = "/ip4/127.0.0.1/tcp/54321".parse().unwrap();
        assert_eq!(build_external_address(&observed, 6884), None);
    }

    #[test]
    fn test_network_group_of_ipv4_is_its_slash_16() {
        let first: Multiaddr = "/ip4/8.8.4.4/tcp/6884".parse().unwrap();
        let second: Multiaddr = "/ip4/8.8.200.1/tcp/6885".parse().unwrap();
        let other: Multiaddr = "/ip4/8.9.4.4/tcp/6884".parse().unwrap();
        assert_eq!(network_group(&first), Some(NetworkGroup::Ipv4([8, 8])));
        assert_eq!(network_group(&first), network_group(&second));
        assert_ne!(network_group(&first), network_group(&other));
    }

    #[test]
    fn test_network_group_of_ipv6_is_its_slash_32() {
        let first: Multiaddr = "/ip6/2001:4860:4860::8888/tcp/6884".parse().unwrap();
        let second: Multiaddr = "/ip6/2001:4860:1::1/tcp/6884".parse().unwrap();
        let other: Multiaddr = "/ip6/2001:4861::1/tcp/6884".parse().unwrap();
        assert_eq!(
            network_group(&first),
            Some(NetworkGroup::Ipv6([0x2001, 0x4860]))
        );
        assert_eq!(network_group(&first), network_group(&second));
        assert_ne!(network_group(&first), network_group(&other));
    }

    #[test]
    fn test_network_group_is_none_for_unroutable_and_dns_addresses() {
        let loopback: Multiaddr = "/ip4/127.0.0.1/tcp/6884".parse().unwrap();
        let private: Multiaddr = "/ip4/192.168.1.1/tcp/6884".parse().unwrap();
        let dns: Multiaddr = "/dns4/pool.example.com/tcp/6884".parse().unwrap();
        assert_eq!(network_group(&loopback), None);
        assert_eq!(network_group(&private), None);
        assert_eq!(network_group(&dns), None);
    }
}
