// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

pub mod block_fetcher;
pub mod peer_block_knowledge;

use self::block_fetcher::{BlockFetcherEvent, BlockFetcherHandle};
use self::peer_block_knowledge::PeerBlockKnowledge;
use crate::config::NetworkConfig;
use crate::node::SwarmSend;
use crate::node::behaviour::request_response::RequestResponseEvent;
use crate::node::messages::{InventoryMessage, Message};
use crate::node::p2p_message_handlers::receivers::block_receiver::BlockReceiverHandle;
use crate::node::request_sender::RequestSender;
use crate::node::response_worker::{ResponseWorkerEvent, ResponseWorkerSender};
use crate::node::validation_worker::ValidationSender;
use crate::service::PeerHandle;
use crate::service::p2p_service::RequestContext;
use crate::service::spawn_peer_service;
#[cfg(test)]
#[mockall_double::double]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
#[cfg(not(test))]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
use crate::shares::transaction_pool::ShareTransactionPool;
use crate::shares::validation::ShareValidator;
use crate::utils::time_provider::SystemTimeProvider;
use libp2p::PeerId;
use libp2p::request_response::ResponseChannel;
use libp2p::swarm::ConnectionId;
use std::collections::HashMap;
use std::error::Error;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, error, warn};

/// Handles request-response events from the libp2p network.
///
/// Generic over the channel type `C` to allow testing with substitute
/// types.  In production, `C` is `ResponseChannel<Message>` from
/// libp2p. In tests, `C` can be any `Send + Sync` type such as
/// `oneshot::Sender<Message>`.
///
/// We need to do this as ResponseChannel is an opaque type and we
/// can't write tests for modules that directly use these types.
///
/// Each connected peer gets a dedicated service task with its own
/// rate limiter. Inbound requests are forwarded to the peer's task
/// via a bounded channel. Responses are handled directly without
/// the service layers since they are solicited by us and do not
/// need peer-protection middleware.
pub struct RequestResponseHandler<C: Send + Sync> {
    peer_handles: HashMap<PeerId, PeerHandle<C, SystemTimeProvider>>,
    max_requests_per_second: u64,
    chain_store_handle: ChainStoreHandle,
    swarm_tx: mpsc::Sender<SwarmSend<C>>,
    block_fetcher_handle: BlockFetcherHandle,
    validation_tx: ValidationSender,
    block_receiver_handle: BlockReceiverHandle,
    peer_block_knowledge: PeerBlockKnowledge,
    share_validator: Arc<dyn ShareValidator + Send + Sync>,
    /// Inbound responses are handed to the response worker rather than processed
    /// on the swarm-driver task, keeping the node actor loop free for other events.
    response_worker_handle: ResponseWorkerSender,
    /// Requests sent by this node that failed, for P2P health metrics.
    outbound_failures_total: u64,
    /// Peer requests this node failed to answer, for P2P health metrics.
    inbound_failures_total: u64,
    /// Responses dropped on a full or closed worker queue, for P2P health metrics.
    responses_dropped_total: u64,
    /// Bitcoin Cash: the pool relayed share transactions go to. `None` on
    /// Bitcoin, which ignores them.
    share_transaction_pool: Option<ShareTransactionPool>,
}

/// Implementation of ResponseChannel<Message>, used in production.
/// The only part left out of tests is the type based dispatching. The
/// dispatch.* functions are tested for the generic implementation.
impl RequestResponseHandler<ResponseChannel<Message>> {
    /// Create a new RequestResponseHandler with per-peer service support.
    #[allow(clippy::too_many_arguments)] // wiring constructor: each parameter is a distinct collaborator, a params struct would only move the list
    pub fn new(
        network_config: NetworkConfig,
        chain_store_handle: ChainStoreHandle,
        swarm_tx: mpsc::Sender<SwarmSend<ResponseChannel<Message>>>,
        block_fetcher_handle: BlockFetcherHandle,
        validation_tx: ValidationSender,
        block_receiver_handle: BlockReceiverHandle,
        share_validator: Arc<dyn ShareValidator + Send + Sync>,
        response_worker_handle: ResponseWorkerSender,
        share_transaction_pool: Option<ShareTransactionPool>,
    ) -> Self {
        Self {
            peer_handles: HashMap::new(),
            max_requests_per_second: network_config.max_requests_per_second,
            chain_store_handle,
            swarm_tx,
            block_fetcher_handle,
            validation_tx,
            block_receiver_handle,
            peer_block_knowledge: PeerBlockKnowledge::default(),
            share_validator,
            response_worker_handle,
            outbound_failures_total: 0,
            inbound_failures_total: 0,
            responses_dropped_total: 0,
            share_transaction_pool,
        }
    }

