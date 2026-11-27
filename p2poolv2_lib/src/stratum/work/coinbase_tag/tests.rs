// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use crate::accounting::{CoinbasePayouts, OutputPair};
use crate::config::StratumConfig;
use crate::shares::bch_coinbase::BchCoinbase;
use crate::shares::merge_mining::MergeMiningCommitment;
use crate::stratum::work::block_template::parse_flags;
use crate::stratum::work::coinbase::{
    build_coinbase, build_declared_coinbase, parse_bitcoin_coinbase_fields,
};
use crate::test_utils::{make_test_address, test_share_chain_id};
use bitcoin::script::{Builder, PushBytesBuf};
use bitcoin::transaction::Version;
use bitcoin::{Amount, Network};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[test]
fn the_same_share_chain_always_gets_the_same_id() {
    let genesis = BlockHash::from_byte_array([1; 32]);
    assert_eq!(
        ShareChainId::new(Network::Regtest, Chain::BitcoinCash, genesis),
        ShareChainId::new(Network::Regtest, Chain::BitcoinCash, genesis)
    );
}

#[test]
fn another_genesis_gets_another_id() {
    assert_ne!(
        ShareChainId::new(
            Network::Regtest,
            Chain::BitcoinCash,
            BlockHash::from_byte_array([1; 32])
        ),
        ShareChainId::new(
            Network::Regtest,
            Chain::BitcoinCash,
            BlockHash::from_byte_array([2; 32])
        )
    );
}

/// The genesis digest of 32 bytes of 0x01 starts 72 cd 6e and the protocol
/// digest of the `rules-a` string 46 26 (`sha256sum`): the family is
/// 0x72cd6 (01110 01011 00110 10110: `w`, `t`, `x`, `k`) and the fork
/// 0b0100011000 (01000 11000: `g`, `c`).
#[test]
fn the_id_is_the_family_of_the_genesis_then_the_fork_of_the_rules() {
    assert_eq!(
        ShareChainId::from_parts(
            BlockHash::from_byte_array([1; 32]),
            "/p2pool/regtest/0101010/rules-a/1.0.0"
        )
        .to_string(),
        "wtxkgc"
    );
}

/// The `rules-b` digest starts 77 bf: fork 0b0111011110 (01110 11110: `w`,
/// `7`). The family stays.
#[test]
fn other_rules_on_the_same_genesis_keep_the_family_and_change_the_fork() {
    let genesis = BlockHash::from_byte_array([1; 32]);
    let rules_a = ShareChainId::from_parts(genesis, "/p2pool/regtest/0101010/rules-a/1.0.0");
    let rules_b = ShareChainId::from_parts(genesis, "/p2pool/regtest/0101010/rules-b/1.0.0");

    assert_eq!(rules_b.to_string(), "wtxkw7");
    assert_eq!(rules_a.family(), rules_b.family());
    assert_ne!(rules_a.as_bytes()[4..], rules_b.as_bytes()[4..]);
}

/// The genesis digest of 32 bytes of 0x02 starts 75 87 7b: family 0x75877
/// (01110 10110 00011 10111: `w`, `k`, `r`, `h`).
#[test]
fn another_genesis_changes_the_family() {
    let first = ShareChainId::from_parts(
        BlockHash::from_byte_array([1; 32]),
        "/p2pool/regtest/0101010/rules-a/1.0.0",
    );
    let second = ShareChainId::from_parts(
        BlockHash::from_byte_array([2; 32]),
        "/p2pool/regtest/0101010/rules-a/1.0.0",
    );

    assert_eq!(second.to_string(), "wkrhgc");
    assert_ne!(first.family(), second.family());
}

/// The fork part hashes the protocol string the node connects with, so a
/// node takes it from `network_prologue`.
#[test]
fn the_id_of_a_share_chain_reads_the_protocol_string_of_the_node() {
    let genesis = BlockHash::from_byte_array([1; 32]);
    assert_eq!(
        ShareChainId::new(Network::Regtest, Chain::BitcoinCash, genesis),
        ShareChainId::from_parts(
            genesis,
            &network_prologue(Network::Regtest, Chain::BitcoinCash, genesis)
        )
    );
}

