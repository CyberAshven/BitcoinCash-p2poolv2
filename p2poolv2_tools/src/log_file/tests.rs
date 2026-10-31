// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use std::io::Write;

#[test]
fn log_lines_strip_endings_and_replace_bad_utf8() {
    let mut file = tempfile::NamedTempFile::new().expect("temp file");
    file.write_all(b"first\r\nsec\xffond\nlast")
        .expect("write temp file");
    let lines: Vec<String> = LogLines::open(file.path()).expect("open").collect();
    assert_eq!(lines, vec!["first", "sec\u{fffd}ond", "last"]);
}
