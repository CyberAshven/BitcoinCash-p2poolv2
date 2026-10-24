// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Guards that keep the dashboard's styling in one place.
//!
//! `assets/` holds the only stylesheets in the repository: Pico,
//! CashDragon0's `dashboard.css` and `pages.css`. The node serves them for
//! the pages it renders here, and the Rust/WASM dashboard (`p2poolv2_web`)
//! has trunk bundle the very same files, so both dashboards look the same.
//! These tests fail as soon as the two could drift apart:
//!
//! - a `.css` file appears anywhere else in the repository;
//! - a class the node's pages render, or the WASM app's views name, has no
//!   selector in the three stylesheets and is not in [`UNSTYLED_CLASSES`];
//! - the WASM views style an element some other way: a computed `class=`,
//!   a `style` attribute or raw HTML;
//! - the WASM `index.html` links anything outside `assets/`, or carries a
//!   script or style of its own, or a stylesheet's `url()` would not
//!   resolve in the bundle;
//! - the WASM app formats a number, time or hash differently from the
//!   pages;
//! - the landing page (`website/index.html`) carries a script or style of
//!   its own, names a class with no selector, or links a local file that is
//!   neither one of the shared assets nor an SVG beside it (the donation QR
//!   codes), which the Pages workflow could not publish with it;
//! - the Pages workflow (`.github/workflows/docs.yml`) stops copying the
//!   assets, the SVGs and the book to where the landing page links them.
//!
//! The WASM views spell every class as a literal, `class="..."` or a
//! `class:name=` toggle, so a scan of their source finds them all; the
//! computed-class check keeps it that way.

use super::assets;
use super::history::Sample;
use super::pages::{
    self, BlockView, BlocksView, FoundBlockRow, MinerRow, MinerView, MinersView, Overview, Paging,
    ShareRow, ShareView, Site, WorkerRow,
};
use crate::api::endpoints::miner::{MinerResponse, MinerShare, WindowSummary};
use crate::api::endpoints::share::{BitcoinHeaderOutput, ShareLookupOutput};
use bitcoin::BlockHash;
use bitcoin::hashes::Hash;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// The WASM app's formatting, compiled here to compare with the pages'.
#[allow(dead_code)]
#[path = "../../../../p2poolv2_web/src/format.rs"]
mod web_format;

/// The stylesheets, in the order both dashboards load them.
const STYLESHEETS: [&str; 3] = ["pico.min.css", "dashboard.css", "pages.css"];

/// Where the WASM app's `index.html` and the landing page find the shared
/// assets. The Pages workflow rewrites this prefix to `assets/` when it
/// copies the assets beside the landing page.
const WEB_ASSETS_HREF: &str = "../p2poolv2_api/src/api/dashboard/assets/";

/// Where the published site serves the mdBook documentation, relative to the
/// landing page.
const LANDING_DOCS_HREF: &str = "docs/";

/// Classes used without a selector of their own, and why that is fine.
const UNSTYLED_CLASSES: [(&str, &str); 1] = [(
    "chart",
    "names the hashrate chart's <figure>; pages.css styles it as `.chart-wrap figure`",
)];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("p2poolv2_api sits in the workspace")
        .to_path_buf()
}

fn assets_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("api")
        .join("dashboard")
        .join("assets")
}

fn web_dir() -> PathBuf {
    workspace_root().join("p2poolv2_web")
}

fn website_dir() -> PathBuf {
    workspace_root().join("website")
}

/// The landing page's HTML.
fn landing_page() -> String {
    read(&website_dir().join("index.html"))
}

/// The `rel="stylesheet"` links of `html`, by file name, in order.
fn stylesheet_links(html: &str) -> Vec<&str> {
    html.split("<link")
        .skip(1)
        .filter(|tag| tag.contains("rel=\"stylesheet\""))
        .filter_map(|tag| attribute_values(tag, "href").into_iter().next())
        .map(|href| href.rsplit('/').next().unwrap_or(href))
        .collect()
}

