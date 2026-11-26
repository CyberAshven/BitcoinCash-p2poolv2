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
use crate::api::endpoints::share_address::{ShareAddressQuery, share_address_for};
use crate::api::error::ApiError;
use crate::api::server::AppState;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use bitcoin::{BlockHash, Target};
use chart::ChartBlock;
use history::{HashrateHistory, is_block, pool_hashrate};
use maud::Markup;
use p2poolv2_lib::accounting::stats::metrics::{PoolMetrics, SUBMISSION_WINDOW_SECS};
use p2poolv2_lib::address::bch_share::BchShareNetwork;
use p2poolv2_lib::address_display::ShareInfoDisplay;
use p2poolv2_lib::address_display::payout_address_string;
use p2poolv2_lib::config::BitcoinCashNetwork;
use pages::{
    BlockView, BlocksView, FoundBlockRow, MinerRow, MinerView, MinersView, Overview, PAGE_SIZE,
    Paging, PeersView, ShareRow, ShareView, SharesView, Site, WorkerRow, range_window_secs,
};
use serde::Deserialize;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Found blocks the overview lists; the blocks page has them all.
const RECENT_BLOCKS: usize = 5;

/// Confirmed share heights the overview's Recent shares panel lists; the
/// shares page has them all, [`pages::PAGE_SIZE`] per page.
const OVERVIEW_SHARES: usize = 5;

/// Rows the overview's Recent shares table always renders: the page is
/// truncated to this count so the panel keeps the same size on every
/// refresh, whatever uncles the window carries.
const OVERVIEW_ROWS: usize = 5;

/// Peers the overview's Peers panel lists; the peers page has them all.
const OVERVIEW_PEERS: usize = 5;

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
        .route("/dashboard/peers", get(peers))
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
    // The `--network` value for the help page's example command. Every
    // badge a Cash network produces is already a name `p2poolv2_cli
    // bch-share-address` accepts; a Bitcoin node's "bitcoin" badge is not,
    // so it has none and the page shows no generator at all.
    let share_cli_network = state
        .app_config
        .cash_network
        .map(BitcoinCashNetwork::from_share_network)
        .map(BitcoinCashNetwork::name);
    Site {
        network,
        badge: badge.to_string(),
        auth: state.auth_user.is_some(),
        stratum_port: state.stratum_port,
        sv2_port: state.sv2_port,
        share_cli_network,
    }
}

pub(crate) fn unix_now() -> u64 {
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
/// `?range=` chart window, `?chart=` chart tab and the series toggles
/// (`?pool=`/`?node=`/`?network`/`?blocks` for the hashrate tab,
/// `?height=`/`?dmin=`/`?dmax=`/`?uncles=`/`?weak=` for the sharechain,
/// `?miners=`/`?workers=` for the sharechain's opt-in count lines)
/// (a series is hidden when its parameter is `0`; the network series is
/// opt-in and shows only for `network=1`, as are miners and workers).
#[derive(Deserialize)]
struct PageQuery {
    page: Option<usize>,
    auto: Option<String>,
    range: Option<String>,
    chart: Option<String>,
    pool: Option<String>,
    node: Option<String>,
    network: Option<String>,
    blocks: Option<String>,
    height: Option<String>,
    dmin: Option<String>,
    dmax: Option<String>,
    uncles: Option<String>,
    weak: Option<String>,
    miners: Option<String>,
    workers: Option<String>,
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
/// shares, `pages::PAGE_SIZE` for the shares page. `row_limit` caps the
/// rows the page may render: the overview passes [`OVERVIEW_ROWS`] so its
/// table keeps the same size on every refresh, the shares page passes
/// `None` for the window's full detail. Truncation drops oldest content
/// only, so a share row still sits directly above its uncles.
fn share_page(
    state: &AppState,
    page: Option<usize>,
    window: usize,
    row_limit: Option<usize>,
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
            weak_count: display.weak_count,
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
                // An uncle never carries weak shares.
                weak_count: 0,
            });
        }
    }
    if let Some(limit) = row_limit {
        rows.truncate(limit);
    }
    (Some(tip), paging, rows)
}

