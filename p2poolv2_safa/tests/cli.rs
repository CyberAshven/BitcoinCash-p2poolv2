// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

mod test_utils;

use std::fs;
use std::process::Command;

use libauth::{decode_transaction_bch, encode_data_push};
use p2poolv2_safa::cli::{read_json, run};
use p2poolv2_safa::{CLAIM_FIXED_BYTES, create_covenant, decode_state, hash256};
use serde_json::{Value, json};
use tempfile::tempdir;

use test_utils::cli::{assert_cli_case, fixture, run_cli_json, successful_json};

#[test]
fn parity_params_chipnet() {
    assert_cli_case("params-chipnet");
}

#[test]
fn parity_params_mainnet() {
    assert_cli_case("params-mainnet");
}

#[test]
fn parity_params_one_thread_zero_virtual_satoshis() {
    assert_cli_case("params-one-thread-zero");
}

#[test]
fn parity_params_maximum_constants() {
    assert_cli_case("params-max-constants");
}

#[test]
fn parity_leaf_without_split() {
    assert_cli_case("leaf-plain");
}

#[test]
fn parity_leaf_with_split() {
    assert_cli_case("leaf-split");
}

#[test]
fn parity_work_with_split() {
    assert_cli_case("work-split");
}

#[test]
fn parity_rebuilt_pickaxe_claim() {
    assert_cli_case("claim-pickaxe-rebuilt");
}

#[test]
fn parity_claim_two_satoshis_per_byte() {
    assert_cli_case("claim-two-sat");
}

#[test]
fn parity_claim_three_satoshis_and_later_locktime() {
    assert_cli_case("claim-three-sat-later");
}

#[test]
fn parity_unlock_pickaxe_claim() {
    assert_cli_case("unlock-pickaxe");
}

#[test]
fn parity_verify_pickaxe_claim() {
    assert_cli_case("verify-pickaxe");
}

#[test]
fn parity_genesis_four_threads() {
    assert_cli_case("genesis-four");
}

#[test]
fn parity_genesis_sixty_three_threads() {
    assert_cli_case("genesis-sixty-three");
}

#[test]
fn params_prints_the_deployment_covenant_and_fee_constant() {
    let deployment = fixture("tsafa-chipnet-fixture.json")["deployment"].clone();
    let output = successful_json(run_cli_json("params", &[deployment]));
    let covenant = create_covenant("chipnet", 4, 84_000_000_000).unwrap();
    assert_eq!(
        output["redeem_script"],
        hex::encode(&covenant.redeem_script)
    );
    assert_eq!(
        output["fee_k"],
        CLAIM_FIXED_BYTES + 3 + covenant.redeem_script.len()
    );
    assert!(
        output["vault_address"]
            .as_str()
            .unwrap()
            .starts_with("bchtest:r")
    );
}

#[test]
fn chipnet_tsafa_fixture_reproduces_byte_for_byte() {
    let fixture = fixture("tsafa-chipnet-fixture.json");
    let output = successful_json(run_cli_json("params", &[fixture["deployment"].clone()]));
    assert_eq!(output, fixture["params"]);
    let covenant = create_covenant("chipnet", 4, 84_000_000_000).unwrap();
    assert_eq!(
        output["locking_bytecode"],
        format!("aa20{}87", hex::encode(hash256(&covenant.redeem_script)))
    );
    assert_eq!(
        output["lock_hash"],
        hex::encode(hash256(&covenant.locking_bytecode))
    );
    assert_eq!(
        fixture["genesis"]["funding"]["txid"],
        fixture["deployment"]["category"]
    );
}

#[test]
fn rebuilt_pickaxe_tsafa_claim_reproduces_verified_transaction() {
    let expected = fixture("tsafa-chipnet-claim-result.json");
    let claim = fixture("tsafa-chipnet-claim.json");
    let output = successful_json(run_cli_json(
        "claim",
        &[expected["deployment"].clone(), claim],
    ));
    assert_eq!(output, expected["result"]);
    assert_eq!(
        output["txid"],
        "89771ec35e517ee27478de8a4639c8847dffbcff5a44db0c5156d180e1274c09"
    );
}

#[test]
fn original_pickaxe_proof_cannot_claim_changed_covenant_lock_hash() {
    let expected = fixture("tsafa-chipnet-claim-result.json");
    let mut input = fixture("tsafa-chipnet-claim.json");
    input["proof"] = expected["original_proof"].clone();
    let output = run_cli_json("claim", &[expected["deployment"].clone(), input]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("covenant refuses this claim")
    );
    assert!(output.stdout.is_empty());
}

