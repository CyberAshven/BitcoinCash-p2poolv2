// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The overview, as `/dashboard` shows it: KPI cards, the hashrate chart,
//! the newest found blocks and the newest few share heights. Its shares are
//! live, from `/ws`; the shares page has the whole chain.

use super::blocks::{NO_BLOCKS_YET, blocks_table};
use crate::api::{
    ChainInfo, FoundBlock, Hashrate, HashratePoint, Miners, Peer, Share, SharechainSeries,
};
use crate::app::{Ctx, ctx, polled};
use crate::chart::{HashrateChart, SharechainChart};
use crate::format::{
    BLOCK_INTERVAL_SECONDS, UNKNOWN, agent_version, difficulty_from_bits,
    difficulty_from_network_hashrate, fmt_ago, fmt_difficulty, fmt_duration, fmt_effort,
    fmt_hashrate, fmt_latency, optional,
};
use crate::layout::{KpiCard, Tone, ago_cell, loading, miner_link, share_link};
use crate::route::{Route, Tab};
use leptos::prelude::*;
use std::cmp::Reverse;
use std::collections::HashSet;

/// Chart windows offered: seconds, button, what the panel note calls it and
/// the chart's name.
const WINDOWS: [(u64, &str, &str, &str); 4] = [
    (
        3_600,
        "Hour",
        "one sample a minute, the last hour",
        "Hashrate over the last hour",
    ),
    (
        86_400,
        "Day",
        "one sample a minute, the last day",
        "Hashrate over the last day",
    ),
    (
        604_800,
        "Week",
        "one sample a minute, the last week",
        "Hashrate over the last week",
    ),
    (
        2_592_000,
        "Month",
        "one sample a minute, the last month",
        "Hashrate over the last month",
    ),
];

/// Most hashrate samples kept for the chart: the store's 30-day retention
/// at one sample a minute, the most the node's history holds.
pub const MAX_HISTORY_POINTS: usize = 2_592_000 / 60;

/// Found blocks the overview lists; the blocks page has them all.
const RECENT_BLOCKS: usize = 5;

/// Confirmed share heights the overview's Recent shares lists; the shares
/// page has them all. [`RECENT_ROWS`] caps the rows those heights expand
/// to, uncles included.
const RECENT_SHARES: usize = 5;

/// Rows the overview's Recent shares table renders: the height window
/// expands to a row per share plus a row per uncle, and this truncates the
/// result to a fixed count so the panel keeps the same size on every
/// refresh, whatever uncles the window carries, as the node's own
/// `OVERVIEW_ROWS` does.
const RECENT_ROWS: usize = 5;

/// Connected peers the overview's Peers panel lists; the peers page has
/// them all. The panel keeps this many rows at every count so a peer
/// joining or leaving never changes its height.
const RECENT_PEERS: usize = 5;

/// What the overview's peers panel says when its request answered and no
/// peer is connected.
pub const NO_PEERS_YET: &str =
    "No p2pool peers connected right now. They appear here once the node dials or accepts a peer.";

/// A confirmed share or one of its uncles, as the overview lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct ShareRow {
    pub height: Option<u32>,
    pub blockhash: String,
    pub time: u64,
    pub payout_address: String,
    pub share_address: String,
    /// `None` for an uncle, whose bits the list does not carry.
    pub difficulty: Option<f64>,
    pub is_uncle: bool,
    pub is_block: bool,
    /// Weak shares this share's body carries; always 0 for an uncle.
    pub weak_count: u16,
}

/// The rows for `shares`, newest first: each share followed by the uncles
/// it includes. A share or uncle in `found` found a block.
pub fn share_rows(shares: &[Share], found: &HashSet<String>) -> Vec<ShareRow> {
    let mut ordered: Vec<&Share> = shares.iter().collect();
    ordered.sort_by_key(|share| Reverse(share.height));
    let mut rows = Vec::with_capacity(shares.len() * 2);
    for share in ordered {
        rows.push(ShareRow {
            height: Some(share.height),
            blockhash: share.blockhash.clone(),
            time: share.timestamp,
            payout_address: share.miner_bitcoin_address.clone(),
            share_address: share.miner_address.clone(),
            difficulty: Some(difficulty_from_bits(share.bits)),
            is_uncle: false,
            is_block: found.contains(&share.blockhash),
            weak_count: share.weak_count,
        });
        rows.extend(share.uncles.iter().map(|uncle| ShareRow {
            height: uncle.height,
            blockhash: uncle.blockhash.clone(),
            time: uncle.timestamp,
            payout_address: uncle.miner_bitcoin_address.clone(),
            share_address: uncle.miner_address.clone(),
            difficulty: None,
            is_uncle: true,
            is_block: found.contains(&uncle.blockhash),
            // An uncle never carries weak shares.
            weak_count: 0,
        }));
    }
    rows
}

/// The rows the overview's Recent shares panel holds: the window's rows,
/// capped at [`RECENT_ROWS`], dropping oldest rows only, so a share row
/// still sits directly above its uncles and the panel keeps the same size
/// on every refresh, whatever uncles the window carries. The node's own
/// overview truncates the same way.
pub fn capped_share_rows(shares: &[Share], found: &HashSet<String>) -> Vec<ShareRow> {
    let mut rows = share_rows(shares, found);
    rows.truncate(RECENT_ROWS);
    rows
}

/// The newest `MAX_HISTORY_POINTS` samples of `history`.
pub fn capped_history(mut history: Vec<HashratePoint>) -> Vec<HashratePoint> {
    let excess = history.len().saturating_sub(MAX_HISTORY_POINTS);
    history.drain(..excess);
    history
}

