// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The found blocks list and one found block, as `/dashboard/blocks` and
//! `/dashboard/block/<hash>` show them.

use crate::api::FoundBlock;
use crate::app::{ctx, fetched};
use crate::format::{fmt_ago, fmt_utc, short};
use crate::layout::{
    Crumb, ErrorPage, ago_cell, dashboard_crumb, loading, miner_link, pager, share_link, stat,
};
use crate::route::{Paging, Route, Tab, block_href, looks_like_hash, share_href};
use leptos::prelude::*;

/// The found blocks table of the overview and the blocks page.
pub fn blocks_table(blocks: Vec<FoundBlock>) -> impl IntoView {
    let rows = blocks
        .into_iter()
        .map(|block| {
            let href = block_href(&block.bitcoin_blockhash);
            view! {
                <tr>
                    <td>
                        <a href=href.clone()>{block.bitcoin_height}</a>
                    </td>
                    <td>
                        <a class="addr" href=href title=block.bitcoin_blockhash.clone()>
                            {short(&block.bitcoin_blockhash)}
                        </a>
                    </td>
                    <td>{share_link(&block.share_blockhash)}</td>
                    <td>{miner_link(&block.miner_bitcoin_address)}</td>
                    {ago_cell(block.bitcoin_time)}
                </tr>
            }
        })
        .collect_view();
    view! {
        <div class="table-x">
            <table class="table">
                <thead>
                    <tr>
                        <th>"Height"</th>
                        <th>"Block hash"</th>
                        <th>"Share"</th>
                        <th>"Found by"</th>
                        <th>"Found"</th>
                    </tr>
                </thead>
                <tbody>{rows}</tbody>
            </table>
        </div>
    }
}

/// What the overview and the blocks page say before the first block.
pub const NO_BLOCKS_YET: &str = "The pool has not found a Bitcoin Cash block yet. Keep mining.";

#[component]
pub fn BlocksPage(page: usize) -> impl IntoView {
    ctx().frame.set(
        "Blocks",
        Tab::Blocks,
        vec![dashboard_crumb(), Crumb::here("Blocks")],
    );
    let blocks = fetched::<Vec<FoundBlock>>(|| "/found_blocks".into());
    move || match blocks.get() {
        None => view! {
            <div class="panel">
                <h2>"Found blocks"</h2>
                {loading()}
            </div>
        }
        .into_any(),
        Some(Err(message)) => view! { <ErrorPage message=message /> }.into_any(),
        Some(Ok(all)) => {
            let paging = Paging::of(page, all.len());
            let total = all.len();
            let shown = paging.slice(&all).to_vec();
            view! {
                <div class="panel">
                    <h2>
                        "Found blocks"
                        <span class="panel-note">{format!("{total} in all, newest first")}</span>
                    </h2>
                    {if shown.is_empty() {
                        view! { <p class="empty-state">{NO_BLOCKS_YET}</p> }.into_any()
                    } else {
                        view! {
                            {blocks_table(shown)}
                            {pager(paging.page, paging.pages, |page| Route::Blocks { page })}
                        }
                        .into_any()
                    }}
                </div>
            }
            .into_any()
        }
    }
}

#[component]
pub fn BlockPage(hash: String) -> impl IntoView {
    if !looks_like_hash(&hash) {
        return view! { <ErrorPage message=format!("Not a block hash: {hash}") /> }.into_any();
    }
    let context = ctx();
    let label = format!("Block {}", short(&hash));
    context.frame.set(
        label.clone(),
        Tab::Blocks,
        vec![
            dashboard_crumb(),
            Crumb::link("Blocks", Route::Blocks { page: 1 }.href()),
            Crumb::here(label),
        ],
    );
    let clock = context.clock;
    let blocks = fetched::<Vec<FoundBlock>>(|| "/found_blocks".into());
    (move || match blocks.get() {
        None => loading(),
        Some(Err(message)) => view! { <ErrorPage message=message /> }.into_any(),
        Some(Ok(all)) => {
            let Some(block) = all
                .into_iter()
                .find(|block| block.bitcoin_blockhash.eq_ignore_ascii_case(&hash))
            else {
                let message = format!("The pool has not found a block with hash {hash}.");
                return view! { <ErrorPage message=message /> }.into_any();
            };
            let title = format!("Block {}", block.bitcoin_height);
            context.frame.set(
                title.clone(),
                Tab::Blocks,
                vec![
                    dashboard_crumb(),
                    Crumb::link("Blocks", Route::Blocks { page: 1 }.href()),
                    Crumb::here(title),
                ],
            );
            let time = block.bitcoin_time;
            view! {
                <a class="back-link" href=Route::Blocks { page: 1 }.href()>
                    "\u{2190} All found blocks"
                </a>
                <div class="panel">
                    <h2>
                        {format!("Block {}", block.bitcoin_height)}
                        <span class="panel-note">"found by the pool"</span>
                    </h2>
                    <div class="stat-list">
                        {stat("Height", view! { {block.bitcoin_height} }.into_any())}
                        {stat(
                            "Found",
                            view! { {move || fmt_ago(clock.get(), time)} }.into_any(),
                        )}
                        {stat("Time", view! { {fmt_utc(time)} }.into_any())}
                        {stat("Found by", miner_link(&block.miner_bitcoin_address).into_any())}
                    </div>
                </div>
                <div class="panel">
                    <h2>"Hashes"</h2>
                    <div class="stat-list">
                        {stat(
                            "Block hash",
                            view! { <span class="mono">{block.bitcoin_blockhash.clone()}</span> }
                                .into_any(),
                        )}
                        {stat(
                            "Share carrying it",
                            view! {
                                <a class="addr" href=share_href(&block.share_blockhash)>
                                    {block.share_blockhash.clone()}
                                </a>
                            }
                                .into_any(),
                        )}
                    </div>
                </div>
            }
            .into_any()
        }
    })
    .into_any()
}
