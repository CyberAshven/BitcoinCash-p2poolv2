// SPDX-FileCopyrightText: 2024 PDM Authors
// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The node's status, ported from PDM's P2Pool status view and extended to
//! the fork's API.
//!
//! PDM's chain info, shares and peers tabs read `/chain_info`, `/shares`,
//! `/peers` and `/ws`. The fork adds an overview with `/network` and
//! `/hashrate`, the miners on this node from `/miners` with one miner's
//! place in the PPLNS window from `/miner`, and the blocks the pool found
//! from `/found_blocks`.

use crate::format;
use crate::model::{
    ChainInfo, FoundBlock, Hashrate, LocalMiner, Miner, Miners, Peer, PeerEvent, PeerStatus, Share,
    Shares, Site,
};
use crate::network::{self, CashNetwork};
use crate::remote::Remote;
use crate::views::{
    PICK_A_CONFIG, alert, field, header_row, muted, remote_lines, remote_message, render_message,
    tab_bar, tabs_and_content, titled,
};
use ratatui::{
    prelude::*,
    widgets::{Cell, Paragraph, Row, Sparkline, Table, TableState, Wrap},
};
use std::collections::{HashSet, VecDeque};

/// The tabs, in order.
pub const POOL_TABS: [&str; 5] = ["Overview", "Shares", "Miners", "Blocks", "Peers"];

/// Most rows in the shares table, as in PDM.
pub const MAX_SHARE_ROWS: usize = 50;

/// Live peer events listed under the peers.
const LIVE_PEER_EVENTS_SHOWN: usize = 8;

/// Rows the peers table keeps when the screen is short: its borders, the
/// header and two peers.
const PEER_TABLE_MIN_HEIGHT: u16 = 6;

/// What the P2Pool status screen shows.
pub struct PoolScreen<'a> {
    /// Index into [`POOL_TABS`].
    pub tab: usize,
    /// The network the node config names, to check the API against.
    pub expected: Option<CashNetwork>,
    /// The node's API, without credentials.
    pub api_url: Option<&'a str>,
    /// Unix seconds, for ages.
    pub now: u64,
    pub site: &'a Remote<Site>,
    pub chain_info: &'a Remote<ChainInfo>,
    pub hashrate: &'a Remote<Hashrate>,
    pub shares: &'a Remote<Shares>,
    /// Shares from `/ws`, oldest first.
    pub live_shares: &'a VecDeque<Share>,
    pub miners: &'a Remote<Miners>,
    /// The miners table has the keyboard.
    pub miners_focused: bool,
    pub miners_selected: usize,
    /// The miner picked in the table, once asked for.
    pub miner: &'a Remote<Miner>,
    pub found_blocks: &'a Remote<Vec<FoundBlock>>,
    pub peers: &'a Remote<Vec<Peer>>,
    /// Peer events from `/ws`, oldest first.
    pub live_peer_events: &'a VecDeque<PeerEvent>,
    /// Why the live feed is down, when it is.
    pub live_error: Option<&'a str>,
}

pub fn render(frame: &mut Frame, area: Rect, screen: &PoolScreen) {
    let (tabs_area, content_area) = tabs_and_content(area);
    frame.render_widget(tab_bar(&POOL_TABS, screen.tab), tabs_area);
    match screen.tab {
        0 => render_overview(frame, content_area, screen),
        1 => render_shares(frame, content_area, screen),
        2 => render_miners(frame, content_area, screen),
        3 => render_blocks(frame, content_area, screen),
        _ => render_peers(frame, content_area, screen),
    }
}

