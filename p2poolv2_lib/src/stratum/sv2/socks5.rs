// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! A SOCKS5 `CONNECT` through a proxy, for a Stratum V2 peer that is reached
//! over Tor or I2P.
//!
//! Tor's SOCKS port takes a `.onion` name, and an I2P router's SOCKS proxy
//! (i2pd: port 4447) takes a `.b32.i2p` name, so one dialer serves both. A
//! host name is sent to the proxy as a name (address type `0x03`, RFC 1928)
//! and is never resolved on this computer: resolving it here would tell the
//! local resolver which peer the node talks to, and an onion or I2P name has
//! no address to resolve to.
//!
//! Every connection offers "no authentication" and username/password
//! (RFC 1929) with a fresh random pair. Tor takes the pair and puts
//! connections with different pairs on different circuits, so a reconnect
//! does not ride the circuit that just failed. A proxy without
//! username/password picks "no authentication".

use bitcoin::secp256k1::rand::random;
use std::net::IpAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const VERSION: u8 = 0x05;
const NO_AUTHENTICATION: u8 = 0x00;
const USERNAME_PASSWORD: u8 = 0x02;
const NO_ACCEPTABLE_METHODS: u8 = 0xff;
/// Version of the username/password negotiation (RFC 1929).
const AUTHENTICATION_VERSION: u8 = 0x01;
const CONNECT: u8 = 0x01;
const ADDRESS_IPV4: u8 = 0x01;
const ADDRESS_NAME: u8 = 0x03;
const ADDRESS_IPV6: u8 = 0x04;
const SUCCEEDED: u8 = 0x00;

/// Open a TCP connection to `proxy`, a SOCKS5 proxy at `HOST:PORT`, and have
/// it connect to `target`, a `HOST:PORT` whose host may be a name, an IP
/// address, or an IPv6 address in brackets.
///
/// The returned stream carries the target's bytes from then on. The caller
/// bounds the whole dial with a timeout.
pub(crate) async fn connect(proxy: &str, target: &str) -> Result<TcpStream, String> {
    let (host, port) = split_host_port(target)?;
    let mut stream = TcpStream::connect(proxy)
        .await
        .map_err(|error| format!("connect to the proxy at {proxy}: {error}"))?;
    negotiate(&mut stream, host, port)
        .await
        .map_err(|error| format!("proxy at {proxy}: {error}"))?;
    Ok(stream)
}

/// Whether `host` is an I2P name. A connection to one takes far longer to
/// set up than one through Tor, so its callers wait longer.
pub(crate) fn is_i2p_host(host: &str) -> bool {
    host.to_ascii_lowercase().ends_with(".i2p")
}

/// `HOST:PORT` as its host, without the brackets of an IPv6 address, and its
/// port.
pub(crate) fn split_host_port(address: &str) -> Result<(&str, u16), String> {
    let (host, port) = address
        .rsplit_once(':')
        .ok_or_else(|| format!("{address} is not HOST:PORT"))?;
    let port = port
        .parse::<u16>()
        .map_err(|_| format!("{address} is not HOST:PORT"))?;
    let host = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host);
    if host.is_empty() {
        return Err(format!("{address} is not HOST:PORT"));
    }
    Ok((host, port))
}

