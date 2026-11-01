// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! New-block announcements from the node's P2P port.
//!
//! Among Bitcoin Cash nodes only BCHN publishes ZMQ `hashblock`; BCHD and
//! Knuth do not, so without this listener the template task hears of a new
//! block only at its next poll. Like jtoomim's p2pool
//! (`p2pool/bitcoin/p2p.py`, `handle_inv`), it connects to the node as a peer
//! that relays no transactions, and wakes the template task when the node
//! announces a block with an `inv` or a `headers` message. It never asks the
//! node for data, so nothing it reads is bigger than an announcement.

use crate::config::BitcoinCashNetwork;
use bitcoin::Network;
use bitcoin::consensus::encode::{deserialize, serialize};
use bitcoin::p2p::address::Address;
use bitcoin::p2p::message::{MAX_MSG_SIZE, NetworkMessage, RawNetworkMessage};
use bitcoin::p2p::message_blockdata::Inventory;
use bitcoin::p2p::message_network::VersionMessage;
use bitcoin::p2p::{Magic, ServiceFlags};
use chrono::Utc;
use std::io::ErrorKind;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::{sleep, timeout};
use tracing::{debug, info, warn};

const NODE_P2P_CHANNEL_SIZE: usize = 1;
/// The protocol version this listener speaks: BCHN's `PROTOCOL_VERSION`
/// (`src/version.h`).
const NODE_PROTOCOL_VERSION: u32 = 70016;
/// Bytes in a message header: network magic (4), command (12), payload size
/// (4) and checksum (4).
const MESSAGE_HEADER_SIZE: usize = 24;
/// Where the little-endian payload size starts in a message header.
const PAYLOAD_SIZE_OFFSET: usize = 16;
/// Time to connect and complete the version handshake.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
/// BCHN, BCHD and Knuth each ping a peer every two minutes, so this long
/// without a message means the connection is gone.
const IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const FIRST_RETRY_DELAY: Duration = Duration::from_secs(1);
const LAST_RETRY_DELAY: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub struct NodeP2pError {
    pub message: String,
}
impl std::fmt::Display for NodeP2pError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Node P2P Error: {}", self.message)
    }
}
impl std::error::Error for NodeP2pError {}

impl From<std::io::Error> for NodeP2pError {
    fn from(error: std::io::Error) -> Self {
        let message = match error.kind() {
            ErrorKind::UnexpectedEof => "the node closed the connection".to_string(),
            _ => error.to_string(),
        };
        NodeP2pError { message }
    }
}

pub trait NodeP2pListenerTrait {
    /// Starts a task that connects to the node's P2P port at `address`
    /// (`HOST:PORT`), speaking the network with `magic`.
    /// Sends a unit message to a channel whenever the node announces a new
    /// block, and reconnects after any error, until the receiver is dropped.
    /// Returns the receiver end of the channel.
    fn start(&self, address: &str, magic: Magic) -> Result<mpsc::Receiver<()>, NodeP2pError>;
}

#[derive(Default)]
pub struct NodeP2pListener;

impl NodeP2pListenerTrait for NodeP2pListener {
    fn start(&self, address: &str, magic: Magic) -> Result<mpsc::Receiver<()>, NodeP2pError> {
        let has_port = address
            .rsplit_once(':')
            .is_some_and(|(host, port)| !host.is_empty() && port.parse::<u16>().is_ok());
        if !has_port {
            return Err(NodeP2pError {
                message: format!("node_p2p_address must be HOST:PORT, got {address}"),
            });
        }
        let (trigger_tx, trigger_rx) = mpsc::channel::<()>(NODE_P2P_CHANNEL_SIZE);
        tokio::spawn(listen(address.to_string(), magic, trigger_tx));
        Ok(trigger_rx)
    }
}

/// The network magic, the first four bytes of every P2P message, of the
/// node's network. `bitcoin_cash_network` is `None` on a Bitcoin node.
///
/// The Bitcoin Cash values are BCHN's `netMagic` (`src/chainparams.cpp`).
/// They differ from Bitcoin's on every network; chipnet shares testnet4's.
pub fn network_magic(bitcoin_cash_network: Option<BitcoinCashNetwork>, network: Network) -> Magic {
    match bitcoin_cash_network {
        Some(BitcoinCashNetwork::Mainnet) => Magic::from_bytes([0xe3, 0xe1, 0xf3, 0xe8]),
        Some(BitcoinCashNetwork::Chipnet) => Magic::from_bytes([0xe2, 0xb7, 0xda, 0xaf]),
        Some(BitcoinCashNetwork::Scalenet) => Magic::from_bytes([0xc3, 0xaf, 0xe1, 0xa2]),
        Some(BitcoinCashNetwork::Regtest) => Magic::from_bytes([0xda, 0xb5, 0xbf, 0xfa]),
        None => Magic::from(network),
    }
}

