// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! The difficulty band of the Bitcoin Cash share chain
//! (`docs/sharechain/small-miners-design.md`, part 2).
//!
//! A share may be up to `C` times harder than the share chain's minimum,
//! never easier:
//!
//! ```text
//! T_asert / C  <=  T  <=  T_asert
//! ```
//!
//! `T` is the share's own target, its `bits`, whose exact work weighs the
//! share in the PPLNS window and in fork choice. `T_asert` is the share's
//! minimum, the ASERT target from its parent, which the share carries as
//! `asert_bits`. The next share's ASERT is anchored at that minimum, not at
//! the parent's own `bits`, so a heavier share does not make the next
//! minimum heavier. `C` is the share's `band_cap` in units of 1/1024, one
//! step from its parent's ([`next_band_cap`]): it is carried from share to
//! share the way ABLA carries Bitcoin Cash's block size limit from block to
//! block, so a node checks it from the parent's header alone. `wanted_bits`
//! is the target the share's node wanted for its miner before the cap, the
//! signal that moves `C`. Every comparison uses the exact 256-bit targets
//! the compact forms decode to.
//!
//! These are share-chain rules. A Bitcoin Cash node never sees them: the
//! blocks P2Pool finds stay ordinary Bitcoin Cash blocks.

use crate::pool_difficulty::{U512, target_to_u512, u512_to_target};
use crate::shares::share_block::ShareHeader;
use crate::shares::weak::next_weak_ratio;
use bitcoin::{CompactTarget, Target, Work};
use std::fmt;

/// `band_cap` of a band of one, `C = 1`: a share's target is the minimum,
/// as before the band.
pub const BAND_CAP_UNIT: u32 = 1024;

/// `C_MAX`: a share is at most 64 times harder than the minimum.
pub const BAND_CAP_MAX_MULTIPLE: u32 = 64;

/// The largest `band_cap`, `C_MAX` in units of 1/1024.
pub const BAND_CAP_MAX: u32 = BAND_CAP_MAX_MULTIPLE * BAND_CAP_UNIT;

/// The genesis share's `band_cap`: a band of one.
pub const GENESIS_BAND_CAP: u32 = BAND_CAP_UNIT;

/// One step of the band cap is `1 / BAND_STEP_DIVISOR` of the cap, times
/// the parent's work multiple when it rises: about 355 heavy shares double
/// `C` and about 355 others halve it.
pub const BAND_STEP_DIVISOR: u32 = 512;

/// Name of the band rule in Bitcoin Cash protocol strings: a share up to
/// `C_MAX` (64) times harder than the minimum.
pub const BCH_BAND_RULE_TAG: &str = "band-c64";

/// What the band rule reads from a share's parent header: its own target,
/// its minimum, the target its node wanted and its band cap, and for the
/// weak ratio, carried the same way (`shares::weak`), the parent's weak
/// ratio and how many weak shares it carried.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParentBand {
    /// The parent's own target.
    pub bits: CompactTarget,
    /// The parent's minimum, its `T_asert`.
    pub asert_bits: CompactTarget,
    /// The target the parent's node wanted for its miner before the cap.
    pub wanted_bits: CompactTarget,
    /// The parent's band cap, in units of 1/1024.
    pub band_cap: u32,
    /// The parent's weak ratio, in units of 1/1024.
    pub weak_ratio: u32,
    /// How many weak shares the parent carried.
    pub weak_count: u16,
}

impl ParentBand {
    /// The band fields of `header` as the parent of the next share.
    pub fn from_header(header: &ShareHeader) -> Self {
        Self {
            bits: header.bits,
            asert_bits: header.asert_bits,
            wanted_bits: header.wanted_bits,
            band_cap: header.band_cap,
            weak_ratio: header.weak_ratio,
            weak_count: header.weak_count,
        }
    }
}

