// SPDX-License-Identifier: AGPL-3.0-or-later

use p2poolv2_photon_payout::simulation::{
    CommittedOwed, Payment, Population, Scheduling, SimulationConfig, SplitCost, SplitFailure,
    allocate_committed, simulate,
};

#[test]
fn exact_reward_is_paid_without_advancing_debt() {
    let committed = [
        CommittedOwed {
            miner: 0,
            owed: 200,
            multiple: 10,
        },
        CommittedOwed {
            miner: 1,
            owed: 100,
            multiple: 10,
        },
    ];
    assert_eq!(
        allocate_committed(&committed, 300, 10).unwrap(),
        vec![
            Payment {
                miner: 0,
                gross: 200
            },
            Payment {
                miner: 1,
                gross: 100
            },
        ]
    );
}

#[test]
fn final_clip_must_meet_its_own_minimum() {
    let committed = [
        CommittedOwed {
            miner: 0,
            owed: 200,
            multiple: 10,
        },
        CommittedOwed {
            miner: 1,
            owed: 100,
            multiple: 10,
        },
    ];
    assert_eq!(
        allocate_committed(&committed, 250, 10),
        Err(SplitFailure::MinimumRemainder)
    );
}

#[test]
fn a_frozen_list_cannot_spend_later_credit() {
    let committed = [CommittedOwed {
        miner: 0,
        owed: 100,
        multiple: 10,
    }];
    assert_eq!(
        allocate_committed(&committed, 200, 10),
        Err(SplitFailure::Coverage)
    );
}

#[test]
fn trailing_owed_candidates_do_not_receive_zero_outputs() {
    let committed = [
        CommittedOwed {
            miner: 0,
            owed: 200,
            multiple: 10,
        },
        CommittedOwed {
            miner: 1,
            owed: 100,
            multiple: 10,
        },
    ];
    assert_eq!(
        allocate_committed(&committed, 200, 10).unwrap(),
        vec![Payment {
            miner: 0,
            gross: 200
        }]
    );
}

#[test]
fn strict_genesis_cannot_create_unbacked_bootstrap_credit() {
    let mut config = SimulationConfig::study(40, Population::Giant);
    config.historical_wins = 0;
    config.opportunities = 20;
    config.scheduling = Scheduling::Strict;
    let report = simulate(&config, |_, _| Ok(SplitCost::default())).unwrap();
    assert_eq!(report.total_won_tokens, 0);
    assert_eq!(report.gross_paid_tokens, 0);
    assert_eq!(report.blocked_coverage, 20);
    assert_eq!(report.unpaid_tokens, 0);
}

#[test]
fn seeded_churn_and_delays_conserve_all_won_tokens() {
    let mut config = SimulationConfig::study(100, Population::LogUniform);
    config.historical_wins = 10;
    config.opportunities = 100;
    config.delayed_funding_every = 2;
    let report = simulate(&config, |_, _| Ok(SplitCost::default())).unwrap();
    assert_eq!(
        report.gross_paid_tokens + report.unpaid_tokens + report.unallocated_tokens,
        report.total_won_tokens
    );
    assert!(report.reserved_tokens <= report.unpaid_tokens);
    assert!(report.outside_claims > 0);
    assert_eq!(
        report.total_won_tokens + report.outside_emitted_tokens + report.ending_vault_tokens,
        config.vault_tokens
    );
    assert!(report.reimbursement_tokens <= report.gross_paid_tokens / 10);
}

#[test]
fn full_funder_refusal_preserves_debt_and_immutable_reservations() {
    let mut config = SimulationConfig::study(100, Population::Giant);
    config.historical_wins = 10;
    config.opportunities = 100;
    config.funding_enabled = false;
    let report = simulate(&config, |_, _| Ok(SplitCost::default())).unwrap();
    assert_eq!(report.gross_paid_tokens, 0);
    assert_eq!(
        report.unpaid_tokens + report.unallocated_tokens,
        report.total_won_tokens
    );
    assert!(report.reserved_tokens > 0);
    assert!(report.outstanding_fundable_roots > 0);
}

#[test]
fn seed_fixes_the_complete_economic_report() {
    let mut config = SimulationConfig::study(30, Population::HeavyTailed);
    config.historical_wins = 5;
    config.opportunities = 30;
    let first = simulate(&config, |_, _| Ok(SplitCost::default())).unwrap();
    let repeated = simulate(&config, |_, _| Ok(SplitCost::default())).unwrap();
    assert_eq!(first, repeated);
}

#[test]
fn unsupported_emission_amount_epoch_fails_closed() {
    let mut config = SimulationConfig::study(30, Population::HeavyTailed);
    config.vault_tokens = 420_000 * (1u64 << 32);
    assert!(simulate(&config, |_, _| Ok(SplitCost::default())).is_err());
}
