// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Local Stratum V2 listener.
//!
//! Connections use authenticated Noise when an authority is configured.
//! Job Declaration
//! and Mining are separate connections that share one [`super::JobRegistry`].
//! A Mining connection may also open standard and extended channels, which
//! get a job for every template the SV1 server sees (see `super::mining`).

use super::mining::{MiningChannels, custom_job_error, is_mining_channel_message};
use super::noise::NoiseTransport;
use super::session::{
    JobRegistry, PoolTransactionCache, Sv2Connection, Sv2Message, encode_extension_frame,
};
use crate::config::Sv2Authority;
use crate::shares::merge_mining::MergeMiningCommitment;
use crate::stratum::merge_disclosure::{
    EXTENSIONS_NEGOTIATION, MERGE_DISCLOSURE_EXTENSION, REQUEST_EXTENSIONS,
    REQUEST_EXTENSIONS_SUCCESS, extensions_payload, requested_extensions,
};
use crate::stratum::server::StratumContext;
use crate::stratum::work::prepared_notify::PreparedNotifyParams;
use binary_sv2::Deserialize;
use mining_sv2::{MESSAGE_TYPE_SET_CUSTOM_MINING_JOB, SetCustomMiningJob};
use std::future;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, watch};
use tokio::time::timeout;
use tracing::{debug, error, info};

/// Time a client has to complete the Noise handshake after connecting.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Watch channel the SV1 notifier publishes prepared templates on.
pub(crate) type TemplateReceiver = watch::Receiver<Option<Arc<PreparedNotifyParams>>>;

/// What a listener needs to serve Mining Protocol channels: the SV1 stratum
/// context, the version-rolling mask, and the template watch.
pub(crate) struct Sv2Mining {
    pub ctx: StratumContext,
    pub version_mask: i32,
    pub template_rx: TemplateReceiver,
}

/// Bind the Stratum V2 listener to `hostname:port`.
///
/// The address is resolved the way the SV1 listener resolves its own, so a
/// host name such as `localhost` works as well as an IP literal. An error
/// here means the node has no SV2 socket, and the caller should stop.
pub async fn bind(hostname: &str, port: u16) -> Result<TcpListener, std::io::Error> {
    let listener = TcpListener::bind((hostname, port)).await?;
    info!("Stratum V2 listening on {}", listener.local_addr()?);
    Ok(listener)
}

/// Serve `listener` until `shutdown` changes or its sender is dropped.
///
/// `merge_mining` is `Some` for Bitcoin Cash, where every allocated job token
/// carries the AuxPoW commitment output. Bitcoin passes `None`. `mining` is
/// `None` when the listener only speaks Job Declaration. With `authority`,
/// every connection must complete the Noise NX handshake signed by that key
/// before any message; without it frames are unencrypted, which the
/// specification allows only between local roles. A failed accept is logged
/// and the listener keeps serving, as the SV1 server does.
///
/// Each connection holds a permit from `connection_limit`, the semaphore the
/// SV1 server takes its own permits from, so the two ports share one
/// connection cap. A connection arriving with no permit left is closed.
pub(crate) async fn serve<ShutdownSignal>(
    listener: TcpListener,
    connection_limit: Arc<Semaphore>,
    pool_transactions: PoolTransactionCache,
    merge_mining: Option<MergeMiningCommitment>,
    mining: Option<Sv2Mining>,
    authority: Option<Sv2Authority>,
    mut shutdown: watch::Receiver<ShutdownSignal>,
) where
    ShutdownSignal: Send + Sync + 'static,
{
    let registry = Arc::new(Mutex::new(JobRegistry::new(
        pool_transactions,
        merge_mining,
    )));
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, peer)) => {
                        let permit = match Arc::clone(&connection_limit).try_acquire_owned() {
                            Ok(permit) => permit,
                            Err(_) => {
                                debug!("Stratum connection limit reached, rejecting SV2 {peer}");
                                continue;
                            }
                        };
                        let connection_mining = mining.as_ref().map(|services| Sv2Mining {
                            ctx: services.ctx.clone(),
                            version_mask: services.version_mask,
                            template_rx: services.template_rx.clone(),
                        });
                        tokio::spawn(handle_connection(
                            stream,
                            permit,
                            Arc::clone(&registry),
                            connection_mining,
                            authority,
                            shutdown.clone(),
                        ));
                    }
                    Err(accept_error) => error!("Stratum V2 accept failed: {accept_error}"),
                }
            }
            _ = shutdown.changed() => return,
        }
    }
}

