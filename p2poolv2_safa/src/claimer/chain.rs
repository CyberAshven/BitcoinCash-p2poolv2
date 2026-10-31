// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Follow the selected mutable NFT through claims and market transactions.

use bitcoindrpc::{BitcoindRpcClient, BitcoindRpcError};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::config::VaultPoint;
use super::files::Journal;
use crate::{Covenant, Vault, decode_state, encode_state, from_hex};

/// The current vault and verified transaction lineage observed during this pass.
#[derive(Debug)]
pub struct Followed {
    pub vault: Option<Vault>,
    pub transactions: Vec<String>,
}

pub(super) trait ChainRpc {
    async fn call(&self, method: &str, parameters: Vec<Value>) -> Result<Value, BitcoindRpcError>;
}

impl ChainRpc for BitcoindRpcClient {
    async fn call(&self, method: &str, parameters: Vec<Value>) -> Result<Value, BitcoindRpcError> {
        self.request(method, parameters).await
    }
}

/// Read the common output fields returned by `gettxout` and verbose transactions.
/// Category identifiers here, unlike the wins log, are in display order.
pub fn parse_vault_output(
    output: &Value,
    covenant: &Covenant,
    category_display: &str,
    expected_thread: Option<u32>,
) -> Result<Vault, String> {
    let category = from_hex(category_display, Some(32), "deployment category")?;
    let token = &output["tokenData"];
    let actual_category = from_hex(
        token["category"]
            .as_str()
            .ok_or("vault has no token category")?,
        Some(32),
        "vault category",
    )?;
    if actual_category != category {
        return Err("vault category does not match the deployment".into());
    }
    if token["nft"]["capability"] != "mutable" {
        return Err("vault must contain a mutable NFT".into());
    }
    let locking_bytecode = from_hex(
        output["scriptPubKey"]["hex"]
            .as_str()
            .ok_or("vault has no locking bytecode")?,
        None,
        "vault locking bytecode",
    )?;
    if locking_bytecode != covenant.locking_bytecode {
        return Err("vault locking bytecode does not match the covenant".into());
    }
    let commitment = from_hex(
        token["nft"]["commitment"]
            .as_str()
            .ok_or("vault has no NFT commitment")?,
        Some(16),
        "vault commitment",
    )?;
    let state = decode_state(&commitment)?;
    encode_state(&state)?;
    if state.thread >= covenant.threads
        || expected_thread.is_some_and(|thread| thread != state.thread)
    {
        return Err("vault moved to a different or invalid thread".into());
    }
    let amount = token["amount"]
        .as_str()
        .filter(|amount| !amount.is_empty() && amount.bytes().all(|byte| byte.is_ascii_digit()))
        .ok_or("vault token amount must be decimal base units")?
        .parse::<u64>()
        .map_err(|_| "vault token amount is out of range")?;
    let Value::Number(value) = &output["value"] else {
        return Err("vault value must be a BCH number".into());
    };
    Ok(Vault {
        category,
        value: decimal_satoshis(&value.to_string())?,
        token_amount: amount,
        commitment,
    })
}

/// Convert the decimal RPC amount without rounding a floating point product.
fn decimal_satoshis(text: &str) -> Result<u64, String> {
    let invalid = || "vault BCH value must be an exact non-negative satoshi amount".to_string();
    let (mantissa, exponent) = match text.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, exponent.parse::<i32>().map_err(|_| invalid())?),
        None => (text, 0),
    };
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(invalid());
    }
    let digits = format!("{whole}{fraction}")
        .parse::<u128>()
        .map_err(|_| invalid())?;
    let fractional_digits = i32::try_from(fraction.len()).map_err(|_| invalid())?;
    let scale = 8_i32
        .checked_add(exponent)
        .and_then(|scale| scale.checked_sub(fractional_digits))
        .ok_or_else(invalid)?;
    let factor = 10_u128
        .checked_pow(scale.unsigned_abs())
        .ok_or_else(invalid)?;
    let satoshis = if scale >= 0 {
        digits.checked_mul(factor).ok_or_else(invalid)?
    } else if digits.is_multiple_of(factor) {
        digits / factor
    } else {
        return Err(invalid());
    };
    u64::try_from(satoshis).map_err(|_| invalid())
}

