// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Built-in DNS seeds used to find the first peers when a node has none.
//! The node queries them once: at startup without dial_peers, or after
//! dial_peers had time to connect and none did.
//!
//! Like Bitcoin's chainparams, each network compiles in a list of seed
//! hostnames and the P2P port their nodes listen on. A seed's A/AAAA records
//! list the IPs of long-running nodes. Seeds only supply addresses to dial;
//! the noise handshake with the network protocol prologue still decides
//! whether a peer is on our network.
//!
//! Bitcoin's seeds are picked by Bitcoin network and Bitcoin Cash's by
//! Bitcoin Cash network, so a Bitcoin Cash node never gets Bitcoin's
//! (`dns_seeds_for_chain`). `network.dns_seed_hosts` replaces the built-in
//! list, so a new seed can be used before a release names it.

use super::{SwarmSend, address_filter};
use crate::config::{BitcoinCashNetwork, Chain};
use libp2p::Multiaddr;
use libp2p::multiaddr::Protocol;
use std::collections::HashSet;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::lookup_host;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tracing::{debug, info, warn};

/// Timeout for a single seed hostname lookup.
const DNS_SEED_LOOKUP_TIMEOUT_SECS: u64 = 10;

/// Time configured dial_peers get to connect before falling back to DNS
/// seeds.
const DNS_SEED_FALLBACK_DELAY_SECS: u64 = 10;

/// Maximum number of addresses taken from one seed, so a single seed
/// operator cannot fill every outbound connection slot.
const MAX_ADDRESSES_PER_SEED: usize = 16;

/// P2P port testnet4 nodes listen on.
const TESTNET4_P2P_PORT: u16 = 46884;

/// Seed hostnames for testnet4.
const TESTNET4_SEED_HOSTNAMES: &[&str] = &["seed.testnet4.p2poolv2.org"];

/// The port of every sample config's `listen_address`. Bitcoin Cash nodes
/// listen on it on every network, and so do Bitcoin nodes on a network
/// without a seed port of its own.
const DEFAULT_P2P_PORT: u16 = 6884;

/// Seed hostnames for Bitcoin Cash mainnet. Each is run by a different
/// operator; its A/AAAA records list long-running mainnet nodes on
/// `DEFAULT_P2P_PORT`.
const BITCOIN_CASH_MAINNET_SEED_HOSTNAMES: &[&str] = &[];

/// Seed hostnames for Bitcoin Cash chipnet, as for mainnet.
const BITCOIN_CASH_CHIPNET_SEED_HOSTNAMES: &[&str] = &[];

/// The DNS seeds for one network and the port their nodes listen on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DnsSeeds {
    pub(crate) hostnames: Vec<String>,
    pub(crate) port: u16,
}

impl DnsSeeds {
    /// The seeds `hostnames` on `port`, or None when there are none.
    fn new<S: AsRef<str>>(hostnames: &[S], port: u16) -> Option<Self> {
        (!hostnames.is_empty()).then(|| Self {
            hostnames: hostnames
                .iter()
                .map(|host| host.as_ref().to_string())
                .collect(),
            port,
        })
    }
}

/// Returns the built-in DNS seeds for a bitcoin network, or None when the
/// network has no seeds.
///
/// For use cases where people run multiple p2poolv2 networks for a
/// single bitcoin network, they should use dial_peers and set
/// dns_seed to false in their configs.
pub(crate) fn dns_seeds_for_network(network: bitcoin::Network) -> Option<DnsSeeds> {
    match network {
        bitcoin::Network::Testnet4 => DnsSeeds::new(TESTNET4_SEED_HOSTNAMES, TESTNET4_P2P_PORT),
        _ => None,
    }
}

