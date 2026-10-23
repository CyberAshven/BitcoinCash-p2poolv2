use super::*;
use crate::test_utils::{
    TestSv2Server, test_coinbase_transaction, test_sv2_authority, test_tdp_new_template,
    test_tdp_set_new_prev_hash, test_tdp_transaction_data,
};
use binary_sv2::to_bytes;
use bitcoin::block::{Header, Version};
use bitcoin::secp256k1::{Keypair, Secp256k1};
use bitcoin::{CompactTarget, ScriptBuf, TxMerkleNode, TxOut};
use common_messages_sv2::SetupConnectionSuccess;
use std::str::FromStr;
// Found blocks are not submitted under the `sim` feature, so the tests
// that follow a block to its provider are left out of that build.
#[cfg(not(feature = "sim"))]
use crate::config::DEFAULT_VERSION_MASK;
#[cfg(not(feature = "sim"))]
use crate::stratum::message_handlers::submit::submit_found_block;
#[cfg(not(feature = "sim"))]
use crate::stratum::sv2::mining::MiningChannels;
#[cfg(not(feature = "sim"))]
use crate::stratum::work::tracker::JobDetails;
#[cfg(not(feature = "sim"))]
use crate::test_utils::{test_bch_prepared_for_template, test_bch_stratum_context};
#[cfg(not(feature = "sim"))]
use binary_sv2::{B032, U256};
#[cfg(not(feature = "sim"))]
use mining_sv2::{
    MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL, MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED,
    NewExtendedMiningJob, OpenExtendedMiningChannel, OpenExtendedMiningChannelSuccess,
    SetNewPrevHash as MiningSetNewPrevHash, SubmitSharesExtended,
};
#[cfg(not(feature = "sim"))]
use p2poolv2_wallet::bch_share::{BchShareNetwork, encode_payout_address};
use template_distribution_sv2::RequestTransactionDataSuccessOwned;
use tokio::net::TcpListener;
use wiremock::matchers::{body_partial_json, method};
use wiremock::{Mock, MockServer, ResponseTemplate};

const REGTEST_TEMPLATE: &str =
    include_str!("../../../../../bitcoindrpc/test_data/bchn-regtest-template.json");
const CHIPNET_TEMPLATE: &str =
    include_str!("../../../../../bitcoindrpc/test_data/bchn-chipnet-template.json");

#[test]
fn a_future_template_waits_for_its_set_new_prev_hash() {
    let template: BlockTemplate = serde_json::from_str(REGTEST_TEMPLATE).unwrap();
    let mut session = ProviderSession::default();
    let announced = session
        .on_message(
            MESSAGE_TYPE_NEW_TEMPLATE,
            &mut to_bytes(test_tdp_new_template(5, true, &template)).unwrap(),
        )
        .unwrap();
    assert!(announced.is_empty());
    let activated = session
        .on_message(
            MESSAGE_TYPE_SET_NEW_PREV_HASH,
            &mut to_bytes(test_tdp_set_new_prev_hash(5, &template)).unwrap(),
        )
        .unwrap();
    assert!(matches!(
        activated.as_slice(),
        [SessionAction::Ready(ready)] if ready.announced.template_id == 5
    ));
}

#[test]
fn a_new_template_with_transactions_asks_for_them() {
    let template: BlockTemplate = serde_json::from_str(CHIPNET_TEMPLATE).unwrap();
    let mut session = ProviderSession::default();
    let actions = session
        .on_message(
            MESSAGE_TYPE_NEW_TEMPLATE,
            &mut to_bytes(test_tdp_new_template(9, true, &template)).unwrap(),
        )
        .unwrap();
    let request = encode_frame(
        RequestTransactionData { template_id: 9 },
        MESSAGE_TYPE_REQUEST_TRANSACTION_DATA,
    )
    .unwrap();
    assert_eq!(actions, vec![SessionAction::Send(request)]);
}

#[test]
fn a_template_is_ready_once_its_transactions_follow_its_set_new_prev_hash() {
    let template: BlockTemplate = serde_json::from_str(CHIPNET_TEMPLATE).unwrap();
    let mut session = ProviderSession::default();
    session
        .on_message(
            MESSAGE_TYPE_NEW_TEMPLATE,
            &mut to_bytes(test_tdp_new_template(9, true, &template)).unwrap(),
        )
        .unwrap();
    let activated = session
        .on_message(
            MESSAGE_TYPE_SET_NEW_PREV_HASH,
            &mut to_bytes(test_tdp_set_new_prev_hash(9, &template)).unwrap(),
        )
        .unwrap();
    assert!(activated.is_empty());
    let ready = session
        .on_message(
            MESSAGE_TYPE_REQUEST_TRANSACTION_DATA_SUCCESS,
            &mut to_bytes(test_tdp_transaction_data(9, &template)).unwrap(),
        )
        .unwrap();
    assert!(matches!(
        ready.as_slice(),
        [SessionAction::Ready(ready)]
            if ready.announced.transactions.as_ref().map(Vec::len) == Some(65)
    ));
}

#[test]
fn transactions_that_arrive_before_set_new_prev_hash_wait_for_it() {
    let template: BlockTemplate = serde_json::from_str(CHIPNET_TEMPLATE).unwrap();
    let mut session = ProviderSession::default();
    session
        .on_message(
            MESSAGE_TYPE_NEW_TEMPLATE,
            &mut to_bytes(test_tdp_new_template(9, true, &template)).unwrap(),
        )
        .unwrap();
    let early = session
        .on_message(
            MESSAGE_TYPE_REQUEST_TRANSACTION_DATA_SUCCESS,
            &mut to_bytes(test_tdp_transaction_data(9, &template)).unwrap(),
        )
        .unwrap();
    assert!(early.is_empty());
    let activated = session
        .on_message(
            MESSAGE_TYPE_SET_NEW_PREV_HASH,
            &mut to_bytes(test_tdp_set_new_prev_hash(9, &template)).unwrap(),
        )
        .unwrap();
    assert!(matches!(
        activated.as_slice(),
        [SessionAction::Ready(ready)] if ready.announced.template_id == 9
    ));
}

#[test]
fn a_current_template_is_ready_when_its_transactions_arrive() {
    let regtest: BlockTemplate = serde_json::from_str(REGTEST_TEMPLATE).unwrap();
    let chipnet: BlockTemplate = serde_json::from_str(CHIPNET_TEMPLATE).unwrap();
    let mut session = ProviderSession::default();
    session
        .on_message(
            MESSAGE_TYPE_NEW_TEMPLATE,
            &mut to_bytes(test_tdp_new_template(5, true, &regtest)).unwrap(),
        )
        .unwrap();
    session
        .on_message(
            MESSAGE_TYPE_SET_NEW_PREV_HASH,
            &mut to_bytes(test_tdp_set_new_prev_hash(5, &regtest)).unwrap(),
        )
        .unwrap();
    session
        .on_message(
            MESSAGE_TYPE_NEW_TEMPLATE,
            &mut to_bytes(test_tdp_new_template(6, false, &chipnet)).unwrap(),
        )
        .unwrap();
    let ready = session
        .on_message(
            MESSAGE_TYPE_REQUEST_TRANSACTION_DATA_SUCCESS,
            &mut to_bytes(test_tdp_transaction_data(6, &chipnet)).unwrap(),
        )
        .unwrap();
    assert!(matches!(
        ready.as_slice(),
        [SessionAction::Ready(ready)]
            if ready.announced.template_id == 6 && ready.tip.template_id == 5
    ));
}

