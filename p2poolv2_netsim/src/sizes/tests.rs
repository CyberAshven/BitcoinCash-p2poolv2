// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;

#[test]
fn a_hybrid_coinbase_without_payouts_is_321_bytes() {
    // S = 367 less the tree output (44) and the two extra count bytes.
    assert_eq!(hybrid_coinbase_bytes(0, false), 321);
    assert_eq!(hybrid_coinbase_bytes(0, true), 365);
}

#[test]
fn a_no_token_coinbase_pays_a_thousand_payees_directly() {
    // 1,003 outputs need a three-byte output count.
    let split = split_payees(CoinbaseLayout::no_tokens(), 1_000);
    assert_eq!(split.direct_outputs, 1_000);
    assert_eq!(split.tree_payees, 0);
    assert_eq!(split.coinbase_bytes, 321 + 34_000 + 2);
    assert_eq!(split.payout_bytes, 34_000);
}

#[test]
fn a_no_token_coinbase_pays_29402_payees_directly_within_bchs_limit() {
    // P = 1,000,000 - 367 = 999,633; with the unused tree output
    // 999,677 >= 29,402 * 34.
    let split = split_payees(CoinbaseLayout::no_tokens(), 29_402);
    assert_eq!(split.direct_outputs, 29_402);
    assert_eq!(split.tree_payees, 0);
    assert_eq!(split.coinbase_bytes, 999_991);
}

#[test]
fn the_29403rd_no_token_payee_starts_a_tree() {
    // P = 999,633 bytes: 29,400 P2PKH outputs.
    let split = split_payees(CoinbaseLayout::no_tokens(), 29_403);
    assert_eq!(split.direct_outputs, 29_400);
    assert_eq!(split.tree_payees, 3);
    assert_eq!(split.coinbase_bytes, 321 + 29_400 * 34 + 44 + 2);
}

#[test]
fn a_million_no_token_payees_keep_the_coinbase_under_bchs_limit() {
    let split = split_payees(CoinbaseLayout::no_tokens(), 1_000_000);
    assert_eq!(split.direct_outputs, 29_400);
    assert_eq!(split.tree_payees, 970_600);
    assert_eq!(split.coinbase_bytes, 999_967);
    assert_eq!(split.payout_bytes, 29_400 * 34 + 44);
}

#[test]
fn eighty_token_payees_fit_without_a_tree() {
    // P = 3,072 - 367 = 2,705; with the unused tree output 2,749 >= 80 * 34.
    let split = split_payees(CoinbaseLayout::with_tokens(), 80);
    assert_eq!(split.direct_outputs, 80);
    assert_eq!(split.tree_payees, 0);
    assert_eq!(split.coinbase_bytes, 321 + 80 * 34);
}

#[test]
fn the_eighty_first_token_payee_starts_a_tree_and_leaves_79_direct() {
    let split = split_payees(CoinbaseLayout::with_tokens(), 81);
    assert_eq!(split.direct_outputs, 79);
    assert_eq!(split.tree_payees, 2);
    assert_eq!(split.coinbase_bytes, 321 + 79 * 34 + 44);
}

#[test]
fn a_million_token_payees_keep_a_3051_byte_coinbase() {
    let split = split_payees(CoinbaseLayout::with_tokens(), 1_000_000);
    assert_eq!(split.direct_outputs, 79);
    assert_eq!(split.tree_payees, 999_921);
    assert_eq!(split.coinbase_bytes, 3_051);
}

#[test]
fn a_limit_below_the_fixed_outputs_pays_everyone_through_the_tree() {
    let split = split_payees(
        CoinbaseLayout::Hybrid {
            tokens: true,
            limit_bytes: 400,
        },
        10,
    );
    assert_eq!(split.direct_outputs, 0);
    assert_eq!(split.tree_payees, 10);
    assert_eq!(split.coinbase_bytes, 365);
}

#[test]
fn cap_11_coinbase_is_782_bytes_for_a_million_payees() {
    // payout-distribution.md: "the largest Bitcoin Cash coinbase is 782 bytes".
    let split = split_payees(CoinbaseLayout::cap_11(), 1_000_000);
    assert_eq!(split.coinbase_bytes, 782);
    assert_eq!(split.direct_outputs, 11);
    assert_eq!(split.drawn_payees, 999_990);
}

#[test]
fn cap_11_coinbase_below_the_cap_pays_every_payee() {
    let split = split_payees(CoinbaseLayout::cap_11(), 5);
    assert_eq!(split.direct_outputs, 5);
    assert_eq!(split.drawn_payees, 0);
    assert_eq!(split.coinbase_bytes, 408 + 5 * 34);
}