/// Returns the built-in DNS seeds for a Bitcoin Cash network, or None when
/// the network has none.
pub(crate) fn dns_seeds_for_bitcoin_cash(network: BitcoinCashNetwork) -> Option<DnsSeeds> {
    match network {
        BitcoinCashNetwork::Mainnet => {
            DnsSeeds::new(BITCOIN_CASH_MAINNET_SEED_HOSTNAMES, DEFAULT_P2P_PORT)
        }
        BitcoinCashNetwork::Chipnet => {
            DnsSeeds::new(BITCOIN_CASH_CHIPNET_SEED_HOSTNAMES, DEFAULT_P2P_PORT)
        }
        _ => None,
    }
}

/// Returns the DNS seeds a node on `chain` may query, or None.
///
/// `configured` (`network.dns_seed_hosts`), when not empty, replaces the
/// built-in hostnames; their nodes listen on the network's seed port.
/// Otherwise Bitcoin looks the seeds up by `network` and Bitcoin Cash by
/// `bitcoin_cash_network`. A Bitcoin Cash node's `stratum.network` is a
/// Bitcoin network (`BitcoinCashNetwork::legacy_network`: mainnet is
/// `Bitcoin`, chipnet `Testnet`), so a lookup by that field would hand it
/// Bitcoin P2Poolv2 nodes.
pub(crate) fn dns_seeds_for_chain(
    chain: Chain,
    network: bitcoin::Network,
    bitcoin_cash_network: Option<BitcoinCashNetwork>,
    configured: &[String],
) -> Option<DnsSeeds> {
    let built_in = match chain {
        Chain::Bitcoin => dns_seeds_for_network(network),
        Chain::BitcoinCash => bitcoin_cash_network.and_then(dns_seeds_for_bitcoin_cash),
    };
    if configured.is_empty() {
        return built_in;
    }
    let port = built_in.map_or(DEFAULT_P2P_PORT, |seeds| seeds.port);
    DnsSeeds::new(configured, port)
}

/// How long to wait before querying DNS seeds. Without dial_peers the seeds
/// are the only way to find the network, so they are queried at once.
/// Otherwise dial_peers get DNS_SEED_FALLBACK_DELAY_SECS to connect and the
/// seeds are only a fallback, as in Bitcoin Core.
pub(crate) fn dns_seed_delay(dial_peers_configured: bool) -> Duration {
    if dial_peers_configured {
        Duration::from_secs(DNS_SEED_FALLBACK_DELAY_SECS)
    } else {
        Duration::ZERO
    }
}

/// Spawns a short-lived task that resolves the seeds and asks the node actor
/// to dial each returned address once. DNS lookups never run on the actor
/// loop; if the actor has stopped, the remaining sends fail and are dropped.
pub(crate) fn spawn_dns_seed_dials<C: Send + 'static>(
    seeds: DnsSeeds,
    swarm_tx: mpsc::Sender<SwarmSend<C>>,
) {
    tokio::spawn(async move {
        let dial_addresses = resolve_dns_seeds(seeds).await;
        info!("Dialing {} addresses from DNS seeds", dial_addresses.len());
        for address in dial_addresses {
            if let Err(error) = swarm_tx.send(SwarmSend::Dial(address)).await {
                debug!("Node actor stopped before DNS seed dial: {error}");
            }
        }
    });
}

/// Resolves every seed hostname and returns the deduplicated, routable
/// addresses to dial. A failed or timed out lookup is logged and skipped so
/// one bad seed does not stop the others.
pub(crate) async fn resolve_dns_seeds(seeds: DnsSeeds) -> HashSet<Multiaddr> {
    let mut dial_addresses = HashSet::with_capacity(seeds.hostnames.len() * MAX_ADDRESSES_PER_SEED);
    for hostname in &seeds.hostnames {
        dial_addresses.extend(resolve_dns_seed(hostname, seeds.port).await);
    }
    dial_addresses
}

