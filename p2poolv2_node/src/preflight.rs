// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use crate::signal::ShutdownReason;
use bitcoindrpc::{BitcoinRpcConfig, BitcoindRpcClient, BitcoindRpcError};
use p2poolv2_lib::config::BitcoinCashNetwork;
use std::time::Duration;
use tokio::sync::watch;
use tracing::{info, warn};

/// `RPC_INVALID_PARAMETER`, BCHN's answer to `getblockhash` for a height
/// past its chain's tip.
const RPC_INVALID_PARAMETER: i32 = -8;

#[derive(Debug)]
pub enum PreflightError {
    NotSynced,
    Rpc(Box<dyn std::error::Error + Send + Sync>),
    ShutdownRequested,
    /// The node in `[bitcoinrpc]` is not BCHN on the configured Bitcoin Cash
    /// network. The text says what was found and what to do.
    NotBitcoinCash(String),
}

impl std::fmt::Display for PreflightError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PreflightError::NotSynced => write!(f, "Bitcoin node still in initial block download"),
            PreflightError::Rpc(e) => write!(f, "{e}"),
            PreflightError::ShutdownRequested => write!(
                f,
                "shutdown requested while waiting for Bitcoin node to sync"
            ),
            PreflightError::NotBitcoinCash(reason) => write!(f, "{reason}"),
        }
    }
}

impl std::error::Error for PreflightError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            PreflightError::Rpc(e) => Some(e.as_ref()),
            PreflightError::NotSynced => None,
            PreflightError::ShutdownRequested => None,
            PreflightError::NotBitcoinCash(_) => None,
        }
    }
}

async fn check_synced(bitcoind: &BitcoindRpcClient) -> Result<(), PreflightError> {
    let is_in_ibd = bitcoind
        .getblockchaininfo()
        .await
        .map_err(|e| PreflightError::Rpc(e.into()))?
        .initial_block_download;

    if is_in_ibd {
        return Err(PreflightError::NotSynced);
    }

    Ok(())
}

/// The network name as `bitcoin_cash_network` spells it in the config.
fn config_name(network: BitcoinCashNetwork) -> &'static str {
    match network {
        BitcoinCashNetwork::Mainnet => "mainnet",
        BitcoinCashNetwork::Chipnet => "chipnet",
        BitcoinCashNetwork::Regtest => "regtest",
    }
}

fn rpc_client(bitcoinrpc_config: &BitcoinRpcConfig) -> Result<BitcoindRpcClient, PreflightError> {
    BitcoindRpcClient::new(
        &bitcoinrpc_config.url,
        &bitcoinrpc_config.username,
        &bitcoinrpc_config.password,
    )
    .map_err(|e| PreflightError::Rpc(e.into()))
}

/// Refuse a node whose `getblockchaininfo` chain is not BCHN's name for
/// `network` (`main`, `chip` or `regtest`).
///
/// Run before waiting for the node to sync, so a node on the wrong network
/// is refused at once instead of after its initial block download. A
/// Bitcoin, Bitcoin SV or eCash node can report the same name as BCHN
/// (`main`, `regtest`), so [`check_bitcoin_cash_block`] runs after the sync.
async fn check_bitcoin_cash_chain_name(
    bitcoind: &BitcoindRpcClient,
    network: BitcoinCashNetwork,
) -> Result<(), PreflightError> {
    let info = bitcoind
        .getblockchaininfo()
        .await
        .map_err(|e| PreflightError::Rpc(e.into()))?;
    let expected = network.bchn_chain_name();
    let name = config_name(network);
    match info.chain.as_deref() {
        Some(chain) if chain == expected => Ok(()),
        Some(chain) => Err(PreflightError::NotBitcoinCash(format!(
            "The node in [bitcoinrpc] is on chain \"{chain}\", but bitcoin_cash_network = \
             \"{name}\" needs a BCHN node on chain \"{expected}\". Point [bitcoinrpc] at a BCHN \
             node on Bitcoin Cash {name}, or set bitcoin_cash_network to the node's network."
        ))),
        None => Err(PreflightError::NotBitcoinCash(
            "The node in [bitcoinrpc] reports no chain in getblockchaininfo. P2Pool on \
             Bitcoin Cash needs BCHN."
                .to_string(),
        )),
    }
}

