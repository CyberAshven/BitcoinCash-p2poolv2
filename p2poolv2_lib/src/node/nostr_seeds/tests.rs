// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use std::sync::Mutex;
use tokio::net::TcpListener;

const TAG: &str = "p2poolv2/bch-regtest/0011223344556677";
const NOW: u64 = 1_800_000_000;

/// Proof of work for tests: enough to test the rule, quick in a debug build.
const TEST_POW_BITS: u32 = 8;

fn node_key() -> libp2p::identity::Keypair {
    libp2p::identity::Keypair::generate_ed25519()
}

fn address_of(key: &libp2p::identity::Keypair, address: &str) -> Multiaddr {
    format!("{address}/p2p/{}", key.public().to_peer_id())
        .parse()
        .unwrap()
}

fn announcement(key: &libp2p::identity::Keypair, address: &str, now: u64) -> Event {
    sign_announcement(
        &nostr_keys(key).unwrap(),
        TAG,
        &[address_of(key, address)],
        now,
        TEST_POW_BITS,
    )
}

fn seeds(own: &libp2p::identity::Keypair, relays: Vec<String>, accept_local: bool) -> NostrSeeds {
    NostrSeeds {
        relays,
        tag: TAG.into(),
        keys: nostr_keys(own).unwrap(),
        own_peer: own.public().to_peer_id(),
        announces: false,
        announce: vec![],
        accept_local,
        pow_bits: TEST_POW_BITS,
    }
}

/// The id hashes exactly NIP-01's serialization: no whitespace, only its
/// seven escapes, every other character as it is.
#[test]
fn the_event_id_follows_nip01s_serialization() {
    let tags = vec![vec!["d".to_string(), "a\"b\\c".to_string()]];
    let content = "line\nnext\ttab\r\u{8}\u{c} \u{1} é";
    let expected =
        "[0,\"ab\",7,30078,[[\"d\",\"a\\\"b\\\\c\"]],\"line\\nnext\\ttab\\r\\b\\f \u{1} é\"]";
    assert_eq!(
        event_id("ab", 7, 30078, &tags, content),
        sha256::Hash::hash(expected.as_bytes()).to_byte_array()
    );
    assert_eq!(
        event_id("ab", 7, 30078, &[], ""),
        sha256::Hash::hash(b"[0,\"ab\",7,30078,[],\"\"]").to_byte_array()
    );
}

#[test]
fn leading_zero_bits_count_across_bytes() {
    let mut id = [0xff; 32];
    assert_eq!(leading_zero_bits(&id), 0);
    id[0] = 0x0f;
    assert_eq!(leading_zero_bits(&id), 4);
    id[0] = 0;
    id[1] = 0x01;
    assert_eq!(leading_zero_bits(&id), 15);
    assert_eq!(leading_zero_bits(&[0; 32]), 256);
}

/// A node's Nostr key is the same on every start with the same node key,
/// and another node's differs.
#[test]
fn the_nostr_key_follows_the_node_key() {
    let key = node_key();
    assert_eq!(nostr_keys(&key).unwrap(), nostr_keys(&key).unwrap());
    assert_ne!(nostr_keys(&key).unwrap(), nostr_keys(&node_key()).unwrap());
}

/// Every share-chain rule change gives the network another tag.
#[test]
fn the_tag_follows_the_network_protocol() {
    let before = announcement_tag("bch-chipnet", "/p2pool/test/a/1.0.0");
    let after = announcement_tag("bch-chipnet", "/p2pool/test/b/1.0.0");
    assert!(before.starts_with("p2poolv2/bch-chipnet/"), "{before}");
    assert_eq!(before.len(), "p2poolv2/bch-chipnet/".len() + 16);
    assert_ne!(before, after);
}

#[test]
fn a_signed_announcement_checks_out_with_its_tags() {
    let key = node_key();
    let event = announcement(&key, "/ip4/1.2.3.7/tcp/6890", NOW);
    check_announcement(&event, TAG, NOW, TEST_POW_BITS).unwrap();
    assert!(
        leading_zero_bits(&<[u8; 32]>::try_from(hex::decode(&event.id).unwrap()).unwrap())
            >= TEST_POW_BITS
    );
    assert_eq!(tag_value(&event, "d"), Some(TAG));
    assert_eq!(
        tag_value(&event, "expiration"),
        Some((NOW + ANNOUNCEMENT_LIFETIME.as_secs()).to_string().as_str())
    );
    assert_eq!(tag_value(&event, "alt"), Some(ALT_TEXT));
    assert_eq!(
        dialable_addresses(&event, |_| true),
        vec![(
            key.public().to_peer_id(),
            address_of(&key, "/ip4/1.2.3.7/tcp/6890")
        )]
    );
}