/// The band cap of a share whose parent is `parent`: one step from the
/// parent's.
///
/// ```text
/// u               = min(work(parent.wanted_bits) / work(parent.asert_bits),
///                       C_MAX), an integer, at least 1
/// wanted more     = parent.wanted_bits is harder than
///                   parent.asert_bits * 1024 / parent.band_cap
///
/// band_cap(child) = parent.band_cap + parent.band_cap * u / 512   if the parent wanted more
/// band_cap(child) = parent.band_cap - parent.band_cap / 512       otherwise
/// then clamped to 1024 .. C_MAX * 1024
/// ```
///
/// A share that wanted more pushes `C` up in proportion to the work its node
/// wanted, even when the cap held its own target at the minimum; any share
/// that did not want more pulls it down a little. `u` counts the wanted
/// target rather than the share's own, so a minority of shares wanting
/// heavier ones opens a band of one: counted by their own bits, at `C = 1`
/// they would weigh one each and `C` would rise only once more than half of
/// all shares wanted more. Integer arithmetic only, so every node computes
/// the same cap.
pub fn next_band_cap(parent: &ParentBand) -> u32 {
    let band_cap = u64::from(parent.band_cap);
    let step_divisor = u64::from(BAND_STEP_DIVISOR);
    let next = if parent_wanted_more(parent) {
        band_cap.saturating_add(band_cap * u64::from(work_multiple(parent)) / step_divisor)
    } else {
        band_cap - band_cap / step_divisor
    };
    //* The clamp keeps the result within u32.
    next.clamp(u64::from(BAND_CAP_UNIT), u64::from(BAND_CAP_MAX)) as u32
}

/// `u`, the work the parent's node wanted in units of its minimum's work:
/// `work(wanted_bits) / work(asert_bits)` rounded down, each the integer work
/// `Target::to_work` gives (`2^256 / (T + 1)` rounded down), as the PPLNS
/// window and fork choice count it. At most `C_MAX`, and at least 1.
fn work_multiple(parent: &ParentBand) -> u32 {
    let wanted_work = work_to_u512(Target::from_compact(parent.wanted_bits).to_work());
    let minimum_work = work_to_u512(Target::from_compact(parent.asert_bits).to_work());
    let ratio = wanted_work / minimum_work;
    let multiple = if ratio > U512::from(BAND_CAP_MAX_MULTIPLE) {
        BAND_CAP_MAX_MULTIPLE
    } else {
        ratio.low_u32()
    };
    multiple.max(1)
}

/// `work` as a `U512`, from its little-endian bytes.
fn work_to_u512(work: Work) -> U512 {
    let mut padded = [0u8; 64];
    padded[..32].copy_from_slice(&work.to_le_bytes());
    U512::from_little_endian(&padded)
}

/// Whether the parent's node wanted a harder target than its band allowed:
/// `wanted_bits` harder than `asert_bits * 1024 / band_cap`, compared
/// exactly as `wanted * band_cap < minimum * 1024`.
fn parent_wanted_more(parent: &ParentBand) -> bool {
    let wanted = target_to_u512(Target::from_compact(parent.wanted_bits));
    let minimum = target_to_u512(Target::from_compact(parent.asert_bits));
    wanted * U512::from(parent.band_cap) < minimum * U512::from(BAND_CAP_UNIT)
}

/// Why a share's band is invalid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BandError {
    /// The share's band cap is not one step from its parent's.
    BandCap { declared: u32, expected: u32 },
    /// The share's target is easier than its minimum.
    EasierThanMinimum { bits: u32, asert_bits: u32 },
    /// The share's target is harder than its band cap allows.
    HarderThanCap {
        bits: u32,
        asert_bits: u32,
        band_cap: u32,
    },
    /// The target the share's node wanted is easier than the share's own.
    WantedEasierThanBits { wanted_bits: u32, bits: u32 },
    /// The share's weak ratio is not one step from its parent's
    /// (`shares::weak::next_weak_ratio`).
    WeakRatio { declared: u32, expected: u32 },
}

impl fmt::Display for BandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BandError::BandCap { declared, expected } => write!(
                formatter,
                "band cap {declared} is not one step from the parent's, which gives {expected}"
            ),
            BandError::EasierThanMinimum { bits, asert_bits } => write!(
                formatter,
                "bits {bits:#010x} are easier than the minimum {asert_bits:#010x}"
            ),
            BandError::HarderThanCap {
                bits,
                asert_bits,
                band_cap,
            } => write!(
                formatter,
                "bits {bits:#010x} are more than {band_cap}/1024 times harder than the minimum {asert_bits:#010x}"
            ),
            BandError::WantedEasierThanBits { wanted_bits, bits } => write!(
                formatter,
                "wanted bits {wanted_bits:#010x} are easier than the share's bits {bits:#010x}"
            ),
            BandError::WeakRatio { declared, expected } => write!(
                formatter,
                "weak ratio {declared} is not one step from the parent's, which gives {expected}"
            ),
        }
    }
}

