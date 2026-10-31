// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use crate::rules::LWMA_45;
use crate::scenario::{FARM_ASIC, Genesis, MinerGroup, PETAHASH, Schedule, upstream_suite};

/// One group on the main chain for `seconds`.
fn single_group(device_hashrate: Option<f64>, schedule: Schedule, seconds: f64) -> Scenario {
    Scenario {
        id: "test",
        title: "test",
        description: "test",
        genesis: Genesis::Main,
        groups: vec![MinerGroup {
            name: "miners",
            device_hashrate,
            schedule,
        }],
        length: Length::Seconds(seconds),
        warmup_shares: 0,
        events: Vec::new(),
        attack: false,
    }
}

/// Upstream's steady scenario cut to `shares` shares.
fn short_steady(shares: usize) -> Scenario {
    let mut scenario = upstream_suite().swap_remove(0);
    scenario.length = Length::Shares(shares);
    scenario
}

fn relative_difference(first: f64, second: f64) -> f64 {
    (first / second - 1.0).abs()
}

#[test]
fn a_seed_fixes_the_run() {
    let scenario = short_steady(500);
    let settings = Settings::default();
    let first = simulate(&scenario, Rule::ParentAnchor, 42, &settings);
    let again = simulate(&scenario, Rule::ParentAnchor, 42, &settings);
    let other = simulate(&scenario, Rule::ParentAnchor, 43, &settings);
    assert_eq!(first.chain.shares(), again.chain.shares());
    assert_eq!(first.records, again.records);
    assert_ne!(first.records, other.records);
}

#[test]
fn a_share_count_run_stops_at_its_count() {
    let run = simulate(&short_steady(300), LWMA_45, 1, &Settings::default());
    assert_eq!(run.records.len(), 300);
    assert_eq!(run.chain.shares().len(), 301);
    assert_eq!(run.end, run.records[299].found);
}

#[test]
fn shares_follow_one_another_with_whole_second_header_times() {
    let settings = Settings::default();
    let run = simulate(&short_steady(400), Rule::ParentAnchor, 7, &settings);
    let mut previous_found = 0.0;
    for (index, record) in run.records.iter().enumerate() {
        assert_eq!(record.started, previous_found);
        assert!(record.found >= record.started);
        let share = run.share(index);
        assert_eq!(share.height as usize, index + 1);
        assert_eq!(
            share.time,
            settings.genesis_time + record.found.floor() as u32
        );
        previous_found = record.found;
    }
    let mean = run.end / run.records.len() as f64;
    assert!((8.0..12.0).contains(&mean), "{mean}");
}

#[test]
fn a_time_run_stops_at_its_end_with_all_its_work_counted() {
    let hashrate = PETAHASH;
    let scenario = single_group(None, Schedule::Constant(hashrate), 3_600.0);
    let run = simulate(&scenario, Rule::ParentAnchor, 3, &Settings::default());
    assert_eq!(run.end, 3_600.0);
    assert!(run.records.iter().all(|record| record.found < 3_600.0));
    let expected_work = hashrate * 3_600.0 / HASHES_PER_DIFFICULTY;
    assert!(relative_difference(run.group_work[0], expected_work) < 1e-9);
    // Without a stratum bound every unit of work is credited on average.
    assert!(relative_difference(run.group_credit[0], run.group_work[0]) < 1e-9);
}

#[test]
fn nobody_mines_through_a_quiet_period() {
    let scenario = single_group(
        Some(FARM_ASIC),
        Schedule::TimeSteps(vec![(0.0, PETAHASH), (600.0, 0.0), (4_200.0, PETAHASH)]),
        6_000.0,
    );
    let run = simulate(&scenario, Rule::ParentAnchor, 5, &Settings::default());
    assert!(
        run.records
            .iter()
            .all(|record| record.found < 600.0 || record.found >= 4_200.0)
    );
    let mining_seconds = 600.0 + (6_000.0 - 4_200.0);
    let expected_work = PETAHASH * mining_seconds / HASHES_PER_DIFFICULTY;
    assert!(relative_difference(run.group_work[0], expected_work) < 1e-9);
    // The share mined across the gap took at least the hour.
    assert!(
        run.records
            .iter()
            .any(|record| record.started < 600.0 && record.found >= 4_200.0)
    );
}

#[test]
fn a_run_ends_when_nobody_will_mine_again() {
    let scenario = Scenario {
        length: Length::Shares(100_000),
        ..single_group(
            None,
            Schedule::TimeSteps(vec![(0.0, PETAHASH), (300.0, 0.0)]),
            0.0,
        )
    };
    let run = simulate(&scenario, Rule::ParentAnchor, 9, &Settings::default());
    assert!(run.records.len() < 2_000);
    assert!(run.records.iter().all(|record| record.found < 300.0));
}

/// Ten 200 TH/s machines submit three times a second. Below their stratum
/// difficulty every submission is a share, so they cannot outpace that,
/// and most of their work earns only the share difficulty.
#[test]
fn shares_cannot_outpace_stratum_submissions() {
    let hashrate = 10.0 * FARM_ASIC;
    let settings = Settings::default();
    let bounded = single_group(Some(FARM_ASIC), Schedule::Constant(hashrate), 60.0);
    let unbounded = single_group(None, Schedule::Constant(hashrate), 60.0);
    let bounded_run = simulate(&bounded, Rule::ParentAnchor, 11, &settings);
    let unbounded_run = simulate(&unbounded, Rule::ParentAnchor, 11, &settings);
    // About 180 submissions in a minute.
    assert!(
        (120..=250).contains(&bounded_run.records.len()),
        "{}",
        bounded_run.records.len()
    );
    assert!(unbounded_run.records.len() > 2 * bounded_run.records.len());
    let credited = bounded_run.group_credit[0] / bounded_run.group_work[0];
    let floor = stratum_difficulty(FARM_ASIC, &settings);
    assert!(credited < 0.1, "{credited}");
    let tip = bounded_run.chain.tip().difficulty;
    assert!(credited <= tip / floor, "{credited} {tip} {floor}");
}

#[test]
fn stratum_difficulty_stays_within_the_session_limits() {
    let settings = Settings::default();
    assert_eq!(stratum_difficulty(1.0, &settings), 1.0);
    assert_eq!(stratum_difficulty(1e30, &settings), 4_294_967_296.0);
    // A 200 TH/s machine settles near 155,200.
    let farm = stratum_difficulty(FARM_ASIC, &settings);
    assert!((farm - 155_200.0).abs() < 100.0, "{farm}");
}

#[test]
fn the_group_that_finds_a_share_is_drawn_by_share_rate() {
    let scenario = Scenario {
        groups: vec![
            MinerGroup {
                name: "small",
                device_hashrate: None,
                schedule: Schedule::Constant(PETAHASH),
            },
            MinerGroup {
                name: "large",
                device_hashrate: None,
                schedule: Schedule::Constant(3.0 * PETAHASH),
            },
        ],
        genesis: Genesis::Regulated {
            hashrate: 4.0 * PETAHASH,
        },
        ..single_group(None, Schedule::Constant(0.0), 20_000.0)
    };
    let run = simulate(&scenario, Rule::ParentAnchor, 13, &Settings::default());
    let small = run
        .records
        .iter()
        .filter(|record| record.group == 0)
        .count() as f64;
    let fraction = small / run.records.len() as f64;
    assert!((fraction - 0.25).abs() < 0.04, "{fraction}");
}
