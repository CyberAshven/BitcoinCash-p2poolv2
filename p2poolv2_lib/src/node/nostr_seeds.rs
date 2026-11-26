// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Nostr announcements: a way for a node to find its first peers that no
//! single party holds.
//!
//! A node its operator says is reachable (`external_address`) publishes its
//! addresses on several Nostr relays, and a node short of peers asks those
//! relays for the addresses others published. Relays can drop an
//! announcement but cannot forge one, since each is signed, and no relay is
//! needed once a node has peers.
//!
//! An announcement is an addressable app-data event (NIP-78, kind 30078):
//! one per node and network, which the node replaces each time it publishes.
//! - Its `d` tag names the network and the hash of the node's network
//!   protocol string (`announcement_tag`), which every share-chain rule
//!   change alters, so a node only finds peers on its own rules.
//! - Each `addr` tag is one multiaddr ending in the node's peer ID, which the
//!   libp2p handshake then proves.
//! - An `expiration` tag (NIP-40) makes relays drop it after
//!   [`ANNOUNCEMENT_LIFETIME`], and the node publishes again every
//!   [`ANNOUNCE_EVERY`].
//! - A proof of work of [`MIN_POW_BITS`] on its id (NIP-13) prices spam, and an
//!   `alt` tag (NIP-31) says what it is to clients that show unknown events.
//!
//! The event is signed with a key derived from the node's libp2p key
//! (`nostr_keys`), so the node needs no second key file. A reader checks each
//! event's id, signature, proof of work and age, ignores its own, and dials at
//! most [`MAX_NOSTR_DIALS`] of the newest peers. Announced addresses are only
//! dial candidates: the noise handshake with the network protocol prologue
//! decides whether a peer is on our network, as for DNS seeds.
//!
//! A node that opens no clearnet connection of its own (`proxy_clearnet`, or
//! an onion-only `onlynet`) reaches its relays through Tor, the name of the
//! relay unresolved, so neither a relay nor a resolver sees its IP: through
//! `tor_proxy`, or the default Tor SOCKS port once the Tor control port has
//! made its onion service, and not at all before. An onion-only node
//! (`onion_only`, or such an `onlynet`) announces only when it reaches its
//! relays through Tor: a relay it reached directly would see its IP beside
//! its onion address.

use super::SwarmSend;
use super::address_filter;
use super::onion::OnionProxy;
use crate::stratum::sv2::socks5;
use bitcoin::hashes::{Hash, sha256};
use bitcoin::secp256k1::{self, Keypair, Message, Secp256k1, SecretKey, XOnlyPublicKey};
use futures::{SinkExt, StreamExt, future::join_all};
use libp2p::multiaddr::Protocol;
use libp2p::{Multiaddr, PeerId};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, watch};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::Uri;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tracing::{debug, info, warn};

/// NIP-78's addressable kind for application data.
pub(crate) const APP_DATA_KIND: u32 = 30078;

/// Leading zero bits an announcement's id needs (NIP-13). About 65,000
/// hashes, a few milliseconds for a node, so a flood of fake announcements
/// costs its sender work per event.
pub(crate) const MIN_POW_BITS: u32 = 16;

/// How long relays keep an announcement (NIP-40), and how old one may be
/// for a reader to use it.
pub(crate) const ANNOUNCEMENT_LIFETIME: Duration = Duration::from_secs(3 * 60 * 60);

/// How often a node publishes its announcement again.
pub(crate) const ANNOUNCE_EVERY: Duration = Duration::from_secs(60 * 60);

/// The shortest time between two announcements, so an onion service that
/// comes and goes does not flood the relays.
const ANNOUNCE_GAP: Duration = Duration::from_secs(60);

/// How often a node short of peers asks the relays.
pub(crate) const LOOKUP_EVERY: Duration = Duration::from_secs(10 * 60);

/// A node asks the relays while it has fewer peers than this.
pub(crate) const LOOKUP_BELOW_PEERS: usize = 8;

