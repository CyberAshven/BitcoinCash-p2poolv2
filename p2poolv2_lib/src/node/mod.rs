// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

mod address_filter;
pub mod bch_header_feeder;
pub mod behaviour;
pub mod connection_tracker;
mod dial_filter;
mod dns_seeds;
pub mod emission_worker;
mod found_block_verifier;
mod node_key;
pub(crate) mod nostr_seeds;
mod onion;
pub mod organise_worker;
mod outgoing_peer_target;
pub mod p2p_health;
pub mod payout_tree_executor;
mod peer_book;
pub mod peer_reconnector;
pub mod request_response_handler;
pub mod request_sender;
pub mod response_worker;
mod running_lock;
pub mod validation_worker;
mod weak_share_fetch;
mod weak_share_fill;
pub mod weak_share_worker;
pub use crate::config::Config;
pub mod actor;
pub mod messages;
pub mod p2p_message_handlers;

use crate::accounting::payout::simple_pplns::SimplePplnsShare;
use crate::config::{Chain, NetworkConfig};
use crate::monitoring_events::{MonitoringEvent, MonitoringEventSender, PeerResponse, PeerStatus};
use crate::node::messages::Message;
use crate::node::p2p_health::P2pHealth;
use crate::node::p2p_message_handlers::receivers::block_receiver::BlockReceiverHandle;
use crate::node::p2p_message_handlers::senders::build_handshake_message;
use crate::node::request_response_handler::RequestResponseHandler;
use crate::node::request_response_handler::block_fetcher::BlockFetcherHandle;
use crate::node::request_sender::RequestSender;
use crate::node::response_worker::ResponseWorkerSender;
use crate::node::validation_worker::ValidationSender;
#[cfg(test)]
#[mockall_double::double]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
#[cfg(not(test))]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
use crate::shares::share_block::ShareBlock;
use crate::shares::transaction_pool::ShareTransactionPool;
use crate::shares::validation::ShareValidator;
use behaviour::request_response::network_prologue;
use behaviour::{P2PoolBehaviour, P2PoolBehaviourEvent};
use libp2p::PeerId;
use libp2p::SwarmBuilder;
use libp2p::core::transport::{Boxed, Transport};
use libp2p::dns::tokio::Transport as DnsTransport;
use libp2p::identify;
use libp2p::ping::Failure;
use libp2p::request_response::ResponseChannel;
use libp2p::tcp::Config as TcpConfig;
use libp2p::tcp::tokio::{TcpStream, Transport as TcpTransport};
use libp2p::{
    Multiaddr, Swarm,
    kad::{Event as KademliaEvent, QueryResult},
    swarm::{DialError, SwarmEvent, dial_opts::DialOpts},
};
use std::collections::HashSet;
use std::error::Error;
use std::io;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

pub struct SwarmResponseChannel<T> {
    channel: ResponseChannel<T>,
}

#[allow(dead_code)]
pub trait SwarmResponseChannelTrait<T> {
    fn new(channel: ResponseChannel<T>) -> Self;
    fn channel(&self) -> &ResponseChannel<T>;
}

impl<T> SwarmResponseChannelTrait<T> for SwarmResponseChannel<T> {
    fn new(channel: ResponseChannel<T>) -> Self {
        Self { channel }
    }
    fn channel(&self) -> &ResponseChannel<T> {
        &self.channel
    }
}

/// Capture send type for swarm p2p messages that can be sent to the swarm
#[allow(dead_code)]
#[derive(Debug)]
pub enum SwarmSend<C> {
    Request(PeerId, Message),
    Response(C, Message),
    /// Broadcast a full ShareBlock to connected peers, filtering by
    /// peer_block_knowledge. The actor records each recipient so that
    /// subsequent broadcasts of the same block are suppressed.
    /// A share block to relay, with the Bitcoin Cash headers it carries: one
    /// this node mined, once validated, or one a peer sent, once its header
    /// checked out and before its body is validated.
    BroadcastBlock(ShareBlock, Vec<bitcoin::block::Header>),
    Disconnect(PeerId),
    /// Dial an address once, without reconnect tracking. Used for
    /// addresses returned by DNS seeds.
    Dial(Multiaddr),
}

use connection_tracker::{BlockedIps, ConnectionAction, ConnectionTracker, PING_FAILURE_THRESHOLD};
use dial_filter::DialFilter;
use outgoing_peer_target::{OutgoingCounts, OutgoingPeerTarget};

/// Close a connection after this long with no streams in use and no protocol
/// asking to keep it alive.
///
/// A backstop only. Identify exchanges every 5 minutes, which counts as
/// activity, so a healthy but quiet connection is never reaped -- the timeout
/// must stay above that interval. For the same reason it does not catch a
/// connection that negotiated but cannot carry p2pool messages (identify
/// keeps working there); closing on repeated ping failures handles that. This
/// only reaps connections where everything, identify included, has gone quiet.
const IDLE_CONNECTION_TIMEOUT_SECS: u64 = 600;

/// The transport below the noise and yamux upgrade. A dial passes through
/// four layers, in this order:
///
/// 1. Refused before anything is dialed or resolved: an address on a network
///    the node does not dial (`address_filter::Reachable`: `onlynet`, and
///    onion only with a `tor_proxy`); a `/dns`, `/dns4`, `/dns6` or
///    `/dnsaddr` address that is not a configured dial_peer, so hostnames
///    learned from peers (Kademlia) are never resolved or dialed; and an IP
///    on the blocklist, for the dials that go through Tor.
/// 2. With `tor_proxy`, an `/onion3` address is dialed through that Tor
///    SOCKS5 proxy (`onion::OnionTransport`), and with `proxy_clearnet` an IP
///    or host-name address too, the name unresolved. Every other address
///    goes on unchanged.
/// 3. DNS resolves the hostname to IP addresses, using the system's
///    resolv.conf. IP addresses pass through unchanged.
/// 4. Each resolved IP is refused if it is on the blocklist or on a network
///    the node does not dial, otherwise TCP connects to it. These checks sit
///    after DNS because only there is the IP behind a hostname known.
///
/// A node that opens no clearnet connection of its own (`proxy_clearnet`, or
/// an `onlynet` without IPv4 and IPv6) has no layers 3 and 4: its TCP only
/// listens, and nothing reaches the system's resolver.
fn base_transport(
    tcp_config: TcpConfig,
    network: &NetworkConfig,
    blocked_ips: BlockedIps,
) -> Boxed<TcpStream> {
    let reachable = address_filter::Reachable::from_config(network);
    let direct = if network.dials_clearnet_directly() {
        let tcp_for_dns_transport =
            blocked_ip_filtered_tcp(tcp_config.clone(), blocked_ips.clone(), reachable);
        let fallback_tcp = blocked_ip_filtered_tcp(tcp_config, blocked_ips.clone(), reachable);
        dns_or_tcp_transport(DnsTransport::system(tcp_for_dns_transport), fallback_tcp)
    } else {
        DialFilter::new(TcpTransport::new(tcp_config), |_: &Multiaddr| false).boxed()
    };
    let tor_or_direct = match &network.tor_proxy {
        Some(proxy) => onion::OnionTransport::new(proxy.clone())
            .with_clearnet(network.proxy_clearnet)
            .or_transport(direct)
            .map(|stream, _| stream.into_inner())
            .map_err(|error| error.into_inner())
            .boxed(),
        None => direct,
    };
    let configured_dns = dial_filter::configured_dns_addresses(&network.dial_peers);
    DialFilter::new(tor_or_direct, move |address: &Multiaddr| {
        reachable.allows(address)
            && dial_filter::is_configured_or_not_dns(address, &configured_dns)
            && dial_filter::is_not_blocked(address, &blocked_ips)
    })
    .boxed()
}

/// TCP that refuses to dial IPs on the blocklist, and IPs on a network the
/// node does not dial (`reachable`), which a host name may resolve to.
fn blocked_ip_filtered_tcp(
    tcp_config: TcpConfig,
    blocked_ips: BlockedIps,
    reachable: address_filter::Reachable,
) -> DialFilter<TcpTransport, impl Fn(&Multiaddr) -> bool + Send + Sync + Unpin + 'static> {
    DialFilter::new(TcpTransport::new(tcp_config), move |address: &Multiaddr| {
        reachable.allows(address) && dial_filter::is_not_blocked(address, &blocked_ips)
    })
}

/// Boxes the DNS transport, or, when the system resolver config could not
/// be loaded (no /etc/resolv.conf or no nameservers in it, as in some minimal
/// containers), warns and boxes `fallback` instead. The node then still runs
/// with `/ip4` and `/ip6` peers; `/dns` dials fail with MultiaddrNotSupported.
/// `fallback` is passed in because DnsTransport::system consumes the inner
/// transport even when it fails.
fn dns_or_tcp_transport<T>(
    dns_transport: Result<DnsTransport<T>, io::Error>,
    fallback: T,
) -> Boxed<TcpStream>
where
    T: Transport<Output = TcpStream> + Send + Unpin + 'static,
    T::Error: Send + Sync + 'static,
    T::Dial: Send + 'static,
    T::ListenerUpgrade: Send + 'static,
{
    match dns_transport {
        Ok(dns_transport) => dns_transport.boxed(),
        Err(error) => {
            warn!(
                "Failed to load system resolver config, /dns dial addresses will not resolve: {error}"
            );
            fallback.boxed()
        }
    }
}