/// The `on...="` event handler attributes in `html`, each a script.
fn event_handlers(html: &str) -> Vec<&str> {
    html.match_indices(" on")
        .map(|(index, _)| &html[index + 1..])
        .filter_map(|rest| {
            let name = identifier(rest);
            (name.len() > 2 && rest[name.len()..].starts_with("=\"")).then_some(name)
        })
        .collect()
}

/// Where a wallet link on the landing page points: a Bitcoin Cash mainnet
/// payment URI, which a wallet opens, not a file.
const LANDING_WALLET_SCHEME: &str = "bitcoincash:";

/// Whether the landing page may link `link`: an anchor on the page, an
/// `https` URL, a Bitcoin Cash wallet link, the published docs, a shared
/// asset that exists, or an SVG beside the page (the donation QR codes),
/// which the Pages workflow copies with it.
fn is_publishable_landing_link(link: &str, ids: &BTreeSet<&str>) -> bool {
    if let Some(id) = link.strip_prefix('#') {
        return ids.contains(id);
    }
    if link.starts_with("https://")
        || link.starts_with(LANDING_WALLET_SCHEME)
        || link == LANDING_DOCS_HREF
    {
        return true;
    }
    if let Some(name) = link.strip_prefix(WEB_ASSETS_HREF) {
        return assets_dir().join(name).is_file();
    }
    link.ends_with(".svg") && !link.contains(['/', '\\', ':']) && website_dir().join(link).is_file()
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// `css` without its comments.
fn without_comments(css: &str) -> String {
    let mut kept = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        kept.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        rest = after.find("*/").map_or("", |end| &after[end + 2..]);
    }
    kept.push_str(rest);
    kept
}

/// The leading `[A-Za-z0-9_-]` run of `text`.
fn identifier(text: &str) -> &str {
    let end = text
        .find(|character: char| {
            !(character.is_ascii_alphanumeric() || character == '-' || character == '_')
        })
        .unwrap_or(text.len());
    &text[..end]
}

/// The `.name` parts of a selector. A part starting with a digit is a
/// number in a media query, not a class.
fn classes_in_selector(selector: &str) -> impl Iterator<Item = String> + '_ {
    selector
        .split('.')
        .skip(1)
        .map(identifier)
        .filter(|name| {
            name.chars()
                .next()
                .is_some_and(|first| first.is_ascii_alphabetic() || first == '-' || first == '_')
        })
        .map(str::to_string)
}

/// Every class a selector in `css` names. Selectors are the text before a
/// `{`; declarations end at `;` or `}`, so values such as `0.5rem` or
/// `url(ubuntu-400.woff2)` are never read as classes.
fn selector_classes(css: &str) -> BTreeSet<String> {
    let mut classes = BTreeSet::new();
    let mut prelude = String::new();
    for character in without_comments(css).chars() {
        match character {
            '{' => {
                classes.extend(classes_in_selector(&prelude));
                prelude.clear();
            }
            ';' | '}' => prelude.clear(),
            other => prelude.push(other),
        }
    }
    classes
}

/// Every class the three stylesheets have a selector for.
fn styled_classes() -> BTreeSet<String> {
    STYLESHEETS
        .iter()
        .flat_map(|name| selector_classes(&read(&assets_dir().join(name))))
        .collect()
}

/// The values of every `attribute="..."` in `text`.
fn attribute_values<'a>(text: &'a str, attribute: &str) -> Vec<&'a str> {
    let opening = format!("{attribute}=\"");
    text.split(opening.as_str())
        .skip(1)
        .filter_map(|rest| rest.find('"').map(|end| &rest[..end]))
        .collect()
}

/// Every class in the `class` attributes of rendered HTML. Also finds the
/// literal `class="..."` of a Leptos `view!`.
fn class_attribute_names(html: &str) -> BTreeSet<String> {
    attribute_values(html, "class")
        .into_iter()
        .flat_map(str::split_whitespace)
        .map(str::to_string)
        .collect()
}

/// The classes of every Leptos `class:name=` toggle in `source`.
fn class_toggles(source: &str) -> BTreeSet<String> {
    source
        .split("class:")
        .skip(1)
        .filter_map(|rest| {
            let name = identifier(rest);
            (!name.is_empty() && rest[name.len()..].starts_with('=')).then(|| name.to_string())
        })
        .collect()
}

