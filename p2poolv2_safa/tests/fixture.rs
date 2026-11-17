// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Rebuild Pickaxe's supplied public claim against the covenant, and
//! regenerate the fixtures that record the covenant's bytes.
mod test_utils;

use std::fs;

use libauth::encode_transaction;
use p2poolv2_safa::cli::run;
use p2poolv2_safa::{self as safa, BuiltClaim, Outpoint, Split, Vault};
use serde_json::{Value, json};
use test_utils::{cli::fixture, *};

/// Pickaxe's supplied claim, rebuilt for this covenant's `lock_hash`: the
/// same deployment, outpoint, vault, split, branches and coinbase head and
/// tail, with the leaf, root, header merkle root and nonce that follow.
struct Rebuilt {
    proof: Vec<u8>,
    original_proof: Vec<u8>,
    leaf_hash: Vec<u8>,
    root: Vec<u8>,
    claim: BuiltClaim,
}

fn rebuild_pickaxe_claim() -> Rebuilt {
    let deployment = fixture("pickaxe-original-deployment.json");
    let original = fixture("pickaxe-original-claim.json");
    let covenant = safa::create_covenant(
        deployment["network"].as_str().unwrap(),
        deployment["threads"].as_u64().unwrap() as u32,
        deployment["virtual_sats"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap(),
    )
    .unwrap();
    let vault = Vault {
        category: hex::decode(deployment["category"].as_str().unwrap()).unwrap(),
        value: original["vault"]["value_sats"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap(),
        token_amount: original["vault"]["token_amount"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap(),
        commitment: hex::decode(original["vault"]["commitment"].as_str().unwrap()).unwrap(),
    };
    let split = Split {
        donation_bps: original["split"]["donation_bps"].as_u64().unwrap() as u16,
        donation_locking_bytecode: hex::decode(
            original["split"]["donation_script"].as_str().unwrap(),
        )
        .unwrap(),
        fee_bps: original["split"]["fee_bps"].as_u64().unwrap() as u16,
        fee_locking_bytecode: hex::decode(original["split"]["fee_script"].as_str().unwrap())
            .unwrap(),
    };
    let payout = hex::decode(original["payout_script"].as_str().unwrap()).unwrap();
    let original_proof = hex::decode(original["proof"].as_str().unwrap()).unwrap();
    let mut parts = safa::decode_proof(&original_proof).unwrap();
    let leaf = safa::claim_leaf(
        &covenant,
        &vault,
        &payout,
        Some(&split),
        safa::coinbase_height(&parts.head).unwrap(),
    )
    .unwrap();
    let root = safa::fold_aux(
        &leaf.leaf_hash,
        safa::slot(&leaf.leaf.category, parts.salt, parts.height),
        &parts.aux_branch,
    );
    let coinbase = [
        parts.head.as_slice(),
        safa::commitment_output(&root, parts.height, parts.salt).as_slice(),
        parts.tail.as_slice(),
    ]
    .concat();
    parts.header[36..68].copy_from_slice(&safa::coinbase_merkle_root(
        &coinbase,
        &parts.coinbase_branch,
    ));
    mine_existing_header(
        &mut parts.header,
        &safa::expand_token_target(leaf.target_bits).unwrap(),
    );
    let proof = safa::encode_proof(&parts).unwrap();
    let claim = safa::build_claim(
        &covenant,
        &Outpoint {
            txid: hex::decode(original["outpoint"]["txid"].as_str().unwrap()).unwrap(),
            vout: original["outpoint"]["vout"].as_u64().unwrap() as u32,
        },
        &vault,
        &payout,
        Some(&split),
        &proof,
        1,
        None,
    )
    .unwrap();
    Rebuilt {
        proof,
        original_proof,
        leaf_hash: leaf.leaf_hash,
        root,
        claim,
    }
}

#[test]
fn pickaxe_original_claim_rebuild_matches_the_recorded_transaction() {
    let corrected = fixture("tsafa-chipnet-claim.json");
    let expected = fixture("tsafa-chipnet-claim-result.json");
    let rebuilt = rebuild_pickaxe_claim();
    assert_eq!(
        hex::encode(&rebuilt.leaf_hash),
        expected["leaf_hash"].as_str().unwrap()
    );
    assert_eq!(
        hex::encode(&rebuilt.root),
        expected["root"].as_str().unwrap()
    );
    assert_eq!(
        hex::encode(&rebuilt.proof),
        corrected["proof"].as_str().unwrap()
    );
    assert_ne!(rebuilt.proof, rebuilt.original_proof);
    assert_eq!(
        hex::encode(&rebuilt.original_proof),
        expected["original_proof"].as_str().unwrap()
    );
    accepts(&rebuilt.claim);
    let encoded = encode_transaction(&rebuilt.claim.transaction);
    assert_eq!(
        hex::encode(&encoded),
        expected["result"]["hex"].as_str().unwrap()
    );
    let mut transaction_id = safa::hash256(&encoded);
    transaction_id.reverse();
    assert_eq!(
        hex::encode(transaction_id),
        expected["result"]["txid"].as_str().unwrap()
    );
    assert_eq!(
        rebuilt.claim.size as u64,
        expected["result"]["size"].as_u64().unwrap()
    );
    assert_eq!(
        rebuilt.claim.plan.size_bound as u64,
        expected["result"]["size_bound"].as_u64().unwrap()
    );
}

fn write_fixture(name: &str, value: &Value) {
    let path = format!("{}/test_data/{name}", env!("CARGO_MANIFEST_DIR"));
    fs::write(
        path,
        format!("{}\n", serde_json::to_string_pretty(value).unwrap()),
    )
    .unwrap();
}

/// Rewrite the fixtures that record the bytes of the covenant `safa.cash`
/// compiles to: the chipnet parameters, the rebuilt Pickaxe claim and its
/// result, and the output of every recorded command. Every one of them
/// changes with the redeem script, so run this after a change to the
/// covenant and review the difference:
///
/// ```sh
/// cargo test -p p2poolv2_safa --test fixture -- --ignored regenerate_recorded_fixtures
/// ```
///
/// The record of the deployment made on chipnet on 2026-10-29 and the live
/// wins mined for it are history and are not touched.
#[test]
#[ignore = "rewrites test_data; run by hand after a covenant change"]
fn regenerate_recorded_fixtures() {
    let note = "Regenerated by the Rust CLI (tests/fixture.rs) for the covenant safa.cash \
        compiles to; the retired Node tool recorded this case for an earlier covenant.";

    let mut parameters = fixture("tsafa-chipnet-fixture.json");
    parameters["description"] = json!(
        "Chipnet tSAFA fixture for cross-checks before funding: the proposed deployment with a \
        placeholder funding outpoint (txid 11 x 32, vout 0, which is also the category). params \
        is what cargo safa params prints for deployment; tests/cli.rs checks it is reproduced \
        byte for byte."
    );
    parameters["params"] = run("params", &[parameters["deployment"].clone()]).unwrap();
    write_fixture("tsafa-chipnet-fixture.json", &parameters);

    let rebuilt = rebuild_pickaxe_claim();
    let proof = json!(hex::encode(&rebuilt.proof));
    let mut claim = fixture("tsafa-chipnet-claim.json");
    let recorded_proof = claim["proof"].clone();
    claim["proof"] = proof.clone();
    write_fixture("tsafa-chipnet-claim.json", &claim);

    let mut result = fixture("tsafa-chipnet-claim-result.json");
    result["description"] = json!(
        "Pickaxe synthetic tSAFA claim rebuilt for the chipnet covenant. Same deployment, \
        outpoint, vault, split, proof branches and coinbase head/tail as the supplied claim; the \
        covenant's lock_hash changes the leaf, commitment root, header merkle root and nonce. \
        Verified by cargo safa claim; not broadcast."
    );
    result["leaf_hash"] = json!(hex::encode(&rebuilt.leaf_hash));
    result["root"] = json!(hex::encode(&rebuilt.root));
    result["result"] = run("claim", &[result["deployment"].clone(), claim]).unwrap();
    write_fixture("tsafa-chipnet-claim-result.json", &result);

    let mut cases = fixture("parity/cases.json");
    for case in cases.as_array_mut().unwrap() {
        for input in case["inputs"].as_array_mut().unwrap() {
            if input["proof"] == recorded_proof {
                input["proof"] = proof.clone();
            }
        }
        let output = run(
            case["command"].as_str().unwrap(),
            case["inputs"].as_array().unwrap(),
        )
        .unwrap_or_else(|error| panic!("{}: {error}", case["name"]));
        case["stdout"] = json!(format!(
            "{}\n",
            serde_json::to_string_pretty(&output).unwrap()
        ));
        // A case keeps what an earlier note says of it.
        case["note"] = match case["note"].as_str() {
            Some(earlier) if earlier.contains(note) => json!(earlier),
            Some(earlier) => json!(format!("{earlier} {note}")),
            None => json!(note),
        };
    }
    write_fixture("parity/cases.json", &cases);
}
