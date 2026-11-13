// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;

#[test]
fn every_table_is_found_by_its_name() {
    assert!(
        Table::ALL
            .iter()
            .all(|table| Table::parse(table.name()) == Ok(*table))
    );
}

#[test]
fn an_unknown_table_name_lists_the_known_ones() {
    let error = Table::parse("no-such-table").unwrap_err();
    assert!(error.contains("designs"));
    assert!(error.contains("attack-spam"));
}

#[test]
fn scenarios_run_in_parallel_come_back_in_order() {
    let base = base_scenario(1, 1);
    let scenarios = vec![
        Scenario {
            design: Design::OneChain,
            ..base.clone()
        },
        Scenario {
            design: Design::Mini,
            ..base.clone()
        },
        Scenario {
            design: Design::Vardiff,
            ..base
        },
    ];
    let reports = run_scenarios(&scenarios);
    assert_eq!(reports[0].scenario.design, Design::OneChain);
    assert_eq!(reports[1].scenario.design, Design::Mini);
    assert_eq!(reports[2].scenario.design, Design::Vardiff);
}

#[test]
fn a_run_is_a_pure_function_of_its_scenario_and_seed() {
    let scenario = Scenario {
        design: Design::BandWeakLevels,
        weak_queue_shares: 10_000,
        ..base_scenario(3, 9)
    };
    let first = scenario.run();
    let second = scenario.run();
    assert_eq!(
        first.miners[0].payments_per_month,
        second.miners[0].payments_per_month
    );
    assert_eq!(
        first.miners[0].income_per_month,
        second.miners[0].income_per_month
    );
}

#[test]
fn parallel_map_keeps_the_items_order() {
    let items: Vec<u64> = (0..100).collect();
    let squares = parallel_map(&items, |item| item * item);
    assert_eq!(squares.len(), 100);
    assert_eq!(squares[0], 0);
    assert_eq!(squares[7], 49);
    assert_eq!(squares[99], 9_801);
}

#[test]
fn the_study_tables_are_in_the_default_run() {
    assert!(Table::ALL.contains(&Table::RefusingRaces));
    assert!(Table::ALL.contains(&Table::RatioMax));
    assert_eq!(Table::parse("refusing-races"), Ok(Table::RefusingRaces));
    assert_eq!(Table::parse("r-max"), Ok(Table::RatioMax));
}
