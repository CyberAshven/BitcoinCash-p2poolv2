// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use crate::scenario::{Length, bch_suite};
use crate::simulate::Settings;
use crate::suite::evaluate;

#[test]
fn segments_split_where_nobody_mines_and_take_log10() {
    let points = [
        (0.0, 10.0),
        (1.0, 100.0),
        (2.0, 0.0),
        (3.0, 0.0),
        (4.0, 1_000.0),
    ];
    let segments = positive_segments(&points);
    let expected = [vec![(0.0, 1.0), (1.0, 2.0)], vec![(4.0, 3.0)]];
    assert_eq!(segments.len(), expected.len());
    for (segment, wanted) in segments.iter().zip(&expected) {
        assert_eq!(segment.len(), wanted.len());
        for ((hours, log), (wanted_hours, wanted_log)) in segment.iter().zip(wanted) {
            assert_eq!(hours, wanted_hours);
            assert!((log - wanted_log).abs() < 1e-12);
        }
    }
    assert!(positive_segments(&[(0.0, 0.0)]).is_empty());
}

#[test]
fn a_chart_is_written_as_svg() {
    let mut scenario = bch_suite()
        .into_iter()
        .find(|scenario| scenario.id == "quiet-1h")
        .expect("quiet-1h is a Bitcoin Cash scenario");
    scenario.length = Length::Seconds(4.0 * 3_600.0);
    let rules = [Rule::ParentAnchor, Rule::GenesisAnchor];
    let results = evaluate(&scenario, &rules, 1, 42, &Settings::default());
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = directory.path().join("quiet-1h.svg");
    write_chart(&results, &path).expect("the chart draws");
    let svg = std::fs::read_to_string(&path).expect("the chart is written");
    assert!(svg.starts_with("<svg"));
    assert!(svg.contains(scenario.title));
    assert!(svg.contains(&Rule::GenesisAnchor.label()));
    assert!(svg.contains("steady difficulty of the hashrate"));
}
