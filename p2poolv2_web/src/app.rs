// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The app: its state, the page the URL fragment names, and the live share
//! feed. All numbers come from the node's API; the browser only formats and
//! draws them.

use crate::api::{self, Connection, Share, Shares, Site};
use crate::format::url_decode;
use crate::layout::{Footer, Header, PageFrame, Trail};
use crate::route::{PAGE_SIZE, Route};
use crate::views::{
    BlockPage, BlocksPage, HelpPage, MinerPage, MinersPage, OverviewPage, SearchPage, SettingsPage,
    SharePage,
};
use futures::{SinkExt, StreamExt};
use gloo_net::websocket::{Message, futures::WebSocket};
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::cmp::Reverse;

/// Milliseconds between polls of the REST endpoints, as often as the node's
/// overview refreshes itself.
const REFRESH_MS: u32 = 30_000;

/// Milliseconds before the live feed reconnects.
const RECONNECT_MS: u32 = 5_000;

/// The state of the `/ws` share feed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feed {
    Connecting,
    Live,
    Offline,
}

/// What every view reads.
#[derive(Clone, Copy)]
pub struct Ctx {
    pub connection: RwSignal<Connection>,
    pub route: RwSignal<Route>,
    /// Bumped on a timer; resources that refresh read it.
    pub tick: RwSignal<u64>,
    /// Unix seconds, updated every second, for ages.
    pub clock: RwSignal<u64>,
    /// The newest page of the confirmed share chain, newest first, at most
    /// [`PAGE_SIZE`] shares: seeded from `/shares` and kept current by `/ws`.
    pub recent: RwSignal<Vec<Share>>,
    pub live: RwSignal<Feed>,
    /// The chain the node mines, once `/network` answered.
    pub site: RwSignal<Option<Site>>,
    pub frame: PageFrame,
}

pub fn ctx() -> Ctx {
    expect_context::<Ctx>()
}

/// Unix seconds now, from the browser's clock.
fn unix_now() -> u64 {
    let milliseconds = window()
        .performance()
        .map(|performance| performance.time_origin() + performance.now())
        .unwrap_or_default();
    (milliseconds / 1000.0) as u64
}

fn local_storage() -> Option<web_sys::Storage> {
    window().local_storage().ok()?
}

fn session_storage() -> Option<web_sys::Storage> {
    window().session_storage().ok()?
}

fn stored(storage: Option<web_sys::Storage>, key: &str) -> Option<String> {
    storage?.get_item(key).ok()?
}

fn store(storage: Option<web_sys::Storage>, key: &str, value: &str) {
    if let Some(storage) = storage
        && storage.set_item(key, value).is_err()
    {
        leptos::logging::warn!("Could not save {key} in browser storage");
    }
}

/// Initial connection: `?api=` in the page URL, then the saved base, then the
/// page's own origin (the node serving the app from `web_root`). The
/// password lives only in session storage, so it is gone when the tab closes.
fn initial_connection() -> Connection {
    let location = window().location();
    let origin = location.origin().unwrap_or_default();
    let from_query = location.search().ok().and_then(|search| {
        search
            .trim_start_matches('?')
            .split('&')
            .find_map(|pair| pair.strip_prefix("api=").map(url_decode))
    });
    let base = from_query
        .or_else(|| stored(local_storage(), "p2pool.api"))
        .unwrap_or(origin);
    Connection {
        base: base.trim_end_matches('/').to_string(),
        user: stored(local_storage(), "p2pool.user").unwrap_or_default(),
        password: stored(session_storage(), "p2pool.password").unwrap_or_default(),
    }
}

/// Save `connection` (the password for this tab only) and use it.
pub fn save_connection(connection: Connection) {
    store(local_storage(), "p2pool.api", &connection.base);
    store(local_storage(), "p2pool.user", &connection.user);
    store(session_storage(), "p2pool.password", &connection.password);
    ctx().connection.set(connection);
}

