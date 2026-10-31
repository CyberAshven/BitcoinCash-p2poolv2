// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::{tally_share_version_votes, tally_share_version_votes_from_headers};
use crate::shares::chain::chain_store_handle::MockChainStoreHandle;
use crate::shares::share_version::{
    SHARE_VERSION_VOTE_DEPTH, SHARE_VERSION_VOTE_SPAN, VersionTally,
};
use crate::store::writer::StoreError;
use crate::test_utils::{
    TestShareBlockBuilder, setup_test_chain_store_handle, store_test_header_chain,
};
use bitcoin::hashes::Hash;
use bitcoin::{BlockHash, CompactTarget, Work};
use p2poolv2_wallet::share_owner::ShareOwner;

/// The real header store has no height indexes: tallying must follow parent
/// links. Only the span desires version 2; its immediate neighbours desire 3.
#[tokio::test]
async fn share_version_tally_counts_exactly_the_stored_vote_span() {
    let (chain_store_handle, _store_dir) = setup_test_chain_store_handle(false).await;
    let template = TestShareBlockBuilder::new()
        .share_owner(ShareOwner::CashP2pkh([0x22; 20]))
        .build()
        .header;
    let first = SHARE_VERSION_VOTE_DEPTH - SHARE_VERSION_VOTE_SPAN;
    let hashes = store_test_header_chain(
        &chain_store_handle,
        &template,
        SHARE_VERSION_VOTE_DEPTH + 1,
        |depth, header| {
            header.bits = CompactTarget::from_consensus(0x207fffff);
            header.share_version.desired_version =
                if (first..SHARE_VERSION_VOTE_DEPTH).contains(&depth) {
                    2
                } else {
                    3
                };
        },
    );

    // Work is 2 per header: all 2,592 votes, including both span endpoints.
    assert_eq!(SHARE_VERSION_VOTE_SPAN, 2_592);
    let mut expected = VersionTally::default();
    expected.add(2, Work::from_hex("0x1440").unwrap());
    let tally = tally_share_version_votes_from_headers(hashes[0], |blockhash| {
        chain_store_handle.get_share_header(blockhash)
    })
    .unwrap();
    assert_eq!(tally, expected);
    assert!(!tally.supports(3, 1));

    // Moving the parent by one drops the first version-2 vote and admits the
    // version-3 header immediately after the previous span's last vote.
    let mut shifted_expected = VersionTally::default();
    shifted_expected.add(2, Work::from_hex("0x143e").unwrap());
    shifted_expected.add(3, Work::from_hex("0x02").unwrap());
    let shifted_tally = tally_share_version_votes_from_headers(hashes[1], |blockhash| {
        chain_store_handle.get_share_header(blockhash)
    })
    .unwrap();
    assert_eq!(shifted_tally, shifted_expected);
}

#[tokio::test]
async fn share_version_tally_stops_at_stored_genesis_before_the_span() {
    let (chain_store_handle, _store_dir) = setup_test_chain_store_handle(false).await;
    let template = TestShareBlockBuilder::new()
        .share_owner(ShareOwner::CashP2pkh([0x22; 20]))
        .build()
        .header;
    let hashes = store_test_header_chain(&chain_store_handle, &template, 3, |_, header| {
        header.share_version.desired_version = 2;
    });
    assert_eq!(
        tally_share_version_votes_from_headers(hashes[0], |blockhash| {
            chain_store_handle.get_share_header(blockhash)
        })
        .unwrap(),
        VersionTally::default()
    );
}

#[tokio::test]
async fn share_version_tally_skips_stored_genesis_immediately_before_the_span() {
    let (chain_store_handle, _store_dir) = setup_test_chain_store_handle(false).await;
    let template = TestShareBlockBuilder::new()
        .share_owner(ShareOwner::CashP2pkh([0x22; 20]))
        .build()
        .header;
    let hashes = store_test_header_chain(
        &chain_store_handle,
        &template,
        SHARE_VERSION_VOTE_DEPTH - SHARE_VERSION_VOTE_SPAN,
        |_, header| {
            header.share_version.desired_version = 2;
        },
    );
    assert_eq!(
        tally_share_version_votes_from_headers(hashes[0], |blockhash| {
            chain_store_handle.get_share_header(blockhash)
        })
        .unwrap(),
        VersionTally::default()
    );
}

