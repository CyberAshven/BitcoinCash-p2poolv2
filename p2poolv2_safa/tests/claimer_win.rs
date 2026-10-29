// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Public pool win parsing and offline tSAFA claim verification.

use libauth::decode_transaction_bch;
use p2poolv2_safa::claimer::config::VaultPoint;
use p2poolv2_safa::claimer::wins::{
    ClaimError, build_win_claim, candidates, parse_win, resolve_script, resolve_split,
};
use p2poolv2_safa::{SplitCommitment, Vault, hash256, split_preimage};
use serde_json::{Value, json};

const LIVE_WIN: &[u8] = include_bytes!("../test_data/tsafa-live-win.jsonl");
const CATEGORY_WIRE: &str = "fe00873edd1d64852faa9dcea2c9b16e9041a8fdf1a353314ba4a0d94e2d641e";
const CATEGORY_DISPLAY: &str = "1e642d4ed9a0a44b3153a3f1fda841906eb1c9a2ce9daa2f85641ddd3e8700fe";
const PAYOUT_SCRIPT: &str = "76a914c40a24bf5b3cfe561cf36afea1b59fe61383e80688ac";
const DONATION_SCRIPT: &str = "76a914b25a2f0a68073fad864f684bf26e350b4526354b88ac";
const GENESIS_TXID: &str = "8c5ecd6d4ce039490c233c9399cc1e591720218fd9712dc8c00edc172a06c471";

#[test]
fn parses_live_win_height_from_bip34_coinbase() {
    let win = parse_win(LIVE_WIN, CATEGORY_WIRE).unwrap().unwrap();
    assert_eq!(win.height().unwrap(), 330_015);
    assert_eq!(win.channel, 1);
    assert_eq!(win.job, 1);
    assert_eq!(win.time, 1_793_288_105);
}

#[test]
fn ignores_other_category_before_parsing_proof() {
    let line = json!({"category": "11".repeat(32), "proof": "invalid"});
    assert!(
        parse_win(&serde_json::to_vec(&line).unwrap(), CATEGORY_WIRE)
            .unwrap()
            .is_none()
    );
}

#[test]
fn rejects_malformed_json_win_line() {
    assert!(parse_win(b"{broken", CATEGORY_WIRE).is_err());
}

#[test]
fn rejects_malformed_proof_win_line() {
    let mut line: Value = serde_json::from_slice(LIVE_WIN).unwrap();
    line["proof"] = json!("43544d5002");
    assert!(parse_win(&serde_json::to_vec(&line).unwrap(), CATEGORY_WIRE).is_err());
}

#[test]
fn accepts_wins_without_sv2_channel() {
    let mut line: Value = serde_json::from_slice(LIVE_WIN).unwrap();
    line.as_object_mut().unwrap().remove("channel");
    line["job"] = json!("cpu-job");
    let win = parse_win(&serde_json::to_vec(&line).unwrap(), CATEGORY_WIRE)
        .unwrap()
        .unwrap();
    assert_eq!(win.channel, Value::Null);
    assert_eq!(win.job, "cpu-job");
}

#[test]
fn normalizes_hex_before_calculating_win_identity() {
    let original = parse_win(LIVE_WIN, CATEGORY_WIRE).unwrap().unwrap();
    let mut line: Value = serde_json::from_slice(LIVE_WIN).unwrap();
    line["proof"] = json!(original.proof.to_uppercase());
    line["category"] = json!(original.category.to_uppercase());
    line["payout_hash"] = json!(original.payout_hash.to_uppercase());
    let normalized = parse_win(&serde_json::to_vec(&line).unwrap(), CATEGORY_WIRE)
        .unwrap()
        .unwrap();
    assert_eq!(normalized.id(), original.id());
}

#[test]
fn selects_oldest_coinbase_height_first() {
    let current = parse_win(LIVE_WIN, CATEGORY_WIRE).unwrap().unwrap();
    let mut older = current.clone();
    let fixture: Value =
        serde_json::from_str(include_str!("../test_data/tsafa-chipnet-claim.json")).unwrap();
    older.proof = fixture["proof"].as_str().unwrap().to_owned();
    let selected = candidates(&[current, older], 329_000, 330_015, &[], &[]);
    assert_eq!(selected.len(), 2);
    assert_eq!(selected[0].height().unwrap(), 329_001);
    assert_eq!(selected[1].height().unwrap(), 330_015);
}