impl std::error::Error for BandError {}

/// The band rule for a Bitcoin Cash share on `parent`: its band cap is one
/// step from the parent's ([`next_band_cap`]), its target lies in
/// `T_asert / C <= T <= T_asert` with `C = band_cap / 1024` its own cap, and
/// the target its node wanted is at least as hard as its own.
///
/// `T_asert` is the share's `asert_bits`; the caller checks that it is the
/// ASERT target from the parent, as it checked the share's bits before the
/// band. The bounds compare exact 256-bit targets, the lower one as
/// `T * band_cap >= T_asert * 1024`.
///
/// The share's weak ratio, the second state each Bitcoin Cash share carries
/// from its parent's header, must likewise be one step from the parent's
/// (`shares::weak::next_weak_ratio`), so the same three call sites check it:
/// full validation, header sync and block receipt.
pub fn check_share_band(header: &ShareHeader, parent: &ParentBand) -> Result<(), BandError> {
    let expected_band_cap = next_band_cap(parent);
    let expected_weak_ratio = next_weak_ratio(parent.weak_ratio, parent.weak_count);
    let minimum = target_to_u512(Target::from_compact(header.asert_bits));
    let target = target_to_u512(Target::from_compact(header.bits));
    let wanted = target_to_u512(Target::from_compact(header.wanted_bits));
    if header.weak_ratio != expected_weak_ratio {
        Err(BandError::WeakRatio {
            declared: header.weak_ratio,
            expected: expected_weak_ratio,
        })
    } else if header.band_cap != expected_band_cap {
        Err(BandError::BandCap {
            declared: header.band_cap,
            expected: expected_band_cap,
        })
    } else if target > minimum {
        Err(BandError::EasierThanMinimum {
            bits: header.bits.to_consensus(),
            asert_bits: header.asert_bits.to_consensus(),
        })
    } else if target * U512::from(header.band_cap) < minimum * U512::from(BAND_CAP_UNIT) {
        Err(BandError::HarderThanCap {
            bits: header.bits.to_consensus(),
            asert_bits: header.asert_bits.to_consensus(),
            band_cap: header.band_cap,
        })
    } else if wanted > target {
        Err(BandError::WantedEasierThanBits {
            wanted_bits: header.wanted_bits.to_consensus(),
            bits: header.bits.to_consensus(),
        })
    } else {
        Ok(())
    }
}

/// The targets of a job's shares: the target they are mined at and the one
/// their node wanted for the miner before the band cap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JobBand {
    /// The share target the job commits to.
    pub bits: CompactTarget,
    /// The target the miner would have had without the cap, at least as
    /// hard as `bits`.
    pub wanted_bits: CompactTarget,
}

/// The band of a job whose miner should mine shares `wanted_multiple /
/// 1024` times the minimum's work: the target at that multiple, at most the
/// band's `band_cap / 1024`, and the target it would have had without the
/// cap. A multiple below one (1024) gives the minimum.
///
/// Node policy, not a rule: what makes a share valid is
/// [`check_share_band`]. Each target is the easiest compact target at its
/// multiple, so the share stays inside the band after compact rounding.
pub fn heavier_job_band(asert_bits: CompactTarget, band_cap: u32, wanted_multiple: u64) -> JobBand {
    let wanted_multiple = wanted_multiple.max(u64::from(BAND_CAP_UNIT));
    let capped_multiple = wanted_multiple
        .min(u64::from(band_cap))
        .max(u64::from(BAND_CAP_UNIT));
    let bits = target_at_multiple(asert_bits, capped_multiple);
    let wanted_bits = if wanted_multiple > capped_multiple {
        harder_of(target_at_multiple(asert_bits, wanted_multiple), bits)
    } else {
        bits
    };
    JobBand { bits, wanted_bits }
}

