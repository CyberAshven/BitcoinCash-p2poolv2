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

#[test]
fn powershell_single_quoted_doubles_quotes() {
    assert_eq!(powershell_single_quoted("a'b"), "'a''b'");
    assert_eq!(powershell_single_quoted(r"C:\sim"), r"'C:\sim'");
}

#[test]
fn windows_image_name_adds_exe_once() {
    assert_eq!(windows_image_name("a"), "a.exe");
    assert_eq!(windows_image_name("bitcoind"), "bitcoind.exe");
    assert_eq!(windows_image_name("bitcoind.exe"), "bitcoind.exe");
    assert_eq!(windows_image_name("bitcoind.EXE"), "bitcoind.EXE");
}

#[test]
fn tasklist_csv_pids_read_the_second_column() {
    let text = "\
\"bitcoind.exe\",\"4242\",\"Console\",\"1\",\"12,345 K\"
INFO: No tasks are running which match the specified criteria.
";
    assert_eq!(parse_tasklist_csv_pids(text), vec![4242]);
}

#[test]
fn output_contains_pid_is_a_whole_field() {
    let text = "notepad.exe                   1234 Console                    1     10,000 K";
    assert!(output_contains_pid(text, 1234));
    assert!(!output_contains_pid(text, 12));
    assert!(!output_contains_pid(text, 123));
}

#[test]
fn output_text_decodes_utf16_le_pid_lines() {
    let bytes: Vec<u8> = "12\n34\n"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    assert_eq!(parse_pids(&output_text(&bytes)), vec![12, 34]);
}
