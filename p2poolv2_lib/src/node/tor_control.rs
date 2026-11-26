// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Tor's control port: the node makes itself an onion service, as Bitcoin
//! Cash Node does with `-listenonion` (`src/torcontrol.cpp`).
//!
//! A task connects to `[network] tor_control` and speaks Tor's control
//! protocol (control-spec.txt): `PROTOCOLINFO 1`, then `AUTHENTICATE` with no
//! password, with Tor's cookie file (`SAFECOOKIE`), or with `tor_password`
//! (`HASHEDPASSWORD`), then `ADD_ONION`. The service's virtual port is always
//! 6884, the default P2Pool port, whatever the node listens on, as Bitcoin
//! Cash Node keeps its default port "to avoid decloaking"; Tor forwards it to
//! the node's listen address.
//!
//! The first `ADD_ONION` asks for a new ed25519 key, and the key Tor sends
//! back is saved as `ED25519-V3:<base64>` in `onion_v3_private_key` beside
//! the store, readable by its owner only, and sent again on every later
//! start, so the onion address stays the node's. With `ephemeral_node_key`
//! the node asks Tor to keep no key (`Flags=DiscardPK`) and saves none.
//!
//! The control connection stays open while the node runs: Tor removes the
//! service when it closes. While it is up the node advertises
//! `/onion3/<service>:6884`; when it closes the node stops, and connects
//! again after Bitcoin Cash Node's backoff, 1 second growing 1.5 times a try,
//! here capped at 10 minutes. A node with no Tor running logs each try at
//! debug only.

use super::SwarmSend;
use super::onion::onion3_target;
use crate::config::{Config, PeerNetwork};
use bitcoin::secp256k1::rand::random;
use hmac::{Hmac, KeyInit, Mac};
use libp2p::Multiaddr;
use libp2p::multiaddr::Protocol;
use sha2::Sha256;
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::iter::Peekable;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::str::Chars;
use std::time::Duration;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tracing::{debug, info, warn};

type HmacSha256 = Hmac<Sha256>;

/// The port peers dial on this node's onion service, whatever it listens on.
pub(crate) const ONION_SERVICE_PORT: u16 = 6884;

/// Tor's SOCKS port where Tor runs with its defaults. Onion peers are dialed
/// through it once the control port has made the node's onion service,
/// unless `tor_proxy` names another, as in Bitcoin Cash Node.
pub(crate) const DEFAULT_SOCKS_PROXY: &str = "127.0.0.1:9050";

/// The first wait before connecting again, as Bitcoin Cash Node's
/// `RECONNECT_TIMEOUT_START`.
const RECONNECT_START: Duration = Duration::from_secs(1);

/// How much longer each wait is than the last, as Bitcoin Cash Node's
/// `RECONNECT_TIMEOUT_EXP`.
const RECONNECT_GROWTH: f64 = 1.5;

/// The longest wait, so a Tor started after the node is found within it.
const RECONNECT_MAX: Duration = Duration::from_secs(600);

/// The longest Tor may take to accept the connection or answer a command.
const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

/// The longest reply line read, as Bitcoin Cash Node's `MAX_LINE_LENGTH`.
const MAX_LINE_BYTES: u64 = 100_000;

/// Most lines one reply may have.
const MAX_REPLY_LINES: usize = 1_000;

/// Bytes in Tor's authentication cookie and in each `SAFECOOKIE` nonce.
const COOKIE_BYTES: usize = 32;
const NONCE_BYTES: usize = 32;

/// The HMAC keys of `SAFECOOKIE` authentication (control-spec.txt,
/// `AUTHCHALLENGE`).
const SERVER_HASH_KEY: &[u8] = b"Tor safe cookie authentication server-to-controller hash";
const CLIENT_HASH_KEY: &[u8] = b"Tor safe cookie authentication controller-to-server hash";

/// How an ed25519 v3 onion key starts, as `ADD_ONION` takes and gives it.
const KEY_TYPE: &str = "ED25519-V3:";

