// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The shares page, as `/dashboard/shares` shows it: the whole confirmed
//! share chain, newest first, `PAGE_SIZE` heights to a page, uncles under
//! the share that includes them.

use super::overview::{ShareRow, share_rows, shares_table};
use crate::api::{ChainInfo, FoundBlock, Shares};
use crate::app::{ctx, fetched};
use crate::layout::{Crumb, ErrorPage, dashboard_crumb, loading, pager};
use crate::route::{PAGE_SIZE, Paging, Route, Tab};
use leptos::prelude::*;
use std::cmp::Reverse;
use std::collections::HashSet;

/// A resource's value once it loaded, without copying the value.
fn loaded<T: 'static, R>(
    resource: &LocalResource<Result<T, String>>,
    read: impl FnOnce(&T) -> R,
) -> Option<R> {
    resource.with(|value| match value {
        Some(Ok(value)) => Some(read(value)),
        _ => None,
    })
}

/// The empty-state text before any chain exists. A page that missed the
/// chain says so in [`empty_panel`]'s caller instead.
const NO_CHAIN_YET: &str =
    "No confirmed shares yet. Point a miner at this node's stratum port to get started.";

/// One page of the confirmed share chain, newest first, each share followed
/// by the uncles it includes, exactly as the node's `/dashboard/shares`
/// serves them. The window comes from `/shares`, whose `to` height is
/// inclusive: `to` down to `to - PAGE_SIZE + 1`, clamped to the chain the
/// node has.
#[component]
pub fn SharesPage(page: usize) -> impl IntoView {
    let context = ctx();
    context.frame.set(
        "Shares",
        Tab::None,
        vec![dashboard_crumb(), Crumb::here("Shares")],
    );
    let connection = context.connection;
    let chain = fetched::<ChainInfo>(|| "/chain_info".into());
    let blocks = fetched::<Vec<FoundBlock>>(|| "/found_blocks".into());
    // The fetch depends on `chain`, but a fetcher that waits on it forever
    // would hold this resource's own worker, which must stay free to re-run
    // once `chain` resolves. So while the tip is unknown it answers at once
    // with `None`, and the render below still shows the loading panel until
    // the real page arrives. The window is `to` down from the clamped tip.
    let page_shares: LocalResource<Result<Option<Shares>, String>> =
        LocalResource::new(move || {
            let connection = connection.get();
            let tip = loaded(&chain, |info| info.chain_tip_height).flatten();
            async move {
                let Some(tip) = tip else {
                    return Ok(None);
                };
                let skipped = Paging::of(page, tip as usize + 1).skipped();
                let to = tip.saturating_sub(u32::try_from(skipped).unwrap_or(u32::MAX));
                let shares = connection
                    .get::<Shares>(&format!("/shares?to={to}&num={PAGE_SIZE}"))
                    .await?;
                Ok(Some(shares))
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
    move || match chain.get() {
        None => loading_panel(),
        Some(Err(message)) => view! { <ErrorPage message=message /> }.into_any(),
        Some(Ok(_)) => {
            let tip = loaded(&chain, |info| info.chain_tip_height).flatten();
            match tip {
                None => empty_panel(NO_CHAIN_YET),
                Some(tip) => match page_shares.get() {
                    None | Some(Ok(None)) => loading_panel(),
                    Some(Err(message)) => view! { <ErrorPage message=message /> }.into_any(),
                    Some(Ok(Some(page_of_shares))) => {
                        let mut newest_first = page_of_shares.shares;
                        newest_first.sort_by_key(|share| Reverse(share.height));
                        let rows: Vec<ShareRow> =
                            found.with(|found| share_rows(&newest_first, found));
                        let paging = Paging::of(page, tip as usize + 1);
                        let body = if rows.is_empty() {
                            view! { <p class="empty-state">{"No shares on this page."}</p> }
                                .into_any()
                        } else {
                            view! {
                                {shares_table(rows)}
                                {pager(paging.page, paging.pages, |page| Route::Shares { page })}
                            }
                            .into_any()
                        };
                        shares_panel(tip, body)
                    }
                },
            }
        }
    }
}

/// The panel before the chain is known.
fn loading_panel() -> AnyView {
    view! {
        <div class="panel">
            <h2>"Shares"</h2>
            {loading()}
        </div>
    }
    .into_any()
}

/// The panel that says the page holds nothing, with `text`.
fn empty_panel(text: &str) -> AnyView {
    view! {
        <div class="panel">
            <h2>"Shares"</h2>
            <p class="empty-state">{text}</p>
        </div>
    }
    .into_any()
}

/// The panel around `body`, its heading carrying the tip note.
fn shares_panel(tip: u32, body: AnyView) -> AnyView {
    view! {
        <div class="panel">
            <h2>
                "Shares"
                <span class="panel-note">
                    "newest first, uncles under the share that includes them"
                    {" \u{b7} tip "}
                    {tip}
                </span>
            </h2>
            {body}
        </div>
    }
    .into_any()
}