/// Most peers dialed from one lookup, so the relays cannot fill every
/// connection slot.
pub(crate) const MAX_NOSTR_DIALS: usize = 8;

/// Most announcements read from one relay in one lookup.
const MAX_EVENTS_PER_RELAY: usize = 500;

/// Most addresses one announcement may carry.
const MAX_ADDRESSES_PER_EVENT: usize = 8;

/// Timeout for one relay's whole exchange.
const RELAY_TIMEOUT: Duration = Duration::from_secs(15);

/// Largest message read from a relay.
const MAX_RELAY_MESSAGE_BYTES: usize = 256 * 1024;

/// How far in the future an announcement's time may be, for clock skew.
const MAX_CLOCK_SKEW_SECS: u64 = 10 * 60;

/// What the `alt` tag tells Nostr clients (NIP-31).
const ALT_TEXT: &str = "P2Poolv2 node announcement: addresses for P2Pool peers to dial";

/// Domain separation for the signing key derived from the node key.
const NOSTR_KEY_TAG: &[u8] = b"p2poolv2/nostr-announcement-key/v1";

/// A Nostr event (NIP-01).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Event {
    pub(crate) id: String,
    pub(crate) pubkey: String,
    pub(crate) created_at: u64,
    pub(crate) kind: u32,
    pub(crate) tags: Vec<Vec<String>>,
    pub(crate) content: String,
    pub(crate) sig: String,
}

/// A string as NIP-01 serializes it for the event id: quoted, with only `"`,
/// `\`, newline, carriage return, tab, backspace and form feed escaped, and
/// every other character verbatim.
fn nip01_string(text: &str, out: &mut String) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            other => out.push(other),
        }
    }
    out.push('"');
}

/// The id of an event with these fields: SHA-256 of
/// `[0,pubkey,created_at,kind,tags,content]` serialized as NIP-01 says.
pub(crate) fn event_id(
    pubkey: &str,
    created_at: u64,
    kind: u32,
    tags: &[Vec<String>],
    content: &str,
) -> [u8; 32] {
    let mut serialized = String::with_capacity(256);
    serialized.push_str("[0,");
    nip01_string(pubkey, &mut serialized);
    serialized.push_str(&format!(",{created_at},{kind},["));
    for (index, tag) in tags.iter().enumerate() {
        if index > 0 {
            serialized.push(',');
        }
        serialized.push('[');
        for (position, value) in tag.iter().enumerate() {
            if position > 0 {
                serialized.push(',');
            }
            nip01_string(value, &mut serialized);
        }
        serialized.push(']');
    }
    serialized.push_str("],");
    nip01_string(content, &mut serialized);
    serialized.push(']');
    sha256::Hash::hash(serialized.as_bytes()).to_byte_array()
}

/// Leading zero bits of an event id (NIP-13).
pub(crate) fn leading_zero_bits(id: &[u8; 32]) -> u32 {
    let mut bits = 0;
    for byte in id {
        if *byte == 0 {
            bits += 8;
        } else {
            bits += byte.leading_zeros();
            break;
        }
    }
    bits
}

/// The Nostr key a node signs its announcements with: derived from its
/// libp2p key, so it is kept with it and changes with it.
pub(crate) fn nostr_keys(node_key: &libp2p::identity::Keypair) -> Result<Keypair, String> {
    let ed25519 = node_key
        .clone()
        .try_into_ed25519()
        .map_err(|error| format!("the node key is not ed25519: {error}"))?;
    let secp = Secp256k1::new();
    let mut seed = sha256::Hash::hash(&[NOSTR_KEY_TAG, ed25519.secret().as_ref()].concat());
    loop {
        if let Ok(secret) = SecretKey::from_slice(seed.as_byte_array()) {
            return Ok(Keypair::from_secret_key(&secp, &secret));
        }
        seed = sha256::Hash::hash(seed.as_byte_array());
    }
}