#[test]
fn a_current_template_before_any_set_new_prev_hash_is_ignored() {
    let template: BlockTemplate = serde_json::from_str(REGTEST_TEMPLATE).unwrap();
    let mut session = ProviderSession::default();
    let actions = session
        .on_message(
            MESSAGE_TYPE_NEW_TEMPLATE,
            &mut to_bytes(test_tdp_new_template(5, false, &template)).unwrap(),
        )
        .unwrap();
    assert!(actions.is_empty());
    assert!(session.pending.is_empty());
}

#[test]
fn a_new_parent_drops_templates_for_the_old_one() {
    let regtest: BlockTemplate = serde_json::from_str(REGTEST_TEMPLATE).unwrap();
    let chipnet: BlockTemplate = serde_json::from_str(CHIPNET_TEMPLATE).unwrap();
    let mut session = ProviderSession::default();
    session
        .on_message(
            MESSAGE_TYPE_NEW_TEMPLATE,
            &mut to_bytes(test_tdp_new_template(5, true, &regtest)).unwrap(),
        )
        .unwrap();
    session
        .on_message(
            MESSAGE_TYPE_SET_NEW_PREV_HASH,
            &mut to_bytes(test_tdp_set_new_prev_hash(5, &regtest)).unwrap(),
        )
        .unwrap();
    session
        .on_message(
            MESSAGE_TYPE_NEW_TEMPLATE,
            &mut to_bytes(test_tdp_new_template(6, false, &chipnet)).unwrap(),
        )
        .unwrap();
    session
        .on_message(
            MESSAGE_TYPE_NEW_TEMPLATE,
            &mut to_bytes(test_tdp_new_template(7, true, &regtest)).unwrap(),
        )
        .unwrap();
    session
        .on_message(
            MESSAGE_TYPE_SET_NEW_PREV_HASH,
            &mut to_bytes(test_tdp_set_new_prev_hash(7, &regtest)).unwrap(),
        )
        .unwrap();
    let stale = session
        .on_message(
            MESSAGE_TYPE_REQUEST_TRANSACTION_DATA_SUCCESS,
            &mut to_bytes(test_tdp_transaction_data(6, &chipnet)).unwrap(),
        )
        .unwrap();
    assert!(stale.is_empty());
}

#[test]
fn a_published_template_drops_older_pending_templates() {
    let regtest: BlockTemplate = serde_json::from_str(REGTEST_TEMPLATE).unwrap();
    let chipnet: BlockTemplate = serde_json::from_str(CHIPNET_TEMPLATE).unwrap();
    let mut session = ProviderSession::default();
    session
        .on_message(
            MESSAGE_TYPE_NEW_TEMPLATE,
            &mut to_bytes(test_tdp_new_template(5, true, &regtest)).unwrap(),
        )
        .unwrap();
    session
        .on_message(
            MESSAGE_TYPE_SET_NEW_PREV_HASH,
            &mut to_bytes(test_tdp_set_new_prev_hash(5, &regtest)).unwrap(),
        )
        .unwrap();
    session
        .on_message(
            MESSAGE_TYPE_NEW_TEMPLATE,
            &mut to_bytes(test_tdp_new_template(6, false, &chipnet)).unwrap(),
        )
        .unwrap();
    let newer = session
        .on_message(
            MESSAGE_TYPE_NEW_TEMPLATE,
            &mut to_bytes(test_tdp_new_template(7, false, &regtest)).unwrap(),
        )
        .unwrap();
    assert!(matches!(
        newer.as_slice(),
        [SessionAction::Ready(ready)] if ready.announced.template_id == 7
    ));
    let older = session
        .on_message(
            MESSAGE_TYPE_REQUEST_TRANSACTION_DATA_SUCCESS,
            &mut to_bytes(test_tdp_transaction_data(6, &chipnet)).unwrap(),
        )
        .unwrap();
    assert!(older.is_empty());
}

#[test]
fn a_transaction_data_error_drops_the_template() {
    let template: BlockTemplate = serde_json::from_str(CHIPNET_TEMPLATE).unwrap();
    let mut session = ProviderSession::default();
    session
        .on_message(
            MESSAGE_TYPE_NEW_TEMPLATE,
            &mut to_bytes(test_tdp_new_template(9, true, &template)).unwrap(),
        )
        .unwrap();
    session
        .on_message(
            MESSAGE_TYPE_REQUEST_TRANSACTION_DATA_ERROR,
            &mut to_bytes(RequestTransactionDataError {
                template_id: 9,
                error_code: "template-id-not-found".try_into().unwrap(),
            })
            .unwrap(),
        )
        .unwrap();
    let activated = session
        .on_message(
            MESSAGE_TYPE_SET_NEW_PREV_HASH,
            &mut to_bytes(test_tdp_set_new_prev_hash(9, &template)).unwrap(),
        )
        .unwrap();
    assert!(activated.is_empty());
}

#[test]
fn a_malformed_transaction_drops_the_template() {
    let template: BlockTemplate = serde_json::from_str(CHIPNET_TEMPLATE).unwrap();
    let mut session = ProviderSession::default();
    session
        .on_message(
            MESSAGE_TYPE_NEW_TEMPLATE,
            &mut to_bytes(test_tdp_new_template(9, true, &template)).unwrap(),
        )
        .unwrap();
    session
        .on_message(
            MESSAGE_TYPE_SET_NEW_PREV_HASH,
            &mut to_bytes(test_tdp_set_new_prev_hash(9, &template)).unwrap(),
        )
        .unwrap();
    let malformed = session
        .on_message(
            MESSAGE_TYPE_REQUEST_TRANSACTION_DATA_SUCCESS,
            &mut to_bytes(RequestTransactionDataSuccessOwned {
                template_id: 9,
                excess_data: Vec::new().try_into().unwrap(),
                transaction_list: vec![vec![0xff, 0x00].try_into().unwrap()]
                    .try_into()
                    .unwrap(),
            })
            .unwrap(),
        )
        .unwrap();
    assert!(malformed.is_empty());
    let retried = session
        .on_message(
            MESSAGE_TYPE_REQUEST_TRANSACTION_DATA_SUCCESS,
            &mut to_bytes(test_tdp_transaction_data(9, &template)).unwrap(),
        )
        .unwrap();
    assert!(retried.is_empty());
}

#[test]
fn an_unknown_message_is_ignored() {
    let mut session = ProviderSession::default();
    assert!(session.on_message(0x99, &mut []).unwrap().is_empty());
}

#[test]
fn a_malformed_new_template_is_an_error() {
    let mut session = ProviderSession::default();
    assert!(
        session
            .on_message(MESSAGE_TYPE_NEW_TEMPLATE, &mut [1, 2, 3])
            .is_err()
    );
}

