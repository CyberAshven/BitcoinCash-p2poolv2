// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use serde_json::json;

fn row(paying_height: u64, late_paid: bool) -> BlockRow {
    BlockRow {
        bitcoin_hash: format!("{paying_height:064x}"),
        paying_height,
        late_paid,
    }
}

#[test]
fn early_payment_is_a_violation() {
    assert_eq!(
        evaluate(&[row(25, true)], 20, 10, 8),
        (
            Verdict::Violation,
            "late miner paid before maturity at paying height(s) 25".to_string()
        )
    );
}

#[test]
fn no_mature_block_is_inconclusive() {
    assert_eq!(
        evaluate(&[row(25, false)], 20, 10, 10).0,
        Verdict::Inconclusive
    );
}

#[test]
fn unpaid_first_mature_block_is_a_violation() {
    assert_eq!(
        evaluate(&[row(31, true), row(30, false)], 20, 10, 10),
        (
            Verdict::Violation,
            "late miner not paid by the first mature block-find at paying height 30".to_string()
        )
    );
}

#[test]
fn paid_first_mature_block_passes() {
    assert_eq!(
        evaluate(&[row(29, false), row(30, true)], 20, 10, 10),
        (
            Verdict::Pass,
            "late miner first paid at paying height 30".to_string()
        )
    );
}

#[test]
fn coinbase_payees_reads_address_and_addresses() {
    let block = json!({"tx": [{"vout": [
        {"scriptPubKey": {"address": "bcrt1qa"}},
        {"scriptPubKey": {"addresses": ["bchreg:qb"]}},
        {"scriptPubKey": {"type": "nulldata"}}
    ]}]});
    assert_eq!(
        coinbase_payees(&block),
        BTreeSet::from(["bcrt1qa".to_string(), "bchreg:qb".to_string()])
    );
}

#[test]
fn report_lists_rows_by_paying_height() {
    let text = report(&[row(31, true), row(30, false)], (1, "addr"), 20, (10, 9));
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "late miner: node 1 (addr), coinbase maturity 20");
    assert_eq!(
        lines[1],
        "first late share: confirmed 10, confirmed or uncle 9"
    );
    assert_eq!(
        lines[2],
        "bitcoin block         paying_h  p-first  mature  late_paid"
    );
    assert_eq!(
        lines[3],
        "0000000000000000..          30       20     yes         no"
    );
    assert_eq!(
        lines[4],
        "0000000000000000..          31       21     yes        yes"
    );
}
