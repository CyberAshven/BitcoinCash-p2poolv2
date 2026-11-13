// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The dashboard as a visitor's browser shows it.
//!
//! The other dashboard tests read the HTML a handler returns. These start
//! the dashboard on a local port and have a real browser, headless Chrome,
//! load each page: it parses the page, fetches its stylesheets, and prints
//! the document it ended up with. The test reads that document, follows the
//! links a visitor could follow from the overview, and fails on a page the
//! browser could not load a resource for.
//!
//! The browser is the one named by `CHROME`, or else Chrome, Chromium or
//! Edge in its usual place. Without one the test is skipped on a
//! developer's machine and fails in CI.

use super::*;
use crate::api::endpoints::miner::MinerWindowSource;
use p2poolv2_lib::accounting::payout::sharechain_pplns::pplns_window::PplnsWindow;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::RwLock;

/// Where a Chromium browser is usually installed.
const BROWSER_PATHS: [&str; 8] = [
    "/usr/bin/google-chrome",
    "/usr/bin/google-chrome-stable",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "C:/Program Files/Google/Chrome/Application/chrome.exe",
    "C:/Program Files (x86)/Google/Chrome/Application/chrome.exe",
    "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
];

/// The browser to drive: `CHROME` when it is set, else the first usual
/// place that holds one.
fn browser() -> Option<PathBuf> {
    std::env::var_os("CHROME")
        .map(PathBuf::from)
        .or_else(|| {
            BROWSER_PATHS
                .iter()
                .map(PathBuf::from)
                .find(|path| path.exists())
        })
        .filter(|path| path.exists())
}

/// What the browser made of one page.
struct Rendered {
    /// The document after the browser parsed and laid it out.
    document: String,
    /// What the browser logged while it loaded the page.
    log: String,
}