#[test]
fn a_chipnet_template_round_trips_through_template_distribution() {
    let fixture: BlockTemplate = serde_json::from_str(CHIPNET_TEMPLATE).unwrap();
    let mut session = ProviderSession::default();
    session
        .on_message(
            MESSAGE_TYPE_NEW_TEMPLATE,
            &mut to_bytes(test_tdp_new_template(1, true, &fixture)).unwrap(),
        )
        .unwrap();
    session
        .on_message(
            MESSAGE_TYPE_SET_NEW_PREV_HASH,
            &mut to_bytes(test_tdp_set_new_prev_hash(1, &fixture)).unwrap(),
        )
        .unwrap();
    let mut actions = session
        .on_message(
            MESSAGE_TYPE_REQUEST_TRANSACTION_DATA_SUCCESS,
            &mut to_bytes(test_tdp_transaction_data(1, &fixture)).unwrap(),
        )
        .unwrap();
    let Some(SessionAction::Ready(ready)) = actions.pop() else {
        panic!("template was not ready");
    };
    let (solutions, _queue) = mpsc::channel(1);
    let template = provided_block_template(
        ready,
        u64::from(fixture.height),
        block_size_floor(BitcoinCashNetwork::Chipnet),
        fixture.curtime,
        solutions,
    )
    .unwrap();
    assert_eq!(template.previousblockhash, fixture.previousblockhash);
    assert_eq!(template.version, fixture.version);
    assert_eq!(template.bits, fixture.bits);
    assert_eq!(template.target, fixture.target);
    assert_eq!(template.coinbasevalue, fixture.coinbasevalue);
    assert_eq!(template.height, fixture.height);
    assert_eq!(template.curtime, fixture.curtime);
    assert_eq!(template.sizelimit, fixture.sizelimit);
    assert_eq!(template.sigoplimit, fixture.sigoplimit);
    assert_eq!(
        template
            .transactions
            .iter()
            .map(|transaction| (&transaction.txid, &transaction.data))
            .collect::<Vec<_>>(),
        fixture
            .transactions
            .iter()
            .map(|transaction| (&transaction.txid, &transaction.data))
            .collect::<Vec<_>>()
    );
}

#[test]
fn a_regtest_template_is_checked_against_the_regtest_block_size() {
    let fixture: BlockTemplate = serde_json::from_str(REGTEST_TEMPLATE).unwrap();
    let mut session = ProviderSession::default();
    session
        .on_message(
            MESSAGE_TYPE_NEW_TEMPLATE,
            &mut to_bytes(test_tdp_new_template(5, true, &fixture)).unwrap(),
        )
        .unwrap();
    let mut actions = session
        .on_message(
            MESSAGE_TYPE_SET_NEW_PREV_HASH,
            &mut to_bytes(test_tdp_set_new_prev_hash(5, &fixture)).unwrap(),
        )
        .unwrap();
    let Some(SessionAction::Ready(ready)) = actions.pop() else {
        panic!("template was not ready");
    };
    let (solutions, _queue) = mpsc::channel(1);
    let template = provided_block_template(
        ready,
        u64::from(fixture.height),
        block_size_floor(BitcoinCashNetwork::Regtest),
        fixture.curtime,
        solutions,
    )
    .unwrap();
    assert_eq!(template.sizelimit, fixture.sizelimit);
    assert_eq!(template.sigoplimit, fixture.sigoplimit);
}

/// BCHN manages the block size: whatever limit it reports is used, past
/// the floor and past 32 bits. Only with no limit reported does the
/// network's floor stand in.
#[test]
fn the_size_limit_is_the_one_bchn_reports_and_the_floor_only_without_one() {
    assert_eq!(
        template_size_limit(Some(5_000_000_000), BitcoinCashNetwork::Mainnet),
        5_000_000_000
    );
    assert_eq!(
        template_size_limit(Some(3_000_000), BitcoinCashNetwork::Chipnet),
        3_000_000
    );
    assert_eq!(
        template_size_limit(None, BitcoinCashNetwork::Mainnet),
        32_000_000
    );
    assert_eq!(
        template_size_limit(None, BitcoinCashNetwork::Chipnet),
        2_000_000
    );
}

/// The chipnet fixture's template, ready on its parent.
fn ready_chipnet_template(fixture: &BlockTemplate) -> ReadyTemplate {
    let mut session = ProviderSession::default();
    session
        .on_message(
            MESSAGE_TYPE_NEW_TEMPLATE,
            &mut to_bytes(test_tdp_new_template(1, true, fixture)).unwrap(),
        )
        .unwrap();
    session
        .on_message(
            MESSAGE_TYPE_SET_NEW_PREV_HASH,
            &mut to_bytes(test_tdp_set_new_prev_hash(1, fixture)).unwrap(),
        )
        .unwrap();
    let mut actions = session
        .on_message(
            MESSAGE_TYPE_REQUEST_TRANSACTION_DATA_SUCCESS,
            &mut to_bytes(test_tdp_transaction_data(1, fixture)).unwrap(),
        )
        .unwrap();
    let Some(SessionAction::Ready(ready)) = actions.pop() else {
        panic!("template was not ready");
    };
    ready
}

/// A provided template takes the size limit it is given, and its SigChecks
/// limit from it. Its transactions must leave room for the largest coinbase
/// `SubmitSolution` carries, and one byte less refuses it.
#[test]
fn a_provided_template_is_checked_against_the_size_limit_it_is_given() {
    let fixture: BlockTemplate = serde_json::from_str(CHIPNET_TEMPLATE).unwrap();
    let transaction_bytes = fixture
        .decode_transactions()
        .unwrap()
        .iter()
        .map(|transaction| transaction.total_size() as u64)
        .sum::<u64>();
    let exact =
        transaction_bytes + u64::from(HEADER_AND_COUNT_SIZE) + u64::from(MAX_SUBMITTED_COINBASE);
    let (solutions, _queue) = mpsc::channel(1);

    let template = provided_block_template(
        ready_chipnet_template(&fixture),
        u64::from(fixture.height),
        exact,
        fixture.curtime,
        solutions.clone(),
    )
    .unwrap();
    assert_eq!(template.sizelimit, exact);
    assert_eq!(template.sigoplimit, exact / u64::from(BYTES_PER_SIGCHECK));

    let refused = provided_block_template(
        ready_chipnet_template(&fixture),
        u64::from(fixture.height),
        exact - 1,
        fixture.curtime,
        solutions,
    )
    .unwrap_err();
    assert!(refused.message.contains("no room for the coinbase"));
}

#[test]
fn a_provided_template_sends_its_solution_back_to_its_connection() {
    let fixture: BlockTemplate = serde_json::from_str(REGTEST_TEMPLATE).unwrap();
    let mut session = ProviderSession::default();
    session
        .on_message(
            MESSAGE_TYPE_NEW_TEMPLATE,
            &mut to_bytes(test_tdp_new_template(5, true, &fixture)).unwrap(),
        )
        .unwrap();
    let mut actions = session
        .on_message(
            MESSAGE_TYPE_SET_NEW_PREV_HASH,
            &mut to_bytes(test_tdp_set_new_prev_hash(5, &fixture)).unwrap(),
        )
        .unwrap();
    let Some(SessionAction::Ready(ready)) = actions.pop() else {
        panic!("template was not ready");
    };
    let (solutions, mut queue) = mpsc::channel(1);
    let template = provided_block_template(
        ready,
        u64::from(fixture.height),
        block_size_floor(BitcoinCashNetwork::Regtest),
        fixture.curtime,
        solutions,
    )
    .unwrap();
    let block = Block {
        header: Header {
            version: Version::from_consensus(0x2000_0000),
            prev_blockhash: BlockHash::all_zeros(),
            merkle_root: TxMerkleNode::all_zeros(),
            time: fixture.curtime,
            bits: CompactTarget::from_consensus(0x207f_ffff),
            nonce: 7,
        },
        txdata: vec![test_coinbase_transaction(1)],
    };
    template.provided.unwrap().submit(block.clone()).unwrap();
    let solution = queue.try_recv().unwrap();
    assert_eq!(solution.template_id, 5);
    assert_eq!(solution.block, block);
}

