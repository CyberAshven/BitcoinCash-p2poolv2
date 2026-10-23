// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::commands::api_client::ApiClient;
use p2poolv2_lib::config::ApiConfig;
use std::error::Error;

/// Execute the peers-info command by querying the running node's API.
pub async fn execute(api_config: &ApiConfig) -> Result<(), Box<dyn Error>> {
    let api_client = ApiClient::new(api_config);
    let response: serde_json::Value = api_client.get_json("/peers").await?;
    println!("{}", serde_json::to_string_pretty(&response)?);
    Ok(())
}

#[cfg(test)]
mod tests;