/// Every `.rs` file under `directory`, with its text.
fn rust_sources(directory: &Path) -> Vec<(PathBuf, String)> {
    let mut sources = Vec::with_capacity(16);
    let entries =
        fs::read_dir(directory).unwrap_or_else(|error| panic!("{}: {error}", directory.display()));
    for entry in entries.map(|entry| entry.expect("a readable directory entry")) {
        let path = entry.path();
        if path.is_dir() {
            sources.extend(rust_sources(&path));
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            let text = read(&path);
            sources.push((path, text));
        }
    }
    sources
}

/// The WASM app's sources.
fn web_sources() -> Vec<(PathBuf, String)> {
    rust_sources(&web_dir().join("src"))
}

/// Every class the WASM app's views name.
fn web_classes() -> BTreeSet<String> {
    web_sources()
        .iter()
        .flat_map(|(_, source)| {
            class_attribute_names(source)
                .into_iter()
                .chain(class_toggles(source))
        })
        .collect()
}

/// Whether a directory is outside the repository's own files: version
/// control and tool state, build output (`target`, trunk's `dist`, which
/// holds the bundled copies of `assets/`), vendored crates and the npm
/// packages of `tools/`.
fn is_outside_the_sources(directory: &Path, root: &Path) -> bool {
    let name = directory
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    name.starts_with('.')
        || name == "target"
        || name == "vendor"
        || (name == "node_modules" && directory.starts_with(root.join("tools")))
        || directory == root.join("p2poolv2_web").join("dist")
        || directory == root.join("p2poolv2_web").join("dist-dev")
}

/// Every `.css` file under `directory` that is not one of the shared assets.
fn stray_stylesheets(directory: &Path, root: &Path) -> Vec<PathBuf> {
    let mut stray = Vec::new();
    let entries =
        fs::read_dir(directory).unwrap_or_else(|error| panic!("{}: {error}", directory.display()));
    for entry in entries.map(|entry| entry.expect("a readable directory entry")) {
        let path = entry.path();
        let file_type = entry.file_type().expect("a file type");
        if file_type.is_dir() && !is_outside_the_sources(&path, root) {
            stray.extend(stray_stylesheets(&path, root));
        } else if file_type.is_file()
            && path.extension().is_some_and(|extension| extension == "css")
            && path.parent() != Some(assets_dir().as_path())
        {
            stray.push(path);
        }
    }
    stray
}

/// Snippets of `source` where a `class=` takes something other than a
/// literal, which the class scan could not see.
fn computed_classes(source: &str) -> Vec<String> {
    source
        .match_indices("class=")
        .filter(|(index, _)| !source[index + "class=".len()..].starts_with('"'))
        .map(|(index, _)| source[index..].chars().take(40).collect())
        .collect()
}

/// The files the WASM `index.html` links, as written there.
fn index_links(index: &str) -> Vec<&str> {
    attribute_values(index, "href")
        .into_iter()
        .chain(attribute_values(index, "src"))
        .collect()
}

/// The `data-trunk` links of `index.html` with `rel`, by file name, in order.
fn trunk_links<'a>(index: &'a str, rel: &str) -> Vec<&'a str> {
    let marker = format!("rel=\"{rel}\"");
    index
        .split("<link")
        .skip(1)
        .filter(|tag| tag.contains("data-trunk") && tag.contains(&marker))
        .filter_map(|tag| attribute_values(tag, "href").into_iter().next())
        .map(|href| href.rsplit('/').next().unwrap_or(href))
        .collect()
}

/// The relative files the stylesheets' `url()`s load; `data:` URLs need no
/// file.
fn stylesheet_urls() -> BTreeSet<String> {
    STYLESHEETS
        .iter()
        .flat_map(|name| {
            without_comments(&read(&assets_dir().join(name)))
                .split("url(")
                .skip(1)
                .filter_map(|rest| rest.find(')').map(|end| rest[..end].to_string()))
                .collect::<Vec<_>>()
        })
        .map(|url| {
            url.trim()
                .trim_matches(|quote| quote == '"' || quote == '\'')
                .to_string()
        })
        .filter(|url| !url.starts_with("data:"))
        .collect()
}

