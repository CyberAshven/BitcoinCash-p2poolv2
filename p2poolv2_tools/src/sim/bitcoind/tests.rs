// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;

#[test]
fn rpc_error_in_body_reads_the_error() {
    assert_eq!(
        rpc_error_in_body(
            r#"{"result":null,"error":{"code":-5,"message":"Block not found"},"id":1}"#
        ),
        Some(r#"{"code":-5,"message":"Block not found"}"#.to_string())
    );
}

#[test]
fn rpc_error_in_body_ignores_a_null_error() {
    assert_eq!(rpc_error_in_body(r#"{"result":1,"error":null}"#), None);
}

#[test]
fn rpc_error_in_body_ignores_non_json() {
    assert_eq!(rpc_error_in_body("Unauthorized"), None);
}

#[test]
fn wallet_endpoint_extends_the_url() {
    let settings = RpcSettings {
        url: "http://127.0.0.1:19443/".to_string(),
        user: "p2pool".to_string(),
        password: "p2pool".to_string(),
    };

    let wallet = settings.for_wallet(WALLET);

    assert_eq!(wallet.url, "http://127.0.0.1:19443/wallet/p2pool");
    assert_eq!(wallet.user, "p2pool");
}

#[test]
fn bchn_is_recognised_by_its_user_agent() {
    assert!(is_bchn_user_agent("/Bitcoin Cash Node:29.2.0(EB32.0)/"));
}

#[test]
fn bitcoin_core_is_not_bchn() {
    assert!(!is_bchn_user_agent("/Satoshi:31.0.0/"));
}