/// The page the location's fragment names.
fn current_route() -> Route {
    Route::parse(&window().location().hash().unwrap_or_default())
}

/// Open `route` in place of the current page, so the back button skips the
/// page that redirected.
pub fn replace_route(route: &Route) {
    if window().location().replace(&route.href()).is_err() {
        leptos::logging::warn!("Could not open {}", route.href());
    }
}

/// Open `route` as a new history entry.
pub fn open_route(route: &Route) {
    if window().location().set_hash(&route.href()).is_err() {
        leptos::logging::warn!("Could not open {}", route.href());
    }
}

/// A resource that fetches `path` again when the connection changes or the
/// timer ticks.
pub fn polled<T>(
    path: impl Fn() -> String + Send + Sync + 'static,
) -> LocalResource<Result<T, String>>
where
    T: serde::de::DeserializeOwned + 'static,
{
    let Ctx {
        connection, tick, ..
    } = ctx();
    LocalResource::new(move || {
        tick.track();
        let connection = connection.get();
        let path = path();
        async move { connection.get::<T>(&path).await }
    })
}

/// A resource that fetches `path` once, and again when the connection
/// changes: for pages the node renders once too.
pub fn fetched<T>(
    path: impl Fn() -> String + Send + Sync + 'static,
) -> LocalResource<Result<T, String>>
where
    T: serde::de::DeserializeOwned + 'static,
{
    let connection = ctx().connection;
    LocalResource::new(move || {
        let connection = connection.get();
        let path = path();
        async move { connection.get::<T>(&path).await }
    })
}

#[component]
pub fn App() -> impl IntoView {
    let ctx = Ctx {
        connection: RwSignal::new(initial_connection()),
        route: RwSignal::new(current_route()),
        tick: RwSignal::new(0),
        clock: RwSignal::new(unix_now()),
        recent: RwSignal::new(Vec::with_capacity(PAGE_SIZE)),
        live: RwSignal::new(Feed::Connecting),
        site: RwSignal::new(None),
        frame: PageFrame::default(),
    };
    provide_context(ctx);
    gloo_timers::callback::Interval::new(REFRESH_MS, move || ctx.tick.update(|tick| *tick += 1))
        .forget();
    gloo_timers::callback::Interval::new(1_000, move || ctx.clock.set(unix_now())).forget();
    // Links change the fragment; a new page starts at the top, as a new
    // document from the node does.
    let _listener = window_event_listener(leptos::ev::hashchange, move |_| {
        ctx.route.set(current_route());
        window().scroll_to_with_x_and_y(0.0, 0.0);
    });
    start_live_feed(ctx);
    load_site(ctx);
    keep_title(ctx);

    view! {
        <Header />
        <main class="container">
            <Trail />
            {move || match ctx.route.get() {
                Route::Dashboard { page } => view! { <OverviewPage page=page /> }.into_any(),
                Route::Blocks { page } => view! { <BlocksPage page=page /> }.into_any(),
                Route::Block(hash) => view! { <BlockPage hash=hash /> }.into_any(),
                Route::Miners { page } => view! { <MinersPage page=page /> }.into_any(),
                Route::Miner { address, page } => {
                    view! { <MinerPage address=address page=page /> }.into_any()
                }
                Route::Share(hash) => view! { <SharePage hash=hash /> }.into_any(),
                Route::Help => view! { <HelpPage /> }.into_any(),
                Route::Settings => view! { <SettingsPage /> }.into_any(),
                Route::Search(text) => view! { <SearchPage text=text /> }.into_any(),
            }}
        </main>
        <Footer />
    }
}

/// Ask the node which chain it mines, again when the connection changes.
fn load_site(ctx: Ctx) {
    Effect::new(move |_| {
        let connection = ctx.connection.get();
        spawn_local(async move {
            ctx.site.set(connection.get::<Site>("/network").await.ok());
        });
    });
}

