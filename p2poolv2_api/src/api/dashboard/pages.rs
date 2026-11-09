// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The dashboard's pages, rendered to HTML in Rust.
//!
//! The layout and stylesheet are CashDragon0's design from PR #8, rebuilt
//! here without a script: every page has the same menu bar, a search box and
//! a trail back to the overview, its pop-ups are pages of their own, long
//! tables are split into pages with links, and a miner's workers open with
//! an HTML `<details>`. The overview reloads every 30 seconds unless `?auto=off`
//! pauses it, and chart tooltips are hit zones with styled tips plus SVG titles.
//! Every value is escaped by maud.

use super::assets;
use super::chart::{ChartBlock, Series, line_chart};
use super::history::Sample;
use crate::api::endpoints::miner::MinerResponse;
use crate::api::endpoints::share::ShareLookupOutput;
use bitcoin::BlockHash;
use maud::{DOCTYPE, Markup, html};
use serde::Serialize;

/// Seconds between refreshes of the overview page while auto is on.
pub(crate) const OVERVIEW_REFRESH_SECONDS: u32 = 30;

/// Hashrate chart time ranges, keyed by the `?range=` query value.
/// `(query key, button label, newest samples kept, period word for titles)`.
/// The node reads its full 30-day ring and count-slices it, so no window
/// seconds are needed here (unlike the replica, which fetches `/hashrate`).
const RANGES: [(&str, &str, usize, u64, &str); 4] = [
    ("1h", "Hour", 60, 3600, "hour"),
    ("24h", "Day", 1440, 86400, "day"),
    ("1w", "Week", 10080, 604800, "week"),
    ("1m", "Month", 43200, 2592000, "month"),
];

/// The `?range=` key's window in seconds: found blocks at least this old
/// fall outside the chart's window and are not drawn on it.
pub(crate) fn range_window_secs(range_key: &str) -> u64 {
    RANGES
        .iter()
        .find(|(key, ..)| *key == range_key)
        .map_or_else(|| RANGES[1].3, |(_, _, _, window_secs, _)| *window_secs)
}

/// The range shown when `?range=` is missing or names no known range.
const DEFAULT_RANGE: &str = "24h";

/// `value` if it names a range in [`RANGES`], otherwise [`DEFAULT_RANGE`].
/// Returns a key from the static table, so the lifetime is `'static`.
pub(crate) fn normalize_range(value: Option<&str>) -> &'static str {
    RANGES
        .iter()
        .find(|(key, ..)| Some(*key) == value)
        .map(|(key, ..)| *key)
        .unwrap_or(DEFAULT_RANGE)
}

/// The overview's stateless auto-refresh toggle, read from `?auto=`:
/// `?auto=on` (the default) or `?auto=off`. On page 1 with `Auto::On` the
/// overview's head carries a 30 second meta refresh; the header shows the
/// matching button, or a hidden same-width slot when this page is not the
/// overview.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Auto {
    On,
    Off,
}

impl Auto {
    /// A missing `?auto=` parameter or any value other than `off` selects
    /// `Auto::On`; only `?auto=off` selects `Auto::Off`.
    pub(crate) fn from_query(auto: Option<&str>) -> Auto {
        match auto {
            Some("off") => Auto::Off,
            _ => Auto::On,
        }
    }
}

/// Rows on one page of a long table.
pub(crate) const PAGE_SIZE: usize = 25;

/// A chart series toggle from its query parameter: only `?pool=0`/`?node=0`
/// hides the series, anything else (including a missing parameter) shows it.
pub(crate) fn series_on(value: Option<&str>) -> bool {
    value != Some("0")
}

/// The network series toggle: unlike Pool and Node, the network line is
/// opt-in, so it shows only for `?network=1` and a missing (or any other)
/// parameter keeps the default URL clean.
pub(crate) fn network_on(value: Option<&str>) -> bool {
    value == Some("1")
}

/// The query fragment carrying the four chart series toggles: pool, node
/// and the found-block diamonds carry their turns off, network carries its
/// turn on (it is opt-in), so `""` is the default state. The links that
/// must survive a click (range buttons, the auto-refresh toggle) append it
/// to keep the state.
pub(crate) fn series_q(pool_on: bool, node_on: bool, network_on: bool, blocks_on: bool) -> String {
    let mut query = String::new();
    if !pool_on {
        query.push_str("&pool=0");
    }
    if !node_on {
        query.push_str("&node=0");
    }
    if network_on {
        query.push_str("&network=1");
    }
    if !blocks_on {
        query.push_str("&blocks=0");
    }
    query
}

/// What every page shows about the node it is served by. `/network` serves
/// it as JSON for frontends such as `p2poolv2_web`.
#[derive(Serialize)]
pub(crate) struct Site {
    /// The chain this node mines, for example "Bitcoin Cash chipnet".
    pub(crate) network: String,
    /// The short name in the header badge, for example "chipnet".
    pub(crate) badge: String,
    /// Whether the node has an API login, so the header offers the Admin
    /// button that leads to `/dashboard/signin`.
    pub(crate) auth: bool,
}

/// The menu bar's pages.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tab {
    Dashboard,
    Blocks,
    Miners,
    Help,
    /// A page that belongs to no tab, such as an error.
    None,
}

const TABS: [(Tab, &str, &str); 4] = [
    (Tab::Dashboard, "Dashboard", "/dashboard"),
    (Tab::Blocks, "Blocks", "/dashboard/blocks"),
    (Tab::Miners, "Miners", "/dashboard/miners"),
    (Tab::Help, "Help", "/dashboard/help"),
];

/// One step of the trail at the top of a page. The last step is the page
/// itself and has no link.
pub(crate) struct Crumb {
    pub(crate) label: String,
    pub(crate) href: Option<String>,
}

impl Crumb {
    fn link(label: impl Into<String>, href: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            href: Some(href.into()),
        }
    }

    fn here(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            href: None,
        }
    }
}

/// Which page of a long table is shown, counted from 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Paging {
    pub(crate) page: usize,
    pub(crate) pages: usize,
}

impl Paging {
    /// The page `requested` of `rows` rows, clamped to the pages there are.
    pub(crate) fn of(requested: Option<usize>, rows: usize) -> Self {
        let pages = rows.div_ceil(PAGE_SIZE).max(1);
        let page = requested.unwrap_or(1).clamp(1, pages);
        Self { page, pages }
    }

    /// The rows of `all` on this page.
    pub(crate) fn slice<'a, T>(&self, all: &'a [T]) -> &'a [T] {
        let start = ((self.page - 1) * PAGE_SIZE).min(all.len());
        let end = (start + PAGE_SIZE).min(all.len());
        &all[start..end]
    }
}

/// A confirmed share or one of its uncles, as the overview lists it.
pub(crate) struct ShareRow {
    pub(crate) height: Option<u32>,
    pub(crate) blockhash: String,
    pub(crate) time: u32,
    pub(crate) payout_address: String,
    pub(crate) share_address: String,
    /// `None` for an uncle, whose bits the list does not carry.
    pub(crate) difficulty: Option<f64>,
    pub(crate) is_uncle: bool,
    pub(crate) is_block: bool,
}

/// A block the pool found.
pub(crate) struct FoundBlockRow {
    /// Bitcoin Cash height.
    pub(crate) height: u64,
    pub(crate) block_hash: BlockHash,
    /// The share whose header carries the block.
    pub(crate) share_hash: BlockHash,
    /// Unix seconds of the block header.
    pub(crate) time: u32,
    /// Payout address of the miner that found it.
    pub(crate) payout_address: String,
}

/// Everything the overview shows.
pub(crate) struct Overview {
    pub(crate) site: Site,
    pub(crate) now: u64,
    pub(crate) tip_height: Option<u32>,
    pub(crate) pool_hashrate: Option<f64>,
    pub(crate) local_hashrate: Option<f64>,
    pub(crate) network_hashrate: Option<f64>,
    pub(crate) network_difficulty: Option<f64>,
    /// Current pool share difficulty (the newest prepared notify's ASERT
    /// target, in the share chain's units), read live from the job tracker.
    pub(crate) share_difficulty: Option<f64>,
    pub(crate) miners_online: usize,
    pub(crate) workers_online: usize,
    /// Shares the pool accepted since the node started, node-wide.
    pub(crate) accepted_shares_total: u64,
    /// Shares the pool rejected since the node started, node-wide.
    pub(crate) rejected_shares_total: u64,
    pub(crate) samples: Vec<Sample>,
    /// Blocks the pool found in all.
    pub(crate) blocks_found: usize,
    /// The newest of them, newest first.
    pub(crate) blocks: Vec<FoundBlockRow>,
    /// The newest confirmed shares, newest first, uncles under the share that
    /// includes them, at most `super::OVERVIEW_SHARES` heights.
    pub(crate) shares: Vec<ShareRow>,
    /// The overview's stateless auto-refresh toggle, from `?auto=`.
    pub(crate) auto: Auto,
    /// The normalized `?range=` key selecting the chart window, from
    /// [`normalize_range`].
    pub(crate) range_key: &'static str,
    /// Whether the pool line shows, from `?pool=0`.
    pub(crate) pool_on: bool,
    /// Whether this node's line shows, from `?node=0`.
    pub(crate) node_on: bool,
    /// Whether the network line shows, from `?network=1` (opt-in).
    pub(crate) network_on: bool,
    /// Whether the found-block diamonds show, from `?blocks=0`.
    pub(crate) blocks_on: bool,
    /// The pool's found blocks inside the chart's window, for the chart's
    /// diamonds and Block tooltip rows.
    pub(crate) chart_blocks: Vec<ChartBlock>,
}

