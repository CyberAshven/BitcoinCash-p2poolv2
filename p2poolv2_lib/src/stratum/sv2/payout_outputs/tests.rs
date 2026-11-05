use super::*;
use crate::stratum::sv2::custom_job::validate_custom_job;
use crate::stratum::sv2::preparation::prepare_coinbase;
use crate::stratum::sv2::session::DeclaredJob;
use crate::test_utils::{
    test_bch_custom_job_with_aux, test_bch_prepared, test_bch_prepared_with_cuts,
    test_bch_prepared_with_payee_count, test_custom_job_from_payout_outputs,
    test_payout_outputs_request,
};
use bitcoin::consensus::{deserialize, serialize};
use bitcoin::script::Builder;
use bitcoin::{Address, Amount, Network, PubkeyHash, TxOut};
use mining_sv2::SetCustomMiningJob;
use p2poolv2_config::DEFAULT_VERSION_MASK;
use std::sync::Arc;

#[test]
fn request_payout_outputs_encodes_its_fields_in_order_little_endian() {
    let token = [0xaa; 3];
    let prev_hash = [0x11; 32];
    let prefix = [0x51, 0x52];
    let request = RequestPayoutOutputs {
        request_id: 0x0102_0304,
        mining_job_token: B0255::new(&token).unwrap(),
        available_payout_value: 0x0506_0708_090a_0b0c,
        height: 0x0d0e_0f10,
        prev_hash: U256::new(&prev_hash).unwrap(),
        nbits: 0x1112_1314,
        coinbase_prefix: B0255::new(&prefix).unwrap(),
    };

    let mut bytes = binary_sv2::to_bytes(request.clone()).unwrap();

    assert_eq!(
        bytes,
        [
            &[0x04, 0x03, 0x02, 0x01][..],
            &[3, 0xaa, 0xaa, 0xaa],
            &[0x0c, 0x0b, 0x0a, 0x09, 0x08, 0x07, 0x06, 0x05],
            &[0x10, 0x0f, 0x0e, 0x0d],
            &[0x11; 32],
            &[0x14, 0x13, 0x12, 0x11],
            &[2, 0x51, 0x52],
        ]
        .concat()
    );
    assert_eq!(
        RequestPayoutOutputs::from_bytes(&mut bytes).unwrap(),
        request
    );
}

#[test]
fn success_and_error_replies_round_trip() {
    let outputs = [0x00];
    let success = RequestPayoutOutputsSuccess {
        request_id: 7,
        flags: EXACT_OUTPUTS,
        coinbase_tx_version: 2,
        coinbase_tx_input_n_sequence: 0xffff_fffe,
        coinbase_tx_locktime: 99,
        coinbase_tx_outputs: B064K::new(&outputs).unwrap(),
    };
    let error = RequestPayoutOutputsError {
        request_id: 7,
        error_code: STALE_PAYOUT_OUTPUTS.try_into().unwrap(),
    };

    let mut success_bytes = binary_sv2::to_bytes(success.clone()).unwrap();
    let mut error_bytes = binary_sv2::to_bytes(error.clone()).unwrap();

    assert_eq!(
        success_bytes,
        [
            7, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 0xfe, 0xff, 0xff, 0xff, 99, 0, 0, 0, 1, 0, 0x00
        ]
    );
    assert_eq!(
        RequestPayoutOutputsSuccess::from_bytes(&mut success_bytes).unwrap(),
        success
    );
    assert_eq!(
        error_bytes,
        [&[7, 0, 0, 0, 20][..], b"stale-payout-outputs"].concat()
    );
    assert_eq!(
        RequestPayoutOutputsError::from_bytes(&mut error_bytes).unwrap(),
        error
    );
}

/// `PayoutOutputsStale` is a header alone: extension `0x5052` with the
/// channel bit clear, message type `0x03`, and an empty payload.
#[test]
fn the_stale_notice_is_an_empty_extension_frame() {
    assert_eq!(stale_frame().unwrap(), [0x52, 0x50, 0x03, 0, 0, 0]);
}