fn site() -> Site {
    Site {
        network: "Bitcoin Cash chipnet".to_string(),
        badge: "chipnet".to_string(),
    }
}

fn found_block() -> FoundBlockRow {
    FoundBlockRow {
        height: 100,
        block_hash: BlockHash::all_zeros(),
        share_hash: BlockHash::all_zeros(),
        time: 900,
        payout_address: "bchtest:qpayout".to_string(),
    }
}

fn worker(active: bool) -> WorkerRow {
    WorkerRow {
        name: "rig".to_string(),
        active,
        accepted_work: 10,
        best_share: 5,
        last_share_at: 900,
    }
}

fn share_row(is_uncle: bool) -> ShareRow {
    ShareRow {
        height: Some(1),
        blockhash: "ab".repeat(32),
        time: 900,
        payout_address: "bchtest:qpayout".to_string(),
        share_address: "tp2pool:qowner".to_string(),
        difficulty: (!is_uncle).then_some(4.0),
        is_uncle,
        is_block: true,
    }
}

fn overview(samples: Vec<Sample>, shares: Vec<ShareRow>, blocks: Vec<FoundBlockRow>) -> String {
    pages::overview(&Overview {
        site: site(),
        now: 1_000,
        tip_height: (!shares.is_empty()).then_some(30),
        pool_hashrate: Some(1e12),
        local_hashrate: Some(1e9),
        network_hashrate: Some(1e15),
        network_difficulty: Some(1e5),
        miners_online: 1,
        workers_online: 1,
        samples,
        blocks_found: blocks.len(),
        blocks,
        shares,
        shares_paging: Paging { page: 1, pages: 2 },
    })
    .into_string()
}

fn miner_report(shares: Vec<MinerShare>) -> MinerResponse {
    MinerResponse {
        address: "bchtest:qpayout".to_string(),
        address_type: "payout",
        anchor: "cd".repeat(32),
        anchor_height: 30,
        window: WindowSummary {
            newest_height: Some(29),
            oldest_height: Some(0),
            share_count: 30,
            weight: "300".to_string(),
        },
        share_count: shares.len(),
        uncle_count: 1,
        weight: "20".to_string(),
        share_fraction: 0.1,
        payout_fraction: 0.2,
        shares,
    }
}

fn miner_share(nephew: Option<String>) -> MinerShare {
    MinerShare {
        blockhash: "ef".repeat(32),
        height: 3,
        nephew,
        difficulty: "4".to_string(),
        weight: "10".to_string(),
        payout_address: "bchtest:qpayout".to_string(),
    }
}

fn share_output(uncles: Vec<String>) -> ShareLookupOutput {
    ShareLookupOutput {
        blockhash: "ab".repeat(32),
        height: Some(1),
        validation_status: "valid".to_string(),
        chain: "confirmed".to_string(),
        parent: "00".repeat(32),
        uncles,
        miner_bitcoin_address: "bchtest:qpayout".to_string(),
        miner_address: "tp2pool:qowner".to_string(),
        share_witness_root: "00".repeat(32),
        bits: "1d00ffff".to_string(),
        time: "2026-10-22 00:00:00 UTC".to_string(),
        bitcoin_header: BitcoinHeaderOutput {
            block_hash: "00".repeat(32),
            version: 0x2000_0000,
            prev_blockhash: "00".repeat(32),
            merkle_root: "00".repeat(32),
            time: "2026-10-22 00:00:00 UTC".to_string(),
            bits: "1d00ffff".to_string(),
            nonce: 7,
        },
        template_merkle_branches_count: 0,
        transactions: None,
        pruned: false,
    }
}

