// SPDX-FileCopyrightText: 2024 PDM Authors
// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Client for the Bitcoin Cash P2Poolv2 node's JSON API, ported from PDM's
//! `P2PoolClient` and extended to the fork's endpoints.
//!
//! Requests carry the API login as HTTP Basic auth when the node config has
//! `[api] auth_user` and `auth_password`. Error messages read like the WASM
//! dashboard's (`p2poolv2_web/src/api.rs`).

use crate::client::http::{MAX_BODY_BYTES, build_client, describe, read_capped, url_component};
use crate::model::{
    ChainInfo, FoundBlock, Hashrate, MAX_FOUND_BLOCKS, MAX_SHARE_CHAIN_PEERS, Miner, Miners, Peer,
    Shares, Site,
};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use reqwest::Client;
use serde::{Deserialize, de::DeserializeOwned};
use std::fmt;

/// Seconds of hashrate history asked for.
pub const HASHRATE_WINDOW_SECS: u64 = 3_600;

/// Recent shares asked for; the live feed adds to them.
pub const RECENT_SHARES: u32 = 25;

/// Talks to one node's API.
#[derive(Clone)]
pub struct ApiClient {
    client: Client,
    /// E.g. `http://127.0.0.1:46884`, without a trailing slash.
    base: String,
    credentials: Option<(String, String)>,
}

impl fmt::Debug for ApiClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ApiClient")
            .field("base", &self.base)
            .field(
                "credentials",
                &self
                    .credentials
                    .as_ref()
                    .map(|(user, _)| (user, "[redacted]")),
            )
            .finish()
    }
}

/// The body of an API error: `{"error": "..."}`.
#[derive(Deserialize)]
struct ErrorBody {
    error: String,
}

impl ApiClient {
    /// A client for the API at `base`, logging in with `credentials` (user,
    /// password) when given.
    pub fn new(base: &str, credentials: Option<(String, String)>) -> Result<Self, String> {
        Ok(Self {
            client: build_client()?,
            base: base.trim_end_matches('/').to_string(),
            credentials,
        })
    }

    /// The API's base URL.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// GET `path` (with its query) and decode the JSON answer.
    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let mut request = self.client.get(format!("{}{path}", self.base));
        if let Some((user, password)) = &self.credentials {
            request = request.basic_auth(user, Some(password));
        }
        let response = request.send().await.map_err(describe)?;
        let status = response.status().as_u16();
        let body = read_capped(response, MAX_BODY_BYTES).await?;
        let body = String::from_utf8_lossy(&body);
        if !(200..300).contains(&status) {
            return Err(error_message(status, &body, path));
        }
        decode(&body, path)
    }

    pub async fn network(&self) -> Result<Site, String> {
        self.get("/network").await
    }

    pub async fn chain_info(&self) -> Result<ChainInfo, String> {
        self.get("/chain_info").await
    }

    pub async fn hashrate(&self) -> Result<Hashrate, String> {
        self.get(&format!("/hashrate?window={HASHRATE_WINDOW_SECS}"))
            .await
    }

    pub async fn shares(&self) -> Result<Shares, String> {
        self.get(&format!("/shares?num={RECENT_SHARES}")).await
    }

    pub async fn miners(&self) -> Result<Miners, String> {
        self.get("/miners?page=1").await
    }

    /// One miner's place in the PPLNS window, without its share list.
    pub async fn miner(&self, address: &str) -> Result<Miner, String> {
        self.get(&format!(
            "/miner?address={}&limit=1",
            url_component(address)
        ))
        .await
    }

    /// The pool's found blocks, newest first, at most [`MAX_FOUND_BLOCKS`].
    pub async fn found_blocks(&self) -> Result<Vec<FoundBlock>, String> {
        let mut blocks: Vec<FoundBlock> = self.get("/found_blocks").await?;
        blocks.truncate(MAX_FOUND_BLOCKS);
        Ok(blocks)
    }

    /// The node's share chain peers, at most [`MAX_SHARE_CHAIN_PEERS`].
    /// `/peers` always needs the API login when the node has one.
    pub async fn peers(&self) -> Result<Vec<Peer>, String> {
        let mut peers: Vec<Peer> = self.get("/peers").await?;
        peers.truncate(MAX_SHARE_CHAIN_PEERS);
        Ok(peers)
    }

    /// The `/ws` URL. The login goes in `?token=`, base64 of
    /// `user:password`, which is how the node's WebSocket handler takes it.
    pub fn websocket_url(&self) -> String {
        let base = if let Some(rest) = self.base.strip_prefix("https://") {
            format!("wss://{rest}")
        } else if let Some(rest) = self.base.strip_prefix("http://") {
            format!("ws://{rest}")
        } else {
            self.base.clone()
        };
        match &self.credentials {
            Some((user, password)) => {
                let token = STANDARD.encode(format!("{user}:{password}"));
                format!("{base}/ws?token={}", url_component(&token))
            }
            None => format!("{base}/ws"),
        }
    }
}

/// Decode a successful answer. A node without the endpoint but with a
/// `web_root` answers with the frontend's HTML page instead of JSON.
pub fn decode<T: DeserializeOwned>(body: &str, path: &str) -> Result<T, String> {
    serde_json::from_str::<T>(body).map_err(|error| {
        if body.trim_start().starts_with('<') {
            not_served(path)
        } else {
            format!("{} answered in an unexpected form: {error}", endpoint(path))
        }
    })
}