/// Node is the main struct that represents the node
struct Node {
    swarm: Swarm<P2PoolBehaviour>,
    swarm_tx: mpsc::Sender<SwarmSend<ResponseChannel<Message>>>,
    swarm_rx: mpsc::Receiver<SwarmSend<ResponseChannel<Message>>>,
    chain_store_handle: ChainStoreHandle,
    request_response_handler: RequestResponseHandler<ResponseChannel<Message>>,
    monitoring_event_sender: MonitoringEventSender,
    peer_reconnector: peer_reconnector::PeerReconnector,
    /// Dials known peers while the node has fewer outgoing peers than
    /// target_outgoing_peers.
    outgoing_peer_target: OutgoingPeerTarget,
    /// Where the outgoing peer target's book is saved, or None with
    /// `ephemeral_node_key` (`peer_book`).
    peer_book_path: Option<std::path::PathBuf>,
    connection_tracker: ConnectionTracker,
    /// Whether an external address has been confirmed and advertised
    external_address_confirmed: bool,
    /// Cached TCP listen port extracted from config
    listen_port: Option<u16>,
    /// Whether kademlia bootstrap has been triggered at least once
    has_bootstrapped_kad: bool,
    /// DNS seeds to fall back on, or None when disabled in config, the
    /// network has none, or the chain is Bitcoin Cash.
    dns_seeds: Option<dns_seeds::DnsSeeds>,
    /// Nostr relays to announce on and look peers up on, or None without
    /// relays (`nostr_seeds`).
    nostr_seeds: Option<Arc<nostr_seeds::NostrSeeds>>,
    /// The task publishing this node's announcement, while it runs.
    nostr_announcer: Option<tokio::task::JoinHandle<()>>,
    /// Held while the node runs, so no other node starts on its key or its
    /// peer book (`running_lock`). None for a node with `ephemeral_node_key`,
    /// which keeps neither.
    _node_key_lock: Option<running_lock::RunningLock>,
    _peer_book_lock: Option<running_lock::RunningLock>,
}

impl Node {
    #[allow(clippy::too_many_arguments)] // wiring constructor: each parameter is a distinct collaborator, a params struct would only move the list
    pub fn new(
        config: Config,
        chain_store_handle: ChainStoreHandle,
        block_fetcher_handle: BlockFetcherHandle,
        validation_tx: ValidationSender,
        block_receiver_handle: BlockReceiverHandle,
        monitoring_event_sender: MonitoringEventSender,
        share_validator: Arc<dyn ShareValidator + Send + Sync>,
        response_worker_tx: ResponseWorkerSender,
        share_transaction_pool: Option<ShareTransactionPool>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let (id_keys, node_key_lock) =
            node_key::load_or_create(config.network.node_key_file(&config.store.path).as_deref())?;
        let peer_book_lock = match config.network.peer_book_file(&config.store.path) {
            Some(path) => running_lock::hold(&path).map_err(|_| {
                format!(
                    "Another node keeps its peer list in {}. A node keeps it beside its store, so two nodes whose stores share a directory would load each other's peers. Give each node its own directory",
                    path.display()
                )
            })?,
            None => None,
        };

        let genesis_hash = crate::shares::bch_genesis::genesis_for_stratum(&config.stratum)
            .map_err(|error| -> Box<dyn Error> { error })?
            .block_hash();
        let network_protocol =
            network_prologue(config.stratum.network, config.stratum.chain, genesis_hash);
        let nostr_seeds =
            nostr_seeds::from_config(&config, &id_keys, &network_protocol)?.map(Arc::new);

        let behavior = match P2PoolBehaviour::new(&id_keys, &config, genesis_hash) {
            Ok(behavior) => behavior,
            Err(err) => {
                error!("Failed to create P2PoolBehaviour: {}", err);
                std::process::exit(1);
            }
        };

        let dial_timeout_secs = config.network.dial_timeout_secs;

        let tcp_config = TcpConfig::default().nodelay(true);
        let noise_config = match libp2p::noise::Config::new(&id_keys) {
            Ok(cfg) => cfg.with_prologue(network_protocol.as_bytes().to_vec()),
            Err(err) => {
                error!("Failed to create Noise config: {}", err);
                return Err(Box::new(err));
            }
        };

        let blocked_ips: HashSet<IpAddr> = config
            .network
            .blocked_ips
            .iter()
            .filter_map(|ip_str| {
                ip_str
                    .parse::<IpAddr>()
                    .map_err(|error| {
                        warn!("Invalid blocked IP '{}' in config: {}", ip_str, error);
                    })
                    .ok()
            })
            .collect();

        if !blocked_ips.is_empty() {
            info!("Loaded {} blocked IPs from config", blocked_ips.len());
        }
        let blocked_ips = BlockedIps::new(blocked_ips);

        let transport = base_transport(tcp_config, &config.network, blocked_ips.clone())
            .upgrade(libp2p::core::upgrade::Version::V1)
            .authenticate(noise_config)
            .multiplex(libp2p::yamux::Config::default())
            .timeout(Duration::from_secs(dial_timeout_secs))
            .boxed();

        let mut swarm = SwarmBuilder::with_existing_identity(id_keys)
            .with_tokio()
            .with_other_transport(|_| transport)?
            .with_behaviour(|_| behavior)?
            .with_swarm_config(|cfg| {
                cfg.with_idle_connection_timeout(Duration::from_secs(IDLE_CONNECTION_TIMEOUT_SECS))
            })
            .build();

        info!("Local peer id: {}", swarm.local_peer_id());
        info!("P2P protocol string: {network_protocol}");

        let listen_port = address_filter::extract_listen_port(&config.network.listen_address);

        let external_address_confirmed =
            if let Some(ref external_addr_str) = config.network.external_address {
                match external_addr_str.parse::<Multiaddr>() {
                    Ok(external_addr) => {
                        info!("Using configured external address: {}", external_addr);
                        swarm.add_external_address(external_addr);
                        true
                    }
                    Err(error) => {
                        warn!(
                            "Invalid external_address '{}' in config: {}",
                            external_addr_str, error
                        );
                        false
                    }
                }
            } else {
                false
            };

        if !config.network.listen_address.is_empty() {
            match config.network.listen_address.parse() {
                Ok(addr) => match swarm.listen_on(addr) {
                    Ok(_) => {
                        info!("Node listening on {}", config.network.listen_address);
                    }
                    Err(e) => {
                        error!(
                            "Failed to listen on {}: {}",
                            config.network.listen_address, e
                        );
                        return Err(format!(
                            "Failed to listen on {}: {}",
                            config.network.listen_address, e
                        )
                        .into());
                    }
                },
                Err(e) => {
                    error!(
                        "Invalid listen address {}: {}",
                        config.network.listen_address, e
                    );
                    return Err(format!(
                        "Invalid listen address {}: {}",
                        config.network.listen_address, e
                    )
                    .into());
                }
            }
        }

        let (swarm_tx, swarm_rx) = mpsc::channel(100);

        let dns_seeds = if !config.network.dns_seed {
            None
        } else if !config.network.dials_clearnet_directly() {
            info!(
                "DNS seeds are off: this node resolves no host name itself (proxy_clearnet or onlynet)"
            );
            None
        } else {
            dns_seeds::dns_seeds_for_chain(config.stratum.chain, config.stratum.network)
        };
        if config.stratum.chain == Chain::BitcoinCash {
            info!("DNS seeds are off on Bitcoin Cash, which has none yet; dialing dial_peers only");
        }

        let request_response_handler = RequestResponseHandler::new(
            config.network.clone(),
            chain_store_handle.clone(),
            swarm_tx.clone(),
            block_fetcher_handle,
            validation_tx,
            block_receiver_handle,
            share_validator,
            response_worker_tx,
            share_transaction_pool,
        );

        let peer_reconnector = peer_reconnector::PeerReconnector::new(&config.network.dial_peers);
        let mut outgoing_peer_target =
            OutgoingPeerTarget::new(&config.network, blocked_ips.clone());
        let peer_book_path = config.network.peer_book_file(&config.store.path);
        if let Some(path) = &peer_book_path {
            outgoing_peer_target.load_book(peer_book::load(path));
        }

        let mut node = Self {
            swarm,
            swarm_tx,
            swarm_rx,
            chain_store_handle,
            request_response_handler,
            monitoring_event_sender,
            peer_reconnector,
            outgoing_peer_target,
            peer_book_path,
            connection_tracker: ConnectionTracker::new(blocked_ips),
            external_address_confirmed,
            listen_port,
            has_bootstrapped_kad: false,
            dns_seeds,
            nostr_seeds,
            nostr_announcer: None,
            _node_key_lock: node_key_lock,
            _peer_book_lock: peer_book_lock,
        };
        node.attempt_reconnections();
        Ok(node)
    }