/// The `d` tag of a network's announcements: its chain and network for
/// people, and the first 8 bytes of the hash of the node's network protocol
/// string, which every share-chain rule change alters.
pub(crate) fn announcement_tag(network_name: &str, network_protocol: &str) -> String {
    let hash = sha256::Hash::hash(network_protocol.as_bytes());
    format!(
        "p2poolv2/{network_name}/{}",
        hex::encode(&hash.as_byte_array()[..8])
    )
}

/// Unix seconds now.
pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// A signed announcement of `addresses` under `tag`, created at `now`, with a
/// proof of work of `pow_bits`.
pub(crate) fn sign_announcement(
    keys: &Keypair,
    tag: &str,
    addresses: &[Multiaddr],
    now: u64,
    pow_bits: u32,
) -> Event {
    let secp = Secp256k1::new();
    let pubkey = hex::encode(keys.x_only_public_key().0.serialize());
    let expiration = now + ANNOUNCEMENT_LIFETIME.as_secs();
    let mut base_tags: Vec<Vec<String>> = vec![
        vec!["d".into(), tag.into()],
        vec!["expiration".into(), expiration.to_string()],
        vec!["alt".into(), ALT_TEXT.into()],
    ];
    base_tags.extend(
        addresses
            .iter()
            .take(MAX_ADDRESSES_PER_EVENT)
            .map(|address| vec!["addr".into(), address.to_string()]),
    );
    let mut nonce: u64 = 0;
    let (tags, id) = loop {
        let mut tags = base_tags.clone();
        tags.push(vec![
            "nonce".into(),
            nonce.to_string(),
            pow_bits.to_string(),
        ]);
        let id = event_id(&pubkey, now, APP_DATA_KIND, &tags, "");
        if leading_zero_bits(&id) >= pow_bits {
            break (tags, id);
        }
        nonce += 1;
    };
    let signature = secp.sign_schnorr_no_aux_rand(&Message::from_digest(id), keys);
    Event {
        id: hex::encode(id),
        pubkey,
        created_at: now,
        kind: APP_DATA_KIND,
        tags,
        content: String::new(),
        sig: hex::encode(signature.serialize()),
    }
}

/// The value of an event's first tag named `name`.
fn tag_value<'a>(event: &'a Event, name: &str) -> Option<&'a str> {
    event
        .tags
        .iter()
        .find(|tag| tag.first().map(String::as_str) == Some(name))
        .and_then(|tag| tag.get(1))
        .map(String::as_str)
}

/// Check that `event` is a valid announcement under `tag` at `now`: its id
/// and signature, its proof of work, and its age.
pub(crate) fn check_announcement(
    event: &Event,
    tag: &str,
    now: u64,
    min_pow_bits: u32,
) -> Result<(), String> {
    if event.kind != APP_DATA_KIND {
        return Err(format!("kind {}", event.kind));
    }
    if tag_value(event, "d") != Some(tag) {
        return Err("another network's tag".into());
    }
    let id = event_id(
        &event.pubkey,
        event.created_at,
        event.kind,
        &event.tags,
        &event.content,
    );
    if hex::encode(id) != event.id {
        return Err("id does not match its fields".into());
    }
    let committed_bits = event
        .tags
        .iter()
        .find(|tag| tag.first().map(String::as_str) == Some("nonce"))
        .and_then(|tag| tag.get(2))
        .and_then(|bits| bits.parse::<u32>().ok())
        .unwrap_or(0);
    if committed_bits < min_pow_bits || leading_zero_bits(&id) < min_pow_bits {
        return Err("proof of work below the minimum".into());
    }
    let pubkey = hex::decode(&event.pubkey)
        .ok()
        .and_then(|bytes| XOnlyPublicKey::from_slice(&bytes).ok())
        .ok_or("invalid pubkey")?;
    let signature = hex::decode(&event.sig)
        .ok()
        .and_then(|bytes| secp256k1::schnorr::Signature::from_slice(&bytes).ok())
        .ok_or("invalid signature encoding")?;
    Secp256k1::verification_only()
        .verify_schnorr(&signature, &Message::from_digest(id), &pubkey)
        .map_err(|_| "bad signature")?;
    if event.created_at > now + MAX_CLOCK_SKEW_SECS {
        return Err("created in the future".into());
    }
    if now.saturating_sub(event.created_at) > ANNOUNCEMENT_LIFETIME.as_secs() {
        return Err("too old".into());
    }
    if let Some(expiration) =
        tag_value(event, "expiration").and_then(|value| value.parse::<u64>().ok())
        && expiration <= now
    {
        return Err("expired".into());
    }
    Ok(())
}

