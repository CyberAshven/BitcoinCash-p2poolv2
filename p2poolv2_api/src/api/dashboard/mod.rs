// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The node's web dashboard, rendered in Rust.
//!
//! Pages are HTML built on the server from the same store, PPLNS window and
//! metrics the JSON API reads, so the dashboard and the API cannot disagree.
//! The design is CashDragon0's (PR #8); its stylesheets, font and logo are
//! compiled into the binary ([`assets`]). The browser runs no script: pages
//! link to each other, search is a plain form, and long tables have pages.
//! The routes sit behind the API's authentication unless
//! `[api] public_dashboard` opens them: a node with `auth_user` set answers
//! with a Basic challenge, which browsers turn into their own login prompt.

pub(crate) mod assets;
pub(crate) mod chart;
pub(crate) mod history;
pub(crate) mod pages;

use crate::api::endpoints::MAX_NUM_SHARES_IN_RESPONSE;
use crate::api::endpoints::miner::miner_report;
use crate::api::endpoints::share::build_share_output;
use crate::api::error::ApiError;
use crate::api::server::AppState;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use bitcoin::{BlockHash, Target};
use history::{HashrateHistory, is_block, newest_shares, pool_hashrate, work_from_bits};
use maud::Markup;
use p2poolv2_lib::address::bch_share::BchShareNetwork;
use p2poolv2_lib::address_display::ShareInfoDisplay;
use p2poolv2_lib::address_display::payout_address_string;
use pages::{
    BlockView, BlocksView, FoundBlockRow, MinerRow, MinerView, MinersView, Overview, PAGE_SIZE,
    Paging, ShareRow, ShareView, Site, WorkerRow,
};
use serde::Deserialize;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Shares a miner's hashrate is estimated from.
const MINER_HASHRATE_SHARES: u32 = 500;

/// Found blocks the overview lists; the blocks page has them all.
const RECENT_BLOCKS: usize = 5;

/// What the dashboard handlers share: the API's state and the history its
/// sampler fills.
pub(crate) struct DashboardState {
    pub(crate) app: Arc<AppState>,
    pub(crate) history: Arc<HashrateHistory>,
}

/// The dashboard's routes, ready to merge into the API's share chain
/// routes.
pub(crate) fn router(app: Arc<AppState>, history: Arc<HashrateHistory>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/dashboard", get(overview))
        .route("/dashboard/blocks", get(blocks))
        .route("/dashboard/block/{hash}", get(block))
        .route("/dashboard/miners", get(miners))
        .route("/dashboard/miner", get(miner))
        .route("/dashboard/share/{hash}", get(share))
        .route("/dashboard/help", get(help))
        .route("/dashboard/search", get(search))
        .route("/dashboard/assets/{name}", get(assets::serve))
        .with_state(Arc::new(DashboardState { app, history }))
}