    /// Handle a request-response event from the libp2p network.
    ///
    /// Inbound requests are dispatched through the Tower service
    /// stack (rate limiting, inactivity tracking). If the service is
    /// not ready within 1 second, the peer is disconnected.
    ///
    /// Inbound responses are dispatched directly to handle_response
    /// without the service layers.
    pub fn handle_event(
        &mut self,
        event: RequestResponseEvent,
        request_sender: &mut impl RequestSender,
    ) -> Result<(), Box<dyn Error>> {
        match event {
            RequestResponseEvent::Message {
                peer,
                connection_id,
                message:
                    libp2p::request_response::Message::Request {
                        request_id: _,
                        request,
                        channel,
                    },
            } => self.dispatch_request(peer, connection_id, request, channel, request_sender),
            RequestResponseEvent::Message {
                peer,
                connection_id,
                message:
                    libp2p::request_response::Message::Response {
                        request_id,
                        response,
                    },
            } => {
                debug!(
                    "Received response {} for request {} from peer {} on connection {}",
                    response, request_id, peer, connection_id
                );
                self.dispatch_response(peer, connection_id, response)
            }
            RequestResponseEvent::OutboundFailure {
                peer,
                connection_id,
                request_id,
                error: failure_error,
            } => {
                self.outbound_failures_total += 1;
                warn!(
                    "Outbound failure from peer {} on connection {}, request_id: {}, error: {:?}",
                    peer, connection_id, request_id, failure_error
                );
                Ok(())
            }
            RequestResponseEvent::InboundFailure {
                peer,
                connection_id,
                request_id,
                error: failure_error,
            } => {
                self.inbound_failures_total += 1;
                warn!(
                    "Inbound failure from peer {} on connection {}, request_id: {}, error: {:?}",
                    peer, connection_id, request_id, failure_error
                );
                Ok(())
            }
            RequestResponseEvent::ResponseSent {
                peer,
                connection_id,
                request_id,
            } => {
                debug!(
                    "Response sent to peer {} on connection {}, request_id: {}",
                    peer, connection_id, request_id
                );
                Ok(())
            }
        }
    }
}

