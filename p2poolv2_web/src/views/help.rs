// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Help, as `/dashboard/help` shows it, with this app's settings added.

use crate::api::ShareAddressResponse;
use crate::app::ctx;
use crate::format::url_component;
use crate::layout::{Crumb, dashboard_crumb};
use crate::route::Tab;
use leptos::prelude::*;
use leptos::task::spawn_local;

/// The `GET /share_address` path for `payout`, percent-encoded. The node
/// decodes it back and runs the conversion rule shared with the CLI.
fn share_address_path(payout: &str) -> String {
    format!("/share_address?payout={}", url_component(payout))
}

/// The example fields' payout half: the submitted payout once the node
/// accepted it, the placeholder before that (a refused payout fills
/// nothing).
fn example_payout(payout: &str, share: &str) -> String {
    if share.is_empty() {
        "<payout address>".to_string()
    } else {
        payout.to_string()
    }
}

#[component]
pub fn HelpPage() -> impl IntoView {
    ctx().frame.set(
        "Help",
        Tab::Help,
        vec![dashboard_crumb(), Crumb::here("Help")],
    );
    // The host this page was loaded from: a miner on the same network
    // reaches the node at it. A page with no hostname (`file:`, a sandboxed
    // frame) falls back to the placeholder.
    let host = window()
        .location()
        .hostname()
        .ok()
        .filter(|hostname| !hostname.is_empty())
        .unwrap_or_else(|| "<this node>".to_string());
    let site = ctx().site;
    // The ports arrive with `/network`, so the placeholders stand in until
    // it answers; a node that sends no stratum port (default `0`) gets one
    // too.
    let url = {
        let host = host.clone();
        move || {
            let port = site
                .get()
                .map(|site| site.stratum_port)
                .filter(|port| *port > 0)
                .map_or_else(|| "<stratum port>".to_string(), |port| port.to_string());
            format!("stratum+tcp://{host}:{port}")
        }
    };
    let sv2 = move || {
        let port = site
            .get()
            .and_then(|site| site.sv2_port)
            .map_or_else(|| "<sv2 port>".to_string(), |port| port.to_string());
        format!("{host}:{port}")
    };
    // The `--network` value in the share address example: the CLI's own
    // spelling for this node's network, absent on a node mining the
    // Bitcoin chain and until `/network` answers. Without it there is no
    // generator to show, exactly as the server-rendered page hides it.
    let cli_net = move || site.get().and_then(|site| site.cli_network);
    // The generator's form: the payout as typed, the share address the
    // node returned (empty until one succeeds), and the error it refuses
    // with. An invalid entry goes to the error alone; the example fields
    // below fill only once `share` has a value.
    let payout = RwSignal::new(String::new());
    let share = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    // Builds the form's submit handler. The widget is rebuilt whenever
    // `/network` answers, so the form takes a fresh handler each time
    // rather than one value moved in on the first build only.
    let generate = move || {
        move |event: leptos::ev::SubmitEvent| {
            event.prevent_default();
            let connection = ctx().connection.get_untracked();
            let entered = payout.get_untracked();
            spawn_local(async move {
                let path = share_address_path(&entered);
                match connection.get::<ShareAddressResponse>(&path).await {
                    Ok(response) => {
                        error.set(None);
                        share.set(response.address);
                    }
                    Err(message) => {
                        error.set(Some(message));
                        share.set(String::new());
                    }
                }
            });
        }
    };
    view! {
        <div class="panel">
            <div class="banner-warn" role="status">
                "P2Poolv2 is alpha software, mine at your own risk!"
            </div>
            <div class="help-section">
                <h3>"Connect a miner"</h3>
                {move || {
                    cli_net().map(|network| {
                        // Built fresh each run, so the strings move into
                        // this view while `url` and `sv2` stay in the
                        // outer closure's environment for the next one.
                        let url_text = url();
                        let sv2_text = sv2();
                        let user_text = example_payout(&payout.get(), &share.get());
                        view! {
                            // The generator exists only where the node
                            // has a share address prefix: `cli_network`
                            // arrives with `/network` and is absent on a
                            // Bitcoin node, which mirrors the server
                            // page's own hiding of this widget.
                            <form class="share-form search-row" on:submit={generate()}>
                                <div class="grow">
                                    <label for="share-payout">
                                        "Enter your payout address to get miner connection info."
                                    </label>
                                    <input
                                        id="share-payout"
                                        type="text"
                                        placeholder="bchtest:q... or bitcoincash:q..."
                                        bind:value=payout
                                        required
                                    />
                                </div>
                                <button class="btn-primary" type="submit">"Generate"</button>
                            </form>
                            {move || {
                                error.get().map(|message| {
                                    view! { <div class="toast" role="alert">{message}</div> }
                                })
                            }}
                            <p>"Stratum V1: point the miner at this node's stratum port."</p>
                            <ul>
                                <li>"URL: " <code>{url_text}</code></li>
                                <li>"User: " <code>{user_text.clone()}</code></li>
                                <li>
                                    "Worker Name (optional): " <code>"<payout address>.<worker>"</code>
                                </li>
                                <li>
                                    "Password: " <code>"p2p=" {move || share.get()}</code>
                                    ", which names who owns your shares."
                                </li>
                            </ul>
                            <p>
                                "Stratum V2: point the miner at this node's SV2 port and pin its authority key."
                            </p>
                            <ul>
                                <li>"Host: " <code>{sv2_text}</code></li>
                                <li>
                                    "User: " <code>
                                        {user_text} ",p2p=" {move || share.get()}
                                    </code>
                                </li>
                                <li>
                                    "Worker Name (optional): " <code>"<payout address>.<worker>"</code>
                                </li>
                                <li>
                                    "Key: the node's authority public key (configure when the node runs Noise keys)"
                                </li>
                            </ul>
                            <p>
                                "SV1 devices can connect to the SV1 port directly, or mine through an SV2 translator."
                            </p>
                            <p>
                                "Validate a share chain address with " <code>
                                    "p2poolv2_cli bch-share-address --network " {network} " <payout address>"
                                </code> "."
                            </p>
                        }
                    })
                }}
                {move || {
                    // `/network` has answered and carries no share network:
                    // a Bitcoin node explains itself instead of showing a
                    // generator that could not exist. While it is still
                    // loading, nothing shows rather than a wrong note.
                    let bitcoin_node = site
                        .get()
                        .map(|site| site.cli_network.is_none())
                        .unwrap_or(false);
                    bitcoin_node.then(|| {
                        view! {
                            <p>
                                "This node mines the Bitcoin chain, which has no share chain address; "
                                "a payout address here names bitcoin coinbase outputs only."
                            </p>
                        }
                    })
                }}
            </div>
            <div class="help-section">
                <h3>"Pages"</h3>
                <ul>
                    <li>
                        <a href="#/">"Dashboard"</a>
                        ": hashrate, found blocks and the newest shares."
                    </li>
                    <li>
                        <a href="#/shares">"Shares"</a>
                        ": the confirmed share chain, newest first."
                    </li>
                    <li><a href="#/blocks">"Blocks"</a> ": every block the pool found."</li>
                    <li>
                        <a href="#/miners">"Miners"</a>
                        ": the miners on this node and their workers."
                    </li>
                    <li>
                        <a href="#/peers">"Peers"</a>
                        ": the peers connected to this node, longest uptime first."
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
                        <code>"/miners"</code> ", " <code>"/network"</code> ", "
                        <code>"/peers"</code>
                    </li>
                    <li>
                        <code>"/metrics"</code> " (Prometheus), " <code>"/blocked_ips"</code>
                        ", " <code>"/jd_prepare"</code> " and " <code>"/share_transaction"</code>
                        ": always behind the API login"
                    </li>
                </ul>
            </div>
            <div class="help-section">
                <h3>"About this dashboard"</h3>
                <p>
                    "This dashboard is Rust compiled to WebAssembly. It reads the node's JSON API, "
                    "takes new shares live from its /ws feed, and lets you zoom the hashrate chart. "
                    "It uses the stylesheets of the pages the node renders itself, so both look the same."
                </p>
            </div>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The payout travels percent-encoded: a colon survives (the node's
    /// own pages keep it, and `url_component` does too), spaces do not.
    #[test]
    fn the_share_address_path_encodes_the_payout() {
        assert_eq!(
            share_address_path("bchtest:qr7fz ep"),
            "/share_address?payout=bchtest:qr7fz%20ep"
        );
    }

    /// The example fields fill only once a share address came back: a
    /// refused payout leaves the placeholder in place.
    #[test]
    fn the_example_payout_waits_for_an_accepted_share_address() {
        assert_eq!(example_payout("nope", ""), "<payout address>");
        assert_eq!(example_payout("bchtest:q1", "tp2pool:q2"), "bchtest:q1");
    }
}