/// Run the SOCKS5 negotiation for a `CONNECT` to `host` and `port` on a
/// stream that reaches the proxy.
async fn negotiate<Stream>(stream: &mut Stream, host: &str, port: u16) -> Result<(), String>
where
    Stream: AsyncReadExt + AsyncWriteExt + Unpin,
{
    let request = connect_request(host, port)?;
    stream
        .write_all(&[VERSION, 2, NO_AUTHENTICATION, USERNAME_PASSWORD])
        .await
        .map_err(|error| format!("write: {error}"))?;
    let mut choice = [0u8; 2];
    read(stream, &mut choice).await?;
    if choice[0] != VERSION {
        return Err(format!("not a SOCKS5 proxy (version {})", choice[0]));
    }
    match choice[1] {
        NO_AUTHENTICATION => (),
        USERNAME_PASSWORD => authenticate(stream).await?,
        NO_ACCEPTABLE_METHODS => {
            return Err("it accepts neither no authentication nor a username".into());
        }
        method => return Err(format!("it chose authentication method {method}")),
    }
    stream
        .write_all(&request)
        .await
        .map_err(|error| format!("write: {error}"))?;
    let mut reply = [0u8; 4];
    read(stream, &mut reply).await?;
    if reply[0] != VERSION {
        return Err(format!("malformed reply (version {})", reply[0]));
    }
    if reply[1] != SUCCEEDED {
        return Err(format!(
            "could not connect to {host}:{port}: {}",
            reply_text(reply[1])
        ));
    }
    // The address the proxy bound for the connection follows; it is read
    // off the stream so the target's first byte is the next one.
    let bound = match reply[3] {
        ADDRESS_IPV4 => 4,
        ADDRESS_IPV6 => 16,
        ADDRESS_NAME => {
            let mut length = [0u8; 1];
            read(stream, &mut length).await?;
            usize::from(length[0])
        }
        kind => return Err(format!("malformed reply (address type {kind})")),
    };
    let mut rest = vec![0u8; bound + 2];
    read(stream, &mut rest).await
}

/// Send a random username and password, which the proxy accepts whatever
/// they are, and read its answer.
async fn authenticate<Stream>(stream: &mut Stream) -> Result<(), String>
where
    Stream: AsyncReadExt + AsyncWriteExt + Unpin,
{
    let username = hex::encode(random::<[u8; 8]>());
    let password = hex::encode(random::<[u8; 8]>());
    let mut message = Vec::with_capacity(3 + username.len() + password.len());
    message.push(AUTHENTICATION_VERSION);
    message.push(username.len() as u8);
    message.extend_from_slice(username.as_bytes());
    message.push(password.len() as u8);
    message.extend_from_slice(password.as_bytes());
    stream
        .write_all(&message)
        .await
        .map_err(|error| format!("write: {error}"))?;
    let mut answer = [0u8; 2];
    read(stream, &mut answer).await?;
    if answer[1] != SUCCEEDED {
        return Err("it refused the username and password".into());
    }
    Ok(())
}

/// The `CONNECT` request for `host` and `port`: an IP address as its bytes,
/// anything else as a name for the proxy to resolve.
fn connect_request(host: &str, port: u16) -> Result<Vec<u8>, String> {
    let mut request = vec![VERSION, CONNECT, 0x00];
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(address)) => {
            request.push(ADDRESS_IPV4);
            request.extend_from_slice(&address.octets());
        }
        Ok(IpAddr::V6(address)) => {
            request.push(ADDRESS_IPV6);
            request.extend_from_slice(&address.octets());
        }
        Err(_) => {
            let length = u8::try_from(host.len())
                .ok()
                .filter(|length| *length > 0)
                .ok_or_else(|| format!("host name of {} bytes", host.len()))?;
            request.push(ADDRESS_NAME);
            request.push(length);
            request.extend_from_slice(host.as_bytes());
        }
    }
    request.extend_from_slice(&port.to_be_bytes());
    Ok(request)
}

async fn read<Stream>(stream: &mut Stream, buffer: &mut [u8]) -> Result<(), String>
where
    Stream: AsyncReadExt + Unpin,
{
    stream
        .read_exact(buffer)
        .await
        .map(|_| ())
        .map_err(|error| format!("read: {error}"))
}

/// What a reply code other than success says (RFC 1928, section 6).
fn reply_text(code: u8) -> String {
    let text = match code {
        0x01 => "general failure",
        0x02 => "not allowed by the proxy's rules",
        0x03 => "network unreachable",
        0x04 => "host unreachable",
        0x05 => "connection refused",
        0x06 => "timed out on the way",
        0x07 => "command not supported",
        0x08 => "address type not supported",
        _ => return format!("reply code {code:#04x}"),
    };
    text.to_string()
}

#[cfg(test)]
mod tests;
