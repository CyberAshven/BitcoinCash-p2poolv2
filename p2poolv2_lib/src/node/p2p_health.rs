// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! P2P health snapshot for Prometheus exposition.
//!
//! The counters live where their events happen -- the connection tracker and
//! the request-response handler -- as plain integers updated on the node actor
//! loop. They are not sent to the metrics actor: every `MetricsHandle` call
//! awaits a reply, which the loop must never do. Instead `/metrics` asks the
//! node for a snapshot at scrape time (`Command::GetP2pHealth`).
//!
//! Counters reset when the node restarts; `rate()` handles counter resets.
//! No labels: peer ids are unbounded and churn, so every series is an
//! aggregate.

use crate::node::p2p_message_handlers::senders::share_block::ShareRelayBytes;
use std::fmt::Write;

/// Point-in-time P2P health of the node.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct P2pHealth {
    /// Peers with at least one open connection.
    pub connected_peers: u64,
    /// Connections accepted since start.
    pub connections_total: u64,
    /// Ping failures reported by libp2p since start.
    pub ping_failures_total: u64,
    /// Connections closed because they kept failing ping.
    pub connections_closed_unresponsive_total: u64,
    /// Requests this node sent that failed (timeout, connection closed, ...).
    pub outbound_failures_total: u64,
    /// Requests from peers that this node failed to answer.
    pub inbound_failures_total: u64,
    /// Responses dropped because the response worker queue was full.
    pub responses_dropped_total: u64,
    /// Responses waiting in the response worker queue.
    pub response_queue_depth: u64,
    /// Share messages this node relayed to peers since start, their payload
    /// bytes, and the bytes they would have taken with every weak share
    /// whole.
    pub share_relay: ShareRelayBytes,
}

impl P2pHealth {
    /// Render as Prometheus exposition text.
    pub fn exposition(&self) -> String {
        let series: [(&str, &str, &str, u64); 12] = [
            (
                "p2p_connected_peers",
                "gauge",
                "Peers with at least one open connection",
                self.connected_peers,
            ),
            (
                "p2p_connections_total",
                "counter",
                "Connections accepted since start",
                self.connections_total,
            ),
            (
                "p2p_ping_failures_total",
                "counter",
                "Ping failures reported by libp2p since start",
                self.ping_failures_total,
            ),
            (
                "p2p_connections_closed_unresponsive_total",
                "counter",
                "Connections closed because they kept failing ping",
                self.connections_closed_unresponsive_total,
            ),
            (
                "p2p_outbound_failures_total",
                "counter",
                "Requests sent by this node that failed",
                self.outbound_failures_total,
            ),
            (
                "p2p_inbound_failures_total",
                "counter",
                "Requests from peers this node failed to answer",
                self.inbound_failures_total,
            ),
            (
                "p2p_responses_dropped_total",
                "counter",
                "Responses dropped because the response worker queue was full",
                self.responses_dropped_total,
            ),
            (
                "p2p_response_queue_depth",
                "gauge",
                "Responses waiting in the response worker queue",
                self.response_queue_depth,
            ),
            (
                "p2p_share_relay_messages_total",
                "counter",
                "Share messages this node relayed to peers",
                self.share_relay.messages,
            ),
            (
                "p2p_share_relay_bytes_total",
                "counter",
                "Payload bytes of the share messages this node relayed, weak shares listed by hash",
                self.share_relay.bytes,
            ),
            (
                "p2p_share_relay_whole_bytes_total",
                "counter",
                "Payload bytes the share messages this node relayed would have taken with every weak share whole",
                self.share_relay.whole_bytes,
            ),
            (
                "p2p_share_relay_prefilled_weak_shares_total",
                "counter",
                "Weak shares the share messages this node relayed sent whole beside their hashes, for peers likely to lack them",
                self.share_relay.prefilled,
            ),
        ];
        let mut exposition = String::with_capacity(series.len() * 160);
        for (name, kind, help, value) in series {
            // Writing to a String cannot fail.
            let _ = writeln!(exposition, "# HELP {name} {help}");
            let _ = writeln!(exposition, "# TYPE {name} {kind}");
            let _ = writeln!(exposition, "{name} {value}");
        }
        exposition
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exposition_renders_each_series_with_type_and_value() {
        let health = P2pHealth {
            connected_peers: 2,
            connections_total: 5,
            ping_failures_total: 7,
            connections_closed_unresponsive_total: 1,
            outbound_failures_total: 3,
            inbound_failures_total: 4,
            responses_dropped_total: 6,
            response_queue_depth: 9,
            share_relay: ShareRelayBytes {
                messages: 10,
                bytes: 11,
                whole_bytes: 12,
                prefilled: 13,
            },
        };

        let exposition = health.exposition();

        assert!(exposition.contains("# TYPE p2p_connected_peers gauge\np2p_connected_peers 2\n"));
        assert!(exposition.contains(
            "# TYPE p2p_connections_closed_unresponsive_total counter\np2p_connections_closed_unresponsive_total 1\n"
        ));
        assert!(exposition.contains("p2p_outbound_failures_total 3\n"));
        assert!(exposition.contains("p2p_responses_dropped_total 6\n"));
        assert!(
            exposition
                .contains("# TYPE p2p_response_queue_depth gauge\np2p_response_queue_depth 9\n")
        );
        assert!(exposition.contains(
            "# TYPE p2p_share_relay_messages_total counter\np2p_share_relay_messages_total 10\n"
        ));
        assert!(exposition.contains("p2p_share_relay_bytes_total 11\n"));
        assert!(exposition.contains("p2p_share_relay_whole_bytes_total 12\n"));
        assert!(exposition.contains("p2p_share_relay_prefilled_weak_shares_total 13\n"));
    }
}
