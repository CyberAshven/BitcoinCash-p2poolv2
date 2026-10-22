// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The dashboard's pages, rendered to HTML in Rust.
//!
//! The layout and stylesheet are CashDragon0's design from PR #8, rebuilt
//! here without a script: every page has the same menu bar, a search box and
//! a trail back to the overview, its pop-ups are pages of their own, long
//! tables are split into pages with links, and a miner's workers open with
//! an HTML `<details>`. The overview refreshes itself with a meta refresh and
//! chart tooltips are SVG titles. Every value is escaped by maud.

use super::assets;
use super::chart::{Series, line_chart};
use super::history::Sample;
use crate::api::endpoints::miner::MinerResponse;
use crate::api::endpoints::share::ShareLookupOutput;
use bitcoin::BlockHash;
use maud::{DOCTYPE, Markup, html};

/// Seconds between refreshes of the overview page.
pub(crate) const OVERVIEW_REFRESH_SECONDS: u32 = 30;

/// Rows on one page of a long table.
pub(crate) const PAGE_SIZE: usize = 25;

/// What every page shows about the node it is served by.
pub(crate) struct Site {
    /// The chain this node mines, for example "Bitcoin Cash chipnet".
    pub(crate) network: String,
    /// The short name in the header badge, for example "chipnet".
    pub(crate) badge: String,
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
    pub(crate) miners_online: usize,
    pub(crate) workers_online: usize,
    pub(crate) samples: Vec<Sample>,
    /// Blocks the pool found in all.
    pub(crate) blocks_found: usize,
    /// The newest of them, newest first.
    pub(crate) blocks: Vec<FoundBlockRow>,
    /// This page of the share chain, newest first.
    pub(crate) shares: Vec<ShareRow>,
    pub(crate) shares_paging: Paging,
}

/// The found blocks page.
pub(crate) struct BlocksView {
    pub(crate) site: Site,
    pub(crate) now: u64,
    pub(crate) blocks_found: usize,
    pub(crate) blocks: Vec<FoundBlockRow>,
    pub(crate) paging: Paging,
}

/// One found block's page.
pub(crate) struct BlockView {
    pub(crate) site: Site,
    pub(crate) now: u64,
    pub(crate) block: FoundBlockRow,
}

/// A worker of a miner on this node.
pub(crate) struct WorkerRow {
    pub(crate) name: String,
    pub(crate) active: bool,
    /// Sum of the difficulty of its accepted shares.
    pub(crate) accepted_work: u64,
    pub(crate) best_share: u64,
    pub(crate) last_share_at: u64,
}

/// A miner on this node, for the miners page.
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
    let mut scaled = value;
    let mut unit = 0;
    while scaled.abs() >= 1000.0 && unit + 1 < UNITS.len() {
        scaled /= 1000.0;
        unit += 1;
    }
    (scaled, UNITS[unit])
}

fn three_digits(value: f64) -> String {
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

fn optional(value: Option<f64>, format: fn(f64) -> String) -> String {
    value.map(format).unwrap_or_else(|| "—".to_string())
}

fn percent(fraction: f64) -> String {
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
pub(crate) fn layout(
    site: &Site,
    title: &str,
    tab: Tab,
    trail: &[Crumb],
    refresh: Option<u32>,
    body: Markup,
) -> Markup {
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
                            li {
                                form.header-search action="/dashboard/search" method="get" role="search" {
                                    input type="search" name="q" placeholder="Miner address or share hash"
                                        aria-label="Miner address or share hash" required;
                                    button.outline.header-btn type="submit" { "Search" }
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
    let chart = line_chart(
        "Hashrate over the last day",
        &[
            Series {
                label: "Pool",
                class: "pool",
                points: series(&view.samples, |sample| sample.pool),
            },
            Series {
                label: "This node",
                class: "local",
                points: series(&view.samples, |sample| sample.local),
            },
        ],
        fmt_hashrate,
        fmt_day_time,
    );
    let last_block = view.blocks.first().map(|block| block.time);
    let body = html! {
        div.kpi-grid {
            (kpi_card("Pool hashrate", optional(view.pool_hashrate, fmt_hashrate), "kpi-value-green", None,
                &["From the newest shares on the share chain".to_string()]))
            (kpi_card("This node", optional(view.local_hashrate, fmt_hashrate), "kpi-value-local", Some("/dashboard/miners"),
                &[format!("{} miners, {} workers online", view.miners_online, view.workers_online)]))
            (kpi_card("Network hashrate (BCH)", optional(view.network_hashrate, fmt_hashrate), "kpi-value-net", None,
                &[format!("Difficulty {}", optional(view.network_difficulty, fmt_difficulty)),
                  "Estimated from difficulty, 10-minute blocks".to_string()]))
            (kpi_card("Share chain height",
                view.tip_height.map(|height| height.to_string()).unwrap_or_else(|| "—".to_string()), "", None, &[]))
            (kpi_card("Blocks found", view.blocks_found.to_string(), "", Some("/dashboard/blocks"),
                &[last_block
                    .map(|time| format!("Last {}", fmt_ago(view.now, u64::from(time))))
                    .unwrap_or_else(|| "None yet".to_string())]))
        }
        div.panel {
            h2 { "Hashrate" span.panel-note { "one sample a minute, the last day" } }
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
                (pager("/dashboard", view.shares_paging))
            }
        }
    };
    let refresh = (view.shares_paging.page == 1).then_some(OVERVIEW_REFRESH_SECONDS);
    layout(&view.site, "Dashboard", Tab::Dashboard, &[], refresh, body)
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
                    li { code { "/hashrate" } ", " code { "/found_blocks" } }
                    li { code { "/metrics" } " (Prometheus) and " code { "/peers" } ": always behind the API login" }
                }
            }
            div.help-section {
                h3 { "About this dashboard" }
                p {
                    "This node renders these pages in Rust, from the same store and PPLNS window the API reads. "
                    "They load no scripts. The design is CashDragon0's."
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
            miners_online: 0,
            workers_online: 0,
            samples: Vec::new(),
            blocks_found: 0,
            blocks: Vec::new(),
            shares: Vec::new(),
            shares_paging: Paging::of(None, 0),
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
    fn difficulties_use_unit_prefixes() {
        assert_eq!(fmt_difficulty(665_151.95), "665k");
        assert_eq!(fmt_difficulty(4.5), "4.50");
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
            Some(30),
            html! { p { "hi" } },
        )
        .into_string();
        assert!(!page.contains("<script"));
        assert!(page.contains("&lt;b&gt;x&lt;/b&gt;"));
        assert!(page.contains("http-equiv=\"refresh\""));
        assert!(page.contains(
            "class=\"outline header-btn active-nav\" role=\"button\" href=\"/dashboard/blocks\" aria-current=\"page\""
        ));
        assert!(page.contains("aria-label=\"breadcrumb\""));
        assert!(page.contains("action=\"/dashboard/search\""));
        assert!(page.contains(&assets::href("dashboard.css")));
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
}
