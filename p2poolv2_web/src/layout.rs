// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The page frame and the pieces every page is built from.
//!
//! The markup and class names are those of the node's pages
//! (`p2poolv2_api/src/api/dashboard/pages.rs`): the same header, menu bar,
//! search box, trail, footer, KPI cards, details lists, pager, links and
//! error page. The stylesheets are the node's too, so both dashboards look
//! the same; the API crate's style guard checks every class used here.

use crate::app::{Feed, ctx, open_route};
use crate::format::{fmt_ago, fmt_utc, short};
use crate::route::{Route, TABS, Tab, miner_href, share_href};
use leptos::prelude::*;

/// One step of the trail at the top of a page. The last step is the page
/// itself and has no link.
#[derive(Clone, Debug, PartialEq)]
pub struct Crumb {
    pub label: String,
    pub href: Option<String>,
}

impl Crumb {
    pub fn link(label: impl Into<String>, href: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            href: Some(href.into()),
        }
    }

    pub fn here(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            href: None,
        }
    }
}

/// The first step of every trail.
pub fn dashboard_crumb() -> Crumb {
    Crumb::link("Dashboard", "#/")
}

/// What the frame shows about the current page: its title, the tab marked
/// in the menu bar and its trail. Each page sets it when it opens.
#[derive(Clone, Copy)]
pub struct PageFrame {
    pub title: RwSignal<String>,
    pub tab: RwSignal<Tab>,
    pub trail: RwSignal<Vec<Crumb>>,
}

impl Default for PageFrame {
    fn default() -> Self {
        Self {
            title: RwSignal::new(String::new()),
            tab: RwSignal::new(Tab::None),
            trail: RwSignal::new(Vec::new()),
        }
    }
}

impl PageFrame {
    pub fn set(&self, title: impl Into<String>, tab: Tab, trail: Vec<Crumb>) {
        self.title.set(title.into());
        self.tab.set(tab);
        self.trail.set(trail);
    }
}

/// The menu bar: logo, chain badge, tabs, search, live status and settings.
#[component]
pub fn Header() -> impl IntoView {
    let context = ctx();
    let tab = context.frame.tab;
    let site = context.site;
    let live = context.live;
    let on_settings = move || context.route.with(|route| *route == Route::Settings);
    let query = RwSignal::new(String::new());
    let search = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let text = query.get_untracked().trim().to_string();
        if !text.is_empty() {
            query.set(String::new());
            open_route(&Route::Search(text));
        }
    };
    let tabs = TABS
        .iter()
        .map(|&(this, label, href)| {
            let current = move || tab.get() == this;
            view! {
                <li>
                    <a
                        class="outline header-btn"
                        class:active-nav=current
                        role="button"
                        href=href
                        aria-current=move || current().then_some("page")
                    >
                        {label}
                    </a>
                </li>
            }
        })
        .collect_view();
    let live_label = move || match live.get() {
        Feed::Connecting => "connecting",
        Feed::Live => "live",
        Feed::Offline => "offline",
    };
    let live_title = move || match live.get() {
        Feed::Connecting => "Connecting to the node's share feed",
        Feed::Live => "New shares arrive as the node confirms them",
        Feed::Offline => "The share feed is down; retrying",
    };
    view! {
        <header class="site-header">
            <nav>
                <ul class="brand">
                    <li>
                        <a href="#/">
                            <img src="logo-horizontal-wt.svg" alt="Bitcoin Cash" />
                        </a>
                        <span class="brand-meta">
                            <a class="wordmark-link" href="#/">
                                <span class="wordmark">"P2Pool" <span class="accent">"v2"</span></span>
                            </a>
                            {move || {
                                site.get()
                                    .map(|site| {
                                        view! {
                                            <span class="chain-badge" title=site.network>
                                                {site.badge}
                                            </span>
                                        }
                                    })
                            }}
                        </span>
                    </li>
                </ul>
                <ul class="tabs">{tabs}</ul>
                <ul class="header-right">
                    <li>
                        <form class="header-search" role="search" on:submit=search>
                            <input
                                type="search"
                                name="q"
                                placeholder="Miner address or share hash"
                                aria-label="Miner address or share hash"
                                required
                                bind:value=query
                            />
                            <button class="outline header-btn" type="submit">
                                "Search"
                            </button>
                        </form>
                    </li>
                    <li title=live_title>
                        <span
                            class="status-dot"
                            class:ok=move || live.get() == Feed::Live
                            class:err=move || live.get() == Feed::Offline
                        ></span>
                        " "
                        {live_label}
                    </li>
                    <li>
                        <a
                            class="outline header-btn"
                            class:active-nav=on_settings
                            role="button"
                            href="#/settings"
                            aria-current=move || on_settings().then_some("page")
                        >
                            "Settings"
                        </a>
                    </li>
                </ul>
            </nav>
        </header>
    }
}

/// The trail back to the overview, on every page but the overview.
#[component]
pub fn Trail() -> impl IntoView {
    let trail = ctx().frame.trail;
    move || {
        let crumbs = trail.get();
        (!crumbs.is_empty()).then(|| {
            let steps = crumbs
                .into_iter()
                .map(|crumb| match crumb.href {
                    Some(href) => view! { <li><a href=href>{crumb.label}</a></li> }.into_any(),
                    None => {
                        view! { <li><span aria-current="page">{crumb.label}</span></li> }.into_any()
                    }
                })
                .collect_view();
            view! {
                <nav aria-label="breadcrumb">
                    <ul>{steps}</ul>
                </nav>
            }
        })
    }
}

