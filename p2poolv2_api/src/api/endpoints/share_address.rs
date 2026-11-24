// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `/share_address?payout=`: the share chain address a payout CashAddr
//! stands for on this node's network.
//!
//! The help page's generator calls it, and the CLI command
//! `bch-share-address` shares its rule, so the three always agree. The
//! network is the node's own (`bitcoin_cash_network`): a mainnet node
//! answers under `p2pool:`, a chipnet node under `tp2pool:`.

use crate::api::error::ApiError;
use crate::api::server::AppState;
use axum::Json;
use axum::extract::{Query, State};
use p2poolv2_lib::address::bch_share::BchShareNetwork;
use p2poolv2_lib::config::{BitcoinCashNetwork, payout_to_share_address};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// The `?payout=` query parameter; absent or empty is a bad request with
/// its own message rather than a parse error about a missing address.
#[derive(Deserialize)]
pub struct ShareAddressQuery {
    payout: Option<String>,
}

/// The generated share chain address, the same string the CLI prints.
#[derive(Serialize)]
pub struct ShareAddressResponse {
    address: String,
}

/// The payout address and the share address answer as one rule: decode the
/// payout for `cash_network` and re-encode its key hash under the share
/// prefix. Rejects a payout of another network, a P2SH script and anything
/// unparseable, each with the wording the help page shows the user. A node
/// with no cash network mines the Bitcoin chain and has no prefix to answer
/// under, so its refusal says so.
fn share_address_for(
    payout: &str,
    cash_network: Option<BchShareNetwork>,
) -> Result<String, ApiError> {
    let payout = payout.trim();
    if payout.is_empty() {
        return Err(ApiError::BadRequest(
            "Enter a payout address from your wallet.".to_string(),
        ));
    }
    let share_network = cash_network.ok_or_else(|| {
        ApiError::BadRequest(
            "This node mines the Bitcoin chain, which has no share address prefix.".to_string(),
        )
    })?;
    let network = BitcoinCashNetwork::from_share_network(share_network);
    payout_to_share_address(payout, network).map_err(|error| ApiError::BadRequest(error.message))
}

/// Answer `GET /share_address?payout=` with the share address for that
/// payout on this node's network.
pub(crate) async fn share_address(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ShareAddressQuery>,
) -> Result<Json<ShareAddressResponse>, ApiError> {
    let payout = query.payout.as_deref().unwrap_or_default();
    let address = share_address_for(payout, state.app_config.cash_network)?;
    Ok(Json(ShareAddressResponse { address }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mainnet payout answers under the `p2pool:` prefix, the vector the
    /// CLI command pins for the same input.
    #[test]
    fn a_mainnet_payout_answers_under_the_p2pool_prefix() {
        let address = share_address_for(
            "bitcoincash:qr6m7j9njldwwzlg9v7v53unlr4jkmx6eylep8ekg2",
            Some(BchShareNetwork::Mainnet),
        )
        .unwrap();
        assert!(address.starts_with("p2pool:q"), "{address}");
    }

    /// A chipnet payout answers under `tp2pool:`, which is what the help
    /// page of a chipnet node shows.
    #[test]
    fn a_chipnet_payout_answers_under_the_tp2pool_prefix() {
        let address = share_address_for(
            "bchtest:qr7fzmep8g7h7ymfxy74lgc0v950j3r295pdnvy3hr",
            Some(BchShareNetwork::Chipnet),
        )
        .unwrap();
        assert!(address.starts_with("tp2pool:q"), "{address}");
    }

    /// No payout at all gets its own message instead of a parse error
    /// about an empty address.
    #[test]
    fn no_payout_is_a_bad_request_asking_for_one() {
        let error = share_address_for("", Some(BchShareNetwork::Chipnet)).unwrap_err();
        let ApiError::BadRequest(message) = error else {
            panic!("expected a bad request");
        };
        assert!(message.contains("payout address"), "{message}");
    }

    /// Surrounding whitespace is not part of the address: a value pasted
    /// with a trailing newline still converts.
    #[test]
    fn a_payout_pasted_with_whitespace_still_converts() {
        let address = share_address_for(
            "\n bchtest:qr7fzmep8g7h7ymfxy74lgc0v950j3r295pdnvy3hr \n",
            Some(BchShareNetwork::Chipnet),
        )
        .unwrap();
        assert!(address.starts_with("tp2pool:q"), "{address}");
    }

    /// An unparseable payout is refused with the parse error's own words,
    /// which is the string the help page shows the user.
    #[test]
    fn an_unparseable_payout_is_refused_with_the_parse_error() {
        let error =
            share_address_for("not-an-address", Some(BchShareNetwork::Chipnet)).unwrap_err();
        let ApiError::BadRequest(message) = error else {
            panic!("expected a bad request");
        };
        assert!(message.contains("Invalid Bitcoin Cash"), "{message}");
    }

    /// A node mining the Bitcoin chain has no share prefix to answer under,
    /// so the request is refused rather than guessed at.
    #[test]
    fn a_node_without_a_cash_network_refuses_the_request() {
        let error = share_address_for(
            "bitcoincash:qr6m7j9njldwwzlg9v7v53unlr4jkmx6eylep8ekg2",
            None,
        )
        .unwrap_err();
        let ApiError::BadRequest(message) = error else {
            panic!("expected a bad request");
        };
        assert!(message.contains("Bitcoin chain"), "{message}");
    }
}