/// Resolves one seed hostname through the system resolver and returns at
/// most MAX_ADDRESSES_PER_SEED routable addresses on `port`. Duplicates are
/// left to the caller, which merges every seed's addresses into one set.
///
/// Seeds are resolved here with getaddrinfo, not dialed as `/dns` addresses
/// through the libp2p DNS transport (hickory) that dial_peers use: a `/dns`
/// dial stops at the first IP that connects, while a seed must yield every
/// A record so the node can connect to many peers. The two resolvers can
/// disagree on names that only nsswitch knows (mDNS, NSS plugins).
async fn resolve_dns_seed(hostname: &str, port: u16) -> Vec<Multiaddr> {
    let lookup = timeout(
        Duration::from_secs(DNS_SEED_LOOKUP_TIMEOUT_SECS),
        lookup_host((hostname, port)),
    );
    match lookup.await {
        Ok(Ok(resolved)) => {
            let dial_addresses = seed_dial_addresses(resolved, MAX_ADDRESSES_PER_SEED);
            info!(
                "DNS seed {hostname} returned {} dialable addresses",
                dial_addresses.len()
            );
            dial_addresses
        }
        Ok(Err(error)) => {
            warn!("DNS seed {hostname} lookup failed: {error}");
            Vec::new()
        }
        Err(_) => {
            warn!("DNS seed {hostname} lookup timed out after {DNS_SEED_LOOKUP_TIMEOUT_SECS}s");
            Vec::new()
        }
    }
}

/// Converts resolved socket addresses to TCP multiaddrs, dropping
/// non-routable IPs and keeping at most `limit`.
fn seed_dial_addresses(resolved: impl Iterator<Item = SocketAddr>, limit: usize) -> Vec<Multiaddr> {
    resolved
        .map(socket_addr_to_multiaddr)
        .filter(address_filter::is_routable_multiaddr)
        .take(limit)
        .collect()
}

