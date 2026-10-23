// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use crate::difficulty::difficulty_of;

#[test]
fn height_schedules_follow_the_share_height() {
    let cycle = Schedule::HeightCycle {
        period: 20,
        on_shares: 10,
        on: 10.0,
        off: 1.0,
    };
    assert_eq!(cycle.hashrate(0, 0.0, 1), 10.0);
    assert_eq!(cycle.hashrate(9, 0.0, 1), 10.0);
    assert_eq!(cycle.hashrate(10, 0.0, 1), 1.0);
    assert_eq!(cycle.hashrate(20, 0.0, 1), 10.0);

    let step = Schedule::HeightStep {
        at: 1_000,
        before: 10.0,
        after: 1.0,
    };
    assert_eq!(step.hashrate(999, 0.0, 1), 10.0);
    assert_eq!(step.hashrate(1_000, 0.0, 1), 1.0);

    let linear = Schedule::HeightLinear {
        from: 1.0,
        to: 10.0,
        over: 5_000,
    };
    assert_eq!(linear.hashrate(0, 0.0, 1), 1.0);
    assert_eq!(linear.hashrate(2_500, 0.0, 1), 5.5);
    assert_eq!(linear.hashrate(5_000, 0.0, 1), 10.0);

    for schedule in [cycle, step, linear] {
        assert_eq!(schedule.next_change(0.0), None);
    }
}

#[test]
fn noise_is_fixed_by_seed_and_height_and_stays_in_range() {
    let noise = Schedule::HeightNoise {
        low: 0.5,
        high: 2.0,
    };
    let draws: Vec<f64> = (0..2_000)
        .map(|height| noise.hashrate(height, 0.0, 42))
        .collect();
    assert!(draws.iter().all(|draw| (0.5..2.0).contains(draw)));
    let mean = draws.iter().sum::<f64>() / draws.len() as f64;
    assert!((mean - 1.25).abs() < 0.05, "{mean}");
    assert_eq!(noise.hashrate(17, 99.0, 42), draws[17]);
    assert_ne!(noise.hashrate(17, 0.0, 43), draws[17]);
}

#[test]
fn time_steps_change_exactly_at_their_starts() {
    let steps = Schedule::TimeSteps(vec![(100.0, 5.0), (200.0, 0.0), (300.0, 7.0)]);
    assert_eq!(steps.hashrate(0, 99.9, 1), 0.0);
    assert_eq!(steps.hashrate(0, 100.0, 1), 5.0);
    assert_eq!(steps.hashrate(0, 250.0, 1), 0.0);
    assert_eq!(steps.hashrate(0, 1e9, 1), 7.0);
    assert_eq!(steps.next_change(0.0), Some(100.0));
    assert_eq!(steps.next_change(100.0), Some(200.0));
    assert_eq!(steps.next_change(250.0), Some(300.0));
    assert_eq!(steps.next_change(300.0), None);
}

#[test]
fn time_cycles_switch_on_and_off_from_their_start() {
    let cycle = Schedule::TimeCycle {
        start: 100.0,
        period: 60.0,
        on_seconds: 20.0,
        on: 9.0,
        off: 1.0,
    };
    assert_eq!(cycle.hashrate(0, 50.0, 1), 1.0);
    assert_eq!(cycle.hashrate(0, 100.0, 1), 9.0);
    assert_eq!(cycle.hashrate(0, 119.9, 1), 9.0);
    assert_eq!(cycle.hashrate(0, 120.0, 1), 1.0);
    assert_eq!(cycle.hashrate(0, 160.0, 1), 9.0);
    let mut boundaries = Vec::new();
    let mut time = 0.0;
    while let Some(next) = cycle.next_change(time).filter(|next| *next < 300.0) {
        boundaries.push(next);
        time = next;
    }
    assert_eq!(
        boundaries,
        [100.0, 120.0, 160.0, 180.0, 220.0, 240.0, 280.0]
    );
}