/// The peer ID an announced address ends in.
fn address_peer(address: &Multiaddr) -> Option<PeerId> {
    match address.iter().last() {
        Some(Protocol::P2p(peer)) => Some(peer),
        _ => None,
    }
}

/// The addresses of a valid announcement that a node may dial: IPv4 or IPv6
/// TCP addresses, and onion addresses, ending in one peer ID, which `accept`
/// allows. A node lets onion addresses through only when it dials through
/// Tor (`NostrSeeds::accepts`).
pub(crate) fn dialable_addresses(
    event: &Event,
    accept: impl Fn(&Multiaddr) -> bool,
) -> Vec<(PeerId, Multiaddr)> {
    event
        .tags
        .iter()
        .filter(|tag| tag.first().map(String::as_str) == Some("addr"))
        .filter_map(|tag| tag.get(1)?.parse::<Multiaddr>().ok())
        .take(MAX_ADDRESSES_PER_EVENT)
        .filter(|address| {
            let ip_with_tcp = matches!(
                address.iter().next(),
                Some(Protocol::Ip4(_) | Protocol::Ip6(_))
            ) && address
                .iter()
                .any(|protocol| matches!(protocol, Protocol::Tcp(_)));
            (ip_with_tcp || is_onion_with_peer(address)) && accept(address)
        })
        .filter_map(|address| Some((address_peer(&address)?, address)))
        .collect()
}

/// Whether `address` is an onion address followed by a peer ID and nothing
/// else, as an onion node announces itself.
fn is_onion_with_peer(address: &Multiaddr) -> bool {
    let mut protocols = address.iter();
    matches!(
        (protocols.next(), protocols.next(), protocols.next()),
        (Some(Protocol::Onion3(_)), Some(Protocol::P2p(_)), None)
    )
}

/// A relay's WebSocket connection, with a cap on what it may send. With a
/// `proxy`, the connection goes through that SOCKS5 proxy, the relay's name
/// unresolved, and TLS and the WebSocket handshake run over it.
async fn connect(
    relay: &str,
    proxy: Option<&str>,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    String,
> {
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_RELAY_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_RELAY_MESSAGE_BYTES));
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let (socket, _) = match proxy {
        None => tokio_tungstenite::connect_async_with_config(relay, Some(config), true).await,
        Some(proxy) => {
            let request = relay
                .into_client_request()
                .map_err(|error| format!("{relay}: {error}"))?;
            let stream = socks5::connect(proxy, &relay_host_port(relay, request.uri())?)
                .await
                .map_err(|error| format!("{relay}: {error}"))?;
            let _ = stream.set_nodelay(true);
            tokio_tungstenite::client_async_tls_with_config(request, stream, Some(config), None)
                .await
        }
    }
    .map_err(|error| format!("{relay}: {error}"))?;
    Ok(socket)
}