/// What the node asks Tor's control port for.
#[derive(Clone)]
pub(crate) struct OnionService {
    /// Tor's control port, `host:port`.
    pub(crate) control: String,
    /// The password for `HASHEDPASSWORD` authentication.
    pub(crate) password: Option<String>,
    /// Where the service forwards to: the node's listen address.
    pub(crate) target: SocketAddr,
    /// The file the onion key is kept in, or None to have Tor keep no key.
    pub(crate) key_file: Option<PathBuf>,
}

impl OnionService {
    /// The onion service `listen_onion` asks for, forwarding to the node's
    /// listen address (its port set once known), with its key beside the
    /// store. None when `listen_onion` is off, the node does not listen or
    /// listens on no IP address, or its `external_address` is an onion
    /// service set up by hand already. None too when `onlynet` leaves onion
    /// out: the node announces no address on a network it does not dial.
    pub(crate) fn from_config(config: &Config) -> Option<Self> {
        let network = &config.network;
        if !network.listen_onion || network.listen_address.is_empty() {
            return None;
        }
        if !network.onlynet_allows(PeerNetwork::Onion) {
            info!(
                "listen_onion: onlynet leaves onion out, so the node asks the Tor control port for no onion service"
            );
            return None;
        }
        if network
            .external_address
            .as_deref()
            .and_then(|address| address.parse::<Multiaddr>().ok())
            .is_some_and(|address| onion3_target(&address).is_some())
        {
            info!(
                "listen_onion: external_address is an onion address already, so the node asks Tor's control port for none"
            );
            return None;
        }
        let listen_address = network.listen_address.parse::<Multiaddr>().ok()?;
        let ip = match listen_address.iter().next() {
            Some(Protocol::Ip4(ip)) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
            Some(Protocol::Ip4(ip)) => IpAddr::V4(ip),
            Some(Protocol::Ip6(ip)) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
            Some(Protocol::Ip6(ip)) => IpAddr::V6(ip),
            _ => {
                warn!(
                    "listen_onion: listen_address {listen_address} has no IP address for Tor to forward to"
                );
                return None;
            }
        };
        Some(Self {
            control: network.tor_control.clone(),
            password: network.tor_password.clone(),
            target: SocketAddr::new(ip, 0),
            key_file: network.onion_key_file(&config.store.path),
        })
    }
}

/// The running control-port task. Dropping it ends the task, which closes
/// the control connection, and Tor removes the service.
pub(crate) struct OnionServiceTask(JoinHandle<()>);

impl Drop for OnionServiceTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Spawns the task that makes the node an onion service and keeps it one,
/// telling the node actor when the service is up and when it is gone.
pub(crate) fn spawn<C: Send + 'static>(
    service: OnionService,
    swarm_tx: mpsc::Sender<SwarmSend<C>>,
) -> OnionServiceTask {
    OnionServiceTask(tokio::spawn(run(service, swarm_tx)))
}

/// Why setting up the service failed.
#[derive(Debug, PartialEq)]
enum Failure {
    /// Nothing answered at the control port: no Tor runs there.
    Unreachable(String),
    /// Tor answered, then refused or broke off.
    Refused(String),
}

/// Sets the service up, reports it, waits until the control connection
/// closes, reports it gone, and starts again after the backoff. The wait
/// starts over once a service was up. A refusal is logged as a warning the
/// first time and at debug while it repeats.
async fn run<C: Send + 'static>(service: OnionService, swarm_tx: mpsc::Sender<SwarmSend<C>>) {
    let mut delay = RECONNECT_START;
    let mut last_refusal: Option<String> = None;
    loop {
        match open(&service).await {
            Ok((address, mut connection)) => {
                last_refusal = None;
                if swarm_tx
                    .send(SwarmSend::OnionServiceUp(address.clone()))
                    .await
                    .is_err()
                {
                    return;
                }
                wait_until_closed(&mut connection).await;
                if swarm_tx
                    .send(SwarmSend::OnionServiceDown(address))
                    .await
                    .is_err()
                {
                    return;
                }
                delay = RECONNECT_START;
            }
            Err(Failure::Unreachable(error)) => debug!(
                "No Tor control port at {}: {error}; trying again in {:.1} s",
                service.control,
                delay.as_secs_f64()
            ),
            Err(Failure::Refused(error)) => {
                if last_refusal.as_ref() == Some(&error) {
                    debug!(
                        "Tor's control port at {} still refuses: {error}",
                        service.control
                    );
                } else {
                    warn!(
                        "Tor's control port at {} made no onion service: {error}; trying again",
                        service.control
                    );
                }
                last_refusal = Some(error);
            }
        }
        tokio::time::sleep(delay).await;
        delay = next_delay(delay);
    }
}