/// Bitcoin Cash share chains carry rule tags in their protocol string that
/// Bitcoin share chains do not, so only the fork part can tell them apart:
/// the same genesis keeps the family.
#[test]
fn a_bitcoin_share_chain_on_the_same_genesis_keeps_the_family() {
    let genesis = BlockHash::from_byte_array([1; 32]);
    let bitcoin = ShareChainId::new(Network::Regtest, Chain::Bitcoin, genesis);
    let bitcoin_cash = ShareChainId::new(Network::Regtest, Chain::BitcoinCash, genesis);

    assert_eq!(bitcoin.family(), bitcoin_cash.family());
    assert_eq!(bitcoin.family(), b"wtxk");
}

/// Family 0x12345 is 00010 01000 11010 00101: 2, 8, 26 and 5, which
/// CashAddr writes `z`, `g`, `6` and `9`. Fork 0x78 0x9a gives the 10 bits
/// 01111 00010: 15 and 2, written `0` and `z`.
#[test]
fn the_id_writes_five_bits_a_character_most_significant_first() {
    assert_eq!(
        ShareChainId::from_digest_starts([0x12, 0x34, 0x56], [0x78, 0x9a]).to_string(),
        "zg690z"
    );
}

#[test]
fn bits_past_the_twenty_of_the_family_and_the_ten_of_the_fork_are_not_written() {
    assert_eq!(
        ShareChainId::from_digest_starts([0x12, 0x34, 0x5f], [0x78, 0xbf]),
        ShareChainId::from_digest_starts([0x12, 0x34, 0x56], [0x78, 0x9a])
    );
}

#[test]
fn digests_of_zeros_are_written_as_q() {
    assert_eq!(
        ShareChainId::from_digest_starts([0; 3], [0; 2]).to_string(),
        "qqqqqq"
    );
}

#[test]
fn digests_of_ones_are_written_as_l() {
    assert_eq!(
        ShareChainId::from_digest_starts([0xff; 3], [0xff; 2]).to_string(),
        "llllll"
    );
}

#[test]
fn a_full_tag_has_the_operator_node_and_chain_lines() {
    let tag = NodeCoinbaseTag::new("CyberAshven", "BCHN 29.2.1", test_share_chain_id());
    assert_eq!(
        tag.as_bytes(),
        b"P2Poolv2\x0fCyberAshven\x0fBCHN 29.2.1\x0fzg690z"
    );
    assert_eq!(
        tag.to_string(),
        "P2Poolv2 | CyberAshven | BCHN 29.2.1 | zg690z"
    );
}

#[test]
fn a_tag_without_an_operator_keeps_the_node_line_in_its_place() {
    let tag = NodeCoinbaseTag::new("", "BCHN 29.2.1 node1", test_share_chain_id());
    assert_eq!(
        tag.as_bytes(),
        b"P2Poolv2\x0f\x0fBCHN 29.2.1 node1\x0fzg690z"
    );
}

#[test]
fn a_tag_without_a_node_line_keeps_the_chain_line_in_its_place() {
    let tag = NodeCoinbaseTag::new("CyberAshven", "", test_share_chain_id());
    assert_eq!(tag.as_bytes(), b"P2Poolv2\x0fCyberAshven\x0f\x0fzg690z");
}

#[test]
fn a_tag_without_an_operator_or_a_node_line_writes_every_line() {
    let tag = NodeCoinbaseTag::new("", "", test_share_chain_id());
    assert_eq!(tag.as_bytes(), b"P2Poolv2\x0f\x0f\x0fzg690z");
    assert_eq!(tag.share_chain_id(), test_share_chain_id());
}

#[test]
fn the_bchn_subversion_becomes_bchn_and_its_version() {
    assert_eq!(
        node_software_line("/Bitcoin Cash Node:29.2.1(EB32.0)/", MAX_NODE_LINE_LENGTH),
        Ok("BCHN 29.2.1".to_string())
    );
}

#[test]
fn the_comments_of_a_bchn_subversion_are_dropped() {
    assert_eq!(
        node_software_line(
            "/Bitcoin Cash Node:29.2.1(EB32.0; rig 7)/",
            MAX_NODE_LINE_LENGTH
        ),
        Ok("BCHN 29.2.1".to_string())
    );
}

