// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Help, as `/dashboard/help` shows it, with this app's settings added.

use crate::app::ctx;
use crate::layout::{Crumb, dashboard_crumb};
use crate::route::Tab;
use leptos::prelude::*;

#[component]
pub fn HelpPage() -> impl IntoView {
    ctx().frame.set(
        "Help",
        Tab::Help,
        vec![dashboard_crumb(), Crumb::here("Help")],
    );
    view! {
        <div class="panel">
            <div class="help-section">
                <h3>"Connect a miner"</h3>
                <p>"Stratum V1: point the miner at this node's stratum port."</p>
                <ul>
                    <li>"URL: " <code>"stratum+tcp://<this node>:<stratum port>"</code></li>
                    <li>"User: " <code>"<payout address>.<worker name>"</code></li>
                    <li>
                        "Password: " <code>"p2p=<share chain address>"</code>
                        ", which names who owns your shares."
                    </li>
                </ul>
                <p>
                    "Stratum V2: connect to the node's SV2 port and pin its authority key. Use "
                    <code>"<payout address>.<worker>,p2p=<share chain address>"</code>
                    " as the user identity. "
                    "SV1 devices can mine through an SV2 translator."
                </p>
                <p>
                    "Get a share chain address with " <code>"p2poolv2_cli bch-share-address"</code>
                    "."
                </p>
            </div>
            <div class="help-section">
                <h3>"Pages"</h3>
                <ul>
                    <li>
                        <a href="#/">"Dashboard"</a>
                        ": hashrate, found blocks and the newest shares."
                    </li>
                    <li><a href="#/blocks">"Blocks"</a> ": every block the pool found."</li>
                    <li>
                        <a href="#/miners">"Miners"</a>
                        ": the miners on this node and their workers."
                    </li>
                    <li>
                        "Search: a payout or share chain address opens the miner, a share hash opens the share."
                    </li>
                    <li>
                        <a href="#/settings">"Settings"</a>
                        ": the node this app reads, and its API login."
                    </li>
                </ul>
            </div>
            <div class="help-section">
                <h3>"API endpoints"</h3>
                <p>"The same data as JSON, for explorers and your own frontend:"</p>
                <ul>
                    <li>
                        <code>"/chain_info"</code> ", " <code>"/shares"</code> ", "
                        <code>"/share?hash="</code> ", " <code>"/share_headers"</code>
                    </li>
                    <li>
                        <code>"/miner?address="</code>
                        ": a miner's shares and its part of the next payout"
                    </li>
                    <li>
                        <code>"/pplns_shares"</code> ", " <code>"/candidates"</code> ", "
                        <code>"/dag"</code> ", " <code>"/transaction"</code>
                    </li>
                    <li>
                        <code>"/hashrate"</code> ", " <code>"/found_blocks"</code> ", "
                        <code>"/miners"</code> ", " <code>"/network"</code>
                    </li>
                    <li>
                        <code>"/metrics"</code> " (Prometheus) and " <code>"/peers"</code>
                        ": always behind the API login"
                    </li>
                </ul>
            </div>
            <div class="help-section">
                <h3>"About this dashboard"</h3>
                <p>
                    "This dashboard is Rust compiled to WebAssembly. It reads the node's JSON API, "
                    "takes new shares live from its /ws feed, and lets you zoom the hashrate chart. "
                    "It uses the stylesheets of the pages the node renders itself, so both look the same. "
                    "The design is CashDragon0's."
                </p>
            </div>
        </div>
    }
}