#[test]
fn excludes_win_at_current_state_height() {
    let win = parse_win(LIVE_WIN, CATEGORY_WIRE).unwrap().unwrap();
    assert!(candidates(&[win], 330_015, 330_020, &[], &[]).is_empty());
}

#[test]
fn defers_win_above_tip_height() {
    let win = parse_win(LIVE_WIN, CATEGORY_WIRE).unwrap().unwrap();
    assert!(candidates(&[win], 330_001, 330_014, &[], &[]).is_empty());
}

#[test]
fn never_selects_dropped_win_again() {
    let win = parse_win(LIVE_WIN, CATEGORY_WIRE).unwrap().unwrap();
    let dropped = vec![win.id()];
    assert!(candidates(&[win], 330_001, 330_015, &[], &dropped).is_empty());
}

#[test]
fn never_selects_claimed_win_again() {
    let win = parse_win(LIVE_WIN, CATEGORY_WIRE).unwrap().unwrap();
    let claimed = vec![win.id()];
    assert!(candidates(&[win], 330_001, 330_015, &claimed, &[]).is_empty());
}

#[test]
fn rotation_duplicates_are_selected_only_once() {
    let win = parse_win(LIVE_WIN, CATEGORY_WIRE).unwrap().unwrap();
    assert_eq!(
        candidates(&[win.clone(), win], 330_001, 330_015, &[], &[]).len(),
        1
    );
}

#[test]
fn resolves_payout_script_by_hash256() {
    let win = parse_win(LIVE_WIN, CATEGORY_WIRE).unwrap().unwrap();
    assert_eq!(
        resolve_script(
            &hex::decode(win.payout_hash).unwrap(),
            &[DONATION_SCRIPT.into(), PAYOUT_SCRIPT.into()]
        )
        .unwrap(),
        PAYOUT_SCRIPT
    );
}

#[test]
fn unknown_payout_script_is_skippable() {
    let win = parse_win(LIVE_WIN, CATEGORY_WIRE).unwrap().unwrap();
    assert!(matches!(
        resolve_script(
            &hex::decode(win.payout_hash).unwrap(),
            &[DONATION_SCRIPT.into()]
        ),
        Err(ClaimError::Unresolved(_))
    ));
}

#[test]
fn resolves_live_donation_split_and_zero_fee_slot() {
    let win = parse_win(LIVE_WIN, CATEGORY_WIRE).unwrap().unwrap();
    let split = resolve_split(&win.split, &[DONATION_SCRIPT.into()]).unwrap();
    assert_eq!(split["donation_bps"], 100);
    assert_eq!(split["donation_script"], DONATION_SCRIPT);
    assert_eq!(split["fee_bps"], 0);
    assert_eq!(split["fee_script"], "");
}

#[test]
fn resolves_both_nonzero_split_slots_by_hash256() {
    let preimage = split_preimage(&SplitCommitment {
        donation_bps: 100,
        donation_hash: hash256(&hex::decode(DONATION_SCRIPT).unwrap()),
        fee_bps: 200,
        fee_hash: hash256(&hex::decode(PAYOUT_SCRIPT).unwrap()),
    })
    .unwrap();
    let split = resolve_split(
        &hex::encode(preimage),
        &[PAYOUT_SCRIPT.into(), DONATION_SCRIPT.into()],
    )
    .unwrap();
    assert_eq!(split["donation_script"], DONATION_SCRIPT);
    assert_eq!(split["fee_script"], PAYOUT_SCRIPT);
    assert_eq!(split["fee_bps"], 200);
}

#[test]
fn unresolvable_donation_split_is_skippable() {
    let win = parse_win(LIVE_WIN, CATEGORY_WIRE).unwrap().unwrap();
    assert!(matches!(
        resolve_split(&win.split, &[]),
        Err(ClaimError::Unresolved(_))
    ));
}