/// Maps a socket address to `/ip4/<ip>/tcp/<port>` or `/ip6/<ip>/tcp/<port>`.
fn socket_addr_to_multiaddr(address: SocketAddr) -> Multiaddr {
    Multiaddr::from(address.ip()).with(Protocol::Tcp(address.port()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_addr_to_multiaddr_ipv4() {
        let address: SocketAddr = "23.227.187.89:46884".parse().unwrap();
        let expected: Multiaddr = "/ip4/23.227.187.89/tcp/46884".parse().unwrap();
        assert_eq!(socket_addr_to_multiaddr(address), expected);
    }

    #[test]
    fn socket_addr_to_multiaddr_ipv6() {
        let address: SocketAddr = "[2606:4700::1111]:46884".parse().unwrap();
        let expected: Multiaddr = "/ip6/2606:4700::1111/tcp/46884".parse().unwrap();
        assert_eq!(socket_addr_to_multiaddr(address), expected);
    }

    #[test]
    fn seed_dial_addresses_drops_private_and_loopback() {
        let resolved: Vec<SocketAddr> = vec![
            "127.0.0.1:46884".parse().unwrap(),
            "192.168.1.10:46884".parse().unwrap(),
            "23.227.187.89:46884".parse().unwrap(),
        ];
        let expected: Multiaddr = "/ip4/23.227.187.89/tcp/46884".parse().unwrap();
        assert_eq!(
            seed_dial_addresses(resolved.into_iter(), MAX_ADDRESSES_PER_SEED),
            vec![expected]
        );
    }

    /// Non-routable addresses do not use up the limit.
    #[test]
    fn seed_dial_addresses_caps_routable_addresses_at_limit() {
        let resolved: Vec<SocketAddr> = vec![
            "127.0.0.1:46884".parse().unwrap(),
            "23.227.187.89:46884".parse().unwrap(),
            "23.227.187.90:46884".parse().unwrap(),
            "23.227.187.91:46884".parse().unwrap(),
        ];
        let first: Multiaddr = "/ip4/23.227.187.89/tcp/46884".parse().unwrap();
        let second: Multiaddr = "/ip4/23.227.187.90/tcp/46884".parse().unwrap();
        assert_eq!(
            seed_dial_addresses(resolved.into_iter(), 2),
            vec![first, second]
        );
    }

    #[test]
    fn dns_seed_delay_is_zero_without_dial_peers() {
        assert_eq!(dns_seed_delay(false), Duration::ZERO);
    }

    #[test]
    fn dns_seed_delay_waits_for_dial_peers() {
        assert_eq!(
            dns_seed_delay(true),
            Duration::from_secs(DNS_SEED_FALLBACK_DELAY_SECS)
        );
    }

    #[test]
    fn dns_seeds_for_network_testnet4_has_seeds() {
        let seeds = dns_seeds_for_network(bitcoin::Network::Testnet4).unwrap();
        assert!(!seeds.hostnames.is_empty());
        assert_eq!(seeds.port, TESTNET4_P2P_PORT);
    }

    #[test]
    fn dns_seeds_for_network_signet_is_none() {
        assert_eq!(dns_seeds_for_network(bitcoin::Network::Signet), None);
    }

    #[test]
    fn dns_seeds_for_chain_bitcoin_testnet4_has_the_network_seeds() {
        assert_eq!(
            dns_seeds_for_chain(Chain::Bitcoin, bitcoin::Network::Testnet4, None, &[]),
            dns_seeds_for_network(bitcoin::Network::Testnet4)
        );
        assert!(
            dns_seeds_for_chain(Chain::Bitcoin, bitcoin::Network::Testnet4, None, &[]).is_some()
        );
    }

    /// Testnet4 is the network with Bitcoin seeds: a Bitcoin Cash node must
    /// not get them even with `stratum.network` set to it.
    #[test]
    fn a_bitcoin_cash_node_never_gets_bitcoin_seeds() {
        for bitcoin_cash_network in [
            None,
            Some(BitcoinCashNetwork::Mainnet),
            Some(BitcoinCashNetwork::Chipnet),
            Some(BitcoinCashNetwork::Regtest),
        ] {
            assert_eq!(
                dns_seeds_for_chain(
                    Chain::BitcoinCash,
                    bitcoin::Network::Testnet4,
                    bitcoin_cash_network,
                    &[]
                ),
                bitcoin_cash_network.and_then(dns_seeds_for_bitcoin_cash)
            );
        }
    }

    /// Mainnet and chipnet have seed lists, empty until operators run seeds;
    /// regtest and scalenet never have any.
    #[test]
    fn bitcoin_cash_seeds_are_picked_by_bitcoin_cash_network() {
        assert_eq!(
            dns_seeds_for_bitcoin_cash(BitcoinCashNetwork::Mainnet),
            DnsSeeds::new(BITCOIN_CASH_MAINNET_SEED_HOSTNAMES, DEFAULT_P2P_PORT)
        );
        assert_eq!(
            dns_seeds_for_bitcoin_cash(BitcoinCashNetwork::Chipnet),
            DnsSeeds::new(BITCOIN_CASH_CHIPNET_SEED_HOSTNAMES, DEFAULT_P2P_PORT)
        );
        assert_eq!(
            dns_seeds_for_bitcoin_cash(BitcoinCashNetwork::Regtest),
            None
        );
        assert_eq!(
            dns_seeds_for_bitcoin_cash(BitcoinCashNetwork::Scalenet),
            None
        );
    }

    /// `dns_seed_hosts` replaces the built-in list, on the network's seed
    /// port.
    #[test]
    fn configured_seed_hosts_replace_the_built_in_ones() {
        let configured = vec!["seed.example.org".to_string()];
        assert_eq!(
            dns_seeds_for_chain(
                Chain::BitcoinCash,
                bitcoin::Network::Testnet,
                Some(BitcoinCashNetwork::Chipnet),
                &configured
            ),
            Some(DnsSeeds {
                hostnames: configured.clone(),
                port: DEFAULT_P2P_PORT,
            })
        );
        assert_eq!(
            dns_seeds_for_chain(
                Chain::Bitcoin,
                bitcoin::Network::Testnet4,
                None,
                &configured
            ),
            Some(DnsSeeds {
                hostnames: configured,
                port: TESTNET4_P2P_PORT,
            })
        );
    }
}