/// The wait after `delay`: [`RECONNECT_GROWTH`] times longer, at most
/// [`RECONNECT_MAX`].
fn next_delay(delay: Duration) -> Duration {
    delay.mul_f64(RECONNECT_GROWTH).min(RECONNECT_MAX)
}

/// Connects to Tor's control port and sets the service up: its address,
/// with the connection that keeps it.
async fn open(service: &OnionService) -> Result<(Multiaddr, BufReader<TcpStream>), Failure> {
    let stream = match timeout(REPLY_TIMEOUT, TcpStream::connect(service.control.as_str())).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(error)) => return Err(Failure::Unreachable(error.to_string())),
        Err(_) => {
            return Err(Failure::Unreachable(format!(
                "no connection in {} s",
                REPLY_TIMEOUT.as_secs()
            )));
        }
    };
    let mut connection = BufReader::new(stream);
    let address = set_up(&mut connection, service)
        .await
        .map_err(Failure::Refused)?;
    Ok((address, connection))
}

/// Authenticates and adds the onion service, saving a new key.
async fn set_up(
    connection: &mut BufReader<TcpStream>,
    service: &OnionService,
) -> Result<Multiaddr, String> {
    let info = command(connection, "PROTOCOLINFO 1")
        .await?
        .accepted("PROTOCOLINFO")?;
    let auth = parse_protocol_info(&info)?;
    authenticate(connection, service, &auth).await?;

    let saved_key = match &service.key_file {
        Some(path) => read_key(path)?,
        None => None,
    };
    let reply = command(
        connection,
        &add_onion_command(
            saved_key.as_deref(),
            service.key_file.is_none(),
            service.target,
        ),
    )
    .await?
    .accepted("ADD_ONION")?;
    let added = parse_add_onion(&reply)?;
    if saved_key.is_none()
        && let Some(path) = &service.key_file
    {
        match added.private_key.as_deref() {
            Some(key) if is_key(key) => match write_key(path, key) {
                Ok(()) => info!("Saved this node's onion service key in {}", path.display()),
                Err(error) => warn!("{error}; the onion address changes on the next start"),
            },
            _ => warn!(
                "Tor sent no usable onion service key; the onion address changes on the next start"
            ),
        }
    }
    Ok(added.address)
}

/// One reply from Tor: its status code and each line's text after the code
/// and separator.
#[derive(Debug, PartialEq)]
struct Reply {
    code: u16,
    lines: Vec<String>,
}

impl Reply {
    /// The reply's lines when Tor accepted `command` (code 250), else what
    /// it said.
    fn accepted(self, command: &str) -> Result<Vec<String>, String> {
        if self.code == 250 {
            Ok(self.lines)
        } else {
            Err(format!("{command}: {} {}", self.code, self.lines.join(" ")))
        }
    }
}

/// Sends `command` and reads Tor's reply to it.
async fn command(connection: &mut BufReader<TcpStream>, command: &str) -> Result<Reply, String> {
    let exchange = async {
        connection
            .get_mut()
            .write_all(format!("{command}\r\n").as_bytes())
            .await
            .map_err(|error| format!("write: {error}"))?;
        read_reply(connection).await
    };
    timeout(REPLY_TIMEOUT, exchange)
        .await
        .map_err(|_| format!("no answer in {} s", REPLY_TIMEOUT.as_secs()))?
}