fn rpc_error(method: &str, error: &BitcoindRpcError) -> String {
    match error {
        BitcoindRpcError::RpcError { code, .. } => format!("{method} failed with RPC code {code}"),
        _ => format!("{method} failed; check the node and RPC connection"),
    }
}

async fn request<T: DeserializeOwned>(
    client: &impl ChainRpc,
    method: &str,
    parameters: Vec<Value>,
) -> Result<T, String> {
    let value = client
        .call(method, parameters)
        .await
        .map_err(|error| rpc_error(method, &error))?;
    serde_json::from_value(value).map_err(|_| format!("{method} returned an invalid result"))
}

fn transaction_spends(transaction: &Value, point: &VaultPoint) -> Result<bool, String> {
    let inputs = transaction["vin"]
        .as_array()
        .ok_or("verbose transaction has no inputs")?;
    Ok(inputs.iter().any(|input| {
        input["txid"]
            .as_str()
            .is_some_and(|txid| txid.eq_ignore_ascii_case(&point.txid))
            && input["vout"].as_u64() == Some(u64::from(point.vout))
    }))
}

fn transaction_point(transaction: &Value) -> Result<VaultPoint, String> {
    let txid = transaction["txid"]
        .as_str()
        .ok_or("verbose transaction has no txid")?;
    from_hex(txid, Some(32), "transaction txid")?;
    Ok(VaultPoint {
        txid: txid.to_ascii_lowercase(),
        vout: 0,
    })
}

fn record_vault(journal: &mut Journal, vault: &Vault) -> Result<(), String> {
    journal.thread = Some(decode_state(&vault.commitment)?.thread);
    journal.commitment = Some(hex::encode(&vault.commitment));
    Ok(())
}

async fn anchor_scan(
    client: &impl ChainRpc,
    journal: &mut Journal,
    height: u32,
) -> Result<(), String> {
    let hash: String = request(client, "getblockhash", vec![json!(height)]).await?;
    journal.scan_height = Some(height);
    journal.scan_hash = Some(hash);
    Ok(())
}

async fn check_scan_anchor(
    client: &impl ChainRpc,
    journal: &Journal,
    tip: u32,
) -> Result<(), String> {
    if let Some(height) = journal.scan_height {
        if height > tip {
            return Err("chain tip precedes the journal scan; recover the journal after reviewing the reorg".into());
        }
        if let Some(expected_hash) = journal.scan_hash.as_ref() {
            let current_hash: String = request(client, "getblockhash", vec![json!(height)]).await?;
            if &current_hash != expected_hash {
                return Err("chain scan anchor changed; recover the journal from the configured starting vault after reviewing the reorg".into());
            }
        }
    }
    Ok(())
}

/// Pin the thread and creation height before following an already spent start.
/// Historical verbose lookup requires BCHN's transaction index.
async fn initialize_spent_vault(
    client: &impl ChainRpc,
    covenant: &Covenant,
    category_display: &str,
    journal: &mut Journal,
    tip: u32,
) -> Result<(), String> {
    let transaction: Value = client
        .call("getrawtransaction", vec![json!(journal.vault.txid), json!(true)])
        .await
        .map_err(|_| "cannot read the spent starting vault; enable BCHN txindex or use a journal captured while the vault was unspent".to_string())?;
    let actual_point = transaction_point(&transaction)?;
    if !actual_point.txid.eq_ignore_ascii_case(&journal.vault.txid) {
        return Err("starting vault transaction identifier does not match".into());
    }
    let output = transaction["vout"]
        .as_array()
        .and_then(|outputs| outputs.get(journal.vault.vout as usize))
        .ok_or("starting vault output is absent")?;
    let vault = parse_vault_output(output, covenant, category_display, journal.thread)?;
    record_vault(journal, &vault)?;
    if let Some(block_hash) = transaction["blockhash"].as_str() {
        let header: Value = request(
            client,
            "getblockheader",
            vec![json!(block_hash), json!(true)],
        )
        .await?;
        let height = header["height"]
            .as_u64()
            .and_then(|height| u32::try_from(height).ok())
            .ok_or("starting vault block has no valid height")?;
        if height > tip {
            return Err("starting vault block is above the current chain tip".into());
        }
        let active_hash: String = request(client, "getblockhash", vec![json!(height)]).await?;
        if active_hash != block_hash {
            return Err("starting vault transaction is outside the active chain".into());
        }
        journal.scan_height = Some(height);
        journal.scan_hash = Some(active_hash);
    } else {
        anchor_scan(client, journal, tip).await?;
    }
    Ok(())
}

