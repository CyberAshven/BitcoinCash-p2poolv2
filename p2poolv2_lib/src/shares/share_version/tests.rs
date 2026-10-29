use super::*;
use bitcoin::CompactTarget;
use bitcoin::consensus::{deserialize, serialize};
use std::convert::Infallible;

/// A tally with `supporting` units of work desiring `version` and the rest
/// desiring `version - 1`.
fn tally(version: u32, supporting: u64, total: u64) -> VersionTally {
    let mut tally = VersionTally::default();
    let unit = Target::from_compact(CompactTarget::from_consensus(0x1d00_ffff)).to_work();
    for _ in 0..supporting {
        tally.add(version, unit);
    }
    for _ in supporting..total {
        tally.add(version - 1, unit);
    }
    tally
}

fn never<T>() -> Result<T, Infallible> {
    panic!("the vote span must not be tallied")
}

#[test]
fn share_version_encodes_in_eight_bytes_and_defaults_to_this_build() {
    assert_eq!(ShareVersion::default(), ShareVersion::CURRENT);
    assert_eq!(
        ShareVersion::CURRENT,
        ShareVersion {
            version: 1,
            desired_version: 1
        }
    );
    let share_version = ShareVersion {
        version: 1,
        desired_version: 0x0102_0304,
    };
    let bytes = serialize(&share_version);
    assert_eq!(bytes, [1, 0, 0, 0, 4, 3, 2, 1]);
    assert_eq!(deserialize::<ShareVersion>(&bytes).unwrap(), share_version);
}

#[test]
fn the_vote_span_is_the_oldest_tenth_of_three_days() {
    assert_eq!(SHARE_VERSION_VOTE_DEPTH, 25_920);
    assert_eq!(SHARE_VERSION_VOTE_SPAN, 2_592);
}

#[test]
fn a_share_may_not_follow_an_unknown_version_or_desire_an_older_one() {
    assert_eq!(
        check_share_version_alone(ShareVersion {
            version: MAX_SUPPORTED_SHARE_VERSION + 1,
            desired_version: MAX_SUPPORTED_SHARE_VERSION + 1,
        }),
        Err(ShareVersionError::Unsupported {
            version: MAX_SUPPORTED_SHARE_VERSION + 1
        })
    );
    assert_eq!(
        check_share_version_alone(ShareVersion {
            version: 1,
            desired_version: 0,
        }),
        Err(ShareVersionError::DesiresOlder {
            version: 1,
            desired_version: 0
        })
    );
    // Desiring a version newer than this build knows is a vote, not an error.
    check_share_version_alone(ShareVersion {
        version: 1,
        desired_version: 7,
    })
    .unwrap();
}

#[test]
fn the_parent_version_is_followed_without_a_vote() {
    check_share_version(ShareVersion::CURRENT, 1, 0, never)
        .unwrap()
        .unwrap();
    check_share_version(ShareVersion::CURRENT, 1, 1_000_000, never)
        .unwrap()
        .unwrap();
}

#[test]
fn a_version_is_never_lowered_or_skipped() {
    let lowered = ShareVersion {
        version: 0,
        desired_version: 1,
    };
    assert_eq!(
        check_share_version(lowered, 1, 1_000_000, never).unwrap(),
        Err(ShareVersionError::NotParentOrNext {
            version: 0,
            parent_version: 1
        })
    );
}

/// Version 0 never existed; it stands in for "the version before this
/// build's", so the switch to version 1 exercises the rule.
#[test]
fn a_switch_needs_a_full_vote_depth_and_sixty_percent_of_the_span() {
    let switching = ShareVersion::CURRENT;
    assert_eq!(
        check_share_version(switching, 0, SHARE_VERSION_VOTE_DEPTH - 1, never).unwrap(),
        Err(ShareVersionError::SwitchWithoutHistory {
            parent_height: SHARE_VERSION_VOTE_DEPTH - 1
        })
    );
    assert_eq!(
        check_share_version(switching, 0, SHARE_VERSION_VOTE_DEPTH, || {
            Ok::<_, Infallible>(tally(1, 59, 100))
        })
        .unwrap(),
        Err(ShareVersionError::SwitchWithoutSupport { version: 1 })
    );
    check_share_version(switching, 0, SHARE_VERSION_VOTE_DEPTH, || {
        Ok::<_, Infallible>(tally(1, 60, 100))
    })
    .unwrap()
    .unwrap();
    // A store error while tallying is passed through.
    assert_eq!(
        check_share_version(switching, 0, SHARE_VERSION_VOTE_DEPTH, || Err(
            "read failed"
        )),
        Err("read failed")
    );
}

#[test]
fn a_node_switches_at_ninety_five_percent_of_the_span() {
    // This build implements no version past its own: it stays, untallied.
    assert_eq!(
        next_share_version(1, 1_000_000, never).unwrap(),
        ShareVersion::CURRENT
    );
    let stay = ShareVersion {
        version: 0,
        desired_version: MAX_SUPPORTED_SHARE_VERSION,
    };
    assert_eq!(
        next_share_version(0, SHARE_VERSION_VOTE_DEPTH - 1, never).unwrap(),
        stay
    );
    assert_eq!(
        next_share_version(0, SHARE_VERSION_VOTE_DEPTH, || {
            Ok::<_, Infallible>(tally(1, 94, 100))
        })
        .unwrap(),
        stay
    );
    assert_eq!(
        next_share_version(0, SHARE_VERSION_VOTE_DEPTH, || {
            Ok::<_, Infallible>(tally(1, 95, 100))
        })
        .unwrap(),
        ShareVersion::CURRENT
    );
}

#[test]
fn tally_weighs_by_work_and_counts_newer_desires_as_support() {
    let mut weighted = VersionTally::default();
    let easy = Target::from_compact(CompactTarget::from_consensus(0x1d00_ffff)).to_work();
    let hard = Target::from_compact(CompactTarget::from_consensus(0x1c00_ffff)).to_work();
    // One hard share (256 times the work) desiring version 2 outweighs 200
    // easy shares desiring version 1.
    weighted.add(2, hard);
    for _ in 0..200 {
        weighted.add(1, easy);
    }
    assert!(weighted.supports(2, 55));
    assert!(!weighted.supports(2, 60));
    assert!(weighted.supports(1, 100));
    // A desire for version 3 supports version 2 too.
    let mut newer = VersionTally::default();
    newer.add(3, easy);
    assert!(newer.supports(2, 100));
    assert!(!VersionTally::default().supports(1, 0));
    // By share target.
    let mut by_bits = VersionTally::default();
    by_bits.add_share(
        ShareVersion::CURRENT,
        CompactTarget::from_consensus(0x1b38_4bd7),
    );
    assert!(by_bits.supports(1, 100));
}
