// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::api::dashboard;
use crate::api::endpoints;
use crate::api::endpoints::miner::MinerWindowSource;
use crate::api::error::ApiError;
use crate::api::{auth::auth_middleware, websocket::websocket_handler};
use axum::{
    Extension, Json, Router,
    extract::{FromRef, Query, Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{Redirect, Response},
    routing::{delete, get, post},
};
use chrono::DateTime;
use p2poolv2_lib::accounting::payout::sharechain_pplns::PplnsWindow;
use p2poolv2_lib::address::bch_share::BchShareNetwork;
use p2poolv2_lib::monitoring_events::MonitoringEventSender;
use p2poolv2_lib::node::actor::NodeHandle;
use p2poolv2_lib::stratum::work::tracker::{JobTracker, parse_coinbase};
use p2poolv2_lib::{
    accounting::{payout::simple_pplns::SimplePplnsShare, stats::metrics::MetricsHandle},
    config::ApiConfig,
    shares::chain::chain_store_handle::ChainStoreHandle,
    store::found_block::FoundBlock,
};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::RwLock;
use std::time::Duration;
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::oneshot;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tracing::{info, trace, warn};

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) app_config: AppConfig,
    pub(crate) chain_store_handle: ChainStoreHandle,
    pub(crate) metrics_handle: MetricsHandle,
    pub(crate) tracker_handle: Arc<JobTracker>,
    pub(crate) node_handle: NodeHandle,
    pub(crate) monitoring_event_sender: MonitoringEventSender,
    pub(crate) auth_user: Option<String>,
    pub(crate) auth_token: Option<String>,
    /// The node's share chain PPLNS window, shared with the payout and
    /// validation code, with the snapshot `/miner` answers from. `None` when
    /// the node does not pay from it (Hydrapool mode), which turns `/miner`
    /// off.
    pub(crate) pplns_window: Option<Arc<MinerWindowSource>>,
}

/// Stores application config values that don't change across requests
/// Used with Extension axum framework
#[derive(Clone)]
pub struct AppConfig {
    pub pool_signature_length: usize,
    pub network: bitcoin::Network,
    /// Bitcoin Cash share prefix. Independent of `network`, which stays the
    /// Bitcoin share-chain network used for taproot addresses and genesis.
    pub cash_network: Option<BchShareNetwork>,
    pub cors_allowed: bool,
}

/// Get AppConfig from AppState ref
impl FromRef<AppState> for AppConfig {
    fn from_ref(state: &AppState) -> Self {
        state.app_config.clone()
    }
}

/// What the API serves browsers besides the JSON routes.
#[derive(Clone, Debug, Default)]
pub(crate) struct WebOptions {
    /// Serve the dashboard, the `web_root` frontend and the read-only share
    /// chain routes they read without a login. The node's own routes keep
    /// their authentication.
    pub(crate) public: bool,
    /// A folder holding a self-hosted frontend's build, served for every
    /// path no API route takes. Paths it lacks get its `index.html`, so a
    /// frontend's own routing works.
    pub(crate) web_root: Option<PathBuf>,
}

#[derive(Deserialize)]
pub struct PplnsQuery {
    limit: Option<u32>,
    start_time: Option<String>,
    end_time: Option<String>,
}

/// Build the axum router with REST routes (protected by auth middleware)
/// and the WebSocket route (which handles its own auth via query param).
///
/// The dashboard gets an empty history no sampler fills; the API server
/// uses [`build_router_with_history`] with a sampled one.
#[cfg_attr(not(test), allow(dead_code))]
fn build_router(app_state: Arc<AppState>, app_config: AppConfig) -> Router {
    build_router_with_history(
        app_state,
        app_config,
        Arc::new(dashboard::history::HashrateHistory::default()),
        WebOptions::default(),
    )
}

/// Add a Basic challenge to a 401, so a browser opening a protected page
/// such as the dashboard shows its own login prompt.
async fn add_basic_challenge(mut response: Response) -> Response {
    if response.status() == StatusCode::UNAUTHORIZED
        && !response.headers().contains_key(header::WWW_AUTHENTICATE)
    {
        response.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Basic realm=\"P2Pool\", charset=\"UTF-8\""),
        );
    }
    response
}