fn render_overview(frame: &mut Frame, area: Rect, screen: &PoolScreen) {
    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(6)])
        .split(area);

    let mut lines = remote_lines(screen.site, "the network", PICK_A_CONFIG, |site| {
        let found = CashNetwork::from_badge(&site.badge);
        let mut lines = Vec::with_capacity(2);
        if let Some(warning) = network::mismatch(screen.expected, "P2Pool API", found, &site.badge)
        {
            lines.push(alert(warning));
        }
        lines.push(field("Network", site.network.clone()));
        lines
    });
    lines.extend(remote_lines(
        screen.chain_info,
        "share chain info",
        "",
        chain_info_lines,
    ));
    lines.extend(remote_lines(screen.hashrate, "hashrate", "", |hashrate| {
        vec![
            field(
                "Pool hashrate",
                format::optional(hashrate.pool_hashrate, format::hashrate),
            ),
            field(
                "This node's hashrate",
                format::optional(hashrate.local_hashrate, format::hashrate),
            ),
            field(
                "Network hashrate",
                format::optional(hashrate.network_hashrate, format::hashrate),
            ),
        ]
    }));
    if let Some(api_url) = screen.api_url {
        lines.push(field("Dashboard", format!("{api_url}/dashboard")));
    }
    lines.push(match screen.live_error {
        Some(error) => alert(format!("Live feed down: {error}")),
        None => field("Live feed", "/ws shares and peers"),
    });
    let paragraph = Paragraph::new(lines)
        .block(titled(" Overview "))
        .wrap(Wrap { trim: true });
    frame.render_widget(paragraph, parts[0]);
    render_hashrate_history(frame, parts[1], screen.hashrate);
}

fn chain_info_lines(info: &ChainInfo) -> Vec<Line<'static>> {
    vec![
        field(
            "Share chain tip",
            format!(
                "{} {}",
                format::optional(info.chain_tip_height, |height| height.to_string()),
                info.chain_tip_blockhash
                    .as_deref()
                    .map_or(String::new(), |hash| format::short(hash, 20))
            ),
        ),
        field(
            "Candidate tip",
            format::optional(info.top_candidate_height, |height| height.to_string()),
        ),
        field("Total work", info.total_work.clone()),
        field(
            "Genesis share",
            info.genesis_blockhash
                .as_deref()
                .map_or(format::UNKNOWN.to_string(), |hash| format::short(hash, 20)),
        ),
    ]
}

/// The pool hashrate over the last hour, newest on the right.
fn render_hashrate_history(frame: &mut Frame, area: Rect, hashrate: &Remote<Hashrate>) {
    let block = titled(" Pool hashrate, last hour ");
    let shown = usize::from(block.inner(area).width);
    let samples: Vec<u64> = hashrate.ready().map_or_else(Vec::new, |hashrate| {
        let skip = hashrate.history.len().saturating_sub(shown);
        hashrate
            .history
            .iter()
            .skip(skip)
            .map(|point| point.pool_hashrate.unwrap_or(0.0).max(0.0) as u64)
            .collect()
    });
    let sparkline = Sparkline::default()
        .block(block)
        .data(&samples)
        .style(Style::default().fg(Color::Green));
    frame.render_widget(sparkline, area);
}

/// Shares from `/shares` and `/ws` without repeats, newest first, at most
/// [`MAX_SHARE_ROWS`]: PDM's merge.
pub fn merged_shares<'a>(recent: &'a Remote<Shares>, live: &'a VecDeque<Share>) -> Vec<&'a Share> {
    let recent = recent.ready().map_or(&[][..], |shares| &shares.shares[..]);
    let mut seen = HashSet::with_capacity(recent.len() + live.len());
    let mut merged: Vec<&Share> = live
        .iter()
        .rev()
        .chain(recent.iter().rev())
        .filter(|share| seen.insert(share.blockhash.as_str()))
        .collect();
    merged.sort_by(|left, right| {
        right
            .height
            .cmp(&left.height)
            .then_with(|| right.timestamp.cmp(&left.timestamp))
    });
    merged.truncate(MAX_SHARE_ROWS);
    merged
}

fn render_shares(frame: &mut Frame, area: Rect, screen: &PoolScreen) {
    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(area);
    let shares = merged_shares(screen.shares, screen.live_shares);
    if shares.is_empty() {
        let message = remote_message(
            screen.shares,
            "recent shares",
            PICK_A_CONFIG,
            "Waiting for shares...",
        );
        render_message(frame, parts[0], " Shares ", message);
    } else {
        let rows: Vec<Row> = shares
            .iter()
            .map(|share| share_row(share, screen.now))
            .collect();
        let widths = [
            Constraint::Length(7),
            Constraint::Length(12),
            Constraint::Length(26),
            Constraint::Length(10),
            Constraint::Length(12),
            Constraint::Length(6),
        ];
        let table = Table::new(rows, widths)
            .block(titled(" Shares "))
            .header(header_row(&[
                "Height",
                "Share",
                "Payout address",
                "Difficulty",
                "Age",
                "Uncles",
            ]))
            .column_spacing(1);
        frame.render_widget(table, parts[0]);
    }
    let live = match screen.live_error {
        Some(error) => alert(format!("Live feed down: {error}")),
        None => muted("New shares arrive live from /ws."),
    };
    frame.render_widget(Paragraph::new(live), parts[1]);
}