/// Serve one connection until it closes, fails, or the listener stops.
///
/// `_permit` is the connection's place under the shared cap, released when
/// this returns. A Noise handshake that does not finish within
/// [`HANDSHAKE_TIMEOUT`] closes the connection, so an idle socket cannot
/// hold a permit before it has proven it speaks SV2.
async fn handle_connection<ShutdownSignal>(
    mut stream: TcpStream,
    _permit: OwnedSemaphorePermit,
    registry: Arc<Mutex<JobRegistry>>,
    mining: Option<Sv2Mining>,
    authority: Option<Sv2Authority>,
    mut shutdown: watch::Receiver<ShutdownSignal>,
) {
    let mut noise = match authority {
        Some(authority) => {
            match timeout(
                HANDSHAKE_TIMEOUT,
                NoiseTransport::accept(&mut stream, &authority),
            )
            .await
            {
                Ok(Ok(transport)) => Some(transport),
                Ok(Err(handshake_error)) => {
                    debug!("Stratum V2 handshake failed: {handshake_error}");
                    return;
                }
                Err(_) => {
                    debug!("Stratum V2 handshake timed out");
                    return;
                }
            }
        }
        None => None,
    };
    let mut connection = Sv2Connection::new(registry);
    let (mut template_rx, mut channels) = match mining {
        Some(services) => (
            Some(services.template_rx),
            Some(MiningChannels::new(services.ctx, services.version_mask)),
        ),
        None => (None, None),
    };
    let (mut reader, mut writer) = stream.into_split();
    let mut buffer = vec![0u8; 4096];
    'connection: loop {
        let frames = tokio::select! {
            read = reader.read(&mut buffer) => {
                let count = match read {
                    Ok(0) | Err(_) => break,
                    Ok(count) => count,
                };
                let messages = match noise.as_mut() {
                    Some(transport) => transport.decode(&buffer[..count]),
                    None => connection.decode(&buffer[..count]),
                };
                let handled = match messages {
                    Ok(messages) => {
                        handle_messages(
                            &mut connection,
                            channels.as_mut(),
                            template_rx.as_ref(),
                            messages,
                        )
                        .await
                    }
                    Err(decode_error) => Err(decode_error),
                };
                match handled {
                    Ok(frames) => frames,
                    Err(_) => break,
                }
            }
            prepared = next_template(template_rx.as_mut()) => {
                match (prepared, channels.as_mut()) {
                    (Some(prepared), Some(channels)) => channels.on_template(&prepared),
                    (None, Some(_)) => break,
                    _ => Vec::new(),
                }
            }
            _ = shutdown.changed() => break,
        };
        for frame in frames {
            let wire = match noise.as_mut() {
                Some(transport) => match transport.encrypt(frame) {
                    Ok(encrypted) => encrypted,
                    Err(_) => break 'connection,
                },
                None => frame,
            };
            if writer.write_all(&wire).await.is_err() {
                break 'connection;
            }
        }
    }
    if let Some(channels) = channels.as_mut() {
        channels.close_all().await;
    }
}