/// The outputs a request gets are the ones `/jd_prepare`'s builder makes for
/// the same token, identity, weights, revenue, height and prefix, and the
/// ones the custom-job validator rebuilds from the token's snapshot.
#[test]
fn requested_outputs_are_the_bytes_jd_prepare_builds() {
    let fee_address = Address::p2pkh(PubkeyHash::from_byte_array([0xfe; 20]), Network::Regtest);
    let prepared = test_bch_prepared_with_cuts(Some(100), Some(fee_address.into()), Some(100));
    let template = prepared.template();
    let token = prepared
        .mining_job_token([0x11; 32], template.curtime)
        .unwrap();
    let value = template.coinbasevalue + 219;
    let prefix = Builder::new()
        .push_int(i64::from(template.height))
        .push_slice(b"miner")
        .into_bytes();
    let payout = PayoutAddress::from(Address::p2pkh(
        PubkeyHash::from_byte_array([0x22; 20]),
        Network::Regtest,
    ));
    let owner = ShareOwner::CashP2pkh([0x22; 20]);
    let mut request_bytes = test_payout_outputs_request(1, &token, template, value, &prefix);
    let request = RequestPayoutOutputs::from_bytes(&mut request_bytes).unwrap();
    let identity = PayoutIdentity {
        payout: payout.clone(),
        owner,
    };

    let answered = prepare_payout_outputs(
        &request,
        &prepared,
        Some(&identity),
        Some(prepared.prev_share_blockhash()),
    )
    .unwrap();

    let jd_prepare = prepare_coinbase(
        &token,
        ShareCommitment::from_mining_job_token(&token, payout, owner).unwrap(),
        prepared.pplns_weights().unwrap(),
        value,
        template.height,
        BchCoinbase {
            prefix: prefix.clone(),
            merge_mining: MergeMiningCommitment::empty(),
        },
    )
    .unwrap();
    assert_eq!(answered.prefix, jd_prepare.prefix);
    assert_eq!(answered.suffix, jd_prepare.suffix);
    let (mut custom_bytes, _) = test_bch_custom_job_with_aux(
        Arc::clone(&prepared),
        token,
        vec![],
        false,
        value,
        MergeMiningCommitment::empty(),
    );
    let custom = SetCustomMiningJob::from_bytes(&mut custom_bytes).unwrap();
    assert_eq!(answered.outputs(), custom.coinbase_tx_outputs.as_bytes());
    let outputs: Vec<TxOut> = deserialize(answered.outputs()).unwrap();
    assert_eq!(
        MergeMiningCommitment::from_output(&outputs[0]).unwrap(),
        MergeMiningCommitment::empty()
    );
    assert_eq!(
        outputs
            .iter()
            .map(|output| output.value.to_sat())
            .sum::<u64>(),
        value
    );
}

/// A client that builds its custom job from the reply alone, with the prefix
/// it asked for, gets it validated; the reply says exact outputs, version 2,
/// sequence `0xfffffffe` and the height less one.
#[test]
fn a_custom_job_built_from_the_reply_alone_is_valid() {
    let prepared = test_bch_prepared();
    let template = prepared.template();
    let token = prepared
        .mining_job_token([0x11; 32], template.curtime)
        .unwrap();
    let prefix = Builder::new()
        .push_int(i64::from(template.height))
        .push_slice(b"kth")
        .into_bytes();
    let payout = PayoutAddress::from(Address::p2pkh(
        PubkeyHash::from_byte_array([0x22; 20]),
        Network::Regtest,
    ));
    let owner = ShareOwner::CashP2pkh([0x22; 20]);
    let mut request_bytes =
        test_payout_outputs_request(1, &token, template, template.coinbasevalue, &prefix);
    let request = RequestPayoutOutputs::from_bytes(&mut request_bytes).unwrap();
    let answered = prepare_payout_outputs(
        &request,
        &prepared,
        Some(&PayoutIdentity {
            payout: payout.clone(),
            owner,
        }),
        Some(prepared.prev_share_blockhash()),
    );

    let frame = answer_frame(&request, answered).unwrap();

    assert_eq!(frame[..3], [0x52, 0x50, REQUEST_PAYOUT_OUTPUTS_SUCCESS]);
    let mut payload = frame[6..].to_vec();
    let success = RequestPayoutOutputsSuccess::from_bytes(&mut payload).unwrap();
    assert_eq!(success.request_id, 1);
    assert_eq!(success.flags, EXACT_OUTPUTS);
    assert_eq!(success.coinbase_tx_version, 2);
    assert_eq!(success.coinbase_tx_input_n_sequence, 0xffff_fffe);
    assert_eq!(success.coinbase_tx_locktime, template.height - 1);
    let mut custom_bytes =
        test_custom_job_from_payout_outputs(&success, &token, template, &prefix, &[]);
    let custom = SetCustomMiningJob::from_bytes(&mut custom_bytes).unwrap();
    let declared = DeclaredJob {
        prepared: Arc::clone(&prepared),
        transactions: None,
        declaration: None,
    };
    assert!(validate_custom_job(&custom, declared, payout, owner, DEFAULT_VERSION_MASK).is_ok());
}

