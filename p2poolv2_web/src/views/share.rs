// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! One share, as `/dashboard/share/<hash>` shows it.

use crate::api::{FoundBlock, ShareDetail};
use crate::app::{ctx, fetched};
use crate::format::{UNKNOWN, short, url_component};
use crate::layout::{Crumb, ErrorPage, dashboard_crumb, loading, miner_link, share_link, stat};
use crate::route::{Route, Tab, looks_like_hash, miner_href};
use leptos::prelude::*;

/// The trail to a share, through its miner once the share is known.
fn share_trail(hash: &str, owner: Option<&str>) -> Vec<Crumb> {
    let mut trail = vec![
        dashboard_crumb(),
        Crumb::link("Miners", Route::Miners { page: 1 }.href()),
    ];
    if let Some(owner) = owner {
        trail.push(Crumb::link(short(owner), miner_href(owner)));
    }
    trail.push(Crumb::here(format!("Share {}", short(hash))));
    trail
}

fn share_view(share: ShareDetail, is_block: bool) -> impl IntoView {
    let header = share.bitcoin_header;
    let owner = share.miner_bitcoin_address;
    let height = share
        .height
        .map_or_else(|| UNKNOWN.to_string(), |height| height.to_string());
    let uncles = if share.uncles.is_empty() {
        view! { <p class="empty-state">"This share includes no uncles."</p> }.into_any()
    } else {
        let rows = share
            .uncles
            .iter()
            .map(|uncle| view! { <tr><td>{share_link(uncle)}</td></tr> })
            .collect_view();
        view! {
            <div class="table-x">
                <table class="table">
                    <thead>
                        <tr>
                            <th>"Uncle"</th>
                        </tr>
                    </thead>
                    <tbody>{rows}</tbody>
                </table>
            </div>
        }
        .into_any()
    };
    view! {
        <a class="back-link" href=miner_href(&owner)>
            {format!("\u{2190} Miner {}", short(&owner))}
        </a>
        <div class="panel">
            <h2>
                "Share"
                {is_block.then(|| view! { " " <span class="badge-block">"found a block"</span> })}
                <span class="panel-note">
                    {format!("{} \u{b7} {}", share.validation_status, share.chain)}
                </span>
            </h2>
            <div class="stat-list">
                {stat("Hash", view! { <span class="mono">{share.blockhash}</span> }.into_any())}
                {stat("Height", view! { {height} }.into_any())}
                {stat("Parent", share_link(&share.parent).into_any())}
                {stat("Payout address", miner_link(&owner).into_any())}
                {stat("Share chain address", miner_link(&share.miner_address).into_any())}
                {stat("Share bits", view! { <span class="mono">{share.bits}</span> }.into_any())}
                {stat("Time", view! { {share.time} }.into_any())}
            </div>
        </div>
        <div class="panel">
            <h2>
                "Uncles"
                <span class="panel-note">{format!("{} included", share.uncles.len())}</span>
            </h2>
            {uncles}
        </div>
        <div class="panel">
            <h2>"Bitcoin Cash header"</h2>
            <div class="stat-list">
                {stat(
                    "Block hash",
                    view! { <span class="mono">{header.block_hash}</span> }.into_any(),
                )}
                {stat(
                    "Previous block",
                    view! { <span class="mono">{header.prev_blockhash}</span> }.into_any(),
                )}
                {stat(
                    "Merkle root",
                    view! { <span class="mono">{header.merkle_root}</span> }.into_any(),
                )}
                {stat("Time", view! { {header.time} }.into_any())}
                {stat("Bits", view! { <span class="mono">{header.bits}</span> }.into_any())}
                {stat("Nonce", view! { <span class="mono">{header.nonce}</span> }.into_any())}
                {stat(
                    "Version",
                    view! { <span class="mono">{format!("{:#x}", header.version)}</span> }
                        .into_any(),
                )}
            </div>
        </div>
    }
}

#[component]
pub fn SharePage(hash: String) -> impl IntoView {
    if !looks_like_hash(&hash) {
        return view! { <ErrorPage message=format!("Not a share hash: {hash}") /> }.into_any();
    }
    let frame = ctx().frame;
    let title = format!("Share {}", short(&hash));
    frame.set(title.clone(), Tab::Miners, share_trail(&hash, None));
    let path = format!("/share?hash={}", url_component(&hash));
    let detail = fetched::<Vec<ShareDetail>>(move || path.clone());
    // A share that found a block is one the pool's found blocks name.
    let blocks = fetched::<Vec<FoundBlock>>(|| "/found_blocks".into());
    (move || match detail.get() {
        None => view! { <div class="panel">{loading()}</div> }.into_any(),
        Some(Err(message)) => view! { <ErrorPage message=message /> }.into_any(),
        Some(Ok(details)) => match details.into_iter().next() {
            None => {
                let message = format!("No share has hash {hash}.");
                view! { <ErrorPage message=message /> }.into_any()
            }
            Some(share) => {
                frame.set(
                    title.clone(),
                    Tab::Miners,
                    share_trail(&hash, Some(&share.miner_bitcoin_address)),
                );
                let is_block = blocks.get().and_then(Result::ok).is_some_and(|blocks| {
                    blocks
                        .iter()
                        .any(|block| block.share_blockhash == share.blockhash)
                });
                share_view(share, is_block).into_any()
            }
        },
    })
    .into_any()
}
