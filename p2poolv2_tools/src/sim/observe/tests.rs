// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use serde_json::json;

#[test]
fn top_counts_sorts_like_sort_rn() {
    let counts = HashMap::from([
        ("a@1".to_string(), 2),
        ("c@1".to_string(), 1),
        ("b@1".to_string(), 2),
    ]);
    assert_eq!(
        top_counts(&counts, 8),
        vec!["      2 b@1", "      2 a@1", "      1 c@1"]
    );
}

#[test]
fn json_length_of_array_counts_items() {
    assert_eq!(json_length(&json!([1, 2, 3])), Some("3".to_string()));
}

#[test]
fn json_length_of_object_counts_keys() {
    assert_eq!(json_length(&json!({"error": "x"})), Some("1".to_string()));
}

#[test]
fn json_length_of_boolean_fails() {
    assert_eq!(json_length(&json!(true)), None);
}

#[test]
fn scan_reads_the_last_promotion_and_errors() {
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("node-0.log");
    std::fs::write(
        &path,
        concat!(
            "t Promoted block 0123456789abcdef01 to confirmed height Some(4)\n",
            "t Promoted block fedcba9876543210ff to confirmed height Some(5)\n",
            "t ERROR something broke\n",
            "t error max_workbase exceeded\n",
            "t ERROR something broke\n",
        ),
    )
    .expect("write log");
    let scan = scan_log(&path).expect("scan");
    assert_eq!(scan.tip, Some((Some(5), Some("fedcba987654".to_string()))));
    assert_eq!(scan.promotion_lines, 2);
    assert_eq!(scan.error_lines, 2);
    assert_eq!(scan.errors.get(&b"t ERROR something broke"[..]), Some(&2));
}