/// Refuse a node that lacks Bitcoin Cash's own block on `network`
/// ([`BitcoinCashNetwork::bch_only_block`]): a Bitcoin, Bitcoin SV or eCash
/// node, or a BCHN node on another test network. Run once the node has
/// synced, so it has the block. Regtest has no such block and is not checked.
async fn check_bitcoin_cash_block(
    bitcoind: &BitcoindRpcClient,
    network: BitcoinCashNetwork,
) -> Result<(), PreflightError> {
    let Some((height, expected)) = network.bch_only_block() else {
        info!("Regtest has no Bitcoin Cash only block to check the node against");
        return Ok(());
    };
    let name = config_name(network);
    match bitcoind.get_block_hash(height).await {
        Ok(hash) if hash == expected => {
            info!("The node is on Bitcoin Cash {name}: its block {height} is {expected}");
            Ok(())
        }
        Ok(hash) => Err(PreflightError::NotBitcoinCash(format!(
            "The node in [bitcoinrpc] has block {hash} at height {height}, but Bitcoin Cash \
             {name} has {expected}. The node follows another chain, such as Bitcoin, Bitcoin \
             SV or eCash. P2Pool will not mine for it: point [bitcoinrpc] at a BCHN node on \
             Bitcoin Cash {name}."
        ))),
        Err(BitcoindRpcError::RpcError {
            code: RPC_INVALID_PARAMETER,
            message,
        }) => Err(PreflightError::NotBitcoinCash(format!(
            "The node in [bitcoinrpc] has no block at height {height} ({message}), where \
             Bitcoin Cash {name} has {expected}, although it reports that it has synced. It is \
             not on Bitcoin Cash {name}: point [bitcoinrpc] at a BCHN node on that network."
        ))),
        Err(error) => Err(PreflightError::Rpc(error.into())),
    }
}

/// Check, before waiting for the node to sync, that the node in
/// `[bitcoinrpc]` reports BCHN's chain name for `network`.
pub async fn verify_bitcoin_cash_chain_name(
    bitcoinrpc_config: &BitcoinRpcConfig,
    network: BitcoinCashNetwork,
) -> Result<(), PreflightError> {
    check_bitcoin_cash_chain_name(&rpc_client(bitcoinrpc_config)?, network).await
}

/// Check, once the node has synced, that the node in `[bitcoinrpc]` has
/// Bitcoin Cash's own block for `network`.
pub async fn verify_bitcoin_cash_block(
    bitcoinrpc_config: &BitcoinRpcConfig,
    network: BitcoinCashNetwork,
) -> Result<(), PreflightError> {
    check_bitcoin_cash_block(&rpc_client(bitcoinrpc_config)?, network).await
}