/// Every page the node renders, each in its full and its empty state.
fn every_page() -> String {
    let samples = vec![
        Sample {
            time: 0,
            pool: Some(1e12),
            local: Some(1e9),
            network: Some(1e15),
        },
        Sample {
            time: 60,
            pool: Some(2e12),
            local: Some(2e9),
            network: Some(1e15),
        },
    ];
    let miners = vec![
        MinerRow {
            address: "bchtest:qpayout".to_string(),
            workers: vec![worker(true), worker(false)],
            accepted_work: 10,
            best_share: 5,
            last_share_at: 900,
            payout_fraction: Some(0.2),
        },
        MinerRow {
            address: "bchtest:qother".to_string(),
            workers: Vec::new(),
            accepted_work: 0,
            best_share: 0,
            last_share_at: 0,
            payout_fraction: None,
        },
    ];
    [
        overview(
            samples,
            vec![share_row(false), share_row(true)],
            vec![found_block()],
        ),
        overview(Vec::new(), Vec::new(), Vec::new()),
        pages::blocks(&BlocksView {
            site: site(),
            now: 1_000,
            blocks_found: 30,
            blocks: vec![found_block()],
            paging: Paging { page: 2, pages: 3 },
        })
        .into_string(),
        pages::blocks(&BlocksView {
            site: site(),
            now: 1_000,
            blocks_found: 0,
            blocks: Vec::new(),
            paging: Paging { page: 1, pages: 1 },
        })
        .into_string(),
        pages::block(&BlockView {
            site: site(),
            now: 1_000,
            block: found_block(),
        })
        .into_string(),
        pages::miners(&MinersView {
            site: site(),
            now: 1_000,
            miners,
            paging: Paging { page: 1, pages: 2 },
        })
        .into_string(),
        pages::miners(&MinersView {
            site: site(),
            now: 1_000,
            miners: Vec::new(),
            paging: Paging { page: 1, pages: 1 },
        })
        .into_string(),
        pages::miner(&MinerView {
            site: site(),
            now: 1_000,
            report: miner_report(vec![miner_share(None), miner_share(Some("aa".repeat(32)))]),
            hashrate: Some(1e9),
            recent_window: Some((500, 600)),
            workers: vec![worker(true)],
            paging: Paging { page: 1, pages: 1 },
        })
        .into_string(),
        pages::miner(&MinerView {
            site: site(),
            now: 1_000,
            report: miner_report(Vec::new()),
            hashrate: None,
            recent_window: None,
            workers: Vec::new(),
            paging: Paging { page: 1, pages: 1 },
        })
        .into_string(),
        pages::share(&ShareView {
            site: site(),
            share: share_output(vec!["aa".repeat(32)]),
            is_block: true,
        })
        .into_string(),
        pages::share(&ShareView {
            site: site(),
            share: share_output(Vec::new()),
            is_block: false,
        })
        .into_string(),
        pages::help(&site()).into_string(),
        pages::error(&site(), "Not here").into_string(),
    ]
    .concat()
}

/// The classes of `used` that have no selector and are not allowed to.
fn unstyled(used: &BTreeSet<String>) -> Vec<String> {
    let styled = styled_classes();
    used.iter()
        .filter(|class| !styled.contains(*class))
        .filter(|class| !UNSTYLED_CLASSES.iter().any(|(name, _)| name == class))
        .cloned()
        .collect()
}

#[test]
fn no_stylesheet_lives_outside_the_shared_assets() {
    let root = workspace_root();
    let stray = stray_stylesheets(&root, &root);
    assert!(
        stray.is_empty(),
        "stylesheets outside {}: {stray:?}; put the rules in pages.css",
        assets_dir().display()
    );
}

#[test]
fn the_shared_stylesheets_exist() {
    let styled = styled_classes();
    assert!(styled.contains("kpi-card"));
    assert!(styled.contains("header-search"));
    assert!(styled.contains("container"));
}

#[test]
fn every_class_the_pages_render_has_a_selector() {
    let used = class_attribute_names(&every_page());
    assert!(used.contains("kpi-card") && used.contains("grid-row") && used.contains("line"));
    assert_eq!(unstyled(&used), Vec::<String>::new());
}

#[test]
fn the_pages_style_only_through_the_stylesheets() {
    let html = every_page();
    assert!(!html.contains("<style"));
    assert!(!html.contains(" style="));
    assert!(!html.contains("<script"));
}