/// Max shares the sharechain tab loads from the store: the range's samples
/// plus the trailing hour the difficulty / uncle / weak windows need, with
/// room for ~17 s intervals (the fixture chain) over a month.
const SHARE_SERIES_MAX: u32 = 3000;

/// One share as the sharechain tab needs it: time, height, difficulty,
/// the timestamps of its uncles (for counting uncle lines) and its weak
/// count. Sorted oldest first.
#[derive(Clone)]
pub(crate) struct ShareSeriesItem {
    pub(crate) time: u32,
    pub(crate) height: u32,
    pub(crate) difficulty: f64,
    pub(crate) uncle_times: Vec<u32>,
    pub(crate) weak_count: u16,
}

/// Shares behind the Sharechain tab, oldest first: pages backwards from the
/// tip through `query_shares` until the oldest share reaches back past the
/// given window plus the trailing hour the difficulty / uncle / weak windows
/// need (or the chain runs out / the max count fills).
pub(crate) fn share_series(state: &AppState, window_secs: u64) -> Vec<ShareSeriesItem> {
    let tip = match state.chain_store_handle.get_tip_height().ok().flatten() {
        Some(tip) => tip,
        None => return Vec::new(),
    };
    let store = state.chain_store_handle.store_handle().store();
    let mut collected: Vec<ShareSeriesItem> = Vec::with_capacity(200);
    // Page backwards: each loop steps down by up to PAGE_HEIGHT_STEP
    // heights, requesting shares through query_shares.
    const PAGE_HEIGHT_STEP: u32 = 100;
    let mut hi = tip;
    loop {
        let lo = hi.saturating_sub(PAGE_HEIGHT_STEP - 1);
        let shares = match store.query_shares(lo, hi) {
            Ok(shares) => shares,
            Err(_) => break,
        };
        if shares.is_empty() {
            break;
        }
        let newest_time = u64::from(shares.iter().map(|s| s.timestamp).max().unwrap_or(0));
        for share in &shares {
            collected.push(ShareSeriesItem {
                time: share.timestamp,
                height: share.height,
                difficulty: Target::from_compact(share.bits).difficulty_float(),
                uncle_times: share.uncles.iter().map(|u| u.timestamp).collect(),
                weak_count: share.weak_count,
            });
        }
        // Stop when the oldest collected share is at least window+3600s
        // before the newest one we have seen so far, or the count fills.
        if collected.len() as u32 >= SHARE_SERIES_MAX {
            break;
        }
        let oldest_in_batch = shares.iter().map(|s| s.timestamp).min().unwrap_or(0);
        if newest_time.saturating_sub(u64::from(oldest_in_batch)) > window_secs + 3600 {
            break;
        }
        // Step down from lo - 1 rather than the lowest height, since
        // query_shares includes both ends and we want the next slice.
        let lowest_height = shares.iter().map(|s| s.height).min().unwrap_or(0);
        if lowest_height == 0 {
            break;
        }
        hi = lowest_height.saturating_sub(1);
    }
    // Sort oldest first (by time, then height as tiebreaker).
    collected.sort_by_key(|s| (s.time, s.height));
    collected
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
            effort: block.effort,
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
    // The stratum-session rates the actor last published: each worker's own
    // figure fills its cell beside the accepted-work columns.
    let rates = state.metrics_handle.node_rates();
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
                    hashrate: rates
                        .workers
                        .get(&(address.clone(), name.clone()))
                        .map(|stat| stat.hashrate),
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
                hashrate: None,
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

/// The same two counts straight off the metrics actor's snapshot, for
/// callers that fetched it for other fields too (the hashrate sampler
/// and the `/hashrate` response): a miner is one with any worker
/// `active`, a worker is one `active`, exactly as [`online_counts`]
/// counts them off the miner rows.
pub(crate) fn online_counts_from_metrics(metrics: &PoolMetrics) -> (usize, usize) {
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
    (miners_online, workers_online)
}

/// Whether a miner on this node is the one looked up as `typed`, which the
/// window lookup spells `canonical` (a payout address may be typed without
/// its prefix).
pub(crate) fn is_looked_up_miner(miner: &MinerRow, typed: &str, canonical: Option<&str>) -> bool {
    miner.address == typed || canonical.is_some_and(|canonical| miner.address == canonical)
}

/// Page `page` of `miners`, each with its fraction of the next block's
/// payout. Only this page's miners are looked up, in the PPLNS window.
pub(crate) async fn miners_page(
    state: &Arc<AppState>,
    miners: Vec<MinerRow>,
    page: Option<usize>,
) -> (Vec<MinerRow>, Paging) {
    let paging = Paging::of(page, miners.len());
    let mut shown: Vec<MinerRow> = miners
        .into_iter()
        .skip((paging.page - 1) * PAGE_SIZE)
        .take(PAGE_SIZE)
        .collect();
    // The Hashrate column: each miner's own stratum-session figure, as
    // last published by the metrics actor. The read is synchronous, so it
    // comes before the payout lookups.
    let rates = state.metrics_handle.node_rates();
    for miner in &mut shown {
        miner.hashrate = rates.miners.get(&miner.address).map(|stat| stat.hashrate);
    }
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
    let chart_key = pages::normalize_chart(query.chart.as_deref());
    let pool_on = pages::series_on(query.pool.as_deref());
    let node_on = pages::series_on(query.node.as_deref());
    let network_on = pages::network_on(query.network.as_deref());
    let blocks_on = pages::series_on(query.blocks.as_deref());
    let height_on = pages::series_on(query.height.as_deref());
    let dmin_on = pages::series_on(query.dmin.as_deref());
    let dmax_on = pages::series_on(query.dmax.as_deref());
    let uncles_on = pages::series_on(query.uncles.as_deref());
    let weak_on = pages::series_on(query.weak.as_deref());
    let miners_on = pages::series_opt_in_on(query.miners.as_deref());
    let workers_on = pages::series_opt_in_on(query.workers.as_deref());
    let now = unix_now();
    let chain = read_store(state, move |state| {
        (
            share_page(state, None, OVERVIEW_SHARES, Some(OVERVIEW_ROWS)),
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
    let interval = history::block_interval_seconds(state);
    let network_hashrate = live_network_difficulty
        .map(|difficulty| history::network_hashrate(difficulty, interval))
        .or_else(|| samples.iter().rev().find_map(|sample| sample.network));
    let network_difficulty = live_network_difficulty.or_else(|| {
        network_hashrate.map(|rate| history::difficulty_from_network_hashrate(rate, interval))
    });
    // The metrics read comes before both local and pool, so the two cards
    // take theirs the same way: live first, then the newest non-null sample
    // (local only while that sample is inside the window it measures).
    let metrics = state.metrics_handle.get_metrics().await;
    // The local rate from the metrics actor's sliding window of accepted
    // submissions. The sample fallback keeps the card alive through the gap
    // before the first sample, but only for samples inside the same
    // submission window they measure: an older sample cannot say what the
    // rate is now, so a card whose miner stopped ages to a dash instead of
    // freezing on the last value it saw.
    let local_hashrate = metrics.local_submission_hashrate(now).or_else(|| {
        samples
            .iter()
            .rev()
            .filter(|sample| now.saturating_sub(sample.time) <= SUBMISSION_WINDOW_SECS)
            .find_map(|sample| sample.local)
    });
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
    // The sharechain tab's shares: oldest first, carrying time, height,
    // difficulty, uncle timestamps and weak count for the trailing-hour
    // envelope and counts. Only fetched when the sharechain tab is active.
    let share_series: Vec<ShareSeriesItem> = if chart_key == "sharechain" {
        read_store(state, move |state| {
            share_series(state, range_window_secs(range_key))
        })
        .await
        .unwrap_or_default()
    } else {
        Vec::new()
    };
    // The connected peers, longest uptime first (get_peer_infos sorts).
    // The panel falls back to its empty state when the node actor cannot
    // answer, rather than failing the whole page.
    let peers = match state.node_handle.get_peer_infos().await {
        Ok(peers) => peers,
        Err(error) => {
            tracing::warn!("Dashboard could not read connected peers: {error}");
            Vec::new()
        }
    };
    html(pages::overview(&Overview {
        site: site(state),
        now,
        tip_height,
        pool_hashrate,
        local_hashrate,
        network_hashrate,
        network_difficulty,
        share_difficulty: state.tracker_handle.get_pool_difficulty(),
        work_since_last_block: Some(metrics.work_since_last_block),
        miners_online,
        workers_online,
        accepted_shares_total: metrics.accepted_total,
        rejected_shares_total: metrics.rejected_total,
        samples,
        blocks_found,
        blocks: blocks.into_iter().take(RECENT_BLOCKS).collect(),
        shares,
        peers,
        auto,
        range_key,
        pool_on,
        node_on,
        network_on,
        blocks_on,
        chart_blocks,
        chart_key,
        height_on,
        dmin_on,
        dmax_on,
        uncles_on,
        weak_on,
        miners_on,
        workers_on,
        share_series,
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
    let (tip_height, paging, shares) = match read_store(state, move |state| {
        share_page(state, query.page, PAGE_SIZE, None)
    })
    .await
    {
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

async fn peers(State(dashboard): State<Arc<DashboardState>>) -> Response {
    let state = &dashboard.app;
    let peers = match state.node_handle.get_peer_infos().await {
        Ok(peers) => peers,
        Err(error) => {
            tracing::warn!("Dashboard could not read connected peers: {error}");
            Vec::new()
        }
    };
    html(pages::peers(&PeersView {
        site: site(state),
        peers,
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
    // The rate the report already carries is the same stratum-session
    // snapshot the miners page's column reads: one source, one number.
    let rate = (report.hashrate, report.recent_window);
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
        hashrate: rate.0,
        recent_window: rate.1,
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

/// The help page, with the share address generator's outcome when a
/// payout was submitted. No `?payout=` is a plain page; a submitted one
/// runs the endpoint's own rule, so the form and `GET /share_address`
/// can never disagree. On a refusal the payout is kept only to echo into
/// the form's input, and the example fields keep their placeholders.
async fn help(
    State(dashboard): State<Arc<DashboardState>>,
    headers: HeaderMap,
    Query(query): Query<ShareAddressQuery>,
) -> Response {
    let site = site(&dashboard.app);
    let host = request_host(&headers);
    let (payout, result, error) = match query.payout {
        None => (String::new(), String::new(), String::new()),
        Some(payout) => match share_address_for(&payout, dashboard.app.app_config.cash_network) {
            Ok(address) => (payout, address, String::new()),
            Err(ApiError::BadRequest(message)) => (payout, String::new(), message),
            Err(error) => (payout, String::new(), error.to_string()),
        },
    };
    html(pages::help(&site, &host, &payout, &result, &error))
}

/// The hostname the request's `Host` header names, without its port: the
/// host the operator browsed the dashboard with is the host a miner on the
/// same network reaches this node at. Empty when the header is absent
/// (HTTP/1.0) or is not valid text, so the help page falls back to its
/// placeholder. An IPv6 literal keeps the brackets its URL needs.
fn request_host(headers: &HeaderMap) -> String {
    let Some(host) = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
    else {
        return String::new();
    };
    if host.starts_with('[') {
        return match host.find(']') {
            Some(close) => host[..=close].to_string(),
            None => host.to_string(),
        };
    }
    host.split(':').next().unwrap_or_default().to_string()
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
