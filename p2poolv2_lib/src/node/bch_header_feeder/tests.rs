// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use crate::shares::bch_asert::tests::{MAINNET_PAIRS, header};
use bitcoin::block::Version;
use bitcoin::consensus::serialize;
use bitcoin::{CompactTarget, TxMerkleNode};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use wiremock::matchers::{body_partial_json, method};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Three real consecutive mainnet headers: 661,647, 661,648 and 661,649.
fn mainnet_chain() -> [Header; 3] {
    [
        header(MAINNET_PAIRS[0].1),
        header(MAINNET_PAIRS[1].1),
        header(MAINNET_PAIRS[1].2),
    ]
}

/// `length` linked headers from a first block at height 0, as a regtest
/// chain has. The feeder trusts its own BCHN, so they need no proof of work.
/// `fork` changes every merkle root, for a competing branch.
fn chain_from_first_block(length: u8, fork: u8) -> Vec<Header> {
    let mut chain: Vec<Header> = Vec::new();
    for height in 0..length {
        chain.push(Header {
            version: Version::from_consensus(0x2000_0000),
            prev_blockhash: chain
                .last()
                .map_or(BlockHash::all_zeros(), Header::block_hash),
            merkle_root: TxMerkleNode::from_byte_array([height ^ fork; 32]),
            time: 1_700_000_000 + u32::from(height) * 600,
            bits: CompactTarget::from_consensus(0x207f_ffff),
            nonce: u32::from(fork),
        });
    }
    chain
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

/// What a store holds: known headers and the best-chain index.
#[derive(Default)]
struct Held {
    headers: HashMap<BlockHash, KnownBchHeader>,
    index: BTreeMap<u32, BlockHash>,
    tip: Option<u32>,
    index_writes: usize,
}

/// A mocked store backed by `held`, behaving as the RocksDB store does.
fn store_holding(held: Arc<Mutex<Held>>) -> ChainStoreHandle {
    let mut store = ChainStoreHandle::default();
    let reads = Arc::clone(&held);
    store
        .expect_get_bch_header()
        .returning(move |hash| Ok(reads.lock().unwrap().headers.get(hash).copied()));
    let writes = Arc::clone(&held);
    store.expect_add_bch_headers().returning(move |headers| {
        let mut held = writes.lock().unwrap();
        for known in headers {
            held.headers.insert(known.block_hash(), known);
        }
        Ok(())
    });
    let index_reads = Arc::clone(&held);
    store
        .expect_get_bch_best_hash()
        .returning(move |height| Ok(index_reads.lock().unwrap().index.get(&height).copied()));
    let index_writes = Arc::clone(&held);
    store
        .expect_set_bch_best_chain()
        .returning(move |updates, tip| {
            let mut held = index_writes.lock().unwrap();
            held.index.retain(|height, _| *height <= tip);
            held.index.extend(updates);
            held.tip = Some(tip);
            held.index_writes += 1;
            Ok(())
        });
    store
}

fn known(height: u32, header: Header) -> KnownBchHeader {
    KnownBchHeader { height, header }
}

fn feeder(server: &MockServer, held: &Arc<Mutex<Held>>) -> BchHeaderFeeder {
    BchHeaderFeeder::new(
        BitcoindRpcClient::new(&server.uri(), "user", "pass").unwrap(),
        store_holding(Arc::clone(held)),
    )
}

#[tokio::test]
async fn a_new_tip_is_walked_back_to_the_known_header_and_indexed() {
    let [first, second, third] = mainnet_chain();
    let server = bchn(&third, &[second, third]).await;
    let held = Arc::new(Mutex::new(Held::default()));
    held.lock()
        .unwrap()
        .headers
        .insert(first.block_hash(), known(661_647, first));

    assert_eq!(feeder(&server, &held).sync_once().await, Ok(2));

    let held = held.lock().unwrap();
    assert_eq!(held.headers[&second.block_hash()], known(661_648, second));
    assert_eq!(held.headers[&third.block_hash()], known(661_649, third));
    assert_eq!(held.tip, Some(661_649));
    assert_eq!(held.index[&661_649], third.block_hash());
    assert_eq!(held.index[&661_648], second.block_hash());
    assert_eq!(held.index[&661_647], first.block_hash());
}

/// A tip already known and indexed costs no write.
#[tokio::test]
async fn a_known_indexed_tip_writes_nothing() {
    let chain = chain_from_first_block(3, 0);
    let server = bchn(&chain[2], &[]).await;
    let held = Arc::new(Mutex::new(Held::default()));
    {
        let mut held = held.lock().unwrap();
        for (height, header) in chain.iter().enumerate() {
            held.headers
                .insert(header.block_hash(), known(height as u32, *header));
            held.index.insert(height as u32, header.block_hash());
        }
        held.tip = Some(2);
    }

    assert_eq!(feeder(&server, &held).sync_once().await, Ok(0));
    assert_eq!(held.lock().unwrap().index_writes, 0);
}

/// A chain shorter than the initial walk, as on regtest, is learned down to
/// its first block at height 0.
#[tokio::test]
async fn a_short_chain_is_learned_from_its_first_block() {
    let chain = chain_from_first_block(4, 0);
    let server = bchn(&chain[3], &chain).await;
    let held = Arc::new(Mutex::new(Held::default()));

    assert_eq!(feeder(&server, &held).sync_once().await, Ok(4));

    let held = held.lock().unwrap();
    for (height, header) in chain.iter().enumerate() {
        assert_eq!(
            held.headers[&header.block_hash()],
            known(height as u32, *header)
        );
        assert_eq!(held.index[&(height as u32)], header.block_hash());
    }
    assert_eq!(held.tip, Some(3));
}

/// A store that knows no header learns a bounded stretch back from the tip,
/// anchored on the height BCHN gives the oldest header it walked.
#[tokio::test]
async fn an_empty_store_learns_a_bounded_stretch_anchored_on_bchns_height() {
    let [_, second, third] = mainnet_chain();
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
    let held = Arc::new(Mutex::new(Held::default()));
    let mut feeder = feeder(&server, &held);
    feeder.initial_headers = 2;

    assert_eq!(feeder.sync_once().await, Ok(2));

    let held = held.lock().unwrap();
    assert_eq!(held.headers[&second.block_hash()], known(661_648, second));
    assert_eq!(held.headers[&third.block_hash()], known(661_649, third));
}

/// BCHN reorganises onto a branch this node already learned from a peer:
/// nothing new to fetch, but the index moves to the branch.
#[tokio::test]
async fn a_reorg_onto_a_known_branch_moves_the_index() {
    let main = chain_from_first_block(4, 0);
    let mut branch = main[..2].to_vec();
    // A competing block 2 and 3 on block 1.
    for (height, header) in chain_from_first_block(4, 0x55)
        .into_iter()
        .enumerate()
        .skip(2)
    {
        let prev = branch.last().unwrap().block_hash();
        branch.push(Header {
            prev_blockhash: prev,
            ..header
        });
        assert_eq!(branch.len(), height + 1);
    }
    let server = bchn(&branch[3], &[]).await;
    let held = Arc::new(Mutex::new(Held::default()));
    {
        let mut held = held.lock().unwrap();
        for chain in [&main, &branch] {
            for (height, header) in chain.iter().enumerate() {
                held.headers
                    .insert(header.block_hash(), known(height as u32, *header));
            }
        }
        for (height, header) in main.iter().enumerate() {
            held.index.insert(height as u32, header.block_hash());
        }
        held.tip = Some(3);
    }

    assert_eq!(feeder(&server, &held).sync_once().await, Ok(0));

    let held = held.lock().unwrap();
    assert_eq!(held.index[&3], branch[3].block_hash());
    assert_eq!(held.index[&2], branch[2].block_hash());
    assert_eq!(held.index[&1], main[1].block_hash());
    assert_eq!(held.tip, Some(3));
}

/// BCHN reorganises onto a shorter branch: heights above its tip leave the
/// index.
#[tokio::test]
async fn a_reorg_onto_a_shorter_branch_trims_the_index() {
    let main = chain_from_first_block(4, 0);
    let server = bchn(&main[2], &[]).await;
    let held = Arc::new(Mutex::new(Held::default()));
    {
        let mut held = held.lock().unwrap();
        for (height, header) in main.iter().enumerate() {
            held.headers
                .insert(header.block_hash(), known(height as u32, *header));
            held.index.insert(height as u32, header.block_hash());
        }
        held.tip = Some(3);
    }

    assert_eq!(feeder(&server, &held).sync_once().await, Ok(0));

    let held = held.lock().unwrap();
    assert_eq!(held.tip, Some(2));
    assert_eq!(held.index.get(&3), None);
    assert_eq!(held.index[&2], main[2].block_hash());
}

/// `learn_from` walks back from a block that is not BCHN's best, such as a
/// new template's previous block, without touching the index.
#[tokio::test]
async fn learn_from_walks_back_from_any_block_bchn_knows() {
    let chain = chain_from_first_block(3, 0);
    let server = bchn(&chain[2], &chain).await;
    let held = Arc::new(Mutex::new(Held::default()));

    assert_eq!(
        feeder(&server, &held)
            .learn_from(chain[1].block_hash())
            .await,
        Ok(2)
    );

    let held = held.lock().unwrap();
    assert_eq!(held.headers.len(), 2);
    assert_eq!(held.headers[&chain[1].block_hash()], known(1, chain[1]));
    assert_eq!(held.index_writes, 0);
}

/// The relay learns each new template's previous block before passing the
/// template on, and asks BCHN only when the previous block changes.
#[tokio::test]
async fn the_template_relay_learns_a_new_templates_parent_first() {
    use crate::stratum::work::block_template::BlockTemplate;
    let chain = chain_from_first_block(3, 0);
    let server = bchn(&chain[2], &chain).await;
    let held = Arc::new(Mutex::new(Held::default()));
    let mut template: BlockTemplate = serde_json::from_str(include_str!(
        "../../../../bitcoindrpc/test_data/bchn-regtest-template.json"
    ))
    .unwrap();
    template.previousblockhash = chain[2].block_hash().to_string();
    let template = Arc::new(template);

    let (input_tx, input_rx) = tokio::sync::mpsc::channel(4);
    let (output_tx, mut output_rx) = tokio::sync::mpsc::channel(4);
    let relay = tokio::spawn(relay_templates_learning_prevs(
        BitcoindRpcClient::new(&server.uri(), "user", "pass").unwrap(),
        store_holding(Arc::clone(&held)),
        input_rx,
        output_tx,
    ));
    for _ in 0..2 {
        input_tx
            .send(NotifyCmd::SendToAll {
                template: Arc::clone(&template),
            })
            .await
            .unwrap();
        let relayed = output_rx.recv().await.unwrap();
        assert!(matches!(relayed, NotifyCmd::SendToAll { .. }));
        assert_eq!(held.lock().unwrap().headers.len(), 3);
    }
    drop(input_tx);
    relay.await.unwrap();

    let requests = server.received_requests().await.unwrap();
    let header_requests = requests
        .iter()
        .filter(|request| String::from_utf8_lossy(&request.body).contains("getblockheader"))
        .count();
    assert_eq!(header_requests, 3, "one walk of three headers, not two");
}

/// Two walks at once, as the feeder and the template relay start them on a
/// new store, fetch each header from BCHN once: the second waits and finds
/// them known.
#[tokio::test]
async fn walks_at_once_fetch_each_header_once() {
    let chain = chain_from_first_block(3, 0);
    let server = bchn(&chain[2], &chain).await;
    let held = Arc::new(Mutex::new(Held::default()));
    let first = feeder(&server, &held);
    let second = feeder(&server, &held);

    let (first_added, second_added) = tokio::join!(
        first.learn_from(chain[2].block_hash()),
        second.learn_from(chain[2].block_hash())
    );

    let mut added = [first_added.unwrap(), second_added.unwrap()];
    added.sort_unstable();
    assert_eq!(added, [0, 3]);
    let requests = server.received_requests().await.unwrap();
    let header_requests = requests
        .iter()
        .filter(|request| String::from_utf8_lossy(&request.body).contains("getblockheader"))
        .count();
    assert_eq!(header_requests, 3);
}

/// The round before share sync learns and indexes BCHN's chain.
#[tokio::test]
async fn the_first_sync_learns_bchns_chain() {
    let chain = chain_from_first_block(3, 0);
    let server = bchn(&chain[2], &chain).await;
    let held = Arc::new(Mutex::new(Held::default()));

    feeder(&server, &held)
        .first_sync(Duration::from_secs(10))
        .await;

    let held = held.lock().unwrap();
    assert_eq!(held.headers.len(), 3);
    assert_eq!(held.tip, Some(2));
}

/// A BCHN that does not answer in time, or at all, never holds up the node:
/// the first round gives up and learns nothing.
#[tokio::test]
async fn the_first_sync_gives_up_on_a_slow_or_absent_bchn() {
    let slow = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"result": null, "error": null, "id": 0}))
                .set_delay(Duration::from_secs(30)),
        )
        .mount(&slow)
        .await;
    let held = Arc::new(Mutex::new(Held::default()));
    let started = std::time::Instant::now();
    feeder(&slow, &held)
        .first_sync(Duration::from_millis(200))
        .await;
    assert!(started.elapsed() < Duration::from_secs(10));

    let absent = BchHeaderFeeder::new(
        BitcoindRpcClient::new("http://127.0.0.1:9", "user", "pass").unwrap(),
        store_holding(Arc::clone(&held)),
    );
    absent.first_sync(Duration::from_secs(10)).await;

    assert!(held.lock().unwrap().headers.is_empty());
}
