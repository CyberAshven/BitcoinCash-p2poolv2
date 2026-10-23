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