#[test]
fn an_uncapped_coinbase_grows_34_bytes_a_payee_without_end() {
    let split = split_payees(CoinbaseLayout::Uncapped, 1_000_000);
    assert_eq!(split.direct_outputs, 1_000_000);
    assert_eq!(split.coinbase_bytes, 321 + 34_000_000 + 4);
}

#[test]
fn the_hybrid_constructor_picks_the_limit_by_tokens() {
    assert_eq!(
        CoinbaseLayout::hybrid(false, 3_072),
        CoinbaseLayout::Hybrid {
            tokens: false,
            limit_bytes: 1_000_000
        }
    );
    assert_eq!(
        CoinbaseLayout::hybrid(true, 3_072),
        CoinbaseLayout::Hybrid {
            tokens: true,
            limit_bytes: 3_072
        }
    );
}

#[test]
fn layout_names_say_the_rule() {
    assert_eq!(CoinbaseLayout::no_tokens().name(), "no-tokens");
    assert_eq!(CoinbaseLayout::with_tokens().name(), "tokens-3072");
    assert_eq!(CoinbaseLayout::cap_11().name(), "cap-11");
    assert_eq!(CoinbaseLayout::Uncapped.name(), "uncapped");
}

#[test]
fn compact_sizes_switch_width_at_the_bitcoin_boundaries() {
    assert_eq!(compact_size_bytes(252), 1);
    assert_eq!(compact_size_bytes(253), 3);
    assert_eq!(compact_size_bytes(65_535), 3);
    assert_eq!(compact_size_bytes(65_536), 5);
    assert_eq!(compact_size_bytes(1 << 32), 9);
}

#[test]
fn merkle_branch_is_the_tree_depth() {
    assert_eq!(merkle_branch_hashes(1), 0);
    assert_eq!(merkle_branch_hashes(2), 1);
    assert_eq!(merkle_branch_hashes(3), 2);
    assert_eq!(merkle_branch_hashes(4), 2);
    assert_eq!(merkle_branch_hashes(5), 3);
}

#[test]
fn a_two_gigabyte_block_of_typical_transfers_has_a_24_hash_branch() {
    // 2,000,000,000 / 226 = 8,849,557 transactions; 2^23 < that <= 2^24.
    let transactions = block_transaction_count(2_000_000_000, 226);
    assert_eq!(transactions, 8_849_557);
    assert_eq!(merkle_branch_hashes(transactions), 24);
}

#[test]
fn a_share_on_a_one_megabyte_template_with_a_cap_11_coinbase() {
    let sizes = ShareSizes::new(CoinbaseLayout::cap_11(), 1_000, 1_000_000, 226, 0);
    // 4,424 transactions: a 13-hash branch.
    assert_eq!(sizes.merkle_branch_hashes, 13);
    assert_eq!(sizes.split.direct_outputs, 11);
    assert_eq!(sizes.coinbase_bytes(), 782);
    assert_eq!(sizes.header_bytes, 305);
    assert_eq!(sizes.body_bytes, 305 + 1 + 118 + 1 + 13 * 32 + 782);
}

#[test]
fn share_transactions_add_to_the_body() {
    let without = ShareSizes::new(CoinbaseLayout::cap_11(), 10, 1, 1, 0);
    let with = ShareSizes::new(CoinbaseLayout::cap_11(), 10, 1, 1, 500);
    assert_eq!(with.body_bytes, without.body_bytes + 500);
}

#[test]
fn uncles_add_a_hash_each_to_the_body() {
    let sizes = ShareSizes::new(CoinbaseLayout::cap_11(), 10, 1, 1, 0);
    assert_eq!(sizes.body_bytes_with_uncles(3), sizes.body_bytes + 96);
    assert_eq!(sizes.message_payload_bytes(3), sizes.body_bytes + 97);
}

#[test]
fn a_share_without_payouts_is_a_few_hundred_bytes_whatever_the_payees() {
    let sizes = ShareSizes::new(CoinbaseLayout::no_tokens(), 29_402, 32_000_000, 226, 0);
    assert_eq!(
        sizes.message_payload_bytes(0),
        1 + 305 + 1 + 118 + 1 + 18 * 32 + 999_991
    );
    assert_eq!(sizes.message_payload_bytes_without_payouts(0), 1_325);
}

#[test]
fn a_no_token_coinbase_breaks_only_the_pre_hybrid_share_limit_and_sv2_fields() {
    let layout = CoinbaseLayout::no_tokens();
    let check = ShareSizes::new(layout, 1_000_000, 1, 1, 0).limit_check(layout);
    assert!(check.coinbase_within_bch_transaction_limit);
    assert!(check.share_body_within_hybrid_limit);
    assert!(check.message_within_p2p_limit);
    assert_eq!(check.token_claim_within_vm_element, None);
    assert_eq!(
        check.broken(),
        vec!["pre-hybrid 200 KiB share body", "SV2 extended 64 KiB field"]
    );
}

