// SPDX-FileCopyrightText: 2024 PDM Authors
// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What this tool takes from the node's config file: where the API and
//! BCHN are, their logins, and the Bitcoin Cash network.
//!
//! As in PDM, one file points the tool at everything. It is read with
//! `p2poolv2_config::Config::load`, so `P2POOL__` environment overrides
//! apply here as they do in the node. The network is the node config's
//! `stratum.bitcoin_cash_network`: mainnet and chipnet differ in nothing
//! else here.

use crate::format;
use crate::network::CashNetwork;
use p2poolv2_config::{BitcoinCashNetwork, Chain, Config};
use std::fmt;
use std::path::Path;

/// Where the API and BCHN are, from one node config.
#[derive(Clone)]
pub struct NodeConnection {
    pub network: CashNetwork,
    /// E.g. `http://127.0.0.1:46884`.
    pub api_base: String,
    /// `[api] auth_user` and `auth_password`, when both are set.
    pub api_credentials: Option<(String, String)>,
    /// `[bitcoinrpc] url`.
    pub bchn_url: String,
    pub bchn_username: String,
    pub bchn_password: String,
}

impl fmt::Debug for NodeConnection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NodeConnection")
            .field("network", &self.network)
            .field("api_base", &self.api_base)
            .field(
                "api_credentials",
                &self
                    .api_credentials
                    .as_ref()
                    .map(|(user, _)| (user, "[redacted]")),
            )
            .field("bchn_url", &self.bchn_url)
            .field("bchn_username", &self.bchn_username)
            .field("bchn_password", &"[redacted]")
            .finish()
    }
}

/// Reads the node config at `path`.
///
/// Only the first line of a parse error is kept, without quoted strings:
/// the lines after it quote the file and a type error quotes the value,
/// and either could be a password.
pub fn load(path: &Path) -> Result<NodeConnection, String> {
    let path_text = path
        .to_str()
        .ok_or_else(|| format!("{} is not a UTF-8 path", path.display()))?;
    let config = Config::load(path_text).map_err(|error| {
        let message = error.to_string();
        let first_line = format::without_quoted(message.lines().next().unwrap_or_default());
        format!("Failed to read {}: {first_line}", path.display())
    })?;
    from_config(&config)
}

/// What a loaded node config points at. A Bitcoin config, or a Bitcoin
/// Cash one without its network, is refused.
pub fn from_config(config: &Config) -> Result<NodeConnection, String> {
    if config.stratum.chain != Chain::BitcoinCash {
        return Err(
            "This node config is for Bitcoin; this tool is for Bitcoin Cash. Set \
             stratum.chain = \"bitcoin-cash\"."
                .to_string(),
        );
    }
    let network = config
        .stratum
        .bitcoin_cash_network
        .map(cash_network)
        .ok_or_else(|| {
            "stratum.bitcoin_cash_network is not set; set it to mainnet, chipnet or regtest."
                .to_string()
        })?;
    let api_credentials = match (&config.api.auth_user, &config.api.auth_password) {
        (Some(user), Some(password)) if !user.is_empty() && !password.is_empty() => {
            Some((user.clone(), password.clone()))
        }
        _ => None,
    };
    Ok(NodeConnection {
        network,
        api_base: format!(
            "http://{}:{}",
            api_host(&config.api.hostname),
            config.api.port
        ),
        api_credentials,
        bchn_url: config.bitcoinrpc.url.clone(),
        bchn_username: config.bitcoinrpc.username.clone(),
        bchn_password: config.bitcoinrpc.password.clone(),
    })
}

/// The address to reach an API listening on `hostname`: a node bound to
/// every interface is reached on loopback.
fn api_host(hostname: &str) -> &str {
    match hostname {
        "" | "0.0.0.0" => "127.0.0.1",
        hostname => hostname,
    }
}

fn cash_network(network: BitcoinCashNetwork) -> CashNetwork {
    match network {
        BitcoinCashNetwork::Mainnet => CashNetwork::Mainnet,
        BitcoinCashNetwork::Chipnet => CashNetwork::Chipnet,
        BitcoinCashNetwork::Regtest => CashNetwork::Regtest,
        BitcoinCashNetwork::Scalenet => CashNetwork::Scalenet,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{BCH_CHIPNET_CONFIG, write_config};

    #[test]
    fn test_load_reads_the_api_bchn_and_network() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(dir.path(), BCH_CHIPNET_CONFIG);
        let connection = load(&path).unwrap();
        assert_eq!(connection.network, CashNetwork::Chipnet);
        assert_eq!(connection.api_base, "http://127.0.0.1:46884");
        assert_eq!(connection.api_credentials, None);
        assert_eq!(connection.bchn_url, "http://127.0.0.1:48332");
        assert_eq!(connection.bchn_username, "p2pool");
    }

    #[test]
    fn test_load_reads_the_api_login() {
        let dir = tempfile::tempdir().unwrap();
        let text = BCH_CHIPNET_CONFIG.replace(
            "port = 46884",
            "port = 46884\nauth_user = \"admin\"\nauth_password = \"pw\"",
        );
        let path = write_config(dir.path(), &text);
        let connection = load(&path).unwrap();
        assert_eq!(
            connection.api_credentials,
            Some(("admin".into(), "pw".into()))
        );
    }

    #[test]
    fn test_load_reaches_an_api_bound_to_every_interface_on_loopback() {
        let dir = tempfile::tempdir().unwrap();
        let text = BCH_CHIPNET_CONFIG.replace(
            "hostname = \"127.0.0.1\"\nport = 46884",
            "hostname = \"0.0.0.0\"\nport = 46884",
        );
        let path = write_config(dir.path(), &text);
        assert_eq!(load(&path).unwrap().api_base, "http://127.0.0.1:46884");
    }

    #[test]
    fn test_load_refuses_a_bitcoin_config() {
        let dir = tempfile::tempdir().unwrap();
        let text = BCH_CHIPNET_CONFIG
            .replace("chain = \"bitcoin-cash\"\n", "")
            .replace("bitcoin_cash_network = \"chipnet\"\n", "");
        let path = write_config(dir.path(), &text);
        assert!(load(&path).unwrap_err().contains("is for Bitcoin"));
    }

    #[test]
    fn test_load_refuses_a_config_without_a_network() {
        let dir = tempfile::tempdir().unwrap();
        let text = BCH_CHIPNET_CONFIG.replace("bitcoin_cash_network = \"chipnet\"\n", "");
        let path = write_config(dir.path(), &text);
        assert!(
            load(&path)
                .unwrap_err()
                .contains("bitcoin_cash_network is not set")
        );
    }

    #[test]
    fn test_load_does_not_quote_the_file_in_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let text = BCH_CHIPNET_CONFIG.replace(
            "password = \"chipnet-rpc-secret\"",
            "password = \"chipnet-rpc-secret",
        );
        let path = write_config(dir.path(), &text);
        let error = load(&path).unwrap_err();
        assert!(!error.contains("chipnet-rpc-secret"), "{error}");
    }

    #[test]
    fn test_debug_hides_the_passwords() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(dir.path(), BCH_CHIPNET_CONFIG);
        let shown = format!("{:?}", load(&path).unwrap());
        assert!(!shown.contains("chipnet-rpc-secret"), "{shown}");
        assert!(shown.contains("bchn_password: \"[redacted]\""), "{shown}");
    }
}
