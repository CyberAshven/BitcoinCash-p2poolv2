// SPDX-FileCopyrightText: 2024-2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use super::*;
use crate::config::BitcoinCashNetwork;
use crate::shares::bch_asert::tests::{MAINNET_PAIRS, header};
use crate::shares::bch_headers::tests::ten_minute_header;
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

/// `length` linked headers from a first block at height 0, dated from
/// `first_time` 600 seconds apart, as a regtest chain has. The feeder trusts
/// its own BCHN, so they need no proof of work. `fork` changes every merkle
/// root, for a competing branch.
fn chain_dated_from(length: u8, fork: u8, first_time: u32) -> Vec<Header> {
    let mut chain: Vec<Header> = Vec::new();
    for height in 0..length {
        chain.push(Header {
            version: Version::from_consensus(0x2000_0000),
            prev_blockhash: chain
                .last()
                .map_or(BlockHash::all_zeros(), Header::block_hash),
            merkle_root: TxMerkleNode::from_byte_array([height ^ fork; 32]),
            time: first_time + u32::from(height) * 600,
            bits: CompactTarget::from_consensus(0x207f_ffff),
            nonce: u32::from(fork),
        });
    }
    chain
}

/// [`chain_dated_from`] in 2023, long before any switch.
fn chain_from_first_block(length: u8, fork: u8) -> Vec<Header> {
    chain_dated_from(length, fork, 1_700_000_000)
}

/// `chain`'s headers with their heights, from 0.
fn at_heights(chain: &[Header]) -> Vec<(u32, Header)> {
    chain
        .iter()
        .enumerate()
        .map(|(height, header)| (height as u32, *header))
        .collect()
}

async fn answer(server: &MockServer, request: serde_json::Value, result: serde_json::Value) {
    answer_after(server, request, result, Duration::ZERO).await;
}

async fn answer_after(
    server: &MockServer,
    request: serde_json::Value,
    result: serde_json::Value,
    delay: Duration,
) {
    Mock::given(method("POST"))
        .and(body_partial_json(request))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({
                    "result": result, "error": null, "id": 0
                }))
                .set_delay(delay),
        )
        .mount(server)
        .await;
}

/// The verbose `getblockheader` of `header`: `report`, such as its height.
async fn report(server: &MockServer, header: &Header, report: serde_json::Value) {
    answer(
        server,
        serde_json::json!({
            "method": "getblockheader",
            "params": [header.block_hash().to_string(), true]
        }),
        report,
    )
    .await;
}

/// BCHN at `tip`, serving the raw headers of `served` and their heights,
/// each answer `delay` late.
async fn bchn_after(tip: &Header, served: &[(u32, Header)], delay: Duration) -> MockServer {
    let server = MockServer::start().await;
    answer(
        &server,
        serde_json::json!({ "method": "getbestblockhash" }),
        serde_json::json!(tip.block_hash().to_string()),
    )
    .await;
    for (height, served) in served {
        answer_after(
            &server,
            serde_json::json!({
                "method": "getblockheader",
                "params": [served.block_hash().to_string(), false]
            }),
            serde_json::json!(hex::encode(serialize(served))),
            delay,
        )
        .await;
        answer_after(
            &server,
            serde_json::json!({
                "method": "getblockheader",
                "params": [served.block_hash().to_string(), true]
            }),
            serde_json::json!({ "height": height }),
            delay,
        )
        .await;
    }
    server
}

/// BCHN at `tip`, serving `served` at once.
async fn bchn(tip: &Header, served: &[(u32, Header)]) -> MockServer {
    bchn_after(tip, served, Duration::ZERO).await
}

/// What a store holds: known headers and the best-chain index.
#[derive(Default)]
struct Held {
    headers: HashMap<BlockHash, KnownBchHeader>,
    /// The heights of each write of headers, in the order written.
    writes: Vec<Vec<u32>>,
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
        held.writes
            .push(headers.iter().map(|known| known.height).collect());
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
    ten_minute_header(height, header)
}

fn feeder(server: &MockServer, held: &Arc<Mutex<Held>>) -> BchHeaderFeeder {
    BchHeaderFeeder::new(
        BitcoindRpcClient::new(&server.uri(), "user", "pass").unwrap(),
        store_holding(Arc::clone(held)),
        TickSchedule::TEN_MINUTES,
    )
}

/// Requests to BCHN for headers, raw and verbose.
async fn header_requests(server: &MockServer) -> usize {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|request| String::from_utf8_lossy(&request.body).contains("getblockheader"))
        .count()
}