/// [`build_router`] with the dashboard reading `history` and the browser
/// side set by `web`.
pub(crate) fn build_router_with_history(
    app_state: Arc<AppState>,
    app_config: AppConfig,
    history: Arc<dashboard::history::HashrateHistory>,
    web: WebOptions,
) -> Router {
    async fn log_req(req: Request, next: Next) -> Result<Response, StatusCode> {
        let method = req.method().clone();
        let uri = req.uri().clone();

        // GET: /test
        trace!("{}: {}", method, uri);
        let dur = std::time::Instant::now();

        let res = next.run(req).await;

        // GET: /test 200 1ms
        trace!(
            "{}: {} {} {}ms",
            method,
            uri,
            res.status(),
            dur.elapsed().as_millis()
        );

        Ok(res)
    }

    // Share chain data and the pages built on it: what a public node opens.
    let share_chain_routes = Router::new()
        .route("/health", get(health_check))
        .route("/pplns_shares", get(pplns_shares))
        .route("/chain_info", get(endpoints::chain_info::chain_info))
        .route("/shares", get(endpoints::shares::shares))
        .route("/candidates", get(endpoints::candidates::candidates))
        .route("/share", get(endpoints::share::share))
        .route("/dag", get(endpoints::dag::dag))
        .route("/miner", get(endpoints::miner::miner))
        .route("/transaction", get(endpoints::transaction::transaction))
        .route(
            "/share_headers",
            get(endpoints::share_headers::share_headers),
        )
        .route("/found_blocks", get(endpoints::found_blocks::found_blocks))
        .route("/miners", get(endpoints::miners::miners))
        .route("/network", get(endpoints::network::network))
        .merge(endpoints::hashrate::router(
            app_state.clone(),
            history.clone(),
        ))
        .merge(dashboard::router(app_state.clone(), history));
    let share_chain_routes = match web.web_root {
        Some(web_root) => {
            let index = ServeFile::new(web_root.join("index.html"));
            share_chain_routes.fallback_service(ServeDir::new(web_root).fallback(index))
        }
        None => share_chain_routes.route("/", get(|| async { Redirect::to("/dashboard") })),
    };

    // The node's own state and controls, always behind the login.
    let mut authenticated_routes = Router::new()
        .route(
            "/jd_prepare",
            post(endpoints::jd_prepare::prepare).layer(axum::extract::DefaultBodyLimit::max(8192)),
        )
        .route(
            "/share_transaction",
            post(endpoints::share_transaction::submit).layer(axum::extract::DefaultBodyLimit::max(
                endpoints::share_transaction::MAX_SHARE_TRANSACTION_REQUEST_BYTES,
            )),
        )
        .route("/metrics", get(metrics))
        .route("/peers", get(peers))
        .route("/blocked_ips", get(get_blocked_ips))
        .route("/blocked_ips", post(block_ip))
        .route("/blocked_ips", delete(unblock_ip));

    // Routes that handle their own auth: the WebSocket validates credentials
    // via query param in its handler. A public node's share chain routes
    // join them.
    let mut unauthenticated_routes = Router::new().route("/ws", get(websocket_handler));
    if web.public {
        unauthenticated_routes = unauthenticated_routes.merge(share_chain_routes);
    } else {
        authenticated_routes = authenticated_routes.merge(share_chain_routes);
    }
    let authenticated_routes = authenticated_routes
        .layer(middleware::from_fn_with_state(
            app_state.clone(),
            auth_middleware,
        ))
        .layer(middleware::map_response(add_basic_challenge));

    let mut router = authenticated_routes
        .merge(unauthenticated_routes)
        .layer(middleware::from_fn(log_req));

    if app_config.cors_allowed {
        router = router.layer(CorsLayer::permissive())
    }

    router.layer(Extension(app_config)).with_state(app_state)
}

