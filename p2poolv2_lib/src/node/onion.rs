// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Dialing Tor onion services: `/onion3/<address>:<port>` peers are reached
//! through a local Tor daemon's SOCKS5 port (`[network] tor_proxy`), as
//! Bitcoin Cash Node's `-onion` does. The onion name goes to the proxy
//! unresolved, so nothing about it is looked up locally. Without a
//! `tor_proxy`, Tor's default SOCKS port is used once Tor's control port has
//! made the node's onion service (`tor_control`), as Bitcoin Cash Node does.
//!
//! A v3 onion address is 35 bytes: the service's ed25519 key (32), a
//! checksum (2) and the version byte 3 (rend-spec-v3, "Encoding onion
//! addresses"). Its host name is those bytes in lowercase base32 plus
//! `.onion`. Tor checks the checksum itself and refuses a bad address before
//! it builds a circuit; this module checks the length and the version.

use crate::stratum::sv2::socks5;
use futures::future::{BoxFuture, Pending};
use libp2p::Multiaddr;
use libp2p::core::transport::{DialOpts, ListenerId, Transport, TransportError, TransportEvent};
use libp2p::multiaddr::Protocol;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, PoisonError, RwLock};
use std::task::{Context, Poll};
use tracing::debug;

/// The last byte of a v3 onion address.
const ONION3_VERSION: u8 = 3;

/// Whether the 35 bytes of an `/onion3` address carry version 3.
pub(crate) fn is_valid_onion3(address: &[u8; 35]) -> bool {
    address[34] == ONION3_VERSION
}

/// The SOCKS5 host (`<56 base32 characters>.onion`) and port of an `/onion3`
/// address, alone or followed by `/p2p`. None for any other address and for
/// an onion address that is not version 3.
pub(crate) fn onion3_target(address: &Multiaddr) -> Option<(String, u16)> {
    let mut protocols = address.iter();
    let Some(Protocol::Onion3(onion)) = protocols.next() else {
        return None;
    };
    if !matches!(protocols.next(), None | Some(Protocol::P2p(_))) || protocols.next().is_some() {
        return None;
    }
    is_valid_onion3(onion.hash()).then(|| {
        (
            format!("{}.onion", base32_lower(onion.hash())),
            onion.port(),
        )
    })
}

/// RFC 4648 base32 in lowercase without padding: 35 bytes give the 56
/// characters of an onion host name.
fn base32_lower(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(5) * 8);
    let mut buffer: u32 = 0;
    let mut bits: u32 = 0;
    for &byte in bytes {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            encoded.push(char::from(ALPHABET[((buffer >> bits) & 0x1f) as usize]));
        }
    }
    if bits > 0 {
        encoded.push(char::from(
            ALPHABET[((buffer << (5 - bits)) & 0x1f) as usize],
        ));
    }
    encoded
}

/// The Tor SOCKS5 proxy onion dials go through, `host:port`, shared by the
/// transport and the node: `tor_proxy`, or none until Tor's control port
/// has made the node's onion service and the node sets Tor's default. A
/// poisoned lock is recovered: a panic cannot leave a proxy address half
/// written.
#[derive(Clone)]
pub(crate) struct OnionProxy(Arc<RwLock<Option<String>>>);

impl OnionProxy {
    pub(crate) fn new(proxy: Option<String>) -> Self {
        Self(Arc::new(RwLock::new(proxy)))
    }

    fn get(&self) -> Option<String> {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Sets `proxy` unless a proxy is set already; whether it did.
    pub(crate) fn set_if_unset(&self, proxy: &str) -> bool {
        let mut current = self.0.write().unwrap_or_else(PoisonError::into_inner);
        if current.is_some() {
            return false;
        }
        *current = Some(proxy.to_string());
        true
    }
}

/// Dials `/onion3` addresses through the SOCKS5 proxy in `proxy` and refuses
/// every other address with `MultiaddrNotSupported`, so `or_transport` hands
/// it to the transport beside it; onion addresses too while no proxy is set.
/// It listens on nothing: inbound onion connections reach the node's TCP
/// listener from the Tor daemon.
pub(crate) struct OnionTransport {
    proxy: OnionProxy,
}

impl OnionTransport {
    pub(crate) fn new(proxy: OnionProxy) -> Self {
        Self { proxy }
    }
}

impl Transport for OnionTransport {
    type Output = libp2p::tcp::tokio::TcpStream;
    type Error = io::Error;
    type ListenerUpgrade = Pending<Result<Self::Output, Self::Error>>;
    type Dial = BoxFuture<'static, Result<Self::Output, Self::Error>>;

    fn listen_on(
        &mut self,
        _id: ListenerId,
        address: Multiaddr,
    ) -> Result<(), TransportError<Self::Error>> {
        Err(TransportError::MultiaddrNotSupported(address))
    }

    fn remove_listener(&mut self, _id: ListenerId) -> bool {
        false
    }

    fn dial(
        &mut self,
        address: Multiaddr,
        _opts: DialOpts,
    ) -> Result<Self::Dial, TransportError<Self::Error>> {
        let Some((host, port)) = onion3_target(&address) else {
            return Err(TransportError::MultiaddrNotSupported(address));
        };
        let Some(proxy) = self.proxy.get() else {
            return Err(TransportError::MultiaddrNotSupported(address));
        };
        debug!("Dialing {host}:{port} through the Tor proxy at {proxy}");
        Ok(Box::pin(async move {
            let stream = socks5::connect(&proxy, &format!("{host}:{port}"))
                .await
                .map_err(io::Error::other)?;
            Ok(libp2p::tcp::tokio::TcpStream(stream))
        }))
    }

    fn poll(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<TransportEvent<Self::ListenerUpgrade, Self::Error>> {
        Poll::Pending
    }
}

#[cfg(test)]
mod tests;