    /// Starts publishing this node's announcement on its Nostr relays, when
    /// it has relays and something to announce. The actor calls this once,
    /// inside the runtime.
    pub(crate) fn start_nostr_announcer(&mut self) {
        if let Some(seeds) = &self.nostr_seeds
            && !seeds.announce.is_empty()
            && self.nostr_announcer.is_none()
        {
            info!(
                "Announcing {} on {} Nostr relays",
                seeds
                    .announce
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", "),
                seeds.relays.len()
            );
            self.nostr_announcer = Some(nostr_seeds::spawn_announcer(Arc::clone(seeds)));
        }
    }

    /// Asks the Nostr relays for announced peers and dials the newest few,
    /// while this node has fewer than `nostr_seeds::LOOKUP_BELOW_PEERS`
    /// peers.
    pub(crate) fn look_up_nostr_peers_if_short(&self) {
        let Some(seeds) = &self.nostr_seeds else {
            return;
        };
        let connected: HashSet<libp2p::PeerId> = self.swarm.connected_peers().copied().collect();
        if connected.len() >= nostr_seeds::LOOKUP_BELOW_PEERS {
            debug!(
                "Skipping the Nostr lookup, {} peers connected",
                connected.len()
            );
            return;
        }
        info!(
            "{} peers connected, asking {} Nostr relays for more",
            connected.len(),
            seeds.relays.len()
        );
        nostr_seeds::spawn_lookup(Arc::clone(seeds), connected, self.swarm_tx.clone());
    }

    /// Queries the DNS seeds and dials the returned addresses, unless a peer
    /// is already connected. The actor calls this once, after
    /// dns_seeds::dns_seed_delay.
    fn query_dns_seeds_if_isolated(&self) {
        let connected_peer_count = self.connection_tracker.connected_peer_count();
        match self.dns_seeds {
            Some(seeds) if connected_peer_count == 0 => {
                info!("No peers connected, querying DNS seeds");
                dns_seeds::spawn_dns_seed_dials(seeds, self.swarm_tx.clone());
            }
            Some(_) => {
                info!("Skipping DNS seeds, {connected_peer_count} peers already connected");
            }
            None => {}
        }
    }

    /// Returns a Vec of peer IDs that are currently connected to this node
    #[allow(dead_code)]
    pub fn connected_peers(&self) -> Vec<libp2p::PeerId> {
        self.swarm.connected_peers().cloned().collect()
    }

    /// Connected peers that share-chain traffic goes to: each sent a
    /// handshake and is not connected only through this node's lookup.
    pub fn share_chain_peers(&self) -> Vec<libp2p::PeerId> {
        self.swarm
            .connected_peers()
            .filter(|peer_id| self.request_response_handler.is_share_chain_peer(peer_id))
            .cloned()
            .collect()
    }

    /// Save the outgoing peer target's book (`peer_book`), unless the node
    /// keeps none. A failure is logged; the node goes on.
    pub(crate) fn save_peer_book(&self) {
        if let Some(path) = &self.peer_book_path
            && let Err(error) = peer_book::save(path, &self.outgoing_peer_target.saved_book())
        {
            warn!("{error}");
        }
    }

    #[allow(dead_code)]
    pub fn shutdown(&mut self) -> Result<(), Box<dyn Error>> {
        self.save_peer_book();
        if let Some(announcer) = self.nostr_announcer.take() {
            announcer.abort();
        }
        for peer_id in self.swarm.connected_peers().cloned().collect::<Vec<_>>() {
            self.swarm.disconnect_peer_id(peer_id).unwrap_or_default();
        }
        Ok(())
    }

    /// Snapshot of P2P health counters for `/metrics`.
    pub(crate) fn p2p_health(&self) -> P2pHealth {
        let connected_peers = self.connection_tracker.connected_peer_count();
        let outgoing_peers = self.outgoing_peer_target.outgoing_peer_count();
        P2pHealth {
            connected_peers,
            outgoing_peers,
            incoming_peers: connected_peers.saturating_sub(outgoing_peers),
            outgoing_target_dials_total: self.outgoing_peer_target.dials_attempted_total(),
            connections_total: self.connection_tracker.connections_total(),
            ping_failures_total: self.connection_tracker.ping_failures_total(),
            connections_closed_unresponsive_total: self
                .connection_tracker
                .connections_closed_unresponsive_total(),
            outbound_failures_total: self.request_response_handler.outbound_failures_total(),
            inbound_failures_total: self.request_response_handler.inbound_failures_total(),
            responses_dropped_total: self.request_response_handler.responses_dropped_total(),
            response_queue_depth: self.request_response_handler.response_queue_depth(),
            share_relay: self.request_response_handler.share_relay(),
            share_rebuilds: self.request_response_handler.share_rebuilds(),
        }
    }

    /// Dial any configured dial_peers that are not connected and have no dial
    /// in flight. Called once from Node::new and then on the reconnect timer.
    /// Each started dial's ConnectionId is handed to the reconnector so the
    /// outcome can be matched back to it.
    fn attempt_reconnections(&mut self) {
        let connected_dial_addresses = self.connection_tracker.connected_dial_addresses();
        let addresses = self
            .peer_reconnector
            .addresses_to_reconnect(&connected_dial_addresses);
        for address in addresses {
            let dial_opts = DialOpts::unknown_peer_id().address(address.clone()).build();
            let connection_id = dial_opts.connection_id();
            match self.swarm.dial(dial_opts) {
                Ok(_) => {
                    info!("Dialing {address}");
                    self.peer_reconnector
                        .record_dial_started(&address, connection_id);
                }
                Err(error) => {
                    error!("Failed to redial {address}: {error}");
                    self.peer_reconnector.record_dial_failure(&address);
                }
            }
        }
    }

    /// Dial one known peer while the node has fewer outgoing peers than
    /// target_outgoing_peers. Called on the outgoing target timer; see
    /// outgoing_peer_target for how the peer is chosen. The started dial's
    /// ConnectionId is handed to the target so its outcome can be matched
    /// back to it.
    fn dial_toward_outgoing_target(&mut self) {
        let network_info = self.swarm.network_info();
        let counters = network_info.connection_counters();
        let counts = OutgoingCounts {
            pending: counters.num_pending_outgoing(),
            established: counters.num_established_outgoing(),
        };
        let swarm = &self.swarm;
        let Some(dial) = self
            .outgoing_peer_target
            .next_dial(counts, |peer_id| swarm.is_connected(peer_id))
        else {
            return;
        };
        let dial_opts = DialOpts::peer_id(dial.peer_id)
            .addresses(vec![dial.address.clone()])
            .build();
        let connection_id = dial_opts.connection_id();
        match self.swarm.dial(dial_opts) {
            Ok(()) => {
                debug!(
                    "Dialing {} at {} toward the outgoing peer target",
                    dial.peer_id, dial.address
                );
                self.outgoing_peer_target
                    .record_dial_started(connection_id, dial);
            }
            // Another dial to the peer is in flight, or the connection limits
            // are full: neither is the peer's fault.
            Err(error @ (DialError::DialPeerConditionFalse(_) | DialError::Denied { .. })) => {
                debug!("Did not dial {} at {}: {error}", dial.peer_id, dial.address);
            }
            Err(error) => {
                debug!(
                    "Failed to dial {} at {}: {error}",
                    dial.peer_id, dial.address
                );
                self.outgoing_peer_target.record_dial_failure(&dial.peer_id);
            }
        }
    }

    /// Close the connections Kademlia opened for lookups once no query needs
    /// them (see outgoing_peer_target), unless target_outgoing_peers is 0.
    /// Called on the outgoing target timer and when a Kademlia query ends.
    fn close_finished_lookups(&mut self) {
        if !self.outgoing_peer_target.has_target() {
            return;
        }
        let queries_running = self
            .swarm
            .behaviour()
            .kademlia
            .iter_queries()
            .next()
            .is_some();
        for connection_id in self.outgoing_peer_target.finished_lookups(queries_running) {
            if self.swarm.close_connection(connection_id) {
                debug!("Closing connection {connection_id}, which Kademlia opened for a lookup");
                self.outgoing_peer_target
                    .record_lookup_closing(connection_id);
            }
        }
    }

    /// Dial an address a DNS or Nostr seed returned, once, without reconnect
    /// tracking. The connection is the node's own, not a lookup to close.
    fn dial_seed(&mut self, address: Multiaddr) {
        let dial_opts = DialOpts::unknown_peer_id().address(address.clone()).build();
        let connection_id = dial_opts.connection_id();
        match self.swarm.dial(dial_opts) {
            Ok(()) => {
                info!("Dialed {address}");
                self.outgoing_peer_target
                    .record_seed_dial_started(connection_id);
            }
            Err(error) => debug!("Failed to dial {address}: {error}"),
        }
    }

    /// Send a message to a specific peer
    pub fn send_to_peer(
        &mut self,
        peer_id: &libp2p::PeerId,
        message: Message,
    ) -> Result<(), Box<dyn Error>> {
        debug!("Sending message to peer: {peer_id}, message: {message:?}");
        self.swarm
            .behaviour_mut()
            .request_response
            .send_request(peer_id, message);
        Ok(())
    }

    /// Handle the command to get pplns shares from store
    pub fn handle_get_pplns_shares(
        &self,
        query: crate::command::GetPplnsShareQuery,
    ) -> Vec<SimplePplnsShare> {
        self.chain_store_handle.get_pplns_shares_filtered(
            Some(query.limit),
            query.start_time,
            query.end_time,
        )
    }

    /// Handle swarm events, these are events that are generated by the libp2p library
    pub fn handle_swarm_event(
        &mut self,
        event: SwarmEvent<P2PoolBehaviourEvent>,
    ) -> Result<(), Box<dyn Error>> {
        match event {
            SwarmEvent::NewListenAddr { address, .. } => {
                info!("Listening on {address:?}");
                if self.listen_port.is_none()
                    && let Some(port) = address_filter::extract_tcp_port(&address)
                {
                    info!("Resolved actual listen port: {port}");
                    self.listen_port = Some(port);
                }
                if !self.external_address_confirmed
                    && address_filter::is_routable_multiaddr(&address)
                {
                    self.swarm.add_external_address(address.clone());
                    self.external_address_confirmed = true;
                    info!("Added routable listen address as external: {address}");
                }
                Ok(())
            }
            SwarmEvent::ConnectionEstablished {
                peer_id,
                connection_id,
                endpoint,
                num_established,
                ..
            } => {
                self.peer_reconnector
                    .record_connection_established(connection_id);
                // A lookup only discovers addresses: with the target on, this
                // node sends no handshake on it, so it carries no share-chain
                // traffic and closing it cuts nothing.
                let lookup = self.outgoing_peer_target.record_connection_established(
                    peer_id,
                    connection_id,
                    &endpoint,
                ) && self.outgoing_peer_target.has_target();

                match self
                    .connection_tracker
                    .handle_established(peer_id, &endpoint)
                {
                    ConnectionAction::Block => {
                        let _ = self.swarm.disconnect_peer_id(peer_id);
                        return Ok(());
                    }
                    // A further connection to an already connected peer needs
                    // none of the per-peer setup: re-running add_peer would
                    // replace the peer's request service and drop its queue.
                    ConnectionAction::Accept(ref peer_info) if num_established.get() > 1 => {
                        info!(
                            "{:?} connection {} to already connected peer {} ({} open)",
                            peer_info.direction, connection_id, peer_id, num_established
                        );
                        // A connection that is not a lookup, to a peer this
                        // node reached only through one, makes the peer a
                        // share-chain peer.
                        if !lookup && self.request_response_handler.clear_lookup_only(&peer_id) {
                            info!(
                                "Connection {connection_id} to lookup-only peer {peer_id} is not a lookup, sending the handshake"
                            );
                            if let Ok(handshake_message) =
                                build_handshake_message(&self.chain_store_handle)
                            {
                                self.swarm.send_request(&peer_id, handshake_message);
                            }
                        }
                        return Ok(());
                    }
                    ConnectionAction::Accept(ref peer_info) if lookup => {
                        self.request_response_handler.set_lookup_only(peer_id);
                        debug!(
                            "{:?} connection {} to {} is a Kademlia lookup, no handshake",
                            peer_info.direction, connection_id, peer_id
                        );
                    }
                    ConnectionAction::Accept(ref peer_info) => {
                        match build_handshake_message(&self.chain_store_handle) {
                            Ok(handshake_message) => {
                                self.swarm.send_request(&peer_id, handshake_message);
                                debug!(
                                    "{:?} connection {} established, handshake sent to peer: {}",
                                    peer_info.direction, connection_id, peer_id
                                );
                            }
                            Err(error) => {
                                error!(
                                    "Failed to build handshake for peer {} on connection {}: {}",
                                    peer_id, connection_id, error
                                );
                            }
                        }
                    }
                }

                self.request_response_handler.add_peer(peer_id);
                let _ = self
                    .monitoring_event_sender
                    .send(MonitoringEvent::Peer(PeerResponse {
                        peer_id: peer_id.to_string(),
                        status: PeerStatus::Connected,
                    }));
                Ok(())
            }
            SwarmEvent::ConnectionClosed {
                peer_id,
                connection_id,
                num_established,
                ..
            } => {
                self.connection_tracker.clear_ping_failures(connection_id);
                self.connection_tracker
                    .handle_closed(&peer_id, num_established);
                let closed_lookup = self
                    .outgoing_peer_target
                    .record_connection_closed(connection_id);
                if num_established > 0 {
                    info!(
                        "Closed connection {connection_id} to peer {peer_id}, {num_established} still open"
                    );
                    return Ok(());
                }
                info!("Disconnected from peer: {peer_id} on connection {connection_id}");
                // A peer whose lookup connection this node closed is still up:
                // Kademlia keeps it in its routing table, as disconnected.
                if !closed_lookup {
                    self.swarm.behaviour_mut().remove_peer(&peer_id);
                }
                self.request_response_handler.remove_peer(&peer_id);
                let _ = self
                    .monitoring_event_sender
                    .send(MonitoringEvent::Peer(PeerResponse {
                        peer_id: peer_id.to_string(),
                        status: PeerStatus::Disconnected,
                    }));
                Ok(())
            }
            SwarmEvent::OutgoingConnectionError {
                peer_id,
                error,
                connection_id,
            } => {
                error!(
                    "Failed to connect to peer: {peer_id:?}, error: {error}, connection_id: {connection_id}"
                );
                self.peer_reconnector
                    .record_connection_failure(connection_id);
                self.outgoing_peer_target
                    .record_connection_failure(connection_id);
                // Kademlia keeps a peer whose lookup connection this node
                // closed, and keeps an unreachable peer at its last address,
                // so a peer that has gone, or comes back under another ID,
                // would stay in the routing table, be handed to other nodes'
                // lookups and be dialed again by all of them. A peer the node
                // failed to reach, and has no connection to, is forgotten; a
                // live one is learned again from the next lookup that finds
                // it. A dial the node itself declined says nothing of the peer.
                if let Some(peer_id) = peer_id
                    && !self.swarm.is_connected(&peer_id)
                    && matches!(
                        error,
                        DialError::WrongPeerId { .. }
                            | DialError::Transport(_)
                            | DialError::NoAddresses
                    )
                {
                    debug!("Removing unreachable peer {peer_id} from the routing table");
                    self.swarm.behaviour_mut().remove_peer(&peer_id);
                }
                Ok(())
            }
            SwarmEvent::Behaviour(event) => match event {
                P2PoolBehaviourEvent::Identify(identify_event) => {
                    self.handle_identify_event(identify_event);
                    Ok(())
                }
                P2PoolBehaviourEvent::Kademlia(kad_event) => {
                    self.handle_kademlia_event(kad_event);
                    Ok(())
                }
                P2PoolBehaviourEvent::Ping(ping_event) => {
                    self.handle_ping_event(ping_event);
                    Ok(())
                }
                P2PoolBehaviourEvent::RequestResponse(request_response_event) => self
                    .request_response_handler
                    .handle_event(request_response_event, &mut self.swarm),
            },
            _ => Ok(()),
        }
    }

    /// Handle identify events, these are events that are generated by the identify protocol
    fn handle_identify_event(&mut self, event: identify::Event) {
        match event {
            identify::Event::Received {
                peer_id,
                info,
                connection_id,
            } => {
                info!(
                    "Identified Peer {} on connection {} with protocol version {}",
                    peer_id, connection_id, info.protocol_version
                );
                self.connection_tracker
                    .record_identified(&peer_id, info.agent_version.clone());
                self.outgoing_peer_target
                    .learn_addresses(peer_id, &info.listen_addrs);
                // Add the peer's reachable advertised addresses to Kademlia:
                // routable IPs, and onion addresses when the node has Tor.
                let reachable = self.outgoing_peer_target.reachable();
                for addr in info.listen_addrs {
                    if address_filter::is_reachable(&addr, reachable) {
                        self.swarm
                            .behaviour_mut()
                            .kademlia
                            .add_address(&peer_id, addr.clone());
                    } else {
                        debug!(
                            "Skipping non-routable address {} from peer {}",
                            addr, peer_id
                        );
                    }
                }
                self.try_confirm_external_address(&info.observed_addr);
                if !self.has_bootstrapped_kad {
                    self.attempt_kademlia_bootstrap();
                }
            }
            _ => {
                debug!("Other identify event: {:?}", event);
            }
        }
    }

    /// Triggers a kademlia bootstrap query to discover peers in the DHT.
    /// Called once on first identify event and periodically thereafter.
    fn attempt_kademlia_bootstrap(&mut self) {
        match self.swarm.behaviour_mut().kademlia.bootstrap() {
            Ok(_) => {
                debug!("Started Kademlia bootstrap");
                self.has_bootstrapped_kad = true;
            }
            Err(error) => {
                warn!("Failed to bootstrap Kademlia: {error}");
            }
        }
    }

    /// Attempts to derive and confirm an external address from an identify
    /// observation. Uses the observed IP combined with our listen port.
    fn try_confirm_external_address(&mut self, observed_addr: &Multiaddr) {
        if self.external_address_confirmed {
            return;
        }
        let listen_port = match self.listen_port {
            Some(port) => port,
            None => {
                debug!("No listen port configured, skipping external address detection");
                return;
            }
        };
        if let Some(external_addr) =
            address_filter::build_external_address(observed_addr, listen_port)
        {
            info!("Detected external address from peer observation: {external_addr}");
            self.swarm.add_external_address(external_addr);
            self.external_address_confirmed = true;
        } else {
            debug!("Peer observed us as {observed_addr}, not usable as external address");
        }
    }

    /// Handle ping events from the libp2p ping protocol.
    /// Ping failures are logged; libp2p handles connection closure automatically.
    fn handle_ping_event(&mut self, event: libp2p::ping::Event) {
        match event.result {
            Ok(rtt) => {
                debug!("Ping to {} succeeded, rtt: {:?}", event.peer, rtt);
                self.connection_tracker.record_ping_rtt(&event.peer, rtt);
                self.connection_tracker
                    .clear_ping_failures(event.connection);
            }
            // The handler reports this once and then stops pinging; it says the
            // peer lacks the protocol, not that the connection is dead, so it is
            // not counted towards closing the connection.
            Err(Failure::Unsupported) => {
                warn!(
                    "Peer {} on connection {} does not support ping",
                    event.peer, event.connection
                );
            }
            Err(error) => {
                let failures = self
                    .connection_tracker
                    .record_ping_failure(event.connection);
                warn!(
                    "Ping to {} on connection {} failed ({failures} consecutive): {error}",
                    event.peer, event.connection
                );
                // A connection can negotiate yet fail to exchange messages
                // Close it so the reconnector dials a fresh one; only this connection,
                // not a healthy sibling to the peer.
                if failures >= PING_FAILURE_THRESHOLD {
                    warn!(
                        "Closing unresponsive connection {} to {} after {failures} ping failures",
                        event.connection, event.peer
                    );
                    self.connection_tracker
                        .clear_ping_failures(event.connection);
                    self.connection_tracker.record_unresponsive_close();
                    self.swarm.close_connection(event.connection);
                }
            }
        }
    }

    /// Handle kademlia events, these are events that are generated by the kademlia protocol
    fn handle_kademlia_event(&mut self, event: KademliaEvent) {
        match event {
            KademliaEvent::RoutingUpdated {
                peer,
                is_new_peer,
                addresses,
                bucket_range,
                old_peer,
            } => {
                debug!(
                    "Routing updated for peer: {peer}, is_new_peer: {is_new_peer}, addresses: {addresses:?}, bucket_range: {bucket_range:?}, old_peer: {old_peer:?}"
                );
                self.outgoing_peer_target
                    .learn_addresses(peer, addresses.iter());
            }
            KademliaEvent::OutboundQueryProgressed { result, step, .. } => {
                match result {
                    QueryResult::GetClosestPeers(Ok(ok)) => {
                        debug!("Got closest peers: {:?}", ok.peers);
                    }
                    QueryResult::GetClosestPeers(Err(err)) => {
                        error!("Failed to get closest peers: {err}");
                    }
                    _ => debug!("Other query result: {:?}", result),
                }
                if step.last {
                    self.close_finished_lookups();
                }
            }
            _ => debug!("Other Kademlia event: {:?}", event),
        }
    }
}