/// Keep a connection to the node and relay its block announcements to
/// `trigger_tx`, until the receiver is dropped.
///
/// After a failed connection the task retries, waiting one second at first
/// and twice as long each time, up to thirty seconds. It warns once per
/// outage and logs the retries at debug level.
async fn listen(address: String, magic: Magic, trigger_tx: mpsc::Sender<()>) {
    let mut retry_delay = FIRST_RETRY_DELAY;
    let mut connection_lost = false;
    while !trigger_tx.is_closed() {
        let session = match connect(&address, magic).await {
            Ok(mut stream) => {
                info!("Listening for new blocks on the node's P2P port at {address}");
                connection_lost = false;
                retry_delay = FIRST_RETRY_DELAY;
                relay_block_announcements(&mut stream, magic, &trigger_tx).await
            }
            Err(error) => Err(error),
        };
        if let Err(error) = session {
            if connection_lost {
                debug!(
                    "Node P2P connection to {address} is still down: {}",
                    error.message
                );
            } else {
                warn!(
                    "Node P2P connection to {address} failed: {}. Retrying until it is back",
                    error.message
                );
                connection_lost = true;
            }
            sleep(retry_delay).await;
            retry_delay = (retry_delay * 2).min(LAST_RETRY_DELAY);
        }
    }
    debug!("Template trigger channel closed, stopping the node P2P listener");
}

/// Connect to the node and complete the version handshake.
async fn connect(address: &str, magic: Magic) -> Result<TcpStream, NodeP2pError> {
    let connecting = async {
        let mut stream = TcpStream::connect(address).await?;
        handshake(&mut stream, magic).await?;
        Ok::<TcpStream, NodeP2pError>(stream)
    };
    timeout(HANDSHAKE_TIMEOUT, connecting)
        .await
        .map_err(|_| NodeP2pError {
            message: format!(
                "no handshake within {} seconds",
                HANDSHAKE_TIMEOUT.as_secs()
            ),
        })?
}

/// Send our `version`, answer the node's `version` with `verack`, and return
/// once both its `version` and its `verack` have arrived.
async fn handshake<Stream>(stream: &mut Stream, magic: Magic) -> Result<(), NodeP2pError>
where
    Stream: AsyncRead + AsyncWrite + Unpin,
{
    send_message(stream, magic, NetworkMessage::Version(version_message())).await?;
    let mut version_received = false;
    let mut verack_received = false;
    while !(version_received && verack_received) {
        match read_message(stream, magic).await? {
            Some(NetworkMessage::Version(_)) => {
                version_received = true;
                send_message(stream, magic, NetworkMessage::Verack).await?;
            }
            Some(NetworkMessage::Verack) => verack_received = true,
            Some(NetworkMessage::Ping(nonce)) => {
                send_message(stream, magic, NetworkMessage::Pong(nonce)).await?
            }
            _ => {}
        }
    }
    Ok(())
}

/// Answer the node's pings and send a unit message to `trigger_tx` for each
/// block the node announces. Returns when the receiver is dropped, or with
/// the error that ended the connection.
///
/// The channel holds one wake-up, so announcements that arrive while one is
/// pending become one template fetch.
async fn relay_block_announcements<Stream>(
    stream: &mut Stream,
    magic: Magic,
    trigger_tx: &mpsc::Sender<()>,
) -> Result<(), NodeP2pError>
where
    Stream: AsyncRead + AsyncWrite + Unpin,
{
    while !trigger_tx.is_closed() {
        let message = timeout(IDLE_TIMEOUT, read_message(stream, magic))
            .await
            .map_err(|_| NodeP2pError {
                message: format!(
                    "no message from the node in {} seconds",
                    IDLE_TIMEOUT.as_secs()
                ),
            })??;
        match message {
            Some(NetworkMessage::Ping(nonce)) => {
                send_message(stream, magic, NetworkMessage::Pong(nonce)).await?
            }
            Some(message) if announces_block(&message) => {
                debug!("The node announced a new block over P2P");
                // A full channel already holds a wake-up, and a closed one
                // ends the loop.
                let _ = trigger_tx.try_send(());
            }
            _ => {}
        }
    }
    Ok(())
}

/// Whether `message` announces a block: an `inv` with a block entry
/// (`MSG_BLOCK`), or `headers` with at least one header.
fn announces_block(message: &NetworkMessage) -> bool {
    match message {
        NetworkMessage::Inv(inventory) => inventory
            .iter()
            .any(|item| matches!(item, Inventory::Block(_))),
        NetworkMessage::Headers(headers) => !headers.is_empty(),
        _ => false,
    }
}