#[test]
fn the_knuth_subversion_becomes_knuth_and_its_version() {
    assert_eq!(
        node_software_line("/Knuth:1.7.0/", MAX_NODE_LINE_LENGTH),
        Ok("Knuth 1.7.0".to_string())
    );
}

/// BCHD announces its user agent name `/bchd` in lower case.
#[test]
fn the_bchd_subversion_becomes_bchd_and_its_version() {
    assert_eq!(
        node_software_line("/bchd:0.22.4/", MAX_NODE_LINE_LENGTH),
        Ok("BCHD 0.22.4".to_string())
    );
}

#[test]
fn an_upper_case_bchd_subversion_becomes_bchd_and_its_version() {
    assert_eq!(
        node_software_line("/BCHD:0.22.4(EB32.0)/", MAX_NODE_LINE_LENGTH),
        Ok("BCHD 0.22.4".to_string())
    );
}

#[test]
fn other_software_keeps_its_own_name_and_version() {
    assert_eq!(
        node_software_line("/Flowee:2025.10.1/", MAX_NODE_LINE_LENGTH),
        Ok("Flowee 2025.10.1".to_string())
    );
}

#[test]
fn a_subversion_with_a_character_outside_the_name_rules_is_left_out() {
    let error = node_software_line("/Cash+Node:1.0/", MAX_NODE_LINE_LENGTH).unwrap_err();
    assert!(error.contains("'+'"), "{error}");
}

#[test]
fn a_subversion_longer_than_the_node_line_is_left_out() {
    let error =
        node_software_line("/A Very Long Node Software:1.0.0/", MAX_NODE_LINE_LENGTH).unwrap_err();
    assert!(error.contains("more than the 23 allowed"), "{error}");
}

#[test]
fn an_empty_subversion_is_left_out() {
    assert!(node_software_line("", MAX_NODE_LINE_LENGTH).is_err());
}

#[test]
fn node_software_that_does_not_fit_beside_the_longest_label_is_left_out() {
    assert!(node_software_line("/Flowee:2025.10.1/", software_room("label-08")).is_err());
}

#[test]
fn bchn_fits_beside_the_longest_label() {
    assert_eq!(
        node_software_line(
            "/Bitcoin Cash Node:29.2.1(EB32.0)/",
            software_room("label-08")
        ),
        Ok("BCHN 29.2.1".to_string())
    );
}

#[test]
fn the_node_line_is_the_software_then_the_label() {
    assert_eq!(node_line("BCHN 29.2.1", "node1"), "BCHN 29.2.1 node1");
}

#[test]
fn the_node_line_is_the_label_alone_without_software() {
    assert_eq!(node_line("", "node1"), "node1");
}

#[test]
fn the_node_line_is_the_software_alone_without_a_label() {
    assert_eq!(node_line("BCHN 29.2.1", ""), "BCHN 29.2.1");
}

#[test]
fn the_node_line_is_empty_without_software_or_a_label() {
    assert_eq!(node_line("", ""), "");
}

/// The longest tag at the worst case of every other push of the scriptSig:
/// a height past 8,388,607, the BCHD aux flags (`/bchd/` pushed), the
/// extranonce and the nanosecond time, with merge mining, fills the 100
/// bytes of a Bitcoin Cash scriptSig exactly.
#[test]
fn the_longest_tag_at_the_largest_other_pushes_fits_the_scriptsig() {
    let software = node_software_line(
        "/Bitcoin Cash Node:129.20.10(EB32.0)/",
        software_room("label-08"),
    )
    .unwrap();
    let tag = NodeCoinbaseTag::new(
        "Longest-Operator-Name-24",
        &node_line(&software, "label-08"),
        test_share_chain_id(),
    );
    assert_eq!(tag.as_bytes().len(), MAX_COINBASE_TAG_BYTES);

    let coinbase = build_coinbase(
        Version::TWO,
        &[OutputPair {
            address: make_test_address(1),
            amount: Amount::from_sat(312_500_000),
        }],
        499_999_999,
        parse_flags(Some("062f626368642f".to_string())).unwrap(),
        None,
        tag.as_bytes(),
        Some(sha256::Hash::all_zeros()),
        u64::MAX,
        Some(&[0xff; EXTRANONCE1_SIZE + EXTRANONCE2_SIZE]),
        Some(MergeMiningCommitment::empty()),
        None,
    )
    .unwrap();

    let script_sig = &coinbase.input[0].script_sig;
    assert!(script_sig.len() <= MAX_COINBASE_SCRIPT_SIG_BYTES);
    assert_eq!(script_sig.len(), MAX_COINBASE_SCRIPT_SIG_BYTES);
    assert_eq!(
        parse_bitcoin_coinbase_fields(&coinbase)
            .unwrap()
            .pool_signature,
        tag.as_bytes()
    );
}