/// Name the browser tab as the node names its pages.
fn keep_title(ctx: Ctx) {
    Effect::new(move |_| {
        let badge = ctx
            .site
            .with(|site| site.as_ref().map(|site| site.badge.clone()))
            .unwrap_or_default();
        let title = format!("{} \u{b7} P2Pool v2 \u{b7} {badge}", ctx.frame.title.get());
        document().set_title(title.trim_end_matches([' ', '\u{b7}']));
    });
}

/// Put a live share at the top of `shares`. A share replaces any at its
/// height or above, as a reorg would, and the list keeps [`PAGE_SIZE`]
/// shares at most.
pub fn push_live_share(shares: &mut Vec<Share>, share: Share) {
    shares.retain(|kept| kept.height < share.height);
    shares.insert(0, share);
    shares.truncate(PAGE_SIZE);
}

/// Keep `recent` filled: seed it from `/shares`, then add `/ws` share
/// events. Reconnects after a delay, and starts over when the connection
/// changes.
fn start_live_feed(ctx: Ctx) {
    let Ctx {
        connection,
        recent,
        live,
        ..
    } = ctx;
    Effect::new(move |generation: Option<u64>| {
        let generation = generation.map_or(0, |generation| generation + 1);
        let current = connection.get();
        let watched = current.clone();
        let still_current = move || connection.with_untracked(|now| *now == watched);
        spawn_local(async move {
            live.set(Feed::Connecting);
            if let Ok(seed) = current
                .get::<Shares>(&format!("/shares?num={PAGE_SIZE}"))
                .await
            {
                let mut shares = seed.shares;
                shares.sort_by_key(|share| Reverse(share.height));
                shares.truncate(PAGE_SIZE);
                recent.set(shares);
            }
            while still_current() {
                if let Ok(socket) = WebSocket::open(&current.websocket_url()) {
                    let (mut sink, mut stream) = socket.split();
                    if sink
                        .send(Message::Text(api::SUBSCRIBE_SHARES.into()))
                        .await
                        .is_ok()
                    {
                        live.set(Feed::Live);
                        let mut open = true;
                        while open && still_current() {
                            match stream.next().await {
                                Some(Ok(Message::Text(text))) => {
                                    if let Some(share) = api::parse_share_event(&text) {
                                        recent.update(|shares| push_live_share(shares, share));
                                    }
                                }
                                Some(Ok(Message::Bytes(_))) => {}
                                Some(Err(_)) | None => open = false,
                            }
                        }
                    }
                }
                live.set(Feed::Offline);
                gloo_timers::future::TimeoutFuture::new(RECONNECT_MS).await;
            }
        });
        generation
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn share(height: u32, blockhash: &str) -> Share {
        Share {
            blockhash: blockhash.to_string(),
            height,
            miner_bitcoin_address: String::new(),
            miner_address: String::new(),
            timestamp: 0,
            bits: 0x1d00_ffff,
            uncles: Vec::new(),
        }
    }

    #[test]
    fn test_a_live_share_goes_on_top() {
        let mut shares = vec![share(2, "b"), share(1, "a")];
        push_live_share(&mut shares, share(3, "c"));
        assert_eq!(shares[0].blockhash, "c");
        assert_eq!(shares.len(), 3);
    }

    #[test]
    fn test_a_reorged_share_replaces_its_height_and_above() {
        let mut shares = vec![share(3, "c"), share(2, "b"), share(1, "a")];
        push_live_share(&mut shares, share(2, "x"));
        assert_eq!(
            shares
                .iter()
                .map(|kept| kept.blockhash.as_str())
                .collect::<Vec<_>>(),
            vec!["x", "a"]
        );
    }

    #[test]
    fn test_the_live_list_keeps_one_page() {
        let mut shares: Vec<Share> = (0..PAGE_SIZE as u32)
            .rev()
            .map(|height| share(height, "s"))
            .collect();
        push_live_share(&mut shares, share(PAGE_SIZE as u32, "new"));
        assert_eq!(shares.len(), PAGE_SIZE);
        assert_eq!(shares[0].blockhash, "new");
    }
}