#[test]
fn a_token_coinbase_breaks_no_rule() {
    let layout = CoinbaseLayout::with_tokens();
    let check = ShareSizes::new(layout, 1_000_000, 1, 1, 0).limit_check(layout);
    assert_eq!(check.token_claim_within_vm_element, Some(true));
    assert!(!check.coinbase_within_asic_evidence);
    assert!(check.broken().is_empty());
}

#[test]
fn cap_11_coinbase_meets_every_limit_and_the_asic_evidence() {
    let layout = CoinbaseLayout::cap_11();
    let check = ShareSizes::new(layout, 1_000_000, 1, 1, 0).limit_check(layout);
    assert!(check.coinbase_within_asic_evidence);
    assert!(check.broken().is_empty());
}

#[test]
fn an_uncapped_coinbase_for_a_million_payees_breaks_bch_consensus() {
    let layout = CoinbaseLayout::Uncapped;
    let check = ShareSizes::new(layout, 1_000_000, 1, 1, 0).limit_check(layout);
    assert_eq!(
        check.broken(),
        vec![
            "BCH 1 MB transaction",
            "hybrid 1,204,800-byte share body",
            "1.25 MiB P2P message",
            "pre-hybrid 200 KiB share body",
            "SV2 extended 64 KiB field"
        ]
    );
}

#[test]
fn the_pre_hybrid_share_body_limit_allows_6010_direct_payees() {
    let largest = largest_direct_payee_count(|sizes| {
        sizes
            .limit_check(CoinbaseLayout::Uncapped)
            .share_body_within_pre_hybrid_limit
    });
    assert_eq!(largest, 6_010);
}

#[test]
fn the_hybrid_share_body_limit_allows_35422_direct_payees() {
    let largest = largest_direct_payee_count(|sizes| {
        sizes
            .limit_check(CoinbaseLayout::Uncapped)
            .share_body_within_hybrid_limit
    });
    assert_eq!(largest, 35_422);
}

#[test]
fn bch_transaction_limit_allows_29402_direct_payees() {
    let largest = largest_direct_payee_count(|sizes| {
        sizes
            .limit_check(CoinbaseLayout::Uncapped)
            .coinbase_within_bch_transaction_limit
    });
    assert_eq!(largest, 29_402);
}

#[test]
fn the_p2p_message_limit_allows_38525_direct_payees() {
    let largest = largest_direct_payee_count(|sizes| {
        sizes
            .limit_check(CoinbaseLayout::Uncapped)
            .message_within_p2p_limit
    });
    assert_eq!(largest, 38_525);
}

#[test]
fn an_sv2_extended_job_field_holds_1922_direct_payees() {
    let largest = largest_direct_payee_count(|sizes| {
        sizes
            .limit_check(CoinbaseLayout::Uncapped)
            .coinbase_within_sv2_extended_fields
    });
    assert_eq!(largest, 1_922);
}

#[test]
fn one_vm_stack_element_holds_a_coinbase_of_284_direct_payees() {
    let largest =
        largest_direct_payee_count(|sizes| sizes.coinbase_bytes() <= BCH_VM_STACK_ELEMENT_BYTES);
    assert_eq!(largest, 284);
}

#[test]
fn the_asic_evidence_allows_13_direct_payees() {
    // 321 + 13 * 34 = 763 bytes fit the 784-byte block; a 14th payee is 797.
    let largest = largest_direct_payee_count(|sizes| {
        sizes
            .limit_check(CoinbaseLayout::Uncapped)
            .coinbase_within_asic_evidence
    });
    assert_eq!(largest, 13);
}

#[test]
fn an_empty_message_still_costs_framing_and_one_packet_header() {
    assert_eq!(wire_bytes(0), 64 + 40);
}

#[test]
fn a_message_over_one_segment_pays_a_second_packet_header() {
    assert_eq!(wire_bytes(1_396), 1_460 + 40);
    assert_eq!(wire_bytes(1_397), 1_461 + 80);
}

#[test]
fn the_tree_starts_after_29402_no_token_payees_and_80_token_payees() {
    assert_eq!(
        largest_payee_count_without_tree(CoinbaseLayout::no_tokens()),
        29_402
    );
    assert_eq!(
        largest_payee_count_without_tree(CoinbaseLayout::with_tokens()),
        80
    );
}
