// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The overview, as `/dashboard` shows it: KPI cards, the hashrate chart,
//! the newest found blocks and the newest few share heights. Its shares are
//! live, from `/ws`; the shares page has the whole chain.

use super::blocks::{NO_BLOCKS_YET, blocks_table};
use crate::api::{ChainInfo, FoundBlock, Hashrate, HashratePoint, Miners, Share};
use crate::app::{Ctx, ctx, polled};
use crate::chart::HashrateChart;
use crate::format::{
    UNKNOWN, difficulty_from_bits, difficulty_from_network_hashrate, fmt_ago, fmt_difficulty,
    fmt_hashrate, optional,
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
/// page has them all.
const RECENT_SHARES: usize = 5;

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
        }));
    }
    rows
}

/// The newest `MAX_HISTORY_POINTS` samples of `history`.
pub fn capped_history(mut history: Vec<HashratePoint>) -> Vec<HashratePoint> {
    let excess = history.len().saturating_sub(MAX_HISTORY_POINTS);
    history.drain(..excess);
    history
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
fn first_error(errors: [Option<String>; 4]) -> Option<String> {
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
    let chain = polled::<ChainInfo>(|| "/chain_info".into());
    let blocks = polled::<Vec<FoundBlock>>(|| "/found_blocks".into());
    let miners = polled::<Miners>(|| "/miners".into());

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
    // first, and the shares page carries the whole chain.
    let rows = Signal::derive(move || {
        recent.with(|shares| {
            found.with(|found| share_rows(&shares[..shares.len().min(RECENT_SHARES)], found))
        })
    });
    let history = Memo::new(move |_| {
        loaded(&hashrate, |hashrate| {
            capped_history(hashrate.history.clone())
        })
        .unwrap_or_default()
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
        let network_difficulty = loaded(&hashrate, |hashrate| {
            hashrate
                .network_hashrate
                .map(difficulty_from_network_hashrate)
                .or_else(|| {
                    hashrate
                        .history
                        .iter()
                        .rev()
                        .find_map(|point| point.network_hashrate)
                        .map(difficulty_from_network_hashrate)
                })
        })
        .flatten();
        vec![
            format!(
                "Difficulty {}",
                optional(network_difficulty, fmt_difficulty)
            ),
            "Est. from diff. 10-min blocks".to_string(),
        ]
    });
    let pool_notes = Signal::derive(move || vec!["Confirmed shares, last hour".to_string()]);
    let share_notes = Signal::derive(move || {
        let share_difficulty = loaded(&hashrate, |hashrate| hashrate.share_difficulty).flatten();
        vec![format!(
            "Difficulty {}",
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
        vec![
            last.map(|time| format!("Last {}", fmt_ago(clock.get(), time)))
                .unwrap_or_else(|| "None yet".to_string()),
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
        view! {
            <HashrateChart
                history=history
                range=zoom
                label=label
                pool_visible=pool_visible
                local_visible=local_visible
                network_visible=network_visible
            />
        }
    };
    let series_toggles = move || {
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
            </span>
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
                "Hashrate"
                <span class="range-group" aria-label="Chart time range">
                    {reset_zoom}
                    {range_buttons}
                </span>
            </h2>
            {series_toggles}
            <div class="chart-wrap">{chart}</div>
        </div>
        <div class="panel">
            <h2>"Found blocks" <span class="panel-note">{blocks_note_text}</span></h2>
            {blocks_panel}
        </div>
        <div class="panel">
            <h2>"Recent shares" <span class="panel-note">{shares_note}</span></h2>
            {shares_panel}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Uncle;

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
    fn test_the_first_error_is_shown() {
        assert_eq!(
            first_error([None, Some("a".to_string()), Some("b".to_string()), None]),
            Some("a".to_string())
        );
        assert_eq!(first_error([None, None, None, None]), None);
    }
}