#[test]
fn exponential_growth_runs_from_start_to_end_in_steps() {
    let growth = exponential_growth(1e12, 1e18, 3_600.0, 6.0 * HOUR, 10.0 * MINUTE);
    let Schedule::TimeSteps(steps) = &growth else {
        panic!("growth is a time schedule");
    };
    assert_eq!(steps.len(), 37);
    assert_eq!(steps[0], (3_600.0, 1e12));
    let (last_time, last_rate) = steps[steps.len() - 1];
    assert_eq!(last_time, 3_600.0 + 6.0 * HOUR);
    assert!((last_rate / 1e18 - 1.0).abs() < 1e-9);
    assert!(steps.windows(2).all(|pair| pair[1].1 > pair[0].1));
    assert_eq!(growth.hashrate(0, 0.0, 1), 0.0);
}

#[test]
fn genesis_bits_follow_the_share_chain() {
    assert_eq!(
        Genesis::Main.bits(),
        CompactTarget::from_consensus(MAIN_ANCHOR_TARGET)
    );
    assert_eq!(
        Genesis::Mini.bits(),
        CompactTarget::from_consensus(MINI_ANCHOR_TARGET)
    );
    let regulated = Genesis::Regulated {
        hashrate: UPSTREAM_UNIT_HASHRATE,
    }
    .bits();
    let steady = steady_difficulty(UPSTREAM_UNIT_HASHRATE);
    assert!((difficulty_of(regulated) / steady - 1.0).abs() < 1e-4);
}

#[test]
fn upstream_suite_matches_upstream_runs() {
    let suite = upstream_suite();
    assert_eq!(suite.len(), 6);
    for scenario in &suite {
        assert_eq!(scenario.length, Length::Shares(5_000));
        assert_eq!(scenario.warmup_shares, 100);
        assert_eq!(scenario.groups.len(), 1);
        assert_eq!(scenario.groups[0].device_hashrate, None);
    }
    let attacks: Vec<&str> = suite
        .iter()
        .filter(|scenario| scenario.attack)
        .map(|scenario| scenario.id)
        .collect();
    assert_eq!(attacks, ["oscillation-100", "oscillation-20", "drop-90"]);
}

#[test]
fn bch_scenarios_bound_every_group_by_its_stratum_sessions() {
    for scenario in bch_suite() {
        assert!(
            matches!(scenario.length, Length::Seconds(_)),
            "{}",
            scenario.id
        );
        assert!(!scenario.attack, "{}", scenario.id);
        for group in &scenario.groups {
            assert!(group.device_hashrate.is_some(), "{}", scenario.id);
        }
        assert!(
            scenario.events.windows(2).all(|pair| pair[0] < pair[1]),
            "{}",
            scenario.id
        );
    }
}

#[test]
fn scenario_ids_are_unique() {
    let mut ids: Vec<&str> = upstream_suite()
        .iter()
        .chain(bch_suite().iter())
        .map(|scenario| scenario.id)
        .collect();
    let count = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), count);
}

#[test]
fn the_mini_chain_miners_start_and_stop_together() {
    let miners = mini_miners(&[(0.0, true), (10.0, false), (20.0, true)]);
    assert_eq!(miners[0].name, "CPU miner");
    assert_eq!(miners[1].name, "ASIC");
    for (group, hashrate) in miners.iter().zip([CPU_MINER, CHIPNET_ASIC]) {
        assert_eq!(group.device_hashrate, Some(hashrate));
        assert_eq!(group.schedule.hashrate(0, 5.0, 1), hashrate);
        assert_eq!(group.schedule.hashrate(0, 15.0, 1), 0.0);
        assert_eq!(group.schedule.hashrate(0, 25.0, 1), hashrate);
    }
}

#[test]
fn total_hashrate_adds_every_group() {
    let scenario = bch_suite()
        .into_iter()
        .find(|scenario| scenario.id == "switch-4h")
        .expect("switch-4h is a Bitcoin Cash scenario");
    assert_eq!(scenario.total_hashrate(0, 0.0, 1), 50.0 * PETAHASH);
    assert_eq!(
        scenario.total_hashrate(0, 2.0 * HOUR, 1),
        50.0 * PETAHASH + 2.0 * EXAHASH
    );
    assert_eq!(scenario.total_hashrate(0, 6.0 * HOUR, 1), 50.0 * PETAHASH);
    // Two hours before the first switch, then three days.
    assert_eq!(scenario.expected_shares(10.0), 26_641);
}