#[test]
fn leaf_prints_token_json_and_split_for_one_height() {
    let expected = fixture("tsafa-chipnet-claim-result.json");
    let input = fixture("tsafa-chipnet-claim.json");
    let job = json!({
        "vault_commitment": input["vault"]["commitment"],
        "height": expected["result"]["coinbase_height"],
        "payout_script": input["payout_script"],
        "split": input["split"],
    });
    let output = successful_json(run_cli_json("leaf", &[expected["deployment"].clone(), job]));
    assert_eq!(
        output["token"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec![
            "category",
            "lock_hash",
            "state_hash",
            "payout_hash",
            "target_bits",
            "split"
        ]
    );
    assert_eq!(
        output["token"]["target_bits"],
        expected["result"]["target_bits"]
    );
    assert_eq!(output["leaf_hash"], expected["leaf_hash"]);
    assert_eq!(output["token"]["split"], input["split"]);
}

#[test]
fn claim_verifies_transaction_with_selected_fee_rate() {
    let expected = fixture("tsafa-chipnet-claim-result.json");
    let mut input = fixture("tsafa-chipnet-claim.json");
    input["fee_rate"] = json!(2);
    let output = successful_json(run_cli_json(
        "claim",
        &[expected["deployment"].clone(), input],
    ));
    assert_eq!(
        output["fee_sats"],
        (2 * output["size_bound"].as_u64().unwrap()).to_string()
    );
    assert_eq!(output["reward"], "1250000000");
    let transaction =
        decode_transaction_bch(&hex::decode(output["hex"].as_str().unwrap()).unwrap()).unwrap();
    assert_eq!(transaction.outputs.len(), 4);
}

#[test]
fn claim_rejects_fee_rate_outside_covenant_bound() {
    let expected = fixture("tsafa-chipnet-claim-result.json");
    let mut input = fixture("tsafa-chipnet-claim.json");
    input["fee_rate"] = json!(4);
    let output = run_cli_json("claim", &[expected["deployment"].clone(), input]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("fee_rate")
    );
}

#[test]
fn genesis_prepares_one_unsigned_vault_per_thread_without_minting_baton() {
    let input = json!({
        "network": "chipnet", "threads": 4, "virtual_sats": "84000000000",
        "supply_per_thread": "525000000000000", "seed_sats": "2000000",
        "start_height": 329001, "target_bits": "1d00ffff", "ref_bits": "1a3106ee",
        "funding": { "txid": "11".repeat(32), "vout": 0, "value_sats": "8100000", "script": "51" },
        "change_script": "76a914cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd88ac", "fee_rate": 1,
    });
    let output = successful_json(run_cli_json("genesis", &[input]));
    let transaction =
        decode_transaction_bch(&hex::decode(output["unsigned_genesis"].as_str().unwrap()).unwrap())
            .unwrap();
    assert_eq!(transaction.outputs.len(), 5);
    assert_eq!(
        transaction.outputs[0].token.as_ref().unwrap().amount,
        525_000_000_000_000
    );
    assert_eq!(
        decode_state(
            &transaction.outputs[0]
                .token
                .as_ref()
                .unwrap()
                .nft
                .as_ref()
                .unwrap()
                .commitment
        )
        .unwrap()
        .thread,
        0
    );
    assert_eq!(
        decode_state(
            &transaction.outputs[1]
                .token
                .as_ref()
                .unwrap()
                .nft
                .as_ref()
                .unwrap()
                .commitment
        )
        .unwrap()
        .thread,
        1
    );
    assert_eq!(
        decode_state(
            &transaction.outputs[2]
                .token
                .as_ref()
                .unwrap()
                .nft
                .as_ref()
                .unwrap()
                .commitment
        )
        .unwrap()
        .thread,
        2
    );
    assert_eq!(
        decode_state(
            &transaction.outputs[3]
                .token
                .as_ref()
                .unwrap()
                .nft
                .as_ref()
                .unwrap()
                .commitment
        )
        .unwrap()
        .thread,
        3
    );
    assert!(transaction.outputs[4].token.is_none());
    assert_eq!(
        transaction.outputs[4].value_satoshis,
        100_000
            - output["genesis_fee_sats"]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap()
    );
    assert!(
        output["sign_with"]
            .as_str()
            .unwrap()
            .starts_with("signrawtransactionwithwallet ")
    );
}

#[test]
fn genesis_refuses_mainnet() {
    let output = run_cli_json("genesis", &[json!({ "network": "mainnet" })]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("chipnet only")
    );
}

#[test]
fn genesis_accepts_63_threads_and_creates_last_thread() {
    let input = json!({
        "network": "chipnet", "threads": 63, "virtual_sats": "84000000000",
        "supply_per_thread": "420000000000", "seed_sats": "10000",
        "start_height": 329001, "target_bits": "1d00ffff", "ref_bits": "1a3106ee",
        "funding": { "txid": "11".repeat(32), "vout": 0, "value_sats": "1000000", "script": "51" },
        "change_script": "51", "fee_rate": 1,
    });
    let output = successful_json(run_cli_json("genesis", &[input]));
    assert_eq!(output["deployment"]["threads"], 63);
    assert_eq!(output["vaults"].as_array().unwrap().len(), 63);
    assert_eq!(output["vaults"][62]["thread"], 62);
    assert_eq!(
        decode_state(&hex::decode(output["vaults"][62]["commitment"].as_str().unwrap()).unwrap())
            .unwrap()
            .thread,
        62
    );
    let transaction =
        decode_transaction_bch(&hex::decode(output["unsigned_genesis"].as_str().unwrap()).unwrap())
            .unwrap();
    assert_eq!(transaction.outputs.len(), 64);
}

#[test]
fn genesis_rejects_64_threads_before_reading_funding() {
    let output = run_cli_json(
        "genesis",
        &[json!({ "network": "chipnet", "threads": 64, "virtual_sats": "84000000000" })],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8(output.stderr).unwrap().contains("63"));
    assert!(output.stdout.is_empty());
}

#[test]
fn genesis_rejects_288_threads_before_reading_funding() {
    let output = run_cli_json(
        "genesis",
        &[json!({ "network": "chipnet", "threads": 288, "virtual_sats": "84000000000" })],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8(output.stderr).unwrap().contains("63"));
    assert!(output.stdout.is_empty());
}

#[test]
fn malformed_hex_fails_with_reason() {
    let deployment = fixture("tsafa-chipnet-fixture.json")["deployment"].clone();
    let output = run_cli_json("leaf", &[deployment, json!({ "vault_commitment": "gg" })]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("hexadecimal")
    );
}

#[test]
fn unknown_command_reports_usage_before_opening_files() {
    let output = Command::new(env!("CARGO_BIN_EXE_safa"))
        .args(["unknown", "missing-file"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8(output.stderr).unwrap().contains("usage"));
}

#[test]
fn json_reader_accepts_exactly_one_mib() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("input.json");
    fs::write(&path, format!("{{}}{}", " ".repeat(1024 * 1024 - 2))).unwrap();
    assert_eq!(read_json(path).unwrap(), json!({}));
}

#[test]
fn json_reader_rejects_one_byte_above_one_mib() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("input.json");
    fs::write(&path, format!("{{}}{}", " ".repeat(1024 * 1024 - 1))).unwrap();
    assert!(read_json(path).unwrap_err().contains("exceeds 1 MiB"));
}