/// A declared job carries the height and the tag before the extranonce,
/// without aux flags or a nanosecond time: the longest tag fits its 88-byte
/// prefix and the scriptSig.
#[test]
fn the_longest_tag_fits_a_declared_prefix() {
    let tag = NodeCoinbaseTag::new(
        "Longest-Operator-Name-24",
        "BCHN 129.20.10 label-08",
        test_share_chain_id(),
    );
    assert_eq!(tag.as_bytes().len(), MAX_COINBASE_TAG_BYTES);
    let prefix = Builder::new()
        .push_int(499_999_999)
        .push_slice(PushBytesBuf::try_from(tag.as_bytes().to_vec()).unwrap())
        .into_bytes();
    let declared = BchCoinbase {
        prefix: prefix.clone(),
        merge_mining: MergeMiningCommitment::empty(),
    };
    assert_eq!(declared.validate(499_999_999), Ok(()));

    let coinbase = build_declared_coinbase(
        &CoinbasePayouts::direct(vec![OutputPair {
            address: make_test_address(1),
            amount: Amount::from_sat(312_500_000),
        }]),
        499_999_999,
        sha256::Hash::all_zeros(),
        &[0; EXTRANONCE1_SIZE + EXTRANONCE2_SIZE],
        &declared,
    )
    .unwrap();

    assert!(coinbase.input[0].script_sig.len() <= MAX_COINBASE_SCRIPT_SIG_BYTES);
    assert_eq!(
        check_declared_coinbase_tag(&prefix, test_share_chain_id()),
        Ok(())
    );
}

#[test]
fn a_declared_tag_of_this_share_chain_is_accepted() {
    let prefix = Builder::new()
        .push_int(702)
        .push_slice(b"P2Poolv2\x0fminer\x0fKnuth 1.7.0\x0fzg690z")
        .into_bytes();
    assert_eq!(
        check_declared_coinbase_tag(&prefix, test_share_chain_id()),
        Ok(())
    );
}

#[test]
fn a_declared_tag_after_another_push_is_accepted() {
    let prefix = Builder::new()
        .push_int(702)
        .push_slice(b"unique id")
        .push_slice(b"P2Poolv2\x0f\x0f\x0fzg690z")
        .into_bytes();
    assert_eq!(
        check_declared_coinbase_tag(&prefix, test_share_chain_id()),
        Ok(())
    );
}

#[test]
fn a_declared_prefix_without_the_marker_is_refused() {
    let prefix = Builder::new()
        .push_int(702)
        .push_slice(b"miner")
        .into_bytes();
    let error = check_declared_coinbase_tag(&prefix, test_share_chain_id()).unwrap_err();
    assert!(error.contains("no push that starts with"), "{error}");
}

#[test]
fn a_declared_prefix_with_the_old_signature_alone_is_refused() {
    let prefix = Builder::new()
        .push_int(702)
        .push_slice(b"P2Poolv2")
        .into_bytes();
    let error = check_declared_coinbase_tag(&prefix, test_share_chain_id()).unwrap_err();
    assert!(error.contains("no push that starts with"), "{error}");
}

#[test]
fn a_declared_tag_of_another_share_chain_is_refused() {
    let prefix = Builder::new()
        .push_int(702)
        .push_slice(b"P2Poolv2\x0fminer\x0f\x0fqqqqqq")
        .into_bytes();
    let error = check_declared_coinbase_tag(&prefix, test_share_chain_id()).unwrap_err();
    assert!(error.contains("another share chain"), "{error}");
}

