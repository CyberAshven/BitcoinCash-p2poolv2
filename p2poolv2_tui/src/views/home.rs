// SPDX-FileCopyrightText: 2024 PDM Authors
// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The home screen: what this tool is pointed at and whether both ends
//! answer. PDM's home view only greeted.

use crate::format;
use crate::model::{BchnStatus, Site};
use crate::network::{self, CashNetwork};
use crate::remote::Remote;
use crate::views::{alert, field, muted, titled};
use ratatui::{
    prelude::*,
    widgets::{Paragraph, Wrap},
};

/// What the home screen shows.
pub struct HomeScreen<'a> {
    /// The node config file, as a path to show.
    pub node_config: Option<&'a str>,
    /// Why the node config could not be used.
    pub config_error: Option<&'a str>,
    /// The network the node config names.
    pub network: Option<CashNetwork>,
    /// The node's API, without credentials.
    pub api_url: Option<&'a str>,
    /// BCHN's JSON-RPC, without credentials.
    pub bchn_url: Option<&'a str>,
    pub site: &'a Remote<Site>,
    pub bchn: &'a Remote<BchnStatus>,
}

pub fn render(frame: &mut Frame, area: Rect, screen: &HomeScreen) {
    let mut lines = vec![
        Line::from("A terminal UI for a Bitcoin Cash P2Poolv2 node and the BCHN it mines on."),
        Line::from(""),
    ];
    match (screen.node_config, screen.config_error) {
        (_, Some(error)) => {
            lines.push(field("Node config", screen.node_config.unwrap_or("-")));
            lines.push(alert(error.to_string()));
        }
        (Some(path), None) => {
            lines.push(field("Node config", path));
            lines.push(field(
                "Network",
                screen.network.map_or(format::UNKNOWN, CashNetwork::name),
            ));
            lines.push(api_line(screen));
            lines.push(bchn_line(screen));
            if let Some(api_url) = screen.api_url {
                lines.push(field("Dashboard", format!("{api_url}/dashboard")));
            }
        }
        (None, None) => {
            lines.push(field("Node config", "not set"));
            lines.push(muted(
                "Pick the node's config.toml in P2Pool Config or Settings. The API and BCHN \
                 are read from it; its network (mainnet or chipnet) is the only switch.",
            ));
        }
    }
    lines.push(Line::from(""));
    lines.push(muted(
        "Up/Down picks a screen, Left/Right a tab, r refreshes, q quits.",
    ));

    let paragraph = Paragraph::new(lines)
        .block(titled(" Home "))
        .wrap(Wrap { trim: true });
    frame.render_widget(paragraph, area);
}

fn api_line(screen: &HomeScreen) -> Line<'static> {
    let url = screen.api_url.unwrap_or(format::UNKNOWN);
    match screen.site {
        Remote::Ready(site) => {
            let found = CashNetwork::from_badge(&site.badge);
            match network::mismatch(screen.network, "P2Pool API", found, &site.badge) {
                Some(warning) => alert(format!("P2Pool API {url}: {warning}")),
                None => field("P2Pool API", format!("{url} ({})", site.network)),
            }
        }
        Remote::Failed(error) => alert(format!("P2Pool API {url}: {error}")),
        Remote::Loading | Remote::Unconfigured => field("P2Pool API", format!("{url} (asking)")),
    }
}

fn bchn_line(screen: &HomeScreen) -> Line<'static> {
    let url = screen.bchn_url.unwrap_or(format::UNKNOWN);
    match screen.bchn {
        Remote::Ready(status) => {
            let chain = &status.chain.chain;
            let found = CashNetwork::from_bchn_chain(chain);
            match network::mismatch(screen.network, "BCHN", found, chain) {
                Some(warning) => alert(format!("BCHN {url}: {warning}")),
                None => field(
                    "BCHN",
                    format!(
                        "{url} (block {}, {})",
                        format::with_commas(status.chain.blocks),
                        sync_state(status)
                    ),
                ),
            }
        }
        Remote::Failed(error) => alert(format!("BCHN {url}: {error}")),
        Remote::Loading | Remote::Unconfigured => field("BCHN", format!("{url} (asking)")),
    }
}

/// `synced`, or how far behind BCHN still is.
pub(crate) fn sync_state(status: &BchnStatus) -> String {
    let chain = &status.chain;
    if chain.initial_block_download == Some(true) || chain.headers > chain.blocks {
        format!(
            "syncing, {} blocks behind",
            format::with_commas(chain.headers.saturating_sub(chain.blocks))
        )
    } else {
        "synced".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{sample_bchn_status, sample_site, screen_text};

    #[test]
    fn test_home_without_a_node_config() {
        let text = screen_text(80, 12, |frame| {
            let area = frame.area();
            super::render(
                frame,
                area,
                &HomeScreen {
                    node_config: None,
                    config_error: None,
                    network: None,
                    api_url: None,
                    bchn_url: None,
                    site: &Remote::Unconfigured,
                    bchn: &Remote::Unconfigured,
                },
            );
        });
        insta::assert_snapshot!(text);
    }

    #[test]
    fn test_home_with_both_ends_answering() {
        let site = Remote::Ready(sample_site());
        let bchn = Remote::Ready(sample_bchn_status());
        let text = screen_text(100, 12, |frame| {
            let area = frame.area();
            super::render(
                frame,
                area,
                &HomeScreen {
                    node_config: Some("D:/pool/node1/config.toml"),
                    config_error: None,
                    network: Some(CashNetwork::Chipnet),
                    api_url: Some("http://127.0.0.1:46884"),
                    bchn_url: Some("http://127.0.0.1:48332"),
                    site: &site,
                    bchn: &bchn,
                },
            );
        });
        insta::assert_snapshot!(text);
    }

    #[test]
    fn test_home_warns_when_bchn_is_on_another_network() {
        let bchn = Remote::Ready(sample_bchn_status());
        let text = screen_text(110, 12, |frame| {
            let area = frame.area();
            super::render(
                frame,
                area,
                &HomeScreen {
                    node_config: Some("config.toml"),
                    config_error: None,
                    network: Some(CashNetwork::Mainnet),
                    api_url: Some("http://127.0.0.1:46884"),
                    bchn_url: Some("http://127.0.0.1:8332"),
                    site: &Remote::Loading,
                    bchn: &bchn,
                },
            );
        });
        assert!(
            text.contains("BCHN is on chipnet but the node config says mainnet"),
            "{text}"
        );
    }

    #[test]
    fn test_home_shows_a_config_error() {
        let text = screen_text(80, 12, |frame| {
            let area = frame.area();
            super::render(
                frame,
                area,
                &HomeScreen {
                    node_config: Some("config.toml"),
                    config_error: Some("stratum.chain is not bitcoin-cash"),
                    network: None,
                    api_url: None,
                    bchn_url: None,
                    site: &Remote::Unconfigured,
                    bchn: &Remote::Unconfigured,
                },
            );
        });
        assert!(text.contains("stratum.chain is not bitcoin-cash"), "{text}");
    }

    #[test]
    fn test_sync_state_counts_blocks_behind() {
        let mut status = sample_bchn_status();
        status.chain.headers = status.chain.blocks + 1_500;
        assert_eq!(sync_state(&status), "syncing, 1,500 blocks behind");
    }
}