async fn mempool_spender(
    client: &impl ChainRpc,
    point: &VaultPoint,
) -> Result<Option<Value>, String> {
    let transactions: Vec<String> = request(client, "getrawmempool", Vec::with_capacity(0)).await?;
    for txid in transactions {
        let transaction = client
            .call("getrawtransaction", vec![json!(txid), json!(true)])
            .await;
        match transaction {
            Ok(transaction) => {
                if transaction_spends(&transaction, point)? {
                    return Ok(Some(transaction));
                }
            }
            // A transaction may leave the mempool between the two requests.
            Err(BitcoindRpcError::RpcError { code: -5, .. }) => {}
            Err(error) => return Err(rpc_error("getrawtransaction", &error)),
        }
    }
    Ok(None)
}

async fn block_spender(
    client: &impl ChainRpc,
    journal: &mut Journal,
    tip: u32,
) -> Result<Option<Value>, String> {
    let start = journal
        .scan_height
        .ok_or("spent vault has no chain scan starting height")?;
    for height in start..=tip {
        let hash: String = request(client, "getblockhash", vec![json!(height)]).await?;
        let block: Value = request(client, "getblock", vec![json!(hash), json!(2)]).await?;
        let transactions = block["tx"]
            .as_array()
            .ok_or("verbose block has no transactions")?;
        for transaction in transactions {
            if transaction_spends(transaction, &journal.vault)? {
                // Keep this height inclusive: output 0 can move again in this block.
                journal.scan_height = Some(height);
                journal.scan_hash = Some(hash);
                return Ok(Some(transaction.clone()));
            }
        }
        journal.scan_height = Some(height);
        journal.scan_hash = Some(hash);
    }
    Ok(None)
}

/// Return the current unspent vault, following every output-0 successor first.
/// An absent vault means its spender is not visible yet; the verified lineage
/// still lets the caller reconcile a pending transaction seen along the way.
/// A changed chain scan anchor fails closed for operator-reviewed recovery.
pub(super) async fn follow_vault(
    client: &impl ChainRpc,
    covenant: &Covenant,
    category_display: &str,
    journal: &mut Journal,
    tip: u32,
) -> Result<Followed, String> {
    follow_with_rpc(client, covenant, category_display, journal, tip).await
}

async fn follow_with_rpc(
    client: &impl ChainRpc,
    covenant: &Covenant,
    category_display: &str,
    journal: &mut Journal,
    tip: u32,
) -> Result<Followed, String> {
    let mut transactions = Vec::with_capacity(4);
    loop {
        check_scan_anchor(client, journal, tip).await?;
        let output: Value = request(
            client,
            "gettxout",
            vec![
                json!(journal.vault.txid),
                json!(journal.vault.vout),
                json!(true),
            ],
        )
        .await?;
        if !output.is_null() {
            let vault = parse_vault_output(&output, covenant, category_display, journal.thread)?;
            record_vault(journal, &vault)?;
            anchor_scan(client, journal, tip).await?;
            return Ok(Followed {
                vault: Some(vault),
                transactions,
            });
        }
        if journal.thread.is_none() || journal.scan_height.is_none() {
            initialize_spent_vault(client, covenant, category_display, journal, tip).await?;
        }
        let spender = match mempool_spender(client, &journal.vault).await? {
            Some(transaction) => Some(transaction),
            None => block_spender(client, journal, tip).await?,
        };
        if let Some(transaction) = spender {
            let next_point = transaction_point(&transaction)?;
            let output = transaction["vout"]
                .as_array()
                .and_then(|outputs| outputs.first())
                .ok_or("vault spender has no output 0")?;
            if output["n"].as_u64() != Some(0) {
                return Err("vault spender output 0 has an invalid index".into());
            }
            let vault = parse_vault_output(output, covenant, category_display, journal.thread)?;
            transactions.push(next_point.txid.clone());
            journal.vault = next_point;
            record_vault(journal, &vault)?;
        } else {
            return Ok(Followed {
                vault: None,
                transactions,
            });
        }
    }
}