/// Reads one reply: `XYZ-` lines, then an `XYZ ` line that ends it. The data
/// after an `XYZ+` line, up to a line holding a lone `.`, is skipped.
async fn read_reply<R: AsyncBufRead + Unpin>(reader: &mut R) -> Result<Reply, String> {
    let mut lines = Vec::new();
    loop {
        let line = read_line(reader).await?;
        let code = line
            .get(..3)
            .filter(|code| code.bytes().all(|byte| byte.is_ascii_digit()))
            .and_then(|code| code.parse::<u16>().ok());
        let separator = line.as_bytes().get(3).copied();
        let (Some(code), Some(separator @ (b' ' | b'-' | b'+'))) = (code, separator) else {
            return Err(format!("malformed reply line: {line}"));
        };
        lines.push(line[4..].to_string());
        match separator {
            b' ' => return Ok(Reply { code, lines }),
            b'+' => while read_line(reader).await? != "." {},
            _ => {}
        }
        if lines.len() >= MAX_REPLY_LINES {
            return Err(format!("a reply of more than {MAX_REPLY_LINES} lines"));
        }
    }
}

/// One line from Tor without its line end. An error when the connection
/// closed or the line is longer than [`MAX_LINE_BYTES`].
async fn read_line<R: AsyncBufRead + Unpin>(reader: &mut R) -> Result<String, String> {
    let mut line = Vec::new();
    let read = (&mut *reader)
        .take(MAX_LINE_BYTES)
        .read_until(b'\n', &mut line)
        .await
        .map_err(|error| format!("read: {error}"))?;
    if read == 0 {
        return Err("Tor closed the connection".into());
    }
    if line.pop() != Some(b'\n') {
        return Err(format!(
            "a reply line cut off or longer than {MAX_LINE_BYTES} bytes"
        ));
    }
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    Ok(String::from_utf8_lossy(&line).into_owned())
}

/// Waits until Tor closes the control connection, which removes the
/// service. Tor sends nothing unasked, as the node asks for no events.
async fn wait_until_closed<R: AsyncBufRead + Unpin>(connection: &mut R) {
    while read_line(connection).await.is_ok() {}
}

/// What `PROTOCOLINFO` says about authenticating.
#[derive(Debug, Default, PartialEq)]
struct AuthMethods {
    methods: Vec<String>,
    cookie_file: Option<String>,
}

/// The `AUTH` line of a `PROTOCOLINFO` reply, such as
/// `AUTH METHODS=COOKIE,SAFECOOKIE COOKIEFILE="/run/tor/control.authcookie"`.
fn parse_protocol_info(lines: &[String]) -> Result<AuthMethods, String> {
    let auth = lines
        .iter()
        .find_map(|line| line.strip_prefix("AUTH "))
        .ok_or("PROTOCOLINFO: no AUTH line")?;
    let mut mapping = parse_mapping(auth)?;
    Ok(AuthMethods {
        methods: mapping
            .get("METHODS")
            .map(|methods| methods.split(',').map(str::to_string).collect())
            .unwrap_or_default(),
        cookie_file: mapping.remove("COOKIEFILE"),
    })
}

/// Authenticates as Bitcoin Cash Node does: with `HASHEDPASSWORD` when a
/// password is set, else with no password when Tor asks for none, else with
/// `SAFECOOKIE`.
async fn authenticate(
    connection: &mut BufReader<TcpStream>,
    service: &OnionService,
    auth: &AuthMethods,
) -> Result<(), String> {
    let offers = |method: &str| auth.methods.iter().any(|offered| offered == method);
    let authenticate = if let Some(password) = &service.password {
        if !offers("HASHEDPASSWORD") {
            return Err(
                "tor_password is set, but Tor does not offer password (HASHEDPASSWORD) authentication"
                    .into(),
            );
        }
        // Hex, which control-spec.txt takes as well as a quoted string and
        // which needs no escaping.
        format!("AUTHENTICATE {}", hex::encode(password.as_bytes()))
    } else if offers("NULL") {
        "AUTHENTICATE".to_string()
    } else if offers("SAFECOOKIE") {
        let cookie_file = auth
            .cookie_file
            .as_deref()
            .ok_or("Tor offers SAFECOOKIE authentication but names no cookie file")?;
        let cookie = read_cookie(cookie_file)?;
        let client_nonce: [u8; NONCE_BYTES] = random();
        let challenge = command(
            connection,
            &format!("AUTHCHALLENGE SAFECOOKIE {}", hex::encode(client_nonce)),
        )
        .await?
        .accepted("AUTHCHALLENGE")?;
        let client_hash = answer_challenge(&challenge, &cookie, &client_nonce)?;
        format!("AUTHENTICATE {}", hex::encode(client_hash))
    } else if offers("HASHEDPASSWORD") {
        return Err("Tor asks for a password (HASHEDPASSWORD): set tor_password".into());
    } else {
        return Err(format!(
            "Tor offers no authentication the node speaks ({})",
            auth.methods.join(",")
        ));
    };
    command(connection, &authenticate)
        .await?
        .accepted("AUTHENTICATE")?;
    Ok(())
}

