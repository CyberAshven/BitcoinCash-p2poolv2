// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Reading a sim node's HTTP API on 127.0.0.1.

use crate::error::{ToolError, ToolResult};
use serde_json::Value;
use std::time::Duration;

/// What a GET returned.
#[derive(Debug)]
pub enum ApiResponse {
    Found(Value),
    NotFound,
}

/// A client for the node APIs, one request at a time.
#[derive(Debug, Clone)]
pub struct NodeApi {
    client: reqwest::Client,
}

impl NodeApi {
    /// A client whose requests give up after `timeout`.
    pub fn new(timeout: Duration) -> ToolResult<Self> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|source| ToolError::Http {
                url: "client setup".to_string(),
                source,
            })?;
        Ok(Self { client })
    }

    /// GET `path` on the node API listening on `port`, parsed as JSON. A 404
    /// is [`ApiResponse::NotFound`]; any other non-success status, a
    /// connection failure or invalid JSON is an error.
    pub async fn get(&self, port: u32, path: &str) -> ToolResult<ApiResponse> {
        let url = format!("http://127.0.0.1:{port}{path}");
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|source| ToolError::Http {
                url: url.clone(),
                source,
            })?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(ApiResponse::NotFound);
        }
        let response = response
            .error_for_status()
            .map_err(|source| ToolError::Http {
                url: url.clone(),
                source,
            })?;
        let body = response.bytes().await.map_err(|source| ToolError::Http {
            url: url.clone(),
            source,
        })?;
        serde_json::from_slice(&body)
            .map(ApiResponse::Found)
            .map_err(|source| ToolError::Json {
                origin: url,
                source,
            })
    }

    /// The JSON body of a GET whatever its status, as `curl -s` returns it,
    /// or `None` when the node does not answer or the body is not JSON.
    pub async fn get_any_status(&self, port: u32, path: &str) -> Option<Value> {
        let url = format!("http://127.0.0.1:{port}{path}");
        let response = self.client.get(&url).send().await.ok()?;
        let body = response.bytes().await.ok()?;
        serde_json::from_slice(&body).ok()
    }

    /// GET `path`, treating a 404 as an error too.
    pub async fn get_found(&self, port: u32, path: &str) -> ToolResult<Value> {
        match self.get(port, path).await? {
            ApiResponse::Found(value) => Ok(value),
            ApiResponse::NotFound => Err(ToolError::message(format!(
                "http://127.0.0.1:{port}{path} returned 404"
            ))),
        }
    }
}
