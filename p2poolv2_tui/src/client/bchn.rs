// SPDX-FileCopyrightText: 2024 PDM Authors
// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! BCHN's JSON-RPC, ported from PDM's Bitcoin Core client.
//!
//! The node's own `bitcoindrpc` client is not reused: it has no request
//! timeout and reads whole bodies, and a status screen polling a node that
//! hangs must neither block nor grow. The calls are the same JSON-RPC 1.0
//! with HTTP Basic auth.

use crate::client::http::{MAX_BODY_BYTES, build_client, describe, read_capped};
use crate::model::{
    BchnChain, BchnMempool, BchnMining, BchnNetwork, BchnPeer, BchnStatus, MAX_BCHN_PEERS,
};
use reqwest::Client;
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::json;
use std::fmt;

/// Talks to one BCHN.
#[derive(Clone)]
pub struct BchnClient {
    client: Client,
    url: String,
    username: String,
    password: String,
}

impl fmt::Debug for BchnClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BchnClient")
            .field("url", &self.url)
            .field("username", &self.username)
            .field("password", &"[redacted]")
            .finish()
    }
}

/// A JSON-RPC 1.0 answer: a result, or an error.
#[derive(Deserialize)]
struct Reply<T> {
    result: Option<T>,
    error: Option<RpcError>,
}

#[derive(Deserialize)]
struct RpcError {
    code: i64,
    message: String,
}

impl BchnClient {
    pub fn new(url: &str, username: &str, password: &str) -> Result<Self, String> {
        Ok(Self {
            client: build_client()?,
            url: url.to_string(),
            username: username.to_string(),
            password: password.to_string(),
        })
    }

    /// Calls `method` without parameters.
    async fn call<T: DeserializeOwned>(&self, method: &str) -> Result<T, String> {
        let request = json!({
            "jsonrpc": "1.0",
            "id": "p2poolv2_tui",
            "method": method,
            "params": [],
        });
        let response = self
            .client
            .post(&self.url)
            .basic_auth(&self.username, Some(&self.password))
            .json(&request)
            .send()
            .await
            .map_err(describe)?;
        let status = response.status().as_u16();
        if status == 401 || status == 403 {
            return Err(format!(
                "BCHN refused the RPC login (HTTP {status}): check [bitcoinrpc] username and \
                 password in the node config"
            ));
        }
        let body = read_capped(response, MAX_BODY_BYTES).await?;
        let reply: Reply<T> = serde_json::from_slice(&body).map_err(|error| {
            if (200..300).contains(&status) {
                format!("BCHN answered {method} in an unexpected form: {error}")
            } else {
                format!("BCHN answered {method} with HTTP {status}")
            }
        })?;
        if let Some(error) = reply.error {
            return Err(format!(
                "BCHN {method} failed: {} (code {})",
                error.message, error.code
            ));
        }
        reply
            .result
            .ok_or_else(|| format!("BCHN answered {method} without a result"))
    }

