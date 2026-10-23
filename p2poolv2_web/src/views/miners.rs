// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The miners on this node and one miner, as `/dashboard/miners` and
//! `/dashboard/miner?address=` show them. Workers open with an HTML
//! `<details>`, as on the node's pages.

use crate::api::{Connection, Miner, Miners, Share, Shares, Worker};
use crate::app::{ctx, fetched};
use crate::format::{
    fmt_difficulty, fmt_hashrate, optional, percent, short, url_component, work_from_bits,
};
use crate::layout::{
    Crumb, ErrorPage, dashboard_crumb, last_seen, loading, lookup_form, pager, share_link, stat,
};
use crate::route::{Paging, Route, Tab, miner_href, share_href};
use leptos::prelude::*;
use std::cmp::Reverse;

/// Shares a miner's hashrate is estimated from, as on the node's miner page.
const MINER_HASHRATE_SHARES: usize = 500;

/// Most shares `/shares` returns at once.
const SHARES_PER_REQUEST: usize = 100;

/// Most window shares `/miner` lists, the node's maximum.
const MINER_SHARES_LIMIT: usize = 100;

/// A miner's hashrate from its share of the work in `shares` (oldest first):
/// its shares' work over the time they span, with how many shares and
/// seconds that was. The oldest share only opens the span, as on the node.
pub fn miner_hashrate(shares: &[Share], address: &str) -> Option<(f64, usize, u64)> {
    let oldest = shares.first()?.timestamp;
    let newest = shares.last()?.timestamp;
    let span = newest.checked_sub(oldest).filter(|span| *span > 0)?;
    let work: f64 = shares
        .iter()
        .skip(1)
        .filter(|share| share.miner_bitcoin_address == address || share.miner_address == address)
        .map(|share| work_from_bits(share.bits))
        .sum();
    Some((work / span as f64, shares.len(), span))
}

/// The newest `count` confirmed shares, oldest first, read a page of
/// `/shares` at a time.
async fn newest_shares(connection: &Connection, count: usize) -> Result<Vec<Share>, String> {
    let mut collected: Vec<Share> = Vec::with_capacity(count);
    let mut below: Option<u32> = None;
    let mut more = true;
    while more && collected.len() < count {
        let wanted = (count - collected.len()).min(SHARES_PER_REQUEST);
        let path = match below {
            Some(height) => format!("/shares?to={height}&num={wanted}"),
            None => format!("/shares?num={wanted}"),
        };
        let page = connection.get::<Shares>(&path).await?.shares;
        let lowest = page.iter().map(|share| share.height).min();
        more = !page.is_empty() && lowest.is_some_and(|lowest| lowest > 0);
        below = lowest.map(|lowest| lowest.saturating_sub(1));
        collected.extend(page);
    }
    collected.sort_by_key(|share| share.height);
    Ok(collected)
}

/// A miner's workers, as rows of the miners grid.
fn worker_rows(workers: Vec<Worker>) -> impl IntoView {
    workers
        .into_iter()
        .map(|worker| {
            view! {
                <div class="grid-row worker">
                    <span></span>
                    <span class="mono">{worker.name}</span>
                    <span>{if worker.active { "online" } else { "idle" }}</span>
                    <span>{fmt_difficulty(worker.accepted_work as f64)}</span>
                    <span></span>
                    <span>"best " {fmt_difficulty(worker.best_share as f64)}</span>
                    {last_seen(worker.last_share_at)}
                    <span></span>
                </div>
            }
        })
        .collect_view()
}

fn miners_grid(miners: Miners) -> impl IntoView {
    let first_rank = (miners.page.max(1) - 1) * miners.page_size;
    let rows = miners
        .miners
        .into_iter()
        .enumerate()
        .map(|(index, miner)| {
            let active = miner.workers.iter().filter(|worker| worker.active).count();
            let total = miner.workers.len();
            let workers = if miner.workers.is_empty() {
                view! { <p class="worker-empty">"No worker-level stats for this address."</p> }
                    .into_any()
            } else {
                worker_rows(miner.workers).into_any()
            };
            view! {
                <details>
                    <summary class="grid-row">
                        <span>{first_rank + index + 1}</span>
                        <a class="addr" href=miner_href(&miner.address) title=miner.address.clone()>
                            {miner.address.clone()}
                        </a>
                        <span>{format!("{active} / {total}")}</span>
                        <span>{fmt_difficulty(miner.accepted_work as f64)}</span>
                        <span>{optional(miner.payout_fraction, percent)}</span>
                        <span>{fmt_difficulty(miner.best_share as f64)}</span>
                        {last_seen(miner.last_share_at)}
                        <span class="toggle" aria-label="Workers"></span>
                    </summary>
                    <div class="worker-list">{workers}</div>
                </details>
            }
        })
        .collect_view();
    view! {
        <div class="grid-table miners-grid">
            <div class="grid-head">
                <span>"#"</span>
                <span>"Payout address"</span>
                <span>"Workers"</span>
                <span>"Accepted work"</span>
                <span>"Window share"</span>
                <span>"Best share"</span>
                <span>"Last seen"</span>
                <span></span>
            </div>
            {rows}
        </div>
        {pager(miners.page, miners.pages, |page| Route::Miners { page })}
    }
}

