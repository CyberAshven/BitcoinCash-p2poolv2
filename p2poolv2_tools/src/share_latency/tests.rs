// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;

const HASH_A: &str = "aaaaaaaa00000000000000000000000000000000000000000000000000000001";

fn at(time: &str, text: &str) -> String {
    format!("2026-04-28T10:00:{time}Z DEBUG p2poolv2: {text}")
}

#[test]
fn format_duration_under_a_second_is_milliseconds() {
    assert_eq!(format_duration(0.25), "250.0ms");
}

#[test]
fn format_duration_negative_is_milliseconds() {
    assert_eq!(format_duration(-1.5), "-1500.0ms");
}

#[test]
fn format_duration_under_a_minute_is_seconds() {
    assert_eq!(format_duration(12.3456), "12.346s");
}

#[test]
fn format_duration_over_a_minute_is_minutes_and_seconds() {
    assert_eq!(format_duration(125.25), "2m 5.2s");
}

#[test]
fn first_seen_keeps_the_first_time() {
    let mut events = NodeEvents::default();
    events.add_line(&at(
        "01.000000",
        &format!("Share block {HASH_A} validated successfully"),
    ));
    events.add_line(&at(
        "02.000000",
        &format!("Share block {HASH_A} validated successfully"),
    ));
    assert_eq!(events.validated.len(), 1);
    assert_eq!(
        events.validated.get(HASH_A),
        parse_micros("2026-04-28T10:00:01.000000")
    );
}

#[test]
fn uncle_promotion_is_recorded_as_uncle() {
    let mut events = NodeEvents::default();
    events.add_line(&at(
        "01.5",
        &format!("Promoted block {HASH_A} to confirmed height None"),
    ));
    assert_eq!(events.uncles.len(), 1);
    assert!(events.confirmed.is_empty());
}

#[test]
fn propagation_measures_mined_to_validated() {
    let mut miner = NodeEvents::default();
    miner.add_line(&at(
        "01.000000",
        &format!("Adding share to chain: {HASH_A}"),
    ));
    miner.add_line(&at(
        "03.000000",
        &format!("Promoted block {HASH_A} to confirmed height Some(7)"),
    ));
    let mut receiver = NodeEvents::default();
    receiver.add_line(&at(
        "01.250000",
        &format!("Share block {HASH_A} validated successfully"),
    ));
    let mut nodes = Nodes::default();
    nodes.insert("a.log".to_string(), miner);
    nodes.insert("b.log".to_string(), receiver);
    assert_eq!(
        propagation_latencies(&nodes, false),
        vec![Propagation {
            hash: HASH_A.to_string(),
            miner: "a.log".to_string(),
            receiver: "b.log".to_string(),
            seconds: 0.25,
        }]
    );
}

#[test]
fn confirmation_measures_first_seen_to_promoted() {
    let mut node = NodeEvents::default();
    node.add_line(&at(
        "02.000000",
        &format!("Adding share and organising header atomically: {HASH_A}"),
    ));
    node.add_line(&at(
        "01.000000",
        &format!("Adding share to chain: {HASH_A}"),
    ));
    node.add_line(&at(
        "04.000000",
        &format!("Promoted block {HASH_A} to confirmed height Some(7)"),
    ));
    let mut nodes = Nodes::default();
    nodes.insert("a.log".to_string(), node);
    assert_eq!(
        confirmation_latencies(&nodes, false),
        vec![Confirmation {
            hash: HASH_A.to_string(),
            node: "a.log".to_string(),
            seconds: 3.0,
        }]
    );
}

#[test]
fn top_outliers_keeps_order_of_equal_latencies() {
    assert_eq!(
        top_outliers(&[
            ("first", 1.0),
            ("second", 2.0),
            ("third", 1.0),
            ("fourth", 1.0)
        ]),
        vec![
            ("second".to_string(), 2.0),
            ("first".to_string(), 1.0),
            ("third".to_string(), 1.0)
        ]
    );
}
