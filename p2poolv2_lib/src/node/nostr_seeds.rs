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

use super::SwarmSend;
use super::address_filter;
use bitcoin::hashes::{Hash, sha256};
use bitcoin::secp256k1::{self, Keypair, Message, Secp256k1, SecretKey, XOnlyPublicKey};
use futures::{SinkExt, StreamExt, future::join_all};
use libp2p::multiaddr::Protocol;
use libp2p::{Multiaddr, PeerId};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message as WsMessage;
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
/// TCP addresses ending in one peer ID, which `accept` allows.
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
            matches!(
                address.iter().next(),
                Some(Protocol::Ip4(_) | Protocol::Ip6(_))
            ) && address
                .iter()
                .any(|protocol| matches!(protocol, Protocol::Tcp(_)))
                && accept(address)
        })
        .filter_map(|address| Some((address_peer(&address)?, address)))
        .collect()
}

/// A relay's WebSocket connection, with a cap on what it may send.
async fn connect(
    relay: &str,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    String,
> {
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_RELAY_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_RELAY_MESSAGE_BYTES));
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let (socket, _) = tokio_tungstenite::connect_async_with_config(relay, Some(config), true)
        .await
        .map_err(|error| format!("{relay}: {error}"))?;
    Ok(socket)
}

/// The announcements `relay` holds under `tag` from the last
/// [`ANNOUNCEMENT_LIFETIME`], at most [`MAX_EVENTS_PER_RELAY`].
pub(crate) async fn fetch_announcements(
    relay: &str,
    tag: &str,
    now: u64,
) -> Result<Vec<Event>, String> {
    let exchange = async {
        let mut socket = connect(relay).await?;
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

/// Publish `event` on `relay` and wait for the relay's `OK`.
pub(crate) async fn publish(relay: &str, event: &Event) -> Result<(), String> {
    let exchange = async {
        let mut socket = connect(relay).await?;
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
    /// This node's addresses, each ending in its peer ID; empty when it does
    /// not announce.
    pub(crate) announce: Vec<Multiaddr>,
    /// Dial loopback and private addresses too, as on regtest.
    pub(crate) accept_local: bool,
    /// Proof of work an announcement needs, and this node's carry:
    /// [`MIN_POW_BITS`].
    pub(crate) pow_bits: u32,
}

impl NostrSeeds {
    fn accepts(&self, address: &Multiaddr) -> bool {
        self.accept_local || address_filter::is_routable_multiaddr(address)
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
/// announces its `external_address` when `nostr_announce` is on, unless it
/// is `onion_only`: it reaches the relays directly, so a relay would see its
/// IP beside its onion address. On regtest it dials loopback and private
/// addresses too.
pub(crate) fn from_config(
    config: &crate::config::Config,
    node_key: &libp2p::identity::Keypair,
    network_protocol: &str,
) -> Result<Option<NostrSeeds>, String> {
    use crate::config::{BitcoinCashNetwork, Chain};
    if config.network.nostr_relays.is_empty() {
        return Ok(None);
    }
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
    if config.network.nostr_announce && config.network.onion_only {
        info!(
            "Not announcing on Nostr: an onion_only node reaches the relays directly, which would show them its IP beside its onion address"
        );
    }
    let announce = if config.network.nostr_announce && !config.network.onion_only {
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
        announce,
        accept_local: regtest,
        pow_bits: MIN_POW_BITS,
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
        let now = unix_now();
        let answers = join_all(
            seeds
                .relays
                .iter()
                .map(|relay| fetch_announcements(relay, &seeds.tag, now)),
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
/// and every [`ANNOUNCE_EVERY`] after.
pub(crate) fn spawn_announcer(seeds: Arc<NostrSeeds>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut every = tokio::time::interval(ANNOUNCE_EVERY);
        loop {
            every.tick().await;
            let event = {
                let seeds = Arc::clone(&seeds);
                match tokio::task::spawn_blocking(move || {
                    sign_announcement(
                        &seeds.keys,
                        &seeds.tag,
                        &seeds.announce,
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
            let results = join_all(seeds.relays.iter().map(|relay| publish(relay, &event))).await;
            let accepted = results.iter().filter(|result| result.is_ok()).count();
            for error in results.into_iter().filter_map(Result::err) {
                debug!("Nostr announcement not published: {error}");
            }
            info!(
                "Announced this node on {accepted} of {} Nostr relays",
                seeds.relays.len()
            );
        }
    })
}

#[cfg(test)]
pub(crate) mod tests;
