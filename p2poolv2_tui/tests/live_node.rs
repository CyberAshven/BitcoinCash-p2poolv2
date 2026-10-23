// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Checks against a running node and its BCHN, skipped unless asked for:
//!
//! ```sh
//! P2POOL_TUI_LIVE_CONFIG=/path/to/config.toml \
//!     cargo nextest run -p p2poolv2_tui --run-ignored only
//! ```
//!
//! It only reads: GET requests to the API, read-only BCHN calls and the
//! `/ws` feed. It prints what each screen would show, never a login.

#![cfg(feature = "client")]

use p2poolv2_tui::client::api::ApiClient;
use p2poolv2_tui::client::bchn::BchnClient;
use p2poolv2_tui::client::http::display_url;
use p2poolv2_tui::client::live::{self, LiveMessage};
use p2poolv2_tui::network::CashNetwork;
use p2poolv2_tui::node_config;
use std::env;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::timeout;

/// The variable naming the node config to check against.
const CONFIG_VARIABLE: &str = "P2POOL_TUI_LIVE_CONFIG";

#[tokio::test]
#[ignore = "needs a running node and BCHN: set P2POOL_TUI_LIVE_CONFIG"]
async fn test_a_running_node_and_bchn_answer_every_screen() {
    let path = PathBuf::from(env::var_os(CONFIG_VARIABLE).expect("set P2POOL_TUI_LIVE_CONFIG"));
    let connection = node_config::load(&path).unwrap();
    println!("network in the node config: {}", connection.network.name());

    let bchn = BchnClient::new(
        &connection.bchn_url,
        &connection.bchn_username,
        &connection.bchn_password,
    )
    .unwrap();
    let status = bchn.status().await.unwrap();
    println!(
        "BCHN {}: {} on {}, block {}, {} peers, mempool {} transactions",
        display_url(&connection.bchn_url),
        status.network.subversion,
        status.chain.chain,
        status.chain.blocks,
        status.peers.len(),
        status.mempool.size
    );
    assert_eq!(
        CashNetwork::from_bchn_chain(&status.chain.chain),
        Some(connection.network)
    );

    let api = ApiClient::new(&connection.api_base, connection.api_credentials.clone()).unwrap();
    let site = api.network().await.unwrap();
    println!("API {}: {}", api.base(), site.network);
    assert_eq!(
        CashNetwork::from_badge(&site.badge),
        Some(connection.network)
    );

    let chain_info = api.chain_info().await.unwrap();
    println!(
        "share chain tip {:?}, candidate {:?}",
        chain_info.chain_tip_height, chain_info.top_candidate_height
    );
    let hashrate = api.hashrate().await.unwrap();
    println!(
        "hashrate pool {:?} local {:?} network {:?}, {} samples",
        hashrate.pool_hashrate,
        hashrate.local_hashrate,
        hashrate.network_hashrate,
        hashrate.history.len()
    );
    let shares = api.shares().await.unwrap();
    println!("recent shares: {}", shares.shares.len());
    let miners = api.miners().await.unwrap();
    println!(
        "miners online {}, workers online {}, listed {}",
        miners.miners_online,
        miners.workers_online,
        miners.miners.len()
    );
    if let Some(local) = miners.miners.first() {
        let miner = api.miner(&local.address).await.unwrap();
        println!(
            "miner {}: {} of {} window shares, payout {:.4}",
            miner.address, miner.share_count, miner.window.share_count, miner.payout_fraction
        );
    }
    let found_blocks = api.found_blocks().await.unwrap();
    println!("found blocks: {}", found_blocks.len());
    match api.peers().await {
        Ok(peers) => println!("share chain peers: {}", peers.len()),
        Err(error) => println!("share chain peers not readable: {error}"),
    }

    let (sender, mut receiver) = mpsc::channel(16);
    let feed = tokio::spawn(live::run(api.websocket_url(), sender));
    match timeout(Duration::from_secs(3), receiver.recv()).await {
        Ok(Some(LiveMessage::Ended(reason))) => panic!("the live feed ended: {reason}"),
        Ok(Some(message)) => println!("live feed: {message:?}"),
        Ok(None) => panic!("the live feed stopped without a reason"),
        Err(_) => println!("live feed open, nothing within 3 s"),
    }
    feed.abort();
}
