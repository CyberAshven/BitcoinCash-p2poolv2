// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The peers page, as `/dashboard/peers` shows it: every connected peer,
//! longest uptime first, one table and no paging. The JSON `/peers`
//! endpoint already returns the list in that order.

use super::overview::{NO_PEERS_YET, PeerTable};
use crate::api::Peer;
use crate::app::{ctx, polled};
use crate::layout::{Crumb, ErrorPage, dashboard_crumb, loading};
use crate::route::Tab;
use leptos::prelude::*;

/// What the panel's note says about `peers`: the sort order the endpoint
/// guarantees and how many answered, worded as the node words it.
fn note_text(peers: &[Peer]) -> String {
    if peers.is_empty() {
        "longest uptime first \u{b7} none connected".to_string()
    } else {
        format!("longest uptime first \u{b7} {} connected", peers.len())
    }
}

/// Every connected peer, longest uptime first, exactly as the node's
/// `/dashboard/peers` serves them. The list is polled with the rest of the
/// app on its refresh timer, and the endpoint sorts it, so this page
/// carries no cap, no sort and no pager.
#[component]
pub fn PeersPage() -> impl IntoView {
    let context = ctx();
    context.frame.set(
        "Peers",
        Tab::None,
        vec![dashboard_crumb(), Crumb::here("Peers")],
    );
    let peers = polled::<Vec<Peer>>(|| "/peers".into());
    move || match peers.get() {
        None => loading_panel(),
        Some(Err(message)) => view! { <ErrorPage message=message /> }.into_any(),
        Some(Ok(all)) if all.is_empty() => empty_panel(),
        Some(Ok(all)) => view! {
            <div class="panel">
                <h2>
                    "Peers"
                    <span class="panel-note">{note_text(&all)}</span>
                </h2>
                <PeerTable peers=all />
            </div>
        }
        .into_any(),
    }
}

/// The panel before `/peers` answered.
fn loading_panel() -> AnyView {
    view! {
        <div class="panel">
            <h2>"Peers"</h2>
            {loading()}
        </div>
    }
    .into_any()
}

/// The panel that says no peer is connected, worded as the node words it.
fn empty_panel() -> AnyView {
    let note = note_text(&[]);
    view! {
        <div class="panel">
            <h2>
                "Peers"
                <span class="panel-note">{note}</span>
            </h2>
            <p class="empty-state">{NO_PEERS_YET}</p>
        </div>
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connected_peer(address: &str, connected_secs: u64) -> Peer {
        Peer {
            ip: Some(address.to_string()),
            address: format!("/ip4/{address}/tcp/6886"),
            direction: "Inbound".to_string(),
            connected_secs,
            version: Some("p2poolv2/0.0.1-alpha".to_string()),
            latency_ms: Some(0.5),
        }
    }

    #[test]
    fn test_the_note_counts_every_connected_peer() {
        let peers = vec![
            connected_peer("10.0.0.1", 100),
            connected_peer("10.0.0.6", 5),
        ];
        assert_eq!(note_text(&peers), "longest uptime first \u{b7} 2 connected");
    }

    #[test]
    fn test_the_note_says_so_without_peers() {
        assert_eq!(note_text(&[]), "longest uptime first \u{b7} none connected");
    }
}