fn share_row(share: &Share, now: u64) -> Row<'static> {
    Row::new([
        Cell::from(share.height.to_string()),
        Cell::from(format::short(&share.blockhash, 12)),
        Cell::from(format::short(&share.miner_bitcoin_address, 26)),
        Cell::from(format::optional(
            format::difficulty_from_bits(share.bits),
            format::difficulty,
        )),
        Cell::from(format::ago(now, share.timestamp)),
        Cell::from(share.uncles.len().to_string()),
    ])
}

fn render_miners(frame: &mut Frame, area: Rect, screen: &PoolScreen) {
    let detail_height = match screen.miner {
        Remote::Unconfigured => 0,
        Remote::Loading | Remote::Failed(_) | Remote::Ready(_) => 9,
    };
    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(detail_height)])
        .split(area);

    match screen.miners.ready() {
        Some(miners) if !miners.miners.is_empty() => {
            render_miners_table(frame, parts[0], screen, miners);
        }
        _ => {
            let message = remote_message(
                screen.miners,
                "miners",
                PICK_A_CONFIG,
                "No miner has sent a share to this node yet.",
            );
            render_message(frame, parts[0], " Miners on this node ", message);
        }
    }

    if detail_height > 0 {
        let lines = remote_lines(screen.miner, "the miner", "", miner_lines);
        let paragraph = Paragraph::new(lines)
            .block(titled(" Miner in the PPLNS window "))
            .wrap(Wrap { trim: true });
        frame.render_widget(paragraph, parts[1]);
    }
}

fn render_miners_table(frame: &mut Frame, area: Rect, screen: &PoolScreen, miners: &Miners) {
    let rows: Vec<Row> = miners
        .miners
        .iter()
        .map(|miner| miner_row(miner, screen.now))
        .collect();
    let title = format!(
        " Miners on this node: {} online, {} workers (page {} of {}) ",
        miners.miners_online, miners.workers_online, miners.page, miners.pages
    );
    let widths = [
        Constraint::Length(30),
        Constraint::Length(8),
        Constraint::Length(9),
        Constraint::Length(9),
        Constraint::Length(13),
        Constraint::Length(8),
    ];
    let border_style = if screen.miners_focused {
        Style::default()
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let table = Table::new(rows, widths)
        .block(titled(&title).border_style(border_style))
        .header(header_row(&[
            "Payout address",
            "Workers",
            "Work",
            "Best",
            "Last share",
            "Payout",
        ]))
        .column_spacing(1)
        .row_highlight_style(Style::default().bg(Color::DarkGray))
        .highlight_symbol("> ");
    let mut state = TableState::default();
    if screen.miners_focused {
        state.select(Some(screen.miners_selected));
    }
    frame.render_stateful_widget(table, area, &mut state);
}

fn miner_row(miner: &LocalMiner, now: u64) -> Row<'static> {
    let active = miner.workers.iter().filter(|worker| worker.active).count();
    Row::new([
        Cell::from(format::short(&miner.address, 30)),
        Cell::from(format!("{active}/{}", miner.workers.len())),
        Cell::from(format::difficulty(miner.accepted_work as f64)),
        Cell::from(format::difficulty(miner.best_share as f64)),
        Cell::from(format::ago(now, miner.last_share_at)),
        Cell::from(format::optional(miner.payout_fraction, format::percent)),
    ])
}

fn miner_lines(miner: &Miner) -> Vec<Line<'static>> {
    vec![
        field(
            "Address",
            format!("{} ({})", miner.address, miner.address_type),
        ),
        field(
            "Window shares",
            format!(
                "{} of {}",
                format::with_commas(miner.share_count as u64),
                format::with_commas(miner.window.share_count as u64)
            ),
        ),
        field("Uncles credited", miner.uncle_count.to_string()),
        field("Share of the window", format::percent(miner.share_fraction)),
        field("Next block payout", format::percent(miner.payout_fraction)),
        field(
            "Window heights",
            format!(
                "{} to {} (anchor {})",
                format::optional(miner.window.oldest_height, |height| height.to_string()),
                format::optional(miner.window.newest_height, |height| height.to_string()),
                miner.anchor_height
            ),
        ),
    ]
}