#[test]
fn absent_split_requires_no_scripts() {
    assert_eq!(resolve_split("", &[]).unwrap(), Value::Null);
}

#[test]
fn real_pool_win_builds_and_verifies_against_deployed_tsafa_vault() {
    let win = parse_win(LIVE_WIN, CATEGORY_WIRE).unwrap().unwrap();
    let deployment = json!({
        "network": "chipnet", "threads": 4, "virtual_sats": "84000000000",
        "category": CATEGORY_DISPLAY,
    });
    let vault = Vault {
        category: hex::decode(CATEGORY_DISPLAY).unwrap(),
        value: 2_000_000,
        token_amount: 525_000_000_000_000,
        commitment: hex::decode("0300000011090500ffff001dba7d451a").unwrap(),
    };
    let claim = build_win_claim(
        &deployment,
        &win,
        &VaultPoint {
            txid: GENESIS_TXID.into(),
            vout: 3,
        },
        &vault,
        &[PAYOUT_SCRIPT.into()],
        &[DONATION_SCRIPT.into()],
        1,
    )
    .unwrap();
    assert_eq!(claim["coinbase_height"], 330_015);
    assert_eq!(
        claim["txid"],
        "e30dc911a420cae10ba767c4eaf22ee720f56400d8099cfcd943050631d6a47f"
    );
    assert_eq!(claim["size"], 2_345);
    assert_eq!(claim["size_bound"], 2_513);
    assert_eq!(claim["fee_sats"], "2513");
    assert_eq!(claim["target_bits"], "1d023ffd");
    assert_eq!(claim["next_state"], "030000001f090500fd3f021dd2872d1a");
    assert_eq!(claim["reward"], "1250000000");
    assert_eq!(claim["shares"]["miner"], "1237500000");
    assert_eq!(claim["shares"]["donation"], "12500000");
    assert_eq!(claim["shares"]["fee"], "0");
    let transaction =
        decode_transaction_bch(&hex::decode(claim["hex"].as_str().unwrap()).unwrap()).unwrap();
    assert_eq!(transaction.locktime, 330_015);
    assert_eq!(transaction.inputs[0].outpoint_index, 3);
    assert_eq!(transaction.outputs.len(), 3);
    assert_eq!(
        transaction.outputs[0].token.as_ref().unwrap().amount,
        524_998_750_000_000
    );
    assert_eq!(
        transaction.outputs[1].locking_bytecode,
        hex::decode(PAYOUT_SCRIPT).unwrap()
    );
    assert_eq!(
        transaction.outputs[2].locking_bytecode,
        hex::decode(DONATION_SCRIPT).unwrap()
    );
}

#[test]
fn real_win_against_another_thread_state_is_invalid() {
    let win = parse_win(LIVE_WIN, CATEGORY_WIRE).unwrap().unwrap();
    let deployment = json!({
        "network": "chipnet", "threads": 4, "virtual_sats": "84000000000",
        "category": CATEGORY_DISPLAY,
    });
    let vault = Vault {
        category: hex::decode(CATEGORY_DISPLAY).unwrap(),
        value: 2_000_000,
        token_amount: 525_000_000_000_000,
        commitment: hex::decode("0200000011090500ffff001dba7d451a").unwrap(),
    };
    assert!(matches!(
        build_win_claim(
            &deployment,
            &win,
            &VaultPoint {
                txid: GENESIS_TXID.into(),
                vout: 2
            },
            &vault,
            &[PAYOUT_SCRIPT.into()],
            &[DONATION_SCRIPT.into()],
            1,
        ),
        Err(ClaimError::Invalid(_))
    ));
}