#[test]
fn a_bridge_form_coinbase_prefix_is_accepted() {
    let fixture: BlockTemplate = serde_json::from_str(REGTEST_TEMPLATE).unwrap();
    let mut announced = test_tdp_new_template(5, true, &fixture);
    // bchn-sv2-bridge sends the pushed height bytes without the push
    // opcode; height 102 is pushed as 01 66.
    announced.coinbase_prefix = vec![0x66].try_into().unwrap();
    let mut session = ProviderSession::default();
    session
        .on_message(MESSAGE_TYPE_NEW_TEMPLATE, &mut to_bytes(announced).unwrap())
        .unwrap();
    let mut actions = session
        .on_message(
            MESSAGE_TYPE_SET_NEW_PREV_HASH,
            &mut to_bytes(test_tdp_set_new_prev_hash(5, &fixture)).unwrap(),
        )
        .unwrap();
    let Some(SessionAction::Ready(ready)) = actions.pop() else {
        panic!("template was not ready");
    };
    let (solutions, _queue) = mpsc::channel(1);
    assert!(
        provided_block_template(
            ready,
            u64::from(fixture.height),
            block_size_floor(BitcoinCashNetwork::Regtest),
            fixture.curtime,
            solutions,
        )
        .is_ok()
    );
}

#[test]
fn a_coinbase_prefix_for_another_height_is_refused() {
    let fixture: BlockTemplate = serde_json::from_str(REGTEST_TEMPLATE).unwrap();
    let mut announced = test_tdp_new_template(5, true, &fixture);
    announced.coinbase_prefix = vec![0x01, 0x67].try_into().unwrap();
    let mut session = ProviderSession::default();
    session
        .on_message(MESSAGE_TYPE_NEW_TEMPLATE, &mut to_bytes(announced).unwrap())
        .unwrap();
    let mut actions = session
        .on_message(
            MESSAGE_TYPE_SET_NEW_PREV_HASH,
            &mut to_bytes(test_tdp_set_new_prev_hash(5, &fixture)).unwrap(),
        )
        .unwrap();
    let Some(SessionAction::Ready(ready)) = actions.pop() else {
        panic!("template was not ready");
    };
    let (solutions, _queue) = mpsc::channel(1);
    let refused = provided_block_template(
        ready,
        u64::from(fixture.height),
        block_size_floor(BitcoinCashNetwork::Regtest),
        fixture.curtime,
        solutions,
    )
    .unwrap_err();
    assert!(
        refused.message.contains("height push"),
        "{}",
        refused.message
    );
}

#[test]
fn a_coinbase_prefix_with_more_than_the_height_push_is_refused() {
    let fixture: BlockTemplate = serde_json::from_str(REGTEST_TEMPLATE).unwrap();
    let mut announced = test_tdp_new_template(5, true, &fixture);
    announced.coinbase_prefix = vec![0x01, 0x66, 0x00].try_into().unwrap();
    let mut session = ProviderSession::default();
    session
        .on_message(MESSAGE_TYPE_NEW_TEMPLATE, &mut to_bytes(announced).unwrap())
        .unwrap();
    let mut actions = session
        .on_message(
            MESSAGE_TYPE_SET_NEW_PREV_HASH,
            &mut to_bytes(test_tdp_set_new_prev_hash(5, &fixture)).unwrap(),
        )
        .unwrap();
    let Some(SessionAction::Ready(ready)) = actions.pop() else {
        panic!("template was not ready");
    };
    let (solutions, _queue) = mpsc::channel(1);
    assert!(
        provided_block_template(
            ready,
            u64::from(fixture.height),
            block_size_floor(BitcoinCashNetwork::Regtest),
            fixture.curtime,
            solutions,
        )
        .is_err()
    );
}

#[test]
fn required_coinbase_outputs_are_refused() {
    let fixture: BlockTemplate = serde_json::from_str(REGTEST_TEMPLATE).unwrap();
    let mut announced = test_tdp_new_template(5, true, &fixture);
    // A Bitcoin provider's segwit commitment output.
    announced.coinbase_tx_outputs_count = 1;
    announced.coinbase_tx_outputs = serialize(&TxOut {
        value: bitcoin::Amount::ZERO,
        script_pubkey: ScriptBuf::from_bytes(vec![0x6a, 0x24, 0xaa, 0x21, 0xa9, 0xed]),
    })
    .try_into()
    .unwrap();
    let mut session = ProviderSession::default();
    session
        .on_message(MESSAGE_TYPE_NEW_TEMPLATE, &mut to_bytes(announced).unwrap())
        .unwrap();
    let mut actions = session
        .on_message(
            MESSAGE_TYPE_SET_NEW_PREV_HASH,
            &mut to_bytes(test_tdp_set_new_prev_hash(5, &fixture)).unwrap(),
        )
        .unwrap();
    let Some(SessionAction::Ready(ready)) = actions.pop() else {
        panic!("template was not ready");
    };
    let (solutions, _queue) = mpsc::channel(1);
    let refused = provided_block_template(
        ready,
        u64::from(fixture.height),
        block_size_floor(BitcoinCashNetwork::Regtest),
        fixture.curtime,
        solutions,
    )
    .unwrap_err();
    assert!(
        refused.message.contains("coinbase outputs"),
        "{}",
        refused.message
    );
}

#[test]
fn a_merkle_path_that_does_not_match_the_transactions_is_refused() {
    let fixture: BlockTemplate = serde_json::from_str(CHIPNET_TEMPLATE).unwrap();
    let mut session = ProviderSession::default();
    session
        .on_message(
            MESSAGE_TYPE_NEW_TEMPLATE,
            &mut to_bytes(test_tdp_new_template(1, true, &fixture)).unwrap(),
        )
        .unwrap();
    session
        .on_message(
            MESSAGE_TYPE_SET_NEW_PREV_HASH,
            &mut to_bytes(test_tdp_set_new_prev_hash(1, &fixture)).unwrap(),
        )
        .unwrap();
    let mut actions = session
        .on_message(
            MESSAGE_TYPE_REQUEST_TRANSACTION_DATA_SUCCESS,
            &mut to_bytes(test_tdp_transaction_data(1, &fixture)).unwrap(),
        )
        .unwrap();
    let Some(SessionAction::Ready(mut ready)) = actions.pop() else {
        panic!("template was not ready");
    };
    ready.announced.merkle_path[0] = [0u8; 32];
    let (solutions, _queue) = mpsc::channel(1);
    let refused = provided_block_template(
        ready,
        u64::from(fixture.height),
        block_size_floor(BitcoinCashNetwork::Chipnet),
        fixture.curtime,
        solutions,
    )
    .unwrap_err();
    assert!(
        refused.message.contains("merkle path"),
        "{}",
        refused.message
    );
}

#[test]
fn transactions_out_of_ctor_order_are_refused() {
    let fixture: BlockTemplate = serde_json::from_str(CHIPNET_TEMPLATE).unwrap();
    let mut session = ProviderSession::default();
    session
        .on_message(
            MESSAGE_TYPE_NEW_TEMPLATE,
            &mut to_bytes(test_tdp_new_template(1, true, &fixture)).unwrap(),
        )
        .unwrap();
    session
        .on_message(
            MESSAGE_TYPE_SET_NEW_PREV_HASH,
            &mut to_bytes(test_tdp_set_new_prev_hash(1, &fixture)).unwrap(),
        )
        .unwrap();
    let mut actions = session
        .on_message(
            MESSAGE_TYPE_REQUEST_TRANSACTION_DATA_SUCCESS,
            &mut to_bytes(test_tdp_transaction_data(1, &fixture)).unwrap(),
        )
        .unwrap();
    let Some(SessionAction::Ready(mut ready)) = actions.pop() else {
        panic!("template was not ready");
    };
    ready.announced.transactions.as_mut().unwrap().swap(0, 1);
    let (solutions, _queue) = mpsc::channel(1);
    let refused = provided_block_template(
        ready,
        u64::from(fixture.height),
        block_size_floor(BitcoinCashNetwork::Chipnet),
        fixture.curtime,
        solutions,
    )
    .unwrap_err();
    assert!(refused.message.contains("CTOR"), "{}", refused.message);
}

