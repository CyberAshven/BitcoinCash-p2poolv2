// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;

const HASH_A: &str = "aaaaa00000000000000000000000000000000000000000000000000000000001";
const HASH_B: &str = "bbbbb00000000000000000000000000000000000000000000000000000000002";

fn line(text: &str) -> String {
    format!("2026-04-23T10:00:00.123456Z DEBUG p2poolv2: {text}")
}

#[test]
fn line_without_leading_timestamp_has_no_events() {
    assert!(events_from_line(&format!("x {}", line("Confirmed reorg needed"))).is_empty());
}

#[test]
fn locally_mined_block_store_is_local() {
    let events = events_from_line(&line(&format!("Adding share to chain: {HASH_A}")));
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].source, Source::Local);
    assert_eq!(
        events[0].detail,
        "Block aaaaa added to chain (locally mined)"
    );
}

#[test]
fn peer_block_store_is_peer() {
    let events = events_from_line(&line(&format!(
        "Adding share and organising header atomically: {HASH_A}"
    )));
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].source, Source::Peer);
    assert_eq!(
        events[0].detail,
        "Block aaaaa stored + header organised (from peer)"
    );
}

#[test]
fn confirmed_reads_the_height() {
    let events = events_from_line(&line(&format!(
        "Promoted block {HASH_A} to confirmed height Some(42)"
    )));
    assert_eq!(events[0].detail, "Block aaaaa CONFIRMED at height 42");
}

#[test]
fn uncle_promotion_has_no_height() {
    let events = events_from_line(&line(&format!(
        "Promoted block {HASH_A} to confirmed height None"
    )));
    assert_eq!(events[0].detail, "Block aaaaa CONFIRMED at height none");
}

#[test]
fn parentage_is_appended_to_the_share_events() {
    let lines = vec![
        line(&format!(
            "Received ShareBlock {{ prev_share_blockhash: {HASH_B}, uncles: [{HASH_B}, {HASH_A}] }}"
        )),
        line(&format!("Adding share to chain: {HASH_A}")),
    ];
    let events = parse_lines(&lines);
    assert_eq!(
        events[0].detail,
        "Block aaaaa added to chain (locally mined) (prev:bbbbb uncles:bbbbb,aaaaa)"
    );
}

#[test]
fn parentage_needs_the_adding_line_right_after_the_dump() {
    let lines = vec![
        line(&format!(
            "Received ShareBlock {{ prev_share_blockhash: {HASH_B}, uncles: [] }}"
        )),
        line("something else"),
        line(&format!("Adding share to chain: {HASH_A}")),
    ];
    let events = parse_lines(&lines);
    assert_eq!(
        events[0].detail,
        "Block aaaaa added to chain (locally mined)"
    );
}

#[test]
fn summary_counts_local_and_peer_stores() {
    let lines = vec![
        line(&format!("Adding share to chain: {HASH_A}")),
        line(&format!(
            "Adding share and organising header atomically: {HASH_B}"
        )),
    ];
    let text = summary(&parse_lines(&lines), "node.log");
    assert!(text.contains("Unique shares seen:   2\n"));
    assert!(text.contains(&format!("Locally mined:        {MAGENTA}1{RESET}")));
    assert!(text.contains(&format!("Received from peers:  {CYAN}1{RESET}")));
}

#[test]
fn compact_event_shows_time_of_day() {
    let events = events_from_line(&line("Confirmed reorg needed"));
    assert_eq!(
        compact_event(&events[0]),
        // The tag keeps its padding: it ends in the reset code, so the
        // Python tool's rstrip() never reached the spaces.
        format!(
            "{DIM}10:00:00.123{RESET} {RED}{BOLD}[REORG] {RESET} {RED}{BOLD}Confirmed chain REORG triggered{RESET}"
        )
    );
}
