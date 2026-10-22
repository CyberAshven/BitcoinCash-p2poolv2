// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Transaction query commands.

use super::TransactionsCommands;
use crate::commands::api_client::ApiClient;
use bitcoin::consensus::encode;
use p2poolv2_lib::config::ApiConfig;
use p2poolv2_lib::store::Store;
use serde::Serialize;
use std::error::Error;
use std::str::FromStr;

#[derive(Serialize)]
struct TransactionOutput {
    txid: String,
    version: i32,
    lock_time: String,
    inputs: Vec<InputOutput>,
    outputs: Vec<OutputOutput>,
}

#[derive(Serialize)]
struct InputOutput {
    previous_output: String,
    script_sig: String,
    sequence: u32,
}

#[derive(Serialize)]
struct OutputOutput {
    value: u64,
    script_pubkey: String,
}

#[derive(Serialize)]
struct RawTransactionOutput {
    txid: String,
    hex: String,
}

/// Execute a transactions subcommand via API.
pub async fn execute_api(
    api_config: &ApiConfig,
    command: &TransactionsCommands,
) -> Result<(), Box<dyn Error>> {
    match command {
        TransactionsCommands::Get { txid, raw } => {
            let api_client = ApiClient::new(api_config);
            let path = if *raw {
                format!("/transaction?txid={txid}&raw=true")
            } else {
                format!("/transaction?txid={txid}")
            };
            let response: serde_json::Value = api_client.get_json(&path).await?;
            println!("{}", serde_json::to_string_pretty(&response)?);
            Ok(())
        }
        TransactionsCommands::Submit { hex } => {
            let hex = match hex {
                Some(hex) => hex.trim().to_string(),
                None => read_stdin_line()?,
            };
            let response = submit(&ApiClient::new(api_config), &hex).await?;
            println!("{}", serde_json::to_string_pretty(&response)?);
            Ok(())
        }
    }
}

/// Hand a signed share transaction, in hex, to the node behind `api_client`
/// (`POST /share_transaction`), which checks, relays and mines it. Checks
/// the hex decodes to a transaction first, so a typo is reported here.
pub async fn submit(
    api_client: &ApiClient,
    hex: &str,
) -> Result<serde_json::Value, Box<dyn Error>> {
    let bytes = hex::decode(hex).map_err(|error| format!("Invalid transaction hex: {error}"))?;
    encode::deserialize::<bitcoin::Transaction>(&bytes)
        .map_err(|error| format!("Invalid transaction: {error}"))?;
    api_client
        .post_json("/share_transaction", &serde_json::json!({ "hex": hex }))
        .await
}

/// The transaction hex from stdin, trimmed.
fn read_stdin_line() -> Result<String, Box<dyn Error>> {
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

/// Execute a transactions subcommand via direct DB access.
pub fn execute_db(store: &Store, command: &TransactionsCommands) -> Result<(), Box<dyn Error>> {
    match command {
        TransactionsCommands::Submit { .. } => Err(
            "transactions submit hands a transaction to a running node: use --config, not --db-path"
                .into(),
        ),
        TransactionsCommands::Get { txid, raw } => {
            let txid =
                bitcoin::Txid::from_str(txid).map_err(|error| format!("Invalid txid: {error}"))?;
            let tx = store
                .get_tx(&txid)
                .map_err(|error| format!("Transaction not found: {error}"))?;

            if *raw {
                let raw_bytes = encode::serialize(&tx);
                let output = RawTransactionOutput {
                    txid: txid.to_string(),
                    hex: hex::encode(raw_bytes),
                };
                println!("{}", serde_json::to_string_pretty(&output)?);
            } else {
                let inputs: Vec<InputOutput> = tx
                    .input
                    .iter()
                    .map(|input| InputOutput {
                        previous_output: format!(
                            "{}:{}",
                            input.previous_output.txid, input.previous_output.vout
                        ),
                        script_sig: input.script_sig.to_hex_string(),
                        sequence: input.sequence.0,
                    })
                    .collect();

                let outputs: Vec<OutputOutput> = tx
                    .output
                    .iter()
                    .map(|output| OutputOutput {
                        value: output.value.to_sat(),
                        script_pubkey: output.script_pubkey.to_hex_string(),
                    })
                    .collect();

                let output = TransactionOutput {
                    txid: txid.to_string(),
                    version: tx.version.0,
                    lock_time: format!("{}", tx.lock_time),
                    inputs,
                    outputs,
                };
                println!("{}", serde_json::to_string_pretty(&output)?);
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::consensus::serialize;
    use bitcoin::hashes::Hash;
    use bitcoin::{
        Amount, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid, Witness, absolute,
        transaction,
    };
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn api_config(port: u16) -> ApiConfig {
        ApiConfig {
            hostname: "127.0.0.1".to_string(),
            port,
            auth_user: None,
            auth_token: None,
            auth_password: None,
            cors_allowed: false,
            public_dashboard: false,
            web_root: None,
        }
    }

    /// A one-input, one-output transaction; the node, not the CLI, checks
    /// its scripts.
    fn transaction_hex() -> String {
        let transaction = Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::new(Txid::from_byte_array([1; 32]), 0),
                script_sig: ScriptBuf::new(),
                sequence: Sequence::MAX,
                witness: Witness::new(),
            }],
            output: vec![TxOut {
                value: Amount::from_sat(1_000),
                script_pubkey: ScriptBuf::new(),
            }],
        };
        hex::encode(serialize(&transaction))
    }

    #[tokio::test]
    async fn submit_posts_the_transaction_hex_and_returns_the_node_answer() {
        let mock_server = MockServer::start().await;
        let hex = transaction_hex();
        Mock::given(method("POST"))
            .and(path("/share_transaction"))
            .and(body_json(serde_json::json!({ "hex": hex })))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "txid": "ab", "status": "accepted" })),
            )
            .expect(1)
            .mount(&mock_server)
            .await;
        let api_client = ApiClient::new(&api_config(mock_server.address().port()));

        let response = submit(&api_client, &hex).await.unwrap();

        assert_eq!(response["status"], "accepted");
    }

    #[tokio::test]
    async fn submit_refuses_hex_that_is_not_a_transaction_without_calling_the_node() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&mock_server)
            .await;
        let api_client = ApiClient::new(&api_config(mock_server.address().port()));

        let error = submit(&api_client, "0102").await.unwrap_err();

        assert!(error.to_string().contains("Invalid transaction"));
    }

    #[tokio::test]
    async fn submit_reports_the_rule_a_node_refused_the_transaction_for() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/share_transaction"))
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "Share transaction refused: an input is already spent on the confirmed share chain"
            })))
            .mount(&mock_server)
            .await;
        let api_client = ApiClient::new(&api_config(mock_server.address().port()));

        let error = submit(&api_client, &transaction_hex()).await.unwrap_err();

        assert!(error.to_string().contains("already spent"));
    }
}