/// Any change to a signed announcement, too little work, the wrong tag or
/// the wrong time makes it fail.
#[test]
fn a_forged_cheap_stale_or_foreign_announcement_fails() {
    let key = node_key();
    let event = announcement(&key, "/ip4/1.2.3.7/tcp/6890", NOW);

    let mut moved = event.clone();
    moved.tags[3][1] = address_of(&key, "/ip4/1.2.5.9/tcp/6890").to_string();
    assert!(
        check_announcement(&moved, TAG, NOW, TEST_POW_BITS)
            .unwrap_err()
            .contains("id")
    );

    let mut resigned = moved.clone();
    resigned.id = hex::encode(event_id(
        &resigned.pubkey,
        resigned.created_at,
        resigned.kind,
        &resigned.tags,
        &resigned.content,
    ));
    let error = check_announcement(&resigned, TAG, NOW, TEST_POW_BITS).unwrap_err();
    assert!(
        error.contains("signature") || error.contains("work"),
        "{error}"
    );

    let cheap = sign_announcement(
        &nostr_keys(&key).unwrap(),
        TAG,
        &[address_of(&key, "/ip4/1.2.3.7/tcp/6890")],
        NOW,
        0,
    );
    assert!(
        check_announcement(&cheap, TAG, NOW, TEST_POW_BITS)
            .unwrap_err()
            .contains("work")
    );

    assert!(
        check_announcement(
            &event,
            "p2poolv2/bch-regtest/ffffffffffffffff",
            NOW,
            TEST_POW_BITS
        )
        .unwrap_err()
        .contains("tag")
    );
    let later = NOW + ANNOUNCEMENT_LIFETIME.as_secs() + 1;
    assert!(check_announcement(&event, TAG, later, TEST_POW_BITS).is_err());
    let earlier = NOW - MAX_CLOCK_SKEW_SECS - 1;
    assert!(
        check_announcement(&event, TAG, earlier, TEST_POW_BITS)
            .unwrap_err()
            .contains("future")
    );
}

/// A reader dials each announcing peer once, newest first, never itself or a
/// peer it has, at most MAX_NOSTR_DIALS, and only routable addresses unless
/// it accepts local ones.
#[test]
fn a_lookup_picks_the_newest_routable_peers() {
    let own = node_key();
    let connected_key = node_key();
    let mut events = vec![
        announcement(&own, "/ip4/1.2.3.1/tcp/6890", NOW),
        announcement(&connected_key, "/ip4/1.2.3.2/tcp/6890", NOW),
        announcement(&node_key(), "/ip4/192.168.1.5/tcp/6890", NOW),
    ];
    let moved = node_key();
    events.push(announcement(&moved, "/ip4/1.2.3.3/tcp/6890", NOW - 600));
    events.push(announcement(&moved, "/ip4/1.2.3.4/tcp/6890", NOW - 60));
    for index in 0..12 {
        events.push(announcement(
            &node_key(),
            &format!("/ip4/1.2.4.{index}/tcp/6890"),
            NOW - 1_000 - index,
        ));
    }
    let connected = HashSet::from([connected_key.public().to_peer_id()]);

    let dials = seeds(&own, vec![], false).pick_dials(events.clone(), &connected, NOW);

    assert_eq!(dials.len(), MAX_NOSTR_DIALS);
    assert_eq!(dials[0], address_of(&moved, "/ip4/1.2.3.4/tcp/6890"));
    assert!(dials.iter().all(|address| {
        let peer = address_peer(address).unwrap();
        peer != own.public().to_peer_id() && !connected.contains(&peer)
    }));
    assert!(
        !dials
            .iter()
            .any(|address| address.to_string().contains("192.168."))
    );

    let local = seeds(&own, vec![], true).pick_dials(events, &connected, NOW);
    assert!(
        local
            .iter()
            .any(|address| address.to_string().contains("192.168."))
    );
}