fn render_blocks(frame: &mut Frame, area: Rect, screen: &PoolScreen) {
    let blocks = match screen.found_blocks.ready() {
        Some(blocks) if !blocks.is_empty() => blocks,
        _ => {
            let message = remote_message(
                screen.found_blocks,
                "found blocks",
                PICK_A_CONFIG,
                "The pool has not found a block yet.",
            );
            render_message(frame, area, " Blocks found by the pool ", message);
            return;
        }
    };
    let rows: Vec<Row> = blocks
        .iter()
        .map(|block| block_row(block, screen.now))
        .collect();
    let title = format!(" Blocks found by the pool ({}) ", blocks.len());
    let widths = [
        Constraint::Length(9),
        Constraint::Length(22),
        Constraint::Length(14),
        Constraint::Min(20),
    ];
    let table = Table::new(rows, widths)
        .block(titled(&title))
        .header(header_row(&["Height", "Block", "Found", "Paid to"]))
        .column_spacing(1);
    frame.render_widget(table, area);
}

fn block_row(block: &FoundBlock, now: u64) -> Row<'static> {
    Row::new([
        Cell::from(format::with_commas(block.bitcoin_height)),
        Cell::from(format::short(&block.bitcoin_blockhash, 22)),
        Cell::from(format::ago(now, block.bitcoin_time)),
        Cell::from(block.miner_bitcoin_address.clone()),
    ])
}

fn render_peers(frame: &mut Frame, area: Rect, screen: &PoolScreen) {
    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(PEER_TABLE_MIN_HEIGHT),
            Constraint::Max(LIVE_PEER_EVENTS_SHOWN as u16 + 3),
        ])
        .split(area);
    match screen.peers.ready() {
        Some(peers) if !peers.is_empty() => {
            let rows: Vec<Row> = peers.iter().map(peer_row).collect();
            let widths = [
                Constraint::Length(20),
                Constraint::Length(28),
                Constraint::Length(9),
                Constraint::Min(12),
            ];
            let title = format!(" Share chain peers ({}) ", peers.len());
            let table = Table::new(rows, widths)
                .block(titled(&title))
                .header(header_row(&["Peer", "Address", "Direction", "Connected"]))
                .column_spacing(1);
            frame.render_widget(table, parts[0]);
        }
        _ => {
            let message =
                remote_message(screen.peers, "peers", PICK_A_CONFIG, "No connected peers.");
            render_message(frame, parts[0], " Share chain peers ", message);
        }
    }

    let mut lines: Vec<Line> = Vec::with_capacity(LIVE_PEER_EVENTS_SHOWN + 1);
    if let Some(error) = screen.live_error {
        lines.push(alert(format!("Live feed down: {error}")));
    }
    if screen.live_peer_events.is_empty() {
        lines.push(muted("No peer has connected or left since this opened."));
    }
    lines.extend(
        screen
            .live_peer_events
            .iter()
            .rev()
            .take(LIVE_PEER_EVENTS_SHOWN)
            .map(|event| {
                let status = match event.status {
                    PeerStatus::Connected => "connected",
                    PeerStatus::Disconnected => "disconnected",
                };
                Line::from(format!("{status}: {}", format::short(&event.peer_id, 42)))
            }),
    );
    frame.render_widget(
        Paragraph::new(lines).block(titled(" Live peer events ")),
        parts[1],
    );
}

