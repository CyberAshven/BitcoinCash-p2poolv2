// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use std::sync::{Arc, Mutex};
use tempfile::tempdir;
use tokio::net::TcpListener;
use tokio::sync::Notify;

/// The Tor Project's own v3 onion service.
const SERVICE_ID: &str = "2gzyxa5ihm7nsggfxnu52rck2vv4rvmdlkiu3zzui5du4xyclen53wid";

/// A key as `ADD_ONION` hands one back: the key type, then base64.
const PRIVATE_KEY: &str = "ED25519-V3:YWJjZGVmZ2hpamtsbW5vcHFyc3R1dnd4eXphYmNkZWZnaGlqa2xtbm9wcXJzdHV2d3h5emFiY2RlZmdoaWprbG0=";

/// A scripted Tor control port on a local port. It answers each command line
/// with what its script returns, records the commands, and closes the
/// connection it holds when `close` is notified.
struct FakeTor {
    address: String,
    commands: Arc<Mutex<Vec<String>>>,
    close: Arc<Notify>,
}

impl FakeTor {
    async fn start(script: impl Fn(&str) -> String + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let commands = Arc::new(Mutex::new(Vec::new()));
        let close = Arc::new(Notify::new());
        let (recorded, closing) = (Arc::clone(&commands), Arc::clone(&close));
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let (reader, mut writer) = stream.into_split();
                let mut lines = BufReader::new(reader).lines();
                loop {
                    tokio::select! {
                        line = lines.next_line() => {
                            let Ok(Some(line)) = line else {
                                break;
                            };
                            recorded.lock().unwrap().push(line.clone());
                            if writer.write_all(script(&line).as_bytes()).await.is_err() {
                                break;
                            }
                        }
                        () = closing.notified() => break,
                    }
                }
            }
        });
        Self {
            address,
            commands,
            close,
        }
    }

    fn commands(&self) -> Vec<String> {
        self.commands.lock().unwrap().clone()
    }
}

/// Tor's answers for a node that may authenticate with `methods`, naming
/// `cookie_file`: any `AUTHENTICATE` is accepted, and `ADD_ONION` makes the
/// service, with a new key when asked for one that is not discarded.
fn tor_answers(methods: &'static str, cookie_file: Option<String>) -> impl Fn(&str) -> String {
    move |command| {
        if command == "PROTOCOLINFO 1" {
            let cookie = cookie_file
                .as_ref()
                .map(|path| format!(" COOKIEFILE=\"{}\"", quote(path)))
                .unwrap_or_default();
            format!(
                "250-PROTOCOLINFO 1\r\n250-AUTH METHODS={methods}{cookie}\r\n250-VERSION Tor=\"0.4.8.13\"\r\n250 OK\r\n"
            )
        } else if command.starts_with("AUTHENTICATE") {
            "250 OK\r\n".into()
        } else if command.starts_with("ADD_ONION NEW:") && !command.contains("DiscardPK") {
            format!("250-ServiceID={SERVICE_ID}\r\n250-PrivateKey={PRIVATE_KEY}\r\n250 OK\r\n")
        } else if command.starts_with("ADD_ONION ") {
            format!("250-ServiceID={SERVICE_ID}\r\n250 OK\r\n")
        } else {
            "510 Unrecognized command\r\n".into()
        }
    }
}