/// How the dashboard names the chain this node mines, in full and in the
/// header badge.
fn site(state: &AppState) -> Site {
    let (network, badge) = match state.app_config.cash_network {
        Some(BchShareNetwork::Mainnet) => ("Bitcoin Cash mainnet".to_string(), "mainnet"),
        Some(BchShareNetwork::Chipnet) => ("Bitcoin Cash chipnet".to_string(), "chipnet"),
        Some(BchShareNetwork::Regtest) => ("Bitcoin Cash regtest".to_string(), "regtest"),
        None => (format!("Bitcoin {}", state.app_config.network), "bitcoin"),
    };
    Site {
        network,
        badge: badge.to_string(),
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

fn html(markup: Markup) -> Response {
    Html(markup.into_string()).into_response()
}

/// Run a synchronous store read on a blocking thread.
async fn read_store<T: Send + 'static>(
    state: &Arc<AppState>,
    read: impl FnOnce(&AppState) -> T + Send + 'static,
) -> Result<T, ApiError> {
    let state = Arc::clone(state);
    tokio::task::spawn_blocking(move || read(&state))
        .await
        .map_err(|error| ApiError::ServerError(format!("Store read failed: {error}")))
}

/// An error as a dashboard page with the matching status.
fn error_page(state: &AppState, error: ApiError) -> Response {
    let (status, message) = match error {
        ApiError::BadRequest(message) => (StatusCode::BAD_REQUEST, message),
        ApiError::NotFound(message) => (StatusCode::NOT_FOUND, message),
        ApiError::ServiceUnavailable(message) => (StatusCode::SERVICE_UNAVAILABLE, message),
        ApiError::ServerError(message) => (StatusCode::INTERNAL_SERVER_ERROR, message),
    };
    (status, html(pages::error(&site(state), &message))).into_response()
}

/// A `?page=` query, counted from 1.
#[derive(Deserialize)]
struct PageQuery {
    page: Option<usize>,
}

/// Whether the share `blockhash` met its own Bitcoin Cash block target.
fn share_found_a_block(state: &AppState, blockhash: &BlockHash) -> bool {
    state
        .chain_store_handle
        .store_handle()
        .store()
        .get_share_header(blockhash)
        .ok()
        .flatten()
        .is_some_and(|header| is_block(&header.bitcoin_header))
}

/// One page of the confirmed share chain, newest first, each share followed
/// by the uncles it includes, with the tip height.
fn share_page(state: &AppState, page: Option<usize>) -> (Option<u32>, Paging, Vec<ShareRow>) {
    let Ok(Some(tip)) = state.chain_store_handle.get_tip_height() else {
        return (None, Paging::of(None, 0), Vec::new());
    };
    let paging = Paging::of(page, tip as usize + 1);
    let skipped = u32::try_from((paging.page - 1) * PAGE_SIZE).unwrap_or(u32::MAX);
    let newest = tip.saturating_sub(skipped);
    let oldest = newest.saturating_sub(PAGE_SIZE as u32 - 1);
    let shares = state
        .chain_store_handle
        .store_handle()
        .store()
        .query_shares(oldest, newest)
        .unwrap_or_default();
    let mut rows = Vec::with_capacity(shares.len());
    for share in shares.iter().rev() {
        let display = ShareInfoDisplay::from_share_info(
            share,
            Some(state.app_config.network),
            state.app_config.cash_network,
        );
        rows.push(ShareRow {
            height: Some(share.height),
            blockhash: share.blockhash.to_string(),
            time: share.timestamp,
            payout_address: display.miner_bitcoin_address,
            share_address: display.miner_address,
            difficulty: Some(Target::from_compact(share.bits).difficulty_float()),
            is_uncle: false,
            is_block: share_found_a_block(state, &share.blockhash),
        });
        for uncle in display.uncles {
            rows.push(ShareRow {
                height: uncle.height,
                blockhash: uncle.blockhash.to_string(),
                time: uncle.timestamp,
                payout_address: uncle.miner_bitcoin_address,
                share_address: uncle.miner_address,
                difficulty: None,
                is_uncle: true,
                is_block: share_found_a_block(state, &uncle.blockhash),
            });
        }
    }
    (Some(tip), paging, rows)
}

/// Blocks the pool found, as the store records them on confirmation, newest
/// first.
fn found_blocks(state: &AppState) -> Vec<FoundBlockRow> {
    let found = match state.chain_store_handle.get_found_blocks() {
        Ok(found) => found,
        Err(error) => {
            tracing::warn!("Dashboard could not read found blocks: {error}");
            return Vec::new();
        }
    };
    found
        .iter()
        .rev()
        .map(|block| FoundBlockRow {
            height: block.bitcoin_height,
            block_hash: block.bitcoin_blockhash,
            share_hash: block.share_blockhash,
            time: block.bitcoin_time,
            payout_address: payout_address_display(state, &block.miner_bitcoin_address),
        })
        .collect()
}

/// A payout address the store keeps in Bitcoin's spelling, spelled for the
/// chain this node mines. One that does not parse is shown as it is.
fn payout_address_display(state: &AppState, stored: &str) -> String {
    match bitcoin::Address::from_str(stored) {
        Ok(address) => {
            payout_address_string(&address.assume_checked(), state.app_config.cash_network)
        }
        Err(_) => stored.to_string(),
    }
}

/// The miners on this node with their workers, by accepted work, most first.
async fn local_miners(state: &AppState) -> Vec<MinerRow> {
    let metrics = state.metrics_handle.get_metrics().await;
    let mut miners: Vec<MinerRow> = metrics
        .users
        .iter()
        .map(|(address, user)| {
            let mut workers: Vec<WorkerRow> = user
                .workers
                .iter()
                .map(|(name, worker)| WorkerRow {
                    name: name.clone(),
                    active: worker.active,
                    accepted_work: worker.shares_valid_total,
                    best_share: worker.best_share,
                    last_share_at: worker.last_share_at,
                })
                .collect();
            workers.sort_by(|left, right| left.name.cmp(&right.name));
            MinerRow {
                address: address.clone(),
                workers,
                accepted_work: user.shares_valid_total,
                best_share: user.best_share,
                last_share_at: user.last_share_at,
                payout_fraction: None,
            }
        })
        .collect();
    miners.sort_by(|left, right| {
        right
            .accepted_work
            .cmp(&left.accepted_work)
            .then_with(|| left.address.cmp(&right.address))
    });
    miners
}

async fn overview(
    State(dashboard): State<Arc<DashboardState>>,
    Query(query): Query<PageQuery>,
) -> Response {
    let state = &dashboard.app;
    let chain = read_store(state, move |state| {
        (
            share_page(state, query.page),
            pool_hashrate(state),
            found_blocks(state),
        )
    })
    .await;
    let ((tip_height, shares_paging, shares), pool_hashrate, blocks) = match chain {
        Ok(chain) => chain,
        Err(error) => return error_page(state, error),
    };
    let metrics = state.metrics_handle.get_metrics().await;
    let miners_online = metrics
        .users
        .values()
        .filter(|user| user.workers.values().any(|worker| worker.active))
        .count();
    let workers_online = metrics
        .users
        .values()
        .map(|user| user.workers.values().filter(|worker| worker.active).count())
        .sum();
    let network_difficulty = state.tracker_handle.get_network_difficulty();
    let samples = dashboard.history.samples();
    let local_hashrate = samples.iter().rev().find_map(|sample| sample.local);
    let blocks_found = blocks.len();

    html(pages::overview(&Overview {
        site: site(state),
        now: unix_now(),
        tip_height,
        pool_hashrate,
        local_hashrate,
        network_hashrate: network_difficulty.map(history::network_hashrate),
        network_difficulty,
        miners_online,
        workers_online,
        samples,
        blocks_found,
        blocks: blocks.into_iter().take(RECENT_BLOCKS).collect(),
        shares,
        shares_paging,
    }))
}

async fn blocks(
    State(dashboard): State<Arc<DashboardState>>,
    Query(query): Query<PageQuery>,
) -> Response {
    let state = &dashboard.app;
    let all = match read_store(state, found_blocks).await {
        Ok(all) => all,
        Err(error) => return error_page(state, error),
    };
    let paging = Paging::of(query.page, all.len());
    let blocks_found = all.len();
    let blocks = all
        .into_iter()
        .skip((paging.page - 1) * PAGE_SIZE)
        .take(PAGE_SIZE)
        .collect();
    html(pages::blocks(&BlocksView {
        site: site(state),
        now: unix_now(),
        blocks_found,
        blocks,
        paging,
    }))
}

async fn block(State(dashboard): State<Arc<DashboardState>>, Path(hash): Path<String>) -> Response {
    let state = &dashboard.app;
    let Ok(block_hash) = BlockHash::from_str(&hash) else {
        return error_page(
            state,
            ApiError::BadRequest(format!("Not a block hash: {hash}")),
        );
    };
    let all = match read_store(state, found_blocks).await {
        Ok(all) => all,
        Err(error) => return error_page(state, error),
    };
    match all.into_iter().find(|block| block.block_hash == block_hash) {
        Some(block) => html(pages::block(&BlockView {
            site: site(state),
            now: unix_now(),
            block,
        })),
        None => error_page(
            state,
            ApiError::NotFound(format!("The pool has not found a block with hash {hash}.")),
        ),
    }
}

async fn miners(
    State(dashboard): State<Arc<DashboardState>>,
    Query(query): Query<PageQuery>,
) -> Response {
    let state = &dashboard.app;
    let all = local_miners(state).await;
    let paging = Paging::of(query.page, all.len());
    let mut miners: Vec<MinerRow> = all
        .into_iter()
        .skip((paging.page - 1) * PAGE_SIZE)
        .take(PAGE_SIZE)
        .collect();
    // Only this page's miners are looked up in the window.
    for miner in &mut miners {
        miner.payout_fraction = miner_report(state, &miner.address, 1)
            .await
            .ok()
            .map(|report| report.payout_fraction);
    }
    html(pages::miners(&MinersView {
        site: site(state),
        now: unix_now(),
        miners,
        paging,
    }))
}

#[derive(Deserialize)]
struct MinerPageQuery {
    address: Option<String>,
    page: Option<usize>,
}

/// A miner's hashrate from its share of the work in the newest shares: its
/// shares' work over the time those shares span, with how many shares and
/// seconds that was.
fn miner_hashrate(state: &AppState, address: &str) -> Option<(f64, usize, u32)> {
    let shares = newest_shares(state, MINER_HASHRATE_SHARES);
    let oldest = shares.first()?.timestamp;
    let newest = shares.last()?.timestamp;
    let span = newest.checked_sub(oldest).filter(|span| *span > 0)?;
    let work: f64 = shares
        .iter()
        .skip(1)
        .filter(|share| {
            let display = ShareInfoDisplay::from_share_info(
                share,
                Some(state.app_config.network),
                state.app_config.cash_network,
            );
            display.miner_bitcoin_address == address || display.miner_address == address
        })
        .map(|share| work_from_bits(share.bits))
        .sum();
    Some((work / f64::from(span), shares.len(), span))
}

async fn miner(
    State(dashboard): State<Arc<DashboardState>>,
    Query(query): Query<MinerPageQuery>,
) -> Response {
    let state = &dashboard.app;
    let Some(address) = query
        .address
        .as_deref()
        .map(str::trim)
        .filter(|address| !address.is_empty())
    else {
        return error_page(
            state,
            ApiError::BadRequest("Enter a payout or share chain address.".to_string()),
        );
    };
    let report = match miner_report(state, address, MAX_NUM_SHARES_IN_RESPONSE as usize).await {
        Ok(report) => report,
        Err(error) => return error_page(state, error),
    };
    let reported_address = report.address.clone();
    let estimate = read_store(state, move |state| miner_hashrate(state, &reported_address))
        .await
        .ok()
        .flatten();
    let workers = local_miners(state)
        .await
        .into_iter()
        .find(|miner| miner.address == report.address || miner.address == address)
        .map(|miner| miner.workers)
        .unwrap_or_default();
    let paging = Paging::of(query.page, report.shares.len());
    html(pages::miner(&MinerView {
        site: site(state),
        now: unix_now(),
        report,
        hashrate: estimate.map(|(hashrate, _, _)| hashrate),
        recent_window: estimate.map(|(_, shares, seconds)| (shares, seconds)),
        workers,
        paging,
    }))
}

async fn share(State(dashboard): State<Arc<DashboardState>>, Path(hash): Path<String>) -> Response {
    let state = &dashboard.app;
    let blockhash = match BlockHash::from_str(&hash) {
        Ok(blockhash) => blockhash,
        Err(error) => {
            return error_page(
                state,
                ApiError::BadRequest(format!("Not a share hash: {error}")),
            );
        }
    };
    let found = read_store(state, move |state| {
        let output = build_share_output(
            &state.chain_store_handle,
            &blockhash,
            false,
            state.app_config.network,
            state.app_config.cash_network,
        )?;
        Ok::<_, ApiError>((output, share_found_a_block(state, &blockhash)))
    })
    .await
    .and_then(|found| found);
    match found {
        Ok((share, is_block)) => html(pages::share(&ShareView {
            site: site(state),
            share,
            is_block,
        })),
        Err(error) => error_page(state, error),
    }
}

async fn help(State(dashboard): State<Arc<DashboardState>>) -> Response {
    html(pages::help(&site(&dashboard.app)))
}

#[derive(Deserialize)]
struct SearchQuery {
    q: Option<String>,
}

/// Where a search goes: a share hash to its share, a found block's hash to
/// its block, anything else to the miner page, which says when it is not an
/// address.
async fn search(
    State(dashboard): State<Arc<DashboardState>>,
    Query(query): Query<SearchQuery>,
) -> Response {
    let state = &dashboard.app;
    let text = query.q.as_deref().map(str::trim).unwrap_or_default();
    if text.is_empty() {
        return error_page(
            state,
            ApiError::BadRequest(
                "Enter a miner address, a share hash or a block hash.".to_string(),
            ),
        );
    }
    let looks_like_hash = text.len() == 64 && text.bytes().all(|byte| byte.is_ascii_hexdigit());
    if !looks_like_hash {
        return Redirect::to(&pages::miner_href(text)).into_response();
    }
    let Ok(hash) = BlockHash::from_str(text) else {
        return error_page(state, ApiError::BadRequest(format!("Not a hash: {text}")));
    };
    let target = read_store(state, move |state| {
        if state
            .chain_store_handle
            .store_handle()
            .store()
            .get_share_header(&hash)
            .ok()
            .flatten()
            .is_some()
        {
            Some(pages::share_href(&hash.to_string()))
        } else if found_blocks(state)
            .iter()
            .any(|block| block.block_hash == hash)
        {
            Some(pages::block_href(&hash.to_string()))
        } else {
            None
        }
    })
    .await;
    match target {
        Ok(Some(href)) => Redirect::to(&href).into_response(),
        Ok(None) => error_page(
            state,
            ApiError::NotFound(format!(
                "No share and no block found by the pool has hash {text}."
            )),
        ),
        Err(error) => error_page(state, error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::server::{AppConfig, WebOptions, build_router_with_history};
    use axum::body::Body;
    use axum::http::{HeaderMap, Request, header};
    use p2poolv2_lib::accounting::stats::metrics;
    use p2poolv2_lib::monitoring_events::create_monitoring_event_channel;
    use p2poolv2_lib::node::actor::NodeHandle;
    use p2poolv2_lib::stratum::work::tracker::start_tracker_actor;
    use p2poolv2_lib::test_utils::{genesis_for_tests, setup_test_chain_store_handle};
    use tower::ServiceExt;

    async fn build_state(
        auth: Option<(&str, &str)>,
        cash_network: Option<BchShareNetwork>,
    ) -> (Arc<AppState>, tempfile::TempDir, tempfile::TempDir) {
        let (chain_store_handle, store_dir) = setup_test_chain_store_handle(true).await;
        let metrics_dir = tempfile::tempdir().unwrap();
        let metrics_handle =
            metrics::start_metrics(metrics_dir.path().to_str().unwrap().to_string())
                .await
                .unwrap();
        let state = Arc::new(AppState {
            app_config: AppConfig {
                pool_signature_length: 0,
                network: bitcoin::Network::Signet,
                cash_network,
                cors_allowed: false,
            },
            chain_store_handle,
            metrics_handle,
            tracker_handle: start_tracker_actor(),
            node_handle: NodeHandle::new_for_test(),
            monitoring_event_sender: create_monitoring_event_channel().0,
            auth_user: auth.map(|(user, _)| user.to_string()),
            auth_token: auth.map(|(_, token)| token.to_string()),
            pplns_window: None,
        });
        (state, store_dir, metrics_dir)
    }

    async fn get(state: &Arc<AppState>, uri: &str) -> (StatusCode, String, HeaderMap) {
        get_with(state, uri, WebOptions::default()).await
    }

    async fn get_with(
        state: &Arc<AppState>,
        uri: &str,
        web: WebOptions,
    ) -> (StatusCode, String, HeaderMap) {
        let app = build_router_with_history(
            Arc::clone(state),
            state.app_config.clone(),
            Arc::new(HashrateHistory::default()),
            web,
        );
        let response = app
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (
            status,
            String::from_utf8_lossy(&bytes).into_owned(),
            headers,
        )
    }

    fn public() -> WebOptions {
        WebOptions {
            public: true,
            web_root: None,
        }
    }

    /// A frontend build: an index page and a WebAssembly module.
    fn frontend() -> tempfile::TempDir {
        let web_root = tempfile::tempdir().unwrap();
        std::fs::write(web_root.path().join("index.html"), "<p>frontend</p>").unwrap();
        std::fs::write(web_root.path().join("app.wasm"), b"\0asm").unwrap();
        web_root
    }

    #[tokio::test]
    async fn the_overview_is_html_without_script() {
        let (state, _store, _metrics) = build_state(None, Some(BchShareNetwork::Chipnet)).await;
        let (status, body, _) = get(&state, "/dashboard").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.starts_with("<!DOCTYPE html>"));
        assert!(body.contains("Bitcoin Cash chipnet"));
        assert!(body.contains("Share chain height"));
        assert!(!body.contains("<script"));
    }

    #[tokio::test]
    async fn a_known_share_has_a_page() {
        let (state, _store, _metrics) = build_state(None, None).await;
        let genesis_block = genesis_for_tests();
        let genesis = genesis_block.block_hash();
        state
            .chain_store_handle
            .init_or_setup_genesis(genesis_block)
            .await
            .unwrap();
        let (status, body, _) = get(&state, &format!("/dashboard/share/{genesis}")).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains(&genesis.to_string()));
        assert!(body.contains("Bitcoin Cash header"));
    }

    #[tokio::test]
    async fn a_bad_share_hash_is_a_400_page() {
        let (state, _store, _metrics) = build_state(None, None).await;
        let (status, body, _) = get(&state, "/dashboard/share/not-a-hash").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("Not a share hash"));
    }

    #[tokio::test]
    async fn an_unknown_share_is_a_404_page() {
        let (state, _store, _metrics) = build_state(None, None).await;
        let missing = "00".repeat(32);
        let (status, _, _) = get(&state, &format!("/dashboard/share/{missing}")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn the_miner_page_needs_an_address() {
        let (state, _store, _metrics) = build_state(None, None).await;
        let (status, body, _) = get(&state, "/dashboard/miner?address=").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("Enter a payout or share chain address."));
    }

    #[tokio::test]
    async fn a_node_with_credentials_challenges_the_browser() {
        let (state, _store, _metrics) = build_state(Some(("operator", "secret-token")), None).await;
        let (status, _, headers) = get(&state, "/dashboard").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let challenge = headers.get(header::WWW_AUTHENTICATE).unwrap();
        assert!(challenge.to_str().unwrap().starts_with("Basic"));
    }

    #[tokio::test]
    async fn a_public_node_opens_its_share_chain_but_not_its_own_routes() {
        let (state, _store, _metrics) = build_state(Some(("operator", "secret-token")), None).await;
        let (status, body, _) = get_with(&state, "/dashboard", public()).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("Share chain height"));
        // A bad address is the handler's 400, not the login's 401.
        let (status, _, _) = get_with(&state, "/miner?address=not-an-address", public()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        for own_route in ["/metrics", "/peers", "/blocked_ips", "/jd_prepare"] {
            let (status, _, headers) = get_with(&state, own_route, public()).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{own_route}");
            assert!(headers.contains_key(header::WWW_AUTHENTICATE));
        }
    }

    #[tokio::test]
    async fn the_root_opens_the_dashboard_without_a_frontend() {
        let (state, _store, _metrics) = build_state(None, None).await;
        let (status, _, headers) = get(&state, "/").await;
        assert!(status.is_redirection());
        assert_eq!(headers.get(header::LOCATION).unwrap(), "/dashboard");
    }

    #[tokio::test]
    async fn a_web_root_serves_its_frontend_beside_the_api() {
        let (state, _store, _metrics) = build_state(None, None).await;
        let web_root = frontend();
        let web = || WebOptions {
            public: false,
            web_root: Some(web_root.path().to_path_buf()),
        };
        let (status, body, _) = get_with(&state, "/", web()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "<p>frontend</p>");
        let (status, _, headers) = get_with(&state, "/app.wasm", web()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            headers.get(header::CONTENT_TYPE).unwrap(),
            "application/wasm"
        );
        // A path of the frontend's own routing gets its index page.
        let (status, body, _) = get_with(&state, "/miners/someone", web()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "<p>frontend</p>");
        // The API and the built-in dashboard keep their paths.
        let (_, body, _) = get_with(&state, "/health", web()).await;
        assert_eq!(body, "OK");
        let (_, body, _) = get_with(&state, "/dashboard", web()).await;
        assert!(body.contains("Share chain height"));
    }

    #[tokio::test]
    async fn a_web_root_follows_the_login() {
        let (state, _store, _metrics) = build_state(Some(("operator", "secret-token")), None).await;
        let web_root = frontend();
        let web = |public| WebOptions {
            public,
            web_root: Some(web_root.path().to_path_buf()),
        };
        let (status, _, headers) = get_with(&state, "/", web(false)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(headers.contains_key(header::WWW_AUTHENTICATE));
        let (status, body, _) = get_with(&state, "/", web(true)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "<p>frontend</p>");
    }

    #[tokio::test]
    async fn the_old_static_files_are_gone() {
        let (state, _store, _metrics) = build_state(None, None).await;
        let (status, _, _) = get(&state, "/static/dashboard.js").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn every_page_has_the_menu_and_no_script() {
        let (state, _store, _metrics) = build_state(None, Some(BchShareNetwork::Chipnet)).await;
        for path in [
            "/dashboard",
            "/dashboard/blocks",
            "/dashboard/miners",
            "/dashboard/help",
        ] {
            let (status, body, _) = get(&state, path).await;
            assert_eq!(status, StatusCode::OK, "{path}");
            assert!(body.contains("class=\"site-header\""), "{path}");
            assert!(body.contains("action=\"/dashboard/search\""), "{path}");
            assert!(body.contains("aria-current=\"page\""), "{path}");
            assert!(!body.contains("<script"), "{path}");
        }
    }

    #[tokio::test]
    async fn the_assets_are_served_from_the_binary() {
        let (state, _store, _metrics) = build_state(None, None).await;
        let (status, body, headers) = get(&state, "/dashboard/assets/dashboard.css").await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            headers
                .get(header::CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("text/css")
        );
        assert!(body.contains(".kpi-card"));
        let (status, _, _) = get(&state, "/dashboard/assets/pico.min.css").await;
        assert_eq!(status, StatusCode::OK);
        let (status, _, _) = get(&state, "/dashboard/assets/dashboard.js").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn search_opens_a_share_or_a_miner() {
        let (state, _store, _metrics) = build_state(None, None).await;
        let genesis_block = genesis_for_tests();
        let genesis = genesis_block.block_hash();
        state
            .chain_store_handle
            .init_or_setup_genesis(genesis_block)
            .await
            .unwrap();
        let (status, _, headers) = get(&state, &format!("/dashboard/search?q={genesis}")).await;
        assert!(status.is_redirection());
        assert_eq!(
            headers.get(header::LOCATION).unwrap(),
            &format!("/dashboard/share/{genesis}")
        );
        let (status, _, headers) = get(&state, "/dashboard/search?q=+tb1qexample+").await;
        assert!(status.is_redirection());
        assert_eq!(
            headers.get(header::LOCATION).unwrap(),
            "/dashboard/miner?address=tb1qexample"
        );
        let (status, body, _) = get(&state, "/dashboard/search?q=").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("Enter a miner address"));
        let missing = "11".repeat(32);
        let (status, _, _) = get(&state, &format!("/dashboard/search?q={missing}")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_share_page_leads_back_to_its_miner() {
        let (state, _store, _metrics) = build_state(None, None).await;
        let genesis_block = genesis_for_tests();
        let genesis = genesis_block.block_hash();
        state
            .chain_store_handle
            .init_or_setup_genesis(genesis_block)
            .await
            .unwrap();
        let (_, body, _) = get(&state, &format!("/dashboard/share/{genesis}")).await;
        assert!(body.contains("aria-label=\"breadcrumb\""));
        assert!(body.contains("href=\"/dashboard/miner?address="));
        assert!(body.contains("href=\"/dashboard/miners\""));
    }

    #[tokio::test]
    async fn an_unknown_block_is_a_404_page() {
        let (state, _store, _metrics) = build_state(None, None).await;
        let (status, body, _) = get(&state, &format!("/dashboard/block/{}", "22".repeat(32))).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.contains("has not found a block"));
        let (status, _, _) = get(&state, "/dashboard/block/nope").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn the_overview_clamps_its_page() {
        let (state, _store, _metrics) = build_state(None, None).await;
        let (status, body, _) = get(&state, "/dashboard?page=999").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("No confirmed shares yet."));
    }
}