/// Events a relay holds, and the replies NIP-01 gives.
#[derive(Default)]
pub(crate) struct Relay {
    pub(crate) events: Mutex<Vec<Event>>,
}

/// A NIP-01 relay on a local port: it keeps the newest event per author and
/// `d` tag, answers `EVENT` with `OK` and `REQ` with the matching events and
/// `EOSE`.
pub(crate) async fn spawn_relay() -> (String, Arc<Relay>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let relay = Arc::new(Relay::default());
    let state = Arc::clone(&relay);
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let state = Arc::clone(&state);
            tokio::spawn(async move {
                let Ok(mut socket) = tokio_tungstenite::accept_async(stream).await else {
                    return;
                };
                while let Some(Ok(WsMessage::Text(text))) = socket.next().await {
                    let parts: Vec<serde_json::Value> = serde_json::from_str(&text).unwrap();
                    match parts[0].as_str().unwrap() {
                        "EVENT" => {
                            let event: Event = serde_json::from_value(parts[1].clone()).unwrap();
                            let id = event.id.clone();
                            {
                                let mut events = state.events.lock().unwrap();
                                events.retain(|known| {
                                    known.pubkey != event.pubkey
                                        || tag_value(known, "d") != tag_value(&event, "d")
                                });
                                events.push(event);
                            }
                            let ok = serde_json::json!(["OK", id, true, ""]).to_string();
                            socket.send(WsMessage::Text(ok.into())).await.unwrap();
                        }
                        "REQ" => {
                            let subscription = parts[1].as_str().unwrap().to_string();
                            let tag = parts[2]["#d"][0].as_str().unwrap().to_string();
                            let matching: Vec<Event> = state
                                .events
                                .lock()
                                .unwrap()
                                .iter()
                                .filter(|event| tag_value(event, "d") == Some(tag.as_str()))
                                .cloned()
                                .collect();
                            for event in matching {
                                let message =
                                    serde_json::json!(["EVENT", subscription, event]).to_string();
                                socket.send(WsMessage::Text(message.into())).await.unwrap();
                            }
                            let eose = serde_json::json!(["EOSE", subscription]).to_string();
                            socket.send(WsMessage::Text(eose.into())).await.unwrap();
                        }
                        _ => {}
                    }
                }
            });
        }
    });
    (url, relay)
}

/// An announcement published on a relay is read back from it and picked to
/// dial; a second publication replaces the first.
#[tokio::test]
async fn an_announcement_goes_through_a_relay() {
    let (url, relay) = spawn_relay().await;
    let announcer = node_key();
    let now = unix_now();

    let first = announcement(&announcer, "/ip4/1.2.3.20/tcp/6890", now - 60);
    publish(&url, &first).await.unwrap();
    let second = announcement(&announcer, "/ip4/1.2.3.21/tcp/6890", now);
    publish(&url, &second).await.unwrap();
    assert_eq!(relay.events.lock().unwrap().len(), 1);

    let events = fetch_announcements(&url, TAG, now).await.unwrap();
    assert_eq!(events, vec![second]);
    let reader = seeds(&node_key(), vec![url], false);
    assert_eq!(
        reader.pick_dials(events, &HashSet::new(), now),
        vec![address_of(&announcer, "/ip4/1.2.3.21/tcp/6890")]
    );
}