#[component]
pub fn MinersPage(page: usize) -> impl IntoView {
    ctx().frame.set(
        "Miners",
        Tab::Miners,
        vec![dashboard_crumb(), Crumb::here("Miners")],
    );
    let miners = fetched::<Miners>(move || format!("/miners?page={page}"));
    move || {
        match miners.get() {
        None => view! {
            <div class="panel">
                <h2>"Miners"</h2>
                {loading()}
            </div>
        }
        .into_any(),
        Some(Err(message)) => view! { <ErrorPage message=message /> }.into_any(),
        Some(Ok(miners)) => view! {
            <div class="panel">
                <h2>
                    "Miners"
                    <span class="panel-note">
                        "mining on this node; window share is across the whole pool. Open a row for its workers."
                    </span>
                </h2>
                {if miners.miners.is_empty() {
                    view! {
                        <p class="empty-state">"No miners have submitted shares to this node yet."</p>
                    }
                        .into_any()
                } else {
                    miners_grid(miners).into_any()
                }}
            </div>
        }
        .into_any(),
    }
    }
}

/// The table of a miner's window shares, a page of them.
fn window_shares(report: &Miner, page: usize) -> impl IntoView + use<> {
    let paging = Paging::of(page, report.shares.len());
    let shares = paging.slice(&report.shares);
    if shares.is_empty() {
        return view! {
            <p class="empty-state">"This address has no shares in the current window."</p>
        }
        .into_any();
    }
    let rows = shares
        .iter()
        .map(|share| {
            let is_uncle = share.nephew.is_some();
            view! {
                <tr class:uncle-row=is_uncle>
                    <td>{share.height}</td>
                    <td>{share_link(&share.blockhash)}</td>
                    <td>{if is_uncle { "uncle" } else { "share" }}</td>
                    <td class="mono">{share.difficulty.clone()}</td>
                    <td class="mono">{share.weight.clone()}</td>
                </tr>
            }
        })
        .collect_view();
    let address = report.address.clone();
    view! {
        <div class="table-x">
            <table class="table">
                <thead>
                    <tr>
                        <th>"Height"</th>
                        <th>"Share"</th>
                        <th>"Kind"</th>
                        <th>"Difficulty"</th>
                        <th>"Weight"</th>
                    </tr>
                </thead>
                <tbody>{rows}</tbody>
            </table>
        </div>
        {pager(
            paging.page,
            paging.pages,
            move |page| Route::Miner {
                address: address.clone(),
                page,
            },
        )}
    }
    .into_any()
}

