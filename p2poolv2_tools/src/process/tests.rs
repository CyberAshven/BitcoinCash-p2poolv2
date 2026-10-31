// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;

#[test]
fn parse_pids_reads_one_per_line() {
    assert_eq!(parse_pids("12\n345\n"), vec![12, 345]);
}

#[test]
fn parse_pids_skips_junk() {
    assert_eq!(parse_pids("\nabc\n 7 \n"), vec![7]);
}

#[test]
fn send_signal_to_no_pids_is_false() {
    assert!(!send_signal(&[], Signal::Probe));
}