#[test]
fn a_target_that_disagrees_with_the_bits_is_refused() {
    let fixture: BlockTemplate = serde_json::from_str(REGTEST_TEMPLATE).unwrap();
    let mut session = ProviderSession::default();
    session
        .on_message(
            MESSAGE_TYPE_NEW_TEMPLATE,
            &mut to_bytes(test_tdp_new_template(5, true, &fixture)).unwrap(),
        )
        .unwrap();
    let mut actions = session
        .on_message(
            MESSAGE_TYPE_SET_NEW_PREV_HASH,
            &mut to_bytes(test_tdp_set_new_prev_hash(5, &fixture)).unwrap(),
        )
        .unwrap();
    let Some(SessionAction::Ready(mut ready)) = actions.pop() else {
        panic!("template was not ready");
    };
    ready.tip.target = [0x01; 32];
    let (solutions, _queue) = mpsc::channel(1);
    let refused = provided_block_template(
        ready,
        u64::from(fixture.height),
        block_size_floor(BitcoinCashNetwork::Regtest),
        fixture.curtime,
        solutions,
    )
    .unwrap_err();
    assert!(refused.message.contains("target"), "{}", refused.message);
}

#[test]
fn a_template_after_its_set_new_prev_hash_takes_the_current_time() {
    let fixture: BlockTemplate = serde_json::from_str(REGTEST_TEMPLATE).unwrap();
    let mut session = ProviderSession::default();
    session
        .on_message(
            MESSAGE_TYPE_NEW_TEMPLATE,
            &mut to_bytes(test_tdp_new_template(5, true, &fixture)).unwrap(),
        )
        .unwrap();
    let mut actions = session
        .on_message(
            MESSAGE_TYPE_SET_NEW_PREV_HASH,
            &mut to_bytes(test_tdp_set_new_prev_hash(5, &fixture)).unwrap(),
        )
        .unwrap();
    let Some(SessionAction::Ready(ready)) = actions.pop() else {
        panic!("template was not ready");
    };
    let (solutions, _queue) = mpsc::channel(1);
    let template = provided_block_template(
        ready,
        u64::from(fixture.height),
        block_size_floor(BitcoinCashNetwork::Regtest),
        fixture.curtime + 30,
        solutions,
    )
    .unwrap();
    assert_eq!(template.curtime, fixture.curtime + 30);
    assert_eq!(template.mintime, fixture.curtime);
}

#[test]
fn coinbase_output_constraints_reserve_the_largest_coinbase_submit_solution_carries() {
    let frame = coinbase_output_constraints_frame().unwrap();
    let mut messages = decode_plain_frames(&mut Decoder::new(), &mut Vec::new(), &frame).unwrap();
    let (message_type, mut payload) = messages.remove(0);
    assert_eq!(message_type, MESSAGE_TYPE_COINBASE_OUTPUT_CONSTRAINTS);
    let constraints = CoinbaseOutputConstraints::from_bytes(&mut payload).unwrap();
    assert_eq!(constraints.coinbase_output_max_additional_size, 65_382);
    assert_eq!(constraints.coinbase_output_max_additional_sigops, 1_923);
    assert_eq!(
        COINBASE_WITHOUT_OUTPUTS + OUTPUT_COUNT_SIZE + COINBASE_OUTPUT_RESERVE,
        u32::from(u16::MAX)
    );
}

#[test]
fn setup_connection_asks_for_template_distribution_version_two() {
    let provider = TemplateProvider {
        address: "127.0.0.1:8442".to_string(),
        authority_public_key: None,
        network: BitcoinCashNetwork::Regtest,
    };
    let frame = setup_connection_frame(&provider).unwrap();
    let mut messages = decode_plain_frames(&mut Decoder::new(), &mut Vec::new(), &frame).unwrap();
    let (message_type, mut payload) = messages.remove(0);
    assert_eq!(message_type, MESSAGE_TYPE_SETUP_CONNECTION);
    let setup = SetupConnection::from_bytes(&mut payload).unwrap();
    assert_eq!(setup.protocol, Protocol::TemplateDistributionProtocol);
    assert_eq!(
        (setup.min_version, setup.max_version, setup.flags),
        (2, 2, 0)
    );
    assert_eq!(setup.endpoint_host.as_utf8_or_hex(), "127.0.0.1");
    assert_eq!(setup.endpoint_port, 8442);
}

#[test]
fn submit_solution_carries_the_header_fields_and_the_coinbase() {
    let block = Block {
        header: Header {
            version: Version::from_consensus(0x2000_0004),
            prev_blockhash: BlockHash::all_zeros(),
            merkle_root: TxMerkleNode::all_zeros(),
            time: 1_791_754_400,
            bits: CompactTarget::from_consensus(0x207f_ffff),
            nonce: 99,
        },
        txdata: vec![test_coinbase_transaction(1)],
    };
    let frame = submit_solution_frame(&Solution {
        template_id: 77,
        height: 102,
        block: block.clone(),
    })
    .unwrap();
    let mut messages = decode_plain_frames(&mut Decoder::new(), &mut Vec::new(), &frame).unwrap();
    let (message_type, mut payload) = messages.remove(0);
    assert_eq!(message_type, MESSAGE_TYPE_SUBMIT_SOLUTION);
    let solution = SubmitSolution::from_bytes(&mut payload).unwrap();
    assert_eq!(solution.template_id, 77);
    assert_eq!(solution.version, 0x2000_0004);
    assert_eq!(solution.header_timestamp, 1_791_754_400);
    assert_eq!(solution.header_nonce, 99);
    assert_eq!(
        solution.coinbase_tx.as_bytes(),
        serialize(&block.txdata[0]).as_slice()
    );
}

#[test]
fn a_coinbase_too_large_for_submit_solution_gets_no_frame() {
    let mut coinbase = test_coinbase_transaction(1);
    coinbase.output.push(TxOut {
        value: bitcoin::Amount::ZERO,
        script_pubkey: ScriptBuf::from_bytes(vec![0x6a; 70_000]),
    });
    let block = Block {
        header: Header {
            version: Version::from_consensus(0x2000_0000),
            prev_blockhash: BlockHash::all_zeros(),
            merkle_root: TxMerkleNode::all_zeros(),
            time: 1,
            bits: CompactTarget::from_consensus(0x207f_ffff),
            nonce: 0,
        },
        txdata: vec![coinbase],
    };
    assert!(
        submit_solution_frame(&Solution {
            template_id: 1,
            height: 102,
            block,
        })
        .is_none()
    );
}