#[tokio::test]
async fn share_version_tally_counts_stored_genesis_as_the_first_vote() {
    let (chain_store_handle, _store_dir) = setup_test_chain_store_handle(false).await;
    let template = TestShareBlockBuilder::new()
        .share_owner(ShareOwner::CashP2pkh([0x22; 20]))
        .build()
        .header;
    let hashes = store_test_header_chain(
        &chain_store_handle,
        &template,
        SHARE_VERSION_VOTE_DEPTH - SHARE_VERSION_VOTE_SPAN + 1,
        |_, header| {
            header.bits = CompactTarget::from_consensus(0x207fffff);
            header.share_version.desired_version = 2;
        },
    );
    let mut expected = VersionTally::default();
    expected.add(2, Work::from_hex("0x02").unwrap());
    assert_eq!(
        tally_share_version_votes_from_headers(hashes[0], |blockhash| {
            chain_store_handle.get_share_header(blockhash)
        })
        .unwrap(),
        expected
    );
}

#[tokio::test]
async fn share_version_tally_stops_at_stored_genesis_inside_the_span() {
    let (chain_store_handle, _store_dir) = setup_test_chain_store_handle(false).await;
    let template = TestShareBlockBuilder::new()
        .share_owner(ShareOwner::CashP2pkh([0x22; 20]))
        .build()
        .header;
    let hashes = store_test_header_chain(
        &chain_store_handle,
        &template,
        SHARE_VERSION_VOTE_DEPTH - SHARE_VERSION_VOTE_SPAN + 100,
        |_, header| {
            header.bits = CompactTarget::from_consensus(0x207fffff);
            header.share_version.desired_version = 2;
        },
    );
    // Genesis terminates the walk after exactly 100 shares of work 2.
    let mut expected = VersionTally::default();
    expected.add(2, Work::from_hex("0xc8").unwrap());
    assert_eq!(
        tally_share_version_votes_from_headers(hashes[0], |blockhash| {
            chain_store_handle.get_share_header(blockhash)
        })
        .unwrap(),
        expected
    );
}

#[tokio::test]
async fn share_version_tally_counts_stored_genesis_as_the_last_vote() {
    let (chain_store_handle, _store_dir) = setup_test_chain_store_handle(false).await;
    let template = TestShareBlockBuilder::new()
        .share_owner(ShareOwner::CashP2pkh([0x22; 20]))
        .build()
        .header;
    let hashes = store_test_header_chain(
        &chain_store_handle,
        &template,
        SHARE_VERSION_VOTE_DEPTH,
        |_, header| {
            header.bits = CompactTarget::from_consensus(0x207fffff);
            header.share_version.desired_version = 2;
        },
    );
    let mut expected = VersionTally::default();
    expected.add(2, Work::from_hex("0x1440").unwrap());
    assert_eq!(
        tally_share_version_votes_from_headers(hashes[0], |blockhash| {
            chain_store_handle.get_share_header(blockhash)
        })
        .unwrap(),
        expected
    );
}

#[tokio::test]
async fn share_version_tally_is_empty_at_the_terminal_hash() {
    let (chain_store_handle, _store_dir) = setup_test_chain_store_handle(false).await;
    assert_eq!(
        tally_share_version_votes_from_headers(BlockHash::all_zeros(), |blockhash| {
            chain_store_handle.get_share_header(blockhash)
        })
        .unwrap(),
        VersionTally::default()
    );
}

#[tokio::test]
async fn share_version_tally_fails_on_a_missing_stored_header() {
    let (chain_store_handle, _store_dir) = setup_test_chain_store_handle(false).await;
    assert!(matches!(
        tally_share_version_votes_from_headers(
            BlockHash::from_byte_array([0x01; 32]),
            |blockhash| { chain_store_handle.get_share_header(blockhash) }
        ),
        Err(StoreError::NotFound(_))
    ));
}

/// The validation handle wrapper forwards a read failure unchanged to callers.
#[test]
fn share_version_tally_wrapper_propagates_a_header_error() {
    let mut chain_store_handle = MockChainStoreHandle::default();
    chain_store_handle
        .expect_get_share_header()
        .times(1)
        .returning(|_| Err(StoreError::NotFound("missing vote header".to_string())));
    assert!(matches!(
        tally_share_version_votes(BlockHash::from_byte_array([0x01; 32]), &chain_store_handle),
        Err(StoreError::NotFound(_))
    ));
}
