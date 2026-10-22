// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The overview, as `/dashboard` shows it: KPI cards, the hashrate chart,
//! the newest found blocks and a page of the share chain. Its first page of
//! shares is live, from `/ws`; the rest refreshes on the node's 30 seconds.

use super::blocks::{NO_BLOCKS_YET, blocks_table};
use crate::api::{ChainInfo, FoundBlock, Hashrate, HashratePoint, Miners, Share, Shares};
use crate::app::{Ctx, ctx, polled};
use crate::chart::HashrateChart;
use crate::format::{
    UNKNOWN, difficulty_from_bits, difficulty_from_network_hashrate, fmt_ago, fmt_difficulty,
    fmt_hashrate, optional,
};
use crate::layout::{KpiCard, Tone, ago_cell, loading, miner_link, pager, share_link};
use crate::route::{PAGE_SIZE, Paging, Route, Tab};
use leptos::prelude::*;
use std::cmp::Reverse;
use std::collections::HashSet;

/// Chart windows offered: seconds, button, what the panel note calls it and
/// the chart's name. The first is the node's chart, the last day.
const WINDOWS: [(u64, &str, &str, &str); 3] = [
    (
        86_400,
        "24h",
        "one sample a minute, the last day",
        "Hashrate over the last day",
    ),
    (
        21_600,
        "6h",
        "one sample a minute, the last 6 hours",
        "Hashrate over the last 6 hours",
    ),
    (
        3_600,
        "1h",
        "one sample a minute, the last hour",
        "Hashrate over the last hour",
    ),
];

/// Most hashrate samples kept for the chart: a day at one a minute, the most
/// the node's history holds.
pub const MAX_HISTORY_POINTS: usize = 1_440;

/// Found blocks the overview lists; the blocks page has them all.
const RECENT_BLOCKS: usize = 5;

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

fn shares_table(rows: Vec<ShareRow>) -> impl IntoView {
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
pub fn OverviewPage(page: usize) -> impl IntoView {
    let context = ctx();
    context.frame.set("Dashboard", Tab::Dashboard, Vec::new());
    let Ctx {
        connection,
        clock,
        recent,
        ..
    } = context;

    let window = RwSignal::new(WINDOWS[0].0);
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
    let paging =
        Signal::derive(move || Paging::of(page, tip.get().map_or(0, |tip| tip as usize + 1)));
    // Older pages are read from `/shares` below the polled tip, so they move
    // on the node's refresh; the first page is the live list.
    let older = LocalResource::new(move || {
        let connection = connection.get();
        let newest = loaded(&chain, |info| info.chain_tip_height).flatten();
        async move {
            let skipped =
                newest.map_or(0, |newest| Paging::of(page, newest as usize + 1).skipped());
            match newest {
                Some(newest) if skipped > 0 => {
                    let to = newest.saturating_sub(u32::try_from(skipped).unwrap_or(u32::MAX));
                    let path = format!("/shares?to={to}&num={PAGE_SIZE}");
                    Some(connection.get::<Shares>(&path).await)
                }
                _ => None,
            }
        }
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
    let rows = Signal::derive(move || {
        if page <= 1 {
            return Some(recent.with(|shares| found.with(|found| share_rows(shares, found))));
        }
        older.with(|older| match older {
            Some(Some(Ok(shares))) => Some(found.with(|found| share_rows(&shares.shares, found))),
            _ => None,
        })
    });
    let history = Memo::new(move |_| {
        loaded(&hashrate, |hashrate| {
            capped_history(hashrate.history.clone())
        })
        .unwrap_or_default()
    });
    let rate = move |pick: fn(&Hashrate) -> Option<f64>| {
        Signal::derive(move || {
            loaded(&hashrate, |hashrate| optional(pick(hashrate), fmt_hashrate))
                .unwrap_or_else(|| UNKNOWN.to_string())
        })
    };
    let error = Signal::derive(move || {
        first_error([
            failure(&hashrate),
            failure(&chain),
            failure(&blocks),
            failure(&miners),
        ])
    });

    let local_note = Signal::derive(move || {
        vec![
            loaded(&miners, |miners| {
                format!(
                    "{} miners, {} workers online",
                    miners.miners_online, miners.workers_online
                )
            })
            .unwrap_or_else(|| format!("{UNKNOWN} miners, {UNKNOWN} workers online")),
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
    let network_notes = Signal::derive(move || {
        let difficulty = loaded(&hashrate, |hashrate| {
            hashrate
                .network_hashrate
                .map(difficulty_from_network_hashrate)
        })
        .flatten();
        vec![
            format!("Difficulty {}", optional(difficulty, fmt_difficulty)),
            "Estimated from difficulty, 10-minute blocks".to_string(),
        ]
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

    let window_note = move || {
        WINDOWS
            .iter()
            .find(|(seconds, ..)| *seconds == window.get())
            .map_or(WINDOWS[0].2, |(_, _, note, _)| note)
    };
    let range_buttons = WINDOWS
        .iter()
        .map(|&(seconds, label, _, _)| {
            view! {
                <button
                    class="range-btn"
                    class:active=move || window.get() == seconds
                    type="button"
                    on:click=move |_| window.set(seconds)
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
        view! { <HashrateChart history=history label=label /> }
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
        match rows.get() {
            _ if !answered => loading(),
            None => loading(),
            Some(rows) if rows.is_empty() => {
                let text = if known_tip.is_none() {
                    "No confirmed shares yet. Point a miner at this node's stratum port to get started."
                } else {
                    "No shares on this page."
                };
                view! { <p class="empty-state">{text}</p> }.into_any()
            }
            Some(rows) => {
                let Paging { page, pages } = paging.get();
                view! {
                    {shares_table(rows)}
                    {pager(page, pages, |page| Route::Dashboard { page })}
                }
                .into_any()
            }
        }
    };

    view! {
        {move || error.get().map(|message| view! { <div class="toast" role="alert">{message}</div> })}
        <div class="kpi-grid">
            <KpiCard
                label="Pool hashrate"
                value=rate(|hashrate| hashrate.pool_hashrate)
                tone=Tone::Green
                notes=vec!["From the newest shares on the share chain".to_string()]
            />
            <KpiCard
                label="This node"
                value=local_rate
                tone=Tone::Local
                href="#/miners"
                notes=local_note
            />
            <KpiCard
                label="Network hashrate (BCH)"
                value=rate(|hashrate| hashrate.network_hashrate)
                tone=Tone::Net
                notes=network_notes
            />
            <KpiCard label="Share chain height" value=height notes={Vec::<String>::new()} />
            <KpiCard label="Blocks found" value=blocks_found href="#/blocks" notes=blocks_note />
        </div>
        <div class="panel">
            <h2>
                "Hashrate"
                <span class="panel-note">{window_note}</span>
                <span class="range-btns">{range_buttons}</span>
            </h2>
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
    fn test_history_keeps_the_newest_day() {
        let history: Vec<HashratePoint> = (0..MAX_HISTORY_POINTS as u64 + 5)
            .map(|minute| HashratePoint {
                time: minute,
                pool_hashrate: None,
                local_hashrate: None,
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