#[tokio::test]
async fn templates_flow_from_an_unencrypted_provider_and_solutions_flow_back() {
    let fixture: BlockTemplate = serde_json::from_str(CHIPNET_TEMPLATE).unwrap();
    let rpc = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_partial_json(serde_json::json!({
            "method": "getblockheader",
            "params": [fixture.previousblockhash, true],
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": {"height": fixture.height - 1},
            "error": null,
            "id": 0,
        })))
        .expect(1)
        .mount(&rpc)
        .await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = TemplateProvider {
        address: listener.local_addr().unwrap().to_string(),
        authority_public_key: None,
        network: BitcoinCashNetwork::Chipnet,
    };
    let (notify_tx, mut notify_rx) = mpsc::channel(4);
    start_template_provider(
        provider,
        BitcoinRpcConfig {
            url: rpc.uri(),
            username: "user".to_string(),
            password: "pass".to_string(),
        },
        notify_tx,
    )
    .await
    .unwrap();
    let mut server = TestSv2Server::accept(&listener, None).await;
    let mut setup_payload = server.receive(MESSAGE_TYPE_SETUP_CONNECTION).await;
    let setup = SetupConnection::from_bytes(&mut setup_payload).unwrap();
    assert_eq!(setup.protocol, Protocol::TemplateDistributionProtocol);
    server
        .send_message(
            MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS,
            SetupConnectionSuccess {
                used_version: 2,
                flags: 0,
            },
        )
        .await;
    let mut constraints_payload = server
        .receive(MESSAGE_TYPE_COINBASE_OUTPUT_CONSTRAINTS)
        .await;
    let constraints = CoinbaseOutputConstraints::from_bytes(&mut constraints_payload).unwrap();
    assert_eq!(
        constraints.coinbase_output_max_additional_size,
        COINBASE_OUTPUT_RESERVE
    );
    server
        .send_message(
            MESSAGE_TYPE_NEW_TEMPLATE,
            test_tdp_new_template(41, true, &fixture),
        )
        .await;
    server
        .send_message(
            MESSAGE_TYPE_SET_NEW_PREV_HASH,
            test_tdp_set_new_prev_hash(41, &fixture),
        )
        .await;
    let mut request_payload = server.receive(MESSAGE_TYPE_REQUEST_TRANSACTION_DATA).await;
    let request = RequestTransactionData::from_bytes(&mut request_payload).unwrap();
    assert_eq!(request.template_id, 41);
    server
        .send_message(
            MESSAGE_TYPE_REQUEST_TRANSACTION_DATA_SUCCESS,
            test_tdp_transaction_data(41, &fixture),
        )
        .await;
    let Some(NotifyCmd::SendToAll { template }) = timeout(Duration::from_secs(5), notify_rx.recv())
        .await
        .unwrap()
    else {
        panic!("expected a template for the notifier");
    };
    assert_eq!(template.height, fixture.height);
    assert_eq!(template.previousblockhash, fixture.previousblockhash);
    assert_eq!(template.transactions.len(), fixture.transactions.len());
    // The node reported no ABLA state, so chipnet's floor stands in.
    assert_eq!(template.sizelimit, 2_000_000);
    let block = Block {
        header: Header {
            version: Version::from_consensus(fixture.version),
            prev_blockhash: BlockHash::from_str(&fixture.previousblockhash).unwrap(),
            merkle_root: TxMerkleNode::all_zeros(),
            time: fixture.curtime,
            bits: CompactTarget::from_unprefixed_hex(&fixture.bits).unwrap(),
            nonce: 1234,
        },
        txdata: vec![test_coinbase_transaction(1)],
    };
    template
        .provided
        .as_ref()
        .unwrap()
        .submit(block.clone())
        .unwrap();
    let mut solution_payload = server.receive(MESSAGE_TYPE_SUBMIT_SOLUTION).await;
    let solution = SubmitSolution::from_bytes(&mut solution_payload).unwrap();
    assert_eq!(solution.template_id, 41);
    assert_eq!(solution.header_nonce, 1234);
    assert_eq!(
        solution.coinbase_tx.as_bytes(),
        serialize(&block.txdata[0]).as_slice()
    );
}

#[tokio::test]
async fn a_noise_provider_signed_by_the_pinned_key_sends_templates() {
    let fixture: BlockTemplate = serde_json::from_str(REGTEST_TEMPLATE).unwrap();
    let rpc = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_partial_json(
            serde_json::json!({"method": "getblockheader"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": {"height": fixture.height - 1},
            "error": null,
            "id": 0,
        })))
        .mount(&rpc)
        .await;
    let authority = test_sv2_authority();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = TemplateProvider {
        address: listener.local_addr().unwrap().to_string(),
        authority_public_key: Some(authority.public_key),
        network: BitcoinCashNetwork::Regtest,
    };
    let (notify_tx, mut notify_rx) = mpsc::channel(4);
    start_template_provider(
        provider,
        BitcoinRpcConfig {
            url: rpc.uri(),
            username: "user".to_string(),
            password: "pass".to_string(),
        },
        notify_tx,
    )
    .await
    .unwrap();
    let mut server = TestSv2Server::accept(&listener, Some(&authority)).await;
    server.receive(MESSAGE_TYPE_SETUP_CONNECTION).await;
    server
        .send_message(
            MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS,
            SetupConnectionSuccess {
                used_version: 2,
                flags: 0,
            },
        )
        .await;
    server
        .receive(MESSAGE_TYPE_COINBASE_OUTPUT_CONSTRAINTS)
        .await;
    server
        .send_message(
            MESSAGE_TYPE_NEW_TEMPLATE,
            test_tdp_new_template(3, true, &fixture),
        )
        .await;
    server
        .send_message(
            MESSAGE_TYPE_SET_NEW_PREV_HASH,
            test_tdp_set_new_prev_hash(3, &fixture),
        )
        .await;
    let Some(NotifyCmd::SendToAll { template }) = timeout(Duration::from_secs(5), notify_rx.recv())
        .await
        .unwrap()
    else {
        panic!("expected a template for the notifier");
    };
    assert_eq!(template.height, fixture.height);
}

#[tokio::test]
async fn a_provider_signed_by_another_authority_is_refused() {
    let authority = test_sv2_authority();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    // The x-only key of secret 0x01..01: a valid key, but not the
    // provider's authority.
    let other_key = Keypair::from_seckey_slice(&Secp256k1::new(), &[1u8; 32])
        .unwrap()
        .x_only_public_key()
        .0
        .serialize();
    let provider = TemplateProvider {
        address: listener.local_addr().unwrap().to_string(),
        authority_public_key: Some(other_key),
        network: BitcoinCashNetwork::Regtest,
    };
    let (_server, opened) = tokio::join!(
        TestSv2Server::accept(&listener, Some(&authority)),
        open_connection(&provider)
    );
    let Err(refused) = opened else {
        panic!("a provider with another authority was accepted");
    };
    assert!(refused.contains("pinned authority"), "{refused}");
}

#[tokio::test]
async fn a_refused_setup_connection_ends_the_connection() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = TemplateProvider {
        address: listener.local_addr().unwrap().to_string(),
        authority_public_key: None,
        network: BitcoinCashNetwork::Regtest,
    };
    let refuse = async {
        let mut server = TestSv2Server::accept(&listener, None).await;
        server.receive(MESSAGE_TYPE_SETUP_CONNECTION).await;
        server
            .send_message(
                MESSAGE_TYPE_SETUP_CONNECTION_ERROR,
                SetupConnectionError {
                    flags: 0,
                    error_code: "unsupported-protocol".try_into().unwrap(),
                },
            )
            .await;
        server
    };
    let (_server, opened) = tokio::join!(refuse, open_connection(&provider));
    let Err(refused) = opened else {
        panic!("a refused setup opened the connection");
    };
    assert!(refused.contains("unsupported-protocol"), "{refused}");
}

