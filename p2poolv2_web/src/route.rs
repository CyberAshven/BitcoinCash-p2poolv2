// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The app's pages, addressed by the URL fragment.
//!
//! Each page mirrors one of the node's `/dashboard` pages, at the same path
//! after `#`: `#/blocks` is `/dashboard/blocks`, `#/miner?address=` is
//! `/dashboard/miner?address=`. The fragment keeps every page a real link
//! (new tab, back button) on any static server, and never collides with the
//! node's API paths when `web_root` serves the app. Pure and unit tested.

use crate::format::{url_component, url_decode};

/// A page of the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    /// The overview, at a page of its recent shares.
    Dashboard {
        page: usize,
    },
    Blocks {
        page: usize,
    },
    Block(String),
    Miners {
        page: usize,
    },
    Miner {
        address: String,
        page: usize,
    },
    Share(String),
    Help,
    /// The API URL and login; the node's own pages need none.
    Settings,
    /// Where a search goes: a share, a found block or a miner.
    Search(String),
}

/// The menu bar's tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Dashboard,
    Blocks,
    Miners,
    Help,
    /// A page that belongs to no tab, such as an error or the settings.
    None,
}

/// The menu bar: tab, label and page, as the node's pages list them.
pub const TABS: [(Tab, &str, &str); 4] = [
    (Tab::Dashboard, "Dashboard", "#/"),
    (Tab::Blocks, "Blocks", "#/blocks"),
    (Tab::Miners, "Miners", "#/miners"),
    (Tab::Help, "Help", "#/help"),
];

/// Rows on one page of a long table, as on the node's pages. The live share
/// list keeps one page of shares, so it never grows.
pub const PAGE_SIZE: usize = 25;

/// Which page of a long table is shown, counted from 1, as the node's
/// `Paging` picks it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Paging {
    pub page: usize,
    pub pages: usize,
}

impl Paging {
    /// The page `requested` of `rows` rows, clamped to the pages there are.
    pub fn of(requested: usize, rows: usize) -> Self {
        let pages = rows.div_ceil(PAGE_SIZE).max(1);
        Self {
            page: requested.clamp(1, pages),
            pages,
        }
    }

    /// The rows of `all` on this page.
    pub fn slice<'a, T>(&self, all: &'a [T]) -> &'a [T] {
        let start = ((self.page - 1) * PAGE_SIZE).min(all.len());
        let end = (start + PAGE_SIZE).min(all.len());
        &all[start..end]
    }

    /// Rows on the pages before this one.
    pub fn skipped(&self) -> usize {
        (self.page - 1) * PAGE_SIZE
    }
}

/// The value of `key` in a `a=1&b=2` query, decoded.
fn query_value(query: &str, key: &str) -> Option<String> {
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(name, _)| *name == key)
        .map(|(_, value)| url_decode(value))
}

/// `?page=` as a page number counted from 1; anything else is the first.
fn page_of(query: &str) -> usize {
    query_value(query, "page")
        .and_then(|page| page.parse().ok())
        .unwrap_or(1)
        .max(1)
}

/// `?page=` for page `page`, nothing for the first.
fn page_suffix(separator: char, page: usize) -> String {
    if page > 1 {
        format!("{separator}page={page}")
    } else {
        String::new()
    }
}

impl Route {
    /// The page a fragment such as `#/miners?page=2` addresses. An unknown
    /// fragment is the overview.
    pub fn parse(fragment: &str) -> Route {
        let fragment = fragment.trim_start_matches('#');
        let (path, query) = fragment.split_once('?').unwrap_or((fragment, ""));
        let path = path.trim_start_matches('/').trim_end_matches('/');
        let (head, rest) = path.split_once('/').unwrap_or((path, ""));
        match (head, rest) {
            ("blocks", _) => Route::Blocks {
                page: page_of(query),
            },
            ("block", hash) if !hash.is_empty() => Route::Block(url_decode(hash)),
            ("miners", _) => Route::Miners {
                page: page_of(query),
            },
            ("miner", _) => Route::Miner {
                address: query_value(query, "address").unwrap_or_default(),
                page: page_of(query),
            },
            ("share", hash) if !hash.is_empty() => Route::Share(url_decode(hash)),
            ("help", _) => Route::Help,
            ("settings", _) => Route::Settings,
            ("search", _) => Route::Search(query_value(query, "q").unwrap_or_default()),
            _ => Route::Dashboard {
                page: page_of(query),
            },
        }
    }