#[test]
fn a_declared_tag_without_a_chain_line_is_refused() {
    let prefix = Builder::new()
        .push_int(702)
        .push_slice(b"P2Poolv2\x0fminer\x0fKnuth 1.7.0")
        .into_bytes();
    let error = check_declared_coinbase_tag(&prefix, test_share_chain_id()).unwrap_err();
    assert!(error.contains("lacks"), "{error}");
}

#[test]
fn a_declared_tag_with_an_email_is_refused() {
    let prefix = Builder::new()
        .push_int(702)
        .push_slice(b"P2Poolv2\x0fme@pool.io\x0f\x0fzg690z")
        .into_bytes();
    let error = check_declared_coinbase_tag(&prefix, test_share_chain_id()).unwrap_err();
    assert!(error.contains("'@'"), "{error}");
}

#[test]
fn a_declared_tag_with_an_address_in_its_node_line_is_refused() {
    let prefix = Builder::new()
        .push_int(702)
        .push_slice(b"P2Poolv2\x0f\x0fqpzry9x8gf2tvdw0s3jn54\x0fzg690z")
        .into_bytes();
    let error = check_declared_coinbase_tag(&prefix, test_share_chain_id()).unwrap_err();
    assert!(error.contains("may be an address"), "{error}");
}

#[test]
fn a_declared_tag_with_a_later_line_is_accepted() {
    let prefix = Builder::new()
        .push_int(702)
        .push_slice(b"P2Poolv2\x0fminer\x0f\x0fzg690z\x0flater")
        .into_bytes();
    assert_eq!(
        check_declared_coinbase_tag(&prefix, test_share_chain_id()),
        Ok(())
    );
}

#[test]
fn a_declared_tag_with_a_url_in_a_later_line_is_refused() {
    let prefix = Builder::new()
        .push_int(702)
        .push_slice(b"P2Poolv2\x0fminer\x0f\x0fzg690z\x0fpool.io/x")
        .into_bytes();
    let error = check_declared_coinbase_tag(&prefix, test_share_chain_id()).unwrap_err();
    assert!(error.contains("'/'"), "{error}");
}

#[test]
fn an_empty_declared_prefix_is_refused() {
    assert!(check_declared_coinbase_tag(&[], test_share_chain_id()).is_err());
}

#[tokio::test]
async fn the_node_tag_names_the_node_software_and_the_label() {
    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": {"subversion": "/Bitcoin Cash Node:29.2.1(EB32.0)/"},
            "error": null,
            "id": 0
        })))
        .mount(&mock_server)
        .await;
    let mut stratum = StratumConfig::new_for_test_default();
    stratum.pool_signature = Some("CyberAshven".to_string());
    stratum.node_label = Some("node1".to_string());
    let bitcoinrpc = BitcoinRpcConfig {
        url: mock_server.uri(),
        username: "p2pool".to_string(),
        password: "p2pool".to_string(),
    };

    let tag = coinbase_tag_for_node(
        &stratum.parse().unwrap(),
        &bitcoinrpc,
        test_share_chain_id(),
    )
    .await;

    assert_eq!(
        tag.as_bytes(),
        b"P2Poolv2\x0fCyberAshven\x0fBCHN 29.2.1 node1\x0fzg690z"
    );
}

/// A node without `getnetworkinfo` gets a node line of the label alone:
/// the start goes on.
#[tokio::test]
async fn a_node_without_getnetworkinfo_gets_the_label_alone() {
    let mock_server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": null,
            "error": {"code": -32601, "message": "Method not found"},
            "id": 0
        })))
        .mount(&mock_server)
        .await;
    let mut stratum = StratumConfig::new_for_test_default();
    stratum.pool_signature = Some("P2Poolv2".to_string());
    stratum.node_label = Some("node1".to_string());
    let bitcoinrpc = BitcoinRpcConfig {
        url: mock_server.uri(),
        username: "p2pool".to_string(),
        password: "p2pool".to_string(),
    };

    let tag = coinbase_tag_for_node(
        &stratum.parse().unwrap(),
        &bitcoinrpc,
        test_share_chain_id(),
    )
    .await;

    assert_eq!(tag.as_bytes(), b"P2Poolv2\x0f\x0fnode1\x0fzg690z");
}