#[cfg(not(feature = "sim"))]
#[tokio::test]
async fn a_block_on_a_provided_template_goes_to_its_provider_and_not_over_rpc() {
    let rpc = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_partial_json(
            serde_json::json!({"method": "submitblock"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": null,
            "error": null,
            "id": 0,
        })))
        .expect(0)
        .mount(&rpc)
        .await;
    let (ctx, _emissions, _dirs) = test_bch_stratum_context(&rpc.uri()).await;
    let (solutions, mut queue) = mpsc::channel(1);
    let mut template: BlockTemplate = serde_json::from_str(REGTEST_TEMPLATE).unwrap();
    template.provided = Some(ProvidedTemplate {
        template_id: 12,
        height: 102,
        solutions,
    });
    let job = JobDetails {
        block_available: true,
        blocktemplate: Arc::new(template),
        coinbase1: String::new(),
        coinbase2: String::new(),
        generation_timestamp: 0,
        share_commitment: None,
        coinbase_nsecs: 0,
        template_merkle_branches: Vec::new(),
        share_transactions: Arc::from([]),
    };
    let header = Header {
        version: Version::from_consensus(0x2000_0000),
        prev_blockhash: BlockHash::all_zeros(),
        merkle_root: TxMerkleNode::all_zeros(),
        time: 1,
        bits: CompactTarget::from_consensus(0x207f_ffff),
        nonce: 5,
    };
    submit_found_block(&ctx, header, test_coinbase_transaction(1), &job)
        .await
        .unwrap();
    let solution = queue.try_recv().unwrap();
    assert_eq!(solution.template_id, 12);
    assert_eq!(solution.block.header, header);
}

#[cfg(not(feature = "sim"))]
#[tokio::test]
async fn a_block_whose_provider_connection_is_gone_goes_over_rpc() {
    let rpc = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_partial_json(
            serde_json::json!({"method": "submitblock"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": null,
            "error": null,
            "id": 0,
        })))
        .expect(1)
        .mount(&rpc)
        .await;
    let (ctx, _emissions, _dirs) = test_bch_stratum_context(&rpc.uri()).await;
    let (solutions, queue) = mpsc::channel(1);
    drop(queue);
    let mut template: BlockTemplate = serde_json::from_str(REGTEST_TEMPLATE).unwrap();
    template.provided = Some(ProvidedTemplate {
        template_id: 12,
        height: 102,
        solutions,
    });
    let job = JobDetails {
        block_available: true,
        blocktemplate: Arc::new(template),
        coinbase1: String::new(),
        coinbase2: String::new(),
        generation_timestamp: 0,
        share_commitment: None,
        coinbase_nsecs: 0,
        template_merkle_branches: Vec::new(),
        share_transactions: Arc::from([]),
    };
    let header = Header {
        version: Version::from_consensus(0x2000_0000),
        prev_blockhash: BlockHash::all_zeros(),
        merkle_root: TxMerkleNode::all_zeros(),
        time: 1,
        bits: CompactTarget::from_consensus(0x207f_ffff),
        nonce: 5,
    };
    submit_found_block(&ctx, header, test_coinbase_transaction(1), &job)
        .await
        .unwrap();
}

#[cfg(not(feature = "sim"))]
#[tokio::test]
async fn an_extended_channel_block_on_a_provided_template_returns_as_submit_solution() {
    let fixture: BlockTemplate = serde_json::from_str(REGTEST_TEMPLATE).unwrap();
    let rpc = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_partial_json(
            serde_json::json!({"method": "getblockheader"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": {"height": fixture.height - 1},
            "error": null,
            "id": 0,
        })))
        .mount(&rpc)
        .await;
    Mock::given(method("POST"))
        .and(body_partial_json(
            serde_json::json!({"method": "submitblock"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": null,
            "error": null,
            "id": 0,
        })))
        .expect(0)
        .mount(&rpc)
        .await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = TemplateProvider {
        address: listener.local_addr().unwrap().to_string(),
        authority_public_key: None,
        network: BitcoinCashNetwork::Regtest,
    };
    let (notify_tx, mut notify_rx) = mpsc::channel(4);
    start_template_provider(
        provider,
        BitcoinRpcConfig {
            url: rpc.uri(),
            username: "user".to_string(),
            password: "pass".to_string(),
        },
        notify_tx,
    )
    .await
    .unwrap();
    let mut server = TestSv2Server::accept(&listener, None).await;
    server.receive(MESSAGE_TYPE_SETUP_CONNECTION).await;
    server
        .send_message(
            MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS,
            SetupConnectionSuccess {
                used_version: 2,
                flags: 0,
            },
        )
        .await;
    server
        .receive(MESSAGE_TYPE_COINBASE_OUTPUT_CONSTRAINTS)
        .await;
    server
        .send_message(
            MESSAGE_TYPE_NEW_TEMPLATE,
            test_tdp_new_template(21, true, &fixture),
        )
        .await;
    server
        .send_message(
            MESSAGE_TYPE_SET_NEW_PREV_HASH,
            test_tdp_set_new_prev_hash(21, &fixture),
        )
        .await;
    let Some(NotifyCmd::SendToAll { template }) = timeout(Duration::from_secs(5), notify_rx.recv())
        .await
        .unwrap()
    else {
        panic!("expected a template for the notifier");
    };
    let prepared = test_bch_prepared_for_template(template.as_ref().clone());
    let (ctx, mut emissions, _dirs) = test_bch_stratum_context(&rpc.uri()).await;
    let mut channels = MiningChannels::new(ctx, DEFAULT_VERSION_MASK);
    let payout = bitcoin::Address::p2pkh(
        bitcoin::PubkeyHash::from_byte_array([0x22; 20]),
        bitcoin::Network::Regtest,
    );
    let identity = encode_payout_address(&payout, BchShareNetwork::Regtest).unwrap();
    let mut open = to_bytes(OpenExtendedMiningChannel {
        request_id: 1,
        user_identity: identity.as_str().try_into().unwrap(),
        nominal_hash_rate: 1.0,
        max_target: U256::new(&[0xff; 32]).unwrap(),
        min_extranonce_size: 8,
    })
    .unwrap();
    let opened = channels
        .handle(
            MESSAGE_TYPE_OPEN_EXTENDED_MINING_CHANNEL,
            &mut open,
            Some(Arc::clone(&prepared)),
        )
        .await
        .unwrap();
    let mut opened_messages =
        decode_plain_frames(&mut Decoder::new(), &mut Vec::new(), &opened.concat()).unwrap();
    let (_, mut success_payload) = opened_messages.remove(0);
    let success = OpenExtendedMiningChannelSuccess::from_bytes(&mut success_payload).unwrap();
    let (_, mut job_payload) = opened_messages.remove(0);
    let job = NewExtendedMiningJob::from_bytes(&mut job_payload).unwrap();
    let (_, mut prev_hash_payload) = opened_messages.remove(0);
    let prev_hash = MiningSetNewPrevHash::from_bytes(&mut prev_hash_payload).unwrap();
    let extranonce = [0x11u8; 8];
    let mut coinbase_bytes = job.coinbase_tx_prefix.as_bytes().to_vec();
    coinbase_bytes.extend_from_slice(success.extranonce_prefix.as_bytes());
    coinbase_bytes.extend_from_slice(&extranonce);
    coinbase_bytes.extend_from_slice(job.coinbase_tx_suffix.as_bytes());
    let coinbase: Transaction = deserialize(&coinbase_bytes).unwrap();
    let unsolved = Header {
        version: Version::from_consensus(job.version as i32),
        prev_blockhash: BlockHash::from_byte_array(*prev_hash.prev_hash.as_array()),
        merkle_root: TxMerkleNode::from_byte_array(coinbase.compute_txid().to_byte_array()),
        time: prev_hash.min_ntime,
        bits: CompactTarget::from_consensus(prev_hash.nbits),
        nonce: 0,
    };
    // Regtest bits: about every second nonce meets the network target.
    let nonce = (0..u32::MAX)
        .find(|nonce| {
            let header = Header {
                nonce: *nonce,
                ..unsolved
            };
            Target::from_compact(header.bits).is_met_by(header.block_hash())
        })
        .unwrap();
    let mut submit = to_bytes(SubmitSharesExtended {
        channel_id: success.channel_id,
        sequence_number: 1,
        job_id: job.job_id,
        nonce,
        ntime: prev_hash.min_ntime,
        version: job.version,
        extranonce: B032::new(&extranonce).unwrap(),
    })
    .unwrap();
    channels
        .handle(MESSAGE_TYPE_SUBMIT_SHARES_EXTENDED, &mut submit, None)
        .await
        .unwrap();
    let mut solution_payload = server.receive(MESSAGE_TYPE_SUBMIT_SOLUTION).await;
    let solution = SubmitSolution::from_bytes(&mut solution_payload).unwrap();
    assert_eq!(solution.template_id, 21);
    assert_eq!(solution.version, job.version);
    assert_eq!(solution.header_timestamp, prev_hash.min_ntime);
    assert_eq!(solution.header_nonce, nonce);
    assert_eq!(solution.coinbase_tx.as_bytes(), coinbase_bytes.as_slice());
    assert_eq!(emissions.try_recv().unwrap().header.nonce, nonce);
}