/// The `HOST:PORT` a relay's URL names, for the SOCKS5 proxy: its port, or
/// 443 for `wss` and 80 for `ws`. An IPv6 host keeps its brackets.
fn relay_host_port(relay: &str, uri: &Uri) -> Result<String, String> {
    let host = uri.host().ok_or_else(|| format!("{relay}: no host name"))?;
    let port = uri
        .port_u16()
        .or_else(|| match uri.scheme_str() {
            Some("wss") => Some(443),
            Some("ws") => Some(80),
            _ => None,
        })
        .ok_or_else(|| format!("{relay}: not a ws:// or wss:// URL"))?;
    Ok(format!("{host}:{port}"))
}

/// The announcements `relay` holds under `tag` from the last
/// [`ANNOUNCEMENT_LIFETIME`], at most [`MAX_EVENTS_PER_RELAY`], asked
/// through `proxy` when one is given.
pub(crate) async fn fetch_announcements(
    relay: &str,
    proxy: Option<&str>,
    tag: &str,
    now: u64,
) -> Result<Vec<Event>, String> {
    let exchange = async {
        let mut socket = connect(relay, proxy).await?;
        let subscription = format!("p2pool-{now}");
        let filter = serde_json::json!({
            "kinds": [APP_DATA_KIND],
            "#d": [tag],
            "since": now.saturating_sub(ANNOUNCEMENT_LIFETIME.as_secs()),
            "limit": MAX_EVENTS_PER_RELAY,
        });
        socket
            .send(WsMessage::Text(
                serde_json::json!(["REQ", subscription, filter])
                    .to_string()
                    .into(),
            ))
            .await
            .map_err(|error| format!("{relay}: {error}"))?;
        let mut events = Vec::new();
        while let Some(message) = socket.next().await {
            let text = match message.map_err(|error| format!("{relay}: {error}"))? {
                WsMessage::Text(text) => text,
                WsMessage::Close(_) => break,
                _ => continue,
            };
            let Ok(serde_json::Value::Array(parts)) = serde_json::from_str(&text) else {
                continue;
            };
            match parts.first().and_then(|kind| kind.as_str()) {
                Some("EVENT") if parts.get(1).and_then(|id| id.as_str()) == Some(&subscription) => {
                    if let Some(Ok(event)) = parts
                        .get(2)
                        .map(|value| serde_json::from_value::<Event>(value.clone()))
                    {
                        events.push(event);
                        if events.len() >= MAX_EVENTS_PER_RELAY {
                            break;
                        }
                    }
                }
                Some("EOSE") | Some("CLOSED") => break,
                _ => {}
            }
        }
        let _ = socket
            .send(WsMessage::Text(
                serde_json::json!(["CLOSE", subscription])
                    .to_string()
                    .into(),
            ))
            .await;
        let _ = socket.close(None).await;
        Ok(events)
    };
    timeout(RELAY_TIMEOUT, exchange)
        .await
        .map_err(|_| format!("{relay}: no answer in {} s", RELAY_TIMEOUT.as_secs()))?
}

/// Publish `event` on `relay`, through `proxy` when one is given, and wait
/// for the relay's `OK`.
pub(crate) async fn publish(relay: &str, proxy: Option<&str>, event: &Event) -> Result<(), String> {
    let exchange = async {
        let mut socket = connect(relay, proxy).await?;
        socket
            .send(WsMessage::Text(
                serde_json::json!(["EVENT", event]).to_string().into(),
            ))
            .await
            .map_err(|error| format!("{relay}: {error}"))?;
        let mut result = Err(format!("{relay}: closed before its OK"));
        while let Some(message) = socket.next().await {
            let text = match message.map_err(|error| format!("{relay}: {error}"))? {
                WsMessage::Text(text) => text,
                WsMessage::Close(_) => break,
                _ => continue,
            };
            let Ok(serde_json::Value::Array(parts)) = serde_json::from_str(&text) else {
                continue;
            };
            if parts.first().and_then(|kind| kind.as_str()) == Some("OK")
                && parts.get(1).and_then(|id| id.as_str()) == Some(&event.id)
            {
                result = match parts.get(2).and_then(|accepted| accepted.as_bool()) {
                    Some(true) => Ok(()),
                    _ => Err(format!(
                        "{relay}: refused: {}",
                        parts
                            .get(3)
                            .and_then(|reason| reason.as_str())
                            .unwrap_or("")
                    )),
                };
                break;
            }
        }
        let _ = socket.close(None).await;
        result
    };
    timeout(RELAY_TIMEOUT, exchange)
        .await
        .map_err(|_| format!("{relay}: no answer in {} s", RELAY_TIMEOUT.as_secs()))?
}