/// This test verifies that dialing an unreachable peer does not hang indefinitely,
/// and that the libp2p Swarm emits a connection error within a reasonable timeout.
///
/// How it works:
/// - We configure the node to dial a local address on an unused TCP port (`127.0.0.1:65535`).
///   This port is almost certainly closed, so the TCP connection will either be refused immediately,
///   or will time out during the transport handshake phase.
/// - The Swarm is polled for events. We expect to receive a `SwarmEvent::OutgoingConnectionError`
///   within a few seconds.
/// - The test asserts that the error string contains either "timeout", "connection refused",
///   or "failed to negotiate transport protocol" (to cover all error variants libp2p might emit
///   for failed or timed-out dials).
/// - If no such event is received within the timeout window (e.g., 5 or 10 seconds), the test fails.
///
/// Why this is important:
/// - It ensures that the node’s dial logic and libp2p’s handshake timeout are working as intended,
///   and that attempts to connect to unreachable peers do not block or hang the node.
/// - This is critical for network robustness, as hanging dials can lead to resource exhaustion
///   or degraded peer connectivity in real-world deployments.
///
/// Note:
/// - The test does not require the error to be specifically a "timeout"; it accepts any connection
///   failure, since the exact error string may vary by OS and libp2p version.
/// - The test must not set connection limits (like `max_pending_outgoing`) to zero, or the dial
///   will be denied before it is attempted.
#[cfg(test)]
mod tests {
    use super::ChainStoreHandle;
    use super::P2PoolBehaviourEvent;
    use super::connection_tracker::BlockedIps;
    use super::dns_or_tcp_transport;
    use super::outgoing_peer_target::OutgoingPeerTarget;
    use crate::config::{
        ApiConfig, BitcoinCashNetwork, Chain, Config, ElectrumConfig, LoggingConfig, NetworkConfig,
        StoreConfig, StratumConfig,
    };
    use crate::monitoring_events::create_monitoring_event_channel;
    use crate::node::Node;
    use crate::node::p2p_message_handlers::receivers::block_receiver::create_block_receiver_channel;
    use crate::node::request_response_handler::block_fetcher::create_block_fetcher_channel;
    use crate::node::response_worker::create_response_worker_channel;
    use crate::node::validation_worker::create_validation_channel;
    use bitcoin::BlockHash;
    use bitcoin::hashes::Hash;
    use bitcoindrpc::BitcoinRpcConfig;
    use futures::StreamExt;
    use libp2p::core::transport::{DialOpts as TransportDialOpts, PortUse, Transport};
    use libp2p::core::{ConnectedPoint, Endpoint};
    use libp2p::identify;
    use libp2p::identity::Keypair;
    use libp2p::ping::Failure;
    use libp2p::swarm::{ConnectionId, DialError, SwarmEvent};
    use libp2p::tcp::Config as TcpConfig;
    use libp2p::tcp::tokio::Transport as TcpTransport;
    use libp2p::{Multiaddr, PeerId, TransportError};
    use std::io;
    use std::num::NonZeroU32;
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use tokio::time::advance;