#[test]
fn a_reconnect_asks_for_a_connection_to_the_named_provider() {
    let mut session = ProviderSession::default();
    let actions = session
        .on_message(
            MESSAGE_TYPE_RECONNECT,
            &mut to_bytes(Reconnect {
                new_host: "127.0.0.1".try_into().unwrap(),
                new_port: 8443,
            })
            .unwrap(),
        )
        .unwrap();
    assert_eq!(
        actions,
        vec![SessionAction::Reconnect {
            host: "127.0.0.1".to_string(),
            port: 8443,
        }]
    );
}

#[tokio::test]
async fn a_reconnect_moves_the_client_to_the_provider_it_names() {
    let rpc = MockServer::start().await;
    let first = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let second = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = TemplateProvider {
        address: first.local_addr().unwrap().to_string(),
        authority_public_key: None,
        network: BitcoinCashNetwork::Regtest,
    };
    let (notify_tx, _notify_rx) = mpsc::channel(4);
    start_template_provider(
        provider,
        BitcoinRpcConfig {
            url: rpc.uri(),
            username: "user".to_string(),
            password: "pass".to_string(),
        },
        notify_tx,
    )
    .await
    .unwrap();
    let mut redirecting = TestSv2Server::accept(&first, None).await;
    redirecting.receive(MESSAGE_TYPE_SETUP_CONNECTION).await;
    redirecting
        .send_message(
            MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS,
            SetupConnectionSuccess {
                used_version: 2,
                flags: 0,
            },
        )
        .await;
    redirecting
        .receive(MESSAGE_TYPE_COINBASE_OUTPUT_CONSTRAINTS)
        .await;
    redirecting
        .send_message(
            MESSAGE_TYPE_RECONNECT,
            Reconnect {
                new_host: "".try_into().unwrap(),
                new_port: second.local_addr().unwrap().port(),
            },
        )
        .await;
    let mut redirected = TestSv2Server::accept(&second, None).await;
    let mut setup_payload = redirected.receive(MESSAGE_TYPE_SETUP_CONNECTION).await;
    let setup = SetupConnection::from_bytes(&mut setup_payload).unwrap();
    assert_eq!(setup.protocol, Protocol::TemplateDistributionProtocol);
    assert_eq!(setup.endpoint_port, second.local_addr().unwrap().port());
}

#[tokio::test]
async fn the_successor_of_a_block_this_node_found_is_published_without_a_parent_lookup() {
    let fixture: BlockTemplate = serde_json::from_str(REGTEST_TEMPLATE).unwrap();
    let rpc = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_partial_json(
            serde_json::json!({"method": "getblockheader"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": {
                "height": fixture.height - 1,
                "ablastate": {"nextblocksizelimit": 40_000_000},
            },
            "error": null,
            "id": 0,
        })))
        .expect(1)
        .mount(&rpc)
        .await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let provider = TemplateProvider {
        address: listener.local_addr().unwrap().to_string(),
        authority_public_key: None,
        network: BitcoinCashNetwork::Regtest,
    };
    let (notify_tx, mut notify_rx) = mpsc::channel(4);
    start_template_provider(
        provider,
        BitcoinRpcConfig {
            url: rpc.uri(),
            username: "user".to_string(),
            password: "pass".to_string(),
        },
        notify_tx,
    )
    .await
    .unwrap();
    let mut server = TestSv2Server::accept(&listener, None).await;
    server.receive(MESSAGE_TYPE_SETUP_CONNECTION).await;
    server
        .send_message(
            MESSAGE_TYPE_SETUP_CONNECTION_SUCCESS,
            SetupConnectionSuccess {
                used_version: 2,
                flags: 0,
            },
        )
        .await;
    server
        .receive(MESSAGE_TYPE_COINBASE_OUTPUT_CONSTRAINTS)
        .await;
    server
        .send_message(
            MESSAGE_TYPE_NEW_TEMPLATE,
            test_tdp_new_template(1, true, &fixture),
        )
        .await;
    server
        .send_message(
            MESSAGE_TYPE_SET_NEW_PREV_HASH,
            test_tdp_set_new_prev_hash(1, &fixture),
        )
        .await;
    let Some(NotifyCmd::SendToAll { template }) = timeout(Duration::from_secs(5), notify_rx.recv())
        .await
        .unwrap()
    else {
        panic!("expected a template for the notifier");
    };
    // The node's limit, above regtest's 32 MB floor.
    assert_eq!(template.sizelimit, 40_000_000);
    let found = Block {
        header: Header {
            version: Version::from_consensus(fixture.version),
            prev_blockhash: BlockHash::from_str(&fixture.previousblockhash).unwrap(),
            merkle_root: TxMerkleNode::all_zeros(),
            time: fixture.curtime,
            bits: CompactTarget::from_unprefixed_hex(&fixture.bits).unwrap(),
            nonce: 3,
        },
        txdata: vec![test_coinbase_transaction(1)],
    };
    template
        .provided
        .as_ref()
        .unwrap()
        .submit(found.clone())
        .unwrap();
    server.receive(MESSAGE_TYPE_SUBMIT_SOLUTION).await;
    let mut constraints_payload = server
        .receive(MESSAGE_TYPE_COINBASE_OUTPUT_CONSTRAINTS)
        .await;
    let constraints = CoinbaseOutputConstraints::from_bytes(&mut constraints_payload).unwrap();
    assert_eq!(
        constraints.coinbase_output_max_additional_size,
        COINBASE_OUTPUT_RESERVE
    );
    let mut successor = fixture.clone();
    successor.previousblockhash = found.block_hash().to_string();
    successor.height = fixture.height + 1;
    server
        .send_message(
            MESSAGE_TYPE_NEW_TEMPLATE,
            test_tdp_new_template(2, true, &successor),
        )
        .await;
    server
        .send_message(
            MESSAGE_TYPE_SET_NEW_PREV_HASH,
            test_tdp_set_new_prev_hash(2, &successor),
        )
        .await;
    let Some(NotifyCmd::SendToAll { template }) = timeout(Duration::from_secs(5), notify_rx.recv())
        .await
        .unwrap()
    else {
        panic!("expected the successor template for the notifier");
    };
    assert_eq!(template.height, fixture.height + 1);
    assert_eq!(template.previousblockhash, found.block_hash().to_string());
    // Not asked about the found block, the node's last limit stands.
    assert_eq!(template.sizelimit, 40_000_000);
}
