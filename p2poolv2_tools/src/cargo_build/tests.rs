// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;

#[test]
fn executable_from_message_reads_a_binary_artifact() {
    let line = r#"{"reason":"compiler-artifact","target":{"name":"verify_chain","kind":["bin"]},"executable":"/t/release/verify_chain"}"#;
    assert_eq!(
        executable_from_message(line),
        Some((
            "verify_chain".to_string(),
            PathBuf::from("/t/release/verify_chain")
        ))
    );
}

#[test]
fn executable_from_message_skips_libraries() {
    let line = r#"{"reason":"compiler-artifact","target":{"name":"p2poolv2_lib","kind":["lib"]},"executable":null}"#;
    assert_eq!(executable_from_message(line), None);
}

#[test]
fn executable_from_message_skips_other_messages() {
    let line = r#"{"reason":"build-finished","success":true}"#;
    assert_eq!(executable_from_message(line), None);
}

#[test]
fn executable_from_message_skips_non_json() {
    assert_eq!(executable_from_message("Compiling foo"), None);
}
