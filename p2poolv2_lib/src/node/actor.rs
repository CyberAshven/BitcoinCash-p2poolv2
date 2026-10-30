// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

#[cfg(test)]
#[mockall_double::double]
use crate::accounting::payout::sharechain_pplns::PplnsWindow;
#[cfg(not(test))]
use crate::accounting::payout::sharechain_pplns::PplnsWindow;
use crate::accounting::payout::sharechain_pplns::pplns_window::ChainDepths;
use crate::accounting::payout::simple_pplns::SimplePplnsShare;
use crate::accounting::stats::metrics::MetricsHandle;
use crate::command::Command;
use crate::config::{Chain, Config, PoolMode};
use crate::monitoring_events::MonitoringEventSender;
use crate::node::Node;
use crate::node::SwarmSend;
use crate::node::dns_seeds;
use crate::node::emission_worker::EmissionWorker;
use crate::node::found_block_verifier::FoundBlockVerifier;
use crate::node::messages::Message;
use crate::node::organise_worker::{OrganiseWorker, create_organise_channel};
use crate::node::p2p_health::P2pHealth;
use crate::node::p2p_message_handlers::receivers::block_receiver::{
    BlockReceiver, create_block_receiver_channel,
};
use crate::node::p2p_message_handlers::senders::build_getheaders_message;
use crate::node::p2p_message_handlers::senders::send_share_block_broadcast;
use crate::node::payout_tree_executor::{
    PayoutTreeExecutor, PayoutTreeSender, payout_tree_channel,
};
use crate::node::request_response_handler::block_fetcher::{
    BlockFetcher, create_block_fetcher_channel,
};
use crate::node::request_sender::RequestSender;
use crate::node::response_worker::{ResponseWorker, create_response_worker_channel};
use crate::node::validation_worker::{ValidationWorker, create_validation_channel};
#[cfg(test)]
#[mockall_double::double]
use crate::pool_difficulty::PoolDifficulty;
#[cfg(not(test))]
use crate::pool_difficulty::PoolDifficulty;
#[cfg(test)]
#[mockall_double::double]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
#[cfg(not(test))]
use crate::shares::chain::chain_store_handle::ChainStoreHandle;
use crate::shares::transaction_pool::{
    PoolRejection, RelayReceiver, RelayTransaction, ShareTransactionPool, SubmitOutcome,
    TransactionSource,
};
use crate::shares::validation::{DefaultShareValidator, ShareValidator};
use crate::stratum::emission::EmissionReceiver;
use crate::stratum::work::notify::NotifySender;
use bitcoin::BlockHash;
use bitcoindrpc::BitcoindRpcClient;
use libp2p::futures::StreamExt;
use libp2p::request_response::ResponseChannel;
use std::collections::HashMap;
use std::error::Error;
use std::ops::ControlFlow;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinSet;
use tracing::{debug, error, info};

/// NodeHandle provides an interface to interact with a Node running in a separate task
#[derive(Clone)]
#[allow(dead_code)]
pub struct NodeHandle {
    // The channel to send commands to the Node Actor
    command_tx: mpsc::Sender<Command>,
    /// Bitcoin Cash: the share transaction pool the node relays and mines
    /// from. `None` on Bitcoin.
    share_transaction_pool: Option<ShareTransactionPool>,
}

/// Why `NodeHandle::submit_share_transaction` did not take a transaction.
#[derive(Debug, thiserror::Error)]
pub enum ShareTransactionSubmitError {
    /// This node keeps no share transaction pool: it is on Bitcoin.
    #[error("this node does not take share transactions")]
    NoPool,
    /// The pool checked the transaction and refused it.
    #[error(transparent)]
    Rejected(#[from] PoolRejection),
}

#[allow(dead_code)]
impl NodeHandle {
    /// Create a new Node and return a handle to interact with it
    ///
    /// On Bitcoin Cash `share_transactions` is the share transaction pool and
    /// the receiver of the transactions it accepts, which the node relays to
    /// its peers; Bitcoin passes `None`.
    #[allow(clippy::too_many_arguments)] // wiring constructor: each parameter is a distinct collaborator, a params struct would only move the list
    pub async fn new(
        config: Config,
        chain_store_handle: ChainStoreHandle,
        emissions_rx: EmissionReceiver,
        metrics: MetricsHandle,
        monitoring_event_sender: MonitoringEventSender,
        notify_tx: NotifySender,
        pplns_window: Arc<RwLock<PplnsWindow>>,
        share_transactions: Option<(ShareTransactionPool, RelayReceiver)>,
    ) -> Result<(Self, oneshot::Receiver<()>), Box<dyn Error + Send + Sync>> {
        let (command_tx, command_rx) = mpsc::channel::<Command>(32);
        let (share_transaction_pool, relay_rx) = match share_transactions {
            Some((pool, relay_rx)) => (Some(pool), Some(relay_rx)),
            None => (None, None),
        };
        let (node_actor, stopping_rx) = NodeActor::new(
            config,
            chain_store_handle,
            command_rx,
            emissions_rx,
            metrics,
            monitoring_event_sender,
            notify_tx,
            pplns_window,
            share_transaction_pool.clone(),
            relay_rx,
        )
        .unwrap();

        tokio::spawn(async move {
            node_actor.run().await;
        });

        Ok((
            Self {
                command_tx,
                share_transaction_pool,
            },
            stopping_rx,
        ))
    }