#[test]
fn real_win_against_another_state_of_the_same_thread_is_invalid() {
    let win = parse_win(LIVE_WIN, CATEGORY_WIRE).unwrap().unwrap();
    let deployment = json!({
        "network": "chipnet", "threads": 4, "virtual_sats": "84000000000",
        "category": CATEGORY_DISPLAY,
    });
    let vault = Vault {
        category: hex::decode(CATEGORY_DISPLAY).unwrap(),
        value: 2_000_000,
        token_amount: 525_000_000_000_000,
        commitment: hex::decode("0300000012090500ffff001dba7d451a").unwrap(),
    };
    assert!(matches!(
        build_win_claim(
            &deployment,
            &win,
            &VaultPoint {
                txid: GENESIS_TXID.into(),
                vout: 3
            },
            &vault,
            &[PAYOUT_SCRIPT.into()],
            &[DONATION_SCRIPT.into()],
            1,
        ),
        Err(ClaimError::Invalid(_))
    ));
}

#[test]
fn real_win_can_claim_an_amm_moved_vault_with_the_same_commitment() {
    let win = parse_win(LIVE_WIN, CATEGORY_WIRE).unwrap().unwrap();
    let deployment = json!({
        "network": "chipnet", "threads": 4, "virtual_sats": "84000000000",
        "category": CATEGORY_DISPLAY,
    });
    let vault = Vault {
        category: hex::decode(CATEGORY_DISPLAY).unwrap(),
        value: 3_000_000,
        token_amount: 524_000_000_000_000,
        commitment: hex::decode("0300000011090500ffff001dba7d451a").unwrap(),
    };
    let claim = build_win_claim(
        &deployment,
        &win,
        &VaultPoint {
            txid: "aa".repeat(32),
            vout: 0,
        },
        &vault,
        &[PAYOUT_SCRIPT.into()],
        &[DONATION_SCRIPT.into()],
        1,
    )
    .unwrap();
    let transaction =
        decode_transaction_bch(&hex::decode(claim["hex"].as_str().unwrap()).unwrap()).unwrap();
    assert_eq!(transaction.inputs[0].outpoint_index, 0);
    assert_eq!(
        transaction.inputs[0].outpoint_transaction_hash,
        vec![0xaa; 32]
    );
    assert_eq!(claim["next_state"], "030000001f090500fd3f021dd2872d1a");
    assert_ne!(
        claim["txid"],
        "e30dc911a420cae10ba767c4eaf22ee720f56400d8099cfcd943050631d6a47f"
    );
}

#[test]
fn underfunded_vault_still_drops_a_win_committed_to_another_state() {
    let win = parse_win(LIVE_WIN, CATEGORY_WIRE).unwrap().unwrap();
    let deployment = json!({
        "network":"chipnet", "threads":4, "virtual_sats":"84000000000",
        "category":CATEGORY_DISPLAY,
    });
    let vault = Vault {
        category: hex::decode(CATEGORY_DISPLAY).unwrap(),
        value: 800,
        token_amount: 525_000_000_000_000,
        commitment: hex::decode("0300000012090500ffff001dba7d451a").unwrap(),
    };
    let error = build_win_claim(
        &deployment,
        &win,
        &VaultPoint {
            txid: GENESIS_TXID.into(),
            vout: 3,
        },
        &vault,
        &[PAYOUT_SCRIPT.into()],
        &[DONATION_SCRIPT.into()],
        1,
    )
    .unwrap_err();
    assert!(matches!(error, ClaimError::Invalid(reason) if reason.contains("proof commitment")));
}

#[test]
fn depleted_token_reserve_still_drops_a_win_committed_to_another_state() {
    let win = parse_win(LIVE_WIN, CATEGORY_WIRE).unwrap().unwrap();
    let deployment = json!({
        "network":"chipnet", "threads":4, "virtual_sats":"84000000000",
        "category":CATEGORY_DISPLAY,
    });
    let vault = Vault {
        category: hex::decode(CATEGORY_DISPLAY).unwrap(),
        value: 2_000_000,
        token_amount: 0,
        commitment: hex::decode("0300000012090500ffff001dba7d451a").unwrap(),
    };
    let error = build_win_claim(
        &deployment,
        &win,
        &VaultPoint {
            txid: GENESIS_TXID.into(),
            vout: 3,
        },
        &vault,
        &[PAYOUT_SCRIPT.into()],
        &[DONATION_SCRIPT.into()],
        1,
    )
    .unwrap_err();
    assert!(matches!(error, ClaimError::Invalid(reason) if reason.contains("proof commitment")));
}