/// What a node needs to announce itself and look others up on Nostr.
#[derive(Debug, Clone)]
pub(crate) struct NostrSeeds {
    pub(crate) relays: Vec<String>,
    pub(crate) tag: String,
    pub(crate) keys: Keypair,
    pub(crate) own_peer: PeerId,
    /// Whether this node announces at all: `nostr_announce` is on, and an
    /// onion-only node reaches the relays through Tor.
    pub(crate) announces: bool,
    /// This node's addresses, each ending in its peer ID; empty when it does
    /// not announce.
    pub(crate) announce: Vec<Multiaddr>,
    /// Dial loopback and private addresses too, as on regtest.
    pub(crate) accept_local: bool,
    /// Proof of work an announcement needs, and this node's carry:
    /// [`MIN_POW_BITS`].
    pub(crate) pow_bits: u32,
    /// The networks this node dials from the start (`onlynet`, and onion
    /// only with a `tor_proxy`); announced addresses on others are skipped.
    reachable: address_filter::Reachable,
    /// The Tor proxy this node dials onion peers through, shared with its
    /// transport: `tor_proxy`, or the default Tor SOCKS port once the Tor
    /// control port has made its onion service. Announced onion addresses
    /// count once it is set. None when the node never dials through Tor.
    onion_proxy: Option<OnionProxy>,
    /// The same proxy when the node opens no clearnet connection of its own
    /// and reaches the relays only through Tor; None to reach them directly.
    relay_proxy: Option<OnionProxy>,
}

impl NostrSeeds {
    /// Whether to dial an announced `address`: on a network this node
    /// dials, at a routable IP or an onion address, or at any IP on regtest.
    fn accepts(&self, address: &Multiaddr) -> bool {
        let reachable = self.reachable();
        if self.accept_local {
            reachable.allows(address)
        } else {
            address_filter::is_reachable(address, reachable)
        }
    }

    /// The networks this node dials now: onion too once its Tor proxy is
    /// set, unless `onlynet` leaves onion out.
    fn reachable(&self) -> address_filter::Reachable {
        match &self.onion_proxy {
            Some(proxy) if proxy.get().is_some() => self.reachable.with_onion_proxy(),
            _ => self.reachable,
        }
    }