#[test]
fn every_class_the_wasm_views_name_has_a_selector() {
    let used = web_classes();
    assert!(used.contains("site-header") && used.contains("chart-tip"));
    assert_eq!(unstyled(&used), Vec::<String>::new());
}

#[test]
fn the_wasm_views_spell_every_class_as_a_literal() {
    let computed: Vec<String> = web_sources()
        .iter()
        .flat_map(|(path, source)| {
            computed_classes(source)
                .into_iter()
                .map(move |snippet| format!("{}: {snippet}", path.display()))
        })
        .collect();
    assert_eq!(computed, Vec::<String>::new());
}

#[test]
fn the_wasm_views_style_only_through_the_stylesheets() {
    let inline: Vec<String> = web_sources()
        .iter()
        .filter(|(_, source)| {
            source.contains(" style=")
                || source.contains(" style:")
                || source.contains("inner_html")
                || source.contains("<style")
        })
        .map(|(path, _)| path.display().to_string())
        .collect();
    assert_eq!(inline, Vec::<String>::new());
}

#[test]
fn every_unstyled_class_is_still_used() {
    let used: BTreeSet<String> = class_attribute_names(&every_page())
        .into_iter()
        .chain(web_classes())
        .collect();
    let stale: Vec<&str> = UNSTYLED_CLASSES
        .iter()
        .map(|(name, _)| *name)
        .filter(|name| !used.contains(*name))
        .collect();
    assert_eq!(stale, Vec::<&str>::new());
}

#[test]
fn the_wasm_index_links_only_the_shared_assets() {
    let index = read(&web_dir().join("index.html"));
    let outside: Vec<&str> = index_links(&index)
        .into_iter()
        .filter(|link| !link.starts_with(WEB_ASSETS_HREF))
        .collect();
    assert_eq!(outside, Vec::<&str>::new());
    let missing: Vec<&str> = index_links(&index)
        .into_iter()
        .filter(|link| !web_dir().join(link).is_file())
        .collect();
    assert_eq!(missing, Vec::<&str>::new());
}

#[test]
fn the_wasm_index_loads_the_stylesheets_in_the_nodes_order() {
    let index = read(&web_dir().join("index.html"));
    assert_eq!(trunk_links(&index, "css"), STYLESHEETS.to_vec());
}

#[test]
fn the_wasm_index_has_no_script_or_style_of_its_own() {
    let index = read(&web_dir().join("index.html"));
    assert!(index.contains("<html lang=\"en\" data-theme=\"dark\">"));
    assert!(!index.contains("<script"));
    assert!(!index.contains("<style"));
    assert!(!index.contains(" style="));
}

#[test]
fn the_stylesheets_urls_resolve_in_the_wasm_bundle() {
    let index = read(&web_dir().join("index.html"));
    let copied: BTreeSet<String> = trunk_links(&index, "copy-file")
        .into_iter()
        .map(str::to_string)
        .collect();
    let urls = stylesheet_urls();
    assert!(urls.contains("ubuntu-400.woff2"));
    let unresolved: Vec<&String> = urls.iter().filter(|url| !copied.contains(*url)).collect();
    assert_eq!(unresolved, Vec::<&String>::new());
}

