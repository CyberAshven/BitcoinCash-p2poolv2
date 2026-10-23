// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Client for the node's JSON API and its `/ws` feed.
//!
//! The types mirror only the fields the dashboard shows; serde ignores the
//! rest, so new fields on the node never break the app.

use crate::format::url_component;
use base64::Engine;
use serde::{Deserialize, de::DeserializeOwned};

/// Where the API is and how to authenticate to it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Connection {
    /// API base URL without a trailing slash, e.g. `http://127.0.0.1:46884`.
    pub base: String,
    pub user: String,
    pub password: String,
}

impl Connection {
    /// Base64 `user:password`, or `None` when no user is set.
    pub fn credentials(&self) -> Option<String> {
        if self.user.is_empty() {
            return None;
        }
        Some(
            base64::engine::general_purpose::STANDARD
                .encode(format!("{}:{}", self.user, self.password)),
        )
    }

    /// The `/ws` URL; credentials go in `?token=` because browsers cannot set
    /// headers on a WebSocket handshake.
    pub fn websocket_url(&self) -> String {
        let base = if let Some(rest) = self.base.strip_prefix("https://") {
            format!("wss://{rest}")
        } else if let Some(rest) = self.base.strip_prefix("http://") {
            format!("ws://{rest}")
        } else {
            self.base.clone()
        };
        match self.credentials() {
            Some(token) => format!("{base}/ws?token={}", url_component(&token)),
            None => format!("{base}/ws"),
        }
    }

    /// GET `path` (with query) and decode the JSON body. An error is the
    /// message to show, as the node's own pages would word it.
    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let mut request = gloo_net::http::Request::get(&format!("{}{path}", self.base));
        if let Some(token) = self.credentials() {
            request = request.header("Authorization", &format!("Basic {token}"));
        }
        let response = request.send().await.map_err(|error| error.to_string())?;
        let body = response.text().await.map_err(|error| error.to_string())?;
        if !response.ok() {
            return Err(error_message(response.status(), &body, path));
        }
        decode(&body, path)
    }
}

/// Decode a successful response. A node without the endpoint but with a
/// `web_root` answers with the frontend's HTML page instead of JSON.
pub fn decode<T: DeserializeOwned>(body: &str, path: &str) -> Result<T, String> {
    serde_json::from_str::<T>(body).map_err(|error| {
        if body.trim_start().starts_with('<') {
            not_served(path)
        } else {
            format!("{path} answered in an unexpected form: {error}")
        }
    })
}

/// What to say when the node has no such endpoint.
fn not_served(path: &str) -> String {
    let endpoint = path.split('?').next().unwrap_or(path);
    format!("{endpoint} is not served by this node; upgrade it to a release with this endpoint.")
}

/// The body of an API error: `{"error": "..."}`.
#[derive(Deserialize)]
struct ErrorBody {
    error: String,
}

/// What to show for a failed request: the node's own message when it sent
/// one, otherwise what the status means here.
pub fn error_message(status: u16, body: &str, path: &str) -> String {
    if let Ok(parsed) = serde_json::from_str::<ErrorBody>(body) {
        return parsed.error;
    }
    match status {
        401 => "Unauthorized: set the API user and password in Settings.".to_string(),
        404 if body.is_empty() => not_served(path),
        status => format!("HTTP {status}: {body}"),
    }
}

/// `/network`: the chain the node mines.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Site {
    /// In full, e.g. `Bitcoin Cash chipnet`.
    pub network: String,
    /// For the header badge, e.g. `chipnet`.
    pub badge: String,
}

/// `/chain_info`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ChainInfo {
    pub chain_tip_height: Option<u32>,
}

/// One sample of `/hashrate`'s history, in hashes per second.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct HashratePoint {
    pub time: u64,
    pub pool_hashrate: Option<f64>,
    pub local_hashrate: Option<f64>,
}

/// `/hashrate?window=`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Hashrate {
    pub pool_hashrate: Option<f64>,
    pub local_hashrate: Option<f64>,
    pub network_hashrate: Option<f64>,
    /// Oldest first.
    pub history: Vec<HashratePoint>,
}

/// An uncle a confirmed share includes.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Uncle {
    pub blockhash: String,
    pub miner_bitcoin_address: String,
    pub miner_address: String,
    pub timestamp: u64,
    #[serde(default)]
    pub height: Option<u32>,
}

/// A confirmed share, from `/shares` or a `/ws` share event.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Share {
    pub blockhash: String,
    pub height: u32,
    pub miner_bitcoin_address: String,
    pub miner_address: String,
    pub timestamp: u64,
    /// Compact share target.
    pub bits: u32,
    #[serde(default)]
    pub uncles: Vec<Uncle>,
}

/// `/shares?to=&num=`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Shares {
    pub shares: Vec<Share>,
}

/// One of `/found_blocks`, newest first.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct FoundBlock {
    pub bitcoin_height: u64,
    pub bitcoin_blockhash: String,
    pub share_blockhash: String,
    pub bitcoin_time: u64,
    pub miner_bitcoin_address: String,
}

/// The Bitcoin Cash header a share carries.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct BitcoinHeader {
    pub block_hash: String,
    pub version: i32,
    pub prev_blockhash: String,
    pub merkle_root: String,
    pub time: String,
    pub bits: String,
    pub nonce: u32,
}

