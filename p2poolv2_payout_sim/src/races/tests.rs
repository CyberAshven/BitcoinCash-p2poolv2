// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;

#[test]
fn the_share_carrying_more_weak_work_wins_when_weak_work_counts() {
    let carrier = Racer {
        found_rate: 1.0,
        on_carrying_node: true,
        work_multiple: 1.0,
    };
    let refuser = Racer {
        found_rate: 1.0,
        on_carrying_node: false,
        work_multiple: 1.0,
    };
    let carried = Carried {
        carrying: 30,
        non_carrying: 0,
    };
    let refused = Carried {
        carrying: 0,
        non_carrying: 5,
    };
    let mut random = SimulationRandom::from_seed(1);
    assert!(!first_wins_race(
        (&refuser, refused),
        (&carrier, carried),
        35.0,
        ForkChoice::WeakWork,
        &mut random,
    ));
}

#[test]
fn the_heavier_share_wins_under_share_work_whatever_it_carries() {
    let light = Racer {
        found_rate: 1.0,
        on_carrying_node: true,
        work_multiple: 1.0,
    };
    let heavy = Racer {
        found_rate: 1.0,
        on_carrying_node: false,
        work_multiple: 2.0,
    };
    let full = Carried {
        carrying: 60,
        non_carrying: 0,
    };
    let mut random = SimulationRandom::from_seed(1);
    assert!(!first_wins_race(
        (&light, full),
        (&heavy, Carried::default()),
        35.0,
        ForkChoice::ShareWork,
        &mut random,
    ));
}

#[test]
fn carried_weak_work_can_outweigh_a_heavier_share_when_it_counts() {
    // 1 + 60 / 35 = 2.71 against 2.
    let light = Racer {
        found_rate: 1.0,
        on_carrying_node: true,
        work_multiple: 1.0,
    };
    let heavy = Racer {
        found_rate: 1.0,
        on_carrying_node: false,
        work_multiple: 2.0,
    };
    let full = Carried {
        carrying: 60,
        non_carrying: 0,
    };
    let mut random = SimulationRandom::from_seed(1);
    assert!(first_wins_race(
        (&light, full),
        (&heavy, Carried::default()),
        35.0,
        ForkChoice::WeakWork,
        &mut random,
    ));
}

#[test]
fn equal_share_work_is_a_coin_toss_under_share_work() {
    let carrier = Racer {
        found_rate: 1.0,
        on_carrying_node: true,
        work_multiple: 1.0,
    };
    let refuser = Racer {
        found_rate: 1.0,
        on_carrying_node: false,
        work_multiple: 1.0,
    };
    let carried = Carried {
        carrying: 60,
        non_carrying: 0,
    };
    let refused = Carried {
        carrying: 0,
        non_carrying: 5,
    };
    let mut random = SimulationRandom::from_seed(5);
    let carrier_wins = (0..10_000)
        .filter(|_| {
            first_wins_race(
                (&carrier, carried),
                (&refuser, refused),
                35.0,
                ForkChoice::ShareWork,
                &mut random,
            )
        })
        .count();
    assert!(
        carrier_wins > 4_800 && carrier_wins < 5_200,
        "{carrier_wins}"
    );
}

#[test]
fn a_refusing_tip_carrying_less_than_an_honest_share_would_is_light() {
    let refuser = Racer {
        found_rate: 1.0,
        on_carrying_node: false,
        work_multiple: 1.0,
    };
    let refused = Carried {
        carrying: 0,
        non_carrying: 10,
    };
    let honest = Carried {
        carrying: 24,
        non_carrying: 10,
    };
    assert!(is_light_tip(&refuser, refused, honest, 35.0));
}

#[test]
fn a_carrying_nodes_tip_is_never_light() {
    let carrier = Racer {
        found_rate: 1.0,
        on_carrying_node: true,
        work_multiple: 1.0,
    };
    let honest = Carried {
        carrying: 34,
        non_carrying: 0,
    };
    assert!(!is_light_tip(&carrier, Carried::default(), honest, 35.0));
}