/// The share commitment binds the scriptSig prefix, so outputs asked for one
/// prefix do not validate under another.
#[test]
fn a_custom_job_with_another_prefix_than_requested_is_refused() {
    let prepared = test_bch_prepared();
    let template = prepared.template();
    let token = prepared
        .mining_job_token([0x11; 32], template.curtime)
        .unwrap();
    let prefix = Builder::new()
        .push_int(i64::from(template.height))
        .push_slice(b"kth")
        .into_bytes();
    let other_prefix = Builder::new()
        .push_int(i64::from(template.height))
        .push_slice(b"other")
        .into_bytes();
    let payout = PayoutAddress::from(Address::p2pkh(
        PubkeyHash::from_byte_array([0x22; 20]),
        Network::Regtest,
    ));
    let owner = ShareOwner::CashP2pkh([0x22; 20]);
    let mut request_bytes =
        test_payout_outputs_request(1, &token, template, template.coinbasevalue, &prefix);
    let request = RequestPayoutOutputs::from_bytes(&mut request_bytes).unwrap();
    let answered = prepare_payout_outputs(
        &request,
        &prepared,
        Some(&PayoutIdentity {
            payout: payout.clone(),
            owner,
        }),
        Some(prepared.prev_share_blockhash()),
    );
    let mut payload = answer_frame(&request, answered).unwrap()[6..].to_vec();
    let success = RequestPayoutOutputsSuccess::from_bytes(&mut payload).unwrap();

    let mut custom_bytes =
        test_custom_job_from_payout_outputs(&success, &token, template, &other_prefix, &[]);

    let custom = SetCustomMiningJob::from_bytes(&mut custom_bytes).unwrap();
    let declared = DeclaredJob {
        prepared: Arc::clone(&prepared),
        transactions: None,
        declaration: None,
    };
    assert_eq!(
        validate_custom_job(&custom, declared, payout, owner, DEFAULT_VERSION_MASK).unwrap_err(),
        "invalid-pplns-outputs"
    );
}

/// A client whose template pays 1,000 satoshis more than it asked outputs
/// for, and adds them to an output, declares another value: the validator
/// splits that value again and refuses the job.
#[test]
fn a_custom_job_paying_another_value_than_requested_is_refused() {
    let prepared = test_bch_prepared();
    let template = prepared.template();
    let token = prepared
        .mining_job_token([0x11; 32], template.curtime)
        .unwrap();
    let prefix = Builder::new()
        .push_int(i64::from(template.height))
        .push_slice(b"kth")
        .into_bytes();
    let payout = PayoutAddress::from(Address::p2pkh(
        PubkeyHash::from_byte_array([0x22; 20]),
        Network::Regtest,
    ));
    let owner = ShareOwner::CashP2pkh([0x22; 20]);
    let mut request_bytes =
        test_payout_outputs_request(1, &token, template, template.coinbasevalue, &prefix);
    let request = RequestPayoutOutputs::from_bytes(&mut request_bytes).unwrap();
    let answered = prepare_payout_outputs(
        &request,
        &prepared,
        Some(&PayoutIdentity {
            payout: payout.clone(),
            owner,
        }),
        Some(prepared.prev_share_blockhash()),
    );
    let mut payload = answer_frame(&request, answered).unwrap()[6..].to_vec();
    let success = RequestPayoutOutputsSuccess::from_bytes(&mut payload).unwrap();
    let mut outputs: Vec<TxOut> = deserialize(success.coinbase_tx_outputs.as_bytes()).unwrap();
    outputs[1].value += Amount::from_sat(1_000);
    let outputs = serialize(&outputs);
    let more = RequestPayoutOutputsSuccess {
        coinbase_tx_outputs: B064K::new(&outputs).unwrap(),
        ..success
    };

    let mut custom_bytes =
        test_custom_job_from_payout_outputs(&more, &token, template, &prefix, &[]);

    let custom = SetCustomMiningJob::from_bytes(&mut custom_bytes).unwrap();
    let declared = DeclaredJob {
        prepared: Arc::clone(&prepared),
        transactions: None,
        declaration: None,
    };
    assert_eq!(
        validate_custom_job(&custom, declared, payout, owner, DEFAULT_VERSION_MASK).unwrap_err(),
        "invalid-pplns-outputs"
    );
}

