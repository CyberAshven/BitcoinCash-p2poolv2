// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! BCHN specification vectors and real BCH header bands in the BCH 2026 VM.
mod test_utils;

use p2poolv2_safa as safa;
use test_utils::{headers::*, *};

macro_rules! specification_run {
    ($name:ident,$file:literal,$count:expr) => {
        #[test]
        fn $name() {
            assert_eq!(
                check_asert_run(include_str!(concat!(
                    "../../p2poolv2_lib/src/shares/bch_asert/aserti3-2d/",
                    $file
                ))),
                $count
            );
        }
    };
}
specification_run!(specification_run01, "run01", 10);
specification_run!(specification_run02, "run02", 10);
specification_run!(specification_run03, "run03", 10);
specification_run!(specification_run04, "run04", 225);
specification_run!(specification_run05, "run05", 225);
specification_run!(specification_run06, "run06", 1000);
specification_run!(specification_run07, "run07", 1000);
specification_run!(specification_run08, "run08", 500);
specification_run!(specification_run09, "run09", 10);
specification_run!(specification_run10, "run10", 10);
specification_run!(specification_run11, "run11", 1000);
specification_run!(specification_run12, "run12", 10000);

#[test]
fn header_band_agrees_across_time_offsets_and_targets() {
    check_band_matrix();
}

#[test]
fn real_mainnet_headers_pass() {
    check_header_pairs("mainnet", &MAINNET_PAIRS);
}
#[test]
fn real_chipnet_headers_pass() {
    check_header_pairs("chipnet", &CHIPNET_PAIRS);
}

macro_rules! mainnet_edge {
    ($name:ident,$bits:expr,$accepted:expr) => {
        #[test]
        fn $name() {
            assert_eq!(
                band("mainnet", $bits, 1_605_447_844 + 600, 661_648),
                $accepted
            );
        }
    };
}
mainnet_edge!(mainnet_reference, 0x1804_dafe, true);
mainnet_edge!(mainnet_hard_boundary, 0x1802_6d7f, true);
mainnet_edge!(mainnet_below_hard_boundary, 0x1802_6d7e, false);
mainnet_edge!(mainnet_easy_boundary, 0x1805_1924, true);
mainnet_edge!(mainnet_above_easy_boundary, 0x1805_1925, false);
mainnet_edge!(
    mainnet_has_no_minimum_difficulty_allowance,
    0x1d00_ffff,
    false
);

#[test]
fn chipnet_band_boundaries() {
    let height = 16_844 + 121;
    let time = 1_605_451_779 + 600;
    let computed = safa::asert_target("chipnet", height, time)
        .unwrap()
        .unwrap();
    assert!(band("chipnet", safa::get_compact(&computed), time, height));
    assert!(band(
        "chipnet",
        safa::get_compact(&computed.shr(3)),
        time,
        height
    ));
    assert!(!band(
        "chipnet",
        safa::get_compact(&computed.shr(4)),
        time,
        height
    ));
    assert!(band(
        "chipnet",
        safa::get_compact(&computed.shl(1)),
        time,
        height
    ));
    assert!(band(
        "chipnet",
        safa::get_compact(&computed.shl(3)),
        time,
        height
    ));
    assert!(!band(
        "chipnet",
        safa::get_compact(&computed.shl(4)),
        time,
        height
    ));
    assert!(band("chipnet", 0x1d00_ffff, time, height));
}
#[test]
fn chipnet_header_time_two_hours_before_parent_passes() {
    let pair = CHIPNET_PAIRS[8];
    assert_eq!(pair.0 + 1, 329001);
    assert_eq!(header_bits(pair.2), 0x1a31_06ee);
    assert_eq!(
        safa::get_compact(
            &safa::asert_target("chipnet", 329001, header_time(pair.1))
                .unwrap()
                .unwrap()
        ),
        0x1a31_06ee
    );
    assert!(band(
        "chipnet",
        0x1a31_06ee,
        header_time(pair.1) - 7200,
        329001
    ));
}
#[test]
fn chipnet_exact_upper_eight_passes() {
    assert_eq!(
        safa::asert_target("chipnet", 26845, 1_605_451_779 + 600)
            .unwrap()
            .unwrap(),
        number(1)
    );
    assert!(band("chipnet", 0x0108_0000, 1_605_451_779 + 600, 26845));
}
#[test]
fn chipnet_upper_eight_plus_one_fails() {
    assert!(!band("chipnet", 0x0109_0000, 1_605_451_779 + 600, 26845));
}
#[test]
fn bitcoin_difficulty_17030ecd_fails_on_both_networks() {
    assert!(!band(
        "mainnet",
        0x1703_0ecd,
        header_time(MAINNET_PAIRS[8].2),
        974001
    ));
    assert!(!band("chipnet", 0x1703_0ecd, block_time(), block_height()));
}
#[test]
fn bitcoin_difficulty_1702349e_fails_on_both_networks() {
    assert!(!band(
        "mainnet",
        0x1702_349e,
        header_time(MAINNET_PAIRS[8].2),
        974001
    ));
    assert!(!band("chipnet", 0x1702_349e, block_time(), block_height()));
}

