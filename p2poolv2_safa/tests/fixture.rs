// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Rebuild Pickaxe's supplied public claim against the corrected covenant.
mod test_utils;

use libauth::encode_transaction;
use p2poolv2_safa::{self as safa, Outpoint, Split, Vault};
use test_utils::{cli::fixture, *};

#[test]
fn pickaxe_original_claim_rebuild_matches_corrected_js_transaction() {
    let deployment = fixture("pickaxe-original-deployment.json");
    let original = fixture("pickaxe-original-claim.json");
    let corrected = fixture("tsafa-chipnet-claim.json");
    let expected = fixture("tsafa-chipnet-claim-result.json");
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
    assert_eq!(
        hex::encode(&leaf.leaf_hash),
        expected["leaf_hash"].as_str().unwrap()
    );
    assert_eq!(hex::encode(&root), expected["root"].as_str().unwrap());
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
    assert_eq!(hex::encode(&proof), corrected["proof"].as_str().unwrap());
    assert_ne!(proof, original_proof);
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
    accepts(&claim);
    let encoded = encode_transaction(&claim.transaction);
    assert_eq!(
        hex::encode(&encoded),
        expected["result"]["hex"].as_str().unwrap()
    );
    let mut transaction_id = safa::hash256(&encoded);
    transaction_id.reverse();
    assert_eq!(
        hex::encode(transaction_id),
        "89771ec35e517ee27478de8a4639c8847dffbcff5a44db0c5156d180e1274c09"
    );
    assert_eq!(claim.size, 2105);
    assert_eq!(claim.plan.size_bound, 2200);
}