    /// One refresh of the chain, network, mempool and peers, asked for at
    /// once. The network hashrate is left out when `getmininginfo` fails.
    pub async fn status(&self) -> Result<BchnStatus, String> {
        let (chain, network, mempool, peers, mining) = tokio::join!(
            self.call::<BchnChain>("getblockchaininfo"),
            self.call::<BchnNetwork>("getnetworkinfo"),
            self.call::<BchnMempool>("getmempoolinfo"),
            self.call::<Vec<BchnPeer>>("getpeerinfo"),
            self.call::<BchnMining>("getmininginfo"),
        );
        let mut peers = peers?;
        peers.truncate(MAX_BCHN_PEERS);
        Ok(BchnStatus {
            chain: chain?,
            network: network?,
            mempool: mempool?,
            peers,
            network_hashrate: mining.ok().and_then(|mining| mining.network_hashrate),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_partial_json, header, method};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn test_status_reads_all_five_calls() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(body_partial_json(json!({"method": "getblockchaininfo"})))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"result":{"chain":"chip","blocks":10,"headers":10,"bestblockhash":"00ab",
                "difficulty":2.0},"error":null,"id":"p2poolv2_tui"}"#,
            ))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(body_partial_json(json!({"method": "getnetworkinfo"})))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"result":{"subversion":"/Bitcoin Cash Node:28.0.1/","protocolversion":70016,
                "connections":3},"error":null}"#,
            ))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(body_partial_json(json!({"method": "getmempoolinfo"})))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"result":{"size":1,"bytes":2,"usage":3,"maxmempool":4,
                "mempoolminfee":0.00001},"error":null}"#,
            ))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(body_partial_json(json!({"method": "getpeerinfo"})))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"result":[{"id":1,"addr":"203.0.113.5:48333"}],"error":null}"#,
            ))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(body_partial_json(json!({"method": "getmininginfo"})))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(r#"{"result":{"networkhashps":5.0e12},"error":null}"#),
            )
            .mount(&server)
            .await;
        let client = BchnClient::new(&server.uri(), "p2pool", "pw").unwrap();
        let status = client.status().await.unwrap();
        assert_eq!(status.chain.chain, "chip");
        assert_eq!(status.network.connections, 3);
        assert_eq!(status.mempool.size, 1);
        assert_eq!(status.peers[0].address, "203.0.113.5:48333");
        assert_eq!(status.network_hashrate, Some(5.0e12));
    }

    #[tokio::test]
    async fn test_calls_carry_the_rpc_login() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(header("authorization", "Basic cDJwb29sOnB3"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(r#"{"result":7,"error":null}"#),
            )
            .mount(&server)
            .await;
        let client = BchnClient::new(&server.uri(), "p2pool", "pw").unwrap();
        assert_eq!(client.call::<u64>("getblockcount").await.unwrap(), 7);
    }

    #[tokio::test]
    async fn test_an_rpc_error_is_reported() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500).set_body_string(
                r#"{"result":null,"error":{"code":-28,"message":"Loading block index..."}}"#,
            ))
            .mount(&server)
            .await;
        let client = BchnClient::new(&server.uri(), "p2pool", "pw").unwrap();
        assert_eq!(
            client.call::<u64>("getblockcount").await.unwrap_err(),
            "BCHN getblockcount failed: Loading block index... (code -28)"
        );
    }

    #[tokio::test]
    async fn test_a_refused_login_does_not_show_the_password() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let client = BchnClient::new(&server.uri(), "p2pool", "hunter2").unwrap();
        let error = client.status().await.unwrap_err();
        assert!(
            error.starts_with("BCHN refused the RPC login (HTTP 401)"),
            "{error}"
        );
        assert!(!error.contains("hunter2"), "{error}");
    }

    #[tokio::test]
    async fn test_status_keeps_at_most_max_peers() {
        let server = MockServer::start().await;
        let peer = r#"{"id":1,"addr":"203.0.113.5:48333"}"#;
        let peers = format!(
            r#"{{"result":[{}],"error":null}}"#,
            vec![peer; MAX_BCHN_PEERS + 10].join(",")
        );
        Mock::given(method("POST"))
            .and(body_partial_json(json!({"method": "getpeerinfo"})))
            .respond_with(ResponseTemplate::new(200).set_body_string(peers))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(body_partial_json(json!({"method": "getblockchaininfo"})))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"result":{"chain":"main","blocks":1,"headers":1,"bestblockhash":"00",
                "difficulty":1.0},"error":null}"#,
            ))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(body_partial_json(json!({"method": "getnetworkinfo"})))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"result":{"subversion":"/BCHN/","protocolversion":1,"connections":0},
                "error":null}"#,
            ))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(body_partial_json(json!({"method": "getmempoolinfo"})))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"result":{"size":0,"bytes":0,"usage":0,"maxmempool":0,"mempoolminfee":0.0},
                "error":null}"#,
            ))
            .mount(&server)
            .await;
        let client = BchnClient::new(&server.uri(), "p2pool", "pw").unwrap();
        let status = client.status().await.unwrap();
        assert_eq!(status.peers.len(), MAX_BCHN_PEERS);
        assert_eq!(status.network_hashrate, None);
    }

    #[test]
    fn test_debug_hides_the_password() {
        let client = BchnClient::new("http://127.0.0.1:48332", "p2pool", "hunter2").unwrap();
        let shown = format!("{client:?}");
        assert!(!shown.contains("hunter2"), "{shown}");
    }
}
