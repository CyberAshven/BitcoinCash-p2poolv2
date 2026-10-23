use super::*;
use crate::shares::merge_mining::MergeMiningCommitment;
use crate::test_utils::{test_bch_custom_job_with_aux, test_bch_prepared};
use bitcoin::consensus::deserialize;
use bitcoin::script::Builder;
use bitcoin::{Network, PubkeyHash};
use p2poolv2_wallet::share_owner::ShareOwner;

#[test]
fn full_preparation_matches_pplns_with_selected_fee_revenue() {
    let prepared = test_bch_prepared();
    let weights = HashMap::from([
        (
            Address::p2pkh(PubkeyHash::from_byte_array([1; 20]), Network::Regtest),
            1,
        ),
        (
            Address::p2pkh(PubkeyHash::from_byte_array([2; 20]), Network::Regtest),
            3,
        ),
    ]);
    let payout = Address::p2pkh(PubkeyHash::from_byte_array([0x22; 20]), Network::Regtest);
    let full = true;
    let value = 5_000_000_219;
    let token = prepared
        .mining_job_token([0x11; 32], prepared.template().curtime)
        .unwrap();
    let aux = MergeMiningCommitment {
        root: [0x77; 32],
        height: 1,
        salt: 9,
    };
    let commitment = ShareCommitment::from_mining_job_token(
        &token,
        payout.clone(),
        ShareOwner::CashP2pkh([0x22; 20]),
    )
    .unwrap();
    let descriptor = BchCoinbase {
        prefix: Builder::new()
            .push_int(i64::from(prepared.template().height))
            .push_slice(b"miner")
            .into_bytes(),
        merge_mining: aux,
    };
    let result = prepare_coinbase(
        &token,
        commitment,
        &weights,
        value,
        prepared.template().height,
        descriptor,
    )
    .unwrap();
    let (_, expected) =
        test_bch_custom_job_with_aux(prepared.clone(), token, vec![], full, value, aux);
    if let Some(declaration) = expected.declaration {
        assert_eq!(result.prefix, declaration.prefix);
        assert_eq!(result.suffix, declaration.suffix);
    }
    let mut raw = result.prefix;
    raw.extend(EXTRANONCE_SEPARATOR);
    raw.extend(result.suffix);
    let coinbase: bitcoin::Transaction = deserialize(&raw).unwrap();
    assert_eq!(
        coinbase
            .output
            .iter()
            .map(|out| out.value.to_sat())
            .sum::<u64>(),
        value
    );
    assert_eq!(
        MergeMiningCommitment::from_output(&coinbase.output[0]).unwrap(),
        aux
    );
}
#[test]
fn coinbase_only_preparation_preserves_pplns_and_aux_commitment() {
    let prepared = test_bch_prepared();
    let weights = HashMap::from([
        (
            Address::p2pkh(PubkeyHash::from_byte_array([1; 20]), Network::Regtest),
            1,
        ),
        (
            Address::p2pkh(PubkeyHash::from_byte_array([2; 20]), Network::Regtest),
            3,
        ),
    ]);
    let payout = Address::p2pkh(PubkeyHash::from_byte_array([0x22; 20]), Network::Regtest);
    let full = false;
    let value = 5_000_000_004;
    let token = prepared
        .mining_job_token([0x11; 32], prepared.template().curtime)
        .unwrap();
    let aux = MergeMiningCommitment {
        root: [0x77; 32],
        height: 1,
        salt: 9,
    };
    let commitment = ShareCommitment::from_mining_job_token(
        &token,
        payout.clone(),
        ShareOwner::CashP2pkh([0x22; 20]),
    )
    .unwrap();
    let descriptor = BchCoinbase {
        prefix: Builder::new()
            .push_int(i64::from(prepared.template().height))
            .push_slice(b"miner")
            .into_bytes(),
        merge_mining: aux,
    };
    let result = prepare_coinbase(
        &token,
        commitment,
        &weights,
        value,
        prepared.template().height,
        descriptor,
    )
    .unwrap();
    let (_, expected) =
        test_bch_custom_job_with_aux(prepared.clone(), token, vec![], full, value, aux);
    if let Some(declaration) = expected.declaration {
        assert_eq!(result.prefix, declaration.prefix);
        assert_eq!(result.suffix, declaration.suffix);
    }
    let mut raw = result.prefix;
    raw.extend(EXTRANONCE_SEPARATOR);
    raw.extend(result.suffix);
    let coinbase: bitcoin::Transaction = deserialize(&raw).unwrap();
    assert_eq!(
        coinbase
            .output
            .iter()
            .map(|out| out.value.to_sat())
            .sum::<u64>(),
        value
    );
    assert_eq!(
        MergeMiningCommitment::from_output(&coinbase.output[0]).unwrap(),
        aux
    );
}
