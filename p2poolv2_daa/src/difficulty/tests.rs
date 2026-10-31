// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;

#[test]
fn production_constants_come_from_the_node() {
    assert_eq!(ideal_share_interval(), 10);
    assert_eq!(production_half_life(), 600);
    assert!((submission_interval() - 10.0 / 3.0).abs() < 1e-12);
    assert_eq!(maximum_stratum_difficulty(), 4_294_967_296.0);
}

#[test]
fn difficulty_one_is_the_bitcoin_difficulty_one_target() {
    assert_eq!(compact_for_difficulty(1.0).to_consensus(), 0x1d00ffff);
    assert_eq!(
        difficulty_of(CompactTarget::from_consensus(0x1d00ffff)),
        1.0
    );
}

/// Every compact form is at most one part in 2^15 harder than asked, and
/// never easier, across the range the scenarios use.
#[test]
fn compact_for_difficulty_rounds_to_a_slightly_harder_target() {
    for difficulty in [1.0, 4.5, 1_164.0, 2_794.0, 2.33e6, 1.16e10, 2.3e13, 1e15] {
        let rounded = difficulty_of(compact_for_difficulty(difficulty));
        assert!(
            rounded >= difficulty * (1.0 - 1e-12),
            "{difficulty}: {rounded}"
        );
        assert!(
            rounded <= difficulty * (1.0 + 2f64.powi(-14)),
            "{difficulty}: {rounded}"
        );
    }
}

/// Mantissas just under and over a byte boundary normalise to valid
/// compact forms (no sign bit, two significant bytes).
#[test]
fn compact_for_difficulty_normalises_the_mantissa() {
    for difficulty in [255.99, 256.0, 256.01, 65_535.9, 65_536.0, 65_536.1] {
        let bits = compact_for_difficulty(difficulty).to_consensus();
        let mantissa = bits & 0x00ff_ffff;
        assert!(mantissa < 0x0080_0000, "{difficulty}: {bits:#x}");
        assert!(mantissa >= 0x0000_8000, "{difficulty}: {bits:#x}");
    }
}

#[test]
fn steady_difficulty_finds_one_share_per_ideal_interval() {
    assert!((steady_difficulty(HASHES_PER_DIFFICULTY / 10.0) - 1.0).abs() < 1e-12);
    // One BitAxe (1.2 TH/s) holds about 2,794 on the 10-second chain.
    assert!((steady_difficulty(1.2e12) - 2_793.97).abs() < 0.01);
}

#[test]
fn no_easier_than_keeps_the_harder_target() {
    let ceiling = CompactTarget::from_consensus(0x1b384bd7);
    let easier = CompactTarget::from_consensus(0x1c384bd7);
    let harder = CompactTarget::from_consensus(0x1a384bd7);
    assert_eq!(no_easier_than(easier, ceiling), ceiling);
    assert_eq!(no_easier_than(harder, ceiling), harder);
    assert_eq!(no_easier_than(ceiling, ceiling), ceiling);
}