/// The easiest compact target at `multiple / 1024` times the minimum's
/// work: the smallest `T` a compact form writes with `T * multiple >=
/// minimum * 1024`. At most the minimum when `multiple` is at least 1024.
fn target_at_multiple(minimum: CompactTarget, multiple: u64) -> CompactTarget {
    let scaled = target_to_u512(Target::from_compact(minimum)) * U512::from(BAND_CAP_UNIT);
    let multiple = U512::from(multiple.max(1));
    let rounded_up = (scaled + multiple - U512::one()) / multiple;
    compact_at_or_above(rounded_up)
}

/// The easiest compact target at least `target` exactly: the truncating
/// `to_compact_lossy`, one mantissa step up when it lost precision.
pub(crate) fn compact_at_or_above(target: U512) -> CompactTarget {
    let truncated = u512_to_target(target).to_compact_lossy();
    if target_to_u512(Target::from_compact(truncated)) >= target {
        truncated
    } else {
        next_compact_up(truncated)
    }
}

/// The compact target one mantissa step above `compact`. A mantissa that
/// overflows its 23 bits carries into the exponent. Only called on a
/// truncated target, whose exponent is above 3: smaller targets are written
/// exactly.
fn next_compact_up(compact: CompactTarget) -> CompactTarget {
    let consensus = compact.to_consensus();
    let exponent = consensus >> 24;
    let mantissa = consensus & 0x007f_ffff;
    let bumped = if mantissa == 0x007f_ffff {
        ((exponent + 1) << 24) | 0x0000_8000
    } else {
        (exponent << 24) | (mantissa + 1)
    };
    CompactTarget::from_consensus(bumped)
}