/// Our `version`: no services, so the node asks us for nothing; no relay,
/// so it announces no transactions; start height 0, so it announces every
/// new block. Both addresses are left unspecified, as BCHN leaves its own.
fn version_message() -> VersionMessage {
    let unspecified = Address::new(
        &SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)),
        ServiceFlags::NONE,
    );
    VersionMessage {
        version: NODE_PROTOCOL_VERSION,
        services: ServiceFlags::NONE,
        timestamp: Utc::now().timestamp(),
        receiver: unspecified.clone(),
        sender: unspecified,
        // Only compared with the node's own nonces, to find a connection to
        // itself.
        nonce: 0,
        user_agent: format!("/P2Poolv2:{}/", env!("CARGO_PKG_VERSION")),
        start_height: 0,
        relay: false,
    }
}

/// Read one message from the node.
///
/// Returns `None` for a message that does not decode, such as one with a
/// bad checksum; it is skipped whole. Fails on another network's magic (a
/// node on another network, or a port that is not a P2P port), on a payload
/// over `MAX_MSG_SIZE` (the most the bitcoin crate decodes; BCHN sends
/// nothing but blocks over 2 MB, and this listener never asks for blocks),
/// and on a read error.
async fn read_message<Reader>(
    reader: &mut Reader,
    magic: Magic,
) -> Result<Option<NetworkMessage>, NodeP2pError>
where
    Reader: AsyncRead + Unpin,
{
    let mut header = [0u8; MESSAGE_HEADER_SIZE];
    reader.read_exact(&mut header).await?;
    let received_magic = Magic::from_bytes([header[0], header[1], header[2], header[3]]);
    if received_magic != magic {
        return Err(NodeP2pError {
            message: format!(
                "network magic {received_magic}, expected {magic}: is this the P2P port of a node on this network?"
            ),
        });
    }
    let payload_size = u32::from_le_bytes([
        header[PAYLOAD_SIZE_OFFSET],
        header[PAYLOAD_SIZE_OFFSET + 1],
        header[PAYLOAD_SIZE_OFFSET + 2],
        header[PAYLOAD_SIZE_OFFSET + 3],
    ]) as usize;
    if payload_size > MAX_MSG_SIZE {
        return Err(NodeP2pError {
            message: format!("payload of {payload_size} bytes, over {MAX_MSG_SIZE}"),
        });
    }
    let mut message = Vec::with_capacity(MESSAGE_HEADER_SIZE + payload_size);
    message.extend_from_slice(&header);
    message.resize(MESSAGE_HEADER_SIZE + payload_size, 0);
    reader
        .read_exact(&mut message[MESSAGE_HEADER_SIZE..])
        .await?;
    match deserialize::<RawNetworkMessage>(&message) {
        Ok(raw_message) => Ok(Some(raw_message.into_payload())),
        Err(error) => {
            debug!("Skipping a node P2P message that does not decode: {error}");
            Ok(None)
        }
    }
}

