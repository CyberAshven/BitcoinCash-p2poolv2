// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! State-machine tests with real proofs and an in-memory RPC transcript.

use std::fs;

use bitcoindrpc::BitcoindRpcError;
use serde_json::{Value, json};

use super::config::VaultPoint;
use super::files::Journal;
use super::test_utils::{
    CATEGORY, CATEGORY_WIRE, CLAIM_TXID, ExpectedCall, FakeRpc, GENESIS, NEXT_STATE,
    runtime_fixture,
};
use super::wins::parse_win;

#[tokio::test(flavor = "current_thread")]
async fn dry_run_builds_real_win_without_rpc_or_file_mutations() {
    let mut fixture = runtime_fixture(FakeRpc::new(vec![]), true);
    let before_token = fs::read(&fixture.claimer.config.token_file).unwrap();
    let before_journal = fs::read(&fixture.claimer.config.journal).unwrap();
    fixture
        .claimer
        .claim_next(&fixture.vault, 330_015)
        .await
        .unwrap();
    fixture.claimer.client.assert_finished();
    assert_eq!(
        fs::read(&fixture.claimer.config.token_file).unwrap(),
        before_token
    );
    assert_eq!(
        fs::read(&fixture.claimer.config.journal).unwrap(),
        before_journal
    );
    assert!(fixture.claimer.journal.pending.is_empty());
    assert!(fixture.claimer.journal.claimed.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn accepted_broadcast_persists_pending_first_then_updates_vault_and_token_state() {
    let mut fixture = runtime_fixture(FakeRpc::new(vec![]), false);
    fixture.claimer.client = FakeRpc::new(vec![ExpectedCall {
        method: "sendrawtransaction",
        parameters: vec![json!(fixture.prepared.hex)],
        result: Ok(json!(CLAIM_TXID)),
        persisted_pending: Some((fixture.claimer.config.journal.clone(), CLAIM_TXID.into())),
    }]);
    fixture
        .claimer
        .claim_next(&fixture.vault, 330_015)
        .await
        .unwrap();
    fixture.claimer.client.assert_finished();
    let resumed = Journal::load(
        &fixture.claimer.config.journal,
        &fixture.claimer.config.vault,
    )
    .unwrap();
    assert_eq!(
        resumed.vault,
        VaultPoint {
            txid: CLAIM_TXID.into(),
            vout: 0
        }
    );
    assert_eq!(resumed.commitment.as_deref(), Some(NEXT_STATE));
    assert_eq!(resumed.claimed, vec![fixture.win.id()]);
    assert!(resumed.pending.is_empty());
    assert!(resumed.wins.is_empty());
    let tokens: Value =
        serde_json::from_slice(&fs::read(&fixture.claimer.config.token_file).unwrap()).unwrap();
    assert_eq!(tokens["tokens"][0]["safa"]["state"], NEXT_STATE);
    assert_eq!(tokens["tokens"][0]["safa"]["keep"], "unchanged");
    assert_eq!(tokens["tokens"][0]["other"], 123);
    assert_eq!(tokens["keep"], true);
}

#[tokio::test(flavor = "current_thread")]
async fn rejected_claim_keeps_original_outpoint_and_follows_it_on_the_next_pass() {
    let mut fixture = runtime_fixture(FakeRpc::new(vec![]), false);
    fixture.claimer.client = FakeRpc::new(vec![ExpectedCall {
        method: "sendrawtransaction",
        parameters: vec![json!(fixture.prepared.hex)],
        result: Err(BitcoindRpcError::RpcError {
            code: -26,
            message: "conflict".into(),
        }),
        persisted_pending: Some((fixture.claimer.config.journal.clone(), CLAIM_TXID.into())),
    }]);
    fixture
        .claimer
        .claim_next(&fixture.vault, 330_015)
        .await
        .unwrap();
    fixture.claimer.client.assert_finished();
    assert_eq!(fixture.claimer.journal.vault, fixture.claimer.config.vault);
    assert!(fixture.claimer.journal.pending.is_empty());
    assert!(fixture.claimer.journal.claimed.is_empty());
    fixture
        .claimer
        .journal
        .delays
        .insert(fixture.win.id(), u64::MAX);
    fixture.claimer.client = FakeRpc::new(vec![
        ExpectedCall {
            method: "getblockcount",
            parameters: vec![],
            result: Ok(json!(330_015)),
            persisted_pending: None,
        },
        ExpectedCall {
            method: "gettxout",
            parameters: vec![json!(GENESIS), json!(3), json!(true)],
            result: Ok(json!({"value":0.02,
                "scriptPubKey":{"hex":hex::encode(&fixture.claimer.covenant.locking_bytecode)},
                "tokenData":{"category":CATEGORY,"amount":"525000000000000",
                    "nft":{"capability":"mutable","commitment":"0300000011090500ffff001dba7d451a"}}})),
            persisted_pending: None,
        },
        ExpectedCall {
            method: "getblockhash",
            parameters: vec![json!(330_015)],
            result: Ok(json!("aa".repeat(32))),
            persisted_pending: None,
        },
    ]);
    fixture.claimer.pass().await.unwrap();
    fixture.claimer.client.assert_finished();
    assert_eq!(fixture.claimer.journal.vault, fixture.claimer.config.vault);
    assert!(fixture.claimer.journal.pending.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn uncertain_send_resumes_and_replays_only_the_exact_verified_transaction() {
    let mut fixture = runtime_fixture(FakeRpc::new(vec![]), false);
    fixture.claimer.client = FakeRpc::new(vec![ExpectedCall {
        method: "sendrawtransaction",
        parameters: vec![json!(fixture.prepared.hex)],
        result: Err(BitcoindRpcError::Transport {
            message: "connection lost".into(),
        }),
        persisted_pending: Some((fixture.claimer.config.journal.clone(), CLAIM_TXID.into())),
    }]);
    fixture
        .claimer
        .claim_next(&fixture.vault, 330_015)
        .await
        .unwrap();
    fixture.claimer.client.assert_finished();
    fixture.claimer.journal = Journal::load(
        &fixture.claimer.config.journal,
        &fixture.claimer.config.vault,
    )
    .unwrap();
    assert_eq!(fixture.claimer.journal.pending[0].hex, fixture.prepared.hex);
    assert_eq!(fixture.claimer.journal.vault, fixture.claimer.config.vault);
    fixture.claimer.client = FakeRpc::new(vec![
        ExpectedCall {
            method: "getrawtransaction",
            parameters: vec![json!(CLAIM_TXID), json!(true)],
            result: Err(BitcoindRpcError::RpcError {
                code: -5,
                message: "not found".into(),
            }),
            persisted_pending: None,
        },
        ExpectedCall {
            method: "sendrawtransaction",
            parameters: vec![json!(fixture.prepared.hex)],
            result: Ok(json!(CLAIM_TXID)),
            persisted_pending: Some((fixture.claimer.config.journal.clone(), CLAIM_TXID.into())),
        },
    ]);
    fixture
        .claimer
        .recover_pending(Some(&fixture.vault), &[])
        .await
        .unwrap();
    fixture.claimer.client.assert_finished();
    assert_eq!(fixture.claimer.journal.vault.txid, CLAIM_TXID);
    assert!(fixture.claimer.journal.pending.is_empty());
    assert_eq!(fixture.claimer.journal.claimed, vec![fixture.win.id()]);
}

#[tokio::test(flavor = "current_thread")]
async fn followed_pending_lineage_recovers_acceptance_without_transaction_index_lookup() {
    let mut fixture = runtime_fixture(FakeRpc::new(vec![]), false);
    fixture
        .claimer
        .journal
        .pending
        .push(fixture.prepared.clone());
    fixture.claimer.journal.vault = VaultPoint {
        txid: "bb".repeat(32),
        vout: 0,
    };
    fixture.claimer.journal.commitment = Some(NEXT_STATE.into());
    fixture
        .claimer
        .recover_pending(None, &[CLAIM_TXID.into(), "bb".repeat(32)])
        .await
        .unwrap();
    fixture.claimer.client.assert_finished();
    assert_eq!(fixture.claimer.journal.vault.txid, "bb".repeat(32));
    assert_eq!(
        fixture.claimer.journal.commitment.as_deref(),
        Some(NEXT_STATE)
    );
    assert_eq!(fixture.claimer.journal.claimed, vec![fixture.win.id()]);
    assert!(fixture.claimer.journal.pending.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn oldest_valid_win_waits_for_its_delay_instead_of_being_leapfrogged() {
    let mut fixture = runtime_fixture(FakeRpc::new(vec![]), false);
    let mut newer = parse_win(
        include_bytes!("../../test_data/tsafa-live-win-second.jsonl"),
        CATEGORY_WIRE,
    )
    .unwrap()
    .unwrap();
    newer.time = fixture.win.time + 1;
    assert_ne!(newer.id(), fixture.win.id());
    fixture
        .claimer
        .journal
        .delays
        .insert(fixture.win.id(), u64::MAX);
    fixture.claimer.journal.delays.insert(newer.id(), 0);
    fixture.claimer.journal.wins.push(newer);
    fixture
        .claimer
        .claim_next(&fixture.vault, 330_015)
        .await
        .unwrap();
    fixture.claimer.client.assert_finished();
    assert_eq!(fixture.claimer.journal.wins.len(), 2);
    assert!(fixture.claimer.journal.pending.is_empty());
    assert!(fixture.claimer.journal.claimed.is_empty());
}

#[test]
fn stale_win_payload_and_delay_are_pruned_but_drop_identity_is_retained() {
    let mut fixture = runtime_fixture(FakeRpc::new(vec![]), false);
    fixture.claimer.drop_stale_wins(330_015).unwrap();
    assert!(fixture.claimer.journal.wins.is_empty());
    assert!(fixture.claimer.journal.delays.is_empty());
    assert_eq!(fixture.claimer.journal.dropped, vec![fixture.win.id()]);
    fixture.claimer.client.assert_finished();
}

#[tokio::test(flavor = "current_thread")]
async fn temporarily_underfunded_win_is_retained_until_the_vault_can_pay() {
    let mut fixture = runtime_fixture(FakeRpc::new(vec![]), false);
    let mut underfunded = fixture.vault.clone();
    underfunded.value = 800;
    fixture
        .claimer
        .claim_next(&underfunded, 330_015)
        .await
        .unwrap();
    assert_eq!(fixture.claimer.journal.wins.len(), 1);
    assert!(fixture.claimer.journal.dropped.is_empty());
    assert!(fixture.claimer.journal.pending.is_empty());
    fixture.claimer.client = FakeRpc::new(vec![ExpectedCall {
        method: "sendrawtransaction",
        parameters: vec![json!(fixture.prepared.hex)],
        result: Ok(json!(CLAIM_TXID)),
        persisted_pending: Some((fixture.claimer.config.journal.clone(), CLAIM_TXID.into())),
    }]);
    fixture
        .claimer
        .claim_next(&fixture.vault, 330_015)
        .await
        .unwrap();
    fixture.claimer.client.assert_finished();
    assert_eq!(fixture.claimer.journal.claimed, vec![fixture.win.id()]);
    assert!(fixture.claimer.journal.wins.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn dry_run_reverifies_pending_claim_without_replay_or_persistent_changes() {
    let mut fixture = runtime_fixture(FakeRpc::new(vec![]), true);
    fixture
        .claimer
        .journal
        .pending
        .push(fixture.prepared.clone());
    fixture
        .claimer
        .journal
        .save(&fixture.claimer.config.journal)
        .unwrap();
    let before_journal = fs::read(&fixture.claimer.config.journal).unwrap();
    let before_token = fs::read(&fixture.claimer.config.token_file).unwrap();
    fixture.claimer.client = FakeRpc::new(vec![ExpectedCall {
        method: "getrawtransaction",
        parameters: vec![json!(CLAIM_TXID), json!(true)],
        result: Err(BitcoindRpcError::RpcError {
            code: -5,
            message: "not found".into(),
        }),
        persisted_pending: None,
    }]);
    fixture
        .claimer
        .recover_pending(Some(&fixture.vault), &[])
        .await
        .unwrap();
    fixture.claimer.client.assert_finished();
    assert_eq!(
        fs::read(&fixture.claimer.config.journal).unwrap(),
        before_journal
    );
    assert_eq!(
        fs::read(&fixture.claimer.config.token_file).unwrap(),
        before_token
    );
    assert_eq!(fixture.claimer.journal.pending.len(), 1);
}
