// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The dashboard's stylesheets, fonts and logos, built into the node binary.
//!
//! The design is CashDragon0's dashboard from PR #8: Pico CSS, his
//! stylesheet, the Ubuntu font and the Bitcoin Cash logo. `pages.css` adds
//! what the server-rendered pages need on top of it. The files are compiled
//! in with `include_bytes!`, so the node serves them from static memory and
//! reads nothing from disk. See `assets/README.md` for their sources and
//! licences.
//!
//! They are also the Rust/WASM dashboard's (`p2poolv2_web`) only styling:
//! trunk bundles the same files, and `style_guard` keeps it that way.

use axum::extract::Path;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};

/// Changed whenever an asset changes, so browsers holding a cached copy fetch
/// the new one. Pages link every asset with `?v=` this value.
pub(crate) const ASSET_VERSION: &str = "3";

/// How long browsers may reuse an asset before asking again.
const CACHE_CONTROL: &str = "public, max-age=86400";

/// CashDragon0's stylesheet followed by the additions for these pages, served
/// as one file.
const DASHBOARD_CSS: &str = concat!(
    include_str!("assets/dashboard.css"),
    "\n",
    include_str!("assets/pages.css")
);

/// The content type and bytes of the asset called `name`, if there is one.
fn asset(name: &str) -> Option<(&'static str, &'static [u8])> {
    Some(match name {
        "pico.min.css" => (
            "text/css; charset=utf-8",
            include_bytes!("assets/pico.min.css"),
        ),
        "dashboard.css" => ("text/css; charset=utf-8", DASHBOARD_CSS.as_bytes()),
        "ubuntu-400.woff2" => ("font/woff2", include_bytes!("assets/ubuntu-400.woff2")),
        "ubuntu-700.woff2" => ("font/woff2", include_bytes!("assets/ubuntu-700.woff2")),
        "logo-horizontal-wt.svg" => (
            "image/svg+xml",
            include_bytes!("assets/logo-horizontal-wt.svg"),
        ),
        "logo-circle.svg" => ("image/svg+xml", include_bytes!("assets/logo-circle.svg")),
        _ => return None,
    })
}

/// The URL a page links the asset `name` with.
pub(crate) fn href(name: &str) -> String {
    format!("/dashboard/assets/{name}?v={ASSET_VERSION}")
}

/// Serve the asset in the path, or 404.
pub(crate) async fn serve(Path(name): Path<String>) -> Response {
    match asset(&name) {
        Some((content_type, body)) => (
            [
                (header::CONTENT_TYPE, content_type),
                (header::CACHE_CONTROL, CACHE_CONTROL),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            ],
            body,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_linked_asset_exists() {
        for name in [
            "pico.min.css",
            "dashboard.css",
            "ubuntu-400.woff2",
            "ubuntu-700.woff2",
            "logo-horizontal-wt.svg",
            "logo-circle.svg",
        ] {
            let (_, body) = asset(name).unwrap_or_else(|| panic!("{name} is missing"));
            assert!(!body.is_empty(), "{name} is empty");
        }
        assert!(asset("dashboard.js").is_none());
        assert!(asset("../Cargo.toml").is_none());
    }

    #[test]
    fn the_stylesheet_carries_both_parts() {
        assert!(DASHBOARD_CSS.contains(".kpi-card"));
        assert!(DASHBOARD_CSS.contains(".header-search"));
    }
}
