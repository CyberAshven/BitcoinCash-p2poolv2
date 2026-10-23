// SPDX-FileCopyrightText: 2024 PDM Authors
// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! BCHN's status, ported from PDM's Bitcoin Core status view: chain, peers
//! and mempool tabs, from `getblockchaininfo`, `getnetworkinfo`,
//! `getpeerinfo`, `getmempoolinfo` and `getmininginfo`.

use crate::format;
use crate::model::{BchnPeer, BchnStatus};
use crate::network::{self, CashNetwork};
use crate::remote::Remote;
use crate::views::{
    PICK_A_CONFIG, alert, field, header_row, home::sync_state, remote_lines, remote_message,
    render_message, tab_bar, tabs_and_content, titled,
};
use ratatui::{
    prelude::*,
    widgets::{Cell, Paragraph, Row, Table, Wrap},
};

/// The tabs, in order.
pub const BCHN_TABS: [&str; 3] = ["Chain", "Peers", "Mempool"];

/// What the BCHN screen shows.
pub struct BchnScreen<'a> {
    pub status: &'a Remote<BchnStatus>,
    /// The network the node config names, to check BCHN against.
    pub expected: Option<CashNetwork>,
    /// Index into [`BCHN_TABS`].
    pub tab: usize,
    /// Unix seconds, for connection times.
    pub now: u64,
}

pub fn render(frame: &mut Frame, area: Rect, screen: &BchnScreen) {
    let (tabs_area, content_area) = tabs_and_content(area);
    frame.render_widget(tab_bar(&BCHN_TABS, screen.tab), tabs_area);
    match screen.tab {
        0 => render_chain(frame, content_area, screen),
        1 => render_peers(frame, content_area, screen),
        _ => render_mempool(frame, content_area, screen),
    }
}

fn render_chain(frame: &mut Frame, area: Rect, screen: &BchnScreen) {
    let lines = remote_lines(screen.status, "BCHN chain info", PICK_A_CONFIG, |status| {
        chain_lines(status, screen.expected)
    });
    let paragraph = Paragraph::new(lines)
        .block(titled(" Chain "))
        .wrap(Wrap { trim: true });
    frame.render_widget(paragraph, area);
}

fn chain_lines(status: &BchnStatus, expected: Option<CashNetwork>) -> Vec<Line<'static>> {
    let chain = &status.chain;
    let found = CashNetwork::from_bchn_chain(&chain.chain);
    let mut lines = Vec::with_capacity(16);
    if let Some(warning) = network::mismatch(expected, "BCHN", found, &chain.chain) {
        lines.push(alert(warning));
    }
    lines.push(field(
        "Network",
        found.map_or(chain.chain.clone(), |network| network.name().to_string()),
    ));
    lines.push(field("Node", status.network.subversion.clone()));
    lines.push(field("Blocks", format::with_commas(chain.blocks)));
    lines.push(field("Headers", format::with_commas(chain.headers)));
    lines.push(field("State", sync_state(status)));
    lines.push(field("Best block hash", chain.best_block_hash.clone()));
    lines.push(field("Difficulty", format::difficulty(chain.difficulty)));
    lines.push(field(
        "Network hashrate",
        format::optional(status.network_hashrate, format::hashrate),
    ));
    lines.push(field(
        "Verification progress",
        format::optional(chain.verification_progress, format::percent),
    ));
    lines.push(field(
        "Initial block download",
        format::yes_no(chain.initial_block_download),
    ));
    lines.push(field(
        "Median time",
        format::optional(chain.median_time, format::utc),
    ));
    lines.push(field(
        "Size on disk",
        format::optional(chain.size_on_disk, format::bytes),
    ));
    lines.push(field("Pruned", format::yes_no(chain.pruned)));
    lines.push(field("Connections", status.network.connections.to_string()));
    if let Some(warnings) = chain.warnings.as_deref().filter(|text| !text.is_empty()) {
        lines.push(alert(format!("BCHN warns: {warnings}")));
    }
    lines
}

fn render_peers(frame: &mut Frame, area: Rect, screen: &BchnScreen) {
    let peers = match screen.status.ready() {
        Some(status) if !status.peers.is_empty() => &status.peers,
        _ => {
            let message = remote_message(
                screen.status,
                "BCHN peers",
                PICK_A_CONFIG,
                "BCHN has no peers.",
            );
            render_message(frame, area, " Peers ", message);
            return;
        }
    };
    let rows: Vec<Row> = peers
        .iter()
        .map(|peer| peer_row(peer, screen.now))
        .collect();
    let title = format!(" Peers ({}) ", peers.len());
    let widths = [
        Constraint::Length(22),
        Constraint::Length(3),
        Constraint::Length(9),
        Constraint::Length(8),
        Constraint::Length(13),
        Constraint::Min(10),
    ];
    let table = Table::new(rows, widths)
        .block(titled(&title))
        .header(header_row(&[
            "Address",
            "Dir",
            "Blocks",
            "Ping",
            "Connected",
            "Version",
        ]))
        .column_spacing(1);
    frame.render_widget(table, area);
}

