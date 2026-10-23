// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Settings: the API URL and the Basic auth login this app sends. The
//! node's own pages have no such page; the browser's login prompt does it
//! there.

use crate::api::Connection;
use crate::app::{ctx, save_connection};
use crate::layout::{Crumb, dashboard_crumb};
use crate::route::Tab;
use leptos::prelude::*;

#[component]
pub fn SettingsPage() -> impl IntoView {
    let context = ctx();
    context.frame.set(
        "Settings",
        Tab::None,
        vec![dashboard_crumb(), Crumb::here("Settings")],
    );
    let current = context.connection.get_untracked();
    let base = RwSignal::new(current.base);
    let user = RwSignal::new(current.user);
    let password = RwSignal::new(current.password);
    let saved = RwSignal::new(false);
    let save = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        save_connection(Connection {
            base: base
                .get_untracked()
                .trim()
                .trim_end_matches('/')
                .to_string(),
            user: user.get_untracked().trim().to_string(),
            password: password.get_untracked(),
        });
        saved.set(true);
    };
    view! {
        <div class="panel">
            <h2>
                "Settings"
                <span class="panel-note">"the node this app reads, and its API login"</span>
            </h2>
            <form class="search-row" on:submit=save>
                <div class="grow">
                    <label for="api-url">"API URL"</label>
                    <input
                        id="api-url"
                        type="text"
                        placeholder="http://127.0.0.1:46884"
                        bind:value=base
                    />
                </div>
                <div class="grow">
                    <label for="api-user">"User"</label>
                    <input id="api-user" type="text" autocomplete="username" bind:value=user />
                </div>
                <div class="grow">
                    <label for="api-password">"Password"</label>
                    <input
                        id="api-password"
                        type="password"
                        autocomplete="current-password"
                        bind:value=password
                    />
                </div>
                <button class="btn-primary" type="submit">
                    "Save"
                </button>
            </form>
            <p class="kpi-sub">
                "User and password are only needed when the node sets api auth. "
                "The password is kept for this browser tab only."
            </p>
            {move || saved.get().then(|| view! { <p class="kpi-sub">"Saved."</p> })}
        </div>
    }
}