#[test]
fn a_heavy_refusing_tip_is_not_light() {
    // 3 against 1 + 34 / 35: no share at the minimum outweighs it.
    let heavy = Racer {
        found_rate: 1.0,
        on_carrying_node: false,
        work_multiple: 3.0,
    };
    let honest = Carried {
        carrying: 34,
        non_carrying: 0,
    };
    assert!(!is_light_tip(&heavy, Carried::default(), honest, 35.0));
}

#[test]
fn without_refusers_or_heavier_shares_every_class_goes_stale_at_the_stale_rate() {
    let carrier = Racer {
        found_rate: 1.0,
        on_carrying_node: true,
        work_multiple: 1.0,
    };
    let inputs = RaceQueueInputs {
        queue: WeakQueueInputs {
            carrying_minimum_shares_per_period: 1.0,
            non_carrying_minimum_shares_per_period: 0.0,
            carrying_share_fraction: 1.0,
            weak_cap: 64,
            weak_fill: 32,
            weak_depth: 6,
            ratio_max: 4_096,
            ceiling_ratio: 1.0e9,
            shares: 200_000,
            warmup_shares: 20_000,
        },
        racers: vec![carrier, carrier],
        settings: RaceSettings {
            fork_choice: ForkChoice::WeakWork,
            races_per_share: 0.021 / 0.979,
            stale_parent_loss: 0.0,
            uncle_carrier_drops: false,
        },
    };
    let outcome = simulate_race_queue(&inputs, &mut SimulationRandom::from_seed(2));
    assert!(
        (outcome.stale_fraction[0] - 0.021).abs() < 0.002,
        "{:?}",
        outcome.stale_fraction
    );
    assert!(
        (outcome.stale_fraction[1] - 0.021).abs() < 0.002,
        "{:?}",
        outcome.stale_fraction
    );
    assert_eq!(outcome.replaced_per_share, 0.0);
}

#[test]
fn heavier_shares_go_stale_less_under_share_work() {
    let light = Racer {
        found_rate: 1.0,
        on_carrying_node: true,
        work_multiple: 1.0,
    };
    let heavy = Racer {
        found_rate: 1.0,
        on_carrying_node: true,
        work_multiple: 8.0,
    };
    let inputs = RaceQueueInputs {
        queue: WeakQueueInputs {
            carrying_minimum_shares_per_period: 0.5,
            non_carrying_minimum_shares_per_period: 0.0,
            carrying_share_fraction: 1.0,
            weak_cap: 64,
            weak_fill: 32,
            weak_depth: 6,
            ratio_max: 4_096,
            ceiling_ratio: 1.0e9,
            shares: 100_000,
            warmup_shares: 20_000,
        },
        racers: vec![light, heavy],
        settings: RaceSettings {
            fork_choice: ForkChoice::ShareWork,
            races_per_share: 0.021 / 0.979,
            stale_parent_loss: 0.0,
            uncle_carrier_drops: false,
        },
    };
    let outcome = simulate_race_queue(&inputs, &mut SimulationRandom::from_seed(3));
    assert!(
        outcome.stale_fraction[1] < outcome.stale_fraction[0] / 2.0,
        "{:?}",
        outcome.stale_fraction
    );
}

#[test]
fn refusers_lose_their_races_when_weak_work_counts() {
    let carrier = Racer {
        found_rate: 1.0,
        on_carrying_node: true,
        work_multiple: 1.0,
    };
    let refuser = Racer {
        found_rate: 1.0,
        on_carrying_node: false,
        work_multiple: 1.0,
    };
    let inputs = RaceQueueInputs {
        queue: WeakQueueInputs {
            carrying_minimum_shares_per_period: 0.5,
            non_carrying_minimum_shares_per_period: 0.5,
            carrying_share_fraction: 0.5,
            weak_cap: 64,
            weak_fill: 32,
            weak_depth: 6,
            ratio_max: 4_096,
            ceiling_ratio: 1.0e9,
            shares: 100_000,
            warmup_shares: 20_000,
        },
        racers: vec![carrier, refuser],
        settings: RaceSettings {
            fork_choice: ForkChoice::WeakWork,
            races_per_share: 0.021 / 0.979,
            stale_parent_loss: 0.0,
            uncle_carrier_drops: false,
        },
    };
    let outcome = simulate_race_queue(&inputs, &mut SimulationRandom::from_seed(4));
    // A refuser loses every race against a carrier: about 3/4 of the
    // races it is in, against 1/4 for a carrier.
    assert!(
        outcome.stale_fraction[1] > 2.0 * outcome.stale_fraction[0],
        "{:?}",
        outcome.stale_fraction
    );
}