    /// How the relays are reached now: `Ok(None)` directly, `Ok(Some(proxy))`
    /// through the Tor SOCKS port at `proxy`, and an error while a node that
    /// reaches them only through Tor has no proxy yet.
    pub(crate) fn relay_route(&self) -> Result<Option<String>, &'static str> {
        match &self.relay_proxy {
            None => Ok(None),
            Some(proxy) => proxy.get().map(Some).ok_or(
                "this node reaches its relays only through Tor, and the Tor control port has not made its onion service yet",
            ),
        }
    }

    /// What this node announces: its `external_address`, then the
    /// addresses it gained while it runs (`run_time`), each ending in its
    /// peer ID; nothing when it does not announce.
    pub(crate) fn addresses(&self, run_time: &[Multiaddr]) -> Vec<Multiaddr> {
        if !self.announces {
            return Vec::new();
        }
        let mut addresses = self.announce.clone();
        for address in run_time {
            let address = with_peer(address.clone(), self.own_peer);
            if !addresses.contains(&address) {
                addresses.push(address);
            }
        }
        addresses
    }

    /// The newest valid announcement per peer from `events`, newest first,
    /// skipping this node and `connected`, as addresses to dial.
    pub(crate) fn pick_dials(
        &self,
        events: impl IntoIterator<Item = Event>,
        connected: &HashSet<PeerId>,
        now: u64,
    ) -> Vec<Multiaddr> {
        let mut newest: HashMap<String, Event> = HashMap::new();
        for event in events {
            if check_announcement(&event, &self.tag, now, self.pow_bits).is_err() {
                continue;
            }
            match newest.get(&event.pubkey) {
                Some(known) if known.created_at >= event.created_at => {}
                _ => {
                    newest.insert(event.pubkey.clone(), event);
                }
            }
        }
        let mut events: Vec<Event> = newest.into_values().collect();
        events.sort_by_key(|event| std::cmp::Reverse(event.created_at));
        let mut seen = HashSet::new();
        let mut dials = Vec::new();
        for event in &events {
            for (peer, address) in dialable_addresses(event, |address| self.accepts(address)) {
                if peer == self.own_peer || connected.contains(&peer) || !seen.insert(peer) {
                    continue;
                }
                dials.push(address);
                break;
            }
            if dials.len() >= MAX_NOSTR_DIALS {
                break;
            }
        }
        dials
    }
}

/// `address` ending in `peer`, as an announcement carries it.
fn with_peer(mut address: Multiaddr, peer: PeerId) -> Multiaddr {
    if address_peer(&address).is_none() {
        address.push(Protocol::P2p(peer));
    }
    address
}

/// The Nostr setup `config` asks for, or `None` without relays. The node
/// announces its `external_address` when `nostr_announce` is on; on regtest
/// it dials loopback and private addresses too. `onion_proxy` is the Tor
/// proxy the node dials onion peers through, None when it never can. A node
/// that opens no clearnet connection of its own reaches the relays through
/// it, and stays off Nostr without one. An onion-only node announces only
/// then: one that reached the relays directly would show them its IP beside
/// its onion address.
pub(crate) fn from_config(
    config: &crate::config::Config,
    node_key: &libp2p::identity::Keypair,
    network_protocol: &str,
    onion_proxy: Option<&OnionProxy>,
) -> Result<Option<NostrSeeds>, String> {
    use crate::config::{BitcoinCashNetwork, Chain};
    if config.network.nostr_relays.is_empty() {
        return Ok(None);
    }
    let relay_proxy = if config.network.dials_clearnet_directly() {
        None
    } else {
        let Some(proxy) = onion_proxy else {
            info!(
                "Nostr is off: this node opens no clearnet connection of its own and has no Tor proxy to reach the relays through, neither tor_proxy nor an onion service from the Tor control port"
            );
            return Ok(None);
        };
        Some(proxy.clone())
    };
    let own_peer = node_key.public().to_peer_id();
    let (network_name, regtest) = match (config.stratum.chain, config.stratum.bitcoin_cash_network)
    {
        (Chain::BitcoinCash, Some(network)) => (
            format!("bch-{}", format!("{network:?}").to_lowercase()),
            network == BitcoinCashNetwork::Regtest,
        ),
        _ => (
            format!("btc-{}", config.stratum.network),
            config.stratum.network == bitcoin::Network::Regtest,
        ),
    };
    let onion_only = config.network.announces_only_onion();
    if config.network.nostr_announce && onion_only && relay_proxy.is_none() {
        info!(
            "Not announcing on Nostr: an onion-only node that reaches the relays directly would show them its IP beside its onion address"
        );
    }
    let announces = config.network.nostr_announce && (!onion_only || relay_proxy.is_some());
    let announce = if announces {
        config
            .network
            .external_address
            .as_deref()
            .and_then(|address| address.parse::<Multiaddr>().ok())
            .map(|address| with_peer(address, own_peer))
            .into_iter()
            .collect()
    } else {
        Vec::new()
    };
    Ok(Some(NostrSeeds {
        relays: config.network.nostr_relays.clone(),
        tag: announcement_tag(&network_name, network_protocol),
        keys: nostr_keys(node_key)?,
        own_peer,
        announces,
        announce,
        accept_local: regtest,
        pow_bits: MIN_POW_BITS,
        reachable: address_filter::Reachable::from_config(&config.network),
        onion_proxy: onion_proxy.cloned(),
        relay_proxy,
    }))
}

