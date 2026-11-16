// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;

#[test]
fn a_per_block_amount_scales_with_the_block_interval() {
    let ratio = BCH_BLOCK_INTERVAL_SECONDS / BCH_TEN_MINUTE_BLOCK_SECONDS;
    [
        (
            BCH_TEN_MINUTE_MAINNET_ABLA_FLOOR_BYTES,
            BCH_MAINNET_ABLA_FLOOR_BYTES,
        ),
        (
            BCH_TEN_MINUTE_CHIPNET_ABLA_FLOOR_BYTES,
            BCH_CHIPNET_ABLA_FLOOR_BYTES,
        ),
        (
            BCH_TEN_MINUTE_SCALENET_LIMIT_BYTES,
            BCH_SCALENET_LIMIT_BYTES,
        ),
    ]
    .into_iter()
    .for_each(|(ten_minute, scaled)| {
        assert_eq!(scaled, (ten_minute as f64 * ratio) as u64);
    });
}

#[test]
fn one_minute_blocks_have_a_tenth_of_the_ten_minute_floors() {
    // CHIP-2025-03 on mainnet and chipnet: 3.2 MB and 200 kB.
    assert_eq!(BCH_MAINNET_ABLA_FLOOR_BYTES, 3_200_000);
    assert_eq!(BCH_CHIPNET_ABLA_FLOOR_BYTES, 200_000);
    assert_eq!(BCH_SCALENET_LIMIT_BYTES, 25_600_000);
}