/// The peers the overview's panel lists: the `RECENT_PEERS` the endpoint
/// sent first, in its longest-uptime order. The panel's note still counts
/// every peer, and the peers page lists them all.
pub fn capped_peers(peers: &[Peer]) -> Vec<Peer> {
    peers.iter().take(RECENT_PEERS).cloned().collect()
}

/// The network difficulty `hashrate` implies: its current network hashrate,
/// else the newest sampled one, over the node's block interval, ten minutes
/// from a node that sends none.
pub fn implied_network_difficulty(hashrate: &Hashrate) -> Option<f64> {
    let interval = hashrate
        .block_interval_secs
        .unwrap_or(BLOCK_INTERVAL_SECONDS);
    hashrate
        .network_hashrate
        .or_else(|| {
            hashrate
                .history
                .iter()
                .rev()
                .find_map(|point| point.network_hashrate)
        })
        .map(|rate| difficulty_from_network_hashrate(rate, interval))
}

fn share_row_view(row: ShareRow) -> impl IntoView {
    let height = row
        .height
        .map_or_else(|| UNKNOWN.to_string(), |height| height.to_string());
    view! {
        <tr class:uncle-row=row.is_uncle>
            <td>{height}</td>
            <td>{share_link(&row.blockhash)}</td>
            <td>
                {if row.is_uncle { "uncle" } else { "share" }}
                {row.is_block.then(|| view! { " " <span class="badge-block">"block"</span> })}
                {(row.weak_count > 0)
                    .then(|| view! { " " <span class="badge-weak">{format!("{} weak", row.weak_count)}</span> })}
            </td>
            <td>{miner_link(&row.payout_address)}</td>
            <td>{miner_link(&row.share_address)}</td>
            {ago_cell(row.time)}
            <td class="mono">{optional(row.difficulty, fmt_difficulty)}</td>
        </tr>
    }
}

pub fn shares_table(rows: Vec<ShareRow>) -> impl IntoView {
    view! {
        <div class="table-wrap">
            <table class="table">
                <thead>
                    <tr>
                        <th>"Height"</th>
                        <th>"Share hash"</th>
                        <th>"Type"</th>
                        <th>"Miner (payout)"</th>
                        <th>"Share owner"</th>
                        <th>"Time"</th>
                        <th>"Difficulty"</th>
                    </tr>
                </thead>
                <tbody>{rows.into_iter().map(share_row_view).collect_view()}</tbody>
            </table>
        </div>
    }
}

/// A peer's address cell: its IP with the connection's port, or the full
/// multiaddr when the IP is not known (its own `/tcp/` component already
/// carries the port), or the dash when neither is there.
fn peer_address(peer: &Peer) -> String {
    if let Some(ip) = peer.ip.as_deref().filter(|ip| !ip.is_empty()) {
        return match peer_port(&peer.address) {
            Some(port) => format!("{ip}:{port}"),
            None => ip.to_string(),
        };
    }
    if peer.address.is_empty() {
        UNKNOWN.to_string()
    } else {
        peer.address.clone()
    }
}

/// The TCP port in a peer's multiaddr: the digits after `/tcp/` up to the
/// next component, or none when the address carries no transport port.
fn peer_port(address: &str) -> Option<&str> {
    let (_, tail) = address.split_once("/tcp/")?;
    let port = tail.split('/').next().unwrap_or("");
    (!port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit())).then_some(port)
}

/// One connected peer as the peers table's row: its address as host:port
/// (or the full multiaddr when the IP is not known, or the dash when
/// neither is there), its version with the identify agent prefix dropped,
/// the connection direction, its uptime and its latest ping round trip.
fn peer_row_view(peer: Peer) -> impl IntoView {
    let address = peer_address(&peer);
    let version = agent_version(peer.version.as_deref());
    let latency = optional(peer.latency_ms, fmt_latency);
    view! {
        <tr>
            <td class="mono peer-addr">{address}</td>
            <td>{version}</td>
            <td>{peer.direction}</td>
            <td>{fmt_duration(peer.connected_secs)}</td>
            <td class="mono">{latency}</td>
        </tr>
    }
}

/// The connected peers table: the overview and the peers page render the
/// same five columns, so one component holds the markup.
#[component]
pub fn PeerTable(peers: Vec<Peer>) -> impl IntoView {
    let rows = peers.into_iter().map(peer_row_view).collect_view();
    view! {
        <div class="table-wrap">
            <table class="table">
                <thead>
                    <tr>
                        <th>"Address:port"</th>
                        <th>"Version"</th>
                        <th>"Direction"</th>
                        <th>"Uptime"</th>
                        <th>"Latency"</th>
                    </tr>
                </thead>
                <tbody>{rows}</tbody>
            </table>
        </div>
    }
}

/// `read` applied to a resource's value once it loaded, without copying the
/// value.
fn loaded<T: 'static, R>(
    resource: &LocalResource<Result<T, String>>,
    read: impl FnOnce(&T) -> R,
) -> Option<R> {
    resource.with(|value| match value {
        Some(Ok(value)) => Some(read(value)),
        _ => None,
    })
}

/// A resource's error, once it failed.
fn failure<T: 'static>(resource: &LocalResource<Result<T, String>>) -> Option<String> {
    resource.with(|value| match value {
        Some(Err(message)) => Some(message.clone()),
        _ => None,
    })
}

/// The first error among the overview's requests, shown above it as the
/// node's error page would.
fn first_error(errors: [Option<String>; 5]) -> Option<String> {
    errors.into_iter().flatten().next()
}

