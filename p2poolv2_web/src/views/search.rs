// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Where a search goes, as `/dashboard/search` decides it: a share hash to
//! its share, a found block's hash to its block, anything else to the miner
//! page, which says when it is not an address.

use crate::api::{Connection, FoundBlock, ShareDetail};
use crate::app::{ctx, replace_route};
use crate::format::url_component;
use crate::layout::{Crumb, ErrorPage, dashboard_crumb, loading};
use crate::route::{Route, Tab, looks_like_hash};
use leptos::prelude::*;
use leptos::task::spawn_local;

/// The page a hash opens: its share if the node has one, else the found
/// block with that hash.
async fn resolve_hash(connection: &Connection, hash: &str) -> Result<Route, String> {
    let path = format!("/share?hash={}", url_component(hash));
    if connection.get::<Vec<ShareDetail>>(&path).await.is_ok() {
        return Ok(Route::Share(hash.to_string()));
    }
    let blocks = connection.get::<Vec<FoundBlock>>("/found_blocks").await?;
    if blocks
        .iter()
        .any(|block| block.bitcoin_blockhash.eq_ignore_ascii_case(hash))
    {
        return Ok(Route::Block(hash.to_string()));
    }
    Err(format!(
        "No share and no block found by the pool has hash {hash}."
    ))
}

#[component]
pub fn SearchPage(text: String) -> impl IntoView {
    let context = ctx();
    context.frame.set(
        "Search",
        Tab::None,
        vec![dashboard_crumb(), Crumb::here("Search")],
    );
    let text = text.trim().to_string();
    if text.is_empty() {
        let message = "Enter a miner address, a share hash or a block hash.".to_string();
        return view! { <ErrorPage message=message /> }.into_any();
    }
    if !looks_like_hash(&text) {
        replace_route(&Route::Miner {
            address: text,
            page: 1,
        });
        return view! { <div class="panel">{loading()}</div> }.into_any();
    }
    let failed = RwSignal::new(None::<String>);
    let connection = context.connection.get_untracked();
    spawn_local(async move {
        match resolve_hash(&connection, &text).await {
            Ok(route) => replace_route(&route),
            Err(message) => failed.set(Some(message)),
        }
    });
    (move || match failed.get() {
        Some(message) => view! { <ErrorPage message=message /> }.into_any(),
        None => view! { <div class="panel">{loading()}</div> }.into_any(),
    })
    .into_any()
}
