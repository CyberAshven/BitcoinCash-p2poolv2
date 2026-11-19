// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The miners on this node and one miner, as `/dashboard/miners` and
//! `/dashboard/miner?address=` show them. Workers open with an HTML
//! `<details>`, as on the node's pages.

use crate::api::{Miner, Miners, Worker};
use crate::app::{ctx, fetched};
use crate::format::{fmt_difficulty, fmt_hashrate, optional, percent, short, url_component};
use crate::layout::{
    Crumb, ErrorPage, dashboard_crumb, last_seen, loading, lookup_form, pager, share_link, stat,
};
use crate::route::{Paging, Route, Tab, miner_href, share_href};
use leptos::prelude::*;

/// Most window shares `/miner` lists, the node's maximum.
const MINER_SHARES_LIMIT: usize = 100;

/// A miner's workers, as rows of the miners grid. `hashrate` adds the
/// worker's own stratum-session figure beside State, as on the node's
/// miners page; the miner page's table has no such column.
fn worker_rows(workers: Vec<Worker>, hashrate: bool) -> impl IntoView {
    workers
        .into_iter()
        .map(|worker| {
            view! {
                <div class="grid-row worker">
                    <span></span>
                    <span class="mono">{worker.name}</span>
                    <span>{if worker.active { "online" } else { "idle" }}</span>
                    {hashrate.then(|| view! {
                        <span>{optional(worker.hashrate, fmt_hashrate)}</span>
                    })}
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
                worker_rows(miner.workers, true).into_any()
            };
            view! {
                <details>
                    <summary class="grid-row">
                        <span>{first_rank + index + 1}</span>
                        <a class="addr" href=miner_href(&miner.address) title=miner.address.clone()>
                            {miner.address.clone()}
                        </a>
                        <span>{format!("{active} / {total}")}</span>
                        <span>{optional(miner.hashrate, fmt_hashrate)}</span>
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
        <div class="grid-table miners-grid hashrate-col">
            <div class="grid-head">
                <span>"#"</span>
                <span>"Payout address"</span>
                <span>"Workers"</span>
                <span>"Hashrate"</span>
                <span>"Work"</span>
                <span>"Window"</span>
                <span>"Best"</span>
                <span>"Seen"</span>
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

/// The carried and credited weak-share totals for the Uncles credited
/// tile, or nothing when both are 0 so the tile stays what `stat()`
/// emits. The carried side sums the whole report, not the paged slice.
fn weak_summary(report: &Miner) -> Option<(usize, usize)> {
    let carried = report
        .shares
        .iter()
        .map(|share| usize::from(share.weak_count))
        .sum();
    let credited = report.weak_share_count;
    (carried > 0 || credited > 0).then_some((carried, credited))
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
            // A weak credit: an uncle whose nephew is itself, as the node
            // marks the credited row.
            let is_weak = share.nephew.as_deref() == Some(share.blockhash.as_str());
            view! {
                <tr class:uncle-row=is_uncle>
                    <td>{share.height}</td>
                    <td>{share_link(&share.blockhash)}</td>
                    <td>
                        {if is_weak { "weak" } else if is_uncle { "uncle" } else { "share" }}
                        {(share.weak_count > 0)
                            .then(|| {
                                view! {
                                    " "
                                    <span class="badge-weak">{format!("{} weak", share.weak_count)}</span>
                                }
                            })}
                    </td>
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
            let workers = move || match local.get() {
                Some(Ok(local)) => local
                    .miners
                    .into_iter()
                    .next()
                    .filter(|miner| !miner.workers.is_empty())
                    .map(|miner| {
                        view! {
                            <div class="panel">
                                <h2>"Workers"</h2>
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
                                    {worker_rows(miner.workers, false)}
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
                        {match weak_summary(&report) {
                            Some((carried, credited)) => view! {
                                <div class="stat-item">
                                    <div class="k">"Uncles credited"</div>
                                    <div class="v">{report.uncle_count}</div>
                                    <div class="k">"Weak shares"</div>
                                    <small>{format!("{carried} carried, {credited} credited")}</small>
                                </div>
                            }
                            .into_any(),
                            None => {
                                stat("Uncles credited", view! { {report.uncle_count} }.into_any())
                                    .into_any()
                            }
                        }}
                        {report
                            .payout_level
                            .zip(report.payout_level_satoshis)
                            .map(|(level, satoshis)| {
                                stat(
                                    "Payout level",
                                    view! {
                                        {format!("{level} ({satoshis} satoshis)")}
                                        <br />
                                        <small>"smaller slices are paid by a draw"</small>
                                    }
                                    .into_any(),
                                )
                            })}
                        {report.expected_income_a_month.map(|income| {
                            stat(
                                "Expected income a month",
                                view! {
                                    {format!("{income:.0} satoshis")}
                                    <br />
                                    <small>"from the pool's recent block rate"</small>
                                }
                                .into_any(),
                            )
                        })}
                        {report
                            .expected_payments_a_month
                            .map(|payments| {
                                stat(
                                    "Expected payments a month",
                                    view! { {format!("{payments:.1}")} }.into_any(),
                                )
                            })}
                        {report.monthly_swing.map(|swing| {
                            stat(
                                "Monthly swing",
                                view! {
                                    {percent(swing)}
                                    <br />
                                    <small>
                                        "how far a month's income typically strays from the expected"
                                    </small>
                                }
                                .into_any(),
                            )
                        })}
                        {stat(
                            "Hashrate",
                            view! {
                                {optional(report.hashrate, fmt_hashrate)}
                                {report.recent_window.map(|(count, seconds)| view! {
                                    <br />
                                    <small>
                                        {format!("over the last {count} shares, {} min", seconds / 60)}
                                    </small>
                                })}
                            }
                                .into_any(),
                        )}
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
                    <h2>"Shares" <span class="panel-note">"newest first"</span></h2>
                    {window_shares(&report, page)}
                </div>
            }
            .into_any()
        }
    })
    .into_any()
}