#[component]
pub fn OverviewPage() -> impl IntoView {
    let context = ctx();
    context.frame.set("Dashboard", Tab::Dashboard, Vec::new());
    let Ctx { clock, recent, .. } = context;

    let stored_window = crate::app::stored(crate::app::local_storage(), "p2pool.range")
        .and_then(|stored| stored.parse::<u64>().ok())
        .and_then(|seconds| {
            WINDOWS
                .iter()
                .any(|(window, ..)| *window == seconds)
                .then_some(seconds)
        })
        .unwrap_or(WINDOWS[1].0);
    let window = RwSignal::new(stored_window);
    // The chart's zoomed window; `None` shows everything. The header's
    // reset button clears it while a zoom is active.
    let zoom = RwSignal::new(None::<(f64, f64)>);
    // The three chart series, persisted like the range so a reload keeps
    // them; the network line is opt-in, so it starts from its own flag.
    let pool_visible = RwSignal::new(crate::app::stored_flag("p2pool.pool-visible"));
    let local_visible = RwSignal::new(crate::app::stored_flag("p2pool.local-visible"));
    let network_visible = RwSignal::new(crate::app::stored_opt_in_flag("p2pool.network-visible"));
    // The found-block diamonds, on by default like the pool and node
    // series: [stored_flag] reads only `"0"` as off.
    let blocks_visible = RwSignal::new(crate::app::stored_flag("p2pool.blocks-visible"));
    // Sharechain chart tab: "sharechain" shows the sharechain chart;
    // anything else shows the hashrate chart. Persisted in localStorage.
    let chart_tab = RwSignal::new(
        match crate::app::stored(crate::app::local_storage(), "p2pool.chart").as_deref() {
            Some("sharechain") => "sharechain",
            _ => "hashrate",
        },
    );
    let switch_to_hashrate = {
        let tab = chart_tab;
        move |_| {
            tab.set("hashrate");
            crate::app::store(crate::app::local_storage(), "p2pool.chart", "hashrate");
        }
    };
    let switch_to_sharechain = {
        let tab = chart_tab;
        move |_| {
            tab.set("sharechain");
            crate::app::store(crate::app::local_storage(), "p2pool.chart", "sharechain");
        }
    };
    // Sharechain series visibility, persisted like the hashrate chips.
    let height_visible = RwSignal::new(crate::app::stored_flag("p2pool.height-visible"));
    let dmin_visible = RwSignal::new(crate::app::stored_flag("p2pool.dmin-visible"));
    let dmax_visible = RwSignal::new(crate::app::stored_flag("p2pool.dmax-visible"));
    let uncles_visible = RwSignal::new(crate::app::stored_flag("p2pool.uncles-visible"));
    let weak_visible = RwSignal::new(crate::app::stored_flag("p2pool.weak-visible"));
    // The miners/workers count lines are opt-in like the network line, so
    // they start from their own opt-in flags.
    let miners_visible = RwSignal::new(crate::app::stored_opt_in_flag("p2pool.miners-visible"));
    let workers_visible = RwSignal::new(crate::app::stored_opt_in_flag("p2pool.workers-visible"));
    // The chips write the flip back so the next visit keeps it. `"1"`/`"0"`
    // suits both flags: [stored_flag] reads only `"0"` as off, and
    // [stored_opt_in_flag] reads only `"1"` as on.
    let flip = |signal: RwSignal<bool>, key: &'static str| {
        move |_| {
            let next = !signal.get_untracked();
            signal.set(next);
            let stored = if next { "1" } else { "0" };
            crate::app::store(crate::app::local_storage(), key, stored);
        }
    };
    let hashrate = polled::<Hashrate>(move || format!("/hashrate?window={}", window.get()));
    let sharechain =
        polled::<SharechainSeries>(move || format!("/sharechain?window={}", window.get()));
    let chain = polled::<ChainInfo>(|| "/chain_info".into());
    let blocks = polled::<Vec<FoundBlock>>(|| "/found_blocks".into());
    let miners = polled::<Miners>(|| "/miners".into());
    let peers = polled::<Vec<Peer>>(|| "/peers".into());

    // The live feed moves the tip between polls.
    let tip = Signal::derive(move || {
        let polled = loaded(&chain, |info| info.chain_tip_height).flatten();
        let live = recent.with(|shares| shares.first().map(|share| share.height));
        polled.max(live)
    });

    let found = Memo::new(move |_| {
        loaded(&blocks, |blocks| {
            blocks
                .iter()
                .map(|block| block.share_blockhash.clone())
                .collect::<HashSet<String>>()
        })
        .unwrap_or_default()
    });
    // The overview shows the few newest heights; `recent` stays newest
    // first, and the shares page carries the whole chain. The rows cap
    // keeps the panel a fixed height when the window carries uncles.
    let rows = Signal::derive(move || {
        recent.with(|shares| {
            found.with(|found| capped_share_rows(&shares[..shares.len().min(RECENT_SHARES)], found))
        })
    });
    let history = Memo::new(move |_| {
        loaded(&hashrate, |hashrate| {
            capped_history(hashrate.history.clone())
        })
        .unwrap_or_default()
    });
    // The found blocks inside the chart's window, oldest first, for the
    // diamonds. The window is the selected range around the now-ticking
    // clock (a tick re-filters so aging blocks drop off, as the sample
    // window does); a zoom is left to the chart, which drops blocks
    // outside its own frame when it snaps them to the line.
    let chart_blocks = Memo::new(move |_| {
        let seconds = window.get();
        let now = clock.get();
        loaded(&blocks, |blocks| {
            blocks
                .iter()
                .filter(|block| {
                    block.bitcoin_time >= now.saturating_sub(seconds) && block.bitcoin_time <= now
                })
                .map(|block| FoundBlock {
                    bitcoin_height: block.bitcoin_height,
                    bitcoin_blockhash: block.bitcoin_blockhash.clone(),
                    share_blockhash: block.share_blockhash.clone(),
                    bitcoin_time: block.bitcoin_time,
                    miner_bitcoin_address: block.miner_bitcoin_address.clone(),
                    effort: block.effort.clone(),
                })
                .collect::<Vec<FoundBlock>>()
        })
        .unwrap_or_default()
    });
    // Sharechain series for the chart: `SharechainPoint` tuples mapped
    // to `(u64, f64)` so the drawing code is generic.
    let to_points = |series: &[crate::api::SharechainPoint]| -> Vec<(u64, f64)> {
        series.iter().map(|p| (p.time, p.value)).collect()
    };
    let sc_height =
        Memo::new(move |_| loaded(&sharechain, |data| to_points(&data.height)).unwrap_or_default());
    let sc_dmin = Memo::new(move |_| {
        loaded(&sharechain, |data| to_points(&data.diff_min)).unwrap_or_default()
    });
    let sc_dmax = Memo::new(move |_| {
        loaded(&sharechain, |data| to_points(&data.diff_max)).unwrap_or_default()
    });
    let sc_uncles =
        Memo::new(move |_| loaded(&sharechain, |data| to_points(&data.uncles)).unwrap_or_default());
    let sc_weak =
        Memo::new(move |_| loaded(&sharechain, |data| to_points(&data.weak)).unwrap_or_default());
    let sc_miners =
        Memo::new(move |_| loaded(&sharechain, |data| to_points(&data.miners)).unwrap_or_default());
    let sc_workers = Memo::new(move |_| {
        loaded(&sharechain, |data| to_points(&data.workers)).unwrap_or_default()
    });
    // Harmonize the KPI cards: fall back to the newest non-null history
    // sample when the live read is unknown, so all three cards show a value
    // (or all show the dash) together.
    let pool_rate = Signal::derive(move || {
        loaded(&hashrate, |hashrate| {
            let newest = hashrate.pool_hashrate.or_else(|| {
                hashrate
                    .history
                    .iter()
                    .rev()
                    .find_map(|point| point.pool_hashrate)
            });
            optional(newest, fmt_hashrate)
        })
        .unwrap_or_else(|| UNKNOWN.to_string())
    });
    let error = Signal::derive(move || {
        first_error([
            failure(&hashrate),
            failure(&chain),
            failure(&blocks),
            failure(&miners),
            failure(&peers),
        ])
    });

    let local_note = Signal::derive(move || {
        let totals = loaded(&miners, |miners| {
            format!(
                "{} rejected of {} shares",
                miners.rejected_shares_total, miners.accepted_shares_total
            )
        });
        vec![
            loaded(&miners, |miners| {
                format!(
                    "{} miners, {} workers online",
                    miners.miners_online, miners.workers_online
                )
            })
            .unwrap_or_else(|| format!("{UNKNOWN} miners, {UNKNOWN} workers online")),
            totals.unwrap_or_else(|| format!("{UNKNOWN} rejected of {UNKNOWN} shares")),
        ]
    });
    // The node's overview shows the newest sample that has this node's rate.
    let local_rate = Signal::derive(move || {
        loaded(&hashrate, |hashrate| {
            let newest = hashrate.local_hashrate.or_else(|| {
                hashrate
                    .history
                    .iter()
                    .rev()
                    .find_map(|point| point.local_hashrate)
            });
            optional(newest, fmt_hashrate)
        })
        .unwrap_or_else(|| UNKNOWN.to_string())
    });
    let network_rate = Signal::derive(move || {
        loaded(&hashrate, |hashrate| {
            let newest = hashrate.network_hashrate.or_else(|| {
                hashrate
                    .history
                    .iter()
                    .rev()
                    .find_map(|point| point.network_hashrate)
            });
            optional(newest, fmt_hashrate)
        })
        .unwrap_or_else(|| UNKNOWN.to_string())
    });
    let network_notes = Signal::derive(move || {
        let network_difficulty = loaded(&hashrate, implied_network_difficulty).flatten();
        vec![
            format!(
                "Difficulty {}",
                optional(network_difficulty, fmt_difficulty)
            ),
            "Est. from diff. and block time".to_string(),
        ]
    });
    let pool_notes = Signal::derive(move || vec!["Confirmed shares, last hour".to_string()]);
    let share_notes = Signal::derive(move || {
        let share_difficulty = loaded(&hashrate, |hashrate| hashrate.share_difficulty).flatten();
        vec![format!(
            "Min. Difficulty {}",
            optional(share_difficulty, fmt_difficulty)
        )]
    });
    let height = Signal::derive(move || {
        tip.get()
            .map_or_else(|| UNKNOWN.to_string(), |tip| tip.to_string())
    });
    let blocks_found = Signal::derive(move || {
        loaded(&blocks, |blocks| blocks.len().to_string()).unwrap_or_else(|| UNKNOWN.to_string())
    });
    let blocks_note = Signal::derive(move || {
        let last = loaded(&blocks, |blocks| {
            blocks.first().map(|block| block.bitcoin_time)
        })
        .flatten();
        // Effort: the metrics actor's accumulated work over the network
        // difficulty, live first, then inverted from the sampled network
        // hashrate (the same fallback the difficulty card uses).
        let effort = loaded(&hashrate, |hashrate| {
            let difficulty = hashrate
                .network_difficulty
                .or_else(|| implied_network_difficulty(hashrate));
            fmt_effort(hashrate.work_since_last_block, difficulty)
        });
        vec![
            last.map(|time| format!("Last {}", fmt_ago(clock.get(), time)))
                .unwrap_or_else(|| "None yet".to_string()),
            format!("Effort {}", effort.unwrap_or_else(|| UNKNOWN.to_string())),
        ]
    });

    let range_buttons = WINDOWS
        .iter()
        .map(|&(seconds, label, _, _)| {
            let selected = move || window.get() == seconds;
            let on_click = move |_| {
                window.set(seconds);
                crate::app::store(
                    crate::app::local_storage(),
                    "p2pool.range",
                    &seconds.to_string(),
                );
            };
            view! {
                <button
                    class="outline header-btn"
                    class:active-nav=selected
                    aria-current=move || selected().then_some("true")
                    type="button"
                    on:click=on_click
                >
                    {label}
                </button>
            }
        })
        .collect_view();
    let chart = move || {
        let seconds = window.get();
        let label = WINDOWS
            .iter()
            .find(|(window_seconds, ..)| *window_seconds == seconds)
            .map_or(WINDOWS[0].3, |(.., label)| label);
        if chart_tab.get() == "sharechain" {
            view! {
                <SharechainChart
                    height=sc_height
                    diff_min=sc_dmin
                    diff_max=sc_dmax
                    uncles=sc_uncles
                    weak=sc_weak
                    miners=sc_miners
                    workers=sc_workers
                    blocks=chart_blocks
                    range=zoom
                    label=label
                    height_visible=height_visible
                    dmin_visible=dmin_visible
                    dmax_visible=dmax_visible
                    uncles_visible=uncles_visible
                    weak_visible=weak_visible
                    miners_visible=miners_visible
                    workers_visible=workers_visible
                    blocks_visible=blocks_visible
                />
            }
            .into_any()
        } else {
            view! {
                <HashrateChart
                    history=history
                    blocks=chart_blocks
                    range=zoom
                    label=label
                    pool_visible=pool_visible
                    local_visible=local_visible
                    network_visible=network_visible
                    blocks_visible=blocks_visible
                />
            }
            .into_any()
        }
    };
    let series_toggles = move || {
        if chart_tab.get() == "sharechain" {
            view! {
                <span class="series-toggles" role="group" aria-label="Chart series">
                    <button
                        class="legend-chip"
                        class:off=move || !height_visible.get()
                        type="button"
                        aria-pressed=move || height_visible.get().to_string()
                        on:click=flip(height_visible, "p2pool.height-visible")
                    >
                        <span class="legend-dot height"></span>
                        "Height"
                    </button>
                    <button
                        class="legend-chip"
                        class:off=move || !uncles_visible.get()
                        type="button"
                        aria-pressed=move || uncles_visible.get().to_string()
                        on:click=flip(uncles_visible, "p2pool.uncles-visible")
                    >
                        <span class="legend-dot uncles"></span>
                        "Uncle"
                    </button>
                    <button
                        class="legend-chip"
                        class:off=move || !weak_visible.get()
                        type="button"
                        aria-pressed=move || weak_visible.get().to_string()
                        on:click=flip(weak_visible, "p2pool.weak-visible")
                    >
                        <span class="legend-dot weak"></span>
                        "Weak"
                    </button>
                    <button
                        class="legend-chip"
                        class:off=move || !dmin_visible.get()
                        type="button"
                        aria-pressed=move || dmin_visible.get().to_string()
                        on:click=flip(dmin_visible, "p2pool.dmin-visible")
                    >
                        <span class="legend-dot dmin"></span>
                        "Min diff"
                    </button>
                    <button
                        class="legend-chip"
                        class:off=move || !dmax_visible.get()
                        type="button"
                        aria-pressed=move || dmax_visible.get().to_string()
                        on:click=flip(dmax_visible, "p2pool.dmax-visible")
                    >
                        <span class="legend-dot dmax"></span>
                        "Max diff"
                    </button>
                    <button
                        class="legend-chip"
                        class:off=move || !miners_visible.get()
                        type="button"
                        aria-pressed=move || miners_visible.get().to_string()
                        on:click=flip(miners_visible, "p2pool.miners-visible")
                    >
                        <span class="legend-dot miners"></span>
                        "Miners"
                    </button>
                    <button
                        class="legend-chip"
                        class:off=move || !workers_visible.get()
                        type="button"
                        aria-pressed=move || workers_visible.get().to_string()
                        on:click=flip(workers_visible, "p2pool.workers-visible")
                    >
                        <span class="legend-dot workers"></span>
                        "Workers"
                    </button>
                    <button
                        class="legend-chip"
                        class:off=move || !blocks_visible.get()
                        type="button"
                        aria-pressed=move || blocks_visible.get().to_string()
                        on:click=flip(blocks_visible, "p2pool.blocks-visible")
                    >
                        <span class="legend-dot blocks"></span>
                        "Blocks"
                    </button>
                </span>
            }
            .into_any()
        } else {
            view! {
                <span class="series-toggles" role="group" aria-label="Chart series">
                    <button
                        class="legend-chip"
                        class:off=move || !pool_visible.get()
                        type="button"
                        aria-pressed=move || pool_visible.get().to_string()
                        on:click=flip(pool_visible, "p2pool.pool-visible")
                    >
                        <span class="legend-dot pool"></span>
                        "Pool"
                    </button>
                    <button
                        class="legend-chip"
                        class:off=move || !local_visible.get()
                        type="button"
                        aria-pressed=move || local_visible.get().to_string()
                        on:click=flip(local_visible, "p2pool.local-visible")
                    >
                        <span class="legend-dot local"></span>
                        "Node"
                    </button>
                    <button
                        class="legend-chip"
                        class:off=move || !network_visible.get()
                        type="button"
                        aria-pressed=move || network_visible.get().to_string()
                        on:click=flip(network_visible, "p2pool.network-visible")
                    >
                        <span class="legend-dot network"></span>
                        "Network"
                    </button>
                    <button
                        class="legend-chip"
                        class:off=move || !blocks_visible.get()
                        type="button"
                        aria-pressed=move || blocks_visible.get().to_string()
                        on:click=flip(blocks_visible, "p2pool.blocks-visible")
                    >
                        <span class="legend-dot blocks"></span>
                        "Blocks"
                    </button>
                </span>
            }
            .into_any()
        }
    };
    let reset_zoom = move || {
        zoom.get().map(|_| {
            view! {
                <button
                    class="legend-chip legend-reset reset-zoom"
                    type="button"
                    on:click=move |_| zoom.set(None)
                >
                    "Reset zoom"
                </button>
            }
        })
    };

    let blocks_panel = move || {
        let newest = blocks.with(|blocks| match blocks {
            None => None,
            Some(Err(_)) => Some(Err(())),
            Some(Ok(all)) => Some(Ok(all
                .iter()
                .take(RECENT_BLOCKS)
                .cloned()
                .collect::<Vec<FoundBlock>>())),
        });
        match newest {
            None => loading(),
            Some(Err(())) => view! { <p class="empty-state">{UNKNOWN}</p> }.into_any(),
            Some(Ok(newest)) if newest.is_empty() => {
                view! { <p class="empty-state">{NO_BLOCKS_YET}</p> }.into_any()
            }
            Some(Ok(newest)) => view! {
                {blocks_table(newest)}
                <div class="panel-more">
                    <a href=Route::Blocks { page: 1 }.href()>"All found blocks \u{2192}"</a>
                </div>
            }
            .into_any(),
        }
    };
    let blocks_note_text = move || {
        let total = loaded(&blocks, |all| all.len().to_string());
        format!("{} in all", total.unwrap_or_else(|| UNKNOWN.to_string()))
    };
    let shares_note = move || {
        let tip_note = tip
            .get()
            .map(|tip| format!(" \u{b7} tip {tip}"))
            .unwrap_or_default();
        format!("newest first, uncles under the share that includes them{tip_note}")
    };
    let shares_panel = move || {
        let known_tip = tip.get();
        let answered = chain.with(Option::is_some) || known_tip.is_some();
        if !answered {
            return loading();
        }
        let rows = rows.get();
        if rows.is_empty() {
            let text = if known_tip.is_none() {
                "No confirmed shares yet. Point a miner at this node's stratum port to get started."
            } else {
                "No shares on this page."
            };
            return view! { <p class="empty-state">{text}</p> }.into_any();
        }
        view! {
            {shares_table(rows)}
            <div class="panel-more">
                <a href=Route::Shares { page: 1 }.href()>"More found shares \u{2192}"</a>
            </div>
        }
        .into_any()
    };
    // The note counts the connected peers (the libp2p note stands in while
    // the request has not answered or failed, as the page's empty note does).
    let peers_note = move || {
        let count = peers.with(|value| match value {
            Some(Ok(all)) => Some(all.len()),
            _ => None,
        });
        count
            .map(|count| format!("{count} connected"))
            .unwrap_or_else(|| "libp2p".to_string())
    };
    let peers_panel = move || {
        let loaded = peers.with(|value| match value {
            None => None,
            Some(Err(_)) => Some(Err(())),
            Some(Ok(all)) => Some(Ok(all.clone())),
        });
        match loaded {
            None => loading(),
            Some(Err(())) => view! { <p class="empty-state">{UNKNOWN}</p> }.into_any(),
            Some(Ok(peers_list)) if peers_list.is_empty() => {
                view! { <p class="empty-state">{NO_PEERS_YET}</p> }.into_any()
            }
            Some(Ok(peers_list)) => view! {
                <PeerTable peers=capped_peers(&peers_list) />
                <div class="panel-more">
                    <a href=Route::Peers.href()>"All connected peers \u{2192}"</a>
                </div>
            }
            .into_any(),
        }
    };

    view! {
        {move || error.get().map(|message| view! { <div class="toast" role="alert">{message}</div> })}
        <div class="kpi-grid">
            <KpiCard
                label="Pool hashrate"
                value=pool_rate
                tone=Tone::Green
                notes=pool_notes
            />
            <KpiCard
                label="Node hashrate"
                value=local_rate
                tone=Tone::Local
                href="#/miners"
                notes=local_note
            />
            <KpiCard
                label="network hashrate"
                value=network_rate
                tone=Tone::Net
                notes=network_notes
            />
            <KpiCard label="Share chain height" value=height notes=share_notes />
            <KpiCard
                label="Blocks found"
                value=blocks_found
                tone=Tone::Green
                href="#/blocks"
                notes=blocks_note
            />
        </div>
        <div class="panel">
            <h2 class="hashrate-head">
                <button
                    class="head-switch"
                    class:dim=move || chart_tab.get() != "hashrate"
                    aria-current=move || (chart_tab.get() == "hashrate").then_some("true")
                    type="button"
                    on:click=switch_to_hashrate
                >
                    "Hashrate"
                </button>
                <button
                    class="head-switch"
                    class:dim=move || chart_tab.get() != "sharechain"
                    aria-current=move || (chart_tab.get() == "sharechain").then_some("true")
                    type="button"
                    on:click=switch_to_sharechain
                >
                    "Sharechain"
                </button>
                <span class="range-group" aria-label="Chart time range">
                    {reset_zoom}
                    {range_buttons}
                </span>
            </h2>
            {series_toggles}
            <div class="chart-wrap">{chart}</div>
        </div>
        <div class="panel">
            <h2>"Blocks" <span class="panel-note">{blocks_note_text}</span></h2>
            {blocks_panel}
        </div>
        <div class="panel">
            <h2>"Shares" <span class="panel-note">{shares_note}</span></h2>
            {shares_panel}
        </div>
        <div class="panel">
            <h2>"Peers" <span class="panel-note">{peers_note}</span></h2>
            {peers_panel}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Uncle;

    /// A connected peer as `/peers` reports it, at `address`.
    fn connected_peer(address: &str) -> Peer {
        Peer {
            ip: Some(address.to_string()),
            address: format!("/ip4/{address}/tcp/6886"),
            direction: "Inbound".to_string(),
            connected_secs: 60,
            version: Some("p2poolv2/0.0.1-alpha".to_string()),
            latency_ms: Some(0.5),
        }
    }

    /// A peer variant the address tests reshape freely.
    fn bare_peer(ip: Option<&str>, address: &str) -> Peer {
        Peer {
            ip: ip.map(str::to_string),
            address: address.to_string(),
            direction: "Inbound".to_string(),
            connected_secs: 60,
            version: None,
            latency_ms: None,
        }
    }

    #[test]
    fn test_an_address_is_the_ip_followed_by_the_connection_port() {
        // The live multiaddr keeps going after the port with /p2p/<peer id>.
        let peer = bare_peer(
            Some("104.164.55.69"),
            "/ip4/104.164.55.69/tcp/6887/p2p/12D3KooWExample",
        );
        assert_eq!(peer_address(&peer), "104.164.55.69:6887");
    }

    #[test]
    fn test_an_address_falls_back_to_the_multiaddr_or_the_ip_alone() {
        let no_ip = bare_peer(None, "/ip6/2001:db8::1/tcp/6884");
        assert_eq!(peer_address(&no_ip), "/ip6/2001:db8::1/tcp/6884");

        let no_port = bare_peer(Some("10.0.0.1"), "/dns4/seed.example");
        assert_eq!(peer_address(&no_port), "10.0.0.1");

        assert_eq!(peer_address(&bare_peer(None, "")), UNKNOWN);
    }

    #[test]
    fn test_the_port_is_the_digits_after_tcp_and_nothing_else() {
        assert_eq!(
            peer_port("/ip4/1.2.3.4/tcp/6887/p2p/12D3KooW"),
            Some("6887")
        );
        assert_eq!(peer_port("/ip4/1.2.3.4/tcp/39196"), Some("39196"));
        assert_eq!(peer_port("/dns4/seed.example"), None);
        assert_eq!(peer_port("/ip4/1.2.3.4/tcp//p2p/x"), None);
        assert_eq!(peer_port("/ip4/1.2.3.4/tcp/12ab"), None);
        assert_eq!(peer_port(""), None);
    }

    #[test]
    fn test_uncles_follow_the_share_that_includes_them() {
        let shares = vec![
            Share {
                blockhash: "older".to_string(),
                height: 1,
                miner_bitcoin_address: "bchtest:a".to_string(),
                miner_address: "tp2pool:a".to_string(),
                timestamp: 10,
                bits: 0x1d00_ffff,
                uncles: Vec::new(),
                weak_count: 0,
            },
            Share {
                blockhash: "newer".to_string(),
                height: 2,
                miner_bitcoin_address: "bchtest:b".to_string(),
                miner_address: "tp2pool:b".to_string(),
                timestamp: 20,
                bits: 0x1d00_ffff,
                uncles: vec![Uncle {
                    blockhash: "uncle".to_string(),
                    miner_bitcoin_address: "bchtest:c".to_string(),
                    miner_address: "tp2pool:c".to_string(),
                    timestamp: 15,
                    height: Some(1),
                }],
                weak_count: 0,
            },
        ];
        let found = HashSet::from(["uncle".to_string()]);
        let rows = share_rows(&shares, &found);
        assert_eq!(
            rows.iter()
                .map(|row| row.blockhash.as_str())
                .collect::<Vec<_>>(),
            vec!["newer", "uncle", "older"]
        );
        assert_eq!(rows[0].difficulty, Some(1.0));
        assert!(rows[1].is_uncle);
        assert!(rows[1].is_block);
        assert_eq!(rows[1].difficulty, None);
        assert!(!rows[2].is_block);
    }

    #[test]
    fn test_a_share_row_carries_its_weak_count_and_an_uncle_has_none() {
        let shares = vec![
            Share {
                blockhash: "carrier".to_string(),
                height: 2,
                miner_bitcoin_address: "bchtest:a".to_string(),
                miner_address: "tp2pool:a".to_string(),
                timestamp: 20,
                bits: 0x1d00_ffff,
                uncles: Vec::new(),
                weak_count: 1,
            },
            Share {
                blockhash: "plain".to_string(),
                height: 1,
                miner_bitcoin_address: "bchtest:b".to_string(),
                miner_address: "tp2pool:b".to_string(),
                timestamp: 10,
                bits: 0x1d00_ffff,
                uncles: vec![Uncle {
                    blockhash: "uncle".to_string(),
                    miner_bitcoin_address: "bchtest:c".to_string(),
                    miner_address: "tp2pool:c".to_string(),
                    timestamp: 5,
                    height: Some(1),
                }],
                weak_count: 0,
            },
        ];
        let rows = share_rows(&shares, &HashSet::new());
        assert_eq!(rows[0].blockhash, "carrier");
        assert_eq!(rows[0].weak_count, 1);
        assert_eq!(rows[1].blockhash, "plain");
        assert_eq!(rows[1].weak_count, 0);
        assert!(rows[2].is_uncle);
        assert_eq!(rows[2].weak_count, 0);
    }

    #[test]
    fn test_history_keeps_the_newest_month() {
        let history: Vec<HashratePoint> = (0..MAX_HISTORY_POINTS as u64 + 5)
            .map(|minute| HashratePoint {
                time: minute,
                pool_hashrate: None,
                local_hashrate: None,
                network_hashrate: None,
            })
            .collect();
        let capped = capped_history(history);
        assert_eq!(capped.len(), MAX_HISTORY_POINTS);
        assert_eq!(capped[0].time, 5);
    }

    #[test]
    fn test_recent_shares_hold_five_rows_when_a_share_carries_an_uncle() {
        let shares = vec![
            Share {
                blockhash: "h5".to_string(),
                height: 5,
                miner_bitcoin_address: "bchtest:a".to_string(),
                miner_address: "tp2pool:a".to_string(),
                timestamp: 50,
                bits: 0x1d00_ffff,
                uncles: Vec::new(),
                weak_count: 0,
            },
            Share {
                blockhash: "h4".to_string(),
                height: 4,
                miner_bitcoin_address: "bchtest:b".to_string(),
                miner_address: "tp2pool:b".to_string(),
                timestamp: 40,
                bits: 0x1d00_ffff,
                uncles: vec![Uncle {
                    blockhash: "u4".to_string(),
                    miner_bitcoin_address: "bchtest:c".to_string(),
                    miner_address: "tp2pool:c".to_string(),
                    timestamp: 39,
                    height: Some(4),
                }],
                weak_count: 0,
            },
            Share {
                blockhash: "h3".to_string(),
                height: 3,
                miner_bitcoin_address: "bchtest:d".to_string(),
                miner_address: "tp2pool:d".to_string(),
                timestamp: 30,
                bits: 0x1d00_ffff,
                uncles: Vec::new(),
                weak_count: 0,
            },
            Share {
                blockhash: "h2".to_string(),
                height: 2,
                miner_bitcoin_address: "bchtest:e".to_string(),
                miner_address: "tp2pool:e".to_string(),
                timestamp: 20,
                bits: 0x1d00_ffff,
                uncles: Vec::new(),
                weak_count: 0,
            },
            Share {
                blockhash: "h1".to_string(),
                height: 1,
                miner_bitcoin_address: "bchtest:f".to_string(),
                miner_address: "tp2pool:f".to_string(),
                timestamp: 10,
                bits: 0x1d00_ffff,
                uncles: Vec::new(),
                weak_count: 0,
            },
        ];
        let rows = capped_share_rows(&shares, &HashSet::new());

        assert_eq!(rows.len(), RECENT_ROWS, "the uncle does not grow the panel");
        assert_eq!(
            rows.iter()
                .map(|row| (row.blockhash.as_str(), row.is_uncle))
                .collect::<Vec<_>>(),
            vec![
                ("h5", false),
                ("h4", false),
                ("u4", true),
                ("h3", false),
                ("h2", false),
            ],
            "the uncle stays under the share that includes it and the oldest row drops"
        );
    }

    #[test]
    fn test_recent_shares_without_uncles_keep_all_five_rows() {
        let shares: Vec<Share> = (1..=5)
            .map(|height| Share {
                blockhash: format!("h{height}"),
                height,
                miner_bitcoin_address: "bchtest:a".to_string(),
                miner_address: "tp2pool:a".to_string(),
                timestamp: u64::from(height) * 10,
                bits: 0x1d00_ffff,
                uncles: Vec::new(),
                weak_count: 0,
            })
            .collect();
        let rows = capped_share_rows(&shares, &HashSet::new());

        assert_eq!(rows.len(), RECENT_ROWS);
        assert_eq!(rows[0].height, Some(5));
        assert_eq!(rows[4].height, Some(1));
    }

    #[test]
    fn test_the_peers_panel_keeps_the_five_longest_connected() {
        let peers: Vec<Peer> = ["1", "2", "3", "4", "5", "6"]
            .iter()
            .map(|octet| connected_peer(&format!("10.0.0.{octet}")))
            .collect();
        let capped = capped_peers(&peers);

        assert_eq!(capped.len(), RECENT_PEERS);
        assert_eq!(
            capped
                .iter()
                .map(|peer| peer.ip.clone())
                .collect::<Vec<_>>(),
            vec![
                Some("10.0.0.1".to_string()),
                Some("10.0.0.2".to_string()),
                Some("10.0.0.3".to_string()),
                Some("10.0.0.4".to_string()),
                Some("10.0.0.5".to_string()),
            ],
            "the sixth peer falls off the capped panel, in the endpoint's order"
        );
    }

    #[test]
    fn test_capping_fewer_peers_than_the_cap_keeps_them() {
        let peers = vec![connected_peer("10.0.0.1")];
        assert_eq!(capped_peers(&peers), peers);
        assert!(capped_peers(&Vec::new()).is_empty());
    }

    #[test]
    fn test_the_first_error_is_shown() {
        assert_eq!(
            first_error([
                None,
                Some("a".to_string()),
                Some("b".to_string()),
                None,
                None
            ]),
            Some("a".to_string())
        );
        assert_eq!(first_error([None, None, None, None, None]), None);
    }
}
