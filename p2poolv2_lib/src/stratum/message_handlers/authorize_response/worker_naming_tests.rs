// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Auto-assigned worker names: a bare-address login gets its own worker key
//! (`addr.bitaxe`, then `addr.worker1`) instead of joining every bare login
//! of that address in one anonymous, blank-named bucket.

use super::{
    MAX_AUTO_WORKER_ATTEMPTS, MAX_AUTO_WORKER_NAME_LEN, MAX_USER_AGENT_PREFIX_LEN,
    allocate_worker_name, candidate_worker_name, handle_authorize, user_agent_prefix,
};
use crate::stratum::difficulty_adjuster::DifficultyAdjuster;
use crate::stratum::messages::{Message, SimpleRequest};
use crate::stratum::server::StratumContext;
use crate::stratum::session::Session;
use crate::test_utils::test_bch_stratum_context;
use bitcoin::hashes::Hash;
use bitcoin::{Address, Network, PubkeyHash};
use p2poolv2_config::DEFAULT_VERSION_MASK;
use p2poolv2_wallet::bch_share::{BchShareNetwork, encode_payout_address};

/// The payout address of the test miner, in its regtest cashaddr spelling:
/// a bare login is exactly this string, with no `.worker` on the end.
fn payout() -> String {
    let address = Address::p2pkh(PubkeyHash::from_byte_array([0x22; 20]), Network::Regtest);
    encode_payout_address(&address, BchShareNetwork::Regtest).unwrap()
}

/// Authorize `username` against `ctx`, with the session already carrying
/// `user_agent` as `mining.subscribe` would have left it. Every session in
/// one test shares its context, so they share one name roster.
async fn authorize(
    ctx: &StratumContext,
    username: &str,
    user_agent: Option<&str>,
) -> Session<DifficultyAdjuster> {
    let mut session = Session::<DifficultyAdjuster>::new(1, 1, None, DEFAULT_VERSION_MASK);
    session.user_agent = user_agent.map(str::to_string);
    let request = SimpleRequest::new_authorize(1, username.to_string(), Some("x".to_string()));
    let messages = handle_authorize(request, &mut session, ctx.clone())
        .await
        .unwrap();
    let Message::Response(response) = &messages[0] else {
        panic!("expected an authorize response");
    };
    assert!(
        response.error.is_none(),
        "the login must be accepted, got {:?}",
        response.error
    );
    session
}

/// A bare address with a bitaxe's user agent names its worker after the
/// device: the dashboard row reads `addr.bitaxe`, not a blank cell.
#[tokio::test]
async fn a_bare_address_takes_its_name_from_the_user_agent() {
    let (ctx, _emissions, _dirs) = test_bch_stratum_context("http://127.0.0.1:1").await;
    let session = authorize(&ctx, &payout(), Some("bitaxe/24.4.0")).await;
    assert_eq!(session.workername.as_deref(), Some("bitaxe"));
}

/// Two bare-address logins of the same address with the same device agent
/// get one name each: the second takes the number behind it.
#[tokio::test]
async fn a_second_device_of_the_same_kind_numbers_up() {
    let (ctx, _emissions, _dirs) = test_bch_stratum_context("http://127.0.0.1:1").await;
    let first = authorize(&ctx, &payout(), Some("bitaxe/24.4.0")).await;
    assert_eq!(first.workername.as_deref(), Some("bitaxe"));

    let second = authorize(&ctx, &payout(), Some("bitaxe/24.4.0")).await;
    assert_eq!(second.workername.as_deref(), Some("bitaxe2"));
}

/// A login with no user agent at all -- a miner that authorized before
/// subscribing -- falls back to numbered names.
#[tokio::test]
async fn a_login_without_a_user_agent_numbers_up() {
    let (ctx, _emissions, _dirs) = test_bch_stratum_context("http://127.0.0.1:1").await;
    let first = authorize(&ctx, &payout(), None).await;
    assert_eq!(first.workername.as_deref(), Some("worker1"));

    let second = authorize(&ctx, &payout(), None).await;
    assert_eq!(second.workername.as_deref(), Some("worker2"));
}

/// An agent with nothing nameable (`/`, spaces, punctuation only) is no
/// device name, so the numeric fallback takes over.
#[tokio::test]
async fn an_agent_with_nothing_nameable_numbers_up() {
    let (ctx, _emissions, _dirs) = test_bch_stratum_context("http://127.0.0.1:1").await;
    let session = authorize(&ctx, &payout(), Some("/// ::::")).await;
    assert_eq!(session.workername.as_deref(), Some("worker1"));
}

