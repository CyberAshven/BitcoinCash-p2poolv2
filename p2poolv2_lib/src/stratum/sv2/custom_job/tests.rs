use super::*;
use crate::test_utils::{bch_template_transaction, test_bch_custom_job, test_bch_prepared};
use binary_sv2::{B064K, Deserialize, Seq0255, U256};
use bitcoin::Network;
use p2poolv2_config::DEFAULT_VERSION_MASK;

#[test]
fn full_job_uses_declared_transactions_and_proportional_fee_revenue() {
    let prepared = test_bch_prepared();
    let token = prepared
        .mining_job_token([0x11; 32], prepared.template().curtime)
        .unwrap();
    let selected: bitcoin::Transaction =
        deserialize(&hex::decode(bch_template_transaction(1).data).unwrap()).unwrap();
    let (mut request_bytes, declared) =
        test_bch_custom_job(prepared, token, vec![selected.clone()], true, 5_000_000_004);
    let request = SetCustomMiningJob::from_bytes(&mut request_bytes).unwrap();
    let payout = Address::p2pkh(
        bitcoin::PubkeyHash::from_byte_array([0x22; 20]),
        Network::Regtest,
    );
    let (job, block) = validate_custom_job(
        &request,
        declared,
        payout,
        ShareOwner::CashP2pkh([0x22; 20]),
        DEFAULT_VERSION_MASK,
    )
    .unwrap();
    assert!(job.block_available);
    let block = block.unwrap();
    assert_eq!(block.txdata[1], selected);
    let outputs = &block.txdata[0].output;
    assert_eq!(outputs[0].value.to_sat(), 1_250_000_001);
    assert_eq!(outputs[1].value.to_sat(), 3_750_000_003);
    assert_eq!(
        job.share_commitment
            .unwrap()
            .bch_coinbase
            .unwrap()
            .merge_mining
            .merkle_root,
        [0x77; 32]
    );
}

#[test]
fn private_job_preserves_hidden_merkle_path_without_pool_transactions() {
    let prepared = test_bch_prepared();
    let token = prepared
        .mining_job_token([0x11; 32], prepared.template().curtime)
        .unwrap();
    let selected = deserialize(&hex::decode(bch_template_transaction(7).data).unwrap()).unwrap();
    let (mut request_bytes, declared) =
        test_bch_custom_job(prepared, token, vec![selected], false, 5_000_000_000);
    let request = SetCustomMiningJob::from_bytes(&mut request_bytes).unwrap();
    let payout = Address::p2pkh(
        bitcoin::PubkeyHash::from_byte_array([0x22; 20]),
        Network::Regtest,
    );
    let (job, block) = validate_custom_job(
        &request,
        declared,
        payout,
        ShareOwner::CashP2pkh([0x22; 20]),
        DEFAULT_VERSION_MASK,
    )
    .unwrap();
    assert!(!job.block_available);
    assert!(block.is_none());
    assert!(job.blocktemplate.transactions.is_empty());
    assert_eq!(job.template_merkle_branches.len(), 1);
    assert_eq!(
        job.template_merkle_branches[0].as_byte_array(),
        request.merkle_path[0].as_array()
    );
}

#[test]
fn full_job_rejects_wrong_merkle_path() {
    let prepared = test_bch_prepared();
    let token = prepared
        .mining_job_token([1; 32], prepared.template().curtime)
        .unwrap();
    let (mut request_bytes, declared) =
        test_bch_custom_job(prepared, token, vec![], true, 5_000_000_000);
    let mut request = SetCustomMiningJob::from_bytes(&mut request_bytes).unwrap();
    request.merkle_path = Seq0255::new(vec![U256::new(&[9; 32]).unwrap()]).unwrap();
    let payout = Address::p2pkh(
        bitcoin::PubkeyHash::from_byte_array([0x22; 20]),
        Network::Regtest,
    );
    assert_eq!(
        validate_custom_job(
            &request,
            declared,
            payout,
            ShareOwner::CashP2pkh([0x22; 20]),
            DEFAULT_VERSION_MASK
        )
        .unwrap_err(),
        "invalid-merkle-path"
    );
}

#[test]
fn private_job_rejects_diverted_pplns_payment() {
    let prepared = test_bch_prepared();
    let token = prepared
        .mining_job_token([1; 32], prepared.template().curtime)
        .unwrap();
    let (mut request_bytes, declared) =
        test_bch_custom_job(prepared, token, vec![], false, 5_000_000_000);
    let mut request = SetCustomMiningJob::from_bytes(&mut request_bytes).unwrap();
    let mut outputs: Vec<TxOut> = deserialize(request.coinbase_tx_outputs.as_bytes()).unwrap();
    outputs[0].value += Amount::from_sat(1);
    outputs[1].value -= Amount::from_sat(1);
    let changed_outputs = serialize(&outputs);
    request.coinbase_tx_outputs = B064K::new(&changed_outputs).unwrap();
    let payout = Address::p2pkh(
        bitcoin::PubkeyHash::from_byte_array([0x22; 20]),
        Network::Regtest,
    );
    assert_eq!(
        validate_custom_job(
            &request,
            declared,
            payout,
            ShareOwner::CashP2pkh([0x22; 20]),
            DEFAULT_VERSION_MASK
        )
        .unwrap_err(),
        "invalid-pplns-outputs"
    );
}

#[test]
fn full_job_rejects_changed_declaration() {
    let prepared = test_bch_prepared();
    let token = prepared
        .mining_job_token([1; 32], prepared.template().curtime)
        .unwrap();
    let (mut request_bytes, mut declared) =
        test_bch_custom_job(prepared, token, vec![], true, 5_000_000_000);
    let request = SetCustomMiningJob::from_bytes(&mut request_bytes).unwrap();
    declared.declaration.as_mut().unwrap().suffix[0] ^= 1;
    let payout = Address::p2pkh(
        bitcoin::PubkeyHash::from_byte_array([0x22; 20]),
        Network::Regtest,
    );
    assert_eq!(
        validate_custom_job(
            &request,
            declared,
            payout,
            ShareOwner::CashP2pkh([0x22; 20]),
            DEFAULT_VERSION_MASK
        )
        .unwrap_err(),
        "declaration-mismatch"
    );
}