#[component]
pub fn MinerPage(address: String, page: usize) -> impl IntoView {
    let address = address.trim().to_string();
    if address.is_empty() {
        return view! { <ErrorPage message="Enter a payout or share chain address.".to_string() /> }
            .into_any();
    }
    let context = ctx();
    context.frame.set(
        format!("Miner {}", short(&address)),
        Tab::Miners,
        vec![
            dashboard_crumb(),
            Crumb::link("Miners", Route::Miners { page: 1 }.href()),
            Crumb::here(short(&address)),
        ],
    );
    let encoded = url_component(&address);
    let report_path = format!("/miner?address={encoded}&limit={MINER_SHARES_LIMIT}");
    let report = fetched::<Miner>(move || report_path.clone());
    let local = fetched::<Miners>(move || format!("/miners?address={encoded}"));
    let connection = context.connection;
    // The hashrate is estimated from the newest shares, for the address the
    // window lookup returned, as the node's miner page does.
    let estimate = LocalResource::new(move || {
        let connection = connection.get();
        let reported = report
            .get()
            .and_then(Result::ok)
            .map(|report| report.address);
        async move {
            let reported = reported?;
            let mut shares = newest_shares(&connection, MINER_HASHRATE_SHARES)
                .await
                .ok()?;
            shares.sort_by_key(|share| Reverse(share.height));
            shares.truncate(MINER_HASHRATE_SHARES);
            shares.reverse();
            miner_hashrate(&shares, &reported)
        }
    });

    (move || match report.get() {
        None => view! { <div class="panel">{loading()}</div> }.into_any(),
        Some(Err(message)) => view! { <ErrorPage message=message /> }.into_any(),
        Some(Ok(report)) => {
            let kind = if report.address_type == "share" {
                "share chain address"
            } else {
                "payout address"
            };
            let in_window = format!(
                "{} of {} ({})",
                report.share_count,
                report.window.share_count,
                percent(report.share_fraction)
            );
            let hashrate = move || {
                let found = estimate.get().flatten();
                let window = found.map(|(_, shares, seconds)| {
                    view! {
                        <br />
                        <small>{format!("over the last {shares} shares, {} min", seconds / 60)}</small>
                    }
                });
                view! {
                    {optional(found.map(|(hashrate, _, _)| hashrate), fmt_hashrate)}
                    {window}
                }
            };
            let workers = move || match local.get() {
                Some(Ok(local)) => local
                    .miners
                    .into_iter()
                    .next()
                    .filter(|miner| !miner.workers.is_empty())
                    .map(|miner| {
                        view! {
                            <div class="panel">
                                <h2>"Workers on this node"</h2>
                                <div class="grid-table miners-grid">
                                    <div class="grid-head">
                                        <span></span>
                                        <span>"Worker"</span>
                                        <span>"State"</span>
                                        <span>"Accepted work"</span>
                                        <span></span>
                                        <span>"Best share"</span>
                                        <span>"Last seen"</span>
                                        <span></span>
                                    </div>
                                    {worker_rows(miner.workers)}
                                </div>
                            </div>
                        }
                    }),
                _ => None,
            };
            view! {
                <div class="panel">{lookup_form(&report.address)}</div>
                <div class="panel">
                    <h2>"Miner" <span class="panel-note">{kind}</span></h2>
                    <div class="stat-list">
                        {stat(
                            "Address",
                            view! { <span class="mono">{report.address.clone()}</span> }.into_any(),
                        )}
                        {stat(
                            "Share of the next block's payout",
                            view! { {percent(report.payout_fraction)} }.into_any(),
                        )}
                        {stat("Shares in the window", view! { {in_window} }.into_any())}
                        {stat("Uncles credited", view! { {report.uncle_count} }.into_any())}
                        {stat("Hashrate", view! { {hashrate} }.into_any())}
                        {stat(
                            "Mining on",
                            view! {
                                <a href=share_href(&report.anchor)>
                                    {format!("share {}", report.anchor_height)}
                                </a>
                            }
                                .into_any(),
                        )}
                    </div>
                </div>
                {workers}
                <div class="panel">
                    <h2>"Shares credited in the window" <span class="panel-note">"newest first"</span></h2>
                    {window_shares(&report, page)}
                </div>
            }
            .into_any()
        }
    })
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn share(height: u32, timestamp: u64, address: &str) -> Share {
        Share {
            blockhash: format!("{height}"),
            height,
            miner_bitcoin_address: address.to_string(),
            miner_address: "tp2pool:owner".to_string(),
            timestamp,
            bits: 0x1d00_ffff,
            uncles: Vec::new(),
        }
    }

    #[test]
    fn test_miner_hashrate_leaves_out_the_oldest_shares_work() {
        let shares = [
            share(1, 0, "bchtest:me"),
            share(2, 10, "bchtest:me"),
            share(3, 20, "bchtest:other"),
        ];
        let (hashrate, count, span) = miner_hashrate(&shares, "bchtest:me").unwrap();
        assert!((hashrate - work_from_bits(0x1d00_ffff) / 20.0).abs() < 1e-6);
        assert_eq!((count, span), (3, 20));
    }

    #[test]
    fn test_miner_hashrate_matches_the_share_owner_too() {
        let shares = [share(1, 0, "bchtest:a"), share(2, 10, "bchtest:b")];
        let (hashrate, _, _) = miner_hashrate(&shares, "tp2pool:owner").unwrap();
        assert!(hashrate > 0.0);
    }

    #[test]
    fn test_miner_hashrate_needs_time_between_shares() {
        assert_eq!(miner_hashrate(&[share(1, 5, "x")], "x"), None);
        assert_eq!(miner_hashrate(&[], "x"), None);
    }
}
