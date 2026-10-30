// SPDX-License-Identifier: AGPL-3.0-or-later

use p2poolv2_photon_payout::simulation::{
    CommittedOwed, Offered, Payment, Population, Scheduling, SimulationConfig, SplitCost,
    SplitFailure, TAIL_SPAN, allocate_committed, plan_queue_list, simulate,
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

fn offer(miner: usize, cap: u64) -> Offered {
    Offered {
        miner,
        cap,
        multiple: 10,
    }
}

/// The list must pay its whole reward at the largest and smallest tail.
fn pays_every_tail(committed: &[CommittedOwed], reward_maximum: u64) {
    for reward in [reward_maximum - TAIL_SPAN, reward_maximum] {
        let paid: u64 = allocate_committed(committed, reward, 10)
            .unwrap()
            .iter()
            .map(|payment| payment.gross)
            .sum();
        assert_eq!(paid, reward);
    }
}

#[test]
fn queue_pays_first_eligible_before_larger_later_debt() {
    let queue = [offer(7, 200_000), offer(3, 900_000)];
    let committed = plan_queue_list(&queue, &[], 64, 10, 1_000_000).unwrap();
    assert_eq!(
        committed
            .iter()
            .map(|entry| entry.miner)
            .collect::<Vec<_>>(),
        vec![7, 3]
    );
    assert_eq!(committed[0].owed, 200_000);
    pays_every_tail(&committed, 1_000_000);
}

#[test]
fn remainder_after_the_queue_is_paid_ahead_within_bounds() {
    let queue = [offer(0, 300_000)];
    let ahead = [offer(1, 500_000), offer(2, 400_000)];
    let committed = plan_queue_list(&queue, &ahead, 64, 10, 1_000_000).unwrap();
    assert_eq!(committed.len(), 3);
    assert_eq!(committed[1].owed, 500_000);
    // The last entry receives no more than its own bound at the top tail.
    assert_eq!(committed[2].owed, 200_000);
    pays_every_tail(&committed, 1_000_000);
}

#[test]
fn small_leftover_goes_to_the_entry_before_it() {
    // After 950,000 listed, the smallest reward leaves less than the next
    // miner's 100,000 minimum, so miner 1 absorbs the leftover instead.
    let queue = [offer(0, 500_000), offer(1, 450_000)];
    let ahead = [Offered {
        miner: 2,
        cap: 400_000,
        multiple: 10_000,
    }];
    let committed = plan_queue_list(&queue, &ahead, 64, 10, 1_000_000).unwrap();
    assert_eq!(committed.len(), 2);
    assert_eq!(committed[1].owed, 500_000);
    pays_every_tail(&committed, 1_000_000);
}

#[test]
fn queue_yields_slots_to_cover_the_reward() {
    // Two small queued debts cannot cover; one slot goes to a large offer.
    let queue = [offer(0, 100_000), offer(1, 100_000)];
    let ahead = [offer(2, 950_000)];
    let committed = plan_queue_list(&queue, &ahead, 2, 10, 1_000_000).unwrap();
    assert_eq!(
        committed
            .iter()
            .map(|entry| entry.miner)
            .collect::<Vec<_>>(),
        vec![0, 2]
    );
    pays_every_tail(&committed, 1_000_000);
}

#[test]
fn uncoverable_list_is_refused() {
    let queue = [offer(0, 100_000)];
    assert!(plan_queue_list(&queue, &[offer(1, 100_000)], 64, 10, 1_000_000).is_none());
}

#[test]
fn candidate_genesis_pays_without_history_or_custody() {
    let mut config = SimulationConfig::candidate(100, Population::Giant);
    config.opportunities = 60;
    let report = simulate(&config, |_, _| Ok(SplitCost::default())).unwrap();
    assert_eq!(report.historical_tokens, 0);
    assert!(report.gross_paid_tokens > 0);
    assert_eq!(report.permanently_stranded_tokens, 0);
}

#[test]
fn candidate_signed_ledger_conserves_won_tokens() {
    let mut config = SimulationConfig::candidate(200, Population::LogUniform);
    config.opportunities = 200;
    config.delayed_funding_every = 3;
    let report = simulate(&config, |_, _| Ok(SplitCost::default())).unwrap();
    assert_eq!(
        i128::from(report.gross_paid_tokens) + i128::from(report.unpaid_tokens)
            - i128::from(report.advanced_tokens)
            + i128::from(report.unallocated_tokens),
        i128::from(report.total_won_tokens)
    );
    assert!(report.peak_advanced_tokens >= report.advanced_tokens);
    assert!(report.reimbursement_tokens <= report.gross_paid_tokens / 10);
}

#[test]
fn candidate_seed_fixes_the_complete_report() {
    let mut config = SimulationConfig::candidate(50, Population::HeavyTailed);
    config.opportunities = 40;
    let first = simulate(&config, |_, _| Ok(SplitCost::default())).unwrap();
    assert_eq!(
        first,
        simulate(&config, |_, _| Ok(SplitCost::default())).unwrap()
    );
}