#[test]
fn the_wasm_views_load_only_bundled_images() {
    let index = read(&web_dir().join("index.html"));
    let copied = trunk_links(&index, "copy-file");
    let images: Vec<String> = web_sources()
        .iter()
        .flat_map(|(_, source)| {
            attribute_values(source, "src")
                .into_iter()
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(images.contains(&"logo-horizontal-wt.svg".to_string()));
    let unbundled: Vec<&String> = images
        .iter()
        .filter(|image| !copied.contains(&image.as_str()))
        .collect();
    assert_eq!(unbundled, Vec::<&String>::new());
}

#[test]
fn the_node_links_the_same_stylesheets() {
    let page = pages::help(&site()).into_string();
    assert!(page.contains(&assets::href("pico.min.css")));
    assert!(page.contains(&assets::href("dashboard.css")));
}

#[test]
fn the_landing_page_loads_the_stylesheets_in_the_nodes_order() {
    assert_eq!(stylesheet_links(&landing_page()), STYLESHEETS.to_vec());
}

#[test]
fn the_landing_page_has_no_script_or_style_of_its_own() {
    let page = landing_page();
    assert!(page.contains("<html lang=\"en\" data-theme=\"dark\">"));
    assert!(!page.contains("<script"));
    assert!(!page.contains("<style"));
    assert!(!page.contains(" style="));
    assert!(!page.contains("javascript:"));
    assert_eq!(event_handlers(&page), Vec::<&str>::new());
}

#[test]
fn event_handlers_are_found_but_prose_is_not() {
    assert_eq!(
        event_handlers("<body onload=\"run()\"><p>one only on</p>"),
        vec!["onload"]
    );
}

#[test]
fn every_class_the_landing_page_names_has_a_selector() {
    let used = class_attribute_names(&landing_page());
    assert!(used.contains("landing") && used.contains("chain-share") && used.contains("kpi-card"));
    assert_eq!(unstyled(&used), Vec::<String>::new());
}

#[test]
fn the_landing_page_links_only_what_the_site_publishes() {
    let page = landing_page();
    let ids: BTreeSet<&str> = attribute_values(&page, "id").into_iter().collect();
    let links = index_links(&page);
    assert!(links.contains(&"../p2poolv2_api/src/api/dashboard/assets/logo-horizontal-wt.svg"));
    assert!(links.contains(&"#run") && links.contains(&LANDING_DOCS_HREF));
    assert!(links.contains(&"#donate") && links.contains(&"donate-bch.svg"));
    let unpublished: Vec<&str> = links
        .into_iter()
        .filter(|link| !is_publishable_landing_link(link, &ids))
        .collect();
    assert_eq!(unpublished, Vec::<&str>::new());
}

#[test]
fn landing_links_are_checked_against_the_page_and_the_assets() {
    let ids = BTreeSet::from(["top"]);
    assert!(is_publishable_landing_link("#top", &ids));
    assert!(!is_publishable_landing_link("#missing", &ids));
    assert!(is_publishable_landing_link("https://example.org/", &ids));
    assert!(!is_publishable_landing_link("http://example.org/", &ids));
    assert!(is_publishable_landing_link("docs/", &ids));
    assert!(is_publishable_landing_link(
        "../p2poolv2_api/src/api/dashboard/assets/pages.css",
        &ids
    ));
    assert!(!is_publishable_landing_link(
        "../p2poolv2_api/src/api/dashboard/assets/missing.css",
        &ids
    ));
    assert!(!is_publishable_landing_link("styles.css", &ids));
    assert!(is_publishable_landing_link(
        "bitcoincash:qze95tc2dqrnltvxfa5yhunwx5952f34fvw4g6365k",
        &ids
    ));
    assert!(!is_publishable_landing_link("bchtest:qq", &ids));
    assert!(is_publishable_landing_link("donate-bch.svg", &ids));
    assert!(!is_publishable_landing_link("missing.svg", &ids));
    assert!(!is_publishable_landing_link(
        "../website/donate-bch.svg",
        &ids
    ));
}

#[test]
fn the_pages_workflow_serves_the_assets_where_the_landing_page_looks() {
    let workflow = read(
        &workspace_root()
            .join(".github")
            .join("workflows")
            .join("docs.yml"),
    );
    let assets = WEB_ASSETS_HREF
        .strip_prefix("../")
        .and_then(|path| path.strip_suffix('/'))
        .expect("a relative directory");
    assert!(workflow.contains(&format!("assets={assets}")));
    assert!(workflow.contains(r#"sed "s#\.\./$assets/#assets/#g" website/index.html"#));
    let docs = LANDING_DOCS_HREF.trim_end_matches('/');
    assert!(workflow.contains(&format!("cp -R target/book \"$site/{docs}\"")));
    assert!(workflow.contains(r#"cp website/LICENSE-MIT website/*.svg "$site/""#));
    assert!(workflow.contains("- \"website/**\""));
    assert!(workflow.contains("vars.DOCS_PAGES == 'true'"));
}

#[test]
fn selectors_are_read_without_declarations_or_comments() {
    let classes = selector_classes(
        "/* .commented { } */ header .brand li > a, .a.b:not(.c) { margin: 0.5rem; \
         src: url(x.woff2) } @media (min-width: 1.5em) { .d { } }",
    );
    assert_eq!(
        classes.into_iter().collect::<Vec<_>>(),
        vec!["a", "b", "brand", "c", "d"]
    );
}

#[test]
fn class_toggles_and_literals_are_found_in_view_source() {
    let source = r#"<p class="kpi-value one" class:kpi-value-green=green class:x>"#;
    assert_eq!(
        class_attribute_names(source)
            .into_iter()
            .collect::<Vec<_>>(),
        vec!["kpi-value", "one"]
    );
    assert_eq!(
        class_toggles(source).into_iter().collect::<Vec<_>>(),
        vec!["kpi-value-green"]
    );
    assert_eq!(computed_classes("<a class=name>"), vec!["class=name>"]);
}

#[test]
fn the_wasm_app_formats_hashrates_as_the_pages_do() {
    assert_eq!(web_format::fmt_hashrate(0.0), pages::fmt_hashrate(0.0));
    assert_eq!(
        web_format::fmt_hashrate(665_000.0),
        pages::fmt_hashrate(665_000.0)
    );
    assert_eq!(
        web_format::fmt_hashrate(38_790_000_000_000.0),
        pages::fmt_hashrate(38_790_000_000_000.0)
    );
    assert_eq!(web_format::fmt_hashrate(5e21), pages::fmt_hashrate(5e21));
}

#[test]
fn the_wasm_app_formats_difficulties_as_the_pages_do() {
    assert_eq!(web_format::fmt_difficulty(4.5), pages::fmt_difficulty(4.5));
    assert_eq!(
        web_format::fmt_difficulty(665_151.95),
        pages::fmt_difficulty(665_151.95)
    );
    assert_eq!(
        web_format::fmt_difficulty(12_345_678_901.0),
        pages::fmt_difficulty(12_345_678_901.0)
    );
}

#[test]
fn the_wasm_app_formats_times_as_the_pages_do() {
    assert_eq!(web_format::fmt_utc(0), pages::fmt_utc(0));
    assert_eq!(
        web_format::fmt_utc(951_782_400),
        pages::fmt_utc(951_782_400)
    );
    assert_eq!(
        web_format::fmt_utc(1_792_675_942),
        pages::fmt_utc(1_792_675_942)
    );
    assert_eq!(
        web_format::fmt_utc(4_102_444_800),
        pages::fmt_utc(4_102_444_800)
    );
    assert_eq!(
        web_format::fmt_day_time(1_792_000_000),
        pages::fmt_day_time(1_792_000_000)
    );
    assert_eq!(
        web_format::fmt_day_time(1_772_323_200),
        pages::fmt_day_time(1_772_323_200)
    );
}

#[test]
fn the_wasm_app_formats_ages_as_the_pages_do() {
    assert_eq!(web_format::fmt_ago(100, 58), pages::fmt_ago(100, 58));
    assert_eq!(web_format::fmt_ago(1_000, 100), pages::fmt_ago(1_000, 100));
    assert_eq!(web_format::fmt_ago(20_000, 0), pages::fmt_ago(20_000, 0));
    assert_eq!(web_format::fmt_ago(200_000, 0), pages::fmt_ago(200_000, 0));
    assert_eq!(web_format::fmt_ago(0, 100), pages::fmt_ago(0, 100));
}

#[test]
fn the_wasm_app_shortens_and_links_as_the_pages_do() {
    let hash = "00000000000cb752459c2b08e1955e5f33252b0207b1c7b5b2f069d5254b9025";
    assert_eq!(web_format::short(hash), pages::short(hash));
    assert_eq!(web_format::short("abc"), pages::short("abc"));
    assert_eq!(
        web_format::url_component("bchtest:q r&x"),
        pages::url_component("bchtest:q r&x")
    );
    assert_eq!(web_format::percent(0.123_45), pages::percent(0.123_45));
    assert_eq!(
        web_format::optional(None, web_format::fmt_hashrate),
        pages::optional(None, pages::fmt_hashrate)
    );
}