/// Load `url` in the headless browser and return the document it rendered.
/// Each load has its own profile directory, so two loads never share one.
fn render(browser: &Path, url: &str) -> Rendered {
    let profile = tempfile::tempdir().unwrap();
    let output = Command::new(browser)
        .arg("--headless=new")
        .arg("--disable-gpu")
        .arg("--no-sandbox")
        .arg("--no-first-run")
        .arg("--enable-logging=stderr")
        .arg("--log-level=0")
        .arg("--virtual-time-budget=5000")
        .arg(format!("--user-data-dir={}", profile.path().display()))
        .arg("--dump-dom")
        .arg(url)
        .output()
        .unwrap_or_else(|error| panic!("{} did not start: {error}", browser.display()));
    assert!(
        output.status.success(),
        "the browser failed on {url}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Rendered {
        document: String::from_utf8_lossy(&output.stdout).into_owned(),
        log: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

/// Save a picture of `url` as the browser draws it, for a person to look at.
fn screenshot(browser: &Path, url: &str, picture: &Path) {
    let profile = tempfile::tempdir().unwrap();
    let status = Command::new(browser)
        .arg("--headless=new")
        .arg("--disable-gpu")
        .arg("--no-sandbox")
        .arg("--no-first-run")
        .arg("--window-size=1280,1600")
        .arg(format!("--user-data-dir={}", profile.path().display()))
        .arg(format!("--screenshot={}", picture.display()))
        .arg(url)
        .status()
        .unwrap_or_else(|error| panic!("{} did not start: {error}", browser.display()));
    assert!(status.success(), "the browser could not draw {url}");
}

/// The values of every `attribute="..."` in `document` that start with
/// `prefix`, in the order they appear.
fn attribute_values(document: &str, attribute: &str, prefix: &str) -> Vec<String> {
    let opening = format!("{attribute}=\"");
    document
        .split(opening.as_str())
        .skip(1)
        .filter_map(|rest| rest.split_once('"').map(|(value, _)| value))
        .filter(|value| value.starts_with(prefix))
        .map(|value| value.replace("&amp;", "&"))
        .collect()
}

/// The dashboard pages a visitor can reach from `document` by a link.
fn page_links(document: &str) -> BTreeSet<String> {
    attribute_values(document, "href", "/dashboard")
        .into_iter()
        .filter(|link| !link.starts_with("/dashboard/assets/"))
        .filter(|link| !link.starts_with("/dashboard/signin"))
        .collect()
}

/// The kind of page `link` opens: its path up to the part that names one
/// share, block or miner, so `/dashboard/share/<hash>` is `/dashboard/share`.
fn page_kind(link: &str) -> String {
    let path = link.split('?').next().unwrap_or(link);
    path.split('/').take(3).collect::<Vec<_>>().join("/")
}

/// The name a picture of `link` is saved under.
fn picture_name(link: &str) -> String {
    let name: String = link
        .trim_start_matches('/')
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .take(80)
        .collect();
    format!("{name}.png")
}

/// A dashboard over a chain of a genesis share and three shares on it,
/// listening on a local port the system chose. The directories keep the
/// stores alive for the test.
async fn serve_dashboard() -> (String, tempfile::TempDir, tempfile::TempDir) {
    let (chain_store_handle, store) = setup_test_chain_store_handle(true).await;
    let metrics_directory = tempfile::tempdir().unwrap();
    let metrics_handle =
        metrics::start_metrics(metrics_directory.path().to_str().unwrap().to_string())
            .await
            .unwrap();
    //* A node that pays from the share chain has the window the miner page
    //* reads; without one that page answers "not found".
    let window = Arc::new(RwLock::new(PplnsWindow::new(bitcoin::Network::Signet)));
    let state = Arc::new(AppState {
        app_config: AppConfig {
            pool_signature_length: 0,
            network: bitcoin::Network::Signet,
            cash_network: None,
            cors_allowed: false,
        },
        chain_store_handle,
        metrics_handle,
        tracker_handle: start_tracker_actor(),
        node_handle: NodeHandle::new_for_test(),
        monitoring_event_sender: create_monitoring_event_channel(),
        auth_user: None,
        auth_token: None,
        pplns_window: Some(Arc::new(MinerWindowSource::new(Arc::clone(&window)))),
    });
    let genesis_block = genesis_for_tests();
    state
        .chain_store_handle
        .init_or_setup_genesis(genesis_block.clone())
        .await
        .unwrap();
    let mut parent = genesis_block.block_hash();
    for _ in 0..3 {
        let share = TestShareBlockBuilder::new()
            .prev_share_blockhash(parent.to_string())
            .miner_pubkey(PUBKEY_G)
            .build();
        confirm_share(&state, &share).await;
        parent = share.block_hash();
    }
    window
        .write()
        .unwrap()
        .update(&state.chain_store_handle)
        .unwrap();
    let app = build_router_with_history(
        Arc::clone(&state),
        state.app_config.clone(),
        Arc::new(HashrateHistory::default()),
        WebOptions::default(),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{address}"), store, metrics_directory)
}

/// A visitor opens the overview, sees the recent shares, and follows every
/// link on it. Each page they reach is a full page with its stylesheets
/// loaded, and each link on those pages leads somewhere too.
#[tokio::test(flavor = "multi_thread")]
async fn a_visitor_can_open_every_page_the_overview_links_to() {
    let Some(browser) = browser() else {
        assert!(
            std::env::var_os("CI").is_none(),
            "CI has no Chromium browser: set CHROME or install one"
        );
        eprintln!("no Chromium browser found, set CHROME to run the dashboard browser test");
        return;
    };
    let (origin, _store, _metrics) = serve_dashboard().await;
    let pictures = std::env::var_os("DASHBOARD_SCREENSHOTS").map(PathBuf::from);
    if let Some(directory) = &pictures {
        std::fs::create_dir_all(directory).unwrap();
    }

    let overview = {
        let browser = browser.clone();
        let url = format!("{origin}/dashboard");
        tokio::task::spawn_blocking(move || render(&browser, &url))
            .await
            .unwrap()
    };
    //* The page itself, whatever its panels are called; that it lists the
    //* shares is the link to a share asserted below.
    assert!(
        overview.document.contains("site-header"),
        "the overview renders as a page: {}",
        overview.document
    );
    let stylesheets = attribute_values(&overview.document, "href", "/dashboard/assets/");
    assert!(
        !stylesheets.is_empty(),
        "the overview links its stylesheets"
    );
    for stylesheet in &stylesheets {
        let response = reqwest::get(format!("{origin}{stylesheet}")).await.unwrap();
        assert!(
            response.status().is_success(),
            "stylesheet {stylesheet} answers {}",
            response.status()
        );
    }

    let links = page_links(&overview.document);
    for expected in [
        "/dashboard/shares",
        "/dashboard/blocks",
        "/dashboard/miners",
        "/dashboard/help",
    ] {
        assert!(
            links.contains(expected),
            "the overview links to {expected}: {links:?}"
        );
    }
    assert!(
        links
            .iter()
            .any(|link| link.starts_with("/dashboard/share/")),
        "the overview links to a share: {links:?}"
    );

    for link in &links {
        let response = reqwest::get(format!("{origin}{link}")).await.unwrap();
        assert!(
            response.status().is_success(),
            "{link} answers {}",
            response.status()
        );
    }

    //* The browser loads one page of each kind, all at once: every share
    //* page is the same template, and one browser start a page keeps the
    //* test short.
    let mut one_of_each: BTreeMap<String, String> = BTreeMap::new();
    for link in &links {
        one_of_each
            .entry(page_kind(link))
            .or_insert_with(|| link.clone());
    }
    let mut renders = tokio::task::JoinSet::new();
    for link in one_of_each.values() {
        let browser = browser.clone();
        let link = link.clone();
        let url = format!("{origin}{link}");
        renders.spawn_blocking(move || {
            let page = render(&browser, &url);
            (link, page)
        });
    }
    let mut further = BTreeSet::new();
    while let Some(rendered) = renders.join_next().await {
        let (link, page) = rendered.unwrap();
        assert!(
            page.document.contains("<title>") && page.document.contains("site-header"),
            "{link} renders a page with its title and the site header: {}",
            page.document
        );
        assert!(
            !attribute_values(&page.document, "href", "/dashboard/assets/").is_empty(),
            "{link} links its stylesheets"
        );
        assert!(
            !page.log.contains("Failed to load resource"),
            "{link} loads every resource: {}",
            page.log
        );
        further.extend(page_links(&page.document));
    }
    if let Some(directory) = &pictures {
        for link in one_of_each.values() {
            let browser = browser.clone();
            let url = format!("{origin}{link}");
            let picture = directory.join(picture_name(link));
            tokio::task::spawn_blocking(move || screenshot(&browser, &url, &picture))
                .await
                .unwrap();
        }
    }

    //* A visitor two clicks from the overview: none of those links is dead.
    for link in further.difference(&links) {
        let response = reqwest::get(format!("{origin}{link}")).await.unwrap();
        assert!(
            response.status().is_success(),
            "{link}, linked from a page the overview links to, answers {}",
            response.status()
        );
    }
}

/// The helper reads links out of a document the way the test needs them.
#[test]
fn page_links_keep_dashboard_pages_and_leave_assets_out() {
    let document = r#"<a href="/dashboard/shares">Shares</a>
        <link rel="stylesheet" href="/dashboard/assets/pages.css">
        <a href="/dashboard/miner?address=a&amp;window=1">Miner</a>
        <a href="https://example.org/">Elsewhere</a>
        <a href="/dashboard/signin">Admin</a>"#;

    assert_eq!(
        page_links(document),
        BTreeSet::from([
            "/dashboard/miner?address=a&window=1".to_string(),
            "/dashboard/shares".to_string(),
        ]),
        "dashboard pages only, with the query as a browser sends it"
    );
    assert_eq!(
        picture_name("/dashboard/miner?address=a"),
        "dashboard-miner-address-a.png"
    );
    assert_eq!(page_kind("/dashboard/share/00ab"), "/dashboard/share");
    assert_eq!(page_kind("/dashboard/miner?address=a"), "/dashboard/miner");
    assert_eq!(page_kind("/dashboard"), "/dashboard");
}
