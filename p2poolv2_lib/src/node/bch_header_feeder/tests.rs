// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use crate::shares::bch_asert::tests::{MAINNET_PAIRS, header};
use bitcoin::consensus::serialize;
use wiremock::matchers::{body_partial_json, method};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Three real consecutive mainnet headers: 661,647, 661,648 and 661,649.
fn chain() -> [Header; 3] {
    [
        header(MAINNET_PAIRS[0].1),
        header(MAINNET_PAIRS[1].1),
        header(MAINNET_PAIRS[1].2),
    ]
}

async fn answer(server: &MockServer, request: serde_json::Value, result: serde_json::Value) {
    Mock::given(method("POST"))
        .and(body_partial_json(request))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "result": result, "error": null, "id": 0
        })))
        .mount(server)
        .await;
}

/// BCHN at `tip`, serving the raw headers of `served`.
async fn bchn(tip: &Header, served: &[Header]) -> MockServer {
    let server = MockServer::start().await;
    answer(
        &server,
        serde_json::json!({ "method": "getbestblockhash" }),
        serde_json::json!(tip.block_hash().to_string()),
    )
    .await;
    for served in served {
        answer(
            &server,
            serde_json::json!({
                "method": "getblockheader",
                "params": [served.block_hash().to_string(), false]
            }),
            serde_json::json!(hex::encode(serialize(served))),
        )
        .await;
    }
    server
}

fn feeder(server: &MockServer, store: ChainStoreHandle) -> BchHeaderFeeder {
    BchHeaderFeeder::new(
        BitcoindRpcClient::new(&server.uri(), "user", "pass").unwrap(),
        store,
    )
}

/// A store that knows only `known`.
fn store_knowing(known: Option<KnownBchHeader>) -> ChainStoreHandle {
    let mut store = ChainStoreHandle::default();
    store
        .expect_get_bch_header()
        .returning(move |hash| Ok(known.filter(|known| known.block_hash() == *hash)));
    store
}

#[tokio::test]
async fn a_new_tip_is_walked_back_to_the_known_header_and_stored_oldest_first() {
    let [first, second, third] = chain();
    let server = bchn(&third, &[second, third]).await;
    let mut store = store_knowing(Some(KnownBchHeader {
        height: 661_647,
        header: first,
    }));
    store
        .expect_add_bch_headers()
        .withf(move |headers| {
            headers
                == &vec![
                    KnownBchHeader {
                        height: 661_648,
                        header: second,
                    },
                    KnownBchHeader {
                        height: 661_649,
                        header: third,
                    },
                ]
        })
        .times(1)
        .returning(|_| Ok(()));

    assert_eq!(feeder(&server, store).sync_once().await, Ok(2));
}

#[tokio::test]
async fn a_known_tip_adds_nothing() {
    let [_, _, third] = chain();
    let server = bchn(&third, &[]).await;
    let mut store = store_knowing(Some(KnownBchHeader {
        height: 661_649,
        header: third,
    }));
    store.expect_add_bch_headers().never();

    assert_eq!(feeder(&server, store).sync_once().await, Ok(0));
}

/// A store that knows no header learns a bounded stretch back from the tip,
/// anchored on the height BCHN gives the oldest header it walked.
#[tokio::test]
async fn an_empty_store_learns_a_bounded_stretch_anchored_on_bchns_height() {
    let [_, second, third] = chain();
    let server = bchn(&third, &[second, third]).await;
    answer(
        &server,
        serde_json::json!({
            "method": "getblockheader",
            "params": [second.block_hash().to_string(), true]
        }),
        serde_json::json!({ "height": 661_648 }),
    )
    .await;
    let mut store = store_knowing(None);
    store
        .expect_add_bch_headers()
        .withf(move |headers| {
            headers.len() == 2
                && headers[0].height == 661_648
                && headers[0].header == second
                && headers[1].height == 661_649
        })
        .times(1)
        .returning(|_| Ok(()));
    let mut feeder = feeder(&server, store);
    feeder.initial_headers = 2;

    assert_eq!(feeder.sync_once().await, Ok(2));
}