fn peer_row(peer: &Peer) -> Row<'static> {
    Row::new([
        Cell::from(format::short(&peer.peer_id, 20)),
        Cell::from(format::short(
            peer.address.as_deref().unwrap_or(format::UNKNOWN),
            28,
        )),
        Cell::from(
            peer.direction
                .clone()
                .unwrap_or_else(|| format::UNKNOWN.to_string()),
        ),
        Cell::from(format::optional(peer.connected_secs, format::duration)),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{
        NOW, sample_chain_info, sample_found_block, sample_hashrate, sample_miner, sample_miners,
        sample_peer, sample_share, sample_site, screen_text,
    };

    #[test]
    fn test_merged_shares_drops_repeats_and_sorts_newest_first() {
        let recent = Remote::Ready(Shares {
            shares: vec![sample_share(10), sample_share(11)],
        });
        let live = VecDeque::from([sample_share(11), sample_share(12)]);
        let merged = merged_shares(&recent, &live);
        let heights: Vec<u32> = merged.iter().map(|share| share.height).collect();
        assert_eq!(heights, vec![12, 11, 10]);
    }

    #[test]
    fn test_merged_shares_keeps_at_most_a_page() {
        let live: VecDeque<Share> = (0..80).map(sample_share).collect();
        let merged = merged_shares(&Remote::Unconfigured, &live);
        assert_eq!(merged.len(), MAX_SHARE_ROWS);
        assert_eq!(merged[0].height, 79);
    }

    #[test]
    fn test_overview_tab() {
        let site = Remote::Ready(sample_site());
        let chain_info = Remote::Ready(sample_chain_info());
        let hashrate = Remote::Ready(sample_hashrate());
        let text = screen_text(90, 24, |frame| {
            let area = frame.area();
            super::render(
                frame,
                area,
                &PoolScreen {
                    tab: 0,
                    expected: Some(CashNetwork::Chipnet),
                    api_url: Some("http://127.0.0.1:46884"),
                    now: NOW,
                    site: &site,
                    chain_info: &chain_info,
                    hashrate: &hashrate,
                    shares: &Remote::Unconfigured,
                    live_shares: &VecDeque::new(),
                    miners: &Remote::Unconfigured,
                    miners_focused: false,
                    miners_selected: 0,
                    miner: &Remote::Unconfigured,
                    found_blocks: &Remote::Unconfigured,
                    peers: &Remote::Unconfigured,
                    live_peer_events: &VecDeque::new(),
                    live_error: None,
                },
            );
        });
        insta::assert_snapshot!(text);
    }

    #[test]
    fn test_overview_warns_when_the_api_is_on_another_network() {
        let site = Remote::Ready(sample_site());
        let text = screen_text(90, 24, |frame| {
            let area = frame.area();
            super::render(
                frame,
                area,
                &PoolScreen {
                    tab: 0,
                    expected: Some(CashNetwork::Mainnet),
                    api_url: None,
                    now: NOW,
                    site: &site,
                    chain_info: &Remote::Loading,
                    hashrate: &Remote::Loading,
                    shares: &Remote::Unconfigured,
                    live_shares: &VecDeque::new(),
                    miners: &Remote::Unconfigured,
                    miners_focused: false,
                    miners_selected: 0,
                    miner: &Remote::Unconfigured,
                    found_blocks: &Remote::Unconfigured,
                    peers: &Remote::Unconfigured,
                    live_peer_events: &VecDeque::new(),
                    live_error: Some("connection refused"),
                },
            );
        });
        assert!(
            text.contains("P2Pool API is on chipnet but the node config says mainnet"),
            "{text}"
        );
        assert!(
            text.contains("Live feed down: connection refused"),
            "{text}"
        );
    }

    #[test]
    fn test_shares_tab() {
        let shares = Remote::Ready(Shares {
            shares: vec![sample_share(1_203), sample_share(1_204)],
        });
        let text = screen_text(90, 10, |frame| {
            let area = frame.area();
            super::render(
                frame,
                area,
                &PoolScreen {
                    tab: 1,
                    expected: Some(CashNetwork::Chipnet),
                    api_url: None,
                    now: NOW,
                    site: &Remote::Unconfigured,
                    chain_info: &Remote::Unconfigured,
                    hashrate: &Remote::Unconfigured,
                    shares: &shares,
                    live_shares: &VecDeque::from([sample_share(1_205)]),
                    miners: &Remote::Unconfigured,
                    miners_focused: false,
                    miners_selected: 0,
                    miner: &Remote::Unconfigured,
                    found_blocks: &Remote::Unconfigured,
                    peers: &Remote::Unconfigured,
                    live_peer_events: &VecDeque::new(),
                    live_error: None,
                },
            );
        });
        insta::assert_snapshot!(text);
    }

    #[test]
    fn test_miners_tab_with_a_miner_picked() {
        let miners = Remote::Ready(sample_miners());
        let miner = Remote::Ready(sample_miner());
        let text = screen_text(100, 20, |frame| {
            let area = frame.area();
            super::render(
                frame,
                area,
                &PoolScreen {
                    tab: 2,
                    expected: Some(CashNetwork::Chipnet),
                    api_url: None,
                    now: NOW,
                    site: &Remote::Unconfigured,
                    chain_info: &Remote::Unconfigured,
                    hashrate: &Remote::Unconfigured,
                    shares: &Remote::Unconfigured,
                    live_shares: &VecDeque::new(),
                    miners: &miners,
                    miners_focused: true,
                    miners_selected: 0,
                    miner: &miner,
                    found_blocks: &Remote::Unconfigured,
                    peers: &Remote::Unconfigured,
                    live_peer_events: &VecDeque::new(),
                    live_error: None,
                },
            );
        });
        insta::assert_snapshot!(text);
    }

    #[test]
    fn test_blocks_tab() {
        let found_blocks = Remote::Ready(vec![sample_found_block()]);
        let text = screen_text(100, 8, |frame| {
            let area = frame.area();
            super::render(
                frame,
                area,
                &PoolScreen {
                    tab: 3,
                    expected: Some(CashNetwork::Chipnet),
                    api_url: None,
                    now: NOW,
                    site: &Remote::Unconfigured,
                    chain_info: &Remote::Unconfigured,
                    hashrate: &Remote::Unconfigured,
                    shares: &Remote::Unconfigured,
                    live_shares: &VecDeque::new(),
                    miners: &Remote::Unconfigured,
                    miners_focused: false,
                    miners_selected: 0,
                    miner: &Remote::Unconfigured,
                    found_blocks: &found_blocks,
                    peers: &Remote::Unconfigured,
                    live_peer_events: &VecDeque::new(),
                    live_error: None,
                },
            );
        });
        insta::assert_snapshot!(text);
    }

    #[test]
    fn test_blocks_tab_before_any_block() {
        let found_blocks = Remote::Ready(Vec::new());
        let text = screen_text(100, 8, |frame| {
            let area = frame.area();
            super::render(
                frame,
                area,
                &PoolScreen {
                    tab: 3,
                    expected: None,
                    api_url: None,
                    now: NOW,
                    site: &Remote::Unconfigured,
                    chain_info: &Remote::Unconfigured,
                    hashrate: &Remote::Unconfigured,
                    shares: &Remote::Unconfigured,
                    live_shares: &VecDeque::new(),
                    miners: &Remote::Unconfigured,
                    miners_focused: false,
                    miners_selected: 0,
                    miner: &Remote::Unconfigured,
                    found_blocks: &found_blocks,
                    peers: &Remote::Unconfigured,
                    live_peer_events: &VecDeque::new(),
                    live_error: None,
                },
            );
        });
        assert!(
            text.contains("The pool has not found a block yet."),
            "{text}"
        );
    }

    #[test]
    fn test_peers_tab_with_live_events() {
        let peers = Remote::Ready(vec![sample_peer()]);
        let events = VecDeque::from([PeerEvent {
            peer_id: "12D3KooWnew".into(),
            status: PeerStatus::Connected,
        }]);
        let text = screen_text(90, 20, |frame| {
            let area = frame.area();
            super::render(
                frame,
                area,
                &PoolScreen {
                    tab: 4,
                    expected: Some(CashNetwork::Chipnet),
                    api_url: None,
                    now: NOW,
                    site: &Remote::Unconfigured,
                    chain_info: &Remote::Unconfigured,
                    hashrate: &Remote::Unconfigured,
                    shares: &Remote::Unconfigured,
                    live_shares: &VecDeque::new(),
                    miners: &Remote::Unconfigured,
                    miners_focused: false,
                    miners_selected: 0,
                    miner: &Remote::Unconfigured,
                    found_blocks: &Remote::Unconfigured,
                    peers: &peers,
                    live_peer_events: &events,
                    live_error: None,
                },
            );
        });
        insta::assert_snapshot!(text);
    }
}