/// The found blocks page.
pub(crate) struct BlocksView {
    pub(crate) site: Site,
    pub(crate) now: u64,
    pub(crate) blocks_found: usize,
    pub(crate) blocks: Vec<FoundBlockRow>,
    pub(crate) paging: Paging,
}

/// The shares page: one page of the whole confirmed share chain.
pub(crate) struct SharesView {
    pub(crate) site: Site,
    pub(crate) now: u64,
    pub(crate) tip_height: Option<u32>,
    /// This page of the share chain, newest first.
    pub(crate) shares: Vec<ShareRow>,
    pub(crate) paging: Paging,
}

/// One found block's page.
pub(crate) struct BlockView {
    pub(crate) site: Site,
    pub(crate) now: u64,
    pub(crate) block: FoundBlockRow,
}

/// A worker of a miner on this node. `/miners` serves the same rows as JSON.
#[derive(Serialize)]
pub(crate) struct WorkerRow {
    pub(crate) name: String,
    pub(crate) active: bool,
    /// Sum of the difficulty of its accepted shares.
    pub(crate) accepted_work: u64,
    pub(crate) best_share: u64,
    pub(crate) last_share_at: u64,
}

/// A miner on this node, for the miners page. `/miners` serves the same rows
/// as JSON.
#[derive(Serialize)]
pub(crate) struct MinerRow {
    pub(crate) address: String,
    pub(crate) workers: Vec<WorkerRow>,
    pub(crate) accepted_work: u64,
    pub(crate) best_share: u64,
    pub(crate) last_share_at: u64,
    /// Its fraction of the next block's PPLNS payout, when the window could
    /// be read for it.
    pub(crate) payout_fraction: Option<f64>,
}

/// The miners page.
pub(crate) struct MinersView {
    pub(crate) site: Site,
    pub(crate) now: u64,
    pub(crate) miners: Vec<MinerRow>,
    pub(crate) paging: Paging,
}

/// Everything a miner's page shows.
pub(crate) struct MinerView {
    pub(crate) site: Site,
    pub(crate) now: u64,
    pub(crate) report: MinerResponse,
    /// The miner's hashrate over the recent shares, if it has any there.
    pub(crate) hashrate: Option<f64>,
    /// How many recent shares, and over how many seconds, it was taken from.
    pub(crate) recent_window: Option<(usize, u32)>,
    /// Its workers, when it mines on this node.
    pub(crate) workers: Vec<WorkerRow>,
    pub(crate) paging: Paging,
}

/// Everything a share's page shows.
pub(crate) struct ShareView {
    pub(crate) site: Site,
    pub(crate) share: ShareLookupOutput,
    pub(crate) is_block: bool,
}

const UNITS: [&str; 7] = ["", "k", "M", "G", "T", "P", "E"];

/// `value` scaled to a unit prefix, three significant digits.
fn with_prefix(value: f64) -> (f64, &'static str) {
    let mut scaled = if value == 0.0 { 0.0 } else { value };
    let mut unit = 0;
    while scaled.abs() >= 1000.0 && unit + 1 < UNITS.len() {
        scaled /= 1000.0;
        unit += 1;
    }
    (scaled, UNITS[unit])
}

/// Three significant digits, never printing a negative zero.
fn three_digits(value: f64) -> String {
    let value = if value < 0.0 && value.abs() < 0.005 {
        0.0
    } else {
        value
    };
    if value.abs() >= 100.0 {
        format!("{value:.0}")
    } else if value.abs() >= 10.0 {
        format!("{value:.1}")
    } else {
        format!("{value:.2}")
    }
}

pub(crate) fn fmt_hashrate(hashes_per_second: f64) -> String {
    let (value, unit) = with_prefix(hashes_per_second);
    format!("{} {unit}H/s", three_digits(value))
}

pub(crate) fn fmt_difficulty(difficulty: f64) -> String {
    let (value, unit) = with_prefix(difficulty);
    format!("{}{unit}", three_digits(value))
        .trim_end()
        .to_string()
}

pub(crate) fn fmt_utc(unix: u64) -> String {
    i64::try_from(unix)
        .ok()
        .and_then(|seconds| chrono::DateTime::from_timestamp(seconds, 0))
        .map(|time| time.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|| unix.to_string())
}

/// Day and time, short enough for a chart axis that spans a day.
pub(crate) fn fmt_day_time(unix: u64) -> String {
    i64::try_from(unix)
        .ok()
        .and_then(|seconds| chrono::DateTime::from_timestamp(seconds, 0))
        .map(|time| time.format("%b %-d %H:%M UTC").to_string())
        .unwrap_or_else(|| unix.to_string())
}

/// Time only, no date: the x-axis edge labels of a range-sliced chart.
pub(crate) fn fmt_hour_edge(unix: u64) -> String {
    i64::try_from(unix)
        .ok()
        .and_then(|seconds| chrono::DateTime::from_timestamp(seconds, 0))
        .map(|time| time.format("%H:%M").to_string())
        .unwrap_or_else(|| unix.to_string())
}

/// Intermediate x-axis labels: hour of day, with minutes when the tick step
/// is finer than an hour (the 1h view uses 10-15 min ticks).
pub(crate) fn fmt_hour_tick(unix: u64) -> String {
    fmt_hour_edge(unix)
}

pub(crate) fn fmt_ago(now: u64, then: u64) -> String {
    let seconds = now.saturating_sub(then);
    match seconds {
        0..=59 => format!("{seconds} s ago"),
        60..=3_599 => format!("{} min ago", seconds / 60),
        3_600..=86_399 => format!("{} h ago", seconds / 3_600),
        _ => format!("{} d ago", seconds / 86_400),
    }
}

/// A long hash or address cut to its ends.
pub(crate) fn short(hash: &str) -> String {
    let length = hash.chars().count();
    if length <= 20 {
        return hash.to_string();
    }
    let head: String = hash.chars().take(10).collect();
    let tail: String = hash.chars().skip(length - 6).collect();
    format!("{head}…{tail}")
}

pub(crate) fn optional(value: Option<f64>, format: fn(f64) -> String) -> String {
    value.map(format).unwrap_or_else(|| "—".to_string())
}

pub(crate) fn percent(fraction: f64) -> String {
    format!("{:.2}%", fraction * 100.0)
}

/// Percent-encode anything outside the unreserved set, for a query or path
/// component. Addresses and hashes are ASCII, but the escape keeps a
/// strange value from changing the URL's structure.
pub(crate) fn url_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~' | b':') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

pub(crate) fn miner_href(address: &str) -> String {
    format!("/dashboard/miner?address={}", url_component(address))
}

pub(crate) fn share_href(blockhash: &str) -> String {
    format!("/dashboard/share/{}", url_component(blockhash))
}

pub(crate) fn block_href(block_hash: &str) -> String {
    format!("/dashboard/block/{}", url_component(block_hash))
}

fn miner_link(address: &str) -> Markup {
    html! { a.addr href=(miner_href(address)) title=(address) { (short(address)) } }
}

fn share_link(blockhash: &str) -> Markup {
    html! { a.addr href=(share_href(blockhash)) title=(blockhash) { (short(blockhash)) } }
}

/// When a miner or worker last sent a share; one that has not sent any
/// yet was never seen.
fn last_seen(now: u64, then: u64) -> Markup {
    if then == 0 {
        return html! { span { "never" } };
    }
    html! { span title=(fmt_utc(then)) { (fmt_ago(now, then)) } }
}

fn ago_cell(now: u64, then: u64) -> Markup {
    html! { td title=(fmt_utc(then)) { (fmt_ago(now, then)) } }
}

/// A KPI card: a label, a value and notes under it.
fn kpi_card(
    label: &str,
    value: String,
    value_class: &str,
    href: Option<&str>,
    notes: &[String],
) -> Markup {
    let value = html! { p class=(format!("kpi-value {value_class}")) { (value) } };
    html! {
        div.kpi-card {
            p.kpi-label { (label) }
            @if let Some(href) = href { a href=(href) { (value) } } @else { (value) }
            @for note in notes { p.kpi-sub { (note) } }
        }
    }
}