/// An explicit worker stands as sent: the allocator never runs for it, and
/// no number is appended.
#[tokio::test]
async fn an_explicit_worker_name_is_left_alone() {
    let (ctx, _emissions, _dirs) = test_bch_stratum_context("http://127.0.0.1:1").await;
    let bare = authorize(&ctx, &payout(), Some("bitaxe/24.4.0")).await;
    assert_eq!(bare.workername.as_deref(), Some("bitaxe"));

    let explicit = authorize(&ctx, &format!("{}.rig1", payout()), Some("bitaxe/24.4.0")).await;
    assert_eq!(explicit.workername.as_deref(), Some("rig1"));
}

/// A name a live connection already holds is never reissued, even when it
/// was set explicitly rather than allocated.
#[tokio::test]
async fn a_name_held_by_an_explicit_worker_is_not_reissued() {
    let (ctx, _emissions, _dirs) = test_bch_stratum_context("http://127.0.0.1:1").await;
    let explicit = authorize(&ctx, &format!("{}.bitaxe", payout()), None).await;
    assert_eq!(explicit.workername.as_deref(), Some("bitaxe"));

    let bare = authorize(&ctx, &payout(), Some("bitaxe/24.4.0")).await;
    assert_eq!(
        bare.workername.as_deref(),
        Some("bitaxe2"),
        "the allocator must skip the explicitly held bitaxe"
    );
}

/// The metrics roster answers the allocator: a claimed name is held only
/// while its claim's connection is live, so a disconnect releases it.
#[tokio::test]
async fn a_disconnect_releases_the_name_for_the_next_login() {
    let (ctx, _emissions, _dirs) = test_bch_stratum_context("http://127.0.0.1:1").await;
    let name = allocate_worker_name(&ctx, "addr", None).await.unwrap();
    assert_eq!(name, "worker1");

    let taken = allocate_worker_name(&ctx, "addr", None).await.unwrap();
    assert_eq!(taken, "worker2", "the first login's claim must be skipped");

    // The first login disconnects, as its session would on close.
    ctx.metrics
        .decrement_worker_count(Some("addr".to_string()), "worker1".to_string())
        .await
        .unwrap();

    let reused = allocate_worker_name(&ctx, "addr", None).await.unwrap();
    assert_eq!(
        reused, "worker1",
        "a disconnected miner's number must be reusable"
    );
}

/// A name held under one address does not hold it under another: each
/// address numbers its own workers.
#[tokio::test]
async fn one_address_holding_a_name_leaves_it_free_for_another() {
    let (ctx, _emissions, _dirs) = test_bch_stratum_context("http://127.0.0.1:1").await;
    allocate_worker_name(&ctx, "addr1", Some("bitaxe/24.4.0"))
        .await
        .unwrap();

    let other = allocate_worker_name(&ctx, "addr2", Some("bitaxe/24.4.0"))
        .await
        .unwrap();
    assert_eq!(other, "bitaxe");
}

/// Both schemes count up against one roster, so the numeric fallback skips
/// a live `workerN` too.
#[tokio::test]
async fn the_numeric_fallback_skips_a_live_numbered_name() {
    let (ctx, _emissions, _dirs) = test_bch_stratum_context("http://127.0.0.1:1").await;
    ctx.metrics
        .increment_worker_count("addr".to_string(), "worker1".to_string())
        .await
        .unwrap();

    let name = allocate_worker_name(&ctx, "addr", None).await.unwrap();
    assert_eq!(name, "worker2");
}

/// The first run of allowed characters, lowercased: the device in front of
/// the version, and only that.
#[test]
fn the_user_agent_prefix_is_the_device_and_nothing_after_it() {
    assert_eq!(user_agent_prefix("bitaxe/24.4.0"), "bitaxe");
    assert_eq!(user_agent_prefix("Braiins OS/3.0"), "braiins");
    assert_eq!(user_agent_prefix("cpuminer 2.5.1"), "cpuminer");
    assert_eq!(user_agent_prefix("4.9.0-cgminer"), "4");
    assert_eq!(user_agent_prefix(" /// "), "");
}

/// The prefix is bounded, so a hostile user agent cannot fill the username
/// with it.
#[test]
fn the_user_agent_prefix_is_bounded() {
    let long = "a".repeat(500);
    assert_eq!(user_agent_prefix(&long).len(), MAX_USER_AGENT_PREFIX_LEN);
}

/// A candidate stays inside the username limit at the furthest ordinal the
/// allocator ever reaches, with the longest prefix it ever produces.
#[test]
fn a_candidate_worker_name_stays_within_the_username_limit() {
    let prefix = "x".repeat(MAX_USER_AGENT_PREFIX_LEN);
    let candidate = candidate_worker_name(Some(&prefix), MAX_AUTO_WORKER_ATTEMPTS);
    assert!(candidate.len() <= MAX_AUTO_WORKER_NAME_LEN);

    let numeric = candidate_worker_name(None, MAX_AUTO_WORKER_ATTEMPTS);
    assert!(numeric.len() <= MAX_AUTO_WORKER_NAME_LEN);
}