/// The contents of Tor's cookie file at `path`.
fn read_cookie(path: &str) -> Result<Vec<u8>, String> {
    let cookie =
        fs::read(path).map_err(|error| format!("Cannot read Tor's cookie file {path}: {error}"))?;
    if cookie.len() != COOKIE_BYTES {
        return Err(format!(
            "Tor's cookie file {path} holds {} bytes, not {COOKIE_BYTES}",
            cookie.len()
        ));
    }
    Ok(cookie)
}

/// Checks Tor's `AUTHCHALLENGE` reply, which proves Tor knows the cookie,
/// and gives the hash that proves the node read it (control-spec.txt,
/// `AUTHCHALLENGE`).
fn answer_challenge(
    lines: &[String],
    cookie: &[u8],
    client_nonce: &[u8],
) -> Result<[u8; 32], String> {
    let answer = lines
        .iter()
        .find_map(|line| line.strip_prefix("AUTHCHALLENGE "))
        .ok_or("AUTHCHALLENGE: no answer line")?;
    let mapping = parse_mapping(answer)?;
    let decode = |key: &str| {
        mapping
            .get(key)
            .and_then(|value| hex::decode(value).ok())
            .ok_or_else(|| format!("AUTHCHALLENGE: no {key}"))
    };
    let server_hash = decode("SERVERHASH")?;
    let server_nonce = decode("SERVERNONCE")?;
    if server_nonce.len() != NONCE_BYTES {
        return Err(format!(
            "AUTHCHALLENGE: a server nonce of {} bytes",
            server_nonce.len()
        ));
    }
    let message = [cookie, client_nonce, &server_nonce].concat();
    let mut server_mac =
        HmacSha256::new_from_slice(SERVER_HASH_KEY).expect("HMAC takes any key length");
    server_mac.update(&message);
    server_mac.verify_slice(&server_hash).map_err(|_| {
        "Tor's SERVERHASH does not match its cookie: something else answers on the control port"
            .to_string()
    })?;
    let mut client_mac =
        HmacSha256::new_from_slice(CLIENT_HASH_KEY).expect("HMAC takes any key length");
    client_mac.update(&message);
    Ok(client_mac.finalize().into_bytes().into())
}

/// The `ADD_ONION` command: the saved key or a new one, with
/// `Flags=DiscardPK` when Tor should keep no key, and the virtual port
/// [`ONION_SERVICE_PORT`] forwarded to `target`.
fn add_onion_command(key: Option<&str>, discard_key: bool, target: SocketAddr) -> String {
    let key = key.unwrap_or("NEW:ED25519-V3");
    let flags = if discard_key { " Flags=DiscardPK" } else { "" };
    format!("ADD_ONION {key}{flags} Port={ONION_SERVICE_PORT},{target}")
}

/// What `ADD_ONION` made: the service's address, and its key when Tor made
/// a new one and was not told to discard it.
#[derive(Debug, PartialEq)]
struct Added {
    address: Multiaddr,
    private_key: Option<String>,
}

/// The `ServiceID=` and `PrivateKey=` lines of an `ADD_ONION` reply.
fn parse_add_onion(lines: &[String]) -> Result<Added, String> {
    let service_id = lines
        .iter()
        .find_map(|line| line.strip_prefix("ServiceID="))
        .ok_or("ADD_ONION: no ServiceID")?;
    let private_key = lines
        .iter()
        .find_map(|line| line.strip_prefix("PrivateKey="))
        .map(str::to_string);
    let well_formed = service_id.len() == 56
        && service_id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || (b'2'..=b'7').contains(&byte));
    let address = format!("/onion3/{service_id}:{ONION_SERVICE_PORT}")
        .parse::<Multiaddr>()
        .ok()
        .filter(|address| well_formed && onion3_target(address).is_some())
        .ok_or_else(|| format!("ADD_ONION: {service_id} is not a v3 onion service"))?;
    Ok(Added {
        address,
        private_key,
    })
}