    /// Hand a signed share transaction to this node's pool, which checks it
    /// against the confirmed share chain, relays it to peers and mines it.
    pub fn submit_share_transaction(
        &self,
        transaction: bitcoin::Transaction,
    ) -> Result<SubmitOutcome, ShareTransactionSubmitError> {
        let pool = self
            .share_transaction_pool
            .as_ref()
            .ok_or(ShareTransactionSubmitError::NoPool)?;
        Ok(pool.submit(transaction, TransactionSource::Local)?)
    }

    /// Get a list of connected peers
    pub async fn get_peers(&self) -> Result<Vec<libp2p::PeerId>, Box<dyn Error + Send + Sync>> {
        let (tx, rx) = oneshot::channel();
        self.command_tx.send(Command::GetPeers(tx)).await?;
        match rx.await {
            Ok(peers) => Ok(peers),
            Err(e) => Err(e.into()),
        }
    }

    /// Shutdown the node
    pub async fn shutdown(&self) -> Result<(), Box<dyn Error + Send + Sync>> {
        let (tx, rx) = oneshot::channel();
        self.command_tx.send(Command::Shutdown(tx)).await?;
        match rx.await {
            Ok(_) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Get PPLNS shares with filtering
    pub async fn get_pplns_shares(
        &self,
        query: crate::command::GetPplnsShareQuery,
    ) -> Vec<SimplePplnsShare> {
        let (tx, rx) = oneshot::channel();
        let _ = self
            .command_tx
            .send(Command::GetPplnsShares(query, tx))
            .await;
        rx.await.unwrap_or_default()
    }

    /// Get enriched info for all connected peers.
    pub async fn get_peer_infos(
        &self,
    ) -> Result<Vec<crate::node::connection_tracker::PeerInfoResponse>, Box<dyn Error + Send + Sync>>
    {
        let (tx, rx) = oneshot::channel();
        self.command_tx.send(Command::GetPeerInfos(tx)).await?;
        Ok(rx.await?)
    }

    /// Add an IP to the runtime blocklist.
    pub async fn block_ip(&self, ip: std::net::IpAddr) -> Result<(), Box<dyn Error + Send + Sync>> {
        let (tx, rx) = oneshot::channel();
        self.command_tx.send(Command::BlockIp(ip, tx)).await?;
        Ok(rx.await?)
    }

    /// Remove an IP from the runtime blocklist.
    pub async fn unblock_ip(
        &self,
        ip: std::net::IpAddr,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        let (tx, rx) = oneshot::channel();
        self.command_tx.send(Command::UnblockIp(ip, tx)).await?;
        Ok(rx.await?)
    }

    /// Snapshot of P2P health counters.
    pub async fn get_p2p_health(&self) -> Result<P2pHealth, Box<dyn Error + Send + Sync>> {
        let (tx, rx) = oneshot::channel();
        self.command_tx.send(Command::GetP2pHealth(tx)).await?;
        Ok(rx.await?)
    }

    /// List all blocked IPs.
    pub async fn get_blocked_ips(
        &self,
    ) -> Result<Vec<std::net::IpAddr>, Box<dyn Error + Send + Sync>> {
        let (tx, rx) = oneshot::channel();
        self.command_tx.send(Command::GetBlockedIps(tx)).await?;
        Ok(rx.await?)
    }
}

#[cfg(any(test, feature = "test-utils"))]
impl NodeHandle {
    /// Create a stub NodeHandle for tests that responds to commands with defaults.
    pub fn new_for_test() -> Self {
        let (command_tx, mut command_rx) = mpsc::channel::<Command>(32);
        tokio::spawn(async move {
            while let Some(command) = command_rx.recv().await {
                match command {
                    Command::GetPeers(reply) => {
                        let _ = reply.send(Vec::new());
                    }
                    Command::Shutdown(reply) => {
                        let _ = reply.send(());
                        return;
                    }
                    Command::GetPplnsShares(_, reply) => {
                        let _ = reply.send(Vec::new());
                    }
                    Command::SendToPeer(_, _, reply) => {
                        let _ = reply.send(Ok(()));
                    }
                    Command::GetPeerInfos(reply) => {
                        let _ = reply.send(Vec::new());
                    }
                    Command::BlockIp(_, reply) => {
                        let _ = reply.send(());
                    }
                    Command::UnblockIp(_, reply) => {
                        let _ = reply.send(());
                    }
                    Command::GetBlockedIps(reply) => {
                        let _ = reply.send(Vec::new());
                    }
                    Command::GetP2pHealth(reply) => {
                        let _ = reply.send(P2pHealth::default());
                    }
                }
            }
        });
        Self {
            command_tx,
            share_transaction_pool: None,
        }
    }

    /// A stub NodeHandle, as `new_for_test`, whose share transactions go to
    /// `share_transaction_pool`.
    pub fn new_for_test_with_share_transaction_pool(
        share_transaction_pool: ShareTransactionPool,
    ) -> Self {
        Self {
            share_transaction_pool: Some(share_transaction_pool),
            ..Self::new_for_test()
        }
    }

    /// Create a stub NodeHandle for tests pre-loaded with a set of generated peers.
    ///
    /// Returns the handle and the string representations of the generated peer IDs
    /// so callers can assert on expected values without depending on libp2p directly.
    pub fn new_for_test_with_peer_count(count: usize) -> (Self, Vec<String>) {
        use crate::node::connection_tracker::{ConnectionDirection, PeerInfoResponse};

        let peer_ids: Vec<libp2p::PeerId> = (0..count)
            .map(|_| {
                libp2p::identity::Keypair::generate_ed25519()
                    .public()
                    .to_peer_id()
            })
            .collect();
        let peer_id_strings: Vec<String> = peer_ids.iter().map(|id| id.to_string()).collect();
        let peer_infos: Vec<PeerInfoResponse> = peer_ids
            .iter()
            .map(|peer_id| PeerInfoResponse {
                peer_id: peer_id.to_string(),
                ip: Some("127.0.0.1".to_string()),
                address: "/ip4/127.0.0.1/tcp/46884".to_string(),
                direction: ConnectionDirection::Outbound,
                connected_secs: 0,
            })
            .collect();
        let (command_tx, mut command_rx) = mpsc::channel::<Command>(32);
        tokio::spawn(async move {
            while let Some(command) = command_rx.recv().await {
                match command {
                    Command::GetPeers(reply) => {
                        let _ = reply.send(peer_ids.clone());
                    }
                    Command::Shutdown(reply) => {
                        let _ = reply.send(());
                        return;
                    }
                    Command::GetPplnsShares(_, reply) => {
                        let _ = reply.send(Vec::new());
                    }
                    Command::SendToPeer(_, _, reply) => {
                        let _ = reply.send(Ok(()));
                    }
                    Command::GetPeerInfos(reply) => {
                        let _ = reply.send(peer_infos.clone());
                    }
                    Command::BlockIp(_, reply) => {
                        let _ = reply.send(());
                    }
                    Command::UnblockIp(_, reply) => {
                        let _ = reply.send(());
                    }
                    Command::GetBlockedIps(reply) => {
                        let _ = reply.send(Vec::new());
                    }
                    Command::GetP2pHealth(reply) => {
                        let _ = reply.send(P2pHealth::default());
                    }
                }
            }
        });
        (
            Self {
                command_tx,
                share_transaction_pool: None,
            },
            peer_id_strings,
        )
    }
}

#[cfg(test)]
use mockall::mock;

#[cfg(test)]
mock! {
    pub NodeHandle {
        pub async fn new(config: Config, chain_store_handle: ChainStoreHandle) -> Result<(Self, oneshot::Receiver<()>), Box<dyn Error>>;
        pub async fn get_peers(&self) -> Result<Vec<libp2p::PeerId>, Box<dyn Error>>;
        pub async fn shutdown(&self) -> Result<(), Box<dyn Error>>;
        pub async fn send_to_peer(&self, peer_id: libp2p::PeerId, message: Message) -> Result<(), Box<dyn Error>>;
    }

    // Provide a clone implementation for NodeHandle mock double
    impl Clone for NodeHandle {
        fn clone(&self) -> Self {
            Self { command_tx: self.command_tx.clone() }
        }
    }
}

/// Why the node actor loop stopped.
enum StopReason {
    /// A `Command::Shutdown`; acknowledged on its reply channel after teardown.
    Requested(oneshot::Sender<()>),
    /// The actor stopped on its own: a worker failed or a channel closed.
    Fatal,
}

/// NodeActor runs the Node in a separate task and handles all its events
struct NodeActor {
    node: Node,
    command_rx: mpsc::Receiver<Command>,
    /// Signalled only when the actor stops on its own (a fatal worker exit or a
    /// closed channel), telling the node binary to shut down with an error. A
    /// requested `Command::Shutdown` is acknowledged on its own reply channel.
    stopping_tx: oneshot::Sender<()>,
    chain_store_handle: ChainStoreHandle,
    #[allow(dead_code)]
    metrics: MetricsHandle,
    /// All node workers, supervised as one set. Each task maps its own result to
    /// `Result<(), String>` so they share a type; the id->name map recovers a
    /// worker's name for logging, including on a panic (which yields no value).
    workers: JoinSet<Result<(), String>>,
    worker_names: HashMap<tokio::task::Id, &'static str>,
    /// Confirmed tip seen at the last sync-retry tick (seeded when the loop
    /// starts). The retry is skipped while the confirmed tip keeps moving.
    last_retry_confirmed_tip: Option<BlockHash>,
    /// Bitcoin Cash: share transactions the pool accepted, to send to peers.
    /// `None` on Bitcoin, or once the pool is gone.
    share_transaction_relay: Option<RelayReceiver>,
}

/// Start the payout tree executor of a Bitcoin Cash share-chain node and
/// return the sink the validator hands found trees to (see
/// `node::payout_tree_executor`). `None` on Bitcoin and in Hydrapool mode,
/// which build no payout trees, and when no BCHN client can be made.
fn start_payout_tree_executor(
    config: &Config,
    chain_store_handle: &ChainStoreHandle,
) -> Option<PayoutTreeSender> {
    if config.stratum.chain != Chain::BitcoinCash || config.stratum.mode != PoolMode::P2poolv2 {
        return None;
    }
    let client = match BitcoindRpcClient::new(
        &config.bitcoinrpc.url,
        &config.bitcoinrpc.username,
        &config.bitcoinrpc.password,
    ) {
        Ok(client) => client,
        Err(error) => {
            error!("No payout tree executor: cannot make a BCHN client: {error}");
            return None;
        }
    };
    let (sink, found) = payout_tree_channel();
    let executor = PayoutTreeExecutor::new(client, chain_store_handle.clone(), found);
    tokio::spawn(executor.run());
    Some(sink)
}

/// The next share transaction to relay, or never when there is no relay.
async fn next_relay_transaction(relay: &mut Option<RelayReceiver>) -> Option<RelayTransaction> {
    match relay {
        Some(receiver) => receiver.recv().await,
        None => std::future::pending().await,
    }
}

impl NodeActor {
    #[allow(clippy::too_many_arguments)] // wiring constructor: each parameter is a distinct collaborator, a params struct would only move the list
    fn new(
        config: Config,
        chain_store_handle: ChainStoreHandle,
        command_rx: mpsc::Receiver<Command>,
        emissions_rx: EmissionReceiver,
        metrics: MetricsHandle,
        monitoring_event_sender: MonitoringEventSender,
        notify_tx: NotifySender,
        pplns_window: Arc<RwLock<PplnsWindow>>,
        share_transaction_pool: Option<ShareTransactionPool>,
        share_transaction_relay: Option<RelayReceiver>,
    ) -> Result<(Self, oneshot::Receiver<()>), Box<dyn Error>> {
        // Create organise channel
        let (organise_tx, organise_rx) = create_organise_channel();

        // Create validation channel
        let (validation_tx, validation_rx) = create_validation_channel();

        // Create block fetcher channel
        let (block_fetcher_tx, block_fetcher_rx) = create_block_fetcher_channel();

        // Create block receiver channel
        let (block_receiver_tx, block_receiver_rx) = create_block_receiver_channel();

        // Create response worker channel
        let (response_worker_tx, response_worker_rx) = create_response_worker_channel();

        // Clone handles for workers before moving them into Node::new
        let validation_tx_for_worker = validation_tx.clone();
        let validation_tx_for_emission = validation_tx.clone();
        let validation_tx_for_organise = validation_tx.clone();
        let validation_tx_for_response = validation_tx.clone();
        let block_fetcher_tx_for_receiver = block_fetcher_tx.clone();
        let block_fetcher_tx_for_response = block_fetcher_tx.clone();
        let block_receiver_tx_for_response = block_receiver_tx.clone();
        let chain = config.stratum.chain;
        let bitcoin_cash_network = config.stratum.bitcoin_cash_network;
        let target_ceiling = crate::shares::bch_genesis::target_ceiling(
            crate::shares::bch_genesis::genesis_for_stratum(&config.stratum)
                .map_err(|error| -> Box<dyn Error> { error })?
                .header
                .bits,
        );

        let pool_difficulty = PoolDifficulty::build(&chain_store_handle, chain)
            .map_err(|error| -> Box<dyn Error> { Box::new(error) })?;

        // The clone is redundant for the real PoolDifficulty, which is Copy, but
        // mockall_double swaps in the non-Copy MockPoolDifficulty under cfg(test)
        // and pool_difficulty is used again below.
        #[allow(clippy::clone_on_copy)]
        let mut validator = DefaultShareValidator::new(pool_difficulty.clone())
            .for_chain(chain)
            .with_target_ceiling(target_ceiling)
            .with_bitcoin_cash_network(bitcoin_cash_network);
        if let Some(sink) = start_payout_tree_executor(&config, &chain_store_handle) {
            validator = validator.with_payout_tree_sink(sink);
        }
        let share_validator: Arc<dyn ShareValidator + Send + Sync> = Arc::new(validator);

        // Accounting verification is separate from share validation and the
        // serial organisation path. A slow BCHN never holds up either one.
        let found_block_verifier = if chain == Chain::BitcoinCash {
            let client = BitcoindRpcClient::new(
                &config.bitcoinrpc.url,
                &config.bitcoinrpc.username,
                &config.bitcoinrpc.password,
            )?;
            Some(FoundBlockVerifier::new(
                client,
                chain_store_handle.clone(),
                metrics.clone(),
            ))
        } else {
            None
        };

        let node = Node::new(
            config,
            chain_store_handle.clone(),
            block_fetcher_tx,
            validation_tx,
            block_receiver_tx,
            monitoring_event_sender.clone(),
            share_validator.clone(),
            response_worker_tx,
            share_transaction_pool,
        )?;

        // Spawn organise worker
        let organise_worker = OrganiseWorker::new(
            organise_rx,
            validation_tx_for_organise,
            chain_store_handle.clone(),
            monitoring_event_sender,
            notify_tx,
            metrics.clone(),
            pplns_window.clone(),
            share_validator.clone(),
        )
        .with_depths(ChainDepths::for_chain(chain));
        let mut workers: JoinSet<Result<(), String>> = JoinSet::new();
        let mut worker_names: HashMap<tokio::task::Id, &'static str> = HashMap::with_capacity(5);

        if let Some(verifier) = found_block_verifier {
            let verifier_id = workers
                .spawn(async move {
                    verifier.run().await;
                    Ok(())
                })
                .id();
            worker_names.insert(verifier_id, "Found block verifier");
        }

        let organise_worker_id = workers
            .spawn(async move {
                organise_worker
                    .run()
                    .await
                    .map_err(|error| error.to_string())
            })
            .id();
        worker_names.insert(organise_worker_id, "Organise worker");

        // Spawn validation worker
        let validation_worker = ValidationWorker::new(
            validation_rx,
            chain_store_handle.clone(),
            organise_tx.clone(),
            node.swarm_tx.clone(),
            pool_difficulty,
        )
        .for_chain(chain)
        .with_target_ceiling(target_ceiling)
        .with_bitcoin_cash_network(bitcoin_cash_network);
        let validation_worker_id = workers
            .spawn(async move {
                validation_worker
                    .run()
                    .await
                    .map_err(|error| error.to_string())
            })
            .id();
        worker_names.insert(validation_worker_id, "Validation worker");

        // Spawn block fetcher
        let block_fetcher = BlockFetcher::new(block_fetcher_rx, node.swarm_tx.clone());
        let block_fetcher_id = workers
            .spawn(async move { block_fetcher.run().await.map_err(|error| error.to_string()) })
            .id();
        worker_names.insert(block_fetcher_id, "Block fetcher");

        // Spawn block receiver
        let block_receiver = BlockReceiver::new(
            block_receiver_rx,
            share_validator.clone(),
            chain_store_handle.clone(),
            block_fetcher_tx_for_receiver,
            validation_tx_for_worker.clone(),
        )
        .with_depths(ChainDepths::for_chain(chain));
        let block_receiver_id = workers
            .spawn(async move {
                block_receiver.run().await;
                Ok(())
            })
            .id();
        worker_names.insert(block_receiver_id, "Block receiver");

        // Spawn response worker
        let response_worker = ResponseWorker::new(
            response_worker_rx,
            chain_store_handle.clone(),
            node.swarm_tx.clone(),
            block_fetcher_tx_for_response,
            validation_tx_for_response,
            block_receiver_tx_for_response,
            share_validator.clone(),
        );
        let response_id = workers
            .spawn(async move {
                response_worker
                    .run()
                    .await
                    .map_err(|error| error.to_string())
            })
            .id();
        worker_names.insert(response_id, "Response worker");

        // Spawn emission worker - turns stratum shares into share blocks
        let emission_worker = EmissionWorker::new(
            emissions_rx,
            chain_store_handle.clone(),
            validation_tx_for_emission,
        );
        let emission_worker_id = workers
            .spawn(async move {
                emission_worker.run().await;
                Ok(())
            })
            .id();
        worker_names.insert(emission_worker_id, "Emission worker");

        let (stopping_tx, stopping_rx) = oneshot::channel();
        Ok((
            Self {
                node,
                command_rx,
                stopping_tx,
                chain_store_handle,
                metrics,
                workers,
                worker_names,
                last_retry_confirmed_tip: None,
                share_transaction_relay,
            },
            stopping_rx,
        ))
    }

    async fn run(mut self) {
        let mut reconnect_interval =
            tokio::time::interval(crate::node::peer_reconnector::PeerReconnector::check_interval());
        reconnect_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // Skip the immediate first tick so we don't reconnect while initial dials are in progress
        reconnect_interval.tick().await;

        const SYNC_RETRY_INTERVAL: u64 = 60;
        let mut sync_retry_interval =
            tokio::time::interval(std::time::Duration::from_secs(SYNC_RETRY_INTERVAL));
        sync_retry_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        sync_retry_interval.tick().await;
        self.last_retry_confirmed_tip = self.chain_store_handle.get_chain_tip().ok();

        const KADEMLIA_BOOTSTRAP_INTERVAL_SECS: u64 = 300;
        let mut kademlia_bootstrap_interval = tokio::time::interval(
            std::time::Duration::from_secs(KADEMLIA_BOOTSTRAP_INTERVAL_SECS),
        );
        kademlia_bootstrap_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        kademlia_bootstrap_interval.tick().await;

        let dns_seed_timer = tokio::time::sleep(dns_seeds::dns_seed_delay(
            self.node.peer_reconnector.has_peers(),
        ));
        tokio::pin!(dns_seed_timer);
        let mut dns_seed_pending = self.node.dns_seeds.is_some();

        let stop_reason = loop {
            tokio::select! {
                buf = self.node.swarm_rx.recv() => {
                    match buf {
                        Some(msg) => {
                            // The sync-retry interval lives in this scope, so the
                            // reset stays here rather than in the handler.
                            if matches!(msg, SwarmSend::Request(_, Message::GetShareHeaders(_, _))) {
                                sync_retry_interval.reset();
                            }
                            self.on_swarm_send(msg);
                        }
                        None => {
                            info!("Stopping node actor on swarm channel close");
                            break StopReason::Fatal;
                        }
                    }
                },
                event = self.node.swarm.select_next_some() => {
                    if let Err(e) = self.node.handle_swarm_event(event) {
                        error!("Error handling swarm event: {e}");
                    }
                },
                command = self.command_rx.recv() => {
                    match command {
                        Some(command) => {
                            if let ControlFlow::Break(reason) = self.on_command(command) {
                                break reason;
                            }
                        }
                        None => {
                            info!("Stopping node actor on channel close");
                            break StopReason::Fatal;
                        }
                    }
                },
                Some(joined) = self.workers.join_next_with_id() => {
                    if let ControlFlow::Break(reason) = self.on_worker_exit(joined) {
                        break reason;
                    }
                }
                _ = reconnect_interval.tick(), if self.node.peer_reconnector.has_peers() => {
                    self.node.attempt_reconnections();
                }
                _ = sync_retry_interval.tick(), if !self.chain_store_handle.is_current() => {
                    self.on_sync_retry_tick();
                }
                _ = kademlia_bootstrap_interval.tick() => {
                    self.node.attempt_kademlia_bootstrap();
                }
                () = &mut dns_seed_timer, if dns_seed_pending => {
                    dns_seed_pending = false;
                    self.node.query_dns_seeds_if_isolated();
                }
                relay = next_relay_transaction(&mut self.share_transaction_relay),
                    if self.share_transaction_relay.is_some() => {
                    match relay {
                        Some(relay) => self.on_relay_transaction(relay),
                        None => self.share_transaction_relay = None,
                    }
                }
            }
        };

        self.shut_down(stop_reason).await;
    }

    /// The single teardown path for every way the loop can end.
    ///
    /// Stops all workers and waits for them to finish before reporting, so the
    /// caller never sees the node as stopped while a worker is still running.
    /// Workers are aborted at their current await point; their durable effects
    /// go through the store writer's atomic batches, so nothing is left half
    /// written, and in-memory state is rebuilt on the next start.
    async fn shut_down(mut self, reason: StopReason) {
        self.workers.shutdown().await;
        match reason {
            StopReason::Requested(reply) => {
                if reply.send(()).is_err() {
                    error!("Failed to send Shutdown response - receiver dropped");
                }
            }
            StopReason::Fatal => {
                if self.stopping_tx.send(()).is_err() {
                    error!("Failed to send stopping signal - receiver dropped");
                }
            }
        }
    }

    /// Send a share transaction the pool accepted to every connected peer but
    /// the one it came from, as a `Transaction` request each answers with
    /// `Ack`. A synchronous enqueue per peer, like a block broadcast.
    fn on_relay_transaction(&mut self, relay: RelayTransaction) {
        let txid = relay.transaction.compute_txid();
        let peers = self.node.connected_peers();
        let mut relayed = 0;
        for peer in peers.iter().filter(|peer| Some(**peer) != relay.source) {
            self.node
                .swarm
                .behaviour_mut()
                .request_response
                .send_request(peer, Message::Transaction(relay.transaction.clone()));
            relayed += 1;
        }
        debug!("Relayed share transaction {txid} to {relayed} peers");
    }

    /// Execute one outbound swarm operation. All calls are synchronous swarm
    /// enqueues; the driver owns the swarm, so nothing here blocks.
    fn on_swarm_send(&mut self, msg: SwarmSend<ResponseChannel<Message>>) {
        match msg {
            SwarmSend::Request(peer_id, msg) => {
                let msg_type = msg.message_type();
                let request_id = self
                    .node
                    .swarm
                    .behaviour_mut()
                    .request_response
                    .send_request(&peer_id, msg);
                debug!("Sent {msg_type} to peer: {peer_id}, request_id: {request_id}");
            }
            SwarmSend::Response(response_channel, msg) => {
                let request_id = self
                    .node
                    .swarm
                    .behaviour_mut()
                    .request_response
                    .send_response(response_channel, msg);
                debug!("Sent message to response channel: {request_id:?}");
            }
            SwarmSend::BroadcastBlock(share_block) => {
                let connected_peers = self.node.connected_peers();
                let peer_knowledge = self
                    .node
                    .request_response_handler
                    .peer_block_knowledge_mut();
                send_share_block_broadcast(
                    share_block,
                    &connected_peers,
                    peer_knowledge,
                    &mut self.node.swarm,
                );
            }
            SwarmSend::Disconnect(peer_id) => {
                if self.node.swarm.disconnect_peer_id(peer_id).is_err() {
                    error!("Error disconnecting peer {peer_id}");
                } else {
                    debug!("Disconnected peer: {peer_id}");
                }
            }
            // Event sent from dns seed resolution
            SwarmSend::Dial(address) => match self.node.swarm.dial(address.clone()) {
                Ok(_) => info!("Dialed {address}"),
                Err(error) => debug!("Failed to dial {address}: {error}"),
            },
        }
    }

    /// Handle one control-plane command. Returns `Break` when the command stops
    /// the actor (`Shutdown`).
    fn on_command(&mut self, command: Command) -> ControlFlow<StopReason> {
        match command {
            Command::GetPeers(tx) => {
                let peers = self
                    .node
                    .swarm
                    .connected_peers()
                    .cloned()
                    .collect::<Vec<_>>();
                if tx.send(peers).is_err() {
                    error!("Failed to send GetPeers response - receiver dropped");
                }
            }
            Command::SendToPeer(peer_id, message, tx) => {
                match self.node.send_to_peer(&peer_id, message) {
                    Ok(_) => {
                        if tx.send(Ok(())).is_err() {
                            error!("Failed to send SendToPeer response - receiver dropped");
                        }
                    }
                    Err(e) => {
                        error!("Error sending message to peer: {e}");
                        if tx
                            .send(Err("Error sending message to peer".into()))
                            .is_err()
                        {
                            error!("Failed to send SendToPeer error response - receiver dropped");
                        }
                    }
                }
            }
            Command::Shutdown(reply) => {
                self.node.shutdown().unwrap();
                return ControlFlow::Break(StopReason::Requested(reply));
            }
            Command::GetPplnsShares(query, tx) => {
                debug!(
                    "Received GetPplnsShares command with limit: {}",
                    query.limit
                );
                let result = self.node.handle_get_pplns_shares(query);
                if tx.send(result).is_err() {
                    error!("Failed to send GetPplnsShares response - receiver dropped");
                }
            }
            Command::GetPeerInfos(tx) => {
                let infos = self.node.connection_tracker.get_peer_infos();
                if tx.send(infos).is_err() {
                    error!("Failed to send GetPeerInfos response - receiver dropped");
                }
            }
            Command::BlockIp(ip, tx) => {
                info!("Blocking IP {ip} via runtime command");
                self.node.connection_tracker.block_ip(ip);
                let _ = tx.send(());
            }
            Command::UnblockIp(ip, tx) => {
                info!("Unblocking IP {ip} via runtime command");
                self.node.connection_tracker.unblock_ip(ip);
                let _ = tx.send(());
            }
            Command::GetP2pHealth(tx) => {
                if tx.send(self.node.p2p_health()).is_err() {
                    error!("Failed to send GetP2pHealth response - receiver dropped");
                }
            }
            Command::GetBlockedIps(tx) => {
                let ips = self.node.connection_tracker.get_blocked_ips();
                if tx.send(ips).is_err() {
                    error!("Failed to send GetBlockedIps response - receiver dropped");
                }
            }
        }
        ControlFlow::Continue(())
    }

    /// Handle a worker task exiting. A clean exit is logged and the loop
    /// continues on the remaining workers; a fatal error or a panic signals the
    /// stop.
    fn on_worker_exit(
        &mut self,
        joined: Result<(tokio::task::Id, Result<(), String>), tokio::task::JoinError>,
    ) -> ControlFlow<StopReason> {
        match joined {
            Ok((id, Ok(()))) => {
                info!("{} stopped cleanly", self.worker_name(id));
                ControlFlow::Continue(())
            }
            Ok((id, Err(message))) => {
                error!("{} fatal error: {message}", self.worker_name(id));
                ControlFlow::Break(StopReason::Fatal)
            }
            Err(join_error) => {
                error!(
                    "{} panicked: {join_error}",
                    self.worker_name(join_error.id())
                );
                ControlFlow::Break(StopReason::Fatal)
            }
        }
    }

    /// The registered name of a worker task, for logging.
    fn worker_name(&self, id: tokio::task::Id) -> &'static str {
        self.worker_names.get(&id).copied().unwrap_or("Worker")
    }

    /// The chain tip is stale: ask a connected peer for more headers, unless the
    /// confirmed tip moved since the last tick. Builds the request from the store
    /// and sends it on the swarm directly (synchronous, so the loop never awaits a
    /// send on `swarm_tx`).
    fn on_sync_retry_tick(&mut self) {
        let current_tip = self.chain_store_handle.get_chain_tip().ok();
        let progressed = current_tip
            .is_some_and(|tip| confirmed_tip_progressed(self.last_retry_confirmed_tip, tip));
        if current_tip.is_some() {
            self.last_retry_confirmed_tip = current_tip;
        }
        if progressed {
            debug!("Confirmed tip advanced since last sync retry tick, skipping sync retry");
            return;
        }

        let peers = self.node.connected_peers();
        if !peers.is_empty() {
            let now_secs = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as usize;
            let peer_id = peers[now_secs % peers.len()];
            info!("Chain tip stale, sending getheaders to peer {peer_id}");
            match build_getheaders_message(&self.chain_store_handle, 0) {
                Ok(getheaders_request) => {
                    self.node.swarm.send_request(&peer_id, getheaders_request);
                }
                Err(error) => error!("Sync retry: failed to build getheaders: {error}"),
            }
        }
    }
}

/// True when the confirmed tip changed since `last_seen`. A reorg changes the tip
/// hash, so it counts as progress. Without a baseline (`None`) there is nothing
/// to compare against, so it is not progress and the sync retry still fires.
fn confirmed_tip_progressed(last_seen: Option<BlockHash>, current: BlockHash) -> bool {
    last_seen.is_some_and(|seen| seen != current)
}

#[cfg(test)]
mod tests;