#[test]
fn a_token_on_an_earlier_anchor_is_stale() {
    let prepared = test_bch_prepared();
    let template = prepared.template();
    let token = prepared
        .mining_job_token([0x11; 32], template.curtime)
        .unwrap();
    let prefix = Builder::new()
        .push_int(i64::from(template.height))
        .into_bytes();
    let mut request_bytes =
        test_payout_outputs_request(1, &token, template, template.coinbasevalue, &prefix);
    let request = RequestPayoutOutputs::from_bytes(&mut request_bytes).unwrap();
    let identity = PayoutIdentity {
        payout: PayoutAddress::from(Address::p2pkh(
            PubkeyHash::from_byte_array([0x22; 20]),
            Network::Regtest,
        )),
        owner: ShareOwner::CashP2pkh([0x22; 20]),
    };

    let answered = prepare_payout_outputs(
        &request,
        &prepared,
        Some(&identity),
        Some(BlockHash::from_byte_array([0x44; 32])),
    );

    assert_eq!(answered.unwrap_err(), STALE_PAYOUT_OUTPUTS);
}

#[test]
fn a_request_for_another_height_than_the_tokens_template_is_stale() {
    let prepared = test_bch_prepared();
    let template = prepared.template();
    let token = prepared
        .mining_job_token([0x11; 32], template.curtime)
        .unwrap();
    let prefix = Builder::new()
        .push_int(i64::from(template.height + 1))
        .into_bytes();
    let mut request_bytes =
        test_payout_outputs_request(1, &token, template, template.coinbasevalue, &prefix);
    let mut request = RequestPayoutOutputs::from_bytes(&mut request_bytes).unwrap();
    request.height += 1;
    let identity = PayoutIdentity {
        payout: PayoutAddress::from(Address::p2pkh(
            PubkeyHash::from_byte_array([0x22; 20]),
            Network::Regtest,
        )),
        owner: ShareOwner::CashP2pkh([0x22; 20]),
    };

    let answered = prepare_payout_outputs(
        &request,
        &prepared,
        Some(&identity),
        Some(prepared.prev_share_blockhash()),
    );

    assert_eq!(answered.unwrap_err(), STALE_PAYOUT_OUTPUTS);
}

/// A token allocated for an identity that names no payout address and share
/// owner has no outputs to give.
#[test]
fn a_token_without_an_identity_is_an_invalid_request() {
    let prepared = test_bch_prepared();
    let template = prepared.template();
    let token = prepared
        .mining_job_token([0x11; 32], template.curtime)
        .unwrap();
    let prefix = Builder::new()
        .push_int(i64::from(template.height))
        .into_bytes();
    let mut request_bytes =
        test_payout_outputs_request(1, &token, template, template.coinbasevalue, &prefix);
    let request = RequestPayoutOutputs::from_bytes(&mut request_bytes).unwrap();

    let answered = prepare_payout_outputs(
        &request,
        &prepared,
        None,
        Some(prepared.prev_share_blockhash()),
    );

    assert_eq!(answered.unwrap_err(), INVALID_REQUEST);
}