/// Generic implementation. The dispatch.* functions can be tested as
/// here we don't depend on the the tokio opaque types.
impl<C: Send + Sync + 'static> RequestResponseHandler<C> {
    /// Returns a reference to the peer block knowledge tracker.
    pub fn peer_block_knowledge(&self) -> &PeerBlockKnowledge {
        &self.peer_block_knowledge
    }

    /// Returns a mutable reference to the peer block knowledge tracker.
    ///
    /// Used by the actor to record outbound block broadcasts so that
    /// subsequent broadcast attempts for the same block are suppressed.
    pub fn peer_block_knowledge_mut(&mut self) -> &mut PeerBlockKnowledge {
        &mut self.peer_block_knowledge
    }

    /// Spawn a per-peer service task for a newly connected peer.
    ///
    /// If a handle already exists for this peer (e.g. duplicate
    /// ConnectionEstablished), the old one is replaced and its task
    /// will exit when the dropped sender closes the channel.
    pub fn add_peer(&mut self, peer_id: PeerId) {
        let handle =
            spawn_peer_service(peer_id, self.max_requests_per_second, self.swarm_tx.clone());
        self.peer_handles.insert(peer_id, handle);
    }

    /// Requests sent by this node that failed, since start.
    pub fn outbound_failures_total(&self) -> u64 {
        self.outbound_failures_total
    }

    /// Peer requests this node failed to answer, since start.
    pub fn inbound_failures_total(&self) -> u64 {
        self.inbound_failures_total
    }

    /// Responses dropped on a full or closed worker queue, since start.
    pub fn responses_dropped_total(&self) -> u64 {
        self.responses_dropped_total
    }

    /// Responses waiting in the response worker queue.
    pub fn response_queue_depth(&self) -> u64 {
        (self.response_worker_handle.max_capacity() - self.response_worker_handle.capacity()) as u64
    }

    /// Whether the peer has a request service.
    #[cfg(test)]
    pub(crate) fn has_peer(&self, peer_id: &PeerId) -> bool {
        self.peer_handles.contains_key(peer_id)
    }

    /// Remove all state for a disconnected peer.
    ///
    /// Drops the peer handle, which closes the channel and causes
    /// the peer's service task to exit. Also removes peer block
    /// knowledge and notifies the block fetcher so it stops sending
    /// requests to this peer.
    pub fn remove_peer(&mut self, peer_id: &PeerId) {
        self.peer_handles.remove(peer_id);
        self.peer_block_knowledge.remove_peer(peer_id);
        // try_send, not an awaited send: this runs on the node's event loop, so
        // it must not block. A full or closed fetcher channel is tolerable --
        // its in-flight requests to the peer time out and are retried, and the
        // PeersUpdated snapshot re-syncs the selector.
        if let Err(send_error) = self
            .block_fetcher_handle
            .try_send(BlockFetcherEvent::PeerRemoved(*peer_id))
        {
            warn!("Failed to notify block fetcher of peer removal for {peer_id}: {send_error}");
        }
    }

    /// Records which blocks a peer knows about based on a message.
    ///
    /// Called before dispatching both requests and responses so that
    /// subsequent inv sends can avoid redundant announcements.
    fn record_peer_knowledge(&mut self, peer: &PeerId, message: &Message) {
        match message {
            Message::Inventory(InventoryMessage::BlockHashes(hashes)) => {
                for hash in hashes {
                    self.peer_block_knowledge.record_block_known(peer, *hash);
                }
            }
            Message::ShareBlock(block) => {
                self.peer_block_knowledge
                    .record_block_known(peer, block.block_hash());
            }
            _ => {}
        }
    }

    /// Dispatch an inbound request to the peer's service task.
    ///
    /// Records peer block knowledge, then forwards the request
    /// context to the peer's channel via try_send.
    ///
    /// `connection_id` identifies the libp2p connection the request arrived on
    /// and is logged with each failure, so a peer with several concurrent
    /// connections can be told apart in the logs.
    ///
    /// - Full: peer is overwhelming us, disconnect.
    /// - Closed: task exited (rate limit or error), remove the stale
    ///   handle so the next request spawns a fresh one.
    /// - No handle: create one on the fly (defensive fallback).
    fn dispatch_request(
        &mut self,
        peer: PeerId,
        connection_id: ConnectionId,
        request: Message,
        channel: C,
        request_sender: &mut impl RequestSender,
    ) -> Result<(), Box<dyn Error>> {
        self.record_peer_knowledge(&peer, &request);

        let ctx = RequestContext::<C, _> {
            peer,
            request,
            chain_store_handle: self.chain_store_handle.clone(),
            response_channel: channel,
            swarm_tx: self.swarm_tx.clone(),
            time_provider: SystemTimeProvider,
            block_fetcher_handle: self.block_fetcher_handle.clone(),
            validation_tx: self.validation_tx.clone(),
            block_receiver_handle: self.block_receiver_handle.clone(),
            share_validator: self.share_validator.clone(),
            share_transaction_pool: self.share_transaction_pool.clone(),
        };

        let peer_handle = match self.peer_handles.get(&peer) {
            Some(handle) => handle,
            None => {
                warn!(
                    "No service handle for peer {} on connection {}, creating one on the fly",
                    peer, connection_id
                );
                self.add_peer(peer);
                self.peer_handles.get(&peer).unwrap()
            }
        };

        match peer_handle.try_send(ctx) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                error!(
                    "Peer {} service channel full on connection {}, disconnecting",
                    peer, connection_id
                );
                request_sender.disconnect_peer(peer);
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                warn!(
                    "Peer {} service task exited on connection {}, removing stale handle",
                    peer, connection_id
                );
                self.peer_handles.remove(&peer);
            }
        }

        Ok(())
    }

    /// Dispatch a response by calling handle_response directly.
    ///
    /// Records peer block knowledge before processing. Responses bypass
    /// the Tower service layers (rate limiting, inactivity tracking)
    /// because they are solicited by us and libp2p only delivers them
    /// for matching outstanding requests.
    ///
    /// `connection_id` identifies the libp2p connection the response arrived
    /// on and is logged on the error path.
    fn dispatch_response(
        &mut self,
        peer: PeerId,
        connection_id: ConnectionId,
        response: Message,
    ) -> Result<(), Box<dyn Error>> {
        self.record_peer_knowledge(&peer, &response);

        // Hand the response to the response worker rather than processing it
        // here: handle_response can be heavy (a ShareHeaders batch runs many
        // organise_header calls) and this runs on the swarm-driver task. A full
        // or closed channel drops the response; every response kind has a retry
        // path (header sync re-requests, the fetcher's request timeout).
        let message_type = response.message_type();
        match self
            .response_worker_handle
            .try_send(ResponseWorkerEvent { peer, response })
        {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.responses_dropped_total += 1;
                warn!(
                    "Response worker channel full on connection {}, dropping {} from peer {}",
                    connection_id, message_type, peer
                );
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                self.responses_dropped_total += 1;
                warn!(
                    "Response worker channel closed on connection {}, dropping {} from peer {}",
                    connection_id, message_type, peer
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