    #[tokio::test]
    async fn test_node_dial_timeout_does_not_hang() {
        // Use a local address that will refuse connections immediately
        let unreachable_peer = "/ip4/127.0.0.1/tcp/65535".to_string(); // Fast fail

        let mut network_config = NetworkConfig {
            listen_address: "/ip4/127.0.0.1/tcp/0".to_string(),
            dial_peers: vec![],
            max_pending_incoming: 10,
            max_pending_outgoing: 10,
            max_established_incoming: 10,
            max_established_outgoing: 10,
            max_established_per_peer: 10,
            max_workbase_per_second: 10,
            max_userworkbase_per_second: 10,
            max_miningshare_per_second: 10,
            max_inventory_per_second: 10,
            max_transaction_per_second: 10,
            max_requests_per_second: 1,
            dial_timeout_secs: 2,
            blocked_ips: vec![],
            external_address: None,
            dns_seed: false,
            tor_proxy: None,
            proxy_clearnet: false,
            onlynet: None,
            node_key_path: None,
            ephemeral_node_key: true,
            nostr_relays: vec![],
            nostr_announce: false,
            target_outgoing_peers: 16,
        };
        network_config.dial_peers = vec![unreachable_peer];
        network_config.dial_timeout_secs = 2;

        let mut config = Config {
            network: network_config.clone(),
            bitcoinrpc: BitcoinRpcConfig {
                url: "http://localhost:8332".to_string(),
                username: "testuser".to_string(),
                password: "testpass".to_string(),
            },
            submit_nodes: vec![],
            store: StoreConfig {
                path: "test_chain.db".to_string(),
                pplns_ttl_days: 3,
                keep_whole_weak_shares: false,
            },

            stratum: StratumConfig::new_for_test_default(),
            logging: LoggingConfig {
                console: Some(true),
                level: "info".to_string(),
                file: Some("./p2pool.log".to_string()),
                stats_dir: "./logs/stats".to_string(),
            },
            api: ApiConfig {
                hostname: "127.0.0.1".to_string(),
                port: 3000,
                auth_user: None,
                auth_token: None,
                auth_password: None,
                cors_allowed: false,
                public_dashboard: false,
                web_root: None,
            },
            electrum: ElectrumConfig::default(),
        };
        config.network = network_config;

        let mut chain_store_handle = ChainStoreHandle::default();
        chain_store_handle
            .expect_clone()
            .returning(ChainStoreHandle::default);

        let (block_fetcher_tx, _block_fetcher_rx) = create_block_fetcher_channel();
        let (validation_tx, _validation_rx) = create_validation_channel();
        let (block_receiver_handle, _block_receiver_rx) = create_block_receiver_channel();
        let monitoring_tx = create_monitoring_event_channel();
        let (response_worker_tx, _response_worker_rx) = create_response_worker_channel();
        let mut node = Node::new(
            config.clone(),
            chain_store_handle,
            block_fetcher_tx,
            validation_tx,
            block_receiver_handle,
            monitoring_tx,
            Arc::new(crate::shares::validation::MockDefaultShareValidator::default()),
            response_worker_tx,
            None,
        )
        .expect("Node initialization failed");

        //  Initiate the dial manually!
        let unreachable_peer_multiaddr: libp2p::Multiaddr =
            config.network.dial_peers[0].parse().unwrap();
        node.swarm
            .dial(unreachable_peer_multiaddr.clone())
            .expect("Dial failed to start");

        let start = Instant::now();
        let timeout = tokio::time::sleep(Duration::from_secs(5));
        tokio::pin!(timeout);

        loop {
            tokio::select! {
                event = node.swarm.next() => {
                    match event {
                        Some(SwarmEvent::OutgoingConnectionError { error, .. }) => {
                            let elapsed = start.elapsed();
                            let err_str = error.to_string();
                            let err_str_lower = err_str.to_lowercase();
                            assert!(
                                err_str_lower.contains("timeout")
                                    || err_str_lower.contains("connection refused")
                                    || err_str_lower
                                        .contains("failed to negotiate transport protocol"),
                                "Expected timeout or connection refused error, got: {err_str}"
                            );
                            assert!(
                                elapsed.as_secs_f32() <= 10.0,
                                "Dialing took too long: {elapsed:?}, expected ~10s"
                            );
                            break;
                        }
                        Some(_) => continue,
                        None => panic!("Swarm event stream ended unexpectedly"),
                    }
                }
                _ = &mut timeout => {
                    panic!("Test timed out after 5 seconds, dial timeout not triggered");
                }
            }
        }
    }

    fn build_test_config(listen_address: &str, external_address: Option<String>) -> Config {
        Config {
            network: NetworkConfig {
                listen_address: listen_address.to_string(),
                dial_peers: vec![],
                max_pending_incoming: 10,
                max_pending_outgoing: 10,
                max_established_incoming: 10,
                max_established_outgoing: 10,
                max_established_per_peer: 10,
                max_workbase_per_second: 10,
                max_userworkbase_per_second: 10,
                max_miningshare_per_second: 10,
                max_inventory_per_second: 10,
                max_transaction_per_second: 10,
                max_requests_per_second: 1,
                dial_timeout_secs: 2,
                blocked_ips: vec![],
                external_address,
                dns_seed: false,
                tor_proxy: None,
                proxy_clearnet: false,
                onlynet: None,
                node_key_path: None,
                ephemeral_node_key: true,
                nostr_relays: vec![],
                nostr_announce: false,
                target_outgoing_peers: 16,
            },
            bitcoinrpc: BitcoinRpcConfig {
                url: "http://localhost:8332".to_string(),
                username: "testuser".to_string(),
                password: "testpass".to_string(),
            },
            submit_nodes: vec![],
            store: StoreConfig {
                path: "test_chain.db".to_string(),
                pplns_ttl_days: 3,
                keep_whole_weak_shares: false,
            },
            stratum: StratumConfig::new_for_test_default(),
            logging: LoggingConfig {
                console: Some(true),
                level: "info".to_string(),
                file: Some("./p2pool.log".to_string()),
                stats_dir: "./logs/stats".to_string(),
            },
            api: ApiConfig {
                hostname: "127.0.0.1".to_string(),
                port: 3000,
                auth_user: None,
                auth_token: None,
                auth_password: None,
                cors_allowed: false,
                public_dashboard: false,
                web_root: None,
            },
            electrum: ElectrumConfig::default(),
        }
    }

    fn build_test_node(config: Config) -> Node {
        try_build_test_node(config).expect("Node initialization failed")
    }

    fn try_build_test_node(config: Config) -> Result<Node, Box<dyn std::error::Error>> {
        let mut chain_store_handle = ChainStoreHandle::default();
        chain_store_handle
            .expect_clone()
            .returning(ChainStoreHandle::default);
        let (block_fetcher_tx, _block_fetcher_rx) = create_block_fetcher_channel();
        let (validation_tx, _validation_rx) = create_validation_channel();
        let (block_receiver_handle, _block_receiver_rx) = create_block_receiver_channel();
        let monitoring_tx = create_monitoring_event_channel();
        let (response_worker_tx, _response_worker_rx) = create_response_worker_channel();
        Node::new(
            config,
            chain_store_handle,
            block_fetcher_tx,
            validation_tx,
            block_receiver_handle,
            monitoring_tx,
            Arc::new(crate::shares::validation::MockDefaultShareValidator::default()),
            response_worker_tx,
            None,
        )
    }

    /// End to end on regtest: node A announces its address on a relay, and
    /// node B, which knows no peer, finds A there and connects to it.
    #[tokio::test]
    async fn a_node_with_no_peers_finds_one_through_a_nostr_relay() {
        use crate::node::nostr_seeds::tests::spawn_relay;
        let (relay_url, relay) = spawn_relay().await;
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let regtest = |mut config: Config| {
            config.stratum.network = bitcoin::Network::Regtest;
            config.network.nostr_relays = vec![relay_url.clone()];
            config
        };
        let mut config_a = regtest(build_test_config(
            &format!("/ip4/127.0.0.1/tcp/{port}"),
            Some(format!("/ip4/127.0.0.1/tcp/{port}")),
        ));
        config_a.network.nostr_announce = true;
        let config_b = regtest(build_test_config("/ip4/127.0.0.1/tcp/0", None));

        let mut node_a = build_test_node(config_a);
        let mut node_b = build_test_node(config_b);
        let peer_a = *node_a.swarm.local_peer_id();
        assert!(node_b.nostr_seeds.as_ref().unwrap().announce.is_empty());
        node_a.start_nostr_announcer();

        let found = tokio::time::timeout(Duration::from_secs(20), async {
            let mut asked = false;
            loop {
                if !asked && !relay.events.lock().unwrap().is_empty() {
                    node_b.look_up_nostr_peers_if_short();
                    asked = true;
                }
                tokio::select! {
                    _ = node_a.swarm.select_next_some() => {}
                    event = node_b.swarm.select_next_some() => {
                        if let SwarmEvent::ConnectionEstablished { peer_id, .. } = event
                            && peer_id == peer_a
                        {
                            return;
                        }
                    }
                    Some(send) = node_b.swarm_rx.recv() => {
                        if let crate::node::SwarmSend::Dial(address) = send {
                            node_b.swarm.dial(address).unwrap();
                        }
                    }
                    () = tokio::time::sleep(Duration::from_millis(50)) => {}
                }
            }
        })
        .await;

        assert!(found.is_ok(), "node B never connected to node A");
        node_a.shutdown().unwrap();
        assert!(node_a.nostr_announcer.is_none());
    }