/// One of `/share?hash=`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct ShareDetail {
    pub blockhash: String,
    pub height: Option<u32>,
    pub validation_status: String,
    pub chain: String,
    pub parent: String,
    pub uncles: Vec<String>,
    pub miner_bitcoin_address: String,
    pub miner_address: String,
    pub bits: String,
    pub time: String,
    pub bitcoin_header: BitcoinHeader,
}

/// A share or uncle a miner earns weight from in the window.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct MinerShare {
    pub blockhash: String,
    pub height: u32,
    /// The window share including this uncle, `None` for a share.
    pub nephew: Option<String>,
    pub difficulty: String,
    pub weight: String,
}

/// The PPLNS window as a whole.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Window {
    pub share_count: usize,
}

/// `/miner?address=`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Miner {
    pub address: String,
    pub address_type: String,
    pub anchor: String,
    pub anchor_height: u32,
    pub window: Window,
    pub share_count: usize,
    pub uncle_count: usize,
    pub share_fraction: f64,
    pub payout_fraction: f64,
    pub shares: Vec<MinerShare>,
}

/// A worker of a miner on this node.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Worker {
    pub name: String,
    pub active: bool,
    pub accepted_work: u64,
    pub best_share: u64,
    pub last_share_at: u64,
}

/// A miner on this node.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct LocalMiner {
    pub address: String,
    pub workers: Vec<Worker>,
    pub accepted_work: u64,
    pub best_share: u64,
    pub last_share_at: u64,
    pub payout_fraction: Option<f64>,
}

/// `/miners?page=` or `/miners?address=`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Miners {
    pub miners_online: usize,
    pub workers_online: usize,
    pub page: usize,
    pub pages: usize,
    pub page_size: usize,
    pub miners: Vec<LocalMiner>,
}

/// A `/ws` frame: `{"topic":"Share","data":{...}}`.
#[derive(Deserialize)]
struct Event {
    topic: String,
    data: serde_json::Value,
}

/// Decode a `/ws` text frame into a share, ignoring other topics.
pub fn parse_share_event(text: &str) -> Option<Share> {
    let event: Event = serde_json::from_str(text).ok()?;
    if event.topic != "Share" {
        return None;
    }
    serde_json::from_value(event.data).ok()
}

/// The frame that subscribes to share events.
pub const SUBSCRIBE_SHARES: &str = r#"{"action":"subscribe","topic":"shares"}"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_websocket_url_without_credentials() {
        let connection = Connection {
            base: "http://127.0.0.1:46884".into(),
            user: String::new(),
            password: "pw".into(),
        };
        assert_eq!(connection.websocket_url(), "ws://127.0.0.1:46884/ws");
    }

    #[test]
    fn test_websocket_url_carries_credentials() {
        let connection = Connection {
            base: "https://pool.example".into(),
            user: "u".into(),
            password: "pw".into(),
        };
        assert_eq!(
            connection.websocket_url(),
            "wss://pool.example/ws?token=dTpwdw%3D%3D"
        );
        assert_eq!(connection.credentials().as_deref(), Some("dTpwdw=="));
    }

    #[test]
    fn test_error_message_prefers_the_nodes_own() {
        assert_eq!(
            error_message(400, r#"{"error":"Invalid address 'x'"}"#, "/miner"),
            "Invalid address 'x'"
        );
        assert!(error_message(401, "", "/miners").starts_with("Unauthorized"));
        assert!(error_message(404, "", "/miners").contains("not served by this node"));
        assert_eq!(error_message(500, "boom", "/x"), "HTTP 500: boom");
    }

    #[test]
    fn test_a_frontend_page_in_place_of_json_is_a_missing_endpoint() {
        let decoded = decode::<Site>("<!doctype html><p>frontend</p>", "/network?x=1");
        assert_eq!(
            decoded,
            Err(
                "/network is not served by this node; upgrade it to a release with this endpoint."
                    .to_string()
            )
        );
        assert!(
            decode::<Site>("{}", "/network")
                .unwrap_err()
                .contains("unexpected form")
        );
    }

    #[test]
    fn test_parse_share_event() {
        let frame = r#"{"topic":"Share","data":{"blockhash":"ab","prev_blockhash":"cd","height":7,
            "miner_bitcoin_address":"bchtest:q","miner_address":"tp2pool:q","timestamp":5,
            "bits":486604799,"uncles":[{"blockhash":"ef","prev_blockhash":"aa",
            "miner_bitcoin_address":"bchtest:u","miner_address":"tp2pool:u","timestamp":4,
            "height":6}]}}"#;
        let share = parse_share_event(frame).unwrap();
        assert_eq!(share.height, 7);
        assert_eq!(share.bits, 0x1d00_ffff);
        assert_eq!(share.uncles[0].height, Some(6));
        assert!(parse_share_event(r#"{"topic":"Peer","data":{}}"#).is_none());
        assert!(parse_share_event("not json").is_none());
    }

    #[test]
    fn test_miners_page_parses() {
        let body = r#"{"miners_online":1,"workers_online":2,"total":1,"page":1,"pages":1,
            "page_size":25,"miners":[{"address":"bchtest:q","workers":[{"name":"rig",
            "active":true,"accepted_work":10,"best_share":5,"last_share_at":9}],
            "accepted_work":10,"best_share":5,"last_share_at":9,"payout_fraction":null}]}"#;
        let miners: Miners = serde_json::from_str(body).unwrap();
        assert_eq!(miners.miners[0].workers[0].name, "rig");
        assert_eq!(miners.miners[0].payout_fraction, None);
    }
}