/// A node with no `external_address` announces nothing until it gains an
/// address while it runs, its onion service's, and then announces it,
/// ending in its peer ID.
#[tokio::test]
async fn the_announcer_publishes_an_address_gained_at_run_time() {
    let (url, relay) = spawn_relay().await;
    let key = node_key();
    let mut announcing = seeds(&key, vec![url], false);
    announcing.announces = true;
    let (run_time, receiver) = watch::channel(Vec::new());
    let announcer = spawn_announcer(Arc::new(announcing), receiver);

    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        relay.events.lock().unwrap().is_empty(),
        "nothing to announce yet"
    );

    let onion = "/onion3/2gzyxa5ihm7nsggfxnu52rck2vv4rvmdlkiu3zzui5du4xyclen53wid:6884";
    run_time.send_modify(|addresses| addresses.push(onion.parse().unwrap()));
    let published = timeout(Duration::from_secs(10), async {
        loop {
            let first = relay.events.lock().unwrap().first().cloned();
            if let Some(event) = first {
                return event;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the onion address was never announced");
    announcer.abort();

    let announced: Vec<&str> = published
        .tags
        .iter()
        .filter(|tag| tag.first().map(String::as_str) == Some("addr"))
        .filter_map(|tag| tag.get(1).map(String::as_str))
        .collect();
    assert_eq!(announced, vec![address_of(&key, onion).to_string()]);
}

/// A relay that is down or never answers costs a lookup its timeout, not
/// the lookup.
#[tokio::test]
async fn a_dead_relay_is_an_error_not_a_hang() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    drop(listener);
    assert!(fetch_announcements(&url, TAG, NOW).await.is_err());
}

/// A node's own announcements carry the full proof of work a reader needs.
#[test]
fn a_node_announces_with_the_minimum_proof_of_work() {
    let key = node_key();
    let event = sign_announcement(
        &nostr_keys(&key).unwrap(),
        TAG,
        &[address_of(&key, "/ip4/1.2.3.7/tcp/6890")],
        NOW,
        MIN_POW_BITS,
    );
    check_announcement(&event, TAG, NOW, MIN_POW_BITS).unwrap();
}

/// Candidate default relays answer a lookup with EOSE. Reads only; run by
/// hand: `cargo nextest run -p p2poolv2_lib --run-ignored only live_relays`.
#[tokio::test]
#[ignore = "talks to public Nostr relays"]
async fn live_relays_answer_a_lookup() {
    let now = unix_now();
    let answers = join_all(
        CANDIDATE_RELAYS
            .iter()
            .map(|relay| fetch_announcements(relay, TAG, now)),
    )
    .await;
    for (relay, answer) in CANDIDATE_RELAYS.iter().zip(answers) {
        match answer {
            Ok(events) => println!("ok   {relay}: {} events", events.len()),
            Err(error) => println!("FAIL {relay}: {error}"),
        }
    }
}

const CANDIDATE_RELAYS: &[&str] = &[
    "wss://relay.damus.io",
    "wss://nos.lol",
    "wss://relay.primal.net",
    "wss://nostr.mom",
    "wss://offchain.pub",
    "wss://relay.nostr.band",
    "wss://nostr.oxtr.dev",
    "wss://relay.snort.social",
    "wss://nostr.bitcoiner.social",
    "wss://relay.nostr.net",
];

/// A node announces only an `external_address` its operator set, ending in
/// its peer ID, reads its own network's tag, and stays off Nostr without
/// relays.
#[test]
fn the_config_decides_what_a_node_announces() {
    let mut config = crate::config::Config::load("../config.sample.toml").unwrap();
    let key = node_key();
    let protocol = "/p2pool/test/sample/1.0.0";

    let seeds = from_config(&config, &key, protocol).unwrap().unwrap();
    assert_eq!(
        seeds.relays.len(),
        crate::config::DEFAULT_NOSTR_RELAYS.len()
    );
    assert!(
        seeds.announce.is_empty(),
        "no external_address, no announcement"
    );
    assert!(seeds.tag.starts_with("p2poolv2/bch-"), "{}", seeds.tag);
    assert!(!seeds.accept_local);
    assert_eq!(seeds.pow_bits, MIN_POW_BITS);

    config.network.external_address = Some("/ip4/1.2.3.4/tcp/6890".into());
    let seeds = from_config(&config, &key, protocol).unwrap().unwrap();
    assert_eq!(
        seeds.announce,
        vec![address_of(&key, "/ip4/1.2.3.4/tcp/6890")]
    );

    // A relay would see an onion-only node's IP beside its onion address.
    let mut onion_only = config.clone();
    onion_only.network.onion_only = true;
    onion_only.network.external_address =
        Some("/onion3/2gzyxa5ihm7nsggfxnu52rck2vv4rvmdlkiu3zzui5du4xyclen53wid:6884".into());
    assert!(
        from_config(&onion_only, &key, protocol)
            .unwrap()
            .unwrap()
            .announce
            .is_empty(),
        "an onion_only node does not announce"
    );

    config.network.nostr_announce = false;
    assert!(
        from_config(&config, &key, protocol)
            .unwrap()
            .unwrap()
            .announce
            .is_empty()
    );

    config.network.nostr_relays = vec![];
    assert!(from_config(&config, &key, protocol).unwrap().is_none());
}