/// Start the API server and return a shutdown channel and the actual bound port.
///
/// `pplns_window` is the node's share chain PPLNS window, or `None` when the
/// node does not pay from it; `/miner` reads it.
#[allow(clippy::too_many_arguments)] // wiring constructor: each parameter is a distinct collaborator, a params struct would only move the list
pub async fn start_api_server(
    config: ApiConfig,
    chain_store_handle: ChainStoreHandle,
    metrics_handle: MetricsHandle,
    tracker_handle: Arc<JobTracker>,
    node_handle: NodeHandle,
    monitoring_event_sender: MonitoringEventSender,
    network: bitcoin::Network,
    cash_network: Option<BchShareNetwork>,
    pool_signature: Option<String>,
    pplns_window: Option<Arc<RwLock<PplnsWindow>>>,
) -> Result<(oneshot::Sender<()>, u16), std::io::Error> {
    let app_config = AppConfig {
        pool_signature_length: pool_signature.unwrap_or_default().len(),
        network,
        cash_network,
        cors_allowed: config.cors_allowed,
    };

    let app_state = Arc::new(AppState {
        app_config: app_config.clone(),
        chain_store_handle,
        metrics_handle,
        tracker_handle,
        node_handle,
        monitoring_event_sender,
        auth_user: config.auth_user.clone(),
        auth_token: config.auth_token.clone(),
        pplns_window: pplns_window.map(|window| Arc::new(MinerWindowSource::new(window))),
    });

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let addr = SocketAddr::new(
        std::net::IpAddr::V4(config.hostname.parse().unwrap()),
        config.port,
    );
    // The dashboard's history, filled once a minute. The sampler holds it
    // weakly and is aborted when the server stops, so neither outlives it.
    let history = Arc::new(dashboard::history::HashrateHistory::default());
    let sampler = tokio::spawn(dashboard::history::run_sampler(
        app_state.clone(),
        Arc::downgrade(&history),
    ));
    let web = WebOptions {
        public: config.public_dashboard,
        web_root: config.web_root.as_ref().map(PathBuf::from),
    };
    if let Some(web_root) = &web.web_root {
        if web_root.join("index.html").is_file() {
            info!("Serving the frontend in {}", web_root.display());
        } else {
            warn!(
                "web_root {} has no index.html; paths outside the API will be 404",
                web_root.display()
            );
        }
    }
    let app = build_router_with_history(app_state, app_config, history, web);

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(e) => return Err(e),
    };

    let actual_port = listener.local_addr()?.port();
    info!(
        "API server listening on {}:{}",
        config.hostname, actual_port
    );

    tokio::spawn(async move {
        let result = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.await;
                info!("API server shutdown signal received");
            })
            .await
            .map_err(|e| ApiError::ServerError(e.to_string()));
        sampler.abort();
        result?;

        info!("API server stopped");
        Ok::<(), ApiError>(())
    });
    Ok((shutdown_tx, actual_port))
}

async fn health_check() -> String {
    "OK".into()
}

/// Returns pool metrics in grafana exposition format
///
/// The exposition also includes parsed coinbase outputs for showing
/// the current coinbase payout distribution
/// How long `/metrics` waits for the node's P2P health snapshot. The node
/// answers from its event loop, which never blocks, so this only trips if the
/// node has stopped; the scrape then omits the P2P series rather than hang.
const P2P_HEALTH_TIMEOUT: Duration = Duration::from_secs(1);