/// Waits for the Bitcoin node to finish initial block download, checking
/// periodically and logging progress, instead of failing immediately.
///
/// The RPC client is built once and reused across polls. Any error other
/// than "still syncing" is returned right away. If a shutdown is signalled
/// via `shutdown_rx` while waiting, this returns early with
/// `PreflightError::ShutdownRequested` instead of sleeping out the full
/// interval.
pub async fn wait_for_bitcoin_node_synced(
    bitcoinrpc_config: &BitcoinRpcConfig,
    poll_interval: Duration,
    mut shutdown_rx: watch::Receiver<ShutdownReason>,
) -> Result<(), PreflightError> {
    let bitcoind = BitcoindRpcClient::new(
        &bitcoinrpc_config.url,
        &bitcoinrpc_config.username,
        &bitcoinrpc_config.password,
    )
    .map_err(|e| PreflightError::Rpc(e.into()))?;

    loop {
        match check_synced(&bitcoind).await {
            Ok(()) => return Ok(()),
            Err(PreflightError::NotSynced) => {
                warn!(
                    "Bitcoin node still syncing, checking again in {}s...",
                    poll_interval.as_secs()
                );
                tokio::select! {
                    _ = tokio::time::sleep(poll_interval) => {},
                    _ = shutdown_rx.changed() => {
                        return Err(PreflightError::ShutdownRequested);
                    }
                }
            }
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_partial_json, header, method, path},
    };

    async fn setup_mock_bitcoin_rpc() -> (MockServer, BitcoinRpcConfig) {
        let mock_server = MockServer::start().await;
        let config = BitcoinRpcConfig {
            url: mock_server.uri(),
            username: "testuser".to_string(),
            password: "testpass".to_string(),
        };
        (mock_server, config)
    }

    fn test_auth_header() -> String {
        format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD
                .encode(format!("{}:{}", "testuser", "testpass"))
        )
    }

    #[tokio::test]
    async fn check_synced_returns_ok_when_not_in_ibd() {
        let (mock_server, bitcoinrpc_config) = setup_mock_bitcoin_rpc().await;

        Mock::given(method("POST"))
            .and(path("/"))
            .and(header("Authorization", test_auth_header().as_str()))
            .and(body_partial_json(serde_json::json!({
                "method": "getblockchaininfo",
                "params": [],
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "result": {
                    "initialblockdownload": false,
                },
                "error": null,
                "id": 0
            })))
            .mount(&mock_server)
            .await;

        let bitcoind = BitcoindRpcClient::new(
            &bitcoinrpc_config.url,
            &bitcoinrpc_config.username,
            &bitcoinrpc_config.password,
        )
        .unwrap();
        let result = check_synced(&bitcoind).await;
        assert!(result.is_ok(), "check_synced returned an error");
    }

    #[tokio::test]
    async fn check_synced_returns_err_when_in_ibd() {
        let (mock_server, bitcoinrpc_config) = setup_mock_bitcoin_rpc().await;

        Mock::given(method("POST"))
            .and(path("/"))
            .and(header("Authorization", test_auth_header().as_str()))
            .and(body_partial_json(serde_json::json!({
                "method": "getblockchaininfo",
                "params": [],
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "result": {
                    "initialblockdownload": true,
                },
                "error": null,
                "id": 0
            })))
            .mount(&mock_server)
            .await;

        let bitcoind = BitcoindRpcClient::new(
            &bitcoinrpc_config.url,
            &bitcoinrpc_config.username,
            &bitcoinrpc_config.password,
        )
        .unwrap();
        let result = check_synced(&bitcoind).await;
        assert!(
            result.is_err(),
            "check_synced should return error when in IBD"
        );
    }

    #[tokio::test]
    async fn wait_for_bitcoin_node_synced_retries_until_synced() {
        let (mock_server, bitcoinrpc_config) = setup_mock_bitcoin_rpc().await;

        // First two checks: still syncing
        Mock::given(method("POST"))
            .and(path("/"))
            .and(header("Authorization", test_auth_header().as_str()))
            .and(body_partial_json(serde_json::json!({
                "method": "getblockchaininfo",
                "params": [],
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "result": {
                    "initialblockdownload": true,
                },
                "error": null,
                "id": 0
            })))
            .up_to_n_times(2)
            .with_priority(1)
            .mount(&mock_server)
            .await;

        // After that: synced
        Mock::given(method("POST"))
            .and(path("/"))
            .and(header("Authorization", test_auth_header().as_str()))
            .and(body_partial_json(serde_json::json!({
                "method": "getblockchaininfo",
                "params": [],
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "result": {
                    "initialblockdownload": false,
                },
                "error": null,
                "id": 0
            })))
            .with_priority(2)
            .mount(&mock_server)
            .await;

        let (_shutdown_tx, shutdown_rx) = watch::channel(ShutdownReason::None);
        let result = wait_for_bitcoin_node_synced(
            &bitcoinrpc_config,
            Duration::from_millis(10),
            shutdown_rx,
        )
        .await;
        assert!(
            result.is_ok(),
            "wait_for_bitcoin_node_synced should eventually succeed once the node is synced"
        );
    }

    /// A made-up hash that is not Bitcoin Cash's block 661,648. It stands
    /// for what a Bitcoin, Bitcoin SV or eCash node answers at that height.
    const OTHER_CHAIN_BLOCK_HASH: &str =
        "0000000000000000000a9f7f4a8b4b5a0d32d5e6a4c6a1d1c1b2a2e2f3a4b5c6";

    #[tokio::test]
    async fn a_bchn_chipnet_node_passes_the_chain_name_check() {
        let (mock_server, bitcoinrpc_config) = setup_mock_bitcoin_rpc().await;
        Mock::given(method("POST"))
            .and(body_partial_json(
                serde_json::json!({"method": "getblockchaininfo"}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "result": {"chain": "chip", "initialblockdownload": true},
                "error": null,
                "id": 0
            })))
            .mount(&mock_server)
            .await;

        let result =
            verify_bitcoin_cash_chain_name(&bitcoinrpc_config, BitcoinCashNetwork::Chipnet).await;

        assert!(result.is_ok(), "{result:?}");
    }

    /// BCHN on testnet4 uses chipnet's genesis and CashAddr prefix, so only
    /// the chain name tells them apart before sync.
    #[tokio::test]
    async fn a_testnet4_node_is_refused_for_chipnet() {
        let (mock_server, bitcoinrpc_config) = setup_mock_bitcoin_rpc().await;
        Mock::given(method("POST"))
            .and(body_partial_json(
                serde_json::json!({"method": "getblockchaininfo"}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "result": {"chain": "test4", "initialblockdownload": false},
                "error": null,
                "id": 0
            })))
            .mount(&mock_server)
            .await;

        let result =
            verify_bitcoin_cash_chain_name(&bitcoinrpc_config, BitcoinCashNetwork::Chipnet).await;

        let Err(PreflightError::NotBitcoinCash(reason)) = result else {
            panic!("a testnet4 node was not refused: {result:?}");
        };
        assert!(reason.contains("\"test4\""), "{reason}");
        assert!(reason.contains("\"chip\""), "{reason}");
    }

    #[tokio::test]
    async fn a_node_reporting_no_chain_is_refused() {
        let (mock_server, bitcoinrpc_config) = setup_mock_bitcoin_rpc().await;
        Mock::given(method("POST"))
            .and(body_partial_json(
                serde_json::json!({"method": "getblockchaininfo"}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "result": {"initialblockdownload": false},
                "error": null,
                "id": 0
            })))
            .mount(&mock_server)
            .await;

        let result =
            verify_bitcoin_cash_chain_name(&bitcoinrpc_config, BitcoinCashNetwork::Mainnet).await;

        assert!(matches!(result, Err(PreflightError::NotBitcoinCash(_))));
    }

    #[tokio::test]
    async fn a_bchn_mainnet_node_has_the_bitcoin_cash_block() {
        let (mock_server, bitcoinrpc_config) = setup_mock_bitcoin_rpc().await;
        Mock::given(method("POST"))
            .and(body_partial_json(serde_json::json!({
                "method": "getblockhash",
                "params": [661648],
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "result": "0000000000000000029e471c41818d24b8b74c911071c4ef0b4a0509f9b5a8ce",
                "error": null,
                "id": 0
            })))
            .expect(1)
            .mount(&mock_server)
            .await;

        let result =
            verify_bitcoin_cash_block(&bitcoinrpc_config, BitcoinCashNetwork::Mainnet).await;

        assert!(result.is_ok(), "{result:?}");
    }

    /// A Bitcoin, Bitcoin SV or eCash node also says chain "main"; its block
    /// at 661,648 gives it away.
    #[tokio::test]
    async fn a_node_on_another_chain_is_refused_by_its_block() {
        let (mock_server, bitcoinrpc_config) = setup_mock_bitcoin_rpc().await;
        Mock::given(method("POST"))
            .and(body_partial_json(
                serde_json::json!({"method": "getblockhash"}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "result": OTHER_CHAIN_BLOCK_HASH,
                "error": null,
                "id": 0
            })))
            .mount(&mock_server)
            .await;

        let result =
            verify_bitcoin_cash_block(&bitcoinrpc_config, BitcoinCashNetwork::Mainnet).await;

        let Err(PreflightError::NotBitcoinCash(reason)) = result else {
            panic!("a node on another chain was not refused: {result:?}");
        };
        assert!(reason.contains(OTHER_CHAIN_BLOCK_HASH), "{reason}");
        assert!(reason.contains("eCash"), "{reason}");
    }

    #[tokio::test]
    async fn a_node_without_the_block_height_is_refused() {
        let (mock_server, bitcoinrpc_config) = setup_mock_bitcoin_rpc().await;
        Mock::given(method("POST"))
            .and(body_partial_json(
                serde_json::json!({"method": "getblockhash"}),
            ))
            .respond_with(ResponseTemplate::new(500).set_body_json(serde_json::json!({
                "result": null,
                "error": {"code": -8, "message": "Block height out of range"},
                "id": 0
            })))
            .mount(&mock_server)
            .await;

        let result =
            verify_bitcoin_cash_block(&bitcoinrpc_config, BitcoinCashNetwork::Chipnet).await;

        let Err(PreflightError::NotBitcoinCash(reason)) = result else {
            panic!("a node without block 115252 was not refused: {result:?}");
        };
        assert!(reason.contains("115252"), "{reason}");
    }

    /// Regtest blocks are local, so there is no Bitcoin Cash only block to
    /// ask for.
    #[tokio::test]
    async fn regtest_is_not_asked_for_a_block() {
        let (mock_server, bitcoinrpc_config) = setup_mock_bitcoin_rpc().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&mock_server)
            .await;

        let result =
            verify_bitcoin_cash_block(&bitcoinrpc_config, BitcoinCashNetwork::Regtest).await;

        assert!(result.is_ok(), "{result:?}");
    }
}