    /// End to end on loopback: node A dials B, B dials C, and A's Kademlia
    /// lookup through B dials C. Once no query runs, A closes that lookup
    /// connection, keeps its dial_peer B, and keeps C in its routing table.
    #[tokio::test]
    async fn a_lookup_connection_closes_and_its_peer_stays_in_the_routing_table() {
        let listener_b = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let listener_c = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address_b = format!(
            "/ip4/127.0.0.1/tcp/{}",
            listener_b.local_addr().unwrap().port()
        );
        let address_c = format!(
            "/ip4/127.0.0.1/tcp/{}",
            listener_c.local_addr().unwrap().port()
        );
        drop((listener_b, listener_c));
        let mut node_c = build_test_node(build_test_config(&address_c, None));
        let mut config_b = build_test_config(&address_b, None);
        config_b.network.dial_peers = vec![address_c];
        let mut node_b = build_test_node(config_b);
        let mut config_a = build_test_config("/ip4/127.0.0.1/tcp/0", None);
        config_a.network.dial_peers = vec![address_b];
        let mut node_a = build_test_node(config_a.clone());
        node_a.outgoing_peer_target =
            OutgoingPeerTarget::new(&config_a.network, BlockedIps::default())
                .with_lookup_settle(Duration::from_millis(300));
        node_a
            .chain_store_handle
            .expect_get_tip_height()
            .returning(|| Ok(None));
        node_a
            .chain_store_handle
            .expect_get_chain_tip()
            .returning(|| Ok(BlockHash::all_zeros()));
        let peer_a = *node_a.swarm.local_peer_id();
        let peer_b = *node_b.swarm.local_peer_id();
        let peer_c = *node_c.swarm.local_peer_id();

        let mut tick = tokio::time::interval(Duration::from_millis(100));
        let mut lookup_opened = false;
        let mut lookup_was_share_chain_peer = false;
        let mut c_got_a_request = false;
        let closed = tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                tokio::select! {
                    event = node_a.swarm.select_next_some() => {
                        let mut lookup_closed = false;
                        match &event {
                            SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. }
                                if *peer_id == peer_c =>
                            {
                                assert!(endpoint.is_dialer(), "A dials C for its lookup");
                                lookup_opened = true;
                            }
                            SwarmEvent::ConnectionClosed { peer_id, .. } if *peer_id == peer_c => {
                                lookup_closed = lookup_opened;
                            }
                            _ => {}
                        }
                        node_a.handle_swarm_event(event).unwrap();
                        lookup_was_share_chain_peer |=
                            node_a.share_chain_peers().contains(&peer_c);
                        if lookup_closed {
                            return;
                        }
                    }
                    _ = node_b.swarm.select_next_some() => {}
                    event = node_c.swarm.select_next_some() => {
                        // A sends nothing of the share chain on its lookup.
                        if let SwarmEvent::Behaviour(P2PoolBehaviourEvent::RequestResponse(
                            libp2p::request_response::Event::Message {
                                peer,
                                message: libp2p::request_response::Message::Request { .. },
                                ..
                            },
                        )) = &event
                        {
                            c_got_a_request |= *peer == peer_a;
                        }
                    }
                    _ = tick.tick() => node_a.close_finished_lookups(),
                }
            }
        })
        .await;
        assert!(
            !c_got_a_request,
            "A sends no handshake or request on its lookup"
        );
        assert!(
            !lookup_was_share_chain_peer,
            "share-chain traffic never goes to a lookup-only peer"
        );

        assert!(closed.is_ok(), "A never closed its lookup connection to C");
        assert!(node_a.swarm.is_connected(&peer_b), "the dial_peer stays");
        assert!(!node_a.swarm.is_connected(&peer_c));
        let routing_table_has_c = node_a
            .swarm
            .behaviour_mut()
            .kademlia
            .kbuckets()
            .any(|bucket| {
                bucket
                    .iter()
                    .any(|entry| *entry.node.key.preimage() == peer_c)
            });
        assert!(routing_table_has_c, "C stays in A's routing table");
    }

    /// End to end on loopback, one connection allowed per peer: B dials A,
    /// dials A again, and B's connection limits refuse the second connection.
    /// Every request B then sends A is answered. With request-response asked
    /// before the limits, it kept the refused connection, and each request it
    /// routed there got no answer, no timeout and no failure (swarm run
    /// 44468164275, where restarted nodes lost most of their requests so).
    #[tokio::test]
    async fn every_request_is_answered_after_the_limits_refuse_a_connection() {
        use crate::node::messages::{GetData, Message};
        use libp2p::request_response;
        use libp2p::swarm::dial_opts::{DialOpts, PeerCondition};
        use std::collections::HashSet;

        const REQUESTS: usize = 20;
        let listener_a = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address_a = format!(
            "/ip4/127.0.0.1/tcp/{}",
            listener_a.local_addr().unwrap().port()
        );
        drop(listener_a);
        let mut config_a = build_test_config(&address_a, None);
        config_a.network.max_established_per_peer = 1;
        let mut node_a = build_test_node(config_a);
        let mut config_b = build_test_config("/ip4/127.0.0.1/tcp/0", None);
        config_b.network.max_established_per_peer = 1;
        let mut node_b = build_test_node(config_b);
        let peer_a = *node_a.swarm.local_peer_id();
        let address_a: Multiaddr = address_a.parse().unwrap();
        let dial_a = || {
            DialOpts::peer_id(peer_a)
                .addresses(vec![address_a.clone()])
                .condition(PeerCondition::Always)
                .allocate_new_port()
                .build()
        };

        let mut second_dial_refused = false;
        let mut pending: HashSet<request_response::OutboundRequestId> = HashSet::new();
        let mut answered = 0;
        let outcome = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                tokio::select! {
                    event = node_a.swarm.select_next_some() => match event {
                        SwarmEvent::NewListenAddr { .. } => node_b.swarm.dial(dial_a()).unwrap(),
                        SwarmEvent::Behaviour(P2PoolBehaviourEvent::RequestResponse(
                            request_response::Event::Message {
                                message: request_response::Message::Request { channel, .. },
                                ..
                            },
                        )) => {
                            let _ = node_a
                                .swarm
                                .behaviour_mut()
                                .request_response
                                .send_response(channel, Message::Ack);
                        }
                        _ => {}
                    },
                    event = node_b.swarm.select_next_some() => match event {
                        SwarmEvent::ConnectionEstablished { peer_id, .. } if peer_id == peer_a => {
                            node_b.swarm.dial(dial_a()).unwrap();
                        }
                        SwarmEvent::OutgoingConnectionError {
                            error: DialError::Denied { .. },
                            ..
                        } => {
                            second_dial_refused = true;
                            for _ in 0..REQUESTS {
                                let request = Message::GetData(GetData::Block(BlockHash::all_zeros()));
                                pending.insert(
                                    node_b
                                        .swarm
                                        .behaviour_mut()
                                        .request_response
                                        .send_request(&peer_a, request),
                                );
                            }
                        }
                        SwarmEvent::Behaviour(P2PoolBehaviourEvent::RequestResponse(
                            request_response::Event::Message {
                                message: request_response::Message::Response { request_id, .. },
                                ..
                            },
                        )) => {
                            pending.remove(&request_id);
                            answered += 1;
                        }
                        SwarmEvent::Behaviour(P2PoolBehaviourEvent::RequestResponse(
                            request_response::Event::OutboundFailure { request_id, .. },
                        )) => {
                            pending.remove(&request_id);
                        }
                        _ => {}
                    },
                }
                if second_dial_refused && pending.is_empty() {
                    return;
                }
            }
        })
        .await;

        assert!(
            second_dial_refused,
            "B's limits refuse the second connection"
        );
        assert!(
            outcome.is_ok(),
            "{} of {REQUESTS} requests got no answer, timeout or failure",
            pending.len()
        );
        assert_eq!(answered, REQUESTS, "every request is answered");
    }

    /// A node saves the peers it knows beside its store, and a node started
    /// again on that store knows them; an ephemeral node writes no book.
    #[tokio::test]
    async fn a_restarted_node_knows_the_peers_it_saved() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = build_test_config("/ip4/127.0.0.1/tcp/0", None);
        config.store.path = dir.path().join("store.db").display().to_string();
        config.network.ephemeral_node_key = false;
        let peer = PeerId::random();

        let mut node = build_test_node(config.clone());
        node.outgoing_peer_target
            .learn_addresses(peer, [&"/ip4/1.2.3.4/tcp/6884".parse().unwrap()]);
        node.save_peer_book();
        let book = dir.path().join(crate::config::DEFAULT_PEER_BOOK_FILE);
        assert!(book.exists());
        drop(node);

        let restarted = build_test_node(config.clone());
        assert!(
            restarted
                .outgoing_peer_target
                .saved_book()
                .peers
                .iter()
                .any(|saved| saved.peer_id == peer.to_string())
        );

        std::fs::remove_file(&book).unwrap();
        config.network.ephemeral_node_key = true;
        build_test_node(config).save_peer_book();
        assert!(!book.exists());
    }

    /// Two nodes whose stores share a directory share the default key file.
    /// The second is refused at start while the first runs, where it used to
    /// take the first one's peer ID; with a key of its own it is still refused
    /// on the peer book beside the stores. In its own directory, or once the
    /// first has stopped, it starts.
    #[tokio::test]
    async fn a_second_node_in_one_directory_is_refused_while_the_first_runs() {
        let dir = tempfile::tempdir().unwrap();
        let mut first_config = build_test_config("/ip4/127.0.0.1/tcp/0", None);
        first_config.store.path = dir.path().join("store-a.db").display().to_string();
        first_config.network.ephemeral_node_key = false;
        let mut second_config = first_config.clone();
        second_config.store.path = dir.path().join("store-b.db").display().to_string();

        let refusal = |config: Config| match try_build_test_node(config) {
            Ok(_) => panic!("a second node started beside the first"),
            Err(error) => error.to_string(),
        };

        let first = build_test_node(first_config);
        let error = refusal(second_config.clone());
        assert!(
            error.contains("Another node is running on the node key"),
            "{error}"
        );

        let mut own_key = second_config.clone();
        own_key.network.node_key_path =
            Some(dir.path().join("b.p2p_node_key").display().to_string());
        let error = refusal(own_key);
        assert!(
            error.contains("Another node keeps its peer list"),
            "{error}"
        );

        let mut own_directory = second_config.clone();
        own_directory.store.path = dir.path().join("b").join("store.db").display().to_string();
        let second = build_test_node(own_directory);
        assert_ne!(first.swarm.local_peer_id(), second.swarm.local_peer_id());

        let first_peer = *first.swarm.local_peer_id();
        drop(first);
        let after = build_test_node(second_config);
        assert_eq!(*after.swarm.local_peer_id(), first_peer);
    }

    /// A node restarted on the same key file has the same peer ID; one with
    /// `ephemeral_node_key` gets a new one each start.
    #[tokio::test]
    async fn a_restarted_node_keeps_its_peer_id() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = build_test_config("/ip4/127.0.0.1/tcp/0", None);
        config.store.path = dir.path().join("store.db").display().to_string();
        config.network.ephemeral_node_key = false;

        let first = *build_test_node(config.clone()).swarm.local_peer_id();
        assert!(
            dir.path()
                .join(crate::config::DEFAULT_NODE_KEY_FILE)
                .exists()
        );
        let restarted = *build_test_node(config.clone()).swarm.local_peer_id();
        assert_eq!(first, restarted);

        config.network.ephemeral_node_key = true;
        let ephemeral = *build_test_node(config.clone()).swarm.local_peer_id();
        let ephemeral_again = *build_test_node(config).swarm.local_peer_id();
        assert_ne!(ephemeral, first);
        assert_ne!(ephemeral, ephemeral_again);
    }

    #[tokio::test]
    async fn test_config_external_address_is_added_to_swarm() {
        let config = build_test_config(
            "/ip4/127.0.0.1/tcp/0",
            Some("/ip4/203.0.113.5/tcp/6884".to_string()),
        );
        let node = build_test_node(config);

        let external_addrs: Vec<_> = node.swarm.external_addresses().cloned().collect();
        let expected: libp2p::Multiaddr = "/ip4/203.0.113.5/tcp/6884".parse().unwrap();
        assert!(
            external_addrs.contains(&expected),
            "Expected external address {expected} not found in {external_addrs:?}"
        );
        assert!(node.external_address_confirmed);
    }

    #[tokio::test]
    async fn test_dns_seeds_enabled_on_network_with_seeds() {
        let mut config = build_test_config("", None);
        config.stratum.network = bitcoin::Network::Testnet4;
        config.network.dns_seed = true;
        let node = build_test_node(config);

        assert!(node.dns_seeds.is_some());
    }

    #[tokio::test]
    async fn test_dns_seeds_disabled_by_config() {
        let mut config = build_test_config("", None);
        config.stratum.network = bitcoin::Network::Testnet4;
        config.network.dns_seed = false;
        let node = build_test_node(config);

        assert!(node.dns_seeds.is_none());
    }

    /// A Bitcoin Cash node gets no DNS seeds even with `dns_seed` on and a
    /// network that has Bitcoin seeds: those seeds are Bitcoin nodes.
    #[tokio::test]
    async fn test_dns_seeds_disabled_on_bitcoin_cash() {
        let mut config = build_test_config("", None);
        config.stratum.chain = Chain::BitcoinCash;
        config.stratum.bitcoin_cash_network = Some(BitcoinCashNetwork::Chipnet);
        config.stratum.network = bitcoin::Network::Testnet4;
        config.network.dns_seed = true;
        let node = build_test_node(config);

        assert!(node.dns_seeds.is_none());
    }

    /// A node that does not listen still dials its configured peers.
    #[tokio::test]
    async fn test_dial_peers_are_dialed_without_listen_address() {
        let mut config = build_test_config("", None);
        config.network.dial_peers = vec!["/ip4/127.0.0.1/tcp/65535".to_string()];
        let node = build_test_node(config);

        assert_eq!(
            node.swarm
                .network_info()
                .connection_counters()
                .num_pending_outgoing(),
            1
        );
    }

    /// Without a usable system resolver config the node falls back to plain
    /// TCP: `/ip4` dials are accepted and `/dns` dials are refused, instead of
    /// the node failing to start.
    #[tokio::test]
    async fn test_dns_or_tcp_transport_falls_back_to_tcp_without_resolver_config() {
        let mut transport = dns_or_tcp_transport::<TcpTransport>(
            Err(io::Error::other("no nameservers found in config")),
            TcpTransport::new(TcpConfig::default()),
        );
        let dial_opts = TransportDialOpts {
            role: Endpoint::Dialer,
            port_use: PortUse::New,
        };

        let ip_address: Multiaddr = "/ip4/127.0.0.1/tcp/65535".parse().unwrap();
        assert!(transport.dial(ip_address, dial_opts).is_ok());

        let dns_address: Multiaddr = "/dns4/localhost/tcp/65535".parse().unwrap();
        assert!(matches!(
            transport.dial(dns_address, dial_opts),
            Err(TransportError::MultiaddrNotSupported(_))
        ));
    }

    /// A dial_peer hostname that resolves to a blocked IP is refused by the
    /// transport before any TCP connection is opened. The listener stands in
    /// for the peer: it must never see a connection.
    #[tokio::test]
    async fn test_dns_dial_to_blocked_ip_is_refused_before_connecting() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut config = build_test_config("", None);
        config.network.dial_peers = vec![format!("/dns4/localhost/tcp/{port}")];
        config.network.blocked_ips = vec!["127.0.0.1".to_string()];
        let mut node = build_test_node(config);

        let dial_error = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(SwarmEvent::OutgoingConnectionError { error, .. }) =
                    node.swarm.next().await
                {
                    return error;
                }
            }
        })
        .await
        .expect("Dial should fail");

        assert!(
            tokio::time::timeout(Duration::from_millis(200), listener.accept())
                .await
                .is_err(),
            "No TCP connection should reach a blocked IP, dial error: {dial_error}"
        );
    }

    /// A failed dial_peer dial reported by the swarm reaches the reconnector,
    /// which ends the pending dial and backs off from 5s to 10s.
    #[tokio::test(start_paused = true)]
    async fn test_outgoing_connection_error_backs_off_dial_peer() {
        let mut config = build_test_config("", None);
        config.network.dial_peers = vec!["/ip4/127.0.0.1/tcp/65535".to_string()];
        let mut node = build_test_node(config);
        let address: Multiaddr = "/ip4/127.0.0.1/tcp/65535".parse().unwrap();
        let connection_id = node
            .peer_reconnector
            .pending_dial(&address)
            .expect("Node::new should have dialed the dial_peer");

        let event = SwarmEvent::OutgoingConnectionError {
            connection_id,
            peer_id: None,
            error: DialError::Transport(vec![(
                address.clone(),
                TransportError::Other(io::Error::other("connection refused")),
            )]),
        };
        node.handle_swarm_event(event).unwrap();

        advance(Duration::from_secs(9)).await;
        assert!(
            node.peer_reconnector.addresses_to_reconnect(&[]).is_empty(),
            "Should still be in 10s backoff"
        );
        advance(Duration::from_secs(2)).await;
        assert_eq!(
            node.peer_reconnector.addresses_to_reconnect(&[]),
            vec![address]
        );
    }

    /// A peer the node failed to reach and has no connection to leaves
    /// Kademlia's routing table, so a node that has gone, or came back under
    /// another ID, is not handed to lookups and dialed again; a dial the node
    /// itself declined keeps the peer.
    #[tokio::test]
    async fn an_unreachable_peer_leaves_the_routing_table() {
        let mut node = build_test_node(build_test_config("", None));
        let address: Multiaddr = "/ip4/11.6.0.1/tcp/7005".parse().unwrap();
        let in_routing_table = |node: &mut Node, peer_id: PeerId| {
            node.swarm
                .behaviour_mut()
                .kademlia
                .kbuckets()
                .any(|bucket| {
                    bucket
                        .iter()
                        .any(|entry| *entry.node.key.preimage() == peer_id)
                })
        };
        let gone = PeerId::random();
        let declined = PeerId::random();
        for peer_id in [gone, declined] {
            node.swarm
                .behaviour_mut()
                .add_address(peer_id, address.clone());
            assert!(in_routing_table(&mut node, peer_id));
        }

        node.handle_swarm_event(SwarmEvent::OutgoingConnectionError {
            connection_id: ConnectionId::new_unchecked(1),
            peer_id: Some(gone),
            error: DialError::WrongPeerId {
                obtained: PeerId::random(),
                address: address.clone(),
            },
        })
        .unwrap();
        node.handle_swarm_event(SwarmEvent::OutgoingConnectionError {
            connection_id: ConnectionId::new_unchecked(2),
            peer_id: Some(declined),
            error: DialError::Aborted,
        })
        .unwrap();

        assert!(
            !in_routing_table(&mut node, gone),
            "the gone peer is forgotten"
        );
        assert!(
            in_routing_table(&mut node, declined),
            "a declined dial keeps it"
        );
    }

    #[tokio::test]
    async fn test_no_config_external_address_leaves_unconfirmed() {
        let config = build_test_config("/ip4/127.0.0.1/tcp/0", None);
        let node = build_test_node(config);

        let external_addrs: Vec<_> = node.swarm.external_addresses().cloned().collect();
        assert!(
            external_addrs.is_empty(),
            "Expected no external addresses, got {external_addrs:?}"
        );
        assert!(!node.external_address_confirmed);
    }

    #[tokio::test]
    async fn test_try_confirm_from_routable_observation() {
        let config = build_test_config("/ip4/127.0.0.1/tcp/0", None);
        let mut node = build_test_node(config);
        node.listen_port = Some(7001);

        assert!(!node.external_address_confirmed);

        let observed: libp2p::Multiaddr = "/ip4/93.184.216.34/tcp/54321".parse().unwrap();
        node.try_confirm_external_address(&observed);

        assert!(node.external_address_confirmed);
        let external_addrs: Vec<_> = node.swarm.external_addresses().cloned().collect();
        let expected: libp2p::Multiaddr = "/ip4/93.184.216.34/tcp/7001".parse().unwrap();
        assert!(
            external_addrs.contains(&expected),
            "Expected external address {expected} not found in {external_addrs:?}"
        );
    }

    #[tokio::test]
    async fn test_try_confirm_ignores_private_observation() {
        let config = build_test_config("/ip4/127.0.0.1/tcp/0", None);
        let mut node = build_test_node(config);
        node.listen_port = Some(7002);

        let observed: libp2p::Multiaddr = "/ip4/192.168.1.1/tcp/54321".parse().unwrap();
        node.try_confirm_external_address(&observed);

        assert!(!node.external_address_confirmed);
        let external_addrs: Vec<_> = node.swarm.external_addresses().cloned().collect();
        assert!(
            external_addrs.is_empty(),
            "Expected no external addresses, got {external_addrs:?}"
        );
    }

    #[tokio::test]
    async fn test_try_confirm_noop_when_already_confirmed() {
        let config = build_test_config(
            "/ip4/127.0.0.1/tcp/0",
            Some("/ip4/203.0.113.5/tcp/7003".to_string()),
        );
        let mut node = build_test_node(config);
        node.listen_port = Some(7003);
        assert!(node.external_address_confirmed);

        let observed: libp2p::Multiaddr = "/ip4/198.51.100.99/tcp/54321".parse().unwrap();
        node.try_confirm_external_address(&observed);

        let external_addrs: Vec<_> = node.swarm.external_addresses().cloned().collect();
        let should_not_exist: libp2p::Multiaddr = "/ip4/198.51.100.99/tcp/7003".parse().unwrap();
        assert!(
            !external_addrs.contains(&should_not_exist),
            "Second observation should not override confirmed address"
        );
    }

    #[tokio::test]
    async fn test_try_confirm_noop_when_no_listen_port() {
        let config = build_test_config("/ip4/127.0.0.1/tcp/0", None);
        let mut node = build_test_node(config);
        assert!(node.listen_port.is_none());

        let observed: libp2p::Multiaddr = "/ip4/198.51.100.7/tcp/54321".parse().unwrap();
        node.try_confirm_external_address(&observed);

        assert!(!node.external_address_confirmed);
    }

    #[tokio::test]
    async fn test_new_listen_addr_resolves_ephemeral_port() {
        let config = build_test_config("/ip4/127.0.0.1/tcp/0", None);
        let mut node = build_test_node(config);
        assert!(node.listen_port.is_none());

        let resolved_addr: libp2p::Multiaddr = "/ip4/127.0.0.1/tcp/45678".parse().unwrap();
        let event = SwarmEvent::NewListenAddr {
            listener_id: libp2p::core::transport::ListenerId::next(),
            address: resolved_addr,
        };
        node.handle_swarm_event(event).unwrap();

        assert_eq!(node.listen_port, Some(45678));
    }

    fn ping_failure(peer: PeerId, connection: ConnectionId) -> libp2p::ping::Event {
        libp2p::ping::Event {
            peer,
            connection,
            result: Err(Failure::Timeout),
        }
    }

    #[tokio::test]
    async fn test_ping_threshold_closes_connection_and_clears_count() {
        let mut node = build_test_node(build_test_config("/ip4/127.0.0.1/tcp/0", None));
        let peer = PeerId::random();
        let connection = ConnectionId::new_unchecked(7);

        node.handle_ping_event(ping_failure(peer, connection));
        node.handle_ping_event(ping_failure(peer, connection));
        assert_eq!(node.connection_tracker.ping_failures(connection), 2);

        // The third reported failure reaches the threshold: the connection is
        // closed and its count reset.
        node.handle_ping_event(ping_failure(peer, connection));
        assert_eq!(node.connection_tracker.ping_failures(connection), 0);

        let health = node.p2p_health();
        assert_eq!(health.ping_failures_total, 3);
        assert_eq!(health.connections_closed_unresponsive_total, 1);
    }

    #[tokio::test]
    async fn test_ping_success_resets_failure_count() {
        let mut node = build_test_node(build_test_config("/ip4/127.0.0.1/tcp/0", None));
        let peer = PeerId::random();
        let connection = ConnectionId::new_unchecked(7);

        node.handle_ping_event(ping_failure(peer, connection));
        node.handle_ping_event(ping_failure(peer, connection));
        assert_eq!(node.connection_tracker.ping_failures(connection), 2);

        node.handle_ping_event(libp2p::ping::Event {
            peer,
            connection,
            result: Ok(Duration::from_millis(20)),
        });
        assert_eq!(node.connection_tracker.ping_failures(connection), 0);
    }

    #[tokio::test]
    async fn test_ping_unsupported_is_not_counted() {
        let mut node = build_test_node(build_test_config("/ip4/127.0.0.1/tcp/0", None));
        let peer = PeerId::random();
        let connection = ConnectionId::new_unchecked(7);

        node.handle_ping_event(libp2p::ping::Event {
            peer,
            connection,
            result: Err(Failure::Unsupported),
        });
        assert_eq!(node.connection_tracker.ping_failures(connection), 0);

        node.handle_ping_event(ping_failure(peer, connection));
        assert_eq!(node.connection_tracker.ping_failures(connection), 1);
    }

    fn connection_closed(
        peer_id: PeerId,
        connection: u64,
        remaining: u32,
    ) -> SwarmEvent<P2PoolBehaviourEvent> {
        SwarmEvent::ConnectionClosed {
            peer_id,
            connection_id: ConnectionId::new_unchecked(connection as usize),
            endpoint: libp2p::core::ConnectedPoint::Dialer {
                address: "/ip4/1.2.3.4/tcp/46884".parse().unwrap(),
                role_override: libp2p::core::Endpoint::Dialer,
                port_use: libp2p::core::transport::PortUse::New,
            },
            num_established: remaining,
            cause: None,
        }
    }

    /// Closing one of two connections to a peer must keep its request service:
    /// removing it would drop the peer's queued requests while it is still
    /// connected. Only the last close tears the peer down.
    #[tokio::test]
    async fn test_peer_torn_down_only_when_last_connection_closes() {
        let mut node = build_test_node(build_test_config("/ip4/127.0.0.1/tcp/0", None));
        let peer = PeerId::random();
        node.request_response_handler.add_peer(peer);

        node.handle_swarm_event(connection_closed(peer, 1, 1))
            .unwrap();
        assert!(node.request_response_handler.has_peer(&peer));

        node.handle_swarm_event(connection_closed(peer, 2, 0))
            .unwrap();
        assert!(!node.request_response_handler.has_peer(&peer));
    }

    /// Below its outgoing target the node dials a peer that Identify reported
    /// at a routable address. The swarm is never polled, so the dial stays
    /// pending and no packet is sent.
    #[tokio::test]
    async fn test_outgoing_target_dials_a_peer_identify_reported() {
        let mut node = build_test_node(build_test_config("/ip4/127.0.0.1/tcp/0", None));
        let public_key = Keypair::generate_ed25519().public();
        let peer_id = public_key.to_peer_id();
        let info = identify::Info {
            public_key,
            protocol_version: "test".to_string(),
            agent_version: "p2poolv2/test".to_string(),
            listen_addrs: vec!["/ip4/1.2.3.4/tcp/6884".parse().unwrap()],
            protocols: vec![],
            observed_addr: "/ip4/127.0.0.1/tcp/1".parse().unwrap(),
            signed_peer_record: None,
        };
        node.handle_identify_event(identify::Event::Received {
            connection_id: ConnectionId::new_unchecked(1),
            peer_id,
            info,
        });

        node.dial_toward_outgoing_target();

        let counters = node.swarm.network_info();
        assert_eq!(counters.connection_counters().num_pending_outgoing(), 1);
        assert_eq!(node.p2p_health().outgoing_target_dials_total, 1);
    }

    fn connection_established(
        peer_id: PeerId,
        connection: usize,
        endpoint: ConnectedPoint,
    ) -> SwarmEvent<P2PoolBehaviourEvent> {
        SwarmEvent::ConnectionEstablished {
            peer_id,
            connection_id: ConnectionId::new_unchecked(connection),
            endpoint,
            num_established: NonZeroU32::new(1).unwrap(),
            concurrent_dial_errors: None,
            established_in: Duration::from_millis(10),
        }
    }

    /// /metrics splits the connected peers into those this node dialed and
    /// those that only dialed it.
    #[tokio::test]
    async fn test_p2p_health_splits_connected_peers_into_outgoing_and_incoming() {
        let mut node = build_test_node(build_test_config("/ip4/127.0.0.1/tcp/0", None));
        node.chain_store_handle
            .expect_get_tip_height()
            .returning(|| Ok(None));
        node.chain_store_handle
            .expect_get_chain_tip()
            .returning(|| Ok(BlockHash::all_zeros()));
        let outgoing = PeerId::random();
        let incoming = PeerId::random();

        node.handle_swarm_event(connection_established(
            outgoing,
            1,
            ConnectedPoint::Dialer {
                address: "/ip4/1.2.3.4/tcp/6884".parse().unwrap(),
                role_override: Endpoint::Dialer,
                port_use: PortUse::Reuse,
            },
        ))
        .unwrap();
        node.handle_swarm_event(connection_established(
            incoming,
            2,
            ConnectedPoint::Listener {
                local_addr: "/ip4/0.0.0.0/tcp/6884".parse().unwrap(),
                send_back_addr: "/ip4/5.6.7.8/tcp/51234".parse().unwrap(),
            },
        ))
        .unwrap();

        let health = node.p2p_health();
        assert_eq!(health.connected_peers, 2);
        assert_eq!(health.outgoing_peers, 1);
        assert_eq!(health.incoming_peers, 1);

        node.handle_swarm_event(connection_closed(outgoing, 1, 0))
            .unwrap();
        let health = node.p2p_health();
        assert_eq!(health.outgoing_peers, 0);
        assert_eq!(health.incoming_peers, 1);
    }
}
