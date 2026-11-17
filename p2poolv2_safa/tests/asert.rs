// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! BCHN specification vectors, CHIP-2025-03's and real BCH header bands in
//! the BCH 2026 VM.
mod test_utils;

use p2poolv2_safa::{self as safa, BigInt, IntervalSwitch, NetworkParameters};
use serde_json::Value;
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

/// CHIP-2025-03's ASERT vectors, vendored in `p2poolv2_lib`
/// (`chip202503/README.md`).
const CHIP_ASERT_VECTORS: &str =
    include_str!("../../p2poolv2_lib/src/shares/bch_asert/chip202503/asert.json");

fn chip_number(value: &Value) -> i64 {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        .unwrap()
}
fn chip_target(value: &Value) -> BigInt {
    let mut bytes = hex::decode(value.as_str().unwrap()).unwrap();
    bytes.reverse();
    BigInt::from_bytes_le(&bytes)
}
fn chip_bits(value: &Value) -> u32 {
    u32::from_str_radix(value.as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
}

/// Every CHIP ASERT vector in Function 6 and in the Rust model: the 600-tick
/// golden ones, their 60-tick siblings ten times easier, a 60-tick anchor
/// and the interval-ratio guard, target and nBits exactly.
#[test]
fn every_chip_asert_vector_matches() {
    let vectors: Value = serde_json::from_str(CHIP_ASERT_VECTORS).unwrap();
    assert_eq!(chip_target(&vectors["powLimit"]), safa::pow_limit());
    let rows = vectors["vectors"].as_array().unwrap();
    assert_eq!(rows.len(), 45);
    let caller = FunctionCaller::new("mainnet");
    let mut one_minute = 0;
    for row in rows {
        let name = row["name"].as_str().unwrap();
        let next_interval = chip_number(&row["nextIdealBlockTime"]);
        let target = asert_in_vm(
            &caller,
            &chip_target(&row["refTarget"]),
            chip_number(&row["nTimeDiff"]),
            chip_number(&row["tickDelta"]),
            chip_number(&row["anchorIdealBlockTime"]),
            next_interval,
            chip_number(&row["nHalfLife"]),
        );
        assert_eq!(target, chip_target(&row["expectedTarget"]), "{name}");
        assert_eq!(
            safa::get_compact(&target),
            chip_bits(&row["expectednBits"]),
            "{name}"
        );
        one_minute += usize::from(next_interval == 60);
    }
    assert_eq!(one_minute, 20);
}

/// The CHIP's end-to-end rows. A test network block takes the
/// proof-of-work limit more than twice its interval after its parent, which
/// is BCHN's rule on two headers; every other row is Function 6 on the
/// parent's ticks less the anchor's own.
#[test]
fn every_chip_asert_wrapper_row_matches() {
    let vectors: Value = serde_json::from_str(CHIP_ASERT_VECTORS).unwrap();
    let rows = vectors["wrapper"].as_array().unwrap();
    assert_eq!(rows.len(), 10);
    let caller = FunctionCaller::new("mainnet");
    for row in rows {
        let name = row["name"].as_str().unwrap();
        let anchor = &row["anchor"];
        let next_interval = chip_number(&row["nextIdealBlockTime"]);
        let parent_time = chip_number(&row["nPrevBlockTime"]);
        let bits = if row["fPowAllowMinDifficultyBlocks"].as_bool().unwrap()
            && chip_number(&row["nNextBlockTime"]) > parent_time + 2 * next_interval
        {
            safa::POW_LIMIT_BITS
        } else {
            safa::get_compact(&asert_in_vm(
                &caller,
                &safa::set_compact(chip_bits(&anchor["nBits"])).unwrap(),
                parent_time - chip_number(&anchor["nPrevBlockTime"]),
                chip_number(&row["nPrevTick"]) - chip_number(&anchor["nTick"]),
                chip_number(&anchor["nBlockIntervalTime"]),
                next_interval,
                chip_number(&row["nASERTHalfLife"]),
            ))
        };
        assert_eq!(bits, chip_bits(&row["expectednBits"]), "{name}");
    }
}

/// The parent's ticks from the anchor's and the header's interval, by
/// chipnet's pinned switch: block 332,450 is the last ten-minute block, so a
/// header at 332,451 is the first with a 60-tick interval and one at 332,452
/// the first on a 60-tick parent. Mainnet has no pin: 600 ticks a block.
#[test]
fn ticks_follow_the_pinned_switch() {
    let chipnet = safa::network_parameters("chipnet").unwrap();
    assert_eq!(
        chipnet.interval_switch,
        Some(IntervalSwitch {
            height: 332_450,
            interval: 60
        })
    );
    let anchor_ticks = 16_844 * 600;
    for (parent_height, ticks, next_interval) in [
        (16_844, 16_844 * 600, 600),
        (332_449, 332_449 * 600, 600),
        (332_450, 332_450 * 600, 60),
        (332_451, 332_450 * 600 + 60, 60),
        // Knuth's own ticks for block 332,460.
        (332_460, 199_470_600, 60),
    ] {
        assert_eq!(
            safa::tick_delta_and_interval(&chipnet, parent_height),
            (ticks - anchor_ticks, next_interval),
            "parent {parent_height}"
        );
    }
    let mainnet = safa::network_parameters("mainnet").unwrap();
    assert_eq!(mainnet.interval_switch, None);
    assert_eq!(
        safa::tick_delta_and_interval(&mainnet, 1_000_000),
        ((1_000_000 - 661_647) * 600, 600)
    );
}

/// Every real chipnet block around the one-minute activation passes the
/// band at its own time, in Function 7 and in the Rust model.
#[test]
fn real_chipnet_blocks_around_the_activation_pass() {
    let chipnet = safa::network_parameters("chipnet").unwrap();
    let caller = FunctionCaller::for_parameters(&chipnet);
    for (height, time, bits) in CHIPNET_AROUND_ACTIVATION {
        assert!(
            band_with(&caller, &chipnet, bits, time, height),
            "block {height}"
        );
    }
}

/// Every one of those blocks carries the nBits Function 6 computes from its
/// parent's time, ticks and next interval: ten times easier from block
/// 332,451 on, or the proof-of-work limit's after BCHN's minimum-difficulty
/// gap, 20 minutes before the switch (332,441 and 332,445) and 2 minutes
/// after it (332,451 and 332,454).
#[test]
fn real_chipnet_blocks_around_the_activation_carry_the_bits_computed_for_them() {
    let chipnet = safa::network_parameters("chipnet").unwrap();
    let caller = FunctionCaller::for_parameters(&chipnet);
    let reference = safa::set_compact(chipnet.anchor_bits).unwrap();
    let mut minimum_difficulty = Vec::with_capacity(4);
    for pair in CHIPNET_AROUND_ACTIVATION.windows(2) {
        let [(parent_height, parent_time, _), (height, time, bits)] = pair else {
            unreachable!()
        };
        let (tick_delta, next_interval) =
            safa::tick_delta_and_interval(&chipnet, i64::from(*parent_height));
        let expected = if i64::from(*time) > i64::from(*parent_time) + 2 * next_interval {
            minimum_difficulty.push(*height);
            safa::POW_LIMIT_BITS
        } else {
            let target = asert_in_vm(
                &caller,
                &reference,
                i64::from(*parent_time) - i64::from(chipnet.anchor_parent_time),
                tick_delta,
                600,
                next_interval,
                i64::from(chipnet.half_life),
            );
            assert_eq!(
                safa::asert_target("chipnet", *height, *parent_time).unwrap(),
                Some(target.clone()),
                "block {height}"
            );
            safa::get_compact(&target)
        };
        assert_eq!(expected, *bits, "block {height}");
    }
    assert_eq!(minimum_difficulty, [332_441, 332_445, 332_451, 332_454]);
}

/// Before and after on real chipnet blocks. The covenant deployed on chipnet
/// in October 2026 counts every block as ten minutes: its own Function 7
/// accepts the real blocks up to 332,451 and refuses every later one that is
/// not a minimum-difficulty block, where it computes a target more than
/// eight times too hard. This covenant accepts them all.
#[test]
fn the_deployed_covenant_refuses_chipnet_blocks_after_the_switch() {
    let chipnet = safa::network_parameters("chipnet").unwrap();
    let after = FunctionCaller::for_parameters(&chipnet);
    let before = FunctionCaller::with_library(deployed_library());
    let reference = safa::set_compact(chipnet.anchor_bits).unwrap();
    let mut refused = Vec::with_capacity(8);
    eprintln!("height  nBits     before: ASERT at its own time  after: ASERT at its own time");
    for (height, time, bits) in CHIPNET_AROUND_ACTIVATION {
        let time_diff = i64::from(time) - i64::from(chipnet.anchor_parent_time);
        // The deployed Function 6: [reference time_diff height_diff half_life].
        let computed_before = stack_number(
            &ok(before.call(
                6,
                &[
                    big_numeric(&reference),
                    numeric(time_diff),
                    numeric(i64::from(height) - 1 - i64::from(chipnet.anchor_height)),
                    numeric(i64::from(chipnet.half_life)),
                ],
            ))[0],
        );
        let computed_after = safa::asert_target("chipnet", height, time)
            .unwrap()
            .unwrap();
        let accepted_before = band_in_vm(&before, bits, time, height);
        let accepted_after = band_with(&after, &chipnet, bits, time, height);
        eprintln!(
            "{height}  {bits:08x}  {:08x} {:<8}                {:08x} {}",
            safa::get_compact(&computed_before),
            if accepted_before {
                "accepts"
            } else {
                "refuses"
            },
            safa::get_compact(&computed_after),
            if accepted_after { "accepts" } else { "refuses" },
        );
        assert!(accepted_after, "block {height}");
        // Up to the last ten-minute block both count the same ticks.
        if height <= 332_450 {
            assert_eq!(computed_before, computed_after, "block {height}");
        }
        if !accepted_before {
            refused.push(height);
        }
    }
    assert_eq!(
        refused,
        [
            332_452, 332_453, 332_455, 332_456, 332_457, 332_458, 332_459, 332_460
        ]
    );
}

/// The band's edges on a one-minute chipnet block, 332,460 at its own time:
/// an eighth of the computed target to eight times it, exactly.
#[test]
fn chipnet_band_boundaries_after_the_switch() {
    let chipnet = safa::network_parameters("chipnet").unwrap();
    let caller = FunctionCaller::for_parameters(&chipnet);
    let (height, time, bits) = CHIPNET_AROUND_ACTIVATION[21];
    let computed = safa::asert_target("chipnet", height, time)
        .unwrap()
        .unwrap();
    // Ten times the target the same header would have as a ten-minute block.
    let unpinned = NetworkParameters {
        interval_switch: None,
        ..chipnet.clone()
    };
    let (tick_delta, next_interval) =
        safa::tick_delta_and_interval(&chipnet, i64::from(height) - 1);
    assert_eq!(next_interval, 60);
    assert_eq!(
        computed,
        safa::calculate_asert(
            &safa::set_compact(chipnet.anchor_bits).unwrap(),
            i64::from(time) - i64::from(chipnet.anchor_parent_time),
            tick_delta,
            600,
            600,
            i64::from(chipnet.half_life),
        )
        .mul(&number(10))
    );
    assert!(
        safa::asert_target_for(&unpinned, height, time)
            .unwrap()
            .unwrap()
            .cmp(&computed.shr(4))
            .is_lt()
    );
    let check = |bits: u32| band_with(&caller, &chipnet, bits, time, height);
    assert!(check(bits));
    assert!(check(safa::get_compact(&computed)));
    // Compact bits round a target down, so the bits of eight times the
    // computed target are within the band and one mantissa step above them
    // is past it; one step above the bits of an eighth is within and one
    // step below is past.
    let easy_edge = safa::get_compact(&computed.shl(3));
    assert!(check(easy_edge));
    assert!(!check(easy_edge + 1));
    let hard_edge = safa::get_compact(&computed.shr(3));
    assert!(check(hard_edge + 1));
    assert!(!check(hard_edge - 1));
    assert!(!check(safa::get_compact(&computed.shl(4))));
    assert!(!check(safa::get_compact(&computed.shr(4))));
    assert!(check(0x1d00_ffff));
}

/// Mainnet's switch block is not known, so its covenant has no pin. With one
/// the same script counts ticks there too: the first one-minute header's
/// target is ten times the ten-minute one's, and the narrow band's edges,
/// half to 1.05 times, hold on it.
#[test]
fn a_pinned_mainnet_switch_makes_one_minute_targets_ten_times_easier() {
    let mainnet = safa::network_parameters("mainnet").unwrap();
    let pinned = NetworkParameters {
        interval_switch: Some(IntervalSwitch {
            height: 1_000_000,
            interval: 60,
        }),
        ..mainnet.clone()
    };
    let caller = FunctionCaller::for_parameters(&pinned);
    // On schedule at the switch block: the anchor target, then ten times it.
    let on_schedule = |ticks: i64| (i64::from(mainnet.anchor_parent_time) + ticks + 600) as u32;
    let anchor_target = safa::set_compact(mainnet.anchor_bits).unwrap();
    let ten_minute_ticks = (1_000_000 - 1 - 661_647) * 600;
    assert_eq!(
        safa::asert_target_for(&pinned, 1_000_000, on_schedule(ten_minute_ticks)).unwrap(),
        Some(anchor_target.clone())
    );
    let first = on_schedule(ten_minute_ticks + 600);
    let computed = safa::asert_target_for(&pinned, 1_000_001, first)
        .unwrap()
        .unwrap();
    assert_eq!(computed, anchor_target.mul(&number(10)));
    assert_eq!(
        safa::asert_target_for(&mainnet, 1_000_001, first).unwrap(),
        Some(anchor_target.clone())
    );
    // The second one-minute header is on a 60-tick parent.
    assert_eq!(
        safa::asert_target_for(&pinned, 1_000_002, on_schedule(ten_minute_ticks + 660)).unwrap(),
        Some(computed.clone())
    );
    let check = |target: &BigInt| {
        band_with(
            &caller,
            &pinned,
            safa::get_compact(target),
            first,
            1_000_001,
        )
    };
    let rounded = safa::set_compact(safa::get_compact(&computed)).unwrap();
    assert!(check(&rounded));
    assert!(check(&rounded.shr(1)));
    assert!(!check(&rounded.shr(2)));
    assert!(check(&rounded.mul(&number(104)).div_mod(&number(100)).0));
    assert!(!check(&rounded.mul(&number(106)).div_mod(&number(100)).0));
    // The unpinned covenant refuses the one-minute target and takes the
    // ten-minute one.
    let unpinned = FunctionCaller::for_parameters(&mainnet);
    assert!(!band_with(
        &unpinned,
        &mainnet,
        safa::get_compact(&rounded),
        first,
        1_000_001
    ));
    assert!(band_with(
        &unpinned,
        &mainnet,
        mainnet.anchor_bits,
        first,
        1_000_001
    ));
}

/// A deployment compiles only with an interval CHIP-2025-03 allows: one that
/// divides 600.
#[test]
fn a_switch_to_an_interval_that_does_not_divide_600_does_not_compile() {
    let chipnet = safa::network_parameters("chipnet").unwrap();
    for (interval, compiles) in [
        (0, false),
        (7, false),
        (601, false),
        (30, true),
        (600, true),
    ] {
        let parameters = NetworkParameters {
            interval_switch: Some(IntervalSwitch {
                height: 332_450,
                interval,
            }),
            ..chipnet.clone()
        };
        assert_eq!(
            safa::compile_library_for(&parameters, "").is_ok(),
            compiles,
            "interval {interval}"
        );
    }
}

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
    // From 4,032 ten-minute blocks' worth of ticks: 4,032 ten-minute blocks,
    // or 40,320 one-minute ones.
    for (tick_delta, applies) in [
        (4_032 * 600, true),
        (4_031 * 600, false),
        (40_320 * 60, true),
        (40_319 * 60, false),
        (4_032 * 60, false),
        (100_000 * 600, true),
    ] {
        assert_eq!(
            safa::fallback_band_ok(safa::get_compact(&hard), 0x1804_dafe, tick_delta),
            applies,
            "{tick_delta} ticks"
        );
    }
}
