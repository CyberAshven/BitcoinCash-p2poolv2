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
//! a 401 that already carried credentials with a Basic challenge, which
//! browsers turn into their own login prompt.

pub(crate) mod assets;
pub(crate) mod chart;
pub(crate) mod history;
pub(crate) mod pages;
#[cfg(test)]
mod style_guard;

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
    Paging, ShareRow, ShareView, SharesView, Site, WorkerRow,
};
use serde::Deserialize;
use std::collections::HashSet;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Shares a miner's hashrate is estimated from.
const MINER_HASHRATE_SHARES: u32 = 500;

/// Found blocks the overview lists; the blocks page has them all.
const RECENT_BLOCKS: usize = 5;

/// Confirmed share heights the overview's Recent shares panel lists; the
/// shares page has them all, [`pages::PAGE_SIZE`] per page.
const OVERVIEW_SHARES: usize = 5;

/// What the dashboard handlers share: the API's state and the history its
/// sampler fills.
pub(crate) struct DashboardState {
    pub(crate) app: Arc<AppState>,
    pub(crate) history: Arc<HashrateHistory>,
}

/// The dashboard's routes, ready to merge into the API's share chain
/// routes. The Admin button's `/dashboard/signin` landing page is not among
/// them: it opens with the node's login, so [`signin_route`] carries it to
/// the authenticated side instead.
pub(crate) fn router(app: Arc<AppState>, history: Arc<HashrateHistory>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/dashboard", get(overview))
        .route("/dashboard/shares", get(shares))
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

/// The Admin button's `/dashboard/signin` landing page, a route the API
/// joins to its authenticated routes so it opens with the node's login.
pub(crate) fn signin_route(
    app: Arc<AppState>,
    history: Arc<HashrateHistory>,
) -> Router<Arc<AppState>> {
    Router::new()
        .route("/dashboard/signin", get(signin))
        .with_state(Arc::new(DashboardState { app, history }))
}

/// How the dashboard names the chain this node mines, in full and in the
/// header badge. `/network` serves the same value as JSON.
pub(crate) fn site(state: &AppState) -> Site {
    let (network, badge) = match state.app_config.cash_network {
        Some(BchShareNetwork::Mainnet) => ("Bitcoin Cash mainnet".to_string(), "mainnet"),
        Some(BchShareNetwork::Chipnet) => ("Bitcoin Cash chipnet".to_string(), "chipnet"),
        Some(BchShareNetwork::Regtest) => ("Bitcoin Cash regtest".to_string(), "regtest"),
        Some(BchShareNetwork::Scalenet) => ("Bitcoin Cash scalenet".to_string(), "scalenet"),
        None => (format!("Bitcoin {}", state.app_config.network), "bitcoin"),
    };
    Site {
        network,
        badge: badge.to_string(),
        auth: state.auth_user.is_some(),
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

/// A `?page=` query, counted from 1, plus the overview's `?auto=` toggle,
/// `?range=` chart window and the `?pool=`/`?node=` chart series toggles
/// (a series is hidden when its parameter is `0`).
#[derive(Deserialize)]
struct PageQuery {
    page: Option<usize>,
    auto: Option<String>,
    range: Option<String>,
    pool: Option<String>,
    node: Option<String>,
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
/// by the uncles it includes, with the tip height. `window` is how many
/// heights a page spans: [`OVERVIEW_SHARES`] for the overview's Recent
/// shares, `pages::PAGE_SIZE` for the shares page.
fn share_page(
    state: &AppState,
    page: Option<usize>,
    window: usize,
) -> (Option<u32>, Paging, Vec<ShareRow>) {
    let Ok(Some(tip)) = state.chain_store_handle.get_tip_height() else {
        return (None, Paging::of(None, 0), Vec::new());
    };
    let paging = Paging::of(page, tip as usize + 1);
    let skipped = u32::try_from((paging.page - 1) * window).unwrap_or(u32::MAX);
    let newest = tip.saturating_sub(skipped);
    let span = u32::try_from(window.saturating_sub(1)).unwrap_or(u32::MAX);
    let oldest = newest.saturating_sub(span);
    let shares = state
        .chain_store_handle
        .store_handle()
        .store()
        .query_shares(oldest, newest)
        .unwrap_or_default();
    let mut rows = Vec::with_capacity(shares.len());
    for share in shares.iter() {
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
    match stored.parse::<p2poolv2_lib::config::PayoutAddress>() {
        Ok(address) => payout_address_string(&address, state.app_config.cash_network),
        Err(_) => stored.to_string(),
    }
}

/// The miners on this node with their workers, by accepted work, most first.
pub(crate) async fn local_miners(state: &AppState) -> Vec<MinerRow> {
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

/// The payout addresses of miners on this node with at least one active
/// worker, as [`local_miners`] lists them. Unlike [`local_miners`], a miner
/// whose workers have all gone inactive (or expired) is left out: it may
/// still show a historical row on the miners page (grace periods), but it
/// is not connected now and its share-chain work should not count toward
/// the local hashrate.
pub(crate) async fn connected_miners(state: &AppState) -> HashSet<String> {
    let metrics = state.metrics_handle.get_metrics().await;
    metrics
        .users
        .iter()
        .filter(|(_, user)| user.any_active_workers())
        .map(|(address, _)| address.clone())
        .collect()
}

/// How many of `miners` have a worker online, and how many workers are
/// online in all.
pub(crate) fn online_counts(miners: &[MinerRow]) -> (usize, usize) {
    let miners_online = miners
        .iter()
        .filter(|miner| miner.workers.iter().any(|worker| worker.active))
        .count();
    let workers_online = miners
        .iter()
        .map(|miner| miner.workers.iter().filter(|worker| worker.active).count())
        .sum();
    (miners_online, workers_online)
}

/// Whether a miner on this node is the one looked up as `typed`, which the
/// window lookup spells `canonical` (a payout address may be typed without
/// its prefix).
pub(crate) fn is_looked_up_miner(miner: &MinerRow, typed: &str, canonical: Option<&str>) -> bool {
    miner.address == typed || canonical.is_some_and(|canonical| miner.address == canonical)
}

/// Page `page` of `miners`, each with its fraction of the next block's
/// payout. Only this page's miners are looked up in the window.
pub(crate) async fn miners_page(
    state: &AppState,
    miners: Vec<MinerRow>,
    page: Option<usize>,
) -> (Vec<MinerRow>, Paging) {
    let paging = Paging::of(page, miners.len());
    let mut shown: Vec<MinerRow> = miners
        .into_iter()
        .skip((paging.page - 1) * PAGE_SIZE)
        .take(PAGE_SIZE)
        .collect();
    for miner in &mut shown {
        miner.payout_fraction = miner_report(state, &miner.address, 1)
            .await
            .ok()
            .map(|report| report.payout_fraction);
    }
    (shown, paging)
}

async fn overview(
    State(dashboard): State<Arc<DashboardState>>,
    Query(query): Query<PageQuery>,
) -> Response {
    let state = &dashboard.app;
    let auto = pages::Auto::from_query(query.auto.as_deref());
    let range_key = pages::normalize_range(query.range.as_deref());
    let pool_on = pages::series_on(query.pool.as_deref());
    let node_on = pages::series_on(query.node.as_deref());
    let now = unix_now();
    let chain = read_store(state, move |state| {
        (
            share_page(state, None, OVERVIEW_SHARES),
            pool_hashrate(state, now),
            found_blocks(state),
        )
    })
    .await;
    let ((tip_height, _, shares), pool_hashrate, blocks) = match chain {
        Ok(chain) => chain,
        Err(error) => return error_page(state, error),
    };
    let miners = local_miners(state).await;
    let (miners_online, workers_online) = online_counts(&miners);
    let samples = dashboard.history.samples();
    let live_network_difficulty = state.tracker_handle.get_network_difficulty();
    // Mirror the wasm dashboard: when the live template feed has not
    // produced a difficulty yet, fall back to the newest sampled network
    // hashrate and invert it back to a difficulty, so the card never goes
    // blank while samples still carry a value.
    let network_hashrate = live_network_difficulty
        .map(history::network_hashrate)
        .or_else(|| samples.iter().rev().find_map(|sample| sample.network));
    let network_difficulty = live_network_difficulty
        .or_else(|| network_hashrate.map(history::difficulty_from_network_hashrate));
    let local_hashrate = samples.iter().rev().find_map(|sample| sample.local);
    // Harmonize with the pool KPI: fall back to the newest non-null sample
    // when the live recompute has too few shares to form a span, so both
    // cards show a value (or both show the dash) together.
    let pool_hashrate =
        pool_hashrate.or_else(|| samples.iter().rev().find_map(|sample| sample.pool));
    let local_hashrate =
        local_hashrate.or_else(|| samples.iter().rev().find_map(|sample| sample.local));
    let blocks_found = blocks.len();
    let metrics = state.metrics_handle.get_metrics().await;
    html(pages::overview(&Overview {
        site: site(state),
        now,
        tip_height,
        pool_hashrate,
        local_hashrate,
        network_hashrate,
        network_difficulty,
        share_difficulty: state.tracker_handle.get_pool_difficulty(),
        miners_online,
        workers_online,
        accepted_shares_total: metrics.accepted_total,
        rejected_shares_total: metrics.rejected_total,
        samples,
        blocks_found,
        blocks: blocks.into_iter().take(RECENT_BLOCKS).collect(),
        shares,
        auto,
        range_key,
        pool_on,
        node_on,
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

async fn shares(
    State(dashboard): State<Arc<DashboardState>>,
    Query(query): Query<PageQuery>,
) -> Response {
    let state = &dashboard.app;
    let (tip_height, paging, shares) =
        match read_store(state, move |state| share_page(state, query.page, PAGE_SIZE)).await {
            Ok(page) => page,
            Err(error) => return error_page(state, error),
        };
    html(pages::shares(&SharesView {
        site: site(state),
        now: unix_now(),
        tip_height,
        shares,
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
    let (miners, paging) = miners_page(state, local_miners(state).await, query.page).await;
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
        .find(|miner| is_looked_up_miner(miner, address, Some(&report.address)))
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
        // The page shows the header only, never the body, so a pruned share
        // renders like any other and the prune height is not needed.
        let output = build_share_output(
            &state.chain_store_handle,
            &blockhash,
            false,
            state.app_config.network,
            state.app_config.cash_network,
            0,
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

pub(crate) async fn signin(State(dashboard): State<Arc<DashboardState>>) -> Response {
    html(pages::signin(&site(&dashboard.app)))
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
    use p2poolv2_lib::accounting::payout::simple_pplns::SimplePplnsShare;
    use p2poolv2_lib::accounting::stats::metrics;
    use p2poolv2_lib::monitoring_events::create_monitoring_event_channel;
    use p2poolv2_lib::node::actor::NodeHandle;
    use p2poolv2_lib::stratum::work::tracker::start_tracker_actor;
    use p2poolv2_lib::test_utils::{
        PUBKEY_G, TestShareBlockBuilder, build_confirmed_test_chain, genesis_for_tests,
        setup_test_chain_store_handle,
    };
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
        // The middleware expects the stored token in the salt$hmac form that
        // config.toml carries; the tests hand over the login password itself.
        let (auth_user, auth_token) = match auth {
            Some((user, password)) => {
                let salt = "dash-test-salt";
                let hmac =
                    p2poolv2_lib::auth::password_to_hmac(salt, password).expect("hmac to compute");
                (Some(user.to_string()), Some(format!("{salt}${hmac}")))
            }
            None => (None, None),
        };
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
            auth_user,
            auth_token,
            pplns_window: None,
        });
        (state, store_dir, metrics_dir)
    }

    async fn get(state: &Arc<AppState>, uri: &str) -> (StatusCode, String, HeaderMap) {
        get_with(state, uri, WebOptions::default()).await
    }

    /// Confirm `share` on top of the chain, as the organise worker would.
    async fn confirm_share(
        state: &AppState,
        share: &p2poolv2_lib::shares::share_block::ShareBlock,
    ) {
        state
            .chain_store_handle
            .add_share_block_and_organise_header(share.clone())
            .await
            .unwrap();
        state
            .chain_store_handle
            .mark_block_valid(share.block_hash())
            .await
            .unwrap();
        state.chain_store_handle.organise_block().await.unwrap();
    }

    fn recent_share_row_heights(body: &str) -> Vec<u32> {
        let table = body
            .split_once("<h2>Recent shares")
            .and_then(|(_, rest)| rest.split_once("</table>"))
            .map(|(table, _)| table)
            .expect("overview has a Recent shares table");
        table
            .split("<tr")
            .skip(1)
            .filter_map(|row| {
                let cell = row.split_once("<td>").map(|(_, cell)| cell)?;
                let height = cell.split_once("</td>").map(|(height, _)| height)?;
                height.parse::<u32>().ok()
            })
            .collect()
    }

    async fn get_with(
        state: &Arc<AppState>,
        uri: &str,
        web: WebOptions,
    ) -> (StatusCode, String, HeaderMap) {
        get_with_headers(state, uri, web, &[]).await
    }

    async fn get_with_headers(
        state: &Arc<AppState>,
        uri: &str,
        web: WebOptions,
        headers: &[(&str, &str)],
    ) -> (StatusCode, String, HeaderMap) {
        let app = build_router_with_history(
            Arc::clone(state),
            state.app_config.clone(),
            Arc::new(HashrateHistory::default()),
            web,
        );
        let mut builder = Request::builder().uri(uri);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let response = app
            .oneshot(builder.body(Body::empty()).unwrap())
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
    async fn the_overview_lists_the_recent_shares_newest_first() {
        let (state, _store, _metrics) = build_state(None, None).await;
        let genesis_block = genesis_for_tests();
        state
            .chain_store_handle
            .init_or_setup_genesis(genesis_block.clone())
            .await
            .unwrap();
        let share = TestShareBlockBuilder::new()
            .prev_share_blockhash(genesis_block.block_hash().to_string())
            .miner_pubkey(PUBKEY_G)
            .build();
        confirm_share(&state, &share).await;
        let (status, body, _) = get(&state, "/dashboard").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(recent_share_row_heights(&body), vec![1, 0]);
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
    async fn a_first_visit_gets_a_bare_401() {
        let (state, _store, _metrics) = build_state(Some(("operator", "secret-token")), None).await;
        let (status, _, headers) = get(&state, "/dashboard").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(!headers.contains_key(header::WWW_AUTHENTICATE));
    }

    #[tokio::test]
    async fn a_wrong_login_gets_a_basic_challenge() {
        let (state, _store, _metrics) = build_state(Some(("operator", "secret-token")), None).await;
        let (status, _, headers) = get_with_headers(
            &state,
            "/dashboard",
            WebOptions::default(),
            &[("authorization", "Basic b3BlcmF0b3I6d3JvbmctcGFzc3dvcmQ=")],
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let challenge = headers.get(header::WWW_AUTHENTICATE).unwrap();
        assert!(challenge.to_str().unwrap().starts_with("Basic"));
    }

    #[tokio::test]
    async fn the_wasm_apps_marked_probe_gets_no_challenge() {
        let (state, _store, _metrics) = build_state(Some(("operator", "secret-token")), None).await;
        let (status, _, headers) = get_with_headers(
            &state,
            "/metrics",
            WebOptions::default(),
            &[
                ("authorization", "Basic b3BlcmF0b3I6d3JvbmctcGFzc3dvcmQ="),
                ("x-requested-with", "XMLHttpRequest"),
            ],
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(!headers.contains_key(header::WWW_AUTHENTICATE));
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
        // The peer list opens beside the share chain, not with the node's own routes.
        let (status, _, _) = get_with(&state, "/peers", public()).await;
        assert_eq!(status, StatusCode::OK);
        for own_route in [
            "/metrics",
            "/blocked_ips",
            "/jd_prepare",
            "/dashboard/signin",
        ] {
            let (status, _, headers) = get_with(&state, own_route, public()).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{own_route}");
            assert!(
                !headers.contains_key(header::WWW_AUTHENTICATE),
                "{own_route}"
            );
        }
    }

    #[tokio::test]
    async fn the_admin_signin_page_answers_the_api_login() {
        use base64::Engine;
        let (state, _store, _metrics) = build_state(Some(("operator", "secret-token")), None).await;
        let (status, _, headers) = get_with(&state, "/dashboard/signin", public()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(!headers.contains_key(header::WWW_AUTHENTICATE));
        let credentials = base64::engine::general_purpose::STANDARD.encode("operator:secret-token");
        let (status, body, _) = get_with_headers(
            &state,
            "/dashboard/signin",
            public(),
            &[("authorization", &format!("Basic {credentials}"))],
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("Signed in"));
        assert!(body.contains("/metrics"));
        assert!(!body.contains("<script"));
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
        assert!(!headers.contains_key(header::WWW_AUTHENTICATE));
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
    async fn the_network_is_named_as_the_header_names_it() {
        let (state, _store, _metrics) = build_state(None, Some(BchShareNetwork::Chipnet)).await;
        let (status, body, _) = get(&state, "/network").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body,
            r#"{"network":"Bitcoin Cash chipnet","badge":"chipnet","auth":false}"#
        );
    }

    #[tokio::test]
    async fn the_miners_page_is_served_as_json() {
        let (state, _store, _metrics) = build_state(None, None).await;
        let (status, body, _) = get(&state, "/miners?page=3").await;
        assert_eq!(status, StatusCode::OK);
        let miners: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(miners["miners_online"], 0);
        assert_eq!(miners["workers_online"], 0);
        assert_eq!(miners["total"], 0);
        assert_eq!(miners["page"], 1);
        assert_eq!(miners["pages"], 1);
        assert_eq!(miners["page_size"], PAGE_SIZE);
        assert_eq!(miners["accepted_shares_total"], 0);
        assert_eq!(miners["rejected_shares_total"], 0);
        assert_eq!(miners["miners"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn the_miners_json_carries_the_pools_share_totals() {
        let (state, _store, _metrics) = build_state(None, None).await;
        state
            .metrics_handle
            .increment_worker_count("bchtest:qrt".to_string(), "rig".to_string())
            .await
            .expect("authorize worker");
        let share = SimplePplnsShare {
            user_id: 1,
            difficulty: 100,
            btcaddress: Some("bchtest:qrt".to_string()),
            workername: Some("rig".to_string()),
            n_time: 1_000,
            job_id: "job".to_string(),
            extranonce2: "extra".to_string(),
            nonce: "nonce".to_string(),
        };
        state
            .metrics_handle
            .record_share_accepted(share, 110)
            .await
            .expect("record share");
        state
            .metrics_handle
            .record_share_rejected()
            .await
            .expect("record rejection");
        let (status, body, _) = get(&state, "/miners").await;
        assert_eq!(status, StatusCode::OK);
        let miners: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(miners["accepted_shares_total"], 1);
        assert_eq!(miners["rejected_shares_total"], 1);
    }

    #[tokio::test]
    async fn a_public_node_opens_the_miners_and_the_network() {
        let (state, _store, _metrics) = build_state(Some(("operator", "secret-token")), None).await;
        let (status, _, _) = get_with(&state, "/miners", public()).await;
        assert_eq!(status, StatusCode::OK);
        let (status, _, _) = get_with(&state, "/network", public()).await;
        assert_eq!(status, StatusCode::OK);
        let (status, _, _) = get(&state, "/miners").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn a_miner_row_serializes_as_the_wasm_app_reads_it() {
        // p2poolv2_web's `LocalMiner` and `Worker` decode these names.
        let row = MinerRow {
            address: "bchtest:q".to_string(),
            workers: vec![WorkerRow {
                name: "rig".to_string(),
                active: true,
                accepted_work: 10,
                best_share: 5,
                last_share_at: 9,
            }],
            accepted_work: 10,
            best_share: 5,
            last_share_at: 9,
            payout_fraction: None,
        };
        assert_eq!(
            serde_json::to_value(&row).unwrap(),
            serde_json::json!({
                "address": "bchtest:q",
                "workers": [{
                    "name": "rig",
                    "active": true,
                    "accepted_work": 10,
                    "best_share": 5,
                    "last_share_at": 9
                }],
                "accepted_work": 10,
                "best_share": 5,
                "last_share_at": 9,
                "payout_fraction": null
            })
        );
    }

    #[test]
    fn online_counts_count_active_workers() {
        let miner = |active: &[bool]| MinerRow {
            address: "bchtest:q".to_string(),
            workers: active
                .iter()
                .map(|active| WorkerRow {
                    name: "rig".to_string(),
                    active: *active,
                    accepted_work: 1,
                    best_share: 1,
                    last_share_at: 1,
                })
                .collect(),
            accepted_work: 1,
            best_share: 1,
            last_share_at: 1,
            payout_fraction: None,
        };
        let miners = [miner(&[true, false, true]), miner(&[false]), miner(&[])];
        assert_eq!(online_counts(&miners), (1, 2));
    }

    #[test]
    fn a_looked_up_miner_matches_either_spelling() {
        let miner = MinerRow {
            address: "qpayout".to_string(),
            workers: Vec::new(),
            accepted_work: 0,
            best_share: 0,
            last_share_at: 0,
            payout_fraction: None,
        };
        assert!(is_looked_up_miner(&miner, "qpayout", None));
        assert!(is_looked_up_miner(
            &miner,
            "bchtest:qpayout",
            Some("qpayout")
        ));
        assert!(!is_looked_up_miner(
            &miner,
            "bchtest:qpayout",
            Some("bchtest:qpayout")
        ));
    }

    #[tokio::test]
    async fn connected_miners_lists_a_miner_with_an_active_worker() {
        let (state, _store, _metrics) = build_state(None, None).await;
        state
            .metrics_handle
            .increment_worker_count("bchtest:qrx".to_string(), "rig".to_string())
            .await
            .expect("authorize worker");
        let share = SimplePplnsShare {
            user_id: 1,
            difficulty: 100,
            btcaddress: Some("bchtest:qrx".to_string()),
            workername: Some("rig".to_string()),
            n_time: 1_000,
            job_id: "job".to_string(),
            extranonce2: "extra".to_string(),
            nonce: "nonce".to_string(),
        };
        state
            .metrics_handle
            .record_share_accepted(share, 110)
            .await
            .expect("record share");
        let connected = connected_miners(&state).await;
        assert!(connected.contains("bchtest:qrx"));
    }

    #[tokio::test]
    async fn connected_miners_leaves_out_a_miner_without_an_active_worker() {
        let (state, _store, _metrics) = build_state(None, None).await;
        state
            .metrics_handle
            .increment_worker_count("bchtest:qrl".to_string(), "rig".to_string())
            .await
            .expect("authorize worker");
        state
            .metrics_handle
            .decrement_worker_count(Some("bchtest:qrl".to_string()), "rig".to_string())
            .await
            .expect("disconnect worker");
        let connected = connected_miners(&state).await;
        assert!(!connected.contains("bchtest:qrl"));
    }

    #[tokio::test]
    async fn the_overview_recent_shares_stop_at_five_heights() {
        let (state, _store, _metrics) = build_state(None, None).await;
        build_confirmed_test_chain(&state.chain_store_handle, 30).await;
        let (status, body, _) = get(&state, "/dashboard").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            recent_share_row_heights(&body),
            vec![30, 29, 28, 27, 26],
            "the overview lists the five newest heights, newest first"
        );
    }

    #[tokio::test]
    async fn the_overview_links_to_the_shares_page_instead_of_a_pager() {
        let (state, _store, _metrics) = build_state(None, None).await;
        build_confirmed_test_chain(&state.chain_store_handle, 8).await;
        let (status, body, _) = get(&state, "/dashboard").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("href=\"/dashboard/shares\""));
        assert!(body.contains("More found shares"));
        assert!(!body.contains("page="));
        assert!(!body.contains("rel=\"prev\""));
    }

    #[tokio::test]
    async fn the_overview_ignores_its_page_parameter() {
        let (state, _store, _metrics) = build_state(None, None).await;
        build_confirmed_test_chain(&state.chain_store_handle, 8).await;
        let (_, plain, _) = get(&state, "/dashboard").await;
        let (status, paged, _) = get(&state, "/dashboard?page=999").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            recent_share_row_heights(&paged),
            recent_share_row_heights(&plain),
            "the overview shows the same recent shares on every request"
        );
        assert_eq!(recent_share_row_heights(&paged), vec![8, 7, 6, 5, 4]);
    }

    #[tokio::test]
    async fn the_shares_page_paces_the_whole_chain() {
        let (state, _store, _metrics) = build_state(None, None).await;
        build_confirmed_test_chain(&state.chain_store_handle, 30).await;
        let (status, body, _) = get(&state, "/dashboard/shares").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            recent_share_row_heights(&body),
            (6..=30).rev().collect::<Vec<u32>>(),
            "the first page holds twenty-five heights, newest first"
        );
        assert!(body.contains("Page 1 of 2"));
        assert!(body.contains("/dashboard/shares?page=2"));
        let (_, second, _) = get(&state, "/dashboard/shares?page=2").await;
        assert_eq!(
            recent_share_row_heights(&second),
            vec![5, 4, 3, 2, 1, 0],
            "the second page carries the rest down to genesis"
        );
        assert!(second.contains("Page 2 of 2"));
    }

    #[tokio::test]
    async fn the_shares_page_without_a_chain_says_so() {
        let (state, _store, _metrics) = build_state(None, None).await;
        let (status, body, _) = get(&state, "/dashboard/shares").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("No confirmed shares yet."));
        assert!(!body.contains("rel=\"prev\""));
    }

    #[tokio::test]
    async fn the_series_toggles_read_their_query_parameters() {
        let (state, _store, _metrics) = build_state(None, None).await;
        let (status, body, _) = get(&state, "/dashboard?pool=0").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("class=\"legend-chip off\""));
        assert!(body.contains("aria-pressed=\"false\""));
        // The pool chip is off: its link turns it back on by omitting the
        // parameter, the node chip still flips node.
        assert!(body.contains("/dashboard?range=24h\">"));
        assert!(body.contains("/dashboard?range=24h&amp;pool=0&amp;node=0\""));
    }
}
