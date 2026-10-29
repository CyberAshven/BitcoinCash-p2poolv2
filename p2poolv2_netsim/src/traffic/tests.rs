// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;

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
fn miners_get_about_a_job_every_five_seconds_at_ten_second_shares() {
    let jobs = stratum_jobs_per_second(10.0);
    assert!((jobs - (0.2 + 1.0 / 600.0)).abs() < 1e-12, "jobs {jobs}");
}

#[test]
fn bchn_relays_about_six_megabytes_a_second_at_two_gigabyte_blocks() {
    // 8,849,557 transactions in 600 s, each 226 bytes plus four inv
    // entries and a getdata entry: 406 bytes.
    let load = bchn_load(2_000_000_000, 226, 8, 782);
    assert_eq!(load.transactions_per_block, 8_849_557);
    let expected = 8_849_557.0 / 600.0 * 406.0;
    assert!((load.relay_bytes_per_second - expected).abs() < 1e-6);
    assert!((megabits(load.relay_bytes_per_second) - 47.9).abs() < 0.1);
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
    let expected = per_template * (0.1 + 1.0 / 600.0);
    assert!((load.template_bytes_per_second - expected).abs() < 1e-3);
}

#[test]
fn a_coinbase_adds_its_transfer_time_on_every_hop() {
    let seconds = coinbase_relay_seconds(1_000_000, 12_500_000.0, 4);
    assert!((seconds - 0.32).abs() < 1e-12);
}

#[test]
fn a_ten_minute_delay_orphans_most_blocks() {
    assert!((orphan_risk(600.0) - (1.0 - (-1.0f64).exp())).abs() < 1e-12);
    assert_eq!(orphan_risk(0.0), 0.0);
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