#[test]
fn refusers_lose_half_their_races_when_only_share_work_counts() {
    let carrier = Racer {
        found_rate: 1.0,
        on_carrying_node: true,
        work_multiple: 1.0,
    };
    let refuser = Racer {
        found_rate: 1.0,
        on_carrying_node: false,
        work_multiple: 1.0,
    };
    let inputs = RaceQueueInputs {
        queue: WeakQueueInputs {
            carrying_minimum_shares_per_period: 0.5,
            non_carrying_minimum_shares_per_period: 0.5,
            carrying_share_fraction: 0.5,
            weak_cap: 64,
            weak_fill: 32,
            weak_depth: 6,
            ratio_max: 4_096,
            ceiling_ratio: 1.0e9,
            shares: 100_000,
            warmup_shares: 20_000,
        },
        racers: vec![carrier, refuser],
        settings: RaceSettings {
            fork_choice: ForkChoice::ShareWork,
            races_per_share: 0.021 / 0.979,
            stale_parent_loss: 0.0,
            uncle_carrier_drops: false,
        },
    };
    let outcome = simulate_race_queue(&inputs, &mut SimulationRandom::from_seed(4));
    assert!(
        (outcome.stale_fraction[1] - outcome.stale_fraction[0]).abs() < 0.004,
        "{:?}",
        outcome.stale_fraction
    );
}

#[test]
fn carriers_mining_siblings_replace_light_refusing_tips() {
    let carrier = Racer {
        found_rate: 1.0,
        on_carrying_node: true,
        work_multiple: 1.0,
    };
    let refuser = Racer {
        found_rate: 1.0,
        on_carrying_node: false,
        work_multiple: 1.0,
    };
    let queue = WeakQueueInputs {
        carrying_minimum_shares_per_period: 0.5,
        non_carrying_minimum_shares_per_period: 0.5,
        carrying_share_fraction: 0.5,
        weak_cap: 64,
        weak_fill: 32,
        weak_depth: 6,
        ratio_max: 4_096,
        ceiling_ratio: 1.0e9,
        shares: 100_000,
        warmup_shares: 20_000,
    };
    let plain = simulate_race_queue(
        &RaceQueueInputs {
            queue,
            racers: vec![carrier, refuser],
            settings: RaceSettings {
                fork_choice: ForkChoice::WeakWork,
                races_per_share: 0.021 / 0.979,
                stale_parent_loss: 0.0,
                uncle_carrier_drops: false,
            },
        },
        &mut SimulationRandom::from_seed(6),
    );
    let siblings = simulate_race_queue(
        &RaceQueueInputs {
            queue,
            racers: vec![carrier, refuser],
            settings: RaceSettings {
                fork_choice: ForkChoice::WeakWorkSiblings,
                races_per_share: 0.021 / 0.979,
                stale_parent_loss: 0.0,
                uncle_carrier_drops: false,
            },
        },
        &mut SimulationRandom::from_seed(6),
    );
    // Half the tips come from refusers and half of the next shares from
    // carriers: about a quarter of the steps replace a tip.
    assert!(
        siblings.replaced_per_share > 0.15,
        "{}",
        siblings.replaced_per_share
    );
    let honest_plain = plain.queue.carrying_expired_fraction + plain.carrying_lost_fraction;
    let honest_siblings =
        siblings.queue.carrying_expired_fraction + siblings.carrying_lost_fraction;
    assert!(
        honest_siblings < honest_plain,
        "{honest_siblings} {honest_plain}"
    );
    // A replaced tip takes the weak shares its own miners found on it.
    assert!(siblings.non_carrying_lost_fraction > 0.0);
    assert!(siblings.stale_fraction[1] > 5.0 * plain.stale_fraction[1]);
}