/// Write one message to the node.
async fn send_message<Writer>(
    writer: &mut Writer,
    magic: Magic,
    payload: NetworkMessage,
) -> Result<(), NodeP2pError>
where
    Writer: AsyncWrite + Unpin,
{
    writer
        .write_all(&serialize(&RawNetworkMessage::new(magic, payload)))
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::hashes::{Hash, sha256d};
    use bitcoin::{BlockHash, Txid};
    use tokio::net::TcpListener;
    use tokio::sync::mpsc::error::TryRecvError;

    #[tokio::test]
    async fn a_message_round_trips_with_the_chipnet_magic() {
        let magic = network_magic(Some(BitcoinCashNetwork::Chipnet), Network::Testnet4);
        let mut wire = Vec::with_capacity(MESSAGE_HEADER_SIZE + 8);
        send_message(&mut wire, magic, NetworkMessage::Ping(42))
            .await
            .unwrap();

        assert_eq!(wire[..4], [0xe2, 0xb7, 0xda, 0xaf]);
        assert_eq!(wire[4..16], *b"ping\0\0\0\0\0\0\0\0");
        assert_eq!(wire[16..20], 8u32.to_le_bytes());
        let checksum = sha256d::Hash::hash(&wire[MESSAGE_HEADER_SIZE..]);
        assert_eq!(wire[20..24], checksum[..4]);
        let message = read_message(&mut wire.as_slice(), magic).await.unwrap();
        assert_eq!(message, Some(NetworkMessage::Ping(42)));
    }

    #[tokio::test]
    async fn a_message_on_another_network_fails() {
        let mainnet = network_magic(Some(BitcoinCashNetwork::Mainnet), Network::Bitcoin);
        let chipnet = network_magic(Some(BitcoinCashNetwork::Chipnet), Network::Testnet4);
        let mut wire = Vec::with_capacity(MESSAGE_HEADER_SIZE + 8);
        send_message(&mut wire, mainnet, NetworkMessage::Ping(42))
            .await
            .unwrap();

        let error = read_message(&mut wire.as_slice(), chipnet)
            .await
            .unwrap_err();
        assert!(error.message.contains("network magic e3e1f3e8"), "{error}");
    }

    #[tokio::test]
    async fn an_oversized_payload_fails_before_it_is_read() {
        let magic = network_magic(Some(BitcoinCashNetwork::Regtest), Network::Regtest);
        let mut header = [0u8; MESSAGE_HEADER_SIZE];
        header[..4].copy_from_slice(&magic.to_bytes());
        header[4..7].copy_from_slice(b"inv");
        header[16..20].copy_from_slice(&(MAX_MSG_SIZE as u32 + 1).to_le_bytes());

        let error = read_message(&mut header.as_slice(), magic)
            .await
            .unwrap_err();
        assert!(
            error.message.contains("payload of 5000001 bytes"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn a_block_inv_wakes_the_template_task() {
        let magic = network_magic(Some(BitcoinCashNetwork::Regtest), Network::Regtest);
        let node = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = node.local_addr().unwrap().to_string();
        let mut trigger_rx = NodeP2pListener.start(&address, magic).unwrap();
        let (mut peer, _) = node.accept().await.unwrap();
        let version = read_message(&mut peer, magic).await.unwrap();
        assert!(matches!(version, Some(NetworkMessage::Version(_))));
        send_message(&mut peer, magic, NetworkMessage::Version(version_message()))
            .await
            .unwrap();
        send_message(&mut peer, magic, NetworkMessage::Verack)
            .await
            .unwrap();
        let verack = read_message(&mut peer, magic).await.unwrap();
        assert_eq!(verack, Some(NetworkMessage::Verack));

        let block = Inventory::Block(BlockHash::all_zeros());
        send_message(&mut peer, magic, NetworkMessage::Inv(vec![block]))
            .await
            .unwrap();

        let wake = timeout(Duration::from_secs(5), trigger_rx.recv())
            .await
            .expect("a wake-up arrives");
        assert_eq!(wake, Some(()));
    }

    #[tokio::test]
    async fn a_transaction_inv_does_not_wake_the_template_task() {
        let magic = network_magic(Some(BitcoinCashNetwork::Regtest), Network::Regtest);
        let node = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = node.local_addr().unwrap().to_string();
        let mut trigger_rx = NodeP2pListener.start(&address, magic).unwrap();
        let (mut peer, _) = node.accept().await.unwrap();
        let version = read_message(&mut peer, magic).await.unwrap();
        assert!(matches!(version, Some(NetworkMessage::Version(_))));
        send_message(&mut peer, magic, NetworkMessage::Version(version_message()))
            .await
            .unwrap();
        send_message(&mut peer, magic, NetworkMessage::Verack)
            .await
            .unwrap();
        let verack = read_message(&mut peer, magic).await.unwrap();
        assert_eq!(verack, Some(NetworkMessage::Verack));

        let transaction = Inventory::Transaction(Txid::all_zeros());
        send_message(&mut peer, magic, NetworkMessage::Inv(vec![transaction]))
            .await
            .unwrap();
        // Messages are handled in order, so once the pong is back the inv
        // has been handled.
        send_message(&mut peer, magic, NetworkMessage::Ping(7))
            .await
            .unwrap();
        let pong = read_message(&mut peer, magic).await.unwrap();
        assert_eq!(pong, Some(NetworkMessage::Pong(7)));

        assert_eq!(trigger_rx.try_recv(), Err(TryRecvError::Empty));
    }

    #[tokio::test]
    async fn a_ping_gets_a_pong_with_the_same_nonce() {
        let magic = network_magic(Some(BitcoinCashNetwork::Regtest), Network::Regtest);
        let node = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = node.local_addr().unwrap().to_string();
        let _trigger_rx = NodeP2pListener.start(&address, magic).unwrap();
        let (mut peer, _) = node.accept().await.unwrap();
        let version = read_message(&mut peer, magic).await.unwrap();
        assert!(matches!(version, Some(NetworkMessage::Version(_))));
        send_message(&mut peer, magic, NetworkMessage::Version(version_message()))
            .await
            .unwrap();
        send_message(&mut peer, magic, NetworkMessage::Verack)
            .await
            .unwrap();
        let verack = read_message(&mut peer, magic).await.unwrap();
        assert_eq!(verack, Some(NetworkMessage::Verack));

        send_message(
            &mut peer,
            magic,
            NetworkMessage::Ping(0x0123_4567_89ab_cdef),
        )
        .await
        .unwrap();

        let pong = read_message(&mut peer, magic).await.unwrap();
        assert_eq!(pong, Some(NetworkMessage::Pong(0x0123_4567_89ab_cdef)));
    }
}