#[tokio::test]
async fn a_new_tip_is_walked_back_to_the_known_header_and_indexed() {
    let [first, second, third] = mainnet_chain();
    let server = bchn(&third, &[(661_648, second), (661_649, third)]).await;
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
/// its first block at height 0, at ticks 0.
#[tokio::test]
async fn a_short_chain_is_learned_from_its_first_block() {
    let chain = chain_from_first_block(4, 0);
    let server = bchn(&chain[3], &at_heights(&chain)).await;
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
/// at the height BCHN gives the tip: here the two headers 1,200 ticks span,
/// after walking the ten below them that their next intervals read.
#[tokio::test]
async fn an_empty_store_learns_a_bounded_stretch_anchored_on_bchns_height() {
    let chain = chain_from_first_block(20, 0);
    let server = bchn(&chain[19], &at_heights(&chain)).await;
    let held = Arc::new(Mutex::new(Held::default()));
    let mut feeder = feeder(&server, &held);
    feeder.initial_header_ticks = 2 * 600;

    assert_eq!(feeder.sync_once().await, Ok(2));

    {
        let held = held.lock().unwrap();
        assert_eq!(held.headers.len(), 2);
        assert_eq!(held.headers[&chain[19].block_hash()], known(19, chain[19]));
        assert_eq!(held.headers[&chain[18].block_hash()], known(18, chain[18]));
    }
    // The walk fetched the two, the ten below the older one and one more,
    // and asked for the tip's height once.
    assert_eq!(header_requests(&server).await, 1 + 13);
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
    let server = bchn(&chain[2], &at_heights(&chain)).await;
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
    let server = bchn(&chain[2], &at_heights(&chain)).await;
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
        TickSchedule::TEN_MINUTES,
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

    assert_eq!(
        header_requests(&server).await,
        1 + 3,
        "one walk of three headers, not two"
    );
}

/// Two walks at once, as the feeder and the template relay start them on a
/// new store, fetch each header from BCHN once: the second waits and finds
/// them known.
#[tokio::test]
async fn walks_at_once_fetch_each_header_once() {
    let chain = chain_from_first_block(3, 0);
    let server = bchn(&chain[2], &at_heights(&chain)).await;
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
    assert_eq!(header_requests(&server).await, 1 + 3);
}

/// The round before share sync learns and indexes BCHN's chain.
#[tokio::test]
async fn the_first_sync_learns_bchns_chain() {
    let chain = chain_from_first_block(3, 0);
    let server = bchn(&chain[2], &at_heights(&chain)).await;
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
        TickSchedule::TEN_MINUTES,
    );
    absent.first_sync(Duration::from_secs(10)).await;

    assert!(held.lock().unwrap().headers.is_empty());
}

/// On one-minute blocks a store with nothing to link to learns until its
/// headers span the ticks it needs: 3 one-minute blocks for 180 ticks,
/// where one ten-minute block spans them. Mainnet's blocks dated after its
/// activation time switch by their own median time past, and the walk counts
/// ticks down from the ticks the node reports for its tip.
#[tokio::test]
async fn an_empty_store_on_one_minute_blocks_learns_until_the_span_is_covered() {
    for (first_time, tip_ticks, learned, interval) in [
        (1_810_400_000, 999_999_960, 3, 60),
        (1_700_000_000, 11_400, 1, 600),
    ] {
        let chain = chain_dated_from(20, 0, first_time);
        let server = bchn(&chain[19], &[]).await;
        for (height, header) in at_heights(&chain) {
            answer(
                &server,
                serde_json::json!({
                    "method": "getblockheader",
                    "params": [header.block_hash().to_string(), false]
                }),
                serde_json::json!(hex::encode(serialize(&header))),
            )
            .await;
            let reported = if height == 19 {
                serde_json::json!({ "height": height, "ticks": tip_ticks })
            } else {
                serde_json::json!({ "height": height })
            };
            report(&server, &header, reported).await;
        }
        let held = Arc::new(Mutex::new(Held::default()));
        let mut feeder = feeder(&server, &held);
        feeder.initial_header_ticks = 180;
        feeder.tick_schedule = TickSchedule::for_network(BitcoinCashNetwork::Mainnet);

        assert_eq!(feeder.sync_once().await, Ok(learned), "{first_time}");

        let held = held.lock().unwrap();
        for (depth, header) in chain.iter().rev().take(learned).enumerate() {
            let stored = held.headers[&header.block_hash()];
            assert_eq!(stored.height, 19 - depth as u32);
            assert_eq!(stored.ticks, tip_ticks - interval * depth as i64);
            assert_eq!(stored.next_interval, interval);
        }
    }
}

/// A new store's walk stores the tip's region first and the rest in chunks
/// as it goes back, newest first, each header with the ticks counted down
/// from the tip, down to the chain's first block, where they meet the ticks
/// worked out up from it.
#[tokio::test]
async fn a_first_walk_stores_the_tip_region_first_in_chunks() {
    let chain = chain_from_first_block(40, 0);
    let server = bchn(&chain[39], &at_heights(&chain)).await;
    let held = Arc::new(Mutex::new(Held::default()));
    let mut feeder = feeder(&server, &held);
    feeder.store_chunk = 5;

    assert_eq!(feeder.learn_from(chain[39].block_hash()).await, Ok(40));

    let held = held.lock().unwrap();
    // Each chunk is stored once the ten headers below it are walked; the
    // last write is everything from the first block up.
    let expected: Vec<Vec<u32>> = vec![
        (35..=39).rev().collect(),
        (30..=34).rev().collect(),
        (25..=29).rev().collect(),
        (20..=24).rev().collect(),
        (15..=19).rev().collect(),
        (0..=14).collect(),
    ];
    assert_eq!(held.writes, expected);
    for (height, header) in chain.iter().enumerate() {
        assert_eq!(
            held.headers[&header.block_hash()],
            known(height as u32, *header)
        );
    }
}

/// While a first walk goes on, the template relay's block, the tip, is
/// stored with the first chunk, and the relay's own walk of it returns
/// then: the first walk lets it in between chunks instead of holding the
/// lock to the end.
#[tokio::test]
async fn the_relay_is_not_held_up_by_a_first_walk() {
    let chain = chain_from_first_block(60, 0);
    let server = bchn_after(&chain[59], &at_heights(&chain), Duration::from_millis(20)).await;
    let held = Arc::new(Mutex::new(Held::default()));
    let mut walker = feeder(&server, &held);
    walker.store_chunk = 5;
    let relay = feeder(&server, &held);
    let tip = chain[59].block_hash();

    let walk = tokio::spawn(async move { walker.learn_from(tip).await });
    // Let the first walk take the lock before the relay asks.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(relay.learn_from(tip).await, Ok(0));
    assert!(held.lock().unwrap().headers.contains_key(&tip));
    assert!(!walk.is_finished(), "the relay waited for the whole walk");

    assert_eq!(walk.await.unwrap(), Ok(60));
    assert_eq!(held.lock().unwrap().headers.len(), 60);
}

/// What tracing writes, for tests that read the log.
#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Log {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A node reporting a next interval P2Pool does not work out gets an error
/// in the log, and P2Pool keeps its own value: the node is never followed.
#[tokio::test]
async fn a_disagreeing_node_report_is_logged_and_ignored() {
    let log = Log::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    let _default = tracing::subscriber::set_default(subscriber);
    let chain = chain_from_first_block(4, 0);
    let server = bchn(&chain[3], &[]).await;
    for (height, header) in at_heights(&chain) {
        answer(
            &server,
            serde_json::json!({
                "method": "getblockheader",
                "params": [header.block_hash().to_string(), false]
            }),
            serde_json::json!(hex::encode(serialize(&header))),
        )
        .await;
        report(
            &server,
            &header,
            serde_json::json!({
                "height": height,
                "ticks": i64::from(height) * 600,
                "target_time": 600,
                "next_target_time": 60
            }),
        )
        .await;
    }
    let held = Arc::new(Mutex::new(Held::default()));

    assert_eq!(feeder(&server, &held).sync_once().await, Ok(4));

    assert_eq!(
        held.lock().unwrap().headers[&chain[3].block_hash()],
        known(3, chain[3])
    );
    let logged = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    assert!(logged.contains("ERROR"), "{logged}");
    assert!(
        logged.contains("next_target_time 60") && logged.contains("works out 600"),
        "{logged}"
    );
    assert!(!logged.contains("target_time 600 "), "{logged}");
}

/// The cross-check compares each value the node reports and skips those it
/// does not.
#[test]
fn report_disagreements_name_each_field_that_differs() {
    let tip = KnownBchHeader {
        height: 332_451,
        header: header(MAINNET_PAIRS[0].1),
        ticks: 199_470_060,
        next_interval: 60,
    };
    let summary = |ticks, target_time, next_target_time| BlockHeaderSummary {
        height: 332_451,
        next_block_size_limit: None,
        ticks,
        target_time,
        next_target_time,
    };
    assert!(report_disagreements(&summary(None, None, None), &tip, Some(60)).is_empty());
    assert!(
        report_disagreements(
            &summary(Some(199_470_060), Some(60), Some(60)),
            &tip,
            Some(60)
        )
        .is_empty()
    );
    let disagreements = report_disagreements(
        &summary(Some(199_470_600), Some(600), Some(60)),
        &tip,
        Some(60),
    );
    assert_eq!(disagreements.len(), 2);
    assert!(disagreements[0].contains("ticks 199470600"));
    assert!(disagreements[1].contains("target_time 600"));
    // An own interval P2Pool cannot tell is not compared.
    assert!(report_disagreements(&summary(None, Some(600), None), &tip, None).is_empty());
}
