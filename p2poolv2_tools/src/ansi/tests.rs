// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;

#[test]
fn strip_removes_colour_sequences() {
    assert_eq!(strip("\x1b[1m\x1b[32mok\x1b[0m done"), "ok done");
}

#[test]
fn visible_length_ignores_colour_sequences() {
    assert_eq!(visible_length("\x1b[2mabc\x1b[0m"), 3);
}

#[test]
fn pad_to_width_pads_by_visible_length() {
    assert_eq!(pad_to_width("\x1b[2mab\x1b[0m", 4), "\x1b[2mab\x1b[0m  ");
}

#[test]
fn pad_to_width_leaves_longer_text_alone() {
    assert_eq!(pad_to_width("abcdef", 3), "abcdef");
}