/// What to show for a failed request: the node's own message when it sent
/// one, otherwise what the status means here.
pub fn error_message(status: u16, body: &str, path: &str) -> String {
    if let Ok(parsed) = serde_json::from_str::<ErrorBody>(body) {
        return parsed.error;
    }
    match status {
        401 => {
            "Unauthorized: set [api] auth_user and auth_password in the node config.".to_string()
        }
        404 if body.trim().is_empty() => not_served(path),
        status => format!("HTTP {status}: {}", body.trim()),
    }
}

fn endpoint(path: &str) -> &str {
    path.split('?').next().unwrap_or(path)
}

fn not_served(path: &str) -> String {
    format!(
        "{} is not served by this node; upgrade it to a release with this endpoint.",
        endpoint(path)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn test_network_decodes_the_answer() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/network"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"{"network":"Bitcoin Cash chipnet","badge":"chipnet"}"#),
            )
            .mount(&server)
            .await;
        let client = ApiClient::new(&server.uri(), None).unwrap();
        let site = client.network().await.unwrap();
        assert_eq!(site.badge, "chipnet");
    }

    #[tokio::test]
    async fn test_requests_carry_the_api_login() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/peers"))
            .and(header("authorization", "Basic dTpwdw=="))
            .respond_with(ResponseTemplate::new(200).set_body_string("[]"))
            .mount(&server)
            .await;
        let client = ApiClient::new(&server.uri(), Some(("u".into(), "pw".into()))).unwrap();
        assert_eq!(client.peers().await.unwrap(), Vec::new());
    }

    #[tokio::test]
    async fn test_miner_encodes_the_address() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/miner"))
            .and(query_param("address", "bchtest:qq9"))
            .and(query_param("limit", "1"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"address":"bchtest:qq9","address_type":"payout","anchor":"aa",
                "anchor_height":5,"window":{"share_count":4},"share_count":1,"uncle_count":0,
                "share_fraction":0.25,"payout_fraction":0.25,"shares":[]}"#,
            ))
            .mount(&server)
            .await;
        let client = ApiClient::new(&server.uri(), None).unwrap();
        assert_eq!(client.miner("bchtest:qq9").await.unwrap().share_count, 1);
    }

    #[tokio::test]
    async fn test_found_blocks_are_capped() {
        let server = MockServer::start().await;
        let block = r#"{"bitcoin_height":1,"bitcoin_blockhash":"aa","share_blockhash":"bb",
            "bitcoin_time":2,"miner_bitcoin_address":"bchtest:q"}"#;
        let body = format!("[{}]", vec![block; MAX_FOUND_BLOCKS + 5].join(","));
        Mock::given(method("GET"))
            .and(path("/found_blocks"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(&server)
            .await;
        let client = ApiClient::new(&server.uri(), None).unwrap();
        assert_eq!(client.found_blocks().await.unwrap().len(), MAX_FOUND_BLOCKS);
    }

    #[tokio::test]
    async fn test_an_unauthorized_answer_names_the_config_keys() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let client = ApiClient::new(&server.uri(), None).unwrap();
        assert_eq!(
            client.peers().await.unwrap_err(),
            "Unauthorized: set [api] auth_user and auth_password in the node config."
        );
    }

    #[test]
    fn test_error_message_prefers_the_nodes_own() {
        assert_eq!(
            error_message(400, r#"{"error":"Invalid address 'x'"}"#, "/miner"),
            "Invalid address 'x'"
        );
    }

    #[test]
    fn test_error_message_of_a_missing_endpoint() {
        assert_eq!(
            error_message(404, "", "/miners?page=1"),
            "/miners is not served by this node; upgrade it to a release with this endpoint."
        );
    }

    #[test]
    fn test_decode_of_a_frontend_page_is_a_missing_endpoint() {
        assert!(
            decode::<Site>("<!doctype html><p>frontend</p>", "/network")
                .unwrap_err()
                .contains("is not served by this node")
        );
    }

    #[test]
    fn test_decode_of_the_wrong_shape() {
        assert!(
            decode::<Site>("{}", "/network")
                .unwrap_err()
                .starts_with("/network answered in an unexpected form")
        );
    }

    #[test]
    fn test_websocket_url_without_a_login() {
        let client = ApiClient::new("http://127.0.0.1:46884/", None).unwrap();
        assert_eq!(client.websocket_url(), "ws://127.0.0.1:46884/ws");
    }

    #[test]
    fn test_websocket_url_carries_the_login_as_a_token() {
        let client =
            ApiClient::new("https://pool.example", Some(("u".into(), "pw".into()))).unwrap();
        assert_eq!(
            client.websocket_url(),
            "wss://pool.example/ws?token=dTpwdw%3D%3D"
        );
    }

    #[test]
    fn test_debug_hides_the_password() {
        let client =
            ApiClient::new("http://127.0.0.1:46884", Some(("u".into(), "pw".into()))).unwrap();
        let shown = format!("{client:?}");
        assert!(!shown.contains("pw\""), "{shown}");
        assert!(shown.contains("[redacted]"), "{shown}");
    }
}