/// `text` inside a quoted string, escaped as Tor escapes it.
fn quote(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

fn service(control: &str, key_file: Option<PathBuf>) -> OnionService {
    OnionService {
        control: control.to_string(),
        password: None,
        target: "127.0.0.1:6884".parse().unwrap(),
        key_file,
    }
}

fn service_address() -> Multiaddr {
    format!("/onion3/{SERVICE_ID}:6884").parse().unwrap()
}

/// With no password asked for, the node authenticates with none, and
/// `ADD_ONION` forwards virtual port 6884 to its listen address.
#[tokio::test]
async fn a_node_authenticates_with_no_password_and_adds_the_service() {
    let tor = FakeTor::start(tor_answers("NULL", None)).await;
    let (address, _connection) = open(&service(&tor.address, None)).await.unwrap();

    assert_eq!(address, service_address());
    assert_eq!(
        tor.commands(),
        vec![
            "PROTOCOLINFO 1",
            "AUTHENTICATE",
            "ADD_ONION NEW:ED25519-V3 Flags=DiscardPK Port=6884,127.0.0.1:6884",
        ]
    );
}

/// With `tor_password` the node authenticates with it, hex encoded, and
/// refuses to try when Tor does not take a password.
#[tokio::test]
async fn a_node_authenticates_with_its_tor_password() {
    let tor = FakeTor::start(tor_answers("HASHEDPASSWORD", None)).await;
    let mut with_password = service(&tor.address, None);
    with_password.password = Some("hunter2 \"quoted\"".to_string());
    open(&with_password).await.unwrap();
    assert_eq!(
        tor.commands()[1],
        format!("AUTHENTICATE {}", hex::encode("hunter2 \"quoted\""))
    );

    let no_password = FakeTor::start(tor_answers("NULL", None)).await;
    with_password.control = no_password.address.clone();
    assert!(matches!(
        open(&with_password).await,
        Err(Failure::Refused(error)) if error.contains("HASHEDPASSWORD")
    ));
}

/// COOKIE: the node reads the cookie file Tor names and sends it, hex
/// encoded. Tor offers SAFECOOKIE beside it, as it does with
/// CookieAuthentication set; the node never asks for an AUTHCHALLENGE.
#[tokio::test]
async fn a_node_authenticates_with_the_tor_cookie() {
    let dir = tempdir().unwrap();
    let cookie_path = dir.path().join("control_auth_cookie");
    let cookie = [0x5au8; COOKIE_BYTES];
    std::fs::write(&cookie_path, cookie).unwrap();
    let expected = format!("AUTHENTICATE {}", hex::encode(cookie));
    let answers = tor_answers("COOKIE,SAFECOOKIE", Some(cookie_path.display().to_string()));
    let tor = FakeTor::start(move |command| {
        if command.starts_with("AUTHENTICATE") {
            if command == expected {
                "250 OK\r\n".into()
            } else {
                "515 Authentication failed: Authentication cookie did not match expected value.\r\n"
                    .into()
            }
        } else {
            answers(command)
        }
    })
    .await;

    let (address, _connection) = open(&service(&tor.address, None)).await.unwrap();
    assert_eq!(address, service_address());
    assert!(
        !tor.commands()
            .iter()
            .any(|command| command.starts_with("AUTHCHALLENGE")),
        "{:?}",
        tor.commands()
    );
}

/// A cookie file that is not 32 bytes is not the cookie of Tor: the node
/// stops before it sends anything from it.
#[tokio::test]
async fn a_cookie_file_of_the_wrong_size_is_refused() {
    let dir = tempdir().unwrap();
    let cookie_path = dir.path().join("control_auth_cookie");
    std::fs::write(&cookie_path, [0x5au8; 16]).unwrap();
    let tor = FakeTor::start(tor_answers(
        "COOKIE",
        Some(cookie_path.display().to_string()),
    ))
    .await;

    match open(&service(&tor.address, None)).await {
        Err(Failure::Refused(error)) => assert!(error.contains("16 bytes"), "{error}"),
        Err(other) => panic!("expected a refusal, got {other:?}"),
        Ok(_) => panic!("the node logged in with a cookie file of the wrong size"),
    }
    assert!(
        !tor.commands()
            .iter()
            .any(|command| command.starts_with("AUTHENTICATE")),
        "{:?}",
        tor.commands()
    );
}

/// The first service's key is saved, readable by its owner only, and sent
/// again on the next start, so the onion address stays; a node with an
/// ephemeral key has Tor discard the key and saves none.
#[tokio::test]
async fn the_onion_key_is_saved_and_reused() {
    let dir = tempdir().unwrap();
    let key_file = dir.path().join(crate::config::DEFAULT_ONION_KEY_FILE);
    let tor = FakeTor::start(tor_answers("NULL", None)).await;

    open(&service(&tor.address, Some(key_file.clone())))
        .await
        .unwrap();
    assert_eq!(std::fs::read_to_string(&key_file).unwrap(), PRIVATE_KEY);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&key_file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    open(&service(&tor.address, Some(key_file.clone())))
        .await
        .unwrap();
    let commands = tor.commands();
    assert_eq!(
        commands[2],
        "ADD_ONION NEW:ED25519-V3 Port=6884,127.0.0.1:6884"
    );
    assert_eq!(
        commands[5],
        format!("ADD_ONION {PRIVATE_KEY} Port=6884,127.0.0.1:6884")
    );

    std::fs::write(&key_file, "not a key\r\nPort=1").unwrap();
    assert!(matches!(
        open(&service(&tor.address, Some(key_file.clone()))).await,
        Err(Failure::Refused(error)) if error.contains("does not hold an onion service key")
    ));
}

/// The node is told when the service is up, and when Tor's connection closes
/// that it is gone; then it connects again and makes it again.
#[tokio::test]
async fn a_closed_control_connection_takes_the_address_away() {
    let tor = FakeTor::start(tor_answers("NULL", None)).await;
    let (swarm_tx, mut swarm_rx) = mpsc::channel::<SwarmSend<u32>>(4);
    let _task = spawn(service(&tor.address, None), swarm_tx);

    assert!(matches!(
        next_message(&mut swarm_rx).await,
        SwarmSend::OnionServiceUp(address) if address == service_address()
    ));
    tor.close.notify_one();
    assert!(matches!(
        next_message(&mut swarm_rx).await,
        SwarmSend::OnionServiceDown(address) if address == service_address()
    ));
    assert!(matches!(
        next_message(&mut swarm_rx).await,
        SwarmSend::OnionServiceUp(address) if address == service_address()
    ));
}

/// The next message the task sends the node actor, within 10 seconds.
async fn next_message(swarm_rx: &mut mpsc::Receiver<SwarmSend<u32>>) -> SwarmSend<u32> {
    timeout(Duration::from_secs(10), swarm_rx.recv())
        .await
        .expect("the node actor heard nothing")
        .expect("the task stopped")
}

/// A refusal from Tor is an error the node reports, not a service.
#[tokio::test]
async fn a_refused_add_onion_makes_no_service() {
    let answers = tor_answers("NULL", None);
    let tor = FakeTor::start(move |command| {
        if command.starts_with("ADD_ONION") {
            "512 Bad arguments to ADD_ONION: Need at least 1 argument(s)\r\n".into()
        } else {
            answers(command)
        }
    })
    .await;
    assert!(matches!(
        open(&service(&tor.address, None)).await,
        Err(Failure::Refused(error)) if error.starts_with("ADD_ONION: 512")
    ));

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let nobody = listener.local_addr().unwrap().to_string();
    drop(listener);
    assert!(matches!(
        open(&service(&nobody, None)).await,
        Err(Failure::Unreachable(_))
    ));
}

/// Quoted values are unescaped as control-spec.txt says, and a word without
/// `=` ends the pairs.
#[test]
fn reply_pairs_are_read_as_tor_writes_them() {
    let mapping = parse_mapping(
        r#"METHODS=COOKIE,SAFECOOKIE COOKIEFILE="C:\\Tor\\control \"auth\" cookie\303\251" rest of line"#,
    )
    .unwrap();
    assert_eq!(mapping["METHODS"], "COOKIE,SAFECOOKIE");
    assert_eq!(
        mapping["COOKIEFILE"],
        "C:\\Tor\\control \"auth\" cookie\u{e9}"
    );
    assert_eq!(mapping.len(), 2);
    assert!(parse_mapping("COOKIEFILE=\"unterminated").is_err());
}

/// The service forwards to the node's listen address, loopback for an
/// unspecified one, keeps its key beside the store, and is not made when
/// the node has an onion address set up by hand, or when `onlynet` leaves
/// onion out.
#[test]
fn the_service_follows_the_config() {
    let mut config = crate::config::Config::load("../config.sample.toml").unwrap();
    config.network.listen_address = "/ip4/0.0.0.0/tcp/6890".to_string();
    let made = OnionService::from_config(&config).expect("listen_onion is on by default");
    assert_eq!(made.target, "127.0.0.1:0".parse().unwrap());
    assert_eq!(made.control, crate::config::DEFAULT_TOR_CONTROL);
    assert_eq!(
        made.key_file,
        Some(Path::new(".").join(crate::config::DEFAULT_ONION_KEY_FILE))
    );

    config.network.ephemeral_node_key = true;
    assert_eq!(OnionService::from_config(&config).unwrap().key_file, None);

    config.network.external_address = Some(format!("/onion3/{SERVICE_ID}:6884"));
    assert!(OnionService::from_config(&config).is_none());

    config.network.external_address = None;
    config.network.onlynet = Some(vec![crate::config::PeerNetwork::Ipv4]);
    assert!(OnionService::from_config(&config).is_none());
    config.network.onlynet = Some(vec![crate::config::PeerNetwork::Onion]);
    assert!(OnionService::from_config(&config).is_some());

    config.network.listen_onion = false;
    assert!(OnionService::from_config(&config).is_none());
}