/// Whether `key` is an ed25519 v3 onion key as `ADD_ONION` gives it: the key
/// type, then base64. Nothing else may go into a command line.
fn is_key(key: &str) -> bool {
    key.strip_prefix(KEY_TYPE).is_some_and(|blob| {
        !blob.is_empty()
            && blob
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='))
    })
}

/// The onion key saved in `path`, or None when there is none yet. A file
/// that holds no key is an error rather than replaced, since a new key is a
/// new onion address.
fn read_key(path: &Path) -> Result<Option<String>, String> {
    match fs::read_to_string(path) {
        Ok(text) if is_key(text.trim()) => Ok(Some(text.trim().to_string())),
        Ok(_) => Err(format!(
            "{} does not hold an onion service key. Move it away to get a new onion address",
            path.display()
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("Cannot read {}: {error}", path.display())),
    }
}

/// Writes `key` to a new file at `path` that only its owner can read, as the
/// node key is written (`node_key`). Refuses to overwrite a file.
fn write_key(path: &Path, key: &str) -> Result<(), String> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Cannot create {}: {error}", parent.display()))?;
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|error| format!("Cannot create {}: {error}", path.display()))?;
    file.write_all(key.as_bytes())
        .map_err(|error| format!("Cannot write {}: {error}", path.display()))?;
    file.sync_all()
        .map_err(|error| format!("Cannot write {}: {error}", path.display()))
}

/// The `KEY=VALUE` pairs of a reply line, a quoted value unescaped, as
/// Bitcoin Cash Node's `ParseTorReplyMapping` reads them. A word without
/// `=` ends the pairs.
fn parse_mapping(text: &str) -> Result<HashMap<String, String>, String> {
    let mut mapping = HashMap::new();
    let mut characters = text.chars().peekable();
    loop {
        while characters.next_if_eq(&' ').is_some() {}
        if characters.peek().is_none() {
            return Ok(mapping);
        }
        let mut key = String::new();
        while let Some(character) =
            characters.next_if(|character| *character != '=' && *character != ' ')
        {
            key.push(character);
        }
        if characters.next_if_eq(&'=').is_none() {
            return Ok(mapping);
        }
        let value = if characters.next_if_eq(&'"').is_some() {
            unquote(&mut characters)?
        } else {
            let mut value = String::new();
            while let Some(character) = characters.next_if(|character| *character != ' ') {
                value.push(character);
            }
            value
        };
        mapping.insert(key, value);
    }
}

/// The rest of a quoted string whose opening quote was read, up to its
/// closing quote, unescaped as control-spec.txt says: `\n`, `\t`, `\r`, up
/// to three octal digits for a byte, and a backslash before any other
/// character for that character.
fn unquote(characters: &mut Peekable<Chars<'_>>) -> Result<String, String> {
    let mut bytes = Vec::new();
    loop {
        let character = match characters.next() {
            None => return Err("a quoted string with no closing quote".into()),
            Some('"') => return Ok(String::from_utf8_lossy(&bytes).into_owned()),
            Some('\\') => match characters.next() {
                None => return Err("a quoted string with no closing quote".into()),
                Some('n') => '\n',
                Some('t') => '\t',
                Some('r') => '\r',
                Some(first @ '0'..='7') => {
                    // Three digits only up to \377, so the byte fits.
                    let most = if first <= '3' { 3 } else { 2 };
                    let mut byte = first.to_digit(8).unwrap_or(0);
                    for _ in 1..most {
                        match characters.next_if(|next| next.is_digit(8)) {
                            Some(next) => byte = byte * 8 + next.to_digit(8).unwrap_or(0),
                            None => break,
                        }
                    }
                    bytes.push(u8::try_from(byte).unwrap_or(u8::MAX));
                    continue;
                }
                Some(other) => other,
            },
            Some(other) => other,
        };
        let mut buffer = [0; 4];
        bytes.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
    }
}

#[cfg(test)]
mod tests;