/// Spawns a task that asks every relay for announcements and has the node
/// actor dial the picked peers once.
pub(crate) fn spawn_lookup<C: Send + 'static>(
    seeds: Arc<NostrSeeds>,
    connected: HashSet<PeerId>,
    swarm_tx: mpsc::Sender<SwarmSend<C>>,
) {
    tokio::spawn(async move {
        let proxy = match seeds.relay_route() {
            Ok(proxy) => proxy,
            Err(why) => {
                debug!("Nostr lookup skipped: {why}");
                return;
            }
        };
        let now = unix_now();
        let answers = join_all(
            seeds
                .relays
                .iter()
                .map(|relay| fetch_announcements(relay, proxy.as_deref(), &seeds.tag, now)),
        )
        .await;
        let mut events = Vec::new();
        for answer in answers {
            match answer {
                Ok(found) => events.extend(found),
                Err(error) => debug!("Nostr relay lookup failed: {error}"),
            }
        }
        let dials = seeds.pick_dials(events, &connected, now);
        info!("Dialing {} peers announced on Nostr", dials.len());
        for address in dials {
            if let Err(error) = swarm_tx.send(SwarmSend::Dial(address)).await {
                debug!("Node actor stopped before a Nostr dial: {error}");
            }
        }
    });
}

/// Spawns a task that publishes this node's announcement on every relay now
/// and every [`ANNOUNCE_EVERY`] after, and again when `run_time`, the
/// addresses the node gains while it runs (its onion service's), changes.
/// It publishes only while it has an address, and at most once every
/// [`ANNOUNCE_GAP`].
pub(crate) fn spawn_announcer(
    seeds: Arc<NostrSeeds>,
    mut run_time: watch::Receiver<Vec<Multiaddr>>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut every = tokio::time::interval(ANNOUNCE_EVERY);
        let mut watching = true;
        loop {
            tokio::select! {
                _ = every.tick() => {}
                changed = run_time.changed(), if watching => {
                    // The node has stopped: its addresses change no more.
                    if changed.is_err() {
                        watching = false;
                        continue;
                    }
                }
            }
            let addresses = seeds.addresses(&run_time.borrow_and_update());
            if addresses.is_empty() {
                continue;
            }
            let proxy = match seeds.relay_route() {
                Ok(proxy) => proxy,
                Err(why) => {
                    debug!("Nostr announcement skipped: {why}");
                    continue;
                }
            };
            let event = {
                let seeds = Arc::clone(&seeds);
                match tokio::task::spawn_blocking(move || {
                    sign_announcement(
                        &seeds.keys,
                        &seeds.tag,
                        &addresses,
                        unix_now(),
                        seeds.pow_bits,
                    )
                })
                .await
                {
                    Ok(event) => event,
                    Err(error) => {
                        warn!("Cannot sign the Nostr announcement: {error}");
                        continue;
                    }
                }
            };
            let results = join_all(
                seeds
                    .relays
                    .iter()
                    .map(|relay| publish(relay, proxy.as_deref(), &event)),
            )
            .await;
            let accepted = results.iter().filter(|result| result.is_ok()).count();
            for error in results.into_iter().filter_map(Result::err) {
                debug!("Nostr announcement not published: {error}");
            }
            info!(
                "Announced this node on {accepted} of {} Nostr relays",
                seeds.relays.len()
            );
            tokio::time::sleep(ANNOUNCE_GAP).await;
        }
    })
}

#[cfg(test)]
pub(crate) mod tests;