macro_rules! invalid_header_bits {
    ($name:ident,$bits:expr) => {
        #[test]
        fn $name() {
            fails(
                FunctionCaller::new("mainnet")
                    .call(7, &[uint32($bits), uint32(1_605_447_844), numeric(661648)]),
            );
        }
    };
}
invalid_header_bits!(asert_rejects_zero, 0);
invalid_header_bits!(asert_rejects_negative, 0x0480_0001);
invalid_header_bits!(asert_rejects_overflow, 0x2300_0001);
invalid_header_bits!(asert_rejects_large_exponent, 0x2100_ffff);
invalid_header_bits!(asert_rejects_above_limit, 0x1d01_0000);
#[test]
fn asert_rejects_zero_height() {
    fails(
        FunctionCaller::new("mainnet")
            .call(7, &[uint32(0x1804_dafe), uint32(1_605_447_844), numeric(0)]),
    );
}
#[test]
fn asert_rejects_anchor_height() {
    fails(FunctionCaller::new("mainnet").call(
        7,
        &[uint32(0x1804_dafe), uint32(1_605_447_844), numeric(661647)],
    ));
}

#[test]
fn fallback_band_edges_and_height_gate() {
    let reference = safa::set_compact(0x1804_dafe).unwrap();
    let caller = FunctionCaller::new("chipnet");
    let hard = reference.shr(4);
    let harder = hard.sub(&number(1).shl(160));
    let easy = reference.mul(&number(11)).div_mod(&number(10)).0;
    let easier = reference.mul(&number(12)).div_mod(&number(10)).0;
    assert_eq!(
        stack_number(
            &ok(caller.call(
                8,
                &[uint32(safa::get_compact(&reference)), uint32(0x1804_dafe)]
            ))[0]
        ),
        number(1)
    );
    assert_eq!(
        stack_number(
            &ok(caller.call(8, &[uint32(safa::get_compact(&hard)), uint32(0x1804_dafe)]))[0]
        ),
        number(1)
    );
    assert_eq!(
        stack_number(
            &ok(caller.call(
                8,
                &[uint32(safa::get_compact(&harder)), uint32(0x1804_dafe)]
            ))[0]
        ),
        number(0)
    );
    assert_eq!(
        stack_number(
            &ok(caller.call(8, &[uint32(safa::get_compact(&easy)), uint32(0x1804_dafe)]))[0]
        ),
        number(1)
    );
    assert_eq!(
        stack_number(
            &ok(caller.call(
                8,
                &[uint32(safa::get_compact(&easier)), uint32(0x1804_dafe)]
            ))[0]
        ),
        number(0)
    );
    assert!(safa::fallback_band_ok(
        safa::get_compact(&hard),
        0x1804_dafe,
        4032
    ));
    assert!(!safa::fallback_band_ok(
        safa::get_compact(&hard),
        0x1804_dafe,
        4031
    ));
    assert!(safa::fallback_band_ok(
        safa::get_compact(&hard),
        0x1804_dafe,
        100000
    ));
}
