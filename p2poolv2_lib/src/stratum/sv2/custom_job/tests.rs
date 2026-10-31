use super::*;
use crate::shares::auxpow::{SplitSlot, TokenSplit};
use crate::stratum::work::block_template::LightJob;
use crate::test_utils::{
    bch_template_transaction, test_bch_custom_job, test_bch_custom_job_with_leaves,
    test_bch_declared_leaves, test_bch_prepared, test_bch_prepared_for_template,
    test_bch_prepared_with_cuts, test_bch_prepared_with_tree_payees, test_merge_token_set,
};
use binary_sv2::{B064K, Deserialize, Seq0255, U256};
use bitcoin::Address;
use bitcoin::Network;
use p2poolv2_config::DEFAULT_VERSION_MASK;
use p2poolv2_wallet::payout_address::PayoutAddress;

#[test]
fn a_full_declared_job_retains_the_tree_of_its_own_coinbase() {
    let prepared = test_bch_prepared_with_tree_payees(100);
    let token = prepared
        .mining_job_token([0x11; 32], prepared.template().curtime)
        .unwrap();
    let value = prepared.template().coinbasevalue;
    let (mut request_bytes, declared) = test_bch_custom_job(prepared, token, vec![], true, value);
    let request = SetCustomMiningJob::from_bytes(&mut request_bytes).unwrap();
    let payout = Address::p2pkh(
        bitcoin::PubkeyHash::from_byte_array([0x22; 20]),
        Network::Regtest,
    );
    let (job, block) = validate_custom_job(
        &request,
        declared,
        payout.into(),
        ShareOwner::CashP2pkh([0x22; 20]),
        DEFAULT_VERSION_MASK,
    )
    .unwrap();
    let tree = job.payout_tree.unwrap();
    assert!(tree.payees().len() > 1);
    let coinbase = &block.unwrap().txdata[0];
    assert!(coinbase.output.contains(&tree.root_output()));
    assert_eq!(
        tree.payees().iter().map(|payee| payee.amount).sum::<u64>(),
        tree.root_output().value.to_sat()
    );
}

#[test]
fn full_job_uses_declared_transactions_and_proportional_fee_revenue() {
    let prepared = test_bch_prepared();
    let token = prepared
        .mining_job_token([0x11; 32], prepared.template().curtime)
        .unwrap();
    let selected: bitcoin::Transaction =
        deserialize(&hex::decode(bch_template_transaction(1).data).unwrap()).unwrap();
    let root = test_bch_declared_leaves(&token).commitment.root;
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
        payout.into(),
        ShareOwner::CashP2pkh([0x22; 20]),
        DEFAULT_VERSION_MASK,
    )
    .unwrap();
    assert!(job.block_available);
    let block = block.unwrap();
    assert_eq!(block.txdata[1], selected);
    let outputs = &block.txdata[0].output;
    // The declared commitment is output 0, before the payouts.
    assert_eq!(
        MergeMiningCommitment::from_output(&outputs[0])
            .unwrap()
            .root,
        root
    );
    assert_eq!(outputs[1].value.to_sat(), 1_250_000_001);
    assert_eq!(outputs[2].value.to_sat(), 3_750_000_003);
    assert_eq!(
        job.share_commitment
            .unwrap()
            .bch_coinbase
            .unwrap()
            .merge_mining
            .root,
        root
    );
}

/// A job declared on the pool's light template has the declarator's
/// transactions, so the pool's light job id and merkle branch are dropped.
#[test]
fn full_job_on_a_light_template_drops_the_light_job() {
    let mut template = test_bch_prepared().template().as_ref().clone();
    template.light = Some(LightJob {
        job_id: "ec631dfd0a785808f1341c9c6596d8bd25c3e13a".to_string(),
        merkle_branch: vec![TxMerkleNode::all_zeros()],
        mining_block_size_limit: 16_000_000,
    });
    let prepared = test_bch_prepared_for_template(template);
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
        payout.into(),
        ShareOwner::CashP2pkh([0x22; 20]),
        DEFAULT_VERSION_MASK,
    )
    .unwrap();
    assert!(job.blocktemplate.light.is_none());
    assert_eq!(job.blocktemplate.transactions.len(), 1);
    assert_eq!(block.unwrap().txdata[1], selected);
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
        payout.into(),
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
            payout.into(),
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
    // Output 0 is the merge-mining commitment; the payouts follow it.
    outputs[1].value += Amount::from_sat(1);
    outputs[2].value -= Amount::from_sat(1);
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
            payout.into(),
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
            payout.into(),
            ShareOwner::CashP2pkh([0x22; 20]),
            DEFAULT_VERSION_MASK
        )
        .unwrap_err(),
        "declaration-mismatch"
    );
}

