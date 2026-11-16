// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;
use crate::constants::{BCH_BLOCK_INTERVAL_SECONDS, TEMPLATE_POLL_SECONDS};

#[test]
fn a_v1_job_carries_the_coinbase_twice_over_in_hex() {
    // 782-byte coinbase less the 12-byte extranonce, in hex, a 13-hash
    // branch and the fixed JSON: 1,540 + 871 + 200 = 2,611 bytes in two
    // segments.
    assert_eq!(stratum_job_bytes(StratumProtocol::V1, 782, 13), 2_611 + 80);
}

#[test]
fn a_v2_extended_job_carries_the_coinbase_in_binary() {
    assert_eq!(
        stratum_job_bytes(StratumProtocol::V2Extended, 782, 13),
        770 + 416 + 60 + 40
    );
}

#[test]
fn a_v2_standard_job_does_not_grow_with_the_coinbase() {
    assert_eq!(
        stratum_job_bytes(StratumProtocol::V2Standard, 3_000_000, 24),
        70 + 40
    );
}

#[test]
fn miners_get_a_job_for_each_share_template_poll_and_block() {
    let jobs = stratum_jobs_per_second(10.0);
    let expected = 0.1 + 1.0 / TEMPLATE_POLL_SECONDS + 1.0 / BCH_BLOCK_INTERVAL_SECONDS;
    assert!((jobs - expected).abs() < 1e-12, "jobs {jobs}");
}

#[test]
fn miners_get_a_job_about_every_4_6_seconds_at_ten_second_shares() {
    // A share and a template poll every 10 s and a block every minute.
    let seconds = 1.0 / stratum_jobs_per_second(10.0);
    assert!((seconds - 4.6).abs() < 0.05, "seconds {seconds}");
}

#[test]
fn bchn_relays_a_blocks_transactions_over_its_interval() {
    // 8,849,557 transactions a block, each 226 bytes plus four inv entries
    // and a getdata entry: 406 bytes.
    let load = bchn_load(2_000_000_000, 226, 8, 782);
    assert_eq!(load.transactions_per_block, 8_849_557);
    let expected = 8_849_557.0 / BCH_BLOCK_INTERVAL_SECONDS * 406.0;
    assert!((load.relay_bytes_per_second - expected).abs() < 1e-6);
}

#[test]
fn bchn_relays_about_sixty_megabytes_a_second_at_two_gigabyte_blocks() {
    // 8,849,557 transactions of 406 relayed bytes each a minute: 479
    // Mbit/s, ten times the 47.9 of ten-minute blocks.
    let load = bchn_load(2_000_000_000, 226, 8, 782);
    assert!((load.relay_bytes_per_second / 1.0e6 - 59.9).abs() < 0.05);
    assert!((megabits(load.relay_bytes_per_second) - 479.0).abs() < 0.5);
}

#[test]
fn a_compact_block_is_six_bytes_a_transaction_and_the_whole_coinbase() {
    let load = bchn_load(32_000_000, 226, 8, 3_070);
    assert_eq!(load.compact_block_bytes, 96 + 6 * 141_592 + 3_070);
}

#[test]
fn full_templates_cost_more_than_twice_the_block_each_poll() {
    let load = bchn_load(32_000_000, 226, 8, 782);
    let per_template = 64_000_000.0 + 208.0 * 141_592.0 + 1_000.0;
    let expected = per_template * (0.1 + 1.0 / BCH_BLOCK_INTERVAL_SECONDS);
    assert!((load.template_bytes_per_second - expected).abs() < 1e-3);
}

#[test]
fn a_coinbase_adds_its_transfer_time_on_every_hop() {
    let seconds = coinbase_relay_seconds(1_000_000, 12_500_000.0, 4);
    assert!((seconds - 0.32).abs() < 1e-12);
}

#[test]
fn a_delay_of_one_block_interval_orphans_most_blocks() {
    let risk = orphan_risk(BCH_BLOCK_INTERVAL_SECONDS);
    assert!((risk - (1.0 - (-1.0f64).exp())).abs() < 1e-12);
    assert_eq!(orphan_risk(0.0), 0.0);
}

#[test]
fn a_one_minute_delay_orphans_most_blocks() {
    assert!((orphan_risk(60.0) - 0.632).abs() < 0.001);
}

#[test]
fn a_second_of_delay_costs_about_1_7_percent_of_blocks() {
    // Ten times the 0.17% of ten-minute blocks.
    assert!((orphan_risk(1.0) - 0.0165).abs() < 0.0001);
}

#[test]
fn stratum_protocols_parse_by_name() {
    assert_eq!(StratumProtocol::parse("v1"), Ok(StratumProtocol::V1));
    assert_eq!(
        StratumProtocol::parse("v2-standard"),
        Ok(StratumProtocol::V2Standard)
    );
    assert!(StratumProtocol::parse("v3").is_err());
    assert_eq!(StratumProtocol::V2Extended.name(), "v2-extended");
}