fn peer_row(peer: &BchnPeer, now: u64) -> Row<'static> {
    Row::new([
        Cell::from(format::short(&peer.address, 22)),
        Cell::from(if peer.inbound { "in" } else { "out" }),
        Cell::from(format::optional(peer.synced_blocks, |blocks| {
            blocks.to_string()
        })),
        Cell::from(format::optional(peer.ping_time, |seconds| {
            format!("{:.0} ms", seconds * 1000.0)
        })),
        Cell::from(format::optional(peer.connected_at, |since| {
            format::duration(now.saturating_sub(since))
        })),
        Cell::from(peer.user_agent.clone()),
    ])
}

fn render_mempool(frame: &mut Frame, area: Rect, screen: &BchnScreen) {
    let lines = remote_lines(
        screen.status,
        "BCHN mempool info",
        PICK_A_CONFIG,
        |status| {
            let mempool = &status.mempool;
            vec![
                field("Transactions", format::with_commas(mempool.size)),
                field("Size", format::bytes(mempool.bytes)),
                field("Memory used", format::bytes(mempool.usage)),
                field("Memory limit", format::bytes(mempool.max_mempool)),
                field("Minimum fee", format::bch_per_kb(mempool.min_fee)),
                field(
                    "Relay fee",
                    format::optional(status.network.relay_fee, format::bch_per_kb),
                ),
                field("Loaded", format::yes_no(mempool.loaded)),
            ]
        },
    );
    let paragraph = Paragraph::new(lines)
        .block(titled(" Mempool "))
        .wrap(Wrap { trim: true });
    frame.render_widget(paragraph, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{NOW, sample_bchn_status, screen_text};

    #[test]
    fn test_chain_tab() {
        let status = Remote::Ready(sample_bchn_status());
        let text = screen_text(90, 22, |frame| {
            let area = frame.area();
            super::render(
                frame,
                area,
                &BchnScreen {
                    status: &status,
                    expected: Some(CashNetwork::Chipnet),
                    tab: 0,
                    now: NOW,
                },
            );
        });
        insta::assert_snapshot!(text);
    }

    #[test]
    fn test_chain_tab_warns_of_another_network() {
        let status = Remote::Ready(sample_bchn_status());
        let text = screen_text(90, 22, |frame| {
            let area = frame.area();
            super::render(
                frame,
                area,
                &BchnScreen {
                    status: &status,
                    expected: Some(CashNetwork::Mainnet),
                    tab: 0,
                    now: NOW,
                },
            );
        });
        assert!(
            text.contains("BCHN is on chipnet but the node config says mainnet"),
            "{text}"
        );
    }

    #[test]
    fn test_chain_tab_shows_a_failure() {
        let status: Remote<BchnStatus> = Remote::Failed("connection refused".into());
        let text = screen_text(90, 10, |frame| {
            let area = frame.area();
            super::render(
                frame,
                area,
                &BchnScreen {
                    status: &status,
                    expected: None,
                    tab: 0,
                    now: NOW,
                },
            );
        });
        assert!(
            text.contains("Failed to fetch BCHN chain info: connection refused"),
            "{text}"
        );
    }

    #[test]
    fn test_peers_tab() {
        let status = Remote::Ready(sample_bchn_status());
        let text = screen_text(100, 10, |frame| {
            let area = frame.area();
            super::render(
                frame,
                area,
                &BchnScreen {
                    status: &status,
                    expected: Some(CashNetwork::Chipnet),
                    tab: 1,
                    now: NOW,
                },
            );
        });
        insta::assert_snapshot!(text);
    }

    #[test]
    fn test_peers_tab_without_a_config() {
        let status: Remote<BchnStatus> = Remote::Unconfigured;
        let text = screen_text(100, 10, |frame| {
            let area = frame.area();
            super::render(
                frame,
                area,
                &BchnScreen {
                    status: &status,
                    expected: None,
                    tab: 1,
                    now: NOW,
                },
            );
        });
        assert!(text.contains("Pick the node's config.toml"), "{text}");
    }

    #[test]
    fn test_mempool_tab() {
        let status = Remote::Ready(sample_bchn_status());
        let text = screen_text(80, 14, |frame| {
            let area = frame.area();
            super::render(
                frame,
                area,
                &BchnScreen {
                    status: &status,
                    expected: Some(CashNetwork::Chipnet),
                    tab: 2,
                    now: NOW,
                },
            );
        });
        insta::assert_snapshot!(text);
    }
}
