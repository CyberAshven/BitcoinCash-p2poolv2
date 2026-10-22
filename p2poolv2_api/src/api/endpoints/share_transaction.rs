// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `POST /share_transaction`: hand a signed share transaction to this node.
//!
//! `p2poolv2_cli bch-trade` signs share-chain trade transactions offline;
//! this is how one reaches the share chain. The node checks it against the
//! confirmed share chain, adds it to its share transaction pool, relays it to
//! its peers and mines it in the shares it builds. Bitcoin Cash only.

use crate::api::error::ApiError;
use crate::api::server::AppState;
use axum::{Json, extract::State};
use bitcoin::consensus::encode;
use p2poolv2_lib::node::actor::ShareTransactionSubmitError;
use p2poolv2_lib::shares::transaction_pool::{MAX_POOL_TRANSACTION_SIZE, SubmitOutcome};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Largest request body the route reads: a transaction at the pool's size
/// limit in hex, and room for the JSON around it.
pub(crate) const MAX_SHARE_TRANSACTION_REQUEST_BYTES: usize = 2 * MAX_POOL_TRANSACTION_SIZE + 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ShareTransactionRequest {
    /// The signed transaction, consensus serialized, in hex.
    hex: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct ShareTransactionResponse {
    txid: String,
    /// `accepted` when the pool took it and relays it, `already_in_pool`
    /// when it had it already.
    status: &'static str,
}

pub(crate) async fn submit(
    State(state): State<Arc<AppState>>,
    Json(request): Json<ShareTransactionRequest>,
) -> Result<Json<ShareTransactionResponse>, ApiError> {
    let bytes = hex::decode(request.hex.trim())
        .map_err(|error| ApiError::BadRequest(format!("Invalid transaction hex: {error}")))?;
    let transaction: bitcoin::Transaction = encode::deserialize(&bytes)
        .map_err(|error| ApiError::BadRequest(format!("Invalid transaction: {error}")))?;
    let outcome = state
        .node_handle
        .submit_share_transaction(transaction)
        .map_err(|error| match error {
            ShareTransactionSubmitError::NoPool => ApiError::ServiceUnavailable(error.to_string()),
            ShareTransactionSubmitError::Rejected(rejection) => {
                ApiError::BadRequest(format!("Share transaction refused: {rejection}"))
            }
        })?;
    let status = match outcome {
        SubmitOutcome::Accepted(_) => "accepted",
        SubmitOutcome::AlreadyInPool(_) => "already_in_pool",
    };
    Ok(Json(ShareTransactionResponse {
        txid: outcome.txid().to_string(),
        status,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::server::AppConfig;
    use bitcoin::consensus::serialize;
    use bitcoin::hashes::Hash;
    use bitcoin::{Amount, NetworkKind, OutPoint, PrivateKey, Txid};
    use p2poolv2_lib::accounting::stats::metrics;
    use p2poolv2_lib::address::share_owner::ShareOwner;
    use p2poolv2_lib::monitoring_events::create_monitoring_event_channel;
    use p2poolv2_lib::node::actor::NodeHandle;
    use p2poolv2_lib::shares::bch_htlc::{HtlcContract, secret_hash};
    use p2poolv2_lib::shares::bch_trade::{Coin, TradeKey, build_lock_transaction};
    use p2poolv2_lib::shares::chain::chain_store_handle::ChainStoreHandle;
    use p2poolv2_lib::shares::transaction_pool::ShareTransactionPool;
    use p2poolv2_lib::stratum::work::tracker::start_tracker_actor;
    use p2poolv2_lib::test_utils::{
        TRADE_BUYER_SECRET_KEY, TRADE_SECRET, TRADE_SELLER_SECRET_KEY,
        build_confirmed_chain_owned_from, setup_test_chain_store_handle, test_coinbase_transaction,
    };

    async fn build_test_state(
        chain_store_handle: ChainStoreHandle,
        node_handle: NodeHandle,
    ) -> (Arc<AppState>, tempfile::TempDir) {
        let metrics_temp = tempfile::tempdir().unwrap();
        let metrics_handle =
            metrics::start_metrics(metrics_temp.path().to_str().unwrap().to_string())
                .await
                .unwrap();
        let state = Arc::new(AppState {
            app_config: AppConfig {
                pool_signature_length: 0,
                network: bitcoin::Network::Signet,
                cash_network: None,
                cors_allowed: false,
            },
            chain_store_handle,
            metrics_handle,
            tracker_handle: start_tracker_actor(),
            node_handle,
            monitoring_event_sender: create_monitoring_event_channel().0,
            auth_user: None,
            auth_token: None,
            pplns_window: None,
        });
        (state, metrics_temp)
    }

    fn request(transaction: &bitcoin::Transaction) -> Json<ShareTransactionRequest> {
        Json(ShareTransactionRequest {
            hex: hex::encode(serialize(transaction)),
        })
    }

    #[tokio::test]
    async fn a_signed_lock_of_a_mature_share_is_accepted_into_the_pool() {
        let (chain_store_handle, _store_dir) = setup_test_chain_store_handle(true).await;
        let seller = TradeKey::new(
            PrivateKey::from_slice(&TRADE_SELLER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let buyer = TradeKey::new(
            PrivateKey::from_slice(&TRADE_BUYER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        let blocks = build_confirmed_chain_owned_from(
            &chain_store_handle,
            ShareOwner::CashP2pkh(seller.pubkey_hash()),
            100,
        )
        .await;
        let contract = HtlcContract::new(
            secret_hash(&TRADE_SECRET),
            buyer.pubkey_hash(),
            seller.pubkey_hash(),
            600,
        )
        .unwrap();
        let lock = build_lock_transaction(
            &[Coin {
                outpoint: OutPoint::new(blocks[1].transactions[0].compute_txid(), 0),
                value: Amount::from_sat(100_000_000),
            }],
            &seller,
            &contract,
            Amount::from_sat(100_000_000),
            &seller.p2pkh_script(),
            0,
        )
        .unwrap();
        let (pool, _relay_rx) = ShareTransactionPool::new(chain_store_handle.clone());
        let (state, _metrics_dir) = build_test_state(
            chain_store_handle,
            NodeHandle::new_for_test_with_share_transaction_pool(pool.clone()),
        )
        .await;

        let response = submit(State(state), request(&lock)).await.unwrap().0;

        assert_eq!(response.txid, lock.compute_txid().to_string());
        assert_eq!(response.status, "accepted");
        assert!(pool.contains(&lock.compute_txid()));
    }

    #[tokio::test]
    async fn a_transaction_spending_an_unknown_output_is_a_bad_request() {
        let (chain_store_handle, _store_dir) = setup_test_chain_store_handle(true).await;
        let seller = TradeKey::new(
            PrivateKey::from_slice(&TRADE_SELLER_SECRET_KEY, NetworkKind::Test).unwrap(),
        );
        build_confirmed_chain_owned_from(
            &chain_store_handle,
            ShareOwner::CashP2pkh(seller.pubkey_hash()),
            2,
        )
        .await;
        let contract = HtlcContract::new(
            secret_hash(&TRADE_SECRET),
            seller.pubkey_hash(),
            seller.pubkey_hash(),
            600,
        )
        .unwrap();
        let lock = build_lock_transaction(
            &[Coin {
                outpoint: OutPoint::new(Txid::from_byte_array([0x77; 32]), 0),
                value: Amount::from_sat(100_000),
            }],
            &seller,
            &contract,
            Amount::from_sat(100_000),
            &seller.p2pkh_script(),
            0,
        )
        .unwrap();
        let (pool, _relay_rx) = ShareTransactionPool::new(chain_store_handle.clone());
        let (state, _metrics_dir) = build_test_state(
            chain_store_handle,
            NodeHandle::new_for_test_with_share_transaction_pool(pool.clone()),
        )
        .await;

        let error = submit(State(state), request(&lock)).await.unwrap_err();

        match error {
            ApiError::BadRequest(message) => assert!(
                message.contains("does not have"),
                "unexpected message: {message}"
            ),
            other => panic!("expected a bad request, got {other}"),
        }
        assert!(pool.is_empty());
    }

    #[tokio::test]
    async fn a_node_without_a_pool_answers_service_unavailable() {
        let (chain_store_handle, _store_dir) = setup_test_chain_store_handle(true).await;
        let (state, _metrics_dir) =
            build_test_state(chain_store_handle, NodeHandle::new_for_test()).await;

        let error = submit(State(state), request(&test_coinbase_transaction(1)))
            .await
            .unwrap_err();

        assert!(matches!(error, ApiError::ServiceUnavailable(_)));
    }

    #[tokio::test]
    async fn malformed_hex_is_a_bad_request() {
        let (chain_store_handle, _store_dir) = setup_test_chain_store_handle(true).await;
        let (state, _metrics_dir) =
            build_test_state(chain_store_handle, NodeHandle::new_for_test()).await;

        let error = submit(
            State(state),
            Json(ShareTransactionRequest {
                hex: "zz".to_string(),
            }),
        )
        .await
        .unwrap_err();

        assert!(matches!(error, ApiError::BadRequest(_)));
    }
}