#[component]
pub fn Footer() -> impl IntoView {
    let site = ctx().site;
    view! {
        <footer class="site-footer">
            <div class="inner">
                <span>
                    "P2Pool v2"
                    {move || site.get().map(|site| format!(" \u{b7} {}", site.network))}
                </span>
                <span>"Rust compiled to WebAssembly, reading this node's API. No tracking."</span>
            </div>
        </footer>
    }
}

/// A link to a miner's page, cut to fit.
pub fn miner_link(address: &str) -> impl IntoView + use<> {
    view! {
        <a class="addr" href=miner_href(address) title=address.to_string()>
            {short(address)}
        </a>
    }
}

/// A link to a share's page, cut to fit.
pub fn share_link(blockhash: &str) -> impl IntoView + use<> {
    view! {
        <a class="addr" href=share_href(blockhash) title=blockhash.to_string()>
            {short(blockhash)}
        </a>
    }
}

/// How long ago `then` was, kept current, with the time on hover.
pub fn ago_cell(then: u64) -> impl IntoView {
    let clock = ctx().clock;
    view! { <td title=fmt_utc(then)>{move || fmt_ago(clock.get(), then)}</td> }
}

/// When a miner or worker last sent a share; one that has not sent any yet
/// was never seen.
pub fn last_seen(then: u64) -> AnyView {
    if then == 0 {
        return view! { <span>"never"</span> }.into_any();
    }
    let clock = ctx().clock;
    view! { <span title=fmt_utc(then)>{move || fmt_ago(clock.get(), then)}</span> }.into_any()
}

/// The colour of a KPI card's value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tone {
    #[default]
    Plain,
    Green,
    Local,
    Net,
}

/// A KPI card: a label, a value and notes under it.
#[component]
pub fn KpiCard(
    label: &'static str,
    #[prop(into)] value: Signal<String>,
    #[prop(optional)] tone: Tone,
    #[prop(optional)] href: Option<&'static str>,
    #[prop(into)] notes: Signal<Vec<String>>,
) -> impl IntoView {
    let green = tone == Tone::Green;
    let local = tone == Tone::Local;
    let net = tone == Tone::Net;
    let value_view = move || {
        view! {
            <p
                class="kpi-value"
                class:kpi-value-green=green
                class:kpi-value-local=local
                class:kpi-value-net=net
            >
                {value}
            </p>
        }
    };
    view! {
        <div class="kpi-card">
            <p class="kpi-label">{label}</p>
            {match href {
                Some(href) => view! { <a href=href>{value_view()}</a> }.into_any(),
                None => value_view().into_any(),
            }}
            {move || {
                notes
                    .get()
                    .into_iter()
                    .map(|note| view! { <p class="kpi-sub">{note}</p> })
                    .collect_view()
            }}
        </div>
    }
}

/// One item of a details list.
pub fn stat(key: &'static str, value: AnyView) -> impl IntoView {
    view! {
        <div class="stat-item">
            <div class="k">{key}</div>
            <div class="v">{value}</div>
        </div>
    }
}

/// Previous and next links for a long table; `page_route` makes the link to
/// a page. Nothing when everything fits on one page.
pub fn pager(page: usize, pages: usize, page_route: impl Fn(usize) -> Route) -> Option<AnyView> {
    if pages <= 1 {
        return None;
    }
    let has_previous = page > 1;
    let has_next = page < pages;
    let previous = page_route(page.saturating_sub(1).max(1)).href();
    let next = page_route((page + 1).min(pages)).href();
    Some(
        view! {
            <nav class="more-row" aria-label="Pages">
                <a
                    class="outline more-btn"
                    role="button"
                    href=previous
                    aria-disabled=(!has_previous).then_some("true")
                    rel="prev"
                >
                    "Previous"
                </a>
                <span class="page-of">{format!("Page {page} of {pages}")}</span>
                <a
                    class="outline more-btn"
                    role="button"
                    href=next
                    aria-disabled=(!has_next).then_some("true")
                    rel="next"
                >
                    "Next"
                </a>
            </nav>
        }
        .into_any(),
    )
}

/// The form that opens a miner's page, for the miner page and errors.
pub fn lookup_form(value: &str) -> impl IntoView + use<> {
    let address = RwSignal::new(value.to_string());
    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let typed = address.get_untracked().trim().to_string();
        if !typed.is_empty() {
            open_route(&Route::Miner {
                address: typed,
                page: 1,
            });
        }
    };
    view! {
        <form class="search-row" on:submit=submit>
            <div class="grow">
                <label for="miner-address">"Payout or share chain address"</label>
                <input
                    id="miner-address"
                    type="text"
                    name="address"
                    placeholder="bchtest:\u{2026} or tp2pool:\u{2026}"
                    required
                    bind:value=address
                />
            </div>
            <button class="btn-primary" type="submit">
                "Look up"
            </button>
        </form>
    }
}

/// A request the page could not answer, as the node's error page shows it.
#[component]
pub fn ErrorPage(message: String) -> impl IntoView {
    ctx().frame.set(
        "Not available",
        Tab::None,
        vec![dashboard_crumb(), Crumb::here("Not available")],
    );
    view! {
        <div class="toast" role="alert">{message}</div>
        <div class="panel">
            <h2>"Find a miner"</h2>
            {lookup_form("")}
        </div>
        <p>
            <a href="#/">"\u{2190} Back to the dashboard"</a>
        </p>
    }
}

/// Shown in a panel until its data arrives.
pub fn loading() -> AnyView {
    view! { <p class="empty-state">"Loading\u{2026}"</p> }.into_any()
}