/// The prefix must begin with the template's height push, as `/jd_prepare`
/// requires.
#[test]
fn a_prefix_without_the_height_push_is_an_invalid_request() {
    let prepared = test_bch_prepared();
    let template = prepared.template();
    let token = prepared
        .mining_job_token([0x11; 32], template.curtime)
        .unwrap();
    let mut request_bytes = test_payout_outputs_request(
        1,
        &token,
        template,
        template.coinbasevalue,
        b"no height push",
    );
    let request = RequestPayoutOutputs::from_bytes(&mut request_bytes).unwrap();
    let identity = PayoutIdentity {
        payout: PayoutAddress::from(Address::p2pkh(
            PubkeyHash::from_byte_array([0x22; 20]),
            Network::Regtest,
        )),
        owner: ShareOwner::CashP2pkh([0x22; 20]),
    };

    let answered = prepare_payout_outputs(
        &request,
        &prepared,
        Some(&identity),
        Some(prepared.prev_share_blockhash()),
    );

    assert_eq!(answered.unwrap_err(), INVALID_REQUEST);
}

#[test]
fn a_value_above_the_monetary_range_is_an_invalid_request() {
    let prepared = test_bch_prepared();
    let template = prepared.template();
    let token = prepared
        .mining_job_token([0x11; 32], template.curtime)
        .unwrap();
    let prefix = Builder::new()
        .push_int(i64::from(template.height))
        .into_bytes();
    let mut request_bytes =
        test_payout_outputs_request(1, &token, template, Amount::MAX_MONEY.to_sat() + 1, &prefix);
    let request = RequestPayoutOutputs::from_bytes(&mut request_bytes).unwrap();
    let identity = PayoutIdentity {
        payout: PayoutAddress::from(Address::p2pkh(
            PubkeyHash::from_byte_array([0x22; 20]),
            Network::Regtest,
        )),
        owner: ShareOwner::CashP2pkh([0x22; 20]),
    };

    let answered = prepare_payout_outputs(
        &request,
        &prepared,
        Some(&identity),
        Some(prepared.prev_share_blockhash()),
    );

    assert_eq!(answered.unwrap_err(), INVALID_REQUEST);
}

/// 2,000 P2PKH payees fit BCH's coinbase but not the 65,535-byte field that
/// carries a declared coinbase's outputs.
#[test]
fn outputs_past_a_declared_field_are_too_large() {
    let prepared = test_bch_prepared_with_payee_count(2_000);
    let template = prepared.template();
    let token = prepared
        .mining_job_token([0x11; 32], template.curtime)
        .unwrap();
    let prefix = Builder::new()
        .push_int(i64::from(template.height))
        .into_bytes();
    let mut request_bytes =
        test_payout_outputs_request(1, &token, template, template.coinbasevalue, &prefix);
    let request = RequestPayoutOutputs::from_bytes(&mut request_bytes).unwrap();
    let identity = PayoutIdentity {
        payout: PayoutAddress::from(Address::p2pkh(
            PubkeyHash::from_byte_array([0x22; 20]),
            Network::Regtest,
        )),
        owner: ShareOwner::CashP2pkh([0x22; 20]),
    };

    let answered = prepare_payout_outputs(
        &request,
        &prepared,
        Some(&identity),
        Some(prepared.prev_share_blockhash()),
    );

    assert_eq!(answered.unwrap_err(), PAYOUT_OUTPUTS_TOO_LARGE);
}

#[test]
fn an_error_reply_carries_its_code() {
    let prepared = test_bch_prepared();
    let template = prepared.template();
    let token = prepared
        .mining_job_token([0x11; 32], template.curtime)
        .unwrap();
    let mut request_bytes =
        test_payout_outputs_request(5, &token, template, template.coinbasevalue, &[]);
    let request = RequestPayoutOutputs::from_bytes(&mut request_bytes).unwrap();

    let frame = answer_frame(&request, Err(PAYOUT_OUTPUTS_UNAVAILABLE)).unwrap();

    assert_eq!(frame[..3], [0x52, 0x50, REQUEST_PAYOUT_OUTPUTS_ERROR]);
    let mut payload = frame[6..].to_vec();
    let error = RequestPayoutOutputsError::from_bytes(&mut payload).unwrap();
    assert_eq!(error.request_id, 5);
    assert_eq!(
        error.error_code.as_utf8_or_hex(),
        PAYOUT_OUTPUTS_UNAVAILABLE
    );
}
