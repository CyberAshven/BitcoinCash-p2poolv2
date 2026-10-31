// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;

#[test]
fn confirmed_heights_reads_the_height() {
    let line = b"x Promoted block ab to confirmed height Some(42) y";
    assert_eq!(confirmed_heights(line).collect::<Vec<_>>(), vec![42]);
}

#[test]
fn confirmed_heights_needs_the_closing_paren() {
    let line = b"to confirmed height Some(42";
    assert_eq!(confirmed_heights(line).count(), 0);
}

#[test]
fn confirmed_heights_ignores_uncles() {
    let line = b"Promoted block ab to confirmed height None";
    assert_eq!(confirmed_heights(line).count(), 0);
}

#[test]
fn promoted_hashes_reads_hex_runs() {
    let line = b"Promoted block 00ff to confirmed height None";
    assert_eq!(
        promoted_hashes(line).collect::<Vec<_>>(),
        vec![&b"00ff"[..]]
    );
}

#[test]
fn promoted_hashes_skips_non_hex() {
    let line = b"Promoted block XYZ";
    assert_eq!(promoted_hashes(line).count(), 0);
}

#[test]
fn confirmed_height_range_spans_the_log() {
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("node-0.log");
    std::fs::write(
        &path,
        "a Promoted block 11 to confirmed height Some(5)\nb Promoted block 12 to confirmed height Some(3)\nc Promoted block 13 to confirmed height None\n",
    )
    .expect("write log");
    assert_eq!(
        confirmed_height_range(&path).expect("scan"),
        Some(HeightRange {
            lowest: 3,
            highest: 5
        })
    );
}

#[test]
fn tip_height_of_missing_log_is_zero() {
    assert_eq!(tip_height(Path::new("/nonexistent/node-0.log")), 0);
}

#[test]
fn first_hash_at_height_takes_the_first_promotion() {
    let directory = tempfile::tempdir().expect("temp dir");
    let path = directory.path().join("node-0.log");
    std::fs::write(
        &path,
        "Promoted block 1111111111111111aa to confirmed height Some(70)\nPromoted block 2222222222222222bb to confirmed height Some(7)\nPromoted block 3333333333333333cc to confirmed height Some(7)\n",
    )
    .expect("write log");
    assert_eq!(
        first_hash_at_height(&path, 7).expect("scan"),
        Some("2222222222222222bb".to_string())
    );
}