/// One item of a details list.
fn stat(key: &str, value: Markup) -> Markup {
    html! { div.stat-item { div.k { (key) } div.v { (value) } } }
}

/// The hashrate chart's Hour/Day/Week/Month buttons, plain links with no
/// script. The active range carries `active-nav` and `aria-current`. When
/// auto-refresh is off the links keep `?auto=off`, and `series_arg` keeps the
/// chart series toggles.
fn range_buttons(range_key: &str, auto: Auto, series_arg: &str) -> Markup {
    let auto_query = if matches!(auto, Auto::Off) {
        "&auto=off"
    } else {
        ""
    };
    html! {
        span.range-group aria-label="Chart time range" {
            @for (key, label, _, _, _) in RANGES {
                a.outline.header-btn.active-nav[key == range_key] role="button"
                    href=(format!("/dashboard?range={key}{auto_query}{series_arg}"))
                    aria-current=[(key == range_key).then_some("true")] { (label) }
            }
        }
    }
}

/// The Pool/Node/Network/Blocks chart series toggles: their own row under
/// the hashrate heading, stateless links that flip `?pool=0`/`?node=0`/
/// `?network=1`/`?blocks=0` while keeping the range and auto-refresh
/// parameters. A hidden series shows `off`; the dot carries the line's
/// colour (the blocks dot wears the marker's diamond shape).
fn series_toggles(
    pool_on: bool,
    node_on: bool,
    network_on: bool,
    blocks_on: bool,
    range_key: &str,
    auto: Auto,
) -> Markup {
    let auto_query = if matches!(auto, Auto::Off) {
        "&auto=off"
    } else {
        ""
    };
    html! {
        span.series-toggles role="group" aria-label="Chart series" {
            a.legend-chip.off[!pool_on] role="button" aria-pressed=(pool_on.to_string())
                href=(format!("/dashboard?range={range_key}{auto_query}{}",
                    series_q(!pool_on, node_on, network_on, blocks_on))) {
                span.legend-dot.pool {} "Pool"
            }
            a.legend-chip.off[!node_on] role="button" aria-pressed=(node_on.to_string())
                href=(format!("/dashboard?range={range_key}{auto_query}{}",
                    series_q(pool_on, !node_on, network_on, blocks_on))) {
                span.legend-dot.local {} "Node"
            }
            a.legend-chip.off[!network_on] role="button" aria-pressed=(network_on.to_string())
                href=(format!("/dashboard?range={range_key}{auto_query}{}",
                    series_q(pool_on, node_on, !network_on, blocks_on))) {
                span.legend-dot.network {} "Network"
            }
            a.legend-chip.off[!blocks_on] role="button" aria-pressed=(blocks_on.to_string())
                href=(format!("/dashboard?range={range_key}{auto_query}{}",
                    series_q(pool_on, node_on, network_on, !blocks_on))) {
                span.legend-dot.blocks {} "Blocks"
            }
        }
    }
}

/// Previous and next links for a long table. `base` is the page's URL
/// without its `page` parameter.
pub(crate) fn pager(base: &str, paging: Paging) -> Markup {
    if paging.pages <= 1 {
        return html! {};
    }
    let separator = if base.contains('?') { '&' } else { '?' };
    let href = |page: usize| format!("{base}{separator}page={page}");
    let has_previous = paging.page > 1;
    let has_next = paging.page < paging.pages;
    html! {
        nav.more-row aria-label="Pages" {
            a.outline.more-btn role="button" href=(href(paging.page.saturating_sub(1).max(1)))
                aria-disabled=[(!has_previous).then_some("true")] rel="prev" { "Previous" }
            span.page-of { "Page " (paging.page) " of " (paging.pages) }
            a.outline.more-btn role="button" href=(href((paging.page + 1).min(paging.pages)))
                aria-disabled=[(!has_next).then_some("true")] rel="next" { "Next" }
        }
    }
}