#[test]
fn json_reader_refuses_directory() {
    let directory = tempdir().unwrap();
    assert!(read_json(directory.path()).is_err());
}

#[test]
fn params_rejects_unsafe_numeric_amount() {
    let input = json!({ "network": "chipnet", "threads": 4, "virtual_sats": 9007199254740992_u64 });
    assert!(
        run("params", &[input])
            .unwrap_err()
            .contains("whole number")
    );
}

#[test]
fn params_accepts_integral_json_float_like_javascript() {
    let input: Value = serde_json::from_str(
        "{\"network\":\"chipnet\",\"threads\":4.0,\"virtual_sats\":42000000000.0}",
    )
    .unwrap();
    assert_eq!(
        run("params", &[input]).unwrap()["virtual_sats"],
        "42000000000"
    );
}

#[test]
fn work_returns_the_v2_leaf_for_the_job() {
    let expected = fixture("tsafa-chipnet-claim-result.json");
    let input = fixture("tsafa-chipnet-claim.json");
    let job = json!({ "vault_commitment": input["vault"]["commitment"], "height": 329001,
        "payout_script": input["payout_script"], "split": input["split"] });
    let inputs = [expected["deployment"].clone(), job];
    assert_eq!(run("work", &inputs).unwrap(), run("leaf", &inputs).unwrap());
}

#[test]
fn unlock_returns_the_claim_arguments_and_redeem_script() {
    let expected = fixture("tsafa-chipnet-claim-result.json");
    let input = fixture("tsafa-chipnet-claim.json");
    let output = run("unlock", &[expected["deployment"].clone(), input]).unwrap();
    let unlocking = hex::decode(output["unlocking_bytecode"].as_str().unwrap()).unwrap();
    let covenant = create_covenant("chipnet", 4, 84_000_000_000).unwrap();
    assert!(unlocking.ends_with(&encode_data_push(&covenant.redeem_script)));
}

#[test]
fn verify_runs_the_bch_2026_vm() {
    let expected = fixture("tsafa-chipnet-claim-result.json");
    let input = fixture("tsafa-chipnet-claim.json");
    assert_eq!(
        run("verify", &[expected["deployment"].clone(), input]).unwrap(),
        json!({ "verified": true })
    );
}