/// Answer a `RequestExtensions` (`0x0001`, `0x00`): grant the merge-mining
/// disclosure extension (`0x4D4D`) when asked for on a connection with
/// mining channels, and leave every other extension out of the answer. A
/// request before `SetupConnection`, or one that is not exactly one request,
/// closes the connection.
fn negotiate_extensions(
    connection: &Sv2Connection,
    channels: Option<&mut MiningChannels>,
    payload: &[u8],
) -> Result<Vec<Vec<u8>>, String> {
    if !connection.is_set_up() {
        return Err("extensions requested before SetupConnection".to_string());
    }
    let (request, asked) = requested_extensions(payload).ok_or("malformed RequestExtensions")?;
    let granted: Vec<u16> = match channels {
        Some(channels) if asked.contains(&MERGE_DISCLOSURE_EXTENSION) => {
            channels.grant_merge_disclosure();
            vec![MERGE_DISCLOSURE_EXTENSION]
        }
        _ => Vec::new(),
    };
    Ok(vec![encode_extension_frame(
        EXTENSIONS_NEGOTIATION,
        REQUEST_EXTENSIONS_SUCCESS,
        false,
        &extensions_payload(request, &granted),
    )?])
}

/// Answer each complete message.
///
/// Mining channel messages need a connection that set up the Mining Protocol
/// on a listener with mining enabled; anything else is an error that closes
/// the connection. The only extension message is the extensions
/// negotiation's request (`negotiate_extensions`); a message of another
/// extension closes the connection.
async fn handle_messages(
    connection: &mut Sv2Connection,
    mut channels: Option<&mut MiningChannels>,
    template_rx: Option<&TemplateReceiver>,
    messages: Vec<Sv2Message>,
) -> Result<Vec<Vec<u8>>, String> {
    let mut frames = Vec::with_capacity(messages.len());
    for message in messages {
        let Sv2Message {
            extension_type,
            message_type,
            mut payload,
        } = message;
        if extension_type == EXTENSIONS_NEGOTIATION && message_type == REQUEST_EXTENSIONS {
            frames.extend(negotiate_extensions(
                connection,
                channels.as_deref_mut(),
                &payload,
            )?);
            continue;
        }
        if extension_type != 0 {
            return Err(format!(
                "unsupported Stratum V2 extension {extension_type:#06x}"
            ));
        }
        let responses = if message_type == MESSAGE_TYPE_SET_CUSTOM_MINING_JOB {
            let request =
                SetCustomMiningJob::from_bytes(&mut payload).map_err(|error| error.to_string())?;
            if !connection.can_select_work() {
                custom_job_error(&request, "work-selection-not-negotiated")?
            } else if let Some(channels) = channels.as_mut() {
                match connection.take_declared_job(request.token.as_bytes()) {
                    Ok(declared) => {
                        channels
                            .install_custom_job(&request, declared, current_template(template_rx))
                            .await?
                    }
                    Err(_) => custom_job_error(&request, "invalid-mining-job-token")?,
                }
            } else {
                custom_job_error(&request, "mining-unavailable")?
            }
        } else if is_mining_channel_message(message_type) {
            let channels = match (channels.as_mut(), connection.is_mining()) {
                (Some(channels), true) => channels,
                (Some(_), false) => return Err("mining channel before Mining setup".to_string()),
                (None, _) => return Err("mining channels are not enabled".to_string()),
            };
            let current = current_template(template_rx);
            channels.handle(message_type, &mut payload, current).await?
        } else {
            connection.handle_message(message_type, &mut payload)?
        };
        frames.extend(responses);
    }
    Ok(frames)
}

/// The latest prepared template, for a new channel's first job.
///
/// Reads without marking the template seen: [`next_template`] must still
/// wake for it so every open channel gets the job. Marking it here dropped
/// a template that arrived while a share submission was being handled, and
/// the connection's channels kept mining the old one.
fn current_template(template_rx: Option<&TemplateReceiver>) -> Option<Arc<PreparedNotifyParams>> {
    template_rx.and_then(|receiver| receiver.borrow().clone())
}

/// Wait for the next prepared template. Never resolves without a template
/// watch, or once the notifier has dropped its sender.
async fn next_template(
    template_rx: Option<&mut TemplateReceiver>,
) -> Option<Arc<PreparedNotifyParams>> {
    match template_rx {
        Some(receiver) => match receiver.changed().await {
            Ok(()) => receiver.borrow_and_update().clone(),
            Err(_) => future::pending().await,
        },
        None => future::pending().await,
    }
}

#[cfg(test)]
mod tests;
