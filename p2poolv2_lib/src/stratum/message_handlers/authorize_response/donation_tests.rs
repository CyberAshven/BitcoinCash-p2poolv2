// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! `donation=` and other comma options on a Stratum V1 login.

use super::handle_authorize;
use crate::config::PoolMode;
use crate::stratum::difficulty_adjuster::DifficultyAdjuster;
use crate::stratum::messages::{Message, SimpleRequest};
use crate::stratum::session::Session;
use crate::test_utils::test_bch_stratum_context;
use bitcoin::hashes::Hash;
use bitcoin::{Address, Network, PubkeyHash};
use p2poolv2_config::DEFAULT_VERSION_MASK;
use p2poolv2_wallet::bch_share::{BchShareAddress, BchShareNetwork, encode_payout_address};
use p2poolv2_wallet::share_owner::ShareOwner;

fn payout() -> String {
    let address = Address::p2pkh(PubkeyHash::from_byte_array([0x22; 20]), Network::Regtest);
    encode_payout_address(&address, BchShareNetwork::Regtest).unwrap()
}

/// Authorize on a Bitcoin Cash regtest node in `mode`, returning the session
/// and the error message of a refusal.
async fn authorize(
    username: String,
    password: Option<&str>,
    mode: PoolMode,
) -> (Session<DifficultyAdjuster>, Option<String>) {
    let (mut ctx, _emissions, _dirs) = test_bch_stratum_context("http://127.0.0.1:1").await;
    ctx.mode = mode;
    let mut session = Session::<DifficultyAdjuster>::new(1, 1, None, DEFAULT_VERSION_MASK);
    let request = SimpleRequest::new_authorize(1, username, password.map(str::to_string));
    let messages = handle_authorize(request, &mut session, ctx).await.unwrap();
    let Message::Response(response) = &messages[0] else {
        panic!("expected an authorize response");
    };
    let error = response
        .error
        .as_ref()
        .map(|error| error.message.to_string());
    (session, error)
}

/// The key agreed with Pickaxe, after a comma in the username.
#[tokio::test]
async fn a_username_option_chooses_the_donation() {
    let (session, error) = authorize(
        format!("{}.rig1,donation=0.5", payout()),
        Some("x"),
        PoolMode::P2poolv2,
    )
    .await;

    assert_eq!(error, None);
    assert_eq!(session.donation, Some(50));
    assert_eq!(session.username, Some(format!("{}.rig1", payout())));
    assert_eq!(session.workername.as_deref(), Some("rig1"));
}

/// Pickaxe's own donation key is not P2Pool's: it is ignored, and P2Pool's
/// own key beside it still applies.
#[tokio::test]
async fn pickaxe_donation_is_ignored_beside_donation() {
    let (session, error) = authorize(
        format!("{}.rig1,pickaxe-donation=0,donation=0.5", payout()),
        Some("x"),
        PoolMode::P2poolv2,
    )
    .await;
    assert_eq!(error, None);
    assert_eq!(session.donation, Some(50));

    let (session, error) = authorize(
        format!("{}.rig1,pickaxe-donation=0", payout()),
        None,
        PoolMode::P2poolv2,
    )
    .await;
    assert_eq!(error, None);
    assert_eq!(session.donation, None, "the node's own donation applies");
}

#[tokio::test]
async fn a_password_option_chooses_the_donation() {
    let (session, error) = authorize(payout(), Some("d=500,donation=1"), PoolMode::P2poolv2).await;
    assert_eq!(error, None);
    assert_eq!(session.donation, Some(100));
}

#[tokio::test]
async fn without_the_option_the_node_donation_applies() {
    let (session, error) = authorize(payout(), Some("x"), PoolMode::P2poolv2).await;
    assert_eq!(error, None);
    assert_eq!(session.donation, None);
    assert!(session.username.is_some());
}

/// A bad value is refused with a reason, and the miner may correct it.
#[tokio::test]
async fn a_bad_donation_is_refused_with_its_reason() {
    for (username, password, reason) in [
        (format!("{},donation=0.25", payout()), None, "steps of 0.5"),
        (format!("{},donation=11", payout()), None, "10% cap"),
        (
            format!("{},donation=1%", payout()),
            None,
            "without a % sign",
        ),
        (payout(), Some("donation="), "donation="),
        (
            format!("{},donation=1", payout()),
            Some("donation=0"),
            "twice",
        ),
    ] {
        let (session, error) = authorize(username.clone(), password, PoolMode::P2poolv2).await;
        let error = error.unwrap_or_else(|| panic!("{username} was accepted"));
        assert!(error.contains(reason), "{username}: {error}");
        assert!(session.username.is_none(), "{username}");
        assert_eq!(session.donation, None);
        assert!(session.auth_failed_once);
    }
}

/// A Hydrapool node builds no share chain, so it cannot honour a miner's
/// donation and says so; other keys still pass.
#[tokio::test]
async fn hydrapool_refuses_donation_but_not_unknown_keys() {
    let (session, error) = authorize(
        format!("{},donation=0", payout()),
        None,
        PoolMode::Hydrapool,
    )
    .await;
    assert!(error.unwrap().contains("cannot honour"));
    assert!(session.username.is_none());

    let (session, error) = authorize(
        format!("{},pickaxe-donation=0", payout()),
        None,
        PoolMode::Hydrapool,
    )
    .await;
    assert_eq!(error, None);
    assert!(session.username.is_some());
}

/// `p2p=` after a comma in the username names the share owner, as in an SV2
/// identity.
#[tokio::test]
async fn a_username_option_names_the_share_owner() {
    let (mut ctx, _emissions, _dirs) = test_bch_stratum_context("http://127.0.0.1:1").await;
    ctx.cash_share = None;
    let share_address = BchShareAddress::new(BchShareNetwork::Regtest, [0x33; 20]).encode();
    let mut session = Session::<DifficultyAdjuster>::new(1, 1, None, DEFAULT_VERSION_MASK);
    let request = SimpleRequest::new_authorize(
        1,
        format!("{}.rig1,donation=0,p2p={share_address}", payout()),
        Some("x".to_string()),
    );

    handle_authorize(request, &mut session, ctx).await.unwrap();

    assert_eq!(session.share_owner, Some(ShareOwner::CashP2pkh([0x33; 20])));
    assert_eq!(session.donation, Some(0));
}