/// The page frame: head, menu bar, search, trail and footer around `body`.
/// `series_arg` is [`series_q`] for the overview, so the auto-refresh links
/// keep the chart series toggles; empty elsewhere.
#[allow(clippy::too_many_arguments)]
pub(crate) fn layout(
    site: &Site,
    title: &str,
    tab: Tab,
    trail: &[Crumb],
    refresh: Option<u32>,
    auto: Option<Auto>,
    range_key: Option<&str>,
    series_arg: &str,
    body: Markup,
) -> Markup {
    let range_query = range_key.map_or(String::new(), |key| format!("&range={key}"));
    html! {
        (DOCTYPE)
        html lang="en" data-theme="dark" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                @if let Some(seconds) = refresh {
                    meta http-equiv="refresh" content=(seconds);
                }
                title { (title) " · P2Pool v2 · " (site.badge) }
                link rel="icon" type="image/svg+xml" href=(assets::href("logo-circle.svg"));
                link rel="stylesheet" href=(assets::href("pico.min.css"));
                link rel="stylesheet" href=(assets::href("dashboard.css"));
            }
            body {
                header.site-header {
                    nav {
                        ul.brand {
                            li {
                                a href="/dashboard" {
                                    img src=(assets::href("logo-horizontal-wt.svg")) alt="Bitcoin Cash";
                                }
                                span.brand-meta {
                                    a.wordmark-link href="/dashboard" {
                                        span.wordmark { "P2Pool" span.accent { "v2" } }
                                    }
                                    span.chain-badge title=(site.network) { (site.badge) }
                                }
                            }
                        }
                        ul.tabs {
                            @for (this, label, href) in TABS {
                                li {
                                    a.outline.header-btn.active-nav[this == tab] role="button" href=(href)
                                        aria-current=[(this == tab).then_some("page")] { (label) }
                                }
                            }
                        }
                        ul.header-right {
                            @match auto {
                                Some(Auto::On) => {
                                    li {
                                        a.outline.header-btn.refresh-main.auto-on role="button"
                                            href=(format!("/dashboard?auto=off{range_query}{series_arg}"))
                                            title="Turn off 30s auto-refresh" { "Auto on" }
                                    }
                                }
                                Some(Auto::Off) => {
                                    li {
                                        a.outline.header-btn.refresh-main.auto-paused role="button"
                                            href=(format!("/dashboard?auto=on{range_query}{series_arg}"))
                                            title="Turn on 30s auto-refresh" { "Auto off" }
                                    }
                                }
                                None => {
                                    li.auto-slot aria-hidden="true" {}
                                }
                            }
                            li {
                                form.header-search action="/dashboard/search" method="get" role="search" {
                                    input type="search" name="q" placeholder="Miner address or share hash"
                                        aria-label="Miner address or share hash" required;
                                    button.outline.header-btn type="submit" { "Search" }
                                }
                            }
                            @if site.auth {
                                li {
                                    a.outline.header-btn role="button" href="/dashboard/signin"
                                        title="Sign in to the node's own routes" { "Admin" }
                                }
                            }
                        }
                    }
                }
                main.container {
                    @if !trail.is_empty() {
                        nav aria-label="breadcrumb" {
                            ul {
                                @for crumb in trail {
                                    li {
                                        @if let Some(href) = &crumb.href {
                                            a href=(href) { (crumb.label) }
                                        } @else {
                                            span aria-current="page" { (crumb.label) }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    (body)
                }
                footer.site-footer {
                    div.inner {
                        span { "P2Pool v2 · " (site.network) }
                        span { "Rendered by this node in Rust. No scripts, no tracking." }
                    }
                }
            }
        }
    }
}

fn dashboard_crumb() -> Crumb {
    Crumb::link("Dashboard", "/dashboard")
}

/// A plain form that opens a miner's page, for the miner page and errors.
fn lookup_form(value: &str) -> Markup {
    html! {
        form.search-row action="/dashboard/miner" method="get" {
            div.grow {
                label for="miner-address" { "Payout or share chain address" }
                input #miner-address type="text" name="address" value=(value)
                    placeholder="bchtest:… or tp2pool:…" required;
            }
            button.btn-primary type="submit" { "Look up" }
        }
    }
}

fn series(samples: &[Sample], pick: fn(&Sample) -> Option<f64>) -> Vec<(u64, f64)> {
    samples
        .iter()
        .filter_map(|sample| pick(sample).map(|value| (sample.time, value)))
        .collect()
}

fn blocks_table(now: u64, blocks: &[FoundBlockRow]) -> Markup {
    html! {
        div.table-x {
            table.table {
                thead { tr { th { "Height" } th { "Block hash" } th { "Share" } th { "Found by" } th { "Found" } } }
                tbody {
                    @for block in blocks {
                        @let block_hash = block.block_hash.to_string();
                        tr {
                            td { a href=(block_href(&block_hash)) { (block.height) } }
                            td { a.addr href=(block_href(&block_hash)) title=(block_hash) { (short(&block_hash)) } }
                            td { (share_link(&block.share_hash.to_string())) }
                            td { (miner_link(&block.payout_address)) }
                            (ago_cell(now, u64::from(block.time)))
                        }
                    }
                }
            }
        }
    }
}

pub(crate) fn overview(view: &Overview) -> Markup {
    let (_, _, max_samples, _, period) = *RANGES
        .iter()
        .find(|(key, ..)| *key == view.range_key)
        .unwrap_or(&RANGES[1]);
    let samples: Vec<Sample> = view
        .samples
        .iter()
        .rev()
        .take(max_samples)
        .rev()
        .copied()
        .collect();
    let mut shown = Vec::with_capacity(3);
    if view.pool_on {
        shown.push(Series {
            label: "Pool",
            class: "pool",
            points: series(&samples, |sample| sample.pool),
        });
    }
    if view.node_on {
        shown.push(Series {
            label: "This node",
            class: "local",
            points: series(&samples, |sample| sample.local),
        });
    }
    if view.network_on {
        shown.push(Series {
            label: "Network",
            class: "network",
            points: series(&samples, |sample| sample.network),
        });
    }
    // All toggles off: a note about the toggles, not the "first points"
    // note the chart shows before any sample arrives.
    let chart = if shown.is_empty() {
        html! {
            p.empty { "All series hidden. Click a legend chip to show one." }
        }
    } else {
        // The chip's turn-off reaches the chart as an empty list: no
        // diamonds and no Block rows, as the replica does for `?blocks=0`.
        let chart_blocks: &[ChartBlock] = if view.blocks_on {
            &view.chart_blocks
        } else {
            &[]
        };
        line_chart(
            &format!("Hashrate over the last {period}"),
            &shown,
            chart_blocks,
            fmt_hashrate,
            fmt_day_time,
            fmt_hour_tick,
            fmt_hour_edge,
        )
    };
    let series_arg = series_q(view.pool_on, view.node_on, view.network_on, view.blocks_on);
    let last_block = view.blocks.first().map(|block| block.time);
    let body = html! {
        div.kpi-grid {
            (kpi_card("Pool hashrate", optional(view.pool_hashrate, fmt_hashrate), "kpi-value-green", None,
                &["Confirmed shares, last hour".to_string()]))
            (kpi_card("Node hashrate", optional(view.local_hashrate, fmt_hashrate), "kpi-value-local", Some("/dashboard/miners"),
                &[format!("{} miners, {} workers online", view.miners_online, view.workers_online),
                  format!("{} rejected of {} shares", view.rejected_shares_total, view.accepted_shares_total)]))
            (kpi_card("network hashrate", optional(view.network_hashrate, fmt_hashrate), "kpi-value-net", None,
                &[format!("Difficulty {}", optional(view.network_difficulty, fmt_difficulty)),
                  "Est. from diff. 10-min blocks".to_string()]))
            (kpi_card("Share chain height",
                view.tip_height.map(|height| height.to_string()).unwrap_or_else(|| "—".to_string()), "", None,
                &[format!("Difficulty {}", optional(view.share_difficulty, fmt_difficulty))]))
            (kpi_card("Blocks found", view.blocks_found.to_string(), "kpi-value-green", Some("/dashboard/blocks"),
                &[last_block
                    .map(|time| format!("Last {}", fmt_ago(view.now, u64::from(time))))
                    .unwrap_or_else(|| "None yet".to_string())]))
        }
        div.panel {
            h2.hashrate-head { "Hashrate" (range_buttons(view.range_key, view.auto, &series_arg)) }
            (series_toggles(view.pool_on, view.node_on, view.network_on, view.blocks_on, view.range_key, view.auto))
            div.chart-wrap { (chart) }
        }
        div.panel {
            h2 { "Found blocks" span.panel-note { (view.blocks_found) " in all" } }
            @if view.blocks.is_empty() {
                p.empty-state { "The pool has not found a Bitcoin Cash block yet. Keep mining." }
            } @else {
                (blocks_table(view.now, &view.blocks))
                div.panel-more { a href="/dashboard/blocks" { "All found blocks →" } }
            }
        }
        div.panel {
            h2 {
                "Recent shares"
                span.panel-note {
                    "newest first, uncles under the share that includes them"
                    @if let Some(tip) = view.tip_height { " · tip " (tip) }
                }
            }
            @if view.shares.is_empty() {
                p.empty-state {
                    @if view.tip_height.is_none() { "No confirmed shares yet. Point a miner at this node's stratum port to get started." }
                    @else { "No shares on this page." }
                }
            } @else {
                div.table-wrap {
                    table.table {
                        thead { tr { th { "Height" } th { "Share hash" } th { "Type" } th { "Miner (payout)" } th { "Share owner" } th { "Time" } th { "Difficulty" } } }
                        tbody {
                            @for row in &view.shares {
                                tr.uncle-row[row.is_uncle] {
                                    td { (row.height.map(|height| height.to_string()).unwrap_or_else(|| "—".to_string())) }
                                    td { (share_link(&row.blockhash)) }
                                    td {
                                        @if row.is_uncle { "uncle" } @else { "share" }
                                        @if row.is_block { " " span.badge-block { "block" } }
                                    }
                                    td { (miner_link(&row.payout_address)) }
                                    td { (miner_link(&row.share_address)) }
                                    (ago_cell(view.now, u64::from(row.time)))
                                    td.mono { (optional(row.difficulty, fmt_difficulty)) }
                                }
                            }
                        }
                    }
                }
                div.panel-more { a href="/dashboard/shares" { "More found shares →" } }
            }
        }
    };
    let refresh = matches!(view.auto, Auto::On).then_some(OVERVIEW_REFRESH_SECONDS);
    layout(
        &view.site,
        "Dashboard",
        Tab::Dashboard,
        &[],
        refresh,
        Some(view.auto),
        Some(view.range_key),
        &series_arg,
        body,
    )
}

pub(crate) fn blocks(view: &BlocksView) -> Markup {
    let body = html! {
        div.panel {
            h2 { "Found blocks" span.panel-note { (view.blocks_found) " in all, newest first" } }
            @if view.blocks.is_empty() {
                p.empty-state { "The pool has not found a Bitcoin Cash block yet. Keep mining." }
            } @else {
                (blocks_table(view.now, &view.blocks))
                (pager("/dashboard/blocks", view.paging))
            }
        }
    };
    layout(
        &view.site,
        "Blocks",
        Tab::Blocks,
        &[dashboard_crumb(), Crumb::here("Blocks")],
        None,
        None,
        None,
        "",
        body,
    )
}

pub(crate) fn shares(view: &SharesView) -> Markup {
    let body = html! {
        div.panel {
            h2 {
                "Recent shares"
                span.panel-note {
                    "newest first, uncles under the share that includes them"
                    @if let Some(tip) = view.tip_height { " · tip " (tip) }
                }
            }
            @if view.shares.is_empty() {
                p.empty-state {
                    @if view.tip_height.is_none() { "No confirmed shares yet. Point a miner at this node's stratum port to get started." }
                    @else { "No shares on this page." }
                }
            } @else {
                div.table-wrap {
                    table.table {
                        thead { tr { th { "Height" } th { "Share hash" } th { "Type" } th { "Miner (payout)" } th { "Share owner" } th { "Time" } th { "Difficulty" } } }
                        tbody {
                            @for row in &view.shares {
                                tr.uncle-row[row.is_uncle] {
                                    td { (row.height.map(|height| height.to_string()).unwrap_or_else(|| "—".to_string())) }
                                    td { (share_link(&row.blockhash)) }
                                    td {
                                        @if row.is_uncle { "uncle" } @else { "share" }
                                        @if row.is_block { " " span.badge-block { "block" } }
                                    }
                                    td { (miner_link(&row.payout_address)) }
                                    td { (miner_link(&row.share_address)) }
                                    (ago_cell(view.now, u64::from(row.time)))
                                    td.mono { (optional(row.difficulty, fmt_difficulty)) }
                                }
                            }
                        }
                    }
                }
                (pager("/dashboard/shares", view.paging))
            }
        }
    };
    layout(
        &view.site,
        "Shares",
        Tab::None,
        &[dashboard_crumb(), Crumb::here("Shares")],
        None,
        None,
        None,
        "",
        body,
    )
}

pub(crate) fn block(view: &BlockView) -> Markup {
    let block = &view.block;
    let block_hash = block.block_hash.to_string();
    let share_hash = block.share_hash.to_string();
    let body = html! {
        a.back-link href="/dashboard/blocks" { "← All found blocks" }
        div.panel {
            h2 { "Block " (block.height) span.panel-note { "found by the pool" } }
            div.stat-list {
                (stat("Height", html! { (block.height) }))
                (stat("Found", html! { (fmt_ago(view.now, u64::from(block.time))) }))
                (stat("Time", html! { (fmt_utc(u64::from(block.time))) }))
                (stat("Found by", miner_link(&block.payout_address)))
            }
        }
        div.panel {
            h2 { "Hashes" }
            div.stat-list {
                (stat("Block hash", html! { span.mono { (block_hash) } }))
                (stat("Share carrying it", html! { a.addr href=(share_href(&share_hash)) { (share_hash) } }))
            }
        }
    };
    layout(
        &view.site,
        &format!("Block {}", block.height),
        Tab::Blocks,
        &[
            dashboard_crumb(),
            Crumb::link("Blocks", "/dashboard/blocks"),
            Crumb::here(format!("Block {}", block.height)),
        ],
        None,
        None,
        None,
        "",
        body,
    )
}

fn worker_rows(now: u64, workers: &[WorkerRow]) -> Markup {
    html! {
        @for worker in workers {
            div.grid-row.worker {
                span {}
                span.mono { (worker.name) }
                span { @if worker.active { "online" } @else { "idle" } }
                span { (fmt_difficulty(worker.accepted_work as f64)) }
                span {}
                span { "best " (fmt_difficulty(worker.best_share as f64)) }
                (last_seen(now, worker.last_share_at))
                span {}
            }
        }
    }
}

pub(crate) fn miners(view: &MinersView) -> Markup {
    let first_rank = (view.paging.page - 1) * PAGE_SIZE;
    let body = html! {
        div.panel {
            h2 {
                "Miners"
                span.panel-note { "mining on this node; window share is across the whole pool. Open a row for its workers." }
            }
            @if view.miners.is_empty() {
                p.empty-state { "No miners have submitted shares to this node yet." }
            } @else {
                div.grid-table.miners-grid {
                    div.grid-head {
                        span { "#" } span { "Payout address" } span { "Workers" } span { "Accepted work" }
                        span { "Window share" } span { "Best share" } span { "Last seen" } span {}
                    }
                    @for (index, miner) in view.miners.iter().enumerate() {
                        details {
                            summary.grid-row {
                                span { (first_rank + index + 1) }
                                a.addr href=(miner_href(&miner.address)) title=(miner.address) { (miner.address) }
                                span { (miner.workers.iter().filter(|worker| worker.active).count()) " / " (miner.workers.len()) }
                                span { (fmt_difficulty(miner.accepted_work as f64)) }
                                span { (miner.payout_fraction.map(percent).unwrap_or_else(|| "—".to_string())) }
                                span { (fmt_difficulty(miner.best_share as f64)) }
                                (last_seen(view.now, miner.last_share_at))
                                span.toggle aria-label="Workers" {}
                            }
                            div.worker-list {
                                @if miner.workers.is_empty() {
                                    p.worker-empty { "No worker-level stats for this address." }
                                } @else {
                                    (worker_rows(view.now, &miner.workers))
                                }
                            }
                        }
                    }
                }
                (pager("/dashboard/miners", view.paging))
            }
        }
    };
    layout(
        &view.site,
        "Miners",
        Tab::Miners,
        &[dashboard_crumb(), Crumb::here("Miners")],
        None,
        None,
        None,
        "",
        body,
    )
}

pub(crate) fn miner(view: &MinerView) -> Markup {
    let report = &view.report;
    let base = miner_href(&report.address);
    let shares = view.paging.slice(&report.shares);
    let body = html! {
        div.panel { (lookup_form(&report.address)) }
        div.panel {
            h2 {
                "Miner"
                span.panel-note { (if report.address_type == "share" { "share chain address" } else { "payout address" }) }
            }
            div.stat-list {
                (stat("Address", html! { span.mono { (report.address) } }))
                (stat("Share of the next block's payout", html! { (percent(report.payout_fraction)) }))
                (stat("Shares in the window", html! {
                    (report.share_count) " of " (report.window.share_count) " (" (percent(report.share_fraction)) ")"
                }))
                (stat("Uncles credited", html! { (report.uncle_count) }))
                @if let (Some(level), Some(satoshis)) = (report.payout_level, report.payout_level_satoshis) {
                    (stat("Payout level", html! {
                        (level) " (" (satoshis) " satoshis)"
                        br; small { "smaller slices are paid by a draw" }
                    }))
                }
                @if let Some(payments) = report.expected_payments_a_month {
                    (stat("Expected payments a month", html! { (format!("{payments:.1}")) }))
                }
                (stat("Hashrate", html! {
                    (optional(view.hashrate, fmt_hashrate))
                    @if let Some((shares, seconds)) = view.recent_window {
                        br; small { "over the last " (shares) " shares, " (seconds / 60) " min" }
                    }
                }))
                (stat("Mining on", html! {
                    a href=(share_href(&report.anchor)) { "share " (report.anchor_height) }
                }))
            }
        }
        @if !view.workers.is_empty() {
            div.panel {
                h2 { "Workers on this node" }
                div.grid-table.miners-grid {
                    div.grid-head {
                        span {} span { "Worker" } span { "State" } span { "Accepted work" }
                        span {} span { "Best share" } span { "Last seen" } span {}
                    }
                    (worker_rows(view.now, &view.workers))
                }
            }
        }
        div.panel {
            h2 { "Shares credited in the window" span.panel-note { "newest first" } }
            @if shares.is_empty() {
                p.empty-state { "This address has no shares in the current window." }
            } @else {
                div.table-x {
                    table.table {
                        thead { tr { th { "Height" } th { "Share" } th { "Kind" } th { "Difficulty" } th { "Weight" } } }
                        tbody {
                            @for share in shares {
                                tr.uncle-row[share.nephew.is_some()] {
                                    td { (share.height) }
                                    td { (share_link(&share.blockhash)) }
                                    td { @if share.nephew.is_some() { "uncle" } @else { "share" } }
                                    td.mono { (share.difficulty) }
                                    td.mono { (share.weight) }
                                }
                            }
                        }
                    }
                }
                (pager(&base, view.paging))
            }
        }
    };
    layout(
        &view.site,
        &format!("Miner {}", short(&report.address)),
        Tab::Miners,
        &[
            dashboard_crumb(),
            Crumb::link("Miners", "/dashboard/miners"),
            Crumb::here(short(&report.address)),
        ],
        None,
        None,
        None,
        "",
        body,
    )
}

pub(crate) fn share(view: &ShareView) -> Markup {
    let share = &view.share;
    let header = &share.bitcoin_header;
    let owner = &share.miner_bitcoin_address;
    let body = html! {
        a.back-link href=(miner_href(owner)) { "← Miner " (short(owner)) }
        div.panel {
            h2 {
                "Share"
                @if view.is_block { " " span.badge-block { "found a block" } }
                span.panel-note { (share.validation_status) " · " (share.chain) }
            }
            div.stat-list {
                (stat("Hash", html! { span.mono { (share.blockhash) } }))
                (stat("Height", html! { (share.height.map(|height| height.to_string()).unwrap_or_else(|| "—".to_string())) }))
                (stat("Parent", share_link(&share.parent)))
                (stat("Payout address", miner_link(owner)))
                (stat("Share chain address", miner_link(&share.miner_address)))
                (stat("Share bits", html! { span.mono { (share.bits) } }))
                (stat("Time", html! { (share.time) }))
            }
        }
        div.panel {
            h2 { "Uncles" span.panel-note { (share.uncles.len()) " included" } }
            @if share.uncles.is_empty() {
                p.empty-state { "This share includes no uncles." }
            } @else {
                div.table-x {
                    table.table {
                        thead { tr { th { "Uncle" } } }
                        tbody { @for uncle in &share.uncles { tr { td { (share_link(uncle)) } } } }
                    }
                }
            }
        }
        div.panel {
            h2 { "Bitcoin Cash header" }
            div.stat-list {
                (stat("Block hash", html! { span.mono { (header.block_hash) } }))
                (stat("Previous block", html! { span.mono { (header.prev_blockhash) } }))
                (stat("Merkle root", html! { span.mono { (header.merkle_root) } }))
                (stat("Time", html! { (header.time) }))
                (stat("Bits", html! { span.mono { (header.bits) } }))
                (stat("Nonce", html! { span.mono { (header.nonce) } }))
                (stat("Version", html! { span.mono { (format!("{:#x}", header.version)) } }))
            }
        }
    };
    layout(
        &view.site,
        &format!("Share {}", short(&share.blockhash)),
        Tab::Miners,
        &[
            dashboard_crumb(),
            Crumb::link("Miners", "/dashboard/miners"),
            Crumb::link(short(owner), miner_href(owner)),
            Crumb::here(format!("Share {}", short(&share.blockhash))),
        ],
        None,
        None,
        None,
        "",
        body,
    )
}

pub(crate) fn help(site: &Site) -> Markup {
    let body = html! {
        div.panel {
            div.help-section {
                h3 { "Connect a miner" }
                p { "Stratum V1: point the miner at this node's stratum port." }
                ul {
                    li { "URL: " code { "stratum+tcp://<this node>:<stratum port>" } }
                    li { "User: " code { "<payout address>.<worker name>" } }
                    li { "Password: " code { "p2p=<share chain address>" } ", which names who owns your shares." }
                }
                p {
                    "Stratum V2: connect to the node's SV2 port and pin its authority key. Use "
                    code { "<payout address>.<worker>,p2p=<share chain address>" } " as the user identity. "
                    "SV1 devices can mine through an SV2 translator."
                }
                p { "Get a share chain address with " code { "p2poolv2_cli bch-share-address" } "." }
            }
            div.help-section {
                h3 { "Pages" }
                ul {
                    li { a href="/dashboard" { "Dashboard" } ": hashrate, found blocks and the newest shares." }
                    li { a href="/dashboard/blocks" { "Blocks" } ": every block the pool found." }
                    li { a href="/dashboard/miners" { "Miners" } ": the miners on this node and their workers." }
                    li { "Search: a payout or share chain address opens the miner, a share hash opens the share." }
                }
            }
            div.help-section {
                h3 { "API endpoints" }
                p { "The same data as JSON, for explorers and your own frontend:" }
                ul {
                    li { code { "/chain_info" } ", " code { "/shares" } ", " code { "/share?hash=" } ", " code { "/share_headers" } }
                    li { code { "/miner?address=" } ": a miner's shares and its part of the next payout" }
                    li { code { "/pplns_shares" } ", " code { "/candidates" } ", " code { "/dag" } ", " code { "/transaction" } }
                    li { code { "/hashrate" } ", " code { "/found_blocks" } ", " code { "/miners" } ", " code { "/network" }
                        ", " code { "/peers" } }
                    li { code { "/metrics" } " (Prometheus), " code { "/blocked_ips" }
                        ", " code { "/jd_prepare" } " and " code { "/share_transaction" }
                        ": always behind the API login" }
                }
            }
            div.help-section {
                h3 { "About this dashboard" }
                p {
                    "This node renders these pages in Rust, from the same store and PPLNS window the API reads. "
                    "They load no scripts. The design is CashDragon0's."
                }
            }
            div.help-section {
                h3 { "Admin sign in and out" }
                p {
                    "The Admin button in the header opens " code { "/dashboard/signin" }
                    ". Your browser asks for the API login; the page then lists what that login unlocks."
                }
                p {
                    "To sign out, submit any wrong password once at that prompt: the browser replaces the "
                    "saved login, and the dashboard keeps serving the public pages."
                }
            }
        }
    };
    layout(
        site,
        "Help",
        Tab::Help,
        &[dashboard_crumb(), Crumb::here("Help")],
        None,
        None,
        None,
        "",
        body,
    )
}

/// The landing page behind the API login: what the Admin button opens.
pub(crate) fn signin(site: &Site) -> Markup {
    let body = html! {
        div.panel {
            div.help-section {
                h3 { "Signed in" }
                p {
                    "Your browser holds the node's API login, so the operator routes below answer "
                    "without another prompt."
                }
                ul {
                    li { code { "/metrics" } ": the Prometheus exposition." }
                    li { code { "/blocked_ips" } ": read, block and unblock peer IPs." }
                    li { code { "/jd_prepare" } " and " code { "/share_transaction" }
                        ": the node's own controls, by POST." }
                }
            }
            div.help-section {
                h3 { "Sign out" }
                p {
                    "Submit any wrong password at the next prompt: the browser replaces the saved "
                    "login, and every visitor is back to the public pages only."
                }
            }
            p { a class="back-link" href="/dashboard" { "Back to the dashboard" } }
        }
    };
    layout(
        site,
        "Admin",
        Tab::None,
        &[dashboard_crumb(), Crumb::here("Admin")],
        None,
        None,
        None,
        "",
        body,
    )
}

/// A page for a request the dashboard could not answer.
pub(crate) fn error(site: &Site, message: &str) -> Markup {
    let body = html! {
        div.toast role="alert" { (message) }
        div.panel {
            h2 { "Find a miner" }
            (lookup_form(""))
        }
        p { a href="/dashboard" { "← Back to the dashboard" } }
    };
    layout(
        site,
        "Not available",
        Tab::None,
        &[dashboard_crumb(), Crumb::here("Not available")],
        None,
        None,
        None,
        "",
        body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site() -> Site {
        Site {
            network: "Bitcoin Cash chipnet".to_string(),
            badge: "chipnet".to_string(),
            auth: true,
        }
    }

    fn empty_overview() -> Overview {
        Overview {
            site: site(),
            now: 1_000,
            tip_height: None,
            pool_hashrate: None,
            local_hashrate: None,
            network_hashrate: None,
            network_difficulty: None,
            share_difficulty: None,
            miners_online: 0,
            workers_online: 0,
            accepted_shares_total: 0,
            rejected_shares_total: 0,
            samples: Vec::new(),
            blocks_found: 0,
            blocks: Vec::new(),
            shares: Vec::new(),
            auto: Auto::On,
            range_key: normalize_range(None),
            pool_on: true,
            node_on: true,
            network_on: false,
            blocks_on: true,
            chart_blocks: Vec::new(),
        }
    }

    #[test]
    fn hashrates_use_unit_prefixes() {
        assert_eq!(fmt_hashrate(0.0), "0.00 H/s");
        assert_eq!(fmt_hashrate(2_400_000_000_000.0), "2.40 TH/s");
        assert_eq!(fmt_hashrate(38_790_000_000_000.0), "38.8 TH/s");
        assert_eq!(fmt_hashrate(665_000.0), "665 kH/s");
    }

    #[test]
    fn auto_defaults_to_on_without_a_query_parameter() {
        assert_eq!(Auto::from_query(None), Auto::On);
        assert_eq!(Auto::from_query(Some("on")), Auto::On);
        assert_eq!(Auto::from_query(Some("off")), Auto::Off);
        assert_eq!(Auto::from_query(Some("bogus")), Auto::On);
    }

    #[test]
    fn hashrates_print_negative_zero_as_zero() {
        assert_eq!(fmt_hashrate(-0.0), "0.00 H/s");
        assert_eq!(fmt_hashrate(-0.001), "0.00 H/s");
    }

    #[test]
    fn difficulties_use_unit_prefixes() {
        assert_eq!(fmt_difficulty(665_151.95), "665k");
        assert_eq!(fmt_difficulty(4.5), "4.50");
    }

    #[test]
    fn difficulties_print_negative_zero_as_zero() {
        assert_eq!(fmt_difficulty(-0.0), "0.00");
        assert_eq!(fmt_difficulty(-0.001), "0.00");
    }

    #[test]
    fn ranges_normalize_to_their_table_or_the_default() {
        assert_eq!(normalize_range(None), "24h");
        assert_eq!(normalize_range(Some("1h")), "1h");
        assert_eq!(normalize_range(Some("bogus")), "24h");
    }

    #[test]
    fn the_range_buttons_mark_the_active_range_without_script() {
        let markup = range_buttons("1h", Auto::On, "").into_string();
        assert!(markup.contains("aria-label=\"Chart time range\""));
        assert!(markup.contains(
            "class=\"outline header-btn active-nav\" role=\"button\" href=\"/dashboard?range=1h\" aria-current=\"true\">Hour</a>"
        ));
        assert!(markup.contains("href=\"/dashboard?range=24h\">Day</a>"));
        assert!(!markup.contains("auto="));
        let paused = range_buttons("24h", Auto::Off, "").into_string();
        assert!(paused.contains("href=\"/dashboard?range=1h&amp;auto=off\""));
        let toggled = range_buttons("24h", Auto::On, "&pool=0").into_string();
        assert!(toggled.contains("href=\"/dashboard?range=1h&amp;pool=0\""));
    }

    #[test]
    fn a_series_is_off_only_when_its_parameter_says_so() {
        assert!(series_on(None));
        assert!(series_on(Some("1")));
        assert!(series_on(Some("bogus")));
        assert!(!series_on(Some("0")));
    }

    #[test]
    fn the_network_series_shows_only_when_its_parameter_asks_for_it() {
        assert!(!network_on(None));
        assert!(!network_on(Some("0")));
        assert!(!network_on(Some("bogus")));
        assert!(network_on(Some("1")));
    }

    #[test]
    fn the_series_query_lists_only_the_hidden_series() {
        assert_eq!(series_q(true, true, false, true), "");
        assert_eq!(series_q(false, true, false, true), "&pool=0");
        assert_eq!(series_q(true, false, false, true), "&node=0");
        assert_eq!(series_q(false, false, false, true), "&pool=0&node=0");
    }

    #[test]
    fn the_series_query_carries_the_network_turn_on() {
        assert_eq!(series_q(true, true, true, true), "&network=1");
        assert_eq!(
            series_q(false, false, true, true),
            "&pool=0&node=0&network=1"
        );
    }

    #[test]
    fn the_series_toggles_link_both_states_without_script() {
        let both_on = series_toggles(true, true, false, true, "24h", Auto::On).into_string();
        assert!(both_on.contains("aria-label=\"Chart series\""));
        assert!(both_on.contains("class=\"legend-chip\" role=\"button\" aria-pressed=\"true\""));
        assert!(both_on.contains("span class=\"legend-dot pool\""));
        assert!(both_on.contains("span class=\"legend-dot local\""));
        assert!(both_on.contains("/dashboard?range=24h&amp;pool=0"));
        assert!(both_on.contains("/dashboard?range=24h&amp;node=0"));
        // Each chip flips only its own series, keeping the other's state.
        let pool_hidden = series_toggles(false, true, false, true, "1h", Auto::Off).into_string();
        assert!(
            pool_hidden
                .contains("class=\"legend-chip off\" role=\"button\" aria-pressed=\"false\"")
        );
        assert!(pool_hidden.contains("/dashboard?range=1h&amp;auto=off\""));
        assert!(pool_hidden.contains("/dashboard?range=1h&amp;auto=off&amp;pool=0&amp;node=0"));
        assert!(!pool_hidden.contains("pool=0\""));
        let both_hidden = series_toggles(false, false, false, true, "24h", Auto::On).into_string();
        assert!(both_hidden.contains("legend-chip off"));
        assert!(both_hidden.contains("aria-pressed=\"false\""));
    }

    #[test]
    fn the_network_toggle_links_both_states_without_script() {
        let hidden = series_toggles(true, true, false, true, "24h", Auto::On).into_string();
        assert!(hidden.contains("span class=\"legend-dot network\""));
        assert!(
            hidden.contains("aria-pressed=\"false\" href=\"/dashboard?range=24h&amp;network=1\"")
        );
        // Shown: the network chip drops its own turn-on, while the other
        // chips keep it so the flip does not lose it.
        let shown = series_toggles(true, true, true, true, "24h", Auto::On).into_string();
        assert!(shown.contains("span class=\"legend-dot network\""));
        assert!(shown.contains("aria-pressed=\"true\" href=\"/dashboard?range=24h\""));
        assert!(shown.contains("href=\"/dashboard?range=24h&amp;pool=0&amp;network=1\""));
    }

    #[test]
    fn the_series_query_carries_the_blocks_turn_off() {
        assert_eq!(series_q(true, true, false, false), "&blocks=0");
        assert_eq!(series_q(false, true, false, false), "&pool=0&blocks=0");
    }

    #[test]
    fn the_blocks_toggle_links_both_states_without_script() {
        let shown = series_toggles(true, true, false, true, "24h", Auto::On).into_string();
        assert!(shown.contains("span class=\"legend-dot blocks\""));
        assert!(shown.contains(">Blocks</a>"));
        // Shown: the chip links its own turn-off, keeping the other series
        // as they are.
        assert!(shown.contains("aria-pressed=\"true\" href=\"/dashboard?range=24h&amp;blocks=0\""));
        // Hidden: the chip links back to the clean URL and marks itself off.
        let hidden = series_toggles(true, true, false, false, "24h", Auto::On).into_string();
        assert!(hidden.contains("class=\"legend-chip off\""));
        assert!(hidden.contains("aria-pressed=\"false\" href=\"/dashboard?range=24h\""));
    }

    #[test]
    fn range_windows_match_the_range_buttons() {
        assert_eq!(range_window_secs("1h"), 3600);
        assert_eq!(range_window_secs("24h"), 86400);
        assert_eq!(range_window_secs("1w"), 604800);
        assert_eq!(range_window_secs("1m"), 2592000);
        // An unknown range falls back with the normalized key itself, so
        // the window always matches the chart the page draws.
        assert_eq!(range_window_secs(DEFAULT_RANGE), 86400);
    }

    #[test]
    fn chart_times_carry_the_day() {
        assert_eq!(fmt_day_time(1_792_000_000), "Oct 14 17:46 UTC");
    }

    #[test]
    fn ages_pick_a_readable_unit() {
        assert_eq!(fmt_ago(100, 58), "42 s ago");
        assert_eq!(fmt_ago(1_000, 100), "15 min ago");
        assert_eq!(fmt_ago(20_000, 0), "5 h ago");
        assert_eq!(fmt_ago(200_000, 0), "2 d ago");
        assert_eq!(fmt_ago(0, 100), "0 s ago");
    }

    #[test]
    fn long_hashes_are_shortened() {
        assert_eq!(short("abc"), "abc");
        let hash = "00000000000cb752459c2b08e1955e5f33252b0207b1c7b5b2f069d5254b9025";
        assert_eq!(short(hash), "0000000000…4b9025");
        // Cut on characters, never inside one.
        assert_eq!(
            short(&"é".repeat(30)),
            format!("{}…{}", "é".repeat(10), "é".repeat(6))
        );
    }

    #[test]
    fn a_worker_without_shares_was_never_seen() {
        assert_eq!(last_seen(1_000, 0).into_string(), "<span>never</span>");
        assert!(last_seen(1_000, 940).into_string().contains("1 min ago"));
    }

    #[test]
    fn url_components_escape_structure() {
        assert_eq!(url_component("bchtest:qr3rryn6"), "bchtest:qr3rryn6");
        assert_eq!(url_component("a&b=c d"), "a%26b%3Dc%20d");
    }

    #[test]
    fn paging_clamps_and_slices() {
        let rows: Vec<usize> = (0..60).collect();
        let first = Paging::of(None, rows.len());
        assert_eq!(first, Paging { page: 1, pages: 3 });
        assert_eq!(first.slice(&rows), &rows[..25]);
        let last = Paging::of(Some(9), rows.len());
        assert_eq!(last, Paging { page: 3, pages: 3 });
        assert_eq!(last.slice(&rows), &rows[50..]);
        assert_eq!(Paging::of(Some(0), 0), Paging { page: 1, pages: 1 });
    }

    #[test]
    fn the_pager_links_neighbouring_pages() {
        let markup =
            pager("/dashboard/miner?address=x", Paging { page: 2, pages: 3 }).into_string();
        assert!(markup.contains("/dashboard/miner?address=x&amp;page=1"));
        assert!(markup.contains("/dashboard/miner?address=x&amp;page=3"));
        assert!(markup.contains("Page 2 of 3"));
        assert!(
            pager("/dashboard", Paging { page: 1, pages: 1 })
                .into_string()
                .is_empty()
        );
        let first = pager("/dashboard", Paging { page: 1, pages: 2 }).into_string();
        assert!(first.contains("aria-disabled=\"true\" rel=\"prev\""));
    }

    #[test]
    fn the_layout_marks_the_tab_and_the_trail_without_script() {
        let page = layout(
            &site(),
            "<b>x</b>",
            Tab::Blocks,
            &[dashboard_crumb(), Crumb::here("Blocks")],
            None,
            None,
            None,
            "",
            html! { p { "hi" } },
        )
        .into_string();
        assert!(!page.contains("<script"));
        assert!(!page.contains("http-equiv"));
        assert!(page.contains("&lt;b&gt;x&lt;/b&gt;"));
        assert!(page.contains(
            "class=\"outline header-btn active-nav\" role=\"button\" href=\"/dashboard/blocks\" aria-current=\"page\""
        ));
        assert!(page.contains("aria-label=\"breadcrumb\""));
        assert!(page.contains("action=\"/dashboard/search\""));
        assert!(page.contains("href=\"/dashboard/signin\""));
        assert!(page.contains(">Admin</a>"));
        assert!(page.contains(&assets::href("dashboard.css")));
    }

    #[test]
    fn the_admin_button_is_absent_without_an_api_login() {
        let page = layout(
            &Site {
                network: "Bitcoin Cash chipnet".to_string(),
                badge: "chipnet".to_string(),
                auth: false,
            },
            "Dashboard",
            Tab::Dashboard,
            &[],
            None,
            None,
            None,
            "",
            html! { p { "hi" } },
        )
        .into_string();
        assert!(!page.contains("/dashboard/signin"));
        assert!(!page.contains(">Admin</a>"));
    }

    #[test]
    fn the_signin_page_lists_the_protected_routes_without_script() {
        let page = signin(&site()).into_string();
        assert!(!page.contains("<script"));
        assert!(page.contains("Sign in"));
        assert!(page.contains("/metrics"));
        assert!(page.contains("/jd_prepare"));
        assert!(page.contains("wrong password"));
        assert!(page.contains("Back to the dashboard"));
    }

    #[test]
    fn the_help_page_explains_signing_in_and_out() {
        let page = help(&site()).into_string();
        assert!(!page.contains("<script"));
        assert!(page.contains("/dashboard/signin"));
        assert!(page.contains("wrong password"));
    }

    #[test]
    fn an_empty_overview_renders() {
        let page = overview(&empty_overview()).into_string();
        assert!(page.contains("No confirmed shares yet."));
        assert!(page.contains("Collecting data"));
        assert!(page.contains("has not found a Bitcoin Cash block yet"));
        assert!(!page.contains("<script"));
    }

    #[test]
    fn the_overview_lists_all_three_series_toggles_by_default() {
        let page = overview(&empty_overview()).into_string();
        assert!(page.contains("series-toggles"));
        assert!(page.contains("legend-dot pool"));
        assert!(page.contains("legend-dot local"));
        assert!(page.contains("legend-dot network"));
        // Pool and Node show; the network line is opt-in, so only its chip is off.
        assert!(page.contains("class=\"legend-chip\" role=\"button\" aria-pressed=\"true\""));
        assert!(page.contains("class=\"legend-chip off\" role=\"button\" aria-pressed=\"false\""));
        assert!(page.contains("href=\"/dashboard?range=24h&amp;pool=0\""));
        assert!(page.contains("href=\"/dashboard?range=24h&amp;node=0\""));
        assert!(page.contains("href=\"/dashboard?range=24h&amp;network=1\""));
    }

    #[test]
    fn showing_the_network_series_draws_its_line_and_chips() {
        let mut view = empty_overview();
        view.samples = vec![
            Sample {
                time: 1_000,
                pool: Some(100.0),
                local: Some(50.0),
                network: Some(4.0e6),
            },
            Sample {
                time: 1_060,
                pool: Some(200.0),
                local: Some(60.0),
                network: Some(5.0e6),
            },
        ];
        view.network_on = true;
        let page = overview(&view).into_string();
        assert!(page.contains("polyline class=\"line network\""));
        assert!(page.contains("class=\"area network\""));
        assert!(page.contains("tracer network"));
        // The network chip drops its own turn-on; the others keep it.
        assert!(page.contains("href=\"/dashboard?range=24h\""));
        assert!(page.contains("href=\"/dashboard?range=24h&amp;network=1\""));
    }

    #[test]
    fn hiding_a_series_drops_it_from_the_chart() {
        let mut view = empty_overview();
        view.samples = vec![
            Sample {
                time: 1_000,
                pool: Some(100.0),
                local: Some(50.0),
                network: None,
            },
            Sample {
                time: 1_060,
                pool: Some(200.0),
                local: Some(60.0),
                network: None,
            },
        ];
        view.pool_on = false;
        let page = overview(&view).into_string();
        assert!(page.contains("polyline class=\"line local\""));
        assert!(!page.contains("polyline class=\"line pool\""));
        assert!(!page.contains("class=\"area pool\""));
        assert!(!page.contains("tracer pool"));
        assert!(page.contains("class=\"legend-chip off\""));
        assert!(page.contains("href=\"/dashboard?range=24h\""));
        assert!(!page.contains("Collecting data"));
    }

    #[test]
    fn hiding_all_series_shows_the_legend_note() {
        let mut view = empty_overview();
        view.pool_on = false;
        view.node_on = false;
        view.network_on = false;
        let page = overview(&view).into_string();
        assert!(page.contains("All series hidden. Click a legend chip to show one."));
        assert!(!page.contains("<svg"));
        assert!(!page.contains("Collecting data"));
    }

    #[test]
    fn the_blocks_turn_off_drops_the_diamonds_from_the_chart() {
        let mut view = empty_overview();
        view.samples = vec![
            Sample {
                time: 940,
                pool: Some(100.0),
                local: Some(50.0),
                network: None,
            },
            Sample {
                time: 1_060,
                pool: Some(200.0),
                local: Some(60.0),
                network: None,
            },
        ];
        view.chart_blocks = vec![ChartBlock {
            height: 42,
            time: 1_000,
        }];
        view.blocks_on = true;
        let shown = overview(&view).into_string();
        assert!(shown.contains("class=\"cube\""));
        assert!(shown.contains("Block 42 \u{b7}"));
        // The chip's turn-off empties the chart's block list: no marker
        // and no tooltip row survive it, as in the replica.
        view.blocks_on = false;
        let hidden = overview(&view).into_string();
        assert!(!hidden.contains("class=\"cube\""));
        assert!(!hidden.contains("Block 42"));
    }

    #[test]
    fn a_hostile_payout_address_is_escaped_in_the_overview() {
        let mut view = empty_overview();
        view.tip_height = Some(1);
        view.shares = vec![ShareRow {
            height: Some(1),
            blockhash: "ab".to_string(),
            time: 900,
            payout_address: "<img src=x onerror=alert(1)>".to_string(),
            share_address: "tp2pool:x".to_string(),
            difficulty: Some(4.0),
            is_uncle: false,
            is_block: true,
        }];
        let page = overview(&view).into_string();
        assert!(!page.contains("<img src=x"));
        assert!(page.contains("&lt;img src=x"));
        assert!(page.contains("class=\"badge-block\""));
    }

    #[test]
    fn the_help_page_explains_the_share_address() {
        let page = help(&site()).into_string();
        assert!(page.contains("p2p=&lt;share chain address&gt;"));
        assert!(page.contains("CashDragon0"));
    }

    #[test]
    fn the_this_node_card_notes_the_rejected_shares() {
        let mut view = empty_overview();
        view.accepted_shares_total = 10;
        view.rejected_shares_total = 3;
        let page = overview(&view).into_string();
        let pool_card = page.split("kpi-label\">Pool hashrate").nth(1).unwrap();
        let pool_card = pool_card.split("kpi-card").next().unwrap();
        assert!(!pool_card.contains("rejected of"));
        assert!(pool_card.contains("Confirmed shares, last hour"));
        let node_card = page.split("kpi-label\">Node hashrate").nth(1).unwrap();
        let node_card = node_card.split("kpi-card").next().unwrap();
        assert!(node_card.contains("3 rejected of 10 shares"));
    }

    #[test]
    fn the_share_chain_height_card_notes_the_share_difficulty() {
        let mut view = empty_overview();
        view.tip_height = Some(30);
        view.share_difficulty = Some(73.0);
        let page = overview(&view).into_string();
        let card = page.split("kpi-label\">Share chain height").nth(1).unwrap();
        let card = card.split("kpi-card").next().unwrap();
        assert!(card.contains("Difficulty 73"));
    }

    #[test]
    fn the_share_chain_height_card_shows_a_dash_before_the_first_notify() {
        let mut view = empty_overview();
        view.tip_height = Some(30);
        view.share_difficulty = None;
        let page = overview(&view).into_string();
        let card = page.split("kpi-label\">Share chain height").nth(1).unwrap();
        let card = card.split("kpi-card").next().unwrap();
        assert!(card.contains("Difficulty —"));
    }

    #[test]
    fn the_blocks_found_card_values_it_green() {
        let page = overview(&empty_overview()).into_string();
        let card = page.split("kpi-label\">Blocks found").nth(1).unwrap();
        let card = card.split("kpi-card").next().unwrap();
        assert!(card.contains("kpi-value kpi-value-green"));
    }

    #[test]
    fn the_overview_recent_shares_link_to_the_shares_page_instead_of_a_pager() {
        let mut view = empty_overview();
        view.tip_height = Some(30);
        view.shares = vec![ShareRow {
            height: Some(30),
            blockhash: "ab".to_string(),
            time: 900,
            payout_address: "bchtest:qpayout".to_string(),
            share_address: "tp2pool:qowner".to_string(),
            difficulty: Some(4.0),
            is_uncle: false,
            is_block: false,
        }];
        let page = overview(&view).into_string();
        assert!(page.contains("href=\"/dashboard/shares\""));
        assert!(page.contains("More found shares"));
        assert!(!page.contains("page="));
        assert!(!page.contains("Previous"));
    }

    #[test]
    fn the_shares_page_lists_the_chain_with_a_pager() {
        let view = SharesView {
            site: site(),
            now: 1_000,
            tip_height: Some(30),
            shares: vec![ShareRow {
                height: Some(30),
                blockhash: "ab".to_string(),
                time: 900,
                payout_address: "bchtest:qpayout".to_string(),
                share_address: "tp2pool:qowner".to_string(),
                difficulty: Some(4.0),
                is_uncle: false,
                is_block: false,
            }],
            paging: Paging { page: 2, pages: 3 },
        };
        let page = shares(&view).into_string();
        assert!(page.contains("<td>30</td>"));
        assert!(page.contains("Page 2 of 3"));
        assert!(page.contains("/dashboard/shares?page=1"));
        assert!(page.contains("/dashboard/shares?page=3"));
        assert!(page.contains("tip 30"));
        assert!(!page.contains("<script"));
    }

    #[test]
    fn the_shares_page_without_a_chain_says_so() {
        let view = SharesView {
            site: site(),
            now: 1_000,
            tip_height: None,
            shares: Vec::new(),
            paging: Paging { page: 1, pages: 1 },
        };
        let page = shares(&view).into_string();
        assert!(page.contains("No confirmed shares yet."));
        assert!(!page.contains("Previous"));
    }
}