#[cfg(test)]
mod test_utils {
    use std::cell::RefCell;
    use std::collections::VecDeque;

    use bitcoindrpc::BitcoindRpcError;
    use serde_json::Value;

    use super::ChainRpc;

    /// A strict in-memory RPC transcript; no socket or node is involved.
    pub(super) struct QueuedRpc {
        pub responses: RefCell<VecDeque<(&'static str, Vec<Value>, Value)>>,
    }

    impl ChainRpc for QueuedRpc {
        async fn call(
            &self,
            method: &str,
            parameters: Vec<Value>,
        ) -> Result<Value, BitcoindRpcError> {
            let (expected_method, expected_parameters, result) = self
                .responses
                .borrow_mut()
                .pop_front()
                .expect("unexpected RPC request");
            assert_eq!(method, expected_method);
            assert_eq!(parameters, expected_parameters);
            Ok(result)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;

    use serde_json::json;

    use super::test_utils::QueuedRpc;
    use super::{
        decimal_satoshis, follow_with_rpc, parse_vault_output, rpc_error, transaction_spends,
    };
    use crate::claimer::config::VaultPoint;
    use crate::claimer::files::Journal;
    use crate::{Covenant, create_covenant};
    use bitcoindrpc::BitcoindRpcError;

    #[test]
    fn decimal_bch_is_converted_without_fractional_satoshi_rounding() {
        assert_eq!(decimal_satoshis("0.02000000").unwrap(), 2_000_000);
        assert_eq!(decimal_satoshis("8e-6").unwrap(), 800);
        assert_eq!(decimal_satoshis("1e+0").unwrap(), 100_000_000);
        assert!(decimal_satoshis("0.000000001").is_err());
        assert!(decimal_satoshis("-1.0").is_err());
        assert!(decimal_satoshis("1e999").is_err());
    }

    #[test]
    fn chain_output_pins_display_category_and_thread() {
        let covenant = create_covenant("chipnet", 4, 84_000_000_000).unwrap();
        let category = "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
        let output = json!({
            "value": 0.02,
            "scriptPubKey": {"hex": hex::encode(&covenant.locking_bytecode)},
            "tokenData": {
                "category": category,
                "amount": "525000000000000",
                "nft": {"capability": "mutable", "commitment": "0300000011090500ffff001dba7d451a"}
            }
        });
        let vault = parse_vault_output(&output, &covenant, category, Some(3)).unwrap();
        assert_eq!(vault.value, 2_000_000);
        assert_eq!(vault.token_amount, 525_000_000_000_000);
        assert_eq!(hex::encode(vault.category), category);
        assert!(parse_vault_output(&output, &covenant, category, Some(2)).is_err());
    }

    #[test]
    fn chain_output_rejects_another_category() {
        let covenant = Covenant {
            network: "chipnet".into(),
            threads: 4,
            virtual_sats: 84_000_000_000,
            fee_k: 0,
            redeem_script: Vec::with_capacity(0),
            locking_bytecode: vec![0x51],
            lock_hash: vec![0; 32],
        };
        let output = json!({"tokenData": {"category": "11".repeat(32)}});
        assert!(parse_vault_output(&output, &covenant, &"22".repeat(32), None).is_err());
    }

    #[test]
    fn chain_output_rejects_a_nonmutable_nft() {
        let covenant = create_covenant("chipnet", 4, 84_000_000_000).unwrap();
        let output =
            json!({"tokenData": {"category": "11".repeat(32), "nft": {"capability": "minting"}}});
        assert!(parse_vault_output(&output, &covenant, &"11".repeat(32), None).is_err());
    }

    #[test]
    fn chain_output_rejects_another_locking_bytecode() {
        let covenant = create_covenant("chipnet", 4, 84_000_000_000).unwrap();
        let output = json!({
            "tokenData": {"category": "11".repeat(32), "nft": {"capability": "mutable"}},
            "scriptPubKey": {"hex": "51"}
        });
        assert!(parse_vault_output(&output, &covenant, &"11".repeat(32), None).is_err());
    }

    #[test]
    fn spender_must_reference_both_transaction_and_output() {
        let point = VaultPoint {
            txid: "11".repeat(32),
            vout: 3,
        };
        assert!(
            transaction_spends(
                &json!({"vin": [{"txid": "11".repeat(32), "vout": 3}]}),
                &point
            )
            .unwrap()
        );
        assert!(
            !transaction_spends(
                &json!({"vin": [{"txid": "11".repeat(32), "vout": 2}]}),
                &point
            )
            .unwrap()
        );
        assert!(
            !transaction_spends(
                &json!({"vin": [{"txid": "22".repeat(32), "vout": 3}]}),
                &point
            )
            .unwrap()
        );
        assert!(!transaction_spends(&json!({"vin": [{"coinbase": "00"}]}), &point).unwrap());
    }

    #[test]
    fn rpc_errors_never_include_node_messages_or_transport_urls() {
        assert_eq!(
            rpc_error(
                "gettxout",
                &BitcoindRpcError::RpcError {
                    code: -5,
                    message: "secret".into()
                }
            ),
            "gettxout failed with RPC code -5"
        );
        assert_eq!(
            rpc_error(
                "gettxout",
                &BitcoindRpcError::Transport {
                    message: "secret".into()
                }
            ),
            "gettxout failed; check the node and RPC connection"
        );
    }

    #[tokio::test]
    async fn unspent_vault_is_read_from_chain_without_transaction_index() {
        let directory = tempfile::tempdir().unwrap();
        let point = VaultPoint {
            txid: "11".repeat(32),
            vout: 3,
        };
        let mut journal = Journal::load(&directory.path().join("journal.json"), &point).unwrap();
        let covenant = create_covenant("chipnet", 4, 84_000_000_000).unwrap();
        let output = json!({
            "value": 0.02, "scriptPubKey": {"hex": hex::encode(&covenant.locking_bytecode)},
            "tokenData": {"category": "aa".repeat(32), "amount": "525000000000000", "nft": {
                "capability": "mutable", "commitment": "0300000011090500ffff001dba7d451a"
            }}
        });
        let rpc = QueuedRpc {
            responses: RefCell::new(VecDeque::from([
                (
                    "gettxout",
                    vec![json!(point.txid), json!(3), json!(true)],
                    output,
                ),
                ("getblockhash", vec![json!(100)], json!("tip-hash")),
            ])),
        };
        let vault = follow_with_rpc(&rpc, &covenant, &"aa".repeat(32), &mut journal, 100)
            .await
            .unwrap()
            .vault
            .unwrap();
        assert_eq!(vault.value, 2_000_000);
        assert_eq!(journal.thread, Some(3));
        assert_eq!(journal.scan_height, Some(100));
        assert_eq!(journal.scan_hash.as_deref(), Some("tip-hash"));
        assert!(rpc.responses.borrow().is_empty());
    }

    #[tokio::test]
    async fn mempool_market_move_changes_outpoint_without_changing_commitment() {
        let directory = tempfile::tempdir().unwrap();
        let point = VaultPoint {
            txid: "11".repeat(32),
            vout: 3,
        };
        let mut journal = Journal::load(&directory.path().join("journal.json"), &point).unwrap();
        journal.thread = Some(3);
        journal.scan_height = Some(99);
        let covenant = create_covenant("chipnet", 4, 84_000_000_000).unwrap();
        let output = json!({
            "n": 0, "value": 0.03, "scriptPubKey": {"hex": hex::encode(&covenant.locking_bytecode)},
            "tokenData": {"category": "aa".repeat(32), "amount": "524000000000000", "nft": {
                "capability": "mutable", "commitment": "0300000011090500ffff001dba7d451a"
            }}
        });
        let spender = json!({"txid": "22".repeat(32), "vin": [{"txid": point.txid, "vout": 3}], "vout": [output.clone()]});
        let rpc = QueuedRpc {
            responses: RefCell::new(VecDeque::from([
                (
                    "gettxout",
                    vec![json!(point.txid), json!(3), json!(true)],
                    json!(null),
                ),
                ("getrawmempool", vec![], json!(["22".repeat(32)])),
                (
                    "getrawtransaction",
                    vec![json!("22".repeat(32)), json!(true)],
                    spender,
                ),
                (
                    "gettxout",
                    vec![json!("22".repeat(32)), json!(0), json!(true)],
                    output,
                ),
                ("getblockhash", vec![json!(100)], json!("tip-hash")),
            ])),
        };
        let vault = follow_with_rpc(&rpc, &covenant, &"aa".repeat(32), &mut journal, 100)
            .await
            .unwrap()
            .vault
            .unwrap();
        assert_eq!(vault.value, 3_000_000);
        assert_eq!(
            journal.vault,
            VaultPoint {
                txid: "22".repeat(32),
                vout: 0
            }
        );
        assert_eq!(
            journal.commitment.as_deref(),
            Some("0300000011090500ffff001dba7d451a")
        );
        assert!(rpc.responses.borrow().is_empty());
    }

    #[tokio::test]
    async fn repeated_moves_in_one_block_keep_the_scan_height_inclusive() {
        let directory = tempfile::tempdir().unwrap();
        let point = VaultPoint {
            txid: "11".repeat(32),
            vout: 3,
        };
        let mut journal = Journal::load(&directory.path().join("journal.json"), &point).unwrap();
        journal.thread = Some(3);
        journal.scan_height = Some(99);
        let covenant = create_covenant("chipnet", 4, 84_000_000_000).unwrap();
        let output = json!({
            "n": 0, "value": 0.02, "scriptPubKey": {"hex": hex::encode(&covenant.locking_bytecode)},
            "tokenData": {"category": "aa".repeat(32), "amount": "525000000000000", "nft": {
                "capability": "mutable", "commitment": "0300000011090500ffff001dba7d451a"
            }}
        });
        let first = json!({"txid": "22".repeat(32), "vin": [{"txid": point.txid, "vout": 3}], "vout": [output.clone()]});
        let second = json!({"txid": "33".repeat(32), "vin": [{"txid": "22".repeat(32), "vout": 0}], "vout": [output.clone()]});
        let block = json!({"tx": [first, second]});
        let rpc = QueuedRpc {
            responses: RefCell::new(VecDeque::from([
                (
                    "gettxout",
                    vec![json!(point.txid), json!(3), json!(true)],
                    json!(null),
                ),
                ("getrawmempool", vec![], json!([])),
                ("getblockhash", vec![json!(99)], json!("block-99")),
                ("getblock", vec![json!("block-99"), json!(2)], block.clone()),
                ("getblockhash", vec![json!(99)], json!("block-99")),
                (
                    "gettxout",
                    vec![json!("22".repeat(32)), json!(0), json!(true)],
                    json!(null),
                ),
                ("getrawmempool", vec![], json!([])),
                ("getblockhash", vec![json!(99)], json!("block-99")),
                ("getblock", vec![json!("block-99"), json!(2)], block),
                ("getblockhash", vec![json!(99)], json!("block-99")),
                (
                    "gettxout",
                    vec![json!("33".repeat(32)), json!(0), json!(true)],
                    output,
                ),
                ("getblockhash", vec![json!(100)], json!("tip-hash")),
            ])),
        };
        assert!(
            follow_with_rpc(&rpc, &covenant, &"aa".repeat(32), &mut journal, 100)
                .await
                .unwrap()
                .vault
                .is_some()
        );
        assert_eq!(
            journal.vault,
            VaultPoint {
                txid: "33".repeat(32),
                vout: 0
            }
        );
        assert!(rpc.responses.borrow().is_empty());
    }

    #[tokio::test]
    async fn already_spent_start_is_validated_before_following_its_successor() {
        let directory = tempfile::tempdir().unwrap();
        let point = VaultPoint {
            txid: "11".repeat(32),
            vout: 3,
        };
        let mut journal = Journal::load(&directory.path().join("journal.json"), &point).unwrap();
        let covenant = create_covenant("chipnet", 4, 84_000_000_000).unwrap();
        let output = json!({
            "n": 0, "value": 0.02, "scriptPubKey": {"hex": hex::encode(&covenant.locking_bytecode)},
            "tokenData": {"category": "aa".repeat(32), "amount": "525000000000000", "nft": {
                "capability": "mutable", "commitment": "0300000011090500ffff001dba7d451a"
            }}
        });
        let original = json!({"txid": point.txid, "vout": [null, null, null, output.clone()], "blockhash": "birth-hash"});
        let spender = json!({"txid": "22".repeat(32), "vin": [{"txid": point.txid, "vout": 3}], "vout": [output.clone()]});
        let rpc = QueuedRpc {
            responses: RefCell::new(VecDeque::from([
                (
                    "gettxout",
                    vec![json!(point.txid), json!(3), json!(true)],
                    json!(null),
                ),
                (
                    "getrawtransaction",
                    vec![json!(point.txid), json!(true)],
                    original,
                ),
                (
                    "getblockheader",
                    vec![json!("birth-hash"), json!(true)],
                    json!({"height": 99}),
                ),
                ("getblockhash", vec![json!(99)], json!("birth-hash")),
                ("getrawmempool", vec![], json!(["22".repeat(32)])),
                (
                    "getrawtransaction",
                    vec![json!("22".repeat(32)), json!(true)],
                    spender,
                ),
                ("getblockhash", vec![json!(99)], json!("birth-hash")),
                (
                    "gettxout",
                    vec![json!("22".repeat(32)), json!(0), json!(true)],
                    output,
                ),
                ("getblockhash", vec![json!(100)], json!("tip-hash")),
            ])),
        };
        assert!(
            follow_with_rpc(&rpc, &covenant, &"aa".repeat(32), &mut journal, 100)
                .await
                .unwrap()
                .vault
                .is_some()
        );
        assert_eq!(journal.thread, Some(3));
        assert_eq!(
            journal.vault,
            VaultPoint {
                txid: "22".repeat(32),
                vout: 0
            }
        );
        assert!(rpc.responses.borrow().is_empty());
    }

    #[tokio::test]
    async fn invisible_spender_keeps_polling_without_inventing_vault_state() {
        let directory = tempfile::tempdir().unwrap();
        let point = VaultPoint {
            txid: "11".repeat(32),
            vout: 3,
        };
        let mut journal = Journal::load(&directory.path().join("journal.json"), &point).unwrap();
        journal.thread = Some(3);
        journal.scan_height = Some(100);
        let covenant = create_covenant("chipnet", 4, 84_000_000_000).unwrap();
        let rpc = QueuedRpc {
            responses: RefCell::new(VecDeque::from([
                (
                    "gettxout",
                    vec![json!(point.txid), json!(3), json!(true)],
                    json!(null),
                ),
                ("getrawmempool", vec![], json!([])),
                ("getblockhash", vec![json!(100)], json!("tip-hash")),
                (
                    "getblock",
                    vec![json!("tip-hash"), json!(2)],
                    json!({"tx": [{"vin": [{"coinbase": "00"}]}]}),
                ),
            ])),
        };
        assert!(
            follow_with_rpc(&rpc, &covenant, &"aa".repeat(32), &mut journal, 100)
                .await
                .unwrap()
                .vault
                .is_none()
        );
        assert_eq!(journal.vault, point);
        assert!(rpc.responses.borrow().is_empty());
    }

    #[tokio::test]
    async fn reorged_scan_anchor_fails_closed() {
        let directory = tempfile::tempdir().unwrap();
        let point = VaultPoint {
            txid: "11".repeat(32),
            vout: 3,
        };
        let mut journal = Journal::load(&directory.path().join("journal.json"), &point).unwrap();
        journal.scan_height = Some(100);
        journal.scan_hash = Some("old-tip".into());
        let covenant = create_covenant("chipnet", 4, 84_000_000_000).unwrap();
        let rpc = QueuedRpc {
            responses: RefCell::new(VecDeque::from([(
                "getblockhash",
                vec![json!(100)],
                json!("new-tip"),
            )])),
        };
        assert!(
            follow_with_rpc(&rpc, &covenant, &"aa".repeat(32), &mut journal, 100)
                .await
                .unwrap_err()
                .contains("anchor changed")
        );
        assert_eq!(journal.vault, point);
        assert!(rpc.responses.borrow().is_empty());
    }
}