#[test]
fn a_stale_parent_loses_its_share_of_the_weak_shares_found() {
    let carrier = Racer {
        found_rate: 1.0,
        on_carrying_node: true,
        work_multiple: 1.0,
    };
    let inputs = RaceQueueInputs {
        queue: WeakQueueInputs {
            carrying_minimum_shares_per_period: 1.0,
            non_carrying_minimum_shares_per_period: 0.0,
            carrying_share_fraction: 1.0,
            weak_cap: 64,
            weak_fill: 32,
            weak_depth: 6,
            ratio_max: 4_096,
            ceiling_ratio: 1.0e9,
            shares: 100_000,
            warmup_shares: 20_000,
        },
        racers: vec![carrier],
        settings: RaceSettings {
            fork_choice: ForkChoice::ShareWork,
            races_per_share: 0.021 / 0.979,
            stale_parent_loss: 0.05,
            uncle_carrier_drops: false,
        },
    };
    let outcome = simulate_race_queue(&inputs, &mut SimulationRandom::from_seed(7));
    assert!(
        (outcome.carrying_lost_fraction - 0.05).abs() < 0.003,
        "{}",
        outcome.carrying_lost_fraction
    );
}

#[test]
fn race_losers_keep_their_weak_shares_waiting_by_default() {
    let carrier = Racer {
        found_rate: 1.0,
        on_carrying_node: true,
        work_multiple: 1.0,
    };
    let inputs = RaceQueueInputs {
        queue: WeakQueueInputs {
            carrying_minimum_shares_per_period: 1.0,
            non_carrying_minimum_shares_per_period: 0.0,
            carrying_share_fraction: 1.0,
            weak_cap: 64,
            weak_fill: 32,
            weak_depth: 6,
            ratio_max: 4_096,
            ceiling_ratio: 1.0e9,
            shares: 100_000,
            warmup_shares: 20_000,
        },
        racers: vec![carrier],
        settings: RaceSettings {
            fork_choice: ForkChoice::ShareWork,
            races_per_share: 0.021 / 0.979,
            stale_parent_loss: 0.0,
            uncle_carrier_drops: false,
        },
    };
    let outcome = simulate_race_queue(&inputs, &mut SimulationRandom::from_seed(8));
    assert_eq!(outcome.carrying_lost_fraction, 0.0);
}

#[test]
fn a_refuser_beating_a_carrier_drops_the_honest_weak_shares_the_carrier_took() {
    // Two carriers take the same oldest weak shares, so a losing carrier
    // drops nothing the winner did not carry. A refuser that wins a race
    // carried only its own nodes' weak shares; the losing carrier took the
    // honest ones too, and dropping them with it loses them.
    let carrier = Racer {
        found_rate: 1.0,
        on_carrying_node: true,
        work_multiple: 1.0,
    };
    let refuser = Racer {
        found_rate: 1.0,
        on_carrying_node: false,
        work_multiple: 1.0,
    };
    let inputs = RaceQueueInputs {
        queue: WeakQueueInputs {
            carrying_minimum_shares_per_period: 0.5,
            non_carrying_minimum_shares_per_period: 0.5,
            carrying_share_fraction: 0.5,
            weak_cap: 64,
            weak_fill: 32,
            weak_depth: 6,
            ratio_max: 4_096,
            ceiling_ratio: 1.0e9,
            shares: 100_000,
            warmup_shares: 20_000,
        },
        racers: vec![carrier, refuser],
        settings: RaceSettings {
            fork_choice: ForkChoice::ShareWork,
            races_per_share: 0.021 / 0.979,
            stale_parent_loss: 0.0,
            uncle_carrier_drops: true,
        },
    };
    let outcome = simulate_race_queue(&inputs, &mut SimulationRandom::from_seed(8));
    assert!(
        outcome.carrying_lost_fraction > 0.001,
        "{}",
        outcome.carrying_lost_fraction
    );
}
