// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use p2poolv2_lib::auth::build_basic_auth_header;
use p2poolv2_lib::config::ApiConfig;
use std::error::Error;

/// HTTP client wrapper for calling the P2Pool API with optional Basic auth.
pub struct ApiClient {
    client: reqwest::Client,
    base_url: String,
    auth_header: Option<String>,
}

impl ApiClient {
    /// Create a new API client from the API configuration.
    pub fn new(api_config: &ApiConfig) -> Self {
        let base_url = format!("http://{}:{}", api_config.hostname, api_config.port);
        let auth_header = match (&api_config.auth_user, &api_config.auth_password) {
            (Some(username), Some(password)) => Some(build_basic_auth_header(username, password)),
            _ => None,
        };
        ApiClient {
            client: reqwest::Client::new(),
            base_url,
            auth_header,
        }
    }

    /// Perform an authenticated GET request to the given path.
    ///
    /// Returns the response body as a string on success. Returns an error if
    /// the server is unreachable, the response status is not successful, or the
    /// body cannot be read.
    pub async fn get(&self, path: &str) -> Result<String, Box<dyn Error>> {
        let url = format!("{}{}", self.base_url, path);
        let mut request = self.client.get(&url);

        if let Some(header_value) = &self.auth_header {
            request = request.header("Authorization", header_value.clone());
        }

        let response = request.send().await.map_err(|error| {
            format!("Failed to connect to API at {url}: {error}. Is the node running?")
        })?;

        if !response.status().is_success() {
            return Err(format!(
                "API returned status {}: {}",
                response.status(),
                response.text().await.unwrap_or_default()
            )
            .into());
        }

        Ok(response.text().await?)
    }

    /// Perform an authenticated GET request and deserialize the JSON response.
    pub async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
    ) -> Result<T, Box<dyn Error>> {
        let body = self.get(path).await?;
        let parsed: T = serde_json::from_str(&body)?;
        Ok(parsed)
    }

    /// Perform an authenticated POST request with a JSON body and
    /// return the parsed JSON response.
    pub async fn post_json(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, Box<dyn Error>> {
        let response = self.send_json(reqwest::Method::POST, path, body).await?;
        Ok(serde_json::from_str(&response)?)
    }

    /// Perform an authenticated DELETE request with a JSON body and
    /// return the parsed JSON response.
    pub async fn delete_json(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, Box<dyn Error>> {
        let response = self.send_json(reqwest::Method::DELETE, path, body).await?;
        Ok(serde_json::from_str(&response)?)
    }

    /// Send a request with a JSON body using the given HTTP method.
    async fn send_json(
        &self,
        method: reqwest::Method,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<String, Box<dyn Error>> {
        let url = format!("{}{}", self.base_url, path);
        let mut request = self.client.request(method, &url).json(body);

        if let Some(header_value) = &self.auth_header {
            request = request.header("Authorization", header_value.clone());
        }

        let response = request.send().await.map_err(|error| {
            format!("Failed to connect to API at {url}: {error}. Is the node running?")
        })?;

        if !response.status().is_success() {
            return Err(format!(
                "API returned status {}: {}",
                response.status(),
                response.text().await.unwrap_or_default()
            )
            .into());
        }

        Ok(response.text().await?)
    }
}

#[cfg(test)]
mod tests;