async fn metrics(State(state): State<Arc<AppState>>) -> String {
    //  Get base metrics
    let pool_metrics = state.metrics_handle.get_metrics().await;
    let mut exposition = pool_metrics.get_exposition();

    if let Some(coinbase_distribution) = parse_coinbase::get_distribution(
        &state.tracker_handle,
        state.app_config.pool_signature_length,
        state.app_config.network,
    ) {
        exposition.push_str("# HELP coinbase_rewards_distribution Current coinbase rewards distribution between users\n");
        exposition.push_str(&coinbase_distribution);
    }

    if let Some(network_difficulty) = state.tracker_handle.get_network_difficulty() {
        exposition.push_str("# HELP network_difficulty Current bitcoin network difficulty\n");
        exposition.push_str("# TYPE network_difficulty gauge\n");
        exposition.push_str(&format!("network_difficulty {network_difficulty}\n"));
    }

    // Cumulative confirmed sharechain work at the tip. Read live from the store
    // so it is reorg- and restart-safe (it always reflects the canonical
    // confirmed chain). Pool hashrate on Grafana is rate(sharechain_work_total),
    // since chain work is measured in expected hashes.
    //
    // Only emitted while the chain is current. During sync the confirmed tip
    // advances by the whole backlog in a short wall-clock window, which would
    // make rate() report replay speed as an inflated hashrate. Suppressing the
    // sample during sync leaves a gap instead; Prometheus staleness then keeps
    // rate() from bridging the sync jump when work resumes.
    if state.chain_store_handle.is_current()
        && let Ok(total_work) = state.chain_store_handle.get_total_work()
    {
        exposition.push_str(
                "# HELP sharechain_work_total Cumulative confirmed sharechain work in expected hashes; pool hashrate is rate() of this\n",
            );
        exposition.push_str("# TYPE sharechain_work_total counter\n");
        exposition.push_str(&format!(
            "sharechain_work_total {}\n",
            work_to_f64(total_work)
        ));
    }

    // Found bitcoin blocks are read live from the store, where they are
    // recorded on confirmation. Not gated on is_current: the list is derived
    // from the share chain and timestamped with bitcoin header times, so it is
    // correct during sync and the same on every node.
    match state.chain_store_handle.get_found_blocks() {
        Ok(found_blocks) => exposition.push_str(&found_blocks_exposition(
            &found_blocks,
            state.app_config.network,
        )),
        Err(error) => warn!("Failed to read found blocks for /metrics: {error}"),
    }

    match tokio::time::timeout(P2P_HEALTH_TIMEOUT, state.node_handle.get_p2p_health()).await {
        Ok(Ok(p2p_health)) => exposition.push_str(&p2p_health.exposition()),
        Ok(Err(error)) => warn!("Failed to read P2P health for /metrics: {error}"),
        Err(_) => warn!("Timed out reading P2P health for /metrics"),
    }

    exposition
}

/// Exposition for the bitcoin blocks found by the pool.
///
/// Emits a counter of all found blocks and one gauge line per block, set to
/// its bitcoin header time and labeled with blockhash, height, miner and the
/// bitcoin network so Grafana can list the blocks and link each to the block
/// explorer for that network. The network label uses bitcoin core's names
/// (main, testnet4, signet, regtest). Entries are never deleted from the
/// store, so the counter is monotonic.
fn found_blocks_exposition(found_blocks: &[FoundBlock], network: bitcoin::Network) -> String {
    let mut output = String::with_capacity(256 + found_blocks.len() * 256);
    output.push_str(
        "# HELP bitcoin_blocks_found_total Total number of bitcoin blocks found by the pool\n",
    );
    output.push_str("# TYPE bitcoin_blocks_found_total counter\n");
    output.push_str(&format!(
        "bitcoin_blocks_found_total {}\n",
        found_blocks.len()
    ));
    output.push_str(
        "# HELP bitcoin_block_found_time_seconds Bitcoin header time of a block found by the pool, labeled with blockhash, height, miner and network\n",
    );
    output.push_str("# TYPE bitcoin_block_found_time_seconds gauge\n");
    let network_name = network.to_core_arg();
    for found_block in found_blocks {
        output.push_str(&format!(
            "bitcoin_block_found_time_seconds{{blockhash=\"{}\",height=\"{}\",miner=\"{}\",network=\"{}\"}} {}\n",
            found_block.bitcoin_blockhash,
            found_block.bitcoin_height,
            found_block.miner_bitcoin_address,
            network_name,
            found_block.bitcoin_time
        ));
    }
    output
}

/// Convert 256-bit chain work to an f64 for Prometheus exposition. Lossy in the
/// low bits but monotonic, which is all rate() needs for a hashrate.
fn work_to_f64(work: bitcoin::Work) -> f64 {
    work.to_le_bytes()
        .iter()
        .rev()
        .fold(0.0_f64, |acc, &byte| acc * 256.0 + byte as f64)
}

