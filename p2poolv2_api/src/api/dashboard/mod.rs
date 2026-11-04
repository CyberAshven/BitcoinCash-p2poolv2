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
use chart::ChartBlock;
use history::{HashrateHistory, is_block, newest_shares, pool_hashrate, work_from_bits};
use maud::Markup;
use p2poolv2_lib::address::bch_share::BchShareNetwork;
use p2poolv2_lib::address_display::ShareInfoDisplay;
use p2poolv2_lib::address_display::payout_address_string;
use pages::{
    BlockView, BlocksView, FoundBlockRow, MinerRow, MinerView, MinersView, Overview, PAGE_SIZE,
    Paging, ShareRow, ShareView, SharesView, Site, WorkerRow, range_window_secs,
};
use serde::Deserialize;
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
/// `?range=` chart window and the `?pool=`/`?node=`/`?network`/`?blocks`
/// chart series toggles (a pool/node/blocks series is hidden when its
/// parameter is `0`; the network series shows only for `network=1`, it
/// being opt-in).
#[derive(Deserialize)]
struct PageQuery {
    page: Option<usize>,
    auto: Option<String>,
    range: Option<String>,
    pool: Option<String>,
    node: Option<String>,
    network: Option<String>,
    blocks: Option<String>,
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
    let network_on = pages::network_on(query.network.as_deref());
    let blocks_on = pages::series_on(query.blocks.as_deref());
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
    // The metrics read comes before both local and pool, so the two cards
    // take theirs the same way: live first, then the newest non-null sample.
    let metrics = state.metrics_handle.get_metrics().await;
    // The local rate from the metrics actor's sliding window of accepted
    // submissions (the pre-sample fallback keeps a value on the card).
    let local_hashrate = metrics
        .local_submission_hashrate(now)
        .or_else(|| samples.iter().rev().find_map(|sample| sample.local));
    // Harmonize with the pool KPI: fall back to the newest non-null sample
    // when the live recompute has too few shares to form a span, so both
    // cards show a value (or both show the dash) together.
    let pool_hashrate =
        pool_hashrate.or_else(|| samples.iter().rev().find_map(|sample| sample.pool));
    let blocks_found = blocks.len();
    // The chart's diamonds: found blocks inside the requested range, oldest
    // first. A block outside the sample history (a restart gap, or a page
    // opened right after one) is skipped by the chart rather than drawn
    // left of the axis.
    let window_start = now.saturating_sub(range_window_secs(range_key));
    let chart_blocks: Vec<ChartBlock> = blocks
        .iter()
        .filter(|block| u64::from(block.time) >= window_start)
        .map(|block| ChartBlock {
            height: block.height,
            time: block.time,
        })
        .collect();
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
        network_on,
        blocks_on,
        chart_blocks,
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
mod tests;