/// A P2PKH regtest address paying twenty `byte` bytes.
fn regtest_payout(byte: u8) -> PayoutAddress {
    Address::p2pkh(
        bitcoin::PubkeyHash::from_byte_array([byte; 20]),
        Network::Regtest,
    )
    .into()
}

/// Validate the Full-Template job on `prepared` whose declaration
/// discloses token leaves paying `leaf_payout` with `split`, installed for
/// the miner paid to P2PKH(22 x 20).
fn validate_declared_leaves(
    prepared: Arc<crate::stratum::work::prepared_notify::PreparedNotifyParams>,
    leaf_payout: &PayoutAddress,
    split: TokenSplit,
) -> Result<(JobDetails, Option<Block>), String> {
    let token = prepared
        .mining_job_token([1; 32], prepared.template().curtime)
        .unwrap();
    let value = prepared.template().coinbasevalue;
    let leaves = test_merge_token_set().miner_job(leaf_payout.script_pubkey().as_bytes(), split);
    let (mut request_bytes, declared) = test_bch_custom_job_with_leaves(
        prepared,
        token,
        vec![],
        true,
        value,
        leaves.commitment,
        leaves.disclosure(),
    );
    let request = SetCustomMiningJob::from_bytes(&mut request_bytes).unwrap();
    validate_custom_job(
        &request,
        declared,
        regtest_payout(0x22),
        ShareOwner::CashP2pkh([0x22; 20]),
        DEFAULT_VERSION_MASK,
    )
}

/// The node's 1% donation and 1% fee, each to the script the coinbase pays.
fn node_cuts_split(fee_address: &PayoutAddress) -> TokenSplit {
    let donation =
        crate::config::bitcoin_cash_donation_address(crate::config::BitcoinCashNetwork::Regtest);
    TokenSplit {
        donation: SplitSlot::new(100, donation.script_pubkey().as_bytes()).unwrap(),
        fee: SplitSlot::new(100, fee_address.script_pubkey().as_bytes()).unwrap(),
    }
}

/// On a node taking a 1% donation and a 1% fee, a declared job whose leaves
/// pay the miner with both cuts in their split is installed: the node's fee
/// reaches the tokens as it reaches BCH.
#[test]
fn declared_leaves_carrying_the_node_s_cuts_are_installed() {
    let fee_address = regtest_payout(0xfe);
    let prepared = test_bch_prepared_with_cuts(Some(100), Some(fee_address.clone()), Some(100));
    assert!(
        validate_declared_leaves(
            prepared,
            &regtest_payout(0x22),
            node_cuts_split(&fee_address)
        )
        .is_ok()
    );
}

/// A declared job whose leaves leave the node's fee out of their split is
/// refused.
#[test]
fn declared_leaves_without_the_node_s_fee_are_refused() {
    let fee_address = regtest_payout(0xfe);
    let prepared = test_bch_prepared_with_cuts(Some(100), Some(fee_address.clone()), Some(100));
    let without_fee = TokenSplit {
        fee: SplitSlot::EMPTY,
        ..node_cuts_split(&fee_address)
    };
    let reason =
        validate_declared_leaves(prepared, &regtest_payout(0x22), without_fee).unwrap_err();
    assert!(reason.contains("split"), "{reason}");
}

/// A declared job whose leaves pay someone other than its miner is refused.
#[test]
fn declared_leaves_paying_another_miner_are_refused() {
    let fee_address = regtest_payout(0xfe);
    let prepared = test_bch_prepared_with_cuts(Some(100), Some(fee_address.clone()), Some(100));
    let reason = validate_declared_leaves(
        prepared,
        &regtest_payout(0x33),
        node_cuts_split(&fee_address),
    )
    .unwrap_err();
    assert!(reason.contains("does not pay the miner"), "{reason}");
}

/// A Coinbase-only job declares no leaves, so its output 0 must be the
/// empty commitment.
#[test]
fn a_coinbase_only_job_cannot_carry_a_token_root() {
    let prepared = test_bch_prepared();
    let token = prepared
        .mining_job_token([1; 32], prepared.template().curtime)
        .unwrap();
    let root = test_bch_declared_leaves(&token).commitment;
    let (mut request_bytes, declared) = crate::test_utils::test_bch_custom_job_with_aux(
        prepared,
        token,
        vec![],
        false,
        5_000_000_000,
        root,
    );
    let request = SetCustomMiningJob::from_bytes(&mut request_bytes).unwrap();
    let reason = validate_custom_job(
        &request,
        declared,
        regtest_payout(0x22),
        ShareOwner::CashP2pkh([0x22; 20]),
        DEFAULT_VERSION_MASK,
    )
    .unwrap_err();
    assert!(reason.contains("Coinbase-only"), "{reason}");
}
