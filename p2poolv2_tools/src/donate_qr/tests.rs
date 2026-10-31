// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use p2poolv2_config::{
    BitcoinCashNetwork, Chain, bitcoin_cash_donation_address, parse_payout_address,
};

/// The addresses as Pickaxe publishes them, the same as P2Pool's.
const PUBLISHED_BCH: &str = "bitcoincash:qze95tc2dqrnltvxfa5yhunwx5952f34fvw4g6365k";
const PUBLISHED_TOKENS: &str = "bitcoincash:zze95tc2dqrnltvxfa5yhunwx5952f34fvflmylut9";

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

#[test]
fn the_codes_carry_the_published_addresses() {
    let addresses: Vec<String> = donate_codes()
        .into_iter()
        .map(|code| code.address)
        .collect();
    assert_eq!(addresses, vec![PUBLISHED_BCH, PUBLISHED_TOKENS]);
    assert_eq!(BITCOIN_CASH_DONATION_ADDRESS, PUBLISHED_BCH);
}

#[test]
fn both_addresses_pay_the_donation_key() {
    let donation = bitcoin_cash_donation_address(BitcoinCashNetwork::Mainnet);
    for code in donate_codes() {
        let paid = parse_payout_address(
            &code.address,
            Chain::BitcoinCash,
            BitcoinCashNetwork::Mainnet.legacy_network(),
            Some(BitcoinCashNetwork::Mainnet),
        )
        .unwrap_or_else(|error| panic!("{}: {error}", code.address));
        assert_eq!(paid, donation, "{}", code.address);
    }
}

#[test]
fn the_committed_qr_codes_encode_the_donation_addresses() {
    for code in donate_codes() {
        let path = website_dir().join(code.file_name);
        assert!(
            read(&path) == qr_svg(&code.address).unwrap(),
            "{} does not encode {}; run `cargo run -p p2poolv2_tools --bin donate_qr`",
            path.display(),
            code.address
        );
    }
}

/// The dark modules an SVG from [`qr_svg`] draws, as rows of its viewBox.
fn modules(svg: &str) -> Vec<Vec<bool>> {
    let size: usize = svg
        .split("viewBox=\"0 0 ")
        .nth(1)
        .and_then(|rest| rest.split(' ').next())
        .and_then(|size| size.parse().ok())
        .expect("a square viewBox");
    let path = svg
        .split("d=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("one path");
    let mut grid = vec![vec![false; size]; size];
    for rectangle in path.split('z').filter(|rectangle| !rectangle.is_empty()) {
        let (origin, run) = rectangle
            .strip_prefix('M')
            .and_then(|rest| rest.split_once('h'))
            .expect("M x y h run");
        let (x, y) = origin.split_once(' ').expect("x y");
        let (x, y): (usize, usize) = (x.parse().unwrap(), y.parse().unwrap());
        let run: usize = run.split_once('v').expect("v1").0.parse().unwrap();
        for module in &mut grid[y][x..x + run] {
            *module = true;
        }
    }
    grid
}

/// Scan an SVG from [`qr_svg`] with a decoder that shares no code with
/// the encoder, at four pixels a module.
fn scan(svg: &str) -> String {
    const SCALE: usize = 4;
    let grid = modules(svg);
    let pixels = grid.len() * SCALE;
    let mut image =
        rqrr::PreparedImage::prepare_from_bitmap(pixels, pixels, |x, y| grid[y / SCALE][x / SCALE]);
    let grids = image.detect_grids();
    assert_eq!(grids.len(), 1, "one QR code in the image");
    grids[0].decode().expect("a readable QR code").1
}

#[test]
fn each_committed_qr_code_scans_as_its_address() {
    for code in donate_codes() {
        let svg = read(&website_dir().join(code.file_name));
        assert_eq!(scan(&svg), code.address, "{}", code.file_name);
    }
}

#[test]
fn the_landing_page_shows_each_code_with_its_address() {
    let page = read(&website_dir().join("index.html"));
    assert!(page.contains("id=\"donate\""));
    assert!(page.contains("href=\"#donate\""));
    for code in donate_codes() {
        assert!(page.contains(&format!("src=\"{}\"", code.file_name)));
        assert!(page.contains(&format!("href=\"{}\"", code.address)));
        assert!(page.contains(&format!(">{}<", code.address)));
    }
}

#[test]
fn the_sponsor_button_links_the_donate_section() {
    let funding = read(&repo::root().join(".github").join("FUNDING.yml"));
    assert!(funding.contains("- \"https://cyberashven.github.io/BitcoinCash-p2poolv2/#donate\""));
}

#[test]
fn every_sponsor_link_shows_both_addresses() {
    let root = repo::root();
    for path in [
        root.join(".github").join("FUNDING.yml"),
        root.join("README.adoc"),
        root.join("docs").join("introduction.md"),
    ] {
        let text = read(&path);
        assert!(text.contains("#donate"), "{}", path.display());
        for code in donate_codes() {
            assert!(
                text.contains(&code.address),
                "{} lacks {}",
                path.display(),
                code.address
            );
        }
    }
}

#[test]
fn a_code_is_drawn_module_by_module_inside_its_quiet_zone() {
    // Version 1 is 21 modules wide: 29 with the quiet zone.
    let svg = qr_svg("bitcoincash").unwrap();
    assert!(svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 29 29\""));
    assert!(svg.ends_with("</svg>\n"));
    // The top-left finder pattern's first row: seven dark modules.
    assert!(svg.contains("d=\"M4 4h7v1h-7z"));
    let path = svg
        .split("d=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .unwrap();
    for rectangle in path.split('z').filter(|rectangle| !rectangle.is_empty()) {
        let (origin, run) = rectangle
            .strip_prefix('M')
            .and_then(|rest| rest.split_once('h'))
            .unwrap();
        let (x, y) = origin.split_once(' ').unwrap();
        let (x, y): (usize, usize) = (x.parse().unwrap(), y.parse().unwrap());
        let run: usize = run.split_once('v').unwrap().0.parse().unwrap();
        assert!(
            x >= QUIET_ZONE && (QUIET_ZONE..25).contains(&y),
            "{rectangle}"
        );
        assert!(run > 0 && x + run <= 25, "{rectangle}");
    }
}

#[test]
fn the_donation_addresses_fit_version_4() {
    for code in donate_codes() {
        let svg = qr_svg(&code.address).unwrap();
        assert!(svg.contains("viewBox=\"0 0 41 41\""), "{}", code.address);
    }
}