/// Response type for the /peers endpoint.
///
use p2poolv2_lib::node::connection_tracker::PeerInfoResponse;

/// Returns enriched info for all connected peers including IP, direction,
/// and connection duration.
async fn peers(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<PeerInfoResponse>>, ApiError> {
    let peer_infos = state
        .node_handle
        .get_peer_infos()
        .await
        .map_err(|error| ApiError::ServerError(format!("Failed to get peer infos: {error}")))?;
    Ok(Json(peer_infos))
}

/// Request body for block/unblock IP endpoints.
#[derive(Deserialize)]
struct BlockIpRequest {
    ip: String,
}

/// List all blocked IPs.
async fn get_blocked_ips(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<String>>, ApiError> {
    let ips =
        state.node_handle.get_blocked_ips().await.map_err(|error| {
            ApiError::ServerError(format!("Failed to get blocked IPs: {error}"))
        })?;
    let ip_strings: Vec<String> = ips.iter().map(|ip| ip.to_string()).collect();
    Ok(Json(ip_strings))
}

/// Add an IP to the runtime blocklist.
async fn block_ip(
    State(state): State<Arc<AppState>>,
    Json(body): Json<BlockIpRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let ip: std::net::IpAddr = body
        .ip
        .parse()
        .map_err(|error| ApiError::BadRequest(format!("Invalid IP address: {error}")))?;
    state
        .node_handle
        .block_ip(ip)
        .await
        .map_err(|error| ApiError::ServerError(format!("Failed to block IP: {error}")))?;
    Ok(Json(serde_json::json!({"blocked": body.ip})))
}

/// Remove an IP from the runtime blocklist.
async fn unblock_ip(
    State(state): State<Arc<AppState>>,
    Json(body): Json<BlockIpRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let ip: std::net::IpAddr = body
        .ip
        .parse()
        .map_err(|error| ApiError::BadRequest(format!("Invalid IP address: {error}")))?;
    state
        .node_handle
        .unblock_ip(ip)
        .await
        .map_err(|error| ApiError::ServerError(format!("Failed to unblock IP: {error}")))?;
    Ok(Json(serde_json::json!({"unblocked": body.ip})))
}

/// Returns PPLNS shares with optional time filtering and limit.
async fn pplns_shares(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PplnsQuery>,
) -> Result<Json<Vec<SimplePplnsShare>>, ApiError> {
    let limit = query.limit.unwrap_or(100);

    if !(1..=endpoints::MAX_NUM_SHARES_IN_RESPONSE).contains(&limit) {
        return Err(ApiError::BadRequest(format!(
            "limit must be between 1 and {}, got {limit}",
            endpoints::MAX_NUM_SHARES_IN_RESPONSE
        )));
    }

    // Convert ISO 8601 strings to Unix timestamps
    let start_time = match query.start_time.as_ref() {
        Some(s) => match DateTime::parse_from_rfc3339(s) {
            Ok(dt) => dt.timestamp() as u64,
            Err(_) => {
                return Err(ApiError::BadRequest(
                    "Invalid start_time format, expected RFC 3339".into(),
                ));
            }
        },
        None => 0,
    };

    let end_time = match query.end_time.as_ref() {
        Some(s) => match DateTime::parse_from_rfc3339(s) {
            Ok(dt) => dt.timestamp() as u64,
            Err(_) => {
                return Err(ApiError::BadRequest(
                    "Invalid end_time format, expected RFC 3339".into(),
                ));
            }
        },
        None => {
            // Default to current time
            let now = chrono::Utc::now();
            now.timestamp() as u64
        }
    };

    if end_time < start_time {
        return Err(ApiError::BadRequest(
            "end_time must not be before start_time".into(),
        ));
    }

    let shares = state.chain_store_handle.get_pplns_shares_filtered(
        Some(limit as usize),
        Some(start_time),
        Some(end_time),
    );

    Ok(Json(shares))
}

#[cfg(test)]
mod tests;
