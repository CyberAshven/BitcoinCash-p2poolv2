// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The donation QR codes on the landing page.
//!
//! `website/index.html` asks for donations to the P2Pool donation address,
//! [`BITCOIN_CASH_DONATION_ADDRESS`], in two forms: the plain CashAddr, and
//! its CashTokens token-aware form, which names the same key and tells a
//! wallet it may send tokens too. Each has a QR code in a static SVG beside
//! the page, so the page needs no script. This module draws them, the
//! `donate_qr` binary writes them, and a test fails when a committed SVG no
//! longer encodes its address.

use crate::{ToolError, ToolResult, repo};
use p2poolv2_config::{BITCOIN_CASH_DONATION_ADDRESS, BITCOIN_CASH_DONATION_PUBKEY_HASH};
use p2poolv2_wallet::bch_share::{CashAddrType, encode_token_aware_cashaddr};
use qrcode::{Color, EcLevel, QrCode};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

/// Light modules around the code: the four the QR specification asks for.
const QUIET_ZONE: usize = 4;

/// The CashAddr prefix of Bitcoin Cash mainnet, where donations are sent.
const MAINNET_PREFIX: &str = "bitcoincash";

/// One QR code on the landing page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DonateCode {
    /// The SVG's file name in `website/`, as the page links it.
    pub file_name: &'static str,
    /// The address the code encodes, with its `bitcoincash:` prefix, so a
    /// wallet reads it as a payment URI.
    pub address: String,
}

/// [`BITCOIN_CASH_DONATION_ADDRESS`] in its token-aware form (`z...`): the
/// same public key hash, CashAddr type 2.
pub fn token_aware_donation_address() -> String {
    encode_token_aware_cashaddr(
        MAINNET_PREFIX,
        CashAddrType::PayToPublicKeyHash,
        &BITCOIN_CASH_DONATION_PUBKEY_HASH,
    )
}

/// The page's two codes: plain BCH, then token-aware.
pub fn donate_codes() -> [DonateCode; 2] {
    [
        DonateCode {
            file_name: "donate-bch.svg",
            address: BITCOIN_CASH_DONATION_ADDRESS.to_string(),
        },
        DonateCode {
            file_name: "donate-tokens.svg",
            address: token_aware_donation_address(),
        },
    ]
}

/// Where the landing page and its QR codes live.
pub fn website_dir() -> PathBuf {
    repo::root().join("website")
}

/// `text` as a QR code in an SVG, at error correction level M and the
/// smallest version that holds it, with a quiet zone.
///
/// One SVG unit is one module. Each row's runs of dark modules are one
/// rectangle each in a single path, drawn black on white whatever the
/// page's theme, which is what scanners expect.
pub fn qr_svg(text: &str) -> ToolResult<String> {
    let code = QrCode::with_error_correction_level(text, EcLevel::M).map_err(|error| {
        ToolError::message(format!("cannot encode {text} as a QR code: {error}"))
    })?;
    let width = code.width();
    let size = width + 2 * QUIET_ZONE;
    let mut path = String::with_capacity(width * width * 4);
    for y in 0..width {
        let mut x = 0;
        while x < width {
            if code[(x, y)] != Color::Dark {
                x += 1;
                continue;
            }
            let start = x;
            while x < width && code[(x, y)] == Color::Dark {
                x += 1;
            }
            let run = x - start;
            write!(
                path,
                "M{} {}h{run}v1h-{run}z",
                start + QUIET_ZONE,
                y + QUIET_ZONE
            )
            .expect("writing to a String cannot fail");
        }
    }
    Ok(format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {size} {size}\" \
         shape-rendering=\"crispEdges\">\n\
         <title>{text}</title>\n\
         <rect width=\"{size}\" height=\"{size}\" fill=\"#fff\"/>\n\
         <path fill=\"#000\" d=\"{path}\"/>\n\
         </svg>\n"
    ))
}

/// Write every code into `directory`, returning the files written.
pub fn write_codes(directory: &Path) -> ToolResult<Vec<PathBuf>> {
    let mut written = Vec::with_capacity(2);
    for code in donate_codes() {
        let path = directory.join(code.file_name);
        fs::write(&path, qr_svg(&code.address)?)
            .map_err(|error| ToolError::io("write", &path, error))?;
        written.push(path);
    }
    Ok(written)
}

#[cfg(test)]
mod tests;