/// The harder of two compact targets.
fn harder_of(first: CompactTarget, second: CompactTarget) -> CompactTarget {
    if Target::from_compact(first) < Target::from_compact(second) {
        first
    } else {
        second
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shares::weak::{GENESIS_WEAK_RATIO, WEAK_FILL};
    use crate::test_utils::TestShareBlockBuilder;
    use p2poolv2_wallet::share_owner::ShareOwner;

    #[test]
    fn a_weak_ratio_one_step_from_the_parents_is_inside_the_band() {
        let parent = ParentBand {
            bits: CompactTarget::from_consensus(MINIMUM),
            asert_bits: CompactTarget::from_consensus(MINIMUM),
            wanted_bits: CompactTarget::from_consensus(MINIMUM),
            band_cap: BAND_CAP_UNIT,
            weak_ratio: 256_000,
            weak_count: WEAK_FILL + 1,
        };
        let header = TestShareBlockBuilder::new()
            .share_owner(ShareOwner::CashP2pkh([0x11; 20]))
            .bits(CompactTarget::from_consensus(MINIMUM))
            .weak_ratio(256_000 - 1_000)
            .build()
            .header;

        assert_eq!(check_share_band(&header, &parent), Ok(()));
    }

    #[test]
    fn a_weak_ratio_off_its_step_is_refused() {
        let parent = ParentBand {
            bits: CompactTarget::from_consensus(MINIMUM),
            asert_bits: CompactTarget::from_consensus(MINIMUM),
            wanted_bits: CompactTarget::from_consensus(MINIMUM),
            band_cap: BAND_CAP_UNIT,
            weak_ratio: 256_000,
            weak_count: WEAK_FILL + 1,
        };
        let header = TestShareBlockBuilder::new()
            .share_owner(ShareOwner::CashP2pkh([0x11; 20]))
            .bits(CompactTarget::from_consensus(MINIMUM))
            .weak_ratio(256_000)
            .build()
            .header;

        assert_eq!(
            check_share_band(&header, &parent),
            Err(BandError::WeakRatio {
                declared: 256_000,
                expected: 256_000 - 1_000,
            })
        );
    }

    /// A minimum whose mantissa divides by 128, so the targets 2 to 128 times
    /// harder are written exactly: `0x1c400000` is `2^222`.
    const MINIMUM: u32 = 0x1c40_0000;
    const MINIMUM_OVER_TWO: u32 = 0x1c20_0000;
    const MINIMUM_OVER_FOUR: u32 = 0x1c10_0000;
    const MINIMUM_OVER_EIGHT: u32 = 0x1c08_0000;
    /// A target far beyond `C_MAX` times harder than [`MINIMUM`]: `2^207`.
    const FAR_HARDER: u32 = 0x1b00_8000;

    #[test]
    fn the_rule_tag_names_c_max() {
        assert_eq!(BCH_BAND_RULE_TAG, format!("band-c{BAND_CAP_MAX_MULTIPLE}"));
    }

    #[test]
    fn a_genesis_parent_gives_a_band_of_one() {
        let genesis = ParentBand {
            bits: CompactTarget::from_consensus(MINIMUM),
            asert_bits: CompactTarget::from_consensus(MINIMUM),
            wanted_bits: CompactTarget::from_consensus(MINIMUM),
            band_cap: GENESIS_BAND_CAP,
            weak_ratio: GENESIS_WEAK_RATIO,
            weak_count: 0,
        };

        assert_eq!(next_band_cap(&genesis), BAND_CAP_UNIT);
    }

    #[test]
    fn a_parent_that_wanted_no_more_pulls_the_cap_down_one_step() {
        // Wanting twice the minimum's work in a band of two is not more.
        let parent = ParentBand {
            bits: CompactTarget::from_consensus(MINIMUM_OVER_TWO),
            asert_bits: CompactTarget::from_consensus(MINIMUM),
            wanted_bits: CompactTarget::from_consensus(MINIMUM_OVER_TWO),
            band_cap: 2048,
            weak_ratio: GENESIS_WEAK_RATIO,
            weak_count: 0,
        };

        assert_eq!(next_band_cap(&parent), 2048 - 4);
    }

    #[test]
    fn a_parent_that_wanted_more_pushes_the_cap_up_by_the_work_it_wanted() {
        // At the cap of four and wanting eight: u = 8, so the cap rises by
        // 4096 * 8 / 512.
        let parent = ParentBand {
            bits: CompactTarget::from_consensus(MINIMUM_OVER_FOUR),
            asert_bits: CompactTarget::from_consensus(MINIMUM),
            wanted_bits: CompactTarget::from_consensus(MINIMUM_OVER_EIGHT),
            band_cap: 4096,
            weak_ratio: GENESIS_WEAK_RATIO,
            weak_count: 0,
        };

        assert_eq!(next_band_cap(&parent), 4096 + 64);
    }

    #[test]
    fn a_parent_held_at_the_minimum_counts_the_work_it_wanted() {
        // A band of one held the share at the minimum, but its node wanted
        // twice the work: u = 2.
        let parent = ParentBand {
            bits: CompactTarget::from_consensus(MINIMUM),
            asert_bits: CompactTarget::from_consensus(MINIMUM),
            wanted_bits: CompactTarget::from_consensus(MINIMUM_OVER_TWO),
            band_cap: BAND_CAP_UNIT,
            weak_ratio: GENESIS_WEAK_RATIO,
            weak_count: 0,
        };

        assert_eq!(next_band_cap(&parent), BAND_CAP_UNIT + 4);
    }

    #[test]
    fn the_work_multiple_is_at_most_c_max() {
        // Far more than 64 times the minimum's work wanted: u = 64.
        let parent = ParentBand {
            bits: CompactTarget::from_consensus(MINIMUM_OVER_TWO),
            asert_bits: CompactTarget::from_consensus(MINIMUM),
            wanted_bits: CompactTarget::from_consensus(FAR_HARDER),
            band_cap: 2048,
            weak_ratio: GENESIS_WEAK_RATIO,
            weak_count: 0,
        };

        assert_eq!(next_band_cap(&parent), 2048 + 256);
    }

    #[test]
    fn the_work_multiple_rounds_down() {
        // Wanting a target 2/5 of the minimum is 2.5 times its work: u = 2.
        let minimum = target_to_u512(Target::from_compact(CompactTarget::from_consensus(MINIMUM)));
        let two_fifths = u512_to_target(minimum * U512::from(2) / U512::from(5));
        let parent = ParentBand {
            bits: CompactTarget::from_consensus(MINIMUM_OVER_TWO),
            asert_bits: CompactTarget::from_consensus(MINIMUM),
            wanted_bits: two_fifths.to_compact_lossy(),
            band_cap: 2048,
            weak_ratio: GENESIS_WEAK_RATIO,
            weak_count: 0,
        };

        assert_eq!(next_band_cap(&parent), 2048 + 8);
    }

    #[test]
    fn the_cap_stops_at_c_max() {
        let parent = ParentBand {
            bits: CompactTarget::from_consensus(MINIMUM_OVER_EIGHT),
            asert_bits: CompactTarget::from_consensus(MINIMUM),
            wanted_bits: CompactTarget::from_consensus(FAR_HARDER),
            band_cap: BAND_CAP_MAX,
            weak_ratio: GENESIS_WEAK_RATIO,
            weak_count: 0,
        };

        assert_eq!(next_band_cap(&parent), BAND_CAP_MAX);
    }

    #[test]
    fn the_cap_never_falls_below_one() {
        let parent = ParentBand {
            bits: CompactTarget::from_consensus(MINIMUM),
            asert_bits: CompactTarget::from_consensus(MINIMUM),
            wanted_bits: CompactTarget::from_consensus(MINIMUM),
            band_cap: BAND_CAP_UNIT,
            weak_ratio: GENESIS_WEAK_RATIO,
            weak_count: 0,
        };

        assert_eq!(next_band_cap(&parent), BAND_CAP_UNIT);
    }

    /// At `C = 1`, one share in ten whose node wanted 30 times the minimum's
    /// work opens the band: its step up (u = 30) outweighs the nine steps
    /// down of the shares that wanted nothing more. Counted by the shares'
    /// own bits, all at the minimum, the cap would stay at one.
    #[test]
    fn a_minority_wanting_thirty_times_the_work_opens_a_band_of_one() {
        let minimum = target_to_u512(Target::from_compact(CompactTarget::from_consensus(MINIMUM)));
        let wanted_thirty_times = u512_to_target(minimum / U512::from(30)).to_compact_lossy();
        let light_step = |band_cap: u32| {
            next_band_cap(&ParentBand {
                bits: CompactTarget::from_consensus(MINIMUM),
                asert_bits: CompactTarget::from_consensus(MINIMUM),
                wanted_bits: CompactTarget::from_consensus(MINIMUM),
                band_cap,
                weak_ratio: GENESIS_WEAK_RATIO,
                weak_count: 0,
            })
        };

        let band_cap = next_band_cap(&ParentBand {
            bits: CompactTarget::from_consensus(MINIMUM),
            asert_bits: CompactTarget::from_consensus(MINIMUM),
            wanted_bits: wanted_thirty_times,
            band_cap: BAND_CAP_UNIT,
            weak_ratio: GENESIS_WEAK_RATIO,
            weak_count: 0,
        });
        assert_eq!(band_cap, BAND_CAP_UNIT + 60);
        let band_cap = light_step(band_cap);
        let band_cap = light_step(band_cap);
        let band_cap = light_step(band_cap);
        let band_cap = light_step(band_cap);
        let band_cap = light_step(band_cap);
        let band_cap = light_step(band_cap);
        let band_cap = light_step(band_cap);
        let band_cap = light_step(band_cap);
        let band_cap = light_step(band_cap);

        assert_eq!(band_cap, 1066);
    }

    #[test]
    fn a_share_at_the_minimum_is_inside_the_band() {
        // The parent at its minimum with cap 4104 gives its child 4104 - 8 =
        // 4096, a band of four.
        let parent = ParentBand {
            bits: CompactTarget::from_consensus(MINIMUM),
            asert_bits: CompactTarget::from_consensus(MINIMUM),
            wanted_bits: CompactTarget::from_consensus(MINIMUM),
            band_cap: 4104,
            weak_ratio: GENESIS_WEAK_RATIO,
            weak_count: 0,
        };
        let header = TestShareBlockBuilder::new()
            .share_owner(ShareOwner::CashP2pkh([0x11; 20]))
            .bits(CompactTarget::from_consensus(MINIMUM))
            .band(
                CompactTarget::from_consensus(MINIMUM),
                CompactTarget::from_consensus(MINIMUM),
                4096,
            )
            .build()
            .header;

        assert_eq!(check_share_band(&header, &parent), Ok(()));
    }

    #[test]
    fn a_share_at_the_cap_is_inside_the_band() {
        let parent = ParentBand {
            bits: CompactTarget::from_consensus(MINIMUM),
            asert_bits: CompactTarget::from_consensus(MINIMUM),
            wanted_bits: CompactTarget::from_consensus(MINIMUM),
            band_cap: 4104,
            weak_ratio: GENESIS_WEAK_RATIO,
            weak_count: 0,
        };
        let header = TestShareBlockBuilder::new()
            .share_owner(ShareOwner::CashP2pkh([0x11; 20]))
            .bits(CompactTarget::from_consensus(MINIMUM_OVER_FOUR))
            .band(
                CompactTarget::from_consensus(MINIMUM),
                CompactTarget::from_consensus(MINIMUM_OVER_FOUR),
                4096,
            )
            .build()
            .header;

        assert_eq!(check_share_band(&header, &parent), Ok(()));
    }

    #[test]
    fn a_share_one_compact_step_past_the_cap_is_refused() {
        let parent = ParentBand {
            bits: CompactTarget::from_consensus(MINIMUM),
            asert_bits: CompactTarget::from_consensus(MINIMUM),
            wanted_bits: CompactTarget::from_consensus(MINIMUM),
            band_cap: 4104,
            weak_ratio: GENESIS_WEAK_RATIO,
            weak_count: 0,
        };
        let header = TestShareBlockBuilder::new()
            .share_owner(ShareOwner::CashP2pkh([0x11; 20]))
            .bits(CompactTarget::from_consensus(0x1c0f_ffff))
            .band(
                CompactTarget::from_consensus(MINIMUM),
                CompactTarget::from_consensus(0x1c0f_ffff),
                4096,
            )
            .build()
            .header;

        assert_eq!(
            check_share_band(&header, &parent),
            Err(BandError::HarderThanCap {
                bits: 0x1c0f_ffff,
                asert_bits: MINIMUM,
                band_cap: 4096,
            })
        );
    }

    #[test]
    fn a_share_twice_past_the_cap_is_refused() {
        let parent = ParentBand {
            bits: CompactTarget::from_consensus(MINIMUM),
            asert_bits: CompactTarget::from_consensus(MINIMUM),
            wanted_bits: CompactTarget::from_consensus(MINIMUM),
            band_cap: 4104,
            weak_ratio: GENESIS_WEAK_RATIO,
            weak_count: 0,
        };
        let header = TestShareBlockBuilder::new()
            .share_owner(ShareOwner::CashP2pkh([0x11; 20]))
            .bits(CompactTarget::from_consensus(MINIMUM_OVER_EIGHT))
            .band(
                CompactTarget::from_consensus(MINIMUM),
                CompactTarget::from_consensus(MINIMUM_OVER_EIGHT),
                4096,
            )
            .build()
            .header;

        assert_eq!(
            check_share_band(&header, &parent),
            Err(BandError::HarderThanCap {
                bits: MINIMUM_OVER_EIGHT,
                asert_bits: MINIMUM,
                band_cap: 4096,
            })
        );
    }

    #[test]
    fn a_share_easier_than_the_minimum_is_refused() {
        let parent = ParentBand {
            bits: CompactTarget::from_consensus(MINIMUM),
            asert_bits: CompactTarget::from_consensus(MINIMUM),
            wanted_bits: CompactTarget::from_consensus(MINIMUM),
            band_cap: 4104,
            weak_ratio: GENESIS_WEAK_RATIO,
            weak_count: 0,
        };
        let header = TestShareBlockBuilder::new()
            .share_owner(ShareOwner::CashP2pkh([0x11; 20]))
            .bits(CompactTarget::from_consensus(0x1c40_0001))
            .band(
                CompactTarget::from_consensus(MINIMUM),
                CompactTarget::from_consensus(0x1c40_0001),
                4096,
            )
            .build()
            .header;

        assert_eq!(
            check_share_band(&header, &parent),
            Err(BandError::EasierThanMinimum {
                bits: 0x1c40_0001,
                asert_bits: MINIMUM,
            })
        );
    }

    #[test]
    fn a_band_cap_off_its_step_is_refused() {
        let parent = ParentBand {
            bits: CompactTarget::from_consensus(MINIMUM),
            asert_bits: CompactTarget::from_consensus(MINIMUM),
            wanted_bits: CompactTarget::from_consensus(MINIMUM),
            band_cap: 4104,
            weak_ratio: GENESIS_WEAK_RATIO,
            weak_count: 0,
        };
        let header = TestShareBlockBuilder::new()
            .share_owner(ShareOwner::CashP2pkh([0x11; 20]))
            .bits(CompactTarget::from_consensus(MINIMUM))
            .band(
                CompactTarget::from_consensus(MINIMUM),
                CompactTarget::from_consensus(MINIMUM),
                4104,
            )
            .build()
            .header;

        assert_eq!(
            check_share_band(&header, &parent),
            Err(BandError::BandCap {
                declared: 4104,
                expected: 4096,
            })
        );
    }

    #[test]
    fn wanted_bits_easier_than_the_share_are_refused() {
        let parent = ParentBand {
            bits: CompactTarget::from_consensus(MINIMUM),
            asert_bits: CompactTarget::from_consensus(MINIMUM),
            wanted_bits: CompactTarget::from_consensus(MINIMUM),
            band_cap: 4104,
            weak_ratio: GENESIS_WEAK_RATIO,
            weak_count: 0,
        };
        let header = TestShareBlockBuilder::new()
            .share_owner(ShareOwner::CashP2pkh([0x11; 20]))
            .bits(CompactTarget::from_consensus(MINIMUM_OVER_FOUR))
            .band(
                CompactTarget::from_consensus(MINIMUM),
                CompactTarget::from_consensus(MINIMUM_OVER_TWO),
                4096,
            )
            .build()
            .header;

        assert_eq!(
            check_share_band(&header, &parent),
            Err(BandError::WantedEasierThanBits {
                wanted_bits: MINIMUM_OVER_TWO,
                bits: MINIMUM_OVER_FOUR,
            })
        );
    }

    #[test]
    fn a_job_at_one_is_at_the_minimum() {
        assert_eq!(
            heavier_job_band(
                CompactTarget::from_consensus(MINIMUM),
                4096,
                u64::from(BAND_CAP_UNIT)
            ),
            JobBand {
                bits: CompactTarget::from_consensus(MINIMUM),
                wanted_bits: CompactTarget::from_consensus(MINIMUM),
            }
        );
    }

    #[test]
    fn a_job_inside_the_cap_wants_its_own_target() {
        assert_eq!(
            heavier_job_band(CompactTarget::from_consensus(MINIMUM), 4096, 2048),
            JobBand {
                bits: CompactTarget::from_consensus(MINIMUM_OVER_TWO),
                wanted_bits: CompactTarget::from_consensus(MINIMUM_OVER_TWO),
            }
        );
    }

    #[test]
    fn a_job_past_the_cap_mines_at_the_cap_and_wants_more() {
        assert_eq!(
            heavier_job_band(CompactTarget::from_consensus(MINIMUM), 4096, 8192),
            JobBand {
                bits: CompactTarget::from_consensus(MINIMUM_OVER_FOUR),
                wanted_bits: CompactTarget::from_consensus(MINIMUM_OVER_EIGHT),
            }
        );
    }

    /// The mini ceiling's mantissa does not divide by 64, so the target at
    /// the cap is rounded; it is rounded to stay inside the band.
    #[test]
    fn a_job_at_c_max_on_an_inexact_minimum_is_inside_the_band() {
        let minimum = CompactTarget::from_consensus(0x1c38_4bd7);
        let job = heavier_job_band(minimum, BAND_CAP_MAX, u64::from(BAND_CAP_MAX));
        let parent = ParentBand {
            bits: minimum,
            asert_bits: minimum,
            wanted_bits: CompactTarget::from_consensus(FAR_HARDER),
            band_cap: BAND_CAP_MAX,
            weak_ratio: GENESIS_WEAK_RATIO,
            weak_count: 0,
        };
        let header = TestShareBlockBuilder::new()
            .share_owner(ShareOwner::CashP2pkh([0x11; 20]))
            .bits(job.bits)
            .band(minimum, job.wanted_bits, BAND_CAP_MAX)
            .build()
            .header;

        assert_eq!(check_share_band(&header, &parent), Ok(()));
        assert!(Target::from_compact(job.bits) < Target::from_compact(minimum));
    }

    #[test]
    fn a_compact_step_up_carries_into_the_exponent() {
        assert_eq!(
            next_compact_up(CompactTarget::from_consensus(0x1c7f_ffff)),
            CompactTarget::from_consensus(0x1d00_8000)
        );
    }
}