    /// The link to this page.
    pub fn href(&self) -> String {
        match self {
            Route::Dashboard { page } => format!("#/{}", page_suffix('?', *page)),
            Route::Blocks { page } => format!("#/blocks{}", page_suffix('?', *page)),
            Route::Block(hash) => format!("#/block/{}", url_component(hash)),
            Route::Miners { page } => format!("#/miners{}", page_suffix('?', *page)),
            Route::Miner { address, page } => format!(
                "#/miner?address={}{}",
                url_component(address),
                page_suffix('&', *page)
            ),
            Route::Share(hash) => format!("#/share/{}", url_component(hash)),
            Route::Help => "#/help".to_string(),
            Route::Settings => "#/settings".to_string(),
            Route::Search(text) => format!("#/search?q={}", url_component(text)),
        }
    }
}

/// Link to a miner's page.
pub fn miner_href(address: &str) -> String {
    Route::Miner {
        address: address.to_string(),
        page: 1,
    }
    .href()
}

/// Link to a share's page.
pub fn share_href(blockhash: &str) -> String {
    Route::Share(blockhash.to_string()).href()
}

/// Link to a found block's page.
pub fn block_href(block_hash: &str) -> String {
    Route::Block(block_hash.to_string()).href()
}

/// Whether a search is a 64-digit hex hash rather than an address.
pub fn looks_like_hash(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_fragment_is_the_overview() {
        assert_eq!(Route::parse(""), Route::Dashboard { page: 1 });
        assert_eq!(Route::parse("#/"), Route::Dashboard { page: 1 });
        assert_eq!(Route::parse("#/nowhere"), Route::Dashboard { page: 1 });
    }

    #[test]
    fn test_pages_parse_and_clamp() {
        assert_eq!(Route::parse("#/?page=3"), Route::Dashboard { page: 3 });
        assert_eq!(Route::parse("#/miners?page=0"), Route::Miners { page: 1 });
        assert_eq!(Route::parse("#/blocks?page=x"), Route::Blocks { page: 1 });
    }

    #[test]
    fn test_miner_route_round_trips_its_address() {
        let route = Route::Miner {
            address: "bchtest:qr3 &x".to_string(),
            page: 2,
        };
        assert_eq!(route.href(), "#/miner?address=bchtest:qr3%20%26x&page=2");
        assert_eq!(Route::parse(&route.href()), route);
    }

    #[test]
    fn test_share_and_block_routes_round_trip() {
        let share = Route::Share("ab".repeat(32));
        assert_eq!(Route::parse(&share.href()), share);
        let block = Route::Block("cd".repeat(32));
        assert_eq!(Route::parse(&block.href()), block);
        assert_eq!(Route::parse("#/share/"), Route::Dashboard { page: 1 });
    }

    #[test]
    fn test_search_decodes_form_input() {
        assert_eq!(
            Route::parse("#/search?q=+tb1q+x"),
            Route::Search(" tb1q x".to_string())
        );
    }

    #[test]
    fn test_paging_clamps_and_slices() {
        let rows: Vec<usize> = (0..60).collect();
        let first = Paging::of(1, rows.len());
        assert_eq!(first, Paging { page: 1, pages: 3 });
        assert_eq!(first.slice(&rows), &rows[..25]);
        let last = Paging::of(9, rows.len());
        assert_eq!(last, Paging { page: 3, pages: 3 });
        assert_eq!(last.slice(&rows), &rows[50..]);
        assert_eq!(last.skipped(), 50);
        assert_eq!(Paging::of(0, 0), Paging { page: 1, pages: 1 });
    }

    #[test]
    fn test_hashes_are_told_from_addresses() {
        assert!(looks_like_hash(&"0f".repeat(32)));
        assert!(!looks_like_hash("bchtest:qr3rryn6"));
        assert!(!looks_like_hash(&"0g".repeat(32)));
    }
}
